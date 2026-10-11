// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The effective configuration: the precedence of ARCHITECTURE §14.1 applied to the global file, the project
//! files, the environment and the built-in defaults. CLI flags (precedence 1) travel in RPC params and are
//! applied by the method that reads them.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::global::GlobalConfig;
use crate::keys;
use crate::load::{Loaded, SourceInfo, SourceScope};
use crate::policy::{AgentsGrant, McpPolicy, agents_grant};
use crate::project::ProjectConfig;

/// `RUSSET_*` environment variables that override configuration keys (precedence 2), as `(variable, key)`.
pub const ENV_OVERRIDES: &[(&str, &str)] = &[("RUSSET_DEVICE", "run.device"), ("RUSSET_LOG", "daemon.log-filter")];

/// The values of [`ENV_OVERRIDES`] in this process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverrides {
    /// `(variable, key, value)` for every override that is set.
    pub values: Vec<(String, String, String)>,
}

impl EnvOverrides {
    /// Reads the overrides from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    /// Reads the overrides through `lookup` (tests pass a map).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let values = ENV_OVERRIDES
            .iter()
            .filter_map(|(var, key)| {
                lookup(var).filter(|v| !v.is_empty()).map(|v| ((*var).to_owned(), (*key).to_owned(), v))
            })
            .collect();
        Self { values }
    }
}

/// A project's files: `russet.toml` and `russet.local.toml`, each validated, plus their layered value.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProject {
    pub root: PathBuf,
    pub committed: Option<Loaded<ProjectConfig>>,
    pub local: Option<Loaded<ProjectConfig>>,
    /// `russet.local.toml` layered over `russet.toml` (except `[agents]`, which only narrows).
    pub merged: ProjectConfig,
}

impl LoadedProject {
    /// The project files, highest precedence first.
    pub fn layers(&self) -> impl Iterator<Item = &Loaded<ProjectConfig>> {
        self.local.iter().chain(self.committed.iter())
    }
}

/// The merged view after ARCHITECTURE §14.1.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The global configuration (defaults filled in).
    pub global: GlobalConfig,
    /// The project configuration (defaults filled in; all defaults when there is no project).
    pub project: ProjectConfig,
    /// The project root, when one was given.
    pub project_root: Option<PathBuf>,
    /// The MCP policy (defaults filled in).
    pub mcp_policy: McpPolicy,
    value: Value,
    layers: Vec<(SourceInfo, Value)>,
    agents_sections: Vec<crate::project::AgentsSection>,
}

impl Effective {
    /// Builds the effective view from its layers.
    pub fn build(
        global: Option<&Loaded<GlobalConfig>>,
        project: Option<&LoadedProject>,
        mcp_policy: Option<&Loaded<McpPolicy>>,
        env: &EnvOverrides,
    ) -> Self {
        let global_value = global.map(|g| g.value.clone()).unwrap_or_default();
        let project_value = project.map(|p| p.merged.clone()).unwrap_or_default();
        let mut value = to_json(&global_value);
        merge_json(&mut value, &to_json(&project_value));
        // Layers, highest precedence first: env, local, project, global, defaults.
        let mut layers: Vec<(SourceInfo, Value)> = Vec::new();
        for (var, key, v) in &env.values {
            if let Ok(segments) = keys::parse(key) {
                set_json(&mut value, &segments, Value::String(v.clone()));
                let mut layer = Value::Object(serde_json::Map::new());
                set_json(&mut layer, &segments, Value::String(v.clone()));
                layers.push((SourceInfo::env(var.clone()), layer));
            }
        }
        if let Some(p) = project {
            for l in p.layers() {
                layers.push((l.source.clone(), toml_to_json(&l.raw)));
            }
        }
        if let Some(g) = global {
            layers.push((g.source.clone(), toml_to_json(&g.raw)));
        }
        let agents_sections = project.map(|p| p.layers().map(|l| l.value.agents.clone()).collect()).unwrap_or_default();
        Self {
            global: global_value,
            project: project_value,
            project_root: project.map(|p| p.root.clone()),
            mcp_policy: mcp_policy.map(|m| m.value.clone()).unwrap_or_default(),
            value,
            layers,
            agents_sections,
        }
    }

    /// The whole merged document (global and project keys, defaults filled in, env applied), as JSON.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Every source that contributed, highest precedence first (ending with the built-in defaults).
    pub fn sources(&self) -> Vec<SourceInfo> {
        let mut out: Vec<SourceInfo> = self.layers.iter().map(|(s, _)| s.clone()).collect();
        out.push(SourceInfo::default_values());
        out
    }

    /// The effective value of `dotted_key` and the sources that define it, highest precedence first (the value
    /// comes from the first). A key no file sets reports the built-in default. `None` when the key is unknown.
    pub fn get(&self, dotted_key: &str) -> russet_core::Result<Option<(Value, Vec<SourceInfo>)>> {
        let segments = keys::parse(dotted_key)?;
        let Some(v) = keys::get_json(&self.value, &segments) else { return Ok(None) };
        let mut sources: Vec<SourceInfo> = self
            .layers
            .iter()
            .filter(|(_, layer)| keys::get_json(layer, &segments).is_some())
            .map(|(s, _)| s.clone())
            .collect();
        if sources.is_empty() {
            sources.push(SourceInfo::default_values());
        }
        Ok(Some((v.clone(), sources)))
    }

