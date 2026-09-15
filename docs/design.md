# ozd: design

This document describes how ozd works. Where the code and this document
disagree, one of them is wrong. `docs/protocols.md` describes the protocols
themselves, and the README says how to build, run and secure ozd.

## 1. The shape of it

ozd is one process with one thread, one UDP socket and one loop. It is
the **switch** of one subnet. The hosts on the subnet name it as their
CHUDP peer, and it passes each packet to the host that the packet's
trailer names, at the endpoint learned from that host's own packets, as
an Ethernet switch does. It also answers its own services. A site that
wants the Global Chaosnet makes `cbridge` its switch instead, and ozd is
then one of its peers (§9).

```text
hosts on the subnet: Lisp Machines
    │  CHUDP over UDP
chudp::Link       the socket, the endpoints, the checks;
    │             a packet for another host passed on, untouched
    │  Framed, for this host
Ncp               AIM-628 chapters 3 and 4
    │  Service / Session
services          STATUS  TIME  UPTIME  FILE  ·  HOSTAB  NAME
```

Nothing blocks except the socket read, and nothing runs except the loop.
This is what lets containment check a path and then open it without
`openat` (§6). It is also why a second thread would be a change of
design rather than an optimisation.

## 2. Build

- **Edition.** ozd uses Rust 2024 (`edition = "2024"`).
- **Toolchain.** `rust-toolchain.toml` pins the stable 1.98.0 toolchain,
  with `clippy` and `rustfmt`. The release profile is Cargo's default.
  The pin moves forward with stable releases.
- **No dependencies.** `[dependencies]` is empty. The standard library
  provides the socket, both clocks, the filesystem, the test harness and
  the command line, and ozd parses its flags and its file of flags by
  hand. In two places a crate would be the natural answer, and ozd does
  without it:
  - **`openat` with `O_NOFOLLOW`** would make containment hold even
    against a concurrent writer. The standard library has it only as a
    private helper (`openat_nofollow_dironly` in `sys/fs/unix.rs`, as of
    1.92), and its constants differ between platforms, so hand-written
    FFI would mean guessing at numbers. With one thread and one writer it
    is not needed (§6). A second thread would make `libc` the one
    dependency.
  - **Signals.** The standard library has no signal handler. The daemon
    has nothing to flush, so the default action of `SIGTERM` is its
    shutdown (§10).
- **One `unsafe`.** `geteuid` is declared `extern "C"`, from the C
  library that the standard library already links, so that the daemon
  can refuse to run as root. `Cargo.toml` denies `unsafe_code` for the
  whole package, tests included, and only that one function allows it.
- **Library and binary.** `src/lib.rs` is the daemon, so that tests can
  drive it in-process. `src/main.rs` handles the command line and runs
  the loop.
- **Lints.** `cargo clippy --all-targets -- -D warnings` is clean.
  `cargo doc --no-deps` runs with `RUSTDOCFLAGS="-D warnings"`, which
  catches a doc link that leads nowhere or to a private item.
- **rustfmt.** `use_small_heuristics = "Max"` keeps a call or a struct
  literal on one line whenever it fits.
- **Licence.** ozd is AGPL-3.0-or-later, with SPDX headers.
- **`Cargo.lock`** is committed, because ozd is a binary.

## 3. Modules

```text
src/
  lib.rs            the modules
  main.rs           arguments, startup, the loop
  config.rs         the flags and the file of them, checked (§8)
  hosts_text.rs     a band's own host table, read as HOSTAB's (§8)
  daemon.rs         the daemon: the link, the NCP, the services, turn (§4)
  log.rs            one line an event on stderr, in UTC, hosts named (§10)
  address.rs        parse_address
  packet.rs         the packet, and the CADR's own check word
  roots.rs          the tree FILE serves: roots, resolve, readonly (§6)
  chudp.rs          the frame, and the link and switch (§5)
  ncp.rs            the NCP
  lispm.rs          the Lisp Machine character set
  service/
    status.rs  time.rs  file.rs  hostab.rs  name.rs
examples/
  ask.rs            asks a host for STATUS, TIME and UPTIME over CHUDP
```

