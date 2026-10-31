// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Out-of-band approvals (ARCHITECTURE §6.5, §11.3 [D 5, D 67]).
//!
//! A tier-2 or tier-3 call from an MCP client without a valid approval creates a pending [`Approval`] and fails
//! with `perm.approvalRequired` (1204, its `details` say how to approve). `approval.requested` goes to `ide`
//! connections and `cli` connections with a TTY; a human grants or denies it there; the agent retries with
//! `approval: "apr_…"`. The retry is checked against the binding:
//! - the same client identity (kind and name; a `session` grant is bound to the requesting connection, which
//!   is the MCP session for stdio bridges [D 67]) and the same method;
//! - for tier 3, the same `paramsDigest` (SHA-256 of the canonical params without `approval`);
//! - not expired: pending approvals expire 10 min after creation, unused `once` grants 10 min after the grant;
//! - a `once` approval is used once.
//!
//! Any mismatch is `perm.approvalInvalid` (1205). A tier-2 `session` grant also covers later calls of the same
//! method on that connection without an `approval` param. Confirmations of `cli` and `ide` calls are recorded
//! as approvals too, granted and used by the requester (ARCHITECTURE §7.5).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use russet_core::error::{Error, Result, codes};
use russet_core::ids::ApprovalId;
use russet_core::rpc::ConnInfo;
use russet_protocol::NotificationMethod;
use russet_protocol::approval::{
    Approval, ApprovalRequested, ApprovalRequestedParams, ApprovalRequiredDetails, ApprovalResolved,
    ApprovalResolvedParams, ApprovalScope, ApprovalState, ApprovalSubject,
};
use russet_protocol::audit::AuditKind;
use russet_protocol::common::ConnectionRef;
use tokio::sync::Notify;

use crate::audit::{AuditLog, params_digest};
use crate::daemon::Connections;

/// How long a pending approval waits for a human (ARCHITECTURE §11.3), and how long an unused `once` grant
/// stays valid.
pub const APPROVAL_TTL: Duration = Duration::from_secs(10 * 60);

/// Final approvals kept for `approval.list`.
const KEEP_FINAL: usize = 200;

#[derive(Debug)]
struct Record {
    approval: Approval,
    expires: SystemTime,
}

#[derive(Debug, Default)]
struct Inner {
    records: Mutex<BTreeMap<ApprovalId, Record>>,
    changed: Notify,
}

/// A cheap-to-clone handle (`DaemonCtx.approvals`).
#[derive(Debug, Clone)]
pub struct ApprovalService {
    inner: Arc<Inner>,
    connections: Connections,
    audit: AuditLog,
}

fn connection_ref(conn: &ConnInfo) -> ConnectionRef {
    ConnectionRef { kind: conn.kind, name: conn.name.clone(), connection_id: conn.id.clone() }
}

fn invalid(why: impl Into<String>) -> Error {
    Error::new(codes::perm::APPROVAL_INVALID, why)
        .with_hint("Request a new approval: retry the call without `approval`.")
}

