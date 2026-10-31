// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The registration hooks of the daemon (russetd's crate docs) and the home lock of in-process daemons:
//! - `ctx::provide` installs a service's constructor without touching the daemon's files; slots nobody
//!   provides keep their F0 constructors;
//! - `daemon::settings::from_config` and `mcp_policy` feed `config.toml`'s daemon settings and
//!   `mcp-policy.toml` into a daemon whose options come from the environment, except what flags pinned;
//! - `perms::project_bundle_ids` answers `Ownership::project_bundle_ids` from the connection's workspace roots;
//! - an in-process daemon of a home whose daemon runs keeps its jobs in memory and never marks the other
//!   daemon's jobs `interrupted` (it recovers only while it holds `runtime/russetd.lock`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use russet_core::async_trait;
use russet_core::error::Result;
use russet_core::rpc::ClientKind;
use russet_ipc::Router;
use russet_protocol::job::{JobInfo, JobState};
use russet_secrets::{BackendInfo, BackendKind, SecretBytes, SecretId, SecretKind, SecretMeta, SecretStore};
use russetd::daemon::settings::{self, DaemonSettings};
use russetd::jobs::JobSpec;
use russetd::{DaemonCtx, JobLimits, McpPolicy};
use serde_json::json;
use support::{Home, client, start};

/// A secret store that holds nothing.
struct EmptyStore;

#[async_trait]
impl SecretStore for EmptyStore {
    async fn put(&self, _id: &SecretId, _label: &str, _v: SecretBytes) -> Result<SecretMeta> {
        Err(russet_core::error::Error::not_implemented("an empty test store"))
    }
    async fn get(&self, _id: &SecretId) -> Result<Option<SecretBytes>> {
        Ok(None)
    }
    async fn delete(&self, _id: &SecretId) -> Result<bool> {
        Ok(false)
    }
    async fn list(&self, _kind: Option<SecretKind>) -> Result<Vec<SecretMeta>> {
        Ok(Vec::new())
    }
    fn backend(&self) -> BackendInfo {
        BackendInfo { kind: BackendKind::Memory, available: true, locked: false, reason: Some("test".into()) }
    }
}

