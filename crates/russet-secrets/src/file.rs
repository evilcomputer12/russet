// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Reading a secret from a local file (`secrets.set {fromFile}`; ARCHITECTURE §7.5 [D 85]).
//!
//! - Only a regular file of at most [`MAX_SECRET_BYTES`] is read. On Unix the file is opened with `O_NONBLOCK`
//!   and checked with `fstat` on the open descriptor, so a FIFO or a device (even one swapped in after a check of
//!   the path) is refused instead of blocking the reading thread forever.
//! - The bytes go into one zeroizing buffer of the file's size (plus one byte to notice a file that grows while it
//!   is read): no reallocation leaves an unzeroized copy of the secret in freed memory.

use std::io::Read as _;
use std::path::Path;

use russet_core::Result;
use russet_core::error::{Error, codes};
use zeroize::Zeroizing;

use crate::{MAX_SECRET_BYTES, SecretBytes};

fn io_error(path: &Path, e: &std::io::Error) -> Error {
    let code = if e.kind() == std::io::ErrorKind::NotFound { codes::core::NOT_FOUND } else { codes::core::IO };
    Error::new(code, format!("cannot read {}: {e}", path.display())).with_hint("Check the path and its permissions.")
}

fn open(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // A FIFO opened for reading without O_NONBLOCK waits for a writer; a terminal must not become ours.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    options.open(path)
}

/// Reads the secret in the regular file `path` (see the module docs).
pub fn read_secret_file(path: &Path) -> Result<SecretBytes> {
    let file = open(path).map_err(|e| io_error(path, &e))?;
    let meta = file.metadata().map_err(|e| io_error(path, &e))?;
    if !meta.is_file() {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("{} is not a regular file", path.display()))
            .with_hint("Pass the key or token file itself, not a pipe, a device or a directory."));
    }
    let too_large = || {
        Error::new(codes::core::INVALID_ARGUMENT, format!("{} is larger than 1 MiB", path.display()))
            .with_hint("Secrets are keys and tokens; this file is too large to be one.")
    };
    let len = usize::try_from(meta.len()).ok().filter(|l| *l <= MAX_SECRET_BYTES).ok_or_else(too_large)?;
    let mut buf = Zeroizing::new(vec![0u8; len + 1]);
    let mut filled = 0;
    let mut reader = &file;
    loop {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if filled == buf.len() {
                    return Err(Error::new(codes::core::IO, format!("{} changed while it was read", path.display()))
                        .with_hint("Retry once nothing writes to the file."));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(io_error(path, &e)),
        }
    }
    buf.truncate(filled);
    // Moves the vector out without copying; the emptied wrapper has nothing left to zeroize.
    Ok(SecretBytes::new(std::mem::take(&mut *buf)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_files_are_read_and_everything_else_is_refused() {
        let dir = tempfile::tempdir().expect("dir");
        let key = dir.path().join("AuthKey_TEST.p8");
        std::fs::write(&key, b"synthetic key bytes").expect("write");
        assert_eq!(read_secret_file(&key).expect("read").expose(), b"synthetic key bytes");
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").expect("write");
        assert!(read_secret_file(&empty).expect("empty").is_empty());

        let err = read_secret_file(dir.path()).expect_err("a directory");
        assert!(matches!(err.code(), c if c == codes::core::INVALID_ARGUMENT || c == codes::core::IO), "{err:?}");
        let err = read_secret_file(&dir.path().join("missing")).expect_err("missing");
        assert_eq!(err.code(), codes::core::NOT_FOUND);

        let big = dir.path().join("big");
        std::fs::write(&big, vec![7u8; MAX_SECRET_BYTES + 1]).expect("write");
        let err = read_secret_file(&big).expect_err("too large");
        assert!(err.message().contains("larger than 1 MiB"), "{}", err.message());
        std::fs::write(&big, vec![7u8; MAX_SECRET_BYTES]).expect("write");
        assert_eq!(read_secret_file(&big).expect("at the limit").len(), MAX_SECRET_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = tempfile::tempdir().expect("dir");
        let fifo = dir.path().join("fifo");
        if !std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
            eprintln!("skipped: no mkfifo");
            return;
        }
        // Without O_NONBLOCK this open would wait for a writer forever.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_secret_file(&fifo).map(|b| b.len()).map_err(|e| e.code()));
        });
        let r = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("returns at once");
        assert_eq!(r, Err(codes::core::INVALID_ARGUMENT));
    }
}
