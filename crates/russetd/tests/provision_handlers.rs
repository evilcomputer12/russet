// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The `provision` handlers in-process: distribution mapping, plan and mutation DTOs, and the error paths of
//! `provision.plan` and `provision.apply`. Planning itself is tested in russet-account.

#![allow(clippy::unwrap_used)]

use russet_account::{CertKind, Mutation};
use russet_protocol::provision as p;
use russet_protocol::sign::Distribution;
use russetd::handlers::provision;

#[test]
fn distributions_round_trip() {
    for d in [Distribution::Development, Distribution::AdHoc, Distribution::AppStoreConnect] {
        assert_eq!(provision::wire_distribution(provision::distribution(d)), d);
    }
    let json = serde_json::to_value(Distribution::AppStoreConnect).unwrap();
    assert_eq!(json, "app-store-connect");
}

#[test]
fn mutations_map_to_kinds_and_summaries() {
    let m = provision::mutation_dto(&Mutation::RegisterBundleId {
        identifier: "com.example.app".into(),
        name: "Russet com example app".into(),
    });
    assert_eq!(m.kind, p::MutationKind::RegisterBundleId);
    assert_eq!(m.bundle_id.as_deref(), Some("com.example.app"));
    assert!(m.summary.contains("com.example.app"));
    let c = provision::mutation_dto(&Mutation::CreateCertificate { kind: CertKind::Development });
    assert_eq!(serde_json::to_value(&c).unwrap()["kind"], "create-certificate");
    let d = provision::mutation_dto(&Mutation::RegisterDevice { udid_redacted: "0000…5F60".into() });
    assert!(d.bundle_id.is_none());
    assert!(d.summary.contains("0000…5F60"));
}

#[test]
fn params_tolerate_unknown_fields_and_default_the_distribution() {
    let params: p::ProvisionPlanParams =
        serde_json::from_value(serde_json::json!({"project": "/p", "somethingNew": true})).unwrap();
    assert_eq!(params.distribution, None);
    assert_eq!(
        provision::distribution(params.distribution.unwrap_or_default()),
        russet_sign::Distribution::Development
    );
}
