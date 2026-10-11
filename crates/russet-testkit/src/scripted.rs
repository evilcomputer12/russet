// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`ScriptedConnector`]: core's FakeDeviceBackend [D 46], an `io::ServiceConnector` (testability contract T5)
//! played from a [`Scenario`] (CORE-15; PLAN W8).
//!
//! - **Attach and detach.** [`ServiceConnector::watch`] reports every attached device at once (as usbmuxd's
//!   `Listen` does), then the scenario's timeline (`attach-at-ms`, `detach-at-ms`, counted from the connector's
//!   creation) and the test's own [`Control::attach`] / [`Control::detach`]. A detach closes the device's open
//!   streams; later calls get `device.disconnected` (3007).
//! - **Facade calls.** [`ServiceConnector::call`] answers from the scenario's `[[call]]` entries, and
//!   `lockdown.get_value {key?, domain?}` from the device's properties. The device is `args.udid`, or the only
//!   attached device. Anything else is `device.serviceNotImplemented` (3013).
//! - **Service streams.** `lockdown_service` and `rsd_service` return in-memory plaintext streams (what the live
//!   connector returns after TLS or inside the tunnel) that play the scenario's `[[service]]` steps: send bytes,
//!   expect the client's bytes, wait, close. A wrong client byte is recorded as a mismatch
//!   ([`Control::mismatches`]) and closes the stream. Every device also serves `com.apple.syslog_relay` over
//!   lockdown: its `[[device.log]]` lines, then whatever [`Control::push_log`] adds, framed as idevice's
//!   `SyslogRelayClient` reads them (`\n\0`).
//! - **Failures.** `[[failure]]` entries and [`Control::fail_next`] make an operation fail with a given code,
//!   always or a number of times.
//! - **Recording.** [`Control::ops`] lists every operation in order, so a test can assert what a daemon asked.
//!
//! The scripted connector never touches usbmuxd, the network or a device: lane D's daemons use it instead of
//! `LiveConnector` (ARCHITECTURE §3.3 item 7).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use russet_core::error::{Error, ErrorCode, Result, codes};
use russet_device::io::{FacadeCall, ServiceConnector};
use russet_device::{DeviceEvent, DeviceSummary, Udid};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, watch};

use crate::scenario::{Action, Device, FailOp, Failure, Scenario, Via, code_by_name};

/// The lockdown service every scripted device serves.
pub const SYSLOG_RELAY: &str = "com.apple.syslog_relay";
/// The delimiter idevice's `SyslogRelayClient::next` reads up to.
pub const SYSLOG_DELIMITER: &[u8] = b"\n\0";
/// Buffer size of each in-memory stream.
const STREAM_BUFFER: usize = 64 * 1024;

/// One operation the connector was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Watch,
    Call { api: String, args: Value },
    Lockdown { udid: String, service: String },
    Rsd { udid: String, service: String },
}

/// The scripted device backend.
#[derive(Clone)]
pub struct ScriptedConnector {
    inner: Arc<Inner>,
}

/// A handle that drives and inspects a [`ScriptedConnector`] while the code under test uses it.
#[derive(Clone)]
pub struct Control {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ScriptedConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedConnector").field("scenario", &self.inner.scenario.name).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control").field("scenario", &self.inner.scenario.name).finish_non_exhaustive()
    }
}

struct Inner {
    scenario: Scenario,
    created: Instant,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Runtime attach (true) or detach (false) by [`Control`]; it beats the timeline.
    overrides: BTreeMap<String, bool>,
    /// Bumped on every runtime change of a device, so its streams re-check whether it is still attached.
    presence: BTreeMap<String, watch::Sender<u64>>,
    watchers: Vec<mpsc::UnboundedSender<DeviceEvent>>,
    failures: Vec<(Failure, Option<u32>)>,
    ops: Vec<Op>,
    mismatches: Vec<String>,
    log_subs: Vec<(String, mpsc::UnboundedSender<String>)>,
    streams: BTreeMap<u64, StreamState>,
    next_conn: u64,
}

