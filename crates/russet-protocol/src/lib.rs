// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `russet/1` wire protocol (ARCHITECTURE §7–§9): every DTO, method and notification.
//!
//! Seam file: lead-only after W1 merges (PLAN §0 rule 2). The list of namespace modules is fixed here; each
//! area owns its `src/<namespace>.rs` and edits nothing else (PLAN §3).
//!
//! Every namespace module exports three arrays, which [`registry()`], [`notifications()`] and
//! [`client_methods()`] concatenate: `METHODS` (served by the daemon), `NOTIFICATIONS` (sent by the daemon)
//! and `CLIENT_METHODS` (requests the daemon sends to clients). W1 registered every name of ARCHITECTURE §8,
//! §9.1, §9.3 and §9.4 with placeholder DTOs ([`Unspecified`]), so unimplemented methods answer
//! `core.notImplemented` (1005) until their wave.

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

#[macro_use]
mod macros;
mod registry;

pub use registry::{
    MethodInfo, Namespace, NotificationInfo, RpcMethod, RpcNotification, TypeHooks, client_methods,
    namespace_of_module, namespaces, notifications, registry,
};
pub use russet_core::error::ErrorData;
pub use russet_core::ids::{ApprovalId, JobId, LeaseId, SessionId, SubscriptionId};
pub use russet_core::progress::{JobEventKind, LogStream};
pub use russet_core::rpc::{ClientKind, Confirm, ConnInfo, Method, NotificationMethod, Ownership, Permission};

/// Paths the exported macros use; not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use russet_core::Result as CoreResult;
}

/// The protocol version (ARCHITECTURE §7.7). Minor: additions only; major: removals, renames, semantics.
pub const PROTOCOL: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };

/// `{major, minor}` as exchanged in `initialize`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolVersion {
    pub major: u32,
    pub minor: u32,
}

/// Placeholder DTO of a method whose shape is not written yet (freeze F0): any JSON object, unknown fields
/// ignored. The owning area replaces its `…Params`/`…Result` aliases with real structs.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
pub struct Unspecified {}

// Namespace modules: fixed by W1; each is owned by its area (PLAN §3).
pub mod account;
pub mod app;
pub mod approval;
pub mod asc;
pub mod assets;
pub mod audit;
pub mod build;
pub mod common;
pub mod config;
pub mod crash;
pub mod daemon;
pub mod debug;
pub mod device;
pub mod doctor;
pub mod files;
pub mod flutter_build;
pub mod flutter_run;
pub mod forward;
pub mod ib;
pub mod ide;
pub mod identity;
pub mod initialize;
pub mod input;
pub mod job;
pub mod lease;
pub mod location;
pub mod logs;
pub mod mcp;
pub mod perf;
pub mod portal;
pub mod process;
pub mod project;
pub mod provision;
pub mod run;
pub mod screen;
pub mod sdk;
pub mod secrets;
pub mod ship;
pub mod sign;
pub mod subscription;
pub mod test;
pub mod toolchain;
pub mod ui;

macro_rules! namespace_table {
    ($($module:ident),* $(,)?) => {
        /// Every namespace module, in catalog order (ARCHITECTURE §8, §9).
        const NAMESPACES: &[Namespace] = &[$(
            Namespace {
                module: stringify!($module),
                methods: $module::METHODS,
                notifications: $module::NOTIFICATIONS,
                client_methods: $module::CLIENT_METHODS,
            },
        )*];
    };
}

namespace_table!(
    // W2: core runtime (ARCHITECTURE §8.1, §9.1, §9.3, §9.5)
    common,
    initialize,
    daemon,
    subscription,
    lease,
    job,
    approval,
    audit,
    ui,
    // W3: config, secrets, toolchain, SDK, doctor
    config,
    secrets,
    toolchain,
    sdk,
    doctor,
    // W4 / W10: device plane (§8.2)
    device,
    app,
    process,
    forward,
    logs,
    screen,
    input,
    files,
    perf,
    location,
    // W5 / W16 / W17 / W21 / W21b / W24: project, build, assets, Flutter build (§8.3)
    project,
    build,
    assets,
    ib,
    flutter_build,
    // W6 / W12 / W22 / W23: accounts, signing, provisioning, App Store Connect (§8.4)
    account,
    identity,
    portal,
    provision,
    sign,
    asc,
    ship,
    // W7 / W11 / W18 / W32: run, debug, crashes, Flutter run, tests (§8.5)
    run,
    debug,
    crash,
    flutter_run,
    test,
    // W13 / W20: MCP administration (§8.6)
    mcp,
    // W11 / W14: IDE reverse requests (§9.4)
    ide,
);
