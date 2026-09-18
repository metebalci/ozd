// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MINI, driven as a cold load drives it (`docs/design.md` §11): a
//! scripted machine end that lays out `cold/mini.lisp`'s packets itself,
//! without an NCP, since MINI has none. Its RFC comes from index 1 as
//! packet 1; each open is numbered one past the last packet it sent; it
//! receipts every packet it takes with an STS offering a window of one;
//! and it passes over anything out of sequence with an STS
//! (`MINI-NEXT-PKT`). The server is an NCP holding the service, and the
//! test carries every packet at a clock it sets, as `tests/ncp.rs` drives
//! one.
//!
//! Each test's world is a directory under `CARGO_TARGET_TMPDIR`: the root,
//! and beside it what must never be reached.

mod support;

use ozd::lispm::{self, NEWLINE};
use ozd::ncp::{Ncp, op};
use ozd::packet::{MAX_DATA, Packet};
use ozd::roots::{Root, Tree};
use ozd::service::file::{BINARY_OP, CHARACTER_OP, QFASL_MAGIC};
use ozd::service::mini::{self, Mini};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use support::arriving;

/// This host: System 100's `MIT-OZ`, where its `cold/mini.lisp` sends.
const OZ: u16 = 0o3060;
/// A Lisp Machine booting a cold load.
const LM1: u16 = 0o3050;
/// The machine's index. `MINI-LOCAL-INDEX` is unbound at each boot, so the
/// first connection's is 1 (`MINI-OPEN-CONNECTION`, `cold/mini.lisp:72`).
const INDEX: u16 = 1;
/// 2001-09-09 01:46:40 UTC, a date to pin a file's to.
const DATED: u64 = 1_000_000_000;

/// A test's directory, made empty: `root`, which is served, and `outside`
/// beside it.
fn world(name: &str) -> PathBuf {
    let dir = support::scratch().join(format!("mini-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("root")).unwrap();
    std::fs::create_dir_all(dir.join("outside")).unwrap();
    dir
}

/// `contents` at `path` under `root`, its directories made, modified at
/// [`DATED`].
fn put(root: &Path, path: &str, contents: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, contents).unwrap();
    let f = std::fs::File::options().write(true).open(&file).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(DATED)).unwrap();
}

/// MINI over `root` alone, read-only, logging to `log` if given.
fn serving(root: &Path, log: Option<ozd::log::Hook>) -> Mini {
    let tree = Tree::new(vec![Root { name: None, path: root.to_path_buf(), readonly: true }]);
    Mini::new(Arc::new(tree.unwrap()), log)
}

/// A compiled file: the magic, then `words` more words.
fn qfasl(words: u32) -> Vec<u8> {
    let mut bytes = QFASL_MAGIC.to_vec();
    bytes.extend((0..words * 2).map(|i| (i * 7) as u8));
    bytes
}

/// The machine end, its state named as `cold/mini.lisp` names it.
struct Machine {
    oz: Ncp,
    now: u64,
    /// `MINI-REMOTE-INDEX`: ozd's index, from its OPN.
    far: u16,
    /// `MINI-OUT-PKT-NUMBER`: the number of the next open.
    out: u16,
    /// `MINI-IN-PKT-NUMBER`: the last packet taken.
    taken: u16,
}

impl Machine {
    /// A connection to MINI at a host serving `service`, opened as
    /// `MINI-OPEN-CONNECTION` opens it: the RFC as packet 1, `MINI LISPM `
    /// in it --- the contact, a user and an empty password --- then the
    /// OPN, and an STS for it.
    fn connect(service: Mini) -> Machine {
        let mut oz = Ncp::new(OZ);
        oz.serve(Box::new(service));
        let mut m = Machine { oz, now: 0, far: 0, out: 1, taken: 0 };
        m.send(op::RFC, b"MINI LISPM ".to_vec());
        let opn = m.oz_next().expect("an answer to the RFC");
        assert_eq!(opn.opcode, op::OPN, "the RFC is accepted");
        m.far = opn.source_index;
        m.taken = opn.number;
        m.out += 1;
        m.sts();
        m
    }

