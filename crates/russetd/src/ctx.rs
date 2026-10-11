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
//! request of W2 (handlers write `cx.config.get()?` instead of `cx.config`, `&*cx.accounts.get()?` instead of
//! `&cx.accounts`).
//!
//! **Who constructs them.** An area installs the constructor of its service with [`provide`] from its
//! `handlers/<ns>.rs::register` (the handler seam), for example W3's `Toolchains`; the factory gets a
//! [`ServiceEnv`] with the directories, the device slot and the other services. Without one the daemon uses
//! the F0 constructors of ARCHITECTURE §5.3 (`ConfigService::load`, `russet_secrets::open_default`,
//! `DebugService::new`, `AccountService::new`); `Toolchains` has none, so its slot answers
//! `core.notImplemented` until W3 provides it.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use russet_ipc::Router;
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
    /// Installed once, right after the daemon created every slot (so factories can name each other).
    factory: OnceLock<Factory<T>>,
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
    /// A slot whose constructor comes later ([`Service::install`]).
    fn deferred(name: &'static str) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                name,
                value: OnceLock::new(),
                broken: Mutex::new(None),
                factory: OnceLock::new(),
            }),
        }
    }

    /// Sets the constructor of a deferred slot (the first one wins).
    fn install(&self, factory: Factory<T>) {
        let _ = self.inner.factory.set(factory);
    }

    /// A service built by `factory` on the first [`Service::get`].
    pub fn lazy(name: &'static str, factory: impl Fn() -> Result<Arc<T>> + Send + Sync + 'static) -> Self {
        let s = Self::deferred(name);
        s.install(Box::new(factory));
        s
    }

    /// A service that is already built (tests, fakes).
    pub fn ready(name: &'static str, value: Arc<T>) -> Self {
        let s = Self::deferred(name);
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
        let Some(factory) = self.inner.factory.get() else {
            return Err(not_ready(self.inner.name, "nothing constructs it"));
        };
        match crate::daemon::panics::catch_expected(factory) {
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

    fn downgrade(&self) -> WeakService<T> {
        WeakService { inner: Arc::downgrade(&self.inner), name: self.inner.name }
    }
}

fn not_ready(name: &str, why: &str) -> Error {
    Error::new(codes::core::NOT_IMPLEMENTED, format!("the {name} service is not available yet: {why}"))
        .with_hint("Its workstream has not landed it yet (docs/PLAN.md); see ARCHITECTURE §8 for its wave.")
}

/// A slot seen from a factory: weak, so the slots and their factories never keep each other alive.
struct WeakService<T: ?Sized + Send + Sync + 'static> {
    inner: Weak<ServiceInner<T>>,
    name: &'static str,
}

impl<T: ?Sized + Send + Sync + 'static> Clone for WeakService<T> {
    fn clone(&self) -> Self {
        Self { inner: Weak::clone(&self.inner), name: self.name }
    }
}

impl<T: ?Sized + Send + Sync + 'static> WeakService<T> {
    fn get(&self) -> Result<Arc<T>> {
        let inner = self.inner.upgrade().ok_or_else(|| {
            Error::new(codes::ipc::CONNECTION_CLOSED, format!("the {} service is gone: the daemon stopped", self.name))
        })?;
        Service { inner }.get()
    }
}

/// What a service factory gets ([`provide`]): the daemon's directories, its device slot and the other
/// services, each constructed on first use.
#[derive(Clone)]
pub struct ServiceEnv {
    pub dirs: RussetDirs,
    pub devices: DeviceSlot,
    config: WeakService<russet_config::ConfigService>,
    secrets: WeakService<dyn russet_secrets::SecretStore>,
    toolchains: WeakService<russet_toolchain::Toolchains>,
    debug: WeakService<russet_debug::DebugService>,
    accounts: WeakService<russet_account::AccountService>,
}

impl fmt::Debug for ServiceEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceEnv").field("dirs", &self.dirs).finish_non_exhaustive()
    }
}