#[derive(Debug, Clone)]
struct StreamState {
    desc: String,
    steps: usize,
    done: usize,
}

impl ScriptedConnector {
    /// A connector playing `scenario`; its timeline starts now.
    pub fn new(scenario: Scenario) -> Self {
        let failures = scenario.failures.iter().map(|f| (f.clone(), f.times)).collect();
        let presence = scenario.devices.iter().map(|d| (d.udid.clone(), watch::channel(0u64).0)).collect();
        let state = State { failures, presence, ..State::default() };
        Self { inner: Arc::new(Inner { scenario, created: Instant::now(), state: Mutex::new(state) }) }
    }

    /// A connector playing a scenario file.
    pub fn from_file(path: &Path) -> Result<Self> {
        Ok(Self::new(Scenario::from_file(path)?))
    }

    /// A connector playing the default lane-D scenario.
    pub fn default_scenario() -> Self {
        Self::new(Scenario::default_scenario())
    }

    /// The connector for a `RUSSET_DEVICE_BACKEND` value: `Some` for `scripted:<file>`, `None` for any other
    /// backend.
    pub fn from_backend_spec(spec: &str) -> Result<Option<Self>> {
        match spec.strip_prefix("scripted:") {
            Some(path) => Ok(Some(Self::from_file(Path::new(path))?)),
            None => Ok(None),
        }
    }

    /// The handle tests keep.
    pub fn control(&self) -> Control {
        Control { inner: Arc::clone(&self.inner) }
    }

    pub fn scenario(&self) -> &Scenario {
        &self.inner.scenario
    }
}

/// The device summary the connector reports. W4 owns `DeviceSummary`'s fields (ARCHITECTURE §9.5); this is the
/// only place the testkit builds one.
fn summary(d: &Device) -> DeviceSummary {
    DeviceSummary { udid: Udid(d.udid.clone()) }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.created.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn on_timeline(&self, d: &Device) -> bool {
        let now = self.elapsed_ms();
        now >= d.attach_at_ms && d.detach_at_ms.is_none_or(|t| now < t)
    }

    fn attached_in(&self, st: &State, d: &Device) -> bool {
        st.overrides.get(&d.udid).copied().unwrap_or_else(|| self.on_timeline(d))
    }

    fn is_attached(&self, udid: &str) -> bool {
        let st = self.lock();
        self.scenario.device(udid).is_some_and(|d| self.attached_in(&st, d))
    }

    fn record(&self, op: Op) {
        self.lock().ops.push(op);
    }

    /// The first matching scripted failure, consuming one of its `times`.
    fn check_failure(&self, op: FailOp, name: Option<&str>, udid: Option<&str>) -> Result<()> {
        let mut st = self.lock();
        for (f, remaining) in st.failures.iter_mut() {
            let matches = f.op == op
                && f.name.as_deref().is_none_or(|n| Some(n) == name)
                && f.udid.as_deref().is_none_or(|u| Some(u) == udid);
            if !matches || *remaining == Some(0) {
                continue;
            }
            if let Some(r) = remaining.as_mut() {
                *r -= 1;
            }
            let code = code_by_name(&f.code)?;
            let msg = f.message.clone().unwrap_or_else(|| format!("scripted failure of {op:?}"));
            return Err(Error::new(code, msg));
        }
        Ok(())
    }

    fn broadcast(&self, st: &mut State, ev: &DeviceEvent) {
        st.watchers.retain(|w| w.send(ev.clone()).is_ok());
    }

    fn set_presence(&self, udid: &str, attached: bool) -> bool {
        let Some(d) = self.scenario.device(udid) else { return false };
        let mut st = self.lock();
        let was = self.attached_in(&st, d);
        st.overrides.insert(udid.to_owned(), attached);
        if was != attached {
            let ev =
                if attached { DeviceEvent::Added(summary(d)) } else { DeviceEvent::Removed(Udid(udid.to_owned())) };
            self.broadcast(&mut st, &ev);
        }
        if let Some(p) = st.presence.get(udid) {
            p.send_modify(|g| *g += 1);
        }
        true
    }

    /// The milliseconds (since creation) at which the timeline detaches `udid`, if that is still ahead.
    fn timeline_detach(&self, udid: &str) -> Option<u64> {
        let d = self.scenario.device(udid)?;
        d.detach_at_ms.filter(|t| *t > self.elapsed_ms())
    }

    fn at(&self, ms: u64) -> tokio::time::Instant {
        tokio::time::Instant::from_std(self.created + Duration::from_millis(ms))
    }
}

