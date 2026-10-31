// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A fake iOS debugserver (RD V5/V6, H6), ported from `docs/design/run-debug-prototypes/fake_ios_stub.py`.
//!
//! It behaves like the debugserver that `com.apple.internal.dt.remote.debugproxy` starts per connection: it
//! begins without a process and answers `vAttach;<pid>` for its one fake process, which is a stopped arm64 iOS
//! process whose main image is any Mach-O (normally [`super::macho::synthetic_ios_executable`]) mapped at its
//! unslid address. Nothing executes: memory is served from the file plus an overlay of written bytes, and each
//! resume plays the next scripted [`ResumeStep`]. It records every packet, `QSetDetachOnError`, breakpoints,
//! memory writes and what happened to the process when the connection was lost (debugserver's documented
//! behaviour: detach with `QSetDetachOnError:1`, kill otherwise).
//!
//! It serves TCP on `127.0.0.1:0`, one connection at a time, with std threads only.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use super::json::Json;
use super::macho::{self, MachOInfo};

/// Darwin's SIGSTOP (the signal of a fresh attach).
pub const SIGSTOP: u8 = 0x11;
/// SIGTRAP.
pub const SIGTRAP: u8 = 0x05;
/// debugserver's arm64 pc register number.
pub const REG_PC: u32 = 32;
pub const REG_LR: u32 = 30;
pub const REG_SP: u32 = 31;

const STACK_BASE: u64 = 0x1_6F00_0000;
const STACK_SIZE: u64 = 0x10_0000;
const HEAP_BASE: u64 = 0x2_8000_0000;

/// The fake process's main image.
#[derive(Debug, Clone)]
pub struct FakeImage {
    /// Where the app lives on the "device", for example
    /// `/private/var/containers/Bundle/Application/<UUID>/RussetProbe.app/RussetProbe`.
    pub device_path: String,
    pub macho: Vec<u8>,
}

/// What a resume does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeStep {
    /// Stop with this signal and reason at `pc`, with extra expedited registers and an optional Mach exception.
    Stop { signal: u8, reason: String, pc: u64, regs: Vec<(u32, u64)>, exception: Option<(u32, Vec<u64>)> },
    /// Write inferior output (an `O` packet), then play the next step.
    Output(Vec<u8>),
    /// The process exits with this status (`W`).
    Exit(u8),
    /// Keep running until the client interrupts (`0x03` → `T11`).
    Run,
}

impl ResumeStep {
    /// A software breakpoint hit at `pc`.
    pub fn breakpoint(pc: u64) -> Self {
        Self::Stop {
            signal: SIGTRAP,
            reason: "breakpoint".to_owned(),
            pc,
            regs: Vec::new(),
            exception: Some((6, vec![1, 0])),
        }
    }

    /// An `EXC_BAD_ACCESS` at `pc` for `address`.
    pub fn bad_access(pc: u64, address: u64) -> Self {
        Self::Stop {
            signal: 0x0b,
            reason: "exception".to_owned(),
            pc,
            regs: Vec::new(),
            exception: Some((1, vec![1, address])),
        }
    }
}

/// The fake's configuration.
#[derive(Debug, Clone)]
pub struct FakeSpec {
    pub pid: u32,
    pub tid: u64,
    pub image: Option<FakeImage>,
    /// The pc of the attach stop (default: the image's entry point).
    pub pc: Option<u64>,
    /// Played in order, one per resume; when they run out, the process keeps running ([`ResumeStep::Run`]).
    pub steps: Vec<ResumeStep>,
}

impl Default for FakeSpec {
    fn default() -> Self {
        Self { pid: 932, tid: 0x1a2b, image: None, pc: None, steps: Vec::new() }
    }
}

/// What happened to the fake process when a connection ended without `D` or `k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostOutcome {
    Detached,
    Killed,
}

