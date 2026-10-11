// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build` handlers (§8.3). Owner: W5 (Wave 1), W16 (build.settings).
//!
//! The handlers are thin: they turn wire params into `russet_build` requests and domain results into the DTOs
//! of `russet_protocol::build`. `build.start` runs the build inside the request, which stays open until it
//! returns `BuildOutput` (ARCHITECTURE §7.6); wrapping it in a `job.*` job (events, `background: true`, the
//! project lock key) needs W2's JobManager (freeze F1; F0's has no API yet). A project whose
//! `[signing] bundle-id-policy` is `xtool-prefix` gets the default account's prefix ([`with_account_policy`]).
//! `build.settings` is W16's (Wave 2).

use std::future::Future;
use std::sync::Arc;

use russet_build::{BuildEnv, BuildRequest, Builder, CleanScope, NullSink, Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::proc::{CommandRunner, ProcessRunner};
use russet_core::progress::EventSink;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::build as p;
use russet_protocol::project::{ProjectStack, TargetKind};
use russet_sdk::DefaultSdkProvider;
use tokio_util::sync::CancellationToken;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::BuildStart, _, _>(build_start);
    r.request::<p::BuildPlan, _, _>(build_plan);
    r.request::<p::BuildSettings, _, _>(not_implemented::<p::BuildSettings>);
    r.request::<p::BuildClean, _, _>(build_clean);
    r.request::<p::BuildInspect, _, _>(build_inspect);
}

/// A wire path: absolute UTF-8 (ARCHITECTURE §7.3).
pub(crate) fn wire_path(s: &str, what: &str) -> Result<Utf8PathBuf> {
    let p = Utf8PathBuf::from(s);
    if p.is_absolute() {
        Ok(p)
    } else {
        Err(Error::new(codes::core::INVALID_ARGUMENT, format!("`{what}` must be an absolute path, not `{s}`"))
            .with_hint("Clients send absolute paths (ARCHITECTURE §7.3)."))
    }
}

fn utf8(p: &std::path::Path) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(p.to_owned()).map_err(|p| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("{} is not UTF-8", p.display()))
            .with_hint("Russet's directories need UTF-8 paths; set RUSSET_HOME to one.")
    })
}

/// The daemon's builder: Russet's data and cache directories, the default SDK order, the daemon's process runner
/// (for the builds and for the SDK provider's toolchain probes).
pub(crate) fn builder(cx: &DaemonCtx) -> Result<Builder> {
    let data = utf8(&cx.dirs.data)?;
    let cache = utf8(&cx.dirs.cache)?;
    // `default()`, not the unit literal: W2's F1 runner may carry state (the russet-spawner thread, [D 73]).
    #[allow(clippy::default_constructed_unit_structs)]
    let runner: Arc<dyn CommandRunner> = Arc::new(ProcessRunner::default());
    let sdk = Arc::new(DefaultSdkProvider::new(data.clone(), None, Arc::clone(&runner)));
    Builder::new(BuildEnv::new(sdk, data, cache, runner))
}

/// Completes a request's bundle-ID policy from the signing account: when the request names none and the project's
/// `[signing] bundle-id-policy` is `xtool-prefix` (PLAN §4.10's `russet.local.toml`), `XTL-<prefix>.<id>` needs the
/// identity prefix of the imported xtool account (ARCHITECTURE §16.5), which only the account service knows.
/// `prefix` is asked only then. Without a prefix the request is unchanged and the build answers
/// `build.bundleIdInvalid` with the import hint.
pub async fn with_account_policy<F, Fut>(b: &Builder, mut r: BuildRequest, prefix: F) -> Result<BuildRequest>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<String>>>,
{
    if b.needs_xtool_prefix(&r)
        && let Some(prefix) = prefix().await?
    {
        r.bundle_id_policy = Some(russet_build::BundleIdPolicy::XtoolPrefix { prefix });
    }
    Ok(r)
}

/// The default account's xtool identity prefix (W6's `Account::xtool_prefix()`: the first segment of the xtool
/// identity id, uppercased).
///
/// F0's `Account` has no fields to derive it from and `AccountService`'s bodies are `todo!()`, so until W6 merges
/// this answers `None`. With W6 (and W2's service slots) the body becomes
/// `Ok(cx.accounts.get()?.accounts().read().await.default_account().and_then(|a| a.xtool_prefix()))`.
pub(crate) async fn account_xtool_prefix(_cx: &DaemonCtx) -> Result<Option<String>> {
    Ok(None)
}

