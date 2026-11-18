//! Folder-level checks where the svmap folder, the client and the server's data folder must agree.
//! Everything here is read only. The per-map rules of `client_rules.rs` are not
//! repeated; these look at one map list with two or three sources, so each finding carries the map id it belongs to (or none).
//!
//! | code | severity | rule |
//! |---|---|---|
//! | `portal-dest-no-map` | error | destination map is not in `Map.ini`/`MapWar.ini`, or the client has no `.wld` for it |
//! | `mapini-no-svmap` | error (Afterglow: info) | an id below `MapCount` has no svmap in the folder |
//! | `svmap-not-loaded` | info | an svmap whose id neither `Map.ini` nor `MapWar.ini` loads |
//! | `mapini-rebirth-outside` | error | a rebirth position outside the target map or on a blocked cell |
//! | `map-no-client` | warning | a server map without `world/<id>.wld` |
//! | `map-no-name` | info | a map id without a sysmsg name (the client shows the number) |
//! | `npc-unknown-server` | error | an NPC (type, type id) that the server's `NpcQuest.SData` lacks |
//! | `npc-client-server-differ` | warning | the client's and the server's `NpcQuest.SData` records differ (names aside) |
//! | `gate-dest-invalid` | error | a gatekeeper destination on a missing map or outside it |
//! | `mob-unknown-server` | error | a monster id the Afterglow defs export lacks (only when an export is given) |
//! | `wld-ref-missing` | warning | the `.wld` names a model, texture, water, sky or sound file the client lacks |
//! | `dg-missing` | error | a dungeon map whose `.dg` is missing |
//! | `minimap-missing` | info | a field map without `interface/MiniMap/<id>.tga` |
//! | `obelisk-pos` | warning | an `Obelisk.ini` position outside its map or on a blocked cell |
//! | `server-data-note` | info | a server file that is missing or unreadable (its rules were skipped) |
//! | `tactics-zone-map` | info | a `TacticsZone.zon` area on a map that does not exist (only when the file parses to its last byte) |

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use shaiya_clientdata::text::map_name_msg;
use shaiya_clientdata::wld::{PlacedList, WldKind};
use shaiya_clientdata::SysMsg;
use shaiya_svmap::{Finding, SectionId, Severity, Svmap};

use crate::client_rules::ClientRules;
use crate::server_data::ServerData;
use crate::keys::TargetServer;

/// A finding with the map it belongs to (`None` for the folder as a whole).
#[derive(Clone, Debug)]
pub struct CrossFinding {
    pub map: Option<u32>,
    pub finding: Finding,
}

pub const RULE_CODES: &[&str] = &[
    "portal-dest-no-map",
    "mapini-no-svmap",
    "svmap-not-loaded",
    "mapini-rebirth-outside",
    "map-no-client",
    "map-no-name",
    "npc-unknown-server",
    "npc-client-server-differ",
    "gate-dest-invalid",
    "mob-unknown-server",
    "wld-ref-missing",
    "dg-missing",
    "minimap-missing",
    "obelisk-pos",
    "server-data-note",
    "tactics-zone-map",
];

/// One area of the client's `world/TacticsZone.zon`.
#[derive(Clone, Debug, PartialEq)]
pub struct TacticsZone {
    pub kind: u8,
    pub sub: u8,
    pub map: u16,
}

/// `world/TacticsZone.zon`: `i32` version, `i32` count, then per area `u8`, `u8` type, `u8` sub type, 4
/// corners of 3 `f32`, 2 `f32` (y range), `u16` map, `i32` length and the name bytes. `None` unless the areas end exactly at the
/// last byte (the layout is inferred; a file that does not fit it is not judged).
pub fn parse_tactics_zones(b: &[u8]) -> Option<Vec<TacticsZone>> {
    let i32_at = |o: usize| b.get(o..o + 4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let count = usize::try_from(i32_at(4)?).ok()?;
    if count > 100_000 {
        return None;
    }
    let mut o = 8;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let head = b.get(o..o + 3)?;
        let (kind, sub) = (head[1], head[2]);
        o += 3 + 48 + 8;
        let m = b.get(o..o + 2)?;
        let map = u16::from_le_bytes([m[0], m[1]]);
        let len = usize::try_from(i32_at(o + 2)?).ok()?;
        o += 6;
        b.get(o..o.checked_add(len)?)?; // the name (cp949), not used
        o += len;
        out.push(TacticsZone { kind, sub, map });
    }
    (o == b.len()).then_some(out)
}

/// What the rules read. Any source may be missing; a rule that needs a missing source does not run.
pub struct Inputs<'a> {
    pub maps: &'a BTreeMap<u32, Arc<Svmap>>,
    pub client: Option<&'a ClientRules>,
    pub server: Option<&'a ServerData>,
    pub target: TargetServer,
}

struct Out(Vec<CrossFinding>);

