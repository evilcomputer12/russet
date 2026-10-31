// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Scenario files for the scripted device backend (`RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>`,
//! ARCHITECTURE §14.9; QA §3.5.5 "Scenario spec").
//!
//! A scenario describes synthetic devices and how they behave: when they attach and detach, what facade calls
//! answer, which service streams exist and what they say, which log lines they emit, and which operations fail.
//! [`crate::ScriptedConnector`] plays it. The format (schema 1, kebab-case keys):
//!
//! ```toml
//! schema = 1
//! name = "example"
//!
//! [[device]]
//! udid = "00000000-0000000000000001"   # chip id 0000: no Apple chip has it, so it can never be a real UDID
//! name = "Russet Test iPhone"
//! product-type = "iPhone13,2"
//! os-version = "27.0"
//! build = "24A437"
//! attach-at-ms = 0                      # after the connector is created (default 0)
//! detach-at-ms = 5000                   # optional
//!
//! [device.lockdown]                     # extra lockdown values (GetValue without a domain)
//! HardwareModel = "D53gAP"
//!
//! [[device.log]]                        # lines of the com.apple.syslog_relay stream
//! message = "PROBE seq={n}"             # {n} counts 1..=repeat
//! process = "RussetProbe"
//! repeat = 100
//! every-ms = 10
//!
//! [[call]]                              # a facade call (ServiceConnector::call)
//! api = "installation_proxy.browse"
//! result = [{ CFBundleIdentifier = "dev.russet.test.probe" }]
//!
//! [[service]]                           # a scripted plaintext service stream
//! name = "com.example.echo"
//! via = "lockdown"                      # or "rsd"
//! steps = [{ expect = "ping\n" }, { send = "pong\n" }, { close = true }]
//!
//! [[failure]]
//! op = "rsd-service"                    # watch | call | lockdown-service | rsd-service
//! name = "com.apple.coredevice.appservice"
//! code = "device.tunnelFailed"
//! times = 1                             # then it works (default: always fails)
//! ```
//!
//! Facade API names belong to the device plane (W4); the scenario answers `lockdown.get_value {key, domain?}`
//! from each device's properties by itself, and everything else from `[[call]]` entries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use russet_core::error::{Error, ErrorCode, Result, codes};
use serde::Deserialize;

/// The scenario format version this testkit reads.
pub const SCHEMA: u32 = 1;

/// The default lane-D scenario, embedded (the same bytes as [`default_scenario_path`]).
pub const DEFAULT_SCENARIO: &str = include_str!("../scenarios/default.toml");

/// `crates/russet-testkit/scenarios/default.toml`, the scenario lane-D worktrees export as
/// `RUSSET_DEVICE_BACKEND=scripted:<path>` (AGENTS.md §4).
pub fn default_scenario_path() -> PathBuf {
    crate::testkit_dir().join("scenarios/default.toml")
}

/// A whole scenario file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Scenario {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "device")]
    pub devices: Vec<Device>,
    #[serde(default, rename = "call")]
    pub calls: Vec<Call>,
    #[serde(default, rename = "service")]
    pub services: Vec<Service>,
    #[serde(default, rename = "failure")]
    pub failures: Vec<Failure>,
}

/// One synthetic device.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Device {
    pub udid: String,
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "default_product_type")]
    pub product_type: String,
    #[serde(default)]
    pub marketing_name: Option<String>,
    #[serde(default = "default_os_version")]
    pub os_version: String,
    #[serde(default = "default_build")]
    pub build: String,
    /// `usb` or `wifi`.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// `txm`, `ppl`, `none` or `unknown` (ARCHITECTURE §9.5).
    #[serde(default)]
    pub code_signing_monitor: Option<String>,
    #[serde(default = "yes")]
    pub paired: bool,
    #[serde(default = "yes")]
    pub developer_mode: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub ddi: Option<Ddi>,
    /// When the device appears, in milliseconds after the connector is created.
    #[serde(default)]
    pub attach_at_ms: u64,
    /// When it goes away, if ever.
    #[serde(default)]
    pub detach_at_ms: Option<u64>,
    /// Extra lockdown values (GetValue without a domain); they override the derived ones.
    #[serde(default)]
    pub lockdown: BTreeMap<String, toml::Value>,
    /// Lines of the device's `com.apple.syslog_relay` stream.
    #[serde(default, rename = "log")]
    pub logs: Vec<LogLine>,
}

