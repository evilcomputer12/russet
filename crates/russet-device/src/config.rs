// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! [`DeviceConfig`]: the device plane's settings (ARCHITECTURE §14.3 `[device]`, `[device.ddi]`,
//! `[devices.aliases]`, `[fallback]`; device.md §5.3).
//!
//! The daemon builds it from the global configuration. Until W3's typed `GlobalConfig` carries these sections,
//! [`DeviceConfig::load`] reads them from `<config>/config.toml` itself (unknown keys ignored, missing file →
//! defaults), and [`set_alias`] edits `[devices.aliases]` in place, keeping comments (toml_edit).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use russet_core::Result;
use russet_core::dirs::RussetDirs;
use russet_core::error::{Error, codes};

use crate::tunnel::TunnelConfig;

/// `[device.ddi]` (ARCHITECTURE §14.3) [D 62, D 82].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdiConfig {
    /// The sources `ensure_ddi` tries, in order: `dir`, `imported` (W10), `pmd3-cache`, `pmd3-auto-mount`.
    pub order: Vec<String>,
    /// A DDI root holding `Restore/` and `version.plist` (a copy of a Mac's
    /// `/Library/Developer/DeveloperDiskImages/iOS_DDI`), or a directory that directly holds
    /// `BuildManifest.plist`.
    pub dir: Option<PathBuf>,
    /// The mirror consent for `pmd3-cache` and `pmd3-auto-mount`.
    pub allow_mirror: bool,
    /// Cache Cryptex1 IM4M tickets (W10).
    pub ticket_cache: bool,
}

impl Default for DdiConfig {
    fn default() -> Self {
        Self { order: vec!["dir".into(), "imported".into()], dir: None, allow_mirror: false, ticket_cache: false }
    }
}

/// Device-plane configuration (mux endpoint, tunnel settings, directories, DDI sources, aliases).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    /// `state/`: mount records (`state/devices/<udid>/ddi.json`) and the install ledger live under it.
    pub state_dir: PathBuf,
    /// `runtime/`: forward sockets (`runtime/fwd-<lease>.sock`, ARCHITECTURE §10.4).
    pub runtime_dir: PathBuf,
    /// `data/`: imported DDIs (`data/ddi/…`, W10).
    pub data_dir: PathBuf,
    /// `config/`: `config.toml` (aliases).
    pub config_dir: PathBuf,
    /// `[device] mux`: `auto | usbmuxd:<path> | amds | netmuxd:<addr>`.
    pub mux: String,
    pub tunnel: TunnelConfig,
    /// `[device] keep-warm`: an idle tunnel closes after this long (default 10 min).
    pub keep_warm: Duration,
    /// `[device] lease-grace`: device-bound leases survive a disconnect this long (default 2 s).
    pub lease_grace: Duration,
    pub ddi: DdiConfig,
    /// `[devices.aliases]`: alias → UDID.
    pub aliases: BTreeMap<String, String>,
    /// `[fallback] pymobiledevice3`: `auto | off | <path>` (W10's fallback runner).
    pub pymobiledevice3: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::new(),
            runtime_dir: PathBuf::new(),
            data_dir: PathBuf::new(),
            config_dir: PathBuf::new(),
            mux: "auto".into(),
            tunnel: TunnelConfig::default(),
            keep_warm: Duration::from_secs(600),
            lease_grace: Duration::from_secs(2),
            ddi: DdiConfig::default(),
            aliases: BTreeMap::new(),
            pymobiledevice3: "auto".into(),
        }
    }
}

impl DeviceConfig {
    /// Defaults with the directories of `dirs`.
    pub fn for_dirs(dirs: &RussetDirs) -> Self {
        Self {
            state_dir: dirs.state.clone(),
            runtime_dir: dirs.runtime.clone(),
            data_dir: dirs.data.clone(),
            config_dir: dirs.config.clone(),
            tunnel: TunnelConfig {
                pcap: std::env::var_os("RUSSET_TUNNEL_PCAP").map(PathBuf::from),
                ..Default::default()
            },
            ..Self::default()
        }
    }

