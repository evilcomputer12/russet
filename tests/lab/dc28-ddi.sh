#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Russet contributors
#
# DC-28 (PLAN §4.10, §11; gate item 7) [D 62, D 82, D 89]: after the owner-approved reboot (Q33), Russet mounts the
# Apple-origin 27A266a DDI from `[device.ddi] dir` with idevice alone (no pymobiledevice3 on PATH), the mount record
# says `source: dir`, and RussetProbe still launches and stops at its breakpoint (demo/m1-russet.sh).
#
# Lane A only. Run from anywhere; the reboot itself is not part of this script.
# Env: RUSSET_BIN (default: `russet` on PATH); RUSSET_DC28_OUT (records; default: a new temporary directory);
# RUSSET_DC28_WAIT (seconds to wait for the phone after a reboot, default 600);
# RUSSET_DC28_FORCE=owner-approved (replace another mounted DDI with `ddi mount --force`).
# Exit: 0 = passed; 1 = failed; 3 = blocked on the owner (another DDI is mounted and --force is not approved, or
# the phone did not come back unlocked within RUSSET_DC28_WAIT).
set -euo pipefail

[ "${RUSSET_LANE:-}" = A ] || { echo "dc28: lane A only (export RUSSET_LANE=A)" >&2; exit 1; }
: "${RUSSET_HOME:?export RUSSET_HOME=\$HOME/russet-lab/home-A}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUSSET_BIN="${RUSSET_BIN:-$(command -v russet || true)}"
[ -x "$RUSSET_BIN" ] || { echo "dc28: no russet binary (set RUSSET_BIN)" >&2; exit 1; }
RUSSET_BIN="$(readlink -f "$RUSSET_BIN")"
export RUSSET_BIN
OUT="${RUSSET_DC28_OUT:-$(mktemp -d "${TMPDIR:-/tmp}/russet-dc28.XXXXXX")}"
mkdir -p "$OUT"

PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -vx "$HOME/.local/bin" | paste -sd:)"
export PATH
if command -v pymobiledevice3 > /dev/null; then echo "dc28: pymobiledevice3 is still on PATH" >&2; exit 1; fi

russet() { "$RUSSET_BIN" "$@"; }
step() { printf '[dc28] %s\n' "$*" >&2; }

DDI_DIR="$HOME/russet-lab/ddi/27A266a/iOS"
[ -f "$DDI_DIR/Restore/BuildManifest.plist" ] || { echo "dc28: no Apple-origin DDI copy at $DDI_DIR" >&2; exit 1; }
russet daemon stop > /dev/null 2>&1 || true
russet config set --scope global device.ddi.dir "$DDI_DIR" > /dev/null

step "waiting for the phone (up to ${RUSSET_DC28_WAIT:-600} s)"
deadline=$(($(date +%s) + ${RUSSET_DC28_WAIT:-600}))
until russet device readiness --json > "$OUT/readiness.json" 2> "$OUT/readiness.err" \
  && jq -e '.paired == true and .tunnel.state == "up"' "$OUT/readiness.json" > /dev/null; do
  if [ "$(date +%s)" -ge "$deadline" ]; then
    step "BLOCKED: the phone is not ready (locked after the reboot?): $(jq -c '.error.message? // .' "$OUT/readiness.err" 2> /dev/null || true)"
    exit 3
  fi
  sleep 10
done

step "DDI status after the reboot"
russet device ddi status --json | jq 'del(.udid)' > "$OUT/dc28-before.json"
cat "$OUT/dc28-before.json" >&2

step "device prepare (mounts from [device.ddi] dir when nothing is mounted)"
russet device prepare --json | jq 'del(.udid)' > "$OUT/dc28-prepare.json" || true
russet device ddi status --json | jq 'del(.udid)' > "$OUT/dc28-after-prepare.json"
if ! jq -e '.productBuild == "27A266a"' "$OUT/dc28-after-prepare.json" > /dev/null; then
  if [ "${RUSSET_DC28_FORCE:-}" != owner-approved ]; then
    step "BLOCKED: another DDI is still mounted; --force needs the owner's OK (RUSSET_DC28_FORCE=owner-approved)"
    exit 3
  fi
  step "replacing the mounted DDI (owner-approved --force)"
  russet device ddi mount --dir "$DDI_DIR" --force --json > "$OUT/dc28-force.json" 2>&1 || true
  jq -c 'del(.udid?)' "$OUT/dc28-force.json" >&2 || cat "$OUT/dc28-force.json" >&2
fi

russet device ddi status --json | jq 'del(.udid)' > "$OUT/dc28-status.json"
cat "$OUT/dc28-status.json" >&2
jq -e '.mounted and .productBuild == "27A266a" and .source == "dir"' "$OUT/dc28-status.json" > /dev/null \
  || { step "FAIL: the 27A266a DDI is not mounted from dir"; exit 1; }

step "launch and debug RussetProbe on that DDI"
RUSSET_M1_OUT="$OUT/m1" "$REPO/demo/m1-russet.sh"
step "PASS ($OUT)"
