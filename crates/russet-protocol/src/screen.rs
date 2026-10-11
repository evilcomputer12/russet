// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `screen.*`: screenshots, the live mirror and UI elements. Owner: W10.
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.2 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified};

/// Params of `screen.screenshot`: `{udid?, format?: png|jpeg, maxDimension?, scale?, quality?, toFile?} → {imageBase64?|path?, format, width, height, orientation}`.
pub type ScreenScreenshotParams = Unspecified;
/// Result of `screen.screenshot`.
pub type ScreenScreenshotResult = Unspecified;
crate::rpc_method!(
    /// `screen.screenshot` (class `read`). W10 (Wave 2).
    ScreenScreenshot,
    "screen.screenshot",
    ScreenScreenshotParams => ScreenScreenshotResult,
    Permission::Read
);

/// Params of `screen.mirror.start`: `{udid?, codecs: (hevc|avc|jpeg|poll)[], maxFps?, input?} → {streamId, url, token, codec, width, height} (class: read, input → device)`.
pub type ScreenMirrorStartParams = Unspecified;
/// Result of `screen.mirror.start`.
pub type ScreenMirrorStartResult = Unspecified;
crate::rpc_method!(
    /// `screen.mirror.start` (class `read`, `input: true` → `device`; resolved per call, ceiling `device`).
    /// W10 (Wave 2).
    ScreenMirrorStart,
    "screen.mirror.start",
    ScreenMirrorStartParams => ScreenMirrorStartResult,
    Permission::Device, permission_fn = screen_mirror_start_permission
);

/// Per-call class of `screen.mirror.start` [D 66]: `read`, or `device` with `input: true`.
///
/// F0 stub: fails closed at the ceiling (`device`) until the owner reads the params.
pub fn screen_mirror_start_permission(
    _p: &ScreenMirrorStartParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(Permission::Device)
}

/// Params of `screen.mirror.stop`: `{streamId} → {}`.
pub type ScreenMirrorStopParams = Unspecified;
/// Result of `screen.mirror.stop`.
pub type ScreenMirrorStopResult = Unspecified;
crate::rpc_method!(
    /// `screen.mirror.stop` (class `read`). W10 (Wave 2).
    ScreenMirrorStop,
    "screen.mirror.stop",
    ScreenMirrorStopParams => ScreenMirrorStopResult,
    Permission::Read
);

/// Params of `screen.elements`: `{udid?, source?: accessibility|wda} → {elements: UiElement[], source}`.
pub type ScreenElementsParams = Unspecified;
/// Result of `screen.elements`.
pub type ScreenElementsResult = Unspecified;
crate::rpc_method!(
    /// `screen.elements` (class `read`). W10 (Wave 2).
    ScreenElements,
    "screen.elements",
    ScreenElementsParams => ScreenElementsResult,
    Permission::Read
);

/// Params of `screen.press`: `{udid?, elementId} → {}`.
pub type ScreenPressParams = Unspecified;
/// Result of `screen.press`.
pub type ScreenPressResult = Unspecified;
crate::rpc_method!(
    /// `screen.press` (class `device`). W10 (Wave 3).
    ScreenPress,
    "screen.press",
    ScreenPressParams => ScreenPressResult,
    Permission::Device
);

/// Params of the `screen.mirror.ended` notification: `{streamId, code: media-in-use|device-gone|stream-failed, message}; to the starter`.
pub type ScreenMirrorEndedParams = Unspecified;
crate::rpc_notification!(
    /// `screen.mirror.ended` (daemon → client). W10.
    ScreenMirrorEnded,
    "screen.mirror.ended",
    ScreenMirrorEndedParams
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] =
    &[ScreenScreenshot::INFO, ScreenMirrorStart::INFO, ScreenMirrorStop::INFO, ScreenElements::INFO, ScreenPress::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[ScreenMirrorEnded::INFO];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
