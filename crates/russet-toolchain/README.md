# russet-toolchain

Pin manifest, probes, resolver with the env contract, installer, GC, SDK registry probe.

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W3 (CORE-10, CORE-11 subset); W26 (pins in CI). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- `reqwest` uses `rustls-no-provider`; binaries install the ring provider (ARCHITECTURE §16.6).
