// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The resolver over fixture toolchains (PLAN §4.3): `sdk.incompatible`, `toolchain.versionMismatch`,
//! `toolchain.linkerUnsupported`, and the LLDB env contract with its libpython search (RD-05).

mod common;

use std::path::Path;
use std::sync::Arc;

use russet_config::project::ToolchainSection;
use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_toolchain::{Component, Provenance, Requirement, Toolchains};

fn toolchains(home: &Path, host: russet_toolchain::host::HostEnv) -> Toolchains {
    let (manifest, ..) = common::signed_manifest("");
    Toolchains::new(RussetDirs::under(&home.join("rh")), Arc::new(manifest)).with_host(host)
}

#[tokio::test]
async fn a_swift_that_does_not_match_the_sdk_row_is_sdk_incompatible() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = common::swift_toolchain(&tmp.path().join("swift-6.3"), "6.3");
    let sdks = tmp.path().join("sdks");
    common::sdk_bundle(&sdks, "27.0", "27A266a", "1.1.0", "21.1.6");
    let tc = toolchains(tmp.path(), common::host(vec![bin], vec![sdks], tmp.path()));
    let err = tc.resolve_build_env(&ToolchainSection::default()).await.expect_err("6.3 cannot use the 27 SDK");
    assert_eq!(err.code(), codes::sdk::INCOMPATIBLE);
    assert!(err.message().contains("needs Swift 6.4"), "{}", err.message());
    // The SDK itself is listed with the problem.
    let tools = tc.probe_all(false).await;
    let sdk = tools.iter().find(|t| t.component == Component::DarwinSdk).expect("sdk tool");
    assert!(!sdk.ok && sdk.problems[0].contains("needs Swift 6.4"), "{sdk:?}");
}

#[tokio::test]
async fn a_project_pin_that_the_host_does_not_have_is_a_version_mismatch() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = common::swift_toolchain(&tmp.path().join("swift-6.4"), "6.4");
    let sdks = tmp.path().join("sdks");
    common::sdk_bundle(&sdks, "27.0", "27A266a", "1.1.0", "21.1.6");
    let tc = toolchains(tmp.path(), common::host(vec![bin], vec![sdks], tmp.path()));
    let pinned = ToolchainSection { swift: Some("6.5".into()), ..ToolchainSection::default() };
    let err = tc.resolve_build_env(&pinned).await.expect_err("6.5 is pinned");
    assert_eq!(err.code(), codes::toolchain::VERSION_MISMATCH);
    let err = tc.resolve(Component::Swift, &Requirement::version("=6.4.1")).await.expect_err("exact");
    assert_eq!(err.code(), codes::toolchain::VERSION_MISMATCH);
    // The matching pin resolves, with the bundle's darwin-tools linker.
    let env = tc.resolve_build_env(&ToolchainSection { swift: Some("6.4".into()), ..Default::default() }).await;
    let env = env.expect("6.4 resolves");
    assert_eq!(env.sdk.id, "xcode-27.0-27A266a");
    assert_eq!(env.linker.version, "21.1.6");
    assert!(matches!(env.linker.provenance, Provenance::XtoolBundle { .. }));
    let by_build = tc.resolve(Component::DarwinSdk, &Requirement::version("27A266a")).await.expect("by build");
    assert_eq!(by_build.version, "27.0");
    assert!(tc.resolve(Component::DarwinSdk, &Requirement::version("27.0")).await.is_ok());
    assert!(tc.resolve(Component::DarwinSdk, &Requirement::version("26A100")).await.is_err());
    let pinned_sdk = ToolchainSection { xcode_sdk: Some("26.0".into()), ..ToolchainSection::default() };
    assert_eq!(tc.resolve_build_env(&pinned_sdk).await.map_err(|e| e.code()).err(), Some(codes::sdk::MISSING));
}

