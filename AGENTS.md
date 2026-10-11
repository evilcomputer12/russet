<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Rules for agents working on Russet

This file condenses [docs/PLAN.md](docs/PLAN.md) §0, which stays authoritative. Contracts (names, APIs, wire
formats, files, error codes) are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), which is binding. Read your
card in PLAN.md completely before you start.

## 1. Worktree, branch, commits

- One worktree per card: `git -C ~/russet worktree add ../russet-wNN -b ws/wNN-<slug> main`.
- Commit only to your branch, with [Conventional Commits](https://www.conventionalcommits.org/). Only the lead
  merges to `main`. Never push or merge unless the lead asks.
- Rebase on `main` at least daily.

## 2. Paths

- Edit only the paths your card owns (PLAN §3). `.config/workstreams.toml` is the machine-readable copy;
  `cargo xtask check-paths --ws WNN` warns about foreign edits and seam files (it reads the workstream from a
  `ws/wNN-…` branch name when `--ws` is omitted).
- **Seam files are lead-only:** the root `Cargo.toml`; `crates/{russet-protocol,russetd,russet-cli}/Cargo.toml`;
  `crates/russet-protocol/src/{lib,registry,macros}.rs`; `crates/russetd/src/handlers/mod.rs`;
  `crates/russetd/src/doctor_registry.rs`; `crates/russet-cli/src/cmd/mod.rs`;
  `crates/russet-core/src/error/codes/mod.rs`; `xtask/src/main.rs`; `xtask/Cargo.toml`; `docs/ARCHITECTURE.md`.
  `extension/package.json` joins them once W14 merges it.
- Every namespace, command module, error-code family and `cargo xtask` subcommand already exists as a stub
  (freeze F0): fill your own module, never declare modules in another card's file.
- Integration tests of the shared crates (`russetd`, `russet-cli`, `russet-protocol`, `russet-core`) go into
  `tests/<area>_*.rs`, owned by the card that owns the area.
- A new third-party dependency goes into your crate's `Cargo.toml` (version from ARCHITECTURE §16.7 or a
  crates.io-checked version), licence-checked with `cargo deny check licenses`; the lead hoists it into
  `[workspace.dependencies]`.
- **Generated files** (`schemas/**`, `extension/src/generated/**`, `docs/reference/**`, `Cargo.lock`): regenerate,
  never hand-merge. On a `Cargo.lock` conflict take `main`'s copy and run `cargo metadata --format-version 1
  >/dev/null`; never resolve it by hand or with `cargo update`.

## 3. Contracts

- Follow ARCHITECTURE.md exactly: §5.3 APIs, §8 methods, §9 events, §13 error codes, §14 files.
- If a contract is wrong, open a contract-change PR that touches only `docs/ARCHITECTURE.md`, with the lead as
  reviewer; code against the current contract until it merges.

## 4. Device rule and daemon isolation

- Only the **lane-A** agent talks to the iPhone: every `russet device|app|run|debug|logs|…` command against a real
  device, idevice tools, pymobiledevice3, LLDB against the phone, `cargo xtask lab`.
- **Lane D** (everyone else) develops against `RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>`, replay traces or
  the mock device, and files device checks in `docs/lab/queue.md` (PLAN §11).
- **One daemon per worktree:** lane-D worktrees export `RUSSET_HOME=<worktree>/.russet-dev` (see `.envrc.example`).
  Development daemons refuse the `live` backend unless `RUSSET_LANE=A` is set.
- **Lane A** exports `RUSSET_LANE=A` and the persistent `RUSSET_HOME=$HOME/russet-lab/home-A`, shared by every
  lane-A card in turn.
- Tests use the testkit's `TempHome` and `RUSSET_SECRETS_BACKEND=memory` (or `passphrase`); no test touches a
  keyring entry it did not create. Live-device lock tests use the debug-only `RUSSET_LIVE_DEVICE_LOCK` and a
  test-local fake usbmuxd, never `RUSSET_LANE=A`. Never delete or bypass the live-device lock.

## 5. Mac rule

Only lane B runs `ssh mac`, read-only, with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`, in a
`mktemp -d` directory with an EXIT-trap cleanup; no installs, settings or SIP changes; light load; ask the owner
before deleting anything there; only in owner-approved windows that never overlap the Mac's VM; never touch the
VM.

