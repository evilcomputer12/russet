<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# W22 status: native Apple ID sign-in (paid personal first)

Branch `ws/w22-apple-signin` (worktree `russet-w22-apple-signin`), on `main` 95047c0. Nothing pushed or merged; no
Apple portal resource touched; no live login made. Seam requests: [W22-SEAM-REQUESTS.md](W22-SEAM-REQUESTS.md).

## What works (mocked suites, offline)

| Piece | Where | Evidence |
|---|---|---|
| SRP-6a (RFC 5054 2048-bit, SHA-256, GrandSlam's `x` without user name, `s2k`/`s2k_fo`), `spd` AES-CBC, app-token AES-GCM, checksum | `russet-gsa` `srp.rs`, `crypto.rs` | RFC 5054 Appendix B vectors; client/server agreement; round trips |
| GrandSlam client: `lookup`, `init`/`complete`, M2 check, status → error codes, trusted-device and SMS two-factor, re-login, Xcode token | `russet-gsa` `client.rs` | `tests/gsa.rs` against `GsaFake` (paid trusted-device, SMS-only + `s2k_fo`, free without 2FA, wrong password, stale token) |
| Apple Root CA only for `gsa.apple.com` (downloaded once, SHA-256 pinned, cached); no connection reuse | `russet-anisette` `apple_tls.rs` | pin and loopback tests |
| DS2: `Ds2Auth` (GET/DELETE → POST + override, `teamId` injection, Xcode/session/anisette headers), QH65B2 `listTeams`, app groups | `russet-asc/src/ds2.rs`, `russet-gsa/src/ds2.rs` | `v1` read and `listTeams` through `GsaFake`; paid/free tier |
| Log canary: a whole sign-in at TRACE with all events captured; no password, Apple ID, code, `adsid`, team ID, token or anisette value | `russet-gsa/tests/canary.rs` | passes; also `Debug` canaries in protocol, session and anisette types |
| Anisette providers: `LocalAdi` (helper over JSON Lines, provisioning via `midStart/midFinish`, `-45061` re-provisioning, `adi.pb` mirrored into the secret store), `RemoteV3` (websocket provisioning + `get_headers`) | `russet-anisette` | `free/tests.rs` (LocalAdi on `FakeAdi` through `GsaFake`), `tests/anisette.rs` (RemoteV3 against a fake v3 server) |
| Library acquisition: HEAD, 64 KiB tail, central directory, local headers, inflate, CRC/size/SHA-256, 0700/0600 files, TOFU record, no re-download, refusal of a CDN ignoring `Range` | `russet-anisette/src/libs.rs` | `tests/anisette.rs`: only the libraries' bytes travel (< 10 % of a synthetic APK with 2 MB filler) |
| `russet-adi-host`: clean-room ELF64 loader (x86_64), bionic shims, own `dlopen`/`dlsym`, trap stubs naming unknown imports (exit 70), seccomp (no sockets/exec/ptrace/namespaces…), Landlock when available | `russet-adi-host` | `tests/host.rs`: a synthetic two-library ADI in C (compiled by `cc` at test time) loads, provisions, answers OTPs; its `socket()` is refused. **Apple's real x86_64 libraries (APK 4.9.6.1447) load locally** under seccomp and Landlock: `load` ok, `isProvisioned` false, `requestOtp` → ADI `-45061` (see "Real-library check") |
| Sign-in service: `AppleIds::begin/request_code/submit_code`, trusted-device push at once, 10-minute pending sessions (5 wrong codes), session sealed as `secret:apple-id-session/<DSID>` (never the password), `accounts.toml` entry (`kind = "apple-id"`, `team-tier`, `anisette`), DS2 portal for the account, `remove --wipe-secrets` | `russet-account/src/free/` | `free/tests.rs` |
| Free-team policy: App ID ledger (10 per rolling 7 days, next free slot), bundle-ID map (`<id>.<TEAMID>`, extensions, app groups), refresh timing (24 h ahead) | `free/policy.rs` | unit tests (not yet called by the provisioner, see seam requests) |
| RPC rows | `handlers/account.rs` | `russetd` builds; clippy clean |
| Doctor | `anisette.local` | unit test |

Gates run on the touched crates (`russet-{gsa,anisette,adi-host,account,asc,protocol,core}`, `russetd` lib +
`account_handlers`): see the last section.

## RPC shapes for the IDE sign-in page (W19)

All five are registered and in `extension/src/generated/protocol.ts`. Team IDs are masked; the label masks the
Apple ID (`Apple ID (so…@ex…)`); errors carry `russetCode` + hint.

1. `account.anisette.status {}` → `{kind: "local", librariesPresent, apkVersion?, provisioned, localSupported,
   pinned?}`. If `librariesPresent` is false, show the consent screen of SIGN §7.1 (what: two Apple libraries, about
   2 MB; from: Apple's CDN; why: Apple ID sign-in needs the machine headers they compute; never redistributed).
2. `account.anisette.fetchLibraries {consent: true}` (class `system`, long-running, cancellable) →
   `{apkVersion, bytesDownloaded, pinned}`. `pinned: false` = trusted on first use (show a note).
3. `account.appleId.begin {appleId, password, anisette?: "local" | {remote: "https://…"}, label?}` (class `secrets`,
   ide/cli only) →
   - `{sessionId, step: "done", account}`, or
   - `{sessionId, step: "two-factor", twoFactor: {via: ["trusted-device", "sms"], codeSentTo?: "trusted-device",
     phoneNumbers: [{id, number: "(•••) •••-••12"}], codeLength: 6}}` — a push already went to the trusted devices
     when `codeSentTo` is set.
4. `account.appleId.requestCode {sessionId, via: "trusted-device" | "sms", phoneId?}` → `{}` (resend, or switch to
   SMS; later codes are checked against the last route). Send `"trusted-device"` (kebab-case, like every unit enum
   on the wire), not `"trustedDevice"`. When `begin` reported
   `codeSentTo: "trusted-device"` the push already went out: the page should not request another one on showing
   the code screen (only for **Send a new code**).
5. `account.appleId.submitCode {sessionId, code}` → `Account` (`id: "appleid:<DSID>"`, `kind: "apple-id"`, masked
   `teamId`). Then `account.list` shows it.

Notes for W19's contract requests: `step` is `"two-factor"` (ARCHITECTURE's spelling); `phoneId` is a number;
the `ide` permission profile already allows these `secrets`-class methods (only a non-interactive `cli` is refused),
so no permission change is needed; team lists, `account.setTeam` and `Account.program` are not implemented (the
paid team is picked automatically and its tier is in `accounts.toml`; exposing it needs W6's `Account` DTO).

Errors the page should handle: `gsa.badCredentials` (5250), `gsa.badCode` (5251, retry same session),
`gsa.locked` (5252), `gsa.identityRejected` (5253, retryable), `gsa.codeRateLimited` (5254),
`gsa.loginExpired` (5258, start again), `anisette.libsMissing` (5270 → step 1–2), `anisette.consentRequired`,
`anisette.fetchFailed`, `anisette.adiFailed`, `anisette.provisioningFailed`, `account.appleServiceUnavailable`.

## Real-library check (lead-approved, 2026-10-31)

One fetch of the two x86_64 libraries, approved by the lead within the owner's download budget, made with Russet's
own `libs::fetch` (HEAD plus HTTP range GETs, nothing else): no GrandSlam, provisioning or other Apple endpoint, no
login. The libraries stay only under the isolated `RUSSET_HOME` in `Russet-Dev`
(`/home/russet/.russet-dev/w22-engine/data/anisette/libs/4.9.6.1447/x86_64/`, 0700/0600); only their hashes are
committed (`crates/russet-anisette/anisette-pins.json`).

| | |
|---|---|
| URL | `https://apps.mzstatic.com/content/android-apple-music-apk/applemusic.apk` |
| Date | 2026-10-31T16:18:39Z |
| APK version (`x-apple-version-number`) | 4.9.6.1447 |
| Bytes received | 2,332,091 (64 KiB tail + central directory + two local headers + the two compressed entries) |
| `lib/x86_64/libstoreservicescore.so` | local header at 125,027,081; 901,029 → 2,391,768 bytes; CRC-32 `0d7ebf7b`; SHA-256 `d8be230df34c0044758d43e33a8dabd3236b9a4258c4d48cb366911224d52f68` |
| `lib/x86_64/libCoreADI.so` | local header at 97,688,726; 1,093,336 → 1,596,128 bytes; CRC-32 `7b0c5a50`; SHA-256 `ece4ffa6bbe4239e8f0d4714da57e62ce5e3c581d19553a83d78ac897783b905` |

The entry sizes match the design's survey [V12]. ELF facts: both are x86_64 `ET_DYN` with `RELA`, `JMPREL`,
`DT_HASH` and `DT_GNU_HASH`, `BIND_NOW`, no `RELR` or packed relocations; relocation types are only
`R_X86_64_RELATIVE/64/GLOB_DAT/JUMP_SLOT`. `libstoreservicescore.so` needs `libc++_shared`, `libCoreFoundation`,
`libmediaplatform`, `libz` and `libandroid` besides bionic, and 360 of its imports are absent by design.

Local run of the helper (`russet-adi-host --libs <dir> --state <empty dir>`, dummy Android ID, sandbox on: seccomp
and **Landlock both on** in this WSL kernel): `load` → ok (`LoadLibraryWithPath` reached `libCoreADI.so` through the
helper's `dlopen`; all of CoreADI's imports resolved), `isProvisioned {-2}` → `false`, `requestOtp {-2}` → ADI
`-45061` (not provisioned), clean exit. The first run showed that one of `libstoreservicescore.so`'s 64 static
initializers constructs a `mediaplatform` work queue (a missing import); the loader now runs a library's initializers
only when all its imports resolved, and the helper leaves with `_exit` so library destructors never run.

## First live sign-in (owner, 2026-10-31): `-80045`, and the fix

The owner's first attempt (integration build `0a45065`, local anisette) fetched the libraries and provisioned the
machine with Apple, then GrandSlam refused the login with `ec -80045`, "This action cannot be completed at this
time." Cause: since 2026-10-12 GrandSlam refuses an SRP `init` whose `X-Mme-Client-Info` claims a platform other
than the one the anisette data comes from. Russet's profile (version 3, from xtool 450223eb0) claimed macOS while
its local ADI is Apple's *Android* CoreADI. xtool hit the same error and fixed it the same day by presenting an
Android identity (xtool #337); Apple then refused Android-identifying clients at two-factor (`-80046`, about
2026-10-19), and Android-ADI clients have since presented a Windows identity (anisette-v3-server #61 and its forks,
2026-10-16 to 10-30). xtool itself now uses a remote macOS anisette server instead (#380).

Fix on this branch:

- `gsa-profile.json` version 4: `X-Mme-Client-Info` `<PC> <Windows;6.2(0,0);9200> <com.apple.AuthKit/1
  (com.apple.akd/1.0)>`, `User-Agent` `akd/1.0 CFNetwork/808.1.4` (a user copy older than version 4 loses).
- Every GrandSlam request (lookup, provisioning, `init`/`complete`/`apptokens`, two-factor) presents the anisette
  provider's identity, user agent included (a remote server's own identity for `RemoteV3`, xtool #406); `cpd`
  repeats `X-MMe-Client-Info` as working clients do; two-factor requests use the akd user agent instead of `Xcode`.
- Local ADI records the identity it provisioned under and provisions again when it changes: the owner's machine,
  provisioned under the macOS identity, re-provisions once on the next attempt.
- `-80045`/`-80046` map to `gsa.identityRejected` naming the stage; a bare HTTP 503 (back-to-back logins) is
  retried once after 8 s.
- `GsaFake` now refuses a macOS identity or a `cpd`/header mismatch with `-80045` and a non-akd user agent with a
  bare 503; the suites pin the request shape.

Diagnosis for the retry: start the engine with `RUSSET_LOG=info,russet_gsa=debug,hyper=warn` (the daemon reads
`RUSSET_LOG`, not `RUST_LOG`; logs go to `state/logs/russetd.<date>.log`). Each GrandSlam call
logs `gsa: request` (stage, claimed platform, header names, `cpd` keys) and `gsa: response` (stage, HTTP status,
`hsc`, `ec`, `em`, `au`); never values, the Apple ID, tokens or identifiers (canary-tested). If Windows is refused
too, the fallback is `anisette: {"remote": "<macOS anisette-v3 server>"}` (xtool's current default is
`https://mac-ani.jkcoxson.com`), whose operator then sees this machine's ADI identity; Russet configures none by
default.

## Second live sign-in (owner, 2026-10-31, remote macOS anisette): GrandSlam passes, two answer shapes fixed

With `anisette: {"remote": "https://mac-ani.jkcoxson.com"}` GrandSlam worked end to end (init, complete with
`trustedDeviceSecondaryAuth`, code push, re-login, `apptokens`, Xcode token). Two answers were misread:

- **Code check:** Apple accepted the code (the next login needed no second factor) but Russet reported a failure:
  it parsed the `validate` answer as a plist, and Apple's success answer is not one. xtool ignores that body and
  judges the HTTP status; Russet now does the same (2xx accepts unless a plist/JSON body carries a non-zero `ec`,
  423/429 rate-limit, other 4xx a wrong code, 5xx unavailable) and sends xtool's `Accept: application/x-buddyml` /
  `Content-Type: application/x-plist` on the trusted-device requests. The SMS check is judged the same way.
- **`listTeams`:** developerservices2 answered HTTP 200 with a body that was not an XML plist. Russet's request
  matches xtool's legacy client (URL, `text/x-xml-plist`, headers, `clientId`/`protocolVersion`/`requestId`/
  `userLocale`), so the answer is now decoded whatever its encoding: gzip (by `Content-Encoding` or magic), a BOM
  or leading whitespace, binary or XML plist, or JSON with the same keys; anything else (HTML, empty) fails with
  `gsa.protocol` naming the action, HTTP status, body kind, length and content type.
- Debug logs (`RUSSET_LOG=info,russet_gsa=debug,hyper=warn`) now add `gsa: response` for `trustedNumbers`,
  `validate`/`validateSms` (HTTP status, content type, length, body kind, `ec`) and `ds2: response` for every
  developerservices2 action (HTTP status, content type and encoding, length, raw and decoded body kind); never a
  body, token, team ID or the Apple ID (canary-tested).

## What still needs the owner's live sign-in

Nothing has talked to Apple's sign-in services. These are interoperability facts taken from xtool, isideload,
omnisette and Provision (SIGN §7) that only a live run confirms:

- GrandSlam's acceptance of Russet's SRP proof (M1/K padding as the srp-crate clients do), the `np` field (read, not
  verified), the two-factor endpoints' answers, `apptokens` decryption, and the `gsa-profile.json` identity (version
  3, xtool 450223eb0); a rejection surfaces as `gsa.identityRejected` with the profile refresh hint.
- ADI provisioning and OTPs with Apple's libraries (`startProvisioning`/`endProvisioning` need Apple's `spim`/`ptm`,
  so they were not run): if a provisioning path calls an import Russet does not provide, the helper exits with
  `an ADI library called <name>` (status 70) on stderr; `RUSSET_ADI_TRACE=1` lists every import's resolution
  (names only).
