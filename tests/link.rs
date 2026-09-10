// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The link and the hub (`DESIGN.md` §5), over loopback: a daemon and test
//! hosts, each on a socket of its own, and every datagram a real one.
//!
//! What is learned and what is fixed; what is dropped before anything
//! else, and counted; and the hub --- a packet for another host of the
//! subnet passed on as it came, a broadcast to every host but its sender,
//! and nothing to another subnet, to a host not yet heard from, or back
//! where it came from.

mod support;

use muir_ah::chudp;
use muir_ah::config::Config;
use muir_ah::daemon::Daemon;
use muir_ah::ncp::op;
use muir_ah::packet::Packet;
use muir_ah::service::time::Time;
use std::time::Duration;
use support::{
    LM1, LM2, LM3, OZ, Recorder, TestHost, ask, daemon, datagram, hear, meters, packet, rfc,
    settle, site,
};

/// A host on subnet 7, which is not this host's: one behind a bridge, as
/// far as this host can tell.
const ELSEWHERE: u16 = 0o3450;

/// A BRD for `contact` from index 0o21 of `from`, AIM-628 §4.5: a subnet
/// bit map, its length in bytes in the acknowledgement field, and then the
/// contact name.
fn brd(from: u16, contact: &str) -> Packet {
    Packet {
        opcode: op::BRD,
        dest: 0,
        ack: 4,
        data: [&[0xff; 4][..], contact.as_bytes()].concat(),
        ..rfc(from, 0, "")
    }
}

/// **An unknown host's endpoint is learned, and it is answered there.** A
/// packet's source --- the header's --- is recorded at the UDP address the
/// datagram came from, as muir's `--chaos-udp-dynamic` records it (muir's
/// `src/chaos/udp.rs`, `arrived`), before anything else is done with the
/// packet, so the answer to it has somewhere to go (`DESIGN.md` §5). A
/// learned endpoint moves when the host does: that is what learning means
/// (`CLAUDE.md` §3).
#[test]
fn an_unknown_hosts_endpoint_is_learned_and_answered() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    assert_eq!(d.link().endpoint(LM1), None, "not heard from yet");
    let ans = ask(&mut d, &mut lm1, 0, "STATUS");
    assert_eq!(d.link().endpoint(LM1), Some(lm1.at), "learned where its packet came from");
    assert_eq!(ans.dest, LM1, "and answered there");
    assert!(ans.data.starts_with(b"MIT-OZ\0"), "by this host's STATUS");
    // The same host on another socket, as after a restart: the endpoint
    // moves with it, and the old one hears nothing more.
    let mut restarted = TestHost::new(LM1, d.at());
    ask(&mut d, &mut restarted, 10, "STATUS");
    assert_eq!(d.link().endpoint(LM1), Some(restarted.at), "moved");
    lm1.turn(10);
    assert!(lm1.heard.is_empty(), "nothing more to where it was");
}

/// **A fixed endpoint is not moved by a packet.** A `--peer` is a
/// statement about where a host is; a packet claiming that host's address
/// from somewhere else is answered at the endpoint it gave, and does
/// not move it --- as muir keeps an endpoint a flag named (`CLAUDE.md` §3;
/// muir's `a_packet_does_not_move_an_endpoint_a_flag_named`).
#[test]
fn a_fixed_endpoint_is_not_moved_by_a_packet() {
    let (named, named_at) = support::socket();
    let mut d = daemon(&site(&format!("--peer 3040@{named_at}\n")));
    assert_eq!(d.link().endpoint(0o3040), Some(named_at), "where --peer put it");
    let mut impostor = TestHost::new(0o3040, d.at());
    let asked = Recorder::default();
    impostor.ncp.connect(0, OZ, "STATUS", Box::new(asked.clone()));
    settle(&mut d, &mut [&mut impostor], 0);
    assert_eq!(d.link().endpoint(0o3040), Some(named_at), "not moved");
    let heard = hear(&named);
    assert_eq!(heard.len(), 1, "the answer went where the line said");
    let (ans, _) = packet(&heard[0]);
    assert_eq!((ans.opcode, ans.source, ans.dest), (op::ANS, OZ, 0o3040));
    assert!(impostor.heard.is_empty(), "and not to whoever claimed the address");
    assert!(asked.events().is_empty(), "so the impostor's question goes unanswered");
}

