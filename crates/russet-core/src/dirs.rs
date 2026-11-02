// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Russet's five directories (ARCHITECTURE §15): config, data, state, cache and runtime. With `RUSSET_HOME`
//! set, all five move under it (ARCHITECTURE §3.3 item 7, §14.9).
//!
//! Resolution, per directory, from highest to lowest precedence:
//! 1. `RUSSET_{CONFIG,DATA,STATE,CACHE,RUNTIME}_DIR`;
//! 2. `RUSSET_HOME/<name>` (`config`, `data`, `state`, `cache`, `runtime`);
//! 3. the platform default: XDG on Linux (`$XDG_CONFIG_HOME/russet`, …; the runtime directory is
//!    `$XDG_RUNTIME_DIR/russet`, or `/tmp/russet-<uid>` without it), Known Folders on Windows
//!    (`%APPDATA%\Russet\config`, `%LOCALAPPDATA%\Russet\{data,state,cache,run}`).
//!
//! Relative XDG values are ignored, as the XDG Base Directory specification requires. Relative `RUSSET_*`
//! values are made absolute against the current directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, codes};

/// The resolved directory set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RussetDirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub runtime: PathBuf,
}

/// The file name of the daemon's socket in the runtime directory (ARCHITECTURE §7.1, [D 9]).
pub const SOCKET_NAME: &str = "russetd.sock";

impl RussetDirs {
    /// Resolves from `RUSSET_HOME`, `RUSSET_{CONFIG,DATA,STATE,CACHE,RUNTIME}_DIR`, the XDG variables, or
    /// `%APPDATA%`/`%LOCALAPPDATA%` on Windows.
    pub fn from_env() -> crate::Result<Self> {
        Self::from_vars(|k| std::env::var_os(k))
    }

    /// [`RussetDirs::from_env`] over an injected environment (tests; the Linux and Windows rules).
    pub fn from_vars(get: impl Fn(&str) -> Option<OsString>) -> crate::Result<Self> {
        let non_empty = |k: &str| get(k).filter(|v| !v.is_empty());
        let home = non_empty("RUSSET_HOME").map(absolute);
        let base = match &home {
            Some(root) => Self::under(root),
            None => platform_defaults(&non_empty)?,
        };
        let pick = |var: &str, default: PathBuf| non_empty(var).map(absolute).unwrap_or(default);
        Ok(Self {
            config: pick("RUSSET_CONFIG_DIR", base.config),
            data: pick("RUSSET_DATA_DIR", base.data),
            state: pick("RUSSET_STATE_DIR", base.state),
            cache: pick("RUSSET_CACHE_DIR", base.cache),
            runtime: pick("RUSSET_RUNTIME_DIR", base.runtime),
        })
    }

    /// Portable layout: every directory under `root` (what `RUSSET_HOME=<root>` selects).
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            cache: root.join("cache"),
            runtime: root.join("runtime"),
        }
    }

    /// Creates the directories 0700 and checks the owner (Unix), or sets a user-only ACL (Windows).
    pub fn ensure(&self) -> crate::Result<()> {
        for dir in [&self.config, &self.data, &self.state, &self.cache, &self.runtime] {
            crate::fsx::ensure_private_dir(dir)?;
        }
        Ok(())
    }

    /// `state/logs`: the daemon's daily log files (ARCHITECTURE §14.8).
    pub fn logs(&self) -> PathBuf {
        self.state.join("logs")
    }

    /// `state/jobs`: job events and results.
    pub fn jobs(&self) -> PathBuf {
        self.state.join("jobs")
    }

    /// `state/panics`: daemon panic reports [D 42].
    pub fn panics(&self) -> PathBuf {
        self.state.join("panics")
    }

    /// `state/audit.jsonl` [D 43].
    pub fn audit_log(&self) -> PathBuf {
        self.state.join("audit.jsonl")
    }

    /// `runtime/russetd.json`, the daemon's discovery and diagnostics file (ARCHITECTURE §3.3 item 2).
    pub fn daemon_json(&self) -> PathBuf {
        self.runtime.join("russetd.json")
    }

    /// `runtime/russetd.lock`, held by the daemon for its whole life.
    pub fn daemon_lock(&self) -> PathBuf {
        self.runtime.join("russetd.lock")
    }

    /// `runtime/spawn.lock`, taken by clients while they start a daemon.
    pub fn spawn_lock(&self) -> PathBuf {
        self.runtime.join("spawn.lock")
    }

    /// `runtime/russetd.sock`, before the 108-byte `sun_path` fallback of `russet-ipc`.
    pub fn socket_path(&self) -> PathBuf {
        self.runtime.join(SOCKET_NAME)
    }

    /// `runtime/tcp.json`: the opt-in TCP endpoint and its token (0600).
    pub fn tcp_json(&self) -> PathBuf {
        self.runtime.join("tcp.json")
    }

    /// The default runtime directory of this user, ignoring `RUSSET_HOME` and the `RUSSET_*_DIR` overrides; a
    /// runtime directory elsewhere belongs to a separate home (named pipes then get a `-<hash8>` suffix).
    pub fn default_runtime() -> crate::Result<PathBuf> {
        let non_empty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        Ok(platform_defaults(&non_empty)?.runtime)
    }
}

