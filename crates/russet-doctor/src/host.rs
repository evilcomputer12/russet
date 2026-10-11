// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The host checks of core §11: `host.os`, `host.disk`, `host.memory`, `host.wsl`, `dirs.private`, and
//! `usbmux.available`, which connects to the mux only when the daemon's device backend is `live`; with any other
//! backend it only stats the socket and reports `skip`, so lane D never connects to `/run/usbmuxd`
//! (ARCHITECTURE §3.3 item 7) [D 83].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russet_core::error::{Error, Result, codes};

use crate::{Category, Check, CheckCx, CheckResult, Fix, Requires};

/// The host the checks ran on (the report's `host`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub os: String,
    pub distro: Option<String>,
    pub arch: String,
    pub wsl: bool,
}

/// This host.
pub fn host_info() -> HostInfo {
    HostInfo {
        os: std::env::consts::OS.to_owned(),
        distro: linux_distro(),
        arch: std::env::consts::ARCH.to_owned(),
        wsl: is_wsl(),
    }
}

/// The `ID` of `/etc/os-release` (Linux).
pub fn linux_distro() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines().find_map(|l| l.strip_prefix("ID=")).map(|v| v.trim_matches('"').to_owned())
}

/// Whether this Linux runs under WSL.
pub fn is_wsl() -> bool {
    cfg!(target_os = "linux")
        && std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|r| r.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

/// The default reserve of `storage.min-free-space`.
pub const DEFAULT_MIN_FREE: u64 = 5 << 30;

struct HostOs;

#[async_trait]
impl Check for HostOs {
    fn id(&self) -> &'static str {
        "host.os"
    }
    fn title(&self) -> &'static str {
        "Operating system"
    }
    fn category(&self) -> Category {
        Category::Host
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        let info = host_info();
        let what = format!("{} {}{}", info.os, info.arch, info.distro.map(|d| format!(" ({d})")).unwrap_or_default());
        match (info.os.as_str(), info.arch.as_str()) {
            ("linux", "x86_64" | "aarch64") => CheckResult::pass(what),
            ("windows", "x86_64" | "aarch64") => CheckResult::pass(what),
            _ => CheckResult::fail(format!("{what} is not supported"))
                .with_hint("Russet runs on Linux (x86_64, aarch64) and Windows 10 1809+ or 11."),
        }
    }
}

struct HostDisk;

#[async_trait]
impl Check for HostDisk {
    fn id(&self) -> &'static str {
        "host.disk"
    }
    fn title(&self) -> &'static str {
        "Free disk space for Russet's data and cache"
    }
    fn category(&self) -> Category {
        Category::Host
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let Some(dirs) = &cx.dirs else { return CheckResult::skip("Russet's directories are not known") };
        let reserve = cx.min_free_space.unwrap_or(DEFAULT_MIN_FREE);
        let mut lowest: Option<(u64, PathBuf)> = None;
        for d in [&dirs.data, &dirs.cache] {
            let Some(existing) = d.ancestors().find(|a| a.exists()) else { continue };
            match fs4::available_space(existing) {
                Ok(free) => {
                    if lowest.as_ref().is_none_or(|(f, _)| free < *f) {
                        lowest = Some((free, d.clone()));
                    }
                }
                Err(e) => return CheckResult::warn(format!("cannot measure free space on {}: {e}", d.display())),
            }
        }
        let Some((free, at)) = lowest else { return CheckResult::skip("no existing directory to measure") };
        let gib = |b: u64| b as f64 / f64::from(1u32 << 30);
        let msg = format!("{:.1} GiB free on {} (reserve {:.1} GiB)", gib(free), at.display(), gib(reserve));
        let hint = "Free some space: `russet toolchain gc`, `russet sdk remove`, or `cargo sweep` in development.";
        if free < reserve {
            CheckResult::fail(msg).with_hint(hint)
        } else if free < reserve.saturating_mul(4) {
            CheckResult::warn(msg).with_hint(hint)
        } else {
            CheckResult::pass(msg)
        }
    }
    fn fix_kind(&self) -> Fix {
        Fix::Manual
    }
}

struct HostMemory;

/// Total memory in bytes (Linux `/proc/meminfo`).
fn total_memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 =
        text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kib * 1024)
}

