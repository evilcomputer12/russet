// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Transports (ARCHITECTURE §7.1; core §7.1, §6.8).
//!
//! | Transport | Address | Access control |
//! |---|---|---|
//! | Unix socket | `runtime/russetd.sock`; paths over 107 bytes fall back to `/tmp/russet-<uid>/<hash8>.sock` | directory 0700 and socket 0600; the server checks `peer_cred().uid()`, the client the directory owner and the server's uid |
//! | Named pipe | `\\.\pipe\russet-<SID>[-<hash8>]` | explicit DACL, `reject_remote_clients`, `first_pipe_instance`; the client checks the server's SID |
//! | TCP (WSL split mode) | `tcp://127.0.0.1:<port>`, token in `runtime/tcp.json` | `initialize.params.auth.token` (constant-time), 2 s to authenticate, loopback unless `--allow-nonlocal` |

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use secrecy::{ExposeSecret, SecretString};
use tokio::net::{TcpListener, TcpStream};

use crate::peer::{Peer, Role};
use crate::{PeerIdentity, TransportKind};

#[cfg(windows)]
pub(crate) mod windows;

/// The longest Unix socket path (`sun_path` holds 108 bytes including the terminator).
pub const MAX_SOCKET_PATH: usize = 107;

/// Where a daemon listens (ARCHITECTURE §7.1).
#[derive(Debug)]
pub enum Endpoint {
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
    /// WSL split mode only (opt-in).
    Tcp { addr: SocketAddr, token: SecretString },
}

impl Endpoint {
    /// The default endpoint for `dirs` (`$RUSSET_HOME/runtime/russetd.sock`, …).
    pub fn default_for(dirs: &RussetDirs) -> Self {
        #[cfg(unix)]
        {
            Self::Unix(unix_socket_path(&dirs.socket_path()))
        }
        #[cfg(windows)]
        {
            Self::NamedPipe(windows::default_pipe_name(dirs))
        }
    }

    /// The endpoint `RUSSET_DAEMON` names, or the default. `RUSSET_DAEMON=inproc` is the caller's business
    /// (`--no-daemon`) and is refused here.
    pub fn from_env(dirs: &RussetDirs) -> Result<Self> {
        match std::env::var("RUSSET_DAEMON") {
            Ok(v) if !v.trim().is_empty() => Self::parse(v.trim()),
            _ => Ok(Self::default_for(dirs)),
        }
    }

    /// Parses `unix:/path`, `pipe:\\.\pipe\name` or `tcp://127.0.0.1:7777#<token-file>`.
    pub fn parse(s: &str) -> Result<Self> {
        let bad = |why: &str| {
            Error::invalid_argument(format!("`{s}` is not a daemon endpoint: {why}"))
                .with_hint("Use unix:/path, pipe:\\\\.\\pipe\\name or tcp://127.0.0.1:<port>#<token file>.")
        };
        if let Some(path) = s.strip_prefix("unix:") {
            #[cfg(unix)]
            return Ok(Self::Unix(PathBuf::from(path)));
            #[cfg(not(unix))]
            return Err(bad(&format!("Unix sockets are not available here ({path})")));
        }
        if let Some(name) = s.strip_prefix("pipe:") {
            #[cfg(windows)]
            return Ok(Self::NamedPipe(name.to_owned()));
            #[cfg(not(windows))]
            return Err(bad(&format!("named pipes are Windows-only ({name})")));
        }
        if let Some(rest) = s.strip_prefix("tcp://") {
            let (addr, token_file) = rest.split_once('#').ok_or_else(|| bad("a TCP endpoint needs #<token file>"))?;
            let addr: SocketAddr = addr.parse().map_err(|_| bad("bad address"))?;
            let token = std::fs::read_to_string(token_file)
                .map_err(|e| Error::io(format!("reading the token file {token_file}"), e))?;
            let token = token_from_file_contents(&token);
            return Ok(Self::Tcp { addr, token: SecretString::from(token) });
        }
        Err(bad("unknown scheme"))
    }