- APK 4.9.6.1447/x86_64 is pinned; if Apple's CDN serves a newer APK, the owner's fetch is trust-on-first-use
  (`pinned: false`), and its hashes are in `<data>/anisette/libs/<apkver>/x86_64/tofu.json`.
- DS2 for the paid personal team: `listTeams` must report the Apple Developer Program membership (tier `paid`).

**Live check, for the owner through the IDE page (nobody else types the credentials):**

1. Prerequisites (lead): `russet-adi-host` built next to `russetd` in the Linux engine (or `RUSSET_ADI_HOST`); the
   engine reaches `apps.mzstatic.com`, `www.apple.com`, `gsa.apple.com`, `developerservices2.apple.com`;
   `RUSSET_OFFLINE` unset; a real secrets backend (not `memory`) so the session survives.
2. Status → consent → `fetchLibraries` (about 2.3 MB; `pinned: true` while the CDN still serves 4.9.6.1447).
3. `begin` with the paid Apple ID and `anisette: "local"` → expect `two-factor` with `codeSentTo: "trusted-device"`.
4. Enter the code → `submitCode` → expect an `apple-id` account with `team-tier = "paid"` in `accounts.toml`.
5. Report back the helper's stderr if anything fails (it never prints secrets).

## Not done / follow-ups

