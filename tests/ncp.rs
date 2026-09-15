// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The NCP, held to AIM-628 chapters 3 and 4 without a socket: each packet
//! handed to [`Ncp::receive`] as the link would hand it, what the NCP sends
//! taken off [`Ncp::transmit`], and the clock set by the test.
//!
//! The transport as a band uses it; then its corners --- a duplicate RFC, one
//! packet in flight, a connection opened from this end, a bad check word,
//! what is not this host's, and a packet for no connection. Last, the log:
//! the line [`Ncp::log`] is given for each connection opened, refused and
//! closed (`DESIGN.md` §10).

mod support;

use ozd::log::Names;
use ozd::ncp::{self, Ncp, Out, Response, Service, Session, op};
use ozd::packet::{self, Framed, Packet};
use ozd::service::time::Time;
use std::sync::{Arc, Mutex};
use support::arriving;

fn rfc(from: (u16, u16), to: u16, number: u16, text: &str) -> Packet {
    Packet {
        opcode: op::RFC,
        forward: 0,
        dest: to,
        dest_index: 0,
        source: from.0,
        source_index: from.1,
        number,
        ack: 0,
        data: text.as_bytes().to_vec(),
    }
}

/// What the host sends next, as a packet.
fn next_from(h: &mut Ncp, now: u64) -> Option<Packet> {
    h.transmit(now).map(|b| Packet::from_buffer(&b).unwrap().0)
}

fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// **TIME is a simple transaction.** AIM-628 §5.8: an RFC to `TIME`
/// evokes an ANS with the universal time in four bytes, least
/// significant first, and no connection. The ANS goes back to the
/// asker's index, acknowledging the RFC.
#[test]
fn time_answers_a_simple_transaction() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(0x1234_5678)));
    h.receive(100, &arriving(&rfc((0o3050, 7), 0o3060, 0o1234, "TIME")));
    let ans = next_from(&mut h, 100).expect("an answer");
    assert_eq!(ans.opcode, op::ANS);
    assert_eq!((ans.dest, ans.dest_index), (0o3050, 7), "back to the asker's index");
    assert_eq!(ans.source, 0o3060);
    assert_eq!(ans.ack, 0o1234, "acknowledging the RFC");
    assert_eq!(ans.data, [0x78, 0x56, 0x34, 0x12], "least significant byte first");
    assert_eq!(next_from(&mut h, 100), None, "and nothing more");
    assert_eq!(h.connections(), 0, "no connection was made");
    // A packet for another host is not ours.
    h.receive(200, &arriving(&rfc((0o3050, 8), 0o3070, 1, "TIME")));
    assert_eq!(next_from(&mut h, 200), None);
    // An unknown contact is refused with a CLS.
    h.receive(300, &arriving(&rfc((0o3050, 9), 0o3060, 2, "NOSUCH")));
    let cls = next_from(&mut h, 300).expect("a refusal");
    assert_eq!(cls.opcode, op::CLS);
    assert_eq!((cls.dest, cls.dest_index), (0o3050, 9));
    assert!(String::from_utf8_lossy(&cls.data).contains("NOSUCH"), "with the reason");
    // A broadcast TIME is answered too, §4.5: "The TIME and STATUS protocols
    // ... will work through BRD packets"; the subnet bit map is skipped.
    let mut brd = rfc((0o3050, 10), 0, 3, "TIME");
    brd.opcode = op::BRD;
    brd.ack = 4;
    brd.data = [vec![0xff, 0xff, 0xff, 0xff], b"TIME".to_vec()].concat();
    h.receive(400, &arriving(&brd));
    let ans = next_from(&mut h, 400).expect("an answer to a broadcast");
    assert_eq!((ans.opcode, ans.dest_index), (op::ANS, 10));
}

/// A service that echoes what it is sent, for the stream protocol.
struct Echo;
struct EchoSession {
    pending: Vec<Out>,
    got_eof: bool,
}
impl Service for Echo {
    fn contact(&self) -> &str {
        "ECHO"
    }
    fn request(&mut self, _now: u64, args: &str, _from: (u16, u16)) -> Response {
        if args == "NO" {
            return Response::Refuse("Not today".into());
        }
        if args == "LONG" {
            return Response::Refuse("not today, and at length ".repeat(40));
        }
        Response::Accept(Box::new(EchoSession { pending: Vec::new(), got_eof: false }))
    }
}
impl Session for EchoSession {
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.pending.push(Out::Data(bytes.to_vec()));
    }
    fn eof(&mut self, _now: u64) {
        self.got_eof = true;
        self.pending.push(Out::Eof);
    }
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.pending)
    }
}

