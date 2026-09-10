# ozd: design

The detailed design, agreed on 2026-09-10 before any code was written,
and built to since: where the code and this differ, one of them is
wrong. `CLAUDE.md` holds the decisions and rules this follows, and
`PROTOCOLS.md` what each protocol is. What is left is the acceptance
test with a band, by hand (§11).

## 1. The shape of it

One process, one thread, one UDP socket, one loop. ozd is the **hub**
of one subnet: the hosts on it name it as their CHUDP peer, it passes
packets between them as the cable would, and it answers its own
services.

    hosts on the subnet: Lisp Machines, cbridge
        │  CHUDP over UDP
    chudp::Link       the socket, the endpoints, the checks;
        │             a packet for another host passed on, untouched
        │  Framed, for this host
    Ncp               AIM-628 chapters 3 and 4
        │  Service / Session
    services          STATUS  TIME  UPTIME  FILE  ·  HOSTAB  NAME

Nothing blocks but the socket read, and nothing runs but the loop. That
is what lets containment check a path and then open it without `openat`
(§6), and it is why a second thread would be a change of design, not an
optimisation.

## 2. Build

- **Rust 2024**, the latest edition (`edition = "2024"`).
- **Toolchain pinned to 1.98.0**, stable, in `rust-toolchain.toml` with
  `clippy` and `rustfmt`; the release profile is Cargo's default. When
  1.99 ships, the pin moves.
