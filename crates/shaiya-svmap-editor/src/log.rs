//! The log file of the data folder (`log.txt`, rotated at 1 MB to `log.1.txt`): what was opened and saved, errors, client
//! problems and panics. Nothing is written until `init` names a folder (the GUI start does; tests and `--no-settings` runs never
//! do), so a missing folder is simply "no log". Also the UTC time formatting the data folder's file names and messages use.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const ROTATE_AT: u64 = 1_000_000;

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Starts logging into `dir` (created when missing).
pub fn init(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    *DIR.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir.to_path_buf());
}

pub fn dir() -> Option<PathBuf> {
    DIR.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn info(msg: impl AsRef<str>) {
    write("info", msg.as_ref());
}

pub fn error(msg: impl AsRef<str>) {
    write("error", msg.as_ref());
}

fn write(level: &str, msg: &str) {
    let Some(dir) = dir() else { return };
    append(&dir, level, msg, SystemTime::now());
}

/// Appends one line to `<dir>/log.txt`, rotating first when the file is over 1 MB. Failures are ignored: the log must never
/// get in the way.
pub fn append(dir: &Path, level: &str, msg: &str, at: SystemTime) {
    let file = dir.join("log.txt");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() >= ROTATE_AT) {
        let old = dir.join("log.1.txt");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&file, &old);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let one_line = msg.replace(['\r', '\n'], " | ");
        let _ = writeln!(f, "{} {level:<5} {one_line}", utc_text(at));
    }
}

/// Installs a panic hook that logs the message and location, then runs the default hook (GUI start only).
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map_or("?".to_string(), |l| format!("{}:{}", l.file(), l.line()));
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        };
        write("panic", &format!("{msg} at {loc}"));
        default(info);
    }));
}

// ---------------------------------------------------------------------------------------------------------------- UTC time

// The stamp and time helpers live in the library (the same code, so the on-disk names stay as they were).
pub use shaiya_svmap_tools::stamp::{parse_utc_stamp, utc_parts, utc_stamp};

/// Milliseconds since 1970-01-01 (0 for a clock before it).
pub fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `2026-11-13 14:02 UTC (12 min ago)`, how backups and recovered work are dated in the UI.
pub fn utc_ago(at: SystemTime, now: SystemTime) -> String {
    let (y, mo, d, h, mi, _) = utc_parts(at);
    let secs = now.duration_since(at).map_or(0, |d| d.as_secs());
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC ({})", ago_text(secs))
}

/// `12 s ago`, `3 min ago`, `2 h ago`, `5 days ago`.
pub fn ago_text(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `2026-11-13 14:02:05 UTC`, the log's time.
pub fn utc_text(at: SystemTime) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(at);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use shaiya_svmap_tools::stamp::{civil_from_days, days_from_civil};

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
        assert_eq!(utc_stamp(t), "20261113-140205Z");
        assert_eq!(utc_text(t), "2026-11-13 14:02:05 UTC");
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000Z");
        // the stamp parses back, and a bad one does not
        assert_eq!(parse_utc_stamp("20261113-140205Z"), Some(t));
        assert_eq!(parse_utc_stamp("19700101-000000Z"), Some(UNIX_EPOCH));
        assert_eq!(parse_utc_stamp("20261313-140205Z"), None);
        assert_eq!(parse_utc_stamp("garbage"), None);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(utc_ago(t, t + Duration::from_secs(720)), "2026-11-13 14:02 UTC (12 min ago)");
        assert_eq!(ago_text(5), "5 s ago");
        assert_eq!(ago_text(7200), "2 h ago");
    }

    #[test]
    fn append_writes_lines_and_rotates_at_one_megabyte() {
        let dir = crate::testutil::TempDir::new("log");
        append(dir.path(), "info", "opened map 1\nsecond line", UNIX_EPOCH);
        let text = std::fs::read_to_string(dir.join("log.txt")).unwrap();
        assert_eq!(text, "1970-01-01 00:00:00 UTC info  opened map 1 | second line\n");
        std::fs::write(dir.join("log.txt"), vec![b'x'; ROTATE_AT as usize]).unwrap();
        append(dir.path(), "error", "after the rotation", UNIX_EPOCH);
        assert_eq!(std::fs::metadata(dir.join("log.1.txt")).unwrap().len(), ROTATE_AT);
        let fresh = std::fs::read_to_string(dir.join("log.txt")).unwrap();
        assert!(fresh.contains("after the rotation") && fresh.lines().count() == 1);
    }

    #[test]
    fn nothing_is_written_before_init() {
        // the global is never set by tests, so these are silent no-ops
        info("not written");
        error("not written");
        assert!(dir().is_none());
    }
}
