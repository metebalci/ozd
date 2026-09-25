// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FILE's dates as a band prints them (`src/timezone.rs`): a zone, the
//! band's `:TIMEZONE`, and the band's own daylight-saving rule on top of it,
//! whatever the zone. The instants are written as Unix times, each with the
//! UTC it is beside it.

use ozd::timezone::{self, Fields};

/// 2026-01-15 12:00:00 UTC, winter.
const WINTER: i64 = 1_768_478_400;
/// 2026-07-15 12:00:00 UTC, summer.
const SUMMER: i64 = 1_784_116_800;

/// 2100-01-01 00:00:00 UTC, the end of the range FILE's dates are read in.
const END: i64 = 4_102_444_800;

/// What the band prints for `t` at `zone`: its decode, the transcription in
/// `src/timezone.rs`, as `PRINT-DIRECTORY-DATE-PROPERTY` writes it, with
/// the year in four digits (`sys/io/file/open.lisp:1458-1462`).
fn band_print(t: i64, zone: i8) -> String {
    let f = timezone::decode(t, zone);
    format!(
        "{:02}/{:02}/{:04} {:02}:{:02}:{:02}",
        f.month, f.day, f.year, f.hour, f.minute, f.second
    )
}

/// [`band_print`], with the year cut to two digits, to compare with what
/// ozd prints.
fn band_print_short(t: i64, zone: i8) -> String {
    let long = band_print(t, zone);
    format!("{}{}", &long[..6], &long[8..])
}

/// What the band reads `text` as at `zone`, its clock in `current_year`:
/// the fields as its fast parser takes them, two digits each, no range
/// checked (`PARSE-DIRECTORY-DATE-PROPERTY`, `sys/io/file/open.lisp:1425`),
/// and then its encode, the transcription in `src/timezone.rs`.
fn band_parse(text: &str, zone: i8, current_year: i64) -> i64 {
    let y = if text.len() == 19 { 4 } else { 2 };
    let n = |from: usize, len: usize| text[from..from + len].parse::<i64>().unwrap();
    let f = Fields {
        month: n(0, 2),
        day: n(3, 2),
        year: n(6, y),
        hour: n(7 + y, 2),
        minute: n(10 + y, 2),
        second: n(13 + y, 2),
    };
    timezone::encode(f, zone, current_year)
}

/// Days since 1970 of a date: Howard Hinnant's days-from-civil.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The band's last Sunday in October for a year as its caller gives it
/// (`LAST-SUNDAY-IN-OCTOBER`, `sys/io1/time.lisp:191-196`).
fn last_sunday_in_october(year: i64) -> i64 {
    let lsa = timezone::last_sunday_in_april(year);
    if lsa <= 25 { lsa + 6 } else { lsa - 1 }
}

/// 00:00 standard time at `zone` on the last Sunday in October of `year`,
/// that Sunday as the band's decode finds it, given the full year.
fn october_midnight(year: i64, zone: i8) -> i64 {
    days_from_civil(year, 10, last_sunday_in_october(year)) * 86_400 + i64::from(zone) * 3600
}

/// The zones the round trips are run at: UTC, System 100's, and one each
/// side further off.
const ZONES: [i8; 4] = [0, 5, -3, 8];

/// **One instant, three ways**: at zone 0 it is UTC, with the band's
/// daylight saving on top in summer; at zone 5, System 100's
/// (`sys/site/site.lisp:99`), five hours west in winter and four in summer;
/// east of Greenwich the same rule moves it the other way.
#[test]
fn one_instant_is_printed_in_the_bands_zone_and_its_daylight_saving() {
    assert_eq!(timezone::print(WINTER, 0), "01/15/26 12:00:00");
    assert_eq!(timezone::print(SUMMER, 0), "07/15/26 13:00:00");
    assert_eq!(timezone::print(WINTER, 5), "01/15/26 07:00:00");
    assert_eq!(timezone::print(SUMMER, 5), "07/15/26 08:00:00");
    assert_eq!(timezone::print(WINTER, -1), "01/15/26 13:00:00");
    assert_eq!(timezone::print(SUMMER, -1), "07/15/26 14:00:00");
    assert_eq!(timezone::print(SUMMER, 12), "07/15/26 01:00:00");
    assert_eq!(timezone::print(SUMMER, -12), "07/16/26 01:00:00");
}

