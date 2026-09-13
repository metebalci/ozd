// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `ozd` daemon: its command line, its startup, and its loop
//! (`DESIGN.md` §4, §10).
//!
//! ```text
//! ozd [--address <addr>] [--name <NAME>[,<NAME>...][,system=<TYPE>]]
//!         [--listen <endpoint>] [--root [<name>=]<path>[,ro]]
//!         [--host <addr>,<NAME>[,<NAME>...][,system=<TYPE>]]
//!         [--hosts-text <file>] [--peer <addr>@<ip>[:<port>]]
//!         [--trace] [--log-simple] [--log-file] [--log-file-probe] [--check]
//!         [-c|--config <file>] [-h|--help]
//! ```
//!
//! The site is flags (`ozd::config`), on the command line or in a file
//! of flags: the one `-c` or `--config` names, which must be there; else
//! the one the environment variable `OZD_RC` names; else `.ozdrc`
//! in the directory ozd is run from; else `.ozdrc` in `$HOME` ---
//! the first of those, not all of them, and one looked for need not be
//! there. A flag the command line gives leaves that flag's lines out of
//! the file.
//!
//! `-h` or `--help`, anywhere on the line, prints the usage and what each
//! flag is on stdout, and exits 0, before anything else is
//! looked at. `--trace` prints every packet, every packet passed on, and
//! every drop with why. `--check` reads the flags and runs the startup
//! checks, binds nothing, and exits 0 if all is well and 1 if not.
//!
//! **What is refused, in the order it is found.** The shape of the command
//! line, then of the file of flags: a usage error, what is wrong and the
//! usage on stderr, exit 2. Then who is running it: not root. Then each
//! value, in the order read, and the flags required: exit 1, a value
//! refused printed as `<file>: line N: <flag> <value>: <what>` for a line
//! of the file and `<flag> <value>: <what>` for the command line. Then the
//! roots. A daemon that starts runs until it is stopped --- `SIGTERM`'s
//! default action is the shutdown (`DESIGN.md` §10).

use ozd::config::{Error, Flags, Place, Run};
use ozd::daemon::Daemon;
use ozd::hosts_text;
use ozd::log;
use ozd::roots::Tree;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What a file of flags is called where ozd looks for one.
const RC: &str = ".ozdrc";

/// The environment variable that names a file of flags in place of the two
/// looked for.
const RC_NAMED: &str = "OZD_RC";

const USAGE: &str = "usage: ozd [--address <addr>] [--name <NAME>[,<NAME>...][,system=<TYPE>]]
           [--listen <endpoint>] [--root [<name>=]<path>[,ro]]
           [--host <addr>,<NAME>[,<NAME>...][,system=<TYPE>]]
           [--hosts-text <file>] [--peer <addr>@<ip>[:<port>]]
           [--trace] [--log-simple] [--log-file] [--log-file-probe] [--check]
           [-c|--config <file>] [-h|--help]";

/// What `-h` and `--help` print after the usage: what this is, then each
/// flag in the order the usage gives them, then the file of flags and how
/// it runs.
const HELP: &str = "\
The associated machine for a site of MIT CADR Lisp Machines: one host on
one Chaosnet subnet, over UDP, serving the machines their files, their
time and their host table, and passing packets between them.

  --address <addr>             this host's Chaosnet address: the sixteen
                               bits in octal, 3060, or subnet:host with
                               each half in octal, 6:60, neither half zero.
                               Required.
  --name <NAME>[,<NAME>...][,system=<TYPE>]
                               this host's names, the official first ---
                               the one STATUS answers with --- and its
                               system type for HOSTAB, in upper case:
                               MIT-OZ,OZ,system=UNIX. A name is printable
                               ASCII, and none is given twice here or in a
                               --host, ignoring case. Required.
  --listen <endpoint>          where the socket is bound: a port, on the
                               loopback; an IP address, at 42042; or IP
                               address:port, IPv6 in brackets before the
                               port. 0.0.0.0 or :: is every interface. A
                               name is not resolved.
                               [default: 127.0.0.1:42042, this host alone]
  --root <path>[,ro]           the base root FILE serves, an absolute path:
                               the homes, and whatever no mount covers.
                               With ,ro every write in it is refused.
  --root <name>=<path>[,ro]    a root mounted at /<name>, the name one
                               lower-case directory name: how a directory
                               elsewhere is served, as a band's sources at
                               /tree or /sys, read-only with ,ro. The flag
                               can come more than once, a base and a mount
                               a name; at least one --root is required, and
                               with mounts and no base, / is read-only. A
                               path cannot hold a comma, which ends it.
  --host <addr>,<NAME>[,<NAME>...][,system=<TYPE>]
                               a host of the site's table, which HOSTAB
                               answers from: its address, its names, the
                               official first, and its system type, as
                               3050,MIT-LISPM-1,LM1,system=LISPM. The flag
                               can come more than once, a host a flag, and
                               never at this host's address.
  --hosts-text <file>          a band's own host table, sys/site/hosts.text,
                               whose hosts HOSTAB answers for as well: the
                               site writes a host once, where its bands read
                               it, and a --host adds what that file does not
                               hold. Its HOST lines are read, a host with no
                               Chaosnet address is skipped, and every other
                               line is; the file is read at startup, so a
                               change to it wants a restart.
  --peer <addr>@<ip>[:<port>]  a host whose endpoint is fixed, so that a
                               packet does not move it: its address and an
                               IP address, 3040@192.0.2.5, at 42042 unless
                               a port is given. The flag can come more
                               than once, a peer an address; every other
                               endpoint is learned from the packets a host
                               sends.
  --trace                      print every packet, every packet passed on to
                               another host, and every drop, with why.
  --log-simple                 log each simple transaction answered, STATUS,
                               TIME or UPTIME, as \"TIME from 3050 answered\".
                               A band asks STATUS of every host at each
                               (hostat), so this is asked for on its own.
  --log-file                   log what FILE serves, not only what it
                               changes: a line for each file read, each
                               directory listed and each LOGIN, with the
                               client's address, as a write's line has it.
                               A band's boot is a few hundred lines.
  --log-file-probe             log each FILE PROBE as well. A band probes far
                               more often than it reads, and serves no file
                               by it, so it is asked for on its own.
  --check                      check the flags and the roots, then exit, 0
                               if all is well and 1 if not; binds nothing and
                               changes nothing.
  -c, --config <file>          the file of flags to read, which must be
                               there. Without it ozd reads the file
                               OZD_RC names, or failing that .ozdrc
                               in the directory it was run from, or failing
                               that .ozdrc in the home directory --- the
                               first of those, not all of them; one looked
                               for need not be there.
  -h, --help                   print this, and exit.

