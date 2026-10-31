// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! JSON-RPC 2.0 messages (ARCHITECTURE §7.2–§7.3, §13.1).
//!
//! Parsing is separate from framing, so a frame that is not valid JSON gets a `-32700` reply and a JSON value
//! that is not a JSON-RPC message gets `-32600`, while the connection stays open. Params that are absent
//! become `{}` (params are always objects, ARCHITECTURE §7.3). Batches (arrays) are not supported: LSP never
//! sends them, and they get `-32600`.

use std::fmt;

use russet_core::error::{Error, ErrorData, codes};
use serde_json::{Map, Value, json};

/// JSON-RPC `-32700`: the frame is not valid JSON.
pub const PARSE_ERROR: i32 = -32700;
/// JSON-RPC `-32600`: not a valid request object.
pub const INVALID_REQUEST: i32 = -32600;
/// JSON-RPC `-32601`: unknown method.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC `-32602`: params that do not fit the method (`data.details.path` names the field).
pub const INVALID_PARAMS: i32 = -32602;
/// JSON-RPC `-32603`: internal error.
pub const INTERNAL_ERROR: i32 = -32603;
/// LSP `-32002`: a request before `initialize`.
pub const SERVER_NOT_INITIALIZED: i32 = -32002;
/// LSP `-32800`: the request was cancelled (`data.russetCode = "core.cancelled"`).
pub const REQUEST_CANCELLED: i32 = -32800;

/// A JSON-RPC request id. Server-originated ids are strings `s:<n>` (ARCHITECTURE §7.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequestId {
    Num(i64),
    Str(String),
}

impl RequestId {
    /// The id as JSON.
    pub fn to_value(&self) -> Value {
        match self {
            Self::Num(n) => json!(n),
            Self::Str(s) => json!(s),
        }
    }

    /// An id from JSON (integers and strings only).
    pub fn from_value(v: &Value) -> Option<Self> {
        match v {
            Value::Number(n) => n.as_i64().map(Self::Num),
            Value::String(s) => Some(Self::Str(s.clone())),
            _ => None,
        }
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{n}"),
            Self::Str(s) => f.write_str(s),
        }
    }
}

/// A JSON-RPC error object. `data` is boxed so results stay small.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Box<ErrorData>>,
}

impl RpcError {
    /// A standard JSON-RPC error whose `data` carries `russet` for Russet clients.
    pub fn standard(code: i32, message: impl Into<String>, russet: Error) -> Self {
        Self { code, message: message.into(), data: Some(Box::new(russet.to_data())) }
    }

    /// `-32601` for `method`.
    pub fn method_not_found(method: &str) -> Self {
        Self::standard(
            METHOD_NOT_FOUND,
            format!("method not found: {method}"),
            Error::new(codes::core::NOT_FOUND, format!("method not found: {method}"))
                .with_hint("Check `capabilities.methods` of the initialize result; the daemon may be older."),
        )
    }

    /// `-32602` with `details.path`.
    pub fn invalid_params(method: &str, path: &str, why: impl fmt::Display) -> Self {
        let message = if path.is_empty() || path == "." {
            format!("invalid params for {method}: {why}")
        } else {
            format!("invalid params for {method} at `{path}`: {why}")
        };
        Self::standard(
            INVALID_PARAMS,
            message.clone(),
            Error::new(codes::core::INVALID_ARGUMENT, message).with_details(json!({ "path": path })),
        )
    }

    /// `-32002`.
    pub fn not_initialized() -> Self {
        Self::standard(
            SERVER_NOT_INITIALIZED,
            "the connection is not initialized",
            Error::new(codes::ipc::NOT_INITIALIZED, "the connection is not initialized"),
        )
    }

    /// `-32800`.
    pub fn cancelled() -> Self {
        Self::from(&Error::cancelled())
    }

    /// `-32603` for a bug (a panicking handler, an unserializable result).
    pub fn internal(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::standard(INTERNAL_ERROR, message.clone(), Error::internal(message))
    }

    /// The error as a Russet [`Error`], for callers.
    pub fn to_error(&self) -> Error {
        Error::from_wire(self.code, self.message.clone(), self.data.as_deref())
    }

    fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("code".into(), json!(self.code));
        m.insert("message".into(), json!(self.message));
        if let Some(d) = &self.data
            && let Ok(v) = serde_json::to_value(d)
        {
            m.insert("data".into(), v);
        }
        Value::Object(m)
    }

    fn from_value(v: &Value) -> Self {
        let code = v.get("code").and_then(Value::as_i64).and_then(|c| i32::try_from(c).ok()).unwrap_or(INTERNAL_ERROR);
        let message = v.get("message").and_then(Value::as_str).unwrap_or("").to_owned();
        let data = v.get("data").and_then(|d| serde_json::from_value::<ErrorData>(d.clone()).ok()).map(Box::new);
        Self { code, message, data }
    }
}

impl From<&Error> for RpcError {
    fn from(e: &Error) -> Self {
        let code = if e.code() == codes::core::CANCELLED { REQUEST_CANCELLED } else { e.code().number() };
        Self { code, message: e.message().to_owned(), data: Some(Box::new(e.to_data())) }
    }
}

impl From<Error> for RpcError {
    fn from(e: Error) -> Self {
        Self::from(&e)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

/// A parsed JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// `id` is `None` only for errors about a message whose id could not be read.
    Response {
        id: Option<RequestId>,
        result: Result<Value, RpcError>,
    },
}

impl Message {
    /// Serializes to the JSON body of a frame.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut m = Map::new();
        m.insert("jsonrpc".into(), json!("2.0"));
        match self {
            Self::Request { id, method, params } => {
                m.insert("id".into(), id.to_value());
                m.insert("method".into(), json!(method));
                m.insert("params".into(), params.clone());
            }
            Self::Notification { method, params } => {
                m.insert("method".into(), json!(method));
                m.insert("params".into(), params.clone());
            }
            Self::Response { id, result } => {
                m.insert("id".into(), id.as_ref().map_or(Value::Null, RequestId::to_value));
                match result {
                    Ok(v) => m.insert("result".into(), v.clone()),
                    Err(e) => m.insert("error".into(), e.to_value()),
                };
            }
        }
        // Serializing a Value cannot fail.
        serde_json::to_vec(&Value::Object(m)).unwrap_or_default()
    }

