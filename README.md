# muir-ah

The associated machine for a site of MIT CADR Lisp Machines: one host on
one Chaosnet subnet, over UDP, serving the machines their files, their
time and their host table, and passing packets between them.

It runs beside [muir](https://github.com/metebalci/muir), the CADR
simulator, whose Chaosnet services it takes over, so that several
machines can share one host --- as MIT's did, one associated machine for
many Lisp Machines.

**Under construction.** STATUS, TIME, UPTIME, HOSTAB and NAME are
served, and the hub passes packets between the machines; FILE is being
written. The design is `DESIGN.md`, and every protocol a Lisp Machine
speaks is recorded in `PROTOCOLS.md`, with where each fact was read.

## Build

    cargo build --release

Rust 2024, on the toolchain `rust-toolchain.toml` pins, with no crate
dependencies: nothing else is downloaded.

## Run

    target/release/muir-ah examples/system-100.conf

The one argument is the site's config (`DESIGN.md` §8). `--check` reads
it and exits; `--trace` prints every packet. It will not run as root, and
it logs to stderr, one line an event.

The two examples are for System 100 and System 304, with each band's own
numbers; edit their paths before using one. Without a `listen` line it
answers on `127.0.0.1:42042`, which is enough for muir runs on the same
host:

    muir --disk-pack /path/to/muir/vendor/run/disk-sys-100-0.img \
         --chaos-address 3050 --chaos-udp 42043 \
         --chaos-udp-peer 3060@127.0.0.1:42042

A second machine takes 3051 on 42044, and so on. Until muir has a
default peer, each run also names every other machine's address at
muir-ah's endpoint, so that their packets to each other come through it
(`DESIGN.md` §9).

## As a service

`contrib/muir-ah.service` is a systemd unit, and
`contrib/com.metebalci.muir-ah.plist` a launchd daemon, each running
muir-ah as a user of its own that owns the writable root and nothing
else. Neither has yet been tried on the system it is for.

## Licence

AGPL-3.0-or-later.
