// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet sign inspect`: one [`SignatureInfo`] per Mach-O slice of an `.app`, `.ipa` or single file. Team IDs
//! are masked and no certificate subject (it names a person) is reported (AGENTS.md §8).

use std::path::Path;

use russet_core::Result;
use russet_macho::consts::*;
use russet_macho::{CodeSignature, MachO};

use crate::bundle::AppBundle;
use crate::ipa::open_input;
use crate::util::{hex, io_error, mask_team, plist_dict_from_bytes, slash_path};

/// One code directory of a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDirectoryInfo {
    /// `0x20400`.
    pub version: String,
    /// `sha1`, `sha256`, …
    pub hash_type: String,
    pub page_size: u32,
    /// `adhoc`, `linker-signed`, `runtime`, …
    pub flags: Vec<String>,
    /// `main-binary`, `allow-unsigned`, …
    pub exec_seg_flags: Vec<String>,
    pub exec_seg_flags_raw: Option<u64>,
    /// The CDHash (20 bytes, hex).
    pub cdhash: String,
}

/// One signed (or unsigned) Mach-O slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    /// The file, relative to the inspected bundle (or its file name).
    pub path: String,
    /// `CFBundleIdentifier` when the file is a bundle's main executable.
    pub bundle_id: Option<String>,
    pub arch: String,
    /// `execute`, `dylib`, `bundle`, …
    pub filetype: String,
    pub signed: bool,
    pub identifier: Option<String>,
    /// The team ID, masked (`*******XYZ`).
    pub team_id: Option<String>,
    pub code_directories: Vec<CodeDirectoryInfo>,
    /// Signed entitlement keys (values can embed the team ID, so they are not reported).
    pub entitlement_keys: Vec<String>,
    pub get_task_allow: Option<bool>,
    pub has_der_entitlements: bool,
    pub has_cms: bool,
    pub platform: Option<u32>,
    pub minos: Option<String>,
    pub sdk: Option<String>,
    /// `L-MACHO-001`: whether the string pool is 8-byte aligned.
    pub string_pool_aligned: bool,
}

pub(crate) fn inspect(path: &Path) -> Result<Vec<SignatureInfo>> {
    if path.is_file() && russet_macho::is_macho_file(path).unwrap_or(false) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let data = std::fs::read(path).map_err(|e| io_error("cannot read", path, e))?;
        return Ok(describe(&data, &name, None));
    }
    let input = open_input(path)?;
    let app = AppBundle::open(input.app())?;
    let mut out = Vec::new();
    for b in app.all().into_iter().rev() {
        for item in &b.code {
            let rel = slash_path(item.path());
            let full = if b.rel_path.is_empty() { rel.clone() } else { format!("{}/{rel}", b.rel_path) };
            let abs = b.root.join(item.path());
            let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
            let is_exe = b.executable_name() == Some(rel.as_str());
            out.extend(describe(&data, &full, if is_exe { b.identifier() } else { None }));
        }
    }
    Ok(out)
}

fn describe(data: &[u8], path: &str, bundle_id: Option<&str>) -> Vec<SignatureInfo> {
    let Ok(m) = MachO::parse(data) else { return Vec::new() };
    m.images()
        .iter()
        .map(|img| {
            let sig = img.code_signature.as_ref();
            let (entitlement_keys, get_task_allow) = entitlement_summary(sig);
            SignatureInfo {
                path: path.to_owned(),
                bundle_id: bundle_id.map(str::to_owned),
                arch: img.arch.name(),
                filetype: filetype_name(img.filetype).to_owned(),
                signed: sig.is_some(),
                identifier: sig.and_then(CodeSignature::identifier).map(str::to_owned),
                team_id: sig.and_then(CodeSignature::team_id).map(mask_team),
                code_directories: sig.map(code_directories).unwrap_or_default(),
                entitlement_keys,
                get_task_allow,
                has_der_entitlements: sig.is_some_and(|s| s.entitlements_der.is_some()),
                has_cms: sig.is_some_and(CodeSignature::has_cms_signature),
                platform: img.build_version.as_ref().map(|b| b.platform),
                minos: img.build_version.as_ref().map(|b| b.minos.to_string()),
                sdk: img.build_version.as_ref().map(|b| b.sdk.to_string()),
                string_pool_aligned: img.string_pool_alignment() >= 8,
            }
        })
        .collect()
}

fn entitlement_summary(sig: Option<&CodeSignature>) -> (Vec<String>, Option<bool>) {
    let Some(xml) = sig.and_then(|s| s.entitlements_xml.as_deref()) else { return (Vec::new(), None) };
    match plist_dict_from_bytes(xml.as_bytes(), "entitlements") {
        Ok(d) => {
            let gta = d.get("get-task-allow").and_then(plist::Value::as_boolean);
            (d.keys().cloned().collect(), gta)
        }
        Err(_) => (Vec::new(), None),
    }
}

fn code_directories(sig: &CodeSignature) -> Vec<CodeDirectoryInfo> {
    sig.code_directories
        .iter()
        .map(|cd| {
            let mut flags = Vec::new();
            for (bit, name) in [
                (CS_ADHOC, "adhoc"),
                (CS_HARD, "hard"),
                (CS_KILL, "kill"),
                (CS_RESTRICT, "restrict"),
                (CS_ENFORCEMENT, "enforcement"),
                (CS_REQUIRE_LV, "library-validation"),
                (CS_RUNTIME, "runtime"),
                (CS_LINKER_SIGNED, "linker-signed"),
            ] {
                if cd.flags & bit != 0 {
                    flags.push(name.to_owned());
                }
            }
            let mut exec = Vec::new();
            if let Some(f) = cd.exec_seg_flags {
                for (bit, name) in [
                    (CS_EXECSEG_MAIN_BINARY, "main-binary"),
                    (CS_EXECSEG_ALLOW_UNSIGNED, "allow-unsigned"),
                    (CS_EXECSEG_DEBUGGER, "debugger"),
                    (CS_EXECSEG_JIT, "jit"),
                    (CS_EXECSEG_SKIP_LV, "skip-library-validation"),
                    (CS_EXECSEG_CAN_LOAD_CDHASH, "can-load-cdhash"),
                    (CS_EXECSEG_CAN_EXEC_CDHASH, "can-exec-cdhash"),
                ] {
                    if f & bit != 0 {
                        exec.push(name.to_owned());
                    }
                }
            }
            CodeDirectoryInfo {
                version: format!("{:#x}", cd.version),
                hash_type: cd.hash_type.name(),
                page_size: cd.page_size,
                flags,
                exec_seg_flags: exec,
                exec_seg_flags_raw: cd.exec_seg_flags,
                cdhash: hex(&cd.cdhash),
            }
        })
        .collect()
}

fn filetype_name(t: u32) -> &'static str {
    match t {
        MH_OBJECT => "object",
        MH_EXECUTE => "execute",
        MH_DYLIB => "dylib",
        MH_DYLINKER => "dylinker",
        MH_BUNDLE => "bundle",
        MH_DSYM => "dsym",
        _ => "other",
    }
}
