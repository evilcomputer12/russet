// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Verification after signing (SIGN §8.7): every Mach-O's code pages and CMS signature (apple-codesign's
//! verifier plus our own check of alternate code directories), the external special slots (Info.plist and
//! CodeResources), the sealed resources, and the L-SIG rules against each embedded profile.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cryptographic_message_syntax::SignedData;
use plist::Value;
use russet_core::Result;
use russet_macho::consts::{CS_EXECSEG_ALLOW_UNSIGNED, CS_EXECSEG_MAIN_BINARY, CSSLOT_INFOSLOT, CSSLOT_RESOURCEDIR};
use russet_macho::{CodeSignature, HashType, MachO};

use crate::bundle::AppBundle;
use crate::entitlements::allowed_value;
use crate::ipa::open_input;
use crate::profile::ProvisioningProfile;
use crate::util::{hex, io_error, plist_dict_from_bytes, slash_path, walk_files};
use crate::{LintFinding, LintProfile, Severity, lint};

/// Verification options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyOptions {
    /// Also runs this lint profile and reports its findings.
    pub lints: Option<LintProfile>,
    /// L-SIG-007: a device that every embedded development or ad hoc profile must list.
    pub device: Option<String>,
}

/// One verified bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBundle {
    pub rel_path: String,
    pub bundle_id: Option<String>,
    /// The main executable's signing identifier.
    pub identifier: Option<String>,
    /// The CDHash (20 bytes, hex) of the main executable's strongest code directory.
    pub cdhash: Option<String>,
    pub team_id: Option<String>,
    pub profile_uuid: Option<String>,
    /// Whether the main executable carries a signature at all.
    pub signed: bool,
}

/// What [`crate::verify`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// No finding of severity error.
    pub ok: bool,
    pub path: PathBuf,
    pub bundles: Vec<VerifiedBundle>,
    pub findings: Vec<LintFinding>,
}

pub(crate) fn verify(path: &Path, o: &VerifyOptions) -> Result<VerifyReport> {
    let input = open_input(path)?;
    let app = AppBundle::open(input.app())?;
    let mut report = verify_app(&app, o)?;
    if let Some(p) = o.lints {
        let extra = lint::lint(&app, p, input.ipa_root())?;
        for f in extra {
            if !report.findings.contains(&f) {
                report.findings.push(f);
            }
        }
        report.ok = !report.findings.iter().any(LintFinding::is_error);
    }
    report.path = path.to_path_buf();
    Ok(report)
}

/// Verifies an opened app tree.
pub(crate) fn verify_app(app: &AppBundle, o: &VerifyOptions) -> Result<VerifyReport> {
    let mut findings = Vec::new();
    let mut bundles = Vec::new();
    for b in app.all() {
        let base = |p: &Path| join_rel(&b.rel_path, &slash_path(p));
        // Every Mach-O of the bundle: signature present, code pages and CMS valid.
        for item in &b.code {
            let rel = base(item.path());
            let abs = b.root.join(item.path());
            let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
            check_macho(&data, &rel, &mut findings);
        }
        let mut record = VerifiedBundle {
            rel_path: b.rel_path.clone(),
            bundle_id: b.identifier().map(str::to_owned),
            identifier: None,
            cdhash: None,
            team_id: None,
            profile_uuid: None,
            signed: false,
        };
        let Some(exe) = b.executable_path() else {
            findings.push(LintFinding::new(
                "L-SIG-000",
                Severity::Error,
                Some(display(&b.rel_path)),
                "the bundle's CFBundleExecutable is missing",
            ));
            bundles.push(record);
            continue;
        };
        let exe_rel = base(exe.strip_prefix(&b.root).unwrap_or(&exe));
        let data = std::fs::read(&exe).map_err(|e| io_error("cannot read", &exe, e))?;
        let Ok(m) = MachO::parse(&data) else {
            findings.push(LintFinding::new(
                "L-SIG-000",
                Severity::Error,
                Some(exe_rel.clone()),
                "the main executable is not a Mach-O file",
            ));
            bundles.push(record);
            continue;
        };
        let Some(sig) = m.code_signature() else {
            bundles.push(record);
            continue;
        };
        record.signed = true;
        record.identifier = sig.identifier().map(str::to_owned);
        record.team_id = sig.team_id().map(str::to_owned);
        record.cdhash = sig.best_code_directory().map(|cd| hex(&cd.cdhash));
        if let (Some(id), Some(ident)) = (b.identifier(), sig.identifier())
            && id != ident
        {
            findings.push(LintFinding::new(
                "L-SIG-006",
                Severity::Error,
                Some(exe_rel.clone()),
                format!("the signing identifier {ident} differs from CFBundleIdentifier {id}"),
            ));
        }
        check_external_slots(b, &sig, &exe_rel, &mut findings);
        check_sealed_resources(b, &mut findings)?;
        if b.kind.needs_profile() {
            record.profile_uuid = check_app_rules(b, &m, &sig, &exe_rel, o, &mut findings)?;
        }
        bundles.push(record);
    }
    let ok = !findings.iter().any(LintFinding::is_error);
    Ok(VerifyReport { ok, path: app.root.clone(), bundles, findings })
}

