// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HOSTAB, asked as a band asks it. The scripted client is the machine's
//! own user end, `CHAOS-UNKNOWN-HOST-FUNCTION`
//! (`sys/network/chaos/chuse.lisp:950`), which a band calls when
//! `SI:PARSE-HOST` meets a name its own table lacks: a band's NCP opens a
//! stream to `HOSTAB` at a second NCP holding the service --- two real
//! NCPs, as `tests/ncp.rs` drives them --- sends the name as the user end
//! sends it, and reads the answer as [`define_host`] reads it, which is
//! what the user end does with each line before it hands the host to
//! `SI:DEFINE-HOST`.
//!
//! **The newline is the machine's, both ways.** The user end's stream is a
//! `CHARACTER-STREAM` without ASCII translation (`OPEN-STREAM`,
//! `chuse.lisp:782`), so what crosses the wire is the Lisp Machine
//! character set. It sends the name with `:LINE-OUT`, which ends it with
//! `#\CR` (`sys/io/stream.lisp:475`), and reads the answer with
//! `:LINE-IN`, which ends a line at `#/NEWLINE` (`sys/io/stream.lisp:535`);
//! both are 0o215 (`sys/io/rddefs.lisp:172`), which is [`NEWLINE`].

use muir_ah::config::Config;
use muir_ah::lispm::{self, NEWLINE};
use muir_ah::ncp::{Ncp, Out, Session};
use muir_ah::packet::{self, Framed, MAX_DATA, Packet};
use muir_ah::service::hostab::Hostab;
use std::sync::{Arc, Mutex};

/// The site the tests ask about: System 100's own names for its file host
/// and its machine (`examples/system-100.conf`), a second machine, and a
/// host whose line gives no system type.
const SITE: &str = "
address  3060
name     MIT-OZ OZ
root     /srv/lispm
host     3050  MIT-LISPM-1 CADR-1 CADR1 LM1  system=LISPM
host     3051  MIT-LISPM-2 LM2  system=LISPM
host     3040  BRIDGE-1
";

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

/// The user end's side of one connection: what it has heard, and what it
/// is to send next --- shared between the test and the session the band's
/// NCP holds.
#[derive(Clone, Default)]
struct UserEnd {
    heard: Arc<Mutex<Vec<Heard>>>,
    to_send: Arc<Mutex<Vec<Out>>>,
}

/// The packets of one answer, up to its EOF.
type Packets = Vec<Vec<u8>>;

impl UserEnd {
    fn send(&self, out: Out) {
        self.to_send.lock().unwrap().push(out);
    }

    /// `:LINE-OUT` and then `:FORCE-OUTPUT` (`chuse.lisp:956-957`): the
    /// name and the machine's newline after it, in one packet.
    fn line_out(&self, name: &str) {
        let mut line = lispm::lispm_text(name);
        line.push(NEWLINE);
        self.send(Out::Data(line));
    }

    fn take(&self) -> Vec<Heard> {
        std::mem::take(&mut *self.heard.lock().unwrap())
    }

    /// Every answer heard since the last look, the packets of each up to
    /// its EOF. What was heard ends in an EOF and is nothing but data and
    /// EOFs.
    fn answers(&self) -> Vec<Packets> {
        let heard = self.take();
        assert_eq!(heard.last(), Some(&Heard::Eof), "an answer ends in an EOF: {heard:?}");
        let mut answers = vec![Packets::new()];
        for h in heard {
            match h {
                Heard::Data(bytes) => answers.last_mut().unwrap().push(bytes),
                Heard::Eof => answers.push(Packets::new()),
                h => panic!("{h:?} among the answers"),
            }
        }
        answers.pop();
        answers
    }

    /// The one answer heard since the last look, as bytes.
    fn answer(&self) -> Vec<u8> {
        let answers = self.answers();
        assert_eq!(answers.len(), 1, "one answer");
        answers[0].concat()
    }
}

/// The session the band's NCP holds for the user end.
struct Recorder(UserEnd);

impl Recorder {
    fn note(&self, h: Heard) {
        self.0.heard.lock().unwrap().push(h);
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
    }
    fn closed(&mut self, _now: u64, reason: &str) {
        self.note(Heard::Closed(reason.to_string()));
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut *self.0.to_send.lock().unwrap())
    }
}

/// This host's NCP holding HOSTAB, made from a site file as the daemon
/// makes it, and a band's NCP at 3050 with a HOSTAB connection open to it.
struct Asking {
    band: Ncp,
    server: Ncp,
    user: UserEnd,
    now: u64,
}

