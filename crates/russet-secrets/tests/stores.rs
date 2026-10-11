// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The sealed store with the passphrase backend, the memory store, backend selection and the redaction canary.
//! No test here touches a keyring (`keyring_live.rs` holds the one env-gated keyring test).

use std::path::Path;

use russet_core::dirs::RussetDirs;
use russet_core::error::codes;
use russet_core::secret::Redacted;
use russet_secrets::passphrase::{KdfCost, PassphraseKeySource};
use russet_secrets::sealed::SealedStore;
use russet_secrets::{
    BackendChoice, BackendKind, SecretBytes, SecretId, SecretKind, SecretStore, SecretsConfig, memory, open_default,
};

/// Cheap Argon2 parameters, so the suite stays fast; one test uses the real cost.
const TEST_COST: KdfCost = KdfCost { m_kib: 64, t: 1, p: 1 };
const CANARY: &str = "RUSSET-CANARY-7f3a9c1e-do-not-log";

fn passphrase_store(dir: &Path, passphrase: Option<&str>) -> SealedStore {
    SealedStore::new(
        dir.to_path_buf(),
        Box::new(PassphraseKeySource::new(dir.to_path_buf(), passphrase.map(str::to_owned), TEST_COST)),
    )
}

fn id(kind: SecretKind, name: &str) -> SecretId {
    SecretId { kind, name: name.into() }
}

fn four_kib_secret() -> Vec<u8> {
    let mut v = CANARY.as_bytes().to_vec();
    while v.len() < 4096 {
        v.push((v.len() % 251) as u8);
    }
    v
}

#[tokio::test]
async fn a_4_kib_secret_round_trips_through_the_passphrase_store() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("secrets");
    let store = passphrase_store(&dir, Some("correct horse"));
    let secret = four_kib_secret();
    let asc = id(SecretKind::AscKey, "2X9R4HXF34");
    let meta = store.put(&asc, "ASC key", SecretBytes::new(secret.clone())).await.expect("put");
    assert_eq!((meta.size_bytes, meta.backend), (4096, BackendKind::PassphraseFile));
    assert_eq!(store.get(&asc).await.expect("get").expect("present").expose(), &secret[..]);

    // A fresh store over the same directory (a daemon restart) opens it with the same passphrase.
    let reopened = passphrase_store(&dir, Some("correct horse"));
    assert_eq!(reopened.get(&asc).await.expect("get").expect("present").expose(), &secret[..]);
    let listed = reopened.list(None).await.expect("list");
    assert_eq!(listed.iter().map(|m| m.id.to_string()).collect::<Vec<_>>(), ["asc-key/2X9R4HXF34"]);
    assert_eq!(listed[0].label, "ASC key");
    assert!(reopened.list(Some(SecretKind::SigningKey)).await.expect("list").is_empty());

    // Overwrite, delete, and missing ids.
    reopened.put(&asc, "ASC key v2", SecretBytes::new(b"v2".to_vec())).await.expect("overwrite");
    assert_eq!(reopened.get(&asc).await.expect("get").expect("present").expose(), b"v2");
    assert!(reopened.delete(&asc).await.expect("delete"));
    assert!(!reopened.delete(&asc).await.expect("delete twice"));
    assert!(reopened.get(&asc).await.expect("get").is_none());
    let err = russet_secrets::require(&reopened, &asc).await.expect_err("missing");
    assert_eq!(err.code(), codes::secrets::NOT_FOUND);
}

#[tokio::test]
async fn a_wrong_or_missing_passphrase_never_overwrites_anything() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("secrets");
    let asc = id(SecretKind::AscKey, "K");
    passphrase_store(&dir, Some("right")).put(&asc, "k", SecretBytes::new(b"v".to_vec())).await.expect("put");

    let wrong = passphrase_store(&dir, Some("wrong"));
    let err = wrong.get(&asc).await.expect_err("wrong passphrase");
    assert_eq!(err.code(), codes::secrets::LOCKED);
    let err = wrong.put(&id(SecretKind::GithubToken, "g"), "g", SecretBytes::new(vec![1])).await.expect_err("locked");
    assert_eq!(err.code(), codes::secrets::LOCKED);
    let status = wrong.status().await;
    assert!(status.locked, "{status:?}");

    let none = passphrase_store(&dir, None);
    let err = none.get(&asc).await.expect_err("no passphrase");
    assert_eq!(err.code(), codes::secrets::PASSPHRASE_REQUIRED);
    assert!(none.status().await.locked);
    // Listing needs no key: names only.
    assert_eq!(none.list(None).await.expect("list").len(), 1);
    // The right passphrase still opens it.
    assert_eq!(passphrase_store(&dir, Some("right")).get(&asc).await.expect("get").expect("v").expose(), b"v");
}

#[cfg(unix)]
#[tokio::test]
async fn the_store_is_private_and_holds_no_plaintext() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("secrets");
    let store = passphrase_store(&dir, Some("pw"));
    store.put(&id(SecretKind::AscKey, "K"), "label", SecretBytes::new(four_kib_secret())).await.expect("put");
    assert_eq!(std::fs::metadata(&dir).expect("dir").permissions().mode() & 0o777, 0o700);
    for entry in std::fs::read_dir(&dir).expect("ls") {
        let path = entry.expect("entry").path();
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
        let bytes = std::fs::read(&path).expect("read");
        assert!(!bytes.windows(CANARY.len()).any(|w| w == CANARY.as_bytes()), "plaintext in {}", path.display());
    }
    let index = std::fs::read_to_string(dir.join("index.json")).expect("index");
    assert!(index.contains("asc-key/K") && index.contains("label"));
    let store_json = std::fs::read_to_string(dir.join("store.json")).expect("store.json");
    assert!(store_json.contains("\"passphrase\""), "{store_json}");
}

