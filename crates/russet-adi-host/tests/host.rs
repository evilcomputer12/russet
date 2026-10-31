// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The helper end to end: a synthetic two-library ADI (tests/fixtures/*.c, compiled here with the system C
//! compiler; skipped without one) loaded by the clean-room loader under the sandbox and driven over JSON Lines:
//! load, provisioning, OTPs (whose library tries a socket, which seccomp must refuse) and erase.

#![allow(clippy::unwrap_used)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

fn compile(src: &str, out: &Path) -> bool {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    Command::new("cc")
        .args(["-shared", "-fPIC", "-O1", "-o"])
        .arg(out)
        .arg(fixtures.join(src))
        .status()
        .is_ok_and(|s| s.success())
}

struct Helper {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Helper {
    fn call(&mut self, req: serde_json::Value) -> serde_json::Value {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{req}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("unreadable answer {line:?}"))
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn a_synthetic_adi_loads_provisions_and_answers_otps_without_network() {
    let tmp = tempfile::tempdir().unwrap();
    let libs = tmp.path().join("libs");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&libs).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    if !compile("fake_storeservicescore.c", &libs.join("libstoreservicescore.so"))
        || !compile("fake_coreadi.c", &libs.join("libCoreADI.so"))
    {
        eprintln!("skipped: no C compiler");
        return;
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_russet-adi-host"))
        .arg("--libs")
        .arg(&libs)
        .arg("--state")
        .arg(&state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut h = Helper { child, out };

    let before = h.call(serde_json::json!({"op": "isProvisioned", "dsid": -2}));
    assert_eq!(before["ok"], false, "nothing is loaded yet: {before}");
    let outside = h.call(serde_json::json!({
        "op": "load", "libDir": "/usr/lib", "androidId": "0123456789abcdef", "provisioningPath": state
    }));
    assert_eq!(outside["ok"], false, "libraries outside --libs are refused: {outside}");
    let load = h.call(serde_json::json!({
        "op": "load", "libDir": libs, "androidId": "0123456789abcdef", "provisioningPath": state
    }));
    assert_eq!(load, serde_json::json!({"ok": true}));
    assert_eq!(h.call(serde_json::json!({"op": "isProvisioned", "dsid": -2}))["provisioned"], false);
    let otp = h.call(serde_json::json!({"op": "requestOtp", "dsid": -2}));
    assert_eq!(otp["adiCode"], -45061, "{otp}");

    let start = h.call(serde_json::json!({"op": "startProvisioning", "dsid": -2, "spim": "c3BpbQ=="}));
    assert_eq!(start["session"], 7, "{start}");
    assert_eq!(start["cpim"], "Y3BpbTpzcGlt"); // "cpim:spim"
    let end = h.call(serde_json::json!({"op": "endProvisioning", "session": 7, "ptm": "cHRt", "tk": "dGs="}));
    assert_eq!(end["ok"], true, "{end}");
    assert_eq!(std::fs::read(state.join("adi.pb")).unwrap(), b"0123456789abcdef");
    assert_eq!(h.call(serde_json::json!({"op": "isProvisioned", "dsid": -2}))["provisioned"], true);

    // The OTP call tries socket(); -99 would mean the sandbox let it through.
    let otp = h.call(serde_json::json!({"op": "requestOtp", "dsid": -2}));
    assert_eq!(otp["ok"], true, "{otp}");
    assert_eq!(otp["mid"], "TUlEIQ=="); // "MID!"
    assert_eq!(otp["otp"], "T1RQIUlEQQ=="); // "OTP" + 0x41444921 (LE), set by libCoreADI's initializer

    assert_eq!(h.call(serde_json::json!({"op": "erase", "dsid": -2}))["ok"], true);
    assert!(!state.join("adi.pb").exists());
    assert_eq!(h.call(serde_json::json!("garbage"))["ok"], false);

    drop(h.child.stdin.take());
    let status = h.child.wait().unwrap();
    assert!(status.success());
    let mut err = String::new();
    std::io::Read::read_to_string(&mut h.child.stderr.take().unwrap(), &mut err).unwrap();
    assert!(err.contains("sandbox seccomp=on"), "{err}");
    assert!(!err.contains("0123456789abcdef"), "the Android ID leaked into stderr: {err}");
}

#[test]
fn bad_arguments_exit_with_usage() {
    let out = Command::new(env!("CARGO_BIN_EXE_russet-adi-host")).output().unwrap();
    assert!(!out.status.success());
}