/// `build.start` / `build.plan` params as a domain request.
pub fn request_from(p: &p::BuildStartParams) -> Result<BuildRequest> {
    let mut r = BuildRequest::new(wire_path(&p.project, "project")?);
    r.target = p.target.clone();
    r.configuration = p.configuration.clone();
    r.overrides = p.overrides.clone().unwrap_or_default();
    r.clean = p.clean.unwrap_or(false);
    r.package = p.package.map(|k| match k {
        p::PackageKind::None => russet_build::PackageKind::None,
        p::PackageKind::IpaDev => russet_build::PackageKind::IpaDev,
        p::PackageKind::IpaStore => russet_build::PackageKind::IpaStore,
    });
    r.backend = p.backend.map(|b| match b {
        p::BuildBackend::Swiftbuild => russet_build::Backend::Swiftbuild,
        p::BuildBackend::Native => russet_build::Backend::Native,
        p::BuildBackend::Xtool => russet_build::Backend::Xtool,
        p::BuildBackend::Pif => russet_build::Backend::Pif,
    });
    r.bundle_id_policy = p.bundle_id_policy.clone().map(|b| match b {
        p::BundleIdPolicy::Exact => russet_build::BundleIdPolicy::Exact,
        p::BundleIdPolicy::TeamSuffix { team_id } => russet_build::BundleIdPolicy::TeamSuffix { team_id },
        p::BundleIdPolicy::XtoolPrefix { prefix } => russet_build::BundleIdPolicy::XtoolPrefix { prefix },
        p::BundleIdPolicy::Custom { map } => russet_build::BundleIdPolicy::Custom(map),
    });
    Ok(r)
}

/// The wire stack of a domain stack.
pub fn stack_dto(s: russet_build::Stack) -> ProjectStack {
    match s {
        russet_build::Stack::Swiftpm => ProjectStack::Swiftpm,
        russet_build::Stack::Flutter => ProjectStack::Flutter,
        russet_build::Stack::Xcode => ProjectStack::Xcode,
        russet_build::Stack::ReactNative => ProjectStack::ReactNative,
        russet_build::Stack::Maui => ProjectStack::Maui,
    }
}

fn kind_dto(k: russet_build::BundleKind) -> TargetKind {
    match k {
        russet_build::BundleKind::App => TargetKind::App,
        russet_build::BundleKind::AppExtension => TargetKind::AppExtension,
    }
}

fn bundle_dto(b: &russet_build::BundleInfo) -> p::BundleInfo {
    p::BundleInfo {
        path: b.path.to_string(),
        executable: b.executable.to_string(),
        bundle_id: b.bundle_id.clone(),
        original_bundle_id: b.original_bundle_id.clone(),
        kind: kind_dto(b.kind),
        product: b.product.clone(),
        version: b.version.clone(),
        build_number: b.build_number.clone(),
    }
}

/// A domain lint finding on the wire.
pub fn lint_dto(l: &russet_build::LintFinding) -> p::LintFinding {
    p::LintFinding {
        id: l.code.clone(),
        severity: match l.severity {
            russet_build::LintSeverity::Info => p::LintSeverity::Info,
            russet_build::LintSeverity::Warning => p::LintSeverity::Warning,
            russet_build::LintSeverity::Error => p::LintSeverity::Error,
        },
        path: l.path.as_ref().map(ToString::to_string),
        message: l.message.clone(),
        fixed: l.fixed,
    }
}

/// `BuildOutput` on the wire.
pub fn output_dto(o: &russet_build::BuildOutput) -> p::BuildOutput {
    p::BuildOutput {
        build_id: o.build_id.clone(),
        app: bundle_dto(&o.app),
        extensions: o.extensions.iter().map(bundle_dto).collect(),
        bundle_ids: o.bundle_ids.clone(),
        signing_plan: p::SigningPlan {
            items: o
                .signing_plan
                .items
                .iter()
                .map(|i| p::SignItem {
                    path: i.path.to_string(),
                    kind: match i.kind {
                        russet_build::SignKind::Bundle => p::SignKind::Bundle,
                        russet_build::SignKind::MachO => p::SignKind::MachO,
                    },
                    entitlements: i.entitlements.as_ref().map(ToString::to_string),
                    get_task_allow: i.get_task_allow,
                })
                .collect(),
        },
        dsyms: o.dsyms.iter().map(ToString::to_string).collect(),
        ipa: o.ipa.as_ref().map(ToString::to_string),
        report_path: o.report_path.to_string(),
        lints: o.lints.iter().map(lint_dto).collect(),
        duration_ms: u64::try_from(o.duration.as_millis()).unwrap_or(u64::MAX),
    }
}

