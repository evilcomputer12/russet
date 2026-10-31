// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The crash store and symbolication (RD §13, RD-13). Owner: W11.
//!
//! The device plane pulls `.ips` reports (DEV-15, W10) and hands their bytes to [`CrashStore::ingest`], which
//! parses the header + body (iOS 15+ two-JSON-document format), writes the raw `.ips` and a [`CrashSummary`]
//! index entry under `data/crashes/<bundleId>/<crashId>.{ips,json}`, and returns the summary. [`CrashStore::list`]
//! reads the index; [`CrashStore::symbolicate`] resolves app frames through the dSYM index
//! (`data/dsyms/<UUID>.dSYM`, RD §11.5) with the `symbolic` crate, caching the JSON and a text rendering next to
//! the report. System frames need symbols tier ≥ 1 (RD §11.4), which is a separate device-gated fetch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_protocol::crash::{CrashFrame, CrashSummary, CrashThread, SymbolCoverage, SymbolicatedCrash};
use serde_json::Value;

/// One image (binary) the crash references (`usedImages`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Image {
    uuid: Option<String>,
    base: u64,
    path: String,
    name: String,
}

impl Image {
    /// The image's short name (the last path component), for display.
    fn short(&self) -> &str {
        if self.name.is_empty() { self.path.rsplit('/').next().unwrap_or(&self.path) } else { &self.name }
    }
}

/// A raw frame as the `.ips` records it: an index into `usedImages` and the offset into that image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawFrame {
    image_index: usize,
    image_offset: u64,
}

/// A thread as parsed from the `.ips`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawThread {
    name: Option<String>,
    crashed: bool,
    frames: Vec<RawFrame>,
}

/// A parsed `.ips` crash report (the fields Russet uses; RD §13 step 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCrash {
    bundle_id: String,
    process: String,
    /// RFC 3339 UTC, derived from the header's `timestamp`.
    timestamp: String,
    /// A sortable key for the crash id and the store path: `YYYY-MM-DDTHH-MM-SS`.
    stamp_key: String,
    incident: String,
    exception: String,
    signal: Option<String>,
    termination: Option<String>,
    images: Vec<Image>,
    threads: Vec<RawThread>,
}

impl ParsedCrash {
    /// The stable store id: the sortable timestamp plus a short incident tag.
    pub fn crash_id(&self) -> String {
        let incident = self.incident.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>();
        if incident.is_empty() { self.stamp_key.clone() } else { format!("{}-{incident}", self.stamp_key) }
    }

    fn summary(&self, symbolicated: bool) -> CrashSummary {
        CrashSummary {
            crash_id: self.crash_id(),
            bundle_id: self.bundle_id.clone(),
            process: self.process.clone(),
            timestamp: self.timestamp.clone(),
            exception: self.exception.clone(),
            signal: self.signal.clone(),
            symbolicated,
        }
    }
}

