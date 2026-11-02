# Russet Windows IDE experience — plan, 2026-10-31

Owner direction: "russet has to do all of that". Installing xtool and vphone-linux,
Apple sign-in, building the SDK from Xcode, creating the virtual iPhone, and
debugging real and virtual iPhones with polished controls are all features of
the installed IDE, driven from its own UI. Nothing here is a one-off manual step.

## Decisions (owner, 2026-10-31)

| Topic | Decision |
|---|---|
| Apple sign-in | Native Russet page (Apple ID, password, 2FA) on the engine's own GSA/anisette; not xtool's login |
| Accounts | Free and paid; paid personal (individual) first |
| Setup | Inside the IDE: first-run page with progress, repair, retry |
| Controls | Xcode-style: target + device picker (real and virtual), Run/Stop/Debug, build status; virtual iPhone in an editor tab with frame, touch/keyboard, hardware buttons; VS Code debugger views |
| Xcode | Russet downloads it after sign-in; the owner's manual `Xcode_27.2.xip` is used meanwhile |
| Virtual iPhone | Default iOS 27 (iPhone17,3 27.0.1, upstream-tested on Linux), user may pick another catalog pairing |
| USB | Microsoft Store Apple Devices app provides usbmux; no Russet driver |
| vphone-linux | Upstream YahiaAngelo `main` (WSL2 support), owner's evilcomputer12 display fixes only if needed; Russet-managed install in Russet-Dev ext4, separate repository |
| Limits | ~20 GB downloads, ~170 GB temporary C: storage (raised from 110 for the virtual iPhone), WSL memory 24 GB (applied) |
| Apple account changes | May create certificates, register devices and App IDs; never revoke |

## Components and owners

