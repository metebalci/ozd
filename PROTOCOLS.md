# Chaosnet protocols

This file lists every contact name that a Lisp Machine serves or calls
in the System 100 and System 304 releases. For each one it says what the
protocol is, what goes over the wire, and where that was read. It
records only what the protocols are. The summary's last column and
`DESIGN.md` say what ozd does about them.

**Where the manual and the machine's code disagree, the code wins**,
because the code is what a band actually sends and expects back. UPTIME
is one such case.

## Sources

Paths below are under System 100's `system-100-0/sys/`, with its line
numbers. Paths marked **(304)** are under System 304's
`system-304-0/sys-304-0/`.

- `man/chaos.text`, chapter *The Chaosnet*, section *Higher-Level
  Protocols*, is the manual. It is cited below as §*Name*.
- `network/chaos/chsaux.lisp` holds the machine's own servers, each put
  on `SERVER-ALIST` by an `ADD-INITIALIZATION`, and most of their user
  ends.
- `network/chaos/chsncp.lisp` is the NCP, which answers STATUS itself.
- `network/chaos/chuse.lisp`, `io1/conver.lisp`, `sys2/band.lisp`,
  `tape/`, `window/` and `file/server.lisp` hold the rest.
- `site/site.lisp` says which hosts a band calls for what.

"Serves" means that the contact name is registered on `SERVER-ALIST` in
that release. Patches are left out.

## Summary

In the two machine columns, ✓ means the source was read and shows it,
**no** means that both releases' `SERVER-ALIST` lack it, and a blank
means it was not read. In the ozd column, ✓ means that ozd serves it.

| contact | kind | the machine serves | the machine calls | ozd |
|---|---|---|---|---|
| `STATUS` | RFC/ANS | ✓ | ✓ | ✓ |
| `TIME` | RFC/ANS | ✓ | ✓ | ✓ |
| `UPTIME` | RFC/ANS | ✓ | ✓ | ✓ |
| `FILE` | stream | ✓ | ✓ | ✓ |
| `HOSTAB` | stream | **no** | ✓ | ✓ |
| `DUMP-ROUTING-TABLE` | RFC/ANS | ✓ | ✓ | not needed |
| `NAME` | stream | ✓ | ✓ | ✓ |
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

RFC/ANS is AIM-628's *simple transaction*: one RFC, one ANS and no
connection. A stream is a connection: an RFC, then an OPN, then numbered
data both ways, closed by EOF and CLS.

## Served

### STATUS

STATUS is a simple transaction. Every node must answer it, bridges
included. The NCP answers it itself rather than starting a server
process, "in order to provide rapid response" (§Status; `SEND-STATUS`,
`chsncp.lisp`). `(hostat)` is the user end, and so is
`NEW-HOST-VALIDATION-FUNCTION` (`chuse.lisp`), which takes a host's name
from the first 32 bytes.

The ANS starts with 32 bytes of the node's name, padded with zero
bytes. Blocks follow, in 16-bit and 32-bit words. Each word is sent low
byte first, and each 32-bit word low half first:

- Word 0 is `400` octal plus a subnet number. It means that this block
  describes the host's direct connection to that subnet.
- Word 1 says how many 16-bit words follow, usually 16.
- Then come eight 32-bit counts for that subnet: packets received;
  packets transmitted; transmissions aborted by a collision or a busy
  receiver; packets lost because the interface had not been read; CRC
  errors; errors found after the packet was read out of the buffer;
  packets rejected for their length; and packets rejected for anything
  else.

Every item past the count is optional, as the count says, and extra
items are to be ignored. An identification of `0` to `377` is an
obsolete block with 16-bit counts, which is no longer to be sent.
Identifications of `1000` and up are reserved.

### TIME

TIME is a simple transaction. The ANS is four bytes: the universal time,
in seconds since midnight GMT on 1 January 1900, least significant byte
first (§Time; AIM-628 §5.8). It wraps on 7 February 2036.

