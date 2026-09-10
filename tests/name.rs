// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! NAME, asked as a band asks it. The scripted client is the machine's own
//! user end, `FINGER` (`sys/network/chaos/chsaux.lisp:457`), which a band
//! sends to its associated machine when given no host: a band's NCP opens
//! a stream to `NAME` at a second NCP holding the service --- two real
//! NCPs, as `tests/ncp.rs` drives them --- and reads the answer as
//! [`printed`] reads it, which is what `FINGER` prints.
//!
//! `FINGER`'s stream is for input only and without ASCII translation
//! (`chsaux.lisp:482-486`, with no gateway): it sends nothing on the
//! connection, reads the Lisp Machine character set as it comes, and ends a
//! line at `#/NEWLINE`, 0o215 (`:LINE-IN`, `sys/io/stream.lisp:535`;
//! `sys/io/rddefs.lisp:172`), which is [`NEWLINE`].

use muir_ah::lispm::{self, NEWLINE};
use muir_ah::ncp::{Ncp, Out, Session};
use muir_ah::packet::{self, Framed, Packet};
use muir_ah::service::name::Name;
use std::sync::{Arc, Mutex};

/// A packet as the link would hand it to the NCP, with the check word the
/// CADR's hardware would have made; `tests/ncp.rs` has the same.
fn arriving(p: &Packet) -> Framed {
    let buffer = p.to_buffer(p.dest);
    let mut over = buffer.clone();
    over.push(p.source);
    let check = packet::check_word(&over);
    Framed { buffer, source: p.source, check, check_ok: true }
}

/// What `h` sends next, as a packet.
fn next_from(h: &mut Ncp, now: u64) -> Option<Packet> {
    h.transmit(now).map(|b| Packet::from_buffer(&b).unwrap().0)
}

/// Carries every packet between two NCPs at `now`, as the link between
/// them would, until neither has anything more to send; `tests/ncp.rs` has
/// the same.
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

/// What reaches the user end's session.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Heard {
    Opened,
    Data(Vec<u8>),
    Eof,
    Closed(String),
}

/// The session the band's NCP holds for the user end: it writes down what
/// it hears, and closes on the EOF if `closes` --- as `FINGER` does, whose
/// input stream is closed on the way out of `WITH-OPEN-STREAM` with a CLS
/// and no reason (`BASIC-STREAM :CLOSE`, `sys/network/chaos/chuse.lisp`).
struct Recorder {
    heard: Arc<Mutex<Vec<Heard>>>,
    closes: bool,
    to_send: Vec<Out>,
}

impl Recorder {
    fn note(&self, h: Heard) {
        self.heard.lock().unwrap().push(h);
    }
}

impl Session for Recorder {
    fn opened(&mut self, _now: u64) {
        self.note(Heard::Opened);
    }
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.note(Heard::Data(bytes.to_vec()));
    }
    fn eof(&mut self, _now: u64) {
        self.note(Heard::Eof);
        if self.closes {
            self.to_send.push(Out::Close(String::new()));
        }
    }
    fn closed(&mut self, _now: u64, reason: &str) {
        self.note(Heard::Closed(reason.to_string()));
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.to_send)
    }
}

/// Asks this host at 3060 from a band's NCP at 3050, with `rfc` as the
/// RFC's text, the user end closing on the EOF if `closes`: what the user
/// end heard, and the connections left open at the band's end and at this
/// host's.
fn ask(rfc: &str, closes: bool) -> (Vec<Heard>, (usize, usize)) {
    let mut server = Ncp::new(0o3060);
    server.serve(Box::new(Name::new()));
    let mut band = Ncp::new(0o3050);
    let heard = Arc::new(Mutex::new(Vec::new()));
    let user = Recorder { heard: heard.clone(), closes, to_send: Vec::new() };
    band.connect(0, 0o3060, rfc, Box::new(user));
    shuttle(&mut band, &mut server, 0);
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    (heard, (band.connections(), server.connections()))
}

/// The one line the service answers with: that nobody is logged in, in the
/// machine's character set and ended by its newline.
fn nobody() -> Vec<u8> {
    [&b"Nobody is logged in."[..], &[NEWLINE]].concat()
}

