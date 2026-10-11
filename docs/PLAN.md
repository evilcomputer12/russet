# Russet implementation plan

- **Status:** v1.2, 2026-10-11, lead architect. Review round 1 is applied ([critique](review/critique-2026-10-11.json); ARCHITECTURE §20 D 62–D 81, §21 Q25–Q33). Review round 2 is applied (ARCHITECTURE §20 D 82–D 91, §21 Q34); its critique is not committed because it quotes the private track's details.
- **Contracts** (names, APIs, wire formats, files, errors) are in [ARCHITECTURE.md](ARCHITECTURE.md), which is binding. This file says **who builds what, in which order, on which paths, with which acceptance test**.
- **Shape:**
  - 38 cards in 6 waves plus one parallel track: `W1`…`W32`, with W27 split into `W27a`–`W27c`, plus `W16b`, `W21b`, `W28b` and `W28c`. `W33` is a deferred card (§10.2).
  - Each workstream is sized for one implementation agent in its own git worktree, on a disjoint set of paths.
  - Area task IDs (`CORE-…`, `DEV-…`, `B…`, `SA-…`, `RD-…`, `IDE-/EXT-/MCP-/AGT-/SPK-…`, `QA-…`) refer to the task lists in `docs/design/*.md`. §15 maps every one of them to a workstream.
- **Sizes** are agent-days (one agent, full speed): S ≤ 2, M 3–5, L 6–10, XL > 10.

---

## 0. Rules for every implementation agent

1. **Worktree and branch.**
   - Create: `git -C ~/russet worktree add ../russet-wNN -b ws/wNN-<slug> main`.
   - Commit only to your branch, using Conventional Commits. Only the lead merges to `main`.
   - Rebase on `main` at least daily.
2. **Paths.**
   - Edit only the paths your card owns. They are mirrored in `.config/workstreams.toml`, which W1 creates; `cargo xtask check-paths --ws WNN` warns on foreign edits.
   - **Seam files** are lead-only after W1 merges: the root `Cargo.toml`, `crates/{russet-protocol,russetd,russet-cli}/Cargo.toml`, `crates/russet-protocol/src/{lib,registry,macros}.rs`, `crates/russetd/src/handlers/mod.rs`, `crates/russetd/src/doctor_registry.rs`, `crates/russet-cli/src/cmd/mod.rs`, `crates/russet-core/src/error/codes/mod.rs`, `xtask/src/main.rs`, `xtask/Cargo.toml`, `docs/ARCHITECTURE.md`. W1 pre-registers every `cargo xtask` subcommand (ARCHITECTURE §4) in `main.rs` as a stub that calls its own module file, so W2, W3, W8 and W9 only fill their modules [D 86]. `extension/package.json` becomes lead-only once W14 merges it in Wave 2.
   - **Module files across owners.** W1 pre-creates every module file whose owner differs from its crate root's owner, with the `mod` line in the parent (§3 lists them), so nobody edits another card's file to declare a module [D 86].
   - **Tests, benches and examples** of a crate belong to the crate's owner. In the shared crates (`russetd`, `russet-cli`, `russet-protocol`, `russet-core`) each card puts its integration tests in `tests/<area>_*.rs` (for example `crates/russetd/tests/pipeline_*.rs` for W7), which that card owns.
   - A new third-party dependency goes into your crate's `Cargo.toml` with the version from ARCHITECTURE §16.7 or a crates.io-checked version, licence-checked with `cargo deny check licenses`; the lead hoists it into `[workspace.dependencies]` at merge.
   - **Generated files** (`schemas/**`, `extension/src/generated/**`, `docs/reference/**`): regenerate with `cargo xtask schema` and commit. If they conflict, regenerate; never hand-merge. **`Cargo.lock`** is generated too: on a conflict take `main`'s copy and let cargo re-add your branch's entries (`cargo metadata --format-version 1 >/dev/null`); never resolve it by hand or with `cargo update`.
3. **Contracts.**
   - Follow ARCHITECTURE.md exactly (§5.3 APIs, §8 method names, §9 events, §13 codes, §14 files).
   - If a contract is wrong, open a "contract change" PR touching only `docs/ARCHITECTURE.md`, with the lead as reviewer. Until it merges, code against the current contract.