**`ncp`** is Chaosnet's connection layer, the equivalent of TCP. It
opens and closes connections (RFC, OPN, CLS), numbers and acknowledges
packets, sends again what is lost, carries EOF, and hands each request
to the service registered for its contact name. MIT called this layer
the Network Control Program, and the Lisp Machine's own is
`sys/network/chaos/chsncp.lisp`. Each service module is named after its
contact name.

A BRD for a contact that no service takes is dropped silently rather
than refused with a CLS. The machine's own NCP does the same
(`RECEIVE-BRD`, `chsncp.lisp:1613`, with `CLS-ON-ERROR-P` nil, `:1588`).
A switch sees every broadcast on the subnet, so it must not answer the
ones it does not serve (`tests/ncp.rs`).

**A connection's index** has two parts. Its low ten bits are a slot in
the connection table, and the six bits above them are that slot's
uniquizer, which goes up by one each time the slot is given out. Slots
are taken in turn around the table. The machine's own NCP works the same
way with 128 slots (`chsncp.lisp`, `INDEX-CONN-FREE-POINTER` and
`UNIQUIZER-TABLE`). So an index is not given out again until the table
has gone round and the slot's uniquizer has changed. A late packet for a
connection that has closed, such as a CLS that crossed this end's own,
then finds no connection instead of the next one. When every slot is
taken, an RFC is refused, and a connection from this end is not made.

**A restart does not give out the last run's indexes.** A band that still
holds a connection from this host's last run, closing it, discards an RFC
from the same index as a duplicate (AIM-628 §4.1), and FILE's data
connections are this host's RFCs. The machine's own NCP met this on reload
and seeds every slot's uniquizer from the clock at reset (`chsncp.lisp`,
`RESET`: "it is exactly what happens with the file job connection!!").
`Ncp::new` does the same from the system clock, and starts its search for
a free slot at a slot the clock gives as well, so two runs give out the
same first index only one time in 65,472.

## 4. The loop

```text
start   config → startup checks (§6) → bind → Ncp with its services
        → stale temporaries removed (§6)
loop    now ← nanoseconds since start, from Instant
        wait for one datagram, at most 100 ms → the link (§5)
        while Ncp::transmit(now) gives a buffer → Link::send(buffer)
```

**Why it wakes when nothing arrives.** Two jobs are due even on a
silent network. A packet that the other end has not acknowledged is sent
again every half second (`RETRANSMIT_NS`), and a connection that has
been silent for three minutes is given up (`HOST_DOWN_NS`). The NCP has
no timer of its own. It does both jobs whenever it is asked for output:
`transmit` runs `service_all` when its queue is empty. So the loop waits
for a packet for at most 100 ms, which is the socket's read timeout. A
packet that arrives is handled at once. If none arrives, the loop wakes
anyway, asks for output and waits again. An idle daemon wakes ten times
a second, which costs nothing measurable, and a retransmission is at
most 100 ms late against its 500 ms interval.

- **`now` is the time since start, from `Instant`, in nanoseconds.** The
  NCP's timeouts are already in nanoseconds, so they work as written.
- **The loop body is `Daemon::turn(now, wait)`.** `main` calls it for
  ever with the real clock. The tests call it with a clock they set, so
  a test of UPTIME or of a retransmission is exact rather than timed.
- **One packet is in flight per connection.** The far end is a CADR
  interface that holds one packet, and a burst would be lost in it. A
  file therefore moves at the rate at which the other end acknowledges
  packets.

## 5. The link and the switch

`chudp::Link` owns the socket and the table of endpoints, and it is the
switch.

- **It binds what `--listen` says**: a port, an address, or an address
  and a port. A bare port is on the loopback. Without `--listen`, it
  binds `127.0.0.1:42042`, so a fresh install answers its own host and
  nothing else. An address without a port uses 42042. `0.0.0.0`, or
  `::`, means every interface.
- **Endpoints are learned.** A packet's source address, from its
  header, is recorded against the UDP address that the datagram came
  from. A host behind `cbridge` is learned at `cbridge`'s endpoint,
  which is correct, because that is where its packets go. A `--peer`
  fixes an endpoint, and a packet does not move a fixed one. The table
  does not expire, and it holds at most one entry per address.
