// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The site's flags (`DESIGN.md` §8): each flag, each form of `--listen`,
//! and each refusal with where it was given; the file of flags,
//! `.muir-ahrc` --- its comments, a value that is the rest of its line,
//! the command line having the last word, and a file that would name
//! another; which file a run reads, run as the binary; and the two
//! example files that ship (`DESIGN.md` §11, test 2).

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use muir_ah::config::{Config, Error, Flags, Host, Peer, Place, Root, Run};
use support::muir_ah;

/// The least a file of flags can be: an address, a name and a root, on
/// lines 1 to 3. A line added after it is line 4.
const LEAST: &str = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n";

/// [`LEAST`] and then `more`, which must be taken.
fn with(more: &str) -> Config {
    let text = format!("{LEAST}{more}");
    Config::parse(&text).unwrap_or_else(|e| panic!("{text:?} was refused: {e}"))
}

/// Why a file of flags holding `text` is refused: the first thing wrong
/// with it.
fn refusal(text: &str) -> Error {
    match Config::parse(text) {
        Ok(c) => panic!("{text:?} was taken: {c:?}"),
        Err(e) => e,
    }
}

/// `text` refused for a value at `line`, or at no line, and saying
/// `says`: no usage error, but what `main` exits 1 for.
fn refused(text: &str, line: Option<usize>, says: &str) {
    let e = refusal(text);
    assert!(!e.usage, "{text:?}: {e} is a usage error");
    assert_eq!(e.place, line.map(Place::Line), "{text:?}: {e}");
    assert!(e.message.contains(says), "{text:?}: {e:?} does not say {says:?}");
}

/// [`LEAST`] and then `more`, refused for a value at `line` and saying
/// `says`.
fn refused_after(more: &str, line: usize, says: &str) {
    refused(&format!("{LEAST}{more}"), Some(line), says);
}

/// `text` refused for its shape at `line`, and saying `says`: a usage
/// error, which `main` exits 2 for.
fn misused(text: &str, line: usize, says: &str) {
    let e = refusal(text);
    assert!(e.usage, "{text:?}: {e} is no usage error");
    assert_eq!(e.place, Some(Place::Line(line)), "{text:?}: {e}");
    assert!(e.message.contains(says), "{text:?}: {e:?} does not say {says:?}");
}

/// [`LEAST`] and then `more`, refused for its shape at `line` and saying
/// `says`.
fn misused_after(more: &str, line: usize, says: &str) {
    misused(&format!("{LEAST}{more}"), line, says);
}

/// The run that the command line `typed` and a file of flags holding
/// `file` give together, as `main` makes it.
fn run(typed: &[&str], file: &str) -> Result<Run, Error> {
    Run::new(&Flags::command_line(typed)?, &Flags::file(file)?)
}

/// The site `typed` and `file` give together, which must be taken.
fn site(typed: &[&str], file: &str) -> Config {
    run(typed, file).unwrap_or_else(|e| panic!("{typed:?} and {file:?} were refused: {e}")).config
}

/// The command line `typed` refused for its shape, and saying `says`.
fn misused_typed(typed: &[&str], says: &str) {
    let e = match Flags::command_line(typed) {
        Ok(flags) => panic!("{typed:?} was taken: {flags:?}"),
        Err(e) => e,
    };
    assert!(e.usage, "{typed:?}: {e} is no usage error");
    assert_eq!(e.place, Some(Place::CommandLine), "{typed:?}: {e}");
    assert!(e.message.contains(says), "{typed:?}: {e:?} does not say {says:?}");
}

