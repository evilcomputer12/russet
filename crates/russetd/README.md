# russetd

The per-user daemon (ARCHITECTURE §3, §6): lifecycle, `DaemonCtx`, handlers, pipelines, permissions, approvals,
jobs, leases, subscriptions, the web endpoint and the audit log. One per OS user (one per worktree with
`RUSSET_HOME`), autostarted by any client.

- Layer **L4** (may depend on anything; ARCHITECTURE §5.1–§5.2). Licence: AGPL-3.0-only.
- **Owner:** W2 (CORE-06 and the runtime modules). W1 created the crate as a compiling stub (freeze F0).
- **Seam files** (lead-only after W1 merges, PLAN §0 rule 2): `Cargo.toml`, `src/handlers/mod.rs` (the fixed
  namespace list) and `src/doctor_registry.rs` (the fixed list of crates' `checks()`).
- **Handler files** `src/handlers/<namespace>.rs` belong to the namespace's area (PLAN §3); the `flutter`
  namespace is split into `flutter_build.rs` (W17) and `flutter_run.rs` (W18). Every method answers
  `core.notImplemented` (1005) until its owner replaces the handler.
- **Pre-created module files:** `src/pipeline/mod.rs` (W7), `src/web/mirror.rs` (W10), `src/web/mcp.rs` (W20).
- Integration tests of a card go into `tests/<area>_*.rs` (for example `tests/pipeline_*.rs` for W7).
