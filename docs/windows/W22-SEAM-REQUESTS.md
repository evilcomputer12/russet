<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# W22 seam and ownership requests

W22 did not edit any lead-only seam file. These are the exact changes it needs from the lead, plus the foreign
(non-seam) files it touched for review.

## 1. Seam files (lead edits)

### 1.1 Root `Cargo.toml`: hoist the new third-party dependencies

W22 declared these directly in its crates (all already in `Cargo.lock`, licence-checked with `cargo deny`):

```toml
# [workspace.dependencies]
base64 = "0.22.1"                                              # russet-gsa, russet-anisette, russet-adi-host
cbc = "0.2.1"                                                  # russet-gsa (crate adds features = ["alloc"])
libc = "0.2.190"                                               # russet-adi-host (Linux only)
wiremock = "0.6.5"                                             # russet-gsa `fake` feature (optional), dev elsewhere
```

### 1.2 `crates/russetd/Cargo.toml`: dev-dependencies for daemon-level Apple ID tests

```toml
[dev-dependencies]
russet-anisette = { workspace = true, features = ["fake"] }
russet-gsa = { workspace = true, features = ["fake"] }
```

With these, a `crates/russetd/tests/account_apple_id.rs` (W22) can drive `account.appleId.*` over IPC against
`GsaFake` + `AnisetteFake` (registering a pre-configured `russet_account::free::AppleIds` with `AppleIds::register`).
Today the handlers are thin mappers over `AppleIds`, whose mocked suite lives in
`crates/russet-account/src/free/tests.rs`.

### 1.3 `docs/ARCHITECTURE.md` (contract updates; code already follows them)

- **§8.4 rows** (additive):
  - `account.appleId.begin`: params `{appleId, password, anisette?: "local" | {remote: url}, label?}`; result
    `{sessionId, step: done|two-factor, twoFactor?: {via: (trusted-device|sms)[], codeSentTo?, phoneNumbers:
    [{id, number}], codeLength}, account?: Account}` (`account` when `step` is `done`).
  - `account.appleId.requestCode`: `{sessionId, via: trusted-device|sms, phoneId?}` (`phoneId` required for sms).
  - `account.anisette.status`: result adds `localSupported` and `pinned?`.
  - `account.anisette.fetchLibraries`: result adds `pinned` (false = trusted on first use).
- **§5.2 [D 72]**: `anisette` is an *untagged* `"local" | {"remote": url}` (like `accounts.toml`), which the wire-enum
  rule accepts; worth one sentence so nobody "fixes" it into an internally tagged enum.
- **§13.3 codes** (in `codes/{gsa,anisette}.rs`): `5254 gsa.codeRateLimited`, `5255 gsa.protocol`,
  `5256 gsa.rootPinMismatch`, `5257 gsa.sessionExpired`, `5258 gsa.loginExpired`, `5259 gsa.ds2Failed`,
  `5273 anisette.consentRequired`, `5274 anisette.fetchFailed`, `5275 anisette.remoteFailed`,
  `5276 anisette.unsupported`, `5277 anisette.provisioningFailed`.
- **§14 files**: `<data>/anisette/adi/` (the helper's provisioning directory, 0700, `adi.pb` mirrored into
  `secret:anisette-state/local`), `<data>/anisette/libs/<apkver>/<abi>/tofu.json` (hashes recorded on first use),
  `<data>/gsa/AppleIncRootCertificate.cer` (pinned cache), `<data>/accounts/<team>/{ledger,bundle-map}.json`
  (free-team policy).
- **§17 / doctor**: new check `anisette.local`.
- **§3.1 sandbox**: the seccomp filter is a deny-list today (network, exec, ptrace, namespaces, BPF, modules,
  keyrings, io_uring); Landlock is applied when the kernel has it (the WSL kernel does not). An allowlist needs a
  syscall trace of the real libraries (follow-up after the owner's first live run).

## 2. Non-seam files outside W22's `owns` (review requested)

`cargo xtask check-paths --ws W22` warns about these; each change is minimal and needed for the card:

| File | Owner | Change |
|---|---|---|
| `crates/russet-protocol/src/account.rs` | W6/W12 file, W22 rows | the five W22 DTOs replace `Unspecified` (lead asked for this) |
| `crates/russetd/src/handlers/account.rs` | W6/W12 file, W22 rows | the five W22 rows replace `not_implemented` (lead asked for this) |
| `crates/russet-account/src/store.rs` | W6/W12 | `AccountEntry` gains `team-tier`, `apple-id`, `anisette` (the names `russet-config`'s schema already defines); `upsert_apple_id`, `apple_id_entry`; `portal()` routes Apple ID accounts to DS2; `remove(wipeSecrets)` also deletes `apple-id-session/<DSID>` |
| `crates/russet-account/src/lib.rs` | W6/W12 | `AccountService::with_xtool_dir` registers the home's `free::AppleIds` (one line), so `provision.*` on an Apple ID account finds its DS2 portal |
| `crates/russet-account/Cargo.toml` | W6/W12 | deps `russet-gsa`, `russet-anisette`, `reqwest`, `tracing`; dev-deps with the `fake` features, `rustls` |
| `crates/russet-asc/src/ds2.rs` | W22 | (owned) `Ds2Auth`, `Ds2Credentials` |
| `crates/russet-core/src/error/codes/{gsa,anisette}.rs` | W22 | (owned) new codes |
| generated: `schemas/**`, `extension/src/generated/protocol.ts`, `docs/reference/rpc.md` | generated | `cargo xtask schema` |

## 3. Requests to other owners

- **W2 (`russet-core/src/net.rs`)**: add a `ds2` endpoint (default `https://developerservices2.apple.com/services/`,
  `RUSSET_ENDPOINTS_FILE` key `ds2`). Until then `AppleIds::with_ds2_base` is the only override, so daemon-level
  fakes cannot reach DS2.
- **W2 (`russetd/src/ctx.rs`)**: optionally a `DaemonCtx` slot for `russet_account::free::AppleIds`; today the
  handlers use `AppleIds::shared` (one service per config directory, process-wide).
- **W12 (`provision.rs`, `lib.rs`)**: `AccountService::verify` still asks every account for an App Store Connect key,
  so `account.verify` on an Apple ID account answers `account.noAccount`; the free-team ledger and bundle-ID map
  (`free::policy`) are not yet called by the provisioner, and the DS2 `bundleIdCapabilities` relationship of SIGN
  §7.4 is not applied by `AscPortal`.
- **Packaging (W26 / W27c / the Windows host)**: ship `russet-adi-host` next to `russetd` in the Linux engine (the WSL
  backend on Windows), or set `RUSSET_ADI_HOST`.
- **W6 (`russet-cli/src/cmd/account.rs`)**: `russet account login` / `anisette` are still stubs naming W22; the IDE page
  (W19) is the owner's path, so W22 left the CLI for a follow-up (TTY password entry, `--json`).
