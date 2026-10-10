# Decision log

Decisions agreed with the project owner. Newest first.

## 2026-10-10: Project foundations

- **Goal.** Anyone can develop, build, install, debug, test and ship iOS apps from Linux
  and Windows with a physical iPhone, without needing a Mac. Headless operation must
  also work.
- **Name.** Russet (an apple variety, and a nod to Rust). The IDE is "Russet"; the CLI is `russet`. Renamed from "Orchard" on 2026-10-11 to avoid clashing with yaelliethy/orchard (macOS on QEMU) and OpenAI/Cirrus Orchard (Tart VMs).
- **License.** GPL-3.0-or-later. Pieces meant for upstreaming may be MIT where
  compatible.
- **Core language.** Rust. The device plane builds on `idevice` (MIT).
  `pymobiledevice3` is used as a reference and fallback.
- **GUI.** Our own IDE built on Code-OSS, with dedicated buttons, plus a bundled
  Russet extension.
- **GUI features in the first build.**
  - Always included:
    - device list
    - Build, Run, Debug and Hot Reload buttons
    - live logs
  - Also in the first build:
    - live iPhone screen mirror
    - project editors (Info.plist, signing and capabilities, entitlements, assets)
    - publishing panel (TestFlight / App Store)
    - profiling graphs and crash reports
- **AI agents.** A built-in MCP server and an agent panel. Day-one targets:
  - Claude Code
  - Codex CLI and Gemini CLI
  - local models (llama.cpp, OpenHands)
  - Copilot, Cline and Continue
- **Stacks.** In order:
  1. SwiftUI + SwiftPM
  2. Flutter
  3. Xcode projects (UIKit/ObjC, storyboards, asset catalogs, CocoaPods)
  4. React Native / Expo
  5. .NET MAUI
- **Accounts.**
  - Paid: App Store Connect API team key (role "App Manager"). This is the default.
  - Free: Apple ID, with anisette generated locally from Apple's Android libraries,
    downloaded at runtime and never shipped.
- **Releases.** Every path is in scope:
  - Linux-native upload through the App Store Connect API
  - an "Apple hardware" release mode (Xcode Cloud / macOS CI)
- **Platforms.** Linux first, then Windows, tested on a physical Windows PC.
- **Repository.** `evilcomputer12/russet`, private until M1 works.
- **Working mode.** Build in one continuous push and check in only when blocked.
- **Mac usage.** The owner's Apple Silicon Mac (macOS 27.0.1, Xcode 27.0) is only a
  development-time reverse-engineering oracle over SSH. Rules:
  - CLI tools may be installed.
  - No security changes (SIP stays on).
  - The product must never require a Mac.
