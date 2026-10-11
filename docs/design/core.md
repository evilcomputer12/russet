# Russet core architecture (Rust side)

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates). Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day): the toolchain pin is `stable` ([D 70]), `sdk.import` is Russet's native importer ([D 69]), development worktrees get their own daemons ([D 63]), and Windows CI runs Rust on `windows-2025` ([D 77]). Review round 2 (same day) added banners to §5.1, §5.8, §6.4–§6.5, §7.6–§7.7, §7.10, §19 and §20 for the RPC flag `background` ([D 65]), `Method::permission` ([D 66]) and other superseded fields (ARCHITECTURE §20.1), and updated §9.6 (one keyring entry per `RUSSET_HOME`) and §11 (`usbmux.available`) ([D 83]).

- **Status:** design, 2026-10-11. Written for the integration lead and the implementation agents.
- **Area:** the Cargo workspace, the daemon, IPC, configuration, secrets, the toolchain manager, `russet doctor`, logging, errors, the async runtime, Windows and cross-compilation, packaging, self-update and licensing hygiene.
- **Binding inputs:** [`docs/decisions.md`](../decisions.md), [`docs/verified-facts.md`](../verified-facts.md), [`demo/m1-baseline.sh`](../../demo/m1-baseline.sh) and [`docs/research/landscape-2026-10.md`](../research/landscape-2026-10.md) (cited below as *landscape*).
- **Markers:**
  - **[V]**: verified hands-on on the dev machine on 2026-10-11. The command log is in section 23.
  - **[S n]**: external source, listed in section 24.
  - **[R:id]**: a fact-checked research record in *landscape*.
  - **[A]**: an assumption that still has to be verified. Each one has a task.

---

## 0. Summary

1. **One binary does three jobs.** `russet` is a multi-call binary: the CLI, the daemon (`russet daemon run`, or the same file invoked as `russetd`) and the MCP bridge (`russet mcp serve`). The CLI and the daemon of one install therefore can never be different builds.
2. **The daemon owns everything long-lived.** `russetd` runs once per user, any client can start it, and it exits when idle. It is the **only** owner of:
   - device tunnels (since iOS 26.2 a device keeps one RSD peer per host [R:dv-34]);
   - port forwards;
   - log, mirror and profiling streams;
   - debug sessions;
   - build, sign, install and ship jobs.
3. **IPC is JSON-RPC 2.0 with LSP framing.** Messages use the LSP base-protocol `Content-Length` framing and travel over:
   - Linux: a Unix socket in `$XDG_RUNTIME_DIR/russet/` (directory 0700, socket 0600, `SO_PEERCRED` uid check);
   - Windows: a per-user named pipe with an explicit DACL.

   Progress and cancellation use LSP's `$/progress` and `$/cancelRequest`. The IDE extension uses stock `vscode-jsonrpc` 9.0.3; interop with a tokio prototype was verified [V].
4. **Bulk binary data uses side channels.** Screen-mirror frames travel as length-prefixed frames on a second connection, so JSON messages stay small.
5. **Workspace.** About 20 crates in five layers.
   - Domain crates contain no transport code.
   - Every wire type lives in `russet-protocol`, with serde, schemars and ts-rs derives. That one crate generates the JSON Schema for MCP tools and the TypeScript types for the extension.
6. **Configuration** comes from four layers: a committed `russet.toml`, a per-user `russet.local.toml`, a global `config.toml` and environment variables. Directories follow XDG on Linux and Known Folders on Windows.
7. **Secrets.** keyring-core (Secret Service on Linux, Credential Manager on Windows) holds **one master key**, and that key seals every other secret into files. This sidesteps Windows' 2,560-byte credential-blob limit [S 7]. xtool, by contrast, keeps its auth token and a second 3 KB item as plain 0644 files [V].
8. **Toolchain manager.**
   - Pins come from a signed manifest that is generated from the upstream APIs, never hand-typed. A hand-guessed swift.org URL returned 404 [V].
   - The resolver returns a tool's path **and the environment it needs**: Swift 6.4's `lldb` and `lldb-dap` do not start without `libpython3.12.so.1.0` [V].
   - It uses xtool's 6.8 GB Darwin SDK bundle where it already sits instead of copying it [V].
9. **Runtime.** tokio throughout; idevice, jktcp, rmcp and reqwest are all tokio-based [V]. Process trees are handled by process-wrap (process groups on Linux, Job Objects on Windows).
10. **Packaging and licensing.**
    - Linux: tarball, `.deb`, `.rpm` and an Arch PKGBUILD. An AppImage exists only for the IDE.
    - Windows: zip, per-user MSI and winget. `dist` cannot publish to winget [S 9], so komac does that.
    - Package-managed installs never update themselves.
    - Licensing: GPL-3.0-or-later, a cargo-deny allow-list, one crypto provider (aws-lc-rs, whose licence expression is GPL-compatible [V]), SPDX headers and a CI gate against Apple files.

---

## 1. Scope and ownership

This document owns the generic machinery. The area design docs own their domain logic and the details of their RPC namespaces. Core reserves names, provides the plumbing, and defines the rules that every namespace follows.

| Topic | Owning doc | What core provides |
|---|---|---|
| Device plane: usbmux/lockdown, pairing, CoreDevice tunnel, RSD, DDI, install, launch, syslog/console, screenshots | device | DeviceManager actor slot, leases, streams, the `device.*`/`app.*`/`forward.*`/`logs.*`/`crash.*` namespaces, doctor check registration |
| Builds (SwiftPM via xtool, Flutter, `.xcodeproj`, RN, MAUI), project model, resource tools | build | Job machinery, `Toolchains::resolve_build_env`, the `project.*`/`build.*` namespaces, `sdk.import` plumbing |
| Signing, accounts, provisioning, IPA, ASC upload, Apple-hardware release mode | signing & publishing | Secret store, account metadata in config, the permission classes `account`, `account.destructive` and `publish`, the `account.*`/`sign.*`/`provisioning.*`/`ship.*` namespaces |
| Run, debug, hot reload: LLDB, lldb-dap, JIT helper, symbol cache | run/debug | `toolchain.resolve` (paths plus env for LLDB), sessions, forwards, the `debug.*`/`flutter.*` namespaces |
| IDE (Code-OSS fork, extension, panels), MCP tool list, agent panel | IDE + agents | IPC transports, the generated TS client, the MCP bridge process, the permission engine, `ui.*` callbacks |
| CI, hardware-in-the-loop rigs, release process | QA/delivery | `russet-testkit`, packaging tasks, cargo-deny/REUSE/no-Apple-files gates |

---

## 2. Constraints that shape the architecture

| Fact | Consequence |
|---|---|
| Rust core; device plane on `idevice` (MIT); pymobiledevice3 only as reference and fallback (decisions.md) | The daemon embeds idevice in-process. Fallback tools may only run as subprocesses that go through the daemon's forwards. |
| Since iOS 26.2 the device keeps one RSD peer per host [R:dv-34] | Exactly one process per host may own a device's tunnel, so that process is a long-lived daemon. A second tool that opens its own tunnel can break ours, and doctor warns about that. |
| jktcp's `AdapterHandle` is "a cloneable, thread-safe handle to a background Adapter task", "cheap to clone" [V: jktcp-0.1.7 `src/handle.rs`] | One software tunnel per device can carry many concurrent service streams and port forwards. |
| Headless must work (decisions.md) | Every feature is reachable from the CLI. The IDE is just one client. |
| Several clients at once: IDE windows, terminals, MCP agents, flutter_tools custom-device commands | Shared state lives in the daemon, not in the clients. |
| No root by default. The Linux usbmuxd socket is `srw-rw-rw- root /run/usbmuxd` [V] | No group setup is needed on Linux, and Russet never asks for sudo. Doctor prints commands for the user to run instead. |
| Never ship Apple files. Disk is tight: SDK bundle 6.8 GB [V], swift.org tarball 1.12 GB, Windows Swift installer 2.1 GB, LLVM Linux 1.19–2.0 GB [V] | Reuse existing installs, check free space before every download or import, and garbage-collect old toolchains. |
| Linux first, then Windows on a physical PC | A cfg-gated platform layer from day one, and Windows CI from the first week. |
| Agents get the same powers through MCP | Permission classes, per-client profiles and confirmation callbacks (section 7.5). |
| GPL-3.0-or-later; pieces meant for upstream may be MIT | cargo-deny allow-list. MIT crates must not depend on GPL crates (section 18). |

---

## 3. Process model

### 3.1 Binaries

`russet` is one multi-call executable:

| Invocation | Role |
|---|---|
| `russet <command> …` | CLI |
| `russet daemon run [--foreground]`, or the file invoked as `russetd` (argv[0] stem) | Daemon |
| `russet mcp serve --stdio` / `--http ADDR` | MCP bridge; a client of the daemon |
| `russet __flutter-device <op>` (hidden) | Commands called by the Flutter custom-device config (build and debug docs) |

Why one binary:
1. There is no CLI/daemon version skew inside one install.
2. There is one artifact to sign and package.
3. The in-process daemon (`--no-daemon`) for CI and tests reuses the same code.

The cost is binary size, estimated at 30–60 MB [A]. The IDE bundles the same binary, and the extension uses it unless the `russet.executablePath` setting overrides it.

### 3.2 Topology

```text
 Russet IDE (Code-OSS)        terminal          AI agents: Claude Code, Codex, Gemini CLI, Cline,
 built-in extension (TS)      `russet …`        Continue, Copilot, OpenHands
 vscode-jsonrpc 9.0.3             │             │ MCP over stdio or streamable HTTP
         │                        │        `russet mcp serve` (rmcp 3.5.x)
         │  JSON-RPC 2.0 + Content-Length framing; Unix socket (Linux) / named pipe (Windows)
         └────────────────────────┴─────────────┘
                                  │
                russetd: one per user, tokio, autostarted, idle-exit
 ┌──────────────┬────────────┬───────────┬───────────┬────────────┬────────────┐
 │DeviceManager │ JobManager │ Sessions  │ StreamHub │ Toolchains │ Accounts   │
 │ idevice:     │ build/sign/│ run/debug │ logs,     │ pins, SDKs,│ ASC JWT,   │
 │ usbmux,      │ install/   │ leases,   │ mirror,   │ probes     │ GSA,       │
 │ lockdown,    │ ship/test  │ forwards  │ perf      │            │ anisette   │
 │ tunnel, RSD  │            │           │           │            │            │
 └──────┬───────┴─────┬──────┴─────┬─────┴───────────┴────────────┴─────┬──────┘
        │             │            │                                      │
 usbmuxd /run/usbmuxd │ subprocesses (process group / Job Object):   HTTPS (rustls):
 or Apple Mobile      │ swift build, xtool, clang, ld64.lld,         App Store Connect API,
 Device Service       │ lldb, lldb-dap, flutter, gen_snapshot        GSA, TSS, downloads
 127.0.0.1:27015 (Win)
        │
     iPhone (USB, later Wi-Fi)
```

### 3.3 Where work runs

| Work | Runs in | Why |
|---|---|---|
| Device protocol: lockdown, tunnel, RSD services, DVT, syslog, screenshots | Daemon, in-process (idevice) | Single tunnel owner; long-lived |
| Port forwards: debugserver, Flutter VM service, WDA | Daemon tasks over the shared `AdapterHandle` | Lifetime equals the lease |
| Compile, link and resource tools: swift, xtool, clang, ld64.lld, Russet's actool, flutter, gen_snapshot | Daemon subprocesses | External toolchains; the whole tree is killed on cancel |
| Signing and IPA packing | Daemon, `spawn_blocking` | CPU-bound pure Rust. Removes the dependency on the `zip` binary, which xtool needs and Arch lacks (verified-facts.md) |
| LLDB / lldb-dap | Subprocess. The IDE's debug adapter launches lldb-dap; the daemon launches LLDB for `russet debug` and the JIT helper. The debug doc decides the split | Needs the libpython env from the resolver |
| ASC API, GSA, TSS, downloads | Daemon (reqwest + rustls) | Token caches, rate limits, backoff |
| Rendering: tables, progress bars | CLI | Client concern |
| Confirmations and prompts | The client: TTY prompt, IDE dialog or MCP elicitation, via `ui.*` callbacks | The daemon has no UI |

---

## 4. Cargo workspace

### 4.1 Layout

```text
russet/
├── Cargo.toml              # [workspace], [workspace.package], [workspace.dependencies], [workspace.lints]
├── rust-toolchain.toml     # channel = "stable"; components rustfmt, clippy (rust-version = "1.99" in Cargo.toml; ARCH D 70)
├── deny.toml  about.toml  REUSE.toml
├── crates/
│   ├── russet-core/  russet-protocol/  russet-ipc/  russet-config/  russet-secrets/
│   ├── russet-doctor/  russet-toolchain/
│   ├── russet-device/  russet-project/  russet-build/  russet-sign/  russet-account/
│   ├── russet-debug/  russet-ship/  russet-xctest/  russet-mirror/  russet-profile/
│   ├── russetd/  russet-mcp/  russet-cli/      # russet-cli builds the `russet` binary
│   └── russet-testkit/
├── xtask/                  # cargo xtask schema|ts|pins|check-layers|check-no-apple|dist
├── protocol/               # generated russet-protocol.schema.json + CHANGELOG.md (committed)
├── pins/                   # generated, signed toolchain manifests: stable.toml + stable.toml.minisig
├── packaging/linux/{deb,rpm,arch,systemd}  packaging/windows/{wix,winget}
└── extension/  ide/  docs/ # owned by the IDE doc
```

`rust-toolchain.toml` matters on this machine. rustup has `stable` (rustc 1.99.0) installed but **no default toolchain**, so a plain `cargo --version` fails [V]. The file names `stable`, so it selects the installed toolchain; naming `1.99.0` would make rustup 1.29.1 auto-install a second copy (0.6–1.5 GiB) on a disk-constrained box (ARCHITECTURE [D 70]). The workspace sets `rust-version = "1.99"`, and CI pins `RUSTUP_TOOLCHAIN=1.99.0`.

### 4.2 Crates

| Crate | Kind | Responsibility | Owner | Key deps |
|---|---|---|---|---|
| `russet-core` | lib | Errors and codes, including the wire `ErrorData`; ids; `RussetDirs`; fs helpers (atomic write, free space, private dirs); managed processes; progress/log sinks; secret wrappers; platform and WSL detection; build info; `rpc` traits (`Method`, `NotificationMethod`, `Permission`) | core | thiserror, serde, schemars, secrecy, tokio, tokio-util, process-wrap, fs4; ts-rs behind feature `ts` |
| `russet-protocol` | lib | All wire DTOs; method and notification definitions; protocol version; schema and TS export | core (every area adds its namespace) | serde, schemars, ts-rs |
| `russet-ipc` | lib | JSON-RPC engine: codec, `Peer`, `Router`, `Server`; Unix, pipe and TCP transports; auth hooks; byte streams; autostart client | core | tokio, tokio-util, serde_json, windows-sys |
| `russet-config` | lib | Config structs, layering, comment-preserving editor, JSON Schemas, file watcher | core | toml, toml_edit, serde_path_to_error, notify |
| `russet-secrets` | lib | `SecretStore` over keyring-core stores, sealed files and env; redaction | core | keyring-core plus store crates, chacha20poly1305, argon2, zeroize |
| `russet-doctor` | lib | `Check` trait, registry, report model, host checks | core | — |
| `russet-toolchain` | lib | Pin manifest, probes, resolver, installer, SDK registry | core (build doc for the importer) | reqwest, sha2, pgp, minisign-verify, tar, liblzma, zstd, zip, plist, semver, goblin |
| `russet-device` | lib | idevice integration, DeviceManager, tunnels, services, forwards, log sources, DDI | device | idevice, jktcp |
| `russet-project` | lib | Project detection, `russet.toml` to project model, Info.plist and entitlements editing (the build doc may merge this into `russet-build`) | build (+IDE) | plist |
| `russet-build` | lib | Build adapters (SwiftPM/xtool, Flutter, `.xcodeproj` later), artifacts | build | — |
| `russet-sign` | lib | Code signing, profiles, entitlements, Mach-O lints, IPA | signing | apple-codesign, goblin/object, zip |
| `russet-account` | lib | ASC API client (ES256 JWT); free Apple ID (GSA + anisette); certificates, devices, profiles | signing | jsonwebtoken or p256, reqwest |
| `russet-debug` | lib | Debug sessions, lldb and lldb-dap configs, attach scripts, symbol cache, JIT helper | run/debug | — |
| `russet-ship` | lib | Packaging lints, uploads, TestFlight, Apple-hardware mode | publishing | — |
| `russet-xctest` | lib | XCTest/XCUITest runs, JUnit output | QA | — |
| `russet-mirror` | lib | Screen capture and encoding, input injection | device/IDE | — |
| `russet-profile` | lib | DVT sysmon/fps/energy samples, crash logs, symbolication | run/debug | — |
| `russetd` | lib | Daemon: actors, lifecycle, jobs, sessions, streams, permission engine, RPC handlers (one module per namespace) | core; handler modules per area | all of the above |
| `russet-mcp` | lib | MCP bridge: tool registry mapped to RPC methods; stdio and HTTP transports | IDE/agents (core: process model and auth) | rmcp |
| `russet-cli` | lib + bin `russet` | clap tree, renderers, autostart, multi-call dispatch | core (area subcommands per doc) | clap, clap_complete, clap_mangen, indicatif, anstream, comfy-table |
| `russet-testkit` | dev lib | Fake device backend, temp-home daemon fixture, protocol transcripts | QA/core | — |
| `xtask` | dev bin | Code generation and repository checks | core | cargo_metadata |

### 4.3 Layering rules

The `cargo xtask check-layers` command enforces these rules. It reads `cargo metadata` and blocks CI on a violation.

- **L0:** `russet-core`.
- **L1:** `russet-protocol` (depends on L0).
- **L2:** `russet-ipc`, `russet-config`, `russet-secrets`, `russet-doctor`, `russet-toolchain` (depend on L0–L1, and `russet-toolchain` also on `russet-config`).
- **L3:** the domain crates. They may depend on L0–L2 **except `russet-ipc`**, so no transport code exists in domain crates. Among themselves they may only depend along these edges:
  - `project → build → sign → ship`
  - `account → sign`, `account → ship`
  - `device → debug | xctest | mirror | profile`
  - `build → debug`
- **L4:**
  - `russetd` may use everything.
  - `russet-mcp` may use only L0–L2 plus rmcp. It reaches the domain through the daemon.
  - `russet-cli` may use L0–L2 plus `russetd`, which it needs for `daemon run` and `--no-daemon`.
- **Dev only:** `russet-testkit` and `xtask` may depend on anything.

Domain crates expose plain async APIs that take `&dyn ProgressSink` and a `CancellationToken`. The `russetd/src/handlers/<namespace>.rs` modules map RPC requests to those APIs. Each area agent owns its own handler file, which avoids merge conflicts.

### 4.4 Workspace settings

```toml
[workspace]
resolver = "3"
members = ["crates/*", "xtask"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.99"
license = "GPL-3.0-or-later"
repository = "https://github.com/evilcomputer12/russet"

[workspace.lints.rust]
unsafe_code = "deny"   # #[allow] only with a // SAFETY: note, in russet-ipc (pipe DACL), russet-core::proc (pre_exec), russet-secrets
[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
unwrap_used = "deny"   # tests may opt out with #[allow]
dbg_macro = "deny"
print_stdout = "deny"  # only russet-cli's renderer prints (module-level allow)

[profile.release]
opt-level = 3
lto = "thin"
debug = "line-tables-only"   # symbolized panics; full debuginfo is split into .dwp/.pdb release assets
panic = "unwind"             # the daemon survives task panics (section 14)
```

