# iOS development without a Mac: landscape, architecture and plan

**Date:** 2026-10-10 · **Scope:** develop, build, install, debug, test, profile and ship iOS apps (Swift/SwiftUI, UIKit, Objective-C; Flutter first among cross-platform stacks) from Linux and Windows with a physical iPhone, with no Mac in the loop · **Basis:** six fact-checked research dimensions produced by this workflow run · **Not legal advice.**

> **State of this machine (checked 2026-10-10, about 23:00)**
> - CachyOS, Ryzen 5 5600X, 31 GiB RAM, RX 7600. **No iPhone is connected.** Xcode is installed on your Apple Silicon Mac and is used only for reverse engineering and tracing; the product must not need it.
> - Present: libimobiledevice 1.4.0, usbmuxd 1.1.1, libplist 2.7.0, clang/lld/llvm 23.1.1, ipsw 3.1.736.
> - Added during this research run but not yet run: pymobiledevice3 11.26.0 (uv tool), go-ios 1.3.2 (`~/.local/bin/ios`, from `tools/go-ios-1.3.2/`), xtool 1.21.0 AppImage (`~/.local/bin/xtool` -> `tools/xtool-1.21.0/`).
> - Missing: `swift`, `lldb`/`lldb-dap`, `flutter`, `rcodesign`, `zsign`, `ideviceinstaller`, `unxip`.
> - Trap: the system lld 23.1.1 cannot read Xcode 27 SDK `.tbd` stubs (`arm64e.x1`). LLVM 23.1.3 fixes this, and xtool installs its own patched lld [tc-05][tc-46].
> - `scripts/debugserver_probe.py` (already in this tree) checks the on-device debugserver over pymobiledevice3's no-root tunnel without needing LLDB. Run it first once a phone is plugged in.

**How to read this.**
- IDs like [tc-05] point to the fact-checked research records of this run: `tc` toolchain and build system, `xt` closed Xcode build tools, `dv` deploy/debug/test, `sg` signing and distribution, `fx` Flutter and cross-platform, `xr` Xcode internals and the macOS fallback. Only records whose status is *confirmed* or *corrected* are treated as facts, using the corrected wording where one exists. [dv-17] (*unverifiable*) appears only as a hypothesis.
- [fc-tc], [fc-dv] and so on mark evidence that the fact-checkers of that dimension added. Where I re-checked such an item with `gh api` today, the text says "re-checked".
- Primary sources are linked inline. Raw copies are in `research/<dimension>/` and `research/<dimension>-factcheck/` next to this file.
- Status words: **WORKS** (demonstrated; the source is named and may be self-reported), **PARTIAL**, **GAP**, **UNVERIFIED**.
- `nomac` is a placeholder name for our kit.

## Contents

1. Executive summary
2. Component map by pipeline stage
3. Proposed architecture of the kit
4. Flutter path (then React Native, .NET MAUI and others)
5. Windows path
6. Reverse-engineering targets on your Mac
7. Role of the macOS emulation tracks
8. Milestones M0–M7
9. Risk register
10. Questions only you can answer
- Appendix A: version pins as of 2026-10-10
- Appendix B: upstream threads to watch

---

## 1. Executive summary

### 1.1 Bottom line

As of 2026-10-10, every stage from SDK to App Store upload has at least one open-source implementation that runs off-Mac and that someone has used against iOS 26 or 27. What is missing is a product:

- The pieces are spread across about ten projects.
- The projects that close the newest gaps (Flutter on Linux, App Store-grade resources, `.xcodeproj` handling) are under three months old and mostly have one maintainer. At least one, omarchy-apple-dev, is largely written and merged by AI agents.
- The debug path has to be assembled by hand.
- The closed resource compilers (actool, ibtool, App Intents, Core ML, Metal) have partial open replacements or none.
- Every build still needs Apple's iOS SDK, taken from an Xcode the user supplies, under a licence that permits only Apple hardware.

The shortest credible route is an **integration project** with three parts:
- **Reuse:** xtool for SDK import and SwiftPM builds, pymobiledevice3 for device services, swift.org and llvm.org LLDB for debugging, rcodesign for signing, and the Flutter techniques that xlinux and xcross have proven.
- **Build the glue nobody ships:** one-command run and debug, a Flutter device adapter, resource-compiler CLIs compatible with Swift Build, packaging lints, and a VS Code extension.
- **Use your Mac only as a black-box oracle** that produces golden outputs and protocol traces during development.

### 1.2 What already works off-Mac (Linux unless stated)

| Capability | Status | Evidence |
|---|---|---|
| iOS SDK from a user-supplied Xcode | WORKS: `xtool sdk install` takes `Xcode.xip`, an `Xcode.app` or a prebuilt `darwin.xtoolsdk` and installs a SwiftPM "darwin" Swift SDK | [tc-10][tc-11]; xtool `Sources/XToolSupport/SDKCommand.swift` @1.21.0 |
| Swift 6.4 / SwiftUI / ObjC compile for `arm64-apple-ios` | WORKS with swift.org Swift 6.4.0 and the Xcode 27 SDK. The Swift minor version must match the SDK | [tc-01][tc-02][tc-03][tc-12] |
| Apple SDK macros | PARTIAL: OpenAppleMacros 1.3.0 covers SwiftUI and FoundationModels. SwiftData, AppIntents and TipKit are missing upstream | [tc-14] |
| Linking | WORKS with LLVM 23.1.3 or later, or with xtool's patched lld. `-r` goes through a libtool trampoline. Pointer-authentication (arm64e) code does **not** link | [tc-04]–[tc-07]; llvm PR #188378 still open (re-checked) |
| Asset catalogs, storyboards | PARTIAL: AssetKit (a library only) and omarchy-apple-dev's Linux actool plus a byte-identical ibtool subset. xtool has neither | [xt-08][tc-22][tc-23][xt-21] |
| Code signing | WORKS: rcodesign 0.29.0, or zsign 1.0.5+ (needed for the iOS 26 app-extension rule) | [sg-04][sg-05][sg-07] |
| Provisioning, paid account | WORKS, officially: App Store Connect API with a team key | [sg-21] |
| Provisioning, free Apple ID | WORKS but fragile: private GrandSlam and developerservices2 APIs, which Apple broke for every client on 2026-09-10 | [sg-12][sg-13][sg-14] |
| Pair, Developer Mode, DDI mount, install on iOS 17.4–27 (including the iPhone 18 Cryptex DDI) | WORKS without root: pymobiledevice3 11.26.0 | [dv-04]–[dv-07][dv-36] |
| Launch suspended, environment variables, stdout | WORKS: pymobiledevice3 CoreDevice and DVT services | [dv-35][dv-38] |
| Native debugging (breakpoints, stepping) | WORKS by hand: debugserver proxy forwarded to localhost, plus upstream LLDB `remote-ios`. No packaged flow exists | [dv-10][dv-11][dv-13] |
| Swift `po` / expression evaluation | UNVERIFIED: would need a Swift-aware LLDB pointed at the iOS SDK | [dv-17] |
| Flutter debug + hot reload | WORKS (community): xlinux with real JIT through upstream LLDB (claims an iPhone 11 on the iOS 27.2 beta); xcross on Linux and Windows, probably interpreted on iOS 26+ | [fx-24][fx-25][fx-26] |
| Flutter release (AOT) | WORKS (community): an iOS-target `gen_snapshot` rebuilt for Linux and Windows hosts | [tc-34][fx-08][fx-09] |
| XCUITest / WebDriverAgent | WORKS with prebuilt runners | [dv-37] |
| Profiling | PARTIAL: raw DVT instrument streams, no `.trace` files | [dv-38][xr-22] |
| TestFlight / App Store upload | WORKS: App Store Connect API build uploads from any OS. Linux-built apps processed as VALID per omarchy-apple-dev, and iOSBuildX reports TestFlight uploads; both are self-reported | [sg-25][sg-26][sg-32][fx-28] |
| Simulator, SwiftUI Previews, Instruments UI, view debugger | GAP | [tc-13][xr-21][dv-39] |

**Windows today.**
- The same device stack runs natively: pymobiledevice3 with Apple's driver service, without Admin rights for the iOS 17.4+ tunnel [dv-05].
- LLDB and lldb-dap ship in the LLVM 23.1.3 Windows installer [dv-12].
- rcodesign and zsign have Windows builds [sg-39].
- xcross builds Flutter apps natively and hot-reloads them [tc-29].
- xtool itself needs WSL2 [tc-10].

### 1.3 What does not work yet

