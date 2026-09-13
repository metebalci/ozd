// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Asks a Chaosnet host for STATUS, TIME and UPTIME over CHUDP, then lists
//! a directory through FILE and counts what is in it, as a band would ask
//! for each: a check, from any Unix host, that a running ozd answers where
//! it listens.
//!
//! ```text
//! cargo run --example ask <address>@<host>[:<port>] [<directory>] [<from>]
//! ```
//!
//! The first argument names the host as `--peer` names a peer
//! (`DESIGN.md` §8): its Chaos address, in octal or as `subnet:host`, then
//! where its CHUDP socket is, at 42042 unless a port is given. Where
//! `--peer` takes an IP address only, this takes a name too. An argument
//! that begins with `/` is the directory to list, `/` unless given.
//!
//! STATUS, TIME and UPTIME are simple transactions (`PROTOCOLS.md`): an RFC
//! for the contact name, and the ANS that comes back. The listing is a FILE
//! session as the band's `qfile.lisp` holds one (`sys/doc/chfile.text`): a
//! control connection to `FILE 1`, a `LOGIN`, a `DATA-CONNECTION` that the
//! server opens back to a contact this end serves, a `DIRECTORY`, whose
//! records come down that connection until its EOF, and a `CLOSE`. It logs
//! in as `ASK`, which a server with `--log-file` writes down, as `--log`
//! writes down the three simple transactions. Each step waits three seconds
//! at most.
//!
//! **It sends from a Chaos address of its own**, `<from>`, or else the
//! asked host's subnet with host 376. A hub learns that address at this
//! socket as it learns any peer's (`DESIGN.md` §5), so a machine that has
//! it would have its packets sent here until it next sends one itself.
//! Give `<from>` if one of your machines is at host 376.
//!
//! The exit code is 0 if everything was answered, 1 if something was not,
//! and 2 for arguments that are not these.

use ozd::address::parse_address;
use ozd::chudp::{self, PORT};
use ozd::lispm::{self, NEWLINE};
use ozd::log;
use ozd::ncp::{Ncp, Out, Response, Service, Session, op};
use ozd::packet::{self, Packet};
use ozd::service::time::UNIX_EPOCH_UNIVERSAL;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::exit;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long each answer is waited for.
const WAIT: Duration = Duration::from_secs(3);

