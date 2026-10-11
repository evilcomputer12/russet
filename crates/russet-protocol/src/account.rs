// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `account.*`: accounts and the free Apple ID flow (ARCHITECTURE §8.4). Owner: W6 (Wave 1), W12 (Wave 2), W22 (appleId and anisette rows, Wave 3).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `account.list`: `{} → {accounts: Account[], default?}`.
pub type AccountListParams = Unspecified;
/// Result of `account.list`.
pub type AccountListResult = Unspecified;
crate::rpc_method!(
    /// `account.list` (class `read`). W6 (Wave 1).
    AccountList,
    "account.list",
    AccountListParams => AccountListResult,
    Permission::Read
);

/// Params of `account.addAscKey`: `{keyId, issuerId?, p8Path, label?} → Account (cli/ide only)`.
pub type AccountAddAscKeyParams = Unspecified;
/// Result of `account.addAscKey`.
pub type AccountAddAscKeyResult = Unspecified;
crate::rpc_method!(
    /// `account.addAscKey` (class `secrets`). W6 (Wave 1).
    AccountAddAscKey,
    "account.addAscKey",
    AccountAddAscKeyParams => AccountAddAscKeyResult,
    Permission::Secrets
);

/// Params of `account.importXtool`: `{dryRun?} → {accounts[], identities[], warnings[]} (cli/ide only; explicit one-time import)`.
pub type AccountImportXtoolParams = Unspecified;
/// Result of `account.importXtool`.
pub type AccountImportXtoolResult = Unspecified;
crate::rpc_method!(
    /// `account.importXtool` (class `secrets`). W6 (Wave 1).
    AccountImportXtool,
    "account.importXtool",
    AccountImportXtoolParams => AccountImportXtoolResult,
    Permission::Secrets
);

/// Params of `account.verify`: `{accountId?} → {ok, teamId?, roleProbe{certificates, profiles, builds}}`.
pub type AccountVerifyParams = Unspecified;
/// Result of `account.verify`.
pub type AccountVerifyResult = Unspecified;
crate::rpc_method!(
    /// `account.verify` (class `read`). W6 (Wave 1).
    AccountVerify,
    "account.verify",
    AccountVerifyParams => AccountVerifyResult,
    Permission::Read
);

/// Params of `account.remove`: `{accountId, wipeSecrets?} → {}`.
pub type AccountRemoveParams = Unspecified;
/// Result of `account.remove`.
pub type AccountRemoveResult = Unspecified;
crate::rpc_method!(
    /// `account.remove` (class `secrets`). W12 (Wave 2).
    AccountRemove,
    "account.remove",
    AccountRemoveParams => AccountRemoveResult,
    Permission::Secrets
);

/// Params of `account.setDefault`: `{accountId} → {}`.
pub type AccountSetDefaultParams = Unspecified;
/// Result of `account.setDefault`.
pub type AccountSetDefaultResult = Unspecified;
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
