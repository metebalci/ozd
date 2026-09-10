// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The site's flags, parsed and checked (`DESIGN.md` §8): this host's
//! address and names, the UDP endpoint it binds, the roots FILE serves,
//! the site's host table for HOSTAB, and the few peers whose endpoints are
//! fixed --- given on the command line, or in a **file of flags**,
//! `.ozdrc`.
//!
//! ```text
//! --address 3060
//! --name    MIT-OZ,OZ
//! --listen  192.0.2.10
//! --root    /srv/lispm
//! --root    tree=/path/to/system-100-0/sys,ro
//! --host    3050,MIT-LISPM-1,LM1,system=LISPM
//! --host    3051,MIT-LISPM-2,LM2,system=LISPM
//! --peer    3040@192.0.2.5
//! ```
//!
//! **A value is one word**, its parts separated by commas, as in
//! `--root /srv/lispm,ro`. On the command line it is the word
//! after its flag, unless that word is itself a flag. In a file a line is a
//! flag and, after a blank, the rest of the line is its value, less the
//! blanks at either end --- so a path in a file may hold a blank without
//! quotes, and a `#` after a flag is its value's; a blank line, or one
//! beginning with `#`, is a comment. **A comma cannot be in a path**: a
//! root's path ends at its first comma, and what follows must be `ro`.
//! What each flag takes is on the field it fills: [`Config::address`],
//! [`Config::names`], [`Config::listen`], [`Config::roots`],
//! [`Config::hosts`], [`Config::peers`]; `--trace` and `--check` are the
//! run's, [`Run`].
//!
//! **The command line has the last word.** A flag it gives leaves every
//! line of that flag out of the file; the file's
//! other lines are read first and the command line after, so where one
//! flag refuses another across the two, the command line's is the one
//! refused. **A file cannot hold `-c` or `--config`** --- which file is
//! read is the command line's to say, and one that could name another could
//! name itself --- **nor `--check`, `-h` or `--help`**: each is
//! what one run is asked to do, and in a file every run would do it and
//! exit 0, the daemon never serving and a service manager seeing a clean
//! exit. `--trace` may be in one: how much is printed is a standing
//! choice.
//!
//! **Two kinds of refusal**, told apart by [`Error::usage`]. A *usage
//! error* is the shape of what was given, found by [`Flags::command_line`]
//! and [`Flags::file`]: a word that is not a flag, an argument that is no
//! flag's value, a flag missing its value or given one it does not take,
//! `-c` twice, and in a file the flags above. The rest is found by
//! [`Run::new`], the first in the order read, with where it was given
//! ([`Place`]): a value that is not one --- an address [`parse_address`]
//! refuses, an endpoint that is not an IP literal, a root path that is not
//! absolute, a part of `--name` or `--host` with `=` other than one
//! `system=` with a type; an `--address`, `--name` or `--listen` given
//! twice, a second base, and a mount's name twice; an address that would be
//! two answers, a host name given twice, a system type that is not upper
//! case, and a name, a system type or a mount's name that is not printable
//! ASCII (below); and, with no place, a required flag missing ---
//! `--address`, `--name`, and at least one `--root`. None of it touches
//! the disk: whether a root exists, is a directory and can be written is
//! the startup's to check (`DESIGN.md` §6), and which file of flags is read
//! is `main`'s to find.
//!
//! **An address is one host, and one endpoint.** The host table and the
//! endpoints are separate (`DESIGN.md` §8), so an address is looked for in
//! two places, and it may be in each once: among `--address` and the
//! `--host`s, for what a host is called, and among `--address` and the
//! `--peer`s, for where its packets go. So this host's own address has no
//! `--host` --- its names are `--name`'s --- and no `--peer`, this host not
//! being its own peer; two `--host`s at one address are two answers to what
//! it is called, and two `--peer`s two answers to where it is. A `--host`
//! and a `--peer` at one address are not
//! ambiguous: they answer different questions about one host, and are how
//! a host is both named and fixed. Whichever is read later is refused,
//! naming the earlier.
//!
//! **A host name is one host.** No name is given twice among this host's
//! and every `--host`'s, and two that differ only in case are the same
//! name: HOSTAB looks a name up ignoring case (`DESIGN.md` §7), and would
//! otherwise find two hosts for it, or one host twice.
//!
//! **A system type is upper case**, in `--name` and `--host` alike: no
//! lower-case letter in it. HOSTAB sends it as written, and the band's user
//! end interns it as sent (`sys/network/chaos/chuse.lisp:983`) and picks
//! the host's flavor by that keyword (`COMPUTE-HOST-FLAVOR`,
//! `sys/network/host.lisp:279-283`): `system=lispm` would be `:|lispm|`,
//! which no flavor is filed under. Nothing more is asked of it. A type the
//! band files no flavor under gets its `:DEFAULT` one (`host.lisp:356`), as
//! a host with no type does --- `WAITS`, which its SUPDUP asks after
//! (`sys/window/supdup.lisp:968`), is one --- so the types the band knows
//! are not a list the flags keep to.
//!
//! **A name, a system type and a mount's name are printable ASCII**, `!`
//! to `~`. HOSTAB sends a name, and FILE a mount's name in a listing of
//! `/`, a byte a character ([`crate::lispm::lispm_text`]): a character
//! below 256 is that byte, and U+008D would go out as 215 octal, the band's
//! newline, in the middle of an answer. The band's own tables hold nothing
//! else. So no name holds a blank, and names are parted by commas.

