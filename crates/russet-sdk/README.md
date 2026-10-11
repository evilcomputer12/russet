# russet-sdk

SDK facts, platform view, toolsets, SDK and DDI import.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W5 (Wave 1); `src/import/` W16b, `src/import/windows/` W27b.
- `facts::BundleFacts::read` reads a Darwin SDK bundle in place (`SDKSettings`, `SystemVersion.plist`, the platform
  `Info.plist` `AdditionalInfo`, the platform and Xcode `version.plist`s, the back-deployment dylib directories, the
  Swift version of the SDK's own interfaces); a missing file is `sdk.incomplete` naming it.
- `XtoolBundleProvider` (an `SdkProvider`) picks the newest bundle in SwiftPM's `swift-sdks` directories that matches
  the selector, materializes the platform view (`data/sdk/<id>/view`, symlinks; rebuilt atomically when its links
  change) and writes `toolset-swb.json` / `toolset-native.json` with the link-time SDK fix (ARCHITECTURE §16.4).
- `host::HostToolchain` is the Swift toolchain and the LLVM ≥ 23 rewriters (W3's resolver supplies them in the
  daemon; `probe()` finds `swift` on `PATH` otherwise). `checks()` registers `sdk.buildFacts`.
- Pre-created module files: `src/import/mod.rs` (W16b) and `src/import/windows/mod.rs` (W27b).
