// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The site file, parsed and checked (`DESIGN.md` §8): this host's
//! address and names, the UDP endpoint it binds, the roots FILE serves,
//! the site's host table for HOSTAB, and the few peers whose endpoints are
//! fixed.
//!
//! ```text
//! address  3060                          # this host's Chaos address; required
//! name     MIT-OZ OZ                     # its names, the official first; required
//! listen   192.0.2.10                    # optional; 127.0.0.1:42042 without it
//! root     /srv/lispm                    # the base root
//! root     tree  /path/to/muir/vendor/system-100-0/sys  readonly
//! host     3050  MIT-LISPM-1 LM1   system=LISPM
//! peer     3040  192.0.2.5
//! ```
//!
//! One directive a line, and its words after it, separated by blanks. `#`
//! starts a comment wherever it is, and a line with nothing left is no
//! line. The directives are lower case. There is no quoting, so no word
//! --- a path, a name --- can hold a blank or a `#`. What each directive
//! takes is on the field it fills: [`Config::address`], [`Config::names`],
//! [`Config::listen`], [`Config::roots`], [`Config::hosts`],
//! [`Config::peers`].
//!
//! **Refused at load, with the line it is on**: a word that is not a
//! directive; an `address` or `name` line missing or given twice, and a
//! `listen` line twice; no `root` line; an address [`parse_address`]
//! refuses; an endpoint that is not an IP literal; a root path that is not
//! absolute, a second base, and a mount's name twice; an address that
//! would be two answers, and a host name given twice (below). The first
//! refusal is the one reported. One that no line is to blame for --- a
//! required line missing --- has no line. [`Config::parse`] touches no
//! disk: whether a root exists, is a directory and can be written is the
//! startup's to check (`DESIGN.md` §6).
//!
//! **An address is one host, and one endpoint.** The host table and the
//! endpoints are separate (`DESIGN.md` §8), so an address is looked for in
//! two places, and it may be in each once: among `address` and the `host`
//! lines, for what a host is called, and among `address` and the `peer`
//! lines, for where its packets go. So this host's own address has no
//! `host` line --- its names are the `name` line's --- and no `peer` line,
//! this host not being its own peer; two `host` lines at one address are
//! two answers to what it is called, and two `peer` lines two answers to
//! where it is, which muir refuses for `--chaos-udp-peer` for the same
//! reason (`src/main.rs`). A `host` and a `peer` line at one address are
//! not ambiguous: they answer different questions about one host, and are
//! how a host is both named and fixed. Whichever line comes later is
//! refused, naming the earlier.
//!
//! **A host name is one host.** No name is given twice among this host's
//! and every `host` line's, and two that differ only in case are the same
//! name: HOSTAB looks a name up ignoring case (`DESIGN.md` §7), and would
//! otherwise find two hosts for it, or one host twice.

use crate::address::parse_address;
use crate::chudp::PORT;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

/// Where the socket is bound without a `listen` line: the loopback at
/// CHUDP's port, so that a fresh install answers its own host and nothing
/// else (`DESIGN.md` §5).
const LISTEN: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT));

