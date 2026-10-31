// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Daemon panic reports (`state/panics/<ts>.txt` [D 42]; core §14): the hook writes the build id, the
//! message, the location and a backtrace before unwinding, so a crashed task leaves evidence while the daemon
//! keeps running.
//!
//! [`catch_expected`] runs code whose panic is not a bug in the daemon: until every F0 stub is filled, the
//! constructors of other workstreams' services end in `todo!()`. Such a panic becomes an error for that call
//! and writes no report.

use std::cell::Cell;
use std::io::Write;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::PathBuf;
use std::sync::OnceLock;

thread_local! {
    static EXPECTED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f`, catching a panic without a report.
pub fn catch_expected<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    EXPECTED.with(|e| e.set(true));
    let r = std::panic::catch_unwind(AssertUnwindSafe(f));
    EXPECTED.with(|e| e.set(false));
    r
}

/// The `todo!()` text of an F0 stub panic, if the payload is one.
pub fn not_implemented_message(payload: &(dyn std::any::Any + Send)) -> String {
    russet_core::task::panic_message(payload)
}

/// Installs the hook that writes reports into `dir` (once per process; later calls change the directory).
pub fn install_hook(dir: PathBuf) {
    static DIR: OnceLock<std::sync::Mutex<PathBuf>> = OnceLock::new();
    let slot = DIR.get_or_init(|| std::sync::Mutex::new(dir.clone()));
    *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if EXPECTED.with(Cell::get) {
                return;
            }
            let dir = DIR.get().map(|d| d.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone());
            if let Some(dir) = dir {
                let _ = write_report(&dir, info);
            }
            default(info);
        }));
    });
}

fn write_report(dir: &std::path::Path, info: &PanicHookInfo<'_>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let now = russet_core::time::now_rfc3339();
    let path = dir.join(format!("{}-{}.txt", now.replace(':', "-"), std::process::id()));
    let message = russet_core::redact::redact_text(&russet_core::task::panic_message(info.payload())).into_owned();
    let location = info.location().map_or_else(|| "unknown".to_owned(), ToString::to_string);
    let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
    let backtrace = std::backtrace::Backtrace::force_capture();
    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "russetd panic report")?;
    writeln!(f, "time: {now}")?;
    writeln!(f, "build: {}", russet_core::build_info::BUILD_ID)?;
    writeln!(f, "pid: {}", std::process::id())?;
    writeln!(f, "thread: {thread}")?;
    writeln!(f, "location: {location}")?;
    writeln!(f, "message: {message}")?;
    writeln!(f, "\n{backtrace}")?;
    Ok(path)
}
