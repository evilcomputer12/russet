// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Bundle-ID policies (ARCHITECTURE §16.5; build §4.5) [D 18]: the effective `CFBundleIdentifier` of the app and
//! of every extension, and the original → effective map the signing area uses to rewrite entitlements.
//!
//! | Policy | App ID | Extension ID |
//! |---|---|---|
//! | `exact` (paid teams) | `[app] bundle-id` | `<app id>.<suffix>` |
//! | `team-suffix` (free teams) | `<id>.<TEAMID>` | `<effective app id>.<suffix>` |
//! | `xtool-prefix` (opt-in; the Wave-1 slice) | `XTL-<PREFIX>.<id>` | `<effective app id>.<suffix>` |
//! | `custom` | the map's value, else the original | the map's value, else `<effective app id>.<suffix>` |
//!
//! An ID is `[A-Za-z0-9.-]+` with no empty component and no leading or trailing dot, and an extension's ID must
//! start with `<app id>.`; anything else is `build.bundleIdInvalid` (4005).

use std::collections::BTreeMap;

use russet_core::Result;
use russet_core::error::{Error, codes};

use crate::BundleIdPolicy;
use crate::manifest::SigningSection;

/// Original and effective IDs of an app and its extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleIds {
    pub app_original: String,
    pub app: String,
    /// `(product, original, effective)` per extension, in manifest order.
    pub extensions: Vec<(String, String, String)>,
}

impl BundleIds {
    /// Original → effective, for `BuildOutput.bundle_ids` and the signing area.
    pub fn map(&self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert(self.app_original.clone(), self.app.clone());
        for (_, original, effective) in &self.extensions {
            m.insert(original.clone(), effective.clone());
        }
        m
    }

    /// The effective ID of an extension product.
    pub fn extension(&self, product: &str) -> Option<&str> {
        self.extensions.iter().find(|(p, _, _)| p == product).map(|(_, _, e)| e.as_str())
    }
}

/// The policy for a build: the request's when given, else `[signing] bundle-id-policy` (`auto` is `exact` until the
/// free-account path picks `team-suffix`, W22).
pub fn policy_for(requested: Option<&BundleIdPolicy>, signing: &SigningSection) -> Result<BundleIdPolicy> {
    if let Some(p) = requested {
        return Ok(p.clone());
    }
    let invalid = |msg: String| {
        Error::new(codes::config::INVALID, msg)
            .with_hint("`[signing] bundle-id-policy` is auto, exact, team-suffix, xtool-prefix or { custom = { … } }.")
    };
    match &signing.bundle_id_policy {
        None => Ok(BundleIdPolicy::Exact),
        Some(toml::Value::String(s)) => match s.as_str() {
            "auto" | "exact" => Ok(BundleIdPolicy::Exact),
            "team-suffix" => match signing.team.as_deref().filter(|t| !t.is_empty()) {
                Some(team) => Ok(BundleIdPolicy::TeamSuffix { team_id: team.to_owned() }),
                None => Err(Error::new(codes::build::BUNDLE_ID_INVALID, "bundle-id-policy `team-suffix` needs `[signing] team`")
                    .with_hint("Set `[signing] team` in russet.local.toml, or pass the policy with the build request.")),
            },
            "xtool-prefix" => Err(Error::new(
                codes::build::BUNDLE_ID_INVALID,
                "bundle-id-policy `xtool-prefix` needs the identity prefix of the imported xtool account",
            )
            .with_hint(
                "Import the account (`russet account import --from-xtool --yes`); `russet run` and `russet debug` pass the \
                 prefix to the build, and `build.start` accepts it as `bundleIdPolicy {kind: xtool-prefix, prefix}`.",
            )),
            other => Err(invalid(format!("unknown bundle-id-policy `{other}`"))),
        },
        Some(toml::Value::Table(t)) => {
            let Some(toml::Value::Table(map)) = t.get("custom") else {
                return Err(invalid("a table bundle-id-policy must be `{ custom = { \"<id>\" = \"<id>\" } }`".into()));
            };
            let mut out = BTreeMap::new();
            for (k, v) in map {
                let v = v.as_str().ok_or_else(|| invalid(format!("custom bundle ID for `{k}` is not a string")))?;
                out.insert(k.clone(), v.to_owned());
            }
            Ok(BundleIdPolicy::Custom(out))
        }
        Some(other) => Err(invalid(format!("bundle-id-policy cannot be {}", other.type_str()))),
    }
}

/// Checks the bundle-ID syntax.
pub fn validate(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.split('.').all(|c| !c.is_empty())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(Error::new(codes::build::BUNDLE_ID_INVALID, format!("`{id}` is not a valid bundle identifier"))
            .with_hint("Use letters, digits, `-` and `.`, with no empty component (for example `com.example.app`).")
            .with_details(serde_json::json!({"bundleId": id})))
    }
}

