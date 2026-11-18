//! The server's data folder as an optional third source of the cross-file checks:
//! `Map.ini`, `MapWar.ini`, `NpcQuest.SData` and `Obelisk.ini` (zone ids and obelisk positions only), plus the monster ids of an
//! Afterglow defs export. A file that is missing or unreadable switches off only the rules that need it and leaves one note.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use shaiya_clientdata::{seed, NpcQuest};

use crate::mapini::MapIni;

/// First map id of the war server's maps: `MapWar.ini` zone `n` is map `100 + n`.
pub const WAR_BASE: u32 = 100;

/// One obelisk of `Obelisk.ini`.
#[derive(Clone, Debug, PartialEq)]
pub struct ObeliskPos {
    pub zone: u32,
    /// The number in `[Obelisk_K]`.
    pub index: u32,
    pub pos: [f32; 3],
}

#[derive(Default)]
pub struct ServerData {
    pub dir: PathBuf,
    pub map_ini: Option<MapIni>,
    pub map_war: Option<MapIni>,
    pub npcs: Option<NpcQuest>,
    pub obelisks: Option<Vec<ObeliskPos>>,
    /// The monster ids of an Afterglow defs export (`mobs.psv`); `None` when no export was given or readable.
    pub mob_ids: Option<BTreeSet<u32>>,
    /// One line per file that is missing or unreadable (and was asked for).
    pub notes: Vec<String>,
}

/// The server data folder: the setting when it is set, else the parent of the svmap folder when that parent holds `Map.ini`.
pub fn resolve_dir(setting: Option<&str>, svmap_folder: Option<&Path>) -> Option<PathBuf> {
    if let Some(s) = setting.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(s));
    }
    let parent = svmap_folder?.parent()?;
    parent.join("Map.ini").is_file().then(|| parent.to_path_buf())
}

/// The file with the monster rows for an Afterglow defs setting: a `mobs.psv` itself, or a folder that holds it (directly,
/// in `defs-export` or in `data/defs-export`).
pub fn afterglow_mobs_file(setting: &Path) -> Option<PathBuf> {
    if setting.is_file() {
        return Some(setting.to_path_buf());
    }
    ["mobs.psv", "defs-export/mobs.psv", "data/defs-export/mobs.psv"].iter().map(|p| setting.join(p)).find(|p| p.is_file())
}

impl ServerData {
    /// Reads what `dir` holds. `afterglow` is the defs export setting (None or empty: the monster check is skipped, no note).
    pub fn load(dir: &Path, afterglow: Option<&Path>) -> ServerData {
        let mut d = ServerData { dir: dir.to_path_buf(), ..Default::default() };
        d.map_ini = d.read_ini("Map.ini");
        d.map_war = d.read_ini("MapWar.ini");
        if d.map_ini.as_ref().is_some_and(|m| m.map_count().is_none()) {
            d.notes.push("Map.ini has no [BASE] MapCount: the map list rules are skipped".to_string());
            d.map_ini = None;
        }
        if d.map_war.as_ref().is_some_and(|m| m.map_count().is_none()) {
            d.notes.push("MapWar.ini has no [BASE] MapCount: its maps are not counted".to_string());
            d.map_war = None;
        }
        d.npcs = d.read_npcs();
        d.obelisks = match std::fs::read(dir.join("Obelisk.ini")) {
            Ok(b) => Some(parse_obelisks(&b)),
            Err(e) => {
                d.notes.push(format!("Obelisk.ini: {}; the obelisk rule is skipped", why(&e)));
                None
            }
        };
        if let Some(a) = afterglow {
            match afterglow_mobs_file(a) {
                None => d.notes.push(format!("{}: no mobs.psv found; the server monster rule is skipped", a.display())),
                Some(f) => match std::fs::read(&f) {
                    Ok(b) => d.mob_ids = Some(parse_mob_ids(&b)),
                    Err(e) => d.notes.push(format!("{}: {}; the server monster rule is skipped", f.display(), why(&e))),
                },
            }
        }
        d
    }

