# Device plane: `russet-device` on `idevice`

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates), and §20.1 lists the superseded fields of this document. Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day) rewrote §6.3 (DDI) and closed OQ-3.

- **Status:** design, backed by a hands-on spike on 2026-10-11 against the test iPhone
  (iPhone 12 / iPhone13,2, iOS 27.0 24A437, Developer Mode on, DDI mounted) from CachyOS.
- **Scope:** everything Russet does with a physical iPhone except building and signing:
  discovery, pairing, Developer Mode, DDI, install, launch, debugserver, logs, screen
  (screenshots, live mirror, touch input), files, crash reports, profiling streams,
  symbols, Wi-Fi devices, and Windows.
- **Binding inputs:** [decisions](../decisions.md), [verified facts](../verified-facts.md),
  [landscape research](../research/landscape-2026-10.md) §2.8–2.11, §2.13, §5.
- **Spike artifacts (outside the repo):** `~/russet-lab/idevice` (shallow clone),
  `~/russet-lab/spike` (prototype crate `russet-device-spike`), `~/russet-lab/out` (logs),
  `~/russet-lab/bin` (built binaries). Build intermediates were deleted afterwards.

Labels used below:
- **VERIFIED**: we ran it on the device on 2026-10-11 (commands in Appendix A).
- **SOURCE**: read in code or docs; the file or URL is cited.
- **ASSUMPTION**: not checked yet. Each one has a task or open question that checks it.

Identifiers: `$UDID` is the test device, `$BUNDLE` is the xtool-installed bundle id
`XTL-<issuer8>.com.evilcomputer12.orchard.hello`. The device serial number, ECID and
name are deliberately left out of this document.

---

## 1. Summary

1. **idevice covers the whole device plane on iOS 27.0, from Linux, without root**
   (VERIFIED). One process opens one CoreDeviceProxy tunnel with idevice's in-process TCP
   stack (`jktcp`). Over that tunnel, plus plain usbmux/lockdown, we did all of these:
   - discovery and lockdown;
   - RSD service listing and DDI status (iOS 27 Cryptex1 DDI);
   - `.app` and `.ipa` install;
   - suspended launch with a pid, through CoreDevice and through DVT;
   - a debugserver forward that Swift LLDB attached through, stopping at the breakpoint;
   - per-pid `os_trace` logs and app stdout;
   - PNG screenshots;
   - **a live HEVC screen stream at 25–56 fps, and touch and button input, without
     WebDriverAgent**;
   - the DVT process list and sysmon;
   - AFC and app-container file access;
   - crash report retrieval.

   We never needed pymobiledevice3 for the M1 device steps.
2. **One blocker, now understood.** idevice sizes TCP segments from the tunnel MTU
   (16000), so it sends 15,940-byte segments. The USB CoreDeviceProxy path drops
   host-to-device IPv6 packets larger than about 8 KiB.
   - Effect: every bulk upload through the tunnel stalls. A 1 MiB AFC upload hit 5
     retransmits and timed out.
   - Fix: clamp the MSS to 8132, which gives **30 MiB/s up and 24–25 MiB/s down**.
   - pymobiledevice3 documents the same 8192-byte limit.
   - `russet-device` must build the tunnel itself with the clamp (§5.3), and we upstream
     the fix (§13).
3. **The live mirror and input come from Xcode's "Device Hub" CoreDevice services.**
   - `com.apple.coredevice.displayservice` sends RTP/HEVC.
   - `com.apple.coredevice.hid.*` takes the input; it is gated by an active display
     session.
   - Only one display session can exist per device. A second session kills the first
     one's stream. The daemon therefore owns a single display session per device, and the
     mirror and input share it.
4. **The daemon design:**
   - **One actor per device.** It owns lockdown access and one lazily created tunnel. The
     RSD service map is refreshed on every tunnel, because ports change.
   - **Leases.** Forwards, log streams, mirrors and debug sessions are leases, released
     on disconnect.
   - **Typed errors with remedies.** CoreDevice NSError chains are decoded.
   - **pymobiledevice3 as an optional subprocess fallback** for what idevice lacks: DSC
     symbols, the accessibility tree, mirror DDI download, streaming install and
     WebInspector.
5. **Windows.**
   - The default transport is the Apple Mobile Device Service usbmux at
     `127.0.0.1:27015`; idevice already defaults to it.
   - netmuxd (LGPL-2.1, subprocess) is optional, for Wi-Fi or for setups without Apple
     software.
   - The userspace tunnel needs no Admin rights and no wintun.

---

## 2. Spike: what we ran and what happened

### 2.1 Setup

| Item | Value |
|---|---|
| idevice | `github.com/jkcoxson/idevice` master @ `3854a5df` (2026-10-08). The `idevice` crate reports 0.1.68, and crates.io 0.1.68 was published 2026-09-15. We diffed the published crate against HEAD: only `usbmuxd/mod.rs` differs. In HEAD, `listen()` returns a `Send` stream, `Disconnected(u64)`, and a short-packet underflow fix |
| Build | `cargo build --release -p idevice-tools -j 10` with `CARGO_TARGET_DIR=~/russet-lab/target`, `CARGO_PROFILE_RELEASE_LTO=false` and `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`. It took **55 s**, the target directory was 474 MB, and `idevice-tools` came out at 13.7 MB. rustc 1.99.0 (`RUSTUP_TOOLCHAIN=stable`, because rustup has no default toolchain on this machine). aws-lc built with the system cmake |
| Rust deps of note | jktcp 0.1.7 (MIT), tokio 1.50, rustls 0.23, aws-lc-rs 1.18 / aws-lc-sys 0.45 |
| Host stack | usbmuxd 1.1.1 (Arch package 1.1.1-4.1), libimobiledevice 1.4.0, kernel 7.2.9-cachyos. `/var/run/usbmuxd` is mode 0666 and `/var/lib/lockdown/<UDID>.plist` is mode 0644, so no root was needed |
| Debugger | swift.org LLDB 21.0.0 (Swift 6.4), run with `LD_LIBRARY_PATH=$HOME/.local/opt/python-3.12/lib PYTHONHOME=$HOME/.local/opt/python-3.12` |
| Comparison | pymobiledevice3 11.26.0 |
| Prototype | `~/russet-lab/spike` (`russet-device-spike`, about 700 lines on idevice). Subcommands: `rsd`, `launch`, `forward`, `forward-unix`, `run-debug`, `gdb-probe`, `shots`, `tap`, `home`, `mirror-test`, `afc-bench`, `applogs`, `listen`, `procs` |

### 2.2 Results for the ten spike items

| # | Capability | How | Result (VERIFIED) | Time |
|---|---|---|---|---|
| 1 | usbmuxd discovery and pair record | `idevice_id`; `idevice-tools --udid $UDID ideviceinfo` (lockdown `StartSession` with the record from usbmuxd `ReadPairRecord`); spike `listen` | Device listed (USB, DeviceID 1). TLS session OK. `Listen` delivers `Attached` for already-present devices within 0.1 ms | 3 ms / 97 ms |
| 2 | Lockdown info | `ideviceinfo` (all domains, 1132 output lines); `lockdown --domain com.apple.security.mac.amfi get DeveloperModeStatus` → `true`; `amfi status` → `Enabled: true` | iPhone13,2, 27.0 (24A437), arm64e, Activated, `PasswordProtected=false`. `wireless_lockdown`: `EnableWifiDebugging=false`, `SupportsWifi=true` | ~0.1 s |
| 3 | CoreDeviceProxy, software tunnel, RSD | `idevice-tools remotexpc`; spike `rsd` (run 10+ times) | CDTunnel with client `fdXX:…::2`, server `…::1`, MTU 16000. RSD `MessagingProtocolVersion` 7, **85 services** (Appendix C). The ULA prefix, RSD port and every service port **change on each tunnel** | **100–112 ms** total: lockdown + CDTunnel handshake 82–94 ms, RSD handshake 16–21 ms |
| 4 | DDI status and mount | `mounter list`; `cryptexd list`; `cryptexd install-ddi ~/.local/share/pymobiledevice3/Xcode_iOS_DDI_Cryptex` without `--force` | Mounted: `DiskImageType=Personalized`, `MountPath=/System/Developer`, backing image `/private/var/db/com.apple.security.cryptexd/codex.system/cryptex/com.apple.MobileAsset.DDI/gdmg`, CoreDevice 642.4, `ProductBuildVersion 27A5228h`. cryptexd lists `com.apple.MobileAsset.DDI 27.1.5228.8`. idevice parsed the Cryptex1 payloads (Image.dmg 15.9 MB, trust cache, cryptex_info, root_hash; nonce domain 4) and answered "already installed". **`mounter lookup` returns `NotFound` on iOS 27**, so do not use it for status. We did not remount or unmount | 0.11–0.14 s |
| 5 | Install | Signed in place with `xtool dev build --sign`, reusing the stored certificate. Then `ideviceinstaller install Hello.app` and `… install Hello.ipa`; the IPA was made with Python `zipfile` | Both OK. The AFC → `PublicStaging` upload plus `installation_proxy` path works. installd empties `PublicStaging` afterwards | 1.4 s (`.app`) / 1.2 s (`.ipa`), 145 KB app |
| 6 | Launch suspended, get pid | Spike `launch $BUNDLE --suspended` (CoreDevice `appservice` plus `openstdiosocket`); `--dvt` (DVT `processcontrol`) | A pid both ways; the process shows in CoreDevice `listprocesses`. A suspended app resumes with SIGCONT through CoreDevice `sendsignaltoprocess` | CoreDevice 83–148 ms warm, **3.78 s** on the first launch after install; DVT 136 ms |
| 7 | debug_proxy forward and Swift LLDB | Spike `forward 41250`, `run-debug $BUNDLE 41251` (one tunnel: forwarder plus CoreDevice suspended launch), `forward-unix $XDG_RUNTIME_DIR/x.sock`, with the `demo/m1-baseline.sh` LLDB sequence | Stopped at `ContentView.swift:6` in `closure #1 in ContentView.body.getter`; app frames symbolized; `process kill` → exit 9. Works over TCP **and over `unix-connect://`** (0600 socket). A launch in one process plus a debug forward in another (separate tunnels) also worked | Device connect 1.1–2.3 ms; 4 gdb packets in 22–27 ms; LLDB script about 8 s including its 2 s sleep |
| 8 | syslog and os_trace | `syslog_relay` 6 s; `os_trace_relay` 6 s; spike `applogs` (pid filter) | 1102 syslog lines. 4290 structured `os_trace` entries carrying pid, level, subsystem/category, image UUID and offset, and mach time. **The device-side pid filter returned 1553 entries for the app and 0 from other processes in 4 s** | Streaming |
| 9 | Screenshot and higher-FPS stream | Spike `shots 10`; `idevice-tools screencapture`; spike `mirror-test 10` | DVT screenshot: **7.5 fps**. CoreDevice `screencaptureservice` PNG: 6.2 fps. Both 1170×2532 PNG. iOS 27 rejects `requestedFormat: "jpeg"`. **displayservice: HEVC Main, 1184×2576, 25–56 frames/s, about 2.4 Mbit/s during UI motion, stream up in 166–172 ms**, with HID in the same session. iOS shows a screen-sharing pill in the status bar while it streams | See text |
| 10 | DVT sysmon and processes, AFC and house_arrest, crash reports | `sysmontap 500`; `device_info processes`; `afc list /`; `afc --container $BUNDLE list/upload/download/remove`; `crash_logs flush/list/pull`; `app_service signal <pid> 6` | sysmon: 74 process and 37 system attributes, a sample every 500 ms. DVT process list: 587 processes with an `isApplication` flag. A 1 MiB container round trip was byte-identical. A SIGABRT sent by us produced `Hello-<ts>.ips` (bug_type 309, `slice_uuid`, `EXC_CRASH SIGABRT`, `byProc dtappserviced`; system frames symbolized on the device) | 0.06–0.17 s for crash operations; about 1.2 s per house_arrest command (VendContainer latency) |

Additional checks:

| Check | Result (VERIFIED) |
|---|---|
| HID surfaces (`hid list`) | touchscreen 257, keyboard 512, mainScreenButtons 1026, avpCustom 1280, touchscreenGesture 1281 |
| Buttons and drags | `hid button home` (indigo) works; the device wakes and shows the home screen. `hid drag` (universal HID) scrolls the home pages |
| Tap | The stock `hid tap` (50 ms contact, then stopping the display session at once) did nothing, twice. The spike tap launched the app on the first try. It sends a contact, 3 position samples over 120 ms, a release, and keeps the session for 600 ms afterwards |
| Two display sessions | A second session (start then stop, as the HID tool does) stopped RTP on the first session for good: the capture saw "no RTP for 5s" |
| Concurrent tunnels | Two idevice tunnels in two processes worked at once. An idevice tunnel stayed healthy before and after a concurrent pymobiledevice3 `--userspace` tunnel (`core-device list-processes`, 592 processes) |
| Bulk transfer over the tunnel (`afc-bench`) | AFC over usbmux: 31.9 MiB/s up, 36.7 MiB/s down. AFC over the tunnel (`afc.shim.remote`): **stalls** with the default MSS 15940, and also with MSS 9000–15000. It works at MSS ≤ 8133: 30.0–30.2 MiB/s up, 23.7–25.2 MiB/s down for 32 MiB at MSS 8132. The device's SYN-ACK advertises MSS 15940 and wscale 6 (pcap parsed) |
| LLDB expressions (input for the debug design) | With `target.memory-module-load-level complete`, `expr -l objc -- (int)printf(...)` **ran inside the app**, and its output came back through the CoreDevice stdio socket. The attach then took about 46 s, against about 8 s with `minimal`. `expr -l swift` fails with "Error while searching for SDK: HostInfoError … missing required module 'SwiftShims'" |
| Device locked | CoreDevice launch failed with `com.apple.dt.CoreDeviceError` 10002 → `FBSOpenApplicationServiceErrorDomain` 1 `RequestDenied` → `FBSOpenApplicationErrorDomain` 7 **"Locked"**. A HID Home press woke the passcode-less test phone |
| Lock state query | `pymobiledevice3 developer core-device get-lockstate` → `CoreDevice.ActionError` 2: "Action 'com.apple.coredevice.feature.getlockstate' is not implemented", although `deviceinfo` advertises the feature |
| App exit | After SIGKILL, the CoreDevice stdio stream closed within about 130 ms. jktcp reports a "channel closed" error rather than a clean EOF |
| Accessibility tree (pymobiledevice3) | `developer accessibility list-items` returned the on-screen elements ("Globe, Image", "Hello, world!") in 2.6 s without WDA |
| DSC symbols (pymobiledevice3) | `developer fetch-symbols list --userspace`: 82 files (main cache, about 70 subcaches, `.symbols`, DriverKit, `/usr/lib/dyld`). **Not downloaded**, because of disk space |
| Remote pairing info | `remotepairing_lockdown info`: `allowsPinlessPairing=true`, `allowsPairSetup=true`, `allowsIncomingTunnelConnections=false`, `ChromeIdentifier=com.apple.dt.devicekit.chrome.phone4`, `MarketingNameString="iPhone 12"` |
| Wi-Fi | Not exercised. `EnableWifiDebugging=false`, and `avahi-browse` found no `_apple-mobdev2._tcp` or `_remotepairing._tcp` on this LAN (10.83.119.0/24, possibly client-isolated). We did not change the setting on the phone |

