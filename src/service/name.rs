// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! NAME: who is logged in at this host, which is nobody (`DESIGN.md` §7;
//! `PROTOCOLS.md`, NAME).
//!
//! A stream. The manual, `sys/man/chaos.text` §Name (line 1186): "The
//! Name/Finger protocol of the Arpanet exists in identical form on the
//! Chaosnet. ... The contact name is NAME, which can be followed by a space
//! and a string of arguments like the 'command line' of the Arpanet Name
//! protocol. A stream connection is established and the 'finger' display
//! is output in Lisp Machine character set, followed by an EOF."
//!
//! **The associated machine is asked by default.** The machine's user end,
//! `FINGER` (`sys/network/chaos/chsaux.lisp:457`), asks
//! `SI:ASSOCIATED-MACHINE` when it is given no host, or a user without one
//! (`:462-473`), and sends it `NAME`, or `NAME`, a space and what it was
//! given (`:476`). A band's `(finger)` asks here.
//!
//! **What the machine's own server says** is one line about its user, in
//! columns: `GIVE-NAME` (`chsaux.lisp:423`), which `LISTEN`s for `NAME`
//! and reads nothing of the RFC. This host has no user at a keyboard, and
//! answers in plain words that nobody is logged in: its text is display,
//! not protocol (`DESIGN.md` §7). What `GIVE-NAME`'s columns hold with
//! nobody logged in is not read.
//!
//! **What `FINGER` does with it** (`chsaux.lisp:490-499`): it prints blank
//! lines, reads the first line with `:LINE-IN` --- the second, if the first
//! is empty --- prints the host's name in brackets above it when asked to
//! and the line has no bracket of its own, prints the line, and copies the
//! rest to the EOF. So the line is not empty, has no bracket, and is ended
//! by the machine's newline, 0o215, as `:LINE-OUT` ends one
//! (`sys/io/stream.lisp:475`; `sys/io/rddefs.lisp:172`). `GIVE-NAME` sends
//! its line without one (`chsaux.lisp:428`, no `~%`); `FINGER` prints the
//! same either way, since `:LINE-IN` hands back an unended last line with
//! the EOF (`sys/io/stream.lisp:552`) and `:LINE-OUT` adds its own, and
//! ended it is a whole line to any other client too.
//!
//! **Then it closes**, once its EOF is receipted, as `GIVE-NAME` does: it
//! answers through `FORMAT-AND-EOF` (`sys/network/chaos/chuse.lisp:550`),
//! which closes its stream, and closing sends the EOF, waits for its
//! receipt, and sends a CLS with no reason (`BASIC-OUTPUT-STREAM :EOF` and
//! `:BEFORE :CLOSE`, `chuse.lisp:639`, `:647`; `BASIC-STREAM :CLOSE`,
//! `:575`). `FINGER` closes its end too once it has read the EOF, and
//! whichever CLS arrives second finds no connection.
//!
//! **It is not written for a login check.** `USER-LOGGED-INTO-HOST-P`
//! (`chsaux.lisp`) reads a host's finger text by the host's system type ---
//! for a TOPS-20, any text not beginning with `?` or `%` and without
//! `LOGOUT` in it is a login --- and Converse asks it of the hosts in
//! `*CONVERSE-EXTRA-HOSTS-TO-CHECK*` (`sys/io1/conver.lisp:1235`), which is
//! empty unless a site fills it (`:49`). This line is not shaped to any of
//! those readings, and by most of them would be taken for a login.
//! **Unverified** what system type a band's table gives this host.

use crate::lispm::{self, NEWLINE};
use crate::ncp::{Out, Response, Service, Session};

/// What this host says: nobody is logged in (`DESIGN.md` §7).
const NOBODY: &str = "Nobody is logged in.";

/// NAME: that nobody is logged in here.
#[derive(Default)]
pub struct Name;

impl Name {
    pub fn new() -> Name {
        Name
    }
}

impl Service for Name {
    fn contact(&self) -> &str {
        "NAME"
    }
    /// Every request is answered alike: the RFC's arguments are ignored, as
    /// `GIVE-NAME` ignores them.
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        let mut line = lispm::lispm_text(NOBODY);
        line.push(NEWLINE);
        Response::Accept(Box::new(NameSession(vec![
            Out::Data(line),
            Out::Eof,
            Out::Close(String::new()),
        ])))
    }
}

/// One NAME connection: the line, the EOF and the CLS, to be sent in turn
/// --- each once the one before is receipted, as the NCP sends them.
struct NameSession(Vec<Out>);

impl Session for NameSession {
    /// Nothing the client sends is read, as `GIVE-NAME` reads nothing.
    fn data(&mut self, _now: u64, _op: u8, _bytes: &[u8]) {}
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.0)
    }
}
