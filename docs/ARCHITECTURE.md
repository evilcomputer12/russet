# Russet architecture

- **Status:** v1.3, 2026-10-11. Written by the lead architect from the seven area designs. Review round 1 is applied: the [critique](review/critique-2026-10-11.json) plus four fact-finding passes (the Mac oracle, read-only; Apple's CDN and catalogs; the rig iPhone's firmware, without device commands; GitHub's runner images and billing). Its results are decisions D 62–D 81 in §20, superseded area-doc fields in §20.1, and owner questions Q25–Q33 in §21. Review round 2 (same day) is applied as decisions D 82–D 91, new §20.1 rows for build and core, and owner question Q34. Its critique is not committed, because it quotes details of the private track (Q34).
- **What this file is:** the single source of truth for everything that crosses an area boundary:
  - process model, crate boundaries and public APIs;
  - the `russetd` JSON-RPC catalog and notification catalog;
  - the MCP tool catalog;
  - configuration and file formats, directories, error codes, permissions and the security model.
- **Precedence.**
  - Cross-cutting topics: this file wins over the area docs.
  - Internals this file does not mention: the area doc is authoritative.
  - Changes to anything here go through the lead (PR touching `docs/ARCHITECTURE.md`).
  - Every disagreement found between the area docs, and how it was settled, is listed in §20.
- **Binding inputs:**
  - [`docs/decisions.md`](decisions.md)
  - [`docs/verified-facts.md`](verified-facts.md)
  - [`demo/m1-baseline.sh`](../demo/m1-baseline.sh)
  - [`docs/research/landscape-2026-10.md`](research/landscape-2026-10.md) (cited as *landscape*)
- **Area docs** (in `docs/design/`, cited by short name):
  - [core.md](design/core.md) (*core*)
  - [device.md](design/device.md) (*device*)
  - [build.md](design/build.md) (*build*)
  - [signing-accounts-publishing.md](design/signing-accounts-publishing.md) (*SIGN*)
  - [run-debug.md](design/run-debug.md) (*RD*)
  - [ide.md](design/ide.md) (*IDE*)
  - [qa-delivery.md](design/qa-delivery.md) (*QA*)
- **The implementation plan** lives in [PLAN.md](PLAN.md). Workstream ids used below (`W1`…`W33`, including the split cards `W16b`, `W21b`, `W27a`–`W27c`, `W28b` and `W28c`) are defined there.
- **Markers:**

  | Marker | Meaning |
  |---|---|
  | **[V]** | Verified hands-on on the dev box or the iPhone. The citation names the doc that holds the evidence, e.g. [V device §2.2] |
  | **[V lead]** | Verified by the lead while writing this file (§22 lists how) |
  | **[A]** | Assumption; the task that verifies it is named |
  | **[D n]** | Decision taken here; the rationale is entry *n* of §20 |

---

## 0. Reading guide

| Question | Section |
|---|---|
| Which process does what, and who owns a device tunnel? | §2, §3, §6 |
| Which crate do I put code in, and what may it depend on? | §4, §5 |
| What is the exact name and shape of a JSON-RPC method or notification? | §7, §8, §9 |
| How do the mirror, DAP and debugserver side channels work? | §10 |
| What can an AI agent do, and how are approvals enforced? | §11, §12 |
| Which error code or exit code do I use? | §13 |
| What goes into `russet.toml`, `config.toml`, `accounts.toml` and the other files? | §14, §15 |
| How are toolchains, the SDK, LLDB, libpython and TLS chosen? | §16 |
| What happens, step by step, on Run, Debug, Flutter run and ship? | §17 |
| What works on Linux, Windows and WSL, and in which wave? | §18 |
| Why was a conflicting proposal rejected? | §20 |
| What does the owner still have to decide? | §21 |

---

## 1. What Russet is, and the constraints that shape it

Russet lets anyone develop, build, install, debug, test and ship iOS apps from Linux and Windows with a physical iPhone and no Mac (decisions.md). It has three faces:
- **`russet`**: one Rust binary that is the CLI, the per-user daemon `russetd` and the MCP bridge;
- **Russet IDE**: Code-OSS built with VSCodium's tooling plus a built-in TypeScript extension;
- **agent access**: a built-in MCP server and an ACP agent panel.

| # | Constraint (source) | Consequence |
|---|---|---|
| K1 | Rust core; the device plane builds on `idevice` (MIT); pymobiledevice3 is reference and fallback only (decisions.md) | idevice runs in-process in the daemon. Fallback tools run only as subprocesses |
| K2 | One device tunnel owner per host; RSD ports change on every tunnel [V device §2.3 F2] | One long-lived daemon owns every device session (§3) |
| K3 | Headless must work (decisions.md) | Every feature is a JSON-RPC method and a CLI command first; the IDE and agents are clients |
| K4 | Never ship Apple files (decisions.md, NOTICE.md) | Apple inputs are Apple-origin copies the user obtains (their own `Xcode.xip` download, or a copy of their Mac's DDI folder), or files fetched from Apple at runtime under the user's own account. Russet never re-hosts them. This includes the Developer Disk Image, which `sdk.import` extracts from the user's `Xcode.xip` [D 62]. Docs and UI say "Apple-origin, user-obtained, never redistributed", never "licensed" (§19, Q26). CI guards scan the tree, caches and artifacts (§19) |
| K5 | No root, no Admin [V verified-facts 2026-10-10] | Userspace TCP tunnel (jktcp); `/run/usbmuxd` is world-writable; doctor prints commands for anything that needs root |
| K6 | Xcode 27 SDK needs Swift 6.4, and an `ld64.lld` that reads `arm64e.x1` TBDs [V build §2, core §10] | A version-locked toolchain resolver (§16) |
| K7 | Swift 6.4's Linux LLDB cannot start without `libpython3.12.so.1.0` [V core §23, RD V2] | The resolver returns tool paths **plus environment**; Russet provisions a 34 MB python-build-standalone runtime (§16.2) |
| K8 | Paid ASC team key is the default account; free Apple ID is opt-in (decisions.md) | Exact bundle IDs for paid teams; GSA and anisette are isolated opt-in crates (§5) |
| K9 | Linux first, then Windows on a physical PC (decisions.md) | cfg-gated platform modules from day one; Windows CI builds from Wave 1; hardening in Wave 5 |
| K10 | The machine is shared with a llama.cpp server (31 GiB RAM) and free disk is volatile (15 to 98 GiB within minutes [V QA V8]) | Build-job caps, `nice`, disk-guard tiers, no download over 500 MB without the owner's explicit OK (the tier check alone is not enough), the Code-OSS build in CI |
| K11 | Agents get the same powers as the user (decisions.md) | Permission classes, MCP tiers and out-of-band human approvals, enforced in the daemon (§7.5, §11) |

---

## 2. Component diagram

```text
┌──────────────────────────────────── CLIENTS ─────────────────────────────────────────────────────┐
│ Russet IDE (Code-OSS 1.135 built with VSCodium's scripts; launcher `russet-ide`)                  │
│   built-in extension `russet.russet` (TypeScript, vscode-jsonrpc 9)                               │
│     ├─ JSON-RPC control connection ───────────────────────────────────────────┐                  │
│     ├─ debug sessions: DAP over the per-session hub socket (§10.2) ──────────┐ │                  │
│     ├─ mirror webview: ws://127.0.0.1:<web>/mirror/v1 + one-time token ───┐  │ │                  │
│     └─ agent panel: ACP v1 client ─► agents (claude-agent-acp, codex-acp, gemini --acp, …)        │
│ `russet` CLI (same binary as the daemon) ──────────────────────────────┐  │  │ │                  │
│ agents: Claude Code, Codex, Gemini, Copilot CLI, Cline, Continue, OpenHands CLI, OpenCode        │
│   └─ spawn `russet mcp serve --client <id>` (stdio MCP) ── JSON-RPC ──┤  │  │ │                  │
│ OpenHands GUI in Docker ─► http://…/mcp (opt-in, bearer token) ───────┼──┤  │ │                  │
│ flutter_tools custom device ─► `russet flutter device <op>` ──────────┤  │  │ │                  │
└───────────────────────────────────────────────────────────────────────┼──┼──┼─┼──────────────────┘
       Linux: unix:$XDG_RUNTIME_DIR/russet/russetd.sock (0700 dir, 0600 socket, SO_PEERCRED)
       Windows: \\.\pipe\russet-<SID> (explicit DACL, server-SID check)   │  │  │ │
┌───────────────────────────────────────────────────────────────────────▼──▼──▼─▼──────────────────┐
│ russetd: one per OS user, tokio, autostarted by any client, idle exit                             │
│  IPC server · PermissionEngine · ApprovalService · JobManager · LeaseTable · SubscriptionHub ·    │
│  Web endpoint 127.0.0.1:<port> (mirror WS; opt-in MCP HTTP) · Pipelines (run/debug) · Audit log   │
│  ─────────── domain crates (plain async Rust APIs, no transport code) ───────────────────────────  │
│  russet-device      russet-sdk · russet-build   russet-sign · russet-account   russet-debug       │
│  (idevice + jktcp:  (SwiftPM via Swift Build,   (apple-codesign, ASC client,   (gdb-remote broker,│
│   usbmux, lockdown,  Info.plist, bundle, IPA,    provisioning, GSA/anisette    LLDB plans, DAP hub│
│   tunnel, RSD, DDI,  russet-tool shims,          opt-in) · russet-ship         JIT servicer,      │
│   screen, HID, perf) Flutter, Xcode later)       (upload, TestFlight)          symbols, crashes)  │
└───────┬─────────────────────┬───────────────────────────┬──────────────────────┬─────────────────┘
        │ usbmuxd /run/usbmuxd │ child processes in a      │ HTTPS (rustls with   │ child processes:
        │ (Linux) or AMDS      │ process group (Linux) or  │ the ring provider)   │ lldb, lldb-dap,
        │ 127.0.0.1:27015      │ Job Object (Windows):     │ App Store Connect,   │ flutter debug_adapter,
        │ (Windows)            │ swift build, ld64.lld,    │ GSA, TSS, Apple CDN, │ pymobiledevice3
        ▼                      │ russet-tool, flutter,     │ GitHub (pins)        │ (optional fallback)
   iPhone (USB; Wi-Fi later)   │ gen_snapshot              ▼                      ▼
                               ▼                     Apple services        python-build-standalone
                         toolchains: Swift 6.4, Russet-imported Darwin SDK, libpython for Swift LLDB
                         darwin-tools ld64.lld (LLD 21.1.6, patched)
```

---

## 3. Process model

### 3.1 Binaries

| Binary | Licence | What it is | Wave |
|---|---|---|---|
| `russet` | GPL-3.0-or-later | Multi-call: `russet <cmd>` is the CLI; `russet daemon run` (or the file invoked as `russetd`) is the daemon; `russet mcp serve` is the MCP bridge; `russet flutter device <op>` is called by flutter_tools [D 40] | 1 |
| `russet-tool` | MIT | Multi-call Xcode-tool shims for Swift Build and flutter_tools: `actool`, `ibtool`, `xcstringstool`, `xcrun`, `codesign` (no-op), `ld64.lld` trampoline (`-r` → `llvm-libtool-darwin -static`), and error shims for closed tools (`momc`, `metal`, …) [build §3.1]. Layer F: it may depend only on F crates (§5.2) | 2 |
| `russet-adi-host` | GPL-3.0-or-later | Sandboxed helper that loads Apple's runtime-downloaded Android ADI libraries; no network; JSON-lines IPC [SIGN §7.5]. Layer L3 binary; depends on F crates and `russet-core` only | 3 |
| `russet-swb` | MIT (links Apache-2.0 SwiftBuild) | Swift driver over the open SwiftBuild library: JSON PIF in, JSON-lines events out; built in CI per Swift minor version [build §7.4] | 6 |
| `russet-ide` | MIT (Code-OSS) + Russet overlay | The IDE launcher; bundles `russet` under `resources/app/bin/` [IDE §3.8] | 4 |

**Why one multi-call `russet` binary** [D 1]: the CLI and daemon of one install can never be different builds; there is one artifact to sign and package; `--no-daemon` and tests reuse the daemon code in-process [core §3.1].

### 3.2 Who owns what

| Resource | Owner | Lifetime |
|---|---|---|
| Device registry, tunnels (CoreDeviceProxy + jktcp, MSS ≤ 8132), RSD service maps | `russetd` → `russet-device` DeviceManager, one actor per UDID | While attached and (leased, or within `device.keep-warm`, default 10 min) |
| Port forwards, debug broker sockets, DAP hubs | `russetd` (device, debug actors) | Their lease |
| Log, perf and mirror streams | `russetd` SubscriptionHub and web endpoint | While subscribed |
| Run and debug sessions | `russetd` → `russet-debug` session registry | Until stopped, app exit, or owner gone (unless detached) |
| Jobs (build, sign, install, toolchain install, upload) | `russetd` JobManager | Queue → run → finish; persisted |
| Account tokens (ASC JWT ≤ 20 min, GSA), anisette helper | `russetd` → `russet-account` | In memory; secrets in `russet-secrets` |
| Approvals and the audit log | `russetd` ApprovalService | Pending ≤ 10 min; audit is append-only |
| UI: dialogs, prompts, editors, webviews | Clients | Client lifetime |

### 3.3 Daemon lifecycle (summary of [core §6.3])

1. **Autostart.** Any client calls `connect_or_spawn`:
   1. Try the endpoint (200 ms).
   2. Take `runtime/spawn.lock`.
   3. Spawn **its own executable** as `daemon run --detach`.
   4. Poll up to 10 s.
2. **Single instance.** The daemon holds `runtime/russetd.lock` for its whole life and writes `runtime/russetd.json`: `{pid, version, buildId, protocol, endpoint, web, startedAt, tcp, deviceBackend}` (0600). `deviceBackend` is `live`, `scripted:<file>`, `replay:<trace>`, `mock:<addr>` or `none`.
3. **Version skew.**
   - Same protocol major: continue. **Development builds** (`debug_assertions`, or any build whose `buildId` ends in `-dirty`) also compare `buildId` **and the executable identity**: `initialize.result.server.executable` plus `executableId`, the `{dev, ino, mtimeNs, size}` (Windows: volume serial and file index) of the file the daemon was started from, recorded once at daemon start, against the client's own `current_exe()`. `buildId` alone (`0.1.0+g<sha>[-dirty]`) does not change between rebuilds of an uncommitted tree. Any difference is handled like a major mismatch, with `ipc.buildMismatch` (1107, `details {serverBuildId, clientBuildId, serverExecutable, restartable}`) instead of `ipc.protocolMismatch`, so a freshly built CLI restarts an idle stale daemon [D 63, D 83].
   - Different major and the daemon is idle: the client asks for `daemon.shutdown {reason:"upgrade"}` and spawns its own build.
   - Otherwise: `ipc.protocolMismatch`.
4. **Idle exit** after `daemon.idle-timeout` (default 30 min) with no connections, no jobs and no detached leases. The daemon never idles out while the MCP HTTP endpoint is enabled.
5. **Shutdown.** Broadcast `daemon.shuttingDown`, cancel jobs (10 s grace, then kill process trees), close sessions, remove the socket.
6. **Crash recovery.**
   - `running` jobs become `interrupted`.
   - Orphan sessions are cleaned. Children are already dead: the primary cleanup is the process-group kill (Linux) or the Job Object (Windows); `PR_SET_PDEATHSIG` is the backstop for a daemon crash. PDEATHSIG fires when the spawning *thread* exits, not the process, so every child is spawned from one long-lived spawner thread (§5.3 `proc`) [D 73].
   - Russet-launched debuggees die because the broker set `QSetDetachOnError:0` [RD §4.6].
7. **Development isolation** (implementation agents share one OS user) [D 63, D 83].
   - **One daemon per worktree.** Every lane-D git worktree exports `RUSSET_HOME=<worktree>/.russet-dev`. That moves the socket, locks, state, config and caches (§7.1, §15), so each worktree autostarts its own daemon, built from its own branch, and never talks to another worktree's daemon.
   - **Lane A has one persistent home.** The device lane works through one agent at a time, so every lane-A card (W4, W9, W10, W18, the Wave-4 checks, W30) uses `RUSSET_HOME=$HOME/russet-lab/home-A` instead of a worktree path. That home keeps `[device.ddi] dir`, the imported accounts and the install ledger across cards; the executable check of item 3 restarts the idle daemon when the next card's build connects [D 82].
   - **Test daemons get unique sockets.** Tests that start a daemon (in-process or spawned) use the testkit's `TempHome`, a fresh temporary `RUSSET_HOME` per test, so parallel nextest runs never share a socket or a lock file.
   - **Secrets are per home.** With `RUSSET_HOME` set, the keyring master-key entry is `master-key-<first 8 hex digits of sha256(RUSSET_HOME)>` instead of `master-key`, so worktrees never share or reset each other's key (§12.2). Tests use `RUSSET_SECRETS_BACKEND=memory` or `passphrase`; the one env-gated keyring test (W3) creates its own uniquely named entry and deletes only that one. Removing a worktree also deletes its entry, which `russet secrets status --json` names [D 83].
   - **Lane D never touches the phone.** Lane-D worktrees also export `RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>`, so their daemon never constructs `LiveConnector` or opens `/run/usbmuxd`. In development builds the daemon also refuses the `live` backend unless `RUSSET_LANE=A` is set or `russet daemon run --allow-live-device` is used. A refused daemon still starts; every device method then returns `device.liveBackendRefused` (3019), and `doctor` says why. Two checks prove it: W2's daemon tests assert the debug counter `russet_device::io::live_connectors_constructed() == 0` (§5.3), and W8's tripwire `USBMUXD_SOCKET_ADDRESS` socket fails a test if anything connects to it. The tripwire cannot see a direct open of `/run/usbmuxd`, and it needs W4's `DeviceManager`, so it stays `#[ignore]` until W4 merges and W9 enables it [D 83].
   - **The device lane is serialized by a lock.** Any daemon whose backend is `live`, in development and release builds alike, holds `flock` on `$XDG_RUNTIME_DIR/russet-live-device.lock` for its whole life (Windows: the named mutex `Local\russet-live-device-<SID>`). The lock sits outside every `RUSSET_HOME`, so two worktrees, or a release daemon and a development daemon, can never own the phone at the same time (K2). A second live daemon starts, but its device methods return 3019 with `details.holder {pid, home}`. The daemon takes the lock at start, before any device code runs, and starts `DeviceManager` (and so constructs `LiveConnector`) only on the first device call that passes the lock check; a daemon that fails the check answers 3019 without constructing either [D 83]. `cargo xtask lab` reads the lock's holder: it runs through the live daemon of its own `RUSSET_HOME` and refuses to start while a daemon of another `RUSSET_HOME` holds the lock. **Tests never take the real lock:** debug builds honour `RUSSET_LIVE_DEVICE_LOCK=<path>` (ignored in release builds), and the lock tests start their `live` daemons with `--allow-live-device`, that override and `USBMUXD_SOCKET_ADDRESS` pointed at a test-local fake usbmuxd, never with `RUSSET_LANE=A` [D 83].
   - **No silent cross-build reuse.** Development clients compare `buildId`, the executable identity (item 3) and the requested device backend (§14.9) with the running daemon's.

### 3.4 Where work runs

| Work | Runs in | Why |
|---|---|---|
| Device protocols (lockdown, tunnel, RSD, DVT, AFC, display stream, HID) | Daemon, in-process (idevice) | Single tunnel owner (K2) |
| Compilers, linkers, resource tools, flutter, gen_snapshot | Daemon child processes, `nice 10`, at most `daemon.max-build-jobs` (default 1) | External toolchains; the whole tree is killed on cancel |
| Signing, IPA packing, Mach-O edits | Daemon, `spawn_blocking` | Pure Rust; no `zip` binary [V verified-facts M1] |
| gdb-remote broker, JIT servicer | Daemon tasks over the device tunnel | Only the daemon can reach the userspace tunnel [RD §3] |
| LLDB and lldb-dap | Daemon child processes, behind the per-session DAP hub; or `lldb -s attach.lldb` spawned by the CLI | libpython env and the Swift overlay are applied in one place [RD §9.3] [D 35] |
| ASC, GSA, TSS, toolchain downloads | Daemon (reqwest + rustls/ring) | Token caches, rate limits |
| Rendering, prompts, confirmations | Client (TTY, IDE dialog) | The daemon has no UI; it asks through `ui.*` server-to-client requests |

---

## 4. Repository layout

```text
russet/
├── Cargo.toml (rust-version = "1.99")  Cargo.lock  rust-toolchain.toml (channel = "stable") [D 70]  .cargo/config.toml
├── deny.toml  about.toml  REUSE.toml  LICENSES/  typos.toml  .config/{nextest.toml,workstreams.toml}
├── crates/                         # every Rust crate (§5); members = ["crates/*", "xtask", "tests/lab"]
├── xtask/                          # schema | pins | check-layers | check-paths | guard (alias check-no-apple) | disk-guard | ci | ci-budget
│                                   #   | test | build | sweep | lab | golden | traces | contracts | release | legal-check | docs
│                                   #   (W1 registers every subcommand as a stub; one module file per subcommand, PLAN §3)
├── schemas/                        # GENERATED, committed: rpc/v1/russet-protocol.schema.json, rpc/CHANGELOG.md,
│                                   #   config/*.schema.json, cli/*.schema.json, mcp/tools.v1.json
├── pins/                           # GENERATED by `xtask pins` in CI: stable.toml + stable.toml.minisig
├── extension/                      # the built-in VS Code extension (TypeScript; IDE doc §5.2)
│   └── src/generated/protocol.ts   # GENERATED from russet-protocol (ts-rs + typed client)
├── ide/                            # Code-OSS/VSCodium overlay: upstream.json, product.json, patches/9xx, branding, scripts
├── tests/
│   ├── lab/                        # crate `russet-lab` (device-in-the-loop, lane A only)
│   └── fixtures/apps/              # RussetProbe (SwiftUI), RussetFlutterProbe, RussetObjCMini, RussetJitProbe (W18): our code, 0BSD
├── corpus/                         # golden corpus: our inputs + normalized semantic JSON only (no raw Apple output)
├── templates/                      # project templates (0BSD)
├── packaging/{linux,windows}/      # deb/rpm/arch/systemd, wix/winget
├── demo/                           # m1-baseline.sh (historical), m1-russet.sh (Wave 1)
├── docs/                           # ARCHITECTURE.md, PLAN.md, decisions.md, verified-facts.md, design/, research/,
│                                   #   reference/ (generated), re/ (behaviour specs only), lab/, site/
└── .github/                        # workflows: ci, nightly, lab, oracle, apple-smoke, apple-release, release, publish, ide; actions/upload-safe
```

`docs/design/run-debug-prototypes/` is evidence only; W7 ports it into tests and deletes it (RD-02).

---

## 5. Crates

### 5.1 Crate table

Layers: **F** = format/protocol libraries (permissive, no `russet-*` dependency except other F crates, upstreamable); **L0–L4** as in [core §4.3]. "Owner" is the workstream that creates the crate; later waves may hand it on (PLAN §3).

| Crate | Layer | Licence | Owner | Responsibility | Key deps |
|---|---|---|---|---|---|
| `russet-plist` | F | MIT | W5 | XML and binary plist I/O; OpenStep reader/writer; Xcode build-setting macro expansion | plist 1.10.1 |
| `russet-macho` | F | MIT | W6 | Mach-O parse (thin/fat), `LC_BUILD_VERSION`, rpaths, load commands, UUID, `LC_SYMTAB` string-pool alignment check and fix, code-signature blob parse, in-place rpath delete, thin; last-resort sdk patch for foreign binaries [D 20] | goblin 0.10.7 or object 0.40.0 |
| `russet-ipa` | F | MIT | W5 | Deterministic IPA writer/reader: sorted entries, fixed mtimes, Unix modes from Mach-O magic, symlinks, Zip64, `dev` (stored) and `store` (deflate-6) modes, `SwiftSupport/` [D 19] | zip 9.0.1 |
| `russet-gdbremote` | F | MIT OR Apache-2.0 | W7 | gdb-remote codec, stop-reply parser, async `GdbClient`, interposer hooks | tokio, tokio-util |
| `russet-car`, `russet-assets` | F | MIT | W21 | BOM/CoreUI CAR writer/reader; `.xcassets` model, icon expansion, partial Info.plist, asset symbols | image, resvg, lzfse |
| `russet-xcstrings`, `russet-ib` | F | MIT | W21 (B10, B12); `russet-ib` then W21b (B21a), W28b (B21b) and W28c (B21c) [D 90] | `.xcstrings` compile and symbols. `russet-ib`: storyboard/XIB reader, launch-screen converter (B12), NIBArchive codec (coder version 10, the iOS `LNE\0` trailer with `IBUINibLNEVersionKey`, class fallback entries) and the storyboard/XIB → `.storyboardc`/`.nib` compiler (B21a–c) [D 75] | — |
| `russet-xcodeproj`, `russet-pif` | F | MIT | W28 | pbxproj / JSON `.xcproj` / workspace / scheme / xcconfig parsing and settings resolver; PIF emitter | — |
| `russet-xar` | F | MIT | W16b | Read-only Apple container formats for SDK and DDI import [D 62, D 69]: xar (zlib TOC, heap, SHA-1 TOC checksum; `.xip` and flat `.pkg`); the xar RSA signature, which is **prehashed** (a PKCS#1 v1.5 SHA-1 DigestInfo over the raw TOC checksum) and is chained to the runtime-pinned Apple Root CA (§16.6), never to the embedded root, ignoring leaf expiry; pbzx (16 MiB chunks, each XZ or stored) and gzip payload streams; `cpio` (odc/newc; hardlinks by (dev, ino)); UDIF (`koly` trailer, `blkx` tables; raw, zero-fill and zlib chunks, with bzip2 and lzfse optional because neither Apple DDI image uses them); a read-only HFS+/HFSX reader (DDIs need no decmpfs). Contract: streaming, with bounded in-order parallel pbzx decoding (at most threads × 2 × 16 MiB in flight); rejects absolute and `..` names; never writes through a link; output and size caps; cargo-fuzz targets | flate2, liblzma 0.4 (static; or lzma-rust2 0.21 if W16b's benchmark allows), quick-xml, sha1, sha2, rsa 0.9 (verify only: RUSTSEC-2023-0071 concerns private-key operations and is ignored in `deny.toml` for this crate with that reason), x509-cert; `cms` optional; `udif` 0.4.2 and `hfsplus` 0.4.0 (MIT) optional, pinned exactly; `apple-xar` and `cpio-archive` (MPL-2.0) only as dev-dependency oracles |
| `russet-tool` (binary) | F | MIT | W16 (`ibtool` compile and link modes: W21b) | The multi-call shim binary of §3.1 | F crates only |
| `russet-core` | L0 | GPL-3.0-or-later | W1/W2 | Errors and codes, ids, `RussetDirs`, fs helpers, processes (`CommandRunner`), progress and event sinks, clock and entropy, endpoints, secret wrappers, platform, build info, supervised tasks, `rpc` traits | thiserror, serde, schemars, secrecy, tokio, process-wrap, fs4 |
| `russet-protocol` | L1 | GPL-3.0-or-later | W1 (+ each area owns its namespace module; `Cargo.toml` is lead-owned after W1) | Every wire DTO; `rpc_method!`/`rpc_notification!`; `PROTOCOL`; `registry()`; schema and TS export [D 31] | serde, schemars, ts-rs |
| `russet-ipc` | L2 | GPL-3.0-or-later | W2 | JSON-RPC engine: LSP codec, `Peer`, `Router`, `Authorizer`, Unix/pipe/TCP transports, in-process duplex, autostart client | tokio, tokio-util, windows-sys |
| `russet-config` | L2 | GPL-3.0-or-later | W3 | Typed config structs, layering, comment-preserving editor, schemas, watcher | toml 1.1.8, toml_edit 0.25.17, notify |
| `russet-secrets` | L2 | GPL-3.0-or-later | W3 | `SecretStore` over keyring-core master key + sealed files, passphrase file store, env store, memory store; redaction helpers [D 13] | keyring-core 1.0.0 + store crates, chacha20poly1305, argon2, zeroize |
| `russet-toolchain` | L2 | GPL-3.0-or-later | W3 | Pin manifest, probes, resolver with env contract, installer, GC, SDK registry probe (`SdkInfo`) | reqwest (rustls-no-provider), sha2, pgp, minisign-verify, tar, zstd, goblin |
| `russet-doctor` | L2 | GPL-3.0-or-later | W3 | `Check` trait, registry, report model, host checks | — |
| `russet-agent-config` | L2 | GPL-3.0-or-later | W13 | Writes MCP registrations for 9 agent clients (plan/apply/detect) [IDE §6.6] | toml_edit, jsonc edits |
| `russet-device` | L3 | GPL-3.0-or-later | W4 | idevice integration: mux, pairing, tunnel, RSD, DDI status and mount (from a directory in W4; imported DDIs in W10), apps, launch, forwards, logs, screen/mirror/HID, files, crash pull, perf, symbols service client, pymobiledevice3 fallback, `io::ServiceConnector` seam | idevice =0.1.68 (ring; patched, [D 22]), jktcp =0.1.7, russet-ipa |
| `russet-sdk` | L3 | GPL-3.0-or-later | W5 | SDK facts (`SdkFacts`), platform view, toolsets, SDK and DDI import (`import`, W16b) [D 58, D 62, D 69] | russet-toolchain, russet-plist, russet-xar |
| `russet-build` | L3 | GPL-3.0-or-later | W5 | Build orchestrator: project model and detection (was `russet-project`), stack adapters (SwiftPM now; Flutter, Xcode, RN, MAUI later), Info.plist synthesis, bundle assembly, lints, metadata policy | russet-sdk, russet-macho, russet-ipa, russet-plist |
| `russet-sign` | L3 | GPL-3.0-or-later | W6 | Bundle walk, provisioning profiles, entitlements merge, signing through apple-codesign, verify, lints | apple-codesign 0.29.0 (`default-features = false`), russet-macho |
| `russet-asc` | L3 | GPL-3.0-or-later (licence open, §21) | W6 | Async typed App Store Connect client: ES256 JWT, scoped GET tokens, rate limiter, open enums, pluggable `Auth` (JWT, DS2) | reqwest, p256 |
| `russet-account` | L3 | GPL-3.0-or-later | W6 | Accounts, identities (CSR, p12, xtool import), `DevPortal`, provisioning planner/applier, free-team policy | russet-sign, russet-asc, russet-secrets, rsa, x509-cert, p12-keystore |
| `russet-gsa`, `russet-anisette` | L3 | GPL-3.0-or-later | W22 | GrandSlam auth, 2FA, DS2; anisette providers and library acquisition | num-bigint, aes-gcm, tokio-tungstenite (RemoteV3) |
| `russet-adi-host` (binary) | L3 | GPL-3.0-or-later | W22 | The sandboxed ADI helper of §3.1 | F crates, russet-core |
| `russet-debug` | L3 | GPL-3.0-or-later | W7 | Sessions, launch strategies, broker, JIT servicer, LLDB plans and overlay, DAP hub, symbols, crash store and symbolication, logs view, Flutter run/debug | russet-device, russet-build, russet-gdbremote, russet-toolchain, symbolic |
| `russet-ship` | L3 | GPL-3.0-or-later | W12/W23 | Archive, dSYMs, store IPA, validation, metadata stamping, upload, TestFlight, metadata, Apple-hardware mode | russet-build, russet-sign, russet-account, russet-asc, russet-ipa |
| `russet-xctest` | L3 | GPL-3.0-or-later | W32 (SwiftPM XCTest, Wave 4), W28 (XCUITest, Xcode projects) | XCTest/XCUITest runs, JUnit | russet-device |
| `russetd` | L4 | GPL-3.0-or-later | W2 (+ one handler file per namespace per area; `Cargo.toml` is lead-owned after W1) | The daemon: lifecycle, ctx, handlers, pipelines, permissions, approvals, jobs, leases, subscriptions, web endpoint, audit | everything |
| `russet-mcp` | L4 | GPL-3.0-or-later | W13 | MCP server on rmcp 3.5.x: tool catalog, stdio bridge, HTTP service mounted by `russetd` | rmcp 3.5.1, russet-ipc, russet-protocol |
| `russet-cli` | L4 | GPL-3.0-or-later | W2 (+ one command module per area; `Cargo.toml` is lead-owned after W1) | The `russet` binary: multi-call dispatch, clap tree, renderers, `--json`; `russet mcp serve` calls `russet_mcp::serve_stdio` | clap 4.6, indicatif, anstream, russetd, russet-mcp |
| `russet-testkit` | dev | GPL-3.0-or-later | W8 (`src/debug/` W7) | Scripted/replay/recording `ServiceConnector`s, mock usbmuxd, cassettes, HTTP fakes, temp-home daemon fixture, RDT traces. Tests of an L3 crate that use the testkit are **integration tests** under that crate's `tests/`, never `#[cfg(test)]` unit modules: the testkit ↔ crate dev-dependency cycle would otherwise compile two copies of the crate's types | — |
| `russet-golden`, `russet-oracle` | dev | GPL-3.0-or-later | W16 | Normalizers and comparison; Mac/GHA oracle runner | — |
| `russet-lab` (`tests/lab`) | dev | GPL-3.0-or-later | W9 (lane A) | Device-in-the-loop harness and suites | — |
| `xtask` | dev | GPL-3.0-or-later | W1 (`main.rs` and `Cargo.toml` are seam files; module files: W2 `schema`, W3 `pins` in Wave 1 then W26, W8 `guard`/`disk_guard`/`ci`/`ci_budget`/`sweep`, W9 `lab`, W15 `traces`/`contracts`, W16 `golden`, W26 `release`/`legal`/`docs`) | Code generation and repository gates | cargo_metadata |

Rejected crate names from QA's map (`russet-proto`, `russet-daemon`, `russet-package`, `russet-flutter`, `russet-mirror`, `russet-profile`, `russet-strings`, `russet-device-proto`, `russet-project`) are mapped in §20 [D 31, D 32].

### 5.2 Layering rules (enforced by `cargo xtask check-layers`)

```text
F   russet-{plist,macho,ipa,gdbremote,car,assets,xcstrings,ib,xcodeproj,pif,xar}, russet-tool (bin)
                                                      (only F deps; permissive licences)
L0  russet-core
L1  russet-protocol                                   → L0
L2  russet-{ipc,config,secrets,toolchain,doctor,agent-config}  → L0–L1 (toolchain → config)
L3  domain crates → F, L0–L2 except russet-ipc; among themselves only these edges and their transitive closure
    (A → B means B may depend on A):
      toolchain → sdk → build → sign → ship        sign → account → ship        asc → account
      asc → gsa → account      anisette → gsa       device → debug               build → debug
      device → xctest          ipa → device         asc → ship
    russet-adi-host (bin) → F, L0 only
L4  russetd → anything      russet-mcp → L0–L2 + rmcp
    russet-cli → L0–L2 + russetd + russet-agent-config + russet-mcp
dev russet-testkit, russet-golden, russet-oracle, russet-lab, xtask → anything
    (an L3 crate's testkit-based tests are integration tests under its tests/ directory, §5.1)
```

- Domain crates take `&dyn ProgressSink` / `EventSink` and a `CancellationToken`, and return `russet_core::Result`.
- Domain crates never open sockets to clients; transport lives in `russet-ipc` and `russetd`.
- MIT/Apache crates (layer F) may depend only on permissively licensed crates; xtask verifies this from `cargo metadata` [core §18]. `check-layers` reads the layer of every package, binaries included, from `.config/workstreams.toml`, so `russet-tool` and `russet-adi-host` are checked like libraries.
- **Wire enums** (every DTO in `russet-protocol`) [D 72]:
  - unit-only enums: `#[serde(rename_all = "kebab-case")]`;
  - enums that carry data: internally tagged, `#[serde(tag = "kind", rename_all = "kebab-case")]`, with struct variants only (serde cannot internally tag tuple variants), and camelCase fields inside;
  - externally tagged enums (serde's default) are not allowed on the wire. A unit test over `registry()` walks the generated schema and rejects them.
  - Domain crates may keep richer Rust enums; the protocol module converts. Example: `russet-device`'s `DeviceState` becomes the flat `DeviceSummary.state` of §9.5.

### 5.3 Public API contracts

The signatures below are what other workstreams call. Bodies, private types and minor renames are up to the owner, as long as all call sites move together. Full detail is in the cited area sections.

**Stub rule (freeze F0)** [D 74, D 86]. W1 lands **every** signature in this section as compiling code: types, traits, free functions, `DaemonCtx` fields, and the handler and doctor seams, with `todo!()` bodies (`core.notImplemented` for RPC handlers). Freezes F1 and F2 (PLAN §12) only fill bodies and make lead-reviewed refinements. No workstream waits for another workstream to *create* a type it names. F0 also includes:
- every error-code constant of §13.3 in its family file, with the `ALL` arrays, so a guard in one workstream can return another family's code (W2's isolation guard returns the `device` family's 3019);
- each stub crate's §5.1 key dependencies with their §16.7 pins and features (at least `apple-codesign` in `russet-sign`, `idevice` and `jktcp` in `russet-device`, `reqwest` in `russet-toolchain` and `russet-asc`, `rmcp` in `russet-mcp`), so `cargo tree -i aws-lc-sys` and `cargo deny` check the real dependency graph from day one;
- every module file whose owner differs from its crate root's owner, with its `mod` line in the parent (PLAN §3 lists them), so no workstream edits another's file to declare a module.

**`russet-core`** [core §5.1, QA §2]

```rust
pub mod error {
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ErrorCode { /* number: u16, name: &'static str ("device.locked"), exit: u8 */ }
    impl ErrorCode { pub const fn new(number: u16, name: &'static str, exit: u8) -> Self; pub const fn number(self) -> i32;
                     pub const fn as_str(self) -> &'static str; pub const fn exit_code(self) -> u8; pub fn from_number(n: i32) -> Option<Self>; }
    pub mod codes { pub mod core; pub mod ipc; pub mod perm; pub mod config; pub mod secrets; pub mod toolchain; pub mod sdk; pub mod device;
                    pub mod app; pub mod build; pub mod assets; pub mod flutter_build; pub mod flutter_run; pub mod xcode; pub mod sign;
                    pub mod account; pub mod provision;
                    pub mod gsa; pub mod anisette; pub mod debug; pub mod crash; pub mod ship; pub mod screen; pub mod perf; pub mod xctest;
                    pub mod mcp; pub mod agents; }   // one file per family (§13.2), each owned by one workstream: `pub const LOCKED: ErrorCode = …;`
    pub fn all_codes() -> &'static [ErrorCode];      // concatenation of per-family ALL arrays; a test asserts unique numbers and names
    pub struct Error { /* code, message, hint, details, retryable, source */ }
    impl Error { pub fn new(code: ErrorCode, msg: impl Into<String>) -> Self; pub fn with_hint(self, h: impl Into<String>) -> Self;
                 pub fn with_details(self, d: serde_json::Value) -> Self; pub fn retryable(self, yes: bool) -> Self;
                 pub fn caused_by(self, e: impl std::error::Error + Send + Sync + 'static) -> Self; pub fn to_data(&self) -> ErrorData; }
    pub struct ErrorData { pub russet_code: String, pub hint: Option<String>, pub details: Option<serde_json::Value>,
                           pub retryable: bool, pub docs: Option<String>, pub chain: Vec<String> }   // JSON-RPC error.data, camelCase
    pub type Result<T, E = Error> = std::result::Result<T, E>;
}
pub mod ids { pub struct JobId; pub struct SessionId; pub struct LeaseId; pub struct SubscriptionId; pub struct ApprovalId; /* prefixed ULIDs, §7.3 */ }
pub mod dirs { pub struct RussetDirs { pub config: PathBuf, pub data: PathBuf, pub state: PathBuf, pub cache: PathBuf, pub runtime: PathBuf }
               impl RussetDirs { pub fn from_env() -> Result<Self>; pub fn under(root: &Path) -> Self; pub fn ensure(&self) -> Result<()>; } }
pub mod fsx { pub fn atomic_write(p: &Path, b: &[u8]) -> Result<()>; pub fn ensure_space(p: &Path, needed: u64, reserve: u64) -> Result<()>;
              pub fn ensure_private_dir(p: &Path) -> Result<()>; }
pub mod proc {                                   // T1
    pub struct CommandSpec { pub program: PathBuf, pub args: Vec<OsString>, pub env: BTreeMap<OsString, OsString>, pub env_clear: bool,
                             pub cwd: Option<PathBuf>, pub nice: Option<i32>, pub timeout: Option<Duration> }
    #[async_trait] pub trait CommandRunner: Send + Sync {
        async fn run(&self, spec: &CommandSpec, sink: &dyn EventSink, cancel: &CancellationToken) -> Result<CommandOutput>;
        async fn spawn(&self, spec: &CommandSpec) -> Result<Box<dyn ChildProcess>>; }
    pub struct ProcessRunner;                    // process-wrap: process group + PDEATHSIG (Linux), Job Object + CREATE_NO_WINDOW (Windows)
    // Contract [D 73]: ProcessRunner forwards every spawn to ONE long-lived "russet-spawner" OS thread (a channel
    // of spawn requests), because PR_SET_PDEATHSIG fires when the spawning *thread* exits. Spawning from
    // spawn_blocking threads (reaped after ~10 s idle) or from the device listen runtime would kill children
    // mid-build. The group kill / Job Object is the primary cleanup; PDEATHSIG only covers a daemon crash.
}
pub mod progress {                               // T7
    pub trait ProgressSink: Send + Sync { fn begin(&self, title: &str, cancellable: bool); fn report(&self, msg: Option<&str>, pct: Option<u8>);
                                          fn end(&self, msg: Option<&str>); fn log(&self, stream: LogStream, line: &str); }
    pub enum JobEventKind { /* §9.2: Log, Progress, Stage, Task, Diagnostic, Artifact, Warning, Transfer, Custom */ }
    pub trait EventSink: ProgressSink { fn emit(&self, e: JobEventKind); }   // structured job events; russet-protocol re-exports the enum
}
pub mod time { pub trait Clock: Send + Sync { fn now(&self) -> SystemTime; fn instant(&self) -> Instant; } pub trait Entropy: Send + Sync { fn fill(&self, b: &mut [u8]); } }   // T2
pub mod net  { pub struct Endpoints { pub asc: Url, pub gsa: Url, pub tss: Url, pub anisette_libs: Url, pub github: Url, pub flutter_storage: Url } }  // T3; RUSSET_ENDPOINTS_FILE, RUSSET_OFFLINE
pub mod secret { pub use secrecy::{ExposeSecret, SecretBox, SecretString}; pub struct Redacted<T>(pub T); }
pub mod rpc {
    pub trait Method {
        const NAME: &'static str;
        const PERMISSION: Permission;            // the documented ceiling (§8 "Class"); MCP listing and docs use it
        const CONFIRM: Confirm;                  // Plain, or Typed (ui.prompt typed-confirmation; `--yes` cannot pre-confirm)
        const LONG_RUNNING: bool;
        type Params; type Result;
        /// Per-call class [D 66]. Default: PERMISSION. Overridden where the class depends on params or ownership:
        /// config.set/unset (scope), doctor.run (fix), screen.mirror.start (input), lease.release and job.cancel
        /// (own → read, else system), app.uninstall (mcp: own or dev.russet.test.* IDs, else Err(perm.denied)).
        fn permission(_p: &Self::Params, _c: &ConnInfo, _o: &dyn Ownership) -> Result<Permission> { Ok(Self::PERMISSION) }
    }
    pub trait NotificationMethod { const NAME: &'static str; type Params; }
    pub enum Permission { None, Read, Device, Build, Project, Account, AccountDestructive, Publish, Secrets, System }
    pub enum Confirm { Plain, Typed }           // Typed for every account-destructive method and ship.review.submit
    pub struct ConnInfo { pub id: String /* "c-7" */, pub kind: ClientKind, pub name: String, pub interactive: bool,
                          pub shared_lease: Option<LeaseId> }    // §6.3 shared device reservation
    pub enum ClientKind { Cli, Ide, Mcp, FlutterDevice, Test }
    pub trait Ownership: Send + Sync {           // implemented by russetd over its JobManager and LeaseTable
        fn owns_job(&self, conn: &str, job: &JobId) -> bool; fn owns_lease(&self, conn: &str, lease: &LeaseId) -> bool;
        fn project_bundle_ids(&self, conn: &str) -> Vec<String>; }
}
```

`rpc_method!(Marker, "ns.verb", Params => Result, Permission::X [, long_running] [, confirm = Typed] [, permission_fn = path])` generates the impl; `permission_fn` overrides `Method::permission`. W1 lands the trait, the macro and the dynamic rows above as stubs.

**`russet-protocol`** [core §5.3]
- One module per namespace (`src/<namespace>.rs`).
- Each method is declared with `rpc_method!` (full form above, under `rpc`).
- `registry() -> &'static [MethodInfo]` concatenates per-module `METHODS` arrays. The list of modules is fixed by W1, so area workstreams edit only their own module files.
- Every DTO derives `Serialize, Deserialize, JsonSchema, TS`, with `#[serde(rename_all = "camelCase")]` and the enum rules of §5.2. Inbound DTOs never use `deny_unknown_fields`.

**`russet-ipc`** [core §5.2]
- `Peer::{call, call_with, notify, connection, closed}`
- `Router<C>::{request::<M>, notification::<N>, methods}`
- `Authorizer::{on_connect, on_initialize, decide}`, returning `Decision::{Allow, Confirm, NeedsApproval, Deny}`
- `Server::{bind, serve}`, `connect`, `connect_or_spawn(dirs, ClientInfo, SpawnSpec)`
- `in_process() -> (Peer, Peer)`, used by `--no-daemon` and by the MCP HTTP service inside the daemon

**`russet-config`** [core §5.4] (the daemon-side service that `DaemonCtx.config` names) [D 86]

```rust
pub struct ConfigService { /* the global Loaded<GlobalConfig>, a per-project cache of Loaded<ProjectConfig>, the watcher */ }
impl ConfigService { pub fn load(dirs: &RussetDirs) -> Result<Self>; pub fn global(&self) -> Arc<GlobalConfig>;
                     pub fn effective(&self, project: Option<&Path>) -> Result<Effective>;     // §14.1 precedence; [agents] only narrows
                     pub fn editor(&self, scope: ConfigScope /* Global | Project | Local */, project: Option<&Path>) -> Result<ConfigEditor>;
                     pub fn subscribe(&self) -> broadcast::Receiver<ConfigChanged>; }
```

**`russet-secrets`** [D 13]

```rust
pub struct SecretId { pub kind: SecretKind, pub name: String }        // string form "<kind>/<name>", e.g. "asc-key/2X9R4HXF34"
pub enum SecretKind { AscKey, SigningKey, AppleIdSession, AppleIdPassword, AnisetteState, GithubToken, McpHttpToken, TcpToken }
pub struct SecretBytes(/* Zeroizing<Vec<u8>>; redacting Debug; no Display/Serialize */);
pub struct SecretMeta { pub id: SecretId, pub label: String, pub backend: BackendKind, pub created_at: OffsetDateTime, pub size_bytes: usize }
pub enum BackendKind { Keyring, PassphraseFile, Env, Memory }
#[async_trait] pub trait SecretStore: Send + Sync {                     // T6
    async fn put(&self, id: &SecretId, label: &str, v: SecretBytes) -> Result<SecretMeta>;
    async fn get(&self, id: &SecretId) -> Result<Option<SecretBytes>>;  // in-process only; never over IPC
    async fn delete(&self, id: &SecretId) -> Result<bool>;
    async fn list(&self, kind: Option<SecretKind>) -> Result<Vec<SecretMeta>>;
    fn backend(&self) -> BackendInfo; }
pub fn open_default(dirs: &RussetDirs, cfg: &SecretsConfig) -> Result<Arc<dyn SecretStore>>;
```

**`russet-toolchain`** [core §5.6, RD §8]

```rust
pub enum Component { Swift, DarwinSdk, Linker, Lldb, LldbDap, LibPython, Xtool, Flutter, GenSnapshotIos, Llvm }
pub struct ToolInfo { pub component: Component, pub version: String, pub path: PathBuf, pub provenance: Provenance,
                      pub env: BTreeMap<String, String>, pub ok: bool, pub problems: Vec<String> }      // env is part of the contract (§16.2)
pub struct SdkInfo { pub id: String /* "xcode-27.0-27A266a" */, pub xcode_version: String, pub xcode_build: String, pub ios_sdk: String,
                     pub swift_sdk_name: String, pub bundle: PathBuf, pub darwin_tools: Option<String>, pub compatible_swift: String }
impl Toolchains { pub async fn probe_all(&self, refresh: bool) -> Vec<ToolInfo>; pub async fn sdks(&self) -> Result<Vec<SdkInfo>>;
                  pub async fn resolve(&self, c: Component, req: &Requirement) -> Result<ToolInfo>;
                  pub async fn install(&self, c: Component, version: &str, p: &dyn ProgressSink, cancel: &CancellationToken) -> Result<ToolInfo>;
                  pub async fn uninstall(&self, c: Component, version: &str) -> Result<u64>; pub async fn gc(&self, dry_run: bool) -> Result<GcReport>; }
```

**`russet-device`** [device §5.3] (only what others call)

```rust
impl DeviceManager { pub async fn start(cfg: DeviceConfig, conn: Arc<dyn io::ServiceConnector>) -> Result<Arc<Self>>;
                     pub fn devices(&self) -> Vec<DeviceSummary>; pub fn events(&self) -> broadcast::Receiver<DeviceEvent>;
                     pub fn get(&self, sel: &DeviceSelector) -> Result<DeviceHandle>; }
impl DeviceHandle {
    pub async fn info(&self) -> Result<DeviceInfo>; pub async fn readiness(&self, refresh: bool) -> Result<Readiness>;
    pub async fn install(&self, pkg: &Path /* .app or .ipa */, o: InstallOptions, p: &dyn ProgressSink) -> Result<InstalledApp>;
    pub async fn apps(&self, f: AppFilter) -> Result<Vec<AppInfo>>; pub async fn uninstall(&self, bundle_id: &str) -> Result<()>;
    pub async fn launch(&self, r: LaunchRequest) -> Result<LaunchedProcess>;              // CoreDevice, DVT fallback; keeps the channel open (§17.1)
    pub async fn signal(&self, pid: u32, sig: i32) -> Result<()>; pub async fn processes(&self) -> Result<Vec<ProcessInfo>>;
    pub async fn connect_rsd(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>>;  // raw stream inside the tunnel (broker, symbols)
    pub async fn connect_lockdown(&self, service: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    pub async fn forward(&self, t: ForwardTarget, b: BindSpec) -> Result<ForwardHandle>;
    pub async fn logs(&self, f: LogFilter) -> Result<LogStream>;
    pub async fn screenshot(&self) -> Result<Screenshot>; pub async fn mirror(&self, o: MirrorOptions) -> Result<MirrorHandle>;
    pub async fn input(&self, e: InputEvent) -> Result<()>;
    pub async fn crash_pull(&self, q: CrashQuery, dest: &Path) -> Result<Vec<IpsReport>>;
    pub async fn sysmon(&self, o: SysmonOptions) -> Result<SysmonStream>; }
pub mod io { #[async_trait] pub trait ServiceConnector: Send + Sync + 'static {            // T5: plaintext streams after TLS / inside the tunnel
    async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>>;
    async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>>;
    async fn call(&self, api: FacadeCall) -> Result<serde_json::Value>; }
    pub struct LiveConnector;                    // Scripted / Replay / Recording connectors live in russet-testkit
    impl LiveConnector { pub fn new(cfg: &DeviceConfig) -> Result<Self>; }   // W4; increments the counter below
    pub fn live_connectors_constructed() -> usize; }  // debug-build counter (a static AtomicUsize; W1 lands it with a real body);
                                                 // W2's lane-D daemon tests assert 0 (§3.3 item 7) [D 83]
pub mod ddi {                                    // W4: status, Dir, mount records; W10: Imported, Pmd3Cache, Pmd3AutoMount, list [D 62]
    pub enum DdiSource { Imported { ddi_build: Option<String> } /* data/ddi/<ddiBuild>/iOS, i.e. its Restore/; newest first if None */,
                         Dir(PathBuf) /* a DDI root holding Restore/ and version.plist, e.g. a copy of a Mac's
                                         /Library/Developer/DeveloperDiskImages/iOS_DDI; Russet loads <dir>/Restore (idevice's
                                         Cryptex1Assets::load reads <restore>/BuildManifest.plist) and also accepts a directory that
                                         directly contains BuildManifest.plist [D 82] */,
                         Pmd3Cache /* ~/.local/share/pymobiledevice3/Xcode_iOS_DDI_Cryptex: mirror-derived, needs the allow-mirror consent */,
                         Pmd3AutoMount { allow_mirror: bool } /* opt-in subprocess; third-party mirror */ }
    pub struct DdiStatus { pub mounted: bool, pub kind: DdiKind /* Legacy | PersonalizedDmg | Cryptex1 */, pub version: Option<String>,
                           pub product_build: Option<String>, pub source: DdiOrigin /* imported | dir | pmd3-cache | pmd3-auto-mount | unknown */ }
    impl DeviceHandle { pub async fn ddi_status(&self) -> Result<DdiStatus>;
                        pub async fn ddi_mount(&self, src: &DdiSource, force: bool, p: &dyn ProgressSink) -> Result<DdiStatus>;
                        pub async fn ensure_ddi(&self) -> Result<DdiStatus>; }   // config order (§14.3 [device.ddi]); lazy; "remount when missing"
    pub fn list_imported(dirs: &RussetDirs) -> Result<Vec<ImportedDdi>>;       // data/ddi/index.json, newest ddiBuild first (device.ddi.list)
    pub struct MountRecord { pub source: DdiOrigin, pub path: Option<PathBuf>, pub ddi_build: String, pub cryptex_version: Option<String>,
                             pub image_sha384: String, pub mounted_at: OffsetDateTime }   // state/devices/<udid>/ddi.json (§14.8) [D 89]
    // Every mount Russet performs writes a MountRecord. ddi_status() reports the recorded source while the mounted cryptex version
    // and product build still match the record, otherwise `unknown`; matching versions alone cannot tell a dir copy from an
    // import of the same bytes.
    // Payload paths always come from the selected BuildManifest identity (Manifest.<key>.Info.Path): the Cryptex1 path uses idevice's
    // Cryptex1Assets::load; the personalized path (iOS 17–26.x) resolves PersonalizedDMG and LoadableTrustCache the same way.
    // Xcode-native names (022-…-411.dmg, Firmware/…) and pymobiledevice3's renamed Image.dmg both work.
}
```

- **Identity is read lazily** [D 63, D 83]. The daemon starts `DeviceManager` on the first device call (§3.3 item 7). On a mux `Attached` event the device actor records only the mux record (UDID, connection type, mux id). Lockdown values, Developer Mode and the DDI state are read the first time a client asks (`device.list`, `device.info`, a `device.watch` subscriber, or any device operation) and cached until detach. A daemon nobody asks never talks to the phone.
- Device errors are `russet_core::Error` values with device-family codes (§13.4). The NSError chain decoder is kept [device §9].

**`russet-sdk` and `russet-build`** [build §9.1]

```rust
// russet-sdk
#[async_trait] pub trait SdkProvider: Send + Sync { async fn resolve(&self, sel: &SdkSelector) -> Result<SdkFacts>; }
pub struct SdkFacts { pub id: String, pub sdk_path: Utf8PathBuf, pub sdk_version: Version, pub sdk_canonical_name: String,
                      pub sdk_build: Option<String>, pub platform_build: Option<String>, pub platform_version: String,
                      pub xcode_version: Version, pub xcode_build: String, pub swift_required: Version,
                      pub valid_deployment_targets: Vec<Version>, pub additional_info: plist::Dictionary,
                      pub back_deploy_dirs: Vec<Utf8PathBuf>, pub platforms_view: Utf8PathBuf,
                      pub toolset_swb: Utf8PathBuf, pub toolset_native: Utf8PathBuf, pub tools: ToolPaths }
pub struct XtoolBundleProvider { /* reads ~/.swiftpm/swift-sdks/*.artifactbundle in place; never copies */ }
pub mod import {                                                  // W16b; sdk.import [D 58, D 62, D 69]
    pub enum ImportPart { Sdk, Ddi }
    pub struct ImportRequest { pub source: Utf8PathBuf /* .xip | Xcode.app | .artifactbundle | XcodeSystemResources.pkg | iOS_DDI.dmg
                                                          | Apple Device Support .dmg | DDI dir (a root with Restore/, or a Restore dir) */,
                               pub parts: BTreeSet<ImportPart>, pub want: WantSet /* default: iOS only (§16.3) */,
                               pub links: LinkPolicy, pub layout: Layout, pub delete_source_after: bool, pub dry_run: bool }
    pub enum LinkPolicy { Symlink /* Unix */, JunctionHardlink /* Windows default */, Copy }
    pub enum Layout { Bundle /* data/sdk/<id>/bundle */, ShortRoot /* %LOCALAPPDATA%\Russet\s\<id8> (Windows) */ }
    pub enum DdiForm { PersonalizedDmg, Cryptex1 }
    pub enum Verified { AppleSignedPkg /* xar signature chained to Apple Root CA */, DigestOnly /* SHA-384 manifest digests */ }
    pub enum DdiImport { Imported { ddi_build: String /* the DDI's own ProductBuildVersion */, xcode_build: Option<String>,
                                    forms: Vec<DdiForm>, cryptex_version: Option<String>, core_device_version: Option<String>,
                                    verified: Verified, path: Utf8PathBuf },
                         Missing { reason: String }, Skipped }
    pub struct ImportReport { pub sdk: Option<SdkInfo>, pub ddi: DdiImport, pub bytes_written: u64, pub max_abs_path_len: usize,
                              pub links: LinkCounts /* junctions, hardlinks, copies, symlinks */, pub signature: Option<SignatureReport> }
    pub async fn import(r: &ImportRequest, dirs: &RussetDirs, sink: &dyn EventSink, cancel: &CancellationToken) -> Result<ImportReport>;
    // delete_source_after is honoured only after both parts succeeded and the import manifest verified.
}
pub struct ImportedBundleProvider { /* Russet-imported bundles in data/sdk/<id>/ (Linux) or the Windows short root */ }
// russet-build
impl Builder { pub fn new(env: BuildEnv) -> Result<Self>; pub async fn detect(&self, dir: &Utf8Path) -> Result<ProjectInfo>;
               pub async fn plan(&self, r: &BuildRequest) -> Result<BuildPlan>;
               pub async fn build(&self, r: BuildRequest, sink: &dyn EventSink, cancel: CancellationToken) -> Result<BuildOutput>;
               pub async fn resolve_info_plist(&self, r: &BuildRequest, product: Option<&str>) -> Result<plist::Dictionary>;
               pub async fn clean(&self, dir: &Utf8Path, scope: CleanScope) -> Result<CleanReport>; }
pub enum BundleIdPolicy { Exact, TeamSuffix { team_id: String }, XtoolPrefix { prefix: String }, Custom(BTreeMap<String, String>) }
pub enum MetadataPolicy { None, Minimal, Sdk, XcodeCompat { consent: Consent } }        // §16.4
pub struct BuildOutput { pub build_id: JobId, pub app: BundleInfo, pub extensions: Vec<BundleInfo>, pub bundle_ids: BTreeMap<String, String>,
                         pub signing_plan: SigningPlan, pub dsyms: Vec<Utf8PathBuf>, pub ipa: Option<Utf8PathBuf>,
                         pub report_path: Utf8PathBuf, pub lints: Vec<LintFinding>, pub duration: Duration }
pub struct SigningPlan { pub items: Vec<SignItem> }      // inside-out: deepest first, app last
pub struct SignItem { pub path: Utf8PathBuf, pub kind: SignKind, pub entitlements: Option<Utf8PathBuf>, pub get_task_allow: bool }
pub struct BuiltApp { pub app: Utf8PathBuf, pub executable: Utf8PathBuf, pub uuids: Vec<Uuid>, pub dsyms: Vec<Utf8PathBuf>,
                      pub sdk_path: Utf8PathBuf, pub swift_module_dirs: Vec<Utf8PathBuf>, pub flutter_framework: Option<Utf8PathBuf> }  // consumed by russet-debug
```

**`russet-macho`** (MIT; union of [build §4.7] and [SIGN §8.5])

```rust
pub struct MachO { /* parsed view; thin or fat */ }
impl MachO { pub fn parse(bytes: &[u8]) -> Result<Self, MachOError>; pub fn slices(&self) -> &[Slice];
             pub fn build_version(&self) -> Option<BuildVersion /* platform, minos, sdk, tools */>; pub fn uuid(&self) -> Option<Uuid>;
             pub fn rpaths(&self) -> Vec<String>; pub fn dylibs(&self) -> Vec<DylibRef>; pub fn encryption_info(&self) -> Option<EncInfo>;
             pub fn string_pool_alignment(&self) -> u32; pub fn code_signature(&self) -> Option<CodeSignature /* CDs, ents, team */>; }
pub fn delete_rpath(path: &Path, rpath: &str) -> Result<(), MachOError>;
pub fn align_string_pool(path: &Path) -> Result<bool, MachOError>;       // iOS 27 8-byte rule
pub fn thin(path: &Path, arch: Arch) -> Result<(), MachOError>;
pub fn patch_sdk_version(path: &Path, sdk: Version) -> Result<(), MachOError>;   // foreign binaries only; never the default
```

**`russet-ipa`** [build §4.8]: `write(app: &Utf8Path, out: &Utf8Path, o: IpaOptions{mode: Dev|Store, swift_support: Option<Vec<Utf8PathBuf>>, epoch: SystemTime}) -> Result<IpaReport>`, `read_listing(ipa) -> Result<Vec<Entry>>`.

**`russet-sign`** [SIGN §8.7]

```rust
pub trait CodeSigningIdentity: Send + Sync {             // implemented by russet-account's LoadedIdentity; breaks the sign↔account cycle
    fn certificate_der(&self) -> &[u8]; fn team_id(&self) -> &str; fn signer(&self) -> &dyn apple_codesign::KeyInfoSigner; }
pub struct ProvisioningProfile { /* parsed CMS payload */ }
impl ProvisioningProfile { pub fn parse(der: &[u8]) -> Result<Self>; pub fn covers(&self, bundle_id: &str, team: &str) -> bool;
                           pub fn has_certificate(&self, der: &[u8]) -> bool; pub fn has_device(&self, udid: &str) -> bool;
                           pub fn expires_within(&self, d: Duration) -> bool; }
pub struct SignRequest<'a> { pub bundle: &'a Path, pub identity: &'a dyn CodeSigningIdentity,
                             pub per_bundle: BTreeMap<String /* rel path; "" = app */, BundleSigningInput>, pub options: SignOptions }
pub fn sign(r: SignRequest<'_>) -> Result<SignReport>;    // pre-fixes (chmod 0755, string-pool alignment), inside-out, post-verify
pub fn verify(path: &Path, o: &VerifyOptions) -> Result<VerifyReport>; pub fn lint(path: &Path, p: LintProfile) -> Result<Vec<LintFinding>>;
pub fn inspect(path: &Path) -> Result<Vec<SignatureInfo>>;
```

**`russet-account` and `russet-asc`** [SIGN §5–§6]

```rust
impl AccountStore { pub fn load(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>) -> Result<Self>; pub fn list(&self) -> Vec<Account>;
                    pub async fn add_asc_key(&mut self, r: AddAscKey) -> Result<Account>; pub async fn import_xtool(&mut self, dry_run: bool) -> Result<XtoolImport>;
                    pub async fn portal(&self, id: &AccountId, team: Option<&str>) -> Result<Arc<dyn DevPortal>>; }
impl IdentityStore { pub fn list(&self, team: Option<&str>) -> Result<Vec<SigningIdentity>>; pub fn load(&self, id: &IdentityId) -> Result<LoadedIdentity>;
                     pub fn import_p12(&self, p12: &[u8], pw: &SecretBytes) -> Result<SigningIdentity>; pub fn new_csr(&self, team: &str, k: CertKind) -> Result<(PendingKey, String)>; }
impl Provisioner { pub async fn plan(&self, r: &ProvisionRequest) -> Result<ProvisionPlan>;            // GET-only
                   pub async fn apply(&self, plan: &ProvisionPlan, approval: &ApprovalRef) -> Result<ProvisionOutcome>; }
pub struct ProvisionPlan { pub plan_id: PlanId, pub digest: [u8; 32], pub team_id: String, pub mutations: Vec<Mutation>, pub warnings: Vec<PlanWarning> }
impl AscClient { /* typed resources: certificates, devices, bundle_ids, capabilities, profiles, apps, builds, build_uploads, testflight, ci; raw_get */ }
impl TokenFactory { pub fn bearer(&self) -> Result<SecretString>;                     // ≤ 19 min cache
                    pub fn scoped(&self, scope: &[ScopeEntry], lifetime: Duration) -> Result<SecretString>; }   // GET-scoped for agents
pub struct AccountService { /* AccountStore + IdentityStore + per-account TokenFactory and DevPortal caches */ }   // DaemonCtx.accounts [D 86]
impl AccountService { pub fn new(dirs: &RussetDirs, secrets: Arc<dyn SecretStore>, endpoints: Endpoints) -> Result<Self>;
                      pub fn accounts(&self) -> &tokio::sync::RwLock<AccountStore>; pub fn identities(&self) -> &IdentityStore;
                      pub async fn provisioner(&self, account: Option<&AccountId>, team: Option<&str>) -> Result<Provisioner>; }
```

**`russet-debug`** [RD §17.2]

```rust
impl DebugService { pub fn new(dev: Arc<dyn DeviceServices>, cfg: DebugConfig) -> Self;
                    pub async fn run(&self, r: RunRequest) -> Result<SessionHandle>;                   // launch without debugger
                    pub async fn prepare_debug(&self, r: DebugRequest) -> Result<PreparedDebug>;       // suspended launch + broker attach
                    pub async fn attach(&self, r: AttachRequest) -> Result<PreparedDebug>;
                    pub fn session(&self, id: &SessionId) -> Option<SessionHandle>; pub fn sessions(&self) -> Vec<SessionSummary>; }
pub struct PreparedDebug { pub session: SessionHandle, pub pid: u32, pub broker: BrokerEndpoint /* Unix{path} | Tcp{addr} */,
                           pub lldb: LldbLaunchPlan /* program, args, env, init_commands, attach_commands */, pub dap: Option<DapEndpoint> }
impl SessionHandle { pub fn state(&self) -> SessionState; pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;
                     pub async fn terminate(&self, how: TerminateMode) -> Result<()>; pub async fn hot_reload(&self, full: bool) -> Result<HotReloadResult>; }
pub trait DeviceServices { /* RD §17.3; implemented for russet_device::DeviceManager inside russet-debug, and by russetd's
                              DeviceSlot, which starts the manager on first use and delegates to it [D 83] */ }
// SessionState and SessionSummary convert to the wire shapes of §9.5 (state, lease, debugger) [D 84]
```

**`russet-gdbremote`** [RD §17.1]: `GdbClient::{handshake, request, pipeline, attach, resume, wait_stop, interrupt, read_memory, write_memory, set_breakpoint, remove_breakpoint, write_register, loaded_images, detach, kill}`.

**`russet-mcp`** [IDE §6.2]
- `serve_stdio(McpConfig) -> anyhow::Result<()>`
- `http_service(HttpState) -> StreamableHttpService<RussetMcp>`, mounted by `russetd`
- `tool_catalog() -> Vec<ToolInfo { name, tier, method_map, tool }>`

The bridge connects with `client.kind = "mcp"` and `client.name = <--client id>` (§11.2).

**`russetd`** [core §5.8]

```rust
pub async fn run(opts: DaemonOptions) -> Result<()>;
pub struct InProcess; impl InProcess { pub async fn start(o: DaemonOptions) -> Result<(InProcess, Peer)>; pub async fn stop(self) -> Result<()>; }
#[derive(Clone)]                                 // every field is cheap to clone; the types are those of this section [D 86]
pub struct DaemonCtx { pub dirs: RussetDirs, pub config: Arc<russet_config::ConfigService>, pub secrets: Arc<dyn russet_secrets::SecretStore>,
                       pub toolchains: Arc<russet_toolchain::Toolchains>, pub jobs: JobManager, pub leases: LeaseTable, pub subs: SubscriptionHub,
                       pub perms: Arc<PermissionEngine>, pub approvals: ApprovalService, pub audit: AuditLog, pub web: WebEndpoint,
                       pub devices: DeviceSlot, pub debug: Arc<russet_debug::DebugService>,
                       pub accounts: Arc<russet_account::AccountService>, pub shutdown: CancellationToken }
#[derive(Clone)] pub struct DeviceSlot { /* backend, live-device lock state, OnceCell<Arc<DeviceManager>> */ }   // W2 [D 83]
impl DeviceSlot { pub async fn manager(&self) -> Result<Arc<russet_device::DeviceManager>>; }
    // starts DeviceManager (and its connector) on the first call that passes the backend and lock checks (§3.3 item 7);
    // otherwise device.liveBackendRefused (3019, with details.holder when another daemon holds the lock)
// handler seam: every file crates/russetd/src/handlers/<ns>.rs exports
pub fn register(r: &mut Router<DaemonCtx>);
// doctor seam: every domain crate exports
pub fn checks() -> Vec<Arc<dyn russet_doctor::Check>>;
```

**`russet-cli`**: each area module `crates/russet-cli/src/cmd/<area>.rs` exports `pub fn command() -> clap::Command` and `pub async fn run(m: &ArgMatches, cx: &CliCtx) -> Result<ExitCode>`. `CliCtx` holds the daemon `Peer`, the reporter (`HumanReporter` or NDJSON `JsonReporter`) and the global flags (`--json --device --project -v/-q --color --no-daemon --yes`).

---

## 6. `russetd` internals

### 6.1 Module map

```text
crates/russetd/src/
  lib.rs ctx.rs daemon/ (lock, spawn, idle, shutdown, recovery)   state/ (persisted indexes)
  jobs/ (JobManager, LockKey, events)   leases/ (LeaseTable, grace)   subs/ (SubscriptionHub, bounded queues)
  perms/ (PermissionEngine, profiles, tier map)   approvals/ (ApprovalService)   audit/ (audit.jsonl)
  web/ (axum on 127.0.0.1:0; routes: mirror.rs [W10], mcp.rs [W20])
  pipeline/ (run.rs: build → provision → sign → install → launch/debug [W7]; install ledger)
  handlers/mod.rs (fixed list, W1) + handlers/<namespace>.rs (one per namespace; owner per §8;
           the `flutter` namespace is split into flutter_build.rs [W17] and flutter_run.rs [W18])
  doctor_registry.rs (fixed list of crates' checks(), W1)
```

### 6.2 Jobs [core §6.4]

- **States:** `queued → running → succeeded | failed | cancelled`, plus `interrupted` after a crash.
- **Kinds:** `build`, `sign`, `install`, `run`, `debug`, `provision`, `toolchain.install`, `sdk.import`, `ship.upload`, `symbols.fetch`, `doctor.fix`, …
- **Locks.**
  - `LockKey::Project(root)` for builds: one build per project.
  - `LockKey::Device(udid)` for install, uninstall, DDI mount, pairing and device reservation.
  - `LockKey::Sdk` for `sdk.import` and `sdk.remove`: one import at a time (it is disk- and CPU-heavy).
  - Read methods never lock.
- **Limits:** `daemon.max-parallel-jobs` (4), `daemon.max-build-jobs` (1).
- **Events:** every job writes `state/jobs/<id>/events.ndjson` (`JobEvent`, §9.2) and `result.json`. Build jobs also mirror events to `<project>/.russet/logs/<job-id>.jsonl`.
- **Ownership.** A job belongs to its requesting connection and is cancelled within 1 s of disconnect unless the request set `background: true` (§7.6; the RPC flag was called `detach` before [D 65]). `job.list` shows every client's jobs.
- **Retention:** 200 jobs or 30 days.

### 6.3 Leases and sessions [core §6.5, device §7]

| Lease kind | Created by | Ends |
|---|---|---|
| `device-reservation` | `device.reserve` (exclusive to its **principal**; others get `device.reserved`) | `lease.release`, or owner gone + 2 s grace |
| `tunnel` (internal) | First device operation | `device.keep-warm` idle (10 min) |
| `forward` | `forward.open`, `flutter.device.forward` | Close, or owner gone + 2 s |
| `app-launch` | `app.launch` | App exit, close, owner gone + 2 s |
| `session` (run/debug/flutter) | `run.start`, `debug.prepare`, `debug.attach`, `flutter.run` | Stop, app exit. Param `sessionLease: owned` (default) ends with the owner + 2 s; `detached` survives the owner. The IDE, `russet run --detach` and `russet debug --detach` create `detached` sessions [D 65, D 84] |
| `mirror` / `input` | `screen.mirror.start`, `input.*` (2 s holds) | Stop, owner gone; the display session stops 2 s after the last holder [device §6.8.2] |

- **Reservation principals** [D 68]. A reservation belongs to the connection that made it **plus** every connection that presents its id in `initialize.params.sharedLease`. The IDE starts the agents of its agent panel with `RUSSET_LEASE=<leaseId>` in their environment (and in the injected `russet mcp serve` arguments), and the bridge forwards it. So "the agent drives, the human watches the mirror" works during an IDE run, while unrelated agents and CLIs still get `device.reserved`. A shared lease cannot be released by a sharer; `lease.release` stays owner-only. Lab and E2E runs reserve without sharing.

### 6.4 Subscriptions

- Subscriptions return a `subscriptionId`.
- Queues are bounded: 10,000 log entries, 256 messages otherwise, and at most 3 mirror frames per WebSocket viewer.
- Slow consumers lose the **oldest** entries. The next notification carries `dropped: n`.
- Device readers are never blocked by clients.

### 6.5 Approvals and audit (see §11.3 for the rules)

- **The `Approval` record:**
  - `{approvalId, state: pending|granted|denied|expired|used, requestedBy: {kind, name, connectionId}, subject, tier, summary, detail, scope?: once|session, createdAt, expiresAt, grantedBy?}`
  - `subject` is one of:
    - `{kind:"method", method, paramsDigest}`
    - `{kind:"plan", planId, planDigest}`
- **Audit.** `state/audit.jsonl` (0600, append-only) records one line per entry: `{ts, kind: rpc|approval|account-mutation, client:{kind,name}, method, tool?, tier?, paramsDigest, paramsRedacted, approvalId?, outcome, durationMs}`.
  - Every MCP call is logged.
  - CLI and IDE calls are logged from class `account` upwards.

### 6.6 Web endpoint

- **What it is.** One axum listener on `127.0.0.1:0`, started lazily on first use. Its URL is written to `russetd.json` as `web`.
- **Routes:**
  - `/mirror/v1`: WebSocket, subprotocol `russet.mirror.v1` (§10.1).
  - `/mcp`: streamable HTTP, only when `[mcp.http] enabled = true` (§11.2).
- **Separate MCP listener.** The MCP HTTP endpoint may use its own configured listener (default proposal `127.0.0.1:43117` [A IDE §6.2]). With `expose-docker`, it also binds the Docker bridge IP and allows the `host.docker.internal` Host header.
- **Checks.** Every route checks `Origin` and `Host` and needs a token (§12).

### 6.7 Budgets [core §6.7]

| Item | Budget |
|---|---|
| Idle daemon | < 50 MB RSS, ~0 % CPU [A, W2 measures] |
| Builds | `max-build-jobs = 1`, `nice 10` (Linux) / `BELOW_NORMAL_PRIORITY_CLASS` (Windows) |
| Disk | `fsx::ensure_space` with `storage.min-free-space` (5 GiB) before any download, import, symbol fetch or cache growth; `core.diskFull` with a hint |
| Device symbols | Opt-in only; quota `storage.symbol-quota` (20 GiB) [RD §11] |

---

## 7. IPC protocol `russet/1`

### 7.1 Transports [core §7.1]

| Transport | Address | Access control |
|---|---|---|
| Unix socket (Linux) | `unix:$XDG_RUNTIME_DIR/russet/russetd.sock` [D 9]. With `RUSSET_HOME`: `$RUSSET_HOME/runtime/russetd.sock`, so each development worktree gets its own daemon (§3.3 item 7). Without `XDG_RUNTIME_DIR`: `/tmp/russet-<uid>/` (0700, owner-checked). Paths over 108 bytes: `/tmp/russet-<uid>/<hash>.sock` | Directory 0700, socket 0600. Server: `peer_cred().uid() == own uid`. Client: the directory owner and the server's peer uid are its own [V core §23] |
| Named pipe (Windows) | `\\.\pipe\russet-<user SID>[-<hash8> when RUSSET_HOME is set]` | DACL `D:P(A;;GA;;;<UserSID>)(A;;GA;;;SY)` via `create_with_security_attributes_raw`; `reject_remote_clients`; `first_pipe_instance(true)`. The client checks the server process owner SID. Wine does not enforce `first_pipe_instance` [V core §23], so the lock file is the primary single-instance guard |
| TCP (opt-in, WSL split mode, Wave 5) | `tcp://127.0.0.1:<port>`; 256-bit token in `runtime/tcp.json` (0600) | Token in `initialize.params.auth.token`, constant-time compare; closed if not authenticated within 2 s; non-loopback needs `--allow-nonlocal` |
| In-process duplex | — | Used by `--no-daemon`, tests, and the MCP HTTP service inside the daemon |

### 7.2 Framing

LSP base protocol: `Content-Length: <n>\r\n\r\n<n bytes of UTF-8 JSON-RPC 2.0>`.
- Header names are case-insensitive. `Content-Type` is ignored.
- Frames are at most 16 MiB; larger frames get `ipc.frameTooLarge` and the connection is closed.
- **Why:** stock `vscode-jsonrpc` 9.0.3 `SocketMessageReader/Writer` speak it. Interop with a tokio prototype was verified for initialize, `$/progress`, cancel → `-32800`, and unknown method → `-32601` [V core §23].

### 7.3 Conventions

| Item | Rule |
|---|---|
| Names | Requests: `namespace.verb` or `namespace.sub.verb` (camelCase verbs: `device.list`, `debug.symbols.fetch`). Notifications: `namespace.event` (`device.changed`, `logs.entries`). LSP built-ins keep their names (`$/progress`, `$/cancelRequest`) |
| Payloads | `params` and `result` are always objects. Fields are camelCase; enum values are kebab-case strings (`"trust-pending"`, `"step-over"`, `"debug"`) [D 53]. Optional fields are omitted, not `null`. Unknown fields are ignored |
| Times, sizes | RFC 3339 UTC strings; durations are integer `…Ms`; sizes are integer `…Bytes` |
| Paths | Absolute UTF-8 strings. Windows paths are normalized with `dunce`; WSL clients get a `PathMapping` (Wave 5) |
| IDs | Prefixed ULIDs: `job_…`, `ses_…` (run/debug session), `lse_…` (lease), `sub_…` (subscription), `apr_…` (approval, `^apr_[0-9A-Z]{26}$`), `pln_…` (provision plan), `upl_…` (upload), `str_…` (mirror stream). Connection ids are `c-<n>`. Server-originated request ids are strings `s:<n>` |
| Common params | `udid?` (UDID or alias; may be omitted when exactly one device is ready); `project` (absolute project root); `target?` (product, scheme or target); `configuration?` (`debug`, `release`, `profile`, or a named profile) |
| Secrets | No method returns secret values. Inputs such as a `.p8` path or an Apple ID password are accepted only from `cli` and `ide` connections |

### 7.4 Handshake

```jsonc
// client → daemon: the first message (anything else → -32002)
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocol":{"major":1,"minor":0},
  "client":{"kind":"ide","name":"russet-vscode","version":"0.2.0","pid":5150},      // kind: cli | ide | mcp | flutter-device | test
  "capabilities":{"confirm":true,"prompt":true,"openUrl":true,"approvals":true,"ideDebug":true,"interactive":false},
  "workspace":{"roots":["/home/u/RussetProbe"]}}}
  // optional, omitted when unused (§7.3): "auth":{"token":"…"} (TCP only), "sharedLease":"lse_…" (§6.3)
// daemon → client
{"jsonrpc":"2.0","id":1,"result":{
  "protocol":{"major":1,"minor":0},
  "server":{"name":"russetd","version":"0.1.0","buildId":"0.1.0+g1a2b3c4","pid":4242,"executable":"/usr/bin/russet",
            "executableId":{"dev":2049,"ino":1311234,"mtimeNs":1791700000000000000,"size":48211968},   // recorded at daemon start (§3.3)
            "startedAt":"2026-10-11T09:00:00Z","platform":"linux","arch":"x86_64","deviceBackend":"live"},
  "connectionId":"c-7",
  "grant":{"profile":"ide","allow":["read","device","build","project","account","secrets","system"],
           "confirm":["account-destructive","publish"],"approval":[],"deny":[]},          // "tierMax" only for mcp
  "capabilities":{"methods":["daemon.ping","…"],"notifications":["job.updated","…"],"features":{"mirror":true,"wifiDevices":false,"mcpHttp":false}},
  "endpoints":{"web":"http://127.0.0.1:41733"}}}
```

- `capabilities.interactive` is true only when the CLI's stdin is a TTY. Approval grants require it (§11.3).
- An MCP bridge sends `client.kind = "mcp"` and `client.name = <--client id>`. The daemon looks up that client's policy (§14.7).
- Major mismatch: error `ipc.protocolMismatch` (1100) with `details {serverProtocol, clientProtocol, restartable}`. Development builds with a different `buildId` or executable identity: `ipc.buildMismatch` (1107) with `details {serverBuildId, clientBuildId, serverExecutable, restartable}` (§3.3).

### 7.5 Permissions

Every method has a permission class (`russet_core::rpc::Permission`). The `PermissionEngine` decides per connection [core §7.5, QA §8.4] [D 6]. The §8 "Class" column is the method's ceiling (`Method::PERMISSION`); rows written "x (cond → y)" or "owner or system" are resolved per call by `Method::permission` (§5.3) [D 66].

| Class | Covers | `cli` | `ide` | `mcp` (tier) | `flutter-device` |
|---|---|---|---|---|---|
| `none` | `initialize`, `daemon.ping` | allow | allow | allow (–) | allow |
| `read` | lists, status, logs, screenshots, plans, `doctor.run` without fix | allow | allow | allow (0) | allow |
| `device` | install, launch, stop, forwards, input, DDI mount, pairing, debugging | allow | allow | allow if `tierMax ≥ 1` (1) | allow |
| `build` | build, clean, local signing, packaging | allow | allow | allow if `tierMax ≥ 1` (1) | allow |
| `project` | edits to `russet.toml`, plists, entitlements, assets inside the project | allow | allow | allow if `tierMax ≥ 1`, project files only (1) | deny |
| `account` | create-only Apple-side changes (register device or bundle ID, enable capability, create profile or certificate) | allow, subject to `provision-policy.toml` | same | **approval**, tier 2 | deny |
| `account-destructive` | revoke certificate, delete profile or bundle ID, disable capability, remove device | confirm (typed) | confirm (typed) | **never** | deny |
| `publish` | upload, TestFlight distribution, metadata apply, CI dispatch | confirm | confirm | **approval**, tier 3 | deny |
| `secrets` | accounts add/remove, Apple ID login, secret set/delete | allow (interactive); local-file imports also non-interactive with `--yes` (below) | allow | **never** | deny |
| `system` | toolchain/SDK install, global config, daemon shutdown, MCP admin, approval grant | allow | allow | **never** | deny |

- **Narrowing.**
  - `mcp-policy.toml` sets `tier-max` per client: default 1, local models 0 [D 60].
  - A project's `[agents]` section can only lower it, or deny tools.
- **Confirm (cli/ide).**
  - The daemon sends `ui.confirm` to the requester. The CLI's `--yes` pre-confirms. A CLI with no TTY and no `--yes` gets `perm.confirmationUnavailable`.
  - Methods with `Confirm::Typed` (every `account-destructive` method, and `ship.review.submit`) use `ui.prompt` with typed confirmation. `--yes` does not satisfy it; a headless CLI must pass `--confirm-text "<expected>"` (sent as `confirmText`), which the daemon compares with the expected string.
  - Every confirmation is recorded as an approval with `grantedBy` set to the requester.
- **Secrets from a CLI without a TTY** [D 85]. `secrets`-class methods need `interactive`, except the three local-file imports: `account.importXtool`, `account.addAscKey` and `secrets.set {fromFile}`. From a non-interactive `cli` connection these get a plain `ui.confirm`, which `--yes` pre-confirms, so agent shells and lab scripts can run `russet account import --from-xtool --yes`. Passwords, 2FA codes (`account.appleId.*`, `ui.prompt` with `secret: true`) and approval grants stay TTY-only. The imports are audited like every `secrets`-class call (§6.5).
- **Approval (mcp).** §11.3. MCP elicitation is a convenience only and never counts as approval [D 5].
- **Profiles are guardrails, not a sandbox.** An agent with shell access can run the CLI as the user. The docs say so, and CLI approvals need a TTY [core §7.5, IDE §6.4].

### 7.6 Progress, cancellation, jobs

- **Long-running methods** (marked **long** in §8) accept an optional `progressToken` and `background?: bool` [D 65].
  - Default: the request stays open until the final result; the job is still visible through `job.*` [D 3].
  - `background: true`: the request returns `{jobId}` at once, and the job survives the requester's disconnect. Get the result with `job.wait`.
  - `background` is about the **job**. Whether a run or debug **session** outlives its creator is the separate `sessionLease: owned|detached` parameter (§6.3).
- **CLI `--detach`** (`russet run`, `russet debug`, `russet debug prepare`, `russet flutter run`) means "leave the session running": the CLI waits for the method's result (build → install → launch), prints `{sessionId, pid, bundleId}` (`--json`) and exits, leaving a `detached` session lease. It never sets `background`. `russet stop <session>` or `russet stop --all` ends such sessions. A CLI that wants the job itself in the background uses `--background` (returns `{jobId}`).
- **`russet debug --detach`** [D 84] calls `debug.prepare {sessionLease: detached}`, and when LLDB is done (the interactive session quits, or the last `--batch` command has run) the generated script ends with `process detach` instead of `process kill`. The broker forwards LLDB's `D` (the "IDE Detach" row of RD §4.6, not `onDebuggerLost`), the app keeps running, and the session stays `running` with `debugger: detached` (§9.5).
- **Progress** uses LSP work-done `$/progress {token, value:{kind: begin|report|end, title?, message?, percentage?, cancellable?, jobId?}}`.
- **Cancellation:** `$/cancelRequest {id}` cancels the job's token. The response is `-32800` with `data.russetCode = "core.cancelled"`.
- **Job details** come from `job.subscribe`, which streams `job.updated` and `job.events`, and from `job.log`.

### 7.7 Versioning and generated artifacts

- **Version.** `PROTOCOL = {major: 1, minor: 0}`.
  - Minor: additions only.
  - Major: removals, renames and semantic changes.
  - Clients check `capabilities.methods` before using anything newer than 1.0.
- **Generated by `cargo xtask schema` and committed:**
  - `schemas/rpc/v1/russet-protocol.schema.json`;
  - `extension/src/generated/protocol.ts` (ts-rs types plus a typed `RussetClient` over vscode-jsonrpc);
  - `docs/reference/rpc.md`;
  - `schemas/config/*.schema.json`;
  - `schemas/mcp/tools.v1.json`.
- **Gates.** `schemas/rpc/CHANGELOG.md` lists every change. CI fails on stale generated files, and on a schema diff without a version bump.
- **Merge rule:** never hand-merge generated files; re-run the generator.

---

## 8. JSON-RPC method catalog (complete for v1)

- **Columns:** **Class** = permission (§7.5); **L** = long-running; **WS** = implementing workstream; **Wave** = first wave in which the method must work.
- **Shapes** are essentials; the DTOs live in `russet-protocol/src/<namespace>.rs`, and the area doc section named in each heading has field-level detail.
- **Stubs.** W1 registers every name below, so unimplemented methods return `core.notImplemented` (1005) until their wave.

### 8.1 Core: daemon, jobs, leases, config, secrets, toolchain, SDK, doctor, approvals [core §7.10]

| Method | Class | L | Params → Result | WS | Wave |
|---|---|---|---|---|---|
| `initialize` | none | | §7.4 | W2 | 1 |
| `daemon.ping` | none | | `{}` → `{}` | W2 | 1 |
| `daemon.status` | read | | `{}` → `{version, buildId, pid, uptimeMs, endpoint, web?, connections[{id,kind,name,pid}], jobs{queued,running}, leases[], devices, rssBytes}` | W2 | 1 |
| `daemon.shutdown` | system | | `{force?, reason?}` → `{accepted, activeSessions}` | W2 | 1 |
| `daemon.setLogFilter` | system | | `{filter}` → `{previous}` | W2 | 1 |
| `daemon.logs.subscribe` | read | | `{minLevel?}` → `{subscriptionId}` ⇒ `daemon.logBatch` | W2 | 1 |
| `subscription.cancel` | read | | `{subscriptionId}` → `{}` | W2 | 1 |
| `lease.list` | read | | `{}` → `{leases: Lease[]}` | W2 | 1 |
| `lease.release` | owner or system | | `{leaseId}` → `{}` | W2 | 1 |
| `job.list` | read | | `{states?, kinds?, project?, limit?}` → `{jobs: JobInfo[]}` | W2 | 1 |
| `job.get` | read | | `{jobId}` → `JobInfo` | W2 | 1 |
| `job.wait` | read | | `{jobId, timeoutMs?}` → `{job, result?, error?: ErrorData}` | W2 | 1 |
| `job.cancel` | owner or system | | `{jobId}` → `{state}` | W2 | 1 |
| `job.subscribe` | read | | `{jobId?, kinds?, includeEvents?}` → `{subscriptionId}` ⇒ `job.updated`, `job.events` | W2 | 1 |
| `job.log` | read | | `{jobId, afterSeq?, tail?, kinds?}` → `{events: JobEvent[], nextSeq}` | W2 | 1 |
| `config.get` | read | | `{scope: effective\|global\|project\|local, project?, key?}` → `{value, sources[{scope,path}]}` | W3 | 1 |
| `config.set`, `config.unset` | project (project/local) or system (global) | | `{scope, project?, key, value?}` → `{path, changed}` | W3 | 1 |
| `config.schema` | read | | `{file: project\|global\|accounts\|mcp-policy\|provision-policy}` → `{schema}` | W3 | 1 |
| `secrets.status` | read | | `{}` → `{backend, available, locked, reason?}` | W3 | 1 |
| `secrets.list` | read | | `{kind?}` → `{items: SecretMeta[]}` (metadata only) | W3 | 1 |
| `secrets.set` | secrets | | `{id, label?, fromFile}` → `SecretMeta` (cli/ide only; never echoes) | W3 | 1 |
| `secrets.delete` | secrets | | `{id}` → `{deleted}` | W3 | 1 |
| `toolchain.list` | read | | `{project?, refresh?}` → `{tools: ToolInfo[], sdks: SdkInfo[], pins{id,source}}` | W3 | 1 |
| `toolchain.resolve` | read | | `{component, project?}` → `ToolInfo` (incl. `env`) | W3 | 1 |
| `toolchain.install` | system | L | `{component, version}` → `ToolInfo` | W3 | 1 (`python-shared`, `darwin-tools`) |
| `toolchain.uninstall` | system | | `{component, version}` → `{freedBytes}` | W3 | 2 |
| `toolchain.gc` | system | | `{dryRun?}` → `{freedBytes, removed[]}` | W3 | 2 |
| `toolchain.pins.update` | system | | `{channel?}` → `{id, changed}` | W26 | 4 |
| `sdk.list` | read | | `{}` → `{sdks: SdkInfo[]}` | W3 | 1 |
| `sdk.import` | system | L | `{source: path (Xcode.xip \| Xcode.app \| .artifactbundle \| XcodeSystemResources.pkg \| iOS_DDI.dmg \| <Device Support>.dmg \| <DDI dir>), parts?: (sdk\|ddi)[] (default both), want?: ios (default) \| full, deleteSourceAfter?, dryRun?}` → `{sdk?: SdkInfo, ddi: {kind: imported, ddiBuild, xcodeBuild?, forms[], cryptexVersion?, coreDeviceVersion?, verified, path} \| {kind: missing, reason} \| {kind: skipped}, bytesWritten, maxAbsPathLen, links{junctions, hardlinks, copies, symlinks}}`. Signature checks (`.xip`, `.pkg`) and the SHA-384 manifest-digest check (every source) run before anything is stored. The DDI is extracted **before** `deleteSourceAfter` is honoured; a source without one (xtool's bundle keeps only `Developer/`, `Info.plist`, `version.plist`) reports `missing` and the CLI asks once for the `.xip` [D 62, D 69] | W16b | 2 (the DDI part first) |
| `sdk.remove` | system | | `{id}` → `{freedBytes}` | W16b | 2 |
| `sdk.deviceSupport.check` | read | | `{xcodeBuild?}` → `{kind: none, forXcode} \| {kind: available, ddiBuild, url, bytes, authentication: none\|virtual}`: Apple's public Xcode downloadables index (metadata only; later Pallas `com.apple.MobileAsset.XcodeDeviceSupport`) for `xcodeDeviceSupport` entries that match the imported Xcode build. **Not v1** (§21 Q25); today the answer would be `{kind: none, forXcode: 27A266a}` | W33 | deferred |
| `sdk.deviceSupport.fetch` | system | L | `{entry, consent: true}` → `ImportReport`: downloads only from Apple hosts under the user's IP (the "virtual audience" cookie only where Apple marks an entry `authentication: virtual`), verifies the `.pkg` signature, then feeds the `sdk.import` DDI pipeline. **Not v1** (§21 Q25) | W33 | deferred |
| `doctor.run` | read (`fix` → system) | | `{checks?, categories?, project?, udid?, online?, fix?}` → `DoctorReport` | W3 (+ area checks) | 1 |
| `approval.list` | read | | `{state?}` → `{approvals: Approval[]}` | W2 | 1 |
| `approval.get` | read | | `{approvalId, waitMs?}` → `Approval` | W2 | 1 |
| `approval.grant` | system; kind `ide`, or `cli` with `interactive` | | `{approvalId, scope?: once\|session}` → `Approval` | W2 | 1 |
| `approval.deny` | system; same restriction | | `{approvalId, reason?}` → `Approval` | W2 | 1 |
| `audit.tail` | read | | `{limit?, kinds?, client?}` → `{entries: AuditEntry[]}` | W2 | 1 |

### 8.2 Device plane [device §7]

| Method | Class | L | Params → Result | WS | Wave |
|---|---|---|---|---|---|
| `device.list` | read | | `{}` → `{devices: DeviceSummary[]}` | W4 | 1 |
| `device.watch` | read | | `{}` → `{subscriptionId}` ⇒ `device.changed` | W4 | 1 |
| `device.info` | read | | `{udid?}` → `DeviceInfo` | W4 | 1 |
| `device.readiness` | read | | `{udid?, refresh?}` → `Readiness {paired, developerMode, ddi, locked, tunnel}` | W4 | 1 |
| `device.prepare` | device | L | `{udid?, ddiSource?, revealDeveloperMode?}` → `Readiness` (pairs if needed; mounts the DDI if missing, per §14.3 `[device.ddi]`) | W4 (DDI from a directory) / W10 (imported DDI default) | 1 |
| `device.pair` | device | L | `{udid?, hostName?}` → `{status: paired\|pending\|denied}` | W4 | 1 |
| `device.unpair` | device | | `{udid}` → `{}` | W10 | 2 |
| `device.developerMode.reveal` | device | | `{udid?}` → `{developerMode}` | W10 | 2 |
| `device.ddi.status` | read | | `{udid?}` → `DdiStatus {mounted, kind, version?, productBuild?, source?}` (§9.5) | W4 | 1 |
| `device.ddi.list` | read | | `{platform?: ios}` → `{ddis: [{ddiBuild, xcodeBuild?, forms[], cryptexVersion?, coreDeviceVersion?, source{kind, path?}, verified, importedAt, preferred}]}`, newest `ddiBuild` first (the analogue of `devicectl list preferredDDI`) | W10 | 2 |
| `device.ddi.mount` | device | L | `{udid?, source?: {kind: imported, ddiBuild?} \| {kind: dir, path} \| {kind: pmd3-cache} \| {kind: pmd3-auto-mount, allowMirror}, force?}` → `DdiStatus`. Default: the `[device.ddi]` order. `pmd3-cache` and `pmd3-auto-mount` are mirror-derived and need the `allow-mirror` consent (3022 otherwise). A mounted DDI is a no-op unless `force`, which replaces it; on the rig `force` is used only with the owner's approval [D 62]. Every mount Russet performs (here, in `device.prepare` and in `ensure_ddi`) writes the `MountRecord` of §5.3 to `state/devices/<udid>/ddi.json` [D 89] | W4 (`dir`) / W10 (the rest) | 1 / 2 |
| `device.reserve` | device | | `{udid?, purpose}` → `{leaseId}`; others get `device.reserved`, except connections that present the lease as `sharedLease` (§6.3) | W4 | 1 |
| `device.alias.set` | system | | `{udid, alias}` → `{}` (global config) | W4 | 1 |
| `device.wifi.enable` | device | | `{udid}` → `{}` | W30 | 6 |
| `app.list` | read | | `{udid?, kind?: user\|all, bundleIdPrefix?}` → `{apps: AppInfo[]}` | W4 | 1 |
| `app.install` | device | L | `{udid?, path: .app\|.ipa}` → `{bundleId, installedPath, durationMs}` | W4 | 1 |
| `app.uninstall` | device | | `{udid?, bundleId}` → `{}` (MCP: own or `dev.russet.test.*` IDs only) | W4 | 1 |
| `app.launch` | device | | `{udid?, bundleId, args?, env?, suspended?, killExisting?, console?}` → `{pid, executable, leaseId}` ⇒ `app.output`, `app.exited` | W4 | 1 |
| `app.terminate` | device | | `{udid?, pid?\|bundleId?}` → `{}` | W4 | 1 |
| `app.icon` | read | | `{udid?, bundleId}` → `{pngBase64}` | W10 | 2 |
| `process.list` | read | | `{udid?}` → `{processes: ProcessInfo[]}` | W4 | 1 |
| `process.signal` | device | | `{udid?, pid, signal}` → `{}` | W4 | 1 |
| `forward.open` | device | | `{udid?, target: {service}\|{tunnelPort}\|{usbmuxPort}, bind?: {tcp}\|{unix}}` → `{leaseId, local}` | W4 | 1 |
| `forward.close` | device | | `{leaseId}` → `{}` | W4 | 1 |
| `forward.list` | read | | `{}` → `{forwards[]}` | W4 | 1 |
| `logs.subscribe` | read | | `{udid?, sources: (oslog\|syslog\|stdout)[], filter: LogFilter, backlog?}` → `{subscriptionId}` ⇒ `logs.entries` (`LogEntry`, §9.5) | W4 | 1 |
| `logs.tail` | read | | `{udid?, bundleId?, pid?, lines?, minLevel?, contains?, since?, cursor?}` → `{entries: LogEntry[], truncated, cursor?}` | W4 | 1 |
| `screen.screenshot` | read | | `{udid?, format?: png\|jpeg, maxDimension?, scale?, quality?, toFile?}` → `{imageBase64?\|path?, format, width, height, orientation}` | W10 | 2 |
| `screen.mirror.start` | read (`input:true` needs device) | | `{udid?, codecs: (hevc\|avc\|jpeg\|poll)[], maxFps?, input?}` → `{streamId, url, token, codec, width, height}` | W10 | 2 |
| `screen.mirror.stop` | read | | `{streamId}` → `{}` | W10 | 2 |
| `screen.elements` | read | | `{udid?, source?: accessibility\|wda}` → `{elements: UiElement[], source}` [D 23] | W10 | 2 |
| `screen.press` | device | | `{udid?, elementId}` → `{}` | W10 | 3 |
| `input.tap` | device | | `{udid?, x, y, space?: normalized\|pixels\|points\|screenshot, holdMs?}` → `{}` [D 51] | W10 | 2 |
| `input.swipe` | device | | `{udid?, from{x,y}, to{x,y}, space?, durationMs?}` → `{}` | W10 | 2 |
| `input.button` | device | | `{udid?, button: home\|lock\|volume-up\|volume-down\|mute\|siri, action?: press\|down\|up}` → `{}` | W10 | 2 |
| `input.text` | device | | `{udid?, text, submit?}` → `{}` | W10 | 2 |
| `input.key` | device | | `{udid?, key\|usage, down?}` → `{}` | W10 | 2 |
| `files.list`, `files.pull` | read | L (pull) | `{udid?, scope: {kind: media\|app-container\|app-documents\|crash-reports, bundleId?}, path, local?}` | W10 | 2 |
| `files.push`, `files.remove` | device | L (push) | same | W10 | 2 |
| `perf.subscribe` | read | | `{udid?, pid?\|bundleId?, intervalMs?, metrics?: (cpu\|memory\|energy\|network\|fps)[]}` → `{subscriptionId}` ⇒ `perf.samples` | W10 | 2 |
| `perf.export` | read | | `{subscriptionId, path, format: chrome-trace\|jsonl\|csv}` → `{path}` | W10 | 3 |
| `location.set`, `location.clear` | device | | `{udid?, lat, lon}` / `{udid?}` → `{}` | W10 | 2 |

### 8.3 Project, build, assets, Flutter build [build §9.4]

| Method | Class | L | Params → Result | WS | Wave |
|---|---|---|---|---|---|
| `project.detect` | read | | `{dir}` → `{stack, root, products[], manifests[]}` | W5 | 1 |
| `project.info` | read | | `{project}` → `{stack, root, targets[{name, bundleId, kind}], configurations[], defaultTarget}` | W5 | 1 |
| `project.init` | project | | `{dir, stack, name, bundleId}` → `{files[]}` | W16 | 2 |
| `project.infoPlist` | read | | `{project, target?, configuration?}` → `{plist, provenance{key: source}}` | W5 | 1 |
| `build.start` | build | L | `{project, target?, configuration?, package?: none\|ipa-dev\|ipa-store, backend?, overrides?, clean?, bundleIdPolicy?}` → `BuildOutput` | W5 | 1 |
| `build.plan` | read | | as `build.start` → `BuildPlan` | W5 | 1 |
| `build.settings` | read | | `{project, target?, configuration?, keys?}` → `{settings, sources}` | W16 | 2 |
| `build.clean` | build | | `{project, scope: build\|all}` → `{freedBytes}` | W5 | 1 |
| `build.inspect` | read | | `{path: .app\|.ipa}` → `{infoPlist, machO[], lints[], dt, car?}` | W5 | 1 |
| `assets.list` | read | | `{catalog}` → `{assets[{name, kind, variants[]}]}` | W21 | 3 |
| `assets.compile` | build | | `{catalog, outDir, appIcon?, accentColor?, deploymentTarget}` → `{outputs[], partialInfoPlist, diagnostics[]}` | W21 | 3 |
| `assets.appIcon` | project | | `{source1024, catalog, name?}` → `{iconSet}` | W21 | 3 |
| `assets.render` | read | | `{catalog, name, scale?, appearance?}` → `{pngBase64}` | W21 | 3 |
| `ib.compile` | build | | `{input: .storyboard\|.xib, outDir, module?, deploymentTarget, devices: (iphone\|ipad)[], mode?: compile\|link}` → `{outputs[], partialInfoPlist?, diagnostics[]}`: the engine behind the `ibtool` shim; an unsupported element gives 4101 with file:line, never partial output [D 75] | W21b (launch storyboards, plus a single-scene main storyboard with an ObjC custom class) / W28b (main storyboards, XIBs) / W28c (tables, collections, traits) | 4 / 6 |
| `ib.dump` | read | | `{path: .nib\|.storyboardc}` → `{graph}` (the normalized decoded object graph of §19) | W21b | 4 |
| `flutter.register` | system | | `{udid?, all?}` → `{registered[{udid, flutterDeviceId, configPath}]}` [D 41] | W17 | 3 |
| `flutter.device.install` | device | L | `{udid, appName, bundlePath}` → `{bundleId}` (build Runner → sign → install) | W17 | 3 |
| `flutter.genSnapshot.ensure` | system | L | `{flutterRoot?, mode}` → `{path, engine, sha256}` | W24 | 4 |

### 8.4 Accounts, signing, provisioning, App Store Connect [SIGN §10]

| Method | Class | L | Params → Result | WS | Wave |
|---|---|---|---|---|---|
| `account.list` | read | | `{}` → `{accounts: Account[], default?}` | W6 | 1 |
| `account.addAscKey` | secrets | | `{keyId, issuerId?, p8Path, label?}` → `Account` (cli/ide only) | W6 | 1 |
| `account.importXtool` | secrets | | `{dryRun?}` → `{accounts[], identities[], warnings[]}` (cli/ide only; explicit one-time import [D 50]) | W6 | 1 |
| `account.verify` | read | | `{accountId?}` → `{ok, teamId?, roleProbe{certificates, profiles, builds}}` | W6 | 1 |
| `account.remove` | secrets | | `{accountId, wipeSecrets?}` → `{}` | W12 | 2 |
| `account.setDefault` | secrets | | `{accountId}` → `{}` | W12 | 2 |
| `account.appleId.begin` | secrets | | `{appleId, password, anisette: local\|{remote}}` → `{sessionId, step: done\|two-factor, twoFactor?}` (cli/ide only) | W22 | 3 |
| `account.appleId.requestCode`, `account.appleId.submitCode` | secrets | | `{sessionId, via, phoneId?}` / `{sessionId, code}` → `{}` / `Account` | W22 | 3 |
| `account.anisette.status` | read | | `{}` → `{kind, librariesPresent, apkVersion?, provisioned}` | W22 | 3 |
| `account.anisette.fetchLibraries` | system | L | `{consent: true}` → `{apkVersion, bytesDownloaded}` | W22 | 3 |
| `identity.list` | read | | `{team?}` → `{identities: SigningIdentity[]}` | W6 | 1 |
| `identity.importP12`, `identity.exportP12` | secrets | | `{path, passwordPrompt: true}` / `{identityId, out}` → `SigningIdentity` / `{}` | W12 | 2 |
| `portal.certificates.list`, `portal.devices.list`, `portal.bundleIds.list`, `portal.profiles.list` | read | | `{accountId?, filter?}` → records (UDIDs masked unless `reveal` from ide) | W6 | 1 |
| `portal.devices.register` | account | | `{accountId?, udid, name}` → `ProvisionPlan` (applied per policy) [D 23] | W12 | 2 |
| `portal.certificates.revoke` | account-destructive | | `{accountId?, certId}` → `{}` (typed confirmation; cli/ide only) | W12 | 2 |
| `provision.plan` | read | | `{project, distribution?: development\|ad-hoc\|app-store-connect, devices?, accountId?}` → `ProvisionPlan` | W6 | 1 |
| `provision.apply` | account | L | `{planId, approval?}` → `ProvisionOutcome` (a zero-mutation plan needs no approval) | W6 (zero-mutation) / W12 | 1 / 2 |
| `provision.capabilities` | read | | `{accountId?}` → `{capabilities[{id, title, entitlements, freeTeam, needsPortalStep, docs}]}` | W12 | 2 |
| `sign.run` | build | L | `{path, distribution, identityId?, profiles?{relPath: path}, mergePolicy?, output?}` → `SignReport` | W6 | 1 |
| `sign.verify`, `sign.lint`, `sign.inspect` | read | | `{path, profile?: device-install\|app-store}` → `VerifyReport` / `LintFinding[]` / `SignatureInfo[]` | W6 | 1 |
| `asc.get` | read | | `{pathAndQuery}` → `{status, body}` (allowlisted GET; request-scoped token) | W12 | 2 |
| `ship.archive` | build | L | `{project, configuration?, method}` → `Archive` | W12 | 2 |
| `ship.ipa` | build | | `{archivePath, method}` → `IpaInfo` | W12 | 2 |
| `ship.stamp.plan` | read | | `{archivePath, policy}` → `StampPlan` | W23 | 4 |
| `ship.stamp.apply` | project | | `{planId}` → `StampReport` (needs recorded consent, §16.4) | W23 | 4 |
| `ship.validate` | read | | `{ipaPath, profile?}` → `ValidationReport` | W23 | 4 |
| `ship.upload` | publish | L | `{ipaPath, appId?, wait?: upload\|valid, approval?}` → `UploadOutcome` | W23 | 4 |
| `ship.upload.resume` | publish | L | `{uploadId, approval?}` → `UploadOutcome` | W23 | 4 |
| `ship.upload.status` | read | | `{uploadId}` → `UploadOutcome` | W23 | 4 |
| `ship.builds.list` | read | | `{appId?\|bundleId?, limit?}` → `{builds[]}` | W23 | 4 |
| `ship.testflight.groups` | read | | `{appId}` → `{groups[{id, name, internal, publicLink?, testerCount}]}` | W23 | 4 |
| `ship.testflight.testers` | read | | `{appId, groupId?}` → `{testers[{id, email (masked for mcp), state, groups[]}]}` | W23 | 4 |
| `ship.testflight.groups.create` | publish | | `{appId, name, internal, publicLinkEnabled?, publicLinkLimit?}` → group (POST `betaGroups`). Russet never deletes groups [SIGN §9.7]. Never exposed to MCP [D 64] | W23 | 4 |
| `ship.testflight.addTesters`, `ship.testflight.removeTesters` | publish | | `{groupId, testers[{email, firstName?, lastName?}]}` / `{groupId, testerIds}` → records (POST `betaTesters`; POST/DELETE `betaGroups/{id}/relationships/betaTesters`). Never exposed to MCP | W23 | 4 |
| `ship.testflight.distribute` | publish | | `{buildId, groupIds, whatToTest?, usesNonExemptEncryption?, notify?, approval?}` → records: PATCH `builds/{id}` (export compliance, when given), POST `builds/{id}/relationships/betaGroups`, POST `betaBuildLocalizations`, POST `buildBetaNotifications` (when `notify`) | W23 | 4 |
| `ship.testflight.betaReview.submit` | publish | | `{buildId}` → `{submissionId, state}` (POST `betaAppReviewSubmissions`; needed before external groups see a build). Never exposed to MCP | W23 | 4 |
| `ship.metadata.pull`, `ship.metadata.plan` | read | | `{appId, version, dir}` → files / plan | W23 | 4 |
| `ship.metadata.apply` | publish | L | `{planId, approval?}` → job (never exposed to MCP) | W23 | 4 |
| `ship.version.plan` | read | | `{appId, versionString, buildId, platform?: ios, usesNonExemptEncryption?: bool, releaseType?: manual\|after-approval\|scheduled, earliestReleaseDate?}` → `VersionPlan {planId, digest, mutations[]}`: create or reuse the `appStoreVersion`, attach the build, set export compliance on the build; no compliance PATCH is planned when the build's Info.plist already declares `ITSAppUsesNonExemptEncryption` (§14.2 `[ship]`) [SIGN §9.8] [D 64] | W23 | 4 |
| `ship.version.apply` | publish | L | `{planId}` → `{versionId, state}` (POST `appStoreVersions`, PATCH `…/relationships/build`, PATCH `builds/{id}` `usesNonExemptEncryption`; never exposed to MCP) | W23 | 4 |
| `ship.review.submit` | publish, `Confirm::Typed` | | `{versionId, confirmText?}` → `{reviewSubmissionId, state}` (POST `reviewSubmissions`, POST `reviewSubmissionItems`, PATCH `reviewSubmissions/{id}` `submitted: true`). Typed confirmation of `"<app name> <version>"`; cli/ide only; never exposed to MCP | W23 | 4 |
| `ship.review.status` | read | | `{appId? \| versionId?}` → `{submissions[{id, state, submittedAt?, items[]}]}` | W23 | 4 |
| `ship.ci.init` | project | | `{provider: github\|xcode-cloud, signing?: russet\|xcode-manual\|xcode-cloud}` → `{files[]}`: the workflow plus its `.russet/ci/` helpers (§17.5); the runner label and action SHAs come from `pins` `[ci.github]` [D 76] | W23 | 4 |
| `ship.ci.dispatch` | publish | | `{ref?, approval?}` → `{runId}` | W23 | 4 |
| `ship.ci.status`, `ship.ci.fetch` | read | | `{runId}` → status / `{paths[]}`. `ship.ci.status` also checks that the pinned runner label and Xcode still exist (7010); `ship.ci.fetch` deletes the run's artifacts after the download (Free plans have 500 MB of artifact storage) | W23 | 4 |
| `ship.crashes.list`, `ship.crashes.get` | read | | `{appId?\|bundleId?, limit?}` / `{submissionId, symbolicate?}` | W23 | 4 |
| `ship.compare` | read | | `{a, b}` → `IpaDiff` | W23 | 4 |

### 8.5 Run, debug, crashes, Flutter run [RD §18] [D 24]

| Method | Class | L | Params → Result | WS | Wave |
|---|---|---|---|---|---|
| `run.start` | device | L | `{project, target?, configuration?, udid?, args?, env?, build?: true, install?: if-changed\|always\|never, launch?: auto\|coredevice\|dvt\|none, sessionLease?: owned\|detached}` → `{sessionId?, pid?, bundleId, installed}` | W7 | 1 |
| `run.list` | read | | `{}` → `{sessions: SessionSummary[]}` (§9.5; every session kind, detached ones included) | W7 | 1 |
| `run.subscribe` | read | | `{sessionId?}` → `{subscriptionId}` ⇒ `run.state`, `run.output`, `run.exited`, `run.jit` | W7 | 1 |
| `run.output.query` | read | | `{sessionId, sinceMs?, limit?, filter?}` → `{entries: LogEntry[], dropped}` | W7 | 1 |
| `run.stop` | device | | `{sessionId, mode?: kill\|detach}` → `{}` | W7 | 1 |
| `run.relaunch` | device | L | `{sessionId}` → `{pid, ms}` (Swift Quick Relaunch) | W11 | 2 |
| `run.hotReload` | device | | `{sessionId, kind?: reload\|restart}` → `{supported, kind, ms, message?}` | W11 (Swift) / W18 (Flutter) | 2 / 3 |
| `debug.prepare` | device | L | `run.start` params + `{stopOnEntry?, frontend?: dap\|lldb-cli\|none, jit?, symbols?, flavor?: swift\|llvm, attachMode?: broker\|legacy-async}` → `PreparedDebug {sessionId, pid, lldb: ToolInfo, lldbDap: ToolInfo, broker{transport, path?\|port?, lldbUrl}, initCommands[], attachCommands[], dapConfig, dap?{transport, path}}` | W7 | 1 |
| `debug.attach` | device | L | `{udid?, pid?\|bundleId?, frontend?, sessionLease?}` → `PreparedDebug` | W11 | 2 |
| `debug.control` | device | | `{sessionId, action: continue\|pause\|step-over\|step-into\|step-out, threadId?, waitForStopMs?}` → `{state, stop?{reason, threadId, frames[]}}` | W11 | 2 |
| `debug.inspect` | read | | `{sessionId, what: threads\|stack\|variables, threadId?, frameId?, scope?, depth?}` → DAP-shaped data | W11 | 2 |
| `debug.evaluate` | device | | `{sessionId, expression, frameId?, context?: repl\|watch}` → `{result, type}` | W11 | 2 |
| `debug.breakpoint.set` | device | | `{sessionId?, file, line, condition?, logMessage?}` → `{breakpointId, verified, pending}` | W11 | 2 |
| `debug.breakpoint.remove` | device | | `{breakpointId}` → `{}` | W11 | 2 |
| `debug.jitStats` | read | | `{sessionId}` → `JitStats` (§9.5; `pagesTouched` must equal `pagesNotified`) | W18 | 3 |
| `debug.symbols.status`, `debug.symbols.plan` | read | | `{udid?}` → `{build, dscUuid?, tier, bytesOnDisk}` / `{files[], totalBytes, freeBytes, quotaBytes, ok, reason?}` | W11 | 2 |
| `debug.symbols.fetch` | device | L | `{udid?, tier: core\|full, confirmBytes?}` → `SymbolSet` (interactive: `ui.confirm`; headless and MCP must pass `confirmBytes`) | W11 | 2 |
| `debug.symbols.prune` | system | | `{maxBytes?, build?}` → `{freedBytes}` | W11 | 2 |
| `crash.list` | read | | `{bundleId?, since?, limit?}` → `{crashes: CrashSummary[]}` (local store) | W11 | 2 |
| `crash.fetch` | device | L | `{udid?, bundleId?, since?}` → `{fetched: CrashSummary[]}` | W11 | 2 |
| `crash.symbolicate` | read | | `{crashId?\|path?, format?: json\|text}` → `{crash}` | W11 | 2 |
| `flutter.run` | device | L | `{project, udid?, mode?, args?, sessionLease?}` → `{sessionId, vmService{wsUri, httpUri}, devtoolsUri?}` | W18 | 3 |
| `flutter.attach` | device | | `{sessionId}` → `{vmService}` | W18 | 3 |
| `flutter.device.runDebug` | device | L | `{udid, appName, mode, engineOptions}` → `{sessionId}`; app stdout is printed by the CLI | W18 | 3 |
| `flutter.device.forward` | device | | `{udid, devicePort, hostPort}` → `{leaseId, local}` | W18 | 3 |
| `test.list` | read | | `{project, target?}` → `{testTargets[{name, kind: unit\|ui, cases[]}]}` | W32 | 4 |
| `test.run` | device | L | `{project, target?, udid?, filter?, ui?: bool, junitPath?}` → `TestReport {passed, failed, skipped, durationMs, cases[{name, status, durationMs, failure?{message, file?, line?}}], junitPath?}`: build the `.xctest` bundle and its host app, sign, install, run through idevice `xctest` (testmanagerd; needs the DDI) | W32 (SwiftPM unit tests) / W28 (Xcode projects, XCUITest) | 4 / 6 |

### 8.6 MCP administration [IDE §5.5]

| Method | Class | Params → Result | WS | Wave |
|---|---|---|---|---|
| `mcp.status` | read | `{}` → `{stdio{command,args}, http?{url, enabled, listeners[]}, clients[{id, registered, scope, path}]}` | W13 | 2 |
| `mcp.install`, `mcp.uninstall` | system | `{client, scope?: user\|project, project?, dryRun?, tierMax?}` → `{changes[]}` | W13 | 2 |
| `mcp.http.set` | system | `{enabled, listen?, exposeDocker?, clientToken?}` → `{url, token?}` (token shown once) | W20 | 3 |
| `mcp.policy.get`, `mcp.policy.set` | read / system | `{client?}` / `{client, tierMax?, tools?}` → policy | W20 | 3 |

### 8.7 CLI command tree (maps onto §8; each area owns its `cmd/<area>.rs`)

Global flags: `--json` (one document, or NDJSON for streams), `--device <udid|alias>`, `--project <dir>`, `-v`/`-q`, `--color`, `--no-daemon`, `--yes`.

```text
russet doctor [--fix] [--online] [--strict] [--check ID]…                      doctor.run                    W3
russet daemon run|start|stop|restart|status|logs|path                          daemon.*                      W2
russet config get|set|unset|list|path|edit [--scope …]                         config.*                      W3
russet secrets list|status · toolchain list|install|uninstall|gc|pins update · sdk list|import|remove        W3/W16b
russet sdk import <Xcode.xip|Xcode.app|bundle|XcodeSystemResources.pkg|iOS_DDI.dmg|Device_Support.dmg|DDI-dir>
           [--ddi-only|--sdk-only] [--full] [--delete-source] [--dry-run]                                W16b
russet sdk device-support check|fetch   (deferred, not v1; §21 Q25)                                     W33
russet account list|default|remove|verify|add-key|import --from-xtool|login
               anisette status|fetch-libraries|reset · certs|devices|bundle-ids|profiles …               W6/W12/W22
russet provision [--distribution development|ad-hoc|app-store-connect] [--dry-run]                         W6/W12
russet sign <path> · sign verify|lint|inspect <path>                                                        W6
russet device list|info|prepare|pair|unpair|reserve|alias · device ddi list|status|mount [--dir D|--imported [DDI_BUILD]] [--force]   W4/W10
russet app list|install|uninstall|terminate · app launch <bundle> [--suspended] [--kill-existing] [--console]
           [--env K=V]… [-- app args] (prints {pid, executable, leaseId}) · install <app|ipa> · launch <bundle>   W4
russet process list|signal · forward (--service NAME | --tunnel-port N | --usbmux-port N) [--bind tcp:ADDR|unix:PATH]  W4
russet logs [--app B | --session S | --pid N] [--level L] [--grep RE] [--follow] [--duration D]            W4/W7
           (--json: NDJSON, one LogEntry (§9.5) per line; --session reads run.output.query, then follows for D)
russet screenshot [-o f] · mirror [--stdout|--record f] · tap|swipe|button|type · files … · perf … · location …  W10
russet build [--release|--profile P] [--ipa[=dev|store]] [--backend B] [--clean] [--set K=V]…              W5
russet clean [--all] · build-settings · inspect <path> · lint <path> [--store] · package ipa <app> -o <ipa> W5/W16
russet assets compile|app-icon · strings compile · ib convert-launch-screen|compile|dump                   W21/W21b/W28b/W28c
russet run [--configuration C] [--no-build] [--install if-changed|always|never] [--launch auto|coredevice|dvt]
           [--detach | --background] [--exit-code app] [-- app args]                                        run.start            W7
           (--detach: wait for the launch, print {sessionId, pid, bundleId}, exit; the session stays, §7.6)
russet sessions (--json: {sessions: SessionSummary[]}) · stop [<session>|--all] · relaunch [<session>] · reload [<session>] [--restart]   run.*   W7/W11
russet debug [--break FILE:LINE]… [--stop-on-entry] [--attach [--pid N|--bundle B]] [--attach-mode broker|legacy-async]
             [--jit …] [--symbols …] [--print-config] [--batch --command CMD…] [--detach] [--lldb PATH] [-- app args]   debug.prepare   W7
russet debug prepare --json · debug jit-bench                                                               W7/W18
russet crash list|show|fetch|symbolicate · symbols status|plan|fetch|prune                                  W11
russet flutter register|build|run · flutter device ping|install|uninstall|run-debug|forward|screenshot
               flutter gen-snapshot ensure|list                                                             W17/W18/W24
russet xcode adapt|dump-pif|show-build-settings                                                             W28
russet test [--filter F] [--junit out.xml] [--ui]   (SwiftPM XCTest on device: W32; Xcode projects and XCUITest: W28)   W32/W28
russet ship archive|ipa|stamp|validate|upload|builds|testflight|metadata|ci|crashes|compare                 W12/W23
russet ship version plan|apply --version V --build B [--encryption exempt|non-exempt] [--release manual|after-approval]
russet ship submit <versionId|--version V> [--confirm-text "<App> <V>"] [--dry-run] · ship review status      W23
russet ship testflight groups [create NAME [--external]] · testers [--group G] · add-testers|remove-testers --group G EMAIL…
                  · distribute BUILD --group G… [--what-to-test F] [--encryption exempt|non-exempt] [--notify] · beta-review BUILD   W23
russet ship ci init github [--signing russet|xcode-manual|xcode-cloud] · ci init xcode-cloud · ci run|status|fetch          W23
russet mcp serve [--client ID] [--http ADDR] · mcp tools|install|uninstall|status · mcp http enable|disable W13/W20
russet approvals list · approve <id> · deny <id> [--reason …]   (TTY only)                                  W2
russet audit tail [--kinds rpc|approval|account-mutation,…] [--client C] [--limit N] [--follow]              W2
russet self update|rollback · completions · bugreport · cache gc                                            W26/W2/W16
```

- `russet debug --batch` [D 84]: Russet's script attaches, sets the `--break` breakpoints, runs `process continue` to the first stop, runs each `--command` there, and then ends with `process detach` (`--detach`) or `process kill` (default). LLDB runs batch commands synchronously, so a `process continue` among the `--command`s blocks until the next stop or exit; use `--detach` to leave the app running [V lead 2026-10-11: with Swift 6.4's LLDB 21.0.0, `lldb -b -o 'process launch --stop-at-entry' -o 'process continue' -- /usr/bin/sleep 4` returned only after the process exited, while the same run ending in `process detach` returned with `sleep 20` still running].
- `russet debug --batch --json` prints `{sessionId, pid, stop{reason, threadId, frame{function, file, line}}, commands[{command, output}], detached, exitedAt?}`. W9's exit script relies on it.
- Mutating commands print the plan and ask for confirmation. Non-interactive runs need `--yes`, otherwise they exit 11.

---

## 9. Notifications and callbacks

### 9.1 Notifications (daemon → client)

| Notification | Params | Delivered to | WS |
|---|---|---|---|
| `$/progress` | LSP work-done (§7.6) | the requester | W2 |
| `daemon.shuttingDown` | `{reason: idle\|request\|upgrade\|signal, graceMs}` | all | W2 |
| `daemon.logBatch` | `{subscriptionId, records[{ts, level, target, message, fields}], dropped?}` | subscribers | W2 |
| `job.updated` | `{subscriptionId, job: JobInfo}` (every state change, including the result summary) | subscribers | W2 |
| `job.events` | `{subscriptionId, jobId, events: JobEvent[], dropped?}` (batched ≤ 50 ms) | subscribers | W2 |
| `config.changed` | `{scope, path}` | all | W3 |
| `toolchain.changed` | `{components[]}` | all | W3 |
| `device.changed` | `{subscriptionId, change: added\|removed\|updated, device: DeviceSummary}` | subscribers | W4 |
| `logs.entries` | `{subscriptionId, entries: LogEntry[], dropped?}` | subscribers | W4 |
| `app.output` | `{leaseId, stream: stdout, data}` | the `app.launch` caller | W4 |
| `app.exited` | `{leaseId, pid, code?, signal?, crashId?}` | the `app.launch` caller | W4 |
| `perf.samples` | `{subscriptionId, samples[]}` | subscribers | W10 |
| `screen.mirror.ended` | `{streamId, code: media-in-use\|device-gone\|stream-failed, message}` | the starter | W10 |
| `run.state` | `{subscriptionId, sessionId, state, detail?}` | subscribers | W7 |
| `run.output` | `{subscriptionId, sessionId, entries: LogEntry[], dropped?}` | subscribers | W7 |
| `run.exited` | `{subscriptionId, sessionId, code?, signal?, crashId?}` | subscribers | W7 |
| `run.jit` | `{subscriptionId, sessionId, …JitStats}` | subscribers | W18 |
| `approval.requested` | `{approval: Approval}` | `ide` connections and `cli` connections with `interactive` | W2 |
| `approval.resolved` | `{approvalId, state}` | the same, plus the requester | W2 |
| `account.event` | `{kind: token-expiring\|profile-expiring\|free-quota, accountId, detail}` | `ide`, `cli` | W12 |
| `mcp.toolCall` | `{client, tool, tier, argsSummary, outcome, durationMs}` | `ide` | W13 |
| `ui.showMessage` | `{level: info\|warning\|error, message, actions?}` | the target client | W2 |

These names replace `job.progress`, `job.done`, `job.output`, `build.event`, `build.diagnostics`, `debug.state`, `debug.output`, `debug.exited`, `device.added/removed`, `logs.batch`, `perf.sample`, `profile.samples` and the single `event` envelope [D 2].

### 9.2 `JobEvent`

`{seq, ts, kind, …}`; `kind` is one of the values below. Areas add fields only inside their own kinds.

| `kind` | Fields | Produced by |
|---|---|---|
| `log` | `stream: stdout\|stderr\|info\|warn, line` | every job |
| `progress` | `done?, total?, fraction?, message?` | every job |
| `stage` | `stage: resolve\|prepare\|compile\|resources\|assemble\|lint\|package\|sign\|install\|launch\|attach, state: started\|finished\|failed` | build, run |
| `task` | `taskId, title, tool?, state, durationMs?` | build |
| `diagnostic` | `severity: error\|warning\|note, message, file?, line?, column?, tool?, code?` | build, sign, ship |
| `artifact` | `kind: app\|ipa\|dsym\|archive\|report, path` | build, ship |
| `warning` | `russetCode, message, hint?` | every job |
| `transfer` | `bytesDone, bytesTotal, part?` | install, upload, symbols, toolchain |
| `custom` | `name, data` | escape hatch; documented in the area doc |

### 9.3 Server-to-client requests [core §7.10]

| Method | Params | Result | Served by |
|---|---|---|---|
| `ui.confirm` | `{title, message, detail?, danger, requestedBy{kind,name}, timeoutMs}` | `{confirmed}` | `cli` (TTY), `ide` |
| `ui.prompt` | `{title, message, kind: text\|password\|code\|typed-confirmation, secret, expected?, requestedBy, timeoutMs}` | `{value?, cancelled}` | `cli`, `ide` only; secret prompts are never sent to `mcp` |
| `ui.openUrl` | `{url, reason}` | `{opened}` | `cli`, `ide` |

### 9.4 IDE reverse requests [IDE §5.4] [D 36]

The daemon sends these only to `ide` connections that declared `capabilities.ideDebug`.

| Method | Params → Result | Used when |
|---|---|---|
| `ide.debug.start` | `{configuration: RussetDebugConfiguration}` → `{sessionId}` | An agent calls `debug_start` while an IDE window shows the project, so the human sees the session |
| `ide.breakpoints.add` | `{file, line, condition?, logMessage?}` → `{id}` | An agent sets a breakpoint while an IDE is attached; VS Code stays the only writer of `setBreakpoints` |
| `ide.breakpoints.remove` | `{id}` → `{}` | same |
| `ide.reveal` | `{file, line?, column?}` → `{}` | "Show me" actions |

`ide.debug.request`, `ide.debug.stop` and `ide.debug.event` from the IDE draft are dropped. Agents drive execution through the DAP hub (§10.2).

### 9.5 Shared wire types [D 52, D 72]

These shapes are binding for every producer (device, run/debug) and consumer (CLI, IDE, MCP). Area-doc variants are superseded (§20.1).

```jsonc
// LogEntry: logs.entries, logs.tail, run.output, run.output.query
{"ts":"2026-10-11T09:00:00.123Z", "source":"oslog",       // LogSource: stdout | stderr | oslog | syslog | debugger | jit | russet
 "level":"default",                                       // debug | info | default | error | fault (Apple's names; never "notice")
 "message":"…",                                           // never "text"
 "pid":932, "tid":1, "process":"RussetProbe", "subsystem":"dev.russet.probe", "category":"ui",
 "imageUuid":"…", "imageOffset":4096, "machTs":123456789}  // all optional fields omitted when unknown
// DeviceSummary: device.list, device.changed
{"udid":"00008101-…", "name":"iPhone", "productType":"iPhone13,2", "marketingName":"iPhone 12",
 "osVersion":"27.0", "build":"24A437", "transport":"usb",                 // usb | wifi
 "codeSigningMonitor":"txm",                              // txm | ppl | none | unknown; optional (§17.3)
 "state":"ready",                                         // attached | needs-trust | trust-pending | ready | unavailable (flat, kebab-case)
 "readiness":{"paired":true, "developerMode":"yes", "locked":"unknown",  // yes | no | unknown
              "ddi":{"mounted":true, "kind":"cryptex1", "version":"27.1.266.1", "productBuild":"27A266a", "source":"dir"},
              "tunnel":{"state":"up", "generation":3, "since":"…"}},     // state: down | up | failed (+ "error": ErrorData)
 "error":{…ErrorData…}}                                   // only when state = unavailable
// JitStats: debug.jitStats, run.jit (wire names; pagesTouched == pagesNotified on a healthy session)
{"stops":41, "pagesNotified":1312, "pagesTouched":1312, "bytesWritten":5640, "p50Ms":0.4, "p95Ms":1.9, "rttMs":0.9, "mode":"safe"}
// SessionSummary: run.list, `russet sessions --json` [D 84]
{"sessionId":"ses_01J…", "kind":"debug",                  // run | debug | flutter
 "bundleId":"dev.russet.test.probe", "udid":"00008101-…", "pid":932, "project":"/home/u/RussetProbe",
 "state":"running",                                       // SessionState, below
 "lease":"detached",                                      // owned | detached (§6.3)
 "debugger":"detached",                                   // none | attached | detached
 "attachMode":"broker",                                   // broker | legacy-async; debug sessions only
 "startedAt":"…", "endedAt":"…", "exit":{"code":0, "signal":9, "crashId":"…"}}   // endedAt and exit only once the session has ended
```

- **`SessionState`** (kebab-case; RD §17.2's Rust enum converts to it): `preparing`, `building`, `installing`, `launching`, `attaching`, `arming` (JIT hook), `ready` (launched suspended, waiting for a debugger), `running` (the app runs, with or without a debugger attached), `stopped` (stopped in the debugger), then the terminal `exited`, `crashed`, `lost` (device or tunnel gone) and `failed` (`run.exited` carries the details). Whether a debugger is attached is `debugger`, not a state: RD's `Detached` state is superseded (§20.1).
- `DdiStatus.kind` is `legacy | personalized-dmg | cryptex1`; `source` is `imported | dir | pmd3-cache | pmd3-auto-mount | unknown`. A DDI found mounted reports the source of Russet's last `MountRecord` for that device (§5.3, `state/devices/<udid>/ddi.json`) while the mounted cryptex version and product build still match it, otherwise `unknown`. Matching versions alone cannot attribute a source, because the Wave-1 directory copy and an import of the same Xcode are the same bytes [D 89].
- `codeSigningMonitor` comes from a ChipID × iOS-major table generated from public BuildManifests (iOS 26: TXM iff ChipID ≥ 0x8110; iOS 27: TXM on every supported chip except 0x8027), optionally confirmed by the probe app's `security.codesigning.monitor` sysctl (0 none, 1 PPL, 2 TXM). It labels doctor output, RDT headers, JIT warnings and test-matrix rows [D 71].

---

## 10. Side channels

### 10.1 Mirror WebSocket `russet.mirror.v1` [IDE §5.10, device §6.8] [D 10]

- **Endpoint.** `screen.mirror.start` returns `url = ws://127.0.0.1:<web>/mirror/v1?stream=<streamId>` and a single-use 32-byte token. The client sends the token in its first message, never in the URL:
  - `{"t":"auth","token":"…"}` within 2 s, else close code 4401;
  - `Origin` must be `vscode-webview://…` or absent (CLI and test clients).
- **Server → client, text:**
  - `config {codec, description (base64 hvcC|avcC), codedWidth, codedHeight, rotation, fpsHint, format:"length-prefixed"}`; a keyframe always follows a `config`;
  - `orientation`, `stats` (1 Hz), `pong`;
  - `end {code: E_MEDIA_IN_USE | E_DEVICE_GONE | E_STREAM_FAILED}`.
- **Server → client, binary:** a 16-byte header `u8 version=1 | u8 kind (1 = access unit, 2 = JPEG) | u8 flags (bit0 key, bit1 discontinuity) | u8 0 | u32 seq LE | u64 ptsMicros LE`, then the payload.
- **Client → server:** `keyframe`, `touch {phase, x, y}` (0..65535, native portrait), `button`, `key`, `text`, `pause`/`resume`, `ping`.
  - Input messages are accepted only on streams started with `input: true` by a connection allowed class `device`.
  - Input shares the device's single display session [V device §2.3 F4].
- **Device side:** CoreDevice `displayservice` RTP/HEVC. The daemon depacketizes (RFC 7798, RFC 6184) and requests an IDR on join (PLI). Backpressure: above 2 MB buffered, drop non-key units and request an IDR.
- **Codec fallback:** HEVC (hardware decode only in Electron [V IDE V10]) → AVC (software decode available) → MJPEG through the user's own `ffmpeg` (never shipped) → screenshot polling.
- **CLI:** `russet mirror --stdout` and `--record f.h265` use the same WebSocket and rewrite length-prefixed NAL units to Annex-B.

### 10.2 DAP hub [RD §9.3] [D 35, D 36]

- **Endpoint:** one hub per debug session.
  - Linux: `runtime/sessions/<sid>/dap.sock` (0600).
  - Windows: `\\.\pipe\russet-dap-<sid>` (user-only DACL).
  - The IDE connects with `vscode.DebugAdapterNamedPipeServer`.
- **Backend:** one daemon-spawned `lldb-dap` with the resolver env (Swift sessions), or `flutter debug_adapter` (Flutter). Mixed sessions get two hubs.
- **Clients:** the IDE (role `ide`) plus in-process agent clients (role `agent`, used by the `debug.*` RPC methods).
  - Sequence numbers are rewritten per client. Responses go to the requester; events are broadcast.
  - Only the `ide` role may send `disconnect`/`terminate`.
- **Breakpoints.**
  - While an IDE is attached, agent breakpoints go through `ide.breakpoints.add`, so VS Code owns `setBreakpoints`.
  - Headless, the hub merges agent breakpoints per source.
- **Custom requests:** `russet/hotReload {full?}`, `russet/relaunch`, `russet/jitStats`, `russet/deviceLogs {sinceMs?}`.
- **Relaunch:** the hub swaps the `lldb-dap` backend and replays the cached breakpoint sets.
- **Fallback (development only):** `DebugAdapterExecutable(lldb-dap, env)` from `debug.prepare`; with it, agents cannot share the session.

### 10.3 Debug broker (gdb-remote) [RD §6]

- **Endpoint.**
  - Linux: `runtime/sessions/<sid>/gdb.sock` (0600), URL `unix-connect://…`.
  - Windows: `127.0.0.1:<ephemeral>`, accept-once, 60 s connect deadline.
- **Clients:** one LLDB client at a time.
- **What the broker does before LLDB connects:**
  1. Handshake upstream with `com.apple.internal.dt.remote.debugproxy`.
  2. Send `QSetDetachOnError` (0 for Russet-launched apps, 1 for attach-to-running).
  3. `vAttach` while the app is still suspended before dyld.
  4. Relay frames with a separate ack state per side.

  LLDB's synchronous `process connect` then returns a stopped process. No Python and no async toggling are needed [V RD V3–V4, host-only; device check D3 in Wave 1].
- **Fallback:** `attachMode: legacy-async` reproduces the M1 sequence (async connect + attach + `SetPlatformFileSpec`), verified on the iPhone [V verified-facts M1] [D 38].

### 10.4 Port forwards [device §6.6]

- **Endpoint.** `forward.open` binds `runtime/fwd-<lease>.sock` (Linux, 0600) or `127.0.0.1:0` (Windows, or when `bind.tcp` is requested).
- **Flutter VM service** forwards always use TCP `127.0.0.1:<hostPort>`, because flutter_tools needs a TCP port.
- **Tunnel generations.** A forward re-resolves RSD ports per tunnel generation. Live connections die on a tunnel rebuild, and that is reported.

### 10.5 Byte-stream side channel on the IPC socket

Reserved, not implemented in v1. [core §7.8] designed it for mirror frames; the WebSocket replaces it. Revisit only if a Rust client needs bulk data that the web endpoint cannot carry [D 54].

---

## 11. MCP

### 11.1 Architecture [IDE §6.1, core §8]

```text
agent CLI ──stdio──► `russet mcp serve --client <id>` (russet-mcp, one process per agent session) ──JSON-RPC (kind=mcp)──┐
OpenHands GUI ──HTTP──► russetd web endpoint /mcp (russet-mcp service in-process; token → client id) ──in-process──────┤
                                                                                                                    ▼
                                         russetd PermissionEngine + ApprovalService + rate limits + audit (§7.5, §11.3)
```

- **`russet-mcp` is a thin translator.**
  - Each tool maps to one RPC method, or to a documented composite (e.g. `build` = `build.start` + `job.wait` ≤ 50 s).
  - Input schemas come from schemars.
  - Every tool carries `_meta {"x-russet-schema": 1, "x-russet-tier": n}`.
- **Enforcement lives in the daemon** (tier checks, approvals, rate limits, audit), so the stdio and HTTP transports cannot diverge.
- **Protocol mapping:**
  - MCP `_meta.progressToken` → RPC `progressToken`;
  - `$/progress` → `notifications/progress`;
  - `notifications/cancelled` → `$/cancelRequest`.

### 11.2 Protocol, transports, identity

- **Protocol versions:** MCP 2026-07-28 (stateless, `server/discover`) and legacy 2025-11-25 (initialize), both through rmcp 3.5.1 [V QA V19, IDE V15].
- **Identity.**
  - stdio: identity is the `--client <id>` argument written into each agent's MCP config; `clientInfo` is untrusted.
  - HTTP: identity comes from the bearer token, which binds to a client id.
- **HTTP endpoint:**
  - off by default; listens on `127.0.0.1`;
  - checks `Origin` and `Host` and requires the token;
  - `russet mcp http enable --expose-docker` also binds the Docker bridge IP and allows `host.docker.internal` (OpenHands GUI; Linux containers need `--add-host=host.docker.internal:host-gateway`) [D 11].
- **Logs** go to stderr and `state/logs/mcp-<pid>.log`, never to stdout.

### 11.3 Tiers and approvals [QA §8.4] [D 5, D 6]

| Tier | Classes | Default | Approval |
|---|---|---|---|
| 0 | `read` | allowed | none |
| 1 | `device`, `build`, `project` | allowed when the client's `tier-max ≥ 1` | none |
| 2 | `account` | listed only when `tier-max ≥ 2` | once per MCP session per method (`scope: session`; "session" is defined below) |
| 3 | `publish` | listed only when `tier-max = 3` | every call (`scope: once`) |
| — | `account-destructive`, `secrets`, `system` | never exposed | — |

**Flow:**
1. A tier-2/3 call without a valid `approval` fails with RPC error `perm.approvalRequired` (1204). `data.details` is `{approvalId, tool?, tier, summary, argsDigest, approveWith: "russet approve apr_… (or Approve in Russet IDE)", expiresAt}`. The bridge turns it into an MCP `isError: true` result with that structured content.
2. The daemon emits `approval.requested`. The IDE shows a notification and the Approval panel, which is reachable only from its own message bus; no command id can grant [IDE §6.4]. A human can also run `russet approve <id>` in a TTY.
3. The agent retries with `approval: "apr_…"`. The daemon checks that the approval:
   - comes from the same client identity and is for the same method;
   - for tier 3, has the same `paramsDigest` (SHA-256 of canonical JSON params without `approval`);
   - has not expired (10 min while pending);
   - for scope `once`, has not been used before.

   Any mismatch → `perm.approvalInvalid` (1205).
4. On 2026-07-28 clients the server may also send `elicitation/create`. That never replaces step 2.

**What "session" means for `scope: session`** [D 67]. MCP 2026-07-28 is stateless, so the binding is Russet's, not the protocol's:
- **stdio:** the bridge's daemon connection id (`c-<n>`). One `russet mcp serve` process is one session; the approval dies with the process.
- **HTTP:** the bearer token's client id plus a TTL, `session-approval-ttl` in `mcp-policy.toml` (default 30 min, at most 8 h). Rotating the token ends it.
- `mcp-session-id` headers and `clientInfo` are never used for binding.

**Rate limits.** Token bucket per (client, method); defaults come from `mcp-policy.toml`, e.g. `ship.upload` 1/10 min. Exceeding → `perm.rateLimited` (1206).

**Audit.** Every MCP call goes to `state/audit.jsonl` (§6.5).

### 11.4 Tool catalog (canonical names) [IDE §6.3] [D 12]

- **Names** are snake_case without a prefix; clients namespace by server, e.g. `mcp__russet__build`. **Arguments and results** are camelCase.
- **Session ids.** The IDE draft's `runId` is `sessionId` everywhere (`ses_…`), the id returned by `run.start` and `debug.prepare`.
- **`udid`** defaults to the IDE-selected or only device. **`project`** defaults to the IDE workspace when an IDE is attached.

| Tool | Tier | RPC | Notes |
|---|---|---|---|
| `device_list` | 0 | `device.list` | UDIDs masked to the last 6 characters unless policy `reveal-udid` |
| `device_info` | 0 | `device.info` + `device.readiness` | |
| `project_info` | 0 | `project.info` | |
| `doctor_run` | 0 | `doctor.run` (no fix) | |
| `job_status` | 0 | `job.wait` (`waitSeconds ≤ 50`) | the continuation for every slow tool |
| `job_log` | 0 | `job.log` | build and upload logs; replaces `russet_build_log` |
| `build_settings` | 0 | `build.settings` | |
| `app_inspect` | 0 | `build.inspect` | |
| `logs_read` | 0 | `logs.tail` or `run.output.query` | wrapped as untrusted |
| `screenshot_take` | 0 | `screen.screenshot` | JPEG, scale 0.5, max dimension 1280 by default; image content block |
| `ui_tree` | 0 | `screen.elements` | wrapped |
| `crashes_list` | 0 | `crash.list` | local store; the daemon auto-fetches after abnormal exits |
| `crash_read` | 0 | `crash.symbolicate` | wrapped |
| `profile_read` | 0 | `perf.subscribe` for 1–30 s | downsampled ≤ 60 samples |
| `debug_inspect` | 0 | `debug.inspect` | |
| `approval_status` | 0 | `approval.get` (`waitSeconds ≤ 50`) | |
| `account_status` | 0 | `account.list` + `account.verify` | team name, key-id suffix, expiries only |
| `signing_status` | 0 | `provision.plan` + `sign.lint` | |
| `provision_plan` | 0 | `provision.plan` | returns `planId` |
| `sign_verify` | 0 | `sign.verify` | |
| `ship_validate` | 0 | `ship.validate` | |
| `ship_builds` | 0 | `ship.builds.list` | |
| `testflight_groups` | 0 | `ship.testflight.groups` | read-only; group and tester management stays CLI/IDE only (§11.5) |
| `testflight_crashes` | 0 | `ship.crashes.list/get` | wrapped |
| `asc_get` | 0 | `asc.get` | allowlisted GET paths only |
| `build` | 1 | `build.start` + `job.wait` | returns within 50 s: result or `{status: running, jobId}` |
| `build_clean` | 1 | `build.clean` | |
| `app_install` | 1 | `app.install` (path) or `run.start {launch: none}` (project) | |
| `app_run` | 1 | `run.start` | |
| `app_stop` | 1 | `run.stop` or `app.terminate` | |
| `app_uninstall` | 1 | `app.uninstall` | only the project's own bundle IDs or `dev.russet.test.*` [D 55] |
| `hot_reload` | 1 | `run.hotReload` | Swift: Quick Relaunch |
| `debug_start` | 1 | `debug.prepare {frontend: dap}` | uses `ide.debug.start` when an IDE is attached |
| `debug_control` | 1 | `debug.control` | returns the resulting stop; no polling needed |
| `debug_evaluate` | 1 | `debug.evaluate` | |
| `debug_stop` | 1 | `run.stop` | agents may kill, not detach Flutter sessions |
| `breakpoint_set`, `breakpoint_remove` | 1 | `debug.breakpoint.set/remove` | visible in VS Code when attached |
| `input_tap`, `input_swipe`, `input_type`, `input_button` | 1 | `input.*`, or `screen.press` (`elementId`) | coordinate space default `screenshot` [D 51] |
| `location_set` | 1 | `location.set` | |
| `assets_app_icon` | 1 | `assets.appIcon` | Wave 3 |
| `plan_apply` | 2 | `provision.apply` | `approval` argument |
| `ship_upload` | 3 | `ship.upload` | `approval` argument |
| `testflight_distribute` | 3 | `ship.testflight.distribute` | `approval` argument |

Area-doc names map onto this list as follows:
- **device:** `ios_devices` → `device_list`, `ios_ui_elements` → `ui_tree`, `ios_logs` → `logs_read`, …
- **build:** `russet_build_status` → `job_status`, `russet_build_log` → `job_log`, `russet_clean` → `build_clean`, `russet_inspect_app` → `app_inspect`, `russet_app_icon` → `assets_app_icon`.
- **SIGN:** see [IDE §6.3].
- **RD:** `russet_run_app` → `app_run`, `russet_debug_wait_for_stop` → `debug_control.waitForStopMs`, `russet_debug_backtrace`/`variables` → `debug_inspect`, `russet_app_logs` → `logs_read`.

### 11.5 Never exposed to agents

- certificate revocation and every other `account-destructive` method;
- account add, import or remove; Apple ID password or 2FA entry; anisette library download;
- `secrets.*`, `approval.grant/deny`;
- `ship.metadata.apply`, `ship.version.apply` and `ship.review.submit` (App Store review submission);
- TestFlight group and tester management and beta review (`ship.testflight.groups.create`, `addTesters`, `removeTesters`, `betaReview.submit`);
- `system`-class methods (toolchain, SDK, daemon, MCP admin);
- `files.push/remove`, `process.signal`.

### 11.6 Output rules [IDE §6.5, QA §8.4]

- Every potentially slow tool returns within 50 s, with the result or `{status: "running", jobId}`. Agents continue with `job_status`. Codex defaults to a 60 s tool timeout [IDE §6.5].
- Results are capped at 64 KiB, with a `cursor` for more. Device- and Apple-originated text is wrapped in `<untrusted source="device">…</untrusted>`, and control characters and ANSI escapes are stripped.
- Tool output never contains key material, GS tokens, raw profiles or full UDIDs.
- Server instructions sent to every client:
  - "call `device_list` first";
  - "builds may return `status: running` — call `job_status`";
  - "account and publishing changes need a human approval in Russet";
  - "never ask the user for Apple passwords or keys".

### 11.7 Registration and the agent panel [IDE §6.6–§6.8]

- **Registration.** `russet mcp install --client <claude-code|codex|gemini|copilot-cli|vscode|cline|continue|openhands|opencode>` writes the registration through `russet-agent-config`.
  - It prefers each client's own `mcp add` command.
  - Edits keep comments, a backup is made first, and the operation is idempotent.
  - Project-scope files use `command: "russet"` from PATH; user scope uses the stable shim (`~/.local/share/russet/bin/russet`, `%LOCALAPPDATA%\Russet\bin\russet.exe`).
- **Agent panel.** An ACP v1 client (`@agentclientprotocol/sdk` 1.8.0) spawns registry agents and injects `russet mcp serve --client <agent>` through `session/new.mcpServers`. While the IDE holds a device reservation, the injected server entry carries `env: {RUSSET_LEASE: <leaseId>}`, so the agent shares the IDE's reservation (§6.3).
- **Local models.** OpenAI-compatible endpoints are probed on 9931, 8080, 11434 and 1234. Tool calling needs `llama-server --jinja` [V IDE V18].

---

## 12. Security model

### 12.1 Assets and adversaries [QA §8.1]

- **Assets:**
  - credentials: the ASC `.p8` key, Apple ID credentials and GSA tokens, anisette state, signing private keys, release signing keys;
  - device-related data: pairing records (`HostPrivateKey` grants full device access), provisioning profiles (UDIDs, team ID), device data (logs, screen, crash reports);
  - the user's source code.
- **Adversaries:** a prompt-injected or misbehaving AI agent; a malicious app writing to device logs or the screen; other local users; network attackers on HTTP paths; malicious PRs and dependencies; hosted CI.

### 12.2 Controls

| Area | Control | Source |
|---|---|---|
| Boundary | The OS user is the boundary. IPC is same-user only (§7.1). Permission profiles are guardrails, not a sandbox, and the docs say so | core §7.5 |
| Agents | Tiers, out-of-band approvals bound to (client, method, params digest), rate limits, audit, `<untrusted>` wrapping, 64 KiB caps, no secret or full-UDID output; tier 3 always needs a human | §11, QA §8.4 |
| Approval UI | Grant is reachable only from the Approval panel's message bus or `russet approve` in a TTY; there is no VS Code command id for it | IDE §6.4 |
| Secrets | One 32-byte master key in the OS keyring (Secret Service, Credential Manager; one entry per `RUSSET_HOME`, §3.3 item 7). Every secret is a sealed file (XChaCha20-Poly1305, AAD = id‖kind‖version). Headless fallback: an Argon2id passphrase-derived key. CI: env only. Values are never on the wire; `SecretBytes` zeroizes and has no `Display` | core §9.6 [D 13] |
| Logs | A field-name redaction layer (`token`, `password`, `secret`, `authorization`, `cookie`, `x-apple-i-md*`, `anisette`, `private key`, `p8`, `p12`, `HostPrivateKey`), JWT/PEM/Bearer pattern masking, HTTP logs without headers or bodies, and a CI canary test on every log path | core §12, SIGN §17 |
| Debug forwards | Unix sockets 0600 in a 0700 directory (Linux). Loopback, accept-once with a 60 s deadline (Windows). debugserver means code execution in the app | device §14 |
| Web endpoint | 127.0.0.1 only; single-use tokens sent in the first message; `Origin`/`Host` checks; MCP HTTP is off by default and needs a bearer token | §6.6, §10.1, §11.2 |
| Pair records | Read through usbmuxd; never copied into Russet state except on explicit export (0600). RPPairing records (Wi-Fi) are 0600 secrets | device §14, QA §8.2 |
| Downloaded Apple code | Anisette libraries come only from Apple's CDN, by HTTP range, hash-pinned per APK version, after user consent. They run in `russet-adi-host` with no network: seccomp + Landlock on Linux, a Job Object on Windows | SIGN §7.5 |
| TLS | rustls with the ring provider. GSA pins Apple Root CA (fetched from apple.com at runtime, SHA-256 pinned; it is not in system stores [V SIGN V15]). ASC uses the WebPKI roots | §16.6 |
| ASC tokens | ES256 JWT ≤ 20 min. Agent reads use GET-scoped tokens, which Apple enforces (403 `REQUEST_DOES_NOT_MATCH_SCOPE`) [V SIGN V3] | SIGN §5.2 |
| Workspace trust | The IDE disables build, run, debug and agents in untrusted workspaces (Package.swift plugins and build scripts execute code) | IDE §6.10 |
| Supply chain | cargo-deny (licences, bans, advisories, sources), SHA-pinned Actions, zizmor, gitleaks, Dependabot, SBOM, `cargo auditable`, minisign `SHA256SUMS` | QA §8.5 |
| Device settings | No device setting changes without an explicit request (Wi-Fi debugging, Developer Mode, DDI); lab tests only check them | device §14, QA §3.9 |

### 12.3 Known limits (stated to users)

- An agent with unrestricted shell access as the same user can run `russet` directly. Mitigations:
  - CLI approvals need a TTY;
  - CLI mutations need confirmation or `--yes`;
  - the audit log records CLI use from class `account` up;
  - the docs say not to auto-approve shell for agents that have the CLI.
- xtool writes the ASC key PEM and a signing private key to **world-readable (0644)** files [V SIGN V1]. `russet account import --from-xtool` imports them once and prints a `chmod 600` / `xtool auth logout` recommendation. On the dev box they were locked down to 0600 on 2026-10-11 (decisions.md; re-checked with `ls -la ~/.config/xtool/data`); `xtool auth logout` follows once Russet's own provisioning works (W12).

---

## 13. Error model

### 13.1 Wire shape [core §7.9, §13] [D 7]

```json
{"jsonrpc":"2.0","id":9,"error":{"code":3002,"message":"iPhone 12 is locked",
 "data":{"russetCode":"device.locked","hint":"Unlock the iPhone and keep it unlocked during install.","retryable":true,
         "details":{"udid":"00008101-…"},"docs":"docs/reference/errors.md#device.locked","chain":["FBSOpenApplicationErrorDomain 7 \"Locked\""]}}}
```

- **Codes.**
  - `error.code` is the Russet number (1000–9999), or a standard JSON-RPC code (`-32700`, `-32600`, `-32601`, `-32602` with `details.path`, `-32603`, `-32002`, `-32800`).
  - `russetCode` is the stable identifier. Tests assert codes, never message text.
  - `docs` points at the generated `docs/reference/errors.md` anchor until the docs site exists (W26).
- **Display:** `device.locked (3002): iPhone 12 is locked. Hint: …`.
- **CLI `--json`:** `{"type":"error","error":{"code":3002,"russetCode":"device.locked","message":…,"hint":…}}`.
- **MCP:** an `isError` tool result with `structuredContent.error = {code: russetCode, number, message, hint, details}`.
- **Hints.** Every user-visible error has a hint when a next step exists; a test over the code registry enforces it.

### 13.2 Ranges

| Range | Family prefix | Owner |
|---|---|---|
| 1000–1099 | `core` | W2 |
| 1100–1199 | `ipc` | W2 |
| 1200–1299 | `perm` | W2 |
| 1300–1399 | `config` | W3 |
| 1400–1499 | `secrets` | W3 |
| 2000–2099 | `toolchain` | W3 |
| 2100–2199 | `sdk` | W3, W16b |
| 3000–3099 | `device` | W4, W10 |
| 3100–3199 | `app` | W4 |
| 3200–3299 | `device` services (fallback, files, location) | W10 |
| 4000–4099 | `build` | W5 |
| 4100–4199 | `assets` (including `ib`), `flutter` (build), `xcode` | W16, W17, W21, W21b, W28, W28b, W28c |
| 5000–5099 | `sign` | W6 |
| 5100–5199 | `account` | W6, W12 |
| 5200–5249 | `provision` | W6, W12 |
| 5250–5299 | `gsa`, `anisette` | W22 |
| 6000–6099 | `debug` | W7, W11, W18 (6016) |
| 6100–6149 | `crash` | W11 |
| 6150–6199 | `flutter` (run) | W18 |
| 7000–7199 | `ship` | W12, W23 |
| 8000–8099 | `screen` (screenshot, mirror, input) | W10 |
| 8100–8199 | `perf` | W10 |
| 8200–8299 | `xctest` | W32, W28 |
| 9000–9099 | `mcp` | W13 |
| 9100–9199 | `agents` | W20 |

### 13.3 Initial code registry

Each workstream adds codes only in its own range and family file (`crates/russet-core/src/error/codes/<family>.rs`). Area-doc names in brackets are the old names they replace.

| Family | Codes |
|---|---|
| `core` | 1000 internal · 1001 cancelled · 1002 timeout · 1003 io · 1004 diskFull · 1005 notImplemented · 1006 invalidArgument · 1007 notFound · 1008 busy · 1009 unsupportedPlatform |
| `ipc` | 1100 protocolMismatch · 1101 notInitialized · 1102 daemonUnavailable · 1103 daemonStarting · 1104 frameTooLarge · 1106 connectionClosed · 1107 buildMismatch (development builds, §3.3) |
| `perm` | 1200 denied · 1201 confirmationDeclined · 1202 confirmationUnavailable · 1203 authFailed · 1204 approvalRequired [RUS-MCP-0403] · 1205 approvalInvalid · 1206 rateLimited · 1207 tierExceeded |
| `config` | 1300 invalid (with file:line:col) [E_MANIFEST_INVALID] · 1301 projectNotFound [E_PROJECT_NOT_FOUND] · 1302 unknownKey · 1303 readOnly |
| `secrets` | 1400 unavailable [E_SECRETS_NO_BACKEND] · 1401 locked · 1402 notFound · 1403 passphraseRequired |
| `toolchain` | 2000 missing · 2001 versionMismatch [E_TOOLCHAIN_MISMATCH] · 2002 broken (e.g. missing libpython) · 2003 downloadFailed · 2004 verifyFailed · 2005 pinsInvalid · 2006 linkerUnsupported [E_LINKER_UNSUPPORTED] |
| `sdk` | 2100 missing [E_SDK_MISSING] · 2101 incompatible · 2102 importFailed · 2103 incomplete [E_SDK_INCOMPLETE] · 2104 archiveInvalid (`details.layer` ∈ xar, signature, pbzx, cpio, udif, hfs; a signature that does not chain to the pinned Apple Root CA, or a SHA-384 manifest-digest mismatch, is layer `signature`) · 2105 pathTooLong (`details {path, length, longPathsEnabled}`) · 2106 linkFailed (a junction or hardlink failed, e.g. on FAT/exFAT or network volumes, which are refused) |
| `device` | 3000 notFound · 3001 notPaired · 3002 locked · 3003 untrusted · 3004 developerModeDisabled · 3005 ddiNotMounted · 3006 tunnelFailed · 3007 disconnected · 3008 usbmuxUnavailable · 3009 trustPending · 3010 ddiPersonalizationFailed · 3011 tunnelStalled · 3012 serviceUnavailable · 3013 serviceNotImplemented · 3014 timeout · 3015 unsupported · 3016 io · 3017 protocol · 3018 reserved [RUS-DEV-0423] · 3019 liveBackendRefused (development guard, §3.3) · 3020 ddiSourceMissing (no imported DDI and no configured directory; hint: `russet sdk import --ddi-only <Xcode_27.xip \| XcodeSystemResources.pkg \| a copy of a Mac's /Library/Developer/DeveloperDiskImages/iOS_DDI>`) · 3021 ddiIncompatible (the DDI does not fit this iOS version or board; `ensure_ddi` tries the next imported build) · 3022 ddiMirrorConsentRequired (`pmd3-cache` or `pmd3-auto-mount` without the `allow-mirror` consent) · 3200 fallbackUnavailable |
| `app` | 3100 notInstalled · 3101 installFailed (`details.subkind`: signature, no-matching-profile, minimum-os, app-id-mismatch) · 3102 launchFailed |
| `build` | 4000 projectAmbiguous · 4001 inProgress · 4002 compileFailed · 4003 linkFailed · 4004 dynamicProductUnsupported · 4005 bundleIdInvalid · 4006 lintFailed · 4007 packageFailed · 4008 resourceUnsupported |
| `assets`, `flutter`, `xcode` | 4100 assets.invalid · 4101 assets.storyboardUnsupported · 4120 flutter.notFound · 4121 flutter.engineMissing · 4122 flutter.genSnapshotMissing · 4123 flutter.customDevicesDisabled · 4124 flutter.pluginUnsupported · 4125 flutter.cocoapodsRequired · 4150 xcode.projectUnsupported · 4151 xcode.pifFailed · 4152 xcode.swbFailed |
| `sign` | 5000 noIdentity · 5001 profileMismatch · 5002 entitlementNotInProfile · 5003 entitlementValueNotAllowed · 5004 entitlementUnresolvedVariable · 5005 lintFailed · 5006 codesignFailed |
| `account` | 5100 ascUnauthorized · 5101 ascForbidden · 5102 ascScope · 5103 ascRateLimited · 5104 individualKeyNoProvisioning · 5105 ascAmbiguous · 5106 twoFactorRequired · 5107 appleServiceUnavailable · 5108 insufficientRole |
| `provision` | 5200 certLimit · 5201 devicePending · 5202 deviceLimit · 5203 portalStepRequired · 5204 freeQuota · 5205 freeUnsupported · 5206 planStale (the digest no longer matches) |
| `gsa`, `anisette` | 5250 gsa.badCredentials · 5251 gsa.badCode · 5252 gsa.locked · 5253 gsa.identityRejected · 5270 anisette.libsMissing · 5271 anisette.pinMismatch · 5272 anisette.adiFailed |
| `debug` | 6000 launchDenied · 6001 profileUntrusted · 6002 attachDenied · 6003 alreadyDebugged · 6004 debugserverUnavailable · 6005 uuidMismatch · 6006 lldbUnsupported · 6007 handoffTimeout · 6008 jitHookNotFound · 6009 jitArmingFailed · 6010 processExited · 6011 symbolsNoSpace · 6012 symbolsService · 6013 brokerLost · 6014 sessionNotFound · 6015 overlayUnavailable [RD §22] · 6016 jitPageMissed (a crash at a page that was announced but not touched, or in an executable region never announced: a servicer bug; `details {region, page, pc, faultAddress}`) |
| `crash`, `flutter` (run) | 6100 crash.notFound · 6101 crash.symbolicationFailed · 6150 flutter.vmServiceNotFound |
| `ship` | 7000 noAppRecord · 7001 duplicateBuild · 7002 uploadFailed (`details.itms`) · 7003 commitAmbiguous · 7004 stampNoConsent · 7005 stampNoSdkMetadata · 7006 validationFailed · 7007 versionStateInvalid (the version is not editable) · 7008 reviewSubmitFailed (Apple's errors verbatim in `details.errors`) · 7009 ciCloudSigningForbidden ("Cloud signing permission error": cloud signing needs an Admin-role team key) · 7010 ciRunnerUnavailable (the pinned runner label or Xcode version is missing from the image) |
| `screen`, `perf` | 8000 screen.mediaInUse [E_MEDIA_IN_USE; device 9022] · 8001 screen.streamFailed · 8002 screen.codecUnsupported · 8003 screen.inputDenied · 8100 perf.serviceUnavailable |
| `mcp` | 9000 toolNotFound · 9001 httpUnauthorized · 9002 originRejected |

**Device remap** (device.md §9 used 1100–1191, which collide with `ipc`):

| device.md kind | New code |
|---|---|
| `mux.unavailable` | 3008 |
| `device.not_found` | 3000 |
| `device.disconnected` | 3007 |
| `pairing.required` | 3001 |
| `pairing.pending` | 3009 |
| `pairing.denied` | 3003 |
| `device.locked` | 3002 |
| `devmode.off` | 3004 |
| `ddi.missing` | 3005 |
| `ddi.personalization_failed` | 3010 |
| `tunnel.failed` | 3006 |
| `tunnel.stalled` | 3011 |
| `service.unavailable` | 3012 |
| `service.not_implemented` | 3013 |
| `install.failed` | 3101 |
| `launch.failed` | 3102 |
| `app.not_installed` | 3100 |
| `timeout` | 3014 |
| `unsupported` | 3015 |
| `fallback.unavailable` | 3200 |
| `io` | 3016 |
| `protocol` | 3017 |

**Warnings** are `JobEvent.warning` or session events, not errors: `debug.warn.jitInterpreted`, `debug.warn.jitUnsupportedEngine`, `debug.warn.slowLink`, `build.warn.lint`.

### 13.4 CLI exit codes [core §13.3] (the only table; [D 8])

| Exit | Meaning | Families |
|---|---|---|
| 0 | Success | |
| 1 | Unexpected failure | `core.internal`, unknown |
| 2 | Usage (clap) | |
| 3 | Not found | `*.notFound` |
| 4 | Environment: run `russet doctor` | `config`, `toolchain`, `sdk`, `core.diskFull` |
| 5 | Credentials and accounts | `secrets`, `account`, `gsa`, `anisette` |
| 6 | Device state | `device` |
| 7 | Build failed | `build`, `assets`, `flutter` (build), `xcode` |
| 8 | Signing or provisioning failed | `sign`, `provision` |
| 9 | Launch or debug failed | `app.launchFailed`, `debug`, `crash`, `flutter` (run) |
| 10 | Publishing failed | `ship` |
| 11 | Permission denied, declined, or approval needed | `perm` |
| 12 | Daemon unavailable or version mismatch | `ipc` |
| 130 | Cancelled (SIGINT) | `core.cancelled` |

- `russet run` and `russet debug` exit with these codes.
- `--exit-code app` makes them exit with the app's status instead, with signals as 128 + signal [RD §20].
- The lab harness uses its own convention (exit 3 = skipped environment) through `cargo xtask lab`, not the `russet` CLI.

---

## 14. Configuration and file formats

### 14.1 Precedence [core §9.1]

From highest to lowest:
1. CLI flags.
2. `RUSSET_*` environment variables.
3. `russet.local.toml` (per user, gitignored).
4. `russet.toml` (committed).
5. Global `config.toml`.
6. Built-in defaults.

Exception: agent permissions can only be **narrowed** by project files (§7.5).

**All TOML keys are kebab-case** [D 15]. JSON files and wire payloads are camelCase. Unknown TOML keys are an error with file:line:col (`config.invalid`, `config.unknownKey`).

### 14.2 `russet.toml` (project; committed) [D 16]

```toml
schema = 1

[project]
name = "Hello"
stack = "swiftpm"                       # swiftpm | flutter | xcode | react-native | maui; auto-detected if omitted

[app]
bundle-id = "com.example.hello"         # canonical; paid teams use it exactly
display-name = "Hello"
version = "1.0.0"                       # CFBundleShortVersionString
build-number = "1"                      # CFBundleVersion | "auto" | "git-count" | "timestamp"
deployment-target = "17.0"              # must be in the SDK's ValidDeploymentTargets
devices = ["iphone"]                    # iphone | ipad
orientations = { iphone = ["portrait"] }
icon = "AppIcon"                        # icon set in a main-bundle catalog, or a .png (development only)
accent-color = "AccentColor"
launch-screen = { color = "LaunchBackground", image = "LaunchLogo", respects-safe-area = true }
info-plist = "Resources/Info.plist"     # extra keys merged at precedence level 3 (build §4.4)
entitlements = "Resources/Hello.entitlements"
privacy-manifest = "Resources/PrivacyInfo.xcprivacy"
resources = ["Resources/Assets.xcassets", "Resources/Localizable.xcstrings"]

[[app.extensions]]
product = "HelloWidget"                 # SwiftPM library product
kind = "widgetkit"
bundle-id-suffix = "widget"             # → com.example.hello.widget
info-plist = "Widget/Info.plist"
entitlements = "Widget/Widget.entitlements"

[build]
default-profile = "debug"
sdk = "auto"                            # auto | iphoneos27.0 | 27A266a
backend = "swiftbuild"                  # swiftbuild | native | xtool | pif
jobs = "auto"
metadata = "sdk"                        # none | minimal | sdk | xcode-compat (§16.4)
metadata-consent = { by = "", at = "" } # required for xcode-compat; recorded by `russet ship stamp`/IDE
dsym = "release"                        # never | release | always
strip = "release"
lint = "auto"                           # auto (warn debug, error release) | warn | error | off
launch-screen-fallback = "error"        # error | empty
launch-storyboard = "auto"              # auto | compile | convert. auto: compile when russet-ib supports the storyboard,
                                        #   else convert to UILaunchScreen if lossless, else launch-screen-fallback [D 75]
overrides = {}                          # build-setting overrides

[build.swiftpm]
package = "."
product = "Hello"
swiftc-flags = []
cc-flags = []
ld-flags = []

[build.flutter]
version = "3.47.7"
target = "lib/main.dart"
flavor = ""
dart-defines = []
plugins = "swiftpm"                     # swiftpm | swiftpm+convert | swiftpm+cocoapods
gen-snapshot = "auto"

[build.xcode]                           # Wave 6
workspace = "App.xcworkspace"
scheme = "App"
configuration = { debug = "Debug", release = "Release" }
cocoapods = "auto"

[build.profiles.release]
dsym = "always"
package = "ipa-store"

[signing]
account = ""                            # account id from accounts.toml; normally set in russet.local.toml
team = ""
mode = "automatic"                      # automatic | manual
bundle-id-policy = "auto"               # auto (exact for paid, team-suffix for free) | exact | team-suffix | xtool-prefix
                                        #   | { custom = { "com.example.hello" = "…" } }
merge = "least"                         # least | profile-all

[signing.targets."PlugIns/HelloWidget.appex"]
entitlements = "Widget/Widget.entitlements"
profile = ""                            # manual mode only: path to a .mobileprovision

[run]
device = "auto"                         # auto | <udid> | <alias>
args = []
env = {}
console = true
install = "if-changed"                  # if-changed | always | never
launch = "auto"                         # auto | coredevice | dvt

[debug]
lldb = "auto"                           # auto | swift | llvm | <path>
init-commands = []
symbols = "auto"                        # auto | none | core | full
jit = "auto"                            # auto | native | lldb-python | off
swift-expressions = "auto"              # auto | on | off (overlay, §16.2)
attach-mode = "broker"                  # broker | legacy-async

[ship]
mode = "linux-native"                   # linux-native | apple-hardware
app-id = ""                             # numeric ASC app id; filled on first upload
build-number = "auto"
testflight-groups = ["Internal"]
uses-non-exempt-encryption = false      # written as ITSAppUsesNonExemptEncryption; export compliance without an API call

[ship.ci]                               # Apple-hardware mode (§17.5) [D 76]
provider = "github"                     # github | xcode-cloud
signing = "russet"                      # russet | xcode-manual | xcode-cloud (Admin-role key)

[toolchain]
swift = "6.4.0"
xcode-sdk = "27.0"
linker = "auto"                         # auto | darwin-tools@1.1.0 | llvm@>=23.1.3 | <path>

[agents]                                # may only NARROW the user's mcp-policy.toml
tier-max = 1
deny-tools = []
```

`russet.local.toml` uses the same schema (typically `[run] device`, `[signing] account/team/bundle-id-policy`, `[app] bundle-id` overrides for a personal team). `russet init` adds it to `.gitignore`.

### 14.3 Global `config.toml` (`$XDG_CONFIG_HOME/russet/`, `%APPDATA%\Russet\config\`)

```toml
schema = 1
[daemon]
idle-timeout = "30m"
max-parallel-jobs = 4
max-build-jobs = 1
build-nice = 10
log-filter = "info"
log-retention-days = 7
listen-tcp = ""                 # WSL split mode only

[device]
keep-warm = "10m"
lease-grace = "2s"
mux = "auto"                    # auto | usbmuxd:<path> | amds | netmuxd:<addr>

[device.ddi]                    # D 62; ensure_ddi tries these in order and stops at the first that mounts
order = ["dir", "imported"]     # "imported": newest ddiBuild first, then older ones on ddiPersonalizationFailed / ddiIncompatible
dir = ""                        # a DDI root holding Restore/ and version.plist, e.g. a copy of a Mac's
                                #   /Library/Developer/DeveloperDiskImages/iOS_DDI; Russet loads <dir>/Restore, and also accepts
                                #   a directory that directly contains BuildManifest.plist [D 82]
allow-mirror = false            # "pmd3-cache" and "pmd3-auto-mount" are mirror content; allowed only after the consent text
ticket-cache = false            # cache the Cryptex1 IM4M per (ECID, ddiBuild, nonce), mode 0600, for offline remounts (W10)

[storage]
min-free-space = "5GiB"
symbol-quota = "20GiB"

[devices.aliases]
iphone12 = "<udid>"

[web]
listen = "127.0.0.1:0"

[mcp.http]
enabled = false
listen = "127.0.0.1:43117"
expose-docker = false

[fallback]
pymobiledevice3 = "auto"        # auto | off | <path>

[update]
check = "notify"
channel = "stable"              # owner question §21 Q12

[privacy]
telemetry = "off"               # Russet has no telemetry
```

Accounts are **not** in `config.toml` [D 14]. MCP policy is in `mcp-policy.toml` [D 44].

### 14.4 `accounts.toml` (config dir; written by `account.*`, human-readable) [SIGN §14.1]

```toml
schema = 1
default = "asc:<KEYID>"

[[account]]
id = "asc:<KEYID>"
kind = "asc-team-key"                   # asc-team-key | asc-individual-key | apple-id
label = "Team key (App Manager)"
key-id = "<KEYID>"
issuer-id = "<UUID>"
team-id = "<TEAMID>"                    # discovered from a certificate OU, else a bundle-ID seedId
secret = "secret:asc-key/<KEYID>"
imported-from = "xtool"                 # optional provenance

[[account]]
id = "appleid:<DSID>"
kind = "apple-id"
apple-id = "someone@example.com"
team-id = "<TEAMID>"
team-tier = "free"
anisette = "local"                      # local | { remote = "https://…" }
secret = "secret:apple-id-session/<DSID>"
```

### 14.5 `provision-policy.toml` (config dir; project files may only make it stricter) [SIGN §6.4]

```toml
schema = 1
[paid]
register-device = "auto"
register-bundle-id = "auto"
enable-capability = "auto"
russet-profiles = "auto"
create-certificate = "ask-once"
revoke-certificate = "typed"
disable-capability = "typed"
delete-foreign-profile = "typed"

[free]
register-device = "ask"
register-bundle-id = "ask"
enable-capability = "auto"
russet-profiles = "auto"
create-certificate = "ask"
revoke-certificate = "typed"
disable-capability = "typed"
delete-foreign-profile = "typed"
```

"ask" is a `ui.confirm` for cli/ide connections; for MCP these are tier-2 approvals.

### 14.6 `mcp-policy.toml` (config dir) [QA §8.4]

```toml
schema = 1
default-tier-max = 1
session-approval-ttl = "30m"    # HTTP clients only; stdio sessions end with the bridge process (§11.3)
[clients.claude-code]
tier-max = 1
reveal-udid = false
[clients.local-llama]
tier-max = 0
[tools.ship_upload]
approval = "always"
rate-limit = "1/10m"            # tier 3 can never be below "always"
[tools.plan_apply]
approval = "session"
```

### 14.7 `pins/stable.toml` (generated by `cargo xtask pins` in CI; minisign-signed; embedded) [core §10.2]

- **No hand-typed URLs.** The first guessed swift.org URL returned 404 [V core §23].
- **Wave 1** [D 87]. CI pin generation and the release key come with W26 (Wave 4), but W3 needs a pinned, verified download in Wave 1. So W3 owns `xtask/src/pins.rs` in Wave 1 and generates the two components it installs, `python-shared` (PBS 3.12.15, release 20261009) and `darwin-tools` 1.1.0: asset URLs and sizes come from the GitHub release metadata (`gh api repos/<owner>/<repo>/releases/tags/<tag>`), and `sha256` from hashing the downloaded asset, never from a hand-typed value. The file is signed with a **development** minisign key whose secret half stays outside the repository (`~/.config/russet-dev/minisign.key`); debug builds embed its public key, and release builds accept only W26's release key. The CORE-11 tamper test signs with a key generated at test time. W26 extends `pins.rs` to every component and moves generation into CI.
- **`[[xcode]]` rows** carry `version`, `build`, `ios-sdk`, `swift` (minor must match) and `min-lld`.
- **`[[component]]` entries:** `swift`, `darwin-tools` (1.1.0, 46 MB), `llvm` (23.1.3, Windows only), `xtool` (1.21.0), `flutter` (3.47.7), and `python-shared`.
- **`python-shared`** is `source = "python-build-standalone"`, `version = "3.12.15"`, `release = "20261009"`, with a per-platform `url`/`size`/`sha256` filled by the generator [RD §8.2, V RD V14] [D 37].
- **`[ci.github]`** [D 76]: `runner-by-xcode-major = { 27 = "xcode-27", 26 = "macos-26" }`; the SHA of every action the generated workflows use (`actions/checkout` v7.0.1 = `3d3c42e5aac5ba805825da76410c181273ba90b1`, `actions/upload-artifact` v7.0.2 = `cf430e030ddbb5b0abf93d22962f4752f3646cd9`); `xtool = { version = "1.21.0", asset = "xtool.app.zip", sha256 = "33bb2ef12de3f2acfb5eb8385fca0de9df3a982862ec067185a59e8ae18b64e8" }`. `ship.ci.init` and the `ship.ci.status` check regenerate or flag user workflows from this data, without a code change, when `xcode-27` goes GA under another name or `xcode-28` appears.

### 14.8 Machine-written files

| File | Owner crate | Format |
|---|---|---|
| `data/secrets/<sha256(id)>.sealed`, `index.json`, `master.salt` | russet-secrets | sealed binary; names-only JSON |
| `data/sdks/registry.json` | russet-toolchain (read) / russet-sdk (write) | `{id, bundle, xcodeVersion, xcodeBuild, iosSdk, darwinTools, openAppleMacros, importedWith: russet-xar\|xtool, importedAt, layout, maxAbsPathLen}` |
| `data/sdk/<id>/bundle/` (Linux), `%LOCALAPPDATA%\Russet\s\<id8>\` (Windows short root) | russet-sdk | a Russet-imported Swift SDK bundle (the iOS-only want-set of §16.3); `info.json` lists the Linux and Windows host triples; used through `swift build --swift-sdks-path` [D 69] |
| `data/sdk/<id>/view/Developer/…`, `toolset-swb.json`, `toolset-native.json` | russet-sdk | symlinks (Linux) / junctions + copies (Windows); SwiftPM toolset JSON |
| `data/ddi/<ddiBuild>/iOS/{Restore/{BuildManifest.plist, Restore.plist, *.dmg, Firmware/*}, version.plist}`, `data/ddi/index.json` | russet-sdk (write) / russet-device (read) | the iOS DDI exactly as macOS installs it (§16.3), about 33.6 MB per build; `index.json` entries = `{ddiBuild, xcodeBuild?, forms, cryptexVersion, coreDeviceVersion, source: {kind: xip\|xcode-app\|pkg\|ddi-dmg\|device-support-dmg\|dir\|apple-device-support, path?\|url?}, signer?, sha256: {relpath: hash}, sha384Checked: true, importedAt}` [D 62] |
| `data/tools/<russet-version>/bin/*` | russet-sdk | links or copies of `russet-tool` |
| `data/toolchains/<component>/<version>/`, `selected.json` | russet-toolchain | no symlinks (Windows-safe) |
| `data/identities/<team>/<serial>.{json,cer}` | russet-account | public part only |
| `data/profiles/<uuid>.mobileprovision`, `index.json` | russet-account | profile cache |
| `data/anisette/libs/<apkver>/<abi>/*.so`, `state.json` | russet-anisette | downloaded at runtime only |
| `data/archives/`, `data/dsyms/<UUID>.dSYM` | russet-ship, russet-debug | `.xcarchive` layout; dSYM index |
| `data/lldb-overlays/<swift>-<sdk>/` | russet-debug | reflink overlay for `po` |
| `state/jobs/index.jsonl`, `state/jobs/<id>/{events.ndjson,result.json}` | russetd | JSONL |
| `state/devices.json`, `state/devices/<udid>/installs.json` | russet-device, pipeline | install ledger `{bundleId: {cdhash, installedAt}}` |
| `state/devices/<udid>/ddi.json` | russet-device | the last DDI mount Russet performed: `{source, path?, ddiBuild, cryptexVersion?, imageSha384, mountedAt}` (§5.3 `MountRecord`) [D 89] |
| `state/device/rppairing/<UDID>.plist` | russet-device | 0600 (Wi-Fi, Wave 6) |
| `state/sessions.json`, `state/sessions/<sid>/{session.json,output.log,oslog.jsonl}` | russet-debug | `session.json` schema in [RD §21] |
| `state/crashes/<bundleId>/` | russet-debug | `.ips` + JSON summary |
| `state/panics/<ts>.txt` | russetd | daemon panic reports [D 42] |
| `state/uploads/<uploadId>.json` | russet-ship | resumable upload journal |
| `state/audit.jsonl` | russetd | 0600, append-only [D 43] |
| `state/logs/russetd.YYYY-MM-DD.log`, `mcp-<pid>.log` | russetd, russet-mcp | text or JSON |
| `cache/downloads/`, `cache/toolchain-probes.json` | russet-toolchain | |
| `cache/device-support/<build>/<dsc-uuid>/`, `cache/lldb-module-cache/` | russet-debug | quota-managed |
| `cache/flutter/{engine,gen_snapshot}/` | russet-build | |
| `runtime/russetd.{sock,lock,pid,json}`, `spawn.lock`, `tcp.json` | russetd | |
| `runtime/sessions/<sid>/{gdb.sock,dap.sock}`, `runtime/fwd-<lease>.sock` | russet-debug, russet-device | 0600 |
| `<project>/.russet/build/<profile>-iphoneos/{App.app,App.app.dSYM,App.ipa,build-report.json}` | russet-build | build-report schema 1 [build §9.6] |
| `<project>/.russet/build/<profile>-iphoneos/signed/App.app` | pipeline (russet-sign) | signed copy; the unsigned product stays |
| `<project>/.russet/work/{builder,swiftpm,flutter,pif,actool,entitlements}/`, `<project>/.russet/logs/<job-id>.jsonl` | russet-build | disposable |
| `<project>/.russet/debug/<sid>/attach.lldb` | russet-debug | Python-free LLDB script |
| `<project>/.russet/signing.lock.json` | russet-account | `{team, identity, profiles{relPath: uuid}, resolvedAt}` |
| Config dir: `gsa-profile.json`, `redaction-salt`, `private-identifiers` | russet-gsa, testkit | `redaction-salt` 0600; `private-identifiers` is the owner's local denylist |

### 14.9 Environment variables

| Variable | Meaning |
|---|---|
| `RUSSET_HOME`; `RUSSET_{CONFIG,DATA,STATE,CACHE,RUNTIME}_DIR` | Portable root; per-directory overrides. Every lane-D development worktree sets `RUSSET_HOME=<worktree>/.russet-dev`; lane A uses the persistent `$HOME/russet-lab/home-A` (§3.3 item 7) |
| `RUSSET_LANE` | Development only: `A` allows the `live` device backend in development builds (§3.3); unset elsewhere |
| `RUSSET_LIVE_DEVICE_LOCK` | Debug builds only (ignored in release builds): path of the host-wide live-device lock, so tests never take the real one (§3.3 item 7) [D 83] |
| `RUSSET_LEASE` | A device-reservation id the process may share (§6.3); set by the IDE for the agents it spawns |
| `RUSSET_DAEMON` | Endpoint override (`unix:`, `pipe:`, `tcp://…`), or `inproc` (= `--no-daemon`) |
| `RUSSET_NO_AUTOSTART=1` | Fail instead of spawning the daemon |
| `RUSSET_LOG`, `RUSSET_LOG_FORMAT`, `RUSSET_LOG_HTTP_BODIES` | tracing filter, `text\|json`, development-only redacted bodies |
| `RUSSET_DEVICE` | Default device selector |
| `RUSSET_DEVICE_BACKEND` | `live` (default), `scripted:<scenario.toml>`, `replay:<trace>`, `mock:<addr>` (QA T5), `none`. Read only when a daemon is spawned; the chosen backend is recorded in `russetd.json` and `initialize.result.server.deviceBackend`, and a client whose env asks for a different backend than the running daemon's gets `ipc.buildMismatch` in development builds |
| `RUSSET_HIL_UDID`, `RUSSET_TUNNEL_PCAP` | Enable hardware tests; pcap the userspace tunnel |
| `RUSSET_ASC_KEY_ID`, `RUSSET_ASC_ISSUER_ID`, `RUSSET_ASC_KEY_P8` (PEM or base64) or `RUSSET_ASC_KEY_PATH` | CI account; memory only [D 13] |
| `RUSSET_SIGNING_P12_B64`, `RUSSET_SIGNING_P12_PASSWORD` | CI signing identity |
| `RUSSET_SECRETS_BACKEND` (`auto\|keyring\|passphrase\|env`, plus `memory` for tests), `RUSSET_SECRETS_PASSPHRASE` | Secret store selection. With `RUSSET_HOME` set, the keyring entry is per home (§3.3 item 7) |
| `RUSSET_ENDPOINTS_FILE`, `RUSSET_OFFLINE=1` | Fake endpoints; refuse non-loopback HTTP (tests) |
| `USBMUXD_SOCKET_ADDRESS` | Consumed by idevice and pymobiledevice3 (mock usbmuxd) [V QA V12] |
| `SOURCE_DATE_EPOCH`, `NO_COLOR`, `CI` | Determinism; colour; non-interactive mode and no update checks |

---

## 15. Directories

| Purpose | Linux | Windows |
|---|---|---|
| config | `$XDG_CONFIG_HOME/russet` (`~/.config/russet`) | `%APPDATA%\Russet\config` |
| data (toolchains, SDK views, identities, profiles, anisette, sealed secrets, overlays) | `$XDG_DATA_HOME/russet` | `%LOCALAPPDATA%\Russet\data` |
| state (jobs, devices, sessions, crashes, uploads, audit, logs) | `$XDG_STATE_HOME/russet` | `%LOCALAPPDATA%\Russet\state` |
| cache (downloads, device symbols, LLDB module cache, Flutter engine) | `$XDG_CACHE_HOME/russet` | `%LOCALAPPDATA%\Russet\cache` |
| runtime (socket, locks, session sockets) | `$XDG_RUNTIME_DIR/russet` (`/run/user/1000/russet` [V]) | `%LOCALAPPDATA%\Russet\run` + named pipes |
| Swift SDK bundles | Russet imports: `data/sdk/<id>/bundle/`, used through `swift build --swift-sdks-path`; xtool bundles are read in place in `~/.swiftpm/swift-sdks/` (never copied) | Russet imports only, in the short root `%LOCALAPPDATA%\Russet\s\<id8>\` (§16.1) [D 69] |
| stable CLI shim | `~/.local/share/russet/bin/russet` | `%LOCALAPPDATA%\Russet\bin\russet.exe` |
| project-local | `<project>/.russet/` (gitignored, disposable, never secrets) | same |

`RussetDirs::ensure()` creates these 0700 and checks the owner (Unix) or sets a user-only ACL (Windows). All five move under `RUSSET_HOME` when it is set.

---

## 16. Toolchains, SDK, version lock, TLS

### 16.1 Components and resolution [core §10.1]

| Component | Linux resolution order | Windows |
|---|---|---|
| Swift 6.4 | project pin → managed swift.org tarball → system (`/usr/lib/swift`, AUR `swift-bin` [V]) → swiftly | managed installer or winget `Swift.Toolchain`; needs VS 2022 + Windows 11 SDK 22621 |
| Darwin SDK | Russet-imported bundles (`sdk.import` on `russet-xar`, W16b; iOS-only, about 421 MiB) → xtool bundles in `~/.swiftpm/swift-sdks/`, read in place; on this box xtool's `darwin` bundle, Xcode 27.0 (27A266a), iOS 27.0, 6.8 GB [V] | Russet-imported bundles only, created natively by the same `sdk.import` code (W16b) and validated on the PC in W27b: directory links become junctions and file links hardlinks (copy as the fallback), never NTFS symlinks, so neither Developer Mode nor admin is needed; the short root keeps absolute paths ≤ 259 characters for the native backend [D 69] |
| Linker | the bundle's darwin-tools `ld64.lld` (LLD 21.1.6, patched, reads `arm64e.x1`) → managed darwin-tools → system LLD ≥ 23.1.3. The system LLD 23.1.1 here is too old [V build V10] | managed LLVM 23.1.3 (`ld64.lld.exe`) behind the `russet-tool` trampoline |
| LLDB, lldb-dap | Swift toolchain LLDB 21.0.0 (default flavor `swift`) → distro LLDB ≥ 23.1 (`llvm`) | Swift for Windows LLDB, or the LLVM 23.1.3 MSI [A, W27a] |
| libpython | ELF `NEEDED` of `liblldb` (`libpython3.12.so.1.0`) → `ldconfig` → an existing uv-managed PBS (`~/.local/share/uv/python/cpython-3.12.*-linux-x86_64-gnu`, provenance `uv`) → managed `python-shared` (PBS 3.12.15) [D 37] | delay-loaded on Windows [A] |
| xtool | `xtool` on PATH; optional: the `xtool` build backend, and reading an existing xtool SDK bundle in place. `sdk.import` never needs it (native `russet-xar`, W16b) | not needed |
| Flutter | managed → PATH | managed → PATH |

**Compatibility rules** (`resolve_build_env`):
- the SDK's Xcode row sets the Swift minor version (Xcode 27 → 6.4), otherwise `sdk.incompatible`;
- the linker must be darwin-tools ≥ 1.1.0 or LLD ≥ 23.1.3, otherwise `toolchain.linkerUnsupported`;
- for Flutter, the `gen_snapshot` Dart revision must equal the engine's.

### 16.2 The env contract and LLDB [core §10.4, RD §8]

- `ToolInfo.env` is part of the contract. Every launcher applies it: daemon children, the DAP hub's `lldb-dap`, `russet debug`'s interactive `lldb`, and `--print-config`.
- **On Linux, Swift LLDB gets:**
  - `LD_LIBRARY_PATH=<pbs>/lib:<swift>/usr/lib/swift/linux:<swift>/usr/lib/swift/host/compiler`;
  - `PYTHONHOME=<pbs>`.

  `<pbs>` is the Russet-provisioned `data/toolchains/python-shared/3.12.15/` (34 MB download, about 107 MB installed). A matching system `libpython3.12.so.1.0`, or an existing uv-managed PBS 3.12 (111 MB at `~/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu` on this box), is used when present, so nothing is downloaded twice. This works with python-build-standalone 3.12.15 [V RD V1; re-checked by the lead 2026-10-11: Swift LLDB's `script` printed `3.12.15` with the uv prefix, and `lldb-dap --help` ran].
- **Python is a load-time dependency only.** Russet's generated LLDB commands contain no `script` lines [RD §9.1].
- **Swift `po` off-Mac** needs a reflink overlay that points LLDB's Swift resource directory at the SDK's `XcodeDefault.xctoolchain/usr/lib/swift`, plus `target.sdk-path` [V RD V6, fake process]. Status on the device is device experiment D7 (W11).

### 16.3 SDK facts and the platform view [build §3.4]

- **Reading the SDK.** `russet-sdk::ImportedBundleProvider` reads Russet's own imports, and `XtoolBundleProvider` reads an existing xtool bundle **in place** (Linux): SDKSettings, `SystemVersion.plist`, the platform `Info.plist` `AdditionalInfo`, the platform `version.plist`, the Xcode `version.plist`, and the back-deploy dylib directories.
- **The platform view** is `data/sdk/<id>/view/Developer/…`. It holds symlinks into the bundle plus `usr/local/bin/{actool, ibtool, xcstringstool, …}` pointing at `russet-tool`, and is passed as `XCODE_EXTRA_PLATFORM_FOLDERS`. Swift Build ignores `PATH` for tools [V build V7–V8].
- **Russet imports (W16b) keep an iOS-only want-set** [D 69]; anything missing fails the build with `sdk.incomplete`:
  - `iPhoneOS.platform`: `Info.plist`, `version.plist`, `Developer/{SDKs, Library/{Frameworks,PrivateFrameworks}, usr/lib}`;
  - toolchain `usr/lib/swift`: `iphoneos` **with** its `prebuilt-modules` (an `import Foundation` typecheck takes 1.15 s with them and 33.4 s without), `apinotes`, `swiftToCxx`;
  - `usr/lib/clang/21/include` and the four iOS-only `libclang_rt.*` files (3.8 MiB);
  - `usr/lib/swift-{5.0,5.5,6.2}/iphoneos` (back-deployment dylibs);
  - `Contents/{Info.plist,version.plist}` (the Xcode version and build);
  - `Contents/Resources/Packages/XcodeSystemResources.pkg`, spooled to a temporary file for the DDI.

  That is about 421 MiB in about 9.8k files, against 2.86 GiB in 47.7k files for xtool's slim `Developer/` tree and 6.8 GiB for its normal bundle, where these files live only in the `Xcode.app` copy (3.7 GB of the 6.8 GB) [V lead `du`; V review 2026-10-11]. xtool 1.21.0 tries to drop other platforms' `prebuilt-modules` but tests the wrong path component (index 9 instead of 8), so it keeps about 1.2 GiB of them.
- **Import pipeline** (`russet-xar`, streaming):
  1. Verify the xar header and the SHA-1 TOC checksum.
  2. Verify the RSA signature: a prehashed PKCS#1 v1.5 signature over the raw checksum, chained to the pinned Apple Root CA (§16.6). The embedded root and leaf expiry are ignored, so archived Xcodes still import.
  3. Stream the pbzx `Content` (16 MiB chunks, decoded in parallel, consumed in order) into the odc cpio parser.
  4. Classify each entry against the want-set; defer links to a link pass that resolves chains and refuses targets outside the import root; spool `XcodeSystemResources.pkg`.
  5. Post-process (below), then write `info.json` with the host triples `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-windows-msvc` and `aarch64-unknown-windows-msvc` (SwiftPM 6.4 filters bundles by host triple: "No Swift SDK found matching query … and host triple …"), `swift-sdk.json` (schema 4.0), the toolsets and `darwin-sdk-version.txt`.
  6. Rename into place atomically and register.

  Budget on the Ryzen 5 5600X: ≤ 3 min for a full `Xcode_27.0.xip`, peak RSS ≤ 1 GiB, ≤ 0.5 GB of temporary disk beyond the output. About 10 GB of cpio is decoded to keep about 0.45 GB; XZ decodes at about 145 MiB/s per core, so I/O dominates.
- **Post-processing** (ported from xtool's MIT `SDKBuilder`, with attribution), always in Russet's own copy:
  - patch the platform `Info.plist` `DefaultProperties` (`SWIFT_RESOURCE_DIR`, `CLANG_RESOURCE_DIR`, `ALTERNATE_LINKER=lld`, `CODESIGN`). On Windows `CODESIGN` points at `russet-tool`'s `codesign.exe`, because `true` is not an executable there;
  - link `Testing`, `XCTest`, `XCUIAutomation` and `XCTestCore` into the SDK's `System/Library/Frameworks` (junctions on Windows);
  - overlay the host clang `include` directory, found through `swiftc -print-target-info`;
  - install the OpenAppleMacros server and plugin stubs. Windows needs a windows-x64 server built in Russet CI, and the stub naming there is unverified (W27b);
  - name the librarian `llvm-lib(.exe)`, because Swift Build skips `-V` discovery for that basename, but make it `llvm-libtool-darwin`, never LLVM's MSVC-style `llvm-lib`.
- **DDI extraction** [D 62]. When the source is an `Xcode.xip` or `Xcode.app`, `sdk.import` also extracts the iOS Developer Disk Image, streaming, before anything is deleted. Xcode 27.0 keeps it only inside `XcodeSystemResources.pkg`; `Contents/Resources/CoreDeviceDDIs` no longer exists [V review, Mac oracle read-only, 2026-10-11]:
  1. `Xcode.xip` (xar, signed) → `Content` (pbzx → cpio) → `Xcode.app/Contents/Resources/Packages/XcodeSystemResources.pkg`: 146,413,318 B, xar v1 (`PackageInfo`, `Scripts`, `Bom`, `Payload`), SHA-1 TOC checksum, RSA signature by "Software Update" ← Apple Software Update Certification Authority ← Apple Root CA (SHA-256 `B0B1730E…01F024`). From an `Xcode.app`, start at the `.pkg`.
  2. `Payload` (145,985,896 B of pbzx with independent XZ chunks → cpio odc) → `./Library/Developer/CoreDevice/CandidateDDIs/iOS_DDI.dmg`: UDIF UDZO (zlib, CRC32), unpartitioned whole disk, 32.7 MB on disk, 35,356,672 B raw.
  3. Journaled HFS+ volume "Xcode_iOS_DDI_Personalized" (no decmpfs) → `data/ddi/<ddiBuild>/iOS/{Restore/, version.plist}`.

  `Restore/BuildManifest.plist` (ProductBuildVersion 27A266a) carries both DDI forms:
  - **Personalized** (iOS 17–26.x, through the image mounter): 140 "Customer iOS Developer PDI" identities, all pointing at `022-20190-411.dmg` (`PersonalizedDMG`) and `Firmware/022-20190-411.dmg.trustcache` (`LoadableTrustCache`); board-specific.
  - **Cryptex1** (iOS 27.0+, through cryptexd; installable from iOS 26.4): one "iOS Customer Developer Disk Image Cryptex" identity (index 140; ChipID 0xFF10, ProductClass 0xF2, NonceDomain 4) with `022-20231-411.dmg` (`Cryptex1,GenericDmg`) and `Firmware/022-20231-411.dmg.{trustcache,cryptex_info,root_hash}`; device-agnostic, so it also covers boards missing from the PDI list. `cryptex_info`: `com.apple.MobileAsset.DDI` 27.1.266.1, mount path `/System/Developer`, Developer Mode required.
  - **Integrity:** each manifest `Digest` is the SHA-384 of its image, so every import (any source) is checked offline before it is stored.

  The extracted files are byte-identical to the Mac's `/Library/Developer/DeveloperDiskImages/iOS_DDI` (`.dmg` SHA-256 `67d44aa6…a426`). A DDI is keyed by its own `ProductBuildVersion` (`ddiBuild`): it equals the Xcode build for Xcode-derived DDIs, differs for Device Support components (DDI 16C7011 for Xcode 16C5032a), and is unknown for copies of a Mac folder until `version.plist` is read. xtool's bundle keeps only `Xcode.app/Contents/{Developer,Info.plist,version.plist}`, so importing from it reports `ddi: missing`.
- **How macOS does it** (for reference; Russet copies none of it). Xcode's first launch installs `XcodeSystemResources.pkg`, which lays down `/Library/Developer/CoreDevice/CandidateDDIs/*_DDI.dmg`; `xcodebuild` then runs `devicectl manage ddis update`, which fills `/Library/Developer/DeveloperDiskImages/<Platform>_DDI`. CoreDevice has no network DDI source. DDIs for hardware released after an Xcode ships arrive as "Device Support" components that carry the same `.pkg`. Today Apple's CDN offers no standalone iOS 27 DDI (Pallas `XcodeDeviceSupport`: "Active not found"; `xcodebuild -showComponent DeviceSupport`: "No updates"), so the only Apple-hosted copy is inside `Xcode_27*.xip` on download.developer.apple.com, behind an Apple ID sign-in.
- **Codec scope for DDIs:** xar, pbzx, cpio odc, UDIF raw/zero/zlib and read-only HFS+. bzip2 and lzfse are optional.
- **Mounting** (W4, W10). iOS ≥ 27.0 uses the Cryptex1 identity through cryptexd; iOS 17.0–26.x uses the PersonalizedDMG through the image mounter, and falls back to the cryptex on 26.4+ for boards missing from the PDI list. Payload paths always come from the identity (§5.3 `ddi`). cryptexd installs use Xcode's persistence 2 and nonce-persistence 1, which may survive phone reboots (pymobiledevice3 saw 1 and 2 survive on iOS 26.4; unverified on the rig, DC-28 records it), so `ensure_ddi` remounts **when missing**, not after every reboot. idevice 0.1.68 sends `nonce_persistence` as `persistence`; Russet carries the one-line fix (§16.7, [D 22]).

### 16.4 Link-time SDK version and metadata policy [build §4.3–§4.4, SIGN §9.5] [D 17]

1. **Link-time SDK version: always on.** Toolsets add `-Xclang-linker -isysroot -Xclang-linker <SDK>`, so every Russet-built Mach-O says `sdk 27.0, minos <deployment target>`, as Xcode output does [V build V5–V6]. This is a truthful fact of the link, not stamping.
   - **Consequence:** iOS 27 "linked on or after" rules apply. TN3187 requires the UIScene life cycle, so Russet always generates `UIApplicationSceneManifest` unless the app has one.
   - **Escape hatch:** `[build] overrides = { RUSSET_LINK_SDK_VERSION = "deployment" }` restores sdk = minos, for diagnosis only.
2. **`Info.plist` `DT*` keys by policy.** `russet ship stamp` uses the same module on existing archives.

   | Key (Xcode 27 value on this box) | `none` | `minimal` | `sdk` (default) | `xcode-compat` (opt-in + consent) |
   |---|---|---|---|---|
   | `DTPlatformName` (`iphoneos`), `DTPlatformVersion` (`27.0`), `DTSDKName` (`iphoneos27.0`) | – | ✓ | ✓ | ✓ |
   | `DTSDKBuild` (`24A430`), `DTPlatformBuild` (`24A430`) | – | – | ✓ | ✓ |
   | `DTCompiler` (`com.apple.compilers.llvm.clang.1_0`), `DTXcode` (`2700`), `DTXcodeBuild` (`27A266a`) | – | – | – | ✓ |
   | `BuildMachineOSBuild` | – | – | – | only with an explicit value |
   | `RussetToolchain` marker `{tool, version, host}` | – | – | – | ✓ (always with xcode-compat; visible disclosure) |

   - `xcode-compat` requires `[build] metadata-consent = {by, at}` committed in `russet.toml`. Values come only from the user's imported SDK, never constants.
   - DT keys of every Russet-built nested bundle are normalized. This fixes Swift Build's `DTXcode=10989` / `DTXcodeBuild=99T999` on Linux [V build V9].
   - Whether App Store builds should default to `xcode-compat` is owner question §21 Q2.

### 16.5 Bundle IDs [build §4.5] [D 18]

| Policy | Effective bundle ID | Default for |
|---|---|---|
| `exact` | `CFBundleIdentifier` = `[app] bundle-id`; extensions are `<app id>.<suffix>` | paid teams |
| `team-suffix` | `<id>.<TEAMID>` | free teams |
| `xtool-prefix` | `XTL-<prefix>.<id>` (prefix = first segment of the xtool identity id, uppercased [V build V12]) | opt-in: updating apps installed by xtool; Wave 1 slice |
| `custom` | explicit map | opt-in |

The build plane writes an original → effective map; the signer rewrites entitlements (`application-identifier`, keychain and app groups).

### 16.6 Crypto provider and TLS [D 21]

- **One rustls provider: `ring`.**
  - idevice uses `default-features = false` with the `ring` feature [device §5.1].
  - reqwest 0.13.5 uses `rustls-no-provider` [V lead: reqwest 0.13.5 `Cargo.toml` features].
  - Each binary calls `rustls::crypto::ring::default_provider().install_default()` at startup.
- **Why not aws-lc-rs:**
  - apple-codesign 0.29.0 already depends on `ring 0.17` **unconditionally** [V lead: apple-codesign 0.29.0 `Cargo.toml`];
  - ring builds with only a C compiler on MSVC and in mingw cross-builds, while aws-lc-sys needs CMake and NASM on Windows [device §5.1, core C14].
- **Gate.** `deny.toml` bans `aws-lc-sys`, `aws-lc-rs`, `openssl`, `openssl-sys` and `native-tls`.
  - apple-codesign also pulls reqwest 0.12 (with `rustls-tls-native-roots`). Whether that path stays ring-only is [A], and W1 checks that `cargo tree -i aws-lc-sys` is empty.
  - Two reqwest major versions in the tree are accepted (cargo-deny `multiple-versions = "warn"`).
- **Roots.**
  - GSA, and Apple's Pallas asset catalog (`gdmf.apple.com`, used only by the deferred Device Support check): a dedicated root store with Apple Root CA, fetched from `apple.com/appleca/AppleIncRootCertificate.cer` at runtime and SHA-256-pinned (`B0:B1:73:0E:…:01:F0:24`) [V SIGN V15].
  - The same pinned Apple Root CA anchors the xar signatures of `.xip` and `.pkg` files (§16.3).
  - Everything else, including `devimages-cdn.apple.com`, `download.developer.apple.com`, `developerservices2.apple.com`, `swscan.apple.com`/`swcdn.apple.com`, `mesu.apple.com` and App Store Connect: the WebPKI and native roots [V review 2026-10-11].

### 16.7 Key pins (exact versions live in `Cargo.lock`, `pins/stable.toml`, `ide/upstream.json`)

| Component | Pin | Source |
|---|---|---|
| Rust | `rust-toolchain.toml` `channel = "stable"` (+ rustfmt, clippy), workspace `rust-version = "1.99"`; CI pins `RUSTUP_TOOLCHAIN=1.99.0`. Naming `1.99.0` in the file would make rustup 1.29.1 auto-install a second copy next to the installed `stable` (rustc 1.99.0), 0.6–1.5 GiB on a full disk [D 70] | core §4.1 |
| idevice patch | `[patch.crates-io] idevice = { git = "<Russet's fork of jkcoxson/idevice>", rev = "<reviewed rev>" }` with a `deny.toml` `allow-git` entry. The rev is upstream `3854a5df4a5a6dee71ffce4d8befc2ea356a8065` plus one Russet commit with two fixes: (1) `CryptexdClient::install()` must send `request.persistence` (Xcode's 2) as `persistence`, not `request.nonce_persistence` (`src/services/cryptexd.rs:273-274` in 0.1.68 and at HEAD); (2) a cap on usbmuxd packet sizes: 3854a5d rejects a declared size below 17, but still allocates `vec![0; size - 16]` for any larger value, up to about 4 GiB (`idevice/src/usbmuxd/mod.rs:467-474`), so a declared size above 16 MiB must return `UsbmuxdError::BadCommand` before allocating, or QA's mock-usbmuxd fuzzing can exhaust memory [V lead, `~/russet-lab/idevice` at 3854a5d, 2026-10-11]. Upstream 3854a5d differs from crates.io 0.1.68 only in `src/usbmuxd/mod.rs` [V lead `diff -rq`, 2026-10-11]: the short-packet (`u32 - 16`) check, a `Send` listen stream and `Disconnected(u64)`. W4 creates the fork commit and files an upstream PR for each fix (DEV-23); the patch goes when a release carries all of them [D 22] | §20 |
| tokio / tokio-util / serde_json / schemars / ts-rs | 1.53.2 / 0.7.20 / 1.0.152 / 1.2.2 / 12.0.1 | core §4.5 |
| idevice / jktcp | `=0.1.68` (crates.io, patched as below) / `=0.1.7` [D 22] | device §5.1, QA V5 |
| apple-codesign | 0.29.0, `default-features = false` | SIGN §3.1 |
| rmcp | 3.5.1 (MCP 2026-07-28 + 2025-11-25) | IDE V15 |
| reqwest / rustls / ring | 0.13.5 / 0.23.x / 0.17.14 | §16.6 |
| keyring-core + stores | 1.0.0; zbus-secret-service-keyring-store 1.0.1; windows-native-keyring-store 1.1.0 | core §4.5 |
| symbolic | 13.9.0 (Swift demangling) | RD §27 |
| vscode-jsonrpc / ACP SDK | 9.0.3 / `@agentclientprotocol/sdk` 1.8.0 | IDE §0 |
| Code-OSS / VSCodium / Electron / Node | 1.135.0 (`08d4889f`) / 1.135.06055 / 42.8.1 / 24.18.0 [D 34] | IDE §3.2 |
| Swift / Xcode SDK / darwin-tools / LLVM (Windows) | 6.4.0 / 27.0 (27A266a) / 1.1.0 / 23.1.3 | core §10.2 |
| python-build-standalone | cpython 3.12.15, release 20261009, `install_only_stripped` | RD V14 |
| Flutter / Dart | 3.47.7 / 3.13.5 | build §2 |
| pymobiledevice3 (optional, user-installed) | ≥ 11.26.0 | device §12 |
| xtool (optional build backend; xtool bundles read in place; the CI template's SwiftPM project generator, hash-pinned) | 1.21.0 | core §10.1, §14.7 |
| SDK import codecs | `rsa` 0.9.10 (verify only), `liblzma` 0.4.8 or `lzma-rust2` 0.21.0, `junction` 2.1.0 (Windows), `udif` 0.4.2 / `hfsplus` 0.4.0 (optional) | §5.1 `russet-xar` |
| GitHub Actions | runner labels and action SHAs in `pins/stable.toml` `[ci.github]` | §14.7 |

---

## 17. Key flows

### 17.1 `russet debug` on the SwiftUI sample (the Wave 1 vertical slice)

```text
russet debug --break ContentView.swift:12            (cwd = tests/fixtures/apps/RussetProbe; russet.local.toml maps the bundle id)
 0  CLI: connect_or_spawn → initialize {kind: cli, interactive: true} → debug.prepare {project, frontend: lldb-cli}   [job kind debug]
 1  pipeline.resolve   project.detect; toolchain.resolve (Swift 6.4, darwin SDK, linker, lldb + env); device readiness;
                       ensure_ddi (mount from [device.ddi] when missing; Wave 1: the configured Apple-origin directory, §16.3)
 2  pipeline.build     russet-build: generated builder package → `swift build --build-system swiftbuild --toolset toolset-swb.json`
                       → assemble .app (scene manifest, DT policy `sdk`, rpath hygiene) → BuildOutput + SigningPlan
 3  pipeline.provision russet-account Provisioner::plan (GET-only ASC; or [signing] mode = manual + profile path)
                       Wave 1: the plan has 0 mutations (existing profile for XTL-<prefix>.com.evilcomputer12.orchard.hello)
 4  pipeline.sign      russet-sign: copy to .russet/build/debug-iphoneos/signed/, inside-out signing with the imported identity, verify
 5  pipeline.install   ledger state/devices/<udid>/installs.json by CDHash; if changed: russet-ipa (dev) → AFC PublicStaging → installation_proxy
 6  launch             russet-device: CoreDevice appservice launchapplication {startStopped, openstdiosocket}; keep the channel open
                       (fallback: DVT processcontrol suspended)
 7  broker             connect_rsd("com.apple.internal.dt.remote.debugproxy") → handshake → QSetDetachOnError:0 → vAttach;<pid> (pre-dyld)
                       → listen runtime/sessions/<sid>/gdb.sock (one client)
 8  result            PreparedDebug {sessionId, pid, lldb ToolInfo + env, attachCommands, broker.lldbUrl};
                       the daemon has written .russet/debug/<sid>/attach.lldb (Python-free)
 9  CLI               appends the --break commands and spawns `lldb -s <attach.lldb>` with ToolInfo.env:
                         platform select remote-ios · target create --arch arm64-apple-ios <local exe> ·
                         target modules search-paths add <device .app> <local .app> · process connect unix-connect://…/gdb.sock
                         (returns stopped) · breakpoint set --file ContentView.swift --line 12 · process continue
10  session           stdout (CoreDevice stdio) + os_trace(pid) → run.output; exit → run.exited; abnormal exit → crash pipeline (Wave 2)
```

Wave 1 fallback: `--attach-mode legacy-async` runs the M1-verified async connect, `process attach --pid`, `continue` sequence on a plain forward, if device check D3 fails for the broker.

### 17.2 F5 in the IDE (Wave 2)

1. `resolveDebugConfigurationWithSubstitutedVariables` calls `debug.prepare {frontend: dap}`, showing `job.*` progress.
2. It returns `__russet: {sessionId, dapPath}`.
3. `createDebugAdapterDescriptor` returns `DebugAdapterNamedPipeServer(dapPath)`.
4. The hub's `lldb-dap` runs `attachCommands` (prefixed with `!`), then emits `initialized`.
5. VS Code sends `setBreakpoints`; `configurationDone` resumes the app.
6. Hot Reload on Swift is Quick Relaunch: incremental build, delta install, relaunch suspended, re-attach, and the hub replays breakpoints. Target ≤ 8 s.

### 17.3 Flutter debug on iOS 26/27 (Wave 3) [RD §4.4, build §6]

1. `flutter run -d russet-<udid>`.
2. flutter_tools builds the kernel, then calls the custom-device hooks `russet flutter device install`, `run-debug` and `forward`.
3. **install** (W17): build `Runner.app` (storyboard-free UIScene Runner; plugins through SwiftPM; kernel reused), sign, install.
4. **run-debug** (W18):
   1. launch suspended with the engine flags;
   2. broker `vAttach`;
   3. arm the JIT hook `NOTIFY_DEBUGGER_ABOUT_RX_PAGES` (run to main, find Flutter.framework, set a breakpoint);
   4. service every RX stop natively (pipelined `M` writes, `pc = lr`);
   5. print stdout, including the VM-service line.

   Page touching is enforced where the code-signing monitor is TXM: A15/M2 and later on iOS 26, and every iOS 27 iPhone and iPad except the A12Z iPad Pro (iPad8,9–8,12). **The rig's iPhone 12 runs TXM on iOS 27.0:** the 24A437 BuildManifest lists `Ap,TrustedExecutionMonitor` (byte-identical to the A15+ image) and `Ap,SecurePageTableMonitor` (`sptm.t8101`), and its kernel has no PPL segments [V review 2026-10-11, firmware only; verified-facts]. JIT runs on the rig therefore exercise TXM. `Dart execution mode: JIT` proves only the VM's one-page start-up test; a page the servicer misses later crashes the app with `EXC_BAD_ACCESS`/`SIGBUS` (flutter#190307). M2 therefore also needs DC-30 on the rig: every page of 1- to 2048-page regions runs, a never-announced control page faults, and `pagesTouched == pagesNotified`. The rig does not cover the iOS 26 TXM build (A15+ phones still on 26.x) or non-TXM devices, where Dart still calls the hook; recorded traces and community reports cover those [D 71].
5. **forward:** a TCP forward for the VM service.
6. flutter_tools then runs DevFS, hot reload and DevTools.

### 17.4 Agent loop with an approval (Waves 2–4)

1. The agent calls `device_list`, `build`, `app_run`, `screenshot_take`, `input_tap`, `logs_read` at tiers 0–1, with no prompts.
2. `ship_upload {ipa}` returns `isError` with `perm.approvalRequired` and an `approvalId`.
3. The daemon broadcasts `approval.requested`. The IDE shows a notification; the Approval panel shows the full arguments and needs the "I understand this publishes to Apple" checkbox for tier 3.
4. The human approves. The agent retries with `approval`; the daemon checks the binding, runs the upload job and writes the audit line.

### 17.5 Ship (Wave 4) [SIGN §9]

- **Linux-native:**
  1. `build.start {configuration: release}`
  2. `ship.archive`, with dSYMs from `dsymutil`
  3. `sign.run {distribution: app-store-connect}`
  4. `ship.validate` (about 45 offline checks)
  5. optional `ship.stamp` (policy and consent, §16.4)
  6. `ship.upload` (approval; buildUploads → parts → commit with MD5 → poll to `VALID`)
  7. `ship.testflight.distribute`
  8. App Store [D 64]: `ship.metadata.plan/apply` (text and screenshots), `ship.version.plan/apply` (create the `appStoreVersion`, attach the build, export compliance), then `ship.review.submit` (typed confirmation in the CLI or the Publishing panel; never MCP); `ship.review.status` follows it.
- **Apple-hardware mode** [D 76]:
  1. `ship.ci.init github` generates `.github/workflows/russet-release.yml` and its `.russet/ci/` helpers [SIGN §9.9]:
     - `runs-on: ${{ vars.RUSSET_RUNNER || 'xcode-27' }}`. The label comes from the imported SDK's Xcode major through `pins` `[ci.github]`: 27 → `xcode-27` (public preview: no SLA, possible queueing), 26 → `macos-26`. `macos-26` and `macos-latest` carry only Xcode 26.0.1–26.6; there is no `macos-27` label, and the GA name is unannounced [actions/runner-images#14404, macos-26-arm64-Readme].
     - Exact Xcode selection: `DEVELOPER_DIR=/Applications/Xcode_<major.minor>.app/Contents/Developer` (this symlink survived the image's RC→GA path rename). Never a glob: `Xcode_27*.app` also matches `Xcode_27.1_beta.app` and `Xcode_27.2_beta_2.app`. A real path containing "beta" is refused, and `xcodebuild -version` must start with the requested version.
     - The archive is always **unsigned**: `CODE_SIGNING_ALLOWED=NO`, no `-allowProvisioningUpdates`, no API key. Nothing is minted on the ephemeral runner, and no registered device is needed.
     - Signing mode `russet` (default): the runner returns the unsigned `.xcarchive`; Russet signs it locally with the same Apple Distribution identity as the Linux-native path, validates and uploads it. No Apple credentials go to GitHub, and it works with the App Manager key.
     - Signing modes `xcode-manual` and `xcode-cloud`: every nested bundle is first pre-signed, deepest first, with a throwaway identity and Russet-computed entitlements; without that, `-exportArchive` drops capability entitlements such as `aps-environment`. Xcode then exports with the distribution identity from GitHub secrets (`xcode-manual`) or with cloud-managed signing (`xcode-cloud`), which needs an **Admin-role** team key: App Manager keys fail with "Cloud signing permission error" (7009; §21 Q5).
     - Both Xcode modes verify the exported IPA's per-bundle entitlements before anything is uploaded.
  2. `ship.ci.dispatch`, then `ship.ci.fetch`, which deletes the run's artifacts after the download.
  3. Russet's own uploader sends the build.
  4. `ship.compare` reports the differences between the two paths.

---

## 18. Platform matrix

| Capability | Linux | Windows native | WSL2 split | Notes |
|---|---|---|---|---|
| `russet` CLI, `russetd`, IPC | W1 | W1 compiles; W27a hardening (Job Objects, pipe DACL, spawn flags) | W27b, only if needed (TCP + token, path mapping) | Windows is tested on the physical PC, reached over SSH (PLAN §8) |
| Device discovery, pairing, tunnel | W4 (usbmuxd, no root) | W27a (Apple Mobile Device Service from iTunes or the Store app, or netmuxd) | daemon on Windows | Store-app AMDS has no Wi-Fi discovery [device §10] |
| Install, launch, logs, forwards | W4 | W27a | via the Windows daemon | |
| Debug (Swift LLDB + broker) | W1 (PBS libpython) | W27a (Swift for Windows or LLVM MSI LLDB; TCP broker; named-pipe DAP hub) | LLDB on Windows, or mirrored networking | |
| Mirror and HID input | W10 (HEVC via VA-API, AVC software fallback) | W27a (D3D11 decode [A]; AVC fallback) | Windows daemon | Verified on AMD/Mesa [V IDE V10] |
| SDK + DDI import (`.xip`) | W16b (native) | the same code (W16b), validated on the PC in W27b (junctions and hardlinks, short root, no admin, no Developer Mode) | not used: the Windows daemon imports natively | [D 69] |
| SwiftPM build | W5 (Swift Build backend) | W27b, native first: the native backend with the short root stays under 260 characters; the swiftbuild backend's platform view adds a 66-character prefix to SDK paths of up to 171, so it needs `LongPathsEnabled`; junctions, `.exe` shims, response files | documented fallback only, if the native build plane fails (build Q7, settled) | build §10 [D 69] |
| Flutter debug | W17/W18 | W27b spike (custom devices declare `linux-*`) | split mode, if needed | build Q8 |
| Signing, IPA, ASC upload | W6/W12/W23 (pure Rust) | same code; W27c tests | — | |
| Secrets | W3 (Secret Service, passphrase fallback) | Credential Manager master key (compiled W3; tested W27a) | — | 2,560-byte limit avoided by sealed files |
| Free Apple ID (anisette) | W22 (adi-host with seccomp + Landlock) | W27c (sysv64 shims, TLS handling [A]) | — | opt-in |
| IDE | W25 (deb, rpm, AppImage, tar.gz, AUR) | W25 CI (Inno user/system, zip; MSI later); verified on the PC in W27c | — | SmartScreen until signing (§21 Q8) |
| Packaging of the CLI | W26 (tar.zst, deb, rpm, AUR) | W27c (zip, per-user MSI, winget via komac) | — | |
| Wi-Fi devices | W30 | W30 | — | |

---

## 19. Licensing, Apple files and trademarks

- **Licences.**
  - Russet is **GPL-3.0-or-later**. Layer-F crates and `russet-tool` are **MIT** (or MIT OR Apache-2.0 for `russet-gdbremote`), so they can go upstream.
  - Templates, fixtures, sample apps and any code injected into user apps (Runner scene delegate, probes) are **0BSD**; the Flutter Runner keeps BSD-3 [QA §9 L9].
- **cargo-deny allowlist:** MIT, MIT-0, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, Zlib, 0BSD, CC0-1.0, Unicode-3.0, BSL-1.0, MPL-2.0, GPL-3.0-or-later.
  - LGPL and anything else only as a per-crate exception after written review [D 48].
  - AGPL code (xcross ≥ 2.0, StikDebug) and unlicensed repositories (android-loader, anisette-v3-server) are reference only, never code.
  - pymobiledevice3 (GPL-3.0) and netmuxd (LGPL-2.1) run only as user-installed subprocesses.
- **REUSE.** Every source file has SPDX headers; `REUSE.toml` covers generated files; `reuse lint` runs in CI.
- **No Apple files** [K4].
  - `cargo xtask guard apple-files` (alias `check-no-apple`) scans the tree, caches and every upload for: Mach-O, BOMStore/CAR, NIBArchive, xar, koly, IMG4 and pbzx magic; `.tbd`, `.xip`, `.ipsw`, `.dmg`, `.mobileprovision`, `.p8`, `.p12`; and Apple copyright headers.
  - **Synthetic container fixtures** [D 88]. Tests build Apple-format inputs at test time, in temporary directories, from Rust builders, as the Mach-O fixtures are: W4's synthetic Restore directory (with `022-…-411.dmg` names), and W16b's xar, pbzx and cpio cases. Only where a builder is impractical (W16b's UDIF and HFS+ images, and the fuzz seed corpora) may generated files be committed, and only under a provenance allowlist: the roots `tests/fixtures/xip/` and `fuzz/russet-xar/corpus/`, each with a committed `PROVENANCE.toml` listing every file's path, SHA-256 and the committed generator script that produced it from open tools. The guard reads provenance files only at roots compiled into `xtask/src/guard.rs`; a file with Apple-format magic or an Apple file extension passes only if it is listed there with a matching hash. A new root is a lead PR.
  - RDT traces elide Apple payloads [QA §3.5].
  - The golden corpus holds only our inputs plus normalized JSON.
  - No `.nib` or `.storyboardc` (NIBArchive magic) is committed, not even a Russet-generated one: IB fixtures are XML inputs plus normalized decoded-graph JSON. Raw oracle `ibtool` outputs stay local (Q21). omarchy-apple-dev's committed Apple goldens are never vendored, and Xcode's own template storyboards are Apple files, so the corpus uses Russet-authored equivalents [D 75].
- **Runtime-only Apple inputs:**
  - the user's `Xcode.xip`/SDK;
  - DDIs per [D 62]: imported from the user's own Apple artifacts (`Xcode_27*.xip`, `Xcode.app`, `XcodeSystemResources.pkg`, `iOS_DDI.dmg`, an Apple Device Support `.dmg`, or a copy of a Mac's `/Library/Developer/DeveloperDiskImages/iOS_DDI`), or a configured directory; later, optionally, a consent-gated fetch of Apple Device Support components from Apple hosts under the user's IP (W33, Q25);
  - Apple's public Xcode downloadables index (metadata only; deferred together with that fetch);
  - Apple TSS (`gs.apple.com`) for DDI personalization;
  - the anisette libraries from Apple's CDN (consent, hash pins, never redistributed);
  - Apple Root CA (public; fetched and pinned).
  - The third-party DDI mirror (doronz88/DeveloperDiskImage; licence: none; it re-hosts Apple files) is used only after an explicit opt-in that says so [D 57]. pymobiledevice3's DDI cache counts as mirror content.
- **Licence note** (not legal advice; owner question Q26). Xcode 27's licence (EA2002) and the Device Support licence (EA1879) authorize the Apple Software only on Apple-branded computers running macOS (§2.5, §2.7). Russet only processes copies the user obtained from Apple, never redistributes them, and the DDI executes on the user's iPhone. W26 owns the legal review; until counsel answers, the public wording is "Apple-origin, user-obtained, never redistributed".
- **apple-codesign embeds Apple's public CA certificates.** This is listed in NOTICE; acceptance is owner question §21 Q9.
- **Trademarks.**
  - Nominative use only. No Apple logos, SF Symbols, SF fonts or device-frame art (the mirror uses a neutral frame).
  - The IDE has no Microsoft branding and uses Open VSX, not the Marketplace [QA §9 L7–L8].
- **Private-track terms** [D 91]. This repository goes public at G1 (Q6); the virtual-iPhone track stays in its own private repository. `cargo xtask guard public-terms` fails on terms that identify that track's approach anywhere outside an allowlist. The plaintext list lives in the private repository; `xtask/src/guard.rs` stores only SHA-256 digests of lowercased tokens and token pairs, so the guard does not publish the terms. The allowlist holds only the files Q34 covers (`docs/decisions.md` and `docs/research/macos-on-x86-feasibility-2026-10.md`) until the owner answers it, and must be empty before the repository goes public (PLAN §5.9).

---

## 20. Conflict resolutions

Each row names the disagreeing documents, the decision, and the reason. Rows are referenced as [D n] above.

| # | Topic | Proposals | Decision | Why |
|---|---|---|---|---|
| 1 | Binary layout | core: one multi-call binary (open Q1) | One `russet` binary (CLI + daemon + MCP bridge + flutter device) | No version skew; one artifact; in-process daemon for tests |
| 2 | Event envelope | QA: one `event` notification; SIGN: `job.progress`/`job.done`; build: `build.event`; core: named | Named notifications, subscription-scoped; `job.updated` + typed `job.events` replace all job/build event variants | Typed schemas, generated TS, works with vscode-jsonrpc handlers |
| 3 | Long-running semantics | SIGN: return `{jobId}` immediately; core: request stays open | core's: stays open unless `background: true` (called `detach` before [D 65]); every long method is also a job | One model for CLI, IDE and MCP (MCP adds the 50 s rule) |
| 4 | Approval RPC names | QA/IDE `approvals.*`; SIGN `approval.pending/grant/deny` | `approval.list/get/grant/deny` + `approval.requested/resolved`; one store for plan and tool approvals | Singular namespaces like the rest of the catalog |
| 5 | MCP confirmations | core: elicitation for `confirm`; QA/IDE: out-of-band approvals | Out-of-band approvals for tiers 2–3; elicitation is optional and never sufficient | An agent harness can auto-answer elicitation |
| 6 | Permission model | core classes vs QA tiers | Both: class per method, tier = f(class) for MCP (§7.5) | One enforcement point in the daemon |
| 7 | Error codes | build/SIGN `E_*`; QA `RUS-<AREA>-<n>`; device 1100–1191; core numeric | Numeric `error.code` + dotted `russetCode`; device remapped to 3000–3299; `E_*` names converted (§13) | Device range collided with `ipc`; one registry, one docs anchor |
| 8 | CLI exit codes | SIGN's own table | core §13.3 only | One table for all commands |
| 9 | Socket name | QA/IDE `daemon.sock` | `russetd.sock` | core owns the transport |
| 10 | Mirror transport | core: IPC byte-stream relayed by the extension host; device + IDE: WebSocket | Loopback WebSocket in `russetd`, protocol `russet.mirror.v1` (IDE spec supersedes device's frame format); token in the first message | Webviews cannot use Unix sockets; the IDE spec is complete; decode stays off the extension host |
| 11 | MCP HTTP host | core: `russet mcp serve --http`; IDE: inside `russetd` | Inside `russetd` (opt-in, persistent for Docker agents); `russet mcp serve --http` stays for ad-hoc foreground use with the same code | OpenHands GUI needs a stable endpoint |
| 12 | MCP tool names | device `ios_*`; build/SIGN/RD `russet_*`; IDE unprefixed | IDE's unprefixed snake_case list (§11.4); camelCase arguments | Clients already namespace by server |
| 13 | Secret storage | SIGN/QA: keyring 4.2 facade, per-item entries, AES-GCM envelope over 1 KiB; core: keyring-core master key, every secret sealed | core's design; SIGN's `SecretKind` list; env names `RUSSET_ASC_KEY_P8`/`_PATH`, `RUSSET_SIGNING_P12_*` | One code path; avoids the 2,560-byte limit and the Linux binary-secret quirk |
| 14 | Account storage | core: `[accounts]` in `config.toml`; SIGN: `accounts.toml` | `accounts.toml`, daemon-managed | Machine-written with discovered fields; keeps `config.toml` hand-editable |
| 15 | TOML key style | build/SIGN/QA snake_case; core kebab-case | kebab-case everywhere | core owns file formats; matches Cargo conventions |
| 16 | `russet.toml` shape | `kind` vs `stack`, `build` vs `build_number`, `[flutter]` vs `[build.flutter]`, three stamping knobs, `bundle-id-strategy` vs policy | Merged schema §14.2 | One schema, one validator |
| 17 | Metadata stamping default | build: `sdk` including `DTXcode`/`DTXcodeBuild`; SIGN: off, two tiers; QA: opt-in | Link-time `sdk 27.0` always; default `sdk` DT facts without Xcode identity; `xcode-compat` opt-in with committed consent and a visible marker | Truthful by default; Xcode identity claims need the user's informed consent (owner Q2) |
| 18 | Free-account bundle IDs | build: team-suffix default; SIGN: undecided | `auto` = exact (paid) / team-suffix (free); xtool-prefix opt-in | Paid exact IDs are binding; free IDs must be unique across teams |
| 19 | IPA writer | device, build and SIGN each had one | One `russet-ipa` crate | Symlinks, modes and determinism done once |
| 20 | `russet-macho` | build: MIT lints; SIGN: GPL patcher | One MIT crate with the union API; lints live in russet-build and russet-sign | Shared parser; upstreamable |
| 21 | Crypto provider | core: aws-lc-rs; device: ring | ring only; aws-lc banned | apple-codesign already needs ring [V lead]; simpler Windows and cross builds |
| 22 | idevice source | device: git rev 3854a5df; QA: exact crates.io; critique 2026-10-11: unchecked `u32 - 16` in `usbmuxd/mod.rs`; review: cryptexd sends `nonce_persistence` as `persistence` | crates.io `=0.1.68` with a `[patch.crates-io]` to a reviewed Russet fork rev: upstream 3854a5d (short-packet check, `Send` listen stream) plus one Russet commit with the cryptexd persistence fix and, since review round 2, a 16 MiB cap on declared usbmuxd packet sizes (3854a5d still allocates up to about 4 GiB), with a `deny.toml` `allow-git` entry (§16.7). W4 carries it and DEV-23 files the upstream PRs; the patch goes once a release carries every fix | Reproducible supply chain; QA's mock-usbmuxd fuzzing would hit the panic; DDI installs should send Xcode's values |
| 23 | Device namespace overlaps | device: `debug.start`, `crash.*`, `symbols.fetch`, `ui.elements`, `device.register` (SIGN) | `debug.*`, `crash.*`, symbols → run/debug; `ui.elements` → `screen.elements` (`ui.*` is the daemon→client callback namespace); SIGN's `device.register` → `portal.devices.register` | Namespaces must not mean two things |
| 24 | Run session naming | IDE `run.start/stop/hotReload`; RD `debug.run/stop/relaunch/sessions/output.*`; core `app.runs` | `run.*` for every app session (run, debug, Flutter); `debug.*` for debugger-only calls | The Run button is not a debugger |
| 25 | Profiler naming | IDE `profile.*`; device/core `perf.*` | `perf.*` | Matches the device service |
| 26 | Crash naming | IDE `crashes.*`; RD `crash.*` | `crash.*` | Singular namespaces |
| 27 | Mirror naming | IDE/core `mirror.*`; device `screen.mirror.*` | `screen.screenshot`, `screen.mirror.*`, `screen.elements` | One screen namespace |
| 28 | Device reservation | QA/IDE `device.reserve`; core/device leases | `device.reserve` creates an exclusive device lease; `lease.release` ends any lease | Same mechanism, discoverable name |
| 29 | Lease release timing | core: immediate; device: 2 s grace | 2 s grace for device-bound leases; detached leases unaffected | Survives quick reconnects |
| 30 | Tunnel idle close | device 300 s; core keep-warm 10 min | `device.keep-warm` (10 min) only | One knob |
| 31 | DTO crate | device: `russet-device-proto`; core: all in `russet-protocol` | `russet-protocol` modules only | One schema and TS generator |
| 32 | QA workspace names | `russet-proto`, `russet-daemon`, `russet-package`, `russet-flutter`, `russet-mirror`, `russet-profile`, `russet-strings`, `russet-sdk`; core `russet-project` | core/area names; `russet-sdk` adopted (SDK facts, view, import); `russet-project` folded into russet-build; Flutter inside build/debug; mirror and perf inside russet-device | Fewer crates; area docs already designed module maps |
| 33 | Extension path | QA `ide/extension` | `extension/` + `ide/` | core and IDE agree; generated TS path fixed |
| 34 | Code-OSS pin | QA 1.141.0; IDE 1.135.0 | VSCodium stable 1.135.06055 / Code-OSS 1.135.0; re-pin to the newest VSCodium stable when W25 starts | VSCodium's ~50 patches are validated per tag |
| 35 | Who launches lldb-dap | IDE: `DebugAdapterExecutable`; RD: daemon hub | Daemon-owned hub (`DebugAdapterNamedPipeServer`); executable mode as dev fallback | Env applied once; survives reloads; agents can share |
| 36 | Agent debugging | IDE: reverse `ide.debug.request/stop/event`; RD: hub multiplexing | Hub for execution and inspection; reverse requests only for `ide.debug.start`, `ide.breakpoints.add/remove`, `ide.reveal` | DAP `setBreakpoints` replaces per source, so VS Code must stay the single writer when attached |
| 37 | libpython for LLDB | core: managed source-built CPython (300 MB); RD: python-build-standalone | PBS 3.12.15 `install_only_stripped` (34 MB) as `python-shared` | Verified working; 10× smaller |
| 38 | LLDB attach method | device/M1: async connect + attach with Python `script`; RD: broker pre-attach | Broker by default; `attach-mode = "legacy-async"` fallback until device check D3 passes | Python-free; breakpoints before dyld; the M1 path stays as the safety net |
| 39 | Debug socket paths | device `runtime/dbg-<lease>.sock`; RD `runtime/sessions/<sid>/gdb.sock` | RD's for the broker; `runtime/fwd-<lease>.sock` for raw forwards | The broker owns the debug socket |
| 40 | Flutter device commands | core hidden `russet __flutter-device`; build `russet flutter device …` | Public `russet flutter device <op>` (client kind `flutter-device`) | Discoverable; the same command in custom_devices.json |
| 41 | Flutter registration | build `flutter.register`; RD `flutter.devices.register` | `flutter.register {udid?, all?}` owned by build | One method |
| 42 | Panic files vs crash store | core `state/crashes/<ts>.txt`; RD `state/crashes/<bundleId>/` | Daemon panics move to `state/panics/` | Collision |
| 43 | Audit logs | SIGN `audit.log`; QA `audit.jsonl` | One `state/audit.jsonl` with `kind` | One reader for the IDE and `russet audit tail` |
| 44 | Policy files | SIGN `policy.toml`; QA `mcp-policy.toml`; core `[agents]` in `config.toml` | `provision-policy.toml` + `mcp-policy.toml`; project `[agents]` narrows only | Clear names; distinct owners |
| 45 | In-process flag | SIGN `--in-process`; core `--no-daemon` | `--no-daemon` | core owns global flags |
| 46 | Test seams | QA T1–T10; core concrete types | QA seams as traits implemented by core types; device seam = `ServiceConnector`; core's FakeDeviceBackend = `ScriptedConnector` | Replay without forking idevice |
| 47 | Linux release baseline | core glibc 2.35; QA musl tar.gz; review: the hosted `ubuntu-22.04` image is deprecated (from 2026-09-17; support ends 2027-04-17) | glibc 2.35, built in an `ubuntu:22.04` container on `ubuntu-24.04` runners | zbus/keyring/musl untested; the hosted 22.04 image is going away |
| 48 | Licence allowlist | QA lists LGPL; core requires reviewed exceptions | core's rule | Keeps copyleft decisions explicit |
| 49 | IPA compression | device deflate-1; build stored (dev) / deflate-6 (store) | build's modes | Install speed vs store size |
| 50 | Reading xtool's store | core: never; SIGN: `account import --from-xtool` | Only the explicit, user-initiated one-time import | Migration without silent coupling |
| 51 | Tap coordinates | device: screenshot pixels; IDE: points | RPC default `normalized`; MCP default `screenshot` (pixels of the last image returned to that client); `points`/`pixels` available | Agents see downscaled screenshots |
| 52 | Log vocabulary | `notice` vs `default`; `text` vs `message` | One `LogEntry {ts, source, level: debug\|info\|default\|error\|fault, pid?, tid?, process?, subsystem?, category?, message, imageUuid?, imageOffset?}` | Apple's level names |
| 53 | Wire enum casing | mixed | kebab-case values | core convention |
| 54 | IPC byte streams | core §7.8 | Deferred; no v1 consumer | The WebSocket covers the mirror |
| 55 | Destructive device tools for agents | device: user toggle; QA tiers | `app_uninstall` restricted to own and test IDs; `files.remove` and `process.signal` not exposed | Simple, safe default |
| 56 | Swift "Hot Reload" | RD: Quick Relaunch | Quick Relaunch in v1 (label is owner Q14) | Real Swift injection is research |
| 57 | DDI third-party mirror | device: opt-in via pymobiledevice3 | Opt-in only, with a consent text naming Apple files from an unlicensed mirror | K4 |
| 58 | `sdk.import` owner | core method; build importer | Core method name, implementation in `russet-sdk` (W16b since review round 1, [D 69]) | Build knows what an imported SDK must keep |
| 59 | Non-interactive refusal exit | SIGN: exit 3 | Exit 11 (`perm.confirmationUnavailable`) | core table |
| 60 | Default MCP `tier-max` | QA/IDE: 1; local models 0 | Adopted; `russet mcp install --tier-max` and the IDE can raise it per client | Conservative default; tier 2–3 still need approvals |
| 61 | `ErrorCode` type | core: one enum | Per-family constant files under `russet-core/src/error/codes/` | Avoids one merge-hot file for 20 workstreams |
| 62 | DDI source | device: the user's Xcode, location open (OQ-3); critique 2026-10-11 (blocker): no workstream extracts a DDI | (1) Default: import from the user's own Apple artifacts: `Xcode_27*.xip` (downloaded by the user from developer.apple.com with a free Apple ID; today the only Apple-hosted copy of the 27.x DDI), `Xcode.app`, `XcodeSystemResources.pkg`, `iOS_DDI.dmg`, an Apple Device Support `.dmg`, or a copy of a Mac's `/Library/Developer/DeveloperDiskImages/iOS_DDI`; keyed by the DDI's own `ProductBuildVersion`, checked against the manifest's SHA-384 digests. (2) Wave 1 mounts from a configured directory holding the Apple-origin 27A266a copy of the owner's Mac folder (W4; `device.ddi.mount {kind: dir}`); W16b's native importer lands early in Wave 2, DDI part first, and W10 makes `imported` the default. (3) Optional and deferred: a consent-gated fetch of Apple Device Support components from Apple hosts under the user's IP (W33, Q25). (4) Russet never re-hosts or caches Apple files on its own infrastructure; the third-party mirror stays opt-in (D 57), and the pymobiledevice3 cache counts as mirror content | K4. Xcode 27 ships the DDI only inside `XcodeSystemResources.pkg`; Apple's CDN and catalogs offer no standalone 27.x DDI (Pallas `XcodeDeviceSupport` "Active not found", `xcodebuild -showComponent DeviceSupport` "No updates", 2026-10-11). No DDI source is licensed for Linux or Windows hosts (§19, Q26) |
| 63 | Daemon per worktree | core/device: one daemon per OS user on a fixed socket; critique: worktree agents share it, so a lane-D build can spawn a live daemon or reuse another branch's | Per-worktree `RUSSET_HOME`; scripted backend in lane D; development builds refuse `live` without `RUSSET_LANE=A`; a host-wide live-device lock outside every `RUSSET_HOME`; development clients compare `buildId` and the backend (1107); test daemons use `TempHome`; device identity is read lazily (§3.3 item 7, §5.3) | The device rule must hold mechanically, not by convention; K2 |
| 64 | App Store and TestFlight surface | decisions.md: a publishing panel for TestFlight and the App Store; SIGN §9.7–§9.8: the API flow; critique: no RPC, CLI or acceptance test | `ship.version.plan/apply` (create or reuse the `appStoreVersion`, attach the build, export compliance), `ship.review.submit/status` (typed confirmation; never MCP), and TestFlight `groups`, `groups.create`, `testers`, `addTesters`, `removeTesters`, `distribute` (with export compliance) and `betaReview.submit`; CLI `russet ship version\|submit\|testflight …`; W23's wiremock suite asserts the exact POST/PATCH sequence; a live App Store submission only with the owner's approval (Q32) | Every decided GUI feature needs a headless method and a test |
| 65 | Detach vs background | core: RPC `detach: true` returns `{jobId}`; PLAN's exit script read `.sessionId` from `russet run --detach` | RPC `background: true`: the job survives its requester and the call returns `{jobId}`. Session lifetime is the separate `sessionLease: owned \| detached`. CLI `--detach` waits for the launch, prints `{sessionId, pid, bundleId}` and leaves a detached session; CLI `--background` maps to the RPC flag | Two meanings, two names |
| 66 | Per-call permission | core: constant `Method::PERMISSION`; critique: several rows depend on params or ownership | `Method::permission(params, conn, ownership)`, defaulting to the constant; `rpc_method!(…, permission_fn = …)`; W1 lands the dynamic rows as stubs | No per-owner workarounds; MCP listings still use the ceiling |
| 67 | MCP "session" approvals | QA: tier 2 once per MCP session; MCP 2026-07-28 is stateless | stdio: the bridge's daemon connection id; HTTP: the bearer token's client id plus `session-approval-ttl` (default 30 min, at most 8 h); never `mcp-session-id` or `clientInfo` | The binding must not be forgeable by the agent |
| 68 | Device reservation principals | IDE: every run reserves the device; critique: the IDE's own agents then get `device.reserved` | A reservation belongs to its connection plus every connection that presents its id as `initialize.sharedLease`; the IDE passes `RUSSET_LEASE` to the agents it spawns; release stays owner-only; lab and E2E runs do not share | "The agent drives, the human watches the mirror" |
| 69 | Native SDK import everywhere | core/PLAN: wrap `xtool sdk install` (Linux), Windows through WSL; critique: no native importer, so Windows depends on WSL | Russet's own importer on `russet-xar` (W16b) on every OS; no xtool and no WSL in the import path; iOS-only want-set (about 421 MiB); on Windows directory links become junctions and file links hardlinks (copy as the fallback), never NTFS symlinks, so neither Developer Mode nor admin is needed, in the short root `%LOCALAPPDATA%\Russet\s\<id8>`; `info.json` lists Linux and Windows host triples; xtool bundles are still read in place on Linux | xtool is Linux-only and its bundle lacks the DDI; Windows must work without WSL or admin |
| 70 | Rust toolchain pin | core: `channel = "1.99.0"`; critique: rustup treats that as a different toolchain from the installed `stable` and auto-installs a second copy | `channel = "stable"` plus workspace `rust-version = "1.99"`; CI pins `RUSTUP_TOOLCHAIN=1.99.0` (minimal profile; cached on Windows, whose image ships 1.98.1) | Saves 0.6–1.5 GiB on a disk-constrained box |
| 71 | JIT/TXM test coverage | critique: the rig has no TXM, so ask for an A15+ device; review (firmware, 2026-10-11): the rig **is** TXM on iOS 27.0 | M2 gates on E2E-08 **and** DC-30 on the rig (every page of 1- to 2048-page regions runs, a never-announced control page faults, `pagesTouched == pagesNotified`) plus the host test H8 (a TXM-model fake debugserver, the critique's proposal, kept). The iOS 26 TXM build and non-TXM devices are covered by traces and community reports; a second device (Q20) is optional. The critique's premise is refuted, its tests are adopted | One TXM image for every iOS 27 SoC from t8030 to t8160; a missed page crashes later, so the one-page JIT check alone proves nothing |
| 72 | Wire enums and shared wire types | area docs: externally tagged Rust enums (`DeviceState`), `LogEntry.text`/`notice`, `step_over` | Unit enums kebab-case; data-carrying enums `#[serde(tag = "kind", rename_all = "kebab-case")]` with struct variants only; externally tagged enums rejected by a registry test; the `LogEntry`, `DeviceSummary` and `JitStats` shapes of §9.5; superseded fields in §20.1 | One shape per concept; simple generated TypeScript |
| 73 | Child-process spawning | core: process-wrap with PDEATHSIG; critique: PDEATHSIG fires when the spawning thread exits | Every spawn goes through one long-lived `russet-spawner` thread; the group kill (Linux) or Job Object (Windows) stays the primary cleanup | tokio's blocking-pool threads exit after about 10 s idle and would kill children mid-build |
| 74 | Contract freeze order | PLAN v1: the F2 contract PRs (day 2 of W3–W6) compile against F1 types due on W2's day 3, while F1's `DaemonCtx` names F2 types | W1 lands every §5.3 signature as compiling stubs (F0); F1 (W2, day 2) fills core bodies; F2 (W3–W6, day 4) refines the domain contracts against F1; W7 starts at once on the gdb-remote crate and the broker and on the pipeline after F2; W8 has float but must merge before W9. Round 2: W6 is on the critical path (W5's `SigningPlan` and W7's pipeline code against its day-4 contract PR), so day 0 starts W2, W3, W5, W6 and W7 in lane D plus W4 in lane A; W8 takes the first free slot; with only four lane-D agents W7 waits, never W6. W2 merges first among W2–W7, and the CLI-level acceptance of W3–W7 is checked after W2 has merged (PLAN §2) | Acyclic: F0 → F1 → F2 → F3; no agent waits for a type someone else must invent |
| 75 | Storyboards and XIBs | build D7: avoid storyboards, with a narrow NIB compiler "phase C (M7+)" and no task; critique: stack 3 and W28's apps need one | `russet-ib` compiles storyboards and XIBs to NIBArchive: B21a launch storyboards, plus a single-scene main storyboard whose controller has an ObjC custom class (W21b, Wave 4); B21b main storyboards and XIBs (W28b, Wave 6); B21c tables, collections, split views and traits (W28c, Wave 6) [D 90]. Ported from omarchy-apple-dev's MIT tools with attribution, with WinObjC xib2nib (MIT) as a second reference. Acceptance is decoded-object-graph equality with Xcode 27 `ibtool --cocoatouch-compiler-mode toolchain` plus on-device load harnesses; byte identity is a metric. `[build] launch-storyboard = "auto"` compiles, else converts when lossless, else follows `launch-screen-fallback` | `UILaunchScreen` is lossy for React Native and flutter_native_splash; NetNewsWire cannot launch without `Main.storyboardc` |
| 76 | Apple-hardware CI template | SIGN §9.9: `macos-26` default, an automatic-signing archive with the API key, unpinned `brew install` of xtool; critique: no Xcode 27 on `macos-26`, certificates minted on ephemeral runners | Default `xcode-27` from pins; exact Xcode selection, never a beta; the archive is always unsigned; signing modes `russet` (default; no Apple credentials on GitHub), `xcode-manual` and `xcode-cloud` (Admin-role key); every nested bundle pre-signed before an Xcode export and entitlements verified after it; actions and xtool pinned by hash; no `${{ }}` in `run:`; `permissions`, `persist-credentials: false`, `timeout-minutes`; a Wave-2 smoke of the archive half (W16) | Runner and Xcode facts of 2026-10-11 (runner-images README, #14404); App Manager keys are refused for cloud signing; `-exportArchive` drops entitlements of unsigned archives |
| 77 | CI budget and runner matrix | QA: about $50–60/month in the private phase, Windows on main, `merge_group`; review: the account is on GitHub Free; decisions.md: free private-repo minutes only, macOS reference jobs on the owner's Mac | Private phase: no payment method, so jobs stop at the 2,000-minute quota and CI cannot cost money; Linux on `main` pushes and on PRs labelled `ci`; Windows build + L1 weekly and on dispatch; macOS only for owner-dispatched Apple-hardware runs; the oracle is the owner's Mac (`gha` once public, or per run with the owner's OK); no `merge_group` (it needs an organization-owned repository). Rust jobs on `windows-2025` with a cached 1.99.0 toolchain; Swift for Windows and gen_snapshot on `windows-2022`; never `*-latest`, `-large`/`-xlarge` or hosted `ubuntu-22.04`. Public phase: Windows on every PR, a weekly oracle compare on `xcode-27`. Round 2: the private-phase budget also counts `apple-release.yml`, `ide.yml` and the `windows-2022` jobs of W16b and W27b (QA §4.8); Tier-2 IDE builds run in CI only once the repository is public, otherwise locally at tier BLUE with the owner's OK (PLAN W25) | decisions.md binds the budget; included minutes are consumed at list price (a Windows minute ≈ 1.67 Linux minutes, macOS ≈ 10.3) |
| 78 | IDE title-bar checks | PLAN W25: the Tier-1 repack shows title-bar buttons; IDE: Tier 1 cannot carry the `9xx` patches, and extensions cannot contribute to the title bar | The title-bar toolbar is checked only on the Tier-2 CI build (IDE-06, L6b, E2E-19); Tier 1 checks the fallbacks (`editor/title/run`, `view/title`, status bar); DC-23 uses the CI-built package | The criterion could never pass as written |
| 79 | Windows workstream shape | PLAN v1: one XL card W27 for all of Wave 5, and no way for lane A to reach the PC | Three cards: W27a (core, IPC, device, debug), W27b (build plane, SDK and DDI import validation, shims, long paths), W27c (packaging, IDE, anisette). Lane A reaches the PC over OpenSSH with key auth in announced windows, and the owner moves the phone (Q31) | Reviewable units; the PC and the phone form one scarce lane |
| 80 | XCTest on device; MAUI scope | PLAN v1: `russet test` only in Wave 6 (W28); MAUI reduced to a report without an owner question | SwiftPM XCTest on device in W32 (Wave 4); XCUITest and Xcode projects stay in W28; MAUI is study-only (the B20 go/no-go report) for the first build (Q30) | "Test" is part of the product goal, and stack 1 can test early |
| 81 | Local build resources | PLAN v1: two cargo target slots capped only by a nightly sweep; QA: one shared target directory; brief: no download over 500 MB | One target slot at tier AMBER, two only from GREEN; `CARGO_INCREMENTAL=0` locally too; cargo runs through `cargo xtask` with a disk-guard pre-check; any download over 500 MB needs the owner's explicit OK, whatever the tier (K10) | Two slots plus Swift scratch can push AMBER into RED |
| 82 | Wave-1 DDI gate and lane A's home | PLAN v1.1: the exit script asserts `productBuild == "27A266a"` under `set -e`, while gate item 7 may be "blocked on Q33"; W9's fresh worktree home has no `[device.ddi] dir`; §14.3 called `dir` a Restore directory, but the Mac copy is the `iOS_DDI` root (`Restore/` + `version.plist`) and idevice's `Cryptex1Assets::load` reads `<restore>/BuildManifest.plist` | Gate items 1–6 must pass; item 7 passes or is recorded as blocked on Q33. The DDI-provenance assertion lives in DC-28's own script (`tests/lab/dc28-ddi.sh`), not in `demo/m1-russet.sh`. Lane A uses one persistent `RUSSET_HOME` (`~/russet-lab/home-A`), and the exit script also sets `device.ddi.dir` when the copy exists. `dir` is the DDI root; Russet loads `<dir>/Restore` and also accepts a directory that directly holds `BuildManifest.plist`. The Mac copy is a lead pre-task in an owner-approved lane-B window that gates DC-28, not W4's start | The script must pass while Q33 is open; the device lane is serial, so one home loses nothing and keeps state across cards; the Mac's folder layout was checked read-only on 2026-10-11 (`Restore/`, `version.plist`) |
| 83 | Development isolation, round 2 | D 63 left shared state: one keyring entry for every home and test; the live-device lock test needs the real lock and `RUSSET_LANE=A` in lane D; `buildId` does not change between rebuilds of an uncommitted tree; doctor's `usbmux.available` connects to `/run/usbmuxd`; the tripwire cannot see a direct socket open and needs W4 | Keyring entry `master-key-<sha256(RUSSET_HOME)[..8]>`; tests use the `memory` or `passphrase` backends; a debug-only `RUSSET_LIVE_DEVICE_LOCK` override plus a test-local fake usbmuxd for the lock tests; development clients also compare the daemon's executable identity (`executableId`) with their own `current_exe()` (1107 → idle restart); `usbmux.available` connects only for the `live` backend and otherwise stats the socket and reports `skip`; W2 asserts a `LiveConnector` construction counter of 0; the live-device lock is taken at daemon start and `DeviceManager` starts only on the first device call that passes it, so the lock tests need no W4 code; the tripwire test stays `#[ignore]` until W4 merges and W9 enables it (§3.3 item 7) | Each guard of D 63 must hold without touching shared state or the phone |
| 84 | Debug batch mode and session lifetime | PLAN v1.1: the exit script expects a `running` session after `russet debug --batch` exits, but `debug.prepare` defaults to `sessionLease: owned` and LLDB's exit triggers `onDebuggerLost: kill`; LLDB batch commands are synchronous, so a final `process continue` blocks; the breakpoint was the `@State var counter = 42` declaration, which is not a statement in `body`, and `frame variable counter` cannot read a SwiftUI `@State` property; no wire values for `SessionSummary`/`SessionState` | `russet debug --detach` (`sessionLease: detached`; the script ends with `process detach`); `--batch` continues to the first stop, runs the commands, then detaches or kills; RussetProbe's `ContentView.body` has `let probeValue = counter  // RUSSET_PROBE_BREAK` followed by a statement that uses it, and the scripts break on the line after the marker and read `frame variable probeValue`; `SessionSummary` and `SessionState` wire values in §9.5 | Verified on the host: a batch `process continue` returns only when the process stops or exits, while `process detach` returns without waiting and the process keeps running [V lead 2026-10-11]; a plain local is readable with `frame variable` without expression evaluation, which is unproven off-Mac until D7 |
| 85 | `secrets` calls from a CLI without a TTY | §7.5: the `secrets` class needs an interactive CLI; W6's acceptance and the exit script run `russet account import --from-xtool --yes` in agent shells, which have no TTY | Local-file imports (`account.importXtool`, `account.addAscKey`, `secrets.set {fromFile}`) accept a non-interactive CLI with a plain confirmation that `--yes` satisfies; passwords, 2FA and approval grants stay TTY-only (§7.5) | An agent that can run the CLI can read those files anyway; secrets typed by a human, and approvals, must still come from a human |
| 86 | F0 completeness and seam ownership | PLAN v1.1: `DaemonCtx` names an undefined `AccountService` and untyped fields; the code families are not pre-filled; stub crates without key dependencies make the `aws-lc-sys` check vacuous; `xtask/src/main.rs`, `Cargo.lock`, `xtask/src/lab.rs`, testkit's `lib.rs` and `apple-release.yml` have no owner, or several | §5.3 defines `AccountService` and `ConfigService` and types every `DaemonCtx` field; W1 lands every §13.3 constant, every stub crate's key dependencies and every module file whose owner differs from its crate root's; `xtask/src/main.rs` and `xtask/Cargo.toml` are seam files with every subcommand pre-registered; `Cargo.lock` is generated (regenerate, never hand-merge); `lab.rs` belongs to W9, `apple-release.yml` to W23 (PLAN §0, §3) | Parallel cards must never edit each other's files to compile |
| 87 | Pins in Wave 1 | §14.7 forbids hand-typed URLs and puts `xtask pins` and the signing key in W26 (Wave 4), but W3 must install a pinned, verified PBS and darwin-tools in Wave 1 and test minisign rejection | W3 owns `xtask/src/pins.rs` in Wave 1 for those two components, from GitHub release metadata plus the SHA-256 of the downloaded file, signed with a development minisign key kept outside the repository; W26 extends it and adds the release key (§14.7) | Keeps the no-hand-typed-URL rule without waiting three waves |
| 88 | Synthetic Apple-format fixtures | §19's guard rejects xar/koly/pbzx magic and `.dmg`/`.xip` names everywhere, yet W16b commits synthetic container fixtures and fuzz seeds, and W4's test uses a Restore directory with `.dmg` names | Build them at test time from Rust builders; where that is impractical (UDIF, HFS+, fuzz seeds), commit generated files only under the guard's provenance allowlist (§19) | The guard stays strict; every committed binary is reproducible from open tools |
| 89 | DDI mount attribution | §9.5 attributed a mounted DDI by cryptex version and product build, which cannot tell the Wave-1 directory copy from an import of the same Xcode; `device.prepare` is a no-op while a DDI is mounted, so DC-29 could not show "mounted from the import" | Every mount Russet performs writes a `MountRecord` (`state/devices/<udid>/ddi.json`), and status reports the recorded source. DC-29 needs a Q33-approved reboot or an owner-approved `ddi mount --imported --force`; otherwise it records "import verified, mount not re-exercised", which Wave-2 gate item 9 accepts explicitly | Evidence must say which bytes were mounted, and must not require a device change the owner has not approved |
| 90 | Storyboard card order and B21a scope | PLAN v1.1: W28's DC-24 needed W28b's B21b, while W28b's NetNewsWire acceptance needed W28's `russet-swb`; W21b's corpus and DC-32 include Flutter's `Main.storyboard` (custom class `FlutterViewController`), which build §5.4 assigned to B21b | W28b (B21b; DC-33 without NetNewsWire) → W28 (DC-24) → W28c (B21c and NetNewsWire, DC-35). B21a also compiles a single-scene main storyboard whose controller has an ObjC custom class (no module mangling) | No card waits on a later card; the acceptance of each card is reachable with its own scope |
| 91 | Public-repository hygiene | Review round 2: decisions.md (lines 23–43) and the committed round-1 critique describe the private track's approach; the main repository goes public at G1 | Owner question Q34 (move those decision details to the private repository's own log, leaving one neutral line); the two strings in the round-1 critique are redacted; `cargo xtask guard public-terms` with hashed terms (§19); a going-public checklist (PLAN §5.9) that also covers the git history, whose earlier commits contain the same text | The approach must stay private until its legal review; deleting a file does not remove it from history |

### 20.1 Superseded area-doc fields

The area docs keep their original text for its rationale; where they disagree with this file, the right-hand column is binding. Affected sections carry a banner that points here.

| Area doc and place | Superseded | Binding replacement |
|---|---|---|
| IDE §1.3, §5.4; QA §5.1, §11.4 WS-01 | handshake `rpc.hello` | `initialize` (§7.4) |
| IDE §5.4 | `events.subscribe` and the single `event` envelope | per-namespace subscriptions (`device.watch`, `job.subscribe`, `logs.subscribe`, …) and named notifications (§8, §9.1) [D 2] |
| IDE §5.4, §7; QA §8.3 | socket `daemon.sock` | `russetd.sock`, or `$RUSSET_HOME/runtime/russetd.sock` (§7.1) [D 9, D 63] |
| IDE §5.5 | `mirror.*`, `profile.*`, `crashes.*`, `approvals.*`, `device.release`, `project.open`, `debug.cleanup`, `ui.elements`, `run.start {runId}` | `screen.mirror.*`, `perf.*`, `crash.*`, `approval.*`, `lease.release`, `project.info`, the DAP hub, `screen.elements`, `run.start {sessionId}` (§8) [D 4, D 24–D 28] |
| IDE §5.7 | `DebugAdapterExecutable` with an async attach recipe | the daemon's DAP hub (`DebugAdapterNamedPipeServer`) and the broker (§10.2, §17.2) [D 35, D 38] |
| IDE §6.3 `input_tap` | `space` default `points` | default `screenshot` [D 51] |
| IDE §6.3 `debug_control` | `step_over`, `step_into`, `step_out` | kebab-case `step-over`, `step-into`, `step-out` [D 53] |
| IDE §6.3 `plan_apply` | tier "2 or 3 (from the plan)" | tier 2 (§11.4) |
| IDE §6.3–§6.4; QA §8.4, E2E-13 | `RUS-MCP-0403` with `approval_request_id`, `args_digest`, `approve_with`, `expires_at` | `perm.approvalRequired` (1204) with `data.details {approvalId, tool?, tier, summary, argsDigest, approveWith, expiresAt}` (§11.3) [D 7] |
| QA §8.4, IDE §6.4 | tier-2 approval "once per MCP session" | the session definition of §11.3 [D 67] |
| RD §18; device §8 `ios_logs.min_level` | `LogEntry.text`; level `notice`; `LogSource` without `syslog` | `message`; level `default`; `LogSource` includes `syslog` (§9.5) [D 52, D 72] |
| RD §18 | `debug.run`, `debug.stop`, `debug.relaunch`, `debug.sessions`, `debug.output.*`, `flutter.reload`, `flutter.devices.register`; notifications `debug.state/output/exited/jit` | `run.start`, `run.stop`, `run.relaunch`, `run.list`, `run.output.query`/`run.subscribe`, `run.hotReload`, `flutter.register`; `run.state/output/exited/jit` [D 24, D 41] |
| device §5.3 | `DeviceState` as an externally tagged enum on the wire; `DdiSource::{RestoreDir, XcodeApp}` | the flat `DeviceSummary.state` (kebab-case) plus `readiness` (§9.5); `DdiSource::{Imported, Dir, Pmd3Cache, Pmd3AutoMount}` (§5.3) [D 62, D 72] |
| device §7 | `device.ddi.install`, `debug.start`, `ui.elements`, `symbols.fetch`, `device.added/removed`, `app.console`, `app.crashed` | `device.ddi.mount`, `debug.prepare`, `screen.elements`, `debug.symbols.fetch`, `device.changed`, `app.output`, `app.exited {crashId}` [D 2, D 23] |
| device §8 | `ios_*` tool names | the unprefixed catalog of §11.4 [D 12] |
| device §9 | error codes 1100–1191 | 3000–3299 (§13.3 remap) [D 7] |
| SIGN §10 | `{jobId}` returned at once; `job.progress`/`job.done`; `approval.pending`; HMAC approval tokens; `cert.revoke`; `device.register`; `E_*` codes | requests stay open unless `background: true` (§7.6); `job.updated`/`job.events`; `approval.list/get`; approval ids (§11.3); `portal.certificates.revoke`; `portal.devices.register`; numeric codes (§13) [D 2, D 3, D 4, D 7, D 23] |
| SIGN §12 | its own exit-code list | §13.4 only [D 8] |
| SIGN §9.9 (draft v1) | `macos-26` default, automatic-signing archive with `-allowProvisioningUpdates`, `brew install` of xtool | the rewritten §9.9 and §17.5 [D 76] |
| QA §2 T8 | `RUS-<AREA>-<NNNN>` codes | numeric `error.code` plus dotted `russetCode` (§13) [D 7] |
| QA §11.2 | crate names `russet-proto`, `russet-daemon`, `russet-package`, `russet-flutter`, `russet-mirror`, `russet-profile`, `russet-strings` | §5.1 [D 31, D 32] |
| core §4.1 | `rust-toolchain.toml` `channel = "1.99.0"` | `channel = "stable"` plus `rust-version = "1.99"` [D 70] |
| core §7.10, §10.6; QA WS-04 | `sdk.import` wraps `xtool sdk install` | the native importer (W16b, §16.3) [D 69] |
| build §5.4 (v1) | "phase C (M7+)" narrow NIB compiler | B21a–c (W21b, W28b, W28c) [D 75, D 90] |
| build §9.1 | `BuildEvent` enum and `EventSink = mpsc::Sender<BuildEvent>`; `BuildOutput.build_id: BuildId` | `JobEvent` kinds (§9.2) through `russet_core::progress::EventSink`; `build_id: JobId` (§5.3) [D 2] |
| build §9.2 | snake_case `russet.toml` keys (`bundle_id`, `launch_screen_fallback`, …) | kebab-case keys of §14.2 [D 15, D 16] |
| build §9.4, §9.6; B02, B08 | `build.cancel`, `build.get`, `build.list`, `build.log`; notification `build.event`; `inspect.app`; params `projectDir`, `profile`, `product`; result `{buildId, logPath}`; logs at `.russet/logs/<buildId>.jsonl` | `job.cancel` (or `$/cancelRequest`), `job.get`, `job.list`, `job.log`; `job.subscribe` → `job.updated`/`job.events`; `build.inspect`; params `project`, `configuration`, `target`; `build.start` returns `BuildOutput` and stays open unless `background: true`; logs at `.russet/logs/<jobId>.jsonl` (§8.3, §7.6, §14.8) [D 2, D 3, D 65] |
| build §9.5; B08 | MCP tools `russet_build`, `russet_build_status`, `russet_build_log`, `russet_build_settings`, `russet_inspect_app`, `russet_clean`, `russet_app_icon`; snake_case arguments and results (`project_dir`, `app_path`) | `build`, `job_status`, `job_log`, `build_settings`, `app_inspect`, `build_clean`, `assets_app_icon`; camelCase arguments and results (§11.4) [D 12] |
| build §9.7; B02 | `E_*` codes, e.g. `E_PROJECT_NOT_FOUND`, `E_BUILD_IN_PROGRESS`, `E_MANIFEST_INVALID` | numeric codes with dotted names (§13.3), e.g. `config.projectNotFound` (1301), `build.inProgress` (4001), `config.invalid` (1300) [D 7] |
| core §5.1 | `pub enum ErrorCode`; `Method` with only `const PERMISSION` | `ErrorCode` as a struct with per-family constant files [D 61]; `Method::permission(params, conn, ownership)` defaulting to `PERMISSION`, plus `CONFIRM` and `rpc_method!(…, permission_fn = …)` (§5.3) [D 66] |
| core §5.8, §6.4, §7.6, §7.10, §19, §20 (CORE-06) | the RPC flag `detach: true` (returns `{jobId}`; `JobSpec.detach`; `toolchain.install {detach?}`; `forward.open {detach?}`; the "detach" integration test) | the RPC flag `background: true` (`JobSpec.background`); session lifetime is `sessionLease: owned \| detached`; forwards end with their lease (§6.2, §6.3, §7.6) [D 65] |
| core §5.8 | `DaemonCtx` with `sessions`, `streams` and `DeviceManagerHandle` | the typed `DaemonCtx` of §5.3 [D 86] |
| core §6.5, §7.7, §7.10 (reserved namespaces) | `debug.sessions`, `app.runs`, `job.output`, `logs.batch`, `stream.close`, `mirror.start`, byte streams | `run.list`, `job.events`, `logs.entries`, `screen.mirror.*` over the WebSocket (§8, §9.1, §10.1) [D 2, D 10, D 24, D 54] |
| core §9.6 | keyring entry `Entry::new("dev.russet", "master-key")` for every home | `master-key-<sha256(RUSSET_HOME)[..8]>` when `RUSSET_HOME` is set (§3.3 item 7) [D 83] |
| core §11 `usbmux.available`; CORE-12 | connects to `/run/usbmuxd` whatever the device backend | connects only for the `live` backend; otherwise a stat-only check that reports `skip` [D 83] |
| RD §17.2 | `SessionState` with `Detached` as a state | the `SessionState` wire values and `SessionSummary.debugger`/`lease` of §9.5 [D 84] |
| device §6.3 | `Dir` as "any Restore-style dir"; source attribution by cryptex version and product build | `Dir` is the DDI root (`<dir>/Restore` is loaded); attribution by `MountRecord` (§5.3, §9.5) [D 82, D 89] |
| QA §3.9.3, E2E-04 | the breakpoint on `@State var counter = 42`; `counter == 42`; "after `continue`" | the `RUSSET_PROBE_BREAK` marker line in `ContentView.body`; `probeValue == 42`; after the batch detaches (PLAN §4.10) [D 84] |

---

## 21. Open questions for the owner

Each question has a **default** that the plan uses until you answer, and the workstream it blocks. Questions that decisions.md has answered keep their row, marked **Answered**, so the record stays in one place.

| # | Question | Default used | Blocks |
|---|---|---|---|
| Q1 | Approve live account changes for exact bundle IDs? **Answered 2026-10-11** (decisions.md): Russet may register **one** bundle ID (for example `com.evilcomputer12.russet.hello`) and create one development profile, reusing the existing development certificate (valid until 2027-10-10); nothing is revoked or deleted | Wave 1 uses the existing `XTL-…` profile with zero changes; W12 registers exactly that one ID (`com.evilcomputer12.russet.hello` unless you name another) and maps RussetProbe onto it through your `russet.local.toml` | W12 acceptance (Wave 2) |
| Q2 | Metadata default: keep `sdk` (no `DTXcode`/`DTXcodeBuild`), or default to `xcode-compat` for App Store builds? Visible `RussetToolchain` marker on? | `sdk`; marker only with `xcode-compat` | W23 |
| Q3 | App Store submissions: default to Apple-hardware mode, with Linux-native for TestFlight iterations? | Both available; Linux-native default | W23 |
| Q4 | A throwaway Apple ID for free-account tests (GSA, anisette, 7-day profiles)? | Free mode built, live tests skipped | W22 |
| Q5 | Distribution signing. The Linux-native path needs a local Apple Distribution identity. The Apple-hardware path can reuse it with no Apple credentials on GitHub (`russet` mode), send it to GitHub secrets for Xcode's export (`xcode-manual`), or use cloud signing (`xcode-cloud`), which needs an **Admin-role** team key stored on GitHub: App Manager keys are refused ("Cloud signing permission error"), and team keys carry only a role. Do we create the identity through `POST /v1/certificates` with the App Manager key [A: omarchy evidence], or export one of the team's 2 Distribution certificates from the Mac? | **No certificate is created until you approve it** (decisions.md allows one bundle ID and one development profile only, and PLAN risk P7 keeps zero-mutation defaults): W23's live steps wait for your answer in DC-21, and then create one Apple Distribution certificate through the API, or import your exported one. Apple-hardware mode `russet`; no Admin key | W23 (DC-21) |
| Q6 | GitHub plan and going public. The account is on GitHub **Free**: branch protection and rulesets on the private repository answer 403 ("Upgrade to GitHub Pro or make this repository public"). decisions.md already limits private CI to the free included minutes. Make the repository public at G1 (standard runners free, 4-core/16 GB Linux and Windows runners, attestations, SignPath)? Until then, upgrade to Pro ($4/month: required checks, 3,000 instead of 2,000 minutes, 1 GB of artifacts) or stay on Free? | Public at G1; stay on Free with no payment method, so jobs stop at the quota and CI cannot cost money; the private-phase matrix of QA §4.2 (about 1,740 of the 2,000 minutes in Waves 1–2, and about 1,820 in a Wave-4 month if the repository is still private, with Tier-2 IDE builds kept out of CI, QA §4.8); the lead's merge step replaces required checks [D 77]. Going public follows the checklist of PLAN §5.9, including Q34 | W8, W25, W26 |
| Q7 | Self-hosted lab runner on the desktop, or manual `cargo xtask lab` runs by the device-lane agent? **Answered 2026-10-11** (decisions.md): no always-on self-hosted runner; device suites run on demand, triggered by an agent in the working session, and device access stays serialized | Manual runs by lane A, under the live-device lock (§3.3 item 7) | W9 onward |
| Q8 | Windows code signing (SignPath Foundation vs an OV certificate) and MSI tooling (WiX v3.14 vs v7 with OSMF EULA vs msitools `wixl`)? | Unsigned; WiX v3.14 | W25, W27c |
| Q9 | apple-codesign compiles Apple's public CA certificates into the binary: accept (listed in NOTICE), or patch to fetch at runtime with pinned hashes? | Accept and list | W26 legal gate |
| Q10 | Licence of `russet-asc`: GPL-3.0-or-later, or MIT/Apache-2.0 for reuse? | GPL-3.0-or-later | — |
| Q11 | Reverse-DNS app id (`io.github.evilcomputer12.Russet`?), Open VSX namespace `russet`, icon designer and palette | Placeholders | W25 |
| Q12 | Update checks: `notify` (weekly anonymous GET, CLI only) or `off`? | `notify` for standalone public releases; off while private | W26 |
| Q13 | Device symbols: allow an opt-in multi-GB download, with a 20 GiB cache quota? | Opt-in, 20 GiB | W11 |
| Q14 | Swift "Hot Reload" button: label it "Quick Relaunch", or hide it? | "Hot Reload" with a "Quick Relaunch (Swift)" tooltip | W14 |
| Q15 | May agents detach debug sessions, or only kill? | Kill only; Flutter sessions are never detached | W11 |
| Q16 | Cleanup. Status on 2026-10-11: `~/orchard-lab/build` is gone (re-checked with `ls`), and xtool's credential files are 0600 (decisions.md). Remaining:<br>• delete `~/.local/opt/python-3.12` (297 MB): uv's python-build-standalone 3.12.15 at `~/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu` replaces it (verified with Swift LLDB), once PLAN rule 8 (done) and `demo/m1-baseline.sh`'s `PY312` default point there;<br>• `xtool auth logout` after W12.<br>Your relayed message ("delete stuff we don't need if they are setup") is taken as the go-ahead for these two | The lead re-points the demo script, then deletes `~/.local/opt/python-3.12`; nothing else | — |
| Q17 | Keep the Mac awake or reachable during oracle windows? It timed out over SSH in several sessions, and it now also hosts the private virtual-iPhone track's VM, so oracle windows must not overlap that VM's use | Oracle runs wait for an owner-approved Mac window. The GitHub `xcode-27` backend runs only once the repository is public, or per run with your OK within the included minutes (decisions.md) | W16, W21, W21b, W28b |
| Q18 | Host the `gen_snapshot` CI in a separate public repository? | Separate public repo | W24 |
| Q19 | CocoaPods route: the user's Ruby + CocoaPods, or a portable Ruby? | User's | W28 |
| Q20 | A second test device? **Answered 2026-10-11** (decisions.md): iPhone 12 (A14, iOS 27) only. Corrected rationale: a second device is **not** needed for TXM coverage, because the rig runs TXM on iOS 27.0 (§17.3, [D 71]). Still useful, in priority order: an A15+ phone kept on iOS 26.x (the older TXM build), then a device on an iOS 27.x beta | iPhone 12 only; recorded traces and community reports for the rest | test matrix |
| Q21 | Legal: may normalized semantic dumps of Apple-tool outputs be published in `corpus/`? | Yes for normalized JSON only; raw output stays local | W16 |
| Q22 | Wi-Fi testing: may the device lane toggle `EnableWifiDebugging` on the phone? | Not toggled | W30 |
| Q23 | Copilot inside Russet IDE: Copilot CLI over ACP (recommended), or try VS Code's built-in chat? | Copilot CLI over ACP | W20 |
| Q24 | Trademark clearance of the name "Russet" (classes 9 and 42) before 1.0 | Keep the name; no registration filed; a clearance search before 1.0 | 1.0 |
| Q25 | DDI policy [D 62]: approve importing DDIs from your own Apple artifacts as the default, and later a consent-gated fetch of Apple "Device Support" components from Apple hosts under the user's IP (anonymous `devimages-cdn.apple.com`, and Xcode's "virtual audience" cookie on `download.developer.apple.com` for entries Apple marks `authentication: virtual`)? | Import only; the fetch (W33) is not implemented | W33 (deferred) |
| Q26 | Legal: how to read Xcode 27's licence (EA2002) and the Device Support licence (EA1879) for SDK and DDI use on Linux and Windows hosts, and the public wording | "Apple-origin, user-obtained, never redistributed"; no claim that the use is licensed | W26 legal gate; going public |
| Q27 | Will you download `Xcode_27.0.xip` (about 3 GB, with your Apple ID; above the 500 MB rule) and keep it until W16b's Linux import test and W27b's Windows import pass? None exists under `~` or on the Mac | W16b tests with the 146 MB `XcodeSystemResources.pkg` and the 33.6 MB `iOS_DDI` folder copied read-only from the Mac; the env-gated full-`.xip` test is skipped until the file exists | W16b, W27b (DC-31) |
| Q28 | decisions.md "Test devices" says A15+/TXM paths are validated only against traces. The firmware shows the iPhone 12 runs TXM on iOS 27, the same TXM image as A15+. May the lead correct that entry to: "iPhone 12 (A14) only. On iOS 27 it runs Apple's TXM, the same build as A15+, so TXM JIT behaviour is tested on it directly. The iOS 26 TXM build (A15+ phones still on 26.x) and non-TXM devices (iOS 26 on A12–A14/M1; A12Z iPads on 27) are covered by recorded traces and community reports until another device is available."? | The plan uses the corrected premise (DC-30 on the rig); decisions.md stays unchanged until you agree | W18, test matrix |
| Q29 | Accounts for the agents DC-18 covers: Gemini CLI, OpenAI (Codex CLI) and GitHub Copilot CLI; Cline and Continue need a model provider; OpenHands needs Docker and a model. Which will you provide? | Claude Code and a local model (OpenCode on llama-server) must pass; agents without an account are reported `skipped (no account)` with the reason | W20 (DC-18) |
| Q30 | .NET MAUI: study-only (the B20 go/no-go report) for the first build? | Study-only [D 80] | W29 |
| Q31 | Windows PC access for lane A: enable the OpenSSH server on the PC with key-only authentication for the agent's normal (non-admin) account, and agree hand-over windows in which you move the iPhone between the Linux box and the PC | Lane A works over SSH in windows announced in `docs/lab/queue.md`; you move the phone at the start and end of each window; Linux device work pauses meanwhile [D 79] | W27a–c |
| Q32 | May W23 submit a real build for App Store review once (the canary app, withdrawn afterwards) to prove the submission path? | No live submission: the exact wiremock sequence and `russet ship submit --dry-run` only; the live run stays a recorded deferral [D 64] | W23 |
| Q33 | During W9, approve one phone reboot and one DDI replacement on the rig, mounting the Apple-origin 27A266a copy in place of the mirror's 27A5228h, for DC-28? Also approve the lead's read-only copy of the Mac's `/Library/Developer/DeveloperDiskImages/iOS_DDI` (33.6 MB) in a Mac window you choose, and later, for DC-29, a second reboot or one `russet device ddi mount --imported --force` | W9 asks in the lab queue. Until you approve, Wave-1 gate item 7 is recorded as blocked on the owner, never as passed, and DC-28 becomes the first lane-A task of Wave 2; without the DC-29 approval, DC-29 records "import verified, mount not re-exercised" [D 82, D 89] | Wave-1 gate (DC-28), DC-29 |
| Q34 | Public-repository hygiene [D 91]. This repository goes public at G1, but decisions.md's "Simulator and virtual iPhone" entry (2026-10-11) describes the private track's approach in detail, and the commits that added it (04e657f, 21d52f5) carry the same text and commit subjects. May the lead (1) move that entry's details to the private repository's own decision log, leaving one neutral line here ("Virtual iPhone: a separate private track; it never blocks this repository"), and (2) publish this repository from a fresh, squashed history (or a filtered one) instead of the current history? Also decide whether `docs/research/macos-on-x86-feasibility-2026-10.md` (a research report on running macOS on this PC, outside this product's scope; see its legal risk R15) stays in the public repository | decisions.md stays unchanged until you answer; `cargo xtask guard public-terms` allowlists it and the research report meanwhile; going public waits for this answer (PLAN §5.9) | Going public (Q6) |

---

## 22. Sources

**Area designs and project files** (the evidence behind [V] markers):
- `docs/design/{core,device,build,signing-accounts-publishing,run-debug,ide,qa-delivery}.md`
- `docs/verified-facts.md`, `docs/decisions.md`, `demo/m1-baseline.sh`
- `docs/research/landscape-2026-10.md`

**Lead checks for this file (2026-10-11):**
- `~/.cargo/registry/src/*/apple-codesign-0.29.0/Cargo.toml`: `[dependencies.ring] version = "0.17.8"` is not optional; `[features] default = ["notarize"]`; reqwest 0.12 with `rustls-tls-native-roots`.
- `~/.cargo/registry/src/*/reqwest-0.13.5/Cargo.toml`: features `rustls` (aws-lc) and `rustls-no-provider`.
- `du -sh ~/.swiftpm/swift-sdks/darwin.artifactbundle/*`: `Xcode.app` 3.7 G, `Developer` 3.0 G.
- `~/orchard-lab/Hello/xtool/Hello.app/embedded.mobileprovision` (shape only): app id `<TEAM>.XTL-<prefix>.com.evilcomputer12.orchard.hello`, 4 devices, `get-task-allow` true, 1 certificate, expires 2027-10-10.
- `df -h /home`: 98 G free.

**Review round 1 fact-finding (2026-10-11; no device commands; the Mac read-only):**
- DDI: `ls`/`pkgutil --check-signature`/`hdiutil imageinfo`/`plutil -p` on the Mac's Xcode 27.0, `/Library/Developer/CoreDevice/CandidateDDIs` and `/Library/Developer/DeveloperDiskImages`; `xcrun devicectl list preferredDDI`; `xcodebuild -showComponent DeviceSupport -json`; Pallas POSTs to `https://gdmf.apple.com/v2/assets`; Apple's downloadables index `https://devimages-cdn.apple.com/downloads/xcode/simulators/index2.dvtdownloadableindex`; pymobiledevice3 11.26 and `developer_disk_image` 0.3.0 sources; idevice 0.1.68 `src/services/cryptexd.rs`; Xcode's `LicenseInfo.plist` (EA2002) and the Device Support `License.pdf` (EA1879).
- TXM: `ipsw extract --remote` (ipsw 3.1.736) of the iPhone13,2 BuildManifests and kernelcaches for 27.0 (24A437), 27.0.1 (24A446) and 26.6.2 (23G90), IPSW URLs from `https://api.ipsw.me/v4/device/iPhone13,2?type=ipsw`; every iPhone/iPad manifest of 27.0 and 26.6.2; xnu-12377.121.6 (`osfmk/vm/vm_map.c`, `bsd/kern/code_signing/txm.c`); llvm debugserver `MachVMRegion.cpp`/`MachVMMemory.cpp`; Dart at Flutter 3.47.7's revision (`runtime/vm/virtual_memory_posix.cc`); flutter/flutter#190307, #193058, #175962.
- CI: https://github.com/actions/runner-images (README, `xcode-27-arm64-Readme.md`, `macos-26-arm64-Readme.md`, #14404, #14167, #14748, #14856); GitHub docs on runners, runner pricing, billing and limits; the 2026-07-16 changelog "Xcode 27 runner image now in public preview"; `gh api repos/evilcomputer12/russet/branches/main/protection` (403); Apple's roles matrix and API-key docs; read-only `xcodebuild -help` on the Mac.
- SDK import and NIB: saagarjha/unxip (facts only), xtool 1.21.0 `SDKBuilder.swift` (MIT), apple-platform-rs `apple-xar`, crates.io (`rsa`, `liblzma`, `lzma-rust2`, `junction`, `udif`, `hfsplus`), Swift 6.4 typecheck timings, compiled storyboards of Apple-built apps on the Mac, joshuaswarren/omarchy-apple-dev, microsoft/WinObjC `xib2nib`, NetNewsWire's iOS storyboards.

**Review round 2 checks (2026-10-11; no device commands; the Mac read-only):**
- `ssh mac ls -la /Library/Developer/DeveloperDiskImages/iOS_DDI{,/Restore}`: the root holds `Restore/` and `version.plist`; `Restore/` holds `022-20190-411.dmg`, `022-20231-411.dmg`, `BuildManifest.plist`, `Restore.plist` and `Firmware/` [D 82].
- `~/russet-lab/idevice` at 3854a5d: `idevice/src/services/cryptexd/assets.rs` (`Cryptex1Assets::load(restore_dir)` reads `restore_dir.join("BuildManifest.plist")`); `idevice/src/usbmuxd/mod.rs:467-474` (sizes below 17 rejected, larger sizes allocated unchecked) [D 22, D 82].
- Swift 6.4's LLDB 21.0.0 on the dev box, host processes only: `lldb -b -o 'process launch --stop-at-entry' -o 'process continue' -- /usr/bin/sleep 4` took 6.2 s (it returned only after the exit); the same with `process detach` returned after 7.2 s while `sleep 20` kept running [D 84].
- The iPhone13,2 27.0 (24A437) `BuildManifest.plist` (re-read from the earlier `ipsw extract --remote` output): identities 0–3 (Customer Erase/Upgrade, d53gap and d53pap) list `Firmware/txm.iphoneos.release.im4p`; identities 4–5 (Research Customer Erase/Upgrade, d53gap) list `Firmware/txm.iphoneos.research.im4p`; all six list `Firmware/sptm.t8101.release.im4p`.

**External specifications and projects:**

| Topic | Source |
|---|---|
| JSON-RPC 2.0 | https://www.jsonrpc.org/specification |
| LSP base protocol (Content-Length framing, `$/progress`, `$/cancelRequest`) | https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/ |
| vscode-jsonrpc | https://www.npmjs.com/package/vscode-jsonrpc |
| MCP specification (2026-07-28, 2025-11-25) | https://modelcontextprotocol.io/specification |
| rmcp | https://github.com/modelcontextprotocol/rust-sdk |
| Agent Client Protocol | https://agentclientprotocol.com |
| idevice / jktcp | https://github.com/jkcoxson/idevice |
| pymobiledevice3 | https://github.com/doronz88/pymobiledevice3 |
| apple-codesign (apple-platform-rs) | https://github.com/indygreg/apple-platform-rs |
| xtool, darwin-tools-linux-llvm | https://github.com/xtool-org/xtool, https://github.com/xtool-org/darwin-tools-linux-llvm |
| swift-driver PR #2158 (`-isysroot` at link) | https://github.com/swiftlang/swift-driver/pull/2158 |
| Swift Build | https://github.com/swiftlang/swift-build |
| TN3187, UIScene life cycle | https://developer.apple.com/documentation/technotes/tn3187-migrating-to-the-uikit-scene-based-life-cycle |
| App Store Connect API | https://developer.apple.com/documentation/appstoreconnectapi |
| python-build-standalone | https://github.com/astral-sh/python-build-standalone |
| keyring-core | https://crates.io/crates/keyring-core |
| Windows `CREDENTIALW` blob limit | https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw |
| XDG Base Directory | https://specifications.freedesktop.org/basedir-spec/latest/ |
| rustls `CryptoProvider` | https://docs.rs/rustls/latest/rustls/crypto/struct.CryptoProvider.html |
| reqwest features | https://docs.rs/crate/reqwest/0.13.5/features |
| RTP payload formats for HEVC / H.264 | https://www.rfc-editor.org/rfc/rfc7798, https://www.rfc-editor.org/rfc/rfc6184 |
| WebCodecs | https://www.w3.org/TR/webcodecs/ |
| VSCodium | https://github.com/VSCodium/vscodium |
| Flutter custom devices | https://github.com/flutter/flutter/tree/main/packages/flutter_tools/lib/src/custom_devices |
| REUSE | https://reuse.software/spec/ |
| cargo-deny | https://github.com/EmbarkStudios/cargo-deny |
