// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `config` handlers (ARCHITECTURE §8.1, §14). Owner: W3.
//!
//! The handlers are thin: [`get`], [`set`], [`unset`] and [`schema`] work on a [`ConfigService`] alone, so they
//! are tested without a daemon (`tests/config_handlers.rs`). `config.changed` comes from
//! `ConfigService::subscribe`: [`forward_changes`] sends each change to every connection through the [`Notify`] the
//! daemon runtime (W2) passes when the service starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russet_config::{ConfigFile, ConfigScope, ConfigService, ReadScope, SourceScope};
use russet_core::error::{Error, Result, codes};
use russet_ipc::Router;
use russet_protocol::NotificationMethod;
use russet_protocol::config as p;
use russet_secrets::SecretStore;
use russet_toolchain::Toolchains;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::DaemonCtx;

/// Registers every method of this namespace (the handler seam of ARCHITECTURE §5.3).
pub fn register(r: &mut Router<DaemonCtx>) {
    r.request::<p::ConfigGet, _, _>(|cx: DaemonCtx, _rq, params| async move { get(&*config_of(&cx)?, params) });
    r.request::<p::ConfigSet, _, _>(|cx: DaemonCtx, _rq, params| async move { set(&*config_of(&cx)?, params) });
    r.request::<p::ConfigUnset, _, _>(|cx: DaemonCtx, _rq, params| async move { unset(&*config_of(&cx)?, params) });
    r.request::<p::ConfigSchema, _, _>(
        |cx: DaemonCtx, _rq, params| async move { Ok(schema(&*config_of(&cx)?, params)) },
    );
}

/// The configuration service of the daemon context.
pub fn config_of(cx: &DaemonCtx) -> Result<Arc<ConfigService>> {
    cx.config.get()
}

/// The secret store of the daemon context.
pub fn secrets_of(cx: &DaemonCtx) -> Result<Arc<dyn SecretStore>> {
    cx.secrets.get()
}

/// The toolchain manager of the daemon context.
pub fn toolchains_of(cx: &DaemonCtx) -> Result<Arc<Toolchains>> {
    cx.toolchains.get()
}

/// Reads a service from its `DaemonCtx` slot. The F0 context holds the services as plain `Arc`s; the daemon
/// runtime (W2) turns the slots into lazily started `Service`s, whose inherent `get()` takes precedence over this
/// trait, so the accessors above compile against both. Delete it once W2 is on main.
#[allow(dead_code)] // unused once the slots are W2's `Service`s
trait Slot<T: ?Sized> {
    fn get(&self) -> Result<Arc<T>>;
}

impl<T: ?Sized> Slot<T> for Arc<T> {
    fn get(&self) -> Result<Arc<T>> {
        Ok(Arc::clone(self))
    }
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

/// Sends one notification (method name, params) to every connection without blocking; the daemon runtime's
/// broadcast (ARCHITECTURE §9.1, "delivered to: all").
pub type Notify = Arc<dyn Fn(&'static str, serde_json::Value) + Send + Sync>;

/// Forwards every item of `rx` as notification `N` until `shutdown` fires or the sender is gone.
pub(crate) fn forward<N, T>(
    mut rx: broadcast::Receiver<T>,
    shutdown: CancellationToken,
    notify: Notify,
    params: impl Fn(&T) -> N::Params + Send + 'static,
) -> tokio::task::JoinHandle<()>
where
    N: NotificationMethod,
    T: Clone + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                () = shutdown.cancelled() => break,
                item = rx.recv() => item,
            };
            match item {
                Ok(v) => match serde_json::to_value(params(&v)) {
                    Ok(json) => notify(N::NAME, json),
                    Err(e) => tracing::warn!(target: "russetd::notify", "cannot encode {}: {e}", N::NAME),
                },
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(target: "russetd::notify", "{} dropped {n} changes in a burst", N::NAME);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// Sends `config.changed` through `notify` for every change of `config` (edits through Russet and, while the
/// watcher runs, edits on disk) until `shutdown`. The daemon runtime calls it once, when it starts the
/// configuration service.
pub fn forward_changes(
    config: &ConfigService,
    shutdown: CancellationToken,
    notify: Notify,
) -> tokio::task::JoinHandle<()> {
    forward::<p::ConfigChanged, _>(config.subscribe(), shutdown, notify, changed_params)
}