/// Resolves when the device is no longer attached (a runtime detach or its timeline).
async fn device_gone(inner: Arc<Inner>, udid: String, mut presence: watch::Receiver<u64>) {
    let mut deadline = inner.timeline_detach(&udid);
    loop {
        if !inner.is_attached(&udid) {
            return;
        }
        let sleep_until = deadline.map(|ms| inner.at(ms));
        tokio::select! {
            changed = presence.changed() => {
                if changed.is_err() {
                    std::future::pending::<()>().await;
                }
            }
            () = async {
                match sleep_until {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            } => {
                deadline = None;
            }
        }
    }
}

#[async_trait]
impl ServiceConnector for ScriptedConnector {
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.open(Via::Lockdown, udid, name)
    }

    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        self.open(Via::Rsd, udid, name)
    }

    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>> {
        let inner = &self.inner;
        inner.record(Op::Watch);
        inner.check_failure(FailOp::Watch, None, None)?;
        let (tx, rx) = mpsc::unbounded_channel();
        let now = inner.elapsed_ms();
        let mut ahead: Vec<(u64, String, bool)> = Vec::new();
        {
            let mut st = inner.lock();
            for d in &inner.scenario.devices {
                if inner.attached_in(&st, d) {
                    let _ = tx.send(DeviceEvent::Added(summary(d)));
                }
                if !st.overrides.contains_key(&d.udid) {
                    if d.attach_at_ms > now {
                        ahead.push((d.attach_at_ms, d.udid.clone(), true));
                    }
                    if let Some(t) = d.detach_at_ms.filter(|t| *t > now) {
                        ahead.push((t, d.udid.clone(), false));
                    }
                }
            }
            st.watchers.push(tx.clone());
        }
        if !ahead.is_empty() {
            ahead.sort();
            let inner = Arc::clone(inner);
            tokio::spawn(async move {
                for (ms, udid, attach) in ahead {
                    tokio::time::sleep_until(inner.at(ms)).await;
                    if tx.is_closed() {
                        return;
                    }
                    if inner.lock().overrides.contains_key(&udid) {
                        continue;
                    }
                    let Some(d) = inner.scenario.device(&udid) else { continue };
                    let ev = if attach { DeviceEvent::Added(summary(d)) } else { DeviceEvent::Removed(Udid(udid)) };
                    let _ = tx.send(ev);
                }
            });
        }
        Ok(stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) }).boxed())
    }

    async fn call(&self, api: FacadeCall) -> Result<Value> {
        let inner = &self.inner;
        let FacadeCall { api, args } = api;
        inner.record(Op::Call { api: api.clone(), args: args.clone() });
        let explicit = args.get("udid").and_then(Value::as_str).map(str::to_owned);
        let udid = explicit.clone().or_else(|| {
            let st = inner.lock();
            let attached: Vec<&Device> = inner.scenario.devices.iter().filter(|d| inner.attached_in(&st, d)).collect();
            (attached.len() == 1).then(|| attached[0].udid.clone())
        });
        inner.check_failure(FailOp::Call, Some(&api), udid.as_deref())?;
        let device = match &udid {
            Some(u) => {
                let d = inner.scenario.device(u).ok_or_else(|| not_found(u))?;
                if !inner.is_attached(u) {
                    return Err(disconnected(u));
                }
                Some(d)
            }
            None => None,
        };
        for c in &inner.scenario.calls {
            if c.api != api || c.udid.as_deref().is_some_and(|u| Some(u) != udid.as_deref()) {
                continue;
            }
            if let Some(want) = &c.args {
                let want = toml_to_json(&toml::Value::Table(want.clone()));
                let all = want.as_object().is_none_or(|w| w.iter().all(|(k, v)| args.get(k) == Some(v)));
                if !all {
                    continue;
                }
            }
            if let Some(e) = &c.error {
                let code = code_by_name(&e.code)?;
                return Err(Error::new(code, e.message.clone().unwrap_or_else(|| format!("scripted error of {api}"))));
            }
            return Ok(c.result.as_ref().map(toml_to_json).unwrap_or(Value::Null));
        }
        if api == "lockdown.get_value" {
            let Some(d) = device else {
                return Err(Error::new(codes::device::NOT_FOUND, "lockdown.get_value needs a device")
                    .with_hint("pass args.udid, or attach exactly one device"));
            };
            return lockdown_value(d, &args);
        }
        Err(Error::new(codes::device::SERVICE_NOT_IMPLEMENTED, format!("the scenario scripts no answer for {api}"))
            .with_hint(format!("add a [[call]] entry with api = \"{api}\" to the scenario")))
    }
}

