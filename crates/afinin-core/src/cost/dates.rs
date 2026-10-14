// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar helpers for `YYYY-MM-DD` dates (UTC, as used by Cost Explorer).

use time::{
    Date, Duration, Month, OffsetDateTime, format_description::FormatItem,
    macros::format_description,
};

use crate::error::{Error, Result};

const FORMAT: &[FormatItem<'static>] = format_description!("[year]-[month]-[day]");

pub fn parse(s: &str) -> Result<Date> {
    Date::parse(s, FORMAT)
        .map_err(|_| Error::Other(format!("invalid date `{s}`, expected YYYY-MM-DD")))
}

pub fn format(d: Date) -> String {
    d.format(FORMAT).expect("date formatting cannot fail")
}

pub fn today_utc() -> Date {
    OffsetDateTime::now_utc().date()
}

pub fn first_of_month(d: Date) -> Date {
    d.replace_day(1).expect("day 1 is always valid")
}

/// First day of the month `n` months after (or before, if negative) `d`'s month.
pub fn add_months(d: Date, n: i32) -> Date {
    let total = d.year() * 12 + (d.month() as i32 - 1) + n;
    let (year, month0) = (total.div_euclid(12), total.rem_euclid(12));
    let month = Month::try_from(month0 as u8 + 1).expect("month in 1..=12");
    Date::from_calendar_date(year, month, 1).expect("valid first of month")
}

pub fn add_days(d: Date, n: i64) -> Date {
    d + Duration::days(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_arithmetic() {
        let d = parse("2026-01-31").unwrap();
        assert_eq!(format(add_months(d, -1)), "2025-12-01");
        assert_eq!(format(add_months(d, 1)), "2026-02-01");
        assert_eq!(format(add_months(d, -12)), "2025-01-01");
        assert_eq!(format(first_of_month(d)), "2026-01-01");
        assert_eq!(format(add_days(d, 1)), "2026-02-01");
        assert!(parse("2026-13-01").is_err());
    }
}
