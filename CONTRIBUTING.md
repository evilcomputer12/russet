<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Contributing to Russet

Russet builds, runs, debugs and ships iOS apps from Linux and Windows with a real iPhone and no Mac. This file is
for everyone who changes the repository; AI agents also follow [AGENTS.md](AGENTS.md), and implementation agents
their card in [docs/PLAN.md](docs/PLAN.md). Contracts (names, wire formats, files, error codes) are in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), which is binding.

## Set up

- **Rust.** `rust-toolchain.toml` selects the installed `stable` toolchain (rustc 1.99) with rustfmt and clippy,
  so plain `cargo` works inside the repository; CI pins 1.99.0. Never install a second toolchain just for Russet.
- **Tools.** `cargo xtask ci` runs without them and names what is missing; install them once (about 200 MiB, Arch
  Linux; QA Appendix A):

  ```sh
  sudo pacman -S --needed cargo-nextest cargo-deny cargo-about cargo-llvm-cov git-cliff reuse typos zizmor \
    gitleaks mdbook lychee minisign xorg-server-xvfb cargo-insta cargo-fuzz cargo-machete cargo-auditable \
    msitools cargo-sweep
  ```

- **Node** (20 or newer) for the testkit's interop harness: `npm ci --prefix crates/russet-testkit/node/interop`.
- **Environment.** Copy `.envrc.example` to `.envrc` (direnv) or source it: each worktree gets its own
  `RUSSET_HOME`, daemon and keyring entry, and a cargo target slot. Only the device lane talks to the iPhone; everyone
  else uses the scripted device backend (`RUSSET_DEVICE_BACKEND=scripted:…`, see
  [docs/contributing/testing.md](docs/contributing/testing.md)).
- **Disk.** Check before heavy work: `cargo xtask disk-guard --need <GiB>` (tiers RED < 8 GiB, AMBER 8–25, GREEN
  25–45, BLUE ≥ 45 with a 20 GiB reserve). `cargo xtask sweep` keeps the target slots under 6 GiB; the cargo
  wrappers run it weekly by themselves.

## The loop

| Command | When |
|---|---|
| `cargo xtask ci --quick` | inner loop: fmt, plus clippy and unit tests of the crates you changed |
| `cargo xtask test …`, `cargo xtask build …` | cargo nextest / cargo build behind the disk guard, with `CARGO_INCREMENTAL=0` |
| `cargo xtask ci --profile pr` | before you say a change is done (the Definition of done below) |
| `cargo xtask ci --profile full` | before the lead merges: plus every feature, doc tests, rustdoc and the RustSec advisories |
| `cargo xtask guard all` | Apple files, secrets, REUSE, typos, private-track terms, workflow audit, commit messages |

What CI runs, when, and what it costs: [docs/contributing/ci.md](docs/contributing/ci.md). Every guard and how to
fix its findings: [docs/contributing/guards.md](docs/contributing/guards.md).

## Definition of done

- `cargo xtask ci --profile pr` is green: fmt, clippy `-D warnings`, tests, `cargo deny`, the guards,
  `check-layers`, `check-paths` and schema drift.
- New behaviour has tests at the lowest level that can catch it; tests assert error codes, never message text.
- New JSON-RPC methods are registered in `russet-protocol` and the schemas regenerated (`cargo xtask schema`).
- New errors have a code in their family file, a hint and a docs anchor; CLI commands support `--json`.
- Environment-dependent features have a doctor check.
- Every file has SPDX headers (`GPL-3.0-or-later`; `MIT` or `MIT OR Apache-2.0` in layer F; `0BSD` for templates,
  fixtures and sample apps). Files that cannot carry a comment get a `<file>.license` sidecar.
- The area design doc is updated where behaviour differs from it.

## Branches, commits, pull requests

