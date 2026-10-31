// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`ReplayConnector`]: an [`io::ServiceConnector`] that serves a recorded [`Trace`] with no sockets and no
//! device (QA §3.5.4). Used by lane-D tests and `RUSSET_DEVICE_BACKEND=replay:<trace>` (in-process daemons).
//!
//! - **Discovery.** [`ServiceConnector::watch`] reports the trace's device (from the header), and `mux.list` is
//!   answered from the header too, so [`russet_device::DeviceManager::start`] finds the device synchronously.
//! - **Facade calls.** Every other [`ServiceConnector::call`] is matched against the recorded
//!   [`Record::Call`]s in order and replays the recorded result or error. A recorded
//!   `device.serviceNotImplemented` replays faithfully, so the device plane's fallbacks (`lockdown.values` →
//!   `lockdown.get_value`) take the same path they did when recording.
//! - **Service streams.** [`ServiceConnector::lockdown_service`] and [`ServiceConnector::rsd_service`] find the
//!   recorded connection for the service and play its `d2c` bytes; the client's `c2d` bytes are checked against
//!   the recording per [`ReplayMode`].
//! - **Checks.** [`ReplayConnector::assert_consumed`] fails if a recorded connection was never opened or a
//!   stream mismatched.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use russet_core::error::{Error, Result, codes};
use russet_device::io::{FacadeCall, ServiceConnector, facade};
use russet_device::{DeviceEvent, Udid};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use crate::rdt::{Dir, Record, Trace, Via};
use crate::scenario::{base64_decode, code_by_name};

/// How strictly the client's bytes must match the recording (QA §3.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplayMode {
    /// Client bytes must equal the recorded bytes exactly, and call arguments must equal the recorded arguments.
    Strict,
    /// Call arguments must be a superset of the recorded arguments; client stream bytes must match. The default.
    #[default]
    Semantic,
    /// Only the framing (byte counts) and the service names are checked; the recorded device bytes are served.
    Lenient,
}

/// A replay failure (QA §3.5.4).
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("conn {conn}: unexpected client bytes at offset {offset}: expected {expected}, got {actual}")]
    UnexpectedClientBytes { conn: u64, offset: u64, expected: String, actual: String },
    #[error("no recorded connection for service {0}")]
    UnknownService(String),
    #[error("{0} recorded connections were never opened")]
    Unconsumed(usize),
    #[error("trace format: {0}")]
    Format(String),
}

const STREAM_BUFFER: usize = 64 * 1024;

/// A connector that plays a recorded trace.
#[derive(Clone)]
pub struct ReplayConnector {
    inner: Arc<Inner>,
}

struct Inner {
    trace: Trace,
    mode: ReplayMode,
    /// Recorded connections, keyed by their id, with the ordered data records and whether they were opened.
    conns: BTreeMap<u64, Conn>,
    state: Mutex<State>,
}

struct Conn {
    via: Via,
    service: String,
    data: Vec<(Dir, Vec<u8>)>,
}

#[derive(Default)]
struct State {
    /// Which recorded `call` records were consumed, by index.
    calls_used: Vec<bool>,
    /// Which recorded connections were opened, by id.
    opened: BTreeMap<u64, bool>,
    mismatches: Vec<String>,
}

impl std::fmt::Debug for ReplayConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayConnector").field("scenario", &self.inner.trace.header.scenario).finish_non_exhaustive()
    }
}

impl ReplayConnector {
    /// Builds a connector from an already-parsed trace.
    pub fn from_trace(trace: Trace, mode: ReplayMode) -> Self {
        let mut conns: BTreeMap<u64, Conn> = BTreeMap::new();
        for rec in &trace.records {
            match rec {
                Record::Conn { id, via: Some(via), service: Some(service), open: true, .. } => {
                    conns.entry(*id).or_insert_with(|| Conn { via: *via, service: service.clone(), data: Vec::new() });
                }
                Record::Data { conn, dir, b64, .. } => {
                    if let Some(c) = conns.get_mut(conn)
                        && let Ok(bytes) = base64_decode(b64)
                    {
                        c.data.push((*dir, bytes));
                    }
                }
                _ => {}
            }
        }
        let calls_used = vec![false; trace.records.iter().filter(|r| matches!(r, Record::Call { .. })).count()];
        let opened = conns.keys().map(|id| (*id, false)).collect();
        let state = State { calls_used, opened, ..State::default() };
        Self { inner: Arc::new(Inner { trace, mode, conns, state: Mutex::new(state) }) }
    }

