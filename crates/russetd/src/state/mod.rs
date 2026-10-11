// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Persisted indexes under `state/` (ARCHITECTURE §14.8): append-only JSON-lines files (0600) and their
//! readers, shared by the audit log and the job store.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Mutex;

use russet_core::error::{Error, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// An append-only JSON-lines file; one writer per path, lines are written whole.
#[derive(Debug)]
pub struct JsonLines {
    path: std::path::PathBuf,
    lock: Mutex<()>,
}

impl JsonLines {
    /// A JSON-lines file at `path` (created 0600 on the first append).
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into(), lock: Mutex::new(()) }
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one value as a line.
    pub fn append<T: Serialize>(&self, value: &T) -> Result<()> {
        let mut line = serde_json::to_vec(value).map_err(|e| Error::internal(format!("encoding a log line: {e}")))?;
        line.push(b'\n');
        let _guard = self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dir) = self.path.parent() {
            russet_core::fsx::ensure_private_dir(dir)?;
        }
        let mut f = russet_core::fsx::open_append(&self.path)?;
        f.write_all(&line).map_err(|e| Error::io(format!("appending to {}", self.path.display()), e))
    }

    /// Every line that parses as `T` (unparsable lines, for example a torn last line, are skipped).
    pub fn read_all<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        read_lines(&self.path)
    }
}

/// Every line of `path` that parses as `T`; a missing file is empty.
pub fn read_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io(format!("reading {}", path.display()), e)),
    };
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line.map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
        if let Ok(v) = serde_json::from_str(&line) {
            out.push(v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_append_and_read_back_and_are_private() {
        let tmp = russet_core::fsx::TempDir::new("jsonl").expect("tempdir");
        let log = JsonLines::new(tmp.path().join("state/a.jsonl"));
        log.append(&serde_json::json!({"n": 1})).expect("append");
        log.append(&serde_json::json!({"n": 2})).expect("append");
        std::fs::OpenOptions::new().append(true).open(log.path()).expect("open").write_all(b"{\"torn").expect("write");
        let all: Vec<serde_json::Value> = log.read_all().expect("read");
        assert_eq!(all, vec![serde_json::json!({"n": 1}), serde_json::json!({"n": 2})]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(log.path()).expect("stat").permissions().mode() & 0o777, 0o600);
        }
    }
}
