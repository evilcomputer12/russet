// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ship` handlers (§8.4). Owner: W12 (`ship.archive`, `ship.ipa`; Wave 2), W23 (the rest; Wave 4).
//!
//! `ship.archive` builds the project (the same entry point as `build.start`) and lays out an `.xcarchive`
//! (SIGN §9.1); `ship.ipa` writes the deterministic store/development IPA (SIGN §9.3) from an archive or a built
//! `.app`. The other rows answer `core.notImplemented` until W23.

use russet_build::Utf8PathBuf;
use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_ipc::{RequestCtx, Router};
use russet_protocol::ship as p;
use russet_ship::{Archive, ArchiveRequest, ExportMethod, IpaInfo};

use super::account::redacted_cx;
use super::build::{build_project, wire_path};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ShipArchive, _, _>(|cx: DaemonCtx, rq: RequestCtx, params: p::ShipArchiveParams| async move {
        redacted_cx(&cx, archive(&cx, rq, params).await).await
    });
    r.request::<p::ShipIpa, _, _>(|cx: DaemonCtx, _rq, params: p::ShipIpaParams| async move {
        redacted_cx(&cx, ipa(params).await).await
    });
    r.request::<p::ShipStampPlan, _, _>(not_implemented::<p::ShipStampPlan>);
    r.request::<p::ShipStampApply, _, _>(not_implemented::<p::ShipStampApply>);
    r.request::<p::ShipValidate, _, _>(not_implemented::<p::ShipValidate>);
    r.request::<p::ShipUpload, _, _>(not_implemented::<p::ShipUpload>);
    r.request::<p::ShipUploadResume, _, _>(not_implemented::<p::ShipUploadResume>);
    r.request::<p::ShipUploadStatus, _, _>(not_implemented::<p::ShipUploadStatus>);
    r.request::<p::ShipBuildsList, _, _>(not_implemented::<p::ShipBuildsList>);
    r.request::<p::ShipTestflightGroups, _, _>(not_implemented::<p::ShipTestflightGroups>);
    r.request::<p::ShipTestflightTesters, _, _>(not_implemented::<p::ShipTestflightTesters>);
    r.request::<p::ShipTestflightGroupsCreate, _, _>(not_implemented::<p::ShipTestflightGroupsCreate>);
    r.request::<p::ShipTestflightAddTesters, _, _>(not_implemented::<p::ShipTestflightAddTesters>);
    r.request::<p::ShipTestflightRemoveTesters, _, _>(not_implemented::<p::ShipTestflightRemoveTesters>);
    r.request::<p::ShipTestflightDistribute, _, _>(not_implemented::<p::ShipTestflightDistribute>);
    r.request::<p::ShipTestflightBetaReviewSubmit, _, _>(not_implemented::<p::ShipTestflightBetaReviewSubmit>);
    r.request::<p::ShipMetadataPull, _, _>(not_implemented::<p::ShipMetadataPull>);
    r.request::<p::ShipMetadataPlan, _, _>(not_implemented::<p::ShipMetadataPlan>);
    r.request::<p::ShipMetadataApply, _, _>(not_implemented::<p::ShipMetadataApply>);
    r.request::<p::ShipVersionPlan, _, _>(not_implemented::<p::ShipVersionPlan>);
    r.request::<p::ShipVersionApply, _, _>(not_implemented::<p::ShipVersionApply>);
    r.request::<p::ShipReviewSubmit, _, _>(not_implemented::<p::ShipReviewSubmit>);
    r.request::<p::ShipReviewStatus, _, _>(not_implemented::<p::ShipReviewStatus>);
    r.request::<p::ShipCiInit, _, _>(not_implemented::<p::ShipCiInit>);
    r.request::<p::ShipCiDispatch, _, _>(not_implemented::<p::ShipCiDispatch>);
    r.request::<p::ShipCiStatus, _, _>(not_implemented::<p::ShipCiStatus>);
    r.request::<p::ShipCiFetch, _, _>(not_implemented::<p::ShipCiFetch>);
    r.request::<p::ShipCrashesList, _, _>(not_implemented::<p::ShipCrashesList>);
    r.request::<p::ShipCrashesGet, _, _>(not_implemented::<p::ShipCrashesGet>);
    r.request::<p::ShipCompare, _, _>(not_implemented::<p::ShipCompare>);
}

