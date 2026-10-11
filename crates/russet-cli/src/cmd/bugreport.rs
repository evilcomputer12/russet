// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet bugreport [--out PATH]` (ARCHITECTURE §8.7; core §12, CORE-13). Owner: W2.
//!
//! Writes a zip (0600) to the cache directory with: this build's identity, `daemon.status`, `doctor.run`,
//! `toolchain.list` (each as JSON, or the error the daemon gave), the last three daemon log files and the
//! configuration files. Every text line goes through the redaction rules again, so the archive holds secret
//! references only. Nothing is uploaded. The daemon is not started for a bug report: without one, the report says
//! so.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Arg, ArgMatches, Command};
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result};
use russet_core::redact::redact_text;
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

/// Every line of `text`, redacted.
fn redact_lines(text: &str) -> String {
    text.lines().map(|l| redact_text(l).into_owned()).collect::<Vec<_>>().join("\n") + "\n"
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
    let mut zip = Zip::default();
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
            zip.add(&format!("logs/{name}"), redact_lines(&text).as_bytes());
        }
    }
    for name in ["config.toml", "mcp-policy.toml", "provision-policy.toml", "accounts.toml"] {
        if let Ok(text) = std::fs::read_to_string(dirs.config.join(name)) {
            zip.add(&format!("config/{name}"), redact_lines(&text).as_bytes());
        }
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

/// A minimal zip writer: stored (uncompressed) entries, UTF-8 names, fixed timestamps.
#[derive(Default)]
pub struct Zip {
    data: Vec<u8>,
    central: Vec<u8>,
    names: Vec<String>,
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
    fn add_json(&mut self, name: &str, v: &Value) {
        let text = serde_json::to_string_pretty(v).unwrap_or_default();
        self.add(name, redact_lines(&text).as_bytes());
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
