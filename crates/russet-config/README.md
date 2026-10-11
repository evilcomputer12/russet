# russet-config

Typed config, layering, comment-preserving editor, schemas, watcher.

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W3 (CORE-08). W1 created the crate as a compiling stub (freeze F0, PLAN §12).

## What it does

- Typed models of every file of ARCHITECTURE §14: `russet.toml`/`russet.local.toml` (`project`), the global
  `config.toml` (`global`), `accounts.toml` (`accounts`), `mcp-policy.toml` and `provision-policy.toml` (`policy`).
  Keys are kebab-case, and unknown keys and invalid values are errors with `path:line:col` (`config.unknownKey`,
  `config.invalid`).
- The precedence of §14.1 (`effective`): `RUSSET_*` variables, `russet.local.toml`, `russet.toml`, `config.toml`,
  built-in defaults; CLI flags travel in RPC params. A project's `[agents]` only narrows `mcp-policy.toml`, and
  `russet.local.toml` cannot widen `russet.toml` either.
- `ConfigEditor` edits one file with toml_edit, keeping comments and order, coerces values to the schema's type
  (`"2"` → `2`), validates the whole file before saving, and saves atomically.
- `ConfigService` (`DaemonCtx.config`) caches files by modification time, never refuses to start on an invalid
  file (doctor and `effective()` report it), and sends `ConfigChanged` for Russet's own edits and, through the
  watcher, for edits on disk.
- JSON Schemas: `schema::schema(ConfigFile)`; the committed copies in `schemas/config/` are checked by
  `tests/schemas.rs` and regenerated with `RUSSET_UPDATE_SCHEMAS=1 cargo test -p russet-config --test schemas`.
