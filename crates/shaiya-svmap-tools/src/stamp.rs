//! UTC time helpers: the `20261113-140205Z` stamp that backup names, install records and client file backups carry. The form is
//! fixed (files the editor already wrote must stay readable); it is not the `shaiya_core::time` stamp.

use std::time::{SystemTime, UNIX_EPOCH};

/// (year, month, day) of a day count since 1970-01-01 (Howard Hinnant's civil-from-days).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day, hour, minute, second) in UTC.
pub fn utc_parts(at: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = match at.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// `20261113-140205Z`, the stamp of backup file names.
pub fn utc_stamp(at: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(at);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}Z")
}


/// Day count since 1970-01-01 of a civil date (the inverse of [`civil_from_days`]).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The time a `20261113-140205Z` stamp names.
pub fn parse_utc_stamp(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() != 16 || b[8] != b'-' || b[15] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let (y, mo, d, h, mi, sec) = (n(0..4)?, n(4..6)?, n(6..8)?, n(9..11)?, n(11..13)?, n(13..15)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let secs = days_from_civil(y as i64, mo, d) * 86_400 + h as i64 * 3600 + mi as i64 * 60 + sec as i64;
    u64::try_from(secs).ok().map(|s| UNIX_EPOCH + std::time::Duration::from_secs(s))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;


    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29)); // leap day
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(20_770), (2026, 11, 13));
    }

    #[test]
    fn stamps_format() {
        // 2026-11-13 14:02:05 UTC
        let t = UNIX_EPOCH + Duration::from_secs(20_770 * 86_400 + 14 * 3600 + 2 * 60 + 5);
        assert_eq!(utc_parts(t), (2026, 11, 13, 14, 2, 5));
        assert_eq!(utc_stamp(t), "20261113-140205Z");
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000Z");
        // the stamp parses back, and a bad one does not
        assert_eq!(parse_utc_stamp("20261113-140205Z"), Some(t));
        assert_eq!(parse_utc_stamp("19700101-000000Z"), Some(UNIX_EPOCH));
        assert_eq!(parse_utc_stamp("20261313-140205Z"), None);
        assert_eq!(parse_utc_stamp("garbage"), None);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }
}