### 4.5 Third-party crates

Versions and licences come from the crates.io API on 2026-10-11 [V]. Pin minor versions in `[workspace.dependencies]`.

| Purpose | Crate (version, licence) |
|---|---|
| Async runtime and utilities | tokio 1.53.2 (MIT); tokio-util 0.7.20 (MIT); futures 0.3.34 (MIT/Apache-2.0); bytes 1.12.1 (MIT); tokio-stream 0.1.19 (MIT) |
| Serialization and schemas | serde 1.0.229; serde_json 1.0.152 (both MIT/Apache-2.0); schemars 1.2.2 (MIT); ts-rs 12.0.1 (MIT); serde_path_to_error 0.1.20 |
| Config | toml 1.1.8 and toml_edit 0.25.17 (TOML 1.1; MIT/Apache-2.0); notify 8.2.0 (CC0-1.0) |
| CLI | clap 4.6.7; clap_complete 4.6.11; clap_mangen 0.3.3; anstream 1.0.0 (all MIT/Apache-2.0); indicatif 0.18.6 (MIT); comfy-table 8.0.1 (MIT) |
| Logging and errors | tracing 0.1.44; tracing-subscriber 0.3.23; tracing-appender 0.2.5; tracing-journald 0.3.2 (all MIT); thiserror 2.0.21 (MIT/Apache-2.0) |
| Filesystem and paths | dirs 7.0.0 (base directories only); dunce 1.0.5 (CC0/MIT-0/Apache-2.0); tempfile 3.27.0; fs4 1.1.0 (file locks); fs-err 3.3.2; which 8.0.6 (MIT); sysinfo 0.39.6 (MIT) |
| Processes | process-wrap 10.0.1 (Apache-2.0/MIT; features `tokio1`, `job-object`, `process-group`, `process-session`, `kill-on-drop`); nix 0.31.3 (MIT); windows-sys 0.61.2 |
| Secrets and crypto | keyring-core 1.0.0; zbus-secret-service-keyring-store 1.0.1; windows-native-keyring-store 1.1.0 (all MIT/Apache-2.0); secrecy 0.10.3; zeroize 1.9.1; chacha20poly1305 0.11.0; argon2 0.6.0; sha2 0.11.0; base64 0.23.1; minisign-verify 0.3.0 (MIT); pgp 0.21.0 (MIT/Apache-2.0) |
| HTTP and TLS | reqwest 0.13.5 (rustls); rustls 0.23.45, whose default provider is `aws_lc_rs`; aws-lc-rs 1.18.1 / aws-lc-sys 0.45.0 (ISC/Apache-2.0/MIT/BSD-3-Clause expression) |
| Archives and formats | tar 0.4.46; liblzma 0.4.8; zstd 0.14.2 (BSD-3-Clause); flate2 1.1.10; zip 9.0.1 (MIT); plist 1.10.1 (MIT); goblin 0.10.7 (MIT); object 0.40.0; semver 1.0.28; url 2.5.8; uuid 1.28.0; ulid 3.0.0 (MIT); time 0.3.55 |
| Domain crates (owned by other docs) | idevice 0.1.68 (MIT; edition 2024; default feature `aws-lc`); jktcp 0.1.7 (MIT); apple-codesign 0.29.0 (MPL-2.0); jsonwebtoken 11.1.0 (MIT); p256 0.14.0; mdns-sd 0.21.5 |
| MCP | rmcp 3.5.1 (Apache-2.0; features `server`, `macros`, `schemars`, `transport-io`, `transport-streamable-http-server`, `elicitation`) |
| Updates | axoupdater 0.10.2 (MIT/Apache-2.0); self-replace 1.5.0 (Apache-2.0) |
| Tests | insta 1.49.0 (Apache-2.0); proptest 1.12.0; assert_cmd 2.2.2; predicates 3.1.4; wiremock 0.6.5 |
| Dev tools (not linked) | cargo-nextest 0.9.148; cargo-deny 0.20.2; cargo-about 0.9.2; cargo-auditable 0.7.7; cargo-cyclonedx 0.5.9; dist 0.32.0; cargo-deb 3.8.0; cargo-generate-rpm 0.21.0; cargo-wix 0.3.9; cargo-xwin 0.23.1; komac 2.16.0 (GPL-3.0-or-later); cargo-machete 0.9.2; cargo-llvm-cov 0.9.1 |
| Extension side (npm) | vscode-jsonrpc 9.0.3 (MIT) [V] |

**Considered and rejected:**
- **jsonrpsee 0.26.1:** built around HTTP and WebSocket; it has no server-to-client requests over a plain byte stream.
- **async-lsp 0.2.4:** pulls in lsp-types 0.95 and an LSP-shaped API.
- **tower-lsp 0.20.0:** no release since 2023-08.
- **lsp-server 0.10.0:** synchronous.
- **interprocess 2.4.5:** fine, but tokio already provides `UnixListener` and named pipes.
- **The `keyring` 4.2.0 facade:** its own docs tell applications that choose their stores to use keyring-core and the store crates directly [S 6].
- **sequoia-openpgp:** LGPL-2.0-or-later and heavy; rPGP is enough to verify swift.org signatures.
- **figment:** last release 2024-05; our own layering is about 200 lines.
- **openssl:** banned, to keep cross-builds simple.

---

## 5. Public Rust APIs of the core crates

The signatures below are normative. Small naming changes are allowed if all call sites move together.

### 5.1 `russet-core`

> **Superseded fields (ARCHITECTURE §5.3, §20.1):** `ErrorCode` is a struct with per-family constant files
> (`russet-core/src/error/codes/<family>.rs`) [D 61], not one enum. `rpc::Method` also has `const CONFIRM` and the
> per-call `fn permission(params, conn, ownership) -> Result<Permission>`, which defaults to `PERMISSION` and is
> overridden through `rpc_method!(…, permission_fn = …)` [D 66]. The ARCHITECTURE §5.3 signatures are binding.

```rust
pub mod error {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[non_exhaustive]
    pub enum ErrorCode { /* table in section 13; each has a number, a "family.name" string and an exit code */ }
    impl ErrorCode {
        pub const fn number(self) -> i32;            // JSON-RPC error.code, e.g. 3002
        pub const fn as_str(self) -> &'static str;   // "device.locked"
        pub const fn exit_code(self) -> u8;          // CLI exit status (section 13.3)
        pub fn from_number(n: i32) -> Option<Self>;
    }
    #[derive(Debug, thiserror::Error)]
    #[error("{message}")]
    pub struct Error { /* code, message, hint, details: Option<serde_json::Value>, retryable, source */ }
    impl Error {
        pub fn new(code: ErrorCode, message: impl Into<String>) -> Self;
        pub fn with_hint(self, hint: impl Into<String>) -> Self;
        pub fn with_details(self, details: serde_json::Value) -> Self;
        pub fn retryable(self, yes: bool) -> Self;
        pub fn caused_by(self, e: impl std::error::Error + Send + Sync + 'static) -> Self;
        pub fn code(&self) -> ErrorCode;
        pub fn hint(&self) -> Option<&str>;
        pub fn to_data(&self) -> ErrorData;          // serialized into JSON-RPC error.data
    }
    /// Wire shape of JSON-RPC `error.data`. It lives in core (L0) because every layer
    /// produces it; russet-protocol re-exports it.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct ErrorData { pub russet_code: String, pub hint: Option<String>, pub details: Option<serde_json::Value>,
                           pub retryable: bool, pub docs: Option<String>, pub chain: Vec<String> }
    pub type Result<T, E = Error> = std::result::Result<T, E>;
    #[macro_export] macro_rules! bail { ($code:expr, $($fmt:tt)+) => { return Err($crate::error::Error::new($code, format!($($fmt)+))) } }
}

pub mod rpc {   // pure traits; no I/O
    pub trait Method: 'static {
        const NAME: &'static str;                     // "device.list"
        const PERMISSION: Permission;
        const LONG_RUNNING: bool = false;             // accepts progressToken, creates a Job
        type Params: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send;
        type Result: serde::de::DeserializeOwned + serde::Serialize + schemars::JsonSchema + Send;
    }
    pub trait NotificationMethod: 'static { const NAME: &'static str; type Params: /* same bounds */; }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub enum Permission { None, Read, Device, Build, Project, Account, AccountDestructive, Publish, Secrets, System }
}

pub mod dirs {
    #[derive(Debug, Clone)]
    pub struct RussetDirs { pub config: PathBuf, pub data: PathBuf, pub state: PathBuf, pub cache: PathBuf, pub runtime: PathBuf }
    impl RussetDirs {
        pub fn from_env() -> crate::Result<Self>;    // RUSSET_HOME, RUSSET_*_DIR, XDG_*, %APPDATA%, %LOCALAPPDATA%
        pub fn under(root: &Path) -> Self;           // portable mode and tests
        pub fn ensure(&self) -> crate::Result<()>;   // create; 0700 on Unix; owner check
        pub fn toolchains(&self) -> PathBuf; pub fn sdks(&self) -> PathBuf; pub fn ddis(&self) -> PathBuf;
        pub fn anisette(&self) -> PathBuf; pub fn secrets(&self) -> PathBuf; pub fn logs(&self) -> PathBuf;
        pub fn jobs(&self) -> PathBuf; pub fn downloads(&self) -> PathBuf; pub fn symbols(&self) -> PathBuf;
        #[cfg(unix)]    pub fn socket_path(&self) -> PathBuf;   // runtime/russetd.sock (section 7.1)
        #[cfg(windows)] pub fn pipe_name(&self) -> String;      // \\.\pipe\russet-<SID>[-<hash8>]
    }
}

pub mod fsx {
    pub fn atomic_write(path: &Path, bytes: &[u8]) -> crate::Result<()>;  // temp file in same dir + fsync + rename
    pub fn free_space(path: &Path) -> crate::Result<u64>;
    pub fn ensure_space(path: &Path, needed: u64, reserve: u64) -> crate::Result<()>; // core.diskFull + hint
    pub fn ensure_private_dir(path: &Path) -> crate::Result<()>;          // 0700 + owner == current uid (Unix); user-only ACL (Windows)
}

pub mod proc {
    pub struct ManagedCommand { /* wraps tokio::process::Command */ }
    impl ManagedCommand {
        pub fn new(program: impl AsRef<OsStr>) -> Self;
        pub fn arg(&mut self, a: impl AsRef<OsStr>) -> &mut Self;
        pub fn args<I, S>(&mut self, a: I) -> &mut Self where I: IntoIterator<Item = S>, S: AsRef<OsStr>;
        pub fn env(&mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> &mut Self;
        pub fn envs(&mut self, e: &BTreeMap<String, String>) -> &mut Self;   // from ToolInfo.env
        pub fn current_dir(&mut self, d: impl AsRef<Path>) -> &mut Self;
        pub fn low_priority(&mut self, nice: i32) -> &mut Self;  // Linux setpriority; Windows BELOW_NORMAL_PRIORITY_CLASS
        pub fn spawn(self) -> crate::Result<ManagedChild>;
        // Linux: own process group + PR_SET_PDEATHSIG(SIGKILL). Windows: Job Object with
        // KILL_ON_JOB_CLOSE and CREATE_NO_WINDOW. The child tree dies with the daemon.
        // Every spawn runs on one long-lived spawner thread, because PDEATHSIG fires when the
        // spawning *thread* exits (ARCHITECTURE §5.3 `proc`, D 73).
    }
    pub struct ManagedChild { /* process_wrap tokio child */ }
    impl ManagedChild {
        pub fn id(&self) -> Option<u32>;
        pub fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout>;
        pub fn take_stderr(&mut self) -> Option<tokio::process::ChildStderr>;
        pub async fn wait(&mut self, cancel: &CancellationToken, grace: Duration) -> crate::Result<ExitStatus>;
        // on cancel: SIGTERM to the group (or TerminateJobObject on Windows), wait `grace`, then SIGKILL
        pub async fn kill_tree(&mut self) -> crate::Result<()>;
    }
    pub async fn run_logged(cmd: ManagedCommand, cancel: &CancellationToken, sink: &dyn crate::progress::ProgressSink) -> crate::Result<ExitStatus>;
}

pub mod progress {
    pub enum LogStream { Stdout, Stderr, Info, Warn }
    pub trait ProgressSink: Send + Sync {
        fn begin(&self, title: &str, cancellable: bool);
        fn report(&self, message: Option<&str>, percentage: Option<u8>);
        fn end(&self, message: Option<&str>);
        fn log(&self, stream: LogStream, line: &str);
    }
    pub struct NullProgress;
}

pub mod secret { pub use secrecy::{ExposeSecret, SecretBox, SecretString}; pub struct Redacted<T>(pub T); /* Debug/Display print "***" */ }
pub mod platform { pub enum HostOs { Linux, Windows }; pub fn host_triple() -> &'static str; pub fn is_wsl() -> bool; pub fn linux_distro() -> Option<OsRelease>; }
pub mod build_info { pub const VERSION: &str; pub const BUILD_ID: &str /* "0.1.0+g<sha>" */; pub const TARGET: &str; pub fn packaged_by() -> Option<&'static str> /* "deb"|"rpm"|"arch"|"msi"|"winget" */; }
pub mod task { pub fn spawn_supervised<F>(name: &'static str, fut: F) -> tokio::task::JoinHandle<Option<F::Output>> where F: Future + Send + 'static, F::Output: Send; /* catch_unwind + log */ }
```

### 5.2 `russet-ipc`

```rust
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub enum Endpoint {
    #[cfg(unix)] Unix(PathBuf),
    #[cfg(windows)] NamedPipe(String),
    Tcp { addr: std::net::SocketAddr, token: SecretString },          // WSL split mode only
}
impl Endpoint {
    pub fn default_for(dirs: &RussetDirs) -> Self;
    pub fn parse(s: &str) -> Result<Self>;   // "unix:/path" | "pipe:\\.\pipe\name" | "tcp://127.0.0.1:7777#<token-file>"
}

pub struct LspCodec;   // tokio_util::codec::{Decoder<Item = Message>, Encoder<Message>}; Content-Length framing
pub enum Message { Request(Request), Notification(Notification), Response(Response) }
pub enum RequestId { Num(i64), Str(String) }
pub struct RpcError { pub code: i32, pub message: String, pub data: Option<ErrorData> }

#[derive(Clone)]
pub struct Peer { /* one per connection; usable from both ends */ }
impl Peer {
    pub async fn call<M: Method>(&self, params: M::Params) -> Result<M::Result>;
    pub async fn call_with<M: Method>(&self, params: M::Params, opts: CallOptions) -> Result<M::Result>;
    pub fn notify<N: NotificationMethod>(&self, params: N::Params) -> Result<()>;
    pub fn connection(&self) -> &ConnectionInfo;   // id, client kind and name, grant, peer identity
    pub async fn closed(&self);
}
pub struct CallOptions {
    pub cancel: Option<CancellationToken>,                            // sends $/cancelRequest when fired
    pub on_progress: Option<Box<dyn FnMut(ProgressValue) + Send>>,    // allocates a progressToken
    pub timeout: Option<Duration>,
}

pub struct Router<C: Clone + Send + Sync + 'static> { /* method name -> boxed handler */ }
impl<C: Clone + Send + Sync + 'static> Router<C> {
    pub fn new() -> Self;
    pub fn request<M, F, Fut>(&mut self, handler: F) -> &mut Self
        where M: Method, F: Fn(C, RequestCtx, M::Params) -> Fut + Send + Sync + 'static,
              Fut: Future<Output = Result<M::Result>> + Send + 'static;
    pub fn notification<N, F, Fut>(&mut self, handler: F) -> &mut Self
        where N: NotificationMethod, F: Fn(C, ConnCtx, N::Params) -> Fut + Send + Sync + 'static,
              Fut: Future<Output = ()> + Send + 'static;
    pub fn methods(&self) -> impl Iterator<Item = MethodInfo> + '_;  // name, permission, long_running
}
pub struct RequestCtx { pub peer: Peer, pub cancel: CancellationToken, pub progress: ProgressReporter, pub request_id: RequestId }

pub trait Authorizer: Send + Sync {
    fn on_connect(&self, id: &PeerIdentity) -> Result<()>;                          // uid/SID/token checks
    fn on_initialize(&self, id: &PeerIdentity, p: &InitializeParams) -> Result<Grant>;
    fn decide(&self, grant: &Grant, method: &MethodInfo, params: &serde_json::Value) -> Decision;
}
pub enum Decision { Allow, Confirm { title: String, message: String, danger: bool }, Deny { reason: String } }
pub struct PeerIdentity { pub uid: Option<u32>, pub sid: Option<String>, pub pid: Option<u32>, pub transport: TransportKind }

pub struct Server { /* listener */ }
impl Server {
    pub async fn bind(endpoint: &Endpoint, opts: ListenOptions) -> Result<Self>;
    pub async fn serve<C: Clone + Send + Sync + 'static>(self, router: Arc<Router<C>>, ctx: C,
        auth: Arc<dyn Authorizer>, shutdown: CancellationToken) -> Result<()>;
}

pub async fn connect(endpoint: &Endpoint, client: ClientInfo) -> Result<(Peer, InitializeResult)>;
pub async fn connect_or_spawn(dirs: &RussetDirs, client: ClientInfo, spawn: SpawnSpec) -> Result<(Peer, InitializeResult)>;
pub struct SpawnSpec { pub executable: PathBuf, pub args: Vec<OsString>, pub ready_timeout: Duration }

// Byte streams (section 7.8)
pub async fn open_stream(endpoint: &Endpoint, stream_id: &str, token: &str) -> Result<StreamReader>;
pub struct StreamReader;
impl StreamReader { pub async fn next(&mut self) -> Result<Option<StreamFrame>>; }
pub struct StreamFrame { pub kind: u8, pub payload: bytes::Bytes }
```

### 5.3 `russet-protocol`

- Each namespace is one module (`daemon`, `job`, `config`, `secrets`, `toolchain`, `sdk`, `doctor`, `ui`, `device`, `app`, `build`, …).
- Each method is declared with the `rpc_method!` macro, which emits a zero-sized marker type plus `impl Method`:

```rust
rpc_method!(DeviceList, "device.list", DeviceListParams => DeviceListResult, Permission::Read);
rpc_method!(BuildRun,  "build.run",   BuildRunParams  => BuildRunResult,  Permission::Build, long_running);
rpc_notification!(JobUpdated, "job.updated", JobUpdatedParams);
pub const PROTOCOL: ProtocolVersion = ProtocolVersion { major: 1, minor: 0 };
pub fn registry() -> &'static [MethodInfo];  // used for capabilities, MCP tools, docs and the schema export
```

- Every DTO derives `Serialize, Deserialize, JsonSchema, TS` with `#[serde(rename_all = "camelCase")]`.
- Inbound DTOs never use `deny_unknown_fields`, for forward compatibility.

### 5.4 `russet-config`

