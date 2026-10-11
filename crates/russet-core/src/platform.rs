// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Host facts and small platform helpers (core §5.1 `platform`): the current uid, the executable identity of
//! ARCHITECTURE §3.3 item 3, the resident set size, TTY checks and an echo-free line reader for secret
//! prompts.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// The current user's uid (Unix).
#[cfg(unix)]
pub fn current_uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

/// The host OS as the wire names it (`initialize.result.server.platform`).
pub fn host_os() -> &'static str {
    // `linux`, `windows`, `macos`: Rust's names are the wire names.
    std::env::consts::OS
}

/// The host CPU architecture (`x86_64`, `aarch64`).
pub fn host_arch() -> &'static str {
    std::env::consts::ARCH
}

/// The identity of an executable file (ARCHITECTURE §3.3 item 3): `{dev, ino, mtimeNs, size}` on Unix. On
/// Windows `dev` and `ino` are the volume serial number and file index once W27a reads them; until then they are
/// 0 and the comparison relies on `mtimeNs` and `size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ExecutableId {
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: i128,
    pub size: u64,
}

impl ExecutableId {
    /// The identity of the file at `path` (following symlinks, like the loader did).
    pub fn of(path: &Path) -> crate::Result<Self> {
        let meta = std::fs::metadata(path).map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i128);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self { dev: meta.dev(), ino: meta.ino(), mtime_ns, size: meta.len() })
        }
        #[cfg(not(unix))]
        {
            Ok(Self { dev: 0, ino: 0, mtime_ns, size: meta.len() })
        }
    }
}

/// This process's executable path and its identity, as recorded at daemon start and compared by development
/// clients (ARCHITECTURE §3.3 item 3).
pub fn current_executable() -> crate::Result<(PathBuf, ExecutableId)> {
    let exe = std::env::current_exe().map_err(|e| Error::io("finding the current executable", e))?;
    let id = ExecutableId::of(&exe)?;
    Ok((exe, id))
}

/// The resident set size of this process in bytes, when the platform reports it (`daemon.status`).
pub fn rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Whether stdin is a terminal (`initialize.capabilities.interactive`, ARCHITECTURE §7.4).
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Whether stderr is a terminal (prompts and progress go there).
pub fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

/// Whether stdout is a terminal.
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Reads one line from stdin; with `hide_input` the terminal does not echo it (passwords, 2FA codes). The
/// prompt goes to stderr. The trailing newline is removed.
pub fn read_line_from_tty(prompt: &str, hide_input: bool) -> std::io::Result<String> {
    use std::io::Write;
    let mut stderr = std::io::stderr();
    stderr.write_all(prompt.as_bytes())?;
    stderr.flush()?;
    let guard = if hide_input { EchoOff::new() } else { None };
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    drop(guard);
    if hide_input {
        let _ = stderr.write_all(b"\n");
    }
    read?;
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(line)
}

/// Turns terminal echo off until dropped (Unix; a no-op elsewhere until W27a).
struct EchoOff {
    #[cfg(unix)]
    saved: nix::sys::termios::Termios,
}

impl EchoOff {
    fn new() -> Option<Self> {
        #[cfg(unix)]
        {
            use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
            let stdin = std::io::stdin();
            let saved = tcgetattr(&stdin).ok()?;
            let mut quiet = saved.clone();
            quiet.local_flags.remove(LocalFlags::ECHO);
            quiet.local_flags.insert(LocalFlags::ECHONL);
            tcsetattr(&stdin, SetArg::TCSANOW, &quiet).ok()?;
            Some(Self { saved })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use nix::sys::termios::{SetArg, tcsetattr};
            let _ = tcsetattr(std::io::stdin(), SetArg::TCSANOW, &self.saved);
        }
    }
}

/// Starts a new session for a detached daemon (`setsid`), so it has no controlling terminal and survives the
/// terminal that started it (core §6.3). A no-op on Windows, where the spawner sets `DETACHED_PROCESS`.
pub fn detach_from_terminal() {
    #[cfg(unix)]
    {
        let _ = nix::unistd::setsid();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_identity_changes_when_the_file_is_replaced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("bin");
        std::fs::write(&p, b"one").expect("write");
        let a = ExecutableId::of(&p).expect("stat");
        assert_eq!(a, ExecutableId::of(&p).expect("stat"), "stable while unchanged");
        // A rebuild writes a new file and renames it over the old one: a new inode.
        let next = tmp.path().join("bin.new");
        std::fs::write(&next, b"two!").expect("write");
        std::fs::rename(&next, &p).expect("rename");
        let b = ExecutableId::of(&p).expect("stat");
        assert_ne!(a, b);
        assert_eq!(b.size, 4);
    }

    #[test]
    fn this_process_has_an_identity_and_a_resident_set() {
        let (exe, id) = current_executable().expect("current exe");
        assert!(exe.is_absolute());
        assert!(id.size > 0);
        #[cfg(target_os = "linux")]
        assert!(rss_bytes().is_some_and(|r| r > 0));
        assert!(!host_os().is_empty() && !host_arch().is_empty());
    }
}
