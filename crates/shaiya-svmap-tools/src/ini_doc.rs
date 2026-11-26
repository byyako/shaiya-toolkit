//! The server's `Map.ini` or `MapWar.ini` as an editable document.
//!
//! `Doc` holds a `.svmap`, so this is a separate document kind with the same rules: every edit is one undo step, a step is a list
//! of whole-line splices ([`LineEdit`]) that carry what they removed, the document is dirty while the history is not at the saved
//! depth, and a save is the safe save of the client files (`.tmp`, read back, compare, timestamped backup, `.bak` once, rename).
//! Lines outside an edit keep their bytes, so the cp949 comments and the CRLF endings stay as they were.

use std::path::{Path, PathBuf};

use crate::client_files::{self, BackupPlan, Saved};
use crate::mapini::{LineEdit, MapIni, WAR_FIRST};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IniKind {
    /// `Map.ini`: zone `n` is map `n`; `MapCount` = highest zone id + 1.
    Map,
    /// `MapWar.ini`: zone `n` is map `n` (100 and up); `MapCount` = number of war maps.
    War,
}

impl IniKind {
    pub fn file_name(self) -> &'static str {
        match self {
            IniKind::Map => "Map.ini",
            IniKind::War => "MapWar.ini",
        }
    }

    pub fn is_war(self) -> bool {
        self == IniKind::War
    }
}

#[derive(Clone, Debug)]
struct Step {
    label: String,
    edits: Vec<LineEdit>,
}

pub struct IniDoc {
    pub kind: IniKind,
    pub path: PathBuf,
    pub ini: MapIni,
    /// The bytes on disk when the file was read or last saved: a save refuses when the file is not these any more.
    on_disk: Vec<u8>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    saved_depth: Option<usize>,
    /// Bumped by every change of the text (edit, undo, redo, reload): the form drops its typed buffers when it moves.
    pub version: u64,
}

