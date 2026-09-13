// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The NCP: the transport protocol of AIM-628 chapters 3 and 4, and the
//! services that answer on it.
//!
//! The link hands it every packet for this host and every broadcast
//! (`DESIGN.md` §5); it keeps those addressed to it or broadcast, runs the
//! connection protocol, and hands what arrives to a [`Service`] by
//! contact name. A service either answers a request outright --- the
//! *simple transaction* of §4.1, RFC then ANS --- or accepts it as a
//! *stream connection*, RFC, OPN, STS and then numbered data both ways,
//! ended with EOF and CLS as §4.4 says. Adding a service is one
//! `impl Service`; the transport does not know what any of them do.
//!
//! There is no cable: the NCP and the socket are wired to each other
//! directly, through [`Ncp::receive`] and [`Ncp::transmit`] (`DESIGN.md`
//! §4).
//!
//! What becomes of each connection --- opened, refused, closed --- it tells
//! the log, [`Ncp::log`], when it has one (`DESIGN.md` §10). Every packet is
//! the trace's, `trace`, and not the log's.

use crate::packet::{Framed, MAX_DATA, Packet};
use std::collections::VecDeque;
use std::fmt;

/// Packet opcodes, AIM-628 chapter 4, as `sys/network/chaos/chsncp.lisp`
/// numbers them.
pub mod op {
    pub const RFC: u8 = 0o1;
    pub const OPN: u8 = 0o2;
    pub const CLS: u8 = 0o3;
    pub const FWD: u8 = 0o4;
    pub const ANS: u8 = 0o5;
    pub const SNS: u8 = 0o6;
    pub const STS: u8 = 0o7;
    pub const RUT: u8 = 0o10;
    pub const LOS: u8 = 0o11;
    pub const LSN: u8 = 0o12;
    pub const MNT: u8 = 0o13;
    pub const EOF: u8 = 0o14;
    pub const UNC: u8 = 0o15;
    pub const BRD: u8 = 0o16;
    /// "Opcodes 200 through 277 (octal) are controlled packets with user
    /// data in 8-bit bytes"; 200 is the default.
    pub const DAT: u8 = 0o200;
    /// "Opcodes 300 through 377 ... 16-bit bytes"; 300 is the default.
    pub const DWD: u8 = 0o300;
    /// Whether an opcode carries user data.
    pub fn is_data(op: u8) -> bool {
        op >= DAT
    }
    /// Whether packets of this opcode are controlled --- numbered,
    /// acknowledged and retransmitted, §3.8.
    pub fn is_controlled(op: u8) -> bool {
        matches!(op, RFC | OPN | EOF) || is_data(op)
    }
}

/// What a service does with a request for connection.
pub enum Response {
    /// A simple transaction: the data of an ANS, and no connection.
    Answer(Vec<u8>),
    /// A refusal: the reason, sent in a CLS.
    Refuse(String),
    /// A stream connection, served by this session.
    Accept(Box<dyn Session>),
}

/// A service answering to a contact name.
pub trait Service: Send {
    /// The contact name, AIM-628 §3.2: "a string of uppercase letters,
    /// numbers, and ASCII punctuation".
    fn contact(&self) -> &str;
    /// A request for connection with this contact name arrived at `now`,
    /// with `args` the rest of the RFC's data after the name, from the
    /// host and index `from`.
    fn request(&mut self, now: u64, args: &str, from: (u16, u16)) -> Response;
}

/// What a session wants sent on its connection.
pub enum Out {
    /// A data packet, 8-bit bytes, opcode `DAT`.
    Data(Vec<u8>),
    /// A data packet with this opcode in the data range.
    DataOp(u8, Vec<u8>),
    /// An EOF, §4.4.
    Eof,
    /// A CLS with this reason, ending the connection.
    Close(String),
    /// A new connection *from* this host to `contact` at `host`, served
    /// by `session` once open: an RFC goes out, and the session's
    /// [`Session::opened`] is called when the OPN comes back. The FILE
    /// protocol's data connections are made this way, the server
    /// calling the contact name the user end listens on.
    Connect { host: u16, contact: String, session: Box<dyn Session> },
}