/// The site, as its file gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `address <addr>`, exactly one line: this host's Chaos address, in
    /// octal or as `subnet:host`, as [`parse_address`] takes it --- which
    /// refuses a zero half, a zero host being every host of its subnet.
    pub address: u16,
    /// `name <NAME>...`, exactly one line: this host's names, the official
    /// first --- the one STATUS answers with, and HOSTAB's first `NAME`
    /// (`DESIGN.md` §7). At least one. None holds an `=`, which in a
    /// `host` line is what marks an attribute. As written; HOSTAB compares
    /// them ignoring case.
    pub names: Vec<String>,
    /// `listen [<endpoint>]`, at most one line: the UDP endpoint the
    /// socket binds, in the forms muir's `--chaos-udp` takes (muir's
    /// `endpoint_at`, `src/main.rs`), against the loopback at 42042:
    ///
    /// - no `listen` line, or one with nothing after it: `127.0.0.1:42042`;
    /// - a bare port, `42043`: on the loopback, `127.0.0.1:42043`;
    /// - a bare address, `192.0.2.10` or `::1`: at 42042;
    /// - address and port, `192.0.2.10:42043` or `[::1]:42043`: as given;
    /// - `0.0.0.0` or `::`, with a port or without: every interface, on
    ///   purpose (`DESIGN.md` §5).
    ///
    /// IPv6 is written bare, or in brackets before a port. **IP literals
    /// only**: a name would be resolved once at startup and not followed
    /// afterwards, which is what muir does for `--chaos-udp-peer` and what
    /// nothing here does (`DESIGN.md` §8). Port 0 is a port the system
    /// picks, which a test on the loopback wants and a site does not.
    pub listen: SocketAddr,
    /// `root [<name>] <path> [readonly]`, one line a root: the tree FILE
    /// serves (`DESIGN.md` §6), in the order written. Without a name, the
    /// **base root**, at most one; with one, a root **mounted** at the top
    /// level under that name, each name once. At least one root, and
    /// mounts without a base are a site: `/` is then read-only and names
    /// only the mounts.
    ///
    /// - **A path is absolute**, and that is all that is checked of it
    ///   here; existing, being a directory, not being `/`, and being
    ///   writable without `readonly` are the startup's checks, made when
    ///   it canonicalises each root (`DESIGN.md` §6).
    /// - **A mount's name is one directory name, in lower case.** It is the
    ///   first component of a pathname under `/`, so it holds no `/` and is
    ///   neither `.` nor `..`; and names match exactly while a band sends
    ///   its pathnames in lower case (`DESIGN.md` §6; `/tree/sys/...` in
    ///   muir's `--chaos-file-root` help), so a mount named in capitals is
    ///   one no band would reach.
    pub roots: Vec<Root>,
    /// `host <addr> <NAME>... [system=<TYPE>]`, one line a host: the
    /// site's host table, which HOSTAB answers from beside this host's own
    /// names (`DESIGN.md` §7), in the order written. This host is not in
    /// it; its names are [`Config::names`]. A word with `=` in it is an
    /// attribute, anywhere after the address, and `system` is the one
    /// there is.
    pub hosts: Vec<Host>,
    /// `peer <addr> <ip>[:<port>]`, one line a peer: a host whose endpoint
    /// is fixed, so that a packet does not move it; every other endpoint
    /// is learned from the packets a host sends (`DESIGN.md` §5). An IP
    /// literal, as `listen` takes one, at 42042 unless a port is given.
    /// Not a bare port, since a peer is a host to send to and not a socket
    /// to bind; not `0.0.0.0` or `::`, which are every interface and no
    /// host; and not port 0, which nothing can be sent to.
    pub peers: Vec<Peer>,
}

/// One `root` line (`DESIGN.md` §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The name it is mounted under at the top level; none for the base.
    pub name: Option<String>,
    /// The directory, absolute and as written: canonicalised at startup,
    /// not here.
    pub path: PathBuf,
    /// `readonly`: every command that writes is refused before the disk is
    /// touched (`DESIGN.md` §6).
    pub readonly: bool,
}

/// One `host` line: what HOSTAB says of a host (`DESIGN.md` §7;
/// `PROTOCOLS.md`, HOSTAB).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    /// Its Chaos address, HOSTAB's `CHAOS`.
    pub address: u16,
    /// Its names, the official first: HOSTAB's `NAME` lines. At least one.
    pub names: Vec<String>,
    /// `system=<TYPE>`, if the line gives one: HOSTAB's `SYSTEM-TYPE` ---
    /// `LISPM`, `ITS`, ... --- as written.
    pub system: Option<String>,
}

/// One `peer` line: a host whose endpoint is fixed (`DESIGN.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its Chaos address.
    pub address: u16,
    /// Where its packets go, which a packet from it does not move.
    pub endpoint: SocketAddr,
}

/// Why a site file was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The line refused, counting from 1; none when no one line is to
    /// blame --- a required line missing, or a file that could not be
    /// read.
    pub line: Option<usize>,
    /// What is wrong, beginning with what was refused.
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(n) => write!(f, "line {n}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

impl Config {
    /// The site file at `path`, read and then [`Config::parse`]d. The path
    /// is not in the error: the caller names it, `<path>: line 4: ...`.
    pub fn load(path: &Path) -> Result<Config, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error { line: None, message: e.to_string() })?;
        Config::parse(&text)
    }

    /// The site a file's text gives, or the first thing wrong with it, as
    /// the module documentation says.
    pub fn parse(text: &str) -> Result<Config, Error> {
        let mut site = Reading::default();
        for (k, line) in text.lines().enumerate() {
            let n = k + 1;
            let code = line.split_once('#').map_or(line, |(code, _)| code);
            let words: Vec<&str> = code.split_whitespace().collect();
            let Some((directive, args)) = words.split_first() else { continue };
            site.line(n, directive, args).map_err(|message| Error { line: Some(n), message })?;
        }
        site.finish()
    }
}

/// A site file as far as it has been read: each thing with the line that
/// gave it, so that a later line can name it when it is refused.
#[derive(Default)]
struct Reading {
    address: Option<(u16, usize)>,
    names: Option<(Vec<String>, usize)>,
    listen: Option<(SocketAddr, usize)>,
    roots: Vec<(Root, usize)>,
    hosts: Vec<(Host, usize)>,
    peers: Vec<(Peer, usize)>,
    /// Every host name given so far, this host's and the `host` lines'.
    named: Vec<(String, usize)>,
}

