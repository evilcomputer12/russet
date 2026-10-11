// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Errors and error codes (ARCHITECTURE §13).
//!
//! An [`ErrorCode`] is a number (the JSON-RPC `error.code`, 1000–9999), a stable dotted name (the
//! `russetCode`, for example `device.locked`) and a CLI exit code (ARCHITECTURE §13.4). The codes live in
//! one file per family under [`codes`]; [`all_codes`] concatenates the families' `ALL` arrays.
//!
//! Every user-visible error carries a hint when a next step exists (ARCHITECTURE §13.1). A call site may set its
//! own with [`Error::with_hint`]; otherwise [`Error::to_data`] falls back to the code's default hint
//! ([`default_hint`]), which W2's families (`core`, `ipc`, `perm`) define for every code. Errors that crossed the
//! wire are rebuilt with [`Error::from_wire`], which keeps the remote `russetCode`, hint, details and chain.

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

    /// Looks a code up by its `russetCode`, for example `"device.locked"`.
    pub fn from_name(name: &str) -> Option<Self> {
        all_codes().iter().copied().find(|c| c.as_str() == name)
    }

    /// The family prefix before the dot (`"device"` for `device.locked`).
    pub fn prefix(self) -> &'static str {
        self.name.split_once('.').map_or(self.name, |(p, _)| p)
    }

    /// The anchor of this code in the generated error reference (ARCHITECTURE §13.1).
    pub fn docs_anchor(self) -> String {
        format!("docs/reference/errors.md#{}", self.name)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.number)
    }
}