/// Parses an `.ips` report: a JSON header line, then a JSON body (iOS 15+). A body-only file is also accepted.
pub fn parse_ips(bytes: &[u8]) -> Result<ParsedCrash> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| crash_error(codes::crash::SYMBOLICATION_FAILED, "the crash report is not UTF-8"))?;
    let trimmed = text.trim_start();
    // The header is the first line; the body is the rest. When the whole file is one JSON document (no header),
    // that document is the body and the header is empty.
    let (header, body) = match trimmed.split_once('\n') {
        Some((h, rest)) if serde_json::from_str::<Value>(h.trim()).is_ok() && !rest.trim().is_empty() => {
            (serde_json::from_str::<Value>(h.trim()).unwrap_or(Value::Null), rest)
        }
        _ => (Value::Null, trimmed),
    };
    let body: Value = serde_json::from_str(body.trim())
        .map_err(|e| crash_error(codes::crash::SYMBOLICATION_FAILED, format!("the crash body is not JSON: {e}")))?;

    let str_at = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let bundle_id = str_at(&header, "bundleID")
        .or_else(|| body.pointer("/bundleInfo/CFBundleIdentifier").and_then(Value::as_str).map(str::to_owned))
        .or_else(|| str_at(&body, "coalitionName"))
        .unwrap_or_default();
    let process = str_at(&header, "procName")
        .or_else(|| str_at(&body, "procName"))
        .or_else(|| str_at(&header, "name"))
        .unwrap_or_default();
    let raw_ts = str_at(&header, "timestamp").or_else(|| str_at(&body, "captureTime")).unwrap_or_default();
    let (timestamp, stamp_key) = normalise_timestamp(&raw_ts);
    let incident = str_at(&header, "incident_id").or_else(|| str_at(&body, "incident")).unwrap_or_default();

    let exc_type = body.pointer("/exception/type").and_then(Value::as_str);
    let exc_signal = body.pointer("/exception/signal").and_then(Value::as_str);
    let exception = match (exc_type, exc_signal) {
        (Some(t), Some(s)) => format!("{t} ({s})"),
        (Some(t), None) => t.to_owned(),
        (None, Some(s)) => s.to_owned(),
        (None, None) => {
            str_at(&header, "bug_type").map(|b| format!("bug_type {b}")).unwrap_or_else(|| "unknown".into())
        }
    };
    let signal = exc_signal.map(str::to_owned);
    let termination = body
        .pointer("/termination/indicator")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| body.pointer("/termination/reason").and_then(Value::as_str).map(str::to_owned));

    let images = parse_images(&body);
    let threads = parse_threads(&body);
    Ok(ParsedCrash {
        bundle_id,
        process,
        timestamp,
        stamp_key,
        incident,
        exception,
        signal,
        termination,
        images,
        threads,
    })
}