> **As implemented (W3, 2026-10-11):** `ConfigService` (ARCHITECTURE §5.3) wraps these functions: `load`, `global`,
> `global_loaded`, `effective`, `project`, `mcp_policy`, `provision_policy`, `agents(client, project)`, `lookup`
> (`config.get`, with `files`), `set`/`unset`, `schema`, `editor`, `subscribe`. Files are re-read when their stamp
> changes; an invalid file never stops the daemon (`global()` falls back to the defaults; `effective()` and doctor
> report the error with `path:line:col`). `ConfigEditor::set` coerces the value to the schema's type (`"2"` → `2`)
> and validates the whole file before `save`, which writes atomically (new `config.toml` files are 0600). Every
> file of ARCHITECTURE §14 has a typed model and a schema (`schemas/config/*.schema.json`, checked by
> `crates/russet-config/tests/schemas.rs`). `[agents]` narrows per file: `russet.local.toml` cannot widen
> `russet.toml` either. `RUSSET_DEVICE` → `run.device` and `RUSSET_LOG` → `daemon.log-filter` are the env layer.

```rust
pub fn find_project_root(start: &Path) -> Result<Option<PathBuf>>;     // walks up to the nearest russet.toml
pub fn load_global(dirs: &RussetDirs) -> Result<Loaded<GlobalConfig>>;
pub fn load_project(root: &Path) -> Result<Loaded<ProjectConfig>>;      // russet.toml + russet.local.toml
pub fn effective(global: &GlobalConfig, project: Option<&ProjectConfig>, env: &EnvOverrides) -> Effective;
pub struct Loaded<T> { pub value: T, pub sources: Vec<SourceInfo> }    // file paths, for error messages and `config get`
pub struct ConfigEditor { /* toml_edit::DocumentMut + path */ }
impl ConfigEditor {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn get(&self, dotted_key: &str) -> Option<serde_json::Value>;
    pub fn set(&mut self, dotted_key: &str, value: serde_json::Value) -> Result<()>;  // validated against the schema
    pub fn unset(&mut self, dotted_key: &str) -> Result<bool>;
    pub fn save(&self) -> Result<()>;                                                  // atomic; keeps comments and order
}
pub fn project_schema() -> schemars::Schema;
pub fn global_schema() -> schemars::Schema;
pub fn watch(paths: &[PathBuf]) -> Result<tokio::sync::mpsc::Receiver<ConfigChanged>>;
```

Errors report the file, line and column. Spans come from `toml_edit` and key paths from `serde_path_to_error`.

### 5.5 `russet-secrets`

> **As implemented (W3, 2026-10-11):** the ARCHITECTURE §5.3 API (`SecretId {kind, name}`, `SecretBytes`,
> `put/get/delete/list/backend`) plus `SecretStore::status()` (an async probe with a default) and
> `BackendInfo.keyring_entry`. Backends: `SealedStore` with `KeyringKeySource` or `PassphraseKeySource`,
> `EnvStore` (read-only CI variables), `MemoryStore` (tests); `redact` holds the field-name and JWT/PEM/Bearer
> helpers. See §9.6 for the on-disk layout.

```rust
pub struct SecretRef(String);   // stable id, e.g. "asc/team-main", "appleid/me@example.com", "cert/dev-ABCDE12345"
pub enum SecretKind { AscPrivateKey, AppleIdSession, AnisetteState, SigningKey, Token }
pub enum BackendKind { SecretService, WindowsCredentialManager, PassphraseFile, Env }
pub struct SecretMeta { pub id: SecretRef, pub kind: SecretKind, pub label: String, pub backend: BackendKind, pub created_at: time::OffsetDateTime, pub size_bytes: usize }

#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(&self, id: &SecretRef, kind: SecretKind, label: &str, value: SecretBox<[u8]>) -> Result<SecretMeta>;
    async fn get(&self, id: &SecretRef) -> Result<SecretBox<[u8]>>;   // only in-process; never sent over IPC
    async fn delete(&self, id: &SecretRef) -> Result<bool>;
    async fn list(&self) -> Result<Vec<SecretMeta>>;
    fn backend(&self) -> BackendKind;
}
pub fn open_default(dirs: &RussetDirs, cfg: &SecretsConfig) -> Result<Arc<dyn SecretStore>>;
```

### 5.6 `russet-toolchain`

> **As implemented (W3, 2026-10-11):** `Requirement` is `{version, flavor}` (F2 refinement; `flavor` carries
> `[toolchain] linker` and `[debug] lldb`), `Provenance` gains `Uv`, `gc(dry_run)` returns a `GcReport`, and
> `Toolchains::{new, load, subscribe, resolve_build_env, set_min_free_space}` exist; `probe_all` lists every
> candidate (a rejected system LLD too) in the ARCHITECTURE §16.1 order. Probes run through an `Exec` seam with a
> cache in `cache/toolchain-probes.json` keyed by path, mtime, size, arguments and env.

```rust
pub enum Component { Swift, DarwinSdk, Linker, Lldb, LldbDap, LibPython, Xtool, Flutter, GenSnapshotIos, Llvm }
pub enum Provenance { Managed { pin: String }, System, XtoolBundle { bundle: PathBuf }, User, ProjectPin }
pub struct ToolInfo {
    pub component: Component, pub version: String, pub path: PathBuf, pub provenance: Provenance,
    pub env: BTreeMap<String, String>,   // e.g. LD_LIBRARY_PATH + PYTHONHOME for Swift's lldb (section 10.4)
    pub ok: bool, pub problems: Vec<String>,
}
pub struct SdkInfo {
    pub id: String /* "xcode-27.0-27A266a" */, pub xcode_version: String, pub xcode_build: String,
    pub ios_sdk: String, pub swift_sdk_name: String /* "darwin" */, pub bundle: PathBuf,
    pub darwin_tools: Option<String>, pub open_apple_macros: Option<String>, pub compatible_swift: String,
}
pub struct BuildEnv { pub swift: ToolInfo, pub sdk: SdkInfo, pub linker: ToolInfo, pub env: BTreeMap<String, String> }

pub struct Toolchains { /* dirs, pins, probe cache */ }
impl Toolchains {
    pub fn new(dirs: RussetDirs, pins: Arc<PinManifest>) -> Self;
    pub async fn probe_all(&self, refresh: bool) -> Vec<ToolInfo>;
    pub async fn sdks(&self) -> Result<Vec<SdkInfo>>;
    pub async fn resolve(&self, c: Component, req: &Requirement) -> Result<ToolInfo>;
    pub async fn resolve_build_env(&self, project: &ToolchainSection) -> Result<BuildEnv>;  // consistent Swift + SDK + linker
    pub async fn install(&self, c: Component, version: &str, p: &dyn ProgressSink, cancel: &CancellationToken) -> Result<ToolInfo>;
    pub async fn uninstall(&self, c: Component, version: &str) -> Result<u64>;  // bytes freed
    pub async fn gc(&self, keep: &[ToolInfo]) -> Result<u64>;
}
pub struct PinManifest { /* section 10.2 */ }
impl PinManifest {
    pub fn embedded() -> &'static PinManifest;
    pub fn load_verified(toml: &[u8], minisig: &str, pubkey: &str) -> Result<Self>;
    pub fn xcode(&self, xcode_version: &str) -> Option<&XcodeCompat>;
}
```

### 5.7 `russet-doctor`

> **As implemented (W3, 2026-10-11):** `Check` also has `fix_kind()` (`None | Auto | Command(cmd) | Manual`);
> `CheckCx` carries the directories, the daemon's device backend, the usbmux address, `min-free-space` and typed
> `extensions` through which russetd hands checks its services (L2 crates cannot implement `Check`, [D 100]);
> `russet_doctor::async_trait` is re-exported for implementors. `Registry::run` runs checks concurrently with a
> 60 s timeout each and reports them in registration order.

```rust
pub enum Status { Pass, Warn, Fail, Skip }
pub enum Category { Host, Daemon, Config, Secrets, Usbmux, Device, Toolchain, Account, Network, Platform }
pub struct Requires { pub daemon: bool, pub device: bool, pub network: bool, pub project: bool }
#[async_trait::async_trait]
pub trait Check: Send + Sync {
    fn id(&self) -> &'static str;          // stable id, e.g. "toolchain.linker"
    fn title(&self) -> &'static str;
    fn category(&self) -> Category;
    fn requires(&self) -> Requires;
    async fn run(&self, cx: &CheckCx) -> CheckResult;
    async fn fix(&self, _cx: &CheckCx) -> Result<()> { Err(Error::new(ErrorCode::NotImplemented, "no automatic fix")) }
}
pub struct Registry { /* Vec<Arc<dyn Check>> */ }
impl Registry {
    pub fn register(&mut self, c: Arc<dyn Check>);
    pub async fn run(&self, cx: &CheckCx, filter: &Filter) -> DoctorReport;
}
```

Area crates implement `Check`. `russetd::doctor::registry()` registers every check.

### 5.8 `russetd`

> **Superseded fields (ARCHITECTURE §5.3, §6.2, §7.6, §20.1):** `JobSpec.detach` is `JobSpec.background` (the RPC
> flag `background: true`) [D 65]; session lifetime is the separate `sessionLease: owned | detached`. `DaemonCtx` is the
> typed struct of ARCHITECTURE §5.3 (`ConfigService`, `AccountService`, `DebugService`, `LeaseTable`,
> `SubscriptionHub`, …) [D 86].

```rust
pub struct DaemonOptions { pub dirs: RussetDirs, pub endpoint: Endpoint, pub idle_timeout: Option<Duration>, pub foreground: bool, pub tcp: Option<TcpListen> }
pub async fn run(opts: DaemonOptions) -> Result<()>;                          // `russet daemon run`
pub struct InProcess;
impl InProcess { pub async fn start(opts: DaemonOptions) -> Result<(InProcess, Peer)>; pub async fn stop(self) -> Result<()>; }  // --no-daemon and tests

#[derive(Clone)]
pub struct DaemonCtx {
    pub dirs: RussetDirs, pub config: ConfigService, pub secrets: Arc<dyn SecretStore>,
    pub toolchains: Arc<Toolchains>, pub jobs: JobManager, pub sessions: SessionRegistry,
    pub streams: StreamHub, pub perms: Arc<PermissionEngine>,
    pub devices: russet_device::DeviceManagerHandle,      // the device doc defines this type
    pub shutdown: CancellationToken,
}
impl JobManager {
    pub async fn submit<T, F, Fut>(&self, spec: JobSpec, f: F) -> Result<JobHandle<T>>
        where T: Serialize + Send + 'static, F: FnOnce(JobCtx) -> Fut + Send + 'static,
              Fut: Future<Output = Result<T>> + Send + 'static;
    pub fn list(&self, filter: &JobFilter) -> Vec<JobInfo>;
    pub fn cancel(&self, id: &JobId) -> Result<()>;
}
pub struct JobSpec { pub kind: &'static str, pub title: String, pub project: Option<PathBuf>, pub device: Option<String>,
                     pub locks: Vec<LockKey> /* Project(path) | Device(udid) | Global(name) */, pub owner: Owner, pub detach: bool }
pub struct JobCtx { pub id: JobId, pub cancel: CancellationToken, pub progress: Arc<dyn ProgressSink>, pub log_path: PathBuf }
```

### 5.9 `russet-cli`

```rust
pub fn main_entry() -> std::process::ExitCode;     // multi-call dispatch on argv[0] ("russet" | "russetd")
pub struct GlobalArgs { pub json: bool, pub device: Option<String>, pub project: Option<PathBuf>, pub verbose: u8, pub quiet: bool, pub color: ColorChoice, pub no_daemon: bool, pub yes: bool }
pub trait Reporter { fn progress(&self, p: &ProgressValue); fn log(&self, l: &LogLine); fn result(&self, v: &serde_json::Value); fn error(&self, e: &Error); }
// HumanReporter (anstream + indicatif) and JsonReporter (NDJSON on stdout, section 12)
```

---

## 6. The daemon (`russetd`)

### 6.1 What it owns

| Resource | Owner inside the daemon | Lifetime | Persisted |
|---|---|---|---|
| Device registry: attached and known devices, state | DeviceManager actor (device doc) | Daemon lifetime | `state/devices.json` (name, model, OS, last seen, aliases) |
| Per-device session: usbmux connection, lockdown, CoreDeviceProxy software tunnel (`AdapterHandle`), RSD service map, heartbeat | DeviceSession actor, one per UDID | While attached **and** (an active lease **or** within `device.keep-warm`, default 10 min) | No |
| Port forwards: localhost listener to a device port or RSD service | ForwardManager under the device session | Its lease | No |
| Log streams: app console, syslog, os_trace | StreamHub source per device and source, plus ring buffers | While subscribed; app console for the run session | Optionally `state/runs/<run-id>/console.log` |
| Mirror and perf streams | StreamHub (device, profile and mirror docs) | While subscribed | No |
| Debug sessions: suspended launch, debugproxy forward, LLDB child, JIT helper | SessionRegistry (debug doc) | Until stopped, client gone (unless detached), or app exit | `state/sessions.json`, for crash cleanup |
| Jobs: build, sign, install, test, ship, toolchain install, SDK import | JobManager | Queue → run → finish; records kept | `state/jobs/index.jsonl`, `state/jobs/<id>/{log.ndjson,result.json}` |
| Account sessions: ASC JWT minted on demand (max 20 min [R:sg-21]), GSA tokens, anisette state | AccountService (signing doc) | In memory; secrets go to the secret store | Through `russet-secrets` only |
| Toolchain and SDK probe cache | Toolchains | Invalidated by file watches and `refresh` | `cache/toolchain-probes.json` |
| Config | ConfigService with watcher | Daemon lifetime | Files |

### 6.2 Internal structure

- **Actors.** Each long-lived resource is a tokio task with a bounded `mpsc` inbox and `oneshot` replies. The rest of the daemon holds a cheap cloneable `*Handle`. There are no global mutexes around device I/O.
- **Supervision.** Tasks start through `russet_core::task::spawn_supervised`.
  - A panic in, say, one DeviceSession is logged with a backtrace and written to `state/crashes/`.
  - The device is marked `failed` and the actor restarts with backoff (1 s, 2 s, 4 s, capped at 30 s).
  - The daemon itself keeps running.
- **Handlers.** `russetd::handlers::<ns>` validates params, asks the `PermissionEngine` for a decision, sends `ui.confirm` if needed, then calls the domain API. Long-running methods go through `JobManager::submit`.
- **Locks.** A `LockKey` grants exclusive access:
  - `Project(root)` for builds;
  - `Device(udid)` for install, uninstall, DDI mount, pairing and reboot.

  Read-only methods never take locks. Jobs that wait for a lock show the state `queued` with a "waiting for …" message.

### 6.3 Lifecycle

**Autostart, run by any client** (implemented in `russet_ipc::connect_or_spawn`):
1. Resolve the endpoint: `RUSSET_DAEMON`, otherwise the default.
2. Try to connect, with a 200 ms timeout. On success, go to the handshake in step 5.
3. If the connection is refused or the socket is missing (on Windows, `ERROR_FILE_NOT_FOUND`):
   1. Take `runtime/spawn.lock` (fs4, exclusive, waiting at most 10 s).
   2. Try to connect again, since another client may have started the daemon in the meantime.
   3. Otherwise spawn **the client's own executable** (`std::env::current_exe()`) as `daemon run --detach`, with stdio set to null:
      - Linux: `setsid` in `pre_exec`.
      - Windows: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW`.
   4. Poll the connection with backoff for up to 10 s, then release the lock.
4. The daemon takes `runtime/russetd.lock` exclusively and holds it for its whole life, before it binds anything.
   - If the lock is already held, it exits with code 12 (`ipc.daemonUnavailable`, "already running").
   - If it gets the lock but a socket file is left over, it unlinks the file and binds.
   - On Windows, `first_pipe_instance(true)` is a second guard. Wine 11.19 does **not** enforce that flag [V], so the lock file is the primary mechanism and the flag only protects against pipe squatting on real Windows [S 4].
5. **Handshake.** `initialize` returns the daemon's `buildId`.
   - Same protocol major, different build: continue in release builds and print a notice once a day in human mode. Development builds fail with `ipc.buildMismatch` (1107) instead, and they also compare the daemon's executable identity with their own `current_exe()`, because `buildId` does not change between rebuilds of an uncommitted tree (ARCHITECTURE §3.3, [D 83]).
   - Different major: if the daemon has no active jobs or sessions, the client sends `daemon.shutdown {reason: "upgrade"}` and then spawns its own build. Otherwise it fails with `ipc.protocolMismatch`, hint `russet daemon restart --force`.

**Readiness.** The daemon writes `runtime/russetd.json`, a discovery and diagnostics file with mode 0600:

```json
{"pid": 4242, "version": "0.1.0", "buildId": "0.1.0+g1a2b3c4", "protocol": {"major": 1, "minor": 0},
 "endpoint": "unix:/run/user/1000/russet/russetd.sock", "startedAt": "2026-10-11T09:00:00Z", "tcp": null}
