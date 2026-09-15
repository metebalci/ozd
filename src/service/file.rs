// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The FILE service: the Chaosnet file protocol, as the band's file
//! server spoke it.
//!
//! **The specification is in the release**, and an earlier note here said
//! it was not. `sys/doc/chfile.text`, 792 lines, "Description of the CHAOS
//! FILE protocol designed by HIC": the control connection to contact
//! `FILE`, the `tid <sp> [fh] <sp> cmd [args]` command form, the newline as
//! `NL = 215`, the opcodes `%CODAT = 200` for ASCII, `300` for binary,
//! `201` and `202` for the synchronous and asynchronous marks, `%COEOF =
//! 014`, the error form and its code table. The manual points at `SYS:
//! DOC; FILE TEXT` and the release ships it as `CHFILE TEXT`, which is why
//! it was looked for and not found. It is byte for byte the same file in
//! System 304, two independently restored releases.
//!
//! **And the release carries MIT's own server**, `sys/file/server.lisp`,
//! 1,253 lines: `(chaos:listen "FILE")` and a dispatch of LOGIN, OPEN,
//! OPEN-FOR-LISPM, DATA-CONNECTION, CLOSE, FILEPOS, DELETE, RENAME,
//! EXPUNGE, COMPLETE, CONTINUE, DIRECTORY, CHANGE-PROPERTIES,
//! CREATE-DIRECTORY and CREATE-LINK. So there are three ends to read
//! against each other, not two, and the third is MIT's own.
//!
//! What this is read from, in the order the project ranks them: MIT's
//! specification above, MIT's server, the Lisp Machine's client
//! `sys/network/chaos/qfile.lisp`, and last the MIT/Symbolics Unix server
//! of 1984, `FILE.c`.
//!
//! **The old question about the host had no answer because it had a false
//! premise.** It asked whether the machine at Chaosnet address 3060 ran
//! `FILE.c`, and took `sys/man/pathnm.text`'s "where OZ is a TOPS-20"
//! against `sys/site/hosts.text`'s `HOST MIT-OZ, CHAOS 3060,SERVER,UNIX,
//! VAX,[OZ]` as the release contradicting itself. It does not. The
//! release's own `README`, under "Network Changes", says "] OZ is not
//! really MIT-OZ --- MIT-OZ is identifies now a Unix machine (instead of
//! TOPS-20), and has a new Chaosnet address." The manual describes MIT's
//! historical machine and the host table describes the restoration's
//! stand-in, which are different machines, and 3060 is the restorers'
//! address. No MIT machine was ever there. `sys/site/site.lisp` confirms
//! what the band expects of it: `OZ-SYS-PATHNAME-TRANSLATIONS` is Unix,
//! `("SYS" "//TREE//SYS//")`, which here is the mount `tree`
//! (`docs/design.md` §8).
//!
//! The shape: the user end opens a *control connection* to contact `FILE
//! 1` and sends commands on it as data packets of text, one command a
//! packet, `tid handle COMMAND args`, lines separated by the Lisp
//! Machine's newline. The server answers each with `tid handle COMMAND
//! results`, or `tid handle ERROR code severity message`. Files move on
//! *data connections* the user end listens for and the server calls: a
//! `DATA-CONNECTION` command names an input and an output handle, the
//! server opens a connection to the output handle's name as a contact,
//! and from then on the input handle is that connection's server-to-user
//! direction. `OPEN READ` on an input handle streams the file down it as
//! data packets, then EOF; `CLOSE` is answered on the control connection
//! and followed by a *synchronous mark* on the data connection, which is
//! what the user end reads until.
//!
//! **Containment** is not the protocol's but this server's, since this
//! FILE answers anyone who reaches it (`docs/design.md` §6):
//!
//! - **Every pathname is resolved in the [`Tree`]**, never against a root
//!   directory directly: a read through [`Tree::resolve`], a write
//!   through [`Tree::resolve_for_writing`], DELETE and RENAME as entries,
//!   which act on a link itself ([`Tree::resolve_entry_for_writing`]).
//!   Nothing outside a root is opened, listed, described, renamed or
//!   removed.
//! - **No allowlist**: `LOGIN` records a user name, and is never a
//!   credential.
//! - **A read-only root refuses every command that writes**, `ATF`,
//!   before anything is touched: the tree refuses the pathname first.
//! - **Only a regular file or a directory is opened**, and anything else
//!   is refused, `WKF`, before it is (`openable`).
//! - **DIRECTORY describes each entry through the tree**, never with
//!   `metadata`, which follows a link out of a root (`Control::describe`).
//! - **A write's temporary is made once**, new, and written through the
//!   handle it was made with, never reopened by name.
//! - **Each change to a root is reported** through the [`LogHook`] the
//!   service is given, with its pathname (`docs/design.md` §10).

