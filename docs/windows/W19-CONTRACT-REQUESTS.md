<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# W19 contract requests (2026-10-31)

Contract changes the W19 IDE surfaces need, for the lead to take into
`docs/ARCHITECTURE.md` (and the host tree's setup-runner contract). The
extension already codes against the shapes below and keeps working against
today's contract; each item says what it does until the change lands.

## 1. `DeviceSummary.transport` gains `"virtual"` (ARCHITECTURE §8 `device.list`, §9 `device.changed`)

- **Change:** `DeviceTransport = "usb" | "wifi" | "virtual"`. A virtual iPhone is
  a `DeviceSummary` like any other: `udid` (the VM's synthetic UDID),
  `productType` (`iPhone17,3`), `osVersion` (`27.0.1`), `state` (`ready` while
  booted and reachable over vphoned, `unavailable` when stopped).
- **Optional:** `virtual?: {vmId: string, power: "stopped" | "booting" | "running"}`
  so the picker can say "Not running" vs "Booting".
- **IDE today:** `src/panels/destinations.ts` already treats
  `transport === "virtual"` as a virtual destination (`isVirtualDevice`) and makes
  it runnable; until the daemon reports one, a VM the setup runner created is
  listed from the runner's `virtual-iphone` step as "Not running", and
  Run/Debug on it explain why they cannot start yet.

## 2. Virtual device control, display and input (W31 interface, W10 mirror)

The Virtual iPhone tab (`src/panels/virtualIphonePanel.ts`) sends these through
`VirtualDeviceBackend`; today `PendingVirtualBackend` reports "not delivered".

| Need | Proposed method | Params (IDE side today) |
|---|---|---|
| Boot / shut down the VM | `device.virtual.start` / `device.virtual.stop` | `{udid}` |
| Screen frames | `screen.mirror.start` with the virtual UDID (ws stream as EXT-09) | `{udid, codecs}` |
| Touch | `input.touch` (or `input.tap`/`input.swipe`) | `{udid, phase: "down"\|"move"\|"up", x, y}`, x/y normalized 0…1 in portrait device coordinates |
| Keys | `input.key` | `{udid, key, code, shift}` (DOM `KeyboardEvent.key`/`code`) |
| Hardware buttons | `input.button` | `{udid, button: "home"\|"lock"\|"volumeUp"\|"volumeDown"}` |
| Rotation | `device.virtual.orientation` | `{udid, degrees: 0\|90\|180\|270}` (clockwise) |
| Screenshot | `screen.screenshot` | `{udid}` → PNG (the tab saves the last frame today) |

`input.*` and `screen.*` are `Unspecified` in the generated protocol; the shapes
above are what the page already produces (`parseInput` validates them).

## 3. Setup runner: `--arg repair=1`

- **Change (host tree contract):** `--setup run <id> --arg repair=1` re-verifies and
  fixes a step even when it is `done`.
- **IDE today:** the page's **Repair** button sends it. Under the current contract
  a second run of a done step is a no-op `done`, so Repair is harmless until the
  runner honours it.

## 4. Apple ID sign-in shapes (ARCHITECTURE §8 rows `account.appleId.*`)

- **`step` spelling:** ARCHITECTURE writes `two-factor`, SIGN §12 writes
  `twoFactor`. Pick one; the IDE accepts both.
- **Trusted-device push:** does `begin` already trigger the push to trusted
  devices? The IDE calls `requestCode {sessionId, via: "trustedDevice"}` once when
  it shows the code screen (and again for "Send a new code").
- **`phoneId` type:** SIGN §5 uses `phone_id: u32`; the IDE sends a number when
  the id the daemon gave is numeric, else the string.
- **Teams:** an Apple ID can belong to several teams (SIGN §13.1 step 5, team
  picker). No RPC lists them yet; the success screen shows the `Account.teamId`
  that `submitCode` returns. Request: `submitCode` → `Account & {teams?: [{id, name,
  kind: "free"|"individual"|"organization"}]}` plus `account.setTeam {accountId, teamId}`.
- **Free vs paid:** `Account.program?: "free" | "individual" | "organization"`, so
  the IDE can show the free-team limits (7-day profiles, 3 apps, 10 App IDs a week).

## 5. Secrets-class calls from the IDE (ARCHITECTURE §7.5, "Secrets from a CLI without a TTY")

- **Issue:** §7.5 says `secrets`-class methods need `interactive` and that passwords
  and 2FA codes (`account.appleId.*`) stay TTY-only; the IDE's `initialize` sends
  `capabilities.interactive: false`. The native sign-in page is human-driven: the
  user types the password and code into it.
- **Request:** allow `account.appleId.*` from `client.kind: "ide"` connections (or
  have the IDE declare `interactive: true`; that file is W14's `daemon/client.ts`).
- **IDE today:** a `perm.*` answer shows "The Russet engine refused sign-in from
  the IDE connection" on the page.

## 6. Setup-runner development override (IDE only, for the record)

`RUSSET_SETUP_RUNNER=<absolute path>` makes the extension use another runner
(an `.exe`, or a `.js`/`.cjs`/`.mjs` script run with the editor's Node). Used by
the fake runner in tests and the screenshot pass; it never changes
`RUSSET_WINDOWS_HOST`, which still carries the daemon relay.
