// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `ship.*`: archives, validation, upload, TestFlight, App Store versions and review, Apple-hardware CI. Owner: W12 (archive, ipa; Wave 2), W23 (the rest; Wave 4).
//!
//! F0 stub (W1): every name of ARCHITECTURE §8.4 is registered here with placeholder DTOs
//! ([`Unspecified`]). Replace each `…Params`/`…Result` alias with a struct deriving `Serialize, Deserialize,
//! JsonSchema, TS` (camelCase; the wire-enum rules of ARCHITECTURE §5.2), keeping the names so call sites stay.

use crate::{MethodInfo, NotificationInfo, RpcMethod, Unspecified};

/// Params of `ship.archive`: `{project, configuration?, method} → Archive`.
pub type ShipArchiveParams = Unspecified;
/// Result of `ship.archive`.
pub type ShipArchiveResult = Unspecified;
crate::rpc_method!(
    /// `ship.archive` (class `build`, long-running). W12 (Wave 2).
    ShipArchive,
    "ship.archive",
    ShipArchiveParams => ShipArchiveResult,
    Permission::Build, long_running
);

/// Params of `ship.ipa`: `{archivePath, method} → IpaInfo`.
pub type ShipIpaParams = Unspecified;
/// Result of `ship.ipa`.
pub type ShipIpaResult = Unspecified;
crate::rpc_method!(
    /// `ship.ipa` (class `build`). W12 (Wave 2).
    ShipIpa,
    "ship.ipa",
    ShipIpaParams => ShipIpaResult,
    Permission::Build
);

/// Params of `ship.stamp.plan`: `{archivePath, policy} → StampPlan`.
pub type ShipStampPlanParams = Unspecified;
/// Result of `ship.stamp.plan`.
pub type ShipStampPlanResult = Unspecified;
crate::rpc_method!(
    /// `ship.stamp.plan` (class `read`). W23 (Wave 4).
    ShipStampPlan,
    "ship.stamp.plan",
    ShipStampPlanParams => ShipStampPlanResult,
    Permission::Read
);

/// Params of `ship.stamp.apply`: `{planId} → StampReport (needs recorded consent, §16.4)`.
pub type ShipStampApplyParams = Unspecified;
/// Result of `ship.stamp.apply`.
pub type ShipStampApplyResult = Unspecified;
crate::rpc_method!(
    /// `ship.stamp.apply` (class `project`). W23 (Wave 4).
    ShipStampApply,
    "ship.stamp.apply",
    ShipStampApplyParams => ShipStampApplyResult,
    Permission::Project
);

/// Params of `ship.validate`: `{ipaPath, profile?} → ValidationReport`.
pub type ShipValidateParams = Unspecified;
/// Result of `ship.validate`.
pub type ShipValidateResult = Unspecified;
crate::rpc_method!(
    /// `ship.validate` (class `read`). W23 (Wave 4).
    ShipValidate,
    "ship.validate",
    ShipValidateParams => ShipValidateResult,
    Permission::Read
);

/// Params of `ship.upload`: `{ipaPath, appId?, wait?: upload|valid, approval?} → UploadOutcome`.
pub type ShipUploadParams = Unspecified;
/// Result of `ship.upload`.
pub type ShipUploadResult = Unspecified;
crate::rpc_method!(
    /// `ship.upload` (class `publish`, long-running). W23 (Wave 4).
    ShipUpload,
    "ship.upload",
    ShipUploadParams => ShipUploadResult,
    Permission::Publish, long_running
);

/// Params of `ship.upload.resume`: `{uploadId, approval?} → UploadOutcome`.
pub type ShipUploadResumeParams = Unspecified;
/// Result of `ship.upload.resume`.
pub type ShipUploadResumeResult = Unspecified;
crate::rpc_method!(
    /// `ship.upload.resume` (class `publish`, long-running). W23 (Wave 4).
    ShipUploadResume,
    "ship.upload.resume",
    ShipUploadResumeParams => ShipUploadResumeResult,
    Permission::Publish, long_running
);

/// Params of `ship.upload.status`: `{uploadId} → UploadOutcome`.
pub type ShipUploadStatusParams = Unspecified;
/// Result of `ship.upload.status`.
pub type ShipUploadStatusResult = Unspecified;
crate::rpc_method!(
    /// `ship.upload.status` (class `read`). W23 (Wave 4).
    ShipUploadStatus,
    "ship.upload.status",
    ShipUploadStatusParams => ShipUploadStatusResult,
    Permission::Read
);

