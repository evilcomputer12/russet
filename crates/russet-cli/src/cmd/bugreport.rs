// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet bugreport [--out PATH]` (ARCHITECTURE §8.7; core §12, CORE-13). Owner: W2.
//!
//! Writes a zip (0600) to the cache directory with: this build's identity, `daemon.status`, `doctor.run`,
//! `toolchain.list` (each as JSON, or the error the daemon gave), the last three daemon log files, the
//! configuration files (`config.toml`, `mcp-policy.toml`, `provision-policy.toml`) and a summary of
//! `accounts.toml` (how many accounts of which kind; the file itself stays out). Nothing is uploaded, and the
//! daemon is not started for a bug report: without one, the report says so.
//!
//! A bug report is meant to be attached to an issue, so every entry goes through two filters: the secret
//! redaction of the logs (secret references only), and an [`IdentifierMask`] that replaces UDIDs, Apple IDs,
//! DSIDs, team, key and issuer IDs, signing-identity and device names with placeholders (`<udid-1>`; equal
//! values share one) and the home directory with `~` (ARCHITECTURE §12.1).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result};
use russet_core::redact::{IdentifierMask, redact_json, redact_text};
use serde_json::Value;

use crate::CliCtx;

/// The `bugreport` command.
pub fn command() -> Command {
    Command::new("bugreport").about("Collect a redacted bug report (nothing is uploaded)").arg(
        Arg::new("out")
            .long("out")
            .value_name("PATH")
            .value_parser(clap::value_parser!(PathBuf))
            .help("Where to write the zip"),
    )
}

/// Every line of `text`, with secrets redacted and identifiers masked.
fn redact_lines(text: &str, mask: &mut IdentifierMask) -> String {
    text.lines().map(|l| mask.text(&redact_text(l))).collect::<Vec<_>>().join("\n") + "\n"
}

/// The user's home directory, which the report shows as `~`.
fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// What a bug report keeps of `accounts.toml`: how many accounts of which kind, and whether one is the default.
fn accounts_summary(text: &str) -> Value {
    let mut kinds = std::collections::BTreeMap::<String, u64>::new();
    let mut accounts = 0u64;
    let mut default = false;
    for line in text.lines().map(str::trim) {
        if line == "[[account]]" {
            accounts += 1;
        } else if let Some(v) = line.strip_prefix("kind").map(str::trim_start).and_then(|r| r.strip_prefix('=')) {
            let kind = v.trim().trim_matches('"');
            if matches!(kind, "asc-team-key" | "asc-individual-key" | "apple-id") {
                *kinds.entry(kind.to_owned()).or_default() += 1;
            } else {
                *kinds.entry("other".to_owned()).or_default() += 1;
            }
        } else if line.starts_with("default") && line.contains('=') {
            default = true;
        }
    }
    let kinds: Vec<Value> =
        kinds.into_iter().map(|(kind, count)| serde_json::json!({"kind": kind, "count": count})).collect();
    serde_json::json!({"accounts": accounts, "kinds": kinds, "default": default})
}

