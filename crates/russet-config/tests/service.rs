// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ConfigService` end to end over a temporary `RUSSET_HOME`: layering, `config.get/set/unset`, invalid files,
//! narrowing and `config.changed`.

use std::path::Path;
use std::time::Duration;

use russet_config::{ConfigScope, ConfigService, EnvOverrides, ReadScope, SourceScope};
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use serde_json::{Value, json};

fn service(home: &Path) -> ConfigService {
    let dirs = RussetDirs::under(home);
    std::fs::create_dir_all(&dirs.config).expect("config dir");
    ConfigService::with_env(&dirs, EnvOverrides::default()).expect("service")
}

#[test]
fn get_set_unset_round_trip_through_the_service() {
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    std::fs::write(project.path().join("russet.toml"), "# committed\n[app]\nbundle-id = \"com.example.hello\"\n")
        .expect("russet.toml");
    let svc = service(home.path());

    // Defaults come from the built-in layer.
    let l = svc.lookup(ReadScope::Effective, Some(project.path()), Some("daemon.max-build-jobs")).expect("get");
    assert_eq!(l.value, Some(json!(1)));
    assert_eq!(l.sources[0].scope, SourceScope::Default);

    // Global and local edits; local wins over the committed file.
    let out = svc.set(ConfigScope::Global, None, "daemon.max-build-jobs", json!("2")).expect("set global");
    assert!(out.changed);
    assert_eq!(out.path, home.path().join("config").join("config.toml"));
    let out = svc.set(ConfigScope::Local, Some(project.path()), "app.bundle-id", json!("com.me.hello")).expect("local");
    assert!(out.changed);
    let again =
        svc.set(ConfigScope::Local, Some(project.path()), "app.bundle-id", json!("com.me.hello")).expect("same");
    assert!(!again.changed, "setting the same value changes nothing");

    let l = svc.lookup(ReadScope::Effective, Some(project.path()), Some("app.bundle-id")).expect("get");
    assert_eq!(l.value, Some(json!("com.me.hello")));
    assert_eq!(l.sources.iter().map(|s| s.scope).collect::<Vec<_>>(), [SourceScope::Local, SourceScope::Project]);
    let l = svc.lookup(ReadScope::Effective, None, Some("daemon.max-build-jobs")).expect("get");
    assert_eq!((l.value, l.sources[0].scope), (Some(json!(2)), SourceScope::Global));

    // File scopes read the file as written.
    let l = svc.lookup(ReadScope::Project, Some(project.path()), Some("app.bundle-id")).expect("project scope");
    assert_eq!(l.value, Some(json!("com.example.hello")));
    let l = svc.lookup(ReadScope::Global, None, Some("daemon.idle-timeout")).expect("unset in file");
    assert_eq!(l.value, None);
    assert!(l.sources.is_empty());
    assert_eq!(l.files.len(), 1);
    assert!(l.files[0].exists);

    // Unset falls back to the committed value; the committed file kept its comment.
    let out = svc.unset(ConfigScope::Local, Some(project.path()), "app.bundle-id").expect("unset");
    assert!(out.changed);
    let l = svc.lookup(ReadScope::Effective, Some(project.path()), Some("app.bundle-id")).expect("get");
    assert_eq!(l.value, Some(json!("com.example.hello")));
    let committed = std::fs::read_to_string(project.path().join("russet.toml")).expect("read");
    assert!(committed.starts_with("# committed\n"));

    // Unknown keys are refused in every scope.
    let err = svc.lookup(ReadScope::Effective, None, Some("daemon.nope")).expect_err("unknown");
    assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
    let err = svc.lookup(ReadScope::Global, None, Some("app.bundle-id")).expect_err("project key in global");
    assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
    let err = svc.set(ConfigScope::Global, None, "app.bundle-id", json!("x")).expect_err("project key in global");
    assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
    let err = svc.set(ConfigScope::Project, None, "app.bundle-id", json!("x")).expect_err("no project");
    assert_eq!(err.code(), codes::config::PROJECT_NOT_FOUND);
}

#[test]
fn an_invalid_global_file_is_reported_with_its_position_but_never_stops_the_service() {
    let home = tempfile::tempdir().expect("home");
    let dirs = RussetDirs::under(home.path());
    std::fs::create_dir_all(&dirs.config).expect("config dir");
    std::fs::write(dirs.config.join("config.toml"), "schema = 1\n[daemon]\nidle-timeout = \"never\"\n").expect("write");
    let svc = ConfigService::with_env(&dirs, EnvOverrides::default()).expect("loads anyway");
    assert_eq!(svc.global().daemon.max_build_jobs, 1, "defaults while the file is invalid");
    let err = svc.effective(None).expect_err("invalid");
    assert_eq!(err.code(), codes::config::INVALID);
    assert!(err.message().contains("config.toml:3:16:"), "{}", err.message());
    // Fixing the file on disk is picked up without a restart.
    std::fs::write(dirs.config.join("config.toml"), "schema = 1\n[daemon]\nidle-timeout = \"1h\"\n").expect("fix");
    let eff = svc.effective(None).expect("valid now");
    assert_eq!(eff.global.daemon.idle_timeout.get(), Duration::from_secs(3600));
}

#[test]
fn a_local_file_cannot_widen_the_committed_agents_section() {
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    let svc = service(home.path());
    std::fs::write(home.path().join("config").join("mcp-policy.toml"), "default-tier-max = 2\n").expect("policy");
    std::fs::write(project.path().join("russet.toml"), "[agents]\ntier-max = 1\n").expect("committed");
    std::fs::write(project.path().join("russet.local.toml"), "[agents]\ntier-max = 3\n").expect("local");
    let grant = svc.agents("claude-code", Some(project.path())).expect("grant");
    assert_eq!(grant.tier_max, 1);
    let grant = svc.agents("claude-code", None).expect("no project");
    assert_eq!(grant.tier_max, 2);
    std::fs::write(home.path().join("config").join("mcp-policy.toml"), "default-tier-max = 9\n").expect("bad");
    assert_eq!(svc.agents("x", None).map_err(|e| e.code()).err(), Some(codes::config::INVALID));
}

#[tokio::test(flavor = "current_thread")]
async fn edits_send_config_changed() {
    let home = tempfile::tempdir().expect("home");
    let svc = service(home.path());
    let mut rx = svc.subscribe();
    svc.set(ConfigScope::Global, None, "fallback.pymobiledevice3", Value::from("off")).expect("set");
    let change = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("in time").expect("change");
    assert_eq!(change.scope, ConfigScope::Global);
    assert!(change.path.ends_with("config.toml"));
}

#[tokio::test(flavor = "current_thread")]
async fn on_disk_edits_send_config_changed_while_watching() {
    let home = tempfile::tempdir().expect("home");
    let svc = service(home.path());
    if !svc.watching() {
        return; // no platform watcher here (inotify limits); edits through Russet still notify
    }
    let mut rx = svc.subscribe();
    std::fs::write(home.path().join("config").join("config.toml"), "[web]\nlisten = \"127.0.0.1:0\"\n").expect("write");
    let change = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("in time").expect("change");
    assert_eq!(change.scope, ConfigScope::Global);
}
