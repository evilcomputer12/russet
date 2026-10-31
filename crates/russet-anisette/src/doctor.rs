// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `anisette.local`: whether Apple ID sign-in with local anisette can run here (the libraries are downloaded and
//! verify, the helper is installed). It reads files only: no helper starts and nothing goes on the network.

use async_trait::async_trait;
use russet_doctor::{Category, Check, CheckCx, CheckResult, Requires};

use crate::libs::{self, Abi};

/// `anisette.local`.
#[derive(Debug, Clone, Default)]
pub struct AnisetteLocal;

/// Whether `accounts.toml` holds an Apple ID account (it is machine-written: `kind = "apple-id"`).
fn has_apple_id(config: &std::path::Path) -> bool {
    std::fs::read_to_string(config.join("accounts.toml")).is_ok_and(|t| t.contains("kind = \"apple-id\""))
}

#[async_trait]
impl Check for AnisetteLocal {
    fn id(&self) -> &'static str {
        "anisette.local"
    }
    fn title(&self) -> &'static str {
        "Local anisette can sign in to Apple"
    }
    fn category(&self) -> Category {
        Category::Account
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let Some(dirs) = cx.dirs.clone() else { return CheckResult::skip("Russet's directories are unknown") };
        let wanted = has_apple_id(&dirs.config);
        if !crate::local_supported() {
            return CheckResult::skip("local anisette runs in Russet's Linux engine (x86_64)");
        }
        let Some(set) = Abi::host().and_then(|abi| libs::installed(&dirs.data, abi)) else {
            if !wanted {
                return CheckResult::skip("no Apple ID account; Apple's ADI libraries are not needed");
            }
            return CheckResult::warn("Apple's ADI libraries are not downloaded")
                .with_hint("Download them with consent (about 2 MB): the Apple ID sign-in page offers it.");
        };
        if crate::local::default_host_exe().is_none() {
            return CheckResult::fail("the anisette helper (russet-adi-host) is not installed next to russetd")
                .with_hint("Reinstall Russet, or set RUSSET_ADI_HOST to the helper.");
        }
        if !set.pinned {
            return CheckResult::warn(format!(
                "Apple's ADI libraries (APK {}) were trusted on first use; their hashes are not in Russet's pin table yet",
                set.apk_version
            ))
            .with_hint("A Russet release pins them; until then they are checked against the hashes recorded at download.");
        }
        CheckResult::pass(format!("Apple's ADI libraries (APK {}) are present and pinned", set.apk_version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn skips_without_an_apple_id_and_warns_with_one() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = russet_core::dirs::RussetDirs::under(tmp.path());
        let cx = CheckCx { dirs: Some(dirs.clone()), ..CheckCx::default() };
        let r = AnisetteLocal.run(&cx).await;
        assert_eq!(r.status, russet_doctor::Status::Skip);
        if crate::local_supported() {
            std::fs::create_dir_all(&dirs.config).expect("config dir");
            std::fs::write(dirs.config.join("accounts.toml"), "[[account]]\nkind = \"apple-id\"\n").expect("write");
            let r = AnisetteLocal.run(&cx).await;
            assert_eq!(r.status, russet_doctor::Status::Warn);
            assert!(r.hint.is_some());
        }
    }
}
