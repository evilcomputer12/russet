// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The mocked anisette suite (W22 acceptance: `AnisetteFake`): the HTTP-range acquisition against a fake CDN
//! serving a synthetic APK (only the two libraries' bytes travel; a server ignoring `Range` is refused), and
//! `RemoteV3` against a fake anisette-v3 server whose provisioning goes through a fake GrandSlam.

#![allow(clippy::unwrap_used)]

// Shared cargo target slots: recompile this crate after another worktree built in the slot (AGENTS.md §6).
const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");

use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use russet_anisette::libs::{self, Abi, Consent};
use russet_anisette::provisioning::ClientIdentity;
use russet_anisette::{AnisetteKind, AnisetteProvider, RemoteV3};
use russet_core::error::codes;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// A synthetic APK: the two libraries for both ABIs plus 2 MB of filler that must never be downloaded.
fn apk() -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("assets/filler.bin", stored).unwrap();
    w.write_all(&vec![7u8; 2 * 1024 * 1024]).unwrap();
    for abi in ["x86_64", "arm64-v8a", "armeabi-v7a"] {
        for lib in libs::LIBRARIES {
            w.start_file(format!("lib/{abi}/{lib}"), deflated).unwrap();
            w.write_all(format!("synthetic {lib} for {abi} ").repeat(2000).as_bytes()).unwrap();
        }
    }
    w.finish().unwrap().into_inner()
}

struct Cdn {
    apk: Arc<Vec<u8>>,
    served: Arc<AtomicU64>,
    honour_range: bool,
}

impl Respond for Cdn {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let base = ResponseTemplate::new(200).insert_header("x-apple-version-number", "9.9.9.1");
        if req.method.as_str() == "HEAD" {
            return base.set_body_bytes(self.apk.as_ref().clone());
        }
        let range = req.headers.get("range").and_then(|v| v.to_str().ok()).and_then(|r| r.strip_prefix("bytes="));
        match range.and_then(|r| r.split_once('-')) {
            Some((a, b)) if self.honour_range => {
                let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                let body = self.apk[a..=b.min(self.apk.len() - 1)].to_vec();
                self.served.fetch_add(body.len() as u64, Ordering::SeqCst);
                ResponseTemplate::new(206).set_body_bytes(body)
            }
            _ => {
                self.served.fetch_add(self.apk.len() as u64, Ordering::SeqCst);
                base.set_body_bytes(self.apk.as_ref().clone())
            }
        }
    }
}

async fn cdn(honour_range: bool) -> (MockServer, Arc<AtomicU64>, usize) {
    let apk = Arc::new(apk());
    let served = Arc::new(AtomicU64::new(0));
    let server = MockServer::start().await;
    Mock::given(path(format!("/{}", libs::APK_PATH)))
        .respond_with(Cdn { apk: apk.clone(), served: served.clone(), honour_range })
        .mount(&server)
        .await;
    (server, served, apk.len())
}

#[tokio::test]
async fn only_the_two_libraries_are_range_fetched_and_trusted_on_first_use() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (server, served, apk_len) = cdn(true).await;
    let tmp = tempfile::tempdir().unwrap();
    let base = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let f = libs::fetch(tmp.path(), &base, Consent::given(true).unwrap()).await.unwrap();
    let abi = Abi::host().unwrap();
    assert_eq!(f.set.apk_version, "9.9.9.1");
    assert!(!f.set.pinned, "an unknown version is trusted on first use");
    assert_eq!(f.bytes_downloaded, served.load(Ordering::SeqCst));
    assert!(f.bytes_downloaded < (apk_len as u64) / 10, "{} of {apk_len} bytes", f.bytes_downloaded);
    assert_eq!(f.entries.len(), 2);
    assert!(f.entries.iter().all(|e| e.name.starts_with(&format!("lib/{}/", Abi::host().unwrap().dir_name()))));
    assert!(f.entries.iter().all(|e| e.sha256.len() == 64 && e.compressed_size < e.uncompressed_size));
    let dir = libs::libs_root(tmp.path()).join("9.9.9.1").join(abi.dir_name());
    for lib in libs::LIBRARIES {
        let text = std::fs::read_to_string(dir.join(lib)).unwrap();
        assert!(text.starts_with(&format!("synthetic {lib} for {}", abi.dir_name())));
    }
    assert!(dir.join("tofu.json").exists());
    assert_eq!(libs::installed(tmp.path(), abi), Some(f.set.clone()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    }

    // Present and verified: nothing is downloaded again.
    let again = libs::fetch(tmp.path(), &base, Consent::given(true).unwrap()).await.unwrap();
    assert_eq!(again.bytes_downloaded, 0);

    // A tampered library no longer verifies.
    std::fs::write(dir.join("libCoreADI.so"), b"tampered").unwrap();
    assert_eq!(libs::installed(tmp.path(), abi), None);
}

