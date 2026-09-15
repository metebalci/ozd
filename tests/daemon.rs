// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon (`docs/design.md` §4): its services asked over loopback by a
//! test host, at a clock the test sets; the turn that wakes when nothing
//! arrives, which is what keeps a retransmission on time; the time a log
//! line carries (§10); and the command line, run as the binary.

mod support;

use ozd::log;
use ozd::ncp::{self, op};
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{
    LM1, OZ, Recorder, SECOND, TestHost, ask, daemon, datagram, hear, meters, ozd, packet, rfc,
    settle, site,
};

/// **STATUS over loopback, its meters counting what the test sent.** The
/// link counts at the socket, into the meters it shares with STATUS
/// (`docs/design.md` §7): two datagrams that are no packet --- one too short,
/// one of another version --- and then the RFC are three received, one
/// rejected for its length and one for anything else, and nothing sent
/// yet when the answer is made. The answer is then one sent, and the next
/// STATUS says so. The block is for subnet 6, this host's, and the name
/// is the official one (`HOSTAT`, `sys/network/chaos/chsaux.lisp`).
#[test]
fn status_over_loopback_counts_what_the_test_sent() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let good = datagram(&rfc(LM1, OZ, "STATUS"), LM1);
    lm1.send_bytes(&good[..10]);
    let mut version = good.clone();
    version[0] = 2;
    lm1.send_bytes(&version);
    settle(&mut d, &mut [&mut lm1], 0);
    let status = |data: &[u8]| -> Vec<u32> {
        (0..8)
            .map(|k| u32::from_le_bytes(data[36 + 4 * k..40 + 4 * k].try_into().unwrap()))
            .collect()
    };
    let ans = ask(&mut d, &mut lm1, 0, "STATUS");
    let name = ans.data[..32].split(|&b| b == 0).next().unwrap();
    assert_eq!(name, b"MIT-OZ", "the official name");
    let word = |i: usize| u16::from_le_bytes([ans.data[2 * i], ans.data[2 * i + 1]]);
    assert_eq!((word(16), word(17)), (0o400 + 6, 16), "subnet 6's block, sixteen words");
    assert_eq!(status(&ans.data), [3, 0, 0, 0, 0, 0, 1, 1], "what the test sent");
    let ans = ask(&mut d, &mut lm1, 10, "STATUS");
    assert_eq!(status(&ans.data), [4, 1, 0, 0, 0, 0, 1, 1], "and the answer that went back");
}

/// **TIME is the system clock, over loopback**: universal time, seconds
/// since 1900, least significant byte first, within a second of the clock
/// read here (`docs/design.md` §11).
#[test]
fn time_over_loopback_is_the_system_clock() {
    // 1900 to 1970: seventy years of 365 days and seventeen leap days, 1904
    // to 1968 --- 1900 was not a leap year.
    let epoch = (70 * 365 + 17) * 86_400;
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let ans = ask(&mut d, &mut lm1, 0, "TIME");
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let answered = u32::from_le_bytes(ans.data[..].try_into().expect("four bytes"));
    let clock = (unix + epoch) as u32;
    assert!(clock.abs_diff(answered) <= 1, "answered {answered}, the clock says {clock}");
}

/// **UPTIME is 600 at ten seconds of the daemon's clock.** The daemon's
/// clock is nanoseconds since it started (`docs/design.md` §4), and UPTIME
/// answers sixtieths of a second since then (§7, `docs/protocols.md` UPTIME):
/// nought at the start, and exactly 600 ten seconds on.
#[test]
fn uptime_is_600_at_ten_seconds_of_the_daemons_clock() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    assert_eq!(ask(&mut d, &mut lm1, 0, "UPTIME").data, 0u32.to_le_bytes(), "at the start");
    assert_eq!(ask(&mut d, &mut lm1, 10 * SECOND, "UPTIME").data, 600u32.to_le_bytes());
}