## 6. Disk, downloads and build hygiene

- Before heavy work run `cargo xtask disk-guard --need <GiB>` (until W8 lands it: `df -h /home`). Tiers: RED
  < 8 GiB, AMBER 8–25, GREEN 25–45, BLUE >= 45 with a 20 GiB reserve.
- **Never download more than 500 MB without the owner's explicit OK**, whatever the tier.
- Delete your own scratch output; never touch files Russet did not create.
- Cargo target slots: `CARGO_TARGET_DIR=$HOME/.cache/russet-dev/target-<slot>` with the slot (`a` or `b`) from
  your card. At tier AMBER only slot `a` exists and every card uses it; slot `b` opens from tier GREEN, and then
  at most two cargo builds run machine-wide. Each slot is capped at 6 GiB: `cargo sweep --time 7` weekly (W8
  automates it as `cargo xtask sweep`).
- **Shared slots and the worktree guard.** Cargo names a workspace crate's artifacts by its path inside the
  workspace and decides freshness by mtime, so without a guard a worktree whose files are older than another
  worktree's last build in the same slot silently reuses that build (another branch's code, and `cargo xtask`
  inspecting the other tree). `.cargo/config.toml` sets `RUSSET_BUILD_WORKTREE` to your worktree, and every crate
  root reads it, so cargo recompiles the workspace crates when the slot last built another worktree (registry
  dependencies stay shared). Therefore:
  - every new crate root (`src/lib.rs`, a binary's `src/main.rs`, `build.rs`; in a package without a library,
    also every test, example and bench root) carries, after its doc comments,
    `const _: Option<&str> = option_env!("RUSSET_BUILD_WORKTREE");` (`cargo xtask check-layers` checks it);
  - run cargo from inside your worktree: cargo reads `.cargo/config.toml` from the current directory, so never
    build a worktree with `--manifest-path` from elsewhere;
  - expect the workspace crates to rebuild after another worktree used your slot. A per-worktree slot avoids
    that churn, but only the lead assigns one.
- `CARGO_BUILD_JOBS=6`, `CARGO_INCREMENTAL=0` (the workspace's `[profile.dev]` also disables incremental builds
  and uses `debug = "line-tables-only"`). Once W8 merges, run cargo through `cargo xtask ci|test|build`, which
  runs disk-guard first.
- RAM is shared with a llama.cpp server: never run two Swift builds at once.

## 7. Environment quirks

- The agents' Bash tool runs zsh: quote globs, never assign `GID`.
- rustup has `stable` (rustc 1.99.0) installed and no default toolchain. Inside the repository
  `rust-toolchain.toml` (`channel = "stable"`) selects it, so plain `cargo` works; elsewhere use `cargo +stable`.
  Never install a second toolchain (no `1.99.0` channel name).
- Swift 6.4 is at `/usr/lib/swift/usr/bin`; the Darwin SDK `~/.swiftpm/swift-sdks/darwin.artifactbundle` is
  read-only to Russet. Until W3 lands, Swift's LLDB needs
  `LD_LIBRARY_PATH=$HOME/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu/lib PYTHONHOME=$HOME/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu`.

## 8. Secrets and personal data

Never print or commit keys, tokens, UDIDs, team IDs, Apple IDs or device names. Tests use synthetic PKI and
redacted fixtures. Real owner artifacts may be read only by local, env-gated tests that are never committed.

## 9. Definition of done

- `cargo xtask ci --profile pr` is green: fmt, clippy `-D warnings`, nextest, deny, guard, check-layers, schema
  drift. Until W8 lands it, run the pieces: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo test --workspace`, `cargo deny check licenses bans sources`, `cargo xtask check-layers`,
  `cargo xtask check-no-apple`, `cargo xtask check-paths`.
- New methods are registered in `russet-protocol` and the schemas regenerated (`cargo xtask schema`).
- New errors have a code in your family file, a hint and a docs anchor.
- Environment-dependent features have a doctor check; CLI commands support `--json`.
- SPDX headers on every file (`GPL-3.0-or-later`; `MIT` or `MIT OR Apache-2.0` in layer F), no Apple files, no
  secrets in logs (canary test).
- The area design doc is updated where behaviour differs from it.