/// The export method of a wire value.
fn method(s: &str) -> Result<ExportMethod> {
    ExportMethod::parse(s).ok_or_else(|| {
        Error::new(codes::core::INVALID_ARGUMENT, format!("unknown export method `{s}`"))
            .with_hint("Use development, ad-hoc, app-store-connect or enterprise (SIGN §9.2).")
    })
}

/// `ship.archive`: build the project, then lay out an `.xcarchive` under `<project>/.russet/archives`.
pub async fn archive(cx: &DaemonCtx, rq: RequestCtx, params: p::ShipArchiveParams) -> Result<p::ShipArchiveResult> {
    let project = wire_path(&params.project, "project")?;
    let method = method(&params.method)?;
    let mut request = russet_build::BuildRequest::new(project.clone());
    request.configuration = params.configuration.clone().or(Some("release".to_owned()));
    let out = build_project(cx, request, &russet_build::NullSink, rq.cancel.clone()).await?;

    let archive = {
        let app = out.app.path.clone();
        let name = out.app.product.clone();
        let dsyms = out.dsyms.clone();
        let dest = project.join(".russet").join("archives");
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&dest)
                .map_err(|e| Error::new(codes::ship::ARCHIVE_FAILED, format!("cannot create {dest}: {e}")))?;
            let mut req = ArchiveRequest::new(app, name, method);
            req.dsyms = dsyms;
            req.created = russet_ship::default_epoch();
            russet_ship::create_archive(&req, &dest)
        })
        .await
        .map_err(|e| Error::new(codes::core::INTERNAL, format!("the archive task failed: {e}")))??
    };
    Ok(archive_dto(&archive))
}

/// `ship.ipa`: package an archive's app (or a built `.app`) as an IPA.
pub async fn ipa(params: p::ShipIpaParams) -> Result<p::ShipIpaResult> {
    let input: Utf8PathBuf = wire_path(&params.archive_path, "archivePath")?;
    let method = method(&params.method)?;
    let out = params.out.map(|o| wire_path(&o, "out")).transpose()?;
    let info = tokio::task::spawn_blocking(move || -> Result<IpaInfo> {
        let app = if input.extension() == Some("app") { input.clone() } else { russet_ship::archive_app(&input)? };
        let out = out.unwrap_or_else(|| input.with_extension("ipa"));
        russet_ship::package_ipa(&app, method, &out)
    })
    .await
    .map_err(|e| Error::new(codes::core::INTERNAL, format!("the IPA task failed: {e}")))??;
    Ok(ipa_dto(&info))
}

/// The wire form of an archive.
pub fn archive_dto(a: &Archive) -> p::ShipArchiveResult {
    p::ShipArchiveResult {
        path: a.path.to_string(),
        app_path: a.app_rel.clone(),
        bundle_id: a.bundle_id.clone(),
        short_version: a.short_version.clone(),
        build_version: a.build_version.clone(),
        dsyms: a.dsyms.iter().map(ToString::to_string).collect(),
        digest: a.digest.clone(),
    }
}

/// The wire form of IPA info.
pub fn ipa_dto(i: &IpaInfo) -> p::ShipIpaResult {
    p::ShipIpaResult {
        path: i.path.to_string(),
        size: i.size,
        sha256: i.sha256.clone(),
        bundle_id: i.bundle_id.clone(),
        short_version: i.short_version.clone(),
        build_version: i.build_version.clone(),
    }
}
