//! Timestamped backups of every save. Before a save replaces a file, the file as it is goes to `<tool>/backups/<name>-<UTC stamp>.bak`
//! (unless the newest backup of that name is the same bytes), and only the newest `keep` backups of a name stay. Generalised from the
//! svmap editor's `backups.rs`: any file, not only `.svmap`.

use shaiya_core::time::utc_stamp;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One backup file.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub path: PathBuf,
    pub when: SystemTime,
    pub size: u64,
    /// 0 for the first backup of a second, then 1, 2 ... (`_n` in the name).
    pub repeat: u32,
}

#[derive(Clone, Debug)]
pub struct Backups {
    dir: PathBuf,
}

/// A file name made safe for use as the first part of a backup name.
fn safe(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' }).collect()
}

fn file_name(name: &str, at: SystemTime, n: u32) -> String {
    if n == 0 {
        format!("{}-{}.bak", safe(name), utc_stamp(at))
    } else {
        format!("{}-{}_{n}.bak", safe(name), utc_stamp(at))
    }
}

/// `(stamp text, repeat)` inside a backup name `<name>-<yyyymmddThhmmssZ>[_n].bak`.
fn parse_name(name: &str, file: &str) -> Option<(String, u32)> {
    let rest = file.strip_prefix(&format!("{}-", safe(name)))?.strip_suffix(".bak")?;
    let (stamp, n) = match rest.split_once('_') {
        Some((s, n)) => (s, n.parse::<u32>().ok()?),
        None => (rest, 0),
    };
    let b = stamp.as_bytes();
    let well_formed = b.len() == 16 && b[8] == b'T' && b[15] == b'Z' && stamp.chars().enumerate().all(|(i, c)| i == 8 || i == 15 || c.is_ascii_digit());
    well_formed.then(|| (stamp.to_string(), n))
}

fn stamp_to_time(stamp: &str) -> Option<SystemTime> {
    let n = |r: std::ops::Range<usize>| stamp.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (n(0..4)?, n(4..6)?, n(6..8)?, n(9..11)?, n(11..13)?, n(13..15)?);
    // days from civil (the inverse of shaiya_core::time::civil_from_days)
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + s;
    u64::try_from(secs).ok().map(|s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s))
}

impl Backups {
    pub fn new(dir: PathBuf) -> Backups {
        Backups { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The backups of `name`, newest first (by the stamp in the name, then the repeat counter).
    pub fn list(&self, name: &str) -> Vec<Backup> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<((String, u32), Backup)> = rd
            .flatten()
            .filter_map(|e| {
                let file = e.file_name().to_string_lossy().into_owned();
                let key = parse_name(name, &file)?;
                let when = stamp_to_time(&key.0)?;
                let size = e.metadata().ok()?.len();
                Some((key.clone(), Backup { path: e.path(), when, size, repeat: key.1 }))
            })
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().map(|(_, b)| b).collect()
    }

    /// Copies the file that is about to be replaced into the backup folder. `Ok(None)` when there is nothing to back up (the file does
    /// not exist yet, `keep` is 0, or the newest backup already has these bytes); otherwise the backup's path. Keeps the newest `keep`
    /// backups of the name.
    pub fn make(&self, name: &str, target: &Path, keep: u32, now: SystemTime) -> Result<Option<PathBuf>, String> {
        if keep == 0 || !target.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(target).map_err(|e| format!("cannot read {}: {e}", target.display()))?;
        let existing = self.list(name);
        if let Some(newest) = existing.first() {
            if std::fs::read(&newest.path).ok().as_deref() == Some(bytes.as_slice()) {
                return Ok(None);
            }
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        // a second backup within the same second gets the next counter (older ones may have been pruned, so count from the newest)
        let n = existing.iter().filter(|b| utc_stamp(b.when) == utc_stamp(now)).map(|b| b.repeat + 1).max().unwrap_or(0);
        let dest = self.dir.join(file_name(name, now, n));
        std::fs::write(&dest, &bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
        for old in self.list(name).into_iter().skip(keep as usize) {
            let _ = std::fs::remove_file(old.path);
        }
        Ok(Some(dest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(s: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(20_772 * 86_400 + s)
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-ui-bak-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn backups_keep_n_and_skip_equal_content() {
        let root = tmp("keep");
        let b = Backups::new(root.join("backups"));
        let target = root.join("Item.SData");
        let mut last = Vec::new();
        for i in 0..25u32 {
            std::fs::write(&target, format!("version {i}")).unwrap();
            // 25 saves inside the same second: the names must not collide
            assert!(b.make("Item.SData", &target, 20, t(100)).unwrap().is_some(), "save {i} made a backup");
            last = format!("version {i}").into_bytes();
        }
        let files = b.list("Item.SData");
        assert_eq!(files.len(), 20);
        assert_eq!(std::fs::read(&files[0].path).unwrap(), last, "the newest backup is the file as it was before the last save");
        assert_eq!(std::fs::read(&files[19].path).unwrap(), b"version 5", "the oldest 5 are gone");
        assert!(b.make("Item.SData", &target, 20, t(200)).unwrap().is_none(), "the same bytes are not copied again");
        assert!(b.make("Item.SData", &target, 0, t(300)).unwrap().is_none(), "keep 0 is off");
        assert!(b.make("Item.SData", &root.join("missing"), 20, t(300)).unwrap().is_none());
        // other names in the same folder are not mixed in
        let other = root.join("Skill.SData");
        std::fs::write(&other, b"x").unwrap();
        b.make("Skill.SData", &other, 20, t(300)).unwrap();
        assert_eq!(b.list("Item.SData").len(), 20);
        assert_eq!(b.list("Skill.SData").len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_carry_the_time() {
        let at = t(12 * 3600);
        assert_eq!(file_name("Item.SData", at, 0), "Item.SData-20261115T120000Z.bak");
        assert_eq!(file_name("Item.SData", at, 2), "Item.SData-20261115T120000Z_2.bak");
        assert_eq!(parse_name("Item.SData", "Item.SData-20261115T120000Z_2.bak"), Some(("20261115T120000Z".into(), 2)));
        assert_eq!(parse_name("Item.SData", "Item.SData-20261115T120000Z.bak"), Some(("20261115T120000Z".into(), 0)));
        assert_eq!(parse_name("Other", "Item.SData-20261115T120000Z.bak"), None);
        assert_eq!(parse_name("Item.SData", "Item.SData-garbage.bak"), None);
        assert_eq!(stamp_to_time("20261115T120000Z"), Some(at));
        assert_eq!(file_name("a/b:c", at, 0), "a_b_c-20261115T120000Z.bak");
    }
}