/// One end of one stream connection at this host.
pub trait Session: Send {
    /// The connection this host asked for is open: the OPN came back.
    fn opened(&mut self, _now: u64) {}
    /// Data arrived: `op` in the data range, and the bytes.
    fn data(&mut self, now: u64, op: u8, bytes: &[u8]);
    /// The other side sent EOF.
    fn eof(&mut self, now: u64);
    /// The connection ended: a CLS or LOS from the other side, with its
    /// reason, or the server's own timeout.
    fn closed(&mut self, now: u64, reason: &str);
    /// What the session wants sent, in order.
    fn poll(&mut self, now: u64) -> Vec<Out>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// We sent an RFC and wait for the OPN, ANS or CLS.
    RfcSent,
    /// We sent an OPN and wait for the STS that acknowledges it; data
    /// may already flow from them.
    OpnSent,
    Open,
}

/// One end of a connection at this host.
struct Conn {
    /// This end's index for it: its slot, and that slot's uniquizer when
    /// it was given out. What a packet for it carries.
    local: u16,
    state: State,
    remote: (u16, u16),
    /// The contact name, for the log: the RFC's first word, without its
    /// arguments, as a service is found by it. A `Box<str>`, since it never
    /// grows.
    contact: Box<str>,
    /// Whether this end asked for the connection, with [`Ncp::connect`],
    /// rather than the other end, with an RFC a service here accepted.
    from_this_end: bool,
    session: Option<Box<dyn Session>>,
    /// The number the next controlled packet we send will carry.
    next_number: u16,
    /// Our controlled packets not yet receipted, with when each was last
    /// sent.
    unacked: VecDeque<(u16, Packet, u64)>,
    /// How many packets they will take: their window, from their OPN or
    /// STS.
    their_window: u16,
    /// The number of the last in-order controlled packet we took from
    /// them, and of the last one we acknowledged to them.
    last_received: u16,
    last_acked: u16,
    /// Our receive window, told to them.
    window: u16,
    /// When we last had a packet from the other end. A connection silent
    /// past [`HOST_DOWN_NS`] is given up, as the band gives up on a host,
    /// so a dead connection is freed rather than retransmitting for ever
    /// and holding its index against a fresh RFC.
    last_heard: u64,
    /// What the session has offered that their window has not yet had
    /// room for.
    backlog: VecDeque<Out>,
}

/// How long before an unreceipted controlled packet goes again, §3.8:
/// "Retransmission occurs every 1/2 second."
pub const RETRANSMIT_NS: u64 = 500_000_000;

/// How long a connection lives without a packet from the other end: the
/// band's `HOST-DOWN-INTERVAL`, `(* 60. 90. 2)` sixtieths of a second,
/// "3 minutes", `sys/network/chaos/chsncp.lisp`. Its `PROBE-CONN` puts a
/// connection in `HOST-DOWN-STATE` when `TIME-LAST-RECEIVED` is further
/// back than that, and this end does the same.
pub const HOST_DOWN_NS: u64 = 180_000_000_000;

/// The bits of a connection index that say which slot of the table holds
/// the connection; the bits above them are that slot's uniquizer. The
/// machine's own NCP has 128 slots, seven bits (`MAXIMUM-INDEX`,
/// `sys/network/chaos/chsncp.lisp`); a site's host holds more connections
/// at once, so ten here, and six bits of uniquizer above them.
const SLOT_BITS: u32 = 10;

/// The table's slots; slot 0 is never a connection, so no index is 0.
const SLOTS: usize = 1 << SLOT_BITS;

/// The slot an index names: its low [`SLOT_BITS`] bits.
fn slot(index: u16) -> usize {
    usize::from(index) & (SLOTS - 1)
}

/// The NCP: a node with services, at one address --- the server side of
/// the protocols the associated machine's servers spoke. This host is
/// that machine.
pub struct Ncp {
    address: u16,
    services: Vec<Box<dyn Service>>,
    /// Connections by slot, the low [`SLOT_BITS`] bits of a local index;
    /// slot 0 is never a connection.
    conns: Vec<Option<Conn>>,
    /// Each slot's uniquizer, the bits of an index above its slot: one
    /// more each time the slot is given out.
    uniquizers: Vec<u16>,
    /// Where the search for a free slot starts: after the last slot given
    /// out.
    next_slot: usize,
    out: VecDeque<Vec<u16>>,
    /// Packets printed as they go by, for watching a run.
    pub trace: bool,
    /// Our receive window for connections we accept.
    pub window: u16,
    /// The log, if there is one (`DESIGN.md` §10): a line for each
    /// connection opened, refused and closed, without the time, which the
    /// log adds. Each names the contact and the other host, in octal ---
    /// `from` it when it asked for the connection, `to` it when this end
    /// did:
    ///
    /// ```text
    /// ECHO from 3050 opened               an RFC accepted: the OPN sent
    /// ECHO to 3060 opened                 an RFC from this end: the OPN back
    /// ECHO from 3050 refused: <reason>    the CLS sent for an RFC
    /// ECHO to 3060 refused: <reason>      the CLS back for one from this end
    /// ECHO from 3050 closed by its CLS: <reason>
    /// ECHO from 3050 closed by its LOS: <reason>
    /// ECHO from 3050 closed by our CLS: <reason>    a session's
    /// ECHO from 3050 closed, host down: nothing heard for 180 s
    /// ```
    ///
    /// The last is [`HOST_DOWN_NS`]. A simple transaction makes no
    /// connection, and is how STATUS is asked, over and over, so it is
    /// logged only where [`Ncp::log_simple`] asks for it, as `TIME from
    /// 3050 answered`. Nothing is logged for a BRD let fall. A contact name
    /// and a reason come off the network, so a control character in either
    /// is written as its escape, and a line stays one line. Unset, nothing
    /// is formatted.
    pub log: Option<crate::log::Hook>,
    /// `--log-simple`: each simple transaction answered is a line of
    /// [`Ncp::log`] too. Off unless set.
    pub log_simple: bool,
}

