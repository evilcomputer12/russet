// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`TempHome`]: a fresh, private `RUSSET_HOME` per test (ARCHITECTURE §3.3 item 7, §14.9, §15; testability
//! contract T4).
//!
//! Every test that starts a daemon, in process or spawned, gets its own home, so parallel nextest runs never
//! share a socket, a lock file or a keyring entry. The home's environment is the lane-D one:
//!
//! | Variable | Value |
//! |---|---|
//! | `RUSSET_HOME` | the temporary directory (config, data, state, cache and runtime move under it) |
//! | `RUSSET_SECRETS_BACKEND` | `memory` (or `passphrase` with a synthetic passphrase): no keyring entry is touched |
//! | `RUSSET_DEVICE_BACKEND` | `scripted:<the default scenario>` unless the builder picks another [`Backend`] |
//! | `RUSSET_LIVE_DEVICE_LOCK` | a lock file inside the home: tests never take the real host-wide lock (debug builds honour it) |
//! | `RUSSET_OFFLINE` | `1`: HTTP clients refuse non-loopback hosts (T3) |
//! | `NO_COLOR` | `1` |
//!
//! and it removes what could leak in from the developer's shell: `RUSSET_LANE` (tests never run as lane A),
//! `RUSSET_DAEMON`, the per-directory overrides, `RUSSET_LEASE`, `RUSSET_DEVICE`, hardware-test and CI
//! credential variables, and `USBMUXD_SOCKET_ADDRESS` (unless a [`crate::Tripwire`] replaces it).
//!
//! Nothing here mutates the test process's own environment (that is unsound with threads): apply the env to a
//! child with [`TempHome::apply`], and pass [`TempHome::dirs`] to in-process code.
//!
//! **The scripted backend in the daemon, today.** ARCHITECTURE §3.3 item 7 makes `scripted:<scenario.toml>` the
//! lane-D backend, so it is the default here. The daemon cannot build [`crate::ScriptedConnector`] yet: russetd
//! may not depend on this dev crate (§5.2), and W8 has asked the lead for a contract change (move the connector
//! where russetd may use it). Until then a daemon started with `scripted:` starts, reports
//! `deviceBackend: "scripted:…"`, and answers device methods with an error (`core.notImplemented` in W2's
//! `DeviceSlot`). So: tests of the device plane drive [`crate::ScriptedConnector`] in process, as an
//! `Arc<dyn russet_device::io::ServiceConnector>`; daemon-level tests that need no device keep the default; a
//! daemon-level test that asserts the device plane is absent picks [`Backend::None`].

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use russet_core::dirs::RussetDirs;

use crate::tripwire::Tripwire;

/// Variables a test home always removes from a child's environment.
pub const REMOVED_ENV: &[&str] = &[
    "RUSSET_LANE",
    "RUSSET_DAEMON",
    "RUSSET_CONFIG_DIR",
    "RUSSET_DATA_DIR",
    "RUSSET_STATE_DIR",
    "RUSSET_CACHE_DIR",
    "RUSSET_RUNTIME_DIR",
    "RUSSET_LEASE",
    "RUSSET_DEVICE",
    "RUSSET_HIL_UDID",
    "RUSSET_TUNNEL_PCAP",
    "RUSSET_ENDPOINTS_FILE",
    "RUSSET_SECRETS_PASSPHRASE",
    "RUSSET_ASC_KEY_ID",
    "RUSSET_ASC_ISSUER_ID",
    "RUSSET_ASC_KEY_P8",
    "RUSSET_ASC_KEY_PATH",
    "RUSSET_SIGNING_P12_B64",
    "RUSSET_SIGNING_P12_PASSWORD",
    "USBMUXD_SOCKET_ADDRESS",
];

