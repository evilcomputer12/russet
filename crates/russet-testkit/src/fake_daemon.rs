// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`FakeDaemon`]: a tiny russet/1 peer for testing clients and harnesses without the real daemon (the Node
//! interop harness, transcripts, client code). It speaks the framing of ARCHITECTURE §7.2 and answers:
//!
//! | Method | Answer |
//! |---|---|
//! | `initialize` | `{protocol {major 1, minor 0}, server {name "fake-russetd", …}, connectionId, grant, capabilities}`; any other request before it gets `-32002` (§7.4) |
//! | `daemon.ping` | `{}` |
//! | `test.echo` | its params |
//! | `test.slow` | `$/progress` begin and reports every 20 ms on `params.progressToken`, until `$/cancelRequest` (`-32800`, `data.russetCode = "core.cancelled"`, §7.6) or 10 s (`{done: true}`) |
//! | `test.confirm` | sends the server request `ui.confirm` (id `s:<n>`, §9.3) and returns `{confirmed}` from the client's answer |
//! | anything else | `-32601` |
//!
//! It is a protocol double, not the daemon: W2's daemon tests use the real one.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russet_core::error::codes;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite, BufReader, WriteHalf};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};
use tokio::task::JoinHandle;

use crate::lsp;

/// JSON-RPC codes the fake uses (ARCHITECTURE §7.4, §7.6).
pub mod rpc_codes {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_CANCELLED: i64 = -32800;
}

/// A fake daemon listening on a Unix socket.
#[derive(Debug)]
pub struct FakeDaemon {
    socket: PathBuf,
    received: Arc<Mutex<Vec<Value>>>,
    task: JoinHandle<()>,
}

impl FakeDaemon {
    /// Listens on `dir/fake-russetd.sock` and serves every connection.
    #[cfg(unix)]
    pub async fn start(dir: &Path) -> io::Result<Self> {
        let socket = dir.join("fake-russetd.sock");
        let listener = tokio::net::UnixListener::bind(&socket)?;
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                serve_with_log(stream, Arc::clone(&log));
            }
        });
        Ok(Self { socket, received, task })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Every message clients sent, in arrival order.
    pub fn received(&self) -> Vec<Value> {
        self.received.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Serves one connection (an in-memory duplex, a socket) until the client closes it.
pub fn serve<S: AsyncRead + AsyncWrite + Send + 'static>(stream: S) -> JoinHandle<()> {
    serve_with_log(stream, Arc::new(Mutex::new(Vec::new())))
}

type Writer<S> = Arc<AsyncMutex<WriteHalf<S>>>;

#[derive(Default)]
struct Conn {
    initialized: bool,
    cancels: BTreeMap<String, Arc<Notify>>,
    pending: BTreeMap<String, oneshot::Sender<Value>>,
    next_server_id: u64,
}

fn serve_with_log<S: AsyncRead + AsyncWrite + Send + 'static>(stream: S, log: Arc<Mutex<Vec<Value>>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (r, w) = tokio::io::split(stream);
        let writer: Writer<S> = Arc::new(AsyncMutex::new(w));
        let conn = Arc::new(Mutex::new(Conn::default()));
        let mut reader = BufReader::new(r);
        while let Ok(Some(msg)) = lsp::read_message(&mut reader).await {
            if let Ok(mut l) = log.lock() {
                l.push(msg.clone());
            }
            dispatch(msg, &writer, &conn);
        }
    })
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

fn dispatch<S: AsyncRead + AsyncWrite + Send + 'static>(msg: Value, writer: &Writer<S>, conn: &Arc<Mutex<Conn>>) {
    let method = msg.get("method").and_then(Value::as_str).map(str::to_owned);
    let id = msg.get("id").cloned();
    match (method, id) {
        // A response to one of our server requests.
        (None, Some(id)) => {
            let tx = conn.lock().ok().and_then(|mut c| c.pending.remove(&id_key(&id)));
            if let Some(tx) = tx {
                let _ = tx.send(msg);
            }
        }
        (Some(m), None) if m == "$/cancelRequest" => {
            if let Some(target) = msg.pointer("/params/id") {
                let n = conn.lock().ok().and_then(|c| c.cancels.get(&id_key(target)).cloned());
                if let Some(n) = n {
                    n.notify_one();
                }
            }
        }
        (Some(_), None) => {}
        (Some(m), Some(id)) => {
            let writer = Arc::clone(writer);
            let conn = Arc::clone(conn);
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            tokio::spawn(async move {
                let reply = handle(&m, &id, params, &writer, &conn).await;
                let _ = send(&writer, &reply).await;
            });
        }
        (None, None) => {}
    }
}

async fn send<S: AsyncRead + AsyncWrite + Send>(writer: &Writer<S>, msg: &Value) -> io::Result<()> {
    let mut w = writer.lock().await;
    lsp::write_message(&mut *w, msg).await
}