#[tokio::test]
async fn provided_factories_construct_their_slots_and_the_rest_keep_f0() {
    let home = Home::new("hooks-services");
    let mut o = home.options();
    o.registrations.push(Arc::new(|r: &mut Router<DaemonCtx>| {
        russetd::ctx::provide::<dyn SecretStore>(r, |env| {
            assert!(env.dirs.config.ends_with("config"), "the factory sees the daemon's directories");
            Ok(Arc::new(EmptyStore))
        });
    }));
    let (daemon, _cli) = start(o).await;
    let cx = daemon.ctx();
    assert!(cx.secrets.try_get().is_none(), "nothing is constructed before its first use");
    let store = cx.secrets.get().expect("the provided store");
    assert_eq!(store.backend().reason.as_deref(), Some("test"));
    // Slots nobody provided keep their default constructor (W3's toolchain service).
    cx.toolchains.get().expect("the default toolchain service");
    // W3's ConfigService loads from the daemon's own directories.
    cx.config.get().expect("the configuration loads");
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn configuration_hooks_set_the_daemon_unless_flags_pinned_it() {
    let settings = DaemonSettings {
        idle_timeout: Some(Duration::from_secs(90)),
        max_parallel_jobs: Some(2),
        max_build_jobs: Some(1),
        log_filter: Some("debug".into()),
        log_retention_days: Some(3),
        web_listen: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
    };
    let mut policy = McpPolicy::default();
    policy.clients.insert("claude-code".into(), 2);
    let hooks = {
        let (settings, policy) = (settings.clone(), policy.clone());
        Arc::new(move |r: &mut Router<DaemonCtx>| {
            let s = settings.clone();
            settings::from_config(r, move |_cx| Ok(s.clone()));
            let p = policy.clone();
            settings::mcp_policy(r, move |cx| {
                assert!(cx.dirs.config.ends_with("config"));
                Ok(p.clone())
            });
        })
    };

    // A daemon whose options come from the environment reads the configuration.
    let home = Home::new("hooks-settings");
    let mut o = home.options();
    o.read_config = true;
    o.limits = JobLimits { max_parallel: 4, max_heavy: 1 };
    o.registrations.push(hooks.clone());
    let (daemon, _cli) = start(o).await;
    let cx = daemon.ctx();
    assert_eq!(cx.daemon.idle_timeout(), Some(Duration::from_secs(90)));
    assert_eq!(cx.jobs.limits(), JobLimits { max_parallel: 2, max_heavy: 1 });
    assert_eq!(cx.daemon.logs.filter(), "debug");
    assert_eq!(cx.daemon.logs.retention_days(), 3);
    assert_eq!(cx.perms.policy().tier_max("claude-code"), 2);
    let (_, init) = daemon.connect(&client(ClientKind::Mcp, "claude-code", false)).await.expect("agent");
    assert_eq!(init.grant.tier_max, Some(2), "the loaded policy decides the grant");
    // SIGHUP and configuration changes run the same reload.
    let again = settings::reload(cx);
    assert_eq!(again.settings, Some(settings.clone()));
    assert!(again.mcp_policy);
    daemon.stop().await.expect("stop");

    // --idle-timeout and RUSSET_LOG pin their values.
    let home = Home::new("hooks-pinned");
    let mut o = home.options();
    o.read_config = true;
    o.idle_timeout = Some(Duration::from_secs(5));
    o.idle_timeout_pinned = true;
    o.log.filter = Some("warn".into());
    o.registrations.push(hooks.clone());
    let (daemon, _cli) = start(o).await;
    assert_eq!(daemon.ctx().daemon.idle_timeout(), Some(Duration::from_secs(5)));
    assert_eq!(daemon.ctx().daemon.logs.filter(), "warn");
    assert_eq!(daemon.ctx().jobs.limits().max_parallel, 2, "unpinned values still apply");
    daemon.stop().await.expect("stop");

    // Options built with DaemonOptions::new (tests, embedders) read nothing.
    let home = Home::new("hooks-off");
    let mut o = home.options();
    o.registrations.push(hooks);
    let (daemon, _cli) = start(o).await;
    assert_eq!(daemon.ctx().jobs.limits(), JobLimits::default());
    assert_eq!(daemon.ctx().perms.policy(), McpPolicy::default());
    assert_eq!(settings::reload(daemon.ctx()), settings::Reloaded::default());
    daemon.stop().await.expect("stop");
}

#[tokio::test]
async fn a_connections_bundle_ids_come_from_its_workspace_roots() {
    let home = Home::new("hooks-bundle-ids");
    let mut o = home.options();
    o.registrations.push(Arc::new(|r: &mut Router<DaemonCtx>| {
        russetd::perms::project_bundle_ids(r, |root| {
            let name = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            vec![format!("dev.example.{name}"), format!("dev.example.{name}.widget")]
        });
    }));
    let (daemon, _cli) = start(o).await;
    let mut agent = client(ClientKind::Mcp, "claude-code", false);
    agent.workspace_roots = vec!["/w/Probe".into(), "/w/Probe".into()];
    let (peer, init) = daemon.connect(&agent).await.expect("agent");
    let (_, other) = daemon.connect(&client(ClientKind::Mcp, "codex", false)).await.expect("other");
    assert_eq!(
        daemon.ownership().project_bundle_ids(&init.connection_id),
        vec!["dev.example.probe".to_owned(), "dev.example.probe.widget".to_owned()]
    );
    assert!(daemon.ownership().project_bundle_ids(&other.connection_id).is_empty(), "no roots, no bundle IDs");
    peer.close();
    daemon.stop().await.expect("stop");
}

fn index_state(home: &Home, id: &str) -> Option<JobState> {
    let text = std::fs::read_to_string(home.dirs.jobs().join("index.jsonl")).ok()?;
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<JobInfo>(l).ok())
        .find(|j| j.job_id.as_str() == id)
        .map(|j| j.state)
}

#[tokio::test]
async fn an_in_process_daemon_beside_the_homes_daemon_keeps_its_jobs_in_memory() {
    let home = Home::new("hooks-home-lock");
    let (first, _cli) = start(home.options()).await;
    assert!(first.ctx().daemon.owns_home && first.ctx().jobs.persistent());
    let running = first
        .ctx()
        .jobs
        .submit(JobSpec::new("test", "runs until stopped"), |job| async move {
            job.cancel.cancelled().await;
            Ok(json!("stopped"))
        })
        .expect("submit")
        .id;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(index_state(&home, running.as_str()), Some(JobState::Running));

    // A second daemon of the same home (`--no-daemon` while the daemon runs) owns nothing.
    let (second, _) = start(home.options()).await;
    assert!(!second.ctx().daemon.owns_home && !second.ctx().jobs.persistent());
    assert!(second.ctx().jobs.get(&running).is_err(), "it does not load the other daemon's jobs");
    let own = second
        .ctx()
        .jobs
        .submit(JobSpec::new("test", "in memory"), |_job| async move { Ok(json!(1)) })
        .expect("submit")
        .id;
    second.ctx().jobs.wait(&own, Some(Duration::from_secs(5))).await.unwrap();
    assert!(!home.dirs.jobs().join(own.as_str()).exists(), "nothing of it reaches state/jobs");
    assert_eq!(first.ctx().jobs.get(&running).unwrap().state, JobState::Running);
    assert_eq!(index_state(&home, running.as_str()), Some(JobState::Running), "not marked interrupted");
    second.stop().await.expect("stop");
    first.stop().await.expect("stop");

    // Once the first has stopped, the home's lock is free again.
    let (third, _) = start(home.options()).await;
    assert!(third.ctx().daemon.owns_home);
    // The job returned when the first daemon stopped; the third sees how it ended, not `interrupted`.
    assert_eq!(third.ctx().jobs.get(&running).unwrap().state, JobState::Succeeded);
    third.stop().await.expect("stop");
}
