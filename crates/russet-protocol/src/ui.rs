// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ui.*`: requests the daemon sends to `cli` and `ide` clients (ARCHITECTURE §9.3), and `ui.showMessage`.
//! Owner: W2.
//!
//! The daemon has no UI: a confirmation goes to the requester as `ui.confirm` (or a typed `ui.prompt`), and
//! secret prompts never go to MCP connections (ARCHITECTURE §7.5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::ClientRef;
use crate::{MethodInfo, NotificationInfo, RpcMethod, RpcNotification};

/// Params of `ui.confirm` (served by `cli` with a TTY or `--yes`, and `ide`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiConfirmParams {
    pub title: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    /// Destructive: the client shows it as dangerous.
    pub danger: bool,
    pub requested_by: ClientRef,
    /// Unanswered after this long counts as declined.
    pub timeout_ms: u64,
}

/// Result of `ui.confirm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiConfirmResult {
    pub confirmed: bool,
}

crate::rpc_method!(
    /// `ui.confirm` (served by the client). W2.
    UiConfirm,
    "ui.confirm",
    UiConfirmParams => UiConfirmResult,
    Permission::None
);

/// What `ui.prompt` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum PromptKind {
    Text,
    Password,
    Code,
    /// The user types `expected` to confirm an account-destructive action or a review submission.
    TypedConfirmation,
}

/// Params of `ui.prompt` (`cli` and `ide` only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiPromptParams {
    pub title: String,
    pub message: String,
    pub kind: PromptKind,
    /// Do not echo; never sent to `mcp`.
    pub secret: bool,
    /// For `typed-confirmation`: the text to type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected: Option<String>,
    pub requested_by: ClientRef,
    pub timeout_ms: u64,
}

/// Result of `ui.prompt`. `Debug` never prints the value (it may be a password).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiPromptResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<String>,
    pub cancelled: bool,
}

impl std::fmt::Debug for UiPromptResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiPromptResult")
            .field("value", &self.value.as_ref().map(|_| "***"))
            .field("cancelled", &self.cancelled)
            .finish()
    }
}

crate::rpc_method!(
    /// `ui.prompt` (served by the client). W2.
    UiPrompt,
    "ui.prompt",
    UiPromptParams => UiPromptResult,
    Permission::None
);

/// Params of `ui.openUrl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiOpenUrlParams {
    pub url: String,
    pub reason: String,
}

/// Result of `ui.openUrl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiOpenUrlResult {
    pub opened: bool,
}

crate::rpc_method!(
    /// `ui.openUrl` (served by the client). W2.
    UiOpenUrl,
    "ui.openUrl",
    UiOpenUrlParams => UiOpenUrlResult,
    Permission::None
);

/// Message levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

/// Params of the `ui.showMessage` notification (to the target client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct UiShowMessageParams {
    pub level: MessageLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_values_never_print() {
        let r = UiPromptResult { value: Some("hunter2".into()), cancelled: false };
        assert!(!format!("{r:?}").contains("hunter2"));
        assert_eq!(
            serde_json::to_value(PromptKind::TypedConfirmation).expect("serializes"),
            serde_json::json!("typed-confirmation")
        );
    }
}
