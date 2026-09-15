// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chaosnet over UDP: the frame, and the link and the switch.
//!
//! CHUDP puts one Chaosnet packet in one UDP datagram behind a four-byte
//! header, and it is what `cbridge`, `usim`, `klh10` and the live
//! Chaosnet hosts speak to each other. Ordinary UDP: no privileges, and
//! it crosses a NAT --- which IP protocol 16, the assigned number for
//! Chaosnet, does not.
//!
//! Two halves. The frame: the constants, [`Order`], [`PACKET_ORDER`],
//! [`TRAILER_ORDER`], [`wrap`] and [`unwrap`], with an **unverified** mark
//! wherever the reading is not settled. Then [`Link`], the link and the
//! switch: the socket, the table of endpoints, and a packet for another host
//! of the subnet passed on untouched (`DESIGN.md` §5).
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

use crate::config::Config;
use crate::log;
use crate::ncp::op_name;
use crate::packet::{Framed, MAX_DATA, Packet, check_word};
use crate::service::status::Meters;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
/// packet's own words least significant byte first, and the trailer in
/// network order. That is what reconciles the protocol's own
/// documentation with the Wireshark dissector ([`PACKET_ORDER`]), and it
/// is belief and not knowledge.
///
/// What would settle it: the same capture or interoperation. One whole
/// packet's bytes are pinned in `tests/frame.rs`, so a correction is a
/// change to these two constants and to that one test.
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
/// A trace of a run against a real peer settles it --- `--trace`
/// (`DESIGN.md` §10) --- and until
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

// --- the link and the switch ---------------------------------------------

/// The link, and the switch of this host's subnet (`DESIGN.md` §5): the one
/// socket, the table of endpoints, and what becomes of each datagram.
///
/// **Endpoints are learned**: a packet's source --- the
/// header's, since that is where an answer is addressed --- is recorded at
/// the UDP address its datagram came from. A host behind `cbridge` is
/// learned at `cbridge`'s endpoint, which is where its packets go. A
/// `--peer` fixes an endpoint, and a packet does not move a fixed one.
/// This host's own address is never learned, and neither is 0, which is
/// no host's. The table is not expired, and holds one endpoint an address.
///
/// **Receiving**, in order ([`Link::receive`]); each drop is counted, and
/// printed with why under [`Link::trace`]:
///
/// 1. [`unwrap`], verbatim: length, version, function. A datagram it
///    refuses for its length is meter 7, `bad_bit_count`; for anything
///    else, meter 8, `other_discarded` (`DESIGN.md` §7).
/// 2. The trailer's source is this host's address, or 0: dropped, as a
///    frame claiming to be from this station.
/// 3. The header's source is learned, before anything else is done with
///    the packet, so that an answer to it can be passed on at once.
/// 4. By the trailer's destination, the cable's, which is what a cable
///    delivers by:
///    - **this host**: to the NCP;
///    - **0**, a broadcast: to the NCP, and passed on to every endpoint
///      but the one it came from;
///    - **another host of this subnet with an endpoint**: passed on to
///      that endpoint, unless it is the one the datagram came from;
///    - **anything else** --- another subnet, a host not yet heard from,
///      or back where it came from: dropped.
///
/// **Passed on means untouched**: the datagram goes out as it came, byte
/// for byte, as a cable does not change a frame --- no forwarding count,
/// no new trailer. That is what makes this a switch and not a bridge, and
/// why nothing goes to another subnet: there is no routing to decide
/// where. A check word that is not the hardware's is traced and the
/// packet handled all the same (`DESIGN.md` §5).
///
/// **Sending this host's own** ([`Link::send`]): [`wrap`] with this host's
/// address and the hardware's check word, to the destination's endpoint,
/// or once to every distinct endpoint for a destination of 0. A
/// destination with no endpoint is dropped, and counted.
///
/// Meter 1, `received`, counts every datagram; meter 2, `transmitted`,
/// every datagram sent, this host's own and those passed on ([`Meters`]).
pub struct Link {
    socket: UdpSocket,
    /// Where the socket is bound, as the system bound it.
    at: SocketAddr,
    /// This host's address.
    address: u16,
    endpoints: BTreeMap<u16, Endpoint>,
    meters: Arc<Meters>,
    /// How the socket waits for a datagram now: not at all, non-blocking,
    /// or at most this long. Changed only when a turn asks for another.
    waiting: Option<Duration>,
    /// Every packet, every packet passed on, and every drop with why,
    /// printed as they go by: `--trace` (`DESIGN.md` §10).
    pub trace: bool,
}

/// Where a host's packets go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Endpoint {
    at: SocketAddr,
    /// Given by a `--peer`, so that no packet moves it.
    fixed: bool,
}

