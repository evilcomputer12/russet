<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Repository guards

`cargo xtask guard [CHECK…]` (default `all`) protects the repository and every CI upload (QA-02; ARCHITECTURE §12.2,
§19). The guards read the files git would commit: tracked files plus untracked files that are not ignored.

| Check | Fails on | Tool |
|---|---|---|
| `apple-files` (alias `check-no-apple`) | Mach-O (thin or fat), BOMStore/CAR, NIBArchive, xar, UDIF `koly`, IMG4 and pbzx magic; the extensions `.tbd`, `.xip`, `.ipsw`, `.dmg`, `.mobileprovision`, `.p8`, `.p12`; Apple copyright headers | built in |
| `secrets` | PEM private-key blocks, AWS key ids, GitHub and Slack tokens, Google API keys, JWTs, real-looking UDIDs, your private identifiers | built in, plus gitleaks when installed |
| `reuse` | files without SPDX copyright and licence information, licences missing from `LICENSES/` | reuse |
| `typos` | misspellings, minus the reviewed vocabulary in `typos.toml` | typos |
| `public-terms` | the private track's identifying terms outside the allowlist | built in |
| `zizmor` | GitHub Actions findings of medium severity or above | zizmor |
| `commits` | commit subjects of the branch that are not Conventional Commits | built in |
| `dco` | commits of the branch without `Signed-off-by:`; not part of `all` during the private phase | built in |

A missing external tool skips its check with a note; `--require-tools` (CI) makes it a failure. `--base REF` sets
the commit range for `commits` and `dco` (default: the merge base with `main`). Findings never print a secret's value
or a private term.

## Apple files and the provenance allowlist

Tests build Apple-format inputs at test time, in temporary directories, from Rust builders (ARCHITECTURE §19
[D 88]). Where that is impractical (UDIF and HFS+ images, fuzz seed corpora), generated files may be committed under
exactly two roots, compiled into `xtask/src/guard.rs`: `tests/fixtures/xip/` and `fuzz/russet-xar/corpus/`. A new
root is a lead change. Each root has a `PROVENANCE.toml`:

```toml
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Russet contributors
schema = 1

[[file]]
path = "udif/zero-fill.dmg"                        # relative to this directory
sha256 = "<64 lowercase hex digits of the committed bytes>"
generator = "tests/fixtures/xip/gen/make-udif.sh"  # repository-relative, committed
description = "UDIF image with one zero-fill chunk, generated from open tools"
```

A flagged file under a root passes only when it is listed, its SHA-256 matches, and its generator script exists and
is committed. Anything else under a root is reported with the reason (`not listed`, `its SHA-256 … differs`, `its
generator … is not committed`). Regenerating a fixture means updating its hash in the same commit.

## Secrets and private identifiers

The built-in scan looks for shapes, not mentions: a `BEGIN … PRIVATE KEY` header needs a key body and an `END` line to
count, an `AKIA` prefix needs its 16 characters. Modern UDIDs (`0000XXXX-` plus 16 hex digits) are reported unless
they are synthetic: chip id `0000` (no Apple chip has it; the testkit's scenario devices) or an all-zero ECID (the
protocol tests). A reviewed false positive can carry `gitleaks:allow` on its line.

Keep the values that must never appear (your phone's UDID and name, your Apple ID, your team id) in
`~/.config/russet/private-identifiers` (or `$XDG_CONFIG_HOME/russet/private-identifiers`; `--private-identifiers
FILE` overrides it), one per line, `#` comments allowed. The guard reports `private-identifier` at the line and
column and never prints the value. The file stays on your machine.

## Private-track terms

The repository goes public at G1 (Q6, Q34); the virtual-iPhone track stays private (ARCHITECTURE §19 [D 91]).
`public-terms` compares the SHA-256 of every lowercased token and every pair of adjacent tokens (joined by one
space, across line breaks) of every file and file name with the digests in `PUBLIC_TERM_DIGESTS`
(`xtask/src/guard.rs`). The plaintext list lives only in the private repository; the lead produces the digests
without committing it:

```sh
cargo xtask guard --hash-terms < private-terms.txt     # one term per line; prints one digest per line
```

`--term-digests FILE` adds digests for a trial run. Until the lead fills `PUBLIC_TERM_DIGESTS`, the check prints a
warning that it has nothing to compare. The allowlist holds only the two files Q34 covers,
`docs/decisions.md` and `docs/research/macos-on-x86-feasibility-2026-10.md`, and must be empty before the repository
goes public (PLAN §5.9).

## Uploads (`--artifacts`)

`cargo xtask guard apple-files secrets --artifacts DIR` scans directories (CI uploads, caches) instead of the tree.
The `upload-safe` composite action runs it before every `actions/upload-artifact`, with
`--upload-allowlist .github/upload-safe.toml --workflow <file> --job <id>`. The allowlist admits a file only when
every reason it was flagged is Mach-O magic and its path matches an entry for that workflow and job; today that is
only `apple-release.yml`'s own RussetProbe binary and dSYM (W23). Every other Apple format stays blocked everywhere.
A path that does not exist is a note, not a failure.

## REUSE and typos

- `reuse` lints a temporary copy of the guarded file set, because `reuse lint` on its own descends into some
  git-ignored directories (the Node harness's `node_modules`). Files that cannot carry a comment get a
  `<file>.license` sidecar; generated files are annotated in `REUSE.toml`. Spell an SPDX tag inside a test string in
  pieces (`concat!("SPDX-", "License-Identifier: …")`) so reuse does not read it as the file's own.
- `typos.toml` lists reviewed vocabulary only (UDID, BUID, ITMS, `s2k_fo`, `DT_RELA`, …), each with its reason,
  preferring exact identifiers over global words. Never add an English misspelling.
