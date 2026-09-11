# ozd

OZ, also called MIT-OZ, was the host where MIT's Lisp Machines kept
their system files. They also asked it for the time and for their host
table (see System 100's `sys/site/site.lisp`). ozd is OZ as a daemon.

ozd is the associated machine for a site of MIT CADR Lisp Machines. It
is one host on one Chaosnet subnet, reached over UDP. It serves the
machines their files, the time and their host table, and it passes
packets between them. Several machines can share one ozd, each naming it
as its CHUDP peer, just as MIT had one associated machine for many Lisp
Machines.

ozd serves STATUS, TIME, UPTIME, FILE, HOSTAB and NAME, and it passes
packets between the machines on its subnet. None of this has been tried
against a real band yet; the acceptance test below is for that. The
design is in `DESIGN.md`. Every protocol a Lisp Machine speaks is
described in `PROTOCOLS.md`, together with where each fact comes from.

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

Each machine names ozd as its CHUDP peer. With
[muir](https://github.com/metebalci/muir), a CADR simulator, that looks
like this:

```sh
muir --disk-pack /path/to/disk-sys-100-0.img \
     --chaos-address 3050 --chaos-udp 42043 \
     --chaos-udp-peer 3060@127.0.0.1:42042
```

A second machine uses 3051 and port 42044, and so on. Some CHUDP
implementations send a packet only to a peer that is named for its
destination. A machine like that must also name every other machine's
address at ozd's endpoint, so that packets between the machines pass
through ozd (`DESIGN.md` §9).

## Trying it

The acceptance test is done by hand (`DESIGN.md` §11). Run ozd as
above, with its `tree` mount pointing at the System 100 release's `sys`
directory and its base root at an empty directory that ozd can write.
Next to it, run two machines, each with a pack of its own. With muir:

```sh
muir --disk-pack /path/to/pack-1.img --chaos-address 3050 \
     --chaos-udp 42043 --chaos-udp-peer 3060@127.0.0.1:42042 \
     --chaos-udp-peer 3051@127.0.0.1:42042
muir --disk-pack /path/to/pack-2.img --chaos-address 3051 \
     --chaos-udp 42044 --chaos-udp-peer 3060@127.0.0.1:42042 \
     --chaos-udp-peer 3050@127.0.0.1:42042
```

Each machine should boot and know the date, read its sources from
`/tree/`, and write to the base root. `(uptime)` should print the right
uptime, and `(hostat)` should show ozd and the other machine. The second
machine boots without a name, because System 100's host table lists only
3050 (`DESIGN.md` §9).

## Security

ozd is meant for a trusted network segment, and it does not
authenticate anyone. Any host that can reach its socket gets an answer,
from FILE as well. Such a host can read every root, and it can write,
rename and delete files in every root that is not marked `,ro`. `LOGIN`
accepts any name and only records it; it is never a credential. So the
address that ozd listens on decides who can use it. Without `--listen`,
only this host can. With a network address or `0.0.0.0`, every host
that can reach that address can. ozd has no TLS and does no routing. A
site that wants to join the Global Chaosnet runs `cbridge` next to it.

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
`contrib/com.metebalci.ozd.plist` is a launchd daemon. Both run ozd as
its own user, which owns the writable roots and nothing else, as the
Security section asks. Neither has been tried on its system yet.

## Licence

AGPL-3.0-or-later.
