// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The permission engine: classes per connection kind, MCP tiers and the confirmation and approval rules
//! (ARCHITECTURE §7.5, §11.3) [D 6, D 66, D 85, D 98].
//!
//! Every call's class is resolved per call through `MethodInfo.permission_of` (the ceiling for static rows,
//! `Method::permission` for the dynamic ones), then:
//!
//! | Class | `cli` | `ide` | `mcp` (tier) | `flutter-device` |
//! |---|---|---|---|---|
//! | `none`, `read` | allow | allow | allow (0) | allow |
//! | `device`, `build`, `project` | allow | allow | allow if `tier-max ≥ 1` (1) | allow (`project`: deny) |
//! | `account` | allow | allow | approval if `tier-max ≥ 2` (2) | deny |
//! | `account-destructive` | typed confirm | typed confirm | never | deny |
//! | `publish` | confirm | confirm | approval if `tier-max = 3` (3) | deny |
//! | `secrets` | allow with a TTY; the three local-file imports with a plain confirm otherwise [D 85] | allow | never | deny |
//! | `system` | allow | allow | never | deny |
//!
//! `approval.grant` and `approval.deny` are also limited to `ide` and interactive `cli` connections. Device-plane
//! methods first pass the daemon's live-backend guard (3019, ARCHITECTURE §3.3 item 7). Profiles are
//! guardrails, not a sandbox (ARCHITECTURE §7.5).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use russet_core::error::{Error, codes};
use russet_core::ids::{JobId, LeaseId};
use russet_core::rpc::{ClientKind, Confirm, ConnInfo, Ownership, Permission};
use russet_ipc::{Authorizer, ConfirmSpec, Decision, Grant, PeerIdentity, TransportKind};
use russet_protocol::MethodInfo;
use russet_protocol::audit::AuditKind;
use russet_protocol::common::ClientRef;
use russet_protocol::initialize::InitializeParams;
use serde_json::Value;

use crate::approvals::ApprovalService;
use crate::audit::{AuditLog, audited};
use crate::ctx::DeviceSlot;
use crate::jobs::JobManager;
use crate::leases::LeaseTable;

/// Namespaces of the device plane (ARCHITECTURE §8.2), guarded by [`DeviceSlot::check`].
pub const DEVICE_NAMESPACES: &[&str] =
    &["device", "app", "process", "forward", "logs", "screen", "input", "files", "perf", "location"];

/// A rate limit: `calls` per `per` (a token bucket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub calls: u32,
    pub per: Duration,
}

/// The MCP policy (`mcp-policy.toml`, ARCHITECTURE §14.6). Loading the file is W13's; until then the daemon
/// uses these defaults, or what its options say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPolicy {
    /// `default-tier-max` (1 [D 60]).
    pub default_tier_max: u8,
    /// `[clients.<id>] tier-max`.
    pub clients: BTreeMap<String, u8>,
    /// `session-approval-ttl` for HTTP clients (30 min).
    pub session_approval_ttl: Duration,
    /// `[tools.<tool>] rate-limit`, by method.
    pub rate_limits: BTreeMap<String, RateLimit>,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            default_tier_max: 1,
            clients: BTreeMap::new(),
            session_approval_ttl: Duration::from_secs(30 * 60),
            rate_limits: BTreeMap::from([(
                "ship.upload".to_owned(),
                RateLimit { calls: 1, per: Duration::from_secs(10 * 60) },
            )]),
        }
    }
}

impl McpPolicy {
    /// The `tier-max` of MCP client `name`.
    pub fn tier_max(&self, name: &str) -> u8 {
        self.clients.get(name).copied().unwrap_or(self.default_tier_max).min(3)
    }
}

/// A typed confirmation's expected text for a method (W12, W23 register theirs).
pub type TypedText = fn(&Value) -> Option<String>;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Decides per connection and per call (`DaemonCtx.perms`).
#[derive(Debug)]
pub struct PermissionEngine {
    policy: RwLock<McpPolicy>,
    buckets: Mutex<HashMap<(String, String), Bucket>>,
    typed: RwLock<BTreeMap<String, TypedText>>,
}