impl Link {
    /// Binds `config.listen` and holds the endpoints `config.peers` fixes,
    /// for the host at `config.address`, counting into `meters`. A socket
    /// that cannot be bound is the error.
    pub fn bind(config: &Config, meters: Arc<Meters>) -> io::Result<Link> {
        let socket = UdpSocket::bind(config.listen)?;
        socket.set_nonblocking(true)?;
        let at = socket.local_addr()?;
        let endpoints = config
            .peers
            .iter()
            .map(|p| (p.address, Endpoint { at: p.endpoint, fixed: true }))
            .collect();
        let address = config.address;
        Ok(Link { socket, at, address, endpoints, meters, waiting: None, trace: false })
    }

    /// Where the socket is bound: a `listen` at port 0 is the port the
    /// system picked.
    pub fn at(&self) -> SocketAddr {
        self.at
    }

    /// Where packets for `address` go: its endpoint, fixed or learned, if
    /// it has one.
    pub fn endpoint(&self, address: u16) -> Option<SocketAddr> {
        self.endpoints.get(&address).map(|e| e.at)
    }

    /// Waits at most `wait` for one datagram --- not at all for a `wait`
    /// of zero --- and puts it through the receive order. What comes back
    /// is the packet for the NCP: one for this host, or a broadcast.
    ///
    /// The buffer is one byte longer than [`MAX_FRAME`], as that says, so
    /// a datagram longer than any packet fills it and is refused for its
    /// length. A socket error is logged, and is nothing arrived.
    pub fn receive(&mut self, now: u64, wait: Duration) -> Option<Framed> {
        if let Err(e) = self.wait_at_most(wait) {
            log::event(format_args!("chudp: the socket at {}: {e}", self.at));
            return None;
        }
        let mut datagram = [0u8; MAX_FRAME + 1];
        let (n, from) = match self.socket.recv_from(&mut datagram) {
            Ok(got) => got,
            Err(e) if nothing_there(&e) => return None,
            Err(e) => {
                log::event(format_args!("chudp: the socket at {}: {e}", self.at));
                return None;
            }
        };
        self.meters.received.fetch_add(1, Ordering::Relaxed);
        self.arrived(now, &datagram[..n], from)
    }

    /// Sets how the socket waits, when a turn asks for another way than it
    /// has. std refuses a read timeout of zero, so zero is non-blocking.
    fn wait_at_most(&mut self, wait: Duration) -> io::Result<()> {
        let want = (!wait.is_zero()).then_some(wait);
        if want != self.waiting {
            match want {
                None => self.socket.set_nonblocking(true)?,
                Some(w) => {
                    self.socket.set_read_timeout(Some(w))?;
                    self.socket.set_nonblocking(false)?;
                }
            }
            self.waiting = want;
        }
        Ok(())
    }

    /// One datagram, from `from`, through the receive order.
    fn arrived(&mut self, now: u64, datagram: &[u8], from: SocketAddr) -> Option<Framed> {
        let f = match unwrap(datagram) {
            Ok(f) => f,
            Err(why) => {
                let meter = if refused_for_length(datagram) {
                    &self.meters.bad_bit_count
                } else {
                    &self.meters.other_discarded
                };
                meter.fetch_add(1, Ordering::Relaxed);
                self.traced(now, format_args!("from {from}: dropped: {why}"));
                return None;
            }
        };
        // What `unwrap` takes is a whole packet, its count answering its
        // length, so this does not fail; if it did, the datagram would be
        // dropped as any other.
        let Ok((p, dest)) = Packet::from_buffer(&f.buffer) else {
            let words = f.buffer.len();
            self.discard(now, format_args!("from {from}: {words} words are not a packet"));
            return None;
        };
        let (source, opcode) = (f.source, p.opcode);
        let odd = if f.check_ok { "" } else { ", its check word not the hardware's" };
        let op = op_name(opcode);
        self.traced(now, format_args!("from {from}: {source:o} -> {dest:o} {op}{odd}"));
        if source == self.address || source == 0 {
            let who = if source == 0 { "no host" } else { "this host" };
            self.discard(now, format_args!("from {from}: a frame from {source:o}, {who}"));
            return None;
        }
        self.learn(now, p.source, from);
        if dest == self.address {
            return Some(f);
        }
        if dest == 0 {
            for to in self.everyone_but(Some(from)) {
                self.pass_on(now, datagram, (source, dest, opcode), to);
            }
            return Some(f);
        }
        if dest >> 8 != self.address >> 8 {
            let subnet = dest >> 8;
            self.discard(
                now,
                format_args!("{source:o} -> {dest:o}: subnet {subnet:o} is not this one"),
            );
            return None;
        }
        match self.endpoint(dest) {
            None => self.discard(now, format_args!("{source:o} -> {dest:o}: not heard from")),
            Some(to) if to == from => self.discard(
                now,
                format_args!("{source:o} -> {dest:o}: at {from}, where it came from"),
            ),
            Some(to) => self.pass_on(now, datagram, (source, dest, opcode), to),
        }
        None
    }

