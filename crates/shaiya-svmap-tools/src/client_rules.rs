//! Client-aware validation rules: a map checked against the client's own
//! data (`Monster.SData`, `NpcQuest.SData` and the map's `world/<id>.wld`). Findings use the same type as the file rules, so
//! the editor's Problems list and the CLI show them together.
//!
//! | code | severity | rule |
//! |---|---|---|
//! | `mob-unknown` | error | monster id >= the `Monster.SData` record count, or the record has no name |
//! | `npc-unknown` | error | NPC (type, type id) is not in `NpcQuest.SData` (type 0 excepted) |
//! | `size-mismatch` | error | svmap size differs from the `.wld` field size |
//! | `portal-far` | error | portal (x, z) is more than 40 units from the centre of the `.wld` portal box at the same index |
//! | `portal-count` / `portal-field` | warning | portal count, order or destination fields differ from the `.wld` |
//! | `npc-count` / `npc-differs` / `npc-not-in-client` | warning | NPC list differs from the `.wld` NPC list |
//! | `region-count` / `region-differs` | warning | regions differ from the `.wld` areas |
//! | `bind-count` / `bind-differs` | info | bind areas differ from the `.wld` zone groups |
//! | `wld-not-lossless` | info | the `.wld` does not write back byte for byte, so the editor keeps it read-only |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use shaiya_clientdata::wld::Wld;
use shaiya_clientdata::{AreaNames, ClientSource, Monsters, NpcQuest};
use shaiya_svmap::{Finding, SectionId, Severity, Svmap};

/// Distance within which the server accepts a portal step.
pub const PORTAL_USE_RANGE: f32 = 40.0;
const POS_TOL: f32 = 0.01;
const ANGLE_TOL: f32 = 0.001;

/// What the editor knows about where a map's `.wld` came from and whether it can write it back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WldState {
    /// The file was read from the staging folder, so it overrides the client's own copy.
    pub staged: bool,
    /// Why the file stays read-only: it does not write back byte for byte, or it does not parse.
    pub read_only: Option<String>,
}

#[derive(Clone)]
struct WldEntry {
    wld: Option<Arc<Wld>>,
    state: WldState,
}

pub struct ClientRules {
    pub source: ClientSource,
    pub monsters: Option<Monsters>,
    pub npcs: Option<NpcQuest>,
    wlds: Mutex<HashMap<u32, WldEntry>>,
    /// The staging folder (`client_out`): `<out>/data/world/<id>.wld` and `.txt` override the client's own files.
    staging: Mutex<Option<PathBuf>>,
    /// Client files that could not be read; the rules that need them are skipped.
    pub problems: Vec<String>,
}

impl ClientRules {
    /// Open the client folder (archive or extracted). Unreadable tables are listed in `problems`, not fatal.
    pub fn open(dir: &Path) -> Result<ClientRules, String> {
        let source = ClientSource::open(dir).map_err(|e| format!("cannot open the client folder: {e}"))?;
        let mut problems = Vec::new();
        let monsters = source.monsters().map_err(|e| problems.push(e.to_string())).ok();
        let npcs = source.npc_quest().map_err(|e| problems.push(e.to_string())).ok();
        Ok(ClientRules { source, monsters, npcs, wlds: Mutex::new(HashMap::new()), staging: Mutex::new(None), problems })
    }