impl Ncp {
    pub fn new(address: u16) -> Ncp {
        Ncp {
            address,
            services: Vec::new(),
            conns: vec![None],
            uniquizers: vec![0],
            next_slot: 1,
            out: VecDeque::new(),
            trace: false,
            window: 8,
            log: None,
            log_simple: false,
        }
    }

    pub fn address(&self) -> u16 {
        self.address
    }

    pub fn serve(&mut self, service: Box<dyn Service>) {
        self.services.push(service);
    }

    /// Whether any connection is open.
    pub fn connections(&self) -> usize {
        self.conns.iter().filter(|c| c.is_some()).count()
    }

    fn send(&mut self, p: Packet) {
        if self.trace {
            eprintln!(
                "chaos {:>6}: {} -> {} {} {}",
                "",
                fmt_end((p.source, p.source_index)),
                fmt_end((p.dest, p.dest_index)),
                op_name(p.opcode),
                fmt_data(p.opcode, &p.data)
            );
        }
        let dest = p.dest;
        self.out.push_back(p.to_buffer(dest));
    }

    fn packet(
        &self,
        op: u8,
        to: (u16, u16),
        from_index: u16,
        number: u16,
        ack: u16,
        data: Vec<u8>,
    ) -> Packet {
        Packet {
            opcode: op,
            forward: 0,
            dest: to.0,
            dest_index: to.1,
            source: self.address,
            source_index: from_index,
            number,
            ack,
            data,
        }
    }

    /// A new index, or none with every slot taken, the machine's own NCP's
    /// way (`sys/network/chaos/chsncp.lisp`): the next free slot after the
    /// last one given out, taken round the table
    /// (`INDEX-CONN-FREE-POINTER`), and above it that slot's uniquizer, one
    /// more than it was (`UNIQUIZER-TABLE`). So an index is not given out
    /// again until the table has gone round, and the slot's uniquizer with
    /// it, and a packet for a connection that has gone --- a CLS that
    /// crossed this end's own --- finds no connection rather than the next
    /// one. The table grows to the slots in use.
    fn free_index(&mut self) -> Option<u16> {
        let usable = SLOTS - 1;
        for k in 0..usable {
            let s = (self.next_slot - 1 + k) % usable + 1;
            if s >= self.conns.len() {
                self.conns.resize_with(s + 1, || None);
                self.uniquizers.resize(s + 1, 0);
            }
            if self.conns[s].is_none() {
                self.next_slot = s % usable + 1;
                let u = self.uniquizers[s].wrapping_add(1) & (u16::MAX >> SLOT_BITS);
                self.uniquizers[s] = u;
                return Some((u << SLOT_BITS) | s as u16);
            }
        }
        None
    }

    /// The connection an index names: the one in its slot, if its index is
    /// the whole of this one. A packet for a connection that has gone
    /// carries the uniquizer it had, and finds none.
    fn conn(&self, index: u16) -> Option<&Conn> {
        self.conns.get(slot(index))?.as_ref().filter(|c| c.local == index)
    }

    fn conn_mut(&mut self, index: u16) -> Option<&mut Conn> {
        self.conns.get_mut(slot(index))?.as_mut().filter(|c| c.local == index)
    }

    /// The connection an index names, out of the table, its slot free.
    fn take(&mut self, index: u16) -> Option<Conn> {
        let s = self.conns.get_mut(slot(index))?;
        if s.as_ref().is_some_and(|c| c.local == index) { s.take() } else { None }
    }