- **No dependencies.** `[dependencies]` stays empty. std has the socket,
  both clocks, the filesystem, the test harness and the arguments; the
  flags and the file of them are parsed by hand. Two places where a
  crate would be the honest answer, and neither is taken:
  - **`openat` with `O_NOFOLLOW`**, for containment that holds against a
    concurrent writer. std has it only as a private helper
    (`openat_nofollow_dironly`, `sys/fs/unix.rs`, read in 1.92's source),
    and the constants differ by platform, so hand-written FFI would be
    guessing at numbers. One thread and one writer make it unnecessary
    (§6); a second thread would make `libc` the one dependency.
  - **Signals.** std has no handler. The daemon has nothing to flush, so
    `SIGTERM`'s default action is the shutdown (§10).
- **One `unsafe`**: `geteuid`, declared `extern "C"` --- std links the C
  library already --- so the daemon can refuse to run as root.
  `unsafe_code` is denied for the whole package in `Cargo.toml`'s
  `[lints]`, tests included, and that one function allows it.
- **Library and binary**: `src/lib.rs` is the daemon, so tests
  drive it in-process; `src/main.rs` is arguments and the loop.
- **Lints**: `cargo clippy --all-targets -- -D warnings` clean, and
  `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"`, so that a doc
  link that leads nowhere, or to a private item, is caught.
- **rustfmt**: `use_small_heuristics = "Max"`, a call or a struct
  literal on one line whenever it fits.
- **Licence**: AGPL-3.0-or-later with SPDX headers.
- **Git**. `.gitignore`: `/target`, and `CLAUDE.md`, which is
  not committed for now. `Cargo.lock` is committed: this is a binary.

## 3. Modules

    src/
      lib.rs            the modules
      main.rs           arguments, startup, the loop
      config.rs         the flags and the file of them, checked (§8)
      daemon.rs         the daemon: the link, the NCP, the services, turn (§4)
      log.rs            one line an event on stderr, stamped in UTC (§10)
      address.rs        parse_address
      packet.rs         the packet and the check word
      roots.rs          the tree FILE serves: roots, resolve, readonly (§6)
      chudp.rs          the frame, and the link and hub (§5)
      ncp.rs            the NCP
      lispm.rs          the Lisp Machine character set
      service/
        status.rs  time.rs  file.rs  hostab.rs  name.rs

**`ncp`** is Chaosnet's connection layer, its TCP. It opens and closes
connections --- RFC, OPN, CLS --- numbers and acknowledges packets, sends
again what is lost, carries EOF, and hands each request to the service
registered for its contact name. MIT's name for that layer is the Network
Control Program; the Lisp Machine's own is
`sys/network/chaos/chsncp.lisp`. Each service module is named after its
contact name.

A BRD for a contact no service takes is let fall, as the machine's own
NCP lets it fall (`RECEIVE-BRD`, `chsncp.lisp:1613`, with
`CLS-ON-ERROR-P` nil, `:1588`), not refused with a CLS: a hub meets every
broadcast on the subnet (`tests/ncp.rs`).

## 4. The loop

    start   config → startup checks (§6) → bind → Ncp with its services
    loop    now ← nanoseconds since start, from Instant
            wait for one datagram, at most 100 ms → the link (§5)
            while Ncp::transmit(now) gives a buffer → Link::send(buffer)

**Why it wakes when nothing arrives.** Two jobs are due even on a silent
network: sending again a packet the other end has not acknowledged,
every half second (`RETRANSMIT_NS`), and giving up a connection silent
for three minutes (`HOST_DOWN_NS`). The NCP has no timer of its own;
it does both whenever it is asked for output --- `transmit` runs
`service_all` when its queue is empty. So the
loop waits for a packet at most 100 ms, the socket's read timeout: a
packet that arrives is handled at once, and if none does the loop wakes
anyway, asks for output, and waits again. An idle daemon wakes ten times
a second, which costs nothing measurable, and a retransmission is at
most 100 ms late against its 500 ms interval.

- **`now` is `Instant` since start, in nanoseconds.** The NCP's timeouts
  are nanoseconds already and hold as written.
- **The body is `Daemon::turn(now, wait)`.** `main` calls it for ever with
  the clock; the tests call it with a clock they set, so a test of UPTIME
  or of a retransmission is exact rather than timed.
- **One packet in flight per connection**: the far end is a CADR
  interface that holds one packet, and a burst would be lost into it. A file moves at the other end's acknowledgement rate.

## 5. The link and the hub

`chudp::Link` owns the socket and the table of endpoints, and is the hub.

- **It binds what `--listen` says**: a port, an address, or an address
  and a port. A bare port is on the
  loopback, and no `--listen` at all is `127.0.0.1:42042` --- a fresh
  install answers its own host and nothing else. An address without a
  port takes 42042. `0.0.0.0`, or `::`, is every interface.
- **Endpoints are learned**: a packet's source address --- the header's
  --- is recorded at the UDP address the datagram came from. A host behind
  `cbridge` is learned at `cbridge`'s endpoint, which is right: that is
  where its packets go. A `--peer` fixes an endpoint, and a packet
  does not move a fixed one. The table is not expired; it holds at most
  one entry per address.
- **Receiving**, in order; each drop is counted for STATUS and printed
  under `--trace`:
  1. `unwrap`, verbatim: version, function, length.
  2. The trailer's source is this host's address, or 0: dropped, as a
     frame claiming to be from this station.
  3. The source is learned, unless it is fixed or is this host --- before
     anything else, so that an answer to it can be passed on at once.
  4. By destination:
     - **this host**: to `Ncp::receive`.
     - **0**, a broadcast: to `Ncp::receive`, and passed on to every
       endpoint but the one it came from.
     - **another host of this subnet with an endpoint**: passed on to
       that endpoint, unless it is the one the datagram came from.
     - **anything else** --- another subnet, or a host not yet heard
       from: dropped.
- **Passed on means untouched.** The datagram goes out as it came, byte
  for byte: the cable does not change a frame, and neither does the hub
  --- no forwarding count, no new trailer. That is what makes it a hub
  and not a bridge, and why nothing goes to another subnet: there is no
  routing to decide where.
- **The trailer's check word**, on a packet for this host, is compared
  and a mismatch traced, never dropped: what a CHUDP peer puts there is
  unverified (`chudp.rs`, `unwrap`), and UDP carries a checksum of its
  own --- optional over IPv4, where a sender may leave it zero.
  `Ncp::receive` does not drop on it.
- **Sending this host's own packets**: the NCP's buffer ends in the
  destination; it goes out as `wrap(buffer, own address,
  check_word(buffer + own address))` --- the 9401's CRC-16 (`packet.rs`)
  --- to the destination's endpoint, or to every distinct
  endpoint once for a destination of 0. A destination with no endpoint
  is dropped and counted.
- **A machine names this host as its CHUDP peer.** One whose CHUDP sends
  a unicast only to a peer named for its destination also names every
  other machine of the subnet at this host's endpoint, or its packets for
  them never reach the hub (§9).

## 6. Containment

`CLAUDE.md` §3, as code. FILE serves anyone who reaches the socket; what
it may name is all the security there is.

**At startup**, before the socket is bound; each failure is a refusal to
start, with its reason:

- `geteuid()` is 0.
- A root is not an absolute path, does not canonicalise, is not a
  directory, or is `/`.
- A root without `,ro` is not writable --- a probe file created in
  it and removed.

Then two things that are not refusals. The base's entries that a mount
covers are **warned about**, by name: they exist and cannot be reached.
And in each writable root, stale FILE temporaries are removed: a daemon
killed mid-write leaves one. Their name comes from one constant, so the
cleanup matches what FILE makes (`roots.rs`, `temporary_name`) and
nothing else.

**Roots.** FILE serves one tree: a **base root**, and optionally **named
roots mounted at its top level**, each read-only or not (`,ro`).

    --root /srv/lispm                         # the base: homes, and the rest
    --root tree=/path/to/system-100-0/sys,ro  # mounted at /tree, read-only

`/tree/...` is the second directory, read-only; everything else under
`/` --- `/<user>/`, the home LOGIN gives a user --- is the base.

- **A mount covers the base.** If the base has a `tree` of its own, a
  mount named `tree` wins, as a Unix mount point covers the directory
  under it; the base's `tree` cannot be reached, and startup says so.
- **A listing of `/`** shows the base's entries and the mounts' names,
  each name once. Nothing can be made at `/` under a mount's name.
- **Names match exactly**, as every directory name does: FILE folds no
  case in a pathname, only LOGIN's home, and a band sends its pathnames
  in lower case (`/tree/...`, System 100's `sys/site/sys.translations`).
  So mounts are named in lower case.
- **With mounts and no base**, `/` itself is read-only and names only
  the mounts. One `--root` without a name is a single root.

**`resolve(pathname)`**:

1. Split on `/` and drop empty components; a `.` or `..` is refused with
   `ATD`, not normalised. So no path climbs out of a root, or from one
   root into another.
2. The first component picks a mount if it names one, and the rest is
   under that root; otherwise the whole path is under the base.
3. Canonicalise the deepest part that exists, following every symlink
   on the way; the result must be that root or lie under it. A symlink
   anywhere is followed, and one that resolves outside its own root is
   refused with `ATD` when used. **No second tree by symlink**: a link
   at a root's top level does not bring its target into the tree --- a
   mount is how a directory elsewhere is served.
4. Return the canonical existing part with the not-yet-existing rest
   joined back on. That is what is opened, never the client's string.

`resolve_for_writing` also refuses a root itself, and anything in a
read-only root (below).

**Why a check and then an open are safe here**: the path checked is the
path opened, it contains no symlink when checked, and nothing else runs
between the two --- the loop is the only thread, and this process is a
writable root's only writer (`CLAUDE.md` §3); a read-only root is
written by nobody. Both conditions are written down in the code where
they are relied on.

**Opening**: `symlink_metadata` first; anything but a regular file or a
directory is refused with `WKF`, "wrong kind of file", which the band
turns into its `WRONG-KIND-OF-FILE` condition (`sys/io/file/open.lisp:260`).
A FIFO would block the loop at `open`.

**A read-only root**, `,ro`: in one, OPEN for output, DELETE, RENAME,
CREATE-DIRECTORY, CREATE-LINK and CHANGE-PROPERTIES are refused with
`ATF`, "Access to file denied", before anything is touched. The band
turns `ATF` into `INCORRECT-ACCESS-TO-FILE` (`io/file/open.lisp:224`),
its ordinary condition for it. FILE has no code of its own for a refused
write (`PROTOCOLS.md`, FILE). A RENAME from one root into another is
refused the same way: it would be a copy, not a rename.

**Writes**: the temporary is made in the target's own directory, so the
same resolve holds it inside its root, and it is renamed over the
target. Two machines writing one file: the last rename wins, whole; no
file is ever half one and half the other.

**CREATE-LINK**: both ends resolved.

**Holes closed in writing `roots.rs`**, each with its test in
`tests/containment.rs`:

- **Roots may not overlap**; startup refuses it. A read-only mount inside
  a writable base could otherwise be written through a link made in the
  base, which resolves under the base.
- **A temporary's name is refused in every pathname**, with `ATD`. No
  client can delete one mid-write and put a link in its place, and the
  cleanup touches only what this daemon made.
- **A link that does not resolve**, dangling or looping, is refused:
  creating through a dangling link would make its target, wherever it
  points.
- **No roots, or a bad mount name, do not start.** The writability probe
  is named as a temporary, so one a crash leaves is removed at the next
  start.

**FILE's rules, from the same work** (§12, step 6):

- A link is followed to read or to write through; **DELETE and RENAME act
  on the link itself** --- its directory resolved, its own name not
  followed --- as Unix does. Removing a link that does
  not resolve is how one is got rid of.
- **DIRECTORY reads each entry with `symlink_metadata`, or through
  `resolve`, never `metadata`**, which follows links, and would give the
  size and date of a file outside the root.
- **A write's temporary is created once**, `create_new`, and written
  through the handle it was made with, never reopened by name.
- `/` itself resolves to the top, not to a path: PROBE and PROPERTIES of
  it, and DIRECTORY, read it through `Tree::list_top`.
- **A listing** describes a link inside its own root as what it leads
  to, and a link the tree refuses --- out of its root, into another, or
  leading nowhere --- by its own size and date, so that it can be seen
  and deleted. A temporary is listed nowhere: no pathname can name one.
- **`/`** answers PROBE and PROPERTIES as a directory of length 0, dated
  now; OPEN of it for reading is `FNF`, and a link to it is refused.

## 7. Services

**Stage 1.**

- **STATUS**: the official name; one block, for this host's subnet, its
  meters counted at the socket and shared with the service through an
  `Arc` of atomics (`Service` is `Send`): 1, every datagram received; 2,
  every datagram sent, this host's own and those passed on; 7, those
  rejected for their length; 8, those rejected for anything else. 3 to
  6 are zero and true: there is no
  interface here to abort, lose a packet in, or fail a CRC.
- **TIME**: universal time from the system clock, least significant
  byte first.
- **UPTIME**: sixtieths of a second since start, `now × 60 / 10⁹`,
  wrapping at 32 bits, about 828 days.
- **FILE**: `sys/doc/chfile.text`, with §6.

**Stage 2.**

- **HOSTAB**: each line the client sends is looked up, ignoring case,
  among this host's names and every `--host`'s. The answer is one
  `NAME` line per name, official first, `CHAOS` in octal, and
  `SYSTEM-TYPE` if its flag gives one --- `--name`'s, for
  this host --- then an EOF; or `ERROR No such host`, then an EOF. Never
  `MACHINE-TYPE` (`PROTOCOLS.md`, HOSTAB). The
  connection stays open for the next name until the client closes it.
  A line longer than any name matches nothing, and no more of it than
  that is kept.
- **NAME**: one line, `Nobody is logged in.`, ended in the Lisp
  Machine's newline; then an EOF, and a CLS once the EOF is receipted, as
  the machine's own server ends (`FORMAT-AND-EOF`, `chuse.lisp:550`). A
  band's `(finger)` asks here when given no host (`PROTOCOLS.md`, NAME).

## 8. The flags, and the file of them

Everything is a flag, and a file of flags gives the defaults. Addresses
are octal, or `subnet:host`. A value is one word, its parts separated by
commas (`--root /srv/lispm,ro`). As a file of them:

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
    # an endpoint that is fixed; the rest are learned
    --peer 3040@192.0.2.5

`--address` and `--name` are required, and given once, as `--listen` is
if at all; `--root`, `--host` and `--peer` may each be given again. A
`--root` whose value begins with `/` is the base; any other is
`<name>=<path>`, mounted at `/<name>`; `,ro` makes either read-only.
`--peer` is `<address>@<ip>[:<port>]`, the port 42042 unless given. A comma cannot be in a path.

The host table and the endpoints are separate: a `--host` is what HOSTAB
says, a `--peer` where packets go. A machine needs neither to be served;
it needs a `--host` to be found by name.

**The file of flags, `.ozdrc`**: `-c|--config <file>` names one, which
must be there; else `OZD_RC` names one; else
`.ozdrc` in the directory ozd is run from; else `.ozdrc` in
the home directory --- the first of those there, not all of them. A line
is a flag and, after a space, the rest of the line is its value; a blank
line, or one beginning with `#`, is a comment. A flag the command line
gives leaves that flag's lines out of the file: the command line has the
last word. A file cannot name another.

`#` begins a comment only at the start of a line, so a path in a file
may hold a blank or a `#`. The file is read first and the command line
after it. `--trace` may be in a file; `--check`, `--help` and `--config`
may not: each is what one run is asked to do, and in a file every run
would do it --- a service would exit at once, cleanly, and never serve.
A flag's value is the next word unless that word is one of ozd's
flags, so `--address --name OZ` is `--address` without its value. `-c`
given twice is refused, and so is a file that is there but cannot be
read.

**What is refused, and how.** What is not flags --- an unknown flag, a
word that is none, a flag without its value, `-c` naming a file that is
not there, a file's line that is not a flag --- is a usage error, with
the usage, and exits 2. A value refused names its flag and value, and
its file and line when it came from one, and exits 1.

**Checked at load**, the first failure reported with its flag, and with
its file and line when it came from one (`src/config.rs`):

- **Addresses**: each valid by `parse_address`, which refuses a zero
  half. This host's may not be a `--host`'s or a `--peer`'s, and no
  address comes twice among the `--host`s or among the `--peer`s. A
  `--host` and a `--peer` may share one: they answer different questions
  about one host, which is how `cbridge` gets a name and a fixed
  endpoint.
- **Names**: each once across `--name` and every `--host`, ignoring case,
  as HOSTAB looks them up. A name, a system type and a mount's name are
  printable ASCII: HOSTAB and FILE send a character as one byte, and
  U+008D would be the band's newline in an answer.
- **System types**: `system=` on a `--host`, and on `--name` for this
  host's own; once a flag, with a value, in upper case. The band interns
  the value as it comes, and a type it has no flavor for gives the host
  its default flavor (`sys/network/host.lisp:279`), so `lispm` would
  quietly name the wrong one. System 100's own table gives `MIT-OZ` as
  `UNIX` (`sys/site/hosts.text:4`), and its example says so.
- **Roots**: at least one, at most one base, no mount's name twice, and a
  mount's name one lower-case directory name, since a band asks in lower
  case and names match exactly (§6); paths absolute.
- **Endpoints**: IP literals, as `--listen` takes them; no names to
  resolve at startup.

**Two examples ship**, `examples/system-100.ozdrc` and
`examples/system-304.ozdrc`, files of flags with each band's own
numbers. System 100's file host is `MIT-OZ` at 3060
(`sys/site/hosts.text`), and its band asks for `/tree/...`
(`sys/site/sys.translations`). System 304's is `OZ`, `AMS-BRIDGE-1`, at
4403, as its band's listener gives it, and its band asks for `/sys/...`
(the site files in its pack). Each mounts that release's sources
read-only --- `system-100-0/sys` and `system-304-0/sys-304-0` --- over a
base for homes.

## 9. A site of several machines

- **Each machine an address.** A band knows the machines in its own host
  table, and System 100's names one Lisp Machine, `MIT-LISPM-1` at 3050.
  A second emulator at another address still boots: a band whose address
  is not in its table makes itself an unnamed Lisp Machine
  (`SETUP-MY-ADDRESS`, `chsncp.lisp:776`), and `CHECK-THIS-SITE-INTEGRITY`
  says to fix the site files (`network/host.lisp:483`). Its own name is
  the band's business, in `SYS: SITE;`. What this host adds is HOSTAB:
  a `--host` here and every band that asks can find that machine by
  name. That System 100's table knows `OZ`, the name its site option
  gives, as 3060 is not verified here; the first HOSTAB test against a
  band shows it.
- **Every machine names this host as its CHUDP peer, and reaches the
  others through it** (§5). A machine whose CHUDP sends a unicast only to
  a peer named for its destination also names every other machine's
  address at this host's endpoint --- the same endpoint every time.
- **On one host**, ports: ozd takes 42042, and each machine one of its
  own, the first 42043, the next 42044, and so on (the README).
- **On several hosts**, `--listen` an address on the segment, or
  `0.0.0.0`, and each machine names it as its peer.
- **The Global Chaosnet** is not reached through the hub, which passes
  nothing to another subnet. A machine that wants it has `cbridge` as a
  peer of its own for those addresses.

## 10. Logging and running

- **stderr**, one line an event, with a UTC timestamp made by `civil`
  (`log.rs`): startup, its checks and its
  warnings; each connection opened, refused and closed, with the host
  and contact; every FILE operation that changes a root --- write,
  rename, delete, create-directory, create-link --- with its pathname;
  errors.
- **`--trace`**: every packet, every packet passed on, and every drop.
- **`--check`**: read the flags and the file of them, run the startup
  checks, exit. For an
  administrator, and for the tests.
- **systemd**, `contrib/ozd.service`: `User=ozd`, `Restart=on-failure`,
  and hardening that costs nothing here --- `NoNewPrivileges=yes`,
  `ProtectSystem=strict`, `ReadWritePaths=` each writable root,
  `ProtectHome=yes`, `PrivateTmp=yes`.
- **launchd**, `contrib/com.metebalci.ozd.plist`: `UserName`,
  `ProgramArguments`, `KeepAlive`, `StandardErrorPath`.
- **Shutdown** is `SIGTERM`'s default. The only state is the roots, and
  an interrupted write's temporary is removed at the next start.

## 11. Tests

`cargo test`, std only: loopback sockets, and temporary directories
that a test removes when done, or that the harness keeps under Cargo's
`CARGO_TARGET_TMPDIR`, inside `target/`, so that a run leaves nothing
behind outside it.

**The harness**, `tests/support/`: a daemon built from a config in a
temporary directory, and **test hosts** --- each an `Ncp` at its own
address on its own loopback socket, with scripted sessions. The NCP is
symmetric, and `Ncp::connect` opens a connection from
a test host's side. All are turned by the test with one clock it sets.

**The tests**, in the order they are written (`CLAUDE.md` §10):

1. **The frame**: one whole packet's bytes, and the check word, pinned
   (`tests/frame.rs`). The byte order is unverified (§5); the first run
   against another implementation, `cbridge` or `klh10`, settles it, and
   a correction is a change to two constants and to those tests.
2. **The flags**: each flag; each form of `--listen`; the file of them,
   its comments, the command line winning, the search order; each refusal,
   with its line.
3. **The link and the hub**: an unknown host's endpoint learned and
   answered; a fixed endpoint not moved by a packet; this host's own
   address as the source dropped; a packet from one test host to another
   reaching it byte for byte; a broadcast reaching every test host but
   its sender, and answered here; a packet for another subnet, or for a
   host not yet heard from, reaching nobody; nothing sent back to the
   endpoint it came from.
4. **STATUS** over loopback, its meters counting what the test sent.
5. **TIME** within a second of the system clock; **UPTIME** 600 at ten
   seconds of the test's clock.
6. **Containment**, the table of `CLAUDE.md` §10.4, written before
   FILE; with it, a symlink inside a root followed, one
   pointing out of it refused, a mount reached by its name and not by
   `..`, a mount covering a base directory of its name and startup
   warning of it, every write in a read-only root refused with `ATF` and
   nothing on disk changed.
7. **FILE**: the scripted client, then two at once.
8. **HOSTAB** and **NAME**, each with a scripted client taken from the
   machine's own user end.

**The acceptance test** is by hand, and written in the README: two
machines run against ozd, with `examples/system-100.ozdrc`, boot, know
the date, read their sources from the read-only mount and write in the base,
print the right `(uptime)`; `(hostat)` on each shows ozd and the
other.

## 12. Order of work

1. **Scaffold** --- `git init`, `Cargo.toml`, the pins, the licence, the
   lints → verify: `cargo build`, `cargo clippy --all-targets -- -D
   warnings` and `cargo test` pass on an empty crate.
2. **Address, packet, frame**, with their tests → verify: the pinned
   bytes.
3. **Config** → verify: its tests.
4. **Link, hub, NCP, loop, STATUS** → verify: the link's and STATUS's
   tests; then by hand, `(hostat)` on a band run with ozd as its
   CHUDP peer, which is the first interoperation of this CHUDP with
   anything but itself.
5. **TIME, UPTIME** → verify: their tests; then a band that boots knowing
   the date.
6. **Containment tests**, failing → **FILE**, as §6 says → verify:
   containment, the scripted client and two clients pass; then a band
   reads and writes its files.
7. **The daemon's edges**: startup checks and warnings, logging,
   `--check`, the units, the two example configs → verify: a test for
   each refusal; the README's acceptance steps, by hand.
8. **HOSTAB**, then **NAME**, each test first.