### 2.3 Findings that drive the design

- **F1 MSS black hole (blocker).** The cause is the code path below. Fix: build the
  adapter ourselves with `set_mss(min(mtu-60, 8132))`. Long term, upstream the clamp, or
  use PLPMTUD the way pymobiledevice3 does.
  - `CoreDeviceProxy::create_software_tunnel` calls `adapter.set_mss(mtu - 60)` with MTU
    16000 (`idevice/src/services/core_device_proxy.rs:91`, `tunnel.rs:16`).
  - jktcp parses the peer's window scale but not its MSS (`jktcp-0.1.7/src/adapter.rs:717`).
  - The device drops host-to-device packets over about 8192 bytes.
  - pymobiledevice3 records the same limit: *"USB CoreDeviceProxy drops host->device IPv6
    packets over 8192 bytes total but passes everything below"*
    (`pymobiledevice3/remote/userspace_tunnel.py`, 11.26.0, comment on `BASE_MSS_SEED`).
- **F2 Service ports are per tunnel.** RSD ports differed on every tunnel (for example
  debugproxy at 50366, then 51282, then 51460). Never cache ports across tunnels.
- **F3 Tunnels per host coexist on USB with iOS 27.0.** idevice with idevice, and idevice
  with pymobiledevice3, both worked. So the pymobiledevice3 subprocess fallback can run
  next to the daemon's tunnel. The research note "from 26.2 the device keeps one RSD peer
  per host" [dv-34] did not show up over USB. Keep the coexistence check in the HIL suite.
- **F4 The display session is exclusive per device and gates HID.** The daemon must
  multiplex the mirror and input over one session (§6.8).
- **F5 Tap timing.** Hold for at least 100 ms with position samples, and keep the session
  open after the release. The upstream tool's `tap` is not reliable on iOS 27.0.
- **F6 DDI on iOS 27.** The DDI is a cryptex: `cryptexd` lists
  `com.apple.MobileAsset.DDI`, and `mobile_image_mounter CopyDevices` shows it mounted.
  `LookupImage` returns NotFound. Status = CopyDevices plus the cryptexd list.
- **F7 Bundle ids from xtool.** Bundle ids are `XTL-…`-prefixed (already in
  verified-facts). Russet's own signing must keep exact bundle ids (signing design, not
  this document).
- **F8 Debug forward over a Unix socket works with LLDB** (`unix-connect://`). That is
  safer than TCP on multi-user Linux machines (§14).
- **F9 iOS 27 quirks.**
  - `screencaptureservice` rejects `"jpeg"`.
  - `getlockstate` is not implemented.
  - Being locked is only visible through errors.
- **F10 The first launch after an install is slow** (3.8 s; warm launches about 0.1 s).
  The UI should show "launching…" rather than time out early. The launch timeout is 30 s.
- **F11 Symbol loading dominates attach time** when LLDB reads libraries from memory
  (46 s with full symbol tables). That is why the DSC symbol cache is in scope (§6.12).
- **F12 idevice's directory upload silently skips symlinks**
  (`utils/installation/helpers.rs:174-199`: it checks `is_dir`/`is_file` on non-followed
  metadata). Russet always installs from an IPA that it writes itself, which preserves
  symlinks (§6.4).

---

## 3. Requirements this plane serves

| Product feature (decisions.md) | Device capability | Section |
|---|---|---|
| Device list | Discovery, pairing state, readiness (Developer Mode, DDI, lock), USB or Wi-Fi | 6.1–6.3 |
| Run | Install, launch, stdio console, terminate, exit detection | 6.4, 6.5 |
| Debug | Launch suspended, debugserver forward, LLDB attach inputs | 6.6 |
| Hot reload (Flutter) | Forwarding a device TCP port (VM service), app stdout (VM service URL) | 6.5, 6.6 |
| Live logs | `os_trace` per pid, syslog, stdout | 6.7 |
| Live screen mirror | displayservice HEVC stream to the IDE; screenshot fallback | 6.8 |
| Agent "tap the screen" | HID tap, swipe, buttons, text; screenshots; accessibility tree (fallback) | 6.8, 8 |
| Profiling graphs | DVT sysmontap, energy, network, graphics (fps) | 6.11 |
| Crash reports | Flush, list and pull `.ips`; parse; app-frame symbolication hook | 6.10 |
| Files | AFC media, app container (house_arrest), crash directory | 6.9 |
| Test (later) | XCTest runner launch (idevice `xctest`) | 6.13 |
| Headless operation | All of the above through the CLI and JSON-RPC, without the IDE | 7 |

Non-goals here: building, signing, provisioning (signing and accounts designs), LLDB
internals and the JIT helper (debug design), and Instruments `.trace` compatibility.

---

## 4. Architecture

```text
 Russet IDE (Code-OSS + extension) ─┐
 russet CLI ────────────────────────┼── JSON-RPC (transport defined by the daemon design)
 MCP clients (agents) ── russetd MCP┘
                                     │
                         russetd (tokio)  ── owns ──>  DeviceManager  (crate russet-device)
                                                        │
               ┌────────────────────────────────────────┼─────────────────────────────┐
               │ MuxClient                              │ DeviceActor (one per UDID)   │
               │  usbmuxd (Linux /var/run/usbmuxd)      │  ├ lockdown via usbmux       │
               │  AMDS (Windows 127.0.0.1:27015)        │  ├ Tunnel (CoreDeviceProxy   │
               │  netmuxd (optional, both OSes)         │  │   + jktcp, MSS<=8132)     │
               │  Listen -> attach/detach events        │  ├ RSD service map (per tun) │
               └────────────────────────────────────────┤  ├ Leases: forwards, logs,  │
                                                        │  │   mirror, debug sessions  │
               Fallback: pymobiledevice3 subprocess     │  └ DisplaySession (exclusive)│
               (optional, version-checked)              └─────────────────────────────┘
```

Principles:
1. **idevice is the only in-process device stack.**
   - Pin it exactly: crates.io `=0.1.68` plus the reviewed `[patch.crates-io]` of ARCHITECTURE
     §16.7 (the usbmuxd fixes of `3854a5df` and the one-line cryptexd persistence fix, §6.3)
     until a release carries them (ARCHITECTURE [D 22]).
   - Wrap everything in our own types so idevice's API churn ("breaking changes at each
     point release until 0.2.0", idevice README) stays inside `russet-device`.
2. **One tunnel per device per daemon.** Every RSD client is multiplexed over it. The CLI
   without a daemon opens its own short-lived tunnel; F3 shows that is safe.
3. **Lockdown-only services go over usbmux**, not the tunnel, when that works the same way:
   - install, AFC, house_arrest, crash reports, `os_trace`, syslog.

   This is faster (F1), and it survives tunnel resets. RSD shims
   (`*.shim.remote`) are the fallback for Wi-Fi tunnels.
4. **No root and no Admin**, with userspace TCP only. A kernel TUN backend is a later,
   optional feature (§17 R-D2).