/// `:LINE-IN`: the text up to the next newline, which it takes; or, at the
/// EOF, what is left (`sys/io/stream.lisp:535`, `:552`).
fn line_in<'a>(rest: &mut &'a str) -> &'a str {
    match rest.split_once(NEWLINE as char) {
        Some((line, after)) => {
            *rest = after;
            line
        }
        None => std::mem::take(rest),
    }
}

/// What `FINGER` prints of an answer, after the blank lines it begins with
/// (`chsaux.lisp:490-499`): its first line, or its second if the first is
/// empty; above it, if `brackets` names the host and the line has no
/// bracket of its own, that name in brackets; the line, ended by a newline;
/// and the rest as it came, to the EOF.
fn printed(answer: &[u8], brackets: Option<&str>) -> String {
    let text = lispm::from_bytes(answer);
    let mut rest = text.as_str();
    let mut first = line_in(&mut rest);
    if first.is_empty() {
        first = line_in(&mut rest);
    }
    let newline = NEWLINE as char;
    let mut out = String::new();
    if let Some(host) = brackets
        && !first.contains(['[', ']'])
    {
        out += &format!("[{host}]{newline}");
    }
    out += first;
    out.push(newline);
    out += rest;
    out
}

/// **One line, then an EOF**: that nobody is logged in, in one packet, then
/// the EOF. Then the service closes once its EOF is receipted, as the
/// machine's own server does: `GIVE-NAME` answers through
/// `FORMAT-AND-EOF` (`chsaux.lisp:427`; `chuse.lisp:550`), which closes its
/// stream, and closing sends the EOF, waits for its receipt and sends a
/// CLS with no reason (`BASIC-OUTPUT-STREAM :EOF` and `:BEFORE :CLOSE`,
/// `BASIC-STREAM :CLOSE`, `chuse.lisp`).
#[test]
fn name_answers_one_line_then_an_eof() {
    let (heard, connections) = ask("NAME", false);
    assert_eq!(
        heard,
        [Heard::Opened, Heard::Data(nobody()), Heard::Eof, Heard::Closed(String::new())]
    );
    assert_eq!(connections, (0, 0), "closed at both ends");
}

/// **`FINGER` prints the line, under the host's name.** It prints the
/// first line, and with `HACK-BRACKETS-P` --- which the keyboard's finger
/// command passes (`KBD-FINGER`, `sys/window/basstr.lisp:1200`) --- puts
/// the host's name in brackets above it unless the line has a bracket of
/// its own (`chsaux.lisp:494-497`); this one has none. It then closes its
/// stream, and the connection ends at both ends.
#[test]
fn finger_prints_the_line() {
    let (heard, connections) = ask("NAME", true);
    assert_eq!(heard, [Heard::Opened, Heard::Data(nobody()), Heard::Eof]);
    assert_eq!(connections, (0, 0), "closed at both ends");
    let newline = NEWLINE as char;
    assert_eq!(
        printed(&nobody(), Some("MIT-OZ")),
        format!("[MIT-OZ]{newline}Nobody is logged in.{newline}")
    );
    assert_eq!(printed(&nobody(), None), format!("Nobody is logged in.{newline}"));
}

/// **The RFC's arguments are ignored**, as the machine's own server ignores
/// them: `GIVE-NAME` `LISTEN`s for `NAME` and reads nothing from the RFC
/// (`chsaux.lisp:423-438`). `FINGER` sends `NAME`, a space and what it was
/// given (`chsaux.lisp:476`): a user, `/W` and a name for `WHOIS`, or
/// nothing after the space for the keyboard's `@host`. The answer is the
/// same for each.
#[test]
fn the_arguments_are_ignored() {
    let (plain, _) = ask("NAME", true);
    assert_eq!(plain, [Heard::Opened, Heard::Data(nobody()), Heard::Eof]);
    for rfc in ["NAME ANYONE", "NAME /W OZ", "NAME "] {
        let (heard, connections) = ask(rfc, true);
        assert_eq!(heard, plain, "{rfc:?}");
        assert_eq!(connections, (0, 0), "{rfc:?}");
    }
}
