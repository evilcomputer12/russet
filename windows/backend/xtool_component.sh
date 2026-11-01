#!/usr/bin/env bash
# Russet-managed xtool inside the Russet-Dev backend. The IDE's setup page runs
# these steps; xtool builds the iOS Swift SDK from an Xcode .xip until the
# engine's own `sdk.import` (W16b) replaces it. Sign-in is Russet's, not xtool's.
#
#   install                 pinned AppImage, checksum-verified, extracted (no FUSE)
#   sdk XIP                 build ~/.swiftpm/swift-sdks/darwin.artifactbundle from Xcode
#   status                  JSON summary for the IDE
set -Eeuo pipefail

VERSION=1.24.0
SHA256=dbfd9d7216f077c84c171b688c4173c37c23ca76bf7fd185d1a9ae8a53dab59f
URL=https://github.com/xtool-org/xtool/releases/download/$VERSION/xtool-x86_64.AppImage
OPT=$HOME/.local/opt/xtool-$VERSION
BIN=$HOME/.local/bin/xtool
# xtool runs swiftc while installing the SDK: use the toolchain Russet builds with.
export PATH=${RUSSET_SWIFT_BIN:-/usr/lib/swift/usr/bin}:$PATH
log() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

install() {
  if [ -x "$OPT/AppRun" ]; then log "xtool $VERSION present"; else
    local tmp; tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
    log "downloading xtool $VERSION"
    curl -fsSL --retry 3 -o "$tmp/xtool.AppImage" "$URL"
    echo "$SHA256  $tmp/xtool.AppImage" | sha256sum -c --quiet || die "xtool checksum mismatch"
    chmod +x "$tmp/xtool.AppImage"
    # Extract instead of mounting: WSL distros may lack libfuse2.
    (cd "$tmp" && ./xtool.AppImage --appimage-extract >/dev/null)
    mkdir -p "$(dirname "$OPT")"
    mv "$tmp/squashfs-root" "$OPT"
  fi
  mkdir -p "$(dirname "$BIN")"
  ln -sfn "$OPT/AppRun" "$BIN"
  "$BIN" --version
}

sdk() {
  local xip=${1:?sdk needs the Xcode .xip path}
  [ -f "$xip" ] || die "no such file: $xip"
  [ -x "$BIN" ] || die "run install first"
  local bundle=$HOME/.swiftpm/swift-sdks/darwin.artifactbundle
  # xtool installs into the real home whatever HOME says; keep the previous bundle
  # aside until the new one is in place, then drop it.
  if [ -e "$bundle" ]; then rm -rf "$bundle.previous"; mv "$bundle" "$bundle.previous"; fi
  if "$BIN" sdk install "$xip" < /dev/null; then rm -rf "$bundle.previous"; return 0; fi
  rm -rf "$bundle"
  [ -e "$bundle.previous" ] && mv "$bundle.previous" "$bundle"
  return 1
}

status() {
  local version=none sdk=false bundle=$HOME/.swiftpm/swift-sdks/darwin.artifactbundle
  [ -x "$BIN" ] && version=$("$BIN" --version 2>/dev/null | head -1 || true)
  # Russet's builds need the full bundle (xtool's Xcode.app copy), not --slim.
  [ -f "$bundle/info.json" ] && [ -f "$bundle/Xcode.app/Contents/version.plist" ] && sdk=true
  printf '{"xtool":"%s","sdk":%s}\n' "$version" "$sdk"
}

case ${1:-} in
  install) install ;;
  sdk) shift; sdk "$@" ;;
  status) status ;;
  *) sed -n '2,10p' "$0"; exit 2 ;;
esac