/// The fake process's life so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FakeReport {
    pub connections: usize,
    pub attached: bool,
    pub attach_count: usize,
    pub detached: bool,
    pub killed: bool,
    pub exited: Option<u8>,
    /// The last `QSetDetachOnError` value.
    pub detach_on_error: Option<u8>,
    pub lost: Option<LostOutcome>,
    pub breakpoints: BTreeSet<u64>,
    /// Every `M` write: `(address, length)`.
    pub writes: Vec<(u64, usize)>,
    /// Every packet received, decoded and lossily converted to text (binary payloads are hex anyway).
    pub transcript: Vec<String>,
}

impl FakeReport {
    /// Whether a packet starting with `prefix` was received.
    pub fn saw(&self, prefix: &str) -> bool {
        self.transcript.iter().any(|p| p.starts_with(prefix))
    }

    /// Index of the first packet starting with `prefix`.
    pub fn position(&self, prefix: &str) -> Option<usize> {
        self.transcript.iter().position(|p| p.starts_with(prefix))
    }
}

struct State {
    spec: FakeSpec,
    image: Option<(FakeImage, MachOInfo)>,
    regs: [u64; 34],
    overlay: BTreeMap<u64, u8>,
    heap: Vec<(u64, u64)>,
    heap_next: u64,
    steps: VecDeque<ResumeStep>,
    last_stop: String,
    report: FakeReport,
}

/// A running fake debugserver.
#[derive(Debug)]
pub struct FakeDebugserver {
    addr: SocketAddr,
    state: Arc<Mutex<StateCell>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

/// Wrapper so `State` can stay private while `Debug` works.
struct StateCell(State);

impl std::fmt::Debug for StateCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeState").field("report", &self.0.report).finish_non_exhaustive()
    }
}

