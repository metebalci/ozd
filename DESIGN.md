# muir-ah: design

The detailed design, agreed on 2026-09-10, before any code is written.
`CLAUDE.md` holds the decisions and rules this follows, and
`PROTOCOLS.md` what each protocol is. §13 lists what is left, all of it
outside this repository.

## 1. The shape of it

One process, one thread, one UDP socket, one loop. muir-ah is the **hub**
of one subnet: the hosts on it name it as their CHUDP peer, it passes
packets between them as the cable would, and it answers its own
services.

    hosts on the subnet: Lisp Machines, cbridge
        │  CHUDP over UDP
    chudp::Link       the socket, the endpoints, the checks;
        │             a packet for another host passed on, untouched
        │  Framed, for this host
    Ncp               AIM-628 chapters 3 and 4, from muir
        │  Service / Session
    services          STATUS  TIME  UPTIME  FILE  ·  HOSTAB  NAME

Nothing blocks but the socket read, and nothing runs but the loop. That
is what lets containment check a path and then open it without `openat`
(§6), and it is why a second thread would be a change of design, not an
optimisation.

## 2. Build

- **Rust 2024**, the latest edition (`edition = "2024"`), as muir.
- **Toolchain pinned to 1.98.0**, stable, in `rust-toolchain.toml` with
  `clippy` and `rustfmt`. muir pins `1.99.0-beta.4` because 1.98
  miscompiles its `board.rs` at `opt-level = 3` with `codegen-units = 1`
  (muir's `rust-toolchain.toml`); neither that code nor that profile
  comes here, and the release profile is Cargo's default. When 1.99
  ships, the pin moves.
- **No dependencies.** `[dependencies]` stays empty. std has the socket,
  both clocks, the filesystem, the test harness and the arguments; the
  config is parsed by hand, as muir parses its flags. Two places where a
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
- **Library and binary**, as muir: `src/lib.rs` is the daemon, so tests
  drive it in-process; `src/main.rs` is arguments and the loop.
- **Lints**: `cargo clippy --all-targets -- -D warnings` clean.
- **rustfmt**: `use_small_heuristics = "Max"`, muir's.
- **Licence**: AGPL-3.0-or-later with SPDX headers, as muir.
- **Git**, as muir. `.gitignore`: `/target`, and `CLAUDE.md`, which is
  not committed for now. `Cargo.lock` is committed: this is a binary.

## 3. Modules

    src/
      lib.rs            the modules
      main.rs           arguments, startup, the loop
      config.rs         the site file, parsed and checked (§8)
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

| here | from muir | what changes |
|---|---|---|
| `address.rs` | `chaos/mod.rs`, `parse_address` | nothing |
| `packet.rs` | `chaos/packet.rs`, 246 lines | keeps `Packet`, `MAX_DATA`, `check_word`, `Framed`; leaves `frame`, `unframe`, `unframe_any` and `Received`, which are cable bits --- about 150 lines come |
| `chudp.rs` | `chaos/udp.rs`, 495 lines | `wrap`, `unwrap`, `Order`, `PACKET_ORDER`, `TRAILER_ORDER` and the constants verbatim, with their unverified marks; `Link` and `Chudp` replaced (§5), the learning kept, the hub new |
| `ncp.rs` | `chaos/server.rs`, 730 lines | `Server` is `Ncp`; `impl Node` becomes inherent `receive(now, &Framed)` and `transmit(now)`, and `aborted` goes; `receive` stops dropping on the trailer's check word (§5). `op`, `Response`, `Service`, `Out`, `Session`, `connect`, one packet in flight, `RETRANSMIT_NS`, `HOST_DOWN_NS`, the trace: verbatim, except that a BRD for a contact no service takes is let fall, as the machine's own NCP does (`chsncp.lisp:1613`) |
| `lispm.rs` | `chaos/file.rs`: `NEWLINE`, `to_lispm`, `from_lispm`, `lispm_text` | in a module of its own, since HOSTAB and NAME speak the character set too |
| `roots.rs` | `chaos/file.rs`: `resolve`, `resolve_for_writing` | rewritten (§6), apart from FILE, so that the containment tests come first and alone |
| `service/status.rs` | `chaos/status.rs`, 118 lines | real meters (§7) |
| `service/time.rs` | `chaos/time.rs`, 106 lines | UPTIME in sixtieths, not seconds (`PROTOCOLS.md`, UPTIME) |
| `service/file.rs` | `chaos/file.rs`, 1491 lines | containment, roots and `readonly` (§6); no allowlist |
| the rest | --- | new |

Copied, never moved: muir is read from here, and not written.

**`ncp`** is the module that is muir's `server.rs`: Chaosnet's
connection layer, its TCP. It opens and closes connections --- RFC, OPN,
CLS --- numbers and acknowledges packets, sends again what is lost,
carries EOF, and hands each request to the service registered for its
contact name. MIT's name for that layer is the Network Control Program;
the Lisp Machine's own is `sys/network/chaos/chsncp.lisp`. Each service
module is named after its contact name.

## 4. The loop

    start   config → startup checks (§6) → bind → Ncp with its services
    loop    now ← nanoseconds since start, from Instant
            wait for one datagram, at most 100 ms → the link (§5)
            while Ncp::transmit(now) gives a buffer → Link::send(buffer)

**Why it wakes when nothing arrives.** Two jobs are due even on a silent
network: sending again a packet the other end has not acknowledged,
every half second (`RETRANSMIT_NS`), and giving up a connection silent
for three minutes (`HOST_DOWN_NS`). muir's NCP has no timer of its own;
it does both whenever it is asked for output --- `transmit` runs
`service_all` when its queue is empty (`server.rs:611`, `:679`). So the
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
- **One packet in flight per connection**, as muir (`server.rs:554`): the
  far end is a CADR interface that holds one packet, and a burst was lost
  into it. A file moves at the other end's acknowledgement rate.

## 5. The link and the hub

`chudp::Link` owns the socket and the table of endpoints, and is the hub.

- **It binds what `listen` says**, in muir's `--chaos-udp` forms: none,
  a port, an address, or an address and a port. A bare port is on the
  loopback, and no `listen` at all is `127.0.0.1:42042` --- a fresh
  install answers its own host and nothing else. An address without a
  port takes 42042. `0.0.0.0`, or `::`, is every interface.
- **Endpoints are learned**, as muir's `--chaos-udp-dynamic` learns them
  (`udp.rs`, `arrived`): a packet's source address --- the header's ---
  is recorded at the UDP address the datagram came from. A host behind
  `cbridge` is learned at `cbridge`'s endpoint, which is right: that is
  where its packets go. A `peer` line fixes an endpoint, and a packet
  does not move a fixed one. The table is not expired; it holds at most
  one entry per address.
- **Receiving**, in order; each drop is counted for STATUS and printed
  under `--trace`:
  1. `unwrap`, verbatim: version, function, length.
  2. The trailer's source is this host's address, or 0: dropped, muir's
     rule for a frame claiming to be from this station.
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
  unverified (`udp.rs`, `unwrap`), and UDP carries a checksum of its
  own --- optional over IPv4, where a sender may leave it zero.
  muir's `Server::receive` drops on it, which is safe there only because
  muir's `Chudp` puts a check word it computed itself on the modelled
  cable. `Ncp::receive` does not drop on it.
- **Sending this host's own packets**: the NCP's buffer ends in the
  destination; it goes out as `wrap(buffer, own address,
  check_word(buffer + own address))` --- the 9401's CRC-16, which is what
  muir sends --- to the destination's endpoint, or to every distinct
  endpoint once for a destination of 0. A destination with no endpoint
  is dropped and counted.
- **A muir needs a default peer to use the hub**, which it does not have:
  it sends a unicast frame only to a peer named for that destination
  (`udp.rs:433`, `addressed`), so a packet for another machine is dropped
  in muir before it reaches this host. §9 has the way round it, which
  works with muir as it is; §13 notes what muir could add, in muir.

## 6. Containment

`CLAUDE.md` §3, as code. FILE serves anyone who reaches the socket; what
it may name is all the security there is.

**At startup**, before the socket is bound; each failure is a refusal to
start, with its reason:

- `geteuid()` is 0.
- A root is not an absolute path, does not canonicalise, is not a
  directory, or is `/`.
- A root without `readonly` is not writable --- a probe file created in
  it and removed.

Then two things that are not refusals. The base's entries that a mount
covers are **warned about**, by name: they exist and cannot be reached.
And in each writable root, stale FILE temporaries are removed: a daemon
killed mid-write leaves one. Their name comes from one constant, so the
cleanup matches what `file.rs` makes (`file.rs:868`) and nothing else.

**Roots.** FILE serves one tree: a **base root**, and optionally **named
roots mounted at its top level**, each with its own `readonly`.

    root        /srv/lispm                                        # the base: homes, and the rest
    root  tree  /path/to/muir/vendor/system-100-0/sys  readonly

`/tree/...` is the second directory, read-only; everything else under
`/` --- `/<user>/`, the home muir's LOGIN gives a user (`file.rs:426`) ---
is the base.

- **A mount covers the base.** If the base has a `tree` of its own, a
  mount named `tree` wins, as a Unix mount point covers the directory
  under it; the base's `tree` cannot be reached, and startup says so.
- **A listing of `/`** shows the base's entries and the mounts' names,
  each name once. Nothing can be made at `/` under a mount's name.
- **Names match exactly**, as every directory name does: muir's FILE
  folds no case in a pathname, only LOGIN's home (`file.rs:426`), and a
  band sends its pathnames in lower case (`/tree/sys/...`, muir's
  `--chaos-file-root` help). So mounts are named in lower case.
- **With mounts and no base**, `/` itself is read-only and names only
  the mounts. One `root` without a name is a single root, as muir's.

**`resolve(pathname)`**, rewritten from muir's (`file.rs:375`):

1. Split on `/` and drop empty components; a `.` or `..` is refused with
   `ATD`, not normalised. So no path climbs out of a root, or from one
   root into another.
2. The first component picks a mount if it names one, and the rest is
   under that root; otherwise the whole path is under the base.
3. Canonicalise the deepest part that exists, following every symlink
   on the way; the result must be that root or lie under it. A symlink
   anywhere is followed, and one that resolves outside its own root is
   refused with `ATD` when used. **No second tree by symlink**: muir's
   admission of the targets of the root's top-level symlinks does not
   come across --- a mount is how a directory elsewhere is served.
4. Return the canonical existing part with the not-yet-existing rest
   joined back on. That is what is opened, never the client's string.

`resolve_for_writing` also refuses a root itself, as muir's does, and
anything in a `readonly` root (below).

**Why a check and then an open are safe here**: the path checked is the
path opened, it contains no symlink when checked, and nothing else runs
between the two --- the loop is the only thread, and this process is a
writable root's only writer (`CLAUDE.md` §3); a `readonly` root is
written by nobody. Both conditions are written down in the code where
they are relied on.

**Opening**: `symlink_metadata` first; anything but a regular file or a
directory is refused with `ATD`. A FIFO would block the loop at `open`.

**`readonly`**: in a read-only root, OPEN for output, DELETE, RENAME,
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

**CREATE-LINK**: both ends resolved, as muir does.

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
  followed --- as Unix does, and as muir did. Removing a link that does
  not resolve is how one is got rid of.
- **DIRECTORY reads each entry with `symlink_metadata`, or through
  `resolve`, never `metadata`**: muir's follows links, and would give the
  size and date of a file outside the root.
- **A write's temporary is created once**, `create_new`, and written
  through the handle it was made with. muir reopens it by name for each
  packet (`file.rs:1390`).
- `/` itself resolves to the top, not to a path: PROBE and PROPERTIES of
  it, and DIRECTORY, read it through `Tree::list_top`.

## 7. Services

**Stage 1.**

- **STATUS**: the official name; one block, for this host's subnet, its
  meters counted at the socket and shared with the service through an
  `Arc` of atomics (`Service` is `Send`): 1, every datagram received; 2,
  every datagram sent, this host's own and those passed on; 7, those
  rejected for their length; 8, those rejected for anything else. 3 to
  6 are zero and true, as muir's documentation argues: there is no
  interface here to abort, lose a packet in, or fail a CRC.
- **TIME**: universal time from the system clock, least significant
  byte first. Verbatim.
- **UPTIME**: sixtieths of a second since start, `now × 60 / 10⁹`,
  wrapping at 32 bits, about 828 days.
- **FILE**: muir's, with §6.

**Stage 2.**

- **HOSTAB**: each line the client sends is looked up, ignoring case,
  among this host's names and every `host` line's. The answer is one
  `NAME` line per name, official first, `CHAOS` in octal, and
  `SYSTEM-TYPE` if its line gives one --- for this host, the `name` line, then an EOF; or `ERROR No such
  host`, then an EOF. Never `MACHINE-TYPE` (`PROTOCOLS.md`, HOSTAB). The
  connection stays open for the next name until the client closes it.
  A line longer than any name matches nothing, and no more of it than
  that is kept.
- **NAME**: one line, `Nobody is logged in.`, ended in the Lisp
  Machine's newline; then an EOF, and a CLS once the EOF is receipted, as
  the machine's own server ends (`FORMAT-AND-EOF`, `chuse.lisp:550`). A
  band's `(finger)` asks here when given no host (`PROTOCOLS.md`, NAME).

## 8. The config

One directive a line; `#` starts a comment. Addresses are octal, or
`subnet:host`, as muir's flags take them. Paths are absolute.

    address  3060                          # this host's Chaos address; required
    name     MIT-OZ OZ                     # its names, the official first; required
    listen   192.0.2.10                    # optional; see §5
    root     /srv/lispm                    # the base root; see §6
    root     tree  /path/to/muir/vendor/system-100-0/sys  readonly
    host     3050  MIT-LISPM-1 LM1   system=LISPM     # the site's host table, for HOSTAB
    host     3051  MIT-LISPM-2 LM2   system=LISPM
    peer     3040  192.0.2.5                          # an endpoint that is fixed; the rest are learned

The host table and the endpoints are separate: a `host` line is what
HOSTAB says, a `peer` line where packets go. A machine needs neither to
be served; it needs a `host` line to be found by name.

**Checked at load**, the first failure reported with its line
(`src/config.rs`): exactly one `address` and one `name` line; at least
one `root`, at most one without a name, no mount name twice, and a
mount's name one lower-case directory name, since a band asks in lower
case and names match exactly (§6); every address valid by
`parse_address`, which refuses a zero half; endpoints IP literals, as
`listen` takes them, no names to resolve at startup.

- **Addresses**: this host's may not appear on a `host` or a `peer`
  line, and no address twice among the `host` lines or among the `peer`
  lines. A `host` line and a `peer` line may share one: they answer
  different questions about one host, which is how `cbridge` gets a name
  and a fixed endpoint.
- **Names**: each once across `name` and every `host` line, ignoring
  case, as HOSTAB looks them up.
- **System types**: `system=` on a `host` line, and on the `name` line
  for this host's own; once a line, with a value, in upper case. The band
  interns the value as it comes, and a type it has no flavor for gives
  the host its default flavor (`sys/network/host.lisp:279`), so `lispm`
  would quietly name the wrong one. System 100's own table gives `MIT-OZ`
  as `UNIX` (`sys/site/hosts.text:4`), and its example says so.
- **One limit, as written**: there is no quoting, so a path with a space
  or a `#` in it cannot be named.

**Two examples ship**, `examples/system-100.conf` and
`examples/system-304.conf`, with each release's own numbers from muir
(`chaos/mod.rs`): System 100's file host is `MIT-OZ` at 3060 and its
band asks for `/tree/...`; System 304's is `OZ`, `AMS-BRIDGE-1`, at 4403,
and asks for `/sys/...`. Each mounts that release's sources read-only
from muir's `vendor/` --- `system-100-0/sys` and `system-304-0/sys-304-0`,
where muir's own `vendor/run/file-root` links point --- over a base for
homes.

## 9. A site of several machines

- **Each machine an address.** A band knows the machines in its own host
  table, and System 100's names one Lisp Machine, `MIT-LISPM-1` at 3050.
  A second emulator at another address still boots: a band whose address
  is not in its table makes itself an unnamed Lisp Machine
  (`SETUP-MY-ADDRESS`, `chsncp.lisp:776`), and `CHECK-THIS-SITE-INTEGRITY`
  says to fix the site files (`network/host.lisp:483`). Its own name is
  the band's business, in `SYS: SITE;`. What this host adds is HOSTAB:
  a `host` line here and every band that asks can find that machine by
  name. That System 100's table knows `OZ`, the name its site option
  gives, as 3060 is not verified here; the first HOSTAB test against a
  band shows it.
- **Every machine names this host, and reaches the others through it**
  (§5). With a default peer in muir (§13.1), one flag each:
  `--chaos-udp-peer 3060@<this host>`. Until then, each muir also names
  every other machine's address at this host's endpoint ---
  `--chaos-udp-peer 3051@<this host> --chaos-udp-peer 3052@<this host>`
  --- the same endpoint every time, which works with muir as it is.
- **On one host**, ports. muir-ah takes 42042, and each muir its own:
  `muir --chaos-address 3050 --chaos-udp 42043 --chaos-udp-peer
  3060@127.0.0.1:42042`, the next `3051` on `42044`, and so on. muir's
  `--chaos-address` without its `,<server>` half leaves muir's own
  server at its default, 177002, out of the band's way, so the band's
  calls to 3060 go out to this host. While muir keeps that server
  (`CLAUDE.md` §8a), a band's broadcast for TIME may be answered by
  either; both have the same clock.
- **On several hosts**, `listen` an address on the segment, or
  `0.0.0.0`, and each muir's `--chaos-udp-peer` names it.
- **The Global Chaosnet** is not reached through the hub, which passes
  nothing to another subnet. A machine that wants it has `cbridge` as a
  peer of its own for those addresses.

## 10. Logging and running

- **stderr**, one line an event, with a UTC timestamp made by
  `file.rs`'s `civil`, which comes across: startup, its checks and its
  warnings; each connection opened, refused and closed, with the host
  and contact; every FILE operation that changes a root --- write,
  rename, delete, create-directory, create-link --- with its pathname;
  errors.
- **`--trace`**: every packet, every packet passed on, and every drop,
  as muir's `--chaos-trace`.
- **`--check`**: parse the config, run the startup checks, exit. For an
  administrator, and for the tests.
- **systemd**, `contrib/muir-ah.service`: `User=muir-ah`, `Restart=on-failure`,
  and hardening that costs nothing here --- `NoNewPrivileges=yes`,
  `ProtectSystem=strict`, `ReadWritePaths=` each writable root,
  `ProtectHome=yes`, `PrivateTmp=yes`.
- **launchd**, `contrib/com.metebalci.muir-ah.plist`: `UserName`,
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
symmetric, and `Ncp::connect` (`server.rs:347`) opens a connection from
a test host's side. All are turned by the test with one clock it sets.

**The tests**, in the order they are written (`CLAUDE.md` §10):

1. **The frame**: the first five tests of muir's `tests/chudp.rs`, from
   `the_frame_is_these_bytes` (line 83), copied with the bytes unchanged,
   and muir's `the_check_word_is_the_boards`, which pins the check word.
   They are the contract between the two repositories (`CLAUDE.md` §8e).
2. **The config**: each directive; each form of `listen`; each refusal,
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
   `file.rs` comes across; with it, a symlink inside a root followed, one
   pointing out of it refused, a mount reached by its name and not by
   `..`, a mount covering a base directory of its name and startup
   warning of it, every write in a `readonly` root refused with `ATF` and
   nothing on disk changed.
7. **FILE**: the scripted client, then two at once.
8. **HOSTAB** and **NAME**, each with a scripted client taken from the
   machine's own user end.

**The acceptance test** is by hand, and written in the README: two muir
runs against muir-ah, with `examples/system-100.conf`, boot, know the
date, read their sources from the read-only mount and write in the base,
print the right `(uptime)`; `(hostat)` on each shows muir-ah and the
other.

## 12. Order of work

1. **Scaffold** --- `git init`, `Cargo.toml`, the pins, the licence, the
   lints → verify: `cargo build`, `cargo clippy --all-targets -- -D
   warnings` and `cargo test` pass on an empty crate.
2. **Address, packet, frame**, copied from muir with its tests → verify: the
   pinned bytes.
3. **Config** → verify: its tests.
4. **Link, hub, NCP, loop, STATUS** → verify: the link's and STATUS's
   tests; then by hand, `(hostat)` on a band run with muir-ah as its
   CHUDP peer, which is the first interoperation of this CHUDP with
   anything but itself.
5. **TIME, UPTIME** → verify: their tests; then a band that boots knowing
   the date.
6. **Containment tests**, failing → **FILE**, copied from muir and
   changed as §6 says → verify:
   containment, the scripted client and two clients pass; then a band
   reads and writes its files.
7. **The daemon's edges**: startup checks and warnings, logging,
   `--check`, the units, the two example configs → verify: a test for
   each refusal; the README's acceptance steps, by hand.
8. **HOSTAB**, then **NAME**, each test first.

## 13. Outside this repository

Nothing here changes muir; these are notes for it, done there if at all.

1. **A default peer in muir**: a unicast frame for an address with no
   peer of its own goes to one named peer, this host, instead of being
   dropped (`udp.rs:433`, `addressed`). A few lines in muir, and what
   turns §9's list of flags into one.
2. **muir's own server** (`CLAUDE.md` §8a): how much of it goes, now
   that this host serves what it served.
3. **A slip in muir's `tests/chudp.rs`**: `an_unknown_version_is_refused`
   passes `"{what} {value} is refused"` to `expect_err`, which takes a
   plain string, so on a failure the braces print as they are. The check
   itself holds. The copy in `tests/frame.rs` keeps it, so the two stay
   the same until muir changes.
4. **muir's NCP refuses a broadcast it does not serve**, with a CLS,
   where the machine's own lets it fall silent (`RECEIVE-BRD`,
   `chsncp.lisp:1613`, with `CLS-ON-ERROR-P` nil, `:1588`). Here it is
   silent (`tests/ncp.rs`); a hub meets every broadcast on the subnet.
5. **CHUDP against anything but muir.** muir and muir-ah share one
   reading of the frame's byte order, marked unverified in both; the
   first run against `cbridge` or `klh10` settles it for both.
