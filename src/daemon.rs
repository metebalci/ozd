// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon: the link, the NCP and the services wired together, and the
//! turn that is the body of its loop (`docs/design.md` §4).
//!
//! What the link has for this host goes to [`Ncp::receive`], and every
//! buffer [`Ncp::transmit`] gives goes to [`Link::send`] --- the pump,
//! `docs/design.md` §4, none of it protocol. Before that, each connection
//! waiting at a `--tcp` listener is carried to a stream (`crate::tcp`).

use crate::chudp::Link;
use crate::config::{Config, Logging};
use crate::log::{self, Hook, Names};
use crate::ncp::{Ncp, Service};
use crate::roots::Tree;
use crate::service::file::File;
use crate::service::hostab::Hostab;
use crate::service::mini::Mini;
use crate::service::name::Name;
use crate::service::status::{Meters, Status};
use crate::service::time::{Time, Uptime};
use crate::tcp::{Carrier, Listener};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// This host: its link --- the socket, the endpoints, the switch --- its NCP
/// with what `services` gives it to serve, the meters the link counts into
/// and STATUS reads, and its `--tcp` listeners.
pub struct Daemon {
    link: Link,
    ncp: Ncp,
    meters: Arc<Meters>,
    /// Each `--tcp`, bound; none without one (`docs/design.md` §8).
    listeners: Vec<Listener>,
    /// The site's host table, for the lines a TCP connection makes.
    names: Arc<Names>,
    /// `--log-tcp`: where each TCP connection's opening and closing are
    /// written; none unless asked for.
    tcp_log: Option<Hook>,
}

