// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Probes through russet-core's `CommandRunner` ([D 73]): `RunnerExec` hands the runner the program, arguments,
//! environment and timeout, and the resolver works the same over it.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};
use russet_core::proc::{ChildProcess, CommandOutput, CommandRunner, CommandSpec};
use russet_core::progress::EventSink;
use russet_toolchain::exec::{Exec, RunnerExec};
use russet_toolchain::{Component, Requirement, Toolchains};
use tokio_util::sync::CancellationToken;

/// A test runner: runs the program with `std::process` on a blocking thread and records every spec.
#[derive(Default)]
struct StdRunner {
    specs: Mutex<Vec<CommandSpec>>,
}

#[async_trait]
impl CommandRunner for StdRunner {
    async fn run(
        &self,
        spec: &CommandSpec,
        _sink: &dyn EventSink,
        _cancel: &CancellationToken,
    ) -> russet_core::Result<CommandOutput> {
        self.specs.lock().expect("specs").push(spec.clone());
        let spec = spec.clone();
        let out = tokio::task::spawn_blocking(move || {
            std::process::Command::new(&spec.program)
                .args(&spec.args)
                .envs(&spec.env)
                .stdin(std::process::Stdio::null())
                .output()
        })
        .await
        .expect("join")
        .map_err(|e| Error::new(codes::core::IO, format!("spawn: {e}")).with_hint("test runner"))?;
        Ok(CommandOutput { code: out.status.code(), signal: None, stdout: out.stdout, stderr: out.stderr })
    }

    async fn spawn(&self, _spec: &CommandSpec) -> russet_core::Result<Box<dyn ChildProcess>> {
        Err(Error::new(codes::core::NOT_IMPLEMENTED, "probes never spawn").with_hint("test runner"))
    }
}

#[tokio::test]
async fn probes_run_through_the_command_runner() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = common::swift_toolchain(&tmp.path().join("swift-6.4"), "6.4");
    let runner = Arc::new(StdRunner::default());

    let exec = RunnerExec::new(runner.clone());
    let env = BTreeMap::from([("RUSSET_TEST_VAR".to_owned(), "1".to_owned())]);
    let out = exec.output(&bin.join("swift"), &["--version"], &env, Duration::from_secs(7)).await.expect("runs");
    assert!(out.success() && out.stdout.contains("Swift version 6.4"), "{out:?}");
    let spec = runner.specs.lock().expect("specs")[0].clone();
    assert_eq!(spec.program, bin.join("swift"));
    assert_eq!(spec.args, ["--version"]);
    assert_eq!(spec.env.get(std::ffi::OsStr::new("RUSSET_TEST_VAR")).and_then(|v| v.to_str()), Some("1"));
    assert_eq!(spec.timeout, Some(Duration::from_secs(7)));
    assert!(!spec.env_clear, "probes inherit the daemon's environment");

    let (manifest, ..) = common::signed_manifest("");
    let tc = Toolchains::new(RussetDirs::under(&tmp.path().join("home")), Arc::new(manifest))
        .with_host(common::host(vec![bin], vec![], tmp.path()))
        .with_runner(runner.clone());
    let swift = tc.resolve(Component::Swift, &Requirement::any()).await.expect("swift resolves");
    assert_eq!(swift.version, "6.4");
    assert!(runner.specs.lock().expect("specs").len() >= 2, "the resolver's probe went through the runner");
}
