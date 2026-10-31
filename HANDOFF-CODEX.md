<!--
SPDX-License-Identifier: AGPL-3.0-only
SPDX-FileCopyrightText: 2026 Russet contributors
PRIVATE handoff. Contains the commercial/Windows direction — strip or move §8 before the public G1 squash.
-->

# Russet — handoff to Codex

**Owner's directive (Martin, 2026-10-30), verbatim intent:**
> I want a **full Xcode / Android-Studio-class experience** — everything inside the app, on buttons,
> **no shell commands**. That includes an **embedded virtual iPhone with live debugging** (like the
> Android Studio emulator / Xcode simulator), with the **real device and the virtual device both
> first-class run/debug targets**. And I want a **Windows app that uses Docker (NOT WSL)** for the
> Linux build/toolchain backend, as a closed-source Microsoft Store product.

Everything below is the full context to make that happen. **Read `AGENTS.md` first** (Codex auto-loads it),
then `docs/PLAN.md` (the 31 workstream cards) and `docs/ARCHITECTURE.md` (binding contracts). This file is the
"what Martin wants + everything the last session learned" layer on top of those.

---

## 1. What Russet is

An open-source kit (Rust core, GPL-3.0) to **build, sign, install and debug iOS apps from Linux and Windows with
a real iPhone and no Mac** — Flutter, SwiftUI/SwiftPM, Xcode projects, React Native, MAUI. Plus a **virtual iPhone**
(iOS 27 emulator) for device-free run/debug. The GUI is a VS Code / Code-OSS IDE with dedicated buttons and panels.
Repo: `github.com/evilcomputer12/russet` (**private** until the public G1 squash). Dir: `~/russet`.

## 2. Current state (verified 2026-10-30)

- **`main` @ `ae1fe20`, CI green** (fmt, clippy -D warnings, nextest, deny, guard incl. REUSE, schema). Pushed.
- **Wave 1 complete and proven on the real iPhone 12** (iOS 27): build → sign → install → launch → breakpoint,
  all with Russet's own code. The #1 technical risk is retired.
- **Wave 2 merged**: W11 run+debug II (DAP hub, crash store, symbol cache, Swift Quick Relaunch), W12 signing II
  (provisioning apply, approvals, deterministic .xcarchive/IPA/dSYMs), W13 MCP (47-tool agent server + agent-config
  for claude-code/codex/gemini/vscode), W14 IDE extension (manifest, Run/Build/Stop, **F5 debug stops at
  breakpoint**, a **"Virtual iPhone" placeholder panel**, Windows **named-pipe** transport), W15 QA II (RDT
  record/replay, threat model, trace lint).
- **W16 `project.init` done** (this session): `russet project init <name>` CLI + IDE **New Project…** command +
  0BSD templates in `templates/swiftpm-app/`. Creates a SwiftUI SwiftPM app that `project detect` recognises and
  `build`/F5 can run. Verified end to end.
- **IDE extension** `russet.russet@0.6.0`. Panels: Devices, Project, Run Configurations, Signing, Agent Activity,
  **iPhone (virtual, placeholder)**, Agents, Console. Commands: run/debug/build/stop/hotReload/hotRestart/clean,
  New Project, select device/target/config, doctor, import Xcode, account (ASC key/Apple ID), device
  (pair/mount DDI/install), console/mirror/screenshot.
- **Real iPhone 12** (`iPhone13,2`, iOS 27.0.1 build 24A446, txm signing) is detected: `russet device list` →
  `state: ready`, transport usb. Full pipeline works against it.
- **Binaries**: `russet` (CLI; the daemon is **embedded** — `russet daemon run` — because `russetd` is a *library*,
  not a separate binary). Release build is ~311 MB because release keeps `debug = line-tables-only`; add
  `[profile.release] strip = true` (or `strip`) for a lean installer build.

## 3. Repo map / where to read

