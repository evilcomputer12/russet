# russet-cli

The `russet` binary (ARCHITECTURE §3.1, §8.7): one multi-call executable that is the CLI, the daemon
(`russet daemon run`, or the file invoked as `russetd`), the MCP bridge (`russet mcp serve`) and the Flutter
custom device (`russet flutter device <op>`).

- Layer **L4** (may depend on L0–L2, russetd, russet-agent-config and russet-mcp; ARCHITECTURE §5.2).
  Licence: AGPL-3.0-only.
- **Owner:** W2 (CORE-07: `main.rs`, `lib.rs`, `global.rs`, `render/`, `spawn.rs`). W1 created the crate as a
  compiling stub (freeze F0).
- **Seam files** (lead-only after W1 merges, PLAN §0 rule 2): `Cargo.toml` and `src/cmd/mod.rs`, which fixes the
  list of command modules.
- **Command modules** `src/cmd/<area>.rs` belong to their area (PLAN §3). Each exports
  `pub fn command() -> clap::Command` and `pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode>`; an
  area with several top-level commands (for example `app`, `install` and `launch`) returns them as a
  `cmd::group`. Every command reports `core.notImplemented` until its owner fills it.
