# Device-check queue (lane A)

The queue of device checks (PLAN §2, §11). Any workstream appends a check; the current lane-A workstream runs the
queue in batches (at least once per working day while a wave is active) and records the redacted results. Owner:
W4 in Wave 1, then W9 (PLAN §3). Results from W9 on are in `docs/lab/results/<date>-DC-nn.md`.

**Filing a check:** id `DC-nn` (PLAN §11), requester, the exact commands or script, the expected result, the commit
to test. **Results** never contain UDIDs, ECIDs, serial numbers, team IDs, Apple IDs, device names or the issuer
prefix of xtool bundle ids (`XTL-<issuer8>` stands for it).

Lane-A environment for every check below:

```sh
export RUSSET_LANE=A RUSSET_HOME="$HOME/russet-lab/home-A"   # [device.ddi] dir = ~/russet-lab/ddi/27A266a/iOS
export CARGO_TARGET_DIR="$HOME/.cache/russet-dev/target-b" CARGO_BUILD_JOBS=6 CARGO_INCREMENTAL=0
# Until W2's daemon and CLI merge, `devlab` (crates/russet-device/examples/devlab.rs) drives the same device code.
# devlab and the HIL tests take the host-wide live-device lock (ARCHITECTURE §3.3 item 7): one at a time, and
# never while a live daemon runs (they answer 3019 with the holder's pid).
cargo build -p russet-device --example devlab && L="$CARGO_TARGET_DIR/debug/examples/devlab"
```

| DC | Requester | Status | Commit tested |
|---|---|---|---|
| DC-01 | W4 | passed (W4, 2026-10-11, second run: 8/8 HIL tests) | `e3e7e71` |
| DC-01 open items | W4 | queued: need the owner, root, or W5/W6 artifacts (below) | the then-current W4/W9 commit |
| DC-04 D1, D2 | W7 | passed (W4, 2026-10-11) | recorded in `0534d24`; run on W4's working tree before its first commit |
| DC-04 D3 | W7 | passed (W9, 2026-10-11): [results](results/2026-10-11-DC-04.md) | `3ad83bd` |
| DC-28 | W4 | passed (W9, 2026-10-11, after the approved reboot and a forced replacement): [results](results/2026-10-11-DC-28.md) | `a5ea275` |
| DC-02 | W5 | passed (W9, 2026-10-11): [results](results/2026-10-11-DC-02.md) | `3ad83bd` |
| DC-03 | W6 | passed (W9, 2026-10-11): [results](results/2026-10-11-DC-03.md) | `3ad83bd` |
| DC-05 | W9 | passed 3/3 (W9, 2026-10-11): [results](results/2026-10-11-DC-05.md) | `3ad83bd` |

---

## DC-01: device HIL budgets for the Wave-1 capabilities (W4)

- **Check** (device.md §15.4; PLAN §4.4 tests): `crates/russet-device/tests/hil.rs`, serially on the rig. The tests
  are `#[ignore]`d (lane A only), so they need `--ignored`:

  ```sh
  RUSSET_HIL_UDID=only RUSSET_HIL_BUNDLE="$($L apps | awk '/orchard.hello/{print $1}')" \
  RUSSET_HIL_APP="$HOME/orchard-lab/Hello/xtool/Hello.app" \
    cargo test -p russet-device --test hil -- --ignored --test-threads=1 --nocapture
  ```

- **Expected:** 100 session opens (tunnel + RSD) with 0 failures, 0 device refusals and p95 < 300 ms at a pace the
  device sustains; 40 back-to-back opens with 0 refusals (paced); 32 MiB AFC through the tunnel at ≥ 20 MiB/s both
  ways, byte-identical; `.app` and `.ipa` installs; a suspended launch returns a pid and SIGCONT resumes it; a
  console line arrives; `app.exited` ≤ 2 s after a kill; os_trace entries from the first instruction (launch
  suspended → subscribe → SIGCONT); `device.list` < 300 ms with the identity read; a dead tunnel is rebuilt on the
  next call.