/// **A datagram from this host's own address is dropped**: a trailer
/// whose source is this host's, or 0, which is no host's --- muir's rule
/// for a frame claiming to be from its own station (`DESIGN.md` §5). Its
/// sender is not learned, and nothing answers it. And this host's address
/// is never learned at an endpoint: a packet whose header claims it, in a
/// frame from another host, is taken, and the answer --- addressed to this
/// host --- has nowhere to go, so it is dropped as any packet of this
/// host's with no endpoint is.
#[test]
fn a_datagram_from_this_hosts_own_address_is_dropped() {
    let mut d = daemon(&site(""));
    let (s, _) = support::socket();
    for source in [OZ, 0] {
        s.send_to(&datagram(&rfc(LM1, OZ, "STATUS"), source), d.at()).unwrap();
    }
    settle(&mut d, &mut [], 0);
    assert!(hear(&s).is_empty(), "neither is answered");
    assert_eq!(d.link().endpoint(LM1), None, "and its sender is not learned");
    assert_eq!(meters(&d), [2, 0, 0, 2], "both received, both dropped");
    s.send_to(&datagram(&rfc(OZ, OZ, "STATUS"), LM1), d.at()).unwrap();
    settle(&mut d, &mut [], 0);
    assert_eq!(d.link().endpoint(OZ), None, "this host's address is never learned");
    assert!(hear(&s).is_empty(), "so the answer goes nowhere");
    assert_eq!(meters(&d), [3, 0, 0, 3], "and is dropped, and counted");
}

/// **A packet from one host to another is passed on byte for byte.** The
/// hub is the cable, and the cable does not change a frame: no forwarding
/// count, no new trailer, the datagram as it came (`DESIGN.md` §5). So a
/// packet as no NCP here would write it --- a forwarding count of 3, and a
/// check word that is not the hardware's --- reaches the other host with
/// both, and the other host's answer comes back the same way.
#[test]
fn a_packet_from_one_host_to_another_is_passed_on_byte_for_byte() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let mut lm2 = TestHost::new(LM2, d.at());
    lm2.ncp.serve(Box::new(Time::fixed(0x1234_5678)));
    // Each is heard from once, as a band is when it first asks the time.
    for h in [&mut lm1, &mut lm2] {
        ask(&mut d, h, 0, "TIME");
    }
    let [_, sent_before, _, _] = meters(&d);
    let p = Packet { forward: 3, ..rfc(LM1, LM2, "TIME") };
    let sent = chudp::wrap(&p.to_buffer(LM2), LM1, 0o12345).expect("a frame");
    assert!(!chudp::unwrap(&sent).unwrap().check_ok, "not the hardware's check word");
    lm1.send_bytes(&sent);
    settle(&mut d, &mut [&mut lm1, &mut lm2], 10);
    assert_eq!(lm2.heard.first(), Some(&sent), "LM2 has it as LM1 sent it, byte for byte");
    let answers = lm1.packets();
    assert_eq!(answers.len(), 1, "and LM1 has LM2's answer: {answers:?}");
    let a = &answers[0];
    assert_eq!((a.opcode, a.source, a.dest), (op::ANS, LM2, LM1));
    assert_eq!(a.data, [0x78, 0x56, 0x34, 0x12], "LM2's time, not this host's");
    // LM1's NCP did not send that RFC, so it has no connection for the
    // answer and says so with a LOS (AIM-628 §4.2), which reaches LM2 the
    // same way: everything between the two goes through the hub.
    let after: Vec<u8> = lm2.packets()[1..].iter().map(|p| p.opcode).collect();
    assert_eq!(after, [op::LOS], "and then LM1's LOS");
    assert_eq!(meters(&d)[1] - sent_before, 3, "each passed on, and counted as sent");
}