    /// Sets (or clears) the staging folder; maps read again afterwards.
    pub fn set_staging(&self, out: Option<PathBuf>) {
        *self.staging.lock().unwrap_or_else(|p| p.into_inner()) = out;
        self.wlds.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    pub fn staging(&self) -> Option<PathBuf> {
        self.staging.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The bytes of a client file and whether the staged copy answered: `<out>/data/<path>` first, else the client's own.
    pub fn read_file(&self, path: &str) -> Result<(Vec<u8>, bool), shaiya_clientdata::Error> {
        if let Some(out) = self.staging() {
            let p = crate::client_files::staged_path(&out, path);
            if p.is_file() {
                if let Ok(b) = std::fs::read(&p) {
                    return Ok((b, true));
                }
            }
        }
        self.source.read(path).map(|b| (b, false))
    }

    fn load_wld(&self, map: u32) -> WldEntry {
        let path = format!("world/{map}.wld");
        let Ok((bytes, staged)) = self.read_file(&path) else { return WldEntry { wld: None, state: WldState::default() } };
        let wld = Wld::parse(&bytes, &path);
        let read_only = shaiya_clientdata::wld_raw::lossless_check(&bytes, &path).err();
        WldEntry { wld: wld.ok().map(Arc::new), state: WldState { staged, read_only } }
    }

    fn wld_entry(&self, map: u32) -> WldEntry {
        let mut cache = self.wlds.lock().unwrap_or_else(|p| p.into_inner());
        cache.entry(map).or_insert_with(|| self.load_wld(map)).clone()
    }

    /// The map's `.wld`, parsed once. A staged copy wins over the client's own.
    pub fn wld(&self, map: u32) -> Option<Arc<Wld>> {
        self.wld_entry(map).wld
    }

    /// Where the map's `.wld` came from and whether it can be written back.
    pub fn wld_state(&self, map: u32) -> WldState {
        self.wld_entry(map).state
    }

    /// `world/<map>.txt` area names (a staged copy wins); no file gives an empty list.
    pub fn area_names(&self, map: u32) -> Result<AreaNames, shaiya_clientdata::Error> {
        let path = format!("world/{map}.txt");
        match self.read_file(&path) {
            Ok((b, _)) => shaiya_clientdata::text::parse_area_names(&b, &path),
            Err(shaiya_clientdata::Error::NotFound(_)) => Ok(AreaNames::default()),
            Err(e) => Err(e),
        }
    }

    /// All client rules for `m`. `map` is the map id (from the file name); without it the `.wld` rules are skipped.
    pub fn check(&self, map: Option<u32>, m: &Svmap) -> Vec<Finding> {
        self.check_with(map, m, None)
    }

    /// [`check`](Self::check) against `edited` (the tab's own client model, with its unsaved edits) instead of the file the client
    /// holds. The read-only note still comes from the file.
    pub fn check_with(&self, map: Option<u32>, m: &Svmap, edited: Option<Arc<Wld>>) -> Vec<Finding> {
        let mut o = Vec::new();
        self.table_rules(m, &mut o);
        if let Some(id) = map {
            let mut e = self.wld_entry(id);
            if edited.is_some() {
                e.wld = edited;
            }
            match e.wld {
                Some(w) => {
                    wld_rules(m, &w, &mut o);
                    if let Some(why) = e.state.read_only {
                        o.push(f(Severity::Info, SectionId::Size, None, None, "wld-not-lossless", format!("world/{id}.wld does not write back byte for byte, so it stays read-only: {why}")));
                    }
                }
                None => o.push(f(Severity::Info, SectionId::Size, None, None, "client-no-wld", format!("the client has no readable world/{id}.wld; the .wld rules were skipped"))),
            }
        }
        o
    }

    fn table_rules(&self, m: &Svmap, o: &mut Vec<Finding>) {
        if let Some(mons) = &self.monsters {
            for (i, a) in m.mob_areas.iter().enumerate() {
                for (k, e) in a.mobs.iter().enumerate() {
                    match mons.get(e.mob) {
                        None => o.push(f(Severity::Error, SectionId::MobAreas, Some(i), Some(k), "mob-unknown", format!("entry {k}: monster id {} is not in Monster.SData ({} records)", e.mob, mons.records.len()))),
                        Some(r) if r.name.trim().is_empty() => o.push(f(Severity::Error, SectionId::MobAreas, Some(i), Some(k), "mob-unknown", format!("entry {k}: monster id {} has no name in Monster.SData", e.mob))),
                        Some(_) => {}
                    }
                }
            }
        }
        if let Some(nq) = &self.npcs {
            for (i, n) in m.npcs.iter().enumerate() {
                if n.kind == 0 || n.kind > 13 {
                    continue; // type 0 is a placeholder; above 13 the file rules already complain
                }
                if nq.get(n.kind, n.type_id).is_none() {
                    let have = nq.lists.get(n.kind as usize - 1).map_or(0, Vec::len);
                    o.push(f(Severity::Error, SectionId::Npcs, Some(i), None, "npc-unknown", format!("NPC type {} id {} is not in NpcQuest.SData (that type has {have} records, ids start at 1)", n.kind, n.type_id)));
                }
            }
        }
    }
}

/// Every rule code the client rules can report.
pub const CLIENT_RULE_CODES: &[&str] = &[
    "mob-unknown",
    "npc-unknown",
    "size-mismatch",
    "portal-far",
    "portal-count",
    "portal-field",
    "npc-count",
    "npc-not-in-client",
    "npc-differs",
    "region-count",
    "region-differs",
    "bind-count",
    "bind-differs",
    "client-no-wld",
    "wld-not-lossless",
];

fn f(severity: Severity, section: SectionId, index: Option<usize>, sub: Option<usize>, code: &'static str, message: String) -> Finding {
    debug_assert!(CLIENT_RULE_CODES.contains(&code), "client rule code {code} is missing from CLIENT_RULE_CODES");
    Finding { severity, section, index, sub, code, message }
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn near3(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| near(*x, y, POS_TOL))
}

fn fmt3(v: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])
}

