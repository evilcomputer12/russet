// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The installer against a local HTTP server and archives built at test time (CORE-11 subset): install and
//! resolve into a temporary `RUSSET_HOME`, resume an interrupted download, refuse when free space is short, reject
//! a digest mismatch and an escaping archive, uninstall and GC. The env-gated test at the end downloads the real
//! python-build-standalone 3.12.15 (34 MB) once.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::progress::{LogStream, ProgressSink};
use russet_toolchain::pins::{self, PinManifest};
use russet_toolchain::{Component, Provenance, Requirement, Toolchains};
use sha2::Digest as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

struct NoProgress;

impl ProgressSink for NoProgress {
    fn begin(&self, _title: &str, _cancellable: bool) {}
    fn report(&self, _msg: Option<&str>, _pct: Option<u8>) {}
    fn end(&self, _msg: Option<&str>) {}
    fn log(&self, _stream: LogStream, _line: &str) {}
}

/// A one-file HTTP/1.1 server with `Range` support; `cut_first` ends the first response after that many bytes.
struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    ranges: Arc<AtomicUsize>,
}

async fn serve(body: Vec<u8>, cut_first: Option<usize>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://127.0.0.1:{}/artifact.tar.gz", listener.local_addr().expect("addr").port());
    let requests = Arc::new(AtomicUsize::new(0));
    let ranges = Arc::new(AtomicUsize::new(0));
    let (req_count, range_count) = (requests.clone(), ranges.clone());
    let body = Arc::new(body);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let n = req_count.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            let range_count = range_count.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut head = Vec::new();
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(k) = sock.read(&mut buf).await else { return };
                    if k == 0 {
                        return;
                    }
                    head.extend_from_slice(&buf[..k]);
                }
                let head = String::from_utf8_lossy(&head).to_lowercase();
                let start = head
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.trim().trim_end_matches('-').parse::<usize>().ok());
                let (status, from) = match start {
                    Some(s) => {
                        range_count.fetch_add(1, Ordering::SeqCst);
                        (
                            format!(
                                "206 Partial Content\r\nContent-Range: bytes {s}-{}/{}",
                                body.len() - 1,
                                body.len()
                            ),
                            s,
                        )
                    }
                    None => ("200 OK".to_owned(), 0),
                };
                let rest = &body[from..];
                let header =
                    format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", rest.len());
                let _ = sock.write_all(header.as_bytes()).await;
                let send = match cut_first {
                    Some(cut) if n == 0 => &rest[..cut.min(rest.len())],
                    _ => rest,
                };
                let _ = sock.write_all(send).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { url, requests, ranges }
}

fn tar_gz(entries: &[(&str, &[u8], u32)], symlinks: &[(&str, &str)], raw_escape: bool) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    for (path, data, mode) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(*mode);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, path, *data).expect("append");
    }
    for (path, target) in symlinks {
        let mut h = tar::Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o777);
        h.set_entry_type(tar::EntryType::Symlink);
        b.append_link(&mut h, path, target).expect("symlink");
    }
    if raw_escape {
        let mut h = tar::Header::new_old();
        let name = b"../escape";
        h.as_old_mut().name[..name.len()].copy_from_slice(name);
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, &b"x"[..]).expect("raw entry");
    }
    let enc = b.into_inner().expect("tar");
    enc.finish().expect("gz")
}

fn python_archive() -> Vec<u8> {
    tar_gz(
        &[
            ("python/bin/python3.12", b"#!/bin/sh\necho 'Python 3.12.15'\n", 0o755),
            ("python/lib/libpython3.12.so.1.0", b"", 0o755),
            ("python/lib/python3.12/os.py", b"# stdlib\n", 0o644),
        ],
        &[("python/bin/python3", "python3.12")],
        false,
    )
}

