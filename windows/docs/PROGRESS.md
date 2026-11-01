# Progress and acceptance evidence

Updated 2026-10-31. This is a development experiment, not the installed product.

## Approved scope and resources

Owner approved up to **5 GB downloads and 30 GB C: storage** for IDE, Rust and
backend development dependencies on 2026-10-31. Reuse installed Visual Studio.
Later explicitly raised temporary C: storage to **45 GB**, download limit still
5 GB. The first 30 GB guard stopped builds, but a coarse allocation step briefly
exceeded that ceiling between checks (about 33.7 GB observed). Heavy builds
paused; verified rootfs/editor download archives and duplicate reims `.a` were
removed, and only Russet-Dev was stopped/compacted. New guard checks every 0.2 s
with 4.5 GB reserve below 45 GB, plus the repository 6 GiB target-slot cap.
No global WSL changes, user Ubuntu mutation or certificate trust changes.
Lane-A discovery/readiness and the existing local DDI mount have run.
`Russet-Dev` is separately imported with a non-root
`russet` account. Rootfs is 415,361,247 bytes, SHA256
`96c7f5fb28a7fe28245331f9bfbe4375f18dd29a4850116ad3c4f60f6700c55c`.
Additional acquisition: pinned extension npm lock (419 installed packages),
small PyYAML validator dependency, Microsoft packaging plugin; compiler/Mesa apt
plan is 153 MB. Check totals before the next bulk acquisition. Heavy VM storage
has not yet been imported.

The recovered Swift compiler is **6.4.1**, not the older handoff's generic 6.4.
It was copied only into the managed Linux backend. The reference Arch toolchain
loads an unversioned `libncursesw.so` (a symlink to ABI 6 on its source host);
Ubuntu's unversioned name is a linker script. Adaptation changes the copied ELF
dependencies to the same `libncursesw.so.6` ABI, without replacing system libraries
or touching the Linux reference. Compiler version invocation succeeds.

Full old SDK inventory is at least 10.1 GiB of regular files plus symlinks, with
deep names inaccessible to ordinary Windows paths. The iOS-only subset uses
verbatim Windows paths and a byte-preserving tar stream into ext4, avoiding slow
per-file DrvFS metadata round trips. Full macOS/simulator/template trees are not
being copied. Apple assets remain local and outside all product repositories.

## Recovered state

- Engine and Windows base: `95047c0`, four mode-only differences on WinBtrfs.
- Emulator root `7aff7ea`; submodule gitlinks are QEMU `5940b611d5`, reims
  `50ad45e8`, metal2vulkan `f77b2d78`.
- Actual submodule checkouts are QEMU `6006d9d1d4` and reims `03fdd277`: **newer
  committed WIP**. Clean tracked files; do not revert to the root's old gitlinks.
- Independent NTFS emulator clone and all three independent module Git object
  stores created, without alternates/hardlinks into Linux. Original launcher and
  setup patch applied after `git apply --check`; snapshot/resume scripts copied.
  Full preserved patches, hashes, commit patches and selected untracked private
  research files live outside product repositories in `.recovery/`.
- Private older vphone repository `32e84bf1`; four authored untracked files
  preserved separately. `cpu-tree/` is a nested vphone-cli source checkout and
  is clean at `755d4919` (vphone-cli 2.13.4). QEMU's mlib dependency is
  `d0fd65a8`; its sources were staged without the original `.git` link.
- Historical `russet-asset-map.md` (2026-10-25) says x86 GPU emulation unproven;
  it predates the 2026-10-30 IOSurface fixes and is not current architecture.
- Latest Claude handoff says black-screen changes uncommitted; current Git
  commits and module histories contradict that. Resume resource reconstruction
  remains incomplete; no fast interactive resume acceptance.
- Original setup VM `.create-state` lists `booted`; its trace records memory fds
  on both gfx paths. This is historical evidence, not Windows guest acceptance.
  Original Disk.img is logically 64 GiB; no snapshot directory found in that
  bundle. Measure allocated data before copying within the 30 GB budget.

## Completed checks

