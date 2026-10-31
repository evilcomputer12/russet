<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Russet threat model

Status: normative for security review (QA-22). Binding contracts live in
[docs/ARCHITECTURE.md](../ARCHITECTURE.md) (§7 IPC, §11 MCP permissions, §13 error codes, §14 files); where this
document and ARCHITECTURE disagree, ARCHITECTURE wins. The design rationale is in
[docs/design/qa-delivery.md](../design/qa-delivery.md) §8. This file states the assets, the adversaries, the
defences and the test cases that assert them.

Russet builds, signs, installs, runs and debugs iOS apps from Linux and Windows, drives a daemon over local IPC,
exposes an MCP server to AI agents, and records device sessions for replay. Each of those is an attack surface.

## 1. Assets

| Asset | Where it lives | Why it matters |
|---|---|---|
| ASC API key (`.p8`), key id, issuer id | `SecretStore` (T6) | Full App Store Connect API access for the team |
| Apple ID credentials, GSA tokens, anisette state | `SecretStore`; anisette cache | Account takeover; TestFlight and App Store actions |
| Signing certificates and private keys | `SecretStore` (encrypted file for large p12) | Code-signing identity |
| Pairing records (`HostPrivateKey`) | usbmuxd, never copied into Russet state | Full trusted access to the paired device |
| Provisioning profiles | project/state | Contain UDIDs and the team id |
| User source code | the workspace | Confidentiality and integrity |
| Device data: logs, crash reports, screen frames | transient; traces (redacted) | May carry PII; a prompt-injection channel |
| Release signing keys (minisign, Authenticode) | offline / SignPath once public | Supply-chain trust |

## 2. Adversaries

- **A1 — a malicious or prompt-injected AI agent** driving the MCP server, trying to exceed its permission tier
  or act on instructions hidden in device/Apple text.
- **A2 — a malicious app on the device** writing crafted bytes to logs, crash reports or the screen.
- **A3 — another local user** on the same machine, trying to reach the daemon socket or secrets.
- **A4 — a network attacker** on the HTTP paths (ASC, GSA, TSS, anisette CDN, Flutter storage).
- **A5 — malicious PRs and dependencies** (supply chain), including trace or fixture contributions.
- **A6 — the hosted CI provider**, which must never see a secret or an Apple binary.

## 3. Defences by surface

### 3.1 Secrets (A3, A4, A6) — ARCHITECTURE §12, qa-delivery §8.2

- All secrets go through `SecretStore` (T6): OS keychain (`keyring`), with an AES-GCM-encrypted file for blobs
  over the Windows credential limit, and a `0700`/`0600` file fallback for headless hosts (permissions re-checked
  on every read).
- In memory, secret types use `secrecy`/`zeroize` and have no `Debug`/`Display`.
- The `tracing` redaction layer drops fields named `*token*`, `*password*`, `*secret*`, `p8`, `p12`,
  `HostPrivateKey`, `RootPrivateKey`, and masks bearer/JWT patterns in message text.
- CI never exposes secrets to `pull_request` jobs; `gitleaks` runs in L0.
- Tests use `RUSSET_SECRETS_BACKEND=memory` and never touch a keyring entry they did not create (AGENTS.md §4).

### 3.2 Daemon IPC and port forwards (A3) — ARCHITECTURE §7.1, §3.3, qa-delivery §8.3

- Linux: a Unix socket under `RUSSET_HOME/runtime/russetd.sock` (directory `0700`); the peer UID is checked with
  `SO_PEERCRED`.
- Windows: a named pipe `\\.\pipe\russet-<user-SID>` with a DACL granting only the current user SID; pipe-instance
  squatting is refused (CORE-05).
- No TCP listener by default. Port forwards (debugserver, Dart VM service, mirror) bind `127.0.0.1` on random
  ports; a debugserver forward lives only for the session and accepts one connection.
- One live daemon owns the host-wide live-device lock, so the device plane is serialized across worktrees; the
  lock is never bypassed. Development daemons refuse the `live` backend without `RUSSET_LANE=A`.

### 3.3 MCP permissions (A1) — ARCHITECTURE §11.3, qa-delivery §8.4

- Four tiers: 0 read-only, 1 local reversible effect, 2 Apple-account effect (**approval per session**), 3
  irreversible/public (**approval for every call**). Tier 3 is never callable without a human, whatever a log or
  the screen says.
- Approval is out-of-band: a Tier ≥ 2 call without a valid approval returns `perm.approvalRequired` (1204) with
  `data.details.approvalId`; the daemon emits `approval.requested`; the human grants via `approvals.grant`; the
  agent retries with the approval id. The server binds the approval to the **same tool**, the **same
  `argsDigest`**, the **same client identity**, an unexpired TTL, and single use. Tier-2 session approvals bind to
  the stdio connection or the HTTP bearer token plus a TTL [D 67].
