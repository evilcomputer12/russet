<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

# Testing without a phone

Only the device lane (lane A) talks to the iPhone (PLAN §0 rule 4). Everyone else develops and tests against the
testkit (`crates/russet-testkit`, CORE-15); its README lists every type.

## The lane-D environment

ARCHITECTURE §3.3 item 7 gives every lane-D worktree its own home and the scripted device backend:

```sh
export RUSSET_HOME="$PWD/.russet-dev"                 # one daemon, socket, lock and keyring entry per worktree
export RUSSET_DEVICE_BACKEND="scripted:$PWD/crates/russet-testkit/scenarios/default.toml"
```

`scenarios/default.toml` is one synthetic iPhone 12 (`iPhone13,2`) on iOS 27.0 (24A437): paired, Developer Mode on,
unlocked, the Xcode 27.0 DDI mounted from a directory, and RussetProbe's 100-line heartbeat on the syslog relay.
Write your own scenario for anything else (attach and detach timelines, facade answers, scripted service streams,
failures); the format is in the testkit README and `src/scenario.rs`.

**Until the daemon can build the scripted connector,** keep `RUSSET_DEVICE_BACKEND=none` in your shell instead, as
`AGENTS.md` §4 and `.envrc.example` say. The connector lives in the testkit, which russetd may not depend on
(ARCHITECTURE §5.2); W8 asked the lead for a contract change that moves it where the daemon can use it. Until that
lands, a daemon started with `scripted:` starts and reports the backend, but answers device methods with an error.
Device-plane tests drive `ScriptedConnector` in process instead (below).

## In tests

| Need | Use |
|---|---|
| A daemon home | `TempHome::new()` (or `TempHome::builder()…build()`): a fresh `RUSSET_HOME` with memory secrets, a private `RUSSET_LIVE_DEVICE_LOCK`, `RUSSET_OFFLINE=1` and the scripted backend. `home.apply(&mut cmd)` for child processes; `home.dirs()` and `home.socket_path()` for in-process daemons. Never set these variables on the test process itself. A daemon-level test that asserts the device plane is absent picks `Backend::None` |
| A device | `ScriptedConnector::new(Scenario::from_toml(…))` as an `Arc<dyn ServiceConnector>`, and its `Control`: `attach`, `detach`, `push_log`, `fail_next`, `ops`, `calls`, `assert_consumed` |
| A spawned daemon | `SpawnedDaemon::spawn(&home, &russet_bin()?, &[])`: waits for the socket, keeps `daemon.log`, kills the daemon on drop. `russet_bin()` runs `cargo build -p russet-cli --bin russet` from your worktree once per test process (`RUSSET_TEST_BIN` overrides it), so a shared target slot never hands the test another worktree's binary |
| Protocol exchanges | `Transcript` files (`*.transcript.jsonl`) replayed over the socket, with `$capture:x` / `${x}`, `$any`, `$absent`, `$type:…`, `$prefix:…`, `$contains` and ignorable notifications |
| A client test without the daemon | `fake_daemon::serve(stream)` or `FakeDaemon::start(dir)` |
| The VS Code client library | `node::run(&InteropPlan::full(socket, progress_method, …, confirm_method, …))` (vscode-jsonrpc 9.0.3; `npm ci --prefix crates/russet-testkit/node/interop` once) |
| Proof that nothing reached usbmuxd | `TempHome::builder().tripwire()` and `home.tripwire().unwrap().assert_untouched()` |

Tests of an L3 crate that use the testkit are integration tests under that crate's `tests/`, never `#[cfg(test)]`
modules (ARCHITECTURE §5.1). In the shared crates (`russetd`, `russet-cli`, `russet-protocol`, `russet-core`) each
area's tests go into `tests/<area>_*.rs`.

## The isolation test

`crates/russet-testkit/tests/isolation_tripwire.rs` starts a daemon with the scripted backend and a tripwire
`USBMUXD_SOCKET_ADDRESS`, lists and watches devices, and fails if anything connected to the tripwire (ARCHITECTURE
§3.3 item 7 [D 63, D 83]). It needs W4's `DeviceManager` behind W2's daemon, and a `DeviceSlot` that builds the scripted
connector, so it is `#[ignore]`d until W4 merges; W9 enables it (`russet_bin()` builds the binary first):

```sh
cargo test -p russet-testkit --test isolation_tripwire -- --ignored
```

It cannot see a direct open of `/run/usbmuxd`; W2's daemon tests cover that with
`russet_device::io::live_connectors_constructed() == 0`.

## Device checks

When something can only be verified on the phone, file a device check in `docs/lab/queue.md` (PLAN §11): id
`DC-nn`, requester, the exact commands, the expected result and the commit to test. Lane A runs the queue in batches
and records redacted results in `docs/lab/results/`.
