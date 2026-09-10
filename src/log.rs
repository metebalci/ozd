// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The log: one line an event on stderr, with its time in UTC
//! (`DESIGN.md` §10) --- startup, its checks and warnings, errors, and the
//! rest §10 lists as they are written.
//!
//! What `--trace` prints is not the log: every packet, every packet passed
//! on and every drop, as muir's `--chaos-trace` prints them, each at the
//! daemon's clock rather than the calendar's (`crate::chudp`,
//! `crate::ncp`).
//!
//! Stderr and nowhere else: under systemd that is the journal, under
//! launchd `StandardErrorPath`, and by hand the terminal. A line is made
//! whole before it is written, so it goes out in one piece; one that
//! cannot be written is lost rather than taking the daemon with it.
//!
//! [`civil`] is the one calendar function: FILE's dates are to be made
//! with it too when FILE comes across (`DESIGN.md` §12), rather than with
//! a copy of its own.

use std::fmt;
use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// One event, on a line of its own: the time now, in UTC, and `what`.
/// Where a module hands its lines for the log: the daemon gives each one
/// that logs a closure calling [`event`], and a test gives one that keeps
/// them. An `Arc`, since FILE shares one among every control connection's
/// session; `Send` and `Sync`, as a session is `Send`.
pub type Hook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

pub fn event(what: impl fmt::Display) {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format!("{} {what}\n", stamp(unix));
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// `secs` since 1970 as a log line's time: ISO 8601, in UTC, to the
/// second --- `2026-09-10T14:41:00Z`.
pub fn stamp(secs: u64) -> String {
    let (y, m, d, hh, mm, ss) = civil(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Calendar date and time from seconds since 1970, UTC: Howard Hinnant's
/// days-to-civil.
///
/// From muir's `src/chaos/file.rs`, `civil`, verbatim: year, month, day,
/// hour, minute, second.
pub fn civil(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m as u64, d as u64, rem / 3600, rem % 3600 / 60, rem % 60)
}
