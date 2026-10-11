# russet-testkit

Scripted, replay and recording connectors, mock usbmuxd, cassettes, HTTP fakes, temp-home daemon fixture, RDT traces.

- Layer **dev** (tests and tooling; never a normal dependency of a shipped crate) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W8 (CORE-15, QA); `src/debug/` W7. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Tests of an L3 crate that use the testkit are integration tests under that crate's `tests/`, never `#[cfg(test)]` unit modules (ARCHITECTURE §5.1).
- Pre-created module file: `src/debug/mod.rs` (W7).