| Piece | Where | Owner card | State |
|---|---|---|---|
| xtool install + SDK from `.xip` | `backend/xtool_component.sh` | host tree (interim for W16b `sdk.import`) | xtool 1.24.0 installed; full (not `--slim`: Russet needs `Xcode.app/Contents/version.plist`) iPhoneOS 27.2 SDK from `Xcode_27.2.xip`, 7.0 GB, built through `--setup run xcode-sdk`; unsigned SwiftUI probe builds against it in 56 s. xtool ignores `HOME` and always installs into the real `~/.swiftpm` |
| vphone-linux install + VM create | `backend/vphone_component.sh` | host tree / W31 track | engine built through `--setup run vphone` (QEMU, reims-vgpu, APFS module, vphone-cli 2.13.4 tooling and ROMs); Russet patches (applied by `fetch`, worth sending upstream): `patches/vphone-linux-wsl3-kernel-headers.patch` (WSL 3 ships an include-only headers tree, so upstream skipped its Microsoft-kernel path) and `patches/vphone-linux-ubuntu2604-usbip.patch` (Ubuntu 26.04's per-kernel linux-tools folders are symlinks, so upstream never linked the real `usbip`; Ubuntu's wrapper fails for Microsoft's kernel and the DFU device never attaches); `create` clears WSL's `$NAME` (the Windows computer name) so the VM is named after device/iOS; iOS 27.0.1 VM creation running |
| Setup step runner | `Russet.Host --setup status\|run <step>` streaming NDJSON | host tree | implemented; component scripts installed at `/opt/russet/lib`; `[Phase] NN%` redraws become deduplicated progress with `fraction` |
| Setup page | `extension/src/setup/**`, `extension/webview/**` | W19 | walkthrough exists, account/SDK/toolchain steps never complete |
| Apple ID sign-in | `account.appleId.begin/requestCode/submitCode`, `account.anisette.*` (protocol exists, handlers stubbed) | W22 (XL) + W27c (Windows anisette) | `russet-gsa`, `russet-anisette`, `russet-adi-host` empty |
| Sign-in page | webview calling the RPCs above; `ui.prompt` fallback already handles codes | W19 | "coming later" stub |
| Xcode download | GSA developer session → developer.apple.com `.xip` | W22 follow-up | after sign-in exists |
| Device picker incl. virtual | `DeviceSummary.transport` gains `virtual`; status bar picker exists | contract change (lead) + W10 | physical works |
| Virtual device backend | daemon device provider over QMP, vsock 1339 (vphoned), install/launch/debug | W31 interface + russet-device | not started |
| Embedded display + input | mirror view (`russet.mirror`, placeholder with `kind: virtual`) fed by guest frames; touch/buttons via QMP/HID | W19 + W10 | placeholder |
| Xcode-style toolbar | `russet/titleBar` menu, editor title, status bar; product build may enable Command Center | W19 + W25 | run/debug/stop commands work |
| Physical F5 in installed editor | `tests/installed-f5.mjs` (local) | W14 | deployed bits; run in progress |

## Backend isolation (owner, 2026-10-31)

The managed WSL distro is internal to Russet: users never log into it, and it
sees Windows only through one shared folder.

- Installing Russet checks for WSL first. When it is missing, the installer asks the
  user to install it (`wsl --install --no-distribution`; administrator rights and
  possibly a reboot), then continues; it never installs a user-facing distro.
- `/etc/wsl.conf` in the managed distro: `[automount] enabled=false` plus one
  `drvfs` fstab entry for the shared folder, mounted `metadata,uid=1000,gid=1000`
  (builds chmod and timestamp the files they write in a project; without it every
  Windows file is root's and bundling fails with EPERM); `[interop] enabled=false`,
  `appendWindowsPath=false`. Russet removes the distro's Start-menu and Terminal
  entries and never makes it the default distro.
- Everything the backend reads from Windows (projects, the Xcode `.xip`, IPSW
  caches) goes through the shared folder; path translation maps only that folder.
- Windows-to-Linux pipes (`wsl.exe --exec` stdio relays) keep working with interop
  off. The device bridge moves from Linux-launches-Windows (`device_mux.py` calling
  `Russet.Host.exe`) to the Windows supervisor connecting to Apple Devices' usbmux
  and relaying it into the distro over stdio.
- Applied after the components that still read the owner's Downloads and D: are in place.
  Until then (since 2026-10-31) Russet-Dev keeps automount on with
  `options = "metadata,uid=1000,gid=1000"`; the original file is `/etc/wsl.conf.russet-bak`.

## Setup runner contract (host tree, consumed by the W19 setup page)

The Windows supervisor runs setup steps; the extension never shells into WSL itself.
It finds the supervisor through `RUSSET_WINDOWS_HOST` (already set by the launcher).

- `Russet.Host.exe --setup status` prints one JSON line:
  `{"version":1,"steps":[{"id":"wsl","state":"done","title":"…","detail":"…"}, …]}`.
  `state` is `done | needed | running | failed | unavailable`; `detail` is user-facing
  text without private paths, device identifiers or account data.
- `Russet.Host.exe --setup run <id> [--arg key=value]…` prints NDJSON:
  `{"step":"<id>","event":"progress","message":"…","fraction":0.42}` (fraction optional),
  `{"step":"<id>","event":"log","message":"…"}`, then exactly one of
  `{"step":"<id>","event":"done"}` or
  `{"step":"<id>","event":"failed","code":"setup.<id>.<reason>","message":"…","retryable":true}`.
  Exit code 0 on done, 1 on failed. A second `run` of a done step is a no-op `done`.
  Cancel = kill the process; steps are re-runnable.
- Step ids, in order: `wsl` (WSL present; else instructions to install),
  `backend` (managed distro imported and isolated), `apple-devices` (Apple Devices
  app / usbmux reachable), `iphone` (paired, Developer Mode, DDI), `xtool`,
  `xcode-sdk` (`--arg xip=<Windows path>`; detection of `Xcode_*.xip` in Downloads
  is the page's job), `vphone` (build vphone-linux), `virtual-iphone`
  (`--arg device=iPhone17,3 --arg ios=27.0.1`; hours; resumable).
  The Apple account step is not here: the page calls `account.appleId.*` on the daemon.

## Order

0. Installer: WSL present (or prompt to install), managed distro imported and
   isolated as above.
1. Backend components that unblock everything: SDK from Xcode 27.2, vphone-linux
   built, fresh iOS 27 VM created from the owner's cached IPSWs (no firmware
   download). Prove the VM reaches vphoned health on WSL.
2. Host setup runner + IDE setup page wired to real steps (backend, Apple Devices
   and phone readiness, Xcode/SDK, virtual iPhone with version picker, project).
3. Engine GSA + anisette (W22) behind the native sign-in page; paid account first,
   then free-account provisioning. Then Xcode download from the signed-in session.
4. Virtual device provider in the daemon and in the device picker; install,
   launch and DAP debug on the virtual iPhone; embedded display and input.
5. Xcode-style toolbar polish, installed-app acceptance for both targets, packaging.

Release gate unchanged: both the physical and the embedded interactive virtual
iPhone must build, install, launch, stop at a source breakpoint, show variables,
step and continue from the installed IDE.
