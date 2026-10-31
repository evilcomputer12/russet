# russet-core

Layer **L0** (ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.

- **Owner:** W2 (CORE-02). W1 created the skeleton: the `ErrorCode` type, every error-code constant of
  ARCHITECTURE §13.3 and the `rpc` traits (freeze F0).
- **Family files** `src/error/codes/<family>.rs` belong to the workstream that owns the family
  (ARCHITECTURE §13.2, PLAN §3); `src/error/codes/mod.rs` is a seam file (lead-only after W1 merges).
- **Contract:** ARCHITECTURE §5.3 `russet-core`.
