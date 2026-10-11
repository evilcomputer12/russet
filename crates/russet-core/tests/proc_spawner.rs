// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The single `russet-spawner` thread [D 73] (ARCHITECTURE §5.3 `proc`, PLAN W2 tests).
//!
//! `PR_SET_PDEATHSIG` fires when the *thread* that forked a child exits. tokio's blocking pool reaps idle
//! threads, so a child forked on a pool thread would be killed mid-build. `ProcessRunner` forks on one
//! long-lived thread instead; the first test proves that a child requested from a `spawn_blocking` task
//! outlives that pool thread, and the control test proves the hazard is real (a child forked with PDEATHSIG on
//! a thread that exits dies), so the first test would catch a regression.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::time::{Duration, Instant};

use russet_core::proc::{CommandRunner, CommandSpec, ProcessRunner};

/// The kernel id of the calling thread.
fn current_tid() -> i32 {
    nix::unistd::gettid().as_raw()
}

fn thread_alive(tid: i32) -> bool {
    Path::new(&format!("/proc/self/task/{tid}")).exists()
}

/// Whether `pid` runs and is not a zombie.
fn process_running(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            // The state follows the parenthesised command name.
            let state = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next());
            !matches!(state, Some("Z") | Some("X") | None)
        }
        Err(_) => false,
    }
}

#[test]
fn a_child_spawned_from_a_blocking_task_survives_the_pool_thread() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .thread_keep_alive(Duration::from_millis(50))
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let handle = tokio::runtime::Handle::current();
        let (mut child, tid) = tokio::task::spawn_blocking(move || {
            let tid = current_tid();
            let child = handle
                .block_on(ProcessRunner.spawn(&CommandSpec::new("sleep").arg("30")))
                .expect("spawn from the blocking pool");
            (child, tid)
        })
        .await
        .expect("blocking task");
        let pid = child.id().expect("running");

        // The pool thread idles out after 50 ms; wait until the kernel has removed it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while thread_alive(tid) {
            assert!(Instant::now() < deadline, "the blocking pool thread {tid} never exited");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // PDEATHSIG would have fired by now; give the kernel a moment to deliver it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(process_running(pid), "the child {pid} died with the blocking pool thread");

        child.kill_tree().await.expect("kill");
        let out = child.wait().await.expect("reap");
        assert!(out.signal.is_some() || out.code.is_some(), "{out:?}");
    });
}

#[test]
fn control_a_child_forked_on_an_exiting_thread_with_pdeathsig_dies() {
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    let mut child = std::thread::spawn(|| {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        #[allow(unsafe_code)]
        // SAFETY: the closure runs between fork and exec and only calls prctl(2), which is async-signal-safe;
        // it allocates nothing (the error is a raw OS error code).
        unsafe {
            cmd.pre_exec(|| {
                nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))
            });
        }
        cmd.spawn().expect("spawn")
    })
    .join()
    .expect("thread");
    // The forking thread has exited: the kernel sends SIGKILL.
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        assert!(Instant::now() < deadline, "the control child survived its thread: the test would be meaningless");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.signal(), Some(9), "{status:?}");
}

/// Records the stdout lines of a run.
#[derive(Default)]
struct Lines(std::sync::Mutex<Vec<String>>);

impl russet_core::progress::ProgressSink for Lines {
    fn begin(&self, _: &str, _: bool) {}
    fn report(&self, _: Option<&str>, _: Option<u8>) {}
    fn end(&self, _: Option<&str>) {}
    fn log(&self, _: russet_core::progress::LogStream, line: &str) {
        self.0.lock().expect("lock").push(line.to_owned());
    }
}

impl russet_core::progress::EventSink for Lines {
    fn emit(&self, _: russet_core::progress::JobEventKind) {}
}

#[tokio::test]
async fn cancelling_a_run_kills_its_grandchildren_within_two_seconds() {
    let sink = Lines::default();
    let cancel = tokio_util::sync::CancellationToken::new();
    let spec = CommandSpec::new("/bin/sh").arg("-c").arg("sleep 30 & echo $!; wait");
    let run = ProcessRunner.run(&spec, &sink, &cancel);
    tokio::pin!(run);
    // Wait for the grandchild's pid on stdout.
    let grandchild = loop {
        tokio::select! {
            r = &mut run => panic!("the run ended early: {r:?}"),
            () = tokio::time::sleep(Duration::from_millis(20)) => {
                if let Some(line) = sink.0.lock().expect("lock").first() {
                    break line.trim().parse::<u32>().expect("a pid");
                }
            }
        }
    };
    assert!(process_running(grandchild));
    cancel.cancel();
    let cancelled_at = Instant::now();
    let err = run.await.expect_err("cancelled");
    assert_eq!(err.code(), russet_core::error::codes::core::CANCELLED);
    while process_running(grandchild) {
        assert!(cancelled_at.elapsed() < Duration::from_secs(2), "grandchild {grandchild} outlived 2 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
