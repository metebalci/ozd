// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chaosnet over UDP: the frame, and --- when they are written --- the
//! link and the hub.
//!
//! CHUDP puts one Chaosnet packet in one UDP datagram behind a four-byte
//! header, and it is what `cbridge`, `usim`, `klh10` and the live
//! Chaosnet hosts speak to each other. Ordinary UDP: no privileges, and
//! it crosses a NAT --- which IP protocol 16, the assigned number for
//! Chaosnet, does not.
//!
//! The frame half of muir's `src/chaos/udp.rs` is copied here with its
//! documentation and every **unverified** mark: the constants, [`Order`],
//! [`PACKET_ORDER`], [`TRAILER_ORDER`], [`wrap`] and [`unwrap`]. Where
//! that documentation spoke of muir's modelled cable or its flags, it
//! speaks of this host's. muir's link is a node on that cable and does
//! not come. The link and the hub --- the socket, the table of endpoints,
//! and a packet for another host of the subnet passed on untouched ---
//! are written later, in this module (`DESIGN.md` §5).
//!
//! ## The frame
//!
//! ```text
//! offset  width  field
//!      0      1  version
//!      1      1  function
//!      2      2  two argument bytes
//! ---- the Chaos packet, AIM-628 §3.5, in 16-bit words ----
//!      4      2  operation
//!      6      2  count: 4-bit forwarding count, 12-bit data count
//!      8      2  destination address
//!     10      2  destination index
//!     12      2  source address
//!     14      2  source index
//!     16      2  packet number
//!     18      2  acknowledge
//!     20      n  data, n the byte count rounded up to a whole word
//! ---- the hardware trailer, AIM-628 §2.2 ----
//! 20 + n      2  destination
//! 22 + n      2  source
//! 24 + n      2  check
//! ```
//!
//! An odd byte count is padded to a whole word here, the cable carrying
//! whole words; [`unwrap`] finds the trailer from the end of the
//! datagram rather than from the count, so a peer that does not pad is
//! read all the same. Which a peer does is **unverified**.
//!
//! `version = 1` and `function = 1`, for "here is a Chaos packet", which
//! is described as the only function defined. The default port is 42042.
//! Read from the Wireshark dissector published at
//! `gist.github.com/ams/6bde1da514479e27c9f70c161b5537c1` and the
//! protocol page at `chaosnet.net/protocol`, cross-read against the
//! Computer History Wiki's Chaosnet page. **Not** from
//! `bictorv/chaosnet-bridge`, the reference implementation, whose author
//! forbids language models to read or process it; that is the author's
//! decision about the author's own work and it is kept here.
//!
//! [`crate::packet::Packet`] is the eight header words and the data, and
//! a [`Framed`] is its buffer, cable destination last, with the source
//! and the check word the hardware adds --- which is the hardware
//! trailer, in the trailer's own order. What this module adds is the
//! four-byte wrapper and, with the link, the socket.
//!
//! ## Byte order, which is **unverified**
//!
//! [`PACKET_ORDER`] and [`TRAILER_ORDER`] say it, and are the only place
//! a word becomes bytes; the protocol's author has said a version 2 may
//! differ from version 1 in nothing but byte order, so that should be
//! two constants to change rather than an audit of the packing.
//! [`VERSION`] is checked on receipt for the same reason: an unknown
//! version is refused with its number rather than parsed as this one.

use crate::packet::{Framed, MAX_DATA, check_word};

/// The port CHUDP is spoken on unless the config's `listen` names another
/// (`DESIGN.md` §5).
pub const PORT: u16 = 42042;

/// The version this speaks, and the only one it takes: a datagram
/// carrying any other is refused rather than read as this one.
pub const VERSION: u8 = 1;

/// The function code for "here is a Chaos packet", described as the only
/// one defined.
pub const PACKET: u8 = 1;

/// The CHUDP header: version, function, and two argument bytes, which
/// this sends as zero and does not read.
pub const HEADER: usize = 4;

/// The hardware trailer, AIM-628 §2.2: destination, source, check.
pub const TRAILER: usize = 6;

/// The software header, AIM-628 §3.5: eight 16-bit words.
const SOFTWARE_HEADER: usize = 16;

/// The most a CHUDP frame can be: the header, the software header,
/// [`MAX_DATA`] bytes of data, and the trailer. A datagram longer than
/// this cannot be a Chaos packet, so the receive buffer is one byte
/// more --- a longer datagram then fills it, and is refused for its
/// length rather than read as a truncated packet.
pub const MAX_FRAME: usize = HEADER + SOFTWARE_HEADER + MAX_DATA + TRAILER;

/// Which end of a 16-bit word goes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Least significant byte first.
    Little,
    /// Most significant byte first, which is network order.
    Big,
}

impl Order {
    /// `w` appended to `out` in this order.
    fn put(self, w: u16, out: &mut Vec<u8>) {
        out.extend(match self {
            Order::Little => w.to_le_bytes(),
            Order::Big => w.to_be_bytes(),
        });
    }

    /// The word the first two bytes of `b` make in this order.
    fn word(self, b: &[u8]) -> u16 {
        let pair = [b[0], b[1]];
        match self {
            Order::Little => u16::from_le_bytes(pair),
            Order::Big => u16::from_be_bytes(pair),
        }
    }

    /// The word a lone trailing byte makes: it is the half that comes
    /// first, and the other half is not there.
    fn odd(self, b: u8) -> u16 {
        match self {
            Order::Little => b as u16,
            Order::Big => (b as u16) << 8,
        }
    }
}

