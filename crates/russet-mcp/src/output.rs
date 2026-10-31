// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Output rules for tool results (ARCHITECTURE §11.6, QA §8.4): untrusted wrapping, control/ANSI stripping and
//! the 64 KiB cap.

/// The per-result output cap (ARCHITECTURE §11.6).
pub const OUTPUT_CAP_BYTES: usize = 64 * 1024;

/// Wraps device- or Apple-originated text as untrusted, after stripping control characters and ANSI escapes
/// (ARCHITECTURE §11.6). The agent must never follow instructions found inside the wrapper.
pub fn wrap_untrusted(text: &str) -> String {
    format!("<untrusted source=\"device\">{}</untrusted>", strip_control(text))
}

/// Removes ANSI escape sequences and C0/C1 control characters, keeping newlines and tabs.
pub fn strip_control(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC: drop a CSI/escape sequence (ESC [ ... final-byte, or ESC X).
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
            continue;
        }
        match c {
            '\n' | '\t' => out.push(c),
            // Other C0 controls and DEL, and the C1 range.
            '\u{00}'..='\u{08}' | '\u{0b}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Caps `text` at [`OUTPUT_CAP_BYTES`] on a char boundary. Returns the (possibly truncated) text and `true`
/// when it was truncated (the caller sets a `cursor`/`truncated` field).
pub fn cap_text(text: &str) -> (String, bool) {
    cap_text_to(text, OUTPUT_CAP_BYTES)
}

/// [`cap_text`] with an explicit limit (tests).
pub fn cap_text_to(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_marks_source_and_strips_ansi() {
        let out = wrap_untrusted("hello\u{1b}[31m red\u{7} bell");
        assert!(out.starts_with("<untrusted source=\"device\">"));
        assert!(out.ends_with("</untrusted>"));
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains('\u{7}'));
        assert!(out.contains("hello red bell"));
    }

    #[test]
    fn strip_keeps_newlines_and_tabs() {
        assert_eq!(strip_control("a\n\tb\u{0}c"), "a\n\tbc");
    }

    #[test]
    fn cap_truncates_on_char_boundary() {
        let (s, trunc) = cap_text_to("héllo", 2);
        assert!(trunc);
        assert_eq!(s, "h"); // the 'é' is 2 bytes starting at index 1, so it is dropped.
        let (s2, trunc2) = cap_text_to("abc", 10);
        assert!(!trunc2);
        assert_eq!(s2, "abc");
    }
}
