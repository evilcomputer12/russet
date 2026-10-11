#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 Russet contributors
#
# The Wave-1 exit script (PLAN §4.10, gate items 1-6; DC-02, DC-03, DC-05): `russet` builds, signs, installs,
# launches and debugs RussetProbe on the iPhone with Russet's own code only. xtool, pymobiledevice3 and go-ios
# (`$HOME/.local/bin`) are hidden from PATH, and the daemon is restarted so it inherits that PATH.
#
# Lane A only (AGENTS.md §4): RUSSET_LANE=A, RUSSET_HOME=$HOME/russet-lab/home-A. Needs the owner-local, gitignored
# tests/fixtures/apps/RussetProbe/russet.local.toml (PLAN §4.10) that maps RussetProbe onto the existing xtool
# profile with zero account changes.
#
# Env: RUSSET_BIN (default: `russet` on PATH), RUSSET_M1_OUT (where run.json, logs.ndjson, dbg.json and
# summary.json go; default: a new temporary directory, printed at the end).
# Exit: 0 = every gate item passed; 1 = a gate item failed (the failing step is printed).
set -euo pipefail

: "${RUSSET_LANE:?lane A only: export RUSSET_LANE=A}"
[ "$RUSSET_LANE" = A ] || { echo "m1-russet: lane A only (RUSSET_LANE=$RUSSET_LANE)" >&2; exit 1; }
: "${RUSSET_HOME:?export RUSSET_HOME=\$HOME/russet-lab/home-A}"

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUSSET_BIN="${RUSSET_BIN:-$(command -v russet || true)}"
[ -x "$RUSSET_BIN" ] || { echo "m1-russet: no russet binary (set RUSSET_BIN)" >&2; exit 1; }
RUSSET_BIN="$(readlink -f "$RUSSET_BIN")"
OUT="${RUSSET_M1_OUT:-$(mktemp -d "${TMPDIR:-/tmp}/russet-m1.XXXXXX")}"
mkdir -p "$OUT"

# Hide xtool, pymobiledevice3 and go-ios: they live in ~/.local/bin on the rig.
PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -vx "$HOME/.local/bin" | paste -sd:)"
export PATH
for tool in xtool pymobiledevice3 ios; do
  if command -v "$tool" > /dev/null; then echo "m1-russet: $tool is still on PATH" >&2; exit 1; fi
done

russet() { "$RUSSET_BIN" "$@"; }
step() { printf '[m1] %s\n' "$*" >&2; }
fail() { printf '[m1] FAIL: %s\n' "$*" >&2; exit 1; }
now_ms() { date +%s%3N; }

cd "$REPO/tests/fixtures/apps/RussetProbe"
[ -f russet.local.toml ] || fail "tests/fixtures/apps/RussetProbe/russet.local.toml is missing (PLAN §4.10)"

# Break on the statement after the code marker (the doc comment above it names the marker too) [D 84].
PROBE_LINE="$(grep -n '// RUSSET_PROBE_BREAK$' Sources/RussetProbe/ContentView.swift | head -1 | cut -d: -f1)"
[ -n "$PROBE_LINE" ] || fail "no RUSSET_PROBE_BREAK marker"
PROBE_LINE=$((PROBE_LINE + 1))

step "restarting the daemon without xtool/pymobiledevice3 on PATH"
russet daemon stop > /dev/null 2>&1 || true
russet config set --scope global fallback.pymobiledevice3 off > /dev/null
DDI_DIR="$HOME/russet-lab/ddi/27A266a/iOS"
if [ -d "$DDI_DIR/Restore" ]; then russet config set --scope global device.ddi.dir "$DDI_DIR" > /dev/null; fi

step "device prepare"
russet device prepare --json > "$OUT/prepare.json" || fail "device prepare"
jq -e '.ddi.mounted' "$OUT/prepare.json" > /dev/null || fail "no DDI mounted"

step "the daemon runs without the hidden tools"
DAEMON_PID="$(russet daemon status --json | jq -r .pid)"
tr '\0' '\n' < "/proc/$DAEMON_PID/environ" | grep '^PATH=' | grep -q "$HOME/.local/bin" \
  && fail "the daemon's PATH still has ~/.local/bin"

