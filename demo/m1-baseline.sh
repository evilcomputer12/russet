#!/usr/bin/env bash
# M1 baseline (2026-10-11), built from off-the-shelf tools before Russet's own code
# existed. It launches an xtool-installed app suspended on an iOS 17+ device over
# pymobiledevice3's no-root userspace tunnel, attaches Swift's LLDB through the
# debugserver proxy, and stops at a breakpoint.
#
# Verified on an iPhone 12 running iOS 27.0, with Swift 6.4 and xtool 1.21.0 on
# CachyOS. Run `xtool dev run --usb` in the app directory first.
#
# Env: BUNDLE (installed bundle id), LOCAL_EXE (local unsigned copy of the app
# executable, used for symbols), BREAK (file:line), PORT, LLDB, PY312 (prefix
# of a Python 3.12 with a shared libpython; Swift's Ubuntu-built lldb needs
# libpython3.12.so.1.0).
set -u
PORT=${PORT:-41240}
BUNDLE=${BUNDLE:?set BUNDLE to the installed bundle id}
LOCAL_EXE=${LOCAL_EXE:?set LOCAL_EXE to the local app executable}
BREAK=${BREAK:-ContentView.swift:6}
PMD3=${PMD3:-pymobiledevice3}
PY312=${PY312:-$HOME/.local/opt/python-3.12}
LLDB=${LLDB:-/usr/lib/swift/usr/bin/lldb}
WORK=$(mktemp -d)

REMOTE_APP=$(timeout 90 "$PMD3" apps list --type User 2>/dev/null | python3 -I -c '
import sys, json
raw = sys.stdin.read(); d = json.loads(raw[raw.find("{"):])
a = d[sys.argv[1]]; print(a["Path"] + "/" + a["CFBundleExecutable"])' "$BUNDLE") || { echo "app $BUNDLE not installed"; exit 1; }
echo "remote executable: $REMOTE_APP"

# The forwarder must run with explicit --userspace --local-port: with the
# automatic userspace fallback it drops forwarded connections.
"$PMD3" developer debugserver start-server --userspace --local-port "$PORT" >"$WORK/fwd.log" 2>&1 &
FWD=$!
trap 'kill $FWD 2>/dev/null; rm -rf "$WORK"' EXIT
for _ in $(seq 120); do grep -q "ACTUAL CONNECTION" "$WORK/fwd.log" && break; sleep 1; done

# On iOS 17+, debugserver may not spawn apps ("Operation not permitted"), so
# launch suspended via DVT and attach.
APP_PID=$(timeout 120 "$PMD3" developer dvt launch --suspended --userspace "$BUNDLE" 2>"$WORK/launch.err" | grep -oE "[0-9]+" | tail -1)
echo "launched suspended, pid=$APP_PID"; [ -n "$APP_PID" ] || { cat "$WORK/launch.err"; exit 1; }

# lldb -b runs synchronously, and a sync `process connect` to a process-less
# debugserver blocks forever, so connect in async mode and switch back.
cat >"$WORK/attach.lldb" <<LLDB
settings set target.memory-module-load-level minimal
settings set plugin.process.gdb-remote.packet-timeout 15
platform select remote-ios
target create "$LOCAL_EXE"
script lldb.target.module[0].SetPlatformFileSpec(lldb.SBFileSpec("$REMOTE_APP"))
script lldb.debugger.SetAsync(True)
process connect connect://127.0.0.1:$PORT
script import time; time.sleep(2)
script lldb.debugger.SetAsync(False)
breakpoint set --file ${BREAK%%:*} --line ${BREAK##*:}
process attach --pid $APP_PID
process continue
thread backtrace -c 6
frame variable
process kill
LLDB
LD_LIBRARY_PATH="$PY312/lib" PYTHONHOME="$PY312" timeout 120 "$LLDB" -b -s "$WORK/attach.lldb" 2>&1
