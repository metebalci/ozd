// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The band's own host table, read as HOSTAB's: `--hosts-text`
//! (`docs/design.md` §8), the file a site already keeps for its machines,
//! `sys/site/hosts.text`, so that a host is written once and not twice.
//!
//! **The format is MIT's**, and this reads of it what the band's own
//! generator reads (`GENERATE-HOST-TABLE-2`,
//! `sys/network/chaos/chsaux.lisp:1524`): a line whose first word is `HOST`
//! is a host, and every other line is skipped --- a `NET` line, which
//! declares the network to the table's own assembler and which the band
//! ignores too, a comment, a blank. A host's fields are parted by commas:
//!
//! ```text
//! HOST MIT-LISPM-1,    CHAOS 3050,USER,LISPM,LISPM,[CADR-1,CADR1,LM1]
//! ```
//!
//! the official name, the addresses, the status, the system type, the
//! machine type, and the nicknames in brackets. An address is a network's
//! name and a number, and a Chaosnet one is octal (`ZWEI:PARSE-NUMBER
//! ... 8`, `chsaux.lisp:1618`); several may be given, in brackets.
//!
//! **What is taken is what HOSTAB answers with** (`docs/design.md` §7): the
//! names, the official first, the Chaos address, and the system type. The
//! status and the machine type are not answered --- `MACHINE-TYPE` the band
//! would read as an address (`docs/protocols.md`, HOSTAB) --- and are read past.
//!
//! **A host with no Chaos address is skipped.** MIT's tables carry hosts of
//! other networks, written `1/14` for the ARPANET (`sys/site/extra.hosts`),
//! and HOSTAB here answers for Chaosnet hosts alone; a site's own table may
//! hold them, and they are not an error. **A Chaos address that is not one
//! is refused**, with its line: a typo there would leave a machine
//! unreachable by name, and silence would hide it.
//!
//! Nothing here touches the disk: `main` reads the file, as it reads the
//! file of flags, and hands the text to [`parse`]. The hosts it gives go
//! before the flags' own ([`crate::config::Config::add_hosts`]), each name
//! and each address still one host's.

use crate::address::parse_address;
use crate::config::{Error, Host, Place};

/// The hosts the text of a host table gives, in the order written; or the
/// first line refused. Every line that is not a `HOST` line is skipped, and
/// so is a host with no Chaos address (the module documentation).
pub fn parse(text: &str) -> Result<Vec<Host>, Error> {
    let mut hosts = Vec::new();
    for (k, line) in text.lines().enumerate() {
        let n = k + 1;
        // A comment is from a `;` to the end of its line, wherever it
        // begins: MIT's table heads itself with them, and writes one after
        // its `NET` line.
        let line = line.split(';').next().unwrap_or("").trim();
        let Some(rest) = after_host(line) else { continue };
        match host_of(rest) {
            Err(what) => {
                return Err(Error { place: Some(Place::Line(n)), usage: false, message: what });
            }
            Ok(Some(host)) => hosts.push(host),
            Ok(None) => {}
        }
    }
    Ok(hosts)
}

/// What follows `HOST` on a host's line, if the line is one: its first
/// word decides, as the band's generator decides it (`STRING-EQUAL`,
/// `chsaux.lisp:1536`), which compares ignoring case.
fn after_host(line: &str) -> Option<&str> {
    let (word, rest) = match line.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim_start()),
        None => (line, ""),
    };
    word.eq_ignore_ascii_case("HOST").then_some(rest)
}

/// The host a `HOST` line's fields give, or `None` where it has no Chaos
/// address; or what is wrong with the line.
fn host_of(rest: &str) -> Result<Option<Host>, String> {
    let fields = fields(rest);
    let name = fields.first().map_or("", |f| f.trim());
    if name.is_empty() || fields.len() < 2 {
        return Err("wants a name and an address, as HOST MIT-OZ, CHAOS 3060".to_string());
    }
    let Some(address) = address_of(fields[1])? else {
        return Ok(None);
    };
    let mut names = vec![printable(name)?.to_string()];
    // The nicknames are the last field, in brackets; an empty pair is no
    // name, however the band's own parser reads it.
    if let Some(last) = fields.last().filter(|f| f.trim_start().starts_with('[')) {
        for nickname in brackets(last).split(',') {
            let nickname = nickname.trim();
            if !nickname.is_empty() {
                names.push(printable(nickname)?.to_string());
            }
        }
    }
    let system = match fields.get(3).map(|f| f.trim()).filter(|f| !f.is_empty()) {
        None => None,
        Some(t) if t.chars().any(char::is_lowercase) => {
            return Err(format!(
                "{t}: a system type is upper case, as {}, since the band takes it as sent",
                t.to_uppercase()
            ));
        }
        Some(t) => Some(printable(t)?.to_string()),
    };
    Ok(Some(Host { address, names, system }))
}

/// The fields of a host's line, parted by the commas outside its brackets:
/// an address field may hold several addresses, in brackets and parted by
/// commas of its own, and the nicknames are one field likewise.
fn fields(rest: &str) -> Vec<&str> {
    let (mut fields, mut start, mut depth) = (Vec::new(), 0, 0u32);
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                fields.push(&rest[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    fields.push(&rest[start..]);
    fields
}

/// What is between a field's brackets, or the field itself where it has
/// none.
fn brackets(field: &str) -> &str {
    field.trim().trim_start_matches('[').trim_end_matches(']')
}

/// The host's Chaos address, the first its address field gives; `None`
/// where the field names no Chaosnet address, which a host of another
/// network has; or why an address of its is not one.
///
/// An address is a network's name and a number, `CHAOS 3060`, and the
/// Chaosnet's is octal, as [`parse_address`] takes one. A field with no
/// network name, `1/14`, is the ARPANET's (`sys/site/extra.hosts`).
fn address_of(field: &str) -> Result<Option<u16>, String> {
    for address in brackets(field).split(',') {
        let address = address.trim();
        let Some((network, number)) = address.split_once(char::is_whitespace) else {
            continue;
        };
        if !network.eq_ignore_ascii_case("CHAOS") {
            continue;
        }
        let number = number.trim();
        return match parse_address(number) {
            Some(a) => Ok(Some(a)),
            None => Err(format!("CHAOS {number}: not an address --- octal, and neither half zero")),
        };
    }
    Ok(None)
}

/// `word` itself, or why it is no name: a name and a system type are
/// printable ASCII, as in a `--host` (`crate::config`), since HOSTAB sends
/// each a byte a character.
fn printable(word: &str) -> Result<&str, String> {
    if word.chars().all(|c| c.is_ascii_graphic()) {
        Ok(word)
    } else {
        Err(format!("{word:?}: not printable ASCII, which a name is"))
    }
}