#[async_trait]
impl Check for HostMemory {
    fn id(&self) -> &'static str {
        "host.memory"
    }
    fn title(&self) -> &'static str {
        "Memory"
    }
    fn category(&self) -> Category {
        Category::Host
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        match total_memory() {
            Some(b) if b < 8 << 30 => CheckResult::warn(format!("{} MiB of memory", b >> 20))
                .with_hint("Swift builds and LLDB need at least 8 GiB; close other programs while building."),
            Some(b) => CheckResult::pass(format!("{:.1} GiB of memory", b as f64 / f64::from(1u32 << 30))),
            None => CheckResult::skip("total memory is not readable on this platform"),
        }
    }
}

struct HostWsl;

#[async_trait]
impl Check for HostWsl {
    fn id(&self) -> &'static str {
        "host.wsl"
    }
    fn title(&self) -> &'static str {
        "Windows Subsystem for Linux"
    }
    fn category(&self) -> Category {
        Category::Host
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, _cx: &CheckCx) -> CheckResult {
        if !cfg!(target_os = "linux") {
            return CheckResult::skip("not Linux");
        }
        if is_wsl() {
            CheckResult::warn("running under WSL: the iPhone is attached to Windows").with_hint(
                "Run russetd on Windows next to the phone and build in WSL (split mode, `[daemon] listen-tcp`).",
            )
        } else {
            CheckResult::pass("not WSL")
        }
    }
    fn fix_kind(&self) -> Fix {
        Fix::Manual
    }
}

struct DirsPrivate;

#[cfg(unix)]
fn dir_problem(d: &std::path::Path) -> Option<String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = match std::fs::metadata(d) {
        Ok(m) => m,
        Err(_) => return Some(format!("{} does not exist yet", d.display())),
    };
    if !meta.is_dir() {
        return Some(format!("{} is not a directory", d.display()));
    }
    // The current user is the owner of this process's /proc entry (Linux); elsewhere the owner is not checked.
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    if me.is_some_and(|uid| uid != meta.uid()) {
        return Some(format!("{} belongs to uid {}, not to this user", d.display(), meta.uid()));
    }
    let mode = meta.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| format!("{} is mode {mode:o}; Russet needs 0700", d.display()))
}

#[async_trait]
impl Check for DirsPrivate {
    fn id(&self) -> &'static str {
        "dirs.private"
    }
    fn title(&self) -> &'static str {
        "Russet's directories are private"
    }
    fn category(&self) -> Category {
        Category::Host
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let Some(dirs) = &cx.dirs else { return CheckResult::skip("Russet's directories are not known") };
        #[cfg(unix)]
        {
            let all = [&dirs.config, &dirs.data, &dirs.state, &dirs.cache, &dirs.runtime];
            let problems: Vec<String> = all.iter().filter_map(|d| dir_problem(d)).collect();
            let missing_only = problems.iter().all(|p| p.ends_with("does not exist yet"));
            if problems.is_empty() {
                CheckResult::pass("config, data, state, cache and runtime are 0700 and owned by this user")
            } else if missing_only {
                CheckResult::warn(problems.join("; "))
                    .with_hint("The daemon creates them on first start; `russet doctor --fix` creates them now.")
            } else {
                CheckResult::fail(problems.join("; ")).with_hint("Run `russet doctor --fix` to make them 0700.")
            }
        }
        #[cfg(not(unix))]
        {
            let _ = dirs;
            CheckResult::skip("user-only ACLs are checked on Windows by W27a")
        }
    }
    async fn fix(&self, cx: &CheckCx) -> Result<()> {
        let dirs = cx.dirs.as_ref().ok_or_else(|| {
            Error::new(codes::core::INTERNAL, "Russet's directories are not known").with_hint("Report this bug.")
        })?;
        for d in [&dirs.config, &dirs.data, &dirs.state, &dirs.cache, &dirs.runtime] {
            std::fs::create_dir_all(d).map_err(|e| {
                Error::new(codes::core::IO, format!("cannot create {}: {e}", d.display()))
                    .with_hint("Check the parent directory's permissions.")
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
                    Error::new(codes::core::IO, format!("cannot protect {}: {e}", d.display()))
                        .with_hint("Check that you own the directory.")
                })?;
            }
        }
        Ok(())
    }
    fn fix_kind(&self) -> Fix {
        Fix::Auto
    }
}

struct UsbmuxAvailable;

/// Where the mux listens: `USBMUXD_SOCKET_ADDRESS` (`unix:<path>`, a path, or `host:port`), else the platform
/// default (`/run/usbmuxd`, or Apple Mobile Device Service on `127.0.0.1:27015` on Windows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxAddress {
    Unix(PathBuf),
    Tcp(String),
}