/// **A broadcast reaches every host but its sender, and is answered
/// here.** On one cable every host hears every broadcast; over CHUDP the
/// hub makes it so, passing the datagram on to every endpoint but the one
/// it came from, and taking it itself (`DESIGN.md` §5, `CLAUDE.md` §8b).
/// A BRD for TIME is then answered by every host that serves TIME, this
/// one among them (AIM-628 §4.5).
#[test]
fn a_broadcast_reaches_every_host_but_its_sender_and_is_answered_here() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let mut lm2 = TestHost::new(LM2, d.at());
    let mut lm3 = TestHost::new(LM3, d.at());
    lm2.ncp.serve(Box::new(Time::fixed(2)));
    lm3.ncp.serve(Box::new(Time::fixed(3)));
    for h in [&mut lm1, &mut lm2, &mut lm3] {
        ask(&mut d, h, 0, "STATUS");
    }
    let sent = datagram(&brd(LM1, "TIME"), LM1);
    lm1.send_bytes(&sent);
    settle(&mut d, &mut [&mut lm1, &mut lm2, &mut lm3], 10);
    // Each has it first, and once. What each has after it is LM1's LOS for
    // the answer it sent: LM1 sent the BRD by hand, not from a connection.
    for h in [&lm2, &lm3] {
        let who = h.ncp.address();
        assert_eq!(h.heard.first(), Some(&sent), "{who:o} hears it, as it was sent");
        assert_eq!(h.heard.iter().filter(|&d| *d == sent).count(), 1, "{who:o}, once");
    }
    assert!(!lm1.heard.contains(&sent), "but not its sender");
    let mut answered: Vec<u16> =
        lm1.packets().iter().filter(|p| p.opcode == op::ANS).map(|p| p.source).collect();
    answered.sort();
    assert_eq!(answered, [LM2, LM3, OZ], "answered here, and by every other host, as on a cable");
}

/// **A packet for another subnet, or for a host not yet heard from,
/// reaches nobody.** The hub passes on within its subnet and routes
/// nothing: a host of another subnet is answered where it was heard from,
/// but nothing is passed on to it; and a host with no endpoint has
/// nowhere to be passed on to. Both are dropped, and counted (`DESIGN.md`
/// §5).
#[test]
fn a_packet_for_another_subnet_or_a_host_not_heard_from_reaches_nobody() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let mut lm2 = TestHost::new(LM2, d.at());
    let mut far = TestHost::new(ELSEWHERE, d.at());
    for h in [&mut lm1, &mut lm2, &mut far] {
        ask(&mut d, h, 0, "STATUS");
    }
    assert_eq!(d.link().endpoint(ELSEWHERE), Some(far.at), "heard from, and answered there");
    let [_, _, _, dropped] = meters(&d);
    lm1.send_bytes(&datagram(&rfc(LM1, ELSEWHERE, "TIME"), LM1));
    lm1.send_bytes(&datagram(&rfc(LM1, 0o3053, "TIME"), LM1));
    settle(&mut d, &mut [&mut lm1, &mut lm2, &mut far], 10);
    for h in [&lm1, &lm2, &far] {
        assert!(h.heard.is_empty(), "{:o} heard {:?}", h.ncp.address(), h.packets());
    }
    assert_eq!(meters(&d)[3] - dropped, 2, "both dropped, and counted");
}

/// **Nothing is sent back to the endpoint it came from.** Two hosts can
/// share an endpoint --- hosts behind a bridge are learned at the
/// bridge's (`DESIGN.md` §5) --- and a packet from one to the other has
/// already been where it is going: it is dropped, and counted. A broadcast
/// from behind that endpoint goes to every other, and not back.
#[test]
fn nothing_is_sent_back_to_the_endpoint_it_came_from() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    ask(&mut d, &mut lm1, 0, "STATUS");
    let (bridge, bridge_at) = support::socket();
    for host in [0o3053, 0o3054] {
        bridge.send_to(&datagram(&rfc(host, OZ, "STATUS"), host), d.at()).unwrap();
    }
    settle(&mut d, &mut [&mut lm1], 0);
    assert_eq!(hear(&bridge).len(), 2, "each answered");
    assert_eq!(d.link().endpoint(0o3053), Some(bridge_at));
    assert_eq!(d.link().endpoint(0o3054), Some(bridge_at), "at one endpoint");
    let [_, _, _, dropped] = meters(&d);
    bridge.send_to(&datagram(&rfc(0o3053, 0o3054, "TIME"), 0o3053), d.at()).unwrap();
    settle(&mut d, &mut [&mut lm1], 10);
    assert!(hear(&bridge).is_empty(), "not sent back");
    assert!(lm1.heard.is_empty(), "nor anywhere else");
    assert_eq!(meters(&d)[3] - dropped, 1, "dropped, and counted");
    let sent = datagram(&brd(0o3053, "STATUS"), 0o3053);
    bridge.send_to(&sent, d.at()).unwrap();
    settle(&mut d, &mut [&mut lm1], 20);
    assert_eq!(lm1.heard.first(), Some(&sent), "a broadcast from behind it reaches LM1");
    assert!(!hear(&bridge).contains(&sent), "and does not come back");
}