fn at(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

fn root(name: Option<&str>, path: &str, readonly: bool) -> Root {
    Root { name: name.map(str::to_string), path: PathBuf::from(path), readonly }
}

fn host(address: u16, names: &[&str], system: Option<&str>) -> Host {
    Host {
        address,
        names: names.iter().map(|n| n.to_string()).collect(),
        system: system.map(str::to_string),
    }
}

// --- each flag -------------------------------------------------------------

/// **The example of the module documentation is a site** (`src/config.rs`),
/// every flag in it, and reads as it says: a bare `--listen` address takes
/// port 42042, `tree` is a mount and read-only, and a `--peer` without a
/// port is at 42042. The blanks between a flag and its value are one
/// separator, however many.
#[test]
fn the_modules_example_is_read_whole() {
    let text = "\
--address 3060
--name    MIT-OZ,OZ
--listen  192.0.2.10
--root    /srv/lispm
--root    tree=/path/to/muir/vendor/system-100-0/sys,ro
--host    3050,MIT-LISPM-1,LM1,system=LISPM
--host    3051,MIT-LISPM-2,LM2,system=LISPM
--peer    3040@192.0.2.5
";
    let config = Config::parse(text).unwrap();
    assert_eq!(
        config,
        Config {
            address: 0o3060,
            names: vec!["MIT-OZ".to_string(), "OZ".to_string()],
            system: None,
            listen: at("192.0.2.10:42042"),
            roots: vec![
                root(None, "/srv/lispm", false),
                root(Some("tree"), "/path/to/muir/vendor/system-100-0/sys", true),
            ],
            hosts: vec![
                host(0o3050, &["MIT-LISPM-1", "LM1"], Some("LISPM")),
                host(0o3051, &["MIT-LISPM-2", "LM2"], Some("LISPM")),
            ],
            peers: vec![Peer { address: 0o3040, endpoint: at("192.0.2.5:42042") }],
        }
    );
}

/// **A comment is a line beginning with `#`**, blanks before it or not,
/// and a line of nothing, or of blanks, is no line at all. A `\r\n` ending
/// is a line ending, and a tab parts a flag from its value as a space
/// does.
#[test]
fn comments_and_blank_lines_are_nothing() {
    let text = "# the site\n\n   \n\t--address\t3060\r\n   # two names\n\
                --name MIT-OZ,OZ   \n# --root /elsewhere\n--root /srv/lispm\n";
    assert_eq!(Config::parse(text).unwrap(), Config::parse(LEAST).unwrap());
}

/// **A value is the rest of its line**, blanks and `#` and all, less the
/// blanks at either end, as in muir's `.muirrc` (muir's `muirrc`,
/// `src/main.rs`): a `#` after a flag is no comment, and a path in a file
/// may hold a blank, as one on the command line may in a shell's quotes.
#[test]
fn a_value_is_the_rest_of_its_line() {
    let text = "--address 3060\n--name OZ\n--root   /srv/lisp machines # the base  \n";
    assert_eq!(
        Config::parse(text).unwrap().roots,
        [root(None, "/srv/lisp machines # the base", false)]
    );
    let config = site(&["--root", "/srv/lisp machines"], "--address 3060\n--name OZ\n");
    assert_eq!(config.roots, [root(None, "/srv/lisp machines", false)]);
}

/// **An address is octal or `subnet:host`**, in every flag that takes
/// one: `6:60` is 3060.
#[test]
fn an_address_is_octal_or_subnet_host() {
    let config = Config::parse(
        "--address 6:60\n--name OZ\n--root /srv/lispm\n--host 6:50,LM1\n--peer 6:40@192.0.2.5\n",
    )
    .unwrap();
    assert_eq!(config.address, 0o3060);
    assert_eq!(config.hosts[0].address, 0o3050);
    assert_eq!(config.peers[0].address, 0o3040);
}

/// **The names keep their order**: the first is the official one.
#[test]
fn the_official_name_is_first() {
    let config = Config::parse("--address 3060\n--name MIT-OZ,OZ,ZO\n--root /srv/lispm\n").unwrap();
    assert_eq!(config.names, ["MIT-OZ", "OZ", "ZO"]);
}

/// **`--name` gives this host's system type**, `system=` as `--host` gives
/// one: a part of the value wherever it is, and the other parts are the
/// names, in their order. Without it, none.
#[test]
fn the_name_gives_this_hosts_system_type() {
    for name in ["MIT-OZ,OZ,system=UNIX", "system=UNIX,MIT-OZ,OZ", "MIT-OZ,system=UNIX,OZ"] {
        let config =
            Config::parse(&format!("--address 3060\n--name {name}\n--root /srv/lispm\n")).unwrap();
        assert_eq!(config.names, ["MIT-OZ", "OZ"], "{name}");
        assert_eq!(config.system.as_deref(), Some("UNIX"), "{name}");
    }
    assert_eq!(Config::parse(LEAST).unwrap().system, None);
}

/// **Without `--listen`, the loopback at 42042**: a fresh install answers
/// its own host and nothing else (`DESIGN.md` §5).
#[test]
fn without_listen_it_is_the_loopback_at_42042() {
    assert_eq!(Config::parse(LEAST).unwrap().listen, at("127.0.0.1:42042"));
}

/// **`--listen` takes muir's `--chaos-udp` forms**: a port on the
/// loopback, an address at 42042, or both; `0.0.0.0` and `::` every
/// interface; IPv6 bare, or in brackets before a port. Port 0 is one the
/// system picks. It always takes one of them: the default is no
/// `--listen` at all.
#[test]
fn listen_takes_the_forms_of_chaos_udp() {
    for (value, want) in [
        ("42043", "127.0.0.1:42043"),
        ("0", "127.0.0.1:0"),
        ("127.0.0.1", "127.0.0.1:42042"),
        ("192.0.2.10", "192.0.2.10:42042"),
        ("192.0.2.10:42043", "192.0.2.10:42043"),
        ("0.0.0.0", "0.0.0.0:42042"),
        ("0.0.0.0:42043", "0.0.0.0:42043"),
        ("::", "[::]:42042"),
        ("::1", "[::1]:42042"),
        ("[::1]:42043", "[::1]:42043"),
        ("[::]:42043", "[::]:42043"),
    ] {
        assert_eq!(with(&format!("--listen {value}\n")).listen, at(want), "{value}");
    }
}

/// **A base root and mounts**, each read-only with `,ro` or not, in the
/// order given. A value beginning with `/` is the base; any other is a
/// mount, its name before `=` and its path after.
#[test]
fn a_root_is_a_base_or_a_mount() {
    let config = Config::parse(
        "--address 3060\n--name OZ\n--root /srv/lispm,ro\n\
         --root tree=/path/to/muir/vendor/system-100-0/sys,ro\n--root doc=/srv/doc\n",
    )
    .unwrap();
    assert_eq!(
        config.roots,
        [
            root(None, "/srv/lispm", true),
            root(Some("tree"), "/path/to/muir/vendor/system-100-0/sys", true),
            root(Some("doc"), "/srv/doc", false),
        ]
    );
}

/// **Mounts without a base are a site**: `/` then names only the mounts
/// (`DESIGN.md` §6).
#[test]
fn mounts_without_a_base_are_a_site() {
    let config = Config::parse("--address 3060\n--name OZ\n--root sys=/srv/sys,ro\n").unwrap();
    assert_eq!(config.roots, [root(Some("sys"), "/srv/sys", true)]);
}

/// **A path may hold `=`**: only a value that does not begin with `/` is
/// parted at its first `=`, so a base's path is whole, and so is a
/// mount's after its name.
#[test]
fn a_path_may_hold_an_equals_sign() {
    let config =
        Config::parse("--address 3060\n--name OZ\n--root /srv/a=b\n--root tree=/srv/c=d,ro\n")
            .unwrap();
    assert_eq!(config.roots, [root(None, "/srv/a=b", false), root(Some("tree"), "/srv/c=d", true)]);
}

/// **`--host` is an address, its names, and a system type if it has one**,
/// in the order given.
#[test]
fn a_host_is_an_address_and_names() {
    let config = with("--host 3050,MIT-LISPM-1,CADR-1,LM1,system=LISPM\n--host 3040,CBRIDGE\n");
    assert_eq!(
        config.hosts,
        [
            host(0o3050, &["MIT-LISPM-1", "CADR-1", "LM1"], Some("LISPM")),
            host(0o3040, &["CBRIDGE"], None),
        ]
    );
}

/// **`--peer` fixes an endpoint**, in muir's `--chaos-udp-peer` form: an
/// IP literal after the `@`, at 42042 unless a port is given, IPv6 as
/// `--listen` takes it.
#[test]
fn a_peer_is_an_address_and_an_endpoint() {
    let config = with(
        "--peer 3040@192.0.2.5\n--peer 3041@192.0.2.5:42043\n--peer 3042@::1\n\
         --peer 3043@[::1]:42044\n--peer 3044@127.0.0.1:42045\n",
    );
    let want = [
        (0o3040, "192.0.2.5:42042"),
        (0o3041, "192.0.2.5:42043"),
        (0o3042, "[::1]:42042"),
        (0o3043, "[::1]:42044"),
        (0o3044, "127.0.0.1:42045"),
    ]
    .map(|(address, endpoint)| Peer { address, endpoint: at(endpoint) });
    assert_eq!(config.peers, want);
}

/// **A `--host` and a `--peer` at one address are one host, named and
/// placed**: the host table and the endpoints are separate (`DESIGN.md`
/// §8), and neither answers the other's question.
#[test]
fn a_host_and_a_peer_at_one_address_are_one_host() {
    let config = with("--host 3040,CBRIDGE\n--peer 3040@192.0.2.5\n");
    assert_eq!(config.hosts[0].address, 0o3040);
    assert_eq!(config.peers[0].address, 0o3040);
}

// --- each refusal, with where it was given ---------------------------------

/// **A word that is not a flag is a usage error, with its line**, and so is
/// a flag in capitals and `--flag=value`: a line is a flag, a blank, and
/// its value. A word that would be a flag with `--` before it says which.
#[test]
fn a_word_that_is_not_a_flag_is_refused() {
    misused_after("--frob 1\n", 4, "--frob: not a flag");
    misused_after("--Listen 42043\n", 4, "not a flag");
    misused_after("--listen=42043\n", 4, "not a flag");
    misused_after("--readonly\n", 4, "not a flag");
    misused("address 3060\n--name OZ\n--root /srv/lispm\n", 1, "not a flag; --address is");
}

/// **A flag that wants a value and has none is a usage error**, with its
/// line and what the flag wants; one that takes none and is given one is
/// too.
#[test]
fn a_flag_without_its_value_is_refused() {
    for flag in ["--address", "--name", "--listen", "--root", "--host", "--peer"] {
        misused_after(&format!("{flag}\n"), 4, &format!("{flag} wants <"));
        misused_after(&format!("{flag}   \n"), 4, &format!("{flag} wants <"));
    }
    misused_after("--trace yes\n", 4, "--trace takes no value");
}

/// **`--address`, `--name` and a `--root` are required**: with none, the
/// refusal has no line to name, and says which is missing and where it may
/// be given.
#[test]
fn what_is_required_is_there() {
    refused("", None, "no --address");
    refused("# nothing\n\n", None, "no --address");
    refused("--name OZ\n--root /srv/lispm\n", None, "no --address");
    refused("--address 3060\n--root /srv/lispm\n", None, "no --name");
    refused("--address 3060\n--name OZ\n", None, "no --root");
    for text in ["", "--address 3060\n", "--address 3060\n--name OZ\n"] {
        refused(text, None, "on the command line or in a file of flags");
    }
}

/// **`--address`, `--name` and `--listen` are given once each**, and the
/// second names the first.
#[test]
fn what_is_once_is_once() {
    refused_after("--address 3061\n", 4, "once only");
    refused_after("--address 3061\n", 4, "line 1");
    refused_after("--name ZO\n", 4, "line 2");
    refused_after("--listen 42043\n--listen 42044\n", 5, "line 4");
}

/// **`--address` is one address, neither half zero** (`parse_address`): a
/// zero host is every host of the subnet, and a zero subnet no subnet.
#[test]
fn an_address_is_refused_by_parse_address() {
    for bad in ["6:0", "0:60", "377", "3068", "400:1", "OZ", "-3060", "3060 3061", "3060,3061"] {
        let text = format!("--address {bad}\n--name OZ\n--root /srv/lispm\n");
        refused(&text, Some(1), "not an address");
    }
}

/// **`--name` gives at least one name**, and a part with `=` in it is an
/// attribute, as in `--host`: `system=`, given a type, once, is the one
/// there is. Parts are parted by commas: an empty one is refused, and so
/// is a name with a blank in it, which says how names are parted.
#[test]
fn a_name_is_names() {
    let name = |value: &str| format!("--address 3060\n--name {value}\n--root /srv/lispm\n");
    refused(&name("system=UNIX"), Some(2), "at least one name");
    refused(&name("OZ,system="), Some(2), "system= wants a type");
    refused(&name("OZ,system=UNIX,system=ITS"), Some(2), "system= twice");
    refused(&name("OZ,machine=VAX"), Some(2), "the one attribute");
    refused(&name("MIT-OZ,,OZ"), Some(2), "empty");
    refused(&name("MIT-OZ,"), Some(2), "empty");
    refused(&name("MIT-OZ OZ"), Some(2), "separated by commas");
}

/// **`--listen` takes IP literals and never a name**, one endpoint, and a
/// port that is a port.
#[test]
fn listen_takes_no_names_and_no_nonsense() {
    for bad in [
        "localhost",
        "localhost:42042",
        "70000",
        "192.0.2.10:",
        "192.0.2.10:x",
        "192.0.2",
        "192.0.2.10 42043",
        "192.0.2.10,42043",
    ] {
        refused_after(&format!("--listen {bad}\n"), 4, "not a port");
    }
}

/// **A root's path is absolute**, the base's and a mount's; that it exists
/// is the startup's to check (`DESIGN.md` §6), not the flag's. A value that
/// neither begins with `/` nor has a name and `=` is a path that is not
/// absolute.
#[test]
fn a_root_path_is_absolute() {
    refused("--address 3060\n--name OZ\n--root srv/lispm\n", Some(3), "not absolute");
    refused("--address 3060\n--name OZ\n--root ro\n", Some(3), "not absolute");
    refused_after("--root tree=path/to/sys,ro\n", 4, "not absolute");
    refused_after("--root tree=./sys\n", 4, "not absolute");
    refused_after("--root tree=\n", 4, "not absolute");
    refused_after("--root tree=,ro\n", 4, "not absolute");
}

/// **One base, and each mount's name once**; the second names the first.
/// A value beginning with `/` is a base whatever follows, so
/// `/tree=/srv/tree` is a second base, and the refusal says how a mount is
/// written.
#[test]
fn one_base_and_each_mount_once() {
    refused_after("--root /srv/other\n", 4, "line 3");
    refused_after("--root /tree=/srv/tree\n", 4, "a mount is <name>=<path>");
    refused_after("--root tree=/srv/a\n--root tree=/srv/b,ro\n", 5, "line 4");
}

/// **A mount's name is one directory name at the top of the tree, in
/// lower case**: a band sends its pathnames in lower case and names match
/// exactly (`DESIGN.md` §6), so a `TREE` would never be reached.
#[test]
fn a_mount_name_is_one_lower_case_directory_name() {
    for bad in ["a/b", ".", "..", ""] {
        refused_after(&format!("--root {bad}=/srv/tree\n"), 4, "not a mount's name");
    }
    refused_after("--root TREE=/srv/tree\n", 4, "lower case");
    refused_after("--root Tree=/srv/tree\n", 4, "lower case");
}

/// **`--root` is a path, a name and `=` before it or not, and `,ro` after
/// it or not**, and nothing else. **A path cannot hold a comma**: the first
/// comma ends the path, and what follows it is `ro` or refused, saying so.
#[test]
fn a_root_is_no_more_than_that() {
    refused_after("--root tree=/srv/tree,ro,ro\n", 4, "ro twice");
    for bad in ["tree=/srv/tree,readonly", "tree=/srv/tree,rw", "tree=/srv/a,b", "tree=/srv/tree,"]
    {
        refused_after(&format!("--root {bad}\n"), 4, "a path cannot hold a comma");
    }
    refused("--address 3060\n--name OZ\n--root /srv/a,b\n", Some(3), "a path cannot hold a comma");
}

/// **`--host` is a good address and at least one name**, its parts parted
/// by commas; its one attribute is `system=`, given a type, once.
#[test]
fn a_host_is_refused_for_what_is_wrong_with_it() {
    refused_after("--host 3050\n", 4, "at least one name");
    refused_after("--host 3050,system=LISPM\n", 4, "at least one name");
    refused_after("--host 6:0,LM1\n", 4, "not an address");
    refused_after("--host LM1,3050\n", 4, "not an address");
    refused_after("--host 3050 LM1\n", 4, "not an address");
    refused_after("--host 3050,LM1,system=\n", 4, "system= wants a type");
    refused_after("--host 3050,LM1,system=LISPM,system=ITS\n", 4, "system= twice");
    refused_after("--host 3050,LM1,machine=LISPM\n", 4, "the one attribute");
    refused_after("--host 3050,LM1,,LM2\n", 4, "empty");
}

/// **A system type is upper case**, in `--name` and `--host` alike, and
/// refused with its line otherwise: HOSTAB's user end interns it as sent
/// (`sys/network/chaos/chuse.lisp:983`), and a flavor is filed under
/// `:LISPM`, never `:lispm` (`sys/network/host.lisp:279`). Upper case is
/// all that is asked: a hyphen or a digit is no lower-case letter, and a
/// type the band files no flavor under, as `WAITS`, is taken --- the band
/// gives it the default flavor, as it gives a host with no type.
#[test]
fn a_system_type_is_upper_case() {
    refused("--address 3060\n--root /srv/lispm\n--name OZ,system=unix\n", Some(3), "upper case");
    refused("--address 3060\n--root /srv/lispm\n--name system=Unix,OZ\n", Some(3), "upper case");
    refused_after("--host 3050,LM1,system=lispm\n", 4, "upper case");
    refused_after("--root tree=/srv/tree\n--host 3050,LM1,system=LISPm\n", 5, "upper case");
    let config = with("--host 3050,LM1,system=TOPS-20\n--host 3051,LM2,system=WAITS\n");
    assert_eq!(config.hosts[0].system.as_deref(), Some("TOPS-20"));
    assert_eq!(config.hosts[1].system.as_deref(), Some("WAITS"));
}

/// **`--peer` is a good address, `@`, and an IP literal to send to**: never
/// a name, never a bare port, never every interface or port 0.
#[test]
fn a_peer_is_refused_for_what_is_wrong_with_it() {
    for bad in ["3040", "3040 192.0.2.5", "192.0.2.5"] {
        refused_after(&format!("--peer {bad}\n"), 4, "<addr>@<ip>");
    }
    refused_after("--peer 6:0@192.0.2.5\n", 4, "not an address");
    for bad in [
        "localhost",
        "cbridge.example.org:42042",
        "42043",
        "192.0.2.5:x",
        "192.0.2.5:",
        "192.0.2.5 192.0.2.6",
        "192.0.2.5@192.0.2.6",
        "",
    ] {
        refused_after(&format!("--peer 3040@{bad}\n"), 4, "not an IP address");
    }
    for bad in ["0.0.0.0", "0.0.0.0:42042", "::", "[::]:42042", "192.0.2.5:0"] {
        refused_after(&format!("--peer 3040@{bad}\n"), 4, "to send to");
    }
}

/// **An address is one host in the host table, and one endpoint**: this
/// host's own address has no `--host` --- its names are `--name`'s --- and
/// no `--peer`, since it is not its own peer; two `--host`s at one address
/// are two answers to whose it is, and two `--peer`s two answers to where
/// it is. The later is refused, whichever order they come in, naming the
/// earlier.
#[test]
fn an_address_is_not_two_answers() {
    refused_after("--host 3060,ZO\n", 4, "this host's own address, --address on line 1");
    refused_after("--peer 3060@192.0.2.5\n", 4, "this host's own address, --address on line 1");
    refused_after("--host 3050,LM1\n--host 3050,LM2\n", 5, "line 4");
    refused_after("--peer 3040@192.0.2.5\n--peer 3040@192.0.2.6\n", 5, "line 4");
    refused("--host 3060,ZO\n--address 3060\n--name OZ\n--root /srv/lispm\n", Some(2), "line 1");
    refused(
        "--peer 3060@192.0.2.5\n--address 3060\n--name OZ\n--root /srv/lispm\n",
        Some(2),
        "line 1",
    );
}

/// **A host name is one host**, this host's and every `--host`'s together,
/// and ignoring case, since HOSTAB looks a name up ignoring case
/// (`DESIGN.md` §7). The later is refused, naming the earlier.
#[test]
fn a_host_name_is_one_host() {
    refused_after("--host 3050,OZ\n", 4, "by --name on line 2");
    refused_after("--host 3050,oz\n", 4, "line 2");
    refused_after("--host 3050,LM1\n--host 3051,lm1\n", 5, "by --host on line 4");
    refused_after("--host 3050,LM1,LM1\n", 4, "twice");
    refused("--address 3060\n--name OZ,oz\n--root /srv/lispm\n", Some(2), "twice");
    refused(
        "--host 3050,OZ\n--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n",
        Some(3),
        "line 1",
    );
}

/// **A name, a system type and a mount's name are printable ASCII**, and
/// refused with their line otherwise. HOSTAB sends a name a byte a
/// character, and FILE a mount's name in a listing of `/`, through
/// `lispm::lispm_text`, where a character below 256 is that byte: U+008D
/// would go out as 215 octal, the band's newline, in the middle of an
/// answer. The band's own tables hold nothing else.
#[test]
fn names_system_types_and_mount_names_are_printable_ascii() {
    refused("--address 3060\n--root /srv/lispm\n--name MIT-OZ\u{8d}\n", Some(3), "printable ASCII");
    refused_after("--host 3050,LM\u{e9}1\n", 4, "printable ASCII");
    refused_after("--host 3050,LM1,system=UNI\u{8d}X\n", 4, "printable ASCII");
    refused_after("--root tr\u{e9}e=/srv/tree\n", 4, "printable ASCII");
}

/// **A refusal says where it was given first**: a line of the file as
/// `line N: `, then `<flag> <value>: ` and what is wrong; the command
/// line's as `<flag> <value>: ` alone; and one with no place --- a flag
/// missing --- neither.
#[test]
fn a_refusal_says_where_it_was_given() {
    let e = refusal(&format!("{LEAST}--host 6:0,LM1\n"));
    assert!(e.to_string().starts_with("line 4: --host 6:0,LM1: 6:0: not an address"), "{e}");
    let e = run(&["--address", "6:0"], "--name OZ\n--root /srv/lispm\n").unwrap_err();
    assert_eq!((e.place, e.usage), (Some(Place::CommandLine), false), "{e}");
    assert!(e.to_string().starts_with("--address 6:0: not an address"), "{e}");
    let e = refusal("");
    assert!(e.to_string().starts_with("no --address: "), "{e}");
}

// --- the command line, and the file with it ---------------------------------

/// **Every flag can be given on the command line**, a value the word after
/// its flag, and the site is the one a file with the same flags gives.
/// `--trace` and `--check` are the run's, not the site's, and neither is
/// set unless given.
#[test]
fn every_flag_can_be_given_on_the_command_line() {
    let typed = [
        "--address",
        "3060",
        "--name",
        "MIT-OZ,OZ,system=UNIX",
        "--listen",
        "192.0.2.10",
        "--root",
        "/srv/lispm",
        "--root",
        "tree=/path/to/muir/vendor/system-100-0/sys,ro",
        "--host",
        "3050,MIT-LISPM-1,LM1,system=LISPM",
        "--peer",
        "3040@192.0.2.5",
        "--trace",
        "--check",
    ];
    let file = "--address 3060\n--name MIT-OZ,OZ,system=UNIX\n--listen 192.0.2.10\n\
                --root /srv/lispm\n--root tree=/path/to/muir/vendor/system-100-0/sys,ro\n\
                --host 3050,MIT-LISPM-1,LM1,system=LISPM\n--peer 3040@192.0.2.5\n";
    let r = run(&typed, "").unwrap();
    assert_eq!(r.config, Config::parse(file).unwrap());
    assert!(r.trace && r.check);
    let r = run(&[], LEAST).unwrap();
    assert!(!r.trace && !r.check, "neither unless given");
}

/// **The command line and the file of flags together are the site**: what
/// one does not give, the other may.
#[test]
fn the_command_line_and_the_file_together_are_the_site() {
    let config = site(&["--root", "/srv/lispm"], "--address 3060\n--name MIT-OZ,OZ\n");
    assert_eq!(config, Config::parse(LEAST).unwrap());
}

/// **A flag the command line gives leaves that flag's lines out of the
/// file**, every one of them: the command line has the last word, as with
/// muir's `.muirrc` (muir's `muirrc`, `src/main.rs`). So a file whose
/// `--address` lines would be refused is not refused for them, the file's
/// mounts go with its base when the command line gives a root, and the
/// file's other flags stay.
#[test]
fn a_flag_the_command_line_gives_leaves_the_files_lines_of_it_out() {
    let file = format!(
        "{LEAST}--address 3061\n--root tree=/srv/tree,ro\n--host 3050,LM1\n--host 3051,LM2\n"
    );
    assert!(Config::parse(&file).is_err(), "alone, the file is refused for its second --address");
    let config = site(&["--address", "3062", "--root", "/srv/other", "--host", "3052,LM3"], &file);
    assert_eq!(config.address, 0o3062);
    assert_eq!(config.roots, [root(None, "/srv/other", false)]);
    assert_eq!(config.hosts, [host(0o3052, &["LM3"], None)]);
    assert_eq!(config.names, ["MIT-OZ", "OZ"], "the file's --name stays");
}

/// **The file is read first and the command line after**, so where one
/// flag refuses another's value across the two, the command line's is the
/// one refused, naming the file's line --- a refusal of the command line's,
/// `<flag> <value>: ` with no line before it. Two given on the command
/// line name the command line.
#[test]
fn the_file_is_read_before_the_command_line() {
    let file = "--name MIT-OZ,OZ\n--root /srv/lispm\n--host 3050,LM1\n";
    let e = run(&["--address", "3050"], file).unwrap_err();
    assert_eq!(e.place, Some(Place::CommandLine), "{e}");
    assert!(e.message.starts_with("--address 3050: the --host on line 3 of the file"), "{e}");
    let e = run(&["--address", "3060", "--host", "3051,OZ"], file).unwrap_err();
    assert!(
        e.message
            .starts_with("--host 3051,OZ: OZ is named already, by --name on line 1 of the file"),
        "{e}"
    );
    let e = run(&["--address", "3060", "--address", "3061"], file).unwrap_err();
    assert!(e.message.contains("given on the command line already"), "{e}");
}

/// **A command line of anything but flags is a usage error**: a word that
/// is not a flag; an argument that is no flag's value, whose refusal says
/// that the config is flags, on the command line or in `.muir-ahrc`, or in
/// the file `-c` names; a flag with no value after it, or with a flag
/// where its value would be; and `-c` twice.
#[test]
fn a_command_line_of_anything_but_flags_is_refused() {
    misused_typed(&["--frobnicate"], "--frobnicate: not a flag");
    misused_typed(&["-x"], "-x: not a flag");
    let positional: [&[&str]; 3] =
        [&["site.conf"], &["--trace", "site.conf"], &["--address", "3060", "3061"]];
    for typed in positional {
        for says in ["the config is flags", ".muir-ahrc", "-c <file>"] {
            misused_typed(typed, says);
        }
    }
    misused_typed(&["--address"], "--address wants <addr>");
    misused_typed(&["--trace", "--root"], "--root wants <");
    misused_typed(&["--address", "--name", "OZ"], "--address wants <addr>");
    misused_typed(&["-c"], "-c wants <file>");
    misused_typed(&["-c", "a", "--config", "b"], "not two");
}

/// **`-c` and `--config` name the file of flags**; a command line with
/// neither names none, and the file is looked for
/// (`the_file_read_is_the_first_of_those_there`).
#[test]
fn config_names_the_file_of_flags() {
    for flag in ["-c", "--config"] {
        let typed = Flags::command_line(&[flag, "/path/to/site.muir-ahrc", "--trace"]).unwrap();
        assert_eq!(typed.config(), Some(Path::new("/path/to/site.muir-ahrc")), "{flag}");
    }
    assert_eq!(Flags::command_line(&["--trace"]).unwrap().config(), None);
}

/// **`--trace` may be in a file**: how much the daemon prints is a
/// standing choice, as muir's `--chaos-trace` in a `.muirrc` is, and
/// changes nothing it does.
#[test]
fn trace_may_be_in_a_file() {
    let r = run(&[], &format!("{LEAST}--trace\n")).unwrap();
    assert!(r.trace && !r.check);
}

/// **`--check` and `-h`/`--help` are the command line's**, and in a file a
/// usage error with its line: each is what one run is asked to do, and in
/// a file every run would do it and exit 0 --- the daemon the file is for
/// never serving, and a service manager seeing a clean exit.
#[test]
fn check_and_help_are_the_command_lines() {
    for flag in ["--check", "--help", "-h"] {
        misused_after(&format!("{flag}\n"), 4, "the command line's");
    }
}

/// **A file of flags cannot name another**, with `-c` or `--config`: a
/// usage error with its line. Which file to read is the command line's to
/// say, and a file that could name another could name itself (muir's
/// `muirrc`).
#[test]
fn a_file_cannot_name_another() {
    misused_after("--config /somewhere/else\n", 4, "cannot name another");
    misused_after("-c /somewhere/else\n", 4, "cannot name another");
}

// --- which file is read, run as the binary ---------------------------------

/// A directory of its own for one test, in the scratch directory, empty.
fn dir(name: &str) -> PathBuf {
    let dir = support::scratch().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A `.muir-ahrc` in `dir` whose one flag is none, `--from-<from>`: a
/// usage error that names it, so that a run's refusal says which file it
/// read.
fn rc(dir: &Path, from: &str) -> PathBuf {
    let path = dir.join(".muir-ahrc");
    let text = format!("# which file this is\n--from-{from}\n");
    std::fs::write(&path, text).expect("a file of flags");
    path
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Which of [`rc`]'s files a run read, by the flag its refusal names.
fn read_from(out: &Output) -> Option<&'static str> {
    let said = said(out);
    ["named", "env", "here", "home"]
        .into_iter()
        .find(|from| said.contains(&format!("--from-{from}: not a flag")))
}

/// The binary, run from `cwd` with `HOME` at `home`, and no `MUIR_AH_RC`
/// unless the test gives it one.
fn from(cwd: &Path, home: &Path) -> Command {
    let mut c = muir_ah();
    c.current_dir(cwd).env("HOME", home);
    c
}

/// Which of [`rc`]'s files the run `c` reads, which it refuses as a usage
/// error.
fn reads(c: &mut Command) -> Option<&'static str> {
    let out = c.output().expect("it runs");
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    read_from(&out)
}

/// **The file read is the first of those there, not all of them**, as with
/// muir's `.muirrc` (muir's `config_path`, `src/main.rs`): the one `-c`
/// names; else the one `MUIR_AH_RC` names; else `.muir-ahrc` in the
/// directory muir-ah is run from; else `.muir-ahrc` in `$HOME`. `HOME` is
/// a directory of the test's own, so the user's own file is never read.
/// Each file here is a usage error, found before who is running it is, so
/// this holds as root too.
#[test]
fn the_file_read_is_the_first_of_those_there() {
    let (here, home, empty) = (dir("rc-here"), dir("rc-home"), dir("rc-empty"));
    let named = rc(&dir("rc-named"), "named");
    let env = rc(&dir("rc-env"), "env");
    rc(&here, "here");
    rc(&home, "home");
    let mut c = from(&here, &home);
    assert_eq!(reads(c.env("MUIR_AH_RC", &env).arg("-c").arg(&named)), Some("named"), "-c first");
    assert_eq!(reads(from(&here, &home).env("MUIR_AH_RC", &env)), Some("env"), "then MUIR_AH_RC");
    assert_eq!(reads(&mut from(&here, &home)), Some("here"), "then the directory run from");
    assert_eq!(reads(&mut from(&empty, &home)), Some("home"), "then the home directory");
}

/// **One looked for need not be there**, and `MUIR_AH_RC` stands in place
/// of the two that are looked for: naming a file that is not there, the
/// run reads none, a `.muir-ahrc` in the directory it was run from and in
/// `$HOME` notwithstanding. A run that reads none has its command line's
/// flags alone, and checks with them.
#[test]
fn one_looked_for_need_not_be_there() {
    let (here, home, empty) = (dir("rc-there-here"), dir("rc-there-home"), dir("rc-there-empty"));
    rc(&here, "here");
    rc(&home, "home");
    let root = dir("rc-there-root");
    let typed =
        ["--check", "--address", "3060", "--name", "OZ", "--listen", "127.0.0.1:0", "--root"];
    let out = from(&here, &home)
        .env("MUIR_AH_RC", here.join("no-such"))
        .args(typed)
        .arg(&root)
        .output()
        .expect("it runs");
    assert_eq!(read_from(&out), None, "neither is read: {}", said(&out));
    let alone = from(&empty, &empty).args(typed).arg(&root).output().expect("it runs");
    if support::running_as_root() {
        return;
    }
    assert!(out.status.success(), "{}", said(&out));
    assert!(alone.status.success(), "{}", said(&alone));
}

/// **A file `-c` names must be there**: one that is not is a usage error,
/// exit 2 with the usage, naming it --- `-c` and `--config` alike, and as
/// root too, since it is found before who is running it is.
#[test]
fn a_file_config_names_must_be_there() {
    let missing = support::scratch().join("no-such.muir-ahrc");
    for flag in ["-c", "--config"] {
        let out = muir_ah().arg(flag).arg(&missing).output().expect("it runs");
        assert_eq!(out.status.code(), Some(2), "{flag}: {}", said(&out));
        assert!(said(&out).contains(&missing.display().to_string()), "{flag}: {}", said(&out));
        assert!(said(&out).contains("usage: muir-ah"), "{flag}: {}", said(&out));
    }
}

/// **One looked for that is there and cannot be read is refused**, exit
/// 1, naming it --- here a directory called `.muir-ahrc` --- and not taken
/// for none: a run without the flags it was meant to have would look as
/// though it had them. It is found before who is running it is, so this
/// holds as root too.
#[test]
fn one_looked_for_that_cannot_be_read_is_refused() {
    let here = dir("rc-unreadable");
    std::fs::create_dir(here.join(".muir-ahrc")).expect("a directory");
    let out = from(&here, &here).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains(".muir-ahrc"), "{}", said(&out));
}

