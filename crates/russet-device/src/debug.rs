// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Port forwards and debug launch helpers (DEV-08; ARCHITECTURE §10.4, device.md §6.6, §14).
//!
//! A forward listens on a Unix socket `runtime/fwd-<lease>.sock` (0600 in the 0700 runtime directory, Linux) or on
//! TCP loopback, and for each accepted connection opens the device target through the connector: an RSD service
//! (its port re-resolved per tunnel generation, so a tunnel rebuild only affects live connections), a TCP port
//! inside the tunnel, or a classic usbmux port. The debug plane uses [`DeviceHandle::connect_rsd`] for its broker
//! and these forwards for the legacy-async attach mode.
//!
//! **Safety of caller-chosen paths** (`--bind unix:PATH`). Russet creates and restricts only its own runtime
//! directory. A caller's path must be absolute and its directory must exist; Russet never creates or `chmod`s it.
//! An existing file there is never replaced: a stale socket (nobody listens) is removed, anything else is refused.
//! The socket is bound 0600 inside the runtime directory and hard-linked into place (`link(2)` never replaces an
//! entry), so it is never reachable with looser permissions; on another filesystem it is bound in place and
//! restricted right after. Closing the forward removes the path only while it is still the socket Russet bound.
//!
//! **gdb-remote over TCP** (device.md §14). debugserver has no authentication, and loopback TCP is open to every
//! local user. A TCP forward to a debugserver service therefore accepts one connection (the listener closes after
//! it, and the forward ends with that connection) and closes when nobody connects within 60 s. Unix sockets need
//! neither: their 0600 mode keeps other users out.

use std::collections::BTreeMap;
use std::net::SocketAddr;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use russet_core::Result;
use russet_core::error::codes;
use russet_core::ids::LeaseId;
use time::OffsetDateTime;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::error::{device_error, invalid_argument};
use crate::manager::DeviceHandle;
use crate::model::Udid;

/// How long a TCP forward to debugserver waits for its one client (device.md §14).
pub const DEBUGSERVER_CONNECT_DEADLINE: Duration = Duration::from_secs(60);

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

impl ForwardTarget {
    /// Whether the target speaks unauthenticated gdb-remote: `com.apple.internal.dt.remote.debugproxy`,
    /// `com.apple.coredevice.debugserverproxy` and their kin (device.md §6.6).
    pub fn is_debugserver(&self) -> bool {
        match self {
            Self::RsdService(name) => name.contains("debugproxy") || name.contains("debugserver"),
            Self::TunnelPort(_) | Self::UsbmuxPort(_) => false,
        }
    }
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
    /// Loopback only.
    Tcp(SocketAddr),
    /// An absolute socket path in an existing directory (see the module notes).
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

    fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }
}

/// When a forward stops accepting connections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcceptPolicy {
    /// Close the listener after the first connection; the forward ends with that connection.
    pub accept_once: bool,
    /// End the forward when nobody connected by then.
    pub first_connect_deadline: Option<Duration>,
}

impl AcceptPolicy {
    /// No limits.
    pub const OPEN: Self = Self { accept_once: false, first_connect_deadline: None };

    /// The default for a target and its local end: accept-once and the 60 s deadline for gdb-remote over TCP
    /// (device.md §14), no limits otherwise (Unix sockets are 0600; Flutter's VM service needs several
    /// connections, ARCHITECTURE §10.4).
    pub fn for_forward(target: &ForwardTarget, local: &LocalEndpoint) -> Self {
        if local.is_tcp() && target.is_debugserver() {
            Self { accept_once: true, first_connect_deadline: Some(DEBUGSERVER_CONNECT_DEADLINE) }
        } else {
            Self::OPEN
        }
    }
}

