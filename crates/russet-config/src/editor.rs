// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`ConfigEditor`]: comment- and order-preserving edits of one TOML file (toml_edit), validated against the
//! file's schema before anything is written, and saved atomically.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use russet_core::error::{Error, Result, codes};
use serde_json::Value;
use tokio::sync::broadcast;
use toml_edit::{DocumentMut, Item, Table, TableLike};

use crate::accounts::AccountsFile;
use crate::global::GlobalConfig;
use crate::load::{self, ConfigFile};
use crate::policy::{McpPolicy, ProvisionPolicy};
use crate::project::ProjectConfig;
use crate::{ConfigChanged, ConfigScope, keys, schema};

/// A comment- and order-preserving editor over one configuration file.
#[derive(Debug)]
pub struct ConfigEditor {
    path: PathBuf,
    file: ConfigFile,
    scope: ConfigScope,
    doc: DocumentMut,
    original: String,
    existed: bool,
    notify: Option<broadcast::Sender<ConfigChanged>>,
}

fn io_error(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::new(codes::core::IO, format!("cannot {what} {}: {e}", path.display()))
        .with_hint("Check the file's permissions and free space.")
        .caused_by(e)
}

impl ConfigEditor {
    /// Opens `path`, a file of kind `file`; a missing file starts as an empty document.
    pub fn open(path: &Path, file: ConfigFile, scope: ConfigScope) -> Result<Self> {
        let (text, existed) = match std::fs::read_to_string(path) {
            Ok(t) => (t, true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
            Err(e) => return Err(io_error("read", path, e)),
        };
        let doc: DocumentMut = text
            .parse()
            .map_err(|e: toml_edit::TomlError| load::located_error(path, &text, e.span(), e.message(), false))?;
        Ok(Self { path: path.to_path_buf(), file, scope, doc, original: text, existed, notify: None })
    }

    /// Sends `config.changed` on `tx` after a successful save.
    pub fn with_notifier(mut self, tx: broadcast::Sender<ConfigChanged>) -> Self {
        self.notify = Some(tx);
        self
    }

    /// The file being edited.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The scope the file belongs to.
    pub fn scope(&self) -> ConfigScope {
        self.scope
    }

    /// Whether the file existed when it was opened.
    pub fn existed(&self) -> bool {
        self.existed
    }

    /// The document as it would be saved.
    pub fn render(&self) -> String {
        self.doc.to_string()
    }

    /// The value written at `dotted_key` (not the default), as JSON.
    pub fn get(&self, dotted_key: &str) -> Result<Option<Value>> {
        let segments = keys::parse(dotted_key)?;
        let raw: toml::Table = toml::from_str(&self.render()).map_err(|e| {
            Error::new(codes::config::INVALID, format!("{}: {}", self.path.display(), e.message().trim()))
        })?;
        let json = serde_json::to_value(raw)
            .map_err(|e| Error::new(codes::core::INTERNAL, format!("cannot convert TOML to JSON: {e}")))?;
        Ok(keys::get_json(&json, &segments).cloned())
    }

    /// Sets `dotted_key` to `value` (coerced to the schema's type), creating tables as needed. Returns whether the
    /// document changed. A `null` value unsets the key. Nothing is saved until [`save`](Self::save).
    pub fn set(&mut self, dotted_key: &str, value: Value) -> Result<bool> {
        if value.is_null() {
            return self.unset(dotted_key);
        }
        let segments = keys::parse(dotted_key)?;
        self.check_writable(&segments)?;
        let root_schema = schema::schema(self.file);
        let Some(key_schema) = schema::schema_at(&root_schema, &segments) else {
            return Err(self.unknown_key(&root_schema, &segments));
        };
        let value = schema::coerce(&root_schema, key_schema, value);
        let before = self.render();
        let item = json_to_item(&value, true)?;
        let (last, parents) = segments.split_last().ok_or_else(|| {
            Error::new(codes::core::INVALID_ARGUMENT, "empty configuration key").with_hint("Name a key.")
        })?;
        let table = table_at_mut(self.doc.as_table_mut(), parents)?;
        table.insert(last, item);
        self.validate()?;
        Ok(self.render() != before)
    }

    /// Removes `dotted_key`, and the tables it leaves empty. Returns whether the key existed.
    pub fn unset(&mut self, dotted_key: &str) -> Result<bool> {
        let segments = keys::parse(dotted_key)?;
        self.check_writable(&segments)?;
        let removed = remove_at(self.doc.as_table_mut(), &segments);
        if removed {
            self.validate()?;
        }
        Ok(removed)
    }

    /// Validates the whole document against the file's typed schema.
    pub fn validate(&self) -> Result<()> {
        let text = self.render();
        match self.file {
            ConfigFile::Project => load::parse::<ProjectConfig>(&self.path, &text).map(drop),
            ConfigFile::Global => load::parse::<GlobalConfig>(&self.path, &text).map(drop),
            ConfigFile::Accounts => load::parse::<AccountsFile>(&self.path, &text).map(drop),
            ConfigFile::McpPolicy => {
                let (p, _) = load::parse::<McpPolicy>(&self.path, &text)?;
                p.validate().map_err(|m| load::located_error(&self.path, &text, None, &m, false))
            }
            ConfigFile::ProvisionPolicy => load::parse::<ProvisionPolicy>(&self.path, &text).map(drop),
        }
    }

    /// Writes the document atomically (temporary file, fsync, rename), keeping comments and key order. A new
    /// file gets `schema = 1` first. Returns whether the file changed on disk.
    pub fn save(&mut self) -> Result<bool> {
        if !self.existed && !self.doc.as_table().contains_key("schema") && !self.doc.as_table().is_empty() {
            let mut fresh = DocumentMut::new();
            fresh.insert("schema", toml_edit::value(i64::from(load::SCHEMA_VERSION)));
            for (k, v) in self.doc.as_table().iter() {
                fresh.insert(k, v.clone());
            }
            self.doc = fresh;
        }
        self.validate()?;
        let text = self.render();
        if self.existed && text == self.original {
            return Ok(false);
        }
        let private = matches!(self.scope, ConfigScope::Global);
        atomic_write(&self.path, text.as_bytes(), private)?;
        self.original = text;
        self.existed = true;
        if let Some(tx) = &self.notify {
            // No receiver is fine: nobody listens yet.
            let _ = tx.send(ConfigChanged { scope: self.scope, path: self.path.clone() });
        }
        Ok(true)
    }

    fn check_writable(&self, segments: &[String]) -> Result<()> {
        if segments.first().map(String::as_str) == Some("schema") {
            return Err(Error::new(
                codes::config::READ_ONLY,
                format!("`schema` in {} is the file-format version and cannot be changed", self.path.display()),
            )
            .with_hint("Russet writes `schema = 1` itself."));
        }
        Ok(())
    }

    fn unknown_key(&self, root_schema: &Value, segments: &[String]) -> Error {
        // The longest existing prefix tells the user where the typo is.
        let mut known = 0;
        while known < segments.len() && schema::schema_at(root_schema, &segments[..=known]).is_some() {
            known += 1;
        }
        let valid = schema::keys_at(root_schema, &segments[..known]);
        let place =
            if known == 0 { "the top level".to_owned() } else { format!("`{}`", keys::render(&segments[..known])) };
        let hint = if valid.is_empty() {
            "`russet config schema` lists the valid keys.".to_owned()
        } else {
            format!("Valid keys under {place}: {}.", valid.join(", "))
        };
        Error::new(
            codes::config::UNKNOWN_KEY,
            format!("`{}` is not a key of {} ({})", keys::render(segments), self.file.file_name(), self.path.display()),
        )
        .with_hint(hint)
        .with_details(serde_json::json!({ "key": keys::render(segments), "path": self.path }))
    }
}

/// The table at `path` below `root`, creating implicit tables for missing segments.
fn table_at_mut<'a>(root: &'a mut Table, path: &[String]) -> Result<&'a mut dyn TableLike> {
    let mut current: &mut dyn TableLike = root;
    for seg in path {
        if current.get(seg).is_none() {
            let mut t = Table::new();
            t.set_implicit(true);
            current.insert(seg, Item::Table(t));
        }
        let item = current
            .get_mut(seg)
            .ok_or_else(|| Error::new(codes::core::INTERNAL, format!("cannot create the table `{seg}`")))?;
        current = item.as_table_like_mut().ok_or_else(|| {
            Error::new(codes::config::INVALID, format!("`{seg}` is a value, not a table"))
                .with_hint("Unset the value first, or set a key that exists.")
        })?;
    }
    Ok(current)
}