    /// Every connection's index, in slot order.
    fn indices(&self) -> Vec<u16> {
        self.conns.iter().flatten().map(|c| c.local).collect()
    }

    /// Gives up a connection silent past [`HOST_DOWN_NS`], as the band's
    /// `PROBE-CONN` moves a host to `HOST-DOWN-STATE` when nothing has been
    /// received on it for that long (`sys/network/chaos/chsncp.lisp`): its
    /// unreceipted packets stop going again, its index is freed, and an RFC
    /// that reuses its (source, index) --- which a reboot of the same band
    /// sends, its indices seeded from a clock the simulator repeats --- is
    /// no longer taken for a duplicate. The comparison is strict, as the
    /// band's `(> DELTA-TIME HOST-DOWN-INTERVAL)` is.
    fn expire(&mut self, now: u64) {
        for index in self.indices() {
            let dead =
                self.conn(index).is_some_and(|c| now.saturating_sub(c.last_heard) > HOST_DOWN_NS);
            if dead {
                let secs = HOST_DOWN_NS / 1_000_000_000;
                self.note(index, format_args!("closed, host down: nothing heard for {secs} s"));
                self.close(now, index, "Host down");
            }
        }
    }

    /// A refusal, its reason cut to the bytes a packet carries: a CLS is a
    /// controlled-nothing packet like any other, and a reason quoting a
    /// long contact name back could run past [`MAX_DATA`] --- the count
    /// word is twelve bits, so an over-long frame would go out past the
    /// interface's buffer (AIM-628 §3.5). The log has the reason whole,
    /// with the `contact` the RFC asked for.
    fn refuse(&mut self, to: (u16, u16), number: u16, contact: &str, reason: String) {
        if let Some(log) = &self.log {
            log(&line(contact, false, to.0, format_args!("refused: {}", OneLine(&reason))));
        }
        let mut bytes = reason.into_bytes();
        bytes.truncate(MAX_DATA);
        let cls = self.packet(op::CLS, to, 0, number, number, bytes);
        self.send(cls);
    }

    /// A request for connection, from `from`, with the contact name and
    /// arguments in `data`.
    ///
    /// `cls_on_error` is the machine's own `CLS-ON-ERROR-P`: an RFC for a
    /// contact no service takes is refused with a CLS, and a BRD is not
    /// (`sys/network/chaos/chsncp.lisp:1588`).
    fn rfc(&mut self, now: u64, p: &Packet, cls_on_error: bool) {
        let from = (p.source, p.source_index);
        // A connection whose peer has fallen silent is given up first, so
        // that its (source, index) does not shadow this RFC: a reboot of
        // the same band reuses the index while the old connection is still
        // in the table.
        self.expire(now);
        // §4.1: "an NCP receives an RFC packet, it checks all pending RFC's
        // and all connections which are in the Open or RFC-received state,
        // to see if the source address and index match; if so, the RFC is
        // a duplicate and is discarded."
        if self.conns.iter().flatten().any(|c| c.remote == from) {
            return;
        }
        let text = String::from_utf8_lossy(&p.data).into_owned();
        let (name, args) = match text.split_once(' ') {
            Some((n, a)) => (n.to_string(), a.to_string()),
            None => (text.clone(), String::new()),
        };
        let Some(k) = self.services.iter().position(|s| s.contact() == name) else {
            if cls_on_error {
                self.refuse(from, p.number, &name, format!("No server for contact name {name}"));
            }
            return;
        };
        match self.services[k].request(now, &args, from) {
            Response::Answer(data) => {
                let ans = self.packet(op::ANS, from, 0, p.number, p.number, data);
                self.send(ans);
                if self.log_simple
                    && let Some(log) = &self.log
                {
                    log(&line(&name, false, from.0, format_args!("answered")));
                }
            }
            Response::Refuse(reason) => {
                self.refuse(from, p.number, &name, reason);
            }
            Response::Accept(session) => {
                let Some(index) = self.free_index() else {
                    return self.refuse(from, p.number, &name, "Connection table full".into());
                };
                let window = self.window;
                // §4.1: the OPN "conveys the server's index number ... its
                // data field is the same as that of STS", a receipt and a
                // window; its acknowledgement field acknowledges the RFC.
                let initial = 1u16;
                let conn = Conn {
                    local: index,
                    state: State::OpnSent,
                    remote: from,
                    contact: name.into_boxed_str(),
                    from_this_end: false,
                    session: Some(session),
                    next_number: initial.wrapping_add(1),
                    unacked: VecDeque::new(),
                    their_window: 1,
                    last_received: p.number,
                    last_acked: p.number,
                    window,
                    last_heard: now,
                    backlog: VecDeque::new(),
                };
                let mut data = Vec::new();
                data.extend_from_slice(&p.number.to_le_bytes());
                data.extend_from_slice(&window.to_le_bytes());
                let opn = self.packet(op::OPN, from, index, initial, p.number, data);
                self.conns[slot(index)] = Some(conn);
                self.remember(index, &opn, now);
                self.send(opn);
                self.note(index, format_args!("opened"));
            }
        }
    }

