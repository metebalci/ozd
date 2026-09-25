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