impl Reading {
    /// Line `n`, its directive and the words after it; or what is wrong.
    fn line(&mut self, n: usize, directive: &str, args: &[&str]) -> Result<(), String> {
        match directive {
            "address" => self.address(n, args),
            "name" => self.name(n, args),
            "listen" => self.listen(n, args),
            "root" => self.root(n, args),
            "host" => self.host(n, args),
            "peer" => self.peer(n, args),
            _ => Err(format!(
                "{directive}: not a directive; the directives are address, name, listen, root, host and peer"
            )),
        }
    }

    fn address(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        if let Some((_, m)) = self.address {
            return Err(format!("address: one line only, and line {m} is it"));
        }
        let [word] = args else {
            return Err("address wants one address, in octal or subnet:host".to_string());
        };
        let a = address_of("address", word)?;
        if let Some(m) = self.host_line(a) {
            return Err(format!(
                "address {a:o}: the host on line {m} is at that address; this host's names are the name line's"
            ));
        }
        if let Some(m) = self.peer_line(a) {
            return Err(format!(
                "address {a:o}: the peer on line {m} is at that address; this host is not its own peer"
            ));
        }
        self.address = Some((a, n));
        Ok(())
    }

    fn name(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        if let Some((_, m)) = self.names {
            return Err(format!("name: one line only, and line {m} is it"));
        }
        if args.is_empty() {
            return Err("name wants at least one name, the official first".to_string());
        }
        let names = self.host_names(n, args)?;
        self.names = Some((names, n));
        Ok(())
    }

    fn listen(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        if let Some((_, m)) = self.listen {
            return Err(format!("listen: one line only, and line {m} is it"));
        }
        let at = match args {
            [] => LISTEN,
            [word] => listen_at(word).ok_or_else(|| {
                format!(
                    "listen {word}: not a port, an IP address, or IP address:port; a name is not resolved"
                )
            })?,
            _ => {
                return Err("listen wants at most one endpoint: a port, an IP address, or IP address:port".to_string());
            }
        };
        self.listen = Some((at, n));
        Ok(())
    }

    fn root(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        let wants = || "root wants [<name>] <path> [readonly]".to_string();
        let (words, readonly) = match args {
            [rest @ .., "readonly"] if !rest.is_empty() => (rest, true),
            _ => (args, false),
        };
        let (name, path) = match *words {
            [path] => (None, path),
            [name, path] => (Some(name), path),
            _ => return Err(wants()),
        };
        if let Some(name) = name {
            if name.contains('/') || name == "." || name == ".." {
                return Err(format!(
                    "root {name}: not a mount's name, which is one directory name at the top of the tree"
                ));
            }
            if name.chars().any(char::is_uppercase) {
                return Err(format!(
                    "root {name}: a mount's name is lower case, as a band's pathnames are, and names match exactly"
                ));
            }
        }
        if !Path::new(path).is_absolute() {
            return Err(format!("root {}: the path is not absolute", words.join(" ")));
        }
        let earlier = self.roots.iter().find(|(r, _)| r.name.as_deref() == name);
        match (name, earlier) {
            (None, Some((_, m))) => {
                return Err(format!(
                    "root {path}: a second base; line {m} is the base, and a mount's name comes before its path"
                ));
            }
            (Some(name), Some((_, m))) => {
                return Err(format!("root {name}: mounted already, on line {m}"));
            }
            _ => {}
        }
        let root = Root { name: name.map(str::to_string), path: PathBuf::from(path), readonly };
        self.roots.push((root, n));
        Ok(())
    }

    fn host(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        let wants = || {
            "host wants an address and at least one name, and system=<TYPE> if it has one"
                .to_string()
        };
        let [word, rest @ ..] = args else { return Err(wants()) };
        let a = address_of("host", word)?;
        let (mut names, mut system) = (Vec::new(), None);
        for &w in rest {
            match w.split_once('=') {
                None => names.push(w),
                Some(("system", "")) => {
                    return Err(format!("host {word} {w}: system= wants a type, as system=LISPM"));
                }
                Some(("system", t)) => {
                    if system.replace(t.to_string()).is_some() {
                        return Err(format!("host {word}: system= twice"));
                    }
                }
                Some(_) => {
                    return Err(format!("host {word} {w}: the one attribute is system=<TYPE>"));
                }
            }
        }
        if names.is_empty() {
            return Err(wants());
        }
        if let Some((own, m)) = self.address
            && own == a
        {
            return Err(format!(
                "host {a:o}: this host's own address, line {m}; its names are the name line's"
            ));
        }
        if let Some(m) = self.host_line(a) {
            return Err(format!(
                "host {a:o}: the host on line {m} is at that address; one line a host, with all its names"
            ));
        }
        let names = self.host_names(n, &names)?;
        self.hosts.push((Host { address: a, names, system }, n));
        Ok(())
    }

