// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config` handlers (ARCHITECTURE §8.1, §14). Owner: W3.
//!
//! The handlers are thin: [`get`], [`set`], [`unset`] and [`schema`] work on a [`ConfigService`] alone, so they
//! are tested without a daemon (`tests/config_handlers.rs`). `config.changed` is produced by
//! `ConfigService::subscribe`; the daemon core (W2) forwards each [`changed_params`] to every connection.

use std::path::{Path, PathBuf};

use russet_config::{ConfigFile, ConfigScope, ConfigService, ReadScope, SourceScope};
use russet_core::error::{Error, Result, codes};
use russet_ipc::Router;
use russet_protocol::config as p;

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ConfigGet, _, _>(|cx: DaemonCtx, _rq, params| async move { get(&cx.config, params) });
    r.request::<p::ConfigSet, _, _>(|cx: DaemonCtx, _rq, params| async move { set(&cx.config, params) });
    r.request::<p::ConfigUnset, _, _>(|cx: DaemonCtx, _rq, params| async move { unset(&cx.config, params) });
    r.request::<p::ConfigSchema, _, _>(|cx: DaemonCtx, _rq, params| async move { Ok(schema(&cx.config, params)) });
}

/// A project root from the wire: absolute, and an existing directory.
pub fn project_root(project: Option<&str>) -> Result<Option<PathBuf>> {
    let Some(p) = project.filter(|p| !p.is_empty()) else { return Ok(None) };
    let path = PathBuf::from(p);
    if !path.is_absolute() {
        return Err(Error::new(codes::core::INVALID_ARGUMENT, format!("the project path `{p}` is not absolute"))
            .with_hint("Pass an absolute project root (ARCHITECTURE §7.3)."));
    }
    if !path.is_dir() {
        return Err(Error::new(codes::config::PROJECT_NOT_FOUND, format!("no project directory at {p}"))
            .with_hint("Run the command inside a project, or pass --project <dir>."));
    }
    Ok(Some(path))
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn read_scope(s: p::ConfigReadScope) -> ReadScope {
    match s {
        p::ConfigReadScope::Effective => ReadScope::Effective,
        p::ConfigReadScope::Global => ReadScope::Global,
        p::ConfigReadScope::Project => ReadScope::Project,
        p::ConfigReadScope::Local => ReadScope::Local,
    }
}

/// The wire scope of a write.
pub fn write_scope(s: p::ConfigWriteScope) -> ConfigScope {
    match s {
        p::ConfigWriteScope::Global => ConfigScope::Global,
        p::ConfigWriteScope::Project => ConfigScope::Project,
        p::ConfigWriteScope::Local => ConfigScope::Local,
    }
}

fn wire_scope(s: ConfigScope) -> p::ConfigWriteScope {
    match s {
        ConfigScope::Global => p::ConfigWriteScope::Global,
        ConfigScope::Project => p::ConfigWriteScope::Project,
        ConfigScope::Local => p::ConfigWriteScope::Local,
    }
}

fn source_scope(s: SourceScope) -> p::ConfigSourceScope {
    match s {
        SourceScope::Env => p::ConfigSourceScope::Env,
        SourceScope::Local => p::ConfigSourceScope::Local,
        SourceScope::Project => p::ConfigSourceScope::Project,
        SourceScope::Global => p::ConfigSourceScope::Global,
        SourceScope::Default => p::ConfigSourceScope::Default,
    }
}

/// `config.get`.
pub fn get(config: &ConfigService, params: p::ConfigGetParams) -> Result<p::ConfigGetResult> {
    let root = project_root(params.project.as_deref())?;
    let l = config.lookup(read_scope(params.scope), root.as_deref(), params.key.as_deref())?;
    Ok(p::ConfigGetResult {
        value: l.value,
        sources: l
            .sources
            .into_iter()
            .map(|s| p::ConfigSource {
                scope: source_scope(s.scope),
                path: s.path.as_deref().map(path_string),
                var: s.var,
            })
            .collect(),
        files: l
            .files
            .into_iter()
            .map(|f| p::ConfigFileInfo { scope: wire_scope(f.scope), path: path_string(&f.path), exists: f.exists })
            .collect(),
    })
}

/// `config.set` (a missing or `null` value unsets).
pub fn set(config: &ConfigService, params: p::ConfigSetParams) -> Result<p::ConfigSetResult> {
    let root = project_root(params.project.as_deref())?;
    let scope = write_scope(params.scope);
    let out = match params.value {
        Some(v) if !v.is_null() => config.set(scope, root.as_deref(), &params.key, v)?,
        _ => config.unset(scope, root.as_deref(), &params.key)?,
    };
    Ok(p::ConfigSetResult { path: path_string(&out.path), changed: out.changed })
}

/// `config.unset`.
pub fn unset(config: &ConfigService, params: p::ConfigUnsetParams) -> Result<p::ConfigUnsetResult> {
    let root = project_root(params.project.as_deref())?;
    let out = config.unset(write_scope(params.scope), root.as_deref(), &params.key)?;
    Ok(p::ConfigSetResult { path: path_string(&out.path), changed: out.changed })
}

/// `config.schema`.
pub fn schema(config: &ConfigService, params: p::ConfigSchemaParams) -> p::ConfigSchemaResult {
    let file = match params.file {
        p::ConfigFileKind::Project => ConfigFile::Project,
        p::ConfigFileKind::Global => ConfigFile::Global,
        p::ConfigFileKind::Accounts => ConfigFile::Accounts,
        p::ConfigFileKind::McpPolicy => ConfigFile::McpPolicy,
        p::ConfigFileKind::ProvisionPolicy => ConfigFile::ProvisionPolicy,
    };
    p::ConfigSchemaResult { schema: config.schema(file) }
}

/// The `config.changed` params of a change the service reported.
pub fn changed_params(c: &russet_config::ConfigChanged) -> p::ConfigChangedParams {
    p::ConfigChangedParams { scope: wire_scope(c.scope), path: path_string(&c.path) }
}
