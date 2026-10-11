// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config.*` handlers over a ConfigService in a temporary home (no daemon needed).

use russet_config::{ConfigService, EnvOverrides};
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_protocol::config as p;
use russetd::handlers::config as h;
use serde_json::json;

/// A temporary directory removed on drop (russetd has no tempfile dev-dependency).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("russet-w3-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn service(home: &std::path::Path) -> ConfigService {
    ConfigService::with_env(&RussetDirs::under(home), EnvOverrides::default()).expect("service")
}

#[test]
fn get_set_unset_and_schema() {
    let home = TempDir::new();
    let project = TempDir::new();
    let svc = service(home.path());
    let root = project.path().to_str().expect("utf-8").to_owned();

    let set = p::ConfigSetParams {
        scope: p::ConfigWriteScope::Local,
        project: Some(root.clone()),
        key: "run.device".into(),
        value: Some(json!("iphone12")),
    };
    let out = h::set(&svc, set).expect("set");
    assert!(out.changed && out.path.ends_with("russet.local.toml"));

    let get = p::ConfigGetParams {
        scope: p::ConfigReadScope::Effective,
        project: Some(root.clone()),
        key: Some("run.device".into()),
    };
    let r = h::get(&svc, get).expect("get");
    assert_eq!(r.value, Some(json!("iphone12")));
    assert_eq!(r.sources[0].scope, p::ConfigSourceScope::Local);
    assert!(r.files.iter().any(|f| f.scope == p::ConfigWriteScope::Global && !f.exists));

    // A null value unsets, like config.unset.
    let null = p::ConfigSetParams {
        scope: p::ConfigWriteScope::Local,
        project: Some(root.clone()),
        key: "run.device".into(),
        value: None,
    };
    assert!(h::set(&svc, null).expect("unset via set").changed);
    let unset = p::ConfigUnsetParams {
        scope: p::ConfigWriteScope::Local,
        project: Some(root.clone()),
        key: "run.device".into(),
    };
    assert!(!h::unset(&svc, unset).expect("unset again").changed);

    let schema = h::schema(&svc, p::ConfigSchemaParams { file: p::ConfigFileKind::McpPolicy });
    assert_eq!(schema.schema["title"], "mcp-policy.toml");
}

#[test]
fn project_paths_must_be_absolute_existing_directories() {
    let home = TempDir::new();
    let svc = service(home.path());
    let rel =
        p::ConfigGetParams { scope: p::ConfigReadScope::Project, project: Some("relative/dir".into()), key: None };
    assert_eq!(h::get(&svc, rel).map_err(|e| e.code()).err(), Some(codes::core::INVALID_ARGUMENT));
    let gone = p::ConfigGetParams {
        scope: p::ConfigReadScope::Project,
        project: Some("/nonexistent/russet".into()),
        key: None,
    };
    assert_eq!(h::get(&svc, gone).map_err(|e| e.code()).err(), Some(codes::config::PROJECT_NOT_FOUND));
    let no_project = p::ConfigGetParams { scope: p::ConfigReadScope::Local, project: None, key: None };
    assert_eq!(h::get(&svc, no_project).map_err(|e| e.code()).err(), Some(codes::config::PROJECT_NOT_FOUND));
}

#[test]
fn changes_map_to_the_notification() {
    let c = russet_config::ConfigChanged { scope: russet_config::ConfigScope::Project, path: "/p/russet.toml".into() };
    let n = h::changed_params(&c);
    assert_eq!(serde_json::to_value(&n).expect("ser"), json!({"scope": "project", "path": "/p/russet.toml"}));
}