- **Receiving** works in this order. Each drop is counted for STATUS and
  printed under `--trace`.
  1. `unwrap` checks the version, the function and the length.
  2. A datagram whose trailer gives this host's address, or 0, as its
     source is dropped, since it claims to come from this station.
  3. The source is learned, unless it is fixed or is this host. This
     happens before anything else, so that an answer to it can be passed
     on at once.
  4. The destination decides the rest:
     - **this host**: the packet goes to `Ncp::receive`.
     - **0**, a broadcast: the packet goes to `Ncp::receive`, and it is
       passed on to every endpoint except the one it came from.
     - **another host of this subnet with a known endpoint**: the packet
       is passed on to that endpoint, unless that is where it came from.
     - **anything else**, such as another subnet or a host not yet heard
       from: the packet is dropped.
- **A packet passed on is not changed.** The datagram goes out byte for
  byte as it came in. A cable does not change a frame, and neither does
  the switch: there is no forwarding count and no new trailer. This is
  what makes ozd a switch rather than a bridge, and it is why nothing
  goes to another subnet: there is no routing to decide where.
- **The frame is `cbridge`'s.** CHUDP is `cbridge`'s convention, and ozd
  follows it: every 16-bit word in network order, the trailer's too, and
  the trailer's check word an Internet checksum over the words, the
  trailer's destination and source among them (`chudp.rs`). The data is
  packed low byte first within its words, as AIM-628 §3.6 has it, so in
  network order each pair of data bytes goes out swapped. A running
  `cbridge` showed all of it, and two frames it sent are pinned in
  `tests/frame.rs`. The protocol page says `cbridge` sends least
  significant byte first, and the running `cbridge` does not.
  `docs/chudp.md` describes the framing in full, with examples.
- **The trailer's check word** on a packet for this host is compared with
  the checksum, and a mismatch is traced but never dropped: UDP has a
  checksum of its own, although over IPv4 a sender may leave it zero, and
  a peer that puts something else there is still understood.
  `Ncp::receive` does not drop a packet for its check word either.
- **Sending this host's own packets.** The NCP's buffer ends with the
  destination. It goes out as `wrap(buffer, own address,
  checksum(buffer + own address))`. It goes to the destination's
  endpoint, or once
  to every distinct endpoint for a destination of 0. A packet for a
  destination with no endpoint is dropped and counted.
- **A machine names this host as its CHUDP peer.** A machine whose CHUDP
  sends a unicast packet only to a peer named for its destination must
  also name every other machine of the subnet at this host's endpoint.
  Otherwise its packets for them never reach the switch (§9).

## 6. Containment

FILE serves anyone who can reach the socket, so what it lets a client
name is all of its security.

**At startup**, before the socket is bound, ozd refuses to start, and
says why, if any of these is true:

- `geteuid()` is 0.
- A root is not an absolute path, cannot be canonicalised, is not a
  directory, or is `/`.
- A root without `,ro` is not writable. ozd checks this by creating a
  probe file in the root and removing it.

Two more things happen that are not refusals. Entries of the base root
that a mount covers are **warned about** by name, because they exist but
cannot be reached. And once the socket is bound, stale FILE temporaries
are removed from each writable root. A daemon killed in the middle of a
write leaves one behind. Binding first means that a second daemon given
the same endpoint stops before it touches a root. A temporary's name
comes from one constant, so the cleanup matches only what FILE makes
(`roots.rs`, `temporary_name`) and nothing else.

**Roots.** FILE serves one tree. It is made of a **base root** and,
optionally, **named roots mounted at its top level**. Each root is
either read-only (`,ro`) or not.

```text
--root /srv/lispm                         # the base: homes, and the rest
--root tree=/path/to/system-100-0/sys,ro  # mounted at /tree, read-only
```

Here `/tree/...` is the second directory, and it is read-only.
Everything else under `/` is in the base, including `/<user>/`, the home
that LOGIN gives a user.

- **A mount covers the base.** If the base has a directory named
  `tree`, a mount named `tree` wins, just as a Unix mount point covers
  the directory under it. The base's `tree` cannot be reached, and ozd
  says so at startup.
