// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! RPC traits (pure; no I/O). `russet-protocol` declares every method with `rpc_method!`, which implements
//! [`Method`]; `russet-ipc` routes on it; the daemon's permission engine calls [`Method::permission`].

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ids::{JobId, LeaseId};

/// A JSON-RPC request method (ARCHITECTURE §7–§8).
pub trait Method: 'static {
    /// The wire name, for example `"device.list"`.
    const NAME: &'static str;
    /// The documented ceiling (ARCHITECTURE §8 "Class"); MCP listings and docs use it.
    const PERMISSION: Permission;
    /// `Plain`, or `Typed` (a `ui.prompt` typed confirmation that `--yes` cannot pre-confirm).
    const CONFIRM: Confirm;
    /// Long-running methods accept `progressToken` and `background` and create a job (ARCHITECTURE §7.6).
    const LONG_RUNNING: bool;
    type Params: DeserializeOwned + Serialize + schemars::JsonSchema + Send + Sync + 'static;
    type Result: DeserializeOwned + Serialize + schemars::JsonSchema + Send + Sync + 'static;

    /// Per-call class [D 66]. Default: [`Self::PERMISSION`]. Overridden (through `rpc_method!(…,
    /// permission_fn = …)`) where the class depends on params or ownership: `config.set`/`config.unset`
    /// (scope), `doctor.run` (fix), `screen.mirror.start` (input), `lease.release` and `job.cancel` (own →
    /// read, else system), `app.uninstall` (mcp: own or `dev.russet.test.*` IDs, else `perm.denied`).
    fn permission(_p: &Self::Params, _c: &ConnInfo, _o: &dyn Ownership) -> crate::Result<Permission> {
        Ok(Self::PERMISSION)
    }
}

/// A JSON-RPC notification method (ARCHITECTURE §9.1).
pub trait NotificationMethod: 'static {
    /// The wire name, for example `"job.updated"`.
    const NAME: &'static str;
    type Params: DeserializeOwned + Serialize + schemars::JsonSchema + Send + Sync + 'static;
}

/// Permission classes (ARCHITECTURE §7.5). Wire values are kebab-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    None,
    Read,
    Device,
    Build,
    Project,
    Account,
    AccountDestructive,
    Publish,
    Secrets,
    System,
}

/// How a method is confirmed for `cli` and `ide` connections (ARCHITECTURE §7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum Confirm {
    /// A plain `ui.confirm` when the class needs one; `--yes` pre-confirms.
    Plain,
    /// A typed confirmation (every `account-destructive` method and `ship.review.submit`).
    Typed,
}

/// The connection a call arrived on, as the permission engine sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnInfo {
    /// Connection id, for example `"c-7"`.
    pub id: String,
    pub kind: ClientKind,
    /// The client's name (`client.name`; for MCP, the `--client` id).
    pub name: String,
    /// True only when a CLI's stdin is a TTY (ARCHITECTURE §7.4).
    pub interactive: bool,
    /// A shared device reservation presented in `initialize.params.sharedLease` (ARCHITECTURE §6.3).
    pub shared_lease: Option<LeaseId>,
}

/// Client kinds (`initialize.params.client.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum ClientKind {
    Cli,
    Ide,
    Mcp,
    FlutterDevice,
    Test,
}

/// Ownership queries for per-call permissions; implemented by russetd over its JobManager and LeaseTable.
pub trait Ownership: Send + Sync {
    fn owns_job(&self, conn: &str, job: &JobId) -> bool;
    fn owns_lease(&self, conn: &str, lease: &LeaseId) -> bool;
    /// Bundle IDs of the projects this connection works on (for `app.uninstall` from MCP).
    fn project_bundle_ids(&self, conn: &str) -> Vec<String>;
}