- **Result (2026-10-11, W4, commit `e3e7e71`; iPhone 12 iPhone13,2, iOS 27.0 24A437): 8/8 passed.**
  - Session opens: 100/100, median 81 ms, p95 86 ms, **0 device refusals, 0 paced opens**.
  - Back-to-back (no pause), 40 opens: 0 refusals; 39 opens waited for the pace (median 298 ms).
  - AFC through the tunnel (`com.apple.afc.shim.remote`, MSS 8132), 32 MiB: up 25.2 MiB/s, down 22.5 MiB/s,
    identical (an earlier run of the same commit's predecessor measured up 19.2 MiB/s once: AFC through the tunnel
    varies between about 19 and 27 MiB/s up).
  - `device.list`: `iPhone13,2` / `27.0` in 20 ms cold (manager start, mux list, identity read).
  - Installs of the xtool-signed Hello: `.app` 1.44 s, `.ipa` 1.19 s.
  - Suspended launch → subscribe → SIGCONT: 1542 os_trace entries and, with `DYLD_PRINT_LIBRARIES=1`, 3154 console
    lines; exit after SIGKILL seen through the console's end in 17 ms.
  - Tunnel rebuild after `tunnel.kill`: generation 1 → 2 in 523 ms (it includes the 500 ms pace, because the killed
    tunnel had just been opened).
  - DDI status: mounted, `cryptex1`, `27.1.5228.8`, `productBuild 27A5228h`, `source: unknown` (unchanged).