impl Asking {
    fn new(site: &str) -> Asking {
        let site = Config::parse(site).expect("the site file");
        let mut server = Ncp::new(site.address);
        let hostab = Hostab::new(site.address, &site.names, site.system.as_deref(), &site.hosts);
        server.serve(Box::new(hostab));
        let mut band = Ncp::new(0o3050);
        let user = UserEnd::default();
        band.connect(0, site.address, "HOSTAB", Box::new(Recorder(user.clone())));
        shuttle(&mut band, &mut server, 0);
        assert_eq!(user.take(), [Heard::Opened], "a stream: the RFC is answered with an OPN");
        Asking { band, server, user, now: 0 }
    }

    /// Carries what each end has to send, a millisecond on.
    fn shuttle(&mut self) {
        self.now += 1_000_000;
        shuttle(&mut self.band, &mut self.server, self.now);
    }

    /// One transaction as the user end makes it: the name out, and the
    /// answer's bytes up to its EOF.
    fn ask(&mut self, name: &str) -> Vec<u8> {
        self.user.line_out(name);
        self.shuttle();
        self.user.answer()
    }

    /// The connections open at the band's end and at this host's.
    fn connections(&self) -> (usize, usize) {
        (self.band.connections(), self.server.connections())
    }
}

/// Lines as the service is to send them: each in the machine's character
/// set and ended by its newline.
fn lines(lines: &[&str]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|l| {
            let mut line = lispm::lispm_text(l);
            line.push(NEWLINE);
            line
        })
        .collect()
}

/// A host as the user end hands it to `SI:DEFINE-HOST` (`chuse.lisp:970`;
/// `sys/network/host.lisp:177`): its name, the car of the list, and from
/// the list's properties its `:HOST-NAMES`, its `:CHAOS` addresses and its
/// `:SYSTEM-TYPE`.
#[derive(Debug, PartialEq, Eq)]
struct Defined {
    name: String,
    host_names: Vec<String>,
    chaos: Vec<u16>,
    system_type: Option<String>,
}

/// What the user end makes of one answer, as `CHAOS-UNKNOWN-HOST-FUNCTION`
/// does it line by line (`chuse.lisp:958-988`), with an assertion wherever
/// the band itself would go wrong:
///
/// - **`:LINE-IN`** gives the text up to the next newline. At the EOF it
///   gives what is left with the EOF flag set (`sys/io/stream.lisp:552`),
///   and the user end does not look at that text (`chuse.lisp:963`): a last
///   line without its newline would be lost.
/// - **The attribute** is the text before the first space, interned as a
///   keyword, and the value what follows it (`:972-975`). A line without a
///   space fails there, at `(INCF SP)` of NIL.
/// - **`ERROR`** ends it, and no host is defined (`:977`).
/// - **`NAME`**: the first makes the list, and so is the host's name; each
///   is pushed onto its `:HOST-NAMES` (`:978-981`).
/// - **`SYSTEM-TYPE`** is interned as sent and put on the list
///   (`:982-983`), so one with a lower-case letter would be a keyword no
///   flavor is filed under (`COMPUTE-HOST-FLAVOR`,
///   `sys/network/host.lisp:279`). Every other attribute is put on the list
///   too (`:984-988`): none may come before the first `NAME`, when there is
///   no list to put it on.
/// - **Any other attribute is an address**, by its own
///   `HOST-ADDRESS-PARSER` or else the Chaosnet one, which reads octal
///   (`ZWEI:PARSE-NUMBER ... 8`, `sys/network/chaos/chsaux.lisp:1618`).
///   `MACHINE-TYPE` is one of these although the clause at `:982` names it,
///   as `PROTOCOLS.md` (HOSTAB) explains: it would be read as a Chaosnet
///   address.
/// - **At the EOF** the names are sorted shortest first, stably
///   (`:964-969`) --- after `PUSH` put them in last first.
fn define_host(answer: &[u8]) -> Result<Defined, String> {
    let text = lispm::from_bytes(answer);
    let mut lines: Vec<&str> = text.split(NEWLINE as char).collect();
    let unended = lines.pop().unwrap_or_default();
    assert!(
        unended.is_empty(),
        "{unended:?} has no newline, and :LINE-IN hands it back with the EOF"
    );
    let mut list: Option<String> = None;
    let (mut names, mut chaos, mut system_type) = (Vec::new(), Vec::new(), None);
    for line in lines {
        let Some((attribute, value)) = line.split_once(' ') else {
            panic!("{line:?} has no space, and (INCF SP) fails on it");
        };
        if attribute == "ERROR" {
            return Err(value.to_string());
        }
        if attribute == "NAME" {
            list.get_or_insert_with(|| value.to_string());
            names.push(value.to_string());
            continue;
        }
        assert!(list.is_some(), "{attribute} before any NAME, with no list to put it on");
        match attribute {
            "SYSTEM-TYPE" => {
                assert!(
                    !value.chars().any(char::is_lowercase),
                    "SYSTEM-TYPE {value} is interned as sent, and names no flavor"
                );
                system_type = Some(value.to_string());
            }
            "CHAOS" => chaos.push(
                u16::from_str_radix(value, 8)
                    .unwrap_or_else(|_| panic!("CHAOS {value} is not an octal address")),
            ),
            _ => panic!("{attribute} {value} would be read as a Chaosnet address"),
        }
    }
    names.reverse();
    names.sort_by_key(String::len);
    let name = list.ok_or("no NAME, and so no host")?;
    Ok(Defined { name, host_names: names, chaos, system_type })
}

