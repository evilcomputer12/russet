// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The Node interop harness (CORE-15, shared with W2): stock vscode-jsonrpc 9.0.3, the client library of the VS
//! Code extension, drives a russet/1 daemon over its socket (ARCHITECTURE §7.2; core §19 "Interop", §23 #12).
//!
//! The JavaScript side is `crates/russet-testkit/node/interop/interop.mjs`; install its one dependency once with
//! `npm ci --prefix crates/russet-testkit/node/interop` (CI does). An [`InteropPlan`] names the socket, the
//! `initialize` params and the steps; [`run`] executes `node interop.mjs <plan>` and returns the
//! [`InteropReport`]. W2's daemon test passes its own method names for the progress and confirmation steps:
//!
//! ```no_run
//! # fn demo(socket: &std::path::Path) -> Result<(), russet_testkit::node::NodeError> {
//! use russet_testkit::node::{InteropPlan, InteropStep, run};
//! let plan = InteropPlan::new(socket)
//!     .step(InteropStep::request("ping", "daemon.ping", serde_json::json!({})))
//!     .step(InteropStep::unknown_method("unknown", "russet.noSuchMethod"));
//! let report = run(&plan)?;
//! assert!(report.ok, "{report:#?}");
//! # Ok(()) }
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The harness directory.
pub fn harness_dir() -> PathBuf {
    crate::testkit_dir().join("node/interop")
}

/// Why the harness could not run or failed.
#[derive(Debug)]
pub enum NodeError {
    /// `node` or the harness's `node_modules` is missing; tests skip with this reason unless
    /// `RUSSET_REQUIRE_NODE_HARNESS=1` (CI).
    Unavailable(String),
    /// The harness ran but its output was not a report.
    BadOutput {
        status: String,
        stdout: String,
        stderr: String,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "the Node interop harness is unavailable: {why}"),
            Self::BadOutput { status, stdout, stderr } => {
                write!(f, "the Node interop harness exited with {status}:\nstdout: {stdout}\nstderr: {stderr}")
            }
            Self::Io(e) => write!(f, "running the Node interop harness: {e}"),
        }
    }
}

impl std::error::Error for NodeError {}

/// One step of a plan (`kind` is the harness's step kind).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InteropStep {
    /// `sendRequest(method, params)`; ok when it succeeds.
    Request { name: String, method: String, params: Value },
    /// ok when the answer is JSON-RPC `-32601`.
    UnknownMethod { name: String, method: String },
    /// ok when the first `$/progress` arrives and the cancelled request answers `-32800`.
    CancelAfterProgress { name: String, method: String, params: Value },
    /// ok when the daemon sends `ui.confirm` and the result's `confirmed` equals the plan's answer.
    Confirm { name: String, method: String, params: Value },
}

impl InteropStep {
    pub fn request(name: &str, method: &str, params: Value) -> Self {
        Self::Request { name: name.into(), method: method.into(), params }
    }
    pub fn unknown_method(name: &str, method: &str) -> Self {
        Self::UnknownMethod { name: name.into(), method: method.into() }
    }
    pub fn cancel_after_progress(name: &str, method: &str, params: Value) -> Self {
        Self::CancelAfterProgress { name: name.into(), method: method.into(), params }
    }
    pub fn confirm(name: &str, method: &str, params: Value) -> Self {
        Self::Confirm { name: name.into(), method: method.into(), params }
    }
}

/// What the harness does.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteropPlan {
    pub socket: PathBuf,
    /// `initialize` params (ARCHITECTURE §7.4).
    pub initialize: Value,
    pub steps: Vec<InteropStep>,
    /// The harness's answer to `ui.confirm` (§9.3).
    pub confirm_answer: Value,
    pub timeout_ms: u64,
}