```

**Idle exit.** The daemon exits when all of these have held for `daemon.idle-timeout` (default 30 min; `0` means never):
- no connections;
- no running or queued jobs;
- no detached leases.

Headless device hosts should set `0`, or run the optional systemd user unit (section 16).

**Graceful shutdown.** Triggered by SIGTERM or SIGINT on Linux, Ctrl-Break on Windows, or `daemon.shutdown`:
1. Stop accepting connections.
2. Broadcast `daemon.shuttingDown {reason, graceMs}`.
3. Cancel jobs, with 10 s of grace before killing process trees.
4. Close sessions and tunnels.
5. Remove the socket and release the lock.

`force` skips the grace period.

**Crash recovery** at startup:
- jobs recorded as `running` become `interrupted`;
- orphaned debug sessions in `state/sessions.json` are cleaned up. Their child processes are already dead, thanks to PDEATHSIG or the Job Object.

**Upgrades.** Package upgrades replace the binary on disk. The next client to connect sees a different `buildId` and restarts the idle daemon. Active debug sessions are never killed implicitly.

**Development isolation** (several agent worktrees under one OS user; ARCHITECTURE §3.3 item 7, [D 63, D 83]): each lane-D worktree sets `RUSSET_HOME=<worktree>/.russet-dev` and gets its own daemon and keyring entry, while the serial device lane uses one persistent `RUSSET_HOME` ([D 82]); lane-D worktrees use the scripted device backend; a development daemon refuses the `live` backend without `RUSSET_LANE=A`; any live daemon holds the host-wide live-device lock (tests override its path in debug builds); development clients also compare the daemon's executable identity; tests use a fresh temporary `RUSSET_HOME` and the `memory` or `passphrase` secrets backend.

### 6.4 Jobs

- **States:** `queued → running → succeeded | failed | cancelled`, plus `interrupted` after a crash.
- **Kinds:** `build`, `install`, `run`, `test`, `sign`, `ship.upload`, `toolchain.install`, `sdk.import`, `doctor.fix` and others. Area docs may add kinds.
- **Ownership.** By default a job belongs to the connection that requested it. If that connection closes, the job is cancelled within 1 s, unless the request set `detach: true` (**superseded:** the flag is `background: true`, ARCHITECTURE §7.6 [D 65]). `job.list` shows jobs from every client: the IDE sees CLI jobs and the reverse.
- **Concurrency.**
  - `daemon.max-parallel-jobs` (default 4).
  - `daemon.max-build-jobs` (default 1, because the machine is shared with a llama.cpp server).
  - Locks as in section 6.2.
  - Build subprocesses run at `daemon.build-nice` (default 10).
- **Logs.** Each job writes `state/jobs/<id>/log.ndjson`, one line per event: `{ts, stream, line}` or `{ts, progress}`.
- **Retention.** The newest 200 jobs, and nothing older than 30 days; whichever limit is hit first applies. Compaction runs at startup.

### 6.5 Leases and sessions

- A **lease** pins a resource: a device tunnel, a forward, a run session, a debug session, a stream. Leases are owned by a connection, a job, or nobody (`detached`).
- When the owner disconnects, its non-detached leases are released at once.
- The IDE creates debug and run sessions as detached, so that a window reload does not kill them. After a reload it finds them again with `debug.sessions` and `app.runs` (**superseded:** `run.list`, which returns `SessionSummary`, ARCHITECTURE §8.5, §9.5 [D 24, D 84]).
- Detached leases end when closed explicitly, when the app exits (for run and debug sessions), or when the daemon shuts down. `russet daemon status` lists them.

### 6.6 Subscriptions and byte streams

- **Subscriptions** (`logs.subscribe`, `job.subscribe`, `device.watch`, `perf.start`, `daemon.logs.subscribe`) return a `subscriptionId`. Notifications carry that id.
  - Each subscription has a bounded queue: 10,000 log entries, or 256 messages for the other kinds.
  - Slow consumers lose the **oldest** entries, and the next batch carries `dropped: n`.
  - A device reader is never blocked by a client.
  - `subscription.cancel` ends a subscription; disconnecting ends all of the connection's subscriptions.
- **Byte streams** (mirror frames; later possibly pcap or bulk crash files) are explained in section 7.8. Slow consumers drop whole frames, never partial ones.

### 6.7 Resource budgets

| Item | Budget |
|---|---|
| Idle daemon | Under 50 MB RSS, about 0 % CPU [A]: no polling, because usbmux attach events are pushed |
| Buffers | Logs: 10k entries × up to 4 KiB per device. Mirror: at most 3 frames per subscriber |
| CPU | Builds use the `max-build-jobs` cap and `nice` 10 on Linux, `BELOW_NORMAL_PRIORITY_CLASS` on Windows |
| Disk | `fsx::ensure_space` with `storage.min-free-space` (default 5 GiB) before every download, extraction, SDK import or build-cache growth. On failure: `core.diskFull` with a hint (`russet toolchain gc`, `russet sdk remove`) |

### 6.8 Platform notes for the daemon

| Topic | Linux | Windows |
|---|---|---|
| Endpoint | Socket `$XDG_RUNTIME_DIR/russet/russetd.sock` (the directory must be 0700 and owned by the uid). Without `XDG_RUNTIME_DIR`: `/tmp/russet-<uid>/`, created 0700 and owner-checked to resist symlink races. `sun_path` holds at most 108 bytes, so overlong paths fall back to `/tmp/russet-<uid>/<hash>.sock` | `\\.\pipe\russet-<user SID>`, plus `-<hash8>` when `RUSSET_HOME` is set. Lock, pid and json files go in `%LOCALAPPDATA%\Russet\run\` |
| Spawn | `setsid`; stdio to `/dev/null`; logs go to files | `DETACHED_PROCESS`, `CREATE_NEW_PROCESS_GROUP`, `CREATE_BREAKAWAY_FROM_JOB`, `CREATE_NO_WINDOW`. Whether the daemon survives the IDE's exit or a Job Object is [A], tested on the Windows PC |
| Children | Process group and `PR_SET_PDEATHSIG(SIGKILL)` | Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, through process-wrap's `job-object` feature |
| Signals | SIGTERM and SIGINT mean graceful; SIGHUP reloads config | `ctrl_c`/`ctrl_break` mean graceful; there are no services |
| Service mode | Optional `systemd --user` unit (`russetd.service`), off by default | None in v0.x. A per-user autostart entry is possible later |
| Device transport | System usbmuxd at `/run/usbmuxd` | Apple Mobile Device Service at `127.0.0.1:27015`, or netmuxd [R:dv-32]. Both are detected by doctor; neither is shipped |

---

## 7. IPC protocol `russet/1`

### 7.1 Endpoints and transports

| Transport | Address | Access control |
|---|---|---|
| Unix socket (Linux, default) | `unix:$XDG_RUNTIME_DIR/russet/russetd.sock` | Directory 0700, socket 0600. The server checks `peer_cred().uid() == own uid`. The client checks that the directory owner is its uid and that the server's peer uid matches. Both checks were prototyped [V] |
| Named pipe (Windows, default) | `\\.\pipe\russet-<SID>` | Explicit DACL `D:P(A;;GA;;;<UserSID>)(A;;GA;;;SY)`, set through `create_with_security_attributes_raw` [S 5]. The default pipe DACL would grant **read access to Everyone and Anonymous** [S 4]. `reject_remote_clients` is on (tokio's default) and `first_pipe_instance(true)` is set. The client calls `GetNamedPipeServerProcessId`, opens the process token and checks that its owner SID is the user's, which blocks pipe squatting |
| TCP (opt-in, WSL split mode) | `tcp://127.0.0.1:<port>`, written with a random 256-bit token to `runtime/tcp.json` (mode 0600, user-only ACL on Windows) | Token in `initialize.params.auth.token`, compared in constant time. A connection that has not authenticated within 2 s is closed. Non-loopback binds need `--allow-nonlocal` and print a warning |
| Byte-stream side channel | Same endpoint, new connection | Preamble with a single-use stream token (section 7.8) |

### 7.2 Framing

The framing is LSP's base protocol: `Content-Length: <n>\r\n\r\n<n bytes of UTF-8 JSON>`.
- Header names are case-insensitive. `Content-Type` is accepted and ignored.
- The maximum frame is 16 MiB. A larger frame gets error 1104 and the connection is closed.

**Why this framing:** the stock `vscode-jsonrpc` `SocketMessageReader`/`SocketMessageWriter` speak it. In the prototype, the Node client on a Unix socket got:
- the `initialize` result;
- typed `$/progress` delivered through `onProgress`;
- `-32800` after a `CancellationTokenSource.cancel()`;
- `-32601` for an unknown method.

Mean round trip was 37 µs from Node and 15 µs from Rust. Under Wine, a Windows named pipe from Rust gave 110 µs [V].

### 7.3 Conventions

- **Requests** are named `namespace.verb` (`device.list`). **Notifications** are named `namespace.event`, a past-tense verb or a noun (`device.changed`, `logs.batch`). The LSP built-ins `$/progress` and `$/cancelRequest` keep their names.
- `params` and `result` are always JSON objects, never positional arrays or bare scalars, so they can grow.
- Field names are camelCase. Enum values are kebab-case strings. Times are RFC 3339 UTC strings. Durations are integer milliseconds with an `Ms` suffix. Sizes are integer bytes with a `Bytes` suffix.
- Paths are absolute UTF-8 strings. A non-UTF-8 path on Windows is rejected with `core.invalidArgument`.
- Request ids from clients are numbers or strings. The server's own requests to clients use string ids starting with `s:`.
- Unknown fields are ignored. Optional fields are omitted rather than sent as `null`.

### 7.4 Handshake and versioning

```jsonc
// client → daemon (must be the first message; anything else returns -32002 "not initialized")
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{
  "protocol":{"major":1,"minor":0},
  "client":{"kind":"ide","name":"russet-ide","version":"0.1.0","pid":5150},
  "capabilities":{"confirm":true,"prompt":true,"openUrl":true,"streams":["mirror"]},
  "workspace":{"roots":["/home/u/Hello"]},
  "auth":{"token":null}}}
// daemon → client
{"jsonrpc":"2.0","id":1,"result":{
  "protocol":{"major":1,"minor":0},
  "server":{"name":"russetd","version":"0.1.0","buildId":"0.1.0+g1a2b3c4","pid":4242,
            "executable":"/usr/bin/russet","startedAt":"2026-10-11T09:00:00Z","platform":"linux","arch":"x86_64"},
  "connectionId":"c-7",
  "grant":{"profile":"ide","allow":["read","device","build","project","account","publish","secrets","system"],
           "confirm":["account-destructive","publish"],"deny":[]},
  "capabilities":{"methods":["daemon.ping","…"],"notifications":["job.updated","…"],
                  "features":{"mirror":true,"wifiDevices":false,"tcpListen":false}}}}
```

- `client.kind` is one of `cli | ide | mcp | flutter-device | test`.
- **Version rules:**
  - The major version changes only for breaking changes: a removed or renamed method or field, or changed semantics.
  - The minor version changes for additions.
  - Clients check `capabilities.methods` and `features` before using anything newer than `1.0`.
  - A major mismatch fails `initialize` with error 1100, whose `data.details` is `{serverProtocol, clientProtocol, restartable}`.
- **Change tracking:** `protocol/CHANGELOG.md` lists every change. The committed schema snapshot (`protocol/russet-protocol.schema.json`) is diffed in CI. A diff without a version bump fails CI.

### 7.5 Authentication and permissions

**Threat model, stated plainly:**
1. The OS user is the security boundary. Other local users and remote hosts are kept out by the transport rules in section 7.1.
2. Permission profiles are **guardrails, not a sandbox.** An agent that has shell access can run the CLI as the user. Profiles stop accidental or prompt-injected destructive actions by agents that only reach Russet through MCP. They also stop a committed `russet.toml` in a cloned repository from widening those permissions.

**Permission classes** (`russet_core::rpc::Permission`):

| Class | Covers |
|---|---|
| `none` | `initialize`, `daemon.ping` |
| `read` | lists, status, logs, screenshots, `doctor.run` without `fix` |
| `device` | install, launch, kill, forwards, mirror input, DDI mount, pairing |
| `build` | build, clean, test; writes only under the project's `.russet/` and build outputs |
| `project` | edits to `russet.toml`, Info.plist, entitlements, assets |
| `account` | Apple-side create and read operations: certificates, devices, bundle IDs, profiles |
| `account-destructive` | revoke certificates, delete profiles or bundle IDs, remove devices |
| `publish` | upload to App Store Connect, TestFlight distribution, submission for review |
| `secrets` | `secrets.set`, `secrets.delete`, adding or removing accounts |
| `system` | toolchain and SDK install or remove, global config writes, `daemon.shutdown`, log level, self-update |

**Default profiles:**

| Profile | Allow | Confirm | Deny |
|---|---|---|---|
| `cli` | everything | `account-destructive`, `publish`: TTY prompt; `--yes` skips it; with no TTY and no `--yes` → `perm.confirmationUnavailable` | — |
| `ide` | everything | `account-destructive`, `publish` (dialog) | — |
| `mcp` | `read`, `device`, `build` | `project`, `account` | `account-destructive`, `publish`, `secrets`, `system` |
| `flutter-device` | `read`, `device`, `build` | — | everything else |
| `test` | everything | — | — |

- The global `config.toml` `[agents]` section may widen or narrow the `mcp` profile.
- A project's `[agents]` section may **only narrow** it, so a cloned repository cannot grant itself rights.
- **Confirmation flow:**
  1. The daemon sends the server-to-client request `ui.confirm` to the requesting connection, if it declared `capabilities.confirm`.
  2. Otherwise it sends the request to an attached IDE connection, whose dialog says which client asked.
  3. Otherwise it denies the request with `perm.confirmationUnavailable`.

  Confirmations time out after 120 s and are then treated as declined.
- **Secret prompts** (`ui.prompt` with `secret: true`), such as an Apple ID password or a 2FA code, go only to `cli` and `ide` clients. **Never to MCP:** the MCP spec says servers "MUST NOT use elicitation to request sensitive information" [S 11].

### 7.6 Progress, cancellation and jobs

- Long-running methods (`LONG_RUNNING = true`) accept an optional `progressToken` (string or number) in their params. Progress follows LSP's work-done shape, which vscode-jsonrpc's `ProgressType` consumes as-is [V]:

```json
{"jsonrpc":"2.0","method":"$/progress","params":{"token":"b-17","value":{"kind":"begin","title":"Building Hello","cancellable":true,"jobId":"01JAB3…"}}}
{"jsonrpc":"2.0","method":"$/progress","params":{"token":"b-17","value":{"kind":"report","message":"Compiling ContentView.swift","percentage":42}}}
{"jsonrpc":"2.0","method":"$/progress","params":{"token":"b-17","value":{"kind":"end","message":"Built in 7.1 s"}}}
```

- **Cancellation:** `$/cancelRequest {"id": <request id>}` cancels the request's token and so its job. The response is error `-32800` (`RequestCancelled`), with `data.russetCode = "core.cancelled"` [S 2][V].
- **Detach:** with `detach: true` the request returns at once with `{jobId}`. The result is then available through `job.wait` or `job.get`. (**Superseded:** the RPC flag is `background: true`; CLI `--detach` means "leave the session running" and never sets it, ARCHITECTURE §7.6 [D 65].)
- **Default:** the request stays open and returns the final result, and the job is still visible to other clients through `job.*`.

### 7.7 Notifications

> **Superseded (ARCHITECTURE §9.1, §20.1):** `job.output` is `job.events`, `logs.batch` is `logs.entries`, and
> `stream.closed` goes with the byte streams, which v1 does not implement (the mirror uses the WebSocket) [D 2, D 54].

| Notification | Params |
|---|---|
| `$/progress` | `{token, value: {kind: begin\|report\|end, title?, message?, percentage?, cancellable?, jobId?}}` |
| `daemon.shuttingDown` | `{reason: "idle"\|"request"\|"upgrade"\|"signal", graceMs}` |
| `daemon.logBatch` | `{subscriptionId, records: [{ts, level, target, message, fields}], dropped?}` |
| `job.updated` | `{subscriptionId, job: JobInfo}` |
| `job.output` | `{subscriptionId, jobId, lines: [{ts, stream, line}], dropped?}` |
| `device.changed` | `{subscriptionId, change: "added"\|"removed"\|"updated", device: DeviceInfo}` (device doc) |
| `logs.batch` | `{subscriptionId, entries: [LogEntry], dropped?}` (device doc) |
| `perf.samples` | `{subscriptionId, samples: [...]}` (profile doc) |
| `config.changed` | `{scope: "global"\|"project"\|"local", path}` |
| `toolchain.changed` | `{components: string[]}` |
| `stream.closed` | `{streamId, reason}` |
| `ui.showMessage` (to client) | `{level: "info"\|"warning"\|"error", message, actions?: string[]}` |

### 7.8 Byte streams

```text
1. client: <stream method>, e.g. mirror.start {device, maxFps, codec}
   → {streamId, token, codec, width, height}
2. client opens a NEW connection to the same endpoint and sends exactly one line:
     RUSSET-STREAM/1 <streamId> <token>\n
   server: "OK\n", or "ERR <code> <message>\n" and close.
   The token is single-use, expires 10 s after issue, and is bound to the requesting
   connection's peer identity.
3. server → client frames until close:
     u32 big-endian length (of kind + payload) | u8 kind | payload
     kind 0x01 media (codec as announced: "jpeg" or "h264-annexb", per the mirror doc)
          0x02 JSON metadata (e.g. orientation change)
          0x03 keepalive
          0x0F end
4. stream.close {streamId} on the control connection, or closing either connection, ends the stream.
```

The IDE's extension host reads the stream and passes the `ArrayBuffer` frames to the mirror webview with `postMessage`. Webviews never touch the socket.

### 7.9 Errors

```json
{"jsonrpc":"2.0","id":9,"error":{"code":3002,"message":"iPhone 12 is locked",
 "data":{"russetCode":"device.locked","hint":"Unlock the phone and keep it unlocked during install.",
         "retryable":true,"details":{"udid":"00008101-…"},"docs":"https://…/errors/device.locked"}}}
```

- JSON-RPC and LSP protocol errors keep their standard codes [S 1][S 2]:
  - `-32700` parse error;
  - `-32600` invalid request;
  - `-32601` method not found;
  - `-32602` invalid params, with `data.details.path` from serde_path_to_error;
  - `-32603` internal error;
  - `-32002` server not initialized;
  - `-32800` request cancelled.
- Russet's own codes are positive numbers from 1000 to 9999 (section 13). The JSON-RPC spec reserves only −32768 to −32000 [S 1].

### 7.10 Method catalog

> **Superseded (ARCHITECTURE §8, §20.1):** ARCHITECTURE §8 is the binding catalog, including for the core-owned rows
> below. In particular the long-running flag is `background`, not `detach` (`toolchain.install`, `forward.open`)
> [D 65]; per-call classes come from `Method::permission` [D 66]; and the reserved namespaces map onto `run.*`,
> `screen.*`, `crash.*`, `job.events` and the other names of ARCHITECTURE §8 and §9.

**Core-owned methods.** ARCHITECTURE §8 now holds the authoritative rows (banner above); this table is kept for its rationale.

| Method | Permission | Params | Result |
|---|---|---|---|
| `initialize` | none | see 7.4 | see 7.4 |
| `daemon.ping` | none | `{}` | `{}` |
| `daemon.status` | read | `{}` | `{version, buildId, pid, uptimeMs, endpoint, connections:[{id, kind, name, pid}], jobs:{queued, running}, leases:[{id, kind, owner, detached}], devices:int, rssBytes}` |
| `daemon.shutdown` | system | `{force?: bool, reason?: string}` | `{accepted: bool, activeSessions: int}` |
| `daemon.setLogFilter` | system | `{filter: string}` (EnvFilter syntax) | `{previous: string}` |
| `daemon.logs.subscribe` | read | `{minLevel?: "trace"\|…\|"error"}` | `{subscriptionId}` |
| `subscription.cancel` | read | `{subscriptionId}` | `{}` |
| `stream.close` | read | `{streamId}` | `{}` |
| `job.list` | read | `{states?: JobState[], kinds?: string[], project?: string, limit?: int}` | `{jobs: JobInfo[]}` |
| `job.get` | read | `{jobId}` | `JobInfo` |
| `job.wait` | read | `{jobId, timeoutMs?}` | `{job: JobInfo, result?: any, error?: ErrorData}` |
| `job.cancel` | the job's owner, or system | `{jobId}` | `{state: JobState}` |
| `job.subscribe` | read | `{jobId?: string, includeOutput?: bool}` | `{subscriptionId}` |
| `config.get` | read | `{scope: "effective"\|"global"\|"project"\|"local", project?: string, key?: string}` | `{value: any, sources: [{scope, path}]}` |
| `config.set` | project (project/local scope) or system (global) | `{scope, project?, key, value}` | `{path, changed: bool}` |
| `config.unset` | as `config.set` | `{scope, project?, key}` | `{path, changed: bool}` |
| `config.schema` | read | `{file: "project"\|"global"}` | `{schema: JSONSchema}` |
| `secrets.status` | read | `{}` | `{backend, available: bool, locked: bool, reason?: string}` |
| `secrets.list` | read | `{}` | `{items: SecretMeta[]}` (metadata only) |
| `secrets.set` | secrets | `{id, kind, label?, value?: string, fromFile?: string}` | `SecretMeta` (the value is never echoed back) |
| `secrets.delete` | secrets | `{id}` | `{deleted: bool}` |
| `toolchain.list` | read | `{project?: string, refresh?: bool}` | `{tools: ToolInfo[], sdks: SdkInfo[], pins: {id, source}}` |
| `toolchain.resolve` | read | `{component, project?: string}` | `ToolInfo`, **including `env`** |
| `toolchain.install` | system, long | `{component, version, progressToken?, detach?}` | `ToolInfo` |
| `toolchain.uninstall` | system | `{component, version}` | `{freedBytes}` |
| `toolchain.gc` | system | `{dryRun?: bool}` | `{freedBytes, removed: string[]}` |
| `toolchain.pins.update` | system | `{channel?: "stable"\|"beta"}` | `{id, changed: bool}` |
| `sdk.list` | read | `{}` | `{sdks: SdkInfo[]}` |
| `sdk.import` | system, long | superseded by ARCHITECTURE §8.1: sources include `XcodeSystemResources.pkg`, `iOS_DDI.dmg` and DDI folders, and the result reports the DDI too | `ImportReport`. Russet's native importer on `russet-xar` (PLAN W16b) [D 69] |
| `sdk.remove` | system | `{id}` | `{freedBytes}` |
| `doctor.run` | read; `fix` needs system | `{checks?: string[], categories?: string[], project?: string, device?: string, online?: bool, fix?: bool}` | `DoctorReport` (section 11) |