/// The default hint of `code`, used when an error carries none (ARCHITECTURE §13.1). W2's families define one
/// for every code; other families may add `(code, hint)` pairs to their own `HINTS` tables and to this
/// lookup when they need one.
pub fn default_hint(code: ErrorCode) -> Option<&'static str> {
    [codes::core::HINTS, codes::ipc::HINTS, codes::perm::HINTS]
        .iter()
        .flat_map(|table| table.iter())
        .find(|(c, _)| *c == code)
        .map(|(_, hint)| *hint)
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
    /// Causes that arrived as text (`error.data.chain` of a remote error); listed before `source`'s chain.
    chain: Vec<String>,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    /// Creates an error with `code` and a human-readable message.
    pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self {
        let inner = Inner {
            code,
            message: msg.into(),
            hint: None,
            details: None,
            retryable: false,
            chain: Vec::new(),
            source: None,
        };
        Self { inner: Box::new(inner) }
    }

    /// A `core.io` error for `what` (for example "writing runtime/russetd.json"), caused by `e`.
    pub fn io(what: impl fmt::Display, e: std::io::Error) -> Self {
        let code = if e.kind() == std::io::ErrorKind::StorageFull {
            codes::core::DISK_FULL
        } else if e.kind() == std::io::ErrorKind::NotFound {
            codes::core::NOT_FOUND
        } else {
            codes::core::IO
        };
        Self::new(code, format!("{what}: {e}")).caused_by(e)
    }

    /// `core.internal` with `msg`: a bug, never an expected condition.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(codes::core::INTERNAL, msg)
    }

    /// `core.invalidArgument` with `msg`.
    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Self::new(codes::core::INVALID_ARGUMENT, msg)
    }

    /// `core.notImplemented` with `msg`.
    pub fn not_implemented(msg: impl Into<String>) -> Self {
        Self::new(codes::core::NOT_IMPLEMENTED, msg)
    }

    /// `core.cancelled`.
    pub fn cancelled() -> Self {
        Self::new(codes::core::CANCELLED, "cancelled")
    }

    /// Rebuilds an error that crossed the wire (JSON-RPC `error`): the code comes from `data.russetCode`, then
    /// from `number`; an unknown code becomes `core.internal` with the remote code kept in `details.remoteCode`.
    pub fn from_wire(number: i32, message: impl Into<String>, data: Option<&ErrorData>) -> Self {
        let named = data.and_then(|d| ErrorCode::from_name(&d.russet_code));
        let code = named.or_else(|| ErrorCode::from_number(number)).unwrap_or_else(|| standard_code(number));
        let mut e = Self::new(code, message);
        if let Some(d) = data {
            e.inner.hint = d.hint.clone();
            e.inner.details = d.details.clone();
            e.inner.retryable = d.retryable;
            e.inner.chain = d.chain.clone();
            if named.is_none() && !d.russet_code.is_empty() && ErrorCode::from_number(number).is_none() {
                let mut details = match e.inner.details.take() {
                    Some(serde_json::Value::Object(m)) => m,
                    Some(other) => {
                        let mut m = serde_json::Map::new();
                        m.insert("details".to_owned(), other);
                        m
                    }
                    None => serde_json::Map::new(),
                };
                details.insert("remoteCode".to_owned(), serde_json::Value::String(d.russet_code.clone()));
                e.inner.details = Some(serde_json::Value::Object(details));
            }
        }
        e
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

    /// The hint set at the call site, if any (see [`Error::effective_hint`] for the fallback).
    pub fn hint(&self) -> Option<&str> {
        self.inner.hint.as_deref()
    }

    /// The call site's hint, or the code's [`default_hint`].
    pub fn effective_hint(&self) -> Option<&str> {
        self.inner.hint.as_deref().or_else(|| default_hint(self.inner.code))
    }

    /// The structured details, if any.
    pub fn details(&self) -> Option<&serde_json::Value> {
        self.inner.details.as_ref()
    }

    /// Whether retrying may succeed.
    pub fn is_retryable(&self) -> bool {
        self.inner.retryable
    }

    /// The cause chain, outermost first: remote causes, then the local source chain.
    pub fn chain(&self) -> Vec<String> {
        let mut chain = self.inner.chain.clone();
        let mut next: Option<&(dyn std::error::Error + 'static)> = match &self.inner.source {
            Some(s) => Some(s.as_ref()),
            None => None,
        };
        while let Some(e) = next {
            chain.push(e.to_string());
            next = e.source();
        }
        chain
    }

    /// The wire form for JSON-RPC `error.data` (ARCHITECTURE §13.1).
    pub fn to_data(&self) -> ErrorData {
        let inner = &self.inner;
        ErrorData {
            russet_code: inner.code.as_str().to_owned(),
            hint: self.effective_hint().map(str::to_owned),
            details: inner.details.clone(),
            retryable: inner.retryable,
            docs: Some(inner.code.docs_anchor()),
            chain: self.chain(),
        }
    }

    /// The one-line display form `device.locked (3002): iPhone 12 is locked. Hint: …` (ARCHITECTURE §13.1).
    pub fn display_line(&self) -> String {
        let mut line = format!("{} ({}): {}", self.inner.code.as_str(), self.inner.code.number(), self.inner.message);
        if let Some(hint) = self.effective_hint() {
            if !line.ends_with('.') {
                line.push('.');
            }
            line.push_str(" Hint: ");
            line.push_str(hint);
        }
        line
    }
}