/// **The band's print: its daylight saving begins at 02:00 standard time on the last
/// Sunday in April and ends at 01:00 standard time on the last Sunday in
/// October** (`DAYLIGHT-SAVINGS-TIME-IN-NORTH-AMERICA-P`,
/// `sys/io1/time.lisp:174`), judged on the standard-time fields, at any
/// zone. In 2026 those Sundays are 04/26 and 10/25. In 2027 the last Sunday
/// in April is the 25th, so October's is not the 24th but the 31st
/// (`LAST-SUNDAY-IN-OCTOBER`, `:191`).
#[test]
fn daylight_saving_turns_where_the_bands_rule_turns() {
    // Zone 5: 07:00 UTC is 02:00 standard, and the clock goes to 03:00.
    assert_eq!(band_print_short(1_777_186_799, 5), "04/26/26 01:59:59");
    assert_eq!(band_print_short(1_777_186_800, 5), "04/26/26 03:00:00");
    // 06:00 UTC is 01:00 standard, and the clock goes back to it.
    assert_eq!(band_print_short(1_792_907_999, 5), "10/25/26 01:59:59");
    assert_eq!(band_print_short(1_792_908_000, 5), "10/25/26 01:00:00");
    // Zone 0, the same rule at the same standard hours.
    assert_eq!(band_print_short(1_777_168_799, 0), "04/26/26 01:59:59");
    assert_eq!(band_print_short(1_777_168_800, 0), "04/26/26 03:00:00");
    assert_eq!(band_print_short(1_792_889_999, 0), "10/25/26 01:59:59");
    assert_eq!(band_print_short(1_792_890_000, 0), "10/25/26 01:00:00");
    // 2027: October's last Sunday is April's plus six.
    assert_eq!(band_print_short(1_824_962_399, 5), "10/31/27 01:59:59");
    assert_eq!(band_print_short(1_824_962_400, 5), "10/31/27 01:00:00");
}

/// **The band's calendar, transcribed, is the calendar**, at every day from
/// 1970 to 2099 (`DECODE-UNIVERSAL-TIME-WITHOUT-DST`, `sys/io1/time.lisp:105`,
/// which says of itself that it does not know 2100 is not a leap year).
/// Measured against `civil`, Howard Hinnant's, at noon each day.
#[test]
fn the_bands_calendar_is_the_calendar_from_1970_to_2099() {
    let end = 4_102_444_800; // 2100-01-01 00:00:00 UTC
    let mut t = 43_200;
    while t < end {
        let (y, m, d, hh, mm, ss) = ozd::log::civil(t as u64);
        let f = timezone::decode_standard(t, 0);
        assert_eq!(
            (f.year, f.month, f.day, f.hour, f.minute, f.second),
            (y as i64, m as i64, d as i64, hh as i64, mm as i64, ss as i64),
            "at {t}"
        );
        t += 86_400;
    }
}

/// **The fields a date is printed from** are the band's decoded ones, the
/// year in full; what is printed keeps two of its digits.
#[test]
fn a_date_is_decoded_to_the_bands_fields() {
    assert_eq!(
        timezone::decode(1_790_347_178, 5),
        Fields { year: 2026, month: 9, day: 25, hour: 10, minute: 39, second: 38 }
    );
    assert_eq!(timezone::print(1_790_347_178, 5), "09/25/26 10:39:38");
}