    fn read_ini(&mut self, name: &str) -> Option<MapIni> {
        match std::fs::read(self.dir.join(name)) {
            Ok(b) => Some(MapIni::parse(&b)),
            Err(e) => {
                self.notes.push(format!("{name}: {}; the rules that need it are skipped", why(&e)));
                None
            }
        }
    }

    fn read_npcs(&mut self) -> Option<NpcQuest> {
        let path = self.dir.join("NpcQuest.SData");
        let raw = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                self.notes.push(format!("NpcQuest.SData: {}; the server NPC rules are skipped", why(&e)));
                return None;
            }
        };
        let plain = if seed::is_encrypted(&raw) {
            match seed::decrypt(&raw, "NpcQuest.SData") {
                Ok(p) => p,
                Err(e) => {
                    self.notes.push(format!("NpcQuest.SData: {e}; the server NPC rules are skipped"));
                    return None;
                }
            }
        } else {
            raw
        };
        match NpcQuest::parse(&plain, "NpcQuest.SData") {
            Ok(n) => Some(n),
            Err(e) => {
                self.notes.push(format!("NpcQuest.SData: {e}; the server NPC rules are skipped"));
                None
            }
        }
    }

    /// The map ids the server loads: `Map.ini` ids `0..MapCount`, and the war server's `100..100 + MapCount` of `MapWar.ini`.
    /// `None` when neither file was read.
    pub fn loaded_ids(&self) -> Option<BTreeSet<u32>> {
        if self.map_ini.is_none() && self.map_war.is_none() {
            return None;
        }
        let mut s = BTreeSet::new();
        if let Some(n) = self.map_ini.as_ref().and_then(MapIni::map_count) {
            s.extend(0..n);
        }
        if let Some(n) = self.map_war.as_ref().and_then(MapIni::map_count) {
            s.extend(WAR_BASE..WAR_BASE.saturating_add(n));
        }
        Some(s)
    }

    /// The ids of the main server alone (`Map.ini`) and of the war server alone (`MapWar.ini`).
    pub fn ranges(&self) -> (Option<std::ops::Range<u32>>, Option<std::ops::Range<u32>>) {
        (
            self.map_ini.as_ref().and_then(MapIni::map_count).map(|n| 0..n),
            self.map_war.as_ref().and_then(MapIni::map_count).map(|n| WAR_BASE..WAR_BASE.saturating_add(n)),
        )
    }
}

fn why(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        "not found".to_string()
    } else {
        format!("cannot be read ({e})")
    }
}

/// Monster ids of `mobs.psv`: one row per line, `row|id|name|...`.
pub fn parse_mob_ids(bytes: &[u8]) -> BTreeSet<u32> {
    let text = String::from_utf8_lossy(bytes);
    text.lines().filter_map(|l| l.split('|').nth(1)?.trim().parse::<u32>().ok()).collect()
}

fn atof(s: &str) -> f32 {
    s.trim().parse::<f32>().unwrap_or(0.0)
}

fn atol(s: &str) -> u32 {
    s.trim().parse::<u32>().unwrap_or(0)
}