impl Out {
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, severity: Severity, map: Option<u32>, section: SectionId, index: Option<usize>, sub: Option<usize>, code: &'static str, message: String) {
        debug_assert!(RULE_CODES.contains(&code), "cross rule code {code} is missing from RULE_CODES");
        self.0.push(CrossFinding { map, finding: Finding { severity, section, index, sub, code, message } });
    }
}

fn fmt3(v: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])
}

/// Where a position falls on a map: `Ok(blocked)` inside, `Err(())` outside.
fn probe(m: &Svmap, x: f32, z: f32) -> Result<bool, ()> {
    match shaiya_svmap::validate::cell_of(m.size, x, z) {
        Some((cx, cz)) => Ok(m.blocked.get(cx, cz)),
        None => Err(()),
    }
}

/// Runs every cross rule that has its sources.
pub fn check(inp: &Inputs) -> Vec<CrossFinding> {
    use SectionId as S;
    use Severity::*;
    let mut o = Out(Vec::new());
    let client_ids: Option<BTreeSet<u32>> = inp.client.map(|c| c.source.map_ids().into_iter().collect());
    let loaded = inp.server.and_then(ServerData::loaded_ids);

    if let Some(s) = inp.server {
        for n in &s.notes {
            o.add(Info, None, S::Size, None, None, "server-data-note", n.clone());
        }
    }

    // ---- portal destinations against the server's map list and the client's .wld files
    if loaded.is_some() || client_ids.is_some() {
        for (id, m) in inp.maps {
            for (i, p) in m.portals.iter().enumerate() {
                if p.to_map == 200 {
                    continue; // the guild ranking has no file
                }
                let mut why = Vec::new();
                if loaded.as_ref().is_some_and(|l| !l.contains(&p.to_map)) {
                    why.push("the server does not load it (not in Map.ini or MapWar.ini)".to_string());
                }
                if client_ids.as_ref().is_some_and(|c| !c.contains(&p.to_map)) {
                    why.push(format!("the client has no world/{}.wld", p.to_map));
                }
                if !why.is_empty() {
                    o.add(Error, Some(*id), S::Portals, Some(i), None, "portal-dest-no-map", format!("destination map {}: {}", p.to_map, why.join("; ")));
                }
            }
        }
    }

    // ---- Map.ini / MapWar.ini against the folder
    if let Some(s) = inp.server {
        let (main, war) = s.ranges();
        let sev = if inp.target == TargetServer::Afterglow { Info } else { Error };
        for (range, file, count_note) in [(main, "Map.ini", ""), (war, "MapWar.ini", " (the war server)")] {
            let Some(range) = range else { continue };
            for id in range {
                if !inp.maps.contains_key(&id) {
                    let tail = if inp.target == TargetServer::Afterglow { "Afterglow skips an id without a map file" } else { "the original server does not start" };
                    o.add(sev, None, S::Size, None, None, "mapini-no-svmap", format!("{file} loads map {id}{count_note} but the folder has no {id}.svmap; {tail}"));
                }
            }
        }
        if let Some(l) = &loaded {
            for id in inp.maps.keys() {
                if !l.contains(id) {
                    o.add(Info, Some(*id), S::Size, None, None, "svmap-not-loaded", format!("neither Map.ini nor MapWar.ini loads map {id}"));
                }
            }
        }
        // rebirth positions
        for (ini, file) in [(&s.map_ini, "Map.ini"), (&s.map_war, "MapWar.ini")] {
            let Some(ini) = ini else { continue };
            for zone in ini.zone_ids() {
                for r in ini.rebirths(zone) {
                    let Some(target) = u32::try_from(r.map).ok().and_then(|t| inp.maps.get(&t).map(|m| (t, m))) else { continue };
                    let owner = inp.maps.contains_key(&zone).then_some(zone);
                    match probe(target.1, r.pos[0], r.pos[2]) {
                        Err(()) => o.add(Error, owner, S::Size, None, None, "mapini-rebirth-outside", format!("{file} zone {zone} {}: {} is outside map {} (size {})", r.key, fmt3(r.pos), target.0, target.1.size)),
                        Ok(true) => o.add(Error, owner, S::Size, None, None, "mapini-rebirth-outside", format!("{file} zone {zone} {}: {} on map {} is a blocked cell", r.key, fmt3(r.pos), target.0)),
                        Ok(false) => {}
                    }
                }
            }
        }
    }

    // ---- the server's maps against the client
    let server_ids: BTreeSet<u32> = loaded.clone().unwrap_or_else(|| inp.maps.keys().copied().collect());
    if let (Some(c), Some(cids)) = (inp.client, &client_ids) {
        let sys = c.source.sysmsg().ok();
        for id in &server_ids {
            let owner = inp.maps.contains_key(id).then_some(*id);
            if !cids.contains(id) {
                o.add(Warning, owner, S::Size, None, None, "map-no-client", format!("map {id} has no world/{id}.wld in the client; players see nothing or crash"));
            }
            if !has_name(*id, sys.as_ref()) {
                o.add(Info, owner, S::Size, None, None, "map-no-name", format!("map {id} has no name in sysmsg; the client shows the number"));
            }
        }
    }

    // ---- NPCs against the server's NpcQuest.SData
    if let Some(sn) = inp.server.and_then(|s| s.npcs.as_ref()) {
        for (id, m) in inp.maps {
            for (i, n) in m.npcs.iter().enumerate() {
                if n.kind == 0 || n.kind > 13 {
                    continue;
                }
                match sn.get(n.kind, n.type_id) {
                    None => {
                        let have = sn.lists.get(n.kind as usize - 1).map_or(0, Vec::len);
                        o.add(Error, Some(*id), S::Npcs, Some(i), None, "npc-unknown-server", format!("NPC type {} id {} is not in the server's NpcQuest.SData (that type has {have} records, ids start at 1)", n.kind, n.type_id));
                    }
                    Some(srv) => {
                        let Some(cl) = inp.client.and_then(|c| c.npcs.as_ref()).and_then(|nq| nq.get(n.kind, n.type_id)) else { continue };
                        let mut diff: Vec<String> = Vec::new();
                        if srv.model != cl.model {
                            diff.push(format!("model {} / {}", srv.model, cl.model));
                        }
                        if srv.move_distance != cl.move_distance {
                            diff.push(format!("move distance {} / {}", srv.move_distance, cl.move_distance));
                        }
                        if srv.move_speed != cl.move_speed {
                            diff.push(format!("move speed {} / {}", srv.move_speed, cl.move_speed));
                        }
                        if srv.faction != cl.faction {
                            diff.push(format!("faction {} / {}", srv.faction, cl.faction));
                        }
                        let gate = |a: &shaiya_clientdata::sdata::Gate, b: &shaiya_clientdata::sdata::Gate| a.map == b.map && a.pos == b.pos && a.cost == b.cost;
                        if srv.gates.len() != cl.gates.len() || !srv.gates.iter().zip(&cl.gates).all(|(a, b)| gate(a, b)) {
                            diff.push("gate destinations or costs".to_string());
                        }
                        if !diff.is_empty() {
                            o.add(Warning, Some(*id), S::Npcs, Some(i), None, "npc-client-server-differ", format!("NPC type {} id {} differs between server and client (server / client): {}", n.kind, n.type_id, diff.join(", ")));
                        }
                    }
                }
            }
        }
    }

    // ---- gatekeeper destinations
    let mut sources: Vec<(&str, &shaiya_clientdata::NpcQuest)> = Vec::new();
    if let Some(n) = inp.server.and_then(|s| s.npcs.as_ref()) {
        sources.push(("server", n));
    }
    if let Some(n) = inp.client.and_then(|c| c.npcs.as_ref()) {
        sources.push(("client", n));
    }
    for (label, nq) in sources {
        for (k, npc) in nq.lists.get(1).into_iter().flatten().enumerate() {
            for (g, gate) in npc.gates.iter().enumerate() {
                if gate.map == 0 && gate.pos == [0.0; 3] && gate.name.trim().is_empty() {
                    continue; // an unused slot
                }
                let id = u32::from(gate.map);
                let what = format!("{label} gatekeeper {} ({}) gate {} ({}) leads to map {id}", k + 1, npc.name.trim(), g + 1, gate.name.trim());
                match inp.maps.get(&id) {
                    Some(m) => {
                        if probe(m, gate.pos[0], gate.pos[2]).is_err() {
                            o.add(Error, None, S::Size, None, None, "gate-dest-invalid", format!("{what}: {} is outside the map (size {})", fmt3(gate.pos), m.size));
                        }
                    }
                    None => {
                        let known = loaded.as_ref().is_some_and(|l| l.contains(&id)) || client_ids.as_ref().is_some_and(|c| c.contains(&id));
                        if !known {
                            o.add(Error, None, S::Size, None, None, "gate-dest-invalid", format!("{what}: that map does not exist"));
                        }
                    }
                }
            }
        }
    }

    // ---- monsters against the Afterglow defs export
    if let Some(ids) = inp.server.and_then(|s| s.mob_ids.as_ref()) {
        for (id, m) in inp.maps {
            for (i, a) in m.mob_areas.iter().enumerate() {
                for (k, e) in a.mobs.iter().enumerate() {
                    if !ids.contains(&e.mob) {
                        o.add(Error, Some(*id), S::MobAreas, Some(i), Some(k), "mob-unknown-server", format!("entry {k}: monster id {} is not in the server's monster table", e.mob));
                    }
                }
            }
        }
    }

    // ---- files the .wld names
    if let Some(c) = inp.client {
        for id in inp.maps.keys() {
            if client_ids.as_ref().is_some_and(|ids| !ids.contains(id)) {
                continue;
            }
            let Some(w) = c.wld(*id) else { continue };
            let src = &c.source;
            let mut missing: Vec<String> = Vec::new();
            let mut model = |dir: &str, list: &PlacedList| {
                for name in &list.names {
                    let name = name.trim();
                    if !name.is_empty() && !src.exists(&format!("Entity/{dir}/{name}")) {
                        missing.push(format!("Entity/{dir}/{name}"));
                    }
                }
            };
            model("Building", &w.building);
            model("Shape", &w.shape);
            model("Tree", &w.tree);
            for t in &w.textures {
                if !t.name.trim().is_empty() && src.find_texture("Terrain/Detail", &t.name).is_none() {
                    missing.push(format!("Terrain/Detail/{}", t.name.trim()));
                }
            }
            if !w.water.trim().is_empty() && !src.exists(&format!("Entity/Water/{}", w.water.trim())) {
                missing.push(format!("Entity/Water/{}", w.water.trim()));
            }
            for s in &w.sky {
                if !s.trim().is_empty() && src.find_texture("sky", s).is_none() {
                    missing.push(format!("sky/{}", s.trim()));
                }
            }
            for b in &w.sound.bgm_names {
                if !b.trim().is_empty() && !src.exists(&format!("sound/music/{}", b.trim())) {
                    missing.push(format!("sound/music/{}", b.trim()));
                }
            }
            for wav in &w.sound.wav_names {
                if !wav.trim().is_empty() && !src.exists(&format!("sound/back/{}", wav.trim())) {
                    missing.push(format!("sound/back/{}", wav.trim()));
                }
            }
            missing.sort_by_key(|p| p.to_ascii_lowercase());
            missing.dedup_by_key(|p| p.to_ascii_lowercase());
            for p in missing {
                o.add(Warning, Some(*id), S::Size, None, None, "wld-ref-missing", format!("world/{id}.wld names {p}, which the client does not have"));
            }
            match w.kind {
                WldKind::Dungeon => {
                    let name = w.dg.trim();
                    let file = if name.to_ascii_lowercase().ends_with(".dg") { name.to_string() } else { format!("{name}.dg") };
                    if name.is_empty() || !src.exists(&format!("world/dungeon/{file}")) {
                        o.add(Error, Some(*id), S::Size, None, None, "dg-missing", format!("dungeon map {id}: world/dungeon/{file} is missing, so the map cannot load"));
                    }
                }
                WldKind::Field => {
                    if src.find_texture("interface/MiniMap", &format!("{id}.tga")).is_none() {
                        o.add(Info, Some(*id), S::Size, None, None, "minimap-missing", format!("field map {id} has no interface/MiniMap/{id}.tga; the client shows no minimap picture"));
                    }
                }
            }
        }
    }

    // ---- obelisks
    if let Some(obs) = inp.server.and_then(|s| s.obelisks.as_ref()) {
        for ob in obs {
            let Some(m) = inp.maps.get(&ob.zone) else { continue };
            let what = format!("Obelisk.ini zone {} obelisk {}: {}", ob.zone, ob.index, fmt3(ob.pos));
            match probe(m, ob.pos[0], ob.pos[2]) {
                Err(()) => o.add(Warning, Some(ob.zone), S::Size, None, None, "obelisk-pos", format!("{what} is outside the map (size {})", m.size)),
                Ok(true) => o.add(Warning, Some(ob.zone), S::Size, None, None, "obelisk-pos", format!("{what} is on a blocked cell")),
                Ok(false) => {}
            }
        }
    }

    // ---- TacticsZone.zon areas (type 100 is the file's placeholder area on map 10000)
    if let Some(zones) = inp.client.and_then(|c| c.source.read("world/TacticsZone.zon").ok()).and_then(|b| parse_tactics_zones(&b)) {
        for (k, z) in zones.iter().enumerate() {
            let id = u32::from(z.map);
            let exists = inp.maps.contains_key(&id) || loaded.as_ref().is_some_and(|l| l.contains(&id)) || client_ids.as_ref().is_some_and(|c| c.contains(&id));
            if z.kind != 100 && !exists {
                o.add(Info, None, S::Size, None, None, "tactics-zone-map", format!("world/TacticsZone.zon area {k} (type {}, sub type {}) is on map {id}, which does not exist", z.kind, z.sub));
            }
        }
    }

    o.0.sort_by(|a, b| b.finding.severity.cmp(&a.finding.severity).then(a.finding.code.cmp(b.finding.code)).then(a.map.cmp(&b.map)).then(a.finding.index.cmp(&b.finding.index)).then(a.finding.sub.cmp(&b.finding.sub)));
    o.0
}