    /// Opens a connection from this host: an RFC to `contact` at `host`,
    /// AIM-628 §4.1, the connection in the RFC-sent state until the OPN.
    /// Its index; or none with every index in use, when nothing is sent and
    /// the session is told so, as a close.
    pub fn connect(
        &mut self,
        now: u64,
        host: u16,
        contact: &str,
        mut session: Box<dyn Session>,
    ) -> Option<u16> {
        let Some(index) = self.free_index() else {
            session.closed(now, "Connection table full");
            return None;
        };
        let initial = 1u16;
        let conn = Conn {
            local: index,
            state: State::RfcSent,
            remote: (host, 0),
            contact: contact.split_once(' ').map_or(contact, |(name, _)| name).into(),
            from_this_end: true,
            session: Some(session),
            next_number: initial.wrapping_add(1),
            unacked: VecDeque::new(),
            their_window: 1,
            last_received: 0,
            last_acked: 0,
            window: self.window,
            last_heard: now,
            backlog: VecDeque::new(),
        };
        self.conns[slot(index)] = Some(conn);
        let rfc = self.packet(op::RFC, (host, 0), index, initial, 0, contact.as_bytes().to_vec());
        self.remember(index, &rfc, now);
        self.send(rfc);
        Some(index)
    }

    /// Keeps a controlled packet for retransmission until receipted.
    fn remember(&mut self, index: u16, p: &Packet, now: u64) {
        if let Some(c) = self.conn_mut(index) {
            c.unacked.push_back((p.number, p.clone(), now));
        }
    }

    /// A receipt or acknowledgement of our packets up to `number`.
    fn receipted(&mut self, index: u16, number: u16) {
        if let Some(c) = self.conn_mut(index) {
            c.unacked.retain(|&(n, _, _)| n.wrapping_sub(number) as i16 > 0);
        }
    }

    fn sts(&mut self, index: u16) {
        let Some(c) = self.conn(index) else { return };
        let mut data = Vec::new();
        data.extend_from_slice(&c.last_received.to_le_bytes());
        data.extend_from_slice(&c.window.to_le_bytes());
        let p = self.packet(op::STS, c.remote, index, c.next_number, c.last_received, data);
        if let Some(c) = self.conn_mut(index) {
            c.last_acked = c.last_received;
        }
        self.send(p);
    }

    fn close(&mut self, now: u64, index: u16, reason: &str) {
        if let Some(mut c) = self.take(index)
            && let Some(s) = c.session.as_mut()
        {
            s.closed(now, reason);
        }
    }

    /// Tells the log, if there is one, `what` came of the connection at
    /// `index`, in a line naming its contact and the other host as
    /// [`Ncp::log`] shows. Without a log nothing is looked up or formatted.
    fn note(&self, index: u16, what: fmt::Arguments) {
        let Some(log) = &self.log else { return };
        let Some(c) = self.conn(index) else { return };
        log(&line(&c.contact, c.from_this_end, c.remote.0, what));
    }

