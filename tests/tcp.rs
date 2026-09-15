// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A TCP listener carried to a Chaosnet stream, `--tcp` (`docs/design.md`
//! §8): over loopback, a TCP client, the daemon, and a test host serving the
//! contact, at a clock the test sets. Nothing listens without the flag; each
//! TCP connection is a stream from this host, bytes both ways; and when
//! either end goes, so does the other.

mod support;

use ozd::daemon::Daemon;
use ozd::ncp::{HOST_DOWN_NS, Out, Response, Service, Session};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{LM1, OZ, SECOND, TestHost, ask, daemon, settle, site};

/// A stream service at the test host, `ECHO`: it sends back what it is
/// sent, and closes the connection when it is sent `bye`.
struct Echo;

struct EchoSession(Vec<Out>);

impl Service for Echo {
    fn contact(&self) -> &str {
        "ECHO"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Accept(Box::new(EchoSession(Vec::new())))
    }
}

impl Session for EchoSession {
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        if bytes == b"bye" {
            self.0.push(Out::Close("bye".to_string()));
        } else {
            self.0.push(Out::Data(bytes.to_vec()));
        }
    }
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.0)
    }
}

/// A TCP client of the daemon's listener: what it has read so far, and
/// whether the daemon has closed its end.
struct Client {
    stream: TcpStream,
    got: Vec<u8>,
    closed: bool,
}

impl Client {
    fn to(at: SocketAddr) -> Client {
        let stream = TcpStream::connect(at).expect("the listener takes a connection");
        stream.set_nonblocking(true).expect("a non-blocking client");
        Client { stream, got: Vec::new(), closed: false }
    }

    /// Whatever has arrived, without waiting.
    fn read(&mut self) {
        let mut buffer = [0u8; 4096];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.closed = true;
                    return;
                }
                Ok(n) => self.got.extend_from_slice(&buffer[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == ErrorKind::ConnectionReset => {
                    self.closed = true;
                    return;
                }
                Err(e) => panic!("the client's read: {e}"),
            }
        }
    }
}

/// Turns the daemon and the host at `now`, round after round, until `done`
/// says so of the daemon; the loopback gets a moment between rounds, since
/// a TCP byte is not a datagram the daemon's meters count.
fn until(d: &mut Daemon, lm1: &mut TestHost, now: u64, mut done: impl FnMut(&mut Daemon) -> bool) {
    for _ in 0..300 {
        settle(d, &mut [&mut *lm1], now);
        if done(d) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("what the test waited for never came");
}

/// A daemon carrying a TCP listener to `contact` at LM1, and LM1 serving
/// `ECHO`, heard from once so that the daemon knows where it is.
fn carrying(contact: &str) -> (Daemon, TestHost) {
    let mut d = daemon(&site(&format!("--tcp 127.0.0.1:0,{contact}@{LM1:o}\n")));
    let mut lm1 = TestHost::new(LM1, d.at());
    lm1.ncp.serve(Box::new(Echo));
    ask(&mut d, &mut lm1, 0, "TIME");
    (d, lm1)
}

/// **Nothing listens without `--tcp`**: a shell on a TCP port is on only
/// where a site asks for it (`docs/design.md` §8).
#[test]
fn nothing_listens_without_a_tcp_flag() {
    assert!(daemon(&site("")).tcp_at().is_empty());
}

/// **A bare port listens on the loopback**, as `--listen` does, so that a
/// listener reaches no other host unless an address is named.
#[test]
fn a_bare_port_listens_on_the_loopback() {
    let d = daemon(&site(&format!("--tcp 0,ECHO@{LM1:o}\n")));
    let [at] = d.tcp_at()[..] else { panic!("one listener: {:?}", d.tcp_at()) };
    assert!(at.ip().is_loopback(), "{at}");
    assert_ne!(at.port(), 0, "at the port the system picked");
}

/// **A TCP connection is a stream, and bytes go both ways.** The client's
/// bytes go to the contact in DAT packets of at most 488 bytes, from this
/// host's own address, and what the contact sends comes back to the client
/// in order. When the client closes its end, this host sends EOF and then
/// CLS, and the connection is gone at both ends.
#[test]
fn a_tcp_connection_is_carried_to_a_stream_and_back() {
    let (mut d, mut lm1) = carrying("ECHO");
    let mut c = Client::to(d.tcp_at()[0]);
    let sent: Vec<u8> = (0..1000u32).map(|i| b'a' + (i % 26) as u8).collect();
    c.stream.write_all(&sent).expect("the client writes");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.got.len() >= sent.len()
    });
    assert_eq!(c.got, sent, "all of it back, in order");
    assert_eq!(d.ncp().connections(), 1, "one stream");
    let data_from_oz = lm1.packets().iter().filter(|p| p.source == OZ && p.opcode >= 0o200).count();
    assert!(data_from_oz >= 3, "a thousand bytes in more than two packets: {data_from_oz}");
    c.stream.shutdown(Shutdown::Write).expect("the client closes its end");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.closed
    });
    assert_eq!((d.ncp().connections(), lm1.ncp.connections()), (0, 0), "gone at both ends");
}