impl ScriptedConnector {
    fn open(&self, via: Via, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>> {
        let inner = &self.inner;
        let u = udid.0.clone();
        inner.record(match via {
            Via::Lockdown => Op::Lockdown { udid: u.clone(), service: name.to_owned() },
            Via::Rsd => Op::Rsd { udid: u.clone(), service: name.to_owned() },
        });
        let op = match via {
            Via::Lockdown => FailOp::LockdownService,
            Via::Rsd => FailOp::RsdService,
        };
        inner.check_failure(op, Some(name), Some(&u))?;
        let device = inner.scenario.device(&u).ok_or_else(|| not_found(&u))?;
        if !inner.is_attached(&u) {
            return Err(disconnected(&u));
        }
        let scripted = inner
            .scenario
            .services
            .iter()
            .find(|s| s.via == via && s.name == name && s.udid.as_deref().is_none_or(|x| x == u));
        let (actions, syslog) = match scripted {
            Some(s) => (s.steps.iter().filter_map(|st| st.action().ok()).collect::<Vec<_>>(), false),
            None if via == Via::Lockdown && name == SYSLOG_RELAY => (syslog_actions(device), true),
            None => {
                return Err(Error::new(
                    codes::device::SERVICE_UNAVAILABLE,
                    format!("the scenario scripts no {} service {name}", via_name(via)),
                )
                .with_hint("add a [[service]] entry to the scenario"));
            }
        };
        let (client, device_end) = tokio::io::duplex(STREAM_BUFFER);
        let (conn, presence, logs) = {
            let mut st = inner.lock();
            st.next_conn += 1;
            let conn = st.next_conn;
            let desc = format!("conn {conn} ({} {name} on {u})", via_name(via));
            st.streams.insert(conn, StreamState { desc, steps: actions.len(), done: 0 });
            let presence = st.presence.get(&u).map(watch::Sender::subscribe);
            let logs = syslog.then(|| {
                let (tx, rx) = mpsc::unbounded_channel();
                st.log_subs.push((u.clone(), tx));
                rx
            });
            (conn, presence, logs)
        };
        let Some(presence) = presence else { return Err(not_found(&u)) };
        tokio::spawn(run_stream(Arc::clone(inner), conn, u, device_end, actions, presence, logs));
        Ok(Box::new(client))
    }
}

fn via_name(v: Via) -> &'static str {
    match v {
        Via::Lockdown => "lockdown",
        Via::Rsd => "rsd",
    }
}

