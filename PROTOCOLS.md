# Chaosnet protocols

Every contact name a Lisp Machine serves or calls, as the vendored
System 100 and System 304 sources have them: what each one is, what goes
over the wire, and where that was read. What muir-ah does about each is
`CLAUDE.md` §4; this file records only what they are, and grows as each
is read more closely.

**Where the manual and the machine's code disagree, the code wins.** It
is what a band sends and what it expects back. One case so far: UPTIME.

## Sources

All in muir's `vendor/`, where its fetch scripts put the releases. Paths
below are under
`system-100-0/sys/`, with System 100's line numbers, unless marked
**(304)**, which is `system-304-0/sys-304-0/`.

- `man/chaos.text`, chapter *The Chaosnet*, section *Higher-Level
  Protocols* --- the manual, cited below as §*Name*.
- `network/chaos/chsaux.lisp` --- the machine's own servers, each put on
  `SERVER-ALIST` by an `ADD-INITIALIZATION`, and most of their user ends.
- `network/chaos/chsncp.lisp` --- the NCP, which answers STATUS itself.
- `network/chaos/chuse.lisp`, `io1/conver.lisp`, `sys2/band.lisp`,
  `tape/`, `window/`, `file/server.lisp` --- the rest.
- `site/site.lisp` --- which hosts a band calls for what.

"Serves" means registered on `SERVER-ALIST` in that release. Patches are
left out.

## Summary

✓ read in the source · **no** absent from both releases' `SERVER-ALIST`
· blank: not read

| contact | kind | the machine serves | the machine calls | muir-ah (`CLAUDE.md` §4) |
|---|---|---|---|---|
| `STATUS` | RFC/ANS | ✓ | ✓ | stage 1 |
| `TIME` | RFC/ANS | ✓ | ✓ | stage 1 |
| `UPTIME` | RFC/ANS | ✓ | ✓ | stage 1 |
| `FILE` | stream | ✓ | ✓ | stage 1 |
| `HOSTAB` | stream | **no** | ✓ | stage 2 |
| `DUMP-ROUTING-TABLE` | RFC/ANS | ✓ | ✓ | not needed |
| `NAME` | stream | ✓ | ✓ | stage 2 |
| `FINGER` | RFC/ANS | ✓ | ✓ | between machines, none here |
| `TELNET` | stream | ✓ | ✓ | wanted, not this project |
| `SUPDUP` | stream | **no** | ✓ | not this project |
| `MAIL` | stream | refuses | | not needed |
| `EXPAND-MAILING-LIST` | stream | **no** | ✓ | not needed |
| `SMTP` | stream | **no** | | not needed |
| `SEND` | stream | ✓ | ✓ | between machines, none here |
| `NOTIFY` | RFC/ANS | ✓ | ✓ | between machines, none here |
| `BABEL` | stream | ✓ | | not needed |
| `RESET-TIME-SERVER` | RFC/ANS | **no** | ✓ | not needed |
| `SPELL` | connection | **no** | ✓ | recorded |
| `EVAL` | stream | ✓ | | the machine's own |
| `REMOTE-DISK` | stream | ✓ | | the machine's own |
| `BAND-TRANSFER` | stream | ✓ | | the machine's own |
| `HKDUMP` | stream | ✓ | | the machine's own |
| `LRTDP` | stream | ✓ (304) | ✓ (304) | the machine's own |

RFC/ANS is AIM-628's *simple transaction*: one RFC, one ANS, no
connection. A stream is a connection: RFC then OPN, numbered data both
ways, closed by EOF and CLS.

## Stage 1

### STATUS

Simple transaction. Every node must answer, bridges included, and the
NCP answers it itself rather than starting a server process, "in order
to provide rapid response" (§Status; `SEND-STATUS`, `chsncp.lisp`).
`(hostat)` is the user end, and so is `NEW-HOST-VALIDATION-FUNCTION`
(`chuse.lisp`), which takes a host's name from the first 32 bytes.