/// **A retransmission happens on a turn with no packet arriving**, which
/// is why the loop waits for a datagram only so long (`docs/design.md` §4): the
/// NCP has no timer, and sends again what is unreceipted only when it is
/// asked for output. An RFC from this host to one that never answers goes
/// out at once; a turn short of `RETRANSMIT_NS` sends nothing; a turn at
/// it waits its wait for a datagram, has none, and sends the RFC again.
/// Nothing arrived through any of it.
#[test]
fn a_retransmission_happens_on_a_turn_with_no_packet_arriving() {
    let (lm1, lm1_at) = support::socket();
    let mut d = daemon(&site(&format!("--peer {LM1:o}@{lm1_at}\n")));
    d.ncp().connect(0, LM1, "STATUS", Box::new(Recorder::default()));
    d.turn(0, Duration::ZERO);
    let first = hear(&lm1);
    assert_eq!(first.len(), 1, "the RFC");
    let (p, _) = packet(&first[0]);
    assert_eq!((p.opcode, p.source, p.dest), (op::RFC, OZ, LM1));
    d.turn(ncp::RETRANSMIT_NS - 1, Duration::ZERO);
    assert!(hear(&lm1).is_empty(), "not yet");
    let wait = Duration::from_millis(50);
    let began = Instant::now();
    d.turn(ncp::RETRANSMIT_NS, wait);
    assert!(began.elapsed() >= wait / 2, "the turn waited for a datagram");
    assert_eq!(hear(&lm1), first, "and then sent the RFC again");
    assert_eq!(meters(&d), [0, 2, 0, 0], "with nothing arriving");
}

