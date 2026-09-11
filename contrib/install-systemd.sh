#!/bin/sh
# SPDX-FileCopyrightText: 2026 Mete Balci
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Installs ozd as a systemd service, as the README's "As a service"
# section describes. It creates the system user ozd, creates the writable
# roots that the file of flags names and gives them to that user, and
# installs the binary, the file of flags and the unit. It then lets the
# unit write to exactly those roots, checks the flags and the roots as
# ozd's user, and starts the service.
#
# Run it as root from a checkout where `cargo build --release` has run:
#
#     sudo contrib/install-systemd.sh ozdrc
#
# where ozdrc is your file of flags. With --dry-run it prints what it
# would do and changes nothing, and it does not need to run as root.

set -euf

usage() {
    echo "usage: $0 [--dry-run] <file of flags>" >&2
    exit 2
}

fail() {
    echo "$0: $*" >&2
    exit 1
}

dry=
if [ "${1:-}" = --dry-run ]; then
    dry=1
    shift
fi
[ $# -eq 1 ] || usage
flags=$1

here=$(cd "$(dirname "$0")/.." && pwd)
binary=$here/target/release/ozd
unit=$here/contrib/ozd.service

# Runs a command, or with --dry-run only prints it.
run() {
    if [ -n "$dry" ]; then
        echo "$*"
    else
        "$@"
    fi
}

[ -r "$flags" ] || fail "cannot read the file of flags: $flags"
[ -x "$binary" ] || fail "$binary is missing: run cargo build --release first"
if [ -z "$dry" ]; then
    [ "$(id -u)" -eq 0 ] || fail "run it as root, with sudo"
    command -v systemctl >/dev/null || fail "systemctl is missing: this script is for systemd"
fi

# The roots, from the file's --root lines. A line's value is the rest of
# the line, without the blanks at either end. A value that does not begin
# with / is name=path. A comma ends the path, and ro after it makes the
# root read-only. ozd checks everything else itself, in the --check below.
writable=
home=
while IFS= read -r line || [ -n "$line" ]; do
    line=$(printf '%s' "$line" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')
    case $line in
        --root[[:space:]]*) ;;
        *) continue ;;
    esac
    value=$(printf '%s' "${line#--root}" | sed 's/^[[:space:]]*//')
    case $value in
        /*) path=$value ;;
        *=*) path=${value#*=} ;;
        *) continue ;;
    esac
    option=
    case $path in
        *,*)
            option=${path#*,}
            path=${path%%,*}
            ;;
    esac
    case $path in
        *[[:space:]]*) fail "$path has a blank in it; list it in the unit's ReadWritePaths= by hand" ;;
        /home | /home/* | /root | /root/* | /run/user/*) home=1 ;;
    esac
    [ "$option" = ro ] || writable=${writable:+$writable }$path
done <"$flags"

if ! id ozd >/dev/null 2>&1; then
    run useradd --system --no-create-home --shell /usr/sbin/nologin ozd
fi
for root in $writable; do
    run mkdir -p "$root"
    run chown ozd: "$root"
done
run install -m 755 "$binary" /usr/local/bin/ozd
run install -m 644 "$flags" /etc/ozdrc
run install -m 644 "$unit" /etc/systemd/system/ozd.service

# The unit lets ozd write only where its ReadWritePaths= says. This
# drop-in replaces the unit's /srv/lispm with the writable roots. A root
# under /home needs ProtectHome=read-only instead of ProtectHome=yes.
dropin=/etc/systemd/system/ozd.service.d
conf="[Service]
ReadWritePaths="
[ -z "$writable" ] || conf="$conf
ReadWritePaths=$writable"
[ -z "$home" ] || conf="$conf
ProtectHome=read-only"
run mkdir -p "$dropin"
if [ -n "$dry" ]; then
    printf '%s\n' "write $dropin/roots.conf:" "$conf"
else
    printf '%s\n' "$conf" >"$dropin/roots.conf"
fi

run runuser -u ozd -- /usr/local/bin/ozd --check -c /etc/ozdrc
run systemctl daemon-reload
run systemctl enable ozd
run systemctl restart ozd
[ -n "$dry" ] || echo "ozd is running: see systemctl status ozd, and journalctl -u ozd for its log."