/// How a forward is opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardOptions {
    /// Where to listen; `None` picks the platform default (a Unix socket named after the lease on Linux, loopback
    /// TCP elsewhere).
    pub bind: Option<BindSpec>,
    /// The connection that opened the forward (`c-7`). Only it may close the forward with `forward.close`
    /// (ARCHITECTURE §6.3: a lease is released by its owner). `None`: a forward the daemon opened for itself.
    pub owner: Option<String>,
    /// Overrides [`AcceptPolicy::for_forward`].
    pub policy: Option<AcceptPolicy>,
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
    /// The connection that opened it; `None` for the daemon's own forwards.
    pub owner: Option<String>,
    pub policy: AcceptPolicy,
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

    /// Closes one forward whatever its owner (lease end, detach, shutdown); `false` when the lease is unknown.
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

    /// `forward.close` from connection `conn`: only the forward's owner may close it (ARCHITECTURE §6.3; `system`
    /// callers end any lease with `lease.release {force}`). `core.notFound` for an unknown lease, `perm.denied` for
    /// another connection's forward.
    pub fn close_owned(&self, lease: &LeaseId, conn: &str) -> Result<()> {
        let mut map = self
            .map
            .lock()
            .map_err(|_| russet_core::Error::new(codes::core::INTERNAL, "the forward table is poisoned"))?;
        let Some(e) = map.get(lease.as_str()) else {
            return Err(russet_core::Error::new(codes::core::NOT_FOUND, format!("no forward with lease {lease}"))
                .with_hint("`russet forward list` (forward.list) shows the open forwards."));
        };
        if e.info.owner.as_deref() != Some(conn) {
            return Err(russet_core::Error::new(
                codes::perm::DENIED,
                format!("forward {lease} belongs to another connection"),
            )
            .with_hint(
                "Only the connection that opened a forward closes it; it also ends when that connection goes. \
                 A system-class client can end it with `lease.release` and `force`.",
            ));
        }
        if let Some(e) = map.remove(lease.as_str()) {
            e.stop.cancel();
        }
        Ok(())
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

    /// The forward listening on `path`, if any.
    #[cfg(unix)]
    fn listening_on(&self, path: &Path) -> Option<LeaseId> {
        self.map.lock().ok().and_then(|m| {
            m.values()
                .find(|e| matches!(&e.info.local, LocalEndpoint::Unix(p) if p == path))
                .map(|e| e.info.lease.clone())
        })
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
    /// Resolves when the forward was closed (by `forward.close`, a detach, the accept policy, or shutdown).
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
    /// The socket path goes when the listener does (`_socket`'s drop).
    #[cfg(unix)]
    Unix {
        listener: tokio::net::UnixListener,
        _socket: BoundSocket,
    },
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
            Self::Unix { listener, .. } => {
                let (s, _) = listener.accept().await?;
                Ok(Box::new(s))
            }
        }
    }
}

/// A socket path Russet bound, with the inode it got: on close the path is removed only while it still names that
/// socket, so a file someone put there since is never deleted.
#[cfg(unix)]
struct BoundSocket {
    path: PathBuf,
    id: Option<(u64, u64)>,
}

#[cfg(unix)]
impl BoundSocket {
    fn new(path: PathBuf) -> Self {
        let id = unix_bind::inode(&path);
        Self { path, id }
    }
}

#[cfg(unix)]
impl Drop for BoundSocket {
    fn drop(&mut self) {
        if self.id.is_some() && unix_bind::inode(&self.path) == self.id {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
mod unix_bind {
    use std::io::ErrorKind;
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::Path;

    use russet_core::Result;
    use russet_core::error::codes;

    use crate::error::{device_error, invalid_argument};

    fn io(e: std::io::Error, what: String) -> russet_core::Error {
        device_error(codes::device::IO, format!("{what}: {e}")).caused_by(e)
    }

    /// `(dev, ino)` of the entry at `path` itself (no symlink following).
    pub(super) fn inode(path: &Path) -> Option<(u64, u64)> {
        std::fs::symlink_metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }

    /// Creates Russet's runtime directory 0700, or tightens it to 0700: the rule of russet-core's
    /// `fsx::ensure_private_dir` (W2), which replaces this once it lands. Only ever called for the directory Russet
    /// owns, never for a directory a caller named.
    pub(super) fn ensure_runtime_dir(dir: &Path) -> Result<()> {
        if dir.as_os_str().is_empty() {
            return Err(device_error(codes::device::IO, "the device plane has no runtime directory"));
        }
        if !dir.is_dir() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| io(e, format!("cannot create {}", dir.display())))?;
        }
        let meta = std::fs::metadata(dir).map_err(|e| io(e, format!("cannot read {}", dir.display())))?;
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| io(e, format!("cannot restrict {} to 0700", dir.display())))?;
        }
        Ok(())
    }

    /// Makes `path` free for a new socket. Nothing there: fine. A socket nobody listens on (left by a crashed
    /// process): removed. A live socket or any other file: refused, never replaced.
    pub(super) fn clear_stale_socket(path: &Path) -> Result<()> {
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e, format!("cannot inspect {}", path.display()))),
            Ok(m) if m.file_type().is_socket() => {
                if std::os::unix::net::UnixStream::connect(path).is_ok() {
                    return Err(invalid_argument(format!(
                        "{} is a socket another program listens on; choose another path",
                        path.display()
                    )));
                }
                std::fs::remove_file(path)
                    .map_err(|e| io(e, format!("cannot remove the stale socket {}", path.display())))
            }
            Ok(_) => Err(invalid_argument(format!(
                "{} already exists and is not a socket; Russet does not replace it",
                path.display()
            ))
            .with_hint("Pass a path that does not exist yet, or omit --bind for a private socket.")),
        }
    }

    /// Restricts a freshly bound socket to its owner.
    pub(super) fn restrict(path: &Path) -> Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| io(e, format!("cannot restrict {}", path.display())))
    }

    /// Binds `path` directly (it must not exist) and restricts it.
    pub(super) fn bind_in_place(path: &Path) -> Result<tokio::net::UnixListener> {
        let l =
            tokio::net::UnixListener::bind(path).map_err(|e| io(e, format!("cannot listen on {}", path.display())))?;
        restrict(path)?;
        Ok(l)
    }

    /// Binds `path` without ever exposing it with looser permissions: bound and restricted under `stage` (in the
    /// 0700 runtime directory), then hard-linked into place (`link(2)` fails rather than replace an entry). Across
    /// filesystems it falls back to [`bind_in_place`].
    pub(super) fn bind_staged(path: &Path, stage: &Path) -> Result<tokio::net::UnixListener> {
        clear_stale_socket(stage)?;
        let l = bind_in_place(stage)?;
        let linked = std::fs::hard_link(stage, path);
        let _ = std::fs::remove_file(stage);
        match linked {
            Ok(()) => Ok(l),
            Err(e) if e.kind() == ErrorKind::CrossesDevices => {
                drop(l);
                bind_in_place(path)
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(invalid_argument(format!(
                "{} appeared while the forward was opening; Russet does not replace it",
                path.display()
            ))),
            Err(e) => Err(io(e, format!("cannot create {}", path.display()))),
        }
    }
}