- **A listing of `/`** shows the base's entries and the mounts' names,
  each name once. Nothing can be created at `/` under a mount's name.
- **Names match exactly**, as directory names do. FILE folds case only in
  LOGIN's home, never in a pathname, and a band sends its pathnames in
  lower case (`/tree/...`, System 100's `sys/site/sys.translations`). So
  mounts are named in lower case.
- **With mounts and no base**, `/` itself is read-only and lists only
  the mounts. A single `--root` without a name is a single root.

**`resolve(pathname)`** works in four steps:

1. It splits the pathname on `/` and drops empty components. A `.` or
   `..` component is refused with `ATD`, not normalised, so no path
   climbs out of a root or from one root into another.
2. If the first component names a mount, the rest of the path is under
   that root. Otherwise the whole path is under the base.
3. It canonicalises the deepest part that exists, following every
   symlink on the way, and the result must be that root or lie under
   it. A symlink anywhere is followed, and one that resolves outside its
   own root is refused with `ATD` when it is used. **There is no second
   tree by symlink**: a link at a root's top level does not bring its
   target into the tree. A directory elsewhere is served by mounting it.
4. It returns the canonical existing part, with the rest of the path,
   which does not exist yet, joined back on. That path is what gets
   opened, never the string the client sent.

`resolve_for_writing` also refuses a root itself, and anything in a
read-only root (below).

**Why checking and then opening is safe here.** The path that is
checked is the path that is opened, and it contains no symlink when it
is checked. Nothing else runs between the two, because the loop is the
only thread and this process is the only writer of a writable root,
while nobody writes to a read-only root at all. The operator keeps the
second condition, as the README's Security section says. The code notes
both conditions where it relies on them.

**Opening.** FILE reads the type of what is at a path with
`symlink_metadata` first. Anything other than a regular file or a
directory is refused with `WKF`, "wrong kind of file", which the band
turns into its `WRONG-KIND-OF-FILE` condition
(`sys/io/file/open.lisp:260`). Opening a FIFO would block the loop.

**A read-only root**, marked `,ro`, refuses OPEN for output, DELETE,
RENAME, CREATE-DIRECTORY, CREATE-LINK and CHANGE-PROPERTIES with `ATF`,
"Access to file denied", before anything is touched. The band turns
`ATF` into `INCORRECT-ACCESS-TO-FILE` (`io/file/open.lisp:224`), its
usual condition for it. FILE has no error code of its own for a refused
write (`docs/protocols.md`, FILE). A RENAME from one root into another is
refused the same way, because it would be a copy rather than a rename.

**Writes.** The temporary is made in the target's own directory, so the
same resolution keeps it inside its root, and at CLOSE it is renamed
over the target. If two machines write the same file, the last rename
wins as a whole, and no file is ever half one and half the other.

**CREATE-LINK** resolves both of its ends.

**More that the tree refuses**, each with a test in
`tests/containment.rs`:

- **Roots may not overlap**, and startup refuses it. Otherwise a
  read-only mount inside a writable base could be written through a
  link made in the base, because the link resolves under the base.
- **A temporary's name is refused in every pathname**, in any ASCII
  case, with `ATD`. A filesystem that folds case would take `#OZD-...#`
  for a temporary's name. So no client can delete a temporary mid-write
  and put a link in its place, and the cleanup, which matches the name
  exactly, removes only what this daemon made.
- **A link that does not resolve**, because it dangles or loops, is
  refused. Creating a file through a dangling link would create its
  target, wherever that is.
- **No roots, or a bad mount name, stop the daemon from starting.** The
  writability probe is named like a temporary, so a probe that a crash
  leaves behind is removed at the next start.

**FILE's rules**:

- A link is followed to read or to write through it, but **DELETE and
  RENAME act on the link itself**: its directory is resolved, and its
  own name is not followed, as on Unix. Removing a link that does not
  resolve is how such a link is got rid of.
- **DELETE on a handle**, a "delete while open", deletes the file being
  read or written at once, as `FILE.c` does. For a write, it deletes the
  temporary, and the CLOSE then puts nothing in place and is answered as
  usual. For a read, it deletes the file through the tree, just as a
  DELETE with a pathname does, so a read-only root refuses it with `ATF`.
  A DELETE with both a handle and a pathname, on a handle with no
  transfer, or on a directory listing is refused with `BUG`, as in
  `FILE.c`.
- **A DATA-CONNECTION whose connection closes before it opens**, because
  the client refused it or it was given up, is answered with `NET`,
  "Data connection could not be established", as `FILE.c` answers it,
  and its two handles are not kept. `NET` is `FILE.c`'s own code. It
  appears neither in `chfile.text`'s table nor in the band's
  `io/file/open.lisp`, so what a band makes of it is **unverified**.
- **DIRECTORY reads each entry with `symlink_metadata`, or through
  `resolve`, and never with `metadata`**, which follows links and would
  give the size and date of a file outside the root.
- **A write's temporary is created once**, with `create_new`, and
  written through the handle it was made with. It is never reopened by
  name.
- `/` itself resolves to the top of the tree, not to a path. PROBE,
  PROPERTIES and DIRECTORY of `/` read it through `Tree::list_top`.
- **A listing** describes a link inside its own root as whatever it
  leads to. It describes a link that the tree refuses, because it leads
  out of its root, into another root or nowhere, by the link's own size
  and date, so that the link can be seen and deleted. A temporary is
  listed nowhere, since no pathname can name one.
- **`/`** answers PROBE and PROPERTIES as a directory of length 0, dated
  now. OPEN of `/` for reading is `FNF`, and a link to `/` is refused.

## 7. Services

ozd serves six protocols (`docs/protocols.md`).

- **STATUS** answers with the official name and one block for this
  host's subnet. Its meters are counted at the socket and shared with
  the service through an `Arc` of atomics, since `Service` is `Send`.
  Meter 1 counts every datagram received, and meter 2 every datagram
  sent, both this host's own and those passed on. Meter 7 counts those
  rejected for their length, and meter 8 those rejected for anything
  else. Meters 3 to 6 are zero, and that is true: there is no interface
  here to abort, to lose a packet in, or to fail a CRC.
- **TIME** answers with the universal time from the system clock, least
  significant byte first.
- **UPTIME** answers with the sixtieths of a second since start,
  `now × 60 / 10⁹`, which wraps at 32 bits after about 828 days.
- **FILE** follows `sys/doc/chfile.text`, with the containment of §6.
- **HOSTAB** looks up each line that the client sends, ignoring case,
  among this host's names, every `--host`'s names, and the names of every
  host of the table `--hosts-text` names (§8). For a match, it
  answers with one `NAME` line per name, the official name first, the
  `CHAOS` address in octal, and a `SYSTEM-TYPE` line if the host's flag
  gives one (for this host, the one on `--name`), and then an EOF. For no
  match, it answers `ERROR No such host` and then an EOF. It never sends
  `MACHINE-TYPE` (`docs/protocols.md`, HOSTAB). The connection stays open for
  the next name until the client closes it. A line longer than any name
  matches nothing, and no more of it than that is kept.
- **NAME** answers with one line, `Nobody is logged in.`, ended with the
  Lisp Machine's newline. It then sends an EOF, and a CLS once the EOF
  is acknowledged, which is how the machine's own server ends
  (`FORMAT-AND-EOF`, `chuse.lisp:550`). A band's `(finger)` asks here
  when it is given no host (`docs/protocols.md`, NAME).

## 8. The flags, and the file of them

Every setting is a flag, and a file of flags can give the defaults.
Addresses are written in octal, or as `subnet:host`. A value is one
word, with its parts separated by commas (`--root /srv/lispm,ro`). As a
file of flags:

```text
# this host's Chaos address; required
--address 3060
# its names, the official first, and its own system type; required
--name MIT-OZ,OZ,system=UNIX
# where it listens; see §5
--listen 192.0.2.10
# the base root, and a root mounted at /tree, read-only
--root /srv/lispm
--root tree=/path/to/system-100-0/sys,ro
# the site's host table, for HOSTAB
--host 3050,MIT-LISPM-1,LM1,system=LISPM
# the band's own host table, whose hosts HOSTAB answers for as well
--hosts-text /srv/lispm/sys/site/hosts.text
# an endpoint that is fixed; the rest are learned
--peer 3040@192.0.2.5
```

`--address` and `--name` are required, and each may be given only once,
as may `--listen` and `--hosts-text`. `--root`, `--host` and `--peer` may
each be given more than once. A `--root` whose value begins with `/` is
the base. Any other `--root` is `<name>=<path>`, mounted at `/<name>`.
`,ro` makes either kind read-only. `--peer` is `<address>@<ip>[:<port>]`,
with port 42042 unless one is given. A path cannot contain a comma.

The host table and the endpoints are separate. A `--host` is what HOSTAB
tells, and a `--peer` is where packets go. A machine needs neither to be
served, but it needs a `--host` to be found by name.

**The band's own host table.** `--hosts-text <file>` names
`sys/site/hosts.text`, the file a site already keeps for its machines, and
HOSTAB answers for the hosts in it as well. Its `HOST` lines are read as the
band's own generator reads them (`GENERATE-HOST-TABLE-2`,
`sys/network/chaos/chsaux.lisp`): the official name, the first Chaosnet
address in octal, the system type, and the nicknames in brackets. A `NET`
line, a comment and a blank are skipped, and so is a host with no Chaosnet
address, such as the ARPANET entries of `sys/site/extra.hosts`. An address
that is not one is refused, naming its line. This host's own line is passed
over, because its names are `--name`'s, so a site can name the table it
keeps without editing itself out of it. The table's hosts come before the
`--host`s, and each name and each address is still one host's. The file is
read at startup, with the roots, so a change to it wants a restart.

