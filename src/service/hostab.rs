// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HOSTAB: the site's host table, by name, for a band that meets a name its
//! own table lacks (`DESIGN.md` §7; `PROTOCOLS.md`, HOSTAB).
//!
//! A stream of transactions. The manual, `sys/man/chaos.text` §Host Table
//! (line 1290): "The user connects to contact name HOSTAB, undertakes a
//! number of transactions, then closes the connection. Each transaction is
//! initiated by the user transmitting a host name followed by a carriage
//! return. The server responds with information about that host,
//! terminated with an EOF, and is then ready for another transaction." An
//! attribute is "an identifying name, a space character, the value of the
//! attribute, and a carriage return".
//!
//! **The machine only calls it, and this is written to what its user end
//! reads.** That is `CHAOS-UNKNOWN-HOST-FUNCTION`
//! (`sys/network/chaos/chuse.lisp:950`), which `SI:PARSE-HOST` calls for a
//! name the band's table lacks (`sys/network/host.lisp:318`), asking each
//! host of the site option `:CHAOS-HOST-TABLE-SERVER-HOSTS` --- `OZ`, in
//! System 100 (`sys/site/site.lisp:91`). It asks one name, reads the
//! answer line by line, and closes:
//!
//! - **The newline is the Lisp Machine's, 0o215, both ways.** The user
//!   end's stream is a `CHARACTER-STREAM` without ASCII translation
//!   (`OPEN-STREAM`, `chuse.lisp:782`), so the bytes are the machine's
//!   character set as they are. The name goes out by `:LINE-OUT`, which
//!   ends it with `#\CR` (`sys/io/stream.lisp:475`); the answer is read by
//!   `:LINE-IN`, which ends a line at `#/NEWLINE`
//!   (`sys/io/stream.lisp:554`). Both are 0o215 (`sys/io/rddefs.lisp:172`),
//!   the manual's "carriage return" in that character set, and [`NEWLINE`]
//!   here.
//! - **Every line ends in it, the last too.** At the EOF, `:LINE-IN` hands
//!   back what is left with its EOF flag set (`sys/io/stream.lisp:552`),
//!   and the user end does not look at that text (`chuse.lisp:963`): a last
//!   line without its newline would be lost.
//! - **Every line has a space.** The attribute is the text before the
//!   first, interned as a keyword as it is sent, and a line without one
//!   fails at `(INCF SP)` of NIL (`chuse.lisp:973-975`). Attributes are in
//!   upper case, as the manual gives them.
//! - **`NAME` comes first, and the official name first of all.** The first
//!   `NAME` makes the list the user end hands to `SI:DEFINE-HOST`, and so is
//!   the host's name; every other attribute is put on that list
//!   (`chuse.lisp:978-988`), so none may come before a `NAME`. The user end
//!   sorts the names shortest first itself (`:964-969`).
//! - **`CHAOS` is octal.** Every attribute but `ERROR`, `NAME` and
//!   `SYSTEM-TYPE` is an address to the user end, and a Chaosnet one is
//!   read by `ZWEI:PARSE-NUMBER ... 8` (`chuse.lisp:984-988`;
//!   `sys/network/chaos/chsaux.lisp:1617`); the manual says "an octal
//!   number".
//! - **`SYSTEM-TYPE` is answered as the `host` line writes it.** The user
//!   end interns it as sent (`chuse.lisp:983`), and it picks the host's
//!   flavor (`COMPUTE-HOST-FLAVOR`, `sys/network/host.lisp:279`); so a type
//!   written in lower case would be a keyword no flavor is filed under.
//! - **Never `MACHINE-TYPE`.** Its clause is written
//!   `(:SYSTEM-TYPE MACHINE-TYPE)` (`chuse.lisp:982`), the second without
//!   its colon, so the keyword the user end interns would not match it, and
//!   the value would be read as a Chaosnet address (`PROTOCOLS.md`,
//!   HOSTAB). **Unverified** against a running band; until it is, it is not
//!   sent.
//! - **`ERROR No such host`** for a name no host has: the one error the
//!   manual expects, "no such host" (line 1315). It ends the user end's
//!   transaction with no host defined (`chuse.lisp:977`).
//!
//! **Names are looked up ignoring case**, as the band's own `PARSE-HOST`
//! compares them (`STRING-EQUAL`, `sys/network/host.lisp:310`), among this
//! host's names and every `host` line's; a line is matched whole. The site
//! file refuses two names that differ only in case (`src/config.rs`), so a
//! name is at most one host's.
//!
//! **This host's own names are answered without a `SYSTEM-TYPE`**: there
//! is no line for it yet (`DESIGN.md` §8). The band then defines it with
//! none, and `COMPUTE-HOST-FLAVOR` falls back to the `:DEFAULT` flavor.
//!
//! **The connection stays open** for the next name until the client closes
//! it. The user end closes on the way out of `WITH-OPEN-STREAM`: an EOF,
//! waited on until it is receipted, then a CLS (`BASIC-OUTPUT-STREAM :EOF`
//! and `:BEFORE :CLOSE`, `chuse.lisp:639`, `:647`; `BASIC-STREAM :CLOSE`,
//! `:575`). The client's EOF says it has no more names and draws nothing;
//! its CLS ends the session. A client that ended its lines with ASCII's CR
//! or LF rather than 0o215 would never end one: no Lisp Machine does, and
//! whether any other HOSTAB client did is **unverified**.