fn not_found(udid: &str) -> Error {
    Error::new(codes::device::NOT_FOUND, format!("no device {udid} in the scenario"))
}

fn disconnected(udid: &str) -> Error {
    Error::new(codes::device::DISCONNECTED, format!("device {udid} is not attached")).retryable(true)
}

/// The device's scripted syslog lines as stream actions.
fn syslog_actions(d: &Device) -> Vec<Action> {
    let mut out = Vec::new();
    for l in &d.logs {
        if l.at_ms > 0 {
            out.push(Action::Delay(l.at_ms));
        }
        for (i, line) in l.render(&d.name).into_iter().enumerate() {
            if i > 0 && l.every_ms > 0 {
                out.push(Action::Delay(l.every_ms));
            }
            out.push(Action::Send(syslog_frame(&line)));
        }
    }
    out
}

/// One syslog_relay message: the line, then `\n\0`.
pub fn syslog_frame(line: &str) -> Vec<u8> {
    let mut v = line.as_bytes().to_vec();
    v.extend_from_slice(SYSLOG_DELIMITER);
    v
}

fn lockdown_value(d: &Device, args: &Value) -> Result<Value> {
    let key = args.get("key").and_then(Value::as_str);
    let domain = args.get("domain").and_then(Value::as_str);
    let mut map = serde_json::Map::new();
    match domain {
        None => {
            map.insert("UniqueDeviceID".into(), json!(d.udid));
            map.insert("DeviceName".into(), json!(d.name));
            map.insert("ProductType".into(), json!(d.product_type));
            map.insert("ProductVersion".into(), json!(d.os_version));
            map.insert("BuildVersion".into(), json!(d.build));
            for (k, v) in &d.lockdown {
                map.insert(k.clone(), toml_to_json(v));
            }
        }
        Some("com.apple.security.mac.amfi") => {
            map.insert("DeveloperModeStatus".into(), json!(d.developer_mode));
        }
        Some(other) => {
            return Err(Error::new(codes::device::PROTOCOL, format!("the scenario has no lockdown domain {other}")));
        }
    }
    match key {
        None => Ok(Value::Object(map)),
        Some(k) => map
            .remove(k)
            .ok_or_else(|| Error::new(codes::device::PROTOCOL, format!("lockdown has no value {k} for {}", d.udid))),
    }
}

/// TOML values as JSON (dates become strings).
pub fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect()),
    }
}