fn parse_images(body: &Value) -> Vec<Image> {
    body.get("usedImages")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|im| Image {
                    uuid: im.get("uuid").and_then(Value::as_str).map(normalise_uuid),
                    base: im.get("base").and_then(Value::as_u64).unwrap_or(0),
                    path: im.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    name: im.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_threads(body: &Value) -> Vec<RawThread> {
    body.get("threads")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|t| RawThread {
                    name: t
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| t.get("queue").and_then(Value::as_str))
                        .map(str::to_owned),
                    crashed: t.get("triggered").and_then(Value::as_bool).unwrap_or(false),
                    frames: t
                        .get("frames")
                        .and_then(Value::as_array)
                        .map(|fr| {
                            fr.iter()
                                .map(|f| RawFrame {
                                    image_index: usize::try_from(
                                        f.get("imageIndex").and_then(Value::as_u64).unwrap_or(0),
                                    )
                                    .unwrap_or(0),
                                    image_offset: f.get("imageOffset").and_then(Value::as_u64).unwrap_or(0),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Normalises an Apple timestamp (`2026-10-30 09:00:00.00 +0000`) to RFC 3339 UTC and a filename-safe key.
fn normalise_timestamp(raw: &str) -> (String, String) {
    // Apple's `.ips` timestamps are like `2026-10-30 09:00:00.00 +0000`. Keep the date and time; drop the rest.
    let mut parts = raw.split_whitespace();
    let date = parts.next().unwrap_or("");
    let time = parts.next().unwrap_or("");
    let time = time.split('.').next().unwrap_or(time);
    if date.len() == 10 && time.len() == 8 {
        let rfc = format!("{date}T{time}.000Z");
        let key = format!("{date}T{}", time.replace(':', "-"));
        (rfc, key)
    } else {
        let now = crate::logs::now_rfc3339();
        let key = now.replace([':', '.'], "-").replace('Z', "");
        (now, key)
    }
}

fn normalise_uuid(u: &str) -> String {
    u.replace('-', "").to_ascii_uppercase()
}

/// The local crash store (`data/crashes/`) with its dSYM index (`data/dsyms/`).
#[derive(Debug, Clone)]
pub struct CrashStore {
    root: PathBuf,
    dsyms: PathBuf,
}

impl CrashStore {
    /// The store rooted at Russet's data directory (ARCHITECTURE §15).
    pub fn new(data_dir: &Path) -> Self {
        Self { root: data_dir.join("crashes"), dsyms: data_dir.join("dsyms") }
    }

    /// Stores a raw `.ips` report and returns its summary. Idempotent: the same report (same id) is not
    /// duplicated. Called by the device plane after it pulls a report (DEV-15) and by `crash.fetch`.
    pub fn ingest(&self, ips: &[u8]) -> Result<CrashSummary> {
        let parsed = parse_ips(ips)?;
        let dir = self.root.join(sanitize(&parsed.bundle_id));
        crate::fsutil::private_dir(&dir).map_err(|e| crate::errors::io("creating the crash store", e))?;
        let id = parsed.crash_id();
        let ips_path = dir.join(format!("{id}.ips"));
        if !ips_path.exists() {
            crate::fsutil::atomic_write(&ips_path, ips)
                .map_err(|e| crate::errors::io("writing the crash report", e))?;
        }
        let summary = parsed.summary(dir.join(format!("{id}.txt")).exists());
        self.write_index(&dir, &summary)?;
        Ok(summary)
    }

    fn write_index(&self, dir: &Path, summary: &CrashSummary) -> Result<()> {
        let path = dir.join(format!("{}.json", summary.crash_id));
        let bytes = serde_json::to_vec_pretty(summary).unwrap_or_default();
        crate::fsutil::atomic_write(&path, &bytes).map_err(|e| crate::errors::io("writing the crash index", e))
    }

    /// Lists stored crashes, newest first, optionally filtered by bundle id and a since-time.
    pub fn list(&self, bundle_id: Option<&str>, since: Option<&str>, limit: Option<usize>) -> Vec<CrashSummary> {
        let mut out = Vec::new();
        let dirs: Vec<PathBuf> = match bundle_id {
            Some(b) => vec![self.root.join(sanitize(b))],
            None => std::fs::read_dir(&self.root)
                .into_iter()
                .flatten()
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect(),
        };
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for entry in rd.filter_map(std::result::Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(summary) = serde_json::from_slice::<CrashSummary>(&bytes)
                {
                    if since.is_some_and(|s| summary.timestamp.as_str() < s) {
                        continue;
                    }
                    let txt = dir.join(format!("{}.txt", summary.crash_id));
                    out.push(CrashSummary { symbolicated: txt.exists(), ..summary });
                }
            }
        }
        // Newest first (the id starts with a sortable timestamp).
        out.sort_by(|a, b| b.crash_id.cmp(&a.crash_id));
        if let Some(n) = limit {
            out.truncate(n);
        }
        out
    }

    /// The raw `.ips` bytes of a stored crash.
    fn read_stored(&self, crash_id: &str) -> Result<Vec<u8>> {
        for dir in std::fs::read_dir(&self.root).into_iter().flatten().filter_map(std::result::Result::ok) {
            let ips = dir.path().join(format!("{crash_id}.ips"));
            if ips.exists() {
                return std::fs::read(&ips).map_err(|e| crate::errors::io("reading the crash report", e));
            }
        }
        Err(crash_error(codes::crash::NOT_FOUND, format!("no crash {crash_id}"))
            .with_details(serde_json::json!({ "crashId": crash_id })))
    }

    /// Symbolicates a stored crash (by id) or an `.ips` file (by path). The JSON result is cached next to a
    /// stored report so `crash.list` can report `symbolicated: true`.
    pub fn symbolicate(&self, crash_id: Option<&str>, path: Option<&Path>) -> Result<SymbolicatedCrash> {
        let (bytes, store_dir) = match (crash_id, path) {
            (Some(id), _) => {
                let bytes = self.read_stored(id)?;
                let parsed = parse_ips(&bytes)?;
                (bytes, Some(self.root.join(sanitize(&parsed.bundle_id))))
            }
            (None, Some(p)) => (std::fs::read(p).map_err(|e| crate::errors::io("reading the crash report", e))?, None),
            (None, None) => {
                return Err(Error::new(codes::core::INVALID_ARGUMENT, "pass a crashId or a path")
                    .with_hint("`russet crash symbolicate <id>` or `--path file.ips`."));
            }
        };
        let parsed = parse_ips(&bytes)?;
        let out = self.symbolicate_parsed(&parsed);
        if let Some(dir) = store_dir {
            let _ = crate::fsutil::atomic_write(
                &dir.join(format!("{}.json", parsed.crash_id())),
                &serde_json::to_vec_pretty(&out).unwrap_or_default(),
            );
            let _ = crate::fsutil::atomic_write(
                &dir.join(format!("{}.txt", parsed.crash_id())),
                render_text(&out).as_bytes(),
            );
        }
        Ok(out)
    }

    fn symbolicate_parsed(&self, parsed: &ParsedCrash) -> SymbolicatedCrash {
        // One symbol cache per dSYM, built on demand and reused across this report's frames.
        let mut caches: BTreeMap<String, Option<DsymCache>> = BTreeMap::new();
        let mut coverage = SymbolCoverage::None;
        let threads = parsed
            .threads
            .iter()
            .enumerate()
            .map(|(ti, thread)| {
                let frames = thread
                    .frames
                    .iter()
                    .enumerate()
                    .map(|(fi, raw)| {
                        let image = parsed.images.get(raw.image_index);
                        let (image_name, uuid) =
                            image.map_or((String::new(), None), |im| (im.short().to_owned(), im.uuid.clone()));
                        let mut frame = CrashFrame {
                            index: u32::try_from(fi).unwrap_or(0),
                            image: image_name,
                            image_offset: raw.image_offset,
                            symbol: None,
                            file: None,
                            line: None,
                            inlined: false,
                        };
                        if let Some(uuid) = uuid {
                            let cache = caches.entry(uuid.clone()).or_insert_with(|| self.load_dsym(&uuid));
                            if let Some(cache) = cache
                                && let Some(loc) = cache.resolve(raw.image_offset)
                            {
                                frame.symbol = Some(loc.symbol);
                                frame.file = loc.file;
                                frame.line = loc.line;
                                coverage = SymbolCoverage::App;
                            }
                        }
                        frame
                    })
                    .collect();
                CrashThread {
                    index: u32::try_from(ti).unwrap_or(0),
                    crashed: thread.crashed,
                    name: thread.name.clone(),
                    frames,
                }
            })
            .collect();
        SymbolicatedCrash { summary: parsed.summary(true), termination: parsed.termination.clone(), threads, coverage }
    }

    /// Opens the dSYM for an image UUID and builds a symbol cache, or `None` when there is no dSYM.
    fn load_dsym(&self, uuid: &str) -> Option<DsymCache> {
        let bundle = self.dsyms.join(format!("{uuid}.dSYM"));
        let dwarf = find_dwarf(&bundle)?;
        DsymCache::open(&dwarf)
    }
}

/// A resolved source location.
struct Resolved {
    symbol: String,
    file: Option<String>,
    line: Option<u32>,
}

/// A parsed dSYM with a `symbolic` symbol cache for address lookup.
struct DsymCache {
    buf: Vec<u8>,
}

impl DsymCache {
    /// Builds a symbol cache from the DWARF Mach-O inside a dSYM bundle.
    fn open(dwarf: &Path) -> Option<Self> {
        use symbolic::debuginfo::Object;
        use symbolic::symcache::SymCacheConverter;
        let data = std::fs::read(dwarf).ok()?;
        let object = Object::parse(&data).ok()?;
        let mut converter = SymCacheConverter::new();
        converter.process_object(&object).ok()?;
        let mut buf = Vec::new();
        converter.serialize(&mut buf).ok()?;
        // Confirm it parses, so `resolve` can unwrap.
        symbolic::symcache::SymCache::parse(&buf).ok()?;
        Some(Self { buf })
    }

    /// Resolves an image-relative address to a function name and, when available, file and line.
    fn resolve(&self, addr: u64) -> Option<Resolved> {
        use symbolic::demangle::{Demangle, DemangleOptions};
        use symbolic::symcache::SymCache;
        let cache = SymCache::parse(&self.buf).ok()?;
        let loc = cache.lookup(addr).next()?;
        let name = loc.function().name_for_demangling().try_demangle(DemangleOptions::name_only()).into_owned();
        let file = loc.file().map(|f| f.full_path());
        let line = (loc.line() != 0).then_some(loc.line());
        Some(Resolved { symbol: name, file, line })
    }
}

/// Finds the DWARF Mach-O inside a `<Name>.dSYM/Contents/Resources/DWARF/<Name>` bundle.
fn find_dwarf(bundle: &Path) -> Option<PathBuf> {
    let dwarf_dir = bundle.join("Contents/Resources/DWARF");
    std::fs::read_dir(&dwarf_dir).ok()?.filter_map(std::result::Result::ok).map(|e| e.path()).find(|p| p.is_file())
}

/// An Xcode-like text rendering of a symbolicated crash.
pub fn render_text(c: &SymbolicatedCrash) -> String {
    let mut out = String::new();
    out.push_str(&format!("Process:   {}\n", c.summary.process));
    out.push_str(&format!("Bundle:    {}\n", c.summary.bundle_id));
    out.push_str(&format!("Date:      {}\n", c.summary.timestamp));
    out.push_str(&format!("Exception: {}\n", c.summary.exception));
    if let Some(t) = &c.termination {
        out.push_str(&format!("Termination: {t}\n"));
    }
    out.push('\n');
    for thread in &c.threads {
        let marker = if thread.crashed { " Crashed" } else { "" };
        let name = thread.name.as_deref().map(|n| format!(" {n}")).unwrap_or_default();
        out.push_str(&format!("Thread {}{}{}:\n", thread.index, marker, name));
        for f in &thread.frames {
            let loc = match (&f.symbol, &f.file, f.line) {
                (Some(sym), Some(file), Some(line)) => format!("{sym} ({file}:{line})"),
                (Some(sym), _, _) => sym.clone(),
                _ => format!("0x{:x}", f.image_offset),
            };
            out.push_str(&format!("{:<4}{:<30} 0x{:<12x} {}\n", f.index, f.image, f.image_offset, loc));
        }
        out.push('\n');
    }
    out
}

/// A filesystem-safe form of a bundle id (and a fallback for an empty one).
fn sanitize(bundle_id: &str) -> String {
    if bundle_id.is_empty() {
        return "unknown".to_owned();
    }
    bundle_id.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect()
}

/// A `crash`-family error with its hint.
pub fn crash_error(code: russet_core::error::ErrorCode, msg: impl Into<String>) -> Error {
    let e = Error::new(code, msg);
    match code {
        c if c == codes::crash::NOT_FOUND => e.with_hint("List stored crashes with `russet crash list`."),
        c if c == codes::crash::SYMBOLICATION_FAILED => {
            e.with_hint("The report may be truncated or redacted; try `crash fetch` again, or open the raw `.ips`.")
        }
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample_ips() -> Vec<u8> {
        let header = serde_json::json!({
            "app_name": "RussetProbe", "timestamp": "2026-10-30 09:00:00.00 +0000", "bug_type": "309",
            "bundleID": "dev.russet.test.probe", "procName": "RussetProbe", "incident_id": "ABCD1234-0000-0000-0000-000000000000"
        });
        let body = serde_json::json!({
            "procName": "RussetProbe",
            "exception": {"type": "EXC_BAD_ACCESS", "signal": "SIGSEGV", "codes": "0x0000000000000000"},
            "termination": {"indicator": "Namespace SIGNAL, Code 11"},
            "usedImages": [
                {"uuid": "11223344-5566-7788-99aa-bbccddeeff00", "base": 4096, "name": "RussetProbe", "path": "/RussetProbe.app/RussetProbe"},
                {"uuid": "00000000-0000-0000-0000-000000000000", "base": 0, "name": "UIKitCore", "path": "/System/.../UIKitCore"}
            ],
            "threads": [
                {"triggered": true, "name": "main", "frames": [{"imageIndex": 0, "imageOffset": 8192}, {"imageIndex": 1, "imageOffset": 1024}]},
                {"frames": [{"imageIndex": 1, "imageOffset": 2048}]}
            ]
        });
        format!("{}\n{}", header, body).into_bytes()
    }

    #[test]
    fn parses_the_two_document_ips_format() {
        let parsed = parse_ips(&sample_ips()).unwrap();
        assert_eq!(parsed.bundle_id, "dev.russet.test.probe");
        assert_eq!(parsed.process, "RussetProbe");
        assert_eq!(parsed.exception, "EXC_BAD_ACCESS (SIGSEGV)");
        assert_eq!(parsed.signal.as_deref(), Some("SIGSEGV"));
        assert_eq!(parsed.timestamp, "2026-10-30T09:00:00.000Z");
        assert_eq!(parsed.crash_id(), "2026-10-30T09-00-00-ABCD1234");
        assert_eq!(parsed.images.len(), 2);
        assert_eq!(
            parsed.images[0].uuid.as_deref(),
            Some("1122334455667788 99AABBCCDDEEFF00".replace(' ', "").as_str())
        );
        assert_eq!(parsed.threads.len(), 2);
        assert!(parsed.threads[0].crashed);
    }

    #[test]
    fn ingest_list_and_symbolicate_without_a_dsym() {
        let base = std::env::temp_dir().join(format!("russet-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = CrashStore::new(&base);
        let summary = store.ingest(&sample_ips()).unwrap();
        assert_eq!(summary.bundle_id, "dev.russet.test.probe");
        assert!(!summary.symbolicated);
        // Ingesting again does not duplicate.
        store.ingest(&sample_ips()).unwrap();
        let listed = store.list(None, None, None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].crash_id, summary.crash_id);
        // A since filter in the future hides it.
        assert!(store.list(None, Some("2027-01-01T00:00:00.000Z"), None).is_empty());
        // Symbolication with no dSYM leaves app frames at `image + offset`, coverage none.
        let out = store.symbolicate(Some(&summary.crash_id), None).unwrap();
        assert_eq!(out.coverage, SymbolCoverage::None);
        assert_eq!(out.threads[0].frames[0].image, "RussetProbe");
        assert_eq!(out.threads[0].frames[0].image_offset, 8192);
        assert!(out.threads[0].frames[0].symbol.is_none());
        let text = render_text(&out);
        assert!(text.contains("Thread 0 Crashed main:"), "{text}");
        assert!(text.contains("0x2000"));
        // After symbolication the store marks it symbolicated.
        assert!(store.list(None, None, None)[0].symbolicated);
        let missing = store.symbolicate(Some("nope"), None).unwrap_err();
        assert_eq!(missing.code(), codes::crash::NOT_FOUND);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn body_only_reports_parse() {
        let body = serde_json::json!({
            "procName": "X", "exception": {"type": "EXC_CRASH"}, "usedImages": [], "threads": []
        });
        let parsed = parse_ips(body.to_string().as_bytes()).unwrap();
        assert_eq!(parsed.process, "X");
        assert_eq!(parsed.exception, "EXC_CRASH");
    }
}