/// Params of `ship.builds.list`: `{appId?|bundleId?, limit?} → {builds[]}`.
pub type ShipBuildsListParams = Unspecified;
/// Result of `ship.builds.list`.
pub type ShipBuildsListResult = Unspecified;
crate::rpc_method!(
    /// `ship.builds.list` (class `read`). W23 (Wave 4).
    ShipBuildsList,
    "ship.builds.list",
    ShipBuildsListParams => ShipBuildsListResult,
    Permission::Read
);

/// Params of `ship.testflight.groups`: `{appId} → {groups[{id, name, internal, publicLink?, testerCount}]}`.
pub type ShipTestflightGroupsParams = Unspecified;
/// Result of `ship.testflight.groups`.
pub type ShipTestflightGroupsResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.groups` (class `read`). W23 (Wave 4).
    ShipTestflightGroups,
    "ship.testflight.groups",
    ShipTestflightGroupsParams => ShipTestflightGroupsResult,
    Permission::Read
);

/// Params of `ship.testflight.testers`: `{appId, groupId?} → {testers[{id, email (masked for mcp), state, groups[]}]}`.
pub type ShipTestflightTestersParams = Unspecified;
/// Result of `ship.testflight.testers`.
pub type ShipTestflightTestersResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.testers` (class `read`). W23 (Wave 4).
    ShipTestflightTesters,
    "ship.testflight.testers",
    ShipTestflightTestersParams => ShipTestflightTestersResult,
    Permission::Read
);

/// Params of `ship.testflight.groups.create`: `{appId, name, internal, publicLinkEnabled?, publicLinkLimit?} → group; never exposed to MCP`.
pub type ShipTestflightGroupsCreateParams = Unspecified;
/// Result of `ship.testflight.groups.create`.
pub type ShipTestflightGroupsCreateResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.groups.create` (class `publish`). W23 (Wave 4).
    ShipTestflightGroupsCreate,
    "ship.testflight.groups.create",
    ShipTestflightGroupsCreateParams => ShipTestflightGroupsCreateResult,
    Permission::Publish
);

/// Params of `ship.testflight.addTesters`: `{groupId, testers[{email, firstName?, lastName?}]} → records; never exposed to MCP`.
pub type ShipTestflightAddTestersParams = Unspecified;
/// Result of `ship.testflight.addTesters`.
pub type ShipTestflightAddTestersResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.addTesters` (class `publish`). W23 (Wave 4).
    ShipTestflightAddTesters,
    "ship.testflight.addTesters",
    ShipTestflightAddTestersParams => ShipTestflightAddTestersResult,
    Permission::Publish
);

/// Params of `ship.testflight.removeTesters`: `{groupId, testerIds} → records; never exposed to MCP`.
pub type ShipTestflightRemoveTestersParams = Unspecified;
/// Result of `ship.testflight.removeTesters`.
pub type ShipTestflightRemoveTestersResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.removeTesters` (class `publish`). W23 (Wave 4).
    ShipTestflightRemoveTesters,
    "ship.testflight.removeTesters",
    ShipTestflightRemoveTestersParams => ShipTestflightRemoveTestersResult,
    Permission::Publish
);

/// Params of `ship.testflight.distribute`: `{buildId, groupIds, whatToTest?, usesNonExemptEncryption?, notify?, approval?} → records`.
pub type ShipTestflightDistributeParams = Unspecified;
/// Result of `ship.testflight.distribute`.
pub type ShipTestflightDistributeResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.distribute` (class `publish`). W23 (Wave 4).
    ShipTestflightDistribute,
    "ship.testflight.distribute",
    ShipTestflightDistributeParams => ShipTestflightDistributeResult,
    Permission::Publish
);

/// Params of `ship.testflight.betaReview.submit`: `{buildId} → {submissionId, state}; never exposed to MCP`.
pub type ShipTestflightBetaReviewSubmitParams = Unspecified;
/// Result of `ship.testflight.betaReview.submit`.
pub type ShipTestflightBetaReviewSubmitResult = Unspecified;
crate::rpc_method!(
    /// `ship.testflight.betaReview.submit` (class `publish`). W23 (Wave 4).
    ShipTestflightBetaReviewSubmit,
    "ship.testflight.betaReview.submit",
    ShipTestflightBetaReviewSubmitParams => ShipTestflightBetaReviewSubmitResult,
    Permission::Publish
);