#[tokio::test]
async fn a_cdn_ignoring_range_is_refused_before_the_body() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (server, _served, _) = cdn(false).await;
    let tmp = tempfile::tempdir().unwrap();
    let base = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    let e = libs::fetch(tmp.path(), &base, Consent::given(true).unwrap()).await.unwrap_err();
    assert_eq!(e.code(), codes::anisette::FETCH_FAILED);
    assert!(libs::installed(tmp.path(), Abi::host().unwrap()).is_none());
}

/// A fake GrandSlam that provisions: `spim = "spim"`, `ptm = "ptm:" ‖ cpim`, `tk = "tk:" ‖ cpim`.
async fn gsa() -> MockServer {
    let server = MockServer::start().await;
    let plist = |entries: Vec<(&str, plist::Value)>| {
        let mut resp = plist::Dictionary::new();
        for (k, v) in entries {
            resp.insert(k.into(), v);
        }
        let mut root = plist::Dictionary::new();
        root.insert("Response".into(), plist::Value::Dictionary(resp));
        let mut out = Vec::new();
        plist::to_writer_xml(&mut out, &plist::Value::Dictionary(root)).unwrap();
        out
    };
    let start = plist(vec![("spim", b64(b"spim").into())]);
    Mock::given(method("POST"))
        .and(path("/grandslam/MidService/startMachineProvisioning"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(start))
        .mount(&server)
        .await;
    struct Finish;
    impl Respond for Finish {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let v: plist::Value = plist::from_bytes(&req.body).unwrap();
            let cpim =
                v.as_dictionary().unwrap()["Request"].as_dictionary().unwrap()["cpim"].as_string().unwrap().to_owned();
            let cpim = base64::engine::general_purpose::STANDARD.decode(cpim).unwrap();
            let mut resp = plist::Dictionary::new();
            resp.insert("ptm".into(), b64(&[b"ptm:".as_slice(), &cpim].concat()).into());
            resp.insert("tk".into(), b64(&[b"tk:".as_slice(), &cpim].concat()).into());
            resp.insert("X-Apple-I-MD-RINFO".into(), "999".into());
            let mut root = plist::Dictionary::new();
            root.insert("Response".into(), plist::Value::Dictionary(resp));
            let mut out = Vec::new();
            plist::to_writer_xml(&mut out, &plist::Value::Dictionary(root)).unwrap();
            ResponseTemplate::new(200).set_body_bytes(out)
        }
    }
    Mock::given(method("POST"))
        .and(path("/grandslam/MidService/finishMachineProvisioning"))
        .respond_with(Finish)
        .mount(&server)
        .await;
    server
}

