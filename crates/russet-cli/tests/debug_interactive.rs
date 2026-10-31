// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet debug`'s interactive LLDB and Ctrl-C. LLDB shares the terminal and `russet`'s process group, so the
//! terminal's SIGINT reaches both: LLDB handles it (it interrupts the app), and `russet` must survive it, or it
//! would die, leave LLDB orphaned and lose its owned session.
//!
//! The test starts this test binary again as the "russet" process (the `helper` test), in a process group of its
//! own as a shell job is, with `sh` standing in for LLDB, and sends SIGINT to the whole group as the terminal does.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Set in the child: the scratch directory the fake LLDB and the test talk through.
const HELPER_DIR: &str = "RUSSET_TEST_INTERACTIVE_DIR";

/// The fake LLDB: counts its SIGINTs, says when it is ready, and exits 7 when told to (or after 30 s).
const FAKE_LLDB: &str = r#"trap 'echo int >> "$DIR/ints"' INT
echo ready > "$DIR/ready"
i=0
while [ ! -e "$DIR/done" ] && [ "$i" -lt 600 ]; do sleep 0.05; i=$((i + 1)); done
exit 7"#;

/// In the child only: what `russet debug` does with an interactive LLDB.
#[test]
fn helper() {
    let Some(dir) = std::env::var_os(HELPER_DIR) else { return };
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let status = rt
        .block_on(async {
            let mut lldb = tokio::process::Command::new("sh");
            lldb.arg("-c").arg(FAKE_LLDB).env("DIR", &dir);
            russet_cli::cmd::debug::run_interactive(&mut lldb).await
        })
        .unwrap();
    std::fs::write(Path::new(&dir).join("status"), status.code().unwrap_or(-1).to_string()).unwrap();
}

fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

#[test]
fn ctrl_c_reaches_lldb_and_russet_survives_it() {
    let dir = std::env::temp_dir().join(format!("russet-cli-sigint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stderr = std::fs::File::create(dir.join("helper.err")).unwrap();
    let mut russet = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "helper", "--nocapture", "--test-threads", "1"])
        .env(HELPER_DIR, &dir)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .unwrap();
    let log = || std::fs::read_to_string(dir.join("helper.err")).unwrap_or_default();
    assert!(wait_for(Duration::from_secs(30), || dir.join("ready").exists()), "the fake LLDB never started: {}", log());

    // Ctrl-C twice, to the whole process group, as the terminal sends it.
    let group = format!("-{}", russet.id());
    for _ in 0..2 {
        assert!(Command::new("kill").args(["-INT", "--", &group]).status().unwrap().success());
        std::thread::sleep(Duration::from_millis(300));
    }
    let ints = || std::fs::read_to_string(dir.join("ints")).map(|s| s.lines().count()).unwrap_or(0);
    assert!(wait_for(Duration::from_secs(10), || ints() >= 2), "LLDB saw {} SIGINTs", ints());
    assert!(russet.try_wait().unwrap().is_none(), "russet died of SIGINT: {}", log());

    // LLDB quits on its own; russet returns its status.
    std::fs::write(dir.join("done"), b"").unwrap();
    assert!(wait_for(Duration::from_secs(30), || russet.try_wait().unwrap().is_some()), "russet did not finish");
    let status = russet.wait().unwrap();
    assert!(status.success(), "{status:?}: {}", log());
    assert_eq!(std::fs::read_to_string(dir.join("status")).unwrap(), "7", "russet waits for LLDB's own exit");
    let _ = std::fs::remove_dir_all(&dir);
}
