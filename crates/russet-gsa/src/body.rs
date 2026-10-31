// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Answers of Apple's services as they arrive: their shape, for debug logs that never contain the body, and
//! decoding that tolerates what proxies and Apple's front ends do to them (gzip without being asked, a BOM or
//! leading whitespace, binary instead of XML plists, JSON instead of plists).

use std::borrow::Cow;
use std::io::Read as _;

/// Bodies are inflated up to this size (Apple's answers here are a few KiB).
const MAX_INFLATED: u64 = 16 * 1024 * 1024;

/// The body without a UTF-8 BOM and leading whitespace.
fn trimmed(b: &[u8]) -> &[u8] {
    let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    &b[start..]
}

/// What a body looks like: `empty`, `gzip`, `binary-plist`, `xml-plist`, `xml`, `html`, `json` or `other`.
pub(crate) fn kind(b: &[u8]) -> &'static str {
    let t = trimmed(b);
    let lower: Vec<u8> = t.iter().take(512).map(u8::to_ascii_lowercase).collect();
    let has = |needle: &[u8]| lower.windows(needle.len()).any(|w| w == needle);
    if t.is_empty() {
        "empty"
    } else if t.starts_with(&[0x1f, 0x8b]) {
        "gzip"
    } else if t.starts_with(b"bplist") {
        "binary-plist"
    } else if lower.starts_with(b"<!doctype html") || lower.starts_with(b"<html") {
        "html"
    } else if t.starts_with(b"<") && has(b"<plist") {
        "xml-plist"
    } else if t.starts_with(b"<") {
        "xml"
    } else if t.starts_with(b"{") || t.starts_with(b"[") {
        "json"
    } else {
        "other"
    }
}

/// The body, inflated when it is gzip (by magic or `Content-Encoding`).
pub(crate) fn decoded<'a>(b: &'a [u8], content_encoding: Option<&str>) -> Cow<'a, [u8]> {
    let gzip = content_encoding.is_some_and(|e| e.eq_ignore_ascii_case("gzip")) || b.starts_with(&[0x1f, 0x8b]);
    if !gzip {
        return Cow::Borrowed(b);
    }
    let mut out = Vec::new();
    match flate2::read::GzDecoder::new(b).take(MAX_INFLATED).read_to_end(&mut out) {
        Ok(_) => Cow::Owned(out),
        Err(_) => Cow::Borrowed(b),
    }
}

/// The body as a plist dictionary (XML or binary).
pub(crate) fn plist_dict(b: &[u8]) -> Option<plist::Dictionary> {
    plist::from_bytes::<plist::Value>(trimmed(b)).ok()?.into_dictionary()
}

/// The body as JSON.
pub(crate) fn json(b: &[u8]) -> Option<serde_json::Value> {
    serde_json::from_slice(trimmed(b)).ok()
}

/// A JSON value as a plist value (numbers stay integers when they are).
pub(crate) fn json_to_plist(v: &serde_json::Value) -> plist::Value {
    match v {
        serde_json::Value::Null => plist::Value::String(String::new()),
        serde_json::Value::Bool(b) => plist::Value::Boolean(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => plist::Value::Integer(i.into()),
            None => plist::Value::Real(n.as_f64().unwrap_or_default()),
        },
        serde_json::Value::String(s) => plist::Value::String(s.clone()),
        serde_json::Value::Array(a) => plist::Value::Array(a.iter().map(json_to_plist).collect()),
        serde_json::Value::Object(m) => {
            plist::Value::Dictionary(m.iter().map(|(k, v)| (k.clone(), json_to_plist(v))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml(d: &plist::Dictionary) -> Vec<u8> {
        let mut out = Vec::new();
        plist::to_writer_xml(&mut out, &plist::Value::Dictionary(d.clone())).expect("xml");
        out
    }

    #[test]
    fn shapes_are_recognised_and_plists_decoded_through_gzip_bom_and_whitespace() {
        let mut d = plist::Dictionary::new();
        d.insert("resultCode".into(), 0.into());
        let x = xml(&d);
        assert_eq!(kind(&x), "xml-plist");
        let mut bin = Vec::new();
        plist::to_writer_binary(&mut bin, &plist::Value::Dictionary(d.clone())).expect("binary");
        assert_eq!(kind(&bin), "binary-plist");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &x).expect("gzip");
        let gz = gz.finish().expect("gzip");
        assert_eq!(kind(&gz), "gzip");
        let bom = [b"\xEF\xBB\xBF\n  ".as_slice(), &x].concat();
        for body in [&x, &bin, &bom] {
            assert_eq!(plist_dict(&decoded(body, None)).as_ref(), Some(&d));
        }
        assert_eq!(plist_dict(&decoded(&gz, Some("gzip"))).as_ref(), Some(&d));
        assert_eq!(kind(b""), "empty");
        assert_eq!(kind(b"  <!DOCTYPE html><html>"), "html");
        assert_eq!(kind(b"<xmlui><page/></xmlui>"), "xml");
        assert_eq!(kind(br#"{"resultCode":0}"#), "json");
        let j = json(br#"{"resultCode": 0, "teams": [{"teamId": "T"}]}"#).expect("json");
        let p = json_to_plist(&j);
        assert_eq!(
            p.as_dictionary().and_then(|d| d.get("resultCode")).and_then(plist::Value::as_signed_integer),
            Some(0)
        );
    }
}