/// **A stream connection opens, moves data both ways and closes**, as
/// AIM-628 §4.1 to §4.4 lay it out: RFC, then the server's OPN carrying
/// its index, initial packet number and window and acknowledging the
/// RFC; the user's STS; numbered data acknowledged in the header of what
/// goes back or in an STS; EOF answered by EOF; CLS.
#[test]
fn a_stream_opens_moves_data_and_closes() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let me = (0o3050, 0o21);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!(opn.opcode, op::OPN);
    assert_eq!((opn.dest, opn.dest_index), me);
    assert_ne!(opn.source_index, 0, "the server's index");
    assert_eq!(opn.ack, 100, "acknowledging the RFC");
    let receipt = u16::from_le_bytes([opn.data[0], opn.data[1]]);
    let window = u16::from_le_bytes([opn.data[2], opn.data[3]]);
    assert_eq!(receipt, 100, "the OPN's data is a receipt");
    assert!(window >= 1, "and a window");
    assert_eq!(h.connections(), 1);
    let server = (0o3060, opn.source_index);
    let mut number = 100u16;
    let mut sts = Packet {
        opcode: op::STS,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number: number + 1,
        ack: opn.number,
        data: Vec::new(),
    };
    sts.data.extend_from_slice(&opn.number.to_le_bytes());
    sts.data.extend_from_slice(&5u16.to_le_bytes());
    h.receive(10, &arriving(&sts));
    assert_eq!(next_from(&mut h, 10), None, "an STS wants nothing back");
    // Data in: echoed back, numbered from the OPN's number on, and
    // acknowledging ours.
    number += 1;
    let dat = Packet {
        opcode: op::DAT,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number,
        ack: opn.number,
        data: b"hello".to_vec(),
    };
    h.receive(20, &arriving(&dat));
    let echo = next_from(&mut h, 20).expect("the echo");
    assert_eq!(echo.opcode, op::DAT);
    assert_eq!(echo.data, b"hello");
    assert_eq!(echo.number, opn.number.wrapping_add(1), "numbered after the OPN");
    assert_eq!(echo.ack, number, "acknowledging our data");
    assert_eq!(next_from(&mut h, 20), None, "the acknowledgement rode on the echo, so no STS");
    // A duplicate of our data draws an STS with a receipt, not another echo.
    h.receive(30, &arriving(&dat));
    let s = next_from(&mut h, 30).expect("an STS for the duplicate");
    assert_eq!(s.opcode, op::STS);
    assert_eq!(u16::from_le_bytes([s.data[0], s.data[1]]), number, "receipting through our packet");
    // EOF in, EOF back.
    number += 1;
    let eof = Packet {
        opcode: op::EOF,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number,
        ack: echo.number,
        data: Vec::new(),
    };
    h.receive(40, &arriving(&eof));
    let back = next_from(&mut h, 40).expect("an EOF back");
    assert_eq!(back.opcode, op::EOF);
    assert_eq!(back.ack, number);
    // CLS ends it.
    let cls = Packet {
        opcode: op::CLS,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number: number + 1,
        ack: back.number,
        data: b"done".to_vec(),
    };
    h.receive(50, &arriving(&cls));
    assert_eq!(h.connections(), 0, "closed");
    // A refusal.
    h.receive(60, &arriving(&rfc((0o3050, 0o22), 0o3060, 200, "ECHO NO")));
    let cls = next_from(&mut h, 60).unwrap();
    assert_eq!((cls.opcode, cls.dest_index), (op::CLS, 0o22));
    assert_eq!(cls.data, b"Not today");
    // Unreceipted packets go again after half a second.
    h.receive(70, &arriving(&rfc((0o3050, 0o23), 0o3060, 300, "ECHO")));
    let opn2 = next_from(&mut h, 70).unwrap();
    assert_eq!(next_from(&mut h, 70 + ncp::RETRANSMIT_NS - 1), None);
    let again = next_from(&mut h, 70 + ncp::RETRANSMIT_NS).expect("retransmitted");
    assert_eq!((again.opcode, again.number), (op::OPN, opn2.number));
}

/// **A peer that falls silent is given up after the band's own interval.**
/// Nothing in the transport freed a connection whose other end had gone:
/// its unreceipted packets went again every half second for ever, and an
/// RFC from the same host and index was a duplicate for ever --- which is
/// what a reboot of the same band sends, its indices seeded from a clock
/// the simulator repeats. `chsncp.lisp`'s `PROBE-CONN` puts a connection
/// in `HOST-DOWN-STATE` once nothing has been received on it for
/// `HOST-DOWN-INTERVAL`, three minutes; the NCP does the same, and a fresh
/// RFC after that is a fresh connection.
#[test]
fn a_silent_peer_is_freed_after_the_host_down_interval() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let me = (0o3050, 0o21);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!((opn.opcode, opn.ack), (op::OPN, 100));
    assert_eq!(h.connections(), 1);
    // Nothing comes back. The OPN goes again to the end of the interval.
    let t = ncp::HOST_DOWN_NS;
    assert_eq!(next_from(&mut h, t).map(|p| p.opcode), Some(op::OPN), "still trying");
    assert_eq!(h.connections(), 1, "and still there");
    // Past it: freed, and quiet.
    assert_eq!(next_from(&mut h, t + 1), None);
    assert_eq!(h.connections(), 0, "given up");
    // The same host and index again is a new connection, not a duplicate.
    h.receive(t + 10, &arriving(&rfc(me, 0o3060, 700, "ECHO")));
    let opn = next_from(&mut h, t + 10).expect("an OPN for the new connection");
    assert_eq!((opn.opcode, opn.ack), (op::OPN, 700));
    assert_eq!(h.connections(), 1);
    // A packet from the peer starts the interval again: an STS two minutes
    // in keeps the connection past the three.
    let heard = t + 10 + 120_000_000_000;
    let mut sts = Packet {
        opcode: op::STS,
        forward: 0,
        dest: 0o3060,
        dest_index: opn.source_index,
        source: me.0,
        source_index: me.1,
        number: 701,
        ack: opn.number,
        data: Vec::new(),
    };
    sts.data.extend_from_slice(&opn.number.to_le_bytes());
    sts.data.extend_from_slice(&5u16.to_le_bytes());
    h.receive(heard, &arriving(&sts));
    assert_eq!(next_from(&mut h, t + 10 + ncp::HOST_DOWN_NS + 1), None);
    assert_eq!(h.connections(), 1, "heard from within the interval");
    assert_eq!(next_from(&mut h, heard + ncp::HOST_DOWN_NS + 1), None);
    assert_eq!(h.connections(), 0, "and not since");
}