- .NET 10 supervisor: Release build succeeds, zero warnings/errors.
- Windows/WSL tests: **4/4 pass**. Real Rust daemon initialize/ping/unknown method/
  shutdown; actual SwiftPM project detection from an NTFS path with Unicode and
  spaces; DAP framing/fragmentation/large Unicode messages; path translation;
  repeated fresh session connections; relay cleanup; insecure socket rejection.
- Linux helper has bounded independent full-duplex buffers and checks mode,
  ownership, symlinks and SO_PEERCRED. Host pipe has a protected current-user/
  SYSTEM DACL and kernel PIPE_REJECT_REMOTE_CLIENTS. No TCP
  listener or transport credential in arguments/logs.
- W14 extension: **17/17 unit tests**, TypeScript and changed-file ESLint pass.
  Existing Linux-assuming tests now select their platform explicitly. Added a
  test that a backend Unix DAP path produces a supervisor Windows pipe and is
  released with the debug session.
- Actual VS Code 1.144 Windows extension-host integration **passes** against the
  real Rust daemon with device backend `none`: supervisor connection, ping and
  project detection. First run exposed generated client's explicit undefined
  cancellation argument becoming positional JSON-RPC parameters; W14 now enforces
  named object parameters at the library boundary. Generated files are untouched;
  generator correction remains a W2 follow-up.
- Isolated Windows UI was inspected with the computer-use skill: Russet device,
  project, run-configuration and signing panels render. This profile intentionally
  used backend `none`; its empty device selector is not a physical-device test.
- Physical bridge discovery **passes using Russet's own mux/device code**:
  one USB device, `ready`, iOS 27.0.1, from the Linux daemon through a private Unix
  socket/WSL interop process to Windows AMDS loopback 27015. Device identifiers and
  names were not emitted. Normal lane-A live-device lock was used and the test
  daemon stopped successfully. Readiness: paired, Developer Mode yes, tunnel up.
  DDI mount from existing Apple-origin 27A266a directory **passes**, without force,
  reboot, mirror download or account mutation. Apple Devices startup supplies
  the listener even though the traditional service list is empty.
- Recovered QEMU builds on Ubuntu after declaring direct Nettle/Hogweed/GMP link
  dependencies for the SEP model. No incompatible SONAME aliases were used.
  Source archive must use `git -c core.autocrlf=false archive` on Windows.
- Synthetic shared-qcow2 block layout passes. Actual VM **paused machine
  initialization passes** with lavapipe and the original disk explicitly read-only,
  plus a separate writable qcow2 overlay. Source size/mtime remain unchanged.
  The actual virtio disk needs `vmapple-virtio-blk-pci.share-rw=on` to share the
  single qcow2 node with the firmware backdoor. Both use the same qcow2 cache.
- The launcher's advertised UDS vsock device is absent in recovered QEMU; use its
  actual `vhost-vsock-pci`. Managed user was added to the existing backend `kvm`
  group and the experiment launched with explicit runuser initialization. No
  world-writable device modes, host/global WSL settings or kernel sysctls changed.
- Bounded 120-second and 600-second guest runs reach the APFS System-volume mount
  with no observed kernel panic. Guest vphoned control does not become ready.
  Diagnostic repeat: all AP CPUs park at the same PC; disk reads stop at about
  2.6 MB/644 operations, no writes/failures, 200 KB overlay. This is a reproducible
  boot stall, not a completed guest/display test. Eight CPUs and baseline TCG
  comparison is in progress. HMP guest virtual-memory disassembly reset QMP and
  ended that diagnostic VM; removed it, do not repeat that intrusive probe.
  The original disk metadata remained unchanged and experiment processes stopped.
  Global WSL max_map_count already equals 1,048,576; no change was needed.
- Extension production bundle and **64 KB experimental VSIX** generated. Installed
  VS Code is 1.144.0 x64. Separate **Russet Development per-user installation**
  created at `%LOCALAPPDATA%/Programs/Russet Development`, 676 MB, from official
  VSCodium stable 1.143.17232, SHA-256
  `a1090868af8ac0ac04c22d79fdd02246fa115a78daf38a741d8db06df1753c1d`.
  Start Menu shortcut, separate profile, bundled W14 extension, supervisor and
  .NET launcher; prepared managed backend required, devices explicitly `none`.
  Actual installed-copy smoke **passes**: bundled extension path, Open VSX,
  telemetry off, updates disabled, real RPC ping and Windows project detection.
  First-run walkthrough inspected in the installed app. This is a Tier-1
  development repack, with upstream icon and no signed standalone installer.
