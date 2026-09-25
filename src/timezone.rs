// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FILE's dates as a band prints them: in a **zone**, and with the band's
//! **daylight saving** on top of it (`docs/design.md` §7, "FILE's dates").
//!
//! **The zone** is `--timezone`, the band's own `:TIMEZONE` site option
//! (`sys/io1/time.lisp:13`): whole hours west of Greenwich, 5 at System
//! 100's site (`sys/site/site.lisp:99`), and 0 unless it is given. Zone 0 is
//! UTC.
//!
//! **Daylight saving is the band's, whatever the zone.** A band decodes a
//! date at its zone and then asks `*DAYLIGHT-SAVINGS-TIME-P-FUNCTION*`
//! whether daylight saving is in effect, and if it is decodes it again an
//! hour to the east (`DECODE-UNIVERSAL-TIME`, `sys/io1/time.lisp:82-103`).
//! Only a zone passed in explicitly skips that, and the date printer passes
//! none (`PRINT-DIRECTORY-DATE-PROPERTY`, `sys/io/file/open.lisp:1458`).
//! The function is a plain variable, not a site option (`time.lisp:15-16`),
//! and its value is the old North American calendar
//! (`DAYLIGHT-SAVINGS-TIME-IN-NORTH-AMERICA-P`, `:174-189`): from 02:00 on
//! the last Sunday in April to 01:00 on the last Sunday in October, each
//! judged on the standard-time fields. So a summer date at zone 0 is UTC
//! with the band's hour of daylight saving on top, which moves it an hour
//! from plain UTC; a band reads every date through that rule, and a date
//! printed without it would be an hour out to the band all summer.
//!
//! **What is transcribed, and why.** [`decode_standard`] is
//! `DECODE-UNIVERSAL-TIME-WITHOUT-DST` (`:105-138`), whose own comment says
//! it does not know that 2100 is not a leap year; transcribed rather than
//! replaced by the calendar, so that a date past 2099 prints as the band
//! would print it. From 1970 to 2099 it is the calendar, measured day by day
//! in `tests/timezone.rs`. `LAST-SUNDAY-IN-APRIL` (`:198-207`) and
//! `LEAP-YEAR-P` (`:376-382`) are transcribed too, arithmetic and all.
//!
//! **The band's print and parse disagree after 2000**, and ozd copies
//! neither: it means what each end means. The band's encode, which reads a
//! date, makes the year one since 1900 before it asks its daylight-saving
//! rule (`sys/io1/time.lisp:162-165`), and the rule takes a year above 100
//! for a full one and subtracts 1900 again (`:199-200`); so after 2000 the
//! encode reads daylight saving by another year's Sundays --- in 2026 from
//! April 28th to October 27th --- while the decode, which prints, has it
//! from April 26th to October 25th. And the encode takes 2000 for no leap
//! year, handing `LEAP-YEAR-P` 100 (`:168`, `:376`), so it puts every date
//! from March 1st 2000 a day early. So [`print()`] is the inverse of the
//! band's parse: for an instant, a string that [`encode`] reads back as
//! that instant. And [`parse`] is the inverse of the band's print: for a
//! string, the instant [`decode`] printed it for. [`decode`] and
//! [`encode`] are the band's own, transcribed, and the tests check ozd's
//! two against them at every hour from 1970 to 2099.
//!
//! **Where no inverse exists** it is the band's clock going back or
//! forward, and each case is named where it is decided: the hour the
//! encode reads nothing as, from 00:00 standard time on its last Sunday in
//! October, which [`print()`] prints as standard time and the band reads an
//! hour early; the hour the decode prints twice, from 01:00 on its last
//! Sunday in October, which [`parse`] reads as the first; and the hour the
//! decode skips in April, which no band prints and [`parse`] reads as
//! standard time.
//!
//! **What is printed** is `MM/DD/YY HH:MM:SS`, the form the band's own fast
//! parser takes (`PARSE-DIRECTORY-DATE-PROPERTY`, `open.lisp:1425-1451`),
//! with the year cut to two digits; the band's printer writes all four
//! (`~2,'0D` over the decoded year, `:1461`), which would send every date
//! ozd prints to the band's slow parser.

use crate::service::time::UNIX_EPOCH_UNIVERSAL;