- **Branches.** Implementation work happens on `ws/wNN-<slug>` branches in their own worktree
  (`git -C ~/russet worktree add ../russet-wNN -b ws/wNN-<slug> main`). Only the lead merges to `main`. Rebase on
  `main` at least daily.
- **Paths.** Change only the paths your card owns; `cargo xtask check-paths` warns about anything else. Seam files
  (the root `Cargo.toml`, `xtask/src/main.rs`, `docs/ARCHITECTURE.md`, …) are lead-only. A contract change is its
  own pull request that touches only `docs/ARCHITECTURE.md`.
- **Commit messages** follow [Conventional Commits 1.0](https://www.conventionalcommits.org/en/v1.0.0/):
  `type(scope): description`, with `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`,
  `chore` or `revert`, and `!` for breaking changes. `cargo xtask guard commits` checks the branch; the changelog
  is generated from them.
- **Pull requests** use the template: what and why, how it was tested, the checklist. Label a pull request `ci` to
  run Linux CI on it while the repository is private ([docs/contributing/ci.md](docs/contributing/ci.md)).
- **Review.** `.github/CODEOWNERS` names the owner for secrets, signing, agent policy, CI, licences and packaging.

## Developer Certificate of Origin

Russet uses the [Developer Certificate of Origin 1.1](https://developercertificate.org/) instead of a CLA. By
adding

```text
Signed-off-by: Your Name <your.email@example.org>
```

to a commit (`git commit -s`) you certify that you wrote the change or otherwise have the right to submit it under
the project's licence. Inbound licence equals outbound licence: GPL-3.0-or-later (layer-F crates MIT or
MIT OR Apache-2.0; 0BSD for `templates/` and `tests/fixtures/`). `cargo xtask guard dco` checks every commit of a
branch; CI enforces it on pull requests once the repository is public (PLAN §5.9). Until then the repository has
no outside contributors.

## No Apple files, no secrets, no device identifiers

- **Never commit Apple files**: SDKs, Developer Disk Images, `.xip`, `.ipsw`, `.dmg`, `.tbd`, `.mobileprovision`,
  `.p8`, `.p12`, Mach-O binaries or asset catalogs produced by Apple tools, or files with Apple copyright headers.
  `cargo xtask guard apple-files` catches them. Tests build Apple-format inputs at test time from Rust builders;
  the few generated files that cannot be built that way live only under the provenance roots, each listed in its
  `PROVENANCE.toml` with its SHA-256 and the committed script that generated it from open tools
  ([docs/contributing/guards.md](docs/contributing/guards.md)).
- **Never commit or print** keys, tokens, passwords, UDIDs, ECIDs, serial numbers, team IDs, Apple IDs or device
  names. Tests use synthetic PKI and redacted fixtures; synthetic UDIDs use chip id `0000`. Keep the real values in
  your local denylist (`~/.config/russet/private-identifiers`, one per line), which `cargo xtask guard secrets`
  checks without ever printing them.
- **Device logs and reports.** Attach the output of `russet doctor --report --redact`, never raw logs, pair records,
  provisioning profiles or crash reports of other apps.

## Clean-room rules

Russet interoperates with Apple's protocols and formats; it does not copy Apple code.

- Write down **behaviour** only: what goes over the wire, what a file contains, what a tool does with which input.
  Record the method, the oracle (device, Xcode and OS versions) and the inputs.
- **No** disassembly listings, decompiled code, or excerpts of SDK headers, Xcode resources or other Apple files in
  the repository, in issues or in pull requests.
- Invasive reverse engineering happens only in a disposable Mac VM, never on the owner's Mac; Apple binaries and raw
  oracle outputs never enter the repository or CI artifacts (`upload-safe` guards every upload).
- Third-party code is used only under a licence `cargo deny` accepts; AGPL projects and unlicensed repositories are
  reference only, never code (ARCHITECTURE §19).

## Security

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md), never in a public issue.

## Conduct

Everyone in the project follows the [Code of Conduct](CODE_OF_CONDUCT.md).
