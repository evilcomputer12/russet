// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `initialize` handshake (ARCHITECTURE §7.4). Owner: W2.
//!
//! The first message of every connection (anything else gets `-32002`). The client sends its protocol
//! version, kind, name and capabilities; the daemon answers with its own identity (build id and executable
//! identity for the development skew checks of ARCHITECTURE §3.3), the connection id, the connection's grant
//! (ARCHITECTURE §7.5) and the method and notification lists.

use russet_core::ids::LeaseId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ClientKind, MethodInfo, NotificationInfo, Permission, ProtocolVersion, RpcMethod};

/// Params of `initialize` (ARCHITECTURE §7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol: ProtocolVersion,
    pub client: ClientDescriptor,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace: Option<Workspace>,
    /// TCP connections only (ARCHITECTURE §7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auth: Option<AuthParams>,
    /// A device reservation this connection shares (ARCHITECTURE §6.3; `RUSSET_LEASE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shared_lease: Option<LeaseId>,
}

/// `initialize.params.client`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientDescriptor {
    pub kind: ClientKind,
    /// For MCP bridges, the `--client` id (ARCHITECTURE §11.2).
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub pid: Option<u32>,
}

/// What the client can do when the daemon asks (ARCHITECTURE §7.4, §9.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Answers `ui.confirm`.
    #[serde(default)]
    pub confirm: bool,
    /// Answers `ui.prompt`.
    #[serde(default)]
    pub prompt: bool,
    /// Answers `ui.openUrl`.
    #[serde(default)]
    pub open_url: bool,
    /// Wants `approval.requested` and `approval.resolved`.
    #[serde(default)]
    pub approvals: bool,
    /// Serves the IDE reverse requests of ARCHITECTURE §9.4.
    #[serde(default)]
    pub ide_debug: bool,
    /// True only when a CLI's stdin is a TTY; approval grants require it (ARCHITECTURE §7.4, §11.3).
    #[serde(default)]
    pub interactive: bool,
}

/// `initialize.params.workspace`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Absolute project roots.
    #[serde(default)]
    pub roots: Vec<String>,
}

/// `initialize.params.auth` (TCP only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthParams {
    pub token: AuthToken,
}

/// A secret token on the wire: serialized as its string, never printed by `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(type = "string")]
pub struct AuthToken(pub String);

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthToken(***)")
    }
}

/// Result of `initialize` (ARCHITECTURE §7.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol: ProtocolVersion,
    pub server: ServerInfo,
    /// `c-<n>`.
    pub connection_id: String,
    pub grant: Grant,
    pub capabilities: ServerCapabilities,
    pub endpoints: ServerEndpoints,
}

/// The daemon's identity (`initialize.result.server`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// `russetd`.
    pub name: String,
    pub version: String,
    /// `<version>+g<sha>[-dirty]`.
    pub build_id: String,
    pub pid: u32,
    /// The executable the daemon was started from.
    pub executable: String,
    /// Recorded once at daemon start (ARCHITECTURE §3.3 item 3).
    pub executable_id: ExecutableId,
    /// RFC 3339 UTC.
    pub started_at: String,
    /// `linux`, `windows`, `macos`.
    pub platform: String,
    /// `x86_64`, `aarch64`.
    pub arch: String,
    /// `live`, `scripted:<file>`, `replay:<trace>`, `mock:<addr>` or `none` (ARCHITECTURE §3.3, §14.9).
    pub device_backend: String,
}

/// `{dev, ino, mtimeNs, size}` of the daemon's executable (Windows: volume serial and file index).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableId {
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: i64,
    pub size: u64,
}

impl From<russet_core::platform::ExecutableId> for ExecutableId {
    fn from(id: russet_core::platform::ExecutableId) -> Self {
        Self { dev: id.dev, ino: id.ino, mtime_ns: i64::try_from(id.mtime_ns).unwrap_or(i64::MAX), size: id.size }
    }
}