use crate::lispm::{NEWLINE, from_bytes, from_lispm, lispm_text, to_lispm};
use crate::log::Names;
use crate::ncp::{Out, Response, Service, Session};
use crate::packet::MAX_DATA;
use crate::roots::{Place, Refusal, Resolved, Tree, is_temporary, temporary_name};
use std::collections::{BTreeMap, VecDeque};
use std::io::{ErrorKind, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The contact name. The band asks for `FILE 1`, the `1` being the
/// protocol version, which arrives as the RFC's argument.
pub const CONTACT: &str = "FILE";
/// Data packet opcodes on a data connection, `qfile.lisp`: character
/// data is plain `DAT`, binary `DAT + 100`, a synchronous mark `DAT + 1`,
/// an asynchronous mark, meaning an error, `DAT + 2`.
pub const CHARACTER_OP: u8 = 0o200;
pub const BINARY_OP: u8 = 0o300;
pub const SYNC_MARK_OP: u8 = 0o201;
pub const ASYNC_MARK_OP: u8 = 0o202;
/// The first two 16-bit words of a compiled file, `QFASL` in sixbit:
/// `FILE.c`'s `QBIN1` 0143150 and `QBIN2` 071660, each low byte first ---
/// `68 c6 b0 73`, which is what `sys/sys/cadrlp.qfasl` in the release
/// begins with.
pub const QFASL_MAGIC: [u8; 4] = [0o150, 0o306, 0o260, 0o163];

/// `ATD`, the band's `INCORRECT-ACCESS-TO-DIRECTORY`, as the tree gives it
/// for every pathname outside a root; here it is for what the tree allows
/// and FILE still cannot do: make a write's temporary, or link to `/`
/// itself.
fn denied() -> Refusal {
    ("ATD", "Access to directory denied".into())
}

/// `WKF`, the band's `WRONG-KIND-OF-FILE` (`sys/io/file/open.lisp:260`),
/// for what is neither a regular file nor a directory (see [`openable`]).
fn wrong_kind() -> Refusal {
    ("WKF", "Not a regular file".into())
}

/// What an asynchronous mark carries: `TIDNO <handle> ERROR <code> R
/// <message>`, as `FILE.c`'s `fherror` writes it, the literal `TIDNO`
/// standing where a transaction id would be --- cut to what a packet
/// carries, since a mark is one packet, and the handle is the client's to
/// make as long as a `DATA-CONNECTION` command holds.
pub fn async_mark_data(handle: &str, code: &str, message: &str) -> Vec<u8> {
    let mut data = lispm_text(&format!("TIDNO {handle} ERROR {code} R {message}"));
    data.truncate(MAX_DATA);
    data
}

/// Where the service reports each change it makes to a root: one line, the
/// client as [`File::names`] names it, what was done, and its pathname as
/// the client wrote it, as in `3050 (MIT-LISPM-1) write /tmp/x.text`, for
/// the daemon to put in its log (`docs/design.md` §10). The changes are a
/// write's rename into place (`write`), `rename`, `delete`,
/// `create-directory`, `create-link` and `change-properties`. Shared by
/// every control connection's session, so an `Arc`; `Send` and `Sync`, as a
/// session is `Send`.
pub type LogHook = crate::log::Hook;

/// The service: the [`Tree`], served as the server's `/`.
pub struct File {
    tree: Arc<Tree>,
    /// A fixed universal time to date things by, or the machine's clock.
    time: Option<u32>,
    /// Where each change to a root is reported, if anywhere.
    log: Option<LogHook>,
    /// `--log-file`: report what is served as well as what is changed
    /// --- each file read, each directory listed, each `LOGIN`
    /// (`docs/design.md` §10).
    pub log_file: bool,
    /// `--log-file-probe`: report each `PROBE` too.
    pub log_file_probe: bool,
    /// The site's host table, for the name each line gives the client
    /// after its address (`docs/design.md` §10). Empty unless set, and then
    /// every client is `(?)`.
    pub names: Arc<Names>,
}

impl File {
    /// A service over `tree`, answering **every** host that reaches it
    /// (`docs/design.md` §6). Each change to a root is reported through `log`,
    /// if given; what is served is reported too where [`File::log_file`]
    /// and [`File::log_file_probe`] say so.
    pub fn new(tree: Arc<Tree>, log: Option<LogHook>) -> File {
        File {
            tree,
            time: None,
            log,
            log_file: false,
            log_file_probe: false,
            names: Arc::default(),
        }
    }

    /// Dates by `universal` instead of the machine's clock, if given.
    pub fn with_time(mut self, universal: Option<u32>) -> File {
        self.time = universal;
        self
    }
}

/// The date now --- `time` if fixed, else the machine's clock --- in the
/// same form as a file's.
fn now_date(time: Option<u32>) -> String {
    let secs = match time {
        Some(t) => (t as u64).saturating_sub(crate::service::time::UNIX_EPOCH_UNIVERSAL),
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{m:02}/{d:02}/{:02} {hh:02}:{mm:02}:{ss:02}", y % 100)
}

impl Service for File {
    fn contact(&self) -> &str {
        CONTACT
    }
    fn request(&mut self, _now: u64, args: &str, from: (u16, u16)) -> Response {
        let version = args.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(1);
        Response::Accept(Box::new(Control::new(self, from.0, version)))
    }
}

/// A data connection as the control connection sees it: the channel
/// behind a pair of file handles. Both ends of one connection share it,
/// so what the user end sends up reaches the control connection's
/// transfer, and what a transfer produces goes down.
#[derive(Default)]
struct Channel {
    /// The connection opened: its OPN came back.
    open: bool,
    /// The connection closed: the client's CLS or LOS, or the NCP giving it
    /// up. Closed before it opened, its `DATA-CONNECTION` could not be made.
    closed: bool,
    /// What is to go down it, in order.
    out: VecDeque<Out>,
    /// What has come up it and not been taken yet: opcode and bytes.
    incoming: VecDeque<(u8, Vec<u8>)>,
    /// Whether the user end has sent its end-of-data.
    eof: bool,
}

/// The data connection's own end: shares the channel with the control
/// connection that asked for it.
struct DataSession(Arc<Mutex<Channel>>);

impl Session for DataSession {
    fn opened(&mut self, _now: u64) {
        self.0.lock().unwrap().open = true;
    }
    fn data(&mut self, _now: u64, op: u8, bytes: &[u8]) {
        self.0.lock().unwrap().incoming.push_back((op, bytes.to_vec()));
    }
    fn eof(&mut self, _now: u64) {
        self.0.lock().unwrap().eof = true;
    }
    fn closed(&mut self, _now: u64, _reason: &str) {
        self.0.lock().unwrap().closed = true;
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        self.0.lock().unwrap().out.drain(..).collect()
    }
}

/// A `DATA-CONNECTION` waiting for its connection to open before it is
/// answered, as `FILE.c` answers it only once `chopen` has succeeded ---
/// or to close without opening, when it is answered with `FILE.c`'s `NET`.
struct Pending {
    tid: String,
    channel: Arc<Mutex<Channel>>,
}

/// The control connection: one client's session, and nothing of any
/// other's --- its handles, its transfers and its user are its own.
struct Control {
    tree: Arc<Tree>,
    /// A fixed universal time to date things by, or the machine's clock.
    time: Option<u32>,
    /// Where each change to a root is reported, if anywhere.
    log: Option<LogHook>,
    /// `--log-file` and `--log-file-probe`, as the service was given them.
    log_file: bool,
    log_file_probe: bool,
    /// The site's host table, as the service was given it.
    names: Arc<Names>,
    client: u16,
    /// The protocol version from the RFC's argument: `FILE 1` is 1. It
    /// chooses the shape of the reply to a write's `CLOSE` --- `FILE.c`
    /// writes the plain form when `protocol > 0` and one with a leading
    /// `-1` for an older client.
    version: u32,
    /// Who `LOGIN` said this is: shown as a file's author, and never a
    /// credential (the README, "Security").
    user: Option<String>,
    /// Handles by name, each to its channel; the input and output
    /// handles of one data connection share a channel.
    ///
    /// Ordered, and not for the order's own sake: the poll walks these,
    /// and with a hashed map that walk is in a different order in every
    /// process, so anything that depends on which handle comes first
    /// happens in some runs and not others. One such bug took an
    /// afternoon to catch. Ordering it does more than take the variation
    /// away: it makes the order that used to lose data the only order
    /// there is, so a test can hold it and fails every time rather than
    /// half of them.
    handles: BTreeMap<String, Arc<Mutex<Channel>>>,
    /// What each handle has open, for CLOSE: a data connection can be
    /// reading on its input handle and writing on its output handle at
    /// the same time.
    transfers: BTreeMap<String, Transfer>,
    pending: Vec<Pending>,
    out: VecDeque<Out>,
}

enum Transfer {
    /// A file being read: its truename and the properties line, repeated
    /// in the CLOSE reply.
    Read {
        truename: String,
        properties: String,
        /// The file as it goes down the wire, kept so that a `FILEPOS`
        /// can send it again from somewhere else, and the opcode it goes
        /// under.
        contents: Vec<u8>,
        op: u8,
    },
    Directory,
    /// A file being written: the temporary it is going into, where it
    /// will be renamed to on close, and how to translate what arrives.
    Write {
        /// The temporary, open: made new by the `OPEN` that resolved its
        /// directory, and written through this handle alone --- never
        /// opened again by name (`docs/design.md` §6, "FILE's rules").
        file: std::fs::File,
        temp: PathBuf,
        real: PathBuf,
        /// The pathname as the client wrote it, for the log.
        pathname: String,
        truename: String,
        characters: bool,
        /// Bytes a failed write is holding, waiting for a `CONTINUE`, and
        /// the message that went out in the asynchronous mark. While this
        /// is set the transfer is stopped.
        stalled: Option<(Vec<u8>, String)>,
        /// Whether the synchronous mark that says the data is all there
        /// has come up the data connection. A flag rather than a count
        /// because a write's only inbound mark is that one: the others
        /// the protocol has are the ones a `FILEPOS` or a
        /// `SET-BYTE-SIZE` provokes, and both of those come *from* the
        /// server.
        marked: bool,
        /// The transaction id of a `CLOSE` that came before the mark and
        /// is waiting for it.
        closing: Option<String>,
        /// Deleted while open: the temporary is gone, and the `CLOSE`
        /// puts nothing in place (`Control::delete_while_open`).
        deleted: bool,
    },
}

/// What an `OPEN WRITE` was asked for: its option words, the pathname
/// as written and the place it resolved to, and the two that decide the
/// character translation.
struct WriteOpen<'a> {
    words: &'a [&'a str],
    pathname: &'a str,
    place: Place,
    binary: bool,
    default: bool,
}

/// A parsed command: `tid handle COMMAND args`, then the further lines.
struct Command<'a> {
    tid: &'a str,
    handle: &'a str,
    name: &'a str,
    args: &'a str,
    lines: Vec<&'a str>,
}

fn parse(text: &str) -> Option<Command<'_>> {
    let mut lines = text.split(NEWLINE as char);
    let first = lines.next()?;
    let (tid, rest) = first.split_once(' ')?;
    // Two spaces where there is no handle.
    let (handle, rest) = match rest.strip_prefix(' ') {
        Some(r) => ("", r),
        None => rest.split_once(' ')?,
    };
    let (name, args) = match rest.split_once(' ') {
        Some((n, a)) => (n, a),
        None => (rest, ""),
    };
    Some(Command { tid, handle, name, args, lines: lines.collect() })
}

impl Control {
    /// A session for `client`, at protocol `version`, with what the service
    /// `file` was given.
    fn new(file: &File, client: u16, version: u32) -> Control {
        Control {
            tree: file.tree.clone(),
            time: file.time,
            log: file.log.clone(),
            log_file: file.log_file,
            log_file_probe: file.log_file_probe,
            names: file.names.clone(),
            client,
            version,
            user: None,
            handles: BTreeMap::new(),
            transfers: BTreeMap::new(),
            pending: Vec::new(),
            out: VecDeque::new(),
        }
    }

    /// `tid handle COMMAND results`, `FILE.c`'s `respond`.
    fn reply(&mut self, tid: &str, handle: &str, name: &str, results: &str) {
        let mut line = format!("{tid} {handle} {name}");
        if !results.is_empty() {
            line.push(' ');
            line.push_str(results);
        }
        self.say(&line);
    }

