// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ui.*`: requests the daemon sends to `cli` and `ide` clients (ARCHITECTURE §9.3), and `ui.showMessage`. Owner: W2.
//!
//! F0 stub (W1): every name of ARCHITECTURE §9.3 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification, Unspecified};

/// Params of `ui.confirm`: `{title, message, detail?, danger, requestedBy{kind,name}, timeoutMs} → {confirmed}; served by cli (TTY) and ide`.
pub type UiConfirmParams = Unspecified;
/// Result of `ui.confirm`.
pub type UiConfirmResult = Unspecified;
crate::rpc_method!(
    /// `ui.confirm` (served by the client). W2.
    UiConfirm,
    "ui.confirm",
    UiConfirmParams => UiConfirmResult,
    Permission::None
);

/// Params of `ui.prompt`: `{title, message, kind: text|password|code|typed-confirmation, secret, expected?, requestedBy, timeoutMs} → {value?, cancelled}; cli and ide only`.
pub type UiPromptParams = Unspecified;
/// Result of `ui.prompt`.
pub type UiPromptResult = Unspecified;
crate::rpc_method!(
    /// `ui.prompt` (served by the client). W2.
    UiPrompt,
    "ui.prompt",
    UiPromptParams => UiPromptResult,
    Permission::None
);

/// Params of `ui.openUrl`: `{url, reason} → {opened}; cli and ide`.
pub type UiOpenUrlParams = Unspecified;
/// Result of `ui.openUrl`.
pub type UiOpenUrlResult = Unspecified;
crate::rpc_method!(
    /// `ui.openUrl` (served by the client). W2.
    UiOpenUrl,
    "ui.openUrl",
    UiOpenUrlParams => UiOpenUrlResult,
    Permission::None
);

/// Params of the `ui.showMessage` notification: `{level: info|warning|error, message, actions?}; to the target client`.
pub type UiShowMessageParams = Unspecified;
crate::rpc_notification!(
    /// `ui.showMessage` (daemon → client). W2.
    UiShowMessage,
    "ui.showMessage",
    UiShowMessageParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[UiShowMessage::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[UiConfirm::INFO, UiPrompt::INFO, UiOpenUrl::INFO];
