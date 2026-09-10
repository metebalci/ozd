// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FILE, driven as a band drives it, through two real NCPs (`CLAUDE.md`
//! §10, items 5 and 6; `DESIGN.md` §11, item 7): a client NCP opens the
//! control connection to `FILE 1` at a server NCP that holds the service,
//! serves the contact names of the data connections the server calls back,
//! and speaks the user end's half of `sys/doc/chfile.text` as
//! `sys/network/chaos/qfile.lisp` does. The test carries every packet
//! between them at a clock it sets, as `tests/ncp.rs` drives two NCPs.
//!
//! First the protocol's ordinary exchanges, against the tree
//! (`src/roots.rs`), with no allowlist, since FILE serves whoever reaches
//! it (`CLAUDE.md` §2). A write as a band makes it --- the SYNC mark on the
//! data connection and the CLOSE on the control, the rename waiting for the
//! mark --- is `a_read_and_a_write_in_the_base`. Then what the tree changes
//! (`DESIGN.md` §6): a read-only mount, listings that never describe a file
//! outside a root, DELETE and RENAME of a link itself, a write cut off, two
//! machines at once, and the containment table through FILE's own commands.
//!
//! Each test's world is one directory under `std::env::temp_dir()`, holding
//! the roots and, beside them, what must never be reached; it is removed
//! when the test ends.

use ozd::lispm::{self, NEWLINE};
use ozd::ncp::{Ncp, Out, Response, Service, Session, op};
use ozd::packet::{self, Framed, Packet};
use ozd::roots::{Root, Tree, is_temporary, temporary_name};
use ozd::service::file::{self, File, LogHook};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

/// This host, the associated machine: System 100's `MIT-OZ`.
const SERVER: u16 = 0o3060;
/// A Lisp Machine, System 100's `MIT-LISPM-1`.
const LM1: u16 = 0o3050;
/// A second machine at the same site.
const LM2: u16 = 0o3051;

/// A packet as the link would hand it to the NCP, with the check word the
/// CADR's hardware would have made; `tests/ncp.rs` has the same.
fn arriving(p: &Packet) -> Framed {
    let buffer = p.to_buffer(p.dest);
    let mut over = buffer.clone();
    over.push(p.source);
    let check = packet::check_word(&over);
    Framed { buffer, source: p.source, check, check_ok: true }
}

fn text(bytes: &[u8]) -> String {
    lispm::from_bytes(bytes)
}

/// One control connection's user end, shared between the test and the
/// sessions its NCP holds.
#[derive(Default)]
struct Wire {
    /// Whether the control connection has opened.
    opened: bool,
    /// What came down the control connection, a packet each, as text.
    replies: Vec<String>,
    /// What came down its data connections: the opcode and the bytes, an
    /// EOF as [`op::EOF`].
    down: Vec<(u8, Vec<u8>)>,
    /// What is to go up the control connection.
    control: Vec<Out>,
    /// What is to go up each data connection, in the order they opened.
    data: Vec<Arc<Mutex<Vec<Out>>>>,
}

/// The user end of a control connection.
struct ControlEnd(Arc<Mutex<Wire>>);

impl Session for ControlEnd {
    fn opened(&mut self, _now: u64) {
        self.0.lock().unwrap().opened = true;
    }
    fn data(&mut self, _now: u64, _opcode: u8, bytes: &[u8]) {
        self.0.lock().unwrap().replies.push(text(bytes));
    }
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.0.lock().unwrap().control)
    }
}

/// The user end listening for a data connection, at the contact name its
/// output file handle names: "Hence, you should listen for such a
/// connection" (`sys/doc/chfile.text:148`, `DATA-CONNECTION`).
struct Listener {
    contact: String,
    wire: Arc<Mutex<Wire>>,
}

impl Service for Listener {
    fn contact(&self) -> &str {
        &self.contact
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        let up = Arc::new(Mutex::new(Vec::new()));
        self.wire.lock().unwrap().data.push(up.clone());
        Response::Accept(Box::new(DataEnd { wire: self.wire.clone(), up }))
    }
}

/// The user end of a data connection.
struct DataEnd {
    wire: Arc<Mutex<Wire>>,
    up: Arc<Mutex<Vec<Out>>>,
}

impl Session for DataEnd {
    fn data(&mut self, _now: u64, opcode: u8, bytes: &[u8]) {
        self.wire.lock().unwrap().down.push((opcode, bytes.to_vec()));
    }
    fn eof(&mut self, _now: u64) {
        self.wire.lock().unwrap().down.push((op::EOF, Vec::new()));
    }
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut *self.up.lock().unwrap())
    }
}

/// This host with FILE, and the machines that call it, each an NCP at its
/// own address; every packet carried by [`Net::settle`] as a cable would.
struct Net {
    server: Ncp,
    hosts: Vec<Ncp>,
    /// Each client: its host, by its place in `hosts`, and its control
    /// connection's user end. One host may hold more than one.
    clients: Vec<(usize, Arc<Mutex<Wire>>)>,
}

impl Net {
    fn new(service: File) -> Net {
        let mut server = Ncp::new(SERVER);
        server.serve(Box::new(service));
        Net { server, hosts: Vec::new(), clients: Vec::new() }
    }

    /// A control connection to FILE from the host at `address`, open: the
    /// RFC carries `FILE 1`, as the band's does, the `1` the version.
    fn client(&mut self, address: u16, now: u64) -> usize {
        let h = match self.hosts.iter().position(|h| h.address() == address) {
            Some(h) => h,
            None => {
                self.hosts.push(Ncp::new(address));
                self.hosts.len() - 1
            }
        };
        let wire = Arc::new(Mutex::new(Wire::default()));
        self.hosts[h].connect(now, SERVER, "FILE 1", Box::new(ControlEnd(wire.clone())));
        self.clients.push((h, wire));
        self.settle(now);
        let c = self.clients.len() - 1;
        assert!(self.wire(c).opened, "the control connection opened");
        c
    }

    fn wire(&self, c: usize) -> MutexGuard<'_, Wire> {
        self.clients[c].1.lock().unwrap()
    }

    /// Carries packets between every host until none has anything more to
    /// send at `now`.
    fn settle(&mut self, now: u64) {
        for _ in 0..10_000 {
            let mut moving = Vec::new();
            while let Some(b) = self.server.transmit(now) {
                moving.push(b);
            }
            for h in &mut self.hosts {
                while let Some(b) = h.transmit(now) {
                    moving.push(b);
                }
            }
            if moving.is_empty() {
                return;
            }
            for b in moving {
                self.deliver(now, &Packet::from_buffer(&b).unwrap().0);
            }
        }
        panic!("the NCPs never fell quiet");
    }

    fn deliver(&mut self, now: u64, p: &Packet) {
        if p.dest == SERVER {
            return self.server.receive(now, &arriving(p));
        }
        let Some(h) = self.hosts.iter_mut().find(|h| h.address() == p.dest) else {
            panic!("a packet for nobody: {p:?}");
        };
        h.receive(now, &arriving(p));
    }

    /// The client's host listens for the data connection the server will
    /// call on `contact`, the output handle's name.
    fn listen(&mut self, c: usize, contact: &str) {
        let (h, wire) = (self.clients[c].0, self.clients[c].1.clone());
        self.hosts[h].serve(Box::new(Listener { contact: contact.into(), wire }));
    }

    /// Sends a command on the control connection and returns the reply to
    /// its transaction.
    fn command(&mut self, c: usize, now: u64, cmd: &str) -> String {
        let tid = cmd.split(' ').next().unwrap().to_string();
        self.send_command(c, now, cmd);
        self.take_reply(c, &tid)
            .unwrap_or_else(|| panic!("no reply to {tid}: replies {:?}", self.wire(c).replies))
    }

    /// [`Net::command`] for a command that need not be answered yet: the
    /// reply, if one has come, is left to be taken later.
    fn send_command(&mut self, c: usize, now: u64, cmd: &str) {
        self.wire(c).control.push(Out::Data(lispm::lispm_text(cmd)));
        self.settle(now);
    }

    /// The reply to `tid`, taken off the pile if it has come.
    fn take_reply(&mut self, c: usize, tid: &str) -> Option<String> {
        let mut w = self.wire(c);
        let i = w.replies.iter().position(|r| r.starts_with(&format!("{tid} ")))?;
        Some(w.replies.remove(i))
    }

    /// Sends a data packet up the latest data connection, as a user end
    /// writing a file does.
    fn send_data(&mut self, c: usize, now: u64, opcode: u8, bytes: &[u8]) {
        self.queue_data(c, opcode, bytes);
        self.settle(now);
    }

    /// A data packet for the latest data connection, not yet sent.
    fn queue_data(&mut self, c: usize, opcode: u8, bytes: &[u8]) {
        let up = self.wire(c).data.last().expect("a data connection").clone();
        up.lock().unwrap().push(Out::DataOp(opcode, bytes.to_vec()));
    }

    /// What has come down the client's data connections since last taken.
    fn down(&mut self, c: usize) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.wire(c).down)
    }
}

/// FILE over these roots, reporting nothing, at [`SERVER`].
fn serve(roots: Vec<Root>) -> Net {
    Net::new(File::new(Arc::new(Tree::new(roots).unwrap()), None))
}

/// FILE over these roots, with the tree it shares and what it reports as
/// changed, a line each.
fn serve_logged(roots: Vec<Root>) -> (Net, Arc<Tree>, Arc<Mutex<Vec<String>>>) {
    let tree = Arc::new(Tree::new(roots).unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let hook: LogHook = Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()));
    (Net::new(File::new(tree.clone(), Some(hook))), tree, log)
}

/// A client at `address` as a band's first file operation leaves it:
/// logged in as `user`, with a data connection of these two handles open.
fn ready(n: &mut Net, address: u16, user: &str, (input, output): (&str, &str), now: u64) -> usize {
    let c = n.client(address, now);
    let r = n.command(c, now, &format!("T1  LOGIN {user} {user} "));
    assert!(r.starts_with(&format!("T1  LOGIN {user} ")), "{r:?}");
    n.listen(c, output);
    let r = n.command(c, now, &format!("T2  DATA-CONNECTION {input} {output}"));
    assert_eq!(r, "T2  DATA-CONNECTION");
    c
}

fn opcodes(down: &[(u8, Vec<u8>)]) -> Vec<u8> {
    down.iter().map(|(o, _)| *o).collect()
}

/// What came down as character data, as text.
fn characters(down: &[(u8, Vec<u8>)]) -> String {
    down.iter().filter(|(o, _)| *o == file::CHARACTER_OP).map(|(_, d)| text(d)).collect()
}

/// The record for `pathname` in a listing: its lines, up to the blank line
/// that ends it.
#[track_caller]
fn record<'a>(listing: &'a str, pathname: &str) -> &'a str {
    let nl = NEWLINE as char;
    listing
        .split(&format!("{nl}{nl}"))
        .find(|r| r.starts_with(&format!("{pathname}{nl}")))
        .unwrap_or_else(|| panic!("{pathname} is not listed: {listing:?}"))
}

/// The FILE service's temporaries in `dir`.
fn temporaries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_temporary(n))
        .collect();
    v.sort();
    v
}

/// Sends `cmd` under a transaction id of its own, and asserts that it is
/// refused with `code` and that nothing in `world` changed.
#[track_caller]
fn refused(n: &mut Net, c: usize, now: &mut u64, world: &Path, code: &str, cmd: &str) {
    *now += 1;
    let before = snapshot(world);
    let tid = format!("R{now}");
    let r = n.command(c, *now, &format!("{tid} {cmd}"));
    assert!(
        r.starts_with(&format!("{tid} ")) && r.contains(&format!(" ERROR {code} ")),
        "{cmd:?}: wanted {code}, got {r:?}"
    );
    assert_eq!(snapshot(world), before, "{cmd:?}: something on disk changed");
    n.down(c);
}

