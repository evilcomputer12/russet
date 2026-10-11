# russet-adi-host

Sandboxed helper that loads Apple's runtime-downloaded Android ADI libraries; no network; JSON-lines IPC.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W22 (SA-19). W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Layer L3 binary that may depend only on F crates and `russet-core` (ARCHITECTURE §5.2).