/// Parses `USBMUXD_SOCKET_ADDRESS`.
pub fn mux_address(configured: Option<&str>) -> MuxAddress {
    match configured.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if s.starts_with("unix:") => MuxAddress::Unix(PathBuf::from(&s["unix:".len()..])),
        Some(s) if s.starts_with('/') => MuxAddress::Unix(PathBuf::from(s)),
        Some(s) => MuxAddress::Tcp(s.trim_start_matches("tcp:").to_owned()),
        None if cfg!(windows) => MuxAddress::Tcp("127.0.0.1:27015".into()),
        None => MuxAddress::Unix(PathBuf::from("/run/usbmuxd")),
    }
}

fn describe(a: &MuxAddress) -> String {
    match a {
        MuxAddress::Unix(p) => p.display().to_string(),
        MuxAddress::Tcp(t) => t.clone(),
    }
}

fn mux_fix_command() -> &'static str {
    if cfg!(windows) {
        "Install Apple Devices (or iTunes) from the Microsoft Store"
    } else {
        "sudo systemctl enable --now usbmuxd"
    }
}

#[async_trait]
impl Check for UsbmuxAvailable {
    fn id(&self) -> &'static str {
        "usbmux.available"
    }
    fn title(&self) -> &'static str {
        "usbmuxd (the USB device mux)"
    }
    fn category(&self) -> Category {
        Category::Usbmux
    }
    fn requires(&self) -> Requires {
        Requires::default()
    }
    async fn run(&self, cx: &CheckCx) -> CheckResult {
        let addr = mux_address(cx.usbmux_address.as_deref());
        // Without a known backend the mux is never contacted: only a `live` daemon may reach it [D 83].
        let Some(backend) = cx.device_backend.clone() else {
            return CheckResult::skip("the daemon's device backend is unknown; the mux is not contacted");
        };
        if backend != "live" || cx.live_refused {
            // Never connect: lane D's daemon must not touch the phone's mux [D 83].
            let state = match &addr {
                MuxAddress::Unix(p) if std::fs::symlink_metadata(p).is_ok() => format!("{} exists", p.display()),
                MuxAddress::Unix(p) => format!("{} does not exist", p.display()),
                MuxAddress::Tcp(t) => format!("{t} (TCP)"),
            };
            let why = if cx.live_refused {
                "the live backend is refused in this development build (RUSSET_LANE is not A)".to_owned()
            } else {
                format!("device backend is {backend}")
            };
            return CheckResult::skip(format!("{why}; the mux is not contacted ({state})"));
        }
        let res = match &addr {
            #[cfg(unix)]
            MuxAddress::Unix(p) => tokio::time::timeout(Duration::from_secs(2), tokio::net::UnixStream::connect(p))
                .await
                .map(|r| r.map(drop)),
            #[cfg(not(unix))]
            MuxAddress::Unix(_) => Ok(Err(std::io::Error::other("Unix sockets are not available here"))),
            MuxAddress::Tcp(t) => {
                tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(t.as_str()))
                    .await
                    .map(|r| r.map(drop))
            }
        };
        match res {
            Ok(Ok(())) => CheckResult::pass(format!("{} accepts connections", describe(&addr))),
            Ok(Err(e)) => CheckResult::fail(format!("cannot connect to {}: {e}", describe(&addr)))
                .with_hint(format!("Start usbmuxd: {}", mux_fix_command())),
            Err(_) => CheckResult::fail(format!("{} did not answer within 2 s", describe(&addr)))
                .with_hint(format!("Restart usbmuxd: {}", mux_fix_command())),
        }
    }
    fn fix_kind(&self) -> Fix {
        if cfg!(windows) { Fix::Manual } else { Fix::Command(mux_fix_command().to_owned()) }
    }
}

/// The host checks.
pub fn checks() -> Vec<Arc<dyn Check>> {
    vec![
        Arc::new(HostOs),
        Arc::new(HostDisk),
        Arc::new(HostMemory),
        Arc::new(HostWsl),
        Arc::new(DirsPrivate),
        Arc::new(UsbmuxAvailable),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_addresses_parse() {
        assert_eq!(mux_address(Some("unix:/tmp/m.sock")), MuxAddress::Unix("/tmp/m.sock".into()));
        assert_eq!(mux_address(Some("/tmp/m.sock")), MuxAddress::Unix("/tmp/m.sock".into()));
        assert_eq!(mux_address(Some("127.0.0.1:27015")), MuxAddress::Tcp("127.0.0.1:27015".into()));
        if cfg!(unix) {
            assert_eq!(mux_address(None), MuxAddress::Unix("/run/usbmuxd".into()));
        }
    }
}
