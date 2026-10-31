// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! A small JSON value, parser and writer for the debug harnesses (DAP messages, the fake stub's image lists).
//!
//! The testkit depends only on russet-core (W8 owns its manifest), so the harnesses carry their own JSON instead
//! of serde_json. It handles everything DAP and debugserver send: objects, arrays, strings with escapes
//! (including surrogate pairs), numbers, booleans and null.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

/// A JSON value. Objects keep their keys sorted, which is fine for test traffic.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// An object from `(key, value)` pairs.
    pub fn object<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Self {
        Json::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// A string value.
    pub fn str(s: impl Into<String>) -> Self {
        Json::String(s.into())
    }

    /// An array of strings.
    pub fn strings<S: Into<String>>(items: impl IntoIterator<Item = S>) -> Self {
        Json::Array(items.into_iter().map(|s| Json::String(s.into())).collect())
    }

    /// `self[key]` for objects; `Null` otherwise.
    pub fn get(&self, key: &str) -> &Json {
        const NULL: Json = Json::Null;
        match self {
            Json::Object(m) => m.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// `self[i]` for arrays; `Null` otherwise.
    pub fn at(&self, i: usize) -> &Json {
        const NULL: Json = Json::Null;
        match self {
            Json::Array(a) => a.get(i).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_f64().filter(|n| *n >= 0.0 && n.fract() == 0.0).map(|n| n as u64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Array(a) => a,
            _ => &[],
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Parses one JSON document (surrounding whitespace allowed).
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing characters at {}", p.i));
        }
        Ok(v)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Json::String(s.to_owned())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Self {
        Json::String(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Self {
        Json::Bool(b)
    }
}

impl From<u64> for Json {
    fn from(n: u64) -> Self {
        Json::Number(n as f64)
    }
}

impl From<i64> for Json {
    fn from(n: i64) -> Self {
        Json::Number(n as f64)
    }
}

impl fmt::Display for Json {
    /// Compact JSON.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(b) => write!(f, "{b}"),
            Json::Number(n) if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 => write!(f, "{}", *n as i64),
            Json::Number(n) => write!(f, "{n}"),
            Json::String(s) => write_string(f, s),
            Json::Array(a) => {
                f.write_char('[')?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_char(']')
            }
            Json::Object(m) => {
                f.write_char('{')?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        f.write_char(',')?;
                    }
                    write_string(f, k)?;
                    write!(f, ":{v}")?;
                }
                f.write_char('}')
            }
        }
    }
}

fn write_string(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c if u32::from(c) < 0x20 => write!(f, "\\u{:04x}", u32::from(c))?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expect(&mut self, lit: &str) -> Result<(), String> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(())
        } else {
            Err(format!("expected {lit} at {}", self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.expect("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Json::Bool(false)),
            Some(b'n') => self.expect("null").map(|()| Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(c) => Err(format!("unexpected {:?} at {}", char::from(c), self.i)),
            None => Err("unexpected end of input".to_owned()),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut m = BTreeMap::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Object(m));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.expect(":")?;
            let v = self.value()?;
            m.insert(k, v);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Object(m));
                }
                _ => return Err(format!("expected , or }} at {}", self.i)),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.i += 1;
        let mut a = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Array(a));
        }
        loop {
            a.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Array(a));
                }
                _ => return Err(format!("expected , or ] at {}", self.i)),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.i..self.i + 4).ok_or("short \\u escape")?;
        let text = std::str::from_utf8(digits).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|e| e.to_string())
    }

    fn string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return Err(format!("expected a string at {}", self.i));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek().ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let e = self.peek().ok_or("unterminated escape")?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF)
                            } else {
                                hi
                            };
                            char::from_u32(code).unwrap_or('\u{fffd}')
                        }
                        other => return Err(format!("bad escape \\{}", char::from(other))),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                c => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
        text.parse::<f64>().map(Json::Number).map_err(|e| format!("bad number {text}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn round_trips_dap_like_messages() {
        let text = r#"{"seq":3,"type":"response","request_seq":2,"command":"attach","success":true,
            "body":{"threads":[{"id":1,"name":"jitdemo"}],"msg":"a \"quoted\"\nline é 😀"}}"#;
        let v = Json::parse(text).unwrap();
        assert_eq!(v.get("command").as_str(), Some("attach"));
        assert_eq!(v.get("success").as_bool(), Some(true));
        assert_eq!(v.get("body").get("threads").at(0).get("id").as_u64(), Some(1));
        assert_eq!(v.get("body").get("msg").as_str(), Some("a \"quoted\"\nline é 😀"));
        let again = Json::parse(&v.to_string()).unwrap();
        assert_eq!(again, v);
    }

    #[test]
    fn writes_compact_json() {
        let v = Json::object([
            ("b", Json::Array(vec![Json::from(1u64), Json::Null, Json::from(false)])),
            ("a", Json::str("x\ty")),
            ("n", Json::Number(-2.5)),
            ("big", Json::from(4_294_967_296u64)),
        ]);
        assert_eq!(v.to_string(), r#"{"a":"x\ty","b":[1,null,false],"big":4294967296,"n":-2.5}"#);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Json::parse("{").is_err());
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("\"abc").is_err());
        assert!(Json::parse("{} x").is_err());
        assert!(Json::parse("nul").is_err());
    }
}