4. **Device rule.**
   - Only the **lane-A** agent (§2) may talk to the iPhone. That covers every `russet device|app|run|debug|logs|…` command against a real device, idevice tools, pymobiledevice3, LLDB against the phone, and `cargo xtask lab`.
   - Everyone else develops against `RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>`, replay traces or the mock device, and files **device checks** in `docs/lab/queue.md` (§11).
   - **Daemon isolation** (ARCHITECTURE §3.3 item 7, [D 63, D 83]). Every lane-D worktree exports `RUSSET_HOME=<worktree>/.russet-dev` (W1's `AGENTS.md` and `.envrc.example` set it), so each worktree runs its own daemon, built from its own branch, with its own keyring entry. Lane-D worktrees also export `RUSSET_DEVICE_BACKEND=scripted:<scenario.toml>`.
   - **Lane A** exports `RUSSET_LANE=A` and the persistent `RUSSET_HOME=$HOME/russet-lab/home-A`, shared by every lane-A card in turn (W4, W9, W10, W18, the Wave-4 checks, W30). It keeps `[device.ddi] dir`, the imported accounts and the install ledger between cards [D 82].
   - Tests use the testkit's `TempHome` and `RUSSET_SECRETS_BACKEND=memory` (or `passphrase`), so every test daemon has its own socket and no test touches a keyring entry it did not create. Tests of the live-device lock use the debug-only `RUSSET_LIVE_DEVICE_LOCK` override and a test-local fake usbmuxd, never `RUSSET_LANE=A`.
   - A daemon on the `live` backend holds the host-wide live-device lock, so the device lane stays serialized even across worktrees; never delete or bypass that lock.
5. **Mac rule.**
   - Only lane B runs `ssh mac`, and only read-only.
   - Prefix tool calls with `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`.
   - Work only in a `mktemp -d` directory with an EXIT-trap cleanup.
   - No installs, no settings changes, no SIP changes. Keep load light (8 GB RAM).
   - Ask the owner before deleting anything on the Mac (decisions.md).
   - The Mac also hosts the private virtual-iPhone track's VM, operated by another workflow. Lane-B windows are owner-approved and never overlap that VM's use; heavy oracle work (for example `ibtool` compile runs, which start Interface Builder's helper processes) runs only in such windows. Never touch the VM.
6. **Disk rule.**
   - Run `cargo xtask disk-guard --need <GiB>` before heavy work (before W8 lands: `df -h /home`). Tiers: RED < 8 GiB, AMBER 8–25, GREEN 25–45, BLUE ≥ 45 with a 20 GiB reserve [QA §10]. The lead measured 98 GiB free on 2026-10-11, but the task brief reported about 12 GB and shrinking (an unrelated game download), so plan for **AMBER** in Waves 1–2 and re-measure before anything heavy.
   - Never download more than 500 MB without the owner's explicit OK, whatever your card or the tier says [D 81].
   - Delete your own scratch output. Never touch files Russet did not create.
7. **Build hygiene.**
   - `CARGO_TARGET_DIR=$HOME/.cache/russet-dev/target-<slot>`, with the slot (`a` or `b`) given on your card. At tier AMBER only slot `a` exists: every card uses it, and the target lock serializes builds. Slot `b` opens from tier GREEN; then at most 2 cargo builds run machine-wide [D 81].
   - `CARGO_BUILD_JOBS=6`; `CARGO_INCREMENTAL=0` (locally too); `[profile.dev] debug = "line-tables-only"`; `cargo sweep --time 7` weekly (W8 automates it).
   - Once W8 merges, run cargo through `cargo xtask` (`ci`, `test`, `build`), which runs disk-guard first.
   - The machine shares RAM with a llama.cpp server. Do not run two Swift builds at once.
8. **Environment quirks.**
   - The Bash tool runs zsh: quote globs, and never assign `GID`.
   - Until W1 merges: `cargo +stable …` (rustup has no default toolchain).
   - Until W3 lands, Swift's LLDB needs `LD_LIBRARY_PATH=$HOME/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu/lib PYTHONHOME=$HOME/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu` (uv's python-build-standalone 3.12.15, verified with Swift LLDB). `~/.local/opt/python-3.12` is being retired (§14).
   - Swift 6.4 is at `/usr/lib/swift/usr/bin`; the Darwin SDK is `~/.swiftpm/swift-sdks/darwin.artifactbundle`, which is read-only to Russet.
9. **Secrets and PII.**
   - Never print or commit keys, tokens, UDIDs, team IDs, Apple IDs or device names.
   - Tests use synthetic PKI and redacted fixtures.
   - Real owner artifacts (the M1 app, its profile, xtool's files) may be read only by local, env-gated tests that are never committed.
10. **Definition of done** for every task:
    - `cargo xtask ci --profile pr` is green: fmt, clippy `-D warnings`, nextest, deny, guard, check-layers, schema drift;
    - new methods are registered in `russet-protocol` and the schemas regenerated;
    - new errors have a code, a hint and a docs anchor;
    - environment-dependent features have a doctor check;
    - CLI commands support `--json`;
    - SPDX headers are present; no Apple files; no secrets in logs (canary test);
    - the area doc section is updated if behaviour differs from it.

---

## 1. Waves at a glance

| Wave | Goal and exit gate | Workstreams (∥ = parallel) | Lane A (device) | Disk tier |
|---|---|---|---|---|
| **1** | **Foundations + SwiftUI vertical slice.** `russet debug` builds, signs, installs, launches and debugs RussetProbe with Russet's own code: no xtool install step, no pymobiledevice3 (§4.10). DDI provenance (a DDI that Russet mounted from an Apple-origin source, DC-28) passes, or is recorded as blocked on the owner (Q33) | W1 → (W2 ∥ W3 ∥ W4 ∥ W5 ∥ W6 ∥ W7 ∥ W8) → W9 | W4, then W9 | AMBER |
| **2** | **M1 complete, agent-ready.** F5 in VS Code (extension Tier 0); MCP v1 with Claude Code; mirror, HID, crashes, perf in the CLI; native DDI and SDK import (DDI first); the one exact bundle ID decisions.md allows; record/replay; the Apple-hardware smoke; gate G1 candidate | W10 ∥ W11 ∥ W12 ∥ W13 ∥ W14 ∥ W15 ∥ W16 ∥ W16b | W10 (+ checks for W11–W16b) | AMBER |
| **3** | **M2 Flutter (TXM page coverage proven on the rig) + M3 kit + first-build panels.** Flutter debug and hot reload; mirror, editors, crash and profiler panels; agent panel and approvals UI; CAR writer; opt-in free Apple ID | W17 ∥ W18 ∥ W19 ∥ W20 ∥ W21 ∥ W22 | W18 (+ checks for W19–W22) | GREEN (Flutter SDK) |
| **4** | **M4/M5 ship + first IDE build (G6).** TestFlight VALID and install; App Store version and submission paths; Flutter AOT; launch-storyboard compiler; SwiftPM XCTest on device; IDE packages from CI; CLI packages | W23 ∥ W24 ∥ W25 ∥ W26 ∥ W21b ∥ W32 | checks for W21b, W23–W25, W32 | AMBER locally; IDE Tier-2 build in CI (BLUE only if local) |
| **5** | **M6 Windows** on the physical PC | W27a → (W27b ∥ W27c) | Windows PC lane (over SSH, Q31) | AMBER on the PC |
| **6** | **M7 ecosystem + Wi-Fi** | W28b → W28 → (W28c ∥ W29), all ∥ W30 [D 90] | W30 (+ checks for W28b, W28, W28c, W29) | GREEN |
| ∥ | **Virtual iPhone**: a separate private track (§10.1); never blocks | W31 | — | — |

**Gates** [QA §11.5]:
- G1 = end of Wave 2: M1 through Russet code, repository may go public (owner Q6).
- G2 = Flutter (Wave 3).
- G3 = kit v0.3 (Wave 3).
- G4 = resource compilers (W21 in Wave 3, W21b in Wave 4).
- G5 = ship (Wave 4).
- G6 = first IDE build (Wave 4).
- G7 = Windows (Wave 5).
- G8 = M7 (Wave 6).

```text
W1 (F0 stubs) ─┬─ W2 ─┐
               ├─ W3 ─┤
               ├─ W4 ─┤ (lane A)
               ├─ W5 ─┼─► W9 (lane A) ══ Wave 1 exit ══► Wave 2: W10(A) W11 W12 W13 W14 W15 W16(B) W16b ══ G1 ══►
               ├─ W6 ─┤      Wave 3: W17(C) W18(A) W19 W20 W21(B) W22 ══ G2/G3 ══► Wave 4: W23 W24(CI) W25(C/CI) W26 W21b(B) W32
               ├─ W7 ─┤      ══ G4/G5/G6 ══► Wave 5: W27a → (W27b ∥ W27c) (Windows PC) ══ G7 ══► Wave 6: W28b(B) → W28 → (W28c(B) ∥ W29); W30(A)
               └─ W8 ─┘      W31 (virtual iPhone, separate private repository) runs alongside; W33 is deferred
```

---

## 2. Lanes, resources and scheduling

| Lane | Resource | Concurrency | Who |
|---|---|---|---|
| **A: device** | The iPhone 12 (iOS 27.0) on USB; later the Windows PC with the phone moved there | **1 agent** at a time, enforced by the live-device lock (ARCHITECTURE §3.3 item 7); device sessions are batched | W4 → W9 → W10 → W18 → (Wave 4 checks) → W27a–c (Windows, over SSH) → W30 |
| **B: oracle** | `ssh mac` (read-only; owner-approved windows that never overlap the Mac's VM use). The GitHub `xcode-27` runner (macOS 27.0.1 26A434 + Xcode 27.0 27A266a, the same builds as the Mac) only once the repository is public, or per run with the owner's OK (decisions.md) | 1 | the lead (the Wave-1 DDI copy: a pre-task in an owner-approved window, Q33; it gates DC-28, not W4's start [D 82]), W16 (corpus/oracle), W16b (DDI and `.pkg` copies), W21 (actool/xcstrings corpus, SPK-04), W21b, W28b and W28c (IB corpus) |
| **C: heavy disk** | Flutter SDK (~3 GB), Code-OSS builds, gen_snapshot builds | 1, and only at tier GREEN/BLUE or in CI | W17, W24 (CI), W25 (CI once public; Tier-1 repack locally) |
| **D: general** | CPU/RAM; the cargo target slots (one at tier AMBER, two from GREEN) | 5 in Wave 1 (6 tolerable: builds serialize on the slots); ≤ 4 recommended later | everything else |

**Scheduling advice for Wave 1** (7 parallel workstreams after W1) [D 74]:
- W1 lands every ARCHITECTURE §5.3 signature as compiling stubs (freeze F0, §12), so nobody waits for a type to exist.
- **Day 0 after W1 merges:** W2, W3, W5, W6 and W7 in lane D (five agents, within the tolerable six) and W4 in lane A. W8 has float and starts in the first slot that frees.
- **With only four lane-D agents,** defer W7 (its day-0 work, RD-01 to RD-03, needs no F2 and can start late), never W6: W5's `SigningPlan` (B06) and W7's pipeline are coded against W6's day-4 contract PR (F2).
- Critical path: W1 → W2 (F1, day 2) → the F2 contract PRs of W3–W6 (day 4) → W6's signer and W7's pipeline → W9.
- **Merge order:** W2 merges first among W2–W7. Every CLI-level acceptance of W3–W7 (`russet … --json`) needs W2's daemon and CLI, not just F1, so it is checked after W2 has merged; until then those cards test their crates and handlers in-process.
- W8 has float, but W9 needs its testkit and CI, so W8 must merge before W9 starts (target: by the time W7 merges).

**Cargo target slots:** `a` = W1, W2, W3, W8, W13, W15, W16b, W20, W26, W32; `b` = W4, W5, W6, W7, W9, W10, W11, W12, W16, W17, W18, W21, W21b, W22, W23, W24, W28, W28b, W28c, W29, W30. At tier AMBER every card uses `a` (§0 rule 7). npm-only work (W14, W19, W25) needs no slot; W27a–c build on the Windows PC. Each slot is capped at 6 GiB by the nightly `cargo sweep` (W8).

**The device queue** (`docs/lab/queue.md`, owned by the current lane-A workstream):
- Any workstream appends a check with: id `DC-nn` (§11), requester, the exact commands or script, the expected result, and the commit to test.
- Lane A runs queued checks in batches (at least once per working day while a wave is active).
- Lane A records results in `docs/lab/results/<date>-DC-nn.md`: redacted, with no UDIDs or team IDs.
- A failed check goes back to the requester with logs.

---

## 3. Path ownership

- `.config/workstreams.toml` (W1) is the machine-readable copy of this table. Paths are repository-relative globs.
- **Within a wave, owned paths are disjoint.** Across waves a path can change owner after the earlier workstream merged.

| WS | Owns (Wave 1) |
|---|---|
| W1 | root config files (`Cargo.toml`, `Cargo.lock` (generated afterwards, §0 rule 2), `rust-toolchain.toml`, `.cargo/`, `deny.toml`, `about.toml`, `REUSE.toml`, `LICENSES/`, `typos.toml`, `.gitattributes`, `.editorconfig`, `.config/`), `AGENTS.md`, `CLAUDE.md`, `.envrc.example` (exports `RUSSET_HOME`, `RUSSET_DEVICE_BACKEND`, `CARGO_TARGET_DIR`; the lane-A variant per §0 rule 4), every `crates/*/Cargo.toml` + `README.md` + initial `src/lib.rs` stub with the §5.3 signatures, the seam files (§0.2, including `xtask/src/main.rs` and `xtask/Cargo.toml`), `xtask/src/{layers.rs,paths.rs,no_apple.rs}`, and the first version of every pre-created module file listed below the table |
| W2 | `crates/russet-core/**` except `src/error/codes/{config,secrets,toolchain,sdk,device,app,build,sign,account,provision,debug}.rs`; `crates/russet-ipc/**`; `crates/russetd/src/{lib.rs,ctx.rs,daemon/**,jobs/**,leases/**,subs/**,perms/**,approvals/**,audit/**,web/{mod.rs,server.rs},state/**}`; `crates/russetd/src/handlers/{initialize,daemon,subscription,lease,job,approval,audit}.rs`; `crates/russet-protocol/src/{initialize,daemon,subscription,lease,job,approval,audit,ui,common}.rs`; `crates/russet-cli/src/{main.rs,lib.rs,global.rs,render/**,spawn.rs}`; `crates/russet-cli/src/cmd/{daemon,approvals,audit,completions,bugreport}.rs`; `xtask/src/schema.rs`; `schemas/rpc/**`; `extension/src/generated/**`; `docs/reference/rpc.md` |
| W3 | `crates/russet-{config,secrets,toolchain,doctor}/**`; `crates/russet-core/src/error/codes/{config,secrets,toolchain,sdk}.rs`; `crates/russetd/src/handlers/{config,secrets,toolchain,sdk,doctor}.rs`; `crates/russet-protocol/src/{config,secrets,toolchain,sdk,doctor}.rs`; `crates/russet-cli/src/cmd/{config,secrets,toolchain,sdk,doctor}.rs`; `pins/**`; `schemas/config/**`; `xtask/src/pins.rs` (Wave 1, for `python-shared` and `darwin-tools`; W26 takes it over in Wave 4) [D 87] |
| W4 | `crates/russet-device/**`; `crates/russet-core/src/error/codes/{device,app}.rs`; `crates/russetd/src/handlers/{device,app,process,forward,logs}.rs`; `crates/russet-protocol/src/{device,app,process,forward,logs}.rs`; `crates/russet-cli/src/cmd/{device,app,logs,forward}.rs`; `docs/lab/queue.md` (Wave 1) |
| W5 | `crates/russet-{plist,sdk,build,ipa}/**`; `crates/russet-core/src/error/codes/build.rs`; `crates/russetd/src/handlers/{build,project}.rs`; `crates/russet-protocol/src/{build,project}.rs`; `crates/russet-cli/src/cmd/{build,project,inspect,package}.rs`; `tests/fixtures/apps/RussetProbe/**` |
| W6 | `crates/russet-{macho,sign,asc,account}/**`; `crates/russet-core/src/error/codes/{sign,account,provision}.rs`; `crates/russetd/src/handlers/{account,identity,portal,provision,sign}.rs`; `crates/russet-protocol/src/{account,identity,portal,provision,sign}.rs`; `crates/russet-cli/src/cmd/{account,provision,sign}.rs` |
| W7 | `crates/russet-{gdbremote,debug}/**`; `crates/russet-core/src/error/codes/debug.rs`; `crates/russetd/src/pipeline/**`; `crates/russetd/src/handlers/{run,debug}.rs`; `crates/russet-protocol/src/{run,debug}.rs`; `crates/russet-cli/src/cmd/{run,debug}.rs`; `crates/russet-testkit/src/debug/**`; `docs/design/run-debug-prototypes/` (to delete) |
| W8 | `crates/russet-testkit/**` except `src/debug/`; `xtask/src/{guard.rs,disk_guard.rs,ci.rs,ci_budget.rs,sweep.rs}`; `.github/**` (`apple-release.yml` is created by W23 in Wave 4; W8's `upload-safe` only allowlists its outputs); `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`; `docs/contributing/**` |
| W9 | `demo/m1-russet.sh`; `tests/lab/**` (including DC-28's `tests/lab/dc28-ddi.sh`); `xtask/src/lab.rs` (`cargo xtask lab`, ARCHITECTURE §3.3 item 7); `docs/lab/**` (takes over the queue from W4); appends to `docs/verified-facts.md`; integration fixes only through small PRs reviewed by the owning workstream or the lead |

**Pre-created module files** [D 86]. W1 creates each of these as a compiling stub with its `mod` line in the parent; the named card fills it and owns it from then on: `crates/russet-testkit/src/debug/mod.rs` and the `pub mod debug;` line in testkit's `lib.rs` (W7); `crates/russetd/src/pipeline/mod.rs` (W7); `crates/russetd/src/web/{mirror,mcp}.rs` (W10, W20); `crates/russet-sdk/src/import/mod.rs` (W16b) and `import/windows/mod.rs` (W27b); `crates/russet-build/src/stack/{flutter,xcode,rn,maui}/mod.rs` (W17, W28, W29, W29), `stack/flutter/aot.rs` (W24), `resources/mod.rs` (W21) and `platform/windows/mod.rs` (W27b); `crates/russet-ib/src/{nib,compile}/mod.rs` (W21b); `crates/russet-account/src/free/mod.rs` and `crates/russet-asc/src/ds2.rs` (W22); `crates/russet-anisette/src/windows/mod.rs` (W27c); `crates/russet-device/src/{wifi,a11y}/mod.rs` and `crates/russet-debug/src/wifi.rs` (W30); every per-namespace `handlers/<ns>.rs`, `russet-protocol/src/<ns>.rs`, `russet-cli/src/cmd/<area>.rs` and `russet-core/src/error/codes/<family>.rs` (each with its §13.3 constants); the xtask modules `schema.rs` (W2), `pins.rs` (W3), `guard.rs`, `disk_guard.rs`, `ci.rs`, `ci_budget.rs`, `sweep.rs` (W8), `lab.rs` (W9), `traces.rs`, `contracts.rs` (W15), `golden.rs` (W16) and `release.rs`, `legal.rs`, `docs.rs` (W26).

Ownership for Waves 2–6 is on each card.

---

## 4. Wave 1: foundations and the SwiftUI vertical slice

**Exit gate (checked by W9, §4.10).** Items 1–6 must pass. Item 7 passes, or is recorded as blocked on the owner (Q33); it is never marked passed without DC-28 [D 82].
1. `russet debug` on `tests/fixtures/apps/RussetProbe` stops at the `ContentView.swift` breakpoint (the line after the `RUSSET_PROBE_BREAK` marker), three runs in a row.
2. Every step is Russet code: build (Russet's SwiftPM backend), sign (`russet-sign`), install (`russet-device`), launch (CoreDevice), attach. Attach mode `broker` satisfies this item. If device check D3 fails and only `legacy-async` passes, Wave 1 still exits, with P3 recorded as open and the broker carried into W11.
3. **No xtool or pymobiledevice3 process runs:** they are hidden from `PATH`, and `[fallback] pymobiledevice3 = "off"`.
4. Zero App Store Connect mutations.
5. `russet doctor --json` has no failures.
6. CI is green (the private-phase matrix of QA §4.2).
7. **DDI provenance** [D 62, D 82]. The DDI on the phone was mounted by `russet device prepare` (idevice cryptexd, no pymobiledevice3) from `[device.ddi] dir` = the Apple-origin 27A266a copy of the owner's Mac folder, shown by DC-28's own script (`tests/lab/dc28-ddi.sh`) after one owner-approved reboot (ARCHITECTURE §21 Q33): `russet device ddi status --json` reports `productBuild 27A266a` and `source: dir` (from Russet's mount record). The mirror-sourced 27A5228h DDI mounted during the spike never satisfies this item. If Q33 is unanswered at the gate, this item is recorded as blocked on the owner, never as passed, and DC-28 becomes the first lane-A task of Wave 2. If the phone keeps the old DDI across the reboot and cryptexd refuses even a forced replacement, DC-28 records cryptexd's answer and the lead and the owner decide the item explicitly.

### 4.1 W1: Bootstrap — workspace, seams and contracts

- **Wave:** 1, first, alone. **Lane:** D, slot a. **Size:** M (4–5 days, including the §5.3 stubs, the code constants and the pre-created module files). **Depends on:** none. **Device:** no. **Disk:** ~2 GB.
- **Scope:**
  - CORE-01, QA-01;
  - the skeleton parts of CORE-02 (ErrorCode type and family files) and CORE-03 (`rpc_method!`, registry);
  - every crate in ARCHITECTURE §5.1 as a compiling stub with its README naming the owner, and **every** ARCHITECTURE §5.3 public type, trait and function signature as compiling code with `todo!()` bodies, including the typed `DaemonCtx` fields (`ConfigService`, `AccountService`, …), `Method::permission`, `rpc_method!(…, permission_fn = …)`, the dynamic rows of §5.3 and the `live_connectors_constructed()` debug counter, which gets a real body (freeze F0, §12) [D 66, D 74, D 86];
  - **every** error-code constant of ARCHITECTURE §13.3 in its family file, with the `ALL` arrays and the uniqueness test [D 86];
  - each stub crate's ARCHITECTURE §5.1 key dependencies, with the §16.7 pins and features (at least `apple-codesign` in `russet-sign`, `idevice` and `jktcp` in `russet-device`, `reqwest` in `russet-toolchain` and `russet-asc`, `rmcp` in `russet-mcp`), so the dependency checks below see the real graph [D 86];
  - every pre-created module file of §3, and every `cargo xtask` subcommand of ARCHITECTURE §4 registered in `xtask/src/main.rs` as a stub [D 86];
  - `rust-toolchain.toml` with `channel = "stable"` (minimal profile plus rustfmt and clippy) and workspace `rust-version = "1.99"` [D 70];
  - the module skeletons later workstreams fill (russet-build `stack::{swiftpm,flutter,xcode,rn,maui}`, `resources`, `bundle`, `infoplist`, `metadata`; russet-debug `launch, broker, jit, lldb, dap, symbols, crash, logs, flutter`; russet-device modules per device.md §5.2);
  - all seam files with every namespace pre-registered as a `core.notImplemented` stub;
  - `[workspace.dependencies]` with ARCHITECTURE §16.7 pins (ring-only rustls; idevice `=0.1.68` with the device feature list and `ring`);
  - `.config/workstreams.toml` (paths and layers, binaries included); `AGENTS.md` (the rules of §0, including daemon isolation and the disk and slot rules) plus `CLAUDE.md` pointing to it; `.envrc.example`;
  - the idevice `[patch.crates-io]` of ARCHITECTURE §16.7 once W4's fork commit exists (a lead edit of the root `Cargo.toml`).
- **Must honor:** ARCHITECTURE §4, §5.1–§5.2 (layering), §5.3 seam signatures, §7.3, §13.2 ranges.
- **Tests:**
  - `cargo build --workspace` without `RUSTUP_TOOLCHAIN`, using the installed `stable` (`rustup toolchain list` is unchanged afterwards: no second toolchain);
  - `cargo xtask check-layers` passes, and rejects a deliberately illegal edge in its unit test;
  - `cargo xtask check-no-apple` catches a seeded Mach-O header;
  - `cargo deny check licenses bans sources`;
  - `cargo tree -i aws-lc-sys` is empty, with the key dependencies declared (a stub without them proves nothing);
  - `cargo xtask --help` lists every ARCHITECTURE §4 subcommand.
- **Acceptance:** all of the above, plus `cargo run -p russet-cli -- --help` lists every command group as a stub. If `aws-lc-sys` still appears, record which crate pulls it in the PR and propose the fix; do not switch provider.

### 4.2 W2: Core runtime — IPC, daemon, CLI framework, permissions, approvals

- **Wave:** 1. **Lane:** D, slot a. **Size:** L (7–9 days). **Depends on:** W1. **Device:** no. **Disk:** < 1 GB extra.
- **Scope:**
  - CORE-02 (rest: dirs, fsx, `CommandRunner`/`ProcessRunner` with the single `russet-spawner` thread [D 73], progress and `EventSink`, ids, clock, endpoints);
  - the development-isolation guards of ARCHITECTURE §3.3 item 7 [D 63, D 83]: `RUSSET_HOME`, the `live` refusal without `RUSSET_LANE=A`, the host-wide live-device lock with the debug-only `RUSSET_LIVE_DEVICE_LOCK` override, `DeviceSlot` (the lazy `DeviceManager` start after the backend and lock checks, ARCHITECTURE §5.3), and the `buildId`, executable-identity (`executableId`) and backend comparison (1107, followed by an idle restart);
  - the RPC flag `background` and the shared-lease principals of ARCHITECTURE §6.3 [D 65, D 68];
  - CORE-04, CORE-05 (Linux transport and auth; the Windows pipe code compiles; Windows DACL tests come in W15);
  - CORE-06 (lifecycle, JobManager with `job.events`, leases with grace, SubscriptionHub, InProcess);
  - CORE-07 (multi-call CLI, reporters, global flags, `daemon *`, `approve`/`deny`/`approvals list`, `audit tail`);
  - CORE-13 (logging, redaction, bugreport);
  - QA-05 seams T1–T4, T7 and T8;
  - PermissionEngine and ApprovalService per ARCHITECTURE §7.5 and §11.3;
  - the web endpoint skeleton (§6.6): axum on `127.0.0.1:0`, a route registry, no routes yet.
- **Must honor:** ARCHITECTURE §3.3, §5.3 (core, ipc, russetd), §6, §7, §8.1, §9.
- **Tests:**
  - proptest framing with arbitrary chunking;
  - a Node interop test with vscode-jsonrpc 9.0.3: initialize, `$/progress`, cancel → `-32800`, unknown method → `-32601`, and a `ui.confirm` answered by the client;
  - 8 concurrent CLIs result in exactly 1 daemon;
  - idle exit;
  - a non-background job is cancelled within 1 s of disconnect and its grandchild process is killed within 2 s; a `background` job survives;
  - two `RUSSET_HOME`s give two daemons; a development daemon without `RUSSET_LANE=A` refuses `live` with 3019, and `live_connectors_constructed()` stays 0; a second live daemon gets 3019 with `details.holder`. The lock tests start their daemons with `--allow-live-device`, a temporary `RUSSET_LIVE_DEVICE_LOCK` and `USBMUXD_SOCKET_ADDRESS` pointed at a test-local fake usbmuxd (it accepts `Listen`, answers `Result 0` and reports no device), never with `RUSSET_LANE=A` and never on the real lock. They need no W4 code: the lock is taken at daemon start and `DeviceManager` starts only on the first device call that passes the lock check (ARCHITECTURE §3.3 item 7), so the holder is never asked and the second daemon answers 3019 without constructing a connector (nextest runs each test in its own process, so the counter is per test);
  - rebuilding the binary without a commit (same `buildId`, new executable identity) makes the next CLI get 1107 and restart the idle daemon;
  - a child spawned from a `spawn_blocking` task survives that pool thread's exit;
  - a `mcp`-kind connection calling a tier-3 method gets `perm.approvalRequired` with an `approvalId`, cannot call `approval.grant`, and the approval's binding rules hold (expired, reused and mismatched cases);
  - the redaction canary never appears in logs;
  - the schema snapshot.
- **Acceptance:**
  - `russet daemon status --json` validates against `schemas/rpc/v1`;
  - `russet --no-daemon daemon status` works in-process;
  - the exit-code table is covered by tests;
  - the contract PR (core bodies behind the F0 signatures, DTOs, Router signature) merges by **day 2**, so other workstreams can rebase onto it (freeze F1, §12);
  - W2 itself merges first among W2–W7, because their CLI-level acceptance runs through its daemon and CLI (§2).

### 4.3 W3: Config, secrets, toolchain resolver, doctor

- **Wave:** 1. **Lane:** D, slot a. **Size:** L (5–7 days). **Depends on:** W1 (F0 stubs; W2's F1 contract PR for bodies). **Device:** no. **Disk:** ~0.2 GB (one 34 MB download).
- **Scope:**
  - CORE-08 (all config files of ARCHITECTURE §14, kebab-case, schemas, the narrowing rule);
  - CORE-09 = SA-01 (sealed store over keyring-core with `SecretKind`, passphrase store, env store, memory store), with the keyring entry `master-key-<sha256(RUSSET_HOME)[..8]>` whenever `RUSSET_HOME` is set [D 83];
  - the non-TTY rule for local-file imports: `secrets.set {fromFile}` from a non-interactive CLI with `--yes` (ARCHITECTURE §7.5) [D 85];
  - CORE-10 (probes, pins schema, resolver, env contract, SDK registry probe);
  - a CORE-11 subset: verified download, install and selection of `python-shared` (PBS 3.12.15) and `darwin-tools` 1.1.0, with the free-space guard, and no Swift or LLVM downloads;
  - `xtask/src/pins.rs` for those two components, per ARCHITECTURE §14.7 [D 87]: URLs and sizes from the GitHub release metadata (PBS release `20261009`, darwin-tools `v1.1.0`), `sha256` from the downloaded file, never a hand-typed value; `pins/stable.toml` signed with a development minisign key whose secret half stays in `~/.config/russet-dev/` (never committed), and its public key in `pins/`;
  - CORE-12 (doctor framework plus the host, dirs, daemon, config, secrets, usbmux and toolchain checks). `usbmux.available` connects to the mux only when the daemon's device backend is `live`; otherwise it only stats the socket and reports `skip` with the backend as the reason, so lane D never connects to `/run/usbmuxd` [D 83];
  - RD-05 (LLDB flavor rules and the PBS provisioning path).
- **Must honor:** ARCHITECTURE §14, §15, §16.1–§16.2, §8.1 config/secrets/toolchain/sdk/doctor rows.
- **Tests:**
  - TOML round trips keep comments;
  - an invalid key reports file:line:col;
  - a project file cannot widen `[agents]`;
  - a 4 KiB secret round-trips with ksecretd on this box (env-gated; the test creates its own uniquely named keyring entry and deletes only that one); every other test uses the `memory` or `passphrase` backend;
  - two `RUSSET_HOME`s get two different keyring entries;
  - passphrase mode works with `DBUS_SESSION_BUS_ADDRESS` unset;
  - the redaction canary;
  - toolchain fixtures produce `sdk.incompatible` and `toolchain.versionMismatch`;
  - PBS installs into a temporary `RUSSET_HOME` and resolves;
  - a manifest whose signature or content was altered is rejected (CORE-11; the test signs with a key generated at test time).
- **Acceptance:**
  - `russet toolchain list --json` on the dev box reports:
    - Swift 6.4 (system);
    - SDK `xcode-27.0-27A266a` with darwinTools 1.1.0;
    - linker = darwin-tools `ld64.lld` (LLD 21.1.6), with the system LLD 23.1.1 flagged too old;
    - `lldb` and `lldb-dap` 21.0.0 `ok: true`, with `env` pointing at the existing uv-managed PBS 3.12.15 (provenance `uv`) or **Russet's** `data/toolchains/python-shared/3.12.15`, never at `~/.local/opt/python-3.12`.
  - `russet doctor --json` passes `toolchain.*` and `secrets.backend`, and reports `usbmux.available` as `pass` or `skip` (lane D's scripted backend); W9 confirms `pass` on lane A's `live` daemon;
  - the CLI-level items are checked once W2 has merged (§2).

### 4.4 W4: Device plane I (lane A)

- **Wave:** 1. **Lane:** **A**, slot b (slot a at tier AMBER). **Size:** L (8–10 days). **Depends on:** W1 (F0 stubs; W2's F1 contract for handlers). **Device:** **yes**. **Disk:** ~1.5 GB (idevice in the target dir).
- **Scope:**
  - DEV-01: crates.io `idevice =0.1.68` with `ring`, patched per ARCHITECTURE §16.7 (upstream's usbmuxd short-packet check and `Send` listen stream, plus one Russet commit with the cryptexd persistence fix and a 16 MiB cap on declared usbmuxd packet sizes, larger → `BadCommand` before any allocation): W4 creates the fork commit and drafts an upstream PR for each fix, the lead edits the root `[patch.crates-io]` [D 22]; the listen stream on a dedicated current-thread runtime until the patch lands;
  - DEV-02, DEV-03 (codes per ARCHITECTURE §13.4), DEV-04 (USB tunnel with the MSS clamp at 8132; per-generation RSD map);
  - DEV-05 (pairing and readiness) plus a minimal DEV-13 slice [D 62, D 82, D 89]: `device.ddi.status`, `device.ddi.mount {kind: dir}` and `device.prepare` through idevice `cryptexd::install_ddi`, from a `dir` that is the DDI root (Russet loads `<dir>/Restore`; a directory that directly holds `BuildManifest.plist` also works), with payload paths taken from the BuildManifest identity (Xcode-native names); every mount Russet performs writes the `MountRecord` (`state/devices/<udid>/ddi.json`), and status reports the recorded source;
  - **the Wave-1 DDI source** [D 82]: a lead pre-task, in an owner-approved lane-B window (Q33), copies the owner's Mac `/Library/Developer/DeveloperDiskImages/iOS_DDI` (27A266a, 33.6 MB; read-only on the Mac; the root holds `Restore/` and `version.plist`) to `~/russet-lab/ddi/27A266a/iOS`, checks the manifest's SHA-384 digests, and sets `[device.ddi] dir` in lane A's persistent home (`~/russet-lab/home-A`). The copy gates DC-28, not W4's start. The pymobiledevice3 cache (the mirror's 27A5228h) is never the gate's source;
  - device check DC-28 (DDI provenance and reboot, §11), defined here and run in W9;
  - DEV-06 (install, uninstall, list; uses `russet-ipa` once W5 merges it, the `.app` directory upload until then);
  - DEV-07 (CoreDevice launch with startStopped and openstdiosocket, DVT fallback, signals, process list, exit detection);
  - DEV-08 (forwards; debug launch helpers as the internal API `russet-debug` needs: `connect_rsd`, the launch channel kept open);
  - DEV-09 (logs: os_trace by pid, stdout merge, `logs.tail` ring); the `russet logs` command, including `--session` (calls W7's `run.output.query`, then follows) and `--duration`. With `--json` it prints NDJSON, one `LogEntry` (ARCHITECTURE §9.5) per line, which the §4.10 script counts with `grep -c`;
  - the CLI forms of ARCHITECTURE §8.7: `russet app launch <bundle> [--suspended] …` and `russet forward --service NAME | --tunnel-port N | --usbmux-port N`;
  - the DEV-19 subset for every Wave-1 row of ARCHITECTURE §8.2;
  - DEV-25 subset doctor checks (mux, pairing, Developer Mode, DDI status, lock hint);
  - the `io::ServiceConnector` seam with `LiveConnector` (its constructor increments the `live_connectors_constructed()` debug counter, ARCHITECTURE §5.3);
  - device checks D1/D2 for W7 (§11 DC-04) using its own code.
- **Must honor:** ARCHITECTURE §5.3 (russet-device), §6.3, §8.2, §10.4, §13.3–§13.4.
- **Tests:**
  - unit tests: NSError decoder fixtures (Locked, ActionError 2), MSS selection, log batching, LogEntry mapping, DDI status parsing from recorded CopyDevices and cryptexd replies, mount-record attribution, `Cryptex1Assets::load` on a synthetic DDI root (`Restore/` with Xcode-native `022-…-411.dmg` names, generated at test time in a temporary directory, never committed, ARCHITECTURE §19 [D 88]) and on a bare Restore directory, a recorded cryptexd install request carrying `persistence: 2` and `nonce-persistence: 1`, and a declared usbmuxd size above 16 MiB rejected without allocation;
  - HIL with `RUSSET_HIL_UDID` set: 100 session opens with 0 failures; a 32 MiB AFC transfer through the tunnel at ≥ 20 MiB/s both ways; installs of `.app` and `.ipa`; a suspended launch returns a pid and SIGCONT resumes it; a console line arrives; `app.exited` ≤ 2 s after a kill; logs from the first instruction (launch suspended → subscribe → SIGCONT).
- **Acceptance** (CLI-level items checked once W2 has merged, §2):
  - `russet device list --json` answers in < 300 ms with `iPhone13,2` / `27.0`;
  - `russet device ddi status --json` reports the mounted DDI (`source: unknown` for the spike's pre-existing mount, which has no mount record); the mount itself runs in DC-28 (W9), with the owner's approval;
  - `russet app launch <bundle> --suspended` returns a pid;
  - `russet forward --service com.apple.internal.dt.remote.debugproxy` plus the M1 LLDB sequence (`demo/m1-baseline.sh` with the forward swapped in) stops at the breakpoint with **no pymobiledevice3 on PATH**;
  - the device is left with only the existing test app installed.

### 4.5 W5: Build plane I — SDK facts, SwiftPM builder, bundle, IPA

- **Wave:** 1. **Lane:** D, slot b. **Size:** L (7–9 days). **Depends on:** W1 (F0 stubs; W2 F1, W3 resolver API for linker and Swift). **Device:** no (its device check runs in W9). **Disk:** ~0.5 GB scratch.
- **Scope:**
  - B01 (XML/binary plist and macro expansion; OpenStep can wait for W16);
  - B02 (request, events as `JobEvent`, the errors of ARCHITECTURE §13.3 `build`, the strict `russet.toml` sections it owns, `build-report.json` schema 1);
  - B03 (generated builder package; `swift build --build-system swiftbuild` with `env -u SDKROOT`, `XCODE_EXTRA_PLATFORM_FOLDERS` = the Russet platform view, and the generated `toolset-swb.json` carrying `-Xclang-linker -isysroot -Xclang-linker <SDK>`);
  - `russet-sdk` (`XtoolBundleProvider` reading the existing bundle in place; platform view; toolsets);
  - B06 subset (Info.plist synthesis with the scene manifest, metadata policy `sdk` exactly as ARCHITECTURE §16.4, bundle assembly, rpath hygiene through `russet-macho`, coded against the F0 stubs and W6's day-4 contract PR (F2), BundleIdPolicy with all four variants, SigningPlan);
  - B07 (`russet-ipa` writer and reader);
  - the fixture `tests/fixtures/apps/RussetProbe`: SwiftUI, 0BSD, bundle id `dev.russet.test.probe`, a `PROBE seq=<n> t=<ms>` stdout heartbeat every 100 ms, a `-RussetProbeCrash 1` launch argument, and a known breakpoint inside `ContentView.body` [D 84]:

    ```swift
    @State var counter = 42
    var body: some View {
        let probeValue = counter  // RUSSET_PROBE_BREAK
        Text("counter \(probeValue)")  // the breakpoint line: the line after the marker
    }
    ```

    Scripts find the marker with `grep`, break on the next line (inside `ContentView.body.getter`, as QA E2E-04 expects) and read `frame variable probeValue`, a plain local. `frame variable counter` cannot read a SwiftUI `@State` property (only `self._counter`), and expression evaluation off-Mac is unproven until D7.
- **Must honor:** ARCHITECTURE §5.3 (sdk, build, ipa), §8.3, §9.2, §14.2, §16.3–§16.5.
- **Tests:**
  - plist round trips (SDKSettings, platform Info.plist) and macro unit tests;
  - building RussetProbe on this box;
  - `llvm-otool -l` shows `sdk 27.0` and `minos 17.0` on the executable and every package dylib;
  - no absolute host `LC_RPATH`;
  - the scene manifest is present;
  - two clean builds are byte-identical before signing;
  - IPA listing, modes 755/644, two runs with equal SHA-256;
  - `russet build --json` in a non-project directory → `config.projectNotFound`.
- **Acceptance:** `russet build --json` in RussetProbe returns `BuildOutput` with a SigningPlan and a `build-report.json`, without the `xtool` binary on PATH, in ≤ 60 s warm on this box (checked once W2 has merged, §2; build.md §9.4–§9.7 payloads are superseded by ARCHITECTURE §8.3, §13.3 and §20.1). The device check DC-02 (the UIScene app with sdk 27.0 launches) is queued for W9.

### 4.6 W6: Signing I — Mach-O, signer, ASC read client, xtool import, read-only provisioning

- **Wave:** 1. **Lane:** D, slot b. **Size:** L (7–9 days). **Depends on:** W1 (W2 F1, W3 secrets API). **Device:** no (DC-03 runs in W9). **Disk:** ~1 GB.
- **Scope:**
  - SA-03 plus the build lint parse API → `russet-macho` (MIT, the union API of ARCHITECTURE §5.3);
  - SA-04 (profiles, entitlement merge, bundle walker);
  - SA-05: `apple-codesign` 0.29.0 with `default-features = false`; pre-sign fixes (chmod 0755, string-pool alignment); post-sign verify; L-SIG/L-MACHO/L-BND lints; the `CodeSigningIdentity` trait;
  - SA-02 GET-only (ES256 JWT, scoped tokens, rate limiter, open enums, wiremock fakes);
  - SA-06 subset (`accounts.toml`, `account.addAscKey`, `account.importXtool`, IdentityStore import, `AccountService` (ARCHITECTURE §5.3), `Provisioner::plan` GET-only with profile matching, `apply` for zero-mutation plans; mutations refused without an approval);
  - the non-TTY rule of ARCHITECTURE §7.5 for `account.importXtool` and `account.addAscKey`: a non-interactive CLI may run them with `--yes` [D 85];
  - SA-07 subset (`russet account list|add-key|import --from-xtool|verify`, `russet provision --dry-run`, `russet sign|sign verify|lint|inspect`).
- **Must honor:** ARCHITECTURE §5.3 (macho, sign, account, asc), §8.4 Wave-1 rows, §12.2, §14.4, §16.5.
- **Tests:**
  - ≥ 40 table-driven entitlement merge cases;
  - profile parsing on a synthetic CMS profile from the test PKI;
  - Mach-O fixtures built by us;
  - wiremock ASC: a scope mismatch gives `account.ascScope`, a 60-minute token gives `account.ascUnauthorized`;
  - an env-gated local-only test checks the real M1 `Hello.app` signature structure: CD v0x20400, SHA-1 + SHA-256, `execSegFlags 0x11`.
- **Acceptance** (CLI-level items checked once W2 has merged, §2):
  - `russet account import --from-xtool --yes`, run from an agent shell without a TTY, seals the ASC key and the development identity into the worktree's store, and prints the `chmod 600` advice; without `--yes` and without a TTY it exits 11 (`perm.confirmationUnavailable`);
  - for RussetProbe with the owner's `russet.local.toml` (§4.10), `russet provision --dry-run --json` finds the existing `XTL-…` profile and lists **0 mutations**;
  - `russet sign <app>` followed by `russet sign verify` passes.

### 4.7 W7: Run and debug I — gdb-remote, broker, launch, attach plan, run pipeline

- **Wave:** 1. **Lane:** D, slot b. **Size:** L (8–10 days). **Depends on:** W1 (F0 stubs). It starts on day 0 with RD-01 to RD-03 (gdb-remote, the testkit port, the broker), which need no F2; the pipeline (`pipeline/run.rs`, RD-04, RD-06) follows the F2 contract PRs of W4–W6 (day 4) [D 74]. With only four lane-D agents, W7 is the card that starts late, never W6 (§2). **Device:** no (D1–D3 run on lane A). **Disk:** ~1 GB.
- **Scope:**
  - RD-01 (`russet-gdbremote`);
  - RD-02 (port `docs/design/run-debug-prototypes` into `russet-testkit/src/debug` and tests, then delete the folder);
  - RD-03 (broker v1 relay; `QSetDetachOnError`; one client);
  - RD-04 (launch strategies over the russet-device API; launch channel kept open until `vAttach`; default env `NSUnbufferedIO=YES`, `OS_ACTIVITY_DT_MODE=YES`);
  - RD-06 (Python-free `attach.lldb`; `russet debug` spawning `lldb -s` with the resolved env; `--batch --command …` with the `--json` output shape of ARCHITECTURE §8.7; `--print-config`; `debug.prepare`);
  - `russet debug --detach` and the batch semantics of ARCHITECTURE §7.6 and §8.7 [D 84]: `debug.prepare {sessionLease: detached}`; the batch script continues to the first stop, runs the `--command`s there, then ends with `process detach` (`--detach`) or `process kill`; LLDB's `D` goes through the broker's detach path (RD §4.6 "IDE Detach"), not `onDebuggerLost`; the JSON result carries `detached`;
  - the CLI verbs `russet run`, `sessions` and `stop` in `cmd/run.rs` (ARCHITECTURE §8.7); `run.list` and `russet sessions --json` return the `SessionSummary` of ARCHITECTURE §9.5 with its `SessionState` wire values. `russet logs --session` belongs to W4's `cmd/logs.rs` and calls `run.output.query`;
  - RD-09 minimal (the `run.output` merge of stdout and os_trace);
  - `russetd/src/pipeline/run.rs` (build → provision plan/apply → sign to `.russet/build/<profile>-iphoneos/signed/` → install-if-changed ledger → launch/debug);
  - the `run.*` Wave-1 rows;
  - `attach-mode = legacy-async` (the M1 sequence) as the fallback.
- **Must honor:** ARCHITECTURE §5.3 (debug, gdbremote), §6.3, §8.5, §10.3, §17.1.
- **Tests:**
  - codec, stop parser and touch-plan units;
  - H1 (vRun/vAttach against lldb-server from the Swift toolchain);
  - H2 (pre-attach relay with `lldb` and `lldb-dap` 21);
  - H3 (JIT demo; servicing comes later);
  - a golden test that generated scripts contain no `script`, `command script` or Python breakpoint commands;
  - pipeline tests with fake builder, signer and device (in `crates/russetd/tests/pipeline_*.rs`, §0 rule 2);
  - a batch run against H2's relay with `--detach` leaves the host test process running and its session `running` with `debugger: detached`; without `--detach` the process is killed and the session ends `exited`;
  - CI runs H1–H3 in < 3 minutes.
- **Acceptance:** host CI green. DC-04 (D1 CoreDevice `startStopped` + stdio shape; D2 launch-channel behaviour; D3 broker sync connect stops at the breakpoint) is queued for lane A; D1/D2 can run in W4's window, D3 runs in W9.

### 4.8 W8: Testkit, CI and repository gates

- **Wave:** 1. **Lane:** D, slot a. **Size:** M (4–5 days). **Depends on:** W1. **Device:** no. **Disk:** < 0.5 GB.
- **Scope:**
  - CORE-15: `ScriptedConnector` (= core's FakeDeviceBackend: scripted attach/detach, logs, failures), a temp-home daemon fixture, transcript replay, the Node interop harness shared with W2;
  - QA-02: `xtask guard` with apple-files, secrets (gitleaks if installed, otherwise a built-in pattern scan), reuse, typos, `--artifacts DIR`;
  - the provenance allowlist of ARCHITECTURE §19 [D 88]: provenance files are read only at the roots compiled into `guard.rs` (`tests/fixtures/xip/`, `fuzz/russet-xar/corpus/`); a file with Apple-format magic or an Apple file extension passes only if its `PROVENANCE.toml` lists it with a matching SHA-256 and a committed generator script;
  - `cargo xtask guard public-terms` (ARCHITECTURE §19) [D 91]: SHA-256 digests of the private track's identifying terms (lowercased tokens and token pairs; the lead supplies the digests from the private repository's list, and the plaintext never enters this repository), with an allowlist that holds only the two files Q34 covers (`docs/decisions.md`, `docs/research/macos-on-x86-feasibility-2026-10.md`) until Q34 is answered; it runs in `guard all`;
  - QA-03: `ci.yml` per the private-phase matrix of QA §4.2 [D 77]: fmt, clippy, nextest, deny, guard, check-layers, check-paths and schema drift on `ubuntu-24.04` on `main` pushes (and PRs labelled `ci`); a `windows-2025` build + L1 job weekly and on dispatch, with the pinned 1.99.0 toolchain cached; no `merge_group`; actions pinned by SHA; zizmor clean; the `upload-safe` composite action, whose allowlist names `apple-release.yml`'s own RussetProbe outputs by job and path;
  - the lane-D isolation test [D 63, D 83]: a daemon started with `RUSSET_DEVICE_BACKEND=scripted:…` and a tripwire `USBMUXD_SOCKET_ADDRESS` socket never connects to it. It exercises W4's `DeviceManager` and cannot see a direct open of `/run/usbmuxd`, so it lands `#[ignore]` with that reason and W9 enables it after W4 merges; W2's `live_connectors_constructed() == 0` assertion covers Wave 1 until then;
  - `cargo xtask` wrappers that run disk-guard before cargo, with `CARGO_INCREMENTAL=0` [D 81]; `cargo xtask ci-budget`, which reads the month's Actions usage through `gh api` and warns at 80 % of the included minutes;
  - QA-04: CONTRIBUTING (DCO, Conventional Commits, clean-room, no Apple files), SECURITY, CODE_OF_CONDUCT, CODEOWNERS, templates;
  - QA-24: `xtask disk-guard` tiers and `xtask sweep`.
- **Must honor:** ARCHITECTURE §12.2 (supply chain), §19; QA §4.
- **Tests:** seeded bad fixtures are all caught by the guard in < 30 s (including an unlisted file with xar magic under a provenance root, a listed file whose hash changed, and a canary term outside the allowlist, with the test supplying its own digest list); disk-guard refuses below a fake tier.
- **Acceptance:**
  - CI is green on GitHub for the workspace as merged so far, and the month's planned use (QA §4.8) stays under 1,800 of the 2,000 included minutes;
  - local `cargo xtask ci --profile pr` works with missing optional tools (prints which are missing). The owner installs the full QA tool set with one `pacman` command (QA Appendix A, about 200 MiB).

### 4.9 W9: Vertical-slice integration and lab smoke (lane A)

- **Wave:** 1, last. **Lane:** **A**, slot b. **Size:** M (3–5 days). **Depends on:** W2–W8 merged. **Device:** **yes**. **Disk:** < 1 GB.
- **Scope:**
  - runs in lane A's persistent home (`RUSSET_HOME=$HOME/russet-lab/home-A`, `RUSSET_LANE=A`; §0 rule 4), which already holds W4's `[device.ddi] dir` once the lead's DDI copy exists [D 82];
  - `demo/m1-russet.sh` (§4.10) and DC-28's own script `tests/lab/dc28-ddi.sh` (the provenance assertion lives there, not in the exit script);
  - `xtask/src/lab.rs` (`cargo xtask lab`, honouring the live-device lock's holder, ARCHITECTURE §3.3 item 7);
  - the `tests/lab` crate with the `smoke` suite: QA E2E-01, 02, 03, 04, 06 and 14 adapted to the Wave-1 bundle-ID mapping;
  - enabling W8's tripwire isolation test (it was `#[ignore]` until W4 merged), and confirming `usbmux.available` reports `pass` on lane A's `live` daemon;
  - device checks DC-02, DC-03, DC-04/D3 and DC-28 (with the owner's approval, ARCHITECTURE §21 Q33);
  - fixes found in integration, landed through small PRs to the owning workstreams' paths (each reviewed by its owner or the lead);
  - appending the results to `docs/verified-facts.md` (redacted).
- **Acceptance:** the §4.10 script passes 3 consecutive times; the lab smoke suite passes; the tripwire test passes un-ignored; DC-28 passes, or is recorded as blocked on the owner (gate item 7); the results are recorded.

### 4.10 Wave 1 exit script (what W9 automates)

The owner-local, gitignored `tests/fixtures/apps/RussetProbe/russet.local.toml` reuses the existing xtool profile with **zero** account changes:

```toml
[app]
bundle-id = "com.evilcomputer12.orchard.hello"     # the bundle id the existing XTL-… profile covers
[signing]
bundle-id-policy = "xtool-prefix"                  # → XTL-<prefix>.com.evilcomputer12.orchard.hello
```

```sh
# demo/m1-russet.sh (sketch). Lane A only: RUSSET_HOME=$HOME/russet-lab/home-A, RUSSET_LANE=A (§0 rule 4).
# Gate items 1–6. Item 7 (DDI provenance) is DC-28's own script, tests/lab/dc28-ddi.sh, below [D 82].
set -euo pipefail
export PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -vx "$HOME/.local/bin" | paste -sd:)"   # hides xtool, pymobiledevice3, go-ios
cd tests/fixtures/apps/RussetProbe
PROBE_LINE="$(grep -rn --include=ContentView.swift 'RUSSET_PROBE_BREAK' . | head -1 | cut -d: -f2)"
[ -n "$PROBE_LINE" ]; PROBE_LINE=$((PROBE_LINE + 1))                # break on the statement after the marker [D 84]
russet config set --scope global fallback.pymobiledevice3 off
DDI_DIR="$HOME/russet-lab/ddi/27A266a/iOS"                           # the lead's Apple-origin copy (W4), once it exists
if [ -d "$DDI_DIR/Restore" ]; then russet config set --scope global device.ddi.dir "$DDI_DIR"; fi
russet device prepare --json | jq -e '.ddi.mounted'                 # mounts from [device.ddi] when missing; a mounted DDI is a no-op
russet doctor --json | jq -e '.summary.fail == 0'
russet account list --json | jq -e '.accounts | length > 0' >/dev/null \
  || russet account import --from-xtool --yes                        # no TTY here: --yes is enough for local-file imports [D 85]
russet build --json | jq -e '.app.path'
llvm-otool -l .russet/build/debug-iphoneos/RussetProbe.app/RussetProbe | grep -A4 LC_BUILD_VERSION | grep -q 'sdk 27.0'
russet run --json --detach > run.json   # build (no-op) → sign → install-if-changed → launch; prints {sessionId, pid, bundleId}
russet logs --session "$(jq -r .sessionId run.json)" --duration 15s --json > logs.ndjson   # NDJSON: one LogEntry per line
[ "$(grep -c 'PROBE seq=' logs.ndjson)" -ge 100 ]
russet stop --all
russet debug --batch --detach --break ContentView.swift:"$PROBE_LINE" --command 'frame variable probeValue' --json > dbg.json
jq -e --argjson l "$PROBE_LINE" \
  '.stop.reason == "breakpoint" and .stop.frame.line == $l and (.commands[0].output | test("= 42")) and .detached' dbg.json
sleep 5; russet sessions --json | jq -e '[.sessions[] | select(.state == "running")] | length == 1'   # the detached debug session
russet stop --all
russet audit tail --kinds account-mutation --json | jq -e '.entries | length == 0'
```

```sh
# tests/lab/dc28-ddi.sh (sketch; W9; run from the repository root). Gate item 7. Runs once the lead's DDI copy exists
# and the owner approved the reboot (Q33).
set -euo pipefail
export PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -vx "$HOME/.local/bin" | paste -sd:)"
DDI_DIR="$HOME/russet-lab/ddi/27A266a/iOS"
russet config set --scope global device.ddi.dir "$DDI_DIR"
russet device ddi status --json | tee dc28-before.json              # after the reboot: did the mirror's 27A5228h survive? (recorded)
russet device prepare --json > /dev/null                            # mounts from [device.ddi] dir when nothing is mounted
if ! russet device ddi status --json | jq -e '.productBuild == "27A266a"' > /dev/null; then
  [ "${RUSSET_DC28_FORCE:-}" = owner-approved ] || { echo "DC-28: another DDI is still mounted; --force needs the owner's OK" >&2; exit 3; }
  russet device ddi mount --dir "$DDI_DIR" --force --json | tee dc28-force.json   # cryptexd's answer is recorded either way
fi
russet device ddi status --json | jq -e '.productBuild == "27A266a" and .source == "dir"'   # source from Russet's mount record [D 89]
demo/m1-russet.sh                                                   # launch and debug RussetProbe on that DDI
```

Pass criteria:
- the breakpoint is hit within 60 s of the command start, on the line after the `RUSSET_PROBE_BREAK` marker, inside `ContentView.body.getter`;
- `probeValue == 42`;
- the app is still alive 5 s after the batch detached: exactly one `running` session, with `debugger: detached`;
- ≥ 100 consecutive heartbeat lines arrive;
- re-running skips the install (ledger) and still passes;
- the only app installed besides the owner's own apps is the `XTL-…` test app (it replaces the M1 Hello install; agreed as the designated test slot);
- `--detach` (`russet run` and `russet debug`) follows ARCHITECTURE §7.6: the CLI waits for the launch and leaves a detached session; it never sets the RPC flag `background` [D 65, D 84];
- `tests/lab/dc28-ddi.sh` exits 0 (gate item 7 passed) or 3 (recorded as blocked on the owner); it is never skipped silently.

---

## 5. Wave 2: M1 complete, agent-ready (gate G1 candidate)

**Exit gate:**
1. QA E2E-01 to 06, 12, 13 and 14 pass 3 times in the lab (E2E-12 uses the one bundle ID that decisions.md allows).
2. F5 in VS Code stops at the breakpoint (Tier-0 extension).
3. Claude Code drives the phone through MCP.
4. Crash reports are symbolicated.
5. The mirror plays from the CLI.
6. L0–L6 are green.
7. Legal L1–L12 pass.
8. The owner decides on going public (Q6); if yes, the checklist of §5.9 passes first, including Q34.
9. W16b imports the DDI from the Mac's `XcodeSystemResources.pkg` copy (and from the owner's `.xip` if Q27 is answered) with the nine SHA-256 values of §5.8, and DC-29 mounts it with idevice alone and debugs RussetProbe, with `source: imported` in Russet's mount record. Mounting the import needs a Q33-approved reboot or an owner-approved `russet device ddi mount --imported --force`, because `device.prepare` is a no-op while the Wave-1 copy (the same bytes) is mounted. Without that approval this item is met by "import verified, mount not re-exercised", recorded as such [D 89]. DC-28 has passed by now, if it was still open after Wave 1.
10. The Apple-hardware smoke (W16) ran once, owner-dispatched, within the included minutes.

### 5.1 W10: Device plane II — screen, mirror, HID, crashes, perf, files, DDI (lane A)

- **Lane:** **A**, slot b. **Size:** L (8–10). **Depends on:** Wave 1; W16b's DDI part (day 8 of Wave 2) for the `imported` default and DC-29. **Device:** **yes**. **Disk:** ~1 GB.
- **Owns:**
  - `crates/russet-device/**`, `crates/russetd/src/web/mirror.rs`;
  - `crates/russet-core/src/error/codes/{device,app,screen,perf}.rs`;
  - handlers, protocol modules and CLI modules `{device,app,process,forward,screen,input,files,perf,location}`;
  - `tests/lab/**`, `docs/lab/**`;
  - `tests/fixtures/apps/RussetProbe/**` (Wave 2), for RussetProbe's clock mode.
- **Scope:**
  - RussetProbe's clock mode: a launch argument that renders a frame counter and a QR code of the device clock, so mirror latency (DC-06, later W19's DC-17) is measured from decoded frames, corrected by the device-time offset, instead of being self-reported;
  - DEV-07b, DEV-10;
  - DEV-11: the display session, RTP depacketizer, the `russet.mirror.v1` route per ARCHITECTURE §10.1, and `russet mirror --stdout|--record`;
  - DEV-12;
  - DEV-13 (the rest) [D 62]: `imported` as the default source (newest `ddiBuild` first, falling back on `ddiPersonalizationFailed`/`ddiIncompatible`); `device.ddi.list`; the personalized path for iOS 17–26.x (PersonalizedDMG and LoadableTrustCache resolved through the identity, with the cryptex fallback from 26.4 for boards missing from the PDI list); `pmd3-cache` and `pmd3-auto-mount` only with the `allow-mirror` consent (3022); error mapping and 3020 hints; the optional IM4M ticket cache (`ticket-cache`);
  - DEV-14;
  - DEV-15: pull and `.ips` parse, with automatic capture after abnormal exits handed to W11's crash store;
  - DEV-16;
  - DEV-18: pymobiledevice3 runner and `screen.elements`;
  - DEV-23 upstream PRs (MSS clamp, FIN as EOF, tap timing, symlink upload, and the cryptexd persistence fix drafted in W4);
  - DEV-24 (HIL suite and 2-hour soak), DEV-25 (rest);
  - SPK-02 (AVC-only offer, rotation and touch coordinates);
  - QA-13 growth and QA-17 budgets;
  - recording RDT traces with W15's recorder (DC-13);
  - running device checks for W11–W16.
- **Acceptance:**
  - a test WebSocket client gets a keyframe within 1 s, and the median is ≥ 25 fps during a scripted swipe loop;
  - two viewers share one display session, and input while mirroring does not interrupt the stream;
  - a tap launches the app icon 20 out of 20 times;
  - a screenshot takes < 400 ms;
  - `-RussetProbeCrash 1` gives `app.exited` and a pulled report within 10 s;
  - perf delivers 2 samples/s with named fields;
  - `device.prepare` on a mounted phone is a no-op in < 1 s;
  - DC-29 [D 89]: after W16b's import, and after a Q33-approved reboot or an owner-approved `russet device ddi mount --imported --force`, the imported 27A266a DDI is mounted with idevice alone (no pymobiledevice3 on PATH), `russet device ddi status --json` reports `source: imported` from the mount record, and RussetProbe launches and debugs. Without the approval, DC-29 records "import verified, mount not re-exercised" (gate item 9 accepts it);
  - the device §15.4 HIL budgets pass; the soak shows no leaks;
  - traces pass the trace lint.

### 5.2 W11: Run and debug II — DAP hub, crashes, symbols, Quick Relaunch, `po`

- **Lane:** D, slot b. **Size:** L (8–10). **Depends on:** Wave 1. **Device:** via DC-07, DC-09, DC-10. **Disk:** ~2 GB. A symbol fetch needs owner Q13 and tier GREEN.
- **Owns:**
  - `crates/russet-{debug,gdbremote}/**`, `crates/russetd/src/pipeline/**`;
  - handlers and protocol `{run,debug,crash}`; CLI `{run,debug,crash,symbols}`;
  - error code files `{debug,crash}`.
- **Scope:**
  - RD-07 (DAP hub v1, ARCHITECTURE §10.2), RD-09 (full);
  - RD-12 (status, plan, consent-gated fetch, quota, tier-0 index; includes DEV-17/17b, the native `remoteFetchSymbols` client over `connect_rsd`);
  - RD-13 (crash store, symbolication with `symbolic`, `crash.*`);
  - RD-14 (agent clients and the `debug.control/inspect/evaluate/breakpoint.*` RPCs);
  - RD-16 (overlay and the D7 experiment);
  - Swift Quick Relaunch (`run.relaunch`, `run.hotReload`), `debug.attach`;
  - RD-19 (attach time and relaunch timings), RD-21 (upstream issues as found).
- **Acceptance:**
  - DC-07: a scripted DAP client gets a `stopped` event at the breakpoint, then variables, step and continue (E2E-05). While an IDE-role client is attached, an agent-role client sets a breakpoint through `ide.breakpoints.add`, and both see the stop;
  - Quick Relaunch ≤ 8 s is recorded;
  - DC-10: a forced crash is symbolicated to file:line for app frames within 30 s;
  - DC-09: the `po` status for iOS 27 is documented (M3 criterion);
  - a symbol fetch is refused without consent or space.

### 5.3 W12: Signing II — provisioning apply, approvals, archive

- **Lane:** D, slot b. **Size:** L (6–8). **Depends on:** Wave 1. **Device:** via DC-08. **Disk:** ~1 GB.
- **Owns:**
  - `crates/russet-{macho,sign,asc,account}/**`, `crates/russet-ship/src/{lib.rs,archive.rs,ipa.rs,dsym.rs}`;
  - handlers and protocol `{account,identity,portal,provision,sign,asc}`, plus the `ship.archive` and `ship.ipa` rows of `handlers/ship.rs`;
  - CLI `{account,provision,sign,ship}`;
  - error code files `{sign,account,provision,ship}`.
- **Scope:**
  - SA-06 (apply: exact bundle IDs, additive capabilities, Russet-named profiles, device registration, certificate creation asked once);
  - SA-07 (rest);
  - SA-08 (handlers; approvals through `ApprovalService`; `account.event`);
  - SA-10 (archive, dSYMs through the toolset `dsymutil`, IPA through `russet-ipa`);
  - `asc.get`, `provision.capabilities`, `portal.certificates.revoke` (typed confirmation), `identity.importP12/exportP12`.
- **Acceptance:**
  - a dry run for the one bundle ID that decisions.md (2026-10-11) allows (`com.evilcomputer12.russet.hello` unless the owner names another; RussetProbe maps onto it through the owner's `russet.local.toml`) lists only `RegisterBundleId` and `CreateProfile`, reusing the existing certificate;
  - DC-08 (approved by decisions.md): apply creates them, and RussetProbe installs under that **exact** bundle ID; a second run plans 0 mutations (E2E-12); nothing is revoked or deleted;
  - the archive SHA-256 is identical across two runs;
  - an MCP-kind connection cannot grant approvals.

### 5.4 W13: MCP server v1 and agent registration

- **Lane:** D, slot a. **Size:** L (6–8). **Depends on:** Wave 1. **Device:** via DC-12. **Disk:** < 1 GB.
- **Owns:** `crates/russet-{mcp,agent-config}/**`; handler, protocol and CLI `mcp`; error code file `mcp`; `schemas/mcp/**`.
- **Scope:**
  - CORE-14, MCP-01 (stdio, dual-era protocol, tool router, the ARCHITECTURE §11.4 tools whose RPC exists by Wave 2, `_meta` tier and schema, the 50 s rule, caps, `<untrusted>` wrapping, server instructions);
  - the approval error mapping for `plan_apply` and `ship_upload`;
  - MCP-04 partial (`russet mcp install` for claude-code, codex, gemini and vscode);
  - SA-23, DEV-20;
  - the MCP part of QA-10 (conformance in both eras).
- **Acceptance:**
  - the `russet mcp tools --json` snapshot equals ARCHITECTURE §11.4 for names and tiers;
  - conformance passes in both eras;
  - DC-12: after `russet mcp install --client claude-code`, Claude Code 2.1.296 runs `device_list` → `build` → `app_run` → `screenshot_take` → `logs_read` → `input_tap` on the rig, and `ship_upload` returns `perm.approvalRequired`;
  - audit lines are written.

### 5.5 W14: Extension v0 (IDE Tier 0)

- **Lane:** D (npm). **Size:** L (7–9). **Depends on:** Wave 1; W11's DAP hub for debugging; W15's QA-15 scripted daemon backend for its integration tests (W15 lands QA-15 first). **Device:** via DC-14. **Disk:** ~1 GB (`node_modules`).
- **Owns:** `extension/**` except `extension/src/generated/**`. **W14 writes the complete `package.json` contribution manifest** for every later feature (IDE §5.1, §5.6); unimplemented items stay hidden by when-clauses. After Wave 2, the manifest is lead-only.
- **Scope:**
  - EXT-01, EXT-02 (`RussetClient` on the generated protocol, `initialize`, named notifications, `ide.*` reverse requests);
  - EXT-03, EXT-04, EXT-05 (tasks, Problems from `job.events` diagnostics, Run and Stop);
  - EXT-06 (debug type `russet` → `debug.prepare {frontend: dap}` → `DebugAdapterNamedPipeServer`; `ideBridge`);
  - EXT-07 (Hot Reload → `run.hotReload`), EXT-08 (Device Console), RD-08;
  - `extension/scripts/dev-ext.sh` (Tier 0 in VS Code 1.141 with an isolated profile);
  - `@types/vscode` pinned to 1.135.x, matching `engines ^1.135.0`, so the extension cannot compile against newer APIs.
- **Acceptance:**
  - vitest and `@vscode/test-cli` integration tests pass against the scripted daemon (W15), using `fromPath: /usr/share/code/code` **and** VSCodium 1.135.06055 (the linux-x64 tarball, 243 MB, cached in `~/.cache/russet-dev/vscodium`; downloaded at tier AMBER or better);
  - DC-14: F5 on RussetProbe stops at the breakpoint in VS Code 1.141 and in VSCodium 1.135 (M3 criterion);
  - the console handles 2,000 lines/s with p95 input latency < 100 ms, measured by the integration suite through a test-only hook that timestamps a key event and its echo in the console webview; never self-reported.

### 5.6 W15: QA II — record/replay, mock device, contracts, security, Windows CI

- **Lane:** D, slot a. **Size:** L (7–9). **Depends on:** Wave 1. **Device:** via DC-13 (recording by W10). **Disk:** ~1 GB.
- **Owns:**
  - `crates/russet-testkit/**` (including the `russet-mockdevice` binary);
  - `xtask/src/{traces,contracts}.rs`, `.github/workflows/{ci,nightly}.yml`;
  - `docs/security/**`.
- **Scope:**
  - QA-06, QA-07, QA-08, QA-09;
  - QA-10 (schema and CLI snapshots, DAP script);
  - QA-15 (mock daemon backend for extension tests), landed first, because W14's integration tests need it;
  - QA-16 (fuzz targets, nightly);
  - QA-22 (security tests; threat model in `docs/security/threat-model.md`);
  - CORE-05 Windows DACL and pipe-squatting tests on `windows-2025`.
- **Acceptance:**
  - a replay-driven `russet device info` test passes on Linux and Windows CI;
  - the pymobiledevice3 differential test against the mock passes, or is skipped with a note when pymobiledevice3 is absent from the CI image;
  - every QA §8.4 security case passes;
  - fuzz targets run nightly.

### 5.7 W16: Build II — dynamic products, lints, `russet-tool`, golden/oracle, Apple-hardware smoke

- **Lane:** D, plus lane B for oracle runs; slot b. **Size:** L (8–10). **Depends on:** Wave 1. **Device:** via DC-15. **Disk:** ~2 GB, plus temporary directories on the Mac.
- **Owns:**
  - `crates/russet-{build,plist,ipa,tool,golden,oracle}/**`, `crates/russet-sdk/**` except `src/import/**` (W16b), `corpus/**`, `templates/**` except `templates/github/**` (W23), `xtask/src/golden.rs`;
  - handlers and protocol `{build,project}`;
  - CLI `{build,project,inspect,package,oracle,cache}`;
  - error code files `{build,assets}`;
  - `.github/workflows/{oracle,apple-smoke}.yml`.
- **Scope:**
  - B04, B05 (lints in `russet-build`), B06 (rest: extensions, back-deploy dylibs, privacy manifest), B08 (rest);
  - B09 (`russet-tool`: `xcrun`, `codesign` no-op, `ld64.lld` trampoline, `ibtool --version`, phase-A `actool`, installed into the platform view);
  - B11 with QA-11 and QA-12 (`russet-golden`, `russet-oracle` with the `ssh` backend as the default and the `gha` backend on `xcode-27` only, with no `macos-26` fallback; in the private phase `gha` runs only with the owner's OK, decisions.md), initial corpus;
  - **the Apple-hardware smoke** [D 76]: the archive half of the SIGN §9.9 template on RussetProbe: `xcode-27`, `Xcode_27.0.app` with a `27A266a` assertion, xtool 1.21.0 verified by hash, an unsigned archive of `xtool/RussetProbe.xcworkspace`, then a pre-sign with a throwaway identity after which `codesign -d --entitlements` shows the expected keys; no Apple credentials; at most 15 macOS minutes; one owner-dispatched run within the included minutes. W23 keeps `templates/github/russet-release.yml` and starts from this proven runner;
  - `project.init` with 0BSD templates;
  - the OpenStep half of B01.
- **Acceptance:**
  - the `.dynamic` product fixture builds through the native fallback and loads (L007);
  - the xtool #310 scenario (a package with `.xcassets`) builds through Swift Build with the phase-A `actool`;
  - `xcrun --sdk iphoneos --show-sdk-path` prints the SDK; `ld64.lld -r` works through the trampoline;
  - one actool case and one plist case run end to end through the oracle (the Mac; `gha` only with the owner's OK), and the guard proves only normalized files were uploaded;
  - the Apple-hardware smoke passed once.

### 5.8 W16b: Apple containers and native SDK/DDI import [D 62, D 69]

- **Lane:** D (lane B only for the read-only copies from the Mac), slot a. **Size:** XL (18–25 agent-days; about 14–18 if the DDI branch uses the `udif` and `hfsplus` crates). **Depends on:** Wave 1 (W5's `russet-sdk`, W3's registry). **Device:** via DC-29 (run by W10). **Disk:** about 1 GB plus the import output; the owner's `.xip` (Q27) only at tier GREEN.
- **Owns:** `crates/russet-xar/**`, `crates/russet-sdk/src/import/**`, `fuzz/russet-xar/**`, `tests/fixtures/xip/**` (synthetic only, under the provenance allowlist of ARCHITECTURE §19 [D 88]); handlers, protocol and CLI `sdk`; error code file `sdk`.
- **Scope, in this order:**
  1. **DDI first** (target: merged by day 8 of Wave 2, so W10 can make `imported` the default): the xar reader with the TOC checksum and the prehashed RSA signature chained to the pinned Apple Root CA; pbzx with bounded parallel decoding; cpio odc; UDIF raw/zero/zlib; read-only HFS+; the DDI branch (`.pkg` → `iOS_DDI.dmg` → `Restore/`) for every source of ARCHITECTURE §8.1 (`.xip`, `Xcode.app`, `.pkg`, `iOS_DDI.dmg`, Device Support `.dmg`, a DDI directory: a root holding `Restore/`, or a `Restore/` directory itself), keyed by `ddiBuild` and SHA-384-checked; `data/ddi/index.json`; `russet sdk import --ddi-only`. Lane B copies the Mac's `XcodeSystemResources.pkg` (146 MB, read-only) for the tests.
  2. **SDK import:** the iOS-only want-set of ARCHITECTURE §16.3, the deferred link pass, the post-processing ported from xtool's MIT `SDKBuilder` (with attribution), `swift-sdk.json`/toolsets/`info.json` with Linux and Windows host triples, the registry, `ImportedBundleProvider`, `deleteSourceAfter` only after both parts verify, `sdk.remove`.
  3. **The Windows materializer** (junctions, hardlinks, copy fallback, the short root, the path-budget check), with unit tests on the `windows-2022` runner.
- **Acceptance:**
  1. `russet sdk import --ddi-only <XcodeSystemResources.pkg copied from the Mac>` (or a `.xip`) reproduces exactly these SHA-256 values:
     - `Restore/022-20190-411.dmg` c7cbaf4f8a94d4b4c1fe9aad21b7c74a1848cfd0139c3030a4082b8b0f6af9b3
     - `Restore/022-20231-411.dmg` 9ab8950fb1288646b530ebba910410e8acd139a5da7856ca7f73ec5f7633e3ee
     - `Restore/BuildManifest.plist` 05beb4f7a054ea1d53f49b04aaf0a210814e4cfa7d2ba9790b03b01c48ea8a6e
     - `Restore/Restore.plist` a4c1c3cec455727fb39c860e410c6c2ffe56fb9cf2bbee30a53e3aa79a40f8a8
     - `Restore/Firmware/022-20190-411.dmg.trustcache` 04dd84b0affafedf86c7ada177a5c2289a1ad197caf7cfded9ccf1dc0a310eb0
     - `Restore/Firmware/022-20231-411.dmg.trustcache` f26711396920f65069e530b140e3d2efe197f8f9861b1d12907f6f7c696ace25
     - `Restore/Firmware/022-20231-411.dmg.cryptex_info` a17a504ab4a0e7a6531a8140af60a278fa1d098b33f7ae828407bd2ce0b97c14
     - `Restore/Firmware/022-20231-411.dmg.root_hash` 868b792cfda9fcf3bb4729a77f532a1e61dcf2f821a86d887e485f9b919fb4ea
     - `version.plist` 14ae498e21f8ade84c66b1ac6763b8625b78bf8a52134da1ca61894218168a95
  2. CI uses only synthetic xar/pbzx/cpio/UDIF/HFS+ fixtures [D 88]: xar, pbzx and cpio cases are built at test time by Rust builders in temporary directories; UDIF and HFS+ images and the fuzz seed corpora, where a builder is impractical, are generated by committed scripts from open tools and committed only under `tests/fixtures/xip/` and `fuzz/russet-xar/corpus/` with a `PROVENANCE.toml` (path, SHA-256, generator) that W8's guard checks. Cases: stored chunks, hardlinks whose first link falls outside the filter, file links chained through directory links, and `..` or absolute names (rejected). A test-CA prehashed signature passes; a tampered TOC, a tampered checksum and a wrong root each give `sdk.archiveInvalid` with the right `details.layer`. No Apple files in CI. Fuzz targets run nightly.
  3. In a local run, importing a DDI directory (the Wave-1 copy, whose root holds `Restore/` and `version.plist`) and an Apple Device Support `.dmg` both work. The `.dmg` is an older component that downloads anonymously from devimages-cdn.apple.com (for example `iPhone_16e_Device_Support.dmg`, 46.8 MB, DDI 16C7011); the lead fetches it once and deletes it after the test, and it is never committed.
  4. Env-gated (Q27): the owner's `Xcode_27.0.xip` produces the iOS-only layout and `data/ddi/27A266a/iOS/Restore/BuildManifest.plist`; files are byte-identical to xtool's bundle where the two overlap; RussetProbe builds with SDK 27.0 on both backends; the import takes ≤ 3 min on the 5600X, with peak RSS ≤ 1 GiB and ≤ 0.5 GB of temporary disk beyond the output.
  5. The Windows materializer's unit tests pass on `windows-2022` (junctions, hardlinks, path-budget refusal, case-collision refusal).
  6. `russet sdk import --dry-run <existing xtool bundle>` registers it without copying and reports `ddi: missing`.

### 5.9 Going public: checklist (owner Q6 and Q34) [D 91]

The repository becomes public only when every item holds. The lead runs the checklist; the owner decides.
1. **Q34 answered:** decisions.md's "Simulator and virtual iPhone" details moved to the private repository's own decision log, leaving one neutral line here; `cargo xtask guard public-terms` passes with an **empty** allowlist.
2. **History:** the public repository starts from a fresh, squashed history (or a filtered one, per Q34). The current history is never pushed public: commits 04e657f and 21d52f5 carry the same decision text, and their subjects name the approach.
3. **Review files:** `docs/review/critique-2026-10-11.json` carries no private-track details (two strings redacted on 2026-10-11); every later review file is checked the same way before it is committed, and round 2's critique stays out of the repository.
4. **Research files:** `docs/research/macos-on-x86-feasibility-2026-10.md` is kept, moved or removed as Q34 decides.
5. **Scans and legal:** `cargo xtask guard all` and a gitleaks scan of the full (new) history are clean (QA §8.2); legal L1–L12 pass (gate item 7); the public wording of Q26 is in place.
6. **No owner data:** no UDIDs, team IDs, Apple IDs or device names anywhere (§0 rule 9).
7. **After the switch:** set `vars.RUSSET_PUBLIC = true` (QA §4.3) so the public-phase CI matrix applies.

---

## 6. Wave 3: M2 Flutter, M3 kit, first-build panels

**Exit gate:**
1. E2E-08 and DC-30: Flutter hot reload on iOS 27 in JIT mode, with page coverage proven on the rig, which runs TXM [D 71].
2. All first-build GUI features of decisions.md work in the Tier-0/Tier-1 extension, except the publishing panel, which W23 builds in Wave 4 and G6 checks.
3. The agent panel runs Claude, Gemini, Codex, OpenCode, Copilot CLI, Cline and OpenHands CLI (DC-18; agents without an account are reported as skipped with the reason, ARCHITECTURE §21 Q29).
4. v0.3 kit packages (G3).

### 6.1 W17: Flutter build (lane C)

- **Lane:** **C**: the Flutter SDK needs about 3 GB and tier GREEN. Slot b. **Size:** L (8–10). **Depends on:** Wave 2 (W16 shims). **Device:** via DC-16. **Disk:** ~5 GB.
- **Owns:**
  - `crates/russet-build/src/stack/flutter/**`, `templates/flutter-runner/**` (0BSD; the Runner keeps its BSD-3 notice);
  - handlers and protocol `flutter_build`, CLI `cmd/flutter.rs` (the dispatcher);
  - error code file `flutter_build`;
  - `tests/fixtures/apps/RussetFlutterProbe/**`.
- **Scope:**
  - B13: engine artifacts, PATH shims, `flutter assemble` invoked the way `xcode_backend.dart` does it, the storyboard-free `RussetSceneDelegate` Runner, `flutter.register`, `flutter.device.install`;
  - B15: plugins through a generated static SwiftPM package, the podspec→SwiftPM ladder, `flutter.cocoapodsRequired`.
- **Acceptance:**
  - `flutter run -d russet-<udid>` builds a `Runner.app` with no storyboard keys and an `App.framework` with sdk 27.0, and installs it (DC-16, together with W18);
  - the two-plugin fixture builds;
  - an unconvertible plugin is named in the error.

### 6.2 W18: Flutter run/debug and the JIT servicer (lane A)

- **Lane:** **A**, slot b. **Size:** L (8–10). **Depends on:** Wave 2 (W11 broker and sessions); W17's Flutter build for E2E-08 and DC-30(b) (RD-10, H5, H8 and DC-30(a) use RussetJitProbe and need no W17). **Device:** **yes**. **Disk:** ~1 GB.
- **Owns:**
  - `crates/russet-debug/**` and `crates/russet-testkit/src/debug/**` (Wave 3);
  - handlers, protocol and CLI `flutter_run` (`cmd/flutter_run.rs`); error code files `{debug,flutter_run}` (Wave 3; `debug` gains 6016 `jitPageMissed`);
  - `tests/fixtures/apps/RussetJitProbe/**`: a C core plus a minimal shell built by the stack-1 SwiftPM pipeline, so RD-10's device validation does not wait for W17;
  - `tests/lab/**`, `docs/lab/**` (Wave 3).
- **Scope:**
  - RD-10 (native JIT servicer);
  - RD-11 (run-debug, forward, VM-service discovery, `flutter.run/attach` through `flutter run --machine`, the Flutter side of `run.hotReload`);
  - RD-15 (interposer, behind a flag), RD-20 (LLDB Python fallback), RD-19 (JIT p50/p95);
  - the TXM test plan [D 71]: host test H8 (the TXM-model fake debugserver, run-debug §23.2); `JitHook` discovery that accepts any loaded image exporting `_NOTIFY_DEBUGGER_ABOUT_RX_PAGES`, preferring Flutter.framework; `pagesNotified`/`pagesTouched` in `jit.stats`; the `debug.jitPageMissed` diagnosis (6016);
  - device checks D9/D10 and DC-30;
  - running device checks for W17 and W19–W22.
- **Acceptance:**
  - E2E-08: hot reload ≤ 2 s; stdout shows "Dart execution mode: JIT" on iOS 27; DevTools is reachable; no JIT faults (`EXC_BAD_ACCESS`/`SIGBUS`, no new `.ips`) and `pagesTouched == pagesNotified`;
  - DC-30 passes on the rig, including the negative control;
  - JIT statistics are recorded;
  - H5 and H8 pass on the host.

### 6.3 W19: IDE panels (first-build GUI features)

- **Lane:** D. **Size:** L (9–12). **Depends on:** Wave 2 (W14, W10). **Device:** via DC-17. **Disk:** ~1 GB.
- **Owns:**
  - `extension/src/{panels,editors,setup}/**`, `extension/webview/**`, `extension/data/**`;
  - excluded: `extension/src/agents/**`, `extension/webview/{agents,approval}/**` (W20), `extension/src/editors/assets/**` and `extension/webview/assets/**` (W21).
- **Scope:**
  - SPK-01 (decode matrix and the `Origin` value);
  - EXT-09 (mirror webview with input);
  - EXT-10 (Info.plist editor and key catalog);
  - EXT-11 (entitlements, Signing & Capabilities, plan display, Apply through approval);
  - EXT-14 (profiler), EXT-15 (crash viewer);
  - EXT-16 (walkthrough, doctor, Xcode import through `sdk.import`, ASC key wizard; the Apple ID UI behind a flag until W22);
  - SA-09.
- **Acceptance** (DC-17 for the mirror items):
  - mirror ≥ 30 fps HEVC on this host; glass to glass < 250 ms, measured rather than self-reported: RussetProbe's clock mode (added by W10) renders a frame counter and a QR code of the device clock, and the test decodes them from the received frames and compares them with the host clock, corrected by the device-time offset; taps within 2 px at three probe points; automatic fallback to AVC;
  - plist fixtures round-trip byte-identically;
  - Apply needs a human approval;
  - a forced crash is shown symbolicated within 30 s;
  - the profiler grows < 20 MB over 10 minutes at 1 Hz;
  - a fresh profile completes the walkthrough with no secrets in settings.

### 6.4 W20: Agents II — approvals UI, HTTP MCP, nine clients, ACP panel, local models

- **Lane:** D, slot a. **Size:** L (8–10). **Depends on:** Wave 2 (W13, W14). **Device:** via DC-18. **Disk:** ~1 GB.
- **Owns:**
  - `crates/russet-{mcp,agent-config}/**`, `crates/russetd/src/web/mcp.rs`;
  - handlers, protocol and CLI `mcp`;
  - `extension/src/agents/**`, `extension/webview/{agents,approval}/**`, `schemas/mcp/**`.
- **Scope:**
  - MCP-02, MCP-03 (HTTP endpoint in `russetd`), MCP-04 (rest, with SPK-03);
  - EXT-17;
  - AGT-01, AGT-02, AGT-03;
  - AGT-04: the ripple overlay as a component that W19's mirror mounts through a documented hook.
- **Acceptance:**
  - every QA §8.4 case passes;
  - a Docker container with host-gateway lists the tools over HTTP;
  - DC-18: claude-agent-acp, `gemini --acp`, codex-acp, OpenCode, Copilot CLI (`--acp`), Cline and OpenHands CLI (`openhands acp`) each list devices, build and take a screenshot from the panel; Continue (MCP config) and the OpenHands GUI (HTTP MCP from Docker) reach at least `tools/list` plus one tier-0 call; agents without an account are reported `skipped (no account)` with the reason (ARCHITECTURE §21 Q29);
  - `llama-server --jinja` is detected on 9931 or 8080.

### 6.5 W21: Resource compilers (lane B oracle)

- **Lane:** D plus B, slot b. **Size:** XL (12+). **Depends on:** Wave 2 (W16). **Device:** via DC-19. **Disk:** ~2 GB.
- **Owns:**
  - `crates/russet-{car,assets,xcstrings,ib}/**`;
  - `crates/russet-tool/src/{actool,ibtool,xcstringstool}.rs`, `crates/russet-build/src/resources/**`;
  - handlers, protocol and CLI `assets` (plus the `strings` and `ib` CLI); error code file `assets`;
  - `corpus/{actool,xcstrings,ib}/**`, `extension/src/editors/assets/**`, `extension/webview/assets/**`, `docs/re/assets/**`.
- **Scope:**
  - B10, B12 (with the launch-storyboard fidelity classification: lossless, or lossy with reasons in the build report; the `launch-storyboard` setting);
  - SPK-04 (the Xcode 27 `AppIcon`/`AccentColor` `Contents.json` spec, lane B, read-only);
  - EXT-12;
  - the RussetProbe asset-harness mode (E2E-15).
- **Acceptance:**
  - every required corpus case is semantically equal to the oracle; output is deterministic;
  - DC-19: the app icon shows on the phone, and every E2E-15 line reads `ASSET ok`.

### 6.6 W22: Free Apple ID (opt-in)

- **Lane:** D, slot b. **Size:** XL (12+). **Depends on:** Wave 2 (W12). **Device:** via DC-20 (owner Q4). **Disk:** ~0.1 GB, plus about 2 MB downloaded at runtime after consent.
- **Owns:**
  - `crates/russet-{gsa,anisette,adi-host}/**`, `crates/russet-account/src/free/**`, `crates/russet-asc/src/ds2.rs`;
  - the `account.appleId.*` and `account.anisette.*` rows of `handlers/account.rs`;
  - error code files `{gsa,anisette}`.
- **Scope:**
  - SA-18;
  - SA-19 (Linux `russet-adi-host` with seccomp and Landlock, HTTP-range fetch < 3 MB, pins);
  - SA-20, SA-21.
- **Acceptance:**
  - the mocked suites pass (`GsaFake`, `AnisetteFake`);
  - live login, provisioning and install happen only with a throwaway Apple ID (DC-20), or are recorded as blocked on Q4;
  - the known 0xe8008015 behaviour is documented.

---

## 7. Wave 4: ship, Flutter AOT, first IDE build, release engineering

**Exit gate:**
1. G5: E2E-11 and E2E-16, TestFlight `VALID` and installed on the phone through both paths; the App Store version and submission sequences pass against wiremock (a live submission only with Q32).
2. G6: IDE packages, the L6b smoke tests and E2E-19, including the publishing panel.
3. CLI v0.x packages are signed and smoke-tested.
4. G4 (second half): W21b's launch storyboards compile (DC-32); W32's SwiftPM XCTest runs on the phone (DC-34).

### 7.1 W23: Ship (M5)

- **Lane:** D, slot b. **Size:** L (9–12). **Depends on:** Wave 3 (W21 for store icons; W12 archive). **Device:** via DC-21 (owner approvals: app record, distribution certificate (Q5) and upload). **Disk:** ~2 GB.
- **Owns:**
  - `crates/russet-ship/**`; handlers, protocol and CLI `ship`; error code file `ship`;
  - `extension/src/panels/publishing*`, `extension/webview/publishing/**`;
  - `templates/github/**` (`russet-release.yml` and the `.russet/ci/` helper templates);
  - `.github/workflows/apple-release.yml` (Russet's own Apple-hardware release of the fixture app, G5; W8's `upload-safe` allowlists its outputs) [D 86].
- **Scope:**
  - SA-11 to SA-17, including the App Store and TestFlight methods of ARCHITECTURE §8.4 [D 64]: `ship.version.plan/apply`, `ship.review.submit/status`, TestFlight groups (list, create), testers (list, add, remove), distribute with export compliance, beta review;
  - SA-24;
  - B18;
  - EXT-13: the publishing panel (versions, export compliance, submission with typed confirmation, TestFlight groups and testers);
  - the Apple-hardware template of SIGN §9.9 with the signing modes `russet` (default), `xcode-manual` and `xcode-cloud` [D 76];
  - stamping through the shared metadata module (ARCHITECTURE §16.4).
- **Acceptance:**
  - E2E-11 dry run sends zero POST, PATCH or DELETE requests;
  - wiremock suites assert the exact request sequences: `ship.version.apply` (POST `appStoreVersions` with `app` and `build` relationships, or PATCH an existing version's `build` relationship; PATCH `builds/{id}` `usesNonExemptEncryption` only when the plan has it), `ship.review.submit` (POST `reviewSubmissions`, POST `reviewSubmissionItems`, PATCH `reviewSubmissions/{id}` with `submitted: true`) and the TestFlight methods (POST `betaGroups`, POST `betaTesters`, POST `builds/{id}/relationships/betaGroups`, POST `betaBuildLocalizations`, POST `buildBetaNotifications`, POST `betaAppReviewSubmissions`); a typed-confirmation mismatch sends nothing;
  - with owner approval, the E2E-16 canary reaches `VALID` and installs through TestFlight from an internal group managed through Russet (DC-21), on both the Linux-native and the Apple-hardware path (signing mode `russet`). No Apple Distribution certificate is created until the owner answers Q5 (ARCHITECTURE §21): then one is created through the API, or the owner's exported one is imported; until then the plan lists `CreateCertificate` and stops;
  - the generated workflow is zizmor-clean; one `xcode-manual` run, with owner approval, exports per-bundle entitlements (app and each appex) equal to the Linux-native IPA's in `ship compare`; `xcode-cloud` runs live only with an Admin key, otherwise its "Cloud signing permission error" path maps to 7009 in a recorded-log test;
  - a live App Store review submission happens only with the owner's approval (ARCHITECTURE §21 Q32); otherwise `russet ship submit --dry-run` and the wiremock sequence stand, and the live run is a recorded deferral;
  - the comparison report is committed.

### 7.2 W24: Flutter release (AOT)

- **Lane:** C (CI), slot b. **Size:** L (6–8). **Depends on:** Wave 3 (W17). **Device:** via DC-22. **Disk:** ~1 GB locally; the ~8 GB Dart build runs only in CI.
- **Owns:**
  - `.github/workflows/gen-snapshot.yml` and `tools/gen-snapshot/**`, or a separate public repository (owner Q18);
  - `crates/russet-build/src/stack/flutter/aot.rs`;
  - the `flutter.genSnapshot.*` rows of `handlers/flutter_build.rs`.
- **Scope:** B14.
- **Acceptance:**
  - the linux-x64 release and profile compilers for Flutter 3.47.7 are byte-identical to the macOS reference, and the client verifies their sha256;
  - DC-22: a release-mode app runs AOT on the phone with `App.framework` sdk 27.0.

### 7.3 W25: IDE product build (G6)

- **Lane:** C. The Tier-1 repack runs locally (≤ 1 GB). **Tier-2 rule** [D 77]: Tier-2 builds run in CI (`ide.yml`) only once the repository is public; while it is private they run locally at tier BLUE with the owner's OK (a Linux plus Windows `ide.yml` run costs about 250–350 Linux-equivalent minutes, IDE §3.13, which the private budget of QA §4.8 cannot carry). **Size:** L (10–14). **Depends on:** Wave 3 (W14, W19, W20). **Device:** via DC-23. **Disk:** ~2 GB locally.
- **Owns:** `ide/**`, `.github/workflows/ide.yml`, `extension/scripts/package*`, `packaging/aur/russet-ide-bin/**`.
- **Scope:**
  - IDE-01 to IDE-09, re-pinned to the newest VSCodium stable at the start [D 34];
  - QA-20;
  - REL-01, with W26.
- **Acceptance:**
  - the Tier-1 repack runs as Russet on this host: Open VSX gallery, telemetry off, built-in extensions active, and the fallback run controls work (`editor/title/run` Run/Debug/Hot Reload, the Devices `view/title` actions, the status-bar device and target items). A Tier-1 repack cannot carry the `9xx` patches, so it is never checked for the title bar [D 78];
  - the Tier-2 CI build has the title-bar Build/Run/Debug/Hot Reload toolbar (IDE-06), checked by L6b and E2E-19;
  - CI produces deb, rpm, tar.gz, AppImage, and the Windows user setup and zip;
  - L6b and E2E-19 pass on the CI-built package (DC-23 downloads it, about 250 MB, instead of repacking); legal L8 passes.

### 7.4 W26: Release engineering, packaging, docs, legal

- **Lane:** D, slot a. **Size:** L (8–10). **Depends on:** Wave 3. **Device:** no. **Disk:** ~2 GB.
- **Owns:**
  - `packaging/linux/**` and the initial `packaging/windows/**` (CLI; Windows packaging is validated and then owned by W27c);
  - `.github/workflows/{release,publish,docs}.yml`;
  - `xtask/src/{release,legal,docs,pins}.rs`;
  - `docs/site/**`, `docs/reference/errors.md`;
  - `crates/russet-cli/src/cmd/self_update.rs`, `CHANGELOG.md`, `cliff.toml`.
- **Scope:**
  - CORE-11 (rest: `xtask pins` in CI, `toolchain.pins.update`), CORE-16, CORE-19, CORE-21;
  - QA-14 (per owner Q7: on-demand lab runs, no always-on runner), QA-18, QA-19 (public phase), QA-21, QA-23;
  - SA-25;
  - the legal review of ARCHITECTURE §21 Q26 (Xcode 27's EA2002 and the Device Support EA1879 for SDK and DDI use off Apple hardware; the public wording);
  - Linux release builds in an `ubuntu:22.04` container on `ubuntu-24.04` runners [D 47].
- **Acceptance:**
  - an rc dry run produces tar.zst, deb, rpm, the AUR package, checksums with minisign, an SBOM and the Corresponding Source;
  - the packages install in Debian 12, Ubuntu 24.04, Fedora 41 and Arch containers, and `russet doctor --json` runs there;
  - legal-check passes; the docs site builds with the generated references.

### 7.5 W21b: Storyboard compiler I — NIBArchive and launch storyboards (B21a) [D 75]

- **Lane:** D, plus B for oracle runs in owner-approved windows (§0 rule 5); slot b. **Size:** L (6–9 agent-days with the port). **Depends on:** Wave 3 (W21's `russet-ib` reader and B12; W16's `russet-tool` and oracle). **Device:** via DC-32. **Disk:** < 1 GB.
- **Owns:** `crates/russet-ib/src/{nib,compile}/**`, `crates/russet-tool/src/ibtool.rs`, `corpus/ib/**`; handlers, protocol and CLI `ib` (the namespace W1 pre-registered for `ib.compile` and `ib.dump`).
- **Scope:** B21a: the NIBArchive codec (coder version 10; the iOS `LNE\0` trailer and `IBUINibLNEVersionKey`; class fallback entries; the CF key-order emulation, used to measure byte identity), the IB XML model and the launch-storyboard compiler (UIViewController, UIView, UIImageView, UILabel/UIFont, colours, constraints with multiplier/constant/priority, safe-area and legacy layout guides, the deployment-target-dependent canvas), plus a **single-scene main storyboard whose view controller has an ObjC `customClass` without module mangling** (Flutter's `Main.storyboard` with `FlutterViewController`; storyboard `Info.plist` and the `--link` step included) [D 90]. Custom Swift classes, outlets, actions and segues stay in B21b (W28b). Ported from omarchy-apple-dev `tools/ibtool`, `nibarchive.py` and `keyorder.py` (MIT, with attribution), with WinObjC xib2nib (MIT) as a second reference. The `ibtool --compile/--compilation-directory/--link/--version` contract in `russet-tool`; `[build] launch-storyboard = "auto"`.
- **Acceptance:**
  1. Oracle corpus `corpus/ib/`: Flutter LaunchScreen and Main, the React Native LaunchScreen, flutter_native_splash variants, NetNewsWire LaunchScreenPhone/Pad and Russet-authored equivalents of the Xcode templates, each at deployment targets 15.0, 17.0 and 26.0, for iphone and ipad. The decoded-graph JSON equals Xcode 27.0 `ibtool --cocoatouch-compiler-mode toolchain` output; byte identity is a reported metric, not a gate. Only normalized JSON is committed (ARCHITECTURE §19).
  2. Output is deterministic.
  3. Unsupported elements give 4101 with file:line and the element, never partial output.
  4. DC-32: a held-launch screenshot shows the compiled React Native launch screen's labels, and `UIStoryboard(name: "Main")` instantiates FlutterViewController.

### 7.6 W32: SwiftPM XCTest on device (`russet test`, stack 1) [D 80]

- **Lane:** D, slot a. **Size:** L (6–8). **Depends on:** Wave 2 (W10's DDI and device services, W11's sessions, W16's build). **Device:** via DC-34. **Disk:** < 1 GB.
- **Owns:** `crates/russet-xctest/**` (until W28 takes it over in Wave 6); handlers, protocol and CLI `test`; error code file `xctest`; `tests/fixtures/apps/RussetProbe/Tests/**` and the test target in RussetProbe's `Package.swift` (Wave 4).
- **Scope:** `test.list` and `test.run` for SwiftPM test targets: build the `.xctest` bundle and a Russet-generated host app, sign, install, run through idevice `xctest` (testmanagerd; needs the DDI), JUnit output; `russet test [--filter F] [--junit out.xml]`.
- **Acceptance:** DC-34: RussetProbe's test target (one passing, one failing and one skipped case) runs on the phone and the JUnit report matches; a missing DDI gives 3005 with the `russet device prepare` hint; host CI runs the planner against the scripted backend.

---

## 8. Wave 5: Windows (M6)

W27 is three cards [D 79]. **PC access and hand-over** (ARCHITECTURE §21 Q31): lane A works on the physical Windows 11 PC over OpenSSH (key authentication, the agent's normal non-admin account) from the Linux box, in windows announced in `docs/lab/queue.md`. At the start of a window the owner moves the iPhone to the PC, and back at the end; Linux device work pauses meanwhile. The live-device lock applies on the PC too.

### 8.1 W27a: Windows core, IPC, device and debug

- **Lane:** **A on the Windows PC**. **Size:** L (8–10). **Depends on:** Wave 4. **Device:** **yes** (Windows). **Disk:** ~10 GB on the PC (Swift for Windows installer 2.1 GB, VS Build Tools, LLVM 0.3 GB).
- **Owns:** the `cfg(windows)` platform modules listed in core §15 (`crates/**/src/**/windows*.rs`, the `russet-ipc` pipe transport, the `russet-secrets` backend, spawn), `docs/windows/**`.
- **Scope:** CORE-17 (Job Objects, spawn flags, long paths, the manifest, AMDS detection, Credential Manager); DEV-22; RD-18; the Windows half of the live-device lock; the device and debug subset of E2E-18 (DC-27).
- **Acceptance:** DC-27: the E2E-01–07 criteria on Windows 11 x64 with the phone attached to Windows (CLI, daemon, device, debug through LLDB and the TCP broker); the pipe DACL is verified; doctor reports which Apple Mobile Device Service flavor is installed.

### 8.2 W27b: Windows build plane, SDK and DDI import

- **Lane:** **A on the Windows PC** (+ D for CI work). **Size:** L (8–10). **Depends on:** W27a and W16b. **Device:** **yes** (DC-31).
- **Owns:** `crates/russet-build/src/platform/windows/**`, `crates/russet-sdk/src/import/windows/**`, `.github/workflows/oam-windows.yml`.
- **Scope:** B17 (the native build plane: the junction view, `.exe` shims including `llvm-lib.exe` and the `ld64.lld` trampoline, response files, short paths, the `native` backend first); validating W16b's importer on the PC (no WSL, no admin, no Developer Mode); the long-path matrix (`LongPathsEnabled` 0/1 × native/swiftbuild backends); a windows-x64 OpenAppleMacrosServer CI build and plugin stubs; the Flutter-on-Windows spike; CORE-20 (WSL split mode) only if the native build plane fails (build Q7, settled).
- **Acceptance:** DC-31: on Windows 11, `russet sdk import Xcode_27.0.xip` succeeds (Q27) and reports a maximum absolute path ≤ 259; RussetProbe builds natively, launches and debugs on the phone attached to Windows, with the DDI mounted from that import; the M2 Flutter demo passes, or the spike result is documented with the fallback chosen.

### 8.3 W27c: Windows packaging, IDE and anisette

- **Lane:** D (+ A for checks on the PC). **Size:** M (4–6). **Depends on:** W27a.
- **Owns:** `packaging/windows/**`, `crates/russet-anisette/src/windows/**`.
- **Scope:** CORE-18 (zip, per-user MSI, winget via komac, the signing decision of Q8); IDE-05 verification on the PC; SA-22 (Credential Manager secrets end to end, the CNG identity option, adi-host x64 shims).
- **Acceptance:** the MSI installs and uninstalls without admin; `winget install --manifest <local>` works; the Windows IDE build passes the L6b smoke on the PC; the paid account flow works end to end; anisette provisioning succeeds with Q4's throwaway Apple ID, or its blocker is documented.

---

## 9. Wave 6: M7 ecosystem and Wi-Fi

### 9.1 W28b: Storyboard compiler II — main storyboards and XIBs (B21b) [D 75, D 90]

- **Lane:** D, plus B for oracle runs in owner-approved windows; slot b. **Size:** XL (12–17 agent-days). **Depends on:** W21b. **Device:** via DC-33.
- **Owns:** `crates/russet-ib/**` and `corpus/ib/**` (Wave 6, until W28c takes them over), `tests/fixtures/apps/RussetProbe/IBHarness/**`.
- **Scope:** B21b: main storyboards and XIBs: custom classes as `_TtC<len><Module><len><Class>` via `--module`; outlets, outlet collections and actions; segues and storyboard identifiers; navigation and tab controllers; common controls; runtime attributes; SF Symbols; catalog colours and images; Base.lproj plus `.strings`; XIB → `.nib` with `~iphone`/`~ipad` variants.
- **Acceptance:** DC-33 (without NetNewsWire): Xcode "Storyboard App" equivalents (Swift and ObjC) launch, and RussetProbe's IB-harness mode dumps view trees on the device (class, frame after layout, colours, text, outlets bound, actions fired by an HID tap) that are equal for Russet-compiled and oracle-compiled nibs (the oracle's raw nibs stay local).

### 9.2 W28: Xcode projects, CocoaPods, `russet test`

- **Lane:** D, plus B for the PIF corpus. **Size:** XL. **Depends on:** Wave 4 (W21b) and W28b: the parsers, PIF and `russet-swb` work may start alongside W28b, but DC-24's Storyboard App needs W28b merged [D 90]. **Device:** via DC-24.
- **Owns:**
  - `crates/russet-{xcodeproj,pif,xctest}/**`, `tools/russet-swb/**` (Swift), `crates/russet-build/src/stack/xcode/**`;
  - handlers, protocol and CLI `{xctest,xcode,test}`; error code files `{xcode,xctest}`;
  - `corpus/pif/**`.
- **Scope:** B16; `russet test` for Xcode projects and XCUITest (JUnit output, through idevice `xctest`), extending W32's `russet-xctest`.
- **Acceptance:**
  - DC-24: three Xcode-project apps, chosen by their Interface Builder needs, build from `.xcodeproj` on Linux and launch on the phone: IceCubesApp (no storyboards), the React Native template's iOS app (B21a, from W21b) and an Xcode "Storyboard App" equivalent (B21b, from W28b). NetNewsWire iOS follows in W28c (DC-35);
  - `russet-swb` builds a project with `IBC_EXEC=russet-tool ibtool`, which W28c uses for NetNewsWire;
  - an XCUITest suite produces JUnit.

### 9.3 W28c: Storyboard compiler III — tables, collections, traits (B21c) and NetNewsWire [D 75, D 90]

- **Lane:** D, plus B for oracle runs in owner-approved windows; slot b. **Size:** L (8–10 agent-days). **Depends on:** W28b and W28 (`russet-swb`). **Device:** via DC-35.
- **Owns:** `crates/russet-ib/**`, `corpus/ib/**` and `tests/fixtures/apps/RussetProbe/IBHarness/**` (after W28b merges).
- **Scope:** B21c: tables, collections, split views, trait storage and size classes, accessibility configurations; NetNewsWire iOS end to end.
- **Acceptance:** DC-35: the IB harness covers B21c's elements with equal view trees for Russet- and oracle-compiled nibs; NetNewsWire iOS (4 storyboards, 4 XIBs) builds through `russet-swb` with `IBC_EXEC=russet-tool ibtool`, launches and navigates to Settings.

### 9.4 W29: React Native / Expo adapter and .NET MAUI study

- **Lane:** D, plus C for the Node/RN toolchain. **Size:** XL. **Depends on:** W28. **Device:** via DC-25.
- **Owns:** `crates/russet-build/src/stack/{rn,maui}/**`, `docs/design/maui-feasibility.md`.
- **Scope:** B19; B20 as a study only (ARCHITECTURE §21 Q30, [D 80]).
- **Acceptance:**
  - DC-25: the Expo template app builds on Linux and runs with Metro on the phone;
  - the MAUI report gives a go/no-go.

### 9.5 W30: Device plane III — Wi-Fi and native accessibility (lane A)

- **Lane:** **A**. **Size:** L. **Depends on:** Wave 2 (W10, W11). **Device:** **yes**; owner Q22 for toggling `EnableWifiDebugging`.
- **Owns:** `crates/russet-device/src/{wifi,a11y}/**`, `crates/russet-debug/src/wifi.rs`.
- **Scope:** DEV-21, RD-17, DEV-20b.
- **Acceptance:**
  - DC-26: with the cable unplugged, install, launch and debug work over Wi-Fi on a LAN without client isolation;
  - accessibility elements come back with frames, and `input_tap` by `elementId` works.

---

## 10. Parallel track

### 10.1 W31: Virtual iPhone (separate private track)

- **Where:** a separate private repository (`russet-vphone`; decisions.md 2026-10-11). Its design, research and spikes live there, not in this repository.
- **Interface to Russet:** if the track succeeds and passes its legal review, it may later ship as a separately distributed, opt-in module that appears to `russet-device` like any other device. Nothing in this plan depends on it, and it never blocks a wave.
- **Shared resources:** it uses the owner's Mac under the lane-B rules (§0 rule 5), in windows that never overlap oracle runs.
- **Evidence rule:** runs on the virtual device are not evidence for retail-device behaviour such as TXM JIT page touching (different security configuration).
- **Owns in this repository:** nothing.

### 10.2 W33: Apple Device Support fetch (deferred)

- **Starts:** only after the owner answers ARCHITECTURE §21 Q25. Not on any critical path: Apple publishes no Device Support component for Xcode 27 today.
- **Scope:** `sdk.deviceSupport.check` and `sdk.deviceSupport.fetch` (ARCHITECTURE §8.1): Apple's downloadables index (and later Pallas) for `xcodeDeviceSupport` entries matching the imported Xcode; downloads only from Apple hosts under the user's IP; the `.pkg` signature check; hand-off to W16b's DDI pipeline.
- **Acceptance:** against a recorded index and an older Device Support component (for example 16C7011), the fetched DDI imports and verifies; with the live index the check reports `{kind: none, forXcode: 27A266a}`.

---

## 11. Device check catalog (lane A queue)

| DC | Requested by | Check | Pass | Wave |
|---|---|---|---|---|
| DC-01 | W4 | Device §15.4 HIL budgets for the Wave-1 capabilities | budgets met | 1 |
| DC-02 | W5 | RussetProbe built with sdk 27.0 and the scene manifest launches (TN3187) | app runs; heartbeat seen | 1 (in W9) |
| DC-03 | W6 | A Russet-signed app (imported xtool identity, existing profile) installs and launches | install OK, pid returned | 1 (in W9) |
| DC-04 | W7 | D1: CoreDevice `startStopped` + stdio shape; D2: behaviour when the launch channel closes; D3: broker sync `process connect` stops at a breakpoint | D3 stops without Python commands | 1 (D1/D2 in W4, D3 in W9) |
| DC-05 | W9 | The §4.10 exit script (`demo/m1-russet.sh`; gate items 1–6), three runs in a row | all pass | 1 |
| DC-06 | W10 | Mirror, HID and screenshot budgets | §5.1 numbers | 2 |
| DC-07 | W11/W14 | E2E-05 DAP path; agent and IDE share a session | stop, variables, step | 2 |
| DC-08 | W12 | Exact-ID provisioning of the one bundle ID decisions.md allows (Q1, answered); E2E-12 idempotency | exact ID installed; 0 mutations on the 2nd run | 2 |
| DC-09 | W11 | D7 `po` experiment (symbol fetch only with Q13) | status documented | 2 |
| DC-10 | W11 | Forced crash symbolicated (T5) | file:line ≤ 30 s | 2 |
| DC-11 | W10 | SPK-02: AVC-only offer; rotation vs touch coordinates | facts recorded | 2 |
| DC-12 | W13 | E2E-13 MCP agent loop with Claude Code | tier 0–1 succeed; `ship_upload` needs approval | 2 |
| DC-13 | W15 | Record RDT traces (install, launch, logs, screenshot) | trace lint clean | 2 |
| DC-14 | W14 | F5 in VS Code 1.141 and in VSCodium 1.135 stops at the breakpoint (M3) | stopped event in both | 2 |
| DC-15 | W16 | App with a `.dynamic` product, and with phase-A `actool` assets, launches | runs | 2 |
| DC-16 | W17/W18 | Flutter debug build installs and runs | VM-service line seen | 3 |
| DC-17 | W19 | Mirror webview fps, latency, tap accuracy | §6.3 numbers | 3 |
| DC-18 | W20 | ACP and MCP agents drive the rig: claude-agent-acp, `gemini --acp`, codex-acp, OpenCode, Copilot CLI, Cline, OpenHands CLI; Continue and the OpenHands GUI at least `tools/list` plus one tier-0 call | each listed agent lists, builds, screenshots; agents without an account are `skipped (no account)` (Q29) | 3 |
| DC-19 | W21 | E2E-15 asset harness; icon visible | all `ASSET ok` | 3 |
| DC-20 | W22 | E2E-17 free Apple ID (owner Q4) | install OK, or 0xe8008015 documented | 3 |
| DC-21 | W23 | E2E-16 TestFlight canary (owner approvals, including the distribution certificate of Q5), through an internal group managed by Russet, on both paths | `VALID` and installed through TestFlight | 4 |
| DC-22 | W24 | Flutter release (AOT) app runs | runs; sdk 27.0 | 4 |
| DC-23 | W25 | E2E-19 IDE device smoke on the CI-built (Tier-2) package | E2E-03/05/06/07 through the UI | 4 |
| DC-24 | W28 | Three Xcode-project apps chosen by IB need (IceCubesApp, the RN template, a Storyboard App equivalent) launch; NetNewsWire is DC-35 | all run | 6 |
| DC-25 | W29 | RN/Expo template runs with Metro | runs | 6 |
| DC-26 | W30 | Wi-Fi T8 (owner Q22) | install, launch, debug without a cable | 6 |
| DC-27 | W27a | E2E-18 device and debug subset on the physical PC (over SSH; the owner moves the phone, Q31) | E2E-01–07 criteria | 5 |
| DC-28 | W4 (run in W9; again in W10) | DDI provenance and reboot [D 62, D 82]: `tests/lab/dc28-ddi.sh` (§4.10), after the lead's DDI copy and one owner-approved reboot (Q33) → `russet device ddi status` (record whether the 27A5228h cryptex survived) → `russet device prepare` mounts from `[device.ddi] dir` (the Apple-origin 27A266a copy) with no pymobiledevice3 on PATH; if a DDI is still mounted, `russet device ddi mount --dir … --force` (owner-approved) replaces it, or cryptexd's refusal is recorded → launch and debug RussetProbe | mounted `productBuild 27A266a`, `source: dir` from Russet's mount record; the breakpoint hits. Exit 3 = blocked on the owner | 1 (in W9) |
| DC-29 | W10 / W16b | Imported DDI [D 89]: `russet sdk import --ddi-only` of the Mac's `.pkg` copy (or the `.xip`) → with a Q33-approved reboot or an owner-approved `russet device ddi mount --imported --force`, the import is mounted → launch and debug | mounted from the import with idevice alone (`source: imported` in the mount record); the breakpoint hits. Without the approval: "import verified, mount not re-exercised" | 2 |
| DC-30 | W18 | TXM JIT validation on the rig (iPhone 12, iOS 27.0, TXM) [D 71]: (a) RussetJitProbe runs code in every page of regions of 1, 2, 31, 32, 33 and 2048 pages, an unaligned region, and two threads announcing at once; a never-announced control region must fault with `EXC_BAD_ACCESS` (proves the device enforces); `security.codesigning.monitor` is logged if readable. (b) Flutter jit-stress: at least 8 × 512 KiB code regions plus 20 hot reloads. (c) The RDT trace is recorded | (a) all pages run and the control faults; (b) no `EXC_BAD_ACCESS`/`SIGBUS`, no new `.ips`, `pagesTouched == pagesNotified`; (c) trace lint clean | 3 |
| DC-31 | W27b | Windows native SDK and DDI import, build and debug | as W27b's acceptance | 5 |
| DC-32 | W21b | Compiled launch storyboards on the phone | the RN launch screen's labels show in a held-launch screenshot; `UIStoryboard(name: "Main")` gives FlutterViewController | 4 |
| DC-33 | W28b | IB harness view trees for B21b (main storyboards, XIBs) | equal view trees for Russet- and oracle-compiled nibs | 6 |
| DC-34 | W32 | SwiftPM XCTest on the phone | the JUnit report matches the expected pass/fail/skip | 4 |
| DC-35 | W28c | IB harness view trees for B21c (tables, collections, split views, traits); NetNewsWire iOS through `russet-swb` | equal view trees; NetNewsWire launches and reaches Settings | 6 |

---

## 12. Interface freeze points

| Freeze | When | What becomes stable | Change process afterwards |
|---|---|---|---|
| F0 | W1 merged | Crate list, layering, seam files (with every `cargo xtask` subcommand registered), ownership map, `[workspace.dependencies]`, and **every** ARCHITECTURE §5.3 public signature as compiling stubs (`todo!()` bodies), including the typed `DaemonCtx` fields and the types they name (`ConfigService`, `DeviceManager`, `DebugService`, `AccountService`, …, all defined in §5.3); every §13.3 error-code constant; each stub crate's key dependencies; the pre-created module files of §3 [D 74, D 86] | Lead PR |
| F1 | W2, day 2 (contract PR) | The `russet-core` and `russet-ipc` bodies behind the F0 signatures (error/`ErrorCode`/`ErrorData`, ids, dirs, `CommandRunner`, `ProgressSink`/`EventSink`/`JobEventKind`), the `Router` and handler signature, the `initialize`/`job.*`/`approval.*` DTOs. Signature changes only by lead-approved refinement of F0 | Lead PR; dependants rebase |
| F2 | Day 4 of W3, W4, W5 and W6 (contract PRs, after F1) | Refinements of the domain signatures, still with stub bodies returning `core.notImplemented`: `SecretStore`, `Toolchains`/`ToolInfo` (W3); `DeviceManager`/`DeviceHandle`/`ServiceConnector`/`ddi` (W4); `Builder`/`BuildOutput`/`SigningPlan`/`BuiltApp`/`SdkFacts` (W5); `CodeSigningIdentity`/`sign`/`Provisioner`/`ProvisioningProfile` (W6). They compile against F1; W7's pipeline compiles against them | Owner + lead review |
| F3 | End of Wave 1 | Protocol 1.0: every Wave-1 row of ARCHITECTURE §8 | Minor version for additions; major needs the lead and a CHANGELOG entry |
| F4 | Day 2 of Wave 2 | The extension `package.json` manifest (W14), the MCP tool catalog snapshot (W13), the `russet.mirror.v1` route (W10) | Lead PR |
| F5 | Start of Wave 3 | Approval UI contract, `assets.*` RPC, the Flutter custom-device JSON | Lead PR |

The order is acyclic: F0 → F1 → F2 → F3. Nothing in F1 names a type that only F2 creates, because F0 already created it.

---

## 13. Plan risks

| # | Risk | Mitigation | Trigger to act |
|---|---|---|---|
| P1 | One phone, one lane-A agent: the bottleneck | Mocks, replay and scripted connectors first; batched device checks; lane-A owners per wave | Queue older than 2 days |
| P2 | Linking with sdk 27.0 turns on iOS 27 behaviour, and RussetProbe may not launch | Scene manifest always; diagnosis override `RUSSET_LINK_SDK_VERSION=deployment`; DC-02 run early in W9 | DC-02 fails |
| P3 | The broker fails on the device (D3) | `attach-mode = legacy-async` (the M1-verified sequence) keeps the slice green | DC-04 D3 fails |
| P4 | apple-codesign: no release since 2024-11; pulls reqwest 0.12; may pull aws-lc | Vendor or patch through a reviewed fork; `apple-codesign-quick` (LGPL) as a reviewed exception | Build or deny failure in W1/W6 |
| P5 | idevice 0.1.68 on crates.io differs from the spike's git rev | Dedicated runtime for the listen stream; reviewed git exception if a fix is missing | W4 blocker |
| P6 | Protocol churn across seven parallel streams | Seams, freeze PRs, regenerate rather than merge, lead-only seam files | Repeated rebase conflicts |
| P7 | Owner approvals or inputs unavailable (Q4, Q5, Q13, Q22, Q27, Q31, Q32, Q33, Q34) | Zero-mutation defaults (no certificate until Q5); the blocked check is recorded, never passed, and other work continues | Check blocked > 1 wave |
| P8 | Free disk drops (it moved 15 → 98 GiB within minutes) | Disk-guard tiers; lane C only at GREEN/BLUE; Code-OSS in CI | Tier below AMBER |
| P9 | RAM contention with llama.cpp | Two cargo slots, `nice`, never two Swift builds at once, cgroup caps for lab runs | OOM or swap |
| P10 | The Mac oracle is unreachable or busy (it also hosts the private track's VM) | Owner-approved windows; the `gha` backend on `xcode-27` runners once public, or per run with the owner's OK | `russet oracle status` fails |
| P11 | Private-repository CI limits (GitHub Free; decisions.md: free included minutes only) | No payment method, so jobs stop at the quota; Linux on `main` pushes, Windows weekly, macOS owner-dispatched only; draft PRs skip CI; cancel-in-progress; path filters; `cargo xtask ci-budget` warns at 80 %; the lead's local `--profile full` is the merge gate; Tier-2 IDE builds stay out of CI while private (W25); public at G1 (Q6, §5.9) | Usage above 80 % |
| P12 | Upstream churn (MCP, ACP, VSCodium, Flutter, iOS 27.x) | Pins, dual-era MCP tests, re-pin rules, traces with drift reports | Conformance or drift failures |
| P13 | DDI provenance and licence: no DDI source is licensed for Linux/Windows hosts (EA2002), and Apple's CDN has no standalone 27.x DDI | Import only user-obtained, Apple-origin copies; never re-host; the mirror stays opt-in; legal review Q26 (W26) | Counsel's advice, or Apple publishes a Device Support component for Xcode 27 |
| P14 | TXM policy drift between iOS releases (the 26.x and 27.x TXM builds differ; 27 adds a "platform code strict" path that can disable Developer Mode) | DC-30's negative control on each new iOS or beta; RDT traces per build; doctor reports Developer Mode before JIT runs | DC-30 fails on a new iOS |
| P15 | `xcode-27` is a preview runner label (no SLA; the GA label is unannounced) | The label lives in pins `[ci.github]` and a repository variable; `ship.ci.status` detects a missing label or Xcode (7010) | A runner-images announcement |
| P16 | Storyboard compiler scope (NetNewsWire-level UIKit coverage is 25–35 agent-days) | Phased B21a–c in three cards (W21b, W28b, W28c); acceptance apps chosen by IB need; semantic equality, not byte identity | B21c over budget |
| P17 | Private-track details become public with the repository (decisions.md, review files, git history) | Q34; `cargo xtask guard public-terms`; the §5.9 checklist, including a fresh history | Q6 answered "public" while Q34 is open |

---

## 14. Cleanup schedule (answers the relayed request "delete stuff we don't need if they are setup")

The relayed message is the owner's go-ahead for items that are set up and redundant (ARCHITECTURE §21 Q16). The lead does these deletions; implementation agents still delete nothing outside their own scratch output.

| When | Item | Size | Action |
|---|---|---|---|
| **Done** (re-checked 2026-10-11 with `ls`) | `~/orchard-lab/build`: the CPython 3.12.15 source and build tree | 361 MB | deleted |
| **Done** (2026-10-11, decisions.md) | xtool credential files in `~/.config/xtool/data/` | — | now 0600 |
| **Now**: rule 8 already points at uv's python-build-standalone 3.12.15 (`~/.local/share/uv/python/cpython-3.12.15-linux-x86_64-gnu`, verified with Swift LLDB); next, the lead points `demo/m1-baseline.sh`'s `PY312` default there | `~/.local/opt/python-3.12` | 297 MB | the lead deletes it |
| Keep until W16b's import and DC-29 pass | `~/russet-lab/ddi/27A266a/` (the Apple-origin DDI copy, 33.6 MB) and the `XcodeSystemResources.pkg` copy (146 MB) | 180 MB | then delete the `.pkg` copy; keep the DDI folder as the `dir` fallback |
| After W7 (RD-02) | `docs/design/run-debug-prototypes/` | 25 KB | W7 deletes it in its PR |
| After W12 (the reference signature is no longer needed) | `~/orchard-lab/Hello`, `~/orchard-lab/last-gdb.log` | 78 MB + 18 MB | delete (owner OK); keep the gdb log until W7's broker passes D3 |
| After W10's upstream PRs (DEV-23) | `~/russet-lab/{idevice,spike,out}` | ~40 MB | delete (owner OK) |
| After W12 | xtool's stored credentials | — | `xtool auth logout` (owner) |
| After W16b's import works (owner OK) | xtool's whole SDK bundle (its `Xcode.app` copy alone is 3.7 GB) | 6.8 GB | replace by a Russet import (about 0.45 GB, the iOS-only want-set of ARCHITECTURE §16.3) |
| Continuous | cargo target slots, `.russet/` scratch, lab outputs | ≤ 6 GiB per slot | `cargo xtask sweep`, lab cleanup step |
| When the lead removes a worktree | its `.russet-dev/` and its keyring entry `master-key-<hash8>` (named by `russet secrets status --json`) [D 83] | small | delete both |
| Keep while lane A works | `~/russet-lab/home-A` (lane A's persistent `RUSSET_HOME`) [D 82] | small | keep; it holds the DDI setting, accounts and the install ledger |
| Never by agents | non-Russet caches (`~/.cache/paru/clone` 17 GiB, `~/.cache/xodus-launcher`, `~/.cache/Shelly`, the `rust-docs` component) | — | owner's decision |

---

## 15. Traceability: every area task → workstream

| Area | Task → workstream |
|---|---|
| core | CORE-01 W1 · 02 W1/W2 · 03 W1/W2 · 04 W2 · 05 W2 (+ Windows tests W15) · 06 W2 · 07 W2 · 08 W3 · 09 W3 · 10 W3 · 11 W3 (python-shared, darwin-tools) / W26 (pins in CI) · 12 W3 · 13 W2 · 14 W13 · 15 W8 · 16 W26 · 17 W27a · 18 W27c · 19 W26 · 20 W27b (only if needed) · 21 W26 |
| device | DEV-01…09 W4 · 07b W10 · 10…12 W10 · 13 W4 (status and `dir` slice) / W10 · 14…16 W10 · 17/17b W11 · 18 W10 · 19 W4 (Wave-1 rows) / W10 · 20 W13 · 20b W30 · 21 W30 · 22 W27a · 23 W10 (incl. the cryptexd persistence fix) · 24 W10 · 25 W4/W10 |
| build | B01 W5 (+ OpenStep W16) · B02 W5 · B03 W5 · B04 W16 · B05 W6 (parse API) / W16 (lints) · B06 W5/W16 · B07 W5 · B08 W5/W16 · B09 W16 · B10 W21 · B11 W16 · B12 W21 · B13 W17 · B14 W24 · B15 W17 · B16 W28 · B17 W27b · B18 W23 · B19 W29 · B20 W29 · B21a W21b · B21b W28b · B21c W28c |
| signing | SA-01 W3 · 02…05 W6 · 06 W6 (read-only) / W12 · 07 W6/W12 · 08 W12 · 09 W19 · 10 W12 · 11…17 W23 · 18…21 W22 · 22 W27c · 23 W13/W20 · 24 W23 · 25 W26 |
| run/debug | RD-01…04 W7 · 05 W3 · 06 W7 · 07 W11 · 08 W14 · 09 W7/W11 · 10 W18 · 11 W18 · 12 W11 · 13 W11 · 14 W11 · 15 W18 · 16 W11 · 17 W30 · 18 W27a · 19 W11/W18 · 20 W18 · 21 W11/W18 |
| review round 1 | `sdk.import` and `russet-xar` W16b · Device Support fetch W33 (deferred) · SwiftPM XCTest W32 · Apple-hardware smoke W16 · DC-28 W4/W9 · DC-29 W10/W16b · DC-30 W18 · DC-31 W27b · DC-32 W21b · DC-33 W28b · DC-34 W32 |
| review round 2 | lane-A home and DC-28 script W9 · DDI root and mount records W4/W10 · isolation guards W2/W3/W8 (tripwire enabled by W9) · `russet debug --detach` and `SessionSummary` W7 · RussetProbe marker W5 · non-TTY imports W3/W6 · F0 completeness and module files W1 · Wave-1 pins W3 · fixture provenance and `guard public-terms` W8 · `apple-release.yml` W23 · `xtask lab` W9 · DC-35 W28c · going-public checklist (§5.9) the lead |
| IDE | SPK-01 W19 · SPK-02 W10 · SPK-03 W20 · SPK-04 W21 · IDE-01…09 W25 (IDE-05 also W27c) · EXT-01…08 W14 · EXT-09…11 W19 · EXT-12 W21 · EXT-13 W23 · EXT-14…16 W19 · EXT-17 W20 · MCP-01 W13 · MCP-02/03 W20 · MCP-04 W13/W20 · AGT-01…04 W20 · REL-01 W25/W26 |
| QA | QA-01 W1 · 02…04 W8 · 05 W2/W8 · 06…10 W15 · 11/12 W16 · 13 W9/W10 · 14 W26 · 15 W14/W15 · 16 W15 · 17 W10 · 18/19/21 W26 · 20 W25 · 22 W15 · 23 W26 · 24 W8 |