    /// `MINI-SEND-PKT`: a packet numbered `MINI-OUT-PKT-NUMBER`,
    /// acknowledging the last packet taken, a millisecond after the last.
    fn send(&mut self, opcode: u8, data: Vec<u8>) {
        let p = Packet {
            opcode,
            forward: 0,
            dest: OZ,
            dest_index: self.far,
            source: LM1,
            source_index: INDEX,
            number: self.out,
            ack: self.taken,
            data,
        };
        self.now += 1_000_000;
        self.oz.receive(self.now, &arriving(&p));
    }

    /// `MINI-SEND-STS`: a receipt of the last packet taken, and a window of
    /// one.
    fn sts(&mut self) {
        let mut data = self.taken.to_le_bytes().to_vec();
        data.extend_from_slice(&1u16.to_le_bytes());
        self.send(op::STS, data);
    }

    /// What ozd sends next, whatever it is.
    fn oz_next(&mut self) -> Option<Packet> {
        self.oz.transmit(self.now).map(|b| Packet::from_buffer(&b).unwrap().0)
    }

    /// `MINI-NEXT-PKT`: the next packet the machine takes --- an EOF, a
    /// win, a lose or data numbered one past the last taken, or an OPN ---
    /// passing over each other one with an STS. A CLS or a LOS is
    /// "Connection broken".
    fn next(&mut self) -> Packet {
        loop {
            let p = self.oz_next().expect("ozd has a packet for the machine");
            assert_eq!((p.dest, p.dest_index), (LM1, INDEX), "for the machine's connection");
            assert!(
                !matches!(p.opcode, op::CLS | op::LOS),
                "Connection broken: {}",
                lispm::from_bytes(&p.data)
            );
            let kinds = [op::EOF, mini::WIN, mini::LOSE, CHARACTER_OP, BINARY_OP];
            let in_sequence = p.number == self.taken.wrapping_add(1);
            if p.opcode == op::OPN || (kinds.contains(&p.opcode) && in_sequence) {
                return p;
            }
            self.sts();
        }
    }

    /// `MINI-OPEN-FILE`: an open for `pathname`, and ozd's reply, receipted.
    /// The reply's opcode, and its text split at the first newline as the
    /// machine splits it.
    fn open(&mut self, pathname: &str, binary: bool) -> (u8, String, String) {
        let opcode = if binary { mini::BINARY_OPEN } else { mini::CHARACTER_OPEN };
        self.send(opcode, pathname.as_bytes().to_vec());
        let reply = self.next();
        assert!(
            matches!(reply.opcode, mini::WIN | mini::LOSE),
            "a win or a lose, not {:o}",
            reply.opcode
        );
        self.taken = reply.number;
        self.out = self.out.wrapping_add(1);
        self.sts();
        let newline = reply.data.iter().position(|&b| b == NEWLINE);
        let cr = newline.expect("a newline in the reply, or the cold load's (1+ NIL) signals");
        let (before, after) = (&reply.data[..cr], &reply.data[cr + 1..]);
        (reply.opcode, lispm::from_bytes(before), lispm::from_bytes(after))
    }

    /// `MINI-REPORT`: a line for the server's log, the message its data and
    /// no newline (**(LMZ)** `cold/mini.lisp:390`). ozd's reply is a lose,
    /// so that no file follows, and it is receipted as an open's reply is.
    /// The machine takes any answer as the packet having arrived and reads
    /// none of it (`:390`-`:406`); the split here is the test's, holding
    /// ozd's answer to the shape an open's reply has.
    fn report(&mut self, message: &str) -> (u8, String, String) {
        self.send(mini::REPORT, lispm::lispm_text(message));
        let reply = self.next();
        self.taken = reply.number;
        self.out = self.out.wrapping_add(1);
        self.sts();
        let newline = reply.data.iter().position(|&b| b == NEWLINE);
        let cr = newline.expect("a newline in the reply, where the machine splits it");
        let (before, after) = (&reply.data[..cr], &reply.data[cr + 1..]);
        (reply.opcode, lispm::from_bytes(before), lispm::from_bytes(after))
    }

