// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The daemon (`DESIGN.md` §4): its services asked over loopback by a
//! test host, at a clock the test sets; the turn that wakes when nothing
//! arrives, which is what keeps a retransmission on time; the time a log
//! line carries (§10); and the command line, run as the binary.

mod support;

use muir_ah::log;
use muir_ah::ncp::{self, op};
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{
    LM1, OZ, Recorder, SECOND, TestHost, ask, daemon, datagram, hear, meters, packet, rfc, settle,
    site,
};

/// **STATUS over loopback, its meters counting what the test sent.** The
/// link counts at the socket, into the meters it shares with STATUS
/// (`DESIGN.md` §7): two datagrams that are no packet --- one too short,
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
/// read here (`DESIGN.md` §11).
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
/// clock is nanoseconds since it started (`DESIGN.md` §4), and UPTIME
/// answers sixtieths of a second since then (§7, `PROTOCOLS.md` UPTIME):
/// nought at the start, and exactly 600 ten seconds on.
#[test]
fn uptime_is_600_at_ten_seconds_of_the_daemons_clock() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    assert_eq!(ask(&mut d, &mut lm1, 0, "UPTIME").data, 0u32.to_le_bytes(), "at the start");
    assert_eq!(ask(&mut d, &mut lm1, 10 * SECOND, "UPTIME").data, 600u32.to_le_bytes());
}

