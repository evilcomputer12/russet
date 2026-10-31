<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# W19 status: the IDE side of the Russet Windows experience (2026-10-31)

Branch `ws/w19-ide-experience`, worktree `russet-w19-ide-experience`, based on `russet-win` (95047c0) plus
the W14 Windows-backend snapshot (`17097ba`) and the lead's verified F5 double-prepare fix (`517df5d`).
Nothing is pushed or merged.

| Commit | What |
|---|---|
| `17097ba` | chore: base on the W14 Windows backend changes (snapshot of the uncommitted W14 worktree) |
| `517df5d` | fix(debug): prepare once when VS Code chains both provider registrations (from the lead) |
| `0e4120e` | feat(setup): guided setup page over the Windows setup runner |
| `25dae01` | feat(setup): native Apple ID sign-in page |
| `e849ae6` | feat(panels): Xcode-style run destinations and the Virtual iPhone tab |
| `50a7d8d` | feat(extension): wire the setup, sign-in and run-destination surfaces (manifest, commands) |
| `215df65` | test(smoke): drive the W19 surfaces in the installed editor |
| (this commit) | docs: W19 status, contract requests, ide.md §5.8 note |

## What works

### 1. Guided setup page (`extension/src/setup/**`, `extension/webview/setup/**`)

- `russet.openSetup [stepId]` opens the "Set up Russet" editor tab; it also opens on the first run when a
  setup runner exists, and from the walkthrough ("Set up this PC", "Choose Xcode…", "Create Virtual iPhone").
- Steps come from `RUSSET_WINDOWS_HOST --setup status` exactly per the host tree's runner contract; runs
  stream `--setup run <id> [--arg k=v]…` NDJSON into a progress bar (determinate when `fraction` is given),
  the current message, and a log tail (last 200 lines kept, 50 shown). Buttons: **Set up**, **Retry**
  (failed, retryable), **Repair** (done; sends `--arg repair=1`, see the contract requests), **Cancel**
  (kills the runner), **Check again** (unavailable), and **Set up remaining steps** (runs every `needed` or
  `failed` step in order, stops at the first failure). One step runs at a time.
- `xcode-sdk`: lists every `Xcode_*.xip` in `%USERPROFILE%\Downloads`, newest Xcode first, plus **Browse…**;
  only a found or browsed `.xip` reaches `--arg xip=`. Without one the step is blocked with the reason.
- `virtual-iphone`: device and iOS pickers from `extension/data/virtual-iphones.json` (vphone-linux's
  catalog, iPhone pairings only, tested/untested marked); default **iPhone17,3 / iOS 27.0.1**; passed as
  `--arg device= --arg ios=`.
- `iphone`: per the live runner ("checked in the device picker"), the card offers **Choose iPhone…** and turns
  Done from the daemon's ready device (model name and iOS only, never the device's own name).
- `wsl` / `apple-devices`: **Copy command** for `wsl --install --no-distribution`, and **Get Apple Devices from
  the Microsoft Store**.
- An **Apple account** card (from `account.list`, not the runner) before the Xcode step: **Sign in with Apple
  ID…** and **Add App Store Connect API key…**.
- Walkthrough completion keys from real state (`SetupModel.contextKeys`): `russet.setupOk`,
  `russet.toolchainOk` (daemon `toolchain.list` swift ok, or runner backend+xtool done), `russet.sdkOk`
  (`toolchain.list` SDKs, or runner `xcode-sdk` done), `russet.accountOk` (`account.list` non-empty),
  `russet.virtualIphoneOk`, plus `russet.windowsSetup` for the Windows-only walkthrough steps.
- Without a runner (Linux, or no supervisor) the page shows the engine-backed toolchain and account steps and
  explains that machine setup runs in the Windows IDE.

### 2. Native Apple ID sign-in (`extension/src/setup/appleId.ts`, `signInPanel.ts`, `webview/signin/**`)

- Screens: consent (and the anisette libraries download when `account.anisette.status` reports them
  missing) → Apple ID + password → two-factor code from the trusted device, **Send a new code**, **Didn't get a
  code? Text me instead** (trusted-number picker → `requestCode {via: "sms", phoneId}`) → signed in, showing
  the account, its type and team.
- RPCs: `account.anisette.status`, `account.anisette.fetchLibraries {consent: true}`,
  `account.appleId.begin {appleId, password, anisette: "local"}`, `account.appleId.requestCode`,
  `account.appleId.submitCode`, with W22's DTOs (`ws/w22-apple-signin`) as local types: `via` on the wire is
  `trusted-device`/`sms`, the code the engine already pushed (`codeSentTo`) is used without a second request,
  trusted numbers come from `phoneNumbers[{id, number}]`, and `step: "done"` uses the returned `account`. The
  ARCHITECTURE draft shapes are still read.
