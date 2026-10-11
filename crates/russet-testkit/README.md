# russet-testkit

Scripted, replay and recording connectors, mock usbmuxd, cassettes, HTTP fakes, temp-home daemon fixture, RDT traces.

- Layer **dev** (tests and tooling; never a normal dependency of a shipped crate) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W8 (CORE-15, QA); `src/debug/` W7. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Tests of an L3 crate that use the testkit are integration tests under that crate's `tests/`, never `#[cfg(test)]` unit modules (ARCHITECTURE §5.1).
- Pre-created module file: `src/debug/mod.rs` (W7).

## What is here (Wave 1)

| Item | Use it for |
|---|---|
| `TempHome` (`src/home.rs`) | Every test that starts a daemon: a fresh `RUSSET_HOME` with `RUSSET_SECRETS_BACKEND=memory`, a private `RUSSET_LIVE_DEVICE_LOCK`, `RUSSET_OFFLINE=1` and the scripted device backend; `RUSSET_LANE` and the other leaking variables are removed. `home.apply(&mut cmd)` for children, `home.dirs()` / `home.socket_path()` for in-process daemons |
| `ScriptedConnector` + `Control` (`src/scripted.rs`) | Core's FakeDeviceBackend [D 46]: an `io::ServiceConnector` played from a scenario file, with attach/detach, facade calls, scripted service streams, syslog lines and failures, and a `Control` handle (attach, detach, push a log line, inject a failure, list the operations, check every scripted step ran) |
| `scenarios/default.toml` | The lane-D device: one synthetic iPhone 12 on iOS 27.0, ready, with the RussetProbe heartbeat. `export RUSSET_DEVICE_BACKEND="scripted:$PWD/crates/russet-testkit/scenarios/default.toml"` |
| `SpawnedDaemon` (`src/daemon.rs`) | `russet daemon run` in a `TempHome`: waits until the socket accepts, keeps the daemon's log, kills it on drop. `russet_bin()` finds the binary (`RUSSET_TEST_BIN`, else `target/<profile>/russet`) |
| `Tripwire` (`src/tripwire.rs`) | A `USBMUXD_SOCKET_ADDRESS` socket that records connections; `TempHome::builder().tripwire()` wires it in |
| `lsp`, `Transcript` (`src/lsp.rs`, `src/transcript.rs`) | russet/1 framing and JSON-RPC transcript replay with captures (`$capture:x`, `${x}`), wildcards (`$any`, `$absent`, `$type:…`, `$prefix:…`, `$contains`) and ignorable notifications. Example: `transcripts/fake-daemon.transcript.jsonl` |
| `fake_daemon` (`src/fake_daemon.rs`) | A tiny russet/1 peer (`initialize`, `daemon.ping`, `test.echo`, `test.slow` with progress and cancel, `test.confirm` with `ui.confirm`) to test clients and harnesses without the real daemon |
| `node` + `node/interop/` | The Node interop harness shared with W2: stock vscode-jsonrpc 9.0.3 runs `initialize`, requests, unknown method → `-32601`, `$/progress` then cancel → `-32800`, and a `ui.confirm` answered by the client. Install once: `npm ci --prefix crates/russet-testkit/node/interop` |

Replay and recording connectors, the out-of-process mock device (fake usbmuxd, lockdown with TLS), cassettes, HTTP
fakes and RDT traces follow in W15 (QA-06 to QA-09).

## Writing a daemon-level test without hardware

```rust
// crates/russetd/tests/<area>_something.rs (the area's owner writes it; PLAN §0 rule 2)
use russet_testkit::{SpawnedDaemon, TempHome, Transcript};
use russet_testkit::daemon::russet_bin;

let home = TempHome::builder().tripwire().build()?;          // scripted default scenario, memory secrets
let daemon = SpawnedDaemon::spawn(&home, &russet_bin()?, &[])?;
let replay = Transcript::from_file(path)?.replay_unix(daemon.socket_path()).await?;
home.tripwire().unwrap().assert_untouched()?;               // lane D never talks to usbmuxd
```

In-process daemons (`russetd::InProcess::start`) take `DaemonOptions { dirs: home.dirs().clone(), endpoint:
Endpoint::Unix(home.socket_path()), .. }`; how they receive a `ScriptedConnector` is W2's `DeviceSlot`
(ARCHITECTURE §5.3).

## Scenario files

Schema 1, kebab-case keys; the full description is in `src/scenario.rs`. In short:

```toml
schema = 1
name = "example"

[[device]]                      # udid, name, product-type, os-version, build, transport, code-signing-monitor,
udid = "00000000-0000000000000001"   # paired, developer-mode, locked, attach-at-ms, detach-at-ms
detach-at-ms = 5000

[device.ddi]                    # mounted, kind, version, product-build, source
mounted = true

[device.lockdown]               # extra lockdown values for lockdown.get_value
HardwareModel = "D53gAP"

[[device.log]]                  # com.apple.syslog_relay lines ("\n\0" framing, idevice's SyslogRelayClient)
message = "PROBE seq={n}"
repeat = 100
every-ms = 10

[[call]]                        # ServiceConnector::call answers; args is a subset match; result or error
api = "installation_proxy.browse"
result = []

[[service]]                     # scripted plaintext streams: send, send-b64, expect, expect-b64, delay-ms, close
name = "com.example.echo"
via = "lockdown"                # or "rsd"
steps = [{ expect = "ping\n" }, { send = "pong\n" }]

[[failure]]                     # op: watch | call | lockdown-service | rsd-service
op = "rsd-service"
code = "device.tunnelFailed"    # a russetCode of ARCHITECTURE §13.3
times = 1
```

Synthetic UDIDs use chip id `0000` (no Apple chip has it), so the secrets guard never mistakes them for real ones.
Facade API names belong to the device plane (W4); the connector answers `lockdown.get_value {key?, domain?}` by
itself and everything else from `[[call]]` entries.