use crate::address::parse_address;
use crate::chudp::PORT;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

/// Where the socket is bound without `--listen`: the loopback at CHUDP's
/// port, so that a fresh install answers its own host and nothing else
/// (`DESIGN.md` §5).
const LISTEN: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT));

/// Where a required flag may be given, as its refusal says.
const WHERE: &str = "on the command line or in a file of flags";

/// What an argument that is no flag's value is told: ozd takes flags
/// and nothing else, and a file given as its one argument, as the config
/// once was, meets this.
const FLAGS_ONLY: &str = "not a flag, and ozd takes nothing else: the config is flags, \
     on the command line or in .ozdrc, or in the file -c <file> names";

/// The site, as its flags give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `--address <addr>`, required, once: this host's Chaos address, in
    /// octal or as `subnet:host`, as [`parse_address`] takes it --- which
    /// refuses a zero half, a zero host being every host of its subnet.
    pub address: u16,
    /// `--name <NAME>[,<NAME>...][,system=<TYPE>]`, required, once: this
    /// host's names, the official first --- the one STATUS answers with,
    /// and HOSTAB's first `NAME` (`DESIGN.md` §7). At least one. A part
    /// with `=` in it is an attribute and not a name, wherever it is among
    /// them, as in `--host`, and `system` is the one there is:
    /// [`Config::system`]. As given; HOSTAB compares them ignoring case.
    pub names: Vec<String>,
    /// `system=<TYPE>` in `--name`, if it gives one: this host's system
    /// type, HOSTAB's `SYSTEM-TYPE` for this host's own names (`DESIGN.md`
    /// §7), as [`Host::system`] is for a `--host`'s. Upper case (the
    /// module documentation), and otherwise as given.
    pub system: Option<String>,
    /// `--listen <endpoint>`, at most once: the UDP endpoint the socket
    /// binds, against the loopback at 42042:
    ///
    /// - no `--listen`: `127.0.0.1:42042`;
    /// - a bare port, `42043`: on the loopback, `127.0.0.1:42043`;
    /// - a bare address, `192.0.2.10` or `::1`: at 42042;
    /// - address and port, `192.0.2.10:42043` or `[::1]:42043`: as given;
    /// - `0.0.0.0` or `::`, with a port or without: every interface, on
    ///   purpose (`DESIGN.md` §5).
    ///
    /// IPv6 is written bare, or in brackets before a port. **IP literals
    /// only**: a name would be resolved once at startup and not followed
    /// afterwards, which nothing here does (`DESIGN.md` §8). Port 0 is a
    /// port the system picks, which a test on the loopback wants and a site
    /// does not. `--listen` always takes an endpoint, and the default is no
    /// `--listen` at all.
    pub listen: SocketAddr,
    /// `--root <path>[,ro]` and `--root <name>=<path>[,ro]`, once a root:
    /// the tree FILE serves (`DESIGN.md` §6), in the order given. A value
    /// beginning with `/` is the **base root**, at most one, its path the
    /// value up to its first comma; any other is a root **mounted** at the
    /// top level, its name before the first `=` and its path after, each
    /// name once. `,ro` after the path is [`Root::readonly`]. At least one
    /// root, and mounts without a base are a site: `/` is then read-only
    /// and names only the mounts.
    ///
    /// - **A path is absolute**, and that is all that is checked of it
    ///   here; existing, being a directory, not being `/`, and being
    ///   writable without `,ro` are the startup's checks, made when it
    ///   canonicalises each root (`DESIGN.md` §6).
    /// - **A path cannot hold a comma**, which ends it: after the path
    ///   comes `,ro` or nothing. It may hold a blank, and `=`.
    /// - **A mount's name is one directory name, in lower case.** It is the
    ///   first component of a pathname under `/`, so it is not empty, holds
    ///   no `/`, and is neither `.` nor `..`; and names match exactly while
    ///   a band sends its pathnames in lower case (`DESIGN.md` §6;
    ///   `/tree/...` in System 100's `sys/site/sys.translations`), so a
    ///   mount named in capitals is one no band would reach.
    pub roots: Vec<Root>,
    /// `--host <addr>,<NAME>[,<NAME>...][,system=<TYPE>]`, once a host: the
    /// site's host table, which HOSTAB answers from beside this host's own
    /// names (`DESIGN.md` §7), in the order given. This host is not in it;
    /// its names are [`Config::names`]. A part with `=` in it is an
    /// attribute, wherever it is after the address, and `system` is the one
    /// there is.
    pub hosts: Vec<Host>,
    /// `--peer <addr>@<ip>[:<port>]`, once a peer: a host whose endpoint is
    /// fixed, so that a packet does not move it; every other endpoint is
    /// learned from the packets a host sends (`DESIGN.md` §5). An IP
    /// literal after the `@`, as `--listen` takes one; at 42042 unless a
    /// port is given. Not a bare port, since a peer is a host to send to
    /// and not a socket to bind; not `0.0.0.0` or `::`, which are every
    /// interface and no host; and not port 0, which nothing can be sent to.
    pub peers: Vec<Peer>,
}

