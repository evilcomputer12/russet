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

/// What to attach to (`debug.attach`): a running pid, or the newest process of a bundle id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugAttachParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub frontend: Option<DebugFrontend>,
    /// The local build with debug info to load, when known (its UUID must match the running image).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_lease: Option<crate::common::SessionLease>,
}
/// Result of `debug.attach`: the same `PreparedDebug` shape as `debug.prepare`.
pub type DebugAttachResult = DebugPrepareResult;
crate::rpc_method!(
    /// `debug.attach` (class `device`, long-running). W11 (Wave 2).
    DebugAttach,
    "debug.attach",
    DebugAttachParams => DebugAttachResult,
    Permission::Device, long_running
);

/// An execution-control action (`debug.control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum ControlAction {
    Continue,
    Pause,
    StepOver,
    StepInto,
    StepOut,
}

/// One stack frame in DAP shape (`debug.inspect what: stack`, `debug.control` stops).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    #[ts(type = "number")]
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column: Option<u32>,
    /// The frame's program counter, as a hex string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub instruction_pointer: Option<String>,
}

/// A thread in DAP shape (`debug.inspect what: threads`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ThreadInfo {
    #[ts(type = "number")]
    pub id: u64,
    pub name: String,
}

/// A variable in DAP shape (`debug.inspect what: variables`, `debug.evaluate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, rename = "type")]
    pub ty: Option<String>,
    /// Non-zero when the variable can be expanded further (a DAP `variablesReference`).
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(type = "number")]
    pub variables_reference: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Where the process stopped (`debug.control` result).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct StopInfo {
    /// The DAP stop reason, for example `breakpoint`, `step`, `exception`, `pause`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub thread_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    pub frames: Vec<StackFrame>,
}

/// Params of `debug.control`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugControlParams {
    pub session_id: SessionId,
    pub action: ControlAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub thread_id: Option<u64>,
    /// Wait up to this long for the next stop before returning (0 or omitted returns at once).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub wait_for_stop_ms: Option<u64>,
}

/// Result of `debug.control`: the resulting state, and the stop when one happened (so agents need not poll).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugControlResult {
    /// `running` or `stopped`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stop: Option<StopInfo>,
}
crate::rpc_method!(
    /// `debug.control` (class `device`). W11 (Wave 2).
    DebugControl,
    "debug.control",
    DebugControlParams => DebugControlResult,
    Permission::Device
);

/// What `debug.inspect` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum InspectWhat {
    Threads,
    Stack,
    Variables,
}

/// Params of `debug.inspect`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugInspectParams {
    pub session_id: SessionId,
    pub what: InspectWhat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub thread_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub frame_id: Option<u64>,
    /// The variable scope (`locals`, `globals`, …) or a DAP `variablesReference`, for `what: variables`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub depth: Option<u32>,
}

/// Result of `debug.inspect`: DAP-shaped data; the field for the requested `what` is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugInspectResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub threads: Option<Vec<ThreadInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stack_frames: Option<Vec<StackFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub variables: Option<Vec<Variable>>,
}
crate::rpc_method!(
    /// `debug.inspect` (class `read`). W11 (Wave 2).
    DebugInspect,
    "debug.inspect",
    DebugInspectParams => DebugInspectResult,
    Permission::Read
);

/// The expression context (`debug.evaluate`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum EvalContext {
    #[default]
    Repl,
    Watch,
}

/// Params of `debug.evaluate`: `po`/`expr`, or an LLDB command prefixed with a backtick (RD §19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugEvaluateParams {
    pub session_id: SessionId,
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub frame_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<EvalContext>,
}

/// Result of `debug.evaluate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugEvaluateResult {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, rename = "type")]
    pub ty: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    #[ts(type = "number")]
    pub variables_reference: u64,
}
crate::rpc_method!(
    /// `debug.evaluate` (class `device`). W11 (Wave 2).
    DebugEvaluate,
    "debug.evaluate",
    DebugEvaluateParams => DebugEvaluateResult,
    Permission::Device
);