/// **A retransmission happens on a turn with no packet arriving**, which
/// is why the loop waits for a datagram only so long (`DESIGN.md` §4): the
/// NCP has no timer, and sends again what is unreceipted only when it is
/// asked for output. An RFC from this host to one that never answers goes
/// out at once; a turn short of `RETRANSMIT_NS` sends nothing; a turn at
/// it waits its wait for a datagram, has none, and sends the RFC again.
/// Nothing arrived through any of it.
#[test]
fn a_retransmission_happens_on_a_turn_with_no_packet_arriving() {
    let (lm1, lm1_at) = support::socket();
    let mut d = daemon(&site(&format!("peer {LM1:o} {lm1_at}\n")));
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
/// (`DESIGN.md` §10): the Unix epoch, a leap day in a century that has
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

/// The daemon's binary.
fn muir_ah() -> Command {
    Command::new(env!("CARGO_BIN_EXE_muir-ah"))
}

/// A config file holding `text`, in this test binary's scratch directory.
fn config_file(name: &str, text: &str) -> PathBuf {
    let path = support::scratch().join(name);
    std::fs::write(&path, text).expect("a config file");
    path
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Whether a run was refused for being root, which is all a run as root
/// can be (`DESIGN.md` §6).
fn refused_as_root(out: &Output) -> bool {
    out.status.code() == Some(1) && said(out).contains("root")
}

/// **A command line that is not one config file is refused with the
/// usage**, on stderr, with exit code 2, and nothing on stdout.
#[test]
fn a_command_line_that_is_not_one_config_is_refused_with_the_usage() {
    let lines: [&[&str]; 4] =
        [&[], &["--trace"], &["--frobnicate", "a.conf"], &["a.conf", "b.conf"]];
    for args in lines {
        let out = muir_ah().args(args).output().expect("it runs");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", said(&out));
        assert!(said(&out).contains("usage: muir-ah [--trace] [--check] <config>"), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}

/// **`--check` reads the config, binds nothing, and says what is wrong.**
/// A good config exits 0, silent, even at a `listen` whose port is taken
/// --- which a run that binds refuses, exit 1, naming it. A config refused
/// is `<path>: line N: ...` and exit 1, and so is one that is not there.
/// As root, every one of them is refused for that instead.
#[test]
fn check_reads_the_config_binds_nothing_and_says_what_is_wrong() {
    let (_taken, taken_at) = support::socket();
    let text = site("").replace("listen 127.0.0.1:0", &format!("listen {taken_at}"));
    assert!(text.contains(&format!("listen {taken_at}")));
    let good = config_file("check-good.conf", &text);
    let out = muir_ah().arg("--check").arg(&good).output().expect("it runs");
    if support::running_as_root() {
        assert!(refused_as_root(&out), "as root: {}", said(&out));
        return;
    }
    assert!(out.status.success(), "{}", said(&out));
    assert!(out.stderr.is_empty() && out.stdout.is_empty(), "silent: {}", said(&out));
    let run = muir_ah().arg(&good).output().expect("it runs");
    assert_eq!(run.status.code(), Some(1), "{}", said(&run));
    assert!(said(&run).contains(&format!("listen {taken_at}")), "{}", said(&run));
    let bad = config_file("check-bad.conf", &site("bogus\n"));
    let out = muir_ah().arg("--check").arg(&bad).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1));
    let want = format!("{}: line 5: bogus: not a directive", bad.display());
    assert!(said(&out).starts_with(&want), "{}", said(&out));
    let missing = support::scratch().join("no-such.conf");
    let out = muir_ah().arg("--check").arg(&missing).output().expect("it runs");
    assert_eq!(out.status.code(), Some(1));
    assert!(said(&out).starts_with(&format!("{}: ", missing.display())), "{}", said(&out));
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
/// whole of `main`: the config read, the socket bound at a port the
/// system picks, a first log line with its UTC time saying where, and the
/// loop turning with the real clock --- a test host at LM1 asks STATUS,
/// its own NCP sending the RFC again on its own clock should one be lost,
/// and has the answer.
#[test]
fn the_daemon_runs_from_its_command_line_and_answers_status() {
    let path = config_file("run.conf", &site(""));
    let mut child = muir_ah()
        .arg(&path)
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

/// **HOSTAB and NAME are served**, each opened through the daemon as a
/// band opens it (`DESIGN.md` §7). NAME answers its one line and an EOF;
/// HOSTAB opens and waits for a name --- what it answers is
/// `tests/hostab.rs`'s business.
#[test]
fn hostab_and_name_are_served() {
    let mut d = daemon(&site(""));
    let mut lm1 = TestHost::new(LM1, d.at());
    let name = Recorder::default();
    lm1.ncp.connect(0, OZ, "NAME", Box::new(name.clone()));
    let hostab = Recorder::default();
    lm1.ncp.connect(0, OZ, "HOSTAB", Box::new(hostab.clone()));
    settle(&mut d, &mut [&mut lm1], 0);
    let name = name.events();
    assert_eq!(name.first().map(String::as_str), Some("opened"), "NAME opens: {name:?}");
    assert!(
        name.iter().any(|e| e.starts_with("data ") && e.contains("Nobody is logged in.")),
        "its line: {name:?}"
    );
    assert!(name.iter().any(|e| e == "eof"), "and an EOF: {name:?}");
    assert_eq!(hostab.events(), ["opened"], "HOSTAB opens and waits for a name");
}

/// **The roots are checked before anything is bound**, by a run and by
/// `--check` alike (`DESIGN.md` §6): a root that is not there, or `/`, is
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
    let head = format!("address {OZ:o}\nname MIT-OZ OZ\nlisten 127.0.0.1:0\n");
    let text = format!("{head}root {}\n", dir.join("no-such").display());
    let out = muir_ah().arg("--check").arg(config_file("roots-missing.conf", &text)).output();
    let out = out.expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(said(&out).contains("no-such"), "naming it: {}", said(&out));
    let text = format!("{head}root /\n");
    let out = muir_ah().arg("--check").arg(config_file("roots-slash.conf", &text)).output();
    let out = out.expect("it runs");
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let text = format!(
        "{head}root {}\nroot tree {} readonly\n",
        dir.join("base").display(),
        dir.join("mount").display()
    );
    let out = muir_ah().arg("--check").arg(config_file("roots-covered.conf", &text)).output();
    let out = out.expect("it runs");
    assert!(out.status.success(), "{}", said(&out));
    assert!(said(&out).contains("tree"), "the covered directory warned of: {}", said(&out));
}

/// **A run removes stale FILE temporaries, and `--check` does not**: a
/// daemon killed mid-write leaves one, and a check changes nothing
/// (`DESIGN.md` §6). The removal is logged before the daemon says where it
/// listens, and nothing else in the root is touched.
#[test]
fn a_run_removes_stale_temporaries_and_check_does_not() {
    if support::running_as_root() {
        return;
    }
    let dir = support::scratch().join("stale");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join(muir_ah::roots::temporary_name(LM1));
    std::fs::write(&stale, b"half a file").unwrap();
    let kept = dir.join("kept.lisp");
    std::fs::write(&kept, b"a file").unwrap();
    let text =
        format!("address {OZ:o}\nname MIT-OZ OZ\nlisten 127.0.0.1:0\nroot {}\n", dir.display());
    let path = config_file("stale.conf", &text);
    let out = muir_ah().arg("--check").arg(&path).output().expect("it runs");
    assert!(out.status.success(), "{}", said(&out));
    assert!(stale.exists(), "--check changes nothing");
    let mut child = muir_ah()
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
        said_removed |= line.contains("stale");
    }
    assert!(said_removed, "the removal is logged");
    assert!(!stale.exists(), "the stale temporary is gone");
    assert!(kept.exists(), "and nothing else");
}

/// **A connection is logged**, by the daemon run from its command line:
/// a test host opens NAME, and the log says `NAME from 3050 opened`, the
/// NCP's line (`DESIGN.md` §10) with the log's UTC time before it. The
/// daemon turns on its own clock and the test host on the test's, until
/// the line comes or five seconds pass.
#[test]
fn a_connection_is_logged() {
    let path = config_file("logged.conf", &site(""));
    let mut child = muir_ah()
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
        if seen.iter().any(|l| l.contains("NAME from 3050 opened")) {
            return;
        }
    }
    panic!("no line for the connection: {seen:?}");
}

/// **FILE is served, from the roots the startup checked** (`DESIGN.md`
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
