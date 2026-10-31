# russet-car

BOM/CoreUI CAR writer and reader.

- Layer **F** (format/protocol library; may depend only on F crates) (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.
- **Owner:** W21 (B10). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- `lzfse` (ARCHITECTURE §5.1) is not declared yet: two crates carry that name with different licences, so W21 picks one with a `cargo deny check licenses` run.
