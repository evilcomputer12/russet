<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# CI and local gates

Sources: QA §4 (CI), §10 (local resources); ARCHITECTURE [D 77] (budget and runner matrix), [D 81] (local builds);
PLAN W8.

## Workflows

| File | Runs on | Jobs |
|---|---|---|
| `.github/workflows/ci.yml` | **private phase:** pushes to `main`; non-draft pull requests labelled `ci`; weekly (Monday 03:17 UTC) and on dispatch for Windows. **Public phase** (repository variable `RUSSET_PUBLIC` = `true`): every non-draft pull request and every push to `main` | `rust-linux` (ubuntu-24.04): `cargo xtask ci --profile pr --require-tools` (fmt, clippy, nextest, deny, every guard, check-layers, check-paths, schema drift), with the commit check and the docs-only filter over the pull request or the push; zizmor's online audits; on `main`, the guard over the target directories rust-cache saves; `guard dco` on public-phase pull requests; the JUnit report through `upload-safe`. `rust-windows` (windows-2025): `cargo xtask ci --profile windows` and the cache guard |
| `.github/workflows/nightly.yml` | weekly (Sunday 02:41 UTC) in the private phase; nightly once public; dispatch | `cargo xtask ci --profile full` (every feature, doc tests, rustdoc `-D warnings`, `cargo deny check` with advisories) |
| `.github/workflows/oracle.yml`, `lab.yml`, `release.yml` | dispatch (`release.yml` also `v*` tags) | stubs that fail at once on a Linux runner, until W16 (oracle, QA-12) and W26 (release, QA-18) fill them; the lab stays local in the private phase (`cargo xtask lab`, W9, QA §4.5) |
| `.github/actions/upload-safe` | every artifact upload | refuses globs, `!`, `~` and several lines in `path`, then `cargo xtask guard apple-files secrets --artifacts <path> --if-missing <if-no-files-found>` with `.github/upload-safe.toml`, then `actions/upload-artifact` |
| `.github/dependabot.yml` | weekly | GitHub Actions (SHA pins), cargo (minor and patch, exact pins excluded), the Node harness; a 7-day cooldown |

Rules every workflow follows (QA §4.1): `permissions: {}` at the top and `contents: read` per job;
`persist-credentials: false`; third-party actions pinned to full commit SHAs with the version in a comment; no
`pull_request_target`; untrusted values reach scripts only through `env`; no `*-latest` labels, no `-large`
runners, no hosted `ubuntu-22.04`, no macOS on pushes or pull requests. `zizmor` (medium severity and above) runs as
one of the guards, offline; a separate step adds its online audits (a pinned SHA that is not its action's commit, a
known-vulnerable version) with the job's read-only token. The guards share `rust-linux` instead of having a job of
their own: every job is billed rounded up to whole minutes and pays its own setup.

**Docs-only changes** (QA §4.1). When every file changed by the push or the pull request matches
`**/*.md`, `docs/**`, `LICENSES/**`, `.github/ISSUE_TEMPLATE/**`, `.github/CODEOWNERS` or
`.github/pull_request_template.md`, `--changed-since <base>` skips fmt, clippy, the tests and deny; the guards,
check-layers, check-paths and schema drift (which checks `docs/reference/`) still run. A dispatch, a new branch or a
change set with any other file runs everything; the weekly full run covers whatever a docs change could affect.

**Caches** (QA §4.1 principle 2). rust-cache saves `~/.cargo` and the target directory's dependency artifacts (it
drops the workspace's own) after a successful job: on `main` for Linux, every run for Windows. Before that, the job
runs `cargo xtask guard apple-files` over `target/debug/{deps,build,.fingerprint}` and `~/.cargo/bin` (the installed
tools), so a failing guard also stops the save. The cargo registry and git caches are not scanned: they hold
crates.io and git archives, compressed and pinned by Cargo.lock's checksums, that anyone can download.

