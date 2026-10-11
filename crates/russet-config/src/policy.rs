// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The policy files in the config directory: `mcp-policy.toml` (ARCHITECTURE §14.6) and `provision-policy.toml`
//! (§14.5), plus the narrowing rule for a project's `[agents]` section (§7.5, §14.1).

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::project::AgentsSection;
use crate::value::DurationStr;

/// The highest MCP tier (ARCHITECTURE §11.3).
pub const MAX_TIER: u8 = 3;
/// `session-approval-ttl` may be at most 8 h (ARCHITECTURE §11.3).
pub const MAX_SESSION_APPROVAL_TTL_SECS: u64 = 8 * 3600;

/// `mcp-policy.toml`: per-client MCP tiers and per-tool approval rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[schemars(title = "mcp-policy.toml", description = "MCP tiers and approvals per client (ARCHITECTURE §14.6).")]
pub struct McpPolicy {
    /// The file-format version; only `1` exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    /// The tier of clients without their own entry (default 1; local models 0 [D 60]).
    #[schemars(range(max = 3))]
    pub default_tier_max: u8,
    /// HTTP clients only; at most 8 h (ARCHITECTURE §11.3) [D 67].
    pub session_approval_ttl: DurationStr,
    pub clients: BTreeMap<String, McpClientPolicy>,
    pub tools: BTreeMap<String, McpToolPolicy>,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            schema: None,
            default_tier_max: 1,
            session_approval_ttl: DurationStr::mins(30),
            clients: BTreeMap::new(),
            tools: BTreeMap::new(),
        }
    }
}

/// `[clients.<id>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct McpClientPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = 3))]
    pub tier_max: Option<u8>,
    /// Whether the client may see full UDIDs (ARCHITECTURE §12.2).
    pub reveal_udid: bool,
}

/// `[tools.<name>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct McpToolPolicy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
}

/// `[tools.<name>] approval`: `session` (once per MCP session) or `always` (every call). Tier 3 is always `always`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Session,
    Always,
}

/// A rate limit written as `"<count>/<duration>"`, for example `"1/10m"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RateLimit {
    pub count: u32,
    pub per: DurationStr,
}

impl FromStr for RateLimit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (count, per) = s.split_once('/').ok_or_else(|| format!("invalid rate limit `{s}`: write \"1/10m\""))?;
        let count: u32 = count.trim().parse().map_err(|_| format!("invalid rate limit `{s}`: bad count"))?;
        if count == 0 {
            return Err(format!("invalid rate limit `{s}`: the count must be at least 1"));
        }
        let per: DurationStr = per.trim().parse()?;
        if per.get().is_zero() {
            return Err(format!("invalid rate limit `{s}`: the period must not be zero"));
        }
        Ok(Self { count, per })
    }
}

impl std::fmt::Display for RateLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.count, self.per)
    }
}

impl Serialize for RateLimit {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RateLimit {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RateLimit {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RateLimit".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "\"<count>/<duration>\", for example \"1/10m\".",
            "pattern": "^[0-9]+/(0|([0-9]+(ms|s|m|h|d))+)$"
        })
    }
}

impl McpPolicy {
    /// Checks the rules serde cannot express: tiers ≤ 3 and `session-approval-ttl` ≤ 8 h.
    pub fn validate(&self) -> Result<(), String> {
        if self.default_tier_max > MAX_TIER {
            return Err(format!("default-tier-max = {} is above the highest tier ({MAX_TIER})", self.default_tier_max));
        }
        for (id, c) in &self.clients {
            if let Some(t) = c.tier_max
                && t > MAX_TIER
            {
                return Err(format!("clients.{id}.tier-max = {t} is above the highest tier ({MAX_TIER})"));
            }
        }
        if self.session_approval_ttl.get().as_secs() > MAX_SESSION_APPROVAL_TTL_SECS {
            return Err(format!("session-approval-ttl = \"{}\" is longer than 8h", self.session_approval_ttl));
        }
        Ok(())
    }

    /// The tier a client may reach before any project narrows it.
    pub fn client_tier_max(&self, client: &str) -> u8 {
        self.clients.get(client).and_then(|c| c.tier_max).unwrap_or(self.default_tier_max).min(MAX_TIER)
    }
}

/// What an MCP client may do in one project: the user's policy narrowed by the project's `[agents]` sections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsGrant {
    pub tier_max: u8,
    pub deny_tools: BTreeSet<String>,
    pub reveal_udid: bool,
}