- **The device's tunnel limit (new finding, corrects the first run's note).** Counting the opens the connector
  retried showed that the first run's "0 failures" hid device refusals: back-to-back open/close cycles (280 ms each)
  were refused (`early eof`) at every 24th open, 4-5 times in a row (2.5-5 s), also with the 200 ms close grace; the
  first run's claim that the grace fixed it was wrong (the backoff hid it). Measured with `$L bench-sessions N
  [--gap-ms G]`, which prints the refused opens:

  | Close grace | Pause after close | Cycle | Refusals |
  |---|---|---|---|
  | 200 ms | 0 | ~280 ms | 20 in 100 opens (#24, #48, #72, #96: 5 retries each), twice |
  | 200 ms | 50 ms | ~330 ms | 20 in 100 (5 retries each, every 24th) |
  | 200 ms | 100 ms | ~380 ms | 16 in 100 (4 retries each, every 24th) |
  | 200 ms | 150 ms / 300 ms / 220 ms | ≥ 430 ms | 0 in 60 each |
  | 0 | 350 ms | ~430 ms | 0 in 60 |
  | 0 | 0 | ~80 ms | 20 in 60, and 2 opens failed after the 4.65 s backoff |

  So the device keeps a closed CoreDeviceProxy tunnel for about 10 s and refuses new ones while about two dozen
  linger. Since `e3e7e71` the live connector starts at most one tunnel per device per 500 ms (60 back-to-back
  cycles: 0 refusals) and the open backoff covers a whole linger (9.45 s).
- **Also on the rig (2026-10-11, `e3e7e71`):**
  - A log stream by bundle id follows a relaunch (`$L follow-relaunch <bundle>`): launch 1 and launch 2 each
    delivered 1536 os_trace entries and 3154 console lines into the same stream, 0 entries from other processes.
  - The M1 LLDB sequence over Russet's `com.apple.internal.dt.remote.debugproxy` forward and its suspended
    CoreDevice launch, with no pymobiledevice3 on LLDB's `PATH`, stopped at
    `closure #1() in ContentView.body.getter at ContentView.swift:6:31` twice: over the default 0600 Unix socket
    and over `--bind tcp:127.0.0.1:0` (accept-once, device.md §14). (`process kill` then reports "failed to send k
    packet": the forward ends with LLDB's one connection; the app was gone afterwards.)
  - Afterwards: 14 user apps, the same set as before (only the Hello test app reinstalled), no Hello process, no
    socket left in the lane-A runtime directory, DDI unchanged, no reboot.
  - cryptexd `copy-installed`, recorded raw (`$L cryptexd-installed`): one entry, `com.apple.MobileAsset.DDI`
    `27.1.5228.8`, no personal data; it is the fixture of russet-device's DDI status test.
- **First run (2026-10-11, W4's working tree before its first commit):** all seven tests passed (session opens:
  median 79 ms, p95 88 ms, with the refusals hidden as above; AFC up 24.6-27.5 MiB/s, down 22.7-23.0 MiB/s; installs
  `.app` 1.15-1.5 s, `.ipa` 1.16-1.29 s; suspended launch pid in 140-283 ms). Also recorded then: `OS_ACTIVITY_DT_MODE`
  (`YES`, `1`, `enable`) does **not** mirror os_log to the console on iOS 27.0; idle close (`DEVLAB_KEEP_WARM_MS=1000
  $L idle-close`) keeps a tunnel with a live stream up and closes it once the stream closed; `device.prepare` on the
  ready phone is a no-op in 182 ms; the DVT fallback launch returns a pid in 451 ms and its exit poll saw a SIGKILL
  after 695 ms; uninstall, a second uninstall (`app.notInstalled`, 3100, with its hint) and a reinstall.

### DC-01 open items (queued; lane A with the named prerequisite)

| Item | Needs | Commands | Expected |
|---|---|---|---|
| 100 MB IPA install time (DEV-06, device.md §15.4) | a signed IPA of about 100 MB from W5/W6 (RussetProbe with a padding resource) | `$L install <probe-100mb.ipa>`, then `$L uninstall dev.russet.test.probe` | installs; < 15 s, time recorded |
| A symlink inside the bundle survives installation (DEV-06) | W6's signer: RussetProbe with a framework reached through a symlink | `$L install …`, `$L launch dev.russet.test.probe --wait-secs 3` | installs and runs (the `dlopen` through the link works) |
| usbmuxd restart re-lists the device within 5 s (DEV-02) | root: the owner runs `sudo systemctl restart usbmuxd` | `$L watch --secs 60` (prints `removed` and `added` with timestamps) while the owner restarts usbmuxd | `added` ≤ 5 s after `removed` plus usbmuxd's own restart time (the fake-mux test: 1.75 s for a 1.5 s restart) |
| First-time pairing (DEV-05) | the owner: an unpaired iPhone (or `idevicepair unpair` with approval), tap Trust | W2's `russet device pair --json` (or devlab after W2) | `paired`; `device.changed` shows `trust-pending` while the dialog is up |
| Developer Mode off (DEV-05, DEV-25) | the owner turns it off (needs a reboot) | `russet doctor --json`, `russet device readiness --json` | `device.developerMode` fails with its hint; readiness `developerMode: no` |
| Locked-phone launch (DEV-07) | the owner locks the phone (screen off) | `$L launch <bundle>` | `device.locked` (3005) with its hint; `device.lockHint` warns; readiness `locked: yes` |

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
- **D3, broker sync `process connect` stops at a breakpoint:** passed in W9 (2026-10-11); see
  [results/2026-10-11-DC-04.md](results/2026-10-11-DC-04.md).

## DC-28: DDI provenance and reboot (W4; run in W9)

**Passed in W9 on 2026-10-11**: [results/2026-10-11-DC-28.md](results/2026-10-11-DC-28.md). The section below is the
request as W4 filed it.

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
  `nonce-persistence 1` (unit test over the encoded XPC request, and since `ea2e299` a test of the install's call
  site that records what it sends: the request, the payload sizes and transfers 1-5 in order), payloads from the
  cryptex build identity of
  `<dir>/Restore/BuildManifest.plist` (synthetic-root test), and the mount record's attribution rules. The dry run
  `$L ddi-check ~/russet-lab/ddi/27A266a/iOS` (2026-10-11) loaded the Apple-origin copy's cryptex payloads and got
  a 2977-byte Cryptex1 ticket from Apple's TSS for the rig in 2.6 s (personalization identifiers and the current
  nonce read, not rolled; nothing installed): the 27A266a DDI fits this device. The identity's `Cryptex1,Version`
  is a placeholder (`39.999.999.0.0,0`); the mount record takes the version cryptexd reports after the install.
  W4 did not mount or replace the DDI on the rig (no reboot, no `--force`); 27A5228h is still mounted.
