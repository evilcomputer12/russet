# russet-ipa

Deterministic IPA writer and reader (build §4.8, B07) [D 19].

- Layer **F** (format/protocol library; permissive; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: MIT.
- **Owner:** W5 (B07).
- `write(app, out, IpaOptions { mode, swift_support, epoch })`: `Payload/<App>.app/…`, entries sorted by bytes with
  explicit directory entries, one fixed timestamp (`SOURCE_DATE_EPOCH`, else 2001-01-01), Unix modes from the
  Mach-O magic (755) or 644, symlinks 120777, `dev` stored or `store` deflate-6, `SwiftSupport/iphoneos/` for store
  IPAs, Zip64 for files of 4 GiB or more, written atomically; the report carries the SHA-256.
- `read_listing`, `read_entry` and `app_dir` serve tests, `build.inspect` and the device plane.
- Never runs the `zip` or `unzip` binaries (the tests cross-check with `unzip -Z` when it is installed).
