//! Autosave and crash recovery. Unsaved work is written to the data folder's `recovery\` folder
//! (`<key>.svmap`, the map as `shaiya_svmap::write` makes it, and `<key>.json`, who it belongs to), never over the real file. A
//! window refreshes the `heartbeat` of its entries on every autosave tick; on the next start, entries whose heartbeat is old
//! are offered back and entries with a fresh heartbeat (another running editor window's) are left alone.
//!
//! Everything here is plain files and numbers: the app decides when to write, remove and ask.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use shaiya_svmap::Svmap;

use crate::doc::Stamp;

/// The file's identity when the work started from it: if it differs now, the file changed on disk since.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StampMs {
    pub modified_unix_ms: u64,
    pub len: u64,
}

pub fn stamp_ms(s: &Stamp) -> StampMs {
    StampMs { modified_unix_ms: s.modified.map_or(0, crate::log::unix_ms), len: s.len }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// The file the work belongs to; none for a map that was never saved.
    pub path: Option<String>,
    pub map_id: Option<u32>,
    pub title: String,
    pub base_stamp: Option<StampMs>,
    pub saved_at_unix_ms: u64,
    /// Undo steps the work consists of (information).
    pub steps: usize,
    pub heartbeat_unix_ms: u64,
    /// The entry also holds the client half of the tab (`<key>.wld` and `<key>.txt`).
    pub client: bool,
}

/// `<data>\recovery`.
pub fn dir(data: &Path) -> PathBuf {
    data.join("recovery")
}

/// The entry name of a document: 16 hex digits over the lower-cased file path, or `new-<pid>-<uid>` for a map with no file yet.
pub fn entry_key(path: Option<&Path>, uid: u64) -> String {
    match path {
        Some(p) => crate::settings::path_key(&p.display().to_string()),
        None => format!("new-{}-{uid}", std::process::id()),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp_name = path.file_name().ok_or("no file name")?.to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes one entry (the map first, then the description, each through a `.tmp` file and a rename).
pub fn write_entry(data: &Path, key: &str, map: &Svmap, meta: &Meta) -> Result<(), String> {
    let d = dir(data);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    write_atomic(&d.join(format!("{key}.svmap")), &shaiya_svmap::write(map))?;
    write_meta(data, key, meta)
}

/// Rewrites only the description (the heartbeat refresh).
pub fn write_meta(data: &Path, key: &str, meta: &Meta) -> Result<(), String> {
    let text = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    write_atomic(&dir(data).join(format!("{key}.json")), text.as_bytes())
}

/// Writes (or, with `None`, removes) the client half of an entry: the `.wld` model as `WldRaw::to_bytes` makes it and the text.
pub fn write_client(data: &Path, key: &str, client: Option<(&[u8], &[u8])>) -> Result<(), String> {
    let d = dir(data);
    match client {
        Some((wld, txt)) => {
            std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
            write_atomic(&d.join(format!("{key}.wld")), wld)?;
            write_atomic(&d.join(format!("{key}.txt")), txt)
        }
        None => {
            for ext in ["wld", "txt", "wld.tmp", "txt.tmp"] {
                let _ = std::fs::remove_file(d.join(format!("{key}.{ext}")));
            }
            Ok(())
        }
    }
}

pub fn remove_entry(data: &Path, key: &str) {
    let d = dir(data);
    for ext in ["svmap", "json", "svmap.tmp", "json.tmp", "wld", "txt", "wld.tmp", "txt.tmp"] {
        let _ = std::fs::remove_file(d.join(format!("{key}.{ext}")));
    }
}

/// Moves an entry that is already there (work from an earlier session nobody restored yet) out of the way, to
/// `<key>-<stamp>`, so a new autosave under the same key does not overwrite it; the next start offers both.
pub fn park_existing(data: &Path, key: &str, stamp: &str) {
    let d = dir(data);
    if d.join(format!("{key}.json")).exists() || d.join(format!("{key}.svmap")).exists() {
        for ext in ["svmap", "json", "wld", "txt"] {
            let _ = std::fs::rename(d.join(format!("{key}.{ext}")), d.join(format!("{key}-{stamp}.{ext}")));
        }
    }
}

/// An entry found on start.
#[derive(Debug)]
pub struct Entry {
    pub key: String,
    pub meta: Meta,
    /// The recovered map, or why it cannot be read ("damaged").
    pub map: Result<Svmap, String>,
    /// The client half: the `.wld` bytes and the text, when the entry has them.
    pub client: Option<(Vec<u8>, Vec<u8>)>,
}

/// What the dialog says about the file the work came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    /// The map was never saved.
    NoFile,
    Unchanged,
    /// The file on disk is not what the work started from.
    Changed,
    Gone,
}

impl Entry {
    pub fn file_state(&self) -> FileState {
        let Some(p) = &self.meta.path else { return FileState::NoFile };
        match crate::doc::stamp_of(Path::new(p)) {
            None => FileState::Gone,
            Some(now) => match self.meta.base_stamp {
                Some(base) if base == stamp_ms(&now) => FileState::Unchanged,
                _ => FileState::Changed,
            },
        }
    }

    pub fn saved_at(&self) -> SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(self.meta.saved_at_unix_ms)
    }
}