    /// A packet for one of our connections.
    fn on_connection(&mut self, now: u64, index: u16, p: &Packet) {
        let Some(c) = self.conn(index) else {
            // §4.2: "LOS is sent in response to situations such as: arrival
            // of a data packet or an STS for a connection that does not
            // exist".
            if !matches!(p.opcode, op::LOS | op::CLS) {
                let los = self.packet(
                    op::LOS,
                    (p.source, p.source_index),
                    index,
                    0,
                    0,
                    b"No such connection".to_vec(),
                );
                self.send(los);
            }
            return;
        };
        // A packet for a connection comes from the far end's host and
        // index. An RFC from this end has no far index yet, so its answer
        // --- an OPN, a refusal's CLS, an ANS, a FWD --- is taken whatever
        // index it carries, but only from the host the RFC went to: from
        // any other it is no answer.
        if c.remote != (p.source, p.source_index)
            && !(c.state == State::RfcSent
                && p.source == c.remote.0
                && matches!(p.opcode, op::OPN | op::CLS | op::ANS | op::FWD))
        {
            return;
        }
        // Every packet's acknowledgement field receipts our packets.
        if p.opcode != op::LOS {
            self.receipted(index, p.ack);
        }
        // A packet from the other end means it is alive: the host-down
        // timer starts again from here.
        if let Some(c) = self.conn_mut(index) {
            c.last_heard = now;
        }
        match p.opcode {
            op::OPN => {
                let Some(c) = self.conn_mut(index) else { return };
                if c.state == State::RfcSent {
                    // The OPN answers the RFC whatever its acknowledgement
                    // field says: the band's OPN did not receipt it, and
                    // the RFC went out again every retransmission interval
                    // for the whole of a file transfer.
                    c.unacked.retain(|(_, q, _)| q.opcode != op::RFC);
                    c.remote = (p.source, p.source_index);
                    c.state = State::Open;
                    c.last_received = p.number;
                    c.last_acked = p.number;
                    if p.data.len() >= 4 {
                        c.their_window = u16::from_le_bytes([p.data[2], p.data[3]]);
                    }
                    self.sts(index);
                    self.note(index, format_args!("opened"));
                    if let Some(c) = self.conn_mut(index)
                        && let Some(s) = c.session.as_mut()
                    {
                        s.opened(now);
                    }
                    self.pump(now, index);
                } else {
                    // "if an OPN is received for a connection which is not
                    // in the RFC-sent state, it is simply discarded and an
                    // STS is sent."
                    self.sts(index);
                }
            }
            op::STS => {
                if p.data.len() >= 4 {
                    let receipt = u16::from_le_bytes([p.data[0], p.data[1]]);
                    let window = u16::from_le_bytes([p.data[2], p.data[3]]);
                    self.receipted(index, receipt);
                    if let Some(c) = self.conn_mut(index) {
                        c.their_window = window;
                        if c.state == State::OpnSent {
                            c.state = State::Open;
                        }
                    }
                    self.pump(now, index);
                }
            }
            op::SNS => self.sts(index),
            op::CLS | op::LOS => {
                let reason = String::from_utf8_lossy(&p.data).into_owned();
                // A CLS for an RFC from this end is the other end's refusal,
                // as a refusal from this one is a CLS ([`Response::Refuse`]):
                // the connection never opened.
                let refused = p.opcode == op::CLS
                    && self.conn(index).is_some_and(|c| c.state == State::RfcSent);
                if refused {
                    self.note(index, format_args!("refused: {}", OneLine(&reason)));
                } else {
                    let by = if p.opcode == op::CLS { "CLS" } else { "LOS" };
                    self.note(index, format_args!("closed by its {by}: {}", OneLine(&reason)));
                }
                self.close(now, index, &reason);
            }
            op::ANS => {
                // The answer to a simple transaction we started. No
                // service here starts one, so the connection is simply
                // closed; a client of the transport would take the data
                // first. Nor is it logged: a simple transaction is not a
                // connection.
                self.close(now, index, "answered");
            }
            op::EOF | op::UNC => self.controlled(now, index, p),
            o if op::is_data(o) => self.controlled(now, index, p),
            _ => {}
        }
    }

    /// A controlled packet in: taken in order, receipted otherwise.
    fn controlled(&mut self, now: u64, index: u16, p: &Packet) {
        let Some(c) = self.conn_mut(index) else { return };
        if c.state == State::OpnSent {
            // Data may arrive before the STS: "the user process may begin
            // transmitting data when it sees the OPN."
            c.state = State::Open;
        }
        let expected = c.last_received.wrapping_add(1);
        if p.opcode == op::UNC {
            if let Some(s) = c.session.as_mut() {
                s.data(now, p.opcode, &p.data);
            }
            return;
        }
        if p.number == expected {
            c.last_received = p.number;
            if let Some(s) = c.session.as_mut() {
                if p.opcode == op::EOF {
                    s.eof(now);
                } else {
                    s.data(now, p.opcode, &p.data);
                }
            }
            // An acknowledgement rides on our next packet; if the session
            // has nothing to say, an STS carries it. §3.8 batches these at
            // a third of the window; here every packet is acknowledged,
            // which the protocol allows and keeps the other side moving.
            self.pump(now, index);
            let needs_sts = self.conn(index).is_some_and(|c| c.last_acked != c.last_received);
            if needs_sts {
                self.sts(index);
            }
        } else if (p.number.wrapping_sub(expected) as i16) < 0 {
            // A duplicate: "evidence of unnecessary retransmission, and an
            // STS is generated to carry a receipt".
            self.sts(index);
        }
        // Out of order: dropped; they retransmit.
    }

