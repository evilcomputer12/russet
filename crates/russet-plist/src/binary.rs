// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The binary (`bplist00`) writer. It lays objects out the way CoreFoundation does, so that a binary plist read
//! from an Apple tool and written back is byte-identical:
//!
//! - objects are listed depth-first from the root; a dictionary lists all its keys, then all its values;
//! - strings (keys and values alike), integers, reals, booleans, dates and data are written once per distinct
//!   value; arrays, dictionaries and UIDs are never uniqued;
//! - object references and offsets use the fewest bytes (1, 2, 4 or 8) that fit the object count and the
//!   offset-table position;
//! - integers use the smallest width (negative values always 8 bytes); strings are ASCII when they can be, else
//!   UTF-16BE.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plist::Value;

use crate::{PlistError, Result};

/// Seconds between the Unix epoch and Apple's reference date (2001-01-01T00:00:00Z).
const APPLE_EPOCH_OFFSET: u64 = 978_307_200;

/// One flattened object: a value, or a dictionary key (keys are strings that share uniquing with values).
#[derive(Clone, Copy)]
enum Obj<'a> {
    Value(&'a Value),
    Key(&'a str),
}

/// Uniquing key of a scalar.
#[derive(Hash, PartialEq, Eq)]
enum Scalar<'a> {
    Str(&'a str),
    Int(i128),
    Real(u64),
    Bool(bool),
    Date(u64),
    Data(&'a [u8]),
}

#[derive(Default)]
struct Flattener<'a> {
    objects: Vec<Obj<'a>>,
    scalars: HashMap<Scalar<'a>, usize>,
    /// Arrays, dictionaries and UIDs by address: each occurrence is its own object.
    others: HashMap<*const Value, usize>,
}

fn int_value(i: plist::Integer) -> i128 {
    match (i.as_signed(), i.as_unsigned()) {
        (Some(s), _) => i128::from(s),
        (None, Some(u)) => i128::from(u),
        (None, None) => 0,
    }
}

fn date_seconds(d: plist::Date) -> f64 {
    let t = SystemTime::from(d);
    let base = UNIX_EPOCH + Duration::from_secs(APPLE_EPOCH_OFFSET);
    match t.duration_since(base) {
        Ok(after) => after.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

fn scalar_key(v: &Value) -> Option<Scalar<'_>> {
    Some(match v {
        Value::String(s) => Scalar::Str(s),
        Value::Integer(i) => Scalar::Int(int_value(*i)),
        Value::Real(r) => Scalar::Real(r.to_bits()),
        Value::Boolean(b) => Scalar::Bool(*b),
        Value::Date(d) => Scalar::Date(date_seconds(*d).to_bits()),
        Value::Data(bytes) => Scalar::Data(bytes),
        _ => return None,
    })
}

impl<'a> Flattener<'a> {
    fn flatten(&mut self, v: &'a Value) {
        if let Some(key) = scalar_key(v) {
            if self.scalars.contains_key(&key) {
                return;
            }
            self.scalars.insert(key, self.objects.len());
        } else {
            let ptr: *const Value = v;
            if self.others.contains_key(&ptr) {
                return;
            }
            self.others.insert(ptr, self.objects.len());
        }
        self.objects.push(Obj::Value(v));
        match v {
            Value::Dictionary(d) => {
                for k in d.keys() {
                    self.flatten_key(k);
                }
                for item in d.values() {
                    self.flatten(item);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.flatten(item);
                }
            }
            _ => {}
        }
    }

    fn flatten_key(&mut self, k: &'a str) {
        let key = Scalar::Str(k);
        if !self.scalars.contains_key(&key) {
            self.scalars.insert(key, self.objects.len());
            self.objects.push(Obj::Key(k));
        }
    }

    fn ref_of(&self, v: &Value) -> usize {
        match scalar_key(v) {
            Some(key) => self.scalars.get(&key).copied().unwrap_or_default(),
            None => {
                let ptr: *const Value = v;
                self.others.get(&ptr).copied().unwrap_or_default()
            }
        }
    }

    fn ref_of_key(&self, k: &str) -> usize {
        self.scalars.get(&Scalar::Str(k)).copied().unwrap_or_default()
    }
}

/// The number of bytes needed for values up to `count` (exclusive bound, as CoreFoundation sizes them).
fn size_for(count: u64) -> u8 {
    if count < 1 << 8 {
        1
    } else if count < 1 << 16 {
        2
    } else if count < 1 << 32 {
        4
    } else {
        8
    }
}

fn push_uint(out: &mut Vec<u8>, value: u64, width: u8) {
    let bytes = value.to_be_bytes();
    out.extend_from_slice(&bytes[8 - usize::from(width)..]);
}

/// A collection or string header: the marker with the length in its low nibble, or `0xF` plus an integer object.
fn push_size(out: &mut Vec<u8>, token: u8, size: usize) {
    let size = size as u64;
    if size < 15 {
        out.push(token | size as u8);
    } else {
        out.push(token | 0x0F);
        let width = size_for(size);
        out.push(0x10 | width.trailing_zeros() as u8);
        push_uint(out, size, width);
    }
}

fn push_int(out: &mut Vec<u8>, v: i128) -> Result<()> {
    if v < 0 {
        let v = i64::try_from(v).map_err(|_| PlistError::Unsupported(format!("integer {v} is out of range")))?;
        out.push(0x13);
        out.extend_from_slice(&v.to_be_bytes());
    } else if v < 1 << 8 {
        out.push(0x10);
        push_uint(out, v as u64, 1);
    } else if v < 1 << 16 {
        out.push(0x11);
        push_uint(out, v as u64, 2);
    } else if v < 1 << 32 {
        out.push(0x12);
        push_uint(out, v as u64, 4);
    } else if v < 1 << 63 {
        out.push(0x13);
        push_uint(out, v as u64, 8);
    } else if v < 1 << 64 {
        out.push(0x14);
        out.extend_from_slice(&v.to_be_bytes());
    } else {
        return Err(PlistError::Unsupported(format!("integer {v} is out of range")));
    }
    Ok(())
}

fn push_string(out: &mut Vec<u8>, s: &str) {
    if s.is_ascii() {
        push_size(out, 0x50, s.len());
        out.extend_from_slice(s.as_bytes());
    } else {
        let units: Vec<u16> = s.encode_utf16().collect();
        push_size(out, 0x60, units.len());
        for u in units {
            out.extend_from_slice(&u.to_be_bytes());
        }
    }
}

/// Serializes `value` as a binary property list.
pub(crate) fn write(value: &Value) -> Result<Vec<u8>> {
    let mut f = Flattener::default();
    f.flatten(value);
    let count = f.objects.len();
    let ref_size = size_for(count as u64);
    let mut out = Vec::with_capacity(64 + count * 16);
    out.extend_from_slice(b"bplist00");
    let mut offsets = Vec::with_capacity(count);
    for obj in &f.objects {
        offsets.push(out.len() as u64);
        let v = match obj {
            Obj::Key(k) => {
                push_string(&mut out, k);
                continue;
            }
            Obj::Value(v) => *v,
        };
        match v {
            Value::Boolean(false) => out.push(0x08),
            Value::Boolean(true) => out.push(0x09),
            Value::Integer(i) => push_int(&mut out, int_value(*i))?,
            Value::Real(r) => {
                out.push(0x23);
                out.extend_from_slice(&r.to_be_bytes());
            }
            Value::Date(d) => {
                out.push(0x33);
                out.extend_from_slice(&date_seconds(*d).to_be_bytes());
            }
            Value::Data(bytes) => {
                push_size(&mut out, 0x40, bytes.len());
                out.extend_from_slice(bytes);
            }
            Value::String(s) => push_string(&mut out, s),
            Value::Uid(uid) => {
                let n = uid.get();
                let width = size_for(n);
                out.push(0x80 | (width - 1));
                push_uint(&mut out, n, width);
            }
            Value::Array(items) => {
                push_size(&mut out, 0xA0, items.len());
                for item in items {
                    push_uint(&mut out, f.ref_of(item) as u64, ref_size);
                }
            }
            Value::Dictionary(d) => {
                push_size(&mut out, 0xD0, d.len());
                for k in d.keys() {
                    push_uint(&mut out, f.ref_of_key(k) as u64, ref_size);
                }
                for item in d.values() {
                    push_uint(&mut out, f.ref_of(item) as u64, ref_size);
                }
            }
            _ => return Err(PlistError::Unsupported("a value type this writer does not know".into())),
        }
    }
    let table = out.len() as u64;
    let offset_size = size_for(table);
    for o in offsets {
        push_uint(&mut out, o, offset_size);
    }
    // Trailer: 5 unused bytes, sort version, offset size, reference size, object count, top object, table offset.
    out.extend_from_slice(&[0; 5]);
    out.push(0);
    out.push(offset_size);
    out.push(ref_size);
    out.extend_from_slice(&(count as u64).to_be_bytes());
    out.extend_from_slice(&(f.ref_of(value) as u64).to_be_bytes());
    out.extend_from_slice(&table.to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use plist::Dictionary;

    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Expected bytes were produced by CPython 3.14's `plistlib.dumps(fmt=FMT_BINARY, sort_keys=False)`, whose
    /// layout reproduces Apple-written binary plists byte for byte (checked against the iOS 27 SDK's
    /// SDKSettings.plist on the dev box; that file is never committed).
    #[test]
    fn layout_matches_the_reference_writer() {
        let mut d = Dictionary::new();
        d.insert("Version".into(), Value::from("27.0"));
        d.insert("Targets".into(), Value::Array(vec![Value::from("27.0"), Value::from(2), Value::Boolean(true)]));
        d.insert("Count".into(), Value::from(300));
        d.insert("Neg".into(), Value::from(-1));
        d.insert("Dup".into(), Value::Boolean(true));
        let bytes = write(&Value::Dictionary(d)).expect("writes");
        assert_eq!(
            hex(&bytes),
            concat!(
                "62706c6973743030d5010203040506070a0b095756657273696f6e575461726765747355436f756e74534e6567",
                "534475705432372e30a306080910020911012c13ffffffffffffffff08131b23292d31363a3c3d4000000000",
                "00000101000000000000000c00000000000000000000000000000049",
            )
        );
    }

    #[test]
    fn long_strings_and_unicode_use_extended_sizes() {
        let long = "x".repeat(20);
        let bytes = write(&Value::Array(vec![Value::from(long.as_str()), Value::from("é")])).expect("writes");
        assert_eq!(
            hex(&bytes),
            concat!(
                "62706c6973743030a201025f101478787878787878787878787878787878787878786100e9080b2200000000",
                "00000101000000000000000300000000000000000000000000000025",
            )
        );
    }

    #[test]
    fn big_and_small_integers_round_trip() {
        let values = [0i128, 255, 256, 65_535, 65_536, (1 << 32) - 1, 1 << 32, (1 << 63) - 1, -1, i128::from(i64::MIN)];
        let mut items: Vec<Value> =
            values.iter().map(|v| if *v < 0 { Value::from(*v as i64) } else { Value::from(*v as u64) }).collect();
        items.push(Value::from(u64::MAX));
        let v = Value::Array(items);
        let bytes = write(&v).expect("writes");
        let (back, _) = crate::from_bytes(&bytes).expect("reads");
        assert_eq!(back, v);
    }

    #[test]
    fn many_objects_widen_references() {
        let items: Vec<Value> = (0..300).map(|i| Value::from(format!("item {i}"))).collect();
        let v = Value::Array(items);
        let bytes = write(&v).expect("writes");
        let trailer = &bytes[bytes.len() - 32..];
        assert_eq!(trailer[7], 2, "reference size");
        let (back, _) = crate::from_bytes(&bytes).expect("reads");
        assert_eq!(back, v);
    }

    #[test]
    fn uids_and_dates_round_trip() {
        let date = plist::Date::from_xml_format("1999-01-02T03:04:05Z").expect("date");
        let v =
            Value::Array(vec![Value::Uid(plist::Uid::new(7)), Value::Uid(plist::Uid::new(70_000)), Value::Date(date)]);
        let (back, _) = crate::from_bytes(&write(&v).expect("writes")).expect("reads");
        assert_eq!(back, v);
    }
}
