// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Finds the host LLDB tools for the H1–H3 harnesses: `lldb`, `lldb-server` and `lldb-dap` from the Swift
//! toolchain, plus the libpython environment Swift's Linux LLDB needs (ARCHITECTURE §16.2, RD V1–V2).
//!
//! Discovery order:
//! - tools: `RUSSET_TEST_LLDB_DIR`, then `/usr/lib/swift/usr/bin`, then the directory of `lldb-server` on `PATH`;
//! - Python: `RUSSET_TEST_PYTHONHOME`, then an existing uv-managed python-build-standalone 3.12
//!   (`~/.local/share/uv/python/cpython-3.12*-linux-<arch>-gnu`). `~/.local/opt/python-3.12` is never used.
//!
//! A test that needs the tools calls [`LldbTools::for_test`]: it skips (returns `None` and says why on stderr)
//! unless `RUSSET_TEST_REQUIRE_LLDB=1` is set, which turns a missing tool into a failure (CI).

use std::path::{Path, PathBuf};
use std::process::Command;

/// The host LLDB tools and the environment every LLDB child needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LldbTools {
    pub bin_dir: PathBuf,
    pub lldb: PathBuf,
    pub lldb_server: PathBuf,
    pub lldb_dap: PathBuf,
    /// `LD_LIBRARY_PATH` and `PYTHONHOME` (empty when no Python was found; `lldb-server` needs none).
    pub env: Vec<(String, String)>,
    /// The python-build-standalone prefix, when found.
    pub python_home: Option<PathBuf>,
}

impl LldbTools {
    /// Finds the tools; `Err` names what is missing.
    pub fn discover() -> Result<Self, String> {
        let bin_dir =
            find_bin_dir().ok_or("no lldb-server: set RUSSET_TEST_LLDB_DIR to the Swift toolchain's usr/bin")?;
        let python_home = find_python_home();
        let mut env = Vec::new();
        if let Some(pbs) = &python_home {
            let mut ld = vec![pbs.join("lib")];
            if let Some(usr) = bin_dir.parent() {
                ld.push(usr.join("lib/swift/linux"));
                ld.push(usr.join("lib/swift/host/compiler"));
            }
            if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                ld.extend(std::env::split_paths(&existing));
            }
            let joined = std::env::join_paths(ld).map_err(|e| e.to_string())?;
            env.push(("LD_LIBRARY_PATH".to_owned(), joined.to_string_lossy().into_owned()));
            env.push(("PYTHONHOME".to_owned(), pbs.to_string_lossy().into_owned()));
        }
        Ok(Self {
            lldb: bin_dir.join(exe("lldb")),
            lldb_server: bin_dir.join(exe("lldb-server")),
            lldb_dap: bin_dir.join(exe("lldb-dap")),
            bin_dir,
            env,
            python_home,
        })
    }

    /// The tools for a test, or `None` (with the reason on stderr) when the host lacks them and
    /// `RUSSET_TEST_REQUIRE_LLDB` is not `1`. `need_python` asks for `lldb`/`lldb-dap` as well.
    pub fn for_test(test: &str, need_python: bool) -> Option<Self> {
        let found = Self::discover().and_then(|t| {
            if need_python && !t.lldb_runs() {
                Err(format!(
                    "{} does not start (libpython3.12 missing?); set RUSSET_TEST_PYTHONHOME to a python-build-standalone 3.12",
                    t.lldb.display()
                ))
            } else {
                Ok(t)
            }
        });
        match found {
            Ok(t) => Some(t),
            Err(why) if std::env::var("RUSSET_TEST_REQUIRE_LLDB").as_deref() == Ok("1") => {
                panic!("{test}: RUSSET_TEST_REQUIRE_LLDB=1 but {why}")
            }
            Err(why) => {
                eprintln!("skipping {test}: {why}");
                None
            }
        }
    }

    /// A command for `program` with the LLDB environment applied.
    pub fn command(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c
    }

    /// Whether `lldb --version` runs (libpython found).
    pub fn lldb_runs(&self) -> bool {
        self.command(&self.lldb).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() }
}

fn find_bin_dir() -> Option<PathBuf> {
    let has_server = |d: &Path| d.join(exe("lldb-server")).is_file() || d.join(exe("lldb")).is_file();
    if let Some(dir) = std::env::var_os("RUSSET_TEST_LLDB_DIR").map(PathBuf::from) {
        return has_server(&dir).then_some(dir);
    }
    let swift = PathBuf::from("/usr/lib/swift/usr/bin");
    if has_server(&swift) {
        return Some(swift);
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).find(|d| d.join(exe("lldb-server")).is_file()))
}

fn find_python_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("RUSSET_TEST_PYTHONHOME").map(PathBuf::from) {
        return home.join("lib").is_dir().then_some(home);
    }
    let base = PathBuf::from(std::env::var_os("HOME")?).join(".local/share/uv/python");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            name.starts_with("cpython-3.12") && name.ends_with("-gnu") && p.join("lib/libpython3.12.so.1.0").exists()
        })
        .collect();
    // Prefer the most specific (patch-versioned) name, for example cpython-3.12.15-linux-x86_64-gnu.
    candidates.sort();
    candidates.pop()
}