impl InteropPlan {
    /// A plan for `socket` that initializes as an `ide` client, the kind vscode-jsonrpc serves in the IDE.
    pub fn new(socket: &Path) -> Self {
        Self {
            socket: socket.to_path_buf(),
            initialize: json!({
                "protocol": { "major": 1, "minor": 0 },
                "client": { "kind": "ide", "name": "russet-node-interop", "version": "0.1.0", "pid": std::process::id() },
                "capabilities": { "confirm": true, "prompt": false, "openUrl": false, "approvals": false,
                                  "ideDebug": false, "interactive": false }
            }),
            steps: Vec::new(),
            confirm_answer: json!({ "confirmed": true }),
            timeout_ms: 10_000,
        }
    }

    pub fn step(mut self, s: InteropStep) -> Self {
        self.steps.push(s);
        self
    }

    pub fn initialize(mut self, params: Value) -> Self {
        self.initialize = params;
        self
    }

    pub fn confirm_answer(mut self, confirmed: bool) -> Self {
        self.confirm_answer = json!({ "confirmed": confirmed });
        self
    }

    /// The plan that exercises every interop case of W2's test list (PLAN W2: initialize, `$/progress`, cancel
    /// → `-32800`, unknown method → `-32601`, a `ui.confirm` answered by the client) against methods named by
    /// the caller.
    pub fn full(
        socket: &Path,
        progress_method: &str,
        progress_params: Value,
        confirm_method: &str,
        confirm_params: Value,
    ) -> Self {
        Self::new(socket)
            .step(InteropStep::request("ping", "daemon.ping", json!({})))
            .step(InteropStep::unknown_method("unknown-method", "russet.noSuchMethod"))
            .step(InteropStep::cancel_after_progress("progress-and-cancel", progress_method, progress_params))
            .step(InteropStep::confirm("ui-confirm", confirm_method, confirm_params))
    }
}

/// One step's outcome.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StepReport {
    pub name: String,
    pub kind: String,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<Value>,
    #[serde(default)]
    pub progress: Option<Vec<Value>>,
    #[serde(default)]
    pub confirm: Option<Value>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// The harness's report.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InteropReport {
    pub ok: bool,
    pub steps: Vec<StepReport>,
    #[serde(default)]
    pub fatal: Option<String>,
}

impl InteropReport {
    pub fn step(&self, name: &str) -> Option<&StepReport> {
        self.steps.iter().find(|s| s.name == name)
    }
}

/// `Ok` when `node` (>= 20) and the harness's dependencies are installed.
pub fn available() -> Result<(), NodeError> {
    let out = Command::new("node")
        .arg("--version")
        .output()
        .map_err(|e| NodeError::Unavailable(format!("node is not on PATH ({e})")))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let major: u32 = version.trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    if major < 20 {
        return Err(NodeError::Unavailable(format!("node {version} is older than 20")));
    }
    let module = harness_dir().join("node_modules/vscode-jsonrpc/package.json");
    if !module.is_file() {
        return Err(NodeError::Unavailable(format!(
            "vscode-jsonrpc is not installed: npm ci --prefix {}",
            harness_dir().display()
        )));
    }
    Ok(())
}

/// Whether a skipped harness must fail the test instead (`RUSSET_REQUIRE_NODE_HARNESS=1`, set in CI).
pub fn required() -> bool {
    std::env::var_os("RUSSET_REQUIRE_NODE_HARNESS").is_some_and(|v| v == "1")
}

/// Runs the harness (blocking; call it from `spawn_blocking` in async tests).
pub fn run(plan: &InteropPlan) -> Result<InteropReport, NodeError> {
    available()?;
    let dir = tempfile::tempdir().map_err(NodeError::Io)?;
    let plan_path = dir.path().join("plan.json");
    let text = serde_json::to_vec_pretty(plan).map_err(|e| NodeError::Io(std::io::Error::other(e)))?;
    std::fs::write(&plan_path, text).map_err(NodeError::Io)?;
    let out = Command::new("node")
        .arg(harness_dir().join("interop.mjs"))
        .arg(&plan_path)
        .current_dir(harness_dir())
        .output()
        .map_err(NodeError::Io)?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or_default();
    serde_json::from_str(line).map_err(|_| NodeError::BadOutput {
        status: out.status.to_string(),
        stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}
