// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`DaemonCtx`]: what every handler gets (ARCHITECTURE §5.3) [D 86]. Every field is cheap to clone.
//!
//! **Domain services start lazily.** `config`, `secrets`, `toolchains`, `debug` and `accounts` are
//! [`Service`] slots, like [`DeviceSlot`] for the device plane: the service is constructed on the first
//! [`Service::get`], and a failure is an error for that call, never a daemon that cannot start. A daemon must
//! start with an invalid `config.toml` (so `doctor` can say so) or without a keyring, and until W3, W6 and W7
//! land their constructors (F0 stubs end in `todo!()`; `Toolchains` has none yet) the slots answer
//! `core.notImplemented`. ARCHITECTURE §5.3 types these fields `Arc<T>`; the slots are a contract change
//! request of W2 (handlers write `cx.config.get()?` instead of `cx.config`).

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use serde_json::json;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::approvals::ApprovalService;
use crate::audit::AuditLog;
use crate::daemon::DaemonState;
use crate::daemon::backend::DeviceBackend;
use crate::daemon::lock::{FileLock, LockHolder};
use crate::jobs::JobManager;
use crate::leases::LeaseTable;
use crate::perms::PermissionEngine;
use crate::subs::SubscriptionHub;
use crate::web::WebEndpoint;

/// The daemon context handed to every handler.
#[derive(Clone)]
pub struct DaemonCtx {
    pub dirs: RussetDirs,
    pub config: Service<russet_config::ConfigService>,
    pub secrets: Service<dyn russet_secrets::SecretStore>,
    pub toolchains: Service<russet_toolchain::Toolchains>,
    pub jobs: JobManager,
    pub leases: LeaseTable,
    pub subs: SubscriptionHub,
    pub perms: Arc<PermissionEngine>,
    pub approvals: ApprovalService,
    pub audit: AuditLog,
    pub web: WebEndpoint,
    pub devices: DeviceSlot,
    pub debug: Service<russet_debug::DebugService>,
    pub accounts: Service<russet_account::AccountService>,
    pub shutdown: CancellationToken,
    /// The daemon's identity, connections, logging and shutdown control (W2; not in ARCHITECTURE §5.3).
    pub daemon: Arc<DaemonState>,
}

type Factory<T> = Box<dyn Fn() -> Result<Arc<T>> + Send + Sync>;

struct ServiceInner<T: ?Sized> {
    name: &'static str,
    value: OnceLock<Arc<T>>,
    /// A constructor that panicked (an F0 stub) stays failed; ordinary errors are retried on the next call.
    broken: Mutex<Option<String>>,
    factory: Factory<T>,
}

/// A domain service constructed on first use (module docs).
pub struct Service<T: ?Sized + Send + Sync + 'static> {
    inner: Arc<ServiceInner<T>>,
}

impl<T: ?Sized + Send + Sync + 'static> Clone for Service<T> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<T: ?Sized + Send + Sync + 'static> fmt::Debug for Service<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Service")
            .field("name", &self.inner.name)
            .field("ready", &self.inner.value.get().is_some())
            .finish()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Service<T> {
    /// A service built by `factory` on the first [`Service::get`].
    pub fn lazy(name: &'static str, factory: impl Fn() -> Result<Arc<T>> + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                name,
                value: OnceLock::new(),
                broken: Mutex::new(None),
                factory: Box::new(factory),
            }),
        }
    }

    /// A service that is already built (tests, fakes).
    pub fn ready(name: &'static str, value: Arc<T>) -> Self {
        let s = Self::lazy(name, || Err(Error::internal("unreachable: the service was built up front")));
        let _ = s.inner.value.set(value);
        s
    }

    /// A service that cannot start, with `error`'s code and message on every call.
    pub fn unavailable(name: &'static str, error: impl Fn() -> Error + Send + Sync + 'static) -> Self {
        Self::lazy(name, move || Err(error()))
    }

    /// The service, constructing it on the first call.
    pub fn get(&self) -> Result<Arc<T>> {
        if let Some(v) = self.inner.value.get() {
            return Ok(Arc::clone(v));
        }
        if let Some(why) = self.inner.broken.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            return Err(not_ready(self.inner.name, why));
        }
        match crate::daemon::panics::catch_expected(|| (self.inner.factory)()) {
            Ok(Ok(v)) => Ok(Arc::clone(self.inner.value.get_or_init(|| v))),
            Ok(Err(e)) => Err(e),
            Err(payload) => {
                let why = crate::daemon::panics::not_implemented_message(payload.as_ref());
                *self.inner.broken.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why.clone());
                Err(not_ready(self.inner.name, &why))
            }
        }
    }

    /// The service if it has been constructed, without constructing it.
    pub fn try_get(&self) -> Option<Arc<T>> {
        self.inner.value.get().cloned()
    }

    /// The slot's name (`config`, `secrets`, …).
    pub fn name(&self) -> &'static str {
        self.inner.name
    }
}

