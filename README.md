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

ozd is built with Cargo, which [rustup](https://rustup.rs) installs:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then, in a new shell (or after `. "$HOME/.cargo/env"`), from this
directory:

```sh
cargo build --release
```

Rust 2024, on the toolchain `rust-toolchain.toml` pins, with no crate
dependencies. rustup reads that file and fetches the toolchain the first
time Cargo runs here --- or `rustup toolchain install` does, where
rustup's `auto-install` is set to `disable` --- and nothing else is
downloaded. A Cargo from a system's own packages does not read the file,
and may be too old for the 2024 edition.

## Run

For a System 100 band, whose host table has its machine as MIT-LISPM-1
at 3050 and its file and time host as MIT-OZ at 3060
(`sys/site/hosts.text`):

```sh
target/release/ozd --address 3060 --name MIT-OZ,OZ,system=UNIX \
    --root /srv/lispm --root tree=/path/to/system-100-0/sys,ro \
    --host 3050,MIT-LISPM-1,CADR-1,CADR1,LM1,system=LISPM
```

- `--address` and `--name` are this host as the band's table has it: its
  address, its names with the official first, and its system type.
- `--root /srv/lispm` is the base, the homes, which ozd writes; `tree=`
  mounts the release's sources at `/tree`, read-only, where the band asks
  for them (`sys/site/sys.translations`). Each must be a directory when
  ozd starts.
- A site may have mounts and no base: `/` is then read-only and names
  only the mounts. `LOGIN` gives each user a home, `/<user>/` in lower
  case, and with no base there is none --- unless a writable mount has
  that name: `--root lispm=/srv/lispm` is user LISPM's home.
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

```sh
muir --disk-pack /path/to/disk-sys-100-0.img \
     --chaos-address 3050 --chaos-udp 42043 \
     --chaos-udp-peer 3060@127.0.0.1:42042
```

A second machine takes 3051 on 42044, and so on. A machine whose CHUDP
sends a packet only to a peer named for its destination also names every
other machine's address at ozd's endpoint, so that their packets to each
other come through it (`DESIGN.md` §9).

## Trying it

The acceptance test is by hand (`DESIGN.md` §11): ozd run as above, its
`tree` mount the System 100 release's `sys` directory and its base an
empty directory it may write, and beside it two machines, each with a
pack of its own --- two muir runs, say:

```sh
muir --disk-pack /path/to/pack-1.img --chaos-address 3050 \
     --chaos-udp 42043 --chaos-udp-peer 3060@127.0.0.1:42042 \
     --chaos-udp-peer 3051@127.0.0.1:42042
muir --disk-pack /path/to/pack-2.img --chaos-address 3051 \
     --chaos-udp 42044 --chaos-udp-peer 3060@127.0.0.1:42042 \
     --chaos-udp-peer 3050@127.0.0.1:42042
```

Each should boot knowing the date, read its sources from `/tree/` and
write in the base, print its uptime right with `(uptime)`, and show
ozd and the other machine with `(hostat)`. The second boots as a
machine its band has no name for, since System 100's table names only
3050 (`DESIGN.md` §9).

## Security

ozd is for a trusted segment, and it authenticates nobody. Whatever can
reach its socket is answered, FILE included, and may read every root,
and write, rename and delete in every root without `,ro`. `LOGIN` takes
any name and records it; it is never a credential. So where it listens
is the whole of who may use it: without `--listen`, this host alone;
with an address, or `0.0.0.0`, everything that can reach that. There is
no TLS and no routing: a site that wants the Global Chaosnet puts
`cbridge` beside it.

What a client can name is the roots and nothing else. A pathname cannot
climb out of one, a symbolic link is followed only while it stays in its
own root, and a directory elsewhere comes in as a mount, never by a
link (`DESIGN.md` §6). Two things no check of a path can see are the
operator's to keep:

- **The roots belong to ozd's own user, and nobody else writes into
  them.** A hard link in a root to a file outside it looks, to any path
  check, like a file in the root, and so does a bind mount of one root's
  directory inside another; a root only ozd's user can write holds
  neither.
- **One ozd to a root, and a `,ro` root read-only on disk as well.**
  Checking a path and then opening it is safe because nothing else
  writes in a writable root between the two, and nothing at all writes
  in a read-only one.

It refuses to run as root, and needs no privilege: 42042 is an ordinary
UDP port.

## As a service

`contrib/ozd.service` is a systemd unit, and
`contrib/com.metebalci.ozd.plist` a launchd daemon, each running
ozd as a user of its own that owns the writable roots and nothing
else, as the section above asks. Neither has yet been tried on the
system it is for.

## Licence

AGPL-3.0-or-later.