/// A fake anisette-v3 server on one port: HTTP for `client_info` and `get_headers`, a websocket for provisioning.
async fn v3(provisionings: Arc<Mutex<u32>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else { return };
            let provisionings = provisionings.clone();
            tokio::spawn(async move {
                let mut peek = [0u8; 256];
                let n = s.peek(&mut peek).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&peek[..n]).into_owned();
                if head.starts_with("GET /v3/provisioning_session") {
                    let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
                    let say = |v: serde_json::Value| Message::Text(v.to_string().into());
                    ws.send(say(serde_json::json!({"result": "GiveIdentifier"}))).await.unwrap();
                    let id = ws.next().await.unwrap().unwrap().into_text().unwrap();
                    assert!(id.contains("identifier"));
                    ws.send(say(serde_json::json!({"result": "GiveStartProvisioningData"}))).await.unwrap();
                    let spim: serde_json::Value =
                        serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                    assert_eq!(spim["spim"], b64(b"spim"));
                    ws.send(say(serde_json::json!({"result": "GiveEndProvisioningData", "cpim": b64(b"server-cpim")})))
                        .await
                        .unwrap();
                    let end: serde_json::Value =
                        serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                    assert_eq!(end["ptm"], b64(b"ptm:server-cpim"));
                    *provisionings.lock().unwrap() += 1;
                    ws.send(say(serde_json::json!({"result": "ProvisioningSuccess", "adi_pb": b64(b"remote-adi-pb")})))
                        .await
                        .unwrap();
                    return;
                }
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let Some(end) = text.find("\r\n\r\n") else { if n == 0 { break } else { continue } };
                    let len = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if n == 0 || buf.len() >= end + 4 + len {
                        break;
                    }
                }
                let req = String::from_utf8_lossy(&buf).into_owned();
                let body = if req.starts_with("GET /v3/client_info") {
                    serde_json::json!({"client_info": "<RemoteFake> <macOS;27.0;0> <com.apple.AuthKit/1>", "user_agent": "akd/1.0"})
                } else if req.contains("remote-adi-pb") || req.contains(&b64(b"remote-adi-pb")) {
                    serde_json::json!({"result": "Headers", "X-Apple-I-MD": "b3Rw", "X-Apple-I-MD-M": "bWlk", "X-Apple-I-MD-RINFO": "999"})
                } else {
                    serde_json::json!({"result": "GetHeadersError", "message": "unknown identity"})
                }
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn remote_v3_provisions_once_through_its_websocket_and_serves_headers() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let gsa = gsa().await;
    let provisionings = Arc::new(Mutex::new(0));
    let url = url::Url::parse(&v3(provisionings.clone()).await).unwrap();
    let secrets: Arc<dyn russet_secrets::SecretStore> = Arc::new(russet_secrets::memory::MemoryStore::new());
    let fallback = ClientIdentity { client_info: "fallback".into(), user_agent: "fallback".into() };
    let gsa_url = url::Url::parse(&format!("{}/", gsa.uri())).unwrap();
    let remote = RemoteV3::new(url.clone(), secrets.clone(), reqwest::Client::new(), gsa_url, fallback).unwrap();
    assert_eq!(remote.kind(), AnisetteKind::Remote { url: url.clone() });
    let id = remote.identity().await.unwrap();
    assert!(id.client_info.starts_with("<RemoteFake>") && id.user_agent == "akd/1.0");

    let h = remote.headers().await.unwrap();
    assert_eq!((h.md.as_str(), h.md_m.as_str(), h.md_rinfo.as_str()), ("b3Rw", "bWlk", "999"));
    assert_eq!(h.md_lu.len(), 64);
    remote.headers().await.unwrap();
    assert_eq!(*provisionings.lock().unwrap(), 1, "provisioned once, then reused");
    let state = russet_anisette::AnisetteState::load(&*secrets, &russet_anisette::remote::state_name(&url))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.adi_pb_bytes().unwrap(), b"remote-adi-pb");

    assert!(
        RemoteV3::new(
            url::Url::parse("http://anisette.example/").unwrap(),
            secrets,
            reqwest::Client::new(),
            url,
            ClientIdentity { client_info: String::new(), user_agent: String::new() },
        )
        .is_err(),
        "plain http to a non-loopback server is refused"
    );
}

#[cfg(feature = "fake")]
#[tokio::test]
async fn local_adi_provisions_again_when_the_client_identity_changes() {
    use russet_anisette::fake::FakeAdi;
    use russet_anisette::{LibrarySet, LocalAdi};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let gsa = gsa().await;
    let gsa_url = url::Url::parse(&format!("{}/", gsa.uri())).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let libs = LibrarySet { apk_version: "t".into(), abi: Abi::X86_64, dir: tmp.path().join("libs"), pinned: false };
    let adi = tmp.path().join("adi");
    let secrets: Arc<dyn russet_secrets::SecretStore> = Arc::new(russet_secrets::memory::MemoryStore::new());
    let windows = ClientIdentity {
        client_info: "<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into(),
        user_agent: "akd/1.0 CFNetwork/808.1.4".into(),
    };
    let mac = ClientIdentity {
        client_info: "<MacBookPro18,3> <macOS;27.0;26A5378j> <com.apple.AuthKit/1 (com.apple.akd/1.0)>".into(),
        user_agent: "akd/1.0 CFNetwork/808.1.4".into(),
    };
    // A new provider (a new helper process) per identity, as after a Russet update.
    let provider = |id: &ClientIdentity| {
        LocalAdi::new(
            libs.clone(),
            Arc::new(FakeAdi::new()),
            secrets.clone(),
            reqwest::Client::new(),
            gsa_url.clone(),
            id.clone(),
            adi.clone(),
        )
    };
    let finishes = || async {
        gsa.received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path().ends_with("finishMachineProvisioning"))
            .map(|r| r.headers.get("x-mme-client-info").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
    };

    let old = provider(&mac);
    old.headers().await.unwrap();
    old.headers().await.unwrap();
    assert_eq!(finishes().await, std::slice::from_ref(&mac.client_info), "provisioned once under the old identity");
    provider(&mac).headers().await.unwrap();
    assert_eq!(finishes().await.len(), 1, "the same identity reuses the provisioning");

    let new = provider(&windows);
    let h = new.headers().await.unwrap();
    assert_eq!(finishes().await, [mac.client_info.clone(), windows.client_info.clone()], "provisioned again");
    assert_eq!(new.identity().await.unwrap(), windows);
    assert!(!h.md.is_empty());
    provider(&windows).headers().await.unwrap();
    assert_eq!(finishes().await.len(), 2);
}
