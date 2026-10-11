# russet-sign

Bundle walk, provisioning profiles, entitlements, signing through apple-codesign, verify, lints.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W6 (SA-04, SA-05). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- `apple-codesign` 0.29.0 with `default-features = false` (ARCHITECTURE §16.7). Its `KeyInfoSigner` trait comes from `x509-certificate` 0.24, which apple-codesign does not re-export; `russet_sign::KeyInfoSigner` re-exports it.
