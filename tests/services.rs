// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The services of stage 1 but FILE, each asked through the NCP as a band
//! asks it --- STATUS as `HOSTAT` reads it, TIME against the system clock,
//! UPTIME in the sixtieths the band divides by --- and the Lisp Machine
//! character set that FILE, HOSTAB and NAME speak.
//!
//! From muir's `tests/chaos.rs` where muir has the test. STATUS's meters
//! and UPTIME's unit are this host's own (`DESIGN.md` §7).

mod support;

use ozd::lispm::{self, NEWLINE};
use ozd::ncp::{Ncp, op};
use ozd::packet::Packet;
use ozd::service::status::{Meters, Status};
use ozd::service::time::{Time, Uptime};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};
use support::arriving;

/// Asks `contact` of the host at 3060, from index 7 of 3050, at `now`: the
/// RFC in, and the ANS that comes back.
fn ask(h: &mut Ncp, now: u64, contact: &str) -> Packet {
    let rfc = Packet {
        opcode: op::RFC,
        forward: 0,
        dest: 0o3060,
        dest_index: 0,
        source: 0o3050,
        source_index: 7,
        number: 0o1234,
        ack: 0,
        data: contact.as_bytes().to_vec(),
    };
    h.receive(now, &arriving(&rfc));
    let ans = h.transmit(now).map(|b| Packet::from_buffer(&b).unwrap().0).expect("an answer");
    assert_eq!(ans.opcode, op::ANS, "{contact} is a simple transaction");
    ans
}

/// **STATUS is a simple transaction, and HOSTAT is what reads it.**
///
/// AIM-628 §5: an RFC to `STATUS` evokes an ANS carrying the server's name in
/// the first 32 bytes, padded with nulls, and then one block per subnet the
/// host has meters for. The format of a block is not guesswork --- MIT's own
/// reader is `HOSTAT-FORMAT-ANS-1` in `sys/network/chaos/chsaux.lisp`, and
/// this asserts exactly what it decodes:
///
/// - the name is the bytes up to the first null in the first 32
///   (`STRING-SEARCH-CHAR 0 ... 0 32.`);
/// - a block begins at data word 16, which is where its `(I 24. ...)`
///   starts once the eight header words are taken off;
/// - a block is an identifier word and a count word, and the count is in
///   *16-bit words*, so the loop steps `(+ I 2 CT)`;
/// - an identifier of `0o400` and up is a subnet block for subnet
///   `ID - 0o400`, whose meters are 32 bits each, low word first ---
///   `(DPB (AREF PKT (1+ J)) #o2020 (AREF PKT J))`.
///
/// The meters are this host's own, counted by the link into the [`Meters`]
/// it shares with the service (`DESIGN.md` §7), in the order MIT's own
/// `SEND-STATUS` sends them (`sys/network/chaos/chsncp.lisp`): 1 received,
/// 2 transmitted, 7 rejected for length, 8 rejected for anything else; 3 to
/// 6 are zero.
#[test]
fn status_answers_what_hostat_reads() {
    let meters = Arc::new(Meters::default());
    meters.received.store(0o1001, Ordering::Relaxed);
    meters.transmitted.store(0x0001_0002, Ordering::Relaxed);
    meters.bad_bit_count.store(3, Ordering::Relaxed);
    meters.other_discarded.store(0xfedc_ba98, Ordering::Relaxed);
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Status::new("MIT-OZ", 6, meters)));
    let ans = ask(&mut h, 100, "STATUS");

    assert_eq!((ans.dest, ans.dest_index), (0o3050, 7), "back to the asker's index");
    assert_eq!(ans.ack, 0o1234, "acknowledging the RFC");
    assert_eq!(h.connections(), 0, "no connection was made");

    // The name: bytes to the first null, within the first 32.
    let d = &ans.data;
    assert!(d.len() >= 32, "the name field is 32 bytes");
    let end = d[..32].iter().position(|&b| b == 0).expect("null-terminated within 32");
    assert_eq!(&d[..end], b"MIT-OZ");
    assert!(d[end..32].iter().all(|&b| b == 0), "padded with nulls, not spaces");

    // One subnet block, read the way HOSTAT reads it.
    let word = |i: usize| u16::from_le_bytes([d[2 * i], d[2 * i + 1]]) as usize;
    let (id, count) = (word(16), word(17));
    assert_eq!(id, 0o400 + 6, "subnet 6, in the 0o400 form");
    assert_eq!(count % 2, 0, "a 32-bit meter is two words");
    assert_eq!(d.len(), 32 + 4 + count * 2, "the block is all there is");

    let meter = |k: usize| word(18 + 2 * k) | word(19 + 2 * k) << 16;
    let meters: Vec<usize> = (0..count / 2).map(meter).collect();
    assert_eq!(
        meters,
        [0o1001, 0x0001_0002, 0, 0, 0, 0, 3, 0xfedc_ba98],
        "the eight in SEND-STATUS's order, each low word first"
    );
}

