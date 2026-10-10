# Facts verified on real hardware

These are things we ran ourselves, as opposed to research claims (for those, see
[research/landscape-2026-10.md](research/landscape-2026-10.md)). Newest first.

Test rig:
- Linux: CachyOS (Arch), Ryzen 5 5600X.
- iPhone: iPhone 12 (iPhone13,2, A14), iOS 27.0 (24A437), connected over USB.
- Mac, used as an oracle only: MacBook Air M1, macOS 27.0.1 (26A434), Xcode 27.0 (27A266a).

## 2026-10-11: M1 baseline works with off-the-shelf tools

Script: [`demo/m1-baseline.sh`](../demo/m1-baseline.sh).

1. **Build.** `xtool dev build` produced a SwiftUI `Hello.app` in about 7 s.
   - Toolchain: Swift 6.4 (AUR `swift-bin`) and the Darwin SDK imported from
     `Xcode_27.xip` by `xtool setup` (darwinTools 1.1.0, OpenAppleMacros 1.3.0).
   - Mach-O `LC_BUILD_VERSION` is platform iOS, but `sdk 17.0`/`minos 17.0`: the SDK
     field mirrors the deployment target, not 27.0.
   - Info.plist has no `DT*` keys. This is relevant for App Store ingestion and for
     SDK-linked behaviour such as the UIScene requirement.
2. **Provision, sign and install.** `xtool dev run --usb` did this with the App Store
   Connect API team key (role App Manager).
   - xtool installs under the bundle ID `XTL-<issuer-id prefix>.<bundleID>`.
     Russet must let paid users keep their exact bundle IDs.
   - xtool shells out to the `zip` binary (`zip -yqru0`), which is not installed by
     default on Arch. Russet must package IPAs natively.
   - With a paid team, xtool creates a new development certificate and does not
     revoke existing ones. It revokes only on free teams, after asking.
3. **Launch.** `pymobiledevice3 developer dvt launch --suspended --userspace <bundle>`
   returns a pid.
   - Plain debugserver `process launch` fails on iOS 17+ with
     `89;Operation not permitted`.
4. **Debug.** `pymobiledevice3 developer debugserver start-server --userspace --local-port N`
   plus Swift's LLDB (`lldb 21.0.0`, Swift 6.4).
   - Command sequence:
     1. `platform select remote-ios`
     2. `target create <local exe>`
     3. `SetPlatformFileSpec(<device path>)`
     4. async `process connect`
     5. `process attach --pid`
     6. `process continue`
   - Result: the breakpoint at `ContentView.swift:6` hit inside `ContentView.body.getter`;
     app frames were symbolized from the local build's debug map; `process kill` worked.
   - Gotcha: in `lldb -b` (synchronous) mode, `process connect` to a process-less
     debugserver hangs forever. Switch to async for the connect.
   - Gotcha: swift.org's Ubuntu-built LLDB needs `libpython3.12.so.1.0`. Arch ships
     3.14, and uv's CPython builds are static, so we built CPython 3.12 with
     `--enable-shared` in `~/.local/opt`. Russet should ship or locate a matching
     libpython, or drive LLDB without Python.

## 2026-10-10: Device services from Linux without root

- Pairing: `idevicepair pair` (libimobiledevice 1.4.0, usbmuxd 1.1.1) succeeded.
- Personalized DDI: `pymobiledevice3 mounter auto-mount` (11.26.0) mounted it on iOS 27.
  - It falls back to the no-root userspace tunnel.
  - The DDI is now delivered as a "Cryptex1" image, personalized via Apple TSS, and
    mounts at `/System/Developer`.
- DVT and debugserver:
  - DVT process list and the installed-app list both work.
  - The on-device debugserver answers gdb-remote `qHostInfo`:
    `cputype:16777228;cpusubtype:2;ostype:ios;os_version:27.0.0;vm-page-size:16384`.
  - The debugserver advertises `SupportedCompressions=lzfse,zlib-deflate,lz4,lzma`.
- Gotcha: `debugserver start-server` only works over the userspace tunnel with an
  explicit `--userspace --local-port`. The automatic fallback mode closes forwarded
  connections immediately.

## Xcode 27 facts, from the Mac oracle

- Xcode 27.0 (27A266a) ships Swift 6.4 (`swiftlang-6.4.0.34.1`) and Apple clang 21.
- These Xcode 27 binaries are **arm64-only** (no x86_64 slice): `xcodebuild`,
  `actool`, `ibtool`, `momc`, `swift-frontend` and `ld`. An Intel macOS VM cannot run
  them.
- The iOS 27 personalized DDI is installed at
  `/Library/Developer/DeveloperDiskImages/iOS_DDI`.
