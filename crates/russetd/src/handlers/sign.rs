// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `sign` handlers (§8.4). Owner: W6.
//!
//! `sign.run` signs with a stored identity and the given or cached profiles (`provision.apply` caches them);
//! `sign.verify`, `sign.lint` and `sign.inspect` read `.app` directories and `.ipa` files. Signing and reading
//! run on blocking threads.

use std::path::PathBuf;

use russet_account::{AccountService, IdentityId, SignAppRequest};
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::Router;
use russet_protocol::sign as p;
use russet_sign::{LintFinding, LintProfile, MergePolicy, PreSignFixes, Severity, SignatureInfo, VerifyOptions};

use super::account::absolute;
use super::provision::distribution;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::SignRun, _, _>(|cx: DaemonCtx, _rq, params: p::SignRunParams| async move {
        run(&cx.accounts, params).await
    });
    r.request::<p::SignVerify, _, _>(
        |_cx: DaemonCtx, _rq, params: p::SignPathParams| async move { verify(params).await },
    );
    r.request::<p::SignLint, _, _>(|_cx: DaemonCtx, _rq, params: p::SignPathParams| async move { lint(params).await });
    r.request::<p::SignInspect, _, _>(
        |_cx: DaemonCtx, _rq, params: p::SignPathParams| async move { inspect(params).await },
    );
}

/// The domain lint profile of a wire value.
pub fn lint_profile(p: p::LintProfile) -> LintProfile {
    match p {
        p::LintProfile::DeviceInstall => LintProfile::DeviceInstall,
        p::LintProfile::AppStore => LintProfile::AppStore,
    }
}

/// `sign.run`.
pub async fn run(svc: &AccountService, params: p::SignRunParams) -> Result<p::SignRunResult> {
    let path = absolute(&params.path, "path")?;
    let mut req = SignAppRequest::new(path);
    req.distribution = distribution(params.distribution);
    req.identity = params.identity_id.map(IdentityId);
    for (rel, file) in params.profiles {
        req.profiles.insert(rel, absolute(&file, "profiles")?);
    }
    for (rel, file) in params.entitlements {
        req.entitlements.insert(rel, absolute(&file, "entitlements")?);
    }
    req.merge_policy = match params.merge_policy.unwrap_or_default() {
        p::MergePolicy::Least => MergePolicy::Least,
        p::MergePolicy::ProfileAll => MergePolicy::ProfileAll,
    };
    req.output = params.output.as_deref().map(|o| absolute(o, "output")).transpose()?;
    req.get_task_allow = params.get_task_allow;
    req.device = params.device;
    req.lint = Some(lint_profile(params.lint.unwrap_or(p::LintProfile::DeviceInstall)));
    let mut fixes = PreSignFixes::default();
    for name in &params.no_fix {
        if !fixes.disable(name) {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("unknown pre-sign fix `{name}`"))
                .with_hint("Known fixes: chmod, align-string-pool, clean, thin."));
        }
    }
    req.fixes = fixes;
    let report = svc.sign_app(req).await?;
    Ok(p::SignRunResult {
        output: report.output.display().to_string(),
        bundles: report
            .bundles
            .iter()
            .map(|b| p::SignedBundle {
                rel_path: b.rel_path.clone(),
                bundle_id: b.bundle_id.clone(),
                kind: b.kind.as_str().to_owned(),
                cdhash: b.cdhash.clone(),
                team_id: russet_sign::mask_team(&b.team_id),
                profile_uuid: b.profile_uuid.clone(),
                notes: b.notes.iter().map(|n| format!("{}: {}", n.key, n.message)).collect(),
            })
            .collect(),
        fixes_applied: report
            .fixes_applied
            .iter()
            .map(|f| p::FixRecord { fix: f.fix.clone(), path: f.path.clone() })
            .collect(),
        lints: report.lints.iter().map(finding_dto).collect(),
    })
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("a signing task failed: {e}")))?
}

/// `sign.verify`.
pub async fn verify(params: p::SignPathParams) -> Result<p::SignVerifyResult> {
    let path: PathBuf = absolute(&params.path, "path")?;
    let opts = VerifyOptions { lints: params.profile.map(lint_profile), device: params.device };
    let report = blocking(move || russet_sign::verify(&path, &opts)).await?;
    Ok(p::SignVerifyResult {
        ok: report.ok,
        path: report.path.display().to_string(),
        bundles: report
            .bundles
            .iter()
            .map(|b| p::VerifiedBundle {
                rel_path: b.rel_path.clone(),
                bundle_id: b.bundle_id.clone(),
                identifier: b.identifier.clone(),
                cdhash: b.cdhash.clone(),
                team_id: b.team_id.as_deref().map(russet_sign::mask_team),
                profile_uuid: b.profile_uuid.clone(),
                signed: b.signed,
            })
            .collect(),
        findings: report.findings.iter().map(finding_dto).collect(),
    })
}

/// `sign.lint`.
pub async fn lint(params: p::SignPathParams) -> Result<p::SignLintResult> {
    let path: PathBuf = absolute(&params.path, "path")?;
    let profile = lint_profile(params.profile.unwrap_or(p::LintProfile::DeviceInstall));
    let findings = blocking(move || russet_sign::lint(&path, profile)).await?;
    Ok(p::SignLintResult { findings: findings.iter().map(finding_dto).collect() })
}

/// `sign.inspect`.
pub async fn inspect(params: p::SignPathParams) -> Result<p::SignInspectResult> {
    let path: PathBuf = absolute(&params.path, "path")?;
    let infos = blocking(move || russet_sign::inspect(&path)).await?;
    Ok(p::SignInspectResult { signatures: infos.iter().map(signature_dto).collect() })
}

/// The wire form of a finding.
pub fn finding_dto(f: &LintFinding) -> p::LintFinding {
    p::LintFinding {
        code: f.code.clone(),
        severity: match f.severity {
            Severity::Info => p::Severity::Info,
            Severity::Warning => p::Severity::Warning,
            Severity::Error => p::Severity::Error,
        },
        message: f.message.clone(),
        path: f.path.clone(),
    }
}

/// The wire form of a signature (russet-sign already masks the team).
pub fn signature_dto(s: &SignatureInfo) -> p::SignatureInfo {
    p::SignatureInfo {
        path: s.path.clone(),
        bundle_id: s.bundle_id.clone(),
        arch: s.arch.clone(),
        filetype: s.filetype.clone(),
        signed: s.signed,
        identifier: s.identifier.clone(),
        team_id: s.team_id.clone(),
        code_directories: s
            .code_directories
            .iter()
            .map(|c| p::CodeDirectoryInfo {
                version: c.version.clone(),
                hash_type: c.hash_type.clone(),
                page_size: c.page_size,
                flags: c.flags.clone(),
                exec_seg_flags: c.exec_seg_flags.clone(),
                cdhash: c.cdhash.clone(),
            })
            .collect(),
        entitlement_keys: s.entitlement_keys.clone(),
        get_task_allow: s.get_task_allow,
        has_der_entitlements: s.has_der_entitlements,
        has_cms: s.has_cms,
        platform: s.platform,
        minos: s.minos.clone(),
        sdk: s.sdk.clone(),
        string_pool_aligned: s.string_pool_aligned,
    }
}
