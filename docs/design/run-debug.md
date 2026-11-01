# Run & debug: design

> **Implementation notes (W7, 2026-10-11):** where the Wave-1 code differs from the text below.
> - `process connect -p gdb-remote <url>`: the attach plan names the plugin. On a host target LLDB 21.0.0
>   otherwise picks another process plugin and the connect fails without a message (§9.1).
> - Batch scripts run through `lldb -x -b -o 'command source -e false <script>'`: LLDB's batch mode stops at the
>   first failing command, so a bad `--command` would otherwise skip the final `process detach`/`process kill`.
>   The first stop is read through `thread-format`/`frame-format` probe lines (`RUSSET-STOP-…`,
>   `RUSSET-FRAME-…`) for the `--json` result of ARCHITECTURE §8.7.
> - `attachMode: legacy-async` is the M1 sequence over a plain forward and needs LLDB's script interpreter: three
>   `script` lines switch to asynchronous mode around `process connect` (LLDB's command interpreter has no other
>   way, §1 V3) and `SetPlatformFileSpec` names the device executable. Every other generated plan is Python-free.
> - The broker answers LLDB's own `QSetDetachOnError` locally (`OK`), so the broker's value (0 for Russet-launched
>   apps) stays in force (§4.6, §6.2).
> - `run.stop {mode: detach}` leaves the app running and ends the session: state `exited`, no `exit` details.
>   LLDB's `D` (`russet debug --detach`) keeps the session `running` with `debugger: detached` (§4.6).
>   Interactive `russet debug` without `--detach` stops the owned session when LLDB quits, because LLDB detaches
>   on quit after a `process connect` to a stopped process.
> - A stop (`run.stop`, or an owned session's owner leaving) while the session is `launching` ends it at once; the
>   launch then kills the app it started, and `run.start`/`debug.prepare` fail with `core.cancelled`.
> - A debugger that leaves without `D` or `k`: before the `detach` policy or a `keep` handover the broker removes
>   the breakpoints and watchpoints the debugger set (`z0`–`z4`), so the app cannot trap on them later (§4.6).
> - `run.output.query {sinceMs}` is Unix epoch milliseconds; the filter has `sources`, `minLevel`, `contains`,
>   `subsystem` and `category` (no regular expressions yet). An os_trace entry is dropped as a duplicate only when
>   it arrives within 50 ms after the same stdio line (§12). Followers page by arrival instead
>   (`{afterSeq}` → `nextSeq`, as `job.log` does), because an os_trace entry carries the device's timestamp and
>   can arrive after a later stdout line; a limited page resumes right after its last entry.
> - Additions to the catalog's shapes, pending a contract PR: `run.exited` carries `error` (code, `russetCode`,
>   message, hint) when Russet knows why a session `failed` or was `lost` (for example `debug.handoffTimeout`);
>   the same error is also written into the session's output as a `russet` entry, so `russet run` and
>   `russet debug` exit with its code. `debug.batchTimeout` (6017) reports a `--batch` that did not stop and finish
>   its commands within `--timeout`. `russet run --background` reads `run.start`'s job answer (`{jobId}`).
> - Ctrl-C in `russet run`/`russet debug --batch` stops the session and exits 130 (`core.cancelled`); batch LLDB
>   runs in its own process group so the interrupt reaches only Russet.
> - The prototypes are ported into `crates/russet-testkit/src/debug` (fake debugserver, synthetic Mach-O,
>   lldb-server and LLDB discovery, `jitdemo.c`, `probe.c`, DAP client) and deleted. On the host, `probe.c
>   --suspended` waits until a tracer attaches (a self-raised `SIGSTOP` would be re-reported to LLDB after the first
>   continue), and `jitdemo.c` aligns its regions to the 16 KiB iOS page, as Dart does.

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates). Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day) corrected the JIT test plan: the rig iPhone 12 runs TXM on iOS 27.0, so DC-30 and the host test H8 prove page coverage on it (ARCHITECTURE §17.3, [D 71]); see §4.4, §7.1, §7.7, §23. Review round 2 added `russet debug --detach` and the session wire states (ARCHITECTURE §7.6, §9.5, [D 84]); see §4.6 and §17.2.

- **Area:** run and debug (one-command run/debug, IDE debugging, JIT servicing, logs, crashes, symbols)
- **Date:** 2026-10-11
- **Status:** design, ready for implementation tasks (section 24)
- **Binding inputs:** [decisions.md](../decisions.md), [verified-facts.md](../verified-facts.md),
  [demo/m1-baseline.sh](../../demo/m1-baseline.sh), [research/landscape-2026-10.md](../research/landscape-2026-10.md)
  sections 1.2–1.4, 2.9, 2.13, 3.5, 4.2–4.3, 5, 9.

Evidence tags used throughout:

| Tag | Meaning |
|---|---|
| **[V]** | Verified hands-on during this design pass (commands in section 1). No iPhone was touched. |
| **[VF]** | Verified on hardware earlier, recorded in `docs/verified-facts.md`. |
| **[S]** | Read in upstream source code (URL given). |
| **[R]** | Research claim from the landscape report (its record IDs, e.g. [fx-21]). |
| **[A]** | Assumption. Each one has a verification task (section 23.4). |

---

## 0. Summary

Russet runs and debugs iOS apps from Linux and Windows with one command (`russet run`, `russet debug`)
or one button in the IDE. The design rests on four ideas:

1. **Launch suspended, then attach through a Russet-owned "debug broker".** On iOS 17+ debugserver
   cannot spawn apps [VF]. Russet launches the app suspended through CoreDevice (DVT as a fallback),
   opens the device's `debugproxy` service itself, and runs `vAttach` itself. Only then does it give
   the already-attached gdb-remote session to LLDB through a local socket.
   - This removes the "async connect" gotcha. A synchronous `process connect` returns at once when the
     stub already owns a stopped process [V].
   - The attach sequence needs no LLDB Python at all [V]. It works the same from `lldb`, `lldb-dap`
     and the IDE.
2. **A native Rust gdb-remote client services the iOS 26/27 JIT protocol** (`NOTIFY_DEBUGGER_ABOUT_RX_PAGES`)
   for Flutter debug builds. LLDB and Python are not needed.
   - The servicing loop was prototyped against `lldb-server` [V]: every page was touched, the return was
     emulated instead of stepping, and writes were pipelined.
   - The same broker can stay between LLDB and the device for mixed Dart + native debugging. It hides
     its own JIT stops from LLDB.
3. **The swift.org LLDB is the one debugger on Linux** (Swift-aware; also does C/ObjC/C++).
   - Its hard `libpython3.12.so.1.0` dependency is met with a python-build-standalone CPython
     downloaded at runtime. That works today [V].
   - For Swift `po`, LLDB must see the SDK's Xcode toolchain resource directory, not the Linux one [V].
     Russet sets this up through a zero-copy overlay (reflink).
   - On Windows, LLDB delay-loads Python, and Russet's command sequence never needs Python.
