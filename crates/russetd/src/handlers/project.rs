// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `project` handlers (§8.3). Owner: W5 (Wave 1), W16 (project.init).
//!
//! `project.detect` and `project.info` read the project without running SwiftPM; `project.infoPlist` resolves the
//! SDK and returns the Info.plist a build would write, with each key's provenance. `project.init` is W16's.

use russet_build::BuildRequest;
use russet_core::Result;
use russet_ipc::{RequestCtx, Router};
use russet_protocol::project::{self as p, TargetKind};

use super::build::{builder, stack_dto, wire_path};
use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ProjectDetect, _, _>(project_detect);
    r.request::<p::ProjectInfo, _, _>(project_info);
    r.request::<p::ProjectInit, _, _>(not_implemented::<p::ProjectInit>);
    r.request::<p::ProjectInfoPlist, _, _>(project_info_plist);
}

/// `project.detect`'s result for a detected project.
pub fn detect_dto(info: &russet_build::ProjectInfo) -> p::ProjectDetectResult {
    p::ProjectDetectResult {
        stack: stack_dto(info.stack),
        root: info.root.to_string(),
        products: info.products.clone(),
        manifests: info.manifests.iter().map(ToString::to_string).collect(),
    }
}

/// `project.info`'s result for a detected project.
pub fn info_dto(info: &russet_build::ProjectInfo) -> p::ProjectInfoResult {
    p::ProjectInfoResult {
        stack: stack_dto(info.stack),
        root: info.root.to_string(),
        name: info.name.clone(),
        targets: info
            .targets
            .iter()
            .map(|t| p::ProjectTarget {
                name: t.name.clone(),
                bundle_id: t.bundle_id.clone(),
                kind: match t.kind {
                    russet_build::BundleKind::App => TargetKind::App,
                    russet_build::BundleKind::AppExtension => TargetKind::AppExtension,
                },
            })
            .collect(),
        configurations: info.configurations.clone(),
        default_target: info.default_target.clone(),
    }
}

async fn project_detect(cx: DaemonCtx, _rq: RequestCtx, p: p::ProjectDetectParams) -> Result<p::ProjectDetectResult> {
    let info = builder(&cx)?.detect(&wire_path(&p.dir, "dir")?).await?;
    Ok(detect_dto(&info))
}

async fn project_info(cx: DaemonCtx, _rq: RequestCtx, p: p::ProjectInfoParams) -> Result<p::ProjectInfoResult> {
    let info = builder(&cx)?.detect(&wire_path(&p.project, "project")?).await?;
    Ok(info_dto(&info))
}

async fn project_info_plist(
    cx: DaemonCtx,
    _rq: RequestCtx,
    p: p::ProjectInfoPlistParams,
) -> Result<p::ProjectInfoPlistResult> {
    let mut r = BuildRequest::new(wire_path(&p.project, "project")?);
    r.configuration = p.configuration.clone();
    let (plist, provenance) = builder(&cx)?.info_plist_with_provenance(&r, p.target.as_deref()).await?;
    Ok(p::ProjectInfoPlistResult { plist: russet_build::infoplist::dict_to_json(&plist), provenance })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_infos_convert_to_the_wire() {
        let info = russet_build::ProjectInfo {
            root: "/p".into(),
            stack: russet_build::Stack::Swiftpm,
            name: "P".into(),
            manifests: vec!["/p/russet.toml".into(), "/p/Package.swift".into()],
            products: vec!["P".into()],
            targets: vec![
                russet_build::ProjectTarget {
                    name: "P".into(),
                    bundle_id: Some("a.b".into()),
                    kind: russet_build::BundleKind::App,
                },
                russet_build::ProjectTarget {
                    name: "W".into(),
                    bundle_id: None,
                    kind: russet_build::BundleKind::AppExtension,
                },
            ],
            configurations: vec!["debug".into(), "release".into()],
            default_target: Some("P".into()),
        };
        let d = detect_dto(&info);
        assert_eq!(d.manifests, vec!["/p/russet.toml".to_owned(), "/p/Package.swift".to_owned()]);
        let i = info_dto(&info);
        assert_eq!(i.targets[1].kind, TargetKind::AppExtension);
        assert_eq!(
            serde_json::to_value(&i).unwrap_or_default()["targets"][0],
            serde_json::json!({"name": "P", "bundleId": "a.b", "kind": "app"})
        );
    }
}