fn wld_rules(m: &Svmap, w: &Wld, o: &mut Vec<Finding>) {
    use SectionId as S;
    use Severity::*;

    if let Some(size) = w.field_size() {
        if size != m.size {
            o.push(f(Error, S::Size, None, None, "size-mismatch", format!("svmap size {} but the .wld terrain is {size}", m.size)));
        }
    }

    // portals: same index, same fields; the position is the box centre (x, z)
    if m.portals.len() != w.portals.len() {
        o.push(f(Warning, S::Portals, None, None, "portal-count", format!("svmap has {} portals, the .wld has {} (the client sends the portal index, so order and count must agree)", m.portals.len(), w.portals.len())));
    }
    for (i, (p, c)) in m.portals.iter().zip(&w.portals).enumerate() {
        let at = c.svmap_pos();
        let dist = ((p.pos[0] - at[0]).powi(2) + (p.pos[2] - at[2]).powi(2)).sqrt();
        if dist > PORTAL_USE_RANGE {
            o.push(f(Error, S::Portals, Some(i), None, "portal-far", format!("portal is {dist:.1} units from the .wld portal box centre ({:.1}, {:.1}); the server refuses the step beyond {PORTAL_USE_RANGE}", at[0], at[2])));
        }
        let mut diff = |what: &str, ours: String, theirs: String| {
            o.push(f(Warning, S::Portals, Some(i), None, "portal-field", format!("{what} is {ours} here and {theirs} in the .wld")));
        };
        if p.country as i64 != c.country as i64 {
            diff("country", p.country.to_string(), c.country.to_string());
        }
        if p.level_min as i32 != c.level_min as i32 {
            diff("level min", p.level_min.to_string(), c.level_min.to_string());
        }
        if p.level_max as i32 != c.level_max as i32 {
            diff("level max", p.level_max.to_string(), c.level_max.to_string());
        }
        if p.to_map as i64 != c.dest_map as i64 {
            diff("destination map", p.to_map.to_string(), c.dest_map.to_string());
        }
        if !near3(p.to_pos, c.dest_pos) {
            diff("destination", fmt3(p.to_pos), fmt3(c.dest_pos));
        }
    }

    // NPCs: the .wld list is what the minimap and the quest marks use
    let key = |kind: i64, id: i64, x: f32, z: f32| (kind, id, x.to_bits(), z.to_bits());
    if m.npcs.len() == w.npcs.len() {
        for (i, (n, c)) in m.npcs.iter().zip(&w.npcs).enumerate() {
            let Some(first) = n.points.first() else { continue };
            let same = n.kind as i64 == c.kind as i64
                && n.type_id as i64 == c.type_id as i64
                && near3(first.pos, c.pos)
                && near(first.dir, c.angle, ANGLE_TOL)
                && n.points.len() == c.point_count()
                && n.points[1..].iter().zip(&c.extra).all(|(a, b)| near3(a.pos, *b));
            if !same {
                o.push(f(
                    Warning,
                    S::Npcs,
                    Some(i),
                    None,
                    "npc-differs",
                    format!(
                        "NPC differs from .wld NPC #{i}: here type {} id {} at {} dir {:.3} with {} points, .wld type {} id {} at {} dir {:.3} with {} points",
                        n.kind,
                        n.type_id,
                        fmt3(first.pos),
                        first.dir,
                        n.points.len(),
                        c.kind,
                        c.type_id,
                        fmt3(c.pos),
                        c.angle,
                        c.point_count()
                    ),
                ));
            }
        }
    } else {
        let mut extra: Vec<String> = Vec::new();
        let ours: std::collections::HashSet<_> = m.npcs.iter().filter_map(|n| n.points.first().map(|p| key(n.kind as i64, n.type_id as i64, p.pos[0], p.pos[2]))).collect();
        let theirs: std::collections::HashSet<_> = w.npcs.iter().map(|c| key(c.kind as i64, c.type_id as i64, c.pos[0], c.pos[2])).collect();
        for c in &w.npcs {
            if !ours.contains(&key(c.kind as i64, c.type_id as i64, c.pos[0], c.pos[2])) && extra.len() < 3 {
                extra.push(format!("type {} id {} at ({:.1}, {:.1})", c.kind, c.type_id, c.pos[0], c.pos[2]));
            }
        }
        let list = if extra.is_empty() { String::new() } else { format!("; only in the .wld: {}", extra.join(", ")) };
        o.push(f(Warning, S::Npcs, None, None, "npc-count", format!("svmap has {} NPCs, the .wld has {}{list}", m.npcs.len(), w.npcs.len())));
        for (i, n) in m.npcs.iter().enumerate() {
            let Some(p) = n.points.first() else { continue };
            if !theirs.contains(&key(n.kind as i64, n.type_id as i64, p.pos[0], p.pos[2])) {
                o.push(f(Warning, S::Npcs, Some(i), None, "npc-not-in-client", "NPC (type, type id, position) is not in the .wld NPC list".to_string()));
            }
        }
    }

    // regions = .wld named areas, same boxes in the same order; kind = the area mode
    if m.regions.len() != w.areas.len() {
        o.push(f(Warning, S::Regions, None, None, "region-count", format!("svmap has {} regions, the .wld has {} areas (names come from the .wld's world/<id>.txt by index)", m.regions.len(), w.areas.len())));
    }
    for (i, (r, a)) in m.regions.iter().zip(&w.areas).enumerate() {
        if !near3(r.min, a.min) || !near3(r.max, a.max) || r.kind as i64 != a.mode as i64 {
            o.push(f(Warning, S::Regions, Some(i), None, "region-differs", format!("region differs from .wld area #{i} ({}): box or kind (svmap {}, .wld mode {})", a.name, r.kind, a.mode)));
        }
    }

    // binds = .wld zone groups
    if m.binds.len() != w.zone_groups.len() {
        o.push(f(Info, S::Binds, None, None, "bind-count", format!("svmap has {} bind areas, the .wld has {} zone groups", m.binds.len(), w.zone_groups.len())));
    }
    for (i, (b, g)) in m.binds.iter().zip(&w.zone_groups).enumerate() {
        let same = b.boxes.len() == g.boxes.len()
            && b.boxes.iter().zip(&g.boxes).all(|(x, y)| near3(x.min, y.min) && near3(x.max, y.max) && b.country as i64 == y.country as i64 && b.attr as i64 == y.attr as i64);
        if !same {
            o.push(f(Info, S::Binds, Some(i), None, "bind-differs", format!("bind area differs from .wld zone group #{i} (boxes, country or attr)")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dirs() -> Option<(PathBuf, PathBuf)> {
        let c = PathBuf::from(std::env::var_os("SHAIYA_CLIENT").or_else(|| {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            None
        })?);
        let s = PathBuf::from(std::env::var_os("SVMAP_ORIGINALS").or_else(|| {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            None
        })?);
        (c.is_dir() && s.is_dir()).then_some((c, s))
    }

    fn load(dir: &Path, id: u32) -> Svmap {
        shaiya_svmap::read(&std::fs::read(dir.join(format!("{id}.svmap"))).unwrap()).unwrap()
    }

    fn count(fs: &[Finding], code: &str) -> usize {
        fs.iter().filter(|x| x.code == code).count()
    }

    #[test]
    fn originals_have_no_client_errors() {
        let Some((c, s)) = dirs() else { return };
        let rules = ClientRules::open(&c).unwrap();
        assert!(rules.problems.is_empty(), "{:?}", rules.problems);
        let mut by_code: std::collections::BTreeMap<(&str, &'static str), usize> = Default::default();
        for entry in std::fs::read_dir(&s).unwrap() {
            let p = entry.unwrap().path();
            let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            let m = load(&s, id);
            for x in rules.check(Some(id), &m) {
                assert_ne!(x.severity, Severity::Error, "map {id}: {} {}", x.code, x.message);
                *by_code.entry((x.severity.label(), x.code)).or_default() += 1;
            }
        }
        eprintln!("client rules on the originals: {by_code:?}");
    }

    #[test]
    fn each_rule_fires_on_a_bad_map() {
        let Some((c, s)) = dirs() else { return };
        let rules = ClientRules::open(&c).unwrap();
        let mut m = load(&s, 2);
        assert_eq!(count(&rules.check(Some(2), &m), "portal-far"), 0);
        // unknown monster, unknown NPC, wrong size, portal moved away
        m.mob_areas[0].mobs.push(shaiya_svmap::MobEntry { mob: 999_999, count: 1 });
        m.npcs[0].kind = 7;
        m.npcs[0].type_id = 60_000;
        m.size = if m.size == 2048 { 1024 } else { 2048 };
        m.portals[0].pos[0] += 100.0;
        m.portals[1].level_max = m.portals[1].level_max.wrapping_add(7);
        let fs = rules.check(Some(2), &m);
        assert_eq!(count(&fs, "mob-unknown"), 1);
        assert_eq!(count(&fs, "npc-unknown"), 1);
        assert_eq!(count(&fs, "size-mismatch"), 1);
        assert_eq!(count(&fs, "portal-far"), 1);
        assert!(count(&fs, "portal-field") >= 1);
        // dropping a portal and an NPC breaks the counts
        let mut m = load(&s, 2);
        m.portals.pop();
        m.npcs.pop();
        m.regions.pop();
        m.binds.pop();
        let fs = rules.check(Some(2), &m);
        assert_eq!(count(&fs, "portal-count"), 1);
        assert_eq!(count(&fs, "npc-count"), 1);
        assert_eq!(count(&fs, "region-count"), 1);
        assert_eq!(count(&fs, "bind-count"), 1);
        // an unknown map id skips the .wld rules with an info
        let fs = rules.check(Some(9999), &load(&s, 2));
        assert_eq!(count(&fs, "client-no-wld"), 1);
    }
}