**Server-to-client requests**, served by clients that declared the capability:

| Method | Params | Result |
|---|---|---|
| `ui.confirm` | `{title, message, detail?, danger: bool, requestedBy: {kind, name}, timeoutMs}` | `{confirmed: bool}` |
| `ui.prompt` | `{title, message, kind: "text"\|"password"\|"code", secret: bool, requestedBy, timeoutMs}` | `{value?: string, cancelled: bool}` |
| `ui.openUrl` | `{url, reason}` | `{opened: bool}` |

**Reserved namespaces.** The shapes are provisional and the owning doc is authoritative. Permissions are as listed.

| Namespace | Owner | Methods (permission) |
|---|---|---|
| `device.*` | device | `list` (read), `get` (read), `watch` (read) → `device.changed`, `pair` (device), `unpair` (device), `prepare` (device; pair, Developer Mode, DDI and tunnel to ready), `developerMode.reveal` (device), `ddi.mount` (device), `screenshot` (read) → `{path\|pngBase64}`, `rename`/`alias.set` (project) |
| `forward.*` | device | `open {udid, devicePort\|service, localPort?:0, detach?}` → `{forwardId, localPort}` (device), `close` (device), `list` (read) |
| `app.*` | device | `list` (read), `install` (device, long), `uninstall` (device), `launch {udid, bundleId, args?, env?, suspended?, console?}` → `{runId, pid, consoleSubscriptionId?}` (device), `kill` (device), `runs` (read), `processes` (read) |
| `logs.*` | device | `subscribe {udid, source: "console"\|"syslog"\|"oslog", filter?}` (read), `unsubscribe` |
| `crash.*` | run/debug | `list` (read), `fetch` (read), `symbolicate` (read) |
| `project.*` | build | `detect` (read), `init` (project), `info` (read), `plist.get/set` (read/project), `entitlements.get/set` (read/project), `assets.*` (project) |
| `build.*` | build | `run {project, configuration, destination, clean?, progressToken?}` → `{artifacts: {app, ipa?, dsym?}, durationMs, warnings}` (build, long), `clean` (build), `settings` (read) |
| `sign.*`, `provisioning.*` | signing | `sign.app` (build), `provisioning.ensure {project, udid?, kind}` (account), `provisioning.list` (read) |
| `account.*` | signing | `list` (read), `add` (secrets; Apple ID sign-in uses `ui.prompt`), `remove` (secrets), `test` (read), `teams` (read), `certs.list` (read), `certs.create` (account), `certs.revoke` (account-destructive), `devices.register` (account), `devices.remove` (account-destructive) |
| `debug.*`, `flutter.*` | run/debug | `debug.prepare` → `{sessionId, lldb: ToolInfo, lldbDap: ToolInfo, debugserverPort, initCommands[], attachCommands[], dapConfig}` (device), `debug.stop` (device), `debug.sessions` (read), `flutter.run`, `flutter.attach`, `flutter.reload` (device) |
| `xctest.*` | QA | `run` (build + device, long), `results` (read) |
| `perf.*`, `mirror.*`, `input.*` | profile, mirror | `perf.start` (read) / `perf.stop`; `mirror.start` (read) → byte stream; `input.tap` / `swipe` / `text` / `button` (device) |
| `ship.*`, `testflight.*` | publishing | `ship.lint` (read), `ship.package` (build), `ship.upload` (publish, long), `ship.status` (read), `testflight.distribute` (publish), `ship.remoteBuild` (publish; Apple-hardware mode) |

### 7.11 Schemas and generated clients

`cargo xtask schema` writes:
- `protocol/russet-protocol.schema.json`: every method, with params and result schemas, built from `registry()` and schemars;
- `extension/src/generated/protocol.ts`: the ts-rs types plus a typed `RussetClient` wrapper around vscode-jsonrpc;
- `docs/reference/rpc.md`: the method tables.

CI runs the generator and fails if the committed output is stale. The MCP bridge reads the same registry at runtime.

---

## 8. MCP bridge: the core part

The IDE/agents doc owns the tool list, client configuration snippets and the agent panel. Core owns:

1. **Process model.** The agent tool spawns `russet mcp serve --stdio`. The bridge then:
   - connects to the daemon through `connect_or_spawn` with `client.kind = "mcp"`;
   - sets `client.name` to MCP's `clientInfo.name`, so confirmations can say "Claude Code wants to …";
   - logs to stderr and to `state/logs/mcp-<pid>.log`, never to stdout, because stdout carries the protocol.
2. **Mapping.**
   - Each exposed tool maps to one RPC method, or to a documented composite such as `russet_run` (build, install, launch).
   - The tool `inputSchema` is the schemars schema of `Params`. Results go back as structured content plus a one-line text summary.
   - MCP `_meta.progressToken` is forwarded as the RPC `progressToken`, and `$/progress` is translated to `notifications/progress` with a monotonic `progress` value [S 12].
   - MCP `notifications/cancelled` becomes `$/cancelRequest`.
3. **Permissions.** The daemon enforces them; the bridge only translates.
   - When the daemon sends `ui.confirm`, the bridge uses `elicitation/create` with a boolean schema if the client declared the `elicitation` capability [S 11]. Otherwise the tool fails with `perm.confirmationUnavailable` and a hint to approve in the Russet IDE or CLI.
   - `ui.prompt` with `secret: true` is always refused over MCP.
4. **HTTP transport** for containerized agents such as OpenHands in Docker: `russet mcp serve --http 127.0.0.1:0 --token-file <path>`, using rmcp's streamable-HTTP server.
   - A bearer token is mandatory.
   - Binding to a Docker bridge address (for example `172.17.0.1`) needs `--allow-nonlocal`.
   - A container on a bridge network cannot reach a listener bound to `127.0.0.1` without host networking. The IDE/agents doc picks the default for the user's stack (open question 9).

---

## 9. Configuration

### 9.1 Files and precedence

Highest priority first:
1. CLI flags.
2. `RUSSET_*` environment variables.
3. `russet.local.toml` next to `russet.toml`: per user, gitignored. Device, account and team choices go here.
4. `russet.toml`: committed with the project.
5. Global `config.toml`.
6. Built-in defaults.

`[agents]` is the exception: project files can only narrow it (section 7.5).

### 9.2 Project file `russet.toml`

Keys are kebab-case. The table names the owning doc for each section; core owns the file format, layering and validation.

```toml
schema = 1                                  # file format version (core)

[project]                                   # core + build
name = "Hello"
kind = "swiftpm"                            # swiftpm | flutter | xcodeproj | react-native | maui
product = "Hello"                           # SwiftPM product (swiftpm); for xcodeproj: project/scheme/target keys (build doc)

[app]                                       # build
bundle-id = "com.example.hello"
display-name = "Hello"
version = "1.0.0"                           # CFBundleShortVersionString
build = "1"                                 # CFBundleVersion; "auto" = increment per build
deployment-target = "17.0"
device-family = ["iphone"]
info-plist = "Resources/Info.plist"         # optional extra keys, merged
entitlements = "Hello.entitlements"         # optional
icon = "Resources/AppIcon.png"

[signing]                                   # signing doc
account = "team-main"                       # id of an account in the global config
team-id = "ABCDE12345"
style = "automatic"                         # automatic | manual
bundle-id-strategy = "exact"                # exact (paid default) | prefixed (free-account fallback)
capabilities = []

[build]                                     # build
configuration = "debug"
env = {}

[run]                                       # device / run-debug
device = "auto"                             # auto | <udid> | <alias>
args = []
env = {}
console = true

[debug]                                     # run/debug
lldb = "auto"                               # auto | swift | llvm | <path>
init-commands = []

[flutter]                                   # build (kind = "flutter")
version = "3.47.7"
target = "lib/main.dart"

[ship]                                      # publishing
asc-app-id = ""
release-mode = "linux"                      # linux | apple-hardware
stamp-metadata = false                      # opt-in and visible ([R:sg-30], risk R4)

[toolchain]                                 # core; overrides of the pin manifest
swift = "6.4.0"
xcode-sdk = "27.0"                          # needs an imported SDK of this Xcode version
linker = "auto"                             # auto | darwin-tools@1.1.0 | llvm@>=23.1.3 | <path>

[agents]                                    # core; may only NARROW the global mcp profile
deny = ["publish"]
```

`russet.local.toml` uses the same schema. Typical content is `[run] device = "iphone12"` and `[signing] account = "me-free"`. `russet init` adds it to `.gitignore`.

### 9.3 Global `config.toml`

```toml
schema = 1
[daemon]
idle-timeout = "30m"            # "0" = never
max-parallel-jobs = 4
max-build-jobs = 1
build-nice = 10
log-filter = "info"
log-retention-days = 7
listen-tcp = ""                 # e.g. "127.0.0.1:0" for WSL split mode; off by default
[device]
keep-warm = "10m"
[storage]
min-free-space = "5GiB"
[accounts]
default = "team-main"
[accounts.team-main]
kind = "asc-api-key"            # paid team key, role App Manager (decisions.md)
issuer-id = "00000000-0000-0000-0000-000000000000"
key-id = "XXXXXXXXXX"
key = "secret:asc/team-main"    # reference into the secret store; never the key itself
team-id = "ABCDE12345"
[accounts.me-free]
kind = "apple-id"
apple-id = "me@example.com"
session = "secret:appleid/me@example.com"
anisette = "local"              # local (Apple's Android libraries, fetched at runtime) | remote:<url>
[devices.aliases]
iphone12 = "<udid>"
[agents]                        # the mcp profile (section 7.5)
allow = ["read", "device", "build"]
confirm = ["project", "account"]
deny = ["account-destructive", "publish", "secrets", "system"]
[update]
check = "notify"                # notify | off (open question 2)
channel = "stable"
[privacy]
telemetry = "off"               # Russet has no telemetry; the key only documents that
```

### 9.4 Environment variables

| Variable | Meaning |
|---|---|
| `RUSSET_HOME` | Portable root. All five directories go under it, and the endpoint gets a hash suffix |
| `RUSSET_CONFIG_DIR`, `RUSSET_DATA_DIR`, `RUSSET_STATE_DIR`, `RUSSET_CACHE_DIR`, `RUSSET_RUNTIME_DIR` | Override single directories |
| `RUSSET_DAEMON` | Endpoint override (`unix:`, `pipe:`, `tcp://…`), or `inproc` (same as `--no-daemon`) |
| `RUSSET_NO_AUTOSTART=1` | Fail instead of spawning the daemon |
| `RUSSET_LOG`, `RUSSET_LOG_FORMAT` | tracing filter; `text` or `json` |
| `RUSSET_DEVICE` | Default device selector |
| `RUSSET_ASC_KEY_ID`, `RUSSET_ASC_ISSUER_ID`, `RUSSET_ASC_KEY` (PEM) or `RUSSET_ASC_KEY_PATH` | CI accounts. Kept in memory only, never stored |
| `RUSSET_SECRETS_BACKEND` | `auto`, `keyring`, `passphrase` or `env` |
| `RUSSET_SECRETS_PASSPHRASE` | Unlocks the passphrase-sealed store on headless hosts |
| `NO_COLOR`, `CI` | Respected: `CI` disables update checks and interactive prompts |

### 9.5 Directories

| Purpose | Linux | Windows |
|---|---|---|
| config | `$XDG_CONFIG_HOME/russet` (`~/.config/russet`) | `%APPDATA%\Russet\config` |
| data: toolchains, SDK registry, DDI imports, anisette libraries, sealed secrets | `$XDG_DATA_HOME/russet` (`~/.local/share/russet`) | `%LOCALAPPDATA%\Russet\data` |
| state: logs, jobs, devices, sessions, crashes | `$XDG_STATE_HOME/russet` (`~/.local/state/russet`) | `%LOCALAPPDATA%\Russet\state` |
| cache: downloads, symbol cache, probe cache | `$XDG_CACHE_HOME/russet` (`~/.cache/russet`) | `%LOCALAPPDATA%\Russet\cache` |
| runtime: socket, locks, pid, json | `$XDG_RUNTIME_DIR/russet` (`/run/user/<uid>/russet` [V]) | `%LOCALAPPDATA%\Russet\run` plus the pipe |
| Swift SDKs | Left where SwiftPM keeps them: `~/.swiftpm/swift-sdks/<name>.artifactbundle` [V]. The registry records them | Same SwiftPM location [A] |
| Project-local | `<project>/.russet/`: disposable build outputs, generated plists, `attach.lldb`. Never holds secrets. Gitignored | Same |

Sources: the XDG Base Directory spec [S 3] and the Windows Known Folders.

**Repository fix found during this work:** `/.gitignore` still lists `.orchard/`, a leftover of the rename. It should list `.russet/`. The lead should fix this when committing.

### 9.6 Secrets

> **As implemented (W3, 2026-10-11):** the master key is stored base64-encoded as the keyring entry's password
> (no binary-secret quirks). `data/secrets/` (0700, files 0600) holds `<sha256(id)>.sealed` (magic `RSSEAL01`,
> version, 24-byte nonce, XChaCha20-Poly1305 ciphertext; AAD `russet-secret\0 id \0 kind \0 version`),
> `index.json` (names, labels, sizes, dates), `master.check` (a sealed constant: a wrong passphrase or a replaced
> keyring entry is refused with `secrets.locked` before anything is overwritten), `store.json` (the sealing backend,
> which `auto` keeps) and `master.salt`. For a new store `auto` takes the keyring when a session bus exists, else
> the passphrase store when `RUSSET_SECRETS_PASSPHRASE` is set. Nothing touches the keyring at open; keyring-core
> runs on blocking threads on first use. CI ids: `asc-key/<RUSSET_ASC_KEY_ID>`, `signing-key/ci`,
> `signing-key/ci.password`.

**What counts as a secret:**
- ASC `.p8` private keys;
- Apple ID session tokens (GSA) and anisette provisioning state;
- private keys for development and distribution certificates that Russet generates;
- tokens: the TCP token and the MCP HTTP token.

**What is not a secret** and lives in config: issuer id, key id, team id, Apple ID email.

**Storage design:**
1. **Master key.** One random 32-byte key in the OS keyring, through keyring-core [S 6]:
   - Linux: `zbus-secret-service-keyring-store`, a pure-Rust D-Bus client, so there is no libdbus dependency. On this machine `org.freedesktop.secrets` is served by `ksecretd` under KDE Plasma 6 [V].
   - Windows: `windows-native-keyring-store` (Credential Manager).
   - Entry: `Entry::new("dev.russet", "master-key")`, then `set_secret`/`get_secret` [S 6]. **Updated (review round 2, ARCHITECTURE §3.3 item 7, [D 83]):** when `RUSSET_HOME` is set, the entry is `master-key-<first 8 hex digits of sha256(RUSSET_HOME)>`, so development worktrees and tests never share or reset one key; tests use the `memory` or `passphrase` backend.
