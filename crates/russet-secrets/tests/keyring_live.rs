// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! The one test that touches the OS keyring (PLAN §4.3, ARCHITECTURE §3.3 item 7) [D 83]. It runs only with
//! `RUSSET_TEST_KEYRING=1`, creates its own uniquely named entry (`russet-test-<pid>-<nanos>` in service
//! `dev.russet`), round-trips a 4 KiB secret sealed under that entry's master key, and deletes only that entry,
//! also when an assertion fails.
//!
//! ```sh
//! RUSSET_TEST_KEYRING=1 cargo test -p russet-secrets --test keyring_live -- --nocapture
//! ```

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use russet_secrets::keyring::KeyringKeySource;
use russet_secrets::sealed::{MasterKeySource, SealedStore};
use russet_secrets::{BackendKind, SecretBytes, SecretId, SecretKind, SecretStore};

#[test]
fn a_4_kib_secret_round_trips_through_the_os_keyring() {
    if std::env::var_os("RUSSET_TEST_KEYRING").is_none_or(|v| v != "1") {
        eprintln!("skipped: set RUSSET_TEST_KEYRING=1 to run the live keyring test");
        return;
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let entry = format!("russet-test-{}-{nanos}", std::process::id());
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("secrets");

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        rt.block_on(async {
            let store = SealedStore::new(dir.clone(), Box::new(KeyringKeySource::new(entry.clone())));
            let status = store.status().await;
            assert!(status.available && !status.locked, "keyring not usable: {status:?}");
            assert_eq!(store.keyring_entry().as_deref(), Some(entry.as_str()));
            let secret: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
            let id = SecretId { kind: SecretKind::AscKey, name: "live-test".into() };
            let meta = store.put(&id, "live keyring test", SecretBytes::new(secret.clone())).await.expect("put");
            assert_eq!((meta.backend, meta.size_bytes), (BackendKind::Keyring, 4096));
            // A second store over the same directory and entry: the master key comes back from the keyring.
            let again = SealedStore::new(dir.clone(), Box::new(KeyringKeySource::new(entry.clone())));
            assert_eq!(again.get(&id).await.expect("get").expect("value").expose(), &secret[..]);
        });
    }));

    // Delete only the entry this test created, whatever happened above (outside any runtime: keyring-core blocks).
    let source = KeyringKeySource::new(entry.clone());
    let deleted = source.delete_entry();
    let gone = source.load();
    if let Err(panic) = outcome {
        resume_unwind(panic);
    }
    assert!(matches!(deleted, Ok(true)), "cleanup of {entry}: {deleted:?}");
    assert!(matches!(gone, Ok(None)), "the entry is gone");
    eprintln!("live keyring round trip passed; entry {entry} created and deleted");
}
