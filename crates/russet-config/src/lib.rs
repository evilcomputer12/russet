// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Typed configuration, layering, the comment-preserving editor, JSON Schemas and the watcher (ARCHITECTURE
//! §14; core §5.4, §9). [`ConfigService`] is the daemon-side service that `DaemonCtx.config` names [D 86].
//!
//! - Files: `russet.toml` and `russet.local.toml` ([`project`]), the global `config.toml` ([`global`]),
//!   `accounts.toml` ([`accounts`]), `mcp-policy.toml` and `provision-policy.toml` ([`policy`]). Keys are
//!   kebab-case; unknown keys and invalid values are errors with `path:line:col` ([`load`]).
//! - Precedence (§14.1): CLI flags, `RUSSET_*` variables, `russet.local.toml`, `russet.toml`, `config.toml`,
//!   built-in defaults ([`effective`]); a project's `[agents]` only narrows `mcp-policy.toml` ([`policy`]).
//! - Edits keep comments and order and are validated before they are saved ([`editor`]).
//!
//! Layer L2 may not depend on russet-doctor (ARCHITECTURE §5.2), so the configuration checks live in russetd
//! (`handlers/doctor.rs`) [D 100].

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, Result, codes};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

pub mod accounts;
pub mod editor;
pub mod effective;
pub mod global;
pub mod keys;
pub mod load;
pub mod policy;
pub mod project;
pub mod schema;
pub mod value;
pub mod watch;

pub use editor::ConfigEditor;
pub use effective::{Effective, EnvOverrides, LoadedProject};
pub use global::GlobalConfig;
pub use load::{ConfigFile, Loaded, SourceInfo, SourceScope, find_project_root};
pub use policy::{AgentsGrant, McpPolicy, ProvisionPolicy};
pub use project::ProjectConfig;

/// Which file an edit goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigScope {
    /// `config.toml` (and the other files of the config directory).
    Global,
    /// `russet.toml`.
    Project,
    /// `russet.local.toml`.
    Local,
}

/// Which view `config.get` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadScope {
    /// The precedence of ARCHITECTURE §14.1, defaults filled in.
    Effective,
    /// `config.toml` as written.
    Global,
    /// `russet.toml` as written.
    Project,
    /// `russet.local.toml` as written.
    Local,
}

impl From<ConfigScope> for ReadScope {
    fn from(s: ConfigScope) -> Self {
        match s {
            ConfigScope::Global => Self::Global,
            ConfigScope::Project => Self::Project,
            ConfigScope::Local => Self::Local,
        }
    }
}

/// A configuration file changed (`config.changed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChanged {
    pub scope: ConfigScope,
    pub path: PathBuf,
}

/// A candidate file of a scope and whether it exists (`russet config path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    pub scope: ConfigScope,
    pub path: PathBuf,
    pub exists: bool,
}

/// The answer of [`ConfigService::lookup`] (`config.get`).
#[derive(Debug, Clone, PartialEq)]
pub struct Lookup {
    /// The value; `None` when the file of a file scope does not set the key.
    pub value: Option<Value>,
    /// Where the value came from, highest precedence first.
    pub sources: Vec<SourceInfo>,
    /// The candidate files of the scope.
    pub files: Vec<FileInfo>,
}

/// The answer of [`ConfigService::set`] and [`ConfigService::unset`] (`config.set`, `config.unset`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub path: PathBuf,
    pub changed: bool,
}

/// A cached file: its stamp and its parse result (errors are kept so they can be reported again).
#[derive(Debug)]
struct Cached<T> {
    path: PathBuf,
    stamp: Option<load::Stamp>,
    loaded: std::result::Result<Option<Loaded<T>>, Error>,
}

impl<T: DeserializeOwned + Clone> Cached<T> {
    fn read(path: PathBuf, scope: SourceScope) -> Self {
        let stamp = load::Stamp::of(&path);
        let loaded = load::read::<T>(&path, scope);
        Self { path, stamp, loaded }
    }

    fn refresh(&mut self, scope: SourceScope) {
        if load::Stamp::of(&self.path) != self.stamp {
            *self = Self::read(self.path.clone(), scope);
        }
    }

    fn get(&self) -> Result<Option<Loaded<T>>> {
        match &self.loaded {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(clone_error(e)),
        }
    }
}

/// Rebuilds an error with the same code, message, hint and details (errors are not `Clone`).
fn clone_error(e: &Error) -> Error {
    let mut out = Error::new(e.code(), e.message());
    if let Some(h) = e.hint() {
        out = out.with_hint(h);
    }
    if let Some(d) = e.to_data().details {
        out = out.with_details(d);
    }
    out
}