/// Params of `debug.breakpoint.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointSetParams {
    /// The session; when omitted, the breakpoint is a pending source breakpoint applied to every matching session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<SessionId>,
    pub file: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub log_message: Option<String>,
}

/// Result of `debug.breakpoint.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointSetResult {
    pub breakpoint_id: u32,
    pub verified: bool,
    /// Whether the breakpoint is pending a load that binds it.
    pub pending: bool,
}
crate::rpc_method!(
    /// `debug.breakpoint.set` (class `device`). W11 (Wave 2).
    DebugBreakpointSet,
    "debug.breakpoint.set",
    DebugBreakpointSetParams => DebugBreakpointSetResult,
    Permission::Device
);

/// Params of `debug.breakpoint.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointRemoveParams {
    pub breakpoint_id: u32,
}

/// Result of `debug.breakpoint.remove`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugBreakpointRemoveResult {}
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

/// Params of `debug.symbols.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsStatusParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
}

/// Result of `debug.symbols.status`: what the local symbol cache holds for this device's build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsStatusResult {
    /// The device's iOS build (`ProductBuildVersion`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dsc_uuid: Option<String>,
    /// 0 (memory index only), 1 (core) or 2 (full).
    pub tier: u8,
    #[ts(type = "number")]
    pub bytes_on_disk: u64,
    /// Whether the raw dyld shared cache is still present (it can be pruned after extraction).
    pub raw_dsc_present: bool,
}
crate::rpc_method!(
    /// `debug.symbols.status` (class `read`). W11 (Wave 2).
    DebugSymbolsStatus,
    "debug.symbols.status",
    DebugSymbolsStatusParams => DebugSymbolsStatusResult,
    Permission::Read
);

/// Params of `debug.symbols.plan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsPlanParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// The tier to plan for (defaults to `core`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tier: Option<SymbolTier>,
}

/// One file a symbol fetch would download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanFile {
    pub path: String,
    #[ts(type = "number")]
    pub size: u64,
}

/// Result of `debug.symbols.plan`: the files and sizes, with the space and quota check (RD §11.2 N3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsPlanResult {
    pub files: Vec<PlanFile>,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub free_bytes: u64,
    #[ts(type = "number")]
    pub quota_bytes: u64,
    /// Whether a fetch is allowed: free space ≥ total + 2 GiB and the quota admits it.
    pub ok: bool,
    /// Why not, when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}
crate::rpc_method!(
    /// `debug.symbols.plan` (class `read`). W11 (Wave 2).
    DebugSymbolsPlan,
    "debug.symbols.plan",
    DebugSymbolsPlanParams => DebugSymbolsPlanResult,
    Permission::Read
);

/// Params of `debug.symbols.fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsFetchParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub udid: Option<String>,
    /// `core` or `full` (not `auto`/`none`).
    pub tier: SymbolTier,
    /// Headless and MCP callers must pass the planned total; interactive clients are asked with `ui.confirm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub confirm_bytes: Option<u64>,
    /// Keep the raw dyld shared cache after extraction (default: delete it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep_raw_dsc: Option<bool>,
}

/// Result of `debug.symbols.fetch` (`SymbolSet`): the extracted symbol set now on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsFetchResult {
    pub build: String,
    pub dsc_uuid: String,
    pub tier: u8,
    #[ts(type = "number")]
    pub bytes_on_disk: u64,
    /// The sysroot LLDB loads with `--sysroot`.
    pub path: String,
}
crate::rpc_method!(
    /// `debug.symbols.fetch` (class `device`, long-running). W11 (Wave 2).
    DebugSymbolsFetch,
    "debug.symbols.fetch",
    DebugSymbolsFetchParams => DebugSymbolsFetchResult,
    Permission::Device, long_running
);

/// Params of `debug.symbols.prune`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsPruneParams {
    /// Prune least-recently-used sets until the cache is at most this big (defaults to the quota).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub max_bytes: Option<u64>,
    /// Prune only this build's set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub build: Option<String>,
}

/// Result of `debug.symbols.prune`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DebugSymbolsPruneResult {
    #[ts(type = "number")]
    pub freed_bytes: u64,
}
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
