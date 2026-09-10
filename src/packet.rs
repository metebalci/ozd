// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A packet as the software lays it out, and the check word.
//!
//! AIM-628 §3.5: the software header is eight 16-bit words --- operation,
//! count, destination address and index, source address and index, packet
//! number, acknowledgement --- followed by the data, up to 488 bytes. §7
//! says how the interface takes it: the eight header words, the data
//! words, then the cable destination as the last word written, the
//! hardware adding the source and the check word itself. A CHUDP datagram
//! carries exactly that, the hardware's three words as its trailer
//! ([`crate::chudp`]).

/// The most data a packet carries, AIM-628 §3.5: "the maximum value is
/// 488".
pub const MAX_DATA: usize = 488;

/// The software header and data, AIM-628 §3.5.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// "The most-significant 8 bits of this word are the Opcode."
    pub opcode: u8,
    /// The forwarding count, the top four bits of the count word.
    pub forward: u8,
    pub dest: u16,
    pub dest_index: u16,
    pub source: u16,
    pub source_index: u16,
    pub number: u16,
    pub ack: u16,
    /// The data, "8-bit bytes"; the byte count is its length.
    pub data: Vec<u8>,
}

impl Packet {
    /// The eight header words and the data words, low byte of each pair
    /// first, AIM-628 §3.6: "The first 8-bit byte in a 16-bit word is the
    /// one in the arithmetically least-significant position." An odd last
    /// byte sits in the low half with "a garbage padding byte in its high
    /// half", §7; the padding here is zero.
    pub fn words(&self) -> Vec<u16> {
        let mut w = vec![
            (self.opcode as u16) << 8,
            (self.forward as u16) << 12 | (self.data.len() as u16 & 0o7777),
            self.dest,
            self.dest_index,
            self.source,
            self.source_index,
            self.number,
            self.ack,
        ];
        for pair in self.data.chunks(2) {
            w.push(pair[0] as u16 | (pair.get(1).copied().unwrap_or(0) as u16) << 8);
        }
        w
    }

    /// What the software writes into the transmit buffer: the words and
    /// then the cable destination, AIM-628 §7.
    pub fn to_buffer(&self, cable_dest: u16) -> Vec<u16> {
        let mut w = self.words();
        w.push(cable_dest);
        w
    }

    /// The packet back out of a buffer as the software reads it: the
    /// header, the data, and the cable destination last.
    pub fn from_buffer(buffer: &[u16]) -> Result<(Packet, u16), String> {
        if buffer.len() < 9 {
            return Err(format!(
                "{} words is too short for a header and a destination",
                buffer.len()
            ));
        }
        let count = (buffer[1] & 0o7777) as usize;
        let data_words = count.div_ceil(2);
        if buffer.len() != 8 + data_words + 1 {
            return Err(format!(
                "a byte count of {count} wants {} words, not {}",
                8 + data_words + 1,
                buffer.len()
            ));
        }
        let mut data = Vec::with_capacity(count);
        for w in &buffer[8..8 + data_words] {
            data.push(*w as u8);
            data.push((*w >> 8) as u8);
        }
        data.truncate(count);
        Ok((
            Packet {
                opcode: (buffer[0] >> 8) as u8,
                forward: (buffer[1] >> 12) as u8,
                dest: buffer[2],
                dest_index: buffer[3],
                source: buffer[4],
                source_index: buffer[5],
                number: buffer[6],
                ack: buffer[7],
                data,
            },
            buffer[8 + data_words],
        ))
    }
}

/// The check word the interface puts on a packet: the Fairchild 9401 at
/// LMTBUF C09 dividing by CRC-16, `x^16 + x^15 + x^2 + 1`, its select
/// pins grounded, from a cleared register, over the buffer's words in the
/// order written --- header, data, cable destination, then the source the
/// hardware adds --- each word most-significant bit first, which is the
/// order the two 74165s at B12 and B13 shift a word out.
///
/// That is not read off a document: it is the one arrangement that
/// reproduces the word a simulation of the board's netlist produced for a
/// packet it looped back (`tests/frame.rs`, `the_check_word_is_the_boards`),
/// and is **unverified** against a board. It is what this host puts in a
/// CHUDP trailer.
pub fn check_word(words: &[u16]) -> u16 {
    let mut r = 0u32; // stage k in bit k
    for &w in words {
        for k in (0..16).rev() {
            let d = (w >> k) & 1 != 0;
            let fb = d ^ (r >> 15 & 1 != 0);
            let mut next = (r << 1) & 0xffff;
            if fb {
                next ^= 1 | 1 << 2 | 1 << 15;
            }
            r = next;
        }
    }
    r as u16
}

/// A packet as a CHUDP datagram delivers it: the buffer its sender wrote,
/// and the two words the sender's hardware would have added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framed {
    /// The buffer as the software would read it back: the words as
    /// written, cable destination last.
    pub buffer: Vec<u16>,
    /// The source address the sending interface put in.
    pub source: u16,
    /// The check word as received, and whether it is the right one.
    pub check: u16,
    pub check_ok: bool,
}
