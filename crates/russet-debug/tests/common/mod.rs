// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Shared pieces of russet-debug's integration tests: a host "device" that launches the probe suspended and
//! hands out fresh lldb-server connections as debugserver connections, a fake-debugserver device, and LLDB
//! runners.

#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_debug::launch::{InstalledAppInfo, LaunchSpec, Launched, ProcessExit};
use russet_debug::{BoxStream, DeviceServices};
use russet_device::{DeviceSelector, Udid};
use russet_testkit::debug::{FakeDebugserver, LldbServer, LldbTools};

pub fn io_err(e: std::io::Error) -> Error {
    Error::new(codes::core::IO, e.to_string())
}

/// A host "device": `launch` spawns a host program (with `--suspended` for debug launches, so it does nothing
/// until a debugger attaches, like a CoreDevice suspended launch), `connect_debugserver` starts a fresh process-less
/// lldb-server per connection, like the debugproxy service starts a debugserver.
pub struct HostDevice {
    pub tools: LldbTools,
    pub program: PathBuf,
    servers: Mutex<Vec<LldbServer>>,
    pub launched: Mutex<Vec<u32>>,
    pub signals: Mutex<Vec<(u32, i32)>>,
}

impl HostDevice {
    pub fn new(tools: LldbTools, program: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            tools,
            program,
            servers: Mutex::new(Vec::new()),
            launched: Mutex::new(Vec::new()),
            signals: Mutex::new(Vec::new()),
        })
    }

    /// Kills every program this device launched (test cleanup).
    pub fn cleanup(&self) {
        for pid in self.launched.lock().unwrap().iter() {
            russet_testkit::debug::programs::kill_process(*pid);
        }
    }
}

impl Drop for HostDevice {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[async_trait]
impl DeviceServices for HostDevice {
    async fn resolve(&self, _sel: &DeviceSelector) -> Result<Udid> {
        Ok(Udid("host".to_owned()))
    }

    async fn installed_app(&self, _udid: &Udid, bundle_id: &str) -> Result<Option<InstalledAppInfo>> {
        Ok(Some(InstalledAppInfo { bundle_id: bundle_id.to_owned(), path: None, executable: None }))
    }

    async fn launch(&self, _udid: &Udid, spec: &LaunchSpec) -> Result<Launched> {
        let mut cmd = tokio::process::Command::new(&self.program);
        if spec.suspended {
            cmd.arg("--suspended");
        }
        cmd.args(&spec.args).envs(&spec.env).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(io_err)?;
        let pid = child.id().ok_or_else(|| Error::new(codes::core::INTERNAL, "no pid"))?;
        self.launched.lock().unwrap().push(pid);
        let stdout = child.stdout.take().map(|s| Box::new(s) as russet_debug::launch::StdioStream);
        let exit = Box::pin(async move {
            use std::os::unix::process::ExitStatusExt;
            match child.wait().await {
                Ok(s) => ProcessExit { code: s.code(), signal: s.signal() },
                Err(_) => ProcessExit::default(),
            }
        });
        let mut l = Launched::pid_only(pid);
        l.executable = Some(self.program.to_string_lossy().into_owned());
        l.stdio = stdout;
        l.exit = Some(exit);
        Ok(l)
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        let server = LldbServer::listen(&self.tools).map_err(io_err)?;
        let s = tokio::net::TcpStream::connect(server.addr()).await.map_err(io_err)?;
        self.servers.lock().unwrap().push(server);
        Ok(Box::new(s))
    }

    async fn signal(&self, _udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.signals.lock().unwrap().push((pid, sig));
        let status =
            std::process::Command::new("kill").arg(format!("-{sig}")).arg(pid.to_string()).status().map_err(io_err)?;
        if status.success() { Ok(()) } else { Err(Error::new(codes::device::IO, format!("kill -{sig} {pid} failed"))) }
    }
}

/// A "device" whose debugserver is a [`FakeDebugserver`] and whose launch only reports a pid.
pub struct FakeDevice {
    pub fake: FakeDebugserver,
    pub pid: u32,
    pub executable: String,
    pub signals: Mutex<Vec<(u32, i32)>>,
    pub launches: Mutex<Vec<LaunchSpec>>,
}

#[async_trait]
impl DeviceServices for FakeDevice {
    async fn resolve(&self, sel: &DeviceSelector) -> Result<Udid> {
        match sel {
            DeviceSelector::Udid(u) => Ok(u.clone()),
            _ => Ok(Udid("00008101-000000000000001E".to_owned())),
        }
    }

    async fn launch(&self, _udid: &Udid, spec: &LaunchSpec) -> Result<Launched> {
        self.launches.lock().unwrap().push(spec.clone());
        let mut l = Launched::pid_only(self.pid);
        l.executable = Some(self.executable.clone());
        Ok(l)
    }

    async fn connect_debugserver(&self, _udid: &Udid) -> Result<BoxStream> {
        let s = tokio::net::TcpStream::connect(self.fake.addr()).await.map_err(io_err)?;
        Ok(Box::new(s))
    }

    async fn signal(&self, _udid: &Udid, pid: u32, sig: i32) -> Result<()> {
        self.signals.lock().unwrap().push((pid, sig));
        Ok(())
    }

    async fn describe(&self, _udid: &Udid) -> Result<Option<russet_debug::DeviceFacts>> {
        Ok(Some(russet_debug::DeviceFacts {
            product_type: Some("iPhone13,2".into()),
            product_version: Some("27.0".into()),
            build_version: Some("24A437".into()),
            transport: Some("usb".into()),
        }))
    }
}

/// Output of an LLDB run.
pub struct LldbRun {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

/// Runs `lldb` with `args` and the tools' env, with a time limit.
pub async fn run_lldb(tools: &LldbTools, args: &[String], limit: Duration) -> LldbRun {
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new(&tools.lldb);
    cmd.args(args).stdin(Stdio::null()).kill_on_drop(true);
    for (k, v) in &tools.env {
        cmd.env(k, v);
    }
    let out = tokio::time::timeout(limit, cmd.output()).await.expect("lldb finished in time").expect("lldb ran");
    LldbRun {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        elapsed: started.elapsed(),
    }
}

/// Polls `pred` until it holds or `limit` passes.
pub async fn eventually(limit: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    pred()
}

/// A short scratch directory for sockets and scripts.
pub fn scratch(tag: &str) -> russet_testkit::debug::ScratchDir {
    russet_testkit::debug::ScratchDir::new(tag).unwrap()
}

pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}