    /// The file, read to its EOF as `MINI-BINARY-STREAM` and
    /// `MINI-ASCII-STREAM` read it (`MINI-CLOSE`): an STS before each packet
    /// waited for, and one for the EOF. Each packet's opcode and data.
    fn read(&mut self) -> Vec<(u8, Vec<u8>)> {
        let mut packets = Vec::new();
        loop {
            self.sts();
            let p = self.next();
            self.taken = p.number;
            if p.opcode == op::EOF {
                self.sts();
                return packets;
            }
            packets.push((p.opcode, p.data));
        }
    }
}

/// Every packet's data, one after another.
fn joined(packets: &[(u8, Vec<u8>)]) -> Vec<u8> {
    packets.iter().flat_map(|(_, d)| d.iter().copied()).collect()
}

/// **A compiled file comes as its truename, its date and its words.** An
/// open for binary, `201`, is won with a `202` whose text is the truename,
/// the machine's newline and the file's date --- `MM/DD/YY HH:MM:SS`, as
/// `cold/minisr.mid` writes it and FILE dates a file --- and then the file
/// goes in `300` packets of whole 16-bit words, low byte first, then EOF.
/// `MINI-FASLOAD` reads the words (`MINI-BINARY-STREAM`), and the first
/// four bytes must be the magic that `FASLOAD` checks.
#[test]
fn a_compiled_file_is_its_truename_its_date_and_its_words() {
    let dir = world("compiled");
    let root = dir.join("root");
    let file = qfasl(600);
    put(&root, "sys/sys2/defsel.qfasl", &file);
    let mut m = Machine::connect(serving(&root, None));
    let (opcode, truename, date) = m.open("/sys/sys2/defsel.qfasl", true);
    assert_eq!(opcode, mini::WIN);
    assert_eq!(truename, "/sys/sys2/defsel.qfasl");
    assert_eq!(date, "09/09/01 01:46:40", "the modification date, in UTC");
    let packets = m.read();
    assert_eq!(packets.len(), file.len().div_ceil(MAX_DATA), "full packets, but the last");
    for (opcode, data) in &packets {
        assert_eq!(*opcode, BINARY_OP, "binary data");
        assert!(data.len() <= MAX_DATA && data.len() % 2 == 0, "whole words in each packet");
    }
    let words = joined(&packets);
    assert_eq!(words[..4], QFASL_MAGIC);
    assert_eq!(words, file, "the file byte for byte");
}

/// **A text file comes in the machine's character set**: an open for
/// characters, `200`, is won with a `202` and then the file goes in `200`
/// packets, translated as FILE translates characters --- Unix's newline to
/// the machine's `215`, and the format effectors up into the `200`s ---
/// then EOF. `MINI-READFILE` reads it with the reader.
#[test]
fn a_text_file_comes_in_the_machines_character_set() {
    let dir = world("text");
    let root = dir.join("root");
    let text = b"(defvar *x* 1)\n\t; a tab, a page\x0c and a return\r\n";
    put(&root, "site/sys.translations", text);
    let mut m = Machine::connect(serving(&root, None));
    let (opcode, truename, _) = m.open("/site/sys.translations", false);
    assert_eq!((opcode, truename.as_str()), (mini::WIN, "/site/sys.translations"));
    let packets = m.read();
    assert!(packets.iter().all(|(opcode, _)| *opcode == CHARACTER_OP), "character data");
    let got = joined(&packets);
    assert_eq!(got, lispm::to_lispm(text));
    assert_eq!(got.iter().filter(|&&b| b == NEWLINE).count(), 2, "each newline the machine's");
}