    /// Parses a frame body. The error is the reply to send (with the id when it could be read).
    pub fn parse(body: &[u8]) -> Result<Self, (Option<RequestId>, RpcError)> {
        let v: Value = serde_json::from_slice(body).map_err(|e| {
            (
                None,
                RpcError::standard(
                    PARSE_ERROR,
                    format!("parse error: {e}"),
                    Error::invalid_argument(format!("the frame is not valid JSON: {e}")),
                ),
            )
        })?;
        Self::from_value(v)
    }

    fn from_value(v: Value) -> Result<Self, (Option<RequestId>, RpcError)> {
        let invalid = |id: Option<RequestId>, why: &str| {
            (
                id,
                RpcError::standard(
                    INVALID_REQUEST,
                    format!("invalid request: {why}"),
                    Error::invalid_argument(why.to_owned()),
                ),
            )
        };
        let Value::Object(mut m) = v else {
            return Err(invalid(None, "a message must be a JSON object (batches are not supported)"));
        };
        let id_value = m.remove("id");
        let id = id_value.as_ref().and_then(RequestId::from_value);
        if m.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(invalid(id, "`jsonrpc` must be \"2.0\""));
        }
        if let Some(method) = m.remove("method") {
            let Value::String(method) = method else {
                return Err(invalid(id, "`method` must be a string"));
            };
            let params = match m.remove("params") {
                None | Some(Value::Null) => Value::Object(Map::new()),
                Some(p) => p,
            };
            return match (id_value, id) {
                (None, _) => Ok(Self::Notification { method, params }),
                (Some(_), Some(id)) => Ok(Self::Request { id, method, params }),
                (Some(_), None) => Err(invalid(None, "`id` must be an integer or a string")),
            };
        }
        let Some(id_value) = id_value else {
            return Err(invalid(None, "neither a request nor a response"));
        };
        let id = RequestId::from_value(&id_value);
        if let Some(err) = m.remove("error") {
            return Ok(Self::Response { id, result: Err(RpcError::from_value(&err)) });
        }
        match m.remove("result") {
            Some(result) => Ok(Self::Response { id, result: Ok(result) }),
            None => Err(invalid(id, "a response needs `result` or `error`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_notifications_and_responses_round_trip() {
        let msgs = [
            Message::Request { id: RequestId::Num(1), method: "daemon.ping".into(), params: json!({}) },
            Message::Request { id: RequestId::Str("s:4".into()), method: "ui.confirm".into(), params: json!({"a": 1}) },
            Message::Notification {
                method: "$/progress".into(),
                params: json!({"token": 1, "value": {"kind": "end"}}),
            },
            Message::Response { id: Some(RequestId::Num(2)), result: Ok(json!({"x": [1, 2]})) },
            Message::Response { id: Some(RequestId::Num(3)), result: Err(RpcError::method_not_found("nope")) },
            Message::Response { id: None, result: Err(RpcError::cancelled()) },
        ];
        for m in msgs {
            assert_eq!(Message::parse(&m.to_bytes()).expect("parses"), m);
        }
    }

    #[test]
    fn absent_params_become_an_empty_object() {
        let m = Message::parse(br#"{"jsonrpc":"2.0","id":7,"method":"daemon.ping"}"#).expect("parses");
        assert_eq!(m, Message::Request { id: RequestId::Num(7), method: "daemon.ping".into(), params: json!({}) });
    }

    #[test]
    fn malformed_input_gets_the_standard_codes() {
        let (id, e) = Message::parse(b"{not json").expect_err("parse error");
        assert_eq!((id, e.code), (None, PARSE_ERROR));
        let (id, e) = Message::parse(br#"[{"jsonrpc":"2.0","method":"x"}]"#).expect_err("batch");
        assert_eq!((id, e.code), (None, INVALID_REQUEST));
        let (id, e) = Message::parse(br#"{"jsonrpc":"1.0","id":3,"method":"x"}"#).expect_err("version");
        assert_eq!((id, e.code), (Some(RequestId::Num(3)), INVALID_REQUEST));
        let (_, e) = Message::parse(br#"{"jsonrpc":"2.0","id":3}"#).expect_err("no result");
        assert_eq!(e.code, INVALID_REQUEST);
    }

    #[test]
    fn errors_map_to_the_wire_and_back() {
        let e = Error::new(codes::device::LOCKED, "locked").with_hint("unlock");
        let rpc = RpcError::from(&e);
        assert_eq!(rpc.code, 3002);
        let back = rpc.to_error();
        assert_eq!(back.code(), codes::device::LOCKED);
        assert_eq!(back.hint(), Some("unlock"));
        let cancelled = RpcError::cancelled();
        assert_eq!(cancelled.code, REQUEST_CANCELLED);
        assert_eq!(cancelled.data.as_ref().map(|d| d.russet_code.as_str()), Some("core.cancelled"));
        assert_eq!(cancelled.to_error().code(), codes::core::CANCELLED);
        let p = RpcError::invalid_params("job.get", "jobId", "missing field");
        assert_eq!(p.code, INVALID_PARAMS);
        assert_eq!(p.data.and_then(|d| d.details), Some(json!({"path": "jobId"})));
    }
}
