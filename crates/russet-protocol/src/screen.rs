// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `screen.*`: screenshots, the live mirror and UI elements. Owner: W10.
//!
//! `screen.screenshot` and `screen.mirror.*` have their DTOs. The other names of ARCHITECTURE §8.2 are still F0 stubs with placeholder DTOs
//! ([`Unspecified`]): replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConnInfo, MethodInfo, NotificationInfo, Ownership, Permission, RpcMethod, RpcNotification, Unspecified};

/// An image encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
}

/// Which way up the screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Params of `screen.screenshot`: `{udid?, format?: png|jpeg, maxDimension?, scale?, quality?, toFile?} → {imageBase64?|path?, format, width, height, orientation}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScreenScreenshotParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// The encoding (default `png`). The device's screenshot service answers PNG only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub format: Option<ImageFormat>,
    /// The longest side, in pixels (downscaling; not available yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_dimension: Option<u32>,
    /// A scale factor in (0, 1] (downscaling; not available yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scale: Option<f64>,
    /// JPEG quality, 1–100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub quality: Option<u8>,
    /// Write the image to this absolute path instead of returning it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to_file: Option<String>,
}

/// Result of `screen.screenshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScreenScreenshotResult {
    /// The image, base64 (absent with `toFile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub image_base64: Option<String>,
    /// The file written (with `toFile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
}
crate::rpc_method!(
    /// `screen.screenshot` (class `read`). W10 (Wave 2).
    ScreenScreenshot,
    "screen.screenshot",
    ScreenScreenshotParams => ScreenScreenshotResult,
    Permission::Read
);

/// A mirror codec, best first: HEVC from the device's display service, AVC, MJPEG, or screenshot polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum MirrorCodec {
    Hevc,
    Avc,
    Jpeg,
    Poll,
}

/// Params of `screen.mirror.start`: `{udid?, codecs: (hevc|avc|jpeg|poll)[], maxFps?, input?} → {streamId, url, token, codec, width, height} (class: read, input → device)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScreenMirrorStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// The codecs the viewer decodes, best first. The daemon streams HEVC; a viewer without it polls
    /// `screen.screenshot` instead.
    #[serde(default)]
    pub codecs: Vec<MirrorCodec>,
    /// A frame-rate cap (not applied yet: the device paces the stream).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_fps: Option<u32>,
    /// Also accept touch, button, key and text messages on the WebSocket (class `device`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub input: Option<bool>,
}

/// Result of `screen.mirror.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScreenMirrorStartResult {
    /// `str_…`.
    pub stream_id: String,
    /// `ws://127.0.0.1:<port>/mirror/v1?stream=<streamId>` (ARCHITECTURE §10.1).
    pub url: String,
    /// The single-use token for the WebSocket's first message.
    pub token: String,
    pub codec: MirrorCodec,
    /// The screen's size in pixels.
    pub width: u32,
    pub height: u32,
}
crate::rpc_method!(
    /// `screen.mirror.start` (class `read`, `input: true` → `device`; resolved per call, ceiling `device`).
    /// W10 (Wave 2).
    ScreenMirrorStart,
    "screen.mirror.start",
    ScreenMirrorStartParams => ScreenMirrorStartResult,
    Permission::Device, permission_fn = screen_mirror_start_permission
);

/// Per-call class of `screen.mirror.start` [D 66]: `read`, or `device` with `input: true`.
pub fn screen_mirror_start_permission(
    p: &ScreenMirrorStartParams,
    _c: &ConnInfo,
    _o: &dyn Ownership,
) -> russet_core::Result<Permission> {
    Ok(if p.input == Some(true) { Permission::Device } else { Permission::Read })
}

/// Params of `screen.mirror.stop`: `{streamId} → {}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ScreenMirrorStopParams {
    pub stream_id: String,
}
/// Result of `screen.mirror.stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ScreenMirrorStopResult {}
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