/// One `--root` (`DESIGN.md` §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Root {
    /// The name it is mounted under at the top level; none for the base.
    pub name: Option<String>,
    /// The directory, absolute and as given: canonicalised at startup,
    /// not here.
    pub path: PathBuf,
    /// `,ro`: every command that writes is refused before the disk is
    /// touched (`DESIGN.md` §6).
    pub readonly: bool,
}

/// One `--host`: what HOSTAB says of a host (`DESIGN.md` §7;
/// `PROTOCOLS.md`, HOSTAB).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    /// Its Chaos address, HOSTAB's `CHAOS`.
    pub address: u16,
    /// Its names, the official first: HOSTAB's `NAME` lines. At least one.
    pub names: Vec<String>,
    /// `system=<TYPE>`, if the `--host` gives one: HOSTAB's `SYSTEM-TYPE`
    /// --- `LISPM`, `ITS`, ... --- upper case (the module documentation),
    /// and otherwise as given.
    pub system: Option<String>,
}

/// One `--peer`: a host whose endpoint is fixed (`DESIGN.md` §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its Chaos address.
    pub address: u16,
    /// Where its packets go, which a packet from it does not move.
    pub endpoint: SocketAddr,
}

/// Where a flag was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// On the command line.
    CommandLine,
    /// On this line of the file of flags, counting from 1.
    Line(usize),
}

/// Why what was given was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Where the flag refused was given; none when no one flag is to blame
    /// --- a required flag missing.
    pub place: Option<Place>,
    /// Whether it is a usage error, the shape of what was given, which
    /// `main` answers with the usage and exit code 2; otherwise a value
    /// refused, or a flag missing, and exit code 1.
    pub usage: bool,
    /// What is wrong: for a value refused, `<flag> <value>: ` and then what
    /// is wrong with it.
    pub message: String,
}

