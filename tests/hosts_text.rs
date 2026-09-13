// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The band's own host table as HOSTAB's, `--hosts-text` (`DESIGN.md` §8):
//! `sys/site/hosts.text`, the file a site keeps for its machines, read here
//! so that a site writes its hosts once instead of twice.
//!
//! **The format is MIT's, and this reads what the band's own generator
//! reads** (`GENERATE-HOST-TABLE-2`, `sys/network/chaos/chsaux.lisp:1524`):
//! a line whose first word is `HOST` is a host, and every other line ---
//! a `NET` line, a comment, a blank --- is skipped. A host's fields are
//! parted by commas: its official name, its addresses, its status, its
//! system type, its machine type, and its nicknames in brackets. An
//! address is a network's name and a number, `CHAOS 3060`, the Chaosnet's
//! in octal (`ZWEI:PARSE-NUMBER ... 8`, `chsaux.lisp:1618`).
//!
//! What ozd takes of it is what HOSTAB answers with (`DESIGN.md` §7): the
//! names, the Chaos address, and the system type. A host with no Chaos
//! address is not one HOSTAB can answer for, and is skipped as the
//! machine's own table skips what has no address it can reach.

use ozd::config::{Config, Error, Host, Place};
use ozd::hosts_text;
use std::path::Path;

/// System 100's own table, as the release ships it (`sys/site/hosts.text`),
/// with its mode line, its `NET` line and its two hosts.
const SYSTEM_100: &str = "\
;;; -*- Mode:Fundamental;Base:8 -*-

NET CHAOS,\t  7

HOST MIT-LISPM-1,\tCHAOS 3050,USER,LISPM,LISPM,[CADR-1,CADR1,LM1]
HOST MIT-OZ,\t\tCHAOS 3060,SERVER,UNIX,VAX,[OZ]
";

fn host(address: u16, names: &[&str], system: Option<&str>) -> Host {
    Host {
        address,
        names: names.iter().map(|n| n.to_string()).collect(),
        system: system.map(str::to_string),
    }
}

/// The hosts `text` gives, which must be taken.
fn hosts(text: &str) -> Vec<Host> {
    hosts_text::parse(text).unwrap_or_else(|e| panic!("{text:?} was refused: {e}"))
}

/// Why `text` is refused: the first thing wrong with it.
fn refusal(text: &str) -> Error {
    match hosts_text::parse(text) {
        Ok(hosts) => panic!("{text:?} was taken: {hosts:?}"),
        Err(e) => e,
    }
}

/// `text` refused at `line`, saying `says`.
fn refused(text: &str, line: usize, says: &str) {
    let e = refusal(text);
    assert_eq!(e.place, Some(Place::Line(line)), "{text:?}: {e}");
    assert!(e.message.contains(says), "{text:?}: {e:?} does not say {says:?}");
}

/// **A `HOST` line is names, a Chaos address and a system type**, and
/// everything else in the file is skipped: the mode line, the `NET` line
/// and the blanks. The official name comes first and the bracketed
/// nicknames after it, in the order written, as `--host` takes them.
#[test]
fn a_host_line_is_names_an_address_and_a_system_type() {
    assert_eq!(
        hosts(SYSTEM_100),
        [
            host(0o3050, &["MIT-LISPM-1", "CADR-1", "CADR1", "LM1"], Some("LISPM")),
            host(0o3060, &["MIT-OZ", "OZ"], Some("UNIX")),
        ]
    );
}

/// **An address is octal**, as the band reads a Chaosnet one
/// (`ZWEI:PARSE-NUMBER ... 8`, `chsaux.lisp:1618`), so `3060` is 1584
/// decimal and not three thousand and sixty. A line with no `CHAOS` address is
/// no host HOSTAB can answer for --- the ARPA-style `1/14` of
/// `sys/site/extra.hosts` is one --- and is skipped rather than refused.
#[test]
fn an_address_is_octal_and_a_host_without_one_is_skipped() {
    let text = "\
HOST OZ,\t\tCHAOS 3060,SERVER,UNIX,VAX
HOST CMU-CS-A,\t1/14,SERVER,TOPS-10,PDP10,[CMU-10A,CMUA]
";
    assert_eq!(hosts(text), [host(0o3060, &["OZ"], Some("UNIX"))]);
}

/// **The first Chaos address of a line is the host's.** A line may give
/// several, in brackets and parted by commas, as `SCH-DONNER` does in
/// `sys/site/extra.hosts`; HOSTAB answers with one address
/// ([`ozd::service::hostab`]), and the first is the one the band's own
/// table puts first.
#[test]
fn the_first_chaos_address_is_the_hosts() {
    let text = "HOST DONNER,\t[CHAOS 22401,CHAOS 23001],USER,MINITS,PDP11,[DONDER]\n";
    assert_eq!(hosts(text), [host(0o22401, &["DONNER", "DONDER"], Some("MINITS"))]);
}