fn sha256(b: &[u8]) -> String {
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn manifest_for(name: &str, version: &str, url: &str, body: &[u8], sha: Option<&str>) -> PinManifest {
    let platform = pins::host_platform();
    let digest = sha.map_or_else(|| sha256(body), str::to_owned);
    let component = format!(
        "[[component]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"test\"\n\n\
         [component.artifacts.{platform}]\nurl = \"{url}\"\nsize = {}\nsha256 = \"{digest}\"\nformat = \"tar.gz\"\n",
        body.len()
    );
    common::signed_manifest(&component).0
}

fn toolchains(home: &Path, m: PinManifest) -> Toolchains {
    Toolchains::new(RussetDirs::under(home), Arc::new(m))
        .with_host(common::host(vec![], vec![], home))
        .with_min_free_space(0)
}

#[tokio::test]
async fn python_shared_installs_into_a_temporary_home_and_resolves() {
    let tmp = tempfile::tempdir().expect("home");
    let body = python_archive();
    let server = serve(body.clone(), None).await;
    let tc = toolchains(tmp.path(), manifest_for("python-shared", "3.12.15", &server.url, &body, None));
    let mut changes = tc.subscribe();
    let tool =
        tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect("install");
    let dir = tmp.path().join("data/toolchains/python-shared/3.12.15");
    assert_eq!(tool.path, dir.join("lib/libpython3.12.so.1.0"));
    assert_eq!(tool.provenance, Provenance::Managed { pin: "python-shared@3.12.15".into() });
    assert_eq!(tool.env.get("PYTHONHOME").map(String::as_str), dir.to_str());
    assert!(dir.join(".russet-install.json").is_file());
    assert!(dir.join("bin/python3").is_symlink(), "relative symlinks inside the tree are kept");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("bin/python3.12")).expect("stat").permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "executable bits survive extraction");
    }
    let selected = std::fs::read_to_string(tmp.path().join("data/toolchains/selected.json")).expect("selected");
    assert!(selected.contains("\"python-shared\": \"3.12.15\""), "{selected}");
    let downloads: Vec<_> = std::fs::read_dir(tmp.path().join("cache/downloads")).expect("downloads").collect();
    assert!(downloads.is_empty(), "the download is deleted after extraction");
    assert_eq!(changes.try_recv().ok(), Some(vec![Component::LibPython]));

    // Resolving finds Russet's install; installing again downloads nothing.
    let resolved = tc.resolve(Component::LibPython, &Requirement::any()).await.expect("resolve");
    assert_eq!(resolved.path, tool.path);
    let before = server.requests.load(Ordering::SeqCst);
    tc.install(Component::LibPython, "", &NoProgress, &CancellationToken::new()).await.expect("idempotent");
    assert_eq!(server.requests.load(Ordering::SeqCst), before);

    // Uninstall frees the bytes and the selection.
    let freed = tc.uninstall(Component::LibPython, "3.12.15").await.expect("uninstall");
    assert!(freed > 0);
    assert!(!dir.exists());
    let err = tc.uninstall(Component::LibPython, "3.12.15").await.expect_err("gone");
    assert_eq!(err.code(), codes::core::NOT_FOUND);
}

#[tokio::test]
async fn an_interrupted_download_resumes() {
    let tmp = tempfile::tempdir().expect("home");
    let body = python_archive();
    let server = serve(body.clone(), Some(body.len() / 2)).await;
    let tc = toolchains(tmp.path(), manifest_for("python-shared", "3.12.15", &server.url, &body, None));
    let err = tc
        .install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new())
        .await
        .expect_err("the connection drops half-way");
    assert_eq!(err.code(), codes::toolchain::DOWNLOAD_FAILED);
    assert!(err.to_data().retryable);
    let part = tmp.path().join(format!("cache/downloads/{}.part", sha256(&body)));
    assert_eq!(std::fs::metadata(&part).expect("partial download kept").len() as usize, body.len() / 2);
    tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect("resumes");
    assert_eq!(server.ranges.load(Ordering::SeqCst), 1, "the second request asked for the rest");
    assert!(!part.exists());
}