impl IniDoc {
    /// Reads the file. A file without `[BASE] MapCount` is refused: the server would not read it either.
    pub fn open(kind: IniKind, path: &Path) -> Result<IniDoc, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        IniDoc::from_bytes(kind, path, bytes)
    }

    pub fn from_bytes(kind: IniKind, path: &Path, bytes: Vec<u8>) -> Result<IniDoc, String> {
        let ini = MapIni::parse(&bytes);
        if ini.map_count().is_none() {
            return Err(format!("{} has no [BASE] MapCount, so it is not edited.", path.display()));
        }
        Ok(IniDoc { kind, path: path.to_path_buf(), ini, on_disk: bytes, undo: Vec::new(), redo: Vec::new(), saved_depth: Some(0), version: 0 })
    }

    pub fn dirty(&self) -> bool {
        self.saved_depth != Some(self.undo.len())
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    /// One undo step. Edits that change nothing make no step. Returns whether a step was made.
    pub fn commit(&mut self, label: impl Into<String>, edits: Vec<LineEdit>) -> bool {
        if edits.is_empty() {
            return false;
        }
        for e in &edits {
            self.ini.apply(e);
        }
        self.undo.push(Step { label: label.into(), edits });
        self.redo.clear();
        // a saved depth above the new top can never be reached again
        if self.saved_depth.is_some_and(|d| d >= self.undo.len()) {
            self.saved_depth = None;
        }
        self.version += 1;
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        for e in step.edits.iter().rev() {
            self.ini.apply(&e.inverse());
        }
        self.redo.push(step);
        self.version += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        for e in &step.edits {
            self.ini.apply(e);
        }
        self.undo.push(step);
        self.version += 1;
        true
    }

    fn zone_section(&self, id: u32) -> Result<String, String> {
        self.ini.zone(id).map(|s| s.name.clone()).ok_or_else(|| format!("[SET_ZONE_{id}] is not in {}", self.kind.file_name()))
    }

    /// Sets a key of zone `id` (a new line after the zone's last key when it is not there yet).
    pub fn set_key(&mut self, id: u32, key: &str, value: &str) -> Result<bool, String> {
        let name = self.zone_section(id)?;
        let edits = self.ini.set_value_edits(&name, key, value)?;
        Ok(self.commit(format!("zone {id}: {key}"), edits))
    }

    /// Drops a key of zone `id`; the zone then falls back to `[DEFAULT_SET_ZONE]`.
    pub fn remove_key(&mut self, id: u32, key: &str) -> Result<bool, String> {
        let name = self.zone_section(id)?;
        let edit = self.ini.remove_key_edit(&name, key);
        Ok(self.commit(format!("zone {id}: remove {key}"), edit.into_iter().collect()))
    }

    /// Appends zone `id` with `keys` and raises `MapCount`: one step.
    pub fn add_zone(&mut self, id: u32, keys: &[(&str, &str)]) -> Result<(), String> {
        if self.kind.is_war() && id < WAR_FIRST {
            return Err(format!("A war map id starts at {WAR_FIRST}."));
        }
        let edits = self.ini.add_zone_edits(id, keys, self.kind.is_war())?;
        self.commit(format!("add zone {id}"), edits);
        Ok(())
    }

    /// Writes the file: `.tmp`, read back, compared and parsed again, a timestamped copy and a `.bak` of the file it replaces,
    /// then the rename. `backups` is `(<data>\backups\server\<key>, copies to keep)`. `forbid` is the client folder: nothing is
    /// written inside it. The file is refused when it changed on disk since it was read (reload first).
    pub fn save(&mut self, backups: Option<(&Path, u32)>, forbid: Option<&Path>) -> Result<Saved, String> {
        match std::fs::read(&self.path) {
            Ok(now) if now != self.on_disk => {
                return Err(format!("{} changed on disk since it was read. Reload it first; the edits here would overwrite the other change.", self.path.display()));
            }
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
            Ok(_) => {}
        }
        let bytes = self.ini.to_bytes();
        let want = bytes.clone();
        let verify = move |back: &[u8]| -> Result<(), String> {
            let parsed = MapIni::parse(back);
            if parsed.to_bytes() != want {
                return Err("the file does not read back as the model".to_string());
            }
            if parsed.map_count().is_none() {
                return Err("the file has no [BASE] MapCount".to_string());
            }
            Ok(())
        };
        let stem = self.kind.file_name().trim_end_matches(".ini").to_string();
        let plan = backups.map(|(dir, keep)| BackupPlan { dir, stem: &stem, ext: "ini", keep });
        let saved = client_files::write_verified(&self.path, &bytes, &verify, plan, forbid)?;
        self.on_disk = bytes;
        self.saved_depth = Some(self.undo.len());
        Ok(saved)
    }

    /// Reads the file again and drops every unsaved edit and the history.
    pub fn reload(&mut self) -> Result<(), String> {
        let fresh = IniDoc::open(self.kind, &self.path)?;
        self.ini = fresh.ini;
        self.on_disk = fresh.on_disk;
        self.undo.clear();
        self.redo.clear();
        self.saved_depth = Some(0);
        self.version += 1;
        Ok(())
    }

    /// The value text of `key` in zone `id` as the zone writes it, or `None` when the zone leaves it out.
    pub fn own_value(&self, id: u32, key: &str) -> Option<String> {
        self.ini.zone(id).and_then(|z| self.ini.value(z, key))
    }

    /// The value the zone would get from `[DEFAULT_SET_ZONE]`.
    pub fn default_value(&self, key: &str) -> Option<String> {
        self.ini.section("DEFAULT_SET_ZONE").and_then(|d| self.ini.value(d, key))
    }
}

