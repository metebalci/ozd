# ozd

OZ, also called MIT-OZ, was the host where MIT's Lisp Machines kept
their system files. They also asked it for the time and for their host
table (see System 100's `sys/site/site.lisp`). ozd is OZ as a daemon.

ozd is the associated machine for a site of MIT CADR Lisp Machines. It
is one host on one Chaosnet subnet, reached over UDP. It serves the
machines their files, the time and their host table, and it is the
switch of their subnet: it passes each packet to the machine it is
addressed to. Several machines can share one ozd, each naming it
as its CHUDP peer, just as MIT had one associated machine for many Lisp
Machines.

ozd serves STATUS, TIME, UPTIME, FILE, HOSTAB and NAME, and it passes
packets between the machines on its subnet. The design is in
`DESIGN.md`. Every protocol a Lisp Machine speaks is described in
`PROTOCOLS.md`, together with where each fact comes from.

## Build

ozd is built with Cargo, which you install with
[rustup](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Open a new shell, or run `. "$HOME/.cargo/env"`, and then build from
this directory:

```sh
cargo build --release
```

ozd is written in Rust 2024 and has no crate dependencies. Its compiler
version is pinned in `rust-toolchain.toml`. rustup reads that file and
downloads the pinned toolchain the first time you run Cargo here. If
rustup's `auto-install` setting is `disable`, run `rustup toolchain
install` in this directory instead. Nothing else is downloaded. A Cargo
installed from your system's packages ignores `rust-toolchain.toml` and
may be too old for the 2024 edition, so use rustup.

## Run

This is the configuration for a System 100 band. Its host table lists
the Lisp Machine as MIT-LISPM-1 at 3050 and its file and time host as
MIT-OZ at 3060 (`sys/site/hosts.text`):

```sh
target/release/ozd --address 3060 --name MIT-OZ,OZ,system=UNIX \
    --root /srv/lispm --root tree=/path/to/system-100-0/sys,ro \
    --host 3050,MIT-LISPM-1,CADR-1,CADR1,LM1,system=LISPM
```

- `--address` and `--name` describe this host as the band's host table
  does: its address, its names with the official name first, and its
  system type.
- `--root /srv/lispm` is the base root. It holds the users' home
  directories, and ozd writes to it.
- `--root tree=...,ro` mounts the release's sources at `/tree`,
  read-only. That is where the band looks for them
  (`sys/site/sys.translations`). Every root must be an existing
  directory when ozd starts.
- A site can have mounts and no base root. `/` is then read-only and
  lists only the mounts. `LOGIN` gives each user a home directory,
  `/<user>/` in lower case. Without a base root that directory does not
  exist, unless a writable mount has the same name. For example,
  `--root lispm=/srv/lispm` becomes the home of the user LISPM.
- `--host` adds a machine to the host table that HOSTAB answers from.
  Bands whose own table does not know the machine can then find it by
  name. Each further machine needs its own address and its own `--host`.
- `--hosts-text` names a band's own host table, such as
  `/srv/lispm/sys/site/hosts.text`, and HOSTAB answers for the hosts in it
  as well. A site that already keeps that file for its machines then writes
  each host once instead of twice. This host's own line in it is passed
  over, because `--name` gives its names, and a host with no Chaosnet
  address is skipped. ozd reads the file when it starts, so a change to it
  wants a restart.
- `--log-simple`, `--log-file` and `--log-file-probe` each add lines to the
  log, and each works on its own. `--log-simple` writes a line for each
  simple transaction ozd answers, such as TIME, STATUS or UPTIME.
  `--log-file` writes one for each file read, each directory listed and each
  login, with the address of the machine that asked. `--log-file-probe`
  writes one for each FILE probe, which a band makes far more often than it
  reads. Without them, a machine's whole boot leaves one line in the log,
  the connection it opened. A line names a machine by its address and by its
  name in the host table, or `(?)` when the table does not have it.
- `--listen` sets where ozd answers. Without it, ozd listens on
  `127.0.0.1:42042`, which only this host can reach. Give an address on
  your network, or `0.0.0.0`, to let other hosts reach it.
- `--peer <addr>@<ip>` fixes the endpoint of a host that must not move,
  such as `cbridge`. ozd learns every other endpoint from the packets
  that hosts send.

The same flags, one per line, can also go in a file of flags. ozd reads
the file that `-c` names. Without `-c`, it reads the file that `OZD_RC`
names. Without either, it reads `.ozdrc` from the current directory, or
else from your home directory. It reads only the first of these that it
finds, and flags on the command line override the file (`DESIGN.md` §8).
`--check` reads the flags, checks the roots and exits without changing
anything. `--trace` prints every packet, and `--help` describes each
flag. ozd refuses to run as root, and it logs to stderr, one line per
event.

To check that a running ozd answers, run the example that comes with its
source, from this directory. It asks a host for STATUS, TIME and UPTIME
over CHUDP, as a Lisp Machine would, then lists a directory through FILE
and counts what is in it:

```sh
cargo run --example ask 3060@127.0.0.1:42042
```

```text
asking 3060 at 127.0.0.1:42042, from 3376
STATUS  MIT-OZ
TIME    2026-09-13T13:24:58Z (+0 s from this host)
UPTIME  0d 1h 6m 45s
FILE    / holds 2 directories and 1 file
```

The argument names ozd as `--peer` names a peer: its Chaos address, then
where it listens, by name or IP address, at port 42042 unless one is
given. The directory listed is `/`, unless an argument that begins with
`/` names another. The example logs in as `ASK`, and it sends from an
address of its own on the same subnet, host 376, which ozd learns as it
learns any machine's. If one of your machines has that address, give
another as an argument.

Each machine names ozd as its CHUDP peer. This example runs a machine
with [muir](https://github.com/metebalci/muir), a CADR simulator, on the
same computer as ozd:

```sh
muir --disk-pack /path/to/disk-sys-100-0.img \
     --chaos-address 3050 --chaos-udp 42043 \
     --chaos-udp-peer 3060@127.0.0.1:42042
```

Because both programs run on one computer, each needs its own UDP port.
ozd uses 42042, the machine uses 42043, and the machine reaches ozd at
`127.0.0.1:42042`. A second machine on the same computer would use the
address 3051 and the port 42044, and so on.

On separate computers, every program can use the default port, 42042,
but each must listen on its network address instead of the loopback.
For example, start ozd with `--listen 192.0.2.10`, and run the machine
on another computer like this:

```sh
muir --disk-pack /path/to/disk-sys-100-0.img \
     --chaos-address 3050 --chaos-udp 192.0.2.20 \
     --chaos-udp-peer 3060@192.0.2.10
```

Some CHUDP implementations send a packet only to a peer that is named
for its destination. A machine like that must also name every other
machine's address at ozd's endpoint, so that packets between the
machines pass through ozd (`DESIGN.md` §9).

ozd does no routing, so it cannot connect a site to the Global
Chaosnet. A site that wants that runs `cbridge`, the Chaosnet bridge, as
its switch instead of ozd. Each machine then names `cbridge` as its
default CHUDP peer, and ozd becomes one more peer of `cbridge`, on a port
of its own if both run on one computer. ozd still serves the machines,
but it passes no packets between them. Through `cbridge`, a test host has
reached ozd's STATUS, TIME, UPTIME and FILE; the Global Chaosnet has not
been tried yet (`DESIGN.md` §9).

## Security

ozd is meant for a trusted network segment, and it does not
authenticate anyone. Any host that can reach its socket gets an answer,
from FILE as well. Such a host can read every root, and it can write,
rename and delete files in every root that is not marked `,ro`. `LOGIN`
accepts any name and only records it; it is never a credential. So the
address that ozd listens on decides who can use it. Without `--listen`,
only this host can. With a network address or `0.0.0.0`, every host
that can reach that address can. ozd has no TLS and does no routing. At
a site that joins the Global Chaosnet through `cbridge`, every host that
`cbridge` lets through can reach ozd, FILE included.

Clients can reach the files in the roots and nothing else. A pathname
cannot climb out of its root. ozd follows a symbolic link only while the
link stays inside its own root. To serve a directory from elsewhere,
mount it with `--root <name>=<path>` instead of linking to it
(`DESIGN.md` §6).

Two risks cannot be caught by checking paths, so the operator must
prevent them:

- **The roots must belong to ozd's own user, and nobody else may write
  to them.** A hard link inside a root to a file outside it looks like
  an ordinary file in the root, and so does a bind mount of another
  directory. If only ozd's user can write to a root, neither can appear
  there.
- **Run only one ozd per root, and make each `,ro` root read-only on
  disk as well.** ozd checks a path and then opens it. This is safe only
  because nothing else writes to a writable root in between, and nothing
  writes to a read-only root at all.

ozd refuses to run as root. It needs no privileges, because 42042 is an
ordinary UDP port.

## As a service

`contrib/ozd.service` is a systemd unit, and
`contrib/com.metebalci.ozd.plist` is a launchd daemon. Both run ozd as a
user of its own, which owns the writable roots and nothing else, as the
Security section asks. The systemd unit is in use; the launchd daemon has
not been tried yet.

On Linux with systemd, `contrib/install-systemd.sh` does the whole
installation. Build ozd first, and then run the script as root from this
directory:

```sh
sudo contrib/install-systemd.sh
```

The script creates the `ozd` user and the base root `/srv/lispm`, which
that user owns. It installs the binary and the unit, and it writes a
standard file of flags to `/etc/ozdrc`: the System 100 site of the Run
section, with `/srv/lispm` as its base root. If `/etc/ozdrc` already
exists, the script keeps it. It then checks the flags and the roots as
the `ozd` user, and starts the service. With `--dry-run`, it prints what
it would do and changes nothing. So far it has been checked only with
`--dry-run`.

Edit `/etc/ozdrc` to suit your site, for example to mount the release's
sources at `tree` or to set `--listen`, and then run
`sudo systemctl restart ozd`.

To install by hand instead, create the user, give it the base root, and
install the binary, a file of flags and the unit:

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin ozd
sudo mkdir -p /srv/lispm
sudo chown ozd: /srv/lispm
sudo install -m 755 target/release/ozd /usr/local/bin/ozd
sudo install -m 644 ozdrc /etc/ozdrc
sudo install -m 644 contrib/ozd.service /etc/systemd/system/ozd.service
sudo systemctl daemon-reload
sudo systemctl enable --now ozd
```

Here `ozdrc` is your file of flags, one flag per line, such as the flags
in the Run section.

Either way, the unit lets ozd write only to `/srv/lispm`. If your
writable roots are elsewhere, list them in the unit's `ReadWritePaths=`
line. A root under `/home` also needs `ProtectHome=read-only` in place of
`ProtectHome=yes`. The log goes to the journal: `journalctl -u ozd`.

On macOS, create a hidden system user named `_ozd` that owns the
writable roots, for example with `dscl`. Install the binary as
`/usr/local/bin/ozd` and the file of flags as `/usr/local/etc/ozdrc`,
and copy the plist to `/Library/LaunchDaemons/`.

## Licence

ozd, the associated machine for a site of MIT CADR Lisp Machines.

Copyright (C) 2026 Mete Balci

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
The full text is in `LICENSE`.
