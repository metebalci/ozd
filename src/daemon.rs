// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon: the link, the NCP and the services wired together, and the
//! turn that is the body of its loop (`DESIGN.md` §4).
//!
//! In muir the NCP and CHUDP are two nodes that meet on a modelled cable.
//! Here there is no cable: what the link has for this host goes to
//! [`Ncp::receive`], and every buffer [`Ncp::transmit`] gives goes to
//! [`Link::send`] --- "the pump" of `DESIGN.md` §7, none of it protocol.

use crate::chudp::Link;
use crate::config::Config;
use crate::ncp::{Ncp, Service};
use crate::service::hostab::Hostab;
use crate::service::name::Name;
use crate::service::status::{Meters, Status};
use crate::service::time::{Time, Uptime};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// This host: its link --- the socket, the endpoints, the hub --- its NCP
/// with what [`services`] gives it to serve, and the meters the link
/// counts into and STATUS reads.
pub struct Daemon {
    link: Link,
    ncp: Ncp,
    meters: Arc<Meters>,
}

impl Daemon {
    /// The daemon for the site `config` gives: its socket bound at
    /// `config.listen`, its NCP at `config.address`, and what it serves.
    /// `trace` is `--trace`, for the link and the NCP both (`DESIGN.md`
    /// §10). A socket that cannot be bound is the error, and then nothing
    /// is served.
    pub fn new(config: &Config, trace: bool) -> io::Result<Daemon> {
        let meters = Arc::new(Meters::default());
        let mut link = Link::bind(config, meters.clone())?;
        link.trace = trace;
        let mut ncp = Ncp::new(config.address);
        ncp.trace = trace;
        for service in services(config, &meters) {
            ncp.serve(service);
        }
        Ok(Daemon { link, ncp, meters })
    }

    /// Where the socket is bound, as the system bound it: a `listen` at
    /// port 0 is the port the system picked.
    pub fn at(&self) -> SocketAddr {
        self.link.at()
    }

    /// What the link has counted, which STATUS answers with (`DESIGN.md`
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

    /// One turn of the loop at `now`, nanoseconds on the daemon's clock:
    /// wait at most `wait` for one datagram, hand it to the link, and hand
    /// what the link has for this host to the NCP; then everything the NCP
    /// has to send, to the link (`DESIGN.md` §4). A `wait` of zero does not
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
        if let Some(framed) = self.link.receive(now, wait) {
            self.ncp.receive(now, &framed);
        }
        while let Some(buffer) = self.ncp.transmit(now) {
            self.link.send(now, &buffer);
        }
    }
}

/// The services this host serves, built from the config: the one place a
/// service is added (`DESIGN.md` §7). STATUS counts into the link's
/// meters; UPTIME counts from the daemon's start, `now` 0; HOSTAB answers
/// from the config's names and host lines. FILE comes with `src/roots.rs`.
fn services(config: &Config, meters: &Arc<Meters>) -> Vec<Box<dyn Service>> {
    vec![
        Box::new(Status::new(&config.names[0], (config.address >> 8) as u8, meters.clone())),
        Box::new(Time::new()),
        Box::new(Uptime::new(0)),
        Box::new(Hostab::new(config.address, &config.names, &config.hosts)),
        Box::new(Name::new()),
    ]
}
