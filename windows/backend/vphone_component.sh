#!/usr/bin/env bash
# Russet-managed vphone-linux inside the Russet-Dev backend (ext4, never /mnt).
# The IDE's setup page runs these steps; each is re-runnable and skips done work.
#
#   prepare-root            (as root) sudo for the managed user, stricter device rules
#   fetch                   clone the pinned upstream and its submodules (no root)
#   install                 fetch, then setup.sh -y (deps, sources, build, release; uses sudo)
#   seed-cache DIR          copy IPSWs from a local read-only cache into ~/.cache/vphone
#   create [ARGS...]        setup/create-vm.sh; default iPhone17,3 / iOS 27.0.1 at 2x
#   status                  JSON summary for the IDE, no private paths or identifiers
#   check                   JSON: the build in use against upstream main (one git ls-remote)
#   update [--retry]        build upstream main beside the build in use, then switch to it
#   rollback                switch back to the build the last update replaced
#   start [--device D --ios V]  start a created virtual iPhone in its window and unlock it
#   stop                    shut the running virtual iPhone down
#   unlock                  unlock the running virtual iPhone's screen (vphoned)
#   button NAME             press lock, volume-up, volume-down or home (vphoned)
#   install-app APP         install a built .app (or .ipa) on the running virtual iPhone and launch it
#   debug-app APP [--runtime DIR]  install APP, launch it under the phone's debugserver; prints
#                           RESULT {json} with the sockets for LLDB and lldb-dap in DIR (vphone_debug.py)
set -Eeuo pipefail

# The first install's commit: upstream main with the WSL2 support (2026-10-30). Updates then
# follow upstream main (below).
PIN=${RUSSET_VPHONE_PIN:-c3b9ecdaf621c3b01d66b5397c00a2252b035df9}
UPSTREAM=https://github.com/YahiaAngelo/vphone-linux.git
BRANCH=main
# Updates: `check` asks GitHub for main's commit, one `git ls-remote`, cheap enough for the IDE
# to poll every 30 s. `update` builds that commit in a folder of its own under VERSIONS, with
# Russet's patches, beside the build in use, which it never touches; it then points CURRENT at
# the new build once no virtual iPhone runs or is being created. A failed update leaves
# everything as it was, and the build it replaced stays as `previous` for `rollback`. Builds
# stay where they were built: their build trees record absolute paths.
VERSIONS=$HOME/.local/share/russet/vphone
CURRENT=$VERSIONS/current
if [ -z "${RUSSET_VPHONE_ROOT:-}" ] && [ -L "$CURRENT" ]; then
  # Resolved once: a step keeps the build it started with while an update switches.
  ROOT=$(readlink -f "$CURRENT")
else
  ROOT=${RUSSET_VPHONE_ROOT:-$HOME/vphone-linux}
fi
CACHE=$HOME/.cache/vphone
# The running virtual iPhone's screen and input for the IDE: QEMU's VNC over a WebSocket.
VNC_WS_PORT=${RUSSET_VPHONE_VNC_WS_PORT:-5700}
# git looks for a repository in the working directory, even for ls-remote: never the caller's.
cd "$HOME"
log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

prepare_root() {
  [ "$(id -u)" = 0 ] || die "prepare-root runs as root (wsl -u root)"
  local user=${1:-russet}
  id "$user" >/dev/null || die "no managed user $user"
  # Upstream's setup and create flows call sudo for packages, udev, module loads and the
  # firmware patcher's ramdisk mounts. This distro exists only for Russet.
  printf '%s ALL=(ALL) NOPASSWD: ALL\n' "$user" > /etc/sudoers.d/90-russet-vphone
  chmod 440 /etc/sudoers.d/90-russet-vphone
  visudo -cq -f /etc/sudoers.d/90-russet-vphone
  # Pre-seed vhost-vsock access through the kvm group, so upstream's world-writable
  # rule (written only when missing) never lands.
  echo vhost_vsock > /etc/modules-load.d/vhost-vsock.conf
  echo 'KERNEL=="vhost-vsock", GROUP="kvm", MODE="0660"' > /etc/udev/rules.d/90-vhost-vsock.rules
  udevadm control --reload
  usermod -aG kvm "$user"
  log "managed user $user prepared"
}