impl Default for PermissionEngine {
    fn default() -> Self {
        Self::new(McpPolicy::default())
    }
}

/// The outcome of one evaluation, with what the audit log needs.
#[derive(Debug)]
pub struct Evaluation {
    pub decision: Decision,
    /// The call's class, when it could be resolved.
    pub class: Option<Permission>,
    /// The MCP tier, for MCP connections.
    pub tier: Option<u8>,
}

fn denied(msg: String, hint: &str) -> Decision {
    Decision::Deny(Error::new(codes::perm::DENIED, msg).with_hint(hint.to_owned()))
}

/// The wire name of a class (`account-destructive`, …).
pub fn class_name(p: Permission) -> &'static str {
    match p {
        Permission::None => "none",
        Permission::Read => "read",
        Permission::Device => "device",
        Permission::Build => "build",
        Permission::Project => "project",
        Permission::Account => "account",
        Permission::AccountDestructive => "account-destructive",
        Permission::Publish => "publish",
        Permission::Secrets => "secrets",
        Permission::System => "system",
    }
}

/// The MCP tier of a class (ARCHITECTURE §11.3); `None` for classes never exposed to agents.
pub fn tier_of(class: Permission) -> Option<u8> {
    match class {
        Permission::None | Permission::Read => Some(0),
        Permission::Device | Permission::Build | Permission::Project => Some(1),
        Permission::Account => Some(2),
        Permission::Publish => Some(3),
        Permission::AccountDestructive | Permission::Secrets | Permission::System => None,
    }
}

/// The three local-file imports a CLI without a TTY may run after a plain confirmation [D 85].
fn is_local_file_import(method: &str, params: &Value) -> bool {
    match method {
        "account.importXtool" | "account.addAscKey" => true,
        "secrets.set" => params.get("fromFile").is_some_and(|v| !v.is_null()),
        _ => false,
    }
}

impl PermissionEngine {
    /// An engine with `policy`.
    pub fn new(policy: McpPolicy) -> Self {
        Self { policy: RwLock::new(policy), buckets: Mutex::new(HashMap::new()), typed: RwLock::new(BTreeMap::new()) }
    }

    /// The MCP policy.
    pub fn policy(&self) -> McpPolicy {
        self.policy.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Replaces the MCP policy (a reloaded `mcp-policy.toml`).
    pub fn set_policy(&self, p: McpPolicy) {
        *self.policy.write().unwrap_or_else(std::sync::PoisonError::into_inner) = p;
    }

    /// Registers the expected text of `method`'s typed confirmation (default: the method name).
    pub fn set_typed_text(&self, method: &str, f: TypedText) {
        self.typed.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(method.to_owned(), f);
    }

    fn typed_text(&self, method: &str, params: &Value) -> String {
        let f = self.typed.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(method).copied();
        f.and_then(|f| f(params)).unwrap_or_else(|| method.to_owned())
    }

    /// The grant of a new connection (ARCHITECTURE §7.4 `grant`). `none` is always allowed and not listed.
    pub fn grant_for(&self, kind: ClientKind, name: &str, interactive: bool) -> Grant {
        use Permission::{Account, AccountDestructive, Build, Device, Project, Publish, Read, Secrets, System};
        match kind {
            ClientKind::Cli => Grant {
                profile: "cli".into(),
                allow: if interactive {
                    vec![Read, Device, Build, Project, Account, Secrets, System]
                } else {
                    vec![Read, Device, Build, Project, Account, System]
                },
                confirm: if interactive {
                    vec![AccountDestructive, Publish]
                } else {
                    vec![AccountDestructive, Publish, Secrets]
                },
                approval: vec![],
                deny: vec![],
                tier_max: None,
            },
            ClientKind::Ide => Grant {
                profile: "ide".into(),
                allow: vec![Read, Device, Build, Project, Account, Secrets, System],
                confirm: vec![AccountDestructive, Publish],
                approval: vec![],
                deny: vec![],
                tier_max: None,
            },
            ClientKind::Mcp => {
                let t = self.policy().tier_max(name);
                let mut g = Grant {
                    profile: "mcp".into(),
                    allow: vec![Read],
                    confirm: vec![],
                    approval: vec![],
                    deny: vec![AccountDestructive, Secrets, System],
                    tier_max: Some(t),
                };
                for (class, tier) in [(Device, 1), (Build, 1), (Project, 1), (Account, 2), (Publish, 3)] {
                    match (tier <= t, tier >= 2) {
                        (true, false) => g.allow.push(class),
                        (true, true) => g.approval.push(class),
                        (false, _) => g.deny.push(class),
                    }
                }
                g
            }
            ClientKind::FlutterDevice => Grant {
                profile: "flutter-device".into(),
                allow: vec![Read, Device, Build],
                confirm: vec![],
                approval: vec![],
                deny: vec![Project, Account, AccountDestructive, Publish, Secrets, System],
                tier_max: None,
            },
            ClientKind::Test => Grant {
                profile: "test".into(),
                allow: vec![Read, Device, Build, Project, Account, AccountDestructive, Publish, Secrets, System],
                confirm: vec![],
                approval: vec![],
                deny: vec![],
                tier_max: None,
            },
        }
    }

    fn rate_limited(&self, client: &str, method: &str) -> Option<Error> {
        let limit = self.policy().rate_limits.get(method).copied()?;
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let b = buckets
            .entry((client.to_owned(), method.to_owned()))
            .or_insert(Bucket { tokens: f64::from(limit.calls), last: now });
        let refill = now.duration_since(b.last).as_secs_f64() / limit.per.as_secs_f64() * f64::from(limit.calls);
        b.tokens = (b.tokens + refill).min(f64::from(limit.calls));
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            None
        } else {
            Some(
                Error::new(
                    codes::perm::RATE_LIMITED,
                    format!("{method} is limited to {} per {:?}", limit.calls, limit.per),
                )
                .with_hint("Wait and retry; the limits are set in mcp-policy.toml.")
                .retryable(true),
            )
        }
    }