A file of flags is a flag a line and, after a blank, its value: the rest of
the line, blanks and # and all. A blank line, or one that begins with #, is
a comment. A flag the command line gives leaves that flag's lines out of the
file. --trace may be in a file; --check, --help and --config may not.

It will not run as root. It logs to stderr, a line an event, stamped in
UTC. What is not flags, on the command line or in the file, exits 2, a
daemon that cannot start exits 1, and one that starts runs until it is
stopped.";

/// How long the loop waits for a datagram before it turns anyway: an idle
/// daemon wakes ten times a second, which costs nothing measurable, and a
/// retransmission is at most this late against its 500 ms (`DESIGN.md`
/// §4).
const WAIT: Duration = Duration::from_millis(100);

fn main() {
    let words: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // `-h` or `--help`, wherever it is: the help, and nothing else looked
    // at --- not the file of flags either.
    if words.iter().any(|w| w.to_str().is_some_and(|w| w == "-h" || w == "--help")) {
        help();
    }
    let words: Vec<String> = words
        .into_iter()
        .map(|w| {
            w.into_string().unwrap_or_else(|w| {
                usage(&format!(
                    "{}: not UTF-8, which every flag and every value is",
                    w.to_string_lossy()
                ))
            })
        })
        .collect();
    let typed = Flags::command_line(&words).unwrap_or_else(|e| usage(&e.to_string()));
    let (file, read) = file_of_flags(typed.config());

    // The startup checks, each a refusal to start, all before anything is
    // bound (`DESIGN.md` §6). What was given has the shape of flags; the
    // first is who is running this, before any value is looked at.
    if running_as_root() {
        fail(
            "refusing to run as root: nothing here needs a privilege, and as root a \
             containment bug would reach every file on this host; run it as a user that \
             owns its roots and nothing else (DESIGN.md §6)",
        );
    }
    let Run { mut config, logging, check } =
        Run::new(&typed, &file).unwrap_or_else(|e| refused(&e, read.as_deref()));
    // A band's own host table, where `--hosts-text` names one: its hosts go
    // before the flags' own, and it is read here, with the startup's other
    // checks, so that `--check` covers it and a table that cannot be read
    // stops the daemon rather than quietly leaving HOSTAB half a site.
    let mut table: Option<(usize, PathBuf)> = None;
    if let Some(path) = config.hosts_text.clone() {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| fail(&format!("--hosts-text {}: {e}", path.display())));
        let hosts =
            hosts_text::parse(&text).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
        let before = config.hosts.len();
        config
            .add_hosts(hosts)
            .unwrap_or_else(|e| fail(&format!("--hosts-text {}: {e}", path.display())));
        // What the table gave, which is its hosts but this one's own line,
        // passed over: the count the log says at startup.
        table = Some((config.hosts.len() - before, path));
    }
    // The roots: each canonicalised, a directory, not `/`, writable unless
    // `,ro`, and inside no other; a base directory a mount covers is warned
    // of (`DESIGN.md` §6). `--check` runs these and stops there, having
    // changed nothing.
    let tree = Tree::new(config.roots.clone()).unwrap_or_else(|e| fail(&e));
    for warning in tree.warnings() {
        log::event(format_args!("warning: {warning}"));
    }
    if check {
        exit(0);
    }
    if let Some(path) = &read {
        log::event(format_args!("flags from {}", path.display()));
    }
    if let Some((hosts, path)) = &table {
        let s = if *hosts == 1 { "" } else { "s" };
        log::event(format_args!("{hosts} host{s} from {}", path.display()));
    }
    // Bound before anything in a root is touched: a second daemon given
    // the first's endpoint stops here, and the temporaries of the writes
    // the first is making stay where they are.
    let tree = Arc::new(tree);
    let mut daemon = Daemon::new(&config, tree.clone(), logging)
        .unwrap_or_else(|e| fail(&format!("--listen {}: {e}", config.listen)));
    // A daemon killed mid-write leaves a FILE temporary: each is removed
    // before FILE can make another, the loop not having turned, and
    // nothing else is touched.
    for removed in tree.remove_temporaries() {
        match removed {
            Ok(path) => log::event(format_args!("removed a stale temporary, {}", path.display())),
            Err(why) => log::event(format_args!("a stale temporary is not removed: {why}")),
        }
    }
    log::event(format_args!(
        "ozd {}: {} at {:o}, listening at {}",
        env!("CARGO_PKG_VERSION"),
        config.names[0],
        config.address,
        daemon.at()
    ));
    let start = Instant::now();
    loop {
        daemon.turn(start.elapsed().as_nanos() as u64, WAIT);
    }
}

