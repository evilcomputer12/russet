# Device-check queue (lane A)

The queue of device checks (PLAN §2, §11). Any workstream appends a check; the current lane-A workstream runs the
queue in batches (at least once per working day while a wave is active) and records the redacted results. Owner:
W4 in Wave 1, then W9 (PLAN §3), which may move the result notes below into `docs/lab/results/<date>-DC-nn.md`.

**Filing a check:** id `DC-nn` (PLAN §11), requester, the exact commands or script, the expected result, the commit
to test. **Results** never contain UDIDs, ECIDs, serial numbers, team IDs, Apple IDs, device names or the issuer
prefix of xtool bundle ids (`XTL-<issuer8>` stands for it).

Lane-A environment for every check below:

```sh
export RUSSET_LANE=A RUSSET_HOME="$HOME/russet-lab/home-A"   # [device.ddi] dir = ~/russet-lab/ddi/27A266a/iOS
export CARGO_TARGET_DIR="$HOME/.cache/russet-dev/target-b" CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0
# Until W2's daemon and CLI merge, `devlab` (crates/russet-device/examples/devlab.rs) drives the same device code:
cargo build -p russet-device --example devlab && L="$CARGO_TARGET_DIR/debug/examples/devlab"
```

| DC | Requester | Status | Commit |
|---|---|---|---|
| DC-01 | W4 | passed (W4, 2026-10-11) | `ws/w04-device-plane` |
| DC-04 D1, D2 | W7 | passed (W4, 2026-10-11) | `ws/w04-device-plane` |
| DC-04 D3 | W7 | queued for W9 | W7's broker |
| DC-28 | W4 | queued for W9 (owner-approved reboot, Q33) | `ws/w04-device-plane` + W9's `tests/lab/dc28-ddi.sh` |

---

## DC-01: device HIL budgets for the Wave-1 capabilities (W4)

- **Check** (device.md §15.4; PLAN §4.4 tests): `crates/russet-device/tests/hil.rs`, serially on the rig:

  ```sh
  RUSSET_HIL_UDID=only RUSSET_HIL_BUNDLE="$($L apps | awk '/orchard.hello/{print $1}')" \
  RUSSET_HIL_APP="$HOME/orchard-lab/Hello/xtool/Hello.app" \
    cargo test -p russet-device --test hil -- --test-threads=1 --nocapture
  ```

- **Expected:** 100 session opens (tunnel + RSD) with 0 failures, p95 < 300 ms; 32 MiB AFC through the tunnel at
  ≥ 20 MiB/s both ways, byte-identical; `.app` and `.ipa` installs; a suspended launch returns a pid and SIGCONT
  resumes it; a console line arrives; `app.exited` ≤ 2 s after a kill; os_trace entries from the first
  instruction (launch suspended → subscribe → SIGCONT); `device.list` < 300 ms with the identity read.
- **Result (2026-10-11, W4; iPhone 12 iPhone13,2, iOS 27.0 24A437):** all seven tests passed.
  - Session opens: 100/100, median 79 ms, p95 88 ms. Without a close grace the device refused back-to-back
    CoreDeviceProxy tunnels after about two dozen cycles (`early eof`) and accepted them again after a pause; a
    200 ms grace before stopping the jktcp adapter, plus an open backoff, fixed it (russet-device `tunnel.rs`).
  - AFC through the tunnel (`com.apple.afc.shim.remote`, MSS 8132), 32 MiB: up 24.6–27.5 MiB/s, down 22.7–23.0
    MiB/s, identical.
  - `device.list`: `iPhone13,2` / `27.0`, `codeSigningMonitor: txm`, 29–36 ms cold (manager start, mux list and
    the lockdown identity read included).
  - Installs of the xtool-signed Hello: `.app` 1.15–1.5 s, `.ipa` 1.16–1.29 s (installd took 7.1 s once, for the
    first `.app` install over the M1 install).
  - Suspended launch: pid in 140–283 ms; while suspended the console is silent; after SIGCONT 1542–1552 os_trace
    entries (first 62–81 ms after SIGCONT) and, with `DYLD_PRINT_LIBRARIES=1`, 3154 console lines.
    `OS_ACTIVITY_DT_MODE=YES` (or `1`, `enable`) does **not** mirror os_log to the console on iOS 27.0.
  - Exit after SIGKILL: seen through the console's end in 18–42 ms.
  - Tunnel rebuild: after `tunnel.kill` (the TCP stack stops while the tunnel stays in its slot, as when the CDTunnel
    stream dies) the next RSD connect rebuilt the tunnel, generation 1 → 2, in 281 ms.
  - DDI status: mounted, `cryptex1`, version `27.1.5228.8`, `productBuild 27A5228h`, `source: unknown` (the spike's
    mirror DDI; Russet has no mount record for it): W4's acceptance item. `device.prepare` on this ready phone is a
    no-op in 182 ms (nothing mounted, no mount record written; DEV-13 asks for < 1 s).
  - DVT fallback launch (`--dvt`): pid in 451 ms, no console; the exit poll saw a SIGKILL after 695 ms.
  - Uninstall of the test app, a second uninstall (`app.notInstalled`, 3100, with its hint) and a reinstall.
  - The M1 LLDB sequence over Russet's own `com.apple.internal.dt.remote.debugproxy` forward (0600 Unix socket) and
    Russet's suspended CoreDevice launch stopped at `ContentView.swift:6:31` in
    `closure #1 () in ContentView.body.getter`, with no pymobiledevice3 on `PATH` (`devlab debug-session` plus the
    `demo/m1-baseline.sh` LLDB script with the forward swapped in).