5. **No Apple files are shipped.** DDIs are Apple-origin copies the user imports (their own
   `Xcode.xip`, or a copy of their Mac's DDI folder), or come from an explicit mirror opt-in
   (§6.3, ARCHITECTURE [D 62]). Device frame art is ours, not Apple's.

---

## 5. Crate layout and public API

### 5.1 Crates

| Crate | Licence | Purpose |
|---|---|---|
| `russet-device` | GPL-3.0-or-later | Library: the device model, manager and actors, all capabilities, the error model, the pymobiledevice3 fallback |
| `russet-device-proto` | GPL-3.0-or-later (could be MIT) | Serde types shared by the JSON-RPC and MCP surfaces (§7, §8); no idevice dependency, so the IDE can use the generated TypeScript |
| `russet-device-hil` | GPL-3.0-or-later | Hardware-in-the-loop tests (§15), gated by `RUSSET_HIL_UDID` |
| (upstream) idevice / jktcp patches | MIT | Upstream PRs, kept free of GPL code (§13) |

`russet-device/Cargo.toml` (sketch):

```toml
[dependencies]
idevice = { git = "https://github.com/jkcoxson/idevice", rev = "3854a5df", default-features = false, features = [
  "ring", "usbmuxd", "tcp", "pair", "mdns", "heartbeat",
  "core_device_proxy", "tunnel_tcp_stack", "rsd", "xpc", "remote_pairing",
  "core_device", "display_stream", "debug_proxy",
  "dvt", "sysmontap", "device_info", "energy_monitor", "network_monitor", "graphics",
  "condition_inducer", "location_simulation",
  "afc", "house_arrest", "installation_proxy", "crashreportcopymobile", "syslog_relay",
  "mobile_image_mounter", "cryptexd", "tss", "amfi", "misagent", "springboardservices",
  "notification_proxy", "diagnostics_relay", "xctest",
] }
tokio = { version = "1", features = ["full"] }
plist = "1"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tracing = "0.1"
zip = { version = "2", default-features = false, features = ["deflate"] }  # IPA writer (symlink entries)
uuid = "1"
```

- **Pins are superseded** by ARCHITECTURE §16.7 and [D 22]: crates.io `idevice =0.1.68` with the reviewed
  `[patch.crates-io]` (upstream 3854a5df plus one Russet commit: the cryptexd persistence fix and a 16 MiB cap
  on declared usbmuxd packet sizes, U10), `zip` 9.0.1 inside `russet-ipa`. The feature list above stays.
- `os_trace_relay` is gated by the `syslog_relay` feature (SOURCE:
  `idevice/src/services/mod.rs:42-43`).
- The crypto backend is `ring`, not the default `aws-lc`.
  - aws-lc-sys needs CMake, and NASM or prebuilt objects on Windows (ASSUMPTION, to
    confirm in DEV-01 CI); ring builds with only a C compiler.
  - Licences: ring 0.17.14 is "Apache-2.0 AND ISC"; aws-lc-sys 0.45.0 has no OpenSSL
    licence term. Both are GPL-3.0-compatible.

### 5.2 Module map

```text
russet_device::
  model        DeviceId, DeviceSummary, DeviceInfo, Transport, Readiness, AppInfo, ProcessInfo,
               LaunchRequest, LogEntry, CrashReportMeta, ScreenGeometry, ...
  error        DeviceError, ErrorKind, Remedy, nserror::NsErrorChain (decoder)
  mux          MuxEndpoint, MuxFlavor (Usbmuxd | Amds{store_app} | Netmuxd), MuxClient, watch()
  pairing      PairRecordStore, pair_lockdown(), RpPairingStore, rp_pair_over_usb()
  manager      DeviceManager, DeviceHandle (cloneable command sender), DeviceEvent
  actor        DeviceActor (private), LeaseTable, TunnelState
  tunnel       Tunnel, TunnelConfig, open_usb_tunnel(), open_wifi_tunnel()
  ddi          DdiStatus, DdiSource, ensure_ddi()
  apps         IpaWriter, install(), uninstall(), list()
  process      launch(), signal(), list(), StdioStream, ExitWatcher
  debug        DebugForward, ForwardTarget, debug_start(), gdb_probe()
  logs         LogStream, LogFilter, LogSource
  screen       screenshot(), DisplaySession, MirrorHandle, InputEvent, HidInjector
  files        FileScope, FileClient
  crash        CrashStore, IpsReport (parser)
  perf         SysmonStream, SysmonOptions, EnergyStream
  symbols      SymbolCache, fetch_dsc()
  extras       location, pasteboard, orientation, appearance (cheap CoreDevice wrappers)
  fallback     Pmd3 (subprocess runner), Pmd3Capability
```

### 5.3 Core types and signatures

> **Superseded on the wire (ARCHITECTURE §9.5, §20.1):** `DeviceState` stays a Rust enum inside
> this crate, but the wire shape is the flat `DeviceSummary.state` (kebab-case) plus `readiness`,
> and `DdiSource` is ARCHITECTURE §5.3's `{Imported, Dir, Pmd3Cache, Pmd3AutoMount}`. The protocol
> module converts; never serialize these types directly.

```rust
// ---- identity & model -------------------------------------------------------------
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);                       // UDID

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Transport { Usb, Wifi { addr: std::net::IpAddr } }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub id: DeviceId,
    pub name: String,                   // lockdown DeviceName
    pub product_type: String,           // "iPhone13,2"
    pub marketing_name: Option<String>, // "iPhone 12" (remotepairing info / lookup table)
    pub os_version: String,             // "27.0"
    pub build: String,                  // "24A437"
    pub transport: Transport,
    pub state: DeviceState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DeviceState {
    Attached,                 // seen by mux, nothing checked yet
    NeedsTrust,               // no valid pair record / InvalidHostID
    TrustPending,             // PairingDialogResponsePending
    Ready(Readiness),
    Unavailable { error: DeviceErrorInfo },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Readiness {
    pub paired: bool,
    pub developer_mode: Tri,            // Yes | No | Unknown (pre-iOS16 = Yes)
    pub ddi: DdiStatus,
    pub locked: Tri,                    // Unknown unless an operation reported "Locked"
    pub tunnel: TunnelStatus,           // Down | Up { since, generation } | Failed { error }
}

// ---- manager ------------------------------------------------------------------------
pub struct DeviceConfig {
    pub mux: Option<MuxEndpoint>,       // None = platform default (§10)
    pub tunnel: TunnelConfig,
    pub pmd3: Option<std::path::PathBuf>, // explicit pymobiledevice3 path; None = PATH lookup
    pub state_dir: std::path::PathBuf,  // pair records (Wi-Fi), caches
    pub wifi: WifiConfig,               // discovery on/off (default off until DEV-21)
}

pub struct DeviceManager { /* registry: DashMap<DeviceId, DeviceHandle>, events tx, ... */ }
impl DeviceManager {
    pub async fn start(cfg: DeviceConfig) -> Result<std::sync::Arc<Self>, DeviceError>;
    pub fn devices(&self) -> Vec<DeviceSummary>;
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<DeviceEvent>;
    pub fn get(&self, id: &DeviceId) -> Result<DeviceHandle, DeviceError>;
    pub async fn shutdown(&self);
}

#[derive(Clone, Debug, Serialize)]
pub enum DeviceEvent {
    Added(DeviceSummary),
    Changed(DeviceSummary),
    Removed(DeviceId),
    LeaseEnded { device: DeviceId, lease: LeaseId, reason: EndReason },
}

/// Cloneable handle; every call is a message to the device's actor.
#[derive(Clone)]
pub struct DeviceHandle { /* tx: mpsc::Sender<Cmd>, id: DeviceId */ }

impl DeviceHandle {
    pub fn id(&self) -> &DeviceId;
    pub async fn info(&self) -> Result<DeviceInfo, DeviceError>;          // lockdown values + RSD properties
    pub async fn readiness(&self, refresh: bool) -> Result<Readiness, DeviceError>;
    pub async fn pair(&self, opts: PairOptions) -> Result<PairOutcome, DeviceError>;
    pub async fn developer_mode(&self, step: DevModeStep) -> Result<Tri, DeviceError>;
    pub async fn ddi_status(&self) -> Result<DdiStatus, DeviceError>;
    pub async fn ensure_ddi(&self, src: DdiSource) -> Result<DdiStatus, DeviceError>;

    pub async fn install(&self, pkg: &std::path::Path, opts: InstallOptions,
                         progress: ProgressSink) -> Result<InstalledApp, DeviceError>;
    pub async fn uninstall(&self, bundle_id: &str) -> Result<(), DeviceError>;
    pub async fn apps(&self, filter: AppFilter) -> Result<Vec<AppInfo>, DeviceError>;

    pub async fn launch(&self, req: LaunchRequest) -> Result<LaunchedProcess, DeviceError>;
    pub async fn signal(&self, pid: u32, sig: i32) -> Result<(), DeviceError>;
    pub async fn processes(&self) -> Result<Vec<ProcessInfo>, DeviceError>;

    pub async fn forward(&self, target: ForwardTarget, bind: BindSpec) -> Result<ForwardHandle, DeviceError>;
    pub async fn debug_start(&self, req: DebugRequest) -> Result<DebugSession, DeviceError>;

    pub async fn logs(&self, filter: LogFilter) -> Result<LogStream, DeviceError>;
    pub async fn screenshot(&self) -> Result<Screenshot, DeviceError>;   // PNG
    pub async fn mirror(&self, opts: MirrorOptions) -> Result<MirrorHandle, DeviceError>;
    pub async fn input(&self, ev: InputEvent) -> Result<(), DeviceError>;
    pub async fn ui_elements(&self) -> Result<Vec<UiElement>, DeviceError>; // pmd3 fallback (6.8.4)

    pub async fn files(&self, scope: FileScope) -> Result<FileClient, DeviceError>;
    pub async fn crash_list(&self, q: CrashQuery) -> Result<Vec<CrashReportMeta>, DeviceError>;
    pub async fn crash_pull(&self, name: &str, dest: &std::path::Path) -> Result<IpsReport, DeviceError>;

    pub async fn sysmon(&self, opts: SysmonOptions) -> Result<SysmonStream, DeviceError>;
    pub async fn fetch_symbols(&self, cache: &SymbolCache, progress: ProgressSink)
        -> Result<SymbolSet, DeviceError>;
}

// ---- launch / debug ------------------------------------------------------------------
pub struct LaunchRequest {
    pub bundle_id: String,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub suspended: bool,
    pub kill_existing: bool,               // default true
    pub console: bool,                     // attach CoreDevice stdio (openstdiosocket)
    pub via: LaunchVia,                    // Auto | CoreDevice | Dvt
}
pub struct LaunchedProcess {
    pub pid: u32,
    pub executable: String,                // device path, from CoreDevice executableURL
    pub stdio: Option<StdioStream>,        // AsyncRead + AsyncWrite; ends when the process dies
    pub exit: ExitWatcher,                 // resolves with ExitInfo{ signal|code|unknown, at }
}

pub enum ForwardTarget {
    RsdService(String),                    // e.g. "com.apple.internal.dt.remote.debugproxy"
    TunnelPort(u16),                       // any TCP port on the device's tunnel address
    UsbmuxPort(u16),                       // classic usbmux Connect (iproxy-style), no tunnel
}
pub enum BindSpec {
    Tcp(std::net::SocketAddr),             // must be loopback unless allow_remote
    #[cfg(unix)] Unix(std::path::PathBuf), // created 0600 in $XDG_RUNTIME_DIR/russet/
}
pub struct ForwardHandle { pub id: LeaseId, pub local: LocalEndpoint }   // drop = close

pub struct DebugRequest {
    pub launch: LaunchRequest,             // suspended forced true
    pub bind: BindSpec,                    // default: Unix socket on Linux, 127.0.0.1:0 on Windows
    pub service: Option<String>,           // default internal.dt.remote.debugproxy
}
pub struct DebugSession {
    pub id: LeaseId,
    pub pid: u32,
    pub remote_executable: String,
    pub gdb_remote_url: String,            // "unix-connect:///run/user/1000/russet/dbg-<id>.sock"
                                           // or "connect://127.0.0.1:41250"
    pub platform: PlatformInfo,            // os "ios", version, build, arch "arm64e", product_type
    pub stdio: Option<StdioStream>,
    pub exit: ExitWatcher,
}
```

The `Tunnel` (crate-private) is built like this. It mirrors the spike's
`open_session` (`~/russet-lab/spike/src/main.rs`):

```rust
pub struct TunnelConfig {
    pub mss_cap: usize,            // 8132 (USB CoreDeviceProxy). See F1.
    pub connect_timeout: Duration, // 8 s (jktcp default for SYN)
    pub rsd_timeout: Duration,     // 5 s
    pub idle_close: Duration,      // 300 s without leases
    pub pcap: Option<PathBuf>,     // debug: RUSSET_TUNNEL_PCAP
}

async fn open_usb_tunnel(p: &UsbmuxdProvider, cfg: &TunnelConfig) -> Result<Tunnel, DeviceError> {
    let proxy = CoreDeviceProxy::connect(p).await?;              // lockdown StartService
    let info = proxy.tunnel_info().clone();                      // addrs, mtu, rsd port
    let stream = proxy.into_tunnel().into_inner();               // raw CDTunnel stream
    let mut ad = Adapter::new(Box::new(stream), info.client_address.parse()?,
                              info.server_address.parse()?);
    ad.set_mss((info.mtu as usize).saturating_sub(60).min(cfg.mss_cap));
    let mut handle = ad.to_async_handle();
    let rsd = timeout(cfg.rsd_timeout,
                      RsdHandshake::new(handle.connect(info.server_rsd_port).await?)).await??;
    Ok(Tunnel { handle, rsd, info, generation: next_gen(), opened: Instant::now() })
}
```

---

## 6. Capability designs

### 6.1 Discovery and identity

- **Mux endpoint.** The default comes from `UsbmuxdAddr::from_env_var()`:
  - Linux: `/var/run/usbmuxd`.
  - Windows: `127.0.0.1:27015` (SOURCE: `idevice/src/usbmuxd/mod.rs:83-150`).

  `USBMUXD_SOCKET_ADDRESS` overrides it, and so does `russet config device.mux`.
- **Watch.**
  - Keep one `Listen` connection per daemon. `Attached` spawns a `DeviceActor`, and
    `Detached` (keyed by mux device id) ends all of its leases with `Disconnected`.
  - The listen stream is not `Send` in 0.1.68. Either pin HEAD, or run `listen()` on a
    dedicated `current_thread` runtime and forward events over a channel.
  - Reconnect the Listen socket with backoff (0.25, 1, 3, 10 s) if the mux daemon
    restarts.
- **Identity, read lazily** (ARCHITECTURE §5.3, [D 63]). On attach the actor records only the mux
  record; the first client request (`device.list`, `device.info`, a `device.watch` subscriber or any
  device operation) reads, through lockdown without a session:
  - `ProductType`, `ProductVersion`, `BuildVersion`, `DeviceName`, `DeviceClass`,
    `HardwareModel`, `CPUArchitecture`.

  With a session, it also reads Developer Mode (amfi domain) and
  `wireless_lockdown.EnableWifiDebugging`.

  Marketing names come from:
  - the remotepairing info when it is cheap (`MarketingNameString`), otherwise
  - a small built-in table keyed by `ProductType`. This table is ours: it maps public
    names to identifiers and contains no Apple files.
- **Code-signing monitor** (`DeviceSummary.codeSigningMonitor`, ARCHITECTURE §9.5). It comes from a
  ChipID × iOS-major table that a small script generates from public BuildManifests, storing only
  derived booleans and digests, never Apple files: on iOS 26, TXM iff ChipID ≥ 0x8110; on iOS 27,
  TXM on every supported chip except 0x8027 (the A12Z iPad Pro). The test iPhone 12 (0x8101) is TXM
  on iOS 27.0 (verified from the 24A437 firmware, 2026-10-11). The probe app can confirm it through
  the `security.codesigning.monitor` sysctl (0 none, 1 PPL, 2 TXM), if a sandboxed app may read it.
  `doctor` also reports Developer Mode before JIT runs: the iOS 27 TXM adds a "platform code strict"
  path that can turn Developer Mode off (trigger unknown).
- **Same device on USB and Wi-Fi.** One `DeviceId` holds two transports, and USB is
  preferred. The IDE shows a badge.
- **Device frame (IDE).** `ChromeIdentifier` (for example
  `com.apple.dt.devicekit.chrome.phone4`), the screen size from the first screenshot, and
  `MainDisplayRotation` drive the IDE's own frame drawing.

### 6.2 Pairing, trust and Developer Mode

- **Pair records.**
  - Read: usbmux `ReadPairRecord` first.
  - Then on Windows, `%ALLUSERSPROFILE%\Apple\Lockdown\<UDID>.plist`. pymobiledevice3
    uses the same path: `osu/win_util.py` `pair_record_path` (SOURCE).
  - Then Russet's store `<state_dir>/pairing/<UDID>.plist`, used for Wi-Fi and for
    netmuxd without persistence.
- **Pairing flow** (`device.pair`), using idevice `LockdownClient::pair(host_id, buid, name)`:
  1. Generate the host id (UUID v4, upper case).
  2. Get the BUID with usbmux `ReadBUID`.
  3. Call `pair()`.
  4. On `PairingDialogResponsePending`, emit `TrustPending`, poll every 1 s for 120 s,
     and return to the user "Unlock the iPhone and tap Trust".
  5. On `UserDeniedPairing`, return `PairingDenied`.
  6. On `PasswordProtected`, return `Locked` ("Unlock the iPhone first").
  7. On success: validate it with `start_session`, then `SavePairRecord` through usbmux,
     so usbmuxd and AMDS own it.

  This is the same sequence as `idevice/tools/src/pair.rs`.
- **iOS 26 first-pair timing on Linux.** usbmuxd 1.1.1 can drop the device while the
  Trust prompt is up (usbmuxd #281, [dv-40]). `doctor` checks for it and suggests
  `--no-preflight`. We did not reproduce it, because the phone was already paired.
- **Developer Mode** (`amfi`):
  - `status` → `Tri`;
  - `show` reveals the toggle (works without a passcode);
  - `enable` works only when no passcode is set; otherwise it returns `NeedsUserAction`,
    with steps;
  - `accept` handles the post-reboot prompt.

  None of these ran in the spike, because the mode was already on.

### 6.3 DDI

> **Updated 2026-10-11 (review rounds 1 and 2):** sources, keying and the Wave-1 path follow ARCHITECTURE
> [D 62], §5.3 `ddi`, §14.3 `[device.ddi]` and §16.3. OQ-3 is closed (§18). Round 2: `dir` is the DDI root (a copy of
> `iOS_DDI` holds `Restore/` and `version.plist`; Russet loads `<dir>/Restore`) [D 82], and a mounted DDI's source comes
> from Russet's own mount record, `state/devices/<udid>/ddi.json` [D 89].

- **Status (VERIFIED approach).**
  - `ImageMounter::copy_devices()`: an entry with `MountPath=/System/Developer` and
    `IsMounted=true`; we record `PersonalizedImageVersion` and the `ProjectMetadata`
    CoreDevice version.
  - For iOS 27 and later, also `CryptexdClient::copy_installed()` →
    `com.apple.MobileAsset.DDI <version>`.
  - **Do not use `LookupImage`** (F6).
  - A mounted DDI reports the source of Russet's last mount record for that device
    (`state/devices/<udid>/ddi.json`: source, path, ddiBuild, cryptex version, image SHA-384, mountedAt) while
    the mounted cryptex version and product build still match it; otherwise `unknown`. Matching versions alone
    cannot tell the Wave-1 directory copy from an import of the same Xcode, which are the same bytes.

```rust
pub struct DdiStatus {
    pub mounted: bool,
    pub kind: DdiKind,                    // Legacy | PersonalizedDmg | Cryptex1
    pub version: Option<String>,          // cryptex "27.1.266.1" / CoreDevice "642.16"
    pub product_build: Option<String>,    // "27A266a"
    pub source: DdiOrigin,                // imported | dir | pmd3-cache | pmd3-auto-mount | unknown
}
pub enum DdiSource {                      // ARCHITECTURE §5.3 is binding
    Imported { ddi_build: Option<String> }, // data/ddi/<ddiBuild>/iOS/Restore; newest first if None
    Dir(PathBuf),                         // a DDI root (Restore/ + version.plist), e.g. a copy of a Mac's iOS_DDI;
                                          // Russet loads <dir>/Restore; a dir holding BuildManifest.plist also works
    Pmd3Cache,                            // ~/.local/share/pymobiledevice3/Xcode_iOS_DDI_Cryptex (mirror-derived; consent)
    Pmd3AutoMount { allow_mirror: bool }, // subprocess `mounter auto-mount`, opt-in (mirror = Apple files from GitHub)
}
```

- **Install.**
  - Payload paths always come from the selected BuildManifest identity
    (`Manifest.<key>.Info.Path`), never from pymobiledevice3's renamed `Image.dmg` files. Xcode's
    own `Restore/` directory (`022-20231-411.dmg`, `Firmware/…`) works as-is.
  - iOS 27.0 and later (Cryptex1): `cryptexd::install_ddi(provider, handshake, &Cryptex1Assets::load(dir))`;
    `Cryptex1Assets::load` already selects the cryptex identity. It gets a TSS ticket from Apple
    (`gs.apple.com`), then calls cryptexd `install` (SOURCE: `idevice/src/services/cryptexd.rs:380`).
    - This sends a 16.4 MB image **through the tunnel**, so it depends on the F1 clamp. It is
      untested on the rig until DC-28 (PLAN §11).
    - Xcode's values are persistence 2 and nonce-persistence 1. idevice 0.1.68 (and HEAD 3854a5d)
      sends `nonce_persistence` as `persistence` (`cryptexd.rs:273-274`); Russet carries the
      one-line fix (ARCHITECTURE §16.7) and upstreams it (U9, DEV-23).
  - iOS 17.0–26.x: `ImageMounter::mount_personalized` (TSS), with the identity's `PersonalizedDMG`
    and `LoadableTrustCache` (all 140 PDI identities of the 27A266a manifest point at the same two
    files) and `BuildManifest.plist` (`mobile_image_mounter.rs:467`). From iOS 26.4 the cryptex form
    also installs; Russet uses it for boards missing from the PDI list (pymobiledevice3 derives the
    26.4 boundary from firmware and calls it unverified below iOS 27).
  - Below iOS 17: `mount_developer` (image plus signature). This is out of scope for the
    first release; the minimum supported iOS is 17.
  - **Remount when missing.** pymobiledevice3 reports that cryptex installs with persistence 1 or 2
    survived a reboot on iOS 26.4, so the DDI may survive phone reboots on iOS 27 (unverified on
    the rig; DC-28 records it). A remount is still needed after an iOS update, on a new device, or
    to switch DDIs. The actor runs `ensure_ddi` lazily, before any DDI-dependent operation (DVT,
    debugproxy, displayservice, HID, testmanagerd), and caches the result until the next attach.
  - Optional (W10): cache the Cryptex1 IM4M per (ECID, ddiBuild, nonce), mode 0600, for offline
    remounts (`[device.ddi] ticket-cache`); pymobiledevice3 notes that CoreDevice reuses tickets.
- **Sources, in the `[device.ddi] order`** (ARCHITECTURE §14.3):
  1. A configured DDI directory (`dir`): the root of a copy of a Mac's
     `/Library/Developer/DeveloperDiskImages/iOS_DDI` (it holds `Restore/` and `version.plist`; Russet passes
     `<dir>/Restore` to `Cryptex1Assets::load`, which reads `<restore>/BuildManifest.plist`), or a `Restore/`
     directory itself. Wave 1 uses the owner's Mac copy (27A266a, Apple-origin) at
     `~/russet-lab/ddi/27A266a/iOS` (PLAN W4).
  2. The imported DDI (`imported`): `data/ddi/<ddiBuild>/iOS/Restore`, written by W16b's importer
     from the user's `Xcode_27*.xip`, `Xcode.app`, `XcodeSystemResources.pkg`, `iOS_DDI.dmg`, an
     Apple Device Support `.dmg`, or a directory. Newest `ddiBuild` first; on
     `ddiPersonalizationFailed` or `ddiIncompatible`, the next one. The default from W10.
  3. Development only, mirror-derived: an existing pymobiledevice3 cache (VERIFIED loadable).
     pymobiledevice3 11.26 fills it only from github.com/doronz88/DeveloperDiskImage (no licence),
     so it needs the same `allow-mirror` consent as source 4.
  4. Opt-in `pymobiledevice3 mounter auto-mount`, which downloads from that mirror. Apple files,
     unlicensed mirror (R2 in the research report); behind an explicit confirmation.

  Russet never hosts DDIs.
- **Errors.**
  - TSS offline → `DdiPersonalizationFailed { retryable: true }` (3010).
  - Developer Mode off → `DeveloperModeOff` (3004).
  - Wrong DDI for the iOS version or board → `DdiIncompatible` (3021).
  - No source → `ddiSourceMissing` (3020), with the `russet sdk import --ddi-only` hint.
  - A mirror source without consent → `ddiMirrorConsentRequired` (3022).

### 6.4 Apps: package, install, uninstall, list

- **IPA writer.** `IpaWriter::write(app_dir, out_ipa)`:
  - creates `Payload/<Name>.app/...`;
  - stores symlinks as zip symlink entries (Unix mode `0o120777`);
  - keeps the exec bit on Mach-O files;
  - stores already-compressed assets and compresses the rest with deflate level 1.

  This replaces xtool's call to the `zip` binary (verified-facts M1), and fixes F12.
- **Install.**
  - USB: `installation::install_package_with_callback(provider, ipa, options, cb)`. This
    is AFC to `PublicStaging`, then `installation_proxy Install` (VERIFIED, 1.2 s for
    145 KB). Progress is reported as `{percent, status}`.
  - Wi-Fi: the RSD variants (`install_package_with_callback_rsd`) through the shims.
  - Options: `{"PackageType":"Developer"}` for a directory; for an IPA, idevice sets
    `CFBundleIdentifier`.
  - Upgrade semantics: always `Install`; installd replaces the app and keeps its data.
- **Later: streaming install.** `streaming_zip_conduit` is Xcode's path [xr-15]. It is not
  in idevice. Use the `pymobiledevice3 apps install --streaming` fallback only if
  measurements show the AFC path is slow for 100+ MB apps (DEV-06 measures a 100 MB IPA).
- **Uninstall:** `installation_proxy Uninstall` over lockdown, falling back to CoreDevice
  `uninstallapp`.
- **List:**
  - CoreDevice `streamapplist` when advertised (iOS 26+ DDIs need the streaming list
    [dv-34]); VERIFIED, 0.25 s;
  - otherwise `listapps`;
  - with no tunnel, `installation_proxy Browse`.

  Output: `AppInfo { bundle_id, name, version, build, path, is_developer_app, is_removable, is_hidden }`.
- **Error mapping** (installation_proxy `Error`/`ErrorDescription`, decoded):
  - `ApplicationVerificationFailed` → `InstallFailed { kind: Signature }` (bad signature,
    entitlements, or a profile that does not cover the device);
  - 0xe8008015 → `InstallFailed { kind: NoMatchingProfile }`;
  - `DeviceOSVersionTooLow` → `InstallFailed { kind: MinimumOsTooHigh }`;
  - `MismatchedApplicationIdentifierEntitlement` → `InstallFailed { kind: AppIdMismatch }`.

  Each comes with a remedy pointing to the signing design. These codes are ASSUMPTIONS
  from community knowledge; DEV-05 builds fixtures by provoking each one with deliberately
  broken IPAs.

### 6.5 Launch, process control, console and exit

- **Default path: CoreDevice** (`com.apple.coredevice.appservice` feature
  `launchapplication`):
  1. Open `openstdiosocket` and read its UUID.
  2. Call `launch_application(bundle, args, kill_existing, suspended, env, None, Some(uuid))`
     (SOURCE: `core_device/app_service.rs:232`).
  3. Return `pid` and `executableURL`.

  VERIFIED: warm 83–148 ms, 3.8 s on the first launch after install.
- **Fallback: DVT** `ProcessControlClient::launch_app(bundle, env, args, suspended, kill_existing)`.
  Use it when CoreDevice is missing (no DDI) or fails with an unexpected error. DVT gives
  no stdio stream, only `outputReceived` messages.
- **Console.**
  - The `StdioStream` is the raw byte stream, with pseudo-terminals on
    (`standardIOUsesPseudoterminals: true`), so lines end in `\r\n`.
  - The daemon splits it into lines and tags them `stdout` (stdout and stderr are merged:
    the same UUID is used for both).
  - Writing to it sends stdin.
  - VERIFIED: a `printf` run inside the process arrived.
  - On iOS 26 and later this is the reliable source of app text, because NSLog dynamic
    strings are redacted in the unified log [dv-35].
- **Exit detection** (`ExitWatcher`), from three sources:
  1. The stdio stream closes. VERIFIED: about 130 ms after SIGKILL; jktcp reports
     "channel closed", which we map to EOF.
  2. CoreDevice `listprocesses` polled every 2 s while a run lease is active. It is cheap:
     114 ms for 583 processes.
  3. Upgrade: CoreDevice `monitorprocesstermination` (advertised in RSD, not implemented
     in idevice; port it in DEV-07b, which also gives the exit status).

  The exit status is `Unknown` until (3) lands. Crash detection: on an abnormal exit,
  `crash.list` is checked for a new `<Name>-*.ips` within 10 s (§6.10).
- **Signals:** CoreDevice `sendsignaltoprocess` (VERIFIED: 6, 9 and 19 SIGCONT). It also
  covers "Stop" (SIGKILL) and resuming a suspended app without a debugger (SIGCONT), the
  launch-suspended-then-logs flow in §6.7.
- **Process list:** CoreDevice `listprocesses` gives pid and executable URL. The bundle id
  is derived by matching the `.app` path prefix against `apps()` paths. DVT `runningProcesses`
  is the fallback.
- **Lock handling.** A launch that fails with `FBSOpenApplicationErrorDomain` code 7 maps
  to `Locked`, with the remedy "Unlock your iPhone". Optional agent tool:
  `input.button home` wakes the screen; with a passcode, the user must unlock.

### 6.6 Debug and port forwarding

- **Forwarder.**
  - For each accepted local connection, connect to the device target and run
    `copy_bidirectional`. The target is an RSD service port (resolved per tunnel
    generation), a tunnel port, or a usbmux port.
  - Set `TCP_NODELAY` on TCP.
  - Log the connect latency; VERIFIED 1.4–2.3 ms.
  - A forward is a lease and closes when its owner disconnects.
  - When the tunnel generation changes, a `RsdService`/`TunnelPort` forward re-resolves
    the port for **new** connections. Live connections die, and that is reported.
- **`debug.start`.** This is the M1 flow without pymobiledevice3; VERIFIED end to end with
  `run-debug`.
  1. `ensure_ddi`.
  2. Bind the forward first: a Unix socket `$XDG_RUNTIME_DIR/russet/dbg-<lease>.sock`
     (0600) on Linux, `127.0.0.1:0` elsewhere.
  3. Launch suspended through CoreDevice with stdio.
  4. Return `DebugSession { pid, remote_executable, gdb_remote_url, platform }`.
  5. The debug design turns that into LLDB commands:
     1. `platform select remote-ios`
     2. `target create`
     3. `SetPlatformFileSpec(remote_executable)`
     4. an async `process connect <gdb_remote_url>`
     5. `process attach --pid`
     6. `continue`
- **Service choice.** Both `com.apple.internal.dt.remote.debugproxy` and
  `com.apple.coredevice.debugserverproxy` speak raw gdb-remote on iOS 27.0 (VERIFIED with
  `qHostInfo`/`qSupported`; the latter advertises `MultiMemRead+` and `jMultiBreakpoint+`).
  The default is the first, which Xcode-era tools use; the second is a config switch.
- **`gdb_probe()`** sends `QStartNoAckMode`, `qHostInfo` and `qSupported` through a fresh
  connection. `doctor` and "is debugserver reachable?" checks use it.
- **Flutter.** The VM service URL comes from stdout (§6.7). The port is forwarded with
  `ForwardTarget::UsbmuxPort(n)`, the iproxy style; idevice's `iproxy` tool does the same.
  This is ASSUMED to reach the Dart VM's listener, because Dart binds loopback on the
  device; check it in M2. `TunnelPort(n)` is the alternative.

### 6.7 Logs

- **Sources.**
  - `os_trace_relay` (structured; lockdown over usbmux, so it survives tunnel resets):
    `start_trace(Some(pid))` filters on the device (VERIFIED). It carries level,
    subsystem/category, image path, image UUID plus offset, and mach time.
  - `syslog_relay` (text): fallback and "all device" view.
  - The app console (§6.5): the primary source of app text on iOS 26+.
- **Filters** (`LogFilter`):
  - `pid` (device side);
  - `process` name, `subsystem` prefix, `category`, `min_level`, `contains` (host side);
  - `bundle_id`, resolved to a pid at launch; a relaunch moves the subscription to the
    new pid.
- **Capturing from the first instruction.** Launch suspended, subscribe to the pid, then
  SIGCONT (VERIFIED: 1553 launch-time entries captured).
- **Volume.** About 700 entries/s system-wide on an idle phone. The daemon batches 50 ms
  windows into one notification and keeps a ring buffer of 20,000 entries per
  subscription (configurable), for MCP "tail" queries.
- **Format.** `LogEntry { ts, mach_ts, pid, process, level, subsystem, category, message, source: OsTrace|Syslog|Stdout }`.

### 6.8 Screen: screenshots, live mirror, input

#### 6.8.1 Screenshots

- **Primary: CoreDevice** `screencaptureservice` `capturescreenshot` with
  `requestedFormat: "png"` (VERIFIED, 1170×2532, about 6 fps if looped).
- **Fallback: DVT** `com.apple.instruments.server.services.screenshot` (VERIFIED, about
  7.5 fps).
- **Below iOS 17: `screenshotr`** (out of scope).
- **Do not request `jpeg`** (F9). To downscale for agents, decode the PNG on the host and
  re-encode it.

#### 6.8.2 Live mirror (CoreDevice displayservice)

Protocol (SOURCE: `idevice/src/services/core_device/display_stream/*`,
`tools/src/coredevice_stream.rs`; VERIFIED on iOS 27.0):

1. Connect `com.apple.coredevice.displayservice` (entitlement
   `com.apple.private.CoreDevice.canViewDeviceDisplay`).
2. Bind two UDP sockets **on the userspace tunnel** with `AdapterHandle::bind_udp(0)`,
   one for audio and one for video.
3. Build the offers:
   - a `VCCallInfoBlob`;
   - `build_screen_audio_offer`;
   - `startmediastream` for audio, which establishes the session;
   - then the video offer with `clientSupportedFeatures = 140` (the value Device Hub
     sends), `display_id 1`, and the same `clientSessionID`.

   All fields are listed in `coredevice_stream.rs`.
4. The device sends **plaintext RTP**, PT 100, HEVC (RFC 7798):
   - `HevcDepacketizer` produces an Annex-B access unit stream;
   - VERIFIED: HEVC Main, 1184×2576, the full screen scaled (no padding observed),
     25–56 fps under motion, about 2.4 Mbit/s.
5. Stop with `stopmediastream`. The offer caps the bitrate at 6 Mbit/s
   (`negotiation.rs`).

Daemon design:
- **`DisplaySession`: at most one per device, reference-counted.** The holders are:
  - a `Mirror` lease, which wants frames;
  - an `Input` lease, which only needs the HID gate open.

  The session starts on the first holder and stops 2 s after the last holder leaves.
  Input-only use discards RTP but keeps the sockets bound. jktcp buffers unread datagrams,
  so the session must drain them into a sink.
- **Delivery to clients.**
  - A loopback WebSocket server in russetd, bound to `127.0.0.1:0`. The URL carries a
    random 128-bit token: `ws://127.0.0.1:PORT/mirror/<id>?t=<token>`.
  - The IDE webview connects and feeds WebCodecs `VideoDecoder` (codec `hvc1.1.6.L150.B0`
    from the SPS), with no `description`, so the input is Annex-B.
  - Wire message: `[u8 kind=1][u8 flags: bit0 keyframe][u64 pts_us][payload Annex-B AU]`.
  - The first message after connect is always a keyframe, and parameter sets are
    prepended. On (re)connect the daemon sends RTCP PLI (`build_pli`), or FIR or
    `build_keyframe_request`, and holds frames until an IDR arrives.
  - CLI: `russet mirror --stdout | ffplay -f hevc -` (raw Annex-B), and
    `russet mirror --record out.h265`.
- **Fallback when HEVC cannot be decoded** (see OQ-1) or displayservice is missing (iOS
  17/18 is ASSUMED unsupported until tested):
  - screenshot polling at up to 7 fps through the same WebSocket, `kind=2` PNG frames;
  - optional daemon-side transcoding, if the user has `ffmpeg` installed (we do not ship
    it).
- **QuickTime-over-USB (quicktime_video_hack) is not used.**
  - It needs libusb to switch the device into a hidden USB configuration (control request
    `0x40, 0x52, 0x00, 0x02`). That makes the device disconnect and come back, and it
    competes with usbmuxd for the device.
  - Its author gave up on Windows (danielpaulus/quicktime_video_hack README and
    `doc/technical_documentation.md` §1.4; MIT).
  - displayservice gives better results over the existing tunnel.

#### 6.8.3 Input (HID)

Services, with entitlement `com.apple.private.CoreDevice.hid` (RSD; VERIFIED present):
- `com.apple.coredevice.hid.indigo`: button, scroll, digitizer, vendordefined.
- `com.apple.coredevice.hid.universalhidservice`: touchscreen and keyboard reports.

`dtuhidd` drops every event unless a displayservice session is active (SOURCE:
`core_device/hid.rs:1-11`). That is why `Input` holds the display session.

```rust
pub enum InputEvent {
    Tap { x: f32, y: f32, hold_ms: u32 },                       // normalized 0..1
    Swipe { from: (f32, f32), to: (f32, f32), duration_ms: u32 }, // steps = duration/16ms
    LongPress { x: f32, y: f32, hold_ms: u32 },
    Button { button: HwButton, hold_ms: Option<u32> },          // Home, Lock, VolUp, VolDown, Mute, Siri
    Text(String),                                               // US-ASCII via keyboard usages
    Key(KeyName),                                               // return, esc, tab, arrows, backspace
}
```

- **Coordinates.** Normalized floats are mapped to `0..=65535`. The IDE maps a click in the
  video frame proportionally; the frame covers the whole screen. MCP tools accept pixel
  coordinates of the last screenshot (1170×2532 on this phone) and normalize them.
- **Tap recipe (VERIFIED):**
  1. Contact.
  2. 3 contact samples spread over `hold_ms` (default 120).
  3. Release.
  4. Keep the session for at least 500 ms afterwards. This is automatic when a mirror is
     live.
- **Button usage pages** (from `tools/src/hid.rs`): Home `0x0C/0x40`, Lock `0x0C/0x30`,
  VolUp `0x0C/0xE9`, VolDown `0x0C/0xEA`, Mute `0x0C/0xE2`, Siri `0x0C/0xCF`.
- **Text.** `type` maps ASCII to HID usages, US layout only. Non-ASCII text goes through
  the CoreDevice pasteboard (`pasteboardservice`) plus a paste gesture. That is a later
  task.

#### 6.8.4 UI elements for agents

- **WDA/XCUITest gives element trees and taps by label.** But building the runner needs
  `xcodebuild`, so it is not off-Mac (research §2.10). idevice can launch an
  already-installed runner (`xctest`/`wda` features), so WDA stays an opt-in path for
  users who have a runner.
- **Default: the accessibility audit service** (`com.apple.accessibility.axAuditDaemon.remoteserver`).
  It is reached through the pymobiledevice3 fallback `developer accessibility list-items`
  (VERIFIED: captions only).
- A native port of the AX audit protocol, with element frames, is a later task (DEV-20b).

### 6.9 Files

- **Scopes:**
  - `Media`: AFC jail (`/DCIM`, `/Downloads`, …);
  - `AppContainer{bundle}` / `AppDocuments{bundle}`: house_arrest `VendContainer` or
    `VendDocuments`; only for developer-signed apps;
  - `CrashReports`: crashreportcopymobile.
- **Operations:** list, stat, pull (streamed to a file), push, mkdir, remove, remove_all.
- **Performance.** VERIFIED: about 32–37 MiB/s over usbmux; a house_arrest open costs about
  1 s, so the `FileClient` is cached for 60 s.
- **Later:** CoreDevice `fileservice.control`/`data`. It has domains, `monitorfilechanges`,
  and the iOS 27 `watch` that pymobiledevice3 has. It feeds an IDE "app container"
  explorer with live updates.

### 6.10 Crash reports

1. `flush`: crashreportmover moves new reports into the copy area. VERIFIED, 0.17 s.
2. `list` (recursive): filter by `<ProcessName>-YYYY-MM-DD-HHMMSS.ips` and by the time
   window.
3. `pull`.
4. Parse `.ips`:
   - line 1 is a JSON header (`app_name`, `bundleID`, `slice_uuid`, `bug_type`,
     `os_version`);
   - the rest is a JSON body (`exception`, `termination`, `faultingThread`, `threads[].frames[]`,
     `usedImages[]`).

```rust
pub struct IpsReport {
    pub header: IpsHeader,               // bug_type 309 = app crash; 298 = jetsam
    pub exception: Option<IpsException>, // { type: "EXC_CRASH", signal: "SIGABRT", codes }
    pub termination: Option<IpsTermination>,
    pub faulting_thread: Option<usize>,
    pub threads: Vec<IpsThread>,         // frames: { image_index, image_offset, symbol? }
    pub images: Vec<IpsImage>,           // { uuid, name, base, path }
    pub raw: serde_json::Value,
}
```

- **Symbolication.** System frames already carry names (VERIFIED). App frames are matched
  by image UUID against the local build (`slice_uuid`) and symbolized with
  `llvm-symbolizer` or `atos`-style lookup on the dSYM or binary. That is a debug-design
  hook; the device plane supplies `image_offset` and `uuid`.
- **Auto-capture.** After an abnormal exit (§6.5), poll `flush` and `list` for 10 s, then
  emit `app.crashed { report }`.

### 6.11 Profiling streams

- **DVT `sysmontap`** (VERIFIED).
  - First fetch the attribute lists through `DeviceInfoClient::sysmon_process_attributes()`
    and `sysmon_system_attributes()`.
  - Then `SysmontapConfig { interval_ms, proc_attrs, sys_attrs }`.
  - The output is per sample: a system vector, `cpu_load`, and per-process rows.
  - The daemon converts attribute-indexed arrays into named JSON:

```json
{"t": 1728600000.5, "system": {"cpuLoad": 84.0, "physMemSize": 4000000000, "vmFreeCount": 16818},
 "processes": [{"pid": 976, "name": "Hello", "cpuUsage": 3.1, "physFootprint": 52428800, "threadCount": 9}]}
```

- **Default attribute subset for the IDE graphs:**
  - process: `cpuUsage`, `physFootprint`, `memResidentSize`, `threadCount`,
    `diskBytesRead`, `diskBytesWritten`, `msgSent`, `msgRecv`;
  - system: `cpuLoad`, `vmFreeCount`, `vmUsedCount`, `netBytesIn`, `netBytesOut`.
- **Also available in idevice:** `energy_monitor`, `network_monitor`, `graphics` (GPU
  utilization and fps), `condition_inducer` (thermal/network conditions). Expose them as
  separate streams with the same envelope. DEV-16 checks each on iOS 27.
- **Export:** JSON lines (`russet perf --out run.jsonl`). A Perfetto converter is later
  (research §2.11).

### 6.12 Symbols (DSC)

- **Service:** `com.apple.dt.remoteFetchSymbols` (feature `dyldSharedCacheFiles`; VERIFIED
  advertised). idevice does not implement it.
- **Phase 1:** `pymobiledevice3 developer fetch-symbols download <dir> --userspace`
  through the fallback runner, with progress parsed from stderr.
- **Phase 2:** a native implementation in `russet-device`. GPL code may be ported here, but
  not into MIT upstream PRs.
- **Size:** 82 files on iOS 27.0 (VERIFIED list). Total size ASSUMED 3–5 GB per OS build;
  measure in DEV-17 on a machine with room.
- **Cache layout:** `<cache>/ios/<ProductType>/<Build>/` (for example
  `iPhone13,2/24A437/`), with `dyld_shared_cache_arm64e*` and `dyld`.
- **Disk guard:**
  - refuse to start if free space is below the size plus 2 GB;
  - resume partial files;
  - a single global download at a time.
- The debug design consumes it: `ipsw dyld extract` / LLDB settings. That this speeds up
  attach (F11) is ASSUMED; the debug design checks it.

### 6.13 Extras

All of these are cheap, and agents and the IDE want them. All exist in idevice; none was
exercised in the spike.

| Capability | Service | API |
|---|---|---|
| Simulate location | `com.apple.coredevice.locationservice` (17+); DVT `location_simulation` fallback | `location.set {lat, lon}`, `location.clear` |
| Pasteboard | `coredevice.pasteboardservice` | `pasteboard.get/set` (text) |
| Orientation | `coredevice.devicecontrol` | `device.rotate {dir}` |
| Appearance (dark/light) | `coredevice.configuration` (`customizeuistyle`) | `device.appearance {style}` |
| App icon | `coredevice.iconservice` | `app.icon {bundleId}` → PNG (raw RGBA to PNG, per `app_service.rs` IconData notes) |
| XCTest runner | DVT/testmanagerd (idevice `xctest`) | `test.run {runnerBundleId, env}` (testing design) |

---

## 7. Daemon JSON-RPC surface (device namespace)

> **Superseded (ARCHITECTURE §8.2, §20.1):** the binding names and shapes are in ARCHITECTURE:
> `device.ddi.mount` and `device.ddi.list` (not `device.ddi.install`), `debug.prepare` (not
> `debug.start`), `screen.elements` (not `ui.elements`), `debug.symbols.fetch`, `device.changed`,
> `app.output`, `app.exited {crashId}`, and numeric error codes (§13). This table is kept for its
> rationale.

The framing (JSON-RPC 2.0 over a local socket), authentication, and the generic `$/progress`
and `$/cancel` conventions belong to the daemon design. ASSUMPTION: progress notifications
carry a client-supplied `progressToken`. Every method takes `udid`; it may be omitted when
exactly one device is ready. Long-lived results return a `leaseId`. Leases end when the
JSON-RPC connection closes, or through `lease.release`.

| Method | Params | Result | Notes |
|---|---|---|---|
| `device.list` | `{}` | `{devices: DeviceSummary[]}` | |
| `device.watch` | `{}` | `{leaseId}` | Notifications `device.added/changed/removed` |
| `device.info` | `{udid}` | `DeviceInfo` (lockdown values, RSD properties, marketing name, chrome id, screen) | |
| `device.readiness` | `{udid, refresh?}` | `Readiness` | |
| `device.prepare` | `{udid, ddiSource?, revealDeveloperMode?: true}` | `Readiness` | Pairs if needed (returns `TrustPending`), reveals Developer Mode, `ensure_ddi` |
| `device.pair` | `{udid, hostName?}` | `{status: "paired" \| "pending" \| "denied"}` | |
| `device.ddi.status` / `device.ddi.install` | `{udid}` / `{udid, source}` | `DdiStatus` | |
| `app.list` | `{udid, filter: {kind: "user" \| "all", bundleIdPrefix?}}` | `{apps: AppInfo[]}` | |
| `app.install` | `{udid, path, progressToken?}` | `{bundleId, path, durationMs}` | `path` = `.ipa` or `.app` (the daemon packs the IPA) |
| `app.uninstall` | `{udid, bundleId}` | `{}` | |
| `app.launch` | `{udid, bundleId, args?, env?, suspended?, killExisting?: true, console?: true}` | `{pid, executable, leaseId, consoleStream?}` | Notifications `app.console {leaseId, data}`, `app.exited {leaseId, pid, status}`, `app.crashed {leaseId, report}` |
| `app.terminate` | `{udid, pid \| bundleId}` | `{}` | SIGKILL |
| `process.list` | `{udid}` | `{processes: ProcessInfo[]}` | |
| `process.signal` | `{udid, pid, signal}` | `{}` | |
| `debug.start` | `{udid, bundleId, args?, env?, bind?: "unix" \| "tcp"}` | `{leaseId, pid, gdbRemoteUrl, remoteExecutable, platform}` | Notifications as for `app.launch` |
| `debug.stop` | `{leaseId, kill?: true}` | `{}` | |
| `forward.open` | `{udid, target: {service} \| {tunnelPort} \| {usbmuxPort}, bind?: {tcp: "127.0.0.1:0"} \| {unix: true}}` | `{leaseId, local: "127.0.0.1:41250" \| "/run/user/1000/russet/fwd-…sock"}` | |
| `logs.subscribe` | `{udid, filter: LogFilter, sources: ["oslog", "stdout", "syslog"], backlog?: 0}` | `{leaseId}` | Notifications `logs.entries {leaseId, entries: LogEntry[]}`, batched every 50 ms |
| `logs.tail` | `{udid, leaseId?, bundleId?, lines?: 200, minLevel?, contains?}` | `{entries}` | From the ring buffer; starts a 5 s capture if none exists |
| `screen.screenshot` | `{udid, maxDimension?, format?: "png"}` | `{png: base64, width, height}` | Or `{path}` when `toFile: true` |
| `screen.mirror.start` | `{udid}` | `{leaseId, url, codec: "hevc", width, height}` | |
| `input.tap` / `input.swipe` / `input.button` / `input.text` | `{udid, x, y, space?: "normalized" \| "screenshot"}` and so on | `{}` | Holds an input lease for 2 s |
| `ui.elements` | `{udid}` | `{elements: [{label, uid}]}` | pymobiledevice3 fallback |
| `files.list` / `files.pull` / `files.push` / `files.remove` | `{udid, scope: {kind: "media" \| "appContainer" \| "appDocuments" \| "crashReports", bundleId?}, path, local?}` | Varies | |
| `crash.list` / `crash.pull` | `{udid, process?, since?}` / `{udid, name}` | `{reports: CrashReportMeta[]}` / `{path, summary}` | |
| `perf.subscribe` | `{udid, pid?, intervalMs: 500, metrics?}` | `{leaseId}` | Notifications `perf.sample` |
| `symbols.fetch` | `{udid, progressToken?}` | `{path, bytes}` | |
| `location.set` / `location.clear` | `{udid, lat, lon}` | `{}` | |
| `lease.release` | `{leaseId}` | `{}` | |

CLI mapping (the `russet` binary talks to the daemon, or runs in-process with `--no-daemon`):
- `russet device list|info|prepare|pair`
- `russet install <ipa|app>`
- `russet launch <bundle> [--suspended] [--env K=V]`
- `russet debug <bundle>` (prints the LLDB `attach.lldb`)
- `russet forward <service|port>`
- `russet logs [--app <bundle>] [--level info]`
- `russet screenshot [-o x.png]`
- `russet mirror [--stdout|--record f]`
- `russet tap X Y`, `russet button home`, `russet type "text"`
- `russet crash list|pull`
- `russet files …`
- `russet perf [--pid]`
- `russet symbols fetch`

---

## 8. MCP tools (device subset)

> **Superseded (ARCHITECTURE §11.4, §20.1):** canonical tool names are unprefixed (`device_list`,
> `input_tap`, …), tap coordinates default to `screenshot`, and log levels use Apple's names
> (`default`, never `notice`).

The MCP server is part of russetd; its general design is elsewhere. Device tools are thin
wrappers over §7. Tool names are prefixed `ios_`. Agents only get destructive actions
(uninstall, files remove, signal) when the user enables "allow destructive device
actions".

```json
[
 {"name":"ios_devices","description":"List connected iPhones and whether they are ready to run apps.",
  "inputSchema":{"type":"object","properties":{}}},
 {"name":"ios_install_app","description":"Install a built .ipa or .app on the device.",
  "inputSchema":{"type":"object","required":["path"],"properties":{"udid":{"type":"string"},"path":{"type":"string"}}}},
 {"name":"ios_launch_app","description":"Launch an installed app; returns pid. Console output is captured and readable with ios_logs.",
  "inputSchema":{"type":"object","required":["bundle_id"],"properties":{"udid":{"type":"string"},"bundle_id":{"type":"string"},
   "args":{"type":"array","items":{"type":"string"}},"env":{"type":"object","additionalProperties":{"type":"string"}}}}},
 {"name":"ios_stop_app","description":"Terminate a running app.",
  "inputSchema":{"type":"object","required":["bundle_id"],"properties":{"udid":{"type":"string"},"bundle_id":{"type":"string"}}}},
 {"name":"ios_screenshot","description":"Capture the screen. Returns a PNG image (downscaled) and the pixel size used for tap coordinates.",
  "inputSchema":{"type":"object","properties":{"udid":{"type":"string"},"max_dimension":{"type":"integer","default":1280}}}},
 {"name":"ios_tap","description":"Tap at a point given in pixels of the most recent ios_screenshot (or normalized 0..1 with space='normalized').",
  "inputSchema":{"type":"object","required":["x","y"],"properties":{"udid":{"type":"string"},"x":{"type":"number"},"y":{"type":"number"},
   "space":{"enum":["screenshot","normalized"],"default":"screenshot"},"hold_ms":{"type":"integer","default":120}}}},
 {"name":"ios_swipe","description":"Swipe from one point to another (same coordinate rules as ios_tap).",
  "inputSchema":{"type":"object","required":["x1","y1","x2","y2"],"properties":{"udid":{"type":"string"},"x1":{"type":"number"},"y1":{"type":"number"},
   "x2":{"type":"number"},"y2":{"type":"number"},"duration_ms":{"type":"integer","default":250},"space":{"enum":["screenshot","normalized"]}}}},
 {"name":"ios_press_button","description":"Press a hardware button.",
  "inputSchema":{"type":"object","required":["button"],"properties":{"udid":{"type":"string"},"button":{"enum":["home","lock","volume_up","volume_down","mute"]}}}},
 {"name":"ios_type_text","description":"Type ASCII text into the focused field.",
  "inputSchema":{"type":"object","required":["text"],"properties":{"udid":{"type":"string"},"text":{"type":"string","maxLength":2000}}}},
 {"name":"ios_ui_elements","description":"List accessibility elements on screen (labels).",
  "inputSchema":{"type":"object","properties":{"udid":{"type":"string"}}}},
 {"name":"ios_logs","description":"Recent log lines and console output of an app.",
  "inputSchema":{"type":"object","properties":{"udid":{"type":"string"},"bundle_id":{"type":"string"},"lines":{"type":"integer","default":200,"maximum":2000},
   "min_level":{"enum":["debug","info","notice","error","fault"]},"contains":{"type":"string"}}}},
 {"name":"ios_crash_reports","description":"List and summarize recent crash reports for an app (exception, faulting thread, top frames).",
  "inputSchema":{"type":"object","properties":{"udid":{"type":"string"},"bundle_id":{"type":"string"},"since_minutes":{"type":"integer","default":60}}}},
 {"name":"ios_set_location","description":"Simulate GPS location.",
  "inputSchema":{"type":"object","required":["lat","lon"],"properties":{"udid":{"type":"string"},"lat":{"type":"number"},"lon":{"type":"number"}}}}
]
```

Tool results carry the `DeviceError` remedy text, so agents can tell the user, for
example, "Unlock your iPhone".

---

## 9. Error model

> **Superseded (ARCHITECTURE §13.3):** device error codes are 3000–3299 (see the remap table there);
> the 1100–1191 range below collided with `ipc`.

```rust
#[derive(thiserror::Error, Debug)]
#[error("{kind}: {message}")]
pub struct DeviceError {
    pub kind: ErrorKind,
    pub message: String,
    pub remedy: Option<Remedy>,
    pub retryable: bool,
    pub source_chain: Vec<String>,          // flattened idevice / NSError / io chain
    #[source] pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}
pub struct Remedy { pub text: String, pub action: Option<RemedyAction> }
pub enum RemedyAction { UnlockDevice, TapTrust, EnableDeveloperMode, Replug, InstallUsbmuxd,
                        StartAmds, ProvideDdi, FixSigning, FreeDisk, UpdateRusset }
```

| `ErrorKind` (JSON `data.kind`) | Code | Raised when (source) | Remedy |
|---|---|---|---|
| `mux.unavailable` | 1100 | Cannot connect to usbmuxd or AMDS | Install or start usbmuxd (Linux) or Apple Mobile Device Service (Windows); `doctor` |
| `device.not_found` | 1101 | UDID not attached | Replug; check the cable |
| `device.disconnected` | 1102 | Detached mid-operation; tunnel EOF | Replug; lease ended |
| `pairing.required` | 1110 | `InvalidHostID`, no pair record | `device.pair` |
| `pairing.pending` | 1111 | `PairingDialogResponsePending` | Unlock and tap Trust |
| `pairing.denied` | 1112 | `UserDeniedPairing` | Re-pair; reset trust in Settings |
| `device.locked` | 1113 | `PasswordProtected`, `DeviceLocked`; CoreDevice 10002 → FBS code 7 "Locked" (VERIFIED) | Unlock the iPhone |
| `devmode.off` | 1120 | `DeveloperModeNotEnabled`; amfi status false | Settings → Privacy & Security → Developer Mode (reboot) |
| `ddi.missing` | 1121 | `ImageNotMounted`; `ServiceNotFound` for DDI services | `device.prepare` |
| `ddi.personalization_failed` | 1122 | TSS errors, `BadBuildManifest` | Network to Apple TSS; correct DDI |
| `tunnel.failed` | 1130 | CoreDeviceProxy start or CDTunnel handshake failed; RSD timeout | Retry; replug; `doctor` |
| `tunnel.stalled` | 1131 | jktcp "timed out after 5 retransmissions" | Report a bug (should not happen with the clamp); retry |
| `service.unavailable` | 1132 | RSD service not advertised | Update iOS or Russet; DDI |
| `service.not_implemented` | 1133 | `CoreDevice.ActionError` code 2 (VERIFIED for getlockstate) | Feature not supported on this iOS |
| `install.failed` | 1140 | installation_proxy errors (subkinds §6.4) | Signing remedies |
| `launch.failed` | 1141 | CoreDevice launch error; DVT error | Depends on the underlying error |
| `app.not_installed` | 1142 | Bundle missing in `apps()`; FBS "not found" | Install first |
| `timeout` | 1150 | Any operation over its budget | Retry |
| `unsupported` | 1160 | iOS < 17; Wi-Fi tunnel not available | |
| `fallback.unavailable` | 1170 | pymobiledevice3 missing or too old | Install pymobiledevice3 ≥ 11.26 (`uv tool install pymobiledevice3`) |
| `io` / `protocol` | 1190 / 1191 | Unexpected I/O or format errors | Report |

**NSError chain decoder.** CoreDevice errors arrive as `CoreDevice.error` dictionaries that
contain `domain`, `code` and `userInfo`, often with `NSUnderlyingError` nested two or three
levels deep (VERIFIED example in Appendix A.7). `nserror::NsErrorChain::from_plist(&Value)`
walks it into `Vec<(domain, code, description, failure_reason)>`. The mapping rules match
on `(domain, code)`:
- `(FBSOpenApplicationErrorDomain, 7)` → `device.locked`;
- `(CoreDevice.ActionError, 2)` → `service.not_implemented`.

The full chain is kept in `source_chain`. Logs never print the raw
`userInfoWithNSSecureCoding` blobs (they are huge); they print the decoded chain.

**Retry policy:**
- idempotent reads (list, info, status): retried once after a tunnel rebuild;
- `install`: retried once on `tunnel.*` or `device.disconnected` only;
- `launch`: not retried automatically (side effects), except on `tunnel.failed` before
  the request was sent.

---

## 10. Linux and Windows

| Topic | Linux (first) | Windows (second, physical PC) |
|---|---|---|
| Mux daemon | Distro usbmuxd (1.1.1 here) on `/var/run/usbmuxd` (0666), started by udev and systemd. VERIFIED, no root | **Apple Mobile Device Service** (iTunes, or the Microsoft Store "Apple Devices" app) on `127.0.0.1:27015`. This is idevice's default on non-Unix (SOURCE `usbmuxd/mod.rs:83,141-148`) |
| Alternatives | netmuxd (LGPL-2.1): USB through nusb, no usbmuxd needed, also Wi-Fi | netmuxd. Either take over Apple's mux (`--kill-amds`, which needs Admin on every run, or `install-service` with Admin once), or use libusb0 through libwdi with no Apple software (Admin; ARM64 needs test signing) (SOURCE: netmuxd README, v0.4.3) |
| Detection (`doctor`) | Socket exists; `ListDevices` answers; usbmuxd version (`usbmuxd --version`) | TCP 27015 answers. The service `ImagePath` in `HKLM\SYSTEM\CurrentControlSet\Services\Apple Mobile Device Service` tells the Store app from iTunes; pymobiledevice3's `win_util.py` does the same. The Store app's service **does not discover Wi-Fi devices** (pymobiledevice3#1968, cited in `win_util.py`) |
| Pair records | usbmuxd (`/var/lib/lockdown`, 0644) | usbmux `ReadPairRecord`, then `%ALLUSERSPROFILE%\Apple\Lockdown\<UDID>.plist`. The ACL may need Admin: ASSUMPTION, DEV-22 |
| Tunnel | Userspace (jktcp), no TUN | Same; no wintun and no Admin (userspace only) |
| Debug forward | Unix socket in `$XDG_RUNTIME_DIR/russet/` (0600), TCP optional | TCP `127.0.0.1:0`. Binding to loopback should not raise a firewall prompt (ASSUMPTION, DEV-22). LLDB from the LLVM MSI connects to it (debug design) |
| Mirror WebSocket | `127.0.0.1:0` plus a token | Same |
| mDNS (Wi-Fi) | Own responder or browser (`mdns-sd`) with SO_REUSEADDR next to avahi (5353) | Apple's Bonjour service may own 5353. Use `mdns-sd` with reuse, or rely on AMDS/netmuxd discovery (ASSUMPTION, DEV-21/22) |
| Paths | `$XDG_STATE_HOME/russet/device`, `$XDG_CACHE_HOME/russet/symbols` | `%LOCALAPPDATA%\Russet\device`, `%LOCALAPPDATA%\Russet\symbols` |
| Crypto build | ring or aws-lc (cmake present) | ring (avoids CMake and NASM) |
| WSL2 split mode | n/a | russetd runs on Windows; the build runs in WSL2. LLDB in WSL needs mirrored networking to reach Windows `127.0.0.1` ([dv-32]; research §5.4), or run LLDB on Windows |
| Windows on ARM | n/a | idevice is pure Rust; ring builds on aarch64-pc-windows-msvc (ASSUMPTION). netmuxd libusb mode needs test signing |

idevice evidence on Windows: CrossCode ("iOS Development IDE for windows/linux", MIT,
github.com/nab138/CrossCode) uses `UsbmuxdConnection::default()` and idevice for all device
I/O (`src-tauri/src/sideloader/device.rs`). We have not run it ourselves.

---

## 11. Wi-Fi devices

Design only; nothing here was exercised (§2.2).

1. **Enable.** `device.wifi.enable {udid}` sets the lockdown key
   `com.apple.mobile.wireless_lockdown/EnableWifiDebugging=true` over USB. This is the
   "Show this iPhone when on Wi-Fi" toggle. It is a user-initiated device setting, logged
   and reversible.
2. **Wireless identity over USB.**
   - `remotepairing_lockdown pair` (idevice `RemotePairingLockdownService`) creates an
     RPPairing record without a prompt, through `com.apple.dt.remotepairingdeviced.lockdown.shim.remote`.
     VERIFIED: the service is present and `allowsPinlessPairing=true`.
   - Store it at `<state_dir>/rppairing/<UDID>.plist`.
   - iOS 27 also supports device-initiated pairing: advertise
     `_remotepairing-pairable-host._tcp` and type a PIN on the phone (idevice `pair_host`).
     Offer it as "Pair over Wi-Fi" when no cable is available.
3. **Discovery.**
   - Browse `_remotepairing._tcp` (RemoteXPC) and `_apple-mobdev2._tcp` (lockdown Wi-Fi
     sync).
   - On iOS 26.4+, match devices with idevice `mdns::derive_auth_tag(host_id, identifier)`
     against the advertised `authTag`s (SOURCE: `idevice/src/mdns.rs`).
4. **Tunnel.** Either:
   - (a) `RemotePairingClient` pair-verify with the stored record, then the untrusted
     tunnel service, then `connect_tls_psk_tunnel_native` (pure Rust TLS-PSK), then a
     `CdTunnel`, the same jktcp adapter and RSD. Or:
   - (b) lockdown over TCP (`TcpProvider{addr, pairing_file}`, port 62078), then
     CoreDeviceProxy over that connection. pymobiledevice3 offers both.

   Default: (a). Keepalive: idevice `heartbeat` for lockdown-over-Wi-Fi connections.
5. **MSS over Wi-Fi.** The MSS limit is unknown, so start at 1340 (pymobiledevice3's
   proven seed) until DEV-21 measures it.
6. **Firewall and networks.**
   - `doctor` checks UDP 5353 and the reachability of the device IP.
   - Client-isolated networks (likely the spike LAN) block it.
   - Report "Wi-Fi debugging needs both on the same LAN without client isolation".

---

## 12. pymobiledevice3 fallback

- **Licence and shipping.** pymobiledevice3 is GPL-3.0, the same family as Russet. It is
  optional and not bundled: the user installs it (`uv tool install pymobiledevice3`).
- **Detection.** `pymobiledevice3 version`, minimum `11.26.0`; the latest is 11.26.1
  (2026-10-10).
- **Runner** (`fallback::Pmd3`):
  - `tokio::process::Command` with `--udid <UDID>` and `--userspace` (iOS 17+);
  - `NO_COLOR=1`, `PYTHONWARNINGS=ignore`;
  - kill-on-drop, a per-call timeout, and stdout parsed as JSON where the command prints
    JSON;
  - capability probing with `--help` once per version, cached.
- **Coexistence.** It opens its own tunnel. VERIFIED to coexist with the daemon's tunnel
  on iOS 27.0 USB (F3).

| Capability | Command | Until |
|---|---|---|
| DSC symbols | `developer fetch-symbols download <dir> --userspace` | A native `remoteFetchSymbols` (DEV-17b) |
| Accessibility elements | `developer accessibility list-items --userspace` | A native AX port (DEV-20b) |
| DDI from mirror (opt-in) | `mounter auto-mount` | Never default |
| Streaming install | `apps install --streaming --developer <ipa> --userspace` | Measured need |
| WebInspector (Safari/WKWebView, React Native) | `webinspector …` | Later |
| sysdiagnose | `developer core-device sysdiagnose` | Later |
| Display info, lock state | `developer core-device get-display-info` | n/a (getlockstate: not implemented on 27.0) |

---

## 13. Upstream work (idevice and jktcp; MIT; clean of GPL code)

| # | Change | Why | Evidence |
|---|---|---|---|
| U1 | Clamp the USB CoreDeviceProxy MSS to ≤ 8132 in `create_software_tunnel` (or honour the peer MSS and add PLPMTUD to jktcp) | Bulk host-to-device stalls (F1) | §2.2 afc-bench; pcap SYN-ACK MSS 15940 |
| U2 | jktcp: surface a FIN as EOF (`Ok(0)`) instead of a "channel closed" error | Clean stdio and forward shutdown | §2.2 "App exit" |
| U3 | `ScreenCaptureService` JPEG: find the accepted value or remove the variant | iOS 27 rejects `"jpeg"` | §2.2 #9 |
| U4 | HID `tap()`: position samples, hold ≥ 100 ms; the tool keeps the stream for 500 ms after the release | The stock tap is a no-op on 27.0 | §2.2 |
| U5 | `afc_upload_dir`: handle symlinks (create an AFC link) or document | Silent loss of symlinks (F12) | Code reading |
| U6 | New: CoreDevice `monitorprocesstermination`, `streamprocesslist`, `deviceinfo` (`getdisplayinfo`) | Exit codes; display size | RSD inventory |
| U7 | New: `remoteFetchSymbols`, `streaming_zip_conduit` | Symbols; Xcode-style install | |
| U8 | Release a version containing the `Send` usbmuxd `listen()` (and the short-packet check) | Daemon ergonomics; no panic on short packets | §2.1 diff |
| U9 | `CryptexdClient::install()`: send `request.persistence` as `persistence` (it sends `request.nonce_persistence`) | Match Xcode's install values (persistence 2, nonce-persistence 1) | `cryptexd.rs:273-274` in 0.1.68 and at 3854a5d |
| U10 | usbmuxd `read_plist`: reject a declared packet size above 16 MiB with `BadCommand` before allocating | 3854a5d rejects sizes below 17 but still allocates `vec![0; size - 16]`, up to about 4 GiB, so a mock or a fuzzer can exhaust memory (ARCHITECTURE §16.7) | `idevice/src/usbmuxd/mod.rs:467-474` at 3854a5d |

Every upstream PR is written from protocol observations and the idevice code, never from
pymobiledevice3 source, to keep MIT clean.

---

## 14. Security

- **debugserver means code execution in the app.** A forwarded gdb-remote port has no
  authentication. Mitigations:
  - On Linux, a Unix socket under `$XDG_RUNTIME_DIR` (0700 directory, 0600 socket);
    VERIFIED to work with LLDB.
  - On Windows, TCP on loopback, accept-once (close the listener after the first
    connection), and a 60 s connect deadline.
- **Mirror and input WebSockets:** loopback, a random 128-bit token, and an `Origin` check
  against the IDE origin.
- **Leases are tied to the client connection**, so nothing outlives a crashed IDE for
  long (2 s grace).
- **Pair and RPPairing records are secrets.** Mode 0600; never logged; Windows ACL
  (current user).
- **No device setting changes without an explicit request:** Wi-Fi enable, Developer
  Mode, DDI install. The MCP gates destructive tools (§8).

---

## 15. Test plan

1. **Unit tests** (no device). They use fixtures from this spike, copied into
   `crates/russet-device/tests/fixtures/` without personal data:
   - NSError chain decoding (the Locked error, `ActionError 2`);
   - the IPA writer: symlinks, exec bits, `unzip -Z` listing;
   - the `.ips` parser (Hello SIGABRT, Jetsam 298);
   - the sysmon attribute-to-JSON mapping;
   - HID coordinate mapping;
   - log filter and batching;
   - MSS selection.
2. **Protocol replay:**
   - RSD handshake bytes;
   - a CoreDevice `listprocesses` reply;
   - a pcap of the tunnel (`RUSSET_TUNNEL_PCAP`) driving jktcp in a loopback harness.
   These guard against idevice upgrades.
3. **Fake mux.** A usbmuxd server built on idevice's `usbmuxd::server` types emits
   attach and detach events and errors, to test the actor, leases and remedies without
   hardware.
4. **HIL suite** (`russet-device-hil`, `RUSSET_HIL_UDID` set, serial, about 3 min). It
   repeats §2.2 with budgets:

   | Test | Budget |
   |---|---|
   | Session open | < 300 ms (p95) |
   | Install of a 145 KB IPA | < 3 s |
   | Install of a 100 MB IPA | < 15 s, measured |
   | Warm launch | < 500 ms; first launch < 10 s |
   | debug.start to LLDB stopped at breakpoint | < 15 s with `minimal` |
   | Log latency | < 500 ms |
   | Screenshot | < 400 ms |
   | Mirror median fps during a scripted swipe loop | ≥ 25 |
   | Tap launches an app icon | 20 / 20 |
   | 32 MiB AFC through the tunnel | ≥ 20 MiB/s both ways |
   | Concurrent pymobiledevice3 tunnel | Does not break the session |

5. **Soak** (2 h). Mirror plus logs, plus an install–launch–debug–kill cycle every 2 min.
   Pass: no leaked tasks, stable RSS, no tunnel stalls.
6. **Manual scenarios:**
   - replug during a mirror and during a debug session;
   - lock and unlock;
   - reboot: record whether the DDI survived (DC-28), and remount if it is missing;
   - first-time pairing on a fresh device;
   - Developer Mode off;
   - airplane mode with Wi-Fi off.
7. **Matrix.**
   - iOS: 27.x (this phone), 26.x, 18.x, 17.x. Borrow devices. displayservice and HID on
     17/18 are unknown.
   - Linux: usbmuxd 1.1.1, netmuxd.
   - Windows 11 x64: AMDS from iTunes, AMDS from the Store app, netmuxd.
   - Later: Windows on ARM.
8. **CI:** `cargo check` and unit tests on `ubuntu-24.04` and `windows-2025` (QA §4.2). HIL only on
   the owner's machines, run on demand by lane A (decisions.md: no always-on self-hosted runner).

---

## 16. Ordered task list

Sizes: S ≤ 1 day, M ≤ 3 days, L ≤ 1 week, XL > 1 week (one developer plus agents).
Each task needs the device unless it is marked *(no device)*.

1. **DEV-01 Crate skeleton and CI** (S, no device). Create `russet-device` and
   `russet-device-proto` with the pinned idevice (§5.1) and `ring`; CI `cargo check` on
   Linux and Windows.
   - Accept: CI green on both; `cargo deny` licence check passes (GPL-compatible set).
2. **DEV-02 Mux and discovery** (M). Depends on DEV-01. `MuxEndpoint` (env and platform
   default), `watch()` with reconnect, `DeviceManager` registry and events, identity read.
   - Accept: `russet device list --json` shows this iPhone with name, iOS and model in
     < 300 ms; restarting usbmuxd re-lists the device within 5 s; a fake-mux
     detach ends leases.
3. **DEV-03 Error model and NSError decoder** (M, no device). Depends on DEV-01. §9 with
   fixtures.
   - Accept: the Locked fixture maps to `device.locked` with its remedy; logs show the
     decoded chain, not raw blobs.
4. **DEV-04 Tunnel with MSS clamp and the actor** (L). Depends on DEV-02 and DEV-03.
   `open_usb_tunnel` (§5.3), the RSD map per generation, `connect_rsd::<T>()`, health and
   rebuild, idle close, leases.
   - Accept: 100 sequential session opens with 0 failures; the 32 MiB tunnel AFC
     benchmark ≥ 20 MiB/s both ways; killing the device-side connection, simulated by
     closing the stream, rebuilds the tunnel on the next call.
5. **DEV-05 Pairing, Developer Mode and readiness** (M). Depends on DEV-04. §6.2;
   `device.readiness` and `device.prepare`, plus the minimal DDI slice of DEV-13 that W4 needs
   (status, and mount from a configured directory).
   - Accept: readiness on the test phone reports paired, Developer Mode yes, DDI mounted.
     On a second, unpaired device (if one is available) the Trust flow reaches `paired`.
6. **DEV-06 IPA writer and install, uninstall, list** (M). Depends on DEV-04. §6.4, with
   progress.
   - Accept: Hello installs from `.app` (packed) and `.ipa`; a symlink inside the bundle
     survives installation (checked by a test app that `dlopen`s a framework through the
     link); install-failure fixtures are mapped.
   - Accept: a 100 MB IPA install time is recorded.
7. **DEV-07 Launch, process control and console** (M). Depends on DEV-04. §6.5, without
   `monitorprocesstermination`.
   - Accept: suspended launch returns a pid; SIGCONT resumes it; a console line written by
     the app arrives; kill gives `app.exited` within 2 s; a locked phone gives
     `device.locked`.
8. **DEV-07b `monitorprocesstermination`** (M). Depends on DEV-07. Native CoreDevice
   feature (and upstream U6).
   - Accept: the exit status and signal are reported for `exit(3)` and SIGABRT.
9. **DEV-08 Forwarder and `debug.start`** (M). Depends on DEV-07. §6.6, Unix and TCP
   binds, accept-once on Windows.
   - Accept: the M1 demo script rewritten to use `russet debug` stops at the Swift
     breakpoint with no pymobiledevice3 on PATH.
10. **DEV-09 Logs** (M). Depends on DEV-04 and DEV-07. §6.7, a ring buffer, `logs.tail`.
    - Accept: `russet logs --app $BUNDLE` shows launch-time entries from the first
      instruction (the suspend–subscribe–SIGCONT flow); latency < 500 ms.
11. **DEV-10 Screenshots** (S). Depends on DEV-04. §6.8.1.
    - Accept: a PNG in < 400 ms; DVT fallback when CoreDevice fails.
12. **DEV-11 DisplaySession and mirror** (L). Depends on DEV-04. §6.8.2: one session per
    device, depacketizer, PLI on join, WebSocket, `russet mirror --stdout`.
    - Accept: `russet mirror --stdout | ffplay -f hevc -` plays live; two viewers share
      one session; a join gets a keyframe within 1 s; median ≥ 25 fps during a swipe
      loop.
13. **DEV-12 HID input** (M). Depends on DEV-11. §6.8.3.
    - Accept: tapping the app icon launches it 20/20; swipe, home and text work; input
      while mirroring does not interrupt the mirror.
14. **DEV-13 DDI** (L). Depends on DEV-05. §6.3: status, Cryptex1 install from a Restore dir with
    identity-resolved payload paths, the personalized DMG for iOS 17–26.x, the sources and their
    consent, `device.ddi.list`, the persistence fix. W4 lands the status and `dir` slice; W10 the
    rest, with `imported` as the default.
    - Accept: `device.prepare` on the mounted phone is a no-op in < 1 s. After an owner-approved
      reboot (DC-28), `device.prepare` mounts from the configured Apple-origin directory, and later
      (DC-29) from the W16b-imported 27A266a DDI, with idevice alone (cryptexd plus TSS) and no
      pymobiledevice3 on PATH; `ddi.status` reports the version and the source. If the DDI survived
      the reboot, that is recorded, and no remount is forced without the owner's approval.
    - OQ-3 is closed: the DDI lives only inside `XcodeSystemResources.pkg` (ARCHITECTURE §16.3).
15. **DEV-14 Files** (M). Depends on DEV-04. §6.9.
    - Accept: list, pull, push and remove in the app container; pushing a 50 MB file is
      ≥ 20 MiB/s.
16. **DEV-15 Crash reports** (M). Depends on DEV-07. §6.10, the parser, auto-capture.
    - Accept: SIGABRT on Hello gives `app.crashed` with the exception and the faulting
      thread's top frames within 10 s.
17. **DEV-16 Perf streams** (M). Depends on DEV-04. §6.11 sysmon, then energy, network
    and graphics (each checked on iOS 27).
    - Accept: `russet perf --pid <Hello>` prints 2 samples/s with named fields; a CPU spike
      from a test app shows up.
18. **DEV-17 Symbols through pymobiledevice3** (M). Depends on DEV-18. §6.12, the disk
    guard.
    - Accept: on a machine with ≥ 10 GB free, the DSC lands in the cache with the expected
      layout and the measured size is recorded; with low disk the command fails before
      downloading.
19. **DEV-17b Native `remoteFetchSymbols`** (L). Depends on DEV-17.
    - Accept: identical files to the pymobiledevice3 download (hashes).
20. **DEV-18 pymobiledevice3 fallback runner** (S). Depends on DEV-03. §12.
    - Accept: version detection; `ui.elements` returns elements; a missing
      pymobiledevice3 gives `fallback.unavailable` with an install hint.
21. **DEV-19 JSON-RPC handlers** (L). Depends on DEV-02 through DEV-16 incrementally.
    §7, lease plumbing, notifications.
    - Accept: the IDE extension's device list, Run, Debug, Logs and Mirror buttons work
      against russetd; a connection drop releases all leases within 2 s.
22. **DEV-20 MCP device tools** (M). Depends on DEV-19. §8.
    - Accept: Claude Code drives install, launch, screenshot, tap, logs and crash through
      MCP on the test phone; destructive tools stay hidden unless enabled.
23. **DEV-20b Native AX tree with frames** (L). Depends on DEV-18.
    - Accept: element labels with frames; `ios_tap_element(label)` works.
24. **DEV-21 Wi-Fi** (XL). Depends on DEV-04 and DEV-05. §11, on a home LAN.
    - Accept: with the cable unplugged, install, launch and debug work over Wi-Fi; MSS and
      throughput are measured; `doctor` explains network failures.
25. **DEV-22 Windows** (L). Depends on DEV-01 through DEV-12. §10 on the physical Windows
    PC: AMDS detection, pair record fallback, Store-app versus iTunes notes, netmuxd mode.
    - Accept: the M1 device steps (install, launch, debug forward, logs, mirror) pass on
      Windows 11 x64 with iTunes AMDS; the firewall and ACL assumptions are resolved.
26. **DEV-23 Upstream PRs U1–U5, U8–U10** (M). Can run in parallel after DEV-04. W4 drafts U9 and U10
    together with the fork commit it carries.
    - Accept: PRs opened; links recorded in this document.
27. **DEV-24 HIL suite and soak** (M). Depends on the tasks it covers.
    - Accept: §15.4 and §15.5 green on the test phone; the results file is committed.
28. **DEV-25 `russet doctor` device checks** (S). Depends on DEV-02, DEV-05 and DEV-13.
    - Accept: it reports the mux flavor, pairing, Developer Mode, DDI, lock hint,
      pymobiledevice3 version, and Windows service flavor, each with a remedy.

---

## 17. Risks

| ID | Risk | Likelihood / impact | Mitigation |
|---|---|---|---|
| R-D1 | idevice breaks its API on every point release until 0.2.0 (README) | High / Medium | Exact pin; all idevice types stay inside `russet-device`; protocol replay tests catch changes |
| R-D2 | jktcp robustness: a simplified stack; F1 shows edge cases (no peer-MSS handling; retransmits give up after 5) | Medium / High | MSS clamp; HIL throughput and soak; upstream fixes. Plan B: a `smoltcp`-based adapter behind the same `RsdProvider` trait, or an optional kernel TUN backend |
| R-D3 | Apple changes CoreDevice or RemoteXPC each release (26.2 RSD peer limits, 27.2 message-id parity) [dv-34] | High / High | Matrix testing on betas; follow idevice and pymobiledevice3; pymobiledevice3 fallback switch per capability |
| R-D4 | displayservice and HID are reverse-engineered "Device Hub" internals (Mac17,7 / 25F71 identity in the offer). Apple could gate them or change their identity checks | Medium / High (mirror and agent input) | Screenshot fallback; WDA opt-in; track idevice upstream |
| R-D5 | HEVC decode not available in the IDE webview on some Linux setups | Medium / Medium | OQ-1 test; PNG fallback; optional ffmpeg transcode |
| R-D6 | DDI provenance: Apple files, and no source is licensed for Linux or Windows hosts (ARCHITECTURE §19, Q26); TSS needs the network | High / Medium | Import from the user's own Xcode or Mac (the default; Apple-origin, user-obtained); mirror opt-in only; clear errors; the optional IM4M ticket cache for offline remounts |
| R-D7 | Lock state cannot be queried on iOS 27 | Certain / Low | Reactive mapping; HID wake helper |
| R-D8 | One display session per device. Another host tool (Xcode Device Hub, QuickTime) or a second Russet daemon kills our mirror | Medium / Medium | Detect the RTP stall (> 2 s with no packets while active), restart, and tell the user "another app took the screen" |
| R-D9 | Windows: AMDS flavors, pair record ACLs, Bonjour port conflicts | Medium / Medium | DEV-22 on the physical PC; netmuxd mode |
| R-D10 | Security of local forwards | Medium / High | §14 |
| R-D11 | Attach time without a DSC cache (46 s with full symbols) | High / Medium | Symbol cache (DEV-17); `minimal` default; debug design |
| R-D12 | Licence mixing: pymobiledevice3 (GPL) code leaking into MIT upstream PRs; netmuxd LGPL | Low / Medium | Subprocess only; clean-room upstream PRs |

## 18. Open questions

- **OQ-1.** Can Code-OSS/Electron's WebCodecs decode HEVC on the target Linux setups
  (VA-API, RX 7600) and on Windows? Otherwise the mirror needs a transcode or PNG path.
  This needs a 1-day prototype with the `mirror-test.h265` from the spike.
- **OQ-2.** Do displayservice and HID work on iOS 17/18 devices with their DDIs? We expect
  iOS 26+ only. Needs a borrowed device.
- **OQ-3 (closed 2026-10-11).** Xcode 27.0 keeps the iOS DDI only inside
  `Contents/Resources/Packages/XcodeSystemResources.pkg`; `Contents/Resources/CoreDeviceDDIs`
  no longer exists. The `.pkg` installs `/Library/Developer/CoreDevice/CandidateDDIs/iOS_DDI.dmg`,
  which `devicectl manage ddis update` copies to `/Library/Developer/DeveloperDiskImages/iOS_DDI`.
  Linux and Windows users get it by importing their own `Xcode_27*.xip` (ARCHITECTURE §16.3,
  [D 62]); a copy of a Mac's folder also works.
- **OQ-4.** Should Russet run its own muxer later, built on idevice's server-side usbmuxd
  types or on netmuxd? It would avoid usbmuxd 1.1.1's iOS 26 pairing timing bug and unify
  Wi-Fi. The default today is the system usbmuxd.
- **OQ-5.** Does the AMDS from the Store app answer `ReadPairRecord` and `SavePairRecord`
  like the iTunes one?
- **OQ-6.** Multi-client arbitration: if the IDE and an agent both drive input on the
  same device, who wins? Proposal: last writer wins, with an "agent is controlling the
  device" banner in the IDE.
- **OQ-7.** Is the exact host-to-device packet limit 8192 or 8193 bytes? MSS 8133 also
  worked in one 4 MiB run. Keep 8132 (pymobiledevice3 states ≤ 8192 total).

---

## Appendix A: commands and outputs (2026-10-11)

Environment for all commands:

```sh
export RUSTUP_TOOLCHAIN=stable CARGO_TARGET_DIR=~/russet-lab/target \
       CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
T=~/russet-lab/bin                    # idevice-tools, idevice_id, iproxy, russet-device-spike
U=$UDID; B=$BUNDLE
```

### A.1 Build

```sh
git clone --depth 1 https://github.com/jkcoxson/idevice.git ~/russet-lab/idevice   # 3854a5df
cd ~/russet-lab/idevice && cargo build --release -p idevice-tools -j 10            # 55 s
```

### A.2 Discovery, lockdown, tunnel, DDI

```sh
$T/idevice_id                                   # UsbmuxdDevice { Usb, udid, device_id: 1 }
$T/idevice-tools --udid $U ideviceinfo          # 0.097 s; ProductVersion 27.0, BuildVersion 24A437
$T/idevice-tools --udid $U lockdown --domain com.apple.security.mac.amfi get DeveloperModeStatus   # true
$T/idevice-tools --udid $U remotexpc            # 85 services, 0.113 s
RUSSET_UDID=$U $T/russet-device-spike rsd       # [session] usbmux=0.15ms lockdown+CoreDeviceProxy=84ms rsd=16-21ms total=~104ms mtu=16000
$T/idevice-tools --udid $U mounter list         # DiskImageType Personalized, MountPath /System/Developer, ...
$T/idevice-tools --udid $U cryptexd list        # com.apple.MobileAsset.DDI 27.1.5228.8
$T/idevice-tools --udid $U cryptexd install-ddi ~/.local/share/pymobiledevice3/Xcode_iOS_DDI_Cryptex
#   loaded DDI assets: image 15895040 bytes, trustcache 1895, info 430, volume hash 229; nonce domain handle 4
#   com.apple.MobileAsset.DDI 27.1.5228.8 is already installed (pass --force to install anyway)
$T/idevice-tools --udid $U mounter lookup       # panics: Failed to lookup images: NotFound   (iOS 27)
```

### A.3 Install and launch

```sh
(cd ~/orchard-lab/Hello && xtool dev build --sign)          # 5.3 s, reused stored cert
$T/idevice-tools --udid $U ideviceinstaller install ~/orchard-lab/Hello/xtool/Hello.app   # 1.4 s
python3 -I ~/russet-lab/scripts/mkipa.py ~/orchard-lab/Hello/xtool/Hello.app Hello.ipa
$T/idevice-tools --udid $U ideviceinstaller install Hello.ipa                             # 1.2 s
RUSSET_UDID=$U $T/russet-device-spike launch $B --suspended        # coredevice pid=925 in 3.78 s (first after install)
RUSSET_UDID=$U $T/russet-device-spike launch $B --suspended --dvt  # dvt pid=930 in 136 ms
```

### A.4 Debug

```sh
RUSSET_UDID=$U $T/russet-device-spike run-debug $B 41251 &   # PID=932, READY; forward + launch on one tunnel
# lldb -b -s attach.lldb with: platform select remote-ios; target create <local Hello>;
#   SetPlatformFileSpec(<remote>); SetAsync(True); process connect connect://127.0.0.1:41251;
#   SetAsync(False); breakpoint set --file ContentView.swift --line 6; process attach --pid 932; process continue
# -> stop reason = breakpoint 1.1 ... closure #1 () in ContentView.body.getter at ContentView.swift:6:31
RUSSET_UDID=$U $T/russet-device-spike forward-unix $XDG_RUNTIME_DIR/russet-dbg-spike.sock &
# same script with: process connect unix-connect:///run/user/1000/russet-dbg-spike.sock  -> breakpoint hit
RUSSET_UDID=$U $T/russet-device-spike gdb-probe [--service com.apple.coredevice.debugserverproxy]
# qHostInfo -> cputype:16777228;cpusubtype:2;addressing_bits:43;ostype:ios;...;os_version:27.0.0;...;vm-page-size:16384;
# qSupported -> ...;SupportedCompressions=lzfse,zlib-deflate,lz4,lzma;...;MultiMemRead+;jMultiBreakpoint+;
```

### A.5 Logs, screen, input

```sh
timeout 6 $T/idevice-tools --udid $U syslog_relay      # 1102 lines
timeout 6 $T/idevice-tools --udid $U os_trace_relay    # 4290 entries
RUSSET_UDID=$U $T/russet-device-spike applogs $B 4     # pid 972: 1553 entries for pid, 0 for other pids
RUSSET_UDID=$U $T/russet-device-spike shots 10         # dvt 7.51 fps; coredevice Png 6.17 fps; Jpeg -> NSCocoaErrorDomain 4864
timeout 8 $T/idevice-tools --udid $U screencapture out.h265   # HEVC Main 1184x2576, 154 frames (static screen)
RUSSET_UDID=$U $T/russet-device-spike mirror-test 10   # frames/s per second: [32, 25, 52, 56, 38, 42, 50, 41, 32, 33]
$T/idevice-tools --udid $U hid list | button home | drag 55000 40000 8000 40000   # work
$T/idevice-tools --udid $U hid tap 40480 15318          # reports "tapped" but nothing happens
RUSSET_UDID=$U $T/russet-device-spike tap 40480 15318 120 600   # launches the app under the finger
```

### A.6 Files, crash, perf, throughput

```sh
$T/idevice-tools --udid $U afc --container $B upload|download|remove ...   # 1 MiB round trip OK, ~1.2 s per call
$T/idevice-tools --udid $U crash_logs flush; ... list /; ... pull /Hello-2026-10-11-002726.ips out.ips
$T/idevice-tools --udid $U app_service signal <pid> 6    # produced the Hello .ips above
timeout 5 $T/idevice-tools --udid $U sysmontap 500       # 74 process attrs, 37 system attrs, 9 samples
$T/idevice-tools --udid $U device_info processes         # 587 processes
RUSSET_UDID=$U $T/russet-device-spike afc-bench 32       # usbmux: up 31.9 MiB/s, down 36.7 MiB/s; tunnel: STALL
RUSSET_UDID=$U RUSSET_MSS=8132 $T/russet-device-spike afc-bench 32 --tunnel-only  # up 30.0, down 23.7 MiB/s
```

### A.7 The "Locked" error, decoded

```text
com.apple.dt.CoreDeviceError 10002 "The application failed to launch."
  └ FBSOpenApplicationServiceErrorDomain 1 (BSErrorCodeDescription RequestDenied)
      "The request was denied by service delegate (SBMainWorkspace) for reason: Locked
       ("Unable to launch <bundle> because the device was not, or could not be, unlocked")."
      └ FBSOpenApplicationErrorDomain 7 (BSErrorCodeDescription Locked)
```

## Appendix B: spike code pointers

- `~/russet-lab/spike/src/main.rs`:
  - `open_session` (tunnel and RSD with optional MSS override and pcap);
  - `spawn_forwarder` (TCP) and `forward-unix`;
  - `launch_coredevice`, `launch_dvt`, `run-debug`;
  - `mirror-test` (shared session: RTP receiver plus indigo and universal HID);
  - `tap` and `home`;
  - `afc-bench`, `applogs`, `listen`.
- `~/russet-lab/spike/src/coredevice_stream.rs`: a copy of idevice
  `tools/src/coredevice_stream.rs` (MIT, Jackson Coxson).
- `~/russet-lab/scripts/`:
  - `mkipa.py` (IPA from `.app`);
  - `pcap_syn.py` (reads MSS and wscale from the jktcp pcap);
  - `gdbping.py` (qHostInfo through a forward).

## Appendix C: RSD services on iOS 27.0 (24A437, iPhone13,2)

85 services; the `com.apple.` prefix is dropped:

GPUTools.MobileService.shim.remote, PurpleReverseProxy.Conn.shim.remote,
PurpleReverseProxy.Ctrl.shim.remote, RestoreRemoteServices.restoreserviced,
accessibility.axAuditDaemon.remoteAXService, accessibility.axAuditDaemon.remoteserver.shim.remote,
afc.shim.remote, amfi.lockdown.shim.remote, atc.shim.remote, atc2.shim.remote,
backgroundassets.lockdownservice.shim.remote, bluetooth.BTPacketLogger.shim.remote,
carkit.remote-iap.service, carkit.service.shim.remote,
commcenter.mobile-helper-cbupdateservice.shim.remote, companion_proxy.shim.remote,
corecaptured.remoteservice, coredevice.appservice, coredevice.configuration,
coredevice.debugserverproxy, coredevice.devicecontrol, coredevice.deviceinfo,
coredevice.diagnosticsservice, coredevice.displayservice, coredevice.fileservice.control,
coredevice.fileservice.data, coredevice.hid.indigo, coredevice.hid.universalhid,
coredevice.hid.universalhidservice, coredevice.iconservice, coredevice.locationservice,
coredevice.openstdiosocket, coredevice.pasteboardservice, coredevice.screencaptureservice,
crashreportcopymobile.shim.remote, crashreportmover.shim.remote, dt.remoteFetchSymbols,
dt.remotepairingdeviced.lockdown.shim.remote, dt.testmanagerd.remote,
dt.testmanagerd.remote.automation, fusion.remote.service, gputools.remote.agent,
idamd.shim.remote, instruments.dtservicehub, internal.devicecompute.CoreDeviceProxy,
internal.devicecompute.CoreDeviceProxy.shim.remote,
internal.dt.coredevice.untrusted.tunnelservice, internal.dt.remote.debugproxy,
internal.ptdremoted, iosdiagnostics.relay.shim.remote, misagent.shim.remote,
mobile.MCInstall.shim.remote, mobile.assertion_agent.shim.remote,
mobile.diagnostics_relay.shim.remote, mobile.file_relay.shim.remote,
mobile.heartbeat.shim.remote, mobile.house_arrest.shim.remote,
mobile.insecure_notification_proxy.remote, mobile.insecure_notification_proxy.shim.remote,
mobile.installation_proxy.shim.remote, mobile.lockdown.remote.trusted,
mobile.lockdown.remote.untrusted, mobile.mobile_image_mounter.shim.remote,
mobile.notification_proxy.remote, mobile.notification_proxy.shim.remote,
mobile.storage_mounter_proxy.bridge, mobileactivationd.shim.remote, mobilebackup2.shim.remote,
mobilesync.shim.remote, modelmanager.remote, os_trace_relay.shim.remote,
osanalytics.logTransfer, pcapd.shim.remote, preboardservice.shim.remote,
preboardservice_v2.shim.remote, private.siriappintentsd.orchestrator,
remote.installcoordination_proxy, security.cryptexd.remote, springboardservices.shim.remote,
streaming_zip_conduit.shim.remote, sysdiagnose.remote, sysdiagnose.remote.trusted,
syslog_relay.shim.remote, timed.remote, webinspector.shim.remote

Features advertised on key services:
- `coredevice.appservice`: installroot, launchapplication, listapps, listprocesses,
  listroots, monitorprocesstermination, rebootdevice, sendmemorywarningtoprocess,
  sendsignaltoprocess, spawnexecutable, streamapplist, streamprocesslist, uninstallapp,
  uninstallroot, installapp.
- `coredevice.displayservice`: getmediasupportinfo, getmediastreamserverstatus,
  startaudiooutput, startvideooutput, startmediastream, stopmediastream.
- `coredevice.deviceinfo`: getauthlistingidentifiers, getdeviceinfo, getdisplayinfo,
  getlockstate (not implemented), querymobilegestalt, monitormotion, displayinfoupdates.
- `coredevice.hid.indigo`: button, scroll, digitizer, vendordefined.
- `coredevice.hid.universalhid`: keyboard, universalhid.
- `security.cryptexd.remote`: CryptexInstall, Cryptex1, ReadIdentifiers,
  `Cryptex1,UseProductClass`, InstallEvents.
- `dt.remoteFetchSymbols`: dyldSharedCacheFiles.
- `coredevice.configuration`: customizeuistyle, customizeappearancesettings, voiceover,
  customizeliquidglass, configurationprofiles, renamedevice.

## Appendix D: sources

- idevice: https://github.com/jkcoxson/idevice (MIT; @3854a5df; crates.io 0.1.68). Files
  cited inline.
- jktcp 0.1.7: https://github.com/jkcoxson/jktcp (MIT): `src/adapter.rs` (MSS, RTO,
  retries, window).
- pymobiledevice3 11.26.0 (GPL-3.0), installed copy:
  `remote/userspace_tunnel.py` (MSS note), `osu/win_util.py` (AMDS flavors, pair record
  path), `pair_records.py` (lookup order), `usbmux.py` (`ITUNES_HOST = 127.0.0.1:27015`).
- netmuxd: https://github.com/jkcoxson/netmuxd (LGPL-2.1; v0.4.3, 2026-07-14): README
  sections "USB support" and "Windows".
- quicktime_video_hack: https://github.com/danielpaulus/quicktime_video_hack (MIT): README;
  `doc/technical_documentation.md` §1.
- CrossCode: https://github.com/nab138/CrossCode (MIT): `src-tauri/src/sideloader/device.rs`.
- Research IDs ([dv-xx], [xr-xx]): [landscape-2026-10.md](../research/landscape-2026-10.md).