- Credentials: the password goes from the page into `account.appleId.begin` only; the form clears it on send;
  the controller never keeps it; nothing logs it (canary unit test). Never written to settings, state or logs.
- Errors by code: bad credentials (5250), bad code (5251), locked (5252), identity rejected (5253), too many
  codes (5254), sign-in timed out (5258, back to the form), libraries missing or consent required (5270/5273),
  local anisette unsupported on this host (5276, also from `localSupported: false`), Apple unavailable (5107),
  `perm.*`, not implemented (1005 / -32601 / 3013).
- Identity (anisette): setting `russet.account.anisetteServer` (machine scope; the extension also reads only the
  user-settings value, so a workspace cannot redirect it). Empty: this PC's own local ADI. An `https://` URL (the
  owner chose xtool's `https://mac-ani.jkcoxson.com` after Apple refused the local identity with -80045):
  `begin` sends `anisette: {remote: url}`, the local status check, consent and library download are skipped, and
  the form names the server's host and says its operator sees this computer's device-identity requests, not the
  password or codes. Non-https values are refused with a message. `gsa.identityRejected` (-80045/-80046) on the
  local path suggests the setting (**Use an identity server…** opens it).
- Setting `russet.account.appleIdSignIn`: `auto` (default; an engine that answers not-implemented gets an
  explanation and the API-key alternative), `on` (show the form even without anisette status), `off`.
- Live check: against today's engine the page shows "not available in this version of the Russet engine yet
  (core.notImplemented)" with **Add App Store Connect API key…**.

### 3. Xcode-style controls (`extension/src/panels/**`, `webview/device/**`, `webview/shared/**`)

- **Run destination picker** (`russet.selectDevice`, status bar, devices tree "Set as Run Destination"):
  "iPhones" and "Virtual iPhones" groups. Daemon devices with `transport: "virtual"` are treated as virtual and
  runnable as soon as the daemon reports them; until then a VM the runner created is listed as "Not running",
  and **Run/Debug** on it explain instead of building. With no VM, the picker offers **Create a Virtual
  iPhone…** (setup page at that step).
- **Status bar**: Run ▶, Debug, Stop (while running/building) next to the destination (`$(vm)` for virtual) and
  the target · configuration; every item has a screen-reader label.
- **Editor title / title bar**: Stop joins Run/Debug in `editor/title/run`; `russet/titleBar` now has Run,
  Debug, Stop, Hot Reload, target, destination (rendered by the product build).
- **Virtual iPhone editor tab** (`russet.openVirtualIphone`, also from the sidebar iPhone view): device frame
  sized from the catalog's panel, Dynamic Island, side buttons; touch (pointer down/move/up → device
  coordinates 0…1 for each rotation, ≤ 2 px at three probe points in the unit test) and keyboard capture while
  the screen has focus (Tab and modified keys pass through, Escape leaves). Buttons Home, Lock, Volume Up/Down,
  Rotate Left/Right, Screenshot, each a `russet.virtual.*` command with editor-title icons and keybindings
  (Ctrl+Shift+H/L/Up/Down/Left/Right/S, only while the tab is focused). States: not created (button to the
  setup step), not running, running. Input goes to `VirtualDeviceBackend`; today `PendingVirtualBackend` says
  "Not delivered: the engine's virtual device provider … is not connected yet" and nothing is sent.
- The sidebar **iPhone** view uses the same frame and follows the destination; **Open in Editor** opens the tab.
- The setup page and the Virtual iPhone tab are restored after a window reload (serializers); the sign-in page
  deliberately is not.

### Accessibility

Real buttons, labels and fieldsets; `role="log"` log tails, polite live regions for progress and step
results, `role="alert"` errors; focus kept across progress updates (steps re-render only when their own data
changes); visible focus rings; VS Code theme variables only (the device screen is black like an off display);
checked in dark, light and high-contrast themes; `prefers-reduced-motion` stops the spinners.

## Verification (2026-10-31, Windows, Node 24.22.1)

- `vitest run`: 12 files, **77 tests pass** (18 pre-existing + setup contract, runner against an in-process
  fake and against `test/fixtures/fake-setup-runner.mjs`, setup model, Xcode detection, sign-in flows with a
  password canary, destinations, catalog, tap geometry, input validation).
