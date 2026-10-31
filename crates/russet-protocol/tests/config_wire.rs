// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config.*` wire shapes (ARCHITECTURE §8.1) and the per-call class of `config.set`/`config.unset` [D 66].

use russet_protocol::config::{
    ConfigFileKind, ConfigGetParams, ConfigGetResult, ConfigReadScope, ConfigSetParams, ConfigUnsetParams,
    ConfigWriteScope,
};
use russet_protocol::{ClientKind, ConnInfo, JobId, LeaseId, Ownership, Permission, registry};
use serde_json::json;

struct NoOwnership;

impl Ownership for NoOwnership {
    fn owns_job(&self, _conn: &str, _job: &JobId) -> bool {
        false
    }
    fn owns_lease(&self, _conn: &str, _lease: &LeaseId) -> bool {
        false
    }
    fn project_bundle_ids(&self, _conn: &str) -> Vec<String> {
        Vec::new()
    }
}

fn conn(kind: ClientKind) -> ConnInfo {
    ConnInfo { id: "c-1".into(), kind, name: "t".into(), interactive: false, shared_lease: None }
}

#[test]
fn writes_are_project_class_for_project_files_and_system_for_global() {
    let info = |name: &str| registry().iter().find(|m| m.name == name).copied().expect(name);
    for name in ["config.set", "config.unset"] {
        let m = info(name);
        assert!(m.dynamic_permission);
        assert_eq!(m.permission, Permission::System, "the ceiling");
        let class = |scope: &str| {
            (m.permission_of)(
                &json!({"scope": scope, "key": "run.device", "value": "x"}),
                &conn(ClientKind::Mcp),
                &NoOwnership,
            )
            .map_err(|e| e.code())
        };
        assert_eq!(class("project"), Ok(Permission::Project), "{name}");
        assert_eq!(class("local"), Ok(Permission::Project), "{name}");
        assert_eq!(class("global"), Ok(Permission::System), "{name}");
        assert_eq!(class("effective"), Err(russet_core::error::codes::core::INVALID_ARGUMENT), "{name}");
    }
    assert_eq!(info("config.get").permission, Permission::Read);
    assert_eq!(info("config.schema").permission, Permission::Read);
}

#[test]
fn shapes_are_camel_case_with_optional_fields_omitted() {
    let p: ConfigGetParams = serde_json::from_value(json!({})).expect("defaults");
    assert_eq!(p.scope, ConfigReadScope::Effective);
    assert_eq!(serde_json::to_value(&p).expect("ser"), json!({"scope": "effective"}));
    let set: ConfigSetParams =
        serde_json::from_value(json!({"scope": "local", "project": "/p", "key": "app.bundle-id", "value": null}))
            .expect("set");
    assert_eq!((set.scope, set.value), (ConfigWriteScope::Local, None));
    let unset: ConfigUnsetParams = serde_json::from_value(json!({"scope": "global", "key": "k"})).expect("unset");
    assert_eq!(unset.project, None);
    let r = ConfigGetResult { value: Some(json!(2)), ..Default::default() };
    assert_eq!(serde_json::to_value(&r).expect("ser"), json!({"value": 2, "sources": [], "files": []}));
    let kinds: Vec<ConfigFileKind> =
        serde_json::from_value(json!(["project", "global", "accounts", "mcp-policy", "provision-policy"]))
            .expect("kinds");
    assert_eq!(kinds.len(), 5);
}
