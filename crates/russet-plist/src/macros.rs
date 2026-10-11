// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Xcode build-setting macro expansion (`INFOPLIST_EXPAND_BUILD_SETTINGS` semantics, build §4.4).
//!
//! - References: `$(NAME)`, `${NAME}`, bare `$NAME`, and nested names such as `$($(DEPLOYMENT_TARGET_SETTING_NAME))`.
//!   `$$` is a literal `$`. An unset setting expands to the empty string, as in Xcode.
//! - Operators, chained left to right: `$(NAME:lower)`, `:upper`, `:rfc1034identifier`, `:c99extidentifier`,
//!   `:identifier`, `:quote`, `:dir`, `:file`, `:base`, `:suffix`, `:standardizepath` and `:default=VALUE` (used
//!   when the value is empty; `VALUE` may contain references). An unknown operator is an error, so that a typo
//!   never silently changes a build.
//! - Layers: a [`SettingsStack`] holds layers from lowest to highest precedence. `$(inherited)`, and a setting
//!   that refers to its own name, expand to the same setting in the layers below the one that defines it.

use std::collections::BTreeMap;

/// Nested references deeper than this are reported as a cycle.
const MAX_DEPTH: usize = 64;

/// A macro expansion error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MacroError {
    /// `$(` or `${` without its closing bracket.
    #[error("unterminated build-setting reference in `{0}`")]
    Unterminated(String),
    /// `$(NAME:op)` with an operator this expander does not know.
    #[error("unknown build-setting operator `{0}`")]
    UnknownOperator(String),
    /// A reference cycle, or nesting deeper than 64 levels.
    #[error("build setting `{0}` refers to itself (cycle or nesting too deep)")]
    Recursion(String),
}

/// Layered build settings (lowest precedence first), each layer labelled for provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsStack {
    layers: Vec<(String, BTreeMap<String, String>)>,
}

impl SettingsStack {
    /// An empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a layer above the existing ones; `label` names its source (for example `russet.toml`).
    pub fn push(&mut self, label: impl Into<String>, settings: BTreeMap<String, String>) -> &mut Self {
        self.layers.push((label.into(), settings));
        self
    }

