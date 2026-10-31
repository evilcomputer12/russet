// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Long-running calls from the CLI (ARCHITECTURE §7.6): the helper area commands use for `build`, `install`,
//! `run` and `debug` prints the job's log lines once and in order, routes `$/progress` to the reporter, turns
//! an interrupt into `$/cancelRequest` (`core.cancelled`, exit 130, the job cancelled) and passes
//! `background: true` through.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_cli::Reporter;
use russet_cli::render::job::call_long;
use russet_core::Error;
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::fsx::TempDir;
use russet_core::progress::{LogStream, ProgressSink};
use russet_core::rpc::ClientKind;
use russet_ipc::{ClientInfo, ProgressValue, Router};
use russet_protocol::common::ProgressKind;
use russet_protocol::job::{JobListParams, JobState};
use russetd::{DaemonCtx, DaemonOptions, InProcess};
use serde_json::{Value, json};

russet_protocol::rpc_method!(
    /// A test build: logs `lines` lines (a short pause between them), then waits for cancellation when
    /// `hang` is set, else answers.
    TestLongBuild,
    "test.longBuild",
    Value => Value,
    Permission::Build,
    long_running
);

#[derive(Default)]
struct Recorder {
    logs: Mutex<Vec<String>>,
    begins: Mutex<Vec<ProgressValue>>,
}

impl Reporter for Recorder {
    fn progress(&self, p: &ProgressValue) {
        if p.kind == ProgressKind::Begin {
            self.begins.lock().unwrap().push(p.clone());
        }
    }
    fn result(&self, _v: &Value) {}
    fn human(&self, _text: &str) {}
    fn item(&self, _v: &Value, _human: &str) {}
    fn log(&self, _stream: LogStream, line: &str) {
        self.logs.lock().unwrap().push(line.to_owned());
    }
    fn error(&self, _e: &Error) {}
    fn is_json(&self) -> bool {
        false
    }
}

async fn daemon(home: &TempDir) -> (InProcess, russet_ipc::Peer) {
    let mut o = DaemonOptions::new(RussetDirs::under(home.path()));
    o.live_device_lock = home.path().join("live-device.lock");
    o.home = Some(home.path().to_path_buf());
    o.registrations.push(Arc::new(|r: &mut Router<DaemonCtx>| {
        r.request::<TestLongBuild, _, _>(|_cx, rq: russet_ipc::RequestCtx, p: Value| async move {
            for i in 0..p["lines"].as_u64().unwrap_or(0) {
                rq.progress.log(LogStream::Stdout, &format!("line {i}"));
                if i % 10 == 0 {
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
            }
            if p["hang"].as_bool() == Some(true) {
                rq.cancel.cancelled().await;
                return Err(Error::cancelled());
            }
            Ok(json!({"built": true}))
        });
    }));
    let mut client = ClientInfo::new(ClientKind::Cli, "test-cli");
    client.capabilities.interactive = true;
    client.capabilities.confirm = true;
    let (d, peer, _) = InProcess::start_with(o, &client).await.expect("daemon");
    (d, peer)
}

fn never() -> std::future::Pending<()> {
    std::future::pending()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn log_lines_arrive_once_and_in_order_with_the_result() {
    let home = TempDir::new("cli-long").expect("home");
    let (d, peer) = daemon(&home).await;
    let rec = Arc::new(Recorder::default());
    let out =
        call_long::<TestLongBuild, _, _>(&peer, rec.clone(), json!({"lines": 120}), true, never).await.expect("built");
    assert_eq!(out, json!({"built": true}));
    let logs = rec.logs.lock().unwrap().clone();
    let want: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    assert_eq!(logs, want);
    let begins = rec.begins.lock().unwrap().clone();
    assert!(begins.iter().any(|b| b.job_id.is_some()), "the begin names the job: {begins:?}");

    // Without logs nothing is printed; background returns {jobId} at once.
    let quiet = Arc::new(Recorder::default());
    call_long::<TestLongBuild, _, _>(&peer, quiet.clone(), json!({"lines": 5}), false, never).await.expect("built");
    assert!(quiet.logs.lock().unwrap().is_empty());
    let bg = call_long::<TestLongBuild, _, _>(&peer, quiet, json!({"lines": 1, "background": true}), true, never)
        .await
        .expect("background");
    assert!(bg["jobId"].as_str().is_some(), "{bg}");
    d.stop().await.expect("stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_interrupt_cancels_the_job() {
    let home = TempDir::new("cli-long-cancel").expect("home");
    let (d, peer) = daemon(&home).await;
    let rec = Arc::new(Recorder::default());
    let interrupt = || tokio::time::sleep(Duration::from_millis(300));
    let started = std::time::Instant::now();
    let e = call_long::<TestLongBuild, _, _>(&peer, rec, json!({"lines": 3, "hang": true}), true, interrupt)
        .await
        .expect_err("cancelled");
    assert_eq!(e.code(), codes::core::CANCELLED);
    assert_eq!(e.code().exit_code(), 130);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    let mut state = None;
    for _ in 0..100 {
        let jobs = d.ctx().jobs.list(&JobListParams::default());
        state = jobs.first().map(|j| j.state);
        if state == Some(JobState::Cancelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(state, Some(JobState::Cancelled));
    d.stop().await.expect("stop");
}