- `AGENTS.md` — the rules (worktrees, seam files, lane model, device rule, build hygiene, DoD). **Binding.**
- `docs/PLAN.md` — 31 workstream cards (W1–W31), waves, ownership, the device-check queue.
- `docs/ARCHITECTURE.md` — contracts: §5.3 APIs, §8 methods, §9 events, §13 error codes, §14 files. **Binding.**
- `crates/` — ~40 crates. Key: `russet-core` (errors/ids), `russet-protocol` (RPC DTOs + registry),
  `russetd` (the daemon library + handlers), `russet-cli` (`russet` binary), `russet-build` (SwiftPM/project
  detection/**scaffold**), `russet-sign`, `russet-device` (CoreDevice/DDI), `russet-debug` (LLDB + **DAP** +
  gdb-remote), `russet-mcp`, `russet-flutter`, `russet-ship` (archive/IPA/TestFlight).
- `extension/` — the VS Code/Code-OSS IDE extension (TypeScript; esbuild + vite; webviews).
- `templates/` — 0BSD project templates (currently `swiftpm-app/`).
- `docs/lab/queue.md` — device checks (lane A runs them on the rig).

## 4. What works today vs. what's shell-only (the gap Martin hates)

| Capability | In the app (button) | Shell-only today |
|---|---|---|
| New project | ✅ New Project… | — |
| Build / Run / Debug (F5) on **real** iPhone | ✅ | — |
| Device list / pair / DDI | ✅ panel + commands | — |
| Signing / accounts | ✅ panel + commands | — |
| **Virtual iPhone (emulator)** | ❌ **placeholder panel** | ⚠️ `~/russet-lab/thirdparty/vphone-linux/run-vphone.sh` |
| Live debugging on the **virtual** device | ❌ | ❌ (not wired) |
| One-click install (no PATH/shell setup) | ❌ | ⚠️ manual binary + PATH |

**The headline job: close the bottom three rows.** Make the emulator and its debugging live *inside* the IDE,
and ship a one-click install so there is never a terminal step.

## 5. The product vision (what "done" looks like)

A single Russet IDE window that is a true Xcode/Android-Studio peer:

1. **Device bar** (top): a dropdown of run destinations mixing **real iPhones** and **virtual iPhones**, exactly
   like Xcode's scheme/destination picker or AS's device chooser. Start/stop a virtual device from here.
2. **Embedded Virtual iPhone panel**: the emulator's live screen *inside* the IDE (not a separate QEMU/reims
   window), with a toolbar — power, home, volume, rotate, screenshot, record — and **click→tap, drag→swipe**
   input. Like the AS emulator pane.
3. **Run/Debug on either target**: F5 builds → installs → launches → **breakpoints, stepping, variables, `po`,
   console** — against a real *or* virtual iPhone. The DAP hub (W11) already multiplexes LLDB; point it at the
   virtual device's debugserver.
4. **Everything else on buttons**: provisioning, TestFlight upload, crash symbolication, profiling, Flutter
   hot-reload. Most of the daemon methods exist; the IDE just needs the surfaces.
5. **One-click install**: Linux AppImage / Windows MSIX that bundles the `russet` engine, the toolchain and the
   IDE. No PATH edits, no `npm`, no manual daemon. (This session had to hand-fix PATH and reinstall binaries —
   that must never be a user step.)

## 6. Next build tasks, prioritized

**A. Embed the Virtual iPhone in the IDE (top priority — the biggest "app not shell" gap).**
   - **Daemon**: a `virtual`/`vdevice` backend + handlers that **start / resume / stop / status** a vphone VM
     (the shell-equivalent of `run-vphone.sh` / `resume-vphone.sh`), **stream frames**, and **forward input**.
     Register it as a device backend so it appears in `device.list` alongside USB devices (give virtual devices a
     synthetic UDID and `transport: "virtual"`).
   - **Protocol**: `virtual.start/resume/stop/status/screenshot/input/snapshot` (follow the `rpc_method!` +
     schema-regen pattern; new error family or extend `build`/a new `virtual` family).
   - **Extension**: replace the placeholder `MirrorViewProvider`/"Virtual iPhone" webview with a live view:
     an `<img>`/`<canvas>` fed by frames, a device-chrome toolbar, and pointer→input forwarding. Reuse the mirror
     plumbing (`extension/src/panels/mirrorView.ts`, `webview/mirror/`).
   - **Run/debug target**: make a running virtual device selectable in the destination picker; F5 should install
     and debug on it via the DAP hub. This is the "live debugging on the virtual iPhone like Android Studio."

   **Emulator technical facts (from `~/russet-lab/thirdparty/vphone-linux/`):**
   - Launch: `run-vphone.sh <vm-dir> [qemu args]`. QEMU `-M vresearch101`, TCG (slow cold boot ~90 min; **always
     prefer snapshot/resume**: `snapshot-vphone.sh` then `resume-vphone.sh` — seconds). `GFX=1` attaches
     **reims-vgpu** (Apple-gfx → Metal → Vulkan via metal2vulkan; AMD/RADV). The black-screen bug was fixed this
     session (IOSurface memfd passing) — it **renders the iOS 27 home screen**.
   - Display: QEMU runs `-display none`; **reims-vgpu owns the output window** (`REIMS_VGPU_WINDOW`), a native
     Vulkan surface. **So embedding the live screen is the hard part** — options for Codex to scope: (a) add a
     frame-export/stream path to reims-vgpu (export the composited surface to a shared buffer / encode to
     H.264/JPEG and pipe to the webview); (b) a VNC/pixel-stream shim; (c) QMP `screendump` if a QEMU display is
     attached instead of reims. `tools/screen-changed.py` and `tools/qmp.py` exist; the Inferno track proved
     QMP `screendump` + `input-send-event` works for a QEMU-displayed VM.
   - Input: iOS takes a **USB touchscreen** (hub port 2.1) as the digitizer; buttons via QMP `send-key`; a
     control channel **vphoned** on **vsock port 1339** (`tools/vphoned.sh`). Taps/swipes = absolute pointer
     events (0..32767) like the Inferno `qmp-input.py`.
   - A VM bundle (Disk.img, nvram.bin, ROMs, `ecid`) lives at `~/vphone-setup-work/devices/…`. These are
     Mac-origin research bundles — **never redistribute**; keep local.
   - `reims-vgpu/` is in-tree (LGPL — must run **out-of-process** in any public/commercial build; see licensing).

**B. Windows app with Docker (NOT WSL) — Martin's explicit requirement.**
   - **Model**: the Windows GUI (the IDE) talks to the **Russet daemon running inside a Linux Docker container**
     that carries the Swift/Flutter toolchain + Darwin SDK. Docker instead of making the user set up WSL2 by hand.
     (Docker Desktop's own backend may be Hyper-V/WSL internally, but the *user* installs Docker, not a WSL distro;
     ship a `Dockerfile`/compose and pull a prebuilt image.)
   - **Transport**: the extension already speaks a **Windows named pipe** (W14) and the daemon binds one; for a
     container, expose the daemon over **TCP** (loopback, token-authed — `russet-ipc` already has a TCP endpoint
     with a bearer token) and have the Windows IDE connect to `localhost:<port>` mapped from the container.
   - **iPhone USB → container**: the hard part. Solve device passthrough: `usbipd-win` to attach the iPhone to the
     container's Linux side, or a usbmuxd bridge (run `usbmuxd` in the container and forward the USB device). The
     Linux device stack (`idevice`/CoreDevice) then sees it. Document and automate this in the installer.
   - **Package**: MSIX for the Microsoft Store, bundled + auto-updating, that installs Docker-engine dependency
     (or guides it), pulls the image, and "just works". That bundling *is* the paid product.
   - **Build on Windows**: `russet-core`/`russetd`/`russet-cli` should cross-compile / build on Windows for the
     thin GUI-side pieces; the heavy toolchain work happens in the container.
   - **START A NEW REPO for the Windows product.** The closed-source Windows app must **not** live in this GPL
     repo (dual-license hygiene: the GPL engine is consumed as a dependency/submodule, the Windows shell +
     installer + Docker glue are a separate proprietary repo). Create e.g. `evilcomputer12/russet-windows`
     (private). It depends on the `russet` engine (git submodule or a versioned crate), adds the Windows GUI
     shell, the `Dockerfile`/compose for the Linux toolchain container, the USB bridge, and the MSIX packaging.
     Keep the engine edits in *this* repo and consume them; never copy GPL engine source into the closed repo.

**C. Flutter stack (W17)** — Flutter build/run/**hot-reload** wired into the pipeline and the IDE. High value for
   Martin's work PC (cross-platform devs on Windows are the prime market).

**D. Packaging/polish** — Linux AppImage + Windows MSIX one-click installers; `strip` the release binary; ship the
   IDE + engine together so there is **never** a PATH/shell step.

## 7. Hard-won gotchas (will bite you otherwise)

- **Optional CI tools skip silently.** `cargo xtask ci` *skips* `reuse`, `gitleaks`, `zizmor` and falls back off
  `cargo-nextest` when they're missing — and still reports green. Two CIs went red this session from exactly this
  (a REUSE/SPDX break and the UDID secrets guard). **Install `reuse cargo-nextest gitleaks zizmor` locally, or
  trust only the GitHub Actions result.**
- **Generated files regenerate, never hand-merge**: `schemas/**`, `extension/src/generated/**`, `docs/reference/**`,
  `Cargo.lock`. Run `cargo xtask schema`. On a `Cargo.lock` conflict take `main`'s and run `cargo metadata`.
- **Seam files are lead-only** (listed in AGENTS.md §2). Work on a `ws/wNN-<slug>` branch; **only the lead merges to
  `main`**; never push unless asked.
- **Synthetic test identifiers**: new fixture UDIDs use chip-id `00000000` or an **all-zero ECID** so `guard
  secrets` admits them without an allowlist entry. Never commit a real UDID/ECID/team-id/Apple-ID.
- **Licensing** (superseded 2026-10-31: all of Russet is `AGPL-3.0-only`, see ARCHITECTURE §19): engine = `GPL-3.0-or-later`; templates + injected-into-user-app code = `0BSD`; permissive layer-F
  crates = `MIT`/`MIT OR Apache-2.0`. `deny.toml` allows only MIT/Apache-2.0/MPL-2.0/BSD/0BSD/GPL-3.0; **no
  LGPL/copyleft** in a shipped binary → **reims-vgpu (LGPL) must run out-of-process**. SPDX header on every file
  (REUSE 3.3); files that can't carry one go in `REUSE.toml`.
- **DCO → CLA before any external contribution.** `CONTRIBUTING.md` uses the DCO, under which contributors keep
  copyright — you cannot relicense their patches into the **closed Windows build**. Switch to a dual-licensing CLA
  (GitLab/Qt pattern) before accepting outside PRs. (Details in the private strategy note, §8.)
- **Build hygiene**: `CARGO_TARGET_DIR=$HOME/.cache/russet-dev/target-{a,b}`, `CARGO_BUILD_JOBS=6`,
  `CARGO_INCREMENTAL=0`. Disk is tight (single btrfs NVMe; keep >25 GiB; the slot-`b` cache is clearable). Rust
  **stable 1.99.0** (no second toolchain). Swift **6.4** at `/usr/lib/swift/usr/bin`; Darwin SDK at
  `~/.swiftpm/swift-sdks/darwin.artifactbundle` (read-only).
- **`russetd` is a library.** The daemon is embedded in `russet` (`russet daemon run|start|stop|status`), autostarts
  for CLI/IDE. The extension finds the binary via `russet.binDir` or PATH; it refuses the `live` backend unless the
  user owns the device (lane model).
- **Never commit Apple files** (users supply their own Xcode.xip) or secrets; tests use synthetic PKI.

## 8. Commercial / Windows track (PRIVATE — strip this section before the public G1 squash)

Full plan in `~/russet-strategy.md` on Martin's Linux box (not in the repo — not on the work PC). Essentials:
- **Open-core / dual-license.** Free GPL-3.0 Linux core builds the brand + engine; the **paid closed Windows Store
  MSIX** is the money: the bundled installer (Docker + toolchain + SDK wizard + USB bridge + auto-update) and the
  signing/account automation. Later: a cloud build/sign subscription for MRR.
- **Pricing anchor**: one-time $49–99 or $5–9/mo; the alternative is a ~$600 Mac mini + $99/yr. Target Windows
  cross-platform (Flutter/RN) indie devs.
- **Apple risk**: keep the Apple toolchain strictly **user-supplied**, ship **zero Apple IP** in the binary, use
  trademark-safe naming (never "Apple/iOS/Xcode" in the product name/branding). Get one IP-lawyer consult before
  charging money.
- **Do before monetizing**: DCO→CLA; `cargo deny check licenses` on the exact Windows feature set (zero
  third-party GPL/LGPL in the closed binary — isolate reims-vgpu out-of-process).

## 9. Environment facts (Martin's machines)

- **Linux dev box**: CachyOS, AMD RX 7600 XT (RADV). Rust 1.99, Swift 6.4, node 26. `russet` installed at
  `~/.local/bin` (now on PATH via `.bashrc`/`.profile`). IDE: VS Code 1.143 + VSCodium, extension installed.
- **Real device**: iPhone 12 (`iPhone13,2`), iOS 27.0.1 (24A446), USB, paired, `state: ready`.
- **Mac** (`ssh mac`, read-only, RE only): MacBook Air M1, macOS 27, Xcode 27 — used to RE Apple tooling; the end
  product must never require it. There's also a known-good **iOS 27 vphone VM on the Mac** used as the boot oracle.
- **Virtual iPhone (Linux)**: `~/russet-lab/thirdparty/vphone-linux/` (QEMU vresearch101 + reims-vgpu). Renders
  iOS 27. Separate project from `~/russet`.
- **Work PC (target for Codex)**: Windows + Visual Studio 2022 + **Intel iGPU**. This is where the Windows+Docker
  app and Flutter work happen. Note: whether the GPU-accelerated emulator runs on an Intel iGPU under Docker is
  **unproven** — treat it as an open question; a software/remote-render fallback may be needed there.

## 10. First concrete steps for Codex

1. Clone, read `AGENTS.md` + the PLAN cards for the device/debug/extension areas, build once
   (`cargo xtask ci --profile pr` with the optional tools installed).
2. **Task A**: design the `virtual` device backend + `virtual.*` protocol; implement start/resume/stop/status
   against `vphone-linux` (snapshot/resume first). Then the live-frame path (scope the reims-vgpu frame export) and
   input forwarding. Then the embedded webview and the destination-picker integration. Ship behind a flag, CI-green.
3. **Task B**: stand up the Docker image (Linux + Swift/Flutter toolchain + `russetd` over TCP), the Windows GUI
   connection, and the `usbipd-win`/usbmuxd device bridge; prototype build→install→debug on the iPhone from Windows
   through the container.
4. Keep everything **button-driven**; every new capability gets an IDE surface, not a CLI-only path.
5. **Stand up a GPU-accelerated Windows VM on this Linux box** (§11) to develop and test the Windows app here
   without the physical work PC, and **start the new `russet-windows` repo** (§6B) for the Windows product.

## 11. Set up a GPU-accelerated Windows VM on THIS Linux box (Codex: do this)

Martin wants a **Windows VM with GPU acceleration on this Linux box** so the Windows app (and the Docker build
backend, and eventually the emulator) can be developed and tested here, not only on the physical work PC.

**This box's actual state (probed 2026-10-30) — build on these facts, don't assume:**
- CPU **AMD Ryzen 5 5600X**, **AMD-V** virtualization on, `/dev/kvm` present → **KVM works**.
- GPU: **one AMD Radeon RX 7600 XT (Navi 33, PCI `1002:7480`), NO integrated GPU** (the 5600X has no iGPU).
- **IOMMU is NOT enabled** in the kernel cmdline (no `amd_iommu=on`); 23 IOMMU groups exist in hardware.
- `qemu-system-x86_64`, `virt-manager`, `virsh` installed; **`libvirtd` is inactive**; **`looking-glass-client`
  is missing**; **no Windows ISO and no `virtio-win` ISO** on disk.
- **Docker 29.9.2** present (for the container track).
- **Disk: only ~18 GiB free (97% full)** — a Windows VM needs 60 GiB+. **This is a blocker; free space first.**

**The GPU constraint and the three honest options (single AMD GPU, no iGPU):**
1. **virtio-gpu paravirtual 3D accel (venus/Vulkan or virgl/OpenGL)** — host keeps its display; no passthrough.
   Best for a *dev/test* Windows VM. Caveat: **Windows-guest virtio-gpu 3D drivers are experimental** (Linux
   guests are solid). Good enough to run/build/test the Windows GUI; may not fully accelerate the emulator.
2. **Single-GPU VFIO passthrough** — real GPU in the guest, but it **detaches the host desktop while the VM runs**
   (bind/unbind scripts). View the guest via **Looking Glass** in a host window. Most powerful, most disruptive.
3. **Add a cheap second GPU** → clean dedicated passthrough, host keeps its RX 7600 XT. The clean long-term answer
   if Martin wants both at once; needs hardware.

**Recommended path for Codex:** start with **option 1 (virtio-gpu venus)** for a usable dev VM that keeps the host
intact; only move to option 2 if the Windows side needs true GPU (e.g. to test the emulator's GPU path under
Windows — though that specific question really belongs on the Intel-iGPU work PC).

**Prerequisites to automate/do first (in order):**
1. **Free disk** to ≥ 80 GiB (clear `~/.cache/russet-dev/target-{a,b}`, old game downloads; keep >25 GiB reserve).
2. `sudo systemctl enable --now libvirtd` (+ add Martin to the `libvirt`/`kvm` groups).
3. For passthrough only: add `amd_iommu=on iommu=pt` to the bootloader kernel cmdline and **reboot**; set up
   `vfio-pci` for `1002:7480` (+ its HDMI-audio function in the same IOMMU group) and install `looking-glass`.
4. **ISOs are >500 MB → needs Martin's explicit OK before downloading** (AGENTS §6): Windows 11 ISO (~6 GB) and
   `virtio-win.iso` (~0.7 GB).

**Where to put it:** VM disk + ISOs under `~/vms/windows/` (or libvirt's `/var/lib/libvirt/images/`). Deliver a
libvirt domain XML + a start script, and later a one-click "Start Windows VM" surface. Virtio everywhere (disk,
net, gpu, input); SPICE or Looking Glass for display; share a folder (virtiofs) with the host for the repos.

**Note for Codex:** confirm the disk-free and ISO-download steps with Martin before running them (they cross the
AGENTS >500 MB / reboot thresholds). Everything else (libvirt domain, scripts, container, repo) you can build.

## 12. Where everything is (resource map on Martin's Linux box)

- **This handoff**: `~/russet/HANDOFF-CODEX.md` (also committed to the repo → in the clone).
- **Russet engine + IDE (GPL, open)**: `~/russet/` → repo `github.com/evilcomputer12/russet` (private).
  - CLI/daemon binary: `~/.local/bin/russet` (now on PATH); release build output
    `~/.cache/russet-dev/target-a/release/russet`.
  - IDE extension source: `~/russet/extension/`; built VSIX `~/russet/extension/russet.vsix`; installed in
    VS Code + VSCodium as `russet.russet`.
  - Templates: `~/russet/templates/swiftpm-app/` (0BSD).
- **New Windows product repo**: create `~/russet-windows/` → `github.com/evilcomputer12/russet-windows` (private,
  proprietary). Consumes the engine; holds the Windows shell, Docker glue, USB bridge, MSIX. (§6B)
- **Virtual iPhone emulator (separate project)**: `~/russet-lab/thirdparty/vphone-linux/`
  — `run-vphone.sh`, `snapshot-vphone.sh`, `resume-vphone.sh`, `build.sh`, `tools/{qmp.py,vphoned.sh,screen-changed.py}`,
  `reims-vgpu/` (LGPL, run out-of-process). VM bundles: `~/vphone-setup-work/devices/…` (Disk.img/nvram/ROMs/ecid —
  Mac-origin, **never redistribute**).
- **Private strategy** (not in any repo, Linux box only): `~/russet-strategy.md` (monetization/licensing).
- **Commercial memory/plan**: Claude memory `russet-wave2-merged.md` + `ios-dev-without-mac.md`.
- **Docker**: `/usr/bin/docker` (29.9.2) — for the Linux toolchain container.
- **Toolchains**: Rust stable 1.99 (`rustup`); Swift 6.4 `/usr/lib/swift/usr/bin`; Darwin SDK
  `~/.swiftpm/swift-sdks/darwin.artifactbundle` (read-only); node 26.
- **Virtualization**: `qemu-system-x86_64`, `virt-manager`, `virsh` installed; `/dev/kvm` present; `libvirtd`
  inactive (start it); Looking Glass + Windows/virtio ISOs not yet present.
- **Real device**: iPhone 12 on USB; `russet device list` shows it (`state: ready`).
- **Mac (RE only, `ssh mac`)**: Xcode 27 + the iOS 27 vphone boot oracle. The product must never require it.
