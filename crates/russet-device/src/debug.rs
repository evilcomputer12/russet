// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Port forwards and debug launch helpers (DEV-08; ARCHITECTURE §10.4, device.md §6.6).
//!
//! A forward listens on a Unix socket `runtime/fwd-<lease>.sock` (0600, Linux) or on TCP loopback, and for each
//! accepted connection opens the device target through the connector: an RSD service (its port re-resolved per
//! tunnel generation, so a tunnel rebuild only affects live connections), a TCP port inside the tunnel, or a
//! classic usbmux port. The debug plane uses [`DeviceHandle::connect_rsd`] for its broker and these forwards for
//! the legacy-async attach mode.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use russet_core::Result;
use russet_core::error::codes;
use russet_core::ids::LeaseId;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::error::{device_error, invalid_argument};
use crate::manager::DeviceHandle;
use crate::model::Udid;

/// What a forward connects to on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardTarget {
    /// An RSD service, for example `com.apple.internal.dt.remote.debugproxy`.
    RsdService(String),
    /// Any TCP port on the device's tunnel address.
    TunnelPort(u16),
    /// A classic usbmux connect (iproxy-style), no tunnel.
    UsbmuxPort(u16),
}

impl std::fmt::Display for ForwardTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RsdService(s) => write!(f, "service {s}"),
            Self::TunnelPort(p) => write!(f, "tunnel port {p}"),
            Self::UsbmuxPort(p) => write!(f, "usbmux port {p}"),
        }
    }
}

/// Where a forward listens locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindSpec {
    /// Loopback unless explicitly allowed.
    Tcp(SocketAddr),
    /// Created 0600 under the runtime directory.
    #[cfg(unix)]
    Unix(PathBuf),
}

/// The local end of a forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalEndpoint {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(PathBuf),
}

impl LocalEndpoint {
    /// `127.0.0.1:41250` or the socket path.
    pub fn display(&self) -> String {
        match self {
            Self::Tcp(a) => a.to_string(),
            #[cfg(unix)]
            Self::Unix(p) => p.display().to_string(),
        }
    }

    /// The gdb-remote URL LLDB's `process connect` takes.
    pub fn lldb_url(&self) -> String {
        match self {
            Self::Tcp(a) => format!("connect://{a}"),
            #[cfg(unix)]
            Self::Unix(p) => format!("unix-connect://{}", p.display()),
        }
    }
}

/// A forward as `forward.list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardInfo {
    pub lease: LeaseId,
    pub udid: Udid,
    pub target: ForwardTarget,
    pub local: LocalEndpoint,
    pub connections: u64,
    pub opened_at: OffsetDateTime,
}

struct Entry {
    info: ForwardInfo,
    connections: Arc<AtomicU64>,
    stop: CancellationToken,
}

/// The manager's forwards, by lease.
#[derive(Default)]
pub struct ForwardRegistry {
    map: Mutex<BTreeMap<String, Entry>>,
}

impl std::fmt::Debug for ForwardRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.map.lock().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("ForwardRegistry").field("open", &n).finish()
    }
}

