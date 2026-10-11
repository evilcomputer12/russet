# russet-secrets

SecretStore over a keyring-core master key and sealed files; passphrase, env and memory stores [D 13].

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W3 (CORE-09 = SA-01). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- The keyring stores are platform dependencies: `zbus-secret-service-keyring-store` (Linux, pure-Rust crypto) and `windows-native-keyring-store` (Windows).