impl ServiceEnv {
    /// The configuration service.
    pub fn config(&self) -> Result<Arc<russet_config::ConfigService>> {
        self.config.get()
    }
    /// The secret store.
    pub fn secrets(&self) -> Result<Arc<dyn russet_secrets::SecretStore>> {
        self.secrets.get()
    }
    /// The toolchain manager.
    pub fn toolchains(&self) -> Result<Arc<russet_toolchain::Toolchains>> {
        self.toolchains.get()
    }
    /// The debug service.
    pub fn debug(&self) -> Result<Arc<russet_debug::DebugService>> {
        self.debug.get()
    }
    /// The account service.
    pub fn accounts(&self) -> Result<Arc<russet_account::AccountService>> {
        self.accounts.get()
    }
}

type ProvidedFactory<T> = Arc<dyn Fn(&ServiceEnv) -> Result<Arc<T>> + Send + Sync>;

/// The factories areas installed with [`provide`] (a router extension until the daemon takes it).
#[derive(Default)]
pub struct ServiceFactories {
    by_type: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl fmt::Debug for ServiceFactories {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceFactories").field("len", &self.by_type.len()).finish()
    }
}

impl ServiceFactories {
    fn take<T: ?Sized + Send + Sync + 'static>(&mut self) -> Option<ProvidedFactory<T>> {
        self.by_type.remove(&TypeId::of::<T>()).and_then(|b| b.downcast::<ProvidedFactory<T>>().ok()).map(|b| *b)
    }
}

/// Installs the constructor of a `DaemonCtx` service: `T` is one of `russet_config::ConfigService`,
/// `dyn russet_secrets::SecretStore`, `russet_toolchain::Toolchains`, `russet_debug::DebugService` and
/// `russet_account::AccountService`. Call it from the owning area's `handlers/<ns>.rs::register`; the factory
/// runs on the slot's first [`Service::get`], and an error (or a panic) is that call's error. The last
/// registration for a type wins.
pub fn provide<T: ?Sized + Send + Sync + 'static>(
    r: &mut Router<DaemonCtx>,
    factory: impl Fn(&ServiceEnv) -> Result<Arc<T>> + Send + Sync + 'static,
) {
    let f: ProvidedFactory<T> = Arc::new(factory);
    r.extensions_mut().get_or_default::<ServiceFactories>().by_type.insert(TypeId::of::<T>(), Box::new(f));
}

/// The five service slots of a daemon, wired to their factories.
pub(crate) struct Services {
    pub config: Service<russet_config::ConfigService>,
    pub secrets: Service<dyn russet_secrets::SecretStore>,
    pub toolchains: Service<russet_toolchain::Toolchains>,
    pub debug: Service<russet_debug::DebugService>,
    pub accounts: Service<russet_account::AccountService>,
}

/// `RUSSET_SECRETS_BACKEND` (`auto` without it).
fn secrets_config() -> russet_secrets::SecretsConfig {
    use russet_secrets::BackendChoice;
    let backend = match std::env::var("RUSSET_SECRETS_BACKEND").unwrap_or_default().as_str() {
        "keyring" => BackendChoice::Keyring,
        "passphrase" => BackendChoice::Passphrase,
        "env" => BackendChoice::Env,
        "memory" => BackendChoice::Memory,
        _ => BackendChoice::Auto,
    };
    russet_secrets::SecretsConfig { backend }
}

fn install<T: ?Sized + Send + Sync + 'static>(
    slot: &Service<T>,
    env: &ServiceEnv,
    provided: Option<ProvidedFactory<T>>,
    default: fn(&ServiceEnv) -> Result<Arc<T>>,
) {
    let env = env.clone();
    match provided {
        Some(f) => slot.install(Box::new(move || f(&env))),
        None => slot.install(Box::new(move || default(&env))),
    }
}

