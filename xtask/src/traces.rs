// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask traces`: RDT trace tools (QA §3.5): `lint` (the redaction safety net, DC-13), `info` (a summary)
//! and `diff` (a semantic drift report between two traces).
//!
//! Pre-created by W1 (PLAN §3 [D 86]); filled by W15.
//!
//! This is a seam crate (`xtask/Cargo.toml` is lead-only), so it cannot depend on `russet-testkit` or `zstd`:
//! it parses plain `*.rdt.jsonl` traces with `serde_json` alone. The authoritative RDT format and the same lint
//! live in `russet-testkit` (`rdt` and `redaction`); committed fixtures are stored as plain JSON Lines so this
//! command can read and lint them. Compressed `*.rdt.jsonl.zst` lab recordings are linted by the testkit before
//! they are committed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::workstreams;

/// Flags of `cargo xtask traces`.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Sub,
}

#[derive(Debug, clap::Subcommand)]
pub enum Sub {
    /// Lint traces for leaked identifiers and Apple-file payloads (QA §3.5.3). Default: every committed trace.
    Lint(LintArgs),
    /// Print a trace's header and record counts.
    Info(OneArgs),
    /// Report the semantic difference (record kinds, call APIs, service names) between two traces.
    Diff(DiffArgs),
}

#[derive(Debug, clap::Args)]
pub struct LintArgs {
    /// Traces to lint; default: `crates/russet-testkit/traces/**/*.rdt.jsonl`.
    pub paths: Vec<PathBuf>,
    /// Emit findings as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct OneArgs {
    pub path: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, clap::Args)]
pub struct DiffArgs {
    pub a: PathBuf,
    pub b: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    match args.cmd {
        Sub::Lint(a) => lint(a),
        Sub::Info(a) => info(a),
        Sub::Diff(a) => diff(a),
    }
}

/// The largest a single `data` record's decoded payload may be (QA §3.5.3).
const MAX_DATA_BYTES: usize = 1024 * 1024;

/// One lint finding.
#[derive(Debug, serde::Serialize)]
struct Finding {
    file: String,
    line: usize,
    kind: &'static str,
    detail: String,
}

fn lint(args: LintArgs) -> Result<()> {
    let paths = if args.paths.is_empty() { committed_traces()? } else { args.paths };
    if paths.is_empty() {
        println!("no traces to lint");
        return Ok(());
    }
    let mut findings = Vec::new();
    for path in &paths {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zst")) {
            bail!("{}: cargo xtask traces cannot read compressed traces; lint them in the testkit", path.display());
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        lint_text(path, &text, &mut findings);
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        for f in &findings {
            println!("{}:{}: {}: {}", f.file, f.line, f.kind, f.detail);
        }
    }
    if findings.is_empty() {
        println!("{} trace(s) clean", paths.len());
        Ok(())
    } else {
        bail!("{} redaction finding(s) in {} trace(s)", findings.len(), paths.len());
    }
}

fn lint_text(path: &Path, text: &str, out: &mut Vec<Finding>) {
    let file = path.display().to_string();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = i + 1;
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            out.push(Finding { file: file.clone(), line: line_no, kind: "parse", detail: "not JSON".into() });
            continue;
        };
        // `data` records: check the decoded payload for file magic and size, not the base64 text.
        if v.get("type").and_then(Value::as_str) == Some("data") {
            if let Some(b64) = v.get("b64").and_then(Value::as_str) {
                let bytes = base64_decode(b64);
                if bytes.len() > MAX_DATA_BYTES {
                    out.push(Finding {
                        file: file.clone(),
                        line: line_no,
                        kind: "oversized-data",
                        detail: format!("{} bytes (max {MAX_DATA_BYTES})", bytes.len()),
                    });
                }
                if let Some(m) = apple_file_magic(&bytes) {
                    out.push(Finding { file: file.clone(), line: line_no, kind: "apple-file", detail: m.into() });
                }
            }
            continue;
        }
        scan_value(&file, line_no, &v, out);
    }
}

fn scan_value(file: &str, line: usize, v: &Value, out: &mut Vec<Finding>) {
    match v {
        Value::String(s) => scan_text(file, line, s, out),
        Value::Array(a) => a.iter().for_each(|v| scan_value(file, line, v, out)),
        Value::Object(o) => o.values().for_each(|v| scan_value(file, line, v, out)),
        _ => {}
    }
}

fn scan_text(file: &str, line: usize, s: &str, out: &mut Vec<Finding>) {
    let push = |kind: &'static str, detail: String, out: &mut Vec<Finding>| {
        out.push(Finding { file: file.into(), line, kind, detail });
    };
    if let Some(d) = find_udid(s) {
        push("udid", d, out);
    }
    if let Some(d) = find_mac(s) {
        push("mac", d, out);
    }
    if let Some(d) = find_email(s) {
        push("email", d, out);
    }
    if s.contains("PRIVATE KEY-----") || s.contains("-----BEGIN RSA") || s.contains("-----BEGIN EC") {
        push("private-key", "a PEM private key".into(), out);
    }
}

/// A real-device UDID shape: `XXXXXXXX-XXXXXXXXXXXXXXXX` with a non-zero chip id, or a 40-hex run.
fn find_udid(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if i + 25 <= b.len()
            && b[i + 8] == b'-'
            && b[i..i + 8].iter().all(u8::is_ascii_hexdigit)
            && b[i + 9..i + 25].iter().all(u8::is_ascii_hexdigit)
            && !s[i..i + 8].eq_ignore_ascii_case("00000000")
        {
            return Some(format!("UDID-shaped {}…", &s[i..i + 9]));
        }
    }
    let (mut run, mut start) = (0usize, 0usize);
    for (i, c) in b.iter().enumerate() {
        if c.is_ascii_hexdigit() {
            if run == 0 {
                start = i;
            }
            run += 1;
        } else {
            if run == 40 {
                return Some(format!("40-hex UDID-shaped {}…", &s[start..start + 8]));
            }
            run = 0;
        }
    }
    (run == 40).then(|| format!("40-hex UDID-shaped {}…", &s[start..start + 8]))
}