/// **A refusal fits in a packet.** A CLS quotes its reason, and one that
/// quotes the RFC's contact name back can run past the 488 bytes a packet
/// carries (AIM-628 §3.5); the count word is twelve bits, so the frame went
/// out longer than the interface's buffer. The reason is cut to fit, from
/// the transport and from a service alike.
#[test]
fn a_refusal_fits_in_a_packet() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let name = "Z".repeat(packet::MAX_DATA);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, &name)));
    let cls = next_from(&mut h, 0).expect("a refusal");
    assert_eq!((cls.opcode, cls.dest_index), (op::CLS, 0o21));
    assert!(cls.data.len() <= packet::MAX_DATA, "{} bytes of reason", cls.data.len());
    assert!(cls.to_buffer(0o3050).len() <= 8 + packet::MAX_DATA / 2 + 1, "within the buffer");
    assert!(text(&cls.data).starts_with("No server for contact name Z"), "and still the reason");
    h.receive(10, &arriving(&rfc((0o3050, 0o22), 0o3060, 2, "ECHO LONG")));
    let cls = next_from(&mut h, 10).expect("the service's refusal");
    assert_eq!((cls.opcode, cls.dest_index), (op::CLS, 0o22));
    assert!(cls.data.len() <= packet::MAX_DATA, "{} bytes of reason", cls.data.len());
    assert!(text(&cls.data).starts_with("not today, and at length "));
}

/// An STS from `me` to the connection at `server`: in its data a receipt
/// through `receipt` and a window, low byte first, and the same receipt in
/// its acknowledgement field.
fn sts(me: (u16, u16), server: (u16, u16), number: u16, receipt: u16, window: u16) -> Packet {
    let mut data = receipt.to_le_bytes().to_vec();
    data.extend_from_slice(&window.to_le_bytes());
    Packet {
        opcode: op::STS,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number,
        ack: receipt,
        data,
    }
}

/// **A duplicate RFC is discarded**, AIM-628 §4.1: "an NCP receives an RFC
/// packet, it checks all pending RFC's and all connections which are in
/// the Open or RFC-received state, to see if the source address and index
/// match; if so, the RFC is a duplicate and is discarded." What goes again
/// is this end's OPN, on its own retransmission; there is no second
/// connection. Another index of the same host is another connection.
#[test]
fn a_duplicate_rfc_is_discarded() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let me = (0o3050, 0o21);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!(opn.opcode, op::OPN);
    h.receive(10, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    assert_eq!(next_from(&mut h, 10), None, "nothing for the duplicate");
    assert_eq!(h.connections(), 1, "and no second connection");
    h.receive(20, &arriving(&rfc((0o3050, 0o22), 0o3060, 200, "ECHO")));
    let other = next_from(&mut h, 20).expect("an OPN for the other index");
    assert_eq!((other.opcode, other.dest_index), (op::OPN, 0o22));
    assert_ne!(other.source_index, opn.source_index, "at an index of its own");
    assert_eq!(h.connections(), 2);
}

/// A service whose session has everything to say at once: three data
/// packets and an EOF, all offered on its first poll.
struct Burst;
struct BurstSession(Vec<Out>);
impl Service for Burst {
    fn contact(&self) -> &str {
        "BURST"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Accept(Box::new(BurstSession(vec![
            Out::Data(b"one".to_vec()),
            Out::Data(b"two".to_vec()),
            Out::Data(b"three".to_vec()),
            Out::Eof,
        ])))
    }
}
impl Session for BurstSession {
    fn data(&mut self, _now: u64, _op: u8, _bytes: &[u8]) {}
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.0)
    }
}

/// **One packet is in flight at a time, whatever window the other end
/// offers**: the far end is a CADR
/// whose interface holds one packet, and a burst up to the window was lost
/// into it. A session with three packets and an EOF to send at once, on a
/// connection whose far end offers a window of five, gets the first out;
/// each next one waits for the receipt of the one before, numbered one
/// past it.
#[test]
fn one_packet_is_in_flight_whatever_the_window() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Burst));
    let me = (0o3050, 0o21);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "BURST")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!(opn.opcode, op::OPN);
    assert_eq!(next_from(&mut h, 0), None, "nothing past the OPN until it is receipted");
    let server = (0o3060, opn.source_index);
    h.receive(10, &arriving(&sts(me, server, 101, opn.number, 5)));
    let mut last = opn.number;
    let want = [
        (op::DAT, &b"one"[..]),
        (op::DAT, &b"two"[..]),
        (op::DAT, &b"three"[..]),
        (op::EOF, &b""[..]),
    ];
    for (i, want) in want.into_iter().enumerate() {
        let now = 20 + 10 * i as u64;
        let p = next_from(&mut h, now).expect("the next packet");
        assert_eq!((p.opcode, p.data.as_slice()), want);
        assert_eq!(p.number, last.wrapping_add(1), "numbered one past the last");
        assert_eq!(next_from(&mut h, now), None, "and nothing else in flight");
        h.receive(now, &arriving(&sts(me, server, 102 + i as u16, p.number, 5)));
        last = p.number;
    }
    assert_eq!(next_from(&mut h, 100), None, "all sent, all receipted");
}

/// What has happened to a [`Recorder`], and what it is to send next:
/// shared between the test and the session the NCP holds.
#[derive(Clone, Default)]
struct Log {
    events: Arc<Mutex<Vec<String>>>,
    to_send: Arc<Mutex<Vec<Out>>>,
}

impl Log {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    fn send(&self, out: Out) {
        self.to_send.lock().unwrap().push(out);
    }
    fn note(&self, event: String) {
        self.events.lock().unwrap().push(event);
    }
}