    /// Sends what a session wants sent, as far as their window allows:
    /// AIM-628 §3.8, "The sending process is only allowed to emit packets
    /// whose packet numbers lie within the window." What the window has
    /// no room for waits in the connection's backlog for the next
    /// receipt.
    fn pump(&mut self, now: u64, index: u16) {
        loop {
            let Some(c) = self.conn_mut(index) else { return };
            // One packet in flight at a time, whatever window the other end
            // offers.  The CADR's interface holds one packet, and its
            // microcode drains it a word a Unibus cycle --- six microseconds
            // a pair when the disk is busy --- so a burst up to the window
            // lost most of its packets into the interface's full buffer,
            // and each loss cost a [`RETRANSMIT_NS`]: CC's files came at a
            // kilobyte a second and stalled for tens of seconds.  Waiting
            // for the receipt of each packet before the next is what a
            // careful host did, and it moves a file at the other end's
            // acknowledgement rate, some three milliseconds a packet.
            if !c.unacked.is_empty() {
                return;
            }
            let next = match c.backlog.pop_front() {
                Some(o) => o,
                None => {
                    let Some(s) = c.session.as_mut() else { return };
                    let outs = s.poll(now);
                    if outs.is_empty() {
                        return;
                    }
                    c.backlog.extend(outs);
                    c.backlog.pop_front().unwrap()
                }
            };
            let Some(c) = self.conn(index) else { return };
            let (remote, number, ack) = (c.remote, c.next_number, c.last_received);
            let p = match next {
                Out::Connect { host, contact, session } => {
                    self.connect(now, host, &contact, session);
                    continue;
                }
                Out::Data(bytes) => self.packet(op::DAT, remote, index, number, ack, bytes),
                Out::DataOp(op, bytes) => self.packet(op, remote, index, number, ack, bytes),
                Out::Eof => self.packet(op::EOF, remote, index, number, ack, Vec::new()),
                Out::Close(reason) => {
                    self.note(index, format_args!("closed by our CLS: {}", OneLine(&reason)));
                    // Cut to what a packet carries, as `refuse` cuts a refusal's.
                    let mut bytes = reason.into_bytes();
                    bytes.truncate(MAX_DATA);
                    let p = self.packet(op::CLS, remote, index, number, ack, bytes);
                    self.send(p);
                    self.take(index);
                    return;
                }
            };
            assert!(p.data.len() <= MAX_DATA, "a session offered {} bytes", p.data.len());
            if let Some(c) = self.conn_mut(index) {
                c.next_number = c.next_number.wrapping_add(1);
                c.last_acked = c.last_received;
            }
            self.remember(index, &p, now);
            self.send(p);
        }
    }

    /// Lets every session send what it has, and retransmits what has gone
    /// unreceipted too long.
    fn service_all(&mut self, now: u64) {
        // Give up connections whose peer has gone silent before doing any
        // more work for them.
        self.expire(now);
        for index in self.indices() {
            self.pump(now, index);
            let mut again = Vec::new();
            if let Some(c) = self.conn_mut(index) {
                for (_, p, last) in c.unacked.iter_mut() {
                    if now.saturating_sub(*last) >= RETRANSMIT_NS {
                        *last = now;
                        again.push(p.clone());
                    }
                }
            }
            for p in again {
                self.send(p);
            }
        }
    }

    /// One packet arrived at `now`: recorded, and dispatched by its opcode.
    pub fn handle(&mut self, now: u64, p: &Packet) {
        if self.trace {
            eprintln!(
                "chaos {now:>6}: {} -> {} {} {}",
                fmt_end((p.source, p.source_index)),
                fmt_end((p.dest, p.dest_index)),
                op_name(p.opcode),
                fmt_data(p.opcode, &p.data)
            );
        }
        match p.opcode {
            op::RFC => self.rfc(now, p, true),
            op::BRD => {
                // §4.5: "a subnet bit map followed by a contact name and
                // possible arguments"; the acknowledgement field is the
                // map's length in bytes. Stripped, it is an RFC.
                let skip = (p.ack as usize).min(p.data.len());
                let mut q = p.clone();
                q.opcode = op::RFC;
                q.data = p.data[skip..].to_vec();
                // And a BRD no service takes is let fall, not refused: the
                // machine's own `RECEIVE-BRD` hands it on with `CLS-ON-ERROR-P`
                // nil (`sys/network/chaos/chsncp.lisp:1613`, `:1588`).
                self.rfc(now, &q, false);
            }
            op::RUT | op::MNT => {}
            _ => self.on_connection(now, p.dest_index, p),
        }
    }

