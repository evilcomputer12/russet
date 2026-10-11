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
| `secrets` | PEM and PGP private-key blocks, AWS key ids, GitHub and Slack tokens, Google API keys, JWTs, real-looking UDIDs, your private identifiers; minus the reviewed synthetic values | built in, plus gitleaks over the branch's history when installed |
| `reuse` | files without SPDX copyright and licence information, licences missing from `LICENSES/` | reuse |
| `typos` | misspellings, minus the reviewed vocabulary in `typos.toml` | typos |
| `public-terms` | the private track's identifying terms outside the allowlist | built in |
| `zizmor` | GitHub Actions findings of medium severity or above; `--zizmor-online` adds the online audits (needs `GH_TOKEN`) | zizmor |
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

The built-in scan looks for shapes, not mentions: a `BEGIN … PRIVATE KEY` (or PGP `PRIVATE KEY BLOCK`) header needs
a body of 64 characters and the closing `KEY-----` to count, on several lines or on one line with `\n` escapes, as
gitleaks sees it; an `AKIA` prefix needs its 16 characters. Modern UDIDs (`0000XXXX-` plus 16 hex digits) are
reported unless they are synthetic by convention: chip id `00000000` (no Apple chip has it; the testkit's scenario
devices) or an all-zero ECID (the protocol tests). Use one of those for new test devices.

gitleaks, when installed (CI installs it), scans the history of the checked-out branch (`git log HEAD`, not the
other branches a shared local repository holds), so a value stays a finding after a later commit removes it. Its
JSON report goes to a private temporary directory and is deleted at once; findings name the file, line, rule and
commit, never the value.

**Reviewed synthetic values.** A test value that only looks secret (a test JWT, a test bearer token, a UDID with a
hand-typed ECID) gets an entry in `.github/synthetic-secrets.toml`: the SHA-256 of the value (upper-cased for UDIDs;
for gitleaks, of the secret it reports), its owner and why it is synthetic. Both the built-in scan and gitleaks skip a
listed value wherever it appears, in the tree and in the history, so an entry survives rebases and merges. To add one,
run `cargo xtask guard secrets --suggest-allowlist` on your machine: it prints a ready entry per finding (it refuses
to run on CI, because the digest of a real identifier can be brute-forced). List only values you checked are
synthetic; the file is a review item for the owner (`.github/CODEOWNERS`). A `gitleaks:allow` (or
`russet-guard:allow`) marker on a line still hides it from the secret rules, but only from the commit that adds the
marker onwards, so it cannot clear a value already in the history.

Keep the values that must never appear (your phone's UDID and name, your Apple ID, your team id) in
`~/.config/russet/private-identifiers` (or `$XDG_CONFIG_HOME/russet/private-identifiers`; `--private-identifiers
FILE` overrides it), one per line, `#` comments allowed. The guard reports `private-identifier` at the line and
column and never prints the value; no allow marker and no allowlist entry hides it. The file stays on your
machine.

## Private-track terms

The repository goes public at G1 (Q6, Q34); the virtual-iPhone track stays private (ARCHITECTURE §19 [D 91]).
`public-terms` compares the SHA-256 of every lowercased token and every pair of adjacent tokens (joined by one
space, across line breaks) of every file and file name with the digests in `PUBLIC_TERM_DIGESTS`
(`xtask/src/guard.rs`). The plaintext list lives only in the private repository; the lead produces the digests
without committing it:

```sh
cargo xtask guard --hash-terms < private-terms.txt     # one term per line; prints one digest per line
```

A term is one token or two: a longer term could never match, so `--hash-terms` refuses it and names its line (never
the term); split it into tokens or pairs. `--term-digests FILE` adds digests for a trial run. Until the lead fills
`PUBLIC_TERM_DIGESTS`, the check prints a warning that it has nothing to compare. Q34 is answered (decisions.md keeps
one neutral line), so the allowlist holds only `docs/research/macos-on-x86-feasibility-2026-10.md` until PLAN §5.9
item 4 decides that file; it must be empty before the repository goes public (§5.9 item 1).

## Uploads (`--artifacts`)

`cargo xtask guard apple-files secrets --artifacts PATH` scans one file or directory per flag (CI uploads, the CI
cache) instead of the tree. The `upload-safe` composite action runs it before every `actions/upload-artifact`, with
`--upload-allowlist .github/upload-safe.toml --workflow <file> --job <id>`. The allowlist admits a file only when
every reason it was flagged is Mach-O magic and its path matches an entry for that workflow and job; today that is
only `apple-release.yml`'s own RussetProbe binary and dSYM (W23). Every other Apple format stays blocked everywhere.

The scan sees what the upload would carry:

- it follows symlinks, as upload-artifact's globber does, and checks a linked file under the link's own path (a
  symlink loop is followed once; a dangling link is a note);
- it refuses a path with glob characters (`* ? [ ] { }`), a `!` exclusion, a leading `~` or several lines, which
  upload-artifact would expand past the guard: the action fails before uploading, so pass one plain path per call;
- a path that does not exist fails, unless `--if-missing warn|ignore` (the action passes its `if-no-files-found`,
  default `warn`): upload-artifact uploads nothing for it either.

## REUSE and typos

- `reuse` lints a temporary copy of the guarded file set, because `reuse lint` on its own descends into some
  git-ignored directories (the Node harness's `node_modules`). Files that cannot carry a comment get a
  `<file>.license` sidecar; generated files are annotated in `REUSE.toml`. Spell an SPDX tag inside a test string in
  pieces (`concat!("SPDX-", "License-Identifier: …")`) so reuse does not read it as the file's own.
- `typos.toml` lists reviewed vocabulary only (UDID, BUID, ITMS, `s2k_fo`, `DT_RELA`, …), each with its reason,
  preferring exact identifiers over global words. Never add an English misspelling.