#[tokio::test]
async fn the_build_env_links_with_the_chosen_sdks_own_darwin_tools() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = common::swift_toolchain(&tmp.path().join("swift-6.4"), "6.4");
    let first = tmp.path().join("sdks-a");
    let second = tmp.path().join("sdks-b");
    common::sdk_bundle(&first, "27.0", "27A266a", "1.1.0", "21.1.6");
    let chosen = common::sdk_bundle(&second, "27.0", "27A300", "1.1.0", "21.1.7");
    let tc = toolchains(tmp.path(), common::host(vec![bin], vec![first, second], tmp.path()));
    let pinned = ToolchainSection { xcode_sdk: Some("27A300".into()), ..ToolchainSection::default() };
    let env = tc.resolve_build_env(&pinned).await.expect("27A300 resolves");
    assert_eq!(env.sdk.id, "xcode-27.0-27A300");
    assert_eq!(env.linker.version, "21.1.7", "{:?}", env.linker);
    assert_eq!(env.linker.provenance, Provenance::XtoolBundle { bundle: chosen });
}

#[tokio::test]
async fn an_old_darwin_tools_bundle_and_an_old_system_lld_are_linker_unsupported() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = common::swift_toolchain(&tmp.path().join("swift-6.4"), "6.4");
    let sdks = tmp.path().join("sdks");
    common::sdk_bundle(&sdks, "27.0", "27A266a", "1.0.0", "21.1.5");
    let sys = tmp.path().join("sysbin");
    common::script(&sys.join("ld64.lld"), "echo 'LLD 23.1.1 (compatible with GNU linkers)'");
    let tc = toolchains(tmp.path(), common::host(vec![bin, sys.clone()], vec![sdks], tmp.path()));
    let err = tc.resolve_build_env(&ToolchainSection::default()).await.expect_err("no usable linker");
    assert_eq!(err.code(), codes::toolchain::LINKER_UNSUPPORTED);
    let tools = tc.probe_all(false).await;
    let linkers: Vec<_> = tools.iter().filter(|t| t.component == Component::Linker).collect();
    assert_eq!(linkers.len(), 2);
    assert!(linkers.iter().all(|l| !l.ok));
    // A new enough system LLD is accepted, and `llvm@>=23.1.3` selects it.
    common::script(&sys.join("ld64.lld"), "echo 'LLD 23.1.3'");
    let tc = toolchains(tmp.path(), common::host(vec![tmp.path().join("swift-6.4/usr/bin"), sys], vec![], tmp.path()));
    let lld = tc.resolve(Component::Linker, &Requirement::flavor("llvm@>=23.1.3")).await.expect("lld 23.1.3");
    assert_eq!((lld.version.as_str(), &lld.provenance), ("23.1.3", &Provenance::System));
    let err = tc.resolve(Component::Linker, &Requirement::flavor("darwin-tools@1.1.0")).await.expect_err("none");
    assert_eq!(err.code(), codes::toolchain::LINKER_UNSUPPORTED);
    // Malformed flavors are configuration errors, never "any version".
    for bad in ["darwin-tools@>=1.1.x", "llvm@latest", "gold"] {
        let err = tc.resolve(Component::Linker, &Requirement::flavor(bad)).await.expect_err(bad);
        assert_eq!(err.code(), codes::config::INVALID, "{bad}");
    }
    let err = tc.resolve(Component::Lldb, &Requirement::flavor("gdb")).await.expect_err("gdb");
    assert_eq!(err.code(), codes::config::INVALID);
}

/// Builds `<root>/usr/lib/liblldb.so` with a real `DT_NEEDED` on `libpython3.12.so.1.0` (needs a C compiler).
fn liblldb_needing_python(root: &Path) -> bool {
    let work = root.join("build");
    std::fs::create_dir_all(&work).expect("work");
    std::fs::create_dir_all(root.join("usr/lib")).expect("lib");
    std::fs::write(work.join("empty.c"), "int russet_test_symbol(void) { return 0; }\n").expect("c");
    let cc = |args: &[&str]| {
        std::process::Command::new("cc").args(args).current_dir(&work).status().is_ok_and(|s| s.success())
    };
    cc(&["-shared", "-fPIC", "-o", "libpython3.12.so.1.0", "-Wl,-soname,libpython3.12.so.1.0", "empty.c"])
        && cc(&[
            "-shared",
            "-fPIC",
            "-o",
            "../usr/lib/liblldb.so.21.0.0",
            "empty.c",
            "-Wl,--no-as-needed",
            "-L.",
            "-l:libpython3.12.so.1.0",
        ])
}

