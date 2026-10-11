// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The pipeline's ports over the daemon's services ([`pipeline_for`]).
//!
//! - LLDB comes from the toolchain resolver (`Toolchains::resolve`, W3).
//! - Signing copies the build product and signs it with `russet_sign::sign` and the account store's identity
//!   (W6); the pipeline then reads the signed executable's CDHash for the install ledger itself.
//! - The device steps (install, launch, debugserver) go through `DaemonCtx.debug`'s `DeviceServices`, which
//!   russetd's `DeviceSlot` provides (W2, W4).
//! - Building and provisioning need contracts that are not on `main` yet: W5's build service (a `Builder` needs
//!   an `SdkProvider`, which F0 cannot construct) and W6's `ProvisionRequest` fields (freeze F2). Until they
//!   land, those two steps answer `core.notImplemented` with the card to wait for.
//! - The other adapters call functions that are still F0 stubs on `main` (`todo!()` until W3 and W6 merge).
//!   [`f0_guard`] turns such a stub's panic into `core.notImplemented`, so a daemon built before those merges
//!   answers an error instead of losing the request to a panic.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::progress::EventSink;
use russet_debug::async_trait;
use russet_protocol::debug::DebuggerFlavor;
use russet_toolchain::{Component, Requirement, ToolInfo, Toolchains};
use tokio_util::sync::CancellationToken;

use super::ledger::InstallLedger;
use super::run::{
    AppSigner, BuildSpec, BuiltProduct, LldbResolver, Pipeline, ProjectBuilder, Provisioned, Provisioning, SignedApp,
};
use crate::DaemonCtx;

/// The pipeline over the daemon's services.
pub fn pipeline_for(cx: &DaemonCtx) -> Pipeline {
    Pipeline {
        builder: Arc::new(LiveBuilder),
        provisioning: Arc::new(LiveProvisioning),
        signer: Arc::new(LiveSigner { accounts: Arc::clone(&cx.accounts) }),
        lldb: Arc::new(LiveLldb { toolchains: Arc::clone(&cx.toolchains) }),
        debug: Arc::clone(&cx.debug),
        ledger: Arc::new(InstallLedger::new(&cx.dirs.state)),
    }
}

/// The build step over W5's build plane.
#[derive(Debug, Default)]
pub struct LiveBuilder;

#[async_trait]
impl ProjectBuilder for LiveBuilder {
    async fn build(
        &self,
        _spec: &BuildSpec,
        _sink: &dyn EventSink,
        _cancel: &CancellationToken,
    ) -> Result<BuiltProduct> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "the run pipeline's build step waits for W5's build service")
            .with_hint(
                "Build with `russet build` once W5 lands; the pipeline calls the same Builder after W5's F2 merge.",
            ))
    }
}

/// The provisioning step over W6's provisioner (GET-only plan, zero-mutation apply).
#[derive(Debug, Default)]
pub struct LiveProvisioning;

#[async_trait]
impl Provisioning for LiveProvisioning {
    async fn provision(&self, _product: &BuiltProduct, _udid: &str) -> Result<Provisioned> {
        Err(Error::new(
            codes::core::NOT_IMPLEMENTED,
            "the run pipeline's provisioning step waits for W6's ProvisionRequest",
        )
        .with_hint("`russet provision --dry-run` lands with W6; the pipeline plans the same way after W6's F2 merge."))
    }
}

/// The signing step: copy, then `russet_sign::sign` with the team's identity.
pub struct LiveSigner {
    pub accounts: Arc<russet_account::AccountService>,
}

#[async_trait]
impl AppSigner for LiveSigner {
    async fn sign(&self, product: &BuiltProduct, provisioned: &Provisioned, dest: &Path) -> Result<SignedApp> {
        let accounts = Arc::clone(&self.accounts);
        let team = provisioned.team_id.clone();
        let identities =
            f0_guard("the identity store (W6)", async move { accounts.identities().list(team.as_deref()) }).await?;
        let identity = identities.first().ok_or_else(|| {
            Error::new(codes::sign::NO_IDENTITY, "no signing identity for this team")
                .with_hint("Import one with `russet account import --from-xtool --yes`.")
        })?;
        let accounts = Arc::clone(&self.accounts);
        let id = identity.id.clone();
        let loaded = f0_guard("the identity store (W6)", async move { accounts.identities().load(&id) }).await?;
        let src = product.app.clone();
        let dest_buf = dest.to_path_buf();
        let per_bundle = provisioned
            .profiles
            .iter()
            .map(|(rel, profile)| {
                (rel.clone(), russet_sign::BundleSigningInput { profile: Some(profile.clone()), entitlements: None })
            })
            .collect();
        tokio::task::spawn_blocking(move || -> Result<()> {
            copy_app(&src, &dest_buf)?;
            russet_sign::sign(russet_sign::SignRequest {
                bundle: &dest_buf,
                identity: &loaded,
                per_bundle,
                options: russet_sign::SignOptions::default(),
            })
            .map(|_| ())
        })
        .await
        .map_err(|e| join_error("the signer (W6)", e))??;
        Ok(SignedApp { app: dest.to_path_buf() })
    }
}

/// LLDB from the toolchain resolver.
pub struct LiveLldb {
    pub toolchains: Arc<Toolchains>,
}