/// **A datagram that is no packet is dropped, and counted**, before it can
/// say anything about where anyone is: every datagram is meter 1; one
/// `unwrap` refuses for its length --- too short for a packet, longer than
/// any, or not the length its data count wants --- is meter 7; one it
/// refuses for its version or its function is meter 8 (`DESIGN.md` §5,
/// §7). A datagram longer than any packet is read into a buffer one byte
/// longer than the longest, and refused for that (`chudp::MAX_FRAME`).
/// Traced, so that each drop is printed with why.
#[test]
fn a_datagram_that_is_no_packet_is_dropped_and_counted() {
    let config = Config::parse(&site("")).unwrap();
    let mut d = Daemon::new(&config, support::tree(&config), true).expect("a daemon");
    let (lm1, _) = support::socket();
    let good = datagram(&rfc(LM1, OZ, "STATUS"), LM1);
    let long = [&good[..], &[0; 600]].concat();
    let mut short = good.clone();
    short.drain(20..22);
    let (mut version, mut function) = (good.clone(), good.clone());
    version[0] = 2;
    function[1] = 2;
    for bad in [&good[..10], &long[..], &short[..], &version[..], &function[..]] {
        lm1.send_to(bad, d.at()).unwrap();
    }
    settle(&mut d, &mut [], 0);
    assert!(hear(&lm1).is_empty(), "none is answered");
    assert_eq!(d.link().endpoint(LM1), None, "and none says where its sender is");
    assert_eq!(meters(&d), [5, 0, 3, 2], "three for their length, two for anything else");
    lm1.send_to(&good, d.at()).unwrap();
    settle(&mut d, &mut [], 0);
    assert_eq!(hear(&lm1).len(), 1, "the one that is a packet is answered");
    assert_eq!(meters(&d), [6, 1, 3, 2]);
}

/// **A broadcast of this host's own goes once to each endpoint** --- not
/// once to each address, since two hosts behind one bridge share its
/// endpoint (`DESIGN.md` §5). An RFC to 0 from the daemon's NCP is no
/// broadcast any NCP takes, but it is a buffer for 0 in the link's hands,
/// which is what this is about.
#[test]
fn this_hosts_own_broadcast_goes_once_to_each_endpoint() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    ask(&mut d, &mut lm1, 0, "STATUS");
    let (bridge, _) = support::socket();
    for host in [0o3053, 0o3054] {
        bridge.send_to(&datagram(&rfc(host, OZ, "STATUS"), host), d.at()).unwrap();
    }
    settle(&mut d, &mut [&mut lm1], 0);
    assert_eq!(hear(&bridge).len(), 2, "both hosts behind it heard from");
    let [_, sent_before, _, _] = meters(&d);
    d.ncp().connect(10, 0, "STATUS", Box::new(Recorder::default()));
    d.turn(10, Duration::ZERO);
    let to_bridge = hear(&bridge);
    assert_eq!(to_bridge.len(), 1, "once to the endpoint two hosts share");
    lm1.turn(10);
    assert_eq!(lm1.heard, to_bridge, "and once to LM1's, the same datagram");
    let (p, cable) = packet(&to_bridge[0]);
    assert_eq!((p.opcode, p.source, cable), (op::RFC, OZ, 0));
    assert_eq!(meters(&d)[1] - sent_before, 2, "two datagrams sent");
}
