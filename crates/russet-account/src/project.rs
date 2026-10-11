// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The signing inputs of a project (ARCHITECTURE §14.2): `[app] bundle-id`, `entitlements`, `[[app.extensions]]`,
//! `[signing] account|team|bundle-id-policy|merge` and `[signing.targets."<rel>"] entitlements`, read from
//! `russet.toml` with `russet.local.toml` on top, and the bundle-ID policy of §16.5.
//!
//! A lenient reader of only these keys, so `provision.plan {project}` works before W3's `ConfigService` (strict
//! validation, comments, schemas) and W5's `BundleIdPolicy` land; it defers to them once they do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use russet_core::Result;
use russet_core::error::{Error, codes};
use russet_sign::Distribution;

use crate::provision::{BundleSpec, ProvisionRequest};
use crate::store::Account;

/// `[signing] bundle-id-policy` (ARCHITECTURE §16.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleIdPolicySpec {
    /// Exact for paid teams (team-suffix for free teams, W22).
    Auto,
    Exact,
    TeamSuffix,
    /// `XTL-<prefix>.<id>`: the IDs xtool registered, to update apps it installed.
    XtoolPrefix,
    Custom(BTreeMap<String, String>),
}

impl BundleIdPolicySpec {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Exact => "exact",
            Self::TeamSuffix => "team-suffix",
            Self::XtoolPrefix => "xtool-prefix",
            Self::Custom(_) => "custom",
        }
    }
}

/// One `[[app.extensions]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSpec {
    pub product: String,
    pub bundle_id_suffix: String,
    pub entitlements: Option<PathBuf>,
}

impl ExtensionSpec {
    /// Where the build plane puts it.
    pub fn rel_path(&self) -> String {
        format!("PlugIns/{}.appex", self.product)
    }
}

/// The signing inputs of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSigning {
    pub dir: PathBuf,
    /// The canonical `[app] bundle-id`.
    pub bundle_id: String,
    pub policy: BundleIdPolicySpec,
    pub account: Option<String>,
    pub team: Option<String>,
    pub merge: Option<String>,
    pub app_entitlements: Option<PathBuf>,
    pub extensions: Vec<ExtensionSpec>,
    /// `[signing.targets."<rel>"] entitlements`, which override the app and extension entries.
    pub target_entitlements: BTreeMap<String, PathBuf>,
}

fn invalid(path: &Path, why: impl std::fmt::Display) -> Error {
    Error::new(codes::config::INVALID, format!("{}: {why}", path.display()))
        .with_hint("Fix the file; ARCHITECTURE §14.2 lists the keys (`russet config` validates them once W3 lands).")
}

fn read_toml(path: &Path) -> Result<Option<toml::Table>> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<toml::Table>(&text).map(Some).map_err(|e| invalid(path, e.message())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::new(codes::core::IO, format!("cannot read {}: {e}", path.display()))),
    }
}

/// Deep-merges `over` into `base` (tables merge key by key; everything else replaces).
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

fn get<'a>(t: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (last, head) = path.split_last()?;
    let mut cur = t;
    for k in head {
        cur = cur.get(*k)?.as_table()?;
    }
    cur.get(*last)
}