- `project.init` created an actual SwiftUI `WindowsProbe` project in the isolated
  Windows runtime workspace with the real daemon. Actual SwiftUI ARM64 iPhone app
  build **passes** (41.9 s), unsigned. Recovered Swift 6.4.1 and iOS-only SDK reused
  locally, no Apple download. Staging repairs: restore 43 POSIX link targets,
  executable macro helper mode, and platform version.plist from Xcode metadata.
  NTFS compile succeeds but Rust bundle copy fails EPERM; redirecting only work
  also breaks builder-relative package paths. Explicit ext4 build view links
  inputs to authoritative Windows source and keeps outputs on ext4. Artifact
  export and full source mapping still need product integration.
- **Real LLDB DAP semantic test passes over the Windows supervisor**: breakpoint
  at a Windows source path with Cyrillic/spaces, variables counter=41, next to42,
  continue, exit0 and cleanup. This is a Linux host C target, not iPhone acceptance.
- Recovered release binary has no release manifest verification keys configured;
  SDK resolution cannot pass that trust gate. Build experiment uses recovered
  `0.1.0+gfcae66b-dirty` debug binary and its intended dev pins. Neither recovered
  executable is a fresh reproducible release; do not bypass trust for packaging.
- Windows certificate store has no usable Apple private identity. Recovered
  public cert/sealed secrets alone are insufficient. Additional read-only probe
  found original xtool identity: private key matches certificate, valid for next
  hour. Existing development profile matches, valid for another day, with debug
  entitlement and the designated Orchard lab-app slot. Supported local xtool
  import loads one account and one identity into memory secrets (no network).
  New WindowsPhysicalProbe build/sign/install/launch and batch source breakpoint
  at ContentView.swift:6 **pass**, probeValue42. Step hit a second location of the
  same breakpoint; full Windows DAP acceptance is still pending. Owned sessions,
  daemon and memory secrets were cleaned up. Owner-artifact scripts are local,
  env-gated and Git-ignored under engine AGENTS section8.
- Fresh engine W11 branch commits `d6164c7`, `5e3130e`: headless DAP initialization
  now supplies pathFormat; late IDE initialize/attach shares the already prepared
  process; pipeline returns the actual session endpoint. Real LLDB compatibility
  test and **6/6 DAP unit tests pass**. Full CLI rebuild in progress. Rust 1.99
  executes read-only from existing mounted toolchain, with checksum-verified
  145 MB locked registry cache copied locally, no Rust installer or new downloads.
- Two migrated skills pass the skill-creator validator. The bundled Python lacked
  PyYAML; validator dependency is isolated in `.recovery/skillcheck`.

## Latest handoff update — 2026-10-31

This update supersedes earlier in-progress statements above. Full continuation
context is in `../HANDOFF-CLAUDE.md`.

- Fresh W11 source and `/opt/russet/bin/russet-dev` are clean `8894314`, runtime
  `0.1.0+g8894314`. The third local fix registers the debug service through the
  supported factory so DAP sockets use the managed runtime home.
- The physical-iPhone scripted Windows RPC/DAP workflow now **passes**: build,
  sign, install, launch, Windows source breakpoint, probeValue42, step, continue,
  pause, continue again and owned-session/daemon cleanup. Evidence is private
  workspace `.recovery/physical-dap-summary.log`. The existing xtool identity was
  imported into memory secrets; no manual P12 export or portal mutation needed.
- Latest extension unit checks **18/18** and host/bridge checks **5/5** pass.
  Debug-provider merge order now preserves `type: russet`; supervisor readiness
  verifies the actual kernel pipe ACL. Installed development bits still need
  those updates and an actual editor F5 test. Earlier installed smoke is RPC only.
- Full Rust CI stopped at the repository's **6 GiB target-slot cap**; it is not
  green and is no longer running. Target cache reports 6.3G. Real LLDB integration
  and six DAP unit tests passed separately. Reclaim only owned generated scratch
  before completing required CI checks.
