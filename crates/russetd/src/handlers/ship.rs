// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ship` handlers (§8.4). Owner: W12 (archive, ipa; Wave 2), W23 (the rest; Wave 4).
//!
//! F0 stub (W1): every method answers `core.notImplemented` (1005) until its owner replaces the handler.

use russet_ipc::Router;
use russet_protocol::ship as p;

use super::not_implemented;
use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ShipArchive, _, _>(not_implemented::<p::ShipArchive>);
    r.request::<p::ShipIpa, _, _>(not_implemented::<p::ShipIpa>);
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
