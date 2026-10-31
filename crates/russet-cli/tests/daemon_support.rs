// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Shared helpers of the CLI's integration tests (included with `#[path]`; on its own it has no tests): a
//! temporary `RUSSET_HOME` per test whose daemons are stopped when it is dropped, the real `russet` binary with
//! a lane-D environment, process checks, a mini JSON Schema validator and a fake usbmuxd.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The `russet` binary cargo built for these tests.
pub fn russet_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_russet"))
}

/// A temporary home; dropping it stops its daemon (`daemon stop --force`, then SIGKILL).
pub struct Home {
    pub root: PathBuf,
    pub bin: PathBuf,
}

impl Home {
    pub fn new(name: &str) -> Self {
        let dir = russet_core::fsx::TempDir::new(name).expect("temp home");
        // Keep the directory (the TempDir guard would delete it now); Drop removes it after stopping the daemon.
        let root = dir.path().to_path_buf();
        std::mem::forget(dir);
        Self { root, bin: russet_bin() }
    }

    /// A `russet` command in this home with a lane-D environment.
    pub fn cmd(&self) -> Command {
        self.cmd_with(&self.bin)
    }

    /// `bin` (a copy of `russet`) in this home with a lane-D environment.
    pub fn cmd_with(&self, bin: &Path) -> Command {
        let mut c = Command::new(bin);
        c.env("RUSSET_HOME", &self.root)
            .env("RUSSET_DEVICE_BACKEND", "none")
            .env("RUSSET_SECRETS_BACKEND", "memory")
            .env_remove("RUSSET_LANE")
            .env_remove("RUSSET_DAEMON")
            .env_remove("RUSSET_NO_AUTOSTART")
            .env_remove("RUSSET_LIVE_DEVICE_LOCK")
            .env_remove("RUSSET_LOG")
            .env_remove("XDG_RUNTIME_DIR")
            .stdin(Stdio::null());
        c
    }

    /// Runs `russet <args>` and returns its output.
    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().expect("russet runs")
    }

    /// Runs `russet --json <args>`, expecting success, and parses stdout.
    pub fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        assert!(
            out.status.success(),
            "russet {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("russet {args:?}: not JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
    }

    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }

    /// The pid in `runtime/russetd.json`, if a daemon runs.
    pub fn daemon_pid(&self) -> Option<u32> {
        let text = std::fs::read_to_string(self.runtime().join("russetd.json")).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v["pid"].as_u64().and_then(|p| u32::try_from(p).ok())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let pid = self.daemon_pid();
        let _ = self.cmd().args(["daemon", "stop", "--force"]).output();
        if let Some(pid) = pid
            && !wait_gone(pid, Duration::from_secs(3))
        {
            kill9(pid);
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Whether process `pid` runs (and is not a zombie).
pub fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            !matches!(stat.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().next()), Some("Z" | "X") | None)
        }
        Err(_) => false,
    }
}

/// Waits up to `timeout` for `pid` to exit.
pub fn wait_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

pub fn kill9(pid: u32) {
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
}

/// Daemon processes (`… daemon run …`) whose environment has `RUSSET_HOME=<home>`.
pub fn daemons_of(home: &Path) -> Vec<u32> {
    let needle = format!("RUSSET_HOME={}", home.display());
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else { return out };
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else { continue };
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let is_daemon = args.windows(2).any(|w| w[0] == b"daemon" && w[1] == b"run");
        if !is_daemon || !alive(pid) {
            continue;
        }
        let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else { continue };
        if environ.split(|b| *b == 0).any(|kv| kv == needle.as_bytes()) {
            out.push(pid);
        }
    }
    out
}

/// The protocol schema committed under `schemas/rpc/v1`.
pub fn protocol_schema() -> Value {
    let manifest =
        std::env::var_os("CARGO_MANIFEST_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    let path = manifest.join("../../schemas/rpc/v1/russet-protocol.schema.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("the schema is committed")).expect("JSON")
}

