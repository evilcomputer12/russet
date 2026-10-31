# Russet: quality, testing, CI and delivery

> **Lead note (2026-10-11):** where this document conflicts with [docs/ARCHITECTURE.md](../ARCHITECTURE.md), ARCHITECTURE.md wins; §20 there lists every resolved conflict (names, codes, files, crates), and §20.1 lists the superseded fields of this document. Work is scheduled in [docs/PLAN.md](../PLAN.md). Review round 1 (same day) rewrote the CI runner matrix and cost plan (§4.2, §4.8; decisions.md limits private CI to the free included minutes, ARCHITECTURE [D 77]) and the JIT test criteria (E2E-08, [D 71]).

| | |
|---|---|
| Area | Quality, testing, CI, release engineering, docs, contribution, security, legal; whole-product implementation plan |
| Status | Design proposal for the lead. Nothing here is committed. |
| Date | 2026-10-11 |
| Inputs | [decisions.md](../decisions.md) (binding), [verified-facts.md](../verified-facts.md), [demo/m1-baseline.sh](../../demo/m1-baseline.sh), [research/landscape-2026-10.md](../research/landscape-2026-10.md) |

**Markers.**
- **[V#]**: verified hands-on in this session. Section 1 lists them, and Appendix A has the commands.
- **[S#]**: external source; see the source list at the end.
- **[R:xx-nn]**: a fact-checked record of the landscape report.
- **[A]**: assumption, to be confirmed.

**Scope boundaries.**
- This document owns the following:
  - the test strategy and test tooling;
  - CI;
  - release and packaging;
  - the docs site;
  - contribution rules;
  - security policy (secrets, IPC, MCP permissions);
  - the legal release gate;
  - the cross-area implementation plan.
- The other area documents in `docs/design/` own the internal design of each plane. Where this document needs something from another area, it states it as a contract in section 2: a *testability seam* that the area must provide. The name may change; the capability is mandatory.

---

## 0. Summary

1. **Ten testability contracts (section 2) are normative for every crate.** They cover:
   - subprocesses, clock, HTTP endpoints, paths, device connections, secrets, events, error codes, JSON output and determinism.

   They are what make record/replay, cassettes and fakes possible without forking upstream code.
2. **The test pyramid has ten levels (L0–L9).**
   - Levels L0–L6 run on every pull request in GitHub Actions, without an iPhone, an Apple SDK or a Mac.
   - L7 (device lab) runs on the owner's machine with the real iPhone 12 on iOS 27.0, on demand by the device-lane agent (decisions.md: no always-on self-hosted runner).
   - L8 (oracle) runs on Apple hardware only.
3. **The mock device records and replays plaintext service streams.** Recording happens at two levels:
   - the plaintext service-stream level, after TLS and inside the tunnel;
   - the facade-call level.

   Replay runs either in-process, through a replay connector, or out-of-process, through a fake usbmuxd. The fake usbmuxd works for both of our clients, because idevice and pymobiledevice3 both honour `USBMUXD_SOCKET_ADDRESS` [V12]. This needs no idevice fork, because the post-TLS socket is reachable through public API [V10].
4. **Golden tests are semantic.**
   - The public repository holds our own inputs plus *normalized JSON* derived from the oracle's outputs.
   - Raw outputs of Apple tools are never committed and never uploaded as CI artifacts.
   - Byte-level comparison happens only on Apple hardware ("compare-on-oracle").
5. **CI runs on GitHub Actions.**
   - Private phase (GitHub Free; decisions.md: only the free included minutes): Linux on `main` pushes, Windows weekly, macOS only for owner-dispatched Apple-hardware runs; the oracle is the owner's Mac (section 4.2).
   - Public phase: `ubuntu-24.04` and `windows-2025` on every pull request.
   - macOS only on `xcode-27` for Xcode 27 work (never `macos-26`, which has only Xcode 26.x) [V15].
   - Actions are pinned by SHA, and workflows are linted with zizmor.
   - An Apple-file guard scans the tree, the caches and every uploaded artifact.
   - Making the repository public at the M1 gate makes standard runners free, and unlocks attestations, environment approvals and SignPath [V16][V17].
6. **Releases.**
   - Lockstep SemVer `0.x`, with a git-cliff changelog generated from Conventional Commits.
   - CLI packages: tar.gz, zip, .deb, .rpm, MSI, AUR (`russet-bin`, `russet-git`) and winget.
   - IDE packages: AppImage, .deb, .rpm, tar.gz, an Inno Setup installer, MSI and zip, built the way VSCodium builds Code-OSS [V20].
   - Signing: `SHA256SUMS` plus minisign from day one. Sigstore attestations and SignPath Authenticode once the repository is public.
7. **Security.**
   - Secrets live in the OS keychain, with an envelope-encrypted file as fallback, and are never logged.
   - The daemon's IPC is same-user only.
   - MCP tools are split into four risk tiers. Tiers 2 and 3 need an out-of-band, single-use approval bound to the tool and a hash of its arguments.
   - Every tool call goes to an append-only audit log.
   - Device-originated text is wrapped as untrusted.
8. **The legal gate runs per release.** It checks:
   - REUSE/SPDX headers, NOTICE, and third-party notices;
   - the Corresponding Source tarball;
   - nominative trademark use only: no Apple logos, SF Symbols, SF fonts or device artwork, and no Microsoft branding or Microsoft Marketplace in the IDE;
   - that no Apple binaries are included;
   - that project templates and any code injected into user apps carry a permissive licence (0BSD).
9. **The implementation plan has 26 workstreams in 7 phases (section 11).**
   - They are spread over 4 lanes: device (exclusive), oracle (exclusive), heavy-disk (deferred or CI) and general.
   - Gate G1 (M1 working through Russet's own code) is the trigger to make the repository public.

---

## 1. Ground truth verified in this session

| ID | Fact | Consequence |
|---|---|---|
| V1 | rustup 1.29.1 has `stable` installed (rustc/cargo **1.99.0**, 2026-09-28), but **no default toolchain**: plain `cargo` and `rustc` fail with "rustup could not choose a version". Installed components: cargo, clippy, rustfmt, rust-docs, rust-std. Only the `x86_64-unknown-linux-gnu` target is installed. | The repository must ship `rust-toolchain.toml` (QA-01). Agents use `RUSTUP_TOOLCHAIN=stable` until then. |
| V2 | System Node is **26.11.1** and npm is 12.2.0. Code-OSS `.nvmrc` pins **24.18.0**, and the latest VS Code is 1.141.0 (2026-10-07). VS Code 1.141.0 is installed (`/usr/share/code`, AUR `visual-studio-code-bin`). | The IDE build needs pinned Node 24 (fnm or setup-node). Extension tests can reuse the installed VS Code instead of downloading one [V22]. |
| V3 | The Swift 6.4 toolchain at `/usr/lib/swift/usr/bin` contains `lldb`, `lldb-dap` and `lldb-server`; Arch `lldb` is not installed. The Darwin SDK bundle is at `~/.swiftpm/swift-sdks/darwin.artifactbundle`: 6.6 GiB, `epoch=3,darwinTools=1.1.0,oam=1.3.0`. | The lab environment spec. The SDK stays on this machine and never goes to CI. |
| V4 | None of the QA tools are installed, but all of them are in Arch `extra` (about 200 MiB in total; sizes in Appendix A). Packages: `cargo-nextest` 0.9.146, `cargo-deny` 0.20.2, `cargo-about` 0.8.4, `cargo-llvm-cov` 0.9.1, `git-cliff` 2.14.2, `reuse` 6.2.0, `typos` 1.50.3, `zizmor` 1.30.1, `gitleaks` 8.30.1, `mdbook` 0.5.4, `lychee` 0.24.2, `minisign` 0.12, `cargo-insta` 1.49.0, `cargo-fuzz` 0.13.2, `cargo-machete` 0.9.2, `cargo-auditable` 0.7.7, `msitools` 0.106, `xorg-server-xvfb`, `mold` 3.0.0. | One `pacman` command for the owner (QA-01). |
| V5 | crates.io (2026-10-11) gives the version and licence of every crate we plan to use. Tooling: nextest 0.9.148, insta 1.49.0 (Apache-2.0), proptest 1.12.0, cargo-fuzz 0.13.2, cargo-dist 0.32.0 (last release 2026-05-22). Libraries: rmcp 3.5.1 (Apache-2.0), idevice 0.1.68 (MIT), apple-codesign 0.29.0 (MPL-2.0, last release 2024-11-29), plist 1.10.1, object 0.40.0, goblin 0.10.7, keyring 4.2.0, jsonrpsee 0.26.1, schemars 1.2.2, interprocess 2.4.5, rcgen 0.14.10, rustls 0.23.45, zstd 0.14.2 (BSD-3), minisign 0.10.0, wiremock 0.6.5. All of them are GPL-3.0-compatible. | Dependency pins and `deny.toml`. |
| V6 | `evilcomputer12/russet` is **private**, created 2026-10-10, default branch `main`. The `gh` token scopes are `gist, read:org, repo, workflow`; the account plan cannot be read with them. **Re-checked 2026-10-11:** branch protection and rulesets answer 403 "Upgrade to GitHub Pro or make this repository public to enable this feature", so the account is on **GitHub Free**. | Private-phase CI limits apply (V17): no enforceable required checks, 2,000 included minutes, 500 MB of artifacts, no environment secrets. |
| V7 | The Mac oracle (`ssh mac`, 192.168.1.8:22) was **unreachable** on two attempts (connect timeout, about 00:25 CEST). | The oracle tooling needs a GitHub macOS backend and must tolerate a missing Mac. |
| V8 | Free space on `/home`: **15 GiB** at about 00:15, and **98 GiB** from 00:30 CEST (stable when re-sampled). | Disk is volatile, so heavy work is gated by measured free space rather than by a fixed schedule (section 10). |
| V9 | `~/.cache/paru/clone` holds **17 GiB** of AUR build caches for packages that are already installed:<br>• xodus-git 10 GiB<br>• swift-bin 1.9 GiB<br>• claude-desktop-extra 1.3 GiB<br>• visual-studio-code-bin 1.0 GiB<br>• shadps4 0.7 GiB (package not installed)<br>• google-chrome 0.6 GiB<br>• exodus 0.6 GiB<br>• shadps4-qtlauncher-git 0.55 GiB<br><br>The `rust-docs` component takes 719 MiB. | Cleanup candidates. **Nothing was deleted**; see open question 10. |
| V10 | idevice (MIT; master pushed 2026-10-08; crate 0.1.68):<br>• `ReadWrite` is a blanket trait over `AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug`.<br>• `Idevice::new(Box<dyn ReadWrite>, label)` and `Idevice::get_socket()` are public.<br>• `Idevice::start_session` sets up lockdown TLS by *replacing* the socket with a TLS stream.<br>• `RsdService::from_stream(Box<dyn ReadWrite>)` is public, and `RsdProvider::connect_to_service_port` returns plaintext streams that run inside the tunnel. | Recording taps after TLS, and replays in memory, need no fork (section 3.5). |
| V11 | idevice's rustls verifier `NoServerNameVerification::verify_server_cert` accepts **any** server certificate (`sni.rs`). | A mock lockdown can terminate TLS with a synthetic certificate. |
| V12 | Both clients honour `USBMUXD_SOCKET_ADDRESS`:<br>• idevice, through `UsbmuxdAddr::from_env_var()`;<br>• pymobiledevice3 11.26.0, in `usbmux.py`.<br><br>idevice also ships server-side usbmuxd message types (`usbmuxd/server.rs`). | One fake usbmuxd serves both clients, which allows differential tests. |
| V13 | idevice features include `display_stream`, `dvt`, `sysmontap`, `xctest`, `wda`, `crashreportcopymobile`, `debug_proxy` and `installcoordination_proxy`. The default TLS backend is `aws-lc`; `ring` and `openssl` are alternatives. | Feature selection; `ring` avoids the CMake/NASM toolchain on Windows [A]. |
| V14 | pymobiledevice3 11.26.0 has:<br>• CoreDevice HID: `com.apple.coredevice.hid.universalhidservice`, with touch and keyboard reports;<br>• HEVC screen capture and stream, and a VNC server;<br>• `remote/core_device/*.py`. | A reference implementation and differential oracle for the mirror and input tests. |
| V15 | GitHub runner labels (re-checked 2026-10-11):<br>• `ubuntu-24.04`, `ubuntu-26.04` and `-arm` variants; `ubuntu-latest` moves to 26.04 in November 2026; hosted `ubuntu-22.04` is deprecated (from 2026-09-17; support ends 2027-04-17);<br>• `windows-2025` (= `windows-latest`; Server 2025, VS 2026 18.10, Windows 11 SDK 26100, Rust 1.98.1), `windows-2022` (VS 2022 17.14, SDKs 22621 and 26100) and `windows-11-arm`;<br>• `macos-26`/`macos-latest` (arm64; Xcode 26.0.1–26.6 only) and `macos-26-intel`; there is no `macos-27` label;<br>• **`xcode-27`** and `xcode-27-xlarge`: arm64, public preview since 2026-07-16, base macOS 27 since 2026-09-16. Image 20261006.0244.1: macOS 27.0.1 (26A434), default Xcode 27.0 (27A266a) at `/Applications/Xcode_27.app` with the symlink `Xcode_27.0.app`, plus 27.1 and 27.2 beta 2. GitHub now ships one Xcode major per image [S8][S40]. | Runner choice for oracle and Apple-hardware jobs. |
| V16 | Per-minute rates: Linux 2-core $0.006, Windows $0.010, macOS $0.062. Private repositories include 2,000 minutes (Free) or 3,000 (Pro, $4/month). The included minutes are consumed at list price: the explicit 2×/10× multipliers left the docs on 2025-12-16 (github/docs ce33310edb), which gives about 1.67× for Windows and 10.3× for macOS. Larger runners never use included minutes and are never free. Without a payment method, jobs stop at the quota. Public repositories are free on standard runners. Self-hosted runners are free; the 2026 self-hosted fee was postponed [S9][S14]. | Going public at G1 removes most CI cost; until then the plan must fit the included minutes (section 4.8). |
| V17 | Private repositories on Free or Pro get no required reviewers and no wait timers. Environment secrets in private repositories need Pro or higher. Artifact attestations in private repositories need Enterprise Cloud [S11][S13]. | Release gating differs between the private phase and the public phase (section 4.6). |
| V18 | A standard private-repository runner has 2 vCPU, 8 GB RAM and 14 GB SSD (Linux and Windows). Public repositories get 4 vCPU and 16 GB. macOS arm64 runners have 3 vCPU (M1), 7 GB and 14 GB SSD, in public and private repositories alike [S10]. | Code-OSS builds are tight on private runners. |
| V19 | MCP: the current revision is **2026-07-28**. It is stateless, carries per-request `_meta`, and makes `server/discover` mandatory. "Legacy" means `2025-11-25` and earlier. rmcp 3.5.1 knows protocol versions 2024-11-05 through 2026-07-28, with `LATEST_WITH_INITIALIZE = 2025-11-25`. The official conformance runner is `npx @modelcontextprotocol/conformance` [S15–S17]. | Dual-era contract tests (section 3.7). |
| V20 | VSCodium builds Code-OSS on standard GitHub runners. It points `product.json` `extensionsGallery` at Open VSX, and its packages are deb, rpm, AppImage, tar.gz and zsync, plus Windows zip and MSI (WiX v3) [S18]. | A template for WS-19 and QA-20. |
| V21 | Code-OSS has `test/smoke` (Playwright automation) and `test/mcp` ("vscode-automation-mcp") [S19]. | IDE UI smoke tests, which agents can drive. |
| V22 | `@vscode/test-cli` supports `useInstallation: { fromPath }` [S20]. | The extension integration tests need no download. |
| V23 | `microsoft/vscode-copilot-chat` (MIT) has been archived since 2026-05-20 [S36]. | The agent area must re-check how Copilot works in a Code-OSS build (open question 13). |

---

## 2. Testability contracts (normative for all crates)

> **Refined (ARCHITECTURE §5.3, §13, §20.1):** T1, T5, T6 and T8 take the shapes of ARCHITECTURE §5.3; T8's `RUS-<AREA>-<NNNN>` codes are superseded by numeric codes with dotted names [D 7]. Each capability stays mandatory.

The owning area may rename these. Every crate that does I/O must take the abstraction as a parameter or through a context struct, and never call the concrete API directly.

| ID | Contract | Minimal Rust shape (in `russet-core` unless noted) | Test doubles (in `russet-testkit`) |
|---|---|---|---|
| T1 | **Subprocesses** (xtool, swift, lldb, flutter, rcodesign, ...) | `pub trait CommandRunner: Send + Sync { fn run(&self, spec: &CommandSpec) -> BoxFuture<'_, Result<CommandOutput, ExecError>>; fn spawn(&self, spec: &CommandSpec) -> BoxFuture<'_, Result<Box<dyn ChildProcess>, ExecError>>; }`<br>`pub struct CommandSpec { pub program: PathBuf, pub args: Vec<OsString>, pub env: BTreeMap<OsString, OsString>, pub env_clear: bool, pub cwd: Option<PathBuf>, pub stdin: StdinSpec, pub timeout: Option<Duration> }` | `CassetteRunner` (replay), `RecordingRunner`, `ScriptedRunner` |
| T2 | **Time and randomness** | `pub trait Clock: Send + Sync { fn now(&self) -> SystemTime; fn instant(&self) -> Instant; fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>; }` and `pub trait Entropy: Send + Sync { fn fill(&self, buf: &mut [u8]); }` | `ManualClock` (advance on demand), `SeededEntropy` |
| T3 | **HTTP endpoints**: ASC, GSA, TSS, the anisette library CDN, Flutter storage | `pub struct Endpoints { pub asc: Url, pub gsa: Url, pub tss: Url, pub anisette_libs: Url, /* … */ }`, loaded from config and overridable by `RUSSET_ENDPOINTS_FILE`. The HTTP client refuses non-loopback hosts when `RUSSET_OFFLINE=1`. | `AscFake`, `GsaFake`, `TssFake`, `AnisetteFake` (wiremock) |
| T4 | **Paths** | `Paths::from_env()`: XDG directories on Linux, Known Folders on Windows. `RUSSET_HOME=<dir>` roots config, state, cache and runtime under one directory. | `TempHome` guard |
| T5 | **Device I/O** (in `russet-device::io`) | `#[async_trait] pub trait ServiceConnector: Send + Sync + 'static { async fn lockdown_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>, DeviceError>; async fn rsd_service(&self, udid: &Udid, name: &str) -> Result<Box<dyn idevice::ReadWrite>, DeviceError>; async fn watch(&self) -> Result<BoxStream<'static, DeviceEvent>, DeviceError>; async fn call(&self, api: FacadeCall) -> Result<serde_json::Value, DeviceError>; }`<br>Both stream methods return **plaintext** streams: after TLS for lockdown, inside the tunnel for RSD. Services are then built with `Idevice::new(stream, label)` or `RsdService::from_stream(stream)` [V10]. The daemon selects the backend from `RUSSET_DEVICE_BACKEND=live\|mock:<addr>\|replay:<trace>`. | `ReplayConnector`, `RecordingConnector<C>`, `MockDevice` |
| T6 | **Secrets** | `pub trait SecretStore: Send + Sync { fn get(&self, key: &SecretKey) -> Result<Option<SecretBox<[u8]>>, SecretError>; fn put(&self, key: &SecretKey, value: &[u8]) -> Result<(), SecretError>; fn delete(&self, key: &SecretKey) -> Result<(), SecretError>; }` | `MemoryStore` |
| T7 | **Events** | Long operations emit `russet_proto::Event` through `EventSink` (`fn emit(&self, e: Event)`). | `VecSink`, with `assert_events!` helpers |
| T8 | **Error codes** | `#[non_exhaustive] pub enum Code`, with stable strings `RUS-<AREA>-<NNNN>` and a docs anchor each. Tests assert codes, never message text. | A test that enumerates all codes and checks that each is unique and documented |
| T9 | **Machine output** | Every CLI command supports `--json`: one JSON document, or NDJSON for streams. Each has a JSON Schema in `schemas/cli/`. Human output is free-form. | Snapshot tests (insta, trycmd) |
| T10 | **Determinism** | No `HashMap` iteration in outputs (use `BTreeMap` or `IndexMap`). Timestamps come from `Clock`. Builds honour `SOURCE_DATE_EPOCH` and `--remap-path-prefix`, and archives are written in stable order. | "Build twice, compare SHA-256" tests |

---

## 3. Test pyramid

### 3.1 Levels

| Level | Content | Runs where and when | Gate |
|---|---|---|---|
| L0 Static | `cargo fmt --check`; `clippy -D warnings`; `typos`; `reuse lint`; Apple-file guard; `gitleaks`; `zizmor`; `cargo deny check`; `cargo machete`; `eslint` and `tsc --noEmit` | Every PR, plus locally through `cargo xtask ci` | Required |
| L1 Unit | Pure logic in each crate (section 3.2) | Every PR, on Linux and Windows | Required |
| L2 Property and fuzz | proptest inside the L1 budget; cargo-fuzz nightly, which needs a nightly toolchain and therefore runs in CI only | PR (proptest); nightly (fuzz) | Required (proptest); report (fuzz) |
| L3 Golden (semantic) | File formats against normalized oracle expectations (section 3.4) | Every PR | Required for cases marked `required = true` |
| L4 Contract | JSON-RPC, CLI JSON, MCP tool schemas, DAP and event schemas; snapshots; MCP conformance in both eras | Every PR | Required |
| L5 Integration (mock device) | Daemon, CLI and MCP against replay or mock devices, command cassettes and HTTP fakes | Every PR on Linux; a Windows subset | Required |
| L6 Extension and IDE | vitest unit tests; `@vscode/test-cli` integration against a mock daemon; IDE smoke tests once WS-19 exists | PR (unit and integration); nightly (IDE smoke) | Required (L6a); advisory then required (L6b) |
| L7 Device lab E2E | Scenarios on the physical iPhone (section 3.9) | Nightly, on demand, pre-release, milestone gates | Required for releases and gates |
| L8 Oracle | Compare-on-oracle diffs on Apple hardware | On demand, weekly when the corpus changes | Advisory; required for G4 and G5 |
| L9 Release acceptance | Package installs on clean containers and VMs; a manual checklist (TestFlight install, Windows PC) | Each release | Required |

**Budgets** on a 2-vCPU runner: L1+L2 under 3 min, L3+L4 under 2 min, L5 under 5 min.

**Coverage**, reported nightly by `cargo llvm-cov` and advisory for now: at least 85 % line coverage for codecs and parsers, at least 60 % elsewhere.

### 3.2 Unit-test focus by crate

Crate names follow the proposed map in section 11.2.

| Crate | Must be unit-tested (no device, no SDK, no network) | Techniques |
|---|---|---|
| `russet-core` | Config precedence (defaults < file < env < flags); per-OS paths; uniqueness and documentation of error codes; the redaction filter | Tables; `TempHome` |
| `russet-proto` | Serde round-trips of every type; backward compatibility (old payload fixtures still deserialize); schema stability | insta JSON snapshots; fixtures in `crates/russet-proto/tests/compat/` |
| `russet-device` | Service orchestration over `ReplayConnector`:<br>• the DDI image selector (board and iOS version to image), using **synthetic** BuildManifest-shaped plists that we write ourselves;<br>• tunnel and pairing state machines, with `ManualClock`;<br>• mapping lockdown and CoreDevice errors to codes. | Replay traces (section 3.5); proptest for codecs |
| `russet-sdk` | Xcode version detection from a synthetic Xcode layout (our own `version.plist`-shaped fixture, no Apple files); the Xcode-to-Swift lock table [R:tc-03]; provenance records; refusal paths | Tables |
| `russet-build` | Building a plan from `swift package dump-package` JSON of *our* sample packages; argv and env construction; incremental decisions; Info.plist synthesis (canonical plist golden) | `CassetteRunner`; insta |
| `russet-macho` | Parsing, linting and patching synthetic Mach-O files written with `object`'s write API:<br>• LINKEDIT 8-byte alignment fixer [R:sg-08];<br>• `LC_BUILD_VERSION` rewrite;<br>• exec-segment flags [R:sg-07].<br><br>Patching must be idempotent. | proptest (parse(write(x)) == x); fuzz |
| `russet-package` | IPA layout (`Payload/`, symlinks, modes); deterministic order and timestamps; zip64; no external `zip` binary [verified-facts 2026-10-11] | "Build twice" determinism test |
| `russet-sign` | Inside-out signing order; entitlement merging; profile embedding. Profiles are synthetic CMS blobs signed by a **test CA**: the Apple chain is injected as a trust root, never hard-coded. The result is checked with our own verifier and with `apple-codesign`'s verify. | rcgen test PKI |
| `russet-account` | ASC JWT (ES256, `exp` no more than 20 min ahead [R:sg-21]); find-or-create idempotency for certificates, profiles, bundle IDs and devices; pagination; 429 back-off. Free accounts: SRP checked against RFC 5054 test vectors, the GSA flow against `GsaFake`, and the anisette interface against `AnisetteFake`. No Apple libraries are involved. | `AscFake`; `ManualClock` |
| `russet-debug` | LLDB command-script generation (snapshot of the M1 sequence [verified-facts]); gdb-remote packet codec (checksum, escaping, RLE); the JIT-helper state machine against a fake debugserver; DAP mapping | Fake gdb-remote server (testkit) |
| `russet-flutter` | Custom-device JSON; parsing the VM-service URL from logs; Runner template rendering; AOT argv | insta; cassettes |
| `russet-mirror` | Frame pipeline on frames our own app recorded (allowed), or on synthetic frames; pacing; reconnect | Replay of `display_stream` traces |
| `russet-profile` | Decoding recorded sysmon DTX and NSKeyedArchive messages into samples; schema of the Perfetto/Chrome-trace export; parsing `.ips` crash reports and symbolicating them against a synthetic dSYM | Replay; JSON Schema validation |
| `russet-ship` | Every lint rule (section 3.4.6) with a positive and a negative fixture; the upload state machine against `AscFake`; the dry-run guarantee (zero mutating HTTP requests) | `AscFake` request log |
| `russet-mcp` | Tool order and schema validity; consistency between tier and annotations; approval-token binding; audit-log format; output sanitization | Section 8.4 cases |
| `russet-daemon` / `russet-cli` | Argument parsing; exit codes; `--json` schemas; daemon lifecycle (single instance, restart on version mismatch); IPC peer check (`SO_PEERCRED` on Linux, a pipe ACL for the user SID on Windows) | `trycmd`, `assert_cmd` |
| `ide/extension` (TypeScript) | View models; when commands are enabled; JSON-RPC client reconnects; validation of webview messages | vitest |

### 3.3 Property tests and fuzzing

- **proptest** round-trips (`decode(encode(x)) == x`) for:
  - the plist codec wrapper;
  - usbmux packets;
  - lockdown framing;
  - XPC objects and the HTTP/2 frames we touch;
  - DTX headers and auxiliary dictionaries;
  - gdb-remote packets;
  - Mach-O load commands we write;
  - RDT records.
- **cargo-fuzz targets** (CI nightly, 3 minutes each, with a corpus cached as a CI artifact that contains *only fuzzer-generated or our own* inputs):
  1. `fuzz_macho_parse`
  2. `fuzz_plist_any`
  3. `fuzz_usbmux_packet`
  4. `fuzz_xpc_decode`
  5. `fuzz_dtx_decode`
  6. `fuzz_nskeyed_decode`
  7. `fuzz_gdbremote_packet`
  8. `fuzz_car_parse`
  9. `fuzz_nib_parse`
  10. `fuzz_ips_crash_parse`
  11. `fuzz_rdt_reader`
  12. `fuzz_mcp_args` (schema-validated tool arguments)
- **Rationale.** Everything a device, a server or a project file sends us is untrusted input. That covers a malicious app's logs, crafted project files, and responses from a compromised network path.
- **Crash triage.** Every fuzz crash becomes a regression unit test before it is fixed.

### 3.4 Golden tests for file formats (CAR, NIB, plists, Mach-O metadata)

#### 3.4.1 Principles

1. **Inputs are ours.** Corpus inputs are asset catalogs, storyboards, xcstrings and projects that we write. Images are rendered from our own SVGs by a script. No Apple sample code and no SDK files are used.
2. **Only normalized expectations are public.** The repository stores `semantic.json`, a normalized and order-insensitive dump, together with `oracle.json`, which records:
   - the tool and its build version;
   - the macOS version;
   - the exact argv;
   - SHA-256 hashes of the raw outputs;
   - a determinism flag.

   Raw Apple-tool outputs (`.car`, compiled `.nib`, `.stringsdict`, binaries built by Xcode) **are never committed and never uploaded as CI artifacts**. They exist only on the owner's Mac or Linux machine, under `~/.cache/russet-oracle/` (gitignored), or on an ephemeral macOS runner. This is the conservative reading of the "never ship Apple files" rule, and it follows the clean-room split of research section 6.1 (open question 5).
3. **Semantic equality is the gate; byte equality is a statistic.**
   - Linux CI compares our compiler's normalized output with `semantic.json`.
   - Byte-level diffs run only in compare-on-oracle mode on Apple hardware, where both outputs exist side by side.
4. **Runtime truth beats file equality.** The on-device probe app (E2E-15) loads every asset, nib and string by name from *our* compiled outputs. Required for G4.
5. **Determinism.** The oracle runs each Apple tool twice and records whether the outputs are byte-identical. Our own tools must be byte-deterministic for identical inputs and `SOURCE_DATE_EPOCH` (T10).

#### 3.4.2 Corpus layout

```text
corpus/
  README.md                         # rules above; how to add a case
  case.schema.json                  # JSON Schema for case.toml (validated in CI)
  KNOWN_DIFFERENCES.md              # accepted semantic deltas, each with reason + issue link
  actool/<case-id>/
    case.toml
    input/Assets.xcassets/...       # ours; PNGs generated by corpus/tools/render.py from SVG
    expected/<xcode-build>/semantic.json   # e.g. expected/27A266a/semantic.json
    expected/<xcode-build>/oracle.json
  ibtool/<case-id>/...              # storyboard/xib inputs we wrote; launch-screen subset first
  xcstrings/<case-id>/...
  plist/<case-id>/...               # Info.plist processing (build-setting expansion, DT* keys)
  macho/<case-id>/...               # inputs = sample project; expected = load-command dump of the Xcode build
  pif/<case-id>/...                 # (M7) xcodebuild -dumpPIF of our sample projects
```

**`case.toml` example.** The argv comes from Swift Build's open xcspecs [R:xt-01][R:xt-03] and must be re-confirmed against `AssetCatalogCompiler.swift` in WS-15 [A].

```toml
id          = "actool/appicon-single-1024"
tool        = "actool"
required    = true                 # gate in L3 once our compiler claims support
min_xcode   = "27.0"
oracle_argv = ["xcrun", "actool", "--compile", "{out}", "--platform", "iphoneos",
               "--minimum-deployment-target", "17.0", "--app-icon", "AppIcon",
               "--output-partial-info-plist", "{out}/partial.plist",
               "--target-device", "iphone", "--output-format", "xml1", "{in}/Assets.xcassets"]
russet_argv = ["russet-actool", "--compile", "{out}", "--platform", "iphoneos",
               "--minimum-deployment-target", "17.0", "--app-icon", "AppIcon",
               "--output-partial-info-plist", "{out}/partial.plist",
               "--target-device", "iphone", "--output-format", "xml1", "{in}/Assets.xcassets"]
normalize   = [{ kind = "car", file = "Assets.car" }, { kind = "plist", file = "partial.plist" }]
masks       = ["car.header.uuid", "car.header.timestamp", "car.renditions[*].compression"]
```

#### 3.4.3 Normalized formats (`russet-golden` normalizers)

| Kind | Schema ID | Content (summary) |
|---|---|---|
| `plist` | `russet.plist-canonical/1` | JSON with explicit types (`{"$type":"date","v":…}`, `{"$type":"data","sha256":…,"len":…}`, `{"$type":"real","v":…}`); keys sorted. Masks cover volatile keys (`BuildMachineOSBuild`, `DTXcodeBuild` when it is not under test). |
| `macho` | `russet.macho-meta/1` | Header (cputype, subtype, filetype, flags); load commands in order. `LC_BUILD_VERSION` (platform, minos, sdk, tools[]), `LC_SOURCE_VERSION`, `LC_UUID` (masked), `LC_DYLD_CHAINED_FIXUPS` presence, `LC_ENCRYPTION_INFO_64`, dylib list. Segment names and protections; LINKEDIT string-pool alignment. Code signature: CodeDirectory version, flags (including `CS_EXECSEG_MAIN_BINARY`), hash type, page size, special slots, identifier, team ID, entitlements (XML and DER, parsed). |
| `car` | `russet.car-semantic/1` | CAR and BOM header versions; key format; facets → renditions (idiom, scale, appearance, subtype, size, layout, pixel format, `pixels_sha256` of **decoded RGBA8**); colours (colour space, components); data assets (UTI, SHA-256); app-icon metadata. Compression and ordering are masked by default. |
| `nib` | `russet.nib-semantic/1` | Object graph with stable IDs: class names, keys, typed values, connections. Sets are order-insensitive. |
| `xcstrings` | `russet.strings-semantic/1` | Compiled `.strings`/`.stringsdict` per locale, as key → plural variants. |
| `pif` (M7) | `russet.pif-semantic/1` | PIF objects with GUIDs replaced by path-derived IDs. |

#### 3.4.4 Tools and APIs

```rust
// crates/russet-golden (dev tool, not shipped)
pub trait Normalizer: Send + Sync {
    fn kind(&self) -> &'static str;                                  // "car", "plist", ...
    fn normalize(&self, file: &Path) -> Result<serde_json::Value, NormalizeError>;
}
pub struct MaskSet(Vec<JsonPathMask>);
pub struct CompareReport { pub case_id: String, pub equivalent: bool, pub diffs: Vec<Diff>, pub masked: usize }
pub fn compare(expected: &Value, actual: &Value, masks: &MaskSet) -> CompareReport;
pub fn load_case(dir: &Path) -> Result<CaseManifest, CaseError>;
```

```sh
russet-golden normalize --kind car path/to/Assets.car > semantic.json
russet-golden check corpus/ [--case 'actool/*'] [--require-only]   # runs russet_argv, compares; L3
russet-oracle run --backend ssh|gha --case 'actool/*' [--compare-on-oracle]
russet-oracle status                                                 # backend reachability, Xcode build
cargo xtask golden update --from-oracle <run-dir>                    # writes expected/<build>/*.json after review
```

**Error cases** (`russet-oracle` exit codes):

| Error | Code / behaviour |
|---|---|
| `OracleUnavailable` (SSH timeout, as observed in V7) | Exit 3. The run falls back to `--backend gha` only when `--fallback gha` is passed. |
| `XcodeMismatch` (expected Xcode build differs from the backend's) | Stored under a new `expected/<build>/`; never overwrites. |
| `NonDeterministicOracle` (two runs differ) | The case is marked `oracle_deterministic = false`; byte comparison is skipped. |
| `RawOutputEscape` (the guard finds raw Apple output in the upload directory) | The job fails. |

#### 3.4.5 Oracle backends

| Backend | How | When | Constraints |
|---|---|---|---|
| `ssh` (the owner's Mac) | `ssh mac 'DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun <tool> …'` in a fresh `mktemp -d`, which is deleted afterwards. Raw outputs are copied to the owner's Linux `~/.cache/russet-oracle/` for normalization. | Default when the Mac is reachable | Single user. Never a shared network service [R:xr-27]. The Mac may install CLI tools (decisions.md), but SIP and system settings are never changed. Light load (8 GB RAM). |
| `gha` (GitHub macOS runner) | `oracle.yml`, triggered by `workflow_dispatch` on `xcode-27` only, with `DEVELOPER_DIR=/Applications/Xcode_27.0.app/Contents/Developer` and an assertion of build `27A266a`. The image has the same macOS (26A434) and Xcode (27A266a) builds as the Mac, so both backends share `expected/27A266a/`. The job builds `russet-golden` for macOS, runs the Apple tool and our compiler, normalizes, and uploads **only** `semantic.json`, `oracle.json` and `compare.json` after the guard passes. | Once the repository is public (free), or per run with the owner's OK; in the private phase the Mac is the oracle (decisions.md) | In the private phase each minute consumes about 10.3 included Linux minutes [V16]. There is no `macos-26` fallback: it has only Xcode 26.x and would produce a different `expected/<build>/` |

#### 3.4.6 Packaging and Mach-O lint corpus

QA owns the fixtures; the ship area owns the rules.

| Rule | Check | Source |
|---|---|---|
| ML-001 | LINKEDIT string pool is 8-byte aligned (iOS 27 dyld) | [R:sg-08] |
| ML-002 | `LC_BUILD_VERSION` platform is iOS, and `minos` is at least 13.0 for App Store builds | [R:sg-29] |
| ML-003 | `CS_EXECSEG_MAIN_BINARY` is set on the main executable and on extensions | [R:sg-07] |
| ML-004 | No arm64e slices (unsupported) | [R:fc-tc] |
| ML-005 | No loose dylibs; they must be wrapped as frameworks (ITMS-90426) | [R:sg-30] |
| ML-006 | Extension `CFBundleVersion` and short version match the app (ITMS-90473) | [R:sg-30] |
| ML-007 | `UIRequiredDeviceCapabilities` contains `arm64` (ITMS-90502) | [R:sg-30] |
| ML-008 | App Store builds have `CFBundleIconName`, an `Assets.car` containing AppIcon, and a 1024 px icon (ITMS-90713) | [R:xt-10] |
| ML-009 | `UIDeviceFamily` and the `DT*` group are present (iOS 26+ registers such apps) | [R:dv-26] |
| ML-010 | Flutter debug builds have `NSBonjourServices` and `NSLocalNetworkUsageDescription` | [R:fx-14] |

Each rule has a positive and a negative fixture built from synthetic Mach-O files and plists. The P0-3 diff (an Xcode-archived vs a Linux-built IPA of the same app [R:P0-3]) runs as an L8 case whose semantic diff feeds `KNOWN_DIFFERENCES.md`.

### 3.5 Mock device: record and replay

#### 3.5.1 Architecture

```text
                    live                                   replay / mock
russet-device ──► ServiceConnector (T5) ──┬─► LiveConnector (idevice: usbmux, lockdown+TLS, RSD tunnel)
                                          │      └─ RecordingConnector<Live>: wraps returned plaintext
                                          │         streams in TapStream ──► RDT trace file
                                          ├─► ReplayConnector (in-process; reads RDT; no sockets)
                                          └─► LiveConnector pointed at MockDevice (out-of-process):
                                                 fake usbmuxd (USBMUXD_SOCKET_ADDRESS) + lockdown with
                                                 TLS (synthetic pair record) + scripted/replayed services
```

**Recording live traffic, without forking idevice [V10]:**

- **Lockdown services.** Connect as usual (lockdown, `StartSession`, `StartService`, TLS). Then:
  1. call `idevice.get_socket()`, which returns the post-TLS stream;
  2. wrap that stream in `TapStream`;
  3. call `Idevice::new(tapped, label)`;
  4. call `Service::from_stream(...)`.
- **RSD services.** Wrap the `Box<dyn ReadWrite>` returned by `RsdProvider::connect_to_service_port`. It is already plaintext inside the tunnel. Then call `RsdService::from_stream`.
- **Facade calls.** Calls such as lockdown `GetValue`, pairing state and DDI status are recorded as `call` records with JSON results.
- **Not recorded:** the lockdown TLS handshake itself and tunnel packets. They are covered by idevice's own tests and by L7.

#### 3.5.2 RDT trace format v1

The Russet Device Trace is a JSON Lines file compressed with zstd: `*.rdt.jsonl.zst`.

```jsonc
{"rdt":1,"type":"header","scenario":"install-launch","recorded_at":"2026-10-11T01:02:03Z",
 "russet":"0.1.0+g1a2b3c","idevice":"0.1.68","host_os":"linux",
 "device":{"product_type":"iPhone13,2","product_version":"27.0","build_version":"24A437","csm":"txm",
           "udid":"00000000-RUSSETFAKE000001"},"redaction":{"policy":"default-v1"}}
{"type":"conn","id":3,"t":0.512,"via":"rsd","service":"com.apple.coredevice.appservice","open":true}
{"type":"data","conn":3,"t":0.513,"dir":"c2d","b64":"…"}
{"type":"data","conn":3,"t":0.540,"dir":"d2c","b64":"…"}
{"type":"note","conn":3,"t":0.541,"proto":"xpc","decoded":{…}}     // optional, for human review/diffs
{"type":"call","t":0.600,"api":"lockdown.get_value","args":{"key":"ProductVersion"},"result":{"ok":"27.0"}}
{"type":"elided","conn":5,"t":2.0,"dir":"c2d","len":123456789,"sha256":"…","reason":"apple-file:ddi"}
{"type":"conn","id":3,"t":1.200,"open":false}
```

- **File naming:** `crates/russet-testkit/traces/<scenario>/<ios>-<build>/<product-type-with-dash>.rdt.jsonl.zst`. Example: `traces/install-launch/27.0-24A437/iPhone13-2.rdt.jsonl.zst`.
- **Code-signing monitor.** The header's `device.csm` is `txm | ppl | none | unknown` (ARCHITECTURE §9.5). Rig recordings are TXM traces (the iPhone 12 runs TXM on iOS 27.0). Replays check protocol handling; host test H8 and device check DC-30 check JIT page coverage. Scenario: `traces/flutter-jit/27.0-24A437/iPhone13-2.rdt.jsonl.zst`.
- **Budgets:** at most 2 MiB per trace and 50 MiB for all traces in the repository. Screen-stream traces keep at most 30 frames of *our* probe app.
- **Elision.** The recorder never stores Apple-file payloads. They are replaced by `elided` records holding a length and a hash. This covers:
  - DDI and cryptex uploads;
  - symbol and shared-cache transfers (`fetch-symbols`);
  - firmware and TSS blobs;
  - screenshots of anything other than our probe app.
- **Pairing records are never recorded.** Replays and mocks use a synthetic pair record generated with rcgen (test PKI under `crates/russet-testkit/pki/`).

#### 3.5.3 Redaction

**Policy `default-v1`.**

- **What is redacted:**
  - `UniqueDeviceID` and UDIDs anywhere;
  - `SerialNumber`, `UniqueChipID`/ECID;
  - `WiFiAddress`, `BluetoothAddress`, `EthernetAddress`;
  - `DeviceName`, `PhoneNumber`, IMEI/MEID;
  - `HostID`, `SystemBUID`;
  - certificates (replaced by synthetic ones);
  - team IDs (mapped to `TEAMID0001`);
  - Apple ID emails;
  - container UUIDs in paths;
  - bundle IDs of apps that are not ours (mapped to `com.example.app<N>`).
- **How replacements are made.** Each replacement is an HMAC-SHA-256 of the original value, keyed with a local salt in `~/.config/russet/redaction-salt` (mode 0600). This keeps pseudonyms consistent within a trace. The salt and the mapping never leave the machine.
- **Trace lint.** `cargo xtask guard traces` fails on any of the following:
  - UDID-shaped strings (40 hex characters, or the `00008xxx-…` form) outside the fake allowlist;
  - MAC-address patterns;
  - email addresses;
  - PEM or DER private keys;
  - Mach-O, IMG4, DMG (`koly`), `BOMStore` or xar magic inside data payloads;
  - any `data` record larger than 1 MiB.
- **Owner denylist.** The owner may also keep a local denylist (`~/.config/russet/private-identifiers`) with the real UDID, device name and Apple ID. A pre-commit hook and the device-lane agent check against it.

#### 3.5.4 Replay modes and errors

```rust
pub enum ReplayMode {
    Strict,    // client bytes must equal recorded bytes after protocol masks
    Semantic,  // decode both sides with the protocol codec (plist/XPC/DTX/gdb-remote), compare with masks
    Lenient,   // check framing only; serve recorded device bytes in order
}
pub struct ReplayConnector { /* implements russet_device::io::ServiceConnector */ }
impl ReplayConnector {
    pub fn open(trace: &Path, mode: ReplayMode) -> Result<Self, ReplayError>;
    pub fn assert_consumed(&self) -> Result<(), ReplayError>;
}
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("conn {conn}: unexpected client bytes at offset {offset}")] UnexpectedClientBytes { conn: u32, offset: u64, expected: String, actual: String },
    #[error("no recorded connection for service {0}")] UnknownService(String),
    #[error("{0} recorded connections were never opened")] Unconsumed(usize),
    #[error("trace format: {0}")] Format(String),
}
```

**Protocol masks** live in `crates/russet-testkit/masks/*.toml`:

- XPC message IDs and the iOS 27.2 message-ID parity [R:dv-34];
- DTX message identifier and conversation index;
- UUIDs and timestamps;
- gdb-remote thread IDs and addresses (ASLR).

#### 3.5.5 Out-of-process mock device (`russet-mockdevice` binary in `russet-testkit`)

- **Fake usbmuxd.** It listens on a Unix socket on Linux and on `127.0.0.1:<random>` on Windows. Clients find it through `USBMUXD_SOCKET_ADDRESS`, which both idevice and pymobiledevice3 honour [V12]. It implements `ListDevices`, `Listen`, `ReadBUID`, `ReadPairRecord` (synthetic) and `Connect`, using idevice's server-side message types.
- **Lockdown on port 62078.** It handles `QueryType`, `GetValue`, `StartSession` and `StartService`, and terminates TLS with a synthetic device certificate. This is accepted because idevice's verifier skips certificate checks [V11].
- **Scripted services**, written in Rust and in-memory:
  - `installation_proxy`: browse, install and uninstall;
  - `afc`;
  - `syslog_relay`;
  - `mobile_image_mounter`: always reports "mounted";
  - `crashreportcopymobile`: serves synthetic `.ips` files of the probe app;
  - a gdb-remote debugserver with scripted stops;
  - DVT stubs for process list, sysmon samples and screenshot frames.
- **Replayed services:** any service that has an RDT trace.
- **RSD.** There is no packet-level fake of the userspace tunnel. In out-of-process mode, the daemon uses `RUSSET_DEVICE_BACKEND=mock:<addr>`, which reaches RSD services over a simple framed side channel of the mock. This is documented as a known gap that L7 covers.
- **Scenario spec** (`crates/russet-testkit/scenarios/*.toml`): device properties, installed apps, log-line scripts, crash files, frame sources and breakpoint stops.
- **Differential test (QA-08).** On Linux, `pymobiledevice3 lockdown info` and `russet device info --json` run against the same mock. They must report identical `ProductVersion`, `ProductType` and `DeviceName`. This keeps the mock honest.

#### 3.5.6 Drift detection

- **Re-recording.** Every lab run can re-record its traces (`cargo xtask lab --record`).
- **Comparison.** `cargo xtask traces diff` compares the new semantic `note` layer with the committed traces. Differences in message shapes, keys or service names after an iOS update (27.1, 27.2, ...) open a "protocol drift" report instead of silently breaking replays.
- **Who commits.** Only the device-lane agent or the owner commits re-recorded traces, after the guard has passed.

### 3.6 Other fakes

| Fake | Covers | Notes |
|---|---|---|
| `CassetteRunner` | xtool, swift, lldb, flutter, rcodesign subprocesses | Cassettes (`*.cassette.json`) store argv, the env allowlist, cwd, exit code, stdout and stderr. Each is recorded from a real run on *our* samples, then has paths normalized to `${HOME}`, `${SDK}` and `${PROJECT}`. |
| `AscFake` | App Store Connect API | In-memory state for certificates, profiles, `bundleIds`, devices, apps, `buildUploads` and `buildUploadFiles` with presigned parts. It records every request so that tests can assert "zero mutating calls in dry-run" and "no new certificate on the second run". |
| `GsaFake`, `AnisetteFake` | Free Apple ID flow | Uses RFC 5054 SRP vectors. **No Apple Android libraries are downloaded or executed in CI.** The real anisette path runs only in the manual test E2E-17. |
| `TssFake` | DDI personalization | Returns a synthetic ticket; mounting is tested only in L7. |
| `ManualClock`, `SeededEntropy` | Timeouts, retries, token expiry (ASC 20-minute JWT, approvals) | |

**Network policy.** L1–L6 run with `RUSSET_OFFLINE=1`; the HTTP client refuses non-loopback hosts (T3). CI also checks this: a run with `strace -f -e trace=connect`, or the nextest `--no-capture` log, must not show external connections [A: the exact enforcement method is chosen in QA-09].

### 3.7 Contract tests

| Contract | Test |
|---|---|
| JSON-RPC daemon API (`russet-proto`) | Schemas are generated with schemars into `schemas/rpc/v<major>/` and checked with `cargo xtask schemas --check`. Compatibility fixtures of older payloads must still deserialize. The `rpc.hello` result (`{api:{major,minor}, features:[…], russet_version}`) is snapshot-tested. |
| CLI `--json` | A JSON Schema per command in `schemas/cli/`. `trycmd` cases validate output against the schemas. |
| Events | `schemas/events/`. Every `Event` variant has an example, validated against the schema. |
| MCP tools | `russet mcp tools --json` is snapshot-tested. The checks are:<br>• names match `^[a-zA-Z0-9_-]{1,64}$` [A: the strictest common client constraint];<br>• `inputSchema` is valid draft 2020-12 with `additionalProperties: false`;<br>• `outputSchema` exists for structured tools;<br>• annotations are consistent with the tier (section 8.4);<br>• order is deterministic [S15]. |
| MCP protocol | `npx @modelcontextprotocol/conformance server --url http://127.0.0.1:<port>/mcp` against `russet mcp serve --http 127.0.0.1:0 --token-file …` [S17]. Run in both eras: 2026-07-28 (`server/discover`, per-request `_meta`) and legacy `initialize` (2025-11-25) [S15][V19]. A stdio smoke test uses an rmcp client. |
| DAP | A scripted DAP client runs the M1 sequence against the Russet debug adapter (or lldb-dap) and a fake debugserver: `initialize` → `attach` → `setBreakpoints` → `configurationDone` → `stopped` → `stackTrace` → `variables` → `continue` → `disconnect`. |
| Error codes | All `RUS-*` codes are unique and each has a docs anchor (T8). |

### 3.8 Extension and IDE tests

- **Unit (L6a):**
  - `ide/extension` uses vitest with no Electron.
  - Covers the device-tree view model, button enablement per project type, the log-panel ring buffer, the mirror webview protocol (message validation and frame pacing), and the plist and entitlements editor models.
- **Integration (L6a):**
  - `@vscode/test-cli` with `useInstallation.fromPath: "/usr/share/code/code"` locally [V22]; in CI it downloads a pinned VS Code.
  - Runs under `xvfb-run -a`.
  - The extension connects to the **mock daemon**: `russet daemon --device-backend mock:<addr>`, with MockDevice.
  - Assertions:
    - the device list shows the fake iPhone;
    - Run produces log lines in the panel;
    - Debug produces a `stopped` event in the debug session (fake debugserver);
    - the mirror panel receives frames.
- **IDE smoke (L6b, after WS-19):**
  - Uses Code-OSS `test/smoke` and `test/automation` (Playwright, MIT) against the Russet IDE build, with Russet-specific steps:
    - the toolbar has the Build, Run, Debug and Hot Reload buttons;
    - the Devices view is populated;
    - the About dialog shows the Russet version and the Code-OSS base version;
    - the extension gallery URL is Open VSX;
    - telemetry is off.
  - Agents may also drive the IDE through Code-OSS's `test/mcp` automation server [V21].
- **Product checks (in QA-20):**
  - `product.json` must not contain "Visual Studio Code" or `marketplace.visualstudio.com`.
  - `extensionsGallery.serviceUrl` must be `https://open-vsx.org/vscode/gallery` [S18].
  - No Microsoft-proprietary extension may be bundled.

### 3.9 Device-in-the-loop E2E (L7)

#### 3.9.1 Lab topology and rules

```text
owner's CachyOS PC ── USB ── iPhone 12 (iPhone13,2), iOS 27.0 (24A437; TXM), paired, Developer Mode, DDI mounted
  ├─ russetd (the lane-A worktree's daemon; holds the live-device lock; reservation API)
  ├─ russet-lab (cargo nextest --profile lab), run on demand by: (a) device-lane agent, (b) owner (no always-on runner, decisions.md)
  ├─ Swift 6.4 + Darwin SDK (local only) ; Flutter SDK (when disk tier ≥ GREEN)
  └─ ASC team key in keychain (lab only)
later: physical Windows PC (PLAN W27a–c), reached over SSH in announced windows
```

1. **Exclusivity.**
   - Only the device-lane agent or the owner runs L7.
   - Tests take a daemon reservation through `device.reserve`. While a reservation is held, other clients get `device.reserved` (3018).
   - The lab harness checks the host-wide live-device lock (`$XDG_RUNTIME_DIR/russet-live-device.lock`, ARCHITECTURE §3.3 item 7) that every live daemon holds: it runs through the lane-A worktree's daemon and refuses to start while a daemon of another `RUSSET_HOME` holds the lock.
   - Agents outside the device lane export `RUSSET_DEVICE_BACKEND=scripted:…` or `mock:…` and their own `RUSSET_HOME` (AGENTS.md, PLAN §0 rule 4).
2. **Leave the device as found.**
   - Test apps use bundle IDs `dev.russet.test.*` and are uninstalled by the cleanup step (E2E-14), which always runs.
   - Tests never touch user apps or data, never erase, and never change settings.
   - Developer Mode and the DDI are only checked, never toggled.
3. **Apple account hygiene.**
   - One fixed test bundle ID per fixture and one reused development certificate.
   - E2E-12 asserts that a second run creates **zero** certificates, profiles and bundle IDs.
   - Free-account tests are manual and weekly, because of Apple's limits: 10 App IDs and 3 devices per 7 days, and 3 apps per device [R:sg-10].
4. **Environment-not-ready is not a failure.**
   - The pre-flight E2E-01 exits with code 3 ("skipped-env") when one of these holds:
     - the phone is locked;
     - the phone is not trusted;
     - Developer Mode is off;
     - the DDI is unmounted.
   - The nightly report shows such runs as amber.
5. **Resource caps.** Lab runs execute under `systemd-run --user --scope -p MemoryMax=8G -p CPUQuota=600%`, so they do not starve the llama.cpp server [A].

#### 3.9.2 Harness

- **Crate.** `tests/lab` is the crate `russet-lab` (`publish = false`).
- **Test form.** Scenarios are ordinary Rust integration tests that use `LabHarness`. They are selected by the nextest profile `lab` and serialized by a test group [S30].

```rust
pub struct LabHarness { /* daemon client, reservation, report sink, budgets */ }
impl LabHarness {
    pub async fn acquire(requires: &[Requirement]) -> Result<Self, LabError>; // Device, Sdk, AscKey, Flutter
    pub async fn cli(&self, args: &[&str]) -> Result<CliOutcome, LabError>;  // runs `russet … --json`
    pub async fn rpc<P: Serialize, R: DeserializeOwned>(&self, method: &str, p: P) -> Result<R, LabError>;
    pub async fn expect_event(&self, pred: impl Fn(&Event) -> bool, within: Duration) -> Result<Event, LabError>;
    pub fn budget(&self, metric: &str, value: Duration);                     // section 3.10
    pub async fn cleanup(self) -> Result<(), LabError>;                     // uninstall dev.russet.test.*
}
```

```toml
# .config/nextest.toml (sketch)
[test-groups]
device = { max-threads = 1 }
[profile.default]
default-filter = "not package(russet-lab)"
[profile.ci]
fail-fast = false
junit = { path = "junit.xml" }
[profile.lab]
default-filter = "package(russet-lab)"
retries = 1
slow-timeout = { period = "60s", terminate-after = 5 }
junit = { path = "lab-junit.xml" }
[[profile.lab.overrides]]
filter = "package(russet-lab)"
test-group = "device"
```

- **Reports.** The harness writes `target/lab-report/{lab-junit.xml, report.json, budgets.json, logs/}`. Before upload, the report goes through `cargo xtask guard --artifacts`, which checks for no IPA, `.app`, dSYM or SDK files, and no raw traces with PII.

#### 3.9.3 Fixture apps

Fixture apps live in `tests/fixtures/apps/`, are licensed 0BSD, and are our own code.

- **`RussetProbe`** (SwiftUI, SwiftPM). It has:
  - a known breakpoint inside `ContentView.body`: the statement after `let probeValue = counter  // RUSSET_PROBE_BREAK`, where `@State var counter = 42` (**updated in review round 2**, ARCHITECTURE [D 84]: `frame variable` reads the plain local `probeValue`, because a SwiftUI `@State` property is readable only as `self._counter`, and expression evaluation off-Mac is unproven until D7);
  - a stdout heartbeat `PROBE seq=<n> t=<ms>` every 100 ms;
  - a background colour that cycles every 2 s, which tests mirror liveness without needing input;
  - a `-RussetProbeCrash 1` launch argument that calls `crashNow()`;
  - an asset and nib harness mode for E2E-15, which loads every corpus asset by name and prints `ASSET ok|fail <name>`.
- **`RussetFlutterProbe`** (Flutter 3.47.x [R:Appendix A]). It is a counter app whose `build()` prints `FLUTTER_PROBE label=<text>`. It uses two SwiftPM plugins (the M2 deliverable).
- **`RussetObjCMini`** (ObjC). It is linked against **hand-written** `.tbd` stubs and headers that we wrote ourselves (a few symbols only). This lets Linux CI exercise linking, signing, packaging and lint without the Apple SDK [A: if counsel considers even hand-written stubs a risk, drop it].

#### 3.9.4 Scenarios

**Suites:**

| Suite | Scenarios | Duration |
|---|---|---|
| `smoke` | 01, 02, 03, 06, 14 | About 5 min |
| `nightly` | 01–14 | About 20 min |
| `release` | nightly, plus 15, 19 and 16 (dry-run) | |
| `manual` | 16 (real upload), 17, 20 | |

**Scenarios:**

| ID | Scenario | Steps (abridged) | Acceptance |
|---|---|---|---|
| E2E-01 | Doctor ready | `russet doctor --json` | Swift, SDK, LLDB, usbmuxd, pairing, Developer Mode and DDI are all `ok`; otherwise exit 3 (skipped-env) |
| E2E-02 | Device list | `russet device list --json` | The iPhone appears with `product_type=iPhone13,2` and `product_version=27.0` within 5 s |
| E2E-03 | SwiftUI build, install and launch | `russet run tests/fixtures/apps/RussetProbe --json` | Exit 0; `PROBE seq=` appears on stdout within 30 s; the app list contains `dev.russet.test.probe`; the bundle ID is exactly as written (no `XTL-` prefix, per verified-facts) |
| E2E-04 | SwiftUI breakpoint (CLI and LLDB path) | `russet debug --batch --detach --break ContentView.swift:<marker line + 1> --command 'frame variable probeValue'` | `stopped` with reason `breakpoint` within 60 s; frame 0 matches `ContentView.body.getter`; `probeValue == 42`; the batch detaches cleanly and the pid is still alive 5 s later (LLDB batch commands are synchronous, so the CLI path detaches instead of continuing; ARCHITECTURE §8.7, [D 84]) |
| E2E-05 | SwiftUI breakpoint (DAP path) | Scripted DAP client against the Russet debug adapter | Same as E2E-04 through DAP messages |
| E2E-06 | Logs | `russet logs --app dev.russet.test.probe --json` for 15 s | At least 100 consecutive `seq` values with no gap; p95 latency between `t` and host receipt below 1 s (clock offset estimated at start) |
| E2E-07 | Screen mirror | JSON-RPC `mirror.start` for 10 s | Frames reach at least the target fps (budget); frame size matches the device (1170×2532, or the negotiated scale); the dominant colour changes within 3 s, following the probe's cycle. Later step: an HID tap changes the label (when WS-12 ships input) |
| E2E-08 | Flutter hot reload | `russet flutter run` on RussetFlutterProbe; edit the label; trigger reload (`r` or VM-service `reloadSources`) | `FLUTTER_PROBE label=<new>` within 5 s; reload time recorded; the "Dart execution mode" line recorded (JIT vs interpreter [R:fx-18]); DevTools URL reachable; no JIT faults (`EXC_BAD_ACCESS`/`SIGBUS`) and `pagesTouched == pagesNotified`; DC-30 passed (PLAN §11; the rig is TXM, ARCHITECTURE [D 71]) |
| E2E-09 | Profile | `russet profile --template sysmon --app … --duration 10s --out trace.json` | At least 9 samples for the probe's pid; `trace.json` validates against the Chrome/Perfetto trace schema |
| E2E-10 | Crash report | Launch with `-RussetProbeCrash 1`; then `russet crashes pull --app … --symbolicate` | A new `.ips` report; the symbolicated crashing frame contains `crashNow` |
| E2E-11 | TestFlight dry-run | `russet ship --testflight --dry-run --json` | Lints evaluated; ASC authentication checked with read-only GETs; upload plan emitted; **the HTTP log shows zero POST, PATCH or DELETE requests** |
| E2E-12 | Idempotent provisioning | Run E2E-03 twice, counting ASC certificates, profiles and bundle IDs before and after | Second run: deltas are all 0 |
| E2E-13 | MCP agent loop | Scripted MCP client over stdio: `tools/list`, `device_list`, `app_run`, `logs_read`, `screenshot_take`, `breakpoint_set`, then `ship_upload` without approval | Tier 0–1 calls succeed; `ship_upload` returns `isError` with `perm.approvalRequired` (1204) and an `approvalId` (ARCHITECTURE §11.3); all calls appear in the audit log |
| E2E-14 | Cleanup | Uninstall `dev.russet.test.*` | The app list no longer contains them; always runs |
| E2E-15 | Asset harness (G4) | Install RussetProbe built with our `Assets.car`, nibs and strings | Every `ASSET` line is `ok` |
| E2E-16 | TestFlight canary (G5) | A real upload of RussetProbe to internal testing, with a manual dispatch and approval | Processing state is `VALID`; the owner installs it from TestFlight and confirms (manual record in the release checklist) |
| E2E-17 | Free Apple ID (manual, weekly) | Throwaway Apple ID; real anisette path | Install and launch succeed, or the known failure 0xe8008015 is reported and tracked [R:fc-sg] |
| E2E-18 | Windows suite (G7) | E2E-01–10 on the physical Windows PC | Same criteria |
| E2E-19 | IDE device smoke (G6) | Playwright drives the Run, Debug, logs and mirror panels against the phone | E2E-03, 05, 06 and 07 criteria, met through the UI |
| E2E-20 | Reconnect (manual) | Unplug and replug | The daemon shows the device again within 10 s, and a run succeeds |

### 3.10 Performance budgets (lab)

- **How budgets start.** Budgets begin in **record-only** mode. After 3 runs, the median becomes the baseline.
- **Enforcement.** A nightly run fails when the median of a metric exceeds the baseline by more than 25 %. Budgets are stored in `tests/lab/budgets.toml`.

| Metric | Initial target (iPhone 12, USB, iOS 27.0) |
|---|---|
| `run.cold` (clean checkout → app visible) | Recorded |
| `run.incremental_noop` | At most 15 s |
| `run.incremental_swift_edit` | At most 25 s |
| `debug.attach_to_stop` | Recorded. Expect it to be slow: system symbols are read from process memory [R:dv-16] |
| `logs.latency_p95` | At most 1 s |
| `mirror.fps` | At least 15 fps |
| `flutter.hot_reload` | At most 1.5 s median (xlinux claims about 0.5 s [R:fx-22]) |
| `profile.sample_interval` | Within 10 % of the request |

### 3.11 Licensing of fixtures and templates

- **Project templates and injected code.** `templates/` (project templates created by `russet new`) and any code that Russet compiles **into user apps** use **0BSD**, so that GPL obligations never reach user apps. That includes the Flutter Runner shim, launch-screen code and any JIT helpers that run in-app. GPL code is incompatible with App Store distribution terms [A: counsel to confirm wording]. The Flutter-derived Runner keeps Flutter's BSD-3 notice.
- **Test fixtures** use 0BSD. Corpus inputs are 0BSD or CC0.
- **REUSE.** All of the above is annotated in `REUSE.toml`.

---

## 4. CI on GitHub Actions

### 4.1 Principles

1. **Hosted CI never sees an Apple SDK, a DDI, Xcode or a user secret.**
   - Builds that need the SDK run only on the owner's machine (L7) or on Apple hardware (L8 and the Apple-hardware release mode).
   - Linux and Windows CI use synthetic fixtures, traces and cassettes.
2. **No Apple files in any artifact or cache.**
   - Every upload goes through a composite action `./.github/actions/upload-safe`, which runs `cargo xtask guard --artifacts <dir>` before `actions/upload-artifact`. Its allowlist names `apple-release.yml`'s own RussetProbe Mach-O outputs by job and path; anything else with Mach-O magic is blocked.
   - Caches are limited to `~/.cargo/registry`, `~/.cargo/git`, `target/` and the npm cache.
3. **Least privilege.**
   - `permissions: {}` at workflow level, raised per job.
   - `persist-credentials: false`.
   - Third-party actions pinned to full commit SHAs [S12].
   - No `pull_request_target`.
   - zizmor in L0.
4. **Cost discipline in the private phase** (decisions.md: free included minutes only; ARCHITECTURE [D 77]).
   - No payment method on the account, so jobs stop at the quota and CI can never cost money.
   - `concurrency` with `cancel-in-progress`; path filters (docs-only changes skip Rust jobs); draft PRs run nothing.
   - Linux on `main` pushes (and on PRs labelled `ci`); Windows weekly and on dispatch; macOS only for owner-dispatched Apple-hardware runs.
   - Agents run `cargo xtask ci` locally before pushing; the lead runs `--profile full` before merging and checks `gh pr checks` where a PR exists, because required checks cannot be enforced on a private Free repository.
   - `merge_group` is not used: merge queues need an organization-owned repository, and this one is user-owned.

### 4.2 Workflows

> **Runner matrix, review round 1** (ARCHITECTURE [D 77]). The private phase must fit GitHub Free's 2,000 included minutes (decisions.md), consumed at list price (V16).

**Windows rule.** Rust build, test and package jobs run on `windows-2025` (Server 2025 + VS 2026 18.10, Windows 11 SDK 26100); the pinned 1.99.0 toolchain is cached (`profile = "minimal"`), because the image ships 1.98.1. Swift-for-Windows work (W27b) and the gen_snapshot Windows build (VS 2022 + clang-cl) run on `windows-2022` (VS 2022 17.14, SDKs 22621 and 26100).

**Never use:** macOS on PR or push; `macos-latest`; any `-large` or `-xlarge` label (never free, no included minutes); `ubuntu-latest` or `windows-latest` (moving targets); hosted `ubuntu-22.04` (deprecated; release builds use an `ubuntu:22.04` container instead, [D 47]).

| File | Trigger | Runners | Jobs | Notes |
|---|---|---|---|---|
| `ci.yml` | private: `push: main`, PRs labelled `ci`, weekly cron (Windows), dispatch. Public: every non-draft `pull_request` and `push: main` | `ubuntu-24.04`; `windows-2025` (weekly and on dispatch in the private phase, every PR once public) | `guard`, `rust-linux`, `rust-windows` (build all + L1 + the named-pipe/DACL subset), `extension`, `docs`, `schemas` | Required checks are enforceable only with Pro or once public; until then the lead's merge step checks them |
| `nightly.yml` | private: weekly cron; public: nightly cron `0 2 * * *`; dispatch | `ubuntu-24.04` (+ a `windows-2025` L5 subset once public) | Fuzz (12 targets × 3 min, nightly toolchain); coverage; reproducibility check (build twice, compare SHA-256); `cargo deny check advisories`; lychee | |
| `lab.yml` | none in the private phase: lane A runs `cargo xtask lab` on demand (decisions.md: no always-on self-hosted runner) | — (public phase: the lab bridge, section 4.5) | `cargo xtask lab --suite <s>` | |
| `oracle.yml` | dispatch (input: case glob, compare-on-oracle flag); public phase also a weekly compare cron | `xcode-27` only, `DEVELOPER_DIR=/Applications/Xcode_27.0.app/Contents/Developer`, asserting build `27A266a` | Generate or compare; upload normalized JSON only | Private phase: only with the owner's OK per run (the Mac is the oracle); about 155 included minutes per 15-minute run |
| `apple-smoke.yml` | dispatch by the owner (once, Wave 2, W16) | `xcode-27` | The archive half of the SIGN §9.9 template on RussetProbe; no Apple credentials | ≤ 15 macOS minutes |
| `apple-release.yml` | dispatch by the owner | `xcode-27` | Apple-hardware release of the fixture app (generated template, signing mode `russet`) → TestFlight (G5) | No ASC secrets on GitHub in `russet` mode (section 4.6) |
| `ide.yml` | private: not run (Tier-2 builds run locally at tier BLUE with the owner's OK, PLAN W25; budget in §4.8); public: dispatch and nightly-on-change | `ubuntu-24.04`, `windows-2025` | Code-OSS fork build, packaging, smoke tests | Free-disk step; Node from `.nvmrc` (24.x) [V2]; do not inherit VSCodium's `ubuntu-22.04` |
| `release.yml` | tag `v*` | an `ubuntu:22.04` container on `ubuntu-24.04`; `windows-2025` | Build matrix, package, smoke-install in containers, SBOM, checksums, minisign, draft release | Attestations once public |
| `publish.yml` | dispatch after the release is published | `ubuntu-24.04` | AUR push, winget PR, docs deploy | Public phase |
| W27b and gen_snapshot jobs | dispatch | `windows-2022` | Swift for Windows samples; the gen_snapshot Windows build | gen_snapshot lives in a separate public repository (build Q4), where runners are free |

Optional: `ubuntu-24.04-arm` ($0.005/min) for run-debug's H4 arm64 job once public.

### 4.3 `ci.yml` sketch

```yaml
name: ci
on:
  pull_request: { types: [opened, synchronize, reopened, ready_for_review, labeled] }
  push: { branches: [main] }
  schedule: [{ cron: "17 3 * * 1" }]    # weekly: the Windows job in the private phase
  workflow_dispatch:
permissions: {}
concurrency: { group: "ci-${{ github.ref }}", cancel-in-progress: true }
env: { CARGO_TERM_COLOR: always, CARGO_INCREMENTAL: "0", RUSSET_OFFLINE: "1" }
jobs:
  guard:
    runs-on: ubuntu-24.04
    permissions: { contents: read }
    steps:
      - uses: actions/checkout@<full-sha>   # v5; pinned by Dependabot
        with: { persist-credentials: false, fetch-depth: 0 }
      - run: cargo xtask guard all          # apple-files (with fixture provenance), public-terms, reuse, typos, gitleaks, zizmor, traces, dco, commits
  rust-linux:
    # private phase: main pushes and PRs labelled "ci"; public phase: every non-draft PR
    if: github.event_name != 'pull_request' || (!github.event.pull_request.draft && (vars.RUSSET_PUBLIC == 'true' || contains(github.event.pull_request.labels.*.name, 'ci')))
    runs-on: ubuntu-24.04
    permissions: { contents: read }
    steps:
      - uses: actions/checkout@<full-sha>
        with: { persist-credentials: false }
      - uses: Swatinem/rust-cache@<full-sha>
      - uses: taiki-e/install-action@<full-sha>
        with: { tool: "cargo-nextest,cargo-deny" }
      - run: cargo xtask ci --profile pr    # fmt, clippy, nextest (L1–L5), golden check, deny, doc
      - uses: ./.github/actions/upload-safe
        if: always()
        with: { name: junit-linux, path: target/nextest/ci/junit.xml }
  rust-windows:
    # private phase: weekly and on dispatch; public phase: every non-draft PR and main push
    if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch' || (vars.RUSSET_PUBLIC == 'true' && (github.event_name != 'pull_request' || !github.event.pull_request.draft))
    runs-on: windows-2025    # Rust jobs; Swift for Windows and gen_snapshot use windows-2022
    permissions: { contents: read }
    steps:
      - uses: actions/checkout@<full-sha>
        with: { persist-credentials: false }
      - uses: Swatinem/rust-cache@<full-sha>
      - run: cargo xtask ci --profile windows   # build all, L1, L4, L5 subset (named pipes, TCP fake usbmuxd)
  extension:
    runs-on: ubuntu-24.04
    permissions: { contents: read }
    defaults: { run: { working-directory: extension } }   # ARCHITECTURE D 33: extension/ at the root
    steps:
      - uses: actions/checkout@<full-sha>
        with: { persist-credentials: false }
      - uses: actions/setup-node@<full-sha>
        with: { node-version-file: extension/.nvmrc, cache: npm, cache-dependency-path: extension/package-lock.json }
      - run: npm ci && npm run lint && npm test
      - run: xvfb-run -a npm run test:integration   # @vscode/test-cli against mock daemon
```

### 4.4 Caching and artifacts

- **Rust caches.** `Swatinem/rust-cache`, keyed on `Cargo.lock`, the toolchain and the job. The repository's cache budget is 10 GB [S9]. Scheduled jobs prune old caches.
- **Lab caches.** The lab runner caches nothing on GitHub. Its target directory is local (section 10).
- **Fuzz corpora.** Stored as artifacts with 7-day retention after the guard. They contain generated inputs only.
- **Retention.** 14 days for test reports, 90 days for release artifacts (GitHub Releases is the long-term store).

### 4.5 The device lab runner and the lab bridge

**Private phase (now until G1).** decisions.md (2026-10-11): no always-on self-hosted runner. The device-lane agent runs `cargo xtask lab --suite <s>` on demand in the working session, as the owner's user (it needs the SDK, the usbmuxd socket and the keychain), under the host-wide live-device lock (ARCHITECTURE §3.3 item 7) and the resource caps of section 3.9.1. Results go to `docs/lab/results/` (PLAN §2).

**Public phase (from G1), only if the owner later wants an always-on lab.** GitHub warns that self-hosted runners "should almost never" serve public repositories [S12]. So:

1. The runner moves to a **private** repository, `evilcomputer12/russet-lab`.
2. Its workflow checks out `evilcomputer12/russet@<sha>`. It is triggered by `repository_dispatch` from the public repository's `main` pushes and nightly schedule. The dispatch uses a fine-grained PAT that can only write Actions on russet-lab.
3. Results are posted back as the commit status `lab/e2e` (PAT with `statuses:write` on russet only), with the summary report attached.
4. Pull requests from forks never reach the lab. A maintainer can dispatch a lab run for a reviewed PR head SHA by hand.

**Later, the physical Windows PC** is reached by the device-lane agent over SSH in announced windows (PLAN §8); the same live-device lock applies there.

### 4.6 Secrets and gating

**Private phase.**

- Repository secrets, because environment secrets need Pro or higher and required reviewers are unavailable [V17]:
  - `MINISIGN_KEY` and `MINISIGN_PASSWORD`.
- **No ASC key in GitHub secrets.** The oracle uses none, lab runs are local, and the Apple-hardware release runs in signing mode `russet`, which uploads locally (SIGN §9.9). Only the `xcode-manual`/`xcode-cloud` modes would need signing secrets on GitHub (open question 8).
- Release, oracle and apple-release workflows trigger only through dispatch or tag pushes by the owner.

**Public phase.** Each group of secrets moves to a GitHub Environment with required reviewers:

| Environment | Secrets |
|---|---|
| `release` | minisign key; SignPath token |
| `apple` | ASC key |
| `publish` | AUR SSH key; winget PAT |

Artifact attestations (`actions/attest`) are enabled at the same point [S13].

### 4.7 macOS jobs

macOS is used only for:

- **oracle generation and comparison** (section 3.4.5);
- **the Apple-hardware release mode**:
  - It is validated in our own CI by `apple-release.yml` on the fixture app (G5).
  - It is also offered to users as a workflow that `russet ship ci init github` generates into *their* repository, with the runner derived from the imported SDK's Xcode major (`xcode-27` for Xcode 27; `pins` `[ci.github]`). The ship area owns its content. QA verifies the generated workflow end to end once per release.
- **Alternatives**, documented for users (SIGN §9.9): a self-hosted macOS runner (free; not for Russet's own CI, because the owner's Mac is the RE oracle and hosts a VM); Codemagic (500 free M2 minutes a month, then $0.095/min); Xcode Cloud (25 hours a month included, onboarding needs a Mac). None is needed for Russet's own CI once the repository is public.

### 4.8 Budget (private phase)

decisions.md allows only GitHub's free private-repo minutes, and the account is on GitHub Free (V6): 2,000 included minutes and 500 MB of artifacts per month, consumed at list price (V16). The account has no payment method, so jobs stop when the quota runs out and CI can never cost money; when that happens, the lead's local `cargo xtask ci --profile full` is the merge gate until the month resets. `cargo xtask ci-budget` reads the month's usage through `gh api` and warns at 80 %.

**Plan that fits** (Linux-equivalent minutes; per month, by wave):

| Item | Volume | Included minutes | When |
|---|---|---|---|
| `ci.yml` Linux jobs on `main` pushes | about 60 runs × 20 min | 1,200 | every month |
| Weekly `nightly.yml` | 4 × 45 min | 180 | every month |
| Weekly `rust-windows` | 4 × 20 Windows min × 1.67 | 134 | every month |
| Apple-hardware smoke (`apple-smoke.yml`, W16, owner-dispatched) | 1 × 15 macOS min × 10.3 | 155 | once, Wave 2 |
| W16b's Windows materializer tests on `windows-2022` | about 4 runs × 10 Windows min × 1.67 | about 67 | Wave 2 |
| **Waves 1–2 total** | | **about 1,740 of 2,000** | |
| `apple-release.yml` (W23, G5; signing mode `russet`, plus the one owner-approved `xcode-manual` run) | 2 × 15 macOS min × 10.3 | about 310 | Wave 4, only if still private |
| W27b's `windows-2022` jobs (Swift for Windows samples, the OpenAppleMacros server build) | about 4 runs × 35 Windows min × 1.67 | about 234 | Wave 5, only if still private |
| `ide.yml` Tier-2 builds (Code-OSS on `ubuntu-24.04` + `windows-2025`) | 250–350 per run (IDE §3.13) | **0 while private**: Tier-2 builds run in CI only once the repository is public, otherwise locally at tier BLUE with the owner's OK (PLAN W25) | Wave 4 |
| **Wave-4 month if still private** | 1,200 + 180 + 134 + 310 | **about 1,820 of 2,000** | |
| **Wave-5 month if still private** | 1,200 + 180 + 134 + 234 | **about 1,750 of 2,000** | |

All volumes are estimates [A]; `cargo xtask ci-budget` measures the real use and warns at 80 % (1,600 minutes), which a private Wave-4 month crosses: the lead then batches merges into fewer `main` pushes, and the local `cargo xtask ci --profile full` stays the merge gate. Oracle runs stay on the owner's Mac. Each `gha` oracle run would add about 155 minutes, so it needs the owner's OK per run.

**For comparison** (if the owner adds a payment method, ARCHITECTURE §21 Q6): QA's original volumes (150 PR/main runs, 30 nightlies, 8 oracle runs) plus 4 Apple-hardware runs cost about $71.50 a month at list price: Linux 5,550 min ($33.30), Windows on every non-draft PR 2,700 min ($27.00), macOS 180 min ($11.16). Net of the quota that is about $59.50 on Free, or about $57.50 on Pro including its $4 fee; Windows on `main` and nightly only saves about $16. Per run: Linux about $0.15, Windows about $0.18, an oracle run about $0.93, an Apple-hardware release $0.62–0.93.

**Public phase:** every standard-runner job above costs $0, including macOS on `xcode-27` [V16]. Recommendation: make the repository public as soon as G1 passes (open question 2).

### 4.9 Local CI for agents

`cargo xtask ci --profile pr` runs the same L0–L5 set as `rust-linux`.

| Command | Purpose |
|---|---|
| `cargo xtask ci --quick` | fmt, clippy and the unit tests of changed crates (uses `git diff --name-only` against `main`); for inner loops |
| `cargo xtask ci --profile pr` | Required before an agent reports a task done |
| `cargo xtask ci --profile full` | Before the lead merges a workstream |

---

## 5. Release engineering

### 5.1 Versioning

**The workspace.**

- One version for all crates (lockstep), `0.MINOR.PATCH` until 1.0.
- Pre-1.0, MINOR bumps carry features or breaking changes and PATCH bumps carry fixes.
- Milestone mapping:

| Version | Milestone |
|---|---|
| 0.1 | G1/M1 |
| 0.2 | M2 (Flutter) |
| 0.3 | M3 kit |
| 0.4 | M4 resources |
| 0.5 | M5 ship |
| 0.6 | First IDE build (G6) |
| 0.7 | Windows (G7) |
| 1.0 | After the legal review and public stabilization |

**Protocol versions.**

| Interface | Rule |
|---|---|
| CLI ↔ daemon | Must be the same build. On a mismatch the daemon restarts, and the reported code is `RUS-RPC-0409`. |
| JSON-RPC API | `api.major` and `api.minor` are reported by `rpc.hello`. The extension declares the range it supports. A method is deprecated for at least one MINOR before it is removed. |
| MCP | Russet serves 2026-07-28 and legacy 2025-11-25 (dual-era) [V19]. Each tool schema carries `x-russet-schema: <n>`. |

**IDE.**

- Code-OSS internals keep the upstream `product.version` (for example 1.141.0), because extension `engines.vscode` checks depend on it.
- The Russet version is stored as `russetVersion` in `product.json`, and shown in About and in package names.

| Format | IDE version scheme |
|---|---|
| AppImage | `Russet-IDE-0.6.0-x86_64.AppImage` |
| deb | `russet-ide_0.6.0~code1.141.0_amd64.deb` |
| rpm | `Version: 0.6.0`, `Release: 1.code1.141.0` |
| MSI | `ProductVersion 0.6.0`, numeric only |

**Pre-releases** use `v0.6.0-rc.1`. Nightlies use `0.6.0-nightly.YYYYMMDD`; they are workflow artifacts only and never published.

### 5.2 Commits and changelog

- [Conventional Commits 1.0.0][S33]:
  - Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.
  - Scopes are crate or area names: `device`, `build`, `sign`, `debug`, `flutter`, `mirror`, `profile`, `ship`, `mcp`, `ide`, `docs`, `release`, `qa`.
  - `!` marks a breaking change.
- `git-cliff` (2.14.2) with `cliff.toml` generates `CHANGELOG.md` and the release notes.
- `cargo xtask release prepare 0.2.0` bumps versions, regenerates the changelog and runs the full CI profile.

### 5.3 Build matrix and artifacts

| Artifact | Target | Built on | Tool |
|---|---|---|---|
| `russet-<v>-x86_64-unknown-linux-musl.tar.gz` | Linux x64 (any distro) | `ubuntu-24.04` | cargo, musl target, rustls/`ring` [A: fall back to gnu with glibc 2.35 if a dependency needs dlopen] |
| `russet_<v>_amd64.deb` | Debian, Ubuntu | `ubuntu-24.04` | `cargo-deb` 3.8.0. `Recommends: usbmuxd` |
| `russet-<v>-1.x86_64.rpm` | Fedora, RHEL, openSUSE | `ubuntu-24.04` | `cargo-generate-rpm` 0.21.0 |
| AUR `russet-bin`, `russet-git` | Arch, CachyOS | PKGBUILDs in `packaging/aur/` | `makepkg` and `namcap` in an `archlinux` container |
| `russet-<v>-x86_64-pc-windows-msvc.zip` | Windows x64 | `windows-2025` | cargo |
| `russet-<v>-x64.msi` | Windows x64 | `windows-2025` | WiX; the version is open question 11 [S27] |
| winget `evilcomputer12.Russet` | Windows | — | Manifest PR (komac or wingetcreate) [S28] |
| `Russet-IDE-<v>-x86_64.AppImage` and `.zsync` | Linux | `ubuntu-24.04` | Code-OSS gulp tasks plus `appimagetool` (type2 runtime; statically linked, no libfuse2 needed [S29]) |
| `russet-ide_<v>…_amd64.deb`, `.rpm`, `.tar.gz` | Linux | `ubuntu-24.04` | Code-OSS gulp tasks, as in VSCodium [S18] |
| `RussetIDEUserSetup-<v>-x64.exe`, `Russet-IDE-<v>-x64.msi`, `.zip` | Windows | `windows-2025` | Inno Setup (upstream Code-OSS), WiX for MSI (as in VSCodium [S18]) |
| winget `evilcomputer12.RussetIDE` | Windows | — | Manifest PR |
| `SHA256SUMS`, `SHA256SUMS.minisig` | all | `ubuntu-24.04` | `cargo xtask release sign` |
| `russet-<v>.cdx.json` | SBOM | `ubuntu-24.04` | `cargo-cyclonedx`, plus npm SBOM for the extension and IDE |
| `russet-<v>-src.tar.zst` | Corresponding Source | `ubuntu-24.04` | `git archive` plus `cargo vendor` plus the npm lockfiles and patches (section 9, L5) |

**What each package contains.**

- **CLI packages:**
  - the `russet` binary, plus daemon and MCP entry points as the CLI design defines them;
  - shell completions (`clap_complete`) and man pages (`clap_mangen`);
  - `LICENSE`, `NOTICE.md` and `THIRD_PARTY_LICENSES.html` (cargo-about);
  - an optional systemd user unit, `russetd.service`.
- **The IDE** bundles the CLI of the same version under `resources/app/bin/`.
- **Binaries carry their own dependency list** through `cargo auditable build`.
- **Later:** aarch64 Linux and Windows arm64 (built in CI after G6). There are no macOS release builds; macOS is not a target platform.

**Package smoke tests (L9).**

| Package | Clean container or VM | Steps |
|---|---|---|
| `.deb` | `ubuntu:24.04`, `debian:13` | Install, then `russet --version` and `russet doctor --json --offline` (exit 0 or 3) |
| `.rpm` | `fedora` | Same as above |
| AUR | `archlinux` container | `makepkg -si` |
| AppImage | — | `--appimage-extract-and-run --version` |
| MSI | Windows runner | Install, check PATH, uninstall, and verify that nothing is left behind |

### 5.4 Signing our own binaries

| Artifact | Private phase | Public phase |
|---|---|---|
| All release files | `SHA256SUMS` signed with **minisign**. The public key goes in the README and docs; the key is a repository secret. | The same, plus GitHub artifact attestations (`actions/attest`), which are unavailable for private repositories outside Enterprise Cloud [S13] |
| AppImage | Embedded signature (`appimagetool --sign`, project GPG key) and zsync update info `gh-releases-zsync\|evilcomputer12\|russet\|latest\|Russet-IDE-*x86_64.AppImage.zsync` | Same |
| .deb, .rpm | Unsigned packages, covered by the signed checksums | `rpm --addsign` with the project GPG key; signed apt and rpm repositories are optional later |
| Windows exe, msi, dll | Unsigned, for the owner's PC only (expect SmartScreen prompts) | **SignPath Foundation** Authenticode. It is free for OSI projects, but the certificate names "SignPath Foundation" as publisher. It requires a published "Code signing policy" page, MFA, and defined author, reviewer and approver roles [S25]. Alternative: an OV certificate in the owner's name ($150–300 a year, HSM-backed) [S26]. Azure Artifact Signing is **not** available to individuals outside the USA and Canada [S26]. |
| AUR | `sha256sums` (and `validpgpkeys` if we GPG-sign tarballs) | Same |

### 5.5 Release process

1. **`cargo xtask release prepare <v>`.** Bumps versions and the changelog; runs `ci --profile full`, the lab `release` suite (device lane) and the legal checklist (section 9).
2. **Tag.** The lead signs the tag (`git tag -s v<v>`) and pushes it; `release.yml` builds a **draft** release.
3. **L9.** Package smoke tests run on the draft's artifacts, plus the manual checks (TestFlight canary for releases from 0.5 on, Windows PC for releases from 0.7 on).
4. **Publish.** The owner publishes the release. `publish.yml` then pushes to the AUR, opens the winget PR and deploys the docs.
5. **Rollback.**
   - Mark the GitHub release as pre-release or withdrawn.
   - Revert the AUR `pkgver`.
   - Close the winget PR, or submit a fixed version.
   - Never reuse a version number.

---

## 6. Docs site

- **Engine.** mdBook 0.5.4 (MPL-2.0) in `docs/site/` (`book.toml`). Built in CI, link-checked with lychee and spell-checked with typos.
- **Hosting.**
  - Public phase: GitHub Pages through `actions/deploy-pages`.
  - Private phase: built as a workflow artifact, and served locally with `mdbook serve`.
  - Stable schema URLs: `/schemas/rpc/v1/*.json`.
- **Generated reference pages** (`cargo xtask docs gen`; CI fails when they are stale):
  - the CLI reference, from the clap command tree (one page per command, with `--json` schema links) plus man pages;
  - the JSON-RPC reference, from schemars schemas, with examples taken from contract-test fixtures;
  - the MCP tool reference, from `russet mcp tools --json` (tier, annotations, schemas);
  - the error-code index (T8), including remedies.
- **Structure:**
  1. **Get started:** install per OS; `russet doctor`; importing Xcode (with a plain-language licence note); pairing; Developer Mode.
  2. **Guides:**
     - SwiftUI run and debug;
     - Flutter;
     - Xcode projects (later);
     - accounts: the paid ASC key (default) and the free Apple ID (opt-in, with warnings);
     - shipping to TestFlight, Linux-native or the Apple-hardware path;
     - mirror, profiling, crash reports;
     - AI agents: MCP setup per agent and the permission tiers.
  3. **Reference:** the generated pages above, plus config file formats and environment variables (`RUSSET_HOME`, `RUSSET_DEVICE_BACKEND`, `RUSSET_OFFLINE`, ...).
  4. **Concepts:** architecture; the security model; what Russet stores and where; the "no Apple files" policy; clean-room RE policy.
  5. **Legal:** NOTICE, Apple licence FAQ (not legal advice), trademark notice, the code signing policy (required by SignPath [S25]), privacy (no telemetry).
  6. **Contributing:** dev setup, the test pyramid, the device lab, the oracle, the release process (links to the repository docs).

---

## 7. Contribution guidelines

QA-04 creates the following files:

| File | Content |
|---|---|
| `CONTRIBUTING.md` | Dev setup (`rust-toolchain.toml`, the tool install line from Appendix A); `cargo xtask ci`; branch and PR flow; Conventional Commits; the DCO; tests and docs required; CODEOWNERS review; "no Apple files"; clean-room rules; how to report device logs safely (`russet doctor --report --redact`) |
| DCO | [Developer Certificate of Origin 1.1][S32]: every commit carries `Signed-off-by`. Enforced by `xtask guard dco`. Inbound licence equals outbound licence (AGPL-3.0-only; 0BSD for `templates/` and `tests/fixtures/`). No CLA. |
| `CODE_OF_CONDUCT.md` | Contributor Covenant **3.0**, the current version (July 2025) [S34] |
| `SECURITY.md` | Private reporting through GitHub Security Advisories (public phase) or email (private phase); supported versions; a 90-day disclosure target |
| `AGENTS.md`, with `CLAUDE.md` pointing to it | Rules for AI agents (below) |
| `.github/CODEOWNERS` | Owner review required for `crates/russet-core/src/secrets*`, `crates/russet-mcp/src/policy*`, `crates/russet-sign/**`, `.github/workflows/**`, `packaging/**`, `LICENSES/**`, `NOTICE.md` |
| Issue and PR templates | A device-info section produced by `russet doctor --report --redact`; a checkbox "I included no Apple files, no device identifiers and no secrets" |
| `docs/re/README.md` | Clean-room rules: behaviour specifications only, recording the method, the oracle version and the inputs; no disassembly, no decompiled code, no SDK header excerpts; invasive work only in a Mac VM [R:6.1] |

**`AGENTS.md` rules for implementation agents:**

1. **Lanes.** Respect the lanes in section 11.1. Outside the device lane, export `RUSSET_DEVICE_BACKEND=mock:auto` and never run `pymobiledevice3`, `ios`, `idevice*` or `xtool dev` against the real phone. Outside the oracle lane, do not touch `ssh mac`.
2. **Disk.** Run `cargo xtask disk-guard` before heavy work. Use the shared target directory: `CARGO_TARGET_DIR=$HOME/.cache/russet/target`. Never download more than 500 MB without lead approval. Delete your scratch outputs.
3. **No secrets, device identifiers or Apple files** in commits, logs or reports. Run `cargo xtask guard all` before handing off.
4. **Definition of done** for any task: `cargo xtask ci --profile pr` is green; new behaviour has tests at the lowest level that can catch it; docs and schemas are regenerated.
5. **Commits.** Agents do not commit; the lead does. The lead adds the attribution trailers required by the session.

---

## 8. Security

### 8.1 Threat model (summary)

**Assets:**

- the ASC API private key (`.p8`);
- Apple ID credentials and GSA session tokens;
- anisette provisioning state;
- signing certificates and private keys;
- pairing records (`HostPrivateKey`, which grants full device access);
- provisioning profiles (they contain UDIDs and the team ID);
- the user's source code;
- device data (logs, crash reports, screen);
- release signing keys.

**Adversaries:**

- a malicious or prompt-injected AI agent;
- a malicious app on the device writing to logs or the screen;
- other local users;
- network attackers on the HTTP paths;
- malicious PRs and dependencies (supply chain);
- the hosted CI provider.

### 8.2 Secrets handling

- **Storage.**
  - `SecretStore` (T6) is backed by the OS keychain through `keyring` 4.2.0: Secret Service on Linux, Credential Manager on Windows.
  - Values larger than Windows' per-credential blob limit (2,560 bytes, `CRED_MAX_CREDENTIAL_BLOB_SIZE` [A]) are stored as an AES-GCM-encrypted file whose data key is kept in the keychain. This mainly affects p12 identities.
  - **Headless fallback**, for servers and CI without Secret Service: a file store `~/.local/share/russet/secrets/` with directory mode 0700 and files 0600. A warning is shown, and file permissions are checked on every read.
  - **Environment injection** for CI: `RUSSET_ASC_KEY_P8` (PEM or base64), `RUSSET_ASC_KEY_ID`, `RUSSET_ASC_ISSUER_ID`. Values are read once and then scrubbed from the child environment.
- **In memory.** `secrecy::SecretBox` and `zeroize` for keys and tokens. No `Debug` or `Display` on secret types; this is enforced by a test that greps the `Debug` impls.
- **Redaction.**
  - The `tracing` layer drops fields named `*token*`, `*password*`, `*secret*`, `p8`, `p12`, `HostPrivateKey` and `RootPrivateKey`.
  - Bearer and JWT patterns are masked in every log line.
  - `--report` output pseudonymizes UDIDs, device names and team IDs, and keeps only the last 4 characters of the key ID.
- **Pairing records.** Read through usbmuxd and never copied into Russet state, except an explicit `russet device pair --export` (0600, with a warning).
- **Rotation.** `russet account rotate-key` is documented. The JWT lifetime is 20 minutes at most [R:sg-21]. Free-account tokens are refreshed per Apple's flow and can be revoked with `russet account logout --all`.
- **CI.** Secret inventory in section 4.6. Secrets are never available to `pull_request` jobs. `gitleaks` runs in L0 and as a full-history scan before going public.

### 8.3 Daemon, IPC and port forwards

> **Superseded names (ARCHITECTURE §7.1, §6.3, §20.1):** the socket is `russetd.sock` (per worktree under `RUSSET_HOME` in development), the handshake is `initialize`, and reservations are `device.reserve`/`lease.release` with shared-lease principals [D 63, D 68].

- **IPC endpoint.**
  - Linux: a Unix socket at `$XDG_RUNTIME_DIR/russet/daemon.sock` (directory mode 0700). The peer UID is checked with `SO_PEERCRED`.
  - Windows: a named pipe `\\.\pipe\russet-<user-SID>` with a DACL that grants only the current user SID.
  - No TCP listener by default.
- **WSL2 split mode** (WS-22). TCP on `127.0.0.1` with a 256-bit token from a 0600 file, required on the first message.
- **Port forwards.** debugserver, the Dart VM service and the mirror bind `127.0.0.1` only, on random ports. debugserver gives full control of the process, so the forward lives only for the debug session and accepts one connection.
- **Device reservation** (`device.reserve`, `device.release`). Tokens are per client, which stops two tools from fighting over one device.

### 8.4 MCP tool permissions

> **Superseded (ARCHITECTURE §11.3, §13.3, §20.1):** the approval error is `perm.approvalRequired` (1204) with `data.details.approvalId`; `RUS-MCP-0403` and `approval_request_id` are retired. Tier-2 "session" approvals bind to the bridge's connection (stdio) or to the bearer token plus a TTL (HTTP) [D 67].

**Tiers.** The MCP area owns tool names; this document owns the tier model.

| Tier | Meaning | Examples | Default policy | Annotations |
|---|---|---|---|---|
| 0 Read-only | No state change on the host, device or Apple side | `device_list`, `device_info`, `logs_read`, `screenshot_take`, `build_status`, `project_info`, `crashes_list`, `profile_read` | Allowed | `readOnlyHint: true` |
| 1 Local effect | Changes the local project or device state, reversibly | `build`, `app_run`, `app_stop`, `app_uninstall` (`dev.russet.test.*` and the project's own app only), `debug_*`, `hot_reload`, `input_tap`, `input_type` | Allowed for configured clients | `readOnlyHint: false`, `destructiveHint: false` |
| 2 Account effect | Side effects in the Apple account | `provision` (create a certificate, profile or bundle ID; register a device), `sign_distribution` | **Approval per session** | `openWorldHint: true` |
| 3 Irreversible or public | Publishing or destructive account actions | `ship_upload`, `ship_submit_review`, `testflight_testers_add`, `certificate_revoke`, `bundle_id_delete` | **Approval for every call** | `destructiveHint: true`, `openWorldHint: true` |

**Approval flow.** It works for clients of both eras, and does not rely on the client's elicitation UI, which an agent harness could answer automatically.

1. A Tier ≥ 2 call without a valid approval returns a tool execution error (`isError: true`) with structured content:
   ```json
   {"error":{"code":"RUS-MCP-0403","approval_request_id":"apr_01JA…","tool":"ship_upload",
    "args_digest":"sha256:…","summary":"Upload RussetProbe 1.0 (42) to TestFlight, team …5Q2Z",
    "approve_with":"russet approve apr_01JA…  (or Approve in Russet IDE)","expires_at":"…"}}
   ```
2. The daemon emits `approval.requested` to IDE and CLI subscribers. The IDE shows the summary and the full arguments. The human approves with `approvals.grant`.
3. The agent retries with `"approval": "apr_01JA…"`. The server checks that the approval:
   - matches the same tool;
   - matches the same `args_digest`;
   - comes from the same client identity;
   - has not expired (10 minutes);
   - has not been used before.

   Any mismatch fails with `RUS-MCP-0403`.
4. On 2026-07-28 clients, the server MAY also send `elicitation/create` as an `InputRequiredResult` [S15] for convenience. That does **not** replace step 2.

**JSON-RPC methods for approvals** (daemon API; payloads are owned by `russet-proto`):

```json
{"jsonrpc":"2.0","id":7,"method":"approvals.list","params":{}}
{"jsonrpc":"2.0","id":8,"method":"approvals.grant","params":{"request_id":"apr_01JA…","scope":"once"}}
{"jsonrpc":"2.0","id":9,"method":"approvals.deny","params":{"request_id":"apr_01JA…","reason":"not now"}}
{"jsonrpc":"2.0","method":"event","params":{"type":"approval.requested","request_id":"apr_01JA…","client":"claude-code",
 "tool":"ship_upload","tier":3,"args_digest":"sha256:…","summary":"…","expires_at":"2026-10-11T01:10:00Z"}}
```

**Policy file** `~/.config/russet/mcp-policy.toml`, created with safe defaults:

```toml
version = 1
default_tier_max = 1                  # calls above need approval
[clients.claude-code]   # identity = `russet mcp serve --client <name>` launch argument (stdio)
tier_max = 1
[clients.local-llama]
tier_max = 0                          # local models start read-only
[tools.ship_upload]
approval = "always"                   # never | session | always (tier 3 cannot be lowered below "always")
rate_limit = "1/10m"
[audit]
path = "~/.local/state/russet/audit.jsonl"
```

**Other rules:**

- **Client identity.**
  - For stdio, identity comes from the launch argument written into each agent's MCP config. The self-reported `clientInfo` is untrusted [S15].
  - For HTTP, identity comes from the token, which binds to a client name.
  - HTTP listens on `127.0.0.1` only, checks `Origin`, requires the bearer token, and is off by default.
- **Output sanitization.**
  - Text that comes from the device or from Apple (logs, crash reports, OCR, screen text) is wrapped: `<untrusted source="device">…</untrusted>`. Structured content carries `"source":"device"`.
  - Control characters and ANSI escapes are stripped.
  - Results are capped at 64 KiB, with a pagination handle for more.
  - Tier 3 is never callable without a human, whatever an agent was told by a log or the screen.
- **Rate limits.** Per tool and per client, with a token bucket; at least the policy defaults. The spec requires servers to "rate limit tool invocations" [S15].
- **Audit.** An append-only JSONL file (0600) records:
  - time, client, tool and tier;
  - an `args_digest` plus redacted arguments;
  - the approval ID;
  - the outcome and duration.

  `russet audit tail` and the IDE agent panel read it.
- **No secrets through MCP.** No tool returns key material. `account_status` returns the team name, the key-ID suffix and expiry dates only.

**Tests (L4 and L5):**

- approval required;
- a token reused or applied to different arguments is rejected;
- an expired token is rejected;
- per-client `tier_max` is enforced;
- the audit line is written;
- the output wrapper and size cap are applied;
- rate limits apply;
- the HTTP transport rejects a bad `Origin` or a missing token.

### 8.5 Supply chain

- **`deny.toml`:**
  - advisories from RustSec;
  - licence allowlist: MIT, Apache-2.0 (with LLVM-exception), BSD-2/3-Clause, ISC, 0BSD, Zlib, Unicode-3.0, MPL-2.0, LGPL-2.1+/3.0, GPL-3.0(-or-later), CC0-1.0, BSL-1.0;
  - denied: AGPL (unless an exception is approved), SSPL, BUSL, non-commercial licences and unlicensed crates [R:3.6];
  - sources limited to crates.io plus explicitly allowed git sources.
- **Pins.** `idevice` is pinned to an exact version, because its API breaks with every point release until 0.2 [R:dv-29]. Upgrades go through a dedicated PR that runs L5 and L7.
- **Dependabot** for cargo, npm and github-actions (SHA pins), grouped weekly.
- **zizmor** and **gitleaks** in L0. OpenSSF Scorecard once public.
- **SBOM** (CycloneDX) with every release, and `cargo auditable` metadata in the binaries.

### 8.6 Vulnerability disclosure

Defined by `SECURITY.md`. Fixes go out as PATCH releases with advisories; GitHub Security Advisories are used once public.

---

## 9. Legal checklist (release gate)

Run by `cargo xtask legal-check` (automated items) together with the manual items in the release PR template. Not legal advice; counsel questions are listed in section 14.

| # | Item | Check |
|---|---|---|
| L1 | AGPL-3.0-only `LICENSE` at the root, plus `LICENSES/` (REUSE layout) [S24] | Automated (`reuse lint`) |
| L2 | Every source file has `SPDX-FileCopyrightText: 2026 The Russet contributors` and `SPDX-License-Identifier: AGPL-3.0-only`. `templates/`, `tests/fixtures/` and `corpus/` use 0BSD or CC0. Bulk annotations go in `REUSE.toml` [S24]. | Automated |
| L3 | `NOTICE.md` (Apple disclaimer, trademarks, no Apple files) is shipped in every package and shown in the IDE About dialog | Automated (package content check) |
| L4 | Third-party notices: cargo-about for Rust; an npm licence report for the extension and IDE; Code-OSS's `ThirdPartyNotices.txt`; MPL-2.0 notices (apple-codesign, mdBook if bundled) | Automated |
| L5 | **Corresponding Source** for every binary: a source tarball with vendored crates, npm lockfiles and Code-OSS patches is attached to the release, and each package points to it (GPLv3 §6) [S31] | Automated (artifact present) |
| L6 | **No Apple files**: the Apple-file guard passes on the tree, artifacts, packages and the AppImage/installer contents. Apple root CA certificates embedded through `apple-codesign` are public certificates [A: confirm with counsel; listed in NOTICE] | Automated |
| L7 | **Trademarks.** Apple marks only in referential phrases ("for iPhone", "compatible with iOS"), less prominent than "Russet". No Apple logo or glyphs [S22]. **No SF Symbols or SF fonts** (Apple licenses them only for Apple-platform UIs [S23, A]). No iPhone device-frame artwork; the mirror view uses a neutral frame. The international trademark notice is in NOTICE and the docs. | Manual, plus an asset scan (fonts and SVGs with "SF" names) |
| L8 | **IDE branding.** No "Visual Studio Code" name or logo. `product.json` uses Open VSX and not the Microsoft Marketplace [S21][S18]. No Microsoft-proprietary extensions are bundled. Microsoft telemetry endpoints are disabled. | Automated (`product.json` and bundle scan) |
| L9 | **User apps stay unencumbered.** Templates and in-app code are 0BSD; the Flutter Runner keeps its BSD-3 notice | Automated (REUSE paths) |
| L10 | **Licence compatibility.** cargo-deny licences pass; the AGPL is used only as a separate process or as reference (xcross, StikDebug) [R:3.6] | Automated |
| L11 | **Pieces meant for upstream** (the actool CLI, macros, xtool patches) live in separate MIT repositories (decisions.md) | Manual |
| L12 | **Clean-room.** `docs/re/` contains behaviour specs only, and no disassembly or SDK excerpts | Manual review, plus a grep for `APPLE_LICENSE_HEADER`, "Apple Inc. All rights reserved" and similar |
| L13 | **Privacy.** No telemetry by default; crash reporting of Russet itself is opt-in | Manual |
| L14 | **Account features.** The free-account path is opt-in with warnings; the paid ASC key is the default (decisions.md, R3) | Manual |
| L15 | Name clearance for "Russet" in software classes 9 and 42 (once, before 1.0) | Manual (open question 6) |
| L16 | Export classification note for cryptography in open-source software [A: counsel] | Manual (once) |

---

## 10. Local resources: disk, memory, device, oracle

**Disk tiers.** `cargo xtask disk-guard` measures `/home` free space and enforces these tiers. They replace a fixed schedule, because free space moved from 15 to 98 GiB within 15 minutes [V8].

| Tier | Free space | Allowed |
|---|---|---|
| RED | Below 8 GiB | Edits, docs and small unit tests only; no new toolchains, no SDK-dependent builds |
| AMBER | 8–25 GiB | Rust workspace (shared target directory, capped at 6 GiB), extension (`node_modules` about 0.5 GiB, uses the installed VS Code), lab Swift builds (cleaned afterwards) |
| GREEN | 25–45 GiB | Plus the Flutter SDK, engine artifacts and `precache --ios` (about 3 GiB [A]) |
| BLUE | 45 GiB and above, with a 20 GiB reserve kept | Plus a local Code-OSS build (estimated 6–10 GiB [A]: clone, `node_modules`, `.build`, packages). Otherwise use `ide.yml` on hosted runners (14 GB SSD [V18]). |

The Dart `gen_snapshot` cross-build (about 8 GB, about 20 min [R:fx-08]) runs in CI only.

**Workspace settings.**

- `.cargo/config.toml`: `[profile.dev] debug = "line-tables-only"`, which roughly halves the size of `target/` [A], and the `clang` linker with `-fuse-ld=lld` on Linux.
- `CARGO_INCREMENTAL=0` in CI and locally.
- `cargo sweep --time 7` runs nightly on the shared target directory.

**Memory.** RAM is shared with the llama.cpp server.

- `CARGO_BUILD_JOBS=6`. At tier AMBER there is one target directory (PLAN slot `a`) whose lock serializes every build; a second slot, and so at most two concurrent cargo builds, only from tier GREEN (PLAN §0 rule 7, ARCHITECTURE [D 81]).
- One IDE build at a time (`NODE_OPTIONS=--max-old-space-size=8192`).
- Lab runs are capped as in section 3.9.1.

**Our own cleanup.**

- Lab runs delete `.build/`, IPAs and DerivedData-like outputs after each run.
- `russet-oracle` deletes its remote `mktemp` directories.
- Raw oracle outputs in `~/.cache/russet-oracle` expire after 14 days.

**Cleanup candidates for the owner** (V9; *not* touched by this agent):

- `~/.cache/paru/clone` (17 GiB). Note that `xodus-git` (10 GiB) may contain local work for the Game Pass project.
- The `rust-docs` component (719 MiB).
- `~/.cache/xodus-launcher` (6.7 GiB) and `~/.cache/Shelly` (5.7 GiB), purpose unknown.

**Oracle availability.** The Mac was unreachable (V7), and it now also hosts the private virtual-iPhone track's VM, so oracle windows are owner-approved and never overlap that use. `russet-oracle status` reports it. The `gha` backend is the fallback once the repository is public, or per run with the owner's OK (decisions.md: macOS reference jobs run on the owner's Mac in the private phase). Waking the Mac (prevent-sleep or Wake-on-LAN) would be a settings change on the Mac, so it is the owner's call.

---

## 11. Phased implementation plan (whole product)

### 11.1 Lanes and concurrency

| Lane | Resource | Max concurrent agents | Rule |
|---|---|---|---|
| A: Device | The physical iPhone (and later the Windows PC) | 1 | The only lane that may run live device commands. It records traces, runs L7 and validates device-plane work. It batches device time: record many traces per session. |
| B: Oracle | `ssh mac` and the GitHub macOS runner | 1 | Read-only inspection plus oracle runs (section 3.4.5); keeps load on the Mac light |
| C: Heavy-disk | The Flutter SDK, Code-OSS builds and gen_snapshot builds | 1 | Only at disk tiers GREEN or BLUE, or in CI |
| D: General | CPU, RAM and the shared target directory | 4 coding agents, at most 2 concurrent cargo builds | Mock-only device access (`RUSSET_DEVICE_BACKEND=mock:*`) |

Worktrees: one per agent (`git worktree add ../russet-ws-NN`). All of them share `CARGO_TARGET_DIR` (section 10).

### 11.2 Proposed workspace map

Area documents may rename crates; the boundaries and contracts are what matter.

```text
russet/
  Cargo.toml (workspace)  rust-toolchain.toml  .cargo/config.toml  deny.toml  REUSE.toml  cliff.toml
  crates/
    russet-core        # T1–T10 traits, config, paths, errors, secrets, http
    russet-proto       # JSON-RPC types, events, schemas (schemars)
    russet-daemon      # russetd: IPC server, device ownership, reservations, approvals
    russet-cli         # `russet` (clap); --json everywhere
    russet-device      # idevice-based device plane; io::ServiceConnector (T5)
    russet-sdk         # Xcode import, version lock, provenance
    russet-build       # orchestration + adapters (swiftpm now; flutter; xcodeproj later)
    russet-macho       # inspect/lint/patch Mach-O
    russet-package     # bundle assembly, IPA (native zip)
    russet-sign        # apple-codesign based signing, entitlements, profiles
    russet-account     # ASC API client (paid), free Apple ID (opt-in)
    russet-anisette    # runtime-downloaded Apple Android libs (never shipped), sandboxed
    russet-debug       # debugproxy forward, LLDB/lldb-dap driver, gdb-remote helper, DAP
    russet-flutter     # custom device, Runner template, VM service, JIT helper, AOT
    russet-mirror      # display stream, frames, HID input
    russet-profile     # DVT instruments, Perfetto export, crash reports, symbolication
    russet-ship        # lints, stamping (opt-in), ASC upload, TestFlight, Apple-hardware mode
    russet-assets / russet-ib / russet-strings   # resource compilers (or Swift tools on AssetKit; per resources doc)
    russet-mcp         # rmcp 3.5.x server (dual-era), tiers, approvals, audit
    russet-testkit     # (dev) fakes, RDT, mock device, cassettes   — publish = false
    russet-golden      # (dev) normalizers, compare                 — publish = false
    russet-oracle      # (dev) Mac/GHA oracle runner                — publish = false
  tests/lab            # russet-lab (L7)                           — publish = false
  xtask/               # ci, guard, disk-guard, schemas, docs, golden, lab, release, legal-check
  ide/extension        # TypeScript built-in extension (also publishable to Open VSX)
  ide/build            # Code-OSS patches + scripts (VSCodium-style), branding, product.json
  templates/  tests/fixtures/  corpus/  schemas/  packaging/  docs/site/  docs/re/
```

### 11.3 Phases and dependency graph

```text
P0  WS-00 Foundations ─► WS-01 Core/IPC/proto/CLI+daemon skeleton
        │                    ├─► WS-02 Testkit ──────────────┐
        │                    ├─► WS-04 SDK ─► WS-05 Build/Package/Mach-O
        │                    ├─► WS-06 Sign & Accounts       │
        │                    ├─► WS-09 MCP server            │
        │                    ├─► WS-10 Extension v0          │
        │                    ├─► WS-20 Release pipeline (CLI)│
        │                    └─► WS-21 Docs site             │
        ├─► WS-14 Golden harness + oracle [lane B]           │
        └─► WS-25 Security & legal (continuous)              │
P1  WS-01+WS-02 ─► WS-03 Device plane [A] ─► WS-07 Debug plane [A]
P2  WS-03,05,06,07 ─► WS-08 M1 integration [A] ══► Gate G1 (go public; v0.1.0)
P3  WS-08 ─► WS-11 Flutter [A,C] ══► G2      WS-03 ─► WS-12 Mirror & input [A]
    WS-03+WS-05 ─► WS-13 Profiling & crashes [A]
    WS-14+WS-05 ─► WS-15 Resource compilers ══► G4 ─► WS-16 Ship ══► G5
    WS-10+WS-12+WS-13+WS-16 ─► WS-17 Editors & panels
    WS-09+WS-10 ─► WS-18 Agent panel & agent configs           (G3 = v0.3 kit: WS-08,10,11,21)
P4  WS-17+WS-18 ─► WS-19 IDE product build [C] ─► WS-20 IDE packaging ══► G6 (first IDE build)
P5  WS-08 (+WS-11, WS-16) ─► WS-22 Windows split → native [A on Windows PC] ══► G7
P6  WS-15+WS-14 ─► WS-23 Xcode projects / CocoaPods / russet test ─► WS-24 RN/Expo; MAUI study ══► G8
P7  1.0: legal review, SignPath, winget/AUR live, docs public
```

**Critical path:** WS-00 → WS-01 → WS-03 → WS-07 → WS-08 (G1) → WS-12, WS-13, WS-16 → WS-17 → WS-19 (G6).

Lane A, the single phone, is the bottleneck. Everything else is built against mocks first, so that device time goes to validation and recording.

### 11.4 Workstream cards

Size: S ≤ 1 agent-day, M 2–4, L 5–10, XL > 10. "Interfaces" lists the contracts each workstream must honour, in addition to T1–T10 everywhere.

**WS-00: Foundations and QA infrastructure** (M, lane D; no dependencies)
- **Scope:** the tasks QA-01 to QA-04 and QA-24 in section 12.
- **Files:** `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml`, `xtask/`, `.config/nextest.toml`, `deny.toml`, `REUSE.toml`, `LICENSES/`, `.github/`, `CONTRIBUTING.md`, `AGENTS.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`.
- **Tests:** guard self-tests with seeded bad files.
- **Done when:** `cargo xtask ci` is green on an empty workspace, both on a machine without a rustup default (V1) and on GitHub.

**WS-01: Core, proto, daemon and CLI skeleton** (M, D; depends on WS-00)
- **Scope:**
  - `russet-core` (T1–T10);
  - `russet-proto`: `rpc.hello`, `daemon.health`, `device.*` stubs, events, approvals;
  - `russetd`: IPC, single instance, version handshake, reservation API;
  - `russet` CLI: global `--json`, error codes.
- **Interfaces:** the JSON-RPC envelope, `--json` schemas and the IPC security of section 8.3.
- **Tests:** L1, and L4 snapshots.
- **Done when:** `russet daemon status --json` round-trips over a Unix socket and a named pipe in CI.

**WS-02: Testkit** (L, D; depends on WS-00 and WS-01)
- **Scope:** QA-05 to QA-09: RDT, recorder and replay, mock device, cassettes, HTTP fakes.
- **Interfaces:** T5 `ServiceConnector`; `USBMUXD_SOCKET_ADDRESS`.
- **Tests:** property tests and lint tests; the pymobiledevice3 differential test.
- **Done when:** a replay-driven `russet device info` test passes on Linux and Windows.

**WS-03: Device plane** (L, A+D; depends on WS-01 and WS-02)
- **Scope:**
  - usbmux discovery and `watch`;
  - lockdown;
  - pairing state and Developer Mode checks;
  - DDI from the user's Xcode (P0-4), with mirrors only as an opt-in;
  - the userspace RSD tunnel;
  - install and uninstall;
  - app list;
  - suspended launch through CoreDevice or DVT;
  - stdout console;
  - syslog;
  - process list and kill.
- **Interfaces:** T5; recording hooks (section 3.5.1); events `device.attached`, `device.detached` and `device.state`.
- **Tests:** L1 over replay; L5 with the mock device; traces recorded on lane A for every operation.
- **Done when:** E2E-01, 02 and 14 pass, and their replay twins pass in CI.

**WS-04: SDK and version lock** (M, D; depends on WS-01)
- **Scope:** import from `Xcode.xip` or `Xcode.app` with Russet's native `russet-xar` importer (PLAN W16b; xtool bundles are only read in place), including the DDI (ARCHITECTURE [D 62], [D 69]); the lock manifest (Xcode build → Swift → LLVM pins [R:tc-03][R:tc-05]); provenance; `doctor` checks.
- **Tests:** a synthetic Xcode layout; cassettes.
- **Done when:** `russet sdk status --json` reports the installed bundle (V3) correctly on the owner's machine.

**WS-05: Build, packaging and Mach-O** (L, D, with lab validation on A; depends on WS-04 and WS-02)
- **Scope:** SwiftPM builds (Swift Build path); Info.plist synthesis (DT* keys, opt-in and visible); native IPA writing (no `zip` binary); dSYM; Mach-O lint ML-001 to ML-010; exact bundle IDs for paid teams (verified-facts).
- **Tests:** L1, L3 (plist and Mach-O normalizers), cassettes, and determinism checks.
- **Done when:** RussetProbe builds on the owner's machine, and `russet build --json` output validates against its schema.

**WS-06: Signing and accounts** (L, D; depends on WS-01 and WS-02)
- **Scope:**
  - signing with apple-codesign: inside-out, entitlements, profiles;
  - the ASC client: JWT, find-or-create for certificates, profiles, bundle IDs and devices, reusing the existing development certificate rather than creating one per run (verified-facts);
  - the free Apple ID path (GSA SRP; `russet-anisette` with runtime download, a pinned SHA-256, and a sandboxed process; opt-in).
- **Interfaces:** T3, T6; the MCP tiers for provisioning (Tier 2).
- **Tests:** `AscFake` and `GsaFake`; RFC 5054 test vectors.
- **Done when:** E2E-12 passes (zero new objects on the second run).

**WS-07: Debug plane** (L, A+D; depends on WS-03)
- **Scope:**
  - the debugproxy forward over RSD;
  - the LLDB driver (async connect, as found in verified-facts);
  - handling libpython for Swift's LLDB, or driving LLDB without Python;
  - an lldb-dap configuration or the Russet debug adapter;
  - a gdb-remote helper for JIT page touching.
- **Tests:** a fake debugserver (L1 and L5); DAP contract tests.
- **Done when:** E2E-04 and E2E-05 pass on the phone.

**WS-08: M1 integration** (M, A; depends on WS-03, WS-05, WS-06 and WS-07)
- **Scope:**
  - `russet run`, `russet debug` and `russet logs` end to end with Russet's own code, replacing xtool and pymobiledevice3 in `demo/m1-baseline.sh`;
  - the RussetProbe fixture;
  - the lab `smoke` and `nightly` suites up to E2E-14.
- **Done when (Gate G1):**
  - E2E-01 to 06 and 12 to 14 pass in 3 consecutive nightly runs;
  - the replay twins are in CI;
  - the legal items L1–L12 pass;
  - a full-history gitleaks scan is clean.

  Then the repository goes public and v0.1.0 is tagged.

**WS-09: MCP server** (M, D; depends on WS-01)
- **Scope:** rmcp 3.5.x serving both 2026-07-28 and 2025-11-25 [V19]; tools mapped onto JSON-RPC; tiers, approvals, audit, sanitization and rate limits (section 8.4); stdio by default, localhost HTTP as an opt-in.
- **Tests:** L4 (conformance in both eras), L5, E2E-13.
- **Done when:** conformance passes and the approval tests pass.

**WS-10: Extension v0** (L, D; depends on WS-01, using WS-02's mock daemon)
- **Scope:** device list view; Build, Run, Debug and Hot Reload commands and buttons; live logs panel; debug configuration provider; status bar; JSON-RPC client.
- **Tests:** L6a against the installed VS Code (V22).
- **Done when:** the integration suite passes against the mock daemon; E2E-05 passes through the extension in the lab.

**WS-11: Flutter** (XL, A+C; depends on WS-08)
- **Scope:** custom device; storyboard-free UIScene Runner (BSD-3); SwiftPM plugins; VM-service forwarding; the JIT RX-page helper with manual continue on iOS 27 [R:fx-21]; AOT with gen_snapshot built in CI later.
- **Tests:** L1 cassettes; E2E-08.
- **Done when (G2):** E2E-08 passes 3 runs out of 3 on iOS 27; DC-30 passes on the rig (TXM on iOS 27.0, verified 2026-10-11 from firmware); the execution mode is documented.

**WS-12: Mirror and input** (L, A+D; depends on WS-03)
- **Scope:** CoreDevice display stream with HEVC decoding (idevice `display_stream` [V13]; pymobiledevice3 as reference [V14]); frame transport to the IDE webview; HID touch and keyboard.
- **Tests:** replay of frames from our probe app; E2E-07.
- **Done when:** E2E-07 meets the fps budget.

**WS-13: Profiling and crash reports** (L, A+D; depends on WS-03 and WS-05)
- **Scope:** DVT sysmon, CPU, memory, energy and network; Perfetto/Chrome-trace export; crash report retrieval; symbolication with the local dSYM.
- **Tests:** L1 replay; E2E-09 and E2E-10.
- **Done when:** both pass.

**WS-14: Golden harness and oracle** (M plus ongoing, B+D; depends on WS-00)
- **Scope:** QA-11 and QA-12; the first corpus (plist, Mach-O, an actool subset, the launch-screen subset of ibtool, xcstrings).
- **Done when:** one case per family is generated through a backend and checked in L3.

**WS-15: Resource compilers** (XL, D+B; depends on WS-14 and WS-05)
- **Scope:** an actool-compatible CLI under Swift Build's contract (version plist, dependency info, partial Info.plist, GeneratedAssetSymbols [R:2.5]); launch-storyboard conversion and a subset compiler; xcstrings including `.stringsdict` and symbol generation.
- **Tests:** L3 corpus; E2E-15.
- **Done when (G4):** at least 40 required cases are equivalent; E2E-15 passes; the xtool #310 scenario builds.

**WS-16: Ship** (L, A+D; depends on WS-06, WS-05 and WS-15)
- **Scope:** lints, opt-in and visible metadata stamping, ASC build upload, TestFlight state polling, the Apple-hardware workflow generator, and `apple-release.yml`.
- **Tests:** `AscFake`; E2E-11 and E2E-16.
- **Done when (G5):** the canary is VALID and installed through TestFlight, through both the Linux-native and the Apple-hardware path.

**WS-17: Project editors and panels** (L, D; depends on WS-10, WS-12, WS-13 and WS-16)
- **Scope:** editors for Info.plist, signing and capabilities, entitlements, and asset catalog / app icon; the publishing panel; profiling graphs; the crash-report view; the mirror panel.
- **Tests:** L6a model tests and editor round-trips (the editor never drops unknown keys).
- **Done when:** all "first build" GUI features exist in the extension (decisions.md).

**WS-18: Agent panel and agent configurations** (M, D; depends on WS-09 and WS-10)
- **Scope:** writers for the MCP configuration of Claude Code, Codex CLI, Gemini CLI, local llama.cpp/OpenHands, Copilot, Cline and Continue; the approval UI; an audit viewer.
- **Tests:** generated configs are validated against each tool's documented format (agent area doc); a scripted MCP client smoke test.
- **Done when:** each agent lists Russet tools in a manual smoke run; the approval UI works.

**WS-19: IDE product build** (XL, C; depends on WS-17 and WS-18)
- **Scope:** VSCodium-style patches over a pinned Code-OSS tag (1.141.0 at the time of writing [V2]); branding; `product.json` (Open VSX, no telemetry, update URL); the built-in Russet extension; dedicated toolbar buttons; Node 24 pin.
- **Tests:** L6b smoke and E2E-19.
- **Done when (G6):** the IDE packages build in CI; the smoke tests pass; the first-build feature checklist is complete; L8 (branding) passes.

**WS-20: Release pipeline** (M, D; depends on WS-00 and WS-01; the IDE part also depends on WS-19)
- **Scope:** QA-18 to QA-20 (section 5).
- **Done when:** the `v0.1.0` and `v0.6.0` release flows are green.

**WS-21: Docs site** (M, D; depends on WS-01 and WS-09)
- **Scope:** QA-21 (section 6).
- **Done when:** the docs build passes its checks and the generated references are current.

**WS-22: Windows** (XL, A on the Windows PC + D; depends on WS-08, plus WS-11 for Flutter and WS-16 for shipping)
- **Scope:**
  1. W1 split mode: native device daemon and LLDB on Windows, build in WSL2.
  2. W2 native build plane: path limits, symlinks, Swift for Windows [R:5.1].
  3. MSI and winget packaging.
- **Done when (G7):** E2E-18 passes on the physical PC.

**WS-23: Xcode projects, CocoaPods and `russet test`** (XL, D+B; depends on WS-15 and WS-14 (PIF corpus))
- **Scope:** `.xcodeproj` and `.xcproj` → PIF; Pods; XCTest and XCUITest with JUnit output.
- **Done when (part of G8):** 3 apps build from `.xcodeproj` and run; an XCUITest suite produces JUnit.

**WS-24: React Native / Expo, and a .NET MAUI study** (XL; depends on WS-23)
- **Done when:** the RN template app runs from Linux; a MAUI feasibility note is written.

**WS-25: Security and legal (continuous)** (M; depends on WS-00)
- **Scope:** QA-16, QA-22 and QA-23; reviewing the threat model per release; the gates of section 9.

### 11.5 Gates and the tests behind them

| Gate | Milestone | Tests |
|---|---|---|
| G1 | M1, repository public | E2E-01 to 06 and 12 to 14 pass 3 times; L0–L6 green; legal L1–L12 pass |
| G2 | M2 | E2E-08 |
| G3 | M3 kit v0.3 | v0.3.0 packages; E2E-05 through the extension; docs live |
| G4 | M4 | L3 required cases; E2E-15 |
| G5 | M5 | E2E-11 and E2E-16; `apple-release.yml` VALID |
| G6 | First IDE build | IDE packages; L6b; E2E-19; L8 |
| G7 | M6 | E2E-18; MSI install and uninstall clean |
| G8 | M7 | WS-23 and WS-24 criteria |

---

## 12. QA task list (ordered)

These are the tasks this area owns; the structured summary repeats them. Every task's acceptance criteria include "`cargo xtask ci --profile pr` green".

| ID | Task | Acceptance (summary) | Size | Depends on |
|---|---|---|---|---|
| QA-01 | Bootstrap the toolchain and repository configuration:<br>• `rust-toolchain.toml` (1.99.0, minimal, clippy and rustfmt);<br>• `.cargo/config.toml` (xtask alias, lld, line-tables-only);<br>• workspace skeleton;<br>• nextest config;<br>• `deny.toml`, `REUSE.toml` and `LICENSES/`;<br>• `typos.toml`, `.gitattributes` (LF; `*.rdt.jsonl.zst binary`), `.editorconfig`;<br>• fix `.gitignore` (`.orchard/` → `.russet/`).<br><br>Owner installs the tools with `sudo pacman -S --needed …` (Appendix A). | `cargo xtask ci` works without a rustup default; `reuse lint` is clean | S | — |
| QA-02 | `xtask guard`: apple-files (extensions, magic numbers, Apple copyright header patterns), secrets (gitleaks), traces, REUSE, typos, zizmor, DCO, commit messages | Seeded bad fixtures are all caught; the clean tree passes; runs in under 30 s | M | QA-01 |
| QA-03 | CI workflows: `ci.yml`, `nightly.yml`, stubs of `oracle.yml`, `lab.yml` and `release.yml`; the `upload-safe` composite action; Dependabot with SHA pins | Green on GitHub; zizmor reports no findings at medium or above | M | QA-02 |
| QA-04 | Governance files (section 7) | Files present; the DCO is enforced | S | QA-01 |
| QA-24 | `xtask disk-guard` (tiers in section 10); `cargo sweep` nightly; AGENTS.md integration | Heavy commands refuse to run below their tier; the target directory stays at 6 GiB or less | S | QA-01 |
| QA-05 | Implement T1–T10 in `russet-core` with reference implementations, plus the doubles in the testkit | Each seam is used by at least one crate; documented | M | WS-01 |
| QA-06 | RDT writer, reader, redactor and trace lint | Property round-trip passes; seeded PII is caught; size budgets are enforced | M | QA-05 |
| QA-07 | `TapStream`, `RecordingConnector`, `ReplayConnector` (strict, semantic, lenient) and the protocol masks | A synthetic trace drives `russet-device` to the same results; mismatch errors name the connection and offset | L | QA-06, WS-03 interfaces |
| QA-09 | `CassetteRunner`; `AscFake`, `GsaFake`, `TssFake` and `AnisetteFake`; offline enforcement | Provisioning idempotency tests pass offline; no egress in L1–L6 | M | QA-05 |
| QA-10 | Contract tests: schema generation and `--check`; insta snapshots; MCP conformance in both eras; DAP script | Drift fails CI; conformance passes | M | WS-01, WS-09 |
| QA-08 | `russet-mockdevice` (fake usbmuxd, lockdown with TLS, scripted services) and the pymobiledevice3 differential test | idevice and pymobiledevice3 read the same values from the mock on Linux; the Windows TCP address works | L | QA-07 |
| QA-15 | Extension test infrastructure: vitest; `@vscode/test-cli` with `fromPath`; mock daemon; xvfb | The device view is populated from the mock in CI | M | WS-10, QA-08 |
| QA-11 | `russet-golden`: plist and Mach-O normalizers first, CAR and NIB once parsers exist; corpus schema; `KNOWN_DIFFERENCES.md` | `russet-golden check` runs in L3 | M | QA-05 |
| QA-12 | `russet-oracle` (`ssh` and `gha` backends), `oracle.yml`, compare-on-oracle mode | One actool case and one plist case generated end to end; only normalized files uploaded (guard proves it) | M | QA-11 |
| QA-13 | `russet-lab` harness, budgets, reports; the RussetProbe and RussetObjCMini fixtures | The `smoke` suite passes 3 times in a row on the iPhone; cleanup verified | L | WS-03 (A) |
| QA-14 | On-demand lab runs (`cargo xtask lab`, private phase; decisions.md: no always-on runner); the lab-bridge repository and PATs only if the owner later wants an always-on lab | Lab reports arrive in `docs/lab/results/`; with a bridge, fork PRs cannot trigger the lab | M | QA-13 |
| QA-16 | Fuzz targets (section 3.3); nightly fuzz job; triage process | 12 targets run nightly; crashes become regression tests | M | QA-06, WS-03/05 |
| QA-17 | Performance budgets: record, then enforce | The baseline is set after 3 runs; a regression of more than 25 % fails the nightly run | S | QA-13 |
| QA-21 | Docs site and reference generators | `xtask docs check` is green; the Pages artifact is built | M | WS-01, WS-09 |
| QA-22 | Security tests: secret store, redaction, IPC peer checks, MCP tiers, approvals and audit; threat model document | All section 8.4 cases pass; the threat model is reviewed by the owner | M | WS-01, WS-09 |
| QA-18 | CLI release pipeline: versioning, git-cliff, `xtask release`, tar.gz, zip, deb, rpm, MSI, minisign, SBOM, source tarball; container smoke tests | An `rc` dry run produces every artifact; checksums verify; installs succeed | L | QA-03, WS-01 |
| QA-23 | `xtask legal-check` plus the manual checklist (section 9) | Passes on the release candidate | M | QA-18 |
| QA-19 | AUR and winget automation (public phase) | `russet-bin` builds in an archlinux container (namcap clean); `winget validate` passes | S | QA-18, G1 |
| QA-20 | IDE packaging and smoke tests (with WS-19): AppImage, deb, rpm, tar.gz, the Windows installers; `product.json` and branding checks | The G6 checklist passes | L | WS-19 |

---

## 13. Risks

| ID | Risk | Mitigation |
|---|---|---|
| R-QA1 | **A single device** (iPhone 12, iOS 27.0 only; it runs TXM on iOS 27.0, verified 2026-10-11 from firmware) means no coverage of iOS 26 (including the iOS 26 TXM build of A15+ phones), 17 or betas, of non-TXM behaviour, or of Cryptex-only iPhone 18 hardware [R:dv-07] | Replays from community devices once public; DC-30's negative control on each new iOS; a second device, ideally an A15+ phone on 26.x or a beta (decisions.md: iPhone 12 only for now) |
| R-QA2 | **The oracle is unavailable** (V7), or the macOS runner's Xcode is a beta [V15] | Use the `gha` backend; record the Xcode build in `oracle.json` and key expectations by build |
| R-QA3 | **The self-hosted lab is exposed** once the repository is public | The lab bridge in a private repository; dispatch only (section 4.5) |
| R-QA4 | **Private-phase limits:** cost, and no attestations, environment approvals or SignPath [V16][V17] | Go public at G1 |
| R-QA5 | **PII in traces** (UDID, names, Apple ID) | Redaction policy; trace lint; the owner's denylist; only lane A commits traces |
| R-QA6 | **Replays are brittle** because of nondeterministic fields (XPC message IDs, DTX) | Semantic mode with masks; re-record and drift tooling |
| R-QA7 | **Legal ambiguity of oracle outputs** | Only semantic output is public; raw output stays local; counsel (open question 5) |
| R-QA8 | **Apple-account side effects in tests:** certificate churn, App ID limits, TestFlight build numbers | Idempotency (E2E-12); dry-run by default; the canary only by manual dispatch |
| R-QA9 | **Disk volatility** (V8) | Tier gates; shared target directory; CI for heavy builds |
| R-QA10 | **RAM contention** with llama.cpp | Job caps, cgroup limits, a concurrency limit for cargo |
| R-QA11 | **The Windows signing identity:** an individual outside the USA and Canada cannot use Azure Artifact Signing [S26] | SignPath once public; OV certificate as fallback |
| R-QA12 | **WiX licensing:** v7 requires accepting the OSMF EULA; the fee applies only above $10,000 a year in revenue [S27] | Decide in open question 11: WiX v3.14 (VSCodium), v7 with EULA acceptance, or msitools |
| R-QA13 | **idevice API churn** [R:dv-29] | Exact pin; T5 isolation; L5 and L7 on upgrades |
| R-QA14 | **MCP spec churn:** the stateless revision 2026-07-28 | Dual-era conformance in CI |
| R-QA15 | **Code-OSS build limits** on 2-vCPU, 8 GB private runners [V18] | Build after going public (4 vCPU, 16 GB), or locally at tier BLUE |
| R-QA16 | **Antivirus or PUA flags** on Windows for a device-tunnel tool [S28] | Signing; false-positive submissions |
| R-QA17 | **Multi-agent contention:** target-directory locks, device collisions | Lanes, reservations, locks |
| R-QA18 | **Flaky device tests** (USB, trust prompts) | Exit code 3 for skipped-env; one retry; quarantine label; the reconnect scenario |

---

## 14. Open questions (for the owner or lead)

1. **GitHub plan.** Is it Free or Pro? **Answered 2026-10-11:** GitHub Free (V6).
2. **CI budget.** **Answered by decisions.md:** the free private-repo minutes only (section 4.8). Going public right after G1 stays the recommendation (ARCHITECTURE §21 Q6).
3. **Self-hosted lab runner.** **Answered by decisions.md:** no always-on self-hosted runner; device suites run on demand, triggered by the device-lane agent.
4. **Windows signing.** SignPath Foundation, with "SignPath Foundation" as publisher, or an OV certificate in the owner's name (about $150–300 a year, hardware token)?
5. **Legal: oracle outputs.**
   - Is it acceptable to publish *normalized semantic dumps* derived from Apple-tool outputs?
   - Is keeping raw outputs locally acceptable?
   - Which jurisdiction applies? The timezone is CEST, which matters for the EU interoperability exception [R:Q4].
6. **Name clearance** for "Russet" in software classes 9 and 42, before 1.0.
7. **Licence for templates and in-app code:** 0BSD (proposed) or MIT?
8. **The paid ASC key in lab and CI secrets** during the private phase (repository secret): acceptable? Not needed in the private phase (section 4.6); it would matter only for the `xcode-manual`/`xcode-cloud` signing modes.
9. **A second test device:** is one available, ideally on an older iOS or a beta? **Answered by decisions.md:** iPhone 12 only (it runs TXM on iOS 27.0, so TXM is covered; ARCHITECTURE §21 Q20).
10. **Disk cleanup.** May we clean the cleanup candidates of V9 and section 10? This agent deleted nothing.
    - `~/.cache/paru/clone` (17 GiB); check `xodus-git` for local work first;
    - `rust-docs` (719 MiB);
    - `~/.cache/xodus-launcher` (6.7 GiB);
    - `~/.cache/Shelly` (5.7 GiB).
11. **MSI tooling:** WiX v3.14 (as VSCodium does), WiX v7 (OSMF EULA, no fee under $10,000 a year in revenue), or msitools `wixl`?
12. **Mac oracle availability.** Can the owner keep the Mac awake or reachable during oracle windows? It was unreachable in this session [V7].
13. **Copilot.** The Copilot Chat repository is archived [V23]. The agent area must confirm how Copilot integrates with a Code-OSS build and whether its terms allow it.
14. **Publishing the extension.** Should we also publish the Russet extension to Open VSX and the VS Code Marketplace, so that VS Code users can use it without the Russet IDE? The Marketplace terms restrict *consumption* by non-Microsoft products [S21]; publishing an extension there for VS Code users is a separate question [A].

---

## Appendix A: commands run in this session

All of these were read-only, except creating `docs/design/` and writing this file. No device commands were run, nothing was changed on the Mac, and nothing was deleted.

```sh
rustup toolchain list; RUSTUP_TOOLCHAIN=stable rustc -Vv; rustup component list --installed     # V1
node -v; npm -v; gh api repos/microsoft/vscode/contents/.nvmrc; command -v code                  # V2
ls /usr/lib/swift/usr/bin; cat ~/.swiftpm/swift-sdks/darwin.artifactbundle/darwin-sdk-version.txt # V3
pacman -Si cargo-nextest cargo-deny cargo-about cargo-llvm-cov git-cliff reuse typos zizmor gitleaks \
  mdbook lychee minisign xorg-server-xvfb mold cargo-insta cargo-fuzz cargo-machete cargo-auditable msitools  # V4
python3 -I crates.py <crate…>      # crates.io API: versions, licences, dates                      # V5
gh repo view evilcomputer12/russet --json …; gh auth status                                         # V6
ssh -o BatchMode=yes -o ConnectTimeout=12 mac 'echo ok'   # → connection timed out (twice)          # V7
df -h /home   # 15G avail (≈00:15) → 98G avail (00:30, 00:30:22 CEST)                             # V8
du -sh --apparent-size ~/.cache/paru/clone/*; du -sh ~/.rustup/toolchains/stable-*/share/doc       # V9
gh api -H 'Accept: application/vnd.github.raw' repos/jkcoxson/idevice/contents/idevice/src/{lib.rs,provider.rs,sni.rs,usbmuxd/mod.rs,usbmuxd/server.rs}  # V10–V12
gh api … repos/jkcoxson/idevice/contents/idevice/Cargo.toml                                          # V13
grep -n USBMUXD_SOCKET_ADDRESS ~/.local/share/uv/tools/pymobiledevice3/lib/python3.14/site-packages/pymobiledevice3/usbmux.py  # V12
sed -n 1,30p …/pymobiledevice3/remote/core_device/hid_service.py; ls …/remote/core_device/          # V14
gh api repos/modelcontextprotocol/rust-sdk/releases; gh api … crates/rmcp/src/model.rs | grep V_20  # V19
gh api repos/modelcontextprotocol/conformance                                                        # V19
gh api … repos/VSCodium/vscodium/contents/prepare_vscode.sh | grep open-vsx                          # V20
gh api repos/microsoft/vscode/contents/test; …/test/mcp/README.md                                    # V21
gh api … repos/microsoft/vscode-test-cli/contents/src/config.cts | grep -A6 useInstallation         # V22
gh api repos/microsoft/vscode-copilot-chat --jq '{license,archived,pushed_at}'                      # V23
```

**Owner's one-time tool install** (about 200 MiB; needs sudo):

```sh
sudo pacman -S --needed cargo-nextest cargo-deny cargo-about cargo-llvm-cov git-cliff reuse typos zizmor \
  gitleaks mdbook lychee minisign xorg-server-xvfb cargo-insta cargo-fuzz cargo-machete cargo-auditable msitools
```

---

## Sources

Every URL was accessed on 2026-10-11.

- [S1] Russet decisions: `docs/decisions.md`.
- [S2] Russet verified facts: `docs/verified-facts.md`.
- [S3] `demo/m1-baseline.sh`.
- [S4] Landscape report: `docs/research/landscape-2026-10.md`. [R:…] IDs point into it.
- [S5] idevice source: https://github.com/jkcoxson/idevice
  - `idevice/src/lib.rs`: `ReadWrite`, `IdeviceService`, `RsdService`, `Idevice::new`, `get_socket` and `start_session`;
  - `sni.rs`;
  - `provider.rs`;
  - `usbmuxd/mod.rs`;
  - `usbmuxd/server.rs`;
  - `Cargo.toml` features.

  Master was pushed 2026-10-08; the crate version is 0.1.68.
- [S6] pymobiledevice3 11.26.0 as installed: `…/site-packages/pymobiledevice3/usbmux.py` and `remote/core_device/{hid_service,screen_stream,screen_capture_service,vnc_server}.py`. Upstream: https://github.com/doronz88/pymobiledevice3
- [S7] crates.io API: `https://crates.io/api/v1/crates/<name>` (queried by script; results in V5).
- [S8] GitHub runner images: https://github.com/actions/runner-images. Xcode 27 preview announcement: https://github.com/actions/runner-images/issues/14404
- [S9] GitHub Actions billing: https://docs.github.com/en/billing/concepts/product-billing/github-actions. Runner pricing: https://docs.github.com/en/billing/reference/actions-runner-pricing
- [S10] GitHub-hosted runner hardware: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- [S11] Environments and plan limits: https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments
- [S12] Secure use of Actions (self-hosted runners and public repositories, SHA pinning, least privilege, `pull_request_target`): https://docs.github.com/en/actions/reference/security/secure-use
- [S13] Artifact attestations (private repositories need Enterprise Cloud): https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations
- [S14] 2026 Actions pricing changes and the postponed self-hosted fee: https://resources.github.com/actions/2026-pricing-changes-for-github-actions/ and https://devclass.com/2025/12/17/github-to-charge-for-self-hosted-runners-from-march-2026/
- [S15] MCP specification 2026-07-28:
  - Tools: https://modelcontextprotocol.io/specification/2026-07-28/server/tools
  - Versioning: https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning
- [S16] rmcp: https://github.com/modelcontextprotocol/rust-sdk (`crates/rmcp/src/model.rs` `ProtocolVersion`; release `rmcp-v3.5.1`, 2026-10-05).
- [S17] MCP conformance framework: https://github.com/modelcontextprotocol/conformance
- [S18] VSCodium:
  - Build documentation: https://github.com/VSCodium/vscodium/blob/master/docs/howto-build.md
  - `prepare_vscode.sh` (Open VSX gallery): https://github.com/VSCodium/vscodium/blob/master/prepare_vscode.sh
  - Linux CI workflow: https://github.com/VSCodium/vscodium/blob/master/.github/workflows/ci-build-linux.yml
- [S19] Code-OSS: https://github.com/microsoft/vscode (`.nvmrc`, `test/smoke`, `test/automation`, `test/mcp`; release 1.141.0).
- [S20] `@vscode/test-cli` configuration: https://github.com/microsoft/vscode-test-cli/blob/main/src/config.cts
- [S21] Visual Studio Marketplace Terms of Use: https://aka.ms/VSMarketplace-TOU. Secondary coverage: https://www.nuget.org/policies/Terms
- [S22] Apple trademark guidelines for third parties: https://www.apple.com/legal/intellectual-property/guidelinesfor3rdparties.html
- [S23] SF Symbols licence scope (forum discussion; check against the licence shipped with Xcode): https://developer.apple.com/forums/thread/757407
- [S24] REUSE Specification 3.3: https://reuse.software/spec-3.3/
- [S25] SignPath Foundation terms: https://signpath.org/terms
- [S26] Microsoft, code signing options for Windows apps (updated 2026-08-29): https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options
- [S27] WiX Open Source Maintenance Fee: https://docs.firegiant.com/wix/osmf/
- [S28] winget-pkgs validation: https://github.com/microsoft/winget-pkgs/blob/master/doc/Validation.md
- [S29] AppImage type2-runtime: https://github.com/AppImage/type2-runtime
- [S30] cargo-nextest test groups: https://nexte.st/docs/configuration/test-groups/
- [S31] GNU GPL v3 (section 6, Corresponding Source): https://www.gnu.org/licenses/gpl-3.0.html
- [S32] Developer Certificate of Origin 1.1: https://developercertificate.org/
- [S33] Conventional Commits 1.0.0: https://www.conventionalcommits.org/en/v1.0.0/
- [S34] Contributor Covenant 3.0: https://www.contributor-covenant.org/ (announcement: https://ethicalsource.dev/blog/contributor-covenant-3)
- [S35] Arch Linux `extra` package metadata (`pacman -Si`, 2026-10-11).
- [S36] `microsoft/vscode-copilot-chat` (archived): https://github.com/microsoft/vscode-copilot-chat
- [S37] zizmor: https://github.com/zizmorcore/zizmor
- [S38] Open VSX: https://open-vsx.org
- [S39] AUR submission guidelines (could not be fetched automatically; the site blocked the request): https://wiki.archlinux.org/title/AUR_submission_guidelines
- [S40] Runner images, re-checked 2026-10-11: https://github.com/actions/runner-images (README; `images/macos/xcode-27-arm64-Readme.md`, `macos-26-arm64-Readme.md`; `images/windows/Windows2025-VS2026-Readme.md`, `Windows2022-Readme.md`; `images/ubuntu/Ubuntu2404-Readme.md`); issues #14404, #14167, #14748, #14856; changelog https://github.blog/changelog/2026-07-16-xcode-27-runner-image-now-in-public-preview/
- [S41] GitHub docs: runners https://docs.github.com/en/actions/reference/runners/github-hosted-runners, larger runners https://docs.github.com/en/actions/reference/runners/larger-runners, limits https://docs.github.com/en/actions/reference/limits, runner pricing https://docs.github.com/en/billing/reference/actions-runner-pricing, Actions billing https://docs.github.com/en/billing/concepts/product-billing/github-actions; the removal of the 2×/10× wording: github/docs commit ce33310edb (2025-12-16); merge queues need an organization-owned repository (github/docs `data/reusables/gated-features/merge-queue.md`)
