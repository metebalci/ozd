// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chaosnet over UDP: the frame's bytes.
//!
//! The frame is pinned here as a byte sequence rather than left implicit
//! in the packing code, because its byte order is **unverified** ---
//! `chudp::PACKET_ORDER` and `chudp::TRAILER_ORDER` say what is believed
//! and why --- and a correction should be a change to two constants and
//! to one test.
//!
//! The frame has two copies, muir's and this one, and nothing in the
//! compiler makes them agree. So these are the first five tests of muir's
//! `tests/chudp.rs`, and the check word's from its `tests/chaos.rs`, with
//! the bytes unchanged: they are the contract between the two
//! repositories (`DESIGN.md` §11), and a correction is made in both.

use muir_ah::chudp;
use muir_ah::ncp::op;
use muir_ah::packet::{self, Framed, Packet};

/// A machine, and a peer over UDP, as muir's `tests/chudp.rs` names them
/// from the machine's side: 3050 is System 100's band, `MIT-LISPM-1` in
/// its `sys/site/hosts.text`, and 3040 a host beside it on subnet 6. The
/// frame between them is muir's, and its names come with its bytes.
const ME: u16 = 0o3050;
const PEER: u16 = 0o3040;

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

/// The source and check word the hardware adds to `buffer`.
fn trailer(buffer: &[u16], source: u16) -> (u16, u16) {
    let mut over = buffer.to_vec();
    over.push(source);
    (source, packet::check_word(&over))
}

/// **The frame is these bytes.** Four header bytes --- version 1,
/// function 1, two arguments --- then the Chaos packet's eight header
/// words and its data, each word least significant byte first, and then
/// the hardware trailer's destination, source and check word, each most
/// significant byte first.
///
/// The mixed order is what makes this worth pinning, and the data is
/// what shows it is not arbitrary: AIM-628 §3.6 puts the first byte of a
/// pair in the word's least significant half, so `STATUS` comes out of a
/// little-endian frame as `STATUS` and out of a big-endian one as
/// `TSTASU`. Bytes 20 to 25 below read `STATUS`.
///
/// The check word `0o171007` is the CADR's own hardware CRC-16 over
/// these words, `packet::check_word`; what a CHUDP peer puts in that
/// field is unverified, and nothing here or in `chudp` drops a packet on
/// it.
#[test]
fn the_frame_is_these_bytes() {
    let p = status_rfc();
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    assert_eq!(check, 0o171007, "the hardware's check word for these words");
    let frame = chudp::wrap(&buffer, source, check).expect("a frame");
    #[rustfmt::skip]
    let want: [u8; 32] = [
        // version, function, and two argument bytes
        0x01, 0x01, 0x00, 0x00,
        0x00, 0x01, // opcode: RFC in the high byte of the word
        0x06, 0x00, // count: no forwarding, six data bytes
        0x28, 0x06, // destination 3050
        0x00, 0x00, // destination index
        0x20, 0x06, // source 3040
        0x11, 0x00, // source index 21
        0x01, 0x00, // packet number
        0x00, 0x00, // acknowledge
        b'S', b'T', b'A', b'T', b'U', b'S',
        0x06, 0x28, // the trailer, in network order: destination 3050
        0x06, 0x20, // source 3040
        0xf2, 0x07, // check word 171007
    ];
    assert_eq!(frame, want, "the frame as it goes into the datagram");
    assert_eq!(&frame[20..26], b"STATUS", "the data reads in order, which is the packet's order");
}

/// **The frame goes out and comes back.** What `wrap` writes, `unwrap`
/// reads: the buffer with the cable destination last, the source, and
/// the check word, which is the hardware's here and so checks good.
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
/// number of 16-bit words on the cable, so this pads it; the trailer is
/// found from the end of the datagram rather than from the count, so a
/// peer that does not pad is read all the same. Which a peer does is
/// **unverified**; one interoperation settles it, and until then neither
/// reading is refused.
#[test]
fn an_odd_data_count_is_taken_padded_or_not() {
    let p = Packet { data: b"odd".to_vec(), ..status_rfc() };
    let buffer = p.to_buffer(ME);
    let (source, check) = trailer(&buffer, PEER);
    let padded = chudp::wrap(&buffer, source, check).expect("a frame");
    assert_eq!(padded.len(), 4 + 16 + 4 + 6, "the data padded to a word");
    // The same frame with the pad byte taken out.
    let mut unpadded = padded.clone();
    unpadded.remove(4 + 16 + 3);
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
            let err = chudp::unwrap(&bad).expect_err("{what} {value} is refused");
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

// --- the check word -----------------------------------------------------

/// **The check word is the board's.** The words muir's board loopback
/// test writes (`a_packet_loops_back_through_the_board`, in muir's
/// `tests/chaos_netlist.rs`), with the source address the hardware
/// appends, and the check word muir's netlist board produced for them:
/// `135771`. Of the 9401's polynomials, the two bit orders and the two
/// seeds, only CRC-16 from a cleared register over the words
/// most-significant bit first gives it.
///
/// From muir's `tests/chaos.rs`. It needs no board: the words and the
/// word the board made for them are written down.
#[test]
fn the_check_word_is_the_boards() {
    let words = [0o400, 0o4, 0o3050, 0, 0o3050, 0o21, 1, 0, 0o44524, 0o42515, 0o3050, 0o3050];
    assert_eq!(packet::check_word(&words), 0o135771);
}