- Free-account provisioning end to end (SA-20/21) beyond reads: the provisioner (W12) does not yet consult the
  ledger or bundle map, apply DS2's `bundleIdCapabilities` relationship, or call the QH65B2 app-group actions; the
  7-day refresh scheduler is not wired. **Known behaviour:** free-account installs fail with `0xe8008015` ("A valid
  provisioning profile for this executable was not found") on iOS 26.5 and 27 across tools (iloader#570, open;
  SIGN §7.1, R-2); paid teams are the path that installs, so this card did paid first. The live free-team check
  (DC-20) stays blocked on Q4 (a throwaway Apple ID).
- `russet account login` / `anisette` CLI commands (W6's file still stubs them).
- aarch64 loader (relocations + trap stubs), a seccomp allowlist from a real trace, Windows (W27c).
- Daemon-level IPC tests need the `russetd` dev-dependencies of the seam requests.
- No "remember password" option (unattended 7-day refresh needs it; SIGN §7.6): sessions are refreshed from the
  sealed `GsIdmsToken` until Apple expires it, then the user signs in again (`gsa.sessionExpired`).

## Gates (WSL `Russet-Dev`, slot a, offline, memory secrets)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| clippy `--all-targets -D warnings` on `russet-{anisette,gsa,adi-host,account,asc,protocol,core}` and `russetd` | clean (the toolchain's own `cargo-clippy`, see below) |
| tests of `russet-{anisette,gsa,adi-host,account,asc,protocol,core}` (`--all-features`) | all pass (incl. `gsa`, `canary`, `anisette`, `host` suites) |
| `russetd` lib + `account_handlers` | 40 + 5 pass |
| `cargo deny check licenses bans sources` | bans ok, licenses ok, sources ok |
| `cargo xtask check-layers` / `check-no-apple` | ok / no Apple files |
| `cargo xtask schema --check` | 3 generated files up to date |
| `cargo xtask check-paths --ws W22` | 8 warnings, all the foreign files listed in the seam requests; no seam file touched |

The full `--workspace` nextest was not run (it does not fit a 6 GiB slot); only the crates W22 touched.

Note for the lead: `/home/russet/.cargo/bin` (the managed `CARGO_HOME`) now holds rustup proxies (1.99.1), and cargo
prefers `$CARGO_HOME/bin/cargo-clippy` for `cargo clippy`; with the read-only 1.99.0 toolchain as `RUSTC` that mixes
compilers (E0514). Calling `$TOOLCHAIN/bin/cargo-clippy clippy …` avoids it.
