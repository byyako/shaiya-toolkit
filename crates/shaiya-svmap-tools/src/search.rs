//! Find in all maps, select similar, replace a monster across maps, and the portal link table.
//! Pure: no egui, no files. The caller hands in the maps (folder copies, open tabs live) and the names it knows.
//!
//! Query words (space separated, every word must match the same record):
//!
//! | word | matches |
//! |---|---|
//! | `thief` | the record's text (monster names, NPC type and name, destination map name, region name, bind country) or its map's name |
//! | `42` | a plain number: areas with monster id 42, NPCs with type id 42, portals to map 42 |
//! | `mob:42` | monster areas with monster id 42 |
//! | `npc:1/159` | NPCs of type 1 and type id 159 |
//! | `npctype:1` | NPCs of type 1 |
//! | `portal:18` | portals to map 18 |
//! | `region:artlum` | regions whose name contains the text |
//! | `code:portal-far` | records with a finding of that rule |
//! | `lv:10-20` | monster areas whose mean level is in the range |
//! | `map:1-5,35` | records of those maps (ids, ranges and lists) |
//! | `count:2-3` | monster areas whose total monster count is in the range |

use std::collections::BTreeMap;

use shaiya_svmap::{Finding, SectionId, Svmap};

use crate::edit::{self, Rec};
use crate::names;

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Word(String),
    Number(u32),
    Mob(u32),
    Npc(u32, u32),
    NpcType(u32),
    Portal(u32),
    Region(String),
    Code(String),
    Level(f32, f32),
    /// Map ids as inclusive ranges (`map:1-5,35` is `[(1, 5), (35, 35)]`).
    Map(Vec<(u32, u32)>),
    /// Total monster count of an area, inclusive.
    Count(u32, u32),
}

/// `1-5,35` as ranges; `None` when anything is not a number or a range runs backwards.
fn parse_ranges(v: &str) -> Option<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    for part in v.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
        if a > b {
            return None;
        }
        out.push((a, b));
    }
    (!out.is_empty()).then_some(out)
}