/// The Russet code a standard JSON-RPC error code maps to on the client side.
fn standard_code(number: i32) -> ErrorCode {
    match number {
        -32800 => codes::core::CANCELLED,
        -32602 => codes::core::INVALID_ARGUMENT,
        -32601 => codes::core::NOT_FOUND,
        -32002 => codes::ipc::NOT_INITIALIZED,
        _ => codes::core::INTERNAL,
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    pub russet_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
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
    fn from_number_and_name_round_trip() {
        let c = codes::device::LOCKED;
        assert_eq!(ErrorCode::from_number(c.number()), Some(c));
        assert_eq!(ErrorCode::from_name("device.locked"), Some(c));
        assert_eq!(ErrorCode::from_number(42), None);
        assert_eq!(ErrorCode::from_name("device.nope"), None);
        assert_eq!(c.prefix(), "device");
    }

    #[test]
    fn every_core_ipc_and_perm_code_has_a_default_hint_except_the_self_explanatory_ones() {
        // ARCHITECTURE §13.1: a hint whenever a next step exists. A cancellation or a declined confirmation has
        // none beyond "run it again".
        let without = [codes::core::CANCELLED, codes::perm::CONFIRMATION_DECLINED];
        for family in [codes::core::ALL, codes::ipc::ALL, codes::perm::ALL] {
            for code in family {
                if without.contains(code) {
                    continue;
                }
                let hint = default_hint(*code).unwrap_or_else(|| panic!("{} has no default hint", code.as_str()));
                assert!(hint.ends_with('.') || hint.ends_with(')'), "{}: hints are sentences: {hint}", code.as_str());
            }
        }
        let e = Error::new(codes::ipc::DAEMON_UNAVAILABLE, "no daemon");
        assert!(e.to_data().hint.is_some_and(|h| h.contains("russet daemon start")));
        let own = Error::new(codes::ipc::DAEMON_UNAVAILABLE, "no daemon").with_hint("mine");
        assert_eq!(own.to_data().hint.as_deref(), Some("mine"));
    }

    #[test]
    fn remote_errors_keep_code_hint_details_and_chain() {
        let data = ErrorData {
            russet_code: "device.locked".into(),
            hint: Some("unlock it".into()),
            details: Some(serde_json::json!({"udid": "x"})),
            retryable: true,
            docs: None,
            chain: vec!["remote cause".into()],
        };
        let e = Error::from_wire(3002, "locked", Some(&data));
        assert_eq!(e.code(), codes::device::LOCKED);
        assert_eq!(e.hint(), Some("unlock it"));
        assert!(e.is_retryable());
        assert_eq!(e.chain(), vec!["remote cause".to_owned()]);
        assert_eq!(e.details(), Some(&serde_json::json!({"udid": "x"})));
        // Standard JSON-RPC codes without data.
        assert_eq!(Error::from_wire(-32800, "x", None).code(), codes::core::CANCELLED);
        assert_eq!(Error::from_wire(-32601, "x", None).code(), codes::core::NOT_FOUND);
        assert_eq!(Error::from_wire(-32602, "x", None).code(), codes::core::INVALID_ARGUMENT);
        assert_eq!(Error::from_wire(-32002, "x", None).code(), codes::ipc::NOT_INITIALIZED);
        // A code this build does not know (a newer daemon) keeps its name in details.
        let newer = ErrorData {
            russet_code: "device.fromTheFuture".into(),
            hint: None,
            details: None,
            retryable: false,
            docs: None,
            chain: Vec::new(),
        };
        let e = Error::from_wire(3099, "new", Some(&newer));
        assert_eq!(e.code(), codes::core::INTERNAL);
        assert_eq!(e.details().and_then(|d| d.get("remoteCode")), Some(&serde_json::json!("device.fromTheFuture")));
    }

    #[test]
    fn io_errors_map_to_their_codes() {
        let e = Error::io("reading x", std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert_eq!(e.code(), codes::core::NOT_FOUND);
        let e = Error::io("writing x", std::io::Error::other("boom"));
        assert_eq!(e.code(), codes::core::IO);
        assert!(e.message().starts_with("writing x: "));
    }

    #[test]
    fn display_line_follows_the_contract() {
        let e = Error::new(codes::device::LOCKED, "iPhone 12 is locked").with_hint("Unlock the iPhone.");
        assert_eq!(e.display_line(), "device.locked (3002): iPhone 12 is locked. Hint: Unlock the iPhone.");
    }

    #[test]
    fn error_data_omits_empty_optional_fields() {
        let d = Error::new(codes::core::CANCELLED, "cancelled").to_data();
        let v = serde_json::to_value(&d).expect("serializes");
        assert_eq!(
            v,
            serde_json::json!({"russetCode": "core.cancelled", "retryable": false,
                                "docs": "docs/reference/errors.md#core.cancelled", "chain": []})
        );
    }
}