fn display(rel: &str) -> String {
    if rel.is_empty() { ".".into() } else { rel.into() }
}

fn join_rel(base: &str, rel: &str) -> String {
    if base.is_empty() { rel.into() } else { format!("{base}/{rel}") }
}

/// Signature presence, apple-codesign's verifier (primary code directory, CMS, embedded slots) and the page
/// hashes of every alternate code directory of every slice.
fn check_macho(data: &[u8], rel: &str, findings: &mut Vec<LintFinding>) {
    let m = match MachO::parse(data) {
        Ok(m) => m,
        Err(e) => {
            findings.push(LintFinding::new(
                "L-SIG-006",
                Severity::Error,
                Some(rel.into()),
                format!("cannot parse: {e}"),
            ));
            return;
        }
    };
    for (i, img) in m.images().iter().enumerate() {
        if let Some(err) = &img.code_signature_error {
            findings.push(LintFinding::new(
                "L-SIG-006",
                Severity::Error,
                Some(rel.into()),
                format!("malformed code signature ({}): {err}", img.cpu),
            ));
            continue;
        }
        let Some(sig) = &img.code_signature else {
            findings.push(LintFinding::new(
                "L-SIG-000",
                Severity::Error,
                Some(rel.into()),
                format!("the {} slice is not signed", img.cpu),
            ));
            continue;
        };
        if let Some(slice) = m.slice_bytes(data, i) {
            for cd in &sig.code_directories {
                let bad = cd.mismatched_pages(slice);
                if !bad.is_empty() {
                    findings.push(LintFinding::new(
                        "L-SIG-006",
                        Severity::Error,
                        Some(rel.into()),
                        format!(
                            "{} code page hash mismatch at page {} ({} directory)",
                            img.cpu,
                            bad[0],
                            cd.hash_type.name()
                        ),
                    ));
                }
            }
        }
    }
    for p in apple_codesign::verify_macho_data(data) {
        use apple_codesign::VerificationProblemType as T;
        let (code, severity) = match &p.problem {
            T::NoCryptographicSignature => ("L-SIG-000", Severity::Error),
            T::CmsOldDigestAlgorithm(_) | T::CmsOldSignatureAlgorithm(_) => ("L-SIG-006", Severity::Warning),
            // Reported above with the slice; apple-codesign repeats it.
            T::NoMachOSignatureData => continue,
            _ => ("L-SIG-006", Severity::Error),
        };
        let msg = p.to_string();
        let f = LintFinding::new(code, severity, Some(rel.into()), msg);
        if !findings.contains(&f) {
            findings.push(f);
        }
    }
}