    /// [`Self::for_dirs`] plus the device sections of `<config>/config.toml`.
    pub fn load(dirs: &RussetDirs) -> Result<Self> {
        let mut cfg = Self::for_dirs(dirs);
        let path = dirs.config.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => cfg.apply_toml(&text, &path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(Error::new(codes::core::IO, format!("cannot read {}: {e}", path.display())).caused_by(e));
            }
        }
        Ok(cfg)
    }

    /// Applies the `[device]`, `[device.ddi]`, `[devices.aliases]` and `[fallback]` sections of a config file.
    pub fn apply_toml(&mut self, text: &str, path: &Path) -> Result<()> {
        let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
            Error::new(codes::config::INVALID, format!("{}: {e}", path.display()))
                .with_hint("Fix the TOML syntax, or run `russet config edit`.")
        })?;
        let bad = |key: &str, want: &str| {
            Error::new(codes::config::INVALID, format!("{}: `{key}` must be {want}", path.display()))
                .with_hint("See ARCHITECTURE §14.3 for the device settings.")
        };
        if let Some(device) = doc.get("device").and_then(|v| v.as_table()) {
            if let Some(v) = device.get("mux") {
                self.mux = v.as_str().ok_or_else(|| bad("device.mux", "a string"))?.to_owned();
            }
            if let Some(v) = device.get("keep-warm") {
                self.keep_warm = v
                    .as_str()
                    .and_then(parse_duration)
                    .ok_or_else(|| bad("device.keep-warm", "a duration like \"10m\""))?;
            }
            if let Some(v) = device.get("lease-grace") {
                self.lease_grace = v
                    .as_str()
                    .and_then(parse_duration)
                    .ok_or_else(|| bad("device.lease-grace", "a duration like \"2s\""))?;
            }
            if let Some(ddi) = device.get("ddi").and_then(|v| v.as_table()) {
                if let Some(v) = ddi.get("order") {
                    let arr = v.as_array().ok_or_else(|| bad("device.ddi.order", "an array of strings"))?;
                    self.ddi.order = arr
                        .iter()
                        .map(|x| x.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| bad("device.ddi.order", "an array of strings"))?;
                }
                if let Some(v) = ddi.get("dir") {
                    let s = v.as_str().ok_or_else(|| bad("device.ddi.dir", "a path string"))?;
                    self.ddi.dir = (!s.trim().is_empty()).then(|| expand_home(s));
                }
                if let Some(v) = ddi.get("allow-mirror") {
                    self.ddi.allow_mirror =
                        v.as_bool().ok_or_else(|| bad("device.ddi.allow-mirror", "true or false"))?;
                }
                if let Some(v) = ddi.get("ticket-cache") {
                    self.ddi.ticket_cache =
                        v.as_bool().ok_or_else(|| bad("device.ddi.ticket-cache", "true or false"))?;
                }
            }
        }
        if let Some(aliases) = doc.get("devices").and_then(|v| v.as_table()).and_then(|d| d.get("aliases")) {
            let t = aliases.as_table().ok_or_else(|| bad("devices.aliases", "a table"))?;
            for (k, v) in t {
                let udid = v.as_str().ok_or_else(|| bad(&format!("devices.aliases.{k}"), "a UDID string"))?;
                self.aliases.insert(k.clone(), udid.to_owned());
            }
        }
        if let Some(v) = doc.get("fallback").and_then(|v| v.as_table()).and_then(|f| f.get("pymobiledevice3")) {
            self.pymobiledevice3 = v.as_str().ok_or_else(|| bad("fallback.pymobiledevice3", "a string"))?.to_owned();
        }
        Ok(())
    }

    /// `state/devices/<udid>/`.
    pub fn device_state_dir(&self, udid: &str) -> PathBuf {
        self.state_dir.join("devices").join(udid)
    }
}

/// `~/x` → `$HOME/x`.
fn expand_home(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(rest),
            None => PathBuf::from(s),
        },
        None => PathBuf::from(s),
    }
}

/// `"10m"`, `"2s"`, `"500ms"`, `"1h"`, `"30"` (seconds).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit() && c != '.') {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, "s"),
    };
    let n: f64 = num.parse().ok()?;
    let secs = match unit {
        "ms" => n / 1000.0,
        "s" | "sec" => n,
        "m" | "min" => n * 60.0,
        "h" => n * 3600.0,
        _ => return None,
    };
    (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Serializes this process's read-modify-write edits of config files (two `device.alias.set` calls at once would
/// otherwise each write their own copy and lose the other's alias).
static CONFIG_EDITS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Sets `[devices.aliases] <alias> = "<udid>"` in `<config_dir>/config.toml`, keeping comments and order; an
/// empty `udid` removes the alias. Written atomically (temporary file + rename), one edit at a time.
///
/// Until W3's `ConfigEditor` lands this is russet-device's own editor; it follows the same rules (comments kept, an
/// existing file keeps its mode, a new one is 0600) and is replaced by `config.set` through the editor after W3.
pub fn set_alias(config_dir: &Path, alias: &str, udid: &str) -> Result<PathBuf> {
    if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(crate::error::invalid_argument(format!("alias {alias:?}: use letters, digits, `-` and `_` only")));
    }
    let _edit = CONFIG_EDITS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = config_dir.join("config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "schema = 1\n".to_owned(),
        Err(e) => return Err(Error::new(codes::core::IO, format!("cannot read {}: {e}", path.display())).caused_by(e)),
    };
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        Error::new(codes::config::INVALID, format!("{}: {e}", path.display()))
            .with_hint("Fix the TOML syntax, or run `russet config edit`.")
    })?;
    let devices = doc.entry("devices").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        toml_edit::Item::Table(t)
    });
    let devices = devices
        .as_table_mut()
        .ok_or_else(|| Error::new(codes::config::INVALID, format!("{}: `devices` is not a table", path.display())))?;
    let aliases = devices.entry("aliases").or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let aliases = aliases.as_table_mut().ok_or_else(|| {
        Error::new(codes::config::INVALID, format!("{}: `devices.aliases` is not a table", path.display()))
    })?;
    if udid.is_empty() {
        aliases.remove(alias);
    } else {
        aliases.insert(alias, toml_edit::value(udid));
    }
    write_atomic(&path, doc.to_string().as_bytes())?;
    Ok(path)
}