/// A date's fields as the band decodes them: the year in full, the month
/// and day from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fields {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

/// The days before each month, from 1: `*CUMULATIVE-MONTH-DAYS-TABLE*`
/// (`sys/io1/time.lisp:74`).
const CUMULATIVE: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// `unix`, seconds since 1970, as the band's universal time, seconds since
/// 1900.
fn universal(unix: i64) -> i64 {
    unix + UNIX_EPOCH_UNIVERSAL as i64
}

/// The fields of `unix` at `zone`, standard time, whatever the date:
/// `DECODE-UNIVERSAL-TIME-WITHOUT-DST` (`sys/io1/time.lisp:105-138`), KLH's
/// algorithm, transcribed. Lisp's `\` is Rust's `%`, both taking the sign
/// of the dividend; `FLOOR` is `div_euclid` for a positive divisor.
pub fn decode_standard(unix: i64, zone: i8) -> Fields {
    let ut = universal(unix) - i64::from(zone) * 3600;
    let secs = ut % 86_400;
    let x = ut / 86_400;
    let (mut a, mut b) = (x.div_euclid(365), x.rem_euclid(365));
    if a != 0 {
        // `(LSH (1- A) -2)`: a logical shift, which is a floor for the
        // years since 1900 that a date after 1970 has.
        b -= (a - 1) >> 2;
        if b < 0 {
            a = a - 1 + b / 365;
            b %= 365;
            b += 365;
            if a & 3 == 0 {
                b += 1;
            }
        }
    }
    let mut c = 12;
    while b < CUMULATIVE[c] {
        c -= 1;
    }
    if a & 3 == 0 && c > 2 {
        b -= 1;
        if b < CUMULATIVE[c] {
            c -= 1;
        }
        if c == 2 {
            b += 1;
        }
    }
    b -= CUMULATIVE[c];
    Fields {
        year: 1900 + a,
        month: c as i64,
        day: b + 1,
        hour: secs.div_euclid(3600),
        minute: (secs % 3600).div_euclid(60),
        second: secs % 60,
    }
}

/// The fields of `unix` at `zone`, as the band decodes a date with no zone
/// given: standard time, and an hour to the east of it where the band's
/// daylight saving says so (`DECODE-UNIVERSAL-TIME`,
/// `sys/io1/time.lisp:82-103`).
pub fn decode(unix: i64, zone: i8) -> Fields {
    let f = decode_standard(unix, zone);
    if daylight_saving(f.hour, f.day, f.month, f.year) {
        decode_standard(unix, zone - 1)
    } else {
        f
    }
}

/// `unix` as FILE prints a date, `MM/DD/YY HH:MM:SS`: **a string the
/// band reads back as `unix`**, by its encode ([`encode`]), and not the
/// string the band would print for it, which after 2000 its own encode can
/// read as another instant (the module documentation). The fields are
/// looked for in this order, and the first the band reads as `unix` is
/// printed:
///
/// 1. standard time at `zone`, and then daylight saving, an hour to the
///    east --- what the band would read, whichever of its windows it is in;
/// 2. the next day's date at the same time, and then the same date with its
///    day one more than the month has: for 2000, which the band's encode
///    takes for no leap year and so reads a day early from March 1st.
///    December 31st 2000 is `12/32/00`, which the band's fast parser takes as
///    it takes any two digits, and reads as December 31st;
/// 3. none of them: the hour from 00:00 standard time on the last Sunday in
///    October **by the encode's own window**, which it reads no string as.
///    Of the fields above, the one the band reads nearest is printed, the
///    earlier in that order where two are as near: standard time, which the
///    band reads an hour early --- in 2000 the next day's, since the band
///    reads that date a day early too.
pub fn print(unix: i64, zone: i8) -> String {
    let f = printed(unix, zone);
    format!(
        "{:02}/{:02}/{:02} {:02}:{:02}:{:02}",
        f.month,
        f.day,
        f.year.rem_euclid(100),
        f.hour,
        f.minute,
        f.second
    )
}