fn not_ready(name: &str, why: &str) -> Error {
    Error::new(codes::core::NOT_IMPLEMENTED, format!("the {name} service is not available yet: {why}"))
        .with_hint("Its workstream has not landed it yet (docs/PLAN.md); see ARCHITECTURE §8 for its wave.")
}

/// Why the live backend cannot be used by this daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRefusal {
    /// A development build without `RUSSET_LANE=A` or `--allow-live-device`.
    DevelopmentBuild,
    /// Another daemon holds the host-wide live-device lock.
    HeldElsewhere(Option<LockHolder>),
}

/// The live-device lock state of a daemon.
#[derive(Debug)]
pub enum LiveState {
    /// The backend is not `live`.
    NotLive,
    Refused(LiveRefusal),
    /// This daemon holds the lock for its whole life.
    Held(FileLock),
}

/// [`LiveState`] without the lock itself (status reports, tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveStatus {
    NotLive,
    Refused(LiveRefusal),
    Held,
    /// The daemon has stopped and released the lock.
    Released,
}

struct DeviceSlotInner {
    backend: DeviceBackend,
    live: std::sync::Mutex<Option<LiveState>>,
    state_dir: std::path::PathBuf,
    manager: OnceCell<Arc<russet_device::DeviceManager>>,
}

/// The lazily started device plane: the backend, the live-device lock state and a
/// `OnceCell<Arc<DeviceManager>>` [D 83]. W2 owns it.
#[derive(Clone)]
pub struct DeviceSlot {
    inner: Arc<DeviceSlotInner>,
}

impl fmt::Debug for DeviceSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceSlot")
            .field("backend", &self.inner.backend.to_string())
            .field("live", &self.live_status())
            .field("started", &self.inner.manager.get().is_some())
            .finish()
    }
}

/// russet-debug reaches the device plane through the slot, which starts the manager on first use [D 83].
impl russet_debug::DeviceServices for DeviceSlot {}

impl DeviceSlot {
    /// A slot for `backend` with the given live-device lock state; device state goes under `state_dir`.
    pub fn new(backend: DeviceBackend, live: LiveState, state_dir: std::path::PathBuf) -> Self {
        Self {
            inner: Arc::new(DeviceSlotInner {
                backend,
                live: std::sync::Mutex::new(Some(live)),
                state_dir,
                manager: OnceCell::new(),
            }),
        }
    }

    /// The backend this daemon was started with.
    pub fn backend(&self) -> &DeviceBackend {
        &self.inner.backend
    }