/// The folder for the timestamped copies of a server file: `<data>\backups\server\<key of the folder>`.
pub fn backup_dir(data: &Path, server_dir: &Path) -> PathBuf {
    data.join("backups").join("server").join(crate::keys::path_key(&server_dir.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &[u8] = b"//////\r\n/*\r\nMapCount = 9\r\n*/\r\n[BASE]\r\nMapCount\t  = 3\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\nWeatherState = 0\r\n\r\n[SET_ZONE_0]\r\nMapType\t\t = F\t// a \xB0\xA1 comment\r\n\r\n[SET_ZONE_1]\r\nMapType = D\r\nRebirthMapPos = 1, 5, 6, 7\r\n\r\n[SET_ZONE_2]\r\nMapType = F\r\n\r\n";

    fn doc() -> IniDoc {
        IniDoc::from_bytes(IniKind::Map, Path::new("Map.ini"), FILE.to_vec()).unwrap()
    }

    #[test]
    fn edits_undo_redo_and_dirty() {
        let mut d = doc();
        assert!(!d.dirty());
        assert!(d.set_key(1, "MapType", "F").unwrap());
        assert!(d.dirty() && d.ini.zone_value(1, "MapType").as_deref() == Some("F"));
        assert!(!d.set_key(1, "MapType", "F").unwrap(), "the same value makes no step");
        d.undo();
        assert!(!d.dirty());
        assert_eq!(d.ini.to_bytes(), FILE);
        d.redo();
        assert!(d.dirty());
        d.set_key(2, "WeatherState", "1").unwrap();
        d.undo();
        d.undo();
        assert_eq!(d.ini.to_bytes(), FILE);
        assert!(d.undo_label().is_none() && d.redo_label().is_some());
    }

    #[test]
    fn a_new_key_goes_after_the_last_key_and_a_removed_key_leaves_the_default() {
        let mut d = doc();
        d.set_key(0, "RebirthMapPos1", "0, 1, 2, 3").unwrap();
        let text = String::from_utf8_lossy(&d.ini.to_bytes()).into_owned();
        assert!(text.contains("MapType\t\t = F\t// a \u{fffd}\u{fffd} comment\r\nRebirthMapPos1\t = 0, 1, 2, 3\r\n\r\n[SET_ZONE_1]"), "{text}");
        d.remove_key(1, "MapType").unwrap();
        assert_eq!(d.ini.zone_value(1, "MapType").as_deref(), Some("F"), "the default");
        assert_eq!(d.own_value(1, "MapType"), None);
        assert_eq!(d.default_value("WeatherState").as_deref(), Some("0"));
        assert!(d.remove_key(1, "NoSuchKey").map(|b| !b).unwrap());
    }

    #[test]
    fn values_that_would_break_the_file_are_refused() {
        let mut d = doc();
        for bad in ["", "  ", "a\r\nb", "x // y", "x /* y", "caf\u{e9}"] {
            assert!(d.set_key(1, "MapType", bad).is_err(), "{bad:?}");
        }
        assert!(d.set_key(1, "Bad Key", "1").is_err());
        assert!(d.set_key(9, "MapType", "F").is_err(), "no such zone");
        assert!(!d.dirty());
    }

    #[test]
    fn a_file_without_an_ending_and_a_key_on_the_last_line() {
        let raw = b"[BASE]\r\nMapCount = 1\r\n[SET_ZONE_0]\r\nMapType = F";
        let mut d = IniDoc::from_bytes(IniKind::Map, Path::new("Map.ini"), raw.to_vec()).unwrap();
        d.set_key(0, "WarType", "P").unwrap();
        assert_eq!(d.ini.to_bytes(), b"[BASE]\r\nMapCount = 1\r\n[SET_ZONE_0]\r\nMapType = F\r\nWarType\t = P");
        d.undo();
        assert_eq!(d.ini.to_bytes(), raw);
        d.remove_key(0, "MapType").unwrap();
        assert_eq!(d.ini.to_bytes(), b"[BASE]\r\nMapCount = 1\r\n[SET_ZONE_0]", "the file still has no ending after its last line");
        d.undo();
        assert_eq!(d.ini.to_bytes(), raw);
        d.add_zone(1, &[("MapType", "D")]).unwrap();
        assert_eq!(d.ini.to_bytes(), b"[BASE]\r\nMapCount = 2\r\n[SET_ZONE_0]\r\nMapType = F\r\n\r\n[SET_ZONE_1]\r\nMapType\t = D\r\n");
        d.undo();
        assert_eq!(d.ini.to_bytes(), raw);
    }

    // ------------------------------------------------------------------------------------------ the install row

    fn lines(b: &[u8]) -> Vec<Vec<u8>> {
        b.split_inclusive(|c| *c == b'\n').map(<[u8]>::to_vec).collect()
    }

    /// The lines that differ between two files, found by cutting the common start and end: (first changed line, lines removed,
    /// lines inserted).
    fn changed(before: &[u8], after: &[u8]) -> (usize, usize, usize) {
        let (a, b) = (lines(before), lines(after));
        let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
        let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
        (pre, a.len() - pre - suf, b.len() - pre - suf)
    }

    /// The line range of zone `id`: its head up to the line before the next section.
    fn zone_lines(ini: &MapIni, id: u32) -> std::ops::Range<usize> {
        let z = ini.zone(id).unwrap();
        let next = ini.sections.iter().map(|s| s.line).filter(|l| *l > z.line).min().unwrap_or(ini.lines.len());
        z.line..next
    }

    fn server_dir() -> Option<PathBuf> {
        let dir = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?.parent()?.to_path_buf();
        dir.join("Map.ini").is_file().then_some(dir)
    }

    /// An edit of one zone changes only that zone's lines: every other byte of the file stays, the cp949 comments and the CRLF
    /// endings too. Undo gives the file back; the saved file reads back; the first save keeps a `.bak` and a timestamped copy.
    #[test]
    fn mapini_edit_zone_only_changes_lines() {
        // the hand-made file
        let mut d = doc();
        let before = d.ini.to_bytes();
        d.set_key(1, "MapType", "F").unwrap();
        d.set_key(1, "WeatherState", "2").unwrap();
        d.remove_key(1, "RebirthMapPos").unwrap();
        let after = d.ini.to_bytes();
        let range = zone_lines(&MapIni::parse(&before), 1);
        let (at, removed, inserted) = changed(&before, &after);
        assert!(at >= range.start && at + removed <= range.end, "the change {at}+{removed} is outside zone 1 {range:?}");
        assert!(removed <= 2 && inserted <= 2, "{removed} {inserted}");
        assert!(after.windows(4).any(|w| w == b"\xB0\xA1 c"), "the cp949 comment of zone 0 stays");
        assert!(lines(&after).iter().all(|l| l.ends_with(b"\r\n")), "CRLF everywhere");
        for _ in 0..3 {
            d.undo();
        }
        assert_eq!(d.ini.to_bytes(), before);

        // the original file of the server, on a copy
        let Some(dir) = server_dir() else { return };
        let tmp = crate::testutil::TempDir::new("c5-edit");
        let path = tmp.join("Map.ini");
        std::fs::copy(dir.join("Map.ini"), &path).unwrap();
        let orig = std::fs::read(&path).unwrap();
        let mut d = IniDoc::open(IniKind::Map, &path).unwrap();
        let orig_ini = d.ini.clone();
        for zone in [0u32, 5, 47, 67] {
            let range = zone_lines(&orig_ini, zone);
            let mut e = IniDoc::open(IniKind::Map, &path).unwrap();
            let t = if e.ini.zone_value(zone, "MapType").as_deref() == Some("D") { "F" } else { "D" };
            e.set_key(zone, "MapType", t).unwrap();
            e.set_key(zone, "ExpireTime", "7").unwrap();
            e.set_key(zone, "RebirthMapPos2", "1, 2.5, 3, 4").unwrap();
            let now = e.ini.to_bytes();
            let (at, removed, inserted) = changed(&orig, &now);
            assert!(at >= range.start && at + removed <= range.end, "zone {zone}: the change {at}+{removed} is outside {range:?}");
            // every line before and after the change is byte-identical
            let (a, b) = (lines(&orig), lines(&now));
            assert_eq!(a[..at], b[..at]);
            assert_eq!(a[at + removed..], b[at + inserted..]);
            assert_eq!(e.ini.zone_value(zone, "MapType").as_deref(), Some(t));
            assert_eq!(e.ini.zone_value(zone, "ExpireTime").as_deref(), Some("7"));
            while e.undo() {}
            assert_eq!(e.ini.to_bytes(), orig, "undo gives zone {zone} back");
        }
        // save: verified, with a .bak and a timestamped copy; the file reads back as the model
        d.set_key(5, "MapType", "F").unwrap();
        let backups = tmp.join("backups");
        let saved = d.save(Some((&backups, 5)), None).unwrap();
        assert!(saved.bak_made && saved.backup.is_some());
        assert_eq!(std::fs::read(tmp.join("Map.ini.bak")).unwrap(), orig, "the .bak is the original file");
        assert_eq!(std::fs::read(saved.backup.unwrap()).unwrap(), orig);
        assert_eq!(std::fs::read(&path).unwrap(), d.ini.to_bytes());
        assert!(!d.dirty() && !tmp.join("Map.ini.tmp").exists());
        assert_eq!(MapIni::parse(&std::fs::read(&path).unwrap()).zone_value(5, "MapType").as_deref(), Some("F"));
        // a change of the file on disk by someone else stops the next save
        std::fs::write(&path, b"[BASE]\r\nMapCount = 1\r\n").unwrap();
        d.set_key(6, "MapType", "F").unwrap();
        let e = d.save(Some((&backups, 5)), None).unwrap_err();
        assert!(e.contains("changed on disk"), "{e}");
        assert_eq!(std::fs::read(&path).unwrap(), b"[BASE]\r\nMapCount = 1\r\n", "the other change is untouched");
    }

    /// Review: a save into the client folder is refused, also by another name of it (the administrative share), and so is a
    /// read-only file; neither leaves a `.tmp`, and the file stays as it was.
    #[test]
    fn save_refuses_client_folder_and_read_only() {
        let tmp = crate::testutil::TempDir::new("c5-save-refuse");
        let client = tmp.join("client");
        std::fs::create_dir_all(&client).unwrap();
        let path = client.join("Map.ini");
        std::fs::write(&path, FILE).unwrap();
        let mut d = IniDoc::open(IniKind::Map, &path).unwrap();
        d.set_key(2, "MapType", "D").unwrap();
        let e = d.save(None, Some(&client)).unwrap_err();
        assert!(e.contains("client folder"), "{e}");
        let s = std::path::absolute(&client).unwrap().display().to_string();
        if let Some(unc) = s.split_once(":\\").map(|(drive, rest)| PathBuf::from(format!(r"\\localhost\{drive}$\{rest}"))) {
            if unc.is_dir() {
                let e = d.save(None, Some(&unc)).unwrap_err();
                assert!(e.contains("client folder"), "the share is the same folder: {e}");
            }
        }
        let mut p = std::fs::metadata(&path).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&path, p.clone()).unwrap();
        let e = d.save(None, None).unwrap_err();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(&path, p).unwrap();
        assert!(e.contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&path).unwrap(), FILE);
        assert!(!client.join("Map.ini.tmp").exists() && d.dirty());
    }

    /// A new zone is appended at the end and `MapCount` becomes the highest zone id + 1; nothing else changes.
    #[test]
    fn mapini_add_zone_mapcount() {
        let mut d = doc();
        let before = d.ini.to_bytes();
        d.add_zone(3, &[("MapType", "D")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(4));
        let after = d.ini.to_bytes();
        let (a, b) = (lines(&before), lines(&after));
        // line 5 is MapCount; the new zone is appended; no other line moved
        assert_eq!(a[..5], b[..5]);
        assert_eq!(b[5], b"MapCount\t  = 4\r\n");
        assert_eq!(a[6..], b[6..a.len()]);
        assert_eq!(&b[a.len()..], &[b"[SET_ZONE_3]\r\n".to_vec(), b"MapType\t = D\r\n".to_vec()][..]);
        assert!(d.add_zone(3, &[]).is_err(), "an id that is there is refused");
        assert!(d.add_zone(4, &[("Bad Key", "1")]).is_err());
        // a gap: the count follows the highest id
        d.add_zone(9, &[("MapType", "F")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(10));
        d.undo();
        d.undo();
        assert_eq!(d.ini.to_bytes(), before, "one undo per zone");
        // a lower id never lowers MapCount
        let raw = b"[BASE]\r\nMapCount = 20\r\n[SET_ZONE_2]\r\nMapType = F\r\n";
        let mut e = IniDoc::from_bytes(IniKind::Map, Path::new("Map.ini"), raw.to_vec()).unwrap();
        e.add_zone(3, &[("MapType", "F")]).unwrap();
        assert_eq!(e.ini.map_count(), Some(20));

        let Some(dir) = server_dir() else { return };
        let mut d = IniDoc::open(IniKind::Map, &dir.join("Map.ini")).unwrap();
        let orig = d.ini.to_bytes();
        d.add_zone(68, &[("MapType", "F")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(69));
        let now = d.ini.to_bytes();
        let (a, b) = (lines(&orig), lines(&now));
        let differing: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
        assert_eq!(differing.len(), 1, "only the MapCount line changes, the zone is appended: {differing:?}");
        let at = differing[0];
        assert_eq!(b[at], a[at].iter().map(|c| if *c == b'8' { b'9' } else { *c }).collect::<Vec<u8>>(), "68 became 69");
        assert_eq!(a[at], b"MapCount\t  = 68\r\n".to_vec());
        assert!(now.ends_with(b"[SET_ZONE_68]\r\nMapType\t = F\r\n"), "appended at the end");
        assert_eq!(d.ini.zone_ids().len(), 69);
        d.undo();
        assert_eq!(d.ini.to_bytes(), orig);
    }

    /// `MapWar.ini` counts war maps: zone `n` is map `n`, `MapCount` = highest id - 99. Id 109 gives 10.
    #[test]
    fn mapwar_mapcount_war_maps() {
        let raw = b"[BASE]\r\nMapCount = 2\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\n\r\n[SET_ZONE_100]\r\nMapType = D\r\n\r\n[SET_ZONE_101]\r\nMapType = D\r\n";
        let mut d = IniDoc::from_bytes(IniKind::War, Path::new("MapWar.ini"), raw.to_vec()).unwrap();
        assert_eq!(d.ini.needed_map_count(true), 2);
        assert!(d.add_zone(50, &[("MapType", "F")]).is_err(), "a war id starts at 100");
        d.add_zone(102, &[("MapType", "F")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(3));
        d.undo();
        d.add_zone(109, &[("MapType", "F")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(10), "id 109 gives MapCount 10");
        let Some(dir) = server_dir() else { return };
        let mut d = IniDoc::open(IniKind::War, &dir.join("MapWar.ini")).unwrap();
        assert_eq!(d.ini.map_count(), Some(9));
        assert_eq!(d.ini.needed_map_count(true), 9, "the file agrees with the rule");
        let orig = d.ini.to_bytes();
        d.add_zone(109, &[("MapType", "F")]).unwrap();
        assert_eq!(d.ini.map_count(), Some(10));
        assert_eq!(d.ini.zone_ids().last(), Some(&109));
        d.undo();
        assert_eq!(d.ini.to_bytes(), orig);
        // the main file's rule is the highest id + 1, and the original agrees
        let m = IniDoc::open(IniKind::Map, &dir.join("Map.ini")).unwrap();
        assert_eq!(m.ini.needed_map_count(false), 68);
        assert_eq!(m.ini.map_count(), Some(68));
    }

    /// What our parser does with a `Map.ini` that holds keys and sections we do not know (invented names; the page says this):
    /// they are read as ordinary keys and sections, kept byte for byte by every edit, never an error; only a file without
    /// `[BASE] MapCount` is refused by the editor; and a key we would write must be letters, digits and `_`.
    #[test]
    fn map_ini_with_extra_keys_is_read_or_refused_as_documented() {
        let raw: &[u8] = b"[BASE]\r\nMapCount = 3\r\nExtraBaseFlag = 1\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\n\r\n[SET_ZONE_1]\r\nMapType = F\r\nExtraZoneKey = 5, 6 // note\r\nOdd-Key = x\r\n\r\n[EXTRA_SECTION_A]\r\nWhatever = 1\r\n";
        let ini = MapIni::parse(raw);
        assert_eq!(ini.map_count(), Some(3));
        assert_eq!(ini.zone_ids(), vec![1]);
        let z = ini.zone(1).unwrap();
        assert_eq!(ini.value(z, "ExtraZoneKey").as_deref(), Some("5, 6"), "an unknown key is an ordinary key; the comment is not its value");
        assert_eq!(ini.value(z, "Odd-Key").as_deref(), Some("x"), "any name before the = is read as a key");
        assert_eq!(ini.value(ini.section("BASE").unwrap(), "ExtraBaseFlag").as_deref(), Some("1"));
        assert!(ini.section("EXTRA_SECTION_A").is_some());
        assert_eq!(ini.to_bytes(), raw, "reading and writing back without an edit gives the same bytes");
        // the editor opens it, an edit of a known key leaves every unknown line as it was, and so does undo
        let mut d = IniDoc::from_bytes(IniKind::Map, Path::new("Map.ini"), raw.to_vec()).unwrap();
        d.set_key(1, "MapType", "D").unwrap();
        let after = d.ini.to_bytes();
        let want = String::from_utf8(raw.to_vec()).unwrap().replacen("MapType = F\r\nExtraZoneKey", "MapType = D\r\nExtraZoneKey", 1).into_bytes();
        assert_eq!(after, want, "only the value of the edited key changed");
        d.set_key(1, "ExtraZoneKey", "7").unwrap();
        assert_eq!(d.own_value(1, "ExtraZoneKey").as_deref(), Some("7"));
        assert!(String::from_utf8_lossy(&d.ini.to_bytes()).contains("ExtraZoneKey = 7 // note"), "the value changed and the comment stayed");
        d.undo();
        d.undo();
        assert_eq!(d.ini.to_bytes(), raw);
        // a key we would write must be plain: the odd key is read but cannot be set
        assert!(d.set_key(1, "Odd-Key", "y").is_err());
        // the server data check reads the same file
        let dir = crate::testutil::TempDir::new("w18-extra");
        std::fs::write(dir.join("Map.ini"), raw).unwrap();
        let s = crate::server_data::ServerData::load(dir.path(), None);
        assert_eq!(s.loaded_ids().unwrap().into_iter().collect::<Vec<_>>(), vec![0, 1, 2]);
        assert!(!s.notes.iter().any(|n| n.starts_with("Map.ini")), "{:?}", s.notes);
        // a file without [BASE] MapCount is refused, with the reason
        let e = IniDoc::from_bytes(IniKind::Map, Path::new("Map.ini"), b"[EXTRA]\r\nMapCount = 3\r\n[SET_ZONE_1]\r\nMapType = F\r\n".to_vec()).err().unwrap();
        assert!(e.contains("MapCount"), "{e}");
    }
}