/// The fields [`print()`] prints for `unix`, in the order it says: the
/// first the band reads as `unix`, or, where none is, the one it reads
/// nearest, the earlier in that order where two are as near.
fn printed(unix: i64, zone: i8) -> Fields {
    let standard = decode_standard(unix, zone);
    let daylight = decode_standard(unix, zone - 1);
    let next = |z: i8| decode_standard(unix + 86_400, z);
    let beyond = |f: Fields| Fields { day: f.day + 1, ..f };
    let candidates =
        [standard, daylight, next(zone), next(zone - 1), beyond(standard), beyond(daylight)];
    let mut best = standard;
    let mut off = i64::MAX;
    for f in candidates {
        let d = (encode(f, zone, f.year) - unix).abs();
        if d == 0 {
            return f;
        }
        if d < off {
            (best, off) = (f, d);
        }
    }
    best
}

/// Whether the band's daylight saving is in effect at these standard-time
/// fields: `DAYLIGHT-SAVINGS-TIME-IN-NORTH-AMERICA-P`
/// (`sys/io1/time.lisp:174-189`). Standard time before 02:00 on the last
/// Sunday in April, and from 01:00 on the last Sunday in October; the two
/// comparisons in the source are the Lisp Machine's `≤` and `≥`, characters
/// 034 and 035, as the comments beside them say. `year` is passed on to
/// [`last_sunday_in_april`] as the caller has it.
pub(crate) fn daylight_saving(hours: i64, day: i64, month: i64, year: i64) -> bool {
    if month < 4
        || month == 4 && {
            let lsa = last_sunday_in_april(year);
            day < lsa || day == lsa && hours < 2
        }
    {
        return false;
    }
    if month > 10
        || month == 10 && {
            let lso = last_sunday_in_october(year);
            day > lso || day == lso && hours >= 1
        }
    {
        return false;
    }
    true
}

/// `LAST-SUNDAY-IN-OCTOBER` (`sys/io1/time.lisp:191-196`): April's, less
/// one, unless that would be the 24th or earlier, when it is April's plus
/// six.
fn last_sunday_in_october(year: i64) -> i64 {
    let lsa = last_sunday_in_april(year);
    if lsa <= 25 { lsa + 6 } else { lsa - 1 }
}

/// `LAST-SUNDAY-IN-APRIL` (`sys/io1/time.lisp:198-207`), from ITS's
/// `GDWOBY`: a year above 100 is taken as a full year and made one since
/// 1900, and any other as one since 1900 already.
pub fn last_sunday_in_april(year: i64) -> i64 {
    let year = if year > 100 { year - 1900 } else { year };
    let b = (year + 1899) % 400;
    let quarter = b.div_euclid(4);
    let dow_beg_year = ((b + 1) + quarter - quarter.div_euclid(25)) % 7;
    let feb29 = i64::from(leap_year_p(year));
    let dow_april_30 = (dow_beg_year + 119 + feb29) % 7;
    30 - dow_april_30
}

