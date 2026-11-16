//! UTC formatting (no time zone database, no dependencies) and human-readable sizes.

use std::time::{SystemTime, UNIX_EPOCH};

/// (year, month, day) of a day count since 1970-01-01.
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
    unix_parts(secs)
}

/// Same as [`utc_parts`] for seconds since the Unix epoch.
pub fn unix_parts(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    (y, m, d, (rem / 3600) as u32, (rem % 3600 / 60) as u32, (rem % 60) as u32)
}

/// `20261115T120000Z` (file names).
pub fn utc_stamp(at: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(at);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// `2026-11-15T12:00:00Z` (JSON, logs).
pub fn utc_iso(at: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(at);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `2026-11-15T12:00:00Z` for seconds since the Unix epoch.
pub fn utc_iso_unix(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = unix_parts(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `2010-05-17T03:12:44Z` for seconds since the Unix epoch (same text as [`utc_iso_unix`]).
pub fn iso_from_unix(secs: i64) -> String {
    utc_iso_unix(secs)
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

/// Unix seconds (`1274079412`), a date (`2026-11-15`, midnight UTC) or `2026-11-15T12:00:00Z` (also `YYYY-MM-DD HH:MM:SS`, `Z` optional).
pub fn parse_utc(s: &str) -> crate::error::Result<i64> {
    let t = s.trim();
    let bad = || crate::error::Error::format("time", None, format!("{s:?} is not a time; use Unix seconds, 2026-11-15 or 2026-11-15T12:00:00Z"));
    if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) {
        return t.parse::<i64>().map_err(|_| bad());
    }
    let num = |x: &str, len: usize| -> Option<u32> { (x.len() == len && x.bytes().all(|b| b.is_ascii_digit())).then(|| x.parse().ok()).flatten() };
    let (date, time) = match t.split_once(['T', ' ']) {
        Some((d, tm)) => (d, Some(tm.trim_end_matches('Z'))),
        None => (t.trim_end_matches('Z'), None),
    };
    let mut dp = date.split('-');
    let (y, mo, d) = (dp.next().ok_or_else(bad)?, dp.next().ok_or_else(bad)?, dp.next().ok_or_else(bad)?);
    if dp.next().is_some() {
        return Err(bad());
    }
    let (y, mo, d) = (num(y, 4).ok_or_else(bad)? as i64, num(mo, 2).ok_or_else(bad)?, num(d, 2).ok_or_else(bad)?);
    let (mut h, mut mi, mut sec) = (0u32, 0u32, 0u32);
    if let Some(tm) = time {
        let mut tp = tm.split(':');
        h = num(tp.next().ok_or_else(bad)?, 2).ok_or_else(bad)?;
        mi = num(tp.next().ok_or_else(bad)?, 2).ok_or_else(bad)?;
        sec = match tp.next() {
            Some(x) => num(x, 2).ok_or_else(bad)?,
            None => 0,
        };
        if tp.next().is_some() {
            return Err(bad());
        }
    }
    let days = days_from_civil(y, mo, d);
    if !(1..=12).contains(&mo) || d == 0 || civil_from_days(days) != (y, mo, d) || h > 23 || mi > 59 || sec > 60 {
        return Err(bad());
    }
    Ok(days * 86_400 + h as i64 * 3600 + mi as i64 * 60 + sec as i64)
}

/// Decimal units: `512 B`, `1.9 KB`, `1.9 MB`, `2.0 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(20_772), (2026, 11, 15));
    }

    #[test]
    fn stamps_format() {
        let t = UNIX_EPOCH + Duration::from_secs(20_772 * 86_400 + 12 * 3600);
        assert_eq!(utc_stamp(t), "20261115T120000Z");
        assert_eq!(utc_iso(t), "2026-11-15T12:00:00Z");
        assert_eq!(utc_iso_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101T000000Z");
        let before = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(utc_iso(before), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn parse_utc_forms() {
        assert_eq!(parse_utc("1274079412").unwrap(), 1_274_079_412);
        assert_eq!(parse_utc("0").unwrap(), 0);
        assert_eq!(parse_utc("1970-01-01").unwrap(), 0);
        assert_eq!(parse_utc("2026-11-15").unwrap(), 20_772 * 86_400);
        assert_eq!(parse_utc("2026-11-15T12:00:00Z").unwrap(), 20_772 * 86_400 + 12 * 3600);
        assert_eq!(parse_utc("2026-11-15 12:00:01").unwrap(), 20_772 * 86_400 + 12 * 3600 + 1);
        assert_eq!(parse_utc("2026-11-15T12:30Z").unwrap(), 20_772 * 86_400 + 12 * 3600 + 1800);
        assert_eq!(parse_utc("2024-02-29").unwrap(), 19_782 * 86_400);
        assert_eq!(parse_utc("1969-12-31").unwrap(), -86_400);
        for bad in ["", "yesterday", "2026-13-01", "2025-02-29", "2026-11-15T25:00:00Z", "2026-11", "2026-11-15-01", "26-11-15"] {
            assert!(parse_utc(bad).is_err(), "{bad:?}");
        }
        assert_eq!(iso_from_unix(1_274_079_412), "2010-05-17T06:56:52Z");
        assert_eq!(days_from_civil(2026, 11, 15), 20_772);
        for d in [-800_000i64, -1, 0, 59, 60, 365, 11_017, 19_782, 20_772, 800_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_900_000), "1.9 MB");
        assert_eq!(human_size(1_991_954_802), "2.0 GB");
        assert_eq!(human_size(771_851), "771.9 KB");
    }
}
