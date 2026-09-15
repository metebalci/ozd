// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The harness (`DESIGN.md` §11), shared by the test binaries, each taking
//! what it needs.
//!
//! - **A daemon** built from the text of a file of flags, listening on
//!   the loopback at a port the system picks: [`site`] and [`daemon`].
//! - **The binary**, run where no file of flags of the user's is found:
//!   [`ozd`].
//! - **Test hosts**: each an [`Ncp`] at an address of its own on a
//!   loopback socket of its own, speaking CHUDP through [`chudp::wrap`] and
//!   [`chudp::unwrap`], with the daemon as its one peer. The NCP is
//!   symmetric, so a
//!   test host opens its connections with [`Ncp::connect`] and serves what
//!   it is given to serve.
//! - **One clock**, set by the test: [`settle`] turns the daemon and the
//!   hosts at the same `now` until nothing moves, so what a test asserts
//!   of a time is exact rather than timed.
//!
//! And [`arriving`], a packet as the link hands it to the NCP, which the
//! NCP's own tests drive it with.

#![allow(dead_code)]

use ozd::chudp;
use ozd::config::Config;
use ozd::daemon::Daemon;
use ozd::lispm;
use ozd::ncp::{Ncp, Out, Session, op};
use ozd::packet::{Framed, Packet};
use ozd::roots::Tree;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// This host in the tests: System 100's file and time host, `MIT-OZ` at
/// 3060 (`sys/site/hosts.text`).
pub const OZ: u16 = 0o3060;

/// Machines on its subnet: System 100's band, `MIT-LISPM-1` at 3050, and
/// the next two, as `DESIGN.md` §9 numbers a site's further machines.
pub const LM1: u16 = 0o3050;
pub const LM2: u16 = 0o3051;
pub const LM3: u16 = 0o3052;

/// How long a test host's socket waits for a datagram before taking the
/// quiet for all there is. Loopback delivers in microseconds; this is a
/// margin, not a rate.
const HOST_WAIT: Duration = Duration::from_millis(5);

/// How long the daemon waits for a datagram at each turn of [`settle`].
const DAEMON_WAIT: Duration = Duration::from_millis(10);

/// A second, in the nanoseconds of the daemon's clock (`DESIGN.md` §4).
pub const SECOND: u64 = 1_000_000_000;

// --- packets ---------------------------------------------------------------

/// A buffer with the source and checksum a CHUDP peer puts in its trailer,
/// so `check_ok`.
fn framed(buffer: Vec<u16>, source: u16) -> Framed {
    let mut over = buffer.clone();
    over.push(source);
    let check = chudp::checksum(&over);
    Framed { buffer, source, check, check_ok: true }
}

/// A packet as the link would hand it to the NCP: the buffer its sender
/// wrote, cable destination last, and the trailer's source and check word
/// --- the checksum, so `check_ok`.
pub fn arriving(p: &Packet) -> Framed {
    framed(p.to_buffer(p.dest), p.source)
}

/// `p` as a CHUDP datagram from `source`, as that host would send it: at
/// the cable destination `p.dest`, with `source` and the checksum in the
/// trailer.
pub fn datagram(p: &Packet, source: u16) -> Vec<u8> {
    let f = framed(p.to_buffer(p.dest), source);
    chudp::wrap(&f.buffer, f.source, f.check).expect("a frame")
}

/// The packet a datagram carries, and the cable destination it was sent
/// to.
pub fn packet(datagram: &[u8]) -> (Packet, u16) {
    let f = chudp::unwrap(datagram).expect("a frame");
    Packet::from_buffer(&f.buffer).expect("a packet")
}

/// An RFC for `contact` from index 0o21 of `from` to `to`.
pub fn rfc(from: u16, to: u16, contact: &str) -> Packet {
    Packet {
        opcode: op::RFC,
        forward: 0,
        dest: to,
        dest_index: 0,
        source: from,
        source_index: 0o21,
        number: 1,
        ack: 0,
        data: contact.as_bytes().to_vec(),
    }
}

// --- the daemon ----------------------------------------------------------

/// A directory of this test binary's own, under Cargo's
/// `CARGO_TARGET_TMPDIR` (`DESIGN.md` §11), made once.
pub fn scratch() -> &'static Path {
    static SCRATCH: OnceLock<PathBuf> = OnceLock::new();
    SCRATCH.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ozd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    })
}