**The file of flags, `.ozdrc`.** `-c|--config <file>` names the file,
which must then exist. Without it, ozd uses the file that `OZD_RC`
names, then `.ozdrc` in the directory it is run from, then `.ozdrc` in
the home directory. It reads only the first of these that exists. Each
line is a flag, and after a space, the rest of the line is its value. A
blank line, or a line that begins with `#`, is a comment. A flag given
on the command line leaves that flag's lines in the file unread, so the
command line has the last word. A file cannot name another file.

`#` begins a comment only at the start of a line, so a path in a file
may contain a blank or a `#`. The file is read first and the command
line after it. `--trace` may be in a file, but `--check`, `--help` and
`--config` may not. Each of those is something one run is asked to do,
and in a file every run would do it: a service would then exit at once,
cleanly, and never serve. A flag's value is the next word, unless that
word is one of ozd's flags, so `--address --name OZ` is `--address`
without a value. `-c` given twice is refused, and so is a file that
exists but cannot be read.

**How a refusal is reported.** Input that is not made of flags is a
usage error: an unknown flag, a word that is not a flag, a flag without
its value, `-c` naming a file that does not exist, or a line of the file
that is not a flag. ozd prints the usage and exits with 2. A refused
value is reported with its flag and value, and with its file and line
if it came from a file, and ozd exits with 1.

