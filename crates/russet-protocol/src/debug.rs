// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `debug.*`: debugger-only calls (ARCHITECTURE §8.5). Owner: W7 (`debug.prepare`, Wave 1), W11 (Wave 2), W18
//! (`debug.jitStats`, Wave 3). The rows of later waves keep their placeholder DTOs ([`Unspecified`]) until their
//! owners write them.

use std::collections::BTreeMap;

use russet_core::ids::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::AttachMode;
use crate::run::RunStartParams;
use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Which debugger front end will connect (`debug.prepare {frontend}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DebugFrontend {
    /// The IDE through the DAP hub (W11).
    Dap,
    /// `russet debug`: `lldb -s attach.lldb`.
    #[default]
    LldbCli,
    /// Nothing yet: `russet debug prepare` prints the plan.
    None,
}

/// The JIT servicer policy (Flutter, W18).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum JitPolicy {
    #[default]
    Auto,
    Native,
    LldbPython,
    Off,
}

/// Device-symbol tiers (W11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolTier {
    #[default]
    Auto,
    None,
    Core,
    Full,
}

/// The LLDB flavor (ARCHITECTURE §16.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DebuggerFlavor {
    #[default]
    Swift,
    Llvm,
}

/// Params of `debug.prepare`: the `run.start` params plus the debugger options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugPrepareParams {
    #[serde(flatten)]
    #[ts(flatten)]
    pub run: RunStartParams,
    /// Stay stopped after the attach (no `process continue`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stop_on_entry: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub frontend: Option<DebugFrontend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub jit: Option<JitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub symbols: Option<SymbolTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub flavor: Option<DebuggerFlavor>,
    /// `broker` (default) or the M1 fallback `legacy-async` (ARCHITECTURE §10.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attach_mode: Option<AttachMode>,
}

/// A toolchain component, with the wire values of `toolchain.*`'s `ToolInfo.component` (ARCHITECTURE §8.1, W3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DebugToolComponent {
    Swift,
    DarwinSdk,
    Linker,
    Lldb,
    LldbDap,
    Libpython,
    Xtool,
    Flutter,
    GenSnapshotIos,
    Llvm,
}

/// Where a resolved tool came from, in the wire shape of `toolchain.*`'s `ToolInfo.provenance` (W3): an object
/// tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DebugToolProvenance {
    /// Installed by Russet; `pin` is `<component>@<version>`.
    Managed { pin: String },
    /// Found on the system (`PATH`, `ldconfig`).
    System,
    /// An existing uv-managed python-build-standalone.
    Uv,
    /// Inside an xtool SDK bundle, read in place.
    #[serde(rename_all = "camelCase")]
    XtoolBundle { bundle: String },
    /// Installed by the user elsewhere.
    User,
    /// Named by the project (`[debug] lldb = "<path>"`).
    ProjectPin,
}

/// A resolved LLDB tool as `debug.prepare` reports it: the wire shape of `toolchain.resolve`'s `ToolInfo`
/// (ARCHITECTURE §8.1), field for field, so `PreparedDebug.lldb` can switch to W3's type once it is on `main`.
/// `env` is part of the contract: every launcher applies it (§16.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugToolInfo {
    /// `lldb` or `lldb-dap`.
    pub component: DebugToolComponent,
    pub version: String,
    pub path: String,
    pub provenance: DebugToolProvenance,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub ok: bool,
    #[serde(default)]
    pub problems: Vec<String>,
}

/// How LLDB reaches the broker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum BrokerTransport {
    Unix,
    Tcp,
}

/// The broker endpoint (ARCHITECTURE §10.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrokerInfo {
    pub transport: BrokerTransport,
    /// The socket path (`unix`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// The loopback port (`tcp`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub port: Option<u16>,
    /// What `process connect` takes: `unix-connect:///…/gdb.sock` or `connect://127.0.0.1:<port>`.
    pub lldb_url: String,
}

/// How the DAP hub is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum DapTransport {
    Unix,
    Pipe,
}

/// The per-session DAP hub endpoint (ARCHITECTURE §10.2; W11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DapInfo {
    pub transport: DapTransport,
    pub path: String,
}