#[derive(Debug)]
struct ProjectCache {
    committed: Cached<ProjectConfig>,
    local: Cached<ProjectConfig>,
}

impl ProjectCache {
    fn read(root: &Path) -> Self {
        Self {
            committed: Cached::read(root.join(load::PROJECT_FILE), SourceScope::Project),
            local: Cached::read(root.join(load::LOCAL_FILE), SourceScope::Local),
        }
    }

    fn refresh(&mut self) {
        self.committed.refresh(SourceScope::Project);
        self.local.refresh(SourceScope::Local);
    }

    fn project(&self, root: &Path) -> Result<LoadedProject> {
        let committed = self.committed.get()?;
        let local = self.local.get()?;
        let mut raw = committed.as_ref().map(|c| c.raw.clone()).unwrap_or_default();
        if let Some(l) = &local {
            load::merge_tables(&mut raw, &l.raw);
        }
        let merged: ProjectConfig = toml::Value::Table(raw).try_into().map_err(|e: toml::de::Error| {
            let path = root.join(load::LOCAL_FILE);
            Error::new(
                codes::config::INVALID,
                format!("{}: russet.local.toml does not combine with russet.toml: {}", path.display(), e.message()),
            )
            .with_hint("Make the overriding values in russet.local.toml the same type as in russet.toml.")
        })?;
        Ok(LoadedProject { root: root.to_path_buf(), committed, local, merged })
    }
}

#[derive(Debug)]
struct Inner {
    global: Cached<GlobalConfig>,
    mcp_policy: Cached<McpPolicy>,
    provision_policy: Cached<ProvisionPolicy>,
    projects: HashMap<PathBuf, ProjectCache>,
    watcher: watch::ConfigWatcher,
}

/// The global configuration, a per-project cache and the watcher (ARCHITECTURE §5.3) [D 86].
///
/// Files are re-read when their modification time or size changes, so the service always answers with what is
/// on disk. An invalid global file does not stop the daemon: [`global`](Self::global) falls back to the
/// defaults, while [`effective`](Self::effective) and `russet doctor` report the error with its position.
#[derive(Debug)]
pub struct ConfigService {
    dirs: RussetDirs,
    env: EnvOverrides,
    inner: Mutex<Inner>,
    tx: broadcast::Sender<ConfigChanged>,
    /// Serializes `set`/`unset`, so two edits never interleave their read-modify-write.
    edits: Mutex<()>,
}

impl ConfigService {
    /// Loads the files of the config directory and starts the watcher. Invalid files are reported later, by the
    /// calls that need them; only an unusable directory layout is an error here.
    pub fn load(dirs: &RussetDirs) -> Result<Self> {
        Self::with_env(dirs, EnvOverrides::from_env())
    }

