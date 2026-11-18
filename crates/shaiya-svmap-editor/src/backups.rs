//! Timestamped backups of every save. Before a save replaces a file, the file as it is goes to
//! `<data>\backups\<folder key>\<id>-<UTC stamp>.svmap` (unless the newest backup of that map is the same bytes), and only the
//! newest `keep` backups of a map stay. The `<file>.bak` (the original before the first save) is separate and
//! never touched here.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::log;

/// `<data>\backups\<folder key>`: the folder key is the same hash as the recovery keys, over the folder that holds the map.
pub fn folder_dir(data: &Path, map_folder: &Path) -> PathBuf {
    data.join("backups").join(crate::settings::path_key(&map_folder.display().to_string()))
}

/// One backup file.
#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub path: PathBuf,
    pub when: SystemTime,
    pub size: u64,
    /// 0 for the first backup of a second, then 1, 2 ... (`_n` in the name).
    pub repeat: u32,
}

fn file_name(id: u32, at: SystemTime, n: u32) -> String {
    if n == 0 {
        format!("{id}-{}.svmap", log::utc_stamp(at))
    } else {
        format!("{id}-{}_{n}.svmap", log::utc_stamp(at))
    }
}

/// The stamp and the repeat counter inside a backup name `<id>-20261113-140205Z[_n].svmap`.
fn parse_name(id: u32, name: &str) -> Option<(SystemTime, u32)> {
    let rest = name.strip_prefix(&format!("{id}-"))?.strip_suffix(".svmap")?;
    let (stamp, n) = match rest.split_once('_') {
        Some((s, n)) => (s, n.parse::<u32>().ok()?),
        None => (rest, 0),
    };
    Some((log::parse_utc_stamp(stamp)?, n))
}

/// The backups of map `id` in `dir`, newest first (by the stamp in the name, then the repeat counter).
pub fn list(dir: &Path, id: u32) -> Vec<Backup> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<((SystemTime, u32), Backup)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = parse_name(id, &name)?;
            let size = e.metadata().ok()?.len();
            Some((key, Backup { path: e.path(), when: key.0, size, repeat: key.1 }))
        })
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.0));
    v.into_iter().map(|(_, b)| b).collect()
}

/// Copies the file that is about to be replaced into the backup folder. `Ok(None)` when there is nothing to back up (the file
/// does not exist yet, `keep` is 0, or the newest backup already has these bytes); otherwise the backup's path. Keeps the newest
/// `keep` backups of the map.
pub fn make(dir: &Path, id: u32, target: &Path, keep: u32, now: SystemTime) -> Result<Option<PathBuf>, String> {
    if keep == 0 || !target.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(target).map_err(|e| format!("cannot read {}: {e}", target.display()))?;
    let existing = list(dir, id);
    if let Some(newest) = existing.first() {
        if std::fs::read(&newest.path).ok().as_deref() == Some(bytes.as_slice()) {
            return Ok(None);
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    // a second backup within the same second gets the next counter (older ones may have been pruned, so count from the newest)
    let n = existing.iter().filter(|b| log::utc_stamp(b.when) == log::utc_stamp(now)).map(|b| b.repeat + 1).max().unwrap_or(0);
    let dest = dir.join(file_name(id, now, n));
    std::fs::write(&dest, &bytes).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    for old in list(dir, id).into_iter().skip(keep as usize) {
        let _ = std::fs::remove_file(old.path);
    }
    Ok(Some(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::time::Duration;

    fn t(s: u64) -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_secs(20_770 * 86_400 + s)
    }

    #[test]
    fn backups_keep_n_and_skip_equal_content() {
        let root = TempDir::new("backups-keep");
        let maps = root.join("maps");
        let data = root.join("data");
        std::fs::create_dir_all(&maps).unwrap();
        let target = maps.join("7.svmap");
        let dir = folder_dir(&data, &maps);
        let mut last = Vec::new();
        for i in 0..25u32 {
            std::fs::write(&target, format!("version {i}")).unwrap();
            // 25 saves inside the same second: the names must not collide
            let made = make(&dir, 7, &target, 20, t(100)).unwrap();
            assert!(made.is_some(), "save {i} made a backup");
            last = format!("version {i}").into_bytes();
        }
        let files = list(&dir, 7);
        assert_eq!(files.len(), 20);
        assert_eq!(std::fs::read(&files[0].path).unwrap(), last, "the newest backup is the file as it was before the last save");
        assert_eq!(std::fs::read(&files[19].path).unwrap(), b"version 5", "the oldest 5 are gone");
        // the same bytes again are not copied
        assert!(make(&dir, 7, &target, 20, t(200)).unwrap().is_none());
        assert_eq!(list(&dir, 7).len(), 20);
        // keep 0 = off, a missing file = nothing to do
        assert!(make(&dir, 7, &target, 0, t(300)).unwrap().is_none());
        assert!(make(&dir, 7, &maps.join("8.svmap"), 20, t(300)).unwrap().is_none());
        // other maps in the same folder are not mixed in
        std::fs::write(maps.join("70.svmap"), b"x").unwrap();
        make(&dir, 70, &maps.join("70.svmap"), 20, t(300)).unwrap();
        assert_eq!(list(&dir, 7).len(), 20);
        assert_eq!(list(&dir, 70).len(), 1);
    }

    #[test]
    fn names_carry_the_time() {
        assert_eq!(file_name(35, t(14 * 3600 + 2 * 60 + 5), 0), "35-20261113-140205Z.svmap");
        assert_eq!(file_name(35, t(14 * 3600 + 2 * 60 + 5), 2), "35-20261113-140205Z_2.svmap");
        assert_eq!(parse_name(35, "35-20261113-140205Z_2.svmap"), Some((t(14 * 3600 + 2 * 60 + 5), 2)));
        assert_eq!(parse_name(35, "35-20261113-140205Z.svmap"), Some((t(14 * 3600 + 2 * 60 + 5), 0)));
        assert_eq!(parse_name(3, "35-20261113-140205Z.svmap"), None);
        assert_eq!(parse_name(35, "35-garbage.svmap"), None);
    }
}