impl ForwardRegistry {
    fn insert(&self, info: ForwardInfo, connections: Arc<AtomicU64>, stop: CancellationToken) {
        if let Ok(mut m) = self.map.lock() {
            m.insert(info.lease.as_str().to_owned(), Entry { info, connections, stop });
        }
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        self.map
            .lock()
            .map(|m| {
                m.values()
                    .map(|e| ForwardInfo { connections: e.connections.load(Ordering::Relaxed), ..e.info.clone() })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Closes one forward; `false` when the lease is unknown.
    pub fn close(&self, lease: &LeaseId) -> bool {
        let e = self.map.lock().ok().and_then(|mut m| m.remove(lease.as_str()));
        match e {
            Some(e) => {
                e.stop.cancel();
                true
            }
            None => false,
        }
    }

    pub fn close_device(&self, udid: &Udid) {
        if let Ok(mut m) = self.map.lock() {
            m.retain(|_, e| {
                let keep = &e.info.udid != udid;
                if !keep {
                    e.stop.cancel();
                }
                keep
            });
        }
    }

    pub fn close_all(&self) {
        if let Ok(mut m) = self.map.lock() {
            for e in m.values() {
                e.stop.cancel();
            }
            m.clear();
        }
    }
}

/// A live forward; dropping it closes the forward.
#[derive(Debug)]
pub struct ForwardHandle {
    pub id: LeaseId,
    pub local: LocalEndpoint,
    stop: CancellationToken,
}

impl ForwardHandle {
    /// Resolves when the forward was closed (by `forward.close`, a detach, or shutdown).
    pub async fn closed(&self) {
        self.stop.cancelled().await;
    }

    /// Closes the forward now.
    pub fn close(&self) {
        self.stop.cancel();
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

enum Listener {
    Tcp(tokio::net::TcpListener),
    #[cfg(unix)]
    Unix(tokio::net::UnixListener, PathBuf),
}

trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

impl Listener {
    async fn accept(&self) -> std::io::Result<Box<dyn Duplex>> {
        match self {
            Self::Tcp(l) => {
                let (s, _) = l.accept().await?;
                let _ = s.set_nodelay(true);
                Ok(Box::new(s))
            }
            #[cfg(unix)]
            Self::Unix(l, _) => {
                let (s, _) = l.accept().await?;
                Ok(Box::new(s))
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Self::Unix(_, path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
fn ensure_private_dir(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)
        .map_err(|e| device_error(codes::device::IO, format!("cannot create {}: {e}", dir.display())))?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    Ok(())
}

async fn bind(h: &DeviceHandle, lease: &LeaseId, b: Option<BindSpec>) -> Result<(Listener, LocalEndpoint)> {
    let spec = match b {
        Some(s) => s,
        #[cfg(unix)]
        None => BindSpec::Unix(h.cfg().runtime_dir.join(format!("fwd-{lease}.sock"))),
        #[cfg(not(unix))]
        None => BindSpec::Tcp(SocketAddr::from(([127, 0, 0, 1], 0))),
    };
    match spec {
        BindSpec::Tcp(addr) => {
            if !addr.ip().is_loopback() {
                return Err(invalid_argument(format!("forwards listen on loopback only, not {addr}")));
            }
            let l = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| device_error(codes::device::IO, format!("cannot listen on {addr}: {e}")))?;
            let local = l.local_addr().map_err(|e| device_error(codes::device::IO, format!("{e}")))?;
            Ok((Listener::Tcp(l), LocalEndpoint::Tcp(local)))
        }
        #[cfg(unix)]
        BindSpec::Unix(path) => {
            use std::os::unix::fs::PermissionsExt;
            if let Some(dir) = path.parent() {
                ensure_private_dir(dir)?;
            }
            let _ = std::fs::remove_file(&path);
            let l = tokio::net::UnixListener::bind(&path)
                .map_err(|e| device_error(codes::device::IO, format!("cannot listen on {}: {e}", path.display())))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| device_error(codes::device::IO, format!("cannot restrict {}: {e}", path.display())))?;
            Ok((Listener::Unix(l, path.clone()), LocalEndpoint::Unix(path)))
        }
    }
}

/// Opens a forward and registers it under a fresh lease.
pub async fn open_forward(h: &DeviceHandle, t: ForwardTarget, b: BindSpec) -> Result<ForwardHandle> {
    open_forward_with(h, t, Some(b)).await
}

/// Like [`open_forward`]; `None` picks the platform default bind (a Unix socket named after the lease on Linux).
pub async fn open_forward_with(h: &DeviceHandle, t: ForwardTarget, b: Option<BindSpec>) -> Result<ForwardHandle> {
    if let ForwardTarget::RsdService(name) = &t
        && name.trim().is_empty()
    {
        return Err(invalid_argument("a service name is required"));
    }
    let lease = crate::ids::new_lease_id();
    let (listener, local) = bind(h, &lease, b).await?;
    let stop = CancellationToken::new();
    let connections = Arc::new(AtomicU64::new(0));
    let info = ForwardInfo {
        lease: lease.clone(),
        udid: h.udid().clone(),
        target: t.clone(),
        local: local.clone(),
        connections: 0,
        opened_at: OffsetDateTime::now_utc(),
    };
    h.shared.forwards.insert(info, Arc::clone(&connections), stop.clone());
    let (h2, s2, lease2) = (h.clone(), stop.clone(), lease.clone());
    tokio::spawn(async move {
        loop {
            let sock = tokio::select! {
                _ = s2.cancelled() => break,
                a = listener.accept() => a,
            };
            let mut sock = match sock {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, "forward accept failed");
                    continue;
                }
            };
            connections.fetch_add(1, Ordering::Relaxed);
            let (h3, t3, s3) = (h2.clone(), t.clone(), s2.clone());
            tokio::spawn(async move {
                let started = Instant::now();
                let remote = match &t3 {
                    ForwardTarget::RsdService(name) => h3.conn().rsd_service(h3.udid(), name).await,
                    ForwardTarget::TunnelPort(p) => h3.conn().tunnel_port(h3.udid(), *p).await,
                    ForwardTarget::UsbmuxPort(p) => h3.conn().usbmux_port(h3.udid(), *p).await,
                };
                let mut remote = match remote {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(target = %t3, error = %e.message(), "forward: device connect failed");
                        return;
                    }
                };
                tracing::debug!(target = %t3, connect_ms = started.elapsed().as_millis() as u64, "forward connected");
                tokio::select! {
                    _ = s3.cancelled() => {}
                    r = tokio::io::copy_bidirectional(&mut sock, &mut remote) => {
                        tracing::debug!(target = %t3, result = ?r.map_err(|e| e.kind()), "forward connection closed");
                    }
                }
            });
        }
        h2.shared.forwards.close(&lease2);
        drop(listener);
    });
    Ok(ForwardHandle { id: lease, local, stop })
}

/// Parses `--bind tcp:ADDR | unix:PATH` (ARCHITECTURE §8.7).
pub fn parse_bind(s: &str) -> Result<BindSpec> {
    if let Some(addr) = s.strip_prefix("tcp:") {
        let addr: SocketAddr = addr.parse().map_err(|_| invalid_argument(format!("{addr:?} is not host:port")))?;
        return Ok(BindSpec::Tcp(addr));
    }
    #[cfg(unix)]
    if let Some(path) = s.strip_prefix("unix:") {
        return Ok(BindSpec::Unix(PathBuf::from(path)));
    }
    Err(invalid_argument(format!("bind {s:?}: expected tcp:ADDR or unix:PATH")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_parse_and_urls_render() {
        assert_eq!(parse_bind("tcp:127.0.0.1:0").expect("tcp"), BindSpec::Tcp("127.0.0.1:0".parse().expect("a")));
        #[cfg(unix)]
        assert_eq!(parse_bind("unix:/run/x.sock").expect("unix"), BindSpec::Unix(PathBuf::from("/run/x.sock")));
        assert!(parse_bind("udp:1").is_err());
        assert_eq!(LocalEndpoint::Tcp("127.0.0.1:41250".parse().expect("a")).lldb_url(), "connect://127.0.0.1:41250");
        #[cfg(unix)]
        assert_eq!(
            LocalEndpoint::Unix(PathBuf::from("/run/user/1000/russet/fwd-x.sock")).lldb_url(),
            "unix-connect:///run/user/1000/russet/fwd-x.sock"
        );
    }
}
