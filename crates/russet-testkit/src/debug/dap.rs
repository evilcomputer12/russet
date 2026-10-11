// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A Debug Adapter Protocol test client over a child's stdio (H2), ported from
//! `docs/design/run-debug-prototypes/dap_attach_test.py`: it drives `lldb-dap` through `initialize`, an
//! `attach` made only of `attachCommands`, `configurationDone`, and `disconnect`.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::json::Json;
use super::tools::LldbTools;

#[derive(Default)]
struct Inbox {
    responses: BTreeMap<u64, Json>,
    events: Vec<Json>,
    closed: bool,
}

type Shared = Arc<(Mutex<Inbox>, Condvar)>;

/// A DAP client of a debug adapter child process.
#[derive(Debug)]
pub struct DapClient {
    child: Child,
    stdin: ChildStdin,
    seq: u64,
    inbox: SharedInbox,
    reader: Option<JoinHandle<()>>,
}

/// `Debug` wrapper for the inbox.
struct SharedInbox(Shared);

impl std::fmt::Debug for SharedInbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Inbox")
    }
}

fn lock(m: &Mutex<Inbox>) -> MutexGuard<'_, Inbox> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl DapClient {
    /// Spawns `lldb-dap` with the LLDB environment.
    pub fn spawn_lldb_dap(tools: &LldbTools) -> io::Result<Self> {
        Self::spawn(tools, &tools.lldb_dap)
    }

    /// Spawns any adapter that speaks DAP on stdio.
    pub fn spawn(tools: &LldbTools, adapter: &Path) -> io::Result<Self> {
        let mut cmd = tools.command(adapter);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| io::Error::other("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| io::Error::other("no stdout"))?;
        let inbox: Shared = Arc::new((Mutex::new(Inbox::default()), Condvar::new()));
        let reader = {
            let inbox = Arc::clone(&inbox);
            std::thread::spawn(move || read_messages(stdout, &inbox))
        };
        Ok(Self { child, stdin, seq: 0, inbox: SharedInbox(inbox), reader: Some(reader) })
    }

    /// Sends a request; returns its `seq`.
    pub fn send(&mut self, command: &str, arguments: Option<Json>) -> io::Result<u64> {
        self.seq += 1;
        let mut msg =
            vec![("seq", Json::from(self.seq)), ("type", Json::str("request")), ("command", Json::str(command))];
        if let Some(a) = arguments {
            msg.push(("arguments", a));
        }
        let body = Json::object(msg).to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
        self.stdin.flush()?;
        Ok(self.seq)
    }

    /// Waits for the response to request `seq`.
    pub fn wait_response(&self, seq: u64, limit: Duration) -> Result<Json, String> {
        let (m, cv) = &*self.inbox.0;
        let deadline = Instant::now() + limit;
        let mut inbox = lock(m);
        loop {
            if let Some(r) = inbox.responses.remove(&seq) {
                return Ok(r);
            }
            if inbox.closed {
                return Err(format!("the adapter closed before answering request {seq}"));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!("no response to request {seq} within {limit:?}"));
            }
            inbox = cv.wait_timeout(inbox, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Sends a request and waits for its response.
    pub fn request(&mut self, command: &str, arguments: Option<Json>, limit: Duration) -> Result<Json, String> {
        let seq = self.send(command, arguments).map_err(|e| e.to_string())?;
        self.wait_response(seq, limit)
    }

    /// Waits for the first event named `name` (events seen earlier count too).
    pub fn wait_event(&self, name: &str, limit: Duration) -> Result<Json, String> {
        let (m, cv) = &*self.inbox.0;
        let deadline = Instant::now() + limit;
        let mut inbox = lock(m);
        loop {
            if let Some(e) = inbox.events.iter().find(|e| e.get("event").as_str() == Some(name)) {
                return Ok(e.clone());
            }
            if inbox.closed {
                return Err(format!("the adapter closed before the {name} event"));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!("no {name} event within {limit:?}"));
            }
            inbox = cv.wait_timeout(inbox, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Every event received so far.
    pub fn events(&self) -> Vec<Json> {
        lock(&self.inbox.0.0).events.clone()
    }

    /// `initialize` with the client capabilities VS Code sends.
    pub fn initialize(&mut self) -> Result<Json, String> {
        let args = Json::object([
            ("clientID", Json::str("russet-test")),
            ("adapterID", Json::str("lldb-dap")),
            ("linesStartAt1", Json::from(true)),
            ("columnsStartAt1", Json::from(true)),
            ("pathFormat", Json::str("path")),
        ]);
        self.request("initialize", Some(args), Duration::from_secs(30))
    }

    /// Sends `attach` with only `initCommands` and `attachCommands` (RD §9.2); returns the request's `seq`.
    /// The response arrives after `configurationDone`.
    pub fn attach(&mut self, init: &[String], attach: &[String], stop_on_entry: bool) -> io::Result<u64> {
        let args = Json::object([
            ("name", Json::str("russet-test")),
            ("stopOnEntry", Json::from(stop_on_entry)),
            ("initCommands", Json::strings(init.iter().cloned())),
            ("attachCommands", Json::strings(attach.iter().cloned())),
            ("timeout", Json::from(60u64)),
        ]);
        self.send("attach", Some(args))
    }

    /// `disconnect`; waits briefly for the response.
    pub fn disconnect(&mut self, terminate: bool) -> Result<Json, String> {
        let args = Json::object([("terminateDebuggee", Json::from(terminate))]);
        self.request("disconnect", Some(args), Duration::from_secs(10))
    }

    /// Waits for the adapter to exit.
    pub fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

fn read_messages(stdout: impl Read, inbox: &Shared) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return close(inbox),
                Ok(_) => {}
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                length = v.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else { return close(inbox) };
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return close(inbox);
        }
        let Ok(msg) = Json::parse(&String::from_utf8_lossy(&body)) else { continue };
        let (m, cv) = &**inbox;
        let mut guard = lock(m);
        match msg.get("type").as_str() {
            Some("response") => {
                if let Some(seq) = msg.get("request_seq").as_u64() {
                    guard.responses.insert(seq, msg);
                }
            }
            Some("event") => guard.events.push(msg),
            _ => {}
        }
        cv.notify_all();
    }
}

fn close(inbox: &Shared) {
    let (m, cv) = &**inbox;
    lock(m).closed = true;
    cv.notify_all();
}