fetch() {
  # The first install's checkout is at PIN; a build an update made stays at its own commit.
  case $ROOT in
    "$VERSIONS"/*) ;;
    *)
      if [ ! -d "$ROOT/.git" ]; then
        log "cloning vphone-linux at $PIN"
        git clone --no-checkout "$UPSTREAM" "$ROOT"
      fi
      git -C "$ROOT" fetch -q origin "$PIN" 2>/dev/null || true
      git -C "$ROOT" checkout -q "$PIN" ;;
  esac
  # Single level, as upstream's setup does: reims-vgpu's vendor/qemu is not used.
  git -C "$ROOT" submodule update --init
  git -C "$ROOT/qemu" submodule update --init util/mlib
  apply_patches "$ROOT"
}

# Russet's local fixes to upstream, until they land there. The tree's .russet-patches lists those
# applied, by content: a listed one is skipped (a reverse check alone misjudges a patch whose lines
# a later one changes again); an unlisted one applies, or is in the tree already (upstream took
# it); one that does neither fails, naming it.
apply_patches() {
  local dir=$1 patch sum list
  list=$dir/.russet-patches
  for patch in "${RUSSET_VPHONE_PATCHES:-$(dirname "$0")/patches}"/vphone-linux-*.patch; do
    [ -f "$patch" ] || continue
    sum="$(sha256sum < "$patch" | cut -c1-64) $(basename "$patch")"
    grep -qxF "$sum" "$list" 2>/dev/null && continue
    if ! git -C "$dir" apply --reverse --check "$patch" 2>/dev/null; then
      log "applying $(basename "$patch")"
      git -C "$dir" apply "$patch" || die "$(basename "$patch") does not apply to $(git -C "$dir" rev-parse --short=12 HEAD)"
    fi
    printf '%s\n' "$sum" >> "$list"
  done
}

install() {
  fetch
  # setup.sh's sources phase clones nothing here: the checkout already exists.
  "$ROOT/setup/setup.sh" -y
}

seed_cache() {
  local from=${1:?seed-cache needs a directory}
  mkdir -p "$CACHE"
  local file name
  for file in "$from"/*.ipsw; do
    [ -f "$file" ] || continue
    name=$(basename "$file")
    if [ -f "$CACHE/$name" ] && [ "$(stat -c %s "$CACHE/$name")" = "$(stat -c %s "$file")" ]; then
      log "cached: $name"; continue
    fi
    log "copying $name ($(( $(stat -c %s "$file") / 1048576 )) MiB)"
    # The source is reference material: copy only, through a partial name.
    cp --no-preserve=ownership "$file" "$CACHE/.$name.partial"
    mv "$CACHE/.$name.partial" "$CACHE/$name"
  done
}

# A restore that stopped part-way leaves an APFS container on Disk.img, and the next restore's
# newfs_apfs then fails ("already a synthesized APFS device: Resource busy"). Upstream's advice:
# restore again from a blank disk with zeroed nvram.bin and SEPStorage. Only while the restore
# step itself is unfinished, so a created VM is never touched.
reset_interrupted_restore() {
  local device=iPhone17,3 ios=27.0.1 vm state
  while [ $# -gt 0 ]; do
    case $1 in --device) device=$2; shift ;; --ios) ios=$2; shift ;; esac; shift
  done
  vm=${VSETUP_DEVICES:-$HOME/vphone-setup-work/devices}/vphone-${device//,/-}-${ios//./}
  state=$vm/.create-state
  [ -f "$state" ] && grep -qx patch "$state" && ! grep -qx restore "$state" && [ -d "$vm/.runs-dfu" ] || return 0
  log "resetting the VM disk, NVRAM and SEP storage after an interrupted restore"
  "$ROOT/tools/dfu-stop.sh" "$vm" >/dev/null 2>&1 || true
  truncate -s 0 "$vm/Disk.img" && truncate -s 68719476736 "$vm/Disk.img"
  local file size
  for file in nvram.bin:33570816 SEPStorage:524288; do
    size=${file#*:}; file=$vm/${file%%:*}
    [ "$(stat -c %s "$file")" = "$size" ] && head -c "$size" /dev/zero > "$file"
  done
}

create() {
  [ -x "$ROOT/setup/create-vm.sh" ] || die "run install first"
  if [ $# -eq 0 ]; then set -- --device "iPhone17,3" --ios 27.0.1 --scale 2; fi
  # vphone-cli finds guest-resources/ relative to the working directory.
  cd "$ROOT"
  reset_interrupted_restore "$@"
  # An update switches builds only while no creation holds this lock.
  mkdir -p "$VERSIONS"
  exec 7>"$VERSIONS/.use.lock"
  flock -s 7
  # WSL exports the Windows computer name as NAME, which create-vm.sh would take as
  # the VM's name: clear it so upstream names the VM after the device and iOS.
  # In a session of its own: the VM it leaves running outlives this step's WSL session,
  # whose end would otherwise hang it up (QEMU: "terminating on signal 1").
  env -u NAME setsid --wait "$ROOT/setup/create-vm.sh" "$@"
}

# Everything create-vm.sh needs from a build: QEMU, the APFS module, vphone-cli's tooling, the ROMs.
built_in() {
  local r=$1
  [ -x "$r/qemu/build/qemu-system-aarch64" ] && [ -f "$r/.build/linux-apfs-rw/apfs.ko" ] &&
    [ -x "$r/linux-create/vphone-cli" ] && [ -f "$r/OpenAVB/OpenAVB.bin" ] && [ -f "$r/OpenSEP/OpenSEP.bin" ]
}

vm_running() { pgrep -f '/qemu-system-aarch64 ' >/dev/null; }

upstream_head() {
  local sha
  sha=$(timeout 30 git ls-remote "$UPSTREAM" "refs/heads/$BRANCH" | cut -f1) || true
  [[ $sha =~ ^[0-9a-f]{40}$ ]] || die "GitHub did not answer for $UPSTREAM $BRANCH"
  printf '%s\n' "$sha"
}

# The state of the build of `sha` under VERSIONS: none, building, interrupted, built or failed.
staged_state() {
  local dir=$VERSIONS/${1:0:12}
  # A commit rolled back from is not switched to again until retried.
  if [ "$(cat "$VERSIONS/skip" 2>/dev/null)" = "$1" ]; then echo skipped
  elif [ ! -d "$dir" ]; then echo none
  elif [ -f "$dir/.russet-ready" ]; then echo built
  elif [ -f "$dir/.russet-failed" ]; then echo failed
  elif ! flock -n "$VERSIONS/.update.lock" true 2>/dev/null; then echo building
  else echo interrupted
  fi
}

check() {
  mkdir -p "$VERSIONS"
  local upstream current state reason=""
  upstream=$(upstream_head)
  current=$(git -C "$ROOT" rev-parse HEAD)
  state=$(staged_state "$upstream")
  [ "$state" = failed ] && reason=$(head -c 200 "$VERSIONS/${upstream:0:12}/.russet-failed" | tr -d '"\\\n')
  printf '{"current":"%s","upstream":"%s","update":%s,"staged":"%s","reason":"%s","running":%s}\n' \
    "${current:0:12}" "${upstream:0:12}" "$([ "$upstream" = "$current" ] && echo false || echo true)" \
    "$state" "$reason" "$(vm_running && echo true || echo false)"
}

# Clones upstream at `sha` into `dir` and its submodules, borrowing the build in use's objects
# (--dissociate copies them, so `dir` does not depend on that build): only new objects download.
clone_beside() {
  local sha=$1 dir=$2 key path name url
  git clone -q --no-checkout --reference-if-able "$ROOT" --dissociate "$UPSTREAM" "$dir"
  git -C "$dir" checkout -q "$sha"
  while read -r key path; do
    name=${key#submodule.}; name=${name%.path}
    url=$(git -C "$dir" config -f .gitmodules "submodule.$name.url")
    git clone -q --no-checkout --reference-if-able "$ROOT/$path" --dissociate "$url" "$dir/$path"
  done < <(git -C "$dir" config -f .gitmodules --get-regexp '^submodule\..*\.path$')
  git -C "$dir" submodule update -q --init
  git -C "$dir/qemu" submodule update -q --init util/mlib
  # setup.sh clones linux-apfs-rw itself where it finds none; give it the build in use's.
  if [ -d "$ROOT/.build/linux-apfs-rw/.git" ]; then
    git clone -q --no-checkout --reference-if-able "$ROOT/.build/linux-apfs-rw" --dissociate \
      https://github.com/linux-apfs/linux-apfs-rw.git "$dir/.build/linux-apfs-rw"
  fi
}

# Points CURRENT at `next`, keeping the build in use as `previous`, when no virtual iPhone runs
# and none is being created; otherwise leaves `next` built for the next `update`.
switch_to() {
  local next=$1
  exec 6>"$VERSIONS/.use.lock"
  if vm_running || ! flock -n 6; then
    log "vphone-linux $(git -C "$next" rev-parse --short=12 HEAD) is built; it is used once no virtual iPhone runs"
    return 0
  fi
  ln -sfn "$ROOT" "$VERSIONS/.previous" && mv -T "$VERSIONS/.previous" "$VERSIONS/previous"
  ln -sfn "$next" "$VERSIONS/.current" && mv -T "$VERSIONS/.current" "$CURRENT"
  flock -u 6
  log "now using vphone-linux $(git -C "$next" rev-parse --short=12 HEAD) (the previous build is kept for rollback)"
  prune
}

# Removes the builds under VERSIONS other than the current and previous ones. The first
# install's checkout (outside VERSIONS) is never removed.
prune() {
  local keep=() d
  keep+=("$(readlink -f "$CURRENT")")
  [ -L "$VERSIONS/previous" ] && keep+=("$(readlink -f "$VERSIONS/previous")")
  for d in "$VERSIONS"/*/; do
    d=${d%/}
    [[ $(basename "$d") =~ ^[0-9a-f]{12}$ ]] || continue
    [[ " ${keep[*]} " == *" $d "* ]] && continue
    log "removing the old build $(basename "$d")"
    rm -rf -- "$d"
  done
}