/// **A file of flags cannot name another**, run as the binary: a usage
/// error, exit 2, naming the file and its line.
#[test]
fn a_file_that_names_another_is_a_usage_error() {
    let file = dir("rc-names-another").join(".muir-ahrc");
    std::fs::write(&file, "--trace\n--config /somewhere/else\n").expect("a file of flags");
    let out = muir_ah().env("MUIR_AH_RC", &file).output().expect("it runs");
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    let want = format!("{}: line 2: --config: a file of flags cannot name another", file.display());
    assert!(said(&out).contains(&want), "{}", said(&out));
}

/// **The command line has the last word**, run as the binary: a file whose
/// base is not there does not check, and the same file with a `--root` on
/// the command line that is there does.
#[test]
fn the_command_line_has_the_last_word() {
    if support::running_as_root() {
        return;
    }
    let d = dir("rc-last-word");
    let root = dir("rc-last-word-root");
    let file = d.join("site.muir-ahrc");
    let text = format!(
        "--address 3060\n--name OZ\n--listen 127.0.0.1:0\n--root {}\n",
        d.join("no-such").display()
    );
    std::fs::write(&file, text).expect("a file of flags");
    let out = muir_ah().arg("--check").arg("-c").arg(&file).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1), "its own base is not there: {}", said(&out));
    let out = muir_ah().arg("--check").arg("-c").arg(&file).arg("--root").arg(&root).output();
    let out = out.expect("it runs");
    assert!(out.status.success(), "{}", said(&out));
}