impl ApprovalService {
    /// Notifications go to `connections`; decisions are written to `audit`.
    pub fn new(connections: Connections, audit: AuditLog) -> Self {
        Self { inner: Arc::new(Inner::default()), connections, audit }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<ApprovalId, Record>> {
        self.inner.records.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Expires overdue approvals and trims old final ones; returns the ids that changed state.
    fn sweep(map: &mut BTreeMap<ApprovalId, Record>, now: SystemTime) -> Vec<(ApprovalId, ApprovalState)> {
        let mut changed = Vec::new();
        for (id, r) in map.iter_mut() {
            let live = matches!(r.approval.state, ApprovalState::Pending)
                || (r.approval.state == ApprovalState::Granted && r.approval.scope == Some(ApprovalScope::Once));
            if live && now >= r.expires {
                r.approval.state = ApprovalState::Expired;
                changed.push((id.clone(), ApprovalState::Expired));
            }
        }
        let finals: Vec<ApprovalId> =
            map.iter().filter(|(_, r)| r.approval.state.is_final()).map(|(id, _)| id.clone()).collect();
        if finals.len() > KEEP_FINAL {
            for id in finals.iter().take(finals.len() - KEEP_FINAL) {
                map.remove(id);
            }
        }
        changed
    }

    fn announce_resolved(&self, approval: &Approval) {
        let params = ApprovalResolvedParams { approval_id: approval.approval_id.clone(), state: approval.state };
        if let Ok(v) = serde_json::to_value(params) {
            let requester = approval.requested_by.connection_id.clone();
            self.connections
                .notify_where(ApprovalResolved::NAME, &v, |c| Connections::is_approver(c) || c.summary.id == requester);
        }
    }

    fn audit_approval(&self, approval: &Approval, outcome: &str) {
        let (method, params) = match &approval.subject {
            ApprovalSubject::Method { method, params_digest } => {
                (method.clone(), serde_json::json!({"paramsDigest": params_digest}))
            }
            ApprovalSubject::Plan { plan_id, plan_digest } => {
                ("provision.apply".to_owned(), serde_json::json!({"planId": plan_id, "planDigest": plan_digest}))
            }
        };
        let entry = crate::audit::entry(
            AuditKind::Approval,
            approval.requested_by.client(),
            &method,
            Some(approval.tier),
            &params,
            Some(approval.approval_id.clone()),
            outcome,
            0,
        );
        self.audit.record(&entry);
    }

    /// Creates a pending approval for `subject` and announces it (`approval.requested`).
    pub fn request(
        &self,
        conn: &ConnInfo,
        subject: ApprovalSubject,
        tier: u8,
        summary: String,
        detail: Option<String>,
    ) -> Approval {
        let now = SystemTime::now();
        let approval = Approval {
            approval_id: ApprovalId::generate(),
            state: ApprovalState::Pending,
            requested_by: connection_ref(conn),
            subject,
            tier,
            summary,
            detail,
            scope: None,
            created_at: russet_core::time::rfc3339(now),
            expires_at: russet_core::time::rfc3339(now + APPROVAL_TTL),
            granted_by: None,
            reason: None,
        };
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        map.insert(approval.approval_id.clone(), Record { approval: approval.clone(), expires: now + APPROVAL_TTL });
        drop(map);
        if let Ok(v) = serde_json::to_value(ApprovalRequestedParams { approval: approval.clone() }) {
            self.connections.notify_where(
                <ApprovalRequested as NotificationMethod>::NAME,
                &v,
                Connections::is_approver,
            );
        }
        self.audit_approval(&approval, "pending");
        self.inner.changed.notify_waiters();
        approval
    }

    /// The `perm.approvalRequired` error for a pending approval (ARCHITECTURE §11.3).
    pub fn required_error(approval: &Approval) -> Error {
        let ApprovalSubject::Method { params_digest, .. } = &approval.subject else {
            return Error::new(codes::perm::APPROVAL_REQUIRED, approval.summary.clone());
        };
        let details = ApprovalRequiredDetails {
            approval_id: approval.approval_id.clone(),
            tool: None,
            tier: approval.tier,
            summary: approval.summary.clone(),
            args_digest: params_digest.clone(),
            approve_with: format!("russet approve {} (or Approve in Russet IDE)", approval.approval_id),
            expires_at: approval.expires_at.clone(),
        };
        Error::new(codes::perm::APPROVAL_REQUIRED, format!("{} needs a human approval", approval.summary))
            .with_hint(format!(
                "Ask the user to run `russet approve {}` in a terminal or to approve it in Russet IDE, then retry \
                 with approval: \"{}\".",
                approval.approval_id, approval.approval_id
            ))
            .with_details(serde_json::to_value(details).unwrap_or_default())
    }

    /// Checks a tier-2/3 call from `conn` (module docs): `Ok` lets it run; `Err` is `perm.approvalRequired`
    /// (a new or still pending approval) or `perm.approvalInvalid`.
    pub fn check_call(&self, conn: &ConnInfo, method: &str, params: &serde_json::Value, tier: u8) -> Result<()> {
        let digest = params_digest(params);
        let now = SystemTime::now();
        let presented = params.get("approval").and_then(serde_json::Value::as_str);
        let mut map = self.lock();
        let expired = Self::sweep(&mut map, now);
        let outcome = match presented {
            Some(id) => match ApprovalId::parse(id) {
                Err(_) => Err(Some(invalid(format!("`{id}` is not an approval id")))),
                Ok(id) => match map.get_mut(&id) {
                    None => Err(Some(invalid(format!("approval {id} does not exist")))),
                    Some(r) => Self::use_approval(r, conn, method, &digest, tier),
                },
            },
            None => {
                let session = map.values().any(|r| {
                    r.approval.state == ApprovalState::Granted
                        && r.approval.scope == Some(ApprovalScope::Session)
                        && r.approval.requested_by.connection_id == conn.id
                        && matches!(&r.approval.subject, ApprovalSubject::Method { method: m, .. } if m == method)
                });
                if session { Ok(None) } else { Err(None) }
            }
        };
        drop(map);
        for (id, state) in expired {
            if let Ok(a) = self.get(&id) {
                debug_assert_eq!(a.state, state);
                self.announce_resolved(&a);
            }
        }
        match outcome {
            Ok(Some(used)) => {
                if used.state == ApprovalState::Used {
                    self.announce_resolved(&used);
                }
                self.audit_approval(&used, "used");
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(Some(e)) => Err(e),
            Err(None) => {
                let summary = format!("{} from {} ({})", method, conn.name, conn.id);
                let approval = self.request(
                    conn,
                    ApprovalSubject::Method { method: method.to_owned(), params_digest: digest },
                    tier,
                    summary,
                    Some(format!("params: {}", crate::audit::redacted_params(params))),
                );
                Err(Self::required_error(&approval))
            }
        }
    }

    /// Validates a presented approval against the call and consumes it if `once`.
    #[allow(clippy::result_large_err)]
    fn use_approval(
        r: &mut Record,
        conn: &ConnInfo,
        method: &str,
        digest: &str,
        tier: u8,
    ) -> std::result::Result<Option<Approval>, Option<Error>> {
        let a = &mut r.approval;
        let ApprovalSubject::Method { method: approved_method, params_digest } = &a.subject else {
            return Err(Some(invalid("this approval is for a provisioning plan, not a call")));
        };
        match a.state {
            ApprovalState::Pending => return Err(Some(Self::required_error(a))),
            ApprovalState::Denied => return Err(Some(invalid(format!("approval {} was denied", a.approval_id)))),
            ApprovalState::Expired => return Err(Some(invalid(format!("approval {} has expired", a.approval_id)))),
            ApprovalState::Used => return Err(Some(invalid(format!("approval {} was already used", a.approval_id)))),
            ApprovalState::Granted => {}
        }
        if a.requested_by.kind != conn.kind || a.requested_by.name != conn.name {
            return Err(Some(invalid(format!("approval {} belongs to another client", a.approval_id))));
        }
        if approved_method != method {
            return Err(Some(invalid(format!("approval {} is for {approved_method}, not {method}", a.approval_id))));
        }
        if a.scope == Some(ApprovalScope::Session) && a.requested_by.connection_id != conn.id {
            return Err(Some(invalid(format!("approval {} belongs to another session", a.approval_id))));
        }
        if (tier >= 3 || a.scope != Some(ApprovalScope::Session)) && params_digest != digest {
            return Err(Some(invalid(format!("approval {} was granted for other arguments", a.approval_id))));
        }
        if a.scope != Some(ApprovalScope::Session) {
            a.state = ApprovalState::Used;
        }
        Ok(Some(a.clone()))
    }

    /// Grants a pending approval (`approval.grant`). Tier 3 is always `once`; tier 2 defaults to `session`.
    pub fn grant(&self, id: &ApprovalId, scope: Option<ApprovalScope>, by: &ConnInfo) -> Result<Approval> {
        let now = SystemTime::now();
        let mut map = self.lock();
        Self::sweep(&mut map, now);
        let r = map.get_mut(id).ok_or_else(|| not_found(id))?;
        if r.approval.state != ApprovalState::Pending {
            return Err(invalid(format!("approval {id} is {}, not pending", state_name(r.approval.state))));
        }
        let scope = match (r.approval.tier, scope) {
            (t, Some(ApprovalScope::Session)) if t >= 3 => {
                return Err(Error::invalid_argument("tier-3 approvals are always for one call (scope once)"));
            }
            (_, Some(s)) => s,
            (2, None) => ApprovalScope::Session,
            (_, None) => ApprovalScope::Once,
        };
        r.approval.state = ApprovalState::Granted;
        r.approval.scope = Some(scope);
        r.approval.granted_by = Some(connection_ref(by));
        r.expires = now + APPROVAL_TTL;
        let approval = r.approval.clone();
        drop(map);
        self.announce_resolved(&approval);
        self.audit_approval(&approval, "granted");
        self.inner.changed.notify_waiters();
        Ok(approval)
    }

    /// Denies a pending approval (`approval.deny`).
    pub fn deny(&self, id: &ApprovalId, reason: Option<String>, by: &ConnInfo) -> Result<Approval> {
        let mut map = self.lock();
        Self::sweep(&mut map, SystemTime::now());
        let r = map.get_mut(id).ok_or_else(|| not_found(id))?;
        if r.approval.state != ApprovalState::Pending {
            return Err(invalid(format!("approval {id} is {}, not pending", state_name(r.approval.state))));
        }
        r.approval.state = ApprovalState::Denied;
        r.approval.reason = reason;
        r.approval.granted_by = Some(connection_ref(by));
        let approval = r.approval.clone();
        drop(map);
        self.announce_resolved(&approval);
        self.audit_approval(&approval, "denied");
        self.inner.changed.notify_waiters();
        Ok(approval)
    }

    /// Records a confirmation of a `cli` or `ide` call as an approval granted (and used) by the requester.
    pub fn record_confirmation(&self, conn: &ConnInfo, method: &str, params: &serde_json::Value, confirmed: bool) {
        let now = SystemTime::now();
        let me = connection_ref(conn);
        let approval = Approval {
            approval_id: ApprovalId::generate(),
            state: if confirmed { ApprovalState::Used } else { ApprovalState::Denied },
            requested_by: me.clone(),
            subject: ApprovalSubject::Method { method: method.to_owned(), params_digest: params_digest(params) },
            tier: 0,
            summary: format!("confirmation of {method}"),
            detail: None,
            scope: Some(ApprovalScope::Once),
            created_at: russet_core::time::rfc3339(now),
            expires_at: russet_core::time::rfc3339(now),
            granted_by: Some(me),
            reason: (!confirmed).then(|| "declined by the requester".to_owned()),
        };
        self.audit_approval(&approval, if confirmed { "confirmed" } else { "declined" });
        let mut map = self.lock();
        map.insert(approval.approval_id.clone(), Record { approval, expires: now });
        Self::sweep(&mut map, now);
    }

    /// Every approval, or those in `state`, oldest first.
    pub fn list(&self, state: Option<ApprovalState>) -> Vec<Approval> {
        let mut map = self.lock();
        Self::sweep(&mut map, SystemTime::now());
        map.values().map(|r| r.approval.clone()).filter(|a| state.is_none_or(|s| a.state == s)).collect()
    }

    /// One approval.
    pub fn get(&self, id: &ApprovalId) -> Result<Approval> {
        let mut map = self.lock();
        Self::sweep(&mut map, SystemTime::now());
        map.get(id).map(|r| r.approval.clone()).ok_or_else(|| not_found(id))
    }

    /// The approval, after waiting up to `wait` while it is pending (`approval.get {waitMs}`).
    pub async fn wait(&self, id: &ApprovalId, wait: Duration) -> Result<Approval> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let changed = self.inner.changed.notified();
            let a = self.get(id)?;
            if a.state != ApprovalState::Pending {
                return Ok(a);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return self.get(id);
            }
        }
    }

    /// `session` grants die with their connection [D 67].
    pub fn connection_closed(&self, conn_id: &str) {
        let mut map = self.lock();
        for r in map.values_mut() {
            if r.approval.scope == Some(ApprovalScope::Session)
                && r.approval.state == ApprovalState::Granted
                && r.approval.requested_by.connection_id == conn_id
            {
                r.approval.state = ApprovalState::Expired;
            }
        }
    }
}

/// The wire name of a state (`pending`, `granted`, …).
pub fn state_name(s: ApprovalState) -> &'static str {
    match s {
        ApprovalState::Pending => "pending",
        ApprovalState::Granted => "granted",
        ApprovalState::Denied => "denied",
        ApprovalState::Expired => "expired",
        ApprovalState::Used => "used",
    }
}