    /// Like [`load`](Self::load), with explicit environment overrides (tests).
    pub fn with_env(dirs: &RussetDirs, env: EnvOverrides) -> Result<Self> {
        if dirs.config.as_os_str().is_empty() {
            return Err(Error::new(codes::core::INVALID_ARGUMENT, "the config directory is empty")
                .with_hint("Set RUSSET_HOME or RUSSET_CONFIG_DIR."));
        }
        let (tx, _) = broadcast::channel(64);
        let mut watcher = watch::ConfigWatcher::start(tx.clone());
        watcher.watch_dir(&dirs.config);
        let inner = Inner {
            global: Cached::read(dirs.config.join(load::GLOBAL_FILE), SourceScope::Global),
            mcp_policy: Cached::read(dirs.config.join(load::MCP_POLICY_FILE), SourceScope::Global),
            provision_policy: Cached::read(dirs.config.join(load::PROVISION_POLICY_FILE), SourceScope::Global),
            projects: HashMap::new(),
            watcher,
        };
        Ok(Self { dirs: dirs.clone(), env, inner: Mutex::new(inner), tx, edits: Mutex::new(()) })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The directories this service reads.
    pub fn dirs(&self) -> &RussetDirs {
        &self.dirs
    }

    /// The global configuration, defaults filled in. An invalid `config.toml` yields the defaults here; see
    /// [`global_loaded`](Self::global_loaded) for the error.
    pub fn global(&self) -> Arc<GlobalConfig> {
        Arc::new(self.global_loaded().ok().flatten().map(|l| l.value).unwrap_or_default())
    }

    /// `config.toml` as loaded: `Ok(None)` when it does not exist, `Err` with `path:line:col` when invalid.
    pub fn global_loaded(&self) -> Result<Option<Loaded<GlobalConfig>>> {
        let mut inner = self.lock();
        inner.global.refresh(SourceScope::Global);
        inner.global.get()
    }

    /// `mcp-policy.toml` as loaded (validated: tiers ≤ 3, `session-approval-ttl` ≤ 8 h).
    pub fn mcp_policy(&self) -> Result<Option<Loaded<McpPolicy>>> {
        let loaded = {
            let mut inner = self.lock();
            inner.mcp_policy.refresh(SourceScope::Global);
            inner.mcp_policy.get()?
        };
        if let Some(l) = &loaded {
            l.value.validate().map_err(|m| {
                let path = self.dirs.config.join(load::MCP_POLICY_FILE);
                Error::new(codes::config::INVALID, format!("{}: {m}", path.display()))
                    .with_hint("Fix the value; `russet config schema --file mcp-policy` describes the file.")
            })?;
        }
        Ok(loaded)
    }

    /// `provision-policy.toml` as loaded.
    pub fn provision_policy(&self) -> Result<Option<Loaded<ProvisionPolicy>>> {
        let mut inner = self.lock();
        inner.provision_policy.refresh(SourceScope::Global);
        inner.provision_policy.get()
    }

    /// A project's files (`russet.toml` and `russet.local.toml`), validated and layered. A root without project
    /// files yields all defaults.
    pub fn project(&self, root: &Path) -> Result<LoadedProject> {
        let mut inner = self.lock();
        if !inner.projects.contains_key(root) {
            inner.watcher.watch_dir(root);
        }
        let cache = inner.projects.entry(root.to_path_buf()).or_insert_with(|| ProjectCache::read(root));
        cache.refresh();
        cache.project(root)
    }

    /// The effective configuration (ARCHITECTURE §14.1 precedence; a project's `[agents]` only narrows).
    pub fn effective(&self, project: Option<&Path>) -> Result<Effective> {
        let global = self.global_loaded()?;
        let policy = self.mcp_policy()?;
        let project = project.map(|root| self.project(root)).transpose()?;
        Ok(Effective::build(global.as_ref(), project.as_ref(), policy.as_ref(), &self.env))
    }

    /// What MCP client `client` may do in `project`: `mcp-policy.toml` narrowed by the project's `[agents]`.
    pub fn agents(&self, client: &str, project: Option<&Path>) -> Result<AgentsGrant> {
        Ok(self.effective(project)?.agents(client))
    }

    /// The path of `scope`'s file.
    pub fn path_of(&self, scope: ConfigScope, project: Option<&Path>) -> Result<PathBuf> {
        match scope {
            ConfigScope::Global => Ok(self.dirs.config.join(load::GLOBAL_FILE)),
            ConfigScope::Project | ConfigScope::Local => {
                let root = project.ok_or_else(|| {
                    Error::new(codes::config::PROJECT_NOT_FOUND, "this scope needs a project")
                        .with_hint("Run the command inside a project, or pass --project <dir>.")
                })?;
                let name = if scope == ConfigScope::Project { load::PROJECT_FILE } else { load::LOCAL_FILE };
                Ok(root.join(name))
            }
        }
    }

    /// The path of one of the config directory's files.
    pub fn config_file_path(&self, file: ConfigFile) -> PathBuf {
        self.dirs.config.join(file.file_name())
    }

    /// An editor for `scope`'s file; saving it sends `config.changed`.
    pub fn editor(&self, scope: ConfigScope, project: Option<&Path>) -> Result<ConfigEditor> {
        let path = self.path_of(scope, project)?;
        let file = if scope == ConfigScope::Global { ConfigFile::Global } else { ConfigFile::Project };
        Ok(ConfigEditor::open(&path, file, scope)?.with_notifier(self.tx.clone()))
    }

    /// Receives `config.changed` for edits through Russet and, while the watcher runs, for edits on disk.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChanged> {
        self.tx.subscribe()
    }

    /// Whether on-disk edits are watched.
    pub fn watching(&self) -> bool {
        self.lock().watcher.is_active()
    }