/// The data connection's handles: the output handle is the contact name
/// the server calls back on (`sys/doc/chfile.text`, DATA-CONNECTION).
const INPUT: &str = "I0001";
const OUTPUT: &str = "O0001";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 3 {
        usage();
    }
    let Some((host, at)) = peer(&args[0]) else {
        eprintln!("ask: {}: not <address>@<host>[:<port>]", args[0]);
        exit(2);
    };
    let mut directory = "/".to_string();
    let mut from = None;
    for arg in &args[1..] {
        if arg.starts_with('/') {
            directory = arg.clone();
        } else {
            from = Some(address(arg));
        }
    }
    let from = from.unwrap_or((host & 0o177400) | 0o376);
    let any: SocketAddr = if at.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }.parse().unwrap();
    let socket = UdpSocket::bind(any).expect("a socket");
    socket.set_read_timeout(Some(Duration::from_millis(10))).expect("a read timeout");
    println!("asking {host:o} at {at}, from {from:o}");

    let mut unanswered = false;

    // STATUS: the host's name, the first 32 bytes, padded with zeros.
    match transaction(&socket, at, from, host, "STATUS", 0o21) {
        Some(data) => {
            let name: String =
                data.iter().take(32).take_while(|&&b| b != 0).map(|&b| b as char).collect();
            println!("STATUS  {name}");
        }
        None => {
            println!("STATUS  no answer");
            unanswered = true;
        }
    }

    // TIME: seconds since 1900, 32 bits, least significant byte first.
    match transaction(&socket, at, from, host, "TIME", 0o22).as_deref().and_then(word) {
        Some(t) => {
            let unix = u64::from(t).saturating_sub(UNIX_EPOCH_UNIVERSAL);
            let here = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let off = unix as i64 - here as i64;
            println!("TIME    {} ({off:+} s from this host)", log::stamp(unix));
        }
        None => {
            println!("TIME    no answer");
            unanswered = true;
        }
    }

    // UPTIME: sixtieths of a second since the host started, as a band
    // reads it (`PROTOCOLS.md`, UPTIME).
    match transaction(&socket, at, from, host, "UPTIME", 0o23).as_deref().and_then(word) {
        Some(n) => {
            let s = n / 60;
            let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
            println!("UPTIME  {d}d {h}h {m}m {}s", s % 60);
        }
        None => {
            println!("UPTIME  no answer");
            unanswered = true;
        }
    }

    // FILE: a directory listed, its directories and files counted.
    match list(&socket, at, from, host, &directory) {
        Ok((directories, files)) => println!(
            "FILE    {directory} holds {} and {}",
            count(directories, "directory", "directories"),
            count(files, "file", "files")
        ),
        Err(why) => {
            println!("FILE    {why}");
            unanswered = true;
        }
    }

    if unanswered {
        eprintln!("ask: not everything was answered by {host:o} at {at}");
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
    send(socket, at, from, &rfc.to_buffer(host));
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

/// A buffer as a CADR's interface would send it, with the check word its
/// hardware would put in the trailer, over the buffer and the source.
fn send(socket: &UdpSocket, at: SocketAddr, from: u16, buffer: &[u16]) {
    let mut over = buffer.to_vec();
    over.push(from);
    if let Some(datagram) = chudp::wrap(buffer, from, packet::check_word(&over)) {
        let _ = socket.send_to(&datagram, at);
    }
}

/// Lists `directory` at `host` through FILE, and counts its directories and
/// its files; or why that could not be done.
fn list(
    socket: &UdpSocket,
    at: SocketAddr,
    from: u16,
    host: u16,
    directory: &str,
) -> Result<(usize, usize), String> {
    let mut link = Link { ncp: Ncp::new(from), socket, at, from, start: Instant::now() };
    let data = End::default();
    link.ncp.serve(Box::new(Listen { contact: OUTPUT.to_string(), end: data.clone() }));
    let control = End::default();
    link.ncp
        .connect(link.now(), host, "FILE 1", Box::new(control.clone()))
        .ok_or("no connection could be made")?;
    link.until(|| control.with(|e| e.opened || e.closed.is_some()));
    if let Some(why) = control.with(|e| e.closed.clone()) {
        return Err(format!("refused: {why}"));
    }
    if !control.with(|e| e.opened) {
        return Err("no answer".to_string());
    }

    let nl = NEWLINE as char;
    link.command(&control, "T1", "T1  LOGIN ASK ASK ")?;
    link.command(&control, "T2", &format!("T2  DATA-CONNECTION {INPUT} {OUTPUT}"))?;
    let pattern =
        if directory.ends_with('/') { format!("{directory}*") } else { format!("{directory}/*") };
    link.command(&control, "T3", &format!("T3 {INPUT} DIRECTORY{nl}{pattern}{nl}"))?;
    if !link.until(|| data.with(|e| e.eof)) {
        return Err("the listing did not end".to_string());
    }
    let listing = lispm::from_bytes(&data.with(|e| std::mem::take(&mut e.data)));
    link.command(&control, "T4", &format!("T4 {INPUT} CLOSE"))?;
    // The session ends as a band's does, with a CLS on each connection.
    data.send(Out::Close(String::new()));
    control.send(Out::Close(String::new()));
    link.until(|| control.with(|e| e.closed.is_some()));
    Ok(entries(&listing))
}

/// The directories and the files a listing holds. A listing is records
/// parted by a blank line: the first is the file system's own, with an
/// empty pathname, and each after it a pathname and its property lines, a
/// directory's among them `DIRECTORY T` (`sys/doc/chfile.text`, DIRECTORY).
fn entries(listing: &str) -> (usize, usize) {
    let nl = NEWLINE as char;
    let blank: String = [nl, nl].iter().collect();
    let (mut directories, mut files) = (0, 0);
    for record in listing.split(blank.as_str()).skip(1) {
        if record.split(nl).all(str::is_empty) {
            continue;
        }
        if record.split(nl).any(|line| line == "DIRECTORY T") {
            directories += 1;
        } else {
            files += 1;
        }
    }
    (directories, files)
}

/// `n` and the noun for it.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// This end of a FILE session: its NCP, the socket it speaks through, and
/// the clock it keeps.
struct Link<'a> {
    ncp: Ncp,
    socket: &'a UdpSocket,
    at: SocketAddr,
    from: u16,
    start: Instant,
}