/// Plays one stream's actions on the device end, then keeps it open (forwarding pushed syslog lines) until the
/// client closes it or the device goes away.
async fn run_stream(
    inner: Arc<Inner>,
    conn: u64,
    udid: String,
    mut io: DuplexStream,
    actions: Vec<Action>,
    presence: watch::Receiver<u64>,
    mut logs: Option<mpsc::UnboundedReceiver<String>>,
) {
    let gone = device_gone(Arc::clone(&inner), udid, presence);
    tokio::pin!(gone);
    for (i, action) in actions.iter().enumerate() {
        let step = run_action(&mut io, action);
        let outcome = tokio::select! {
            () = &mut gone => return,
            r = step => r,
        };
        match outcome {
            Ok(continue_stream) => {
                if let Some(s) = inner.lock().streams.get_mut(&conn) {
                    s.done = i + 1;
                }
                if !continue_stream {
                    return;
                }
            }
            Err(why) => {
                let mut st = inner.lock();
                let desc = st.streams.get(&conn).map(|s| s.desc.clone()).unwrap_or_default();
                st.mismatches.push(format!("{desc}, step {}: {why}", i + 1));
                return;
            }
        }
    }
    let mut buf = [0u8; 4096];
    loop {
        tokio::select! {
            () = &mut gone => return,
            line = async {
                match logs.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => match line {
                Some(l) => {
                    if io.write_all(&syslog_frame(&l)).await.is_err() {
                        return;
                    }
                }
                None => logs = None,
            },
            n = io.read(&mut buf) => {
                if !matches!(n, Ok(k) if k > 0) {
                    return;
                }
            }
        }
    }
}

/// Runs one action. `Ok(false)` closes the stream; `Err` is a mismatch or I/O failure.
async fn run_action(io: &mut DuplexStream, action: &Action) -> std::result::Result<bool, String> {
    match action {
        Action::Send(bytes) => io.write_all(bytes).await.map(|()| true).map_err(|e| format!("send failed: {e}")),
        Action::Expect(want) => {
            let mut got = vec![0u8; want.len()];
            match io.read_exact(&mut got).await {
                Ok(_) if &got == want => Ok(true),
                Ok(_) => Err(format!("expected {:?}, the client sent {:?}", show(want), show(&got))),
                Err(e) => Err(format!("expected {:?}, the client closed the stream ({e})", show(want))),
            }
        }
        Action::Delay(ms) => {
            tokio::time::sleep(Duration::from_millis(*ms)).await;
            Ok(true)
        }
        Action::Close => {
            let _ = io.shutdown().await;
            Ok(false)
        }
    }
}

fn show(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(&bytes[..bytes.len().min(80)]).into_owned();
    if bytes.len() > 80 { format!("{s}…") } else { s }
}

impl Control {
    /// Attaches a scenario device now (overrides its timeline). `false` for an unknown udid.
    pub fn attach(&self, udid: &str) -> bool {
        self.inner.set_presence(udid, true)
    }

    /// Detaches a scenario device now: watchers get `Removed`, its streams close. `false` for an unknown udid.
    pub fn detach(&self, udid: &str) -> bool {
        self.inner.set_presence(udid, false)
    }

    pub fn is_attached(&self, udid: &str) -> bool {
        self.inner.is_attached(udid)
    }

    /// Sends a syslog line on every open `com.apple.syslog_relay` stream of the device.
    pub fn push_log(&self, udid: &str, line: &str) -> usize {
        let mut st = self.inner.lock();
        st.log_subs.retain(|(_, tx)| !tx.is_closed());
        st.log_subs.iter().filter(|(u, _)| u == udid).filter(|(_, tx)| tx.send(line.to_owned()).is_ok()).count()
    }

    /// Makes the next `times` matching operations fail with `code`.
    pub fn fail_next(&self, op: FailOp, name: Option<&str>, code: ErrorCode, times: u32) {
        let f = Failure {
            op,
            name: name.map(str::to_owned),
            udid: None,
            code: code.as_str().to_owned(),
            message: Some(format!("injected failure ({})", code.as_str())),
            times: Some(times),
        };
        self.inner.lock().failures.insert(0, (f, Some(times)));
    }

    /// Every operation so far, in order.
    pub fn ops(&self) -> Vec<Op> {
        self.inner.lock().ops.clone()
    }

    /// The facade calls so far: `(api, args)`.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.ops()
            .into_iter()
            .filter_map(|op| match op {
                Op::Call { api, args } => Some((api, args)),
                _ => None,
            })
            .collect()
    }

    /// Client bytes that did not match a scripted `expect`, and streams that failed.
    pub fn mismatches(&self) -> Vec<String> {
        self.inner.lock().mismatches.clone()
    }

    /// `Ok` when every opened scripted stream ran all its steps and nothing mismatched.
    pub fn assert_consumed(&self) -> std::result::Result<(), String> {
        let st = self.inner.lock();
        let mut problems: Vec<String> = st.mismatches.clone();
        for s in st.streams.values() {
            if s.done < s.steps {
                problems.push(format!("{}: {} of {} steps ran", s.desc, s.done, s.steps));
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
    }

    pub fn scenario(&self) -> &Scenario {
        &self.inner.scenario
    }
}