impl FakeDebugserver {
    /// Starts listening on `127.0.0.1:0`.
    pub fn start(spec: FakeSpec) -> io::Result<Self> {
        let image = match &spec.image {
            Some(img) => Some((img.clone(), macho::parse(&img.macho)?)),
            None => None,
        };
        let entry = image.as_ref().and_then(|(_, info)| {
            let text = info.segments.iter().find(|s| s.name == "__TEXT")?;
            Some(text.vmaddr + info.entryoff?)
        });
        let mut regs = [0u64; 34];
        regs[REG_SP as usize] = STACK_BASE + STACK_SIZE - 0x1000;
        regs[29] = regs[REG_SP as usize] + 0x40;
        regs[REG_PC as usize] = spec.pc.or(entry).unwrap_or(0x1_0000_1000);
        regs[REG_LR as usize] = regs[REG_PC as usize] + 0x100;
        regs[33] = 0x6000_0000;
        let steps = spec.steps.iter().cloned().collect();
        let state = State {
            spec,
            image,
            regs,
            overlay: BTreeMap::new(),
            heap: Vec::new(),
            heap_next: HEAP_BASE,
            steps,
            last_stop: String::new(),
            report: FakeReport::default(),
        };
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(StateCell(state)));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    // One client at a time, like debugserver.
                    serve(&state, conn, &stop);
                }
            })
        };
        Ok(Self { addr, state, stop, accept: Some(accept) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A snapshot of what happened so far.
    pub fn report(&self) -> FakeReport {
        lock(&self.state).0.report.clone()
    }

    /// The bytes at `addr` as the fake process sees them (file plus overlay).
    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        let st = lock(&self.state);
        (addr..addr + len as u64).map(|a| read_byte(&st.0, a).unwrap_or(0)).collect()
    }

    /// Waits until `pred` holds for the report, up to `limit`.
    pub fn wait_for(&self, limit: Duration, pred: impl Fn(&FakeReport) -> bool) -> bool {
        let deadline = std::time::Instant::now() + limit;
        loop {
            if pred(&self.report()) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for FakeDebugserver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock accept().
        let _ = TcpStream::connect(self.addr);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
    }
}

fn lock(m: &Mutex<StateCell>) -> MutexGuard<'_, StateCell> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn checksum(p: &[u8]) -> u8 {
    p.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

fn frame(payload: &str) -> Vec<u8> {
    let mut esc = Vec::with_capacity(payload.len() + 4);
    esc.push(b'$');
    let start = esc.len();
    for &b in payload.as_bytes() {
        if matches!(b, b'#' | b'$' | b'}' | b'*') {
            esc.push(b'}');
            esc.push(b ^ 0x20);
        } else {
            esc.push(b);
        }
    }
    let cs = checksum(&esc[start..]);
    esc.extend_from_slice(format!("#{cs:02x}").as_bytes());
    esc
}

fn unescape(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        if p[i] == b'}' && i + 1 < p.len() {
            out.push(p[i + 1] ^ 0x20);
            i += 2;
        } else {
            out.push(p[i]);
            i += 1;
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

fn le(v: u64, size: usize) -> String {
    hex(&v.to_le_bytes()[..size])
}

fn serve(state: &Arc<Mutex<StateCell>>, mut conn: TcpStream, stop: &AtomicBool) {
    let _ = conn.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = conn.set_nodelay(true);
    let mut ack = true;
    let mut buf: Vec<u8> = Vec::new();
    let mut running = false;
    let mut ended = false;
    {
        let mut st = lock(state);
        st.0.report.connections += 1;
    }
    let mut chunk = [0u8; 65536];
    while !ended && !stop.load(Ordering::SeqCst) {
        match conn.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => continue,
            Err(_) => break,
        }
        loop {
            while matches!(buf.first(), Some(b'+' | b'-')) {
                buf.remove(0);
            }
            match buf.first() {
                None => break,
                Some(0x03) => {
                    buf.remove(0);
                    let reply = {
                        let mut st = lock(state);
                        st.0.report.transcript.push("\u{3}".to_owned());
                        if running {
                            running = false;
                            Some(stop_reply(&mut st.0, SIGSTOP, "signal", None, &[], None))
                        } else {
                            None
                        }
                    };
                    if let Some(r) = reply {
                        let _ = conn.write_all(&frame(&r));
                    }
                    continue;
                }
                Some(b'$') => {}
                Some(_) => {
                    buf.remove(0);
                    continue;
                }
            }
            let Some(hash) = buf.iter().position(|&b| b == b'#') else { break };
            if buf.len() < hash + 3 {
                break;
            }
            let payload = unescape(&buf[1..hash]);
            buf.drain(..hash + 3);
            if ack {
                let _ = conn.write_all(b"+");
            }
            let text = String::from_utf8_lossy(&payload).into_owned();
            let (replies, outcome) = {
                let mut st = lock(state);
                st.0.report.transcript.push(text.clone());
                handle(&mut st.0, &text, &mut running)
            };
            for r in &replies {
                let _ = conn.write_all(&frame(r));
            }
            if text == "QStartNoAckMode" {
                ack = false;
            }
            if outcome {
                ended = true;
                let _ = conn.flush();
                let _ = conn.shutdown(Shutdown::Both);
                break;
            }
        }
    }
    if !ended {
        // Lost connection: debugserver detaches with QSetDetachOnError:1 and kills otherwise.
        let mut st = lock(state);
        let report = &mut st.0.report;
        if report.attached && !report.detached && !report.killed && report.exited.is_none() {
            let outcome = if report.detach_on_error == Some(1) { LostOutcome::Detached } else { LostOutcome::Killed };
            report.lost = Some(outcome);
            report.attached = false;
        }
    }
}

/// Handles one packet; returns the replies to send and whether the connection ends after them.
fn handle(st: &mut State, p: &str, running: &mut bool) -> (Vec<String>, bool) {
    let one = |s: &str| (vec![s.to_owned()], false);
    let attached = st.report.attached;
    if p == "QStartNoAckMode" {
        return one("OK");
    }
    if p.starts_with("qSupported") {
        return one("PacketSize=20000;qEcho+;QThreadSuffixSupported+;QListThreadsInStopReply+;qXfer:features:read-;\
             SupportedCompressions=lzfse,zlib-deflate,lz4,lzma");
    }
    if let Some(v) = p.strip_prefix("QSetDetachOnError:") {
        st.report.detach_on_error = v.parse().ok();
        return one("OK");
    }
    if matches!(p, "QThreadSuffixSupported" | "QListThreadsInStopReply" | "QEnableErrorStrings")
        || ["QSetSTDIO", "QSetWorkingDir", "QEnvironment", "QSetDisableASLR", "QLaunchArch", "QSetMaxPacketSize"]
            .iter()
            .any(|q| p.starts_with(q))
    {
        return one("OK");
    }
    if p == "qHostInfo" {
        return one(
            "cputype:16777228;cpusubtype:2;addressing_bits:47;ostype:ios;watchpoint_exceptions_received:before;\
             vendor:apple;os_version:27.0.0;os_build:24A437;endian:little;ptrsize:8;vm-page-size:16384;",
        );
    }
    if let Some(rest) = p.strip_prefix("qEcho:") {
        return one(&format!("qEcho:{rest}"));
    }
    if let Some(rest) = p.strip_prefix("vAttach;") {
        let pid = u32::from_str_radix(rest, 16).unwrap_or(0);
        if attached {
            return one(&format!("E37;{}", hex(b"debugserver is already attached")));
        }
        if pid != st.spec.pid || st.report.killed || st.report.exited.is_some() {
            return one(&format!("E01;{}", hex(b"no such process")));
        }
        st.report.attached = true;
        st.report.attach_count += 1;
        let reply = stop_reply(st, SIGSTOP, "signal", None, &[], None);
        return (vec![reply], false);
    }
    if p.starts_with("vRun") || (p.starts_with('A') && p.len() > 1 && p.as_bytes()[1].is_ascii_digit()) {
        // debugserver cannot spawn apps on iOS 17+ [VF].
        return one(&format!("E59;{}", hex(b"Operation not permitted")));
    }
    if !attached {
        return match p {
            "?" => one("W00"),
            "qProcessInfo" | "qC" => one("E44"),
            _ => one(""),
        };
    }
    if p == "qProcessInfo" {
        return one(&format!(
            "pid:{:x};parent-pid:1;real-uid:1f5;real-gid:1f5;effective-uid:1f5;effective-gid:1f5;cputype:100000c;\
             cpusubtype:2;ptrsize:8;ostype:ios;vendor:apple;endian:little;",
            st.spec.pid
        ));
    }
    if p == "qC" {
        return one(&format!("QC{:x}", st.spec.tid));
    }
    if p == "?" || p.starts_with("qThreadStopInfo") {
        let s = st.last_stop.clone();
        return one(&s);
    }
    if p == "qfThreadInfo" {
        return one(&format!("m{:x}", st.spec.tid));
    }
    if p == "qsThreadInfo" {
        return one("l");
    }
    if let Some(n) = p.strip_prefix("qRegisterInfo") {
        return one(&register_info(u32::from_str_radix(n, 16).unwrap_or(u32::MAX)));
    }
    if let Some(rest) = p.strip_prefix('p')
        && let Ok(n) = u32::from_str_radix(rest.split(';').next().unwrap_or_default(), 16)
    {
        return one(&if n < 34 { le(st.regs[n as usize], if n == 33 { 4 } else { 8 }) } else { "E45".to_owned() });
    }
    if let Some(rest) = p.strip_prefix('P')
        && let Some((n, v)) = rest.split(';').next().unwrap_or_default().split_once('=')
    {
        let (Ok(n), Some(bytes)) = (u32::from_str_radix(n, 16), unhex(v)) else { return one("E46") };
        if n >= 34 {
            return one("E45");
        }
        let mut b = [0u8; 8];
        let len = bytes.len().min(8);
        b[..len].copy_from_slice(&bytes[..len]);
        st.regs[n as usize] = u64::from_le_bytes(b);
        return one("OK");
    }
    if p.starts_with("QSaveRegisterState") {
        return one("1");
    }
    if p.starts_with("QRestoreRegisterState") {
        return one("OK");
    }
    if let Some(rest) = p.strip_prefix('m')
        && let Some((a, l)) = rest.split_once(',')
    {
        let (Ok(addr), Ok(len)) = (u64::from_str_radix(a, 16), usize::from_str_radix(l, 16)) else { return one("E08") };
        let mut out = Vec::new();
        for a in addr..addr + len as u64 {
            match read_byte(st, a) {
                Some(b) => out.push(b),
                None => break,
            }
        }
        return one(&if out.is_empty() { "E08".to_owned() } else { hex(&out) });
    }
    if let Some(rest) = p.strip_prefix('M')
        && let Some((head, data)) = rest.split_once(':')
        && let Some((a, _)) = head.split_once(',')
    {
        let (Ok(addr), Some(bytes)) = (u64::from_str_radix(a, 16), unhex(data)) else { return one("E09") };
        for (i, b) in bytes.iter().enumerate() {
            st.overlay.insert(addr + i as u64, *b);
        }
        st.report.writes.push((addr, bytes.len()));
        return one("OK");
    }
    if let Some(addr) = p.strip_prefix("qMemoryRegionInfo:") {
        return one(&memory_region(st, u64::from_str_radix(addr, 16).unwrap_or(0)));
    }
    if let Some(rest) = p.strip_prefix("_M") {
        let size = rest.split(',').next().and_then(|s| u64::from_str_radix(s, 16).ok()).unwrap_or(0x4000);
        let size = (size + 0x3FFF) & !0x3FFF;
        let start = st.heap_next;
        st.heap.push((start, size));
        st.heap_next += size + 0x4000;
        return one(&format!("{start:x}"));
    }
    if p.starts_with("_m") {
        return one("OK");
    }
    if let Some(args) = p.strip_prefix("jGetLoadedDynamicLibrariesInfos:") {
        return one(&if args.is_empty() { "OK".to_owned() } else { images_json(st) });
    }
    if let Some(args) = p.strip_prefix("jGetSharedCacheInfo:") {
        if args.is_empty() {
            return one("OK");
        }
        return one(
            r#"{"shared_cache_base_address":0,"shared_cache_uuid":"00000000-0000-0000-0000-000000000000","no_shared_cache":true,"shared_cache_private_cache":false}"#,
        );
    }
    if p.starts_with("qWatchpointSupportInfo") {
        return one("num:4;");
    }
    if p == "vCont?" {
        return one("vCont;c;C;s;S");
    }
    if p.starts_with("vCont;") || p == "c" || p == "s" || (p.starts_with('C') && p.len() == 3) {
        return (resume(st, running), false);
    }
    // Breakpoints (Z0, Z1) and watchpoints (Z2–Z4), keyed by address.
    let point = |p: &str, lead: char| -> Option<u64> {
        let mut chars = p.chars();
        (chars.next() == Some(lead) && matches!(chars.next(), Some('0'..='4')) && chars.next() == Some(','))
            .then(|| p[3..].split(',').next().and_then(|a| u64::from_str_radix(a, 16).ok()))
            .flatten()
    };
    if let Some(addr) = point(p, 'Z') {
        st.report.breakpoints.insert(addr);
        return one("OK");
    }
    if let Some(addr) = point(p, 'z') {
        st.report.breakpoints.remove(&addr);
        return one("OK");
    }
    if p == "k" {
        st.report.killed = true;
        st.report.attached = false;
        return (vec!["X09".to_owned()], true);
    }
    if p == "D" || p.starts_with("D;") {
        st.report.detached = true;
        st.report.attached = false;
        return (vec!["OK".to_owned()], true);
    }
    one("")
}

fn resume(st: &mut State, running: &mut bool) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        match st.steps.pop_front().unwrap_or(ResumeStep::Run) {
            ResumeStep::Output(bytes) => out.push(format!("O{}", hex(&bytes))),
            ResumeStep::Exit(code) => {
                st.report.exited = Some(code);
                st.report.attached = false;
                out.push(format!("W{code:02x}"));
                return out;
            }
            ResumeStep::Run => {
                *running = true;
                return out;
            }
            ResumeStep::Stop { signal, reason, pc, regs, exception } => {
                let r = stop_reply(st, signal, &reason, Some(pc), &regs, exception);
                out.push(r);
                return out;
            }
        }
    }
}

