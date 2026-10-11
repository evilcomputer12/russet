// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `build.*` and `project.*` wire shapes (ARCHITECTURE §8.3): camelCase fields, kebab-case enum values,
//! internally tagged policies, omitted optionals.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use russet_protocol::build::{
    BuildBackend, BuildCleanParams, BuildOutput, BuildStartParams, BundleIdPolicy, BundleInfo, CleanScope, LintFinding,
    LintSeverity, PackageKind, SignItem, SignKind, SigningPlan,
};
use russet_protocol::project::{ProjectDetectResult, ProjectInfoPlistParams, ProjectStack, TargetKind};
use serde_json::json;

fn round_trip<T>(value: &T, expected: serde_json::Value)
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
    let back: T = serde_json::from_value(expected).unwrap();
    assert_eq!(&back, value);
}

#[test]
fn build_start_params_use_the_catalog_names() {
    let p = BuildStartParams {
        project: "/p".into(),
        configuration: Some("release".into()),
        package: Some(PackageKind::IpaStore),
        backend: Some(BuildBackend::Swiftbuild),
        bundle_id_policy: Some(BundleIdPolicy::XtoolPrefix { prefix: "AB12".into() }),
        ..Default::default()
    };
    round_trip(
        &p,
        json!({"project": "/p", "configuration": "release", "package": "ipa-store", "backend": "swiftbuild",
               "bundleIdPolicy": {"kind": "xtool-prefix", "prefix": "AB12"}}),
    );
    // Unknown fields (background, progressToken) are ignored, never refused.
    let p: BuildStartParams = serde_json::from_value(json!({"project": "/p", "background": true})).unwrap();
    assert_eq!(p.project, "/p");
}

#[test]
fn bundle_id_policies_are_internally_tagged() {
    round_trip(&BundleIdPolicy::Exact, json!({"kind": "exact"}));
    round_trip(&BundleIdPolicy::TeamSuffix { team_id: "T".into() }, json!({"kind": "team-suffix", "teamId": "T"}));
    let map = BTreeMap::from([("a.b".to_owned(), "c.d".to_owned())]);
    round_trip(&BundleIdPolicy::Custom { map }, json!({"kind": "custom", "map": {"a.b": "c.d"}}));
}

#[test]
fn build_output_matches_what_scripts_read() {
    let out = BuildOutput {
        build_id: serde_json::from_value(json!("job_01J00000000000000000000000")).unwrap(),
        app: BundleInfo {
            path: "/p/.russet/build/debug-iphoneos/P.app".into(),
            executable: "/p/.russet/build/debug-iphoneos/P.app/P".into(),
            bundle_id: "dev.russet.test.probe".into(),
            original_bundle_id: "dev.russet.test.probe".into(),
            kind: TargetKind::App,
            product: "P".into(),
            version: "1.0.0".into(),
            build_number: "1".into(),
        },
        extensions: Vec::new(),
        bundle_ids: BTreeMap::from([("dev.russet.test.probe".to_owned(), "dev.russet.test.probe".to_owned())]),
        signing_plan: SigningPlan {
            items: vec![SignItem {
                path: "/p/P.app".into(),
                kind: SignKind::Bundle,
                entitlements: None,
                get_task_allow: true,
            }],
        },
        dsyms: Vec::new(),
        ipa: None,
        report_path: "/p/.russet/build/debug-iphoneos/build-report.json".into(),
        lints: vec![LintFinding {
            id: "L006".into(),
            severity: LintSeverity::Warning,
            path: None,
            message: "m".into(),
            fixed: true,
        }],
        duration_ms: 8000,
    };
    let v = serde_json::to_value(&out).unwrap();
    // PLAN §4.10: `russet build --json | jq -e '.app.path'`.
    assert_eq!(v["app"]["path"], "/p/.russet/build/debug-iphoneos/P.app");
    assert_eq!(v["buildId"], "job_01J00000000000000000000000");
    assert_eq!(v["signingPlan"]["items"][0], json!({"path": "/p/P.app", "kind": "bundle", "getTaskAllow": true}));
    assert_eq!(v["lints"][0]["severity"], "warning");
    assert_eq!(v["app"]["kind"], "app");
    assert!(v.get("ipa").is_none(), "optional fields are omitted, not null");
    let back: BuildOutput = serde_json::from_value(v).unwrap();
    assert_eq!(back, out);
    assert_eq!(serde_json::to_value(SignKind::MachO).unwrap(), "mach-o");
}

#[test]
fn clean_and_project_shapes() {
    round_trip(
        &BuildCleanParams { project: "/p".into(), scope: CleanScope::All },
        json!({"project": "/p", "scope": "all"}),
    );
    round_trip(
        &ProjectDetectResult {
            stack: ProjectStack::ReactNative,
            root: "/p".into(),
            products: vec!["A".into()],
            manifests: Vec::new(),
        },
        json!({"stack": "react-native", "root": "/p", "products": ["A"], "manifests": []}),
    );
    round_trip(
        &ProjectInfoPlistParams { project: "/p".into(), target: None, configuration: None },
        json!({"project": "/p"}),
    );
}
