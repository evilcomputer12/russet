// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The PLAN §4.3 acceptance facts on the development box, checked against the real host (env-gated: it reads
//! `~/.swiftpm/swift-sdks` and runs the installed Swift, LLDB and linkers; it downloads nothing).
//!
//! ```sh
//! RUSSET_TEST_HOST=1 cargo test -p russet-toolchain --test dev_box -- --nocapture
//! ```

use russet_core::dirs::RussetDirs;
use russet_toolchain::{Component, Provenance, Requirement, Toolchains};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_dev_box_resolves_the_wave_1_toolchain() {
    if std::env::var_os("RUSSET_TEST_HOST").is_none_or(|v| v != "1") {
        eprintln!("skipped: set RUSSET_TEST_HOST=1 to probe this host's toolchains");
        return;
    }
    let home = tempfile::tempdir().expect("home");
    let tc = Toolchains::load(&RussetDirs::under(home.path())).expect("embedded pins");
    let tools = tc.probe_all(true).await;
    for t in &tools {
        eprintln!(
            "{:<10} {:<10} ok={:<5} {:?} {} env={:?} problems={:?}",
            t.component.as_str(),
            t.version,
            t.ok,
            t.provenance,
            t.path.display(),
            t.env,
            t.problems
        );
    }
    let sdks = tc.sdks().await.expect("sdks");
    eprintln!("sdks: {sdks:#?}");

    let swift = tc.resolve(Component::Swift, &Requirement::any()).await.expect("swift");
    assert!(swift.version.starts_with("6.4"), "{swift:?}");
    assert_eq!(swift.provenance, Provenance::System);

    let sdk = sdks.iter().find(|s| s.id == "xcode-27.0-27A266a").expect("the Xcode 27 SDK");
    assert_eq!(sdk.darwin_tools.as_deref(), Some("1.1.0"));
    assert_eq!(sdk.compatible_swift, "6.4");

    let linker = tc.resolve(Component::Linker, &Requirement::any()).await.expect("linker");
    assert_eq!(linker.version, "21.1.6", "{linker:?}");
    assert!(matches!(linker.provenance, Provenance::XtoolBundle { .. }));
    let system_lld = tools
        .iter()
        .find(|t| t.component == Component::Linker && t.provenance == Provenance::System)
        .expect("the system LLD is listed");
    assert!(!system_lld.ok && system_lld.problems.iter().any(|p| p.contains("too old")), "{system_lld:?}");

    for c in [Component::Lldb, Component::LldbDap] {
        let t = tc.resolve(c, &Requirement::any()).await.expect("lldb");
        assert_eq!(t.version, "21.0.0", "{t:?}");
        assert!(t.ok);
        let home = t.env.get("PYTHONHOME").expect("PYTHONHOME");
        assert!(!home.contains(".local/opt/python-3.12"), "never the retired prefix: {home}");
        assert!(home.contains("cpython-3.12") || home.contains("python-shared/3.12"), "{home}");
        let ld = t.env.get("LD_LIBRARY_PATH").expect("LD_LIBRARY_PATH");
        assert!(ld.starts_with(&format!("{home}/lib:")) && ld.contains("usr/lib/swift/linux"), "{ld}");
    }
    let env = tc.resolve_build_env(&Default::default()).await.expect("build env");
    assert_eq!(env.sdk.id, "xcode-27.0-27A266a");
}