impl fmt::Display for Error {
    /// `line N: ` and the message for a line of a file of flags, whose
    /// name the caller puts before it; the message alone otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.place {
            Some(Place::Line(n)) => write!(f, "line {n}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

/// Every flag ozd takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    Address,
    Name,
    Listen,
    Root,
    Host,
    Peer,
    Trace,
    Check,
    Config,
    Help,
}

impl Flag {
    const ALL: [Flag; 10] = [
        Flag::Address,
        Flag::Name,
        Flag::Listen,
        Flag::Root,
        Flag::Host,
        Flag::Peer,
        Flag::Trace,
        Flag::Check,
        Flag::Config,
        Flag::Help,
    ];

    /// The flag `word` is, if it is one: its long name, or `-c` or `-h`.
    fn of(word: &str) -> Option<Flag> {
        match word {
            "-c" => Some(Flag::Config),
            "-h" => Some(Flag::Help),
            _ => Flag::ALL.into_iter().find(|f| f.name() == word),
        }
    }

    /// Its long name, as a refusal of its value names it.
    fn name(self) -> &'static str {
        match self {
            Flag::Address => "--address",
            Flag::Name => "--name",
            Flag::Listen => "--listen",
            Flag::Root => "--root",
            Flag::Host => "--host",
            Flag::Peer => "--peer",
            Flag::Trace => "--trace",
            Flag::Check => "--check",
            Flag::Config => "--config",
            Flag::Help => "--help",
        }
    }

    /// Its value, as the usage writes it; none for a flag that takes none.
    fn wants(self) -> Option<&'static str> {
        match self {
            Flag::Address => Some("<addr>"),
            Flag::Name => Some("<NAME>[,<NAME>...][,system=<TYPE>]"),
            Flag::Listen => Some("<endpoint>"),
            Flag::Root => Some("<path>[,ro] or <name>=<path>[,ro]"),
            Flag::Host => Some("<addr>,<NAME>[,<NAME>...][,system=<TYPE>]"),
            Flag::Peer => Some("<addr>@<ip>[:<port>]"),
            Flag::Config => Some("<file>"),
            Flag::Trace | Flag::Check | Flag::Help => None,
        }
    }
}

/// One flag as given: where, which, and its value --- empty for a flag
/// that takes none.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Given {
    place: Place,
    flag: Flag,
    value: String,
}

/// The flags one place gives, the command line or a file of flags, in the
/// order given: each a flag ozd takes, with a value if it wants one.
/// Nothing of a value is checked here but that it is there; [`Run::new`]
/// checks the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    given: Vec<Given>,
}

impl Flags {
    /// The command line's `words`, the program's name not among them, as
    /// flags; or the first usage error. A flag's value is the word after
    /// it, whatever it is --- unless that word is one of the
    /// flags, when the flag has no value: `--address --name OZ` is
    /// `--address` missing its value, not an address named `--name`.
    pub fn command_line<S: AsRef<str>>(words: &[S]) -> Result<Flags, Error> {
        let misused =
            |message: String| Error { place: Some(Place::CommandLine), usage: true, message };
        let mut given: Vec<Given> = Vec::new();
        let mut words = words.iter().map(|w| -> &str { w.as_ref() }).peekable();
        while let Some(word) = words.next() {
            let flag = match Flag::of(word) {
                Some(flag) => flag,
                None if word.starts_with('-') => {
                    return Err(misused(format!("{word}: not a flag")));
                }
                None => return Err(misused(format!("{word}: {FLAGS_ONLY}"))),
            };
            let value = match flag.wants() {
                None => String::new(),
                Some(wants) => match words.next_if(|next| Flag::of(next).is_none()) {
                    Some(value) => value.to_string(),
                    None => return Err(misused(format!("{word} wants {wants}"))),
                },
            };
            if flag == Flag::Config && given.iter().any(|g| g.flag == Flag::Config) {
                return Err(misused(format!("{word}: one file of flags, not two")));
            }
            given.push(Given { place: Place::CommandLine, flag, value });
        }
        Ok(Flags { given })
    }

