// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet doctor`: the [`Check`] trait, the registry, the report model and the host checks (core §5.7, §11).
//!
//! Every L3 domain crate and russet-mcp exports `pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>>`;
//! russetd's `doctor_registry.rs` (a seam file) concatenates them with this crate's host checks, russetd's daemon
//! checks (W2) and the configuration, secrets and toolchain checks of `handlers/doctor.rs` (W3) (ARCHITECTURE
//! §5.3) [D 100].
//!
//! Checks reach daemon services through [`CheckCx::extension`]: russetd inserts its handles (the config
//! service, the secret store, the toolchains, …), and a check downcasts to the type it knows. A check whose
//! service is missing reports `skip`.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};

pub mod host;

/// The `async_trait` attribute, re-exported so that crates implementing [`Check`] need no direct dependency on
/// async-trait (`#[russet_doctor::async_trait]`).
pub use async_trait::async_trait;

/// A check's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl Status {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skip => "skip",
        }
    }
}

/// Report grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Host,
    Daemon,
    Config,
    Secrets,
    Usbmux,
    Device,
    Toolchain,
    Account,
    Network,
    Platform,
}

impl Category {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Daemon => "daemon",
            Self::Config => "config",
            Self::Secrets => "secrets",
            Self::Usbmux => "usbmux",
            Self::Device => "device",
            Self::Toolchain => "toolchain",
            Self::Account => "account",
            Self::Network => "network",
            Self::Platform => "platform",
        }
    }
}

/// What a check needs before it can run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requires {
    pub daemon: bool,
    pub device: bool,
    pub network: bool,
    pub project: bool,
}

/// Type-erased handles to daemon services, keyed by type. Two sets are equal when they hold the same handles
/// (pointer identity), so `CheckCx` keeps F0's `PartialEq` and `Eq`.
#[derive(Clone, Default)]
pub struct Extensions {
    map: BTreeMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extensions").field("len", &self.map.len()).finish()
    }
}

impl PartialEq for Extensions {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len()
            && self.map.iter().all(|(k, v)| other.map.get(k).is_some_and(|o| Arc::ptr_eq(v, o)))
    }
}

impl Eq for Extensions {}

/// What a check runs against. Build it with `..CheckCx::default()`: W3 added fields to F0's
/// `{project, udid, online}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckCx {
    pub project: Option<PathBuf>,
    pub udid: Option<String>,
    pub online: bool,
    /// Russet's directories.
    pub dirs: Option<RussetDirs>,
    /// The daemon's device backend as `RUSSET_DEVICE_BACKEND` writes it (`live`, `scripted:<file>`, `none`, …),
    /// and whether a `live` backend is refused (a development build without `RUSSET_LANE=A`).
    pub device_backend: Option<String>,
    pub live_refused: bool,
    /// The usbmux address (`USBMUXD_SOCKET_ADDRESS`); `None` = the platform default.
    pub usbmux_address: Option<String>,
    /// `storage.min-free-space` in bytes; `None` = 5 GiB.
    pub min_free_space: Option<u64>,
    /// Daemon services (see the crate docs).
    pub extensions: Extensions,
}

impl CheckCx {
    /// Makes `value` reachable through [`extension`](Self::extension).
    pub fn insert<T: Any + Send + Sync>(&mut self, value: Arc<T>) {
        self.extensions.map.insert(TypeId::of::<T>(), value);
    }

    /// A handle inserted with [`insert`](Self::insert).
    pub fn extension<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.extensions.map.get(&TypeId::of::<T>()).cloned()?.downcast::<T>().ok()
    }
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub status: Status,
    pub summary: String,
    pub detail: Option<String>,
    /// The next step for the user, when one exists.
    pub hint: Option<String>,
}

impl CheckResult {
    fn new(status: Status, summary: impl Into<String>) -> Self {
        Self { status, summary: summary.into(), detail: None, hint: None }
    }

