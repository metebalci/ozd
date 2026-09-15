// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chaosnet over UDP: the frame's bytes.
//!
//! The frame is pinned here as a byte sequence rather than left implicit
//! in the packing code, and beside it two frames that `cbridge` itself
//! sent, captured on 2026-09-15: CHUDP is `cbridge`'s convention, and these
//! are its bytes. Every 16-bit word goes most significant byte first, the
//! trailer's too, and the trailer's third word is an Internet checksum
//! (`chudp::PACKET_ORDER`, `chudp::TRAILER_ORDER`, `chudp::checksum`). The
//! check word the CADR's own hardware makes, which CHUDP does not carry, is
//! pinned below them.

use ozd::chudp;
use ozd::ncp::op;
use ozd::packet::{self, Framed, Packet};

/// A machine, and a peer over UDP, named from the machine's side: 3050 is
/// System 100's band, `MIT-LISPM-1` in its `sys/site/hosts.text`, and 3040
/// a host beside it on subnet 6.
const ME: u16 = 0o3050;
const PEER: u16 = 0o3040;

/// Bytes written as a capture prints them, in hexadecimal pairs.
fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|b| u8::from_str_radix(b, 16).expect("a hex byte")).collect()
}

// --- the frame ----------------------------------------------------------

/// One known packet: an RFC for `STATUS` from [`PEER`] to [`ME`], six
/// bytes of data. Its buffer, cable destination last, as the software
/// writes it.
fn status_rfc() -> Packet {
    Packet {
        opcode: op::RFC,
        forward: 0,
        dest: ME,
        dest_index: 0,
        source: PEER,
        source_index: 0o21,
        number: 1,
        ack: 0,
        data: b"STATUS".to_vec(),
    }
}

/// The source, and the checksum a CHUDP peer puts in the trailer, for
/// `buffer`.
fn trailer(buffer: &[u16], source: u16) -> (u16, u16) {
    let mut over = buffer.to_vec();
    over.push(source);
    (source, chudp::checksum(&over))
}

/// **The frame is these bytes.** Four header bytes --- version 1,
/// function 1, two arguments --- then the Chaos packet's eight header
/// words and its data, then the trailer's destination, source and check
/// word, every word most significant byte first.
///
/// The data shows the packing: AIM-628 §3.6 puts the first byte of a pair
/// in the word's least significant half, so in network order `STATUS` goes
/// out as `TSTASU`, just as `cbridge`'s own name goes out as `bcetts` below.
///
/// The check word `0o165155` is the Internet checksum of the words, the
/// trailer's destination and source among them.
#[test]
fn the_frame_is_these_bytes() {
    let p = status_rfc();
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    assert_eq!(check, 0o165155, "the Internet checksum of these words");
    let frame = chudp::wrap(&buffer, source, check).expect("a frame");
    #[rustfmt::skip]
    let want: [u8; 32] = [
        // version, function, and two argument bytes
        0x01, 0x01, 0x00, 0x00,
        0x01, 0x00, // opcode: RFC in the high byte of the word
        0x00, 0x06, // count: no forwarding, six data bytes
        0x06, 0x28, // destination 3050
        0x00, 0x00, // destination index
        0x06, 0x20, // source 3040
        0x00, 0x11, // source index 21
        0x00, 0x01, // packet number
        0x00, 0x00, // acknowledge
        b'T', b'S', b'T', b'A', b'S', b'U',
        0x06, 0x28, // the trailer: destination 3050
        0x06, 0x20, // source 3040
        0xea, 0x6d, // checksum 165155
    ];
    assert_eq!(frame, want, "the frame as it goes into the datagram");
    assert_eq!(
        &frame[20..26],
        b"TSTASU",
        "each pair of data bytes swapped, as the words hold them"
    );
}