    /// The decision for one call (module docs).
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &self,
        grant: &Grant,
        info: &MethodInfo,
        params: &Value,
        conn: &ConnInfo,
        ownership: &dyn Ownership,
        approvals: &ApprovalService,
        devices: &DeviceSlot,
    ) -> Evaluation {
        let class = match (info.permission_of)(params, conn, ownership) {
            Ok(c) => c,
            Err(e) => return Evaluation { decision: Decision::Deny(e), class: None, tier: None },
        };
        let tier = (conn.kind == ClientKind::Mcp).then(|| tier_of(class)).flatten();
        let decide = |decision| Evaluation { decision, class: Some(class), tier };
        let namespace = info.name.split('.').next().unwrap_or_default();
        if DEVICE_NAMESPACES.contains(&namespace)
            && let Err(e) = devices.check()
        {
            return decide(Decision::Deny(e));
        }
        if class == Permission::None {
            return decide(Decision::Allow);
        }
        if matches!(info.name, "approval.grant" | "approval.deny")
            && !(conn.kind == ClientKind::Ide || (conn.kind == ClientKind::Cli && conn.interactive))
        {
            return decide(denied(
                format!("{} is for a human, from Russet IDE or an interactive terminal", info.name),
                "Run `russet approve <id>` in a terminal, or approve it in Russet IDE.",
            ));
        }
        if conn.kind == ClientKind::Mcp {
            let Some(t) = tier_of(class) else {
                return decide(denied(
                    format!("{} ({}) is never available to agents", info.name, class_name(class)),
                    "Account-destructive, secrets and system calls are for humans only (ARCHITECTURE §11.5).",
                ));
            };
            let tier_max = grant.tier_max.unwrap_or(0);
            if t > tier_max {
                return decide(Decision::Deny(
                    Error::new(
                        codes::perm::TIER_EXCEEDED,
                        format!("{} is a tier-{t} call; this client's tier-max is {tier_max}", info.name),
                    )
                    .with_hint("A human may raise the client's `tier-max` in mcp-policy.toml (ARCHITECTURE §11.3)."),
                ));
            }
            if let Some(e) = self.rate_limited(&conn.name, info.name) {
                return decide(Decision::Deny(e));
            }
            if t >= 2 {
                return decide(match approvals.check_call(conn, info.name, params, t) {
                    Ok(()) => Decision::Allow,
                    Err(e) => Decision::NeedsApproval(e),
                });
            }
            return decide(Decision::Allow);
        }
        if grant.deny.contains(&class) {
            return decide(denied(
                format!("the {} profile does not allow {} ({})", grant.profile, info.name, class_name(class)),
                "This client's permission profile does not allow the call (ARCHITECTURE §7.5).",
            ));
        }
        if class == Permission::Secrets && conn.kind == ClientKind::Cli && !conn.interactive {
            if !is_local_file_import(info.name, params) {
                return decide(denied(
                    format!("{} needs an interactive terminal", info.name),
                    "Run it in a terminal; only the local-file imports work with `--yes` (ARCHITECTURE §7.5).",
                ));
            }
            return decide(Decision::Confirm(self.confirm_spec(info, params, conn, class, false)));
        }
        if grant.confirm.contains(&class) {
            let typed = info.confirm == Confirm::Typed;
            return decide(Decision::Confirm(self.confirm_spec(info, params, conn, class, typed)));
        }
        if grant.allow.contains(&class) {
            return decide(Decision::Allow);
        }
        decide(denied(
            format!("the {} profile does not allow {} ({})", grant.profile, info.name, class_name(class)),
            "This client's permission profile does not allow the call (ARCHITECTURE §7.5).",
        ))
    }

    fn confirm_spec(
        &self,
        info: &MethodInfo,
        params: &Value,
        conn: &ConnInfo,
        class: Permission,
        typed: bool,
    ) -> ConfirmSpec {
        let danger = class == Permission::AccountDestructive;
        let typed = typed.then(|| self.typed_text(info.name, params));
        let message = match &typed {
            Some(t) => {
                format!("{} wants to run {} ({}). Type `{t}` to confirm.", conn.name, info.name, class_name(class))
            }
            None => format!("{} wants to run {} ({}).", conn.name, info.name, class_name(class)),
        };
        ConfirmSpec {
            title: format!("Confirm {}", info.name),
            message,
            detail: Some(format!("params: {}", crate::audit::redacted_params(params))),
            danger,
            typed,
        }
    }
}