/// Writes `bytes` to a uniquely named temporary file next to `path` and renames it over `path`. An existing file
/// keeps its mode; a new one is 0600 (Unix). The temporary file is removed when anything fails.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::new(codes::core::IO, format!("cannot create {}: {e}", dir.display())).caused_by(e))?;
    let tmp = dir.join(format!(
        ".{}.{}-{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let io =
        |e: std::io::Error| Error::new(codes::core::IO, format!("cannot write {}: {e}", path.display())).caused_by(e);
    let written = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            opts.mode(0o600);
            let mut f = opts.open(&tmp)?;
            if let Ok(m) = std::fs::metadata(path) {
                f.set_permissions(std::fs::Permissions::from_mode(m.permissions().mode() & 0o7777))?;
            }
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let mut f = opts.open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("2s"), Some(Duration::from_secs(2)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("soon"), None);
    }

    #[test]
    fn the_device_sections_of_config_toml_apply() {
        let text = r#"
schema = 1
[daemon]
idle-timeout = "30m"
[device]
keep-warm = "5m"
mux = "amds"
[device.ddi]
order = ["dir"]
dir = "/srv/ddi/27A266a/iOS"
allow-mirror = false
[devices.aliases]
iphone12 = "00008101-000A1B2C3D4E5F60"
[fallback]
pymobiledevice3 = "off"
"#;
        let mut cfg = DeviceConfig::default();
        cfg.apply_toml(text, Path::new("config.toml")).expect("valid");
        assert_eq!(cfg.keep_warm, Duration::from_secs(300));
        assert_eq!(cfg.mux, "amds");
        assert_eq!(cfg.ddi.order, vec!["dir".to_owned()]);
        assert_eq!(cfg.ddi.dir, Some(PathBuf::from("/srv/ddi/27A266a/iOS")));
        assert_eq!(cfg.aliases.get("iphone12").map(String::as_str), Some("00008101-000A1B2C3D4E5F60"));
        assert_eq!(cfg.pymobiledevice3, "off");
        let err = cfg.apply_toml("[device]\nkeep-warm = 5", Path::new("c.toml")).expect_err("bad type");
        assert_eq!(err.code(), codes::config::INVALID);
    }

    #[test]
    fn aliases_are_set_without_losing_comments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "schema = 1\n# keep me\n[device]\nkeep-warm = \"10m\"\n").expect("seed");
        set_alias(dir.path(), "iphone12", "00008101-000A1B2C3D4E5F60").expect("set");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("# keep me"), "{text}");
        let mut cfg = DeviceConfig::default();
        cfg.apply_toml(&text, &path).expect("valid");
        assert_eq!(cfg.aliases.len(), 1);
        set_alias(dir.path(), "iphone12", "").expect("remove");
        let mut cfg = DeviceConfig::default();
        cfg.apply_toml(&std::fs::read_to_string(&path).expect("read"), &path).expect("valid");
        assert!(cfg.aliases.is_empty());
        assert!(set_alias(dir.path(), "bad alias", "x").is_err());
    }

    #[test]
    fn concurrent_alias_edits_keep_every_alias_and_the_file_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "schema = 1\n").expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        }
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let d = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    set_alias(&d, &format!("phone{i}"), &format!("00008101-00000000000000{i:02}"))
                })
            })
            .collect();
        for t in threads {
            t.join().expect("join").expect("set");
        }
        let mut cfg = DeviceConfig::default();
        cfg.apply_toml(&std::fs::read_to_string(&path).expect("read"), &path).expect("valid");
        assert_eq!(cfg.aliases.len(), 8, "no edit lost: {:?}", cfg.aliases);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777, 0o640);
        }
        // No temporary files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