/// **`cbridge`'s own frames read, and are written back byte for byte.**
/// Two datagrams a `cbridge` sent to a test host on 2026-09-15, as a
/// capture recorded them: its answer to a STATUS request, and its refusal
/// of a contact name it has no server for. The `cbridge` was 177020 on
/// subnet 376 and called itself `cbtest`; the host asking was 177022.
///
/// Each reads with its checksum good and its data the right way round, and
/// `wrap` makes the same bytes again from what `unwrap` read, which is what
/// makes this host's frames `cbridge`'s. The refusal's 31 bytes of data go
/// out in 32: `cbridge` pads an odd count.
#[test]
fn cbridges_own_frames_read_and_are_written_back() {
    let ans = hex(
        "01 01 00 00 05 00 00 44 fe 12 00 12 fe 10 00 00 00 00 00 00
         62 63 65 74 74 73 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         01 fe 00 10 00 02 00 00 00 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00 00 00 00 fe 12 fe 10 c4 05",
    );
    assert_eq!(ans.len(), 94);
    let f = chudp::unwrap(&ans).expect("cbridge's ANS reads");
    assert!(f.check_ok, "its checksum is good");
    let (p, cable) = Packet::from_buffer(&f.buffer).expect("a packet");
    assert_eq!(p.opcode, op::ANS);
    assert_eq!((p.dest, p.dest_index, p.source, p.source_index), (0o177022, 0o22, 0o177020, 0));
    assert_eq!((cable, f.source), (0o177022, 0o177020), "the trailer's destination and source");
    assert_eq!(&p.data[..8], b"cbtest\0\0", "its name, in the first 32 bytes");
    let word = |i: usize| u16::from_le_bytes([p.data[i], p.data[i + 1]]);
    assert_eq!(word(32), 0o400 + 0o376, "a block for subnet 376");
    assert_eq!(word(34), 16, "of sixteen words");
    assert_eq!((word(36), word(40)), (2, 1), "two packets received and one sent");
    assert_eq!(chudp::wrap(&f.buffer, f.source, f.check).expect("a frame"), ans, "written back");
    assert_eq!(trailer(&f.buffer, f.source).1, f.check, "and its checksum made again");

    let cls = hex(
        "01 01 00 00 03 00 00 1f fe 12 00 11 fe 10 00 00 45 68 00 00
         6f 4e 73 20 72 65 65 76 20 72 6f 66 20 72 68 74 73 69 63 20 6e 6f 61 74 74 63 6e 20 6d 61 00 65
         fe 12 fe 10 f5 5d",
    );
    let f = chudp::unwrap(&cls).expect("cbridge's CLS reads");
    assert!(f.check_ok, "its checksum is good");
    let (p, _) = Packet::from_buffer(&f.buffer).expect("a packet");
    assert_eq!(p.opcode, op::CLS);
    assert_eq!(p.data, b"No server for this contact name", "31 bytes, the right way round");
    assert_eq!(
        chudp::wrap(&f.buffer, f.source, f.check).expect("a frame"),
        cls,
        "written back, padded"
    );
}

/// **The frame goes out and comes back.** What `wrap` writes, `unwrap`
/// reads: the buffer with the cable destination last, the source, and
/// the check word, which is the checksum here and so checks good.
#[test]
fn a_frame_goes_out_and_comes_back() {
    for data in [Vec::new(), b"STATUS".to_vec(), b"odd".to_vec(), vec![0xff; 488]] {
        let p = Packet { data, ..status_rfc() };
        let buffer = p.to_buffer(ME);
        let (source, check) = trailer(&buffer, PEER);
        let frame = chudp::wrap(&buffer, source, check).expect("a frame");
        let back = chudp::unwrap(&frame).expect("it reads back");
        assert_eq!(back, Framed { buffer, source, check, check_ok: true }, "{p:?}");
        assert_eq!(Packet::from_buffer(&back.buffer).expect("a packet").0, p);
    }
}

/// **An odd data count is taken padded or not.** The data is a whole
/// number of 16-bit words, so this pads it, as `cbridge` does. The pad is
/// the last word's high half, which network order sends first, so a peer
/// that left it out would send the last data byte alone; the trailer is
/// found from the end of the datagram rather than from the count, and that
/// lone byte is read as the low half it belongs in.
#[test]
fn an_odd_data_count_is_taken_padded_or_not() {
    let p = Packet { data: b"odd".to_vec(), ..status_rfc() };
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    let padded = chudp::wrap(&buffer, source, check).expect("a frame");
    assert_eq!(padded.len(), 4 + 16 + 4 + 6, "the data padded to a word");
    assert_eq!(&padded[4 + 16..4 + 16 + 4], [b'd', b'o', 0, b'd'], "the pad first in its word");
    // The same frame with the pad byte taken out.
    let mut unpadded = padded.clone();
    unpadded.remove(4 + 16 + 2);
    assert_eq!(unpadded.len(), 4 + 16 + 3 + 6);
    let a = chudp::unwrap(&padded).expect("padded reads");
    let b = chudp::unwrap(&unpadded).expect("unpadded reads too");
    assert_eq!(a.buffer, b.buffer, "and to the same words");
    assert_eq!(Packet::from_buffer(&b.buffer).expect("a packet").0.data, b"odd");
}

