// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! Launch strategies over the device plane (RD-04; RD §5): the launch spec with Russet's default environment,
//! the launched process with its stdio stream, launch channel and exit notice, and installed-app resolution
//! (exact bundle IDs and xtool's `XTL-<prefix>.<id>`).
//!
//! The launch channel (CoreDevice `appservice`, or DVT `processcontrol`) stays open until the broker's
//! `vAttach` has succeeded: xlinux reports that closing it early resumes the app (RD §4.2; device check D2).
//! Dropping [`LaunchChannel`] closes it.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use tokio::io::AsyncRead;

/// The environment Russet adds unless the user overrides it (RD §5.3): unbuffered `print`/`NSLog`, and
/// `os_log` mirrored to stderr for this process.
pub const DEFAULT_ENV: [(&str, &str); 2] = [("NSUnbufferedIO", "YES"), ("OS_ACTIVITY_DT_MODE", "YES")];

/// How to launch (`[run] launch`, `run.start {launch}`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LaunchStrategy {
    /// CoreDevice, falling back to DVT (RD §5.1).
    #[default]
    Auto,
    CoreDevice,
    Dvt,
}

/// One launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub bundle_id: String,
    pub args: Vec<String>,
    /// The final environment ([`merged_env`] applied).
    pub env: BTreeMap<String, String>,
    /// Start stopped before dyld runs (debug sessions).
    pub suspended: bool,
    /// Terminate running instances first.
    pub kill_existing: bool,
    /// Attach CoreDevice stdio (`openstdiosocket`).
    pub console: bool,
    pub strategy: LaunchStrategy,
}

impl LaunchSpec {
    /// A spec with the default environment, `kill_existing` and `console` on.
    pub fn new(bundle_id: impl Into<String>, args: Vec<String>, user_env: &BTreeMap<String, String>) -> Self {
        Self {
            bundle_id: bundle_id.into(),
            args,
            env: merged_env(user_env),
            suspended: false,
            kill_existing: true,
            console: true,
            strategy: LaunchStrategy::Auto,
        }
    }
}

/// Russet's defaults first, the user's values last (they win, RD §5.3).
pub fn merged_env(user: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> =
        DEFAULT_ENV.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    env.extend(user.iter().map(|(k, v)| (k.clone(), v.clone())));
    env
}

/// Keeps the launch channel open while it lives.
pub struct LaunchChannel {
    _guard: Box<dyn Any + Send>,
}

impl LaunchChannel {
    pub fn new<T: Any + Send>(inner: T) -> Self {
        Self { _guard: Box::new(inner) }
    }
}

impl fmt::Debug for LaunchChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchChannel")
    }
}

/// How a process ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// Resolves when the process ends (CoreDevice `monitorprocesstermination`, DVT's termination callback).
pub type ExitWatch = Pin<Box<dyn Future<Output = ProcessExit> + Send>>;

/// The app's stdout and stderr (CoreDevice: one PTY stream with `\r\n` line ends).
pub type StdioStream = Box<dyn AsyncRead + Send + Unpin>;

/// Console lines the device plane already split into entries (`source: stdout`).
pub type ConsoleEntries = tokio::sync::mpsc::Receiver<russet_protocol::common::LogEntry>;

/// A launched process.
pub struct Launched {
    pub pid: u32,
    /// The executable's path on the device, when the launch reported it.
    pub executable: Option<String>,
    /// Raw stdio bytes, split into lines here.
    pub stdio: Option<StdioStream>,
    /// Or console lines as entries, when the device plane splits them itself.
    pub entries: Option<ConsoleEntries>,
    pub channel: Option<LaunchChannel>,
    pub exit: Option<ExitWatch>,
    /// The strategy that was used.
    pub strategy: LaunchStrategy,
}

impl fmt::Debug for Launched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launched")
            .field("pid", &self.pid)
            .field("executable", &self.executable)
            .field("stdio", &self.stdio.is_some())
            .field("entries", &self.entries.is_some())
            .field("channel", &self.channel.is_some())
            .field("exit", &self.exit.is_some())
            .field("strategy", &self.strategy)
            .finish()
    }
}