**Checked at load.** The first failure is reported with its flag, and
with its file and line if it came from a file (`src/config.rs`):

- **Addresses** must be valid for `parse_address`, which refuses a zero
  half. This host's address may not be a `--host`'s or a `--peer`'s, and
  no address may appear twice among the `--host`s or among the
  `--peer`s. A `--host` and a `--peer` may share an address, because
  they answer different questions about one host. That is how `cbridge`
  gets a name and a fixed endpoint.
- **Names** may appear only once across `--name` and all `--host`s,
  ignoring case, because HOSTAB looks them up that way. A name, a system
  type and a mount's name must be printable ASCII. HOSTAB and FILE send
  a character as one byte, and U+008D would be the band's newline in an
  answer.
- **System types** are given as `system=` on a `--host`, and on `--name`
  for this host's own type. Each flag may have one, with a value, in
  upper case. The band interns the value as it comes, and a type it has
  no flavor for gives the host its default flavor
  (`sys/network/host.lisp:279`), so `lispm` would quietly name the wrong
  one. System 100's own table gives `MIT-OZ` as `UNIX`
  (`sys/site/hosts.text:4`).
- **Roots**: there must be at least one root, at most one base, and no
  mount name twice. A mount's name must be one lower-case directory
  name, because a band asks in lower case and names match exactly (§6).
  Paths must be absolute.