- Client identity is the stdio launch argument or the HTTP bearer token; the self-reported `clientInfo` is
  untrusted.
- HTTP is off by default; when on, it listens on `127.0.0.1`, checks `Origin`, and requires the token.
- Every tool call is written to an append-only audit log (`0600`): time, client, tool, tier, `argsDigest`,
  redacted arguments, approval id, outcome, duration.

### 3.4 Untrusted device/Apple text (A1, A2) — qa-delivery §8.4

- Text from the device or Apple (logs, crash reports, OCR, screen text) is wrapped
  `<untrusted source="device">…</untrusted>`; structured content carries `"source":"device"`.
- Control characters and ANSI escapes are stripped; results are capped at 64 KiB with a pagination handle.

### 3.5 Device traces (A2, A5, A6) — qa-delivery §3.5, this card

- The recorder never stores Apple-file payloads: DDI/cryptex uploads, symbol/shared-cache transfers, firmware and
  TSS blobs, and any screenshot other than our own probe app become `elided` records (a length and a SHA-256).
- The `default-v1` redaction policy maps UDIDs, serials, chip ids, MAC/Bluetooth/Ethernet addresses, device
  names, phone numbers, IMEI/MEID, `HostID`/`SystemBUID`, team ids, Apple-ID emails and non-ours bundle ids to
  synthetic pseudonyms before a record is written (`russet_testkit::redaction::redact_value`).
- The **trace lint** (`russet_testkit::redaction::lint`, and `cargo xtask traces lint` for committed plain
  traces) is the safety net: it fails on a UDID shape (outside the fake chip-id-`00000000` allowlist), a MAC, an
  email, a PEM private key, Mach-O/IMG4/DMG/BOMStore/xar magic in a payload, or a `data` record over 1 MiB. No
  trace reaches the repository or CI without passing it (DC-13).
- Pairing records are never recorded; replays use synthetic PKI.

### 3.6 Network (A4) — T3, qa-delivery §3.6

- All HTTP endpoints are injectable (T3) and overridable by `RUSSET_ENDPOINTS_FILE`. L1–L6 run with
  `RUSSET_OFFLINE=1`; the HTTP client refuses non-loopback hosts. No Apple Android libraries are downloaded or run
  in CI; the real anisette path runs only in the manual free-account test.

### 3.7 Supply chain (A5, A6) — qa-delivery §4, §9

- Actions pinned by SHA; workflows linted with zizmor. `cargo deny` checks licences, bans and sources.
- An Apple-file guard scans the tree, caches and every uploaded artifact; raw Apple-tool outputs are never
  committed or uploaded. Golden expectations are normalized JSON only.

## 4. Security test cases (QA-22)

Each case asserts a defence above. "Owner" is the crate/card that implements the test.

| Case | Asserts | Owner |
|---|---|---|
| SEC-redact-01 | `redact_value` replaces every sensitive key; the result lints clean | russet-testkit (this card) |
| SEC-lint-01 | The trace lint flags a planted UDID, MAC, email, PEM key and Mach-O payload | russet-testkit (this card) |
| SEC-lint-02 | Every committed trace is redaction-clean (`cargo xtask traces lint`) | xtask traces (this card) |
| SEC-elide-01 | A recorded Apple-file payload becomes an `elided` record, not bytes | russet-testkit (this card) |
| SEC-secret-01 | Secret types have no `Debug`/`Display`; the `tracing` layer drops secret fields | russet-core / russetd |
| SEC-ipc-01 | A different-UID peer is refused on Linux (`SO_PEERCRED`); the Windows pipe DACL grants only the user SID, and a squatted pipe instance is refused (CORE-05) | russetd / russet-ipc |
| SEC-mcp-tier-01 | A Tier ≥ 2 tool without an approval returns `perm.approvalRequired` (1204); annotations match the tier | russet-mcp |
| SEC-mcp-approval-01 | An approval is bound to tool + `argsDigest` + client + TTL + single use; any mismatch fails | russet-mcp |
| SEC-mcp-sanitize-01 | Device/Apple text is wrapped `source="device"`; control chars stripped; output capped | russet-mcp |
| SEC-audit-01 | Every tool call appends a `0600` audit record with the required fields | russet-mcp / russetd |

Cases owned by other crates are listed so the model is complete; this card lands the trace/redaction cases
(SEC-redact-01, SEC-lint-01/02, SEC-elide-01) and the Windows CI job that runs the IPC case on `windows-2025`.

## 5. Residual risks

- The out-of-process mock's RSD side channel is not a packet-level fake of the userspace tunnel; tunnel integrity
  is covered by idevice's own tests and by the device lab (L7).
- The trace lint is heuristic; the redaction pass (not the lint) is the primary control, and new sensitive keys
  must be added to `redact_value` as the facade grows.
- CI minutes and artifact limits (GitHub Free) constrain how much runs per PR until the repository is public.
