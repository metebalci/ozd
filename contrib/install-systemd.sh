#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Installs ozd as a systemd service, as the README's "As a service"
# section describes. It creates the system user ozd and the base root
# /srv/lispm, which that user owns. It installs the binary, the unit, and
# a standard file of flags at /etc/ozdrc, keeping any /etc/ozdrc that is
# already there. It then checks the flags and the roots as ozd's user,
# and starts the service.
#
# Run it as root from a checkout where `cargo build --release` has run:
#
#     sudo contrib/install-systemd.sh
#
# With --dry-run it prints what it would do and changes nothing, and it
# does not need to run as root.

set -euf

usage() {
    echo "usage: $0 [--dry-run]" >&2
    exit 2
}

fail() {
    echo "$0: $*" >&2
    exit 1
}

dry=
case $# in
    0) ;;
    1)
        [ "$1" = --dry-run ] || usage
        dry=1
        ;;
    *) usage ;;
esac

here=$(cd "$(dirname "$0")/.." && pwd)
binary=$here/target/release/ozd
unit=$here/contrib/ozd.service
flags=/etc/ozdrc
base=/srv/lispm

# Runs a command, or with --dry-run only prints it.
run() {
    if [ -n "$dry" ]; then
        echo "$*"
    else
        "$@"
    fi
}

[ -x "$binary" ] || fail "$binary is missing: run cargo build --release first"
if [ -z "$dry" ]; then
    [ "$(id -u)" -eq 0 ] || fail "run it as root, with sudo"
    command -v systemctl >/dev/null || fail "systemctl is missing: this script is for systemd"
fi

# The standard file of flags: this host as a System 100 band's host table
# has its file host, the base root, and the band's own machine for HOSTAB.
standard="# ozd's file of flags: one flag per line, as the README describes.
# This host, as a System 100 band's host table has its file host.
--address 3060
--name MIT-OZ,OZ,system=UNIX
# The base root, which holds the users' homes and which ozd writes.
--root $base
# The release's sources, read-only, where a System 100 band looks for them:
#--root tree=/path/to/system-100-0/sys,ro
# The band's own machine, so that HOSTAB can name it.
--host 3050,MIT-LISPM-1,CADR-1,CADR1,LM1,system=LISPM
# Without --listen, only this host can reach ozd. To serve other hosts:
#--listen 0.0.0.0"

if ! id ozd >/dev/null 2>&1; then
    run useradd --system --no-create-home --shell /usr/sbin/nologin ozd
fi
run mkdir -p "$base"
run chown ozd: "$base"
run install -m 755 "$binary" /usr/local/bin/ozd
if [ -e "$flags" ]; then
    echo "keeping the existing $flags"
elif [ -n "$dry" ]; then
    printf '%s\n' "write $flags:" "$standard"
else
    printf '%s\n' "$standard" >"$flags"
    chmod 644 "$flags"
fi
run install -m 644 "$unit" /etc/systemd/system/ozd.service
run runuser -u ozd -- /usr/local/bin/ozd --check -c "$flags"
run systemctl daemon-reload
run systemctl enable ozd
run systemctl restart ozd
[ -n "$dry" ] || echo "ozd is running: see systemctl status ozd, and journalctl -u ozd for its log."
