<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# CI and local gates

Sources: QA §4 (CI), §10 (local resources); ARCHITECTURE [D 77] (budget and runner matrix), [D 81] (local builds);
PLAN W8.

## Workflows

| File | Runs on | Jobs |
|---|---|---|
| `.github/workflows/ci.yml` | **private phase:** pushes to `main`; non-draft pull requests labelled `ci`; weekly (Monday 03:17 UTC) and on dispatch for Windows. **Public phase** (repository variable `RUSSET_PUBLIC` = `true`): every non-draft pull request and every push to `main` | `rust-linux` (ubuntu-24.04): `cargo xtask ci --profile pr --require-tools` (fmt, clippy, nextest, deny, every guard, check-layers, check-paths, schema drift), with the commit check over the pull request or the push; `guard dco` on public-phase pull requests; the JUnit report through `upload-safe`. `rust-windows` (windows-2025): `cargo xtask ci --profile windows` |
| `.github/workflows/nightly.yml` | weekly (Sunday 02:41 UTC) in the private phase; nightly once public; dispatch | `cargo xtask ci --profile full` (every feature, doc tests, rustdoc `-D warnings`, `cargo deny check` with advisories) |
| `.github/actions/upload-safe` | every artifact upload | `cargo xtask guard apple-files secrets --artifacts <path>` with `.github/upload-safe.toml`, then `actions/upload-artifact` |
| `.github/dependabot.yml` | weekly | GitHub Actions (SHA pins), cargo (minor and patch, exact pins excluded), the Node harness; a 7-day cooldown |

Rules every workflow follows (QA §4.1): `permissions: {}` at the top and `contents: read` per job;
`persist-credentials: false`; third-party actions pinned to full commit SHAs with the version in a comment; no
`pull_request_target`; untrusted values reach scripts only through `env`; no `*-latest` labels, no `-large`
runners, no hosted `ubuntu-22.04`, no macOS on pushes or pull requests. `zizmor` (medium severity and above) runs as
one of the guards. The guards share `rust-linux` instead of having a job of their own: every job is billed rounded
up to whole minutes and pays its own setup.

Not yet present, by design: `oracle.yml`, `apple-smoke.yml` (W16), `apple-release.yml` (W23), `release.yml`,
`publish.yml` (W26), `ide.yml` (W25), the `extension` job (W14) and the lab (local only, `cargo xtask lab`, W9).

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
missing tool a failure. Other flags: `--quick`, `--profile full|windows`, `--skip <step>`, `--fail-fast`,
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

`cargo xtask sweep` keeps every slot (`~/.cache/russet-dev/target-*` and `CARGO_TARGET_DIR`) under 6 GiB: it skips a
slot whose cargo lock is held by a running build, runs `cargo sweep --time 7` (or a built-in equivalent based on
modification times, since `/home` is mounted `noatime`), and with `--clean-over-cap` cleans a slot that stays over
the cap. The cargo wrappers run the time-based sweep of the current slot by themselves once a week (stamp file
`.russet-sweep` in the slot).