/// A session that writes down what happens to it, and sends what the test
/// gives it to send.
struct Recorder(Log);

impl Session for Recorder {
    fn opened(&mut self, _now: u64) {
        self.0.note("opened".into());
    }
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.0.note(format!("data {}", text(bytes)));
    }
    fn eof(&mut self, _now: u64) {
        self.0.note("eof".into());
    }
    fn closed(&mut self, _now: u64, reason: &str) {
        self.0.note(format!("closed {reason}"));
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut *self.0.to_send.lock().unwrap())
    }
}

/// Carries every packet between two NCPs at `now`, as a link between them
/// would, until neither has anything more to send.
fn shuttle(a: &mut Ncp, b: &mut Ncp, now: u64) {
    for _ in 0..100 {
        let mut quiet = true;
        while let Some(p) = next_from(a, now) {
            b.receive(now, &arriving(&p));
            quiet = false;
        }
        while let Some(p) = next_from(b, now) {
            a.receive(now, &arriving(&p));
            quiet = false;
        }
        if quiet {
            return;
        }
    }
    panic!("the two NCPs never fell quiet");
}

/// **A connection is opened from this end**, and the NCP is symmetric:
/// one NCP's [`Ncp::connect`] and another's service are the two ends of one
/// stream --- which is how FILE calls a user end's data connection, and how
/// a test host opens its connections (`DESIGN.md` §11). The RFC carries the
/// contact name from an index of this end's and goes again until answered;
/// the OPN, from the far end's own index, opens it, and the session is told;
/// then data, EOF and CLS go as they do from the other side.
#[test]
fn a_connection_is_opened_from_this_end() {
    let mut far = Ncp::new(0o3060);
    far.serve(Box::new(Echo));
    let mut near = Ncp::new(0o3050);
    let log = Log::default();
    log.send(Out::Data(b"hello".to_vec()));
    log.send(Out::Eof);
    let index = near.connect(0, 0o3060, "ECHO", Box::new(Recorder(log.clone()))).expect("an index");
    assert_ne!(index, 0, "index 0 is never a connection");
    let rfc = next_from(&mut near, 0).expect("an RFC");
    assert_eq!(rfc.opcode, op::RFC);
    assert_eq!((rfc.dest, rfc.dest_index), (0o3060, 0), "to the host, at no index yet");
    assert_eq!((rfc.source, rfc.source_index), (0o3050, index));
    assert_eq!(rfc.data, b"ECHO");
    assert_eq!(next_from(&mut near, 0), None, "and nothing before the OPN");
    // Lost, it goes again after half a second.
    assert_eq!(next_from(&mut near, ncp::RETRANSMIT_NS - 1), None);
    let t = ncp::RETRANSMIT_NS;
    let again = next_from(&mut near, t).expect("the RFC again");
    assert_eq!((again.opcode, again.number), (op::RFC, rfc.number));
    far.receive(t, &arriving(&again));
    shuttle(&mut near, &mut far, t);
    assert_eq!(log.take(), ["opened", "data hello", "eof"], "opened, echoed, and ended");
    assert_eq!((near.connections(), far.connections()), (1, 1));
    // The OPN answered the RFC, and everything since is receipted.
    assert_eq!(next_from(&mut near, t + ncp::RETRANSMIT_NS), None, "nothing goes again");
    assert_eq!(next_from(&mut far, t + ncp::RETRANSMIT_NS), None);
    // This end closes it.
    log.send(Out::Close("done".into()));
    shuttle(&mut near, &mut far, t + ncp::RETRANSMIT_NS);
    assert_eq!((near.connections(), far.connections()), (0, 0), "closed at both ends");
}

/// **A connection asked for from this end can be refused, or answered.**
/// The far end's CLS ends it with the far end's reason; an ANS, the answer
/// of a simple transaction, ends it too. Either way the session hears why.
#[test]
fn a_connection_from_this_end_is_refused_or_answered() {
    let mut far = Ncp::new(0o3060);
    far.serve(Box::new(Echo));
    far.serve(Box::new(Time::fixed(1)));
    let mut near = Ncp::new(0o3050);
    let log = Log::default();
    near.connect(0, 0o3060, "ECHO NO", Box::new(Recorder(log.clone())));
    shuttle(&mut near, &mut far, 0);
    assert_eq!(log.take(), ["closed Not today"]);
    near.connect(10, 0o3060, "TIME", Box::new(Recorder(log.clone())));
    shuttle(&mut near, &mut far, 10);
    assert_eq!(log.take(), ["closed answered"]);
    assert_eq!((near.connections(), far.connections()), (0, 0));
}

/// **A connection asked for from this end hears only the host it asked.**
/// Until the OPN comes the other end's index is not known, so the OPN, a
/// refusal's CLS, an ANS or a FWD is taken whatever index it carries ---
/// but from the host the RFC went to, and from no other. A CLS or an OPN
/// from a third host at this end's index is no answer and changes nothing,
/// and the real OPN still opens the connection.
#[test]
fn a_connection_from_this_end_hears_only_the_host_it_asked() {
    let mut far = Ncp::new(0o3060);
    far.serve(Box::new(Echo));
    let mut near = Ncp::new(0o3050);
    let log = Log::default();
    let index = near.connect(0, 0o3060, "ECHO", Box::new(Recorder(log.clone()))).expect("an index");
    for opcode in [op::CLS, op::OPN] {
        let stranger = Packet {
            opcode,
            forward: 0,
            dest: 0o3050,
            dest_index: index,
            source: 0o3051,
            source_index: 9,
            number: 1,
            ack: 1,
            data: vec![1, 0, 5, 0],
        };
        near.receive(0, &arriving(&stranger));
    }
    assert!(log.take().is_empty(), "a third host's packets are no answer");
    shuttle(&mut near, &mut far, 0);
    assert!(!log.take().iter().any(|e| e.starts_with("closed")), "and nothing closed it");
    assert_eq!((near.connections(), far.connections()), (1, 1), "the real OPN opened it");
}