update() {
  mkdir -p "$VERSIONS"
  exec 8>"$VERSIONS/.update.lock"
  flock -n 8 || die "another update is running"
  local sha dir
  UPDATE_PHASE="fetching failed"
  sha=$(upstream_head)
  dir=$VERSIONS/${sha:0:12}
  if [ "$sha" = "$(git -C "$ROOT" rev-parse HEAD)" ]; then
    log "vphone-linux is up to date (${sha:0:12})"; return 0
  fi
  if [ "${1:-}" = --retry ]; then rm -f "$dir/.russet-failed" "$VERSIONS/skip"; fi
  case $(staged_state "$sha") in
    skipped) log "${sha:0:12} was rolled back from; update --retry uses it again"; return 0 ;;
    built) switch_to "$dir"; return 0 ;;
    failed) die "the update to ${sha:0:12} failed before ($(cat "$dir/.russet-failed")); retry with update --retry" ;;
  esac
  # A build takes about 15 GB; the distro's disk grows on C:.
  local free
  if free=$(df --output=avail -BG /mnt/c 2>/dev/null | tail -1 | tr -dc 0-9) && [ -n "$free" ] && [ "$free" -lt 30 ]; then
    die "an update needs 30 GB free on C: (${free} GB free)"
  fi
  # Interrupted or retried: start this commit's folder again (only ever under VERSIONS).
  rm -rf -- "$dir"
  # A step that fails marks the commit failed, so polling does not rebuild it until retried
  # or upstream moves on; a killed update leaves it interrupted, and the next one restarts it.
  # The folder is fixed into the trap now; the phase is global (the trap runs at the script's exit).
  trap "mkdir -p '$dir' && printf '%s\n' \"\$UPDATE_PHASE\" > '$dir/.russet-failed'" EXIT
  log "fetching vphone-linux ${sha:0:12} beside $(git -C "$ROOT" rev-parse --short=12 HEAD)"
  clone_beside "$sha" "$dir"
  UPDATE_PHASE="a Russet patch does not apply"
  apply_patches "$dir"
  UPDATE_PHASE="the build failed"
  log "building vphone-linux ${sha:0:12} (an hour or more; the build in use stays usable)"
  # At idle scheduling: the build gets only the CPU a running virtual iPhone leaves, which on
  # TCG and lavapipe is little enough already (a niced build still starved it).
  ( cd "$dir" && chrt -i 0 nice -n 19 "$dir/setup/setup.sh" -y )
  UPDATE_PHASE="the build is incomplete"
  built_in "$dir" || die "$UPDATE_PHASE"
  "$dir/qemu/build/qemu-system-aarch64" -machine help | grep -q vresearch101 || die "$UPDATE_PHASE"
  # Built: no failure marker at the script's exit.
  trap - EXIT
  touch "$dir/.russet-ready"
  switch_to "$dir"
}