/// Whether `PARSE-HOST`, looking again once the user end has defined the
/// host (`sys/network/host.lisp:318-320`), finds it by the name it asked:
/// that name among its `:HOST-NAMES`, ignoring case as `STRING-EQUAL`
/// does, and an address to reach it at (`:310-312`).
fn finds(asked: &str, host: &Defined) -> bool {
    host.host_names.iter().any(|n| n.eq_ignore_ascii_case(asked)) && !host.chaos.is_empty()
}

/// **A name is answered with every name of its host, the official first,
/// its address in octal, and its system type**, each on a line of its own,
/// then an EOF: these bytes, and the host the band defines from them.
#[test]
fn a_name_is_answered_as_the_band_reads_it() {
    let mut site = Asking::new(SITE);
    let answer = site.ask("MIT-LISPM-2");
    assert_eq!(answer, lines(&["NAME MIT-LISPM-2", "NAME LM2", "CHAOS 3051", "SYSTEM-TYPE LISPM"]));
    let host = define_host(&answer).expect("a host");
    assert_eq!(
        host,
        Defined {
            name: "MIT-LISPM-2".into(),
            host_names: vec!["LM2".into(), "MIT-LISPM-2".into()],
            chaos: vec![0o3051],
            system_type: Some("LISPM".into()),
        }
    );
    assert!(finds("MIT-LISPM-2", &host));
}

/// **A nickname, in any case, is the same host.** The name is looked up
/// ignoring case, as the band's own `PARSE-HOST` looks it up
/// (`STRING-EQUAL`, `sys/network/host.lisp:310`), and whichever name was
/// asked, the answer is every name in the order its line gives them, the
/// official first --- which the band makes the host's name.
#[test]
fn a_nickname_in_any_case_is_the_same_host() {
    let mut site = Asking::new(SITE);
    let want = lines(&[
        "NAME MIT-LISPM-1",
        "NAME CADR-1",
        "NAME CADR1",
        "NAME LM1",
        "CHAOS 3050",
        "SYSTEM-TYPE LISPM",
    ]);
    for asked in ["MIT-LISPM-1", "mit-lispm-1", "LM1", "lm1", "Lm1", "cadr-1", "CADR1"] {
        let answer = site.ask(asked);
        assert_eq!(answer, want, "asked {asked}");
        let host = define_host(&answer).unwrap();
        assert_eq!(host.name, "MIT-LISPM-1", "the official name is the host's");
        assert_eq!(
            host.host_names,
            ["LM1", "CADR1", "CADR-1", "MIT-LISPM-1"],
            "shortest first, as the band sorts them"
        );
        assert!(finds(asked, &host), "and the band finds it by {asked}");
    }
}

/// **This host's own names are answered too**, from its `name` and
/// `address` lines, and without a system type when the `name` line gives
/// none, as [`SITE`]'s does not.
#[test]
fn this_hosts_own_names_are_answered() {
    let mut site = Asking::new(SITE);
    for asked in ["MIT-OZ", "oz"] {
        let answer = site.ask(asked);
        assert_eq!(answer, lines(&["NAME MIT-OZ", "NAME OZ", "CHAOS 3060"]), "asked {asked}");
        let host = define_host(&answer).unwrap();
        assert_eq!(
            host,
            Defined {
                name: "MIT-OZ".into(),
                host_names: vec!["OZ".into(), "MIT-OZ".into()],
                chaos: vec![0o3060],
                system_type: None,
            }
        );
        assert!(finds(asked, &host));
    }
}