/// **An index is not given out again at once**, and a late packet for a
/// connection that has gone finds none: slots are taken round the table,
/// and a slot taken again carries a new uniquizer, as the machine's own NCP
/// takes them (`sys/network/chaos/chsncp.lisp`, `INDEX-CONN-FREE-POINTER`
/// and `UNIQUIZER-TABLE`). So a CLS that crossed this end's own, or data
/// retransmitted late, neither closes nor feeds the connection made since.
#[test]
fn a_late_packet_for_a_closed_connection_misses_the_next_one() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let opened = |h: &mut Ncp, from: u16, now: u64| -> u16 {
        h.receive(now, &arriving(&rfc((0o3050, from), 0o3060, 1, "ECHO")));
        let opn = next_from(h, now).expect("an OPN");
        assert_eq!(opn.opcode, op::OPN);
        opn.source_index
    };
    let late = |opcode: u8, to: u16, number: u16| Packet {
        opcode,
        forward: 0,
        dest: 0o3060,
        dest_index: to,
        source: 0o3050,
        source_index: 7,
        number,
        ack: 1,
        data: b"late".to_vec(),
    };
    let first = opened(&mut h, 7, 0);
    h.receive(10, &arriving(&late(op::CLS, first, 2)));
    assert_eq!(h.connections(), 0, "the first is closed");
    let second = opened(&mut h, 8, 20);
    assert_ne!(second, first, "the index just freed is not given out again");
    h.receive(30, &arriving(&late(op::CLS, first, 2)));
    assert_eq!(h.connections(), 1, "a late CLS for the first leaves the second open");
    h.receive(40, &arriving(&late(op::DAT, first, 3)));
    let los = next_from(&mut h, 40).expect("an answer to the late data");
    assert_eq!((los.opcode, los.dest_index), (op::LOS, 7), "no such connection, to its sender");
}

/// **A full table takes no more connections**: with every index in use, a
/// connection from this end is not made --- its session is told so, as a
/// close --- and an RFC from the other end is refused with a CLS, rather
/// than an index being given out twice.
#[test]
fn a_full_table_takes_no_more_connections() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Log::default();
    let mut made = 0;
    while h.connect(0, 0o3051, "FOO", Box::new(Recorder(log.clone()))).is_some() {
        made += 1;
        assert!(made < 5_000, "the table never fills");
    }
    assert!(log.take().iter().any(|e| e.starts_with("closed ")), "the session is told");
    while next_from(&mut h, 0).is_some() {}
    h.receive(0, &arriving(&rfc((0o3050, 7), 0o3060, 1, "ECHO")));
    let cls = next_from(&mut h, 0).expect("a refusal");
    assert_eq!((cls.opcode, cls.dest_index), (op::CLS, 7));
    assert_eq!(h.connections(), made, "no connection past the table's end");
}

/// **A bad check word does not lose the packet.** One that is not CHUDP's
/// checksum is traced and the packet handled (`chudp::unwrap`), since UDP
/// carries a checksum of its own (`DESIGN.md` §5).
#[test]
fn a_bad_check_word_is_still_handled() {
    let mut h = Ncp::new(0o3060);
    h.trace = true;
    h.serve(Box::new(Time::fixed(0x1234_5678)));
    let mut framed = arriving(&rfc((0o3050, 7), 0o3060, 1, "TIME"));
    framed.check ^= 1;
    framed.check_ok = false;
    h.receive(100, &framed);
    let ans = next_from(&mut h, 100).expect("answered all the same");
    assert_eq!((ans.opcode, ans.dest_index), (op::ANS, 7));
    assert_eq!(ans.data, [0x78, 0x56, 0x34, 0x12]);
}

/// **Only a packet for this host, or a BRD to 0, is taken**: another
/// host's packet is not this one's, a packet to
/// 0 is a broadcast only if it is a BRD, and a buffer that holds no packet
/// is dropped without a word.
#[test]
fn only_this_hosts_packets_and_brds_are_taken() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(1)));
    h.receive(0, &arriving(&rfc((0o3050, 1), 0o3070, 1, "TIME")));
    assert_eq!(next_from(&mut h, 0), None, "another host's");
    h.receive(0, &arriving(&rfc((0o3050, 2), 0, 1, "TIME")));
    assert_eq!(next_from(&mut h, 0), None, "an RFC to 0 is not a broadcast");
    h.receive(0, &Framed { buffer: vec![0; 4], source: 0o3050, check: 0, check_ok: false });
    assert_eq!(next_from(&mut h, 0), None, "no packet at all");
    let mut brd = rfc((0o3050, 3), 0, 1, "TIME");
    brd.opcode = op::BRD;
    h.receive(0, &arriving(&brd));
    let ans = next_from(&mut h, 0).expect("a BRD to 0 is answered");
    assert_eq!((ans.opcode, ans.dest, ans.dest_index), (op::ANS, 0o3050, 3));
}