/// `LEAP-YEAR-P` (`sys/io1/time.lisp:376-382`): a year below 100 is taken
/// as one since 1900, and any other as it is.
pub(crate) fn leap_year_p(year: i64) -> bool {
    let year = if year < 100 { year + 1900 } else { year };
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The universal time of these fields at `zone`, in seconds since 1970, as
/// the band encodes a date with no zone given: `ENCODE-UNIVERSAL-TIME`
/// (`sys/io1/time.lisp:150-170`), transcribed. A year below 100 is the one
/// within 50 years of `current_year`; the year is then made one since
/// 1900, and **that** is what the daylight-saving rule and `LEAP-YEAR-P`
/// are given. Two quirks follow, and are the band's, kept here, since this
/// is the reference [`print()`] is the inverse of: after 2000 the rule takes
/// a year above 100 for a full one and subtracts 1900 again (`:199-200`), so its
/// last Sundays are another year's; and 2000 is 100, which `LEAP-YEAR-P`
/// takes as it is (`:376`), so it is no leap year here.
pub fn encode(f: Fields, zone: i8, current_year: i64) -> i64 {
    let year = if f.year < 100 {
        current_year + (50 + (f.year - current_year % 100)).rem_euclid(100) - 50
    } else {
        f.year
    };
    let year = year - 1900;
    let zone = i64::from(zone);
    let zone = if daylight_saving(f.hour, f.day, f.month, year) { zone - 1 } else { zone };
    let mut days =
        (f.day - 1) + CUMULATIVE[f.month as usize] + (year - 1).div_euclid(4) + year * 365;
    if f.month > 2 && leap_year_p(year) {
        days += 1;
    }
    let ut = f.second + 60 * f.minute + 3600 * f.hour + days * 86_400 + zone * 3600;
    ut - UNIX_EPOCH_UNIVERSAL as i64
}

/// A date as a band writes one in a CHANGE-PROPERTIES, in seconds since
/// 1970: **the instant the band printed it for**, by its decode
/// ([`decode`]) at `zone`, and not what the band's own encode would read it
/// as, which after 2000 can be another instant (the module documentation).
/// `None` if it is not a date.
///
/// Of the two instants a string can mean, daylight saving at `zone` and
/// standard time, the one the band prints as that string is taken. The
/// band prints one string for two instants in the hour after its clock
/// goes back, from 01:00 standard time on its last Sunday in October **by
/// its decode's window**; the first, in daylight saving, is taken. It
/// prints no instant as a time in the hour its clock skips in April; that
/// is read as standard time, the instant the band shows an hour later.
///
/// **The band writes the year in four digits**:
/// `PRINT-DIRECTORY-DATE-PROPERTY` prints the decoded year with `~2,'0D`
/// (`sys/io/file/open.lisp:1458-1462`), and the decoded year is the full
/// one, `(+ 1900. A)` (`sys/io1/time.lisp:132`), which a width pads and
/// never cuts (`FORMAT-CTL-DECIMAL`, `sys/io/format.lisp:515`). ozd writes
/// two ([`print()`]). So both are read: `MM/DD/YYYY HH:MM:SS` and `MM/DD/YY
/// HH:MM:SS`, exactly, a two-digit year the one within 50 years of the year
/// `now` is at `zone`, as the band takes its current year.
///
/// **Stricter than the band's parser**, which computes something for any
/// digits: here every field is in range for the calendar, and the instant
/// is from 1970 to 2099 --- the band's decode knows no later year
/// (`:120`) --- though its date may be the last of 1969 or the first of
/// 2100 at a zone. The band's printer writes nothing else.
pub fn parse(text: &str, zone: i8, now: i64) -> Option<i64> {
    let b = text.as_bytes();
    let long = match b.len() {
        17 => false,
        19 => true,
        _ => return None,
    };
    let y = if long { 4 } else { 2 };
    let shape_ok =
        b[2] == b'/' && b[5] == b'/' && b[6 + y] == b' ' && b[9 + y] == b':' && b[12 + y] == b':';
    if !shape_ok {
        return None;
    }
    let number = |from: usize, len: usize| -> Option<i64> {
        let digits = &b[from..from + len];
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(digits.iter().fold(0, |n, d| n * 10 + i64::from(d - b'0')))
    };
    let mut f = Fields {
        month: number(0, 2)?,
        day: number(3, 2)?,
        year: number(6, y)?,
        hour: number(7 + y, 2)?,
        minute: number(10 + y, 2)?,
        second: number(13 + y, 2)?,
    };
    let current_year = decode(now, zone).year;
    if !long {
        // The year the band's encode will take, to check the day against.
        f.year = current_year + (50 + (f.year - current_year % 100)).rem_euclid(100) - 50;
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = f.year % 4 == 0 && (f.year % 100 != 0 || f.year % 400 == 0);
    let last_day = if f.month == 2 && leap { 29 } else { *month_days.get(f.month as usize)? };
    if !(1969..=2100).contains(&f.year)
        || !(1..=12).contains(&f.month)
        || !(1..=last_day).contains(&f.day)
        || f.hour > 23
        || f.minute > 59
        || f.second > 59
    {
        return None;
    }
    let days = days_from_civil(f.year, f.month, f.day);
    let base = days * 86_400 + f.hour * 3600 + f.minute * 60 + f.second;
    let daylight = base + (i64::from(zone) - 1) * 3600;
    let standard = base + i64::from(zone) * 3600;
    let unix = if decode(daylight, zone) == f { daylight } else { standard };
    (0..END).contains(&unix).then_some(unix)
}

/// 2100-01-01 00:00:00 UTC: FILE reads no date from it on.
const END: i64 = 4_102_444_800;

/// Days since 1970 of a date of the calendar: Howard Hinnant's
/// days-from-civil, the inverse of `crate::log::civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
