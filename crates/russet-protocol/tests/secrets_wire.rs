// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `secrets.*` wire shapes (ARCHITECTURE §8.1): metadata only, and `secrets.set` takes a file, never a value.

use russet_protocol::secrets::{SecretBackend, SecretKind, SecretMeta, SecretsSetParams, SecretsStatusResult};
use russet_protocol::{Permission, registry};
use serde_json::json;

#[test]
fn secrets_methods_have_their_classes() {
    let class = |name: &str| registry().iter().find(|m| m.name == name).map(|m| m.permission);
    assert_eq!(class("secrets.status"), Some(Permission::Read));
    assert_eq!(class("secrets.list"), Some(Permission::Read));
    assert_eq!(class("secrets.set"), Some(Permission::Secrets));
    assert_eq!(class("secrets.delete"), Some(Permission::Secrets));
}

#[test]
fn shapes_carry_no_value() {
    let meta = SecretMeta {
        id: "asc-key/K".into(),
        kind: SecretKind::AscKey,
        name: "K".into(),
        label: "ASC".into(),
        backend: SecretBackend::Passphrase,
        created_at: "2026-10-11T00:00:00Z".into(),
        size_bytes: 257,
    };
    assert_eq!(
        serde_json::to_value(&meta).expect("ser"),
        json!({"id": "asc-key/K", "kind": "asc-key", "name": "K", "label": "ASC", "backend": "passphrase",
               "createdAt": "2026-10-11T00:00:00Z", "sizeBytes": 257})
    );
    let set: SecretsSetParams =
        serde_json::from_value(json!({"id": "github-token/me", "fromFile": "/tmp/t"})).expect("set");
    assert_eq!(set.from_file, "/tmp/t");
    let status = SecretsStatusResult {
        backend: SecretBackend::Keyring,
        available: true,
        locked: false,
        reason: None,
        keyring_entry: Some("master-key-0123abcd".into()),
    };
    assert_eq!(
        serde_json::to_value(&status).expect("ser"),
        json!({"backend": "keyring", "available": true, "locked": false, "keyringEntry": "master-key-0123abcd"})
    );
}
