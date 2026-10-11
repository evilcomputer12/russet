// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The per-bundle entitlement merge (SIGN §8.4) and the entitlement → capability map (SIGN §6.5).
//!
//! Inputs: the requested entitlements `R` (the project's `.entitlements`), the profile's `P`, the final bundle
//! ID `B`, the App ID prefix `X`, the team `T` and the distribution `D`. The result signs exactly what the
//! profile authorizes, so the device installer cannot reject it for entitlements (`0xe8008015`/`0xe8008016`).

use plist::{Dictionary, Value};
use russet_core::error::{Error, codes};

use crate::profile::ProvisioningProfile;
use crate::util::{glob_match, redact_app_id};

/// The distribution a bundle is signed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Distribution {
    #[default]
    Development,
    AdHoc,
    /// App Store Connect (TestFlight and the App Store).
    AppStore,
}

impl Distribution {
    /// The wire value: `development`, `ad-hoc`, `app-store-connect`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::AdHoc => "ad-hoc",
            Self::AppStore => "app-store-connect",
        }
    }

    /// Parses the wire value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "development" => Some(Self::Development),
            "ad-hoc" => Some(Self::AdHoc),
            "app-store-connect" | "app-store" => Some(Self::AppStore),
            _ => None,
        }
    }
}

/// How profile-only keys are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MergePolicy {
    /// Least privilege: only the requested keys plus the required ones (the default).
    #[default]
    Least,
    /// Copy the profile's keys with wildcards resolved; for re-signing third-party apps.
    ProfileAll,
}

impl MergePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Least => "least",
            Self::ProfileAll => "profile-all",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "least" => Some(Self::Least),
            "profile-all" => Some(Self::ProfileAll),
            _ => None,
        }
    }
}

/// Why a merge failed (SIGN §8.4). Converts to `sign.*` errors. Values and App IDs are rendered with the App ID
/// prefix masked (`*******345.*`), so neither the message nor the details carry a team ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementsError {
    UnresolvedVariable { key: String, var: String },
    NotInProfile { key: String, capability_hint: Option<String> },
    ValueNotAllowed { key: String, requested: String, allowed: String },
    AppIdMismatch { profile: String, bundle: String },
}

impl std::fmt::Display for EntitlementsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnresolvedVariable { key, var } => write!(f, "entitlement {key} uses the unknown variable $({var})"),
            Self::NotInProfile { key, .. } => {
                write!(f, "the provisioning profile does not grant the entitlement {key}")
            }
            Self::ValueNotAllowed { key, requested, allowed } => {
                write!(f, "the profile does not allow {key} = {requested} (it grants {allowed})")
            }
            Self::AppIdMismatch { profile, bundle } => {
                write!(f, "the profile's App ID {profile} does not cover the bundle ID {bundle}")
            }
        }
    }
}

impl std::error::Error for EntitlementsError {}

impl From<EntitlementsError> for Error {
    fn from(e: EntitlementsError) -> Self {
        let msg = e.to_string();
        match e {
            EntitlementsError::UnresolvedVariable { key, var } => {
                Error::new(codes::sign::ENTITLEMENT_UNRESOLVED_VARIABLE, msg)
                    .with_hint("Use $(AppIdentifierPrefix), $(TeamIdentifierPrefix), $(PRODUCT_BUNDLE_IDENTIFIER) or a literal value in the .entitlements file.")
                    .with_details(serde_json::json!({"key": key, "variable": var}))
            }
            EntitlementsError::NotInProfile { key, capability_hint } => {
                let hint = match &capability_hint {
                    Some(c) => format!("Enable the {c} capability for the App ID and recreate the profile (`russet provision`), or remove {key} from the .entitlements file."),
                    None => format!("Remove {key} from the .entitlements file, or enable the matching capability in the developer portal and recreate the profile."),
                };
                Error::new(codes::sign::ENTITLEMENT_NOT_IN_PROFILE, msg)
                    .with_hint(hint)
                    .with_details(serde_json::json!({"key": key, "capability": capability_hint}))
            }
            EntitlementsError::ValueNotAllowed { key, requested, allowed } => {
                Error::new(codes::sign::ENTITLEMENT_VALUE_NOT_ALLOWED, msg)
                    .with_hint("Change the value in the .entitlements file to one the profile grants, or recreate the profile with the capability configured.")
                    .with_details(serde_json::json!({"key": key, "requested": requested, "allowed": allowed}))
            }
            EntitlementsError::AppIdMismatch { profile, bundle } => Error::new(codes::sign::PROFILE_MISMATCH, msg)
                .with_hint("Use a profile whose App ID matches the bundle ID (or a wildcard App ID), or fix [app] bundle-id.")
                .with_details(serde_json::json!({"profileAppId": profile, "bundleId": bundle})),
        }
    }
}

