# russet-device

idevice integration: mux, pairing, tunnel, RSD, DDI, apps, launch, forwards, logs, screen, files, crashes, perf.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W4 (Wave 1), W10 (Wave 2), W30 (`src/{wifi,a11y}/`). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- idevice `=0.1.68` from crates.io with `ring` and the feature list of device.md §5.1 (declared in the root `[workspace.dependencies]`).
- Pre-created module files: `src/wifi/mod.rs` and `src/a11y/mod.rs` (W30).

## Wave 1 (W4)

| Module | What |
|---|---|
| `mux` | Russet's usbmuxd client: list, `Listen` as a `Send` stream, `Connect`, pair records, BUID. A declared packet size below 16 or above 16 MiB is rejected before any allocation. `MuxProvider` implements idevice's `IdeviceProvider` on it |
| `tunnel` | CoreDeviceProxy with the jktcp MSS clamped to 8132 (device.md F1), the RSD map of one generation (F2), a close grace and an open backoff |
| `io` | The `ServiceConnector` seam, the facade-call catalog (`io::facade`) and `LiveConnector` (`io/live.rs`) |
| `manager`, `actor`, `model`, `config` | `DeviceManager` (one actor per UDID, lazy identity [D 63]), `DeviceHandle`, `DeviceConfig` (`[device]`, `[device.ddi]`, `[devices.aliases]`) |
| `pairing` | Identity, pairing, Developer Mode, readiness, `device.prepare` |
| `ddi` | DDI status (CopyDevices + cryptexd), mount records [D 89], the Cryptex1 mount from a DDI directory [D 82] with Xcode's persistence values |
| `apps`, `process`, `coredevice` | Install/uninstall/list; CoreDevice launch with a console, DVT fallback, signals, processes, exit detection |
| `debug`, `logs` | Forwards (RSD service, tunnel port, usbmux port; 0600 Unix socket or loopback TCP); os_trace/syslog/stdout logs, batching, the tail ring |
| `error`, `doctor`, `wire`, `rpc`, `ids` | Device error codes and hints, the NSError decoder, doctor checks, wire conversions, the method layer russetd calls, lease/subscription ids |

idevice 0.1.68 needs no patch for this crate: the usbmuxd size cap and the `Send` listen stream come from `mux`, and
the cryptexd `persistence` value from `ddi::cryptex_install_argv`. The reviewed fork commit for the root
`[patch.crates-io]` ([D 22]) exists locally (`~/russet-lab/idevice`, branch `russet/patch`); the lead decides on it.

**Lane A only** (PLAN §0 rule 4): `examples/devlab.rs` drives the live backend without a daemon, and `tests/hil.rs`
runs the hardware-in-the-loop suite when `RUSSET_HIL_UDID` and `RUSSET_LANE=A` are set (see `docs/lab/queue.md`
DC-01). Everything else is tested without a device (`tests/manager.rs` over an in-memory connector).
