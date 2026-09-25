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

/// **The band's daylight saving begins at 02:00 standard time on the last
/// Sunday in April and ends at 01:00 standard time on the last Sunday in
/// October** (`DAYLIGHT-SAVINGS-TIME-IN-NORTH-AMERICA-P`,
/// `sys/io1/time.lisp:172`), judged on the standard-time fields, at any
/// zone. In 2026 those Sundays are 04/26 and 10/25. In 2027 the last Sunday
/// in April is the 25th, so October's is not the 24th but the 31st
/// (`LAST-SUNDAY-IN-OCTOBER`, `:188`).
#[test]
fn daylight_saving_turns_where_the_bands_rule_turns() {
    // Zone 5: 07:00 UTC is 02:00 standard, and the clock goes to 03:00.
    assert_eq!(timezone::print(1_777_186_799, 5), "04/26/26 01:59:59");
    assert_eq!(timezone::print(1_777_186_800, 5), "04/26/26 03:00:00");
    // 06:00 UTC is 01:00 standard, and the clock goes back to it.
    assert_eq!(timezone::print(1_792_907_999, 5), "10/25/26 01:59:59");
    assert_eq!(timezone::print(1_792_908_000, 5), "10/25/26 01:00:00");
    // Zone 0, the same rule at the same standard hours.
    assert_eq!(timezone::print(1_777_168_799, 0), "04/26/26 01:59:59");
    assert_eq!(timezone::print(1_777_168_800, 0), "04/26/26 03:00:00");
    assert_eq!(timezone::print(1_792_889_999, 0), "10/25/26 01:59:59");
    assert_eq!(timezone::print(1_792_890_000, 0), "10/25/26 01:00:00");
    // 2027: October's last Sunday is April's plus six.
    assert_eq!(timezone::print(1_824_962_399, 5), "10/31/27 01:59:59");
    assert_eq!(timezone::print(1_824_962_400, 5), "10/31/27 01:00:00");
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
/// (`LAST-SUNDAY-IN-APRIL`, `sys/io1/time.lisp:195`, ITS's `GDWOBY`
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
/// encode takes one (`sys/io1/time.lisp:153-160`): in 2026, `76` is 1976
/// and `75` is 2075.
#[test]
fn a_two_digit_year_is_within_fifty_years_of_now() {
    assert_eq!(timezone::parse("01/01/76 00:00:00", 0, SUMMER), Some(189_302_400));
    assert_eq!(timezone::parse("01/01/75 00:00:00", 0, SUMMER), Some(3_313_526_400));
    assert_eq!(timezone::parse("01/01/1976 00:00:00", 0, SUMMER), Some(189_302_400));
}

/// **What is printed is read back as the same time**, wherever the band's
/// own decode and encode agree: winter and summer, at zone 0, 5 and -1, in
/// 1995 and in 2026, the year in two digits and in four.
#[test]
fn a_date_printed_is_read_back() {
    for t in [WINTER, SUMMER, 804_873_600, 1_790_361_578, 189_302_400] {
        for zone in [0, 5, -1, 8, -12, 12] {
            let shown = timezone::print(t, zone);
            assert_eq!(timezone::parse(&shown, zone, t), Some(t), "{shown} at {zone}");
            let f = timezone::decode(t, zone);
            let long = format!("{}{:04}{}", &shown[..6], f.year, &shown[8..]);
            assert_eq!(timezone::parse(&long, zone, t), Some(t), "{long} at {zone}");
        }
    }
}

/// **The band's quirks are read as the band reads them**, since a band
/// reads ozd's dates with them:
///
/// - **The spring-forward hour.** 02:30 on 1995-04-30, the last Sunday in
///   April, is a time the band's clock skips. The band reads it as
///   daylight saving, 06:30 UTC at zone 5, and prints that back as 01:30.
/// - **After 2000 the band's encode takes its last Sundays from another
///   year.** It makes the year one since 1900 before it asks the rule
///   (`sys/io1/time.lisp:161-165`), and the rule takes a year above 100 as
///   a full one and subtracts 1900 again (`:196`). So in 2026 it reads
///   daylight saving from 04/28 to 10/27, where its decode, given the full
///   year, prints it from 04/26 to 10/25: a date printed between the two
///   is read back an hour off, by the band and by ozd alike.
/// - **2000 is no leap year to the encode**: the year there is 100, which
///   `LEAP-YEAR-P` takes as it is (`:376`), so a date after February reads a
///   day early.
#[test]
fn the_bands_quirks_are_read_as_the_band_reads_them() {
    assert_eq!(timezone::parse("04/30/95 02:30:00", 5, SUMMER), Some(799_223_400));
    assert_eq!(timezone::print(799_223_400, 5), "04/30/95 01:30:00");
    assert_eq!(timezone::parse("04/30/95 01:30:00", 5, SUMMER), Some(799_223_400));

    let april = 1_777_291_200; // 2026-04-27 12:00:00 UTC
    assert_eq!(timezone::print(april, 5), "04/27/26 08:00:00");
    assert_eq!(timezone::parse("04/27/26 08:00:00", 5, april), Some(april + 3600));
    let october = 1_793_016_000; // 2026-10-26 12:00:00 UTC
    assert_eq!(timezone::print(october, 5), "10/26/26 07:00:00");
    assert_eq!(timezone::parse("10/26/26 07:00:00", 5, october), Some(october - 3600));

    // 2000-02-29 12:00:00 UTC, which the band's encode gives for March 1st.
    assert_eq!(timezone::parse("03/01/2000 12:00:00", 0, SUMMER), Some(951_825_600));
    assert_eq!(timezone::print(951_825_600, 0), "02/29/00 12:00:00");
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
