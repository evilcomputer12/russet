// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Signing through apple-codesign 0.29.0 (SIGN §8.2, §8.7): one `SigningSettings` for the whole tree, with the
//! signing key and team global and the entitlements scoped per bundle path, then `BundleSigner`, which signs
//! nested code first and seals each level in `_CodeSignature/CodeResources`.

use std::path::Path;

use apple_codesign::{BundleSigner, SettingsScope, SigningSettings};
use plist::{Dictionary, Value};
use russet_core::Result;
use russet_core::error::{Error, codes};
use x509_certificate::CapturedX509Certificate;

use crate::bundle::AppBundle;
use crate::entitlements::{Distribution, EntitlementsResolver};
use crate::profile::{ProfileKind, ProvisioningProfile};
use crate::util::{bundle_invalid, codesign_failed, io_error, plist_xml, read_plist_dict};
use crate::{LintFinding, SignOutput, SignReport, SignRequest, SignedBundle, fixes, lint, verify};

pub(crate) fn sign(r: SignRequest<'_>) -> Result<SignReport> {
    let opts = &r.options;
    let root = match &opts.output {
        SignOutput::InPlace => r.bundle.to_path_buf(),
        SignOutput::CopyTo(dest) => {
            copy_bundle(r.bundle, dest)?;
            dest.clone()
        }
    };
    let fixes_applied = fixes::apply(&root, opts.fixes)?;
    for (rel, input) in &r.per_bundle {
        if let Some(id) = &input.bundle_id_override {
            rewrite_bundle_id(&root.join(rel), id)?;
        }
    }
    let app = AppBundle::open(&root)?;
    for rel in r.per_bundle.keys() {
        if app.find(rel).is_none() {
            return Err(bundle_invalid(&root, format!("it has no nested bundle {rel}")));
        }
    }

    let cert = CapturedX509Certificate::from_der(r.identity.certificate_der().to_vec()).map_err(|e| {
        Error::new(codes::sign::NO_IDENTITY, format!("the signing certificate cannot be parsed: {e}"))
            .with_hint("Import the identity again (`russet account import --from-xtool`).")
    })?;
    let team = r.identity.team_id();
    let mut settings = SigningSettings::default();
    settings.set_signing_key(r.identity.signer(), cert);
    // Apple-issued certificates get their intermediate and root chained; synthetic test PKI does not.
    let _ = settings.chain_apple_certificates();
    settings.set_team_id(team);

    let mut planned = Vec::new();
    for b in app.all() {
        if !b.kind.needs_profile() {
            continue;
        }
        let bundle_id = b
            .identifier()
            .ok_or_else(|| bundle_invalid(&b.root, "its Info.plist has no CFBundleIdentifier"))?
            .to_owned();
        if b.executable_path().is_none() {
            return Err(bundle_invalid(&b.root, "its CFBundleExecutable is missing"));
        }
        let input = r.per_bundle.get(&b.rel_path);
        let profile_path = input.and_then(|i| i.profile.as_deref()).ok_or_else(|| {
            Error::new(
                codes::sign::PROFILE_MISMATCH,
                format!("no provisioning profile was given for {}", display_rel(&b.rel_path)),
            )
            .with_hint("Run `russet provision` (it caches the profile), or pass `--profile <bundle-rel>=<file>`.")
            .with_details(serde_json::json!({"bundle": b.rel_path, "bundleId": bundle_id}))
        })?;
        let profile = ProvisioningProfile::open(profile_path)?;
        check_profile(&profile, &bundle_id, team, r.identity.certificate_der(), opts.distribution, &b.rel_path)?;

        let mut requested = match input.and_then(|i| i.entitlements.as_deref()) {
            Some(p) => read_plist_dict(p)?,
            None => Dictionary::new(),
        };
        if let Some(gta) = input.and_then(|i| i.get_task_allow)
            && !requested.contains_key("get-task-allow")
        {
            requested.insert("get-task-allow".into(), Value::Boolean(gta));
        }
        let resolver = EntitlementsResolver {
            profile: &profile,
            bundle_id: &bundle_id,
            distribution: opts.distribution,
            policy: opts.merge_policy,
        };
        let resolved = resolver.resolve(&requested)?;
        let xml = plist_xml(&resolved.dict)?;
        let embedded = b.root.join("embedded.mobileprovision");
        std::fs::write(&embedded, &profile.raw).map_err(|e| io_error("cannot write", &embedded, e))?;
        let scope = if b.rel_path.is_empty() { SettingsScope::Main } else { SettingsScope::Path(b.rel_path.clone()) };
        settings
            .set_entitlements_xml(scope, &xml)
            .map_err(|e| codesign_failed(format!("entitlements of {}", display_rel(&b.rel_path)), e))?;
        planned.push((b.rel_path.clone(), bundle_id, b.kind, profile.uuid.clone(), xml, resolved.notes));
    }

    let mut signer = BundleSigner::new_from_path(&root).map_err(|e| codesign_failed("opening the bundle", e))?;
    signer.collect_nested_bundles().map_err(|e| codesign_failed("collecting nested bundles", e))?;
    signer
        .write_signed_bundle(&root, &settings)
        .map_err(|e| codesign_failed(format!("signing {}", root.display()), e))?;

    // Post-sign verification always runs: it is cheap, and it catches a signature the device would reject.
    let report = verify::verify_app(&AppBundle::open(&root)?, &verify::VerifyOptions::default())?;
    if !report.ok {
        let errors: Vec<&LintFinding> = report.findings.iter().filter(|f| f.is_error()).collect();
        return Err(Error::new(
            codes::sign::VERIFY_FAILED,
            format!(
                "the signed app does not verify: {}",
                errors.first().map_or("unknown problem", |f| f.message.as_str())
            ),
        )
        .with_hint("Run `russet sign verify` for every finding; check the pre-sign fixes (`--no-fix` disables one).")
        .with_details(findings_json(&errors)));
    }
    let lints = match opts.lint {
        Some(p) => lint::lint(&AppBundle::open(&root)?, p, None)?,
        None => Vec::new(),
    };
    let lint_errors: Vec<&LintFinding> = lints.iter().filter(|f| f.is_error()).collect();
    if !lint_errors.is_empty() {
        return Err(Error::new(
            codes::sign::LINT_FAILED,
            format!("{} signing lint(s) failed: {}", lint_errors.len(), lint_errors[0].message),
        )
        .with_hint("Fix the findings listed in details (`russet sign lint` shows them all).")
        .with_details(findings_json(&lint_errors)));
    }

    let mut bundles = Vec::new();
    for (rel_path, bundle_id, kind, profile_uuid, entitlements_xml, notes) in planned {
        let cdhash = report.bundles.iter().find(|v| v.rel_path == rel_path).and_then(|v| v.cdhash.clone());
        bundles.push(SignedBundle {
            rel_path,
            bundle_id,
            kind,
            cdhash: cdhash.unwrap_or_default(),
            team_id: team.to_owned(),
            profile_uuid,
            entitlements_xml,
            notes,
        });
    }
    Ok(SignReport { output: root, bundles, fixes_applied, lints })
}