/// The device backend a test daemon uses (`RUSSET_DEVICE_BACKEND`, ARCHITECTURE §14.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// `scripted:<scenario.toml>`, the lane-D default.
    Scripted(PathBuf),
    /// `replay:<trace>` (W15).
    Replay(PathBuf),
    /// `mock:<addr>` (W15).
    Mock(String),
    /// `none`: no connector at all.
    None,
    /// `live`: only for the live-device lock tests, which also pass `--allow-live-device`, a private lock and a
    /// fake usbmuxd; `RUSSET_LANE` stays unset.
    Live,
}

impl Backend {
    /// The value of `RUSSET_DEVICE_BACKEND`.
    pub fn env_value(&self) -> OsString {
        match self {
            Self::Scripted(p) => prefixed("scripted:", p.as_os_str()),
            Self::Replay(p) => prefixed("replay:", p.as_os_str()),
            Self::Mock(addr) => format!("mock:{addr}").into(),
            Self::None => "none".into(),
            Self::Live => "live".into(),
        }
    }

    /// The scripted backend with the testkit's default scenario.
    pub fn default_scripted() -> Self {
        Self::Scripted(crate::scenario::default_scenario_path())
    }
}

fn prefixed(prefix: &str, value: &OsStr) -> OsString {
    let mut s = OsString::from(prefix);
    s.push(value);
    s
}

/// How [`TempHome`]'s secrets backend is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretsBackend {
    /// `memory`: nothing persists.
    Memory,
    /// `passphrase` with a synthetic passphrase: sealed files inside the home.
    Passphrase,
}

/// Builder for [`TempHome`].
#[derive(Debug)]
pub struct TempHomeBuilder {
    backend: Backend,
    secrets: SecretsBackend,
    tripwire: bool,
    no_autostart: bool,
    env: Vec<(OsString, OsString)>,
}

impl Default for TempHomeBuilder {
    fn default() -> Self {
        Self {
            backend: Backend::default_scripted(),
            secrets: SecretsBackend::Memory,
            tripwire: false,
            no_autostart: false,
            env: Vec::new(),
        }
    }
}

impl TempHomeBuilder {
    /// The device backend (default: the scripted default scenario).
    pub fn backend(mut self, b: Backend) -> Self {
        self.backend = b;
        self
    }

    /// A scenario file for the scripted backend.
    pub fn scenario(self, path: impl Into<PathBuf>) -> Self {
        self.backend(Backend::Scripted(path.into()))
    }

    pub fn secrets(mut self, s: SecretsBackend) -> Self {
        self.secrets = s;
        self
    }

    /// Bind a [`Tripwire`] and point `USBMUXD_SOCKET_ADDRESS` at it.
    pub fn tripwire(mut self) -> Self {
        self.tripwire = true;
        self
    }

    /// Set `RUSSET_NO_AUTOSTART=1`, so a CLI fails instead of spawning a daemon.
    pub fn no_autostart(mut self) -> Self {
        self.no_autostart = true;
        self
    }

    /// One more variable for children.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn build(self) -> io::Result<TempHome> {
        // Short paths under the system temp dir keep the socket path under the 108-byte limit (ARCHITECTURE §7.1).
        let dir = tempfile::Builder::new().prefix("russet-test-").tempdir()?;
        let dirs = RussetDirs::under(dir.path());
        for d in [&dirs.config, &dirs.data, &dirs.state, &dirs.cache, &dirs.runtime] {
            std::fs::create_dir_all(d)?;
            private(d)?;
        }
        private(dir.path())?;
        let tripwire = if self.tripwire { Some(Tripwire::bind(dir.path())?) } else { None };
        Ok(TempHome {
            tripwire,
            dir: Some(dir),
            dirs,
            backend: self.backend,
            secrets: self.secrets,
            no_autostart: self.no_autostart,
            extra: self.env,
        })
    }
}