/// **An empty bracket pair is no name at all.** A line ending `,[]` is one
/// a site writes for a host with no nicknames, and the band's own parser
/// takes the empty text between the brackets for a name --- a name no
/// pathname and no `(hostat)` could ever be. Here it is nothing.
#[test]
fn an_empty_bracket_pair_is_no_name() {
    let text = "HOST OZ,\t\tCHAOS 3060,SERVER,UNIX,VAX,[]\n";
    assert_eq!(hosts(text), [host(0o3060, &["OZ"], Some("UNIX"))]);
}

/// **A comment is from a `;` to the end of its line**, wherever it begins:
/// MIT's own table heads itself with them and writes one after its `NET`
/// line ("Supported by HOSTS2", `sys/site/hosts.text` in System 304).
#[test]
fn a_comment_runs_to_the_end_of_its_line() {
    let text = "\
; the site's hosts
NET CHAOS,\t7\t; supported by HOSTS2
HOST OZ,\tCHAOS 3060,SERVER,UNIX,VAX\t; the file host
";
    assert_eq!(hosts(text), [host(0o3060, &["OZ"], Some("UNIX"))]);
}

/// **A line with no system type has none**, as a `--host` without
/// `system=` has none, and HOSTAB then sends no `SYSTEM-TYPE`
/// (`DESIGN.md` §7). The fields after the address may be empty or missing
/// altogether.
#[test]
fn a_line_without_a_system_type_has_none() {
    let text = "\
HOST BRIDGE-1,\tCHAOS 3040
HOST BRIDGE-2,\tCHAOS 3041,USER
HOST BRIDGE-3,\tCHAOS 3042,USER,,PDP11
";
    assert_eq!(
        hosts(text),
        [
            host(0o3040, &["BRIDGE-1"], None),
            host(0o3041, &["BRIDGE-2"], None),
            host(0o3042, &["BRIDGE-3"], None),
        ]
    );
}

/// **An address that is not one is refused, with its line.** A typo in the
/// site's own table would leave a machine unreachable by name, and quiet
/// about it: the file is the site's, and what it says about a Chaosnet host
/// is either read or refused.
#[test]
fn an_address_that_is_not_one_is_refused() {
    refused("HOST OZ,\tCHAOS 3068,SERVER,UNIX,VAX\n", 1, "not an address");
    refused("\n\nHOST OZ,\tCHAOS abc,SERVER,UNIX,VAX\n", 3, "not an address");
    refused("HOST OZ,\tCHAOS 3060 3061,SERVER,UNIX\n", 1, "not an address");
}

/// **A host line wants a name and an address.** `HOST` alone, or a name
/// with nothing after it, is a line no band wrote.
#[test]
fn a_host_line_wants_a_name_and_an_address() {
    refused("HOST\n", 1, "wants a name");
    refused("HOST OZ\n", 1, "wants a name");
    refused("HOST ,CHAOS 3060\n", 1, "wants a name");
}

/// **A name is printable ASCII and a system type upper case**, as they are
/// in a `--host` (`src/config.rs`): HOSTAB sends a name a byte a
/// character, and the band interns a system type as it is sent
/// (`chuse.lisp:983`), so a lower-case one names no flavor.
#[test]
fn a_name_is_printable_ascii_and_a_system_type_upper_case() {
    refused("HOST OZ,\tCHAOS 3060,SERVER,unix,VAX\n", 1, "upper case");
    refused("HOST Ö,\tCHAOS 3060,SERVER,UNIX,VAX\n", 1, "printable ASCII");
}

/// **A file of nothing but comments and `NET` lines is no hosts**, and not
/// an error: a site may keep its machines in `--host` flags and its table
/// for the bands alone.
#[test]
fn a_table_with_no_hosts_is_no_hosts() {
    assert_eq!(hosts("; nothing here\n\nNET CHAOS,\t7\n"), []);
    assert_eq!(hosts(""), []);
}

/// **The table's hosts come before the flags' own**, so that `--host` adds
/// to the site's table rather than being buried in it, and every name is
/// still one host's: a name in both is refused, naming the table.
#[test]
fn the_tables_hosts_come_first_and_a_name_is_still_one_hosts() {
    let table = "\
HOST MIT-LISPM-1,\tCHAOS 3050,USER,LISPM,LISPM,[CADR-1,CADR1,LM1]
HOST MIT-FILE,\t\tCHAOS 3061,SERVER,UNIX,VAX,[FILE]
";
    let site = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n--host 3040,BRIDGE-1\n";
    let mut config = Config::parse(site).expect("the site's flags");
    config.add_hosts(hosts(table)).expect("the table");
    assert_eq!(
        config.hosts,
        [
            host(0o3050, &["MIT-LISPM-1", "CADR-1", "CADR1", "LM1"], Some("LISPM")),
            host(0o3061, &["MIT-FILE", "FILE"], Some("UNIX")),
            host(0o3040, &["BRIDGE-1"], None),
        ],
        "the table's first, in its order, then the flags'"
    );
}