    /// Sets `name` in the top layer (adding an unlabelled layer when the stack is empty).
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        if self.layers.is_empty() {
            self.layers.push((String::new(), BTreeMap::new()));
        }
        if let Some((_, top)) = self.layers.last_mut() {
            top.insert(name.into(), value.into());
        }
        self
    }

    /// The unexpanded value of `name` from the highest layer that defines it.
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.layers.iter().rev().find_map(|(_, l)| l.get(name).map(String::as_str))
    }

    /// The label of the highest layer that defines `name`.
    pub fn source(&self, name: &str) -> Option<&str> {
        self.layers.iter().rev().find(|(_, l)| l.contains_key(name)).map(|(label, _)| label.as_str())
    }

    /// Every setting name defined in any layer, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.layers.iter().flat_map(|(_, l)| l.keys().map(String::as_str)).collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The fully expanded value of setting `name` (empty when unset).
    pub fn get(&self, name: &str) -> Result<String, MacroError> {
        self.setting(name, self.layers.len(), 0)
    }

    /// Expands every reference in `text`.
    pub fn expand(&self, text: &str) -> Result<String, MacroError> {
        self.expand_in(text, None, 0)
    }

    /// The value of `name` from the highest layer below `upto` (exclusive), expanded.
    fn setting(&self, name: &str, upto: usize, depth: usize) -> Result<String, MacroError> {
        if depth > MAX_DEPTH {
            return Err(MacroError::Recursion(name.to_owned()));
        }
        let found = self.layers[..upto].iter().enumerate().rev().find_map(|(i, (_, l))| l.get(name).map(|v| (i, v)));
        match found {
            Some((layer, raw)) => self.expand_in(raw, Some((name, layer)), depth + 1),
            None => Ok(String::new()),
        }
    }

    /// Expands `text`; `ctx` is the setting being evaluated and its layer (for `$(inherited)`).
    fn expand_in(&self, text: &str, ctx: Option<(&str, usize)>, depth: usize) -> Result<String, MacroError> {
        if depth > MAX_DEPTH {
            return Err(MacroError::Recursion(ctx.map_or_else(|| text.to_owned(), |(n, _)| n.to_owned())));
        }
        let mut out = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'$' {
                let next = text[i..].find('$').map_or(text.len(), |p| i + p);
                out.push_str(&text[i..next]);
                i = next;
                continue;
            }
            match bytes.get(i + 1) {
                Some(b'$') => {
                    out.push('$');
                    i += 2;
                }
                Some(&open @ (b'(' | b'{')) => {
                    let close = if open == b'(' { b')' } else { b'}' };
                    let end =
                        matching(bytes, i + 2, open, close).ok_or_else(|| MacroError::Unterminated(text.to_owned()))?;
                    let inner = &text[i + 2..end];
                    out.push_str(&self.reference(inner, ctx, depth)?);
                    i = end + 1;
                }
                Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
                    let end = bytes[i + 1..]
                        .iter()
                        .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
                        .map_or(bytes.len(), |p| i + 1 + p);
                    out.push_str(&self.lookup(&text[i + 1..end], ctx, depth)?);
                    i = end;
                }
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    /// One `NAME(:op)*` reference body.
    fn reference(&self, inner: &str, ctx: Option<(&str, usize)>, depth: usize) -> Result<String, MacroError> {
        let parts = split_top_level(inner, b':');
        let name = self.expand_in(parts[0], ctx, depth + 1)?;
        let mut value = self.lookup(name.trim(), ctx, depth)?;
        for op in &parts[1..] {
            value = self.apply(op, value, ctx, depth)?;
        }
        Ok(value)
    }

    fn lookup(&self, name: &str, ctx: Option<(&str, usize)>, depth: usize) -> Result<String, MacroError> {
        match ctx {
            // `$(inherited)`, or a setting that names itself: the same setting from the layers below.
            Some((current, layer)) if name == "inherited" || name == current => self.setting(current, layer, depth + 1),
            _ if name == "inherited" => Ok(String::new()),
            _ => self.setting(name, self.layers.len(), depth + 1),
        }
    }

    fn apply(&self, op: &str, value: String, ctx: Option<(&str, usize)>, depth: usize) -> Result<String, MacroError> {
        if let Some(default) = op.strip_prefix("default=") {
            return if value.is_empty() { self.expand_in(default, ctx, depth + 1) } else { Ok(value) };
        }
        Ok(match op {
            "lower" => value.to_lowercase(),
            "upper" => value.to_uppercase(),
            "rfc1034identifier" => rfc1034_identifier(&value),
            "c99extidentifier" => c99_identifier(&value, true),
            "identifier" => c99_identifier(&value, false),
            "quote" => shell_quote(&value),
            "dir" => match value.rfind('/') {
                Some(p) => value[..=p].to_owned(),
                None => "./".to_owned(),
            },
            "file" => value.rsplit('/').next().unwrap_or_default().to_owned(),
            "base" => {
                let file = value.rsplit('/').next().unwrap_or_default();
                match file.rfind('.') {
                    Some(p) if p > 0 => file[..p].to_owned(),
                    _ => file.to_owned(),
                }
            }
            "suffix" => {
                let file = value.rsplit('/').next().unwrap_or_default();
                match file.rfind('.') {
                    Some(p) if p > 0 => file[p..].to_owned(),
                    _ => String::new(),
                }
            }
            "standardizepath" => standardize_path(&value),
            other => return Err(MacroError::UnknownOperator(other.to_owned())),
        })
    }
}

impl From<BTreeMap<String, String>> for SettingsStack {
    fn from(settings: BTreeMap<String, String>) -> Self {
        let mut s = Self::new();
        s.push("", settings);
        s
    }
}

