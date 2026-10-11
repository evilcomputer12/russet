// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Errors and error codes (ARCHITECTURE §13).
//!
//! An [`ErrorCode`] is a number (the JSON-RPC `error.code`, 1000–9999), a stable dotted name (the
//! `russetCode`, for example `device.locked`) and a CLI exit code (ARCHITECTURE §13.4). The codes live in
//! one file per family under [`codes`]; [`all_codes`] concatenates the families' `ALL` arrays.
//!
//! W1 gave `ErrorCode`, `Error` and `ErrorData` small working bodies so that F0 handlers can already return
//! `core.notImplemented`; W2 owns this file and refines it (freeze F1).

use std::fmt;

pub mod codes;

pub use codes::all_codes;

/// A registered error code: number, `russetCode` and CLI exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorCode {
    number: u16,
    name: &'static str,
    exit: u8,
}

impl ErrorCode {
    /// Defines a code. Only the family files under [`codes`] call this.
    pub const fn new(number: u16, name: &'static str, exit: u8) -> Self {
        Self { number, name, exit }
    }

    /// The JSON-RPC `error.code`, for example `3002`.
    pub const fn number(self) -> i32 {
        self.number as i32
    }

    /// The stable `russetCode`, for example `"device.locked"`. Tests assert codes, never message text.
    pub const fn as_str(self) -> &'static str {
        self.name
    }

    /// The CLI exit status for this code (ARCHITECTURE §13.4).
    pub const fn exit_code(self) -> u8 {
        self.exit
    }

    /// Looks a code up by its number.
    pub fn from_number(n: i32) -> Option<Self> {
        all_codes().iter().copied().find(|c| c.number() == n)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.number)
    }
}

/// A Russet error: a code, a message, an optional hint and details, a retryable flag and a source chain.
///
/// One pointer wide (the fields are boxed), so `Result<T>` stays small on every path.
pub struct Error {
    inner: Box<Inner>,
}

struct Inner {
    code: ErrorCode,
    message: String,
    hint: Option<String>,
    details: Option<serde_json::Value>,
    retryable: bool,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    /// Creates an error with `code` and a human-readable message.
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        let inner = Inner { code, message: msg.into(), hint: None, details: None, retryable: false, source: None };
        Self { inner: Box::new(inner) }
    }

    /// Adds the next step for the user (ARCHITECTURE §13.1: every user-visible error has one when it exists).
    pub fn with_hint(mut self, h: impl Into<String>) -> Self {
        self.inner.hint = Some(h.into());
        self
    }

    /// Adds structured details (`error.data.details`).
    pub fn with_details(mut self, d: serde_json::Value) -> Self {
        self.inner.details = Some(d);
        self
    }

    /// Marks the error as retryable.
    pub fn retryable(mut self, yes: bool) -> Self {
        self.inner.retryable = yes;
        self
    }

    /// Records the underlying cause; its chain becomes `error.data.chain`.
    pub fn caused_by(mut self, e: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.inner.source = Some(Box::new(e));
        self
    }

    /// The error's code.
    pub fn code(&self) -> ErrorCode {
        self.inner.code
    }

    /// The human-readable message.
    pub fn message(&self) -> &str {
        &self.inner.message
    }

    /// The hint, if any.
    pub fn hint(&self) -> Option<&str> {
        self.inner.hint.as_deref()
    }

    /// The wire form for JSON-RPC `error.data` (ARCHITECTURE §13.1).
    pub fn to_data(&self) -> ErrorData {
        let mut chain = Vec::new();
        let inner = &self.inner;
        let mut next: Option<&(dyn std::error::Error + 'static)> = match &inner.source {
            Some(s) => Some(s.as_ref()),
            None => None,
        };
        while let Some(e) = next {
            chain.push(e.to_string());
            next = e.source();
        }
        ErrorData {
            russet_code: inner.code.as_str().to_owned(),
            hint: inner.hint.clone(),
            details: inner.details.clone(),
            retryable: inner.retryable,
            docs: Some(format!("docs/reference/errors.md#{}", inner.code.as_str())),
            chain,
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.inner.code.as_str())
            .field("message", &self.inner.message)
            .field("hint", &self.inner.hint)
            .field("retryable", &self.inner.retryable)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inner.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.inner.source {
            Some(s) => Some(s.as_ref()),
            None => None,
        }
    }
}

/// JSON-RPC `error.data` (ARCHITECTURE §13.1), camelCase on the wire; optional fields are omitted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    pub russet_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(default)]
    pub chain: Vec<String>,
}

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_data_carries_code_hint_and_chain() {
        let io = std::io::Error::other("disk on fire");
        let e = Error::new(codes::core::IO, "write failed").with_hint("free some space").caused_by(io);
        let d = e.to_data();
        assert_eq!(d.russet_code, "core.io");
        assert_eq!(d.hint.as_deref(), Some("free some space"));
        assert_eq!(d.chain, vec!["disk on fire".to_owned()]);
        assert_eq!(d.docs.as_deref(), Some("docs/reference/errors.md#core.io"));
    }

    #[test]
    fn errors_stay_one_pointer_wide() {
        assert_eq!(std::mem::size_of::<Error>(), std::mem::size_of::<usize>());
    }

    #[test]
    fn from_number_round_trips() {
        let c = codes::device::LOCKED;
        assert_eq!(ErrorCode::from_number(c.number()), Some(c));
        assert_eq!(ErrorCode::from_number(42), None);
    }
}