/// Params of `ship.metadata.pull`: `{appId, version, dir} → files`.
pub type ShipMetadataPullParams = Unspecified;
/// Result of `ship.metadata.pull`.
pub type ShipMetadataPullResult = Unspecified;
crate::rpc_method!(
    /// `ship.metadata.pull` (class `read`). W23 (Wave 4).
    ShipMetadataPull,
    "ship.metadata.pull",
    ShipMetadataPullParams => ShipMetadataPullResult,
    Permission::Read
);

/// Params of `ship.metadata.plan`: `{appId, version, dir} → plan`.
pub type ShipMetadataPlanParams = Unspecified;
/// Result of `ship.metadata.plan`.
pub type ShipMetadataPlanResult = Unspecified;
crate::rpc_method!(
    /// `ship.metadata.plan` (class `read`). W23 (Wave 4).
    ShipMetadataPlan,
    "ship.metadata.plan",
    ShipMetadataPlanParams => ShipMetadataPlanResult,
    Permission::Read
);

/// Params of `ship.metadata.apply`: `{planId, approval?} → job (never exposed to MCP)`.
pub type ShipMetadataApplyParams = Unspecified;
/// Result of `ship.metadata.apply`.
pub type ShipMetadataApplyResult = Unspecified;
crate::rpc_method!(
    /// `ship.metadata.apply` (class `publish`, long-running). W23 (Wave 4).
    ShipMetadataApply,
    "ship.metadata.apply",
    ShipMetadataApplyParams => ShipMetadataApplyResult,
    Permission::Publish, long_running
);

/// Params of `ship.version.plan`: `{appId, versionString, buildId, platform?: ios, usesNonExemptEncryption?, releaseType?, earliestReleaseDate?} → VersionPlan {planId, digest, mutations[]}`.
pub type ShipVersionPlanParams = Unspecified;
/// Result of `ship.version.plan`.
pub type ShipVersionPlanResult = Unspecified;
crate::rpc_method!(
    /// `ship.version.plan` (class `read`). W23 (Wave 4).
    ShipVersionPlan,
    "ship.version.plan",
    ShipVersionPlanParams => ShipVersionPlanResult,
    Permission::Read
);

/// Params of `ship.version.apply`: `{planId} → {versionId, state} (never exposed to MCP)`.
pub type ShipVersionApplyParams = Unspecified;
/// Result of `ship.version.apply`.
pub type ShipVersionApplyResult = Unspecified;
crate::rpc_method!(
    /// `ship.version.apply` (class `publish`, long-running). W23 (Wave 4).
    ShipVersionApply,
    "ship.version.apply",
    ShipVersionApplyParams => ShipVersionApplyResult,
    Permission::Publish, long_running
);

/// Params of `ship.review.submit`: `{versionId, confirmText?} → {reviewSubmissionId, state} (typed confirmation; cli/ide only; never exposed to MCP)`.
pub type ShipReviewSubmitParams = Unspecified;
/// Result of `ship.review.submit`.
pub type ShipReviewSubmitResult = Unspecified;
crate::rpc_method!(
    /// `ship.review.submit` (class `publish`, typed confirmation). W23 (Wave 4).
    ShipReviewSubmit,
    "ship.review.submit",
    ShipReviewSubmitParams => ShipReviewSubmitResult,
    Permission::Publish, confirm = Typed
);

/// Params of `ship.review.status`: `{appId?|versionId?} → {submissions[{id, state, submittedAt?, items[]}]}`.
pub type ShipReviewStatusParams = Unspecified;
/// Result of `ship.review.status`.
pub type ShipReviewStatusResult = Unspecified;
crate::rpc_method!(
    /// `ship.review.status` (class `read`). W23 (Wave 4).
    ShipReviewStatus,
    "ship.review.status",
    ShipReviewStatusParams => ShipReviewStatusResult,
    Permission::Read
);

/// Params of `ship.ci.init`: `{provider: github|xcode-cloud, signing?: russet|xcode-manual|xcode-cloud} → {files[]}`.
pub type ShipCiInitParams = Unspecified;
/// Result of `ship.ci.init`.
pub type ShipCiInitResult = Unspecified;
crate::rpc_method!(
    /// `ship.ci.init` (class `project`). W23 (Wave 4).
    ShipCiInit,
    "ship.ci.init",
    ShipCiInitParams => ShipCiInitResult,
    Permission::Project
);

