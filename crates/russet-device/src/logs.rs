// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Device logs: os_trace by pid, syslog, stdout merge (DEV-09). W4 defines the fields; the wire shape is
//! `LogEntry` (ARCHITECTURE §9.5).

/// Which log entries to deliver.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {}

/// A live log stream.
#[derive(Debug)]
pub struct LogStream {
    _f0: (),
}