/// **A version this does not speak is refused, and so is a function.**
/// The protocol's author has said a version 2 may differ from version 1
/// in nothing but byte order, so a version 2 peer read as a version 1
/// one would exchange nonsense; it is refused by number instead.
#[test]
fn an_unknown_version_is_refused() {
    let p = status_rfc();
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    let good = chudp::wrap(&buffer, source, check).expect("a frame");
    assert!(chudp::unwrap(&good).is_ok());
    for (byte, what) in [(0, "version"), (1, "function")] {
        for value in [0u8, 2, 255] {
            let mut bad = good.clone();
            bad[byte] = value;
            let Err(err) = chudp::unwrap(&bad) else { panic!("{what} {value} is refused") };
            assert!(err.contains(what), "the refusal says which: {err}");
        }
    }
}

/// **A length that does not answer the data count is refused**, which is
/// what a wrong byte order looks like on the first packet: the count
/// word comes out of the other half and no longer accounts for the
/// datagram.
#[test]
fn a_length_that_is_not_the_count_is_refused() {
    let p = status_rfc();
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    let good = chudp::wrap(&buffer, source, check).expect("a frame");
    let mut short = good.clone();
    short.truncate(good.len() - 2);
    chudp::unwrap(&short).expect_err("two bytes fewer than the count wants");
    let mut long = good.clone();
    long.extend([0, 0]);
    chudp::unwrap(&long).expect_err("two more");
    // The count word swapped, which is the wrong order for that field.
    let mut swapped = good.clone();
    swapped.swap(6, 7);
    chudp::unwrap(&swapped).expect_err("an absurd count");
    chudp::unwrap(&good[..HEADER_AND_TRAILER]).expect_err("nothing but a header and a trailer");
}

/// The header and the trailer with no packet between them: shorter than
/// any frame.
const HEADER_AND_TRAILER: usize = 4 + 6;

// --- the check words ----------------------------------------------------

/// **The checksum is the Internet checksum.** The one's complement of the
/// one's complement sum of the words, so that the words and the checksum
/// together sum to all ones: what `cbridge` checks, and what it printed as
/// "Bad checksum" for a frame that failed, which is how its kind was found.
#[test]
fn the_checksum_makes_the_words_sum_to_all_ones() {
    let p = status_rfc();
    let mut words = p.to_buffer(ME);
    words.push(PEER);
    let check = chudp::checksum(&words);
    let mut sum = 0u32;
    for w in words.iter().chain([&check]) {
        sum += *w as u32;
        sum = (sum & 0xffff) + (sum >> 16);
    }
    assert_eq!(sum, 0xffff, "the words and the checksum sum to all ones");
    assert_eq!(chudp::checksum(&[]), 0xffff, "and nothing sums to nothing");
}

/// **The check word is the board's.** The words of a packet looped back
/// through the board, with the source address the hardware appends, and
/// the check word a simulation of the board's netlist produced for them:
/// `135771`, **unverified** against a board. Of the 9401's polynomials,
/// the two bit orders and the two seeds, only CRC-16 from a cleared
/// register over the words most-significant bit first gives it. It is what
/// the CADR puts on its own cable, and not what CHUDP carries.
///
/// It needs no board: the words and the word made for them are written
/// down.
#[test]
fn the_check_word_is_the_boards() {
    let words = [0o400, 0o4, 0o3050, 0, 0o3050, 0o21, 1, 0, 0o44524, 0o42515, 0o3050, 0o3050];
    assert_eq!(packet::check_word(&words), 0o135771);
}