    /// A line down the control connection, in as many packets as it
    /// takes: the connection is a stream, and a reply quoting a command
    /// back --- an unknown command's name, a `LOGIN`'s user in its home
    /// directory --- may run past the [`MAX_DATA`] bytes a packet carries.
    fn say(&mut self, line: &str) {
        for chunk in lispm_text(line).chunks(MAX_DATA) {
            self.out.push_back(Out::Data(chunk.to_vec()));
        }
    }

    /// `tid handle ERROR code severity message`, `FILE.c`'s `error`: the
    /// severity `C` for an error in the command, `F` fatal to a transfer,
    /// `R` recoverable.
    fn error(&mut self, tid: &str, handle: &str, code: &str, severity: char, message: &str) {
        let line = format!("{tid} {handle} ERROR {code} {severity} {message}");
        self.say(&line);
    }

    /// Reports a change this session made to a root, `what` and its
    /// pathname, through the service's [`LogHook`] (`docs/design.md` §10).
    fn changed(&self, what: &str) {
        self.note(what);
    }

    /// Reports what this session served --- a `LOGIN`, a file read, a
    /// directory listed --- where `--log-file` asks for it (`docs/design.md`
    /// §10).
    fn served(&self, what: &str) {
        if self.log_file {
            self.note(what);
        }
    }

    /// Reports a `PROBE`, where `--log-file-probe` asks for it: a band probes
    /// far more often than it reads, and serves no file by it, so it is
    /// asked for on its own.
    fn probed(&self, what: &str) {
        if self.log_file_probe {
            self.note(what);
        }
    }

    /// One line for the log: which client, and `what` it did.
    fn note(&self, what: &str) {
        if let Some(log) = &self.log {
            log(&format!("{} {what}", self.names.host(self.client)));
        }
    }