/// Result of `debug.prepare` (`PreparedDebug`): the app is launched suspended and the broker holds the attach.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugPrepareResult {
    pub session_id: SessionId,
    pub pid: u32,
    /// The bundle ID on the device.
    pub bundle_id: String,
    pub lldb: DebugToolInfo,
    pub lldb_dap: DebugToolInfo,
    pub broker: BrokerInfo,
    /// LLDB settings (RD §8.4), before the attach commands.
    pub init_commands: Vec<String>,
    /// The Python-free attach sequence (RD §9.1); `legacy-async` adds the M1 sequence's `script` lines.
    pub attach_commands: Vec<String>,
    /// The `lldb-dap` attach configuration (RD §9.2).
    #[ts(type = "Record<string, unknown>")]
    pub dap_config: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dap: Option<DapInfo>,
    /// The `attach.lldb` the daemon wrote: init plus attach commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attach_script: Option<String>,
    pub attach_mode: AttachMode,
}
crate::rpc_method!(
    /// `debug.prepare` (class `device`, long-running). W7 (Wave 1).
    DebugPrepare,
    "debug.prepare",
    DebugPrepareParams => DebugPrepareResult,
    Permission::Device, long_running
);

/// Params of `debug.attach`: `{udid?, pid?|bundleId?, frontend?, sessionLease?} → PreparedDebug`.
pub type DebugAttachParams = Unspecified;
/// Result of `debug.attach`.
pub type DebugAttachResult = Unspecified;
crate::rpc_method!(
    /// `debug.attach` (class `device`, long-running). W11 (Wave 2).
    DebugAttach,
    "debug.attach",
    DebugAttachParams => DebugAttachResult,
    Permission::Device, long_running
);

/// Params of `debug.control`: `{sessionId, action: continue|pause|step-over|step-into|step-out, threadId?, waitForStopMs?} → {state, stop?{reason, threadId, frames[]}}`.
pub type DebugControlParams = Unspecified;
/// Result of `debug.control`.
pub type DebugControlResult = Unspecified;
crate::rpc_method!(
    /// `debug.control` (class `device`). W11 (Wave 2).
    DebugControl,
    "debug.control",
    DebugControlParams => DebugControlResult,
    Permission::Device
);

/// Params of `debug.inspect`: `{sessionId, what: threads|stack|variables, threadId?, frameId?, scope?, depth?} → DAP-shaped data`.
pub type DebugInspectParams = Unspecified;
/// Result of `debug.inspect`.
pub type DebugInspectResult = Unspecified;
crate::rpc_method!(
    /// `debug.inspect` (class `read`). W11 (Wave 2).
    DebugInspect,
    "debug.inspect",
    DebugInspectParams => DebugInspectResult,
    Permission::Read
);

/// Params of `debug.evaluate`: `{sessionId, expression, frameId?, context?: repl|watch} → {result, type}`.
pub type DebugEvaluateParams = Unspecified;
/// Result of `debug.evaluate`.
pub type DebugEvaluateResult = Unspecified;
crate::rpc_method!(
    /// `debug.evaluate` (class `device`). W11 (Wave 2).
    DebugEvaluate,
    "debug.evaluate",
    DebugEvaluateParams => DebugEvaluateResult,
    Permission::Device
);

/// Params of `debug.breakpoint.set`: `{sessionId?, file, line, condition?, logMessage?} → {breakpointId, verified, pending}`.
pub type DebugBreakpointSetParams = Unspecified;
/// Result of `debug.breakpoint.set`.
pub type DebugBreakpointSetResult = Unspecified;
crate::rpc_method!(
    /// `debug.breakpoint.set` (class `device`). W11 (Wave 2).
    DebugBreakpointSet,
    "debug.breakpoint.set",
    DebugBreakpointSetParams => DebugBreakpointSetResult,
    Permission::Device
);