/// **A log line's time is UTC**, by `civil`, the one calendar function
/// (`docs/design.md` §10): the Unix epoch, a leap day in a century that has
/// one, the end of February in one that does not, and today. Each checked
/// against `date -u -d @<seconds>`.
#[test]
fn a_log_lines_time_is_utc() {
    assert_eq!(log::stamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(log::stamp(951_782_400), "2000-02-29T00:00:00Z", "2000 was a leap year");
    assert_eq!(log::stamp(951_868_799), "2000-02-29T23:59:59Z");
    assert_eq!(log::stamp(4_107_542_399), "2100-02-28T23:59:59Z", "2100 will not be");
    assert_eq!(log::stamp(4_107_542_400), "2100-03-01T00:00:00Z");
    assert_eq!(log::stamp(1_788_998_400), "2026-09-10T00:00:00Z");
    assert_eq!(log::civil(1_788_998_400 + 3661), (2026, 9, 10, 1, 1, 1));
}

// --- the command line ------------------------------------------------------

/// A file of flags holding `text`, in this test binary's scratch
/// directory.
fn flags_file(name: &str, text: &str) -> PathBuf {
    let path = support::scratch().join(name);
    std::fs::write(&path, text).expect("a file of flags");
    path
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Whether a run was refused for being root, which is all a run as root
/// can be once what it was given is flags (`docs/design.md` §6).
fn refused_as_root(out: &Output) -> bool {
    out.status.code() == Some(1) && said(out).contains("root")
}

/// How the usage begins: every usage error prints it on stderr, and the
/// help, first, on stdout.
const USAGE: &str = "usage: ozd [--address <addr>] [--name ";

/// **A command line that is not flags is refused with the usage**, on
/// stderr, with exit code 2, and nothing on stdout: a word that is not a
/// flag, an argument that is no flag's value, a flag missing its value,
/// and a file `-c` names that is not there --- each before who is running
/// it is looked at, so as root too. An argument that is no flag's value
/// says that the config is flags, on the command line or in `.ozdrc`,
/// or in the file `-c` names.
#[test]
fn a_command_line_that_is_not_flags_is_refused_with_the_usage() {
    let missing = support::scratch().join("no-such.ozdrc");
    let missing = missing.to_str().expect("a path in UTF-8");
    let lines: [&[&str]; 8] = [
        &["--frobnicate"],
        &["--frobnicate", "a.conf"],
        &["a.conf"],
        &["a.conf", "b.conf"],
        &["--check", "--address"],
        &["--address", "--name", "MIT-OZ"],
        &["-c"],
        &["-c", missing],
    ];
    for args in lines {
        let out = ozd().args(args).output().expect("it runs");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", said(&out));
        assert!(said(&out).contains(USAGE), "{args:?}: {}", said(&out));
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    let out = ozd().arg("site.conf").output().expect("it runs");
    let told = said(&out);
    for words in ["site.conf", "the config is flags", ".ozdrc", "-c <file>"] {
        assert!(told.contains(words), "{words} in {told}");
    }
}

/// **With nothing given, a required flag is missing**, exit 1. An empty
/// command line is no usage error, since a run whose flags are all in a
/// file has one; with no file of flags where the run looks, `--address` is
/// missing, said with where it may be given and that this run read no
/// file. As root, it is refused for that instead.
#[test]
fn with_nothing_given_a_required_flag_is_missing() {
    let lines: [&[&str]; 3] = [&[], &["--trace"], &["--check"]];
    for args in lines {
        let out = ozd().args(args).output().expect("it runs");
        if support::running_as_root() {
            assert!(refused_as_root(&out), "as root: {}", said(&out));
            continue;
        }
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", said(&out));
        let told = said(&out);
        assert!(told.starts_with("ozd: no --address"), "{args:?}: {told}");
        assert!(told.contains("on the command line or in a file of flags"), "{told}");
        assert!(told.contains("read none"), "{told}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

/// **`--check` reads the flags, binds nothing, and says what is wrong.** A
/// line of the file of flags that is not a flag is a usage error, exit 2,
/// with its file and line, before who is running it is looked at. A good
/// site exits 0, silent, even at a `--listen` whose port is taken --- which
/// a run that binds refuses, exit 1, naming it. A value refused is
/// `<file>: line N: <flag> <value>: ...` for a line of the file, and
/// `<flag> <value>: ...` for the command line, exit 1. As root, each of
/// those is refused for that instead.
#[test]
fn check_reads_the_flags_binds_nothing_and_says_what_is_wrong() {
    let misused = flags_file("check-misused.ozdrc", &site("--bogus\n"));
    let out = ozd().arg("--check").arg("-c").arg(&misused).output().expect("it runs");
    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    let want = format!("ozd: {}: line 5: --bogus: not a flag", misused.display());
    assert!(said(&out).starts_with(&want), "{}", said(&out));
    assert!(said(&out).contains(USAGE), "{}", said(&out));

    let (_taken, taken_at) = support::socket();
    let text = site("").replace("--listen 127.0.0.1:0", &format!("--listen {taken_at}"));
    assert!(text.contains(&format!("--listen {taken_at}")));
    let good = flags_file("check-good.ozdrc", &text);
    let out = ozd().arg("--check").arg("-c").arg(&good).output().expect("it runs");
    if support::running_as_root() {
        assert!(refused_as_root(&out), "as root: {}", said(&out));
        return;
    }
    assert!(out.status.success(), "{}", said(&out));
    assert!(out.stderr.is_empty() && out.stdout.is_empty(), "silent: {}", said(&out));
    let run = ozd().arg("-c").arg(&good).output().expect("it runs");
    assert_eq!(run.status.code(), Some(1), "{}", said(&run));
    assert!(said(&run).contains(&format!("--listen {taken_at}")), "{}", said(&run));

    let bad = flags_file("check-bad.ozdrc", &site("--host 6:0,LM1\n"));
    let out = ozd().arg("--check").arg("-c").arg(&bad).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let want = format!("{}: line 5: --host 6:0,LM1: ", bad.display());
    assert!(said(&out).starts_with(&want), "{}", said(&out));
    let out = ozd().args(["--check", "-c"]).arg(&good).args(["--address", "6:0"]).output();
    let out = out.expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).starts_with("--address 6:0: not an address"), "{}", said(&out));
}

/// A daemon run as a process, killed when the test is done with it.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// **The daemon runs from its command line, and answers STATUS.** The
/// whole of `main`: the site given by flags on the command line alone,
/// with no file of flags; the socket bound at a port the system picks; a
/// first log line with its UTC time saying where; and the loop turning
/// with the real clock --- a test host at LM1 asks STATUS, its own NCP
/// sending the RFC again on its own clock should one be lost, and has the
/// answer.
#[test]
fn the_daemon_runs_from_its_command_line_and_answers_status() {
    let oz = format!("{OZ:o}");
    let mut child = ozd()
        .args(["--address", &oz, "--name", "MIT-OZ,OZ", "--listen", "127.0.0.1:0", "--root"])
        .arg(support::own_root())
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("it starts");
    let stderr = child.stderr.take().expect("its stderr");
    let _running = Running(child);
    // The daemon may log its startup first --- a warning, a temporary
    // removed --- and says where it listens once it has bound.
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).expect("a line") > 0, "it ended before listening");
        if support::running_as_root() || line.contains("listening at ") {
            break;
        }
    }
    if support::running_as_root() {
        assert!(line.contains("root"), "as root: {line}");
        return;
    }
    let stamp = line.split(' ').next().unwrap_or("");
    assert!(stamp.len() == 20 && stamp.ends_with('Z') && &stamp[10..11] == "T", "{line:?}");
    let at: SocketAddr = line
        .split("listening at ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|w| w.parse().ok())
        .unwrap_or_else(|| panic!("where it listens, in {line:?}"));
    let mut lm1 = TestHost::new(LM1, at);
    let asked = Recorder::default();
    lm1.ncp.connect(0, OZ, "STATUS", Box::new(asked.clone()));
    let began = Instant::now();
    while asked.events().is_empty() {
        assert!(began.elapsed() < Duration::from_secs(10), "no answer from {at}");
        lm1.turn(began.elapsed().as_nanos() as u64);
    }
    assert_eq!(asked.events(), ["closed answered"]);
    let ans = lm1.packets().into_iter().find(|p| p.opcode == op::ANS).expect("the ANS");
    assert_eq!((ans.source, ans.dest), (OZ, LM1));
    assert!(ans.data.starts_with(b"MIT-OZ\0"), "this host's STATUS");
}

/// **HOSTAB, NAME and MINI are served**, each opened through the daemon as a
/// band opens it (`docs/design.md` §7). NAME answers its one line and an EOF;
/// HOSTAB opens and waits for a name, and MINI for an open --- what they
/// answer is `tests/hostab.rs`'s and `tests/mini.rs`'s business.
#[test]
fn hostab_name_and_mini_are_served() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let name = Recorder::default();
    lm1.ncp.connect(0, OZ, "NAME", Box::new(name.clone()));
    let hostab = Recorder::default();
    lm1.ncp.connect(0, OZ, "HOSTAB", Box::new(hostab.clone()));
    let mini = Recorder::default();
    lm1.ncp.connect(0, OZ, "MINI LISPM ", Box::new(mini.clone()));
    settle(&mut d, &mut [&mut lm1], 0);
    assert_eq!(mini.events(), ["opened"], "MINI opens and waits for an open");
    let name = name.events();
    assert_eq!(name.first().map(String::as_str), Some("opened"), "NAME opens: {name:?}");
    assert!(
        name.iter().any(|e| e.starts_with("data ") && e.contains("Nobody is logged in.")),
        "its line: {name:?}"
    );
    assert!(name.iter().any(|e| e == "eof"), "and an EOF: {name:?}");
    assert_eq!(hostab.events(), ["opened"], "HOSTAB opens and waits for a name");
}

/// **A `--hosts-text` table says at startup how many hosts it gave**
/// (`docs/design.md` §10), so that a site can see its table was read and read
/// whole. This host's own line is passed over, so a table naming it and
/// two machines is two hosts.
#[test]
fn the_host_table_says_how_many_hosts_it_gave() {
    let table = support::scratch().join("counted-hosts.text");
    std::fs::write(
        &table,
        "; the site's hosts\nNET CHAOS,\t7\n\
         HOST MIT-OZ,\tCHAOS 3060,SERVER,UNIX,VAX,[OZ]\n\
         HOST MIT-LISPM-1,\tCHAOS 3050,USER,LISPM,LISPM,[LM1]\n\
         HOST MIT-LISPM-2,\tCHAOS 3051,USER,LISPM,LISPM\n",
    )
    .expect("a host table");
    let flags = format!("{}--hosts-text {}\n", site(""), table.display());
    let path = flags_file("counted.ozdrc", &flags);
    let mut child = ozd()
        .arg("-c")
        .arg(&path)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("it starts");
    let stderr = child.stderr.take().expect("its stderr");
    let _running = Running(child);
    // Everything it says up to the line that means it is serving.
    let mut said = Vec::new();
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).expect("a line") > 0, "it ended before listening");
        said.push(line.clone());
        if support::running_as_root() || line.contains("listening at ") {
            break;
        }
    }
    if support::running_as_root() {
        return;
    }
    let counted = said.iter().find(|l| l.contains(" hosts from ")).unwrap_or_else(|| {
        panic!("no count of the table among {said:?}");
    });
    assert!(counted.contains("2 hosts from "), "the two machines, not this host: {counted:?}");
    assert!(counted.contains("counted-hosts.text"), "and which file: {counted:?}");
}