fn stop_reply(
    st: &mut State,
    signal: u8,
    reason: &str,
    pc: Option<u64>,
    regs: &[(u32, u64)],
    exception: Option<(u32, Vec<u64>)>,
) -> String {
    if let Some(pc) = pc {
        st.regs[REG_PC as usize] = pc;
    }
    for (n, v) in regs {
        if (*n as usize) < st.regs.len() {
            st.regs[*n as usize] = *v;
        }
    }
    let tid = st.spec.tid;
    let mut s = format!("T{signal:02x}thread:{tid:x};threads:{tid:x};thread-pcs:{:x};", st.regs[REG_PC as usize]);
    for n in [0u32, 1, 29, 30, 31, 32] {
        s.push_str(&format!("{n:02x}:{};", le(st.regs[n as usize], 8)));
    }
    s.push_str(&format!("reason:{reason};"));
    if let Some((metype, medata)) = exception {
        s.push_str(&format!("metype:{metype:x};mecount:{:x};", medata.len()));
        for d in medata {
            s.push_str(&format!("medata:{d:x};"));
        }
    }
    st.last_stop = s.clone();
    s
}

fn register_info(n: u32) -> String {
    if n > 33 {
        return "E45".to_owned();
    }
    let (name, generic) = match n {
        29 => ("fp".to_owned(), "generic:fp;"),
        30 => ("lr".to_owned(), "generic:ra;"),
        31 => ("sp".to_owned(), "generic:sp;"),
        32 => ("pc".to_owned(), "generic:pc;"),
        33 => ("cpsr".to_owned(), "generic:flags;"),
        n if n < 8 => (
            format!("x{n}"),
            [
                "generic:arg1;",
                "generic:arg2;",
                "generic:arg3;",
                "generic:arg4;",
                "generic:arg5;",
                "generic:arg6;",
                "generic:arg7;",
                "generic:arg8;",
            ][n as usize],
        ),
        n => (format!("x{n}"), ""),
    };
    let bits = if n == 33 { 32 } else { 64 };
    let alt = if n == 29 || n == 30 { format!("alt-name:x{n};") } else { String::new() };
    format!(
        "name:{name};{alt}bitsize:{bits};offset:{};encoding:uint;format:hex;set:General Purpose Registers;ehframe:{n};dwarf:{n};{generic}",
        n * 8
    )
}

