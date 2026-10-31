// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The machine identity anisette is computed from (SIGN §7.5): the local user UUID, generated once, and the ADI
//! provisioning state (`adi.pb`). Both identify this machine to Apple ("don't leak your ADI data"), so they live
//! only in the secret store (`AnisetteState`, envelope-encrypted) and in the provisioning directory the helper
//! reads (0700); nothing logs them.

use base64::Engine as _;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_secrets::{SecretBytes, SecretId, SecretKind, SecretStore};
use serde::{Deserialize, Serialize};

use crate::DEFAULT_RINFO;

/// The persisted anisette identity of one provider (`secret:anisette-state/<name>`). `Debug` prints nothing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnisetteState {
    #[serde(default = "one")]
    pub v: u32,
    /// The local user UUID (lowercase, hyphenated).
    pub uuid: String,
    /// `adi.pb` (base64): what the ADI library wrote for local providers, what a v3 server returned for remote
    /// ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adi_pb: Option<String>,
    /// `X-Apple-I-MD-RINFO` from provisioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rinfo: Option<String>,
}

fn one() -> u32 {
    1
}

impl std::fmt::Debug for AnisetteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnisetteState").field("provisioned", &self.adi_pb.is_some()).finish_non_exhaustive()
    }
}

impl AnisetteState {
    /// A new identity with a random local user UUID.
    pub fn generate() -> Self {
        Self { v: 1, uuid: uuid::Uuid::new_v4().hyphenated().to_string(), adi_pb: None, rinfo: None }
    }

    /// `X-Apple-I-MD-LU`: `hex_upper(SHA256(uuid))`.
    pub fn md_lu(&self) -> String {
        let d = ring::digest::digest(&ring::digest::SHA256, self.uuid.as_bytes());
        d.as_ref().iter().map(|b| format!("{b:02X}")).collect()
    }

    /// `X-Mme-Device-Id`: the UUID, uppercase.
    pub fn device_id(&self) -> String {
        self.uuid.to_uppercase()
    }

    /// The Android ID handed to ADI: the first 16 hex digits of the UUID without dashes, lowercase.
    pub fn android_id(&self) -> String {
        self.uuid.chars().filter(|c| *c != '-').take(16).collect::<String>().to_lowercase()
    }

    /// The 16 bytes of the UUID (the anisette-v3 identifier).
    pub fn identifier(&self) -> Vec<u8> {
        uuid::Uuid::parse_str(&self.uuid).map(|u| u.as_bytes().to_vec()).unwrap_or_default()
    }

    pub fn rinfo(&self) -> String {
        self.rinfo.clone().unwrap_or_else(|| DEFAULT_RINFO.to_owned())
    }

    /// The decoded `adi.pb`.
    pub fn adi_pb_bytes(&self) -> Option<Vec<u8>> {
        self.adi_pb.as_deref().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
    }

    pub fn set_adi_pb(&mut self, bytes: &[u8]) {
        self.adi_pb = Some(base64::engine::general_purpose::STANDARD.encode(bytes));
    }

    /// The secret holding the state of provider `name` (`local`, or `remote-<hash of the URL>`).
    pub fn secret_id(name: &str) -> SecretId {
        SecretId { kind: SecretKind::AnisetteState, name: name.to_owned() }
    }

    /// Loads the state of `name`, or generates (and stores) a new identity.
    pub async fn load_or_create(store: &dyn SecretStore, name: &str) -> Result<Self> {
        let id = Self::secret_id(name);
        if let Some(bytes) = store.get(&id).await? {
            return serde_json::from_slice(bytes.expose()).map_err(|_| {
                Error::new(codes::anisette::ADI_FAILED, "the stored anisette identity is unreadable").with_hint(
                    "Reset it with `russet account anisette reset`; the machine provisions again on the next sign-in.",
                )
            });
        }
        let state = Self::generate();
        state.save(store, name).await?;
        Ok(state)
    }

    /// The state of `name`, if any (no side effects).
    pub async fn load(store: &dyn SecretStore, name: &str) -> Result<Option<Self>> {
        match store.get(&Self::secret_id(name)).await? {
            Some(bytes) => Ok(serde_json::from_slice(bytes.expose()).ok()),
            None => Ok(None),
        }
    }

    pub async fn save(&self, store: &dyn SecretStore, name: &str) -> Result<()> {
        let bytes = serde_json::to_vec(self)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot encode the anisette identity: {e}")))?;
        store.put(&Self::secret_id(name), "anisette identity", SecretBytes::new(bytes)).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_derivations_follow_the_known_clients() {
        let s = AnisetteState { v: 1, uuid: "0a1b2c3d-4e5f-4061-8273-94a5b6c7d8e9".into(), adi_pb: None, rinfo: None };
        assert_eq!(s.android_id(), "0a1b2c3d4e5f4061");
        assert_eq!(s.device_id(), "0A1B2C3D-4E5F-4061-8273-94A5B6C7D8E9");
        assert_eq!(s.md_lu().len(), 64);
        assert!(s.md_lu().chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
        assert_eq!(s.identifier().len(), 16);
        assert_eq!(s.rinfo(), DEFAULT_RINFO);
        assert!(!format!("{s:?}").contains("0a1b2c3d"));
    }

    #[tokio::test]
    async fn the_identity_is_created_once() {
        let store = russet_secrets::memory::MemoryStore::new();
        let a = AnisetteState::load_or_create(&store, "local").await.ok();
        let b = AnisetteState::load_or_create(&store, "local").await.ok();
        assert!(a.is_some() && a == b);
    }
}
