// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FILE's dates as `--file-dates` chooses them (`src/timezone.rs`):
//! `utc`, the default, plain UTC calendar fields as System 1002 and later
//! put them on the wire, with no zone and no daylight saving; `mit`, System
//! 100's convention at `--timezone`, which `tests/timezone.rs` covers. The
//! instants are written as Unix times, each with the UTC it is beside it.

use ozd::timezone::FileDates;

/// 2026-01-15 12:00:00 UTC, winter.
const WINTER: i64 = 1_768_478_400;
/// 2026-07-15 12:00:00 UTC, summer.
const SUMMER: i64 = 1_784_116_800;
/// 2000-12-31 12:00:00 UTC, the last day of a leap year MIT's encode takes
/// for none.
const DECEMBER_2000: i64 = 978_264_000;
/// 2000-02-29 12:00:00 UTC.
const LEAP_DAY_2000: i64 = 951_825_600;
/// 2100-01-01 00:00:00 UTC, the end of the range FILE's dates are read in.
const END: i64 = 4_102_444_800;

/// **Under `utc` a date is its UTC fields, summer or winter, the year in
/// four digits**, so that 1970 and 2099 read back as themselves (MIT's
/// `chfile.text` gives `mm/dd/yy`; the four digits are an extension for
/// System 1002 and later). Under `mit` the same instant has the band's
/// daylight saving on top, at its zone, 13:00 at zone 0 and 14:00 at zone
/// -1, and the year stays in two digits, the only form those bands' fast
/// parser takes.
#[test]
fn utc_prints_the_plain_utc_fields() {
    assert_eq!(FileDates::Utc.print(SUMMER), "07/15/2026 12:00:00");
    assert_eq!(FileDates::Utc.print(WINTER), "01/15/2026 12:00:00");
    assert_eq!(FileDates::Mit(-1).print(SUMMER), "07/15/26 14:00:00");
    assert_eq!(FileDates::Mit(0).print(SUMMER), "07/15/26 13:00:00");
    assert_eq!(FileDates::Mit(0).print(WINTER), "01/15/26 12:00:00");
}

/// **The ends of the range, in four digits**: the first instant, 1970-01-01
/// 00:00:00, and the last, 2099-12-31 23:59:59, which in two digits were
/// `70` and `99` and read back, 50 years either side of 2026, as 2070 and
/// 1999. Each is 19 characters and reads back as itself.
#[test]
fn utc_prints_the_ends_of_the_range_in_four_digits() {
    for (t, shown) in [(0, "01/01/1970 00:00:00"), (END - 1, "12/31/2099 23:59:59")] {
        assert_eq!(ozd::timezone::print_utc(t), shown);
        assert_eq!(FileDates::Utc.print(t), shown);
        assert_eq!(shown.len(), 19);
        assert_eq!(FileDates::Utc.parse(shown, SUMMER), Some(t), "{shown}");
    }
}

/// **2000 is a leap year under `utc`**: December 31st is the 31st, where
/// `mit` prints `12/32/00` for its band's encode, and February 29th is
/// itself.
#[test]
fn utc_takes_2000_for_a_leap_year() {
    assert_eq!(FileDates::Utc.print(DECEMBER_2000), "12/31/2000 12:00:00");
    assert_eq!(FileDates::Mit(0).print(DECEMBER_2000), "12/32/00 12:00:00");
    assert_eq!(FileDates::Utc.print(LEAP_DAY_2000), "02/29/2000 12:00:00");
    assert_eq!(FileDates::Utc.parse("02/29/2000 12:00:00", SUMMER), Some(LEAP_DAY_2000));
    assert_eq!(FileDates::Utc.parse("12/31/00 12:00:00", SUMMER), Some(DECEMBER_2000));
    assert_eq!(FileDates::Utc.parse("12/32/00 12:00:00", SUMMER), None, "no 32nd");
}

/// **Under `utc` a date is read as its UTC fields**, in the two forms ozd
/// reads: the year in four digits, as the band writes it, or in two.
#[test]
fn utc_reads_the_plain_utc_fields() {
    assert_eq!(FileDates::Utc.parse("07/15/2026 12:00:00", SUMMER), Some(SUMMER));
    assert_eq!(FileDates::Utc.parse("07/15/26 12:00:00", SUMMER), Some(SUMMER));
    assert_eq!(FileDates::Utc.parse("01/15/2026 12:00:00", SUMMER), Some(WINTER));
    assert_eq!(FileDates::Mit(0).parse("07/15/2026 13:00:00", SUMMER), Some(SUMMER));
    // 2026-09-25 18:39:38 UTC.
    assert_eq!(FileDates::Utc.parse("09/25/2026 18:39:38", SUMMER), Some(1_790_361_578));
}

/// **A two-digit year is the one within 50 years of now**, as under `mit`:
/// in 2026, `76` is 1976 and `75` is 2075.
#[test]
fn utc_takes_a_two_digit_year_within_fifty_years_of_now() {
    assert_eq!(FileDates::Utc.parse("01/01/76 00:00:00", SUMMER), Some(189_302_400));
    assert_eq!(FileDates::Utc.parse("01/01/75 00:00:00", SUMMER), Some(3_313_526_400));
}

/// **Every hour from 1970 to 2099 goes round under `utc`**: what ozd
/// prints, the year in four digits, it reads back as the same instant, with
/// no hour a year missing or read twice, as there is under `mit`, and
/// whatever year `now` is in; and each is the calendar's UTC, measured
/// against `ozd::log::civil`. The two-digit form of the same fields is read
/// too, as the band's own before 1002 wrote it, within 50 years of now.
#[test]
fn every_hour_goes_round_under_utc() {
    let mut t = 0;
    while t < END {
        let shown = FileDates::Utc.print(t);
        let (y, m, d, hh, mm, ss) = ozd::log::civil(t as u64);
        assert_eq!(shown, format!("{m:02}/{d:02}/{y:04} {hh:02}:{mm:02}:{ss:02}"), "at {t}");
        assert_eq!(FileDates::Utc.parse(&shown, t), Some(t), "{shown} at {t}");
        assert_eq!(FileDates::Utc.parse(&shown, SUMMER), Some(t), "{shown} in 2026");
        let short = format!("{}{:02}{}", &shown[..6], y % 100, &shown[10..]);
        assert_eq!(FileDates::Utc.parse(&short, t), Some(t), "{short} at {t}");
        t += 3600;
    }
}

/// **What is not a date is not read under `utc`** either: the shape, the
/// calendar, and the range from 1970 to 2099.
#[test]
fn what_is_not_a_date_is_not_read_under_utc() {
    for bad in [
        "",
        "07/15/2026",
        "7/15/2026 12:00:00",
        "07-15-2026 12:00:00",
        "00/15/26 12:00:00",
        "13/15/26 12:00:00",
        "02/29/2027 00:00:00",
        "02/30/26 00:00:00",
        "07/15/26 24:00:00",
        "07/15/26 12:60:00",
        "07/15/26 12:00:60",
        "12/31/1969 23:59:59",
        "01/01/2100 00:00:00",
    ] {
        assert_eq!(FileDates::Utc.parse(bad, SUMMER), None, "{bad:?}");
    }
    assert_eq!(FileDates::Utc.parse("01/01/1970 00:00:00", SUMMER), Some(0));
}

/// **The default is `utc`.**
#[test]
fn the_default_is_utc() {
    assert_eq!(FileDates::default(), FileDates::Utc);
}