## DC-04: CoreDevice launch shape for the debug plane (W7)

- **D1, CoreDevice `startStopped` + stdio shape.** `$L stdio-shape <bundle>`, `$L launch <bundle> --suspended …`,
  `$L debug-session <bundle>` + LLDB.
  - **Result (2026-10-11, W4):** `launchapplication` with `startStopped: true` returns `processToken
    {processIdentifier, executableURL.relative}` (a `file://` URL of the app executable) in 140–283 ms (3.8 s on the
    first launch after an install, device.md F10). The process is stopped before any user code: LLDB's attach reports
    `stop reason = signal SIGSTOP` in `dyld\`_dyld_start`. The `openstdiosocket` stream starts with the 16-byte UUID
    (version 4) that goes into `standardIOIdentifiers` for stdin, stdout and stderr alike; it stays silent while the
    app is suspended; after SIGCONT it carries the app's pseudo-terminal output with CRLF line ends (0 bare LF), stdout
    and stderr merged. After SIGKILL the stream ends at once (jktcp reports `BrokenPipe`, not a clean EOF).
- **D2, behaviour when the launch channel closes.** `$L launch <bundle> --suspended --close-channel-after-ms 500
  --sigcont-after-ms 3000 --wait-secs 6 --env=DYLD_PRINT_LIBRARIES=1`, and the same without `--suspended`.
  - **Result (2026-10-11, W4):** closing the `com.apple.coredevice.appservice` connection that launched the app
    neither kills nor resumes it. A suspended app stays suspended (no output) until SIGCONT, which then works from a
    new appservice connection; the stdio stream keeps working after the launch channel closed (2035 console lines
    after the SIGCONT). A running app keeps running. So a broker may attach after the launch channel closed, and the
    daemon does not need to keep the channel open for the app's sake (it keeps it anyway, ARCHITECTURE §17.1).
- **D3, broker sync `process connect` stops at a breakpoint:** queued for W9 (needs W7's broker).

## DC-28: DDI provenance and reboot (W4; run in W9)

- **Requester:** W4 (PLAN §4.4, §11; ARCHITECTURE [D 62, D 82, D 89]). **Runs in:** W9, after the owner-approved
  reboot (decisions.md Q33: approved for W9).
- **Preconditions:** the Apple-origin 27A266a copy at `~/russet-lab/ddi/27A266a/iOS` (`Restore/` + `version.plist`;
  done by the lead 2026-10-11); `[device.ddi] dir` set in lane A's home (done by W4 2026-10-11); no pymobiledevice3
  on `PATH`.
- **Commands:** W9's `tests/lab/dc28-ddi.sh` (PLAN §4.10 sketch): after the reboot, `russet device ddi status
  --json` (record whether the mirror's 27A5228h cryptex survived), `russet device prepare --json` (mounts from
  `[device.ddi] dir` when nothing is mounted), and only when another DDI is still mounted and
  `RUSSET_DC28_FORCE=owner-approved`: `russet device ddi mount --dir "$HOME/russet-lab/ddi/27A266a/iOS" --force
  --json`; then `demo/m1-russet.sh`. Until W2's CLI lands, `$L ddi-status`, `$L readiness` and
  `$L ddi-mount ~/russet-lab/ddi/27A266a/iOS [--force]` call the same code.
- **Expected:** `russet device ddi status --json` reports `mounted: true`, `kind: cryptex1`, `productBuild:
  27A266a`, `source: dir` (from Russet's mount record `state/devices/<udid>/ddi.json`), and the RussetProbe
  breakpoint hits. Exit 3 = blocked on the owner (another DDI mounted and no approval for `--force`). If cryptexd
  refuses a forced replacement, its answer (`device.ddiPersonalizationFailed` with the cryptexd domain and code in
  `details`) is recorded and the lead and the owner decide gate item 7.
- **What W4 verified without the reboot:** the mount path is idevice's cryptexd with Xcode's `persistence 2` and
  `nonce-persistence 1` (unit test over the encoded XPC request), payloads from the cryptex build identity of
  `<dir>/Restore/BuildManifest.plist` (synthetic-root test), and the mount record's attribution rules. W4 did not
  mount or replace the DDI on the rig (no reboot, no `--force`).
