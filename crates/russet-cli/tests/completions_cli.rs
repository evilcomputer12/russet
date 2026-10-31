// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `russet completions` (core §16.1): shell scripts and man pages for the whole tree.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "daemon_support.rs"]
mod support;

use support::Home;

#[test]
fn completions_and_man_pages_cover_the_tree() {
    let home = Home::new("completions");
    for shell in ["bash", "zsh", "fish"] {
        let out = home.run(&["completions", shell]);
        assert!(out.status.success(), "{shell}");
        let script = String::from_utf8_lossy(&out.stdout);
        assert!(script.contains("russet") && script.contains("daemon"), "{shell}: {script}");
    }
    let man = home.root.join("man");
    let v = home.json(&["completions", "--man-dir", man.to_str().unwrap()]);
    assert!(v["pages"].as_array().is_some_and(|p| p.len() > 50));
    for page in ["russet.1", "russet-daemon.1", "russet-daemon-status.1", "russet-approve.1"] {
        assert!(man.join(page).exists(), "{page} missing");
    }
}