/// Validates `v` against `schema` (the subset of JSON Schema that schemars emits: `type`, `properties`,
/// `required`, `additionalProperties`, `items`, `enum`, `const`, `oneOf`, `anyOf`, `allOf`, `$ref` into `$defs`,
/// `minimum`, `maximum`, `pattern` is ignored). Returns the first violation.
pub fn validate(schema: &Value, v: &Value, defs: &Value, path: &str) -> Result<(), String> {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let name = r.strip_prefix("#/$defs/").ok_or_else(|| format!("{path}: unsupported $ref {r}"))?;
        return validate(&defs[name], v, defs, path);
    }
    if let Some(Value::Bool(true)) = Some(schema) {
        return Ok(());
    }
    if let Some(types) = schema.get("type") {
        let names: Vec<&str> = match types {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let ok = names.iter().any(|t| match *t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => false,
        });
        if !ok {
            return Err(format!("{path}: {v} is not of type {names:?}"));
        }
    }
    if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
        && v.as_f64().is_some_and(|x| x < min)
    {
        return Err(format!("{path}: {v} < {min}"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        return Err(format!("{path}: {v} is not one of {e:?}"));
    }
    if let Some(c) = schema.get("const")
        && c != v
    {
        return Err(format!("{path}: {v} is not {c}"));
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(options) = schema.get(key).and_then(Value::as_array) {
            let passing = options.iter().filter(|o| validate(o, v, defs, path).is_ok()).count();
            let ok = if key == "oneOf" { passing == 1 } else { passing >= 1 };
            if !ok {
                return Err(format!("{path}: {passing} of the {key} options match {v}"));
            }
        }
    }
    if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
        for s in all {
            validate(s, v, defs, path)?;
        }
    }
    if let Some(obj) = v.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for r in required.iter().filter_map(Value::as_str) {
                if !obj.contains_key(r) {
                    return Err(format!("{path}: missing required `{r}`"));
                }
            }
        }
        for (k, child) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(s) => validate(s, child, defs, &format!("{path}/{k}"))?,
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(format!("{path}: unexpected `{k}`")),
                    Some(s @ Value::Object(_)) => validate(s, child, defs, &format!("{path}/{k}"))?,
                    _ => {}
                },
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), v.as_array()) {
        for (i, child) in arr.iter().enumerate() {
            validate(items, child, defs, &format!("{path}/{i}"))?;
        }
    }
    Ok(())
}

/// A fake usbmuxd on a Unix socket (QA §3, ARCHITECTURE §3.3 item 7): it accepts `Listen` and answers
/// `Result 0`, answers `ListDevices` with no device, and counts its connections, which a lane-D daemon must
/// never make.
pub struct FakeUsbmuxd {
    pub path: PathBuf,
    pub connections: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl FakeUsbmuxd {
    pub fn start(path: PathBuf) -> Self {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("fake usbmuxd binds");
        let connections = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = std::sync::Arc::clone(&connections);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                count.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    loop {
                        let mut header = [0u8; 16];
                        if s.read_exact(&mut header).is_err() {
                            return;
                        }
                        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
                        let tag = [header[12], header[13], header[14], header[15]];
                        let mut body = vec![0u8; len.saturating_sub(16)];
                        if s.read_exact(&mut body).is_err() {
                            return;
                        }
                        let text = String::from_utf8_lossy(&body);
                        let reply = if text.contains("ListDevices") {
                            "<key>DeviceList</key><array/>"
                        } else {
                            "<key>MessageType</key><string>Result</string><key>Number</key><integer>0</integer>"
                        };
                        let plist = format!(
                            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>{reply}</dict></plist>\n"
                        );
                        let total = u32::try_from(16 + plist.len()).unwrap_or(u32::MAX);
                        let mut out = Vec::new();
                        out.extend_from_slice(&total.to_le_bytes());
                        out.extend_from_slice(&1u32.to_le_bytes());
                        out.extend_from_slice(&8u32.to_le_bytes());
                        out.extend_from_slice(&tag);
                        out.extend_from_slice(plist.as_bytes());
                        if s.write_all(&out).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { path, connections }
    }

    pub fn connections(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::SeqCst)
    }
}