/// Makes a directory private to the user (0700), as `RussetDirs::ensure` does (ARCHITECTURE §15).
fn private(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// A temporary `RUSSET_HOME`, removed on drop.
#[derive(Debug)]
pub struct TempHome {
    // Fields drop in order: the tripwire stops listening before its directory goes.
    tripwire: Option<Tripwire>,
    dir: Option<tempfile::TempDir>,
    dirs: RussetDirs,
    backend: Backend,
    secrets: SecretsBackend,
    no_autostart: bool,
    extra: Vec<(OsString, OsString)>,
}

impl TempHome {
    /// A home with the lane-D defaults: memory secrets and the scripted default scenario.
    pub fn new() -> io::Result<Self> {
        Self::builder().build()
    }

    pub fn builder() -> TempHomeBuilder {
        TempHomeBuilder::default()
    }

    /// The home directory (`RUSSET_HOME`).
    pub fn root(&self) -> &Path {
        self.dir.as_ref().map_or_else(|| Path::new(""), |d| d.path())
    }

    /// The five directories under the home (`RussetDirs::under`).
    pub fn dirs(&self) -> &RussetDirs {
        &self.dirs
    }

    /// The daemon's socket with `RUSSET_HOME` set: `$RUSSET_HOME/runtime/russetd.sock` (ARCHITECTURE §7.1).
    pub fn socket_path(&self) -> PathBuf {
        self.dirs.runtime.join("russetd.sock")
    }

    /// `runtime/russetd.json`, the running daemon's description (ARCHITECTURE §3.3).
    pub fn daemon_state_file(&self) -> PathBuf {
        self.dirs.runtime.join("russetd.json")
    }

    /// The private live-device lock (`RUSSET_LIVE_DEVICE_LOCK`), never the host-wide one.
    pub fn live_device_lock(&self) -> PathBuf {
        self.root().join("live-device.lock")
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn tripwire(&self) -> Option<&Tripwire> {
        self.tripwire.as_ref()
    }

    /// The variables a child process gets (see the module docs).
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = vec![
            ("RUSSET_HOME".into(), self.root().as_os_str().to_owned()),
            ("RUSSET_DEVICE_BACKEND".into(), self.backend.env_value()),
            ("RUSSET_LIVE_DEVICE_LOCK".into(), self.live_device_lock().into_os_string()),
            ("RUSSET_OFFLINE".into(), "1".into()),
            ("NO_COLOR".into(), "1".into()),
        ];
        match self.secrets {
            SecretsBackend::Memory => env.push(("RUSSET_SECRETS_BACKEND".into(), "memory".into())),
            SecretsBackend::Passphrase => {
                env.push(("RUSSET_SECRETS_BACKEND".into(), "passphrase".into()));
                env.push(("RUSSET_SECRETS_PASSPHRASE".into(), "russet-testkit synthetic passphrase".into()));
            }
        }
        if self.no_autostart {
            env.push(("RUSSET_NO_AUTOSTART".into(), "1".into()));
        }
        if let Some(t) = &self.tripwire {
            env.push(t.env());
        }
        env.extend(self.extra.iter().cloned());
        env
    }

    /// The variables removed from a child's environment ([`REMOVED_ENV`], minus what [`TempHome::env`] sets).
    pub fn env_remove(&self) -> Vec<&'static str> {
        let set: Vec<OsString> = self.env().into_iter().map(|(k, _)| k).collect();
        REMOVED_ENV.iter().copied().filter(|k| !set.iter().any(|s| s == k)).collect()
    }

    /// Applies the home's environment to a command.
    pub fn apply<'c>(&self, cmd: &'c mut std::process::Command) -> &'c mut std::process::Command {
        for k in self.env_remove() {
            cmd.env_remove(k);
        }
        cmd.envs(self.env())
    }

    /// Applies the home's environment to a tokio command.
    pub fn apply_tokio<'c>(&self, cmd: &'c mut tokio::process::Command) -> &'c mut tokio::process::Command {
        for k in self.env_remove() {
            cmd.env_remove(k);
        }
        cmd.envs(self.env())
    }

    /// Keeps the directory after the test (for debugging) and returns its path.
    pub fn keep(mut self) -> PathBuf {
        match self.dir.take() {
            Some(d) => d.keep(),
            None => PathBuf::new(),
        }
    }
}