/// **One connection carries one file after another.** The machine never
/// closes its connection: after a file's EOF it sends the next open on the
/// same one, numbered on from the last (`MINI-OPEN-FILE`), text and binary
/// in any order, and a text longer than a packet goes in several.
#[test]
fn one_connection_carries_one_file_after_another() {
    let dir = world("several");
    let root = dir.join("root");
    let long = "three\n".repeat(200);
    put(&root, "one.text", b"one\n");
    put(&root, "two.qfasl", &qfasl(10));
    put(&root, "three.text", long.as_bytes());
    let mut m = Machine::connect(serving(&root, None));
    assert_eq!(m.open("/one.text", false).0, mini::WIN);
    assert_eq!(joined(&m.read()), lispm::to_lispm(b"one\n"));
    assert_eq!(m.open("/two.qfasl", true).0, mini::WIN);
    assert_eq!(joined(&m.read()), qfasl(10));
    assert_eq!(m.open("/three.text", false).0, mini::WIN);
    let packets = m.read();
    assert_eq!(packets.len(), 3, "1,200 bytes in three packets");
    assert_eq!(joined(&packets), lispm::to_lispm(long.as_bytes()));
    assert_eq!(m.oz.connections(), 1, "all on the one connection");
}

/// **What cannot be read is a lose, and the connection stays.** A missing
/// file, a pathname that leaves the root by `..` or by a link, a directory,
/// `/` itself and a FIFO are each answered with a `203` whose message is
/// followed by the machine's newline, as `cold/minisr.mid` answers, since
/// the machine splits the reply at it; and then nothing, no data and no
/// EOF. Nothing outside the root is read, and the FIFO is not opened, which
/// would hold the loop's one thread. The connection still serves the next
/// open.
#[test]
fn what_cannot_be_read_is_a_lose_and_the_connection_stays() {
    let dir = world("lose");
    let root = dir.join("root");
    put(&root, "sys/real.text", b"real\n");
    std::fs::create_dir_all(root.join("sys/dir")).unwrap();
    std::fs::write(dir.join("outside/secret.text"), b"secret\n").unwrap();
    std::os::unix::fs::symlink(dir.join("outside"), root.join("out")).unwrap();
    let made = std::process::Command::new("mkfifo").arg(root.join("sys/fifo")).status();
    assert!(made.unwrap().success(), "mkfifo");
    let mut m = Machine::connect(serving(&root, None));
    let refused = [
        "/sys/missing.text",
        "/../outside/secret.text",
        "/sys/../../outside/secret.text",
        "/out/secret.text",
        "/sys/dir",
        "/",
        "/sys/fifo",
    ];
    for pathname in refused {
        for binary in [false, true] {
            let (opcode, message, rest) = m.open(pathname, binary);
            assert_eq!(opcode, mini::LOSE, "{pathname} is refused");
            assert!(!message.is_empty() && rest.is_empty(), "{pathname}: a message, a newline");
            assert_eq!(m.oz_next(), None, "{pathname}: nothing after the lose");
        }
    }
    assert_eq!(m.open("/sys/real.text", false).0, mini::WIN, "the connection still serves");
    assert_eq!(m.read(), [(CHARACTER_OP, lispm::to_lispm(b"real\n"))]);
}