fn read_byte(st: &State, addr: u64) -> Option<u8> {
    if let Some(b) = st.overlay.get(&addr) {
        return Some(*b);
    }
    if let Some((img, info)) = &st.image {
        for s in &info.segments {
            if s.name != "__PAGEZERO" && s.vmaddr <= addr && addr < s.vmaddr + s.vmsize {
                let rel = addr - s.vmaddr;
                if rel < s.filesize {
                    return img.macho.get((s.fileoff + rel) as usize).copied();
                }
                return Some(0);
            }
        }
    }
    if (STACK_BASE..STACK_BASE + STACK_SIZE).contains(&addr) {
        return Some(0);
    }
    st.heap.iter().any(|&(a, n)| (a..a + n).contains(&addr)).then_some(0)
}

fn memory_region(st: &State, addr: u64) -> String {
    let mut regions: Vec<(u64, u64, &str)> = Vec::new();
    if let Some((_, info)) = &st.image {
        for s in info.segments.iter().filter(|s| s.name != "__PAGEZERO") {
            let perm = match s.maxprot {
                5 => "rx",
                3 => "rw",
                7 => "rwx",
                _ => "r",
            };
            regions.push((s.vmaddr, s.vmsize, perm));
        }
    }
    regions.push((STACK_BASE, STACK_SIZE, "rw"));
    regions.extend(st.heap.iter().map(|&(a, n)| (a, n, "rwx")));
    regions.sort_unstable();
    for &(start, size, perm) in &regions {
        if (start..start + size).contains(&addr) {
            return format!("start:{start:x};size:{size:x};permissions:{perm};");
        }
    }
    let end = regions.iter().map(|r| r.0).filter(|&s| s > addr).min().unwrap_or(u64::MAX);
    format!("start:{addr:x};size:{:x};", end - addr)
}