The ANS: 32 bytes of the node's name, padded with zero bytes. Then
blocks, in 16- and 32-bit words, low byte first, and the low half of a
32-bit word first:

- word 0: `400` octal plus a subnet number --- this block is the host's
  direct connection to that subnet.
- word 1: how many 16-bit words follow, usually 16.
- then eight 32-bit counts for that subnet: packets received;
  transmitted; transmissions aborted by collision or a busy receiver;
  packets lost because the interface had not been read; CRC errors;
  errors found after the packet was read out of the buffer; rejected
  for length; rejected for anything else.

Every item past the count is optional by the count, and extra items are
to be ignored. An identification of `0`--`377` is an obsolete block with
16-bit counts, no longer to be sent; `1000` and up are reserved.

muir: `src/chaos/status.rs`.

### TIME

Simple transaction. The ANS is four bytes: the universal time ---
seconds since midnight GMT, 1 January 1900 --- least significant byte
first (§Time; AIM-628 §5.8). It wraps on 7 February 2036.

The machine serves it (`TIME-SERVER`, registered `chsaux.lisp:879`) only
once it knows the time, and otherwise refuses with "I don't know what
time it is." Its user end, `HOST-TIME` (`chuse.lisp`), asks the site's
time servers and takes the first answer.

muir: `src/chaos/time.rs`.

### UPTIME

Simple transaction. The ANS is four bytes, least significant first,
like TIME's.

**The unit is sixtieths of a second, and the manual is wrong about
it.** §Uptime says "an interval (in seconds)". The machine's own server
sends `(* 60. (- (TIME:GET-UNIVERSAL-TIME) TIME:*UT-AT-BOOT-TIME*))`
(`UPTIME-SERVER`, registered `chsaux.lisp:880`), and both its user ends
--- `HOST-UPTIME` and `UPTIME`, just below it --- divide what comes back
by 60 before printing it. `DECODE-CANONICAL-TIME-PACKET`'s own
documentation says "an integral number of 60ths of a second".

muir's `time.rs` followed the manual and answers seconds, so a band
asking muir's server prints a sixtieth of the real uptime; no muir test
pins the unit. muir-ah answers sixtieths. At that unit, four bytes wrap
after about 828 days.

### FILE

A stream, and more: the server opens data connections back to contact
names the user end chooses. The protocol is `doc/chfile.text`; the
machine's user end is `network/chaos/qfile.lisp`, and the machine can
serve FILE itself, from its own file system (`file/server.lisp`). A band
calls it at the file host its site file names.

**Errors.** `doc/chfile.text`'s table (line 728) has no code for a
refused write. Past the table a server's codes are its own system's ---
ITS's table, or on TOPS-20 the initials of the error message's first
three words. muir answers `ATD`, "Access to directory denied", which is
`FILE.c`'s, for a pathname outside the tree. The machine turns `FNF`,
`ATF` and `ATD` into `FILE-NOT-FOUND`, `INCORRECT-ACCESS-TO-FILE` and
`INCORRECT-ACCESS-TO-DIRECTORY` (`io/file/open.lisp:180`, `:224`,
`:231`).

muir: `src/chaos/file.rs`. Containment: `CLAUDE.md` §3.

## Stage 2

### HOSTAB

A stream of transactions (§Host Table). The user end sends a host name
and a newline; the server answers with lines of `ATTRIBUTE value`, then
an EOF, and is ready for the next name. The user end closes when done.
Values are strings --- unquoted, no newlines --- or octal numbers; names
and most values are upper case, and an attribute may repeat.

Attributes: `ERROR` (the message; "no such host" is the one to expect),
`NAME` (the first is the official name, the rest nicknames),
`MACHINE-TYPE` (`LISPM`, `PDP10`, ...), `SYSTEM-TYPE` (`LISPM`, `ITS`,
...), `CHAOS` (an address, octal), and `ARPA`, `DIAL`, `LCS` and `SU`
for other networks.

