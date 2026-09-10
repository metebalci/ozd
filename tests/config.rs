// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The site file (`DESIGN.md` §8): each directive, each form of `listen`,
//! each refusal with its line, and the two example files that ship
//! (`DESIGN.md` §11, test 2).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use muir_ah::config::{Config, Host, Peer, Root};

/// The least a site file can be: an address, a name and a root, on lines
/// 1 to 3. A line added after it is line 4.
const LEAST: &str = "address 3060\nname MIT-OZ OZ\nroot /srv/lispm\n";

/// [`LEAST`] and then `more`, which must be taken.
fn with(more: &str) -> Config {
    let text = format!("{LEAST}{more}");
    Config::parse(&text).unwrap_or_else(|e| panic!("{text:?} was refused: {e}"))
}

/// `text` refused at `line`, and saying `says`.
fn refused(text: &str, line: Option<usize>, says: &str) {
    let e = match Config::parse(text) {
        Ok(c) => panic!("{text:?} was taken: {c:?}"),
        Err(e) => e,
    };
    assert_eq!(e.line, line, "{text:?}: {e}");
    assert!(e.message.contains(says), "{text:?}: {e:?} does not say {says:?}");
}

/// [`LEAST`] and then `more`, refused at `line` and saying `says`.
fn refused_after(more: &str, line: usize, says: &str) {
    refused(&format!("{LEAST}{more}"), Some(line), says);
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

// --- each directive -------------------------------------------------------

/// **`DESIGN.md` §8's own example is a site**, every directive in it, and
/// reads as that section says it does: a bare `listen` address takes port
/// 42042, `tree` is a mount and read-only, and a `peer` without a port is
/// at 42042.
#[test]
fn the_designs_example_is_read_whole() {
    let text = "\
address  3060                          # this host's Chaos address; required
name     MIT-OZ OZ                     # its names, the official first; required
listen   192.0.2.10                    # optional; see §5
root     /srv/lispm                    # the base root; see §6
root     tree  /path/to/muir/vendor/system-100-0/sys  readonly
host     3050  MIT-LISPM-1 LM1   system=LISPM     # the site's host table, for HOSTAB
host     3051  MIT-LISPM-2 LM2   system=LISPM
peer     3040  192.0.2.5                          # an endpoint that is fixed; the rest are learned
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

/// **A comment is `#` to the end of its line, wherever it starts**, and a
/// line of nothing, or of spaces, or of a comment, is no line at all. A
/// `\r\n` ending is a line ending.
#[test]
fn comments_and_blank_lines_are_nothing() {
    let text = "# the site\n\n   \n\taddress\t3060# no space before it\r\n\
                name MIT-OZ OZ   # two names\n# root /elsewhere\nroot /srv/lispm\n";
    assert_eq!(Config::parse(text).unwrap(), Config::parse(LEAST).unwrap());
}

/// **An address is octal or `subnet:host`**, in every directive that takes
/// one: `6:60` is 3060.
#[test]
fn an_address_is_octal_or_subnet_host() {
    let config = Config::parse(
        "address 6:60\nname OZ\nroot /srv/lispm\nhost 6:50 LM1\npeer 6:40 192.0.2.5\n",
    )
    .unwrap();
    assert_eq!(config.address, 0o3060);
    assert_eq!(config.hosts[0].address, 0o3050);
    assert_eq!(config.peers[0].address, 0o3040);
}

/// **The names keep their order**: the first is the official one.
#[test]
fn the_official_name_is_first() {
    let config = Config::parse("address 3060\nname MIT-OZ OZ ZO\nroot /srv/lispm\n").unwrap();
    assert_eq!(config.names, ["MIT-OZ", "OZ", "ZO"]);
}

/// **The `name` line gives this host's system type**, `system=` as a
/// `host` line gives one: anywhere after the directive, and the other
/// words are the names, in their order. Without it, none.
#[test]
fn the_name_line_gives_this_hosts_system_type() {
    for line in
        ["name MIT-OZ OZ system=UNIX", "name system=UNIX MIT-OZ OZ", "name MIT-OZ system=UNIX OZ"]
    {
        let config = Config::parse(&format!("address 3060\n{line}\nroot /srv/lispm\n")).unwrap();
        assert_eq!(config.names, ["MIT-OZ", "OZ"], "{line}");
        assert_eq!(config.system.as_deref(), Some("UNIX"), "{line}");
    }
    assert_eq!(Config::parse(LEAST).unwrap().system, None);
}

/// **Without `listen`, the loopback at 42042**: a fresh install answers
/// its own host and nothing else (`DESIGN.md` §5).
#[test]
fn without_listen_it_is_the_loopback_at_42042() {
    assert_eq!(Config::parse(LEAST).unwrap().listen, at("127.0.0.1:42042"));
}

/// **`listen` takes muir's `--chaos-udp` forms**: nothing, a port on the
/// loopback, an address at 42042, or both; `0.0.0.0` and `::` every
/// interface; IPv6 bare, or in brackets before a port. Port 0 is one the
/// system picks.
#[test]
fn listen_takes_the_forms_of_chaos_udp() {
    for (line, want) in [
        ("listen", "127.0.0.1:42042"),
        ("listen 42043", "127.0.0.1:42043"),
        ("listen 0", "127.0.0.1:0"),
        ("listen 127.0.0.1", "127.0.0.1:42042"),
        ("listen 192.0.2.10", "192.0.2.10:42042"),
        ("listen 192.0.2.10:42043", "192.0.2.10:42043"),
        ("listen 0.0.0.0", "0.0.0.0:42042"),
        ("listen 0.0.0.0:42043", "0.0.0.0:42043"),
        ("listen ::", "[::]:42042"),
        ("listen ::1", "[::1]:42042"),
        ("listen [::1]:42043", "[::1]:42043"),
        ("listen [::]:42043", "[::]:42043"),
    ] {
        assert_eq!(with(&format!("{line}\n")).listen, at(want), "{line}");
    }
}

/// **A base root and mounts**, each with its own `readonly`, in the order
/// written. The base has no name; a mount's is before its path.
#[test]
fn a_root_is_a_base_or_a_mount() {
    let config = Config::parse(
        "address 3060\nname OZ\nroot /srv/lispm readonly\n\
         root tree /path/to/muir/vendor/system-100-0/sys readonly\nroot doc /srv/doc\n",
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
    let config = Config::parse("address 3060\nname OZ\nroot sys /srv/sys readonly\n").unwrap();
    assert_eq!(config.roots, [root(Some("sys"), "/srv/sys", true)]);
}

/// **A `host` line is an address, its names, and a system type if it has
/// one**, in the order written.
#[test]
fn a_host_is_an_address_and_names() {
    let config = with("host 3050 MIT-LISPM-1 CADR-1 LM1 system=LISPM\nhost 3040 CBRIDGE\n");
    assert_eq!(
        config.hosts,
        [
            host(0o3050, &["MIT-LISPM-1", "CADR-1", "LM1"], Some("LISPM")),
            host(0o3040, &["CBRIDGE"], None),
        ]
    );
}

/// **A `peer` line fixes an endpoint**: an IP literal, at 42042 unless a
/// port is given, IPv6 as `listen` takes it.
#[test]
fn a_peer_is_an_address_and_an_endpoint() {
    let config = with(
        "peer 3040 192.0.2.5\npeer 3041 192.0.2.5:42043\npeer 3042 ::1\n\
         peer 3043 [::1]:42044\npeer 3044 127.0.0.1:42045\n",
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

/// **A `host` and a `peer` at one address are one host, named and
/// placed**: the host table and the endpoints are separate (`DESIGN.md`
/// §8), and neither answers the other's question.
#[test]
fn a_host_and_a_peer_at_one_address_are_one_host() {
    let config = with("host 3040 CBRIDGE\npeer 3040 192.0.2.5\n");
    assert_eq!(config.hosts[0].address, 0o3040);
    assert_eq!(config.peers[0].address, 0o3040);
}

// --- each refusal, with its line ---------------------------------------

/// **A word that is not a directive is refused, and so is a directive in
/// capitals**: the directives are lower case.
#[test]
fn an_unknown_directive_is_refused() {
    refused_after("frob 1\n", 4, "not a directive");
    refused_after("Listen 42043\n", 4, "not a directive");
    refused_after("readonly\n", 4, "not a directive");
}

/// **`address`, `name` and a `root` are required**: with none, the refusal
/// has no line to name, and says which is missing.
#[test]
fn what_is_required_is_there() {
    refused("", None, "no address");
    refused("# nothing\n\n", None, "no address");
    refused("name OZ\nroot /srv/lispm\n", None, "no address");
    refused("address 3060\nroot /srv/lispm\n", None, "no name");
    refused("address 3060\nname OZ\n", None, "no root");
}

/// **`address`, `name` and `listen` are one line each**, and the second
/// names the first.
#[test]
fn what_is_once_is_once() {
    refused_after("address 3061\n", 4, "line 1");
    refused_after("name ZO\n", 4, "line 2");
    refused_after("listen 42043\nlisten 42044\n", 5, "line 4");
}

/// **`address` is one address, neither half zero** (`parse_address`): a
/// zero host is every host of the subnet, and a zero subnet no subnet.
#[test]
fn an_address_is_refused_by_parse_address() {
    for bad in ["6:0", "0:60", "377", "3068", "400:1", "OZ", "-3060"] {
        refused(&format!("address {bad}\nname OZ\nroot /srv/lispm\n"), Some(1), "not an address");
    }
    refused("address\nname OZ\nroot /srv/lispm\n", Some(1), "wants one address");
    refused("address 3060 3061\nname OZ\nroot /srv/lispm\n", Some(1), "wants one address");
}

/// **`name` gives at least one name**, and a word with `=` in it is an
/// attribute, as on a `host` line: `system=`, given a type, once, is the
/// one there is.
#[test]
fn a_name_line_is_names() {
    refused("address 3060\nname\nroot /srv/lispm\n", Some(2), "at least one name");
    refused("address 3060\nname system=UNIX\nroot /srv/lispm\n", Some(2), "at least one name");
    refused("address 3060\nname OZ system=\nroot /srv/lispm\n", Some(2), "system= wants a type");
    refused(
        "address 3060\nname OZ system=UNIX system=ITS\nroot /srv/lispm\n",
        Some(2),
        "system= twice",
    );
    refused("address 3060\nname OZ machine=VAX\nroot /srv/lispm\n", Some(2), "the one attribute");
}

/// **`listen` takes IP literals and never a name**, one endpoint, and a
/// port that is a port.
#[test]
fn listen_takes_no_names_and_no_nonsense() {
    for bad in ["localhost", "localhost:42042", "70000", "192.0.2.10:", "192.0.2.10:x", "192.0.2"] {
        refused_after(&format!("listen {bad}\n"), 4, "not a port");
    }
    refused_after("listen 192.0.2.10 42043\n", 4, "at most one");
}

/// **A root's path is absolute**, the base's and a mount's; that it exists
/// is the startup's to check (`DESIGN.md` §6), not the file's.
#[test]
fn a_root_path_is_absolute() {
    refused("address 3060\nname OZ\nroot srv/lispm\n", Some(3), "not absolute");
    refused("address 3060\nname OZ\nroot readonly\n", Some(3), "not absolute");
    refused_after("root tree path/to/sys readonly\n", 4, "not absolute");
    refused_after("root tree ./sys\n", 4, "not absolute");
}

/// **One base, and each mount's name once**; the second names the first.
#[test]
fn one_base_and_each_mount_once() {
    refused_after("root /srv/other\n", 4, "line 3");
    refused_after("root tree /srv/a\nroot tree /srv/b readonly\n", 5, "line 4");
}

/// **A mount's name is one directory name at the top of the tree, in
/// lower case**: a band sends its pathnames in lower case and names match
/// exactly (`DESIGN.md` §6), so a `TREE` would never be reached.
#[test]
fn a_mount_name_is_one_lower_case_directory_name() {
    for bad in ["a/b", "/tree", ".", ".."] {
        refused_after(&format!("root {bad} /srv/tree\n"), 4, "not a mount's name");
    }
    refused_after("root TREE /srv/tree\n", 4, "lower case");
    refused_after("root Tree /srv/tree\n", 4, "lower case");
}

/// **`root` is a path, a name before it or not, and `readonly` after it or
/// not**, and nothing else.
#[test]
fn a_root_line_is_no_more_than_that() {
    refused_after("root\n", 4, "root wants");
    refused_after("root tree /srv/tree ro\n", 4, "root wants");
    refused_after("root tree /srv/tree readonly readonly\n", 4, "root wants");
}

/// **A `host` line is a good address and at least one name**; its one
/// attribute is `system=`, given a type, once.
#[test]
fn a_host_line_is_refused_for_what_is_wrong_with_it() {
    refused_after("host\n", 4, "host wants");
    refused_after("host 3050\n", 4, "host wants");
    refused_after("host 3050 system=LISPM\n", 4, "host wants");
    refused_after("host 6:0 LM1\n", 4, "not an address");
    refused_after("host LM1 3050\n", 4, "not an address");
    refused_after("host 3050 LM1 system=\n", 4, "system= wants a type");
    refused_after("host 3050 LM1 system=LISPM system=ITS\n", 4, "system= twice");
    refused_after("host 3050 LM1 machine=LISPM\n", 4, "the one attribute");
}

/// **A system type is upper case**, on the `name` line and a `host` line
/// alike, and refused with its line otherwise: HOSTAB's user end interns
/// it as sent (`sys/network/chaos/chuse.lisp:983`), and a flavor is filed
/// under `:LISPM`, never `:lispm` (`sys/network/host.lisp:279`). Upper case
/// is all that is asked: a hyphen or a digit is no lower-case letter, and a
/// type the band files no flavor under, as `WAITS`, is taken --- the band
/// gives it the default flavor, as it gives a host with no type.
#[test]
fn a_system_type_is_upper_case() {
    refused("address 3060\nroot /srv/lispm\nname OZ system=unix\n", Some(3), "upper case");
    refused("address 3060\nroot /srv/lispm\nname system=Unix OZ\n", Some(3), "upper case");
    refused_after("host 3050 LM1 system=lispm\n", 4, "upper case");
    refused_after("root tree /srv/tree\nhost 3050 LM1 system=LISPm\n", 5, "upper case");
    let config = with("host 3050 LM1 system=TOPS-20\nhost 3051 LM2 system=WAITS\n");
    assert_eq!(config.hosts[0].system.as_deref(), Some("TOPS-20"));
    assert_eq!(config.hosts[1].system.as_deref(), Some("WAITS"));
}

/// **A `peer` is a good address and an IP literal to send to**: never a
/// name, never a bare port, never every interface or port 0.
#[test]
fn a_peer_line_is_refused_for_what_is_wrong_with_it() {
    refused_after("peer\n", 4, "peer wants");
    refused_after("peer 3040\n", 4, "peer wants");
    refused_after("peer 3040 192.0.2.5 192.0.2.6\n", 4, "peer wants");
    refused_after("peer 6:0 192.0.2.5\n", 4, "not an address");
    for bad in ["localhost", "cbridge.example.org:42042", "42043", "192.0.2.5:x", "192.0.2.5:"] {
        refused_after(&format!("peer 3040 {bad}\n"), 4, "not an IP address");
    }
    for bad in ["0.0.0.0", "0.0.0.0:42042", "::", "[::]:42042", "192.0.2.5:0"] {
        refused_after(&format!("peer 3040 {bad}\n"), 4, "to send to");
    }
}

/// **An address is one host in the host table, and one endpoint**: this
/// host's own address has no `host` line --- its names are `name`'s --- and
/// no `peer` line, since it is not its own peer; two `host` lines at one
/// address are two answers to whose it is, and two `peer` lines two
/// answers to where it is. The later line is refused, whichever order they
/// come in, naming the earlier.
#[test]
fn an_address_is_not_two_answers() {
    refused_after("host 3060 ZO\n", 4, "this host's own address, line 1");
    refused_after("peer 3060 192.0.2.5\n", 4, "this host's own address, line 1");
    refused_after("host 3050 LM1\nhost 3050 LM2\n", 5, "line 4");
    refused_after("peer 3040 192.0.2.5\npeer 3040 192.0.2.6\n", 5, "line 4");
    refused("host 3060 ZO\naddress 3060\nname OZ\nroot /srv/lispm\n", Some(2), "line 1");
    refused("peer 3060 192.0.2.5\naddress 3060\nname OZ\nroot /srv/lispm\n", Some(2), "line 1");
}

/// **A host name is one host**, this host's and every `host` line's
/// together, and ignoring case, since HOSTAB looks a name up ignoring case
/// (`DESIGN.md` §7). The later is refused, naming the earlier.
#[test]
fn a_host_name_is_one_host() {
    refused_after("host 3050 OZ\n", 4, "line 2");
    refused_after("host 3050 oz\n", 4, "line 2");
    refused_after("host 3050 LM1\nhost 3051 lm1\n", 5, "line 4");
    refused_after("host 3050 LM1 LM1\n", 4, "twice");
    refused("address 3060\nname OZ oz\nroot /srv/lispm\n", Some(2), "twice");
    refused("host 3050 OZ\naddress 3060\nname MIT-OZ OZ\nroot /srv/lispm\n", Some(3), "line 1");
}

/// **A refusal prints its line first**, and one with no line prints none.
#[test]
fn a_refusal_says_its_line() {
    let e = Config::parse(&format!("{LEAST}frob\n")).unwrap_err();
    assert!(e.to_string().starts_with("line 4: "), "{e}");
    let e = Config::parse("").unwrap_err();
    assert!(!e.to_string().starts_with("line"), "{e}");
}

// --- the examples, and loading --------------------------------------------

/// An example site file, loaded from the crate's `examples/`.
fn example(name: &str) -> Config {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(name);
    Config::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// **System 100's example is a site**: this host `MIT-OZ` at 3060, of
/// system type `UNIX` as the band's own table has it
/// (`sys/site/hosts.text`), the band's `MIT-LISPM-1` at 3050 in the host
/// table, the homes writable and the release's sources mounted read-only
/// at `/tree`, where the band asks for them. On the loopback, at 42042.
#[test]
fn the_system_100_example_is_a_site() {
    let config = example("system-100.conf");
    assert_eq!(config.address, 0o3060);
    assert_eq!(config.names[0], "MIT-OZ");
    assert_eq!(config.system.as_deref(), Some("UNIX"));
    assert_eq!(config.listen, at("127.0.0.1:42042"));
    assert_eq!(
        config.roots,
        [
            root(None, "/srv/lispm", false),
            root(Some("tree"), "/path/to/muir/vendor/system-100-0/sys", true),
        ]
    );
    assert!(config.hosts.iter().any(|h| h.address == 0o3050 && h.names[0] == "MIT-LISPM-1"));
    assert!(config.peers.is_empty(), "every endpoint is learned");
}

/// **System 304's example is a site**: this host `OZ` at 4403, also
/// `AMS-BRIDGE-1`, of no system type, since the host table in the sources
/// is MIT's and not this band's; the band's `AMS-LISPM-1` at 4401, and the
/// sources mounted read-only at `/sys`.
#[test]
fn the_system_304_example_is_a_site() {
    let config = example("system-304.conf");
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
    assert!(config.hosts.iter().any(|h| h.address == 0o4401 && h.names[0] == "AMS-LISPM-1"));
    assert!(config.peers.is_empty(), "every endpoint is learned");
}

/// **A file that is not there is refused**, with no line to name.
#[test]
fn a_file_that_is_not_there_is_refused() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join("no-such-site.conf");
    let e = Config::load(&path).unwrap_err();
    assert_eq!(e.line, None, "{e}");
}