    /// The header's source learned at `from`, unless it is 0, this host,
    /// or fixed by a `--peer`.
    fn learn(&mut self, now: u64, source: u16, from: SocketAddr) {
        if source == 0 || source == self.address {
            return;
        }
        match self.endpoints.get(&source).copied() {
            Some(e) if e.fixed && e.at != from => {
                let at = e.at;
                self.traced(now, format_args!("{source:o} is fixed at {at}, not moved to {from}"));
            }
            Some(e) if e.fixed || e.at == from => {}
            _ => {
                self.endpoints.insert(source, Endpoint { at: from, fixed: false });
                self.traced(now, format_args!("{source:o} is at {from}"));
            }
        }
    }

    /// One of this host's own packets out: the NCP's buffer, cable
    /// destination last, as `wrap(buffer, own address, check_word(buffer +
    /// own address))` --- the 9401's CRC-16 (`packet::check_word`) ---
    /// to the destination's endpoint, or once to every distinct endpoint
    /// for 0. A destination with no endpoint is dropped, and counted.
    pub fn send(&self, now: u64, buffer: &[u16]) {
        let mut over = buffer.to_vec();
        over.push(self.address);
        let framed = wrap(buffer, self.address, check_word(&over));
        let (Some(datagram), Some(&dest)) = (framed, buffer.last()) else {
            self.discard(now, format_args!("an empty buffer is no packet"));
            return;
        };
        let to = match dest {
            0 => self.everyone_but(None),
            d => self.endpoint(d).into_iter().collect(),
        };
        let (own, op) = (self.address, op_name(buffer[0].to_be_bytes()[0]));
        if to.is_empty() {
            self.discard(now, format_args!("{own:o} -> {dest:o} {op}: no endpoint"));
            return;
        }
        for at in to {
            self.traced(now, format_args!("{own:o} -> {dest:o} {op} to {at}"));
            self.send_to(&datagram, at);
        }
    }

    /// The datagram as it came, to `to`.
    fn pass_on(&self, now: u64, datagram: &[u8], packet: (u16, u16, u8), to: SocketAddr) {
        let (source, dest, op) = (packet.0, packet.1, op_name(packet.2));
        self.traced(now, format_args!("{source:o} -> {dest:o} {op} passed on to {to}"));
        self.send_to(datagram, to);
    }

    /// Every distinct endpoint, each once, but `but`.
    fn everyone_but(&self, but: Option<SocketAddr>) -> BTreeSet<SocketAddr> {
        self.endpoints.values().map(|e| e.at).filter(|&at| Some(at) != but).collect()
    }

    /// `datagram` to `to`: meter 2 if it went; logged, and meter 8, if the
    /// socket would not take it.
    fn send_to(&self, datagram: &[u8], to: SocketAddr) {
        match self.socket.send_to(datagram, to) {
            Ok(_) => {
                self.meters.transmitted.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                self.meters.other_discarded.fetch_add(1, Ordering::Relaxed);
                log::event(format_args!("chudp: to {to}: {e}"));
            }
        }
    }

    /// A packet dropped for anything but its length: meter 8, and why,
    /// under trace.
    fn discard(&self, now: u64, why: fmt::Arguments) {
        self.meters.other_discarded.fetch_add(1, Ordering::Relaxed);
        self.traced(now, format_args!("dropped: {why}"));
    }

    /// A line of the trace, at `now` on the daemon's clock.
    fn traced(&self, now: u64, what: fmt::Arguments) {
        if self.trace {
            eprintln!("chudp {now:>6}: {what}");
        }
    }
}

/// Whether [`unwrap`] refused `datagram` for its length --- too short for
/// a packet, longer than any, or not the length its data count wants ---
/// rather than for its version or its function, which it reads between
/// the two. Meter 7 counts the first kind, meter 8 the second (`DESIGN.md`
/// §7).
fn refused_for_length(datagram: &[u8]) -> bool {
    let fits = (HEADER + SOFTWARE_HEADER + TRAILER..=MAX_FRAME).contains(&datagram.len());
    !fits || (datagram[0] == VERSION && datagram[1] == PACKET)
}

/// Whether a read that failed only found nothing: its timeout ran out ---
/// `WouldBlock` on Unix, and `TimedOut` on some platforms, std's
/// `set_read_timeout` says --- or, not waiting, there was nothing there,
/// or a signal came first.
fn nothing_there(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