#[tokio::test]
async fn the_default_argon2_cost_works() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("secrets");
    let store =
        SealedStore::new(dir.clone(), Box::new(PassphraseKeySource::new(dir, Some("pw".into()), KdfCost::DEFAULT)));
    let t = std::time::Instant::now();
    store.put(&id(SecretKind::TcpToken, "t"), "t", SecretBytes::new(vec![7; 32])).await.expect("put");
    assert_eq!(store.get(&id(SecretKind::TcpToken, "t")).await.expect("get").expect("v").expose(), &[7; 32]);
    assert!(t.elapsed().as_secs() < 30, "Argon2id with m = 64 MiB took {:?}", t.elapsed());
}

#[tokio::test]
async fn auto_keeps_the_backend_that_sealed_the_store() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dirs = RussetDirs::under(tmp.path());
    let cfg = SecretsConfig {
        backend: BackendChoice::Passphrase,
        passphrase: Some(Redacted("pw".into())),
        ..SecretsConfig::default()
    };
    let store = open_default(&dirs, &cfg).expect("open");
    store.put(&id(SecretKind::GithubToken, "gh"), "gh", SecretBytes::new(b"t".to_vec())).await.expect("put");

    // auto with a session bus would pick the keyring for a new store, but this one was sealed by the passphrase.
    let auto = SecretsConfig { backend: BackendChoice::Auto, session_bus: true, ..cfg.clone() };
    let reopened = open_default(&dirs, &auto).expect("auto");
    assert_eq!(reopened.backend().kind, BackendKind::PassphraseFile);
    assert_eq!(reopened.get(&id(SecretKind::GithubToken, "gh")).await.expect("get").expect("v").expose(), b"t");

    let keyring = SecretsConfig { backend: BackendChoice::Keyring, ..cfg };
    let err = open_default(&dirs, &keyring).map(|_| ()).expect_err("conflicting backend");
    assert_eq!(err.code(), codes::secrets::LOCKED);
}

#[test]
fn two_russet_homes_get_two_keyring_entries() {
    let a = tempfile::tempdir().expect("a");
    let b = tempfile::tempdir().expect("b");
    let cfg = |home: &Path| SecretsConfig {
        backend: BackendChoice::Keyring,
        russet_home: Some(home.to_path_buf()),
        ..SecretsConfig::default()
    };
    // open_default never touches the keyring; the entry is named without I/O.
    let sa = open_default(&RussetDirs::under(a.path()), &cfg(a.path())).expect("a");
    let sb = open_default(&RussetDirs::under(b.path()), &cfg(b.path())).expect("b");
    let (ea, eb) = (sa.backend().keyring_entry.expect("entry a"), sb.backend().keyring_entry.expect("entry b"));
    assert!(ea.starts_with("master-key-") && eb.starts_with("master-key-"));
    assert_ne!(ea, eb);
    let no_home = SecretsConfig { backend: BackendChoice::Keyring, ..SecretsConfig::default() };
    assert_eq!(no_home.keyring_entry_name(), "master-key");
}

#[tokio::test]
async fn the_redaction_canary_never_leaves_the_store() {
    let tmp = tempfile::tempdir().expect("tmp");
    let canary_id = id(SecretKind::AppleIdPassword, "someone");
    let stores: Vec<Box<dyn SecretStore>> =
        vec![Box::new(memory::MemoryStore::new()), Box::new(passphrase_store(&tmp.path().join("s"), Some("pw")))];
    for store in stores {
        let meta = store.put(&canary_id, "label", SecretBytes::new(CANARY.as_bytes().to_vec())).await.expect("put");
        let got = store.get(&canary_id).await.expect("get").expect("present");
        let rendered = [
            format!("{meta:?}"),
            format!("{got:?}"),
            format!("{:?}", store.list(None).await.expect("list")),
            format!("{:?}", store.backend()),
            format!("{:?}", store.status().await),
        ];
        for r in rendered {
            assert!(!r.contains(CANARY), "the canary leaked: {r}");
        }
        assert_eq!(got.expose(), CANARY.as_bytes(), "the value itself is intact in-process");
    }
    let header = format!("Authorization: Bearer {CANARY}");
    assert!(!russet_secrets::redact::mask(&header).contains(CANARY));
}

#[tokio::test]
async fn memory_stores_are_independent() {
    let a = memory::MemoryStore::new();
    let b = memory::MemoryStore::new();
    a.put(&id(SecretKind::TcpToken, "t"), "t", SecretBytes::new(vec![1])).await.expect("put");
    assert!(b.get(&id(SecretKind::TcpToken, "t")).await.expect("get").is_none());
    assert_eq!(a.backend().kind, BackendKind::Memory);
    let too_big = SecretBytes::new(vec![0; russet_secrets::MAX_SECRET_BYTES + 1]);
    let err = a.put(&id(SecretKind::TcpToken, "big"), "big", too_big).await.expect_err("too big");
    assert_eq!(err.code(), codes::core::INVALID_ARGUMENT);
}