fn not_found(id: &ApprovalId) -> Error {
    Error::new(codes::core::NOT_FOUND, format!("approval {id} does not exist"))
        .with_hint("List the approvals with `russet approvals list`.")
}

#[cfg(test)]
mod tests {
    use russet_core::rpc::ClientKind;
    use serde_json::json;

    use super::*;

    fn conn(id: &str, kind: ClientKind, name: &str) -> ConnInfo {
        ConnInfo { id: id.into(), kind, name: name.into(), interactive: true, shared_lease: None }
    }

    fn approval_id(e: &Error) -> ApprovalId {
        let id = e.details().and_then(|d| d.get("approvalId")).and_then(|v| v.as_str()).expect("approvalId");
        ApprovalId::parse(id).expect("valid")
    }

    #[test]
    fn tier_three_needs_a_fresh_approval_for_the_same_arguments() {
        let svc = ApprovalService::new(Connections::default(), AuditLog::disabled());
        let agent = conn("c-7", ClientKind::Mcp, "claude-code");
        let human = conn("c-1", ClientKind::Ide, "russet-ide");
        let params = json!({"ipa": "/p/App.ipa"});
        let e = svc.check_call(&agent, "ship.upload", &params, 3).expect_err("needs approval");
        assert_eq!(e.code(), codes::perm::APPROVAL_REQUIRED);
        let id = approval_id(&e);
        assert!(e.details().and_then(|d| d.get("approveWith")).is_some());
        // Retrying before the grant still needs it (same approval).
        let mut retry = params.clone();
        retry["approval"] = json!(id.as_str());
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &retry, 3).expect_err("pending").code(),
            codes::perm::APPROVAL_REQUIRED
        );
        let granted = svc.grant(&id, Some(ApprovalScope::Session), &human);
        assert!(granted.is_err(), "tier 3 is never session-scoped");
        svc.grant(&id, None, &human).expect("granted once");
        // Mismatched arguments.
        let mut other = json!({"ipa": "/p/Other.ipa"});
        other["approval"] = json!(id.as_str());
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &other, 3).expect_err("mismatch").code(),
            codes::perm::APPROVAL_INVALID
        );
        // Mismatched method and client.
        assert_eq!(
            svc.check_call(&agent, "ship.other", &retry, 3).expect_err("method").code(),
            codes::perm::APPROVAL_INVALID
        );
        let codex = conn("c-9", ClientKind::Mcp, "codex");
        assert_eq!(
            svc.check_call(&codex, "ship.upload", &retry, 3).expect_err("client").code(),
            codes::perm::APPROVAL_INVALID
        );
        // The right call, once.
        svc.check_call(&agent, "ship.upload", &retry, 3).expect("approved");
        assert_eq!(svc.get(&id).expect("exists").state, ApprovalState::Used);
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &retry, 3).expect_err("reused").code(),
            codes::perm::APPROVAL_INVALID
        );
    }

    #[test]
    fn tier_two_session_grants_cover_the_connection_until_it_closes() {
        let svc = ApprovalService::new(Connections::default(), AuditLog::disabled());
        let agent = conn("c-7", ClientKind::Mcp, "claude-code");
        let e =
            svc.check_call(&agent, "portal.devices.register", &json!({"udid": "a"}), 2).expect_err("needs approval");
        let id = approval_id(&e);
        let a = svc.grant(&id, None, &conn("c-1", ClientKind::Cli, "russet")).expect("granted");
        assert_eq!(a.scope, Some(ApprovalScope::Session));
        // Later calls of the method on the connection need no approval param, whatever the arguments.
        svc.check_call(&agent, "portal.devices.register", &json!({"udid": "b"}), 2).expect("session");
        // Another connection of the same client is another session.
        let other = conn("c-8", ClientKind::Mcp, "claude-code");
        assert!(svc.check_call(&other, "portal.devices.register", &json!({"udid": "b"}), 2).is_err());
        svc.connection_closed("c-7");
        assert!(svc.check_call(&agent, "portal.devices.register", &json!({"udid": "c"}), 2).is_err());
    }

    #[test]
    fn expired_and_denied_approvals_are_invalid() {
        let svc = ApprovalService::new(Connections::default(), AuditLog::disabled());
        let agent = conn("c-7", ClientKind::Mcp, "claude-code");
        let e = svc.check_call(&agent, "ship.upload", &json!({}), 3).expect_err("needs approval");
        let id = approval_id(&e);
        // Expire it by moving its deadline into the past.
        svc.lock().get_mut(&id).expect("record").expires = SystemTime::now() - Duration::from_secs(1);
        let retry = json!({"approval": id.as_str()});
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &retry, 3).expect_err("expired").code(),
            codes::perm::APPROVAL_INVALID
        );
        assert_eq!(svc.get(&id).expect("kept").state, ApprovalState::Expired);
        assert!(
            svc.grant(&id, None, &conn("c-1", ClientKind::Ide, "ide")).is_err(),
            "an expired approval cannot be granted"
        );

        let e = svc.check_call(&agent, "ship.upload", &json!({}), 3).expect_err("needs approval");
        let id = approval_id(&e);
        svc.deny(&id, Some("no".into()), &conn("c-1", ClientKind::Ide, "ide")).expect("denied");
        let retry = json!({"approval": id.as_str()});
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &retry, 3).expect_err("denied").code(),
            codes::perm::APPROVAL_INVALID
        );
        assert_eq!(
            svc.check_call(&agent, "ship.upload", &json!({"approval": "nope"}), 3).expect_err("bad id").code(),
            codes::perm::APPROVAL_INVALID
        );
    }
}
