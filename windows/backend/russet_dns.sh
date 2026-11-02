#!/bin/sh
# Name lookups in the Russet-Dev backend. WSL writes /mnt/wsl/resolv.conf: Windows' resolver
# through DNS tunneling (nameserver 10.255.255.254) or its NAT gateway. On some networks that
# answers nothing while Windows, and WSL's own connections, work ("Temporary failure resolving
# 'archive.ubuntu.com'" on a work PC), and apt, git and SwiftPM fail. WSL would also put its
# link back at the start of every command, so Russet-Dev's /etc/wsl.conf turns that off
# (generateResolvConf = false, BackendSync) and /etc/resolv.conf is this script's to write. It
# is always a file of its own: WSL deletes a link to its file when it does not generate it. As
# root, at every start of the distribution (russet-dns.service) and before the setup steps that
# use apt:
#   - WSL's DNS answers: /etc/resolv.conf is a copy of WSL's file, so a VPN's or a company's
#     names work;
#   - it does not and public DNS does: /etc/resolv.conf names public servers;
#   - neither: WSL's copy, the likelier to answer once the network does.
# Only this distribution changes; WSL's settings, /mnt/wsl and other distributions never do.
#
#   russet_dns.sh           check and choose
#   russet_dns.sh install   the same at every start of the distribution (systemd), then check
set -u

WSL_FILE=${RUSSET_DNS_WSL_FILE:-/mnt/wsl/resolv.conf}
TARGET=${RUSSET_DNS_TARGET:-/etc/resolv.conf}
PUBLIC="1.1.1.1 8.8.8.8"
PROBE=archive.ubuntu.com
WSL_HEADER="# Russet (russet_dns.sh): WSL's DNS, copied from $WSL_FILE, answers on this network."
PUBLIC_HEADER="# Russet (russet_dns.sh): WSL's DNS did not answer on this network; these public servers did."
UNIT=/etc/systemd/system/russet-dns.service

log() { printf '==> %s\n' "$*"; }

# Whether glibc resolves PROBE with the resolver configuration in file $1, with short timeouts
# (a resolver that never answers costs seconds, not a minute): mounted over /etc/resolv.conf in
# a mount namespace of its own, which nothing else sees. The mount needs a file there.
resolves() {
  [ -f /etc/resolv.conf ] || { rm -f /etc/resolv.conf; : > /etc/resolv.conf; }
  { cat "$1"; echo "options timeout:3 attempts:2"; } > "$1.probe"
  unshare -m sh -c 'mount --bind "$1" /etc/resolv.conf && timeout 15 getent ahostsv4 "$2" >/dev/null' sh "$1.probe" "$PROBE" 2>/dev/null
}

# Puts file $1 at TARGET in one step, saying $2 when that changed it.
use() {
  if ! [ -f "$TARGET" ] || [ -L "$TARGET" ] || ! cmp -s "$1" "$TARGET"; then
    chmod 644 "$1" && cp "$1" "$TARGET.russet" && mv -f "$TARGET.russet" "$TARGET" && log "$2"
  fi
}

check() {
  tmp=$(mktemp -d) || return 0
  have_wsl=
  if [ -f "$WSL_FILE" ]; then
    { echo "$WSL_HEADER"; grep -v '^#' "$WSL_FILE"; } > "$tmp/wsl" && have_wsl=1
  fi
  { echo "$PUBLIC_HEADER"; for server in $PUBLIC; do echo "nameserver $server"; done; } > "$tmp/public"
  if [ -n "$have_wsl" ] && resolves "$tmp/wsl"; then
    use "$tmp/wsl" "WSL's DNS answers on this network; Russet-Dev uses it"
  elif resolves "$tmp/public"; then
    use "$tmp/public" "WSL's DNS does not answer on this network; Russet-Dev uses public DNS ($PUBLIC)"
  else
    log "no DNS server answers, WSL's or public"
    if [ -n "$have_wsl" ] && [ "$(head -n 1 "$TARGET" 2>/dev/null)" != "$PUBLIC_HEADER" ]; then
      use "$tmp/wsl" "Russet-Dev uses WSL's DNS"
    fi
  fi
  rm -rf "$tmp"
}

install_unit() {
  cat > "$UNIT" <<EOF
[Unit]
Description=Russet: name lookups for Russet-Dev (public DNS where WSL's does not answer)
After=network.target systemd-tmpfiles-setup.service

[Service]
Type=oneshot
ExecStart=/opt/russet/lib/russet_dns.sh

[Install]
WantedBy=multi-user.target
EOF
  # Enabled by hand: before its first restart the distribution may not run systemd yet.
  mkdir -p /etc/systemd/system/multi-user.target.wants
  ln -sfn "$UNIT" /etc/systemd/system/multi-user.target.wants/russet-dns.service
  if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
}

[ "$(id -u)" = 0 ] || { echo "russet_dns.sh runs as root" >&2; exit 1; }
case ${1:-} in
  install) install_unit; check ;;
  "") check ;;
  *) echo "usage: russet_dns.sh [install]" >&2; exit 2 ;;
esac