impl Token {
    /// The word as it is typed (parsing it again gives the same token).
    pub fn text(&self) -> String {
        let range = |a: u32, b: u32| if a == b { a.to_string() } else { format!("{a}-{b}") };
        match self {
            Token::Word(w) => w.clone(),
            Token::Number(n) => n.to_string(),
            Token::Mob(n) => format!("mob:{n}"),
            Token::Npc(k, id) => format!("npc:{k}/{id}"),
            Token::NpcType(k) => format!("npctype:{k}"),
            Token::Portal(n) => format!("portal:{n}"),
            Token::Region(w) => format!("region:{w}"),
            Token::Code(c) => format!("code:{c}"),
            Token::Level(a, b) => {
                if a == b {
                    format!("lv:{a}")
                } else {
                    format!("lv:{a}-{b}")
                }
            }
            Token::Map(r) => format!("map:{}", r.iter().map(|(a, b)| range(*a, *b)).collect::<Vec<_>>().join(",")),
            Token::Count(a, b) => format!("count:{}", range(*a, *b)),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub tokens: Vec<Token>,
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut tokens = Vec::new();
        for w in text.split_whitespace() {
            let lower = w.to_lowercase();
            let t = match lower.split_once(':') {
                Some(("mob", v)) => v.parse().ok().map(Token::Mob),
                Some(("npc", v)) => v.split_once('/').and_then(|(a, b)| Some(Token::Npc(a.parse().ok()?, b.parse().ok()?))),
                Some(("npctype", v)) => v.parse().ok().map(Token::NpcType),
                Some(("portal", v)) => v.parse().ok().map(Token::Portal),
                Some(("region", v)) if !v.is_empty() => Some(Token::Region(v.to_string())),
                Some(("code", v)) if !v.is_empty() => Some(Token::Code(v.to_string())),
                Some(("lv", v)) => {
                    let (a, b) = v.split_once('-').unwrap_or((v, v));
                    match (a.parse::<f32>(), b.parse::<f32>()) {
                        (Ok(a), Ok(b)) => Some(Token::Level(a.min(b), a.max(b))),
                        _ => None,
                    }
                }
                Some(("map", v)) => parse_ranges(v).map(Token::Map),
                Some(("count", v)) => {
                    let (a, b) = v.split_once('-').unwrap_or((v, v));
                    match (a.parse::<u32>(), b.parse::<u32>()) {
                        (Ok(a), Ok(b)) if a <= b => Some(Token::Count(a, b)),
                        _ => None,
                    }
                }
                _ => None,
            };
            tokens.push(match t {
                Some(t) => t,
                None => match lower.parse::<u32>() {
                    Ok(n) => Token::Number(n),
                    Err(_) => Token::Word(lower),
                },
            });
        }
        Query { tokens }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The query as it is typed: the words joined by one space.
    pub fn text(&self) -> String {
        self.tokens.iter().map(Token::text).collect::<Vec<_>>().join(" ")
    }

    /// Like [`Query::parse`], but a `key:value` word that is not a valid one is an error (a typo would otherwise become a free
    /// word that matches nothing).
    pub fn parse_strict(text: &str) -> Result<Query, String> {
        let q = Query::parse(text);
        for (t, w) in q.tokens.iter().zip(text.split_whitespace()) {
            if let Token::Word(s) = t {
                if let Some((key, _)) = s.split_once(':') {
                    return Err(match key {
                        "mob" | "npc" | "npctype" | "portal" | "region" | "code" | "lv" | "map" | "count" => format!("{w}: the value does not fit {key}:"),
                        _ => format!("{w}: Find has no such word"),
                    });
                }
            }
        }
        Ok(q)
    }

    /// Every `mob:` word.
    pub fn mobs(&self) -> Vec<u32> {
        self.tokens.iter().filter_map(|t| if let Token::Mob(m) = t { Some(*m) } else { None }).collect()
    }

    /// The monster the query is about (the first `mob:` word; a plain number is not enough, it also matches NPCs and portals):
    /// what Replace monster replaces.
    pub fn mob(&self) -> Option<u32> {
        self.tokens.iter().find_map(|t| match t {
            Token::Mob(m) => Some(*m),
            _ => None,
        })
    }

    pub fn has_code(&self) -> bool {
        self.tokens.iter().any(|t| matches!(t, Token::Code(_)))
    }
}

/// What the search knows about the client (all optional: without it names fall back to ids).
pub struct Env<'a> {
    /// (name, level) of a monster id.
    pub mob: &'a dyn Fn(u32) -> Option<(String, u32)>,
    pub npc: &'a dyn Fn(u32, u32) -> Option<String>,
    pub map_name: &'a dyn Fn(u32) -> String,
    /// Name of region `index` of map `id`.
    pub region: &'a dyn Fn(u32, usize) -> Option<String>,
}

impl Env<'_> {
    pub fn mob_name(&self, id: u32) -> String {
        match (self.mob)(id) {
            Some((n, _)) if !n.trim().is_empty() => n.trim().to_string(),
            _ => format!("monster {id}"),
        }
    }

    /// Mean level of an area, weighted by count; `None` without client data or entries.
    pub fn mean_level(&self, a: &shaiya_svmap::MobArea) -> Option<f32> {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for e in &a.mobs {
            if let Some((_, l)) = (self.mob)(e.mob) {
                let w = e.count.max(1) as f32;
                sum += l as f32 * w;
                n += w;
            }
        }
        (n > 0.0).then(|| sum / n)
    }

    fn npc_name(&self, n: &shaiya_svmap::NpcEntry) -> String {
        match (self.npc)(n.kind, n.type_id) {
            Some(x) if !x.trim().is_empty() => x.trim().to_string(),
            _ if n.kind == 0 => "placeholder".to_string(),
            _ => format!("id {}", n.type_id),
        }
    }