/// A non-fatal note about the merge (for example `aps-environment` taken from the profile).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeNote {
    pub key: String,
    pub message: String,
}

/// The entitlements to sign one bundle with.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedEntitlements {
    pub dict: Dictionary,
    pub notes: Vec<MergeNote>,
}

/// Resolves the entitlements of one bundle.
#[derive(Debug, Clone, Copy)]
pub struct EntitlementsResolver<'a> {
    pub profile: &'a ProvisioningProfile,
    pub bundle_id: &'a str,
    pub distribution: Distribution,
    pub policy: MergePolicy,
}

/// Keys the merge always computes itself.
const REQUIRED: &[&str] = &[
    "application-identifier",
    "com.apple.developer.team-identifier",
    "get-task-allow",
    "beta-reports-active",
    "keychain-access-groups",
];

impl EntitlementsResolver<'_> {
    /// Merges `requested` with the profile (SIGN §8.4 steps 1–4).
    pub fn resolve(&self, requested: &Dictionary) -> Result<ResolvedEntitlements, EntitlementsError> {
        let p = &self.profile.entitlements;
        let x = self.profile.app_id_prefix.as_str();
        let t = self.profile.team_id.as_str();
        let b = self.bundle_id;
        let mut notes = Vec::new();

        // 1. Expand variables.
        let mut r = Dictionary::new();
        for (k, v) in requested {
            r.insert(k.clone(), expand(k, v, x, t, b)?);
        }

        // 2. Required keys.
        let app_id = format!("{x}.{b}");
        let profile_app_id = self.profile.application_identifier.as_str();
        if !glob_match(profile_app_id, &app_id) {
            return Err(EntitlementsError::AppIdMismatch { profile: redact_app_id(profile_app_id), bundle: b.into() });
        }
        if let Some(req) = r.get("application-identifier")
            && req.as_string() != Some(app_id.as_str())
        {
            return Err(not_allowed("application-identifier", req, &Value::String(app_id.clone())));
        }
        let mut out = Dictionary::new();
        out.insert("application-identifier".into(), Value::String(app_id.clone()));
        if p.contains_key("com.apple.developer.team-identifier") {
            out.insert("com.apple.developer.team-identifier".into(), Value::String(t.into()));
        }
        let profile_gta = p.get("get-task-allow").and_then(Value::as_boolean).unwrap_or(false);
        let gta = match r.get("get-task-allow") {
            Some(Value::Boolean(true)) if !profile_gta => {
                return Err(not_allowed("get-task-allow", &Value::Boolean(true), &Value::Boolean(false)));
            }
            Some(Value::Boolean(v)) => *v,
            Some(other) => return Err(not_allowed("get-task-allow", other, &Value::Boolean(profile_gta))),
            None => profile_gta,
        };
        out.insert("get-task-allow".into(), Value::Boolean(gta));
        let profile_beta = p.get("beta-reports-active").and_then(Value::as_boolean).unwrap_or(false);
        if self.distribution == Distribution::AppStore && profile_beta {
            out.insert("beta-reports-active".into(), Value::Boolean(true));
        } else if r.get("beta-reports-active").and_then(Value::as_boolean) == Some(true) {
            if !profile_beta {
                return Err(not_allowed("beta-reports-active", &Value::Boolean(true), &Value::Boolean(false)));
            }
            notes.push(MergeNote {
                key: "beta-reports-active".into(),
                message: "dropped: only App Store Connect distribution signs beta-reports-active".into(),
            });
        }
        let profile_kag = p.get("keychain-access-groups");
        match (r.get("keychain-access-groups"), profile_kag) {
            (Some(req), Some(allowed)) => {
                if !allowed_value(req, allowed) {
                    return Err(not_allowed("keychain-access-groups", req, allowed));
                }
                out.insert("keychain-access-groups".into(), req.clone());
            }
            (Some(_), None) => {
                return Err(EntitlementsError::NotInProfile {
                    key: "keychain-access-groups".into(),
                    capability_hint: None,
                });
            }
            (None, Some(allowed)) => {
                // What a device accepted from xtool: the App ID itself, granted by `<TEAM>.*` [V11].
                let default = Value::Array(vec![Value::String(app_id.clone())]);
                if allowed_value(&default, allowed) {
                    out.insert("keychain-access-groups".into(), default);
                }
            }
            (None, None) => {}
        }

        // 3. Every other requested key must be granted.
        for (k, req) in &r {
            if REQUIRED.contains(&k.as_str()) {
                continue;
            }
            let Some(allowed) = p.get(k) else {
                return Err(EntitlementsError::NotInProfile {
                    key: k.clone(),
                    capability_hint: capability_for_entitlement(k).map(|c| c.capability.to_owned()),
                });
            };
            if k == "aps-environment" {
                // The signed value is the profile's (development or production).
                if req != allowed {
                    notes.push(MergeNote {
                        key: k.clone(),
                        message: format!("signed with the profile's value {}", render(allowed)),
                    });
                }
                out.insert(k.clone(), allowed.clone());
                continue;
            }
            if !allowed_value(req, allowed) {
                return Err(not_allowed(k, req, allowed));
            }
            out.insert(k.clone(), req.clone());
        }

        // 4. Profile-only keys are copied only under ProfileAll, with wildcards resolved.
        if self.policy == MergePolicy::ProfileAll {
            for (k, v) in p {
                if out.contains_key(k) || REQUIRED.contains(&k.as_str()) {
                    continue;
                }
                match resolve_wildcards(v, x, t, b) {
                    Some(v) => {
                        out.insert(k.clone(), v);
                    }
                    None => notes.push(MergeNote {
                        key: k.clone(),
                        message: "not copied: the profile grants a bare wildcard that cannot be signed".into(),
                    }),
                }
            }
        }
        Ok(ResolvedEntitlements { dict: out, notes })
    }
}