fn default_name() -> String {
    "Russet Test iPhone".into()
}
fn default_product_type() -> String {
    "iPhone13,2".into()
}
fn default_os_version() -> String {
    "27.0".into()
}
fn default_build() -> String {
    "24A437".into()
}
fn default_transport() -> String {
    "usb".into()
}
fn yes() -> bool {
    true
}

/// The DDI state a device reports (ARCHITECTURE §9.5 `readiness.ddi`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Ddi {
    pub mounted: bool,
    /// `legacy`, `personalized-dmg` or `cryptex1`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub product_build: Option<String>,
    /// `imported`, `dir`, `pmd3-cache`, `pmd3-auto-mount` or `unknown`.
    #[serde(default)]
    pub source: Option<String>,
}

/// A syslog line, or a run of numbered lines.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LogLine {
    /// The whole line, verbatim. Otherwise the line is composed from the fields below.
    #[serde(default)]
    pub line: Option<String>,
    /// The message; `{n}` is replaced by the line's number when `repeat` is set.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default = "default_process")]
    pub process: String,
    #[serde(default = "default_pid")]
    pub pid: u32,
    /// Apple's syslog level name: `Debug`, `Info`, `Notice`, `Error`, `Fault`.
    #[serde(default = "default_level")]
    pub level: String,
    /// Delay before the first line, after the stream opens.
    #[serde(default)]
    pub at_ms: u64,
    /// How many numbered lines (default 1).
    #[serde(default)]
    pub repeat: Option<u32>,
    /// The gap between repeated lines.
    #[serde(default)]
    pub every_ms: u64,
}

fn default_process() -> String {
    "RussetProbe".into()
}
fn default_pid() -> u32 {
    932
}
fn default_level() -> String {
    "Notice".into()
}

impl LogLine {
    /// The rendered lines, in order (syslog_relay framing is added by the connector).
    pub fn render(&self, device_name: &str) -> Vec<String> {
        let count = self.repeat.unwrap_or(1).max(1);
        (1..=count)
            .map(|n| match &self.line {
                Some(l) => l.replace("{n}", &n.to_string()),
                None => {
                    let msg = self.message.as_deref().unwrap_or("").replace("{n}", &n.to_string());
                    // A fixed synthetic timestamp keeps traces and snapshots deterministic (QA §2 T10).
                    let host = device_name.replace(' ', "-");
                    format!("Oct 11 09:00:00 {host} {}[{}] <{}>: {msg}", self.process, self.pid, self.level)
                }
            })
            .collect()
    }
}

/// A facade call's scripted answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Call {
    pub api: String,
    /// Only for this device (the call's `args.udid`, or the only attached device).
    #[serde(default)]
    pub udid: Option<String>,
    /// Every key here must equal the call's argument of the same name; other arguments are ignored.
    #[serde(default)]
    pub args: Option<toml::Table>,
    #[serde(default)]
    pub result: Option<toml::Value>,
    #[serde(default)]
    pub error: Option<ScriptedError>,
}

/// An error a scenario returns, by its `russetCode` (ARCHITECTURE §13.3).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ScriptedError {
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// Which connector entry point a service or failure concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    /// `ServiceConnector::lockdown_service` (plaintext after TLS).
    Lockdown,
    /// `ServiceConnector::rsd_service` (inside the tunnel).
    Rsd,
}

/// A scripted service stream.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Service {
    pub name: String,
    #[serde(default = "default_via")]
    pub via: Via,
    #[serde(default)]
    pub udid: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

fn default_via() -> Via {
    Via::Lockdown
}

/// One step of a scripted stream; exactly one key is set.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Step {
    /// Device → client text.
    #[serde(default)]
    pub send: Option<String>,
    /// Device → client bytes, base64.
    #[serde(default)]
    pub send_b64: Option<String>,
    /// Client → device text that must come next.
    #[serde(default)]
    pub expect: Option<String>,
    /// Client → device bytes that must come next, base64.
    #[serde(default)]
    pub expect_b64: Option<String>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    /// Close the stream (the client reads EOF).
    #[serde(default)]
    pub close: Option<bool>,
}

/// A step after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Send(Vec<u8>),
    Expect(Vec<u8>),
    Delay(u64),
    Close,
}