    /// Connects (without `initialize`), failing with `ipc.daemonUnavailable` when nothing listens.
    pub async fn open(&self, timeout: Duration) -> Result<Peer> {
        let unavailable = |e: std::io::Error| {
            Error::new(codes::ipc::DAEMON_UNAVAILABLE, format!("cannot connect to the daemon at {self}: {e}"))
                .caused_by(e)
        };
        match self {
            #[cfg(unix)]
            Self::Unix(path) => {
                let stream = tokio::time::timeout(timeout, tokio::net::UnixStream::connect(path))
                    .await
                    .map_err(|_| unavailable(std::io::ErrorKind::TimedOut.into()))?
                    .map_err(unavailable)?;
                let identity = unix::check_server(path, &stream)?;
                Ok(Peer::start(stream, Role::Client, identity, String::new()))
            }
            #[cfg(windows)]
            Self::NamedPipe(name) => {
                let pipe = windows::connect(name, timeout).await.map_err(unavailable)?;
                let identity = windows::check_server(&pipe)?;
                Ok(Peer::start(pipe, Role::Client, identity, String::new()))
            }
            Self::Tcp { addr, .. } => {
                let stream = tokio::time::timeout(timeout, TcpStream::connect(addr))
                    .await
                    .map_err(|_| unavailable(std::io::ErrorKind::TimedOut.into()))?
                    .map_err(unavailable)?;
                let _ = stream.set_nodelay(true);
                let identity = PeerIdentity { uid: None, sid: None, pid: None, transport: TransportKind::Tcp };
                Ok(Peer::start(stream, Role::Client, identity, String::new()))
            }
        }
    }

    /// The TCP token, for `initialize.params.auth`.
    pub(crate) fn token(&self) -> Option<&str> {
        match self {
            Self::Tcp { token, .. } => Some(token.expose_secret()),
            #[cfg(any(unix, windows))]
            _ => None,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Unix(p) => write!(f, "unix:{}", p.display()),
            #[cfg(windows)]
            Self::NamedPipe(n) => write!(f, "pipe:{n}"),
            Self::Tcp { addr, .. } => write!(f, "tcp://{addr}"),
        }
    }
}

/// The token from a `tcp.json` (`{"token": "…"}`) or a plain token file.
fn token_from_file_contents(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("token").and_then(serde_json::Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| text.trim().to_owned())
}

/// `path`, or `/tmp/russet-<uid>/<hash8>.sock` when `path` does not fit `sun_path`.
#[cfg(unix)]
pub fn unix_socket_path(path: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    if path.as_os_str().as_bytes().len() <= MAX_SOCKET_PATH {
        return path.to_path_buf();
    }
    let hash = russet_core::dirs::hash8(path.as_os_str().as_bytes());
    PathBuf::from(format!("/tmp/russet-{}", russet_core::platform::current_uid())).join(format!("{hash}.sock"))
}

/// A bound listener.
pub(crate) enum Listener {
    #[cfg(unix)]
    Unix {
        listener: tokio::net::UnixListener,
        path: PathBuf,
    },
    Tcp(TcpListener),
    #[cfg(windows)]
    Pipe(windows::PipeListener),
}