/// The file of flags this run reads, as flags, and where it is; no flags
/// and nowhere when it reads none.
///
/// The one `-c` names, `named`, which must be there: one that cannot be
/// read is a usage error. Else the one [`RC_NAMED`] names, else [`RC`] in
/// the directory ozd was run from if it is there, else [`RC`] in
/// `$HOME`: **the first of those, not all of them**. One looked for that
/// is not there is none. **One that is there and cannot be read is
/// refused**, exit 1 --- a directory, a file this user may not read, one
/// not in UTF-8 --- rather than taken for none: a daemon run without the
/// flags it was meant to have would look as though it had them. A file's shape is refused as the command line's is, a usage
/// error, with its line.
fn file_of_flags(named: Option<&Path>) -> (Flags, Option<PathBuf>) {
    let (path, named) = match named {
        Some(path) => (path.to_path_buf(), true),
        None => match looked_for() {
            Some(path) => (path, false),
            None => return (Flags::default(), None),
        },
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if named => usage(&format!("--config {}: {e}", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => return (Flags::default(), None),
        Err(e) => fail(&format!("{}: {e}", path.display())),
    };
    let flags = Flags::file(&text).unwrap_or_else(|e| usage(&format!("{}: {e}", path.display())));
    (flags, Some(path))
}

/// Where the file of flags is looked for when `-c` names none: the one
/// [`RC_NAMED`] names, else [`RC`] here if it is there, else [`RC`] in
/// `$HOME`; nowhere with no `$HOME`.
fn looked_for() -> Option<PathBuf> {
    if let Some(named) = std::env::var_os(RC_NAMED) {
        return Some(PathBuf::from(named));
    }
    let here = PathBuf::from(RC);
    if here.exists() {
        return Some(here);
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(RC))
}

/// Whether this process runs as root, by its effective user id, which is
/// what decides what it may touch.
///
/// **The one `unsafe` in the package** (`DESIGN.md` §2): `geteuid`, from the
/// C library std links already. POSIX: it "shall always be successful", and
/// it takes nothing, so there is no memory for it to misuse. Its `uid_t` is
/// 32 bits unsigned --- `unsigned int` on Linux, read in glibc's
/// `bits/typesizes.h`; `__uint32_t` on macOS, by its `sys/_types.h`,
/// **unverified** here.
#[allow(unsafe_code)]
fn running_as_root() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: no argument, no failure, no memory; as above.
    unsafe { geteuid() == 0 }
}

/// `-h` or `--help`: the usage and the help, on stdout; exit code 0.
fn help() -> ! {
    println!("{USAGE}\n\n{HELP}");
    exit(0);
}

/// What was given is not flags, on the command line or in the file of
/// flags: what is wrong, and the usage, on stderr; exit code 2.
fn usage(msg: &str) -> ! {
    eprintln!("ozd: {msg}");
    eprintln!("{USAGE}");
    exit(2);
}

/// A value refused, or a flag required and not given, on stderr, with exit
/// code 1: a value as `<file>: line N: <flag> <value>: <what>` for a line
/// of the file of flags and as `<flag> <value>: <what>` for the command
/// line, and a flag missing with which, where it may be given, and which
/// file this run read, if any.
fn refused(e: &Error, read: Option<&Path>) -> ! {
    match (e.place, read) {
        (Some(Place::Line(_)), Some(path)) => eprintln!("{}: {e}", path.display()),
        (Some(_), _) => eprintln!("{e}"),
        (None, Some(path)) => eprintln!("ozd: {e}, and this run read {}", path.display()),
        (None, None) => eprintln!("ozd: {e}, and this run read none"),
    }
    exit(1);
}

/// A daemon that cannot start, for a reason that is not the shape of what
/// it was given: why, on stderr; exit code 1.
fn fail(msg: &str) -> ! {
    eprintln!("ozd: {msg}");
    exit(1);
}