    /// A passing result.
    pub fn pass(summary: impl Into<String>) -> Self {
        Self::new(Status::Pass, summary)
    }

    /// A warning.
    pub fn warn(summary: impl Into<String>) -> Self {
        Self::new(Status::Warn, summary)
    }

    /// A failure.
    pub fn fail(summary: impl Into<String>) -> Self {
        Self::new(Status::Fail, summary)
    }

    /// A skipped check, with the reason.
    pub fn skip(summary: impl Into<String>) -> Self {
        Self::new(Status::Skip, summary)
    }

    /// Adds the next step.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Adds detail.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A failure (or warning) built from an error: its message and hint.
    pub fn from_error(status: Status, e: &Error) -> Self {
        let mut r = Self::new(status, format!("{} ({}): {}", e.code().as_str(), e.code().number(), e.message()));
        r.hint = e.hint().map(str::to_owned);
        r
    }
}

/// How a failing or warning check can be fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// No fix is known.
    None,
    /// [`Check::fix`] fixes it without root.
    Auto,
    /// A command the user runs (for example one that needs root).
    Command(String),
    /// The user acts; the hint says how.
    Manual,
}

/// What a check found, and how a failure or warning is fixed (ignored for passes and skips).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub result: CheckResult,
    pub fix: Fix,
}

impl Diagnosis {
    /// `result`, fixed as `fix` says.
    pub fn new(result: CheckResult, fix: Fix) -> Self {
        Self { result, fix }
    }
}

/// An environment check.
#[async_trait]
pub trait Check: Send + Sync {
    /// Stable id, for example `toolchain.linker`.
    fn id(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn category(&self) -> Category;
    fn requires(&self) -> Requires;
    async fn run(&self, cx: &CheckCx) -> CheckResult;
    /// The automatic fix. `doctor --fix` calls it for a failure or warning whose fix is [`Fix::Auto`], and for
    /// one whose fix is [`Fix::None`] (F0 checks implement `fix` without saying so); `core.notImplemented`, the
    /// default, means there is none.
    async fn fix(&self, _cx: &CheckCx) -> Result<()> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "no automatic fix").with_hint("Follow the check's hint instead."))
    }
    /// How a failure or warning of this check is fixed, whatever the cause.
    fn fix_kind(&self) -> Fix {
        Fix::None
    }
    /// Runs the check and says how what it found is fixed; the registry calls this. The default is
    /// [`run`](Self::run) with [`fix_kind`](Self::fix_kind). A check whose fix depends on the cause (an automatic
    /// install for one failure, a manual step for another) overrides it and implements `run` as
    /// `self.diagnose(cx).await.result`.
    async fn diagnose(&self, cx: &CheckCx) -> Diagnosis {
        Diagnosis::new(self.run(cx).await, self.fix_kind())
    }
}

/// Which checks to run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Check ids, or prefixes ending in `.` (`toolchain.`); empty = all.
    pub checks: Vec<String>,
    pub categories: Vec<Category>,
    /// Apply automatic fixes.
    pub fix: bool,
}

impl Filter {
    fn wants(&self, c: &dyn Check) -> bool {
        let by_id = self.checks.is_empty()
            || self.checks.iter().any(|w| w == c.id() || (w.ends_with('.') && c.id().starts_with(w.as_str())));
        let by_category = self.categories.is_empty() || self.categories.contains(&c.category());
        by_id && by_category
    }
}

/// One entry of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    pub id: String,
    pub title: String,
    pub category: Category,
    pub result: CheckResult,
    /// How to fix a failure or warning; [`Fix::Auto`] when this run applied the fix, whatever the result now.
    /// `None` for passes and skips that needed no fix.
    pub fix: Option<Fix>,
    /// Whether this run applied the automatic fix.
    pub fix_applied: bool,
    pub duration_ms: u64,
}

/// Counts per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub pass: u32,
    pub warn: u32,
    pub fail: u32,
    pub skip: u32,
}

/// The result of `doctor.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorReport {
    pub checks: Vec<CheckOutcome>,
}