/// The narrowing rule (ARCHITECTURE §7.5, §14.1): `tier-max` is the minimum of the policy's value and every
/// project file's value (`russet.toml` and `russet.local.toml` separately, so the local file cannot widen the
/// committed one either), and `deny-tools` is the union. A project file never raises anything.
pub fn agents_grant<'a>(
    policy: &McpPolicy,
    client: &str,
    project_sections: impl IntoIterator<Item = &'a AgentsSection>,
) -> AgentsGrant {
    let mut tier_max = policy.client_tier_max(client);
    let mut deny_tools = BTreeSet::new();
    for s in project_sections {
        if let Some(t) = s.tier_max {
            tier_max = tier_max.min(t);
        }
        deny_tools.extend(s.deny_tools.iter().cloned());
    }
    let reveal_udid = policy.clients.get(client).is_some_and(|c| c.reveal_udid);
    AgentsGrant { tier_max, deny_tools, reveal_udid }
}

/// `provision-policy.toml` (ARCHITECTURE §14.5): what the provisioning engine may do without asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
#[schemars(
    title = "provision-policy.toml",
    description = "Auto-approval rules for Apple-side provisioning changes (ARCHITECTURE §14.5)."
)]
pub struct ProvisionPolicy {
    /// The file-format version; only `1` exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<u32>,
    pub paid: ProvisionRules,
    pub free: ProvisionRules,
}

impl Default for ProvisionPolicy {
    fn default() -> Self {
        Self { schema: None, paid: ProvisionRules::paid(), free: ProvisionRules::free() }
    }
}

/// One team tier's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProvisionRules {
    pub register_device: PolicyAction,
    pub register_bundle_id: PolicyAction,
    pub enable_capability: PolicyAction,
    pub russet_profiles: PolicyAction,
    pub create_certificate: PolicyAction,
    pub revoke_certificate: PolicyAction,
    pub disable_capability: PolicyAction,
    pub delete_foreign_profile: PolicyAction,
}

impl ProvisionRules {
    /// The defaults for paid teams.
    pub fn paid() -> Self {
        Self {
            register_device: PolicyAction::Auto,
            register_bundle_id: PolicyAction::Auto,
            enable_capability: PolicyAction::Auto,
            russet_profiles: PolicyAction::Auto,
            create_certificate: PolicyAction::AskOnce,
            revoke_certificate: PolicyAction::Typed,
            disable_capability: PolicyAction::Typed,
            delete_foreign_profile: PolicyAction::Typed,
        }
    }

    /// The defaults for free teams.
    pub fn free() -> Self {
        Self {
            register_device: PolicyAction::Ask,
            register_bundle_id: PolicyAction::Ask,
            create_certificate: PolicyAction::Ask,
            ..Self::paid()
        }
    }
}

/// What a provisioning step needs: nothing (`auto`), a confirmation once per machine and team (`ask-once`),
/// a confirmation every time (`ask`), or a typed confirmation (`typed`). Ordered from least to most strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyAction {
    Auto,
    AskOnce,
    Ask,
    Typed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limits_round_trip() {
        let r: RateLimit = "1/10m".parse().expect("rate limit");
        assert_eq!((r.count, r.per.get().as_secs()), (1, 600));
        assert_eq!(r.to_string(), "1/10m");
        for bad in ["", "1", "0/10m", "1/0", "x/10m", "1/ten"] {
            assert!(bad.parse::<RateLimit>().is_err(), "{bad}");
        }
    }

    #[test]
    fn projects_only_narrow_the_policy() {
        let mut policy = McpPolicy::default();
        policy.clients.insert("claude-code".into(), McpClientPolicy { tier_max: Some(2), reveal_udid: true });
        let widen = AgentsSection { tier_max: Some(3), deny_tools: vec!["ship_upload".into()] };
        let g = agents_grant(&policy, "claude-code", [&widen]);
        assert_eq!(g.tier_max, 2, "a project cannot raise the tier");
        assert!(g.deny_tools.contains("ship_upload"));
        assert!(g.reveal_udid);
        let narrow = AgentsSection { tier_max: Some(0), deny_tools: vec![] };
        assert_eq!(agents_grant(&policy, "claude-code", [&widen, &narrow]).tier_max, 0);
        // A client without an entry gets the default tier.
        assert_eq!(agents_grant(&policy, "other", [&widen]).tier_max, 1);
        assert!(!agents_grant(&policy, "other", []).reveal_udid);
    }

    #[test]
    fn policy_validation_rejects_out_of_range_values() {
        let mut p = McpPolicy { default_tier_max: 4, ..McpPolicy::default() };
        assert!(p.validate().is_err());
        p.default_tier_max = 1;
        p.session_approval_ttl = "9h".parse().expect("9h");
        assert!(p.validate().is_err());
        p.session_approval_ttl = "8h".parse().expect("8h");
        assert!(p.validate().is_ok());
    }

    #[test]
    fn free_defaults_are_stricter_than_paid() {
        let (paid, free) = (ProvisionRules::paid(), ProvisionRules::free());
        assert!(free.register_device > paid.register_device);
        assert!(free.register_bundle_id > paid.register_bundle_id);
        assert_eq!(free.revoke_certificate, PolicyAction::Typed);
    }
}
