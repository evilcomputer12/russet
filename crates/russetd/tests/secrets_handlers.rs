// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `secrets.*` handlers over the memory store: metadata only, `fromFile` imports from the CLI and IDE only, and
//! the redaction canary never reaches a result.

use russet_core::error::codes;
use russet_core::rpc::{ClientKind, ConnInfo};
use russet_protocol::secrets as p;
use russet_secrets::SecretStore;
use russet_secrets::memory::MemoryStore;
use russetd::handlers::secrets as h;

/// A temporary directory removed on drop (russetd has no tempfile dev-dependency).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("russet-w3-test-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CANARY: &str = "RUSSET-CANARY-handler-3b8e-never-on-the-wire";

fn conn(kind: ClientKind) -> ConnInfo {
    ConnInfo { id: "c-1".into(), kind, name: "test".into(), interactive: false, shared_lease: None }
}

#[tokio::test]
async fn set_list_status_delete_without_ever_returning_the_value() {
    let store = MemoryStore::new();
    let dir = TempDir::new();
    let file = dir.path().join("AuthKey_TEST.p8");
    std::fs::write(&file, CANARY).expect("write");
    let params = p::SecretsSetParams { id: "asc-key/TEST".into(), label: None, from_file: file.display().to_string() };
    let meta = h::set(&store, &conn(ClientKind::Cli), params).await.expect("set from a non-interactive CLI");
    assert_eq!(
        (meta.label.as_str(), meta.size_bytes, meta.kind),
        ("AuthKey_TEST.p8", CANARY.len() as u64, p::SecretKind::AscKey)
    );
    let stored =
        store.get(&russet_secrets::SecretId::parse("asc-key/TEST").expect("id")).await.expect("get").expect("value");
    assert_eq!(stored.expose(), CANARY.as_bytes());

    let list = h::list(&store, p::SecretsListParams { kind: Some(p::SecretKind::AscKey) }).await.expect("list");
    let status = h::status(&store).await;
    let rendered = serde_json::to_string(&(&meta, &list, &status)).expect("json");
    assert!(!rendered.contains(CANARY), "the canary reached a result: {rendered}");
    assert_eq!(list.items.len(), 1);
    assert_eq!(status.backend, p::SecretBackend::Memory);

    let deleted = h::delete(&store, p::SecretsDeleteParams { id: "asc-key/TEST".into() }).await.expect("delete");
    assert!(deleted.deleted);
    assert!(!h::delete(&store, p::SecretsDeleteParams { id: "asc-key/TEST".into() }).await.expect("again").deleted);
}

#[tokio::test]
async fn imports_are_refused_from_agents_and_bad_paths() {
    let store = MemoryStore::new();
    let dir = TempDir::new();
    let file = dir.path().join("token");
    std::fs::write(&file, "t").expect("write");
    let params =
        |from: String| p::SecretsSetParams { id: "github-token/me".into(), label: Some("gh".into()), from_file: from };
    let err = h::set(&store, &conn(ClientKind::Mcp), params(file.display().to_string())).await.expect_err("mcp");
    assert_eq!(err.code(), codes::perm::DENIED);
    let err = h::set(&store, &conn(ClientKind::Ide), params("token".into())).await.expect_err("relative");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
    let missing = dir.path().join("missing").display().to_string();
    let err = h::set(&store, &conn(ClientKind::Ide), params(missing)).await.expect_err("missing");
    assert_eq!(err.code(), codes::core::NOT_FOUND);
    let big = dir.path().join("big");
    std::fs::write(&big, vec![0u8; russet_secrets::MAX_SECRET_BYTES + 1]).expect("big");
    let err = h::set(&store, &conn(ClientKind::Cli), params(big.display().to_string())).await.expect_err("too big");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
    let bad_id = p::SecretsSetParams { id: "nope".into(), label: None, from_file: file.display().to_string() };
    assert_eq!(
        h::set(&store, &conn(ClientKind::Cli), bad_id).await.map_err(|e| e.code()).err(),
        Some(codes::core::INVALID_ARGUMENT)
    );
    assert!(store.list(None).await.expect("list").is_empty(), "nothing was stored");
}