/// Reads the recovery folder. Entries whose heartbeat is younger than `live_ms` before `now_ms` belong to another running editor
/// window and are skipped. An entry whose description is unreadable shows up with a default one (title = its key) and a map
/// error, so it can be discarded.
pub fn scan(data: &Path, now_ms: u64, live_ms: u64) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir(data)) else { return out };
    let mut keys: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("json")).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())).flatten()
        })
        .collect();
    keys.sort();
    for key in keys {
        let meta_text = std::fs::read_to_string(dir(data).join(format!("{key}.json")));
        let meta: Option<Meta> = meta_text.ok().and_then(|t| serde_json::from_str(&t).ok());
        if let Some(m) = &meta {
            if now_ms.saturating_sub(m.heartbeat_unix_ms) < live_ms {
                continue;
            }
        }
        let map = match std::fs::read(dir(data).join(format!("{key}.svmap"))) {
            Ok(b) => shaiya_svmap::read(&b).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let (meta, map) = match meta {
            Some(m) => (m, map),
            None => (Meta { title: key.clone(), ..Default::default() }, Err("the description file is damaged".to_string())),
        };
        let client = if meta.client {
            match (std::fs::read(dir(data).join(format!("{key}.wld"))), std::fs::read(dir(data).join(format!("{key}.txt")))) {
                (Ok(w), Ok(t)) => Some((w, t)),
                _ => None,
            }
        } else {
            None
        };
        // client edits that cannot be read back make the whole entry damaged, so Restore never drops them silently
        let map = if meta.client && client.is_none() && map.is_ok() { Err("its client map file is missing".to_string()) } else { map };
        out.push(Entry { key, meta, map, client });
    }
    out
}

/// Heartbeats older than this are not a running window: twice the autosave interval plus ten seconds.
pub fn live_window_ms(autosave_secs: u32) -> u64 {
    (2 * autosave_secs.max(15) as u64 + 10) * 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use shaiya_svmap::{MobArea, MobEntry};

    fn map() -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [5.0, 2.0, 5.0], mobs: vec![MobEntry { mob: 3, count: 2 }] });
        m
    }

    fn meta(path: Option<&str>, heartbeat: u64) -> Meta {
        Meta { path: path.map(String::from), map_id: Some(9), title: "9.svmap".into(), base_stamp: None, saved_at_unix_ms: 5_000, steps: 2, heartbeat_unix_ms: heartbeat, client: false }
    }

    #[test]
    fn entries_roundtrip_and_are_removed() {
        let dir = TempDir::new("rec-roundtrip");
        write_entry(dir.path(), "abc", &map(), &meta(Some("D:\\x\\9.svmap"), 1_000)).unwrap();
        assert!(super::dir(dir.path()).join("abc.svmap").is_file() && super::dir(dir.path()).join("abc.json").is_file());
        assert!(!super::dir(dir.path()).join("abc.svmap.tmp").exists());
        let found = scan(dir.path(), 10_000_000, 70_000);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].map.as_ref().unwrap(), &map());
        assert_eq!(found[0].meta.steps, 2);
        remove_entry(dir.path(), "abc");
        assert!(scan(dir.path(), 10_000_000, 70_000).is_empty());
    }

    #[test]
    fn a_fresh_heartbeat_belongs_to_another_window() {
        let dir = TempDir::new("rec-heartbeat");
        write_entry(dir.path(), "live", &map(), &meta(None, 1_000_000)).unwrap();
        write_entry(dir.path(), "old", &map(), &meta(None, 100)).unwrap();
        let found = scan(dir.path(), 1_030_000, 70_000);
        assert_eq!(found.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["old"]);
        // once it has been quiet for longer than the window, it is offered too
        assert_eq!(scan(dir.path(), 1_100_000, 70_000).len(), 2);
    }

    #[test]
    fn damaged_entries_are_listed_and_can_be_discarded() {
        let dir = TempDir::new("rec-damaged");
        write_entry(dir.path(), "bad", &map(), &meta(None, 0)).unwrap();
        std::fs::write(super::dir(dir.path()).join("bad.svmap"), b"not a map").unwrap();
        let found = scan(dir.path(), 10_000_000, 70_000);
        assert_eq!(found.len(), 1);
        assert!(found[0].map.is_err());
        // a description that is not JSON: the entry still shows up
        std::fs::write(super::dir(dir.path()).join("junk.json"), b"{").unwrap();
        let found = scan(dir.path(), 10_000_000, 70_000);
        assert_eq!(found.len(), 2);
        assert!(found.iter().find(|e| e.key == "junk").is_some_and(|e| e.map.is_err()));
        remove_entry(dir.path(), "junk");
        remove_entry(dir.path(), "bad");
        assert!(scan(dir.path(), 10_000_000, 70_000).is_empty());
    }

    #[test]
    fn keys_follow_the_path_and_the_window_follows_the_interval() {
        assert_eq!(entry_key(Some(Path::new("D:\\Maps\\1.svmap")), 7), entry_key(Some(Path::new("d:\\maps\\1.SVMAP")), 8));
        assert_eq!(entry_key(Some(Path::new("a")), 1).len(), 16);
        assert!(entry_key(None, 5).starts_with("new-"));
        assert_eq!(live_window_ms(60), 130_000);
        assert_eq!(live_window_ms(0), 40_000);
    }
}