/// **The roots are checked before anything is bound**, by a run and by
/// `--check` alike (`docs/design.md` §6): a root that is not there, or `/`, is
/// a refusal to start, naming it, exit 1. A base directory a mount covers
/// is warned of, and the check still passes. Each case has a directory of
/// its own, since a daemon started by another test cleans its base.
#[test]
fn the_roots_are_checked_at_startup() {
    if support::running_as_root() {
        return;
    }
    let dir = support::scratch().join("roots-checked");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("base/tree")).unwrap();
    std::fs::create_dir_all(dir.join("mount")).unwrap();
    let head = format!("--address {OZ:o}\n--name MIT-OZ,OZ\n--listen 127.0.0.1:0\n");
    let check = |name: &str, text: &str| {
        ozd().args(["--check", "-c"]).arg(flags_file(name, text)).output().expect("it runs")
    };
    let text = format!("{head}--root {}\n", dir.join("no-such").display());
    let out = check("roots-missing.ozdrc", &text);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("no-such"), "naming it: {}", said(&out));
    let out = check("roots-slash.ozdrc", &format!("{head}--root /\n"));
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let text = format!(
        "{head}--root {}\n--root tree={},ro\n",
        dir.join("base").display(),
        dir.join("mount").display()
    );
    let out = check("roots-covered.ozdrc", &text);
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("tree"), "the covered directory warned of: {}", said(&out));
}