/// **A packet for no connection draws a LOS**, AIM-628 §4.2: "LOS is sent
/// in response to situations such as: arrival of a data packet or an STS
/// for a connection that does not exist". A CLS or a LOS for no connection
/// is not answered.
#[test]
fn a_packet_for_no_connection_draws_a_los() {
    let mut h = Ncp::new(0o3060);
    let dat = Packet {
        opcode: op::DAT,
        forward: 0,
        dest: 0o3060,
        dest_index: 5,
        source: 0o3050,
        source_index: 0o21,
        number: 1,
        ack: 0,
        data: b"lost".to_vec(),
    };
    h.receive(0, &arriving(&dat));
    let los = next_from(&mut h, 0).expect("a LOS");
    assert_eq!(los.opcode, op::LOS);
    assert_eq!((los.dest, los.dest_index), (0o3050, 0o21), "to the sender's index");
    assert_eq!(los.source_index, 5, "from the index it named");
    assert_eq!(los.data, b"No such connection");
    for opcode in [op::CLS, op::LOS] {
        h.receive(0, &arriving(&Packet { opcode, ..dat.clone() }));
        assert_eq!(next_from(&mut h, 0), None, "{} is not answered", ncp::op_name(opcode));
    }
}

/// **A broadcast for a contact nobody here serves draws nothing.** The
/// machine's own NCP makes a BRD an RFC and hands it on with no CLS on
/// error: `RECEIVE-BRD` calls `(HANDLE-RFC-PKT PKT NIL)`, and
/// `HANDLE-RFC-PKT` sends "No server for this contact name" only "if
/// CLS-ON-ERROR-P is T" (`sys/network/chaos/chsncp.lisp:1588`, `:1613`).
/// A switch hears every machine's broadcasts; a refusal to each would go back
/// to every one of them. An RFC for the same contact is still refused.
#[test]
fn a_broadcast_nobody_serves_draws_nothing() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(0)));
    let mut brd = rfc((0o3050, 11), 0, 4, "NOSUCH");
    brd.opcode = op::BRD;
    brd.ack = 4;
    brd.data = [vec![0xff, 0xff, 0xff, 0xff], b"NOSUCH".to_vec()].concat();
    h.receive(100, &arriving(&brd));
    assert_eq!(next_from(&mut h, 100), None, "no refusal to a broadcast");
    assert_eq!(h.connections(), 0);
    h.receive(200, &arriving(&rfc((0o3050, 12), 0o3060, 5, "NOSUCH")));
    let cls = next_from(&mut h, 200).map(|p| p.opcode);
    assert_eq!(cls, Some(op::CLS), "an RFC is still refused");
}

/// What an NCP's log hook has been given, line by line.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    /// A hook on `h` that keeps each line it is given, and where they are
    /// kept.
    fn hook(h: &mut Ncp) -> Lines {
        let lines = Lines::default();
        let kept = lines.clone();
        h.log = Some(Arc::new(move |line: &str| kept.0.lock().unwrap().push(line.to_string())));
        lines
    }
    /// The lines given since the last take.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// No line at all.
const NOTHING: [&str; 0] = [];

/// **The log follows an accepted stream from its OPN to its close**
/// (`DESIGN.md` §10): a line when the RFC is accepted, naming the contact
/// and the other host in octal, and one when the other end's CLS closes
/// it, with its reason. What goes between --- a duplicate of the RFC, the
/// STS, data both ways, EOF --- is the trace's, not the log's.
#[test]
fn the_log_follows_an_accepted_stream_to_its_close() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Lines::hook(&mut h);
    let me = (0o3050, 0o21);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!(log.take(), ["ECHO from 3050 (?) opened"]);
    h.receive(5, &arriving(&rfc(me, 0o3060, 100, "ECHO")));
    let server = (0o3060, opn.source_index);
    h.receive(10, &arriving(&sts(me, server, 101, opn.number, 5)));
    let dat = Packet {
        opcode: op::DAT,
        forward: 0,
        dest: server.0,
        dest_index: server.1,
        source: me.0,
        source_index: me.1,
        number: 101,
        ack: opn.number,
        data: b"hello".to_vec(),
    };
    h.receive(20, &arriving(&dat));
    let echo = next_from(&mut h, 20).expect("the echo");
    assert_eq!((echo.opcode, echo.data.as_slice()), (op::DAT, &b"hello"[..]));
    let eof =
        Packet { opcode: op::EOF, number: 102, ack: echo.number, data: Vec::new(), ..dat.clone() };
    h.receive(30, &arriving(&eof));
    let back = next_from(&mut h, 30).expect("an EOF back");
    assert_eq!(back.opcode, op::EOF);
    assert_eq!(log.take(), NOTHING, "nothing between");
    let cls =
        Packet { opcode: op::CLS, number: 103, ack: back.number, data: b"done".to_vec(), ..dat };
    h.receive(40, &arriving(&cls));
    assert_eq!(h.connections(), 0);
    assert_eq!(log.take(), ["ECHO from 3050 (?) closed by its CLS: done"]);
}