/// An empty directory of its own in [`scratch`], for one daemon's base
/// root.
///
/// **A root of its own, each time.** The design has one daemon to a root
/// (`DESIGN.md` §6), and a daemon's startup treats a temporary in its root
/// as its own to remove --- its writability probe is named as one. Daemons
/// a test run starts side by side on one root raced: one's cleanup took
/// another's probe, and logged it as a temporary it could not remove.
pub fn own_root() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = scratch().join(format!("root-{n}"));
    std::fs::create_dir_all(&root).expect("a root of its own");
    root
}

/// A site's file of flags: this host at [`OZ`] as `MIT-OZ`, listening on
/// the loopback at a port the system picks, its base root an [`own_root`],
/// on lines 1 to 4; and then the lines in `more`, from line 5.
pub fn site(more: &str) -> String {
    let root = own_root();
    format!(
        "--address {OZ:o}\n--name MIT-OZ,OZ\n--listen 127.0.0.1:0\n--root {}\n{more}",
        root.display()
    )
}

/// The site's roots, checked as the startup checks them (`DESIGN.md` §6).
pub fn tree(config: &Config) -> Arc<Tree> {
    Arc::new(Tree::new(config.roots.clone()).expect("the site's roots"))
}

/// A daemon for the site `text` gives, bound and not yet turned.
pub fn daemon(text: &str) -> Daemon {
    let config = Config::parse(text).expect("the site's flags");
    Daemon::new(&config, tree(&config), ozd::config::Logging::default()).expect("a daemon")
}

/// The daemon's meters that count anything, in STATUS's order: every
/// datagram received, every one sent, those rejected for their length,
/// and those rejected for anything else (`DESIGN.md` §7).
pub fn meters(d: &Daemon) -> [u32; 4] {
    let m = d.meters();
    [&m.received, &m.transmitted, &m.bad_bit_count, &m.other_discarded]
        .map(|c| c.load(Ordering::Relaxed))
}

/// Everything the daemon's meters have counted, together: it moves
/// whenever a datagram arrives, goes out, or is dropped.
fn counted(d: &Daemon) -> u64 {
    meters(d).iter().map(|&m| m as u64).sum()
}

/// Turns the daemon and the hosts at `now`, round after round, until two
/// rounds in a row move nothing. At one `now` nothing is sent again, so
/// this ends; what it leaves is every answer to what the test started.
pub fn settle(d: &mut Daemon, hosts: &mut [&mut TestHost], now: u64) {
    let mut quiet = 0;
    for _ in 0..1000 {
        let before = counted(d);
        d.turn(now, DAEMON_WAIT);
        let mut moved = counted(d) != before;
        for h in hosts.iter_mut() {
            moved |= h.turn(now) > 0;
        }
        quiet = if moved { 0 } else { quiet + 1 };
        if quiet == 2 {
            return;
        }
    }
    panic!("the daemon and the test hosts never fell quiet");
}

/// Asks `contact` of the daemon from `host` at `now`, as a band's user end
/// asks a simple transaction: an RFC from the host's NCP, everything
/// turned until quiet, and the one ANS that came back --- which the host's
/// NCP took through `unwrap` too, ending the connection it had opened.
/// What the host heard is taken with it, so its `heard` is empty after.
pub fn ask(d: &mut Daemon, host: &mut TestHost, now: u64, contact: &str) -> Packet {
    host.take();
    let session = Recorder::default();
    host.ncp.connect(now, OZ, contact, Box::new(session.clone()));
    settle(d, &mut [host], now);
    let heard: Vec<Packet> = host.take().iter().map(|d| packet(d).0).collect();
    let answers: Vec<&Packet> =
        heard.iter().filter(|p| p.opcode == op::ANS && p.source == OZ).collect();
    assert_eq!(answers.len(), 1, "one answer to {contact}: {heard:?}");
    assert_eq!(session.events(), ["closed answered"], "and the asker's NCP took it");
    answers[0].clone()
}

// --- test hosts ------------------------------------------------------------

/// A socket on the loopback at a port the system picks, and where it is.
/// Its reads wait [`HOST_WAIT`] at most.
pub fn socket() -> (UdpSocket, SocketAddr) {
    let s = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("a socket");
    s.set_read_timeout(Some(HOST_WAIT)).expect("a read timeout");
    let at = s.local_addr().expect("where it is");
    (s, at)
}

