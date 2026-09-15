# CHUDP

CHUDP carries Chaosnet packets over UDP, and it is the only way ozd
reaches another host (`DESIGN.md` §2). This document describes the
framing exactly as ozd implements it, which is the framing `cbridge` uses.
It also says how that was established, so that nobody needs to consult the
sources again.

## The datagram

A CHUDP datagram carries one Chaos packet. The default UDP port is 42042.

| bytes | field |
|---|---|
| 0 | version, 1 |
| 1 | function, 1: a Chaos packet |
| 2–3 | 0 and 0 |
| 4–19 | the packet's eight header words (AIM-628 §3.5) |
| 20– | the packet's data, as whole 16-bit words |
| last 6 | the trailer's three words: destination, source, check word |

**Every 16-bit word is written most significant byte first**, in the
header, in the data and in the trailer.

**The header words** are AIM-628's. The opcode is the high byte of word 0.
Word 1 holds the forwarding count in its top 4 bits and the data byte count
in its low 12 bits. Then come the destination address and index, the
source address and index, the packet number and the acknowledgement.

**The data** is packed into 16-bit words as AIM-628 §3.6 says: the first
byte of each pair is the least significant half of its word. Because the
words are then written most significant byte first, each pair of data
bytes appears swapped on the wire. `STATUS` goes out as `TSTASU`. An odd
byte count is padded to a whole word with a zero byte in the high half of
the last word, which is the byte that comes first on the wire. `cbridge`
always pads.

**The trailer** holds the same three words that the CADR's interface adds
on its cable. The first is the address the packet is sent to on this
subnet, which is the next hop and not necessarily the header's
destination. The second is the sender's own address. The third is the
check word. In CHUDP the check word is not the CADR's 9401 CRC-16. It is
the Internet checksum.

The version is 1. The protocol's author has said that a version 2 might
differ from version 1 in nothing but byte order, so ozd refuses any other
version by its number rather than reading it as version 1.

## The checksum

The checksum is the one's complement of the one's complement sum of every
16-bit word before it: the eight header words, the data words, the
trailer's destination and the trailer's source. A frame is good when all
of its words, the checksum included, sum to 0xFFFF in one's complement
arithmetic.

```
sum = 0
for w in header_words + data_words + [trailer_destination, trailer_source]:
    sum = sum + w
    sum = (sum & 0xFFFF) + (sum >> 16)
checksum = ~sum & 0xFFFF
```

`cbridge` checks it, and drops a frame whose words do not sum right with
"Bad checksum".

## Two examples

An RFC for `STATUS` from 3040 to 3050, source index 21, packet number 1,
sent to 3050. Addresses and the index are octal. The datagram is 32
bytes, and `tests/frame.rs` pins the same bytes:

```
01 01 00 00          version 1, function 1, two zero bytes
01 00                opcode RFC (1) in the high byte
00 06                forwarding count 0, 6 data bytes
06 28 00 00          destination 3050, index 0
06 20 00 11          source 3040, index 21
00 01 00 00          packet number 1, acknowledgement 0
54 53 54 41 53 55    "STATUS", which is "TSTASU" on the wire
06 28 06 20          trailer: destination 3050, source 3040
ea 6d                check word
```

A frame that `cbridge` itself sent: its answer to a STATUS request, 94
bytes. The `cbridge` was a test instance at 177020 that named itself
`cbtest`, and the host asking was 177022. Its name appears as `bcetts` on
the wire, and its checksum verifies as above:

```
01 01 00 00 05 00 00 44 fe 12 00 12 fe 10 00 00 00 00 00 00
62 63 65 74 74 73 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
01 fe 00 10 00 02 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 fe 12 fe 10 c4 05
```

## What the CADR's own sources decide, and what they do not

The CADR's sources and netlist define the packet's words, how data bytes go
into them, and what its interface puts on its cable, including the 9401
CRC-16. They do not define how CHUDP lays those words out in a UDP
datagram, or which check word CHUDP carries. No CADR ever sent a UDP
packet, and for CHUDP `cbridge` is the reference.

So a program that connects a CADR, simulated or in an FPGA, to CHUDP
converts at that edge. A packet leaving the machine goes out with its words
in network order and the Internet checksum in the trailer. A frame
arriving has its Internet checksum checked, and if the machine's interface
checks a CRC on receipt, that CRC is made for it.

## How it was established

On 2026-09-15 a `cbridge` built from its current sources was run with a
test configuration on loopback, and observed through its own log and a
packet capture. Its code was not read; its author does not allow language
models to read it.

- Frames with their words least significant byte first were rejected as
  "bogus". `cbridge` reported the source address byte-swapped and the
  opcode as 0.
- Frames in network order with the CADR's CRC in the trailer were rejected
  with "Bad checksum". The value it printed was exactly the one's
  complement sum above, taken over the frame as sent.
- Frames in network order with the Internet checksum were accepted and
  forwarded. `cbridge`'s own frames, like the one above, verify with the
  same checksum.
- ozd, built with this framing and run behind that `cbridge`, answered a
  test host's STATUS, TIME and UPTIME through it, and served it a FILE
  listing. Neither side saw a bad checksum.

The protocol page at chaosnet.net/protocol (§2.3) also names an Internet
checksum. It says that `cbridge` sends words least significant byte first,
which the running `cbridge` does not.

## What else `cbridge` did

- When it passed a packet between two of its CHUDP peers on the same
  subnet, it added one to the forwarding count and put its own address in
  the trailer's source.
- It padded every odd data count.
- It sent no RUT packets over CHUDP within a minute.
- It did not pass packets between a host on another subnet and ozd in the
  test configuration. Whether that was the configuration or `cbridge` is
  **unverified**.

## In ozd

- `src/chudp.rs` has `PACKET_ORDER` and `TRAILER_ORDER`, both network
  order, and `checksum`, the Internet checksum. `wrap` and `unwrap` are the
  only places a word becomes bytes.
- `src/packet.rs` packs the data into words, low half first
  (`Packet::words`). Its `check_word` is the CADR's own CRC-16, which CHUDP
  does not carry.
- `tests/frame.rs` pins the example RFC above and two frames that `cbridge`
  sent. ozd reads them with a good checksum and writes them back byte for
  byte.
- ozd traces a frame whose check word is not the checksum, and handles the
  packet anyway. `cbridge` drops it.
- ozd also reads a frame whose odd data count is not padded, taking the
  lone last byte as the last data byte.

Before commit 4eb38f7, ozd wrote the packet's words least significant byte
first and put the CADR's CRC in the trailer. A host that still frames CHUDP
that way does not reach an ozd built from that commit or later.