/// **A long name is cut to leave its null.** HOSTAT finds the end of the
/// name at the first null within 32 bytes (`HOSTAT-FORMAT-ANS`,
/// `sys/network/chaos/chsaux.lisp`), so a name of 32 bytes or more is cut
/// to 31 and the null is always there; the block starts where it always
/// does.
#[test]
fn status_cuts_a_long_name_to_leave_its_null() {
    let long = "A-NAME-LONGER-THAN-THE-THIRTY-TWO-BYTES-OF-THE-FIELD";
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Status::new(long, 6, Arc::new(Meters::default()))));
    let d = ask(&mut h, 0, "STATUS").data;
    assert_eq!(&d[..31], &long.as_bytes()[..31]);
    assert_eq!(d[31], 0, "the null HOSTAT looks for");
    assert_eq!(u16::from_le_bytes([d[32], d[33]]), 0o400 + 6, "and the block after it");
}

/// **The meters are read when STATUS is asked**, not when the service was
/// made: the link counts into the same [`Meters`] for as long as the host
/// runs, and each answer carries what it has counted by then.
#[test]
fn status_meters_are_what_the_link_has_counted() {
    let meters = Arc::new(Meters::default());
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Status::new("MIT-OZ", 6, meters.clone())));
    let meter =
        |d: &[u8], k: usize| u32::from_le_bytes(d[36 + 4 * k..40 + 4 * k].try_into().unwrap());
    let d = ask(&mut h, 0, "STATUS").data;
    assert_eq!((0..8).map(|k| meter(&d, k)).collect::<Vec<_>>(), [0; 8], "nothing counted yet");
    meters.received.fetch_add(5, Ordering::Relaxed);
    meters.transmitted.fetch_add(4, Ordering::Relaxed);
    meters.bad_bit_count.fetch_add(2, Ordering::Relaxed);
    meters.other_discarded.fetch_add(1, Ordering::Relaxed);
    let d = ask(&mut h, 10, "STATUS").data;
    assert_eq!((0..8).map(|k| meter(&d, k)).collect::<Vec<_>>(), [5, 4, 0, 0, 0, 0, 2, 1]);
}

/// **TIME is the system clock, as universal time**: seconds since midnight
/// GMT, 1 January 1900, least significant byte first (AIM-628 §5.8), within
/// a second of the clock read here (`DESIGN.md` §11).
#[test]
fn time_is_the_system_clock_in_universal_time() {
    // 1900 to 1970: seventy years of 365 days and seventeen leap days, 1904
    // to 1968 --- 1900 was not a leap year.
    let epoch = (70 * 365 + 17) * 86_400;
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::new()));
    let ans = ask(&mut h, 0, "TIME");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    assert_eq!(ans.data.len(), 4, "four bytes");
    let answered = u32::from_le_bytes(ans.data[..4].try_into().unwrap());
    let clock = (unix + epoch) as u32;
    assert!(clock.abs_diff(answered) <= 1, "answered {answered}, the clock says {clock}");
}

