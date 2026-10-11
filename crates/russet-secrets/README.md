# russet-secrets

SecretStore over a keyring-core master key and sealed files; passphrase, env and memory stores [D 13].

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W3 (CORE-09 = SA-01). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- The keyring stores are platform dependencies: `zbus-secret-service-keyring-store` (Linux, pure-Rust crypto) and `windows-native-keyring-store` (Windows).

## What it does

- `open_default(dirs, cfg)` picks the backend from `RUSSET_SECRETS_BACKEND` (`SecretsConfig::from_env`): `keyring`
  (the master key in the OS keyring), `passphrase` (Argon2id, `RUSSET_SECRETS_PASSPHRASE`), `env` (read-only CI
  variables) or `memory` (tests). `auto` keeps the backend that sealed `data/secrets` (`store.json`); for a new
  store it takes the keyring when a session bus exists, else the passphrase store when a passphrase is set.
- Keyring and passphrase stores share one sealed layout in `data/secrets/` (0700, files 0600):
  `<sha256(id)>.sealed` (XChaCha20-Poly1305, AAD = id ‖ kind ‖ version), `index.json` (names only),
  `master.check` (detects a wrong passphrase or a replaced key before anything is overwritten), `store.json`,
  `master.salt` (passphrase only).
- The keyring entry is service `dev.russet`, user `master-key`, or `master-key-<first 8 hex of
  sha256(RUSSET_HOME)>` with `RUSSET_HOME` set (`printf %s "$RUSSET_HOME" | sha256sum | cut -c1-8`) [D 83];
  `BackendInfo.keyring_entry` names it. The key is stored base64-encoded as the entry's password.
- Nothing blocks or touches the keyring at open: keyring-core runs on blocking threads on first use, and
  `SecretStore::status()` probes it (`secrets.status`, doctor's `secrets.backend`).
- `redact`: the field-name rule and the JWT/PEM/Bearer mask of ARCHITECTURE §12.2 for the logging layer (W2).

## Tests

`cargo test -p russet-secrets` uses the passphrase and memory backends only. The one keyring test is env-gated,
creates its own uniquely named entry and deletes only that one:
`RUSSET_TEST_KEYRING=1 cargo test -p russet-secrets --test keyring_live -- --nocapture`.