/// Sends `cmd` under a transaction id of its own, and asserts that it is
/// not refused.
#[track_caller]
fn accepted(n: &mut Net, c: usize, now: &mut u64, cmd: &str) -> String {
    *now += 1;
    let tid = format!("A{now}");
    let r = n.command(c, *now, &format!("{tid} {cmd}"));
    assert!(r.starts_with(&format!("{tid} ")) && !r.contains(" ERROR "), "{cmd:?}: {r:?}");
    r
}

/// One test's world: a directory of its own under the system's temporary
/// directory, canonical so that paths compare with what the tree returns,
/// and removed when the test ends however it ends.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("ozd-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir: std::fs::canonicalize(&dir).unwrap() }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// A directory, with its parents.
    fn dir(&self, relative: &str) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// A file holding `contents`, its directory made first.
    fn file(&self, relative: &str, contents: &str) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, contents).unwrap();
        p
    }

    /// A symbolic link at `relative` whose content is `target`, as given.
    #[cfg(unix)]
    fn link(&self, relative: &str, target: impl AsRef<Path>) -> PathBuf {
        let p = self.path(relative);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &p).unwrap();
        p
    }

    /// A FIFO at `relative`, if `mkfifo` can make one.
    fn fifo(&self, relative: &str) -> bool {
        let made = std::process::Command::new("mkfifo").arg(self.path(relative)).status();
        made.is_ok_and(|m| m.success())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An entry on disk as [`snapshot`] records it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    File(Vec<u8>),
    Directory,
    Link(PathBuf),
    /// A FIFO or a socket: that it is there, never what it holds, since
    /// reading a FIFO would block.
    Other,
}

/// Everything under `dir`, links not followed: each entry by its path
/// relative to `dir`, with a file's bytes and a link's content;
/// `tests/containment.rs` has the same.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut seen = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let t = e.file_type().unwrap();
            let entry = if t.is_symlink() {
                Entry::Link(std::fs::read_link(&p).unwrap())
            } else if t.is_dir() {
                stack.push(p.clone());
                Entry::Directory
            } else if t.is_file() {
                Entry::File(std::fs::read(&p).unwrap())
            } else {
                Entry::Other
            };
            seen.insert(p.strip_prefix(dir).unwrap().to_path_buf(), entry);
        }
    }
    seen
}

fn base(path: &Path) -> Root {
    Root { name: None, path: path.to_path_buf(), readonly: false }
}

fn mount(name: &str, path: &Path) -> Root {
    Root { name: Some(name.to_string()), path: path.to_path_buf(), readonly: false }
}

fn readonly(root: Root) -> Root {
    Root { readonly: true, ..root }
}

/// **The FILE protocol serves a file and a directory, read-only**, as the
/// band's client `qfile.lisp` speaks it and the Unix server `FILE.c`
/// answered it: login, a data connection the server calls, OPEN with the
/// properties line and the truename, the file down the data connection in
/// the Lisp Machine character set, EOF, CLOSE answered and then the
/// synchronous mark; DIRECTORY as records; PROBE of a missing file as an
/// FNF error; a compiled file recognised by its magic and sent binary.
#[test]
fn the_file_service_serves_files_and_directories() {
    let s = Scratch::new("serves");
    let root = s.dir("base");
    let sys = s.dir("base/tree/sys");
    s.dir("base/tree/sys/sub");
    std::fs::write(sys.join("hello.lisp"), "hello\nworld\n").unwrap();
    let mut qfasl = file::QFASL_MAGIC.to_vec();
    qfasl.extend_from_slice(&[1, 2, 3, 4, 5]);
    std::fs::write(sys.join("x.qfasl"), &qfasl).unwrap();

    let mut n = serve(vec![base(&root)]);
    let c = n.client(LM1, 0);
    let nl = NEWLINE as char;

    // LOGIN: the user, a home directory, a personal name.
    let r = n.command(c, 10, "T1  LOGIN LISPM LISPM ");
    assert_eq!(r, format!("T1  LOGIN LISPM /lispm/{nl}LISPM{nl}"));

    // DATA-CONNECTION: the server calls our output handle's name.
    n.listen(c, "O0001");
    let r = n.command(c, 20, "T2  DATA-CONNECTION I0001 O0001");
    assert_eq!(r, "T2  DATA-CONNECTION");
    assert_eq!(n.wire(c).data.len(), 1, "the data connection is open");

    // OPEN READ CHARACTER: properties, truename, then the file, then EOF.
    let r = n.command(c, 30, &format!("T3 I0001 OPEN READ CHARACTER{nl}/tree/sys/hello.lisp{nl}"));
    let head = "T3 I0001 OPEN ";
    assert!(r.starts_with(head), "{r:?}");
    let rest = r.strip_prefix(head).unwrap();
    let (props, tn) = rest.split_once(nl).unwrap();
    let fields: Vec<&str> = props.split(' ').collect();
    assert_eq!(fields.len(), 4, "date, time, length, QFASL: {props:?}");
    assert_eq!(fields[0].len(), 8, "MM/DD/YY");
    assert_eq!(fields[1].len(), 8, "HH:MM:SS");
    assert_eq!(fields[2], "12", "the length in bytes");
    assert_eq!(fields[3], "NIL", "not a compiled file");
    assert_eq!(tn, format!("/tree/sys/hello.lisp{nl}"));
    let down = n.down(c);
    assert_eq!(opcodes(&down), [file::CHARACTER_OP, op::EOF], "the data as characters, then EOF");
    assert_eq!(text(&down[0].1), format!("hello{nl}world{nl}"), "newlines the Lisp Machine's");

    // CLOSE: answered on the control connection, then the synchronous mark.
    let r = n.command(c, 40, "T4 I0001 CLOSE");
    assert!(r.starts_with("T4 I0001 CLOSE "), "{r:?}");
    assert_eq!(opcodes(&n.down(c)), [file::SYNC_MARK_OP]);

    // PROBE of a file that is not there.
    let r = n.command(c, 50, &format!("T5  OPEN PROBE CHARACTER{nl}/tree/sys/nope.lisp{nl}"));
    assert_eq!(r, "T5  ERROR FNF C File not found");

    // DIRECTORY: records down the data connection.
    let r = n.command(c, 60, &format!("T6 I0001 DIRECTORY{nl}/tree/sys/*{nl}"));
    assert_eq!(r, "T6 I0001 DIRECTORY");
    let down = n.down(c);
    assert!(down.iter().any(|(o, _)| *o == op::EOF), "EOF after the listing");
    let listing = characters(&down);
    let records: Vec<&str> = listing.split(&format!("{nl}{nl}")).collect();
    assert!(records[0].starts_with(nl), "the first record has no pathname: {:?}", records[0]);
    assert!(records[0].contains("BLOCK-SIZE 1024"));
    let hello = record(&listing, "/tree/sys/hello.lisp");
    assert!(hello.contains(&format!("{nl}LENGTH-IN-BYTES 12{nl}")), "{hello:?}");
    assert!(hello.contains(&format!("{nl}CREATION-DATE ")));
    let sub = record(&listing, "/tree/sys/sub");
    assert!(sub.ends_with(&format!("{nl}DIRECTORY T")), "{sub:?}");
    let r = n.command(c, 70, "T7 I0001 CLOSE");
    assert_eq!(r, "T7 I0001 CLOSE");
    n.down(c);

    // A compiled file, opened with DEFAULT: recognised, and sent binary.
    let r = n.command(c, 80, &format!("T8 I0001 OPEN READ DEFAULT{nl}/tree/sys/x.qfasl{nl}"));
    let props = r.strip_prefix("T8 I0001 OPEN ").unwrap().split(nl).next().unwrap();
    assert!(props.ends_with(" 5 T NIL"), "length in words, QFASL, not characters: {props:?}");
    let down = n.down(c);
    assert_eq!(opcodes(&down), [file::BINARY_OP, op::EOF]);
    assert_eq!(down[0].1, qfasl, "the bytes as they are");
}

/// **A date is the calendar's, in UTC**: `civil`, Howard Hinnant's
/// days-to-civil, which dates everything FILE describes. The character set
/// is tested in `tests/services.rs`, with `src/lispm.rs`.
#[test]
fn a_date_is_the_calendars_in_utc() {
    assert_eq!(file::civil(0), (1970, 1, 1, 0, 0, 0));
    assert_eq!(file::civil(1_788_480_000), (2026, 9, 4, 0, 0, 0));
    assert_eq!(file::civil(1_000_000_000), (2001, 9, 9, 1, 46, 40));
}

