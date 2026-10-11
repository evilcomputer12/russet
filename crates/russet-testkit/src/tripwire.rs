// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`Tripwire`]: a socket that `USBMUXD_SOCKET_ADDRESS` points at, which records every connection attempt
//! (ARCHITECTURE §3.3 item 7 [D 63, D 83]; PLAN W8, the lane-D isolation test).
//!
//! idevice and pymobiledevice3 both read `USBMUXD_SOCKET_ADDRESS` [V QA V12]: on Unix a path (a value with a
//! `:` is a TCP address), on Windows `host:port`. A lane-D daemon with a `scripted:` backend must never talk to
//! the mux, so a test points the variable at a tripwire and asserts afterwards that nothing connected. The
//! tripwire cannot see a direct open of `/run/usbmuxd`; W2's `live_connectors_constructed() == 0` assertion
//! covers that side.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// The variable the tripwire replaces.
pub const ENV: &str = "USBMUXD_SOCKET_ADDRESS";

/// A listening socket that counts connections and answers none of them.
#[derive(Debug)]
pub struct Tripwire {
    addr: OsString,
    path: Option<PathBuf>,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tripwire {
    /// Binds a tripwire: a Unix socket `dir/usbmuxd-tripwire.sock` (Unix) or `127.0.0.1:<port>` (Windows).
    pub fn bind(dir: &Path) -> io::Result<Self> {
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        {
            let path = dir.join("usbmuxd-tripwire.sock");
            let listener = std::os::unix::net::UnixListener::bind(&path)?;
            listener.set_nonblocking(true)?;
            let thread = spawn_accept_loop(move || listener.accept().map(|_| ()), Arc::clone(&hits), Arc::clone(&stop))?;
            Ok(Self { addr: path.clone().into_os_string(), path: Some(path), hits, stop, thread: Some(thread) })
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let addr = listener.local_addr()?.to_string();
            let thread = spawn_accept_loop(move || listener.accept().map(|_| ()), Arc::clone(&hits), Arc::clone(&stop))?;
            Ok(Self { addr: addr.into(), path: None, hits, stop, thread: Some(thread) })
        }
    }

    /// `(USBMUXD_SOCKET_ADDRESS, value)` for a child's environment.
    pub fn env(&self) -> (OsString, OsString) {
        (ENV.into(), self.addr.clone())
    }

    /// The socket path (Unix) or address (Windows).
    pub fn address(&self) -> &OsString {
        &self.addr
    }

    /// How many connections arrived so far.
    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// `Ok` when nothing connected.
    pub fn assert_untouched(&self) -> Result<(), String> {
        match self.hits() {
            0 => Ok(()),
            n => Err(format!(
                "{n} connection(s) reached the usbmuxd tripwire {:?}: a lane-D daemon talked to the device mux \
                 (ARCHITECTURE §3.3 item 7)",
                self.addr
            )),
        }
    }
}

fn spawn_accept_loop(
    mut accept: impl FnMut() -> io::Result<()> + Send + 'static,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name("usbmuxd-tripwire".into()).spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match accept() {
                // The connection is dropped at once: a client sees EOF, and the hit is recorded.
                Ok(()) => {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    })
}

impl Drop for Tripwire {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }
}