/// **The band's last Sunday in April is the calendar's**, every year from
/// 1970 to 2099 given the year in full, as the band's decode gives it ---
/// **but 2000**, when it is the 24th, a Monday, and not the 30th
/// (`LAST-SUNDAY-IN-APRIL`, `sys/io1/time.lisp:198`, ITS's `GDWOBY`
/// transcribed). The year is made 100, and `LEAP-YEAR-P` takes 100 as it
/// is (`:376`), which is not a leap year, so February 29th is missed. A
/// band's own quirk, kept. Measured against the calendar's own Sundays:
/// 1970-01-01 was a Thursday.
#[test]
fn the_bands_last_sunday_in_april_is_the_calendars() {
    let mut last = std::collections::BTreeMap::new();
    for days in 0..47_482_i64 {
        let (y, m, d, ..) = ozd::log::civil(days as u64 * 86_400);
        if m == 4 && (days + 4) % 7 == 0 {
            last.insert(y as i64, d as i64);
        }
    }
    assert_eq!(last.len(), 130, "1970 to 2099");
    assert_eq!(last[&2000], 30);
    for (year, day) in last {
        let band = if year == 2000 { 24 } else { day };
        assert_eq!(timezone::last_sunday_in_april(year), band, "{year}");
    }
}

/// **A date is read as the band reads one** (`ENCODE-UNIVERSAL-TIME`,
/// `sys/io1/time.lisp:150-170`): the zone, and the band's daylight saving
/// on top. The band writes the year in four digits (`~2,'0D` over the
/// decoded year, `sys/io/file/open.lisp:1461`), and ozd in two; both are
/// read.
#[test]
fn a_date_is_read_in_the_bands_zone_with_its_daylight_saving() {
    // 2026-09-25 18:39:38 UTC: 14:39:38 at zone 5, in daylight saving.
    assert_eq!(timezone::parse("09/25/2026 14:39:38", 5, SUMMER), Some(1_790_361_578));
    assert_eq!(timezone::parse("09/25/26 14:39:38", 5, SUMMER), Some(1_790_361_578));
    // Winter, and zone 0.
    assert_eq!(timezone::parse("01/15/26 07:00:00", 5, SUMMER), Some(WINTER));
    assert_eq!(timezone::parse("01/15/2026 12:00:00", 0, SUMMER), Some(WINTER));
    assert_eq!(timezone::parse("07/15/26 13:00:00", 0, SUMMER), Some(SUMMER));
    assert_eq!(timezone::parse("07/15/26 14:00:00", -1, SUMMER), Some(SUMMER));
}

/// **A two-digit year is the one within 50 years of now**, as the band's
/// encode takes one (`sys/io1/time.lisp:154-160`): in 2026, `76` is 1976
/// and `75` is 2075.
#[test]
fn a_two_digit_year_is_within_fifty_years_of_now() {
    assert_eq!(timezone::parse("01/01/76 00:00:00", 0, SUMMER), Some(189_302_400));
    assert_eq!(timezone::parse("01/01/75 00:00:00", 0, SUMMER), Some(3_313_526_400));
    assert_eq!(timezone::parse("01/01/1976 00:00:00", 0, SUMMER), Some(189_302_400));
}

/// **Every instant ozd prints, the band reads as that instant**, every
/// hour from 1970 to 2099, at four zones: ozd's print is the inverse of the
/// band's parse, its encode (`sys/io1/time.lisp:150-170`), and not a copy
/// of the band's print. The band's own print and parse disagree after
/// 2000, its encode asking its daylight-saving rule about the year less
/// 1900 (`:162-165`), which the rule takes for another year (`:199-200`); and
/// in 2000, which its encode takes for no leap year (`:376`).
///
/// **The one hour a year with no answer**: the band's parse reads nothing
/// as the hour from 00:00 standard time on its last Sunday in October,
/// where its clock goes back, by its own window --- in 2000 the hour a day
/// before, as it reads every date after February that year. There ozd
/// prints the standard time the band reads nearest, an hour early. Listed,
/// and nothing else fails.
#[test]
fn every_instant_ozd_prints_the_band_reads_as_that_instant() {
    for zone in ZONES {
        let mut failed = Vec::new();
        let mut t = 0;
        while t < END {
            let shown = timezone::print(t, zone);
            let year = timezone::decode(t, zone).year;
            if band_parse(&shown, zone, year) != t {
                failed.push(t);
            }
            t += 3600;
        }
        // The hour before the instant the band's encode reads 01:00 on its
        // own last Sunday in October as: in 2000 a day early, like every
        // date it reads after February that year.
        let expected: Vec<i64> = (1970..=2099)
            .map(|year| {
                let sunday = last_sunday_in_october(year - 1900);
                band_parse(&format!("10/{sunday:02}/{year} 01:00:00"), zone, year) - 3600
            })
            .filter(|&t| (0..END).contains(&t))
            .collect();
        assert_eq!(failed, expected, "zone {zone}");
        assert_eq!(expected.len(), 130, "zone {zone}: one a year");
        for &t in &expected {
            let shown = timezone::print(t, zone);
            // Standard time; in 2000 the next day's, which the band reads
            // a day early.
            let day = if timezone::decode_standard(t, zone).year == 2000 { 86_400 } else { 0 };
            let f = timezone::decode_standard(t + day, zone);
            let standard = format!(
                "{:02}/{:02}/{:02} {:02}:{:02}:{:02}",
                f.month,
                f.day,
                f.year % 100,
                f.hour,
                f.minute,
                f.second
            );
            assert_eq!(shown, standard, "zone {zone}, at {t}: standard time");
            let year = timezone::decode(t, zone).year;
            assert_eq!(band_parse(&shown, zone, year), t - 3600, "zone {zone}, at {t}");
        }
    }
}