/// **The FILE protocol writes a file**, as `qfile.lisp`'s output stream
/// does it and `FILE.c` answered: `OPEN WRITE` on the output handle, the
/// data up the data connection, the user end's own synchronous mark, and
/// `CLOSE`, which renames the temporary into place and answers with the
/// file's date, its length and its truename. Until the close the real file
/// is untouched, which is what makes an abandoned write harmless.
#[test]
fn the_file_service_writes_files() {
    let s = Scratch::new("writes");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    std::fs::write(dir.join("old.lisp"), "was here\n").unwrap();

    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    // OPEN WRITE on the output handle, and nothing is there yet.
    let r = n.command(c, 30, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tree/sys/new.lisp{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    assert!(!dir.join("new.lisp").exists(), "the real file waits for the close");
    assert_eq!(temporaries(&dir).len(), 1, "a temporary is there instead");

    // The data, in the Lisp Machine character set, then the mark.
    n.send_data(c, 40, file::CHARACTER_OP, &[b'h', b'i', NEWLINE, b'!', NEWLINE]);
    n.send_data(c, 45, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 50, "T4 O0001 CLOSE");
    let head = "T4 O0001 CLOSE ";
    assert!(r.starts_with(head), "{r:?}");
    let rest = r.strip_prefix(head).unwrap();
    let (props, tn) = rest.split_once(nl).unwrap();
    let fields: Vec<&str> = props.split(' ').collect();
    assert_eq!(fields.len(), 3, "date, time, length: {props:?}");
    assert_eq!(fields[2], "5", "the length written");
    assert_eq!(tn, format!("/tree/sys/new.lisp{nl}"));
    assert_eq!(
        std::fs::read(dir.join("new.lisp")).unwrap(),
        b"hi\n!\n",
        "the newlines came back to Unix's"
    );
    assert!(temporaries(&dir).is_empty(), "and the temporary is gone");

    // Writing over a file that is there: refused with IF-EXISTS ERROR,
    // taken with SUPERSEDE.
    let r = n.command(
        c,
        60,
        &format!("T5 O0001 OPEN WRITE CHARACTER IF-EXISTS ERROR{nl}/tree/sys/old.lisp{nl}"),
    );
    assert_eq!(r, "T5 O0001 ERROR FAE C File already exists");
    let r = n.command(
        c,
        70,
        &format!("T6 O0001 OPEN WRITE CHARACTER IF-EXISTS SUPERSEDE{nl}/tree/sys/old.lisp{nl}"),
    );
    assert!(r.starts_with("T6 O0001 OPEN "), "{r:?}");
    n.send_data(c, 75, file::CHARACTER_OP, b"now this");
    n.send_data(c, 76, file::SYNC_MARK_OP, &[]);
    assert_eq!(
        std::fs::read_to_string(dir.join("old.lisp")).unwrap(),
        "was here\n",
        "still the old one"
    );
    n.command(c, 80, "T7 O0001 CLOSE");
    assert_eq!(std::fs::read_to_string(dir.join("old.lisp")).unwrap(), "now this");

    // A write abandoned: DELETE on the handle, and the file is as it was.
    let r = n.command(
        c,
        90,
        &format!("T8 O0001 OPEN WRITE CHARACTER IF-EXISTS SUPERSEDE{nl}/tree/sys/old.lisp{nl}"),
    );
    assert!(r.starts_with("T8 O0001 OPEN "));
    n.send_data(c, 95, file::CHARACTER_OP, b"rubbish");
    let r = n.command(c, 100, "T9 O0001 DELETE");
    assert_eq!(r, "T9 O0001 DELETE");
    assert_eq!(std::fs::read_to_string(dir.join("old.lisp")).unwrap(), "now this", "untouched");
    assert!(temporaries(&dir).is_empty(), "and its temporary gone");

    // A binary write goes through as it is.
    let r = n.command(c, 110, &format!("TA O0001 OPEN WRITE BINARY{nl}/tree/sys/b.qfasl{nl}"));
    assert!(r.starts_with("TA O0001 OPEN "));
    n.send_data(c, 115, file::BINARY_OP, &[0o215, 0o12, 0, 0o377]);
    n.send_data(c, 116, file::SYNC_MARK_OP, &[]);
    n.command(c, 120, "TB O0001 CLOSE");
    assert_eq!(
        std::fs::read(dir.join("b.qfasl")).unwrap(),
        [0o215, 0o12, 0, 0o377],
        "bytes as sent"
    );

    // A write where the file must exist and does not.
    let r = n.command(
        c,
        130,
        &format!("TC O0001 OPEN WRITE CHARACTER IF-DOES-NOT-EXIST ERROR{nl}/tree/sys/nope{nl}"),
    );
    assert_eq!(r, "TC O0001 ERROR FNF C File not found");
}

/// **The commands that change a directory**, each answered by name:
/// delete, rename, create a directory, create a link, change properties,
/// expunge, complete, and properties down a data connection. Each change to
/// the root is reported, with its pathname, through the hook the service
/// was given (`DESIGN.md` §10), and nothing that was refused is.
#[test]
fn the_file_service_manages_a_directory() {
    let s = Scratch::new("manage");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    std::fs::write(dir.join("one.lisp"), "1").unwrap();
    std::fs::write(dir.join("only.text"), "2").unwrap();

    let (mut n, _, log) = serve_logged(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    // CREATE-DIRECTORY, then again, which is an error.
    assert_eq!(
        n.command(c, 30, &format!("T3  CREATE-DIRECTORY{nl}/tree/sys/made{nl}")),
        "T3  CREATE-DIRECTORY"
    );
    assert!(dir.join("made").is_dir());
    assert_eq!(
        n.command(c, 31, &format!("T4  CREATE-DIRECTORY{nl}/tree/sys/made{nl}")),
        "T4  ERROR DAE C Directory already exists"
    );

    // RENAME, and over something that is there.
    assert_eq!(
        n.command(c, 40, &format!("T5  RENAME{nl}/tree/sys/one.lisp{nl}/tree/sys/two.lisp{nl}")),
        "T5  RENAME"
    );
    assert!(dir.join("two.lisp").exists() && !dir.join("one.lisp").exists());
    assert_eq!(
        n.command(c, 41, &format!("T6  RENAME{nl}/tree/sys/two.lisp{nl}/tree/sys/only.text{nl}")),
        "T6  ERROR REF C Rename to existing file"
    );
    assert_eq!(
        n.command(c, 42, &format!("T7  RENAME{nl}/tree/sys/gone{nl}/tree/sys/x{nl}")),
        "T7  ERROR FNF C File not found"
    );

    // DELETE with a pathname, and one that is not there.
    assert_eq!(n.command(c, 50, &format!("T8  DELETE{nl}/tree/sys/two.lisp{nl}")), "T8  DELETE");
    assert!(!dir.join("two.lisp").exists());
    assert_eq!(
        n.command(c, 51, &format!("T9  DELETE{nl}/tree/sys/two.lisp{nl}")),
        "T9  ERROR FNF C File not found"
    );

    // CREATE-LINK.
    assert_eq!(
        n.command(c, 60, &format!("TA  CREATE-LINK{nl}/tree/sys/link{nl}/tree/sys/only.text{nl}")),
        "TA  CREATE-LINK"
    );
    assert_eq!(std::fs::read_to_string(dir.join("link")).unwrap(), "2");

    // CHANGE-PROPERTIES: one it has, and one it has not.
    assert_eq!(
        n.command(
            c,
            70,
            &format!("TB  CHANGE-PROPERTIES{nl}/tree/sys/only.text{nl}AUTHOR LISPM{nl}")
        ),
        "TB  CHANGE-PROPERTIES"
    );
    let r = n.command(
        c,
        71,
        &format!("TC  CHANGE-PROPERTIES{nl}/tree/sys/only.text{nl}COLOUR BLUE{nl}"),
    );
    assert_eq!(r, "TC  ERROR UKP C COLOUR cannot be set here");

    // EXPUNGE: a number, and here always none.
    assert_eq!(n.command(c, 80, &format!("TD  EXPUNGE{nl}/tree/sys/{nl}")), "TD  EXPUNGE 0");

    // COMPLETE: one match completes, none is NIL.
    let r = n.command(c, 90, &format!("TE  COMPLETE{nl}/tree/sys/x{nl}/tree/sys/onl{nl}"));
    assert_eq!(r, format!("TE  COMPLETE OLD{nl}/tree/sys/only.text{nl}"));
    let r = n.command(c, 91, &format!("TF  COMPLETE{nl}/tree/sys/x{nl}/tree/sys/zzz{nl}"));
    assert_eq!(r, format!("TF  COMPLETE NIL{nl}/tree/sys/zzz{nl}"));

    // PROPERTIES: one record down the data connection.
    assert_eq!(
        n.command(c, 100, &format!("TG I0001 PROPERTIES{nl}/tree/sys/only.text{nl}")),
        "TG I0001 PROPERTIES"
    );
    let r = characters(&n.down(c));
    assert!(r.starts_with(&format!("/tree/sys/only.text{nl}")), "{r:?}");
    assert!(r.contains(&format!("{nl}LENGTH-IN-BYTES 1{nl}")), "{r:?}");
    n.command(c, 110, "TH I0001 CLOSE");
    n.down(c);

    // CONTINUE with nothing to continue, and with a transfer that is not
    // stopped: the server's own `BUG` either way, not an unknown command.
    assert_eq!(n.command(c, 115, "TJ  CONTINUE"), "TJ  ERROR BUG C No transfer to continue");
    n.command(c, 116, &format!("TK I0001 PROPERTIES{nl}/tree/sys/only.text{nl}"));
    assert_eq!(
        n.command(c, 117, "TL I0001 CONTINUE"),
        "TL I0001 ERROR BUG C CONTINUE received when not in error state"
    );
    n.down(c);
    n.command(c, 118, "TM I0001 CLOSE");
    n.down(c);

    // A command nobody serves.
    assert_eq!(n.command(c, 120, "TI  SPARKLE"), "TI  ERROR UKC C SPARKLE is not served here");

    assert_eq!(
        *log.lock().unwrap(),
        [
            "3050 create-directory /tree/sys/made",
            "3050 rename /tree/sys/one.lisp to /tree/sys/two.lisp",
            "3050 delete /tree/sys/two.lisp",
            "3050 create-link /tree/sys/link to /tree/sys/only.text",
            "3050 change-properties /tree/sys/only.text",
        ]
    );
}

/// **The FILE service never writes outside the tree it serves.** A
/// pathname is taken component by component with `..` and `.` refused; a
/// root itself is no file to open for writing, delete, rename or create; a
/// link deeper in that points out of the tree leads nowhere, for reading as
/// for writing. A link in the root's top level is refused like any other
/// that leaves its root (`CLAUDE.md` §3), and the same directory is served
/// by mounting it, under the mount's own rules. Every refusal is `ATD`, and
/// afterwards nothing has appeared beside the root or where either link
/// points.
#[cfg(unix)]
#[test]
fn the_file_service_never_writes_outside_its_root() {
    let s = Scratch::new("escape");
    let root = s.dir("root");
    let sys = s.dir("root/tree/sys");
    let elsewhere = s.dir("elsewhere");
    let release = s.dir("release");
    std::fs::write(elsewhere.join("victim.text"), "keep\n").unwrap();
    std::fs::write(sys.join("in.text"), "in\n").unwrap();
    std::fs::write(release.join("hello.text"), "hello\n").unwrap();
    // A link deeper in the tree that leads out of it, and one in the root's
    // top level, which is no mount.
    s.link("root/tree/sys/out", &elsewhere);
    s.link("root/mount", &release);

    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let mut now = 30;

    // The root itself, spelt three ways, and climbing out or staying put.
    for name in [
        "",
        "/",
        "//",
        "/../evil.text",
        "/tree/../../evil.text",
        "/./evil.text",
        "/tree/sys/./x.text",
    ] {
        let cmd = format!("O0001 OPEN WRITE CHARACTER{nl}{name}{nl}");
        refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    }
    refused(&mut n, c, &mut now, &s.dir, "ATD", &format!(" DELETE{nl}{nl}"));
    refused(&mut n, c, &mut now, &s.dir, "ATD", &format!(" CREATE-DIRECTORY{nl}{nl}"));
    let cmd = format!(" RENAME{nl}/tree/sys/in.text{nl}/../moved.text{nl}");
    refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);

    // Through the deeper link: writing, deleting, renaming into it, making a
    // directory in it, reading from it, and listing it.
    for cmd in [
        format!("O0001 OPEN WRITE CHARACTER{nl}/tree/sys/out/evil.text{nl}"),
        format!(" DELETE{nl}/tree/sys/out/victim.text{nl}"),
        format!(" RENAME{nl}/tree/sys/in.text{nl}/tree/sys/out/moved.text{nl}"),
        format!(" CREATE-DIRECTORY{nl}/tree/sys/out/newdir{nl}"),
        format!("I0001 OPEN READ CHARACTER{nl}/tree/sys/out/victim.text{nl}"),
        format!("I0001 DIRECTORY{nl}/tree/sys/out/*{nl}"),
    ] {
        refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    }

    // The link in the root's top level is no second tree: refused, read and
    // write alike.
    for cmd in [
        format!("I0001 OPEN PROBE CHARACTER{nl}/mount/hello.text{nl}"),
        format!("O0001 OPEN WRITE CHARACTER{nl}/mount/new.text{nl}"),
        format!("I0001 DIRECTORY{nl}/mount/*{nl}"),
    ] {
        refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    }

    // Nothing moved, nothing appeared beside the root or where either link
    // points, and no temporary was left anywhere.
    assert_eq!(std::fs::read_to_string(elsewhere.join("victim.text")).unwrap(), "keep\n");
    let names = |d: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    assert_eq!(names(&s.dir), ["elsewhere", "release", "root"]);
    assert_eq!(names(&elsewhere), ["victim.text"]);
    assert_eq!(names(&root), ["mount", "tree"]);
    assert_eq!(names(&sys), ["in.text", "out"]);
    assert_eq!(names(&release), ["hello.text"]);

    // And a write under the root is still a write.
    accepted(&mut n, c, &mut now, &format!("O0001 OPEN WRITE CHARACTER{nl}/tree/sys/ok.text{nl}"));

    // The same directory mounted is served, by the mount's rules: here
    // read, and not written.
    let mut n = serve(vec![base(&root), readonly(mount("mount", &release))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    accepted(&mut n, c, &mut now, &format!("I0001 OPEN PROBE CHARACTER{nl}/mount/hello.text{nl}"));
    let cmd = format!("O0001 OPEN WRITE CHARACTER{nl}/mount/new.text{nl}");
    refused(&mut n, c, &mut now, &s.dir, "ATF", &cmd);
    assert_eq!(names(&release), ["hello.text"]);
}

/// **A write goes into the temporary it made, and nowhere else.** The
/// temporary is made once, new, and written through the
/// handle it was made with (`DESIGN.md` §6, "FILE's rules"), so taking it
/// away fails nothing and makes nothing again by name: the data goes on into
/// the file that was taken away, and the CLOSE, whose rename finds nothing
/// to put into place, fails --- fatal, `MSC`, as a failed rename is --- and
/// puts nothing there. The next write on the handle is a write.
#[test]
fn a_temporary_taken_away_is_not_made_again() {
    let s = Scratch::new("taken");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    n.command(c, 30, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tree/sys/w.text{nl}"));

    // The first chunk lands in the temporary.
    n.send_data(c, 40, file::CHARACTER_OP, b"first ");
    let temp = dir.join(&temporaries(&dir)[0]);
    assert_eq!(std::fs::read(&temp).unwrap(), b"first ");

    // Take it away: the next chunk is written all the same, and not by name.
    std::fs::remove_file(&temp).unwrap();
    n.send_data(c, 50, file::CHARACTER_OP, b"second");
    assert!(n.down(c).is_empty(), "no mark: nothing failed");
    assert!(temporaries(&dir).is_empty(), "and nothing was made again");

    // The mark and the CLOSE: nothing to rename, so nothing in place.
    n.send_data(c, 55, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 60, "T4 O0001 CLOSE");
    assert!(r.starts_with("T4 O0001 ERROR MSC F "), "{r:?}");
    assert!(!dir.join("w.text").exists(), "nothing was put in place");
    assert!(temporaries(&dir).is_empty());

    // Again, and this time it goes through.
    n.command(c, 70, &format!("T5 O0001 OPEN WRITE CHARACTER{nl}/tree/sys/w.text{nl}"));
    n.send_data(c, 75, file::CHARACTER_OP, b"first second third");
    n.send_data(c, 76, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 80, "T6 O0001 CLOSE");
    assert!(r.starts_with("T6 O0001 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read_to_string(dir.join("w.text")).unwrap(), "first second third");
}

/// **`FILEPOS` moves a read**, which is what the client's
/// `:SET-BUFFER-POINTER` sends. It goes with the mark flag set, so the
/// reply is followed by a synchronous mark --- the client reads until that
/// to throw away what was in flight --- and then the file again from the
/// byte asked for.
#[test]
fn a_read_can_be_repositioned() {
    let s = Scratch::new("pos");
    let root = s.dir("base");
    s.file("base/tree/sys/p.text", "0123456789");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    n.command(c, 30, &format!("T3 I0001 OPEN READ CHARACTER{nl}/tree/sys/p.text{nl}"));
    assert_eq!(characters(&n.down(c)), "0123456789");

    // Back to byte four: the mark first, then the rest.
    assert_eq!(n.command(c, 40, "T4 I0001 FILEPOS 4"), "T4 I0001 FILEPOS");
    let down = n.down(c);
    assert_eq!(down[0].0, file::SYNC_MARK_OP, "the mark comes first: {down:?}");
    assert_eq!(characters(&down), "456789");
    assert!(down.iter().any(|(o, _)| *o == op::EOF), "and it ends");

    // The end of the file is a position; past it is not.
    assert_eq!(n.command(c, 50, "T5 I0001 FILEPOS 10"), "T5 I0001 FILEPOS");
    let down = n.down(c);
    assert!(!down.iter().any(|(o, _)| *o == file::CHARACTER_OP), "nothing left: {down:?}");
    assert_eq!(
        n.command(c, 60, "T6 I0001 FILEPOS 11"),
        "T6 I0001 ERROR FOR C Filepos out of range"
    );
    assert_eq!(n.command(c, 70, "T7  FILEPOS 0"), "T7  ERROR BUG C No transfer to position");
    n.command(c, 80, "T8 I0001 CLOSE");
}

/// **Two files written one after the other on the same data connection,
/// the second long.** What the band does when asked twice: `OPEN WRITE` on
/// the same output handle again, the data in as many packets as it takes,
/// the mark, `CLOSE`. Both files go into place.
#[test]
fn the_file_service_writes_a_second_long_file_on_the_same_handle() {
    let s = Scratch::new("write2");
    let root = s.dir("base");
    let dir = s.dir("base/tmp");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let r = n.command(c, 30, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tmp/first.text{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 40, file::CHARACTER_OP, &[b'o', b'n', b'e', NEWLINE]);
    n.send_data(c, 45, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 50, "T4 O0001 CLOSE");
    assert!(r.starts_with("T4 O0001 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read(dir.join("first.text")).unwrap(), b"one\n");

    let r = n.command(c, 60, &format!("T5 O0001 OPEN WRITE CHARACTER{nl}/tmp/second.text{nl}"));
    assert!(r.starts_with("T5 O0001 OPEN "), "{r:?}");
    let line: Vec<u8> =
        b"Data is address shifted 13 places".iter().copied().chain([NEWLINE]).collect();
    let all = line.repeat(200);
    let mut t = 70;
    for chunk in all.chunks(packet::MAX_DATA) {
        n.send_data(c, t, file::CHARACTER_OP, chunk);
        t += 1;
    }
    n.send_data(c, t, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, t + 10, "T6 O0001 CLOSE");
    assert!(r.starts_with("T6 O0001 CLOSE "), "{r:?}");
    let got = std::fs::read(dir.join("second.text")).expect("the second file in place");
    assert_eq!(got.len(), all.len());
    assert!(temporaries(&dir).is_empty(), "no temporary left");
}

/// **A reply longer than a packet crosses in as many as it takes.** The
/// control connection is a character stream and a packet carries 488 bytes
/// of it. A command the user end sends fits one; the reply quoting it back
/// --- an unknown command's name in its `ERROR`, a `LOGIN`'s user in its
/// home directory --- need not, and goes as a stream does.
#[test]
fn a_reply_longer_than_a_packet_is_sent_in_pieces() {
    let s = Scratch::new("long");
    let root = s.dir("base");
    let mut n = serve(vec![base(&root)]);
    let c = n.client(LM1, 0);
    let name = "X".repeat(470);
    n.send_command(c, 10, &format!("T9  {name}"));
    let replies = std::mem::take(&mut n.wire(c).replies);
    let all = replies.concat();
    assert!(all.starts_with("T9  ERROR UKC C "), "{all:?}");
    assert!(all.contains(&name), "the name is quoted back whole");
    let sizes: Vec<usize> = replies.iter().map(String::len).collect();
    assert!(sizes.len() > 1, "more than one packet: {sizes:?}");
    assert!(sizes.iter().all(|&n| n <= packet::MAX_DATA), "each within a packet: {sizes:?}");
}

/// **A completion is cut between characters.** Names on the host are
/// UTF-8; two that share a lead byte and differ in the next used to be cut
/// inside the character, which is not a string and stopped the server.
#[test]
fn a_completion_is_cut_between_characters() {
    let s = Scratch::new("utf8");
    let root = s.dir("base");
    s.file("base/tree/sys/α1.lisp", "");
    s.file("base/tree/sys/β2.lisp", "");
    let mut n = serve(vec![base(&root)]);
    let c = n.client(LM1, 0);
    let nl = NEWLINE as char;
    let r = n.command(c, 10, &format!("T1  COMPLETE{nl}/tree/sys/x{nl}/tree/sys/{nl}"));
    assert_eq!(
        r,
        format!("T1  COMPLETE NIL{nl}/tree/sys/{nl}"),
        "nothing in common past the slash"
    );
}

/// **A wildcard is matched in linear time.** `DIRECTORY` matches names
/// against the glob the user end sends, `*` any run and `?` any one
/// character. A matcher that tries every way of dividing the name among the
/// stars takes time exponential in their number on a name that almost
/// matches: ten stars against sixty `a`s is billions of tries, and the
/// loop's one thread stops answering while it makes them. The answers are
/// the same; they come at once.
#[test]
fn a_wildcard_is_matched_in_linear_time() {
    use ozd::service::file::matches;
    for (pattern, name, want) in [
        ("*", "anything", true),
        ("*", "", true),
        ("", "anything", true),
        ("*.lisp", "hello.lisp", true),
        ("*.lisp", "hello.qfasl", false),
        ("h?llo.*", "hello.lisp", true),
        ("h?llo.*", "hllo.lisp", false),
        ("hello.lisp", "hello.lisp", true),
        ("hello.lisp", "hello.lis", false),
        ("hello.lis", "hello.lisp", false),
        ("*o*", "hello", true),
        ("*x*", "hello", false),
        ("a*b*c", "abc", true),
        ("a*b*c", "aXbYc", true),
        ("a*b*c", "aXbY", false),
        ("*a", "aaa", true),
        ("a**", "a", true),
        ("?", "", false),
        ("*βγ", "αβγ", true),
    ] {
        assert_eq!(matches(pattern, name), want, "{pattern:?} against {name:?}");
    }
    let name = "a".repeat(60);
    let started = std::time::Instant::now();
    assert!(!matches("*a*a*a*a*a*a*a*a*a*a*b", &name), "there is no b in it");
    assert!(matches("*a*a*a*a*a*a*a*a*a*a*", &name));
    assert!(matches("*a*a*a*a*a*a*a*a*a*a*a", &name));
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());

    // And through the service, on a directory holding such a name.
    let s = Scratch::new("glob");
    let root = s.dir("base");
    let dir = s.dir("base/tree");
    std::fs::write(dir.join(&name), "").unwrap();
    // Ten a's and a b: the one name the pattern matches.
    let hit = format!("{}b", "a".repeat(10));
    std::fs::write(dir.join(&hit), "").unwrap();
    let mut n = serve(vec![base(&root)]);
    let c = n.client(LM1, 0);
    let nl = NEWLINE as char;
    n.listen(c, "O0001");
    n.command(c, 10, "T1  DATA-CONNECTION I0001 O0001");
    let started = std::time::Instant::now();
    let r = n.command(c, 20, &format!("T2 I0001 DIRECTORY{nl}/tree/*a*a*a*a*a*a*a*a*a*a*b{nl}"));
    assert_eq!(r, "T2 I0001 DIRECTORY");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    let listing = characters(&n.down(c));
    assert!(listing.contains(&format!("/tree/{hit}{nl}")), "{listing:?}");
    assert!(!listing.contains(&name), "sixty a's do not end in b");
}

/// **A FIFO in the root is not opened.** A named pipe with no writer blocks
/// whoever opens it --- here the loop's one thread, and every client with
/// it (`CLAUDE.md` §8d) --- and a device streams without end. So only a
/// regular file or a directory is opened (`DESIGN.md` §6, "Opening"), its
/// kind read first with `symlink_metadata`, which opens nothing, and
/// anything else is refused: on `OPEN READ`, on `PROBE`, and on an `OPEN
/// WRITE` over it, before a temporary is made.
///
/// **The code is `WKF`**, not the `ATD` the tree gives for it. The band
/// makes a condition of both --- `open.lisp` puts each code
/// on `FILE-ERROR` (`sys/io/file/open.lisp:231`, `:260`) and `qfile.lisp`
/// signals what it finds there (`QFILE-PROCESS-ERROR-NEW`) --- so the
/// choice is what each says: `ATD` is `INCORRECT-ACCESS-TO-DIRECTORY`, an
/// `ACCESS-ERROR`, "Directory protection screwed you."; `WKF` is
/// `WRONG-KIND-OF-FILE`, the condition whose kinds are an operation invalid
/// for a directory or for a link, which is what this is. A listing looks at
/// the FIFO without opening it.
#[cfg(unix)]
#[test]
fn a_fifo_in_the_root_is_not_read() {
    let s = Scratch::new("fifo");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    if !s.fifo("base/tree/sys/pipe") {
        eprintln!("skipped: mkfifo is not available");
        return;
    }
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let r = n.command(c, 30, &format!("T3 I0001 OPEN READ CHARACTER{nl}/tree/sys/pipe{nl}"));
    assert_eq!(r, "T3 I0001 ERROR WKF C Not a regular file");
    let r = n.command(c, 40, &format!("T4  OPEN PROBE CHARACTER{nl}/tree/sys/pipe{nl}"));
    assert_eq!(r, "T4  ERROR WKF C Not a regular file");
    for (tid, how) in [("T5", "APPEND"), ("T6", "OVERWRITE"), ("T7", "SUPERSEDE")] {
        let r = n.command(
            c,
            50,
            &format!("{tid} O0001 OPEN WRITE CHARACTER IF-EXISTS {how}{nl}/tree/sys/pipe{nl}"),
        );
        assert_eq!(r, format!("{tid} O0001 ERROR WKF C Not a regular file"), "{how}");
    }
    assert!(temporaries(&dir).is_empty(), "nothing was started");
    // A listing looks at it without opening it.
    let r = n.command(c, 60, &format!("T8 I0001 DIRECTORY{nl}/tree/sys/*{nl}"));
    assert_eq!(r, "T8 I0001 DIRECTORY");
    let listing = characters(&n.down(c));
    assert!(listing.contains(&format!("/tree/sys/pipe{nl}")), "{listing:?}");
    // Named as a directory, it is none, and is not opened to find out.
    let r = n.command(c, 70, &format!("T9 I0001 DIRECTORY{nl}/tree/sys/pipe/*{nl}"));
    assert_eq!(r, "T9 I0001 ERROR DNF C Directory not found");
}

/// **Two control connections from one host write in one directory without
/// touching each other's temporary.** The band opens a second control
/// connection to a host when the first is busy --- a second host unit in
/// `qfile.lisp` --- and a temporary used to be named by the client's address
/// and a count kept per control connection, so the first write of each
/// connection in one directory took the same name, and making the second
/// emptied the first. A temporary is made new under a name no other write
/// can take (`roots::temporary_name`).
#[test]
fn two_control_connections_write_in_one_directory() {
    let s = Scratch::new("two");
    let root = s.dir("base");
    let dir = s.dir("base/tmp");
    let mut n = serve(vec![base(&root)]);
    let nl = NEWLINE as char;
    let a = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    // The same host, on another connection.
    let b = ready(&mut n, LM1, "LISPM", ("I0002", "O0002"), 1);

    let r = n.command(a, 30, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tmp/a.text{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(a, 35, file::CHARACTER_OP, b"from a");
    let r = n.command(b, 31, &format!("T3 O0002 OPEN WRITE CHARACTER{nl}/tmp/b.text{nl}"));
    assert!(r.starts_with("T3 O0002 OPEN "), "{r:?}");
    assert_eq!(temporaries(&dir).len(), 2, "a temporary each: {:?}", temporaries(&dir));
    n.send_data(b, 36, file::CHARACTER_OP, b"from b");
    n.send_data(a, 40, file::SYNC_MARK_OP, &[]);
    n.send_data(b, 41, file::SYNC_MARK_OP, &[]);
    let r = n.command(a, 50, "T4 O0001 CLOSE");
    assert!(r.starts_with("T4 O0001 CLOSE "), "{r:?}");
    let r = n.command(b, 51, "T4 O0002 CLOSE");
    assert!(r.starts_with("T4 O0002 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read_to_string(dir.join("a.text")).unwrap(), "from a");
    assert_eq!(std::fs::read_to_string(dir.join("b.text")).unwrap(), "from b");
    assert!(temporaries(&dir).is_empty(), "{:?}", temporaries(&dir));
}

/// **A control connection that goes away takes its temporaries with it.** A
/// write in progress is a temporary beside the real file. When the user
/// end's connection closes under it --- a CLS, or the machine rebooted ---
/// the write will never finish, and the temporary is removed instead of
/// staying in the directory. An `UNDATA-CONNECTION` on a handle in the
/// middle of a write ends that write the same way.
#[test]
fn a_lost_control_connection_leaves_no_temporary() {
    let s = Scratch::new("lost");
    let root = s.dir("base");
    let dir = s.dir("base/tmp");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    // UNDATA-CONNECTION in the middle of a write.
    let r = n.command(c, 30, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tmp/w.text{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 40, file::CHARACTER_OP, b"half");
    assert_eq!(temporaries(&dir).len(), 1);
    assert_eq!(n.command(c, 50, "T4 O0001 UNDATA-CONNECTION"), "T4 O0001 UNDATA-CONNECTION");
    assert!(temporaries(&dir).is_empty(), "{:?}", temporaries(&dir));
    assert!(!dir.join("w.text").exists(), "and nothing went into place");

    // A new data connection, a write, and the control connection closes.
    n.listen(c, "O0002");
    assert_eq!(n.command(c, 60, "T5  DATA-CONNECTION I0002 O0002"), "T5  DATA-CONNECTION");
    let r = n.command(c, 70, &format!("T6 O0002 OPEN WRITE CHARACTER{nl}/tmp/w.text{nl}"));
    assert!(r.starts_with("T6 O0002 OPEN "), "{r:?}");
    n.send_data(c, 80, file::CHARACTER_OP, b"half");
    assert_eq!(temporaries(&dir).len(), 1);
    n.wire(c).control.push(Out::Close("Rebooted".into()));
    n.settle(90);
    assert_eq!(n.server.connections(), 0, "the data connection went with it");
    assert!(temporaries(&dir).is_empty(), "{:?}", temporaries(&dir));
    assert!(!dir.join("w.text").exists());
}

/// **Data goes only into the write in progress.** What comes up a data
/// connection is the file being written on its output handle. Before an
/// `OPEN WRITE`, or after a `CLOSE` --- a mark the user end sent that
/// crossed the reply --- there is no file for it; it used to wait in the
/// channel, unbounded, and join the next write on that handle. It is
/// dropped. And a data packet with the `CLOSE` on its heels, no turn of the
/// server between them, used to miss the file: the close renamed the
/// temporary before the data had been looked at. It goes in first.
#[test]
fn data_goes_only_into_the_write_in_progress() {
    let s = Scratch::new("stray");
    let root = s.dir("base");
    let dir = s.dir("base/tmp");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    // Before any write: nowhere to go.
    n.send_data(c, 30, file::CHARACTER_OP, b"junk");
    n.send_data(c, 31, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 40, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tmp/w.text{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 50, file::CHARACTER_OP, b"real");

    // The last of the data, the mark and the CLOSE, back to back: each
    // packet carried by hand, and the server asked only for what it has
    // queued --- the receipt of each --- so it is never let poll its
    // sessions, which is the turn a real server might not get between them.
    n.queue_data(c, file::CHARACTER_OP, b" tail");
    n.queue_data(c, file::SYNC_MARK_OP, &[]);
    let h = n.clients[c].0;
    for want in [file::CHARACTER_OP, file::SYNC_MARK_OP] {
        let up = Packet::from_buffer(&n.hosts[h].transmit(60).unwrap()).unwrap().0;
        assert_eq!(up.opcode, want);
        n.server.receive(60, &arriving(&up));
        let receipt = Packet::from_buffer(&n.server.transmit(60).unwrap()).unwrap().0;
        assert_eq!(receipt.opcode, op::STS, "the server's queue held only the receipt");
        n.hosts[h].receive(60, &arriving(&receipt));
    }
    n.wire(c).control.push(Out::Data(lispm::lispm_text("T4 O0001 CLOSE")));
    let close = Packet::from_buffer(&n.hosts[h].transmit(61).unwrap()).unwrap().0;
    n.server.receive(61, &arriving(&close));
    n.settle(62);
    let r = n.take_reply(c, "T4").expect("the CLOSE is answered");
    assert!(r.starts_with("T4 O0001 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read_to_string(dir.join("w.text")).unwrap(), "real tail");
}

/// **A write is not put into place until the synchronous mark has come,
/// however early the CLOSE arrives.**
///
/// MIT's own `sys/doc/chfile.text` sets both halves of this. Of CLOSE it
/// says "a synchronous mark will be sent or awaited accordingly" --- sent
/// when reading, awaited when writing. And of the order the two arrive in,
/// its worked example of writing a file says to send "a SYNC mark on the
/// DATA connection and a CLOSE on the CONTROL connection (in either
/// order)". So a client that does everything the protocol asks of it may
/// still have its CLOSE overtake its mark, the two travelling on different
/// connections, and the server has to wait rather than take the CLOSE as
/// the end of the data.
///
/// Holding the reply back cannot hold the mark back with it: the client
/// writes the two without waiting between them --- `qfile.lisp`'s
/// `:COMMAND` sends the command packet on the control connection and then,
/// for an output stream, `(SEND STREAM :WRITE-SYNCHRONOUS-MARK)`, before it
/// waits for the response.
#[test]
fn a_write_is_not_placed_until_the_synchronous_mark_has_come() {
    let s = Scratch::new("mark");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let r = n.command(c, 30, &format!("T3 O0001 OPEN WRITE BINARY{nl}/tree/sys/late.qfasl{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");

    // The data, and then the CLOSE overtaking the mark.
    n.send_data(c, 40, file::BINARY_OP, &[0o215, 0o12, 0, 0o377]);
    n.send_command(c, 50, "T4 O0001 CLOSE");
    let real = dir.join("late.qfasl");
    assert!(!real.exists(), "the file waits for the mark, and does not go into place on the CLOSE");
    assert_eq!(temporaries(&dir).len(), 1, "what has been written so far is in the temporary");
    assert!(n.take_reply(c, "T4").is_none(), "and the CLOSE waits too");

    // The mark, and now it may go into place.
    n.send_data(c, 60, file::SYNC_MARK_OP, &[]);
    let r = n.take_reply(c, "T4").expect("the CLOSE is answered once the mark has come");
    assert!(r.starts_with("T4 O0001 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read(&real).unwrap(), [0o215, 0o12, 0, 0o377], "every byte sent");
    assert!(temporaries(&dir).is_empty(), "and the temporary is gone");
}

/// **A CLOSE still waiting for its mark is answered when the transfer is
/// taken away under it.** A write's CLOSE is held until the synchronous
/// mark comes, so a client that abandons the transfer instead of sending
/// the mark --- an `UNDATA-CONNECTION` on the handle --- would otherwise
/// wait for a reply that no longer had anything to come from. It gets
/// `CNO`, `chfile.text`'s "CLOSE on non-open channel".
///
/// The band does not do this, and the test is here because the deferral is
/// what makes it possible to hang. Its `:REAL-CLOSE` waits for the CLOSE's
/// reply before freeing the data connection, and it only undoes a
/// connection that has gone dormant.
#[test]
fn a_close_waiting_for_its_mark_is_answered_if_the_transfer_goes_away() {
    let s = Scratch::new("strand");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let r = n.command(c, 30, &format!("T3 O0001 OPEN WRITE BINARY{nl}/tree/sys/gone.qfasl{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 40, file::BINARY_OP, &[1, 2, 3]);
    n.send_command(c, 50, "T4 O0001 CLOSE");
    assert!(n.take_reply(c, "T4").is_none(), "the CLOSE waits for the mark");

    // The mark never comes; the data connection is undone instead.
    let r = n.command(c, 60, "T5 I0001 UNDATA-CONNECTION");
    assert!(r.starts_with("T5 I0001 UNDATA-CONNECTION"), "{r:?}");
    let r = n.take_reply(c, "T4").expect("and the CLOSE is answered rather than left waiting");
    assert!(r.starts_with("T4 O0001 ERROR CNO C "), "{r:?}");
    assert!(!dir.join("gone.qfasl").exists(), "nothing went into place");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "and no temporary was left");
}

/// **A file being read and a file being written on one data connection
/// keep their own bytes.** A data connection has two file handles and both
/// halves may be busy at once --- "the input file handle is used to
/// describe the receive half of the DATA connection and the output file
/// handle is use to describe the send half" (`sys/doc/chfile.text`) --- and
/// the band does it whenever it compiles: `sys/qcfile.lisp`'s `QC-FILE`
/// holds the source open around the `WITH-OPEN-FILE` that writes the QFASL.
///
/// **The handles are named so that the input one sorts first, and that is
/// what makes this test bite**: the poll walks the handles in order, and
/// the drain that would lose the bytes is the input handle's. Named the
/// other way, the test would pass against a service with the bug back.
#[test]
fn a_read_and_a_write_on_one_data_connection_keep_their_own_bytes() {
    let s = Scratch::new("both");
    let root = s.dir("base");
    let dir = s.dir("base/tree/sys");
    std::fs::write(dir.join("source.lisp"), "(DEFUN F (X) X)").unwrap();
    let mut n = serve(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    // The source on the receive half, and the QFASL on the send half while
    // that read is still open.
    let r = n.command(c, 30, &format!("T3 I0001 OPEN READ CHARACTER{nl}/tree/sys/source.lisp{nl}"));
    assert!(r.starts_with("T3 I0001 OPEN "), "{r:?}");
    let r = n.command(c, 40, &format!("T4 O0001 OPEN WRITE BINARY{nl}/tree/sys/source.qfasl{nl}"));
    assert!(r.starts_with("T4 O0001 OPEN "), "{r:?}");

    n.send_data(c, 50, file::BINARY_OP, &[0o215, 0o12, 0, 0o377]);
    n.send_data(c, 60, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, 70, "T5 O0001 CLOSE");
    assert!(r.starts_with("T5 O0001 CLOSE "), "{r:?}");
    assert_eq!(
        std::fs::read(dir.join("source.qfasl")).unwrap(),
        [0o215, 0o12, 0, 0o377],
        "the write kept every byte it sent, with a read open beside it"
    );
    assert_eq!(characters(&n.down(c)), "(DEFUN F (X) X)", "and the read delivered its own file");
}

/// **A file is written in the base and read back**, in the exchange a
/// band makes: `OPEN WRITE`, the data, and then the CLOSE on the control connection and
/// the SYNC mark on the data connection sent together, as `qfile.lisp`'s
/// `:COMMAND` sends them without waiting between; the rename waits for the
/// mark, the file is in place, no temporary is left beside it, and the
/// write is reported with its pathname. A read-only mount beside the base
/// changes nothing about it.
#[test]
fn a_read_and_a_write_in_the_base() {
    let s = Scratch::new("base");
    let root = s.dir("base");
    let tmp = s.dir("base/tmp");
    let sys = s.dir("sys-src");
    s.file("sys-src/hello.lisp", "hello\n");
    let (mut n, _, log) = serve_logged(vec![base(&root), readonly(mount("sys", &sys))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;

    let r = n.command(c, 10, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/tmp/file-write.text{nl}"));
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 20, file::CHARACTER_OP, &[b'L', b'I', b'S', b'P', b'M', NEWLINE]);
    // The CLOSE and the mark, together.
    n.wire(c).control.push(Out::Data(lispm::lispm_text("T4 O0001 CLOSE")));
    n.queue_data(c, file::SYNC_MARK_OP, &[]);
    n.settle(30);
    let r = n.take_reply(c, "T4").expect("the CLOSE is answered");
    assert!(r.starts_with("T4 O0001 CLOSE "), "{r:?}");
    assert!(r.ends_with(&format!("{nl}/tmp/file-write.text{nl}")), "{r:?}");
    assert_eq!(std::fs::read(tmp.join("file-write.text")).unwrap(), b"LISPM\n");
    assert!(temporaries(&tmp).is_empty(), "a temporary was left behind: {:?}", temporaries(&tmp));
    assert_eq!(*log.lock().unwrap(), ["3050 write /tmp/file-write.text"]);

    // Read back, in the Lisp Machine's character set.
    let r = n.command(c, 40, &format!("T5 I0001 OPEN READ CHARACTER{nl}/tmp/file-write.text{nl}"));
    assert!(r.starts_with("T5 I0001 OPEN "), "{r:?}");
    assert_eq!(characters(&n.down(c)), format!("LISPM{nl}"));
    assert!(n.command(c, 50, "T6 I0001 CLOSE").starts_with("T6 I0001 CLOSE "));
    assert_eq!(log.lock().unwrap().len(), 1, "a read changes nothing");
}

/// **A read-only mount is read, and every write there is refused with `ATF`
/// before anything is touched** (`DESIGN.md` §6, "A read-only root"; `CLAUDE.md`
/// §3): OPEN for output, of a new file and over one, and through a link;
/// DELETE, of a file, a link, a directory, and the mount itself; RENAME
/// within the mount, and across its line either way; CREATE-DIRECTORY;
/// CREATE-LINK; CHANGE-PROPERTIES. The band turns `ATF` into
/// `INCORRECT-ACCESS-TO-FILE` (`sys/io/file/open.lisp:224`). Nothing on disk
/// changes, no temporary is made, nothing is reported; and the base beside
/// the mount is written as ever.
#[cfg(unix)]
#[test]
fn a_readonly_mount_is_read_and_every_write_there_is_refused_with_atf() {
    let s = Scratch::new("readonly");
    let root = s.dir("base");
    s.file("base/mine.text", "mine\n");
    let sys = s.dir("sys-src");
    s.file("sys-src/file.lisp", "(sys)\n");
    s.dir("sys-src/dir");
    s.link("sys-src/link", "file.lisp");
    let (mut n, _, log) = serve_logged(vec![base(&root), readonly(mount("sys", &sys))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let mut now = 10;

    // Read: a file, one through a link, and the listing.
    for pathname in ["/sys/file.lisp", "/sys/link"] {
        accepted(&mut n, c, &mut now, &format!("I0001 OPEN READ CHARACTER{nl}{pathname}{nl}"));
        assert_eq!(characters(&n.down(c)), format!("(sys){nl}"));
        accepted(&mut n, c, &mut now, "I0001 CLOSE");
        n.down(c);
    }
    accepted(&mut n, c, &mut now, &format!("I0001 DIRECTORY{nl}/sys/*{nl}"));
    assert!(characters(&n.down(c)).contains(&format!("/sys/file.lisp{nl}")));
    accepted(&mut n, c, &mut now, "I0001 CLOSE");
    n.down(c);

    // Written: refused, every way.
    for cmd in [
        format!("O0001 OPEN WRITE CHARACTER{nl}/sys/new.lisp{nl}"),
        format!("O0001 OPEN WRITE CHARACTER IF-EXISTS SUPERSEDE{nl}/sys/file.lisp{nl}"),
        format!("O0001 OPEN WRITE CHARACTER IF-EXISTS APPEND{nl}/sys/file.lisp{nl}"),
        format!("O0001 OPEN WRITE BINARY{nl}/sys/dir/new.qfasl{nl}"),
        format!("O0001 OPEN WRITE CHARACTER{nl}/sys/link{nl}"),
        format!(" DELETE{nl}/sys/file.lisp{nl}"),
        format!(" DELETE{nl}/sys/link{nl}"),
        format!(" DELETE{nl}/sys/dir{nl}"),
        format!(" DELETE{nl}/sys{nl}"),
        format!(" RENAME{nl}/sys/file.lisp{nl}/sys/moved.lisp{nl}"),
        format!(" RENAME{nl}/sys/link{nl}/sys/other{nl}"),
        format!(" RENAME{nl}/sys/file.lisp{nl}/stolen.lisp{nl}"),
        format!(" RENAME{nl}/mine.text{nl}/sys/mine.text{nl}"),
        format!(" CREATE-DIRECTORY{nl}/sys/made{nl}"),
        format!(" CREATE-DIRECTORY{nl}/sys/dir/made{nl}"),
        format!(" CREATE-LINK{nl}/sys/made-link{nl}/sys/file.lisp{nl}"),
        format!(" CREATE-LINK{nl}/sys/made-link{nl}/mine.text{nl}"),
        format!(" CHANGE-PROPERTIES{nl}/sys/file.lisp{nl}AUTHOR LISPM{nl}"),
    ] {
        refused(&mut n, c, &mut now, &s.dir, "ATF", &cmd);
    }
    assert!(log.lock().unwrap().is_empty(), "{:?}", log.lock().unwrap());

    // The base beside it is written as ever.
    accepted(&mut n, c, &mut now, &format!(" DELETE{nl}/mine.text{nl}"));
    assert!(!root.join("mine.text").exists());
    assert_eq!(*log.lock().unwrap(), ["3050 delete /mine.text"]);
}

/// **A listing never describes a file outside a root** (`DESIGN.md` §6,
/// "FILE's rules"). DIRECTORY describes each entry through the tree, never
/// with `metadata`, which follows a link and would give the size and date
/// of a file outside its root to anyone who listed the link's directory.
/// So a link that leads somewhere in its own root is described
/// as what it leads to --- a directory as a directory --- and one the tree
/// will not follow, out of its root, into another root, or nowhere, as the
/// link itself: listed, so that it can be deleted, with its own length and
/// date. The file outside, its length and its date appear in no listing:
/// of the base, of a mount, or of `/`, which is the base's entries and the
/// mounts' names (`Tree::list_top`). `/` itself is a directory, and a
/// PROPERTIES through a link out of its root is refused as a read is.
#[cfg(unix)]
#[test]
fn a_listing_never_describes_a_file_outside_a_root() {
    let s = Scratch::new("listing");
    let root = s.dir("base");
    s.file("base/lispm/a.text", "abc");
    s.dir("base/lispm/sub");
    let outside = s.dir("outside");
    let victim = s.file("outside/victim.text", &"x".repeat(12345));
    std::fs::File::options()
        .write(true)
        .open(&victim)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    let sys = s.dir("sys-src");
    s.file("sys-src/file.lisp", "(sys)\n");
    for dir in ["base", "base/lispm", "sys-src"] {
        s.link(&format!("{dir}/out-file"), &victim);
        s.link(&format!("{dir}/out"), &outside);
        s.link(&format!("{dir}/dangling"), outside.join("nowhere"));
    }
    s.link("base/lispm/in", "a.text");
    s.link("base/lispm/in-dir", "sub");
    s.link("base/lispm/to-sys", sys.join("file.lisp"));
    let mut n = serve(vec![base(&root), readonly(mount("sys", &sys))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let mut now = 10;
    // The victim's length, and its date as FILE writes one, 2001-09-09.
    let secrets = ["LENGTH-IN-BYTES 12345".to_string(), "09/09/01 01:46:40".to_string()];

    for (dir, host) in [("/lispm", root.join("lispm")), ("/sys", sys.clone()), ("", root.clone())] {
        accepted(&mut n, c, &mut now, &format!("I0001 DIRECTORY{nl}{dir}/*{nl}"));
        let listing = characters(&n.down(c));
        accepted(&mut n, c, &mut now, "I0001 CLOSE");
        n.down(c);
        for secret in &secrets {
            assert!(!listing.contains(secret.as_str()), "{dir}/*: {secret} in {listing:?}");
        }
        // Each link the tree will not follow, listed as itself.
        for name in ["out-file", "out", "dangling"] {
            let r = record(&listing, &format!("{dir}/{name}"));
            let own = std::fs::symlink_metadata(host.join(name)).unwrap().len();
            assert!(r.contains(&format!("{nl}LENGTH-IN-BYTES {own}{nl}")), "{r:?}");
            assert!(!r.contains("DIRECTORY T"), "{r:?}");
        }
    }

    // In the base, a link in its own root is what it leads to; one into
    // another root is the link itself.
    accepted(&mut n, c, &mut now, &format!("I0001 DIRECTORY{nl}/lispm/*{nl}"));
    let listing = characters(&n.down(c));
    accepted(&mut n, c, &mut now, "I0001 CLOSE");
    n.down(c);
    assert!(record(&listing, "/lispm/in").contains(&format!("{nl}LENGTH-IN-BYTES 3{nl}")));
    assert!(record(&listing, "/lispm/in-dir").ends_with(&format!("{nl}DIRECTORY T")));
    let own = std::fs::symlink_metadata(root.join("lispm/to-sys")).unwrap().len();
    let to_sys = record(&listing, "/lispm/to-sys");
    assert!(to_sys.contains(&format!("{nl}LENGTH-IN-BYTES {own}{nl}")), "{to_sys:?}");

    // `/` lists the base's entries and the mount, each a directory where it
    // is one.
    accepted(&mut n, c, &mut now, &format!("I0001 DIRECTORY{nl}/*{nl}"));
    let listing = characters(&n.down(c));
    accepted(&mut n, c, &mut now, "I0001 CLOSE");
    n.down(c);
    for name in ["/lispm", "/sys"] {
        assert!(record(&listing, name).ends_with(&format!("{nl}DIRECTORY T")), "{name}");
    }

    // `/` itself: a directory, of no length.
    let r = accepted(&mut n, c, &mut now, &format!(" OPEN PROBE-DIRECTORY CHARACTER{nl}/{nl}"));
    assert!(r.contains(&format!(" 0 NIL{nl}/{nl}")), "{r:?}");
    accepted(&mut n, c, &mut now, &format!("I0001 PROPERTIES{nl}/{nl}"));
    let properties = characters(&n.down(c));
    accepted(&mut n, c, &mut now, "I0001 CLOSE");
    n.down(c);
    assert!(properties.starts_with(&format!("/{nl}")), "{properties:?}");
    assert!(properties.contains(&format!("{nl}DIRECTORY T{nl}")), "{properties:?}");

    // Through a link out of its root, PROPERTIES is refused as a read is.
    for pathname in ["/lispm/out-file", "/out-file", "/sys/out-file", "/lispm/to-sys"] {
        let cmd = format!("I0001 PROPERTIES{nl}{pathname}{nl}");
        refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    }
}

/// **DELETE removes a link, never what it leads to** (`DESIGN.md` §6,
/// "FILE's rules"), as Unix does: its directory is
/// resolved, and its own name is not followed. So a link that leads nowhere
/// or round in a circle --- which no read or write will touch --- is got rid
/// of the way any other is, and so is one out of its root or into another;
/// what they led to stays, and each is reported. A link in a read-only mount
/// is refused like everything else there, and a DELETE through a link out
/// of its root reaches nothing beyond it.
#[cfg(unix)]
#[test]
fn a_link_is_deleted_itself_and_not_what_it_leads_to() {
    let s = Scratch::new("delete-links");
    let root = s.dir("base");
    s.file("base/a.text", "a\n");
    s.file("base/sub/b.text", "b\n");
    let outside = s.dir("outside");
    let victim = s.file("outside/victim.text", "keep\n");
    let sys = s.dir("sys-src");
    s.file("sys-src/file.lisp", "(sys)\n");
    s.link("sys-src/link", "file.lisp");
    let links = [
        ("in", PathBuf::from("a.text")),
        ("in-dir", PathBuf::from("sub")),
        ("out-file", victim.clone()),
        ("out", outside.clone()),
        ("dangling", outside.join("nowhere")),
        ("loop", PathBuf::from("loop")),
        ("to-sys", sys.join("file.lisp")),
    ];
    for (name, target) in &links {
        s.link(&format!("base/{name}"), target);
    }
    s.link("base/keep-out", &outside);
    let (mut n, _, log) = serve_logged(vec![base(&root), readonly(mount("sys", &sys))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let mut now = 10;

    let mut expected = snapshot(&s.dir);
    for (name, _) in &links {
        accepted(&mut n, c, &mut now, &format!(" DELETE{nl}/{name}{nl}"));
        expected.remove(Path::new("base").join(name).as_path()).unwrap();
        assert_eq!(snapshot(&s.dir), expected, "DELETE /{name}: the link, and nothing else");
    }
    let reported: Vec<String> = links.iter().map(|(n, _)| format!("3050 delete /{n}")).collect();
    assert_eq!(*log.lock().unwrap(), reported);

    // In a read-only mount, refused; through a link out of its root,
    // refused, and what is beyond it kept.
    refused(&mut n, c, &mut now, &s.dir, "ATF", &format!(" DELETE{nl}/sys/link{nl}"));
    refused(&mut n, c, &mut now, &s.dir, "ATD", &format!(" DELETE{nl}/keep-out/victim.text{nl}"));
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep\n");
}

/// **RENAME moves a link, never what it leads to** (`DESIGN.md` §6, "FILE's
/// rules"): the link goes to its new name as it is, its content unchanged.
/// One that holds an absolute path leads where it did --- into its root,
/// where it is read through as before, or out of it, where it is refused as
/// before; a relative one leads on from where it now is, here to nothing,
/// and is refused as any link that leads nowhere is. One that leads nowhere
/// is moved like any other. A new name that is a link is there already,
/// whether or not it leads anywhere, `REF`. Each move is reported.
#[cfg(unix)]
#[test]
fn a_link_is_renamed_itself() {
    let s = Scratch::new("rename-links");
    let root = s.dir("base");
    s.file("base/a.text", "a\n");
    s.dir("base/sub");
    let outside = s.dir("outside");
    s.file("outside/victim.text", "keep\n");
    s.link("base/in", root.join("a.text"));
    s.link("base/rel", "a.text");
    s.link("base/dangling", outside.join("nowhere"));
    s.link("base/out", &outside);
    s.link("base/taken", "nowhere");
    let (mut n, _, log) = serve_logged(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let mut now = 10;

    for (from, to) in
        [("in", "sub/moved"), ("rel", "sub/rel"), ("dangling", "sub/dangling"), ("out", "sub/out")]
    {
        let before = std::fs::read_link(root.join(from)).unwrap();
        accepted(&mut n, c, &mut now, &format!(" RENAME{nl}/{from}{nl}/{to}{nl}"));
        assert!(std::fs::symlink_metadata(root.join(from)).is_err(), "{from} moved");
        assert_eq!(std::fs::read_link(root.join(to)).unwrap(), before, "{to} leads where it did");
    }
    assert_eq!(std::fs::read_to_string(root.join("a.text")).unwrap(), "a\n");
    accepted(&mut n, c, &mut now, &format!("I0001 OPEN READ CHARACTER{nl}/sub/moved{nl}"));
    assert_eq!(characters(&n.down(c)), format!("a{nl}"));
    accepted(&mut n, c, &mut now, "I0001 CLOSE");
    n.down(c);
    let cmd = format!("I0001 OPEN READ CHARACTER{nl}/sub/rel{nl}");
    refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    let cmd = format!("I0001 OPEN READ CHARACTER{nl}/sub/out/victim.text{nl}");
    refused(&mut n, c, &mut now, &s.dir, "ATD", &cmd);
    refused(&mut n, c, &mut now, &s.dir, "REF", &format!(" RENAME{nl}/a.text{nl}/taken{nl}"));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "3050 rename /in to /sub/moved",
            "3050 rename /rel to /sub/rel",
            "3050 rename /dangling to /sub/dangling",
            "3050 rename /out to /sub/out",
        ]
    );
}

/// **A write cut off leaves only its temporary, and startup removes it**
/// (`DESIGN.md` §6, §10): a daemon killed in the middle of a write --- here
/// the server's NCP dropped, sessions and all, with no CLS and no CLOSE ---
/// has put nothing in place, and the one thing it leaves is the temporary
/// it was writing, which [`Tree::remove_temporaries`] finds by the one name
/// FILE gives them, and removes.
#[test]
fn an_interrupted_write_leaves_only_a_temporary_that_startup_removes() {
    let s = Scratch::new("interrupted");
    let root = s.dir("base");
    let tmp = s.dir("base/tmp");
    s.file("base/tmp/old.text", "old\n");
    let (mut n, tree, log) = serve_logged(vec![base(&root)]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let before = snapshot(&s.dir);

    let r = n.command(
        c,
        10,
        &format!("T3 O0001 OPEN WRITE CHARACTER IF-EXISTS SUPERSEDE{nl}/tmp/old.text{nl}"),
    );
    assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    n.send_data(c, 20, file::CHARACTER_OP, b"half a new ");
    let temps = temporaries(&tmp);
    assert_eq!(temps.len(), 1);
    drop(n);

    let mut expected = before.clone();
    expected.insert(Path::new("base/tmp").join(&temps[0]), Entry::File(b"half a new ".to_vec()));
    assert_eq!(snapshot(&s.dir), expected, "what was there, and the temporary");
    assert!(log.lock().unwrap().is_empty(), "nothing was put in place");
    let removed: Vec<PathBuf> = tree.remove_temporaries().into_iter().map(|r| r.unwrap()).collect();
    assert_eq!(removed, [tmp.join(&temps[0])]);
    assert_eq!(snapshot(&s.dir), before, "and then only what was there");
}

/// **Two machines at once, and neither sees the other's session**
/// (`CLAUDE.md` §10, item 6): each holds a control connection and a data
/// connection of its own, under the same handle names and the same
/// transaction ids, and each writes and reads at the same time as the
/// other. Every reply, every file and every listing goes to the machine
/// that asked; who is logged in is each session's own; and one machine
/// going away in the middle of a write takes only its own temporary, while
/// the other's write goes on into place.
#[test]
fn two_clients_at_once_see_only_their_own_sessions() {
    let s = Scratch::new("two-machines");
    let root = s.dir("base");
    let a_dir = s.dir("base/a");
    let b_dir = s.dir("base/b");
    s.file("base/a/in.text", "for a\n");
    s.file("base/b/in.text", "for b\n");
    let mut n = serve(vec![base(&root)]);
    let nl = NEWLINE as char;
    let a = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let b = ready(&mut n, LM2, "OTHER", ("I0001", "O0001"), 1);
    let both = [(a, "a", "LISPM"), (b, "b", "OTHER")];

    // Both write at once.
    for (c, who, _) in both {
        let r = n.command(c, 10, &format!("T3 O0001 OPEN WRITE CHARACTER{nl}/{who}/out.text{nl}"));
        assert!(r.starts_with("T3 O0001 OPEN "), "{r:?}");
    }
    n.send_data(a, 20, file::CHARACTER_OP, b"from a");
    n.send_data(b, 21, file::CHARACTER_OP, b"from b");

    // And both read, while the writes are open: each its own file.
    for (c, who, _) in both {
        let r = n.command(c, 30, &format!("T4 I0001 OPEN READ CHARACTER{nl}/{who}/in.text{nl}"));
        assert!(r.ends_with(&format!("{nl}/{who}/in.text{nl}")), "{r:?}");
        assert_eq!(characters(&n.down(c)), format!("for {who}{nl}"), "{who}: its own file");
        assert!(n.command(c, 31, "T5 I0001 CLOSE").starts_with("T5 I0001 CLOSE "));
        assert_eq!(opcodes(&n.down(c)), [file::SYNC_MARK_OP]);
    }

    // Each session's user is its own.
    for (c, who, user) in both {
        n.command(c, 40, &format!("T6 I0001 DIRECTORY{nl}/{who}/*{nl}"));
        let listing = characters(&n.down(c));
        assert!(listing.contains(&format!("{nl}AUTHOR {user}{nl}")), "{listing:?}");
        n.command(c, 41, "T7 I0001 CLOSE");
        n.down(c);
    }

    // B goes away in the middle of its write, and A's goes on into place.
    n.wire(b).control.push(Out::Close("Rebooted".into()));
    n.settle(50);
    assert_eq!(n.server.connections(), 2, "A's two connections, and none of B's");
    assert!(temporaries(&b_dir).is_empty() && !b_dir.join("out.text").exists());
    assert_eq!(temporaries(&a_dir).len(), 1, "A's write is still open");
    n.send_data(a, 60, file::SYNC_MARK_OP, &[]);
    let r = n.command(a, 70, "T8 O0001 CLOSE");
    assert!(r.starts_with("T8 O0001 CLOSE "), "{r:?}");
    assert_eq!(std::fs::read_to_string(a_dir.join("out.text")).unwrap(), "from a");
    assert!(temporaries(&a_dir).is_empty());

    // Nothing either was sent went to the other.
    for (c, who, _) in both {
        assert!(n.wire(c).replies.is_empty(), "{who}: {:?}", n.wire(c).replies);
        assert!(n.wire(c).down.is_empty(), "{who}: {:?}", n.wire(c).down);
    }
}

/// **The containment table, through FILE's own commands** (`CLAUDE.md` §10,
/// item 4; `tests/containment.rs` holds it against the tree alone). Each
/// pathname a client can send that would reach outside a root, sent in
/// every command that names one, is refused with its code and changes
/// nothing on disk:
///
/// - `..` and `.` at every depth, and a temporary's name: `ATD`.
/// - A host's absolute pathname lands under the root, and finds nothing
///   there: `FNF`, or `DNF` for what would be made in a directory that is
///   not there. The host's own file is never it.
/// - A link under the root that leads out of it, or nowhere, followed:
///   `ATD`, through it and to it.
/// - A link the client makes with `CREATE-LINK`: its target is resolved in
///   the tree, so it cannot lead out; one into another root is made, and
///   refused wherever it is used, `ATD`; so is one to a host path, which
///   lands under the root and leads nowhere.
/// - A directory swapped for such a link between two commands, with FILE's
///   own RENAME and CREATE-LINK: `ATD` the next time it is named. And in
///   the middle of a write, the CLOSE's rename through the link finds no
///   temporary where it leads, `MSC`, and puts nothing there.
/// - A write whose temporary or rename target would be outside: `ATD`.
/// - A FIFO: `WKF`, not containment but the loop's one thread.
///
/// What is outside, and the read-only mount, are the same at the end.
#[cfg(unix)]
#[test]
fn the_containment_table_through_file_commands() {
    let s = Scratch::new("table");
    let root = s.dir("base");
    s.file("base/ok.text", "ok\n");
    s.file("base/later/inner.text", "inner\n");
    let outside = s.dir("outside");
    let victim = s.file("outside/victim.text", "keep\n");
    let sys = s.dir("sys-src");
    s.file("sys-src/file.lisp", "(sys)\n");
    s.link("base/out", &outside);
    s.link("base/out-file", &victim);
    s.link("base/dangling", outside.join("new.text"));
    let fifo = s.fifo("base/pipe");
    let (mut n, _, log) = serve_logged(vec![base(&root), readonly(mount("sys", &sys))]);
    let c = ready(&mut n, LM1, "LISPM", ("I0001", "O0001"), 0);
    let nl = NEWLINE as char;
    let never = || (snapshot(&outside), snapshot(&sys));
    let untouched = never();
    let mut now = 10;
    let world = s.dir.clone();

    // Every command that names a pathname, naming it.
    let every = |p: &str| -> Vec<String> {
        vec![
            format!("I0001 OPEN READ CHARACTER{nl}{p}{nl}"),
            format!(" OPEN PROBE CHARACTER{nl}{p}{nl}"),
            format!("O0001 OPEN WRITE CHARACTER{nl}{p}{nl}"),
            format!("I0001 DIRECTORY{nl}{p}/*{nl}"),
            format!("I0001 PROPERTIES{nl}{p}{nl}"),
            format!(" DELETE{nl}{p}{nl}"),
            format!(" RENAME{nl}{p}{nl}/moved.text{nl}"),
            format!(" RENAME{nl}/ok.text{nl}{p}{nl}"),
            format!(" CREATE-DIRECTORY{nl}{p}{nl}"),
            format!(" CREATE-LINK{nl}{p}{nl}/ok.text{nl}"),
            format!(" CREATE-LINK{nl}/made-link{nl}{p}{nl}"),
            format!(" CHANGE-PROPERTIES{nl}{p}{nl}AUTHOR LISPM{nl}"),
            format!(" COMPLETE{nl}/ok.text{nl}{p}/x{nl}"),
        ]
    };
    // The commands that follow a pathname to its end, for a link's own name.
    let following = |p: &str| -> Vec<String> {
        vec![
            format!("I0001 OPEN READ CHARACTER{nl}{p}{nl}"),
            format!(" OPEN PROBE CHARACTER{nl}{p}{nl}"),
            format!("O0001 OPEN WRITE CHARACTER{nl}{p}{nl}"),
            format!("O0001 OPEN WRITE CHARACTER IF-EXISTS APPEND{nl}{p}{nl}"),
            format!("I0001 DIRECTORY{nl}{p}/*{nl}"),
            format!("I0001 PROPERTIES{nl}{p}{nl}"),
            format!(" CREATE-DIRECTORY{nl}{p}{nl}"),
            format!(" CREATE-LINK{nl}/made-link{nl}{p}{nl}"),
            format!(" CHANGE-PROPERTIES{nl}{p}{nl}AUTHOR LISPM{nl}"),
            format!(" COMPLETE{nl}/ok.text{nl}{p}/x{nl}"),
        ]
    };

    // `..` and `.` at every depth, and a temporary's name.
    let temporary = temporary_name(LM1);
    let climbs = [
        "/..".to_string(),
        "/../outside/victim.text".into(),
        "/later/..".into(),
        "/later/../../outside".into(),
        "/./ok.text".into(),
        "/sys/..".into(),
        "/sys/../ok.text".into(),
        format!("/{temporary}"),
        format!("/later/{temporary}"),
    ];
    for p in &climbs {
        for cmd in every(p) {
            refused(&mut n, c, &mut now, &world, "ATD", &cmd);
        }
    }

    // A host's absolute pathname lands under the root.
    let own = victim.to_str().unwrap().to_string();
    for p in ["/etc/passwd", own.as_str()] {
        for (code, cmd) in [
            ("FNF", format!("I0001 OPEN READ CHARACTER{nl}{p}{nl}")),
            ("FNF", format!(" OPEN PROBE CHARACTER{nl}{p}{nl}")),
            ("FNF", format!("I0001 PROPERTIES{nl}{p}{nl}")),
            ("FNF", format!(" DELETE{nl}{p}{nl}")),
            ("DNF", format!("O0001 OPEN WRITE CHARACTER{nl}{p}{nl}")),
            ("DNF", format!("I0001 DIRECTORY{nl}{p}/*{nl}")),
        ] {
            refused(&mut n, c, &mut now, &world, code, &cmd);
        }
    }

    // A link under the root that leads out of it, or nowhere: through it,
    // and to it.
    for p in ["/out/victim.text", "/out/new.text", "/out/a/b", "/dangling/x", "/out-file/x"] {
        for cmd in every(p) {
            refused(&mut n, c, &mut now, &world, "ATD", &cmd);
        }
    }
    for p in ["/out", "/out-file", "/dangling"] {
        for cmd in following(p) {
            refused(&mut n, c, &mut now, &world, "ATD", &cmd);
        }
    }

    // A link the client makes.
    let cmd = format!(" CREATE-LINK{nl}/made{nl}/../outside/victim.text{nl}");
    refused(&mut n, c, &mut now, &world, "ATD", &cmd);
    accepted(&mut n, c, &mut now, &format!(" CREATE-LINK{nl}/to-sys{nl}/sys/file.lisp{nl}"));
    accepted(&mut n, c, &mut now, &format!(" CREATE-LINK{nl}/to-host{nl}{own}{nl}"));
    for p in ["/to-sys", "/to-host"] {
        for cmd in following(p) {
            refused(&mut n, c, &mut now, &world, "ATD", &cmd);
        }
    }

    // A directory swapped for a link, between two commands.
    accepted(&mut n, c, &mut now, &format!(" RENAME{nl}/later{nl}/was-later{nl}"));
    accepted(&mut n, c, &mut now, &format!(" CREATE-LINK{nl}/later{nl}/sys{nl}"));
    for cmd in [
        format!("O0001 OPEN WRITE CHARACTER{nl}/later/new.text{nl}"),
        format!("I0001 OPEN READ CHARACTER{nl}/later/file.lisp{nl}"),
        format!("I0001 DIRECTORY{nl}/later/*{nl}"),
        format!(" DELETE{nl}/later/file.lisp{nl}"),
        format!(" CREATE-DIRECTORY{nl}/later/made{nl}"),
        format!(" RENAME{nl}/ok.text{nl}/later/ok.text{nl}"),
        format!(" RENAME{nl}/later/file.lisp{nl}/stolen.lisp{nl}"),
    ] {
        refused(&mut n, c, &mut now, &world, "ATD", &cmd);
    }
    // And in the middle of a write.
    accepted(
        &mut n,
        c,
        &mut now,
        &format!("O0001 OPEN WRITE CHARACTER{nl}/was-later/new.text{nl}"),
    );
    n.send_data(c, now, file::CHARACTER_OP, b"written");
    accepted(&mut n, c, &mut now, &format!(" RENAME{nl}/was-later{nl}/moved-later{nl}"));
    accepted(&mut n, c, &mut now, &format!(" CREATE-LINK{nl}/was-later{nl}/sys{nl}"));
    n.send_data(c, now, file::SYNC_MARK_OP, &[]);
    let r = n.command(c, now, "TW O0001 CLOSE");
    assert!(r.starts_with("TW O0001 ERROR MSC F "), "{r:?}");
    assert_eq!(
        temporaries(&root.join("moved-later")).len(),
        1,
        "the temporary stays where it was made, for the next start to remove"
    );

    // A FIFO.
    if fifo {
        for cmd in [
            format!("I0001 OPEN READ CHARACTER{nl}/pipe{nl}"),
            format!(" OPEN PROBE CHARACTER{nl}/pipe{nl}"),
            format!("O0001 OPEN WRITE CHARACTER IF-EXISTS SUPERSEDE{nl}/pipe{nl}"),
        ] {
            refused(&mut n, c, &mut now, &world, "WKF", &cmd);
        }
    } else {
        eprintln!("skipped the FIFO: mkfifo is not available");
    }

    assert_eq!(never(), untouched, "outside, and in the read-only mount, nothing changed");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "3050 create-link /to-sys to /sys/file.lisp".to_string(),
            format!("3050 create-link /to-host to {own}"),
            "3050 rename /later to /was-later".into(),
            "3050 create-link /later to /sys".into(),
            "3050 rename /was-later to /moved-later".into(),
            "3050 create-link /was-later to /sys".into(),
        ]
    );
}