/// Expands `text` against a single map of settings (no layers).
pub fn expand(text: &str, settings: &BTreeMap<String, String>) -> Result<String, MacroError> {
    SettingsStack::from(settings.clone()).expand(text)
}

/// The index of the bracket that closes the one opened just before `start`, counting nested `(`/`{` pairs.
fn matching(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut level = 0usize;
    for (offset, &b) in bytes[start..].iter().enumerate() {
        if b == open {
            level += 1;
        } else if b == close {
            if level == 0 {
                return Some(start + offset);
            }
            level -= 1;
        }
    }
    None
}

/// Splits on `sep` outside nested `$(…)`/`${…}` references.
fn split_top_level(s: &str, sep: u8) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut level = 0usize;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'{' => level += 1,
            b')' | b'}' => level = level.saturating_sub(1),
            _ if b == sep && level == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Keeps ASCII letters, digits, `-` and `.`; anything else becomes `-` (a legal DNS label character).
pub fn rfc1034_identifier(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' }).collect()
}

/// Replaces characters that cannot appear in a C identifier with `_` and prefixes `_` before a leading digit.
/// `extended` keeps non-ASCII letters and digits (C99 extended identifiers).
pub fn c99_identifier(s: &str, extended: bool) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            let ok = c.is_ascii_alphanumeric() || c == '_' || (extended && !c.is_ascii() && c.is_alphanumeric());
            if ok { c } else { '_' }
        })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_whitespace() || "\\\"'$`()[]{}*?;&|<>!#~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn standardize_path(s: &str) -> String {
    let absolute = s.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in s.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            p => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_owned()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(pairs: &[(&str, &str)]) -> SettingsStack {
        SettingsStack::from(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<BTreeMap<_, _>>())
    }

    #[test]
    fn plain_references_in_every_syntax() {
        let s = stack(&[("PRODUCT_NAME", "Hello"), ("SDK_NAME", "iphoneos27.0")]);
        assert_eq!(s.expand("$(PRODUCT_NAME)").as_deref(), Ok("Hello"));
        assert_eq!(s.expand("${PRODUCT_NAME}.app").as_deref(), Ok("Hello.app"));
        assert_eq!(s.expand("$PRODUCT_NAME-$SDK_NAME").as_deref(), Ok("Hello-iphoneos27.0"));
        assert_eq!(s.expand("cost: $$5 and $").as_deref(), Ok("cost: $5 and $"));
        assert_eq!(s.expand("[$(UNSET)]").as_deref(), Ok("[]"));
    }

    #[test]
    fn nested_names_resolve_like_the_platform_template() {
        // The iOS platform Info.plist writes MinimumOSVersion as $($(DEPLOYMENT_TARGET_SETTING_NAME)).
        let s = stack(&[
            ("DEPLOYMENT_TARGET_SETTING_NAME", "IPHONEOS_DEPLOYMENT_TARGET"),
            ("IPHONEOS_DEPLOYMENT_TARGET", "17.0"),
        ]);
        assert_eq!(s.expand("$($(DEPLOYMENT_TARGET_SETTING_NAME))").as_deref(), Ok("17.0"));
    }

    #[test]
    fn values_expand_recursively_and_cycles_are_errors() {
        let s = stack(&[("A", "$(B)-a"), ("B", "b"), ("LOOP1", "$(LOOP2)"), ("LOOP2", "$(LOOP1)")]);
        assert_eq!(s.get("A").as_deref(), Ok("b-a"));
        assert!(matches!(s.get("LOOP1"), Err(MacroError::Recursion(_))));
    }

    #[test]
    fn operators_chain_left_to_right() {
        let s = stack(&[("PRODUCT_NAME", "My App_1"), ("EMPTY", "")]);
        assert_eq!(s.expand("com.example.$(PRODUCT_NAME:rfc1034identifier)").as_deref(), Ok("com.example.My-App-1"));
        assert_eq!(s.expand("$(PRODUCT_NAME:rfc1034identifier:lower)").as_deref(), Ok("my-app-1"));
        assert_eq!(s.expand("$(PRODUCT_NAME:c99extidentifier)").as_deref(), Ok("My_App_1"));
        assert_eq!(s.expand("$(PRODUCT_NAME:upper)").as_deref(), Ok("MY APP_1"));
        assert_eq!(s.expand("$(EMPTY:default=$(PRODUCT_NAME:lower))").as_deref(), Ok("my app_1"));
        assert_eq!(s.expand("$(PRODUCT_NAME:default=x)").as_deref(), Ok("My App_1"));
        assert!(matches!(s.expand("$(PRODUCT_NAME:bogus)"), Err(MacroError::UnknownOperator(op)) if op == "bogus"));
        assert!(matches!(s.expand("$(PRODUCT_NAME"), Err(MacroError::Unterminated(_))));
    }

    #[test]
    fn identifier_operators_handle_digits_and_unicode() {
        assert_eq!(c99_identifier("3D Touch", true), "_3D_Touch");
        assert_eq!(c99_identifier("Größe", true), "Größe");
        assert_eq!(c99_identifier("Größe", false), "Gr__e");
        assert_eq!(rfc1034_identifier("a_b c.d-e"), "a-b-c.d-e");
    }

    #[test]
    fn path_operators() {
        let s = stack(&[("P", "/a/b/c.tar.gz"), ("F", "name"), ("U", "/a/./b//../c/")]);
        assert_eq!(s.expand("$(P:dir)").as_deref(), Ok("/a/b/"));
        assert_eq!(s.expand("$(P:file)").as_deref(), Ok("c.tar.gz"));
        assert_eq!(s.expand("$(P:base)").as_deref(), Ok("c.tar"));
        assert_eq!(s.expand("$(P:suffix)").as_deref(), Ok(".gz"));
        assert_eq!(s.expand("$(F:dir)").as_deref(), Ok("./"));
        assert_eq!(s.expand("$(F:suffix)").as_deref(), Ok(""));
        assert_eq!(s.expand("$(U:standardizepath)").as_deref(), Ok("/a/c"));
        assert_eq!(stack(&[("Q", "a b(c)")]).expand("$(Q:quote)").as_deref(), Ok("a\\ b\\(c\\)"));
        // Values are expanded too: `$c` in a value is a reference to the (unset) setting `c`.
        assert_eq!(stack(&[("Q", "a$c")]).get("Q").as_deref(), Ok("a"));
    }

    #[test]
    fn inherited_reaches_the_layer_below() {
        let mut s = SettingsStack::new();
        s.push("defaults", BTreeMap::from([("FLAGS".to_owned(), "-DBASE".to_owned())]));
        s.push("project", BTreeMap::from([("FLAGS".to_owned(), "$(inherited) -DPROJECT".to_owned())]));
        s.push("cli", BTreeMap::from([("FLAGS".to_owned(), "$(FLAGS) -DCLI".to_owned())]));
        assert_eq!(s.get("FLAGS").as_deref(), Ok("-DBASE -DPROJECT -DCLI"));
        assert_eq!(s.source("FLAGS"), Some("cli"));
        assert_eq!(s.raw("FLAGS"), Some("$(FLAGS) -DCLI"));
        // Outside a setting, $(inherited) has nothing to inherit.
        assert_eq!(s.expand("[$(inherited)]").as_deref(), Ok("[]"));
        assert_eq!(s.names(), vec!["FLAGS"]);
    }

    #[test]
    fn free_function_expands_against_one_map() {
        let m = BTreeMap::from([("X".to_owned(), "1".to_owned())]);
        assert_eq!(expand("v$(X)", &m).as_deref(), Ok("v1"));
    }
}