- **Endpoints** must be IP literals, as `--listen` takes them, so there
  are no names to resolve at startup.

ozd ships no example files. The flags are few, and the README configures
a System 100 site with them on one command line.

## 9. A site of several machines

- **Each machine has an address.** A band knows the machines in its own
  host table, and System 100's table names one Lisp Machine,
  `MIT-LISPM-1` at 3050. A second emulator at another address still
  boots. A band whose address is not in its table makes itself an
  unnamed Lisp Machine (`SETUP-MY-ADDRESS`, `chsncp.lisp:776`), and
  `CHECK-THIS-SITE-INTEGRITY` says to fix the site files
  (`network/host.lisp:483`). The machine's own name is set in the band,
  in `SYS: SITE;`. What ozd adds is HOSTAB: with a `--host` here, every
  band that asks can find that machine by name. It is unverified that
  System 100's band knows `OZ`, the name its site option gives, as 3060.
  The first HOSTAB test against a band will show it.
- **Every machine names this host as its CHUDP peer and reaches the
  others through it** (§5). A machine whose CHUDP sends a unicast packet
  only to a peer named for its destination also names every other
  machine's address at this host's endpoint, which is the same endpoint
  each time.
- **On one host**, each program needs its own port. ozd takes 42042,
  and the machines take 42043, 42044 and so on (see the README).
- **On several hosts**, give `--listen` an address on the segment, or
  `0.0.0.0`, and have each machine name that address as its peer.
- **The Global Chaosnet** cannot be reached through ozd's switch, which
  passes nothing to another subnet. A site that wants it makes
  `cbridge` its switch instead: every machine names `cbridge` as its
  default CHUDP peer, and ozd is one more peer of `cbridge`. ozd then
  serves its services and passes nothing on, because every packet
  reaches it from `cbridge`'s endpoint, where every host is learned, and
  nothing is sent back to the endpoint it came from (§5). A band drops a
  packet it has no route for before sending it (`TRANSMIT-INT-PKT`,
  `chsncp.lisp:1928`), so ozd needs no route of its own; the machines
  would learn theirs from the RUT packets a bridge broadcasts. A run
  against a `cbridge` on 2026-09-15 showed that it passes a packet between
  two of its CHUDP peers on one subnet, raising its forwarding count by
  one and putting its own address in the trailer's source, and that ozd's
  answers pass back through it: a test host behind it got STATUS, TIME,
  UPTIME and a FILE listing from ozd. Two things are still
  **unverified**, because `cbridge`'s code is not read: a host on another
  subnet, which the test configuration did not reach, and RUT, none of
  which came over CHUDP in a minute.

## 10. Logging and running

- **stderr** gets one line per event, stamped in UTC by `civil` (`log.rs`).
  The events are: startup, its checks and its warnings, the file of flags it
  read and how many hosts a `--hosts-text` table gave (§8); each connection
  opened, refused and closed, with host and contact; every FILE operation
  that changes a root (write, rename, delete, create-directory, create-link
  and change-properties), with its pathname; and errors. A line that names a
  host gives its address in octal and then, in parentheses, its official
  name in the host table that `--name`, `--host` and `--hosts-text` make:
  `3050 (MIT-LISPM-1)`, or `3051 (?)` for an address the table does not
  hold.