/// Params of `debug.breakpoint.remove`: `{breakpointId} → {}`.
pub type DebugBreakpointRemoveParams = Unspecified;
/// Result of `debug.breakpoint.remove`.
pub type DebugBreakpointRemoveResult = Unspecified;
crate::rpc_method!(
    /// `debug.breakpoint.remove` (class `device`). W11 (Wave 2).
    DebugBreakpointRemove,
    "debug.breakpoint.remove",
    DebugBreakpointRemoveParams => DebugBreakpointRemoveResult,
    Permission::Device
);

/// Params of `debug.jitStats`: `{sessionId} → JitStats (pagesTouched must equal pagesNotified)`.
pub type DebugJitStatsParams = Unspecified;
/// Result of `debug.jitStats`.
pub type DebugJitStatsResult = Unspecified;
crate::rpc_method!(
    /// `debug.jitStats` (class `read`). W18 (Wave 3).
    DebugJitStats,
    "debug.jitStats",
    DebugJitStatsParams => DebugJitStatsResult,
    Permission::Read
);

/// Params of `debug.symbols.status`: `{udid?} → {build, dscUuid?, tier, bytesOnDisk}`.
pub type DebugSymbolsStatusParams = Unspecified;
/// Result of `debug.symbols.status`.
pub type DebugSymbolsStatusResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.status` (class `read`). W11 (Wave 2).
    DebugSymbolsStatus,
    "debug.symbols.status",
    DebugSymbolsStatusParams => DebugSymbolsStatusResult,
    Permission::Read
);

/// Params of `debug.symbols.plan`: `{udid?} → {files[], totalBytes, freeBytes, quotaBytes, ok, reason?}`.
pub type DebugSymbolsPlanParams = Unspecified;
/// Result of `debug.symbols.plan`.
pub type DebugSymbolsPlanResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.plan` (class `read`). W11 (Wave 2).
    DebugSymbolsPlan,
    "debug.symbols.plan",
    DebugSymbolsPlanParams => DebugSymbolsPlanResult,
    Permission::Read
);