/// **UPTIME is in sixtieths of a second, and the manual is wrong about
/// it.** The band's own server sends `(* 60. (- (TIME:GET-UNIVERSAL-TIME)
/// TIME:*UT-AT-BOOT-TIME*))`, and both its user ends, `HOST-UPTIME` and
/// `UPTIME`, divide what comes back by 60 (`UPTIME-SERVER` and the two
/// after it, `sys/network/chaos/chsaux.lisp`). The manual's "an interval
/// (in seconds)" is what muir's server answers, and a band asking it prints
/// a sixtieth of the real uptime. Ten seconds up is 600, exactly, at ten
/// seconds of the test's clock (`DESIGN.md` §11); muir's answers 10.
#[test]
fn uptime_is_in_sixtieths_of_a_second() {
    let second = 1_000_000_000;
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Uptime::new(0)));
    assert_eq!(ask(&mut h, 10 * second, "UPTIME").data, 600u32.to_le_bytes(), "ten seconds");
    // From when the host came up, and a sixtieth, 16,666,666⅔ ns, counts
    // once it is whole.
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Uptime::new(5 * second)));
    assert_eq!(ask(&mut h, 15 * second, "UPTIME").data, 600u32.to_le_bytes());
    assert_eq!(ask(&mut h, 15 * second + 16_666_666, "UPTIME").data, 600u32.to_le_bytes());
    assert_eq!(ask(&mut h, 15 * second + 16_666_667, "UPTIME").data, 601u32.to_le_bytes());
    // Asked before it came up, it answers nothing rather than underflowing.
    assert_eq!(ask(&mut h, second, "UPTIME").data, 0u32.to_le_bytes());
}

/// **UPTIME wraps at 32 bits**, the four bytes it has --- about 828 days of
/// sixtieths (`DESIGN.md` §7) --- and nothing overflows on the way there,
/// even at the last nanosecond a `u64` holds.
#[test]
fn uptime_wraps_at_32_bits() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Uptime::new(0)));
    // 71,582,789 seconds is 4,294,967,340 sixtieths: 44 past 2^32.
    let ans = ask(&mut h, 71_582_789 * 1_000_000_000, "UPTIME");
    assert_eq!(ans.data, 44u32.to_le_bytes());
    // u64::MAX ns is 1,106,804,644,422 sixtieths: 2,998,049,350 past 257
    // wraps.
    let ans = ask(&mut h, u64::MAX, "UPTIME");
    assert_eq!(ans.data, 2_998_049_350u32.to_le_bytes());
}

/// The character translation is `FILE.c`'s `to_lispm`, both ways round
/// the 0200 boundary.
#[test]
fn unix_text_becomes_lisp_machine_text() {
    assert_eq!(lispm::to_lispm(b"a\nb\tc"), [b'a', 0o215, b'b', 0o211, b'c']);
    assert_eq!(lispm::to_lispm(&[0o215, 0o377, 0o15, 0o177]), [0o15, 0o177, 0o212, 0o377]);
}

/// The character translation back, `FILE.c`'s `from_lispm`, and that it
/// undoes [`lispm::to_lispm`] on ordinary text.
#[test]
fn lisp_machine_text_becomes_unix_text() {
    assert_eq!(lispm::from_lispm(&[b'a', NEWLINE, b'b', 0o211, b'c']), b"a\nb\tc");
    assert_eq!(lispm::from_lispm(&[0o212]), [0o15]);
    assert_eq!(lispm::from_lispm(&[0o12]), [0o212]);
    for text in [&b"plain\ntext\n"[..], b"tabs\there\n", b"\x08\x0c\x7f"] {
        assert_eq!(lispm::from_lispm(&lispm::to_lispm(text)), text, "{text:?} round trips");
    }
}

/// **Protocol text is a byte a character, not UTF-8.** The Lisp Machine's
/// newline is 0o215, a continuation byte in UTF-8: read as UTF-8 it is
/// replaced, and every line after the first is lost --- muir's FILE lost
/// an OPEN's pathname so (muir's `tests/chaos.rs`,
/// `the_file_service_serves_files_and_directories`). [`lispm::from_bytes`]
/// and [`lispm::lispm_text`] keep it, each the other's inverse; a
/// character past a byte goes out as `?`.
#[test]
fn protocol_text_is_a_byte_a_character() {
    let bytes = [b'O', b'Z', NEWLINE, b'L', b'M', b'1', NEWLINE];
    let text = lispm::from_bytes(&bytes);
    assert_eq!(text.split(NEWLINE as char).collect::<Vec<_>>(), ["OZ", "LM1", ""]);
    assert_eq!(lispm::lispm_text(&text), bytes, "and back, byte for byte");
    assert_ne!(String::from_utf8_lossy(&bytes), text, "which UTF-8 would not keep");
    assert_eq!(lispm::lispm_text("λ"), b"?");
}