    fn command(&mut self, text: &[u8]) {
        let text = from_bytes(text);
        let Some(c) = parse(&text) else {
            return;
        };
        let (tid, handle) = (c.tid.to_string(), c.handle.to_string());
        match c.name {
            "LOGIN" => {
                let user = c.args.split_whitespace().next().unwrap_or("").to_string();
                if user.is_empty() {
                    self.error(&tid, &handle, "UNK", 'C', "Unknown user");
                    return;
                }
                // `FILE.c`: the name, the home directory with a slash, the
                // full name; the user end takes the home directory and the
                // personal name off the two lines.
                let home = format!("/{}/", user.to_lowercase());
                let results = format!("{user} {home}{}{user}{}", NEWLINE as char, NEWLINE as char);
                self.served(&format!("login {user}"));
                self.user = Some(user);
                self.reply(&tid, &handle, "LOGIN", &results);
            }
            "DATA-CONNECTION" => {
                let mut a = c.args.split_whitespace();
                let (Some(input), Some(output)) = (a.next(), a.next()) else {
                    self.error(&tid, &handle, "BUG", 'C', "DATA-CONNECTION wants two handles");
                    return;
                };
                if self.handles.contains_key(input) || self.handles.contains_key(output) {
                    self.error(&tid, &handle, "BUG", 'C', "File handle already exists");
                    return;
                }
                let channel = Arc::new(Mutex::new(Channel::default()));
                self.handles.insert(input.to_string(), channel.clone());
                self.handles.insert(output.to_string(), channel.clone());
                // "The output file handle name is the contact name the user
                // end is listening for, so send it."
                self.out.push_back(Out::Connect {
                    host: self.client,
                    contact: output.to_string(),
                    session: Box::new(DataSession(channel.clone())),
                });
                self.pending.push(Pending { tid, channel });
            }
            "UNDATA-CONNECTION" => {
                // Both handles of the data connection go, and the
                // transfers on both of them: "UNDATA-CONNECTION implies
                // a CLOSE on each file handle of the DATA connection for
                // which there is a file transfer in progress". The
                // client names the input handle --- `qfile.lisp` sends
                // `(DATA-INPUT-HANDLE DATA-CONN)` --- and a write is on
                // the output one, so taking only the named handle's
                // transfer would leave a write behind, and its temporary
                // in the directory.
                let mut going = vec![handle.clone()];
                if let Some(ch) = self.handles.remove(&handle) {
                    ch.lock().unwrap().out.push_back(Out::Close("Undata".into()));
                    going.extend(
                        self.handles
                            .iter()
                            .filter(|(_, v)| Arc::ptr_eq(v, &ch))
                            .map(|(k, _)| k.clone()),
                    );
                    self.handles.retain(|_, v| !Arc::ptr_eq(v, &ch));
                }
                for h in going {
                    if let Some(Transfer::Write { temp, closing, .. }) = self.transfers.remove(&h) {
                        let _ = std::fs::remove_file(&temp);
                        self.stranded(&h, closing);
                    }
                }
                self.reply(&tid, &handle, "UNDATA-CONNECTION", "");
            }
            "OPEN" => self.open(&tid, &handle, c.args, c.lines.first().copied().unwrap_or("")),
            "DIRECTORY" => {
                self.directory(&tid, &handle, c.args, c.lines.first().copied().unwrap_or(""))
            }
            "CLOSE" => self.close(&tid, &handle),
            "DELETE" => self.delete(&tid, &handle, c.lines.first().copied().unwrap_or("")),
            "RENAME" => self.rename(
                &tid,
                &handle,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            "CREATE-DIRECTORY" => {
                self.create_directory(&tid, &handle, c.lines.first().copied().unwrap_or(""))
            }
            "CREATE-LINK" => self.create_link(
                &tid,
                &handle,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            // `FILE.c`'s `expunge` answers with the number of blocks it
            // recovered, and on Unix, where a delete is a delete, that is
            // always none.
            "EXPUNGE" => self.reply(&tid, &handle, "EXPUNGE", "0"),
            "CHANGE-PROPERTIES" => self.change_properties(&tid, &handle, &c.lines),
            "COMPLETE" => self.complete(
                &tid,
                &handle,
                c.args,
                c.lines.first().copied().unwrap_or(""),
                c.lines.get(1).copied().unwrap_or(""),
            ),
            // Position within a transfer: nothing here reads a file in
            // pieces, so the only position that can be asked for is the
            // one it is already at.
            "FILEPOS" => self.filepos(&tid, &handle, c.args.trim()),
            "PROPERTIES" => self.properties(&tid, &handle, c.lines.first().copied().unwrap_or("")),
            // `CONTINUE` resumes a transfer the server stopped with a
            // recoverable error, and the client only ever sends it after
            // an **asynchronous mark**: `QFILE-PROCESS-ASYNC-MARK` puts
            // the stream in `:ASYNC-MARKED`, and `:CONTINUE`'s own guard
            // is `(EQ STATUS :ASYNC-MARKED)`, so without a mark it does
            // nothing. A write stopped by a failed write sends one; with
            // none sent, nothing can be continued --- and `FILE.c`
            // answers the same way, "CONTINUE received when not in error
            // state".
            "CONTINUE" => self.cont(&tid, &handle),
            other => {
                self.error(&tid, &handle, "UKC", 'C', &format!("{other} is not served here"));
            }
        }
    }

    /// `OPEN direction mode options`, then the pathname on the next line:
    /// resolved for writing to write, and otherwise for reading; and read
    /// only once it is known to be a regular file or a directory.
    fn open(&mut self, tid: &str, handle: &str, args: &str, pathname: &str) {
        let words: Vec<&str> = args.split_whitespace().collect();
        let direction = words.first().copied().unwrap_or("READ");
        let binary = words.contains(&"BINARY");
        let default = words.contains(&"DEFAULT");
        let given_byte_size = words
            .iter()
            .position(|w| *w == "BYTE-SIZE")
            .and_then(|i| words.get(i + 1))
            .and_then(|n| n.parse::<u32>().ok());
        if direction == "WRITE" {
            let place = match self.tree.resolve_for_writing(pathname) {
                Ok(p) => p,
                Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
            };
            return self.open_write(
                tid,
                handle,
                WriteOpen { words: &words, pathname, place, binary, default },
            );
        }
        let place = match self.tree.resolve(pathname) {
            Ok(Resolved::Place(p)) => p,
            Ok(Resolved::Top) => return self.open_top(tid, handle, direction, pathname),
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let meta = match openable(&place) {
            Ok(Some(m)) => m,
            Ok(None) => return self.error(tid, handle, "FNF", 'C', "File not found"),
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if direction == "PROBE-DIRECTORY" || (direction == "PROBE" && meta.is_dir()) {
            let results = format!(
                "{} 0 NIL{}{}{}",
                date(&meta),
                NEWLINE as char,
                truename(pathname, true),
                NEWLINE as char
            );
            self.probed(&format!("probe {pathname}"));
            self.reply(tid, handle, "OPEN", &results);
            return;
        }
        if meta.is_dir() {
            return self.error(tid, handle, "FNF", 'C', "That is a directory");
        }
        // A regular file, by `openable`: nothing else is read, so neither a
        // FIFO with no writer, which would hold the loop's one thread for
        // ever, nor a device, which would stream without end. PROBE comes
        // here too, reading the whole file to measure it.
        let Ok(contents) = std::fs::read(&place.path) else {
            return self.error(tid, handle, "ATF", 'C', "Access to file denied");
        };
        let qfasl = contents.starts_with(&QFASL_MAGIC);
        // Characters unless the file is compiled, when DEFAULT asks.
        // `FILE.c` decides that first and only then has a byte size:
        // `options |= O_CHARACTER` happens inside the DEFAULT case, and
        // the length is `options & O_CHARACTER || bytesize <= 8 ?
        // st_size : (st_size + 1) / 2` --- bytes for characters, words
        // for binary, so a compiled file opened DEFAULT is measured in
        // words even though nobody said BINARY.
        let characters = if default { !qfasl } else { !binary };
        let byte_size = given_byte_size.unwrap_or(if characters { 8 } else { 16 });
        let length =
            if characters || byte_size <= 8 { contents.len() } else { contents.len().div_ceil(2) };
        // `FILE.c`: date, length, QFASL as T or NIL, and with DEFAULT the
        // characters decision as T or NIL after a space.
        let mut properties =
            format!("{} {} {}", date(&meta), length, if qfasl { "T" } else { "NIL" });
        if default {
            properties.push_str(if characters { " T" } else { " NIL" });
        }
        let tn = truename(pathname, false);
        let results = format!("{properties}{}{tn}{}", NEWLINE as char, NEWLINE as char);
        if direction == "PROBE" {
            self.probed(&format!("probe {pathname}"));
            self.reply(tid, handle, "OPEN", &results);
            return;
        }
        // READ: down the input handle's data connection.
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        self.served(&format!("read {pathname}"));
        self.reply(tid, handle, "OPEN", &results);
        let (op, bytes) =
            if characters { (CHARACTER_OP, to_lispm(&contents)) } else { (BINARY_OP, contents) };
        {
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(op, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(
            handle.to_string(),
            Transfer::Read { truename: tn, properties, contents: bytes, op },
        );
    }

    /// `OPEN` of `/` itself, which the tree resolves to its top and not to
    /// a path (`docs/design.md` §6, "FILE's rules"): a directory, so a PROBE is
    /// answered as one and anything else as opening a directory is. It has
    /// no date of its own --- the base and each mount have theirs --- and
    /// is dated now; what a band makes of that is not seen.
    fn open_top(&mut self, tid: &str, handle: &str, direction: &str, pathname: &str) {
        if direction == "PROBE-DIRECTORY" || direction == "PROBE" {
            let results = format!(
                "{} 0 NIL{}{}{}",
                now_date(self.time),
                NEWLINE as char,
                truename(pathname, true),
                NEWLINE as char
            );
            self.probed(&format!("probe {pathname}"));
            return self.reply(tid, handle, "OPEN", &results);
        }
        self.error(tid, handle, "FNF", 'C', "That is a directory")
    }

    /// `DIRECTORY options`, the pathname on the next line: the listing
    /// goes down the input handle as records of text, `FILE.c`'s
    /// `diropen` and `dirread`: a first record with the file system's
    /// properties, then one a file, each a pathname line, property lines
    /// `NAME value`, and a blank line. Each name is described through the
    /// tree ([`Control::describe`]), never with `metadata`.
    fn directory(&mut self, tid: &str, handle: &str, _args: &str, pathname: &str) {
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        let (dir, pattern) = match pathname.rsplit_once('/') {
            Some((d, p)) => (d.to_string(), p.to_string()),
            None => (String::new(), pathname.to_string()),
        };
        let names = match self.names_in(&dir) {
            Ok(Some(names)) => names,
            Ok(None) => return self.error(tid, handle, "DNF", 'C', "Directory not found"),
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let nl = NEWLINE as char;
        let mut text = String::new();
        text.push(nl);
        text.push_str(&format!("BLOCK-SIZE 1024{nl}"));
        text.push_str(&format!("SETTABLE-PROPERTIES CREATION-DATE AUTHOR{nl}"));
        text.push(nl);
        for name in names.into_iter().filter(|n| matches(&pattern, n)) {
            let shown = format!("{}/{}", dir.trim_end_matches('/'), name);
            let Some(meta) = self.describe(&shown) else { continue };
            text.push_str(&format!("{shown}{nl}"));
            text.push_str(&self.file_properties(&meta));
            text.push(nl);
        }
        self.served(&format!("directory {pathname}"));
        self.reply(tid, handle, "DIRECTORY", "");
        {
            // A byte a character: `text` holds the protocol's newline at
            // 0o215, which `as_bytes` would encode as two.
            let bytes = lispm_text(&text);
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(CHARACTER_OP, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(handle.to_string(), Transfer::Directory);
    }

    /// The names in the directory a pathname names, sorted, as DIRECTORY
    /// and COMPLETE list them: `/`'s from [`Tree::list_top`], the base's
    /// entries and the mounts' names; any other's read from the directory
    /// the tree resolves it to, once it is known to be one. `None` if it is
    /// not a directory or cannot be read. A temporary is not among them: no
    /// pathname can name one ([`is_temporary`]).
    fn names_in(&self, dir: &str) -> Result<Option<Vec<String>>, Refusal> {
        let mut names: Vec<String> = match self.tree.resolve(dir)? {
            Resolved::Top => match self.tree.list_top() {
                Ok(names) => names,
                Err(_) => return Ok(None),
            },
            // Its kind first, and only a directory read: `read_dir` opens
            // what it is given.
            Resolved::Place(p) => match openable(&p) {
                Ok(Some(m)) if m.is_dir() => match std::fs::read_dir(&p.path) {
                    Ok(rd) => {
                        rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
                    }
                    Err(_) => return Ok(None),
                },
                _ => return Ok(None),
            },
        };
        names.retain(|n| !is_temporary(n));
        names.sort();
        Ok(Some(names))
    }

    /// What DIRECTORY says of one name in a listing (`docs/design.md` §6,
    /// "FILE's rules"): what the tree resolves the name to, described as
    /// [`described`] describes it, so that a link in its own root is what it
    /// leads to --- a directory a directory. A name the tree will not
    /// follow --- a link out of its root, into another root, or nowhere ---
    /// is described as the entry itself, with `symlink_metadata`: the
    /// link, never what it leads to, so that it is listed and can be
    /// deleted. `None` for a name gone by the time it is looked at.
    ///
    /// Never with `metadata`, which follows a link, and would give the size
    /// and date of a file outside its root to anyone who listed the link's
    /// directory.
    fn describe(&self, pathname: &str) -> Option<std::fs::Metadata> {
        match self.tree.resolve(pathname) {
            Ok(Resolved::Place(p)) => described(&p),
            Ok(Resolved::Top) => None,
            Err(_) => {
                let entry = self.tree.resolve_entry(pathname).ok()?;
                std::fs::symlink_metadata(entry.path()).ok()
            }
        }
    }

    /// `CLOSE`: answered on the control connection, and for a read
    /// followed by the synchronous mark down the data connection, in
    /// that order --- "we must respond to the close before sending the
    /// SYNCMARK since otherwise we would likely block".
    ///
    /// For a write it is the other way about: the mark is **awaited**,
    /// and only then does the close rename the temporary into place and
    /// answer with the file's date, its length and its truename ---
    /// `FILE.c`'s `xclose`, which writes that plain form when the
    /// protocol version is above zero and one with a leading `-1` for an
    /// older client.
    fn close(&mut self, tid: &str, handle: &str) {
        // Any data that arrived on a data connection but has not yet been
        // moved into its write goes in first: a CLOSE can follow the last
        // data packet with no turn of the server between them. It goes to
        // the **write**, whichever handle this CLOSE names: drained for
        // its own handle, a read's CLOSE would take its sibling write's
        // bytes and drop them.
        self.drain_writes();
        // A write's CLOSE comes up the control connection and its mark
        // up the data connection, and the two can overtake each other:
        // `sys/doc/chfile.text`'s worked example for writing a file
        // sends "a SYNC mark on the DATA connection and a CLOSE on the
        // CONTROL connection (in either order)", so a CLOSE that
        // arrives first is a correct client's doing rather than a
        // fault. The mark is what says the data is all there, and
        // renaming without it would put a file into place short of
        // whatever had not arrived --- empty, if none of it had. So the
        // transfer stays open and the CLOSE is answered from the poll
        // once the mark has come.
        //
        // The client sends the two that way round and does not wait
        // between them: `qfile.lisp`'s `:COMMAND` writes the command
        // packet on the control connection and then, for an output
        // stream, `(SEND STREAM :WRITE-SYNCHRONOUS-MARK)` before it
        // waits for the response. So holding the reply back cannot
        // hold the mark back with it.
        if let Some(Transfer::Write { marked: false, stalled: None, closing, .. }) =
            self.transfers.get_mut(handle)
        {
            *closing = Some(tid.to_string());
            return;
        }
        let Some(transfer) = self.transfers.remove(handle) else {
            return self.error(
                tid,
                handle,
                "BUG",
                'C',
                "No transfer in progress on this file handle",
            );
        };
        match transfer {
            Transfer::Read { truename, properties, .. } => {
                let results =
                    format!("{properties}{}{truename}{}", NEWLINE as char, NEWLINE as char);
                self.reply(tid, handle, "CLOSE", &results);
                if let Some(ch) = self.handles.get(handle) {
                    ch.lock().unwrap().out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
                }
            }
            Transfer::Directory => {
                self.reply(tid, handle, "CLOSE", "");
                if let Some(ch) = self.handles.get(handle) {
                    ch.lock().unwrap().out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
                }
            }
            Transfer::Write { stalled: Some((_, why)), file, temp, .. } => {
                // The file would be short of whatever the stall is
                // holding, so it does not go into place; the error that
                // stopped it is repeated, fatal this time.
                drop(file);
                let _ = std::fs::remove_file(&temp);
                self.error(tid, handle, "NMR", 'F', &why);
            }
            Transfer::Write { file, temp, real, pathname, truename, deleted, .. } => {
                // What was written, measured through the handle it was
                // written through.
                let written = file.metadata();
                drop(file);
                // Deleted while open, its temporary is gone already and
                // nothing is put in place; the CLOSE is answered as ever,
                // as `FILE.c`'s `xclose` answers one.
                //
                // Otherwise the rename is by the paths the OPEN resolved.
                // Should a later command have swapped a directory on the
                // way for a link, the temporary's path moved with it ---
                // the two share every directory --- and where the link
                // leads there is no temporary, which no client can name or
                // make: the rename fails, and nothing is put there (see
                // `roots::temporary_name`).
                if !deleted {
                    if let Err(e) = std::fs::rename(&temp, &real) {
                        let _ = std::fs::remove_file(&temp);
                        return self.error(tid, handle, "MSC", 'F', &e.to_string());
                    }
                    self.changed(&format!("write {pathname}"));
                }
                let (length, when) = match written {
                    Ok(m) => (m.len(), date(&m)),
                    Err(_) => (0, now_date(self.time)),
                };
                let nl = NEWLINE as char;
                let body = format!("{when} {length}{nl}{truename}{nl}");
                let results = if self.version > 0 { body } else { format!("-1 {body}") };
                self.reply(tid, handle, "CLOSE", &results);
            }
        }
    }

    /// Answers a `CLOSE` that was still waiting for its synchronous mark
    /// when the write it was waiting on was taken away by an
    /// `UNDATA-CONNECTION`. `CNO`, "CLOSE on non-open channel", is
    /// `chfile.text`'s code for it: by the time the CLOSE could be answered
    /// there was no longer a channel to close. A `DELETE` on the handle
    /// takes nothing away: the write stays, deleted, for its `CLOSE`.
    ///
    /// The band never gets here. Its `:REAL-CLOSE` waits for the CLOSE's
    /// reply before it frees the data connection, and it only undoes a data
    /// connection that has gone dormant. This is so that a client which
    /// does it the other way round is told, rather than left waiting for a
    /// reply that would never come.
    fn stranded(&mut self, handle: &str, closing: Option<String>) {
        if let Some(tid) = closing {
            self.error(&tid, handle, "CNO", 'C', "The transfer was abandoned before its mark");
        }
    }

    /// `OPEN WRITE`, then the pathname: a temporary file beside the real
    /// one, which `CLOSE` renames into place --- `FILE.c` creates
    /// `tempfile(dirname)` and links it over the real name on close,
    /// "we know that both names are in the same directory".
    ///
    /// `IF-EXISTS` and `IF-DOES-NOT-EXIST` say what to do about what is
    /// there; the client sends them by name. `NEW-VERSION` is what a
    /// versioned file system does and this one has no versions, so it is
    /// `SUPERSEDE` here, which is what `FILE.c` turns it into.
    fn open_write(&mut self, tid: &str, handle: &str, o: WriteOpen<'_>) {
        let WriteOpen { words, pathname, place, binary, default } = o;
        let named = |key: &str| {
            words.iter().position(|w| *w == key).and_then(|i| words.get(i + 1)).copied()
        };
        let if_exists = named("IF-EXISTS").unwrap_or("NEW-VERSION");
        let if_missing = named("IF-DOES-NOT-EXIST").unwrap_or("CREATE");
        // What is there already, its kind read without opening it.
        let existing = openable(&place);
        let exists = !matches!(existing, Ok(None));
        if exists {
            match if_exists {
                "ERROR" => {
                    return self.error(tid, handle, "FAE", 'C', "File already exists");
                }
                "NEW-VERSION" | "SUPERSEDE" | "RENAME" | "RENAME-AND-DELETE" | "TRUNCATE"
                | "OVERWRITE" | "APPEND" => {}
                other => {
                    return self.error(
                        tid,
                        handle,
                        "UOO",
                        'C',
                        &format!("{other} is not a way to write an existing file"),
                    );
                }
            }
        } else if if_missing == "ERROR" {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        // The tree never gives a root itself to write, so the place has a
        // directory, and it is the root or lies in it; canonical, so it is
        // read here without following anything.
        let Some(dir) = place.path.parent() else {
            return self.error(tid, handle, "DNF", 'C', "Directory not found");
        };
        if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
            return self.error(tid, handle, "DNF", 'C', "Directory not found");
        }
        if !self.handles.contains_key(handle) {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        }
        // A FIFO or a device where the file would be is not a regular
        // file to overwrite, and reading it for APPEND or OVERWRITE would
        // block or exhaust the loop's one thread: refused before a
        // temporary is made (see [`openable`]). So is a directory.
        match &existing {
            Err((code, msg)) => return self.error(tid, handle, code, 'C', msg),
            Ok(Some(m)) if !m.is_file() => {
                let (code, msg) = wrong_kind();
                return self.error(tid, handle, code, 'C', &msg);
            }
            _ => {}
        }
        // The temporary: made once, here, in the directory the tree just
        // resolved, as a new file --- `create_new` opens no name that is
        // there, a link included --- under a name no other write takes and
        // no client can name (`roots::temporary_name`); then written
        // through this handle alone, never opened again by name
        // (`docs/design.md` §6, "FILE's rules"). A name a crash left, which the
        // count starting again at 0 can meet, is stepped past, as the
        // tree's probe steps past one.
        let mut made = None;
        for _ in 0..16 {
            let temp = dir.join(temporary_name(self.client));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(f) => {
                    made = Some((f, temp));
                    break;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
        let Some((mut file, temp)) = made else {
            let (code, msg) = denied();
            return self.error(tid, handle, code, 'C', &msg);
        };
        // What is already there, for APPEND, and to start from for
        // OVERWRITE; otherwise the temporary starts empty. A regular
        // file, by `openable`.
        let start = match if_exists {
            "APPEND" | "OVERWRITE" if exists => std::fs::read(&place.path).unwrap_or_default(),
            _ => Vec::new(),
        };
        if file.write_all(&start).is_err() {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            let (code, msg) = denied();
            return self.error(tid, handle, code, 'C', &msg);
        }
        let characters = if default { true } else { !binary };
        let tn = truename(pathname, false);
        // The reply is the same shape as a read's: the file's date, its
        // length, whether it is compiled. Nothing is written yet.
        let results = format!(
            "{} {} NIL{}{tn}{}",
            now_date(self.time),
            start.len(),
            NEWLINE as char,
            NEWLINE as char
        );
        self.reply(tid, handle, "OPEN", &results);
        self.transfers.insert(
            handle.to_string(),
            Transfer::Write {
                file,
                temp,
                real: place.path,
                pathname: pathname.to_string(),
                truename: tn,
                characters,
                stalled: None,
                marked: false,
                closing: None,
                deleted: false,
            },
        );
    }

    /// What has come up the data connections, taken in by the write in
    /// progress on each: `drain_incoming` for every handle that is writing,
    /// and for no other. Both handles of a connection share one channel,
    /// so draining the handle that is writing takes it all, and a drain for
    /// the other would take the write's bytes and drop them (`poll`).
    fn drain_writes(&mut self) {
        let writing: Vec<String> = self
            .handles
            .keys()
            .filter(|h| matches!(self.transfers.get(*h), Some(Transfer::Write { .. })))
            .cloned()
            .collect();
        for handle in writing {
            self.drain_incoming(&handle);
        }
    }

    /// Takes everything waiting on a handle's data connection and gives it
    /// to the write in progress on that handle, in order. A handle with no
    /// transfer has its waiting data discarded --- there is nowhere for it
    /// to go, and keeping it would grow without bound.
    fn drain_incoming(&mut self, handle: &str) {
        let Some(ch) = self.handles.get(handle) else {
            return;
        };
        let items: Vec<(u8, Vec<u8>)> = ch.lock().unwrap().incoming.drain(..).collect();
        let writing = matches!(self.transfers.get(handle), Some(Transfer::Write { .. }));
        for (op, bytes) in items {
            // A synchronous mark says the data is all there; it carries
            // none of its own, and it is what a CLOSE waits for.
            if op == SYNC_MARK_OP {
                if let Some(Transfer::Write { marked, .. }) = self.transfers.get_mut(handle) {
                    *marked = true;
                }
                continue;
            }
            if op == ASYNC_MARK_OP {
                continue;
            }
            if writing {
                self.wrote(handle, op, &bytes);
            }
        }
    }

    /// Data that has come up a data connection: written to whatever the
    /// handle is writing, through the temporary's own handle, translated
    /// out of the Lisp Machine character set if it is characters.
    fn wrote(&mut self, handle: &str, op: u8, bytes: &[u8]) {
        let Some(Transfer::Write { file, characters, stalled, .. }) =
            self.transfers.get_mut(handle)
        else {
            return;
        };
        let out = if *characters && op != BINARY_OP { from_lispm(bytes) } else { bytes.to_vec() };
        // Already stopped: the client should have stopped sending, but
        // whatever arrives joins what is waiting rather than being lost.
        if let Some((held, _)) = stalled.as_mut() {
            held.extend_from_slice(&out);
            return;
        }
        if let Err(e) = file.write_all(&out) {
            let why = e.to_string();
            *stalled = Some((out, why.clone()));
            self.async_mark(handle, "NMR", &why);
        }
    }

    /// Stops a transfer with a **recoverable** error, AIM-628's
    /// `E_RECOVERABLE`: an asynchronous mark down the data connection,
    /// which `FILE.c`'s `fherror` writes as `TIDNO <handle> ERROR <code>
    /// R <message>`, the literal `TIDNO` standing where a transaction id
    /// would be. The client's `QFILE-PROCESS-ASYNC-MARK` strips that
    /// first word, shows the error as proceedable, and sends `CONTINUE`
    /// if the user proceeds.
    fn async_mark(&mut self, handle: &str, code: &str, message: &str) {
        if let Some(ch) = self.handles.get(handle) {
            let data = async_mark_data(handle, code, message);
            ch.lock().unwrap().out.push_back(Out::DataOp(ASYNC_MARK_OP, data));
        }
    }

    /// `CONTINUE`: retries what the stalled write is holding.
    ///
    /// `FILE.c` answers the command and lets the transfer retry after ---
    /// `filecontinue` sets `X_RETRY` and responds --- so the reply says
    /// only that the command was understood, and a retry that fails
    /// again raises another mark. With no transfer, or one that is not
    /// stopped, it is the server's own `BUG`, "CONTINUE received when
    /// not in error state".
    ///
    /// **Not reached by a test here.** Taking a write's temporary away
    /// between two packets fails nothing, since the write goes through its
    /// own handle; what stops one is the disk itself failing, full or
    /// erring.
    fn cont(&mut self, tid: &str, handle: &str) {
        let stopped =
            matches!(self.transfers.get(handle), Some(Transfer::Write { stalled: Some(_), .. }));
        if handle.is_empty() || !self.transfers.contains_key(handle) {
            return self.error(tid, handle, "BUG", 'C', "No transfer to continue");
        }
        if !stopped {
            return self.error(
                tid,
                handle,
                "BUG",
                'C',
                "CONTINUE received when not in error state",
            );
        }
        self.reply(tid, handle, "CONTINUE", "");
        let Some(Transfer::Write { file, stalled, .. }) = self.transfers.get_mut(handle) else {
            return;
        };
        let (held, _) = stalled.take().expect("stopped");
        if let Err(e) = file.write_all(&held) {
            let why = e.to_string();
            *stalled = Some((held, why.clone()));
            self.async_mark(handle, "NMR", &why);
        }
    }

    /// `DELETE`: on a handle, the file being read or written on it
    /// (`Control::delete_while_open`); with a pathname and no handle, what
    /// the pathname names (`Control::delete_entry`).
    fn delete(&mut self, tid: &str, handle: &str, pathname: &str) {
        if handle.is_empty() {
            self.delete_entry(tid, handle, pathname)
        } else {
            self.delete_while_open(tid, handle, pathname)
        }
    }

    /// `DELETE` on a handle, a "delete while open": the file being read
    /// or written on the handle "will be deleted after we close it
    /// (regardless of direction)" (`sys/doc/chfile.text`, DELETE). As
    /// `FILE.c`'s `delete` does it, at once: a write's temporary, and the
    /// `CLOSE` then puts nothing in place and is answered as ever; a read's
    /// file, removed as a pathname's `DELETE` removes it --- through the
    /// tree, so a read-only root refuses it, `ATF`, and the change is
    /// reported --- while the read goes on to its `CLOSE`. The band's
    /// `:REAL-CLOSE` sends one when it aborts a write, and then the
    /// `CLOSE`; its `:DELETE` sends one on any open stream.
    ///
    /// Refused, each a `BUG`, where `FILE.c` refuses it: with a pathname as
    /// well as the handle, on a handle with no transfer, and on a listing.
    fn delete_while_open(&mut self, tid: &str, handle: &str, pathname: &str) {
        if !pathname.is_empty() {
            return self.error(
                tid,
                handle,
                "BUG",
                'C',
                "Both a file handle and filename in DELETE",
            );
        }
        let what = match self.transfers.get(handle) {
            None => Err("No transfer when DELETE on file handle"),
            Some(Transfer::Directory) => Err("Trying to DELETE a directory list transfer"),
            Some(Transfer::Read { truename, .. }) => Ok(Some(truename.clone())),
            Some(Transfer::Write { .. }) => Ok(None),
        };
        match what {
            Err(why) => self.error(tid, handle, "BUG", 'C', why),
            Ok(Some(read)) => self.delete_entry(tid, handle, &read),
            Ok(None) => {
                if let Some(Transfer::Write { temp, deleted, .. }) = self.transfers.get_mut(handle)
                    && !*deleted
                {
                    let _ = std::fs::remove_file(&*temp);
                    *deleted = true;
                }
                self.reply(tid, handle, "DELETE", "")
            }
        }
    }

    /// `DELETE` of what a pathname names, and **a link itself, never what
    /// it leads to**: the pathname is resolved as an entry, its directory
    /// followed and its own name not (`docs/design.md` §6, "FILE's rules"). So a
    /// link that leads nowhere is removed like any other.
    fn delete_entry(&mut self, tid: &str, handle: &str, pathname: &str) {
        let path = match self.tree.resolve_entry_for_writing(pathname) {
            Ok(entry) => entry.path(),
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        // The entry itself: `symlink_metadata`, `remove_dir` and
        // `remove_file` follow no link that is the path's last component.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        };
        let gone =
            if meta.is_dir() { std::fs::remove_dir(&path) } else { std::fs::remove_file(&path) };
        match gone {
            Ok(()) => {
                self.changed(&format!("delete {pathname}"));
                self.reply(tid, handle, "DELETE", "")
            }
            Err(e) if meta.is_dir() => {
                self.error(tid, handle, "DNE", 'C', &format!("Directory not empty: {e}"))
            }
            Err(e) => self.error(tid, handle, "ATF", 'C', &e.to_string()),
        }
    }

    /// `RENAME`, the old pathname then the new, each resolved as an entry:
    /// **a link itself is moved**, never what it leads to, and a new name
    /// that is a link is there already, whether or not it leads anywhere
    /// (`docs/design.md` §6, "FILE's rules"). Two ends in different roots are
    /// refused, `ATF`: that would be a copy.
    fn rename(&mut self, tid: &str, handle: &str, old: &str, new: &str) {
        let (from, to) = match self.tree.resolve_entries_for_renaming(old, new) {
            Ok((from, to)) => (from.path(), to.path()),
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if std::fs::symlink_metadata(&from).is_err() {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        if std::fs::symlink_metadata(&to).is_ok() {
            return self.error(tid, handle, "REF", 'C', "Rename to existing file");
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                self.changed(&format!("rename {old} to {new}"));
                self.reply(tid, handle, "RENAME", "")
            }
            Err(e) => self.error(tid, handle, "ATF", 'C', &e.to_string()),
        }
    }

    /// `CREATE-DIRECTORY`, the pathname on the next line.
    fn create_directory(&mut self, tid: &str, handle: &str, pathname: &str) {
        let place = match self.tree.resolve_for_writing(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if std::fs::symlink_metadata(&place.path).is_ok() {
            return self.error(tid, handle, "DAE", 'C', "Directory already exists");
        }
        match std::fs::create_dir(&place.path) {
            Ok(()) => {
                self.changed(&format!("create-directory {pathname}"));
                self.reply(tid, handle, "CREATE-DIRECTORY", "")
            }
            Err(e) => self.error(tid, handle, "CCD", 'C', &e.to_string()),
        }
    }

    /// `CREATE-LINK`, the link then what it points at, both resolved
    /// (`docs/design.md` §6): the link for writing, the
    /// target for reading, so it can only lead into the tree. The link
    /// holds the target's canonical path. One into another root is made,
    /// and refused wherever it is used, since a link must lead into its
    /// own root (`roots::Tree::resolve`). `/` itself is no target: it has
    /// no path, and the base's alone would miss the mounts.
    fn create_link(&mut self, tid: &str, handle: &str, link: &str, target: &str) {
        let (link_path, target_path) =
            match (self.tree.resolve_for_writing(link), self.tree.resolve(target)) {
                (Ok(a), Ok(Resolved::Place(b))) => (a.path, b.path),
                (Ok(_), Ok(Resolved::Top)) => {
                    let (code, msg) = denied();
                    return self.error(tid, handle, code, 'C', &msg);
                }
                (Err((code, msg)), _) | (_, Err((code, msg))) => {
                    return self.error(tid, handle, code, 'C', &msg);
                }
            };
        if std::fs::symlink_metadata(&link_path).is_ok() {
            return self.error(tid, handle, "FAE", 'C', "File already exists");
        }
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target_path, &link_path);
        #[cfg(not(unix))]
        let made: std::io::Result<()> =
            Err(std::io::Error::other("links are not made on this platform"));
        match made {
            Ok(()) => {
                self.changed(&format!("create-link {link} to {target}"));
                self.reply(tid, handle, "CREATE-LINK", "")
            }
            Err(e) => self.error(tid, handle, "CCL", 'C', &e.to_string()),
        }
    }

    /// `CHANGE-PROPERTIES`, the pathname then `NAME value` a line. Only
    /// the ones a file here has are settable; the rest are refused by
    /// name, as `FILE.c` refuses what its property table has no setter
    /// for.
    fn change_properties(&mut self, tid: &str, handle: &str, lines: &[&str]) {
        let pathname = lines.first().copied().unwrap_or("");
        let place = match self.tree.resolve_for_writing(pathname) {
            Ok(p) => p,
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        if std::fs::symlink_metadata(&place.path).is_err() {
            return self.error(tid, handle, "FNF", 'C', "File not found");
        }
        for line in lines.iter().skip(1).filter(|l| !l.is_empty()) {
            let name = line.split(' ').next().unwrap_or("");
            match name {
                // The dates and the author are what `FILE.c` can set;
                // nothing here keeps an author, and a date is the file's
                // own, which is left as the filesystem has it.
                "CREATION-DATE" | "MODIFICATION-DATE" | "REFERENCE-DATE" | "AUTHOR" => {}
                "" => {}
                other => {
                    return self.error(
                        tid,
                        handle,
                        "UKP",
                        'C',
                        &format!("{other} cannot be set here"),
                    );
                }
            }
        }
        self.changed(&format!("change-properties {pathname}"));
        self.reply(tid, handle, "CHANGE-PROPERTIES", "");
    }

    /// `COMPLETE options`, the default pathname then the string to
    /// complete. The reply is a status word and the completion, a line
    /// each: the client reads the word as a keyword and takes `NIL` for
    /// no completion.
    fn complete(&mut self, tid: &str, handle: &str, args: &str, default: &str, partial: &str) {
        let new_ok = args.contains("NEW-OK");
        let (dir, stem) = match partial.rsplit_once('/') {
            Some((d, p)) => (d.to_string(), p.to_string()),
            None => (
                default.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default(),
                partial.to_string(),
            ),
        };
        let hits: Vec<String> = match self.names_in(&dir) {
            Ok(names) => {
                names.unwrap_or_default().into_iter().filter(|n| n.starts_with(&stem)).collect()
            }
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        let nl = NEWLINE as char;
        let (status, completed) = match hits.len() {
            0 if new_ok => ("NEW", format!("{}/{stem}", dir.trim_end_matches('/'))),
            0 => ("NIL", format!("{}/{stem}", dir.trim_end_matches('/'))),
            1 => ("OLD", format!("{}/{}", dir.trim_end_matches('/'), hits[0])),
            _ => {
                // The longest head they share, which is as far as a
                // completion can go.
                let mut common = hits[0].clone();
                for h in &hits[1..] {
                    // By character and not by byte: a name is UTF-8 on the
                    // host, and a cut inside a character is not a string.
                    let n = common
                        .chars()
                        .zip(h.chars())
                        .take_while(|(a, b)| a == b)
                        .map(|(a, _)| a.len_utf8())
                        .sum();
                    common.truncate(n);
                }
                ("NIL", format!("{}/{common}", dir.trim_end_matches('/')))
            }
        };
        self.reply(tid, handle, "COMPLETE", &format!("{status}{nl}{completed}{nl}"));
    }

    /// `FILEPOS <n>`: the read goes on from byte `n`.
    ///
    /// The client sends this with its mark flag set --- `:COMMAND T
    /// "File Position" "FILEPOS " n` --- and then reads until a
    /// synchronous mark, which is how it throws away what is already in
    /// flight from the old position. So the reply comes first, then the
    /// mark, and then the file again from where it was asked for.
    ///
    /// The position is in the bytes as they go down the wire, which for
    /// a character file is after the translation: that is what the
    /// client counts, having read them itself.
    fn filepos(&mut self, tid: &str, handle: &str, arg: &str) {
        let Ok(at) = arg.parse::<usize>() else {
            return self.error(tid, handle, "FOR", 'C', "Filepos out of range");
        };
        let Some(Transfer::Read { contents, op, .. }) = self.transfers.get(handle) else {
            return self.error(tid, handle, "BUG", 'C', "No transfer to position");
        };
        if at > contents.len() {
            return self.error(tid, handle, "FOR", 'C', "Filepos out of range");
        }
        let (rest, op) = (contents[at..].to_vec(), *op);
        self.reply(tid, handle, "FILEPOS", "");
        if let Some(ch) = self.handles.get(handle) {
            let mut ch = ch.lock().unwrap();
            // What is still queued from the old position never goes.
            ch.out.retain(|o| !matches!(o, Out::DataOp(..) | Out::Eof));
            ch.out.push_back(Out::DataOp(SYNC_MARK_OP, Vec::new()));
            for chunk in rest.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(op, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
    }

    /// `PROPERTIES`, the pathname on the next line: one record down the
    /// data connection, the same shape a directory's entries have. `/`
    /// itself is a directory with nothing on disk of its own, described as
    /// [`Control::top_properties`] says.
    fn properties(&mut self, tid: &str, handle: &str, pathname: &str) {
        let Some(channel) = self.handles.get(handle).cloned() else {
            return self.error(tid, handle, "BUG", 'C', "No such file handle");
        };
        let nl = NEWLINE as char;
        let text = match self.tree.resolve(pathname) {
            Ok(Resolved::Top) => {
                format!("{}{nl}{}{nl}", truename(pathname, true), self.top_properties())
            }
            Ok(Resolved::Place(p)) => {
                let Some(meta) = described(&p) else {
                    return self.error(tid, handle, "FNF", 'C', "File not found");
                };
                format!(
                    "{}{nl}{}{nl}",
                    truename(pathname, meta.is_dir()),
                    self.file_properties(&meta)
                )
            }
            Err((code, msg)) => return self.error(tid, handle, code, 'C', &msg),
        };
        self.reply(tid, handle, "PROPERTIES", "");
        {
            let bytes = lispm_text(&text);
            let mut ch = channel.lock().unwrap();
            for chunk in bytes.chunks(MAX_DATA) {
                ch.out.push_back(Out::DataOp(CHARACTER_OP, chunk.to_vec()));
            }
            ch.out.push_back(Out::Eof);
        }
        self.transfers.insert(handle.to_string(), Transfer::Directory);
    }

    /// The property lines a file has, `NAME value` each, from `FILE.c`'s
    /// property table.
    fn file_properties(&self, meta: &std::fs::Metadata) -> String {
        self.property_lines(meta.len(), &date(meta), meta.is_dir())
    }

    /// The property lines of `/` itself: a directory, of no length, dated
    /// now, having no date of its own (see [`Control::open_top`]).
    fn top_properties(&self) -> String {
        self.property_lines(0, &now_date(self.time), true)
    }

    fn property_lines(&self, length: u64, date: &str, directory: bool) -> String {
        let nl = NEWLINE as char;
        let mut t = String::new();
        t.push_str(&format!("AUTHOR {}{nl}", self.user.as_deref().unwrap_or("nobody")));
        t.push_str(&format!("BYTE-SIZE 8{nl}"));
        t.push_str(&format!("LENGTH-IN-BLOCKS {}{nl}", length.div_ceil(1024)));
        t.push_str(&format!("LENGTH-IN-BYTES {length}{nl}"));
        t.push_str(&format!("CREATION-DATE {date}{nl}"));
        if directory {
            t.push_str(&format!("DIRECTORY T{nl}"));
        }
        t
    }
}

impl Session for Control {
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.command(bytes);
    }
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {
        // A write that never reached its CLOSE --- the user end's
        // connection dropped, or the machine rebooted under it --- leaves
        // a temporary that will never be renamed into place; remove it.
        for transfer in self.transfers.values() {
            if let Transfer::Write { temp, .. } = transfer {
                let _ = std::fs::remove_file(temp);
            }
        }
        for ch in self.handles.values() {
            ch.lock().unwrap().out.push_back(Out::Close("Control connection closed".into()));
        }
    }
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        // Data connections that have opened since: answered now. One that
        // closed without opening --- refused by the client, or given up by
        // the NCP --- is answered with `FILE.c`'s error for it, `NET`, and
        // its two handles go: `FILE.c` makes them only once its `chopen`
        // has succeeded.
        let mut opened = Vec::new();
        let mut failed = Vec::new();
        self.pending.retain(|p| {
            let ch = p.channel.lock().unwrap();
            if ch.open {
                opened.push(p.tid.clone());
                false
            } else if ch.closed {
                failed.push((p.tid.clone(), p.channel.clone()));
                false
            } else {
                true
            }
        });
        for tid in opened {
            self.reply(&tid, "", "DATA-CONNECTION", "");
        }
        for (tid, channel) in failed {
            self.handles.retain(|_, ch| !Arc::ptr_eq(ch, &channel));
            self.error(&tid, "", "NET", 'C', "Data connection could not be established");
        }
        // What the user end has sent up a data connection belongs to the
        // **write** in progress on that connection, and to nothing else.
        // Both handles of a connection share one channel, so draining the
        // handle that is writing takes it all; the other is left alone,
        // or it would steal its sibling's data --- and it will have a
        // transfer of its own whenever the user end is reading a file and
        // writing one at once, which is what the band does through every
        // compile (`sys/qcfile.lisp`'s `QC-FILE` holds the source open
        // around the QFASL it writes). Selecting on *a* transfer rather
        // than a write is how a compiled file came to be written to no
        // one: the read's drain took the write's bytes, dropped them for
        // having nowhere to go, and the clear below finished the job.
        // `a_read_and_a_write_on_one_data_connection_keep_their_own_bytes`
        // in `tests/file.rs`.
        self.drain_writes();
        // A CLOSE that overtook its synchronous mark waits in the
        // transfer; the mark has now come, so it can be answered. A
        // write that has stalled since goes through here too, to the
        // error the stalled arm gives it: a stall holds bytes that have
        // not been written, so no mark can make that file whole.
        let ready: Vec<(String, String)> = self
            .transfers
            .iter()
            .filter_map(|(handle, transfer)| match transfer {
                Transfer::Write { closing: Some(tid), marked, stalled, .. }
                    if *marked || stalled.is_some() =>
                {
                    Some((tid.clone(), handle.clone()))
                }
                _ => None,
            })
            .collect();
        for (tid, handle) in ready {
            self.close(&tid, &handle);
        }
        // Anything still waiting has no write to go to --- data before an
        // OPEN WRITE, or after a CLOSE --- and is discarded, so a channel
        // never grows without bound.
        for ch in self.handles.values() {
            ch.lock().unwrap().incoming.clear();
        }
        self.out.drain(..).collect()
    }
}

/// What is at a place, if FILE may open it: [`Place::metadata`] --- a
/// regular file or a directory, or `None` if nothing is there --- read
/// with `symlink_metadata`, which opens nothing (`docs/design.md` §6,
/// "Opening"). The loop is one thread, and opening a FIFO that has no
/// writer would block it, and every client with it.
///
/// **Anything else is `WKF`**, and not the `ATD` the tree gives. The band
/// makes a condition of both: `sys/io/file/open.lisp` puts each code on
/// `FILE-ERROR` (`:231`, `:260`), and `qfile.lisp`'s
/// `QFILE-PROCESS-ERROR-NEW` signals what it finds there. So the choice is
/// what each says. `ATD` is
/// `INCORRECT-ACCESS-TO-DIRECTORY`, an `ACCESS-ERROR`, "Directory
/// protection screwed you." `WKF` is `WRONG-KIND-OF-FILE`, whose own kinds
/// are an operation invalid for a directory or for a link --- what this
/// is. The two are told apart by a second `symlink_metadata`, which opens
/// nothing either; every other refusal comes back as the tree gave it.
pub(crate) fn openable(place: &Place) -> Result<Option<std::fs::Metadata>, Refusal> {
    place.metadata().map_err(|refusal| match std::fs::symlink_metadata(&place.path) {
        Ok(m) if !m.is_file() && !m.is_dir() => wrong_kind(),
        _ => refusal,
    })
}

/// What is at a place, to describe and not to open: what [`openable`]
/// finds, or, for what may not be opened, the thing itself as
/// `symlink_metadata` reads it --- a FIFO's length and date, read without
/// opening it. `None` if nothing is there, or it cannot be read.
fn described(place: &Place) -> Option<std::fs::Metadata> {
    match place.metadata() {
        Ok(m) => m,
        Err(_) => std::fs::symlink_metadata(&place.path).ok(),
    }
}

/// The name as the user end will see it: the pathname it asked for.
pub(crate) fn truename(pathname: &str, directory: bool) -> String {
    let t = pathname.trim_end_matches('/');
    if directory { format!("{t}/") } else { t.to_string() }
}

/// `MM/DD/YY HH:MM:SS`, the form `PARSE-DIRECTORY-DATE-PROPERTY` reads
/// fastest, from the file's modification time, in UTC.
pub(crate) fn date(meta: &std::fs::Metadata) -> String {
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{m:02}/{d:02}/{:02} {hh:02}:{mm:02}:{ss:02}", y % 100)
}

/// The calendar a date is made by: the log's (`crate::log::civil`).
pub use crate::log::civil;

/// `*` any run, `?` any one, else itself: the glob the user end sends.
///
/// The two-pointer match, linear in the lengths: walk the name, and on a
/// mismatch fall back to the last `*` seen, advancing by one the name
/// position it is taken to have matched to. The recursive form that tried
/// both branches of every `*` was exponential in the number of stars ---
/// a pattern of ten `*a` against a long run of `a`s pinned the engine's
/// thread for billions of tries. Bytes, not characters, as the rest of
/// the matcher is: a name is UTF-8 on the host, and `?` matching one byte
/// is what it did before.
pub fn matches(pattern: &str, name: &str) -> bool {
    // An empty pattern matches anything, as it did before: DIRECTORY of a
    // path ending in a slash lists the whole directory.
    if pattern.is_empty() {
        return true;
    }
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut pi, mut ni) = (0, 0);
    // The last `*` in the pattern, and the name position it is currently
    // taken to have matched up to; `None` until one is seen.
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            // Back to the last `*`, letting it swallow one more character.
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, ni));
        } else {
            return false;
        }
    }
    // The name is used up; any trailing `*`s match the empty run.
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}
