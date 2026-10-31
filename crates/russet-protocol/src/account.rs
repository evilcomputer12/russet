// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account.*`: accounts and the free Apple ID flow (ARCHITECTURE §8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22
//! (appleId and anisette rows, Wave 3).
//!
//! `account.event` keeps a placeholder DTO ([`Unspecified`]) until W12 defines it. The Apple ID and anisette rows
//! (W22) carry passwords and two-factor codes: their `Debug` never prints them.
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

/// Where the anisette data of an Apple ID sign-in comes from (`local|{remote}`): `"local"` (Apple's ADI libraries
/// in the sandboxed `russet-adi-host`, the default) or `{"remote": "<url>"}` (an anisette-v3 server the user runs;
/// its operator learns this machine's ADI identity) (SIGN §7.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum AnisetteChoice {
    #[default]
    Local,
    /// The base URL of an anisette-v3 server.
    Remote(String),
}

/// Params of `account.appleId.begin` (cli/ide only): signs in to Apple with the engine's own GrandSlam client.
/// `Debug` never prints the Apple ID or the password; neither is logged, audited or echoed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAppleIdBeginParams {
    pub apple_id: String,
    pub password: String,
    #[serde(default)]
    pub anisette: AnisetteChoice,
    /// The account's label in `account.list` (default: `Apple ID (<masked id>)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub label: Option<String>,
}

impl std::fmt::Debug for AccountAppleIdBeginParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountAppleIdBeginParams")
            .field("apple_id", &"***")
            .field("password", &"***")
            .field("anisette", &self.anisette)
            .field("label", &self.label)
            .finish()
    }
}

/// The step a sign-in session is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AppleIdStep {
    /// Signed in: the account is in `account.list` (and in `account` of this result).
    Done,
    /// Apple asks for a two-factor code: call `account.appleId.submitCode` (after `requestCode` for an SMS).
    TwoFactor,
}

/// Where a two-factor code is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TwoFactorVia {
    /// A push to the user's trusted Apple devices.
    TrustedDevice,
    /// A text message to one of the trusted phone numbers.
    Sms,
}

/// A trusted phone number, as Apple obfuscates it (`(•••) •••-••12`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPhone {
    pub id: u32,
    /// Apple's obfuscated form; never the full number.
    pub number: String,
}

/// What a two-factor step offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct TwoFactorInfo {
    /// The ways a code can be requested (`requestCode`).
    pub via: Vec<TwoFactorVia>,
    /// Where the current code went: Russet asks Apple to push one to the trusted devices as soon as Apple offers
    /// that; omitted when nothing was sent yet (call `requestCode`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code_sent_to: Option<TwoFactorVia>,
    /// The trusted phone numbers for `requestCode {via: "sms", phoneId}` (empty until Apple lists them).
    pub phone_numbers: Vec<TrustedPhone>,
    /// The code length (6).
    pub code_length: u32,
}

/// Result of `account.appleId.begin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAppleIdBeginResult {
    /// The sign-in session (valid for 10 minutes).
    pub session_id: String,
    pub step: AppleIdStep,
    /// With `step: "two-factor"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub two_factor: Option<TwoFactorInfo>,
    /// With `step: "done"`: the account that was added (or refreshed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account: Option<Account>,
}
crate::rpc_method!(
    /// `account.appleId.begin` (class `secrets`). W22 (Wave 3).
    AccountAppleIdBegin,
    "account.appleId.begin",
    AccountAppleIdBeginParams => AccountAppleIdBeginResult,
    Permission::Secrets
);

/// Params of `account.appleId.requestCode`: `{sessionId, via, phoneId?} → {}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAppleIdRequestCodeParams {
    pub session_id: String,
    pub via: TwoFactorVia,
    /// Required with `via: "sms"`: a `phoneNumbers[].id` of the two-factor step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phone_id: Option<u32>,
}
/// Result of `account.appleId.requestCode`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAppleIdRequestCodeResult {}
crate::rpc_method!(
    /// `account.appleId.requestCode` (class `secrets`). W22 (Wave 3).
    AccountAppleIdRequestCode,
    "account.appleId.requestCode",
    AccountAppleIdRequestCodeParams => AccountAppleIdRequestCodeResult,
    Permission::Secrets
);

/// Params of `account.appleId.submitCode`: `{sessionId, code} → Account`. The code is checked against the way it
/// was last requested (the trusted devices unless `requestCode` sent an SMS). `Debug` never prints the code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAppleIdSubmitCodeParams {
    pub session_id: String,
    pub code: String,
}