fn images_json(st: &State) -> String {
    let Some((img, info)) = &st.image else {
        return r#"{"images":[]}"#.to_owned();
    };
    let load = info.segments.iter().find(|s| s.name == "__TEXT").map(|s| s.vmaddr).unwrap_or(0);
    let segments = info
        .segments
        .iter()
        .map(|s| {
            Json::object([
                ("name", Json::str(s.name.clone())),
                ("vmaddr", Json::from(s.vmaddr)),
                ("vmsize", Json::from(s.vmsize)),
                ("fileoff", Json::from(s.fileoff)),
                ("filesize", Json::from(s.filesize)),
                ("maxprot", Json::from(u64::from(s.maxprot))),
            ])
        })
        .collect();
    let header = Json::object([
        ("magic", Json::from(u64::from(macho::MH_MAGIC_64))),
        ("cputype", Json::from(u64::from(info.cputype))),
        ("cpusubtype", Json::from(u64::from(info.cpusubtype))),
        ("filetype", Json::from(u64::from(info.filetype))),
        ("flags", Json::from(u64::from(info.flags))),
    ]);
    let image = Json::object([
        ("load_address", Json::from(load)),
        ("mod_date", Json::from(0u64)),
        ("pathname", Json::str(img.device_path.clone())),
        ("uuid", info.uuid.clone().map(Json::str).unwrap_or(Json::Null)),
        ("min_version_os_name", Json::str("iphoneos")),
        ("min_version_os_sdk", Json::str("17.0")),
        ("mach_header", header),
        ("segments", Json::Array(segments)),
    ]);
    Json::object([("images", Json::Array(vec![image]))]).to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// A minimal ack-mode client, enough to drive the fake without the real one.
    struct Client {
        s: TcpStream,
        buf: Vec<u8>,
    }

    impl Client {
        fn connect(addr: SocketAddr) -> Self {
            let s = TcpStream::connect(addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            Self { s, buf: Vec::new() }
        }
        fn send(&mut self, p: &str) {
            self.s.write_all(&frame(p)).unwrap();
        }
        fn recv(&mut self) -> String {
            loop {
                while matches!(self.buf.first(), Some(b'+')) {
                    self.buf.remove(0);
                }
                if let Some(h) = self.buf.iter().position(|&b| b == b'#')
                    && self.buf.len() >= h + 3
                {
                    let p = unescape(&self.buf[1..h]);
                    self.buf.drain(..h + 3);
                    return String::from_utf8(p).unwrap();
                }
                let mut chunk = [0u8; 4096];
                let n = self.s.read(&mut chunk).unwrap();
                assert!(n > 0, "closed");
                self.buf.extend_from_slice(&chunk[..n]);
            }
        }
        fn req(&mut self, p: &str) -> String {
            self.send(p);
            self.recv()
        }
    }

    fn spec() -> FakeSpec {
        FakeSpec {
            image: Some(FakeImage {
                device_path: "/private/var/containers/Bundle/Application/X/Probe.app/Probe".into(),
                macho: macho::synthetic_ios_executable([7; 16]),
            }),
            steps: vec![
                ResumeStep::Output(b"hi\n".to_vec()),
                ResumeStep::breakpoint(0x1_0000_1004),
                ResumeStep::Exit(3),
            ],
            ..FakeSpec::default()
        }
    }

    #[test]
    fn attach_inspect_resume_and_exit() {
        let fake = FakeDebugserver::start(spec()).unwrap();
        let mut c = Client::connect(fake.addr());
        assert_eq!(c.req("QStartNoAckMode"), "OK");
        assert_eq!(c.req("?"), "W00", "no process before vAttach");
        assert!(c.req("vRun;2f62696e").starts_with("E59"));
        assert!(c.req("vAttach;1").starts_with("E01"));
        let stop = c.req(&format!("vAttach;{:x}", 932));
        assert!(stop.starts_with("T11thread:1a2b;"), "{stop}");
        assert!(stop.contains("thread-pcs:100001000;"), "{stop}");
        assert_eq!(c.req("QSetDetachOnError:0"), "OK");
        let images = Json::parse(&c.req("jGetLoadedDynamicLibrariesInfos:{\"fetch_all_solibs\":true}")).unwrap();
        let img = images.get("images").at(0);
        assert_eq!(img.get("load_address").as_u64(), Some(0x1_0000_0000));
        assert_eq!(img.get("uuid").as_str(), Some("07070707-0707-0707-0707-070707070707"));
        // The Mach-O header is readable at the load address.
        assert_eq!(c.req("m100000000,4"), "cffaedfe");
        assert_eq!(c.req("M100000000,2:0000"), "OK");
        assert_eq!(fake.read(0x1_0000_0000, 4), vec![0, 0, 0xed, 0xfe]);
        assert!(c.req("qMemoryRegionInfo:100000000").contains("permissions:rx"));
        assert_eq!(c.req("Z0,100001004,4"), "OK");
        c.send("vCont;c");
        assert_eq!(c.recv(), format!("O{}", hex(b"hi\n")));
        let bp = c.recv();
        assert!(bp.starts_with("T05") && bp.contains("reason:breakpoint;") && bp.contains("metype:6;"), "{bp}");
        assert_eq!(c.req("p20;thread:1a2b;"), le(0x1_0000_1004, 8));
        assert_eq!(c.req("vCont;c"), "W03");
        let report = fake.report();
        assert_eq!(report.exited, Some(3));
        assert_eq!(report.detach_on_error, Some(0));
        assert!(report.breakpoints.contains(&0x1_0000_1004));
        assert_eq!(report.writes, vec![(0x1_0000_0000, 2)]);
        assert!(report.position("vAttach;3a4") > report.position("QStartNoAckMode"));
    }

    #[test]
    fn a_lost_connection_kills_or_detaches_per_qsetdetachonerror() {
        for (value, expected) in [(0u8, LostOutcome::Killed), (1, LostOutcome::Detached)] {
            let fake = FakeDebugserver::start(spec()).unwrap();
            {
                let mut c = Client::connect(fake.addr());
                c.req(&format!("QSetDetachOnError:{value}"));
                c.req("vAttach;3a4");
            }
            assert!(fake.wait_for(Duration::from_secs(5), |r| r.lost.is_some()));
            assert_eq!(fake.report().lost, Some(expected));
        }
    }

    #[test]
    fn interrupt_stops_a_running_process_and_detach_ends_the_connection() {
        let fake = FakeDebugserver::start(FakeSpec { steps: vec![ResumeStep::Run], ..spec() }).unwrap();
        let mut c = Client::connect(fake.addr());
        c.req("QStartNoAckMode");
        c.req("vAttach;3a4");
        c.send("vCont;c");
        c.s.write_all(&[0x03]).unwrap();
        assert!(c.recv().starts_with("T11"));
        assert_eq!(c.req("D"), "OK");
        assert!(fake.wait_for(Duration::from_secs(5), |r| r.detached && r.lost.is_none()));
    }
}
