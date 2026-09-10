# ozd

OZ, MIT-OZ, was the host MIT's Lisp Machines kept their system files on
and asked for the time and their host table (System 100's
`sys/site/site.lisp`); ozd is OZ, as a daemon.

The associated machine for a site of MIT CADR Lisp Machines: one host on
one Chaosnet subnet, over UDP, serving the machines their files, their
time and their host table, and passing packets between them.

It runs beside [muir](https://github.com/metebalci/muir), the CADR
simulator, whose Chaosnet services it takes over, so that several
machines can share one host --- as MIT's did, one associated machine for
many Lisp Machines.

**Under construction.** STATUS, TIME, UPTIME, FILE, HOSTAB and NAME
are served, and the hub passes packets between the machines; none of it
has yet met a real band, which is what the acceptance test below is
for. The design is `DESIGN.md`, and every protocol a Lisp Machine
speaks is recorded in `PROTOCOLS.md`, with where each fact was read.

## Build

    cargo build --release

Rust 2024, on the toolchain `rust-toolchain.toml` pins, with no crate
dependencies: nothing else is downloaded.

## Run

    target/release/ozd -c examples/system-100.ozdrc

Everything is a flag, as muir's are (`DESIGN.md` §8): `--address`,
`--name`, `--listen`, `--root`, `--host` and `--peer`. Their defaults
come from a file of flags --- the one `-c` names, else the one
`OZD_RC` names, else `.ozdrc` in the directory it is run from or
in the home directory --- and the command line has the last word.
`--check` reads them and checks the roots, changing nothing, then exits;
`--trace` prints every packet; `--help` says what each flag is. It will
not run as root, and it logs to stderr, one line an event.

The two examples, `examples/system-100.ozdrc` and
`examples/system-304.ozdrc`, are for System 100 and System 304, with
each band's own numbers; edit their paths before using one. Without
`--listen` it
answers on `127.0.0.1:42042`, which is enough for muir runs on the same
host:

    muir --disk-pack /path/to/muir/vendor/run/disk-sys-100-0.img \
         --chaos-address 3050 --chaos-udp 42043 \
         --chaos-udp-peer 3060@127.0.0.1:42042

A second machine takes 3051 on 42044, and so on. Until muir has a
default peer, each run also names every other machine's address at
ozd's endpoint, so that their packets to each other come through it
(`DESIGN.md` §9).

## Trying it with muir

The acceptance test is by hand (`DESIGN.md` §11). With the System 100
release in muir's `vendor/`, and `examples/system-100.ozdrc` edited so
that its `tree` mount is that release's `sys` directory and its base an
empty directory ozd may write:

    target/release/ozd -c examples/system-100.ozdrc

and beside it two muir runs, each with a pack of its own:

    muir --disk-pack /path/to/pack-1.img --chaos-address 3050 \
         --chaos-udp 42043 --chaos-udp-peer 3060@127.0.0.1:42042 \
         --chaos-udp-peer 3051@127.0.0.1:42042
    muir --disk-pack /path/to/pack-2.img --chaos-address 3051 \
         --chaos-udp 42044 --chaos-udp-peer 3060@127.0.0.1:42042 \
         --chaos-udp-peer 3050@127.0.0.1:42042

Each should boot knowing the date, read its sources from `/tree/` and
write in the base, print its uptime right with `(uptime)`, and show
ozd and the other machine with `(hostat)`. The second boots as a
machine its band has no name for, since System 100's table names only
3050 (`DESIGN.md` §9).

## As a service

`contrib/ozd.service` is a systemd unit, and
`contrib/com.metebalci.ozd.plist` a launchd daemon, each running
ozd as a user of its own that owns the writable root and nothing
else. Neither has yet been tried on the system it is for.

## Licence

AGPL-3.0-or-later.