    /// The text of a file of flags as flags, each with its line; or the
    /// first usage error, with its line. A line is a flag and, after a
    /// blank, the rest of the line is its value, less the blanks at either
    /// end; a blank line, or one beginning with `#`, is a comment. `-c`,
    /// `--config`, `--check`, `-h` and `--help` are refused (the module
    /// documentation).
    pub fn file(text: &str) -> Result<Flags, Error> {
        let mut given = Vec::new();
        for (k, line) in text.lines().enumerate() {
            let n = k + 1;
            let misused =
                |message: String| Error { place: Some(Place::Line(n)), usage: true, message };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, value) = match line.split_once(char::is_whitespace) {
                Some((word, value)) => (word, value.trim()),
                None => (line, ""),
            };
            let Some(flag) = Flag::of(word) else {
                // A word that is a flag with `--` before it: what a line
                // without its dashes meant.
                let meant = Flag::of(&format!("--{word}")).filter(|_| !word.starts_with('-'));
                let hint = meant.map_or(String::new(), |f| format!("; {} is", f.name()));
                return Err(misused(format!("{word}: not a flag{hint}")));
            };
            match flag {
                Flag::Config => {
                    return Err(misused(format!(
                        "{word}: a file of flags cannot name another; which file is read is the command line's to say"
                    )));
                }
                Flag::Check | Flag::Help => {
                    return Err(misused(format!(
                        "{word}: the command line's, not a file's: it is what one run is asked to do, and in a file every run would do it and exit, and never serve"
                    )));
                }
                _ => {}
            }
            match flag.wants() {
                Some(wants) if value.is_empty() => {
                    return Err(misused(format!("{word} wants {wants}")));
                }
                None if !value.is_empty() => {
                    return Err(misused(format!("{word} takes no value")));
                }
                _ => {}
            }
            given.push(Given { place: Place::Line(n), flag, value: value.to_string() });
        }
        Ok(Flags { given })
    }

    /// The file of flags `-c` or `--config` names, if these flags give
    /// one: only the command line's can.
    pub fn config(&self) -> Option<&Path> {
        self.given.iter().find(|g| g.flag == Flag::Config).map(|g| Path::new(&g.value))
    }

    /// Whether `flag` is among these.
    fn gives(&self, flag: Flag) -> bool {
        self.given.iter().any(|g| g.flag == flag)
    }
}

/// What one run is given: the site, and what the run does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The site.
    pub config: Config,
    /// `--trace`: print every packet, every packet passed on, and every
    /// drop, with why (`DESIGN.md` §10).
    pub trace: bool,
    /// `--check`: check the flags and the roots, bind nothing, and exit
    /// (`DESIGN.md` §10).
    pub check: bool,
}

impl Run {
    /// The run that the command line's flags, `typed`, and a file's,
    /// `file`, give together; or the first value refused, or the first
    /// flag required that neither gives.
    ///
    /// **A flag `typed` gives leaves every line of that flag out of
    /// `file`**, and `file`'s other lines are read first and `typed` after:
    /// the command line has the last word.
    pub fn new(typed: &Flags, file: &Flags) -> Result<Run, Error> {
        let read = file.given.iter().filter(|g| !typed.gives(g.flag)).chain(&typed.given);
        let mut reading = Reading::default();
        for (i, g) in read.enumerate() {
            reading.take(i, g).map_err(|what| Error {
                place: Some(g.place),
                usage: false,
                message: format!("{} {}: {what}", g.flag.name(), g.value),
            })?;
        }
        reading.finish()
    }
}

impl Config {
    /// The site the text of a file of flags gives, alone --- as a run with
    /// nothing on its command line reads it --- or the first thing wrong
    /// with it, as the module documentation says. `--trace` in it is the
    /// run's, [`Run::new`]'s, and not the site's.
    pub fn parse(text: &str) -> Result<Config, Error> {
        Ok(Run::new(&Flags::default(), &Flags::file(text)?)?.config)
    }
}

/// A host name given, and where: so that a later one can name it when it
/// is refused.
struct Named {
    name: String,
    /// `--name` or `--host`.
    by: Flag,
    place: Place,
    /// Which of the flags read gave it, counting from 0: two names in one
    /// flag's value are one name given twice there.
    index: usize,
}

/// The site as far as its flags have been read: each thing with where it
/// was given, so that a later flag can name it when it is refused.
#[derive(Default)]
struct Reading {
    address: Option<(u16, Place)>,
    names: Option<(Vec<String>, Place)>,
    /// This host's system type, from `--name`.
    system: Option<String>,
    listen: Option<(SocketAddr, Place)>,
    roots: Vec<(Root, Place)>,
    hosts: Vec<(Host, Place)>,
    peers: Vec<(Peer, Place)>,
    /// Every host name given so far, this host's and every `--host`'s.
    named: Vec<Named>,
    trace: bool,
    check: bool,
}

