# ozd

OZ, MIT-OZ, was the host MIT's Lisp Machines kept their system files on
and asked for the time and their host table (System 100's
`sys/site/site.lisp`); ozd is OZ, as a daemon.

The associated machine for a site of MIT CADR Lisp Machines: one host on
one Chaosnet subnet, over UDP, serving the machines their files, their
time and their host table, and passing packets between them. Several
machines share it, each naming it as its CHUDP peer --- as MIT's did,
one associated machine for many Lisp Machines.

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

For a System 100 band, whose host table has its machine as MIT-LISPM-1
at 3050 and its file and time host as MIT-OZ at 3060
(`sys/site/hosts.text`):

    target/release/ozd --address 3060 --name MIT-OZ,OZ,system=UNIX \
        --root /srv/lispm --root tree=/path/to/system-100-0/sys,ro \
        --host 3050,MIT-LISPM-1,CADR-1,CADR1,LM1,system=LISPM

- `--address` and `--name` are this host as the band's table has it: its
  address, its names with the official first, and its system type.
- `--root /srv/lispm` is the base, the homes, which ozd writes; `tree=`
  mounts the release's sources at `/tree`, read-only, where the band asks
  for them (`sys/site/sys.translations`). Each must be a directory when
  ozd starts.
- `--host` is a machine HOSTAB names for a band whose own table lacks
  it: its address, its names, its system type. Another machine takes an
  address, and a `--host`, of its own.
- `--listen` is where it answers: without it `127.0.0.1:42042`, this
  host alone; an address on the segment, or `0.0.0.0`, for others.
- `--peer <addr>@<ip>` fixes an endpoint that must not move, cbridge's
  for one; every other is learned from the packets a host sends.

The same flags, one a line, make a file of flags --- the one `-c` names,
else the one `OZD_RC` names, else `.ozdrc` in the directory it is run
from or in the home directory --- and the command line has the last
word (`DESIGN.md` §8). `--check` reads them and checks the roots,
changing nothing, then exits; `--trace` prints every packet; `--help`
says what each flag is. It will not run as root, and it logs to stderr,
one line an event.

A machine names ozd as its CHUDP peer; with
[muir](https://github.com/metebalci/muir), a CADR simulator, for
example:

    muir --disk-pack /path/to/disk-sys-100-0.img \
         --chaos-address 3050 --chaos-udp 42043 \
         --chaos-udp-peer 3060@127.0.0.1:42042

A second machine takes 3051 on 42044, and so on. A machine whose CHUDP
sends a packet only to a peer named for its destination also names every
other machine's address at ozd's endpoint, so that their packets to each
other come through it (`DESIGN.md` §9).

## Trying it

The acceptance test is by hand (`DESIGN.md` §11): ozd run as above, its
`tree` mount the System 100 release's `sys` directory and its base an
empty directory it may write, and beside it two machines, each with a
pack of its own --- two muir runs, say:

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