fn display_rel(rel: &str) -> &str {
    if rel.is_empty() { "the app" } else { rel }
}

fn findings_json(f: &[&LintFinding]) -> serde_json::Value {
    serde_json::json!({
        "findings": f.iter().map(|x| serde_json::json!({
            "code": x.code, "severity": x.severity.as_str(), "message": x.message, "path": x.path
        })).collect::<Vec<_>>()
    })
}

/// The profile must cover the bundle and team, hold the signing certificate, fit the distribution and be valid.
fn check_profile(
    profile: &ProvisioningProfile,
    bundle_id: &str,
    team: &str,
    cert_der: &[u8],
    distribution: Distribution,
    rel: &str,
) -> Result<()> {
    let mismatch = |why: String, hint: &str| {
        Error::new(
            codes::sign::PROFILE_MISMATCH,
            format!("profile {} cannot sign {}: {why}", profile.uuid, display_rel(rel)),
        )
        .with_hint(hint.to_owned())
        .with_details(serde_json::json!({"profile": profile.uuid, "bundle": rel, "bundleId": bundle_id}))
    };
    if profile.team_id != team {
        return Err(mismatch(
            "it belongs to another team than the signing identity".into(),
            "Use a profile of the identity's team, or another identity.",
        ));
    }
    if !profile.covers(bundle_id, team) {
        return Err(mismatch(
            format!("its App ID {} does not cover {bundle_id}", profile.application_identifier),
            "Use a profile for this bundle ID (`russet provision`), or check [app] bundle-id and the bundle-id policy.",
        ));
    }
    if !profile.has_certificate(cert_der) {
        return Err(mismatch(
            "it does not include the signing certificate".into(),
            "Recreate the profile with this certificate (`russet provision`), or sign with a certificate it lists.",
        ));
    }
    if profile.expires_within(std::time::Duration::ZERO) {
        return Err(mismatch("it has expired".into(), "Recreate the profile (`russet provision`)."));
    }
    let expected = match distribution {
        Distribution::Development => ProfileKind::Development,
        Distribution::AdHoc => ProfileKind::AdHoc,
        Distribution::AppStore => ProfileKind::AppStore,
    };
    if profile.kind != expected {
        return Err(mismatch(
            format!("it is a {} profile, not {}", profile.kind.as_str(), distribution.as_str()),
            "Pick the profile kind that matches --distribution.",
        ));
    }
    Ok(())
}

