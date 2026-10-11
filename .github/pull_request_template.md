<!--
SPDX-License-Identifier: GPL-3.0-or-later
SPDX-FileCopyrightText: 2026 Russet contributors
-->

## What and why

<!-- One paragraph; link the PLAN card (WNN) and the design task ids (CORE-…, DEV-…, B…, QA-…). -->

## How it was tested

<!-- Paste the summary of `cargo xtask ci --profile pr`; name device checks (DC-nn) filed for lane A. -->

## Checklist

- [ ] `cargo xtask ci --profile pr` is green (or the summary names what was skipped and why)
- [ ] Commits follow Conventional Commits; in the public phase each carries `Signed-off-by:` (DCO, CONTRIBUTING.md)
- [ ] Only my card's paths changed (`cargo xtask check-paths`); contract changes went to `docs/ARCHITECTURE.md` separately
- [ ] New errors have a code, a hint and a docs anchor; CLI commands support `--json`
- [ ] SPDX headers on new files; I included no Apple files, no device identifiers and no secrets
- [ ] The area design doc is updated where behaviour differs from it