    /// The live-device lock state.
    pub fn live_status(&self) -> LiveStatus {
        match &*self.inner.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            Some(LiveState::NotLive) => LiveStatus::NotLive,
            Some(LiveState::Refused(r)) => LiveStatus::Refused(r.clone()),
            Some(LiveState::Held(_)) => LiveStatus::Held,
            None => LiveStatus::Released,
        }
    }

    /// Releases the live-device lock (the daemon stops; ARCHITECTURE §3.3 item 5).
    pub fn release_lock(&self) {
        let mut live = self.inner.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*live, Some(LiveState::Held(_))) {
            *live = None;
        }
    }

    /// The guard every device method passes first: `device.liveBackendRefused` (3019) when this daemon may
    /// not use the live backend, with `details.holder` when another daemon holds the lock (ARCHITECTURE §3.3
    /// item 7). Never constructs a connector.
    pub fn check(&self) -> Result<()> {
        match self.live_status() {
            LiveStatus::NotLive | LiveStatus::Held => Ok(()),
            LiveStatus::Released => Err(Error::new(codes::device::LIVE_BACKEND_REFUSED, "the daemon is shutting down")
                .with_hint("Retry once the daemon has restarted.")
                .retryable(true)),
            LiveStatus::Refused(LiveRefusal::DevelopmentBuild) => Err(Error::new(
                codes::device::LIVE_BACKEND_REFUSED,
                "this development daemon refuses the live device backend",
            )
            .with_hint(
                "Lane D uses RUSSET_DEVICE_BACKEND=scripted:<scenario.toml> (or none); only the lane-A agent sets \
                 RUSSET_LANE=A (AGENTS.md §4).",
            )
            .with_details(json!({"reason": "development-build", "backend": "live"}))),
            LiveStatus::Refused(LiveRefusal::HeldElsewhere(holder)) => {
                Err(Error::new(codes::device::LIVE_BACKEND_REFUSED, "another daemon holds the live-device lock")
                    .with_hint(
                        "Only one daemon may own the phone; stop the holder (its pid and home are in the details).",
                    )
                    .with_details(json!({"reason": "held-elsewhere", "backend": "live", "holder": holder})))
            }
        }
    }

    /// The device manager, started on the first call that passes [`DeviceSlot::check`]; otherwise
    /// `device.liveBackendRefused` (3019, with `details.holder` when another daemon holds the lock).
    pub async fn manager(&self) -> Result<Arc<russet_device::DeviceManager>> {
        self.check()?;
        let inner = Arc::clone(&self.inner);
        self.inner
            .manager
            .get_or_try_init(|| async move {
                let cfg = russet_device::DeviceConfig { state_dir: inner.state_dir.clone() };
                match &inner.backend {
                    DeviceBackend::Live => {
                        let connector = russet_device::io::LiveConnector::new(&cfg)?;
                        russet_device::DeviceManager::start(cfg, Arc::new(connector)).await
                    }
                    DeviceBackend::None => Err(Error::new(
                        codes::device::USBMUX_UNAVAILABLE,
                        "this daemon has no device plane (RUSSET_DEVICE_BACKEND=none)",
                    )
                    .with_hint(
                        "Start the daemon with RUSSET_DEVICE_BACKEND=scripted:<scenario.toml> to use a fake device.",
                    )),
                    other => Err(Error::not_implemented(format!("the {other} device backend is not available yet"))
                        .with_hint("The scripted, replay and mock connectors come with russet-testkit (W8, W15).")),
                }
            })
            .await
            .cloned()
    }

    /// The device manager if it has started, without starting it.
    pub fn started(&self) -> Option<Arc<russet_device::DeviceManager>> {
        self.inner.manager.get().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_start_once_and_report_stub_panics_as_not_implemented() {
        let built = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let b = Arc::clone(&built);
        let s: Service<String> = Service::lazy("demo", move || {
            b.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Arc::new("ready".to_owned()))
        });
        assert!(s.try_get().is_none());
        assert_eq!(s.get().expect("builds").as_str(), "ready");
        assert_eq!(s.get().expect("cached").as_str(), "ready");
        assert_eq!(built.load(std::sync::atomic::Ordering::SeqCst), 1);

        let stub: Service<String> = Service::lazy("stub", || todo!("W3 (CORE-08)"));
        let e = stub.get().expect_err("a todo!() stub");
        assert_eq!(e.code(), codes::core::NOT_IMPLEMENTED);
        assert!(e.message().contains("W3 (CORE-08)"), "{}", e.message());
        assert_eq!(stub.get().expect_err("stays broken").code(), codes::core::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn refused_slots_answer_3019_without_a_connector() {
        let slot =
            DeviceSlot::new(DeviceBackend::Live, LiveState::Refused(LiveRefusal::DevelopmentBuild), "/nope".into());
        let e = slot.manager().await.expect_err("refused");
        assert_eq!(e.code(), codes::device::LIVE_BACKEND_REFUSED);
        let holder = LockHolder { pid: 7, home: "/w/h".into() };
        let slot = DeviceSlot::new(
            DeviceBackend::Live,
            LiveState::Refused(LiveRefusal::HeldElsewhere(Some(holder))),
            "/nope".into(),
        );
        let e = slot.manager().await.expect_err("held elsewhere");
        assert_eq!(e.details().and_then(|d| d.get("holder")), Some(&json!({"pid": 7, "home": "/w/h"})));
        assert_eq!(russet_device::io::live_connectors_constructed(), 0);
        let none = DeviceSlot::new(DeviceBackend::None, LiveState::NotLive, "/nope".into());
        assert_eq!(none.manager().await.expect_err("no plane").code(), codes::device::USBMUX_UNAVAILABLE);
    }
}