    /// What MCP client `client` may do here: `mcp-policy.toml` narrowed by every project file's `[agents]`.
    pub fn agents(&self, client: &str) -> AgentsGrant {
        agents_grant(&self.mcp_policy, client, self.agents_sections.iter())
    }

    /// The project root, if any.
    pub fn root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// Whether a layer of `scope` defines anything.
    pub fn has_layer(&self, scope: SourceScope) -> bool {
        self.layers.iter().any(|(s, _)| s.scope == scope)
    }
}

/// Serializes a config struct to JSON (it cannot fail for these types; an empty object is the fallback).
pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

/// A raw TOML table as JSON (datetimes become strings).
pub(crate) fn toml_to_json(t: &toml::Table) -> Value {
    to_json(t)
}

/// Deep-merges JSON objects; everything else is replaced.
pub(crate) fn merge_json(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge_json(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

fn set_json(root: &mut Value, segments: &[String], v: Value) {
    let Some((last, parents)) = segments.split_last() else { return };
    let mut cur = root;
    for s in parents {
        if !cur.get(s).is_some_and(Value::is_object) {
            if let Some(obj) = cur.as_object_mut() {
                obj.insert(s.clone(), Value::Object(serde_json::Map::new()));
            } else {
                return;
            }
        }
        match cur.get_mut(s) {
            Some(next) => cur = next,
            None => return,
        }
    }
    if let Some(obj) = cur.as_object_mut() {
        obj.insert(last.clone(), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::parse;

    fn loaded<T: serde::de::DeserializeOwned>(scope: SourceScope, path: &str, text: &str) -> Loaded<T> {
        let (value, raw) = parse::<T>(Path::new(path), text).expect(path);
        Loaded { value, raw, source: SourceInfo::file(scope, path) }
    }

    #[test]
    fn precedence_is_env_local_project_global_default() {
        let global = loaded::<GlobalConfig>(SourceScope::Global, "/c/config.toml", "[daemon]\nlog-filter = \"warn\"\n");
        let committed =
            loaded::<ProjectConfig>(SourceScope::Project, "/p/russet.toml", "[run]\ndevice = \"a\"\nconsole = false\n");
        let local = loaded::<ProjectConfig>(SourceScope::Local, "/p/russet.local.toml", "[run]\ndevice = \"b\"\n");
        let mut merged = committed.raw.clone();
        crate::load::merge_tables(&mut merged, &local.raw);
        let merged: ProjectConfig = merged.try_into().expect("merged");
        let project = LoadedProject { root: "/p".into(), committed: Some(committed), local: Some(local), merged };

        let none = EnvOverrides::default();
        let eff = Effective::build(Some(&global), Some(&project), None, &none);
        let (v, src) = eff.get("run.device").expect("key").expect("set");
        assert_eq!(v, "b");
        assert_eq!(src.iter().map(|s| s.scope).collect::<Vec<_>>(), [SourceScope::Local, SourceScope::Project]);
        assert_eq!(eff.get("run.console").expect("key").map(|(v, _)| v), Some(Value::Bool(false)));
        let (v, src) = eff.get("daemon.idle-timeout").expect("key").expect("default");
        assert_eq!((v, src[0].scope), (Value::from("30m"), SourceScope::Default));
        assert_eq!(eff.get("daemon.log-filter").expect("key").map(|(v, _)| v), Some(Value::from("warn")));
        assert_eq!(eff.get("nope.nothing").expect("key"), None);

        let env = EnvOverrides::from_lookup(|v| (v == "RUSSET_DEVICE").then(|| "c".to_owned()));
        let eff = Effective::build(Some(&global), Some(&project), None, &env);
        let (v, src) = eff.get("run.device").expect("key").expect("set");
        assert_eq!(
            (v, src[0].scope, src[0].var.as_deref()),
            (Value::from("c"), SourceScope::Env, Some("RUSSET_DEVICE"))
        );
        assert_eq!(eff.project.run.device, "b", "the typed project view holds the files' value; env is in value()");
    }

    #[test]
    fn a_project_cannot_widen_agents() {
        let policy = loaded::<McpPolicy>(SourceScope::Global, "/c/mcp-policy.toml", "default-tier-max = 1\n");
        let committed = loaded::<ProjectConfig>(SourceScope::Project, "/p/russet.toml", "[agents]\ntier-max = 3\n");
        let local =
            loaded::<ProjectConfig>(SourceScope::Local, "/p/russet.local.toml", "[agents]\ndeny-tools = [\"build\"]\n");
        let merged = committed.value.clone();
        let project = LoadedProject { root: "/p".into(), committed: Some(committed), local: Some(local), merged };
        let eff = Effective::build(None, Some(&project), Some(&policy), &EnvOverrides::default());
        let grant = eff.agents("claude-code");
        assert_eq!(grant.tier_max, 1);
        assert!(grant.deny_tools.contains("build"));
    }
}