/// Applies `policy` to the app ID and to `extensions` (`(product, suffix)` pairs).
pub fn apply(policy: &BundleIdPolicy, app_id: &str, extensions: &[(String, String)]) -> Result<BundleIds> {
    validate(app_id)?;
    let app = match policy {
        BundleIdPolicy::Exact => app_id.to_owned(),
        BundleIdPolicy::TeamSuffix { team_id } => format!("{app_id}.{team_id}"),
        BundleIdPolicy::XtoolPrefix { prefix } => format!("XTL-{}.{app_id}", prefix.to_uppercase()),
        BundleIdPolicy::Custom(map) => map.get(app_id).cloned().unwrap_or_else(|| app_id.to_owned()),
    };
    validate(&app)?;
    let mut out = Vec::with_capacity(extensions.len());
    for (product, suffix) in extensions {
        let original = format!("{app_id}.{suffix}");
        validate(&original)?;
        let effective = match policy {
            BundleIdPolicy::Custom(map) => map.get(&original).cloned().unwrap_or_else(|| format!("{app}.{suffix}")),
            BundleIdPolicy::Exact => original.clone(),
            _ => format!("{app}.{suffix}"),
        };
        validate(&effective)?;
        if !effective.starts_with(&format!("{app}.")) {
            return Err(Error::new(
                codes::build::BUNDLE_ID_INVALID,
                format!("the extension ID `{effective}` does not start with the app ID `{app}.`"),
            )
            .with_hint("An extension's bundle ID must be the app's ID plus a suffix (App Store rule)."));
        }
        out.push((product.clone(), original, effective));
    }
    Ok(BundleIds { app_original: app_id.to_owned(), app, extensions: out })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn ext() -> Vec<(String, String)> {
        vec![("Widget".into(), "widget".into())]
    }

    #[test]
    fn exact_keeps_the_ids() {
        let ids = apply(&BundleIdPolicy::Exact, "com.example.hello", &ext()).unwrap();
        assert_eq!(ids.app, "com.example.hello");
        assert_eq!(ids.extension("Widget"), Some("com.example.hello.widget"));
        assert_eq!(ids.map().len(), 2);
    }

    #[test]
    fn team_suffix_and_xtool_prefix_rewrite_app_and_extensions_consistently() {
        let ids =
            apply(&BundleIdPolicy::TeamSuffix { team_id: "ABCDE12345".into() }, "com.example.hello", &ext()).unwrap();
        assert_eq!(ids.app, "com.example.hello.ABCDE12345");
        assert_eq!(ids.extension("Widget"), Some("com.example.hello.ABCDE12345.widget"));
        assert_eq!(
            ids.map().get("com.example.hello.widget").map(String::as_str),
            Some("com.example.hello.ABCDE12345.widget")
        );
        let ids =
            apply(&BundleIdPolicy::XtoolPrefix { prefix: "f00dcafe".into() }, "com.example.hello", &ext()).unwrap();
        assert_eq!(ids.app, "XTL-F00DCAFE.com.example.hello");
        assert_eq!(ids.extension("Widget"), Some("XTL-F00DCAFE.com.example.hello.widget"));
    }

    #[test]
    fn custom_maps_and_checks_the_prefix_rule() {
        let map = BTreeMap::from([("com.example.hello".to_owned(), "org.mine.hello".to_owned())]);
        let ids = apply(&BundleIdPolicy::Custom(map), "com.example.hello", &ext()).unwrap();
        assert_eq!(ids.app, "org.mine.hello");
        assert_eq!(ids.extension("Widget"), Some("org.mine.hello.widget"));
        let bad = BTreeMap::from([("com.example.hello.widget".to_owned(), "other.widget".to_owned())]);
        let err = apply(&BundleIdPolicy::Custom(bad), "com.example.hello", &ext()).unwrap_err();
        assert_eq!(err.code(), codes::build::BUNDLE_ID_INVALID);
    }

    #[test]
    fn invalid_ids_are_refused() {
        for bad in ["", "a..b", ".a", "a.", "a b", "a_b", "é.a"] {
            assert_eq!(validate(bad).unwrap_err().code(), codes::build::BUNDLE_ID_INVALID, "{bad}");
        }
        validate("XTL-ABC.com.example-1").unwrap();
    }

    #[test]
    fn policies_come_from_the_signing_section() {
        let s =
            |v: &str| SigningSection { bundle_id_policy: Some(toml::Value::String(v.into())), team: Some("T1".into()) };
        assert_eq!(policy_for(None, &SigningSection::default()).unwrap(), BundleIdPolicy::Exact);
        assert_eq!(policy_for(None, &s("auto")).unwrap(), BundleIdPolicy::Exact);
        assert_eq!(policy_for(None, &s("team-suffix")).unwrap(), BundleIdPolicy::TeamSuffix { team_id: "T1".into() });
        assert_eq!(policy_for(None, &s("xtool-prefix")).unwrap_err().code(), codes::build::BUNDLE_ID_INVALID);
        assert_eq!(policy_for(None, &s("weird")).unwrap_err().code(), codes::config::INVALID);
        let requested = BundleIdPolicy::XtoolPrefix { prefix: "AB".into() };
        assert_eq!(policy_for(Some(&requested), &s("xtool-prefix")).unwrap(), requested);
        let custom: toml::Value =
            toml::from_str::<toml::Table>("p = { custom = { \"a.b\" = \"c.d\" } }").unwrap()["p"].clone();
        let c = SigningSection { bundle_id_policy: Some(custom), team: None };
        assert_eq!(
            policy_for(None, &c).unwrap(),
            BundleIdPolicy::Custom(BTreeMap::from([("a.b".to_owned(), "c.d".to_owned())]))
        );
    }
}
