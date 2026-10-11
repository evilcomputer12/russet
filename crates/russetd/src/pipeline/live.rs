// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The pipeline's ports over the daemon's services ([`pipeline_for`]).
//!
//! - LLDB comes from the toolchain resolver (`Toolchains::resolve`, W3).
//! - Signing copies the build product and signs it with `russet_sign::sign` and the account store's identity
//!   (W6); the ledger hash is the SHA-256 of the signed main executable, which changes whenever the code or a
//!   sealed resource changes (the CodeDirectory hash is in it).
//! - The device steps (install, launch, debugserver) go through `DaemonCtx.debug`'s `DeviceServices`, which
//!   russetd's `DeviceSlot` provides (W2, W4).
//! - Building and provisioning need contracts that are not on `main` yet: W5's build service (a `Builder` needs
//!   an `SdkProvider`, which F0 cannot construct) and W6's `ProvisionRequest` fields (freeze F2). Until they
//!   land, those two steps answer `core.notImplemented` with the card to wait for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_core::progress::EventSink;
use russet_debug::async_trait;
use russet_protocol::debug::DebuggerFlavor;
use russet_toolchain::{Component, Requirement, ToolInfo, Toolchains};
use sha2::{Digest, Sha256};
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
        let identities = self.accounts.identities().list(provisioned.team_id.as_deref())?;
        let identity = identities.first().ok_or_else(|| {
            Error::new(codes::sign::NO_IDENTITY, "no signing identity for this team")
                .with_hint("Import one with `russet account import --from-xtool --yes`.")
        })?;
        let loaded = self.accounts.identities().load(&identity.id)?;
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
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("the signing task failed: {e}")))??;
        let exe = dest.join(product.executable.file_name().unwrap_or_default());
        Ok(SignedApp { app: dest.to_path_buf(), cdhash: file_sha256(&exe)? })
    }
}

/// LLDB from the toolchain resolver.
pub struct LiveLldb {
    pub toolchains: Arc<Toolchains>,
}

#[async_trait]
impl LldbResolver for LiveLldb {
    async fn lldb(&self, _flavor: DebuggerFlavor) -> Result<ToolInfo> {
        self.toolchains.resolve(Component::Lldb, &Requirement::default()).await
    }
    async fn lldb_dap(&self, _flavor: DebuggerFlavor) -> Result<ToolInfo> {
        self.toolchains.resolve(Component::LldbDap, &Requirement::default()).await
    }
}

/// SHA-256 of a file, lower-case hex.
pub fn file_sha256(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).map_err(|e| Error::new(codes::core::IO, format!("reading {}: {e}", path.display())))?;
    Ok(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
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
        let h = file_sha256(&dest.join("Probe")).unwrap();
        assert_eq!(h.len(), 64);
        let _ = std::fs::remove_dir_all(&base);
    }
}
