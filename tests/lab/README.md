# russet-lab

Device-in-the-loop harness and suites (L7).

- Layer **dev** (tests and tooling; never a normal dependency of a shipped crate) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W9 (lane A only). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Lane A only: device suites run through `cargo xtask lab` under the live-device lock (ARCHITECTURE §3.3 item 7).