/// **A LOS from the other end closes a connection as its CLS does**
/// (`Session::closed`), and the log says which of the two it was, with the
/// reason it carried.
#[test]
fn a_los_from_the_other_end_is_logged() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Lines::hook(&mut h);
    let me = (0o3050, 0o22);
    h.receive(0, &arriving(&rfc(me, 0o3060, 1, "ECHO")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    let los = Packet {
        opcode: op::LOS,
        forward: 0,
        dest: 0o3060,
        dest_index: opn.source_index,
        source: me.0,
        source_index: me.1,
        number: 0,
        ack: 0,
        data: b"No such connection".to_vec(),
    };
    h.receive(10, &arriving(&los));
    assert_eq!(h.connections(), 0);
    assert_eq!(
        log.take(),
        ["ECHO from 3050 (?) opened", "ECHO from 3050 (?) closed by its LOS: No such connection"]
    );
}

/// **A refusal is logged with its reason**: the CLS this end sends for an
/// RFC no service takes, and for one a service refuses, each naming the
/// host that asked. The contact name is the RFC's first word, without its
/// arguments, as a service is found by it.
#[test]
fn a_refusal_is_logged_with_its_reason() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "NOSUCH")));
    h.receive(10, &arriving(&rfc((0o3051, 0o22), 0o3060, 2, "ECHO NO")));
    assert_eq!(next_from(&mut h, 10).map(|p| p.opcode), Some(op::CLS));
    assert_eq!(next_from(&mut h, 10).map(|p| p.opcode), Some(op::CLS));
    assert_eq!(h.connections(), 0);
    assert_eq!(
        log.take(),
        [
            "NOSUCH from 3050 (?) refused: No server for contact name NOSUCH",
            "ECHO from 3051 (?) refused: Not today",
        ]
    );
}

/// **With `--log-simple`, each simple transaction answered is a line of the
/// log** (`DESIGN.md` §10), in the shape a connection's lines have: the
/// contact, the host that asked, and `answered`. Without it there is no
/// line, since an answer opens no connection for the log to follow.
#[test]
fn a_simple_transaction_answered_is_logged_when_asked() {
    let mut quiet = Ncp::new(0o3060);
    quiet.serve(Box::new(Time::fixed(0)));
    let quiet_log = Lines::hook(&mut quiet);
    quiet.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "TIME")));
    assert_eq!(next_from(&mut quiet, 0).map(|p| p.opcode), Some(op::ANS));
    assert_eq!(quiet_log.take(), NOTHING, "no line without --log-simple");

    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(0)));
    h.log_simple = true;
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "TIME")));
    assert_eq!(next_from(&mut h, 0).map(|p| p.opcode), Some(op::ANS));
    assert_eq!(log.take(), ["TIME from 3050 (?) answered"]);
}

/// **A line names a host by the host table as well as by its address**
/// (`DESIGN.md` §10): its official name in parentheses, and `?` for an
/// address the table does not hold --- in an answer's line and a
/// connection's alike.
#[test]
fn a_host_is_named_in_the_log_by_the_host_table() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(0)));
    h.names = Arc::new(Names::new([(0o3050, "MIT-LISPM-1")]));
    h.log_simple = true;
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "TIME")));
    h.receive(0, &arriving(&rfc((0o3051, 0o21), 0o3060, 1, "TIME")));
    h.receive(0, &arriving(&rfc((0o3050, 0o22), 0o3060, 1, "NOSUCH")));
    while next_from(&mut h, 0).is_some() {}
    assert_eq!(
        log.take(),
        [
            "TIME from 3050 (MIT-LISPM-1) answered",
            "TIME from 3051 (?) answered",
            "NOSUCH from 3050 (MIT-LISPM-1) refused: No server for contact name NOSUCH",
        ]
    );
}

/// **A connection opened from this end is logged when the OPN comes
/// back**, not when its RFC goes: until then nothing is open. With a log on
/// each of two NCPs, one's [`Ncp::connect`] and the other's service, each
/// end names the other host --- `to` it at the end that asked, `from` it at
/// the end that was asked --- and a CLS the asking end's session sends
/// closes it at both, with the session's reason.
#[test]
fn a_connection_opened_from_this_end_is_logged_at_both_ends() {
    let mut far = Ncp::new(0o3060);
    far.serve(Box::new(Echo));
    let far_log = Lines::hook(&mut far);
    let mut near = Ncp::new(0o3050);
    let near_log = Lines::hook(&mut near);
    let events = Log::default();
    near.connect(0, 0o3060, "ECHO", Box::new(Recorder(events.clone())));
    let rfc = next_from(&mut near, 0).expect("an RFC");
    assert_eq!(rfc.opcode, op::RFC);
    assert_eq!(near_log.take(), NOTHING, "an RFC is not yet a connection");
    far.receive(0, &arriving(&rfc));
    shuttle(&mut near, &mut far, 0);
    assert_eq!(events.take(), ["opened"]);
    assert_eq!(near_log.take(), ["ECHO to 3060 (?) opened"]);
    assert_eq!(far_log.take(), ["ECHO from 3050 (?) opened"]);
    events.send(Out::Close("done".into()));
    shuttle(&mut near, &mut far, 10);
    assert_eq!((near.connections(), far.connections()), (0, 0));
    assert_eq!(near_log.take(), ["ECHO to 3060 (?) closed by our CLS: done"]);
    assert_eq!(far_log.take(), ["ECHO from 3050 (?) closed by its CLS: done"]);
}

/// **An RFC from this end that the other end refuses is logged as
/// refused**, at both ends: it never opened, and the CLS that ends it is
/// the refusal the other end's service gave ([`Response::Refuse`]).
#[test]
fn a_refusal_of_this_ends_rfc_is_logged_at_both_ends() {
    let mut far = Ncp::new(0o3060);
    far.serve(Box::new(Echo));
    let far_log = Lines::hook(&mut far);
    let mut near = Ncp::new(0o3050);
    let near_log = Lines::hook(&mut near);
    near.connect(0, 0o3060, "ECHO NO", Box::new(Recorder(Log::default())));
    shuttle(&mut near, &mut far, 0);
    assert_eq!(near.connections(), 0);
    assert_eq!(near_log.take(), ["ECHO to 3060 (?) refused: Not today"]);
    assert_eq!(far_log.take(), ["ECHO from 3050 (?) refused: Not today"]);
}