/// **This host's system type is its `name` line's `system=`**, answered for
/// its own names as a `host` line's is for that host, and for no other
/// host: one whose line gives none still has none.
#[test]
fn this_hosts_system_type_when_the_name_line_gives_one() {
    let text = SITE.replacen("MIT-OZ OZ\n", "MIT-OZ OZ  system=UNIX\n", 1);
    assert_ne!(text, SITE, "the name line gains a system=");
    let mut site = Asking::new(&text);
    for asked in ["MIT-OZ", "oz"] {
        let answer = site.ask(asked);
        assert_eq!(
            answer,
            lines(&["NAME MIT-OZ", "NAME OZ", "CHAOS 3060", "SYSTEM-TYPE UNIX"]),
            "asked {asked}"
        );
        let host = define_host(&answer).unwrap();
        assert_eq!(host.system_type.as_deref(), Some("UNIX"));
        assert!(finds(asked, &host));
    }
    assert_eq!(site.ask("BRIDGE-1"), lines(&["NAME BRIDGE-1", "CHAOS 3040"]));
    assert_eq!(define_host(&site.ask("LM1")).unwrap().system_type.as_deref(), Some("LISPM"));
}

/// **`SYSTEM-TYPE` only when the host line gives one**, and then as it is
/// written there.
#[test]
fn a_system_type_only_when_the_line_gives_one() {
    let mut site = Asking::new(SITE);
    assert_eq!(site.ask("BRIDGE-1"), lines(&["NAME BRIDGE-1", "CHAOS 3040"]));
    assert_eq!(define_host(&site.ask("bridge-1")).unwrap().system_type, None);
    assert_eq!(define_host(&site.ask("LM2")).unwrap().system_type.as_deref(), Some("LISPM"));
}

/// **Never `MACHINE-TYPE`.** The user end's clause for it is written
/// `(:SYSTEM-TYPE MACHINE-TYPE)`, the second without its colon, while the
/// attribute is interned as a keyword (`chuse.lisp:974`, `:982`), so it
/// would be read as a Chaosnet address (`PROTOCOLS.md`, HOSTAB). No answer
/// for any name of the site carries it, and [`define_host`], which fails
/// on anything it would read as an address but `CHAOS`, takes them all.
#[test]
fn never_a_machine_type() {
    let config = Config::parse(SITE).unwrap();
    let mut site = Asking::new(SITE);
    for asked in config.names.iter().chain(config.hosts.iter().flat_map(|h| &h.names)) {
        let answer = site.ask(asked);
        let text = lispm::from_bytes(&answer);
        assert!(!text.split(NEWLINE as char).any(|l| l.starts_with("MACHINE-TYPE")), "{text:?}");
        assert!(define_host(&answer).is_ok(), "asked {asked}");
    }
}

/// **An unknown name is `ERROR No such host`, then an EOF** --- the one
/// error the manual expects, "no such host" (`sys/man/chaos.text`, §Host
/// Table) --- and the connection stays open for the next name. A name is
/// matched whole: a part of one, or an empty line, is no host's.
#[test]
fn an_unknown_name_is_an_error_then_an_eof() {
    let mut site = Asking::new(SITE);
    for asked in ["NO-SUCH-HOST", "LM", "MIT-LISPM", ""] {
        let answer = site.ask(asked);
        assert_eq!(answer, lines(&["ERROR No such host"]), "asked {asked:?}");
        assert_eq!(define_host(&answer), Err("No such host".to_string()));
        assert_eq!(site.connections(), (1, 1), "and the connection is still open");
    }
}

/// **Several names on one connection**, each a transaction ended by its own
/// EOF: the user "undertakes a number of transactions, then closes the
/// connection" (`sys/man/chaos.text`, §Host Table). The band's own user
/// end asks one name a connection; the protocol takes any number.
#[test]
fn several_names_on_one_connection() {
    let mut site = Asking::new(SITE);
    assert_eq!(define_host(&site.ask("OZ")).unwrap().name, "MIT-OZ");
    assert_eq!(define_host(&site.ask("LM1")).unwrap().name, "MIT-LISPM-1");
    assert_eq!(define_host(&site.ask("NOWHERE")), Err("No such host".to_string()));
    assert_eq!(define_host(&site.ask("lm2")).unwrap().name, "MIT-LISPM-2");
    assert_eq!(site.connections(), (1, 1), "one connection throughout");
}