/// Where an earlier flag was given, as the refusal of one given at `here`
/// says it: `on line 4`; `on line 4 of the file`, from the command line;
/// or `on the command line`.
fn on(earlier: Place, here: Place) -> String {
    match (earlier, here) {
        (Place::Line(m), Place::Line(_)) => format!("on line {m}"),
        (Place::Line(m), Place::CommandLine) => format!("on line {m} of the file"),
        (Place::CommandLine, _) => "on the command line".to_string(),
    }
}

impl Reading {
    /// The `i`th flag read, `g`; or what is wrong with its value.
    fn take(&mut self, i: usize, g: &Given) -> Result<(), String> {
        let (here, value) = (g.place, g.value.as_str());
        match g.flag {
            Flag::Address => self.address(here, value),
            Flag::Name => self.name(i, here, value),
            Flag::Listen => self.listen(here, value),
            Flag::Root => self.root(here, value),
            Flag::Host => self.host(i, here, value),
            Flag::Peer => self.peer(here, value),
            Flag::Trace => {
                self.trace = true;
                Ok(())
            }
            Flag::Check => {
                self.check = true;
                Ok(())
            }
            // `main`'s, before any of this: the help before anything, and
            // the file before it is read.
            Flag::Config | Flag::Help => Ok(()),
        }
    }

    fn address(&mut self, here: Place, value: &str) -> Result<(), String> {
        if let Some((_, p)) = self.address {
            return Err(format!("once only, and given {} already", on(p, here)));
        }
        let a = address_of(value)?;
        if let Some(p) = self.host_place(a) {
            return Err(format!(
                "the --host {} is at that address; this host's names are --name's",
                on(p, here)
            ));
        }
        if let Some(p) = self.peer_place(a) {
            return Err(format!(
                "the --peer {} is at that address; this host is not its own peer",
                on(p, here)
            ));
        }
        self.address = Some((a, here));
        Ok(())
    }

    fn name(&mut self, i: usize, here: Place, value: &str) -> Result<(), String> {
        if let Some((_, p)) = self.names {
            return Err(format!("once only, and given {} already", on(p, here)));
        }
        let parts: Vec<&str> = value.split(',').collect();
        let (names, system) = names_and_system(&parts)?;
        if names.is_empty() {
            return Err("wants at least one name, the official first".to_string());
        }
        let names = self.host_names(Flag::Name, i, here, &names)?;
        self.names = Some((names, here));
        self.system = system;
        Ok(())
    }

    fn listen(&mut self, here: Place, value: &str) -> Result<(), String> {
        if let Some((_, p)) = self.listen {
            return Err(format!("once only, and given {} already", on(p, here)));
        }
        let at = listen_at(value)
            .ok_or("not a port, an IP address, or IP address:port; a name is not resolved")?;
        self.listen = Some((at, here));
        Ok(())
    }

    fn root(&mut self, here: Place, value: &str) -> Result<(), String> {
        let mut parts = value.split(',');
        let spec = parts.next().unwrap_or("");
        let (name, path) = match spec.split_once('=') {
            _ if spec.starts_with('/') => (None, spec),
            Some((name, path)) => (Some(name), path),
            None => {
                return Err("the path is not absolute, and a mount is <name>=<path>".to_string());
            }
        };
        if let Some(name) = name {
            mount_name(name)?;
        }
        if !Path::new(path).is_absolute() {
            return Err("the path is not absolute".to_string());
        }
        let mut readonly = false;
        for part in parts {
            if part != "ro" {
                return Err(format!(
                    "{part:?} after a comma is not ro, and a path cannot hold a comma, which ends it"
                ));
            }
            if std::mem::replace(&mut readonly, true) {
                return Err("ro twice".to_string());
            }
        }
        match (name, self.roots.iter().find(|(r, _)| r.name.as_deref() == name)) {
            (None, Some(&(_, p))) => {
                return Err(format!(
                    "a second base, and the base is given {}; a mount is <name>=<path>",
                    on(p, here)
                ));
            }
            (Some(name), Some(&(_, p))) => {
                return Err(format!("{name} is mounted already, {}", on(p, here)));
            }
            _ => {}
        }
        let root = Root { name: name.map(str::to_string), path: PathBuf::from(path), readonly };
        self.roots.push((root, here));
        Ok(())
    }

