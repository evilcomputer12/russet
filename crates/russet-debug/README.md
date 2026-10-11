# russet-debug

Sessions, launch strategies, broker, JIT servicer, LLDB plans, DAP hub, symbols, crashes, logs, Flutter run.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W7 (Wave 1), W11 (Wave 2), W18 (Wave 3); `src/wifi.rs` W30.
- Wave 1 (W7): `broker` (pre-attach, relay, detach/kill policy), `launch` (specs, default environment, launch
  channel), `lldb` (Python-free attach plans, batch scripts and transcript parsing), `logs` (the merged run
  output), `session` and `DebugService` (states, leases, persistence), `jit::touch_plan`, `errors`, and the
  `DeviceServices` seam over russet-device.
- Pre-created module file: `src/wifi.rs` (W30).
