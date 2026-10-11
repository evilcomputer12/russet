# Facts verified on real hardware

These are things we ran ourselves, as opposed to research claims (for those, see
[research/landscape-2026-10.md](research/landscape-2026-10.md)). Newest first.

Test rig:
- Linux: CachyOS (Arch), Ryzen 5 5600X.
- iPhone: iPhone 12 (iPhone13,2, A14), iOS 27.0 (24A437), connected over USB.
- Mac, used as an oracle only: MacBook Air M1, macOS 27.0.1 (26A434), Xcode 27.0 (27A266a).

## 2026-10-11: Review round 2 checks (no device commands; the Mac read-only)

- **LLDB batch mode is synchronous.** With Swift 6.4's LLDB 21.0.0 on this box (env from uv's PBS 3.12.15),
  `lldb -b -o 'process launch --stop-at-entry' -o 'process continue' -- /usr/bin/sleep 4` returned only after
  the process exited (6.2 s in total). The same run ending in `process detach` returned (7.2 s, mostly
  LLDB's own start-up and teardown) while `/usr/bin/sleep 20` kept running. So a batch script that should
  leave an app running must end with `process detach`, not `process continue` (ARCHITECTURE §8.7, [D 84]).
- **The Mac's DDI folder is a root, not a Restore directory.** `ls -la /Library/Developer/DeveloperDiskImages/iOS_DDI`
  shows `Restore/` and `version.plist`; `Restore/` holds `022-20190-411.dmg`, `022-20231-411.dmg`,
  `BuildManifest.plist`, `Restore.plist` and `Firmware/`. idevice's `Cryptex1Assets::load(restore_dir)` reads
  `restore_dir/BuildManifest.plist` (`idevice/src/services/cryptexd/assets.rs`, 3854a5d), so Russet passes
  `<dir>/Restore` ([D 82]).
- **idevice usbmuxd sizes.** At 3854a5d, `read_plist` rejects a declared size below 17 but then allocates
  `vec![0; size - 16]` for any larger value, up to about 4 GiB (`idevice/src/usbmuxd/mod.rs:467-474`).
  Russet's fork caps it at 16 MiB (ARCHITECTURE §16.7).

## 2026-10-11: Review round 1 fact-finding (no device commands; the Mac read-only)

### Where the iOS 27 DDI comes from (Mac oracle; Apple's CDN)

- Xcode 27.0 (27A266a) keeps the iOS DDI only inside
  `Xcode.app/Contents/Resources/Packages/XcodeSystemResources.pkg`: 146,413,318 B, xar v1, SHA-1 TOC
  checksum, RSA signature (`pkgutil --check-signature`: "signed Apple Software", chain "Software
  Update" → Apple Software Update Certification Authority → Apple Root CA).
  `Contents/Resources/CoreDeviceDDIs` does not exist.
- The `.pkg` installs `/Library/Developer/CoreDevice/CandidateDDIs/iOS_DDI.dmg` (UDIF UDZO,
  32,730,624 B; one journaled HFS+ volume, no decmpfs). `xcodebuild` then runs
  `devicectl manage ddis update`, which copies it to `/Library/Developer/DeveloperDiskImages/iOS_DDI`;
  `devicectl list preferredDDI` reports 27A266a, CoreDevice 642.16. CoreDevice has no network DDI
  source.
- Extracted on Linux, the `.dmg`'s 9 files are byte-identical (SHA-256) to the Mac's `iOS_DDI` folder
  (`.dmg` SHA-256 67d44aa638a0c597d0c4245f4809b9757e1b1fffda0e1b2bf585e987f78da426).
- One `BuildManifest.plist` carries both forms: 140 "Customer iOS Developer PDI" identities
  (PersonalizedDMG `022-20190-411.dmg`) and one "iOS Customer Developer Disk Image Cryptex" identity
  (`022-20231-411.dmg`; cryptex `com.apple.MobileAsset.DDI` 27.1.266.1). Each manifest Digest equals
  the image's SHA-384.
- Apple's CDN offers no standalone iOS 27 DDI: Pallas `com.apple.MobileAsset.XcodeDeviceSupport`
  answers 404 "Active not found"; `xcodebuild -showComponent DeviceSupport -json` says "No updates";
  Apple's downloadables index maps Device Support components only to older Xcode builds. Those older
  components (a `.dmg` holding the same `.pkg`) download anonymously from devimages-cdn.apple.com.
- pymobiledevice3 11.26 `mounter auto-mount` takes DDIs only from
  raw.githubusercontent.com/doronz88/DeveloperDiskImage (licence: none). The DDI mounted on the rig
  since 2026-10-10 is that mirror's beta 27A5228h (cryptex 27.1.5228.8).
- idevice 0.1.68 (and HEAD 3854a5d) `CryptexdClient::install()` sends `nonce_persistence` (1) as
  `persistence`; Xcode uses 2 (`src/services/cryptexd.rs:273-274`).
- Licences: Xcode 27.0 is EA2002 and the Device Support licence is EA1879; both authorize the Apple
  Software only on Apple-branded computers running macOS (§2.5, §2.7). See ARCHITECTURE §19 and Q26.

### The rig iPhone runs TXM on iOS 27 (firmware evidence, no device command)

- The iPhone13,2 27.0 (24A437) IPSW's BuildManifest lists, for D53gAP/D53pAP (ApChipID 0x8101),
  `Ap,SecurePageTableMonitor` (`Firmware/sptm.t8101.release.im4p`) and `Ap,TrustedExecutionMonitor` in all
  six identities. The four Customer Erase/Upgrade identities (0–3) use `Firmware/txm.iphoneos.release.im4p`;
  the two Research Customer identities (4–5, D53gAP) use `Firmware/txm.iphoneos.research.im4p` (corrected
  2026-10-11 by re-reading the extracted manifest; the TXM conclusion is unchanged). The 26.6.2 (23G90)
  IPSW has neither.
- The 27.0 kernel (xnu-13432.2.10) has no `__PPLTEXT`/`__PPLDATA` segments and adds `__DATA_SPTM` and
  TXM client strings; the 26.6.2 kernel still has the PPL segments.
- The `Ap,TrustedExecutionMonitor` digest is the same for chips 0x8030 through 0x8160 on iOS 27.0, and
  in 27.0.1 (24A446). Across every iPhone/iPad IPSW: iOS 26.6.2 has TXM only on 0x8110 (A15) and
  later; iOS 27.0 has it on everything except the A12Z iPad Pro (0x8027).
- Commands: `ipsw extract --remote --pattern '^BuildManifest\.plist$' <IPSW URL>` and
  `ipsw extract --remote --kernel --device iPhone13,2 <IPSW URL>` (ipsw 3.1.736), with URLs from
  `https://api.ipsw.me/v4/device/iPhone13,2?type=ipsw`, for example
  https://updates.cdn-apple.com/2026FallFCS/f35a2940-4b73-46b3-8ac7-01296d8a033b/iPhone13,2,iPhone13,3_27.0_24A437_Restore.ipsw
  and
  https://updates.cdn-apple.com/2026SummerFCS/35f1049f-080d-4483-bcca-05f47e3d6555/iPhone13,2,iPhone13,3_26.6.2_23G90_Restore.ipsw.

### Xcode 27 `xcodebuild -help` (Mac oracle, read-only)

- `-allowProvisioningUpdates` lets xcodebuild create and update profiles, App IDs and certificates for
  automatically signed targets; the `-authenticationKeyPath`, `-authenticationKeyID` and
  `-authenticationKeyIssuerID` flags exist.
- ExportOptions `method`: app-store-connect, release-testing, enterprise, debugging, developer-id,
  mac-application, validation (`app-store` is deprecated). `manageAppVersionAndBuildNumber` defaults to
  YES. A manually signed archive exported with `signingStyle = automatic` gets profiles and cloud
  certificates without device or App ID changes.
- `/usr/bin/openssl` on macOS 27.0.1 is LibreSSL 3.3.6.

### SDK import and NIB facts

- The xar RSA signature of `XcodeSystemResources.pkg` is prehashed: the recovered PKCS#1 v1.5
  DigestInfo is SHA-1 plus the raw 20-byte TOC checksum. The Payload is pbzx with 16 MiB chunks (XZ,
  or stored when incompressible) over an odc cpio.
- Installed Xcode 27.0: 10.05 GB logical, 166,836 entries, 11,140 symlinks. An iOS-only subset is about
  421 MiB in about 9.8k files.
- Swift 6.4-RELEASE on this box uses Apple's iOS `prebuilt-modules`: an `import Foundation` typecheck
  for arm64-apple-ios17.0 takes 1.15 s with them and 33.4 s without.
- SwiftPM 6.4 filters Swift SDK bundles by host triple ("No Swift SDK found matching query
  `arm64-apple-ios` and host triple `x86_64-unknown-linux-gnu`").
- Compiled storyboards (Apple-built Catalyst apps on macOS 27.0.1): `<Name>.storyboardc/` holds a
  binary `Info.plist` and flat nibs; NIBArchive coder version 10, with a 4-byte `LNE\0` trailer and the
  root key `IBUINibLNEVersionKey`.

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
     3.14, so we built CPython 3.12 with `--enable-shared` in `~/.local/opt` at the time.
     **Correction (2026-10-11):** uv's python-build-standalone 3.12.15
     (`~/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu`, 111 MB) does ship
     `lib/libpython3.12.so.1.0` and works with Swift LLDB (run-debug V1), so that custom build is
     redundant. Russet locates or provisions such a runtime (ARCHITECTURE §16.2).

## 2026-10-10: Device services from Linux without root

- Pairing: `idevicepair pair` (libimobiledevice 1.4.0, usbmuxd 1.1.1) succeeded.
- Personalized DDI: `pymobiledevice3 mounter auto-mount` (11.26.0) mounted it on iOS 27. (It came from
  pymobiledevice3's third-party GitHub mirror: beta build 27A5228h; see the 2026-10-11 section.)
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
- The iOS 27 DDI is installed at `/Library/Developer/DeveloperDiskImages/iOS_DDI`. It holds both the
  personalized (PDI) and the Cryptex1 identities; `devicectl manage ddis update` copies it there from
  `/Library/Developer/CoreDevice/CandidateDDIs`, which `XcodeSystemResources.pkg` installs (see the
  2026-10-11 section).