// --- the examples ------------------------------------------------------------

/// An example file of flags from the crate's `examples/`, read as a run
/// with nothing on its command line reads it.
fn example(name: &str) -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Config::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// **System 100's example is a site**: this host `MIT-OZ` at 3060, of
/// system type `UNIX` as the band's own table has it
/// (`sys/site/hosts.text`), the band's `MIT-LISPM-1` at 3050 in the host
/// table, the homes writable and the release's sources mounted read-only
/// at `/tree`, where the band asks for them. On the loopback, at 42042.
#[test]
fn the_system_100_example_is_a_site() {
    let config = example("system-100.muir-ahrc");
    assert_eq!(config.address, 0o3060);
    assert_eq!(config.names, ["MIT-OZ", "OZ"]);
    assert_eq!(config.system.as_deref(), Some("UNIX"));
    assert_eq!(config.listen, at("127.0.0.1:42042"));
    assert_eq!(
        config.roots,
        [
            root(None, "/srv/lispm", false),
            root(Some("tree"), "/path/to/muir/vendor/system-100-0/sys", true),
        ]
    );
    let lm1 = host(0o3050, &["MIT-LISPM-1", "CADR-1", "CADR1", "LM1"], Some("LISPM"));
    assert_eq!(config.hosts, [lm1]);
    assert!(config.peers.is_empty(), "every endpoint is learned");
}

/// **System 304's example is a site**: this host `OZ` at 4403, also
/// `AMS-BRIDGE-1`, of no system type, since the host table in the sources
/// is MIT's and not this band's; the band's `AMS-LISPM-1` at 4401, and the
/// sources mounted read-only at `/sys`.
#[test]
fn the_system_304_example_is_a_site() {
    let config = example("system-304.muir-ahrc");
    assert_eq!(config.address, 0o4403);
    assert_eq!(config.names, ["OZ", "AMS-BRIDGE-1"]);
    assert_eq!(config.system, None);
    assert_eq!(config.listen, at("127.0.0.1:42042"));
    assert_eq!(
        config.roots,
        [
            root(None, "/srv/lispm", false),
            root(Some("sys"), "/path/to/muir/vendor/system-304-0/sys-304-0", true),
        ]
    );
    assert_eq!(config.hosts, [host(0o4401, &["AMS-LISPM-1"], Some("LISPM"))]);
    assert!(config.peers.is_empty(), "every endpoint is learned");
}