/// Params of `debug.symbols.fetch`: `{udid?, tier: core|full, confirmBytes?} → SymbolSet`.
pub type DebugSymbolsFetchParams = Unspecified;
/// Result of `debug.symbols.fetch`.
pub type DebugSymbolsFetchResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.fetch` (class `device`, long-running). W11 (Wave 2).
    DebugSymbolsFetch,
    "debug.symbols.fetch",
    DebugSymbolsFetchParams => DebugSymbolsFetchResult,
    Permission::Device, long_running
);

/// Params of `debug.symbols.prune`: `{maxBytes?, build?} → {freedBytes}`.
pub type DebugSymbolsPruneParams = Unspecified;
/// Result of `debug.symbols.prune`.
pub type DebugSymbolsPruneResult = Unspecified;
crate::rpc_method!(
    /// `debug.symbols.prune` (class `system`). W11 (Wave 2).
    DebugSymbolsPrune,
    "debug.symbols.prune",
    DebugSymbolsPruneParams => DebugSymbolsPruneResult,
    Permission::System
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    DebugPrepare::INFO,
    DebugAttach::INFO,
    DebugControl::INFO,
    DebugInspect::INFO,
    DebugEvaluate::INFO,
    DebugBreakpointSet::INFO,
    DebugBreakpointRemove::INFO,
    DebugJitStats::INFO,
    DebugSymbolsStatus::INFO,
    DebugSymbolsPlan::INFO,
    DebugSymbolsFetch::INFO,
    DebugSymbolsPrune::INFO,
];
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
    fn prepare_params_flatten_the_run_params() {
        let p: DebugPrepareParams = serde_json::from_value(json!({
            "project": "/home/u/RussetProbe", "sessionLease": "detached", "frontend": "lldb-cli",
            "attachMode": "legacy-async", "stopOnEntry": true, "jit": "lldb-python", "flavor": "llvm"}))
        .unwrap();
        assert_eq!(p.run.project, "/home/u/RussetProbe");
        assert_eq!(p.run.session_lease, Some(crate::common::SessionLease::Detached));
        assert_eq!(p.frontend, Some(DebugFrontend::LldbCli));
        assert_eq!(p.attach_mode, Some(AttachMode::LegacyAsync));
        assert_eq!(
            (p.stop_on_entry, p.jit, p.flavor),
            (Some(true), Some(JitPolicy::LldbPython), Some(DebuggerFlavor::Llvm))
        );
        let back = serde_json::to_value(&p).unwrap();
        assert_eq!(back["project"], "/home/u/RussetProbe");
        assert_eq!(back["frontend"], "lldb-cli");
        let ts = <DebugPrepareParams as TS>::decl(&ts_rs::Config::default());
        assert!(ts.contains("project: string") && ts.contains("attachMode?: AttachMode"), "{ts}");
    }

    #[test]
    fn the_prepared_debug_shape() {
        let tool = |c: DebugToolComponent, name: &str| DebugToolInfo {
            component: c,
            version: "21.0.0".into(),
            path: format!("/usr/lib/swift/usr/bin/{name}"),
            provenance: DebugToolProvenance::System,
            env: BTreeMap::from([("PYTHONHOME".to_owned(), "/pbs".to_owned())]),
            ok: true,
            problems: vec![],
        };
        let r = DebugPrepareResult {
            session_id: serde_json::from_value(json!("ses_01J00000000000000000000000")).unwrap(),
            pid: 932,
            bundle_id: "dev.russet.test.probe".into(),
            lldb: tool(DebugToolComponent::Lldb, "lldb"),
            lldb_dap: tool(DebugToolComponent::LldbDap, "lldb-dap"),
            broker: BrokerInfo {
                transport: BrokerTransport::Unix,
                path: Some("/run/user/1000/russet/sessions/ses_x/gdb.sock".into()),
                port: None,
                lldb_url: "unix-connect:///run/user/1000/russet/sessions/ses_x/gdb.sock".into(),
            },
            init_commands: vec!["settings set target.load-cwd-lldbinit false".into()],
            attach_commands: vec!["platform select remote-ios".into()],
            dap_config: json!({"type": "lldb-dap", "request": "attach"}),
            dap: None,
            attach_script: Some("/p/.russet/debug/ses_x/attach.lldb".into()),
            attach_mode: AttachMode::Broker,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["sessionId"], "ses_01J00000000000000000000000");
        assert_eq!(v["lldbDap"]["component"], "lldb-dap");
        assert_eq!(v["lldb"]["provenance"], json!({"kind": "system"}));
        assert_eq!(v["broker"]["transport"], "unix");
        assert_eq!(v["broker"]["lldbUrl"], "unix-connect:///run/user/1000/russet/sessions/ses_x/gdb.sock");
        assert_eq!(v["attachMode"], "broker");
        assert!(v.get("dap").is_none());
        let back: DebugPrepareResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }

    /// W3's `ToolInfo` (ARCHITECTURE §8.1) on the wire: kebab-case components, provenance tagged by `kind`.
    #[test]
    fn tool_info_has_the_toolchain_wire_shape() {
        let t = DebugToolInfo {
            component: DebugToolComponent::Libpython,
            version: "3.12.15".into(),
            path: "/d/toolchains/python-shared/cpython-3.12.15/lib/libpython3.12.so".into(),
            provenance: DebugToolProvenance::Managed { pin: "python-shared@3.12.15".into() },
            env: BTreeMap::new(),
            ok: true,
            problems: vec![],
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["component"], "libpython");
        assert_eq!(v["provenance"], json!({"kind": "managed", "pin": "python-shared@3.12.15"}));
        let xtool =
            DebugToolProvenance::XtoolBundle { bundle: "/home/u/.swiftpm/swift-sdks/darwin.artifactbundle".into() };
        assert_eq!(
            serde_json::to_value(&xtool).unwrap(),
            json!({"kind": "xtool-bundle", "bundle": "/home/u/.swiftpm/swift-sdks/darwin.artifactbundle"})
        );
        assert_eq!(serde_json::to_value(DebugToolProvenance::ProjectPin).unwrap(), json!({"kind": "project-pin"}));
        assert_eq!(serde_json::to_value(DebugToolComponent::GenSnapshotIos).unwrap(), json!("gen-snapshot-ios"));
        let back: DebugToolInfo = serde_json::from_value(v).unwrap();
        assert_eq!(back, t);
    }
}