/// The Info.plist (slot 1) and CodeResources (slot 3) hashes of every code directory.
fn check_external_slots(b: &AppBundle, sig: &CodeSignature, exe_rel: &str, findings: &mut Vec<LintFinding>) {
    let info = std::fs::read(b.root.join("Info.plist")).ok();
    let resources = std::fs::read(b.root.join("_CodeSignature/CodeResources")).ok();
    for cd in &sig.code_directories {
        for (slot, name, content) in
            [(CSSLOT_INFOSLOT, "Info.plist", &info), (CSSLOT_RESOURCEDIR, "_CodeSignature/CodeResources", &resources)]
        {
            let stored = cd.special_slot(slot);
            match (stored, content) {
                (Some(h), Some(c)) => {
                    if cd.hash_type.slot_digest(c).as_deref() != Some(h) {
                        findings.push(LintFinding::new(
                            "L-SIG-006",
                            Severity::Error,
                            Some(exe_rel.into()),
                            format!(
                                "the sealed hash of {name} does not match the file ({} directory)",
                                cd.hash_type.name()
                            ),
                        ));
                    }
                }
                (Some(_), None) => findings.push(LintFinding::new(
                    "L-SIG-006",
                    Severity::Error,
                    Some(join_rel(&b.rel_path, name)),
                    format!("{name} is sealed by the signature but missing"),
                )),
                (None, Some(_)) => findings.push(LintFinding::new(
                    "L-SIG-006",
                    Severity::Error,
                    Some(exe_rel.into()),
                    format!("{name} exists but the signature does not seal it"),
                )),
                (None, None) => {}
            }
        }
    }
}

/// Every `files2` entry of `_CodeSignature/CodeResources` matches the bundle, and every file is sealed.
fn check_sealed_resources(b: &AppBundle, findings: &mut Vec<LintFinding>) -> Result<()> {
    let path = b.root.join("_CodeSignature/CodeResources");
    let Ok(raw) = std::fs::read(&path) else {
        findings.push(LintFinding::new(
            "L-SIG-006",
            Severity::Error,
            Some(join_rel(&b.rel_path, "_CodeSignature/CodeResources")),
            "the bundle has no sealed resources (_CodeSignature/CodeResources)",
        ));
        return Ok(());
    };
    let dict = match plist_dict_from_bytes(&raw, "CodeResources") {
        Ok(d) => d,
        Err(e) => {
            findings.push(LintFinding::new(
                "L-SIG-006",
                Severity::Error,
                Some(join_rel(&b.rel_path, "_CodeSignature/CodeResources")),
                e.message().to_owned(),
            ));
            return Ok(());
        }
    };
    let files2 = dict.get("files2").and_then(Value::as_dictionary).cloned().unwrap_or_default();
    let mut sealed: BTreeSet<String> = BTreeSet::new();
    for (name, entry) in &files2 {
        sealed.insert(name.clone());
        let abs = b.root.join(name);
        let rel = join_rel(&b.rel_path, name);
        let fail = |msg: String| LintFinding::new("L-SIG-006", Severity::Error, Some(rel.clone()), msg);
        let entry_dict = entry.as_dictionary();
        let optional = entry_dict.and_then(|d| d.get("optional")).and_then(Value::as_boolean).unwrap_or(false);
        if abs.symlink_metadata().is_err() {
            if !optional {
                findings.push(fail("a sealed resource is missing".into()));
            }
            continue;
        }
        if let Some(d) = entry_dict {
            if let Some(target) = d.get("symlink").and_then(Value::as_string) {
                let actual = std::fs::read_link(&abs).ok();
                if actual.as_deref() != Some(Path::new(target)) {
                    findings.push(fail("the sealed symlink target changed".into()));
                }
                continue;
            }
            if let Some(cdhash) = d.get("cdhash").and_then(Value::as_data) {
                if !nested_cdhash_matches(&abs, cdhash) {
                    findings.push(fail("the nested code's CDHash differs from the sealed one".into()));
                }
                continue;
            }
            if let Some(h2) = d.get("hash2").and_then(Value::as_data) {
                let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
                if HashType::Sha256.digest(&data).as_deref() != Some(h2) {
                    findings.push(fail("the sealed SHA-256 does not match the file".into()));
                }
                continue;
            }
        }
        // Rules-v1 style entries: raw SHA-1 data, or {hash}.
        let h1 = entry.as_data().or_else(|| entry_dict.and_then(|d| d.get("hash")).and_then(Value::as_data));
        if let Some(h1) = h1 {
            let data = std::fs::read(&abs).map_err(|e| io_error("cannot read", &abs, e))?;
            if HashType::Sha1.digest(&data).as_deref() != Some(h1) {
                findings.push(fail("the sealed SHA-1 does not match the file".into()));
            }
        }
    }

    // Coverage: every file outside nested bundles, the signature directory and the omitted names is sealed.
    let nested: Vec<PathBuf> = b.nested.iter().map(|n| n.root.clone()).collect();
    let exe = b.executable_name().map(str::to_owned);
    for rel in walk_files(&b.root)? {
        let abs = b.root.join(&rel);
        let name = slash_path(&rel);
        if nested.iter().any(|n| abs.starts_with(n))
            || name.starts_with("_CodeSignature/")
            || exe.as_deref() == Some(name.as_str())
            || name == "Info.plist"
            || name == "PkgInfo"
            || name.ends_with(".DS_Store")
            || (name.contains(".lproj/") && name.ends_with("/locversion.plist"))
        {
            continue;
        }
        let covered = sealed.contains(&name) || sealed.iter().any(|s| name.starts_with(&format!("{s}/")));
        if !covered {
            findings.push(LintFinding::new(
                "L-SIG-006",
                Severity::Error,
                Some(join_rel(&b.rel_path, &name)),
                "the file is not sealed by the bundle's signature (added after signing?)",
            ));
        }
    }
    Ok(())
}

