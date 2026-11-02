#!/bin/sh
# Name lookups in the Russet-Dev backend. WSL points /etc/resolv.conf at the file it writes,
# /mnt/wsl/resolv.conf: Windows' resolver through DNS tunneling (nameserver 10.255.255.254) or
# its NAT gateway. On some networks that answers nothing while Windows, and WSL's own
# connections, work ("Temporary failure resolving 'archive.ubuntu.com'" on a work PC), and apt,
# git and SwiftPM fail. As root, at every start of the distribution (russet-dns.service) and
# before the setup steps that use apt:
#   - WSL's DNS answers: /etc/resolv.conf is WSL's again, so a VPN's or a company's names work;
#   - it does not and public DNS does: /etc/resolv.conf is Russet's file naming public servers;
#   - neither, or a resolv.conf someone else wrote: left as it is.
# Only this distribution changes; WSL's settings and other distributions never do.
#
#   russet_dns.sh           check and choose
#   russet_dns.sh install   the same at every start of the distribution (systemd), then check
set -u

WSL_FILE=${RUSSET_DNS_WSL_FILE:-/mnt/wsl/resolv.conf}
TARGET=${RUSSET_DNS_TARGET:-/etc/resolv.conf}
PUBLIC="1.1.1.1 8.8.8.8"
PROBE=archive.ubuntu.com
HEADER="# Russet (russet_dns.sh): WSL's DNS did not answer on this network; these public servers did."
UNIT=/etc/systemd/system/russet-dns.service

log() { printf '==> %s\n' "$*"; }

# Whether glibc resolves PROBE with the resolver configuration in file $1: mounted over
# /etc/resolv.conf in a mount namespace of its own, which nothing else sees.
resolves() {
  unshare -m sh -c 'mount --bind "$1" /etc/resolv.conf && timeout 15 getent ahostsv4 "$2" >/dev/null' sh "$1" "$PROBE" 2>/dev/null
}

check() {
  # Someone's own resolv.conf (generateResolvConf=false, or written by hand) is theirs.
  if [ "$(readlink "$TARGET" 2>/dev/null)" != "$WSL_FILE" ] && [ "$(head -n 1 "$TARGET" 2>/dev/null)" != "$HEADER" ]; then
    return 0
  fi
  tmp=$(mktemp -d) || return 0
  # Short timeouts for the probe only: a resolver that never answers costs seconds, not a minute.
  if [ -f "$WSL_FILE" ] && { cat "$WSL_FILE"; echo "options timeout:3 attempts:2"; } > "$tmp/wsl" && resolves "$tmp/wsl"; then
    if [ "$(readlink "$TARGET" 2>/dev/null)" != "$WSL_FILE" ]; then
      ln -sfn "$WSL_FILE" "$TARGET" && log "WSL's DNS answers again; Russet-Dev uses it"
    fi
    rm -rf "$tmp"
    return 0
  fi
  { echo "$HEADER"; for server in $PUBLIC; do echo "nameserver $server"; done; } > "$tmp/public"
  if resolves "$tmp/public"; then
    if ! cmp -s "$tmp/public" "$TARGET"; then
      rm -f "$TARGET" && cp "$tmp/public" "$TARGET" && chmod 644 "$TARGET" &&
        log "WSL's DNS does not answer on this network; Russet-Dev uses public DNS ($PUBLIC)"
    fi
  else
    log "no DNS server answers, WSL's or public; name lookups stay as they are"
  fi
  rm -rf "$tmp"
}

install_unit() {
  cat > "$UNIT" <<EOF
[Unit]
Description=Russet: name lookups for Russet-Dev (public DNS where WSL's does not answer)
After=network.target

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
