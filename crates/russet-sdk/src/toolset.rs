// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! SwiftPM toolsets for an SDK (ARCHITECTURE §16.3–§16.4; build §3.4).
//!
//! - `toolset-swb.json` (`swift build --build-system swiftbuild --toolset …`): the SDK's linker and librarian, and
//!   the link-time SDK fix `-Xclang-linker -isysroot -Xclang-linker <SDK>` in `swiftCompiler.extraCLIOptions`.
//!   Swift Build maps toolset Swift flags into `OTHER_LDFLAGS` of swiftc-driven links, so every Mach-O it links,
//!   package dylibs included, records `sdk 27.0` instead of the deployment target [V build V5–V6].
//! - `toolset-native.json` (the `native` fallback backend) also carries `-Xfrontend -enable-cross-import-overlays`
//!   and `-use-ld=lld`, as xtool's own toolset does [V build V1].
//!
//! The diagnosis-only escape hatch `RUSSET_LINK_SDK_VERSION = "deployment"` builds a toolset without the fix
//! ([`Toolset::without_sdk_fix`]); it is never used for release builds.

use camino::{Utf8Path, Utf8PathBuf};
use russet_core::Result;
use russet_core::error::{Error, codes};

/// Which backend a toolset is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsetKind {
    Swiftbuild,
    Native,
}

/// A SwiftPM toolset (schema 1.0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolset {
    pub root_path: Utf8PathBuf,
    pub linker: String,
    pub librarian: String,
    pub swift_flags: Vec<String>,
}

/// The four arguments that make swiftc-driven links read `SDKSettings.json` (§16.4 item 1).
pub fn sdk_fix_flags(sdk: &Utf8Path) -> Vec<String> {
    vec!["-Xclang-linker".into(), "-isysroot".into(), "-Xclang-linker".into(), sdk.to_string()]
}

impl Toolset {
    /// The toolset for `kind`, with the SDK fix.
    pub fn new(kind: ToolsetKind, linker_dir: &Utf8Path, sdk: &Utf8Path) -> Self {
        let mut swift_flags = Vec::new();
        if kind == ToolsetKind::Native {
            swift_flags.extend(["-Xfrontend", "-enable-cross-import-overlays", "-use-ld=lld"].map(String::from));
        }
        swift_flags.extend(sdk_fix_flags(sdk));
        Self { root_path: linker_dir.to_owned(), linker: "ld64.lld".into(), librarian: "llvm-lib".into(), swift_flags }
    }

    /// The same toolset without the SDK fix (sdk = minos; diagnosis only).
    pub fn without_sdk_fix(mut self, sdk: &Utf8Path) -> Self {
        let fix = sdk_fix_flags(sdk);
        if let Some(pos) = self.swift_flags.windows(fix.len()).position(|w| w == fix.as_slice()) {
            self.swift_flags.drain(pos..pos + fix.len());
        }
        self
    }

    /// The JSON text, with a stable key order and a trailing newline.
    pub fn to_json(&self) -> String {
        let mut v = serde_json::json!({
            "schemaVersion": "1.0",
            "rootPath": self.root_path,
            "linker": {"path": self.linker},
            "librarian": {"path": self.librarian},
        });
        if !self.swift_flags.is_empty() {
            v["swiftCompiler"] = serde_json::json!({"extraCLIOptions": self.swift_flags});
        }
        let mut text = serde_json::to_string_pretty(&v).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Writes the toolset to `path` unless it already holds the same text (so SwiftPM sees no change).
    pub fn write(&self, path: &Utf8Path) -> Result<()> {
        let text = self.to_json();
        if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, e))?;
        }
        let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text).map_err(|e| io_error(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| io_error(path, e))
    }
}

pub(crate) fn io_error(path: &Utf8Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("{path}: {e}"))
        .with_hint("Check that Russet's data directory exists and is writable (`russet doctor`).")
        .caused_by(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swiftbuild_toolset_carries_the_sdk_fix() {
        let t = Toolset::new(
            ToolsetKind::Swiftbuild,
            Utf8Path::new("/b/toolset/bin"),
            Utf8Path::new("/b/iPhoneOS27.0.sdk"),
        );
        let json: serde_json::Value = serde_json::from_str(&t.to_json()).unwrap_or_default();
        assert_eq!(json["schemaVersion"], "1.0");
        assert_eq!(json["rootPath"], "/b/toolset/bin");
        assert_eq!(json["linker"]["path"], "ld64.lld");
        assert_eq!(json["librarian"]["path"], "llvm-lib");
        assert_eq!(
            json["swiftCompiler"]["extraCLIOptions"],
            serde_json::json!(["-Xclang-linker", "-isysroot", "-Xclang-linker", "/b/iPhoneOS27.0.sdk"])
        );
    }

    #[test]
    fn native_toolset_adds_xtools_flags_and_the_fix_can_be_dropped() {
        let sdk = Utf8Path::new("/s.sdk");
        let t = Toolset::new(ToolsetKind::Native, Utf8Path::new("/bin"), sdk);
        assert_eq!(t.swift_flags[..3], ["-Xfrontend", "-enable-cross-import-overlays", "-use-ld=lld"]);
        assert_eq!(t.swift_flags.len(), 7);
        let plain = t.without_sdk_fix(sdk);
        assert_eq!(plain.swift_flags.len(), 3);
        let swb = Toolset::new(ToolsetKind::Swiftbuild, Utf8Path::new("/bin"), sdk).without_sdk_fix(sdk);
        assert!(!swb.to_json().contains("swiftCompiler"));
    }
}