    fn host(&mut self, i: usize, here: Place, value: &str) -> Result<(), String> {
        let mut parts = value.split(',');
        let word = parts.next().unwrap_or("");
        let a = address_of(word).map_err(|e| format!("{word}: {e}"))?;
        let rest: Vec<&str> = parts.collect();
        let (names, system) = names_and_system(&rest)?;
        if names.is_empty() {
            return Err(
                "wants an address and at least one name, parted by commas, and system=<TYPE> if it has one"
                    .to_string(),
            );
        }
        if let Some((own, p)) = self.address
            && own == a
        {
            return Err(format!(
                "this host's own address, --address {}; its names are --name's",
                on(p, here)
            ));
        }
        if let Some(p) = self.host_place(a) {
            return Err(format!(
                "the --host {} is at that address; one --host a host, with all its names",
                on(p, here)
            ));
        }
        let names = self.host_names(Flag::Host, i, here, &names)?;
        self.hosts.push((Host { address: a, names, system }, here));
        Ok(())
    }

    fn peer(&mut self, here: Place, value: &str) -> Result<(), String> {
        let (word, lives) = value.split_once('@').ok_or(
            "wants <addr>@<ip>[:<port>]: the address in octal or subnet:host, and an IP address after the @",
        )?;
        let a = address_of(word).map_err(|e| format!("{word}: {e}"))?;
        let endpoint = peer_endpoint(lives)
            .ok_or("not an IP address, or IP address:port, after the @; a name is not resolved")?;
        if endpoint.ip().is_unspecified() {
            return Err(format!("{} is every interface, not a host to send to", endpoint.ip()));
        }
        if endpoint.port() == 0 {
            return Err("port 0 is no port to send to".to_string());
        }
        if let Some((own, p)) = self.address
            && own == a
        {
            return Err(format!(
                "this host's own address, --address {}; it is not its own peer",
                on(p, here)
            ));
        }
        if let Some(p) = self.peer_place(a) {
            return Err(format!(
                "the --peer {} is at that address; one endpoint an address",
                on(p, here)
            ));
        }
        self.peers.push((Peer { address: a, endpoint }, here));
        Ok(())
    }

    /// `words` as host names given by the `i`th flag read, `by`, at `here`,
    /// each recorded so that no later name can be it; or the first that is
    /// refused. None has an `=`: [`names_and_system`] has taken those as
    /// attributes.
    fn host_names(
        &mut self,
        by: Flag,
        i: usize,
        here: Place,
        words: &[&str],
    ) -> Result<Vec<String>, String> {
        for &word in words {
            if let Some(e) = self.named.iter().find(|e| e.name.eq_ignore_ascii_case(word)) {
                let when = if e.index == i {
                    "twice here".to_string()
                } else {
                    format!("already, by {} {}", e.by.name(), on(e.place, here))
                };
                let case = if e.name == word {
                    String::new()
                } else {
                    format!(" --- as {}, and HOSTAB finds a name ignoring case", e.name)
                };
                return Err(format!("{word} is named {when}{case}"));
            }
            self.named.push(Named { name: word.to_string(), by, place: here, index: i });
        }
        Ok(words.iter().map(|w| w.to_string()).collect())
    }

    /// Where the `--host` at `a` was given, if there is one.
    fn host_place(&self, a: u16) -> Option<Place> {
        self.hosts.iter().find(|(h, _)| h.address == a).map(|&(_, p)| p)
    }

    /// Where the `--peer` at `a` was given, if there is one.
    fn peer_place(&self, a: u16) -> Option<Place> {
        self.peers.iter().find(|(p, _)| p.address == a).map(|&(_, p)| p)
    }