/// Removes the item at `path` and every table that is left empty. Returns whether it existed.
fn remove_at(table: &mut dyn TableLike, path: &[String]) -> bool {
    let Some((first, rest)) = path.split_first() else { return false };
    if rest.is_empty() {
        return table.remove(first).is_some();
    }
    let Some(child) = table.get_mut(first).and_then(Item::as_table_like_mut) else { return false };
    let removed = remove_at(child, rest);
    if removed && child.is_empty() {
        table.remove(first);
    }
    removed
}

/// Converts JSON to a TOML item: objects become tables at the top of an assignment and inline tables inside
/// arrays.
fn json_to_item(v: &Value, top: bool) -> Result<Item> {
    Ok(match v {
        Value::Object(map) if top => {
            let mut t = Table::new();
            for (k, val) in map {
                if !val.is_null() {
                    t.insert(k, json_to_item(val, true)?);
                }
            }
            Item::Table(t)
        }
        other => Item::Value(json_to_value(other)?),
    })
}

fn json_to_value(v: &Value) -> Result<toml_edit::Value> {
    Ok(match v {
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else if let Some(f) = n.as_f64() {
                f.into()
            } else {
                return Err(Error::new(codes::config::INVALID, format!("the number {n} does not fit in TOML")));
            }
        }
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => {
            let mut a = toml_edit::Array::new();
            for i in items {
                a.push(json_to_value(i)?);
            }
            toml_edit::Value::Array(a)
        }
        Value::Object(map) => {
            let mut t = toml_edit::InlineTable::new();
            for (k, val) in map {
                if !val.is_null() {
                    t.insert(k, json_to_value(val)?);
                }
            }
            toml_edit::Value::InlineTable(t)
        }
        Value::Null => {
            return Err(
                Error::new(codes::config::INVALID, "TOML has no null value").with_hint("Unset the key instead.")
            );
        }
    })
}