async fn bind(h: &DeviceHandle, lease: &LeaseId, b: Option<BindSpec>) -> Result<(Listener, LocalEndpoint)> {
    let spec = match b {
        Some(s) => s,
        #[cfg(unix)]
        None => {
            let dir = &h.cfg().runtime_dir;
            unix_bind::ensure_runtime_dir(dir)?;
            let path = dir.join(format!("fwd-{lease}.sock"));
            unix_bind::clear_stale_socket(&path)?;
            let l = unix_bind::bind_in_place(&path)?;
            let listener = Listener::Unix { listener: l, _socket: BoundSocket::new(path.clone()) };
            return Ok((listener, LocalEndpoint::Unix(path)));
        }
        #[cfg(not(unix))]
        None => {
            let _ = (lease, h);
            BindSpec::Tcp(SocketAddr::from(([127, 0, 0, 1], 0)))
        }
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
            if !path.is_absolute() {
                return Err(invalid_argument(format!(
                    "{} is not an absolute path (ARCHITECTURE §7.3)",
                    path.display()
                )));
            }
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or_else(|| invalid_argument(format!("{} has no parent directory", path.display())))?;
            if !parent.is_dir() {
                return Err(invalid_argument(format!("the directory {} does not exist", parent.display()))
                    .with_hint("Create it first; Russet creates only its own runtime directory."));
            }
            if let Some(other) = h.shared.forwards.listening_on(&path) {
                return Err(invalid_argument(format!("forward {other} already listens on {}", path.display())));
            }
            unix_bind::clear_stale_socket(&path)?;
            let dir = &h.cfg().runtime_dir;
            unix_bind::ensure_runtime_dir(dir)?;
            let l = unix_bind::bind_staged(&path, &dir.join(format!(".bind-{lease}.sock")))?;
            Ok((Listener::Unix { listener: l, _socket: BoundSocket::new(path.clone()) }, LocalEndpoint::Unix(path)))
        }
    }
}

/// Opens a forward and registers it under a fresh lease.
pub async fn open_forward(h: &DeviceHandle, t: ForwardTarget, b: BindSpec) -> Result<ForwardHandle> {
    open_forward_opts(h, t, ForwardOptions { bind: Some(b), ..Default::default() }).await
}

/// Like [`open_forward`]; `None` picks the platform default bind (a Unix socket named after the lease on Linux).
pub async fn open_forward_with(h: &DeviceHandle, t: ForwardTarget, b: Option<BindSpec>) -> Result<ForwardHandle> {
    open_forward_opts(h, t, ForwardOptions { bind: b, ..Default::default() }).await
}

/// Resolves at `deadline`, or never.
async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

/// Relays one accepted connection to the device target until either side closes or the forward ends.
async fn relay(h: DeviceHandle, t: ForwardTarget, stop: CancellationToken, mut sock: Box<dyn Duplex>) {
    let started = Instant::now();
    let remote = match &t {
        ForwardTarget::RsdService(name) => h.conn().rsd_service(h.udid(), name).await,
        ForwardTarget::TunnelPort(p) => h.conn().tunnel_port(h.udid(), *p).await,
        ForwardTarget::UsbmuxPort(p) => h.conn().usbmux_port(h.udid(), *p).await,
    };
    let mut remote = match remote {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target = %t, error = %e.message(), "forward: device connect failed");
            return;
        }
    };
    tracing::debug!(target = %t, connect_ms = started.elapsed().as_millis() as u64, "forward connected");
    tokio::select! {
        _ = stop.cancelled() => {}
        r = tokio::io::copy_bidirectional(&mut sock, &mut remote) => {
            tracing::debug!(target = %t, result = ?r.map_err(|e| e.kind()), "forward connection closed");
        }
    }
}