impl Services {
    /// Creates the slots and installs each one's factory: the provided one, else the F0 constructor.
    pub(crate) fn wire(dirs: &RussetDirs, devices: &DeviceSlot, mut provided: ServiceFactories) -> Self {
        let s = Self {
            config: Service::deferred("config"),
            secrets: Service::deferred("secrets"),
            toolchains: Service::deferred("toolchains"),
            debug: Service::deferred("debug"),
            accounts: Service::deferred("accounts"),
        };
        let env = ServiceEnv {
            dirs: dirs.clone(),
            devices: devices.clone(),
            config: s.config.downgrade(),
            secrets: s.secrets.downgrade(),
            toolchains: s.toolchains.downgrade(),
            debug: s.debug.downgrade(),
            accounts: s.accounts.downgrade(),
        };
        install(&s.config, &env, provided.take(), |env| Ok(Arc::new(russet_config::ConfigService::load(&env.dirs)?)));
        install(&s.secrets, &env, provided.take(), |env| russet_secrets::open_default(&env.dirs, &secrets_config()));
        install(&s.toolchains, &env, provided.take(), |env| {
            Ok(Arc::new(russet_toolchain::Toolchains::load(&env.dirs)?))
        });
        install(&s.debug, &env, provided.take(), |env| {
            let services: Arc<dyn russet_debug::DeviceServices> = Arc::new(env.devices.clone());
            Ok(Arc::new(russet_debug::DebugService::new(services, russet_debug::DebugConfig::default())))
        });
        install(&s.accounts, &env, provided.take(), |env| {
            let endpoints = russet_core::net::Endpoints::from_env()?;
            Ok(Arc::new(russet_account::AccountService::new(&env.dirs, env.secrets()?, endpoints)?))
        });
        if !provided.by_type.is_empty() {
            tracing::warn!(
                "{} service factories name no DaemonCtx slot (russetd::ctx::provide takes the five service types)",
                provided.by_type.len()
            );
        }
        s
    }
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
    dirs: RussetDirs,
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
    /// A slot for `backend` with the given live-device lock state; the device plane reads its configuration and
    /// keeps its state under `dirs`.
    pub fn new(backend: DeviceBackend, live: LiveState, dirs: RussetDirs) -> Self {
        Self {
            inner: Arc::new(DeviceSlotInner {
                backend,
                live: std::sync::Mutex::new(Some(live)),
                dirs,
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
                let cfg = russet_device::DeviceConfig::load(&inner.dirs)?;
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

    #[test]
    fn wired_slots_use_provided_factories_and_do_not_keep_each_other_alive() {
        let dirs = RussetDirs::under(std::path::Path::new("/nonexistent/russet-wire-test"));
        let devices =
            DeviceSlot::new(DeviceBackend::None, LiveState::NotLive, RussetDirs::under(std::path::Path::new("/nope")));
        let mut r = Router::<DaemonCtx>::new();
        provide::<russet_toolchain::Toolchains>(&mut r, |env| {
            // A factory reaches the other slots through its env (the config under a missing home still loads).
            let _config = env.config();
            Err(Error::new(codes::core::NOT_IMPLEMENTED, "toolchains are not wired in this test").with_hint("provided"))
        });
        let factories = r.extensions_mut().remove::<ServiceFactories>().expect("registered");
        let services = Services::wire(&dirs, &devices, factories);
        let e = services.toolchains.get().expect_err("the provided factory fails");
        assert_eq!(e.code(), codes::core::NOT_IMPLEMENTED);
        assert_eq!(e.hint(), Some("provided"));
        let weak = Arc::downgrade(&services.config.inner);
        drop(services);
        assert!(weak.upgrade().is_none(), "the factories hold their env weakly: no cycle");
    }

    #[tokio::test]
    async fn refused_slots_answer_3019_without_a_connector() {
        let slot = DeviceSlot::new(
            DeviceBackend::Live,
            LiveState::Refused(LiveRefusal::DevelopmentBuild),
            RussetDirs::under(std::path::Path::new("/nope")),
        );
        let e = slot.manager().await.expect_err("refused");
        assert_eq!(e.code(), codes::device::LIVE_BACKEND_REFUSED);
        let holder = LockHolder { pid: 7, home: "/w/h".into() };
        let slot = DeviceSlot::new(
            DeviceBackend::Live,
            LiveState::Refused(LiveRefusal::HeldElsewhere(Some(holder))),
            RussetDirs::under(std::path::Path::new("/nope")),
        );
        let e = slot.manager().await.expect_err("held elsewhere");
        assert_eq!(e.details().and_then(|d| d.get("holder")), Some(&json!({"pid": 7, "home": "/w/h"})));
        assert_eq!(russet_device::io::live_connectors_constructed(), 0);
        let none =
            DeviceSlot::new(DeviceBackend::None, LiveState::NotLive, RussetDirs::under(std::path::Path::new("/nope")));
        assert_eq!(none.manager().await.expect_err("no plane").code(), codes::device::USBMUX_UNAVAILABLE);
    }
}