    /// Opens a trace file and builds a connector.
    pub fn open(trace: &std::path::Path, mode: ReplayMode) -> Result<Self> {
        Ok(Self::from_trace(Trace::read(trace)?, mode))
    }

    /// The connector for a `RUSSET_DEVICE_BACKEND` value: `Some` for `replay:<trace>`, `None` otherwise.
    pub fn from_backend_spec(spec: &str, mode: ReplayMode) -> Result<Option<Self>> {
        match spec.strip_prefix("replay:") {
            Some(path) => Ok(Some(Self::open(std::path::Path::new(path), mode)?)),
            None => Ok(None),
        }
    }

    /// The trace being replayed.
    pub fn trace(&self) -> &Trace {
        &self.inner.trace
    }

    /// `Ok` when every recorded connection was opened and no stream mismatched (QA §3.5.4).
    pub fn assert_consumed(&self) -> std::result::Result<(), ReplayError> {
        let st = self.lock();
        if !st.mismatches.is_empty() {
            return Err(ReplayError::Format(st.mismatches.join("; ")));
        }
        let never = st.opened.values().filter(|opened| !**opened).count();
        if never > 0 {
            return Err(ReplayError::Unconsumed(never));
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Inner {
    /// The next unconsumed recorded call matching `api` (and arguments per the mode); returns its record index.
    fn match_call<'a>(&'a self, st: &mut State, api: &str, args: &Value) -> Option<(usize, &'a Record)> {
        let mut idx = 0;
        for rec in &self.trace.records {
            if let Record::Call { api: rec_api, args: rec_args, .. } = rec {
                if !st.calls_used[idx] && rec_api == api && self.args_match(rec_args, args) {
                    st.calls_used[idx] = true;
                    return Some((idx, rec));
                }
                idx += 1;
            }
        }
        None
    }

    fn args_match(&self, recorded: &Value, incoming: &Value) -> bool {
        match self.mode {
            ReplayMode::Lenient => true,
            ReplayMode::Strict => recorded == incoming,
            ReplayMode::Semantic => {
                recorded.as_object().is_none_or(|r| r.iter().all(|(k, v)| incoming.get(k) == Some(v)))
            }
        }
    }
}

#[async_trait]
impl ServiceConnector for ReplayConnector {
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        let summary = self.inner.trace.device_summary();
        Ok(stream::iter(vec![DeviceEvent::Added(summary)]).chain(stream::pending()).boxed())
    }

    async fn call(&self, call: FacadeCall) -> Result<Value> {
        let FacadeCall { api, args } = call;
        // Discovery is synthesized from the header so `DeviceManager::start` finds the device at once.
        if api == facade::MUX_LIST {
            let d = &self.inner.trace.header.device;
            return Ok(json!([{ "udid": d.udid, "deviceId": 1, "transport": d.transport }]));
        }
        let mut st = self.lock();
        match self.inner.match_call(&mut st, &api, &args) {
            Some((_, Record::Call { result, error, .. })) => {
                if let Some(e) = error {
                    let code = code_by_name(&e.code)?;
                    Err(Error::new(code, e.message.clone().unwrap_or_else(|| format!("recorded error of {api}"))))
                } else {
                    Ok(result.clone().unwrap_or(Value::Null))
                }
            }
            _ => Err(Error::new(
                codes::device::SERVICE_NOT_IMPLEMENTED,
                format!("the trace recorded no answer for {api}"),
            )
            .with_hint("re-record the scenario, or use a trace that covers this call")),
        }
    }

    async fn lockdown_service(&self, _udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.open_stream(Via::Lockdown, name)
    }

    async fn rsd_service(&self, _udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.open_stream(Via::Rsd, name)
    }
}

impl ReplayConnector {
    fn open_stream(&self, via: Via, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        let id = {
            let mut st = self.lock();
            let found = self
                .inner
                .conns
                .iter()
                .find(|(id, c)| c.via == via && c.service == name && st.opened.get(id) == Some(&false))
                .map(|(id, _)| *id);
            match found {
                Some(id) => {
                    st.opened.insert(id, true);
                    id
                }
                None => {
                    return Err(Error::new(
                        codes::device::SERVICE_UNAVAILABLE,
                        format!("the trace has no recorded {via:?} stream for {name}"),
                    )
                    .with_hint("record the scenario with this service, or check the service name"));
                }
            }
        };
        let data = self.inner.conns[&id].data.clone();
        let (client, device_end) = tokio::io::duplex(STREAM_BUFFER);
        tokio::spawn(play_stream(Arc::clone(&self.inner), id, self.inner.mode, device_end, data));
        Ok(Box::new(client))
    }
}

/// Plays one recorded connection's data on the device end.
async fn play_stream(inner: Arc<Inner>, conn: u64, mode: ReplayMode, mut io: DuplexStream, data: Vec<(Dir, Vec<u8>)>) {
    let mut offset = 0u64;
    for (dir, bytes) in data {
        match dir {
            Dir::D2c => {
                if io.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            Dir::C2d => {
                let mut got = vec![0u8; bytes.len()];
                if io.read_exact(&mut got).await.is_err() {
                    return; // the client closed early; not a mismatch.
                }
                if mode != ReplayMode::Lenient && got != bytes {
                    let mut st = inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    st.mismatches.push(format!(
                        "conn {conn} at offset {offset}: expected {}, got {}",
                        show(&bytes),
                        show(&got)
                    ));
                    return;
                }
                offset += bytes.len() as u64;
            }
        }
    }
    let _ = io.shutdown().await;
}

fn show(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(48)]).into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use russet_device::io::ServiceConnector;
    use russet_device::{DeviceConfig, DeviceManager, DeviceSelector};