/// Ownership queries over the daemon's jobs and leases.
#[derive(Debug, Clone)]
pub struct DaemonOwnership {
    pub jobs: JobManager,
    pub leases: LeaseTable,
}

impl Ownership for DaemonOwnership {
    fn owns_job(&self, conn: &str, job: &JobId) -> bool {
        self.jobs.owns(conn, job)
    }
    fn owns_lease(&self, conn: &str, lease: &LeaseId) -> bool {
        self.leases.owns(conn, lease)
    }
    fn project_bundle_ids(&self, _conn: &str) -> Vec<String> {
        // The project model is W5's; until a connection's project is known, no bundle id is "its own".
        Vec::new()
    }
}

/// The daemon's [`Authorizer`] over the [`PermissionEngine`], the approvals and the device guard.
#[derive(Debug, Clone)]
pub struct DaemonAuthorizer {
    pub engine: Arc<PermissionEngine>,
    pub approvals: ApprovalService,
    pub devices: DeviceSlot,
    pub ownership: DaemonOwnership,
    pub audit: AuditLog,
}

impl Authorizer for DaemonAuthorizer {
    fn on_connect(&self, id: &PeerIdentity) -> russet_core::Result<()> {
        #[cfg(unix)]
        if id.transport == TransportKind::Unix {
            let me = russet_core::platform::current_uid();
            if id.uid != Some(me) {
                return Err(Error::new(
                    codes::perm::AUTH_FAILED,
                    format!("uid {:?} is not this daemon's user", id.uid),
                ));
            }
        }
        let _ = id;
        Ok(())
    }

    fn on_initialize(&self, _id: &PeerIdentity, p: &InitializeParams) -> russet_core::Result<Grant> {
        let interactive = p.capabilities.interactive && p.client.kind == ClientKind::Cli;
        Ok(self.engine.grant_for(p.client.kind, &p.client.name, interactive))
    }

