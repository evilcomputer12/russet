// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Host programs for the harnesses, built at test time from sources in this module (never committed binaries):
//! [`JITDEMO_C`] (the RX-page demo, H3) and [`PROBE_C`] (a host RussetProbe). Plus the small ELF and
//! `/proc/<pid>/maps` readers the host servicer needs to find a symbol at run time, and [`ScratchDir`].

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The RX-page demo (RD V8).
pub const JITDEMO_C: &str = include_str!("jitdemo.c");
/// The host stand-in for RussetProbe.
pub const PROBE_C: &str = include_str!("probe.c");
/// The marker whose next line is the probe's breakpoint line [D 84].
pub const PROBE_MARKER: &str = "RUSSET_PROBE_BREAK";

/// A temporary directory, removed on drop.
#[derive(Debug)]
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// A fresh directory under the system temp dir, named after `tag`. The name stays short, so sockets under it
    /// fit `sun_path` (about 100 bytes) with a session id and a file name added.
    pub fn new(tag: &str) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_nanos();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("russet-{tag}-{}-{n}-{:06}", std::process::id(), nanos % 1_000_000));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.path.join(p)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A program built for a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProgram {
    pub path: PathBuf,
    pub source: PathBuf,
}

/// The C compiler: `$CC`, then `cc`, `clang`, `gcc` on `PATH`.
pub fn find_cc() -> Option<PathBuf> {
    if let Some(cc) = std::env::var_os("CC").map(PathBuf::from) {
        return Some(cc);
    }
    let path = std::env::var_os("PATH")?;
    ["cc", "clang", "gcc"]
        .iter()
        .find_map(|name| std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// Builds `source_text` as `dir/name` with `-O0 -g`.
pub fn build(dir: &Path, name: &str, source_text: &str) -> io::Result<HostProgram> {
    let cc = find_cc().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no C compiler (cc, clang, gcc)"))?;
    let source = dir.join(format!("{name}.c"));
    std::fs::write(&source, source_text)?;
    let path = dir.join(name);
    let out = Command::new(cc).args(["-O0", "-g", "-o"]).arg(&path).arg(&source).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("cc failed: {}", String::from_utf8_lossy(&out.stderr))));
    }
    Ok(HostProgram { path, source })
}

pub fn build_jitdemo(dir: &Path) -> io::Result<HostProgram> {
    build(dir, "jitdemo", JITDEMO_C)
}

pub fn build_probe(dir: &Path) -> io::Result<HostProgram> {
    build(dir, "probe", PROBE_C)
}

/// The 1-based line that follows the first line containing `marker`.
pub fn line_after_marker(source_text: &str, marker: &str) -> Option<u32> {
    source_text.lines().position(|l| l.contains(marker)).map(|i| i as u32 + 2)
}

/// `st_value` of the symbol `name` in a 64-bit little-endian ELF file (`.symtab`, then `.dynsym`).
pub fn elf_symbol(path: &Path, name: &str) -> io::Result<Option<u64>> {
    let data = std::fs::read(path)?;
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a 64-bit little-endian ELF file");
    if data.get(..6) != Some(&[0x7f, b'E', b'L', b'F', 2, 1][..]) {
        return Err(bad());
    }
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let u64_at =
        |o: usize| data.get(o..o + 8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]));
    let shoff = u64_at(0x28).ok_or_else(bad)? as usize;
    let shentsize = usize::from(u16_at(0x3A).ok_or_else(bad)?);
    let shnum = usize::from(u16_at(0x3C).ok_or_else(bad)?);
    let section = |i: usize| -> Option<(u32, usize, usize, u32)> {
        let base = shoff + i * shentsize;
        Some((u32_at(base + 4)?, u64_at(base + 24)? as usize, u64_at(base + 32)? as usize, u32_at(base + 40)?))
    };
    for wanted in [2u32, 11] {
        for i in 0..shnum {
            let Some((kind, offset, size, link)) = section(i) else { continue };
            if kind != wanted {
                continue;
            }
            let Some((_, str_off, str_size, _)) = section(link as usize) else { continue };
            let strtab = data.get(str_off..str_off + str_size).ok_or_else(bad)?;
            for sym in (offset..offset + size).step_by(24) {
                let Some(name_off) = u32_at(sym) else { break };
                let Some(rest) = strtab.get(name_off as usize..) else { continue };
                let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
                if &rest[..end] == name.as_bytes() {
                    return Ok(u64_at(sym + 8));
                }
            }
        }
    }
    Ok(None)
}

/// Where `exe` is mapped in process `pid` (the start of its offset-0 mapping in `/proc/<pid>/maps`).
pub fn proc_load_base(pid: u32, exe: &Path) -> io::Result<Option<u64>> {
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps"))?;
    let exe = std::fs::canonicalize(exe)?;
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(range), Some(_perms), Some(offset)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let path = fields.nth(2).map(PathBuf::from);
        if path.as_deref() == Some(exe.as_path()) && u64::from_str_radix(offset, 16) == Ok(0) {
            let start = range.split('-').next().and_then(|s| u64::from_str_radix(s, 16).ok());
            return Ok(start);
        }
    }
    Ok(None)
}

/// Whether process `pid` exists and is not a zombie (Linux `/proc`).
pub fn process_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state is the field after the parenthesised command name.
        Ok(stat) => stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|s| s != "Z" && s != "X"),
        Err(_) => false,
    }
}

/// Sends SIGKILL to `pid` (best effort; through `kill(1)`, since this crate has no libc binding).
pub fn kill_process(pid: u32) {
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).stderr(std::process::Stdio::null()).status();
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_probe_breakpoint_is_the_line_after_the_marker() {
        let line = line_after_marker(PROBE_C, PROBE_MARKER).unwrap();
        let text = PROBE_C.lines().nth(line as usize - 1).unwrap();
        assert!(text.contains("return probeValue + seq;"), "{text}");
    }

    #[test]
    fn host_programs_build_and_expose_their_symbols() {
        if find_cc().is_none() {
            eprintln!("skipping: no C compiler");
            return;
        }
        let dir = ScratchDir::new("programs").unwrap();
        let demo = build_jitdemo(dir.path()).unwrap();
        let notify = elf_symbol(&demo.path, "NOTIFY_DEBUGGER_ABOUT_RX_PAGES").unwrap();
        assert!(notify.is_some_and(|v| v > 0));
        assert_eq!(elf_symbol(&demo.path, "no_such_symbol").unwrap(), None);
        let probe = build_probe(dir.path()).unwrap();
        assert!(elf_symbol(&probe.path, "main").unwrap().is_some());
        // Without a debugger nothing touches the pages: the demo fails, but runs.
        let out = Command::new(&demo.path).arg("1").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("RESULT FAIL"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs Unix processes and a shell; the engine runs in Linux (WSL on Windows)")]
    fn processes_and_maps() {
        let me = std::process::id();
        assert!(process_alive(me));
        let exe = std::env::current_exe().unwrap();
        assert!(proc_load_base(me, &exe).unwrap().is_some());
        assert!(!process_alive(u32::MAX - 1));
    }
}
