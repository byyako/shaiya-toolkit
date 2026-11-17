//! Autosave and crash recovery. Unsaved work is written to the tool's `recovery/` folder (`<key>.bin`, the document bytes the tool
//! hands over, and `<key>.json`, who it belongs to), never over the real file. A window refreshes the `heartbeat` of its entries on every
//! autosave tick; on the next start, entries whose heartbeat is old are offered back and entries with a fresh heartbeat (another running
//! window's) are left alone. Generalised from the svmap editor's `recovery.rs`: the entry holds bytes and metadata instead of a map.

use crate::settings::path_key;
use serde::{Deserialize, Serialize};
use shaiya_core::fs::write_atomic;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    /// The file the work belongs to; none for a document that was never saved.
    pub path: Option<String>,
    pub title: String,
    /// Modified time (Unix ms) and length of the file when the work started from it.
    pub base_modified_ms: u64,
    pub base_len: u64,
    pub saved_at_unix_ms: u64,
    /// Undo steps the work consists of (information).
    pub steps: usize,
    pub heartbeat_unix_ms: u64,
    /// A tool-defined note (format name, schema, ...).
    pub note: String,
}

/// The recovery folder of one tool.
#[derive(Clone, Debug)]
pub struct Recovery {
    dir: PathBuf,
}

/// An entry found on start.
#[derive(Debug)]
pub struct Entry {
    pub key: String,
    pub meta: Meta,
    /// The recovered bytes, or why they cannot be read.
    pub bytes: Result<Vec<u8>, String>,
}

/// What a recovery dialog says about the file the work came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    NoFile,
    Unchanged,
    /// The file on disk is not what the work started from.
    Changed,
    Gone,
}

impl Entry {
    pub fn file_state(&self) -> FileState {
        let Some(p) = &self.meta.path else { return FileState::NoFile };
        match std::fs::metadata(p) {
            Err(_) => FileState::Gone,
            Ok(m) => {
                let ms = m.modified().ok().map_or(0, unix_ms);
                if ms == self.meta.base_modified_ms && m.len() == self.meta.base_len {
                    FileState::Unchanged
                } else {
                    FileState::Changed
                }
            }
        }
    }
}

/// Milliseconds since 1970-01-01 (0 for a clock before it).
pub fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn now_ms() -> u64 {
    unix_ms(SystemTime::now())
}

/// The entry name of a document: 16 hex digits over the lower-cased file path, or `new-<pid>-<uid>` for a document with no file yet.
pub fn entry_key(path: Option<&Path>, uid: u64) -> String {
    match path {
        Some(p) => path_key(&p.display().to_string()),
        None => format!("new-{}-{uid}", std::process::id()),
    }
}

/// Heartbeats older than this are not a running window: twice the autosave interval plus ten seconds.
pub fn live_window_ms(autosave_secs: u32) -> u64 {
    (2 * autosave_secs.max(15) as u64 + 10) * 1000
}