#[tokio::test]
async fn installs_are_refused_without_free_space_and_on_digest_mismatch() {
    let tmp = tempfile::tempdir().expect("home");
    let body = python_archive();
    let server = serve(body.clone(), None).await;
    let tc = toolchains(tmp.path(), manifest_for("python-shared", "3.12.15", &server.url, &body, None))
        .with_min_free_space(u64::MAX / 4);
    let err =
        tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect_err("full");
    assert_eq!(err.code(), codes::core::DISK_FULL);
    assert_eq!(server.requests.load(Ordering::SeqCst), 0, "nothing was downloaded");

    let wrong = "0".repeat(64);
    let tc = toolchains(tmp.path(), manifest_for("python-shared", "3.12.15", &server.url, &body, Some(&wrong)));
    let err =
        tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect_err("sha");
    assert_eq!(err.code(), codes::toolchain::VERIFY_FAILED);
    assert!(!tmp.path().join("data/toolchains/python-shared/3.12.15").exists());
    assert!(!tmp.path().join(format!("cache/downloads/{wrong}.part")).exists(), "a bad download is deleted");

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let tc = toolchains(tmp.path(), manifest_for("python-shared", "3.12.15", &server.url, &body, None));
    let err = tc.install(Component::LibPython, "3.12.15", &NoProgress, &cancelled).await.expect_err("cancelled");
    assert_eq!(err.code(), codes::core::CANCELLED);

    let tc = toolchains(
        tmp.path(),
        manifest_for("python-shared", "3.12.15", "https://example.invalid/p.tar.gz", &body, None),
    )
    .with_offline(true);
    let err =
        tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect_err("offline");
    assert_eq!(err.code(), codes::toolchain::DOWNLOAD_FAILED);

    let err = tc.install(Component::Swift, "6.4.0", &NoProgress, &CancellationToken::new()).await.expect_err("swift");
    assert_eq!(err.code(), codes::core::NOT_IMPLEMENTED, "no Swift downloads in Wave 1");
}

#[tokio::test]
async fn an_archive_that_escapes_its_directory_is_rejected() {
    let tmp = tempfile::tempdir().expect("home");
    let body = tar_gz(&[("bin/ld64.lld", b"#!/bin/sh\necho 'LLD 21.1.6'\n", 0o755)], &[], true);
    let server = serve(body.clone(), None).await;
    let tc = toolchains(tmp.path(), manifest_for("darwin-tools", "1.1.0", &server.url, &body, None));
    let err = tc.install(Component::Linker, "1.1.0", &NoProgress, &CancellationToken::new()).await.expect_err("escape");
    assert_eq!(err.code(), codes::toolchain::VERIFY_FAILED);
    assert!(!tmp.path().join("data/escape").exists() && !tmp.path().join("data/toolchains/escape").exists());
    let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join("data/toolchains"))
        .expect("toolchains")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".staging-"))
        .collect();
    assert!(leftovers.is_empty(), "the staging directory is removed");

    let body = tar_gz(&[], &[("bin/evil", "../../../../etc/passwd")], false);
    let server = serve(body.clone(), None).await;
    let tc = toolchains(tmp.path(), manifest_for("darwin-tools", "1.1.0", &server.url, &body, None));
    let err = tc.install(Component::Linker, "1.1.0", &NoProgress, &CancellationToken::new()).await.expect_err("link");
    assert_eq!(err.code(), codes::toolchain::VERIFY_FAILED);
}

#[tokio::test]
async fn darwin_tools_installs_its_linker_and_gc_keeps_the_selection() {
    let tmp = tempfile::tempdir().expect("home");
    let body =
        tar_gz(&[("bin/ld64.lld", b"#!/bin/sh\necho 'LLD 21.1.6'\n", 0o755), ("bin/dsymutil", b"", 0o755)], &[], false);
    let server = serve(body.clone(), None).await;
    let tc = toolchains(tmp.path(), manifest_for("darwin-tools", "1.1.0", &server.url, &body, None));
    let tool = tc.install(Component::Linker, "1.1.0", &NoProgress, &CancellationToken::new()).await.expect("install");
    assert_eq!(tool.version, "21.1.6");
    assert_eq!(tool.provenance, Provenance::Managed { pin: "darwin-tools@1.1.0".into() });
    assert!(tool.ok, "{tool:?}");
    let linker = tc.resolve(Component::Linker, &Requirement::flavor("darwin-tools@1.1.0")).await.expect("flavor");
    assert_eq!(linker.path, tool.path);

    // An older, unselected version is garbage; the selected one stays.
    let old = tmp.path().join("data/toolchains/darwin-tools/1.0.0");
    std::fs::create_dir_all(old.join("bin")).expect("old");
    std::fs::write(old.join(".russet-install.json"), "{}").expect("marker");
    std::fs::write(old.join("bin/ld64.lld"), vec![0u8; 4096]).expect("old linker");
    let report = tc.gc(true).await.expect("dry run");
    assert_eq!(report.removed, vec![old.clone()]);
    assert!(old.exists(), "a dry run removes nothing");
    let report = tc.gc(false).await.expect("gc");
    assert!(report.freed_bytes >= 4096);
    assert!(!old.exists());
    assert!(tmp.path().join("data/toolchains/darwin-tools/1.1.0/bin/ld64.lld").exists());
}