/// Whether the code at `path` (a nested bundle's main executable, or a Mach-O file) has a code directory whose
/// SHA-256-truncated or own-hash CDHash equals `cdhash`.
fn nested_cdhash_matches(path: &Path, cdhash: &[u8]) -> bool {
    let exe = if path.is_dir() {
        let info = path.join("Info.plist");
        let Ok(v) = plist::Value::from_file(&info) else { return false };
        let Some(name) = v.as_dictionary().and_then(|d| d.get("CFBundleExecutable")).and_then(Value::as_string) else {
            return false;
        };
        path.join(name)
    } else {
        path.to_path_buf()
    };
    let Ok(m) = MachO::open(&exe) else { return false };
    m.images()
        .iter()
        .filter_map(|i| i.code_signature.as_ref())
        .flat_map(|s| s.code_directories.iter())
        .any(|cd| cd.cdhash == cdhash || HashType::Sha256Truncated.slot_digest(cd.raw()).as_deref() == Some(cdhash))
}

/// L-SIG-001 … L-SIG-005 and L-SIG-007 on an app-like bundle; returns the embedded profile's UUID.
fn check_app_rules(
    b: &AppBundle,
    m: &MachO,
    sig: &CodeSignature,
    exe_rel: &str,
    o: &VerifyOptions,
    findings: &mut Vec<LintFinding>,
) -> Result<Option<String>> {
    let at = Some(exe_rel.to_owned());
    let cd = sig.code_directory();
    if m.filetype() == russet_macho::consts::MH_EXECUTE
        && !cd.is_some_and(|c| c.has_exec_seg_flag(CS_EXECSEG_MAIN_BINARY))
    {
        findings.push(LintFinding::new(
            "L-SIG-001",
            Severity::Error,
            at.clone(),
            "the main executable lacks CS_EXECSEG_MAIN_BINARY (iOS 26+ kills such extensions)",
        ));
    }
    let entitlements = match sig.entitlements_xml.as_deref() {
        Some(xml) => match plist_dict_from_bytes(xml.as_bytes(), "the signed entitlements") {
            Ok(d) => d,
            Err(e) => {
                findings.push(LintFinding::new("L-SIG-004", Severity::Error, at.clone(), e.message().to_owned()));
                plist::Dictionary::new()
            }
        },
        None => plist::Dictionary::new(),
    };
    let gta = entitlements.get("get-task-allow").and_then(Value::as_boolean).unwrap_or(false);
    let allow_unsigned = cd.is_some_and(|c| c.has_exec_seg_flag(CS_EXECSEG_ALLOW_UNSIGNED));
    if gta != allow_unsigned {
        findings.push(LintFinding::new(
            "L-SIG-002",
            Severity::Error,
            at.clone(),
            format!("CS_EXECSEG_ALLOW_UNSIGNED is {allow_unsigned} but get-task-allow is {gta}"),
        ));
    }
    if !entitlements.is_empty() && sig.entitlements_der.is_none() {
        findings.push(LintFinding::new(
            "L-SIG-005",
            Severity::Error,
            at.clone(),
            "entitlements are signed without the DER entitlements blob (required since iOS 15)",
        ));
    }
    let profile_path = b.root.join("embedded.mobileprovision");
    let profile = match std::fs::read(&profile_path) {
        Ok(raw) => match ProvisioningProfile::parse(&raw) {
            Ok(p) => p,
            Err(e) => {
                findings.push(LintFinding::new(
                    "L-SIG-004",
                    Severity::Error,
                    Some(join_rel(&b.rel_path, "embedded.mobileprovision")),
                    e.message().to_owned(),
                ));
                return Ok(None);
            }
        },
        Err(_) => {
            findings.push(LintFinding::new(
                "L-SIG-004",
                Severity::Error,
                Some(display(&b.rel_path)),
                "no embedded.mobileprovision: a device will refuse to install it",
            ));
            return Ok(None);
        }
    };
    // L-SIG-003: one team everywhere.
    let signer_cert = cms_signer_certificate(sig);
    let cert_team =
        signer_cert.as_ref().and_then(|der| crate::identity::certificate_info(der).ok()).and_then(|i| i.team_id);
    let cd_team = sig.team_id();
    if cd_team != Some(profile.team_id.as_str()) || cert_team.as_deref().is_some_and(|t| t != profile.team_id) {
        findings.push(LintFinding::new(
            "L-SIG-003",
            Severity::Error,
            at.clone(),
            "the code directory's team, the profile's team and the certificate's team differ",
        ));
    }
    // L-SIG-004: entitlements within the profile; the signing certificate listed by it.
    let profile_ents = &profile.entitlements;
    for (k, v) in &entitlements {
        let ok = match profile_ents.get(k) {
            Some(allowed) => allowed_value(v, allowed),
            None => false,
        };
        if !ok {
            findings.push(LintFinding::new(
                "L-SIG-004",
                Severity::Error,
                at.clone(),
                format!("the signed entitlement {k} is not granted by the embedded profile"),
            ));
        }
    }
    match &signer_cert {
        Some(der) if profile.has_certificate(der) => {}
        Some(_) => findings.push(LintFinding::new(
            "L-SIG-004",
            Severity::Error,
            at.clone(),
            "the signing certificate is not one of the profile's DeveloperCertificates",
        )),
        None => {}
    }
    // L-SIG-007: validity and the device.
    if profile.expires_within(Duration::ZERO) {
        findings.push(LintFinding::new("L-SIG-007", Severity::Error, at.clone(), "the embedded profile has expired"));
    } else if profile.expires_within(Duration::from_secs(24 * 3600)) {
        findings.push(LintFinding::new(
            "L-SIG-007",
            Severity::Warning,
            at.clone(),
            "the embedded profile expires within 24 hours",
        ));
    }
    if let Some(udid) = &o.device
        && !profile.has_device(udid)
    {
        findings.push(LintFinding::new(
            "L-SIG-007",
            Severity::Error,
            at,
            "the device is not in the embedded profile's ProvisionedDevices",
        ));
    }
    Ok(Some(profile.uuid))
}

/// The DER certificate that produced the CMS signature, matched by issuer and serial number.
pub(crate) fn cms_signer_certificate(sig: &CodeSignature) -> Option<Vec<u8>> {
    let cms = sig.cms.as_deref().filter(|c| !c.is_empty())?;
    let signed = SignedData::parse_ber(cms).ok()?;
    let signer = signed.signers().next()?;
    let (issuer, serial) = signer.certificate_issuer_and_serial()?;
    signed
        .certificates()
        .find(|c| c.issuer_name() == issuer && c.serial_number_asn1() == serial)
        .map(|c| c.constructed_data().to_vec())
}
