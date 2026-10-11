# russet-device

idevice integration: mux, pairing, tunnel, RSD, DDI, apps, launch, forwards, logs, screen, files, crashes, perf.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W4 (Wave 1), W10 (Wave 2), W30 (`src/{wifi,a11y}/`). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- idevice `=0.1.68` from crates.io with `ring` and the feature list of device.md §5.1 (declared in the root `[workspace.dependencies]`); the reviewed `[patch.crates-io]` fork rev is a lead edit once W4's fork commit exists [D 22].
- Pre-created module files: `src/wifi/mod.rs` and `src/a11y/mod.rs` (W30).
