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
    /// The identity Russet ships (version 3, from xtool 450223eb0 of 2026-09-10).
    pub fn builtin() -> Self {
        Self {
            version: 3,
            client_info: "<{model}> <macOS;27.0;26A5378j> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into(),
            user_agent: "AuthKit/1 (Macintosh; OS X 27.0) (com.apple.dt.Xcode/26.5)".into(),
            xcode_version: "26.5 (17F42)".into(),
            app_info: "com.apple.gs.xcode.auth".into(),
            anisette_client_info_override: None,
            model: default_model(),
            source: Some("xtool 450223eb0 (2026-09-10); isideload uses X-Xcode-Version 27.0 (27A5218g)".into()),
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
        new.version = 4;
        new.user_agent = "new".into();
        std::fs::write(ClientProfile::path(tmp.path()), serde_json::to_vec(&new).expect("json")).expect("write");
        assert_eq!(ClientProfile::load(tmp.path()).expect("loads").user_agent, "new");
        assert!(ClientProfile::builtin().client_info().starts_with("<MacBookPro18,3> <macOS;27.0;"));
    }
}
