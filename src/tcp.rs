// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A TCP listener carried to a Chaosnet stream, `--tcp` (`docs/design.md`
//! §8): each connection a listener takes becomes a stream from this host to
//! the listener's contact at its host, and the bytes go both ways.
//!
//! **What it hands out.** Carried to TELNET, a connection is a Lisp top
//! level with no login, for whoever reaches the listener. So nothing listens
//! without `--tcp`, a bare port is the loopback, and each listener names its
//! one contact ([`crate::config::Config::tcps`]).
//!
//! **How the bytes go.** [`Carrier`] is the connection's session. The NCP
//! polls it whenever the connection has nothing unreceipted
//! ([`Session::poll`]), and it reads the TCP connection then, without
//! waiting, at most one packet's worth: what is not read waits in TCP, which
//! holds a fast client back. What the contact sends is written to the TCP
//! connection as it arrives, and held for a slow client up to
//! [`PENDING_LIMIT`]. The client closing its end is an EOF and then a CLS,
//! which the NCP sends only once what went before them is receipted. The
//! Chaosnet connection ending, however it ends, drops the carrier, and the
//! TCP connection with it.
//!
//! **What it writes down**, with `--log-tcp`: a line when the TCP connection
//! opens and one when it closes, with who closed it, and nothing for what
//! passes between (`docs/design.md` §10).

use crate::config::Tcp;
use crate::log::Hook;
use crate::ncp::{Out, Session};
use crate::packet::MAX_DATA;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};

/// What a carrier holds for a client that reads slower than the contact
/// sends, before it gives the connection up: a megabyte, far past any
/// screenful a TELNET session makes.
pub const PENDING_LIMIT: usize = 1 << 20;

/// One `--tcp` listener, bound, and never waiting.
pub struct Listener {
    listener: TcpListener,
    at: SocketAddr,
    /// The contact name its connections are carried to.
    pub contact: String,
    /// The host of this subnet the contact is at.
    pub host: u16,
}

impl Listener {
    /// `tcp`'s listener, bound at its endpoint.
    pub fn bind(tcp: &Tcp) -> io::Result<Listener> {
        let listener = TcpListener::bind(tcp.listen)?;
        listener.set_nonblocking(true)?;
        let at = listener.local_addr()?;
        Ok(Listener { listener, at, contact: tcp.contact.clone(), host: tcp.host })
    }

    /// Where it is bound, as the system bound it: a port 0 is the port the
    /// system picked.
    pub fn at(&self) -> SocketAddr {
        self.at
    }

    /// The next connection waiting, and where it is from; none if none is.
    pub fn accept(&self) -> io::Result<Option<(TcpStream, SocketAddr)>> {
        match self.listener.accept() {
            Ok(taken) => Ok(Some(taken)),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// The session one TCP connection is carried by.
pub struct Carrier {
    stream: TcpStream,
    /// What the contact sent that the client has not taken yet.
    pending: Vec<u8>,
    /// The contact sent EOF: the client's reading end is shut once what is
    /// pending has gone.
    eof: bool,
    /// The client's reading end is shut.
    shut: bool,
    /// The TCP connection has ended at the client, and the EOF and CLS, or
    /// the CLS alone, are asked for.
    ended: bool,
    /// `--log-tcp`: where the connection's opening and closing are written,
    /// and what each line begins with.
    log: Option<(Hook, String)>,
    /// The closing is written already.
    closing_logged: bool,
}

impl Carrier {
    /// A carrier for `stream`, set not to wait for anything. `log`, if
    /// given, is where its opening is written now and its closing later,
    /// and what each line begins with.
    pub fn new(stream: TcpStream, log: Option<(Hook, String)>) -> io::Result<Carrier> {
        stream.set_nonblocking(true)?;
        // A keystroke's echo goes to the client when it comes, not with the
        // next one.
        stream.set_nodelay(true)?;
        if let Some((hook, head)) = &log {
            hook(&format!("{head} opened"));
        }
        Ok(Carrier {
            stream,
            pending: Vec::new(),
            eof: false,
            shut: false,
            ended: false,
            log,
            closing_logged: false,
        })
    }

    /// As much of what is pending as the client takes now; an error is the
    /// client gone.
    fn flush(&mut self) -> io::Result<()> {
        while !self.pending.is_empty() {
            match self.stream.write(&self.pending) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.pending.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if self.eof && !self.shut {
            let _ = self.stream.shutdown(Shutdown::Write);
            self.shut = true;
        }
        Ok(())
    }

    /// Writes how the connection closed, once, where `--log-tcp` asks.
    fn closing(&mut self, how: &str) {
        if let Some((hook, head)) = &self.log
            && !self.closing_logged
        {
            hook(&format!("{head} {how}"));
            self.closing_logged = true;
        }
    }

    /// The TCP connection has ended at the client: what to send, and how it
    /// closed.
    fn end(&mut self, outs: &mut Vec<Out>, eof: bool, reason: &str, how: &str) {
        self.ended = true;
        self.closing(how);
        if eof {
            outs.push(Out::Eof);
        }
        outs.push(Out::Close(reason.to_string()));
    }
}

impl Session for Carrier {
    fn data(&mut self, _now: u64, _op: u8, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        // A client that cannot be written to is found gone at the next poll.
        let _ = self.flush();
    }

    fn eof(&mut self, _now: u64) {
        self.eof = true;
        let _ = self.flush();
    }

    fn closed(&mut self, _now: u64, reason: &str) {
        let _ = self.flush();
        self.closing(&format!("closed: {reason}"));
    }

    fn poll(&mut self, _now: u64) -> Vec<Out> {
        let mut outs = Vec::new();
        if self.ended {
            return outs;
        }
        if self.flush().is_err() {
            let gone = "closed, the client is gone";
            self.end(&mut outs, false, "The TCP client is gone", gone);
            return outs;
        }
        if self.pending.len() > PENDING_LIMIT {
            let slow = "closed, the client is not reading";
            self.end(&mut outs, false, "The TCP client is not reading", slow);
            return outs;
        }
        let mut buffer = [0u8; MAX_DATA];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => self.end(&mut outs, true, "The TCP client closed", "closed by the client"),
                Ok(n) => outs.push(Out::Data(buffer[..n].to_vec())),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    let gone = "closed, the client is gone";
                    self.end(&mut outs, false, "The TCP client is gone", gone);
                }
            }
            return outs;
        }
    }
}