    fn decide(
        &self,
        grant: &Grant,
        method: &MethodInfo,
        params: &Value,
        conn: &ConnInfo,
        ownership: &dyn Ownership,
    ) -> Decision {
        let ev = self.engine.evaluate(grant, method, params, conn, ownership, &self.approvals, &self.devices);
        if let (Decision::Deny(e) | Decision::NeedsApproval(e), Some(class)) = (&ev.decision, ev.class)
            && audited(conn.kind, class)
        {
            let approval = e
                .details()
                .and_then(|d| d.get("approvalId"))
                .and_then(Value::as_str)
                .and_then(|id| russet_core::ids::ApprovalId::parse(id).ok());
            self.audit.record(&crate::audit::entry(
                AuditKind::Rpc,
                ClientRef { kind: conn.kind, name: conn.name.clone() },
                method.name,
                ev.tier,
                params,
                approval,
                e.code().as_str(),
                0,
            ));
        }
        ev.decision
    }

    fn ownership(&self) -> &dyn Ownership {
        &self.ownership
    }

    fn on_confirmation(&self, conn: &ConnInfo, method: &MethodInfo, params: &Value, confirmed: bool) {
        self.approvals.record_confirmation(conn, method.name, params, confirmed);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ctx::{LiveRefusal, LiveState};
    use crate::daemon::Connections;
    use crate::daemon::backend::DeviceBackend;

    fn conn(kind: ClientKind, interactive: bool) -> ConnInfo {
        ConnInfo { id: "c-1".into(), kind, name: "client".into(), interactive, shared_lease: None }
    }

    fn info(name: &str) -> MethodInfo {
        *russet_protocol::registry().iter().find(|m| m.name == name).unwrap_or_else(|| panic!("{name} is registered"))
    }

    struct World {
        engine: PermissionEngine,
        approvals: ApprovalService,
        devices: DeviceSlot,
    }

    impl World {
        fn new() -> Self {
            Self {
                engine: PermissionEngine::default(),
                approvals: ApprovalService::new(Connections::default(), AuditLog::disabled()),
                devices: DeviceSlot::new(DeviceBackend::None, LiveState::NotLive, "/nope".into()),
            }
        }

        fn decide(&self, kind: ClientKind, interactive: bool, method: &str, params: Value) -> Decision {
            let c = conn(kind, interactive);
            let grant = self.engine.grant_for(kind, &c.name, interactive);
            self.engine
                .evaluate(&grant, &info(method), &params, &c, &russet_ipc::NoOwnership, &self.approvals, &self.devices)
                .decision
        }
    }

    fn code(d: &Decision) -> Option<&'static str> {
        match d {
            Decision::Deny(e) | Decision::NeedsApproval(e) => Some(e.code().as_str()),
            _ => None,
        }
    }

