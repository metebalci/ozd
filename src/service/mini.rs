// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The MINI service: the file protocol a cold load reads its files by,
//! before it has a network stack or a FILE client (`docs/protocols.md`,
//! MINI).
//!
//! A cold load made by `MAKE-COLD` holds the reader, the evaluator, the
//! fasloader and `cold/mini.lisp`. `(si:qld)` loads the inner system
//! through MINI, FILE's own client `qfile` among it, and only then does
//! FILE take over (`sys/ltop.lisp`, `QLD`). The machine drives its Chaos
//! interface itself, without an NCP; at this end MINI is an ordinary
//! stream.
//!
//! **The protocol**, as `cold/minisr.mid`, MIT's server, gives it in its
//! header and `cold/mini.lisp` reads it:
//!
//! - The machine opens a file with a data packet of opcode `200` for
//!   characters or `201` for binary, the file name its data.
//! - The server wins with `202`: the truename, the machine's newline and
//!   the file's date as `MM/DD/YY HH:MM:SS`. Or it loses with `203`: a
//!   message and the newline. The machine splits either at the newline.
//! - After a win the file follows, in `200` packets of characters or `300`
//!   packets of 16-bit words, and then an EOF. The machine then opens the
//!   next file on the same connection, which it never closes.
//!
//! **Containment** is FILE's reading: each name is resolved in the
//! [`Tree`], and only a regular file is read (`docs/design.md` §6). MINI
//! only reads, so a read-only root serves it whole.

use crate::lispm::{NEWLINE, from_bytes, lispm_text, to_lispm};
use crate::log::{Hook, Names};
use crate::ncp::{Out, Response, Service, Session};
use crate::packet::MAX_DATA;
use crate::roots::{Resolved, Tree};
use crate::service::file::{BINARY_OP, CHARACTER_OP, date, openable, truename};
use std::fmt;
use std::sync::Arc;

/// The contact name. A cold load asks for `MINI LISPM `, a user and an
/// empty password after the name, and nothing is made of them.
pub const CONTACT: &str = "MINI";
/// An open for characters, the file name its data.
pub const CHARACTER_OPEN: u8 = 0o200;
/// An open for 16-bit binary, the file name its data.
pub const BINARY_OPEN: u8 = 0o201;
/// The open won: the truename, the newline and the date.
pub const WIN: u8 = 0o202;
/// The open lost: a message and the newline.
pub const LOSE: u8 = 0o203;

/// The service: the [`Tree`], read.
pub struct Mini {
    tree: Arc<Tree>,
    /// Where each open is written, if anywhere.
    log: Option<Hook>,
    /// `--log-mini`: a line for each open, the file read or the reason it
    /// was refused (`docs/design.md` §10).
    pub log_mini: bool,
    /// The site's host table, for the name each line gives the machine
    /// after its address. Empty unless set, and then every machine is `(?)`.
    pub names: Arc<Names>,
}

impl Mini {
    /// A service over `tree`, answering every host that reaches it, as FILE
    /// does (`docs/design.md` §6). Each open is written to `log` where
    /// [`Mini::log_mini`] asks for it.
    pub fn new(tree: Arc<Tree>, log: Option<Hook>) -> Mini {
        Mini { tree, log, log_mini: false, names: Arc::default() }
    }
}

impl Service for Mini {
    fn contact(&self) -> &str {
        CONTACT
    }
    fn request(&mut self, _now: u64, _args: &str, from: (u16, u16)) -> Response {
        let head = format!("{CONTACT} from {}", self.names.host(from.0));
        let log = if self.log_mini { self.log.clone().map(|hook| (hook, head)) } else { None };
        Response::Accept(Box::new(Reader { tree: self.tree.clone(), log, out: Vec::new() }))
    }
}

/// One machine's connection.
struct Reader {
    tree: Arc<Tree>,
    /// Where each open is written, and what its line begins with.
    log: Option<(Hook, String)>,
    /// What is to go down the connection, in order.
    out: Vec<Out>,
}

impl Reader {
    /// An open of `pathname`: the win, the file and its EOF, or the lose.
    fn open(&mut self, pathname: &str, characters: bool) {
        let (meta, contents) = match self.read(pathname) {
            Ok(read) => read,
            Err(message) => {
                self.note(format_args!("refused {pathname}: {message}"));
                let mut lose = lispm_text(&message);
                lose.truncate(MAX_DATA - 1);
                lose.push(NEWLINE);
                self.out.push(Out::DataOp(LOSE, lose));
                return;
            }
        };
        self.note(format_args!("read {pathname}"));
        let date = date(&meta);
        let mut win = lispm_text(&truename(pathname, false));
        // The date always fits: a pathname as long as a packet gives way.
        win.truncate(MAX_DATA - 1 - date.len());
        win.push(NEWLINE);
        win.extend_from_slice(date.as_bytes());
        self.out.push(Out::DataOp(WIN, win));
        let (op, bytes) =
            if characters { (CHARACTER_OP, to_lispm(&contents)) } else { (BINARY_OP, contents) };
        self.out.extend(bytes.chunks(MAX_DATA).map(|chunk| Out::DataOp(op, chunk.to_vec())));
        self.out.push(Out::Eof);
    }

    /// The file at `pathname` and what it is, read whole; or the message
    /// that refuses it. The tree and `openable` decide as they decide for
    /// FILE's reads, and a directory, `/` among them, is not a file.
    fn read(&self, pathname: &str) -> Result<(std::fs::Metadata, Vec<u8>), String> {
        let place = match self.tree.resolve(pathname) {
            Ok(Resolved::Place(place)) => place,
            Ok(Resolved::Top) => return Err("That is a directory".into()),
            Err((_, message)) => return Err(message),
        };
        let meta = match openable(&place) {
            Ok(Some(meta)) => meta,
            Ok(None) => return Err("File not found".into()),
            Err((_, message)) => return Err(message),
        };
        if meta.is_dir() {
            return Err("That is a directory".into());
        }
        // A regular file, by `openable`: a FIFO, which would hold the loop's
        // one thread, is refused before anything opens it.
        std::fs::read(&place.path)
            .map(|contents| (meta, contents))
            .map_err(|_| "Access to file denied".into())
    }

    /// Writes what came of an open, where `--log-mini` asks.
    fn note(&self, what: fmt::Arguments) {
        if let Some((hook, head)) = &self.log {
            hook(&format!("{head} {what}"));
        }
    }
}

impl Session for Reader {
    /// An open. The machine sends nothing else, and anything else is passed
    /// over.
    fn data(&mut self, _now: u64, op: u8, bytes: &[u8]) {
        let characters = match op {
            CHARACTER_OPEN => true,
            BINARY_OPEN => false,
            _ => return,
        };
        self.open(&from_bytes(bytes), characters);
    }

    fn eof(&mut self, _now: u64) {}

    fn closed(&mut self, _now: u64, _reason: &str) {}

    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }
}
