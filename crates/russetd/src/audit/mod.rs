// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The append-only `state/audit.jsonl` (ARCHITECTURE §6.5 [D 43]): one line per entry,
//! `{ts, kind: rpc|approval|account-mutation, client:{kind,name}, method, tool?, tier?, paramsDigest,
//! paramsRedacted, approvalId?, outcome, durationMs}`. Every MCP call is recorded, and CLI and IDE calls from
//! class `account` upwards. Params are redacted before they are written (ARCHITECTURE §12.2).

use std::sync::Arc;

use russet_core::error::Result;
use russet_core::ids::ApprovalId;
use russet_core::rpc::{ClientKind, Permission};
use russet_protocol::audit::{AuditEntry, AuditKind, AuditTailParams};
use russet_protocol::common::ClientRef;
use sha2::Digest;

use crate::state::JsonLines;

/// The default number of entries `audit.tail` returns.
pub const DEFAULT_TAIL: usize = 50;

/// A cheap-to-clone handle (`DaemonCtx.audit`).
#[derive(Debug, Clone)]
pub struct AuditLog {
    file: Option<Arc<JsonLines>>,
}

impl AuditLog {
    /// The log at `path` (`state/audit.jsonl`).
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { file: Some(Arc::new(JsonLines::new(path))) }
    }

    /// A log that records nothing.
    pub fn disabled() -> Self {
        Self { file: None }
    }

    /// Appends one entry; a failing disk never fails the call it records (the error is logged).
    pub fn record(&self, entry: &AuditEntry) {
        if let Some(f) = &self.file
            && let Err(e) = f.append(entry)
        {
            tracing::warn!("cannot write the audit log: {}", e.message());
        }
    }

    /// The newest `limit` entries matching the filters, oldest first (`audit.tail`).
    pub fn tail(&self, p: &AuditTailParams) -> Result<Vec<AuditEntry>> {
        let Some(f) = &self.file else { return Ok(Vec::new()) };
        let limit = p.limit.map_or(DEFAULT_TAIL, |l| l as usize);
        let all: Vec<AuditEntry> = f.read_all()?;
        let matching: Vec<AuditEntry> = all
            .into_iter()
            .filter(|e| p.kinds.is_empty() || p.kinds.contains(&e.kind))
            .filter(|e| p.client.as_ref().is_none_or(|c| &e.client.name == c))
            .collect();
        let skip = matching.len().saturating_sub(limit);
        Ok(matching.into_iter().skip(skip).collect())
    }
}

/// Whether a call of `class` from a `kind` client is audited: every MCP call; CLI, IDE and the others from
/// class `account` upwards (ARCHITECTURE §6.5).
pub fn audited(kind: ClientKind, class: Permission) -> bool {
    kind == ClientKind::Mcp
        || matches!(
            class,
            Permission::Account
                | Permission::AccountDestructive
                | Permission::Publish
                | Permission::Secrets
                | Permission::System
        )
}

/// The params as JSON without `approval`, `progressToken`, `background` and `confirmText` (the call's
/// envelope, not its meaning), with sorted keys.
pub fn canonical_params(params: &serde_json::Value) -> serde_json::Value {
    fn sort(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let sorted: std::collections::BTreeMap<String, serde_json::Value> =
                    m.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    let mut v = sort(params);
    if let serde_json::Value::Object(m) = &mut v {
        for envelope in ["approval", "progressToken", "background", "confirmText"] {
            m.remove(envelope);
        }
    }
    v
}

/// SHA-256 (lowercase hex) of the canonical JSON params without `approval` (ARCHITECTURE §11.3).
pub fn params_digest(params: &serde_json::Value) -> String {
    let canonical = canonical_json(&canonical_params(params));
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compact JSON with object keys in sorted order, whatever map type serde_json was built with.
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::Value::String(k.clone()), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// The params with secrets masked, for `paramsRedacted`.
pub fn redacted_params(params: &serde_json::Value) -> serde_json::Value {
    let mut v = canonical_params(params);
    russet_core::redact::redact_json(&mut v);
    v
}

/// Builds an entry for one call.
#[allow(clippy::too_many_arguments)]
pub fn entry(
    kind: AuditKind,
    client: ClientRef,
    method: &str,
    tier: Option<u8>,
    params: &serde_json::Value,
    approval_id: Option<ApprovalId>,
    outcome: impl Into<String>,
    duration_ms: u64,
) -> AuditEntry {
    let redact = |s: &str| russet_core::redact::redact_text(s).into_owned();
    AuditEntry {
        ts: russet_core::time::now_rfc3339(),
        kind,
        client: ClientRef { kind: client.kind, name: redact(&client.name) },
        method: method.to_owned(),
        tool: params.get("_meta").and_then(|m| m.get("tool")).and_then(serde_json::Value::as_str).map(redact),
        tier,
        params_digest: params_digest(params),
        params_redacted: redacted_params(params),
        approval_id,
        outcome: outcome.into(),
        duration_ms,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn digests_ignore_key_order_and_the_envelope() {
        let a = json!({"b": 1, "a": {"y": 2, "x": [1, 2]}, "approval": "apr_x", "progressToken": "p-1"});
        let b = json!({"a": {"x": [1, 2], "y": 2}, "b": 1});
        assert_eq!(params_digest(&a), params_digest(&b));
        assert_ne!(params_digest(&b), params_digest(&json!({"a": {"x": [2, 1], "y": 2}, "b": 1})));
        assert_eq!(params_digest(&json!({})).len(), 64);
    }

    #[test]
    fn entries_are_redacted_and_tailed() {
        let tmp = russet_core::fsx::TempDir::new("audit").expect("tempdir");
        let log = AuditLog::new(tmp.path().join("state/audit.jsonl"));
        let client = ClientRef { kind: ClientKind::Mcp, name: "claude-code".into() };
        for (i, outcome) in ["ok", "perm.denied", "ok"].iter().enumerate() {
            let params = json!({"index": i, "token": "s3cr3t-canary"});
            log.record(&entry(AuditKind::Rpc, client.clone(), "ship.upload", Some(3), &params, None, *outcome, 5));
        }
        let text = std::fs::read_to_string(tmp.path().join("state/audit.jsonl")).expect("read");
        assert!(!text.contains("s3cr3t-canary"), "{text}");
        let tail = log.tail(&AuditTailParams { limit: Some(2), ..AuditTailParams::default() }).expect("tail");
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].outcome, "perm.denied");
        assert_eq!(tail[1].params_redacted["index"], 2);
        let none =
            log.tail(&AuditTailParams { client: Some("codex".into()), ..AuditTailParams::default() }).expect("tail");
        assert!(none.is_empty());
        assert!(audited(ClientKind::Mcp, Permission::Read));
        assert!(!audited(ClientKind::Cli, Permission::Build));
        assert!(audited(ClientKind::Ide, Permission::Account));
    }
}