#[async_trait]
impl LldbResolver for LiveLldb {
    async fn lldb(&self, _flavor: DebuggerFlavor) -> Result<ToolInfo> {
        let t = Arc::clone(&self.toolchains);
        f0_guard(
            "the toolchain resolver (W3)",
            async move { t.resolve(Component::Lldb, &Requirement::default()).await },
        )
        .await
    }
    async fn lldb_dap(&self, _flavor: DebuggerFlavor) -> Result<ToolInfo> {
        let t = Arc::clone(&self.toolchains);
        f0_guard(
            "the toolchain resolver (W3)",
            async move { t.resolve(Component::LldbDap, &Requirement::default()).await },
        )
        .await
    }
}

/// `todo!()`'s panic message: what an F0 stub on `main` panics with until its owner fills it.
const F0_PANIC: &str = "not yet implemented";

/// Runs `fut`; if it panics in an F0 stub (`todo!()`), answers `core.notImplemented` naming `what` instead. Any
/// other panic is passed on.
pub async fn f0_guard<T>(what: &str, fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    use futures::FutureExt as _;
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(payload) => Err(f0_error(what, payload)),
    }
}

fn f0_error(what: &str, payload: Box<dyn std::any::Any + Send>) -> Error {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default()
        .to_owned();
    if !message.starts_with(F0_PANIC) {
        std::panic::resume_unwind(payload);
    }
    Error::new(codes::core::NOT_IMPLEMENTED, format!("{what} is not available in this build yet ({message})"))
        .with_hint("This daemon was built before the workstream that fills it merged; update Russet.")
}

/// A blocking task's failure: an F0 stub's panic becomes `core.notImplemented`, anything else `core.internal`.
fn join_error(what: &str, e: tokio::task::JoinError) -> Error {
    if e.is_panic() {
        return f0_error(what, e.into_panic());
    }
    Error::new(codes::core::INTERNAL, format!("{what} stopped: {e}"))
}

/// Replaces `dest` with a copy of the bundle `src` (files keep their modes; symlinks stay links).
pub fn copy_app(src: &Path, dest: &Path) -> Result<()> {
    let io =
        |what: &str, p: &Path, e: std::io::Error| Error::new(codes::core::IO, format!("{what} {}: {e}", p.display()));
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| io("removing", dest, e))?;
    }
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        std::fs::create_dir_all(&to).map_err(|e| io("creating", &to, e))?;
        for entry in std::fs::read_dir(&from).map_err(|e| io("reading", &from, e))? {
            let entry = entry.map_err(|e| io("reading", &from, e))?;
            let kind = entry.file_type().map_err(|e| io("reading", &entry.path(), e))?;
            let target = to.join(entry.file_name());
            if kind.is_dir() {
                stack.push((entry.path(), target));
            } else if kind.is_symlink() {
                let link = std::fs::read_link(entry.path()).map_err(|e| io("reading", &entry.path(), e))?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&link, &target).map_err(|e| io("linking", &target, e))?;
                #[cfg(not(unix))]
                std::fs::copy(from.join(&link), &target).map_err(|e| io("copying", &target, e)).map(|_| ())?;
            } else {
                std::fs::copy(entry.path(), &target).map_err(|e| io("copying", &target, e))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn copies_bundles_with_links_and_modes() {
        let base = std::env::temp_dir().join(format!("russet-copy-{}", std::process::id()));
        let src = base.join("Probe.app");
        std::fs::create_dir_all(src.join("Frameworks/X.framework")).unwrap();
        std::fs::write(src.join("Probe"), b"\xcf\xfa\xed\xfe").unwrap();
        std::fs::write(src.join("Frameworks/X.framework/X"), b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(src.join("Probe"), std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("X", src.join("Frameworks/X.framework/Current")).unwrap();
        }
        let dest = base.join("signed/Probe.app");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("stale"), b"old").unwrap();
        copy_app(&src, &dest).unwrap();
        assert!(!dest.join("stale").exists());
        assert_eq!(std::fs::read(dest.join("Frameworks/X.framework/X")).unwrap(), b"x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dest.join("Probe")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
            assert_eq!(std::fs::read_link(dest.join("Frameworks/X.framework/Current")).unwrap(), PathBuf::from("X"));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn f0_stubs_answer_not_implemented_and_other_panics_pass() {
        async fn stub() -> Result<u32> {
            todo!("W3 (CORE-10)")
        }
        let e = f0_guard("the toolchain resolver (W3)", stub()).await.unwrap_err();
        assert_eq!(e.code(), codes::core::NOT_IMPLEMENTED);
        assert!(e.message().contains("W3 (CORE-10)") && e.hint().is_some());
        assert_eq!(f0_guard("x", async { Ok(7) }).await.unwrap(), 7);
        let other = tokio::spawn(f0_guard("x", async {
            if true {
                panic!("a real bug");
            }
            Ok(())
        }))
        .await
        .unwrap_err();
        assert!(other.is_panic());
        let blocking = tokio::task::spawn_blocking(|| -> Result<()> { todo!("W6 (SA-05)") }).await.unwrap_err();
        assert_eq!(join_error("the signer (W6)", blocking).code(), codes::core::NOT_IMPLEMENTED);
    }
}