step "doctor (with the device selected)"
UDID="$(russet device list --json | jq -r '.devices[0].udid // empty')"   # never printed
[ -n "$UDID" ] || fail "no device"
russet --device "$UDID" doctor --json > "$OUT/doctor.json" || true
jq -e '.summary.fail == 0' "$OUT/doctor.json" > /dev/null || fail "doctor reports failures: $(jq -c '[.checks[] | select(.status == "fail") | .id]' "$OUT/doctor.json")"

step "account"
russet account list --json | jq -e '.accounts | length > 0' > /dev/null \
  || russet account import --from-xtool --yes --json > /dev/null || fail "account import"

step "build (item 1)"
russet build --json > "$OUT/build.json" || fail "build"
jq -e '.app.path' "$OUT/build.json" > /dev/null || fail "build has no app"
llvm-otool -l .russet/build/debug-iphoneos/RussetProbe.app/RussetProbe | grep -A4 LC_BUILD_VERSION | grep -q 'sdk 27.0' \
  || fail "the executable is not built with sdk 27.0"

step "run --detach (items 2-3)"
russet run --json --detach > "$OUT/run.json" || fail "run"
SESSION="$(jq -r .sessionId "$OUT/run.json")"
russet logs --session "$SESSION" --duration 15s --json > "$OUT/logs.ndjson" || fail "logs"
# The longest run of consecutive heartbeat sequence numbers.
HEARTBEATS="$(jq -r 'select(.message | test("^PROBE seq=")) | .message | capture("seq=(?<n>[0-9]+)").n' "$OUT/logs.ndjson" \
  | awk 'NR == 1 || $1 != prev + 1 { run = 0 } { run++; if (run > best) best = run; prev = $1 } END { print best + 0 }')"
[ "$HEARTBEATS" -ge 100 ] || fail "only $HEARTBEATS consecutive heartbeat lines"
russet stop --all --json > /dev/null

step "debug --batch --detach at ContentView.swift:$PROBE_LINE (items 4-5)"
T0="$(now_ms)"
russet debug --batch --detach --break "ContentView.swift:$PROBE_LINE" --command 'frame variable probeValue' --json \
  > "$OUT/dbg.json" || fail "debug: $(cat "$OUT/dbg.json")"
DEBUG_MS=$(($(now_ms) - T0))
jq -e --argjson l "$PROBE_LINE" '.stop.reason == "breakpoint" and .stop.frame.line == $l
  and (.stop.frame.function | test("ContentView.body.getter"))
  and (.commands[0].output | test("= 42")) and .detached' "$OUT/dbg.json" > /dev/null \
  || fail "debug result: $(jq -c '{stop, commands, detached}' "$OUT/dbg.json")"
[ "$DEBUG_MS" -le 60000 ] || fail "the breakpoint took ${DEBUG_MS} ms (limit 60 s)"

step "the app survives the detach (item 6)"
sleep 5
russet sessions --json > "$OUT/sessions.json"
jq -e '[.sessions[] | select(.state == "running")] | length == 1 and .[0].debugger == "detached"' "$OUT/sessions.json" \
  > /dev/null || fail "sessions: $(jq -c '[.sessions[] | {state, debugger, kind}]' "$OUT/sessions.json")"
russet stop --all --json > /dev/null

step "no account mutations"
russet audit tail --kinds account-mutation --json > "$OUT/audit.json"
jq -e '.entries | length == 0' "$OUT/audit.json" > /dev/null || fail "account mutations were recorded"

jq -n --argjson heartbeats "$HEARTBEATS" --argjson debugMs "$DEBUG_MS" --argjson line "$PROBE_LINE" \
  --slurpfile run "$OUT/run.json" --slurpfile dbg "$OUT/dbg.json" --slurpfile prep "$OUT/prepare.json" \
  --slurpfile doc "$OUT/doctor.json" \
  '{passed: true, heartbeats: $heartbeats, debugMs: $debugMs, breakLine: $line,
    installed: $run[0].installed, probeValue: $dbg[0].commands[0].output, function: $dbg[0].stop.frame.function,
    ddi: ($prep[0].ddi | {productBuild, source}), doctor: $doc[0].summary}' > "$OUT/summary.json"
step "PASS ($OUT)"
cat "$OUT/summary.json"