impl Link<'_> {
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    /// Every datagram waiting, to the NCP; then everything the NCP has to
    /// send, to the socket.
    fn turn(&mut self) {
        let now = self.now();
        let mut bytes = [0u8; 2048];
        while let Ok((n, _)) = self.socket.recv_from(&mut bytes) {
            if let Ok(framed) = chudp::unwrap(&bytes[..n]) {
                self.ncp.receive(now, &framed);
            }
        }
        while let Some(buffer) = self.ncp.transmit(now) {
            send(self.socket, self.at, self.from, &buffer);
        }
    }

    /// Turns until `done` or [`WAIT`] passes; whether `done` came.
    fn until(&mut self, done: impl Fn() -> bool) -> bool {
        let began = Instant::now();
        while began.elapsed() < WAIT {
            self.turn();
            if done() {
                return true;
            }
        }
        false
    }

    /// A FILE command on the control connection, and its reply, which
    /// begins with the command's transaction id; or the error it answers.
    fn command(&mut self, control: &End, tid: &str, command: &str) -> Result<String, String> {
        control.send(Out::Data(lispm::lispm_text(command)));
        let replied = self.until(|| control.with(|e| lispm::from_bytes(&e.data).starts_with(tid)));
        // The command's name: the first word all capitals and dashes, which
        // passes over the transaction id and a handle, both holding digits.
        let name = command
            .split(|c: char| c.is_whitespace() || c == NEWLINE as char)
            .find(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_uppercase() || c == '-'))
            .unwrap_or(command);
        if !replied {
            return Err(format!("no answer to {name}"));
        }
        let reply = lispm::from_bytes(&control.with(|e| std::mem::take(&mut e.data)));
        match reply.split_once(" ERROR ") {
            Some((_, why)) => Err(format!("{name}: {}", why.trim_end_matches(NEWLINE as char))),
            None => Ok(reply),
        }
    }
}

/// What has arrived on one connection, and what is to be sent on it,
/// shared between this program and the session the NCP holds.
#[derive(Clone, Default)]
struct End(Arc<Mutex<Heard>>);

#[derive(Default)]
struct Heard {
    opened: bool,
    data: Vec<u8>,
    eof: bool,
    closed: Option<String>,
    out: Vec<Out>,
}

impl End {
    fn with<T>(&self, f: impl FnOnce(&mut Heard) -> T) -> T {
        f(&mut self.0.lock().unwrap())
    }

    fn send(&self, out: Out) {
        self.with(|e| e.out.push(out));
    }
}

impl Session for End {
    fn opened(&mut self, _now: u64) {
        self.with(|e| e.opened = true);
    }
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.with(|e| e.data.extend_from_slice(bytes));
    }
    fn eof(&mut self, _now: u64) {
        self.with(|e| e.eof = true);
    }
    fn closed(&mut self, _now: u64, reason: &str) {
        self.with(|e| e.closed = Some(reason.to_string()));
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        self.with(|e| std::mem::take(&mut e.out))
    }
}

/// The contact the server calls back on for the data connection.
struct Listen {
    contact: String,
    end: End,
}

impl Service for Listen {
    fn contact(&self) -> &str {
        &self.contact
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Accept(Box::new(self.end.clone()))
    }
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

/// The first 32 bits of an answer, least significant byte first.
fn word(data: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(..4)?.try_into().ok()?))
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
    eprintln!("usage: cargo run --example ask <address>@<host>[:<port>] [<directory>] [<from>]");
    eprintln!("       each address in octal or as subnet:host; the port is 42042 unless given;");
    eprintln!("       the directory begins with / and is / unless given");
    exit(2);
}