/// Writes `bytes` to `path` atomically: a temporary file in the same directory, fsync, rename. `private` makes a
/// new file 0600 (Unix); an existing file keeps its mode.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = match std::fs::metadata(path) {
                Ok(m) => m.permissions().mode() & 0o7777,
                Err(_) if private => 0o600,
                Err(_) => 0o644,
            };
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = private;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error("write", path, e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(dir: &Path, name: &str, text: &str, file: ConfigFile) -> ConfigEditor {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("write fixture");
        ConfigEditor::open(&path, file, ConfigScope::Global).expect("open")
    }

    #[test]
    fn edits_keep_comments_and_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = "# Russet global config\nschema = 1\n\n[daemon]\n# keep me\nidle-timeout = \"30m\" # trailing\nmax-build-jobs = 1\n\n[web]\nlisten = \"127.0.0.1:0\"\n";
        let mut e = editor(dir.path(), "config.toml", text, ConfigFile::Global);
        assert!(e.set("daemon.max-build-jobs", Value::from("2")).expect("set"));
        assert!(e.set("device.ddi.dir", Value::from("/ddi/27A266a/iOS")).expect("set nested"));
        assert!(e.save().expect("save"));
        let saved = std::fs::read_to_string(e.path()).expect("read");
        assert!(saved.starts_with("# Russet global config\nschema = 1\n"), "{saved}");
        assert!(saved.contains("# keep me\nidle-timeout = \"30m\" # trailing\nmax-build-jobs = 2\n"), "{saved}");
        assert!(saved.contains("[device.ddi]\ndir = \"/ddi/27A266a/iOS\""), "{saved}");
        assert!(saved.find("[daemon]") < saved.find("[web]"), "order kept: {saved}");
        // Unset removes the key and the tables it empties; comments elsewhere stay.
        assert!(e.unset("device.ddi.dir").expect("unset"));
        assert!(!e.unset("device.ddi.dir").expect("unset twice"));
        e.save().expect("save");
        let saved = std::fs::read_to_string(e.path()).expect("read");
        assert!(!saved.contains("[device"), "{saved}");
        assert!(saved.contains("# keep me"), "{saved}");
        // A round trip without edits is byte-identical.
        let mut again = ConfigEditor::open(e.path(), ConfigFile::Global, ConfigScope::Global).expect("open");
        assert!(!again.save().expect("noop save"));
        assert_eq!(std::fs::read_to_string(e.path()).expect("read"), saved);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_refused_before_saving() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut e = editor(dir.path(), "config.toml", "[daemon]\nmax-build-jobs = 1\n", ConfigFile::Global);
        let err = e.set("daemon.idle-timout", Value::from("5m")).expect_err("typo");
        assert_eq!(err.code(), codes::config::UNKNOWN_KEY);
        assert!(err.hint().is_some_and(|h| h.contains("idle-timeout")), "{:?}", err.hint());
        let err = e.set("daemon.idle-timeout", Value::from("soon")).expect_err("bad duration");
        assert_eq!(err.code(), codes::config::INVALID);
        let err = e.set("schema", Value::from(2)).expect_err("schema");
        assert_eq!(err.code(), codes::config::READ_ONLY);
        assert_eq!(std::fs::read_to_string(e.path()).expect("read"), "[daemon]\nmax-build-jobs = 1\n");
    }

    #[test]
    fn new_files_start_with_the_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("russet.local.toml");
        let mut e = ConfigEditor::open(&path, ConfigFile::Project, ConfigScope::Local).expect("open");
        assert!(!e.existed());
        e.set("signing.targets.\"PlugIns/W.appex\".profile", Value::from("w.mobileprovision")).expect("set");
        e.set("app.bundle-id", Value::from("com.example.hello")).expect("set");
        e.save().expect("save");
        let saved = std::fs::read_to_string(&path).expect("read");
        assert!(saved.starts_with("schema = 1\n"), "{saved}");
        assert!(saved.contains("[signing.targets.\"PlugIns/W.appex\"]\nprofile = \"w.mobileprovision\""), "{saved}");
        let (p, _) = load::parse::<ProjectConfig>(&path, &saved).expect("valid");
        assert_eq!(p.app.bundle_id.as_deref(), Some("com.example.hello"));
    }

    #[cfg(unix)]
    #[test]
    fn new_global_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cfg").join("config.toml");
        let mut e = ConfigEditor::open(&path, ConfigFile::Global, ConfigScope::Global).expect("open");
        e.set("fallback.pymobiledevice3", Value::from("off")).expect("set");
        e.save().expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