**The machine only calls it**, and neither release serves it, though
the manual says Lisp Machine servers exist. The user end is
`CHAOS-UNKNOWN-HOST-FUNCTION` (`chuse.lisp:950`), installed as
`SI::UNKNOWN-HOST-FUNCTION`: when `si:parse-host` meets a name the
band's host table lacks, it asks each host of the site option
`:CHAOS-HOST-TABLE-SERVER-HOSTS` and defines the host from the answer
with `SI:DEFINE-HOST`. **System 100's site names OZ** (`site/site.lisp:91`,
`'("OZ")`), so a band already asks its associated machine. System 304's
names `"MC" "OZ" "XX" "EE" "SCRC-TENEX"` (**(304)** `site/site.lisp:173`).

What that user end reads: `ERROR` ends the transaction; each `NAME` is
kept, and on EOF they are sorted shortest first; `SYSTEM-TYPE` is kept
as a symbol; every other attribute is parsed as an address, by that
attribute's own `HOST-ADDRESS-PARSER` or else by the Chaosnet one.
**`MACHINE-TYPE` looks as if it falls into the last case**: the clause
is written `(:SYSTEM-TYPE MACHINE-TYPE)`, the second without its colon,
while the attribute's name is interned as a keyword, so it would not
match and would be parsed as a Chaosnet address. Unverified against a
running band; until it is, an answer from here leaves `MACHINE-TYPE`
out.

**Lines, as that user end sends and reads them** (read for muir-ah's
HOSTAB). The stream carries the Lisp Machine character set untranslated,
`OPEN-STREAM`'s default (`chuse.lisp:782`, `:787`). The user end sends
each name with `:LINE-OUT`, which ends it with `#\CR`, `215` octal
(`io/stream.lisp:473`, `io/rddefs.lisp:172`), then `:FORCE-OUTPUT`
(`chuse.lisp:956`). It reads the answer with `:LINE-IN`, which ends a
line at `215` and hands back an unfinished last line flagged as at EOF,
which the user end ignores (`io/stream.lisp:552`, `:554`;
`chuse.lisp:963`). So every line of an answer ends in `215`, the last
included. Each needs its space (`chuse.lisp:973`); the attribute's name
is interned as sent, so it is upper case (`:974`), and so is a
`SYSTEM-TYPE`'s value, to name a flavor (`:983`); `NAME` comes first,
the official name first of all, since the first becomes the host's name
(`:978`); and `CHAOS` is octal (`chsaux.lisp:1617`).

### NAME

A stream (§Name): the Arpanet's Name/Finger protocol, unchanged. The
contact name may be followed by a space and arguments, like the Arpanet
protocol's command line. The server sends the finger display in the
Lisp Machine character set, then EOF. The machine serves it (`GIVE-NAME`,
registered `chsaux.lisp:420`) in a process of its own, since a stream
needs the background process free for retransmission; the same page has
the user end.

**The associated machine is the default.** The machine's `FINGER`
fingers `SI:ASSOCIATED-MACHINE` when given no host --- no argument, or a
user without `@` --- and sends it `NAME` (`chsaux.lisp:476`). So do the
keyboard's finger prompt (`KBD-FINGER`, `window/basstr.lisp`) for a user
without a host, and the time parser's birthday lookup (`SET-BIRTHDAY`,
`io1/timpar.lisp`, reached only by parsing a user's birthday). What this
host says: `CLAUDE.md` §8f.

The machine's display is one line, and it ignores the RFC's arguments:
the user ID in six columns, the group affiliation character, the full
name in 22, what the selected window is (`Zmacs`, `Lisp`, `Supdup`,
`Telnet`, ...; `USER-ACTIVITY-STRING`), the idle time, and the
terminal's location. What those fields hold with nobody logged in is not
read.

