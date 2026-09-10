// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The TIME and UPTIME services, AIM-628 §5.8 and the Lisp Machine
//! manual's Chaosnet chapter.
//!
//! "An RFC to contact name TIME evokes an ANS containing the number of
//! seconds since midnight Greenwich Mean Time, Jan 1, 1900 as a 32-bit
//! number in four 8-bit bytes, least-significant byte first. Some
//! computers --- Lisp machines, for example --- which don't have hardware
//! calendar-clocks use this protocol to find out the date and time when
//! they first come up." The Lisp Machine's `HOST-TIME` in
//! `sys/network/chaos/chuse.lisp` asks each of its time-server hosts in
//! turn and `DECODE-CANONICAL-TIME-PACKET` reads the two words back, low
//! word first.
//!
//! UPTIME "is similar to the TIME protocol, except that the contact name
//! is UPTIME, and the time returned is actually an interval (in seconds)
//! describing how long the host has been up." **The unit is wrong.** The
//! machine's own `UPTIME-SERVER` sends `(* 60. (- (TIME:GET-UNIVERSAL-TIME)
//! TIME:*UT-AT-BOOT-TIME*))`; both its user ends, `HOST-UPTIME` and
//! `UPTIME`, divide what comes back by 60 before printing it; and
//! `DECODE-CANONICAL-TIME-PACKET`'s own documentation says "an integral
//! number of 60ths of a second" --- all in `sys/network/chaos/chsaux.lisp`.
//! Where the manual and the machine's code disagree, the code is what a
//! band does, so [`Uptime`] answers sixtieths (`PROTOCOLS.md`, UPTIME).
//! muir's answers seconds, and a band asking it prints a sixtieth of the
//! real uptime.

use crate::ncp::{Response, Service};

/// Seconds from 1 January 1900 to 1 January 1970: the universal time of
/// the Unix epoch.
pub const UNIX_EPOCH_UNIVERSAL: u64 = 2_208_988_800;

/// Where the time comes from: the machine's clock, or a fixed value for
/// a test.
pub enum Clock {
    System,
    Fixed(u32),
}

pub struct Time {
    clock: Clock,
}

impl Time {
    pub fn new() -> Time {
        Time { clock: Clock::System }
    }

    /// A server whose answer is always this universal time.
    pub fn fixed(universal: u32) -> Time {
        Time { clock: Clock::Fixed(universal) }
    }

    /// The universal time now: seconds since 1900, as the Lisp Machine
    /// counts it.
    pub fn universal(&self) -> u32 {
        match self.clock {
            Clock::Fixed(t) => t,
            Clock::System => {
                let unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                // Four bytes on the wire: the count wraps on 7 February
                // 2036, as the band's own universal time does.
                (unix + UNIX_EPOCH_UNIVERSAL) as u32
            }
        }
    }
}

impl Default for Time {
    fn default() -> Self {
        Time::new()
    }
}

impl Service for Time {
    fn contact(&self) -> &str {
        "TIME"
    }
    fn request(&mut self, _now: u64, _args: &str, _from: (u16, u16)) -> Response {
        Response::Answer(self.universal().to_le_bytes().to_vec())
    }
}

/// UPTIME: sixtieths of a second since the host came up, by the NCP's
/// clock --- the `now` it is handed, in nanoseconds (`DESIGN.md` §4).
pub struct Uptime {
    since: u64,
}

impl Uptime {
    pub fn new(since: u64) -> Uptime {
        Uptime { since }
    }
}

impl Service for Uptime {
    fn contact(&self) -> &str {
        "UPTIME"
    }
    fn request(&mut self, now: u64, _args: &str, _from: (u16, u16)) -> Response {
        // (now - since) x 60 / 10^9, `DESIGN.md` §7's, in 128 bits so that
        // no clock a `u64` holds overflows on the way; the four bytes are
        // the low 32 bits, which wrap after about 828 days.
        let sixtieths = (now.saturating_sub(self.since) as u128 * 60 / 1_000_000_000) as u32;
        Response::Answer(sixtieths.to_le_bytes().to_vec())
    }
}
