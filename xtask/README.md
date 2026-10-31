# xtask

`cargo xtask <subcommand>`: code generation and repository gates (ARCHITECTURE §4).

- Layer **dev** (may depend on anything). Licence: AGPL-3.0-only.
- **Owner:** W1 created it. `Cargo.toml` and `src/main.rs` are seam files (lead-only after W1 merges, PLAN §0
  rule 2): every subcommand is registered in `main.rs` and calls its own module file, so owners only fill their
  module (and add flags to its `Args`).
- **Module owners** (PLAN §3): `layers.rs`, `paths.rs`, `no_apple.rs`, `workstreams.rs` W1; `schema.rs` W2;
  `pins.rs` W3 (Wave 1), then W26; `guard.rs`, `disk_guard.rs`, `ci.rs` (also `test` and `build`), `ci_budget.rs`,
  `sweep.rs` W8; `lab.rs` W9; `traces.rs`, `contracts.rs` W15; `golden.rs` W16; `release.rs`, `legal.rs`,
  `docs.rs` W26.
- **Implemented by W1:** `check-layers` (ARCHITECTURE §5.2 from `cargo metadata` and `.config/workstreams.toml`,
  plus the permissive-licence rule for layer F), `check-paths --ws WNN` (warns about edits outside a card's
  paths and about seam files) and `check-no-apple` (the `guard apple-files` scan). Every other subcommand
  reports that it is not implemented yet and names its owner.
