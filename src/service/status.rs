// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! STATUS: the host's name and its subnet meters.
//!
//! A simple transaction --- an RFC evokes an ANS and no connection --- and
//! not an optional one. AIM-628 §5: "All network nodes, even bridges, are
//! required to answer RFC's with contact name STATUS", and §5.1 again, "All
//! hosts are required to implement the STATUS protocol since it is used for
//! network maintenance". It is how one machine finds out whether another is
//! alive: `HOST-UP-P` in `sys/network/chaos/chsaux.lisp` asks for nothing
//! but the ANS coming back, and `HOSTAT` in the same file reads what is in
//! it.
//!
//! **Two sources fix the format and they agree.** AIM-628 §5.1 specifies it,
//! and MIT's own reader --- `HOSTAT-FORMAT-ANS` and `HOSTAT-FORMAT-ANS-1`
//! --- decodes it:
//!
//! | | |
//! |---|---|
//! | bytes 0..32 | the node's name, "padded on the right with zero bytes". `STRING-SEARCH-CHAR 0 (PKT-STRING PKT) 0 32.` is what finds the end |
//! | then, per subnet | an identifier word, a count word, and the meters |
//!
//! The count is the number of **16-bit words to follow**, not the number of
//! meters --- "usually 16" for the eight below, and the reader steps
//! `(+ I 2 CT)`. The identifier is "400 plus a subnet number", and the
//! meters after the first two words are 32-bit, low half first, which is
//! what `(DPB (AREF PKT (1+ J)) #o2020 (AREF PKT J))` assembles.
//!
//! An identifier of 0 to 0o377 is the same thing with 16-bit counts, which
//! the memo calls obsolete: "This format should no longer be sent by any
//! hosts." So this writes the 0o400 form. 0o1000 and up are reserved.
//!
//! **The meters are this host's own, counted at its socket.** Every one of
//! the eight counts something a network interface does, and this host has
//! no interface, only a UDP socket. The link counts at it what has a
//! meaning there, into the [`Meters`] it shares with this service
//! (`DESIGN.md` §7):
//!
//! - 1, every datagram received;
//! - 2, every datagram sent, this host's own and those passed on as the
//!   subnet's switch;
//! - 7, those rejected for their length;
//! - 8, those rejected for anything else.
//!
//! They are the places the Lisp Machine's own `SEND-STATUS` puts its
//! `PKTS-RECEIVED`, `PKTS-TRANSMITTED`, `PKTS-BAD-BIT-COUNT` and
//! `PKTS-OTHER-DISCARDED` (`sys/network/chaos/chsncp.lisp`). **3 to 6 are
//! zero, and that is not a placeholder**: there is no interface here to
//! abort a transmission on a collision or a busy receiver, to lose a packet
//! in because the one before had not been read out, or to fail a CRC, on
//! the wire or out of the packet buffer, so nought is the true count of
//! each. A CHUDP trailer's check word is not one of them: what a peer puts
//! there is unverified, and a mismatch is traced, not counted (`DESIGN.md`
//! §5). What the host answers is its name, which subnet it is on, and what
//! its socket has seen.

use crate::ncp::{Response, Service};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The 32-byte name field at the front of the answer.
const NAME: usize = 32;

/// The eight meters AIM-628 §5.1 defines, in its order --- which is also
/// `HOSTAT`'s columns, `#-in #-out abort lost crc ram bitc other`:
///
/// 1. packets received from this subnet;
/// 2. packets transmitted to it;
/// 3. transmissions aborted by collisions or a busy receiver;
/// 4. incoming packets lost because the host had not read the previous one
///    out of the interface;
/// 5. incoming packets with CRC errors;
/// 6. incoming packets that were clean on the wire but wrong out of the
///    packet buffer;
/// 7. incoming packets rejected for a length that is not a multiple of 16
///    bits;
/// 8. incoming packets rejected for anything else.
///
/// Sixteen words of them, which is the "usually 16" the memo gives for the
/// count field.
const METERS: usize = 8;

/// A subnet block's identifier is the subnet plus this; below it is the
/// older format, whose meters are 16 bits.
const SUBNET_BLOCK: u16 = 0o400;

/// The meters this host counts, one atomic each. The link counts into
/// them and STATUS reads them, so they are shared as an `Arc<Meters>`, a
/// [`Service`] being `Send` (`DESIGN.md` §7). Each is 32 bits, as a meter
/// is on the wire, and wraps as `fetch_add` does. Each is a count of its
/// own and nothing orders one against another, so `Ordering::Relaxed` is
/// enough to count and to read them.
#[derive(Debug, Default)]
pub struct Meters {
    /// Meter 1, `PKTS-RECEIVED`: every datagram received.
    pub received: AtomicU32,
    /// Meter 2, `PKTS-TRANSMITTED`: every datagram sent, this host's own
    /// and those passed on.
    pub transmitted: AtomicU32,
    /// Meter 7, `PKTS-BAD-BIT-COUNT`: datagrams rejected for their length.
    pub bad_bit_count: AtomicU32,
    /// Meter 8, `PKTS-OTHER-DISCARDED`: datagrams rejected for anything
    /// else.
    pub other_discarded: AtomicU32,
}

/// STATUS: a host's name and the meters of the subnets it is on.
pub struct Status {
    name: String,
    subnet: u8,
    meters: Arc<Meters>,
}

impl Status {
    /// Answering for a host of this name on this subnet, with the meters
    /// the link counts into. A Chaosnet address is the subnet in the high
    /// byte and the host in the low, so `0o3060` is subnet 6.
    pub fn new(name: &str, subnet: u8, meters: Arc<Meters>) -> Status {
        Status { name: name.to_string(), subnet, meters }
    }

    /// The answer's data, as `HOSTAT` reads it.
    fn answer(&self) -> Vec<u8> {
        let mut d = Vec::with_capacity(NAME + 4 + METERS * 4);
        // The name, truncated rather than overrunning the field, and padded
        // with nulls: MIT looks for a null to end it.
        let name = self.name.as_bytes();
        d.extend_from_slice(&name[..name.len().min(NAME - 1)]);
        d.resize(NAME, 0);
        // One subnet block. The count is in words, and a meter is two.
        d.extend_from_slice(&(SUBNET_BLOCK + self.subnet as u16).to_le_bytes());
        d.extend_from_slice(&((METERS * 2) as u16).to_le_bytes());
        // In `SEND-STATUS`'s places, each low half first and low byte first
        // within it, which is `to_le_bytes`; 3 to 6 are nought, as the
        // module documentation says.
        let m = &self.meters;
        let meters: [u32; METERS] = [
            m.received.load(Ordering::Relaxed),
            m.transmitted.load(Ordering::Relaxed),
            0,
            0,
            0,
            0,
            m.bad_bit_count.load(Ordering::Relaxed),
            m.other_discarded.load(Ordering::Relaxed),
        ];
        for meter in meters {
            d.extend_from_slice(&meter.to_le_bytes());
        }
        d
    }
}

impl Service for Status {
    fn contact(&self) -> &str {
        "STATUS"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Answer(self.answer())
    }
}
