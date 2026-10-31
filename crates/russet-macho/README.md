# russet-macho

Mach-O parsing, load commands, string-pool alignment, code-signature blobs, rpath edits.

- Layer **F** (format/protocol library; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.
- **Owner:** W6 (SA-03, the union API of ARCHITECTURE §5.3). W1 created the crate as a compiling stub (freeze F0, PLAN §12).

## What it does

- `MachO::parse` reads thin and fat files (32/64-bit, little-endian images, `FAT_MAGIC` and `FAT_MAGIC_64`) with
  bounds-checked reads: malformed input is an error, never a panic. Per slice (`Image`): `LC_BUILD_VERSION` (or
  `LC_VERSION_MIN_*`), UUID, rpaths, dylibs, install name, `LC_ENCRYPTION_INFO(_64)`, `LC_SYMTAB`, segments, header
  padding, and the `LC_CODE_SIGNATURE` superblob. The single-valued accessors describe the arm64 slice (then
  arm64e, then the first). `Arch` keeps ARCHITECTURE §5.3's three variants; slices of other CPUs are described by
  `Cpu` and listed by `cpu_slices()` (`slices()` lists the `Arch` slices).
- `CodeSignature` parses every code directory (version, flags, hash type, page size, identifier, team, code limit,
  exec-segment fields, runtime, special and code slot hashes, CDHash), the requirement set, XML and DER
  entitlements and the CMS wrapper; `CodeDirectory::mismatched_pages` re-hashes the code pages.
- In-place edits on every slice (the fat header is rebuilt when a slice changes size; files are replaced
  atomically and keep their mode): `delete_rpath`, `align_string_pool` (iOS 27's 8-byte rule: the string table
  moves to the next 8-byte boundary and everything after it in `__LINKEDIT` moves by 16 bytes, so it keeps its
  alignment), `thin`, and the last-resort `patch_sdk_version` (never the default, ARCHITECTURE §16.4).
- The `fixture` feature builds synthetic images for tests (optionally linker-signed like lld), so no Mach-O file
  is committed (ARCHITECTURE §19).

Its own parser replaces goblin (ARCHITECTURE §5.1 allows goblin or object): the edits need raw load-command offsets
and neither crate covers the code-signature blobs.