impl Recovery {
    pub fn new(dir: PathBuf) -> Recovery {
        Recovery { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes one entry (the bytes first, then the description, each atomically).
    pub fn write(&self, key: &str, bytes: &[u8], meta: &Meta) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        write_atomic(&self.dir.join(format!("{key}.bin")), bytes).map_err(|e| e.to_string())?;
        self.write_meta(key, meta)
    }

    /// Rewrites only the description (the heartbeat refresh).
    pub fn write_meta(&self, key: &str, meta: &Meta) -> Result<(), String> {
        let text = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        write_atomic(&self.dir.join(format!("{key}.json")), text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn remove(&self, key: &str) {
        for ext in ["bin", "json"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{key}.{ext}")));
        }
    }

    /// Moves an entry that is already there (work from an earlier session nobody restored yet) out of the way, to `<key>-<stamp>`, so a
    /// new autosave under the same key does not overwrite it; the next start offers both.
    pub fn park_existing(&self, key: &str, stamp: &str) {
        if self.dir.join(format!("{key}.json")).exists() || self.dir.join(format!("{key}.bin")).exists() {
            for ext in ["bin", "json"] {
                let _ = std::fs::rename(self.dir.join(format!("{key}.{ext}")), self.dir.join(format!("{key}-{stamp}.{ext}")));
            }
        }
    }

    /// Reads the recovery folder. Entries whose heartbeat is younger than `live_ms` before `now_ms` belong to another running window and
    /// are skipped. An entry whose description is unreadable shows up with a default one (title = its key) and a bytes error, so it can
    /// be discarded.
    pub fn scan(&self, now_ms: u64, live_ms: u64) -> Vec<Entry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        let mut keys: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("json")).then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())).flatten()
            })
            .collect();
        keys.sort();
        for key in keys {
            let meta: Option<Meta> = std::fs::read_to_string(self.dir.join(format!("{key}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok());
            if let Some(m) = &meta {
                if now_ms.saturating_sub(m.heartbeat_unix_ms) < live_ms {
                    continue;
                }
            }
            let bytes = std::fs::read(self.dir.join(format!("{key}.bin"))).map_err(|e| e.to_string());
            let (meta, bytes) = match meta {
                Some(m) => (m, bytes),
                None => (Meta { title: key.clone(), ..Default::default() }, Err("the description file is damaged".to_string())),
            };
            out.push(Entry { key, meta, bytes });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-ui-rec-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn meta(path: Option<&str>, heartbeat: u64) -> Meta {
        Meta { path: path.map(String::from), title: "Item.SData".into(), saved_at_unix_ms: 5_000, steps: 2, heartbeat_unix_ms: heartbeat, ..Default::default() }
    }

    #[test]
    fn entries_round_trip_and_are_removed() {
        let r = Recovery::new(tmp("roundtrip"));
        r.write("abc", b"unsaved bytes", &meta(Some("C:\\x\\Item.SData"), 1_000)).unwrap();
        assert!(r.dir().join("abc.bin").is_file() && r.dir().join("abc.json").is_file());
        let found = r.scan(10_000_000, 70_000);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bytes.as_ref().unwrap(), b"unsaved bytes");
        assert_eq!(found[0].meta.steps, 2);
        r.remove("abc");
        assert!(r.scan(10_000_000, 70_000).is_empty());
        let _ = std::fs::remove_dir_all(r.dir());
    }

    #[test]
    fn a_fresh_heartbeat_belongs_to_another_window() {
        let r = Recovery::new(tmp("heartbeat"));
        r.write("live", b"x", &meta(None, 1_000_000)).unwrap();
        r.write("old", b"x", &meta(None, 100)).unwrap();
        let found = r.scan(1_030_000, 70_000);
        assert_eq!(found.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["old"]);
        assert_eq!(r.scan(1_100_000, 70_000).len(), 2, "quiet for longer than the window: offered too");
        let _ = std::fs::remove_dir_all(r.dir());
    }

    #[test]
    fn damaged_entries_are_listed_and_can_be_discarded() {
        let r = Recovery::new(tmp("damaged"));
        r.write("bad", b"x", &meta(None, 0)).unwrap();
        std::fs::remove_file(r.dir().join("bad.bin")).unwrap();
        std::fs::write(r.dir().join("junk.json"), b"{").unwrap();
        let found = r.scan(10_000_000, 70_000);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|e| e.bytes.is_err()));
        r.remove("junk");
        r.remove("bad");
        assert!(r.scan(10_000_000, 70_000).is_empty());
        let _ = std::fs::remove_dir_all(r.dir());
    }

    #[test]
    fn parking_keeps_the_old_entry_and_file_state_is_reported() {
        let r = Recovery::new(tmp("park"));
        r.write("k", b"old work", &meta(None, 0)).unwrap();
        r.park_existing("k", "20261115T120000Z");
        r.write("k", b"new work", &meta(None, 0)).unwrap();
        let mut found = r.scan(10_000_000, 70_000);
        found.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(found.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["k", "k-20261115T120000Z"]);
        assert_eq!(found[1].bytes.as_ref().unwrap(), b"old work");
        // file state
        let f = std::env::temp_dir().join(format!("shaiya-ui-rec-file-{}", std::process::id()));
        std::fs::write(&f, b"12345").unwrap();
        let m = std::fs::metadata(&f).unwrap();
        let base = Meta { path: Some(f.display().to_string()), base_modified_ms: unix_ms(m.modified().unwrap()), base_len: 5, ..Default::default() };
        let e = Entry { key: "k".into(), meta: base.clone(), bytes: Ok(vec![]) };
        assert_eq!(e.file_state(), FileState::Unchanged);
        std::fs::write(&f, b"123456789").unwrap();
        assert_eq!(e.file_state(), FileState::Changed);
        std::fs::remove_file(&f).unwrap();
        assert_eq!(e.file_state(), FileState::Gone);
        assert_eq!(Entry { key: "k".into(), meta: Meta::default(), bytes: Ok(vec![]) }.file_state(), FileState::NoFile);
        let _ = std::fs::remove_dir_all(r.dir());
    }

    #[test]
    fn keys_follow_the_path_and_the_window_follows_the_interval() {
        assert_eq!(entry_key(Some(Path::new("C:\\Maps\\1.x")), 7), entry_key(Some(Path::new("c:\\maps\\1.X")), 8));
        assert_eq!(entry_key(Some(Path::new("a")), 1).len(), 16);
        assert!(entry_key(None, 5).starts_with("new-"));
        assert_eq!(live_window_ms(60), 130_000);
        assert_eq!(live_window_ms(0), 40_000);
    }
}
