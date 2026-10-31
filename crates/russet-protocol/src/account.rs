// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account.*`: accounts and the free Apple ID flow (ARCHITECTURE §8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22
//! (appleId and anisette rows, Wave 3).
//!
//! The Wave-2 and Wave-3 rows keep placeholder DTOs ([`Unspecified`]) until their owners define them.
//! `account.addAscKey` and `account.importXtool` are `secrets`-class local-file imports: a non-interactive CLI
//! may run them with `--yes` (ARCHITECTURE §7.5 [D 85]). Team IDs are masked on the wire (`*******345`) and
//! identities are named by their certificate serial.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::identity::SigningIdentity;
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// What an account is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AccountKind {
    AscTeamKey,
    AscIndividualKey,
    AppleId,
}

/// An account (`Account`), from `accounts.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// `asc:<KEYID>` or `appleid:<DSID>`.
    pub id: String,
    pub kind: AccountKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key_id: Option<String>,
    /// Masked (`*******XYZ`); omitted until discovered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<String>,
    /// `xtool` for the one-time import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub imported_from: Option<String>,
    pub is_default: bool,
}

/// Params of `account.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountListParams {}
/// Result of `account.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountListResult {
    pub accounts: Vec<Account>,
    /// The default account's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default: Option<String>,
}
crate::rpc_method!(
    /// `account.list` (class `read`). W6 (Wave 1).
    AccountList,
    "account.list",
    AccountListParams => AccountListResult,
    Permission::Read
);

/// Params of `account.addAscKey` (cli/ide only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAddAscKeyParams {
    pub key_id: String,
    /// Omitted for an individual key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub issuer_id: Option<String>,
    /// The `AuthKey_<KEYID>.p8` file (absolute); the key is sealed, never echoed.
    pub p8_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}
/// Result of `account.addAscKey`.
pub type AccountAddAscKeyResult = Account;
crate::rpc_method!(
    /// `account.addAscKey` (class `secrets`). W6 (Wave 1).
    AccountAddAscKey,
    "account.addAscKey",
    AccountAddAscKeyParams => AccountAddAscKeyResult,
    Permission::Secrets
);

/// Params of `account.importXtool` (cli/ide only; the explicit one-time import [D 50]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportXtoolParams {
    /// Report what would be imported without sealing or writing anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dry_run: Option<bool>,
}
/// Result of `account.importXtool`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportXtoolResult {
    pub accounts: Vec<Account>,
    pub identities: Vec<SigningIdentity>,
    /// Advice (always the `chmod 600` / `xtool auth logout` line, which names xtool's directory but none of its
    /// files) and anything skipped.
    pub warnings: Vec<String>,
    pub dry_run: bool,
}
crate::rpc_method!(
    /// `account.importXtool` (class `secrets`). W6 (Wave 1).
    AccountImportXtool,
    "account.importXtool",
    AccountImportXtoolParams => AccountImportXtoolResult,
    Permission::Secrets
);

/// One `account.verify` probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeOutcome {
    Ok,
    Forbidden,
    Unauthorized,
    Error,
}

/// The role probe of `account.verify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RoleProbe {
    pub certificates: ProbeOutcome,
    pub profiles: ProbeOutcome,
    pub builds: ProbeOutcome,
}

/// Params of `account.verify`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountVerifyParams {
    /// Omitted: the default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account_id: Option<String>,
}
/// Result of `account.verify`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountVerifyResult {
    pub ok: bool,
    pub account_id: String,
    /// Masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<String>,
    pub role_probe: RoleProbe,
}
crate::rpc_method!(
    /// `account.verify` (class `read`). W6 (Wave 1).
    AccountVerify,
    "account.verify",
    AccountVerifyParams => AccountVerifyResult,
    Permission::Read
);

/// Params of `account.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountRemoveParams {
    pub account_id: String,
    /// Also delete the account's sealed App Store Connect key from the secret store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wipe_secrets: Option<bool>,
}
/// Result of `account.remove`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountRemoveResult {}
crate::rpc_method!(
    /// `account.remove` (class `secrets`). W12 (Wave 2).
    AccountRemove,
    "account.remove",
    AccountRemoveParams => AccountRemoveResult,
    Permission::Secrets
);

/// Params of `account.setDefault`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountSetDefaultParams {
    pub account_id: String,
}
/// Result of `account.setDefault`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountSetDefaultResult {}
crate::rpc_method!(
    /// `account.setDefault` (class `secrets`). W12 (Wave 2).
    AccountSetDefault,
    "account.setDefault",
    AccountSetDefaultParams => AccountSetDefaultResult,
    Permission::Secrets
);