## Not this project

### TELNET

A stream: the Arpanet's Telnet (RFC 854), unchanged, in 8-bit bytes
(§Telnet and Supdup). Chaosnet has no out-of-band signal, so Telnet
sends a controlled data packet of opcode `201` octal where the Arpanet
sent INS; it is not out of band, but serves for interrupt-process and
discard-output.

The machine serves TELNET (registered `chsaux.lisp:1254`) and has a user
end (`window/telnet-code.lisp`, `window/telnet-front-hack.lisp`). So
whether a Lisp Machine "has telnet" is a question about the other end:

- **Into the machine**: a Chaosnet TELNET user end on Unix, or a gateway
  from TCP telnet to Chaosnet TELNET. It can be a CHUDP peer of the
  machine directly --- muir takes more than one `--chaos-udp-peer` ---
  or reach it through muir-ah, the subnet's hub.
- **Out of the machine, to Unix**: a Chaosnet TELNET server on Unix,
  which is a login shell for a client that does not authenticate.
  `CLAUDE.md` §3.

### SUPDUP

A stream: a display terminal protocol, the manual's AIM-644 (§Telnet and
Supdup). Where Telnet sends a stream of characters and leaves the screen
to whatever escape codes the terminal happens to take, SUPDUP has the
user end describe its screen once, and the server then drives it with a
small set of screen operations of its own. Its words are ITS's own
terminal variables, and the machine's code expects ITS on the far side
("Print out the greeting message ITS sends"), where an editor --- EMACS,
which the code names --- knows exactly what this screen can do and uses
it.

The machine has a user end and does not serve it (`window/supdup.lisp`;
`BASIC-NVT`, its network virtual terminal window, is shared with
Telnet). What that user end does, read from `:CONNECT`,
`SEND-TTY-VARIABLES` and `:GOBBLE-GREETING`:

- **It connects** to contact `SUPDUP`, or through a gateway to socket
  `137` octal (`PARSE-PATH PATH "SUPDUP" 137`).
- **It describes itself**, in 18-bit words, a PDP-10's halfwords: a count
  of the words that follow; `TCTYP`, always `%TNSFW`, 7; `TTYOPT`, what
  it can do; `TCMXV` and `TCMXH`, its height in lines and width in
  characters, each less one; `TTYROL`, 0, no scrolling; and `TTYSMT`,
  with its line height and character width. `TTYOPT`'s left half says:
  erase, move backward, SAIL characters, overprint, move up, lower case,
  the full character set on input --- the keyboard's Control and Meta
  --- more-processing, and line insert and delete. Character insert and
  delete only if `SUPDUP-%TOCID` is set, which it is not by default,
  "because the Lispm is so fast at outputting characters that EMACS is
  effectively faster for the user without CID capability".
- **It sends a finger string**, then prints the server's greeting, in
  ASCII, up to a `%TDNOP` (`210` octal).
- **Then it obeys `%TD` codes**, the codes of `200` octal and up in the
  output (`SUPDUP-%TD-DISPATCH`): cursor motion (`%TDMOV`, `%TDMV0`),
  clear to the end of the line and of the screen (`%TDEOL`, `%TDEOF`),
  clear the screen (`%TDCLR`), insert and delete lines and characters
  (`%TDILP`, `%TDDLP`, `%TDICP`, `%TDDCP`), scroll a region (`%TDRSU`,
  `%TDRSD`), the bell, graphics (`%TDGRF`), and a few more.

Its far end is an ITS, or any host with a SUPDUP server; on Unix that
server is a login shell, as TELNET's would be, `CLAUDE.md` §3.

## Between machines

One Lisp Machine to another. muir-ah neither sends nor receives them
(`CLAUDE.md` §8h); what a site needs for them to arrive is a path
between the two machines: peered directly, or through muir-ah as
its subnet's hub, which passes them on without reading them
(`CLAUDE.md` §8b).