- The owner explicitly increased temporary C: development storage to **45 GB**;
  cumulative download approval remains **5 GB**. Latest C: free observation is
  345750167552 bytes, versus baseline 379717488640. Do not use the older 30 GB
  figure as the current authorization.
- Virtual boot remains stalled at APFS System mount after bounded CPU/TCG
  comparisons. No guest-ready control, embedded display/input or virtual debug
  acceptance. A compressed ext4 disk-base experiment is proposed, not performed;
  it requires storage accounting/reclamation first. Original disk stays read-only.
- Actual W14 directory branch is currently `russet-win`; inspect and preserve it.
  Computer Use ended on the owner's physical Escape before deployment/F5 checks.

## Claude continuation — 2026-10-31 afternoon

- **Repository CI at `8894314` is green, run split under the 6 GiB slot cap.**
  `backend/build_engine.py ci-split` runs `cargo xtask ci --profile pr --skip test`
  (disk-guard, fmt, clippy `-D warnings`, deny, guard, check-layers, check-paths,
  schema: all ok), then CI's exact `cargo xtask test --workspace --locked --profile ci`
  over disjoint target selections: `--lib --bins --examples` (634 passed, 42
  binaries) and eleven chunks of eight integration targets (331 passed, 12
  skipped, 83 binaries = every `crates/*/tests/*.rs`). `--workspace` keeps feature
  unification identical to CI. Between stages it unlinks linked executables and
  check-only metadata; peak slot allocation 6.05 GB. Total **965 passed, 0 failed**.
  Local guard skipped optional gitleaks, reuse and zizmor (not installed). An
  earlier letter-grouped attempt was killed by the cap guard; its five SIGTERM
  "failures" were rerun and pass. The slot was wiped first (owned scratch only).
- Installed development bits are **stale**: bundled extension hash `0157…` vs the
  current VSIX `9814…` (merge-order fix), host predates the ACL read-back.
  `scripts/update-dev-install.ps1` updates reversibly (backup under
  `%LOCALAPPDATA%\Russet\Backups`, `-Restore`) and refuses while the app runs; it
  has not been applied because the launcher-started editor from 11:46 is still open.
- New lane-A installed-editor F5 regression, local and Git-ignored harness:
  `tests/installed-f5.mjs` (+ shared `tests/lane-a.mjs`) drives the installed
  `Russet.exe` with in-editor test `tests/installed-f5/index.cjs`: launch.json F5,
  editor SourceBreakpoint, focused frame/revealed Windows line, Locals
  probeValue 42, F10, continue, pause, continue, Shift+F5. It asserts the session
  and the `attach` arguments stay `type: russet` and carry the backend's
  `attachCommands` (backend `dapConfig` says `lldb-dap`). **Not yet run.**