- **No one-command run and debug.** `xtool launch` fails on iOS 17+ (#44). go-ios `ios debug` and libimobiledevice's `idevicedebug` use the pre-iOS-17 debugserver. Xcode's own attach path is a closed LLDB `device` command [dv-41][dv-28][dv-02][dv-13].
- **Closed resource compilers.**
  - App Store uploads need a compiled asset catalog with `CFBundleIconName` (ITMS-90713) and a 1024 px "Any Appearance" icon [xt-10]. Open actool work is partial, and no build made with it has been installed from TestFlight yet [xt-08][sg-32].
  - ibtool exists only as a byte-identical subset [tc-23].
  - App Intents metadata, Core ML, Core AI (`aimodelc`) and the Reality/SceneKit tools have no open replacement [tc-24].
  - Metal has only Apple's Windows compiler [xt-25].
  - Icon Composer `.icon` files need Apple's renderer [xt-05].
- **Xcode projects.** There is no mature open `.xcodeproj` to PIF converter, only two new single-maintainer projects and an xtool plan (#144) [tc-19][xr-19].
- **Pointer-authentication apps** (Enhanced Security, arm64e) cannot be linked with stock lld [fc-tc].
- **System symbols.** Off macOS, LLDB cannot read the device's dyld shared cache from disk, and `ipsw dyld split` is macOS-only. Attaching therefore reads libraries out of process memory, which is slow [dv-16].
- **CocoaPods-only dependencies.** CocoaPods runs on Linux, but its output needs `xcodebuild`. The trunk goes read-only on 2026-12-02 [tc-36][tc-37].
- **Legal footing.**
  - The Xcode and Apple SDKs Agreement (EA2002, 2026-06-08) §2.5: users are "expressly prohibited from separately using the Apple SDKs or attempting to run any part of the Apple Software on non-Apple-branded hardware" [fx-47][tc-31].
  - Developer Disk Images reach open tools only through unlicensed re-hosts [dv-08].
  - The free-account path uses private Apple endpoints [sg-12][fc-sg].
  - App Store acceptance of Linux builds currently relies on stamping Xcode and SDK metadata [sg-30][fx-29].
- **Mac-only developer tools:** Simulator (xtool #311), the Previews runtime, Instruments `.trace`, view-hierarchy and GPU debugging [tc-13][xr-21][xr-22][dv-39].

### 1.4 Shortest path to the two demos

**A. SwiftUI app: build, install and debug from Linux (the M1 target).** Steps 1–3 can run now; steps 4–7 need the iPhone plugged into this machine.

1. **SDK.** Download Xcode 27.x in a browser with your Apple ID (the download requires a login), or reuse the Xcode on your Mac. Then run one of:
   - `xtool sdk install <Xcode.xip | Xcode.app>`;
   - `xtool setup`, which first asks for a login mode (API key for paid accounts, or password for any Apple ID through private APIs) and then for the `.xip` path.

   `swift sdk list` should then print `darwin` (xtool `Documentation/xtool.docc/Installation-Linux.md` @1.21.0) [sg-16].
2. **Toolchain.** Install swift.org Swift 6.4.0. The UBI10 tarball works without sudo, and AUR `swift-bin` 6.4.0 also exists. Xcode 27 SDKs only work with Swift 6.4 [tc-03][tc-41].
3. **Build.** `xtool new Hello && cd Hello && xtool dev build` produces `./xtool/Hello.app`. Give it a PNG icon through `iconPath` and keep the default empty `UILaunchScreen`; development installs need no asset catalog [xt-11].
4. **Pair.**
   - Run `idevicepair pair`. If iOS 26 drops the device while the Trust prompt is still up, run usbmuxd with `--no-preflight` (reported on CachyOS, usbmuxd #281) [dv-40].
   - Reveal Developer Mode with `pymobiledevice3 amfi reveal-developer-mode` [dv-36].
   - Mount the DDI with `pymobiledevice3 mounter auto-mount` [dv-07].
5. **Sign and install.** Use either:
   - `xtool dev`, which signs and installs. Its launch step uses the legacy debugserver and fails on iOS 17+ (#44).
   - rcodesign to sign, then `pymobiledevice3 apps install --streaming --developer ./xtool/Hello.app` [sg-16][dv-41][xr-15].
6. **Launch suspended.** Use either:
   - `pymobiledevice3 developer dvt launch "<bundle-id>" --suspended`, which prints the pid;
   - `pymobiledevice3 developer core-device launch-application <bundle-id> --suspended`.

   Source: pymobiledevice3 `cli/developer/dvt/__init__.py` and `cli/developer/core_device.py`.
7. **Debug.** `pymobiledevice3 developer debugserver start-server --local-port 41234` forwards the device's debugserver over the no-root userspace tunnel [dv-10]. Then run in LLDB:
   1. `platform select remote-ios`
   2. `target create ./xtool/Hello.app/Hello` (point at the executable: LLDB cannot resolve `.app` bundles off Apple platforms [dv-14])
   3. `SetPlatformFileSpec(...)` with the on-device path
   4. `process connect connect://127.0.0.1:41234`
   5. `process attach --pid <pid>`

   Try the Swift-aware LLDB from the swift.org toolchain first; whether it contains `remote-ios` is not yet binary-verified. Otherwise use Arch `lldb` 23.1.1, which is verified to contain `PlatformRemoteiOS` [dv-11].

The demo is done when a breakpoint in Swift code stops on the phone.

**B. Flutter app with hot reload (the M2 target).**

1. Flutter 3.47.x. `flutter precache --ios` fetches Flutter.xcframework on Linux [fx-04][tc-33].
2. Debug builds contain no AOT code. A debug build needs a kernel from the Linux `frontend_server`, a one-line stub `App.framework`, and Flutter.xcframework [fx-12][fx-13].
3. Use a storyboard-free UIScene Runner (UIScene is mandatory with the Xcode 27 SDK) and SwiftPM plugins [fx-31][xt-12][fx-33].
4. Register the phone as a Flutter **custom device** declared as `linux-x64` (`flutter config --enable-custom-devices`). Its install and run commands perform steps 4–7 of path A, and `flutter run` then gives hot reload and DevTools. This is the xlinux technique [fx-27].
5. Launch with `--enable-dart-profiling --enable-checked-mode --verify-entry-points --vm-service-port=N` [fx-16].
   - On iOS 26+, attach LLDB with Flutter's RX-page helper, writing 8 bytes per 16 KiB page as xlinux does.
   - On iOS 27, continue manually at each stop.
   - Without a debugger, iOS 26+ falls back to Dart's interpreter [fx-18]–[fx-22].
6. Release builds:
   1. Build an iOS-target `gen_snapshot` for Linux from the engine's pinned Dart revision, using the documented GN patch.
   2. Emit `App.framework` directly with `--snapshot_kind=app-aot-macho-dylib`.
   3. Finish with LLVM tools and rcodesign [fx-08][fx-10].

Quickest demo: run xlinux unmodified, then fold its approach into the kit. It is GPL-3.0 Python, the same licence as the kit.

**C. Windows.**

1. Start with the "split" route.
   - Build inside WSL2, which is xtool's supported path. The device is reached there through usbipd or a relay to Windows' usbmuxd.
   - Run the device stack, tunnel and LLDB natively on Windows: pymobiledevice3 with iTunes' Apple Mobile Device Service, and LLDB plus lldb-dap from the LLVM 23.1.3 MSI.
2. Then add a native build plane on Swift for Windows and stock LLVM, following xcross's workarounds for `MAX_PATH`, command-line length and symlinks [tc-10][tc-29][dv-12][dv-32].

---

## 2. Component map by pipeline stage

### 2.0 Overview

| # | Stage | Best existing component | iOS 26/27 status | Our action |
|---|---|---|---|---|
| 1 | SDK acquisition | xtool's SDK builder (libunxip), fed a user-supplied Xcode | Works with Xcode 27. The `.xip` is arm64-only, but the SDK data is architecture-neutral | Reuse plus a wrapper; never redistribute Apple files |
| 2 | Project model, build orchestration | xtool + Swift Build (SwiftPM 6.4) | Works for SwiftPM. Any `.xcassets` breaks Swift Build builds (#310) | Reuse and contribute; `.xcodeproj` to PIF later |
| 3 | Compile | swift.org Swift 6.4, clang, OpenAppleMacros | Works when the Swift minor version matches the SDK | Reuse; build a version-lock manager; contribute macros |
| 4 | Link, post-link | lld 23.1.3+ or xtool darwin-tools; LLVM 23+ binutils | Works, except arm64e | Pin versions, lint, watch llvm #188378 |
| 5 | Resources | AssetKit; omarchy stand-ins | Development builds need none. The App Store needs a compiled catalog | Build CLIs on AssetKit; avoid storyboards; reverse-engineer golden corpora |
| 6 | Package | xtool Packer; omarchy ship scripts | ITMS rules; iOS 26 needs DT* keys to register an app | Contribute; build lints |
| 7 | Sign, provision | rcodesign/zsign; ASC API; xtool XKit | iOS 26 extension rule; Apple's GSA changes | Reuse behind a wrapper |
| 8 | Install, connect | pymobiledevice3 | Tracks iOS 27.2; Cryptex DDI | Reuse; take DDIs from the user's own Xcode |
| 9 | Launch, debug | pymobiledevice3 + LLDB | iOS 26 JIT rules; iOS 27 breakpoint-stop quirks | Build the integration (highest leverage) |
| 10 | Test | pymobiledevice3, go-ios, idevice and Appium runners | Runner aborts reported on iOS 26.4.2 | Build a `test` command |
| 11 | Profile | pymobiledevice3 DVT | Works (raw streams) | Later: exporters |
| 12 | Distribute | ASC API build uploads | Xcode 26 SDK required now; an Xcode 27 requirement is likely around April 2027 | Build `ship`; offer an Apple-hardware release mode |

### 2.1 SDK acquisition

- **Best: xtool 1.21.0's SDK builder.**
  - It unpacks the `.xip` in-process with libunxip: XAR, then pbzx/LZMA chunks, then cpio.
  - It keeps the iPhoneOS, iPhoneSimulator and MacOSX platforms and the `XcodeDefault.xctoolchain` Swift libraries.
  - It writes a schema-4.0 `swift-sdk.json` for `arm64-apple-ios`, the simulator triples and macOS, with toolsets that use a patched `ld64.lld` and `llvm-lib`.
  - It swaps in the host's clang headers and symlinks XCTest, Testing and XCUIAutomation into the SDK.
  - Installed size is about 7 GB, or about 3 GB with `--slim` [tc-11][tc-30][tc-40].
  - `xtool sdk build <Xcode.xip|Xcode.app> <dir>` produces a portable `darwin.xtoolsdk`, and `xtool sdk install` accepts all three forms (`SDKCommand.swift` @1.21.0).
  - Alternatives: xcross `darwin_sdk_kit` (pure Dart, works on Windows) [tc-30]; CrossCode (stale; supports Xcode 26 and Swift 6.2) [tc-45].
- **iOS 26/27.**
  - Xcode 27.0 (27A266a, 2026-09-14) ships as a single arm64-only `.xip`. Its toolchain binaries are useless off-Mac, but the SDK data is fine [tc-01][tc-30][xr-02].
  - The back-deployment libraries (`swiftCompatibility*`) exist only inside Xcode [tc-39].
  - Simulator runtimes and the Metal toolchain are separate downloads [xr-09].
- **Gaps.**
  - Licence: EA2002 §2.5 and §2.7 [tc-31][fx-47].
  - swift.org publishes no Apple-platform Swift SDK [tc-02].
  - The Xcode 27 `.xip` size is not published [xr-07].
  - The SDK-free mode covers only ObjC/C, a tiny Swift/SwiftUI subset and Flutter release builds [tc-32]. It uses link stubs cut from a dyld cache or IPSW with `ipsw dyld tbd`, plus hand-written headers. IPSW contents are Apple software too [fc-sg].
- **Action.**
  - *Reuse* xtool's builder as a subprocess.
  - *Build* `nomac sdk`:
    - imports a user-supplied Xcode;
    - reads the SDK's compiler version from its `.swiftinterface` headers;
    - selects or installs the matching toolchain;
    - records provenance;
    - never fetches Apple files except with the user's own login.
  - Keep the SDK-free mode only as a research hedge.

### 2.2 Project model and build orchestration

- **Best for SwiftPM apps: xtool** (`Package.swift` plus `xtool.yml`).
  - On Swift 6.4+ it runs `swift build --build-system swiftbuild --triple … --toolset <sdk>/toolset-swb.json` with `XCODE_EXTRA_PLATFORM_FOLDERS`. Below 6.4 it uses SwiftPM's native build [tc-12].
  - SourceKit-LSP works through xtool's BSP server (`xtool dev build-server`) [tc-12].
- **iOS 26/27.**
  - Swift Build is SwiftPM 6.4's default and applies xcodebuild's resource rules. Any package containing `.xcassets` therefore makes it run `actool`, which does not exist on Linux (xtool #310, opened 2026-10-10) [tc-16][tc-17][xt-21].
  - Apple documents the Apple-platform plugin as requiring Xcode on macOS, so the Linux route is unofficial [tc-16].
- **Gaps.**
  - **`.xcodeproj` and `.xcworkspace`.** Nothing open generates Swift Build's PIF except:
    - viraptor/xcbuild, a `DumpPIF` fork of Facebook's xcbuild;
    - viraptor/dumppif (Rust, no licence), which claims byte-identical xcodeproj objects against Xcode 26.6 [tc-19].
  - **Partial pieces:**
    - xtool #144 is planned, and xtool main already parses `project.pbxproj` and the new JSON `project.xcproj` via apple/xcode-project-format (Apache-2.0) [tc-13][tc-20].
    - LibreDarwin/IDETools (BSD-3, created 2026-09-25) reimplements `xcodebuild -showBuildSettings` with parity on Xcode 26.4. PIF output is deferred [fc-xr, re-checked].
    - omarchy's `xcodeproj2xtool.py` turns Xcode targets into SwiftPM adapters [tc-21].
  - **xtool planner bugs:** trait-conditional dependencies (#154) and branch pins (the fix, PR #290, is still open) [tc-13].
  - **CocoaPods** runs on Linux, but its output needs `xcodebuild`. The trunk goes read-only on 2026-12-02, and the Xcodeproj gem cannot read `.xcproj` yet (#1052) [tc-36][tc-37][tc-20].
- **Action.**
  - *Reuse* xtool, and *contribute* to #144 and #290.
  - *Build* (M7) a `.xcodeproj` to PIF generator on tuist/XcodeProj and apple/xcode-project-format. Validate it against a corpus of `xcodebuild -dumpPIF` outputs from your Mac (RE target P0-2).
  - Until then, generate SwiftPM adapters.

### 2.3 Compile

- **Best.**
  - swift.org Swift 6.4.0, released 2026-09-14/15 for Linux distributions and Windows and also in the AUR [tc-02][tc-41].
  - Stock clang 19 or newer for C, Objective-C and C++; Xcode 26+ ships libc++ 21 [tc-25].
  - A package's own macros compile for the host [tc-15].
  - Apple SDK macros run through OpenAppleMacros 1.3.0: a statically linked replacement for `swift-plugin-server` plus empty stub libraries [tc-14].
- **iOS 26/27.**
  - The Swift minor version is locked to Xcode: 27.x needs 6.4, 26.4–26.6 need 6.3, 26.0–26.3 need 6.2 [tc-01][tc-03].
  - An older compiler fails with "this SDK is not supported by the compiler". A newer one breaks SDK macros and runtime symbols [tc-03].
  - Some distro Swift builds lack the AArch64 backend (Gentoo) [tc-41].
- **Gaps.**
  - SwiftData, AppIntents, TipKit and StateReporting macros are missing; a fork adds SwiftData and FoundationMacros [tc-14].
  - `#Predicate` and `#Expression` expand to FoundationEssentials references [tc-15].
  - Swift Build has Linux quirks: `.strings` encoding detection, cross-import overlays, `ulimit -n 65536` [tc-42].
- **Action.**
  - *Reuse.*
  - *Build* the version-lock manager and an Xcode x Swift CI matrix.
  - *Contribute* macro families, with expansion-parity tests driven from the Mac (RE target P1-8).

### 2.4 Link and post-link

- **Best.** `ld64.lld` from either source [tc-46][tc-05]:
  - xtool-org/darwin-tools-linux-llvm v1.1.0: static lld, dsymutil and llvm-libtool-darwin, which reverts Swift's Darwin block and adds `arm64e.x1`;
  - stock LLVM 23.1.3 or later.

  It covers what iOS apps need [tc-07]:
  - chained fixups (PIE arm64, iOS 13.4+);
  - `-dead_strip`;
  - ObjC relative method lists and category merging;
  - selector stubs (fast and small);
  - autolink controls.

  Chromium ships its iOS release builds with LLD [tc-07].
- **iOS 26/27.**
  - Xcode 27 `.tbd` stubs declare `arm64e.x1`. LLD 23.1.2 and older reject the whole file; this was reproduced here with 23.1.1 (`research/toolchain-factcheck/lldtest/repro.sh`). 23.1.3 (2026-10-06) is the first release with the fix [tc-05].
  - The Swift 6.4 toolchain's own lld refuses Apple platforms by design [tc-04].
  - Xcode 27 removed classic ld64, and Apple's current linker is closed source [tc-09].
  - iOS 27 dyld refuses images whose LINKEDIT string pool is only 4-byte aligned [sg-08][tc-26]:
    - `llvm-strip` and `llvm-install-name-tool` before LLVM 23, and Rust 1.97 or earlier, produce such images.
    - Swift 6.4's LLVM-21-based binutils are therefore affected; this host's 23.1.1 is not.
- **Gaps.**
  - `-r` is unimplemented; xtool rewrites it to `libtool -static` [tc-06].
  - Pointer-authentication (arm64e) relocations fail in stock lld. The fix, llvm PR #188378, is still open; it was last updated 2026-04-24 [fc-tc, re-checked].
- **Action.**
  - *Pin* xtool's toolset or LLVM 23.1.3+.
  - *Build* a post-link lint for string-pool alignment, `LC_BUILD_VERSION` and exec-segment flags.
  - *Watch* #188378. arm64e apps are out of scope until it lands.

### 2.5 Resources

Swift Build's open xcspecs are the interface contract. They list every closed tool an Xcode-style build can call, with its exact command line [xt-01][tc-24]. The steps that are not closed tools are already open in Swift Build and run on Linux: Info.plist processing, strings/plist copy, asset-catalog link and Swift stdlib embedding [xt-02].

**Development installs need none of the closed tools.** xtool ships a loose PNG icon with `CFBundleIconFile` and an empty `UILaunchScreen` [xt-11]. App Store builds do need them.

| Input | Closed Xcode tool | Best open option | Status | Our action |
|---|---|---|---|---|
| `.xcassets` | `actool`. Under Swift Build it runs up to three times: thinned, unthinned, and GenerateAssetSymbols, which writes `GeneratedAssetSymbols.swift/.h/-Index.plist` [xt-03][xt-04] | xtool-org/AssetKit: MIT, clean-room, CoreUI 970, a library with no CLI. omarchy's fork adds a CLI, Icon Composer pre-render, HEIC, PDF and dark/tinted icons [xt-08][tc-22] | PARTIAL. See the notes below the table | **Build `nomac-actool`.** See the notes below the table |
| `.icon` (Icon Composer) | `actool` plus Apple's renderer | Only approximate, unlicensed renderers (JeanxPereira/Icon-Composer, liquid-composer) [xt-05] | GAP | Require a classic appiconset, or render once on a Mac; reverse-engineer later |
| `.storyboard`, `.xib` | `ibtool` (compile and link) | omarchy `tools/ibtool`, a byte-identical subset [tc-23]. NIBArchive codecs exist (michaelwright235/nibarchive, Rust, Apache-2.0); per-class UIKit encoding is the hard part [xt-18][xt-19] | GAP beyond small subsets | **Avoid them**: `UILaunchScreen` plus a programmatic UIScene delegate (Apple TN3187) [xt-12], and convert simple launch storyboards. A narrow compiler later. Xcode 27 ibtool's `toolchain` mode, which needs no simulator, is the oracle [xt-16] |
| `.xcstrings` | `xcstringstool` (compile, generate-symbols) | algoriddim/xcstringstool (Rust, MIT, no `.stringsdict`) [xt-27]; omarchy stand-in [tc-24] | PARTIAL | Add `.stringsdict` and symbol generation; contribute upstream |
| `.strings`, plists, Info.plist, stdlib, asset link | `builtin-*` steps | Swift Build (open) | WORKS. Real Xcode iOS builds pass `--outputencoding binary` [xt-02] | Reuse |
| `.xcdatamodeld` | `momc`/`mapc` | omarchy momc (self-reported); building `NSManagedObjectModel` in code [xt-24] | PARTIAL | Evaluate; code generator later |
| `.metal` | `metal`/`metallib` | Only Apple's proprietary Metal Developer Tools for Windows. Runtime `makeLibrary(source:)` works, but SwiftUI `ShaderLibrary` needs a precompiled metallib [xt-25] | GAP on Linux | Document; Apple's tool on Windows; test under Wine |
| App Intents | `appintentsmetadataprocessor` (plus NL training and shortcut-strings processors) and the AppIntents macros | omarchy stand-in (self-reported) [tc-24][xt-26] | GAP | Later; reverse-engineer from `.swiftconstvalues` |
| Core ML / Core AI | `coremlc`, `aimodelc` (Xcode 27) | Only runtime `MLModel.compileModel(at:)` [xt-24][xt-33] | GAP | Unsupported; runtime fallback |
| SpriteKit, SceneKit, Reality, reference objects, ExtensionKit, PNG crush | `TextureAtlas`, `scntool`…, `realitytool`, `referenceobjectc`, `exutil`, `copypng`, `appleProductTypesTool` | None [tc-24][xt-01] | GAP | Unsupported, with clear errors |

Notes on `.xcassets`:
- **Status.**
  - The App Store needs a compiled `Assets.car` plus `CFBundleIconName` (ITMS-90713) and a 1024 px "Any Appearance" icon [xt-10].
  - AssetKit rejects single-size icons, uses CRC32 name identifiers where actool's hash is unknown, and has not been tested with App Store Connect [xt-09].
  - omarchy reports VALID builds after fixing the BOM index and `BITMAPKEYS` [sg-31].
- **Our action: `nomac-actool` on AssetKit**, honouring Swift Build's contract:
  - selected through `ASSETCATALOG_EXEC`;
  - answers `--version --output-format xml1` with a plist keyed `com.apple.<basename>.version`;
  - writes binary dependency info, a partial Info.plist and the GeneratedAssetSymbols files.

  Then add single-size icon expansion and appearances, upstream the work to AssetKit and xtool, and validate it against a golden corpus.

### 2.6 Package

- **Best.**
  - xtool's Packer assembles the bundle: Info.plist, icon and embedded products [tc-13][xt-11].
  - omarchy's `ship.sh` and `tools/asc.py` show the shape the App Store wants [sg-30][sg-31]:
    - dylibs wrapped as frameworks;
    - `DT*` keys stamped into the app and every extension;
    - the `LC_BUILD_VERSION` SDK field rewritten in every Mach-O.
- **iOS 26/27.**
  - Per xcross, iOS 26+ installs an app that lacks the `UIDeviceFamily`/`DT*` group but refuses to register it. This is a single source [dv-26].
  - App Store checks [sg-30][sg-31]:
    - `DTPlatformName` and the SDK/Xcode version (ITMS-90725, read from `LC_BUILD_VERSION`);
    - no loose dylibs (90426);
    - extension versions must match the app (90473);
    - `arm64` must be in `UIRequiredDeviceCapabilities` (90502).
  - Flutter debug builds also need `NSBonjourServices` and `NSLocalNetworkUsageDescription` [fx-14].
- **Gaps.**
  - omarchy's dylib, framework and LSP fixes are not upstream; xtool PRs #290–#293 and #302 are still open [fc-tc].
  - There is no API for uploading dSYMs [sg-38].
- **Action.**
  - *Contribute*: review and test those PRs.
  - *Build* packaging lints and opt-in, visible metadata stamping.

### 2.7 Sign and provision

- **Signers.**
  - **rcodesign 0.29.0** (MPL-2.0) [sg-01]–[sg-05]:
    - signs bundles recursively;
    - takes keys from p12, PEM, a YubiKey or the Windows certificate store;
    - sets `CS_EXECSEG_MAIN_BINARY` on every executable;
    - does not handle IPAs or profiles: you copy `embedded.mobileprovision` yourself and pass the entitlements and `--team-name`;
    - has had no release since 2024-11-29.
  - **zsign v1.1.2** (MIT): handles IPAs and profiles and takes entitlements from the profile. It has the iOS 26 fix since v1.0.5 [sg-06][sg-07].
- **Paid accounts.**
  - The App Store Connect API with a team key covers certificates, devices, bundle IDs, capabilities and profiles. Tokens are ES256 JWTs valid for at most 20 minutes; individual keys cannot use the provisioning endpoints [sg-21].
  - App records still need the web UI [sg-22].
  - fastlane match, cert and sigh run on Linux; set `generate_apple_certs` explicitly [sg-24].
- **Free accounts.**
  - The flow is GrandSlam SRP login, then anisette headers, then developerservices2 [sg-12][sg-13]. Anisette is generated locally with Apple's Android `libCoreADI.so` through xadi, or fetched from remote servers.
  - Implementations: xtool XKit, isideload, Impactor, AltSign.
  - Apple's limits: 10 App IDs and 3 devices per 7 days, 3 apps per device, 7-day profiles, 9 capabilities [sg-10][sg-11].
- **Breakage.**
  - iOS 26 AMFI kills extensions without `MAIN_BINARY` [sg-07].
  - iOS 27's LINKEDIT alignment rule applies to any re-signing or rewriting step [sg-08].
  - From about 2026-09-10, Apple's GrandSlam service (GSA) returned 503 to the Xcode client identity, plus intermittent 429s. Clients switched to `com.apple.akd` within days [sg-14].
  - Free-account installs fail with 0xe8008015 across several tools on iOS 26.5 and 27; nab138/iloader#570 is still open (re-checked) [fc-sg].
- **Action.**
  - *Reuse* rcodesign, with zsign as a fallback, behind our wrapper.
  - *Reuse* xtool's XKit for free accounts as an explicit opt-in.
  - *Build* a small ASC API client, or call the MIT `asc` CLI [sg-37].

### 2.8 Install and connect

- **Best: pymobiledevice3 v11.26.0** (GPL-3.0) [dv-04]–[dv-07][dv-30][xr-15]:
  - usbmux and lockdown;
  - an in-process RSD/RemoteXPC tunnel for iOS 17.4+ through CoreDeviceProxy, built on a pure-Python TCP stack, with no root;
  - kernel tunnels (root/Admin) only for external tools that must reach the tunnel address, and for iOS 17.0–17.3.1 on Linux and Windows;
  - DDI auto-mount: classic DMG below iOS 17, PersonalizedDMG for 17–26.x, Cryptex1 from 27. The iPhone 18 only works with the Cryptex DDI;
  - Xcode-style streaming install;
  - iOS 27 device-initiated pairing.
- **Alternatives.**
  - go-ios (MIT) has a userspace gVisor tunnel. Its Cryptex DDI support is only on unreleased main, and the maintainers' device farm stops at iOS 18 [dv-09][dv-27][dv-28].
  - idevice (Rust, MIT, C FFI) is embeddable but breaks its API every point release until 0.2.0 [dv-29].
  - libimobiledevice 1.4.0 covers lockdown services only [dv-01][dv-02].
  - netmuxd adds Wi-Fi devices [dv-31][dv-32].
- **iOS 26/27.**
  - The protocol changes every release [dv-34]:
    - iOS 26 DDIs require streaming app lists;
    - from 26.2 the device keeps one RSD peer per host;
    - 27.2 adds message-id parity.
  - QUIC tunnels were removed in iOS 18.2; TLS-PSK needs Python 3.13+ [dv-33].
  - First pairing fails on Linux when the Trust prompt is slow [dv-40].
  - In one field report, a Linux host did not appear for iOS 27.0.1 Wi-Fi pairing, likely because of a firewall [dv-31].
- **Gaps.** DDIs come from github.com/doronz88/DeveloperDiskImage (Apple files, no licence) or deviceboxhq.com. Personalization needs Apple's TSS server online [dv-08].
- **Action.**
  - *Reuse* pymobiledevice3 as a library behind a backend interface, so idevice or go-ios can be swapped in.
  - *Build* "DDI from your own Xcode" import (RE target P0-4).
  - *Contribute* fixes upstream.

### 2.9 Launch and debug

- **Best.**
  - **Launch:** pymobiledevice3's CoreDevice launch (`--suspended`, `--env`, and `--console`, which streams stdout/stderr through `openstdiosocket`) and its DVT process control [dv-35][dv-38].
  - **debugserver:** reached through the RSD service `com.apple.internal.dt.remote.debugproxy`. `developer debugserver start-server --local-port` forwards it to localhost and works over the userspace tunnel [dv-10].
  - **LLDB:** upstream LLDB builds `remote-ios` on every host [dv-11][dv-12]:
    - Arch `lldb` 23.1.1, with lldb-dap;
    - CodeLLDB 1.12.3 (LLDB 22.1.8, Windows x64);
    - the LLVM 23.1.3 Windows MSI (lldb, lldb-dap, Python 3.14).
  - **IDE:** VS Code can drive this through lldb-dap `attachCommands` or CodeLLDB `processCreateCommands` [dv-18].
- **iOS 26/27.**
  - XNU only honours debug code mappings that a debugger creates.
  - On devices with Apple's TXM monitor (A15 and later on iOS 26, nearly all devices on iOS 27), the debugger must also touch every executable page. StikDebug, Frida and Flutter all do this [dv-19][dv-20]. Frida's change is commit 03d861ca80, "fruity: Support iOS 26 through dual-mapping", 2025-07-03 [fc-dv, re-checked]. *Verified 2026-10-11 from firmware (verified-facts.md): iOS 26.6.2 has TXM on chips 0x8110 and later; iOS 27.0 has it on every supported iPhone and iPad except the A12Z iPad Pro (0x8027), including the test iPhone 12 (0x8101).*
  - LLDB's stops at scripted breakpoints are unreliable on iOS 27; Flutter continues manually [fx-21].
  - iOS 26 redacts NSLog's dynamic strings to `<private>` in the unified log, so capture stdout instead [dv-35].
- **Gaps.**
  - No packaged flow exists [dv-41][dv-28][dv-02].
  - Xcode's `device select` and `device process attach` live in a closed LLDB plug-in [dv-13][xr-37].
  - On non-Apple hosts, LLDB cannot resolve `.app` bundles and reports "unable to locate SDK" [dv-14].
  - System symbols: LLDB cannot load an on-disk dyld cache off macOS, and `ipsw dyld split` is macOS-only, so libraries are read slowly from process memory [dv-15][dv-16].
  - Swift `po` is unverified [dv-17].
  - pymobiledevice3's `debugserver lldb` helper needs a kernel tunnel and a Unix pty, so it does not work on Windows [dv-10]. Since commit d63f1aaa (2026-06-22) it also refuses userspace tunnels [fc-fx, re-checked].
- **Action.**
  - *Build* `nomac debug`, the highest-leverage gap.
  - *Contribute* to LLDB [dv-14][dv-16]:
    - `.app` resolution: `Host::ResolveExecutableInBundle` returns false on non-Apple hosts;
    - on-disk shared-cache loading: `HostInfoBase::GetSharedCacheImageInfo` returns `{}`.
  - *Reverse-engineer* Xcode's attach sequence (target P1-6).

### 2.10 Test

- **Best.**
  - XCUITest and WebDriverAgent runners can be launched off-Mac by any of [dv-37]:
    - pymobiledevice3 (`developer dvt xcuitest`);
    - go-ios (`runtest`, `runwda`, `runxctest`);
    - idevice;
    - Appium's XCUITest driver in non-macOS mode (real devices on iOS 18+, with a preinstalled WDA).
  - xtool's SDK includes the XCTest and Testing frameworks [tc-40].
- **iOS 26/27.**
  - Some users report runner aborts on iOS 26.4.2; the maintainer could not reproduce them [dv-37].
  - Maestro stopped listing physical iPhones in CLI 2.11.0 [fc-dv].
- **Gaps.** Nothing packages "build the test bundle and runner off-Mac", and there is no `.xcresult` output.
- **Action.**
  - *Build* `nomac test`: build test bundles with the xtool SDK, run them through pymobiledevice3 or go-ios, and emit JUnit.
  - *Reverse-engineer* testmanagerd only when it breaks.

### 2.11 Profile

- **Best.** pymobiledevice3's DVT services: sysmon, energy, network, fps, allocations, memgraph and core-profile [dv-38][xr-22].
- **Gaps.**
  - `.trace` files are undocumented; only `xctrace export` on macOS reads them.
  - There is no view-hierarchy or GPU tooling [xr-22][dv-39].
  - Xcode 27 Instruments itself needs iOS 17+ devices [xr-05].
- **Action.** *Later*: export DVT streams to Perfetto or pprof. Compatibility with Instruments is not a goal.

### 2.12 Distribute

- **Best.**
  - App Store Connect API 4.1+ build uploads work from any OS: `/v1/buildUploads`, `/v1/buildUploadFiles` and presigned parts. Uploading only the IPA as `ASSET` worked in practice [sg-25][sg-26].
  - The MIT `asc` CLI has `asc builds upload --ipa` [sg-37].
- **Rules.**
  - Since 2026-04-28, uploads must be built with Xcode 26+ and an iOS 26 SDK.
  - Since 2026-09-09, apps must target iOS 13+.
  - An Xcode 27 / iOS 27 SDK requirement is likely around late April 2027. It is not announced; the estimate follows Apple's yearly pattern [sg-29][xr-04].
- **Evidence.** omarchy-apple-dev reports three Linux-built apps processed as VALID and APP_STORE_ELIGIBLE: a demo app, NetNewsWire with extensions, and a Flutter app built without Xcode. This is a single, self-reported source; no TestFlight tester has installed these builds, and nothing was submitted for review [sg-32][tc-08].
- **Alternatives.**
  - Transporter 4.2 runs on Windows 11 and RHEL, but it needs the Xcode-generated `AppStoreInfo.plist`, which only the macOS `swinfo` tool produces [sg-27][sg-28].
  - ios-uploader synthesizes that plist itself and uses private endpoints [sg-28].
  - For licence-clean final builds on Apple hardware [sg-35][fc-sg]:
    - Xcode Cloud, with 25 compute hours per month included;
    - GitHub macOS runners: `macos-26` with Xcode 26.6 is generally available, and an Xcode 27 image is in preview.
- **Action.**
  - *Build* `nomac ship` on the ASC API, with lints.
  - *Offer* an "Apple-hardware release" mode.

### 2.13 iOS 26/27 and Xcode 26/27 breakage watchlist

| Change | Effect on off-Mac tooling | Handling | Source |
|---|---|---|---|
| Xcode 27 is arm64-only and needs macOS 26.6+ | No x86 macOS can run Xcode 27 tools. SDK extraction is unaffected | x86 fallbacks are capped at Xcode 26.6 | [xr-01][xr-02] |
| Xcode 27 SDK `.tbd` files list `arm64e.x1` | lld 23.1.2 and older cannot link | LLVM 23.1.3 or xtool's lld | [tc-05] |
| Xcode 27 works only with Swift 6.4 | The toolchain must match | Version-lock manager | [tc-03] |
| Classic ld64 removed in Xcode 27 | Only lld, or Apple's closed linker | lld | [tc-09] |
| iOS 27 SDK makes UIScene mandatory | Storyboard-free runners must still be scene-based | Templates | [fx-31][xt-12] |
| Xcode 27 ibtool `toolchain` mode | IB compiles on the Mac without a simulator | Better oracle | [xt-16] |
| devicectl JSON v5 deprecations | Flutter still reads the deprecated fields | Keep emitting them if we ever shim devicectl | [xr-13][fc-xr] |
| Xcode 27 debugs and profiles only iOS 17+ | Matches the RemoteXPC-only stack | None needed | [xr-05] |
| Xcode 27.2 beta: JSON `project.xcproj` | New project format; CocoaPods cannot read it yet | apple/xcode-project-format | [tc-20] |
| iOS 26 AMFI `MAIN_BINARY` rule for app extensions | Extensions signed by old zsign die | rcodesign, or zsign 1.0.5+ | [sg-07] |
| iOS 26 debugger-only JIT and TXM page touching | Flutter debug needs the RX helper or runs interpreted | M2 | [dv-19][fx-18] |
| iOS 26 NSLog redaction | syslog loses app log text | Capture stdout | [dv-35] |
| iOS 26 streaming app list | Old CoreDevice clients wedge `dtappserviced` | Current pymobiledevice3 | [dv-34] |
| iOS 26 pairing timing vs usbmuxd 1.1.1 | First pairing fails on Linux | `--no-preflight` | [dv-40] |
| iOS 27 LINKEDIT string-pool alignment | Rewritten Mach-O files are rejected | LLVM 23+ tools; lint | [sg-08] |
| iOS 27 Cryptex DDI; iPhone 18 is Cryptex-only | Old DDI mounters fail | pymobiledevice3; go-ios main | [dv-07][dv-09] |
| iOS 27 auto-continue unreliable at the JIT breakpoint | Flutter debug crashes | Manual continue | [fx-21] |
| iOS 27.2 RemoteXPC message-id parity | Older clients break | Track pymobiledevice3 | [dv-34] |
| iOS 27 device-initiated Wi-Fi pairing | New option; re-pair needed after OS upgrades | pair-host | [dv-30] |
| SwiftPM 6.4 defaults to Swift Build | Resource tools get invoked, so builds with `.xcassets` fail | `nomac-actool` | [tc-16][xt-21] |
| GSA rejects the Xcode client identity (2026-09-10) | Free-account login broke in every tool | akd identity; paid path as default | [sg-14] |
| App Store requires the Xcode 26 SDK since 2026-04-28 | x86 Xcode 26.6 remains usable until the next bump | Plan for about April 2027 | [sg-29][xr-04] |
| CocoaPods trunk read-only from 2026-12-02 | Pod-only plugins stagnate | SwiftPM first | [tc-36] |


---

## 3. Proposed architecture of the kit

### 3.1 Principles

1. **No Apple files are ever shipped.** The user supplies Xcode (`.xip` or `Xcode.app`) and everything derived from it: the SDK and the DDIs. This is the same policy your Game Pass project uses for Microsoft binaries.
2. **Compose, don't fork.** Upstream tools are used as subprocesses or libraries, and fixes go upstream. Fork only when blocked.
3. **Version-locked toolsets.** One manifest pins the Xcode build, Swift, LLVM/lld, the Flutter engine and `gen_snapshot`.
4. **No root by default.** pymobiledevice3's userspace tunnel plus localhost port forwards. Kernel tunnels are optional.
5. **One CLI on Linux and Windows.** WSL2 is only a stopgap for the build plane.
6. **The Mac is an optional oracle, never a runtime dependency.**
7. **Clean-room work on closed tools.** Oracle outputs, public docs and Apple's open source are turned into a written spec; the implementation is written from the spec (section 6.1).

### 3.2 Layers

```text
IDE        VS Code extension (TypeScript): tasks, debug configs (lldb-dap or CodeLLDB),
           SourceKit-LSP via xtool's BSP server; Dart-Code sees the iPhone as a Flutter custom device
             |
CLI        nomac (Python 3.13, GPL-3.0-or-later) + nomac-deviced (keeps the userspace tunnel and port forwards)
           doctor | sdk | build | sign | account | device | run | debug | logs | flutter | test | profile | ship | oracle
             |
BUILD      xtool 1.21 (SDK import, SwiftPM / Swift Build, packing, BSP) + swift.org Swift 6.4 + OpenAppleMacros
           ld64.lld (xtool toolset or LLVM >= 23.1.3), llvm-dsymutil/strip/lipo, plistutil
           nomac-actool | storyboard->UILaunchScreen | xcstrings   (ours, plugged in via ASSETCATALOG_EXEC / IBC_EXEC)
           Flutter adapter: frontend_server, stub App.framework, UIScene Runner, gen_snapshot built from Dart in our CI
SIGN       nomac sign = rcodesign (MPL-2.0) + profile/entitlement handling + Mach-O lint
           accounts: App Store Connect API (paid, official) | xtool XKit password mode (free, opt-in)
DEVICE     pymobiledevice3 (GPL-3.0): usbmux/lockdown, userspace RSD tunnel, DDI from the user's Xcode,
           install, CoreDevice launch/console, debugproxy forward, DVT, xcuitest     [swappable: idevice, go-ios]
DEBUG      LLDB (swift.org for Swift; llvm.org >= 23.1 for Flutter JIT scripts) over gdb-remote -> debugproxy;
           RX-page JIT helper; symbol cache (fetch-symbols + ipsw dyld extract)
SHIP       packaging lints, optional and visible metadata stamping, ASC build upload
           -- or the "Apple-hardware release" mode (GitHub macOS runner / Xcode Cloud)
ORACLE     dev/CI only: SSH to your Mac or a macOS VM on it -> xcrun actool / ibtool / xcstringstool /
           xcodebuild -dumpPIF, assetutil -> golden corpus (never used by end-user builds)
```

**Why the orchestrator is in Python:**
- The most complete device stack, pymobiledevice3, is a GPL-3.0 Python library [dv-06].
- LLDB automation, including Flutter's JIT helper, is LLDB Python [fx-20].
- xlinux, the closest existing implementation, is GPL-3.0 Python [fx-24].

The trade-off is that Python gives weaker single-binary distribution on Windows. To keep that open, the device layer sits behind an interface, so idevice (Rust, MIT) can replace pymobiledevice3 later [dv-29].

### 3.3 Modules

| Module | Responsibility | Built on | We build |
|---|---|---|---|
| `nomac doctor` | Host, toolchain and device checks; version matrix | Package managers, `strings` on binaries | Yes |
| `nomac sdk` | Import the user's Xcode, version lock, toolset pins | xtool `sdk`, swiftly / swift.org tarballs | Wrapper |
| `nomac build` | Project adapters: SwiftPM via xtool (now), Flutter (M2), `.xcodeproj` and React Native (M7) | xtool, Swift Build, flutter_tools | Adapters |
| Resource CLIs | `nomac-actool`, storyboard to `UILaunchScreen` conversion, launch-storyboard subset, xcstrings | AssetKit, nibarchive, algoriddim/xcstringstool | Yes (MIT, so they can go upstream) |
| `nomac sign` | Profile embedding, per-bundle entitlements, inside-out signing, Mach-O lint | rcodesign / zsign | Wrapper + lint |
| `nomac account` | Paid: ASC API keys, certificates, profiles. Free: opt-in XKit | ASC API, xtool XKit | Thin client |
| `nomac-deviced` | Pairing, Developer Mode, DDI, userspace tunnel, port forwards, install, launch, console, logs | pymobiledevice3 | Daemon + DDI import |
| `nomac debug` | Launch suspended, debugproxy forward, LLDB / lldb-dap config, symbol cache, JIT helper | LLDB, pymobiledevice3, ipsw | Yes |
| `nomac flutter` | Custom device, Runner template, VM-service forwarding, AOT pipeline | flutter_tools, Dart SDK | Yes |
| `nomac test` | Build and run XCTest/XCUITest bundles; JUnit output | pymobiledevice3, go-ios | Yes |
| `nomac ship` | Lints, optional stamping, ASC upload, or Apple-hardware CI | ASC API, `asc` CLI | Yes |
| `nomac oracle` | Run closed tools on your Mac over SSH and store golden outputs | SSH, xcrun | Yes (development only) |
| VS Code extension | Tasks, debug configurations, BSP wiring | lldb-dap / CodeLLDB, xtool BSP, Dart-Code | Yes |

### 3.4 Reuse, contribute, build, reverse-engineer

- **Reuse:**
  - xtool; swift.org Swift + Swift Build;
  - LLVM: clang, lld, lldb, lldb-dap and binutils;
  - OpenAppleMacros; AssetKit;
  - rcodesign/zsign;
  - pymobiledevice3; netmuxd for Wi-Fi; ipsw for DSC handling, stubs and inspection;
  - tuist/XcodeProj and apple/xcode-project-format;
  - flutter_tools and the Dart SDK sources;
  - the App Store Connect API and the `asc` CLI.
- **Contribute upstream:**
  - xtool: #44 (launch), #144 (xcodeproj), #219/#310 (assets), #131 (per-bundle entitlements); help land PRs #290–#293 and #302.
  - AssetKit: a CLI, single-size icons, appearances.
  - OpenAppleMacros: SwiftData, AppIntents, TipKit.
  - LLDB: `.app` resolution and on-disk DSC loading on non-Apple hosts.
  - pymobiledevice3: mounting a DDI from a local Xcode.
  - Flutter: #56511; publish Linux and Windows iOS `gen_snapshot`; custom devices declaring iOS.
  - rcodesign: IPA support (#30).
  - go-ios: debugproxy (#614).
- **Build ourselves:**
  - the orchestrator and device daemon;
  - the debug integration;
  - the Flutter adapter;
  - resource-compiler CLIs;
  - the signing wrapper and lints;
  - `ship` and `test`;
  - the `.xcodeproj` to PIF generator, unless xtool #144 lands first;
  - the VS Code extension;
  - the Mac oracle harness;
  - an on-device harness app that loads every asset and nib.
- **Reverse-engineer (black-box first; see section 6):**
  - actool, ibtool and xcstringstool outputs;
  - `xcodebuild -dumpPIF`;
  - the differences between Xcode-built and Linux-built IPAs that App Store processing cares about;
  - the DDI layout inside Xcode 27;
  - CoreDevice and debugproxy traffic from Xcode;
  - Apple macro expansions;
  - App Intents metadata;
  - later: the Previews runtime and Instruments traces.

### 3.5 VS Code integration

- **Build and run.** A task provider calls `nomac build`, `nomac run` and `nomac logs`. Logs come from stdout via CoreDevice `--console`, so iOS 26's NSLog redaction does not apply [dv-35].
- **Debug.** A `preLaunchTask` runs `nomac debug prepare`, which:
  1. installs the app;
  2. launches it suspended;
  3. forwards debugproxy to a localhost port;
  4. writes `.nomac/attach.lldb`.

  The debug configuration then attaches with lldb-dap `attachCommands` or CodeLLDB `processCreateCommands` [dv-18]. Sketch, to be validated in M1/M3:

  ```jsonc
  {
    "type": "lldb-dap",
    "request": "attach",
    "name": "iPhone (nomac)",
    "preLaunchTask": "nomac: debug prepare",
    "attachCommands": ["command source ${workspaceFolder}/.nomac/attach.lldb"]
  }
  ```

  `attach.lldb` contains the pymobiledevice3 sequence: `platform select remote-ios`, `target create <executable>`, `SetPlatformFileSpec`, `process connect connect://127.0.0.1:<port>`, `process attach --pid <pid>`.
- **Code intelligence.** `xtool dev build-server` writes `.bsp/xtool.json` for SourceKit-LSP; it needs Swift 6.4 [tc-12].
- **Flutter.** Dart-Code lists the custom device, so the normal Flutter debug UI (hot reload, DevTools) works unchanged [fx-24][fx-27].
- **Windows.** The same extension works with lldb-dap from the LLVM MSI, or with CodeLLDB (x64 only) [dv-12][fc-dv].

### 3.6 Licensing

**Policy:**
- The kit is **GPL-3.0-or-later**, matching your other projects.
- Pieces meant for upstream (the actool CLI, macros, xtool patches) are written under the upstream licence (MIT), in separate repositories, so the upstream projects can accept them.
- Apple files are never redistributed.

| Dependency | Licence | How we use it | GPL-3.0 note |
|---|---|---|---|
| xtool, darwin-tools scripts, OpenAppleMacros, AssetKit, go-ios, idevice, zsign, ipsw, fastlane, `asc` CLI, ios-uploader, omarchy-apple-dev, iOSBuildX, tuist/XcodeProj, algoriddim/xcstringstool, Impactor, iloader | MIT | Subprocess, library, selective code reuse, upstream PRs | Compatible: the Expat licence is "a lax, permissive non-copyleft free software license, compatible with the GNU GPL" ([GNU license list](https://www.gnu.org/licenses/license-list.html)) |
| swift.org toolchain, Swift Build, SwiftPM, apple/xcode-project-format, nibarchive (Rust), hatch | Apache-2.0 (plus runtime/Swift exceptions) | External tools, libraries | Apache-2.0 is "compatible with version 3 of the GNU GPL" (GNU list) |
| LLVM: clang, lld, lldb, lldb-dap | Apache-2.0 WITH LLVM-exception | External tools | As above |
| pymobiledevice3, pmd-pytcp, xlinux, MatrixEditor/nibarchive, Darling, Sideloader | GPL-3.0 | Library; code reuse (xlinux) | Same licence |
| rcodesign | MPL-2.0 | Subprocess | MPL-2.0 §3.3 allows combining it into a "Larger Work" with GPL code |
| libimobiledevice, netmuxd, xadi, apple-codesign-quick | LGPL-2.1(+) | Library or subprocess | LGPLv2.1 "is compatible with GPLv2 and GPLv3" (GNU list) |
| unxip | LGPL-3.0 | Used through xtool | Compatible |
| usbmuxd | GPL (the README says GPL-3.0; GitHub detects GPL-2.0) | System daemon | Separate process |
| LibreDarwin/IDETools | BSD-3-Clause | Reference; possible reuse | Compatible ("Modified BSD license … compatible with the GNU GPL") |
| xcross v2.0.0+, StikDebug, AltStore, SideStore | AGPL-3.0 | Reference only, or a separate process | GPLv3 §13 allows combining with AGPLv3 code, but "the special requirements of the GNU Affero General Public License, section 13, concerning interaction through a network will apply to the combination as such". xcross up to v1.5.1 was MIT [fx-25] |
| Frida | wxWindows Library Licence | Reference only | Not evaluated for code reuse |
| xcross_gen_snapshot, viraptor/dumppif, iineva/bom, unxip-rs, davidquesada/ibtool | No licence | Read only; ask the authors | No licence means no permission to copy. Rebuild from the BSD-licensed Dart sources instead [tc-34] |
| hatch-ios repository | No LICENSE file; `Cargo.toml` says MIT OR Apache-2.0 | Reference | Confirm with the author before reusing code |
| StikPair | MIT, non-commercial | Avoid | Not free software |
| Xcode `.xip`, iOS SDK, DDIs, Transporter, Metal tools for Windows, the Apple Music APK's ADI libraries, IPSWs | Apple proprietary | Never shipped; user-supplied or user-fetched | EA2002 §2.5 and §2.7 [tc-31][fx-47] |

---

## 4. Flutter path in detail

### 4.1 Where official Flutter blocks off-Mac

- **The CLI refuses iOS off macOS.** `flutter build ios` checks `supported => isMacOS` and stops with "Building for iOS is only supported on macOS". Device discovery and control are macOS-only and go through `xcdevice`, `devicectl`, Xcode's LLDB and `xcodebuild` [fx-01][fx-02][fx-15].
- **The iOS AOT compiler is macOS-only.** `gen_snapshot_arm64` is published only as a macOS universal Mach-O, whereas Android's compiler is published for every host [fx-05]. Dart's `tools/gn.py` refuses an iOS target on non-mac hosts. That is a configuration gate, not a code barrier [fx-06].
- **Everything else is host-neutral or replaceable:**
  - The engine zips download on any host [fx-04].
  - Debug builds contain no AOT code [fx-12][fx-13].
  - In release builds, `gen_snapshot` writes `App.framework/App` directly (`app-aot-macho-dylib`, since Flutter 3.44). The only steps after it are `dsymutil`, `strip`, `lipo`, `plutil` and `codesign`, and each has an open equivalent [fx-10].

### 4.2 Debug and hot-reload pipeline (M2)

1. **Artifacts.**
   - `flutter precache --ios` fetches Flutter.xcframework (debug, profile, release) on Linux.
   - The linux-x64 host artifacts supply `frontend_server`, the JIT snapshots and `impellerc` [fx-04][fx-13].
2. **Kernel and assets.**
   - `flutter assemble` with the defines that `xcode_backend` passes, or `frontend_server` directly, produces `kernel_blob.bin`, `vm_snapshot_data` and `isolate_snapshot_data` in `App.framework/flutter_assets`.
   - Debug kernels are platform-neutral.
   - The Linux host snapshot differs from the macOS one in a single NaN sign bit, which is semantically the same [fx-12][fx-13].
3. **`App.framework`.** A clang stub dylib plus Info.plist and `flutter_assets` [fx-12][fx-49]:
   - source: `static const int Moo = 88;`
   - `-target arm64-apple-ios15.0`
   - install name `@rpath/App.framework/App`
4. **Runner.**
   - An ObjC or Swift UIScene delegate (`FlutterSceneDelegate`) creates `FlutterViewController` in code. There is no `UISceneStoryboardFile` or `UIMainStoryboardFile`, and `GeneratedPluginRegistrant` registers the plugins [fx-31][xt-13].
   - Info.plist needs [fx-14][dv-26]:
     - `UIApplicationSceneManifest`
     - `UILaunchScreen`
     - `NSBonjourServices` (`_dartVmService._tcp`)
     - `NSLocalNetworkUsageDescription`
     - the `UIDeviceFamily`/`DT*` group
5. **Plugins.** SwiftPM has been the default since Flutter 3.44 [fx-33]. Build plugins against the xtool SDK; xcross links them into a single `libFlutterPluginsGenerated.dylib` [fx-25]. For CocoaPods-only plugins, see 4.4.
6. **Link and sign.**
   - Link with ld64.lld from the xtool toolset or LLVM 23.1.3+, because the Xcode 27 SDK needs `arm64e.x1` [fx-32].
   - Sign every Mach-O with `get-task-allow` and a development profile that lists the device.
7. **Install and launch.**
   - Install with pymobiledevice3 `apps install`.
   - Launch suspended with `--enable-dart-profiling --enable-checked-mode --verify-entry-points --vm-service-port=N`. Debug engines refuse to start unless they see `--enable-checked-mode` or a debugger [fx-16].
8. **JIT.** See 4.3.
9. **VM service.** Find the URL in syslog or through mDNS, and forward it with `pymobiledevice3 usbmux forward` or `iproxy` [fx-15][dv-24].
10. **Hot reload and IDE.** Register the phone as a Flutter custom device [fx-27]:
    - Custom devices only accept `linux-x64`/`linux-arm64`, so it is declared as `linux-x64`.
    - Its `install` and `runDebug` commands call steps 6–9.
    - flutter_tools then drives DevFS, hot reload, hot restart and DevTools.

    This works because debug kernels are platform-neutral and plugin registration checks `Platform.isIOS` at runtime. Alternatives: `flutter attach --debug-url` against that device, or a DevFS reimplementation as in xcross [fx-25].

### 4.3 JIT by iOS version

| Device iOS | What the debug engine needs | Off-Mac option |
|---|---|---|
| 17.x–18.x | A debugger attached for JIT; there is no interpreter fallback [fx-19] | Any gdb-remote client that attaches and continues. xcross's minimal client was validated on iOS 18.7.8 [fx-26] |
| 26.x | The VM dual-maps code pages and calls `NOTIFY_DEBUGGER_ABOUT_RX_PAGES`. A debugger must write into each new RX page. Otherwise `CheckIfRXWorks` fails and the engine runs in Dart's arm64 simulator, i.e. interpreted ("Dart execution mode: simulator") [fx-18][fx-19][dv-22] | LLDB plus Flutter's Python helper. The helper is plain LLDB Python. It should run on llvm.org LLDB 23.1+, which has the scripted auto-continue fix, or in synchronous mode [fx-20] |
| 27.x | Same as 26.x, but with auto-continue LLDB sometimes misses the stop at the JIT breakpoint, so Flutter continues manually [fx-21] | LLDB, the helper and manual continue. xlinux claims real JIT on an iPhone 11 running the iOS 27.2 beta [fx-24] |

- **Cost of a JIT stop:** about 130–150 ms over USB [fx-22].
- **Cheaper writes:** xlinux cut its first hot reload from 6.8 s to about 0.5 s by writing 8 bytes per 16 KiB page instead of whole regions (README claim) [fx-22].
- **A third option (no LLDB or Python):** a native gdb-remote client. This is how StikDebug and Frida serve iOS 26 [fx-19][dv-20][fc-dv]:
  1. Set a `Z0` breakpoint on the exported `_NOTIFY_DEBUGGER_ABOUT_RX_PAGES`. In the 3.47.7 debug engine it is `T 0x744174`.
  2. Read `x0` and `x1` at the stop.
  3. Write one `M` packet per page.
  4. Continue.
- **Regressions so far:** Xcode 26.4 (#184254) and iOS 27 (#190307), both fixed. Add2App on iOS 26 still fails to intercept the hook (#170416, open) [dv-21]. We need a regression test for every Flutter release.

### 4.4 Plugins, CocoaPods, launch screens

- **SwiftPM first.** Flutter warns that disabling SwiftPM "won't be allowed in the future" [tc-36].
- **CocoaPods-only plugins.** Existing approaches:
  - xcross skips them with a warning [fx-25].
  - xlinux converts podspecs to SwiftPM, or runs real CocoaPods [fx-24].
  - iOSBuildX runs the CocoaPods installer with `integrate_targets: false` and its own pod compiler (15 plugins / 43 pods in about 151 s) [fx-28].

  We start with "convert, else fail clearly", and add a PIF-based Pods build in M7.
- **Launch screens.**
  - The default template uses `Main.storyboard` and `LaunchScreen.storyboard` (one centred `LaunchImage`) [xt-13]. Both are replaced by the programmatic scene and `UILaunchScreen` [xt-12].
  - `flutter_native_splash` rewrites the storyboard with extra views [xt-14]. Its output can be converted to `UILaunchScreen` `UIColorName`/`UIImageName`. Those keys reference asset-catalog entries, so this needs `nomac-actool` (M4) [xt-12].
- **The template's asset catalog fits AssetKit's supported subset:** AppIcon has 19 per-size entries including `ios-marketing` 1024, and `LaunchImage` has 1x/2x/3x [xt-13].

### 4.5 Release and profile (AOT) builds

1. **Kernel.** `frontend_server --aot --tfa --target-os ios -Ddart.vm.product=true` against `flutter_patched_sdk_product`, plus the plugin registrant. `--target-os` is passed only for AOT compiles [fx-13]; the command line comes from the Flutter gap analysis in the research.
2. **An iOS-target `gen_snapshot` for the host.** In order of preference:
   - **(a) Build it ourselves in CI** from the Dart SDK at the engine's `dart_revision` (read from Flutter's `DEPS`). Licence-clean, because Dart is BSD-3 [fx-08]:
     1. Patch `runtime/BUILD.gn` so that `dart_os_config` emits the iOS defines in a Linux host build.
     2. Run `tools/gn.py --mode product --arch simarm64` with `dart_use_compressed_pointers=false`.
     3. Run `ninja gen_snapshot`. This takes about 20 min and 8 GB and builds product mode only; profile needs a second build.

     For byte-identical output, also reimplement the documented determinism fixes: an Apple libc qsort/heapsort port, timeline support, and MSVC argument order [fx-09].
   - **(b) arxdeus/xcross_gen_snapshot prebuilt zips** for Linux and Windows. They cover Flutter 3.47.0–3.47.6 (no 3.47.7 yet) and are CI-verified byte-identical, but the repository has **no licence**. Use them for personal experiments only [fx-09].
   - **(c) Flutter's macOS `gen_snapshot` under Darling** with xlinux's shim. Heavy, and x86_64 only [fx-24].
3. **Compile.** `gen_snapshot --deterministic --snapshot_kind=app-aot-macho-dylib --macho=App.framework/App --macho-object=… --macho-min-os-version=… --macho-rpath=… --macho-install-name=@rpath/App.framework/App` [fx-10].
4. **Post-process** [fx-10]:
   - `llvm-dsymutil`
   - `llvm-strip -x`
   - `llvm-lipo -create`
   - `plistutil` for `MinimumOSVersion`
   - rcodesign on `App.framework` and `Flutter.framework`
5. **SDK version stamp.** Dart 3.13.5 (Flutter 3.47.x) writes `LC_BUILD_VERSION` sdk = minos. `--macho-sdk-version` exists only on Dart main, and the flutter_tools change (PR #191369) is still open. Patch the field afterwards, visibly [fx-11][fc-fx].
6. **Distribution** [fx-30][sg-21][sg-25]:
   - a compiled `Assets.car` containing AppIcon (ITMS-90713);
   - a distribution certificate and an App Store profile;
   - upload through the ASC API.

**Version coupling.** `gen_snapshot` must match the engine's Dart revision; stale artifacts give "Invalid SDK hash" and a black screen [fx-51]. Flutter 3.47.6 and 3.47.7 share Dart `04bcd10` [fc-fx].

### 4.6 Which base: xlinux, xcross, or our own

| | xlinux | xcross | Kit (proposal) |
|---|---|---|---|
| Licence | GPL-3.0 | AGPL-3.0 from v2.0.0 (MIT up to v1.5.1) | GPL-3.0-or-later |
| Host | Linux | Linux and Windows (native) | Linux first, Windows in M6 |
| Debug JIT on iOS 26/27 | Yes (upstream LLDB + helper) | Probably interpreted (no RX hook) | Yes (helper; later a native gdb-remote client) |
| Release AOT | Darling + macOS `gen_snapshot` | Open PR #106, using xcross_gen_snapshot | `gen_snapshot` built in our CI |
| CocoaPods-only plugins | Converted, or real CocoaPods | Skipped | Converted; PIF later |
| Maturity | v0.5.1, created 2026-09-28, 2 stars | v2.0.1, 64 stars, green Windows CI | n/a |
| Sources | [fx-24] | [fx-25][fx-26][tc-29] | |

**Recommendation.**
1. Start M2 by running and auditing xlinux. It is in the same licence family, so its code can be reused directly.
2. Propose upstream collaboration before forking.
3. Study xcross's Windows techniques, and reuse only its MIT-era code.

### 4.7 Upstream asks and the long-term fix

- Publish linux-x64 and windows-x64 iOS `gen_snapshot` zips, as Flutter already does for Android [fx-05].
- Revive #56511: an optional external iOS device backend that leaves the macOS path untouched [fx-03].
- Let custom devices declare an `ios` platform [fx-27].
- Land PR #191369 (`--macho-sdk-version`) [fx-11].
- Watch dart-lang/sdk#61635, "Development modular AOT compiler targeting iOS/arm64" (open, updated 2026-09-28, re-checked). If it lands, debug builds stop needing JIT, and the RX-page/LLDB workaround goes away [fc-fx].

### 4.8 React Native and Expo

- **JavaScript side: host-neutral.** `hermes-compiler` ships `hermesc` for linux64, osx and win64, and Hermes bytecode needs no JIT on the device [fx-38].
- **Native side: needs Xcode or cloud Macs.**
  - EAS builds on macOS VMs on Mac minis. The free tier includes 15 iOS builds per month [fx-38].
  - Expo Go on a physical iPhone must now be self-built and installed through TestFlight, which needs a paid account. Since Expo Go 57 it also requires the same Expo login as the CLI [fx-39].
- **Off-Mac evidence so far.**
  - xlinux claims Expo / RN 0.85 debug and release builds on Linux [fx-24].
  - omarchy built and signed an RN 0.87.1 Hermes release app on Linux but never ran it on a device [tc-43].
  - The RN template ships `LaunchScreen.storyboard`, `Images.xcassets` and `SceneDelegate.swift` [xt-14].
- **Plan.** RN support arrives with `.xcodeproj` and CocoaPods support (M7). A cheaper interim is a prebuilt "dev client" shell with JavaScript served over Metro.

### 4.9 .NET MAUI

- **Hot Restart** was the only vendor-supported way to deploy to an iPhone without a Mac [fx-35][fx-36]:
  - How it worked: Microsoft built a host app with the Mono interpreter on a Mac; Windows re-signed it and pushed IL alongside.
  - Limits: debug only; no storyboards, XIBs, static libraries or asset catalogs.
  - Status: removed from the .NET 10 iOS SDK and not supported in Visual Studio 2026.
- **Otherwise**, .NET iOS builds need Pair to Mac [fx-37].
- **Lesson:** a prebuilt signed shell with an interpreted payload deploys fast, but cannot ship real apps.
- **Scope:** out of scope until there is demand.

### 4.10 Kotlin/Compose, Unity, Godot, Qt

- **Kotlin/Native** still officially needs a Mac for cinterop, CocoaPods and final Apple binaries (2.2.20). xcross works around this by patching the compiler JAR [fx-40].
- **Unity, Godot and Qt** all hand off to Xcode projects [fx-41][fx-42][fx-43]. Godot leaves the Xcode project behind on non-Mac hosts. All three become reachable once M7's `.xcodeproj` builds work.

---

## 5. Windows path

### 5.1 Options per layer

| Layer | WSL2 route | Native route | Evidence |
|---|---|---|---|
| USB transport | usbipd-win binds the iPhone into WSL (xtool docs), or a relay to the Windows usbmuxd port (xtool #19 comment) | Apple Mobile Device Service at `127.0.0.1:27015` | [tc-10][dv-32] |
| Tunnel (iOS 17+) | pymobiledevice3 inside WSL; WSL2 needs mirrored networking | pymobiledevice3 userspace tunnel, no Admin; its relays use loopback TCP. Kernel tunnels need Admin and wintun (bundled with pytun-pmd3) | [dv-05][dv-32] |
| Build | xtool AppImage (xtool's documented Windows path) | Swift 6.4 for Windows, stock LLVM 23.1.3 and our own SDK extraction. Workarounds needed: `MAX_PATH` (260), the 32767-character command-line limit, duplicated `\\?\` prefixes, symlinks, case-insensitive paths, and forcing `--build-system native`. xcross does all of this, with green CI on windows-2022 and windows-11-arm | [tc-29] |
| Signing | rcodesign / zsign inside WSL | rcodesign Windows builds (Windows certificate store); zsign windows-x64 | [sg-02][sg-39] |
| Free-account anisette | xadi inside WSL | iCloud for Windows DLLs (AltServer), remote anisette servers, or Impactor's omnisette | [sg-13][sg-39] |
| Debugger | Linux LLDB connecting to a forwarded port | LLVM 23.1.3 MSI (lldb, lldb-dap, Python 3.14); CodeLLDB 1.12.3 (LLDB 22.1.8, x64 only) | [dv-11][dv-12][fc-dv] |
| Flutter | The Linux pipeline, unchanged | xcross natively today (debug only); for AOT, a Windows-host `gen_snapshot` built in our CI | [fx-25][fx-09] |
| Upload | ASC API | ASC API; Transporter 4.2 on Windows 11 (needs `AppStoreInfo.plist`) | [sg-25][sg-27] |

### 5.2 USB and usbmux drivers

- **Classic iTunes installer.** Provides the Apple Mobile Device Service and is pymobiledevice3's recommended choice.
- **Microsoft Store "Apple Devices" app.** Easier to install, but its service does **not** discover Wi-Fi devices.
- **netmuxd** (LGPL-2.1, v0.4.3). Either replaces AMDS (`--kill-amds`, `install-service`), or runs on libusb0 via libwdi with no Apple software at all. On Windows on ARM64, libwdi's self-signed CA is rejected, so test signing is needed.

Sources: [dv-32]. The kit's `doctor` should detect which of these is active.

### 5.3 Tunnels

- **pymobiledevice3:** for iOS 17.4+, its in-process tunnel needs neither Admin rights nor wintun. The kernel tunnel is needed only for iOS 17.0–17.3.1, or for external tools that must reach the device's tunnel address [dv-04][dv-05].
- **go-ios:** offers `tunnel start --userspace` (gVisor), but its README still documents an Admin shell with `wintun.dll` copied into system32 [dv-27].
- **xcross:** its documented default runs `sudo pymobiledevice3 lockdown start-tunnel` (Administrator on Windows) and falls back to userspace only on a privilege error [dv-24].

**We default to the userspace tunnel.**

### 5.4 Recommendation and test setup

- **W1 (M6a): "split" mode.** The least new code to get the M1 and M2 demos onto Windows.
  - The device daemon, tunnel and LLDB run natively on Windows.
  - The build runs in WSL2 on the unchanged Linux build plane.
  - Artifacts move through the shared filesystem; debug ports go through localhost (mirrored networking).
- **W2 (M6b): native build plane.**
  - Swift for Windows, LLVM 23.1.3, and an SDK extractor that materializes symlinks, following xcross's fixes in `windows_swift_plan_repair.dart` [tc-29].
  - Open question from the toolchain research: does Swift Build with `XCODE_EXTRA_PLATFORM_FOLDERS` work on a Windows host, and will SwiftPM keep `--build-system native`?
- **Windows on ARM.** CodeLLDB has no arm64 VSIX, and netmuxd needs test signing. LLVM's woa64 MSI is the likely LLDB source, but nobody has inspected it yet [fc-dv].
- **Test machines.**
  - The WinBoat VM (Windows 11 in Docker; 2 vCPU, 8 GB, no 3D) was not used in this research. It is fine for build CI.
  - USB passthrough of an iPhone into that VM is untested (question 11).
  - GitHub `windows-2022` runners cover build-only CI.

---

## 6. Reverse-engineering targets on your Mac

### 6.1 Ground rules

- **What the licence says.** EA2002 §2.7 prohibits decompiling, reverse engineering or disassembling the Apple Software "except as and only to the extent any foregoing restriction is prohibited by applicable law". §2.5 also bars running Apple Software on non-Apple hardware [xt-30][xr-27].
- **Default methods.** None of these involve disassembly:
  - black-box input/output comparison of Apple's tools on the Mac;
  - observing our own device's protocol traffic;
  - reading Apple's open source (swift-build, xnu, Security, ld64).
- **Disassembly of Xcode frameworks** only after a legal check. The EU Software Directive 2009/24/EC Art. 6 (interoperability) may allow it, but that needs counsel for your jurisdiction (question 4).
- **Clean-room split.** Findings are written down as behaviour and format specs, never as code excerpts, and implementation starts from the spec. Never commit Apple binaries, disassembly or SDK headers.
- **Invasive work goes in a VM.** Anything with SIP off or hooks runs in a macOS VM on the Mac (UTM/tart; the SLA allows two per Apple computer), never on your daily install [xr-28][xr-23].
- **The oracle stays personal and single-user.** The licences forbid making Apple software available over a network for simultaneous use by several computers [xr-27][xr-28].

### 6.2 Prioritized targets

| Priority | Target | Why (gap it closes) | Method | Output |
|---|---|---|---|---|
| P0-1 | Golden corpus for actool, ibtool (`--cocoatouch-compiler-mode toolchain`, plus `simulator` if available), xcstringstool (compile and generate-symbols), and momc | Validate `nomac-actool`, the launch-storyboard subset and xcstrings (M4) | A script runs the `xcrun` tools with Swift Build's exact command lines (xcspecs, `AssetCatalogCompiler.swift`), varying one property at a time. It records versions and compiles twice to test determinism. Diff with `assetutil --info`, `ipsw car` on Linux and nibarchive decoders [xt-03][xt-16][xr-26] | Corpus repo: inputs, outputs, arguments, tool versions, hashes |
| P0-2 | `xcodebuild -dumpPIF`, `-dumpBuildRequest` and `-showBuildSettings` for the Flutter Runner, the RN template, Xcode's templates and 2–3 large OSS apps | `.xcodeproj` to PIF generator (M7) | First confirm the hidden flags still exist in Xcode 27; `-dumpPIF` needs `-scheme` when `-derivedDataPath` is given. Alternatively, capture PIF and build requests by setting `XCBBUILDSERVICE_PATH`/`SWBBUILDSERVICE_PATH` to a proxy or a locally built service. An open swift-build client has already driven Xcode 27 beta 5's service [xr-18][xr-19][fc-xr] | PIF corpus and build-settings dumps |
| P0-3 | Xcode-archived vs Linux-built IPA of the same app | Learn what App Store processing reads, so we stamp as little as possible | `otool -l` (`LC_BUILD_VERSION` tools, `LC_SOURCE_VERSION`, `LC_ENCRYPTION_INFO_64`), `codesign -dvvv`, `rcodesign diff-signatures`, `plutil -p` for the DT* keys, and bundle layout (Frameworks, SwiftSupport, Symbols) [sg-30][fx-29] | Diff report and lint rules |
| P0-4 | DDI inventory: Xcode 27's `Contents/Resources/CoreDeviceDDIs`, `/Library/Developer/CoreDevice/CandidateDDIs` and `/Library/Developer/DeveloperDiskImages`. *Done 2026-10-11: Xcode 27.0 has no `Contents/Resources/CoreDeviceDDIs`; the DDI ships only inside `XcodeSystemResources.pkg` (verified-facts.md, ARCHITECTURE §16.3)* | Let users take DDIs from their own Xcode instead of third-party mirrors | `ls` and `plutil` on each BuildManifest. Map the Personalized and Cryptex images and the boards each supports; Xcode 27.1's PersonalizedDMG stops at iPhone18,5 [xr-10][dv-08] | Import spec for `nomac-deviced` |
| P0-5 | Binary facts | Decide fallbacks; refresh our list of hidden defaults | `lipo -archs` on Xcode 27's `actool`, `ibtool` and `xcodebuild` (arm64-only?). CoreDevice version (642.16 reported). Regenerate the strings corpus, since keith's corpus is frozen at Xcode 16. Check DVT* defaults such as `DVTCoreDeviceEnableDTXTrace` [xr-25][xt-29][xr-11] | Facts file |
| P1-6 | Xcode/devicectl device flows: run plus debug attach, streaming install, Device Hub, testmanagerd on iOS 26.4+ and 27 | Keep our device layer correct on each release; learn the order Xcode uses to attach | `tcpdump` on remoted's `utun` (cleartext HTTP/2 + XPC) decoded with pymobiledevice3's `remotexpc_sniffer.py`; a `socat` relay on `/var/run/usbmuxd`; `rvictl`; CoreDevice logging via `devicectl manage loggingProfile register` plus `log stream` [xr-14][xr-16][xr-17][fc-xr] | Protocol notes and pcaps of our own device |
| P1-7 | Flutter on the Mac against an iOS 26/27 device | Reference for M2: launch arguments, LLDB transcript, number and latency of RX stops | `flutter run -v`; LLDB log channels | Transcript and timings |
| P1-8 | Apple macro expansions: SwiftData, AppIntents, TipKit | Parity for OpenAppleMacros | `swiftc -Xfrontend -dump-macro-expansions` on sample sources, with Xcode 27 | Expansion corpus |
| P1-9 | App Intents metadata | Support for widgets, Shortcuts and Siri | Capture the inputs (`.swiftconstvalues`, stringsdata, binary) and the output (`Metadata.appintents`) while varying intent shapes [xt-26] | Format notes |
| P1-10 | Personal-team provisioning flow (only if free accounts are in scope) | Keep free-account support working through Apple's changes | Unified logs and `CFNETWORK_DIAGNOSTICS`. TLS is pinned, so a MITM proxy will not work. SIP-off hooks only inside the Mac VM [xr-24][sg-14] | Header and endpoint notes |
| P2-11 | CAR internals: actool's 16-bit name-identifier hash, single-size icon expansion, `.icon` iconstack renditions | App Store-grade icons and `.icon` support | Corpus diffs; `assetutil` JSON [xt-09][xt-05] | Format spec |
| P2-12 | Previews runtime (XOJIT, PreviewInfo messages) | A Previews-like loop, later | Swift Previews logging profile plus build-service capture [xr-21][xr-17] | Notes |
| P2-13 | Instruments `.trace` | Trace compatibility (low priority) | `xctrace export` as the oracle [xr-22] | Notes |
| P2-14 | Device-side daemons: dtremotedisplayd, cryptexd, testmanagerd | Protocol depth | From IPSWs, **on Linux**, with ipsw and Ghidra/REA. No Mac needed [xr-15] | Notes |

### 6.3 Method toolbox

- **Static analysis.**
  - ipsw: `class-dump` and `swift-dump --diff` between Xcode versions; `pkg`/`lsbom` on `Contents/Resources/Packages`; `car` for asset catalogs.
  - A fresh strings corpus generated with keith/export-strings.
  - Ghidra 12.1 with the REA MCP server, both installed on this machine [xr-25].
- **Dynamic analysis.**
  - Apple's logging profiles [xr-17]:
    - CoreDevice Host Logging: Debug level plus private data for `com.apple.dt.coredevice`, `remotepairing` and `DeviceKit`.
    - Swift Previews.
  - `lldb`, `dtrace` and Frida against Apple's hardened processes need SIP relaxed from recoveryOS, so use them only in the Mac VM [xr-23].
- **Network capture.**
  - Capturing on `utun` works because the security sits at the tunnel edge (rPlayHub `COREDEVICE-CAPTURE-METHOD.md`) [fc-xr].
  - A `socat` relay for usbmux; `rvictl` [xr-16].
- **Build-service interception.** Swift Build documents `XCBBUILDSERVICE_PATH`, `XCBUILD_LAUNCH_IN_PROCESS=1` and debug defaults (`EnableDebugActivityLogs`, `EnableBuildBacktraceRecording`). Its `launch-xcode` plugin starts Xcode with a custom service [xr-18].
- **Automation on the Mac.** Xcode 27's MCP server (`sudo xcrun mcp-server enable`) and `lldb-mcp` can drive builds and debugger state headlessly [xr-39].

### 6.4 One-time Mac setup (no SIP changes on your main install)

1. **Record a baseline:**
   - macOS version;
   - Xcode version and build (`xcodebuild -version`);
   - `xcrun --find actool ibtool xcstringstool momc`;
   - Swift version;
   - CoreDevice version.
2. **Enable SSH for an `oracle` user,** so `nomac oracle` can run `xcrun` tools for a single user.
3. **Create a macOS VM (tart/UTM) for invasive work.** Caution: installing an Xcode 27 beta on Tahoe 26.5.1 or earlier breaks macOS VM installation (Xcode 27 release notes, 179068335) [fc-xr].
4. **Install the CoreDevice logging profile** with `devicectl manage loggingProfile register`, or download it from Apple's profiles page [fc-xr][xr-17].
5. **Optionally, install Xcode 26.6 next to 27.x** to compare tool versions. It also runs on Apple Silicon [xr-03].

---

## 7. Role of the macOS emulation tracks

### 7.1 Facts that decide it

- **Xcode on Intel.** Xcode 27 installs and runs only on Apple Silicon with macOS Tahoe 26.6+. The last Xcode that runs on Intel is 26.6 Universal (2026-06-25; iOS 26.5 SDK; Swift 6.3.3; needs macOS 26.2+) [xr-01][xr-02][xr-03].
- **App Store deadline.** The App Store has accepted Xcode 26 builds since 2026-04-28. An Xcode 27 / iOS 27 SDK floor will probably arrive around late April 2027. After that, an x86 Xcode can no longer produce builds the App Store accepts [xr-04][sg-29].
- **Tahoe on x86 KVM works, but it is fragile.**
  - OSX-KVM added Tahoe support on 2026-01-26, and OpenCore 1.0.8 shipped 2026-09-27. AMD hosts need specific CPU flags [xr-29].
  - On this machine the RX 7600 (Navi 33) has no macOS driver: no Metal, so no usable Simulator or Previews [xr-30].
  - Signing into an Apple Account inside QEMU guests is unreliable [xr-31].
  - Passing an iPhone through for iOS 17+ CoreDevice is unproven. PCIe USB passthrough worked before iOS 17 (2019), and usbfluxd leaves Xcode stuck at "iPhone is busy" [xr-32].
- **arm64 macOS has no upstream QEMU path.** QEMU's `vmapple` machine needs an Apple Silicon host (HVF) and only runs macOS 12 guests [xr-33]. Darling cannot run a modern Xcode [xr-34][xt-28].
- **Licences.**
  - The macOS SLA (Tahoe EA1955; Golden Gate EA2005) allows up to two macOS VMs per Apple-branded computer you own, not as a service, and forbids running macOS on non-Apple hardware (§2J).
  - Cloud leases must be at least 24 hours, exclusive, and notified to Apple [xr-28].
  - So macOS in KVM on this Ryzen breaches §2J, and so would a QEMU-TCG Golden Gate guest.

### 7.2 When each option is worth it

| Option | Can run | Licence | Practical here | Worth it when |
|---|---|---|---|---|
| Your Apple Silicon Mac | Xcode 27.x and 26.x tools, devicectl, Simulator, Previews | Licensed: single user, not as a multi-user network service | Already exists | Always: it is the oracle and the RE workbench |
| macOS VM on your Mac (UTM/tart, up to 2) | Same as the Mac; SIP can be off | Licensed (§2B(iii)) | Needs spare capacity on the Mac | Invasive RE; a disposable oracle |
| Cloud Macs: GitHub Actions, Codemagic, AWS/Scaleway, Xcode Cloud | Xcode 26.6 (x64 image) and 27 (arm64 preview) | Licensed: CI services; AWS/Scaleway have a 24 h minimum | Paid per use. GitHub: $0.062/min for private repos, free for public ones. Codemagic: 500 free M2 minutes/month. Xcode Cloud: 25 h/month | CI oracle when the Mac is off; licence-clean release builds [xr-35][fx-44][fx-45][fx-46] |
| Tahoe x86_64 KVM VM on this Ryzen | At most Xcode 26.6 CLI tools; no GPU; fragile Apple ID; device passthrough unproven | Breaches SLA §2J | Doable but brittle | Rarely. It adds nothing the Mac lacks, it becomes useless for App Store builds after about April 2027, and it must never be a product dependency |
| arm64 macOS 27 under QEMU TCG (the "Golden Gate" track) | In theory, Xcode 27 tools | Breaches SLA §2J | No upstream path; very high effort; slow (binary translation); no GPU | Research only. If it ever boots, it can plug into `nomac oracle` as an experimental backend |

### 7.3 Recommendation

- **The kit needs no macOS** on Linux or Windows.
- **Its only macOS touchpoint is `nomac oracle`,** a development-time interface with interchangeable backends:
  - your Mac;
  - a VM on your Mac;
  - a cloud Mac;
  - an experimental VM.
- **The emulation tracks stay off the critical path.** This covers both the KVM Tahoe VM and the QEMU Golden Gate effort in `~/RE/goldengate/`. They are worth pursuing only as research, or for personal experiments where you knowingly accept the licence breach.
- **For licence-clean App Store builds,** the documented answer is Apple hardware run by someone else: Xcode Cloud or GitHub's macOS runners [sg-35].

---

## 8. Milestones M0–M7

### 8.0 Summary

| ID | Title | Effort (1 developer + agents) | Depends on |
|---|---|---|---|
| M0 | Foundations and decisions | 3–5 days | Your Xcode (`.xip` or the Mac's copy); answers to section 10 |
| M1 | Tiny end-to-end demo: SwiftUI on an iPhone, from Linux | 1–2 weeks | M0; iPhone connected to this machine |
| M2 | Flutter debug + hot reload from Linux | 2–4 weeks | M1 |
| M3 | Kit v0.1: CLI + VS Code on Linux | 3–5 weeks | M1, M2 |
| M4 | Resource compilers (actool, launch screens, xcstrings) | 4–8 weeks | M0 oracle, M3 |
| M5 | Release builds and App Store / TestFlight | 3–6 weeks | M4; paid Apple Developer Program |
| M6 | Windows (split mode, then native) | 4–8 weeks | M3 (plus M5 for AOT) |
| M7 | Xcode projects, CocoaPods, tests, React Native | 8–16+ weeks | M4, M3, the PIF corpus from your Mac |

### M0: Foundations and decisions (no phone needed)

- **Deliverable:**
  - repository skeleton (GPL-3.0-or-later, with a NOTICE stating that no Apple files are included);
  - `nomac doctor` prototype;
  - SDK installed from your Xcode;
  - version-lock record;
  - Mac oracle baseline;
  - decision record: licences, account type, legal posture.
- **Key work:**
  - Install swift.org Swift 6.4.0 (UBI10 tarball under `~/.local`, no sudo) and `lldb` (`pacman -S lldb`, which needs your sudo).
  - Run `xtool sdk install <Xcode_27.x.xip | Xcode.app>`.
  - Link a UIKit hello-world for `arm64-apple-ios` with the xtool toolset. This exercises the `arm64e.x1` stubs.
  - Check the swift.org `liblldb` for `PlatformRemoteiOS` with `strings`, the same check that was done for Arch's build [dv-11].
  - Audit xlinux, xtool and the pymobiledevice3 APIs.
  - Record the Mac baseline: section 6.4 steps 1–2 and target P0-5.

  First commands:

  ```sh
  xtool sdk install ~/Downloads/Xcode_27.1.xip   # or: xtool setup (login mode, then the .xip path)
  swift sdk list                                  # expect: darwin
  xtool new Hello && cd Hello && xtool dev build  # -> ./xtool/Hello.app
  llvm-otool -l xtool/Hello.app/Hello | grep -A5 LC_BUILD_VERSION
  ```
- **Exit criteria:**
  - `swift sdk list` shows `darwin`.
  - `xtool dev build` produces `Hello.app`, and its binary shows `LC_BUILD_VERSION platform IOS`.
  - `doctor` reports the Swift, SDK, LLVM and LLDB versions and flags the limitation of the system lld 23.1.1.
  - The Mac facts file is committed.
  - The decisions are recorded.
- **Effort:** 3–5 days.
- **Depends on:** your Xcode; answers to questions 1–5.

### M1: Tiny end-to-end demo (SwiftUI, Linux)

- **Deliverable:** `demo/m1.sh` plus a recorded transcript. The script:
  - builds a SwiftUI "Hello" app on Linux and signs it;
  - installs it on your iPhone and launches it suspended;
  - attaches LLDB over the no-root tunnel and hits a breakpoint;
  - inspects variables and captures stdout.
- **Key work:**

  ```sh
  idevicepair pair                               # if iOS 26 drops it at the Trust prompt: usbmuxd --no-preflight (#281)
  pymobiledevice3 amfi reveal-developer-mode     # then enable Developer Mode on the phone
  pymobiledevice3 mounter auto-mount             # DDI (see risk R2 about its source)
  python3 scripts/debugserver_probe.py           # gdb-remote sanity check without LLDB
  xtool dev                                      # sign + install; its launch step fails on iOS 17+ (#44)
  pymobiledevice3 apps install-record <bundle-id>                  # on-device install path
  pymobiledevice3 developer dvt launch "<bundle-id>" --suspended   # prints the pid
  pymobiledevice3 developer debugserver start-server --local-port 41234 &
  lldb -s demo/attach.lldb                       # remote-ios; target create; SetPlatformFileSpec; process connect; process attach --pid
  ```

  - Try the swift.org LLDB first, then Arch's LLDB.
  - Record what `frame variable` shows.
  - Try `po` with `target.sdk-path` set to the xtool SDK. This is the experiment for [dv-17].
  - Time the attach, because system libraries are read from process memory [dv-16].
  - Use a paid account if you have one: free-account installs currently fail with 0xe8008015 on some iOS 26.5/27 devices [fc-sg].
- **Exit criteria:** on a real iPhone running iOS 26 or 27, all from Linux and without root:
  - the app icon is visible;
  - a breakpoint in a Swift function stops;
  - the backtrace shows symbolized app frames;
  - after `continue` the app keeps running;
  - console output is captured;
  - the script re-runs cleanly.
- **Effort:** 1–2 weeks.
- **Depends on:** M0; the iPhone connected to this machine with Developer Mode on; an Apple account.

### M2: Flutter debug + hot reload on device from Linux

- **Deliverable:**
  - a `nomac flutter` custom-device adapter;
  - a storyboard-free UIScene Runner template;
  - a build for SwiftPM plugins;
  - VM-service forwarding;
  - the RX-page JIT helper, with manual continue on iOS 27;
  - a demo: a counter app with two SwiftPM plugins.
- **Key work:**
  - Implement sections 4.2–4.4.
  - Measure JIT mode against interpreter mode, and record the "Dart execution mode" line from the device log.
  - Integrate with Dart-Code.
  - Audit xlinux and reuse it where possible.
- **Exit criteria:**
  - `flutter run -d <iphone>` works from Linux on iOS 26 and/or 27.
  - Hot reload and hot restart work, and DevTools connects.
  - Reload time is recorded.
  - The execution mode is documented for each iOS version tested.
- **Effort:** 2–4 weeks.
- **Depends on:** M1; Flutter 3.47.x.

### M3: Kit v0.1 (CLI + VS Code, Linux)

- **Deliverable:**
  - `nomac` v0.1 with `doctor`, `sdk`, `build`, `sign`, `account`, `device`, `run`, `debug`, `logs` and `flutter`;
  - `nomac-deviced`, which holds the userspace tunnel and port forwards;
  - VS Code extension v0.1: tasks, lldb-dap/CodeLLDB attach, BSP;
  - documentation.
- **Key work:**
  - Symbol cache: `pymobiledevice3 developer fetch-symbols` with an explicit output directory, plus `ipsw dyld extract --all`. Test whether LLDB loads the result [dv-15][dv-16].
  - Write up the result of the `po` investigation.
  - A CI matrix that builds the samples against the Xcode 26.6 and 27.x SDKs. You provide the SDK; it never goes into public CI.
  - Optional: Wi-Fi through iOS 27 `pair-host` or netmuxd, with documented firewall rules (mDNS on 5353, the pair-host port) [dv-30][dv-31].
- **Exit criteria:**
  - F5 in VS Code on Linux builds, installs, attaches and stops at a breakpoint in the SwiftUI sample.
  - The Flutter sample runs from VS Code.
  - The status of `po` is documented.
  - v0.1 is tagged.
- **Effort:** 3–5 weeks.
- **Depends on:** M1, M2.

### M4: Resource compilers

- **Deliverable:**
  - `nomac-actool`, a Swift CLI on AssetKit (MIT);
  - a storyboard-to-`UILaunchScreen` converter, plus a narrow launch-storyboard compiler only if needed;
  - xcstrings completion: `.stringsdict` and generate-symbols;
  - a golden corpus from the Mac;
  - an on-device harness app that loads every asset and nib;
  - upstream PRs.
- **Key work:**
  - Implement the Swift Build contract [xt-03][xt-04]:
    - three invocations;
    - the `--version` plist;
    - dependency info;
    - a partial Info.plist;
    - GeneratedAssetSymbols.
  - Expand single-size 1024 icons.
  - Support appearances.
  - Choose name identifiers that do not break lookups; actool's hash is unknown [xt-09].
- **Exit criteria:**
  - The scenario from xtool #310 (a package with `.xcassets`, built through Swift Build) builds on Linux.
  - The Flutter and RN template catalogs compile.
  - The outputs parse with Apple's `assetutil` on the Mac.
  - The harness app loads every asset on the phone.
  - PRs are open on AssetKit and xtool.
- **Effort:** 4–8 weeks.
- **Depends on:** M0 (oracle), M3.

### M5: Release builds and distribution

- **Deliverable:**
  - a Flutter AOT pipeline whose `gen_snapshot` is built from the Dart sources in our CI for each Flutter tag, linux-x64 first;
  - packaging lints and opt-in metadata stamping;
  - `nomac ship` on the ASC API;
  - an "Apple-hardware release" mode using a GitHub macOS runner or Xcode Cloud.
- **Exit criteria:**
  - SwiftUI and Flutter samples uploaded from Linux are processed as VALID **and install through TestFlight on your phone**. This closes the gap in omarchy's evidence.
  - The same samples are released through the Apple-hardware path.
  - The differences between the two paths are documented.
- **Effort:** 3–6 weeks.
- **Depends on:** M4 (`Assets.car`); paid Apple Developer Program.

### M6: Windows

- **Deliverable:**
  - W1, split mode: a native Windows device daemon and LLDB, with the build in WSL2.
  - W2, a native Windows build plane for SwiftPM apps and Flutter, including a Windows-host `gen_snapshot` from our CI.
- **Exit criteria:**
  - The M1 and M2 demos pass from Windows 11 x64 with the phone attached to Windows.
  - `windows-2022` CI builds the samples.
  - Windows on ARM status is documented.
- **Effort:** 4–8 weeks.
- **Depends on:** M3 (M5 for AOT); a Windows machine with USB access to the phone.

### M7: Xcode projects and the ecosystem

- **Deliverable:**
  - `.xcodeproj` and `.xcworkspace` support (pbxproj and the JSON `.xcproj`): generate PIF, then build with Swift Build, validated against the Mac corpus;
  - CocoaPods support, either podspec to SwiftPM or Pods.xcodeproj through PIF;
  - `nomac test` (XCTest/XCUITest with JUnit output);
  - a React Native / Expo adapter;
  - a DVT to Perfetto exporter;
  - upstream contributions.
- **Exit criteria:**
  - Three Xcode-project apps build from `.xcodeproj` on Linux and run on the phone, for example NetNewsWire, IceCubesApp (both already built through adapters by omarchy [tc-21]) and the RN template.
  - An XCUITest suite runs with JUnit output.
- **Effort:** 8–16+ weeks.
- **Depends on:** M4, M3, the P0-2 corpus.

---

## 9. Risk register

| ID | Risk | Likelihood | Impact | Mitigation | Early warning |
|---|---|---|---|---|---|
| R1 | **Xcode/SDK licence.** EA2002 §2.5/§2.7 forbid using Apple SDKs or software on non-Apple hardware, so users breach it by building on Linux or Windows [tc-31][fx-47][xr-27] | Certain (it is the licence text) | High: reputation, takedowns, user liability | Ship no Apple files; accept only a user-supplied Xcode; plain-language docs; a licence-clean release mode on Apple hardware; the SDK-free mode as a research hedge; legal review before a public launch | Apple legal notices to xtool or xcross; DMCA notices on GitHub |
| R2 | **DDIs** are Apple files served by unlicensed mirrors, and personalization needs Apple's TSS server [dv-08] | High | Medium | Import DDIs from the user's own Xcode (P0-4); never host them; mirrors only as an explicit opt-in | A mirror is removed; new hardware is missing from the mirrors |
| R3 | **Private APIs for free accounts.** ADPLA §2.1/§2.8 and the Apple Developer Agreement §3 restrict non-Apple access, and Apple can lock accounts [fc-sg][sg-12] | Medium | Medium–High | Make the paid ASC API the default; free mode is opt-in with warnings (xtool suggests a throwaway Apple ID) | 503/429 errors; new GSA headers |
| R4 | **Metadata stamping.** Writing Xcode/SDK values (`DTXcode`, `LC_BUILD_VERSION` tools) into Linux builds could be judged misrepresentation, and Apple could add fingerprint checks [sg-30][fx-29] | Medium | High: rejections, account trouble | Opt-in and visible; prefer Apple-hardware release builds; use the P0-3 diff to stamp as little as possible | New ITMS errors on Linux uploads |
| R5 | **Apple-side breakage:** GSA identity changes, JIT/TXM rules, CoreDevice protocol churn, DDI format, AMFI/dyld rules [sg-14][dv-19][dv-34][dv-07][sg-07][sg-08] | High (every release) | High | Device matrix for each iOS beta; follow pymobiledevice3, xtool, AltStore and SideStore; backend abstraction; a spare test device on betas | iOS betas in June and September; GSA errors |
| R6 | **Version coupling:** Xcode ↔ Swift; Flutter engine ↔ `gen_snapshot` ↔ Dart; LLVM ↔ SDK TBDs [tc-03][fx-51][tc-05] | High | Medium | Manifest pins; `doctor` checks; CI for each Flutter tag | "SDK not supported by the compiler", "Invalid SDK hash" |
| R7 | **Fragile dependencies.** Key projects are weeks old, single-maintainer or largely AI-written: xlinux (2 stars), omarchy (agent-merged PRs), xcross_gen_snapshot (no licence), the viraptor tools [fx-24][sg-32][tc-34][tc-19] | High | Medium | Pin versions; vendor only where the licence allows; contribute; own the critical path (debug glue, Flutter adapter) | Stale repositories; breaking releases |
| R8 | **Licence traps:** AGPL (xcross 2.0+, StikDebug), unlicensed repositories, non-commercial StikPair [fx-25][tc-34][fc-dv] | Medium | Medium | Use them only as subprocesses or references; ask the authors; rebuild from the BSD-licensed Dart sources | Licence changes (xcross moved from MIT to AGPL on 2026-10-08) |
| R9 | **App Store processing of open-tool output:** a malformed `Assets.car` is rejected as INVALID or stalls for hours [sg-31][tc-22] | Medium | Medium | Golden corpus; a TestFlight canary; fall back to an Apple-hardware build | Builds stuck in PROCESSING |
| R10 | **Free-account install failures** (0xe8008015) across tools on iOS 26.5/27 [fc-sg] | Medium | Medium: blocks a free-account M1 | Use a paid account for demos; track iloader#570 and Impactor#224 | The same error in our tests |
| R11 | **Performance:** JIT stops of 130–150 ms; the userspace tunnel at about 20 MB/s; interpreter speed unknown; symbol reads from process memory [fx-22][dv-16] | Medium | Medium | Sparse page writes; a native gdb-remote helper; a symbol cache; the kernel tunnel as an option | Slow reloads and attaches |
| R12 | **arm64e / Enhanced Security apps** cannot be linked [fc-tc] | Low (few apps today) | High for those apps | Declare them unsupported; watch llvm #188378 | Users asking for Enhanced Security |
| R13 | **Windows complexity:** path limits, symlinks, the dependency on Apple's driver, gaps on Windows on ARM [tc-29][dv-32][fc-dv] | Medium | Medium | Split mode first; CI on windows-2022; document the driver choice | — |
| R14 | **Limits of RE on the Mac:** dynamic work needs SIP relaxed, TLS is pinned, and the main Mac is at risk [xr-23][xr-24] | Medium | Low–Medium | A VM on the Mac for invasive work; black-box methods first | — |
| R15 | **Hardware availability:** no phone is connected now; the iPhone 18 needs the Cryptex DDI; beta iOS (27.2) behaves differently [dv-07][fc-fx] | High (now) | High: blocks M1 | Question 1; a test matrix | — |
| R16 | **Emulation fallbacks** breach the SLA and are fragile [xr-28][xr-29][xr-33] | n/a | Medium | Keep them off the critical path | — |
| R17 | **Swift `po` may not work off-Mac** [dv-17] | Medium | Medium | Fall back to `frame variable` and reflection; document; contribute fixes | The M1 experiment |
| R18 | **Flutter debug-hook regressions** with each Xcode, iOS or Flutter release (#184254, #190307; #170416 still open) [dv-21] | High | Medium | A regression test for each Flutter tag and iOS beta | The Flutter issue tracker |
| R19 | **The App Store SDK floor moves to Xcode 27 / iOS 27,** likely around April 2027 [xr-04] | High | Low for us (we use the user's SDK); high for x86 fallbacks | Keep the SDK import current; drop the x86 VM idea | Apple's "Upcoming requirements" page |

## 10. Questions only you can answer

1. **iPhone.**
   - Which model(s) and iOS version(s) can you test with? An iPhone 18 needs the Cryptex DDI. Is any device on a beta?
   - When will one be connected to this Linux machine?
   - May we turn on Developer Mode on it?
2. **Apple account.**
   - Do you have a paid Apple Developer Program membership? The App Store Connect API, TestFlight and ad hoc distribution all need one.
   - If you only have a free Apple ID, is a throwaway Apple ID acceptable for the private-API flow?
3. **Your Mac.**
   - What model, macOS version and Xcode version/build (27.0? 27.1 RC? also 26.x)?
   - Should the Linux SDK come from a freshly downloaded `.xip`, or from the Mac's `Xcode.app`?
   - May the Mac, or a VM on it, be reachable over SSH as a single-user oracle?
   - Is a macOS VM with SIP disabled, on the Mac, acceptable for dynamic RE?
4. **Legal posture.**
   - Which jurisdiction applies? This decides whether the EU interoperability exception is available.
   - Do you accept the Xcode SDK licence risk for personal use?
   - For the public kit: should the default release path be Apple hardware (Xcode Cloud or GitHub macOS runners), with Linux-native App Store builds and metadata stamping either opt-in or left out?
5. **Licensing.**
   - Is this split acceptable: GPL-3.0-or-later for the kit, and MIT for pieces meant for upstream (actool CLI, macros, xtool patches)?
   - Are AGPL dependencies such as xcross acceptable when run as separate processes?
6. **Scope and priority.**
   - Which comes first: SwiftUI/SwiftPM, UIKit with storyboards, Flutter, or React Native?
   - Do you need App Store distribution in v1, or are development and debugging enough?
   - Do you need Wi-Fi debugging?
7. **Flutter.**
   - Which channel and version, and which plugins? Are any CocoaPods-only?
   - Should we collaborate with xlinux upstream, or keep our own adapter?
8. **Windows.**
   - Is WSL2 acceptable as the interim build path?
   - Is Windows on ARM in scope?
   - Is fully native (no WSL) a must?
9. **Project setup.**
   - What name should replace the placeholder `nomac`?
   - Where should the repository live?
   - Publish publicly from day one, or after M1?
10. **Budget.** Is a small CI spend on cloud Macs (Codemagic or GitHub minutes, Xcode Cloud hours) acceptable for oracle CI and licence-clean releases?
11. **Windows test machine.** Can the iPhone be passed through over USB to the WinBoat Windows VM, or is there a physical Windows PC?

## Appendix A: version pins (2026-10-10)

| Item | Pin | Source |
|---|---|---|
| Xcode | 27.0 (27A266a, 2026-09-14); 27.1 RC (27A9275, 2026-10-05); 27.2 beta 2 (27B5028f) | [tc-01][xr-01] |
| Swift for each Xcode | 27.x → 6.4.0; 26.4–26.6 → 6.3.x; 26.0–26.3 → 6.2.x | [tc-01][tc-03] |
| Last Intel Xcode | 26.6 (17F113, 2026-06-25): iOS 26.5 SDK, Swift 6.3.3, needs macOS 26.2+ | [xr-02][xr-03] |
| LLVM / lld | 23.1.3 or newer for Xcode 27 TBDs (this host has 23.1.1), or xtool darwin-tools v1.1.0 | [tc-05][tc-46] |
| xtool | 1.21.0 (2026-10-05, re-checked) | [tc-10] |
| pymobiledevice3 | v11.26.0 (2026-10-08, re-checked) | [dv-06] |
| go-ios | v1.3.2 (2026-08-11); Cryptex DDI support only on main | [dv-09] |
| idevice | v0.1.68 (2026-09-15) | [dv-29] |
| libimobiledevice / usbmuxd | 1.4.0 (2025-10-10) / 1.1.1 (2020) | [dv-01][dv-03] |
| rcodesign / zsign | 0.29.0 (2024-11-29) / v1.1.2 (2026-08-21) | [sg-01][sg-06] |
| OpenAppleMacros | 1.3.0 (tracks Xcode 27A266a) | [tc-14] |
| Flutter | 3.47.7 stable (2026-10-08; engine deb2874; Dart 04bcd10 = 3.13.5) | [fx-01][fc-fx] |
| xlinux / xcross | v0.5.1 (2026-10-05, GPL-3.0) / v2.0.1 (2026-10-10, AGPL-3.0), both re-checked | [fx-24][fx-25] |
| LLDB | Arch 23.1.1; CodeLLDB 1.12.3 (LLDB 22.1.8); LLVM 23.1.3 Windows MSI | [dv-11][dv-12] |
| iOS | 27.0 (2026-09-14), 27.0.1 (2026-09-28), 27.2 beta 3 (2026-10-05) | [fc-fx] |
| App Store floor | Xcode 26 + iOS 26 SDK (since 2026-04-28); apps must target iOS 13+ (since 2026-09-09) | [sg-29] |

## Appendix B: upstream threads to watch

- **xtool:**
  - #44: launch on iOS 17+
  - #144: `.xcodeproj` support
  - #219: asset catalogs (has merge conflicts)
  - #310: actool under Swift Build
  - #311: simulator
  - #131: per-bundle entitlements
  - #305 and #308: free-team certificate and token
  - PRs #290–#293 and #302
- **LLVM:** PR #188378 (arm64e linking).
- **Flutter:**
  - #56511: non-mac iOS backend
  - #191429 and PR #191369: SDK version stamp
  - #170416: Add2App JIT hook
- **Dart:** #61635 (modular AOT for iOS development), #60202, #60205.
- **go-ios:**
  - #614: debugproxy
  - #700: symbols
  - #749: TLS-PSK Wi-Fi
  - #782: iOS 27 pairing
- **libimobiledevice / usbmuxd:** libimobiledevice #1638 (RemoteXPC); usbmuxd #281 (pairing timing).
- **Free accounts:** nab138/iloader#570 and claration/impactor#224 (0xe8008015).
- **Other:**
  - CocoaPods/Xcodeproj#1052: `.xcproj` support
  - indygreg/apple-platform-rs#30: IPA signing
