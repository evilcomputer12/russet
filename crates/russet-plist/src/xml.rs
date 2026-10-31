// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The XML writer, in the layout of Apple's own tools: the standard prologue, one element per line, tab indents
//! starting at column 0 inside `<plist>`, `<dict/>` and `<array/>` for empty collections, only `&`, `<` and `>`
//! escaped, reals as `%.17g` (`DBL_DIG + 2`, as CoreFoundation formats them), dates as whole UTC seconds, and
//! base64 data on lines of at most `76 - 8 × depth` characters at the depth of its `<data>` element.

use plist::Value;

use crate::{PlistError, Result};

const PROLOGUE: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n";

/// Serializes `value` as an XML property list.
pub(crate) fn write(value: &Value) -> Result<Vec<u8>> {
    let mut out = String::with_capacity(1024);
    out.push_str(PROLOGUE);
    write_value(&mut out, value, 0)?;
    out.push_str("</plist>\n");
    Ok(out.into_bytes())
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn element(out: &mut String, depth: usize, name: &str, text: &str) {
    indent(out, depth);
    out.push('<');
    out.push_str(name);
    out.push('>');
    out.push_str(text);
    out.push_str("</");
    out.push_str(name);
    out.push_str(">\n");
}

fn empty(out: &mut String, depth: usize, name: &str) {
    indent(out, depth);
    out.push('<');
    out.push_str(name);
    out.push_str("/>\n");
}

fn write_value(out: &mut String, value: &Value, depth: usize) -> Result<()> {
    match value {
        Value::String(s) => element(out, depth, "string", &escape(s)?),
        Value::Boolean(true) => empty(out, depth, "true"),
        Value::Boolean(false) => empty(out, depth, "false"),
        Value::Integer(i) => element(out, depth, "integer", &i.to_string()),
        Value::Real(r) => element(out, depth, "real", &format_real(*r)),
        Value::Date(d) => element(out, depth, "date", &d.to_xml_format()),
        Value::Data(bytes) => write_data(out, bytes, depth),
        Value::Array(items) if items.is_empty() => empty(out, depth, "array"),
        Value::Array(items) => {
            indent(out, depth);
            out.push_str("<array>\n");
            for item in items {
                write_value(out, item, depth + 1)?;
            }
            indent(out, depth);
            out.push_str("</array>\n");
        }
        Value::Dictionary(dict) if dict.is_empty() => empty(out, depth, "dict"),
        Value::Dictionary(dict) => {
            indent(out, depth);
            out.push_str("<dict>\n");
            for (key, item) in dict {
                element(out, depth + 1, "key", &escape(key)?);
                write_value(out, item, depth + 1)?;
            }
            indent(out, depth);
            out.push_str("</dict>\n");
        }
        Value::Uid(_) => return Err(PlistError::Unsupported("UIDs exist only in binary property lists".into())),
        _ => return Err(PlistError::Unsupported("a value type this writer does not know".into())),
    }
    Ok(())
}

/// Escapes text content: `&`, `<` and `>` (as Apple's writer does), plus `\r` as a character reference, because
/// an XML parser would otherwise normalize it to `\n`. Other control characters cannot appear in XML 1.0.
fn escape(s: &str) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                return Err(PlistError::Unsupported(format!(
                    "the control character U+{:04X} cannot appear in an XML property list",
                    c as u32
                )));
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

/// Base64 lines at the depth of the `<data>` element; indents count 8 columns against the 76-column line, and
/// the indent is capped at 8 (CoreFoundation's rule).
fn write_data(out: &mut String, bytes: &[u8], depth: usize) {
    indent(out, depth);
    out.push_str("<data>\n");
    let width = 76 - 8 * depth.min(8);
    let chunk = width / 4 * 3;
    for line in bytes.chunks(chunk.max(3)) {
        indent(out, depth.min(8));
        out.push_str(&crate::base64(line));
        out.push('\n');
    }
    indent(out, depth);
    out.push_str("</data>\n");
}

/// `%.17g`, plus CoreFoundation's spellings of the non-finite values.
pub(crate) fn format_real(r: f64) -> String {
    if r.is_nan() {
        return "nan".into();
    }
    if r.is_infinite() {
        return if r > 0.0 { "+infinity".into() } else { "-infinity".into() };
    }
    if r == 0.0 {
        return if r.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    const PRECISION: i32 = 17;
    let sci = format!("{:.*e}", (PRECISION - 1) as usize, r);
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if !(-4..PRECISION).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", strip_zeros(mantissa), sign, exponent.unsigned_abs())
    } else {
        let decimals = (PRECISION - 1 - exponent).max(0) as usize;
        strip_zeros(&format!("{r:.decimals$}")).to_owned()
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

#[cfg(test)]
mod tests {
    use plist::Dictionary;

    use super::*;

    fn xml(value: &Value) -> String {
        String::from_utf8(write(value).expect("writes")).expect("utf-8")
    }

    #[test]
    fn layout_matches_apples_writer() {
        let mut d = Dictionary::new();
        d.insert("CFBundleIdentifier".into(), Value::from("dev.russet.test.probe"));
        d.insert("Flags".into(), Value::Array(vec![Value::Boolean(true), Value::from(2)]));
        d.insert("Empty".into(), Value::Dictionary(Dictionary::new()));
        d.insert("None".into(), Value::Array(Vec::new()));
        let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>CFBundleIdentifier</key>\n\
\t<string>dev.russet.test.probe</string>\n\
\t<key>Flags</key>\n\
\t<array>\n\
\t\t<true/>\n\
\t\t<integer>2</integer>\n\
\t</array>\n\
\t<key>Empty</key>\n\
\t<dict/>\n\
\t<key>None</key>\n\
\t<array/>\n\
</dict>\n\
</plist>\n";
        assert_eq!(xml(&Value::Dictionary(d)), expected);
    }

    #[test]
    fn only_ampersand_and_angle_brackets_are_escaped() {
        let s = xml(&Value::from("a & b < c > d \"e\" 'f'"));
        assert!(s.contains("<string>a &amp; b &lt; c &gt; d \"e\" 'f'</string>"), "{s}");
        assert!(matches!(write(&Value::from("bell\u{7}")), Err(PlistError::Unsupported(_))));
        assert!(xml(&Value::from("a\rb")).contains("a&#13;b"));
    }

    #[test]
    fn reals_use_seventeen_significant_digits() {
        assert_eq!(format_real(0.5), "0.5");
        assert_eq!(format_real(1.0), "1");
        assert_eq!(format_real(-2.0), "-2");
        assert_eq!(format_real(0.1), "0.10000000000000001");
        assert_eq!(format_real(123456.789), "123456.789");
        assert_eq!(format_real(0.3), "0.29999999999999999");
        assert_eq!(format_real(1.0 / 3.0), "0.33333333333333331");
        assert_eq!(format_real(1e16), "10000000000000000");
        assert_eq!(format_real(1e17), "1e+17");
        assert_eq!(format_real(1e-5), "1.0000000000000001e-05");
        assert_eq!(format_real(1e23), "9.9999999999999992e+22");
        assert_eq!(format_real(5e-324), "4.9406564584124654e-324");
        assert_eq!(format_real(1e20), "1e+20");
        assert_eq!(format_real(1.5e-7), "1.4999999999999999e-07");
        assert_eq!(format_real(0.0001), "0.0001");
        assert_eq!(format_real(f64::INFINITY), "+infinity");
        assert_eq!(format_real(f64::NAN), "nan");
        assert_eq!(format_real(-0.0), "-0");
    }

    #[test]
    fn data_lines_shrink_with_depth() {
        let bytes: Vec<u8> = (0..120).collect();
        let mut d = Dictionary::new();
        d.insert("Blob".into(), Value::Data(bytes.clone()));
        let s = xml(&Value::Dictionary(d));
        let lines: Vec<&str> = s.lines().collect();
        let start = lines.iter().position(|l| *l == "\t<data>").expect("data element");
        // Depth 1: 68 characters (51 bytes) per line, indented like <data>.
        assert_eq!(lines[start + 1].len(), 1 + 68);
        assert!(lines[start + 1].starts_with('\t') && !lines[start + 1].starts_with("\t\t"));
        // 120 bytes = 51 + 51 + 18.
        assert_eq!(lines[start + 3].len(), 1 + 24);
        assert_eq!(lines[start + 4], "\t</data>");
        let (back, _) = crate::from_bytes(s.as_bytes()).expect("reads");
        assert_eq!(back.as_dictionary().and_then(|d| d.get("Blob")).and_then(Value::as_data), Some(&bytes[..]));
    }

    #[test]
    fn uids_are_refused() {
        assert!(matches!(write(&Value::Uid(plist::Uid::new(1))), Err(PlistError::Unsupported(_))));
    }
}