/// Rewrites `CFBundleIdentifier` in `bundle/Info.plist`, keeping its format (binary or XML).
fn rewrite_bundle_id(bundle: &Path, id: &str) -> Result<()> {
    let path = bundle.join("Info.plist");
    let raw = std::fs::read(&path).map_err(|e| io_error("cannot read", &path, e))?;
    let mut dict = read_plist_dict(&path)?;
    dict.insert("CFBundleIdentifier".into(), Value::String(id.into()));
    let value = Value::Dictionary(dict);
    let res = if raw.starts_with(b"bplist00") { value.to_file_binary(&path) } else { value.to_file_xml(&path) };
    res.map_err(|e| Error::new(codes::core::IO, format!("cannot write {}: {e}", path.display())).caused_by(e))
}

/// Copies a bundle tree, keeping symlinks and permissions. An existing destination is replaced only when it is
/// itself a bundle (a previous output); anything else is refused.
pub(crate) fn copy_bundle(src: &Path, dest: &Path) -> Result<()> {
    let src_c = src.canonicalize().map_err(|e| io_error("cannot resolve", src, e))?;
    if let Ok(dest_c) = dest.canonicalize() {
        if dest_c == src_c || src_c.starts_with(&dest_c) {
            return Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                format!("the output {} overlaps the input {}", dest.display(), src.display()),
            )
            .with_hint("Choose an output path outside the bundle being signed."));
        }
        if !dest_c.join("Info.plist").is_file() {
            return Err(Error::new(
                codes::core::INVALID_ARGUMENT,
                format!("refusing to replace {}: it is not a previously signed bundle", dest.display()),
            )
            .with_hint("Remove it yourself, or choose another output path."));
        }
        std::fs::remove_dir_all(dest).map_err(|e| io_error("cannot remove", dest, e))?;
    }
    copy_tree(src, dest)
}

fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(|e| io_error("cannot create", dest, e))?;
    let perms = std::fs::metadata(src).map_err(|e| io_error("cannot stat", src, e))?.permissions();
    for entry in std::fs::read_dir(src).map_err(|e| io_error("cannot read", src, e))? {
        let entry = entry.map_err(|e| io_error("cannot read", src, e))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| io_error("cannot stat", &from, e))?;
        if ft.is_symlink() {
            let target = std::fs::read_link(&from).map_err(|e| io_error("cannot read the link", &from, e))?;
            symlink(&target, &to)?;
        } else if ft.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| io_error("cannot copy", &from, e))?;
        }
    }
    std::fs::set_permissions(dest, perms).map_err(|e| io_error("cannot chmod", dest, e))?;
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_error("cannot create the symlink", link, e))
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    // iOS bundles normally have no symlinks; on Windows copy the target file instead.
    let resolved = link.parent().map(|p| p.join(target)).unwrap_or_else(|| target.to_path_buf());
    std::fs::copy(&resolved, link).map(|_| ()).map_err(|e| io_error("cannot copy", &resolved, e))
}