fn has_name(id: u32, sys: Option<&SysMsg>) -> bool {
    map_name_msg(id).and_then(|m| sys.and_then(|s| s.get(m))).is_some_and(|t| !t.trim().is_empty())
}

/// `(errors, warnings, info)` over the findings.
pub fn tally(fs: &[CrossFinding]) -> (usize, usize, usize) {
    let mut t = (0, 0, 0);
    for f in fs {
        match f.finding.severity {
            Severity::Error => t.0 += 1,
            Severity::Warning => t.1 += 1,
            Severity::Info => t.2 += 1,
        }
    }
    t
}

/// The folder-level line of the CLI and the Problems tab: `map 12 <section> #i: message`.
pub fn line(f: &CrossFinding) -> String {
    let at = match (f.finding.index, f.finding.sub) {
        (Some(i), Some(k)) => format!(" #{i}.{k}"),
        (Some(i), None) => format!(" #{i}"),
        _ => String::new(),
    };
    let sect = if f.map.is_some() && f.finding.index.is_some() { format!(" {}", f.finding.section.label()) } else { String::new() };
    let map = f.map.map_or("folder".to_string(), |m| format!("map {m}"));
    format!("{map}{sect}{at} [{}]: {}", f.finding.code, f.finding.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapini::MapIni;
    use shaiya_svmap::{MobArea, MobEntry, NpcEntry, NpcPoint, Portal};

    fn map(size: u32) -> Svmap {
        Svmap::new(size)
    }

    fn portal(to_map: u32, to: [f32; 3]) -> Portal {
        Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map, to_pos: to }
    }

    fn codes(fs: &[CrossFinding]) -> Vec<&'static str> {
        fs.iter().map(|f| f.finding.code).collect()
    }

    fn count(fs: &[CrossFinding], code: &str) -> usize {
        fs.iter().filter(|f| f.finding.code == code).count()
    }

    fn server(map_ini: &str, war: Option<&str>) -> ServerData {
        ServerData { map_ini: Some(MapIni::parse(map_ini.as_bytes())), map_war: war.map(|w| MapIni::parse(w.as_bytes())), ..Default::default() }
    }

    fn run(maps: &BTreeMap<u32, Arc<Svmap>>, s: &ServerData, target: TargetServer) -> Vec<CrossFinding> {
        check(&Inputs { maps, client: None, server: Some(s), target })
    }

    /// Each rule that needs no client fires once on a hand-made case and is quiet on its fixed twin.
    #[test]
    fn cross_rules_fixtures() {
        // twin A: broken
        let mut bad: BTreeMap<u32, Arc<Svmap>> = BTreeMap::new();
        let mut m0 = map(64);
        m0.portals.push(portal(3, [9.0, 0.0, 9.0])); // map 3 is not loaded
        m0.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [4.0, 2.0, 4.0], mobs: vec![MobEntry { mob: 99, count: 1 }] });
        m0.npcs.push(NpcEntry { kind: 1, type_id: 5, points: vec![NpcPoint { pos: [3.0, 0.0, 3.0], dir: 0.0 }] });
        bad.insert(0, Arc::new(m0));
        bad.insert(7, Arc::new(map(64))); // loaded by nobody
        let ini = "[BASE]\r\nMapCount = 2\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\n[SET_ZONE_0]\r\nRebirthMapPos = 0, 500, 0, 5\r\n[SET_ZONE_1]\r\nRebirthMapPos = 0, 10, 0, 10\r\n";
        let mut s = server(ini, Some("[BASE]\r\nMapCount = 1\r\n"));
        s.mob_ids = Some([0u32, 1].into_iter().collect());
        s.obelisks = Some(vec![crate::server_data::ObeliskPos { zone: 0, index: 1, pos: [500.0, 0.0, 5.0] }]);
        s.notes.push("Obelisk.ini: not found; the obelisk rule is skipped".to_string());
        let fs = run(&bad, &s, TargetServer::Original);
        assert_eq!(count(&fs, "portal-dest-no-map"), 1, "{:?}", codes(&fs));
        assert_eq!(count(&fs, "mapini-no-svmap"), 2, "map 1 and the war map 100 have no svmap");
        assert!(fs.iter().filter(|f| f.finding.code == "mapini-no-svmap").all(|f| f.finding.severity == Severity::Error));
        assert_eq!(count(&fs, "svmap-not-loaded"), 1);
        assert_eq!(count(&fs, "mapini-rebirth-outside"), 1, "zone 0 is outside; zone 1 sits at (10, 10)");
        assert_eq!(count(&fs, "mob-unknown-server"), 1);
        assert_eq!(count(&fs, "obelisk-pos"), 1);
        assert_eq!(count(&fs, "server-data-note"), 1);
        // the Afterglow target makes a missing svmap an info
        let fs = run(&bad, &s, TargetServer::Afterglow);
        assert!(fs.iter().filter(|f| f.finding.code == "mapini-no-svmap").all(|f| f.finding.severity == Severity::Info));
        // twin B: fixed
        let mut good: BTreeMap<u32, Arc<Svmap>> = BTreeMap::new();
        let mut g0 = map(64);
        g0.portals.push(portal(1, [9.0, 0.0, 9.0]));
        g0.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [4.0, 2.0, 4.0], mobs: vec![MobEntry { mob: 1, count: 1 }] });
        good.insert(0, Arc::new(g0));
        good.insert(1, Arc::new(map(64)));
        good.insert(100, Arc::new(map(64)));
        let mut s = server(&ini.replace("0, 500, 0, 5", "0, 5, 0, 5"), Some("[BASE]\r\nMapCount = 1\r\n"));
        s.mob_ids = Some([0u32, 1].into_iter().collect());
        s.obelisks = Some(vec![crate::server_data::ObeliskPos { zone: 0, index: 1, pos: [5.0, 0.0, 5.0] }]);
        let fs = run(&good, &s, TargetServer::Original);
        assert!(fs.is_empty(), "{:?}", fs.iter().map(line).collect::<Vec<_>>());
    }

    #[test]
    fn rebirth_on_a_blocked_cell_and_ignored_values() {
        let mut maps: BTreeMap<u32, Arc<Svmap>> = BTreeMap::new();
        let mut m = map(64);
        m.blocked.set(10, 10, true);
        maps.insert(0, Arc::new(m));
        let ini = "[BASE]\r\nMapCount = 1\r\n[SET_ZONE_0]\r\nRebirthMapPos = 0, 10.5, 0, 10.5\r\nRebirthMapPos1 = -1, 0, 0, 0\r\nRebirthMapPos2 = 0, 0, 0, 0\r\n";
        let fs = run(&maps, &server(ini, None), TargetServer::Original);
        assert_eq!(codes(&fs), vec!["mapini-rebirth-outside"]);
        assert!(fs[0].finding.message.contains("blocked"));
    }

    #[test]
    fn line_text_names_the_map() {
        let f = CrossFinding { map: Some(4), finding: Finding { severity: Severity::Error, section: SectionId::Portals, index: Some(2), sub: None, code: "portal-dest-no-map", message: "m".into() } };
        assert_eq!(line(&f), "map 4 portals #2 [portal-dest-no-map]: m");
        let g = CrossFinding { map: None, finding: Finding { severity: Severity::Info, section: SectionId::Size, index: None, sub: None, code: "server-data-note", message: "n".into() } };
        assert_eq!(line(&g), "folder [server-data-note]: n");
        assert_eq!(tally(&[f, g]), (1, 0, 1));
    }

    fn originals() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let maps = std::path::PathBuf::from(std::env::var_os("SVMAP_ORIGINALS").or_else(|| {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            None
        })?);
        let client = std::path::PathBuf::from(std::env::var_os("SHAIYA_CLIENT").or_else(|| {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            None
        })?);
        (maps.is_dir() && client.is_dir()).then_some((maps, client))
    }

    fn load_originals(dir: &std::path::Path) -> BTreeMap<u32, Arc<Svmap>> {
        let mut maps = BTreeMap::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                if p.extension().is_some_and(|x| x == "svmap") {
                    maps.insert(id, Arc::new(shaiya_svmap::read(&std::fs::read(&p).unwrap()).unwrap()));
                }
            }
        }
        maps
    }

    /// Gatekeeper 1 of the client's NpcQuest.SData has three gates with a map and a position, and the server's copy agrees.
    #[test]
    fn gate_dest_parsed() {
        let Some((m, c)) = originals() else { return };
        let rules = ClientRules::open(&c).unwrap();
        let g = &rules.npcs.as_ref().unwrap().lists[1];
        assert_eq!(g.len(), 61);
        assert_eq!(g[0].gates.len(), 3);
        assert_eq!((g[0].gates[0].map, g[0].gates[0].pos, g[0].gates[0].cost), (19, [586.25, 11.4, 1902.14], 8600));
        assert_eq!((g[0].gates[1].map, g[0].gates[2].map), (28, 35));
        assert!(g.iter().all(|n| n.gates.len() == 3), "every gatekeeper has three gates");
        assert!(rules.npcs.as_ref().unwrap().lists.iter().enumerate().all(|(k, l)| k == 1 || l.iter().all(|n| n.gates.is_empty())), "only type 2 has gates");
        let s = ServerData::load(m.parent().unwrap(), None);
        let sg = &s.npcs.as_ref().unwrap().lists[1];
        assert_eq!((sg[0].gates[0].map, sg[0].gates[0].pos, sg[0].gates[0].cost), (19, [586.25, 11.4, 1902.14], 8600), "the server copy shares the layout");
    }

    /// The new rules on the 77 originals with the client and the server's data folder. No error fires; the counts per code are
    /// the ones measured on the author's data.
    #[test]
    fn cross_checks_originals() {
        let Some((m, c)) = originals() else { return };
        let maps = load_originals(&m);
        assert_eq!(maps.len(), 77);
        let rules = ClientRules::open(&c).unwrap();
        let s = ServerData::load(m.parent().unwrap(), None);
        assert!(s.notes.is_empty(), "{:?}", s.notes);
        let fs = check(&Inputs { maps: &maps, client: Some(&rules), server: Some(&s), target: TargetServer::Original });
        let mut by: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for f in &fs {
            *by.entry((f.finding.severity.label(), f.finding.code)).or_default() += 1;
        }
        eprintln!("cross rules on the originals: {by:?}");
        assert_eq!(tally(&fs).0, 0, "no error on the originals: {:?}", fs.iter().filter(|f| f.finding.severity == Severity::Error).map(line).collect::<Vec<_>>());
        for code in ["portal-dest-no-map", "mapini-no-svmap", "svmap-not-loaded", "mapini-rebirth-outside", "map-no-client", "npc-unknown-server", "gate-dest-invalid", "dg-missing", "obelisk-pos", "server-data-note", "mob-unknown-server", "tactics-zone-map"] {
            assert_eq!(count(&fs, code), 0, "{code} fires on the originals: {:?}", fs.iter().filter(|f| f.finding.code == code).map(line).collect::<Vec<_>>());
        }
        assert_eq!(count(&fs, "map-no-name"), 13);
        assert_eq!(count(&fs, "minimap-missing"), 6);
        assert_eq!(count(&fs, "npc-client-server-differ"), 1, "gatekeeper 21: the gate costs differ");
        assert_eq!(count(&fs, "wld-ref-missing"), 9, "nine music files the client archive lacks");
        assert_eq!(tally(&fs), (0, 10, 19));
        // the Afterglow target gives the same result here (no id is missing a file)
        let ag = check(&Inputs { maps: &maps, client: Some(&rules), server: Some(&s), target: TargetServer::Afterglow });
        assert_eq!(tally(&ag), (0, 10, 19));
        // dropping a map the server loads: the original server will not start (error); Afterglow only notes it
        let mut less = maps.clone();
        less.remove(&30);
        let fs = check(&Inputs { maps: &less, client: Some(&rules), server: Some(&s), target: TargetServer::Original });
        assert_eq!(fs.iter().filter(|f| f.finding.code == "mapini-no-svmap").map(|f| f.finding.severity).collect::<Vec<_>>(), vec![Severity::Error]);
        assert!(count(&fs, "portal-dest-no-map") == 0, "a portal to a map that only lacks its svmap is not a portal-dest-no-map");
    }

    fn name256(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(256, 0);
        v
    }

    /// A tiny `.wld`: the given head, then zeros (every later count, name and number is 0) until it parses to its last byte.
    fn tiny_wld(head: Vec<u8>) -> Vec<u8> {
        for n in 0..4096 {
            let mut b = head.clone();
            b.extend(std::iter::repeat_n(0u8, n));
            if shaiya_clientdata::Wld::parse(&b, "tiny").is_ok() {
                return b;
            }
        }
        panic!("no zero tail makes a parsable .wld");
    }

    fn field_wld(water: &str, building: &str) -> Vec<u8> {
        let mut h = b"FLD\0".to_vec();
        h.extend(2i32.to_le_bytes());
        h.extend([0u8; 4 * 2 + 4]); // 2 x 2 corners: heights, texture indices
        h.extend(0i32.to_le_bytes()); // no terrain textures
        h.extend(name256(water));
        h.extend(1i32.to_le_bytes()); // one building name, no placements
        h.extend(name256(building));
        h.extend(0i32.to_le_bytes());
        tiny_wld(h)
    }

    fn dungeon_wld(dg: &str) -> Vec<u8> {
        let mut h = b"DUN\0".to_vec();
        h.extend(name256(dg));
        tiny_wld(h)
    }

    fn put(dir: &std::path::Path, path: &str, bytes: &[u8]) {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn sysmsg_with(ids: &[u32]) -> Vec<u8> {
        let mut text = String::from("\u{feff}");
        for id in ids {
            text.push_str(&format!("{id} \"name\"\r\n"));
        }
        text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    /// A `TacticsZone.zon` with one area per (type, map), each with a 3-byte name.
    fn zon(areas: &[(u8, u16)]) -> Vec<u8> {
        let mut b = 5i32.to_le_bytes().to_vec();
        b.extend((areas.len() as i32).to_le_bytes());
        for (kind, map) in areas {
            b.extend([0, *kind, 0]);
            b.extend([0u8; 56]);
            b.extend(map.to_le_bytes());
            b.extend(3i32.to_le_bytes());
            b.extend(b"abc");
        }
        b
    }

    /// The layout reads the hand-made file to its end and refuses one byte more or less; the client's file has 35 areas.
    #[test]
    fn tactics_zones_parse_to_the_last_byte() {
        let b = zon(&[(1, 45), (100, 10000)]);
        let z = parse_tactics_zones(&b).unwrap();
        assert_eq!(z, vec![TacticsZone { kind: 1, sub: 0, map: 45 }, TacticsZone { kind: 100, sub: 0, map: 10000 }]);
        assert!(parse_tactics_zones(&b[..b.len() - 1]).is_none());
        let mut more = b.clone();
        more.push(0);
        assert!(parse_tactics_zones(&more).is_none());
        assert!(parse_tactics_zones(b"").is_none() && parse_tactics_zones(&[0, 0, 0, 0, 255, 255, 255, 255]).is_none());
        let Some(c) = std::env::var_os("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let src = shaiya_clientdata::ClientSource::open(std::path::Path::new(&c)).unwrap();
        let z = parse_tactics_zones(&src.read("world/TacticsZone.zon").unwrap()).expect("the client's TacticsZone.zon parses to its last byte");
        assert_eq!(z.len(), 35);
        assert_eq!(z.iter().filter(|a| a.kind == 1).map(|a| a.map).collect::<BTreeSet<_>>(), [45, 46, 47].into_iter().collect(), "the safe zones");
        assert_eq!(z.iter().filter(|a| a.kind == 100).map(|a| a.map).collect::<Vec<_>>(), vec![10000]);
    }

    /// The client rules and the NPC rules on a hand-made client folder: each fires on the broken case, none on the fixed twin.
    #[test]
    fn cross_rules_client_fixtures() {
        use shaiya_clientdata::sdata::{Gate, Npc};
        use shaiya_clientdata::NpcQuest;
        let npc = |id: i16, model: i32| Npc { kind: 1, type_id: id, model, move_distance: 0, move_speed: 0, faction: 0, name: format!("n{id}"), welcome: String::new(), gates: Vec::new() };
        let gatekeeper = |map: u16, pos: [f32; 3]| Npc { kind: 2, type_id: 1, model: 9, move_distance: 0, move_speed: 0, faction: 0, name: "Gate".into(), welcome: String::new(), gates: vec![Gate { map, pos, name: "Town".into(), cost: 10 }] };
        let table = |kind1: Vec<Npc>, gate: Npc| {
            let mut lists = vec![Vec::new(); 13];
            lists[0] = kind1;
            lists[1] = vec![gate];
            NpcQuest { lists, quests_offset: 0 }
        };
        let mut maps: BTreeMap<u32, Arc<Svmap>> = BTreeMap::new();
        let mut m5 = map(64);
        for id in 1..=3 {
            m5.npcs.push(NpcEntry { kind: 1, type_id: id, points: vec![NpcPoint { pos: [3.0, 0.0, 3.0], dir: 0.0 }] });
        }
        maps.insert(5, Arc::new(m5));
        maps.insert(6, Arc::new(map(64)));
        maps.insert(9, Arc::new(map(64)));
        for fixed in [false, true] {
            let dir = crate::testutil::TempDir::new("cross-client");
            put(dir.path(), "world/5.wld", &dungeon_wld("ghost"));
            put(dir.path(), "world/6.wld", &field_wld("nowater.wtr", "Gone.smod"));
            put(dir.path(), "world/TacticsZone.zon", &zon(&[(100, 10000), (1, if fixed { 6 } else { 77 })]));
            if fixed {
                put(dir.path(), "world/9.wld", &dungeon_wld("ghost"));
                put(dir.path(), "world/dungeon/ghost.dg", b"x");
                put(dir.path(), "Entity/Water/nowater.wtr", b"x");
                put(dir.path(), "Entity/Building/Gone.smod", b"x");
                put(dir.path(), "interface/MiniMap/6.tga", b"x");
                put(dir.path(), "sysmsg-uni.txt", &sysmsg_with(&[5072, 5096]));
            }
            let mut rules = ClientRules::open(dir.path()).unwrap();
            let client = vec![npc(1, 1), npc(2, 2), npc(3, 3)];
            let (srv, sgate, cgate) = if fixed {
                (client.clone(), gatekeeper(9, [5.0, 0.0, 5.0]), gatekeeper(5, [5.0, 0.0, 5.0]))
            } else {
                (vec![npc(1, 1), npc(2, 22)], gatekeeper(31, [5.0, 0.0, 5.0]), gatekeeper(5, [500.0, 0.0, 5.0]))
            };
            rules.npcs = Some(table(client, cgate));
            let s = ServerData { npcs: Some(table(srv, sgate)), ..Default::default() };
            let fs = check(&Inputs { maps: &maps, client: Some(&rules), server: Some(&s), target: TargetServer::Original });
            let n = |code| count(&fs, code);
            let all = || fs.iter().map(line).collect::<Vec<_>>();
            if fixed {
                assert!(fs.is_empty(), "{:?}", all());
                continue;
            }
            assert_eq!(n("map-no-client"), 1, "{:?}", all());
            assert_eq!(n("map-no-name"), 3, "no sysmsg: every map shows its number");
            assert_eq!(n("dg-missing"), 1);
            assert_eq!(n("minimap-missing"), 1);
            assert_eq!(n("wld-ref-missing"), 2, "the water and the building: {:?}", all());
            assert_eq!(n("npc-unknown-server"), 1, "type 1 id 3 is not in the server table");
            assert_eq!(n("npc-client-server-differ"), 1, "type 1 id 2: model 22 against 2");
            assert_eq!(n("gate-dest-invalid"), 2, "the server gate leads to map 31, the client gate lands outside map 5");
            assert_eq!(n("tactics-zone-map"), 1, "the area on map 77; the placeholder (type 100, map 10000) is skipped");
        }
    }
}