/// `BuildPlan` on the wire.
pub fn plan_dto(plan: &russet_build::BuildPlan) -> p::BuildPlanResult {
    let sdk = &plan.sdk;
    p::BuildPlanResult {
        root: plan.root.to_string(),
        stack: stack_dto(plan.stack),
        product: plan.product.clone(),
        extensions: plan.extensions.clone(),
        configuration: plan.configuration.clone(),
        release: plan.release,
        backend: match plan.backend {
            russet_build::Backend::Swiftbuild => p::BuildBackend::Swiftbuild,
            russet_build::Backend::Native => p::BuildBackend::Native,
            russet_build::Backend::Xtool => p::BuildBackend::Xtool,
            russet_build::Backend::Pif => p::BuildBackend::Pif,
        },
        package: match plan.package {
            russet_build::PackageKind::None => p::PackageKind::None,
            russet_build::PackageKind::IpaDev => p::PackageKind::IpaDev,
            russet_build::PackageKind::IpaStore => p::PackageKind::IpaStore,
        },
        metadata: plan.metadata.as_str().to_owned(),
        sdk: p::PlanSdk {
            id: sdk.id.clone(),
            canonical_name: sdk.sdk_canonical_name.clone(),
            version: sdk.sdk_version.to_string(),
            build: sdk.sdk_build.clone(),
            xcode_version: sdk.xcode_version.to_string(),
            xcode_build: sdk.xcode_build.clone(),
            swift_required: sdk.swift_required.to_string(),
            bundle: sdk.bundle.to_string(),
        },
        deployment_target: plan.deployment_target.map(|v| v.to_string()),
        bundle_ids: plan.bundle_ids.clone(),
        app_path: plan.app_path.as_ref().map(ToString::to_string),
        report_path: plan.report_path.to_string(),
        stages: plan.stages.iter().map(|s| s.as_str().to_owned()).collect(),
        warnings: plan.warnings.clone(),
    }
}

/// `build.inspect`'s report on the wire.
pub fn inspect_dto(r: &russet_build::inspect::InspectReport) -> p::BuildInspectResult {
    p::BuildInspectResult {
        info_plist: r.info_plist_json(),
        mach_o: r
            .mach_o
            .iter()
            .map(|m| p::MachOSummary {
                path: m.path.clone(),
                archs: m.summary.archs.clone(),
                platform: m.summary.platform,
                minos: m.summary.minos.map(|v| v.to_string()),
                sdk: m.summary.sdk.map(|v| v.to_string()),
                tools: m.summary.tools.clone(),
                uuid: m.summary.uuid.map(|u| u.to_string()),
                rpaths: m.summary.rpaths.clone(),
                dylibs: m.summary.dylibs.clone(),
                encrypted: m.summary.encrypted,
            })
            .collect(),
        lints: r.lints.iter().map(lint_dto).collect(),
        dt: r.dt.clone(),
        car: None,
    }
}

/// Builds a project the way `build.start` does: the daemon's [`builder`], the account's bundle-ID prefix
/// ([`with_account_policy`]), then [`Builder::build`] with the caller's sink and cancellation.
///
/// This is the one entry point for in-daemon callers. The run pipeline's build step (W7's `LiveBuilder`) calls it
/// and maps the domain output: `out.app.path` is the unsigned `.app`, `out.app.bundle_id` the effective bundle ID,
/// `out.signing_plan` the inside-out items with their entitlements files and `get-task-allow`, and `out.built` what
/// the debugger needs (the executable, its UUIDs, the dSYMs, the SDK path for `target.sdk-path`).
pub async fn build_project(
    cx: &DaemonCtx,
    request: BuildRequest,
    sink: &dyn EventSink,
    cancel: CancellationToken,
) -> Result<russet_build::BuildOutput> {
    let b = builder(cx)?;
    let request = with_account_policy(&b, request, || account_xtool_prefix(cx)).await?;
    b.build(request, sink, cancel).await
}