/// The last `n` daemon log files, oldest first.
fn last_logs(dir: &Path, n: usize) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .is_some_and(|f| f.starts_with("russetd.") && f.ends_with(".log"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let skip = files.len().saturating_sub(n);
    files.split_off(skip)
}

/// Runs this area's command; `m` is the root matches, so `m.subcommand()` names the command.
pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode> {
    let Some((_, a)) = m.subcommand() else { return Err(Error::invalid_argument("no bugreport command")) };
    let dirs = RussetDirs::from_env()?;
    let mut zip = Zip::new(IdentifierMask::new(home_dir().as_deref()));
    let about = serde_json::json!({
        "version": russet_core::build_info::VERSION,
        "buildId": russet_core::build_info::BUILD_ID,
        "target": russet_core::build_info::TARGET,
        "platform": russet_core::platform::host_os(),
        "arch": russet_core::platform::host_arch(),
        "createdAt": russet_core::time::now_rfc3339(),
    });
    zip.add_json("russet.json", &about);

    // The daemon's view, without starting one.
    match cx.peer.existing().await {
        Ok(_) => {
            for (file, method, params) in [
                ("daemon-status.json", "daemon.status", serde_json::json!({})),
                ("doctor.json", "doctor.run", serde_json::json!({})),
                ("toolchain.json", "toolchain.list", serde_json::json!({})),
            ] {
                let v = match cx.peer.call_raw(method, params, russet_ipc::CallOptions::default()).await {
                    Ok(v) => v,
                    Err(e) => serde_json::json!({"error": e.to_data(), "message": e.message()}),
                };
                zip.add_json(file, &v);
            }
        }
        Err(e) => {
            zip.add_json("daemon-status.json", &serde_json::json!({"error": e.to_data(), "message": e.message()}))
        }
    }

    for log in last_logs(&dirs.logs(), 3) {
        if let (Ok(text), Some(name)) = (std::fs::read_to_string(&log), log.file_name().and_then(|n| n.to_str())) {
            zip.add_text(&format!("logs/{name}"), &text);
        }
    }
    for name in ["config.toml", "mcp-policy.toml", "provision-policy.toml"] {
        if let Ok(text) = std::fs::read_to_string(dirs.config.join(name)) {
            zip.add_text(&format!("config/{name}"), &text);
        }
    }
    if let Ok(text) = std::fs::read_to_string(dirs.config.join("accounts.toml")) {
        zip.add_json("config/accounts.summary.json", &accounts_summary(&text));
    }

    let out = match a.get_one::<PathBuf>("out") {
        Some(p) => p.clone(),
        None => {
            russet_core::fsx::ensure_private_dir(&dirs.cache)?;
            let stamp = russet_core::time::now_rfc3339().replace(':', "-");
            dirs.cache.join(format!("bugreport-{stamp}.zip"))
        }
    };
    let names = zip.names.clone();
    russet_core::fsx::atomic_write(&out, &zip.finish())?;
    cx.output(&serde_json::json!({"path": out.display().to_string(), "entries": names}), |_| {
        format!("wrote {} ({} files); nothing was uploaded", out.display(), names.len())
    })?;
    Ok(ExitCode::SUCCESS)
}

/// A minimal zip writer: stored (uncompressed) entries, UTF-8 names, fixed timestamps. Text and JSON entries go
/// through the redaction and the identifier mask of the report.
#[derive(Default)]
pub struct Zip {
    data: Vec<u8>,
    central: Vec<u8>,
    names: Vec<String>,
    mask: IdentifierMask,
}

/// CRC-32 (IEEE 802.3), as zip uses it.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

impl Zip {
    /// An archive whose text entries `mask` filters.
    pub fn new(mask: IdentifierMask) -> Self {
        Self { mask, ..Self::default() }
    }

    fn add_json(&mut self, name: &str, v: &Value) {
        let mut v = v.clone();
        redact_json(&mut v);
        self.mask.json(&mut v);
        let text = serde_json::to_string_pretty(&v).unwrap_or_default();
        self.add_text(name, &text);
    }

    fn add_text(&mut self, name: &str, text: &str) {
        let filtered = redact_lines(text, &mut self.mask);
        self.add(name, filtered.as_bytes());
    }

    /// Adds a file.
    pub fn add(&mut self, name: &str, bytes: &[u8]) {
        // DOS time 00:00, date 1980-01-01; flag 0x0800: UTF-8 names.
        let (time, date, flags) = (0u16, 0x0021u16, 0x0800u16);
        let crc = crc32(bytes);
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let name_len = u16::try_from(name.len()).unwrap_or(u16::MAX);
        let offset = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        let local = &mut self.data;
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for v in [20u16, flags, 0, time, date] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(bytes);
        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // Made by Unix (3) version 2.0, needs 2.0.
        for v in [0x0314u16, 20, flags, 0, time, date] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0, 0, 0, 0] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        // External attributes: a regular file, mode 0600.
        c.extend_from_slice(&(0o100_600u32 << 16).to_le_bytes());
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        self.names.push(name.to_owned());
    }

    /// The archive.
    pub fn finish(mut self) -> Vec<u8> {
        let cd_offset = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        let cd_size = u32::try_from(self.central.len()).unwrap_or(u32::MAX);
        let count = u16::try_from(self.names.len()).unwrap_or(u16::MAX);
        self.data.extend_from_slice(&self.central);
        self.data.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        for v in [0u16, 0, count, count] {
            self.data.extend_from_slice(&v.to_le_bytes());
        }
        self.data.extend_from_slice(&cd_size.to_le_bytes());
        self.data.extend_from_slice(&cd_offset.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_accounts_summary_names_no_account() {
        let text = "schema = 1\ndefault = \"asc:QW3ERT5YU7\"\n\n[[account]]\nid = \"asc:QW3ERT5YU7\"\nkind = \"asc-team-key\"\n\
                    team-id = \"ZX9Y8W7V6U\"\n\n[[account]]\nkind = \"apple-id\"\napple-id = \"a@example.com\"\n";
        let v = accounts_summary(text);
        assert_eq!(
            v,
            serde_json::json!({"accounts": 2, "default": true, "kinds": [{"kind": "apple-id", "count": 1},
                                                                         {"kind": "asc-team-key", "count": 1}]})
        );
    }

    #[test]
    fn archives_have_entries_and_an_end_record() {
        let mut z = Zip::default();
        z.add("a.txt", b"hello");
        z.add("dir/b.json", b"{}");
        let bytes = z.finish();
        assert_eq!(&bytes[..4], &0x0403_4b50u32.to_le_bytes());
        let eocd = bytes.len() - 22;
        assert_eq!(&bytes[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]), 2);
        assert!(bytes.windows(5).any(|w| w == b"hello"));
    }
}