impl Daemon {
    /// The daemon for the site `config` gives: its socket bound at
    /// `config.listen`, its NCP at `config.address`, and what it serves.
    /// `logging` is what this run writes down: `--trace` for the link and
    /// the NCP both, `--log-simple` for the NCP's answers, `--log-file`
    /// and `--log-file-probe` for FILE, `--log-mini` for MINI, and
    /// `--log-tcp` for `--tcp`'s connections (`docs/design.md` §10). A socket or a `--tcp` listener
    /// that cannot be bound is the error, naming its flag, and then nothing
    /// is served. `tree` is the roots as the startup checked them, which
    /// FILE serves (`docs/design.md` §6).
    pub fn new(config: &Config, tree: Arc<Tree>, logging: Logging) -> io::Result<Daemon> {
        let meters = Arc::new(Meters::default());
        let mut link = Link::bind(config, meters.clone())
            .map_err(|e| io::Error::new(e.kind(), format!("--listen {}: {e}", config.listen)))?;
        link.trace = logging.trace;
        let mut ncp = Ncp::new(config.address);
        // Each connection opened, refused and closed, as a line of the log
        // naming the host by the site's table (`docs/design.md` §10); `trace`
        // stays the packets.
        ncp.log = Some(Arc::new(|line: &str| log::event(line)));
        ncp.trace = logging.trace;
        ncp.log_simple = logging.simple;
        let names = Arc::new(names(config));
        ncp.names = names.clone();
        // The switch's one unconditional line: a packet for a host of this
        // subnet that has never spoken (`docs/design.md` §10).
        link.names = names.clone();
        link.log = Some(Arc::new(|line: &str| log::event(line)));
        for service in services(config, &meters, &tree, logging, &names) {
            ncp.serve(service);
        }
        let listeners = config
            .tcps
            .iter()
            .map(|t| {
                Listener::bind(t)
                    .map_err(|e| io::Error::new(e.kind(), format!("--tcp {}: {e}", t.listen)))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let tcp_log: Option<Hook> =
            if logging.tcp { Some(Arc::new(|line: &str| log::event(line))) } else { None };
        Ok(Daemon { link, ncp, meters, listeners, names, tcp_log })
    }

    /// Where the socket is bound, as the system bound it: a `listen` at
    /// port 0 is the port the system picked.
    pub fn at(&self) -> SocketAddr {
        self.link.at()
    }

    /// Where each `--tcp` listener is bound, as the system bound it, in the
    /// order given; none without `--tcp`.
    pub fn tcp_at(&self) -> Vec<SocketAddr> {
        self.listeners.iter().map(Listener::at).collect()
    }

    /// What the link has counted, which STATUS answers with (`docs/design.md`
    /// §7).
    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    /// The link, and what it knows of where each host is.
    pub fn link(&self) -> &Link {
        &self.link
    }

    /// The NCP, for a connection opened from this host's end --- as FILE
    /// opens its data connections, and as a test opens one to watch it go
    /// unanswered.
    pub fn ncp(&mut self) -> &mut Ncp {
        &mut self.ncp
    }

    /// Where the lines `--log-tcp` asks for go, for the TCP connections
    /// taken from now on: none for none. A test collects them this way, as
    /// it collects the NCP's through [`Ncp::log`].
    pub fn set_tcp_log(&mut self, log: Option<Hook>) {
        self.tcp_log = log;
    }

    /// Where the switch's own line goes --- a packet dropped for a host of
    /// this subnet never heard from --- for a test to collect. The daemon
    /// writes it to the log; it is not behind a flag, being the one drop
    /// whose cure is an action (`docs/design.md` §10).
    pub fn set_link_log(&mut self, log: Option<Hook>) {
        self.link.log = log;
    }

    /// One turn of the loop at `now`, nanoseconds on the daemon's clock:
    /// wait at most `wait` for one datagram, hand it to the link, and hand
    /// what the link has for this host to the NCP; then everything the NCP
    /// has to send, to the link (`docs/design.md` §4). A `wait` of zero does not
    /// wait at all.
    ///
    /// **It asks the NCP for output whether anything arrived or not**, and
    /// that is what the wait is for: the NCP has no timer of its own, and
    /// sends again what is unreceipted, and gives up a silent connection,
    /// only when [`Ncp::transmit`] is called. Turned with a wait of 100 ms,
    /// as `main` turns it, a retransmission is at most that late against
    /// its 500 ms.
    ///
    /// **The clock is the caller's**: `main` passes nanoseconds since the
    /// daemon started, from `Instant`, and a test passes the clock it sets,
    /// so what depends on time is exact rather than timed. `now` is taken
    /// before the wait, so a datagram that arrives during it is handled at
    /// a `now` up to `wait` old, and what is sent in answer can go again up
    /// to that much early --- by as much as the wait makes one late.
    pub fn turn(&mut self, now: u64, wait: Duration) {
        self.accept(now);
        if let Some(framed) = self.link.receive(now, wait) {
            self.ncp.receive(now, &framed);
        }
        while let Some(buffer) = self.ncp.transmit(now) {
            self.link.send(now, &buffer);
        }
    }

    /// Every TCP connection waiting at a `--tcp` listener, each carried to
    /// a stream from this host to the listener's contact (`crate::tcp`).
    /// None waits: a listener with nothing waiting says so at once. The
    /// client's bytes go when the NCP next polls its carrier, at the end of
    /// this turn and every turn after, so a keystroke waits at most one
    /// turn's wait (`docs/design.md` §4). A listener's own error is always a
    /// line of the log; a connection's opening and closing are lines only
    /// with `--log-tcp`.
    fn accept(&mut self, now: u64) {
        for l in &self.listeners {
            loop {
                match l.accept() {
                    Ok(Some((stream, from))) => {
                        let head = format!(
                            "TCP from {from} to {} at {}",
                            l.contact,
                            self.names.host(l.host)
                        );
                        let log = self.tcp_log.clone().map(|hook| (hook, head));
                        match Carrier::new(stream, log) {
                            Ok(carrier) => {
                                self.ncp.connect(now, l.host, &l.contact, Box::new(carrier));
                            }
                            Err(e) => log::event(format_args!("TCP from {from}: {e}")),
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        log::event(format_args!("--tcp {}: {e}", l.at()));
                        break;
                    }
                }
            }
        }
    }
}

/// What this host serves, and **the one place a service is added**
/// (`docs/design.md` §7); the NCP and the link know nothing of any of them.
///
/// - STATUS, with the official name --- the first of `--name`'s ---
///   this host's subnet, the high byte of its address, and the meters the
///   link counts into;
/// - TIME, from the system clock;
/// - UPTIME, from 0 on the daemon's clock, which is when it started
///   ([`Daemon::turn`]);
/// - HOSTAB, from `--name` --- its `system=` too, if it has one --- and
///   every `--host`;
/// - NAME, saying that nobody is logged in;
/// - FILE, from the roots the startup checked (`docs/design.md` §6), each of
///   its changes to a root a line of the log, and what it serves as well
///   where `--log-file` and `--log-file-probe` ask for it (§10);
/// - MINI, from the same roots, read as FILE reads them, each open a line
///   of the log where `--log-mini` asks for it (§10).
fn services(
    config: &Config,
    meters: &Arc<Meters>,
    tree: &Arc<Tree>,
    logging: Logging,
    names: &Arc<Names>,
) -> Vec<Box<dyn Service>> {
    let mut file = File::new(tree.clone(), Some(Arc::new(|line: &str| log::event(line))));
    file.log_file = logging.file;
    file.log_file_probe = logging.file_probe;
    file.names = names.clone();
    let mut mini = Mini::new(tree.clone(), Some(Arc::new(|line: &str| log::event(line))));
    mini.log_mini = logging.mini;
    mini.names = names.clone();
    vec![
        Box::new(Status::new(&config.names[0], (config.address >> 8) as u8, meters.clone())),
        Box::new(Time::new()),
        Box::new(Uptime::new(0)),
        Box::new(Hostab::new(
            config.address,
            &config.names,
            config.system.as_deref(),
            &config.hosts,
        )),
        Box::new(Name::new()),
        Box::new(file),
        Box::new(mini),
    ]
}

/// The site's host table as the log names a host (`docs/design.md` §10): this
/// host's official name, and each `--host`'s and `--hosts-text` host's.
fn names(config: &Config) -> Names {
    let own = (config.address, config.names[0].as_str());
    let hosts = config.hosts.iter().map(|h| (h.address, h.names[0].as_str()));
    Names::new(std::iter::once(own).chain(hosts))
}