/// **This host's own line is passed over.** A site's table holds every
/// host of the site, this one among them --- System 100's holds `MIT-OZ`
/// at 3060 --- and here this host's names are `--name`'s and its address
/// `--address`'s. The line is what this host already answers for, so it is
/// not a second host at that address, and a site can name its own table
/// without editing this host out of it.
#[test]
fn this_hosts_own_line_is_passed_over() {
    let site = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n";
    let mut config = Config::parse(site).expect("the site's flags");
    config.add_hosts(hosts(SYSTEM_100)).expect("the table, this host's line and all");
    assert_eq!(
        config.hosts,
        [host(0o3050, &["MIT-LISPM-1", "CADR-1", "CADR1", "LM1"], Some("LISPM"))],
        "the machine, and no second MIT-OZ"
    );
}

/// **A name the flags give already is refused**, as two `--host`s at one
/// name are (`src/config.rs`), since HOSTAB would otherwise find two hosts
/// for it; and so is one this host answers to, whose names are `--name`'s.
#[test]
fn a_name_given_twice_is_refused() {
    let site = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n--host 3050,LM1\n";
    let mut config = Config::parse(site).expect("the site's flags");
    let e = config.add_hosts(hosts("HOST LM1,\tCHAOS 3051,USER,LISPM,LISPM\n")).unwrap_err();
    assert!(e.contains("LM1"), "{e}");
    let mut config = Config::parse(site).expect("the site's flags");
    let e = config.add_hosts(hosts("HOST OZ,\tCHAOS 3052,USER,LISPM,LISPM\n")).unwrap_err();
    assert!(e.contains("OZ"), "{e}");
}

/// **An address the flags give already is refused**, as two `--host`s at
/// one address are: one host is one answer to where it is. Two lines of
/// the table at one address are refused the same way.
#[test]
fn an_address_given_twice_is_refused() {
    let site = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n--host 3050,LM1\n";
    let mut config = Config::parse(site).expect("the site's flags");
    let e = config.add_hosts(hosts("HOST CADR-1,\tCHAOS 3050,USER,LISPM,LISPM\n")).unwrap_err();
    assert!(e.contains("3050"), "{e}");
    let table = "HOST ONE,\tCHAOS 3051,USER,LISPM,LISPM\nHOST TWO,\tCHAOS 3051,USER,LISPM,LISPM\n";
    let mut config = Config::parse(site).expect("the site's flags");
    let e = config.add_hosts(hosts(table)).unwrap_err();
    assert!(e.contains("3051"), "{e}");
}

// --- the flag --------------------------------------------------------------

/// The least a file of flags can be, as `tests/config.rs` has it.
const LEAST: &str = "--address 3060\n--name MIT-OZ,OZ\n--root /srv/lispm\n";

/// **`--hosts-text` names the file, and nothing more happens here**: the
/// flags touch no disk, so the file is read where the roots are checked
/// (`src/config.rs`). It is given once, and without it there is no table.
#[test]
fn the_flag_names_a_file_and_is_given_once() {
    let text = format!("{LEAST}--hosts-text /srv/lispm/sys/site/hosts.text\n");
    let config = Config::parse(&text).expect("the site's flags");
    assert_eq!(config.hosts_text.as_deref(), Some(Path::new("/srv/lispm/sys/site/hosts.text")));
    assert_eq!(Config::parse(LEAST).unwrap().hosts_text, None, "and none without it");
}

/// **Twice is refused**, as `--address`, `--name` and `--listen` are: one
/// table a site.
#[test]
fn the_flag_twice_is_refused() {
    let text = format!("{LEAST}--hosts-text /one\n--hosts-text /two\n");
    let e = Config::parse(&text).expect_err("two tables");
    assert_eq!(e.place, Some(Place::Line(5)), "{e}");
    assert!(e.message.contains("once only"), "{e}");
}

/// **The flag wants its file**, which is the shape of what was given and so
/// a usage error, as every flag missing its value is.
#[test]
fn the_flag_wants_a_file() {
    let e = Config::parse(&format!("{LEAST}--hosts-text\n")).expect_err("no file");
    assert!(e.usage, "{e} is no usage error");
    assert!(e.message.contains("--hosts-text"), "{e}");
}
