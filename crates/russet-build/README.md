# russet-build

Build orchestrator: project detection, stack adapters, Info.plist synthesis, bundle assembly, lints, metadata policy.

- Layer **L3** (domain crate; may depend on F and L0–L2 except russet-ipc, and on the L3 edges of ARCHITECTURE §5.2) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W5 (Wave 1), W16 (Wave 2); stack and resource modules per PLAN §3. W1 created the crate as a compiling stub (freeze F0, PLAN §12).
- Pre-created module files: `src/stack/{flutter,xcode,rn,maui}/mod.rs` (W17, W28, W29, W29), `src/stack/flutter/aot.rs` (W24), `src/resources/mod.rs` (W21), `src/platform/windows/mod.rs` (W27b).