/// **A run removes stale FILE temporaries, and `--check` does not**: a
/// daemon killed mid-write leaves one, and a check changes nothing
/// (`docs/design.md` §6). The removal is logged before the daemon says where it
/// listens, and nothing else in the root is touched.
#[test]
fn a_run_removes_stale_temporaries_and_check_does_not() {
    if support::running_as_root() {
        return;
    }
    let dir = support::scratch().join("stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join(ozd::roots::temporary_name(LM1));
    std::fs::write(&stale, b"half a file").unwrap();
    let kept = dir.join("kept.lisp");
    std::fs::write(&kept, b"a file").unwrap();
    let text = format!(
        "--address {OZ:o}\n--name MIT-OZ,OZ\n--listen 127.0.0.1:0\n--root {}\n",
        dir.display()
    );
    let path = flags_file("temporaries.ozdrc", &text);
    let out = ozd().args(["--check", "-c"]).arg(&path).output().expect("it runs");
    assert!(out.status.success(), "{}", said(&out));
    assert!(stale.exists(), "--check changes nothing");
    let mut child = ozd()
        .arg("-c")
        .arg(&path)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("it starts");
    let stderr = child.stderr.take().expect("its stderr");
    let _running = Running(child);
    let mut said_removed = false;
    for line in BufReader::new(stderr).lines() {
        let line = line.expect("a line");
        if line.contains("listening at") {
            break;
        }
        said_removed |= line.contains("removed a stale temporary");
    }
    assert!(said_removed, "the removal is logged");
    assert!(!stale.exists(), "the stale temporary is gone");
    assert!(kept.exists(), "and nothing else");
}

/// **A second daemon on the first's endpoint touches nothing**: it binds
/// before it removes a stale temporary, so it stops at the bind, and the
/// temporary of a write the first is making stays where it is
/// (`docs/design.md` §6).
#[test]
fn a_second_daemon_on_the_same_endpoint_removes_nothing() {
    if support::running_as_root() {
        return;
    }
    let root = support::own_root();
    let text = format!(
        "--address {OZ:o}\n--name MIT-OZ,OZ\n--listen 127.0.0.1:0\n--root {}\n",
        root.display()
    );
    let first = support::daemon(&text);
    let writing = root.join(ozd::roots::temporary_name(LM1));
    std::fs::write(&writing, b"a write in progress").unwrap();
    let second = text.replace("127.0.0.1:0", &first.at().to_string());
    let out = ozd().arg("-c").arg(flags_file("second.ozdrc", &second)).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("--listen"), "refused at the bind: {}", said(&out));
    assert!(writing.exists(), "the first daemon's temporary is where it was");
}