async fn build_start(cx: DaemonCtx, rq: RequestCtx, p: p::BuildStartParams) -> Result<p::BuildOutput> {
    let out = build_project(&cx, request_from(&p)?, &NullSink, rq.cancel.clone()).await?;
    Ok(output_dto(&out))
}

async fn build_plan(cx: DaemonCtx, _rq: RequestCtx, p: p::BuildPlanParams) -> Result<p::BuildPlanResult> {
    let b = builder(&cx)?;
    let request = with_account_policy(&b, request_from(&p)?, || account_xtool_prefix(&cx)).await?;
    Ok(plan_dto(&b.plan(&request).await?))
}

async fn build_clean(cx: DaemonCtx, _rq: RequestCtx, p: p::BuildCleanParams) -> Result<p::BuildCleanResult> {
    let scope = match p.scope {
        p::CleanScope::Build => CleanScope::Build,
        p::CleanScope::All => CleanScope::All,
    };
    let report = builder(&cx)?.clean(&wire_path(&p.project, "project")?, scope).await?;
    Ok(p::BuildCleanResult { freed_bytes: report.freed_bytes })
}

async fn build_inspect(cx: DaemonCtx, _rq: RequestCtx, p: p::BuildInspectParams) -> Result<p::BuildInspectResult> {
    let path = wire_path(&p.path, "path")?;
    let b = builder(&cx)?;
    let report = tokio::task::spawn_blocking(move || b.inspect(&path))
        .await
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("inspect panicked: {e}")))??;
    Ok(inspect_dto(&report))
}