/// The first 8 hex digits of SHA-256 of `bytes`: per-home names (the keyring entry `master-key-<hash8>`, named
/// pipes, socket paths too long for `sun_path`; ARCHITECTURE §3.3 item 7, §7.1).
pub fn hash8(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn absolute(p: OsString) -> PathBuf {
    let p = PathBuf::from(p);
    std::path::absolute(&p).unwrap_or(p)
}

/// An absolute path from an XDG variable; relative values are invalid and ignored.
#[cfg_attr(windows, allow(dead_code))]
fn xdg(get: &dyn Fn(&str) -> Option<OsString>, var: &str) -> Option<PathBuf> {
    get(var).map(PathBuf::from).filter(|p| p.is_absolute())
}

#[cfg(unix)]
fn platform_defaults(get: &dyn Fn(&str) -> Option<OsString>) -> crate::Result<RussetDirs> {
    let home = || {
        get("HOME").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, "cannot find Russet's directories: HOME is not set")
                .with_hint("Set HOME, or set RUSSET_HOME to a directory Russet may use.")
        })
    };
    let under_home = |var: &str, rel: &str| -> crate::Result<PathBuf> {
        Ok(match xdg(get, var) {
            Some(p) => p,
            None => home()?.join(rel),
        }
        .join("russet"))
    };
    let runtime = match xdg(get, "XDG_RUNTIME_DIR") {
        Some(p) => p.join("russet"),
        None => PathBuf::from(format!("/tmp/russet-{}", crate::platform::current_uid())),
    };
    Ok(RussetDirs {
        config: under_home("XDG_CONFIG_HOME", ".config")?,
        data: under_home("XDG_DATA_HOME", ".local/share")?,
        state: under_home("XDG_STATE_HOME", ".local/state")?,
        cache: under_home("XDG_CACHE_HOME", ".cache")?,
        runtime,
    })
}

#[cfg(windows)]
fn platform_defaults(get: &dyn Fn(&str) -> Option<OsString>) -> crate::Result<RussetDirs> {
    let known = |var: &str| -> crate::Result<PathBuf> {
        get(var).map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, format!("cannot find Russet's directories: %{var}% is not set"))
                .with_hint("Set RUSSET_HOME to a directory Russet may use.")
        })
    };
    let roaming = known("APPDATA")?.join("Russet");
    let local = known("LOCALAPPDATA")?.join("Russet");
    Ok(RussetDirs {
        config: roaming.join("config"),
        data: local.join("data"),
        state: local.join("state"),
        cache: local.join("cache"),
        runtime: local.join("run"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: BTreeMap<String, OsString> = pairs.iter().map(|(k, v)| ((*k).to_owned(), OsString::from(v))).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path expectations; the engine runs in Linux (WSL on Windows)")]
    fn russet_home_moves_all_five() {
        let dirs =
            RussetDirs::from_vars(env(&[("RUSSET_HOME", "/w/.russet-dev"), ("HOME", "/home/u")])).expect("resolves");
        assert_eq!(dirs, RussetDirs::under(Path::new("/w/.russet-dev")));
        assert_eq!(dirs.socket_path(), PathBuf::from("/w/.russet-dev/runtime/russetd.sock"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path expectations; the engine runs in Linux (WSL on Windows)")]
    fn per_directory_overrides_win() {
        let dirs = RussetDirs::from_vars(env(&[
            ("RUSSET_HOME", "/w/h"),
            ("RUSSET_CACHE_DIR", "/big/cache"),
            ("RUSSET_RUNTIME_DIR", "/run/r"),
        ]))
        .expect("resolves");
        assert_eq!(dirs.cache, PathBuf::from("/big/cache"));
        assert_eq!(dirs.runtime, PathBuf::from("/run/r"));
        assert_eq!(dirs.config, PathBuf::from("/w/h/config"));
    }

    #[cfg(unix)]
    #[test]
    fn linux_follows_xdg() {
        let dirs = RussetDirs::from_vars(env(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/home/u/cfg"),
            ("XDG_DATA_HOME", "relative/is/ignored"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]))
        .expect("resolves");
        assert_eq!(dirs.config, PathBuf::from("/home/u/cfg/russet"));
        assert_eq!(dirs.data, PathBuf::from("/home/u/.local/share/russet"));
        assert_eq!(dirs.state, PathBuf::from("/home/u/.local/state/russet"));
        assert_eq!(dirs.cache, PathBuf::from("/home/u/.cache/russet"));
        assert_eq!(dirs.runtime, PathBuf::from("/run/user/1000/russet"));
    }

    #[cfg(unix)]
    #[test]
    fn linux_runtime_falls_back_to_tmp() {
        let dirs = RussetDirs::from_vars(env(&[("HOME", "/home/u")])).expect("resolves");
        assert_eq!(dirs.runtime, PathBuf::from(format!("/tmp/russet-{}", crate::platform::current_uid())));
        let e = RussetDirs::from_vars(env(&[])).expect_err("no HOME");
        assert_eq!(e.code(), codes::core::INVALID_ARGUMENT);
    }

    #[test]
    fn hash8_is_stable() {
        assert_eq!(hash8(b"abc"), "ba7816bf");
        assert_eq!(hash8(b"/w/.russet-dev").len(), 8);
    }

    #[test]
    fn ensure_creates_private_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dirs = RussetDirs::under(&tmp.path().join("home"));
        dirs.ensure().expect("creates");
        for d in [&dirs.config, &dirs.data, &dirs.state, &dirs.cache, &dirs.runtime] {
            assert!(d.is_dir(), "{} missing", d.display());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(d).expect("stat").permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{}", d.display());
            }
        }
    }
}