### FINGER

Simple transaction: NAME cut down to one ANS, and the Lisp Machine's
own (§Name). The ANS is five lines in the Lisp Machine character set:
the logged-in user ID; where the terminal is
(`SI::LOCAL-FINGER-LOCATION`); the idle time --- empty if none, minutes
under an hour, else `H:MM`; the user's full name, first name first; and
the group affiliation, one character (`GIVE-FINGER`, registered
`chsaux.lisp:391`, which caches the string since it is nearly always
the same as last time).

Its user end is `FINGER-LISPMS` (`chsaux.lisp`), behind `finger-all-lms`
and the keyboard's finger command, and it asks only Lisp Machines ---
`ALL-LOCAL-LISPMS` when not given the hosts.

### SEND

A stream (§Send): **an interactive message to someone logged in now,
not mail** --- Unix's `write`, not its `mail`. The RFC carries `SEND`
and the recipient. Its being accepted means the recipient is there and
taking messages, and a refusal says why ("~A is not accepting
messages."). The first line is a header, `user@host date`; then the
message; then EOF.

The machine serves it (`RECEIVE-MSG`, `io1/conver.lisp:1320`; **(304)**
`io1/conver.lisp:1367`) and shows it in Converse. Its user ends are
`:LMSEND`, and `SHOUT`, which sends to every Lisp Machine
(`chsaux.lisp:342`).

### NOTIFY

Simple transaction, not in the manual. The RFC carries `NOTIFY` and a
short message; the machine pops it up as a notification, "From *host*:
*message*", unless it repeats the last one, and answers `Done`
(`NOTIFY-SERVER`, `chsaux.lisp:1318`). The user ends are `chaos:notify`
and `chaos:notify-all-lms`.

## Not needed

### MAIL

A stream (§Mail). The sender sends each recipient on a line, then a
blank line, and a reply comes back for each recipient at once. Then the
text, which normally carries an Arpanet header, then EOF; one more reply
once the mail is swallowed, and both sides close. A reply is one line:
`+` success, `-` permanent failure, `%` temporary failure, then text.

**The machine refuses all mail.** Its server (`DUMMY-MAIL-SERVER`,
registered `chsaux.lisp:928`) answers every recipient "-Lisp Machines do
not accept mail, maybe you want the :LMSEND command." A Lisp Machine
only sends mail, to a host with a mail system.

### EXPAND-MAILING-LIST

A stream, not in the manual. The user end (`EXPAND-MAILING-LISTS`,
`chsaux.lisp`) sends one name per line; an answer starting `-` is a bad
address, and one starting `+` is followed by the list's members, one per
line, up to a blank line. The machine calls it and does not serve it.

### SMTP

RFC 821 over a Chaosnet stream, contact `SMTP` (§Mail, its last
paragraph).

### BABEL

A stream, not in the manual. The server accepts and then sends the
printable ASCII characters, in order, over and over until the
connection breaks: "Useful for debugging chaosnet" (`BABEL-SERVER`,
`chsaux.lisp:1028`). The Arpanet's chargen. As a load on an NCP --- its
window, its retransmission, its throughput --- nothing is cheaper.

### RESET-TIME-SERVER

Simple transaction (§Time Server Control). It tells a time server with
no timebase of its own, "usually because it is a bridge", to reset its
time from the network; the ANS is the success. The manual advises such
a server to stop answering TIME, wait about 20 seconds, and then ask, so
as not to take back a bad time from machines it "infected".

The machine only calls it (`RESET-TIME-SERVER`, `chsaux.lisp:327`, "This
always works for MINITS boxes"). A host that keeps its time by the
operating system's clock has nothing to reset.

### DUMP-ROUTING-TABLE

Simple transaction (§Routing Information). The ANS is two 16-bit words
per subnet, indexed by subnet number: for subnet *n*, word 2*n* is the
method and word 2*n*+1 the cost. A method of 0 is no known way; 1 to
`377` octal is an interface on that subnet; `400` and up is the address
of a host that forwards to it.

The machine's server (`DUMP-ROUTING-TABLE`, registered
`chsaux.lisp:1146`) sends *N* subnets, *N* the smaller of its table
(`ROUTING-TABLE-SIZE`, 96, `chsncp.lisp:253`) and what a packet holds
(`MAX-DATA-WORDS-PER-PKT`, 244, `chsncp.lisp:80`, over two: 122) --- so
96 subnets in 384 bytes --- and sets its own subnet's entry to interface
1 at cost 15. It indexes that entry by subnet whatever *N* is, so a
machine on subnet `140` octal or higher writes it past the bytes it
sends, and on `172` octal or higher past the packet's data. That is read
from the code and not seen.

**Two user ends call it, and nothing else in the tree names the
contact** (`chsaux.lisp:1073-1128`). Both are commands a person types,
and both send the bare contact name: **the RFC carries no subnet
number**, and the server reads nothing from it. Both honour the answer's
length. `SHOW-ROUTING-TABLE` prints every entry the answer carries whose
method is not 0 and whose cost is under `MAXIMUM-ROUTING-COST`, through
`FORMAT-ROUTING-TABLE-PKT`. `SHOW-ROUTING-PATH` reads one entry, the
destination subnet's: past the answer's length, or at a method of 0, it
says "No routing table entry for subnet ~O"; at an interface, "Direct
path"; at a bridge, it asks that bridge in turn --- one request per hop.
It reads the entry's words before it checks the length, so a subnet past
the packet's data may fail at the read rather than print that message;
unverified.

## Recorded, not planned

### SPELL

A connection, not in the manual. The user end
(`CHECK-SPELLING-WORDLIST`, `chsaux.lisp`) sends a word list and reads
one packet back, from a host of the site option `:SPELL-SERVER-HOSTS`.
The answer's format is not read. The machine does not serve it.

## The machine's own

Served by a Lisp Machine for other machines. Each is access to the
machine's insides for a client that does not authenticate; serving any
of them from here is the decision `CLAUDE.md` §3 asks for.

### EVAL

A stream: a read-eval-print loop over ASCII (§The Eval Server). **On by
default in System 100** (`EVAL-SERVER-ON`, `chsaux.lisp:1168`, `T`;
`:NOTIFY` accepts and tells the user), so every machine offers its Lisp
to whoever reaches it. What keeps that inside the site is whom each
machine's CHUDP link will hear from.

### REMOTE-DISK

A stream (§Remote Disk): read or change another machine's disk,
"primarily ... for printing and editing the disk label". It also takes a
`SAY` command, which pops up a notification (registered
`chsaux.lisp:933`).

### BAND-TRANSFER

A stream, not in the manual. The RFC is `BAND-TRANSFER`, then `READ` or
`WRITE`, the band, an optional subset, a size and a comment; the band
--- a disk partition --- is copied off or onto the machine. Refused while
someone is logged in, unless `BAND-TRANSFER-SERVER-ON`
(`sys2/band.lisp:16`, registered at `:286`).

### HKDUMP

A stream, not in the manual: the "Hack Dump Server", a dump of the
machine's file system, `FULL` or `INCREMENTAL` by the contact name's
argument. It streams file names, byte sizes and authors, and marks each
file dumped when told `:OK` (`tape/fdump-def.lisp:146`, registered at
`:155`).

### LRTDP

A stream, System 304 only, not in the manual: the remote tape device ---
another machine drives this one's tape (**(304)**
`tape/remote-tape-device.lisp:110`, registered at `:116`; 1986).

## Out of scope

The **Internet gateway** (§Internet Gateway) carries Chaosnet streams to
TCP, which is a path to another network. **DOVER** (§Dover) sends a press
file to the Dover printer at MIT through a protocol translator.