/// **Every instant the band prints, ozd reads as that instant**, every
/// hour from 1970 to 2099, at four zones: ozd's parse is the inverse of the
/// band's print, its decode (`sys/io1/time.lisp:82-103`), and not a copy of
/// the band's parse.
///
/// **The one hour a year with two answers**: the band prints the hour
/// after its clock goes back, 01:00 standard time on its last Sunday in
/// October by its own window, as it printed the hour before. ozd reads
/// that string as the first, the daylight-saving one; the second is
/// listed, and nothing else fails.
#[test]
fn every_instant_the_band_prints_ozd_reads_as_that_instant() {
    for zone in ZONES {
        let mut failed = Vec::new();
        let mut t = 0;
        while t < END {
            let shown = band_print(t, zone);
            if timezone::parse(&shown, zone, t) != Some(t) {
                failed.push(t);
            }
            t += 3600;
        }
        let expected: Vec<i64> = (1970..=2099)
            .map(|year| october_midnight(year, zone) + 3600)
            .filter(|&t| (0..END).contains(&t))
            .collect();
        assert_eq!(failed, expected, "zone {zone}");
        assert_eq!(expected.len(), 130, "zone {zone}: one a year");
        for &t in &expected {
            assert_eq!(
                timezone::parse(&band_print(t, zone), zone, t),
                Some(t - 3600),
                "{zone} {t}"
            );
        }
    }
}