    /// The run, once every flag is read: what must be there is.
    fn finish(self) -> Result<Run, Error> {
        let missing = |message: String| Error { place: None, usage: false, message };
        let Some((address, _)) = self.address else {
            return Err(missing(format!(
                "no --address: this host's Chaos address is required, {WHERE}"
            )));
        };
        let Some((names, _)) = self.names else {
            return Err(missing(format!(
                "no --name: this host's names are required, the official first, {WHERE}"
            )));
        };
        if self.roots.is_empty() {
            return Err(missing(format!(
                "no --root: FILE serves a root that is named and nothing else, and there is no default; one is required, {WHERE}"
            )));
        }
        let config = Config {
            address,
            names,
            system: self.system,
            listen: self.listen.map_or(LISTEN, |(at, _)| at),
            roots: self.roots.into_iter().map(|(r, _)| r).collect(),
            hosts: self.hosts.into_iter().map(|(h, _)| h).collect(),
            peers: self.peers.into_iter().map(|(p, _)| p).collect(),
        };
        Ok(Run { config, trace: self.trace, check: self.check })
    }
}

/// The parts of `--name`'s value, or of `--host`'s after its address, as
/// names and a system type; or the first that is refused. A part with `=`
/// in it is an attribute, and `system=<TYPE>` the one there is: once, with
/// a type, in upper case (the module documentation). A part is not empty,
/// and a name is printable ASCII, so holds no blank.
fn names_and_system<'a>(parts: &[&'a str]) -> Result<(Vec<&'a str>, Option<String>), String> {
    let (mut names, mut system) = (Vec::new(), None);
    for &w in parts {
        match w.split_once('=') {
            None if w.is_empty() => return Err("an empty part: a comma too many".to_string()),
            None if w.contains(char::is_whitespace) => {
                return Err(format!(
                    "{w:?} holds a blank, and no name does: names are separated by commas"
                ));
            }
            None if !w.chars().all(|c| c.is_ascii_graphic()) => {
                return Err(format!(
                    "{w:?}: not printable ASCII, which a name is (the module documentation)"
                ));
            }
            None => names.push(w),
            Some(("system", "")) => {
                return Err("system= wants a type, as system=LISPM".to_string());
            }
            Some(("system", t)) if !t.chars().all(|c| c.is_ascii_graphic()) => {
                return Err(format!(
                    "{w:?}: not printable ASCII, which a system type is (the module documentation)"
                ));
            }
            Some(("system", t)) if t.chars().any(char::is_lowercase) => {
                return Err(format!(
                    "{w}: a system type is upper case, as system={}, since the band takes it as sent",
                    t.to_uppercase()
                ));
            }
            Some(("system", t)) => {
                if system.replace(t.to_string()).is_some() {
                    return Err("system= twice".to_string());
                }
            }
            Some(_) => return Err(format!("{w}: the one attribute is system=<TYPE>")),
        }
    }
    Ok((names, system))
}

/// A mount's name, or why it is not one ([`Config::roots`]).
fn mount_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(format!(
            "{name:?} is not a mount's name, which is one directory name at the top of the tree"
        ));
    }
    if !name.chars().all(|c| c.is_ascii_graphic()) {
        return Err(format!(
            "{name:?}: not printable ASCII, which a mount's name is (the module documentation)"
        ));
    }
    if name.chars().any(char::is_uppercase) {
        return Err(format!(
            "{name}: a mount's name is lower case, as a band's pathnames are, and names match exactly"
        ));
    }
    Ok(())
}

/// `word` as a Chaos address, by [`parse_address`], or why not.
fn address_of(word: &str) -> Result<u16, String> {
    parse_address(word).ok_or_else(|| {
        "not an address --- octal, or subnet:host, and neither half zero".to_string()
    })
}

/// `--listen`'s endpoint: address and port are themselves, a bare port is
/// that port on the loopback, and a bare address is at 42042.
fn listen_at(word: &str) -> Option<SocketAddr> {
    if let Ok(at) = word.parse::<SocketAddr>() {
        return Some(at);
    }
    if let Ok(port) = word.parse::<u16>() {
        return Some(SocketAddr::new(LISTEN.ip(), port));
    }
    word.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, PORT))
}

/// A `--peer`'s endpoint, after its `@`: address and port, or a bare
/// address at 42042. No bare port: a peer is a host, and a port alone names
/// none.
fn peer_endpoint(word: &str) -> Option<SocketAddr> {
    word.parse::<SocketAddr>()
        .ok()
        .or_else(|| word.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, PORT)))
}
