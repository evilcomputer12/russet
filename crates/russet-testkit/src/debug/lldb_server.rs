// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `lldb-server gdbserver` as a stand-in for the device's debugserver (H1–H3, RD §23.2).
//!
//! [`LldbServer::listen`] starts it with no process, like debugserver behind the `debugproxy` service: the client
//! (the broker) then sends `vAttach` or `vRun`. [`LldbServer::with_program`] launches a program stopped at its
//! entry. The port comes from `--pipe 1` (lldb-server writes it to stdout, NUL-terminated). The server and its
//! process tree are killed when the fixture drops.

use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::tools::LldbTools;

/// A running `lldb-server gdbserver`.
#[derive(Debug)]
pub struct LldbServer {
    child: Child,
    port: u16,
}

impl LldbServer {
    /// No process: the client attaches (`vAttach`) or launches (`vRun`).
    pub fn listen(tools: &LldbTools) -> io::Result<Self> {
        Self::start(tools, None, &[])
    }

    /// Launches `program` stopped at its entry point.
    pub fn with_program(tools: &LldbTools, program: &Path, args: &[&str]) -> io::Result<Self> {
        Self::start(tools, Some(program), args)
    }

    fn start(tools: &LldbTools, program: Option<&Path>, args: &[&str]) -> io::Result<Self> {
        let mut cmd = tools.command(&tools.lldb_server);
        cmd.arg("gdbserver").arg("127.0.0.1:0").arg("--pipe").arg("1");
        if let Some(p) = program {
            cmd.arg("--").arg(p).args(args);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut port = Vec::new();
            let mut byte = [0u8; 1];
            while let Ok(1) = stdout.read(&mut byte) {
                if byte[0] == 0 {
                    break;
                }
                port.push(byte[0]);
            }
            let _ = tx.send(port);
            // Keep draining, so the server never blocks on a full pipe.
            let _ = io::copy(&mut stdout, &mut io::sink());
        });
        let port = rx
            .recv_timeout(Duration::from_secs(10))
            .ok()
            .and_then(|p| String::from_utf8(p).ok())
            .and_then(|p| p.trim().parse::<u16>().ok());
        match port {
            Some(port) => Ok(Self { child, port }),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                Err(io::Error::other("lldb-server did not report its port"))
            }
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port))
    }

    /// The server's own pid.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether the server is still running.
    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits up to `limit` for the server to exit by itself.
    pub fn wait_exit(&mut self, limit: Duration) -> bool {
        let deadline = std::time::Instant::now() + limit;
        while std::time::Instant::now() < deadline {
            if !self.is_running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for LldbServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