2. **Sealed blobs.** Each secret is a file `data/secrets/<sha256(id)>.sealed`, encrypted with XChaCha20-Poly1305 under the master key. The additional authenticated data is `id ‖ kind ‖ version`. Why:
   - Windows Credential Manager caps a blob at 2,560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE` = 5×512) [S 7]. xtool's own credential item on this machine is already 3,265 bytes [V], and a `.p12` with a private key is similar.
   - The keyring prompts at most once.
   - Every secret uses the same code path.
3. **Headless fallback.** When no keyring is available (SSH, containers, CI), or keyring-core returns `NoStorageAccess` or `NoDefaultStore` [S 6]: the master key is derived from `RUSSET_SECRETS_PASSPHRASE` with Argon2id (m = 64 MiB, t = 3, p = 1; salt in `data/secrets/master.salt`).
4. **CI.** In `env` mode, ASC credentials come only from `RUSSET_ASC_*` and are never written to disk.
5. **The IPC never returns secret values.** `secrets.get` does not exist on the wire. Inside the daemon, values stay in `SecretBox` and are zeroized on drop.
6. **Migration.** `russet account add asc-key --key-file AuthKey_X.p8` reads the file once and seals it. Russet never reads xtool's store. That store holds `~/.config/xtool/data/XTLAuthToken` (369 bytes) plus a 3,265-byte item, both mode **0644** (world-readable) [V; contents not read], which Russet should not copy as a pattern.

---

## 10. Toolchain manager

### 10.1 Components

| Component | Linux sources (in resolution order) | Windows sources | Probe |
|---|---|---|---|
| Swift | project pin → managed (swift.org tarball) → system `swift` (AUR `swift-bin` gives `/usr/lib/swift`; 6.4 [V]) → swiftly toolchains if present | managed or system install (`Swift.Toolchain` via winget [S 8]); needs VS 2022, Windows 11 SDK 22621 and VC tools [S 8] | `swift --version` |
| Darwin SDK | Registry of imported bundles in `~/.swiftpm/swift-sdks/*.artifactbundle` | Same, once the importer runs on Windows (build doc, M6) | `Xcode.app/Contents/version.plist`, `iPhoneOS.sdk/SDKSettings.json`, `darwin-sdk-version.txt` |
| Linker (`ld64.lld` that can read `arm64e.x1` TBDs) | xtool bundle `toolset/bin/ld64.lld` (darwin-tools 1.1.0, LLD 21.1.6 patched [V]) → managed darwin-tools → system lld ≥ 23.1.3 (this host has 23.1.1, too old [V][R:tc-05]) | Managed LLVM 23.1.3 Windows build | `--version`; for darwin-tools, the bundle's version file |
| LLDB (Swift-aware) | Swift toolchain `lldb` 21.0.0 [V] plus libpython 3.12 → llvm.org `lldb` ≥ 23.1 for non-Swift and Flutter JIT [R:fx-20] | Swift toolchain's or LLVM's lldb [A: Python dependency] | `lldb --version` with the env applied |
| lldb-dap | The same toolchain as LLDB; it also needs libpython 3.12 [V] | Same | `lldb-dap --version` |
| libpython (shared) | ELF `NEEDED` of `liblldb.so` (`libpython3.12.so.1.0` [V]), searched in `ldconfig` paths → managed `python-shared` → known prefixes (`~/.local/opt/python-3.12` [V], pyenv) | n/a [A] | `goblin` reads `NEEDED`; then a dlopen-free existence check |
| xtool | managed AppImage → `xtool` on PATH (1.21.0 [V]) | none (WSL only [R:tc-10]) | `xtool --version` |
| Flutter | managed → PATH | managed → PATH | `flutter --version --machine` |
| gen_snapshot (iOS target) | Russet CI builds per Flutter tag (build doc) | Same | Dart revision embedded |

### 10.2 Pin manifest

> **As implemented (W3, 2026-10-11; [D 87]):** Wave 1's `pins/stable.toml` holds the Xcode 27 row and the
> `python-shared` (3.12.15, release 20261009) and `darwin-tools` (1.1.0) artifacts for `linux-x86_64` and
> `linux-aarch64` (`url`, `size`, `sha256`, `format`). `cargo xtask pins` reads the release metadata with
> `gh api`, hashes each asset as it streams (cross-checked with GitHub's published digest), and signs with a
> minisign-compatible CLI (`minisign` or `rsign`) and the development key `~/.config/russet-dev/minisign.key`
> (`--init-dev-key`; public half `pins/dev-minisign.pub`). Debug builds trust the dev key; release builds trust only
> W26's keys (none yet).

`pins/stable.toml` is generated by `cargo xtask pins` from the upstream APIs:
- swift.org `api/v1/install/releases.json` [S 13];
- GitHub releases;
- Flutter's `releases_<os>.json`.

It is signed with minisign (`pins/stable.toml.minisig`), and a copy is embedded in each release binary. Hand-typed URLs are forbidden. The first guess, `swift-6.4-RELEASE`, returned 404; the real tag is `swift-6.4.0-RELEASE` [V].

```toml
schema = 1
id = "2026.10.11"
min-russet = "0.1.0"

[[xcode]]                         # compatibility matrix [R:tc-01][R:tc-03][R:tc-05]
version = "27.0"
build = "27A266a"
ios-sdk = "27.0"
swift = "6.4"                     # minor must match
min-lld = "23.1.3"                # arm64e.x1 TBDs; darwin-tools >= 1.1.0 also accepted
[[xcode]]
version = "26.6"
build = "17F113"
ios-sdk = "26.5"
swift = "6.3"

[[component]]
name = "swift"
version = "6.4.0"
tag = "swift-6.4.0-RELEASE"       # released 2026-09-14
[component.artifacts.linux-x86_64-ubi10]
url = "https://download.swift.org/swift-6.4.0-release/ubi10/swift-6.4.0-RELEASE/swift-6.4.0-RELEASE-ubi10.tar.gz"
size = 1122271085
signature = { kind = "pgp", url = "https://download.swift.org/swift-6.4.0-release/ubi10/swift-6.4.0-RELEASE/swift-6.4.0-RELEASE-ubi10.tar.gz.sig" }
sha256 = "<filled in by xtask pins>"
[component.artifacts.linux-x86_64-ubuntu2404]
url = "https://download.swift.org/swift-6.4.0-release/ubuntu2404/swift-6.4.0-RELEASE/swift-6.4.0-RELEASE-ubuntu24.04.tar.gz"
size = 1124757497
[component.artifacts.windows-x86_64]
url = "https://download.swift.org/swift-6.4.0-release/windows10/swift-6.4.0-RELEASE/swift-6.4.0-RELEASE-windows10.exe"
size = 2097914680
installer = "exe"                 # or winget "Swift.Toolchain"
requires = ["vs2022: Windows11SDK.22621, VC.Tools.x86.x64"]

[[component]]
name = "darwin-tools"
version = "1.1.0"                 # 2026-09-21; ld64.lld reports "LLD 21.1.6" [V]
[component.artifacts.linux-x86_64]
url = "https://github.com/xtool-org/darwin-tools-linux-llvm/releases/download/v1.1.0/toolset-x86_64.tar.gz"
size = 46205054
[component.artifacts.linux-aarch64]
url = "https://github.com/xtool-org/darwin-tools-linux-llvm/releases/download/v1.1.0/toolset-aarch64.tar.gz"
size = 44224433

[[component]]
name = "llvm"
version = "23.1.3"                # 2026-10-06
[component.artifacts.windows-x86_64]
url = "https://github.com/llvm/llvm-project/releases/download/llvmorg-23.1.3/clang+llvm-23.1.3-x86_64-pc-windows-msvc.tar.zst"
size = 276320456                  # the MSI is 639551217 bytes; .sig and .jsonl assets exist for each [V]
# Linux: not downloaded by default (1.19 GB .zst / 2.0 GB .xz [V]); use the distro's lld >= 23.1.3 or darwin-tools

[[component]]
name = "xtool"
version = "1.21.0"                # 2026-10-05
[component.artifacts.linux-x86_64]
url = "https://github.com/xtool-org/xtool/releases/download/1.21.0/xtool-x86_64.AppImage"

[[component]]
name = "flutter"
version = "3.47.7"                # 2026-10-08, Dart 3.13.5
[component.artifacts.linux-x86_64]
url = "https://storage.googleapis.com/flutter_infra_release/releases/stable/linux/flutter_linux_3.47.7-stable.tar.xz"
sha256 = "af7455f540df9511…"      # full value from releases_linux.json [V]
[component.artifacts.windows-x86_64]
url = "https://storage.googleapis.com/flutter_infra_release/releases/stable/windows/flutter_windows_3.47.7-stable.zip"
sha256 = "c2c875da25a1a144…"      # releases_windows.json [V]

[[component]]
name = "python-shared"
version = "3.12"
reason = "swift.org LLDB 21 (Swift 6.4) links libpython3.12.so.1.0"   # [V]
source = "build-from-python.org"  # the debug doc decides managed build vs user-provided (open question 4)
```

### 10.3 Compatibility rules

These are evaluated by `resolve_build_env`; each failure has a doctor id.
- The SDK's Xcode version picks `xcode[*]`. Swift's `major.minor` must equal `xcode.swift`; otherwise `sdk.incompatible`, with a hint to install the matching Swift ([R:tc-03]: an older compiler fails with "this SDK is not supported by the compiler").
- The linker must be darwin-tools ≥ 1.1.0, or LLD ≥ `min-lld`; otherwise `toolchain.versionMismatch`.
- Flutter projects: Flutter version = pin, and the `gen_snapshot` Dart revision = the engine's Dart revision. A stale artifact causes "Invalid SDK hash" [R:fx-51].
- Any Rust code compiled **for iOS**, which is not planned, must use Rust ≥ 1.98. Older Rust writes LINKEDIT string pools that iOS 27's dyld rejects [R:sg-08].

### 10.4 Resolution and the env contract

`ToolInfo.env` is part of the contract. Every launcher must apply it: the daemon, the IDE's lldb-dap launcher, and `russet debug --print-config`. On this machine:

```json
{"component":"lldb-dap","version":"21.0.0","path":"/usr/lib/swift/usr/bin/lldb-dap","provenance":"system",
 "env":{"LD_LIBRARY_PATH":"/home/martinv/.local/opt/python-3.12/lib","PYTHONHOME":"/home/martinv/.local/opt/python-3.12"},
 "ok":true,"problems":[]}
```

Without that env, `lldb-dap` fails at load time: `libpython3.12.so.1.0: cannot open shared object file` [V]. Probe results are cached in `cache/toolchain-probes.json`, keyed by path, mtime and size. A file watcher or `refresh` invalidates them.

### 10.5 Installation

> **As implemented (W3, 2026-10-11):** Wave 1 installs `python-shared` and `darwin-tools` only. The archive root
> is component-specific (PBS wraps its files in `python/`; darwin-tools ships `bin/` at the top); entries that
> leave the tree (absolute paths, `..`, links pointing outside) fail with `toolchain.verifyFailed`. Each install
> writes `.russet-install.json`; `RUSSET_OFFLINE=1` refuses non-loopback URLs.

1. **Free space:** `ensure_space(data volume, size × 2.2, min-free-space)`. Compressed plus extracted size, plus slack.
2. **Download** to `cache/downloads/<sha256>.part` with HTTP Range resume. Verify size, then sha256. Then verify the signature where upstream signs: PGP for swift.org (rPGP, with swift.org's keys pinned in the manifest); minisign for Russet-built artifacts.
3. **Extract** to `data/toolchains/.staging-<rand>/`, then rename atomically to `data/toolchains/<component>/<version>/`. There are no symlinks on Windows: selection lives in `data/toolchains/selected.json`, not in a `current` link.
4. **Clean up:** delete the download after a successful extraction, unless `--keep-download` is given. Disk is tight.
5. **Garbage collection:** `toolchain.gc` removes versions that no project pin or default selection references; `--dry-run` lists them first.
6. **Windows Swift** is an `.exe` installer. Russet runs it interactively, after the user agrees, or prints `winget install --id Swift.Toolchain -e`. Russet never installs Visual Studio itself; doctor prints the winget command [S 8].

### 10.6 SDK registry

- **Russet does not copy or move existing SDK bundles**; its own imports go to `data/sdk/<id>/` (Windows: the short root, ARCHITECTURE §15). It records each bundle in `data/sdks/registry.json`:
  - `{id, bundle, xcodeVersion, xcodeBuild, iosSdk, darwinTools, openAppleMacros, importedWith, importedAt, sourceSha256?}`.
  - On this machine: `xcode-27.0-27A266a`, iOS SDK `27.0`, `epoch=3,darwinTools=1.1.0,oam=1.3.0`, bundle size 6.8 GB, and `swift sdk list` → `darwin` [V].
- `sdk.import` is Russet's native importer on `russet-xar` (PLAN W16b; ARCHITECTURE §16.3, [D 69]); it also extracts the DDI. xtool bundles are only read in place. Every import is local. Russet never uploads SDK content, and SDK bytes never reach logs or bug reports.

### 10.7 Linux and Windows differences

- **Linux:** the swift.org distribution is chosen from `/etc/os-release`.
  - `ubuntu 24.04` → `ubuntu2404`; `debian 13` → `debian13`; `fedora` → `fedora41`.
  - Anything else, including Arch and CachyOS → `ubi10` [A]. *landscape* reports that the UBI10 tarball works without sudo, but we have not run it ourselves.
- **Windows:** xtool has no Windows build, which no longer matters: SDK and DDI import are native (W16b). LLVM comes from `clang+llvm-23.1.3-x86_64-pc-windows-msvc.tar.zst`. Paths are made long-path-safe with `dunce` (section 15).

---

## 11. `russet doctor`

> **As implemented (W3, 2026-10-11):** russet-doctor's host checks are `host.os`, `host.disk` (warn below 4× the
> reserve), `host.memory`, `host.wsl`, `dirs.private` (auto fix) and `usbmux.available`; russetd's
> `handlers/doctor.rs` holds `config.global` (also validates the policy files), `config.project`,
> `secrets.backend`, `toolchain.swift`, `toolchain.sdk`, `toolchain.linker` (auto fix: install darwin-tools),
> `toolchain.lldb`/`toolchain.lldb-dap` (auto fix: install python-shared) and `toolchain.xtool` (`skip` when
> absent). `daemon.reachable` belongs to W2's `daemon::checks`. Each check's JSON adds `detail?` and
> `fix?: {kind: auto, applied} | {kind: command, command} | {kind: manual}`; `--category` filters by category.

`russet doctor [--json] [--fix] [--online] [--strict] [--check ID]… [--device D] [--project P]`

When the daemon cannot be reached, doctor runs in-process.

| ID | Checks | Fix |
|---|---|---|
| `host.os` | Linux x86_64/aarch64; Windows 10 1809+ or 11 x64 | — |
| `host.disk` | Free space on data and cache ≥ `min-free-space` | manual: `russet toolchain gc`, `russet sdk remove` |
| `host.memory` | Warn below 8 GiB total | — |
| `host.wsl` | Detects WSL and gives split-mode hints | — |
| `dirs.private` | The five directories exist, are owned by the user and are private | auto |
| `daemon.reachable` | Connects or autostarts; build skew; protocol major | auto (restart if idle) |
| `config.global`, `config.project` | Parse and schema, with file:line errors | manual |
| `secrets.backend` | Keyring reachable and unlocked, or a passphrase configured | manual |
| `usbmux.available` | With the `live` device backend: Linux: `/run/usbmuxd` exists and accepts connections; Windows: AMDS on `127.0.0.1:27015`, or netmuxd. With any other backend (lane D's `scripted:`), only a stat of the socket path and status `skip`, so a development daemon never connects to the real mux (ARCHITECTURE §3.3 item 7, [D 83]) | command: `sudo systemctl enable --now usbmuxd` / install Apple Devices or iTunes (Windows) |
| `usbmux.conflicts` | Another tunnel owner is running: pymobiledevice3 `tunneld`, the go-ios agent | manual |
| `device.*` | Only with `--device` or an attached phone: paired, trusted, Developer Mode, DDI, tunnel (device doc) | auto/manual |
| `toolchain.swift` | Present; minor version matches the SDK | auto (install) |
| `toolchain.sdk` | At least one SDK imported; provenance readable | manual: `russet sdk import <Xcode.xip>` |
| `toolchain.linker` | An `arm64e.x1`-capable ld64.lld is selected. This host shows the system 23.1.1 as *warn*, darwin-tools as *pass* [V] | auto |
| `toolchain.lldb`, `toolchain.lldb-dap` | They run with the resolved env; `remote-ios` is present (strings check [V]) | auto/command |
| `toolchain.xtool` | Version ≥ pin (Linux) | auto |
| `toolchain.flutter`, `toolchain.gen-snapshot` | Flutter projects only | auto |
| `account.default` | Default account configured and its secret available | manual |
| `account.online` (`--online`) | ASC accepts the JWT (`GET /v1/apps?limit=1`) | manual |
| `network.apple` (`--online`) | TSS (`gs.apple.com`) and `api.appstoreconnect.apple.com` reachable | manual |
| `windows.longpaths` | `LongPathsEnabled` | manual |

**Output.** Human mode prints a grouped table. `--json` prints:

```json
{"russet":{"version":"0.1.0","buildId":"…"},"host":{"os":"linux","distro":"cachyos","arch":"x86_64","wsl":false},
 "checks":[{"id":"toolchain.linker","category":"toolchain","title":"Linker for the Xcode 27 SDK","status":"pass",
   "message":"darwin-tools 1.1.0 ld64.lld (LLD 21.1.6, arm64e.x1) from the darwin SDK bundle; system ld64.lld 23.1.1 is too old",
   "hint":null,"fix":null,"durationMs":14}],
 "summary":{"pass":17,"warn":2,"fail":0,"skip":4}}
```

**Exit codes:**
- 0 when there are no failures. With `--strict`, warnings also count as failures.
- 4 when any check fails (code family "environment", section 13.3).
- `--fix` only runs auto fixes that need no root. Root-requiring fixes are printed as commands.

---

## 12. Logging and tracing

- **Library:** `tracing`, with `tracing-subscriber` and `EnvFilter`.
  - Default filter: `info,idevice=warn,jktcp=warn,hyper=warn`.
  - Override with `RUSSET_LOG`, or at runtime with `daemon.setLogFilter`, which uses a reload layer.
- **Daemon:**
  - Daily files `state/logs/russetd.YYYY-MM-DD.log` through `tracing-appender`, keeping `log-retention-days` files.
  - Text format by default; JSON when `RUSSET_LOG_FORMAT=json`.
  - In the foreground it also writes to stderr. Under systemd (`JOURNAL_STREAM` set) it uses `tracing-journald`.
- **CLI:**
  - Diagnostics go to stderr at `-v`, `-vv` or `-vvv`. User output goes to stdout through the `Reporter`, and the two never mix.
  - `--json` prints NDJSON on stdout:
    - `{"type":"progress",…}`
    - `{"type":"log","stream":"stderr","line":"…"}`
    - `{"type":"result","value":{…}}`
    - `{"type":"error","error":{code, russetCode, message, hint}}`
- **MCP bridge:** logs go to stderr and `state/logs/mcp-<pid>.log`. stdout belongs to the protocol.
- **Span fields:** `conn`, `req`, `method`, `job`, `udid`, `session`. Clients may send `_meta.traceId`, which the daemon records, so the IDE, the CLI and the daemon logs can be correlated.
- **Redaction:**
  - Secrets live in `secrecy` types, whose `Debug` prints `***`.
  - A field-name redaction layer masks names matching `(?i)token|password|secret|authorization|cookie|x-apple-i-md(-m)?|anisette|private.?key|p8|p12`.
  - HTTP logging shows the method, host, path, status and duration only. There are never headers or bodies at `info`/`debug`; `RUSSET_LOG_HTTP_BODIES=1` turns them on, still redacted, for development.
  - CI runs a redaction test that feeds known canaries through every log path.
- **Device logs** (console, syslog) are data in StreamHub, not tracing events.
- **`russet bugreport`** writes a zip to the cache directory containing:
  - `doctor --json`;
  - `daemon.status`;
  - the last 3 daemon log files (redacted);
  - config files with secret references only;
  - the toolchain list.

  Nothing is uploaded automatically.

---

## 13. Error model

### 13.1 Types

- Library crates define `thiserror` enums internally and convert them at the API boundary into `russet_core::Error`: a code, a message, an optional hint, optional details, a retryable flag and a source chain. `anyhow` is allowed only in `xtask` and tests.
- **Every user-visible error must have a hint whenever a next step exists.** Reviewers enforce this; there is a clippy-style test over the code table.

### 13.2 Code ranges

| Range | Family | Codes defined by core |
|---|---|---|
| 1000–1099 | `core` | 1000 internal · 1001 cancelled · 1002 timeout · 1003 io · 1004 diskFull · 1005 notImplemented · 1006 invalidArgument · 1007 notFound · 1008 busy · 1009 unsupportedPlatform |
| 1100–1199 | `ipc` | 1100 protocolMismatch · 1101 notInitialized · 1102 daemonUnavailable · 1103 daemonStarting · 1104 frameTooLarge · 1105 streamRejected · 1106 connectionClosed |
| 1200–1299 | `perm` | 1200 denied · 1201 confirmationDeclined · 1202 confirmationUnavailable · 1203 authFailed |
| 1300–1399 | `config` | 1300 invalid · 1301 projectNotFound · 1302 unknownKey · 1303 readOnly |
| 1400–1499 | `secrets` | 1400 unavailable · 1401 locked · 1402 notFound · 1403 passphraseRequired |
| 2000–2199 | `toolchain`, `sdk` | 2000 missing · 2001 versionMismatch · 2002 broken (fails to start, e.g. missing libpython) · 2003 downloadFailed · 2004 verifyFailed · 2005 pinsInvalid · 2100 sdk.missing · 2101 sdk.incompatible · 2102 sdk.importFailed |
| 3000–3299 | `device`, `app` | Device doc. Suggested: 3000 notFound, 3001 notPaired, 3002 locked, 3003 untrusted, 3004 developerModeDisabled, 3005 ddiNotMounted, 3006 tunnelFailed, 3007 disconnected, 3008 usbmuxUnavailable, 3100 app.notInstalled, 3101 app.installFailed, 3102 app.launchFailed |
| 4000–4199 | `build` | Build doc |
| 5000–5299 | `sign`, `account` | Signing doc. Suggested: 5100 account.authFailed, 5101 twoFactorRequired, 5102 rateLimited, 5103 appleServiceUnavailable, 5104 insufficientRole |
| 6000–6099 | `debug` | Run/debug doc |
| 7000–7199 | `ship` | Publishing doc |
| 8000–8299 | `mirror`, `perf`, `xctest` | Respective docs |
| 9000–9099 | `mcp` | IDE/agents doc |

### 13.3 CLI exit codes

| Exit | Meaning | Families |
|---|---|---|
| 0 | Success | — |
| 1 | Unexpected failure | `core.internal`, unknown |
| 2 | Usage error (clap) | — |
| 3 | Not found | `*.notFound` |
| 4 | Environment problem: run `russet doctor` | `config`, `toolchain`, `sdk`, `core.diskFull` |
| 5 | Credentials | `secrets`, `account` |
| 6 | Device state | `device` |
| 7 | Build failed | `build` |
| 8 | Signing or provisioning failed | `sign` |
| 9 | Launch or debug failed | `app.launchFailed`, `debug` |
| 10 | Publishing failed | `ship` |
| 11 | Permission denied or declined | `perm` |
| 12 | Daemon unavailable or version mismatch | `ipc` |
| 130 | Cancelled (SIGINT) | `core.cancelled` |

---

## 14. Async runtime and concurrency

- **tokio** everywhere. idevice and jktcp depend on tokio [V]; rmcp, reqwest and process-wrap all support it.
  - Daemon: multi-threaded, `worker_threads = min(4, cores)`, thread name `russetd-worker`, blocking pool capped at 64.
  - CLI and MCP bridge: `current_thread`.
- **Blocking work** goes through `spawn_blocking`: keyring-core calls (synchronous API [S 6]), signing, zip and tar, and Mach-O parsing. Heavy CPU work, such as hashing a 1 GB download, uses `spawn_blocking` too, with progress reports.
- **Cancellation tree.**
  - The daemon's `shutdown` token is the parent of every job token, and each job token is the parent of its children.
  - `$/cancelRequest` and owner disconnect cancel the job token.
  - A cancelled job has 10 s to clean up. After that its `JoinSet` tasks are aborted and its process trees killed.
- **Backpressure.** Bounded `mpsc` everywhere. jktcp uses unbounded channels internally [V]. Russet bounds that at its edge: forward copy loops use fixed 64 KiB buffers, so the adapter is never flooded faster than the socket drains.
- **Timeouts.**
  - HTTP: 30 s to connect, 120 s per request by default; uploads override.
  - Device requests: the device doc sets per-service defaults.
  - Subprocesses have no timeout but can always be cancelled.
- **Panics.** `panic = "unwind"`. Supervised tasks catch the panic, log it, and restart actors (section 6.2). A panic hook writes `state/crashes/<ts>.txt`, with the build id and backtrace, before unwinding.

---

## 15. Windows and cross-compilation

**Targets:**

| Target | Use |
|---|---|
| `x86_64-unknown-linux-gnu` | Release. Built in an `ubuntu:22.04` container on `ubuntu-24.04` runners, glibc 2.35 baseline [A]; the hosted 22.04 image is deprecated (ARCHITECTURE D 47). No OpenSSL; zbus is pure Rust, so no libdbus. Allowed dynamic deps: glibc, libgcc_s, libm |
| `aarch64-unknown-linux-gnu` | Later |
| `x86_64-pc-windows-msvc` | Release, built on `windows-2025` runners (ring only, so no CMake or NASM; ARCHITECTURE D 21) |
| `aarch64-pc-windows-msvc` | Later (Windows on ARM) |
| `x86_64-pc-windows-gnu` | Developer cross-check from Linux only, not released |

The windows-gnu cross-check was verified here [V]:
- `rustup target add x86_64-pc-windows-gnu` (138 MB) plus mingw-w64 gcc 16.2.0 built the IPC prototype in 5 s.
- The 1.5 MB `.exe` imports only UCRT `api-ms-win-crt-*`, `kernel32`, `ntdll`, `ws2_32`, `userenv` and `bcryptprimitives`.
- It runs under Wine 11.19 as both server and client over a named pipe.
- Wine does not enforce `first_pipe_instance` and cannot validate DACLs, so security tests must run on real Windows.

`cargo xwin` (0.23.1) is an optional MSVC cross-build. It downloads Microsoft's CRT and SDK under their licence, onto the developer's machine; they are never committed.

**CI matrix:**
- `ubuntu-24.04`: tests;
- an `ubuntu:22.04` container on `ubuntu-24.04`: release builds;
- an Arch container: tests against the distro toolchain;
- `windows-2025` (Server 2025 + VS 2026 18.10, Windows 11 SDK 26100): MSVC build, tests and DACL tests, with the pinned 1.99.0 toolchain cached (`profile = "minimal"`; the image ships 1.98.1). Swift-for-Windows work (W27b) and the gen_snapshot Windows build stay on `windows-2022` (VS 2022 17.14, SDKs 22621 and 26100). In the private phase these jobs follow QA §4.2's schedule (decisions.md: free included minutes only);
- optionally, the Wine smoke test.

**Platform-specific code** is limited to these modules; everything else is portable:
- `russet-core::{dirs, fsx, proc, platform}`
- `russet-ipc::transport::{unix, pipe}`
- `russet-secrets::backend`
- `russet-device::transport` (device doc)
- `russet-cli::spawn`

**Windows rules:**
1. Paths:
   - normalize with `dunce` (avoid `\\?\` unless necessary);
   - embed an application manifest with `longPathAware` and `activeCodePage = UTF-8` (`embed-manifest` 1.5.1);
   - doctor warns when `LongPathsEnabled` is off;
   - compare paths case-insensitively;
   - never create symlinks; toolchain selection lives in a JSON file.
2. Subprocesses: `CREATE_NO_WINDOW`, Job Objects, and `which` honouring `PATHEXT`.
3. Console: anstream enables VT processing. Ctrl-C and Ctrl-Break are handled through `tokio::signal::windows`.
4. Named-pipe DACL and server-identity check (section 7.1). Credential Manager's limit is solved by sealing (section 9.6).
5. Device transport through AMDS or netmuxd. Doctor detects them and they are never shipped [R:dv-32].
6. Windows Defender slows extraction of large toolchains. Doctor may suggest a Defender exclusion, but never changes Defender settings.
7. **WSL split mode (M6):** the daemon runs on Windows next to the phone, and the build runs in WSL [R:§5.4]. A WSL client connects through the TCP endpoint, discovered at `/mnt/c/Users/<u>/AppData/Local/Russet/run/tcp.json`. RPC path fields carry a `PathMapping` (`\\wsl$\…` ↔ `/…`, `C:\…` ↔ `/mnt/c/…`) applied by the daemon.

---

## 16. Packaging and distribution

### 16.1 Linux

| Artifact | Tool | Notes |
|---|---|---|
| `russet-<v>-x86_64-unknown-linux-gnu.tar.zst` | dist 0.32.0 (`dist build`) | The portable install, plus a shell installer for `curl \| sh` users |
| `.deb` | cargo-deb 3.8.0 | `Depends: usbmuxd`; `Recommends: avahi-daemon` (Wi-Fi, later) |
| `.rpm` | cargo-generate-rpm 0.21.0 | `Requires: usbmuxd` |
| Arch | `packaging/linux/arch/PKGBUILD` | `russet` (source) and `russet-bin` for the AUR; `depends=('usbmuxd')` |
| IDE AppImage | IDE doc | Bundles the same `russet` binary |
| Flatpak, Snap | Not planned | The sandbox gets in the way of reaching the usbmuxd socket and toolchains |

**Installed files:**
- `/usr/bin/russet`, and the symlink `/usr/bin/russetd → russet`;
- shell completions for bash, zsh and fish (`clap_complete`) and man pages (`clap_mangen`);
- `/usr/lib/systemd/user/russetd.service`, optional and not enabled;
- `/usr/share/licenses/russet/{LICENSE,NOTICE.md,THIRD-PARTY-LICENSES.html}` (from cargo-about).

### 16.2 Windows

| Artifact | Tool | Notes |
|---|---|---|
| `russet-<v>-x86_64-pc-windows-msvc.zip` | dist | Portable. Contains a `russet.portable` marker |
| MSI, per user | dist's MSI (WiX through cargo-wix) | Installs to `%LOCALAPPDATA%\Programs\Russet\bin`, adds it to the user `PATH`, needs no admin [A: per-user WiX template] |
| winget `Russet.Russet` | komac 2.16.0 in the release workflow | dist does not support winget [S 9] |
| Scoop | later | Manifest JSON pointing at the zip |
| Authenticode signing | open question 3 | Unsigned MSIs trigger SmartScreen |

### 16.3 Every release

- `--locked` builds, `SOURCE_DATE_EPOCH` and `cargo auditable build`, which embeds the dependency list;
- a CycloneDX SBOM (cargo-cyclonedx);
- `THIRD-PARTY-LICENSES.html` (cargo-about);
- `SHA256SUMS`, a minisign signature on every artifact, and GitHub artifact attestations;
- the embedded pin manifest;
- `protocol/russet-protocol.schema.json` as a release asset, for third-party clients.

---

## 17. Self-update policy

1. **Package-managed installs never update themselves:** deb, rpm, AUR, winget and MSI.
   - `russet self update` prints the right command (`sudo apt upgrade russet`, `paru -Syu russet-bin`, `winget upgrade Russet.Russet`) and exits with code 4.
   - These builds are made with `RUSSET_PACKAGED=<kind>`, which `build_info::packaged_by()` reports.
2. **Standalone installs** (tarball, zip, dist installers) get `russet self update [--version X] [--channel stable|beta]`:
   1. Fetch the release manifest, then the artifact.
   2. Verify the minisign signature with the embedded public key, then the sha256.
   3. Replace the binary atomically; `self-replace` handles the running `.exe` on Windows.
   4. Keep `russet.old`, so `russet self rollback` works.

   `axoupdater` can be used when dist's install receipt exists.
3. **Update checks.** With `update.check = "notify"`, the **CLI** checks at most once every 7 days, with an anonymous GET to GitHub's latest-release endpoint, and prints one line in human mode. Checks are off in CI, with `--json`, in MCP and in packaged builds. `off` disables them completely. The daemon never phones home. The default is open question 2.
4. **Never under load.** No update runs while the daemon has active jobs or sessions. After an update, the next handshake sees a different build and restarts the daemon once it is idle (section 6.3).
5. **Pin manifest updates** are separate: `russet toolchain pins update`, signed with the same key. They never change projects that pin explicitly.
6. **IDE updates** are owned by the IDE doc; Code-OSS ships with no update server.

---

## 18. Licensing hygiene

- **Workspace licence:** GPL-3.0-or-later. Every source file starts with:

  ```text
  // SPDX-License-Identifier: GPL-3.0-or-later
  // SPDX-FileCopyrightText: 2026 Russet contributors
  ```

  `REUSE.toml` covers generated files, and `reuse lint` runs in CI.
- **cargo-deny 0.20.2** (`deny.toml`):

```toml
[licenses]
version = 2
allow = ["MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
         "ISC", "Zlib", "0BSD", "CC0-1.0", "Unicode-3.0", "BSL-1.0", "MPL-2.0", "GPL-3.0-or-later"]
confidence-threshold = 0.93
exceptions = []           # e.g. LGPL-2.1-or-later or GPL-3.0-only: add only after a written review
[licenses.private]
ignore = true
[bans]
multiple-versions = "warn"
deny = [{ crate = "openssl-sys" }, { crate = "openssl" }, { crate = "native-tls" }]
[sources]
unknown-registry = "deny"
unknown-git = "deny"
[advisories]
version = 2
yanked = "deny"
```

- **Not on the allow-list**, so cargo-deny rejects them:
  - AGPL (xcross ≥ 2.0, StikDebug): reference or separate process only [R:§3.6];
  - GPL-2.0-only: incompatible with GPLv3;
  - SSPL, BUSL, non-commercial licences, and crates with no licence.
- **Notable dependency licences:**
  - apple-codesign is MPL-2.0. MPL §3.3 allows combining it into a GPL "Larger Work" [R:§3.6].
  - notify is CC0-1.0, dunce is CC0/MIT-0/Apache-2.0, ed25519-dalek and zstd are BSD-3-Clause.
  - rmcp is Apache-2.0, which is compatible with GPLv3 [S 10].
- **Crypto provider:** one provider, **aws-lc-rs**.
  - It is rustls 0.23's default [V] and idevice's default feature [V].
  - aws-lc-sys 0.45.0 declares `ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0)`. That expression contains no OpenSSL licence term, so it is GPL-compatible [V].
  - `ring` (Apache-2.0 AND ISC) is the fallback if aws-lc-sys cross-builds become painful.
  - At startup the binary calls `rustls::crypto::aws_lc_rs::default_provider().install_default()`.
- **Upstream-bound crates** may be MIT (`license = "MIT"` in their `Cargo.toml`). They may depend only on permissive crates, which `xtask check-layers` verifies through `cargo metadata`.
- **No Apple files:** `cargo xtask check-no-apple` runs in CI. It fails on tracked files that are:
  - Mach-O (magic `0xfeedfacf`, `0xcafebabe`);
  - BOM/CAR (`BOMStore`);
  - `.tbd`, `.xip`, `.ipsw`, `.mobileprovision`, `.p8`, `.p12`, or `.dmg`.

  The only exception is a fixture listed in `tests/fixtures/ALLOWLIST` with a provenance note saying Russet generated it. This works together with the `.gitignore` rules.
- **Notices:** cargo-about produces `THIRD-PARTY-LICENSES.html` for every package. `NOTICE.md`, covering Apple trademarks and the absence of Apple files, ships with every artifact.

---

## 19. Test plan

| Layer | What | Where |
|---|---|---|
| Unit | Codec (split reads, oversized frames, missing or lower-case headers, extra `Content-Type`), router dispatch, permission decisions, error mapping, config layering, `toml_edit` round trips that keep comments, pin parsing and compatibility evaluation, `RussetDirs` with injected env (Linux and Windows rules), sealed-store round trip, redaction | All OSes |
| Property | Codec with arbitrary chunking (proptest); merge rules for config layering | All |
| Integration | Real daemon in a temp `RUSSET_HOME`:<br>• **autostart race:** 8 concurrent CLIs result in exactly 1 daemon;<br>• idle exit;<br>• **disconnect:** a non-background job is cancelled in under 1 s and its grandchildren die;<br>• **background** (was "detach", [D 65]): the job survives;<br>• **skew:** a test hook fakes an old protocol;<br>• an `mcp` client is denied `publish`;<br>• a confirmation is answered by a fake IDE client | Linux, Windows |
| Interop | Node test that uses vscode-jsonrpc 9.0.3 against the daemon, as the prototype did in section 23 | Linux, Windows |
| Schema | `xtask schema` output committed, with insta snapshots. A diff without a protocol bump fails | Linux |
| Security | Linux: socket 0600, directory 0700, a foreign uid is rejected (container with two users). Windows: read the DACL back with `GetSecurityInfo`; a squatting test where the client refuses a pipe owned by another SID | Linux container; Windows runner |
| Fake hardware | `russet-testkit::FakeDeviceBackend`: scripted attach and detach, log streams, failures. It drives the daemon flows without a phone | All |
| Hardware-in-the-loop | `cargo xtask hil` on the dev box with the iPhone, run only by the agent allowed to touch the device. Gated; not in CI | Dev box |
| Packaging | Install the .deb in Debian 12 and Ubuntu 24.04, the .rpm in Fedora 41, and the PKGBUILD in an Arch container; the MSI on `windows-2025` (`msiexec /qn`). Then `russet --version` and `russet doctor --json` must run without crashing | CI |
| Gates | `cargo fmt --check`, `clippy -D warnings`, `cargo deny check`, `cargo machete`, `reuse lint`, `xtask check-layers`, `xtask check-no-apple` | CI |
| Fuzz (optional) | cargo-fuzz targets for the codec and the config parser | Nightly CI |

---

## 20. Task list (ordered)

Each task lists its size and dependencies. CORE-01 to CORE-07 unblock every other area; do them first.

1. **CORE-01 Workspace skeleton and gates (S).** Create every crate as a stub with a README naming its owning doc. Add `rust-toolchain.toml` (`channel = "stable"`, minimal profile plus clippy and rustfmt; workspace `rust-version = "1.99"`), workspace lints, `deny.toml`, `about.toml`, `REUSE.toml`, the `xtask` skeleton (`check-layers`, `check-no-apple`), and Linux CI (fmt, clippy, nextest, deny). Fix `.gitignore` (`.orchard/` → `.russet/`).
   - *Accept:* `cargo build --workspace` and `cargo deny check` pass in CI. Plain `cargo` works in the repo on the dev box without `RUSSET_TOOLCHAIN`/`RUSTUP_TOOLCHAIN` hacks.
2. **CORE-02 `russet-core` (M; after 01).** Errors and codes with the exit-code mapping, ids, `RussetDirs` (Linux and Windows rules, env overrides), `fsx`, `proc` (process-wrap groups and Job Objects, PDEATHSIG, kill-tree), `progress`, `secret`, `platform`, `build_info`, `task::spawn_supervised`, the `rpc` traits.
   - *Accept:* unit tests pass on Linux and Windows CI. The kill-tree test kills a grandchild on both OSes within 2 s of cancellation.
3. **CORE-03 `russet-protocol` core namespaces (M; after 02).** `rpc_method!` and `rpc_notification!` macros, DTOs for `initialize`, `daemon.*`, `job.*`, `config.*`, `secrets.*`, `toolchain.*`, `sdk.*`, `doctor.*`, `ui.*`, and placeholder modules for the reserved namespaces. `xtask schema` plus TS generation; insta snapshot.
   - *Accept:* the generated schema and `protocol.ts` are committed; CI fails on stale output.
4. **CORE-04 `russet-ipc` engine (M; after 03).** `LspCodec` (16 MiB cap), `Peer` with requests in both directions, `Router`, `$/progress`, `$/cancelRequest` (-32800), `Authorizer` hook, `call_with` options.
   - *Accept:* proptest framing. A Node vscode-jsonrpc 9.0.3 test passes `initialize`, progress, cancel (→ -32800), unknown method (→ -32601) and server-to-client `ui.confirm`.
5. **CORE-05 Transports and auth (M; after 04).**
   - Unix: 0700/0600, uid check on both ends, fallback paths.
   - Windows pipe: DACL, `reject_remote_clients`, `first_pipe_instance`, client-side server-SID check.
   - TCP with token, behind a flag.
   - Byte-stream channel.
   - *Accept:* the security tests from section 19 pass on Linux and Windows CI. Stream throughput on loopback is at least 100 MB/s [A] on the dev box.
6. **CORE-06 `russetd` lifecycle and jobs (L; after 05).**
   - Locks, autostart (`connect_or_spawn`), readiness file, idle exit, graceful shutdown, crash recovery, skew handling.
   - JobManager (queue, locks, limits, cancellation tree, persistence, retention), leases, StreamHub with bounded queues and drop counters.
   - The `daemon.*`, `job.*`, `subscription.cancel` and `stream.close` handlers, and `InProcess`.
   - *Accept:* the integration tests from section 19 pass: autostart race, idle exit, disconnect cancellation, `background` jobs (was "detach", [D 65]), and `job.list` across clients.
7. **CORE-07 `russet-cli` skeleton (M; after 06).** Multi-call dispatch (`russetd` argv0), global flags, Human and JSON reporters, `russet daemon run|start|stop|restart|status|logs`, completions and man pages, area subcommands stubbed with `core.notImplemented`.
   - *Accept:* `russet daemon status --json` validates against the schema; `--no-daemon doctor` works; `russet --help` shows the whole tree.
8. **CORE-08 `russet-config` (M; after 02/03).** Structs and kebab-case serde, layering including `[agents]` narrowing, `ConfigEditor` (toml_edit), JSON Schemas for both files, the watcher and `config.changed`, `config.*` handlers, `russet config get|set|unset|list|path|edit`.
   - *Accept:* round-trip edits keep comments and order; an invalid value reports file:line:col; a project file cannot widen agent permissions (test).
9. **CORE-09 `russet-secrets` (M; after 02).** keyring-core with the zbus Secret Service and Windows stores; the master key; sealed files; the Argon2id passphrase fallback; env mode; `secrets.*` handlers; `russet secrets list|status`; `russet account add asc-key --key-file` sealing (handing over to the signing doc's account service).
   - *Accept:* a 4 KiB secret round-trips on this host (ksecretd), in a gnome-keyring container and on Windows CI (beyond the 2,560-byte credential limit). The headless passphrase mode works. The redaction canary test passes. The secret value never appears in an RPC response (test).
10. **CORE-10 Toolchain probes and resolver (L; after 08).** Manifest schema and embedded default; probes for Swift, the SDK bundle (`version.plist`, `SDKSettings.json`, `darwin-sdk-version.txt`), linker, LLDB and lldb-dap (ELF `NEEDED` → libpython search → env), xtool and Flutter; compatibility rules; `toolchain.list|resolve`, `sdk.list`.
    - *Accept:* on the dev box `russet toolchain list --json` reports:
      - Swift 6.4 at `/usr/bin/swift` (system);
      - SDK `xcode-27.0-27A266a` (darwinTools 1.1.0, OAM 1.3.0);
      - linker = darwin-tools `ld64.lld` (LLD 21.1.6), with the system 23.1.1 flagged too old;
      - `lldb-dap` 21.0.0 with an `env` holding `LD_LIBRARY_PATH`/`PYTHONHOME` at the uv-managed PBS 3.12.15 or Russet's own `python-shared` (never `~/.local/opt/python-3.12`), and `ok: true`.

      Fixture tests cover the mismatch errors.
11. **CORE-11 Toolchain installer (L; after 10).** Resumable download, sha256, PGP (rPGP) and minisign verification, free-space guard, staging and atomic rename, JSON selection (no symlinks), uninstall, GC, `toolchain.install|uninstall|gc|pins.update`, and `xtask pins` (manifest generation from the upstream APIs; it runs in CI, not on dev boxes).
    - *Accept:* installing darwin-tools 1.1.0 (46 MB) into a temp `RUSSET_HOME` works and resolves. Install is refused when free space would drop below the reserve. An interrupted download resumes. A tampered manifest is rejected.
12. **CORE-12 Doctor (M; after 10, 09).** Framework, the core checks from section 11, JSON output, `--fix`, exit codes, registration hooks for area checks.
    - *Accept:* on this box: `toolchain.linker` passes with the darwin-tools note, `toolchain.lldb-dap` passes using the env, `usbmux.available` passes on a `live` daemon and reports `skip` on lane D's scripted backend ([D 83]). `--json` validates against the schema.
13. **CORE-13 Logging and bug report (S; after 06).** Subscriber setup per role, rolling files, JSON format, reload, redaction layer, `daemon.logs.subscribe`, `russet daemon logs -f`, `russet bugreport`.
    - *Accept:* rotation and retention tests pass; the canary redaction test passes; the bug-report zip contains no canary.
14. **CORE-14 MCP bridge skeleton (M; after 06, 03).** `russet mcp serve --stdio` on rmcp 3.5.x; tool registry from `registry()`; progress, cancel and elicitation mapping; secret prompts refused; `--http` with a mandatory token.
    - *Accept:* after `claude mcp add`, Claude Code lists the tools and runs a `doctor` tool. A denied tool returns an MCP error with a hint. An elicitation-capable client can confirm a `project`-class call.
15. **CORE-15 Testkit and CI matrix (M; parallel from 06).** `FakeDeviceBackend`, temp-daemon fixture, Node interop harness, Windows runner, Arch container, optional Wine job.
    - *Accept:* the matrix is green; area teams can write daemon-level tests without hardware.
16. **CORE-16 Linux packaging (M; after 07).** dist config, cargo-deb, cargo-generate-rpm, PKGBUILDs, systemd user unit, completions, man pages, cargo-about, SBOM, minisign, attestations.
    - *Accept:* the container install tests from section 19 pass; `ldd russet` lists only glibc, libgcc_s and libm.
17. **CORE-17 Windows port hardening (L; after 05, 06, 09).** Job Objects, spawn flags, long paths, the manifest, AMDS detection, Credential Manager, a real-PC test run.
    - *Accept:* on the physical Windows PC: CLI autostart, IDE autostart, the daemon surviving an IDE restart, idle exit, DACL check, doctor detecting AMDS.
18. **CORE-18 Windows packaging (M; after 17).** Zip, per-user MSI, komac winget manifest, the signing decision.
    - *Accept:* `winget install --manifest <local>` works on Windows 11; uninstall keeps user data.
19. **CORE-19 Self-update and pin updates (M; after 11, 16).** Receipts and packaged detection, signed manifests, atomic replace, rollback, the notify check.
    - *Accept:* standalone vN → vN+1 → rollback works in a temp install; a tampered artifact is rejected; packaged builds refuse with the right command.
20. **CORE-20 WSL split mode (M; M6; after 17).** TCP listener and token, discovery file, `PathMapping`.
    - *Accept:* a CLI inside WSL2 Ubuntu calls `app.install` on the Windows daemon with a WSL path.
21. **CORE-21 Protocol reference docs (S; after 03).** Generated method tables, the error-code table, examples, and CONTRIBUTING (SPDX, deny, no Apple files).
    - *Accept:* `docs/reference/rpc.md` is regenerated in CI.

---

## 21. Risks

| ID | Risk | Mitigation |
|---|---|---|
| C1 | The daemon is a single point of failure: one panic could drop every session | Supervised actors, `panic = "unwind"`, clients that auto-restart the daemon, crash files |
| C2 | Version skew between the binary bundled with the IDE and the one on PATH | Build ids in the handshake; clients spawn their own executable; restart when idle; protocol rules |
| C3 | Windows Job Object and console behaviour may kill the daemon with the IDE, or leave orphans | Explicit spawn flags, KILL_ON_JOB_CLOSE for children only, a test on the physical PC (CORE-17) [A] |
| C4 | Secret Service missing or locked on headless Linux | Passphrase-sealed fallback, env mode for CI, clear `secrets.*` errors |
| C5 | Windows credential blobs are capped at 2,560 bytes [S 7] | Master key in the keyring, sealed files for everything else |
| C6 | Disk exhaustion: SDK 6.8 GB, Swift 1.1–2.1 GB, LLVM 0.3–2 GB [V] | Free-space guard, no SDK copies, download cleanup, GC, a Linux strategy that avoids the full LLVM tarball (darwin-tools is 46 MB) |
| C7 | Licence drift in dependencies (relicensing, AGPL creeping in) | cargo-deny in CI, review of every new exception, SBOM |
| C8 | Users treat permission profiles as a sandbox | Document them as guardrails (section 7.5); widening only through global config; secret prompts never go to agents |
| C9 | Protocol churn while many agents build in parallel | `russet-protocol` as the single source, reserved namespaces, schema snapshots, CHANGELOG, minor-version discipline |
| C10 | Wine hides Windows security semantics (`first_pipe_instance` not enforced [V]) | Lock file as the primary single-instance mechanism; DACL and squatting tests only on real Windows CI |
| C11 | Churn in rmcp's API (already at 3.x) | Pin the minor version; keep rmcp behind `russet-mcp` |
| C12 | Upstream URL schemes change (swift.org uses `swift-6.4.0-RELEASE` [V]) | Pins generated from upstream APIs, validated in CI (HEAD checks), signed |
| C13 | Another tool opens its own RSD tunnel and breaks ours (iOS 26.2+ [R:dv-34]) | Doctor's `usbmux.conflicts` check; fallback tools go through daemon forwards (device doc) |
| C14 | aws-lc-sys build requirements (cmake, NASM) complicate Windows and cross-builds [A] | CI on native runners; ring as the documented fallback; one provider per binary |
| C15 | Swift's LLDB depends on a libpython that distros may not ship (Arch has 3.14) [V] | The resolver's env contract; a managed shared CPython; llvm.org LLDB fallback (debug doc) |

---

## 22. Open questions

1. **Single binary.** Is one multi-call `russet` binary acceptable to the lead? This doc recommends it. The alternative is separate `russet` and `russetd` binaries plus a build-id check.
2. **Update checks.** Should `update.check` default to `notify` (weekly anonymous GET to GitHub) or `off`?
3. **Windows code signing.** Which provider and budget: SignPath Foundation (free for OSS) or Azure Trusted Signing (paid)? Without signing, users see SmartScreen warnings.
4. **libpython 3.12 for Swift's LLDB** on distros that lack it. Options: a managed CPython build with `--enable-shared` (about 300 MB installed, the same as `~/.local/opt/python-3.12` here [V]), requiring the user to install it, or preferring llvm.org LLDB. The debug doc decides; core supports all three.
5. **Swift's LLDB on Windows** (Swift 6.4): does it need Python, and which version? Verify on the Windows PC.
6. **systemd socket activation** on Linux packages. Proposal: off by default, with clients autostarting the daemon; enabling it is opt-in.
7. **Telemetry:** confirm "none, ever". Only an opt-in, local `russet bugreport`.
8. **glibc 2.35 baseline vs musl static** for Linux release binaries. Proposal: glibc 2.35 now, revisit later.
9. **MCP over HTTP for agents in Docker** (OpenHands): bind to the Docker bridge IP with a token, or require host networking? This is for the IDE/agents doc. Core supports both, with `--allow-nonlocal`.
10. **Wi-Fi devices** (netmuxd or the iOS 27 pair-host): is Avahi a hard or soft dependency in Linux packages? Device doc.

---

## 23. Verified on this machine (2026-10-11)

No device commands were run; the phone is reserved for the device agent.

| # | Command (summary) | Result |
|---|---|---|
| 1 | crates.io API (`/api/v1/crates/<name>`) for about 90 crates | Versions and licences as in section 4.5. Notably:<br>• tokio 1.53.2 (MIT)<br>• rmcp 3.5.1 (Apache-2.0)<br>• idevice 0.1.68 (MIT, edition 2024, default feature `aws-lc`, depends on tokio)<br>• keyring 4.2.0 is a facade over keyring-core 1.0.0 and store crates<br>• aws-lc-sys 0.45.0's licence expression contains no OpenSSL term<br>• rustls 0.23.45 defaults to `aws_lc_rs` |
| 2 | `RUSTUP_TOOLCHAIN=stable rustc --version`; plain `cargo --version` | rustc 1.99.0 (2026-09-28). Without an explicit toolchain: "rustup could not choose a version of cargo to run… no default is configured" |
| 3 | HEAD requests to swift.org | `swift-6.4-RELEASE` paths return 404. `swift-6.4.0-RELEASE` paths return 200:<br>• ubi10 tarball 1,122,271,085 B, plus a `.sig`<br>• ubuntu24.04 1,124,757,497 B<br>• windows10 `.exe` 2,097,914,680 B<br>• windows10-arm64 1,625,343,336 B<br>The swift.org releases API lists 6.4.0 (2026-09-14) for ubi9, ubi10, Ubuntu 22.04/24.04/26.04, Debian 12/13, Fedora 41, Amazon Linux 2023 and Windows 10 |
| 4 | `gh api` GitHub releases | • LLVM 23.1.3 (2026-10-06): Linux-X64 `.tar.zst` 1,188,398,549 B and `.tar.xz` 2,016,524,848 B; win64 MSI 639,551,217 B; woa64 MSI 575,832,219 B; windows-msvc `.tar.zst` 276,320,456 B; `.sig` and `.jsonl` for each<br>• xtool 1.21.0 (2026-10-05): x86_64 and aarch64 AppImages<br>• darwin-tools-linux-llvm v1.1.0 (2026-09-21): toolset-x86_64 46,205,054 B, toolset-aarch64 44,224,433 B<br>• swiftly 1.2.0 (2026-09-22) |
| 5 | Flutter `releases_linux.json` and `releases_windows.json` | 3.47.7 stable (2026-10-08, Dart 3.13.5). Archive paths and sha256 values are in section 10.2 |
| 6 | `readelf -d` on Swift's `lldb`, `lldb-dap`, `liblldb.so.21.0.0`; running them | `liblldb` (232 MB) needs `libpython3.12.so.1.0`. `lldb` and `lldb-dap` fail to load without it. With `LD_LIBRARY_PATH`/`PYTHONHOME` set to `~/.local/opt/python-3.12`, `lldb` 21.0.0 starts. `strings` finds `PlatformRemoteiOS` |
| 7 | Linker versions | System `ld64.lld`: LLD 23.1.1. Swift toolchain `ld64.lld`: LLD 21.0.0 (swiftlang). darwin-tools `orig/ld64.lld`: LLD 21.1.6, behind a shell trampoline that rewrites `-r` to `llvm-lib -static` |
| 8 | SDK bundle inspection | `~/.swiftpm/swift-sdks/darwin.artifactbundle`, 6.8 GB:<br>• `darwin-sdk-version.txt` = `epoch=3,darwinTools=1.1.0,oam=1.3.0`<br>• `version.plist`: 27.0 / 27A266a / 25183.107.5<br>• `SDKSettings.json`: `iphoneos27.0`, Version 27.0, MaximumDeploymentTarget 27.0.99<br>• `toolset.json` → `toolset/bin/ld64.lld`<br>• `swift sdk list` → `darwin` |
| 9 | `ls` on the xtool and ASC files (contents not read) | `~/.config/xtool/data/XTLAuthToken` (369 B) and another 3,265 B file, both mode 0644. The ASC `.p8` is mode 0600, 257 B |
| 10 | `busctl --user list`; `ls -la /run/usbmuxd`; env | `org.freedesktop.secrets` owned by `ksecretd`, plus `kwalletd6` (KDE Plasma, Wayland). `/run/usbmuxd` is `srw-rw-rw- root`. `XDG_RUNTIME_DIR=/run/user/1000` |
| 11 | IPC prototype (scratchpad `ipcproto`): tokio Unix socket, Content-Length framing, Rust client | `initialize`, `$/progress` ×3, cancel → -32800, 2,000 pings with a mean of 15.2 µs. Socket 0600 in a 0700 directory; `peer_cred` reported uid 1000 and the pid |
| 12 | Same server with a Node client on vscode-jsonrpc 9.0.3 (`SocketMessageReader/Writer`) | `initialize` OK; `ProgressType`/`onProgress` received [1,2,3,4]; `CancellationTokenSource` → -32800; unknown method → -32601; ping mean 37.3 µs |
| 13 | `cargo build --target x86_64-pc-windows-gnu` (mingw-w64 gcc 16.2.0), run under Wine 11.19 with a throwaway prefix in tmpfs | 1.5 MB PE32+ binary with UCRT imports. Named-pipe server and client: same results as #11, mean 110 µs. A second server with `first_pipe_instance(true)` also started, so Wine does not enforce the flag |
| 14 | Microsoft Learn: CREDENTIALW; named-pipe security. docs.rs: tokio `ServerOptions`, keyring-core `Entry`/`Error` | Blob ≤ `CRED_MAX_CREDENTIAL_BLOB_SIZE` (5×512); mingw's header still says 512. The default pipe DACL grants read access to Everyone and Anonymous. tokio's `reject_remote_clients` is on by default; `first_pipe_instance` makes later instances fail with `PermissionDenied`; `create_with_security_attributes_raw` exists. keyring-core has `Entry::new(service, user)`, `set_secret`/`get_secret`, and error variants `NoStorageAccess`, `NoEntry`, `NoDefaultStore`, `TooLong` and more |
| 15 | dist docs; swift.org Windows install page; MCP spec (elicitation, progress) | dist supports shell, powershell, npm, homebrew and msi installers, with winget only "requested". Swift on Windows needs VS 2022 + Windows 11 SDK 22621 + VC tools, winget id `Swift.Toolchain`. Elicitation uses `elicitation/create` with actions accept/decline/cancel, and servers "MUST NOT" request sensitive information through it. Progress uses `notifications/progress` with `_meta.progressToken` |

**Cleanup:** the Wine prefix, the prototype build trees and the downloaded JSON were deleted. `rustup target remove x86_64-pc-windows-gnu` freed the 138 MB again. No Apple files were touched or copied.

---

## 24. Sources

1. JSON-RPC 2.0 specification, error codes: https://www.jsonrpc.org/specification
2. LSP 3.17 base protocol (`Content-Length` framing, `$/cancelRequest`, `$/progress`, `RequestCancelled` −32800, `ServerNotInitialized` −32002): https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
3. XDG Base Directory Specification: https://specifications.freedesktop.org/basedir-spec/latest/
4. Microsoft, Named Pipe Security and Access Rights (default DACL): https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
5. tokio `ServerOptions` (`reject_remote_clients`, `first_pipe_instance`, `create_with_security_attributes_raw`): https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html
6. keyring 4.2.0 and keyring-core 1.0.0 (`Entry`, `Error`): https://docs.rs/keyring/4.2.0/keyring/ , https://docs.rs/keyring-core/1.0.0/keyring_core/ ; https://github.com/open-source-cooperative/keyring-core
7. Microsoft, CREDENTIALW (`CRED_MAX_CREDENTIAL_BLOB_SIZE` 5×512): https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw
8. swift.org, Install Swift on Windows: https://www.swift.org/install/windows/
9. dist (cargo-dist) installers: https://axodotdev.github.io/cargo-dist/book/installers/index.html ; https://github.com/axodotdev/cargo-dist
10. GNU licence list, compatibility of Expat, Apache-2.0, MPL-2.0 and LGPL with GPLv3: https://www.gnu.org/licenses/license-list.html (as cited in *landscape* §3.6)
11. MCP specification 2025-06-18, Elicitation: https://modelcontextprotocol.io/specification/2025-06-18/client/elicitation
12. MCP specification 2025-06-18, Progress: https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/progress
13. swift.org releases API: https://www.swift.org/api/v1/install/releases.json ; downloads: https://download.swift.org/swift-6.4.0-release/
14. Crate repositories:
    - rmcp: https://github.com/modelcontextprotocol/rust-sdk
    - idevice: https://github.com/jkcoxson/idevice
    - jktcp: https://github.com/jkcoxson/jktcp
    - process-wrap: https://github.com/watchexec/process-wrap
    - ts-rs: https://github.com/Aleph-Alpha/ts-rs
    - schemars: https://github.com/GREsau/schemars
    - toml_edit: https://github.com/toml-rs/toml
    - axoupdater: https://github.com/axodotdev/axoupdater
    - minisign-verify: https://github.com/jedisct1/rust-minisign-verify
    - apple-codesign: https://github.com/indygreg/apple-platform-rs
    - komac: https://github.com/russellbanks/Komac
15. GitHub releases:
    - https://github.com/llvm/llvm-project/releases/tag/llvmorg-23.1.3
    - https://github.com/xtool-org/xtool/releases/tag/1.21.0
    - https://github.com/xtool-org/darwin-tools-linux-llvm/releases/tag/v1.1.0
16. Flutter release index: https://storage.googleapis.com/flutter_infra_release/releases/releases_linux.json (and `releases_windows.json`)
17. vscode-jsonrpc 9.0.3 (npm, MIT): https://www.npmjs.com/package/vscode-jsonrpc
18. Local sources:
    - `~/.cargo/registry/src/index.crates.io-*/jktcp-0.1.7/src/handle.rs` (`AdapterHandle`)
    - `idevice-0.1.68/src/services/{core_device_proxy,rsd}.rs`
    - `docs/research/landscape-2026-10.md` (§2.4, §2.8, §3.6, §4.3, §5, Appendix A)
    - `docs/verified-facts.md`