The machine serves it (`TIME-SERVER`, registered `chsaux.lisp:879`)
only once it knows the time. Otherwise it refuses with "I don't know
what time it is." Its user end, `HOST-TIME` (`chuse.lisp`), asks the
site's time servers and takes the first answer.

### UPTIME

UPTIME is a simple transaction. The ANS is four bytes, least significant
byte first, like TIME's.

**The unit is sixtieths of a second, and the manual is wrong about
it.** §Uptime says "an interval (in seconds)". The machine's own server
sends `(* 60. (- (TIME:GET-UNIVERSAL-TIME) TIME:*UT-AT-BOOT-TIME*))`
(`UPTIME-SERVER`, registered `chsaux.lisp:880`). Both of its user ends,
`HOST-UPTIME` and `UPTIME` just below it, divide the answer by 60 before
printing it. `DECODE-CANONICAL-TIME-PACKET`'s own documentation says "an
integral number of 60ths of a second".

A server that follows the manual and answers in seconds makes a band
print a sixtieth of the real uptime. In sixtieths, four bytes wrap after
about 828 days.

### FILE

FILE is a stream, and more: the server opens data connections back to
contact names that the user end chooses. The protocol is
`doc/chfile.text`, and the machine's user end is
`network/chaos/qfile.lisp`. The machine can also serve FILE itself, from
its own file system (`file/server.lisp`). A band calls it at the file
host that its site file names.

**Errors.** `doc/chfile.text`'s table (line 728) has no code for a
refused write. Beyond the table, a server uses its own system's codes:
ITS's table, or on TOPS-20 the initials of the first three words of the
error message (`:787`). `ATD` is one of these; `FILE.c` gives it as
"Incorrect access to directory" (`FILE.h:78`). ozd gives it for a
pathname outside the tree. The machine turns `FNF`, `ATF` and `ATD` into
`FILE-NOT-FOUND`, `INCORRECT-ACCESS-TO-FILE` and
`INCORRECT-ACCESS-TO-DIRECTORY` (`io/file/open.lisp:180`, `:224`,
`:231`), and `WKF` into `WRONG-KIND-OF-FILE` (`:260`), each through
`QFILE-PROCESS-ERROR-NEW` (`network/chaos/qfile.lisp:300`).

`DESIGN.md` §6 describes how ozd keeps FILE inside its roots.

### HOSTAB

HOSTAB is a stream of transactions (§Host Table). The user end sends a
host name and a newline. The server answers with lines of `ATTRIBUTE
value`, then an EOF, and is then ready for the next name. The user end
closes the connection when it is done. Values are strings, unquoted and
without newlines, or octal numbers. Names and most values are upper
case, and an attribute may repeat.

The attributes are `ERROR` (the message; "no such host" is the one to
expect), `NAME` (the first is the official name and the rest are
nicknames), `MACHINE-TYPE` (`LISPM`, `PDP10`, ...), `SYSTEM-TYPE`
(`LISPM`, `ITS`, ...), `CHAOS` (an address, in octal), and `ARPA`,
`DIAL`, `LCS` and `SU` for other networks.

**The machine only calls it.** Neither release serves it, although the
manual says that Lisp Machine servers exist. The user end is
`CHAOS-UNKNOWN-HOST-FUNCTION` (`chuse.lisp:950`), installed as
`SI::UNKNOWN-HOST-FUNCTION`. When `si:parse-host` meets a name that the
band's host table lacks, it asks each host of the site option
`:CHAOS-HOST-TABLE-SERVER-HOSTS`, and it defines the host from the
answer with `SI:DEFINE-HOST`. **System 100's site names OZ**
(`site/site.lisp:91`, `'("OZ")`), so a band already asks its associated
machine. System 304's site names `"MC" "OZ" "XX" "EE" "SCRC-TENEX"`
(**(304)** `site/site.lisp:173`).