#[test]
fn a_manifest_whose_signature_or_content_was_altered_is_rejected() {
    let (_, text, sig, pk) = common::signed_manifest("");
    assert!(PinManifest::load_verified(&text, &sig, &[&pk]).is_ok());
    let altered = text.replace("min-lld = \"23.1.3\"", "min-lld = \"21.0.0\"");
    let err = PinManifest::load_verified(&altered, &sig, &[&pk]).expect_err("content altered");
    assert_eq!(err.code(), codes::toolchain::PINS_INVALID);
    let (other_pk, other_sk) = common::keypair();
    let forged = common::sign(&other_sk, &altered);
    let err = PinManifest::load_verified(&altered, &forged, &[&pk]).expect_err("signed by an untrusted key");
    assert_eq!(err.code(), codes::toolchain::PINS_INVALID);
    assert!(PinManifest::load_verified(&altered, &forged, &[&other_pk]).is_ok(), "the signer's own key verifies");
}

/// The real thing, once (PLAN §4.3: "PBS installs into a temporary RUSSET_HOME and resolves"): downloads the
/// pinned 34 MB python-build-standalone with the embedded, dev-key-signed manifest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pinned_python_build_standalone_installs_for_real() {
    if std::env::var_os("RUSSET_TEST_NETWORK").is_none_or(|v| v != "1") {
        eprintln!("skipped: set RUSSET_TEST_NETWORK=1 to download python-build-standalone 3.12.15 (34 MB)");
        return;
    }
    let tmp = tempfile::tempdir().expect("home");
    let tc = Toolchains::load(&RussetDirs::under(tmp.path()))
        .expect("embedded pins")
        .with_host(common::host(vec![], vec![], tmp.path()))
        .with_min_free_space(1 << 30);
    let tool =
        tc.install(Component::LibPython, "3.12.15", &NoProgress, &CancellationToken::new()).await.expect("install");
    eprintln!("installed {tool:?}");
    assert_eq!(tool.version, "3.12.15");
    let home = tmp.path().join("data/toolchains/python-shared/3.12.15");
    assert!(home.join("lib/libpython3.12.so.1.0").is_file());
    let python = home.join("bin/python3.12");
    let out = std::process::Command::new(&python).arg("--version").output().expect("run python");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "Python 3.12.15");

    // RD-05: with no uv Python and no system libpython in sight, the host's Swift LLDB and lldb-dap start with
    // Russet's python-shared through the env contract alone.
    let mut host = russet_toolchain::host::HostEnv::from_env();
    host.uv_python_dirs.clear();
    host.ldconfig = None;
    let tc = Toolchains::load(&RussetDirs::under(tmp.path())).expect("embedded pins").with_host(host);
    if tc.resolve(Component::Swift, &Requirement::any()).await.is_err() {
        eprintln!("no Swift toolchain on this host; LLDB part skipped");
        return;
    }
    for c in [Component::Lldb, Component::LldbDap] {
        let t = tc.resolve(c, &Requirement::any()).await.expect("lldb with Russet's python");
        eprintln!("resolved {t:?}");
        assert_eq!(t.env.get("PYTHONHOME").map(String::as_str), home.to_str(), "{t:?}");
        assert!(t.ok && t.version.starts_with("21."), "{t:?}");
    }
}