impl Step {
    /// The step's single action.
    pub fn action(&self) -> std::result::Result<Action, String> {
        let mut found = Vec::new();
        if let Some(s) = &self.send {
            found.push(Action::Send(s.as_bytes().to_vec()));
        }
        if let Some(s) = &self.send_b64 {
            found.push(Action::Send(base64_decode(s)?));
        }
        if let Some(s) = &self.expect {
            found.push(Action::Expect(s.as_bytes().to_vec()));
        }
        if let Some(s) = &self.expect_b64 {
            found.push(Action::Expect(base64_decode(s)?));
        }
        if let Some(ms) = self.delay_ms {
            found.push(Action::Delay(ms));
        }
        if self.close == Some(true) {
            found.push(Action::Close);
        }
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err("a step needs one of send, send-b64, expect, expect-b64, delay-ms, close".into()),
            _ => Err("a step has exactly one of send, send-b64, expect, expect-b64, delay-ms, close".into()),
        }
    }
}

/// The connector operation a failure applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailOp {
    Watch,
    Call,
    LockdownService,
    RsdService,
}

/// A scripted failure.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Failure {
    pub op: FailOp,
    /// The service name or call api; every one when absent.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub udid: Option<String>,
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
    /// Fail this many times, then succeed; always when absent.
    #[serde(default)]
    pub times: Option<u32>,
}

impl Scenario {
    /// Parses and validates a scenario.
    pub fn from_toml(text: &str) -> Result<Self> {
        let s: Self = toml::from_str(text)
            .map_err(|e| Error::new(codes::core::INVALID_ARGUMENT, format!("invalid scenario: {e}")))?;
        s.validate()?;
        Ok(s)
    }

    /// Reads a scenario file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            Error::new(codes::core::NOT_FOUND, format!("cannot read the scenario {}: {e}", path.display()))
                .with_hint("RUSSET_DEVICE_BACKEND=scripted:<path to a scenario TOML file>")
        })?;
        Self::from_toml(&text).map_err(|e| {
            Error::new(e.code(), format!("{}: {}", path.display(), e.message()))
                .with_hint("see crates/russet-testkit/README.md for the scenario format")
        })
    }

    /// The default lane-D scenario.
    pub fn default_scenario() -> Self {
        match Self::from_toml(DEFAULT_SCENARIO) {
            Ok(s) => s,
            Err(e) => panic!("the embedded default scenario is invalid: {e}"),
        }
    }

    pub fn device(&self, udid: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.udid == udid)
    }

    fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::new(codes::core::INVALID_ARGUMENT, format!("invalid scenario: {m}")));
        if self.schema != SCHEMA {
            return bad(format!("schema {} (this testkit reads schema {SCHEMA})", self.schema));
        }
        for (i, d) in self.devices.iter().enumerate() {
            if d.udid.trim().is_empty() {
                return bad(format!("device {} has an empty udid", i + 1));
            }
            if self.devices[..i].iter().any(|o| o.udid == d.udid) {
                return bad(format!("udid {} appears twice", d.udid));
            }
            if d.detach_at_ms.is_some_and(|t| t <= d.attach_at_ms) {
                return bad(format!("device {}: detach-at-ms must come after attach-at-ms", d.udid));
            }
            for l in &d.logs {
                if l.line.is_none() && l.message.is_none() {
                    return bad(format!("device {}: a log entry needs line or message", d.udid));
                }
            }
        }
        let known = |u: &Option<String>| u.as_ref().is_none_or(|u| self.device(u).is_some());
        for c in &self.calls {
            if c.result.is_some() == c.error.is_some() {
                return bad(format!("call {}: give exactly one of result and error", c.api));
            }
            if let Some(e) = &c.error {
                code_by_name(&e.code)?;
            }
            if !known(&c.udid) {
                return bad(format!("call {}: unknown udid", c.api));
            }
        }
        for s in &self.services {
            if s.name.is_empty() {
                return bad("a service has an empty name".into());
            }
            for (i, step) in s.steps.iter().enumerate() {
                if let Err(e) = step.action() {
                    return bad(format!("service {} step {}: {e}", s.name, i + 1));
                }
            }
            if !known(&s.udid) {
                return bad(format!("service {}: unknown udid", s.name));
            }
        }
        for f in &self.failures {
            code_by_name(&f.code)?;
            if !known(&f.udid) {
                return bad(format!("failure {:?}: unknown udid", f.op));
            }
        }
        Ok(())
    }
}

/// Looks up an error code by its `russetCode` (`device.locked`).
pub fn code_by_name(name: &str) -> Result<ErrorCode> {
    russet_core::error::all_codes().iter().copied().find(|c| c.as_str() == name).ok_or_else(|| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("invalid scenario: unknown error code {name:?}"))
            .with_hint("use a russetCode of ARCHITECTURE §13.3, for example device.locked")
    })
}

/// Standard base64 (with or without padding), for `send-b64` and `expect-b64`.
pub fn base64_decode(s: &str) -> std::result::Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return Err(format!("not base64: {s:?}")),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