/// Every datagram waiting at `socket`, as it came, oldest first.
pub fn hear(socket: &UdpSocket) -> Vec<Vec<u8>> {
    let mut heard = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((n, _)) => heard.push(buffer[..n].to_vec()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return heard;
            }
            Err(e) => panic!("a test host's socket: {e}"),
        }
    }
}

/// A host on the subnet that is not this one: an [`Ncp`] at its own
/// address, on its own loopback socket, whose one peer is the daemon.
pub struct TestHost {
    /// Its NCP, which the test opens connections from and gives services
    /// to.
    pub ncp: Ncp,
    socket: UdpSocket,
    /// Where its socket is: the endpoint the daemon learns for it.
    pub at: SocketAddr,
    /// Where it sends everything: the daemon.
    pub switch: SocketAddr,
    /// Every datagram it has had, as it came, oldest first.
    pub heard: Vec<Vec<u8>>,
}

impl TestHost {
    pub fn new(address: u16, switch: SocketAddr) -> TestHost {
        let (socket, at) = socket();
        TestHost { ncp: Ncp::new(address), socket, at, switch, heard: Vec::new() }
    }

    /// A datagram, whatever its bytes, from this host's socket to the switch.
    pub fn send_bytes(&self, datagram: &[u8]) {
        self.socket.send_to(datagram, self.switch).expect("a datagram goes");
    }

    /// One turn at `now`: every datagram the socket has, kept in
    /// [`TestHost::heard`] and handed to the NCP through `unwrap`; then
    /// everything the NCP has to send, through `wrap` with this host's
    /// address and the checksum, to the switch. How many
    /// datagrams moved.
    pub fn turn(&mut self, now: u64) -> usize {
        let mut moved = 0;
        for datagram in hear(&self.socket) {
            if let Ok(f) = chudp::unwrap(&datagram) {
                self.ncp.receive(now, &f);
            }
            self.heard.push(datagram);
            moved += 1;
        }
        while let Some(buffer) = self.ncp.transmit(now) {
            let f = framed(buffer, self.ncp.address());
            self.send_bytes(&chudp::wrap(&f.buffer, f.source, f.check).expect("a frame"));
            moved += 1;
        }
        moved
    }

    /// What it has heard, and nothing since.
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.heard)
    }

    /// The packets it has heard, oldest first.
    pub fn packets(&self) -> Vec<Packet> {
        self.heard.iter().map(|d| packet(d).0).collect()
    }
}

/// A session that writes down what happens to it, for the test to read,
/// and sends nothing.
#[derive(Clone, Default)]
pub struct Recorder(Arc<Mutex<Vec<String>>>);

impl Recorder {
    pub fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn note(&self, event: String) {
        self.0.lock().unwrap().push(event);
    }
}

impl Session for Recorder {
    fn opened(&mut self, _now: u64) {
        self.note("opened".into());
    }
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.note(format!("data {}", lispm::from_bytes(bytes)));
    }
    fn eof(&mut self, _now: u64) {
        self.note("eof".into());
    }
    fn closed(&mut self, _now: u64, reason: &str) {
        self.note(format!("closed {reason}"));
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        Vec::new()
    }
}

// --- the command line ------------------------------------------------------

/// Whether these tests run as root, which the daemon refuses to: asked of
/// `id -u`, since a test may not call `geteuid` itself --- `unsafe_code` is
/// denied for the whole package, tests included (`DESIGN.md` §2).
pub fn running_as_root() -> bool {
    let out = std::process::Command::new("id").arg("-u").output().expect("id -u runs");
    String::from_utf8_lossy(&out.stdout).trim() == "0"
}

/// The daemon's binary, run where nothing of the user's is: no
/// `OZD_RC`, and both the directory it is run from and its `HOME` a
/// directory of this test binary's own with no `.ozdrc` in it. So a
/// run reads no file of flags but one its test gives it --- by `-c`, by
/// `OZD_RC`, or by setting `HOME` or the directory again --- and never
/// the user's own.
pub fn ozd() -> Command {
    let away = scratch().join("no-file-of-flags");
    std::fs::create_dir_all(&away).expect("a directory with no file of flags");
    let mut c = Command::new(env!("CARGO_BIN_EXE_ozd"));
    c.env_remove("OZD_RC").env("HOME", &away).current_dir(&away);
    c
}