    fn peer(&mut self, n: usize, args: &[&str]) -> Result<(), String> {
        let [word, lives] = *args else {
            return Err(
                "peer wants an address and an endpoint: an IP address, with a port or without"
                    .to_string(),
            );
        };
        let a = address_of("peer", word)?;
        let endpoint = peer_at(lives).ok_or_else(|| {
            format!(
                "peer {word} {lives}: not an IP address, or IP address:port; a name is not resolved"
            )
        })?;
        if endpoint.ip().is_unspecified() {
            return Err(format!(
                "peer {word} {lives}: {} is every interface, not a host to send to",
                endpoint.ip()
            ));
        }
        if endpoint.port() == 0 {
            return Err(format!("peer {word} {lives}: port 0 is no port to send to"));
        }
        if let Some((own, m)) = self.address
            && own == a
        {
            return Err(format!(
                "peer {a:o}: this host's own address, line {m}; it is not its own peer"
            ));
        }
        if let Some(m) = self.peer_line(a) {
            return Err(format!(
                "peer {a:o}: the peer on line {m} is at that address; one endpoint an address"
            ));
        }
        self.peers.push((Peer { address: a, endpoint }, n));
        Ok(())
    }

    /// `words` as host names given on line `n`, each recorded so that no
    /// later name can be it; or the first that is refused.
    fn host_names(&mut self, n: usize, words: &[&str]) -> Result<Vec<String>, String> {
        for &word in words {
            if word.contains('=') {
                return Err(format!(
                    "{word}: a name has no =, which marks a host line's attribute"
                ));
            }
            if let Some((earlier, m)) =
                self.named.iter().find(|(e, _)| e.eq_ignore_ascii_case(word))
            {
                let place = if *m == n {
                    "twice on this line".to_string()
                } else {
                    format!("already, on line {m}")
                };
                let case = if earlier == word {
                    String::new()
                } else {
                    format!(" --- as {earlier}, and HOSTAB finds a name ignoring case")
                };
                return Err(format!("{word}: named {place}{case}"));
            }
            self.named.push((word.to_string(), n));
        }
        Ok(words.iter().map(|w| w.to_string()).collect())
    }

    /// The line of the `host` at `a`, if there is one.
    fn host_line(&self, a: u16) -> Option<usize> {
        self.hosts.iter().find(|(h, _)| h.address == a).map(|&(_, m)| m)
    }

    /// The line of the `peer` at `a`, if there is one.
    fn peer_line(&self, a: u16) -> Option<usize> {
        self.peers.iter().find(|(p, _)| p.address == a).map(|&(_, m)| m)
    }

    /// The site, once every line is read: what must be there is.
    fn finish(self) -> Result<Config, Error> {
        let missing = |message: &str| Error { line: None, message: message.to_string() };
        let Some((address, _)) = self.address else {
            return Err(missing("no address line: this host's Chaos address is required"));
        };
        let Some((names, _)) = self.names else {
            return Err(missing(
                "no name line: this host's names are required, the official first",
            ));
        };
        if self.roots.is_empty() {
            return Err(missing(
                "no root line: FILE serves a root that is named and nothing else, and there is no default",
            ));
        }
        Ok(Config {
            address,
            names,
            listen: self.listen.map_or(LISTEN, |(at, _)| at),
            roots: self.roots.into_iter().map(|(r, _)| r).collect(),
            hosts: self.hosts.into_iter().map(|(h, _)| h).collect(),
            peers: self.peers.into_iter().map(|(p, _)| p).collect(),
        })
    }
}

/// `word` as a Chaos address, by [`parse_address`], or why not.
fn address_of(directive: &str, word: &str) -> Result<u16, String> {
    parse_address(word).ok_or_else(|| {
        format!(
            "{directive} {word}: not an address --- octal, or subnet:host, and neither half zero"
        )
    })
}

/// `listen`'s endpoint as muir's `--chaos-udp` reads its own (muir's
/// `endpoint_at`, `src/main.rs`): address and port are themselves, a bare
/// port is that port on the loopback, and a bare address is at 42042.
fn listen_at(word: &str) -> Option<SocketAddr> {
    if let Ok(at) = word.parse::<SocketAddr>() {
        return Some(at);
    }
    if let Ok(port) = word.parse::<u16>() {
        return Some(SocketAddr::new(LISTEN.ip(), port));
    }
    word.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, PORT))
}

/// A `peer`'s endpoint: address and port, or a bare address at 42042. No
/// bare port: a peer is a host, and a port alone names none.
fn peer_at(word: &str) -> Option<SocketAddr> {
    word.parse::<SocketAddr>()
        .ok()
        .or_else(|| word.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, PORT)))
}