/// Zones and obelisks of `Obelisk.ini`, read as the server does: lines starting `//` or `--` are skipped, the rest is split at
/// ` , = ( ) ] [ { }` and blanks, heads are matched without
/// case. `[zone_N]` starts a zone, `[Obelisk_K] = (x, y, z), ...` adds an obelisk to it. `[Portal_` and `[bothmob_` leave the zone.
pub fn parse_obelisks(bytes: &[u8]) -> Vec<ObeliskPos> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut zone: Option<u32> = None;
    for line in text.lines() {
        let t = line.trim_start_matches([' ', '\t']);
        if !t.starts_with('[') {
            continue;
        }
        let toks: Vec<&str> = t.split([' ', ',', '=', '(', ')', ']', '[', '{', '}', '\t', '\r', '\n']).filter(|s| !s.is_empty()).collect();
        let Some(head) = toks.first() else { continue };
        let lower = head.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix("zone_") {
            zone = Some(atol(n));
        } else if let Some(k) = lower.strip_prefix("obelisk_") {
            if let Some(z) = zone {
                let at = |i: usize| toks.get(i).map_or(0.0, |s| atof(s));
                out.push(ObeliskPos { zone: z, index: atol(k), pos: [at(1), at(2), at(3)] });
            }
        } else if lower.starts_with("portal_") || lower.starts_with("bothmob_") {
            zone = None;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obelisk_ini_lines() {
        let src = "// note\r\n[Portal_x, (1, 2) ]\r\n[zone_0]\r\n    [Obelisk_001] = (155.10,39.43,190.38),(52, 252, 152),0\r\n        [Mob] = (155.17,39.44,180.20,1),(77, 69, 73),1\r\n  [OBELISK_002] = (1,2,3),(0,0,0),1\r\n-- x\r\n[zone_18]\r\n[Obelisk_001] = (5.5,6,7),(1,1,1),0\r\n[bothmob_1, (19, 5, 600, (19, 1001))] = (1,2,3),(4)\r\n[Obelisk_009] = (1,1,1),(1,1,1),0\r\n";
        let o = parse_obelisks(src.as_bytes());
        assert_eq!(o.len(), 3, "{o:?}");
        assert_eq!(o[0], ObeliskPos { zone: 0, index: 1, pos: [155.10, 39.43, 190.38] });
        assert_eq!(o[1].index, 2);
        assert_eq!(o[2], ObeliskPos { zone: 18, index: 1, pos: [5.5, 6.0, 7.0] }, "an obelisk after [bothmob_ belongs to no zone");
    }

    #[test]
    fn mob_rows_and_resolve() {
        let ids = parse_mob_ids(b"1|0|3F3F|0\n2|1|D0A1|38\n3|7|AA|1\nbad\n");
        assert_eq!(ids.iter().copied().collect::<Vec<_>>(), vec![0, 1, 7]);
        assert_eq!(resolve_dir(Some(" D:/srv "), None), Some(PathBuf::from("D:/srv")));
        assert_eq!(resolve_dir(Some(""), None), None);
        let dir = crate::testutil::TempDir::new("server-resolve");
        let maps = dir.join("Map");
        std::fs::create_dir_all(&maps).unwrap();
        assert_eq!(resolve_dir(None, Some(&maps)), None, "no Map.ini in the parent");
        std::fs::write(dir.join("Map.ini"), b"[BASE]\r\nMapCount = 2\r\n").unwrap();
        assert_eq!(resolve_dir(None, Some(&maps)), Some(dir.path().to_path_buf()));
        let d = ServerData::load(dir.path(), None);
        assert_eq!(d.loaded_ids().unwrap().into_iter().collect::<Vec<_>>(), vec![0, 1]);
        assert!(d.notes.iter().any(|n| n.starts_with("MapWar.ini: not found")), "{:?}", d.notes);
        assert!(d.notes.iter().any(|n| n.starts_with("NpcQuest.SData")));
        assert!(d.mob_ids.is_none() && !d.notes.iter().any(|n| n.contains("mobs.psv")), "no defs setting: no note");
        let d = ServerData::load(dir.path(), Some(Path::new("D:/does/not/exist")));
        assert!(d.notes.iter().any(|n| n.contains("no mobs.psv found")));
    }

    /// The shipped `Obelisk.ini`: map 0 has seven obelisks, maps 18 and 30 one each; the sample `[Portal_` line adds none.
    #[test]
    fn obelisk_ini_parsed() {
        let Some(maps) = std::env::var_os("SVMAP_ORIGINALS") else {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            return;
        };
        let dir = PathBuf::from(maps).parent().unwrap().to_path_buf();
        if !dir.join("Obelisk.ini").is_file() {
            eprintln!("skipped: no Obelisk.ini beside the map folder");
            return;
        }
        let d = ServerData::load(&dir, None);
        let o = d.obelisks.as_ref().unwrap();
        let per_zone = |z: u32| o.iter().filter(|x| x.zone == z).count();
        assert_eq!((o.len(), per_zone(0), per_zone(18), per_zone(30)), (9, 7, 1, 1));
        assert_eq!(o[0], ObeliskPos { zone: 0, index: 1, pos: [155.10, 39.43, 190.38] });
        assert!(d.notes.is_empty(), "{:?}", d.notes);
    }
}
