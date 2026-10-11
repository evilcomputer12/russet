# russet-doctor

Check trait, registry, report model, host checks.

- Layer **L2** (may depend on L0–L1) (ARCHITECTURE §5.1–§5.2). Licence: GPL-3.0-or-later.
- **Owner:** W3 (CORE-12). W1 created the crate as a compiling stub (freeze F0, PLAN §12).

## What it does

- `Check` (with `fix`, `fix_kind` and `diagnose`, which returns the result with the fix for its cause),
  `CheckResult` constructors, `Registry::run` (filters by id, id prefix and category; concurrent runs reported in
  registration order; network checks only with `online`; a 60 s timeout per check; `--fix` calls `fix` for a
  failure or warning whose fix is `Auto`, or undeclared as in F0, re-runs the check and reports the applied fix),
  `DoctorReport::summary` and `exit_code` (0, or 4 on a failure; `--strict` counts warnings).
- `CheckCx` keeps F0's fields and derives and adds Russet's directories, the daemon's device backend, the usbmux
  address, `storage.min-free-space`, and typed `extensions` through which russetd hands checks its services. Build
  it with `CheckCx { …, ..CheckCx::default() }`.
- Host checks: `host.os`, `host.disk`, `host.memory`, `host.wsl`, `dirs.private` (auto fix: create 0700) and
  `usbmux.available`, which connects only for the `live` backend and otherwise stats the socket and reports
  `skip` [D 83].