- `tsc --noEmit`: clean. ESLint on every changed file: clean.
- Installed editor (`%LOCALAPPDATA%\Programs\Russet Development`, VSCodium 1.143) with this worktree's
  extension as the development copy, its own `--user-data-dir`/`--extensions-dir` under
  `extension/.vscode-test/`:
  - `node test/smoke/run-installed.mjs --screens` (synthetic daemon, fake runner): **15/15 checks** — 9 steps,
    `Xcode_27.2.xip` detected, iPhone17,3 / 27.0.1 default, Xcode step runs to Done, sign-in through 2FA to
    the account and team, Set up remaining steps, picker lists USB and virtual iPhones, status bar shows the
    virtual destination, touch/keys captured with the stub's reason, Run on the virtual iPhone explains,
    sidebar view follows the destination.
  - `node test/smoke/run-installed.mjs --live` (managed backend daemon with devices `none` and a throwaway
    engine home, removed afterwards; the real runner `russet-windows/src/Russet.Host/bin/Release/net10.0/
    Russet.Host.exe`, **status only**): **6/6 checks** — connected through the managed backend, the real
    runner's eight steps render without errors, sign-in reports `core.notImplemented` from today's engine.
- Smoke-run caution: the run opens a visible editor window that the script drives; nobody should type into it.

## Screenshots

`C:\Users\pc\AppData\Local\Claude-Data\anthropicpbc.claude_fnn82j28hfe8t\side-session-notes\local_dcd07145-8778-4885-8a1f-30d85976d55b\w19-screenshots\`
(synthetic data only; regenerate with the smoke commands above, which write to
`extension/.vscode-test/w19-{screens,live}/shots/`):

| File | Surface |
|---|---|
| `01-setup-dark.png` | Setup page, dark theme |
| `02-setup-running.png` | Xcode step running: progress, log tail, Cancel |
| `03-signin-consent.png` … `07-signin-done.png` | Sign-in: consent, credentials (masked), 2FA code, text-me numbers, signed in |
| `08-virtual-iphone-not-created.png` | Virtual iPhone tab before the VM exists |
| `09-setup-complete.png` | All steps done |
| `10-destination-picker.png` | Run destination picker with USB and virtual iPhones |
| `11-virtual-iphone.png`, `12-virtual-iphone-landscape.png` | Virtual iPhone tab, portrait and rotated |
| `12b-sidebar-mirror.png` | Sidebar iPhone view with the shared frame |
| `13-…light.png`, `14-setup-light.png`, `15-setup-high-contrast.png`, `16-…high-contrast.png` | Light and high-contrast themes |
| `live-01-setup.png`, `live-02-signin.png`, `live-03-virtual-iphone.png` | Against the real runner and engine |

## VSIX for the lead

`extension/russet-w19.vsix` (built with the extension's own `vscode:prepublish` steps; 36 files, ~100 KB).
Deploy with the host tree's script (the editor must be closed):

```powershell
& C:\Users\pc\Documents\ChatGPT\russet\russet-windows\scripts\update-dev-install.ps1 -ExtensionVsix C:\Users\pc\Documents\ChatGPT\russet\russet-w19-ide-experience\extension\russet-w19.vsix
```

The installed launcher must point `RUSSET_WINDOWS_HOST` at a supervisor that has `--setup` (the Release build
above); with the current installed host the page shows "The setup runner could not report its status".

## Waiting on others

- **W22** (`ws/w22-apple-signin`): its handlers and DTOs are committed and the owner can sign in. When W22 merges,
  regenerate `src/generated/protocol.ts` and swap the local types in `src/setup/appleId.ts` for the generated
  ones (the local copies already match W22's wire shapes). Not yet checked end to end against W22's daemon from
  this branch: the unit tests use W22's shapes.
- **Setup runner (host tree)**: deploy the runner build in the installed app; honour `--arg repair=1`
  (contract request 3).
- **Contract changes** (`docs/windows/W19-CONTRACT-REQUESTS.md`): `DeviceSummary.transport: "virtual"`;
  virtual device start/stop, display stream and `input.*` shapes; Apple ID `step` spelling, trusted-device
  push, `phoneId` type, team list and program kind; `account.appleId.*` from IDE connections (`interactive`).
- **Virtual device provider (W31 interface, W10 mirror)**: display frames and input delivery replace
  `PendingVirtualBackend`; until then the tab shows the frame, captures input and says nothing is delivered.
- **`sdk.import` (W16b)**: Xcode import outside Windows still says it is pending.

## Files outside W19's own paths

Small hooks, listed for the lead's merge: `extension/package.json` (contributions; a seam file once W14
merges), `extension/src/commands/index.ts` (route existing ids through `experience`, Run/Debug guard, the API key's
issuer ID prompt checked by `setup/ascKey.ts`),
`extension/src/statusbar/index.ts` (Run/Debug/Stop, virtual destination), `extension/src/extension.ts`
(`installExperience`), `extension/vite.config.ts` (three pages), `extension/media/walkthrough/*.md`,
`docs/design/ide.md` §5.8 (note). New JSON data is covered by `REUSE.toml`'s `extension/data/*.json`
annotation; every other new file carries an SPDX header.