This is what that user end reads. `ERROR` ends the transaction. Each
`NAME` is kept, and on EOF the names are sorted shortest first.
`SYSTEM-TYPE` is kept as a symbol. Every other attribute is parsed as an
address, by that attribute's own `HOST-ADDRESS-PARSER` or else by the
Chaosnet one. **`MACHINE-TYPE` looks as if it falls into the last
case.** The clause is written `(:SYSTEM-TYPE MACHINE-TYPE)`, the second
without its colon, while the attribute's name is interned as a keyword.
So it would not match, and its value would be parsed as a Chaosnet
address. This is unverified against a running band, so ozd's answers
leave `MACHINE-TYPE` out.

**Lines, as that user end sends and reads them.** These details were
read for ozd's HOSTAB. The stream carries the Lisp Machine character set
untranslated, which is `OPEN-STREAM`'s default (`chuse.lisp:782`,
`:787`). The user end sends each name with `:LINE-OUT`, which ends it
with `#\CR`, `215` octal (`io/stream.lisp:473`, `io/rddefs.lisp:172`),
and then calls `:FORCE-OUTPUT` (`chuse.lisp:956`). It reads the answer
with `:LINE-IN`, which ends a line at `215`. `:LINE-IN` hands back an
unfinished last line flagged as at EOF, and the user end ignores that
line (`io/stream.lisp:552`, `:554`; `chuse.lisp:963`). So every line of
an answer ends in `215`, the last line included. Each line needs its
space (`chuse.lisp:973`). The attribute's name is interned as sent, so
it must be upper case (`:974`), and so must a `SYSTEM-TYPE`'s value,
because it names a flavor (`:983`). `NAME` comes first, with the
official name first of all, since the first name becomes the host's name
(`:978`). `CHAOS` is in octal (`chsaux.lisp:1617`).

### NAME

NAME is a stream (§Name): the Arpanet's Name/Finger protocol, unchanged.
The contact name may be followed by a space and arguments, like the
Arpanet protocol's command line. The server sends the finger display in
the Lisp Machine character set, then EOF. The machine serves it
(`GIVE-NAME`, registered `chsaux.lisp:420`) in a process of its own,
since a stream needs the background process to be free for
retransmission. The same page has the user end.

**The associated machine is the default.** When the machine's `FINGER`
is given no host, either no argument at all or a user without `@`, it
fingers `SI:ASSOCIATED-MACHINE` and sends it `NAME` (`chsaux.lisp:476`).
The keyboard's finger prompt (`KBD-FINGER`, `window/basstr.lisp`) does
the same for a user without a host. So does the time parser's birthday
lookup (`SET-BIRTHDAY`, `io1/timpar.lisp`), which is reached only by
parsing a user's birthday. ozd answers that nobody is logged in
(`DESIGN.md` §7).

The machine's display is one line, and it ignores the RFC's arguments.
The line holds the user ID in six columns, the group affiliation
character, the full name in 22 columns, what the selected window is
(`Zmacs`, `Lisp`, `Supdup`, `Telnet`, ...; `USER-ACTIVITY-STRING`), the
idle time, and the terminal's location. What those fields hold with
nobody logged in has not been read.

## Not this project

### TELNET

TELNET is a stream: the Arpanet's Telnet (RFC 854), unchanged, in 8-bit
bytes (§Telnet and Supdup). Chaosnet has no out-of-band signal, so
Telnet sends a controlled data packet of opcode `201` octal where the
Arpanet sent INS. This packet is not out of band, but it serves for
interrupt-process and discard-output.

The machine serves TELNET (registered `chsaux.lisp:1254`) and has a user
end (`window/telnet-code.lisp`, `window/telnet-front-hack.lisp`). So
whether a Lisp Machine "has telnet" is a question about the other end:

- **Into the machine** needs a Chaosnet TELNET user end on Unix, or a
  gateway from TCP telnet to Chaosnet TELNET. It can be a CHUDP peer of
  the machine directly, or reach the machine through ozd, the subnet's
  hub.
