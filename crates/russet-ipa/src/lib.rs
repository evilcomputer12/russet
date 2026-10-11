// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Deterministic IPA writer and reader (ARCHITECTURE §5.3 `russet-ipa`, build §4.8) [D 19]: sorted entries,
//! fixed mtimes, Unix modes from Mach-O magic, symlinks, Zip64, `dev` (stored) and `store` (deflate-6) modes,
//! `SwiftSupport/`.
//!
//! Layer F (MIT). F0 stub (W1); W5 fills the bodies (B07).

// F0 stubs: parameters of `todo!()` bodies are unused until the owner fills them.
#![allow(unused_variables)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::time::SystemTime;

use camino::{Utf8Path, Utf8PathBuf};

/// Errors of this crate.
#[derive(Debug, thiserror::Error)]
pub enum IpaError {
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `Result` with [`IpaError`].
pub type Result<T, E = IpaError> = std::result::Result<T, E>;

/// Compression mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpaMode {
    /// Stored entries, for fast development installs.
    Dev,
    /// Deflate level 6, for App Store uploads.
    Store,
}

/// Writer options.
#[derive(Debug, Clone)]
pub struct IpaOptions {
    pub mode: IpaMode,
    /// Swift runtime libraries to copy into `SwiftSupport/` (store IPAs).
    pub swift_support: Option<Vec<Utf8PathBuf>>,
    /// The fixed modification time of every entry (`SOURCE_DATE_EPOCH`).
    pub epoch: SystemTime,
}

/// What [`write()`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaReport {
    pub path: Utf8PathBuf,
    pub entries: usize,
    pub bytes: u64,
}

/// One entry of an IPA listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub unix_mode: u32,
    pub size: u64,
    pub symlink_target: Option<String>,
}

/// Packs `app` (a `.app` directory) into the IPA `out`.
pub fn write(app: &Utf8Path, out: &Utf8Path, o: IpaOptions) -> Result<IpaReport> {
    todo!("W5 (B07)")
}

/// Lists the entries of an IPA.
pub fn read_listing(ipa: &Utf8Path) -> Result<Vec<Entry>> {
    todo!("W5 (B07)")
}