    fn files_of(&self, scope: ReadScope, project: Option<&Path>) -> Vec<FileInfo> {
        let mut out = Vec::new();
        let mut push = |scope: ConfigScope, path: PathBuf| {
            let exists = path.is_file();
            out.push(FileInfo { scope, path, exists });
        };
        let wants = |s: ConfigScope| scope == ReadScope::Effective || scope == ReadScope::from(s);
        if let Some(root) = project {
            if wants(ConfigScope::Local) {
                push(ConfigScope::Local, root.join(load::LOCAL_FILE));
            }
            if wants(ConfigScope::Project) {
                push(ConfigScope::Project, root.join(load::PROJECT_FILE));
            }
        }
        if wants(ConfigScope::Global) {
            push(ConfigScope::Global, self.dirs.config.join(load::GLOBAL_FILE));
        }
        out
    }

    /// `config.get`: the effective view, or one file as written, optionally narrowed to `key`.
    pub fn lookup(&self, scope: ReadScope, project: Option<&Path>, key: Option<&str>) -> Result<Lookup> {
        let files = self.files_of(scope, project);
        if scope == ReadScope::Effective {
            let eff = self.effective(project)?;
            return match key {
                None => Ok(Lookup { value: Some(eff.value().clone()), sources: eff.sources(), files }),
                Some(k) => match eff.get(k)? {
                    Some((value, sources)) => Ok(Lookup { value: Some(value), sources, files }),
                    None => Err(unknown_effective_key(k)),
                },
            };
        }
        let write_scope = match scope {
            ReadScope::Global => ConfigScope::Global,
            ReadScope::Project => ConfigScope::Project,
            _ => ConfigScope::Local,
        };
        let path = self.path_of(write_scope, project)?;
        let file = if write_scope == ConfigScope::Global { ConfigFile::Global } else { ConfigFile::Project };
        let source_scope = match write_scope {
            ConfigScope::Global => SourceScope::Global,
            ConfigScope::Project => SourceScope::Project,
            ConfigScope::Local => SourceScope::Local,
        };
        let raw = match file {
            ConfigFile::Global => load::read::<GlobalConfig>(&path, source_scope)?.map(|l| l.raw),
            _ => load::read::<ProjectConfig>(&path, source_scope)?.map(|l| l.raw),
        };
        let sources: Vec<SourceInfo> =
            raw.as_ref().map(|_| SourceInfo::file(source_scope, &path)).into_iter().collect();
        let json = raw.as_ref().map(effective::toml_to_json).unwrap_or_else(|| Value::Object(Default::default()));
        let value = match key {
            None => Some(json),
            Some(k) => {
                let segments = keys::parse(k)?;
                let root_schema = schema::schema(file);
                if schema::schema_at(&root_schema, &segments).is_none() {
                    return Err(unknown_key_in(file, &path, k));
                }
                keys::get_json(&json, &segments).cloned()
            }
        };
        let sources = if value.is_some() { sources } else { Vec::new() };
        Ok(Lookup { value, sources, files })
    }

    /// `config.set`: writes one key of `scope`'s file, validated against its schema.
    pub fn set(&self, scope: ConfigScope, project: Option<&Path>, key: &str, value: Value) -> Result<WriteOutcome> {
        let _edit = self.edits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut editor = self.editor(scope, project)?;
        let edited = editor.set(key, value)?;
        let changed = edited && editor.save()?;
        Ok(WriteOutcome { path: editor.path().to_path_buf(), changed })
    }

    /// `config.unset`: removes one key of `scope`'s file.
    pub fn unset(&self, scope: ConfigScope, project: Option<&Path>, key: &str) -> Result<WriteOutcome> {
        let _edit = self.edits.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut editor = self.editor(scope, project)?;
        let removed = editor.unset(key)?;
        let changed = removed && editor.save()?;
        Ok(WriteOutcome { path: editor.path().to_path_buf(), changed })
    }

    /// `config.schema`: the JSON Schema of one file kind.
    pub fn schema(&self, file: ConfigFile) -> Value {
        schema::schema(file)
    }
}

fn unknown_effective_key(key: &str) -> Error {
    Error::new(codes::config::UNKNOWN_KEY, format!("`{key}` is not a configuration key"))
        .with_hint("`russet config list` shows every key with its effective value.")
        .with_details(serde_json::json!({ "key": key }))
}

fn unknown_key_in(file: ConfigFile, path: &Path, key: &str) -> Error {
    Error::new(codes::config::UNKNOWN_KEY, format!("`{key}` is not a key of {} ({})", file.file_name(), path.display()))
        .with_hint("`russet config schema` lists the valid keys.")
        .with_details(serde_json::json!({ "key": key, "path": path }))
}