fn string(t: &toml::Table, path: &[&str]) -> Option<String> {
    get(t, path).and_then(toml::Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

impl ProjectSigning {
    /// Reads `<project>/russet.toml` and `<project>/russet.local.toml`.
    pub fn read(project: &Path) -> Result<Self> {
        let main = project.join("russet.toml");
        let Some(mut table) = read_toml(&main)? else {
            return Err(Error::new(
                codes::config::PROJECT_NOT_FOUND,
                format!("{} has no russet.toml", project.display()),
            )
            .with_hint("Run Russet from the project root (or pass --project); `russet init` creates russet.toml."));
        };
        let local = project.join("russet.local.toml");
        if let Some(over) = read_toml(&local)? {
            merge(&mut table, over);
        }
        let bundle_id = string(&table, &["app", "bundle-id"]).ok_or_else(|| invalid(&main, "no [app] bundle-id"))?;
        let policy = match get(&table, &["signing", "bundle-id-policy"]) {
            None => BundleIdPolicySpec::Auto,
            Some(toml::Value::String(s)) => match s.as_str() {
                "auto" | "" => BundleIdPolicySpec::Auto,
                "exact" => BundleIdPolicySpec::Exact,
                "team-suffix" => BundleIdPolicySpec::TeamSuffix,
                "xtool-prefix" => BundleIdPolicySpec::XtoolPrefix,
                other => return Err(invalid(&main, format!("unknown bundle-id-policy `{other}`"))),
            },
            Some(toml::Value::Table(t)) => {
                let map = t
                    .get("custom")
                    .and_then(toml::Value::as_table)
                    .ok_or_else(|| invalid(&main, "bundle-id-policy table needs `custom`"))?;
                BundleIdPolicySpec::Custom(
                    map.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect(),
                )
            }
            Some(_) => return Err(invalid(&main, "bundle-id-policy must be a string or { custom = { … } }")),
        };
        let rel = |s: String| -> PathBuf { project.join(s) };
        let extensions = get(&table, &["app", "extensions"])
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(toml::Value::as_table)
                    .filter_map(|e| {
                        let product = e.get("product")?.as_str()?.to_owned();
                        let suffix = e
                            .get("bundle-id-suffix")
                            .and_then(toml::Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| product.to_lowercase());
                        let entitlements = e.get("entitlements").and_then(toml::Value::as_str).map(|s| project.join(s));
                        Some(ExtensionSpec { product, bundle_id_suffix: suffix, entitlements })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let target_entitlements = get(&table, &["signing", "targets"])
            .and_then(toml::Value::as_table)
            .map(|t| {
                t.iter()
                    .filter_map(|(k, v)| {
                        let e = v.as_table()?.get("entitlements")?.as_str()?;
                        Some((if k == "app" { String::new() } else { k.clone() }, project.join(e)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            dir: project.to_path_buf(),
            bundle_id,
            policy,
            account: string(&table, &["signing", "account"]),
            team: string(&table, &["signing", "team"]),
            merge: string(&table, &["signing", "merge"]),
            app_entitlements: string(&table, &["app", "entitlements"]).map(rel),
            extensions,
            target_entitlements,
        })
    }

    /// The effective bundle ID of `canonical` under the policy (ARCHITECTURE §16.5).
    pub fn effective_bundle_id(
        &self,
        canonical: &str,
        account: Option<&Account>,
        team: Option<&str>,
    ) -> Result<String> {
        match &self.policy {
            BundleIdPolicySpec::Auto | BundleIdPolicySpec::Exact => Ok(canonical.to_owned()),
            BundleIdPolicySpec::TeamSuffix => {
                let team = team.ok_or_else(|| {
                    Error::new(codes::account::NO_ACCOUNT, "the team-suffix bundle-ID policy needs a known team")
                        .with_hint("Set [signing] team, or run `russet account verify` to discover it.")
                })?;
                Ok(format!("{canonical}.{team}"))
            }
            BundleIdPolicySpec::XtoolPrefix => {
                let prefix = account.and_then(Account::xtool_prefix).ok_or_else(|| {
                    Error::new(
                        codes::account::NO_ACCOUNT,
                        "the xtool-prefix bundle-ID policy needs the account imported from xtool",
                    )
                    .with_hint("Run `russet account import --from-xtool --yes`, or pick another bundle-id-policy.")
                })?;
                Ok(format!("XTL-{prefix}.{canonical}"))
            }
            BundleIdPolicySpec::Custom(map) => Ok(map.get(canonical).cloned().unwrap_or_else(|| canonical.to_owned())),
        }
    }

    /// The provisioning request of the project's app and extensions.
    pub fn request(
        &self,
        account: &Account,
        team: Option<&str>,
        distribution: Distribution,
        devices: Vec<String>,
    ) -> Result<ProvisionRequest> {
        let app_id = self.effective_bundle_id(&self.bundle_id, Some(account), team)?;
        let mut bundles = vec![BundleSpec {
            rel_path: String::new(),
            bundle_id: app_id.clone(),
            requested: self.requested_for("", self.app_entitlements.as_deref())?,
        }];
        for ext in &self.extensions {
            let canonical = format!("{}.{}", self.bundle_id, ext.bundle_id_suffix);
            let id = match &self.policy {
                // Custom maps extensions explicitly; the others keep the app's ID as prefix.
                BundleIdPolicySpec::Custom(map) => map.get(&canonical).cloned().unwrap_or(canonical),
                _ => format!("{app_id}.{}", ext.bundle_id_suffix),
            };
            let rel = ext.rel_path();
            bundles.push(BundleSpec {
                requested: self.requested_for(&rel, ext.entitlements.as_deref())?,
                rel_path: rel,
                bundle_id: id,
            });
        }
        Ok(ProvisionRequest { bundles, distribution, devices, identity: None })
    }

    /// The entitlements file of one bundle (`[signing.targets]` wins), parsed.
    pub fn entitlements_path(&self, rel: &str, default: Option<&Path>) -> Option<PathBuf> {
        self.target_entitlements.get(rel).cloned().or_else(|| default.map(Path::to_path_buf))
    }

    fn requested_for(&self, rel: &str, default: Option<&Path>) -> Result<plist::Dictionary> {
        match self.entitlements_path(rel, default) {
            Some(p) => {
                let v = plist::Value::from_file(&p).map_err(|e| invalid(&p, e))?;
                v.into_dictionary().ok_or_else(|| invalid(&p, "not a dictionary property list"))
            }
            None => Ok(plist::Dictionary::new()),
        }
    }
}