- **`--log-simple`** adds a line for each simple transaction answered,
  STATUS, TIME or UPTIME, in the shape of a connection's lines: `TIME from
  3050 (MIT-LISPM-1) answered`. A band asks STATUS of every host at each
  `(hostat)`, so it is asked for on its own.
- **`--log-file`** adds what FILE serves to what it changes: a line for
  each file read, each directory listed and each `LOGIN`, in the same shape
  as a change's line, the client's address and then what it did. Without it
  a band's whole boot leaves one line, the connection it opened.
- **`--log-file-probe`** adds a line for each FILE `PROBE`. A band probes
  far more often than it reads, before a read and through a compile, and
  serves no file by it, so it is asked for on its own.
- **`--trace`** prints every packet, every packet passed on, and every
  drop.
- **`--check`** reads the flags and the file of flags, runs the startup
  checks and exits. It is for administrators and for the tests.
- **systemd**: `contrib/ozd.service` sets `User=ozd` and
  `Restart=on-failure`, and it adds hardening that costs nothing here:
  `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ReadWritePaths=` for
  each writable root, `ProtectHome=yes` and `PrivateTmp=yes`.
- **Installing**: `contrib/install-systemd.sh` installs the systemd
  service. It creates the `ozd` user and `/srv/lispm`, which that user
  owns, writes a standard `/etc/ozdrc` unless one exists, and runs
  `--check` as that user before it starts the service.
- **launchd**: `contrib/com.metebalci.ozd.plist` sets `UserName`,
  `ProgramArguments`, `KeepAlive` and `StandardErrorPath`.
- **Shutdown** is the default action of `SIGTERM`. The only state is the
  roots, and the temporary of an interrupted write is removed at the
  next start.

## 11. Tests

`cargo test` needs only the standard library. The tests use loopback
sockets and temporary directories. A test removes its directories when
it is done, or the harness keeps them under Cargo's
`CARGO_TARGET_TMPDIR`, inside `target/`, so a run leaves nothing behind
outside it.

**The harness**, `tests/support/`, builds a daemon from a configuration
in a temporary directory, and it provides **test hosts**. Each test host
is an `Ncp` at its own address, on its own loopback socket, with
scripted sessions. The NCP is symmetric, so `Ncp::connect` opens a
connection from a test host's side. The test turns all of them with one
clock that it sets.

**The tests** cover:

1. **The frame.** One whole packet's bytes and its checksum are pinned
   (`tests/frame.rs`), and so are two frames a running `cbridge` sent,
   which must read with a good checksum and be written back byte for
   byte (§5). The CADR's own check word is pinned beside them.
2. **The flags**: each flag, each form of `--listen`, the file of flags
   with its comments, the command line winning over the file, the search
   order, and each refusal with its line. A band's own host table is read
   as HOSTAB's beside them (`tests/hosts_text.rs`), and what the log flags
   add is asserted where FILE and the NCP are (`tests/file.rs`,
   `tests/ncp.rs`).
3. **The link and the switch.** An unknown host's endpoint is learned
   and answered. A fixed endpoint is not moved by a packet. A datagram
   with this host's own address as its source is dropped. A packet from
   one test host to another reaches it byte for byte. A broadcast
   reaches every test host except its sender, and it is answered here. A
   packet for another subnet, or for a host not yet heard from, reaches
   nobody. Nothing is sent back to the endpoint that a packet came from.
4. **STATUS** over loopback, with its meters counting what the test
   sent.
5. **TIME** within a second of the system clock, and **UPTIME** at 600
   after ten seconds of the test's clock.
6. **Containment.** A table of pathnames that a client can send, none of
   which may reach or touch anything outside the tree: `..` at every
   depth, a host's absolute path such as `/etc/passwd` (which lands
   under the root), a symlink under a root that points outside it, a
   symlink that the client makes with CREATE-LINK, a symlink swapped in
   between two commands, a write whose temporary or rename target would
   be outside, and a FIFO. Alongside them, a symlink inside a root is
   followed, a mount is reached by its name and not by `..`, a mount
   covers a base directory of its name and startup warns of it, and every
   write in a read-only root is refused with `ATF`, with nothing on disk
   changed.
7. **FILE**, with a scripted client, and with two clients at once.
8. **HOSTAB** and **NAME**, each with a scripted client taken from the
   machine's own user end.

**The acceptance test** is done by hand. ozd runs with the System 100
site that the README configures, its `tree` mount pointing at the
release's `sys` directory and its base root at an empty directory that
it can write. Two machines run next to it, each with a pack of its own,
and each names the other at ozd's endpoint (§9). Each machine should
boot, know the date, read its sources from the read-only mount, write in
the base, and print the right `(uptime)`, and `(hostat)` on each should
show ozd and the other machine. The second machine boots without a name,
because System 100's host table lists only 3050 (§9).
