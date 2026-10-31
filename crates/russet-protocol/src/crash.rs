// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `crash.*`: the local crash store and symbolication (ARCHITECTURE §8.5, RD §13). Owner: W11.
//!
//! The shared [`CrashSummary`] is the wire shape of a stored `.ips` report; [`SymbolicatedCrash`] is the
//! symbolicated view `crash.symbolicate` returns as JSON (or a text rendering when `format: text`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{MethodInfo, NotificationInfo, RpcMethod};

/// One stored crash report (`crash.list`, `crash.fetch`): the fields a reader needs before opening it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashSummary {
    /// The stable id in the local store (`<timestamp>-<incident>`).
    pub crash_id: String,
    pub bundle_id: String,
    /// The crashing process name (the executable), for example `RussetProbe`.
    pub process: String,
    /// RFC 3339 UTC of the crash (the `.ips` header's `timestamp`, normalised).
    pub timestamp: String,
    /// The exception type, for example `EXC_BAD_ACCESS (SIGSEGV)`.
    pub exception: String,
    /// The terminating signal name, for example `SIGSEGV`, when the report has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub signal: Option<String>,
    /// Whether a symbolicated rendering is already cached next to the `.ips`.
    pub symbolicated: bool,
}

/// One symbolicated stack frame (`SymbolicatedCrash`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashFrame {
    /// The frame index within its thread.
    pub index: u32,
    /// The image (binary) the frame is in, for example `RussetProbe` or `UIKitCore`.
    pub image: String,
    /// The byte offset into the image.
    #[ts(type = "number")]
    pub image_offset: u64,
    /// The resolved symbol, when Russet could name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub symbol: Option<String>,
    /// The source file, when a dSYM resolved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<String>,
    /// The source line, when a dSYM resolved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line: Option<u32>,
    /// Whether this frame was inlined (`symbolic` inline expansion).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inlined: bool,
}

/// One thread of a symbolicated crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashThread {
    pub index: u32,
    /// Whether this is the crashing thread.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crashed: bool,
    /// The thread name or queue label, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    pub frames: Vec<CrashFrame>,
}

/// The symbolicated crash `crash.symbolicate` returns as JSON (RD §11.4, §13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SymbolicatedCrash {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: CrashSummary,
    /// The termination reason line, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub termination: Option<String>,
    pub threads: Vec<CrashThread>,
    /// How far symbolication got: `app` (dSYM app frames), `system` (tier ≥ 1) or `none`.
    pub coverage: SymbolCoverage,
}

/// How much of a crash Russet could symbolicate (RD §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolCoverage {
    /// App frames resolved from a dSYM or debug map.
    App,
    /// System frames resolved too (symbols tier ≥ 1).
    System,
    /// Only `<image> + 0x<offset>`.
    None,
}

/// The rendering `crash.symbolicate` returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CrashFormat {
    #[default]
    Json,
    Text,
}

/// Params of `crash.list`: filter the local store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    /// Only crashes at or after this RFC 3339 time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `crash.list`: newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashListResult {
    pub crashes: Vec<CrashSummary>,
}
crate::rpc_method!(
    /// `crash.list` (class `read`). W11 (Wave 2).
    CrashList,
    "crash.list",
    CrashListParams => CrashListResult,
    Permission::Read
);

/// Params of `crash.fetch`: pull new reports off the device into the store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashFetchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<String>,
}

/// Result of `crash.fetch`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashFetchResult {
    pub fetched: Vec<CrashSummary>,
}
crate::rpc_method!(
    /// `crash.fetch` (class `device`, long-running). W11 (Wave 2).
    CrashFetch,
    "crash.fetch",
    CrashFetchParams => CrashFetchResult,
    Permission::Device, long_running
);

/// Params of `crash.symbolicate`: a stored crash by id, or an `.ips` file by path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct CrashSymbolicateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub crash_id: Option<String>,
    /// An `.ips` file to symbolicate without storing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub format: Option<CrashFormat>,
}

/// Result of `crash.symbolicate`: the symbolicated crash (JSON), or a text rendering under `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(untagged)]
pub enum CrashSymbolicateResult {
    /// `format: json` (the default).
    Json { crash: SymbolicatedCrash },
    /// `format: text`: an Xcode-like rendering.
    Text { crash: String },
}
crate::rpc_method!(
    /// `crash.symbolicate` (class `read`). W11 (Wave 2).
    CrashSymbolicate,
    "crash.symbolicate",
    CrashSymbolicateParams => CrashSymbolicateResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[CrashList::INFO, CrashFetch::INFO, CrashSymbolicate::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;

    #[test]
    fn crash_summary_is_camel_case() {
        let s = CrashSummary {
            crash_id: "2026-10-30T09-00-00-abcd".into(),
            bundle_id: "dev.russet.test.probe".into(),
            process: "RussetProbe".into(),
            timestamp: "2026-10-30T09:00:00.000Z".into(),
            exception: "EXC_BAD_ACCESS (SIGSEGV)".into(),
            signal: Some("SIGSEGV".into()),
            symbolicated: false,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["crashId"], "2026-10-30T09-00-00-abcd");
        assert_eq!(v["bundleId"], "dev.russet.test.probe");
        let back: CrashSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn symbolicate_result_is_untagged() {
        let text = CrashSymbolicateResult::Text { crash: "Thread 0 crashed:\n0  RussetProbe  main".into() };
        assert_eq!(serde_json::to_value(&text).unwrap(), json!({"crash": "Thread 0 crashed:\n0  RussetProbe  main"}));
        let back: CrashSymbolicateResult =
            serde_json::from_value(json!({"crash": "Thread 0 crashed:\n0  RussetProbe  main"})).unwrap();
        assert!(matches!(back, CrashSymbolicateResult::Text { .. }));
        assert_eq!(serde_json::to_value(CrashFormat::Text).unwrap(), json!("text"));
    }
}