    use super::*;
    use crate::rdt::{Device, Header, Redaction, VERSION};
    use crate::recording::RecordingConnector;
    use crate::scenario::Scenario;
    use crate::scripted::ScriptedConnector;

    const UDID: &str = "00000000-0000000000000001";

    fn header() -> Header {
        Header {
            rdt: VERSION,
            scenario: "device-info".into(),
            recorded_at: "2026-10-11T00:00:00Z".into(),
            russet: "0.1.0".into(),
            idevice: "0.1.68".into(),
            host_os: "linux".into(),
            device: Device {
                product_type: "iPhone13,2".into(),
                product_version: "27.0".into(),
                build_version: "24A437".into(),
                csm: "txm".into(),
                udid: UDID.into(),
                transport: "usb".into(),
            },
            redaction: Redaction::default(),
        }
    }

    async fn device_info(conn: Arc<dyn ServiceConnector>, tmp: &std::path::Path) -> russet_device::DeviceInfo {
        let cfg = DeviceConfig { state_dir: tmp.join("state"), runtime_dir: tmp.join("runtime"), ..Default::default() };
        let m = DeviceManager::start(cfg, conn).await.expect("device manager");
        for _ in 0..500 {
            if let Ok(h) = m.get(&DeviceSelector::Only) {
                return h.info().await.expect("device info");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the device never appeared");
    }

    /// QA-06 acceptance, as a lib unit test so it runs on Linux and Windows CI (the Windows profile runs `--lib`
    /// only): record `device info` against the scripted iPhone, replay the trace, and get the same identity.
    #[tokio::test]
    async fn record_then_replay_device_info_cross_platform() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let scripted = ScriptedConnector::new(Scenario::default_scenario());
        let recorder = Arc::new(RecordingConnector::new(scripted, header()));
        let recorded = device_info(recorder.clone() as Arc<dyn ServiceConnector>, &tmp.path().join("rec")).await;

        let trace_path = tmp.path().join("device-info.rdt.jsonl");
        recorder.write(&trace_path).expect("write trace");
        assert!(crate::redaction::lint(&recorder.trace()).is_empty(), "the trace is redaction-clean");

        let replay = ReplayConnector::open(&trace_path, ReplayMode::Semantic).expect("open replay");
        let replayed = device_info(Arc::new(replay) as Arc<dyn ServiceConnector>, &tmp.path().join("rep")).await;

        assert_eq!(recorded.identity.product_type.as_deref(), Some("iPhone13,2"));
        assert_eq!(recorded.identity, replayed.identity, "identity replays identically");
        assert_eq!(recorded.properties, replayed.properties, "properties replay identically");
        assert_eq!(replayed.udid.0, UDID);
    }
}