fn not_allowed(key: &str, requested: &Value, allowed: &Value) -> EntitlementsError {
    EntitlementsError::ValueNotAllowed { key: key.into(), requested: render(requested), allowed: render(allowed) }
}

/// A compact, single-line rendering of a plist value for messages, with App ID prefixes and team IDs masked
/// (`*******345.com.example.app`): errors reach the wire and logs (AGENTS.md §8).
fn render(v: &Value) -> String {
    match v {
        Value::Boolean(b) => b.to_string(),
        Value::String(s) => format!("\"{}\"", redact_app_id(s)),
        Value::Integer(i) => i.to_string(),
        Value::Real(r) => r.to_string(),
        Value::Array(a) => format!("[{}]", a.iter().map(render).collect::<Vec<_>>().join(", ")),
        Value::Dictionary(d) => {
            format!("{{{}}}", d.iter().map(|(k, v)| format!("{k}: {}", render(v))).collect::<Vec<_>>().join(", "))
        }
        Value::Data(d) => format!("<{} bytes>", d.len()),
        Value::Date(_) => "<date>".into(),
        _ => "<value>".into(),
    }
}

/// Whether a requested value is within what the profile grants.
pub fn allowed_value(requested: &Value, allowed: &Value) -> bool {
    match (requested, allowed) {
        (Value::Boolean(r), Value::Boolean(a)) => !r || *a,
        (Value::String(r), Value::String(a)) => glob_match(a, r),
        (Value::String(r), Value::Array(a)) => a.iter().any(|x| x.as_string().is_some_and(|p| glob_match(p, r))),
        (Value::Array(r), Value::Array(_) | Value::String(_)) => r.iter().all(|x| allowed_value(x, allowed)),
        (Value::Dictionary(r), Value::Dictionary(a)) => {
            r.iter().all(|(k, v)| a.get(k).is_some_and(|av| allowed_value(v, av)))
        }
        (r, a) => r == a,
    }
}

/// Expands Xcode variables in every string of `v`.
fn expand(key: &str, v: &Value, x: &str, t: &str, b: &str) -> Result<Value, EntitlementsError> {
    Ok(match v {
        Value::String(s) => Value::String(expand_str(key, s, x, t, b)?),
        Value::Array(a) => Value::Array(a.iter().map(|e| expand(key, e, x, t, b)).collect::<Result<_, _>>()?),
        Value::Dictionary(d) => {
            let mut out = Dictionary::new();
            for (k, e) in d {
                out.insert(k.clone(), expand(key, e, x, t, b)?);
            }
            Value::Dictionary(out)
        }
        other => other.clone(),
    })
}

