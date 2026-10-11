// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Where the resolver looks on this host (ARCHITECTURE §16.1): `PATH`, SwiftPM's SDK directory, uv's
//! python-build-standalone installs, swiftly, `ldconfig`. Tests build a [`HostEnv`] over a fixture tree.

use std::path::{Path, PathBuf};

/// A prefix the resolver never uses: `~/.local/opt/python-3.12` is being retired (PLAN §14).
pub const RETIRED_PYTHON: &str = ".local/opt/python-3.12";

/// The search locations of one host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostEnv {
    /// `PATH`, in order.
    pub path: Vec<PathBuf>,
    /// The user's home directory.
    pub home: Option<PathBuf>,
    /// SwiftPM's SDK directories (`~/.swiftpm/swift-sdks`); xtool bundles are read in place there.
    pub swift_sdk_dirs: Vec<PathBuf>,
    /// uv's managed Pythons (`~/.local/share/uv/python`).
    pub uv_python_dirs: Vec<PathBuf>,
    /// swiftly's toolchains (`~/.local/share/swiftly/toolchains`).
    pub swiftly_dirs: Vec<PathBuf>,
    /// `ldconfig`, for the system's shared-library cache.
    pub ldconfig: Option<PathBuf>,
}

impl HostEnv {
    /// The locations of this process's environment.
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let mut swift_sdk_dirs = Vec::new();
        if let Some(h) = &home {
            swift_sdk_dirs.push(h.join(".swiftpm/swift-sdks"));
        }
        if let Some(cfg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
            swift_sdk_dirs.push(cfg.join("swiftpm/swift-sdks"));
        }
        let uv_python_dirs = std::env::var_os("UV_PYTHON_INSTALL_DIR")
            .map(PathBuf::from)
            .into_iter()
            .chain(data_home.as_ref().map(|d| d.join("uv/python")))
            .collect();
        let swiftly_dirs = std::env::var_os("SWIFTLY_HOME_DIR")
            .map(|d| PathBuf::from(d).join("toolchains"))
            .into_iter()
            .chain(data_home.as_ref().map(|d| d.join("swiftly/toolchains")))
            .collect();
        let ldconfig = ["/usr/bin/ldconfig", "/sbin/ldconfig", "/usr/sbin/ldconfig"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file());
        Self { path, home, swift_sdk_dirs, uv_python_dirs, swiftly_dirs, ldconfig }
    }

    /// The first executable called `name` on `PATH`.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
        self.path.iter().map(|d| d.join(&exe)).find(|p| is_executable(p))
    }

    /// Every executable called `name` on `PATH`, in order, without duplicates of the same file.
    pub fn which_all(&self, name: &str) -> Vec<PathBuf> {
        let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for p in self.path.iter().map(|d| d.join(&exe)).filter(|p| is_executable(p)) {
            let real = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
            if !seen.contains(&real) {
                seen.push(real);
                out.push(p);
            }
        }
        out
    }

    /// Whether `p` lies under the retired Python prefix.
    pub fn is_retired(&self, p: &Path) -> bool {
        self.home.as_ref().is_some_and(|h| p.starts_with(h.join(RETIRED_PYTHON)))
    }
}

/// Whether `p` is a regular file the user may execute.
pub fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
