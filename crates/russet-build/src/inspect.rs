// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `build.inspect` / `russet inspect` (build §9.3): an `.app` or `.ipa`'s Info.plist, its DT* keys, a summary of
//! every Mach-O file, and the structural lints that need no SDK (L001 platform, L004 slices, L006 host rpaths,
//! L009 encryption info). The full lint catalog is W16's (B05, B18).

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::bundle::MachOEntry;
use crate::macho::{MachOTool, PLATFORM_IOS, is_host_rpath};
use crate::stack::swiftpm::io;
use crate::{LintFinding, LintSeverity, infoplist, metadata};

/// What `build.inspect` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectReport {
    /// The bundle's path inside the input (`Payload/X.app` for an IPA, the directory name for an app).
    pub bundle: String,
    pub info_plist: Dictionary,
    pub mach_o: Vec<MachOEntry>,
    pub lints: Vec<LintFinding>,
    pub dt: BTreeMap<String, String>,
}

impl InspectReport {
    /// The Info.plist in its JSON form.
    pub fn info_plist_json(&self) -> serde_json::Value {
        infoplist::to_json(&Value::Dictionary(self.info_plist.clone()))
    }
}

fn read_info(bytes: &[u8], what: &str) -> Result<Dictionary> {
    match russet_plist::from_bytes(bytes) {
        Ok((Value::Dictionary(d), _)) => Ok(d),
        Ok(_) => Err(Error::new(codes::core::INVALID_ARGUMENT, format!("{what} is not a dictionary"))),
        Err(e) => Err(Error::new(codes::core::INVALID_ARGUMENT, format!("{what}: {e}"))),
    }
}

/// Inspects an `.app` directory or an `.ipa` file.
pub fn inspect(path: &Utf8Path, macho: &dyn MachOTool) -> Result<InspectReport> {
    if !path.exists() {
        return Err(Error::new(codes::core::NOT_FOUND, format!("{path} does not exist"))
            .with_hint("Pass the path of an .app or .ipa."));
    }
    let (bundle, info, files) = if path.is_dir() {
        let info = read_info(
            &std::fs::read(path.join("Info.plist")).map_err(|e| io(&path.join("Info.plist"), e))?,
            "Info.plist",
        )?;
        let mut files = Vec::new();
        collect(path, path, &mut files)?;
        (path.file_name().unwrap_or_default().to_owned(), info, files)
    } else if path.extension() == Some("ipa") {
        let listing = russet_ipa::read_listing(path).map_err(ipa_error)?;
        let app = russet_ipa::app_dir(&listing)
            .ok_or_else(|| Error::new(codes::core::INVALID_ARGUMENT, format!("{path} has no single Payload/*.app")))?;
        let info =
            read_info(&russet_ipa::read_entry(path, &format!("{app}Info.plist")).map_err(ipa_error)?, "Info.plist")?;
        let mut files = Vec::new();
        for e in listing.iter().filter(|e| !e.is_dir() && e.symlink_target.is_none() && e.name.starts_with(&app)) {
            let bytes = russet_ipa::read_entry(path, &e.name).map_err(ipa_error)?;
            files.push((e.name[app.len()..].to_owned(), bytes));
        }
        (app.trim_end_matches('/').to_owned(), info, files)
    } else {
        return Err(Error::new(
            codes::core::INVALID_ARGUMENT,
            format!("{path} is neither an .app directory nor an .ipa"),
        ));
    };

    let executable = info.get("CFBundleExecutable").and_then(Value::as_string).unwrap_or_default().to_owned();
    let mut mach_o = Vec::new();
    let mut lints = Vec::new();
    for (rel, bytes) in files {
        if !russet_ipa::is_macho(&bytes[..bytes.len().min(8)]) {
            continue;
        }
        let summary = macho.summary(&bytes)?;
        let display = format!("{bundle}/{rel}");
        let finding = |code: &str, severity: LintSeverity, message: String| LintFinding {
            code: code.into(),
            severity,
            path: Some(Utf8PathBuf::from(&display)),
            message,
            fixed: false,
        };
        if summary.platform.is_some_and(|p| p != PLATFORM_IOS) {
            lints.push(finding(
                "L001",
                LintSeverity::Error,
                format!("LC_BUILD_VERSION platform is {:?}, not iOS (2)", summary.platform),
            ));
        }
        if summary.archs.iter().any(|a| a != "arm64") {
            lints.push(finding(
                "L004",
                LintSeverity::Warning,
                format!("slices {} include more than arm64", summary.archs.join(",")),
            ));
        }
        for r in summary.rpaths.iter().filter(|r| is_host_rpath(r)) {
            lints.push(finding("L006", LintSeverity::Warning, format!("host rpath {r}")));
        }
        if rel == executable && summary.encrypted.is_none() {
            lints.push(finding("L009", LintSeverity::Info, "the app executable has no LC_ENCRYPTION_INFO_64".into()));
        }
        mach_o.push(MachOEntry { path: display, summary, built_by_russet: false });
    }
    let dt = metadata::as_strings(
        &info.iter().filter(|(k, _)| metadata::is_metadata_key(k)).map(|(k, v)| (k.clone(), v.clone())).collect(),
    );
    Ok(InspectReport { bundle, info_plist: info, mach_o, lints, dt })
}

fn collect(root: &Utf8Path, dir: &Utf8Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    let mut entries: Vec<_> = dir.read_dir_utf8().map_err(|e| io(dir, e))?.filter_map(|e| e.ok()).collect();
    entries.sort_by(|a, b| a.file_name().cmp(b.file_name()));
    for e in entries {
        let kind = e.file_type().map_err(|err| io(e.path(), err))?;
        if kind.is_dir() {
            collect(root, e.path(), out)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(e.path()).map_err(|err| io(e.path(), err))?;
            let rel = e.path().strip_prefix(root).map_or_else(|_| e.path().to_string(), |p| p.to_string());
            out.push((rel, bytes));
        }
    }
    Ok(())
}

fn ipa_error(e: russet_ipa::IpaError) -> Error {
    Error::new(codes::core::INVALID_ARGUMENT, format!("IPA: {e}")).caused_by(e)
}