    #[test]
    fn the_class_table_of_section_7_5_holds() {
        let w = World::new();
        use ClientKind::{Cli, FlutterDevice, Ide, Mcp};
        // cli and ide
        assert!(matches!(w.decide(Cli, true, "daemon.status", json!({})), Decision::Allow));
        assert!(matches!(w.decide(Cli, false, "daemon.shutdown", json!({})), Decision::Allow));
        assert!(matches!(
            w.decide(Ide, false, "ship.upload", json!({})),
            Decision::Confirm(ConfirmSpec { typed: None, .. })
        ));
        assert!(matches!(
            w.decide(Cli, true, "portal.certificates.revoke", json!({})),
            Decision::Confirm(ConfirmSpec { typed: Some(_), danger: true, .. })
        ));
        // secrets from a CLI without a TTY: only the local-file imports, with a plain confirmation [D 85]
        assert!(matches!(w.decide(Cli, true, "secrets.delete", json!({"id": "x"})), Decision::Allow));
        assert_eq!(code(&w.decide(Cli, false, "secrets.delete", json!({"id": "x"}))), Some("perm.denied"));
        assert!(matches!(
            w.decide(Cli, false, "account.importXtool", json!({})),
            Decision::Confirm(ConfirmSpec { typed: None, .. })
        ));
        assert!(matches!(
            w.decide(Cli, false, "secrets.set", json!({"id": "x", "fromFile": "/k.p8"})),
            Decision::Confirm(_)
        ));
        // approval.grant is for humans
        assert!(matches!(
            w.decide(Ide, false, "approval.grant", json!({"approvalId": "apr_01J00000000000000000000000"})),
            Decision::Allow
        ));
        assert_eq!(code(&w.decide(Cli, false, "approval.grant", json!({}))), Some("perm.denied"));
        assert_eq!(code(&w.decide(Mcp, false, "approval.grant", json!({}))), Some("perm.denied"));
        // mcp: tiers
        assert!(matches!(w.decide(Mcp, false, "job.list", json!({})), Decision::Allow));
        assert!(matches!(w.decide(Mcp, false, "build.start", json!({})), Decision::Allow));
        assert_eq!(code(&w.decide(Mcp, false, "ship.upload", json!({}))), Some("perm.tierExceeded"));
        assert_eq!(code(&w.decide(Mcp, false, "daemon.shutdown", json!({}))), Some("perm.denied"));
        assert_eq!(code(&w.decide(Mcp, false, "secrets.list", json!({}))), None, "secrets.list is read");
        assert_eq!(code(&w.decide(Mcp, false, "secrets.delete", json!({}))), Some("perm.denied"));
        // Dynamic rows resolve per call: job.cancel of someone else's job is `system`, never for agents.
        assert_eq!(
            code(&w.decide(Mcp, false, "job.cancel", json!({"jobId": "job_01J00000000000000000000000"}))),
            Some("perm.denied")
        );
        assert!(matches!(
            w.decide(Cli, false, "job.cancel", json!({"jobId": "job_01J00000000000000000000000"})),
            Decision::Allow
        ));
        // Params a dynamic row cannot read are core.invalidArgument (the server answers -32602).
        assert_eq!(code(&w.decide(Cli, false, "job.cancel", json!({"jobId": 7}))), Some("core.invalidArgument"));
        // flutter-device
        assert!(matches!(w.decide(FlutterDevice, false, "build.start", json!({})), Decision::Allow));
        assert_eq!(
            code(&w.decide(FlutterDevice, false, "config.set", json!({"scope": "project", "key": "x"}))),
            Some("perm.denied")
        );
    }

    #[test]
    fn mcp_tier_two_and_three_need_approvals() {
        let mut policy = McpPolicy::default();
        policy.clients.insert("client".into(), 3);
        policy.rate_limits.clear();
        let w = World { engine: PermissionEngine::new(policy), ..World::new() };
        let d = w.decide(ClientKind::Mcp, false, "ship.upload", json!({"ipa": "/a.ipa"}));
        assert_eq!(code(&d), Some("perm.approvalRequired"));
        let grant = w.engine.grant_for(ClientKind::Mcp, "client", false);
        assert_eq!(grant.tier_max, Some(3));
        assert!(grant.approval.contains(&Permission::Account) && grant.approval.contains(&Permission::Publish));
        assert!(grant.deny.contains(&Permission::System));
    }

    #[test]
    fn rate_limits_refuse_the_second_call() {
        let mut policy = McpPolicy::default();
        policy.clients.insert("client".into(), 1);
        policy.rate_limits.insert("build.start".into(), RateLimit { calls: 1, per: Duration::from_secs(600) });
        let w = World { engine: PermissionEngine::new(policy), ..World::new() };
        assert!(matches!(w.decide(ClientKind::Mcp, false, "build.start", json!({})), Decision::Allow));
        assert_eq!(code(&w.decide(ClientKind::Mcp, false, "build.start", json!({}))), Some("perm.rateLimited"));
    }

    #[test]
    fn device_methods_hit_the_live_guard_first() {
        let w = World {
            devices: DeviceSlot::new(
                DeviceBackend::Live,
                LiveState::Refused(LiveRefusal::DevelopmentBuild),
                "/nope".into(),
            ),
            ..World::new()
        };
        assert_eq!(code(&w.decide(ClientKind::Cli, true, "device.list", json!({}))), Some("device.liveBackendRefused"));
        assert_eq!(
            code(&w.decide(ClientKind::Cli, true, "app.install", json!({"path": "/a"}))),
            Some("device.liveBackendRefused")
        );
        assert!(matches!(w.decide(ClientKind::Cli, true, "daemon.status", json!({})), Decision::Allow));
    }
}