impl Listener {
    pub(crate) async fn bind(endpoint: &Endpoint, allow_nonlocal: bool) -> Result<Self> {
        match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => unix::bind(path).await,
            #[cfg(windows)]
            Endpoint::NamedPipe(name) => Ok(Self::Pipe(windows::PipeListener::bind(name)?)),
            Endpoint::Tcp { addr, .. } => {
                if !addr.ip().is_loopback() && !allow_nonlocal {
                    return Err(Error::invalid_argument(format!("{addr} is not a loopback address"))
                        .with_hint("Pass --allow-nonlocal to listen beyond this machine (WSL split mode)."));
                }
                let l = TcpListener::bind(addr).await.map_err(|e| Error::io(format!("binding {addr}"), e))?;
                Ok(Self::Tcp(l))
            }
        }
    }

    /// The bound address as an endpoint string (`unix:…`, `tcp://127.0.0.1:<port>`).
    pub(crate) fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Unix { path, .. } => format!("unix:{}", path.display()),
            Self::Tcp(l) => l.local_addr().map_or_else(|_| "tcp://?".to_owned(), |a| format!("tcp://{a}")),
            #[cfg(windows)]
            Self::Pipe(p) => format!("pipe:{}", p.name()),
        }
    }

    /// Accepts the next connection.
    pub(crate) async fn accept(&mut self) -> std::io::Result<Accepted> {
        match self {
            #[cfg(unix)]
            Self::Unix { listener, .. } => {
                let (stream, _) = listener.accept().await?;
                let cred = stream.peer_cred()?;
                let identity = PeerIdentity {
                    uid: Some(cred.uid()),
                    sid: None,
                    pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
                    transport: TransportKind::Unix,
                };
                Ok(Accepted::Unix(stream, identity))
            }
            Self::Tcp(l) => {
                let (stream, _) = l.accept().await?;
                let _ = stream.set_nodelay(true);
                let identity = PeerIdentity { uid: None, sid: None, pid: None, transport: TransportKind::Tcp };
                Ok(Accepted::Tcp(stream, identity))
            }
            #[cfg(windows)]
            Self::Pipe(p) => {
                let (pipe, identity) = p.accept().await?;
                Ok(Accepted::Pipe(pipe, identity))
            }
        }
    }

    /// Removes the socket file (Unix) when the daemon shuts down.
    pub(crate) fn cleanup(&self) {
        #[cfg(unix)]
        if let Self::Unix { path, .. } = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// An accepted connection and who is on the other end.
pub(crate) enum Accepted {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream, PeerIdentity),
    Tcp(TcpStream, PeerIdentity),
    #[cfg(windows)]
    Pipe(tokio::net::windows::named_pipe::NamedPipeServer, PeerIdentity),
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;

    use russet_core::error::{Error, Result, codes};

    use super::Listener;
    use crate::{PeerIdentity, TransportKind};

    pub(super) async fn bind(path: &Path) -> Result<Listener> {
        let dir =
            path.parent().ok_or_else(|| Error::invalid_argument(format!("{} has no directory", path.display())))?;
        russet_core::fsx::ensure_private_dir(dir)?;
        if path.exists() {
            // A live daemon answers; a stale socket (a crashed daemon) refuses and is removed.
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(Error::new(
                    codes::ipc::DAEMON_UNAVAILABLE,
                    format!("another daemon already listens on {}", path.display()),
                )
                .with_hint("Use `russet daemon status`, or stop it with `russet daemon stop`."));
            }
            std::fs::remove_file(path)
                .map_err(|e| Error::io(format!("removing the stale socket {}", path.display()), e))?;
        }
        let listener =
            tokio::net::UnixListener::bind(path).map_err(|e| Error::io(format!("binding {}", path.display()), e))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::io(format!("restricting {} to 0600", path.display()), e))?;
        Ok(Listener::Unix { listener, path: path.to_path_buf() })
    }

    /// The client's checks (ARCHITECTURE §7.1): the socket's directory and the server belong to this user.
    pub(super) fn check_server(path: &Path, stream: &tokio::net::UnixStream) -> Result<PeerIdentity> {
        let me = russet_core::platform::current_uid();
        if let Some(dir) = path.parent() {
            let meta = std::fs::metadata(dir).map_err(|e| Error::io(format!("reading {}", dir.display()), e))?;
            if meta.uid() != me {
                return Err(Error::new(
                    codes::perm::AUTH_FAILED,
                    format!("{} belongs to uid {}, not to this user", dir.display(), meta.uid()),
                )
                .with_hint("Another user owns the daemon's directory; check RUSSET_HOME and XDG_RUNTIME_DIR."));
            }
        }
        let cred = stream.peer_cred().map_err(|e| Error::io("reading the daemon's credentials", e))?;
        if cred.uid() != me {
            return Err(Error::new(
                codes::perm::AUTH_FAILED,
                format!("the daemon at {} runs as uid {}, not as this user", path.display(), cred.uid()),
            ));
        }
        Ok(PeerIdentity {
            uid: Some(cred.uid()),
            sid: None,
            pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
            transport: TransportKind::Unix,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn long_socket_paths_fall_back_to_tmp() {
        let short = Path::new("/run/user/1000/russet/russetd.sock");
        assert_eq!(unix_socket_path(short), short);
        let long = PathBuf::from(format!("/{}/runtime/russetd.sock", "x".repeat(120)));
        let fallback = unix_socket_path(&long);
        assert!(fallback.starts_with(format!("/tmp/russet-{}", russet_core::platform::current_uid())));
        assert!(fallback.to_string_lossy().ends_with(".sock"));
        assert_eq!(fallback, unix_socket_path(&long), "stable");
    }

    #[test]
    fn endpoints_parse_and_display() {
        #[cfg(unix)]
        {
            let e = Endpoint::parse("unix:/run/user/1000/russet/russetd.sock").expect("parses");
            assert_eq!(e.to_string(), "unix:/run/user/1000/russet/russetd.sock");
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let token_file = tmp.path().join("tcp.json");
        std::fs::write(&token_file, r#"{"addr": "127.0.0.1:7777", "token": "abc"}"#).expect("write");
        let e = Endpoint::parse(&format!("tcp://127.0.0.1:7777#{}", token_file.display())).expect("parses");
        assert_eq!(e.to_string(), "tcp://127.0.0.1:7777");
        assert_eq!(e.token(), Some("abc"));
        assert!(!format!("{e:?}").contains("abc"), "the token never prints");
        assert!(Endpoint::parse("http://x").is_err());
    }
}