#[tokio::test]
async fn swift_lldb_gets_the_libpython_env_and_never_the_retired_prefix() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("swift-6.4");
    let bin = common::swift_toolchain(&root, "6.4");
    if !liblldb_needing_python(&root) {
        eprintln!("skipped: no C compiler to build the fixture liblldb");
        return;
    }
    // lldb fails to "load" without PYTHONHOME, like Swift's real LLDB without libpython.
    let lldb_body = "[ -n \"$PYTHONHOME\" ] || { echo 'libpython3.12.so.1.0: cannot open shared object file' >&2; exit 127; }\n\
         case \"$*\" in *'platform list'*) echo 'remote-ios: Remote iOS platform plug-in.';; *) echo 'lldb version 21.0.0';; esac";
    common::script(&root.join("usr/bin/lldb"), lldb_body);
    common::script(&root.join("usr/bin/lldb-dap"), "[ -n \"$PYTHONHOME\" ] || exit 127\necho 'LLVM version 21.0.0'");

    let home = tmp.path().join("home");
    let mut host = common::host(vec![bin], vec![], &home);
    // No libpython anywhere: lldb is broken, and the hint names python-shared.
    let tc = toolchains(tmp.path(), host.clone());
    let err = tc.resolve(Component::Lldb, &Requirement::any()).await.expect_err("no libpython");
    assert_eq!(err.code(), codes::toolchain::BROKEN);
    assert!(err.hint().is_some_and(|h| h.contains("python-shared")), "{:?}", err.hint());

    // The retired prefix is ignored even when ldconfig lists it.
    let retired = home.join(".local/opt/python-3.12/lib");
    std::fs::create_dir_all(&retired).expect("retired");
    std::fs::write(retired.join("libpython3.12.so.1.0"), b"").expect("retired lib");
    let ldconfig = tmp.path().join("ldconfig");
    common::script(
        &ldconfig,
        &format!("echo '\tlibpython3.12.so.1.0 (libc6,x86-64) => {}/libpython3.12.so.1.0'", retired.display()),
    );
    host.ldconfig = Some(ldconfig);
    let tc = toolchains(tmp.path(), host.clone());
    assert_eq!(
        tc.resolve(Component::Lldb, &Requirement::any()).await.map_err(|e| e.code()).err(),
        Some(codes::toolchain::BROKEN)
    );

    // A uv-managed PBS 3.12 is found, newest first, and becomes the env contract.
    let uv = tmp.path().join("uv");
    for v in ["3.12.14", "3.12.15", "3.11.9"] {
        let lib = uv.join(format!("cpython-{v}-linux-x86_64-gnu/lib"));
        std::fs::create_dir_all(&lib).expect("uv lib");
        let soname = if v.starts_with("3.12") { "libpython3.12.so.1.0" } else { "libpython3.11.so.1.0" };
        std::fs::write(lib.join(soname), b"").expect("uv libpython");
    }
    host.uv_python_dirs = vec![uv.clone()];
    let tc = toolchains(tmp.path(), host);
    let lldb = tc.resolve(Component::Lldb, &Requirement::any()).await.expect("lldb resolves");
    let pbs = uv.join("cpython-3.12.15-linux-x86_64-gnu");
    assert_eq!(lldb.version, "21.0.0");
    assert_eq!(lldb.env.get("PYTHONHOME").map(String::as_str), Some(pbs.to_str().expect("utf-8")));
    let expected = format!(
        "{}/lib:{}/usr/lib/swift/linux:{}/usr/lib/swift/host/compiler",
        pbs.display(),
        std::fs::canonicalize(&root).expect("root").display(),
        std::fs::canonicalize(&root).expect("root").display()
    );
    assert_eq!(lldb.env.get("LD_LIBRARY_PATH"), Some(&expected));
    let dap = tc.resolve(Component::LldbDap, &Requirement::any()).await.expect("lldb-dap");
    assert_eq!((dap.version.as_str(), &dap.env), ("21.0.0", &lldb.env));
    let py = tc.resolve(Component::LibPython, &Requirement::any()).await.expect("libpython");
    assert_eq!((py.version.as_str(), &py.provenance), ("3.12.15", &Provenance::Uv));
    // The llvm flavor finds nothing here.
    let err = tc.resolve(Component::Lldb, &Requirement::flavor("llvm")).await.expect_err("no distro lldb");
    assert_eq!(err.code(), codes::toolchain::MISSING);
}