impl std::fmt::Debug for AccountAppleIdSubmitCodeParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountAppleIdSubmitCodeParams")
            .field("session_id", &self.session_id)
            .field("code", &"***")
            .finish()
    }
}
/// Result of `account.appleId.submitCode`: the account that was added (or refreshed).
pub type AccountAppleIdSubmitCodeResult = Account;
crate::rpc_method!(
    /// `account.appleId.submitCode` (class `secrets`). W22 (Wave 3).
    AccountAppleIdSubmitCode,
    "account.appleId.submitCode",
    AccountAppleIdSubmitCodeParams => AccountAppleIdSubmitCodeResult,
    Permission::Secrets
);

/// Which anisette provider `account.anisette.status` describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AnisetteKind {
    Local,
    Remote,
}

/// Params of `account.anisette.status`: `{} → {kind, librariesPresent, apkVersion?, provisioned}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnisetteStatusParams {}
/// Result of `account.anisette.status`: the local ADI provider (remote servers are configured per account).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnisetteStatusResult {
    pub kind: AnisetteKind,
    /// Apple's two ADI libraries are downloaded and match their pins (or their trust-on-first-use record).
    pub libraries_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub apk_version: Option<String>,
    /// This machine holds ADI provisioning state.
    pub provisioned: bool,
    /// Local ADI runs on this host (Linux x86_64; Windows through the engine's WSL backend).
    pub local_supported: bool,
    /// The libraries' hashes are in Russet's shipped pin table (false: recorded on first use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pinned: Option<bool>,
}
crate::rpc_method!(
    /// `account.anisette.status` (class `read`). W22 (Wave 3).
    AccountAnisetteStatus,
    "account.anisette.status",
    AccountAnisetteStatusParams => AccountAnisetteStatusResult,
    Permission::Read
);

/// Params of `account.anisette.fetchLibraries`: `{consent: true} → {apkVersion, bytesDownloaded}`. `consent`
/// records that the user read the consent screen (SIGN §7.1); anything but `true` is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnisetteFetchLibrariesParams {
    pub consent: bool,
}
/// Result of `account.anisette.fetchLibraries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountAnisetteFetchLibrariesResult {
    pub apk_version: String,
    /// HTTP traffic of the range fetch (about 2–3 MB; 0 when the libraries were already present).
    pub bytes_downloaded: u64,
    /// The hashes are in Russet's pin table; false means they were recorded on first use.
    pub pinned: bool,
}
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

    #[test]
    fn anisette_choice_is_local_or_a_remote_object() {
        let local: Result<AccountAppleIdBeginParams, _> =
            serde_json::from_value(serde_json::json!({"appleId": "a", "password": "b"}));
        assert!(local.is_ok_and(|p| p.anisette == AnisetteChoice::Local));
        let remote: Result<AccountAppleIdBeginParams, _> = serde_json::from_value(
            serde_json::json!({"appleId": "a", "password": "b", "anisette": {"remote": "https://ani.example"}}),
        );
        assert!(remote.is_ok_and(|p| p.anisette == AnisetteChoice::Remote("https://ani.example".into())));
        assert!(serde_json::to_value(AnisetteChoice::Local).is_ok_and(|v| v == "local"));
    }

    #[test]
    fn debug_never_prints_credentials() {
        let begin = AccountAppleIdBeginParams {
            apple_id: "canary-user@example.invalid".into(),
            password: "canary-password-7731".into(),
            anisette: AnisetteChoice::Local,
            label: None,
        };
        let code = AccountAppleIdSubmitCodeParams { session_id: "s1".into(), code: "731904".into() };
        let text = format!("{begin:?} {code:?}");
        for canary in ["canary-user", "canary-password-7731", "731904"] {
            assert!(!text.contains(canary), "{canary} leaked: {text}");
        }
    }

    #[test]
    fn two_factor_steps_are_kebab_case() {
        let r = AccountAppleIdBeginResult {
            session_id: "s".into(),
            step: AppleIdStep::TwoFactor,
            two_factor: Some(TwoFactorInfo {
                via: vec![TwoFactorVia::TrustedDevice, TwoFactorVia::Sms],
                code_sent_to: Some(TwoFactorVia::TrustedDevice),
                phone_numbers: vec![TrustedPhone { id: 1, number: "(•••) •••-••12".into() }],
                code_length: 6,
            }),
            account: None,
        };
        let v = serde_json::to_value(r).unwrap_or_default();
        assert_eq!(v["step"], "two-factor");
        assert_eq!(v["twoFactor"]["via"][0], "trusted-device");
        assert_eq!(v["twoFactor"]["codeSentTo"], "trusted-device");
        assert_eq!(v["twoFactor"]["phoneNumbers"][0]["id"], 1);
    }
}