Not yet present, by design: `apple-smoke.yml` (W16), `apple-release.yml` (W23), `publish.yml` (W26), `ide.yml`
(W25) and the `extension` job (W14). Not planned while private: an Arch Linux container job and a Wine job
(core's CORE-15): every extra job costs included minutes the QA §4.8 plan does not have, and windows-2025 covers
Windows.

## Toolchain and tools in CI

| Tool | Version | Installed by |
|---|---|---|
| Rust | 1.99.0 (`RUSTUP_TOOLCHAIN`), minimal profile with rustfmt and clippy | `rustup toolchain install`; cached on Windows (the image ships 1.98.1) |
| cargo-nextest | 0.9.148 | taiki-e/install-action |
| cargo-deny | 0.20.2 | taiki-e/install-action |
| typos | 1.51.1 | taiki-e/install-action |
| zizmor | 1.30.1 | taiki-e/install-action |
| reuse | 6.2.0 | pipx |
| gitleaks | 8.30.1 | release tarball, SHA-256 checked |
| vscode-jsonrpc | 9.0.3 | `npm ci` in `crates/russet-testkit/node/interop` |

## Private-phase budget

GitHub Free with no payment method: 2,000 included minutes a month, consumed at list price (a Windows minute counts
about 1.67 Linux minutes, a macOS minute about 10.3), and jobs simply stop at the quota, so CI never costs money
(QA V16, decisions.md). The plan of QA §4.8 for Waves 1–2:

| Item | Volume | Linux-equivalent minutes |
|---|---|---|
| `ci.yml` Linux jobs on `main` pushes | about 60 runs × 20 min | 1,200 |
| `nightly.yml`, weekly | 4 × 45 min | 180 |
| `rust-windows`, weekly | 4 × 20 Windows min × 1.67 | 134 |
| Apple-hardware smoke (W16, owner-dispatched, once) | 15 macOS min × 10.3 | 155 |
| W16b's Windows tests | about 4 × 10 Windows min × 1.67 | 67 |
| **Total** | | **about 1,740 of 2,000** |

`cargo xtask ci-budget` measures the real month (Linux-equivalent minutes from the billing report when the token has
the `user` scope, otherwise from the month's runs and jobs, which the `repo` scope can read) and warns at 80 %
(1,600). When it warns, the lead batches merges into fewer `main` pushes; the lead's local
`cargo xtask ci --profile full` stays the merge gate, because required checks cannot be enforced on a private Free
repository. Labelling a pull request `ci` spends minutes too: use it when a change needs a clean-machine run.

## Local equivalents

`cargo xtask ci --profile pr` runs the same gates as `rust-linux` and `guard` (QA §4.9). It works with missing
optional tools: cargo-nextest falls back to `cargo test`; cargo-deny, reuse, typos, gitleaks and zizmor are
skipped with a note; the summary lists what is missing and the one install line. `--require-tools` (CI) makes a
missing tool a failure. Other flags: `--quick` (fmt, plus clippy and the unit tests of the packages changed against
`--base`: `--lib` for packages with a library, `--bins` for packages with binaries such as xtask),
`--profile full|windows`, `--changed-since <rev>` (the docs-only filter), `--skip <step>`, `--fail-fast`,
`--dry-run` (prints the steps).

## Disk tiers and target slots

`cargo xtask disk-guard` measures the filesystem of `CARGO_TARGET_DIR` (else `$HOME`) and enforces QA §10's tiers:

| Tier | Free | Allowed |
|---|---|---|
| RED | < 8 GiB | edits, docs, small unit tests |
| AMBER | 8–25 GiB | the Rust workspace in one target slot (`a`), the extension, lab Swift builds |
| GREEN | 25–45 GiB | plus the Flutter SDK; slot `b` opens |
| BLUE | ≥ 45 GiB, keeping 20 GiB | plus a local Code-OSS build |

`--need N` refuses when `free - N` would fall below the reserve (8 GiB, or 20 GiB with `--tier blue`); `--tier T`
refuses below tier `T`; `--json` prints one object. `RUSSET_DISK_GUARD_FREE_GIB` simulates a value for tests and
drills. `cargo xtask ci|test|build` run `disk-guard --tier amber --need 2` first and refuse below it; on a CI runner
(`CI` set) the tiers are advisory, because they describe the shared dev box, not an ephemeral runner.

`cargo xtask sweep` sweeps every slot (`~/.cache/russet-dev/target-*` and `CARGO_TARGET_DIR`): it skips a slot whose
cargo lock is held by a running build, runs `cargo sweep --time 7` (or a built-in equivalent based on modification
times, since `/home` is mounted `noatime`), measures the slot against the cap (6 GiB, QA-24; `--max-gib` or
`RUSSET_SWEEP_CAP_GIB`), counting a hard-linked file once as `du` does, and with `--clean-over-cap` empties the
profile directories of a slot that stays over the cap. The cargo wrappers run the time-based sweep of the current
slot by themselves once a week (stamp file `.russet-sweep` in the slot) and warn when it is over the cap; they never
clean, because cleaning a shared slot throws away every worktree's build.

**The 6 GiB cap is not met today.** A slot that several worktrees build in holds each worktree's workspace crates
next to the shared dependencies: on 2026-10-11, slot `a` (W2, W3, W8) measured 29 GiB and slot `b` 16 GiB, all of it
used within the week, so the time-based sweep removes nothing. The lead decides between a larger cap and
per-worktree slots; until then `--clean-over-cap` is the manual remedy.
