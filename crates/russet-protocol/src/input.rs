// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `input.*`: HID input (ARCHITECTURE §8.2 [D 51]). Owner: W10.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `input.tap`: `{udid?, x, y, space?: normalized|pixels|points|screenshot, holdMs?} → {}`.
pub type InputTapParams = Unspecified;
/// Result of `input.tap`.
pub type InputTapResult = Unspecified;
crate::rpc_method!(
    /// `input.tap` (class `device`). W10 (Wave 2).
    InputTap,
    "input.tap",
    InputTapParams => InputTapResult,
    Permission::Device
);

/// Params of `input.swipe`: `{udid?, from{x,y}, to{x,y}, space?, durationMs?} → {}`.
pub type InputSwipeParams = Unspecified;
/// Result of `input.swipe`.
pub type InputSwipeResult = Unspecified;
crate::rpc_method!(
    /// `input.swipe` (class `device`). W10 (Wave 2).
    InputSwipe,
    "input.swipe",
    InputSwipeParams => InputSwipeResult,
    Permission::Device
);

/// Params of `input.button`: `{udid?, button: home|lock|volume-up|volume-down|mute|siri, action?: press|down|up} → {}`.
pub type InputButtonParams = Unspecified;
/// Result of `input.button`.
pub type InputButtonResult = Unspecified;
crate::rpc_method!(
    /// `input.button` (class `device`). W10 (Wave 2).
    InputButton,
    "input.button",
    InputButtonParams => InputButtonResult,
    Permission::Device
);

/// Params of `input.text`: `{udid?, text, submit?} → {}`.
pub type InputTextParams = Unspecified;
/// Result of `input.text`.
pub type InputTextResult = Unspecified;
crate::rpc_method!(
    /// `input.text` (class `device`). W10 (Wave 2).
    InputText,
    "input.text",
    InputTextParams => InputTextResult,
    Permission::Device
);

/// Params of `input.key`: `{udid?, key|usage, down?} → {}`.
pub type InputKeyParams = Unspecified;
/// Result of `input.key`.
pub type InputKeyResult = Unspecified;
crate::rpc_method!(
    /// `input.key` (class `device`). W10 (Wave 2).
    InputKey,
    "input.key",
    InputKeyParams => InputKeyResult,
    Permission::Device
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[InputTap::INFO, InputSwipe::INFO, InputButton::INFO, InputText::INFO, InputKey::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
