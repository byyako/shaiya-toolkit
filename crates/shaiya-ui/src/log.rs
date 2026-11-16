//! The log file of a tool's data folder (`log.txt`, rotated at 1 MB to `log.1.txt`): what was opened and saved, errors and panics.
//! Nothing is written until `init` names a folder (the GUI start does; tests and `--no-settings` runs never do), so a missing folder is
//! simply "no log". Generalised from the svmap editor's `log.rs`; the UTC helpers live in `shaiya_core::time`.

use shaiya_core::time::utc_iso;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

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

pub fn warn(msg: impl AsRef<str>) {
    write("warn", msg.as_ref());
}

pub fn error(msg: impl AsRef<str>) {
    write("error", msg.as_ref());
}

fn write(level: &str, msg: &str) {
    let Some(dir) = dir() else { return };
    append(&dir, level, msg, SystemTime::now());
}

/// Appends one line to `<dir>/log.txt`, rotating first when the file is over 1 MB. Failures are ignored: the log must never get in the
/// way.
pub fn append(dir: &Path, level: &str, msg: &str, at: SystemTime) {
    let file = dir.join("log.txt");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() >= ROTATE_AT) {
        let old = dir.join("log.1.txt");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&file, &old);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
        let one_line = msg.replace(['\r', '\n'], " | ");
        let _ = writeln!(f, "{} {level:<5} {one_line}", utc_iso(at));
    }
}

/// Installs a panic hook that logs the message, location and a backtrace, then runs the default hook (GUI start only).
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
        let backtrace = std::backtrace::Backtrace::force_capture();
        write("panic", &format!("{msg} at {loc}\n{backtrace}"));
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lines_are_single_line_and_rotate() {
        let d = std::env::temp_dir().join(format!("shaiya-ui-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let t = std::time::UNIX_EPOCH + Duration::from_secs(20_772 * 86_400 + 12 * 3600);
        append(&d, "info", "first\nsecond", t);
        let text = std::fs::read_to_string(d.join("log.txt")).unwrap();
        assert_eq!(text, "2026-11-15T12:00:00Z info  first | second\n");
        // over 1 MB: the next line starts a new file and the old one becomes log.1.txt
        std::fs::write(d.join("log.txt"), vec![b'x'; 1_000_001]).unwrap();
        append(&d, "error", "after", t);
        assert!(d.join("log.1.txt").is_file());
        assert!(std::fs::read_to_string(d.join("log.txt")).unwrap().contains("error after"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