rollback() {
  [ -L "$VERSIONS/previous" ] || die "no previous build to go back to"
  exec 6>"$VERSIONS/.use.lock"
  if vm_running || ! flock -n 6; then die "stop the virtual iPhone first"; fi
  local back
  back=$(readlink -f "$VERSIONS/previous")
  git -C "$ROOT" rev-parse HEAD > "$VERSIONS/skip"
  ln -sfn "$ROOT" "$VERSIONS/.previous" && mv -T "$VERSIONS/.previous" "$VERSIONS/previous"
  ln -sfn "$back" "$VERSIONS/.current" && mv -T "$VERSIONS/.current" "$CURRENT"
  log "now using vphone-linux $(git -C "$back" rev-parse --short=12 HEAD) again"
}

status() {
  local head=none qemu=false built=false vms=0 ready=0 state
  [ -d "$ROOT/.git" ] && head=$(git -C "$ROOT" rev-parse --short HEAD)
  [ -x "$ROOT/qemu/build/qemu-system-aarch64" ] && qemu=true
  built_in "$ROOT" && built=true
  # create-vm.sh keeps one folder per VM; its state file ends with "booted" when created.
  for state in "${VSETUP_DEVICES:-$HOME/vphone-setup-work/devices}"/*/.create-state; do
    [ -f "$state" ] || continue
    vms=$((vms + 1))
    grep -qx booted "$state" && ready=$((ready + 1))
  done
  printf '{"pin":"%s","checkout":"%s","qemuBuilt":%s,"built":%s,"vms":%s,"vmsReady":%s,"running":%s}\n' \
    "${PIN:0:10}" "$head" "$qemu" "$built" "$vms" "$ready" "$(vm_running && echo true || echo false)"
}

vphoned() { CID=${CID:-3} VPHONED_TIMEOUT=${VPHONED_TIMEOUT:-30} timeout 60 "$ROOT/tools/vphoned.sh" "$@"; }

# Starts a created virtual iPhone in a session of its own, so it outlives this step, in its window
# on the Windows desktop (WSLg); waits for vphoned, then unlocks it. A running one is left alone.
start() {
  local device=iPhone17,3 ios=27.0.1 vm i
  while [ $# -gt 0 ]; do
    case $1 in --device) device=$2; shift ;; --ios) ios=$2; shift ;; esac; shift
  done
  vm=${VSETUP_DEVICES:-$HOME/vphone-setup-work/devices}/vphone-${device//,/-}-${ios//./}
  grep -qx booted "$vm/.create-state" 2>/dev/null || die "this virtual iPhone is not created yet"
  if vm_running; then log "the virtual iPhone is running"; return 0; fi
  log "starting the virtual iPhone (several minutes to the lock screen)"
  # The IDE shows the screen in its Virtual iPhone tab, from QEMU's VNC server over a WebSocket on
  # the loopback (WSL forwards it to Windows): no WSLg window, whose drags bring no motion
  # (microsoft/wslg#1300). RUSSET_VPHONE_WINDOW=1 brings the window back as well.
  # QEMU's VNC server loads its keymap from the build's data folder, which the build leaves empty
  # without qemu-keymap: copy the source tree's.
  if [ ! -f "$ROOT/qemu/build/pc-bios/keymaps/en-us" ]; then
    mkdir -p "$ROOT/qemu/build/pc-bios/keymaps"
    find "$ROOT/qemu/pc-bios/keymaps" -maxdepth 1 -type f ! -name 'meson.build' -exec cp -t "$ROOT/qemu/build/pc-bios/keymaps" {} +
  fi
  # run-vphone.sh keeps its GPU census under the working directory's runs/.
  # 6 vCPUs: 8, the SEP core and lavapipe's threads oversubscribe the distro's 12 CPUs, and an app
  # launch keeps 2-3 busy (vphone-linux docs/WSL.md and PERFORMANCE.md). RUSSET_VPHONE_SMP overrides.
  ( cd "$ROOT" && GFX=1 SMP=${RUSSET_VPHONE_SMP:-6} WINDOW=${RUSSET_VPHONE_WINDOW:-0} CID=${CID:-3} setsid -f "$ROOT/run-vphone.sh" "$vm" \
      -vnc "127.0.0.1:0,websocket=127.0.0.1:$VNC_WS_PORT" -qmp "unix:$vm/qmp.sock,server=on,wait=off" \
      > "$vm/serial.log" 2>&1 < /dev/null )
  for i in $(seq 1 120); do
    vphoned GET /v1/health 2>/dev/null | grep -q '"status":"ok"' && break
    sleep 1; vm_running || die "the virtual iPhone stopped while starting (see serial.log in its folder)"
    sleep 9
  done
  vphoned GET /v1/health 2>/dev/null | grep -q '"status":"ok"' || die "the virtual iPhone did not answer within 20 minutes"
  unlock_screen
  # Right after a boot every service starts at once on emulated CPUs, and iOS's launch watchdogs
  # (0x8badf00d) kill the wallpaper (PosterBoard, 30 s; its renderer, 20 s) and the widgets
  # (10 s) before they get the CPU: a black home screen without labels. Once the boot has
  # settled, restarting SpringBoard starts them alone, in time.
  log "letting the boot settle, then restarting the home screen for the wallpaper and widgets"
  sleep 90
  vphoned POST /v1/rpc '{"method":"system.respring","params":{"force":true},"id":1}' >/dev/null 2>&1 || true
  sleep 30
  for i in $(seq 1 30); do
    vphoned GET /v1/health 2>/dev/null | grep -q '"status":"ok"' && break
    sleep 10
  done
  unlock_screen
  log "the virtual iPhone is running"
}

# A hardware button through vphoned's `input.button`: power (the side button: Lock), volume-up,
# volume-down, home. (Its `input.hid` power usage does not lock.)
press_button() {
  local name
  case $1 in
    lock) name=power ;; volume-up|volume-down|home) name=$1 ;;
    *) die "unknown button: $1" ;;
  esac
  vm_running || die "the virtual iPhone is not running"
  vphoned POST /v1/rpc "{\"method\":\"input.button\",\"params\":{\"name\":\"$name\"},\"id\":1}" | grep -q '"result"' ||
    die "the virtual iPhone did not take the $1 button"
}

# Installs an app built for iOS (an .app folder, or an .ipa) on the running virtual iPhone and
# launches it: vphoned signs it inside the guest, so the build's own signature does not matter
# (vphone-linux tools/install-ipa.py; about 8 s, a reinstall about 3 s).
install_app() {
  local app=${1:-} bundle=${2:-} launch=${3:-launch} tmp ipa
  vm_running || die "the virtual iPhone is not running"
  [ -e "$app" ] || die "no app at $app"
  case $app in
    *.ipa) ipa=$app; [ -n "$bundle" ] || die "give the bundle id of $app" ;;
    *)
      [ -n "$bundle" ] || bundle=$(python3 -c 'import plistlib, sys; print(plistlib.load(open(sys.argv[1], "rb"))["CFBundleIdentifier"])' "$app/Info.plist")
      tmp=$(mktemp -d)
      mkdir "$tmp/Payload" && cp -a "$app" "$tmp/Payload/"
      ( cd "$tmp" && python3 -m zipfile -c app.ipa Payload )
      ipa=$tmp/app.ipa ;;
  esac
  log "installing $bundle on the virtual iPhone"
  if [ "$launch" = launch ]; then
    python3 "$ROOT/tools/install-ipa.py" "$ipa" --launch "$bundle" --cid "${CID:-3}"
  else
    python3 "$ROOT/tools/install-ipa.py" "$ipa" --cid "${CID:-3}"
  fi
  [ -z "${tmp:-}" ] || rm -rf -- "$tmp"
  [ "$launch" != launch ] || log "$bundle is running on the virtual iPhone"
  INSTALLED_BUNDLE=$bundle
}

# Installs APP, then launches it with the phone's debugserver attached (vphone_debug.py); the IDE
# connects lldb-dap through the sockets on the RESULT line.
debug_app() {
  local app="" runtime=$HOME/.russet-dev/ide-development/runtime bundle session
  while [ $# -gt 0 ]; do
    case $1 in --runtime) runtime=$2; shift ;; *) app=$1 ;; esac; shift
  done
  vm_running || die "the virtual iPhone is not running"
  [ -f "$app/Info.plist" ] || die "no built app at $app"
  bundle=$(python3 -c 'import plistlib, sys; print(plistlib.load(open(sys.argv[1], "rb"))["CFBundleIdentifier"])' "$app/Info.plist")
  # The previous debug session ends first: its helper, its lldb-dap and the app it held stopped
  # (the phone refuses to replace a running app).
  pkill -f 'vphone_debug.py prepare' 2>/dev/null || true
  pkill -f "lldb-dap --connection accept://$runtime/virtual-debug/" 2>/dev/null || true
  find "$runtime/virtual-debug" -mindepth 1 -maxdepth 1 -mmin +60 -exec rm -rf -- {} + 2>/dev/null || true
  vphoned POST /v1/rpc "{\"method\":\"apps.terminate\",\"params\":{\"bundle_id\":\"$bundle\"},\"id\":1}" >/dev/null 2>&1 || true
  install_app "$app" "$bundle" no-launch
  session=$runtime/virtual-debug/$(date +%s)
  python3 "$(dirname "$(readlink -f "$0")")/vphone_debug.py" prepare "$INSTALLED_BUNDLE" "$session" --cid "${CID:-3}" --local-app "$(readlink -f "$app")"
}

unlock_screen() {
  local i
  log "unlocking"
  for i in $(seq 1 20); do
    vphoned POST /v1/rpc '{"method":"screen.unlock","params":{},"id":1}' 2>/dev/null | grep -q '"locked":false' && return 0
    sleep 10
  done
}

# Shuts the running virtual iPhone down through vphoned, then stops its QEMU if it lingers.
stop() {
  local i
  if ! vm_running; then log "the virtual iPhone is not running"; return 0; fi
  log "shutting the virtual iPhone down"
  vphoned POST /v1/rpc '{"method":"system.shutdown","params":{"force":true},"id":1}' >/dev/null 2>&1 || true
  for i in $(seq 1 60); do vm_running || break; sleep 2; done
  if vm_running; then
    pkill -TERM -f '/qemu-system-aarch64 .*vphone-setup-work/devices/' || true
    for i in $(seq 1 15); do vm_running || break; sleep 2; done
  fi
  vm_running && die "the virtual iPhone did not stop"
  log "the virtual iPhone is stopped"
}

case ${1:-} in
  prepare-root) shift; prepare_root "$@" ;;
  fetch) fetch ;;
  install) install ;;
  seed-cache) shift; seed_cache "$@" ;;
  create) shift; create "$@" ;;
  status) status ;;
  check) check ;;
  update) shift; update "$@" ;;
  rollback) rollback ;;
  start) shift; start "$@" ;;
  stop) stop ;;
  unlock) vm_running || die "the virtual iPhone is not running"; unlock_screen ;;
  button) shift; press_button "${1:-}" ;;
  install-app) shift; install_app "$@" ;;
  debug-app) shift; debug_app "$@" ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