    /// A packet the link handed over at `now`, as a CHUDP datagram
    /// delivered it: kept and handled if it is addressed to this host or
    /// is a BRD to 0. Anything else is not this host's, and a buffer that
    /// holds no packet is dropped.
    ///
    /// **A bad check word is not a reason to drop it.** One that is not
    /// what the CADR's hardware would have made is printed under `trace`
    /// and the packet handled all the same: what a CHUDP peer puts in the
    /// trailer's third word is unverified (`chudp::unwrap`), and UDP
    /// carries a checksum of its own --- optional over IPv4, where a sender
    /// may leave it zero (`DESIGN.md` §5).
    pub fn receive(&mut self, now: u64, packet: &Framed) {
        let Ok((p, _)) = Packet::from_buffer(&packet.buffer) else { return };
        if p.dest != self.address && !(p.dest == 0 && p.opcode == op::BRD) {
            return;
        }
        if !packet.check_ok && self.trace {
            eprintln!(
                "chaos {now:>6}: {} -> {} {} check word {:o} does not match; handled",
                fmt_end((p.source, p.source_index)),
                fmt_end((p.dest, p.dest_index)),
                op_name(p.opcode),
                packet.check
            );
        }
        self.handle(now, &p);
    }

    /// The next buffer to send, at `now`: the words as the software would
    /// write them, cable destination last, for the link to add the source
    /// and the check word (`DESIGN.md` §5). With nothing queued, every
    /// session is first let send what it has and what has gone unreceipted
    /// too long is sent again: the NCP has no timer of its own, and this is
    /// when its retransmission and its host-down interval are kept
    /// (`DESIGN.md` §4).
    pub fn transmit(&mut self, now: u64) -> Option<Vec<u16>> {
        if self.out.is_empty() {
            self.service_all(now);
        }
        self.out.pop_front()
    }
}

/// A line for the log: the `contact`, `from` the other host `host` if it
/// asked for the connection and `to` it if this end did, and `what` came of
/// it --- `ECHO from 3050 opened` ([`Ncp::log`]).
fn line(contact: &str, from_this_end: bool, host: u16, what: fmt::Arguments) -> String {
    let way = if from_this_end { "to" } else { "from" };
    format!("{} {way} {host:o} {what}", OneLine(contact))
}

/// Text off the network, fit for one line of the log: a control character
/// --- a newline among them, which would start a line of the other end's
/// writing --- written as its escape, `\n`.
struct OneLine<'a>(&'a str);

impl fmt::Display for OneLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for c in self.0.chars() {
            if c.is_control() {
                write!(f, "{}", c.escape_debug())?;
            } else {
                fmt::Write::write_char(f, c)?;
            }
        }
        Ok(())
    }
}

fn fmt_end((a, i): (u16, u16)) -> String {
    format!("{a:o}/{i}")
}

fn fmt_data(op: u8, data: &[u8]) -> String {
    if op::is_data(op) && op >= op::DWD {
        return format!("{} words", data.len() / 2);
    }
    let printable = data.iter().all(|&b| (0x20..0x7f).contains(&b) || b == b'\r' || b == b'\n');
    if printable && !data.is_empty() {
        format!("{:?}", String::from_utf8_lossy(data))
    } else {
        format!("{} bytes", data.len())
    }
}

pub fn op_name(op: u8) -> String {
    match op {
        op::RFC => "RFC".into(),
        op::OPN => "OPN".into(),
        op::CLS => "CLS".into(),
        op::FWD => "FWD".into(),
        op::ANS => "ANS".into(),
        op::SNS => "SNS".into(),
        op::STS => "STS".into(),
        op::RUT => "RUT".into(),
        op::LOS => "LOS".into(),
        op::LSN => "LSN".into(),
        op::MNT => "MNT".into(),
        op::EOF => "EOF".into(),
        op::UNC => "UNC".into(),
        op::BRD => "BRD".into(),
        o if o >= op::DWD => format!("DWD{:o}", o),
        o if o >= op::DAT => format!("DAT{:o}", o),
        o => format!("op{o:o}"),
    }
}