#[tokio::test]
async fn nothing_installed_is_missing_with_a_hint() {
    let tmp = tempfile::tempdir().expect("tmp");
    let tc = toolchains(tmp.path(), common::host(vec![], vec![], tmp.path()));
    for c in [Component::Swift, Component::Linker, Component::Xtool] {
        let err = tc.resolve(c, &Requirement::any()).await.expect_err("missing");
        assert_eq!(err.code(), codes::toolchain::MISSING, "{c:?}");
        assert!(err.hint().is_some());
    }
    let err = tc.resolve(Component::DarwinSdk, &Requirement::any()).await.expect_err("no sdk");
    assert_eq!(err.code(), codes::sdk::MISSING);
    assert!(tc.sdks().await.expect("sdks").is_empty());
}

/// A manifest that pins darwin-tools 1.1.0 and python-shared 3.12.15 (the artifacts are never fetched here).
fn manifest_with_installs() -> russet_toolchain::pins::PinManifest {
    let component = |name: &str, version: &str| {
        format!(
            "[[component]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"test\"\n\n\
             [component.artifacts.linux-x86_64]\nurl = \"https://example.com/{name}.tar.gz\"\nsize = 1\n\
             sha256 = \"{}\"\nformat = \"tar.gz\"\n\n",
            "0".repeat(64)
        )
    };
    let components = component("darwin-tools", "1.1.0") + &component("python-shared", "3.12.15");
    common::signed_manifest(&components).0
}

fn install_detail(e: &russet_core::Error) -> Option<String> {
    e.to_data().details?.get("install")?.as_str().map(str::to_owned)
}

#[tokio::test]
async fn errors_name_the_install_that_fixes_them() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("swift-6.4");
    let bin = common::swift_toolchain(&root, "6.4");
    let sys = tmp.path().join("sysbin");
    common::script(&sys.join("ld64.lld"), "echo 'LLD 23.1.1'");
    let host = common::host(vec![bin, sys], vec![], tmp.path());
    let tc = Toolchains::new(RussetDirs::under(&tmp.path().join("rh")), Arc::new(manifest_with_installs()))
        .with_host(host.clone());

    // Only a too-old system LLD: darwin-tools fixes it, unless the project asks for another linker.
    let err = tc.resolve(Component::Linker, &Requirement::any()).await.expect_err("too old");
    assert_eq!(err.code(), codes::toolchain::LINKER_UNSUPPORTED);
    assert_eq!(install_detail(&err).as_deref(), Some("darwin-tools"));
    let err = tc.resolve(Component::Linker, &Requirement::flavor("darwin-tools@>=1.1")).await.expect_err("none");
    assert_eq!(install_detail(&err).as_deref(), Some("darwin-tools"));
    for other in ["llvm@>=23.1.3", "darwin-tools@>=1.2"] {
        let err = tc.resolve(Component::Linker, &Requirement::flavor(other)).await.expect_err(other);
        assert_eq!(install_detail(&err), None, "{other}: installing darwin-tools 1.1.0 does not satisfy it");
    }

    // An LLDB without remote-ios is not fixed by any install; one without libpython is.
    common::script(&root.join("usr/bin/lldb"), "echo 'lldb version 21.0.0'");
    let err = tc.resolve(Component::Lldb, &Requirement::any()).await.expect_err("no remote-ios");
    assert_eq!(err.code(), codes::toolchain::BROKEN);
    assert_eq!(install_detail(&err), None);
    if liblldb_needing_python(&root) {
        let tc = Toolchains::new(RussetDirs::under(&tmp.path().join("rh2")), Arc::new(manifest_with_installs()))
            .with_host(host);
        let err = tc.resolve(Component::Lldb, &Requirement::any()).await.expect_err("no libpython");
        assert_eq!(err.code(), codes::toolchain::BROKEN);
        assert_eq!(install_detail(&err).as_deref(), Some("python-shared"));
    } else {
        eprintln!("skipped the libpython half: no C compiler to build the fixture liblldb");
    }
    // Without a pin for it, nothing can be installed.
    let tc = toolchains(tmp.path(), common::host(vec![], vec![], tmp.path()));
    let err = tc.resolve(Component::Linker, &Requirement::any()).await.expect_err("missing");
    assert_eq!((err.code(), install_detail(&err)), (codes::toolchain::MISSING, None));
}
