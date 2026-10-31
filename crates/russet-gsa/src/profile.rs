// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The client identity profile (`gsa-profile.json`, SIGN §7.2, §21 Q9): how Russet presents itself to GrandSlam
//! and developerservices2. GSA has rejected outdated identities before (2026-09-10), so the profile is versioned,
//! a user copy in the config directory overrides the built-in one, and Russet releases refresh it.

use std::path::{Path, PathBuf};

use russet_anisette::ClientIdentity;
use russet_core::Result;
use russet_core::error::{Error, codes};
use serde::{Deserialize, Serialize};

/// The profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientProfile {
    pub version: u32,
    /// `X-Mme-Client-Info`; `{model}` is replaced by [`Self::model`].
    pub client_info: String,
    pub user_agent: String,
    /// `X-Xcode-Version`.
    pub xcode_version: String,
    /// `X-Apple-App-Info` and the app token Russet asks for.
    pub app_info: String,
    /// A client info to present instead of the anisette provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anisette_client_info_override: Option<String>,
    /// The Mac model in `{model}`.
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn default_model() -> String {
    "MacBookPro18,3".into()
}

impl ClientProfile {
    /// The identity Russet ships for its local (Android) ADI: version 4. Since 2026-10-12 GrandSlam refuses a
    /// macOS client info with Android ADI data (`-80045` at init) and, since about 2026-10-19, Android-identifying
    /// clients at two-factor (`-80046`); Android-ADI clients present this Windows identity with the akd user agent
    /// (anisette-v3-server #61; xtool #337, #406).
    pub fn builtin() -> Self {
        Self {
            version: 4,
            client_info: "<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into(),
            user_agent: "akd/1.0 CFNetwork/808.1.4".into(),
            xcode_version: "26.5 (17F42)".into(),
            app_info: "com.apple.gs.xcode.auth".into(),
            anisette_client_info_override: None,
            model: default_model(),
            source: Some(
                "2026-10-31: Windows identity for Android ADI (GrandSlam -80045 with macOS since 2026-10-12, anisette-v3-server #61, xtool #337/#406); X-Xcode-Version as xtool"
                    .into(),
            ),
        }
    }

    /// `<config>/gsa-profile.json`.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("gsa-profile.json")
    }

    /// The user's profile when it exists and is at least as new as the built-in one, else the built-in one.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let path = Self::path(config_dir);
        let builtin = Self::builtin();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let p: Self = serde_json::from_str(&text).map_err(|e| {
                    Error::new(codes::config::INVALID, format!("{} is invalid: {e}", path.display()))
                        .with_hint("Fix or delete gsa-profile.json; Russet then uses its built-in client identity.")
                })?;
                Ok(if p.version >= builtin.version { p } else { builtin })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(builtin),
            Err(e) => Err(Error::io(format!("reading {}", path.display()), e)),
        }
    }

    /// `X-Mme-Client-Info` with the model filled in.
    pub fn client_info(&self) -> String {
        self.client_info.replace("{model}", &self.model)
    }

    /// The identity anisette provisioning presents.
    pub fn identity(&self) -> ClientIdentity {
        ClientIdentity {
            client_info: self.anisette_client_info_override.clone().unwrap_or_else(|| self.client_info()),
            user_agent: self.user_agent.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_user_profile_loses_to_the_builtin_one() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(ClientProfile::load(tmp.path()).expect("builtin"), ClientProfile::builtin());
        let mut old = ClientProfile::builtin();
        old.version = 2;
        old.user_agent = "old".into();
        std::fs::write(ClientProfile::path(tmp.path()), serde_json::to_vec(&old).expect("json")).expect("write");
        assert_eq!(ClientProfile::load(tmp.path()).expect("loads").user_agent, ClientProfile::builtin().user_agent);
        let mut new = ClientProfile::builtin();
        new.version = 5;
        new.user_agent = "new".into();
        std::fs::write(ClientProfile::path(tmp.path()), serde_json::to_vec(&new).expect("json")).expect("write");
        assert_eq!(ClientProfile::load(tmp.path()).expect("loads").user_agent, "new");
        let id = ClientProfile::builtin().identity();
        assert_eq!(russet_anisette::platform_of(&id.client_info), "windows");
        assert!(id.user_agent.starts_with("akd/1.0 CFNetwork/"));
    }
}