fn expand_str(key: &str, s: &str, x: &str, t: &str, b: &str) -> Result<String, EntitlementsError> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let start = match (rest.find("$("), rest.find("${")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let close = if rest[start + 1..].starts_with('(') { ')' } else { '}' };
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(close) else {
            return Err(EntitlementsError::UnresolvedVariable { key: key.into(), var: after.into() });
        };
        let var = &after[..end];
        // Xcode accepts modifiers such as `$(PRODUCT_BUNDLE_IDENTIFIER:rfc1034identifier)`; the bundle ID is
        // already a valid identifier, so the modifier changes nothing here.
        let name = var.split(':').next().unwrap_or(var);
        match name {
            "AppIdentifierPrefix" => {
                out.push_str(x);
                out.push('.');
            }
            "TeamIdentifierPrefix" => {
                out.push_str(t);
                out.push('.');
            }
            "PRODUCT_BUNDLE_IDENTIFIER" | "CFBundleIdentifier" => out.push_str(b),
            "DEVELOPMENT_TEAM" => out.push_str(t),
            _ => return Err(EntitlementsError::UnresolvedVariable { key: key.into(), var: var.into() }),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolves `<X>.*`/`<T>.*` wildcards to the bundle; `None` for values that stay a bare wildcard.
fn resolve_wildcards(v: &Value, x: &str, t: &str, b: &str) -> Option<Value> {
    match v {
        Value::String(s) if s == "*" => None,
        Value::String(s) => {
            let resolved = if s == &format!("{x}.*") {
                format!("{x}.{b}")
            } else if s == &format!("{t}.*") {
                format!("{t}.{b}")
            } else {
                s.clone()
            };
            Some(Value::String(resolved))
        }
        Value::Array(a) => {
            let items: Vec<Value> = a.iter().filter_map(|e| resolve_wildcards(e, x, t, b)).collect();
            if items.is_empty() { None } else { Some(Value::Array(items)) }
        }
        other => Some(other.clone()),
    }
}

/// One row of the entitlement → capability map (SIGN §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityInfo {
    /// The App Store Connect `capabilityType`.
    pub capability: &'static str,
    /// Whether free (personal) teams can use it.
    pub free_team: bool,
    /// Whether part of it needs a manual developer-portal step (app groups, iCloud containers).
    pub needs_portal_step: bool,
}

/// The capability an entitlement needs, when the portal manages it (SIGN §6.5). Keys such as
/// `keychain-access-groups` and `get-task-allow` need none.
pub fn capability_for_entitlement(key: &str) -> Option<&'static CapabilityInfo> {
    const fn cap(capability: &'static str, free_team: bool, needs_portal_step: bool) -> CapabilityInfo {
        CapabilityInfo { capability, free_team, needs_portal_step }
    }
    static TABLE: &[(&str, CapabilityInfo)] = &[
        ("com.apple.security.application-groups", cap("APP_GROUPS", true, true)),
        ("aps-environment", cap("PUSH_NOTIFICATIONS", false, false)),
        ("com.apple.developer.associated-domains", cap("ASSOCIATED_DOMAINS", false, false)),
        ("com.apple.developer.icloud-container-identifiers", cap("ICLOUD", false, true)),
        ("com.apple.developer.icloud-services", cap("ICLOUD", false, true)),
        ("com.apple.developer.ubiquity-container-identifiers", cap("ICLOUD", false, true)),
        ("com.apple.developer.ubiquity-kvstore-identifier", cap("ICLOUD", false, false)),
        ("com.apple.developer.in-app-payments", cap("APPLE_PAY", false, false)),
        ("com.apple.developer.healthkit", cap("HEALTHKIT", true, false)),
        ("com.apple.developer.healthkit.access", cap("HEALTHKIT", true, false)),
        ("com.apple.developer.homekit", cap("HOMEKIT", true, false)),
        ("com.apple.developer.networking.networkextension", cap("NETWORK_EXTENSIONS", false, false)),
        ("com.apple.developer.networking.vpn.api", cap("PERSONAL_VPN", false, false)),
        ("com.apple.developer.networking.wifi-info", cap("ACCESS_WIFI_INFORMATION", false, false)),
        ("com.apple.developer.networking.HotspotConfiguration", cap("HOT_SPOT", false, false)),
        ("com.apple.developer.networking.multipath", cap("MULTIPATH", false, false)),
        ("com.apple.developer.nfc.readersession.formats", cap("NFC_TAG_READING", false, false)),
        ("com.apple.developer.siri", cap("SIRIKIT", false, false)),
        ("com.apple.developer.applesignin", cap("APPLE_ID_AUTH", false, false)),
        ("com.apple.developer.game-center", cap("GAME_CENTER", false, false)),
        ("com.apple.developer.default-data-protection", cap("DATA_PROTECTION", true, false)),
        ("com.apple.external-accessory.wireless-configuration", cap("WIRELESS_ACCESSORY_CONFIGURATION", true, false)),
        ("inter-app-audio", cap("INTER_APP_AUDIO", true, false)),
        ("com.apple.developer.ClassKit-environment", cap("CLASSKIT", false, false)),
        (
            "com.apple.developer.authentication-services.autofill-credential-provider",
            cap("AUTOFILL_CREDENTIAL_PROVIDER", false, false),
        ),
        ("com.apple.developer.pass-type-identifiers", cap("WALLET", false, false)),
        ("com.apple.developer.coremedia.hls.low-latency", cap("COREMEDIA_HLS_LOW_LATENCY", false, false)),
    ];
    TABLE.iter().find(|(k, _)| *k == key).map(|(_, c)| c)
}
