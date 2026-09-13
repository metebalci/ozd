// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Asks a Chaosnet host for STATUS, TIME and UPTIME over CHUDP, as a band
//! asks for them, and prints the answers: a check, from any Unix host, that
//! a running ozd answers where it listens.
//!
//! ```text
//! cargo run --example ask <address>@<host>[:<port>] [<from>]
//! ```
//!
//! The first argument names the host as `--peer` names a peer
//! (`DESIGN.md` §8): its Chaos address, in octal or as `subnet:host`, then
//! where its CHUDP socket is, at 42042 unless a port is given. Where
//! `--peer` takes an IP address only, this takes a name too. Each request is
//! a simple transaction (`PROTOCOLS.md`): an RFC for the contact name, and
//! the ANS that comes back, or nothing within three seconds.
//!
//! **It sends from a Chaos address of its own**, `<from>`, or else the
//! asked host's subnet with host 376. A hub learns that address at this
//! socket as it learns any peer's (`DESIGN.md` §5), so a machine that has
//! it would have its packets sent here until it next sends one itself.
//! Give `<from>` if one of your machines is at host 376.
//!
//! The exit code is 0 if every request was answered, 1 if one was not, and
//! 2 for arguments that are not these.

use ozd::address::parse_address;
use ozd::chudp::{self, PORT};
use ozd::log;
use ozd::ncp::op;
use ozd::packet::{self, Packet};
use ozd::service::time::UNIX_EPOCH_UNIVERSAL;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::exit;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long an answer is waited for.
const WAIT: Duration = Duration::from_secs(3);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        usage();
    }
    let Some((host, at)) = peer(&args[0]) else {
        eprintln!("ask: {}: not <address>@<host>[:<port>]", args[0]);
        exit(2);
    };
    let from = args.get(1).map_or((host & 0o177400) | 0o376, |a| address(a));
    let any: SocketAddr = if at.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse().unwrap();
    let socket = UdpSocket::bind(any).expect("a socket");
    socket.set_read_timeout(Some(Duration::from_millis(200))).expect("a read timeout");
    println!("asking {host:o} at {at}, from {from:o}");

    let mut unanswered = false;
    let mut ask = |contact: &str, index: u16| {
        let answer = transaction(&socket, at, from, host, contact, index);
        unanswered |= answer.is_none();
        answer
    };

    // STATUS: the host's name, the first 32 bytes, padded with zeros.
    match ask("STATUS", 0o21) {
        Some(data) => {
            let name: String =
                data.iter().take(32).take_while(|&&b| b != 0).map(|&b| b as char).collect();
            println!("STATUS  {name}");
        }
        None => println!("STATUS  no answer"),
    }

    // TIME: seconds since 1900, 32 bits, least significant byte first.
    match ask("TIME", 0o22).as_deref().and_then(word) {
        Some(t) => {
            let unix = u64::from(t).saturating_sub(UNIX_EPOCH_UNIVERSAL);
            let here = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let off = unix as i64 - here as i64;
            println!("TIME    {} ({off:+} s from this host)", log::stamp(unix));
        }
        None => println!("TIME    no answer"),
    }

    // UPTIME: sixtieths of a second since the host started, as a band
    // reads it (`PROTOCOLS.md`, UPTIME).
    match ask("UPTIME", 0o23).as_deref().and_then(word) {
        Some(n) => {
            let s = n / 60;
            let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
            println!("UPTIME  {d}d {h}h {m}m {}s", s % 60);
        }
        None => println!("UPTIME  no answer"),
    }

    if unanswered {
        eprintln!("ask: no answer: nothing listens at {at}, or no host there is at {host:o}");
        exit(1);
    }
}

/// One simple transaction: an RFC for `contact` from `from` to `host`,
/// sent to the CHUDP socket at `at`, and the data of the ANS that comes
/// back from `host`; or `None` if none comes within [`WAIT`].
fn transaction(
    socket: &UdpSocket,
    at: SocketAddr,
    from: u16,
    host: u16,
    contact: &str,
    index: u16,
) -> Option<Vec<u8>> {
    let rfc = Packet {
        opcode: op::RFC,
        forward: 0,
        dest: host,
        dest_index: 0,
        source: from,
        source_index: index,
        number: 1,
        ack: 0,
        data: contact.as_bytes().to_vec(),
    };
    // The buffer a CADR's interface would send, and the check word its
    // hardware would put in the trailer: over the buffer and the source.
    let buffer = rfc.to_buffer(host);
    let mut over = buffer.clone();
    over.push(from);
    let datagram = chudp::wrap(&buffer, from, packet::check_word(&over))?;
    socket.send_to(&datagram, at).ok()?;
    let began = Instant::now();
    let mut bytes = [0u8; 2048];
    while began.elapsed() < WAIT {
        let Ok((n, _)) = socket.recv_from(&mut bytes) else { continue };
        let Ok(framed) = chudp::unwrap(&bytes[..n]) else { continue };
        let Ok((answer, _)) = Packet::from_buffer(&framed.buffer) else { continue };
        if answer.opcode == op::ANS && answer.source == host {
            return Some(answer.data);
        }
    }
    None
}

/// The first 32 bits of an answer, least significant byte first.
fn word(data: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?))
}

/// `<address>@<host>[:<port>]`, as `--peer` writes a peer: the Chaos
/// address, as [`parse_address`] takes it, and where its CHUDP socket is.
/// `None` if the word is not that.
fn peer(word: &str) -> Option<(u16, SocketAddr)> {
    let (address, at) = word.split_once('@')?;
    Some((parse_address(address)?, endpoint(at)?))
}

/// Where a CHUDP socket is: an IP address and a port, an IP address alone
/// at 42042, or, unlike `--peer`, a name, with a port or at 42042. IPv6 is
/// written bare, or in brackets before a port, as `--peer` takes it.
fn endpoint(word: &str) -> Option<SocketAddr> {
    if let Ok(at) = word.parse::<SocketAddr>() {
        return Some(at);
    }
    if let Ok(ip) = word.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, PORT));
    }
    let (name, port) = match word.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().ok()?),
        None => (word, PORT),
    };
    (name, port).to_socket_addrs().ok()?.next()
}

/// `word` as a Chaos address, octal or `subnet:host`, or the usage and exit
/// code 2.
fn address(word: &str) -> u16 {
    parse_address(word).unwrap_or_else(|| {
        eprintln!("ask: {word}: not a Chaos address, which is octal or subnet:host");
        exit(2)
    })
}

fn usage() -> ! {
    eprintln!("usage: cargo run --example ask <address>@<host>[:<port>] [<from>]");
    eprintln!("       each address in octal or as subnet:host; the port is 42042 unless given");
    exit(2);
}