/// `russet package ipa <app> -o <ipa> [--store]` (build §9.3). No RPC method carries it yet (a contract change
/// is requested), so the CLI calls this in-process; packing a local bundle needs no daemon state.
pub fn package_ipa(app: &str, out: &str, store: bool) -> Result<russet_build::PackagedIpa> {
    let app = Utf8Path::new(app);
    russet_build::package_ipa(app, Utf8Path::new(out), store)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn params_become_requests() {
        let p = p::BuildStartParams {
            project: "/p".into(),
            configuration: Some("release".into()),
            package: Some(p::PackageKind::IpaDev),
            backend: Some(p::BuildBackend::Native),
            overrides: Some(BTreeMap::from([("A".to_owned(), "1".to_owned())])),
            clean: Some(true),
            bundle_id_policy: Some(p::BundleIdPolicy::TeamSuffix { team_id: "T".into() }),
            ..Default::default()
        };
        let r = request_from(&p).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(r.project, Utf8PathBuf::from("/p"));
        assert_eq!(r.package, Some(russet_build::PackageKind::IpaDev));
        assert_eq!(r.backend, Some(russet_build::Backend::Native));
        assert!(r.clean);
        assert_eq!(r.bundle_id_policy, Some(russet_build::BundleIdPolicy::TeamSuffix { team_id: "T".into() }));
        let relative = p::BuildStartParams { project: "rel".into(), ..Default::default() };
        assert_eq!(
            request_from(&relative).map(|_| ()).expect_err("relative paths are refused").code(),
            codes::core::INVALID_ARGUMENT
        );
    }

    /// A temporary project directory, removed on drop (russetd has no tempfile dev-dependency).
    struct TempProject(Utf8PathBuf);

    impl TempProject {
        fn new(files: &[(&str, &str)]) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("russetd-build-{}-{n}", std::process::id()));
            let dir = Utf8PathBuf::from_path_buf(dir).unwrap_or_else(|p| panic!("{}", p.display()));
            std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
            for (name, text) in files {
                std::fs::write(dir.join(name), text).unwrap_or_else(|e| panic!("{e}"));
            }
            Self(dir)
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_builder(root: &Utf8Path) -> Builder {
        #[allow(clippy::default_constructed_unit_structs)]
        let runner: Arc<dyn CommandRunner> = Arc::new(ProcessRunner::default());
        let sdk = Arc::new(DefaultSdkProvider::new(root.join("data"), None, Arc::clone(&runner)));
        Builder::new(BuildEnv::new(sdk, root.join("data"), root.join("cache"), runner))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// PLAN §4.10: `bundle-id-policy = "xtool-prefix"` in russet.local.toml and no policy in the request gives
    /// `XTL-<prefix>`, with the prefix from the account; other projects never ask the account service.
    #[tokio::test]
    async fn xtool_prefix_projects_get_the_account_prefix() {
        let toml = "[app]\nbundle-id = \"com.example.hello\"\n";
        let local = "[signing]\nbundle-id-policy = \"xtool-prefix\"\n";
        let project = TempProject::new(&[("russet.toml", toml), ("russet.local.toml", local)]);
        let b = test_builder(&project.0);
        let r = with_account_policy(&b, BuildRequest::new(&project.0), || async { Ok(Some("F00DCAFE".to_owned())) })
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.bundle_id_policy, Some(russet_build::BundleIdPolicy::XtoolPrefix { prefix: "F00DCAFE".into() }));
        // No account prefix: the request stays as it was (the build then answers build.bundleIdInvalid).
        let r = with_account_policy(&b, BuildRequest::new(&project.0), || async { Ok(None) })
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.bundle_id_policy, None);
        // Account errors surface.
        let e = with_account_policy(&b, BuildRequest::new(&project.0), || async {
            Err(Error::new(codes::secrets::LOCKED, "locked"))
        })
        .await
        .map(|_| ())
        .expect_err("the account error surfaces");
        assert_eq!(e.code(), codes::secrets::LOCKED);
        // A request with a policy, or a project without xtool-prefix, never asks.
        let asked = std::sync::atomic::AtomicBool::new(false);
        let ask = || async {
            asked.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(Some("F00DCAFE".to_owned()))
        };
        let mut explicit = BuildRequest::new(&project.0);
        explicit.bundle_id_policy = Some(russet_build::BundleIdPolicy::Exact);
        let r = with_account_policy(&b, explicit, ask).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.bundle_id_policy, Some(russet_build::BundleIdPolicy::Exact));
        let plain = TempProject::new(&[("russet.toml", toml)]);
        let r = with_account_policy(&b, BuildRequest::new(&plain.0), ask).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.bundle_id_policy, None);
        assert!(!asked.load(std::sync::atomic::Ordering::Relaxed), "the account service was asked");
    }

    #[test]
    fn outputs_convert_to_the_wire() {
        let app = russet_build::BundleInfo {
            path: "/p/A.app".into(),
            bundle_id: "a.b".into(),
            original_bundle_id: "a.b".into(),
            executable: "/p/A.app/A".into(),
            kind: russet_build::BundleKind::App,
            product: "A".into(),
            version: "1.0".into(),
            build_number: "3".into(),
        };
        let out = russet_build::BuildOutput {
            build_id: russet_build::new_job_id(),
            app: app.clone(),
            extensions: Vec::new(),
            bundle_ids: BTreeMap::from([("a.b".to_owned(), "a.b".to_owned())]),
            signing_plan: russet_build::SigningPlan {
                items: vec![russet_build::SignItem {
                    path: "/p/A.app".into(),
                    kind: russet_build::SignKind::Bundle,
                    entitlements: None,
                    get_task_allow: true,
                }],
            },
            dsyms: Vec::new(),
            ipa: Some("/p/A.ipa".into()),
            report_path: "/p/build-report.json".into(),
            lints: vec![russet_build::LintFinding {
                code: "L006".into(),
                severity: russet_build::LintSeverity::Warning,
                path: None,
                message: "m".into(),
                fixed: true,
            }],
            duration: std::time::Duration::from_millis(1500),
            built: russet_build::BuiltApp {
                app: app.path.clone(),
                executable: app.executable.clone(),
                uuids: Vec::new(),
                dsyms: Vec::new(),
                sdk_path: "/s".into(),
                swift_module_dirs: Vec::new(),
                flutter_framework: None,
            },
        };
        let dto = output_dto(&out);
        assert_eq!(dto.app.path, "/p/A.app");
        assert_eq!(dto.duration_ms, 1500);
        assert_eq!(dto.ipa.as_deref(), Some("/p/A.ipa"));
        assert_eq!(dto.lints[0].id, "L006");
        assert!(dto.signing_plan.items[0].get_task_allow);
        let json = serde_json::to_value(&dto).unwrap_or_default();
        assert_eq!(json["app"]["path"], "/p/A.app");
    }
}