- Virtual boot is a **real stall, not slowness**. The guest reaches its last
  console line ~17 s after `cont`, then every QEMU thread sleeps (12 CPU-seconds
  in 3 minutes; Linux's slow APFS phase was CPU-bound). No I/O is in flight: no
  thread is in a read syscall. A/B runs (`virtual_probe.py` switches) all stall
  identically at 644 root reads: `--no-vsock`, `--split-disk` (separate overlays,
  no share-rw), `--no-gfx` (run-vphone.sh's headless default), plus the earlier
  8-CPU, baseline-TCG and icount runs. The QEMU commits since the Linux-booted
  `5940b611d5` only touch migration paths. NVRAM holds no boot-args or
  panic-recovery flags.
- New `--trace-sep` (private trace in the VM work directory) shows the SEP OS
  boots (1,830 boot-ROM messages) but announces only control `0x00` and xART
  slave/master `0x10`/`0x13`; keystore `0x12` and secure-credentials `0x0a` never
  appear. The console shows AMFI asking ACM for developer-mode status and ACM
  waiting on a SEP endpoint (`cmd=25`) with no answer; no `launchd` follows.
  The xART master's one request (op `0x0e`) gets no AP reply, which may be normal
  before userland. A blank private SEPStorage makes the SEP panic in AESS, so the
  recovered storage is what lets sepOS start at all. The VM's disk/NVRAM/SEP
  state was last touched by Linux fast-resume experiments (Disk.img 09:32,
  SEPStorage/nvram 09:26 on 2026-10-30; snapshots since deleted).
- Next discriminator: the same `--trace-sep` endpoint list from a known-good boot
  of this VM (Linux host or the Mac oracle). A fresh VM would need an online
  restore (Apple signing, likely over the download approval): owner's decision.

## Later 2026-10-31: installed F5, product direction, components

- **Installed-editor F5 passes on the physical iPhone** (15:37): installed Russet
  Development app, bundled VSIX `2d15d442…` and current supervisor, launch.json F5,
  one `debug.prepare`, session type `russet` with the merged attach recipe, editor
  breakpoint verified and line revealed, Locals probeValue42, F10, continue, pause
  (SIGSTOP), continue, Shift+F5, cleanup exit 0. The first run hung because the
  extension registers its provider twice and VS Code chained a second
  `debug.prepare`; the W14 provider now passes prepared configurations through
  (new unit test, 19/19). Harness: Git-ignored `tests/installed-f5.mjs` +
  `tests/lane-a.mjs`; in-editor test `tests/installed-f5/index.cjs`.
  `scripts/update-dev-install.ps1` deployed it reversibly (backups under
  `%LOCALAPPDATA%\Russet\Backups`).
- Owner direction: Russet itself installs xtool and vphone-linux, signs in with
  Apple, builds the SDK, creates the virtual iPhone and debugs both targets. See
  `IDE-EXPERIENCE-PLAN.md` for decisions, approvals (~20 GB downloads, ~110 GB C:,
  WSL memory 24 GB, backend sudo), backend isolation and the setup-runner contract.
- `Russet.Host --setup status|run <step>` implemented; component scripts
  `backend/xtool_component.sh` and `backend/vphone_component.sh` are installed in
  the distro at `/opt/russet/lib`. xtool 1.24.0 installed. vphone-linux upstream
  `c3b9ecda` fetched and building; the owner's cached iOS 27.0.1 and cloudOS 26.4
  IPSWs were copied in (no firmware download). Upstream lists iPhone17,3 + iOS
  27.0.1 as tested end to end on Linux.
- `Xcode_27.2.xip` (1.9 GB) contains iPhoneOS/iPhoneSimulator/MacOSX 27.2 SDKs; the
  first xtool install failed only because `swiftc` was not on PATH; rerunning in a
  staging home so the working SDK and the Linux xtool identity stay untouched.
- Parallel sessions: W22 native Apple sign-in (`ws/w22-apple-signin`, slot a) and
  W19 setup page / sign-in page / Xcode-style controls (`ws/w19-ide-experience`).

## Native Apple sign-in works (2026-10-31, owner's live run)

Integration branch `integration/windows-2026-10-31` (local only; W11 + W22 + W19 + lead
typos fix), engine `0.1.0+ge44f6af`, extension `ff3eb0e7…`. The owner signed in from the
installed IDE's native page: GrandSlam SRP init/complete, trusted-device 2FA (validate →
XML plist ec=0), Xcode token issued, DS2 `listTeams` (gzip-compressed XML plist) →
`accounts.toml` has an `apple-id` account with team tier `paid`. Path to get there:
- Local ADI (Apple's Android CoreADI) provisioned fine, but GrandSlam refused SRP init
  with -80045 when the client info claimed macOS (and still when it claimed Windows,
  with a 503 throttle): Apple now rejects a client identity that does not match the
  anisette's platform. The owner chose xtool's remote macOS anisette
  `https://mac-ani.jkcoxson.com` (setting `russet.account.anisetteServer`, machine scope);
  its operator sees the device-identity requests, never the password or codes.
- Then 2FA "validate" answered a non-plist success and DS2 answered gzip: W22 now judges
  code checks by HTTP status and decodes DS2 bodies (gzip/BOM/binary/XML/JSON).
- Diagnostics: launcher sets `RUSSET_LOG=info,russet_gsa=debug,hyper=warn` (stage, status,
  header names and body shape only; canary-tested).
Still open: secrets are `memory` in the launcher, so the session ends with the backend;
persistence (DPAPI-protected passphrase via `WSLENV`, or Windows Credential Manager) next.

## Evening 2026-10-31: the owner's own app runs from the IDE; physical mirror works

Integration branch `integration/windows-2026-10-31` (installed engine `0.1.0+g82eb229`):

- **New project → Run on the owner's iPhone works end to end** (owner confirmed). Fixes on the way:
  `85c6ff3` gives `swift build` the toolchain's `usr/lib/swift/linux` (Swift Build ran the recovered
  toolchain's `ld.lld`, which needs the private `libxml2.so.2` staged there); a stale Swift Build
  plan from the failed build had to be cleaned once (Russet should drop it when its env changes);
  Russet-Dev's `/etc/wsl.conf` now mounts C: `metadata,uid=1000,gid=1000` (bundling hit EPERM on
  root-owned drvfs files; original kept as `/etc/wsl.conf.russet-bak`).
- **Provisioning**: Apple's PLA had to be accepted (`71051ab` names it); `1e7c9c8` creates the signing
  certificate an approved plan needs (RSA-2048, sealed pending key, CSR, `/v1/certificates`), untested
  live because the owner imported xtool's identity first. Run still cannot ask for the approval: the
  owner approved with `russet approve <id>` in a terminal and the plan was applied with
  `scratchpad/provision_assist.py`. **Open:** an in-IDE approval prompt in Run, and `russet provision`
  cannot pass an approval id.
- **IDE**: API-key path mapping (`FramedPaths`, host), issuer-ID prompt (`c130a43`), organization
  identifier saved in `russet.project.organizationIdentifier` and used for new projects and to replace
  `com.example.*` before Run (`d47dfc9`).
- **Physical mirror (W10 first piece)**: `screen.screenshot` serves PNGs from CoreDevice
  `screencaptureservice` (DVT screenshot hung on iOS 27.0.1 and is only the fallback), each step limited
  to 15 s; the iPhone view polls it (`cd47dc4`, `4c8bea0`, `82eb229`). Owner sees the screen. Frames are
  6 MB 16-bit PNGs (~1 fps through the relay): next are host-side downscaling and the displayservice
  HEVC stream with HID input.
- **Launcher**: a second start used to restart the daemon without phones (its bridge could not take the
  socket); `DeviceBridge.Start` now re-probes and uses the running bridge.
- **Virtual iPhone**: the restore drops at `apfs_invert` because the guest's USB goes silent for minutes;
  `patches/vphone-linux-usbip-urb-idle.patch` keeps data URBs alive (NAKs, then up to 3600 s of silence)
  and reports real timeouts as ETIMEDOUT. Attempts 11 and 12 failed at 30 s and 300 s limits; attempt 13
  runs with the hour limit. Do not run heavy builds during the restore's boot stages.

## Open release gates

The physical iPhone passes the scripted Windows build/install/launch/application
debug sequence and the installed-editor F5 sequence. Guided product acceptance
(setup page, sign-in, device picker) remains open. The virtual target has not passed that sequence. Host-target LLDB success
alone is not iPhone source breakpoint/variables/step/continue acceptance.
The virtual panel is still a placeholder; no embedded interactive guest yet.
Guided setup completion, managed repair, production installed app/update/signing,
device/debug lifecycle and full packaging remain open. Current walkthrough has
several stub actions; opening it does not prove setup completion.

Apple Devices and iTunes Store apps are installed. No AMDS service was listed;
Apple Devices manifest includes an AppleMobileDeviceLauncher startup task. Do
not equate missing traditional service inventory with missing Apple software.

## Next experiments

1. Reclaim owned Rust target scratch below the 6 GiB cap and finish required CI.
   Reversibly deploy the latest extension/supervisor and verify installed-editor
   F5 using the proven lane-A memory-secret import and cleanup pattern.
2. Diagnose the bounded guest boot/control stall; paused initialization passed.
   Backing remains read-only. Any compressed ext4 conversion must fit the owner's
   45 GB temporary storage approval with space for overlays and build scratch.
3. Integrate general build views, source mapping, artifact export and SDK setup;
   retain release-manifest trust gates when replacing development artifacts.
4. Embed display/input, wire virtual install and DAP; preserve real target in the
   same release gate and schedule its lane-A checks with existing locks.
5. Package and verify the actual installed application; do not label the VSIX or
   supervisor package as completion of the product acceptance target.