/// **An open sent again gets one reply.** `MINI-OPEN-FILE` sends its open
/// again, with the same packet number, when no reply comes within a couple
/// of seconds. ozd has taken the first, so the second is a duplicate: it
/// draws an STS carrying the receipt, and not a second win, and the file
/// goes once.
#[test]
fn an_open_sent_again_gets_one_reply() {
    let dir = world("again");
    let root = dir.join("root");
    put(&root, "one.text", b"one\n");
    let mut m = Machine::connect(serving(&root, None));
    m.send(mini::CHARACTER_OPEN, b"/one.text".to_vec());
    let win = m.oz_next().expect("the reply");
    assert_eq!(win.opcode, mini::WIN);
    m.send(mini::CHARACTER_OPEN, b"/one.text".to_vec());
    let again = m.oz_next().expect("an answer to the open sent again");
    assert_eq!(again.opcode, op::STS, "a receipt, not a second win");
    assert_eq!(m.oz_next(), None, "and nothing more until the win is receipted");
    m.taken = win.number;
    m.out += 1;
    m.sts();
    assert_eq!(m.read(), [(CHARACTER_OP, lispm::to_lispm(b"one\n"))], "the file once");
}

/// **`--log-mini` writes a line for each open**, in the shape of a
/// connection's lines: the contact, the machine, and then `read` and the
/// pathname for a file sent, or `refused`, the pathname and the message for
/// a lose --- which is where a cold load stops. Without `--log-mini` MINI
/// writes nothing but a report.
#[test]
fn log_mini_writes_a_line_for_each_open() {
    let dir = world("log");
    let root = dir.join("root");
    put(&root, "one.text", b"one\n");
    for log_mini in [true, false] {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let kept = lines.clone();
        let log: ozd::log::Hook =
            Arc::new(move |line: &str| kept.lock().unwrap().push(line.into()));
        let mut service = serving(&root, Some(log));
        service.log_mini = log_mini;
        let mut m = Machine::connect(service);
        m.open("/one.text", false);
        m.read();
        m.open("/missing.text", true);
        let lines = lines.lock().unwrap();
        if log_mini {
            assert_eq!(
                *lines,
                [
                    "MINI from 3050 (?) read /one.text",
                    "MINI from 3050 (?) refused /missing.text: File not found",
                ]
            );
        } else {
            assert!(lines.is_empty(), "nothing without --log-mini: {lines:?}");
        }
    }
}

/// **A report is a line for the log whatever the flags say.** A cold load
/// that runs a script through MINI has no other way to say how far it got,
/// and it sends the line as `204` (`docs/protocols.md`, MINI). ozd writes
/// it whether or not `--log-mini` is set: it is the one thing the machine
/// chose to send, where an open is one of a couple of hundred. It is
/// answered with a `203`, a message and the machine's newline, so that the
/// machine knows it arrived and no file follows. A log line is one line, so
/// the machine's own newline in a report --- and every other control
/// character --- is written as a space. The connection then serves the next
/// open.
#[test]
fn a_report_is_a_line_for_the_log_whatever_the_flags_say() {
    let dir = world("report");
    let root = dir.join("root");
    put(&root, "one.text", b"one\n");
    for log_mini in [true, false] {
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let kept = lines.clone();
        let log: ozd::log::Hook =
            Arc::new(move |line: &str| kept.lock().unwrap().push(line.into()));
        let mut service = serving(&root, Some(log));
        service.log_mini = log_mini;
        let mut m = Machine::connect(service);
        let (opcode, message, rest) = m.report("COLDRUN: form-3");
        assert_eq!(opcode, mini::LOSE, "a lose, so that no file follows");
        assert_eq!((message.as_str(), rest.as_str()), ("noted", ""), "a message, then the newline");
        assert_eq!(m.oz_next(), None, "nothing after the lose");
        let newline = char::from(NEWLINE);
        m.report(&format!("two{newline}lines"));
        assert_eq!(
            *lines.lock().unwrap(),
            ["MINI from 3050 (?) report: COLDRUN: form-3", "MINI from 3050 (?) report: two lines",],
            "a line each, whatever --log-mini says"
        );
        assert_eq!(m.open("/one.text", false).0, mini::WIN, "the connection still serves");
        assert_eq!(m.read(), [(CHARACTER_OP, lispm::to_lispm(b"one\n"))]);
    }
}