/// Params of `account.appleId.begin`: `{appleId, password, anisette: local|{remote}} → {sessionId, step: done|two-factor, twoFactor?} (cli/ide only)`.
pub type AccountAppleIdBeginParams = Unspecified;
/// Result of `account.appleId.begin`.
pub type AccountAppleIdBeginResult = Unspecified;
crate::rpc_method!(
    /// `account.appleId.begin` (class `secrets`). W22 (Wave 3).
    AccountAppleIdBegin,
    "account.appleId.begin",
    AccountAppleIdBeginParams => AccountAppleIdBeginResult,
    Permission::Secrets
);

/// Params of `account.appleId.requestCode`: `{sessionId, via, phoneId?} → {}`.
pub type AccountAppleIdRequestCodeParams = Unspecified;
/// Result of `account.appleId.requestCode`.
pub type AccountAppleIdRequestCodeResult = Unspecified;
crate::rpc_method!(
    /// `account.appleId.requestCode` (class `secrets`). W22 (Wave 3).
    AccountAppleIdRequestCode,
    "account.appleId.requestCode",
    AccountAppleIdRequestCodeParams => AccountAppleIdRequestCodeResult,
    Permission::Secrets
);

/// Params of `account.appleId.submitCode`: `{sessionId, code} → Account`.
pub type AccountAppleIdSubmitCodeParams = Unspecified;
/// Result of `account.appleId.submitCode`.
pub type AccountAppleIdSubmitCodeResult = Unspecified;
crate::rpc_method!(
    /// `account.appleId.submitCode` (class `secrets`). W22 (Wave 3).
    AccountAppleIdSubmitCode,
    "account.appleId.submitCode",
    AccountAppleIdSubmitCodeParams => AccountAppleIdSubmitCodeResult,
    Permission::Secrets
);

/// Params of `account.anisette.status`: `{} → {kind, librariesPresent, apkVersion?, provisioned}`.
pub type AccountAnisetteStatusParams = Unspecified;
/// Result of `account.anisette.status`.
pub type AccountAnisetteStatusResult = Unspecified;
crate::rpc_method!(
    /// `account.anisette.status` (class `read`). W22 (Wave 3).
    AccountAnisetteStatus,
    "account.anisette.status",
    AccountAnisetteStatusParams => AccountAnisetteStatusResult,
    Permission::Read
);

/// Params of `account.anisette.fetchLibraries`: `{consent: true} → {apkVersion, bytesDownloaded}`.
pub type AccountAnisetteFetchLibrariesParams = Unspecified;
/// Result of `account.anisette.fetchLibraries`.
pub type AccountAnisetteFetchLibrariesResult = Unspecified;
crate::rpc_method!(
    /// `account.anisette.fetchLibraries` (class `system`, long-running). W22 (Wave 3).
    AccountAnisetteFetchLibraries,
    "account.anisette.fetchLibraries",
    AccountAnisetteFetchLibrariesParams => AccountAnisetteFetchLibrariesResult,
    Permission::System, long_running
);

/// Params of the `account.event` notification: `{kind: token-expiring|profile-expiring|free-quota, accountId, detail}; to ide and cli`.
pub type AccountEventParams = Unspecified;
crate::rpc_notification!(
    /// `account.event` (daemon → client). W12.
    AccountEvent,
    "account.event",
    AccountEventParams
);

/// The `secrets`-class methods of this namespace that are local-file imports: from a non-interactive `cli`
/// connection the permission engine asks a plain `ui.confirm` (which `--yes` pre-confirms) instead of refusing
/// for want of a TTY (ARCHITECTURE §7.5 [D 85]). `secrets.set {fromFile}` is the third such import (W3).
pub const LOCAL_FILE_IMPORTS: &[&str] = &[AccountImportXtool::INFO.name, AccountAddAscKey::INFO.name];

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    AccountList::INFO,
    AccountAddAscKey::INFO,
    AccountImportXtool::INFO,
    AccountVerify::INFO,
    AccountRemove::INFO,
    AccountSetDefault::INFO,
    AccountAppleIdBegin::INFO,
    AccountAppleIdRequestCode::INFO,
    AccountAppleIdSubmitCode::INFO,
    AccountAnisetteStatus::INFO,
    AccountAnisetteFetchLibraries::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[AccountEvent::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Confirm, Permission};

    #[test]
    fn the_local_file_imports_are_plain_secrets_methods() {
        assert_eq!(LOCAL_FILE_IMPORTS, ["account.importXtool", "account.addAscKey"]);
        for name in LOCAL_FILE_IMPORTS {
            let m = METHODS.iter().find(|m| m.name == *name).copied();
            assert!(m.is_some_and(|m| m.permission == Permission::Secrets && m.confirm == Confirm::Plain), "{name}");
        }
    }

    #[test]
    fn import_results_are_camel_case() {
        let v = serde_json::to_value(AccountImportXtoolResult {
            accounts: Vec::new(),
            identities: Vec::new(),
            warnings: vec!["chmod 600".into()],
            dry_run: true,
        });
        assert!(v.is_ok_and(|v| v["dryRun"] == true && v["warnings"][0] == "chmod 600"));
    }
}