/// **Where the band's print and parse disagree, ozd means what each end
/// means**, each case pinned:
///
/// - **2026, between the two windows.** At 2026-04-27 12:00 UTC the band's
///   print is in daylight saving, `04/27/2026 08:00:00`, and its parse of
///   that is in standard time, an hour later. ozd prints `07:00`, which
///   the band reads as the instant, and reads the band's `08:00` as the
///   instant. In October the other way about.
/// - **2000.** The band's parse puts March 1st 2000 on February 29th, so
///   ozd prints the next day's date for it, and for December 31st, which
///   has no next date in its month the band reads rightly, day 32, which
///   the band's parser takes as it takes any two digits. ozd reads the
///   band's own print, which is right, as it is.
/// - **The hour the band's clock skips.** 02:30 on 1995-04-30 is printed by
///   no band; ozd reads it as standard time, the instant the band shows as
///   03:30.
/// - **The hour its parse has no string for**, 00:30 standard time on
///   2026-10-27, the encode's last Sunday: ozd prints the standard time,
///   and the band reads it an hour early.
/// - **The hour its print repeats**, 01:30 on 2026-10-25, the decode's last
///   Sunday: ozd reads it as the first of the two, in daylight saving.
#[test]
fn where_the_bands_print_and_parse_disagree_ozd_means_what_each_end_means() {
    let april = 1_777_291_200; // 2026-04-27 12:00:00 UTC
    assert_eq!(band_print(april, 5), "04/27/2026 08:00:00");
    assert_eq!(band_parse("04/27/2026 08:00:00", 5, 2026), april + 3600, "the band's own");
    assert_eq!(timezone::print(april, 5), "04/27/26 07:00:00");
    assert_eq!(band_parse("04/27/26 07:00:00", 5, 2026), april);
    assert_eq!(timezone::parse("04/27/2026 08:00:00", 5, april), Some(april));
    let october = 1_793_016_000; // 2026-10-26 12:00:00 UTC
    assert_eq!(band_print(october, 5), "10/26/2026 07:00:00");
    assert_eq!(band_parse("10/26/2026 07:00:00", 5, 2026), october - 3600, "the band's own");
    assert_eq!(timezone::print(october, 5), "10/26/26 08:00:00");
    assert_eq!(band_parse("10/26/26 08:00:00", 5, 2026), october);
    assert_eq!(timezone::parse("10/26/2026 07:00:00", 5, october), Some(october));

    let march = 951_912_000; // 2000-03-01 12:00:00 UTC
    assert_eq!(band_parse("03/01/2000 12:00:00", 0, 2000), march - 86_400, "the band's own");
    assert_eq!(timezone::print(march, 0), "03/02/00 12:00:00");
    assert_eq!(timezone::parse("03/01/2000 12:00:00", 0, march), Some(march));
    let december = 978_264_000; // 2000-12-31 12:00:00 UTC
    assert_eq!(timezone::print(december, 0), "12/32/00 12:00:00");
    assert_eq!(band_parse("12/32/00 12:00:00", 0, 2000), december);
    assert_eq!(timezone::parse(&band_print(december, 0), 0, december), Some(december));

    assert_eq!(timezone::parse("04/30/95 02:30:00", 5, SUMMER), Some(799_227_000));
    assert_eq!(band_print_short(799_227_000, 5), "04/30/95 03:30:00");

    let gap = 1_793_079_000; // 2026-10-27 05:30:00 UTC, 00:30 standard at zone 5
    assert_eq!(timezone::print(gap, 5), "10/27/26 00:30:00");
    assert_eq!(band_parse("10/27/26 00:30:00", 5, 2026), gap - 3600);

    let first = 1_792_906_200; // 2026-10-25 05:30:00 UTC
    let second = first + 3600;
    assert_eq!(band_print(first, 5), band_print(second, 5));
    assert_eq!(timezone::parse("10/25/2026 01:30:00", 5, first), Some(first));
}

/// **What is not a date as the band prints one is not read**: the shape
/// is `MM/DD/YY HH:MM:SS` or `MM/DD/YYYY HH:MM:SS`, every field in range
/// for the calendar, the year from 1970 to 2099 --- the band's own decode
/// knows no later one (`sys/io1/time.lisp:120`) --- and the time not before
/// 1970.
#[test]
fn what_is_not_a_date_is_not_read() {
    for bad in [
        "",
        "09/25/2026",
        "9/25/2026 14:39:38",
        "09/25/2026 14:39",
        "09-25-2026 14:39:38",
        "09/25/2026T14:39:38",
        "09/25/2026 14:39:38 ",
        "0a/25/26 14:39:38",
        "+9/25/26 14:39:38",
        "00/25/26 14:39:38",
        "13/25/26 14:39:38",
        "02/30/26 14:39:38",
        "02/29/2027 00:00:00",
        "09/00/26 14:39:38",
        "09/25/26 24:00:00",
        "09/25/26 14:60:00",
        "09/25/26 14:39:60",
        "01/01/2100 00:00:00",
        "12/31/1969 23:00:00",
        "09/25/12026 14:39:38",
    ] {
        assert_eq!(timezone::parse(bad, 0, SUMMER), None, "{bad:?}");
    }
    assert_eq!(timezone::parse("01/01/1970 00:00:00", 0, SUMMER), Some(0), "1970 itself");
    assert_eq!(timezone::parse("01/01/1970 05:00:00", 5, SUMMER), Some(36_000), "zone 5");
    assert_eq!(timezone::parse("01/01/1970 00:00:00", -1, SUMMER), None, "before 1970");
}