fn error(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut e = json!({ "code": code, "message": message });
    if let Some(d) = data {
        e["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": e })
}

fn result(id: &Value, r: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

async fn handle<S: AsyncRead + AsyncWrite + Send + 'static>(
    method: &str,
    id: &Value,
    params: Value,
    writer: &Writer<S>,
    conn: &Arc<Mutex<Conn>>,
) -> Value {
    let initialized = conn.lock().map(|c| c.initialized).unwrap_or(false);
    if method != "initialize" && !initialized {
        return error(id, rpc_codes::SERVER_NOT_INITIALIZED, "initialize first", None);
    }
    match method {
        "initialize" => {
            if let Ok(mut c) = conn.lock() {
                c.initialized = true;
            }
            result(
                id,
                json!({
                    "protocol": { "major": 1, "minor": 0 },
                    "server": { "name": "fake-russetd", "version": "0.0.0", "pid": std::process::id(),
                                "platform": std::env::consts::OS, "deviceBackend": "none" },
                    "connectionId": "c-1",
                    "grant": { "profile": "test", "allow": ["read"], "confirm": [], "approval": [], "deny": [] },
                    "capabilities": { "methods": ["daemon.ping", "test.echo", "test.slow", "test.confirm"],
                                      "notifications": ["$/progress"], "features": {} }
                }),
            )
        }
        "daemon.ping" => result(id, json!({})),
        "test.echo" => result(id, params),
        "test.slow" => slow(id, params, writer, conn).await,
        "test.confirm" => confirm(id, params, writer, conn).await,
        _ => error(id, rpc_codes::METHOD_NOT_FOUND, &format!("method not found: {method}"), None),
    }
}

async fn slow<S: AsyncRead + AsyncWrite + Send + 'static>(
    id: &Value,
    params: Value,
    writer: &Writer<S>,
    conn: &Arc<Mutex<Conn>>,
) -> Value {
    let cancel = Arc::new(Notify::new());
    if let Ok(mut c) = conn.lock() {
        c.cancels.insert(id_key(id), Arc::clone(&cancel));
    }
    let token = params.get("progressToken").cloned();
    let progress = |value: Value| token.clone().map(|t| json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": t, "value": value } }));
    if let Some(p) = progress(json!({ "kind": "begin", "title": "test.slow", "cancellable": true })) {
        let _ = send(writer, &p).await;
    }
    let mut n = 0u32;
    let outcome = loop {
        tokio::select! {
            () = cancel.notified() => break None,
            () = tokio::time::sleep(Duration::from_millis(20)) => {
                n += 1;
                if n >= 500 {
                    break Some(());
                }
                if let Some(p) = progress(json!({ "kind": "report", "percentage": (n % 100) as u8 })) {
                    let _ = send(writer, &p).await;
                }
            }
        }
    };
    if let Ok(mut c) = conn.lock() {
        c.cancels.remove(&id_key(id));
    }
    if let Some(p) = progress(json!({ "kind": "end" })) {
        let _ = send(writer, &p).await;
    }
    match outcome {
        None => error(
            id,
            rpc_codes::REQUEST_CANCELLED,
            "cancelled",
            Some(json!({ "russetCode": codes::core::CANCELLED.as_str(), "retryable": false, "chain": [] })),
        ),
        Some(()) => result(id, json!({ "done": true })),
    }
}

async fn confirm<S: AsyncRead + AsyncWrite + Send + 'static>(
    id: &Value,
    params: Value,
    writer: &Writer<S>,
    conn: &Arc<Mutex<Conn>>,
) -> Value {
    let (tx, rx) = oneshot::channel();
    let server_id = {
        let Ok(mut c) = conn.lock() else { return error(id, -32603, "internal", None) };
        c.next_server_id += 1;
        let sid = json!(format!("s:{}", c.next_server_id));
        c.pending.insert(id_key(&sid), tx);
        sid
    };
    let request = json!({
        "jsonrpc": "2.0", "id": server_id, "method": "ui.confirm",
        "params": {
            "title": params.get("title").cloned().unwrap_or(json!("Confirm")),
            "message": params.get("message").cloned().unwrap_or(json!("Continue?")),
            "danger": false,
            "requestedBy": { "kind": "test", "name": "fake-russetd" },
            "timeoutMs": 5000
        }
    });
    if send(writer, &request).await.is_err() {
        return error(id, -32603, "cannot send ui.confirm", None);
    }
    match tokio::time::timeout(Duration::from_secs(5), rx).await {
        Ok(Ok(answer)) => {
            let confirmed = answer.pointer("/result/confirmed").and_then(Value::as_bool).unwrap_or(false);
            result(id, json!({ "confirmed": confirmed }))
        }
        _ => {
            let code = codes::perm::CONFIRMATION_UNAVAILABLE;
            error(
                id,
                i64::from(code.number()),
                "no answer to ui.confirm",
                Some(json!({ "russetCode": code.as_str(), "retryable": false, "chain": [] })),
            )
        }
    }
}