/// How the Chaos packet's own 16-bit words are laid out: least
/// significant byte first.
///
/// **Unverified.** The protocol's own documentation says so in as many
/// words, and says of it "I'm really sorry about this, and might develop
/// version 2 of the protocol with the only change being big-endian byte
/// order"; the Wireshark dissector reads its 16-bit fields in a way that
/// appears big-endian, and the two are reconciled by [`TRAILER_ORDER`]
/// below rather than by either being wrong. What it also fits: AIM-628
/// §3.6 puts "the first 8-bit byte in a 16-bit word ... in the
/// arithmetically least-significant position", so the data bytes of a
/// packet come out of a little-endian frame in the order they were
/// written and out of a big-endian one swapped in pairs.
///
/// What would settle it: a capture of a live exchange, or one
/// interoperation. The wrong order fails loudly on the first packet ---
/// an absurd 12-bit data count against the datagram's length, and
/// addresses that match nothing configured --- so it does not fail
/// quietly.
pub const PACKET_ORDER: Order = Order::Little;

/// How the hardware trailer's three words are laid out: network order.
///
/// **Unverified.** The reading is that CHUDP is a mixed frame: the
/// reference implementation was read by a person as taking the trailer
/// through `ntohs` --- `srctrailer = ntohs(tr->ch_hw_srcaddr)` --- and
/// not the packet's own words, which is what makes the mixture the
/// likely reading rather than a guess. How the packet's bytes are
/// assembled there was not traced, so this is belief and not knowledge.
///
/// What would settle it: the same capture or interoperation. One whole
/// packet's bytes are pinned in `tests/frame.rs`, as in muir's
/// `tests/chudp.rs`, so a correction is a change to these two constants
/// and to that one test, in both.
pub const TRAILER_ORDER: Order = Order::Big;

/// A frame as it would stand on a cable, as a CHUDP datagram.
///
/// `buffer` is the buffer the software wrote, its last word the cable
/// destination, and `source` and `check` are the two words the hardware
/// added --- which is exactly [`Framed`]. `None` if there is not even a
/// destination word to put in the trailer.
pub fn wrap(buffer: &[u16], source: u16, check: u16) -> Option<Vec<u8>> {
    let (&dest, words) = buffer.split_last()?;
    let mut out = Vec::with_capacity(HEADER + buffer.len() * 2 + TRAILER);
    out.extend([VERSION, PACKET, 0, 0]);
    for &w in words {
        PACKET_ORDER.put(w, &mut out);
    }
    for w in [dest, source, check] {
        TRAILER_ORDER.put(w, &mut out);
    }
    Some(out)
}

/// The frame back out of a datagram, as a cable would have handed it to
/// an interface: the buffer with the cable destination last, and the
/// source and check word the far side's hardware added.
///
/// `check_ok` is that check word against the one the CADR's own
/// hardware would have made for these words, [`check_word`]. **Nothing
/// is dropped on it**: what a CHUDP peer puts in the trailer's third
/// word is unverified --- the hardware trailer's is the 9401's CRC-16,
/// and the trailer has also been described as carrying an Internet
/// checksum --- so this reports the answer and leaves the packet alone.
/// A trace of a run against a real peer settles it --- muir's
/// `--chaos-trace`, or `--trace` here (`DESIGN.md` §10) --- and until
/// then a mismatch on a packet for this host is traced and not dropped
/// (`DESIGN.md` §5).
///
/// The data is a whole number of 16-bit words on the cable, and this
/// takes both a peer that pads an odd byte count to a word and one that
/// does not: the trailer is found from the end of the datagram, so where
/// it starts is not a guess, and the length is then held to one of the
/// two.
pub fn unwrap(datagram: &[u8]) -> Result<Framed, String> {
    if datagram.len() > MAX_FRAME {
        return Err(format!("{} bytes is longer than any Chaos packet", datagram.len()));
    }
    if datagram.len() < HEADER + SOFTWARE_HEADER + TRAILER {
        return Err(format!("{} bytes is too short for a packet", datagram.len()));
    }
    let version = datagram[0];
    if version != VERSION {
        return Err(format!("version {version}, and this speaks {VERSION}"));
    }
    let function = datagram[1];
    if function != PACKET {
        return Err(format!("function {function}, and only {PACKET} carries a packet"));
    }
    let body = &datagram[HEADER..datagram.len() - TRAILER];
    let count = (PACKET_ORDER.word(&body[2..]) & 0o7777) as usize;
    if count > MAX_DATA {
        return Err(format!("a data count of {count}, and the most is {MAX_DATA}"));
    }
    if body.len() != SOFTWARE_HEADER + count.next_multiple_of(2)
        && body.len() != SOFTWARE_HEADER + count
    {
        return Err(format!("{} bytes of packet against a data count of {count}", body.len()));
    }
    let mut buffer: Vec<u16> = body
        .chunks(2)
        .map(|c| match c {
            [_, _] => PACKET_ORDER.word(c),
            _ => PACKET_ORDER.odd(c[0]),
        })
        .collect();
    let trailer = &datagram[datagram.len() - TRAILER..];
    let dest = TRAILER_ORDER.word(&trailer[0..]);
    let source = TRAILER_ORDER.word(&trailer[2..]);
    let check = TRAILER_ORDER.word(&trailer[4..]);
    buffer.push(dest);
    let mut over = buffer.clone();
    over.push(source);
    let check_ok = check_word(&over) == check;
    Ok(Framed { buffer, source, check, check_ok })
}
