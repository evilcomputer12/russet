// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `TempHome` (the lane-D environment of a test daemon) and the usbmuxd tripwire (ARCHITECTURE §3.3 item 7,
//! §14.9; PLAN W8).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;

use russet_testkit::home::{Backend, SecretsBackend};
use russet_testkit::scenario::default_scenario_path;
use russet_testkit::{TempHome, Tripwire};

fn env_map(home: &TempHome) -> BTreeMap<OsString, OsString> {
    home.env().into_iter().collect()
}

#[test]
fn a_temp_home_carries_the_lane_d_environment() {
    let home = TempHome::new().expect("home");
    let env = env_map(&home);
    let get = |k: &str| env.get(&OsString::from(k)).cloned();
    assert_eq!(get("RUSSET_HOME"), Some(home.root().as_os_str().to_owned()));
    let mut scripted = OsString::from("scripted:");
    scripted.push(default_scenario_path());
    assert_eq!(get("RUSSET_DEVICE_BACKEND"), Some(scripted));
    assert_eq!(get("RUSSET_SECRETS_BACKEND"), Some("memory".into()));
    assert_eq!(get("RUSSET_OFFLINE"), Some("1".into()));
    let lock = get("RUSSET_LIVE_DEVICE_LOCK").expect("a private live-device lock");
    assert!(std::path::Path::new(&lock).starts_with(home.root()), "never the host-wide lock");
    assert!(home.env_remove().contains(&"RUSSET_LANE"), "tests never run as lane A");
    assert!(home.env_remove().contains(&"USBMUXD_SOCKET_ADDRESS"));
    for d in [&home.dirs().config, &home.dirs().data, &home.dirs().state, &home.dirs().cache, &home.dirs().runtime] {
        assert!(d.is_dir() && d.starts_with(home.root()), "{}", d.display());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(d).expect("meta").permissions().mode() & 0o777, 0o700);
        }
    }
    assert_eq!(home.socket_path(), home.dirs().runtime.join("russetd.sock"));
    assert!(home.socket_path().as_os_str().len() < 100, "Unix socket paths stay under 108 bytes");
}

#[test]
fn every_test_gets_its_own_home_and_it_is_removed() {
    let a = TempHome::new().expect("a");
    let b = TempHome::new().expect("b");
    assert_ne!(a.root(), b.root());
    assert_ne!(a.socket_path(), b.socket_path());
    let root = a.root().to_path_buf();
    drop(a);
    assert!(!root.exists(), "a dropped home is deleted");
    let kept = b.keep();
    assert!(kept.is_dir(), "keep() leaves the directory for debugging");
    std::fs::remove_dir_all(kept).expect("cleanup");
}

#[test]
fn builder_options_reach_the_environment() {
    let home = TempHome::builder()
        .backend(Backend::None)
        .secrets(SecretsBackend::Passphrase)
        .no_autostart()
        .env("RUSSET_LOG", "debug")
        .build()
        .expect("home");
    let env = env_map(&home);
    let get = |k: &str| env.get(&OsString::from(k)).cloned();
    assert_eq!(get("RUSSET_DEVICE_BACKEND"), Some("none".into()));
    assert_eq!(get("RUSSET_SECRETS_BACKEND"), Some("passphrase".into()));
    assert!(get("RUSSET_SECRETS_PASSPHRASE").is_some());
    assert!(!home.env_remove().contains(&"RUSSET_SECRETS_PASSPHRASE"), "a variable the home sets is not removed");
    assert_eq!(get("RUSSET_NO_AUTOSTART"), Some("1".into()));
    assert_eq!(get("RUSSET_LOG"), Some("debug".into()));
    assert_eq!(Backend::Replay("/t.rdt.jsonl.zst".into()).env_value(), OsString::from("replay:/t.rdt.jsonl.zst"));
    assert_eq!(Backend::Mock("127.0.0.1:27015".into()).env_value(), OsString::from("mock:127.0.0.1:27015"));
}

#[cfg(unix)]
#[test]
fn a_child_sees_the_home_and_never_the_lane_a_variables() {
    let home = TempHome::new().expect("home");
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "env"]).env("RUSSET_LANE", "A").env("RUSSET_DAEMON", "unix:/elsewhere");
    home.apply(&mut cmd);
    let out = cmd.output().expect("sh");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l == format!("RUSSET_HOME={}", home.root().display())), "{text}");
    assert!(!text.contains("RUSSET_LANE="), "{text}");
    assert!(!text.contains("RUSSET_DAEMON="), "{text}");
}

#[cfg(unix)]
#[tokio::test]
async fn the_tripwire_counts_idevice_connections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tripwire = Tripwire::bind(dir.path()).expect("bind");
    tripwire.assert_untouched().expect("nothing yet");
    let (key, value) = tripwire.env();
    assert_eq!(key, "USBMUXD_SOCKET_ADDRESS");
    // idevice's own usbmuxd client, pointed where USBMUXD_SOCKET_ADDRESS points (ARCHITECTURE §14.9).
    let addr = idevice::usbmuxd::UsbmuxdAddr::UnixSocket(value.to_string_lossy().into_owned());
    let _ = addr.to_socket().await;
    for _ in 0..100 {
        if tripwire.hits() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(tripwire.hits(), 1);
    let err = tripwire.assert_untouched().expect_err("a connection was made");
    assert!(err.contains("talked to the device mux"), "{err}");
}

/// The TCP form Windows uses, exercised on every platform: idevice parses a `host:port` value of
/// `USBMUXD_SOCKET_ADDRESS` into `UsbmuxdAddr::TcpSocket` (idevice 0.1.68 `from_env_var`) and connects there.
#[tokio::test]
async fn the_tcp_tripwire_counts_idevice_connections() {
    let tripwire = Tripwire::bind_tcp().expect("bind");
    let (_, value) = tripwire.env();
    let addr: std::net::SocketAddr = value.to_string_lossy().parse().expect("host:port, as idevice parses it");
    assert!(addr.ip().is_loopback());
    tripwire.assert_untouched().expect("nothing yet");
    let _ = idevice::usbmuxd::UsbmuxdAddr::TcpSocket(addr).to_socket().await;
    for _ in 0..100 {
        if tripwire.hits() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(tripwire.hits(), 1);
    assert!(tripwire.assert_untouched().is_err());
}

#[test]
fn a_home_can_carry_a_tripwire() {
    let home = TempHome::builder().tripwire().build().expect("home");
    let tripwire = home.tripwire().expect("tripwire");
    let env = env_map(&home);
    assert_eq!(env.get(&OsString::from("USBMUXD_SOCKET_ADDRESS")), Some(tripwire.address()));
    assert!(!home.env_remove().contains(&"USBMUXD_SOCKET_ADDRESS"));
}