impl DoctorReport {
    /// Counts per status.
    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for c in &self.checks {
            match c.result.status {
                Status::Pass => s.pass += 1,
                Status::Warn => s.warn += 1,
                Status::Fail => s.fail += 1,
                Status::Skip => s.skip += 1,
            }
        }
        s
    }

    /// The CLI exit status (core §11): 0 without failures (with `strict`, without warnings either), else 4.
    pub fn exit_code(&self, strict: bool) -> u8 {
        let s = self.summary();
        if s.fail > 0 || (strict && s.warn > 0) { 4 } else { 0 }
    }
}

/// How long one check may take before it is reported as failed.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// Registered checks.
#[derive(Default)]
pub struct Registry {
    checks: Vec<Arc<dyn Check>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, c: Arc<dyn Check>) {
        self.checks.push(c);
    }
    /// The ids of the registered checks, in registration order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.checks.iter().map(|c| c.id()).collect()
    }

    /// Runs the checks `filter` selects, concurrently, and reports them in registration order. Checks that need
    /// the network run only with `cx.online`. With `filter.fix`, a failing or warning check whose diagnosis offers
    /// an automatic fix (or no fix kind at all, see [`Check::fix`]) is fixed and run again.
    pub async fn run(&self, cx: &CheckCx, filter: &Filter) -> DoctorReport {
        let selected: Vec<&Arc<dyn Check>> = self.checks.iter().filter(|c| filter.wants(c.as_ref())).collect();
        let runs = selected.iter().map(|c| run_one(c.as_ref(), cx, filter.fix));
        DoctorReport { checks: futures::future::join_all(runs).await }
    }
}

async fn timed(check: &dyn Check, cx: &CheckCx) -> Diagnosis {
    match tokio::time::timeout(CHECK_TIMEOUT, check.diagnose(cx)).await {
        Ok(d) => d,
        Err(_) => Diagnosis::new(
            CheckResult::fail(format!("the check did not finish within {} s", CHECK_TIMEOUT.as_secs()))
                .with_hint("Run it alone with `russet doctor --check <id>` to see where it hangs."),
            Fix::Manual,
        ),
    }
}

fn is_problem(r: &CheckResult) -> bool {
    matches!(r.status, Status::Fail | Status::Warn)
}

async fn run_one(check: &dyn Check, cx: &CheckCx, fix: bool) -> CheckOutcome {
    let started = Instant::now();
    let skip = |why: &str| Diagnosis::new(CheckResult::skip(why), Fix::None);
    let mut d = if check.requires().network && !cx.online {
        skip("needs the network; run `russet doctor --online`")
    } else if check.requires().project && cx.project.is_none() {
        skip("no project; run it inside a project or pass --project")
    } else if check.requires().device && cx.udid.is_none() {
        skip("no device selected")
    } else {
        timed(check, cx).await
    };
    let mut fix_applied = false;
    if fix && is_problem(&d.result) && matches!(d.fix, Fix::Auto | Fix::None) {
        match check.fix(cx).await {
            Ok(()) => {
                fix_applied = true;
                d = timed(check, cx).await;
            }
            // The F0 default: this check has no automatic fix.
            Err(e) if d.fix == Fix::None && e.code() == codes::core::NOT_IMPLEMENTED => {}
            Err(e) => d.result.detail = Some(format!("the automatic fix failed: {}", e.message())),
        }
    }
    let fix =
        if fix_applied { Some(Fix::Auto) } else { Some(d.fix).filter(|k| *k != Fix::None && is_problem(&d.result)) };
    CheckOutcome {
        id: check.id().to_owned(),
        title: check.title().to_owned(),
        category: check.category(),
        result: d.result,
        fix,
        fix_applied,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The host checks (W3): `host.os`, `host.disk`, `host.memory`, `host.wsl`, `dirs.private` and
/// `usbmux.available`.
pub fn checks() -> Vec<Arc<dyn Check>> {
    host::checks()
}
