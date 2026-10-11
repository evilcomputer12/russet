# russet-xar

Read-only Apple container formats for SDK and DDI import: xar, pbzx, cpio, UDIF, HFS+.

- Layer **F** (format/protocol library; permissive; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: MIT.
- **Owner:** W16b [D 62, D 69]. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- XZ decoding uses `lzma-rust2` 0.21 (pure Rust, the alternative ARCHITECTURE §5.1 names): `liblzma` 0.4.8 cannot be linked next to apple-codesign 0.29.0, whose `apple-xar` → `xz2` → `lzma-sys` also declares `links = "lzma"`. W16b decides with its benchmark (lzma-rust2, or `xz2` sharing apple-codesign's `lzma-sys`).
- `rsa` is verify-only: RUSTSEC-2023-0071 concerns private-key operations (ARCHITECTURE §5.1); W16b adds the `deny.toml` advisory ignore with that reason when the advisories check runs.
- `apple-xar` and `cpio-archive` (MPL-2.0) are allowed only as dev-dependency oracles.
