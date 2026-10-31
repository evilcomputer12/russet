# Decision log

Decisions agreed with the project owner. Newest first.

## 2026-10-11: Owner answers (ARCHITECTURE §21)

- **Q33.** Approved for the device lane:
  - one iPhone reboot plus a DDI replacement during W9 (DC-28);
  - later, a second reboot or a forced re-mount (DC-29).

  The Apple-origin iOS DDI (27A266a) was copied read-only from the owner's Mac to
  `~/russet-lab/ddi/27A266a/iOS` on 2026-10-11. It stays out of git.
- **Q34.** Before going public at G1:
  - the private track's details live only in the private repository's decision log;
  - this repository is published from a fresh, squashed history.
- **Q6.** Stay on GitHub Free with no payment method, so CI can never cost money. Go
  public at G1.
- **Q27.** The owner will re-download `Xcode_27.0.xip` when W16b asks for it, and keep it
  until the Linux and Windows import tests pass.
- **Q28.** The TXM premise in "Test devices" is corrected (see that entry).

## 2026-10-11: Accounts, CI and test devices

- **Developer account changes (Wave 2).** Russet may register one bundle ID (for example
  `com.evilcomputer12.russet.hello`) and create one development profile, reusing the
  existing development certificate (valid until 2027-10-10). Nothing is revoked or
  deleted.
- **CI budget.** GitHub's free private-repo minutes only:
  - Linux and Windows jobs run in GitHub Actions.
  - macOS reference/golden jobs run on the owner's Mac (the oracle), not on paid macOS
    runners.
- **Device tests.** No always-on self-hosted runner. Device suites run on demand,
  triggered by an agent in the working session; device access stays serialized.
- **Test devices.** iPhone 12 (A14, iOS 27) only.
  - Correction (2026-10-11, verified from the iPhone13,2 iOS 27.0 BuildManifest and
    kernelcache): the A14 switched from PPL to SPTM/TXM at iOS 27.0 and runs the same TXM
    image as A15+ devices.
  - So the rig *does* exercise the TXM JIT path (Flutter debug on iOS 26/27). Only
    behaviour specific to other SoCs needs recorded traces or community reports.
- **xtool credential files** were locked down to 0600 (2026-10-11). Run
  `xtool auth logout` once Russet's own provisioning works.

## 2026-10-11: Simulator

- **Device mirror** serves as the simulator replacement in the first build: the real iPhone
  appears live in the IDE with input and hot reload.
- **Rejected:** running Apple's iOS Simulator natively on Linux or Windows. The iOS 27
  simulator runtime is arm64-only and needs the macOS kernel, macOS frameworks and Metal.
- **Virtual iPhone:** a separate private track; it never blocks this repository.

## 2026-10-10: Project foundations

- **Goal.** Anyone can develop, build, install, debug, test and ship iOS apps from Linux
  and Windows with a physical iPhone, without needing a Mac. Headless operation must
  also work.
- **Name.** Russet (an apple variety, and a nod to Rust). The IDE is "Russet"; the CLI is `russet`. Renamed from "Orchard" on 2026-10-11 to avoid clashing with yaelliethy/orchard (macOS on QEMU) and OpenAI/Cirrus Orchard (Tart VMs).
- **License.** ~~GPL-3.0-or-later; pieces meant for upstreaming may be MIT where compatible.~~ Superseded
  2026-10-31 by the owner: **AGPL-3.0-only** for all of Russet, layer F included, so nobody can ship a closed
  fork or run a modified Russet as a service without sharing its source. Contributions use the DCO, no CLA
  (ARCHITECTURE §19).
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