    /// The one-line label of a record (the same look as the outliner's).
    pub fn label(&self, map_id: Option<u32>, m: &Svmap, s: SectionId, i: usize) -> String {
        match s {
            SectionId::MobAreas => m.mob_areas.get(i).map(|a| {
                if a.mobs.is_empty() {
                    return "(no monsters)".to_string();
                }
                let parts: Vec<String> = a.mobs.iter().map(|e| format!("{} x{}", self.mob_name(e.mob), e.count)).collect();
                let mut t = parts.join(", ");
                if let Some(l) = self.mean_level(a) {
                    t.push_str(&format!(" · lv {}", l.round() as i32));
                }
                t
            }),
            SectionId::Npcs => m.npcs.get(i).map(|n| format!("{}: {}", names::npc_kind_name(n.kind), self.npc_name(n))),
            SectionId::Portals => m.portals.get(i).map(|p| names::portal_label(i, p, self.map_name)),
            SectionId::Binds => m.binds.get(i).map(|b| format!("{} · {} box{}", names::country_name(b.country), b.boxes.len(), if b.boxes.len() == 1 { "" } else { "es" })),
            SectionId::Regions => m.regions.get(i).map(|r| map_id.and_then(|id| (self.region)(id, i)).unwrap_or_else(|| format!("kind {}", r.kind))),
            SectionId::SectionB => m.points_b.get(i).map(|p| format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2])),
            SectionId::SectionA => m.section_a.get(i).map(|a| format!("({:.1}, {:.1}, {:.1}) · {} / {}", a.pos[0], a.pos[1], a.pos[2], a.a, a.b)),
            SectionId::Size | SectionId::Blocked => None,
        }
        .unwrap_or_default()
    }
}

/// One map handed to the search.
pub struct MapIn<'a> {
    pub id: Option<u32>,
    pub map: &'a Svmap,
    /// The map's findings, for `code:` words (None: not known for this map).
    pub findings: Option<&'a [Finding]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub map: Option<u32>,
    pub section: SectionId,
    pub index: usize,
    pub label: String,
    /// What matched ("monster 5 (Wolf)", "destination map 18", "text").
    pub what: String,
}

/// Sections a query looks at, in result order.
pub const SECTIONS: [SectionId; 5] = [SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions];

/// Records matching every word of `q`, map by map in the order given. Stops at `limit` hits. A plain number sorts its groups
/// (monster, NPC, portal) apart.
pub fn search(q: &Query, maps: &[MapIn], env: &Env, limit: usize) -> Vec<Hit> {
    search_sections(q, maps, env, limit, &SECTIONS)
}