/// Params of `ship.ci.dispatch`: `{ref?, approval?} → {runId}`.
pub type ShipCiDispatchParams = Unspecified;
/// Result of `ship.ci.dispatch`.
pub type ShipCiDispatchResult = Unspecified;
crate::rpc_method!(
    /// `ship.ci.dispatch` (class `publish`). W23 (Wave 4).
    ShipCiDispatch,
    "ship.ci.dispatch",
    ShipCiDispatchParams => ShipCiDispatchResult,
    Permission::Publish
);

/// Params of `ship.ci.status`: `{runId} → status (also checks the pinned runner label and Xcode, 7010)`.
pub type ShipCiStatusParams = Unspecified;
/// Result of `ship.ci.status`.
pub type ShipCiStatusResult = Unspecified;
crate::rpc_method!(
    /// `ship.ci.status` (class `read`). W23 (Wave 4).
    ShipCiStatus,
    "ship.ci.status",
    ShipCiStatusParams => ShipCiStatusResult,
    Permission::Read
);

/// Params of `ship.ci.fetch`: `{runId} → {paths[]} (deletes the run's artifacts after the download)`.
pub type ShipCiFetchParams = Unspecified;
/// Result of `ship.ci.fetch`.
pub type ShipCiFetchResult = Unspecified;
crate::rpc_method!(
    /// `ship.ci.fetch` (class `read`). W23 (Wave 4).
    ShipCiFetch,
    "ship.ci.fetch",
    ShipCiFetchParams => ShipCiFetchResult,
    Permission::Read
);

/// Params of `ship.crashes.list`: `{appId?|bundleId?, limit?}`.
pub type ShipCrashesListParams = Unspecified;
/// Result of `ship.crashes.list`.
pub type ShipCrashesListResult = Unspecified;
crate::rpc_method!(
    /// `ship.crashes.list` (class `read`). W23 (Wave 4).
    ShipCrashesList,
    "ship.crashes.list",
    ShipCrashesListParams => ShipCrashesListResult,
    Permission::Read
);

/// Params of `ship.crashes.get`: `{submissionId, symbolicate?}`.
pub type ShipCrashesGetParams = Unspecified;
/// Result of `ship.crashes.get`.
pub type ShipCrashesGetResult = Unspecified;
crate::rpc_method!(
    /// `ship.crashes.get` (class `read`). W23 (Wave 4).
    ShipCrashesGet,
    "ship.crashes.get",
    ShipCrashesGetParams => ShipCrashesGetResult,
    Permission::Read
);

/// Params of `ship.compare`: `{a, b} → IpaDiff`.
pub type ShipCompareParams = Unspecified;
/// Result of `ship.compare`.
pub type ShipCompareResult = Unspecified;
crate::rpc_method!(
    /// `ship.compare` (class `read`). W23 (Wave 4).
    ShipCompare,
    "ship.compare",
    ShipCompareParams => ShipCompareResult,
    Permission::Read
);

/// Methods the daemon serves in this namespace.
pub const METHODS: &[MethodInfo] = &[
    ShipArchive::INFO,
    ShipIpa::INFO,
    ShipStampPlan::INFO,
    ShipStampApply::INFO,
    ShipValidate::INFO,
    ShipUpload::INFO,
    ShipUploadResume::INFO,
    ShipUploadStatus::INFO,
    ShipBuildsList::INFO,
    ShipTestflightGroups::INFO,
    ShipTestflightTesters::INFO,
    ShipTestflightGroupsCreate::INFO,
    ShipTestflightAddTesters::INFO,
    ShipTestflightRemoveTesters::INFO,
    ShipTestflightDistribute::INFO,
    ShipTestflightBetaReviewSubmit::INFO,
    ShipMetadataPull::INFO,
    ShipMetadataPlan::INFO,
    ShipMetadataApply::INFO,
    ShipVersionPlan::INFO,
    ShipVersionApply::INFO,
    ShipReviewSubmit::INFO,
    ShipReviewStatus::INFO,
    ShipCiInit::INFO,
    ShipCiDispatch::INFO,
    ShipCiStatus::INFO,
    ShipCiFetch::INFO,
    ShipCrashesList::INFO,
    ShipCrashesGet::INFO,
    ShipCompare::INFO,
];
/// Notifications the daemon sends in this namespace.
pub const NOTIFICATIONS: &[NotificationInfo] = &[];
/// Requests the daemon sends to clients in this namespace.
pub const CLIENT_METHODS: &[MethodInfo] = &[];