fn find_mac(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if i + 17 <= b.len() {
            let w = &b[i..i + 17];
            let ok =
                w.iter().enumerate().all(|(k, c)| if (k + 1) % 3 == 0 { *c == b':' } else { c.is_ascii_hexdigit() });
            if ok && &s[i..i + 17] != "00:00:00:00:00:00" {
                return Some(format!("MAC {}…", &s[i..i + 8]));
            }
        }
    }
    None
}

fn find_email(s: &str) -> Option<String> {
    let at = s.find('@')?;
    let (local, rest) = (&s[..at], &s[at + 1..]);
    let atom = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'+' | b'%' | b'-');
    let local_ok = local.bytes().rev().take_while(|c| atom(*c)).count() > 0;
    let dot = rest.find('.')?;
    let domain = &rest[..dot];
    let tld: String = rest[dot + 1..].bytes().take_while(u8::is_ascii_alphabetic).map(char::from).collect();
    if local_ok
        && !domain.is_empty()
        && domain.bytes().all(atom)
        && tld.len() >= 2
        && !domain.eq_ignore_ascii_case("example")
    {
        return Some(format!("email …@{domain}.{tld}"));
    }
    None
}

fn apple_file_magic(bytes: &[u8]) -> Option<&'static str> {
    let starts = |m: &[u8]| bytes.len() >= m.len() && &bytes[..m.len()] == m;
    if starts(&[0xca, 0xfe, 0xba, 0xbe])
        || starts(&[0xfe, 0xed, 0xfa, 0xce])
        || starts(&[0xfe, 0xed, 0xfa, 0xcf])
        || starts(&[0xcf, 0xfa, 0xed, 0xfe])
    {
        return Some("a Mach-O binary");
    }
    if starts(b"IMG4") || (bytes.len() > 1 && &bytes[1..5.min(bytes.len())] == b"IMG4") {
        return Some("an IMG4 firmware image");
    }
    if starts(b"xar!") {
        return Some("a xar archive");
    }
    if starts(b"BOMStore") {
        return Some("a BOMStore");
    }
    let tail = &bytes[bytes.len().saturating_sub(512)..];
    if tail.windows(4).any(|w| w == b"koly") {
        return Some("a DMG (koly trailer)");
    }
    None
}

fn info(args: OneArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.path).with_context(|| format!("reading {}", args.path.display()))?;
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Value = serde_json::from_str(lines.next().context("empty trace")?).context("bad header")?;
    let mut kinds: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut apis: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in lines {
        let v: Value = serde_json::from_str(line).context("bad record")?;
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("?").to_owned();
        *kinds.entry(kind.clone()).or_default() += 1;
        if kind == "call"
            && let Some(api) = v.get("api").and_then(Value::as_str)
        {
            apis.insert(api.to_owned());
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "header": header, "records": kinds, "apis": apis }))?
        );
    } else {
        let d = &header["device"];
        println!(
            "scenario={} device={} {} ({})",
            header["scenario"].as_str().unwrap_or("?"),
            d["product_type"].as_str().unwrap_or("?"),
            d["product_version"].as_str().unwrap_or("?"),
            d["build_version"].as_str().unwrap_or("?"),
        );
        for (k, n) in &kinds {
            println!("  {k}: {n}");
        }
        if !apis.is_empty() {
            println!("  apis: {}", apis.iter().cloned().collect::<Vec<_>>().join(", "));
        }
    }
    Ok(())
}

/// The semantic skeleton of a trace: the ordered record kinds, the call APIs and the service names. A drift in
/// any of these after an iOS update is a protocol-drift signal (QA §3.5.6).
fn skeleton(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()).skip(1) {
        let v: Value = serde_json::from_str(line).context("bad record")?;
        match v.get("type").and_then(Value::as_str) {
            Some("call") => out.push(format!("call {}", v.get("api").and_then(Value::as_str).unwrap_or("?"))),
            Some("conn") if v.get("open").and_then(Value::as_bool) == Some(true) => {
                out.push(format!("open {}", v.get("service").and_then(Value::as_str).unwrap_or("?")));
            }
            Some(k) => out.push(k.to_owned()),
            None => {}
        }
    }
    Ok(out)
}

fn diff(args: DiffArgs) -> Result<()> {
    let a = skeleton(&args.a)?;
    let b = skeleton(&args.b)?;
    if a == b {
        println!("no semantic drift ({} records)", a.len());
        return Ok(());
    }
    println!("semantic drift between {} and {}:", args.a.display(), args.b.display());
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            println!("  [{i}] {x}  →  {y}");
        }
    }
    if a.len() != b.len() {
        println!("  length {} → {}", a.len(), b.len());
    }
    bail!("the traces differ");
}

/// Every committed `*.rdt.jsonl` under the testkit's `traces/`.
fn committed_traces() -> Result<Vec<PathBuf>> {
    let root = workstreams::repo_root().join("crates/russet-testkit/traces");
    let mut out = Vec::new();
    collect(&root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.to_string_lossy().ends_with(".rdt.jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// Standard base64 decode (xtask has no base64 dependency).
fn base64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return out,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}