- **Out of the machine, to Unix** needs a Chaosnet TELNET server on
  Unix. That server would be a login shell for a client that does not
  authenticate, and ozd does not provide one.

### SUPDUP

SUPDUP is a stream: a display terminal protocol, the manual's AIM-644
(§Telnet and Supdup). Telnet sends a stream of characters and leaves the
screen to whatever escape codes the terminal happens to take. SUPDUP
instead has the user end describe its screen once, and the server then
drives the screen with a small set of screen operations of its own. Its
words are ITS's own terminal variables, and the machine's code expects
ITS on the far side ("Print out the greeting message ITS sends"). There,
an editor such as EMACS, which the code names, knows exactly what the
screen can do and uses it.

The machine has a user end and does not serve SUPDUP
(`window/supdup.lisp`). `BASIC-NVT`, its network virtual terminal
window, is shared with Telnet. This is what that user end does, as read
from `:CONNECT`, `SEND-TTY-VARIABLES` and `:GOBBLE-GREETING`:

- **It connects** to contact `SUPDUP`, or through a gateway to socket
  `137` octal (`PARSE-PATH PATH "SUPDUP" 137`).
- **It describes itself** in 18-bit words, a PDP-10's halfwords. It
  sends a count of the words that follow; `TCTYP`, always `%TNSFW`, 7;
  `TTYOPT`, what it can do; `TCMXV` and `TCMXH`, its height in lines and
  its width in characters, each less one; `TTYROL`, 0, meaning no
  scrolling; and `TTYSMT`, with its line height and character width.
  `TTYOPT`'s left half says that it can erase, move backward, show SAIL
  characters, overprint, move up, show lower case, take the full
  character set on input (the keyboard's Control and Meta), do
  more-processing, and insert and delete lines. It claims character
  insert and delete only if `SUPDUP-%TOCID` is set, which it is not by
  default, "because the Lispm is so fast at outputting characters that
  EMACS is effectively faster for the user without CID capability".
- **It sends a finger string**, and then prints the server's greeting,
  in ASCII, up to a `%TDNOP` (`210` octal).
- **Then it obeys `%TD` codes**, the codes of `200` octal and up in the
  output (`SUPDUP-%TD-DISPATCH`). They cover cursor motion (`%TDMOV`,
  `%TDMV0`), clearing to the end of the line and of the screen
  (`%TDEOL`, `%TDEOF`), clearing the screen (`%TDCLR`), inserting and
  deleting lines and characters (`%TDILP`, `%TDDLP`, `%TDICP`,
  `%TDDCP`), scrolling a region (`%TDRSU`, `%TDRSD`), the bell, graphics
  (`%TDGRF`), and a few more.

Its far end is an ITS, or any host with a SUPDUP server. On Unix, that
server would be a login shell, as a TELNET server would be.

## Between machines

These protocols go from one Lisp Machine to another. ozd neither sends
nor receives them. For them to arrive, a site needs a path between the
two machines: either the machines are peered directly, or ozd passes the
packets between them as its subnet's hub, without reading them
(`DESIGN.md` §5).

### FINGER

FINGER is a simple transaction: NAME cut down to one ANS, and the Lisp
Machine's own (§Name). The ANS is five lines in the Lisp Machine
character set: the logged-in user ID; where the terminal is
(`SI::LOCAL-FINGER-LOCATION`); the idle time, which is empty if there is
none, in minutes under an hour, and otherwise `H:MM`; the user's full
name, first name first; and the group affiliation, one character. The
server is `GIVE-FINGER` (registered `chsaux.lisp:391`), which caches the
string, since it is nearly always the same as last time.

Its user end is `FINGER-LISPMS` (`chsaux.lisp`), behind
`finger-all-lms` and the keyboard's finger command. It asks only Lisp
Machines, and it asks `ALL-LOCAL-LISPMS` when it is not given the hosts.

### SEND

SEND is a stream (§Send). **It is an interactive message to someone
logged in now, not mail**: it is Unix's `write`, not its `mail`. The RFC
carries `SEND` and the recipient. If the RFC is accepted, the recipient
is there and taking messages; a refusal says why ("~A is not accepting
messages."). The first line is a header, `user@host date`, then comes
the message, and then EOF.

The machine serves it (`RECEIVE-MSG`, `io1/conver.lisp:1320`; **(304)**
`io1/conver.lisp:1367`) and shows the message in Converse. Its user ends
are `:LMSEND`, and `SHOUT`, which sends to every Lisp Machine
(`chsaux.lisp:342`).

### NOTIFY

NOTIFY is a simple transaction, and it is not in the manual. The RFC
carries `NOTIFY` and a short message. The machine pops the message up as
a notification, "From *host*: *message*", unless it repeats the last
one, and answers `Done` (`NOTIFY-SERVER`, `chsaux.lisp:1318`). The user
ends are `chaos:notify` and `chaos:notify-all-lms`.

## Not needed

### MAIL

MAIL is a stream (§Mail). The sender sends each recipient on a line,
then a blank line, and a reply comes back for each recipient at once.
Then the sender sends the text, which normally carries an Arpanet
header, and then EOF. One more reply comes once the mail is swallowed,
and both sides close. A reply is one line: `+` for success, `-` for a
permanent failure or `%` for a temporary failure, followed by text.

**The machine refuses all mail.** Its server (`DUMMY-MAIL-SERVER`,
registered `chsaux.lisp:928`) answers every recipient with "-Lisp
Machines do not accept mail, maybe you want the :LMSEND command." A Lisp
Machine only sends mail, to a host with a mail system.

### EXPAND-MAILING-LIST

EXPAND-MAILING-LIST is a stream, and it is not in the manual. The user
end (`EXPAND-MAILING-LISTS`, `chsaux.lisp`) sends one name per line. An
answer that starts with `-` is a bad address. An answer that starts with
`+` is followed by the list's members, one per line, up to a blank line.
The machine calls it and does not serve it.

### SMTP

SMTP is RFC 821 over a Chaosnet stream, at contact `SMTP` (§Mail, its
last paragraph).

### BABEL

BABEL is a stream, and it is not in the manual. The server accepts the
connection and then sends the printable ASCII characters, in order, over
and over, until the connection breaks: "Useful for debugging chaosnet"
(`BABEL-SERVER`, `chsaux.lisp:1028`). It is the Arpanet's chargen. As a
load on an NCP, testing its window, its retransmission and its
throughput, nothing is cheaper.

### RESET-TIME-SERVER

RESET-TIME-SERVER is a simple transaction (§Time Server Control). It
tells a time server that has no timebase of its own, "usually because it
is a bridge", to reset its time from the network. The ANS means success.
The manual advises such a server to stop answering TIME, wait about 20
seconds, and then ask, so that it does not take back a bad time from
machines that it "infected".

The machine only calls it (`RESET-TIME-SERVER`, `chsaux.lisp:327`, "This
always works for MINITS boxes"). A host that keeps its time by the
operating system's clock has nothing to reset.

### DUMP-ROUTING-TABLE

DUMP-ROUTING-TABLE is a simple transaction (§Routing Information). The
ANS is two 16-bit words per subnet, indexed by subnet number: for subnet
*n*, word 2*n* is the method and word 2*n*+1 is the cost. A method of 0
means no known way. A method from 1 to `377` octal is an interface on
that subnet. A method of `400` or more is the address of a host that
forwards to it.

The machine's server (`DUMP-ROUTING-TABLE`, registered
`chsaux.lisp:1146`) sends *N* subnets. *N* is the smaller of its table's
size (`ROUTING-TABLE-SIZE`, 96, `chsncp.lisp:253`) and what a packet
holds (`MAX-DATA-WORDS-PER-PKT`, 244, `chsncp.lisp:80`, divided by two:
122). So it sends 96 subnets in 384 bytes. It sets its own subnet's
entry to interface 1 at cost 15. It indexes that entry by subnet
whatever *N* is, so a machine on subnet `140` octal or higher writes it
past the bytes it sends, and a machine on subnet `172` octal or higher
writes it past the packet's data. This is read from the code and has not
been observed.

**Two user ends call it, and nothing else in the tree names the
contact** (`chsaux.lisp:1073-1128`). Both are commands a person types,
and both send the bare contact name: **the RFC carries no subnet
number**, and the server reads nothing from it. Both honour the answer's
length. `SHOW-ROUTING-TABLE` prints, through `FORMAT-ROUTING-TABLE-PKT`,
every entry that the answer carries whose method is not 0 and whose cost
is under `MAXIMUM-ROUTING-COST`. `SHOW-ROUTING-PATH` reads one entry, the
destination subnet's. Past the answer's length, or at a method of 0, it
says "No routing table entry for subnet ~O". At an interface, it says
"Direct path". At a bridge, it asks that bridge in turn, one request per
hop. It reads the entry's words before it checks the length, so a subnet
past the packet's data may fail at the read rather than print that
message; this is unverified.

## Recorded, not planned

### SPELL

SPELL is a connection, and it is not in the manual. The user end
(`CHECK-SPELLING-WORDLIST`, `chsaux.lisp`) sends a word list and reads
one packet back, from a host of the site option `:SPELL-SERVER-HOSTS`.
The answer's format has not been read. The machine does not serve it.

## The machine's own

A Lisp Machine serves these for other machines. Each one gives a client
that does not authenticate access to the machine's insides. ozd serves
none of them.

### EVAL

EVAL is a stream: a read-eval-print loop over ASCII (§The Eval Server).
**It is on by default in System 100** (`EVAL-SERVER-ON`,
`chsaux.lisp:1168`, `T`; `:NOTIFY` accepts and tells the user), so every
machine offers its Lisp to whoever reaches it. What keeps that inside
the site is which hosts each machine's CHUDP link will hear from.

### REMOTE-DISK

REMOTE-DISK is a stream (§Remote Disk) that reads or changes another
machine's disk, "primarily ... for printing and editing the disk label".
It also takes a `SAY` command, which pops up a notification (registered
`chsaux.lisp:933`).

### BAND-TRANSFER

BAND-TRANSFER is a stream, and it is not in the manual. The RFC is
`BAND-TRANSFER`, then `READ` or `WRITE`, the band, an optional subset, a
size and a comment. The band, which is a disk partition, is copied off
or onto the machine (`sys2/band.lisp:16`, registered at `:286`).
`BAND-TRANSFER-SERVER-ON` decides whether a transfer is allowed
(`sys2/band.lisp:3`). Its default, `:NOTIFY`, allows it and tells the
user (`:45`). `T` allows it without telling anyone. `NIL` refuses it,
but only while someone is logged in (`:23`).

### HKDUMP

HKDUMP is a stream, and it is not in the manual. It is the "Hack Dump
Server", a dump of the machine's file system, `FULL` or `INCREMENTAL`
according to the contact name's argument. It streams file names, byte
sizes and authors, and it marks each file as dumped when it is told
`:OK` (`tape/fdump-def.lisp:146`, registered at `:155`).

### LRTDP

LRTDP is a stream in System 304 only, and it is not in the manual. It is
the remote tape device: another machine drives this machine's tape
(**(304)** `tape/remote-tape-device.lisp:110`, registered at `:116`;
1986).

## Out of scope

The **Internet gateway** (§Internet Gateway) carries Chaosnet streams to
TCP, which is a path to another network. **DOVER** (§Dover) sends a
press file to the Dover printer at MIT through a protocol translator.