use crate::config::Host;
use crate::lispm::{self, NEWLINE};
use crate::ncp::{Out, Response, Service, Session};
use crate::packet::MAX_DATA;
use std::sync::Arc;

/// The answer for a name no host has: the one error the manual expects,
/// "no such host" (`sys/man/chaos.text`, §Host Table).
const NO_SUCH_HOST: &str = "ERROR No such host";

/// HOSTAB: this host's names and the site's host table, looked up by name.
pub struct Hostab {
    /// Every host there is an answer for: this one first, then the `host`
    /// lines in the order written. Shared with each connection's session.
    hosts: Arc<[Host]>,
    /// The most characters any of their names has. A line longer than that
    /// is no host's name, so a session keeps no more of a line than that
    /// and one character over.
    longest: usize,
}

impl Hostab {
    /// Answering for this host, at `address` under `names`, the official
    /// first, and for every host of the site's table, `hosts`: the site
    /// file's `Config::address`, `Config::names` and `Config::hosts`.
    pub fn new(address: u16, names: &[String], hosts: &[Host]) -> Hostab {
        let own = Host { address, names: names.to_vec(), system: None };
        let hosts: Arc<[Host]> = std::iter::once(own).chain(hosts.iter().cloned()).collect();
        let longest =
            hosts.iter().flat_map(|h| &h.names).map(|n| n.chars().count()).max().unwrap_or(0);
        Hostab { hosts, longest }
    }
}

impl Service for Hostab {
    fn contact(&self) -> &str {
        "HOSTAB"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Accept(Box::new(HostabSession {
            hosts: self.hosts.clone(),
            longest: self.longest,
            line: Vec::new(),
            pending: Vec::new(),
        }))
    }
}

/// One HOSTAB connection: the line coming in, and the answers going out.
struct HostabSession {
    hosts: Arc<[Host]>,
    longest: usize,
    /// The line so far, to its newline, a byte a character: kept to
    /// `longest` characters and one more, which is enough to tell that it
    /// is longer than any name. The newline itself is not kept.
    line: Vec<u8>,
    /// What is to be sent: the answers so far, each ended by its EOF.
    pending: Vec<Out>,
}

impl HostabSession {
    /// The answer for the host named `name`, onto `pending`: its lines, then
    /// an EOF.
    fn answer(&mut self, name: &str) {
        let mut text = Vec::new();
        match self.hosts.iter().find(|h| h.names.iter().any(|n| n.eq_ignore_ascii_case(name))) {
            None => line(&mut text, NO_SUCH_HOST),
            Some(host) => {
                for n in &host.names {
                    line(&mut text, &format!("NAME {n}"));
                }
                line(&mut text, &format!("CHAOS {:o}", host.address));
                if let Some(system) = &host.system {
                    line(&mut text, &format!("SYSTEM-TYPE {system}"));
                }
            }
        }
        // As many packets as it takes: a host may have more names than one
        // packet holds, and `:LINE-IN` reads a line on from one packet into
        // the next (`sys/io/stream.lisp:535`).
        self.pending.extend(text.chunks(MAX_DATA).map(|c| Out::Data(c.to_vec())));
        self.pending.push(Out::Eof);
    }
}

/// `s` onto `text` as one line of an answer: in the machine's character
/// set, and ended by its newline.
fn line(text: &mut Vec<u8>, s: &str) {
    text.extend(lispm::lispm_text(s));
    text.push(NEWLINE);
}

impl Session for HostabSession {
    /// Each line, once its newline arrives, is a name, and is answered. A
    /// line may come in several packets and several lines in one.
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        for &b in bytes {
            if b == NEWLINE {
                let name = lispm::from_bytes(&std::mem::take(&mut self.line));
                self.answer(&name);
            } else if self.line.len() <= self.longest {
                self.line.push(b);
            }
        }
    }
    /// The client has no more names; its CLS, which follows, ends the
    /// session. A line begun and never ended is no name.
    fn eof(&mut self, _now: u64) {}
    fn closed(&mut self, _now: u64, _reason: &str) {}
    fn poll(&mut self, _now: u64) -> Vec<Out> {
        std::mem::take(&mut self.pending)
    }
}