impl Launched {
    /// A process with only a pid.
    pub fn pid_only(pid: u32) -> Self {
        Self {
            pid,
            executable: None,
            stdio: None,
            entries: None,
            channel: None,
            exit: None,
            strategy: LaunchStrategy::Auto,
        }
    }

    /// The app bundle directory on the device (the executable's parent).
    pub fn device_app_dir(&self) -> Option<String> {
        let exe = self.executable.as_deref()?;
        let (dir, _) = exe.rsplit_once('/')?;
        dir.ends_with(".app").then(|| dir.to_owned())
    }
}

/// An installed app as the debug plane needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAppInfo {
    /// The bundle ID on the device (maybe `XTL-<prefix>.<id>`).
    pub bundle_id: String,
    /// `/private/var/containers/Bundle/Application/<UUID>/X.app`, when known.
    pub path: Option<String>,
    /// `CFBundleExecutable`, when known.
    pub executable: Option<String>,
}

impl InstalledAppInfo {
    /// The executable's device path, when both parts are known.
    pub fn executable_path(&self) -> Option<String> {
        Some(format!("{}/{}", self.path.as_ref()?, self.executable.as_ref()?))
    }
}

/// Finds `wanted` among installed apps: the exact bundle ID, else xtool's `XTL-<prefix>.<wanted>` (RD §5.2).
pub fn resolve_bundle<'a>(apps: &'a [InstalledAppInfo], wanted: &str) -> Option<&'a InstalledAppInfo> {
    apps.iter().find(|a| a.bundle_id == wanted).or_else(|| {
        apps.iter().find(|a| {
            a.bundle_id
                .strip_prefix("XTL-")
                .and_then(|rest| rest.split_once('.'))
                .is_some_and(|(prefix, id)| !prefix.is_empty() && id == wanted)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_env_is_set_and_user_values_win() {
        let env = merged_env(&BTreeMap::new());
        assert_eq!(env.get("NSUnbufferedIO").map(String::as_str), Some("YES"));
        assert_eq!(env.get("OS_ACTIVITY_DT_MODE").map(String::as_str), Some("YES"));
        let user: BTreeMap<String, String> =
            [("OS_ACTIVITY_DT_MODE".to_owned(), "NO".to_owned()), ("FOO".to_owned(), "1".to_owned())].into();
        let env = merged_env(&user);
        assert_eq!(env.get("OS_ACTIVITY_DT_MODE").map(String::as_str), Some("NO"));
        assert_eq!(env.get("FOO").map(String::as_str), Some("1"));
        assert_eq!(env.len(), 3);
        let spec = LaunchSpec::new("dev.russet.test.probe", vec![], &user);
        assert!(spec.kill_existing && spec.console && !spec.suspended);
    }

    #[test]
    fn xtool_prefixed_ids_resolve() {
        let apps = vec![
            InstalledAppInfo { bundle_id: "com.other.app".into(), path: None, executable: None },
            InstalledAppInfo {
                bundle_id: "XTL-2X9R4HXF34.com.example.hello".into(),
                path: Some("/private/var/containers/Bundle/Application/1C2/Hello.app".into()),
                executable: Some("Hello".into()),
            },
        ];
        let found = resolve_bundle(&apps, "com.example.hello");
        assert_eq!(found.map(|a| a.bundle_id.as_str()), Some("XTL-2X9R4HXF34.com.example.hello"));
        assert_eq!(
            found.and_then(InstalledAppInfo::executable_path).as_deref(),
            Some("/private/var/containers/Bundle/Application/1C2/Hello.app/Hello")
        );
        assert_eq!(resolve_bundle(&apps, "com.other.app").map(|a| a.bundle_id.as_str()), Some("com.other.app"));
        assert!(resolve_bundle(&apps, "example.hello").is_none());
    }

    #[test]
    fn the_app_dir_comes_from_the_executable() {
        let mut l = Launched::pid_only(7);
        assert_eq!(l.device_app_dir(), None);
        l.executable = Some("/private/var/containers/Bundle/Application/1C2/Probe.app/Probe".into());
        assert_eq!(l.device_app_dir().as_deref(), Some("/private/var/containers/Bundle/Application/1C2/Probe.app"));
        l.executable = Some("/usr/bin/sleep".into());
        assert_eq!(l.device_app_dir(), None);
    }
}