/// **The contact closing the stream closes the TCP connection.**
#[test]
fn the_contact_closing_closes_the_tcp_connection() {
    let (mut d, mut lm1) = carrying("ECHO");
    let mut c = Client::to(d.tcp_at()[0]);
    c.stream.write_all(b"bye").expect("the client writes");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.closed
    });
    assert_eq!(d.ncp().connections(), 0);
}

/// **A contact the host refuses closes the TCP connection**: the RFC comes
/// back as a CLS, and the client is not left waiting.
#[test]
fn a_refused_contact_closes_the_tcp_connection() {
    let (mut d, mut lm1) = carrying("NOSUCH");
    let mut c = Client::to(d.tcp_at()[0]);
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.closed
    });
    assert_eq!(d.ncp().connections(), 0);
}

/// **A host that never answers is given up, and so is the TCP connection**:
/// an RFC to a host not heard from goes nowhere, and after the NCP's
/// host-down interval the connection is gone and the client told.
#[test]
fn a_host_that_never_answers_is_given_up() {
    let mut d = daemon(&site(&format!("--tcp 127.0.0.1:0,ECHO@{LM1:o}\n")));
    let mut lm1 = TestHost::new(LM1, d.at());
    let mut c = Client::to(d.tcp_at()[0]);
    until(&mut d, &mut lm1, 0, |d| d.ncp().connections() == 1);
    c.read();
    assert!(!c.closed, "still waiting for the host");
    until(&mut d, &mut lm1, HOST_DOWN_NS + SECOND, |_| {
        c.read();
        c.closed
    });
    assert_eq!(d.ncp().connections(), 0);
}

/// The lines the daemon's TCP log is given, collected for the test in
/// place of the log itself.
fn logging(d: &mut Daemon) -> Arc<Mutex<Vec<String>>> {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let kept = lines.clone();
    d.set_tcp_log(Some(Arc::new(move |line: &str| kept.lock().unwrap().push(line.to_string()))));
    lines
}

/// **`--log-tcp` writes a line when a TCP connection opens, and one when it
/// closes**, and nothing for what passes between: where the client is, the
/// contact and its host by the site's table, and who closed it
/// (`docs/design.md` §10).
#[test]
fn log_tcp_writes_a_line_when_a_connection_opens_and_when_it_closes() {
    let (mut d, mut lm1) = carrying("ECHO");
    let lines = logging(&mut d);
    let mut c = Client::to(d.tcp_at()[0]);
    let from = c.stream.local_addr().expect("where the client is");
    c.stream.write_all(b"hello").expect("the client writes");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.got == b"hello"
    });
    c.stream.shutdown(Shutdown::Write).expect("the client closes its end");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.closed
    });
    assert_eq!(
        *lines.lock().unwrap(),
        [
            format!("TCP from {from} to ECHO at 3050 (?) opened"),
            format!("TCP from {from} to ECHO at 3050 (?) closed by the client"),
        ]
    );
}

/// **The contact closing is logged with its reason**, the CLS's.
#[test]
fn log_tcp_gives_the_contacts_reason_for_closing() {
    let (mut d, mut lm1) = carrying("ECHO");
    let lines = logging(&mut d);
    let mut c = Client::to(d.tcp_at()[0]);
    let from = c.stream.local_addr().expect("where the client is");
    c.stream.write_all(b"bye").expect("the client writes");
    until(&mut d, &mut lm1, 0, |_| {
        c.read();
        c.closed
    });
    assert_eq!(
        lines.lock().unwrap().last().map(String::as_str),
        Some(format!("TCP from {from} to ECHO at 3050 (?) closed: bye").as_str())
    );
}

/// **Two clients are two streams**, each at an index of its own, and each
/// gets its own bytes back.
#[test]
fn two_clients_are_two_streams() {
    let (mut d, mut lm1) = carrying("ECHO");
    let mut a = Client::to(d.tcp_at()[0]);
    let mut b = Client::to(d.tcp_at()[0]);
    a.stream.write_all(b"from a").expect("a writes");
    b.stream.write_all(b"from b").expect("b writes");
    until(&mut d, &mut lm1, 0, |_| {
        a.read();
        b.read();
        a.got.len() >= 6 && b.got.len() >= 6
    });
    assert_eq!((a.got.as_slice(), b.got.as_slice()), (&b"from a"[..], &b"from b"[..]));
    assert_eq!(d.ncp().connections(), 2);
}