/// **A connection given up for silence is logged as closed, host down**
/// ([`ncp::HOST_DOWN_NS`]): past the interval and not at it, whichever end
/// asked for it --- an RFC from this end that nothing ever answered is
/// given up the same way.
#[test]
fn a_host_down_expiry_is_logged() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "ECHO")));
    h.connect(0, 0o3051, "FOO", Box::new(Recorder(Log::default())));
    while next_from(&mut h, 0).is_some() {}
    assert_eq!(log.take(), ["ECHO from 3050 (?) opened"]);
    let t = ncp::HOST_DOWN_NS;
    while next_from(&mut h, t).is_some() {}
    assert_eq!((h.connections(), log.take()), (2, vec![]), "not at the interval");
    assert_eq!(next_from(&mut h, t + 1), None);
    assert_eq!(h.connections(), 0);
    assert_eq!(
        log.take(),
        [
            "ECHO from 3050 (?) closed, host down: nothing heard for 180 s",
            "FOO to 3051 (?) closed, host down: nothing heard for 180 s",
        ]
    );
}

/// **A simple transaction logs nothing**: an RFC answered with an ANS makes
/// no connection, and STATUS is asked that way over and over. Nor does a
/// broadcast, answered or let fall; nor a simple transaction this end
/// asks, which the other end's ANS ends.
#[test]
fn a_simple_transaction_logs_nothing() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Time::fixed(1)));
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 7), 0o3060, 1, "TIME")));
    assert_eq!(next_from(&mut h, 0).map(|p| p.opcode), Some(op::ANS));
    for contact in ["TIME", "NOSUCH"] {
        let mut brd = rfc((0o3050, 8), 0, 2, contact);
        brd.opcode = op::BRD;
        h.receive(10, &arriving(&brd));
    }
    assert_eq!(next_from(&mut h, 10).map(|p| p.opcode), Some(op::ANS), "the broadcast answered");
    assert_eq!(next_from(&mut h, 10), None, "and the other let fall");
    let mut near = Ncp::new(0o3050);
    let near_log = Lines::hook(&mut near);
    near.connect(20, 0o3060, "TIME", Box::new(Recorder(Log::default())));
    shuttle(&mut near, &mut h, 20);
    assert_eq!((near.connections(), h.connections()), (0, 0));
    assert_eq!(log.take(), NOTHING);
    assert_eq!(near_log.take(), NOTHING);
}

/// **A line is one line, whatever the other end sends.** A contact name
/// and a reason come off the network, and a newline in either would start
/// a line of the other end's writing in the log, which is one line an
/// event (`src/log.rs`); a control character is written as its escape.
#[test]
fn a_log_line_is_one_line_whatever_arrives() {
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Echo));
    let log = Lines::hook(&mut h);
    h.receive(0, &arriving(&rfc((0o3050, 0o21), 0o3060, 1, "NO\nSUCH")));
    h.receive(0, &arriving(&rfc((0o3050, 0o22), 0o3060, 2, "ECHO")));
    assert_eq!(next_from(&mut h, 0).map(|p| p.opcode), Some(op::CLS));
    let opn = next_from(&mut h, 0).expect("an OPN");
    let cls = Packet {
        opcode: op::CLS,
        forward: 0,
        dest: 0o3060,
        dest_index: opn.source_index,
        source: 0o3050,
        source_index: 0o22,
        number: 3,
        ack: opn.number,
        data: b"bye\r\nECHO from 3051 opened\x07".to_vec(),
    };
    h.receive(10, &arriving(&cls));
    let lines = log.take();
    assert!(lines.iter().all(|l| !l.chars().any(char::is_control)), "{lines:?}");
    assert_eq!(
        lines,
        [
            r"NO\nSUCH from 3050 (?) refused: No server for contact name NO\nSUCH",
            "ECHO from 3050 (?) opened",
            r"ECHO from 3050 (?) closed by its CLS: bye\r\nECHO from 3051 opened\u{7}",
        ]
    );
}

/// **A session's close reason is cut to fit a packet**, as a refusal's is
/// (`refuse`): a CLS carries at most `MAX_DATA` bytes of reason (AIM-628
/// §3.5), or its count word would run past what an interface takes.
#[test]
fn a_sessions_close_reason_fits_a_packet() {
    struct Closer;
    struct Closing(bool);
    impl Service for Closer {
        fn contact(&self) -> &str {
            "CLOSER"
        }
        fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
            Response::Accept(Box::new(Closing(false)))
        }
    }
    impl Session for Closing {
        fn data(&mut self, _now: u64, _op: u8, _bytes: &[u8]) {}
        fn eof(&mut self, _now: u64) {}
        fn closed(&mut self, _now: u64, _reason: &str) {}
        fn poll(&mut self, _now: u64) -> Vec<Out> {
            if std::mem::replace(&mut self.0, true) {
                Vec::new()
            } else {
                vec![Out::Close("x".repeat(600))]
            }
        }
    }
    let mut h = Ncp::new(0o3060);
    h.serve(Box::new(Closer));
    let me = (0o3050, 7);
    h.receive(0, &arriving(&rfc(me, 0o3060, 100, "CLOSER")));
    let opn = next_from(&mut h, 0).expect("an OPN");
    assert_eq!(opn.opcode, op::OPN);
    let server = (0o3060, opn.source_index);
    h.receive(10, &arriving(&sts(me, server, 101, opn.number, 5)));
    let cls = next_from(&mut h, 10).expect("the session's CLS");
    assert_eq!(cls.opcode, op::CLS);
    assert!(cls.data.len() <= packet::MAX_DATA, "{} bytes of reason", cls.data.len());
}