/// **A line is a name whatever the packets.** A stream carries bytes, so
/// half a name in one packet waits for the rest, and two names in one
/// packet are two transactions, answered in order, each with its EOF.
#[test]
fn a_line_is_a_name_whatever_the_packets() {
    let mut site = Asking::new(SITE);
    site.user.send(Out::Data(b"LM".to_vec()));
    site.shuttle();
    assert!(site.user.take().is_empty(), "half a name is not a name yet");
    site.user.send(Out::Data([&b"2"[..], &[NEWLINE]].concat()));
    site.shuttle();
    assert_eq!(define_host(&site.user.answer()).unwrap().name, "MIT-LISPM-2");
    site.user.send(Out::Data([&b"OZ"[..], &[NEWLINE], b"LM1", &[NEWLINE]].concat()));
    site.shuttle();
    let names: Vec<String> =
        site.user.answers().iter().map(|a| define_host(&a.concat()).unwrap().name).collect();
    assert_eq!(names, ["MIT-OZ", "MIT-LISPM-1"]);
}

/// **A line longer than any name is no host's**, however long: the service
/// keeps no more of a line than can still be a name, and a line that
/// begins with one and runs on is not that name.
#[test]
fn a_line_longer_than_any_name_is_no_hosts() {
    let mut site = Asking::new(SITE);
    let mut long = lispm::lispm_text(&format!("MIT-LISPM-1{}", "X".repeat(3 * MAX_DATA)));
    long.push(NEWLINE);
    for chunk in long.chunks(MAX_DATA) {
        site.user.send(Out::Data(chunk.to_vec()));
    }
    site.shuttle();
    assert_eq!(site.user.answer(), lines(&["ERROR No such host"]));
    assert_eq!(define_host(&site.ask("LM1")).unwrap().name, "MIT-LISPM-1", "and the next line");
}

/// **A long answer is cut into packets.** A host may have more names than
/// one packet's data holds, 488 bytes (AIM-628 §3.5); the answer goes out
/// in as many packets as it takes, each within that, and `:LINE-IN` reads a
/// line across two of them (`sys/io/stream.lisp:535`).
#[test]
fn a_long_answer_is_cut_into_packets() {
    let names: Vec<String> = (0..30).map(|i| format!("A-HOST-WITH-A-LONG-NAME-{i:02}")).collect();
    let text = format!("{SITE}host 3052 {} system=LISPM\n", names.join(" "));
    let mut site = Asking::new(&text);
    site.user.line_out("a-host-with-a-long-name-17");
    site.shuttle();
    let answers = site.user.answers();
    let [packets] = answers.as_slice() else { panic!("one answer, not {}", answers.len()) };
    assert!(packets.len() > 1, "in more than one packet");
    assert!(packets.iter().all(|p| p.len() <= MAX_DATA), "each within a packet's data");
    let host = define_host(&packets.concat()).unwrap();
    assert_eq!(host.name, names[0]);
    assert_eq!(host.host_names.len(), names.len());
    assert_eq!(host.chaos, [0o3052]);
}

/// **The client closes, and the connection ends at both ends.** The user
/// end's stream is closed on the way out of `WITH-OPEN-STREAM`
/// (`chuse.lisp:953`): an EOF, waited on until it is receipted, then a CLS
/// with no reason (`BASIC-OUTPUT-STREAM :EOF` and `:BEFORE :CLOSE`,
/// `BASIC-STREAM :CLOSE`, `chuse.lisp`). The service sends nothing for the
/// EOF, and the CLS frees it. A client that aborts sends the CLS alone.
#[test]
fn the_client_closing_ends_the_connection() {
    let mut site = Asking::new(SITE);
    site.ask("LM1");
    site.user.send(Out::Eof);
    site.user.send(Out::Close(String::new()));
    site.shuttle();
    assert_eq!(site.connections(), (0, 0), "closed at both ends");
    assert!(site.user.take().is_empty(), "and nothing came back for the EOF");
    let mut site = Asking::new(SITE);
    site.user.send(Out::Close("Aborted".into()));
    site.shuttle();
    assert_eq!(site.connections(), (0, 0), "aborted");
}