4. **The IDE talks DAP to a hub in `russetd`** that fronts `lldb-dap` (and Flutter's DAP).
   - Humans (IDE) and AI agents (MCP tools) share one live debug session.
   - Russet-specific requests (hot reload, relaunch, JIT statistics) are added as DAP custom requests.

Not in scope here:
- device-plane internals (tunnels, pairing, DDI): the device-plane design;
- building, signing and installing: the build and sign designs;
- the MCP server framework: the agents design.

This document lists the interfaces it consumes from those areas (section 17.3).

---

## 1. What was verified during this design pass

All commands ran on the CachyOS host with the Swift 6.4 toolchain (`/usr/lib/swift`). No device command
was run; the iPhone belongs to the device agent.

The throw-away prototypes behind V4, V5, V6 and V8 were kept in `run-debug-prototypes/`; RD-02 ported them
into `crates/russet-testkit/src/debug` and the H1–H3 and H6 tests of `crates/russet-debug/tests`, and deleted
the folder:
- `preattach_relay.py`
- `dap_attach_test.py`
- `fake_ios_stub.py`
- `rx_servicer_proto.py`
- `jitdemo.c`

They were Python/C evidence, not product code.

| # | Finding | How |
|---|---|---|
| V1 | Swift 6.4's LLDB (`lldb 21.0.0`, swiftlang 903b9fa) runs with **python-build-standalone CPython 3.12.15's** `libpython3.12.so.1.0`. **Correction to verified-facts.md:** uv's CPython *executable* is static, but the distribution ships `lib/libpython3.12.so.1.0` (32 MB), and LLDB accepts it. | `LD_LIBRARY_PATH=$PBS/lib PYTHONHOME=$PBS /usr/lib/swift/usr/bin/lldb -b -o 'script import sys,lldb; print(sys.version)'` printed `3.12.15` and an lldb version. `$PBS=~/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu`. |
| V2 | `lldb-dap` from Swift 6.4 cannot even start without that library: `error while loading shared libraries: libpython3.12.so.1.0`. It is a hard `DT_NEEDED` of `liblldb.so.21.0.0`. `lldb-server` has no Python dependency. | `ldd`, then running `lldb-dap --help` with and without `LD_LIBRARY_PATH` |
| V3 | **Root cause of the async-connect gotcha, reproduced without a device.** A synchronous `process connect` to a gdb-remote stub *without* a process hangs: killed by `timeout 15`, rc=124. To a stub that already owns a stopped process, it returns at once. In source: `Platform::DoConnectProcess` calls `WaitForProcessToStop(std::nullopt…)` in sync mode, and `lldb-dap`'s attach handler wraps `attachCommands` in `ScopeSyncMode` and then calls `WaitForProcessToStop(timeout)` [S]. | `lldb-server gdbserver 127.0.0.1:P` (no program) vs `lldb-server gdbserver 127.0.0.1:P -- /usr/bin/sleep 60`, then `lldb -b -o 'process connect connect://127.0.0.1:P'` |
| V4 | **Pre-attach broker pattern works.** A 90-line prototype broker opened the stub connection in ack mode, made the stub own a stopped process (`vRun` here; `vAttach` on iOS), then relayed bytes. Results:<br>• `lldb -b` sync connect: stopped process in 257 ms.<br>• **`lldb-dap` 21.0.0 with `attachCommands: ["process connect connect://127.0.0.1:P"]`:** `attach success=True` at about 260 ms. The process resumed after `configurationDone` (stopOnEntry=false).<br>• The same over a Unix socket (`process connect unix-connect:///path`). | `preattach_relay.py` + `dap_attach_test.py` (DAP over stdio) |
| V5 | **UUID match is enough to bind the local executable.** Against a fake iOS debugserver: LLDB with `platform select remote-ios` matched `target create <local exe>` to the image reported by `jGetLoadedDynamicLibrariesInfos` under a *device* path.<br>• It needed neither `SetPlatformFileSpec` nor search paths, even when the local file had a different name (`Hello-App` vs `Hello`). Device confirmation: task D4.<br>• `target create --remote-file` **fails** on the unconnected `remote-ios` platform ("unable to open target file").<br>• LLDB probes `jGetLoadedDynamicLibrariesInfos:` and `jGetSharedCacheInfo:` with an empty argument and expects `OK`. | `fake_ios_stub.py` serving `Hello-App` (UUID 4C4C44A9-…-60BFA66E4425) as a stopped arm64 iOS process |
| V6 | **Swift `po` off-Mac: the first blocker found, and its fix.**<br>• With the default Linux resource dir, `expr` fails to import the user module. A Swift Build explicit PCM refers to `/usr/lib/swift/usr/lib/swift/CoreFoundation/Modules/module.modulemap` (the swift-corelibs `CoreFoundation` in the Linux resource dir conflicts).<br>• With LLDB's Swift resource dir pointed at the SDK's `XcodeDefault.xctoolchain/usr/lib/swift`, the `lldb expr` log shows `Imported module Hello`, `Swift`, `SwiftUI`, `_Concurrency`, `_StringProcessing` and `SwiftOnoneSupport` (serialized ASTs from the xtool SDK). Expression compilation started.<br>• It then needed `libswift_StringProcessing.dylib`/`libswift_Concurrency.dylib` loaded in the process. They are always loaded in a real app, but absent from the fake process.<br>• `frame variable` (TypeSystemSwiftTypeRef) worked: `(Hello.ContentView) self = {}`. | Fake stub, `settings set target.sdk-path …/iPhoneOS27.0.sdk`, `log enable lldb expr`, an overlay prefix with a copy of `liblldb` |
| V7 | `platform select remote-ios --sysroot <dir>` is accepted on Linux. In LLDB source, non-Apple hosts search **only** `--sysroot` for device support. `~/Library/Developer/Xcode/iOS DeviceSupport` and `PLATFORM_SDK_DIRECTORY` are consulted only when an Xcode developer dir exists [S]. | `PlatformDarwinDevice.cpp` `UpdateSDKDirectoryInfosIfNeeded` / `GetSharedModuleWithLocalCache` |
| V8 | **The native RX-page servicer loop works (host-only analogue).** A C program calls `NOTIFY_DEBUGGER_ABOUT_RX_PAGES(base,size)` on fresh 16 KiB-page mappings and checks for `IHELPED!` and that every page was touched. A Python client against `lldb-server` did this at each stop:<br>1. read the argument registers;<br>2. pipeline the touch writes in one TCP write;<br>3. emulate `ret` instead of stepping over the trap;<br>4. continue.<br>Result: 6 hits, 111 pages, all touched, `RESULT PASS`, exit `W00`, about 0.19 ms per stop on loopback. Packets per stop = 1 + ⌈(n−1)/2⌉ + register writes. | `jitdemo.c`, `rx_servicer_proto.py`. On x86_64 the return emulation is `pc=[sp], sp+=8`; on arm64 it is `pc=lr`. |
| V9 | debugserver (llvm `main`) supports all of these:<br>• `jGetLoadedDynamicLibrariesInfos`, `jGetDyldProcessState`, `jGetSharedCacheInfo`;<br>• `QSetIgnoredExceptions:`, `QEnableCompression:`, `_M`/`_m`, `x`/`X`;<br>• `qThreadStopInfo`, `jThreadsInfo`, `QListThreadsInStopReply`, `QSaveRegisterState`.<br>Stop replies expedite **all general-purpose registers** plus exception-state registers and frame-0 stack memory. At the dyld notifier they add `added-binaries:` [S]. | `lldb/tools/debugserver/source/RNBRemote.cpp` (fetched 2026-10-11) |
| V10 | dyld can report the images of a **suspended, not yet started** process. `dyld_process_info_base::makeSuspended` scans r-x regions for `MH_EXECUTE` and `MH_DYLINKER` headers [S]. | `apple-oss-distributions/dyld` `libdyld/dyld_process_info.cpp` |
| V11 | `cp --reflink=always` of the 232 MB `liblldb.so.21.0.0` from `/usr/lib/swift` into `$HOME` succeeds on this machine: both are btrfs subvolumes of `/dev/nvme0n1p2`, so the copy costs no extra disk. `/tmp` is tmpfs. | `findmnt`, `cp --reflink=always` (copy deleted at once) |
| V12 | Mac oracle, read-only:<br>• Xcode 27's LLDB is `lldb-2103.0.34.103` (Swift 6.4), and `lldb-dap` ships in Xcode.<br>• There is no `iOS DeviceSupport` on the Mac (the phone was never attached to it).<br>• `/Library/Developer/DeveloperDiskImages/iOS_DDI` holds two 16 MB cryptex DMGs (`version.plist` ProductBuildVersion 27A266a). They were not mounted. | `ssh mac 'xcrun lldb --version; xcrun -f lldb-dap; ls …'` |
| V13 | The build Russet debugs:<br>• The DWARF of the M1 `Hello` objects has `DW_AT_LLVM_sysroot` = the xtool `iPhoneOS27.0.sdk` path and `DW_AT_APPLE_sdk` = `iPhoneOS27.0.sdk`.<br>• The executable has **no `__swift_ast` section**; debug info is a debug map to `.o` files.<br>• The signed `Hello.app/Hello` and the unsigned `Hello-App` share `LC_UUID`.<br>• `swift-sdk.json` sets `swiftResourcesPath` to `Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift`. | `llvm-dwarfdump`, `llvm-objdump --macho`, reading `~/.swiftpm/swift-sdks/darwin.artifactbundle` |
| V14 | python-build-standalone release `20261009` provides `cpython-3.12.15+20261009-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz` (34.3 MB) and `…-x86_64-pc-windows-msvc-install_only_stripped.tar.gz` (22.0 MB). Repository licence: MPL-2.0 (build scripts); CPython is PSF. | `gh api repos/astral-sh/python-build-standalone/releases/latest` |

---

## 2. Requirements

| ID | Requirement | Source |
|---|---|---|
| F1 | `russet run` builds, installs, launches and streams logs in one command. The IDE has a Run button. | decisions: Build/Run/Debug/Hot-Reload buttons; live logs |
| F2 | `russet debug` and the IDE Debug button stop at source breakpoints in Swift, ObjC and C code on the phone. | M1/M3 exit criteria |
| F3 | Flutter debug builds run with real JIT on iOS 26/27, with hot reload and hot restart from the IDE. | stacks: Flutter is #2; landscape 4.3 |
| F4 | stdout, stderr and os_log are shown live, filtered to the app, and also available to agents. | decisions: live logs; MCP |
| F5 | Crash reports are captured, symbolicated, and shown in the IDE crash panel and to agents. | decisions: profiling graphs + crash reports |
| F6 | Device system frames are symbolicated without a Mac, through a local symbol cache. | landscape 2.9 gaps |
| F7 | Wi-Fi debugging works with the same flows. | research question 6 |
| F8 | Linux first, then Windows, with no Python required by Russet itself. | decisions: platforms |
| F9 | Headless operation: every IDE action is also a CLI command, a JSON-RPC method and an MCP tool. | decisions: headless must also work |
| N1 | No Apple files are shipped. Symbols come from the user's own device; DDIs and SDKs come from the user's Xcode. | decisions, NOTICE |
| N2 | No root or Admin by default (userspace tunnel). | landscape 3.1 |
| N3 | Disk-aware: never download a device shared cache (several GB) without explicit consent and a free-space check. | environment |
| N4 | Performance targets, over USB, excluding build and install:<br>• launch to stopped-at-breakpoint ≤ 5 s;<br>• Flutter JIT stop ≤ 15 ms;<br>• first hot reload ≤ 1 s. | xlinux baseline about 0.5 s [R fx-22] |

Non-goals for v1:
- the Simulator;
- the view debugger;
- Instruments `.trace` files;
- debugging app extensions (open question 7);
- iOS < 17 devices. Xcode 27 itself debugs only iOS 17+ [R xr-05].

---

## 3. Architecture

```text
┌────────────────────────────── Russet IDE (Code-OSS) ──────────────────────────────┐
│ Russet extension: Run / Debug / Hot Reload buttons, device picker, log & crash     │
│ panels. Debug type "russet" -> DebugAdapterNamedPipeServer(dap hub endpoint)        │
└───────────────┬───────────────────────────────────────────────┬────────────────────┘
                │ JSON-RPC (russetd API)                         │ DAP (per session)
┌───────────────▼───────────────────────────────────────────────▼────────────────────┐
│ russetd (Rust daemon)                                                               │
│  ├─ russet-debug ───────────────────────────────────────────────────────────────┐   │
│  │   session (state machine) ─ launch ─ broker ─ jit ─ lldb ─ dap-hub ─ logs ─   │   │
│  │   crash ─ symbols ─ flutter                                                  │   │
│  └─ uses russet-gdbremote (codec/client/interposer)                             │   │
│  ├─ MCP server (agents area) -> calls the same DebugService API                  │   │
│  └─ russet-device (device-plane area, idevice-based): tunnels, RSD services       │   │
└───────┬───────────────┬────────────────────┬───────────────────────┬──────────────┘
        │ child process │ child process      │ RSD/lockdown streams  │
   lldb-dap / lldb   flutter debug_adapter   │ (userspace tunnel)    │
   (swift.org)       (Flutter SDK)           ▼                       ▼
                                   iPhone: appservice, openstdiosocket, debugproxy,
                                   os_trace_relay, crashreportcopymobile/mover,
                                   remoteFetchSymbols, dvt processcontrol
```

Process model:

- **russetd** owns:
  - device connections and every broker;
  - the DAP hub endpoints;
  - log and crash pipelines;
  - the symbol cache.

  It survives IDE restarts.
- **LLDB never touches the device directly.** It talks gdb-remote to the broker over a local Unix
  socket (Linux) or loopback TCP (Windows). This works with the userspace tunnel, which only russetd can
  reach (pymobiledevice3 cannot hand a userspace-tunnel address to an external LLDB either [R dv-10]).
- **The CLI (`russet`)** is a thin JSON-RPC client of russetd.
  - It auto-starts russetd if it is not running.
  - `russet debug` spawns an interactive `lldb` attached to a broker socket.

---

## 4. Flows

### 4.1 `russet run` (no debugger): Swift, Xcode-project and release builds

```text
russet run [--device D] [--configuration Debug|Release] [-- args...]
 1 device.ensureReady(D)            tunnel up, Developer Mode on, DDI mounted        (device area)
 2 build.run(target)                -> artifacts{app, exe, uuids, dsyms}             (build area)
 3 install.ifChanged(D, app)        compares the per-device install ledger           (install area)
 4 stdio = coredevice.openstdiosocket()   -> 16-byte UUID
 5 pid = appservice.launchapplication{bundleId, args, env, startStopped:false,
                                      terminateExisting:true, standardIOIdentifiers:stdio}
 6 logs: stdio bytes  ⊕  os_trace_relay(pid)  -> debug.output notifications
 7 wait: appservice monitorprocesstermination / stdio EOF / DVT termination notice
 8 if exit by signal -> crash pipeline (section 13)
```

Default environment, unless the user overrides it (section 5.3): `NSUnbufferedIO=YES`, `OS_ACTIVITY_DT_MODE=YES`.

### 4.2 `russet debug` and the IDE Debug button (Swift/ObjC/C)

```text
 1-3  as in run
 4    kill orphans: running instances of the bundle (CoreDevice listprocesses / DVT kill)
 5    stdio = openstdiosocket()
 6    pid = launchapplication{..., startStopped:true}                 keep the appservice connection OPEN
      fallback: DVT processcontrol launchSuspendedProcessWithDevicePath(StartSuspendedKey=1), keep the DVT channel OPEN
 7    gdb = rsd.connect("com.apple.internal.dt.remote.debugproxy")    raw TCP over the tunnel
 8    broker.handshake(gdb): +, QStartNoAckMode(+), qSupported, QThreadSuffixSupported,
                              QListThreadsInStopReply, QEnableErrorStrings, qHostInfo
 9    broker: vAttach;<pid hex>  -> T11 (SIGSTOP)        process is pre-dyld, suspended, owned by the broker
10    close the launch channel (attach done)
11    broker.listen(unix socket | 127.0.0.1:ephemeral), single client
12    LLDB: attachCommands (section 9.1) ... `process connect <broker url>` returns immediately (stopped)
13    IDE sets breakpoints -> configurationDone -> lldb-dap continues -> app runs; breakpoints hit
14    exit: debugserver W/X -> LLDB -> DAP exited/terminated -> session end -> crash check
```

- **Why steps 6–9 are ordered this way.** The process is created suspended before dyld runs. The broker
  attaches while it is still suspended. LLDB sees a stopped process.
  - Even breakpoints in `main`, `App.init` or `+load` hit.
  - No Python is needed, and there is no async-mode toggling [V3, V4].
- **Keep the launch channel open until after `vAttach`.** xlinux reports that closing the DVT session early
  resumes the app [R fx-24 README "Hard-won details"]. The M1 baseline closed it before attaching and
  still stopped at the breakpoint [VF], so the behaviour is unclear (task D2). Keeping the channel open
  costs nothing.

### 4.3 Attach to a running app

`russet debug --attach [--pid N | --bundle B]`, IDE command "Attach to iPhone process…":
- Steps 7–12 of 4.2 without launching.
- The app is stopped from `vAttach` until LLDB's first `continue`, normally under a second.
- The broker sends `QSetDetachOnError:1`, so a lost connection never kills a running app.
- stdout cannot be captured for a process Russet did not launch. Logs come from os_trace only.

### 4.4 Flutter debug (JIT on iOS 26/27): native servicer, no LLDB

```text
flutter run -d russet-<udid>   (or the IDE Flutter Debug, via Flutter's DAP)
 flutter_tools: builds the asset bundle + kernel (platform-neutral), then calls the custom-device hooks:
   install  -> russet flutter device install   (Russet packages the iOS app, signs, installs)
   runDebug -> russet flutter device run-debug --udid D --app ${appName} --mode ${buildMode} --engine-options=${engineOptions}
      1 launch suspended with engine args (Dart flags; add --vm-service-host=::0 on Wi-Fi)
      2 broker: vAttach; JIT arming (section 7.4): run to main, resolve NOTIFY, Z0
      3 broker continues; services every NOTIFY stop natively; passes signals; captures crashes
      4 prints the app's stdout/stderr (contains "The Dart VM service is listening on http://127.0.0.1:P/…")
   forwardPort -> russet flutter device forward --udid D --device-port ${devicePort} --host-port ${hostPort}
 flutter_tools: connects to the VM service, DevFS sync, hot reload/restart, DevTools.
```

- On iOS 17–18 the engine does not dual-map [S `virtual_memory_posix.cc` `IsAtLeastIOS26_0()`]. The
  broker just stays attached, which gives the debugger-attached JIT those versions need [R fx-19].
- If the VM's one-page start-up test region is not serviced, debug engines run interpreted ("Dart
  execution mode: simulator"), and engines without the simulator stop with FATAL "Unable to JIT"
  [S][R fx-18]. Russet watches stdout for these lines and raises `debug.warn.jitInterpreted`.
- If a **later** region is missed or only partly touched on a TXM device, there is no fallback: the app
  crashes with `EXC_BAD_ACCESS`/`SIGBUS` when it executes code from an untouched page (flutter#190307,
  #193058). Russet reports such a crash as `debug.jitPageMissed` (§7.7). The rig's iPhone 12 runs TXM
  on iOS 27.0 (ARCHITECTURE §17.3), so this is tested there (DC-30).

### 4.5 Mixed Flutter + native debugging

A compound IDE session "Flutter + native (LLDB)":
- The broker of 4.4 additionally accepts an LLDB connection in **interposer mode** (section 6.5).
- It keeps servicing JIT stops and hides them from LLDB.
- Dart debugging goes through Flutter's DAP; native debugging goes through `lldb-dap`.

### 4.6 Stop, restart, detach

| Action | Swift session | Flutter session |
|---|---|---|
| IDE Stop / `disconnect{terminateDebuggee:true}` | `k`, the app dies | flutter_tools stops the app; the broker sends `k` |
| IDE Detach / `terminateDebuggee:false` | `D` forwarded, the app keeps running | the broker swallows LLDB's `D`, removes LLDB's breakpoints, keeps servicing JIT. Detaching the broker would freeze the app at its next JIT stop [R xlinux README]. |
| Restart | the session tears down, then 4.2 runs again (incremental build, delta install) | hot restart = flutter_tools `app.restart(fullRestart:true)` (same process); "Relaunch" = new session |
| LLDB crashes or disconnects | policy `onDebuggerLost` (default `kill`, configurable to `detach`) | the broker keeps the app alive (native mode); the IDE can attach again |
| `russet debug --detach` (CLI, interactive or `--batch`; ARCHITECTURE §7.6, [D 84]) | the generated script ends with `process detach`: the broker forwards `D` (the Detach row above, not `onDebuggerLost`), the app keeps running, and the detached session stays `running` | as Detach above |
| russetd crashes | debugserver sees its connection drop. The broker sets the outcome explicitly with `QSetDetachOnError:0` (kill). debugserver "will detach (1) or kill (0) … if it loses connection to lldb" [S RNBRemote.cpp]. Kill is used for Russet-launched sessions, so no half-debugged app is left behind. | `QSetDetachOnError:0` as well. A detached Flutter debug app would stall at its next JIT stop. For attach-to-running sessions (4.3) Russet sends `:1`. Task D11 checks the behaviour. |

### 4.7 Hot Reload button for Swift (v1 semantics)

Real code injection into Swift apps is research (open question 8). In v1 the Hot Reload button on Swift
projects does **Quick Relaunch**:
1. incremental build;
2. delta install;
3. relaunch suspended;
4. the broker attaches again, and the IDE breakpoints are re-applied by the same DAP session (the DAP hub
   swaps the underlying `lldb-dap`, section 9.3).

Target: ≤ 8 s for a one-file change.

---

## 5. Launch subsystem (`russet_debug::launch`)

### 5.1 Strategies

| Strategy | Service (RSD) | Suspended | stdio | Termination notice | Status |
|---|---|---|---|---|---|
| `CoreDevice` (default) | `com.apple.coredevice.appservice`, feature `com.apple.coredevice.feature.launchapplication`; `options.startStopped`, `terminateExisting`, `arguments`, `environmentVariables`, `standardIOUsesPseudoterminals:true`, `standardIOIdentifiers{standardInput/Output/Error: <uuid>}` | yes | `com.apple.coredevice.openstdiosocket`: raw bytes, PTY, `\r\n` line ends; closes at exit | `feature.monitorprocesstermination` | Shape from pymobiledevice3 11.26 `remote/core_device/app_service.py` [S]. Not yet run on our phone (task D1). |
| `Dvt` (fallback) | `com.apple.instruments.server.services.processcontrol`: `launchSuspendedProcessWithDevicePath:bundleIdentifier:environment:arguments:options:` with `{StartSuspendedKey:1, KillExisting:1}` | yes | `outputReceived:fromProcess:atTime:` DTX callbacks | `processWithPID:terminatedWithExitCode:orCrashingSignal:` | **Verified** on iOS 27 [VF]; pymobiledevice3 `services/dvt/instruments/process_control.py` [S] |

The strategy is chosen per device:
1. CoreDevice is the default.
2. On `FeatureNotSupported`, a launch error, or no stdio within 2 s of a non-suspended launch, Russet falls
   back to DVT and remembers the result in the device profile.

### 5.2 Resolving the installed app

- `installation_proxy` browse with `ApplicationType=User`, attributes `CFBundleIdentifier`, `Path`,
  `CFBundleExecutable`, `CFBundleVersion`. The CoreDevice `listapps` equivalent also works.
- Accept the exact bundle id, and also the `XTL-<prefix>.<id>` form that xtool produces [VF]. Paid
  accounts keep exact ids (sign area).
- Result: `InstalledApp { bundle_id, path: "/private/var/containers/Bundle/Application/<UUID>/X.app", executable }`.
  The broker and LLDB use it for image search paths.

### 5.3 Environment and arguments

| Variable / arg | Default | Why |
|---|---|---|
| `NSUnbufferedIO=YES` | on | Xcode sets it. It makes `print`/`NSLog` reach the PTY immediately. |
| `OS_ACTIVITY_DT_MODE=YES` | on (debug and run) | Mirrors `os_log` and `Logger` output of *this process* to stderr. Unified-log redaction does not apply there [A; task D6]. iOS 26 redacts `NSLog` dynamic strings in the unified log [R dv-35]. Opt-out: `russet.run.mirrorOsLogToStderr=false`. |
| user `env` / `args` | from the launch config | merged last |
| Flutter engine args | from flutter_tools `${engineOptions}`, e.g. `--enable-dart-profiling=true --enable-checked-mode=true --verify-entry-points=true --vm-service-port=N` [S `custom_device.dart`] | Russet adds `--vm-service-host=::0` (or `0.0.0.0`) on Wi-Fi, as flutter_tools does for wireless iOS devices [S `device.dart` `getIOSLaunchArguments`] |

### 5.4 Launch errors

These map to the error catalogue (section 22):
- device family:
  - 3002 locked
  - 3004 developerModeDisabled
  - 3005 ddiNotMounted
  - 3006 tunnelFailed
  - 3100 app.notInstalled
  - 3102 app.launchFailed
- `debug` family:
  - 6000 `debug.launchDenied` (code-signature or entitlement rejection)
  - 6001 `debug.profileUntrusted` (free account: trust the developer in Settings)

The device's error string is always kept in `data.deviceMessage`.

---

## 6. The debug broker (`russet_debug::broker`)

### 6.1 Responsibilities

1. Own the single debugserver connection for the session (debugserver serves one client).
2. Pre-attach (`vAttach`) so that any LLDB can do a synchronous `process connect` [V3, V4].
3. Expose one local endpoint for one LLDB client at a time. Later reconnects are allowed: "attach the
   debugger again" without relaunching.
4. Service JIT stops itself, in **native mode** (no LLDB) and in **interposer mode** (LLDB connected).
5. Catch fatal exceptions when no LLDB is connected (crash capture).
6. Apply the detach/kill policy when LLDB leaves.

### 6.2 Upstream (debugserver) side

| Step | Packet | Notes |
|---|---|---|
| 1 | `+` then `QStartNoAckMode` → `OK`, then `+` | debugserver switches to no-ack after the final ack (same handshake xcross uses [S xcross `gdb_remote_client.dart`]) |
| 2 | `qSupported:swbreak+;hwbreak+;vContSupported+` | records `PacketSize`, `SupportedCompressions` |
| 3 | `QThreadSuffixSupported`, `QListThreadsInStopReply`, `QEnableErrorStrings` | for `;thread:` suffixes and readable errors |
| 4 | `qHostInfo` | `vm-page-size:16384`, `os_version`, `ostype:ios` [VF] |
| 5 | `QEnableCompression:type:zlib-deflate;` (optional, phase 2) | debugserver advertises `lzfse,zlib-deflate,lz4,lzma` [VF]. It speeds memory reads over the slow userspace tunnel [R R11]. The broker decompresses, so LLDB never sees compression. |
| 6 | `QSetDetachOnError:0` (Russet-launched) or `:1` (attach to a running app) | the outcome when the broker's connection is lost (4.6) [S] |
| 6b | `vAttach;<pid hex>` | → `T11…` on success, `E..;<msg>` on failure (no `get-task-allow`, already debugged, pid gone) |
| 7 | `jGetDyldProcessState` (optional) | dyld progress; used by JIT arming and diagnostics [S V9] |

### 6.3 Downstream (LLDB) side: endpoint

| Platform | Endpoint | LLDB URL | Protection |
|---|---|---|---|
| Linux | `$XDG_RUNTIME_DIR/russet/sessions/<sid>/gdb.sock` (dir 0700, socket 0600) | `unix-connect://<path>` (verified [V4]) | filesystem permissions |
| Windows | `127.0.0.1:<ephemeral>` | `connect://127.0.0.1:<port>` | loopback only; accept one client; the listener closes after `handoffTimeout` (60 s) or when the session ends |

### 6.4 Relay mode (v1)

After `vAttach` the broker becomes a frame-level relay:
- It reads complete frames on each side: `$…#xx`, ack chars, and the raw `0x03` interrupt.
- It keeps **separate ack state** per side. It answers LLDB's `QStartNoAckMode` itself, because upstream is
  already in no-ack mode.
- It forwards payload bytes unchanged. Binary escapes (`}`) and run-length encoding (`*`) are not decoded
  except where the broker must inspect a payload.

| From LLDB | Broker action |
|---|---|
| `+`, `-` | consumed (downstream ack state) |
| `0x03` | forwarded raw |
| `QStartNoAckMode` | answered `OK` locally; downstream switches to no-ack |
| `qSupported:…` | forwarded; `SupportedCompressions=…` is removed from the reply |
| `vAttach*`, `vRun`, `A…` | answered `E16;<hex "russet broker already attached pid N">` |
| `Z0/Z1/z0/z1` | forwarded; recorded in `lldb_breakpoints` |
| `vCont…`, `c`, `C`, `s`, `S` | forwarded; `resume_pending = Some(action)` |
| `D` | policy (4.6): Swift sessions forward; Flutter sessions get `OK` locally, the broker removes LLDB's breakpoints and returns to native mode |
| `k` | forwarded; the session goes to `Terminating` |
| anything else | forwarded verbatim |

| From debugserver | Broker action |
|---|---|
| reply to an LLDB request | forwarded |
| async stop `T/S` while `resume_pending` | interposer mode: JIT check (6.5). Relay mode, and stops that are not JIT stops: forwarded. |
| `O<hex>` | forwarded (rare: we attach, we do not launch) |
| `W`, `X` | forwarded; session `Exited` |

Native mode (no LLDB connected) runs the client state machine of section 7.5 on the same connection.

### 6.5 Interposer mode (v2): JIT stops hidden from LLDB

When a stop arrives while LLDB has a resume pending:

1. **Is it ours?** It is ours when *all* of these hold; otherwise forward it unchanged:
   - `pc == notify_addr` (expedited register `20`);
   - the stop reason is a breakpoint;
   - the thread is the reporting thread;
   - LLDB has no breakpoint site at `notify_addr`.
2. **Service it** (7.5 steps S1–S3).
3. **Re-resume or report.**
   - If LLDB's pending action was a plain continue (`c`, `vCont;c`, `vCont;c:-1`), send it again.
   - Otherwise LLDB was stepping thread A. Query `qThreadStopInfo<A>`:
     - if A has a stop reason (trace, breakpoint, exception), forward A's stop reply to LLDB with the JIT
       thread's entry removed from `jstopinfo`;
     - else send LLDB's original `vCont` again.
4. **Scrub.** Filter `jThreadsInfo` and `qThreadStopInfo` replies so the serviced thread reports no stop
   reason. The thread's pc was already moved to `lr`, so LLDB sees a normal stopped thread.

If LLDB has its own breakpoint at `notify_addr`, the broker touches the pages but does not emulate the
return, and forwards the stop. LLDB then steps over normally.

### 6.6 Broker states

```text
Connecting -> Handshaken -> Attached(pre-dyld) -> [Arming(JIT)] -> Ready
Ready --lldb connects--> Relaying ⇄ (JitServicing)     Ready --no lldb--> Native
Relaying --lldb D (flutter)--> Native      Relaying --lldb k--> Terminating
Any --W/X--> Exited      Any --upstream EOF--> Lost
```

---

## 7. JIT servicing for iOS 26/27 (`russet_debug::jit`)

### 7.1 Protocol facts

- **Dart VM side** (`runtime/vm/virtual_memory_posix.cc`, `DART_ENABLE_RX_WORKAROUNDS`) [S]:
  1. On iOS ≥ 26 the VM dual-maps code. It maps a region RX, creates an RW alias with `vm_remap`, and
     calls `NOTIFY_DEBUGGER_ABOUT_RX_PAGES(void* base, size_t size)`. This is an exported, `noinline`,
     empty `extern "C"` function.
  2. `CheckIfRXWorks()` runs one test allocation and reads its first 8 bytes. `"IHELPED!"` there is only
     a *diagnostic* ("debugger assist: ok/not detected").
  3. The real test is executing a JIT'd `mul w0,w0,w0; ret`.
  4. If the test fails, the VM uses the simulator (interpreter) and prints `Dart execution mode: simulator`.
- **Kernel side** (mechanism verified from source, 2026-10-11). A debugger re-protecting a range makes
  XNU call `csm_associate_debug_region` for exactly that page-rounded range; on SPTM/TXM kernels that
  is a TXM call (`txm_associate_debug_region`). debugserver's memory-write path does exactly this:
  `mach_vm_protect` RW (retried with `VM_PROT_COPY`), then `mach_vm_write` per page. TXM refuses
  associations not initiated by a debugger and requires an address space that allows invalid code
  (`get-task-allow` plus Developer Mode). The fault path maps unsigned pages of associated entries with
  `PMAP_OPTIONS_XNU_USER_DEBUG`, and unassociated unsigned executable pages fault. So every page needs
  at least one debugger write, and a write that straddles a page boundary covers both pages
  [S xnu-12377.121.6 `osfmk/vm/vm_map.c` `vm_map_entry_cs_associate`, `bsd/kern/code_signing/txm.c`;
  llvm debugserver `MachVMRegion.cpp`, `MachVMMemory.cpp`; the 24A437 TXM strings]
  [R dv-19, dv-20]. On iOS 26 only A15/M2 and later run TXM; from iOS 27 nearly everything does,
  including the rig's A14 (one TXM image for every SoC). Apple's DDI debugserver is assumed to behave
  like LLVM's; DC-30 checks the outcome.
- **Dart picks the RX path by iOS version only.** At Flutter 3.47.7's Dart revision, `VirtualMemory::Init`
  dual-maps from iOS 26.0 with no TXM detection, so the hook is called on TXM and non-TXM devices alike;
  `CheckIfRXWorks` tests a single 16 KiB page.
- **Cost today.** Flutter's LLDB Python helper writes the whole region (up to 512 KiB per stop). xlinux
  writes 8 bytes per 16 KiB page: first hot reload went from 6.8 s to about 0.5 s; each stop costs about
  130–150 ms over USB [R fx-22].
- **iOS 27.** LLDB sometimes misses the stop at an auto-continue scripted breakpoint, so Flutter continues
  by hand [R fx-21] [S `flutter_tools/lib/src/ios/lldb.dart`]. LLDB callbacks also need synchronous mode
  [R xlinux README].

### 7.2 Options compared

| | (a) LLDB + Python helper (Flutter's way) | (b) Native Rust client (default) | (c) Interposer (b) + LLDB |
|---|---|---|---|
| Needs LLDB / Python | yes / yes | no / no | LLDB yes, Python no |
| Works with `lldb-dap` (async) | unreliable: callbacks need sync mode | n/a | yes |
| Cost per stop | about 130–150 ms [R] | ≤ 2 RTT; about 0.2 ms on loopback [V8] | same as (b) |
| iOS 27 manual-continue issue | must handle | not applicable (we resume ourselves) | not applicable |
| Windows | LLDB + Python DLL | yes, pure Rust | yes |
| Native breakpoints in plugins | yes | no | yes |
| Use | fallback `--jit-servicer=lldb-python` (CLI, headless sync LLDB) | Flutter Run/Debug (Dart) | Flutter + native compound sessions |

### 7.3 Choosing the hook

- Look for `_NOTIFY_DEBUGGER_ABOUT_RX_PAGES` (Mach-O symbol with the leading underscore) in the **local
  copy** of `Flutter.framework/Flutter` that was installed. Read it with the `object` crate.
- Example: Flutter 3.47.7 debug engine `T 0x744174` [R fx-19].
- No symbol means the engine is too old or the build is AOT: `jit = None`. On iOS ≥ 26 that gives an
  interpreted debug build plus the warning `debug.warn.jitUnsupportedEngine`.
- Compare the framework's `LC_UUID` with the device image's UUID (from `jGetLoadedDynamicLibrariesInfos`).
  A mismatch means the install is stale: `debug.uuidMismatch` (6005).
- **Any exporting image** (W18): `JitHook` discovery accepts any loaded image that exports
  `_NOTIFY_DEBUGGER_ABOUT_RX_PAGES`, preferring `Flutter.framework`, so the native `RussetJitProbe`
  fixture (a C core that announces its own regions) is serviced the same way.

### 7.4 Arming: setting the breakpoint before the engine starts

The broker attaches **before dyld runs**, so `Flutter.framework` is not mapped yet. Dart VM
initialisation happens after `main`, when the engine starts, so the arming sequence is:

1. Send `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}`.
   - dyld's `makeSuspended` returns the main executable and dyld for a suspended, unstarted task [S V10].
   - Take the `MH_EXECUTE` image's `load_address` and check its UUID against the local Runner.
   - **Fallback:** walk `qMemoryRegionInfo` from `0x100000000`. Pick the first `rx` region whose first 4
     bytes are `cffaedfe` with `filetype == 2` and a matching UUID (dyld's own algorithm).
2. `entry = load_address + LC_MAIN.entryoff`. Send `Z0,<entry>,4`, then `vCont;c`.
3. When the stop has `pc == entry`:
   1. Send `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` again. Dependent dylibs (including
      `Flutter.framework`) are loaded before `main`.
   2. Find `…/Frameworks/Flutter.framework/Flutter` by path suffix and UUID.
   3. `notify_addr = load_address + (n_value - __TEXT.vmaddr)`.
4. `Z0,<notify_addr>,4`, then `z0,<entry>,4`. The original instruction is restored. On arm64 pc already
   points at it, so nothing needs adjusting.
5. Then either stay stopped (handoff to LLDB in mixed mode) or `vCont;c` (native mode).

Arming budget: 3–4 round trips plus two image-list queries, under 300 ms over USB [A, measured in RD-19].

### 7.5 Servicing algorithm (native mode)

State: `notify_addr`, `page = qHostInfo.vm-page-size (16384)`, `stats`.

```text
loop:
  ev = wait_stop()
  match ev:
    W/X                      -> session Exited/Terminated; stop
    T with pc==notify_addr & reason breakpoint:
      S1 base = reg[0x00], size = reg[0x01], lr = reg[0x1e], tid = thread    (all expedited in T [S V9];
                                                                         if missing: p0/p1/p1e;thread:<tid>;)
      S2 plan = touch_plan(base, size, page):
             M base,8:4948454c50454421                        # "IHELPED!" (page 0; Dart diagnostic)
             for k = 1, 3, 5, ... while k < n:
                 if k+1 < n: M (base+(k+1)*page-1),2:0000    # last byte of page k + first of page k+1
                 else:       M (base+(k+1)*page-1),1:00      # odd tail
         n = ceil((base+size - page_floor(base)) / page)
      S3 P20=<lr little-endian 8 bytes>;thread:<tid>;           # emulate `ret` (function body is empty)
      S4 send S2+S3 as ONE write (pipelined); read n_writes+1 replies; any E.. -> error event
         then vCont;c          (eager mode: S2+S3+vCont in one write; see 7.6)
    T with signal (reason:signal, no metype)  -> vCont;C<sig>:<tid>          (pass through, as LLDB does)
    T with metype=EXC_BREAKPOINT at another address (not ours) -> treat as a user trap: report, detach
    T with metype in {EXC_BAD_ACCESS(1), EXC_BAD_INSTRUCTION(2), EXC_ARITHMETIC(3), EXC_GUARD(12), EXC_CRASH(10)}:
         capture crash snapshot (regs, fp-chain walk via m reads, 64 frames max, image list)
         -> D   (detach: the exception goes back to the task's ports, so ReportCrash writes the .ips)
    T with metype=EXC_RESOURCE(11) -> vCont;c
```

Notes:

- **The touch plan writes the same bytes the pages already hold** (fresh anonymous pages are zero),
  except the 8-byte diagnostic on page 0, as Flutter's helper does [S].
  - Invariant: every byte written lies inside `[base, base+size)`, and every page that overlaps the
    region gets at least one written byte.
  - Dart passes page-aligned regions [S], so the plan above applies as written.
  - For unaligned input, straddling writes are used only where both bytes are inside the region; a page
    at either edge gets a single-byte write inside the region instead.
  - Writes per stop = 1 + ⌈(n−1)/2⌉. Example: a 512 KiB region (32 pages) takes 17 `M` packets of
    about 30 bytes, roughly 0.5 KB in one TCP write.
  - Frida uses the same boundary-straddling trick [S frida-core `src/fruity/debugger-mappings.vala`
    `spray_pages`]. Its last-iteration rule seems to skip the final page when the page count is even;
    our plan covers every page, as checked in [V8].
- **Return emulation** removes three round trips per stop (`z0`, single-step, `Z0`). It is valid because
  the hook's body is empty (`asm volatile("" ::: "memory")` [S]) and AArch64 `ret` only does `pc = x30`.
  - RD-10 (with D10) must check this against the shipped engine: decode the first instruction at
    `notify_addr` with the small decoder in `russet-gdbremote::arm64`. It recognises only `ret`,
    `bti`, `paciasp`/`retaa` and `nop`, so no disassembler library is needed.
  - If the prologue is anything else, fall back to step-over: `z0`, `vCont;s:tid`, `Z0`, `vCont;c`.
- **Signals and exceptions.** The Dart VM's own `EXC_BAD_ACCESS` probe in `CheckIfRXWorks` uses a
  *thread* exception port, which is served before the debugger's task port [S comment in
  `virtual_memory_posix.cc`]. Native mode therefore never sees it.
- **Concurrency.** debugserver is all-stop. If several threads hit `notify_addr` in the same stop, the
  broker services each thread listed with a breakpoint reason in `jstopinfo` before resuming.

### 7.6 Pipelining modes

| Mode | Wire pattern per stop | Round trips | Risk |
|---|---|---|---|
| `safe` (default) | [M…, P20] → wait for all OK → `vCont;c` | 2 | none |
| `eager` | [M…, P20, vCont;c] in one write | 1 | a failed `M` is only seen after the thread resumed; logged and counted, and the session falls back to `safe` |

### 7.7 Metrics and diagnostics

- `jit.stats` events (every 2 s while active): `stops`, `pagesNotified`, `pagesTouched` (equal on a
  healthy session), `bytesWritten`, `p50/p95 stop service ms`, `rttEstimateMs`, `mode`. The wire shape is
  ARCHITECTURE §9.5 `JitStats`.
- **`debug.jitPageMissed`** (6016): when crash capture sees `EXC_BAD_ACCESS` whose pc or fault address lies
  in an announced region's page that the servicer did not touch, or in an executable region that was
  never announced, Russet reports a servicer bug with the region and the page, not a Dart crash.
- After the first stop, Russet watches stdout for `Dart execution mode: JIT`. Seeing `simulator` instead
  raises `debug.warn.jitInterpreted`, with the hint "breakpoint not hit before VM init".
- `russet debug jit-bench --device D` launches a Russet JIT test app, a tiny Flutter app shipped as source
  in `tests/fixtures/jit-bench`. It reports per-stop latency over USB and over Wi-Fi.

### 7.8 Generic debugger-assisted JIT (extension point)

`trait JitHook` lets other runtimes plug in:
- StikDebug-style `brk #0xf00d` "JIT syscalls" (x16 = command; prepare region / detach) [S StikDebug
  `universal.js`, AGPL; reference only, re-implemented from its observable protocol];
- future engines (React Native Hermes has no JIT on iOS [R fx-38]).

v1 ships only `FlutterRxPages`.

---

## 8. LLDB toolchains (`russet_debug::lldb`)

### 8.1 Flavors

| Flavor | Used for | Linux source | Windows source |
|---|---|---|---|
| `swift` (default) | everything: Swift, ObjC, C, C++, mixed Flutter | the Swift toolchain Russet already uses to build (Swift 6.4 must match the Xcode 27 SDK [R tc-03]): swiftly, `/usr/lib/swift` (AUR `swift-bin`), `$SWIFT_TOOLCHAIN`. `lldb`, `lldb-dap`, `lldb-server`, `swift-demangle`, `llvm-symbolizer` are all present [V]. | the Swift for Windows toolchain (6.4): `lldb.exe`, `lldb-dap.exe` [A: `remote-ios` plugin present, task D12] |
| `llvm` (fallback) | non-Swift debugging when the Swift toolchain is missing; A/B testing upstream fixes | the distro package: Arch `lldb` ≥ 23.1, apt.llvm.org `lldb-23` (Arch 23.1.1 has `PlatformRemoteiOS` [R dv-11]) | the LLVM 23.1.3 MSI (lldb, lldb-dap, Python 3.14 DLL) [R dv-12] |

- **Who implements it.** Discovery and provisioning live in core's toolchain resolver: `russet-toolchain`,
  `Component::{Lldb, LldbDap, LibPython}`, and `toolchain.resolve`, which returns `ToolInfo { path, env }`
  ([core.md](core.md) section 10; task CORE-10). This document fixes the rules; `russet-debug`
  only consumes `ToolInfo`.
- **Config.** `lldb = "auto" | "swift" | "llvm" | <path>` (core config).
- **Discovery order:** user setting, then the build toolchain, then `PATH`, then known prefixes.
- **Probes** (≤ 2 s each):
  - `lldb --version`;
  - `lldb -b -o 'platform list'`, which must list `remote-ios`;
  - Linux only: the ELF `NEEDED` of `liblldb.so*`, to find the required `libpython3.X.so.1.0`.

### 8.2 The libpython problem on Linux

> **As implemented (W3, RD-05, 2026-10-11):** the resolver reads `liblldb`'s `DT_NEEDED` libpython with its own
> ELF reader (russet-toolchain's `elf.rs`: the header, the program headers and the dynamic section only; goblin is
> not used) and searches the `ldconfig` cache, then an existing uv-managed python-build-standalone (provenance
> `uv`), then Russet's managed `python-shared`, installed by `russet toolchain install python-shared` into
> `data/toolchains/python-shared/3.12.15/` (ARCHITECTURE §14.8) from the signed `pins/stable.toml` (not
> `$XDG_DATA_HOME/russet/python/…` or `russet-toolchains.toml`). `~/.local/opt/python-3.12` is never used. Doctor
> reports, for example, `swift lldb 21.0.0 at /usr/lib/swift/usr/bin/lldb, python: pbs 3.12.15 (uv)`, and offers
> the python-shared install as its automatic fix only when a missing libpython is the cause.

- **Facts.** Swift's Linux `liblldb` has a hard `DT_NEEDED` on `libpython3.12.so.1.0`; without it neither
  `lldb` nor `lldb-dap` starts [V2]. Arch ships Python 3.14 [VF].
- **Decision** (answers [core.md](core.md) section 22, open question 4: "managed build vs user-provided"): use a pinned
  **python-build-standalone** runtime. A source build of CPython is not needed.
  - The stripped PBS archive is a 34 MB download and about 107 MB installed.
  - A source-built `--enable-shared` CPython takes about 300 MB [V1, V14].
  - The resolver's `python-shared` component therefore uses `source = "python-build-standalone"`.
- **Russet's answer.** Python is a load-time dependency only. Russet's own command sequences never run
  Python: no `script …`, no `SetPlatformFileSpec`, no async toggling [V3–V5].
  1. If the system has a matching `libpython3.X.so.1.0` (`ldconfig -p`), use it.
  2. Otherwise provision python-build-standalone `cpython-3.X.Y+<rel>-<triple>-install_only_stripped.tar.gz`
     (34 MB for Linux x86_64 [V14]) into `$XDG_DATA_HOME/russet/python/cpython-3.X.Y/`.
     - Download it from the GitHub release.
     - Pin its SHA-256 in `russet-toolchains.toml`.
     - The version and release are picked from the `DT_NEEDED` soname.
  3. Every LLDB child process gets this environment:
     `LD_LIBRARY_PATH=<python>/lib[:<swift>/usr/lib/swift/linux:<swift>/usr/lib/swift/host/compiler]`,
     `PYTHONHOME=<python>`. Verified with PBS 3.12.15 [V1].
  4. `russet doctor` reports the result: `lldb: swift 6.4 (lldb 21.0.0), python: pbs 3.12.15 (provisioned)`.
- **Upstream asks.**
  - swift.org: build Linux LLDB with Python delay-loaded, or ship an `LLDB_ENABLE_PYTHON=OFF` variant.
    Windows already delay-loads; `lldb-dap --check-python` exists there.
  - Arch: a `swift-lldb` package built against the system Python.

### 8.3 Swift resource-dir overlay (required for `po`)

- **Problem [V6].** LLDB derives the Swift resource dir from the real path of `liblldb`, which is
  `<toolchain>/usr/lib/swift`. On Linux that dir contains swift-corelibs module maps (`CoreFoundation`,
  `dispatch`, `os`, `Block`, …) that clash with the iOS SDK's modules. Importing a module built by Swift
  Build then fails.
- **Fix.** A per-(toolchain, SDK) overlay prefix at `$XDG_DATA_HOME/russet/lldb-overlays/<swift-ver>-<sdk-build>/`:

```text
usr/bin/lldb, usr/bin/lldb-dap      copies (0.7 MB, 1.8 MB)
usr/lib/liblldb.so.21.0             cp --reflink=auto from the toolchain (0 bytes on btrfs/XFS [V11]; 232 MB on ext4)
usr/lib/swift  -> <SDK>/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift   (symlink)
usr/lib/clang  -> <toolchain>/usr/lib/clang                                          (symlink)
usr/local/lib64/python3.12 -> <toolchain>/usr/local/lib64/python3.12                  (symlink)
```

- Child environment: add `<toolchain>/usr/lib/swift/linux:<toolchain>/usr/lib/swift/host/compiler` to
  `LD_LIBRARY_PATH`. The Swift runtime and the compiler host libs are reached through RUNPATH entries that
  the overlay redirects [V].
- The overlay is optional (`russet.debug.swiftExpressions = auto|on|off`). `auto` builds it on first use
  and only when it costs no disk (reflink works) or the user agrees.
- Caveat: macro plugins. LLDB looks for macro plugins under `<resource dir>/host/plugins`. With the overlay
  those are macOS binaries. Expressions that expand macros need
  `target.experimental.swift-plugin-server-for-path` mapped to the Linux `swift-plugin-server` plus the
  xtool `OpenAppleMacrosServer` [A, RD-16].
- On Windows the same idea needs a copy of `liblldb.dll`. ReFS/Dev Drive block cloning avoids the cost
  [A]. Deferred to RD-18.
- **Upstream ask** (RD-21): a `target.experimental.swift-resource-dir` setting, or honouring the SDK's
  `swiftResourcesPath`, in swiftlang/llvm-project. The overlay then becomes unnecessary.

### 8.4 Settings Russet applies (init commands)

```text
settings set target.memory-module-load-level <partial|minimal>   # partial on USB, minimal on Wi-Fi (RD-19 decides)
settings set plugin.process.gdb-remote.packet-timeout <30|60>    # USB | Wi-Fi
settings set symbols.clang-modules-cache-path <cache>/lldb-module-cache
settings set target.sdk-path <SDK>/…/iPhoneOS<ver>.sdk           # Swift: debug-info SDK lookup fails off macOS [V6]
settings set target.load-cwd-lldbinit false
settings set interpreter.stop-command-source-on-error false     # lldb CLI scripts only
```

---

## 9. LLDB commands, DAP integration, IDE

### 9.1 Generated attach script (Python-free)

`russetd` writes `<session dir>/attach.lldb`. It is used by `russet debug`; the same command list goes into
`attachCommands` for `lldb-dap`.

```text
platform select remote-ios --sysroot "<symbol cache>/<build>/<dsc-uuid>"     # only when local symbols exist [V7]
target create --arch arm64-apple-ios "<local executable (unsigned build product or signed copy)>"
target modules search-paths add "<device .app dir>" "<local .app dir>"     # belt and braces; UUID match suffices [V5]
process connect -p gdb-remote "<broker url>"                              # unix-connect:///… or connect://127.0.0.1:N
```

With no `--sysroot`, use `platform select remote-ios`.

Why each line is there:
- `target create` uses the build product that has the debug map, or the dSYM next to it (section 11.5).
- No `script …` lines appear anywhere. `SetPlatformFileSpec` (M1 [VF]) is replaced by the UUID match plus
  search paths.
- `target create --remote-file` must not be used: it fails offline [V5].

### 9.2 `lldb-dap` attach configuration (what the hub sends to `lldb-dap`)

```jsonc
{
  "type": "lldb-dap", "request": "attach", "name": "iPhone (Russet) - Hello",
  "initCommands": [ /* section 8.4 */ ],
  "attachCommands": [ /* section 9.1, with ! prefix so a failure stops the chain */
    "!platform select remote-ios",
    "!target create --arch arm64-apple-ios \"/home/u/proj/.build/out/Products/Debug-iphoneos/Hello-App\"",
    "target modules search-paths add \"/private/var/containers/Bundle/Application/1C2…/Hello.app\" \"/home/u/proj/xtool/Hello.app\"",
    "!process connect unix-connect:///run/user/1000/russet/sessions/ses_7k2/gdb.sock"
  ],
  "stopOnEntry": false,
  "terminateCommands": [],
  "sourceMap": [],
  "timeout": 60
}
```

- `lldb-dap` runs `attachCommands` before it emits `initialized` (`lldb-dap` 21 [V4]). The IDE then sends
  `setBreakpoints`, and `configurationDone` resumes the app.
- The `!` prefix stops the command chain on failure [S lldb-dap README].
- `lldb-dap` is spawned by the hub with the environment from 8.2/8.3. It is not spawned by VS Code.

### 9.3 DAP hub (`russet_debug::dap`)

Core leaves the split open ([core.md](core.md) 3.3: "the debug doc decides"). **Decision:** the daemon
launches and owns `lldb-dap` behind a per-session DAP hub, and the IDE only connects to it. Reasons:
- the libpython/overlay environment (8.2–8.3) is applied in one place;
- the session survives window reloads as a detached lease (core 6.5);
- agents can share the session.

The IDE-launched `lldb-dap` (Mode 1, 9.4) stays as a development fallback.

One hub per session, hosted in `russetd`:

- **Endpoint.** Linux: Unix socket `…/sessions/<sid>/dap.sock`. Windows: named pipe
  `\\.\pipe\russet-dap-<sid>`. The IDE connects with `vscode.DebugAdapterNamedPipeServer`.
- **Backend.** One `lldb-dap` child (Swift sessions) or `flutter debug_adapter` (Flutter sessions).
  Mixed sessions get two hubs (compound).
- **Multiplexing.** Clients are the IDE plus any number of agent clients (MCP tools).
  - Sequence numbers are rewritten per client.
  - Responses go only to the requester.
  - Events are broadcast.
  - Only the IDE client may send `disconnect`/`terminate`. Agent requests that change execution
    (`continue`, `next`, …) are allowed and produce normal events that the IDE renders.
- **Russet custom requests** (DAP `customRequest`):

| Command | Arguments | Result |
|---|---|---|
| `russet/hotReload` | `{ full?: boolean }` | `{ ms: number }`. Flutter: forwarded as Flutter DAP `hotReload` / `hotRestart`. Swift: Quick Relaunch (4.7). |
| `russet/relaunch` | `{}` | `{ pid }`. The backend `lldb-dap` is replaced; breakpoints are re-sent from the hub's cache. |
| `russet/jitStats` | `{}` | `JitStats` |
| `russet/deviceLogs` | `{ sinceMs?: number }` | `{ entries: LogEntry[] }` |

- **Swapping the backend on relaunch.** The hub keeps the last `setBreakpoints`, `setFunctionBreakpoints`
  and `setExceptionBreakpoints` per source. After a new broker attaches, it replays them to a fresh
  `lldb-dap`. The IDE sees `continued`, then `stopped` events in the same session.

### 9.4 IDE extension contract (Russet extension, TypeScript)

- `contributes.debuggers`: type `russet`. Launch-config schema:

```jsonc
{
  "type": "russet", "request": "launch", "name": "Debug on iPhone",
  "device": "${command:russet.pickDevice}",     // UDID or "default"
  "target": "Hello",                              // product / scheme
  "configuration": "Debug",
  "args": [], "env": {},
  "stopOnEntry": false,
  "flutter": { "mode": "dart|dart+native" },     // Flutter projects only
  "jitServicer": "native|lldb-python",           // advanced
  "symbols": "auto|none|core|full"               // section 11
}
```

- `resolveDebugConfigurationWithSubstitutedVariables`:
  1. Call `debug.prepare` (JSON-RPC) and show its job progress (`job.*` notifications) in the status bar
     and the Run panel.
  2. Return the config with an internal field `__russet: { sessionId, dapPath }` added.
- `createDebugAdapterDescriptor` returns `new DebugAdapterNamedPipeServer(config.__russet.dapPath)`.
- If preparation fails, the error `kind` and `hint` are shown, and the config resolves to `undefined`, which
  cancels the launch.
- Buttons:
  - **Run** → `debug.run`;
  - **Debug** → `vscode.debug.startDebugging(russet config)`;
  - **Hot Reload** → `session.customRequest('russet/hotReload')`. Without a debug session it uses
    `flutter.reload` (Flutter) or `debug.relaunch` (Swift).
- A `russet: Attach to process…` command lists processes through `app.processes` (device area) and
  starts a `request: "attach"` config.
- Mode 1 fallback, for development: `DebugAdapterExecutable(lldb-dap, env)` with the 9.2 config straight
  from `debug.prepare`. No hub; agents cannot share the session.

---

## 10. Swift `po` and expression evaluation off-Mac

### 10.1 Status after this pass

| Stage | Off-Mac status |
|---|---|
| `frame variable` / DAP Variables (TypeSystemSwiftTypeRef, reflection) | **works** in the fake-process test [V6]. M1 ran `frame variable` on the device, but its output was not recorded [VF]; T2 records it. |
| SwiftASTContext setup for an `arm64-apple-ios` frame on Linux | **works** when `target.sdk-path` is set *and* the resource dir comes from the SDK's Xcode toolchain [V6] |
| Import of the user module and SDK modules (Hello, SwiftUI, _Concurrency, …) | **works** under the same conditions [V6] |
| Linked-library check (`libswift_Concurrency.dylib`, …) | needs them in the process image list; they are in every Swift app's process [A, D7] |
| JIT linking: resolving runtime symbols (`swift_retain`, `$s…`) | needs **symbol tables** for libswiftCore and friends. `memory-module-load-level minimal` gives none. Needs `partial`/`complete`, or local DSC-extracted dylibs (section 11). This is the likely next blocker [A, D7]. |
| Allocating JIT memory and running it on the device (`_M` + thread-plan call) | the path Xcode uses daily. TXM accepts debugger-created code mappings [R dv-19] [A, D7]. |

### 10.2 What Russet does

1. Build side (owned by the build area; required interface):
   - debug builds keep the module's `.swiftmodule` reachable: either `-Xlinker -add_ast_path -Xlinker <M.swiftmodule>`, as SwiftPM does on Darwin, or pass the module directories to LLDB as `target.swift-module-search-paths`;
   - keep `-serialize-debugging-options`.
2. Use the overlay of 8.3 and the settings of 8.4.
3. For a `po` session, load these from the symbol cache when present: `libswiftCore`, `libswift_Concurrency`, `libswift_StringProcessing`, `libobjc.A`, `libdyld`, `libsystem_*`, `Foundation`, `CoreFoundation`, `SwiftUI`, `SwiftUICore`, `UIKitCore`. Otherwise set `target.memory-module-load-level complete` *for this session only*, after a confirmation that warns it is slow.
4. Keep `target.experimental.swift-use-context-free-po` at its default (true) [S `TargetProperties.td` in the swift fork]. It lets `po obj` work through the runtime even when module import fails.
5. Fallback chain shown in the UI:
   1. `po`;
   2. `expr -O --`;
   3. `frame variable -O` (no compiler);
   4. `v`.

Device experiment D7 (section 23.4) decides how much of this is on by default. It is part of the M3
exit criterion "status of `po` documented" [R landscape M3].

---

## 11. Symbols and symbolication (`russet_debug::symbols`)

### 11.1 Tiers

| Tier | What | Cost | Gives |
|---|---|---|---|
| 0 (default) | nothing downloaded. LLDB at `partial`/`minimal`. Russet's **memory symbol index**, built once per DSC UUID: export tries and in-memory `LC_SYMTAB` of images read through the broker, stored as a compact `addr→name` table | about 10–40 MB per iOS build [A, RD-12] | names for exported/global functions in crash reports and native-mode backtraces |
| 1 "core" | DSC files fetched from the device + `ipsw dyld extract` of a core set (10.2 item 3 plus app-linked frameworks) into `Symbols/` | the full DSC download first (several GB [A, D8]). The raw DSC can be deleted afterwards. | full local symbols for the core set in LLDB (`--sysroot`); `po` symbol resolution |
| 2 "full" | as tier 1 with `--all` | about DSC size on disk | Xcode-equivalent DeviceSupport |

### 11.2 Fetch

- **Service:** RSD `com.apple.dt.remoteFetchSymbols`. It needs the capability
  `com.apple.dt.remoteFetchSymbols.dyldSharedCacheFiles`.
  1. Request `{"DSCFilePaths": []}` (with a side-channel UUID).
  2. Read *n* responses, each `{filePath, fileTransfer{expectedLength}}`.
  3. Downloads run as parallel per-file transfers (pymobiledevice3 uses 4 workers)
     [S `services/remote_fetch_symbols.py`].
- **Legacy service:** lockdown `com.apple.dt.fetchsymbols`. Commands are `0x30303030` (list) and `1`
  (get file N); each reply is a big-endian u64 size, then the bytes [S `services/dtfetchsymbols.py`].
- **Rules (N3):**
  1. Call `debug.symbols.plan` first. It lists files and total size without downloading.
  2. Download only when **all** hold:
     - the user confirmed the size (`confirmBytes` must equal the planned total);
     - free space ≥ total + 2 GiB;
     - the cache quota allows it (default 20 GiB, LRU by `lastUsed`).
  3. Resumable: partial files keep `.part` and a byte offset. Writes are streamed; there is no
     double buffering.
- **Cache key and layout:**

```text
$XDG_CACHE_HOME/russet/device-support/<ProductBuildVersion>/<dsc-uuid>/     (Windows: %LOCALAPPDATA%\Russet\cache\device-support\…)
  manifest.json
  dsc/System/Library/Caches/com.apple.dyld/dyld_shared_cache_arm64e{,.01,…,.symbols}   (may be deleted after extraction)
  Symbols/usr/lib/libobjc.A.dylib, Symbols/System/Library/Frameworks/UIKit.framework/UIKit, …   (LLDB --sysroot layout [V7])
  index/memsyms.bin             tier-0 index
  index/dsc.a2s                 ipsw address-to-symbol cache
```

`manifest.json`:

```json
{ "schema": 1, "productVersion": "27.0", "buildVersion": "24A437", "productTypes": ["iPhone13,2"],
  "dscUUID": "…", "arch": "arm64e", "tier": "core", "files": [{"path": "…", "size": 0, "sha256": "…"}],
  "extracted": [{"image": "/usr/lib/libobjc.A.dylib", "uuid": "…", "path": "Symbols/usr/lib/libobjc.A.dylib"}],
  "createdAt": "…", "lastUsed": "…", "rawDscDeleted": true }
```

- **DSC UUID source:** `jGetSharedCacheInfo` through the broker (`shared_cache_uuid`) [S V9], or the
  header of the first fetched file.
- **Extraction:**
  - Tool: `ipsw dyld extract <dsc> <dylib> --output <Symbols root> [--slide] [--objc]`. ipsw is MIT,
    a single Go binary, Linux and Windows builds; version pinned (3.1.736 here).
  - Each output must keep the original `LC_UUID` [A, RD-12 checks] and be readable by LLDB as a regular
    dylib. The in-cache flag `MH_DYLIB_IN_CACHE` must be cleared [A].
  - `ipsw dyld split` is macOS-only and is never used [R dv-16].
- **Upstream ask:** LLDB loading an on-disk DSC on non-Apple hosts. Today
  `HostInfoBase::GetSharedCacheImageInfo` returns `{}` [R dv-16]. This would make extraction unnecessary.

### 11.3 How LLDB gets the symbols

- `platform select remote-ios --sysroot <…/<dsc-uuid>>` when tier ≥ 1. LLDB resolves
  `<sysroot>/Symbols/<device path>` by UUID [S V7].
- Images not present locally fall back to `memory-module-load-level`.
- Tier 0 does not feed LLDB. There is no per-module "symbols from file" for memory images.

### 11.4 Russet's own symbolizer (crashes, native-mode backtraces, logs)

`symbolize(frames: &[RawFrame]) -> Vec<SymbolicatedFrame>`, in this order:

1. **App images** (UUID matches a build in the dSYM index, 11.5): the `symbolic` crate (getsentry,
   MIT, 13.9.0) reads DWARF from the dSYM or the debug map. `symbolic-demangle` with the `swift` feature
   demangles. Result: function, file, line, inlined frames.
2. **System images, tier ≥ 1:** extracted dylib symbols (`object` crate), plus the DSC `.symbols` local
   symbols through `ipsw dyld a2s --cache index/dsc.a2s` when the raw DSC is still present.
3. **System images, tier 0:** `memsyms.bin` nearest preceding symbol.
4. Otherwise: `<image> + 0x<offset>`.

### 11.5 dSYM index (interface with the build area)

- Every debug or release build records `{uuid → {binary, dsym?, objectFiles?}}` in
  `.russet/builds/<build-id>/symbols.json`.
- When the build area produces a dSYM (`dsymutil` from the xtool toolset or `llvm-dsymutil`), it is
  copied to `$XDG_DATA_HOME/russet/dsyms/<UUID>.dSYM`.
- That copy keeps crash symbolication working after `.build` is cleaned. The debug map refers to `.o`
  files by absolute path [V13].
- Debug builds write no dSYM by default ([build.md](build.md): `dsym = "never" | "release" | "always"`).
  For them the index records the object files, so crash symbolication works while those files exist.
  Users who need crash history should set `dsym = "always"`. The crash panel offers this when it meets an
  app UUID it cannot resolve.

---

## 12. Logs (`russet_debug::logs`)

| Source | Service | When | Notes |
|---|---|---|---|
| stdout/stderr | CoreDevice `openstdiosocket` (PTY, merged streams, `\r\n`) or DVT `outputReceived` | Russet-launched processes | `print`; `NSLog` and `os_log` when mirrored to stderr under DT mode [A D6] |
| os_log / unified log | `com.apple.os_trace_relay` (lockdown) or `com.apple.os_trace_relay.shim.remote` (RSD). `StartActivity {Pid, MessageFilter:0xFFFF, StreamFlags: PAYLOAD\|HISTORICAL\|CALLSTACK\|DEBUG(\|INFO)}` | always (pid filter) | each entry has pid, timestamp, level (notice/info/debug/error/fault), image, offset, subsystem/category, message [S `services/os_trace.py`]. Dynamic strings may read `<private>` on iOS 26+ [R dv-35]. |
| debugger | broker / `lldb-dap` `output` events | debug sessions | category `debugger` |
| JIT | servicer | Flutter | category `jit` (warnings, stats) |

- **Pipeline.**
  - The raw sources are core StreamHub sources owned by the device area: `app.launch` console and
    `logs.subscribe {source: "oslog"}` ([core.md](core.md) 6.1, 6.6).
  - This area adds the per-session merged view (`debug.output.*`). It has a bounded buffer of 10 000
    entries, matching core's subscription queues; slow consumers lose the oldest entries and are told
    `dropped: n`.
  - Lines are de-duplicated: an os_log line mirrored to stderr by DT mode is dropped when the same message
    text arrives from both sources within 50 ms (stdout wins).
  - Filtering is the same for UI and agents: `{pid, process, subsystem, category, minLevel, text, regex}`.
  - Rate limit: at most 2 000 lines/s to the IDE; the overflow is counted and reported.
- **Persistence.** `run` and `debug` sessions write `.russet/sessions/<sid>/output.log` (plain text) and
  `oslog.jsonl`. They are rotated, keeping the last 20 sessions.

---

## 13. Crash reports (`russet_debug::crash`)

1. **Live capture.**
   - With LLDB attached: LLDB stops on the exception and the user sees it.
   - Without LLDB (`run`, native mode): the broker records a `CrashSnapshot`
     (`{pid, tid, metype, medata[], regs, frames (fp walk), images}`), then detaches so the system crash
     reporter runs (7.5).
   - Plain `run` sessions have no broker. They rely on post-mortem retrieval only.
2. **Detection.**
   - The process exits by signal (CoreDevice `monitorprocesstermination`, DVT
     `processWithPID:terminatedWithExitCode:orCrashingSignal:`), or the stdio socket hits EOF without an
     exit code.
   - Also watch os_trace for `osanalyticshelper` / `OSAnalytics` "Saved type … <file>.ips" lines, the
     method pymobiledevice3 `crash_reports.watch` uses [S]. The filename may be redacted on iOS 26+ [A].
3. **Retrieval.**
   1. Send `com.apple.crashreportmover` (RSD `…shim.remote`) a flush (ack `ping\0`).
   2. List with `com.apple.crashreportcopymobile` (AFC).
   3. Pick `.ips` files whose name starts with the executable name and whose mtime is ≥ the session start
      [S `services/crash_reports.py`].
4. **Parsing.**
   - `.ips` = a JSON header line followed by a JSON body (iOS 15+).
   - Fields: `bug_type`, `exception{type,signal,codes}`, `termination`, `threads[].frames[{imageIndex,imageOffset}]`,
     `usedImages[{uuid,base,path,arch}]`, `sharedCache{base,size,uuid}`, `lastExceptionBacktrace`, `asi`.
   - Unknown fields are kept raw.
5. **Symbolication:** section 11.4. The output is `SymbolicatedCrash` JSON plus an Xcode-like text
   rendering.
6. **Storage:** `$XDG_DATA_HOME/russet/crashes/<bundleId>/<timestamp>-<incident>.{ips,json,txt}`, with an
   index used by `crash.list`.

---

## 14. Flutter specifics (`russet_debug::flutter`)

Shared with the Flutter build area. This section covers the run/debug half only.

### 14.1 Custom device registration (owned by the build design)

- Registration, the `custom_devices.json` entry and the `install` command are specified in
  [build.md](build.md) section 6.3 (task B13). This document uses its command names and argument
  conventions:
  - `--udid <udid>`
  - `run-debug … --engine-options=${engineOptions}`
  - `forward --device-port ${devicePort} --host-port ${hostPort}`, with success regex `^forwarding `
- **Who implements what.** build.md assigns `run-debug` and `forward` to the device area. The
  behaviour specified here (broker, JIT servicer, crash capture, VM-service handling) lives in
  `russet-debug` and uses the device area's launch and forward APIs. The lead decides which handler module
  hosts the CLI entry point.
- Background facts:
  - `linux-x64` is the only option for custom devices. Debug kernels are platform-neutral, and the plugin
    registrant checks `Platform.isIOS` at runtime [R fx-27] [S `custom_device_config.dart`].
  - Removing the last app of a free team makes iOS drop the developer trust [R xlinux
    `custom_device.py` comment], so `uninstall` should stay a no-op by default.
  - On a Windows host, flutter_tools must still build the asset bundle for the `linux-x64` target. This is
    verified in RD-18 / B17 [A]. The fallback is the xcross-style native Windows path [R fx-25].

### 14.2 `run-debug` contract

- **stdout:** the app's stdout/stderr verbatim, so `ProtocolDiscovery.vmService` finds "The Dart VM
  service is listening on …" [S `custom_device.dart`].
- **stderr:** Russet status lines prefixed `[russet]`.
- **Lifetime:** runs until the app exits or flutter_tools kills it. On SIGTERM/SIGINT, `k` the app and
  exit 0.
- **JIT:** native servicer (7.5) by default. `--jit-servicer=lldb-python` uses a headless synchronous
  `lldb` running Flutter's helper (BSD-3, vendored with notice) and manual continue on iOS 27 [S `lldb.dart`].

### 14.3 VM service forwarding

| Transport | How the host reaches the device's VM service port |
|---|---|
| USB | usbmux `Connect(port)` (iproxy-equivalent), loopback-bound service OK |
| Wi-Fi (lockdown over TCP) | usbmux network device `Connect` via the device daemon (netmuxd/AMDS) [R dv-31] |
| RSD tunnel only (RemotePairing) | TCP to `[tunnel address]:port`. The service must listen on all interfaces (`--vm-service-host=::0`), and only russetd can reach it (userspace tunnel). |

- `forward` prints `forwarding <hostPort> -> <devicePort>` once it is listening (matches build.md's
  `^forwarding ` regex), then stays alive until killed. flutter_tools kills it to unforward. It is a
  thin client of the device area's `forward.open`/`forward.close`.
- **Discovery fallbacks**, in order: stdout banner; os_trace (if unredacted); mDNS `_dartVmService._tcp`
  (Info.plist `NSBonjourServices`) [R fx-14]; `--vm-service-port=<fixed>` with
  `--disable-service-auth-codes` (opt-in on USB only).

### 14.4 Hot reload and restart

- **IDE Flutter debug:** the hub fronts `flutter debug_adapter` (Flutter's DAP). `russet/hotReload` maps
  to its custom requests `hotReload` / `hotRestart`.
- **Headless and agents:** `russetd` runs `flutter run --machine -d russet-<udid>` and sends
  `app.restart {appId, fullRestart}`.
- Both paths depend on the JIT servicer keeping the app alive. That is the only Russet-specific part.

---

## 15. Wi-Fi debugging

- **Prerequisites** (device area):
  - pairing over USB once;
  - Wi-Fi connections enabled (`lockdown wifi-connections on`), or a RemotePairing record created over
    USB without a prompt;
  - mDNS discovery of `_apple-mobdev2._tcp` / `_remotepairing._tcp`.
  - Firewall: UDP 5353 in, plus the RemotePairing TCP port [R dv-30, dv-31] [S pymobiledevice3 docs
    `ios17-tunnels.md`].
- **Run/debug differences:**
  - packet timeout 60 s;
  - attach timeout 45 s;
  - `memory-module-load-level minimal`;
  - JIT `eager` pipelining (7.6) once `safe` has proven stable for 50 stops;
  - VM service host `::0`;
  - lockdown heartbeat kept alive for the session (StikDebug does this during debug sessions [S]);
  - Russet warns when RTT > 40 ms (`jit.stats.rttEstimateMs`).
- **Failure modes:**
  - The device locks or sleeps: the tunnel drops, the session goes to `Lost`, and the IDE offers
    "Reconnect" (attach again when the app is still alive).
  - The tunnel stalls for minutes, as reported for the Python userspace stack [R xlinux README]: a
    watchdog at the transport level detects it (tunnel keepalive plus lockdown heartbeat, from the device
    plane). No gdb-remote packets are sent while the inferior runs; only `0x03` is legal then.

---

## 16. Windows specifics

| Topic | Linux | Windows |
|---|---|---|
| Broker endpoint | Unix socket, 0600 | `127.0.0.1:<ephemeral>`, one client |
| DAP hub endpoint | Unix socket | named pipe `\\.\pipe\russet-dap-<sid>` |
| LLDB | Swift toolchain + PBS libpython (hard dependency) | Swift for Windows `lldb.exe`/`lldb-dap.exe`, or the LLVM MSI. Python is delay-loaded and not needed by Russet's commands. |
| `po` overlay | reflink copy of `liblldb.so` | copy of `liblldb.dll` (block clone on ReFS/Dev Drive) [A] |
| Paths | POSIX | the device paths stay POSIX inside LLDB commands. Local paths are quoted; long paths need `\\?\` for deep `Symbols/…` trees [R tc-29]. LLDB long-path support is to be verified (D12). |
| Symbol cache | `$XDG_CACHE_HOME` | `%LOCALAPPDATA%\Russet\cache` |
| Crash, symbols, logs | same | same (ipsw has Windows builds) |
| Flutter JIT | native servicer | native servicer, no LLDB at all (biggest Windows win) |

The device plane (Apple Mobile Device Service or netmuxd, userspace tunnel without Admin) is
specified by the device area [R dv-05, dv-32].

---

## 17. Rust APIs

### 17.1 Crate `russet-gdbremote`

Licence: AGPL-3.0-only, like all of Russet. It has no device knowledge.

```rust
pub mod codec {
    /// One frame on the wire. Payload is still escaped exactly as received.
    pub enum Frame { Ack, Nack, Interrupt, Packet(bytes::Bytes) }
    pub struct GdbCodec { pub ack_mode: bool, pub max_packet: usize }
    impl tokio_util::codec::Decoder for GdbCodec { type Item = Frame; type Error = GdbError; /* … */ }
    impl tokio_util::codec::Encoder<&[u8]> for GdbCodec { type Error = GdbError; /* frames + checksums */ }
    pub fn unescape(payload: &[u8]) -> Vec<u8>;          // '}' escapes + '*' RLE
    pub fn escape_binary(data: &[u8]) -> Vec<u8>;
}

pub mod stop {
    pub struct StopReply {
        pub signal: u8,
        pub thread: Option<u64>,
        pub reason: Option<String>,               // "breakpoint" | "signal" | "exception" | "trace" | ...
        pub regs: std::collections::BTreeMap<u32, u64>, // expedited GPRs, keyed by debugserver regnum
        pub threads: Vec<u64>,
        pub thread_pcs: Vec<u64>,
        pub mach_exception: Option<MachException>,
        pub jstopinfo: Option<serde_json::Value>,
        pub raw: bytes::Bytes,
    }
    pub struct MachException { pub metype: u32, pub medata: Vec<u64> }
    pub enum StopEvent { Stopped(StopReply), Exited { code: u8 }, Terminated { signal: u8 }, Output(Vec<u8>) }
    pub fn parse_stop(payload: &[u8]) -> Result<StopEvent, GdbError>;
}

pub struct ServerInfo { pub packet_size: usize, pub compressions: Vec<String>, pub page_size: u64,
                        pub os_version: Option<String>, pub ostype: Option<String> }

pub enum ResumeAction { ContinueAll, ContinueWithSignal { tid: u64, signal: u8 }, Step { tid: u64 }, Raw(String) }

pub struct GdbClient<S> { /* split stream, codec, pending replies, event channel */ }
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> GdbClient<S> {
    pub async fn handshake(stream: S, opts: HandshakeOptions) -> Result<(Self, ServerInfo), GdbError>;
    pub async fn request(&mut self, payload: &[u8]) -> Result<bytes::Bytes, GdbError>;
    /// Writes all packets in one syscall, then collects one reply per packet, in order.
    pub async fn pipeline(&mut self, payloads: &[Vec<u8>]) -> Result<Vec<bytes::Bytes>, GdbError>;
    pub async fn attach(&mut self, pid: u32) -> Result<StopReply, GdbError>;
    pub async fn resume(&mut self, action: ResumeAction) -> Result<(), GdbError>;   // does not wait
    pub async fn wait_stop(&mut self) -> Result<StopEvent, GdbError>;
    pub async fn interrupt(&mut self) -> Result<StopReply, GdbError>;
    pub async fn read_memory(&mut self, addr: u64, len: usize) -> Result<Vec<u8>, GdbError>;
    pub async fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), GdbError>;
    pub async fn set_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError>;     // Z0
    pub async fn remove_breakpoint(&mut self, addr: u64, kind: u8) -> Result<(), GdbError>;  // z0
    pub async fn write_register(&mut self, tid: u64, reg: u32, value: u64) -> Result<(), GdbError>;
    pub async fn loaded_images(&mut self) -> Result<Vec<ImageInfo>, GdbError>;  // jGetLoadedDynamicLibrariesInfos
    pub async fn shared_cache_info(&mut self) -> Result<SharedCacheInfo, GdbError>;
    pub async fn memory_region(&mut self, addr: u64) -> Result<MemoryRegion, GdbError>;
    pub async fn thread_stop_info(&mut self, tid: u64) -> Result<StopReply, GdbError>;
    pub async fn detach(self) -> Result<(), GdbError>;
    pub async fn kill(self) -> Result<Option<StopEvent>, GdbError>;
    pub fn into_parts(self) -> RawSession<S>;   // hand to the interposer
}

pub mod interpose {
    /// Frame-level relay with per-side ack state and hooks.
    pub struct Interposer<U, D> { /* upstream: debugserver, downstream: LLDB */ }
    pub trait InterposeHooks: Send {
        fn on_downstream(&mut self, pkt: &[u8]) -> DownstreamAction;      // Forward | Reply(Vec<u8>) | Drop
        fn on_upstream_stop(&mut self, stop: &stop::StopReply) -> UpstreamStopAction; // Forward | Handle
        fn filter_upstream_reply(&mut self, request: &[u8], reply: &mut Vec<u8>);   // jThreadsInfo scrub
    }
}

pub mod arm64 { pub fn is_plain_return(insn: u32) -> bool; pub const REG_X0: u32 = 0; pub const REG_LR: u32 = 30; pub const REG_PC: u32 = 32; }

#[derive(thiserror::Error, Debug)]
pub enum GdbError { Io(std::io::Error), Protocol(String), Remote { code: u8, message: Option<String> },
                    Timeout(&'static str), Unsupported(&'static str), Disconnected }
```

### 17.2 Crate `russet-debug`

Licence: AGPL-3.0-only.

> **Superseded on the wire (ARCHITECTURE §9.5, §20.1):** `SessionState` and `SessionSummary` convert to the wire
> shapes of ARCHITECTURE §9.5. `Detached` is not a wire state: a session whose debugger detached stays `running`,
> with `debugger: detached`, and its lifetime is `lease: owned | detached` [D 84].

```rust
pub struct DebugService { /* device: Arc<dyn DeviceServices>, build: Arc<dyn BuildArtifacts>, cfg, sessions */ }

impl DebugService {
    pub fn new(device: Arc<dyn DeviceServices>, build: Arc<dyn BuildArtifacts>, cfg: DebugConfig) -> Self;
    pub async fn run(&self, req: RunRequest) -> Result<SessionHandle, DebugError>;
    pub async fn prepare_debug(&self, req: DebugRequest) -> Result<PreparedDebug, DebugError>;
    pub async fn attach(&self, req: AttachRequest) -> Result<PreparedDebug, DebugError>;
    pub fn session(&self, id: &SessionId) -> Option<SessionHandle>;
    pub fn sessions(&self) -> Vec<SessionSummary>;
}

pub struct RunRequest { pub device: DeviceSelector, pub app: AppRef, pub args: Vec<String>,
                        pub env: BTreeMap<String, String>, pub launch: LaunchStrategy, pub capture: CaptureOptions }
pub struct DebugRequest { pub run: RunRequest, pub stop_on_entry: bool, pub flavor: DebuggerFlavor,
                          pub jit: JitPolicy, pub symbols: SymbolTier, pub frontend: Frontend }   // Frontend::Dap | LldbCli | None
pub struct AttachRequest { pub device: DeviceSelector, pub target: AttachTarget }              // Pid(u32) | Bundle(String)
pub enum LaunchStrategy { Auto, CoreDevice, Dvt }
pub enum JitPolicy { Auto, Native, LldbPython, Off }
pub enum DebuggerFlavor { Swift, Llvm }

pub struct PreparedDebug {
    pub session: SessionHandle,
    pub pid: u32,
    pub broker: BrokerEndpoint,              // Unix { path } | Tcp { addr }
    pub lldb: LldbLaunchPlan,                // program, args, env, init_commands, attach_commands
    pub dap: Option<DapEndpoint>,            // hub endpoint when Frontend::Dap
}

pub struct SessionHandle { /* Arc<Session> */ }
impl SessionHandle {
    pub fn id(&self) -> &SessionId;
    pub fn state(&self) -> SessionState;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SessionEvent>;
    pub async fn terminate(&self, how: TerminateMode) -> Result<(), DebugError>;   // Kill | Detach
    pub async fn hot_reload(&self, full: bool) -> Result<HotReloadResult, DebugError>;
    pub async fn wait(&self) -> SessionOutcome;
    pub fn logs(&self) -> logs::LogView;
    pub fn jit_stats(&self) -> Option<jit::JitStats>;
}

pub enum SessionState { Preparing, Building, Installing, Launching, Attaching, Arming, Ready,
                        Running, Stopped, Exited { code: i32 }, Crashed { report: Option<crash::CrashId> },
                        Detached, Lost, Failed(DebugErrorKind) }

pub enum SessionEvent { State(SessionState), Output(logs::LogEntry), Jit(jit::JitStats),
                        Crash(crash::CrashSummary), Progress { step: String, fraction: f32 }, Warning(DebugWarning) }

pub mod launch { pub trait Launcher { async fn launch(&self, d: &DeviceId, req: &LaunchSpec) -> Result<Launched, DebugError>; } }
pub mod broker { pub struct Broker; impl Broker {
    pub async fn connect(dev: &dyn DeviceServices, d: &DeviceId) -> Result<Self, DebugError>;
    pub async fn attach(&mut self, pid: u32) -> Result<(), DebugError>;
    pub async fn arm_jit(&mut self, hook: Box<dyn jit::JitHook>) -> Result<(), DebugError>;
    pub async fn serve(self, endpoint: BrokerEndpoint, policy: BrokerPolicy) -> Result<BrokerHandle, DebugError>; } }
pub mod jit {
    pub trait JitHook: Send { fn name(&self) -> &'static str;
        async fn arm(&mut self, gdb: &mut GdbSession) -> Result<Vec<u64>, DebugError>;        // breakpoint addrs
        async fn service(&mut self, gdb: &mut GdbSession, stop: &StopReply) -> Result<Serviced, DebugError>; }
    pub struct FlutterRxPages { pub framework: std::path::PathBuf, pub mode: PipelineMode }
    pub fn touch_plan(base: u64, size: u64, page: u64) -> Vec<Vec<u8>>;   // pure, unit-tested (section 23.1)
    pub struct JitStats { pub stops: u64, pub pages: u64, pub bytes: u64, pub p50_ms: f32, pub p95_ms: f32, pub rtt_ms: f32 }
}
pub mod lldb {
    /// Built from core's resolver output (russet_toolchain::ToolInfo for Lldb, LldbDap, LibPython).
    pub struct LldbToolchain { pub flavor: DebuggerFlavor, pub lldb: russet_toolchain::ToolInfo,
                               pub lldb_dap: russet_toolchain::ToolInfo, pub version: String, pub has_remote_ios: bool }
    pub async fn from_resolver(tc: &russet_toolchain::Toolchains, pref: LldbPreference) -> Result<LldbToolchain, DebugError>;
    pub async fn ensure_overlay(tc: &LldbToolchain, sdk: &SdkInfo, dirs: &RussetDirs) -> Result<Overlay, DebugError>; // 8.3
    pub fn child_env(tc: &LldbToolchain, ov: Option<&Overlay>) -> Vec<(String, String)>;   // ToolInfo.env + overlay additions
    pub fn attach_commands(plan: &AttachPlan) -> Vec<String>;            // section 9.1, Python-free
}
pub mod dap { pub struct DapHub; impl DapHub {
    pub async fn start(session: SessionHandle, backend: DapBackend, endpoint: DapEndpoint) -> Result<Self, DebugError>;
    pub async fn client(&self, role: ClientRole) -> DapClient;   // for MCP tools (in-process)
} }
pub mod symbols { pub struct SymbolCache; impl SymbolCache {
    pub async fn plan(&self, d: &DeviceId) -> Result<FetchPlan, DebugError>;
    pub async fn fetch(&self, d: &DeviceId, tier: SymbolTier, confirm_bytes: u64) -> Result<SymbolSet, DebugError>;
    pub fn lookup(&self, build: &str, dsc_uuid: &str) -> Option<SymbolSet>;
    pub async fn prune(&self, max_bytes: u64) -> Result<u64, DebugError>; } }
pub mod crash { pub async fn collect(dev: &dyn DeviceServices, d: &DeviceId, filter: CrashFilter) -> Result<Vec<CrashId>, DebugError>;
                pub async fn symbolicate(id: &CrashId, syms: &symbols::SymbolCache) -> Result<SymbolicatedCrash, DebugError>; }
pub mod logs { pub struct LogEntry { pub ts: Timestamp, pub source: LogSource, pub level: LogLevel, pub pid: Option<u32>,
                                     pub subsystem: Option<String>, pub category: Option<String>, pub text: String } }
```

### 17.3 Interfaces consumed from other areas

These must be reconciled with the device, build and agents designs.
- Under core's layering rules, `russet-debug` (L3) may depend on `russet-device` and `russet-build`
  (edges `device → debug`, `build → debug`) and on L0–L2 (`russet-core`, `russet-protocol`,
  `russet-toolchain`, `russet-config`). It may not depend on `russet-ipc`.
- `russet-gdbremote` is a new dependency-free utility crate (MIT OR Apache-2.0). It should be placed at
  L2 or vendored as an external crate; this is a note for core.
- The trait below is the *shape* `russet-debug` needs. In code it can be the concrete `russet-device`
  API.

```rust
#[async_trait::async_trait]
pub trait DeviceServices: Send + Sync {               // device-plane crate (russet-device, idevice-based)
    fn info(&self, d: &DeviceId) -> Result<DeviceInfo, DeviceError>;   // product_type, product_version, build_version, transport
    async fn connect_rsd(&self, d: &DeviceId, service: &str) -> Result<Box<dyn AsyncStream>, DeviceError>;
    async fn connect_lockdown(&self, d: &DeviceId, service: &str) -> Result<Box<dyn AsyncStream>, DeviceError>;
    async fn connect_device_port(&self, d: &DeviceId, port: u16) -> Result<Box<dyn AsyncStream>, DeviceError>; // usbmux / tunnel
    async fn app_service(&self, d: &DeviceId) -> Result<Box<dyn AppService>, DeviceError>;   // launch/list/kill/monitor
    async fn dvt(&self, d: &DeviceId) -> Result<Box<dyn DvtProcessControl>, DeviceError>;
    async fn installed_app(&self, d: &DeviceId, bundle_id: &str) -> Result<InstalledApp, DeviceError>;
    async fn heartbeat(&self, d: &DeviceId) -> Result<HeartbeatGuard, DeviceError>;          // Wi-Fi
}
pub trait BuildArtifacts: Send + Sync {                // build area
    fn product(&self, app: &AppRef) -> Result<BuiltApp, BuildError>;   // .app, exe, uuids, dsyms, sdk path, swift module dirs, flutter framework path
}
```

idevice `master` (2026-10-08; latest release 0.1.68, API unstable until 0.2.0 [R dv-29]) already has
`debug_proxy`, `core_device::{app_service, openstdiosocket}`, `dvt::process_control`, `os_trace_relay`,
`syslog_relay`, `crashreportcopymobile` and `core_device_proxy` with an in-process TCP stack
[S jkcoxson/idevice source tree]. It has **no** fetchsymbols client: Russet implements both
symbol services (11.2) and offers them upstream. idevice's `DebugProxyClient` reads byte by byte and has
no async stop handling, so Russet uses only the raw stream from it.

---

## 18. JSON-RPC methods (russetd)

> **Superseded (ARCHITECTURE §8.5, §9.5, §20.1):** sessions are `run.*` (`run.start`, `run.stop`,
> `run.relaunch`, `run.list`, `run.subscribe`, `run.output.query`, `run.hotReload`), Flutter
> registration is `flutter.register`, `LogEntry` uses `message` and the level `default` (never
> `text`/`notice`), `LogSource` includes `syslog`, notifications are `run.state/output/exited/jit`, and
> `JitStats` carries `pagesNotified`/`pagesTouched`. This table is kept for its rationale.

Conventions come from [core.md](core.md) sections 6–7:
- JSON-RPC 2.0 with LSP framing;
- wire types in `russet-protocol` (schemars + ts-rs);
- permission classes `read` / `device` / `build`;
- long methods run as jobs (`jobId`, `job.subscribe`);
- subscriptions return a `subscriptionId`;
- confirmations go through `ui.confirm`;
- debug and run sessions are **detached leases** created by the IDE, so a window reload keeps them
  (core 6.5).

This area owns the `debug.*`, `flutter.*` and `crash.*` namespaces (core's area table). It *consumes* the
device area's `app.launch`, `app.kill`, `app.processes`, `forward.open/close`, `logs.subscribe`, and the
core `toolchain.resolve`.

| Method | Perm | Params | Result |
|---|---|---|---|
| `debug.run` | device + build | `{ app: AppRef, udid?, configuration?: "Debug"\|"Release"\|"Profile", args?: string[], env?: Record<string,string>, launch?: "auto"\|"coredevice"\|"dvt", build?: boolean, install?: "ifChanged"\|"always"\|"never" }` | job → `{ sessionId, pid, bundleId, outputSubscriptionId }`. Orchestrated run without a debugger (4.1). Proposed addition to core's namespace table. |
| `debug.prepare` | device + build | `debug.run` params & `{ stopOnEntry?: boolean, frontend?: "dap"\|"lldb-cli"\|"none", jit?: "auto"\|"native"\|"lldb-python"\|"off", symbols?: "auto"\|"none"\|"core"\|"full", flavor?: "swift"\|"llvm" }` | job → `{ sessionId, pid, lldb: ToolInfo, lldbDap: ToolInfo, broker: { transport: "unix"\|"tcp", path?, host?, port?, lldbUrl }, initCommands: string[], attachCommands: string[], dapConfig: object, dap?: { transport: "unix"\|"pipe", path } }`. This refines core's sketch: `broker` replaces `debugserverPort`, and `dap` is the hub endpoint (9.3). |
| `debug.attach` | device | `{ udid?, pid?: number, bundleId?: string, frontend? }` | as `debug.prepare` |
| `debug.stop` | device | `{ sessionId, mode?: "kill"\|"detach" }` | `{}` |
| `debug.relaunch` | device + build | `{ sessionId }` | job → `{ pid, ms }` (Swift Quick Relaunch, 4.7) |
| `debug.sessions` | read | `{}` | `{ sessions: { sessionId, kind: "run"\|"debug", udid, bundleId, pid, state, startedAt, leaseOwner }[] }` |
| `debug.output.subscribe` | read | `{ sessionId, filter?: { minLevel?, source?: LogSource[], subsystem?, category?, regex? }, history?: number }` | `{ subscriptionId }`, then notifications `debug.output { subscriptionId, entries: LogEntry[], dropped? }`. A merged session view over the device area's console and os_trace sources (section 12). |
| `debug.output.query` | read | `{ sessionId, sinceMs?, limit?, filter? }` | `{ entries: LogEntry[], dropped: number }` |
| `debug.jitStats` | read | `{ sessionId }` | `JitStats` |
| `debug.symbols.status` | read | `{ udid? }` | `{ build, dscUUID?, tier: 0\|1\|2, bytesOnDisk, rawDscPresent: boolean }` |
| `debug.symbols.plan` | read | `{ udid? }` | `{ files: { path, size }[], totalBytes, freeBytes, quotaBytes, ok: boolean, reason? }` |
| `debug.symbols.fetch` | device | `{ udid?, tier: "core"\|"full", confirmBytes?: number, keepRawDsc?: boolean }` | job (progress in bytes). Interactive clients get `ui.confirm` with the size. Headless and MCP callers must pass `confirmBytes` equal to the planned total (N3). |
| `debug.symbols.prune` | read | `{ maxBytes?: number, build?: string }` | `{ freedBytes }` |
| `crash.list` | read | `{ bundleId?: string, since?: string }` | `{ crashes: CrashSummary[] }` (local store) |
| `crash.fetch` | device | `{ udid?, bundleId?, since? }` | job → `{ fetched: CrashSummary[] }` (mover flush + AFC pull, 13) |
| `crash.symbolicate` | read | `{ crashId, format?: "json"\|"text" }` | `{ crash: SymbolicatedCrash \| string }` |
| `flutter.run` | device + build | `{ project, udid?, mode?: "debug"\|"profile"\|"release", args?: string[] }` | job → `{ sessionId, vmService: { wsUri, httpUri }, devtoolsUri? }` (headless `flutter run --machine`, 14.4) |
| `flutter.attach` | device | `{ sessionId }` | `{ vmService }` |
| `flutter.reload` | device | `{ sessionId, full?: boolean }` | `{ ms, kind: "reload"\|"restart" }` |
| `flutter.devices.register` | read | `{ all?: boolean }` | `{ registered: string[] }` |

Notifications:
- `debug.state { sessionId, state, detail? }`
- `debug.output { subscriptionId, entries, dropped? }`
- `debug.exited { sessionId, code?, signal?, crashId? }`
- `debug.jit { sessionId, ...JitStats }`

Job progress uses core's `job.*` notifications.

Shared types (in `russet-protocol`):
- `LogSource = "stdout"|"stderr"|"oslog"|"debugger"|"jit"|"russet"`
- `LogEntry = { ts: string, source: LogSource, level: "debug"|"info"|"notice"|"error"|"fault", pid?: number, process?: string, subsystem?: string, category?: string, text: string }`
- `CrashSummary = { crashId, bundleId, process, timestamp, exception: string, signal?: string, symbolicated: boolean }`
- `JitStats = { stops, pages, bytes, p50Ms, p95Ms, rttMs, mode: "safe"|"eager" }`

---

## 19. MCP tools (contracts for the agents area)

Each tool maps one-to-one onto JSON-RPC (section 18) or the DAP hub (`client(role=Agent)`). Input schemas
(JSON Schema, abridged):

```json
[
 {"name": "russet_run_app", "description": "Build, install and launch the app on the iPhone; returns a session id. Logs via russet_app_logs.",
  "inputSchema": {"type": "object", "properties": {"device": {"type": "string"}, "configuration": {"enum": ["Debug", "Release", "Profile"]},
   "args": {"type": "array", "items": {"type": "string"}}, "env": {"type": "object", "additionalProperties": {"type": "string"}}}}},
 {"name": "russet_debug_start", "description": "Start a debug session with optional breakpoints; returns when the app is running or stopped.",
  "inputSchema": {"type": "object", "properties": {"device": {"type": "string"}, "stopOnEntry": {"type": "boolean"},
   "breakpoints": {"type": "array", "items": {"type": "object", "required": ["file", "line"],
     "properties": {"file": {"type": "string"}, "line": {"type": "integer"}, "condition": {"type": "string"}}}}}}},
 {"name": "russet_debug_wait_for_stop", "inputSchema": {"type": "object", "required": ["sessionId"],
  "properties": {"sessionId": {"type": "string"}, "timeoutMs": {"type": "integer", "default": 30000}}}},
 {"name": "russet_debug_backtrace", "inputSchema": {"type": "object", "required": ["sessionId"],
  "properties": {"sessionId": {"type": "string"}, "threadId": {"type": "integer"}, "maxFrames": {"type": "integer", "default": 32}}}},
 {"name": "russet_debug_variables", "inputSchema": {"type": "object", "required": ["sessionId"],
  "properties": {"sessionId": {"type": "string"}, "frameId": {"type": "integer"}, "depth": {"type": "integer", "default": 1}}}},
 {"name": "russet_debug_evaluate", "description": "Evaluate an expression (po/expr) or an LLDB command prefixed with a backtick in the selected frame.",
  "inputSchema": {"type": "object", "required": ["sessionId", "expression"],
  "properties": {"sessionId": {"type": "string"}, "expression": {"type": "string"}, "frameId": {"type": "integer"}}}},
 {"name": "russet_debug_control", "inputSchema": {"type": "object", "required": ["sessionId", "action"],
  "properties": {"sessionId": {"type": "string"}, "action": {"enum": ["continue", "pause", "stepOver", "stepIn", "stepOut"]}, "threadId": {"type": "integer"}}}},
 {"name": "russet_debug_set_breakpoints", "inputSchema": {"type": "object", "required": ["sessionId", "file", "lines"],
  "properties": {"sessionId": {"type": "string"}, "file": {"type": "string"}, "lines": {"type": "array", "items": {"type": "integer"}}}}},
 {"name": "russet_debug_stop", "inputSchema": {"type": "object", "required": ["sessionId"],
  "properties": {"sessionId": {"type": "string"}, "mode": {"enum": ["kill", "detach"], "default": "kill"}}}},
 {"name": "russet_app_logs", "inputSchema": {"type": "object", "properties": {"sessionId": {"type": "string"}, "bundleId": {"type": "string"},
  "sinceMs": {"type": "integer"}, "limit": {"type": "integer", "default": 200}, "minLevel": {"enum": ["debug", "info", "notice", "error", "fault"]}, "regex": {"type": "string"}}}},
 {"name": "russet_crash_reports", "inputSchema": {"type": "object", "properties": {"bundleId": {"type": "string"}, "since": {"type": "string"},
  "symbolicate": {"type": "boolean", "default": true}, "limit": {"type": "integer", "default": 5}}}},
 {"name": "russet_hot_reload", "inputSchema": {"type": "object", "required": ["sessionId"],
  "properties": {"sessionId": {"type": "string"}, "full": {"type": "boolean", "default": false}}}}
]
```

Agent guard rails:
- Results are truncated at 16 KB, with `truncated: true`.
- Agents cannot detach from Flutter sessions (that would freeze the app, 4.6).
- Execution-control tools return the resulting `stopped` / `continued` event so agents do not have to poll.

---

## 20. CLI

```text
russet run      [--device D] [--configuration C] [--no-build] [--install ifChanged|always|never]
                [--launch auto|coredevice|dvt] [--env K=V]... [--json] [-- app args...]
russet debug    [--device D] [--break FILE:LINE]... [--stop-on-entry] [--attach [--pid N|--bundle B]]
                [--jit auto|native|lldb-python|off] [--symbols auto|none|core|full] [--print-config]
                [--lldb PATH] [-- app args...]          # interactive: spawns `lldb -s <session>/attach.lldb`
russet debug prepare --json ...                         # pre-steps only; prints PreparedDebug JSON; keeps the session
russet debug jit-bench [--device D] [--iterations N]
russet logs     [--device D] [--session S | --bundle B | --pid N] [--level L] [--subsystem S] [--grep RE] [--json]   # --session: debug.output; else device-area logs.*
russet crash    list|show|symbolicate [--bundle B] [--since T] [ID]
russet symbols  status|plan|fetch --tier core|full --yes-bytes N|prune [--max-bytes N]
russet flutter  register | device ping|install|uninstall|run-debug|forward|screenshot ...   # registration/install: build.md 6.3
russet doctor   debug                                   # LLDB, python, overlay, broker self-test against lldb-server
```

Exit codes follow [core.md](core.md) section 13.3:
- `9`: launch or debug failed (`debug` family and `app.launchFailed`)
- `6`: device state
- `4`: environment (missing LLDB or libpython, disk full)
- `7`: build failed

`russet run` and `russet debug` additionally return the app's exit status once the app has run, mapped as
`128 + signal` for signal exits, so scripts can detect crashes.

---

## 21. Files and locations

Directory roles are core's `RussetDirs` ([core.md](core.md) section 9.5): `runtime`, `state`, `cache`,
`data`.

| Item | Linux | Windows |
|---|---|---|
| Broker and DAP sockets | `runtime/sessions/<sid>/{gdb.sock,dap.sock}`, i.e. `/run/user/<uid>/russet/sessions/…` (0700/0600) | loopback TCP (broker), `\\.\pipe\russet-dap-<sid>` (hub) |
| Session registry (crash cleanup) | `state/sessions.json` (core 6.1) | `%LOCALAPPDATA%\Russet\state\sessions.json` |
| Session record and logs | `state/sessions/<sid>/{session.json,output.log,oslog.jsonl}` (last 20 kept) | same under `state` |
| Generated attach script | `<project>/.russet/debug/<sid>/attach.lldb` (project-local, gitignored; core 9.5) | same |
| Build symbol index | `<project>/.russet/builds/<build-id>/symbols.json` (written by the build area) | same |
| dSYM archive | `data/dsyms/<UUID>.dSYM` | `%LOCALAPPDATA%\Russet\data\dsyms` |
| Crash store | `state/crashes/<bundleId>/` | `%LOCALAPPDATA%\Russet\state\crashes` |
| Device symbols | `cache/device-support/<build>/<dsc-uuid>/` | `%LOCALAPPDATA%\Russet\cache\device-support` |
| LLDB module cache | `cache/lldb-module-cache/` | `%LOCALAPPDATA%\Russet\cache\lldb-module-cache` |
| Provisioned Python (resolver `python-shared`) | `data/toolchains/python-shared/cpython-<ver>/` (core's toolchain layout) | not needed |
| LLDB overlays | `data/lldb-overlays/<swift>-<sdk>/` | `%LOCALAPPDATA%\Russet\data\lldb-overlays` |
| Pins | core's pin manifest: PBS URL + SHA-256 per platform, ipsw version + SHA-256, minimum LLDB versions | same |

`session.json` schema:

```json
{ "schema": 1, "sessionId": "ses_7k2…", "kind": "debug", "device": {"udid": "…", "productType": "iPhone13,2", "productVersion": "27.0", "buildVersion": "24A437", "transport": "usb"},
  "app": {"bundleId": "…", "devicePath": "/private/var/containers/Bundle/Application/…/Hello.app", "executable": "Hello", "uuid": "4C4C44A9-…"},
  "pid": 1234, "launch": "coredevice", "broker": {"lldbUrl": "unix-connect:///run/user/1000/russet/sessions/ses_7k2/gdb.sock"},
  "jit": {"hook": "FlutterRxPages", "notifyAddr": "0x10a744174"}, "lldb": {"flavor": "swift", "version": "21.0.0", "python": "pbs-3.12.15"},
  "startedAt": "2026-10-11T00:00:00Z", "endedAt": null, "outcome": null }
```

`.gitignore`: the repository ignores `.orchard/`. It should also ignore `.russet/` (rename leftover; for
the lead).

---

## 22. Error catalogue

- Errors use core's `russet_core::Error`: a code, a message, a hint, optional details and a retryable
  flag ([core.md](core.md) section 13).
- This area owns the **`debug` family, 6000–6099**.
- Device-state failures reuse the device family (3000–3299, e.g. 3002 locked, 3004
  developerModeDisabled, 3005 ddiNotMounted, 3100 app.notInstalled, 3102 app.launchFailed).
- Toolchain failures reuse 2000–2002: missing, versionMismatch, broken (for example missing libpython).
- `details.deviceMessage` always carries the raw device or debugserver text.

| Code | Name | When | Hint |
|---|---|---|---|
| 6000 | `debug.launchDenied` | signature, entitlement or profile rejected at launch | check the signing panel |
| 6001 | `debug.profileUntrusted` | free account, developer not trusted | Settings → General → VPN & Device Management |
| 6002 | `debug.attachDenied` | `vAttach` → E.., no `get-task-allow` (release/distribution signing) | use a Debug configuration with a development profile |
| 6003 | `debug.alreadyDebugged` | an orphaned debug session holds the process | Russet kills orphans and retries once, then this error |
| 6004 | `debug.debugserverUnavailable` | the `debugproxy` service is missing from RSD | mount the DDI (device doc) |
| 6005 | `debug.uuidMismatch` | the installed binary differs from the local build | reinstall |
| 6006 | `debug.lldbUnsupported` | the LLDB found lacks `remote-ios` | `russet doctor` |
| 6007 | `debug.handoffTimeout` | no LLDB connected to the broker within 60 s | — |
| 6008 | `debug.jitHookNotFound` | no `NOTIFY_DEBUGGER_ABOUT_RX_PAGES` in a debug engine on iOS ≥ 26 | upgrade Flutter (minimum: open question 1) |
| 6009 | `debug.jitArmingFailed` | main/Flutter image not found before VM init | — |
| 6010 | `debug.processExited` | the app died before or during attach | see `crash.list` |
| 6011 | `debug.symbolsNoSpace` | free space or quota too small (wraps `core.diskFull` 1004) | prune or raise the quota |
| 6012 | `debug.symbolsService` | `remoteFetchSymbols` missing or failed | — |
| 6013 | `debug.brokerLost` | upstream EOF during a session (tunnel or device gone) | reconnect |
| 6014 | `debug.sessionNotFound` | unknown `sessionId` | — |
| 6015 | `debug.overlayUnavailable` | Swift expression overlay could not be built (no reflink, user declined the copy) | `po` falls back to `frame variable` |

Warnings are events, not errors:
- `debug.warn.jitInterpreted`: `Dart execution mode: simulator`.
- `debug.warn.jitUnsupportedEngine`: a debug build without the hook on iOS ≥ 26.
- `debug.warn.slowLink`: RTT > 40 ms; the hint is to prefer USB.

---

## 23. Test plan

### 23.1 Unit tests (CI, every platform)

- **Codec:** checksums, `}` escapes, `*` RLE, binary `x` replies, partial frames, interrupts.
  Golden packets are captured from `lldb-server` and from the device (task D3 records a debugserver
  transcript; no Apple files).
- **Stop parser:** debugserver-style T packets with expedited registers, `jstopinfo` hex JSON, `metype`/`medata`.
- **`touch_plan`:** n = 1…64 pages, aligned and unaligned base and size. Assert:
  - every overlapping page is covered;
  - no byte outside `[base, base+size)` is written;
  - for aligned input, write count = 1 + ⌈(n−1)/2⌉.
- **`is_plain_return`** decoder.
- **Interposer hooks:** scrubbing `jThreadsInfo`, the re-resume decision table (6.5).
- **`.ips` parser and symbolizer:** fixtures from our own test-app crashes (no Apple files committed;
  synthetic system frames).
- **LLDB command generator:** golden `attach.lldb`; no `script` lines allowed.

### 23.2 Host-only integration tests (CI on Linux; no iPhone)

| Test | Setup | Pass criteria |
|---|---|---|
| H1 broker handoff (sync) | `lldb-server gdbserver` (Swift toolchain), broker `vRun`/`vAttach`, then `lldb -b -o 'process connect …'` over a Unix socket and TCP | the connect returns in < 2 s with a stopped process [V4] |
| H2 broker handoff (lldb-dap) | as H1, driven by a DAP test client (initialize → attach with attachCommands → configurationDone) | `attach` success; `process` event; `exited` after the run [V4] |
| H3 JIT servicer x86_64 | `jitdemo` fixture (C, in-repo) under `lldb-server`; Rust servicer | `RESULT PASS`, every page touched (verify mode), no step-over packets [V8] |
| H4 JIT servicer arm64 | same on `ubuntu-24.04-arm` runners | identical; return emulation is `pc=lr` (the iOS path) |
| H5 interposer | H3 + an LLDB client connected through the interposer, with a user breakpoint in `main` and stepping | the user breakpoint hits, JIT stops never reach LLDB, `RESULT PASS` |
| H6 fake debugserver | Rust re-implementation of the fake iOS stub (from V5/V6), in `russet-testkit` | LLDB binds the executable by UUID; `frame variable` works; the arming sequence (7.4) runs against a synthetic image list |
| H7 Python-free sequence | (a) static lint: generated command lists contain no `script`, `command script` or `breakpoint command add -s python`; (b) H2 with `log enable lldb script`; (c) from RD-18 on: H2 on a Windows runner with the LLVM MSI LLDB and no Python installed | (a) clean; (b) no script-interpreter activity after start-up; (c) passes |
| H8 TXM-model fake debugserver | Rust, in `russet-testkit` (extends H6). Pages become executable only after a client `M`/`X` write overlaps them, page-rounded like debugserver's protect. Scripted Dart-like allocation sequences: a 1-page test region, 512 KiB regions, 1/2/31/32/33 pages, unaligned input, two threads stopping on the hook at once, hook stops during eager-mode writes. After each resume the fake "executes" chosen pages and answers with a `T` stop whose `metype` is `EXC_BAD_ACCESS` for any unassociated page. Negative tests: skip a page, miss a stop, an `E` reply to an `M` in eager mode, a `P20` failure | no faults in the normal run; every injected fault is reported as `debug.jitPageMissed`; a property test cross-checks `touch_plan` against the model |

### 23.3 Device tests (device agent / owner; matrix: iPhone 12 iOS 27.0 (TXM, firmware-verified) now; iOS 26.x on A15+ and 27.x betas when available; USB first, then Wi-Fi)

Runs on the virtual-iPhone track (a separate private project) are not evidence for retail-device JIT
behaviour, because its security configuration differs.

| ID | Scenario | Pass |
|---|---|---|
| T1 | `russet run` SwiftUI Hello: stdout and os_log visible, exit code reported | lines appear in under 1 s |
| T2 | `russet debug` + breakpoint in `ContentView.body`, CLI | stops; `bt` shows app frames; `continue`; `kill` |
| T3 | IDE F5 (hub + lldb-dap) | the IDE stops at the breakpoint; variables render; stepping works |
| T4 | early breakpoint in `HelloApp.init` | hits (proves the pre-dyld attach) |
| T5 | crash button in the test app | `.ips` fetched, app frames symbolicated with file and line |
| T6 | Flutter counter app, iOS 27 | stdout shows `Dart execution mode: JIT`; hot reload ≤ 1 s; ≥ 20 reloads without a hang; no `EXC_BAD_ACCESS`/`SIGBUS` over a 10-minute soak that visits every route of a gallery-style app; `pagesTouched == pagesNotified` |
| T7 | Flutter + native compound session | a breakpoint in a Swift plugin hits while hot reload keeps working |
| T8 | Wi-Fi T2 + T6 | pass with Wi-Fi timeouts |
| T9 | perf: attach time, JIT p50/p95, symbol tiers | recorded in `docs/verified-facts.md` |
| T10 | RussetJitProbe, every page plus a negative control (the device half of DC-30): code runs in every page of 1, 2, 31, 32, 33 and 2048-page regions, an unaligned region, and two threads announcing at once; a region that is never announced must fault | all pages run; the control faults with `EXC_BAD_ACCESS`; `security.codesigning.monitor` logged if readable |

### 23.4 Verification tasks for open assumptions (device agent; read-only where possible)

| ID | Experiment | Decides |
|---|---|---|
| D1 | CoreDevice `launchapplication{startStopped:true}` + `openstdiosocket` on iOS 27: pid returned; stdout bytes arrive after resume | default launch strategy (5.1) |
| D2 | launch suspended, **close** the launch channel, wait 10 s, attach with `qThreadStopInfo`/`jGetDyldProcessState`. Has the process run? | the keep-open rule (4.2) |
| D3 | broker pre-attach on the device + `lldb -b` sync `process connect` + breakpoint, with no `script` commands. Record the packet transcript (`log enable gdb-remote packets`) | the core of this design |
| D4 | as D3 without search paths: is the binding by UUID alone enough? | the 9.1 line set |
| D5 | `jGetLoadedDynamicLibrariesInfos:{"fetch_all_solibs":true}` right after `vAttach` on a suspended launch: main and dyld reported? | the arming step (7.4.1) |
| D6 | env `OS_ACTIVITY_DT_MODE=YES`: are `Logger`/`os_log` lines (including `%{private}`) mirrored to the stdio socket? | the 5.3 default |
| D7 | `po` matrix (overlay on/off × `memory-module-load-level` minimal/partial/complete × core symbols): `po 1+2`, `po self`, `expr items.count` | 10.2 defaults |
| D8 | `remoteFetchSymbols` **listing only** (no download): file count and sizes for iOS 27.0 iPhone13,2 | 11.1 tier sizes, quota default |
| D9 | `vAttach` to a Release-signed build | the `debug.attachDenied` message format |
| D10 | Flutter 3.47.x engine: disassemble the first instruction of `NOTIFY_DEBUGGER_ABOUT_RX_PAGES` (host-side, from the downloaded engine artifact) | return emulation (7.5) |
| D11 | kill russetd mid-session, once with `QSetDetachOnError:0` and once with `:1`: is the app killed, detached and running, or left stopped? Also measure what happens to a detached Flutter debug app. | 4.6 last row |
| D12 | Windows: Swift for Windows `lldb.exe` has `remote-ios`; `lldb-dap.exe` starts without Python; long paths | 8.1, 16 |
| D13 | Research lead, unverified (lane B, read-only): Xcode 27 can download device symbols. DVTFoundation's `DVTDeviceSymbolsCoordinator` lists `https://devimages-cdn.apple.com/downloads/xcode/dsc/`, `https://download.developer.apple.com/Developer_Tools/` and MobileAsset, driven by `xcodebuild -prepareDeviceSupport -platform -osVersion -buildVersion -modelCode -architecture`; the URL scheme is unknown and the base paths are not listable | whether 11.2 can fetch symbols from Apple instead of the device (ARCHITECTURE §21 Q13) |

---

## 24. Ordered task list

| ID | Task | Size | Depends on | Acceptance |
|---|---|---|---|---|
| RD-01 | `russet-gdbremote`: codec, stop parser, `GdbClient` (handshake, request, pipeline, attach, resume, wait_stop, memory, breakpoints, registers, images) | M | — | unit tests 23.1 green; H1 passes with the Rust client doing `vRun`/`vAttach` against `lldb-server` |
| RD-02 | Test harnesses: port `run-debug-prototypes/` into `russet-testkit` (the fake gdb-remote server and the mock device's scripted debugserver, [qa-delivery.md](qa-delivery.md) 3.5.5): `jitdemo.c` fixture, DAP test client, Rust fake iOS debugserver with the V5/V6 behaviour, `lldb-server` fixture launcher; CI jobs (x86_64 + arm64) | M | RD-01, QA testkit skeleton | H1–H3 and H6 run in CI in < 3 min; the prototypes directory is deleted |
| RD-03 | Broker v1: pre-attach, relay with per-side ack state, Unix socket/TCP endpoints, one-client handoff, detach/kill policy, orphan handling | M | RD-01 | H1/H2 green; D3 green on the iPhone |
| RD-04 | Launch subsystem: CoreDevice launch + stdio socket, DVT fallback, keep-open guard, app resolution (`XTL-`), env defaults | M | device plane | D1/D2 answered; T1 green |
| RD-05 | LLDB toolchain rules inside core's resolver (with CORE-10): Swift/LLVM flavor selection, `remote-ios` probe, `python-shared` component sourced from pinned python-build-standalone (SHA-256), `ToolInfo.env`; `russet-debug::lldb::from_resolver` + `child_env`; doctor checks `toolchain.lldb`/`toolchain.lldb-dap` | M | CORE-10 | on this host `russet doctor` reports swift lldb 21.0.0 with pbs 3.12.15 provisioned by Russet (not `~/.local/opt/python-3.12`); `lldb-dap` starts from Russet with no manual env |
| RD-06 | Attach-plan generator + `russet debug` CLI (interactive lldb) + `debug.prepare` JSON-RPC | M | RD-03, RD-04, RD-05 | T2 green; the generated scripts contain no `script` commands |
| RD-07 | DAP hub v1: single client (IDE), `lldb-dap` backend, breakpoint cache, `russet/*` custom requests skeleton | M | RD-06 | T3 green from the Russet IDE (F5) |
| RD-08 | IDE extension: `russet` debug type, config provider calling `debug.prepare` with progress, buttons (Run/Debug/Hot Reload), Attach-to-process | M | RD-07 | F5 on the SwiftUI sample stops at a breakpoint (M3 exit criterion) |
| RD-09 | Session output: merged view over the device area's console + os_trace StreamHub sources, de-dup, filters, `debug.output.*` methods, persistence | M | RD-04, device-area `logs.*` | T1 and D6; 10 000 lines/s burst handled without UI stalls |
| RD-10 | JIT servicer (native): hook discovery for any exporting image (7.3), arming (7.4), servicing loop (7.5) with `safe`/`eager` modes, signal/exception handling, `jit.stats` with `pagesNotified`/`pagesTouched`, `debug.jitPageMissed` | L | RD-01, RD-03 | H3/H4/H8 green; T6 and T10 green on iOS 27 on the rig (DC-30, PLAN §11) |
| RD-11 | Flutter run/debug half: `run-debug` behaviour (launch suspended, broker, JIT, stdout passthrough, crash capture), `forward` over `forward.open`, VM-service discovery fallbacks, `flutter.run`/`flutter.reload` (`flutter run --machine` driver). Registration and `install` come from build.md B13. | L | RD-10, B13 | `flutter run -d russet-<udid>` works with hot reload and DevTools (M2 exit) |
| RD-12 | Symbols: `remoteFetchSymbols` + legacy fetchsymbols clients, plan/consent/quota, `ipsw` extraction of the core set, tier-0 memory index, `--sysroot` wiring | L | RD-01, RD-03 | D8 answered; after a tier-1 fetch, LLDB `bt` shows named UIKit/libswiftCore frames; quota and prune enforced |
| RD-13 | Crash pipeline: live snapshot (broker), mover flush + AFC pull, `.ips` parser, symbolizer (`symbolic`), store, `crash.*` methods, IDE crash panel data | L | RD-12 (system frames), build symbol index | T5 green with file:line for app frames |
| RD-14 | DAP hub v2: multi-client (agent clients), seq rewriting, backend swap on relaunch, Flutter DAP backend, MCP tool bindings (19) | L | RD-07, RD-11 | an agent can set a breakpoint, wait for the stop, read variables and continue while the IDE shows the same session |
| RD-15 | Interposer mode (6.5) for compound Flutter + native sessions | L | RD-10, RD-03 | H5 green; T7 green |
| RD-16 | Swift expressions: overlay builder (reflink/copy), settings, build-side requirements filed with the build area, D7 experiment, defaults | M | RD-05, RD-12 | `po` status documented for iOS 27 (M3 criterion); `po self` works in the SwiftUI sample, or the remaining blocker is named with a fix plan |
| RD-17 | Wi-Fi profile: timeouts, heartbeat, VM-service host, watchdog, slow-link warnings | M | RD-04, RD-10, device-plane Wi-Fi | T8 green |
| RD-18 | Windows: TCP broker, named-pipe hub, LLDB discovery (Swift for Windows/LLVM MSI), paths, D12 | L | RD-03…RD-11 | T2/T3/T6 green on the physical Windows PC (M6) |
| RD-19 | Perf suite and `russet debug jit-bench`; choose `memory-module-load-level` defaults; record timings | M | RD-10, RD-12 | N4 targets met or gaps filed; numbers added to verified-facts |
| RD-20 | `--jit-servicer=lldb-python` fallback (headless sync lldb + vendored Flutter helper with sparse writes + iOS 27 manual continue) | S | RD-05, RD-06 | T6 passes in fallback mode (slower) |
| RD-21 | Upstream: LLDB (Swift resource-dir setting; SDK lookup via `target.sdk-path` without `HostInfoError`; DeviceSupport lookup without Xcode; on-disk DSC off macOS), swift.org (Python delay-load on Linux), idevice (fetchsymbols, debug_proxy streaming), Flutter (custom devices declaring `ios`, [R fx-27]) | M | evidence from RD-16/RD-12 | issues or PRs opened with links in `docs/research` |

**Critical path to M1 (CLI) and M3 (IDE):** RD-01 → RD-03 → RD-04 → RD-05 → RD-06 → RD-07 → RD-08.
**Critical path to M2 (Flutter):** RD-10 → RD-11.

---

## 25. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| CoreDevice launch shape differs on iOS 27.x | M | M | DVT fallback (verified [VF]); D1 |
| Closing the launch channel resumes the app early | M | L | keep it open until `vAttach` (4.2); D2 |
| On-device debugserver lacks a newer packet (`jGetDyldProcessState`, `added-binaries`) | L | L | used only as optimisations; the core relies on long-standing packets (`vAttach`, `Z0`, `M`, `P`, `jGetLoadedDynamicLibrariesInfos`) |
| The NOTIFY hook body is not a plain `ret` in some engine build | L | M | instruction check and step-over fallback (7.5); D10 |
| Interposer state bugs under heavy stepping during JIT bursts | M | M | v1 ships native mode and relay mode separately; interposer behind a flag until H5/T7 pass |
| `po` needs system symbols (multi-GB DSC) | H | M | tiered symbols, context-free `po`, honest UI fallback chain; upstream on-disk DSC support |
| libpython ABI drift (Swift 6.5 links 3.13) | M | L | soname-driven PBS provisioning; pins in `russet-toolchains.toml` |
| ipsw-extracted dylibs not accepted by LLDB (in-cache flags, UUID) | M | M | RD-12 validation; fallback to the tier-0 index plus memory loading |
| Userspace-tunnel throughput or stalls hurt JIT stops and symbol reads | M | M | pipelining, compression (phase 2), kernel tunnel option (device area) |
| AGPL/unlicensed references (StikDebug, xcross) leak into code | L | H | reference only; clean re-implementation from protocol docs and our own captures; reviewed in PRs |
| Agents disrupt the user's session through the shared hub | M | L | role rules (9.3), IDE shows agent actions in the Debug Console |
| TXM policy drift between iOS releases (the 26.x and 27.x TXM builds differ; iOS 27 adds a "platform code strict" path that can disable Developer Mode) | M | H | DC-30's negative control on each new iOS or beta; RDT traces per build; community trace submissions; doctor reports Developer Mode before JIT runs |

## 26. Open questions (for the lead or owner)

1. **Minimum Flutter version.** Which is the minimum supported Flutter for iOS 26+ JIT? Proposal: the
   oldest engine that exports the hook *and* has the iOS 27 fix (#190307) [R dv-21].
2. **Symbol downloads.** Is a one-time multi-GB device-symbol download acceptable as an opt-in, and is
   20 GiB the right default cache quota?
3. **Hot Reload on Swift projects.** Should the button be hidden or labelled "Quick Relaunch"?
4. **Agent rights.** May agents detach, or only kill?
5. **Older iOS.** Should iOS < 17 (legacy `com.apple.debugserver.DVTSecureSocketProxy`) be supported
   at all? Xcode 27 dropped it [R xr-05].
6. **Cleanup now possible.** `~/.local/opt/python-3.12` (297 MB, the custom CPython built for M1) is
   superseded by the uv/PBS 3.12.15 runtime [V1]. Proposal:
   1. switch `demo/m1-baseline.sh`'s `PY312` default to the PBS path;
   2. then delete it.

   It was not deleted in this pass: other agents may be using it right now. (Status 2026-10-11: the
   relayed message is the go-ahead; PLAN rule 8 already points at the uv path, and PLAN §14 schedules
   the deletion once the demo script does too.)
7. **App extensions.** When should debugging of extensions (widgets, share extensions) be scheduled? It
   needs CoreDevice launch of the host app plus attach-by-name to the extension process (`vAttachWait`),
   with the same broker.
8. **Swift code injection.** Should real hot reload for Swift be a research track? The idea is
   InjectionNext-style: compile the changed file into a dylib, sign it with the dev certificate, then
   `dlopen` it through an LLDB expression or a broker-driven call. Russet already signs; the risk is
   TXM/AMFI rules for newly loaded code.

## 27. Sources

Local copies of most of these are in `~/RE/ios-nomac/research/` (from the landscape run).

- **LLDB and debugserver** (llvm/llvm-project `main`, fetched 2026-10-11):
  - `lldb/tools/debugserver/source/RNBRemote.cpp`
  - `lldb/tools/lldb-dap/Handler/AttachRequestHandler.cpp`
  - `lldb/source/Target/Platform.cpp`
  - `lldb/source/Plugins/Platform/MacOSX/PlatformDarwinDevice.cpp`, `PlatformRemoteDarwinDevice.cpp`
  - lldb-dap README (`lldb/tools/lldb-dap/README.md`)
  - gdb-remote extensions: https://lldb.llvm.org/resources/lldbgdbremote.html
- **swiftlang/llvm-project** (Swift 6.4 LLDB):
  - `lldb/source/Plugins/TypeSystem/Swift/SwiftASTContext.cpp`
  - `lldb/source/Target/TargetProperties.td` (swift-use-context-free-po, swift-pcm-validation, swift-module-search-paths)
- **dyld:** https://github.com/apple-oss-distributions/dyld, `libdyld/dyld_process_info.cpp` (`makeSuspended`).
- **XNU:** https://github.com/apple-oss-distributions/xnu, `bsd/kern/code_signing/txm.c` (`txm_associate_debug_region`).
- **Dart VM:** https://github.com/dart-lang/sdk, `runtime/vm/virtual_memory_posix.cc`.
- **Flutter** (https://github.com/flutter/flutter):
  - `engine/src/flutter/runtime/ptrace_check.cc`
  - `packages/flutter_tools/lib/src/ios/lldb.dart`
  - `…/custom_devices/custom_device.dart`, `custom_device_config.dart`
  - `…/device.dart` (`getIOSLaunchArguments`)
- **pymobiledevice3 11.26** (https://github.com/doronz88/pymobiledevice3, GPL-3.0):
  - `remote/core_device/app_service.py`, `open_stdio_socket.py`
  - `services/dvt/instruments/process_control.py`
  - `services/remote_fetch_symbols.py`, `dtfetchsymbols.py`
  - `services/os_trace.py`, `crash_reports.py`
  - `cli/developer/debugserver.py`
  - `docs/guides/ios17-tunnels.md`
- **idevice** (https://github.com/jkcoxson/idevice, MIT; v0.1.68, master 2026-10-08): `idevice/src/services/{debug_proxy.rs, core_device/*, dvt/process_control.rs, os_trace_relay.rs, crashreportcopymobile.rs}`.
- **xlinux** (https://github.com/cesardev31/xlinux, GPL-3.0, v0.5.1): `README.md` ("Hard-won details"), `device_bridge.py`, `lldb_driver.py`, `flutter_lldb_helper.py`, `custom_device.py`.
- **xcross** (https://github.com/arxdeus/xcross, AGPL-3.0 from v2.0.0): `gdb_remote_client.dart`. Reference only.
- **Frida:** https://github.com/frida/frida-core, `src/fruity/debugger-mappings.vala`, `injector.vala`; commit 03d861ca80 [R].
- **StikDebug** (https://github.com/StephenDev0/StikDebug, AGPL-3.0): `universal.js`, `JITEnableContext.swift`. Reference only.
- **python-build-standalone:** https://github.com/astral-sh/python-build-standalone/releases (release 20261009).
- **ipsw** (https://github.com/blacktop/ipsw, MIT; 3.1.736): `ipsw dyld extract|a2s`, `ipsw symbolicate`.
- **Sentry symbolic:** https://github.com/getsentry/symbolic (MIT; 13.9.0).
- **Crate versions** (crates.io, 2026-10-11): object 0.40.0, gimli 0.34.0, addr2line 0.27.1, flate2 1.1.10, lz4_flex 0.14.0, tokio-util 0.7.20.
- **Landscape report records:** [dv-10] [dv-11] [dv-12] [dv-14]–[dv-21] [dv-30]–[dv-35] [fx-14] [fx-16] [fx-18]–[fx-22] [fx-24] [fx-27] [fx-38] [xr-05] [tc-03] [tc-29] [R11 risk].