/// A connection's permissions (ARCHITECTURE §7.4 `grant`, §7.5): which classes run, which need a
/// confirmation, which need an out-of-band approval, and which are denied. `none` is always allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    /// `cli`, `ide`, `mcp`, `flutter-device` or `test`.
    pub profile: String,
    #[serde(default)]
    pub allow: Vec<Permission>,
    #[serde(default)]
    pub confirm: Vec<Permission>,
    #[serde(default)]
    pub approval: Vec<Permission>,
    #[serde(default)]
    pub deny: Vec<Permission>,
    /// MCP connections only (ARCHITECTURE §11.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tier_max: Option<u8>,
}

/// What the daemon serves (`initialize.result.capabilities`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub methods: Vec<String>,
    pub notifications: Vec<String>,
    pub features: ServerFeatures,
}

/// Optional daemon features.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerFeatures {
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub wifi_devices: bool,
    #[serde(default)]
    pub mcp_http: bool,
}

/// Side-channel endpoints (`initialize.result.endpoints`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ServerEndpoints {
    /// The web endpoint (`http://127.0.0.1:<port>`), once it has started (ARCHITECTURE §6.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web: Option<String>,
}

/// `details` of `ipc.protocolMismatch` (1100).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolMismatchDetails {
    pub server_protocol: ProtocolVersion,
    pub client_protocol: ProtocolVersion,
    /// The daemon is idle and shuts down so the client can start its own build (ARCHITECTURE §3.3 item 3).
    pub restartable: bool,
}

/// `details` of `ipc.buildMismatch` (1107; development builds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct BuildMismatchDetails {
    pub server_build_id: String,
    pub client_build_id: String,
    pub server_executable: String,
    /// The daemon was idle and accepted `daemon.shutdown {reason: "upgrade"}`.
    pub restartable: bool,
}

/// `details` of the `ipc.daemonUnavailable` (1102) that a stopping daemon answers to `initialize`: it keeps its
/// endpoint until its jobs have had their grace (core §6.3), so a client waits for it to go and then starts a
/// new daemon instead of racing the old one for `runtime/russetd.lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonUnavailableDetails {
    /// The daemon is shutting down.
    pub shutting_down: bool,
    /// How long the daemon still gives its jobs, at most (`0` when forced).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub grace_ms: Option<u64>,
}

crate::rpc_method!(
    /// `initialize` (class `none`). W2 (Wave 1).
    Initialize,
    "initialize",
    InitializeParams => InitializeResult,
    Permission::None
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[Initialize::INFO];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_handshake_of_section_7_4_parses() {
        let params: InitializeParams = serde_json::from_value(json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"kind": "ide", "name": "russet-vscode", "version": "0.2.0", "pid": 5150},
            "capabilities": {"confirm": true, "prompt": true, "openUrl": true, "approvals": true, "ideDebug": true,
                             "interactive": false},
            "workspace": {"roots": ["/home/u/RussetProbe"]},
            "sharedLease": "lse_01J00000000000000000000000",
            "somethingNewer": 1
        }))
        .expect("parses");
        assert_eq!(params.client.kind, ClientKind::Ide);
        assert!(params.capabilities.open_url && params.capabilities.ide_debug);
        assert!(params.shared_lease.is_some());
        // A minimal client.
        let min: InitializeParams = serde_json::from_value(
            json!({"protocol": {"major": 1, "minor": 0}, "client": {"kind": "cli", "name": "russet"}}),
        )
        .expect("parses");
        assert!(!min.capabilities.confirm && min.auth.is_none());
    }

    #[test]
    fn tokens_never_print() {
        let auth = AuthParams { token: AuthToken("s3cr3t".into()) };
        assert!(!format!("{auth:?}").contains("s3cr3t"));
        assert_eq!(serde_json::to_value(&auth).expect("serializes"), json!({"token": "s3cr3t"}));
    }

    #[test]
    fn grants_serialize_like_the_example() {
        let g = Grant {
            profile: "ide".into(),
            allow: vec![Permission::Read, Permission::Device],
            confirm: vec![Permission::AccountDestructive, Permission::Publish],
            approval: vec![],
            deny: vec![],
            tier_max: None,
        };
        assert_eq!(
            serde_json::to_value(&g).expect("serializes"),
            json!({"profile": "ide", "allow": ["read", "device"], "confirm": ["account-destructive", "publish"],
                   "approval": [], "deny": []})
        );
    }
}