/// [`search`] over some of the sections only (Bulk edit looks at monster areas and skips the labels of the rest).
pub fn search_sections(q: &Query, maps: &[MapIn], env: &Env, limit: usize, sections: &[SectionId]) -> Vec<Hit> {
    let mut out: Vec<(u8, Hit)> = Vec::new();
    if q.is_empty() {
        return Vec::new();
    }
    'maps: for mi in maps {
        let map_name = mi.id.map(|id| (env.map_name)(id).to_lowercase()).unwrap_or_default();
        for s in sections.iter().copied() {
            for i in 0..edit::count(mi.map, s) {
                let label = env.label(mi.id, mi.map, s, i);
                let text = label.to_lowercase();
                let mut what = String::new();
                let mut group = 3u8;
                let mut ok = true;
                for t in &q.tokens {
                    match match_token(t, mi, s, i, &text, &map_name, env) {
                        Some((w, g)) => {
                            if what.is_empty() {
                                what = w;
                                group = g;
                            }
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    out.push((group, Hit { map: mi.id, section: s, index: i, label, what }));
                    if out.len() >= limit {
                        break 'maps;
                    }
                }
            }
        }
    }
    if q.tokens.iter().any(|t| matches!(t, Token::Number(_))) {
        out.sort_by_key(|(g, _)| *g); // stable: map, section and index order stay inside a group
    }
    out.into_iter().map(|(_, h)| h).collect()
}

fn match_token(t: &Token, mi: &MapIn, s: SectionId, i: usize, text: &str, map_name: &str, env: &Env) -> Option<(String, u8)> {
    let m = mi.map;
    match t {
        Token::Word(w) => {
            if text.contains(w.as_str()) {
                Some(("text".to_string(), 3))
            } else if !map_name.is_empty() && map_name.contains(w.as_str()) {
                Some(("map name".to_string(), 3))
            } else {
                None
            }
        }
        Token::Number(n) => match s {
            SectionId::MobAreas => m.mob_areas[i].mobs.iter().any(|e| e.mob == *n).then(|| (format!("monster {n} ({})", env.mob_name(*n)), 0)),
            SectionId::Npcs => (m.npcs[i].type_id == *n).then(|| (format!("NPC type id {n}"), 1)),
            SectionId::Portals => (m.portals[i].to_map == *n).then(|| (format!("destination map {n}"), 2)),
            _ => None,
        },
        Token::Mob(n) => match s {
            SectionId::MobAreas => m.mob_areas[i].mobs.iter().any(|e| e.mob == *n).then(|| (format!("monster {n} ({})", env.mob_name(*n)), 0)),
            _ => None,
        },
        Token::Npc(k, id) => match s {
            SectionId::Npcs => (m.npcs[i].kind == *k && m.npcs[i].type_id == *id).then(|| (format!("NPC {k}/{id}"), 1)),
            _ => None,
        },
        Token::NpcType(k) => match s {
            SectionId::Npcs => (m.npcs[i].kind == *k).then(|| (format!("NPC type {k}"), 1)),
            _ => None,
        },
        Token::Portal(n) => match s {
            SectionId::Portals => (m.portals[i].to_map == *n).then(|| (format!("destination map {n}"), 2)),
            _ => None,
        },
        Token::Region(w) => match s {
            SectionId::Regions => {
                let name = mi.id.and_then(|id| (env.region)(id, i))?;
                name.to_lowercase().contains(w.as_str()).then(|| (format!("region {name}"), 3))
            }
            _ => None,
        },
        Token::Code(c) => {
            let f = mi.findings?;
            f.iter().any(|f| f.section == s && f.index == Some(i) && f.code == c.as_str()).then(|| (format!("rule {c}"), 3))
        }
        Token::Level(lo, hi) => match s {
            SectionId::MobAreas => {
                let l = env.mean_level(&m.mob_areas[i])?;
                (l >= *lo && l <= *hi).then(|| (format!("mean level {l:.1}"), 0))
            }
            _ => None,
        },
        Token::Map(ranges) => {
            let id = mi.id?;
            ranges.iter().any(|(a, b)| id >= *a && id <= *b).then(|| (format!("map {id}"), 3))
        }
        Token::Count(lo, hi) => match s {
            SectionId::MobAreas => {
                let n: u32 = m.mob_areas[i].mobs.iter().map(|e| e.count).sum();
                (n >= *lo && n <= *hi).then(|| (format!("count {n}"), 0))
            }
            _ => None,
        },
    }
}

/// Number of results per map, in map order (for "41 areas on 6 maps").
pub fn per_map(hits: &[Hit]) -> BTreeMap<Option<u32>, usize> {
    let mut m = BTreeMap::new();
    for h in hits {
        *m.entry(h.map).or_insert(0) += 1;
    }
    m
}

// ------------------------------------------------------------------------------------------------------ select similar

/// What "similar" means per section: areas with the same set of monster ids, NPCs with the same type and id, portals to the
/// same map, binds with the same country, regions with the same kind; section A and B points are all alike.
pub fn similar_key(r: &Rec) -> String {
    match r {
        Rec::Mob(a) => {
            let mut ids: Vec<u32> = a.mobs.iter().map(|e| e.mob).collect();
            ids.sort_unstable();
            ids.dedup();
            format!("mob:{ids:?}")
        }
        Rec::Npc(n) => format!("npc:{}/{}", n.kind, n.type_id),
        Rec::Portal(p) => format!("portal:{}", p.to_map),
        Rec::Bind(b) => format!("bind:{}", b.country),
        Rec::Region(g) => format!("region:{}", g.kind),
        Rec::A(_) => "a".to_string(),
        Rec::B(_) => "b".to_string(),
    }
}

/// Every record of `m` that is similar to one of `sel` (same section and [`similar_key`]); the selection itself is included.
pub fn select_similar(m: &Svmap, sel: &[(SectionId, usize)]) -> Vec<(SectionId, usize)> {
    let keys: Vec<(SectionId, String)> = sel.iter().filter_map(|(s, i)| edit::get(m, *s, *i).map(|r| (*s, similar_key(&r)))).collect();
    let mut out = Vec::new();
    for s in edit::LIST_SECTIONS {
        if !keys.iter().any(|(k, _)| *k == s) {
            continue;
        }
        for i in 0..edit::count(m, s) {
            if let Some(r) = edit::get(m, s, i) {
                let key = similar_key(&r);
                if keys.iter().any(|(ks, k)| *ks == s && *k == key) {
                    out.push((s, i));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------------ portal links

/// One portal of the folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub from: u32,
    pub index: usize,
    pub to: u32,
    pub country: u32,
    pub level_min: u16,
    pub level_max: u16,
    pub pos: [f32; 3],
    pub to_pos: [f32; 3],
    /// A portal on the destination map leads back to this map.
    pub back: bool,
    /// The destination map has no file in the folder (and is not 200, the guild ranking).
    pub dest_missing: bool,
    pub dest_outside: bool,
    pub dest_blocked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Links {
    pub links: Vec<Link>,
    /// Maps no portal of another map leads to.
    pub no_in: Vec<u32>,
    /// Maps without a portal that leads to another map.
    pub no_out: Vec<u32>,
    /// Links whose destination map has a file but no way back.
    pub one_way: usize,
}

/// Every portal of the given maps, with the way-back and the destination checks.
pub fn portal_links(maps: &[(u32, &Svmap)]) -> Links {
    let by_id: BTreeMap<u32, &Svmap> = maps.iter().copied().collect();
    let mut out = Links::default();
    let mut has_in: std::collections::BTreeSet<u32> = Default::default();
    let mut has_out: std::collections::BTreeSet<u32> = Default::default();
    for (id, m) in maps {
        for (i, p) in m.portals.iter().enumerate() {
            let dest = by_id.get(&p.to_map);
            let back = dest.is_some_and(|d| d.portals.iter().any(|q| q.to_map == *id));
            let (mut outside, mut blocked) = (false, false);
            if let Some(d) = dest {
                match shaiya_svmap::validate::cell_of(d.size, p.to_pos[0], p.to_pos[2]) {
                    None => outside = true,
                    Some((x, z)) => blocked = d.blocked.get(x, z),
                }
            }
            if p.to_map != *id {
                has_out.insert(*id);
                has_in.insert(p.to_map);
            }
            if dest.is_some() && !back {
                out.one_way += 1;
            }
            out.links.push(Link {
                from: *id,
                index: i,
                to: p.to_map,
                country: p.country,
                level_min: p.level_min,
                level_max: p.level_max,
                pos: p.pos,
                to_pos: p.to_pos,
                back,
                dest_missing: dest.is_none() && p.to_map != 200,
                dest_outside: outside,
                dest_blocked: blocked,
            });
        }
    }
    out.no_in = maps.iter().map(|(id, _)| *id).filter(|id| !has_in.contains(id)).collect();
    out.no_out = maps.iter().map(|(id, _)| *id).filter(|id| !has_out.contains(id)).collect();
    out
}

/// The destination as the table shows it: map 200 has no file ("guild ranking").
pub fn dest_label(to: u32, has_file: bool, name: &str) -> String {
    if to == 200 && !has_file {
        "guild ranking (no file)".to_string()
    } else {
        format!("{to} {name}")
    }
}

/// The portals of other maps that lead to map `id` (for the map card).
pub fn leading_here(links: &Links, id: u32) -> Vec<&Link> {
    links.links.iter().filter(|l| l.to == id && l.from != id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::{MobArea, MobEntry, NpcEntry, NpcPoint, Portal, Region};

    fn area(mobs: &[(u32, u32)]) -> MobArea {
        MobArea { min: [0.0; 3], max: [4.0, 2.0, 4.0], mobs: mobs.iter().map(|(m, c)| MobEntry { mob: *m, count: *c }).collect() }
    }

    fn env_with<R>(f: impl FnOnce(&Env) -> R) -> R {
        let mob = |id: u32| Some((format!("Mob{id}"), id * 2));
        let npc = |k: u32, id: u32| Some(format!("Npc{k}-{id}"));
        let name = |id: u32| format!("Map{id}");
        let region = |_m: u32, i: usize| Some(format!("Region {i}"));
        f(&Env { mob: &mob, npc: &npc, map_name: &name, region: &region })
    }

    fn sample() -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas.push(area(&[(5, 2), (6, 1)]));
        m.mob_areas.push(area(&[(7, 1)]));
        m.mob_areas.push(area(&[(5, 1)]));
        m.npcs.push(NpcEntry { kind: 1, type_id: 5, points: vec![NpcPoint { pos: [1.0, 0.0, 1.0], dir: 0.0 }] });
        m.npcs.push(NpcEntry { kind: 2, type_id: 9, points: vec![NpcPoint { pos: [2.0, 0.0, 2.0], dir: 0.0 }] });
        m.portals.push(Portal { pos: [1.0, 0.0, 1.0], country: 0, level_min: 0, level_max: 0, to_map: 5, to_pos: [3.0, 0.0, 3.0] });
        m.regions.push(Region { min: [0.0; 3], max: [8.0, 2.0, 8.0], kind: 0, b: 0, c: 0, d: 0 });
        m
    }

    #[test]
    fn queries_parse() {
        let q = Query::parse("Thief mob:12 npc:1/159 npctype:3 portal:18 region:Art code:portal-far lv:20-10 77 mob:x");
        assert_eq!(
            q.tokens,
            vec![
                Token::Word("thief".into()),
                Token::Mob(12),
                Token::Npc(1, 159),
                Token::NpcType(3),
                Token::Portal(18),
                Token::Region("art".into()),
                Token::Code("portal-far".into()),
                Token::Level(10.0, 20.0),
                Token::Number(77),
                Token::Word("mob:x".into()),
            ]
        );
        assert_eq!(q.mob(), Some(12));
        assert!(Query::parse("  ").is_empty());
    }

    #[test]
    fn search_finds_records_by_every_kind_of_word() {
        let m = sample();
        let maps = [MapIn { id: Some(1), map: &m, findings: None }];
        env_with(|env| {
            let run = |q: &str| search(&Query::parse(q), &maps, env, 100);
            let ids = |h: &[Hit]| h.iter().map(|x| (x.section, x.index)).collect::<Vec<_>>();
            assert_eq!(ids(&run("mob:5")), vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 2)]);
            assert_eq!(ids(&run("mob99")), vec![]);
            // a plain number: areas with the monster, NPCs with the type id, portals to the map, in groups
            assert_eq!(ids(&run("5")), vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 2), (SectionId::Npcs, 0), (SectionId::Portals, 0)]);
            assert_eq!(ids(&run("npc:2/9")), vec![(SectionId::Npcs, 1)]);
            assert_eq!(ids(&run("npctype:1")), vec![(SectionId::Npcs, 0)]);
            assert_eq!(ids(&run("portal:5")), vec![(SectionId::Portals, 0)]);
            assert_eq!(ids(&run("region:region")), vec![(SectionId::Regions, 0)]);
            // a word: monster names, NPC names; a word of the map's name matches every record of the map, and words combine
            assert_eq!(ids(&run("mob7")), vec![(SectionId::MobAreas, 1)]);
            assert_eq!(ids(&run("npc2-9")), vec![(SectionId::Npcs, 1)]);
            assert_eq!(ids(&run("map1 mob6")), vec![(SectionId::MobAreas, 0)]);
            // levels: area 0 = (2*10 + 1*12) / 3 = 10.67, area 1 = 14, area 2 = 10
            assert_eq!(ids(&run("lv:10.5-11")), vec![(SectionId::MobAreas, 0)]);
            assert_eq!(ids(&run("lv:12-20")), vec![(SectionId::MobAreas, 1)]);
            assert!(run("mob:5")[0].what.contains("Mob5") && run("mob:5")[0].label.contains("Mob5 x2"));
            // limit
            assert_eq!(search(&Query::parse("map1"), &maps, env, 3).len(), 3);
        });
    }

    #[test]
    fn map_and_count_words() {
        // the words parse into ranges and back
        let q = Query::parse("map:1-5,35 count:2-3 count:4");
        assert_eq!(q.tokens, vec![Token::Map(vec![(1, 5), (35, 35)]), Token::Count(2, 3), Token::Count(4, 4)]);
        assert_eq!(q.text(), "map:1-5,35 count:2-3 count:4");
        for bad in ["map:5-1", "map:", "map:a", "count:3-2", "count:x"] {
            assert!(matches!(Query::parse(bad).tokens[0], Token::Word(_)), "{bad} is not a word of Find");
            assert!(Query::parse_strict(bad).is_err(), "{bad}");
        }
        assert!(Query::parse_strict("thief mob:5 map:1").is_ok());
        // they match: map ids on every section, count on areas only
        let m = sample();
        let maps = [MapIn { id: Some(1), map: &m, findings: None }, MapIn { id: Some(9), map: &m, findings: None }];
        env_with(|env| {
            let run = |q: &str| search(&Query::parse(q), &maps, env, 100).iter().map(|h| (h.map, h.section, h.index)).collect::<Vec<_>>();
            assert_eq!(run("map:1 mob:5"), vec![(Some(1), SectionId::MobAreas, 0), (Some(1), SectionId::MobAreas, 2)]);
            assert_eq!(run("map:2-8 mob:5"), vec![]);
            assert_eq!(run("map:1,9 mob:7"), vec![(Some(1), SectionId::MobAreas, 1), (Some(9), SectionId::MobAreas, 1)]);
            assert_eq!(run("map:9 npctype:1"), vec![(Some(9), SectionId::Npcs, 0)], "a map word works on every section");
            // area 0 holds 2 + 1 monsters, areas 1 and 2 one each
            assert_eq!(run("map:9 count:3"), vec![(Some(9), SectionId::MobAreas, 0)]);
            assert_eq!(run("map:1 count:1-2"), vec![(Some(1), SectionId::MobAreas, 1), (Some(1), SectionId::MobAreas, 2)]);
            assert_eq!(run("count:1"), vec![(Some(1), SectionId::MobAreas, 1), (Some(1), SectionId::MobAreas, 2), (Some(9), SectionId::MobAreas, 1), (Some(9), SectionId::MobAreas, 2)]);
            // a map without a number has no id to match
            let none = [MapIn { id: None, map: &m, findings: None }];
            assert!(search(&Query::parse("map:1-99"), &none, env, 10).is_empty());
            // searching one section only
            let only = search_sections(&Query::parse("map:1"), &maps, env, 100, &[SectionId::Portals]);
            assert_eq!(only.len(), 1);
        });
    }

    #[test]
    fn code_words_use_the_findings_of_the_map() {
        let m = sample();
        let f = vec![Finding { severity: shaiya_svmap::Severity::Warning, section: SectionId::MobAreas, index: Some(1), sub: None, code: "area-count-high", message: "x".into() }];
        let with = [MapIn { id: Some(1), map: &m, findings: Some(&f) }];
        let without = [MapIn { id: Some(1), map: &m, findings: None }];
        env_with(|env| {
            let q = Query::parse("code:area-count-high");
            assert_eq!(search(&q, &with, env, 10).len(), 1);
            assert!(search(&q, &without, env, 10).is_empty());
        });
    }

    #[test]
    fn similar_groups_by_key() {
        let m = sample();
        // similar: areas with the same set of monster ids
        let mut m2 = m.clone();
        m2.mob_areas.push(area(&[(6, 3), (5, 1)]));
        assert_eq!(select_similar(&m2, &[(SectionId::MobAreas, 0)]), vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 3)]);
        assert_eq!(select_similar(&m2, &[(SectionId::Npcs, 1)]), vec![(SectionId::Npcs, 1)]);
    }

    #[test]
    fn links_find_way_back_and_missing_maps() {
        let mut a = Svmap::new(64);
        let mut b = Svmap::new(64);
        let mut c = Svmap::new(64);
        let p = |to: u32, x: f32| Portal { pos: [1.0, 0.0, 1.0], country: 0, level_min: 0, level_max: 0, to_map: to, to_pos: [x, 0.0, 5.0] };
        a.portals.push(p(2, 5.0)); // a -> b, b leads back
        a.portals.push(p(200, 5.0)); // guild ranking, no file
        a.portals.push(p(9, 5.0)); // missing
        b.portals.push(p(1, 5.0));
        b.portals.push(p(3, 99.0)); // b -> c outside (c is 64 wide: 99 is out); c has no way back
        c.blocked.set(5, 5, true);
        c.portals.push(p(3, 5.0)); // c -> itself: no way out of c
        let l = portal_links(&[(1, &a), (2, &b), (3, &c)]);
        assert_eq!(l.links.len(), 6);
        assert!(l.links[0].back && !l.links[0].dest_missing);
        assert!(!l.links[1].dest_missing && !l.links[1].back, "200 is not a missing map");
        assert!(l.links[2].dest_missing);
        assert!(l.links[4].dest_outside && !l.links[4].back);
        assert!(l.no_in.is_empty());
        assert_eq!(l.no_out, vec![3]);
        assert_eq!(l.one_way, 1, "b -> c only (a -> 200 and a -> 9 have no file to come back from)");
        assert_eq!(leading_here(&l, 2).len(), 1);
        assert_eq!(dest_label(200, false, "Map 200"), "guild ranking (no file)");
        assert_eq!(dest_label(200, true, "Map 200"), "200 Map 200");
        assert_eq!(dest_label(2, true, "X"), "2 X");
    }

    /// `mob:1` over the 77 original files counts the same as a direct count, and the search of all maps is fast.
    #[test]
    fn search_mob_counts() {
        let Some(dir) = std::env::var_os("SVMAP_ORIGINALS") else {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            return;
        };
        let mut maps: Vec<(u32, Svmap)> = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "svmap") {
                if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                    maps.push((id, shaiya_svmap::read(&std::fs::read(&p).unwrap()).unwrap()));
                }
            }
        }
        maps.sort_by_key(|(id, _)| *id);
        assert_eq!(maps.len(), 77);
        let direct: usize = maps.iter().map(|(_, m)| m.mob_areas.iter().filter(|a| a.mobs.iter().any(|e| e.mob == 1)).count()).sum();
        let ins: Vec<MapIn> = maps.iter().map(|(id, m)| MapIn { id: Some(*id), map: m, findings: None }).collect();
        env_with(|env| {
            let t = std::time::Instant::now();
            let hits = search(&Query::parse("mob:1"), &ins, env, 100_000);
            let dt = t.elapsed();
            println!("mob:1 over 77 maps: {} hits in {dt:?} (direct count {direct})", hits.len());
            assert_eq!(hits.len(), direct);
            assert!(direct > 0, "the originals do use monster 1");
            // the plain number finds at least those, and a word search over every label is timed (printed, not asserted)
            assert!(search(&Query::parse("1"), &ins, env, 100_000).len() >= direct);
            let t = std::time::Instant::now();
            let _ = search(&Query::parse("mob1 map"), &ins, env, 100_000);
            println!("all-maps word search took {:?} (no timing assertion)", t.elapsed());
        });
    }

    /// The link table has one row per portal of the 77 files, and map 200 (the guild ranking) has no file.
    #[test]
    fn portal_links_originals() {
        let Some(dir) = std::env::var_os("SVMAP_ORIGINALS") else {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            return;
        };
        let mut maps: Vec<(u32, Svmap)> = Vec::new();
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()).filter(|_| p.extension().is_some_and(|x| x == "svmap")) {
                maps.push((id, shaiya_svmap::read(&std::fs::read(&p).unwrap()).unwrap()));
            }
        }
        let total: usize = maps.iter().map(|(_, m)| m.portals.len()).sum();
        let refs: Vec<(u32, &Svmap)> = maps.iter().map(|(i, m)| (*i, m)).collect();
        let l = portal_links(&refs);
        println!("{} links, {} maps with no way in, {} with no way out, {} one-way", l.links.len(), l.no_in.len(), l.no_out.len(), l.one_way);
        assert_eq!(l.links.len(), total);
        let to200 = l.links.iter().filter(|x| x.to == 200).count();
        assert!(l.links.iter().filter(|x| x.to == 200).all(|x| !x.dest_missing), "200 is never reported as a missing map");
        println!("{to200} portals lead to map 200");
        assert_eq!(dest_label(200, maps.iter().any(|(i, _)| *i == 200), "Map 200"), "guild ranking (no file)");
    }
}
