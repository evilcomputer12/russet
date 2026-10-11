// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Account doctor checks: an account is configured, a development identity is usable, and xtool's credential
//! files (if any) are private. They read files only: no secret is unsealed and nothing goes on the network.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use russet_core::dirs::RussetDirs;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires, Status};

use crate::identity::{CertKind, IdentityStore};

/// The directories of ARCHITECTURE §15 (`RUSSET_HOME`, the per-directory overrides, XDG), resolved without the
/// daemon. W2's `RussetDirs::from_env` is the shared implementation once it lands.
pub(crate) fn resolve_dirs() -> Option<RussetDirs> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut dirs = if let Some(home) = env("RUSSET_HOME") {
        RussetDirs::under(&home)
    } else {
        let home = env("HOME")?;
        let xdg = |k: &str, d: &str| env(k).unwrap_or_else(|| home.join(d)).join("russet");
        RussetDirs {
            config: xdg("XDG_CONFIG_HOME", ".config"),
            data: xdg("XDG_DATA_HOME", ".local/share"),
            state: xdg("XDG_STATE_HOME", ".local/state"),
            cache: xdg("XDG_CACHE_HOME", ".cache"),
            runtime: env("XDG_RUNTIME_DIR").map_or_else(|| home.join(".cache/russet/run"), |r| r.join("russet")),
        }
    };
    if let Some(p) = env("RUSSET_CONFIG_DIR") {
        dirs.config = p;
    }
    if let Some(p) = env("RUSSET_DATA_DIR") {
        dirs.data = p;
    }
    Some(dirs)
}

fn result(status: Status, summary: impl Into<String>, hint: Option<&str>) -> CheckResult {
    CheckResult { status, summary: summary.into(), detail: None, hint: hint.map(str::to_owned) }
}

/// `account.configured`.
#[derive(Debug, Clone, Default)]
pub struct AccountConfigured {
    pub dirs: Option<RussetDirs>,
}

#[async_trait]
impl Check for AccountConfigured {
    fn id(&self) -> &'static str {
        "account.configured"
    }
    fn title(&self) -> &'static str {
        "An App Store Connect account is configured"
    }
    fn category(&self) -> Category {
        Category::Account
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let Some(dirs) = self.dirs.clone().or_else(resolve_dirs) else {
            return result(Status::Skip, "no home directory", None);
        };
        let path = dirs.config.join("accounts.toml");
        let n = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|t| t.get("account").and_then(toml::Value::as_array).map(Vec::len))
            .unwrap_or(0);
        if n > 0 {
            result(Status::Pass, format!("{n} account(s) in accounts.toml"), None)
        } else {
            result(
                Status::Warn,
                "no account is configured",
                Some(
                    "Run `russet account import --from-xtool --yes`, or `russet account add-key --key-id … --issuer-id … --p8 …`.",
                ),
            )
        }
    }
}

/// `account.identity`.
#[derive(Debug, Clone, Default)]
pub struct IdentityUsable {
    pub dirs: Option<RussetDirs>,
}

#[async_trait]
impl Check for IdentityUsable {
    fn id(&self) -> &'static str {
        "account.identity"
    }
    fn title(&self) -> &'static str {
        "A development signing identity is usable"
    }
    fn category(&self) -> Category {
        Category::Account
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let Some(dirs) = self.dirs.clone().or_else(resolve_dirs) else {
            return result(Status::Skip, "no home directory", None);
        };
        // Public parts only: the store is never asked for a key here.
        let store = IdentityStore::new(&dirs.data, std::sync::Arc::new(NoSecrets));
        let ids = store.list(None).unwrap_or_default();
        let dev: Vec<_> = ids.iter().filter(|i| i.kind == CertKind::Development).collect();
        let Some(best) = dev.first() else {
            return result(
                Status::Warn,
                "no development identity",
                Some("Run `russet account import --from-xtool --yes` to reuse xtool's development certificate."),
            );
        };
        if best.is_expired() {
            return result(
                Status::Fail,
                "the development certificate has expired",
                Some("Create or import a new one (W12: `russet account certs create`)."),
            );
        }
        let soon = SystemTime::now() + Duration::from_secs(30 * 24 * 3600);
        if best.not_after <= soon {
            return result(
                Status::Warn,
                "the development certificate expires within 30 days",
                Some("Plan a new certificate before it expires."),
            );
        }
        result(Status::Pass, format!("{} development identity(ies); the newest is valid", dev.len()), None)
    }
}

/// `account.xtool-files`.
#[derive(Debug, Clone, Default)]
pub struct XtoolFilesPrivate {
    pub dir: Option<PathBuf>,
}

#[async_trait]
impl Check for XtoolFilesPrivate {
    fn id(&self) -> &'static str {
        "account.xtool-files"
    }
    fn title(&self) -> &'static str {
        "xtool's stored credentials are private"
    }
    fn category(&self) -> Category {
        Category::Account
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let Some(dir) = self.dir.clone().or_else(crate::xtool::default_dir) else {
            return result(Status::Skip, "no home directory", None);
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return result(Status::Skip, "xtool stores no credentials here", None);
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let open: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o077 != 0))
                .map(|e| e.path().display().to_string())
                .collect();
            if !open.is_empty() {
                return CheckResult {
                    status: Status::Warn,
                    summary: format!("{} xtool credential file(s) are readable by other users", open.len()),
                    detail: Some(open.join("\n")),
                    hint: Some(format!("chmod 600 {}", open.join(" "))),
                };
            }
        }
        #[cfg(not(unix))]
        let _ = entries;
        result(Status::Pass, "xtool's credential files are private", None)
    }
}

/// A secret store that holds nothing (the doctor reads public parts only).
struct NoSecrets;

#[async_trait]
impl russet_secrets::SecretStore for NoSecrets {
    async fn put(
        &self,
        _id: &russet_secrets::SecretId,
        _label: &str,
        _v: russet_secrets::SecretBytes,
    ) -> russet_core::Result<russet_secrets::SecretMeta> {
        Err(russet_core::Error::new(russet_core::error::codes::secrets::UNAVAILABLE, "read-only"))
    }
    async fn get(&self, _id: &russet_secrets::SecretId) -> russet_core::Result<Option<russet_secrets::SecretBytes>> {
        Ok(None)
    }
    async fn delete(&self, _id: &russet_secrets::SecretId) -> russet_core::Result<bool> {
        Ok(false)
    }
    async fn list(
        &self,
        _kind: Option<russet_secrets::SecretKind>,
    ) -> russet_core::Result<Vec<russet_secrets::SecretMeta>> {
        Ok(Vec::new())
    }
    fn backend(&self) -> russet_secrets::BackendInfo {
        russet_secrets::BackendInfo {
            kind: russet_secrets::BackendKind::Memory,
            available: false,
            locked: false,
            reason: None,
        }
    }
}