/// Opens a forward with explicit options (owner, bind, accept policy).
pub async fn open_forward_opts(h: &DeviceHandle, t: ForwardTarget, o: ForwardOptions) -> Result<ForwardHandle> {
    if let ForwardTarget::RsdService(name) = &t
        && name.trim().is_empty()
    {
        return Err(invalid_argument("a service name is required"));
    }
    let lease = crate::ids::new_lease_id();
    let (listener, local) = bind(h, &lease, o.bind).await?;
    let policy = o.policy.unwrap_or_else(|| AcceptPolicy::for_forward(&t, &local));
    let stop = CancellationToken::new();
    let connections = Arc::new(AtomicU64::new(0));
    let info = ForwardInfo {
        lease: lease.clone(),
        udid: h.udid().clone(),
        target: t.clone(),
        local: local.clone(),
        connections: 0,
        opened_at: OffsetDateTime::now_utc(),
        owner: o.owner,
        policy,
    };
    h.shared.forwards.insert(info, Arc::clone(&connections), stop.clone());
    let (h2, s2, lease2) = (h.clone(), stop.clone(), lease.clone());
    tokio::spawn(async move {
        let deadline = policy.first_connect_deadline.map(|d| tokio::time::Instant::now() + d);
        let mut listener = Some(listener);
        while let Some(l) = listener.as_ref() {
            let waiting_for_first = connections.load(Ordering::Relaxed) == 0;
            let sock = tokio::select! {
                _ = s2.cancelled() => break,
                _ = until(deadline.filter(|_| waiting_for_first)) => {
                    tracing::info!(target = %t, "forward closed: no client connected within the deadline");
                    break;
                }
                a = l.accept() => a,
            };
            let sock = match sock {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(error = %e, "forward accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            connections.fetch_add(1, Ordering::Relaxed);
            let one = tokio::spawn(relay(h2.clone(), t.clone(), s2.clone(), sock));
            if policy.accept_once {
                // Stop listening: the one client has its connection; the forward ends with it (device.md §14).
                listener = None;
                tokio::select! {
                    _ = s2.cancelled() => {}
                    _ = one => {}
                }
                break;
            }
        }
        h2.shared.forwards.close(&lease2);
        s2.cancel();
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

    #[test]
    fn gdb_remote_over_tcp_accepts_once_with_a_deadline() {
        let tcp = LocalEndpoint::Tcp("127.0.0.1:1".parse().expect("a"));
        let debugproxy = ForwardTarget::RsdService("com.apple.internal.dt.remote.debugproxy".into());
        let coredevice = ForwardTarget::RsdService("com.apple.coredevice.debugserverproxy".into());
        let afc = ForwardTarget::RsdService("com.apple.afc.shim.remote".into());
        let once = AcceptPolicy { accept_once: true, first_connect_deadline: Some(Duration::from_secs(60)) };
        assert_eq!(AcceptPolicy::for_forward(&debugproxy, &tcp), once);
        assert_eq!(AcceptPolicy::for_forward(&coredevice, &tcp), once);
        // Other services and raw ports (Flutter's VM service needs several connections) stay open.
        assert_eq!(AcceptPolicy::for_forward(&afc, &tcp), AcceptPolicy::OPEN);
        assert_eq!(AcceptPolicy::for_forward(&ForwardTarget::UsbmuxPort(8181), &tcp), AcceptPolicy::OPEN);
        // A 0600 Unix socket keeps other users out without limits.
        #[cfg(unix)]
        assert_eq!(
            AcceptPolicy::for_forward(&debugproxy, &LocalEndpoint::Unix(PathBuf::from("/run/x.sock"))),
            AcceptPolicy::OPEN
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_files_are_never_replaced_and_stale_sockets_are() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "keep me").expect("seed");
        let err = unix_bind::clear_stale_socket(&file).expect_err("a regular file");
        assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
        assert_eq!(std::fs::read_to_string(&file).expect("still there"), "keep me");
        // A socket nobody listens on is stale and goes.
        let stale = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).expect("bind"));
        assert!(stale.exists());
        unix_bind::clear_stale_socket(&stale).expect("stale");
        assert!(!stale.exists());
        // A live socket is someone else's.
        let live = dir.path().join("live.sock");
        let _l = std::os::unix::net::UnixListener::bind(&live).expect("bind");
        assert_eq!(unix_bind::clear_stale_socket(&live).expect_err("live").code(), codes::core::INVALID_ARGUMENT);
        assert!(live.exists());
    }
}