/// **A connection is logged**, by the daemon run from its command line and
/// a file of flags: a test host opens NAME, and the log says `NAME from
/// 3050 (MIT-LISPM-1) opened`, the NCP's line (`docs/design.md` §10) with the
/// host named by the site's `--host` and the log's UTC time before it. The
/// daemon turns on its own clock and the test host on the test's, until the
/// line comes or five seconds pass.
#[test]
fn a_connection_is_logged() {
    let path = flags_file("logged.ozdrc", &site("--host 3050,MIT-LISPM-1,LM1\n"));
    let mut child = ozd()
        .arg("-c")
        .arg(&path)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("it starts");
    let stderr = child.stderr.take().expect("its stderr");
    let _running = Running(child);
    let (lines, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    // Its startup may be logged first; the line that matters says where.
    let first = loop {
        let line = heard.recv_timeout(Duration::from_secs(5)).expect("a line of its startup");
        if support::running_as_root() || line.contains("listening at ") {
            break line;
        }
    };
    if support::running_as_root() {
        assert!(first.contains("root"), "as root: {first}");
        return;
    }
    let at: SocketAddr = first
        .split("listening at ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|a| a.parse().ok())
        .expect("where it listens");
    let mut lm1 = TestHost::new(LM1, at);
    lm1.ncp.connect(0, OZ, "NAME", Box::new(Recorder::default()));
    let start = Instant::now();
    let mut seen = Vec::new();
    while start.elapsed() < Duration::from_secs(5) {
        lm1.turn(start.elapsed().as_nanos() as u64);
        seen.extend(heard.try_iter());
        if seen.iter().any(|l| l.contains("NAME from 3050 (MIT-LISPM-1) opened")) {
            return;
        }
    }
    panic!("no line for the connection: {seen:?}");
}

/// **FILE is served, from the roots the startup checked** (`docs/design.md`
/// §6): opened through the daemon as a band opens it, it answers OPN and
/// waits for a command. What it serves is `tests/file.rs`'s business.
#[test]
fn file_is_served_from_the_roots() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let file = Recorder::default();
    lm1.ncp.connect(0, OZ, "FILE", Box::new(file.clone()));
    settle(&mut d, &mut [&mut lm1], 0);
    let events = file.events();
    assert_eq!(events.first().map(String::as_str), Some("opened"), "FILE opens: {events:?}");
}

/// **`-h` and `--help` print the help on stdout, and exit 0**: the
/// usage, what ozd is, each flag, the file of flags and where
/// it is looked for, and the default endpoint --- wherever the flag is on
/// the command line and whatever else is there, even a flag that is not
/// one or a file of flags that is not there, and before anything else is
/// looked at, so as root too.
#[test]
fn help_is_printed_on_stdout_and_exits_0() {
    let lines: [&[&str]; 6] = [
        &["--help"],
        &["-h"],
        &["--check", "--help"],
        &["--help", "site.conf"],
        &["--frobnicate", "--help"],
        &["-c", "/no/such.ozdrc", "-h"],
    ];
    for args in lines {
        let out = ozd().args(args).output().expect("it runs");
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", said(&out));
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.starts_with(USAGE), "{args:?}: {text}");
        for word in [
            "--address",
            "--name",
            "--listen",
            "--root",
            "--host",
            "--peer",
            "--tcp",
            "--log-mini",
            "--log-tcp",
            "--trace",
            "--check",
            "-c, --config",
            "-h, --help",
            ".ozdrc",
            "OZD_RC",
            "127.0.0.1:42042",
            ",ro",
        ] {
            assert!(text.contains(word), "{args:?}: {word} in {text}");
        }
        assert!(out.stderr.is_empty(), "{args:?}: nothing on stderr: {}", said(&out));
    }
}
