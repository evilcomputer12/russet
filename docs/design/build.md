# Russet design: the build plane

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates). Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day) changed D7 and §5.4 (native storyboard compiler, B21a–c), §3.4 and §10 (Russet's own SDK importer, W16b) and settled Q6/Q7. Review round 2 (same day) added banners to §9.1, §9.2, §9.4–§9.7 and to the B02/B08 acceptance (ARCHITECTURE §20.1 lists the superseded fields), moved B21c to its own card (W28c) and gave B21a a single-scene main storyboard with an ObjC custom class ([D 90]).

**Area:** build plane (turning a project into a signed-ready `.app`, `.ipa` and dSYMs) · **Status:** design, 2026-10-11 · **Owner of this doc:** build-plane design agent · **Binding inputs:** [decisions.md](../decisions.md), [verified-facts.md](../verified-facts.md), [m1-baseline.sh](../../demo/m1-baseline.sh), [landscape report](../research/landscape-2026-10.md) §2.1–2.6, §4, §5, §6, §8.

**Markers used in this document**
- **[V#]**: verified hands-on during this design pass. Commands and results are in [Appendix A](#appendix-a-verification-log).
- **[S#]**: external source. URLs are in [Appendix B](#appendix-b-sources).
- **[L§x]**: the landscape report section (and its fact-check record IDs such as [tc-05]).
- **ASSUMPTION**: not verified; each one has a task that verifies it.

**What this area owns:** project detection, compilation and linking orchestration, resource compilers, Info.plist synthesis, bundle assembly, Mach-O lints, IPA packaging, the per-stack adapters (SwiftPM, Flutter, Xcode projects, React Native, .NET MAUI), and the build parts of the CLI, daemon (JSON-RPC) and MCP.

**What it consumes from other areas (assumed interfaces, to be reconciled by the lead):**
- **SDK/toolchain area:** a resolved `SdkFacts` record and a Russet-owned "platform view" directory (section 3.4).
- **Signing area:** takes the unsigned bundle plus a `SigningPlan` (section 9.1), and supplies the `BundleIdPolicy` for the account in use.
- **Device area:** installs and launches the result. It also implements the device side of the Flutter custom-device commands (section 6.3).
- **Core/daemon area:** process model, JSON-RPC transport and event fan-out.
- **IDE and MCP areas:** call the methods in sections 9.4 and 9.5.

---

## 1. Decisions at a glance

| # | Topic | Decision | Why |
|---|---|---|---|
| D1 | SwiftUI/SwiftPM builds | Russet generates its own **builder package** and runs `swift build --build-system swiftbuild` with a Russet toolset and a Russet platform view (xtool's technique, re-implemented in Rust). Fallbacks: `--build-system native`, and `xtool dev build` as a compatibility backend. | Proven in M1. We control linker flags, tool shims and bundle assembly. No dependency on the xtool binary, which is Linux-only (Windows needs WSL). |
| D2 | `LC_BUILD_VERSION` sdk | Fixed **at link time**: the toolset adds `-Xclang-linker -isysroot -Xclang-linker <SDK>` to every swiftc-driven link. Direct clang links always pass `-isysroot`. No post-link patching of our own binaries. | Root cause found and fix verified ([V3]–[V6]). The output is `sdk 27.0, minos 17.0`, the same as Xcode. Forward-compatible with swift-driver PR #2158 [S1]. |
| D3 | Info.plist | Russet synthesizes it, with Xcode's precedence rules and DT* keys derived from the imported SDK (section 4.4). It also **normalizes DT* keys in every bundle Russet built**, because Swift Build on Linux stamps `DTXcode=10989` and `DTXcodeBuild=99T999` [V9]. | Truthful SDK provenance. Avoids bogus "Xcode 99.99.99" stamps. |
| D4 | Bundle IDs | Paid team: the **exact** bundle ID, always. Free account: a configurable policy (default `<id>.<TEAMID>`; `XTL-<prefix>.<id>` for xtool compatibility). The policy is applied at build time, and the original-to-effective map is emitted for the signing and device areas. | xtool prefixes `XTL-<issuer prefix>.` on all accounts [V12]. App Store records need exact IDs. |
| D5 | IPA | Native Rust writer (`zip` crate), deterministic, symlink- and permission-aware, with `SwiftSupport/` for store builds. No `zip` binary. | xtool shells out to `zip -yqru0`, which Arch does not install [verified-facts]. |
| D6 | Asset catalogs | Our own **Rust CAR writer** (`russet-car`, MIT) and an actool-compatible CLI (`russet-actool`), exposed to Swift Build through the platform view's `usr/local/bin`. AssetKit (MIT) is the format reference and a differential oracle. Phase A ships a "loose PNG" stand-in. | Single static binary on Linux and Windows. The IDE asset editor can call it in-process. The actool contract is pinned by [V7], [V8], [V13]–[V15]. |
| D7 | Storyboards | Compile them natively with `russet-ib` (NIBArchive): launch storyboards in B21a, main storyboards and XIBs in B21b/B21c. Until a storyboard is supported, convert a launch storyboard to `UILaunchScreen` when that is lossless, and replace known template `Main.storyboard`s with a generated scene delegate (kept as a fallback). `[build] launch-storyboard = "auto"` picks compile, then lossless convert, then `launch-screen-fallback` (ARCHITECTURE [D 75]). | ibtool is closed and Mac-only, but the NIBArchive format and the storyboardc layout are documented (§5.4) and MIT prior art exists (omarchy-apple-dev, WinObjC xib2nib). `UILaunchScreen` is lossy for React Native and flutter_native_splash. |
| D8 | xcstrings | Rust `xcstringstool`-compatible `compile`/`generate-symbols` (`russet-xcstrings`, MIT), seeded from algoriddim/xcstringstool (MIT). | The CLI contract is pinned by the Xcode 27 oracle [V16]. |
| D9 | Flutter debug | A custom device declared `linux-x64`, a storyboard-free UIScene Runner, `flutter assemble` driven exactly like `xcode_backend.dart` behind Russet `xcrun`/`codesign` shims, and Flutter.xcframework from `flutter precache --ios`. | xlinux technique (GPL-3.0, reusable) [S20]. Flutter's interpolation contract verified in source [S16]. |
| D10 | Flutter release | `gen_snapshot` for linux-x64 and windows-x64 hosts, built **in our CI** from the engine's Dart revision, with the `--macho-sdk-version` backport (Dart c8495e3003 [S12]), byte-compared against the official macOS compiler on a macOS runner. | Licence-clean (BSD-3). Gives the correct SDK version without patching. |
| D11 | Xcode projects | `.xcodeproj`/`.xcworkspace`/`.xcproj` are parsed in Rust, turned into a **JSON PIF**, and built by `russet-swb`, a thin Swift driver over the open `SwiftBuild` library, shipped per Swift minor version. Before M7, simple app targets are adapted to SwiftPM. | The OSS build service loads `.xcodeproj` only by shelling out to `xcodebuild -dumpPIF`, which is macOS-only. It loads JSON PIF directly [V17]. |
| D12 | RN/Expo, MAUI | Later phases. They reuse the Xcode-project plane, CocoaPods and the `xcrun` shim. MAUI starts as a feasibility spike (section 8). | Decisions order the stacks. Both need `.xcodeproj` and CocoaPods first. |

---

## 2. Facts this design rests on

Each item is verified on this machine (Swift 6.4, xtool's Darwin SDK from Xcode 27.0 27A266a) or on the Mac oracle unless marked otherwise. Full commands are in Appendix A.

1. **Why `LC_BUILD_VERSION` said `sdk 17.0`** [V3]–[V6]:
   - Swift Build links Swift targets with `LINKER_DRIVER=swiftc` and passes `-sdk` (not `-isysroot`); see `Ld.xcspec` lines 167–189 [S2].
   - swift-driver 6.4 (branches `release/6.4.0`, `6.4.2`, `6.4.x`) hands the link to clang with `--sysroot` [S1].
   - clang reads `SDKSettings.json` only via `-isysroot` or the `SDKROOT` environment variable. Without either, it emits `-platform_version ios <min> <min>`.
   - xtool deliberately unsets `SDKROOT` [S5]. Setting it globally breaks SwiftPM's host manifest link [V4].
   - swift-driver main switched to `-isysroot` in PR #2158, merged 2026-07-15 and not in any 6.4 branch [S1].
2. **Fix:** the toolset `swiftCompiler.extraCLIOptions = ["-Xclang-linker","-isysroot","-Xclang-linker",<SDK>]` gives `sdk 27.0` on the app executable and on package dylibs [V5][V6]. Swift Build maps toolset Swift flags into `OTHER_LDFLAGS` for swiftc-driven links [S3]. In lld, a trailing `-platform_version` overrides an earlier one [V10].
3. **Swift Build resolves `actool` only through its executable search paths:** `<platform>/usr/bin`, `<platform>/usr/local/bin`, `<platform>/Developer/usr/bin` and `<platform>/Developer/usr/local/bin` [S4]. It does not search `PATH`. With no `actool` found, it tries `<package dir>/actool` and fails (reproduces xtool #310) [V7]. A Russet **platform view** with our tool in `usr/local/bin` works [V8].
4. **The exact actool command lines Swift Build issues on Linux** (symbols pass and thinned compile; cwd = package root; task environment sanitized to empty) [V8]. The Xcode 27 actool output contract (symbols-only mode, compile mode, partial Info.plist, loose icons, dependency info, CAR header) [V13]–[V15].
5. **DT* keys** from the SDK (Xcode 27.0): `DTPlatformBuild`=`DTSDKBuild`=`24A430`, `DTSDKName`=`iphoneos27.0`, `DTPlatformVersion`=`27.0`, `DTXcode`=`2700`, `DTXcodeBuild`=`27A266a` [V11]. On Linux, Swift Build's own Info.plist processor stamps `DTXcode=10989`, `DTXcodeBuild=99T999` and an empty `DTPlatformBuild` into the resource bundles it generates [V9]. The cause is that it treats a Swift-toolchain install as Xcode 99.99.99 [S6].
6. **Generated paths leak into binaries:** xtool/Swift Build bake absolute host paths into `LC_RPATH`, such as `/home/…/PackageFrameworks` and `/usr/lib/swift/usr/lib/swift-6.2/iphoneos` [V18].
7. **Swift Build on Linux cannot link a SwiftPM `.dynamic` product into the builder executable.** It produces a loose `libDyn.dylib` and the link fails with an undefined symbol, deterministically. The native build system links it [V19].
8. **Swift back-deployment dylibs** (`libswiftCompatibilitySpan.dylib`, `libswift_Concurrency.dylib`) exist only in the SDK bundle's full `Xcode.app` copy (xtool "normal" flavor) [V20]. Swift Build adds an rpath for them.
9. **lld writes `LC_ENCRYPTION_INFO_64`** for iOS executables, as Apple's ld does. It does not write `LC_SOURCE_VERSION`, and its build-version tool entry is `lld 21.1.6`. Apple's linker writes `ld 27037.1` [V21].
10. **Swift Build loads a JSON PIF directly.** For `.xcodeproj`/`.xcworkspace` it shells out to `xcrun xcodebuild -dumpPIF` [V17]. It ships an `swbuild` console tool and a `SwiftBuild` library product [S7].
11. **Flutter:**
    - Custom-device `runDebug` receives `${appName}`, `${remotePath}`, `${engineOptions}`, `${buildMode}`, `${icuDataPath}` and `${engineRevision}`. The VM service is discovered from the command's output [S16].
    - Flutter's iOS AOT invocation (`--snapshot_kind=app-aot-macho-dylib` …) [S15].
    - Dart 3.13.5 writes `LC_BUILD_VERSION` with sdk = minos. `--macho-sdk-version` exists on Dart main since c8495e3003 [S12][S13].
12. **No Mac-only tool is runnable off-Mac:** Xcode 27 `actool`, `ibtool` and `xcstringstool` are arm64-only [V16].

---

## 3. Architecture

### 3.1 Crates and binaries

| Crate / binary | Kind | Licence | Responsibility |
|---|---|---|---|
| `russet-build` | lib | GPL-3.0-or-later | Orchestrator: request, plan, execute, report. Stack adapters (`stack::{swiftpm, flutter, xcode, rn, maui}`), bundle assembly (`bundle`), Info.plist synthesis (`infoplist`), events, build reports. |
| `russet-plist` | lib | MIT | plist I/O: XML and binary via the `plist` crate, plus our own OpenStep (ASCII) reader/writer. Xcode build-setting macro expansion (`$(VAR)`, `${VAR}`, `$(VAR:op)`). |
| `russet-macho` | lib | MIT | Mach-O read and lint. Safe in-place edits (delete `LC_RPATH`, thin fat files). A last-resort `LC_BUILD_VERSION` sdk patch, used only for foreign binaries. |
| `russet-ipa` | lib | MIT | Deterministic IPA writer and reader. |
| `russet-car` | lib | MIT | BOM container and CoreUI CAR writer/reader. |
| `russet-assets` | lib | MIT | `.xcassets` model, icon expansion, partial Info.plist, symbol generation, dependency info. |
| `russet-xcstrings` | lib | MIT | `.xcstrings` → `.strings`/`.stringsdict`, plus symbol generation. |
| `russet-ib` | lib | MIT | Storyboard/xib reader; launch-storyboard → `UILaunchScreen` converter (B12); NIBArchive codec and storyboard/XIB compiler (B21a–c). |
| `russet-xcodeproj` | lib | MIT | `project.pbxproj`, JSON `.xcproj`, `.xcworkspace`, `.xcscheme`, `.xcconfig`; layered build-settings resolver. |
| `russet-pif` | lib | MIT | PIF emitter, in the format of `xcodebuild -dumpPIF` output. |
| `russet-tool` | bin (multi-call) | MIT | Dispatches on `argv[0]`: `actool`, `ibtool`, `xcstringstool`, `xcrun`, `codesign` (no-op), `ld64.lld` (trampoline), and `momc`/`metal`/`appintentsmetadataprocessor` (clear errors). Installed as hard links or copies in the platform view and in a shim `bin/` directory. |
| `russet-swb` | Swift executable | our code MIT; links `SwiftBuild` (Apache-2.0) | Feeds a JSON PIF and a build request to Swift Build and streams JSON-lines events. Built in our CI per Swift minor version (M7). |

All crates use `camino` UTF-8 paths, `tokio` for subprocesses, `thiserror` errors with stable codes (section 9.7) and `serde` for every wire type.

### 3.2 Common pipeline

```text
BuildRequest
  -> resolve      project detect + russet.toml + stack config; SdkFacts from the SDK area; toolchain lock check
  -> prepare      stack adapter: resolve dependencies; generate builder package / Flutter packages / PIF
  -> compile+link stack adapter: swift build | flutter assemble + Runner link | russet-swb
  -> resources    main-bundle catalogs, xcstrings, launch screen (Swift Build handles package resources itself)
  -> assemble     .app layout, Info.plist synthesis, DT normalization, bundle-ID policy,
                  Swift back-deploy embedding, rpath hygiene, entitlements collection
  -> lint         russet-macho + bundle lints (error in release/ship, warning in debug)
  -> package      optional IPA (dev: stored; distribution: deflate + SwiftSupport)
  -> BuildOutput  + build-report.json + SigningPlan (handed to the signing area)
```

Signing is not done here. The build plane produces an unsigned bundle and a `SigningPlan` that lists items in inside-out order. Each item carries its entitlements and a `get-task-allow` flag.

**Destinations.** There is exactly one build destination: `iphoneos` / `arm64`.
- **No simulator builds.** Per the 2026-10-11 decision, the device mirror replaces the simulator, and running Apple's simulator off-Mac is rejected.
- **Virtual iPhone** (a separate private track; decisions.md). If it ever becomes a Russet target, it consumes the same device build, signed like a physical device. Whether it accepts standard `arm64` (non-`arm64e`) apps is that track's question; `arm64e` would be blocked on LLVM #188378 [L§2.4].

### 3.3 On-disk layout and disk budget

```text
<project>/russet.toml
<project>/.russet/                       # git-ignored (repo .gitignore still lists ".orchard/": rename to ".russet/")
  build/<profile>-iphoneos/<App>.app
  build/<profile>-iphoneos/<App>.app.dSYM   (release/profile, or dsym = "always")
  build/<profile>-iphoneos/<App>.ipa        (only when requested)
  build/<profile>-iphoneos/build-report.json
  work/builder/Package.swift                (generated SwiftPM builder)
  work/swiftpm/                             (SwiftPM scratch path; Swift Build "out/")
  work/flutter/ , work/pif/ , work/actool/  (stack intermediates)
  logs/<build-id>.jsonl                     (full event stream)

$XDG_DATA_HOME/russet  (Windows: %LOCALAPPDATA%\Russet\data)
  sdk/<sdk-id>/view/Developer/Platforms/iPhoneOS.platform/...   (platform view, section 3.4)
  sdk/<sdk-id>/toolset-swb.json, toolset-native.json
  tools/<russet-version>/bin/{actool,ibtool,xcstringstool,xcrun,codesign,ld64.lld,...} -> russet-tool
$XDG_CACHE_HOME/russet (Windows: %LOCALAPPDATA%\Russet\cache)
  flutter/engine/<engine-hash>/{ios,ios-profile,ios-release}/
  flutter/gen_snapshot/<engine-hash>/<mode>/<host>/gen_snapshot[.exe] + manifest.json
```

Disk rules:
- Debug builds write no dSYM by default. LLDB symbolicates from the debug map in the `.o` files, which worked in M1 [verified-facts].
- `russet clean` removes `.russet/work` and `.russet/build`. `russet cache gc` removes engine and gen_snapshot versions that no project references.
- Every build reports `.russet` size growth in `build-report.json`.
- A Hello build uses about 80 MB of scratch [V2].

### 3.4 Inputs from the SDK area

**`SdkFacts`** (the SDK area produces it; Rust type in section 9.1). The build plane needs:
- SDK path, version and canonical name;
- `SystemVersion.plist` `ProductBuildVersion` (`DTSDKBuild`);
- platform `version.plist` `ProductBuildVersion` (`DTPlatformBuild`);
- the platform `Info.plist` `AdditionalInfo` template;
- the Xcode `version.plist` (`CFBundleShortVersionString`, `ProductBuildVersion`);
- valid and minimum deployment targets (`SDKSettings.plist` `SupportedTargets`);
- the toolchain `usr/lib/swift*` directories, including the back-deploy `swift-5.0/5.5/6.2/iphoneos`;
- paths to swift/swiftc/clang, the linker, librarian, dsymutil and LLVM tools;
- the platform view and toolset paths.

**Requirement on the SDK importer** (Russet's own, W16b; ARCHITECTURE §16.3 lists the full iOS-only want-set): an import must keep:
- `iPhoneOS.platform/version.plist`;
- `Xcode.app/Contents/version.plist` (or the two values from it);
- the platform `Info.plist`;
- `XcodeDefault.xctoolchain/usr/lib/swift-*/iphoneos/*.dylib`;
- the toolchain's `usr/lib/swift/iphoneos/prebuilt-modules` (an `import Foundation` typecheck takes 1.15 s with them and 33.4 s without).

In xtool's bundle these live only in its 6.6 GB `Xcode.app` copy [V11][V20]. If they are missing, the build plane fails with `E_SDK_INCOMPLETE` (`sdk.incomplete`) and names the missing file.

**Platform view.** The view is a Russet-owned directory that mirrors the SDK's `Developer/` layout and adds our tools. It is what `XCODE_EXTRA_PLATFORM_FOLDERS` points at:

```text
<data>/sdk/<id>/view/Developer/
  Toolchains -> <sdk bundle>/Developer/Toolchains                (Swift Build derives SWIFT_RESOURCE_DIR as $(PLATFORM_DIR)/../../Toolchains/...)
  Platforms/iPhoneOS.platform/
    Info.plist -> <sdk>/.../iPhoneOS.platform/Info.plist
    version.plist -> <xcode copy>/.../iPhoneOS.platform/version.plist   (gives PLATFORM_PRODUCT_BUILD_VERSION)
    Developer  -> <sdk>/.../iPhoneOS.platform/Developer
    usr/local/bin/{actool, ibtool, xcstringstool, momc, mapc, metal, metallib, appintentsmetadataprocessor, ...}
```

- **Linux:** symlinks (verified [V8]).
- **Windows:** directory junctions plus copies of the tools as `.exe`; the SDK itself is a Russet import in the short root (§10). Whether Swift Build appends `.exe` when it resolves `actool` on Windows is unverified; see task B17. The swiftbuild backend's view adds a 66-character `Developer\Platforms\iPhoneOS.platform\Developer\SDKs\iPhoneOS27.0.sdk\` prefix to SDK paths of up to 171 characters, so it needs `LongPathsEnabled`; the native backend with the short root stays under 260.
- We never modify Apple files or the xtool bundle. The view is additive.

**Toolsets** that Russet generates per SDK:

```json
// <data>/sdk/<id>/toolset-swb.json   (swift build --build-system swiftbuild --toolset ...)
{
  "schemaVersion": "1.0",
  "rootPath": "<linker bin dir>",
  "linker":    { "path": "ld64.lld" },
  "librarian": { "path": "llvm-libtool-darwin" },   // xtool ships this binary renamed to "llvm-lib" [V1]
  "swiftCompiler": { "extraCLIOptions": ["-Xclang-linker", "-isysroot", "-Xclang-linker", "<.../iPhoneOS27.0.sdk>"] }
}
// toolset-native.json (fallback backend) additionally carries
//   "-Xfrontend", "-enable-cross-import-overlays", "-use-ld=lld"   (as xtool's toolset.json does [V1])
```

**Linker choice and version lock:**
- **Linker:** xtool darwin-tools `ld64.lld` 21.1.6 (patched to accept Apple platforms and `arm64e.x1` TBDs) or LLVM ≥ 23.1.3. The system lld 23.1.1 fails on Xcode 27 TBDs (`unknown target arm64e.x1`) [V10][L§2.4].
- **`ld -r`:** goes through the `russet-tool` `ld64.lld` trampoline. It rewrites `-r` to `llvm-libtool-darwin -static -filelist … -dependency_info … -o …`, as xtool's shell script does [V1], and is also usable on Windows.
- **Rewriters:** `strip` and `install_name_tool` must come from LLVM ≥ 23. LLVM 21 tools can produce LINKEDIT string pools aligned only to 4 bytes, which iOS 27 dyld refuses [L§2.4][sg-08].
- **Lock check:** the SDK's Swift requirement (Xcode 27 → Swift 6.4) must equal the toolchain's minor version, otherwise `E_TOOLCHAIN_MISMATCH` [L§2.3].

---

## 4. Stack (a): SwiftUI / SwiftPM

### 4.1 Backends

| Backend | How | Status | Use |
|---|---|---|---|
| `swiftbuild` (default) | Generated builder package + `swift build --build-system swiftbuild --triple arm64-apple-ios --toolset <toolset-swb.json>` + platform view | Verified on Linux [V2][V5][V8]. Gap: `.dynamic` products [V19] | Default for SwiftPM apps |
| `native` | Same builder + `--build-system native --swift-sdk arm64-apple-ios --toolset <toolset-native.json>` | Verified, including dynamic products and the SDK fix [V19]. **Deprecated** by SwiftPM 6.4 (warning printed) | Automatic fallback when a dependency graph has `.dynamic` products. Early Windows path |
| `xtool` | `xtool dev build` subprocess, then Russet post-processing (Info.plist, lints, IPA) | Works (M1) but: sdk = minos, no DT keys, XTL- IDs, Linux/WSL only | Compatibility and diffing only. Never default |
| `pif` (M7+) | SwiftPM app synthesized as a PIF application target + `russet-swb` | Planned | Option once `russet-swb` exists. Swift Build then does Xcode-identical app bundling |

### 4.2 Builder package and invocation

**Planning** (Rust, equivalent to xtool's `Planner` [S5]). Russet runs, with the build environment:
- `swift package resolve --package-path <pkg>`;
- `swift package show-dependencies --format json -o <tmp>`;
- `swift package describe --type json` for every package in the graph.

From that it picks:
- **The app product:** `[build.swiftpm] product`, or the only library product.
- **Extension products:** from `[[app.extensions]]`.
- **The deployment target:** `[app] deployment_target`, else the package's `.iOS` platform, else 15.0. Values must be in the SDK's `ValidDeploymentTargets` [V11].
- **Resource bundles:** `<Package>_<Target>.bundle` for targets with resources.
- **Binary targets:** a `.framework` is embedded unless the binary is a static archive (`!<arch>` magic, as in xtool).
- **Transitive `.dynamic` products:** if any are present, the build switches to the `native` backend (D1) until the swiftbuild gap is fixed upstream (task B04).

**Generated `<project>/.russet/work/builder/Package.swift`:**

```swift
// swift-tools-version: 6.0
// Generated by Russet. Do not edit.
import PackageDescription
let package = Package(
  name: "RussetBuilder",
  platforms: [.iOS("17.0")],
  dependencies: [.package(name: "App", path: "../../..")],
  targets: [
    .executableTarget(
      name: "Hello-App", dependencies: [.product(name: "Hello", package: "App")], path: "Sources/Hello-App",
      linkerSettings: [.unsafeFlags(["-Xlinker", "-rpath", "-Xlinker", "@executable_path/Frameworks"])]),
    .executableTarget(
      name: "HelloWidget-Extension", dependencies: [.product(name: "HelloWidget", package: "App")],
      path: "Sources/HelloWidget-Extension",
      linkerSettings: [.linkedFramework("Foundation"), .unsafeFlags([
        "-Xlinker", "-e", "-Xlinker", "_NSExtensionMain",
        "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../../Frameworks",
        "-Xlinker", "-rpath", "-Xlinker", "@executable_path/Frameworks"])]),
  ])
// each Sources/<target>/stub.c is an empty file
```

Notes:
- `unsafeFlags` cannot use `$(…)` build-setting references [V19].
- The SDK fix lives in the toolset, not here, so that dependency dylibs are covered too [V6].

**Invocation** (the environment is given explicitly. The global `SDKROOT` must be unset [V4]):

```sh
env -u SDKROOT \
  XCODE_EXTRA_PLATFORM_FOLDERS=<data>/sdk/<id>/view/Developer/Platforms \
  PATH=<linker-bin>:$PATH \
swift build --configuration debug --build-system swiftbuild \
  --package-path <proj>/.russet/work/builder --scratch-path <proj>/.russet/work/swiftpm \
  --triple arm64-apple-ios --toolset <data>/sdk/<id>/toolset-swb.json \
  --disable-automatic-resolution [-j N]
# products: <scratch>/out/Products/<Debug|Release>-iphoneos/{Hello-App, HelloWidget-Extension, *_*.bundle, PackageFrameworks/}
```

**Output handling:**
- Parse `[n / m]` lines into `Progress` events.
- Parse `file:line:col: error|warning|note: msg` into `Diagnostic` events.
- Strip lines matching `does not support linking for platform iOS`, a known benign warning from the patched lld [V19].
- When Swift Build prints a full `env -i …` dump on a failed launch, redact the environment before logging it. It contains secrets from the parent environment [V7].

### 4.3 Link-time SDK version (the `sdk 17.0` bug)

- **Root cause and fix:** section 2, items 1–2.
- **Rules:**
  1. Every swiftc-driven link gets `-Xclang-linker -isysroot -Xclang-linker <SDK>` through the toolset (swiftbuild and native).
  2. Every direct clang link Russet runs (stub `App.framework`, Runner, shims) passes `-isysroot <SDK>` and `-target arm64-apple-ios<min>`.
  3. Lint `L002` checks the sdk field of **every** Mach-O that Russet built. Mismatch is an error in release and a warning in debug.
  4. Foreign prebuilt binaries (vendor xcframeworks, Flutter.framework) are reported, never rewritten.
  5. `gen_snapshot` output uses `--macho-sdk-version` (section 6.6).
- **Compatibility:** once swift-driver ≥ 6.5 ships #2158, the extra `-isysroot` is redundant but harmless, because it has the same value.
- **Behavioural consequence (important):** with `sdk 27.0`, iOS applies iOS 27 SDK linked-on-or-after behaviour. TN3187: "In the next major release following iOS 26, UIScene lifecycle will be required when building with the latest SDK; otherwise, your app won't launch" [S9]. Russet therefore always generates a scene manifest for apps (4.4), and the device area must re-run the M1 demo after this change (acceptance of task B03).

### 4.4 Info.plist synthesis

**Precedence** (later wins). The resolver works on a `plist::Dictionary` and expands build settings in string values (Xcode `INFOPLIST_EXPAND_BUILD_SETTINGS` semantics, including `$(VAR:rfc1034identifier)`, `:c99extidentifier`, `:lower`, `:upper`, `:default=`):
1. **Product-type defaults:**
   - app: `CFBundleInfoDictionaryVersion=6.0`, `CFBundlePackageType=APPL`, `CFBundleDevelopmentRegion=en`, `LSRequiresIPhoneOS=true`, `UIDeviceFamily` from `devices`, `UISupportedInterfaceOrientations{,~ipad}` (the Xcode template defaults), `UILaunchScreen={}`;
   - app scene manifest: `UIApplicationSceneManifest={UIApplicationSupportsMultipleScenes: true, UISceneConfigurations: {}}`. This matches Swift Build's `UIApplicationSceneManifest_Generation` [S6] and is required by D2's consequence. It is skipped only if the user supplies their own manifest;
   - extension: `CFBundlePackageType=XPC!` plus `NSExtension` defaults per `kind` (widgetkit → `NSExtensionPointIdentifier=com.apple.widgetkit-extension`).
2. **`russet.toml [app]`:** names, versions, orientations, `launch_screen`, `icon`/`accent_color` (as asset names).
3. **Stack data:**
   - SwiftPM: the `info_plist` file;
   - Flutter: `ios/Runner/Info.plist` with the `Flutter/Generated.xcconfig` values;
   - Xcode: `INFOPLIST_FILE` plus `INFOPLIST_KEY_*` settings, or Swift Build's processed output.
4. **Partial plists from resource compilers:** actool (`CFBundleIcons`, `CFBundleIcons~ipad`, `NSAccentColorName`, [V14]) and the launch-screen converter.
5. **Enforced keys** (Russet sets them and warns when a user value differs):
   - `CFBundleExecutable`, `CFBundleIdentifier` (effective, 4.5), `MinimumOSVersion` (= deployment target);
   - `CFBundleSupportedPlatforms=[iPhoneOS]`;
   - `UIRequiredDeviceCapabilities` contains `arm64`. Array and dictionary forms are both handled, as Swift Build does [S6];
   - the DT* keys per the metadata policy.
6. **Output:** binary plist. iOS `INFOPLIST_OUTPUT_FORMAT=binary` [V11].

**DT* derivation.** The values are taken from the SDK's own template: platform `Info.plist` → `AdditionalInfo` [V11].

| Key | Template | Source in the SDK | Xcode 27.0 value [V11] | Policy `none` | `minimal` | `sdk` (default) | `xcode-compat` |
|---|---|---|---|---|---|---|---|
| `DTPlatformName` | literal | AdditionalInfo | `iphoneos` | – | ✓ | ✓ | ✓ |
| `DTPlatformVersion` | literal | AdditionalInfo | `27.0` | – | ✓ | ✓ | ✓ |
| `DTSDKName` | `$(SDK_NAME)` | SDKSettings `CanonicalName` | `iphoneos27.0` | – | ✓ | ✓ | ✓ |
| `DTSDKBuild` | `$(SDK_PRODUCT_BUILD_VERSION)` | SDK `System/Library/CoreServices/SystemVersion.plist` [S6] | `24A430` | – | – | ✓ | ✓ |
| `DTPlatformBuild` | `$(PLATFORM_PRODUCT_BUILD_VERSION)` | platform `version.plist` [S6] | `24A430` | – | – | ✓ | ✓ |
| `DTCompiler` | `$(GCC_VERSION)` | `DEFAULT_COMPILER` | `com.apple.compilers.llvm.clang.1_0` | – | – | ✓ | ✓ |
| `DTXcode` | `$(XCODE_VERSION_ACTUAL)` | Xcode `version.plist` `CFBundleShortVersionString` → `MMmp` | `2700` | – | – | ✓ (SDK provenance) | ✓ |
| `DTXcodeBuild` | `$(XCODE_PRODUCT_BUILD_VERSION)` | Xcode `version.plist` `ProductBuildVersion` | `27A266a` | – | – | ✓ (SDK provenance) | ✓ |
| `BuildMachineOSBuild` | `$(MAC_OS_X_PRODUCT_BUILD_VERSION)` | none: the host is not macOS | – | – | – | – | only with explicit `build_machine_os_build = "…"` |

Rules for the table:
- The `sdk` policy writes values that are true of the SDK used. Its two Xcode keys name the Xcode the SDK was imported from.
- `xcode-compat` is opt-in, prints a warning, and is recorded in the build report (risk R4 in the landscape report).
- Swift Build elides `BuildMachineOSBuild`, `DTXcode` and `DTXcodeBuild` when they are empty [S6]. Russet does the same under `none` and `minimal`.
- Whether `DTXcode`/`DTXcodeBuild` should be on by default for App Store uploads is open question Q1.

**DT normalization in nested bundles.**
- **Scope:** every Info.plist in bundles that this build produced: SwiftPM resource bundles, Russet-wrapped frameworks, appex, `App.framework`.
- **Action:** rewrite DT* keys to the app's values (or remove them under `none`). This fixes the `10989`/`99T999`/empty values Swift Build writes on Linux [V9].
- **Excluded:** vendor binaries keep their own keys.

### 4.5 Bundle identifiers

- **Paid team (ASC API key):** `CFBundleIdentifier` = `[app] bundle_id`, exactly. Extensions are `<app id>.<suffix>`. Russet never rewrites them. Registration conflicts ("not available") surface as signing-area errors with a suggestion, never a silent rename.
- **Free account:** `BundleIdPolicy` comes from the signing area:
  - `TeamSuffix`: `<id>.<TEAMID>`. This is the default.
  - `XtoolPrefix`: `XTL-<identity prefix>.<id>`. Use it to update apps already installed by xtool [V12].
  - `Custom`: an explicit map.
- **Application:** at assembly. It covers `CFBundleIdentifier` of the app and every appex (extension IDs must keep the app ID as prefix), plus Info.plist values that reference the app ID (for example `NSExtensionAttributes` fields that name the host).
- **Hand-off:** entitlement rewriting (`application-identifier`, keychain and app groups) is the signing area's job. The build plane hands over a `BundleIdMap {original → effective}`.
- **Validation:** `[A-Za-z0-9.-]+`, no empty components, no leading or trailing dot. An extension ID must start with `<app id>.`.

### 4.6 Bundle assembly

Layout written by `russet-build::bundle` (iOS bundles are shallow; no symlinks are created):

```text
<App>.app/
  <App>                       executable (from <Products>/<App>-App), arm64 only
  Info.plist                  binary
  PkgInfo                     "APPL????" (what Xcode writes)
  Assets.car, AppIcon60x60@2x.png, AppIcon76x76@2x~ipad.png       (main-bundle catalog via russet-actool)
  <Pkg>_<Target>.bundle/      SwiftPM resource bundles (DT-normalized)
  PrivacyInfo.xcprivacy       if provided (app and per framework)
  <lang>.lproj/…              compiled strings
  Frameworks/<X>.framework/   dynamic frameworks (package binary targets, wrapped dylibs, Flutter/App frameworks)
  Frameworks/libswift*.dylib  back-deployment Swift libs (only if referenced)
  PlugIns/<Ext>.appex/        extensions (each: executable, Info.plist, resources)
```

Steps and rules:
1. **Executable:** copy and thin to arm64 with LLVM ≥ 23 `llvm-lipo`. Strip in release (`llvm-strip -x`, LLVM ≥ 23) after the dSYM step (`dsymutil` from the toolset).
2. **Loose dylibs** (`lib<X>.dylib`, produced when a `.dynamic` product is built) are wrapped into `Frameworks/<X>.framework/<X>`:
   - write an Info.plist (`CFBundlePackageType=FMWK`);
   - set the install name `@rpath/<X>.framework/<X>`: at link time when Russet controls the link, otherwise with `russet-macho` (an id rewrite plus the dependents' `LC_LOAD_DYLIB`; ASSUMPTION that this keeps LINKEDIT alignment; verified in tests by lint `L005`).
   - The App Store rejects loose non-Swift dylibs [L§2.6 sg-30].
3. **Swift back-deployment:**
   - for each `@rpath/libswift*.dylib` load command in any embedded Mach-O, copy the matching dylib from `SdkFacts.back_deploy_dirs` into `Frameworks/`, thinned to arm64;
   - for store IPAs, also copy the **unmodified** original into `SwiftSupport/iphoneos/` (section 4.8);
   - if a dylib is missing (slim SDK), fail with `E_SDK_INCOMPLETE`.
   - Not needed for deployment ≥ 26.0 (Span) or ≥ 15.0 (Concurrency), per `SwiftCompatibilityLibraryNotNeededStartingIn` [V11].
4. **rpath hygiene:**
   - delete `LC_RPATH` entries that are absolute host paths (anything not under `/usr/lib`, `/System` and not starting with `@`), such as `/home/…/PackageFrameworks` and `/usr/lib/swift/usr/lib/swift-6.2/iphoneos` [V18];
   - keep `/usr/lib/swift`, `@executable_path/Frameworks` and `@loader_path/Frameworks`;
   - done in place by `russet-macho` before signing, then re-linted.
5. **Entitlements:** collected per bundle (`[app] entitlements`, extension entitlements, Xcode `CODE_SIGN_ENTITLEMENTS`). Debug profiles add `get-task-allow`. They are passed through the `SigningPlan`; nothing is embedded here.
6. **Extensions:**
   - `CFBundleShortVersionString` and `CFBundleVersion` are forced equal to the app's (App Store rule [L§2.6]);
   - the appex executable is linked with `-e _NSExtensionMain` (builder template);
   - frameworks are shared through the `@executable_path/../../Frameworks` rpath.
7. **Determinism:** fixed mtimes (`SOURCE_DATE_EPOCH`, else 2001-01-01) and sorted directory walks. Two identical builds give byte-identical bundles before signing (test T-BUNDLE-03).

### 4.7 Mach-O and bundle lints (`russet-macho`, `russet-build::lint`)

| ID | Check | Severity (debug / release) | Auto-fix |
|---|---|---|---|
| L001 | `LC_BUILD_VERSION` platform = iOS (2) | error / error | – |
| L002 | `sdk` = SDK version for binaries Russet built | warn / error | – (fix is at link time) |
| L003 | `minos` of each embedded binary ≤ app `MinimumOSVersion` | error / error | – |
| L004 | arm64 only. No x86_64/simulator slices; arm64e only if the toolchain supports it (llvm #188378 open [L§2.4]) | warn / error | thin with `llvm-lipo` |
| L005 | LINKEDIT string pool and symbol table 8-byte aligned (iOS 27 dyld rule [L§2.4]) | error / error | – |
| L006 | no absolute host `LC_RPATH` | warn / error | delete (4.6) |
| L007 | embedded dylib install names are `@rpath/…` and resolvable inside the bundle | error / error | – |
| L008 | no loose non-Swift dylibs in `Frameworks/` | warn / error | wrap (4.6) |
| L009 | `LC_ENCRYPTION_INFO_64` present on the app executable (lld does this [V21]) | info / warn | – |
| L010 | Info.plist: required keys, `UIRequiredDeviceCapabilities` contains `arm64`, extension versions equal the app's, `CFBundleIconName` present when an icon set is configured (ITMS-90713 [L§1.3]) | warn / error | – |
| L011 | DT* consistent across all Russet-built bundles | warn / error | normalize |
| L012 | resource name collisions that differ only in case (they break Windows checkouts) | warn / warn | – |

### 4.8 IPA packaging (`russet-ipa`)

- **Layout:**
  - `Payload/<App>.app/...`;
  - store IPAs add `SwiftSupport/iphoneos/libswift*.dylib`, the Apple-signed originals. They are present only when the app embeds Swift runtime dylibs, and their absence then triggers "Invalid Swift Support".
  - No `Symbols/`: it needs Apple's closed `symbols` tool, and there is no dSYM upload API [L§2.6].
- **Writer:** the `zip` crate (MIT).
  - Entries are sorted, with explicit directory entries.
  - Timestamps are fixed (`SOURCE_DATE_EPOCH`, DOS minimum 1980-01-01).
  - Unix external attributes: directories `040755`; Mach-O executables and dylibs `100755` (detected by Mach-O magic, so it also works on Windows, where the filesystem has no exec bit); other files `100644`; symlinks `120777` with the target as data (iOS bundles normally have none).
  - Zip64 is used automatically above 4 GiB. Data descriptors are not used: sizes go in local headers.
- **Modes:**
  - `--ipa dev`: stored (no compression) for speed.
  - `--ipa store` (default for `ship`): deflate level 6.
- **API:** `russet_ipa::write(app: &Utf8Path, out: &Utf8Path, opts: IpaOptions) -> Result<IpaReport>` and `read_listing(ipa) -> Vec<Entry>` (for tests and `russet inspect`).
- The `zip`/`unzip` binaries are never invoked.

### 4.9 Main-bundle resources for SwiftPM apps

- **SwiftPM semantics:** resources in package targets compile into `<Pkg>_<Target>.bundle` (Swift Build runs actool and generates asset symbols there [V8]).
- **App icon:** must sit in the **main** bundle's `Assets.car` together with `CFBundleIcons`/`CFBundleIconName`. Russet therefore compiles `[app] resources` (catalogs, xcstrings, fonts, plain files) into the main bundle with `russet-actool --app-icon <icon> --accent-color <color>` and merges the partial plist.
- **Dev fallback:** while the CAR writer is not available, a single PNG `icon` uses xtool's loose `CFBundleIconFile` approach. It is enough for development installs [L§2.5].

---

## 5. Stack-independent resource compilers (c)

### 5.1 How tools are plugged in

- Swift Build finds `actool`, `ibtool` and `xcstringstool` through the platform view's `usr/local/bin` [S4][V8].
- `russet-swb` builds (M7) also set `ASSETCATALOG_EXEC`/`IBC_EXEC` explicitly.
- Tasks run with an **empty environment** [V8]. Our tools therefore must not depend on environment variables to find their data: they locate siblings from `argv[0]`/`current_exe()`.

### 5.2 `russet-actool`: contract (subset of actool 27.0)

**Invocations Swift Build issues on Linux** (captured [V8]):

```text
# 1) asset-symbol generation (per target with a catalog)
actool <X.xcassets> --compile <Products/Debug-iphoneos> --output-format human-readable-text --notices --warnings \
  --export-dependency-info <T.build>/assetcatalog_dependencies \
  --output-partial-info-plist <T.build>/assetcatalog_generated_info.plist \
  --compress-pngs --enable-on-demand-resources NO --development-region en \
  --target-device iphone --target-device ipad --minimum-deployment-target 17.0 --platform iphoneos \
  --bundle-identifier <id> --generate-swift-asset-symbol-extensions NO \
  --generate-swift-asset-symbols <DerivedSources>/GeneratedAssetSymbols.swift \
  --generate-objc-asset-symbols <DerivedSources>/GeneratedAssetSymbols.h \
  --generate-asset-symbol-index <DerivedSources>/GeneratedAssetSymbols-Index.plist
# 2) thinned compile into the resource bundle
actool <X.xcassets> --compile <B.build>/assetcatalog_output/thinned --output-format human-readable-text --notices --warnings \
  --export-dependency-info <B.build>/assetcatalog_dependencies_thinned \
  --output-partial-info-plist <B.build>/assetcatalog_generated_info.plist_thinned \
  --compress-pngs --enable-on-demand-resources NO --development-region en \
  --target-device iphone --target-device ipad --minimum-deployment-target 17.0 --platform iphoneos
# planning-time probe (full environment): actool --version --output-format xml1
```

**Flags.** We accept every flag in `AssetCatalogCompiler.xcspec` [S8], so that unknown-flag errors never break a build. These are implemented (rest: accepted and ignored with a notice, or rejected with a clear error):
- `--compile`, `--platform`, `--minimum-deployment-target`, `--target-device`;
- `--app-icon`, `--alternate-app-icon`, `--include-all-app-icons`, `--accent-color`, `--widget-background-color`, `--launch-image`;
- `--output-partial-info-plist`, `--export-dependency-info`;
- `--output-format {human-readable-text,xml1}`, `--notices`, `--warnings`;
- `--compress-pngs` (ignored: our PNG output is already optimal), `--enable-on-demand-resources NO|YES` (ODR not supported: YES is accepted only with no tagged assets), `--development-region`;
- `--bundle-identifier`, `--generate-swift-asset-symbols`, `--generate-objc-asset-symbols`, `--generate-asset-symbol-index`, `--generate-swift-asset-symbol-extensions`, `--generate-asset-symbol-framework-support`, `--generate-asset-symbol-warnings`, `--generate-asset-symbol-errors`, `--generate-asset-symbol-backwards-deployment-support`;
- `--print-asset-tag-combinations` (returns an empty set);
- `--filter-for-*` (thinning, accepted; no-op in phase A);
- `--standalone-icon-behavior`, `--include-partial-info-plist-localizations`, `--optimization`, `--lightweight-asset-runtime-mode`.

Sticker-pack flags (`--sticker-pack-*`) are rejected.

**Behaviour to match** (golden facts from Xcode 27.0 actool 25098 [V13]–[V15]):

| Situation | actool 27.0 behaviour (oracle) | russet-actool |
|---|---|---|
| `--version --output-format xml1` | `{com.apple.actool.version: {bundle-version: "25098", short-bundle-version: "27.0"}}` | Same shape. `short-bundle-version` = the CoreUI/Xcode level we emulate; `bundle-version` = our numeric version. Adds `org.russet.actool.version`. Swift Build parses the value; no version gating was found in the sources reviewed [S8], and a stand-in reporting `24505` worked [V8] |
| any `--generate-*-asset-symbol*` flag | writes **only** the three symbol files. No `Assets.car`, no partial plist, does not create the `--compile` dir. stdout `/* com.apple.actool.compilation-results */` + one path per line | Same |
| compile mode, output dir missing | `<catalog>: error: The output directory "<dir>" does not exist.` | Same message and format |
| single-size 1024 universal icon, iphone+ipad, min 17.0 | `Assets.car` + `AppIcon60x60@2x.png` (120 px) + `AppIcon76x76@2x~ipad.png` (152 px). Partial plist: `CFBundleIcons.CFBundlePrimaryIcon={CFBundleIconFiles:[AppIcon60x60], CFBundleIconName:AppIcon}`, `CFBundleIcons~ipad` (files `AppIcon60x60`, `AppIcon76x76`), `NSAccentColorName` | Same file names, sizes and keys. Also verified for min 15.0. With iPhone-only targets, the `~ipad` dict has no `CFBundleIconName` |
| dependency info | ld64 dependency-info format: `0x00 "actool-25098"`, `0x10 <input catalog dir>`, `0x40 <each output>` | Same opcodes. Version string `russet-actool-<v>` |
| `--output-format xml1` results | `{com.apple.actool.compilation-results: {output-files: [...]}}`. Errors as `{com.apple.actool.errors: [{description: …}]}`, also for unknown args | Same |
| human-readable diagnostics | `/* com.apple.actool.errors */` header, then `<path>: error: <msg>` | Same (Swift Build parses `^([^:]*):([^:]*): error: (.*)$` [S8]) |

**CAR writer (`russet-car`).**
- **Format reference:** AssetKit's `docs/coreui-970-format.md` and code, MIT [S10]. Porting with attribution is allowed.
- **Header:**
  - Key format as Xcode 27 writes it: appearance, localization, scale, idiom, subtype, dimension2, identifier, element, part [V15].
  - AssetKit's CoreUI 970 layout first (validated by `assetutil` in AssetKit's CI). Xcode 27 writes CoreUIVersion 1010 / StorageVersion 17 / SchemaVersion 2 [V15]. Corpus diffs decide whether to emit the 1010 structures (task B10).
  - `Timestamp` = `SOURCE_DATE_EPOCH` (deterministic).
  - The header strings (`AssetStorageVersion`, authoring tool, main version) say "Russet" unless `metadata = "xcode-compat"`. Xcode writes "Xcode 27.0 (27A266a) via AssetCatalogAgent-AssetRuntime" [V15], so it falls under the same policy as the DT keys.
- **Renditions in phase B:**
  - colors (sRGB, Display P3; any/dark);
  - PNG/JPEG images at 1x/2x/3x;
  - SVG (rasterized with `resvg` plus the preserved vector, as AssetKit does);
  - app icons: per-size sets and single-size 1024 expansion; any/dark/tinted as a later step; "MultiSized Image" index renditions as Xcode emits [V15];
  - data sets.
- **Compression:** LZFSE through the `lzfse` crate (reference C implementation). Xcode also uses `deepmap2` for some images [V15]. Phase B does not need it.
- **Not supported, with a clear error:** PDF vector images (planned through a PDF rasterizer crate, open choice), `.icon` Icon Composer files, stickers, ODR, AR resources, textures.
- **Symbol generation:** `GeneratedAssetSymbols.swift`/`.h`/`-Index.plist` in Xcode 27's exact format [V13]:
  - `DeveloperToolsSupport.ColorResource`/`ImageResource` extensions;
  - names derived as `AccentColor` → `accent` (the "Color" suffix is dropped) and `Logo` → `logo`;
  - `#if SWIFT_PACKAGE` `Bundle.module` switch;
  - index keys `catalogPath`, `objcSymbol` (`ACColorName…`, `ACImageName…`), `relativePath`, `swiftSymbol`.

**Why Rust over wrapping AssetKit:**
1. A single static binary on Linux and Windows, with no swift-corelibs-foundation runtime and no `rsvg-convert` dependency.
2. The IDE asset-catalog editor and app-icon generator call the same code in-process through the daemon.
3. We own the critical path (risk R7: AssetKit has 1 star, a single maintainer, last push 2026-05-25 [S10]).

AssetKit stays in CI as a **differential oracle**: compile the same catalog with both, parse both with our reader and with `ipsw car --json` (Linux), and compare rendition sets.

**Phasing:**
- **Phase A** (needed from the first build, because Swift Build calls actool for any package with `.xcassets`):
  - a "loose" stand-in writes imagesets as `name@Nx.png` files, app icons as loose PNGs plus `CFBundleIconFiles`, and colors only as symbols;
  - it writes a valid empty `Assets.car`, generates symbols, and emits the partial plist and dependency info;
  - it is good for development installs, not for the App Store.
- **Phase B:** the full CAR writer.

### 5.3 `russet-xcstrings` (`xcstringstool` compatible)

- **Contract** (Xcode 27 help [V16] and `XCStrings.xcspec` [S8]):
  - `compile <in.xcstrings> -o|--output-directory <dir> [-l <lang>]... [-f stringsAndStringsdict|stringsdictOnly] [--serialization-format text|binary] [--dry-run]`;
  - `generate-symbols <in> -o <dir> -l swift|objc`;
  - `print`, `sync`, `extract` and `installloc` are rejected with "not supported".
  - Swift Build runs `compile --dry-run` first (the flag goes in position 2) to learn the output paths. Each path goes on its own line.
- **Outputs:**
  - `<lang>.lproj/<Table>.strings` (binary plist when `--serialization-format binary`; iOS `STRINGS_FILE_OUTPUT_ENCODING=binary` [V11]);
  - `<lang>.lproj/<Table>.stringsdict` for plural, device and substitution variations;
  - the symbols file, in the format taken from the golden corpus.
- **Starting point:** algoriddim/xcstringstool (Rust, MIT, about 14 KB, no stringsdict) [S11]. We add stringsdict, symbols and dry-run, and offer the changes upstream.

### 5.4 Storyboards and XIBs (`russet-ib` and the `ibtool` shim)

> **Updated 2026-10-11 (review rounds 1 and 2, ARCHITECTURE [D 75], [D 90]):** storyboards and XIBs are compiled natively (B21a in W21b, B21b in W28b, B21c in W28c). B21a also covers a single-scene main storyboard whose view controller has an ObjC custom class (Flutter's `Main.storyboard`). Conversion to `UILaunchScreen` stays for the cases where it is lossless.

**What UIKit loads** (measured on four Apple-built Catalyst apps on the Mac oracle, macOS 27.0.1; iOS output is confirmed by the oracle corpus of §5.5):
- `<Name>.storyboardc/` holds a binary `Info.plist` (`UIStoryboardDesignatedEntryPointIdentifier = "UIViewController-<vcID>"`, `UIStoryboardVersion = 1`, `UIViewControllerIdentifiersToNibNames`) and flat nibs: `UIViewController-<vcID>.nib` and `<vcID>-view-<viewID>.nib`.
- **NIBArchive:** header `NIBArchive`, format 1, coder version 10; sections objects, keys, values, classes; the iOS trailer `LNE\0` after the class table, paired with the root key `IBUINibLNEVersionKey = 1`; the extra int on a class entry is a fallback-class index (UIColor → NSColor, `_UILayoutGuide` → UIView, `_UILayoutSupportConstraint` → NSLayoutConstraint); NSArrays are `NSInlinedValue = false` followed by repeated `UINibEncoderEmptyKey` entries; content-hugging priority is the string `"{251, 251}"`.
- Output depends on `--minimum-deployment-target` (scene view 393×852 for 13.0–16.6, 1000×1000 for 17.0 and later), and Swift classes with `customModuleProvider=target` are written as `_TtC<len><Module><len><Class>` using `--module`. Byte identity also needs the CF `__NSSetM` key order. UIKit looks values up by key, so semantic equality of the decoded object graph plus on-device load harnesses is the acceptance bar; byte identity is a reported metric.
- Swift Build compiles storyboards with `ibtool … --compilation-directory <TempResourcesDir> X.storyboard` and then links with `ibtool … --link <ProductResourcesDir> X.storyboardc`; XIBs use `--compile <dir>/X.nib`. Flags: `--target-device`, `--minimum-deployment-target`, `--module`, `--output-partial-info-plist`, `--auto-activate-custom-fonts`, `--cocoatouch-compiler-mode`. The tool is probed with `--version --output-format xml1`.

1. **Compile.** `russet-tool ibtool` implements that contract on the `russet-ib` compiler: B21a launch storyboards and single-scene main storyboards with an ObjC custom class (W21b); B21b main storyboards and XIBs (W28b); B21c tables, collections, split views, traits and accessibility configurations (W28c; §12).
2. **Launch storyboards → `UILaunchScreen`** (`russet ib convert-launch-screen <storyboard> [--catalog <xcassets>]`), used by `launch-storyboard = "auto"` when the storyboard is not yet compilable and the conversion is lossless.
   - **Supported pattern:** a root view with a background color (named, system or sRGB literal) and at most one image view with a named image, with centering, edge or safe-area constraints.
   - **Output:** `UILaunchScreen = {UIColorName, UIImageName, UIImageRespectsSafeAreaInsets}`. A literal color becomes a generated colorset `RussetLaunchBackground` in the main catalog.
   - **Fidelity:** lossless for the Xcode template (system background) and Flutter (one centred image on white, `UIImageRespectsSafeAreaInsets = NO`); **lossy** for React Native (two UILabels) and for flutter_native_splash's background and branding images. The build report lists every lossy conversion with its reason.
   - **Anything else:** `assets.storyboardUnsupported` (4101) with file:line in release, unless `[build] launch-screen-fallback = "empty"`; a warning and `UILaunchScreen = {}` in debug.
3. **`Main.storyboard`/`UIMainStoryboardFile`/`UISceneStoryboardFile`.** B21a compiles a single-scene main storyboard whose view controller has an ObjC `customClass` without module mangling (Flutter's Runner `Main.storyboard` with `FlutterViewController`); everything else is compiled once B21b supports the storyboard. Until then, for known templates (Flutter Runner, RN `AppDelegate`, a UIKit template with a single root view controller), Russet removes the storyboard keys and installs a generated scene-delegate subclass that builds the window in code (Flutter version in section 6.4); this stays as a fallback.
4. **Codec and prior art:** michaelwright235/nibarchive (Rust, **MIT OR Apache-2.0**, no trailer support) [S14]; omarchy-apple-dev `tools/ibtool`, `nibarchive.py` and `keyorder.py` (MIT; ported with attribution; its committed Apple goldens are never vendored) [S23]; microsoft/WinObjC `xib2nib` (MIT; about 50 UIKit classes) [S24]; tokenbleed/nibkit (Go, MIT; an independent differential decoder) [S25]; MatrixEditor/nibarchive (GPL-3.0; reference only) [S26].
5. **Other closed tools** (`momc`, `mapc`, `metal`, `metallib`, `appintentsmetadataprocessor`, `coremlc`, `aimodelc`, `realitytool`, `scntool`, `TextureAtlas`, `copypng`): shims exit with `E_RESOURCE_UNSUPPORTED`, naming the input and the workaround from [L§2.5]. Examples: build `NSManagedObjectModel` in code; use runtime `MLModel.compileModel`; Metal through Apple's Windows tools (later). App Intents get a warning, not a failure: the app builds, but Shortcuts metadata is missing.

### 5.5 Golden corpus from the Mac oracle (development only)

- **Repository:** `russet-corpus`, private. It contains only inputs we authored and outputs Apple's tools produced from them. No SDK files or Apple binaries go in.

  ```text
  cases/<tool>/<case-id>/input/...  args.json  oracle/<xcode-build>/{stdout,stderr,exit_code,files/...}  normalized/*.json
  ```
- **Runner:** `russet oracle run --tool actool --case <id>`.
  - It packs the input, runs `ssh mac` with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` in a `mktemp -d` directory with an **EXIT trap that deletes it**, and pulls the outputs back.
  - That temp directory is the only thing it ever deletes on the Mac. The decision log (2026-10-11) says to ask the owner before deleting anything else there.
  - It also runs `assetutil --info` and `xcstringstool print`, which produces JSON.
  - It runs each case twice to detect nondeterminism (for example the `Timestamp` field).
- **Normalizers** remove paths and timestamps. Comparators diff our `russet-car dump --json` against `assetutil` JSON on rendition sets and attributes. Byte equality is not a goal.
- **Mac load:** each oracle call does at most one tool invocation per case, sequentially. The oracle is never used by end-user builds.
- **Initial cases:**
  - colorset (any/dark, P3);
  - imageset 1x/2x/3x;
  - single-size icon for min 15.0 and 17.0, iPhone-only and universal;
  - per-size icon set (the Flutter template);
  - symbols mode;
  - xcstrings with plurals and with device variations;
  - launch storyboards for the converter and the compiler, and `ibtool` cases (B21a): deployment targets 15.0, 17.0 and 26.0; devices iphone and ipad; `--module`; inputs Flutter LaunchScreen and Main, the RN LaunchScreen, flutter_native_splash variants, NetNewsWire LaunchScreenPhone/Pad and Russet-authored equivalents of the Xcode templates (Xcode's own template storyboards are Apple files). The comparator decodes NIBs into a canonical object graph: objects reached from the root, keys sorted per object, exact float bits, the class fallback table and a trailer flag.

---

## 6. Stack (b): Flutter

Pins (from the research): Flutter 3.47.7 stable, engine `deb287481e3c…`, Dart 3.13.5 = `04bcd1036cdc…` [L§App A; research copies of `bin/internal/engine.version` and `DEPS`].

### 6.1 Engine artifacts

- **Primary:** `flutter precache --ios`. It downloads `ios/`, `ios-profile/` and `ios-release/artifacts.zip` into `$FLUTTER_ROOT/bin/cache/artifacts/engine/` on Linux [L§4.2 fx-04].
- **Fallback:** download directly from `https://storage.googleapis.com/flutter_infra_release/flutter/<engine>/<ios|ios-profile|ios-release>/artifacts.zip` [S17] into `<cache>/flutter/engine/<engine>/`, unzipped in Rust.
- **Used from the artifacts:** `Flutter.xcframework/ios-arm64/Flutter.framework` (debug or release flavor).
- **Never used:** the macOS `gen_snapshot_arm64` inside `ios-release/`.

### 6.2 Shims Flutter needs on Linux

`flutter assemble`'s iOS targets call `xcrun clang`, `xcrun dsymutil`, `xcrun strip` and plain `codesign` [S15]. Russet prepends `<data>/tools/<v>/bin` to `PATH`. There, `russet-tool` provides:
- **`xcrun`:**
  - `--sdk iphoneos --show-sdk-path|--show-sdk-version|--show-sdk-build-version|--show-sdk-platform-path`;
  - `--find <tool>`;
  - `clang`/`clang++` → Swift toolchain clang, adding `-isysroot <SDK>` and a default `-target arm64-apple-ios<min>` derived from `-m*-version-min`/`-arch` when missing, as Apple's driver does;
  - `ld`, `libtool`, `lipo`, `strip`, `dsymutil`, `otool`, `install_name_tool` → toolset or LLVM ≥ 23;
  - `actool`/`ibtool`/`xcstringstool` → our tools.
- **`codesign`:** a no-op that exits 0 and logs. The signing area signs the final bundle.

xlinux demonstrates the same arrangement ([S20] `support/core/bin/xcrun`). It is ASSUMPTION-level for Flutter 3.47.7 until task B13 verifies it.

### 6.3 Custom-device adapter

**Registration:** `russet flutter register [--device <udid>]`.
1. Run `flutter config --enable-custom-devices`.
2. Upsert into Flutter's custom-devices config. Prefer `flutter custom-devices add` non-interactively if it accepts JSON (ASSUMPTION). Otherwise edit the file: Linux `$XDG_CONFIG_HOME/flutter/custom_devices.json` [S20]. The Windows path is to be confirmed (task B13).
3. One entry per paired iPhone. Re-running updates in place.

```json
{
  "id": "russet-00008101",
  "label": "iPhone 12 (Russet)",
  "sdkNameAndVersion": "iOS 27.0 (Russet)",
  "platform": "linux-x64",
  "enabled": true,
  "ping": ["russet", "flutter", "device", "ping", "--udid", "<udid>"],
  "pingSuccessRegex": "^ok$",
  "postBuild": null,
  "install": ["russet", "flutter", "device", "install", "--udid", "<udid>", "--app", "${appName}", "--bundle", "${localPath}"],
  "uninstall": ["russet", "flutter", "device", "uninstall", "--udid", "<udid>", "--app", "${appName}"],
  "runDebug": ["russet", "flutter", "device", "run-debug", "--udid", "<udid>", "--app", "${appName}",
               "--mode", "${buildMode}", "--engine-options=${engineOptions}"],
  "forwardPort": ["russet", "flutter", "device", "forward", "--udid", "<udid>", "--device-port", "${devicePort}", "--host-port", "${hostPort}"],
  "forwardPortSuccessRegex": "^forwarding ",
  "screenshot": ["russet", "flutter", "device", "screenshot", "--udid", "<udid>", "--base64"]
}
```

Why `linux-x64`:
- custom devices accept only `linux-x64`/`linux-arm64` [S16];
- debug kernels are platform-neutral, and plugin registration picks iOS at runtime [L§4.2];
- the Linux bundle Flutter builds is ignored, except that its kernel is reused (below).

Command split:
- **`install`:** the build plane builds `Runner.app` (debug, 6.5). It reuses the kernel `flutter run` just compiled, `.dart_tool/flutter_build/<.last_build_id>/app.dill`, which carries that run's `--dart-define`s (xlinux technique [S20]). It then hands the app to the device area to sign and install.
- **The device area** implements `ping`, `uninstall`, `run-debug` (launch suspended, JIT helper, print the VM-service line Flutter discovers), `forward` and `screenshot`.
- **Flavor:** recovered from `--flavor` in the parent `flutter` process arguments (`/proc` on Linux, as xlinux does). On Windows there is no `/proc`: use `--flavor` in the `[build.flutter]` config.

### 6.4 Storyboard-free UIScene Runner

Generated `RussetSceneDelegate.swift`, compiled into the Runner. It subclasses the project's `SceneDelegate` (the current template: `class SceneDelegate: FlutterSceneDelegate` [S18]):

```swift
// Generated by Russet. Do not edit.
import Flutter
import UIKit
class RussetSceneDelegate: SceneDelegate {
  override func scene(_ scene: UIScene, willConnectTo session: UISceneSession,
                      options connectionOptions: UIScene.ConnectionOptions) {
    if window == nil, let windowScene = scene as? UIWindowScene {
      let window = UIWindow(windowScene: windowScene)
      window.rootViewController = FlutterViewController(project: nil, nibName: nil, bundle: nil)
      self.window = window
      window.makeKeyAndVisible()
    }
    super.scene(scene, willConnectTo: session, options: connectionOptions)
  }
}
```

- The template's `AppDelegate` (`FlutterImplicitEngineDelegate` → `GeneratedPluginRegistrant.register`) is left untouched [S18].
- For older projects that still have no `SceneDelegate.swift`, Russet generates a `SceneDelegate: FlutterSceneDelegate` too.

Info.plist rewrite for the Runner:
- `UISceneDelegateClassName` = `$(PRODUCT_MODULE_NAME).RussetSceneDelegate`;
- delete `UISceneStoryboardFile` and `UIMainStoryboardFile`;
- replace `UILaunchStoryboardName` with the converter's `UILaunchScreen` (5.4).
- Debug only: add `NSBonjourServices += _dartVmService._tcp` and `NSLocalNetworkUsageDescription` [L§4.2].

### 6.5 Debug build

1. Run `flutter pub get`.
2. Run `flutter assemble` exactly as `xcode_backend.dart` does [S15][S20], with the shims on `PATH`:

   ```sh
   flutter --no-version-check assemble --output=<work>/flutter/assemble/ \
     -dTargetPlatform=ios -dIosArchs=arm64 -dSdkRoot=<SDK> -dBuildMode=debug -dConfiguration=Debug \
     -dTargetFile=lib/main.dart -dTrackWidgetCreation=true -dTreeShakeIcons=false -dDartObfuscation=false \
     -dSplitDebugInfo= -dAction=build -dSrcRoot=<proj>/ios --DartDefines=<base64(k=v),...> [-dFlavor=<f>] \
     debug_ios_bundle_flutter_assets
   ```

   This produces:
   - `App.framework/App`: the stub dylib Flutter builds with `static const int Moo = 88;` via `xcrun clang -dynamiclib … -install_name @rpath/App.framework/App -isysroot <SDK>` [S15]. Through our clang shim it gets `sdk 27.0`.
   - `App.framework/flutter_assets/{kernel_blob.bin, vm_snapshot_data, isolate_snapshot_data, …}`.
   - `native_assets/*.framework`.
3. Swap in the `flutter run` kernel when present (6.3).
4. **Runner:**
   - Without iOS plugins: compile `ios/Runner/*.swift`, `*.m` and `RussetSceneDelegate.swift` with swiftc/clang directly. Use `-import-objc-header Runner-Bridging-Header.h`, `-F <Flutter.xcframework/ios-arm64>`, `-framework Flutter`, `-Xlinker -rpath -Xlinker @executable_path/Frameworks`, `-Xclang-linker -isysroot -Xclang-linker <SDK>`.
   - With plugins: section 6.7.
5. **Assemble `Runner.app`:**
   - `Frameworks/{Flutter.framework (debug flavor, without Headers/Modules/_CodeSignature), App.framework (Info.plist from ios/Flutter/AppFrameworkInfo.plist with MinimumOSVersion), native asset frameworks, plugin frameworks}`;
   - `Assets.car` from `ios/Runner/Assets.xcassets` via `russet-actool --app-icon AppIcon` (the template's 19-entry set fits the supported subset [L§4.4]);
   - plugin resource bundles; Info.plist (6.4 and 4.4); entitlements with `get-task-allow`.
6. Lint, then hand off to the signing and device areas.

### 6.6 Release and profile (AOT)

Same as 6.5 with `-dBuildMode=release|profile` and target `release_ios_bundle_flutter_assets` (`profile_…`).

**gen_snapshot.** flutter_tools calls `$FLUTTER_ROOT/bin/cache/artifacts/engine/ios-release/gen_snapshot_arm64`, a macOS binary [S15]. Russet installs a **wrapper** at that path (and in `ios-profile/`):
- It is marked by a `.russet` file next to it. `russet doctor` detects when `flutter precache --force` has restored the original.
- It executes our CI-built `gen_snapshot` for the engine hash (6.8) and appends `--macho-sdk-version=<SDK version>`.
  - flutter_tools does not pass that flag yet (PR #191369 open [L§4.5]).
  - Our build backports the flag (Dart c8495e3003 [S12]).
  - Result: `App.framework/App` has `sdk 27.0` with no post-patch.
- If the wrapper cannot be installed (read-only Flutter SDK), Russet runs the documented AOT steps itself:
  1. `frontend_server --aot --tfa --target-os ios -Ddart.vm.product=true`;
  2. `gen_snapshot --deterministic --snapshot_kind=app-aot-macho-dylib --macho=App.framework/App --macho-object=app.o --macho-min-os-version=<min> --macho-rpath=@executable_path/Frameworks,@loader_path/Frameworks --macho-install-name=@rpath/App.framework/App --macho-sdk-version=<sdk> <app.dill>` [S15];
  3. `dsymutil -o App.framework.dSYM`;
  4. `llvm-strip -x` (LLVM ≥ 23).

**Version coupling:** a gen_snapshot from a different Dart revision fails with "Invalid SDK hash" [L§4.5 fx-51]. The cache key is therefore the **engine hash**. Flutter 3.47.6 and 3.47.7 share Dart `04bcd10` but have different engine hashes, so each gets its own entry.

**Distribution build** additionally needs: a full `Assets.car` with `CFBundleIconName` (phase B actool), store signing (signing area), and a store IPA (4.8).

### 6.7 Plugins via SwiftPM

Generated under `<work>/flutter/Packages/`, mirroring flutter_tools' `swift_package_manager.dart` [S19]:
- **`.packages/FlutterFramework/`:** an empty library target that plugins depend on. `Flutter.framework` itself comes through `-F`.
- **`.packages/<plugin>`:** a link (symlink on Linux, junction on Windows) to `<plugin>/ios/<plugin>` or `<plugin>/darwin/<plugin>`. The list comes from `.flutter-plugins-dependencies` (`plugins.ios[]`).
- **`FlutterGeneratedPluginSwiftPackage/Package.swift`:** a **static** product depending on all plugins and `FlutterFramework`. Flutter: "must be statically linked to ensure any dynamic dependencies are linked to Runner and prevent undefined symbols" [S19].
- **Builder package:**
  - an executable target `Runner` with the Runner Swift sources plus `RussetSceneDelegate.swift` plus `RussetRunnerExports.swift` (`@_exported import RunnerObjC`, ASSUMPTION: makes `GeneratedPluginRegistrant` visible module-wide; fallback: `-import-objc-header` through `unsafeFlags`);
  - a C-family target `RunnerObjC` (`GeneratedPluginRegistrant.m/.h`, other `.m` files);
  - dependency on `FlutterGeneratedPluginSwiftPackage`;
  - built with the same command line as 4.2, with `-F <Flutter.xcframework/ios-arm64>` added via unsafe flags.
- **Embedding:** resource bundles and binary-target frameworks (Firebase-style xcframeworks) are embedded as in 4.6.

### 6.8 `gen_snapshot` CI job (linux-x64, windows-x64 → ios-arm64)

- **Where:** a public repository `evilcomputer12/russet-gen-snapshot` (BSD-3 artifacts; public so macOS runners are free; see Q4). The recipe is ours.
- **Facts it uses:** the iOSBuildX recipe (MIT) [S21] and the research notes in the landscape report on determinism (libc `qsort` tie order; MSVC argument-evaluation order; `--macho-object` path embedded as an `N_OSO` stab, so names must match when comparing). The unlicensed xcross_gen_snapshot is used as **facts only, no code** [L§3.6].

```yaml
name: gen-snapshot
on:
  workflow_dispatch: { inputs: { flutter: { required: true } } }
  schedule: [{ cron: "23 4 * * *" }]          # detect new stable tags; dispatch per new tag
jobs:
  resolve:      # ubuntu-24.04
    # engine = raw.githubusercontent.com/flutter/flutter/<tag>/bin/internal/engine.version
    # dart   = 'dart_revision' from raw.githubusercontent.com/flutter/flutter/<tag>/DEPS
    # skip if a release for <engine> already exists
  build:
    needs: resolve
    strategy: { matrix: { host: [linux-x64, windows-x64], mode: [release, profile] } }   # later: linux-arm64, windows-arm64
    runs-on: ${{ matrix.host == 'linux-x64' && 'ubuntu-24.04' || 'windows-2022' }}
    steps:
      # 1 sparse, shallow fetch of dart-lang/sdk @ <dart> (blob:none; build/, runtime/, sdk/, tools/, third_party/ as needed);
      #   Windows: git config core.autocrlf false; short work dir (C:\g)
      # 2 DEPS-pinned deps (zlib, boringssl, icu; perfetto for profile) + CIPD tools (gn, ninja, clang, Linux sysroot);
      #   Windows: VS 2022 + clang-cl from CIPD, DEPOT_TOOLS_WIN_TOOLCHAIN=0
      # 3 apply patches/ (ours):
      #   a) runtime: GN arg `russet_target_os_ios` -> dart_os_config emits DART_TARGET_OS_MACOS + DART_TARGET_OS_MACOS_IOS
      #      while target_os stays the host; keep the timeline enabled as in Apple-hosted iOS builds
      #   b) determinism: FreeBSD-derived qsort/heapsort (BSD) in GrowableArray::Sort; explicit argument-evaluation order
      #      at the IL-builder call sites (Windows)
      #   c) backport --macho-sdk-version (c8495e3003) when absent
      # 4 gn gen out/<mode> --args='dart_target_arch="arm64" dart_use_compressed_pointers=false is_clang=true
      #      dart_runtime_mode="<release|profile>" russet_target_os_ios=true ...'   (+ product/release pair per mode)
      # 5 ninja -C out/<mode> gen_snapshot          (~20 min, ~8 GB per research [fx-08])
      # 6 upload gen_snapshot[.exe] + licences (Dart BSD-3, zlib, boringssl, icu, double-conversion, perfetto[profile])
  reference:    # macos-26 (Apple hardware, licence-clean): official compiler output. A GA image; Xcode is not under
                #   test here; switch to xcode-27 only if Flutter 3.47.x requires Xcode 27 (QA §4.2)
    # flutter <tag>: `flutter create ref`; `flutter build ios --release --no-codesign` (and --profile)
    # take .dart_tool/flutter_build/*/app.dill; run ios-release/gen_snapshot_arm64 with the canonical args
    # (same relative names: --macho=App --macho-object=app.o) -> sha256(App)
  verify:       # needs: build, reference
    # run each candidate on the same app.dill with identical args -> sha256 must equal the reference
    # run again with --macho-sdk-version=27.0 -> only LC_BUILD_VERSION.sdk may differ (russet-macho diff)
    # if windows byte-identity is not reached: block publishing for that host; keep linux
  publish:      # needs: verify
    # manifest.json {schema:1, flutter, engine, dart, patches_sha256, assets{name:{sha256, executable_sha256, size}},
    #                reference{release_app_sha256, profile_app_sha256}}
    # actions/attest-build-provenance for each zip + manifest; gh release create engine-<engine-short> (title: Flutter <tag>)
```

**Client side:** `russet flutter gen-snapshot ensure`.
1. Look up the engine hash, then `<cache>/flutter/gen_snapshot/<engine>/<mode>/<host>/`.
2. If absent, download `manifest.json` and the zip, and verify both SHA-256 values (optionally `gh attestation verify`).
3. A user pin (`[build.flutter] gen_snapshot = "<path>"`) overrides the download.
4. Missing compiler: `E_FLUTTER_GEN_SNAPSHOT_MISSING`, listing the available engines and the local-build command `russet flutter gen-snapshot build` (documented as needing ~8 GB and ~20 min).

### 6.9 CocoaPods-only plugins

Decision ladder, per plugin:
1. Has `Package.swift`: SwiftPM (6.7).
2. Podspec convertible: `podspec2spm` generates a package.
   - **Input:** `pod ipc spec` JSON when Ruby and CocoaPods are present; otherwise a minimal reader for the common Ruby-DSL subset, as xlinux does [S20]; third-party dependencies come as CDN JSON.
   - **Supported:**
     - Swift-only or ObjC-only sources (mixed sources split into two targets plus a module map);
     - `vendored_frameworks` → `.binaryTarget`;
     - `resource_bundles`/`resources` → resources (catalogs through actool);
     - `frameworks`/`libraries`/`weak_frameworks` → `linkerSettings`;
     - a `pod_target_xcconfig` subset (`GCC_PREPROCESSOR_DEFINITIONS`, `HEADER_SEARCH_PATHS`, `OTHER_LDFLAGS`, `SWIFT_VERSION`);
     - `dependency` (recursive conversion).
3. Not convertible (script phases, `prepare_command`, `static_framework` with complex module maps, C++ with custom flags): the **real CocoaPods** route (M7, section 7.6).
4. Otherwise `E_COCOAPODS_REQUIRED`, naming the plugin, the reason and the workaround.

CocoaPods trunk becomes read-only on 2026-12-02 [L§2.2 tc-36]. Existing specs stay resolvable (ASSUMPTION, verify via CDN in task B15).

---

## 7. Stack (d): Xcode projects

### 7.1 Pipeline

```text
.xcworkspace | .xcodeproj (pbxproj) | .xcproj (JSON, Xcode 27.2 beta)
  -> russet-xcodeproj: parse objects, groups, synchronized folders, schemes, xcconfigs; resolve layered settings
  -> russet-pif: JSON PIF (format = `xcodebuild -dumpPIF` output) + package PIF from SwiftPM for XCSwiftPackage refs
  -> russet-swb build --pif <file> --request <json>  (SwiftBuild library; XCODE_EXTRA_PLATFORM_FOLDERS = platform view)
  -> Swift Build does compile/link/resources/Info.plist/embedding (tools via platform view; CODE_SIGNING_ALLOWED=NO)
  -> russet-build: DT normalization, bundle-ID policy, lints, IPA, SigningPlan
```

The open build service loads `.json`/`.pif` containers directly. For `.xcodeproj` it would run `xcrun xcodebuild -dumpPIF`, which needs a Mac [V17]. So we generate the PIF ourselves.

### 7.2 `russet-xcodeproj`

- **pbxproj:** an OpenStep plist parser (strings, data, arrays, dicts, comments). The object graph covers:
  - `PBXProject`, `PBXNativeTarget`, `PBXAggregateTarget`;
  - `PBX*BuildPhase` (Sources, Frameworks, Resources, CopyFiles, ShellScript, Headers);
  - `PBXBuildFile`, `PBXFileReference`, `PBXGroup`, `PBXVariantGroup`, `XCVersionGroup`;
  - `PBXFileSystemSynchronizedRootGroup` and its exception sets (Xcode 16+);
  - `PBXTargetDependency`, `PBXContainerItemProxy`;
  - `XCBuildConfiguration`, `XCConfigurationList`;
  - `XCRemoteSwiftPackageReference`, `XCLocalSwiftPackageReference`, `XCSwiftPackageProductDependency`.
- **JSON `.xcproj`:** mapped to the same model. Reference: apple/xcode-project-format (Apache-2.0) [S22].
- **Also parsed:** `contents.xcworkspacedata` (XML) and `.xcscheme` (XML: build action entries, launch configuration).
- **Settings resolver:**
  - layers: SDK defaults, then project xcconfig, then project settings, then target xcconfig, then target settings, then overrides;
  - supports `$(inherited)`, conditional keys `[sdk=iphoneos*]`/`[arch=arm64]`/`[config=Debug]`, `#include` in xcconfig, and the macro operators;
  - parity target: `xcodebuild -showBuildSettings` for corpus projects. LibreDarwin/IDETools (BSD-3) is a reference [L§2.2].
- **Rust model for reference:** tuist/XcodeProj (Swift, MIT) [S22]. We do not depend on it at runtime.

### 7.3 PIF generation (`russet-pif`)

- The output is the JSON document `xcodebuild -dumpPIF` writes, which Swift Build loads [V17]: workspace, project and target objects with GUIDs and signatures.
- The exact schema is learned from the P0-2 corpus [L§6.2]: dumpPIF for the Flutter Runner, the RN template, Xcode templates and two or three OSS apps. The Mac oracle confirms first that `-dumpPIF` still exists in Xcode 27.
- Reference: `SWBProjectModel`/`PIFGenerationModel.swift` and `SWBProtocol/PIFKeyConstants.swift` [S7].
- **GUIDs** are deterministic: the pbxproj object ID with a project prefix. **Signatures** are content hashes.
- **Mapping:**
  - native target → `productTypeIdentifier`;
  - phases → PIF build phases with build-file GUIDs;
  - `PBXShellScriptBuildPhase` → shell-script phase (executed by Swift Build with `/bin/sh` on Linux);
  - package products → package PIF from SwiftPM (`swift package` resolution of a synthesized manifest, whose PIF SwiftPM writes as `manifest.pif` in its scratch path [V2]), merged by GUID.

### 7.4 `russet-swb`

```text
russet-swb build --pif <workspace.pif.json> --request <request.json> --derived-data <dir> [--jobs N]
  stdout: JSON lines  {"kind":"planningStarted"|"taskStarted"|"taskOutput"|"taskFinished"|"diagnostic"|"progress"|"buildCompleted", ...}
russet-swb settings --pif <..> --request <..>  -> resolved build settings JSON per target (for IDE editors)
```

- **Request:** configuration; target GUIDs; `parameters.overrides`:
  - `SDKROOT=iphoneos`, `ARCHS=arm64`, `ONLY_ACTIVE_ARCH=YES`, `CODE_SIGNING_ALLOWED=NO`;
  - `ASSETCATALOG_EXEC`, `IBC_EXEC`;
  - `XCODE_VERSION_ACTUAL`/`XCODE_PRODUCT_BUILD_VERSION`/`PLATFORM_PRODUCT_BUILD_VERSION`, set from `SdkFacts` per the metadata policy (ASSUMPTION: these builtins can be overridden; otherwise the bundle normalization in 4.4 repairs the values);
  - `SWIFT_SDK_TOOLSETS`, with the toolset from 3.4.
- **Implementation:** links the `SwiftBuild` library product of swift-build at the `swift-<version>-RELEASE` tag matching the toolchain [S7]. Session flow: `createSession` → `sendPIF` → build request → stream `SwiftBuildMessage`.
- **Build:** in our CI for linux-x64 and windows-x64, per Swift minor version. Shipped as a Russet component (Apache-2.0 dependencies; no Apple files). Its size is measured in task B16.
- **Fallback for debugging:** `swbuild build <pif> --buildRequestFile … --buildParametersFile …` from the same tag [S7].

### 7.5 Before M7: SwiftPM adaptation

For single-app-target projects (SwiftUI or UIKit sources, asset catalogs, package dependencies), `russet xcode adapt` generates `.russet/work/adapted/Package.swift` plus a `russet.toml` fragment and builds through section 4. Approach seen in omarchy and xlinux [L§2.2 tc-21]. Anything with script phases, multiple app targets or storyboards gets a clear "needs the PIF backend (M7)" error.

### 7.6 CocoaPods (M7)

- **Install:** `pod install` with `PATH` shims: `xcrun`; an `xcodebuild` stub that answers `-version` and `-showsdks` from `SdkFacts`; `codesign`; `ditto`. CocoaPods runs on Linux, but its output needs `xcodebuild` [L§2.2 tc-37]. The stub covers only the queries `pod install` makes.
- **Build:** `Pods.xcodeproj` + app project → workspace PIF → `russet-swb`.
- **Ruby:** the user's own Ruby (≥ 3.1) and `gem install cocoapods`, or a Russet-managed portable Ruby (Q3).
- Xcodeproj cannot read JSON `.xcproj` yet (CocoaPods/Xcodeproj#1052) [L§2.2].

---

## 8. Stack (e): React Native / Expo and .NET MAUI (later phases)

**React Native / Expo (after M7).** Russet must provide:
1. **JavaScript side:** the user's Node and Metro. `hermesc` comes from the `hermes-compiler` npm package (linux64 and win64 builds exist [L§4.8 fx-38]). Russet only checks versions.
2. **Native project:** `expo prebuild` (Expo) or the RN template `ios/`. Then `pod install` (7.6), including RN codegen. Then Xcode-project build through PIF (7.1).
   - Script phases ("Bundle React Native code and images", Hermes replacement) run under Swift Build with build settings as environment. Linux needs `bash` and `node` on `PATH`; Windows needs Git Bash or WSL (ASSUMPTION; spike).
3. **Prebuilt RN core xcframeworks**, when the RN version supports them, to cut pod compile time (ASSUMPTION; verify per RN version).
4. **Expo's Swift macro plugin** built for the Linux host, once per version (xlinux does this [S20]).
5. **Launch screen and app icon:** through 5.2 and 5.4. The RN template ships `LaunchScreen.storyboard` and `Images.xcassets` [L§4.8 xt-14].
6. **Dev loop:** install a dev client once, then reload JS over Metro.
   - The phone must reach Metro on the host's LAN address. usbmux cannot open device-to-host connections, so a USB-only Metro path needs device-area work.
   - Release builds bundle `main.jsbundle` as Hermes bytecode.
7. **Interim before M7:** a prebuilt "dev-client shell" per RN version, built by Russet from the template, with JS over Metro.

**.NET MAUI (spike, no build commitment).** What Russet would have to provide:
1. **The `xcrun` shim** (6.2), extended for the tools `dotnet/macios` MSBuild tasks call (`clang`, `ld` → lld, `actool`, `ibtool`, `dsymutil`, `strip`, `lipo`, `codesign`).
2. **Off-Mac AOT compilers** for ios-arm64:
   - Mono AOT cross compiler and/or NativeAOT ILCompiler packs for linux-x64 and windows-x64 hosts. Today these exist only for macOS hosts (ASSUMPTION, verify in spike B20).
   - This would need a CI job like 6.8 against `dotnet/runtime`.
3. **Signing, packaging and install** from the other areas.

Hot Restart was removed in .NET 10 [L§4.9]. The spike's deliverable is a feasibility report with the list of macOS-only packs.

---

## 9. Interfaces

### 9.1 `russet-build` public API (Rust)

> **Superseded fields (ARCHITECTURE §5.3, §9.2, §20.1):** `BuildEvent` and `EventSink = mpsc::Sender<BuildEvent>` are
> replaced by the `JobEvent` kinds (`stage`, `task`, `diagnostic`, `log`, `progress`, `artifact`, …) emitted through
> `russet_core::progress::EventSink`; `BuildOutput.build_id` is a `JobId`; errors are `russet_core::Error` values with
> the numeric codes of ARCHITECTURE §13.3. The rest of this API is kept for its rationale and detail.

```rust
// crates/russet-build/src/lib.rs
pub struct Builder { /* private */ }

impl Builder {
    pub fn new(env: BuildEnv) -> Result<Self, BuildError>;
    pub async fn detect(&self, project_dir: &Utf8Path) -> Result<ProjectInfo, BuildError>;
    pub async fn plan(&self, req: &BuildRequest) -> Result<BuildPlan, BuildError>;          // dry run, no side effects
    pub async fn build(&self, req: BuildRequest, sink: EventSink, cancel: CancellationToken)
        -> Result<BuildOutput, BuildError>;
    pub async fn resolve_settings(&self, req: &BuildRequest) -> Result<ResolvedSettings, BuildError>;
    pub async fn resolve_info_plist(&self, req: &BuildRequest, product: Option<&str>)
        -> Result<plist::Dictionary, BuildError>;                                            // IDE Info.plist editor preview
    pub async fn clean(&self, project_dir: &Utf8Path, scope: CleanScope) -> Result<CleanReport, BuildError>;
}

pub struct BuildEnv {
    pub sdk: Arc<dyn SdkProvider>,          // implemented by the SDK area
    pub tools_dir: Utf8PathBuf,             // russet-tool multi-call links (xcrun, actool, ...)
    pub data_dir: Utf8PathBuf,
    pub cache_dir: Utf8PathBuf,
    pub host: HostOs,                       // Linux | Windows
    pub jobs: NonZeroUsize,
}

#[async_trait::async_trait]
pub trait SdkProvider: Send + Sync {
    async fn resolve(&self, selector: &SdkSelector) -> Result<SdkFacts, BuildError>;  // "auto" | canonical name | xcode build
}

pub struct SdkFacts {
    pub id: String,                                   // "iphoneos27.0-27A266a"
    pub sdk_path: Utf8PathBuf,                        // .../iPhoneOS27.0.sdk
    pub sdk_version: Version,                         // 27.0
    pub sdk_canonical_name: String,                   // iphoneos27.0
    pub sdk_build: Option<String>,                    // 24A430
    pub platform_build: Option<String>,               // 24A430
    pub platform_version: String,                     // 27.0
    pub xcode_version: Version,                       // 27.0
    pub xcode_build: String,                          // 27A266a
    pub swift_required: Version,                      // 6.4
    pub valid_deployment_targets: Vec<Version>,
    pub additional_info: plist::Dictionary,           // platform Info.plist AdditionalInfo
    pub back_deploy_dirs: Vec<Utf8PathBuf>,           // .../usr/lib/swift-{5.0,5.5,6.2}/iphoneos
    pub platforms_view: Utf8PathBuf,                  // value for XCODE_EXTRA_PLATFORM_FOLDERS
    pub toolset_swb: Utf8PathBuf,
    pub toolset_native: Utf8PathBuf,
    pub tools: ToolPaths,                             // swift, swiftc, clang, ld64.lld, libtool, dsymutil, llvm-{strip,lipo,otool,install-name-tool}
}

pub struct BuildRequest {
    pub project_dir: Utf8PathBuf,
    pub profile: Profile,                             // Debug | Release | Profile | Named(String)
    pub product: Option<String>,                      // product / scheme / target
    pub package: PackageKind,                         // None | Ipa { store: bool }
    pub bundle_id_policy: BundleIdPolicy,             // from the signing area
    pub team_id: Option<String>,
    pub overrides: BTreeMap<String, String>,          // KEY=VALUE build-setting overrides
    pub flutter: Option<FlutterRunContext>,           // kernel override, dart defines, flavor (custom-device install)
    pub clean: bool,
    pub offline: bool,
}

pub enum BundleIdPolicy { Exact, TeamSuffix { team_id: String }, XtoolPrefix { prefix: String },
                          Custom(BTreeMap<String, String>) }

pub struct BuildOutput {
    pub build_id: BuildId,
    pub app: BundleInfo,                              // path, executable, effective bundle id, versions
    pub extensions: Vec<BundleInfo>,
    pub bundle_ids: BTreeMap<String, String>,         // original -> effective
    pub signing_plan: SigningPlan,                    // inside-out items for the signing area
    pub dsyms: Vec<Utf8PathBuf>,
    pub ipa: Option<Utf8PathBuf>,
    pub report_path: Utf8PathBuf,                     // build-report.json
    pub lints: Vec<LintFinding>,
    pub duration: Duration,
}

pub struct SigningPlan { pub items: Vec<SignItem> }   // ordered: deepest first, app last
pub struct SignItem { pub path: Utf8PathBuf, pub kind: SignKind /* Bundle | MachO */,
                      pub entitlements: Option<Utf8PathBuf>, pub get_task_allow: bool }

pub enum BuildEvent {                                   // serialized with #[serde(tag = "kind", rename_all = "camelCase")]
    Started { build_id: BuildId, summary: RequestSummary },
    Stage { stage: Stage, state: StageState },        // resolve|prepare|compile|resources|assemble|lint|package
    TaskStarted { task: TaskId, title: String, tool: Option<String> },
    TaskFinished { task: TaskId, ok: bool, duration_ms: u64 },
    Diagnostic(Diagnostic),                           // severity, message, file, line, column, tool, code
    Log { task: Option<TaskId>, stream: Stream, line: String },
    Progress { done: u32, total: u32 },
    Artifact { kind: ArtifactKind, path: Utf8PathBuf },
    Finished { ok: bool, duration_ms: u64, error: Option<ErrorInfo> },
}
pub type EventSink = tokio::sync::mpsc::Sender<BuildEvent>;
```

Stack adapters implement an internal trait:
- `detect(dir) -> Option<DetectedStack>`;
- `prepare(&mut BuildContext)`;
- `compile(&mut BuildContext) -> CompiledProducts`;
- `resources(&mut BuildContext, &CompiledProducts) -> ResourceOutputs`.

Assembly, lint and packaging are shared. Detection order:
1. `russet.toml [project] stack`;
2. `pubspec.yaml` with `flutter:` → flutter;
3. `*.xcworkspace`/`*.xcodeproj`/`*.xcproj` → xcode;
4. `package.json` with `react-native` → rn;
5. `*.csproj` with `net*-ios` → maui;
6. `Package.swift` → swiftpm.

Ambiguity gives `E_PROJECT_AMBIGUOUS` with the candidates.

### 9.2 `russet.toml` (build-relevant sections)

> **Superseded (ARCHITECTURE §14.2, §20.1):** every TOML key is kebab-case (`bundle-id`, `launch-screen-fallback`,
> `build-number`, …), and the merged schema of ARCHITECTURE §14.2 is binding [D 15, D 16].

```toml
[project]
name  = "Hello"
stack = "swiftpm"                      # swiftpm | flutter | xcode | react-native | maui  (auto-detected if omitted)

[app]
bundle_id         = "com.example.hello"   # canonical, exact (paid teams use it verbatim)
display_name      = "Hello"
version           = "1.0.0"               # CFBundleShortVersionString
build_number      = "1"                   # CFBundleVersion | "git-count" | "timestamp"
deployment_target = "17.0"                # must be in the SDK's ValidDeploymentTargets
devices           = ["iphone", "ipad"]
orientations      = { iphone = ["portrait"], ipad = ["portrait", "portrait-upside-down", "landscape-left", "landscape-right"] }
icon              = "AppIcon"             # icon set in a main-bundle catalog, or a .png path (development only)
accent_color      = "AccentColor"
launch_screen     = { color = "LaunchBackground", image = "LaunchLogo", respects_safe_area = true }  # UILaunchScreen
info_plist        = "Resources/Info.plist"         # merged at precedence level 3
entitlements      = "Resources/Hello.entitlements"
privacy_manifest  = "Resources/PrivacyInfo.xcprivacy"
resources         = ["Resources/Assets.xcassets", "Resources/Localizable.xcstrings", "Resources/Fonts"]

[[app.extensions]]
product          = "HelloWidget"           # SwiftPM library product
kind             = "widgetkit"             # sets NSExtension defaults
bundle_id_suffix = "widget"                # -> com.example.hello.widget
info_plist       = "Widget/Info.plist"
entitlements     = "Widget/Widget.entitlements"

[build]
default_profile = "debug"
sdk       = "auto"                         # "auto" | "iphoneos27.0" | "27A266a"
backend   = "swiftbuild"                   # swiftbuild | native | xtool | pif
jobs      = "auto"
metadata  = "sdk"                          # none | minimal | sdk | xcode-compat   (section 4.4)
build_machine_os_build = ""                # only used with metadata = "xcode-compat"
dsym      = "release"                      # never | release | always
strip     = "release"
lint      = "auto"                         # auto (warn debug / error release) | warn | error | off
launch_screen_fallback = "error"           # error | empty   (unconvertible launch storyboards)
overrides = { }                            # build-setting overrides, e.g. { SWIFT_ACTIVE_COMPILATION_CONDITIONS = "DEBUG RUSSET" }

[build.swiftpm]
package = "."
product = "Hello"
swiftc_flags = []                          # -Xswiftc
cc_flags     = []                          # -Xcc
ld_flags     = []                          # -Xlinker

[build.flutter]
target            = "lib/main.dart"
flavor            = ""
dart_defines      = []                     # "KEY=VALUE"
dart_define_files = []
plugins           = "swiftpm"              # swiftpm | swiftpm+convert | swiftpm+cocoapods
gen_snapshot      = "auto"                 # auto (Russet CI) | <path>
obfuscate         = false
split_debug_info  = ""

[build.xcode]                               # M7
workspace     = "App.xcworkspace"
project       = "App.xcodeproj"
scheme        = "App"
configuration = { debug = "Debug", release = "Release" }
cocoapods     = "auto"                     # auto | off

[build.react_native]                        # later
entry = "index.js"
expo  = true

[build.profiles.release]                    # any [build] key may be overridden per profile
dsym    = "always"
package = "ipa-store"
```

**Precedence:** CLI flags > `[build.profiles.<p>]` > `[build]`/`[app]` > stack-native configuration (`Package.swift` platforms, `pubspec.yaml`, `.xcodeproj` settings) > Russet defaults.

**Unknown keys** are an error (`E_MANIFEST_INVALID`, pointing at the line), so that typos never silently change a build.

### 9.3 CLI (build-plane commands)

```text
russet build [--release|--profile <name>] [--project <dir>] [--product <name>] [--ipa[=dev|store]]
             [--backend swiftbuild|native|xtool|pif] [--clean] [--set KEY=VALUE]... [--json] [--offline]
russet clean [--all]                         # .russet/work + .russet/build; --all also per-project caches
russet build-settings [--product <p>] [--key KEY] [--json]
russet inspect <App.app|App.ipa> [--json]    # Info.plist, Mach-O summary, lints, DT keys, CAR summary
russet lint <App.app|App.ipa> [--store]
russet package ipa <App.app> -o <out.ipa> [--store]
russet assets compile <X.xcassets> -o <dir> [--app-icon N] [--accent-color N]   # wraps russet-actool
russet assets app-icon <1024.png> --catalog <X.xcassets> [--name AppIcon]
russet strings compile <X.xcstrings> -o <dir>
russet ib convert-launch-screen <LaunchScreen.storyboard> [--catalog <X.xcassets>] [--write-info-plist <p>]
russet flutter register [--udid U] | unregister | build [--release|--profile] | gen-snapshot ensure|build|list
russet flutter device ping|install|uninstall|run-debug|forward|screenshot ...   # called by flutter_tools (6.3)
russet xcode adapt|dump-pif|show-build-settings [--scheme S] [--configuration C]
russet oracle run --tool actool|xcstringstool|ibtool|dumppif --case <id>          # development only
russet cache gc
```

`--json` prints the `BuildOutput` (success) or `ErrorInfo` (failure) as one JSON document, for scripts and CI.

### 9.4 JSON-RPC (daemon `russetd`)

> **Superseded (ARCHITECTURE §8.3, §7.6, §9, §20.1):** `build.start` takes `{project, target?, configuration?, …}`
> and stays open until it returns `BuildOutput` (or returns `{jobId}` at once with `background: true`). Job control is
> `job.*`: `job.cancel` (or `$/cancelRequest`), `job.get`, `job.list`, `job.log`; events come from `job.subscribe` as
> `job.updated` and `job.events` (no `build.event` notification). `inspect.app` is `build.inspect`. Logs go to
> `.russet/logs/<jobId>.jsonl`. Builds of one project are serialized by the project lock (ARCHITECTURE §6.2), and
> `build.inProgress` (4001) replaces `E_BUILD_IN_PROGRESS`. This table is kept for its rationale.

Method names are dotted and camelCase payloads are used (ASSUMPTION; to be aligned with the core-area conventions).

| Method | Params | Result |
|---|---|---|
| `build.start` | `{projectDir, profile?, product?, package?: "none"\|"ipa-dev"\|"ipa-store", backend?, overrides?, clean?, bundleIdPolicy?}` | `{buildId, logPath}` |
| `build.cancel` | `{buildId}` | `{cancelled: bool}` |
| `build.get` | `{buildId}` | `{state: "queued"\|"running"\|"succeeded"\|"failed"\|"cancelled", progress, output?: BuildOutput, error?: ErrorInfo}` |
| `build.list` | `{projectDir?, limit?}` | `[{buildId, projectDir, profile, state, startedAt, durationMs}]` |
| `build.log` | `{buildId, filter?: "all"\|"errors"\|"warnings", tail?: number, afterSeq?: number}` | `{events: BuildEvent[], nextSeq}` |
| `build.plan` | same as `build.start` | `BuildPlan` (stages, tasks, resolved SDK/toolchain, effective bundle IDs) |
| `build.settings` | `{projectDir, product?, profile?, keys?: string[]}` | `{settings: {KEY: VALUE}, sources: {KEY: "russet.toml:12"\|"xcconfig:…"}}` |
| `build.clean` | `{projectDir, scope: "build"\|"all"}` | `{freedBytes}` |
| `project.detect` | `{dir}` | `{stack, products[], manifests[]}` |
| `project.infoPlist` | `{projectDir, product?}` | `{plist (JSON form), provenance: {key: source}}` |
| `assets.list` | `{catalog}` | `{assets: [{name, kind, variants[]}]}` |
| `assets.compile` | `{catalog, outDir, appIcon?, accentColor?, deploymentTarget}` | `{outputs[], partialInfoPlist, diagnostics[]}` |
| `assets.appIcon` | `{source1024, catalog, name?}` | `{iconSet}` |
| `assets.render` | `{catalog, name, scale?, appearance?}` | `{pngBase64}` (editor previews) |
| `inspect.app` | `{path}` | `{infoPlist, machO[], lints[], dt, car?}` |
| `flutter.register` | `{udid}` | `{deviceId, configPath}` |
| `flutter.genSnapshot.ensure` | `{flutterRoot?, mode}` | `{path, engine, sha256}` |

Notification `build.event`: `{buildId, seq, event: BuildEvent}`. Example:

```json
{"jsonrpc":"2.0","method":"build.event","params":{"buildId":"b-20261011-0007","seq":42,
 "event":{"kind":"diagnostic","severity":"error","message":"cannot find 'Foo' in scope",
          "file":"/home/u/Hello/Sources/Hello/ContentView.swift","line":6,"column":13,"tool":"swiftc"}}}
```

Only one build runs per project directory at a time. A second `build.start` for the same directory returns `E_BUILD_IN_PROGRESS` with the running `buildId`.

### 9.5 MCP tools (build plane)

> **Superseded (ARCHITECTURE §11.4, §20.1):** the tools are `build`, `job_status`, `job_log`, `build_settings`,
> `app_inspect`, `build_clean` and `assets_app_icon`, with camelCase arguments and results (`project`, `app.path`), and
> slow calls return `{status: "running", jobId}` within 50 s [D 12]. The schemas below are kept for their rationale.

Names are prefixed `russet_` (ASSUMPTION; the MCP area may namespace them differently).

```json
{"name":"russet_build","description":"Build the iOS app in a project directory. Returns the .app path, effective bundle id, errors and lint findings.",
 "inputSchema":{"type":"object","additionalProperties":false,"properties":{
   "project_dir":{"type":"string","description":"Absolute path; defaults to the workspace root"},
   "profile":{"type":"string","enum":["debug","release","profile"],"default":"debug"},
   "product":{"type":"string"},
   "package":{"type":"string","enum":["none","ipa-dev","ipa-store"],"default":"none"},
   "clean":{"type":"boolean","default":false},
   "wait":{"type":"boolean","default":true,"description":"Block until finished (timeout 900 s); otherwise return build_id"}}}}
// result: {"ok":bool,"build_id":"...","app_path":"...","bundle_id":"...","duration_ms":6890,
//          "errors":[{"file":"...","line":6,"column":13,"message":"..."}],   // first 20
//          "warning_count":3,"lints":[{"id":"L006","severity":"warning","message":"...","fixed":true}],"log_tail":"..."}

{"name":"russet_build_status","inputSchema":{"type":"object","required":["build_id"],"properties":{"build_id":{"type":"string"}}}}
{"name":"russet_build_log","inputSchema":{"type":"object","required":["build_id"],"properties":{
   "build_id":{"type":"string"},"filter":{"type":"string","enum":["all","errors","warnings"],"default":"errors"},
   "tail":{"type":"integer","minimum":1,"maximum":2000,"default":200}}}}
{"name":"russet_build_settings","inputSchema":{"type":"object","properties":{
   "project_dir":{"type":"string"},"product":{"type":"string"},"keys":{"type":"array","items":{"type":"string"}}}}}
{"name":"russet_inspect_app","inputSchema":{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}}
{"name":"russet_clean","inputSchema":{"type":"object","properties":{"project_dir":{"type":"string"},
   "scope":{"type":"string","enum":["build","all"],"default":"build"}}}}
{"name":"russet_app_icon","inputSchema":{"type":"object","required":["source_png"],"properties":{
   "source_png":{"type":"string"},"project_dir":{"type":"string"},"name":{"type":"string","default":"AppIcon"}}}}
```

Results never include environment dumps or file contents beyond the log tail. Paths are absolute.

### 9.6 `build-report.json` (schema 1)

> **Note (ARCHITECTURE §20.1):** `buildId` in this file is the job id (`job_…`); the rest of schema 1 stands.

```json
{
  "schema": 1, "buildId": "b-20261011-0007", "russetVersion": "0.1.0",
  "project": {"dir": "/home/u/Hello", "stack": "swiftpm", "product": "Hello", "backend": "swiftbuild"},
  "profile": "debug", "startedAt": "2026-10-11T00:30:00Z", "durationMs": 6890, "diskDeltaBytes": 81234567,
  "toolchain": {"swift": "6.4 (swift-6.4-RELEASE)", "linker": "ld64.lld 21.1.6 (xtool darwin-tools 1.1.0)",
                "sdk": {"name": "iphoneos27.0", "build": "24A430", "xcode": "27.0 (27A266a)", "id": "iphoneos27.0-27A266a"}},
  "metadata": {"policy": "sdk", "stamped": {"DTPlatformName": "iphoneos", "DTPlatformVersion": "27.0", "DTSDKName": "iphoneos27.0",
               "DTSDKBuild": "24A430", "DTPlatformBuild": "24A430", "DTCompiler": "com.apple.compilers.llvm.clang.1_0",
               "DTXcode": "2700", "DTXcodeBuild": "27A266a"}},
  "bundles": [{"path": "Hello.app", "kind": "app", "bundleId": "com.example.hello", "originalBundleId": "com.example.hello",
               "machO": [{"path": "Hello", "arch": "arm64", "minos": "17.0", "sdk": "27.0", "tool": "lld 21.1.6", "builtByRusset": true}]}],
  "lints": [{"id": "L006", "severity": "warning", "path": "Hello.app/Hello", "message": "removed host rpath /home/...", "fixed": true}],
  "artifacts": {"app": ".russet/build/debug-iphoneos/Hello.app", "dsym": null, "ipa": null},
  "signingPlan": [{"path": "Hello.app", "entitlements": ".russet/work/entitlements/Hello.plist", "getTaskAllow": true}]
}
```

### 9.7 Error codes

> **Superseded (ARCHITECTURE §13.3, §20.1):** errors carry numeric codes with dotted names, for example
> `config.projectNotFound` (1301, was `E_PROJECT_NOT_FOUND`), `build.projectAmbiguous` (4000), `build.inProgress`
> (4001, was `E_BUILD_IN_PROGRESS`), `config.invalid` (1300, was `E_MANIFEST_INVALID`), `sdk.missing` (2100) and
> `assets.storyboardUnsupported` (4101) [D 7]. The table keeps the hints.

| Code | Meaning (hint shown to the user) |
|---|---|
| `E_SDK_MISSING` | No SDK imported (`russet sdk import <Xcode.xip>`) |
| `E_SDK_INCOMPLETE` | The SDK lacks a file the build needs (platform `version.plist`, back-deploy dylib, …). The message names it |
| `E_TOOLCHAIN_MISMATCH` | The Swift minor version is not the one the SDK requires (Xcode 27 → 6.4) |
| `E_LINKER_UNSUPPORTED` | The linker cannot read the SDK's TBDs (`arm64e.x1`): use LLVM ≥ 23.1.3 or the xtool toolset |
| `E_MANIFEST_INVALID` | `russet.toml` parse or validation error, with file:line |
| `E_PROJECT_NOT_FOUND` / `E_PROJECT_AMBIGUOUS` | Detection failed or found several candidates |
| `E_BUILD_IN_PROGRESS` | Another build is running for this project |
| `E_COMPILE_FAILED` / `E_LINK_FAILED` | Tool failure. Diagnostics are attached |
| `E_DYNAMIC_PRODUCT_UNSUPPORTED` | swiftbuild cannot link `.dynamic` products [V19]. The fallback to `native` was disabled or failed |
| `E_RESOURCE_UNSUPPORTED` | A closed-tool input with no open replacement (Core Data model, Metal, …) |
| `E_ASSET_INVALID` | Invalid catalog content (bad `Contents.json`, missing file, unsupported icon format) |
| `E_STORYBOARD_UNSUPPORTED` | A storyboard or xib outside the supported subset |
| `E_BUNDLE_ID_INVALID` | Illegal characters, or an extension ID not prefixed by the app ID |
| `E_LINT_FAILED` | A lint at error severity (release/ship) |
| `E_PACKAGE_FAILED` | IPA writing failed |
| `E_FLUTTER_NOT_FOUND` / `E_FLUTTER_ENGINE_MISSING` | `flutter` not on `PATH` / iOS engine artifacts missing |
| `E_FLUTTER_GEN_SNAPSHOT_MISSING` | No CI compiler for this engine hash |
| `E_FLUTTER_CUSTOM_DEVICES_DISABLED` | `flutter config --enable-custom-devices` failed |
| `E_PLUGIN_UNSUPPORTED` / `E_COCOAPODS_REQUIRED` | Plugin conversion failed. The message names the plugin and the reason |
| `E_XCODEPROJ_UNSUPPORTED` / `E_PIF_FAILED` / `E_SWB_FAILED` | Xcode-project path errors |
| `E_CANCELLED`, `E_IO` | – |

---

## 10. Platform differences

| Topic | Linux | Windows |
|---|---|---|
| Phasing | Native from day one | Native first (B17, W27b). SDK and DDI import are always native (W16b). WSL2 split mode is only a documented fallback if Swift Build or the native backend fails on Windows (Q7, settled) |
| Swift | swift.org 6.4 (AUR `swift-bin` here) | swift.org 6.4 installer |
| SwiftPM backend | `swiftbuild` (verified) | `native` first. Whether Swift Build's Apple plugin works on Windows with `XCODE_EXTRA_PLATFORM_FOLDERS` is unverified [L§5.4]; spike in B17 |
| Platform view | symlinks | directory junctions (no admin) plus copied `.exe` tools. Whether Swift Build resolves `actool` to `actool.exe` is unverified |
| Linker | xtool darwin-tools `ld64.lld` 21.1.6 or LLVM ≥ 23.1.3 | LLVM ≥ 23.1.3 Windows build (`ld64.lld.exe`, ASSUMPTION: shipped in the installer) behind the `russet-tool` trampoline (replaces xtool's POSIX `sh` script) |
| Command-line length | large | 32,767 UTF-16 units: response files (`@file`) for swiftc/clang/lld above 30,000 characters. SwiftPM plan repair may be needed (xcross does it [L§5.1 tc-29]) |
| Paths | case-sensitive | `MAX_PATH` 260 unless long paths are enabled. Keep work directories short (`%LOCALAPPDATA%\Russet\w\<hash8>`). Avoid `\\?\` in tool arguments (Foundation mishandles it [L§5.1]). Lint L012 for case collisions. The SDK is imported into a short root `%LOCALAPPDATA%\Russet\s\<id8>\` with an iOS-only layout (longest relative path inside the SDK: 171 characters), so the native backend stays under `MAX_PATH`; the swiftbuild backend needs `LongPathsEnabled` (§3.4) |
| Exec bits in IPA | from the filesystem | inferred from Mach-O type (4.8) |
| SDK links | kept (symlinks) | The importer (W16b) makes directory links junctions and file links hardlinks; copies only off NTFS, where directory links are refused (copying them would duplicate module maps); targets normalized to `\`; chains resolved at import; neither Developer Mode nor admin is needed. An `Xcode.app` directory input is accepted only when its links are valid NTFS links; otherwise Russet asks for the `.xip` |
| Swift SDK registration | `swift build --swift-sdks-path <Russet data>` | same; `info.json` lists `x86_64-unknown-windows-msvc` and `aarch64-unknown-windows-msvc` next to the Linux triples, because SwiftPM filters bundles by host triple |
| Flutter | custom device `linux-x64` works with Linux host artifacts | custom devices require a `linux-*` platform. Building that bundle on Windows is unverified; alternatives are `flutter attach` against a Russet-launched app, or our own DevFS (as xcross does). Spike in B17 |
| gen_snapshot | `linux-x64` from CI | `windows-x64` from CI |
| CocoaPods / RN scripts | Ruby, bash, node | Ruby on Windows untested: WSL fallback |
| Tool shims | links to `russet-tool` | copies of `russet-tool.exe` named `xcrun.exe`, `actool.exe`, … |

---

## 11. Test plan

**Unit tests** (no SDK; run in public CI on Linux and Windows):

| ID | What is tested |
|---|---|
| T-PLIST | XML, binary and OpenStep round trips; macro expansion, including operators and `$(inherited)`; Info.plist precedence |
| T-DT | DT* derivation from **synthetic** SDK fixtures we write (fake `SDKSettings`, `SystemVersion`, `version.plist`), for every metadata policy; nested-bundle normalization turns `10989`/`99T999` into the app's values |
| T-BUNDLEID | the policies; extension-prefix rule; invalid characters |
| T-IPA | entry order; directory entries; modes 755/644/symlink; Zip64 threshold; two runs give the same SHA-256; Windows exec-bit inference |
| T-MACHO | lints on Mach-O files built in-test by hand-assembling minimal headers (no Apple binaries); rpath deletion keeps `sizeofcmds` consistent and LINKEDIT untouched |
| T-ACTOOL-CLI | parsing of every xcspec flag; `--version` plist; xml1 and human-readable results; dependency-info bytes; missing-directory error text; symbols-only mode writes no `Assets.car` |
| T-CAR | write, then read with our reader; parse with `ipsw car --metadata-only --json` (CI optional); deterministic output |
| T-XCSTRINGS | the dry-run list; plural and device variations into stringsdict |
| T-PBXPROJ / T-PIF | parse the corpus pbxproj files; build-settings parity against recorded `-showBuildSettings`; PIF diff against recorded `-dumpPIF` (normalized GUIDs) |

**Integration tests** (need a user-imported SDK; run on this Linux host and a self-hosted runner, never public CI):

| ID | What is tested |
|---|---|
| I-SWIFTUI-HELLO | `russet build`: `sdk 27.0`, `minos` = target, Info.plist has the scene manifest and DT keys, no host rpaths, lints clean, time ≤ 1.5 × `xtool dev build` |
| I-SWIFTPM-RESOURCES | package with a `.xcassets` builds (phase A actool), and `Bundle.module` resources are present. This is the xtool #310 scenario |
| I-DYNAMIC-PRODUCT | `.dynamic` dependency builds through the automatic `native` fallback; the framework is wrapped and resolvable |
| I-EXTENSION | app plus a WidgetKit extension: appex layout, ID prefix, equal versions |
| I-IPA | `--ipa=store` with a back-deploy dylib (deployment target 17.0, using Span): `SwiftSupport` present |
| I-FLUTTER-DEBUG / I-FLUTTER-RELEASE | counter app; release uses the CI gen_snapshot; `App.framework/App` has `sdk 27.0` |
| I-FLUTTER-PLUGINS | two SwiftPM plugins plus one converted CocoaPods-only plugin |

**Device tests** (run by the device-area agent only, per the DEVICE RULE). The build plane provides artifacts and expectations:
- the M1 demo re-run with the new `sdk 27.0` app (must launch and stop at the breakpoint);
- the Flutter debug app (hot reload);
- a harness app that loads every asset and string from the corpus catalogs (phase B).

**Golden corpus** (section 5.5): actool, xcstringstool and launch-storyboard cases; `-dumpPIF` and `-showBuildSettings` corpora. Re-recorded for each Xcode release (27.1, 27.2 …).

**App Store canary** (M5, with the signing area): a Linux-built SwiftUI and Flutter build uploaded to TestFlight must process as VALID **and install from TestFlight** on the test phone [L§M5].

---

## 12. Ordered task list

Sizes: S ≤ 2 days, M ≤ 1 week, L ≤ 3 weeks, XL > 3 weeks (one agent). Tasks in the same group can run in parallel.

| ID | Task | Depends on | Size |
|---|---|---|---|
| B01 | `russet-plist`: XML/binary via `plist`; OpenStep reader/writer; Xcode macro expansion | – | M |
| B02 | `russet-build` skeleton: request, events, errors, report, project detection, `russet.toml` schema (9.2) with strict validation | B01 | M |
| B03 | SwiftPM backend: planning (describe/show-dependencies), builder generation, invocation, log parsing, toolset generation with the SDK fix, platform view consumption | B02, SDK area (`SdkFacts`, view) | L |
| B04 | Dynamic-product handling: automatic `native` fallback, framework wrapping, upstream issue with a reproducer (SwiftPM or swift-build) | B03 | M |
| B05 | `russet-macho`: parsing, lints L001–L009, rpath deletion, thinning, last-resort sdk patch (foreign binaries only, never default) | B01 | M |
| B06 | Bundle assembly and Info.plist synthesis: precedence, DT policy and normalization, bundle-ID policies and map, back-deploy embedding, extensions, PkgInfo, privacy manifest, `SigningPlan` | B03, B05 | L |
| B07 | `russet-ipa`: deterministic writer and reader, dev/store modes, `SwiftSupport` | B05 | S |
| B08 | Surfaces: CLI `build/clean/inspect/lint/package`, daemon methods 9.4, MCP tools 9.5 | B06, B07, core/MCP areas | M |
| B09 | `russet-tool` multi-call: `xcrun`, `codesign` no-op, `ld64.lld` trampoline (`-r` → libtool), closed-tool error shims, phase-A `actool` (loose PNGs, symbols, partial plist, dependency info) | B01 | M |
| B10 | `russet-car` and `russet-assets`, phase B: CAR/BOM writer and reader, colors, images, SVG, icons (single-size expansion), Xcode-27-format symbols; AssetKit differential test | B09, B11 | XL |
| B11 | Golden corpus repository and `russet oracle run` (actool, xcstringstool, launch storyboards) | – (Mac oracle) | M |
| B12 | `russet-xcstrings` (compile, dry-run, stringsdict, generate-symbols) and `russet-ib` launch-screen converter plus `ibtool` shim | B09, B11 | M |
| B13 | Flutter debug: engine artifacts, shims, `flutter assemble`, custom-device register and install, storyboard-free Runner, Runner build, assembly | B06, B09, device area | L |
| B14 | gen_snapshot CI (6.8), client `ensure`, release/profile AOT through the wrapper, dSYM and strip | B13 | L |
| B15 | Flutter plugins: SwiftPM package generation and `podspec2spm` conversion ladder | B13 | L |
| B16 | Xcode projects: `russet-xcodeproj`, `russet-pif`, `russet-swb`, the `adapt` interim path, dumpPIF corpus validation, CocoaPods through PIF | B06, B11 | XL |
| B17 | Windows native build plane: junction view, `.exe` shims, response files, short paths, `native` backend, Flutter-on-Windows spike | B03–B09 | L |
| B18 | Packaging lints for App Store (L010/L011 store mode), stamping report, `ship` hand-off | B06, B10 | S |
| B19 | React Native/Expo adapter (after B16) | B16 | XL |
| B20 | .NET MAUI feasibility spike (report only; study-only for the first build, ARCHITECTURE Q30) | B09 | S |
| B21a | `russet-ib` NIBArchive codec, IB XML model and the launch-storyboard compiler, plus a single-scene main storyboard with an ObjC custom class (no module mangling); `ibtool --compile/--compilation-directory/--link/--version` in `russet-tool` (PLAN W21b) | B09, B11, B12 | L |
| B21b | Main storyboards and XIBs: custom classes and modules, outlets and actions, segues, navigation and tab controllers, common controls, runtime attributes, SF Symbols, localization (PLAN W28b) | B21a | L |
| B21c | Tables, collections, split views, traits and size classes, accessibility configurations; NetNewsWire (PLAN W28c) | B21b, B16 | L |

**Acceptance criteria**

| ID | Acceptance |
|---|---|
| B01 | Round-trips the SDK's own `SDKSettings.plist`, platform `Info.plist` and a corpus `project.pbxproj` with byte-equal re-serialization (OpenStep: semantic equality). Macro tests cover `$(inherited)`, `:rfc1034identifier` and `:default=` |
| B02 | `russet build --json` on a non-project directory returns `config.projectNotFound` (1301; was `E_PROJECT_NOT_FOUND`). A typo in `russet.toml` reports file:line (`config.invalid`). Events stream to `.russet/logs/<jobId>.jsonl` (ARCHITECTURE §20.1) |
| B03 | On this host the M1 Hello app builds. `llvm-otool -l` shows `sdk 27.0 minos 17.0` on the executable **and** on every package dylib. The `.russet/work` scratch is inside the project. No `SDKROOT` leaks into the environment. **Device agent** re-runs `demo/m1-baseline.sh` against this build and it stops at the breakpoint |
| B04 | The I-DYNAMIC-PRODUCT fixture (local package with a `.dynamic` product, the [V19] reproducer) builds and runs. An upstream issue is filed with a minimal reproducer |
| B05 | Lints flag L002 on an xtool-built Hello (`sdk 17.0`) and L006 on its `/home/…` rpaths [V18]. After rpath deletion the file passes L005 and `llvm-objdump --macho --private-headers` stays valid |
| B06 | Info.plist of Hello contains the scene manifest and the DT values of 4.4 under `sdk`, none of them under `none`. Resource-bundle plists no longer contain `10989`/`99T999`. With `TeamSuffix` the app and appex IDs are rewritten consistently and the map is emitted. Two clean builds are byte-identical before signing |
| B07 | IPA listing matches the spec. `unzip -Z` (test-only) shows modes 755/644. Same SHA-256 across two runs. A Windows-built IPA has the same listing |
| B08 | The MCP tool `build` returns `app.path` and the first errors on a broken file. `job.events` notifications arrive in order with `seq`. `job.cancel` (or `$/cancelRequest`) stops `swift build` within 2 s (ARCHITECTURE §11.4, §20.1; the old names were `russet_build`, `build.event`, `build.cancel`) |
| B09 | A package with `.xcassets` builds through swiftbuild (xtool #310 scenario) with phase-A actool. `xcrun --sdk iphoneos --show-sdk-path` prints the SDK. `ld64.lld -r` works through the trampoline |
| B10 | For every corpus case: the rendition set and attributes equal `assetutil --info` of actool's output (ignoring compression codec and timestamp). The app icon shows on the phone (device agent). Output is deterministic |
| B11 | The corpus repository exists with the initial cases of 5.5. The runner leaves no files in the Mac's `/tmp` (checked after each run). Tool versions are recorded |
| B12 | The xcstrings corpus compiles to the same `.strings`/`.stringsdict` content as the oracle (semantic plist equality). Launch storyboards convert as classified: lossless for Flutter and the Xcode template; lossy, with reasons in the build report, for React Native and flutter_native_splash; `launch-storyboard` honours `auto`, `compile` and `convert` |
| B13 | `flutter run -d russet-<udid>` builds, installs and launches the counter app on the iPhone (device agent runs it). `Runner.app` has no storyboard keys. `App.framework/App` has `sdk 27.0`. Hot reload works (device area) |
| B14 | The CI publishes linux-x64 release and profile compilers for Flutter 3.47.7 that are byte-identical to the macOS reference. The client verifies SHA-256. A release build of the counter app runs AOT on the phone (device agent) |
| B15 | Counter app plus `shared_preferences` and `url_launcher` (SwiftPM) plus one CocoaPods-only Swift plugin (converted) builds and the plugins work on the phone. An unconvertible plugin gives `E_COCOAPODS_REQUIRED` naming it |
| B16 | Three Xcode projects chosen by their Interface Builder needs build from `.xcodeproj` and launch (device agent): IceCubesApp (no storyboards), the RN template's iOS part (B21a) and an Xcode "Storyboard App" equivalent (B21b); NetNewsWire follows B21c. PIF diff against the corpus is clean after normalization. `russet-swb` is published for Swift 6.4 linux-x64 |
| B17 | The M1 Hello and Flutter debug builds pass on the physical Windows PC, with the phone attached to Windows. The CI job on `windows-2022` builds the samples (no device). Also covered: the windows-x64 OpenAppleMacros server, the `llvm-lib.exe` and `ld64.lld.exe` shims, the long-path matrix (`LongPathsEnabled` 0/1 × native/swiftbuild) and the Windows `info.json` triples |
| B18 | `russet lint --store` flags each App Store rule in 4.7 on crafted fixtures. The report lists every stamped key |
| B19 | The Expo template app builds on Linux and runs with Metro (device agent) |
| B20 | A report listing each macOS-only pack and tool the iOS workload needs, with an effort estimate |
| B21a | The oracle corpus (§5.5) has decoded-graph equality with Xcode 27.0 `ibtool --cocoatouch-compiler-mode toolchain`; output is deterministic; unsupported elements give 4101 with file:line and no partial output; on the phone the RN launch screen's labels show and `UIStoryboard(name: "Main")` gives FlutterViewController (DC-32) |
| B21b | Xcode "Storyboard App" equivalents (Swift and ObjC) launch; an IB-harness mode of RussetProbe dumps equal view trees for Russet-compiled and oracle-compiled nibs (DC-33) |
| B21c | The IB harness covers B21c's elements with equal view trees; NetNewsWire iOS (4 storyboards, 4 XIBs) builds through `russet-swb` with `IBC_EXEC=russet-tool ibtool`, launches and navigates to Settings (DC-35) |

Milestones: M1/M3 = B01–B09 (B10 phase A suffices); M2 = B13; M4 = B10–B12 and B21a; M5 = B14 + B18; M6 = B17; M7 = B15 (CocoaPods part), B16, B19, B21b, B21c.

---

## 13. Risks (build-plane specific)

| ID | Risk | Mitigation |
|---|---|---|
| RB1 | **Turning on `sdk 27.0` activates iOS 27 linked-on-or-after behaviour.** Apps without a scene manifest may not launch (TN3187 [S9]); other SDK-gated changes are unknown | Always generate `UIApplicationSceneManifest`. Device re-test is part of B03's acceptance. A `[build] link_sdk_version = "deployment"` escape hatch for diagnosis only, never for release |
| RB2 | **Swift Build on Linux is young:** dynamic-product link failure [V19], bogus DT stamps [V9], actool resolution [V7] | Automatic `native` fallback (but deprecated, RB3), normalization, platform view. File upstream issues with reproducers (B04) |
| RB3 | **SwiftPM's `native` build system is deprecated** (warning printed [V19]) | Track SwiftPM. Get B04 fixed upstream. Long term, the `pif` backend |
| RB4 | **CAR acceptance** (format 970 vs 1010; App Store processing) [L§9 R9] | Corpus diffs, the TestFlight canary, the Apple-hardware release mode |
| RB5 | **Metadata stamping** (DT keys, CAR header strings) could be judged misrepresentation [L§9 R4] | Truthful SDK-provenance default, `xcode-compat` opt-in, everything reported. Owner decision Q1 |
| RB6 | **Flutter coupling:** engine ↔ gen_snapshot ↔ Dart; flutter_tools changes break the `assemble` + shim approach; the gen_snapshot wrapper can be overwritten by `flutter precache` | CI per stable tag, `doctor` checks, the self-run AOT fallback, a regression job per Flutter tag |
| RB7 | **gen_snapshot determinism on Windows** (MSVC evaluation order) | Gate Windows publishing on byte identity; ship Linux first |
| RB8 | **PIF schema churn** between Xcode and Swift Build versions | Generate from the corpus for each Xcode release; version `russet-swb` per Swift minor |
| RB9 | **Ruby/CocoaPods dependency; trunk read-only from 2026-12-02** | SwiftPM first, then conversion, then real CocoaPods only when needed |
| RB10 | **Disk:** a "normal" SDK is 6.8 GB [V11]; build scratch about 80 MB per app; Flutter engines | dSYM off in debug, `cache gc`, size reporting. Slim SDK plus the required-files list (3.4) |
| RB11 | **Licences:** xlinux GPL-3.0 (reuse fine); xcross AGPL and xcross_gen_snapshot unlicensed (facts only); hatch-ios has no LICENSE (reference only) [L§3.6] | Code review rule: no copied code from unlicensed or AGPL sources; record provenance in NOTICE |

---

## 14. Open questions

- **Q1 (owner): metadata default.** Should `DTXcode`/`DTXcodeBuild` (taken from the imported Xcode) be stamped by default (`metadata = "sdk"`), or only for store builds, or only on opt-in? The same question covers CAR header version strings.
- **Q2 (signing area and owner): free-account bundle-ID policy.** Default `<id>.<TEAMID>` or xtool's `XTL-<prefix>.<id>` (which allows in-place updates of apps already installed by xtool)?
- **Q3 (owner): Ruby.** Rely on the user's Ruby and CocoaPods, or ship a portable Ruby for the CocoaPods route (M7)?
- **Q4 (owner): where to host the gen_snapshot CI.** A public repository (free macOS reference runners, BSD artifacts) or the private Russet repository (paid minutes)?
- **Q5 (lead): SwiftPM apps on `pif`.** Once `russet-swb` exists, should SwiftPM apps move to the `pif` backend, so that Swift Build does the bundling exactly as Xcode does?
- **Q6 (SDK area): slim SDK contents.** Can the slim SDK keep the files listed in 3.4 (platform `version.plist`, back-deploy dylibs, Xcode `version.plist`)? Today only the 6.6 GB `Xcode.app` copy has them [V11][V20]. **Answered (2026-10-11):** yes; W16b's iOS-only want-set keeps them (ARCHITECTURE §16.3).
- **Q7 (lead): Windows build plane.** Is W1 split mode (build in WSL2) acceptable as the first Windows deliverable, before the native build plane (B17)? **Settled (2026-10-11, ARCHITECTURE [D 69]):** SDK and DDI import are always native on Windows; the build plane is native-first (B17, PLAN W27b); WSL2 split mode is only a documented fallback if Swift Build or the native backend fails on Windows.

---

## Appendix A: Verification log

All commands ran on this host (CachyOS, Swift 6.4, xtool SDK `epoch=3,darwinTools=1.1.0,oam=1.3.0` from Xcode 27.0 27A266a) inside the session scratchpad on tmpfs, unless marked "Mac". No device commands were run. Scratch outputs (about 220 MB) were deleted afterwards. The Mac temp directories were removed (an EXIT trap, plus one manual cleanup after an aborted run).

| ID | What | Command (abridged) | Result |
|---|---|---|---|
| V1 | xtool toolset contents | `cat ~/.swiftpm/swift-sdks/darwin.artifactbundle/{toolset.json,toolset-swb.json,toolset/bin/ld64.lld}`; `llvm-lib --help` | `ld64.lld` is a `sh` trampoline (`-r` → `llvm-lib -static -filelist … -dependency_info … -o …`, else `orig/ld64.lld`, which is LLD 21.1.6). `llvm-lib` is `llvm-libtool-darwin` 21.1.6. `toolset.json` adds `-Xfrontend -enable-cross-import-overlays -use-ld=lld` |
| V2 | Baseline Swift Build of the M1 Hello copy | `env -u SDKROOT XCODE_EXTRA_PLATFORM_FOLDERS=… swift build --build-system swiftbuild --package-path xtool/.xtool-tmp --triple arm64-apple-ios --toolset …/toolset-swb.json --scratch-path .build` | "Build complete! (6.89 secs)". `LC_BUILD_VERSION platform 2, sdk 17.0, minos 17.0`. SwiftPM writes `.build/manifest.pif` |
| V3 | Link job of swiftc | `swiftc … -emit-executable -sdk <SDK> -use-ld=lld … -driver-print-jobs` | The link runs `clang … -fuse-ld=lld --ld-path=… --sysroot <SDK> --target=arm64-apple-ios17.0 …` |
| V4 | clang sysroot vs SDK version; global SDKROOT | `clang -### -fuse-ld=lld --sysroot <SDK> --target=arm64-apple-ios17.0 x.o` vs `-isysroot <SDK>` vs `SDKROOT=<SDK>`; then `SDKROOT=<SDK> swift build …` | `--sysroot` → `-platform_version ios 17.0.0 17.0.0`. `-isysroot` or `SDKROOT` → `17.0.0 27.0`. A global `SDKROOT` breaks the host manifest link: `no such file … iPhoneOS27.0.sdk/usr/lib/swift/linux/x86_64/swiftrt.o` |
| V5 | Per-target fix | builder `linkerSettings.unsafeFlags` += `-Xclang-linker -isysroot -Xclang-linker <SDK>` | App executable `sdk 27.0, minos 17.0` |
| V6 | Toolset-level fix | `toolset-russet.json` with `swiftCompiler.extraCLIOptions` = the same four arguments | Package dylib `libDyn.dylib` `sdk 27.0` (it had `sdk 17.0` with the per-target fix). Plain Hello app executable `sdk 27.0`. Same with `--build-system native` and the flag in `toolset.json` |
| V7 | actool lookup | package with `Assets.xcassets`, fake `actool` on `PATH` | Swift Build ignores `PATH`. It runs `<pkg>/xtool/.xtool-tmp/actool --version --output-format xml1` → "is not an executable file" (= xtool #310) |
| V8 | Platform view | `Developer/Platforms/iPhoneOS.platform/{Developer,Info.plist}` symlinked, `usr/local/bin/actool` = logging stand-in; `XCODE_EXTRA_PLATFORM_FOLDERS=<view>` | Build OK. Captured the two compile command lines in 5.2 (task env `{}`, cwd = package root). The planning probe got the full environment. `Hello_Hello.bundle/{Assets.car,Info.plist}` produced |
| V9 | DT keys Swift Build stamps on Linux | `plistutil -i …/Hello_Hello.bundle/Info.plist` | `DTXcode=10989`, `DTXcodeBuild=99T999`, `DTPlatformBuild=""`, `DTSDKBuild=24A430`, `DTSDKName=iphoneos27.0`, `DTPlatformVersion=27.0`, `DTCompiler=com.apple.compilers.llvm.clang.1_0` |
| V10 | lld behaviours | clang `-dynamiclib` stub with `-isysroot`; then with a trailing `-Xlinker -platform_version ios 15.0 26.4`; then system `/usr/bin/ld64.lld` | Stub `sdk 27.0, minos 15.0, tool 4`. The trailing `-platform_version` wins (`sdk 26.4`). System lld 23.1.1: `libSystem.tbd … unknown target arm64e.x1` |
| V11 | SDK facts | python `plistlib` on the platform `Info.plist` (`AdditionalInfo`, `DefaultProperties`), `SDKSettings.plist`, SDK `SystemVersion.plist`, `Xcode.app/…/iPhoneOS.platform/version.plist`, `Xcode.app/Contents/version.plist` | DT templates as in 4.4; SDK build 24A430; platform build 24A430; Xcode 27.0 / 27A266a; valid deployment targets 15.0–27.0; `INFOPLIST_OUTPUT_FORMAT=binary`; `STRINGS_FILE_OUTPUT_ENCODING=binary`; `SwiftCompatibilityLibraryNotNeededStartingIn` Span 26.0, Concurrency 15.0. The SDK bundle is 6.8 GB (the `Xcode.app` copy alone is 6.6 GB) |
| V12 | xtool bundle-ID prefix | read `Sources/XKit/Utilities/ProvisioningIdentifiers.swift` and `DeveloperServicesAddAppOperation.swift` @ d802c71 | `XTL-<first segment of identityID, uppercased>.<bundleID>` for every account |
| V13 | Mac: actool symbols mode | `xcrun actool A.xcassets --compile out-sym … --generate-swift-asset-symbols … --generate-objc-asset-symbols … --generate-asset-symbol-index …` (temp directory) | Writes only the three symbol files; no `out-sym/`, no partial plist. Captured the Swift symbol and index formats (5.2) |
| V14 | Mac: actool compile mode | same catalog, `--app-icon AppIcon --accent-color AccentColor --target-device iphone --target-device ipad --minimum-deployment-target 17.0` (and 15.0, iPhone-only) | Missing output dir is an error. Outputs: `Assets.car` (26,568 B), `AppIcon60x60@2x.png` (120 px), `AppIcon76x76@2x~ipad.png` (152 px), the partial plist in 5.2, dependency info `00 "actool-25098" 10 <catalog> 40 <outputs…>`, xml1 `compilation-results` |
| V15 | Mac: CAR header | `xcrun assetutil --info out/Assets.car` | CoreUIVersion 1010, StorageVersion 17, SchemaVersion 2, 9-attribute key format, `AssetStorageVersion "Xcode 27.0 (27A266a) via AssetCatalogAgent-AssetRuntime"`, Platform ios, PlatformVersion = minimum deployment target, Timestamp. Renditions: Color, Icon Image 1024 ×2 (lzfse), MultiSized Image ×2, Image @2x (deepmap2) |
| V16 | Mac: tool versions and help | `xcrun actool --version --output-format xml1`; `ibtool --version …`; `xcstringstool --help/compile --help/generate-symbols --help`; `lipo -archs` | actool and ibtool `bundle-version 25098`, `short-bundle-version 27.0`. actool has no `--help` (errors come back as a `com.apple.actool.errors` plist). xcstringstool has no `--version`; its subcommands and options are as in 5.3. All three are arm64-only |
| V17 | Swift Build container loading | read `Sources/SWBBuildService/Messages.swift` and `SwiftBuild/SWBBuildServiceSession.swift` | `.xcodeproj`/`.xcworkspace` → `xcrun xcodebuild -dumpPIF`; `.json`/`.pif` read directly; `sendPIF` API |
| V18 | rpaths | `llvm-otool -l …/Hello.app/Hello` (xtool) and our build | `/usr/lib/swift`, `@loader_path`, `/home/martinv/orchard-lab/Hello/.build/out/Products/Debug-iphoneos/PackageFrameworks`, `/usr/lib/swift/usr/lib/swift-6.2/iphoneos`, `@executable_path/Frameworks` |
| V19 | `.dynamic` product | local package `Dyn` with `type: .dynamic`; swiftbuild twice (with and without an explicit `-lDyn`); native once | swiftbuild: loose `libDyn.dylib` (install name `@rpath/libDyn.dylib`); `ld64.lld: error: undefined symbol: $s3Dyn11dynGreetingSSyF`, persistent; `EagerLinkingTBDs` empty. native: "Build complete", `Hello-App` loads `@rpath/libDyn.dylib`, both binaries `sdk 27.0`. Warning: "'--build-system native' has been deprecated" |
| V20 | Back-deploy dylibs | `ls …/Xcode.app/…/XcodeDefault.xctoolchain/usr/lib/swift-*/iphoneos` | `swift-6.2/iphoneos/libswiftCompatibilitySpan.dylib`, `swift-5.5/iphoneos/libswift_Concurrency.dylib` (plus static compat libraries). Absent from the bundle-root `Developer/Toolchains` copy |
| V21 | Load commands | `llvm-otool -l` on the xtool Hello and on the SDK's Apple-built `XCTest` | Both have `LC_ENCRYPTION_INFO_64`, `LC_DYLD_CHAINED_FIXUPS` and `LC_FUNCTION_STARTS`. Apple's also has `LC_SOURCE_VERSION`, and its tool entry is `3 (ld) 27037.1`; lld writes `4 (lld) 21.1.6` |

## Appendix B: Sources

- [S1] swift-driver PR #2158 "Darwin needs -isysroot instead of --sysroot to take precedence over SDKROOT" (merged 2026-07-15): https://github.com/swiftlang/swift-driver/pull/2158. `Sources/SwiftDriver/Jobs/DarwinToolchain+LinkerSupport.swift` on `release/6.4.0`, `release/6.4.2`, `release/6.4.x` (still `--sysroot`, lines 224–226) vs `main` (`-isysroot`, line 233): https://github.com/swiftlang/swift-driver/blob/release/6.4.x/Sources/SwiftDriver/Jobs/DarwinToolchain%2BLinkerSupport.swift
- [S2] swift-build `Ld.xcspec` (`LINKER_DRIVER`, `CLANG_SYSROOT_LINKER_INPUT` → `-isysroot`, `SWIFTC_SDKROOT_LINKER_INPUT` → `-sdk`): https://github.com/swiftlang/swift-build/blob/main/Sources/SWBUniversalPlatform/Specs/Ld.xcspec
- [S3] swift-build toolset → settings mapping test (`swiftCompilerFlagsPassedToLinkerDriver`): https://github.com/swiftlang/swift-build/blob/main/Tests/SWBCoreTests/SwiftSDKToolsetSettingsTests.swift. SwiftPM `SwiftBuildSystem.swift` (`SWIFT_SDK_TOOLSETS`, `OTHER_LDFLAGS`): https://github.com/swiftlang/swift-package-manager/blob/main/Sources/SwiftBuildSupport/SwiftBuildSystem.swift
- [S4] swift-build Apple platform plugin, `additionalPlatformExecutableSearchPaths`: https://github.com/swiftlang/swift-build/blob/main/Sources/SWBApplePlatform/Plugin.swift (lines 328–342)
- [S5] xtool @ d802c71 (main, 5 commits after 1.21.0 = d76498a3d7): `Sources/PackLib/{BuildSettings,Packer,Planner,PackSchema}.swift`, `Sources/XToolSupport/ProcessZIPCompressor.swift`, `Sources/XKit/Utilities/ProvisioningIdentifiers.swift`: https://github.com/xtool-org/xtool. Issue #310: https://github.com/xtool-org/xtool/issues/310
- [S6] swift-build `InfoPlistProcessorTaskAction.swift` (elided keys, scene-manifest generation, `AdditionalInfo` merge, `UIRequiredDeviceCapabilities`), `SDKRegistry.swift` (`SystemVersion.plist`), `PlatformRegistry.swift` (`version.plist`), `Core.swift` (Swift-toolchain mode → Xcode 99.99.99 / 99T999): https://github.com/swiftlang/swift-build/tree/main/Sources/SWBTaskExecution/TaskActions and https://github.com/swiftlang/swift-build/tree/main/Sources/SWBCore
- [S7] swift-build `Package.swift` products (`swbuild`, `SwiftBuild`, `SWBProjectModel`), `Sources/SwiftBuild/ConsoleCommands/SWBServiceConsoleBuildCommand.swift`, `SWBBuildServiceSession.swift`, `Sources/SWBBuildService/Messages.swift`: https://github.com/swiftlang/swift-build
- [S8] swift-build `AssetCatalogCompiler.xcspec`/`.swift`, `InterfaceBuilderShared.swift` (version-plist parsing), `XCStrings.xcspec`/`XCStringsCompiler.swift`, `IBStoryboardCompiler.xcspec`, `IBCompiler.xcspec`: https://github.com/swiftlang/swift-build/tree/main/Sources/SWBApplePlatform (research copies in `~/RE/ios-nomac/research/closed-build-tools/swift-build/`)
- [S9] Apple TN3187 "Migrating to the UIKit scene-based life cycle": https://developer.apple.com/documentation/technotes/tn3187-migrating-to-the-uikit-scene-based-life-cycle (research copy `closed-build-tools/apple-docs/tn3187.md`, lines 35–36)
- [S10] AssetKit (MIT; CoreUI 970; last push 2026-05-25; 1 star): https://github.com/xtool-org/AssetKit
- [S11] algoriddim/xcstringstool (Rust, MIT): https://github.com/algoriddim/xcstringstool
- [S12] Dart SDK commit c8495e3003 "[vm] Add --macho-sdk-version for specifying the SDK version" (2026-08-19), `runtime/vm/mach_o.cc`: https://github.com/dart-lang/sdk/commits/main/runtime/vm/mach_o.cc
- [S13] Dart 3.13.5 `runtime/vm/mach_o.cc` (sdk = minos) and `runtime/bin/gen_snapshot.cc` (`--macho`, `--macho-object`): https://github.com/dart-lang/sdk/blob/3.13.5/runtime/bin/gen_snapshot.cc
- [S14] michaelwright235/nibarchive (Rust, MIT OR Apache-2.0; crates.io 0.1.0, 2025-05-28; no trailer support): https://github.com/michaelwright235/nibarchive
- [S15] Flutter `packages/flutter_tools/lib/src/base/build.dart` (AOT args, dsymutil, strip) and `build_system/targets/ios.dart` (stub `App.framework`, debug assets, `MinimumOSVersion`): https://github.com/flutter/flutter/tree/master/packages/flutter_tools/lib/src (research copies in `flutter-crossplat-factcheck/fl/`)
- [S16] Flutter `custom_devices/custom_device_config.dart` (keys, platform restriction) and `custom_device.dart` (interpolation, VM-service discovery): https://github.com/flutter/flutter/tree/master/packages/flutter_tools/lib/src/custom_devices
- [S17] Flutter `flutter_cache.dart` (`ios/artifacts.zip`, `ios-profile`, `ios-release`): research copy `flutter-crossplat-factcheck/fl/packages_flutter_tools_lib_src_flutter_cache.dart` lines 924–926
- [S18] Flutter app template `ios/Runner` (`AppDelegate.swift` with `FlutterImplicitEngineDelegate`, `SceneDelegate: FlutterSceneDelegate`, `Info.plist` scene manifest with `UISceneStoryboardFile Main`) @ abaf9c52: research copies `closed-build-tools/flutter/`
- [S19] Flutter `macos/swift_package_manager.dart` (`FlutterGeneratedPluginSwiftPackage` static, `FlutterFramework` package): https://github.com/flutter/flutter/blob/master/packages/flutter_tools/lib/src/macos/swift_package_manager.dart
- [S20] xlinux (GPL-3.0), `xlinux/adapters/flutter/{build,plugins,custom_device}.py`, `support/flutter/FlutterLinuxSceneDelegate.swift`: https://github.com/cesardev31/xlinux
- [S21] iOSBuildX (MIT), `11-gensnapshot.sh` (`dart_target_os_override`, `tools/gn.py --mode product --arch simarm64`, `dart_use_compressed_pointers=false`): research copy `flutter-crossplat/community/iOSBuildX/`
- [S22] tuist/XcodeProj (MIT): https://github.com/tuist/XcodeProj. apple/xcode-project-format (Apache-2.0): https://github.com/apple/xcode-project-format
- [S23] joshuaswarren/omarchy-apple-dev (MIT): `tools/ibtool`, `tools/nibarchive.py`, `tools/keyorder.py`, `receipts/2026-10-08-ibtool-flutter-storyboards.md`: https://github.com/joshuaswarren/omarchy-apple-dev (its `tests/ibtool/golden/*.nib` are raw Apple outputs and are never vendored)
- [S24] microsoft/WinObjC (MIT, archived 2022) `tools/vsimporter/xib2nib`: https://github.com/microsoft/WinObjC
- [S25] tokenbleed/nibkit (Go, MIT; NIB decoder): https://github.com/tokenbleed/nibkit
- [S26] MatrixEditor/nibarchive (Python, GPL-3.0; reference only for an MIT crate): https://github.com/MatrixEditor/nibarchive
- Rust crates (crates.io, checked 2026-10-11): `plist` 1.10.1 (MIT), `zip` 9.0.1 (MIT), `goblin` 0.10.7 (MIT), `object` 0.40.0 (Apache-2.0 OR MIT), `lzfse` 0.2.0 (MIT; C reference BSD-3), `lzfse_rust` 0.2.1 (MIT OR Apache-2.0), `image` 0.25.10 (MIT OR Apache-2.0), `resvg` 0.48.1 (Apache-2.0 OR MIT).
