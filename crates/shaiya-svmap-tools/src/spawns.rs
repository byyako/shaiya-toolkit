//! Spawn insight: the spawn table of a map, the map's level range, the sanity findings and
//! the monsters no area spawns. Pure: no egui, no files. The caller hands in the maps (folder copies, open tabs live) and the
//! names and levels it knows through the same [`Env`] the search uses.
//!
//! These findings are not validation rules. Problems, `check` and its totals do not change.

use std::collections::{BTreeMap, BTreeSet};

use shaiya_clientdata::Monsters;
use shaiya_svmap::{MobArea, Svmap};

use crate::search::{Env, Link, Links, MapIn};

/// Two boxes are the same when every coordinate is within this.
pub const SAME_BOX: f32 = 0.01;
/// An area whose entries' levels span more than this is mixed.
pub const MIXED_SPAN: u32 = 10;
/// A monster with at most this many areas on a map, while most of its count is on another map, is a stray there.
pub const STRAY_MAX_AREAS: usize = 2;
/// The share of a monster's count one other map must hold for it to be the monster's main map.
pub const STRAY_SHARE: f64 = 0.8;
/// Fewer entries with a known level than this give no range.
pub const BAND_MIN_ENTRIES: usize = 5;
pub const DEFAULT_TOLERANCE: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    LevelOutside,
    Stray,
    Duplicate,
    Mixed,
    UnknownLevel,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::LevelOutside, Group::Stray, Group::Duplicate, Group::Mixed, Group::UnknownLevel];

    pub fn index(self) -> usize {
        Group::ALL.iter().position(|g| *g == self).unwrap_or(0)
    }

    /// The heading in the Spawns tab.
    pub fn label(self) -> &'static str {
        match self {
            Group::LevelOutside => "Level outside the map's range",
            Group::Stray => "Strays",
            Group::Duplicate => "Duplicate areas",
            Group::Mixed => "Mixed levels",
            Group::UnknownLevel => "Unknown level",
        }
    }

    /// The name in the CLI output and JSON.
    pub fn key(self) -> &'static str {
        match self {
            Group::LevelOutside => "level-outside",
            Group::Stray => "stray",
            Group::Duplicate => "duplicate",
            Group::Mixed => "mixed",
            Group::UnknownLevel => "unknown-level",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Group::LevelOutside => "The monster's level is below or above the map's level range by more than the tolerance.",
            Group::Stray => "A monster with 1 or 2 areas here whose other areas are mostly on one other map.",
            Group::Duplicate => "Two areas with the same box and the same monsters.",
            Group::Mixed => "One area holds monsters more than 10 levels apart.",
            Group::UnknownLevel => "The monster has no level in Monster.SData, or is not in it.",
        }
    }
}

/// What to check: the tolerance, the per-map ranges that win over the derived ones, and whether monster levels exist at all
/// (without them only duplicate areas and strays are looked for).
pub struct Opts<'a> {
    pub tolerance: u32,
    pub overrides: &'a dyn Fn(u32) -> Option<(u32, u32)>,
    pub levels: bool,
}

fn no_override(_: u32) -> Option<(u32, u32)> {
    None
}

impl Opts<'static> {
    pub fn plain(levels: bool) -> Opts<'static> {
        Opts { tolerance: DEFAULT_TOLERANCE, overrides: &no_override, levels }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpawnFinding {
    pub map: Option<u32>,
    pub group: Group,
    /// The monster areas it is about (a duplicate names both).
    pub areas: Vec<usize>,
    /// The monsters involved, for the flags in the spawn table.
    pub mobs: Vec<u32>,
    pub text: String,
}

/// The level of a monster when the client knows one (> 0).
fn level_of(env: &Env, id: u32) -> Option<u32> {
    (env.mob)(id).map(|(_, l)| l).filter(|l| *l > 0)
}

/// The count-weighted 10th and 90th percentiles of `(level, weight)` pairs, rounded: the smallest level whose running weight
/// reaches 10 % (90 %) of the total. Fewer than [`BAND_MIN_ENTRIES`] entries give no range.
pub fn band(levels: &[(f32, u32)]) -> Option<(u32, u32)> {
    if levels.len() < BAND_MIN_ENTRIES {
        return None;
    }
    let mut v: Vec<(f32, f64)> = levels.iter().map(|(l, w)| (*l, (*w).max(1) as f64)).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|x| x.1).sum();
    let pick = |p: f64| {
        let mut cum = 0.0;
        for (l, w) in &v {
            cum += w;
            if cum >= p * total - 1e-9 {
                return *l;
            }
        }
        v.last().map_or(0.0, |x| x.0)
    };
    Some((pick(0.10).round() as u32, pick(0.90).round() as u32))
}

/// The levels of every entry of a map that has one, with the entry's count as its weight.
pub fn map_levels(map: &Svmap, env: &Env) -> Vec<(f32, u32)> {
    let mut v = Vec::new();
    for a in &map.mob_areas {
        for e in &a.mobs {
            if let Some(l) = level_of(env, e.mob) {
                v.push((l as f32, e.count));
            }
        }
    }
    v
}

/// The map's own level range (from its spawns).
pub fn derived_band(map: &Svmap, env: &Env) -> Option<(u32, u32)> {
    band(&map_levels(map, env))
}

/// The range an override or the derived one gives.
pub fn effective_band(derived: Option<(u32, u32)>, over: Option<(u32, u32)>) -> Option<(u32, u32)> {
    over.or(derived)
}

/// "Entry portals: lv 15-30 (2)": the level limits of the portals of other maps that lead into map `id` (limits of 0 are skipped).
pub fn entry_hint(links: &Links, id: u32) -> Option<String> {
    let into: Vec<&Link> = links.links.iter().filter(|l| l.to == id && l.from != id).collect();
    let (mut lo, mut hi) = (u16::MAX, 0u16);
    for l in &into {
        if l.level_min > 0 || l.level_max > 0 {
            lo = lo.min(if l.level_min > 0 { l.level_min } else { l.level_max });
            hi = hi.max(l.level_max.max(l.level_min));
        }
    }
    (hi > 0).then(|| format!("Entry portals: lv {lo}\u{2013}{hi} ({})", into.len()))
}

// ------------------------------------------------------------------------------------------------------------------ table

/// One monster on one map.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnRow {
    pub mob: u32,
    pub name: String,
    pub level: Option<u32>,
    pub areas: usize,
    pub total: u32,
    /// Share of the map's monsters, 0 to 1.
    pub share: f32,
}

/// One row per monster id on the map, biggest first.
pub fn table(map: &Svmap, env: &Env) -> Vec<SpawnRow> {
    let mut by: BTreeMap<u32, (usize, u32)> = BTreeMap::new();
    for a in &map.mob_areas {
        let mut seen: BTreeSet<u32> = BTreeSet::new();
        for e in &a.mobs {
            let r = by.entry(e.mob).or_default();
            r.1 += e.count;
            if seen.insert(e.mob) {
                r.0 += 1;
            }
        }
    }
    let all: u32 = by.values().map(|v| v.1).sum();
    let mut rows: Vec<SpawnRow> = by
        .into_iter()
        .map(|(mob, (areas, total))| SpawnRow { mob, name: env.mob_name(mob), level: level_of(env, mob), areas, total, share: if all > 0 { total as f32 / all as f32 } else { 0.0 } })
        .collect();
    rows.sort_by(|a, b| b.total.cmp(&a.total).then(a.mob.cmp(&b.mob)));
    rows
}

// ------------------------------------------------------------------------------------------------------------------ findings

#[derive(Clone, Debug, PartialEq)]
pub struct MapSummary {
    pub id: Option<u32>,
    pub areas: usize,
    /// Total count of monsters.
    pub monsters: u32,
    /// Distinct monster ids.
    pub kinds: usize,
    /// The range in use (an override, else the derived one).
    pub band: Option<(u32, u32)>,
    pub derived: Option<(u32, u32)>,
    pub overridden: bool,
    /// Findings per group, in [`Group::ALL`] order.
    pub counts: [usize; 5],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub maps: Vec<MapSummary>,
    pub findings: Vec<SpawnFinding>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub maps: usize,
    pub areas: usize,
    pub monsters: u32,
    pub kinds: usize,
    pub findings: [usize; 5],
}

impl Report {
    pub fn summary(&self, id: Option<u32>) -> Option<&MapSummary> {
        self.maps.iter().find(|m| m.id == id)
    }

    pub fn findings_of(&self, id: Option<u32>) -> impl Iterator<Item = &SpawnFinding> {
        self.findings.iter().filter(move |f| f.map == id)
    }

    /// The folder totals; `kinds` counts distinct monster ids over every map.
    pub fn totals(&self, maps: &[MapIn]) -> Totals {
        let mut t = Totals { maps: self.maps.len(), ..Default::default() };
        let mut kinds: BTreeSet<u32> = BTreeSet::new();
        for m in &self.maps {
            t.areas += m.areas;
            t.monsters += m.monsters;
        }
        for mi in maps {
            for a in &mi.map.mob_areas {
                kinds.extend(a.mobs.iter().map(|e| e.mob));
            }
        }
        t.kinds = kinds.len();
        for f in &self.findings {
            t.findings[f.group.index()] += 1;
        }
        t
    }
}

fn same_box(a: &MobArea, b: &MobArea) -> bool {
    (0..3).all(|i| (a.min[i] - b.min[i]).abs() <= SAME_BOX && (a.max[i] - b.max[i]).abs() <= SAME_BOX)
}

fn sorted_entries(a: &MobArea) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = a.mobs.iter().map(|e| (e.mob, e.count)).collect();
    v.sort_unstable();
    v
}

fn find_duplicates(id: Option<u32>, map: &Svmap, out: &mut Vec<SpawnFinding>) {
    let n = map.mob_areas.len();
    let keys: Vec<Vec<(u32, u32)>> = map.mob_areas.iter().map(sorted_entries).collect();
    // sorted by min x so the inner loop stops early
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| map.mob_areas[*a].min[0].total_cmp(&map.mob_areas[*b].min[0]));
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (oi, &i) in order.iter().enumerate() {
        for &j in &order[oi + 1..] {
            if map.mob_areas[j].min[0] - map.mob_areas[i].min[0] > SAME_BOX {
                break;
            }
            if same_box(&map.mob_areas[i], &map.mob_areas[j]) && keys[i] == keys[j] {
                pairs.push((i.min(j), i.max(j)));
            }
        }
    }
    pairs.sort_unstable();
    for (i, j) in pairs {
        let mobs: Vec<u32> = keys[i].iter().map(|e| e.0).collect();
        out.push(SpawnFinding { map: id, group: Group::Duplicate, areas: vec![i, j], mobs, text: format!("#{i} and #{j} are the same box with the same monsters") });
    }
}

/// Every monster's areas and count per map, for the strays.
type Spread = BTreeMap<u32, BTreeMap<u32, (usize, u32)>>;

fn spread_of(maps: &[MapIn]) -> Spread {
    let mut s: Spread = BTreeMap::new();
    for mi in maps {
        let Some(id) = mi.id else { continue };
        for a in &mi.map.mob_areas {
            let mut seen: BTreeSet<u32> = BTreeSet::new();
            for e in &a.mobs {
                let r = s.entry(e.mob).or_default().entry(id).or_default();
                r.1 += e.count;
                if seen.insert(e.mob) {
                    r.0 += 1;
                }
            }
        }
    }
    s
}

/// The summaries and findings of every map handed in. A stray needs the other maps, so the whole folder goes in at once.
pub fn analyze(maps: &[MapIn], env: &Env, opts: &Opts) -> Report {
    let spread = spread_of(maps);
    let mut report = Report::default();
    for mi in maps {
        let m = mi.map;
        let id = mi.id;
        let mut found: Vec<SpawnFinding> = Vec::new();
        let derived = if opts.levels { derived_band(m, env) } else { None };
        let over = id.and_then(opts.overrides);
        let used = effective_band(derived, over);

        find_duplicates(id, m, &mut found);

        if let Some(id) = id {
            for (&mob, per) in &spread {
                let Some(&(areas_here, _)) = per.get(&id) else { continue };
                if areas_here > STRAY_MAX_AREAS || per.len() < 2 {
                    continue;
                }
                let total: u32 = per.values().map(|v| v.1).sum();
                let Some((&main, &(main_areas, main_count))) = per.iter().filter(|(k, _)| **k != id).max_by_key(|(k, v)| (v.1, std::cmp::Reverse(**k))) else { continue };
                if total == 0 || (main_count as f64) < STRAY_SHARE * total as f64 {
                    continue;
                }
                for (i, a) in m.mob_areas.iter().enumerate() {
                    if a.mobs.iter().any(|e| e.mob == mob) {
                        found.push(SpawnFinding {
                            map: Some(id),
                            group: Group::Stray,
                            areas: vec![i],
                            mobs: vec![mob],
                            text: format!("{} is mostly on map {main} ({main_areas} area{})", env.mob_name(mob), if main_areas == 1 { "" } else { "s" }),
                        });
                    }
                }
            }
        }

        if opts.levels {
            for (i, a) in m.mob_areas.iter().enumerate() {
                let mut known: Vec<u32> = Vec::new();
                for e in &a.mobs {
                    match level_of(env, e.mob) {
                        Some(l) => {
                            known.push(l);
                            if let Some((lo, hi)) = used {
                                if (l as i64) < lo as i64 - opts.tolerance as i64 || (l as i64) > hi as i64 + opts.tolerance as i64 {
                                    found.push(SpawnFinding {
                                        map: id,
                                        group: Group::LevelOutside,
                                        areas: vec![i],
                                        mobs: vec![e.mob],
                                        text: format!("#{i} {} lv {l}, range {lo}\u{2013}{hi}", env.mob_name(e.mob)),
                                    });
                                }
                            }
                        }
                        None => found.push(SpawnFinding {
                            map: id,
                            group: Group::UnknownLevel,
                            areas: vec![i],
                            mobs: vec![e.mob],
                            text: format!("#{i} {} (id {}) has no level", env.mob_name(e.mob), e.mob),
                        }),
                    }
                }
                if let (Some(&lo), Some(&hi)) = (known.iter().min(), known.iter().max()) {
                    if hi - lo > MIXED_SPAN {
                        found.push(SpawnFinding {
                            map: id,
                            group: Group::Mixed,
                            areas: vec![i],
                            mobs: a.mobs.iter().map(|e| e.mob).collect(),
                            text: format!("#{i} has levels {lo} to {hi}"),
                        });
                    }
                }
            }
        }

        let mut counts = [0usize; 5];
        for f in &found {
            counts[f.group.index()] += 1;
        }
        let kinds: BTreeSet<u32> = m.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|e| e.mob)).collect();
        report.maps.push(MapSummary {
            id,
            areas: m.mob_areas.len(),
            monsters: m.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|e| e.count)).sum(),
            kinds: kinds.len(),
            band: used,
            derived,
            overridden: over.is_some(),
            counts,
        });
        found.sort_by_key(|f| (f.group, f.areas.first().copied()));
        report.findings.extend(found);
    }
    report
}

/// Monster ids in `Monster.SData` with a name and a level above 0 that no monster area of any given map uses.
pub fn never_spawned(maps: &[&Svmap], monsters: &Monsters) -> Vec<u32> {
    let used: BTreeSet<u32> = maps.iter().flat_map(|m| m.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|e| e.mob))).collect();
    monsters.records.iter().enumerate().filter(|(i, r)| !r.name.trim().is_empty() && r.level > 0 && !used.contains(&(*i as u32))).map(|(i, _)| i as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::MobEntry;

    fn area(x: f32, z: f32, mobs: &[(u32, u32)]) -> MobArea {
        MobArea { min: [x, 0.0, z], max: [x + 8.0, 2.0, z + 8.0], mobs: mobs.iter().map(|(m, c)| MobEntry { mob: *m, count: *c }).collect() }
    }

    fn map(areas: Vec<MobArea>) -> Svmap {
        let mut m = Svmap::new(512);
        m.mob_areas = areas;
        m
    }

    /// Monster id -> level for the synthetic maps (a few ids have no level or do not exist).
    fn lv(id: u32) -> Option<u32> {
        match id {
            11 => Some(5),
            12 => Some(10),
            13 => Some(15),
            14 => Some(20),
            15 => Some(25),
            9 => Some(15),
            16 => Some(5),
            17 => Some(20),
            21..=25 => Some(10 + (id - 21) * 5),
            26 => Some(60),
            27 => Some(12),
            31..=35 => Some(12 + (id - 31)),
            _ => None,
        }
    }

    fn with_env<R>(f: impl FnOnce(&Env) -> R) -> R {
        let mob = |id: u32| lv(id).map(|l| (format!("m{id}"), l));
        let npc = |_: u32, _: u32| None;
        let name = |id: u32| format!("map {id}");
        let region = |_: u32, _: usize| None;
        f(&Env { mob: &mob, npc: &npc, map_name: &name, region: &region })
    }

    #[test]
    fn spawn_band_percentiles() {
        // levels 1 to 10, weight 1 each: the smallest level whose running weight reaches 10 % / 90 % of 10
        let v: Vec<(f32, u32)> = (1..=10).map(|l| (l as f32, 1)).collect();
        assert_eq!(band(&v), Some((1, 9)));
        // weights move the percentiles: level 20 holds 90 of 100
        let v = vec![(10.0, 5), (12.0, 5), (14.0, 5), (20.0, 90), (60.0, 1)];
        assert_eq!(band(&v), Some((14, 20)));
        // fewer than 5 entries with a known level: no range
        assert_eq!(band(&[(10.0, 3), (20.0, 3), (30.0, 3), (40.0, 3)]), None);
        assert_eq!(band(&[]), None);
        // unsorted input gives the same range; a weight of 0 counts as 1
        let v = vec![(30.0, 1), (10.0, 0), (20.0, 1), (40.0, 1), (50.0, 1)];
        assert_eq!(band(&v), Some((10, 50)));
        // an override wins over the derived range
        assert_eq!(effective_band(Some((10, 50)), Some((1, 5))), Some((1, 5)));
        assert_eq!(effective_band(Some((10, 50)), None), Some((10, 50)));
        assert_eq!(effective_band(None, None), None);
        // and through the analysis
        let m = map((0..6).map(|i| area(i as f32 * 20.0, 0.0, &[(11 + (i % 5) as u32, 2)])).collect());
        let ins = [MapIn { id: Some(1), map: &m, findings: None }];
        with_env(|env| {
            let r = analyze(&ins, env, &Opts::plain(true));
            let s = r.summary(Some(1)).unwrap();
            assert!(s.band.is_some() && !s.overridden && s.band == s.derived);
            let over = |id: u32| (id == 1).then_some((100u32, 110u32));
            let r = analyze(&ins, env, &Opts { tolerance: 5, overrides: &over, levels: true });
            let s = r.summary(Some(1)).unwrap();
            assert_eq!(s.band, Some((100, 110)));
            assert!(s.overridden && s.derived.is_some());
            assert_eq!(r.findings.iter().filter(|f| f.group == Group::LevelOutside).count(), 6, "every entry is far below 100");
        });
    }

    /// Three maps, each flaw once: a level outlier, a stray with its main map, a duplicate pair, a mixed area; nothing else.
    #[test]
    fn spawn_findings_synthetic() {
        // map 10: five levels (5..25) in separate areas, monster 9 (lv 15) in four areas (its main map), one mixed area
        let mut a10: Vec<MobArea> = (0..5).map(|i| area(i as f32 * 30.0, 0.0, &[(11 + i as u32, 10)])).collect();
        a10.extend((0..4).map(|i| area(i as f32 * 30.0, 100.0, &[(9, 2)])));
        a10.push(area(0.0, 200.0, &[(16, 1), (17, 1)])); // levels 5 and 20: span 15
        // map 11: five levels (10..30), the outlier (lv 60), a duplicate pair of monster 27, monster 9 once (the stray)
        let mut a11: Vec<MobArea> = (0..5).map(|i| area(i as f32 * 30.0, 0.0, &[(21 + i as u32, 10)])).collect();
        a11.push(area(0.0, 100.0, &[(26, 1)]));
        a11.push(area(60.0, 100.0, &[(27, 3)]));
        a11.push(area(60.0, 100.0, &[(27, 3)]));
        a11.push(area(120.0, 100.0, &[(9, 2)]));
        // map 12: five monsters inside the range, nothing wrong
        let a12: Vec<MobArea> = (0..5).map(|i| area(i as f32 * 30.0, 0.0, &[(31 + i as u32, 4)])).collect();
        let (m10, m11, m12) = (map(a10), map(a11), map(a12));
        let ins = [MapIn { id: Some(10), map: &m10, findings: None }, MapIn { id: Some(11), map: &m11, findings: None }, MapIn { id: Some(12), map: &m12, findings: None }];
        with_env(|env| {
            let r = analyze(&ins, env, &Opts::plain(true));
            let mut got: Vec<(Option<u32>, Group, Vec<usize>)> = r.findings.iter().map(|f| (f.map, f.group, f.areas.clone())).collect();
            got.sort();
            let want = vec![
                (Some(10), Group::Mixed, vec![9]),
                (Some(11), Group::LevelOutside, vec![5]),
                (Some(11), Group::Stray, vec![8]),
                (Some(11), Group::Duplicate, vec![6, 7]),
            ];
            let mut want = want;
            want.sort();
            assert_eq!(got, want, "{:#?}", r.findings);
            let t = r.totals(&ins);
            assert_eq!(t.findings, [1, 1, 1, 1, 0]);
            assert_eq!(t.maps, 3);
            assert_eq!(t.areas, 10 + 9 + 5);
            // the stray names its main map
            let stray = r.findings.iter().find(|f| f.group == Group::Stray).unwrap();
            assert!(stray.text.contains("map 10") && stray.text.contains("4 areas"), "{}", stray.text);
            // without client levels only the duplicate and the stray remain
            let r = analyze(&ins, env, &Opts::plain(false));
            let groups: Vec<Group> = r.findings.iter().map(|f| f.group).collect();
            assert_eq!(groups.len(), 2);
            assert!(groups.contains(&Group::Duplicate) && groups.contains(&Group::Stray));
            assert!(r.maps.iter().all(|m| m.band.is_none()));
        });
    }

    #[test]
    fn unknown_levels_and_the_table() {
        let m = map(vec![area(0.0, 0.0, &[(11, 3), (99, 1)]), area(20.0, 0.0, &[(11, 1)])]);
        let ins = [MapIn { id: Some(1), map: &m, findings: None }];
        with_env(|env| {
            let r = analyze(&ins, env, &Opts::plain(true));
            let unknown: Vec<&SpawnFinding> = r.findings.iter().filter(|f| f.group == Group::UnknownLevel).collect();
            assert_eq!(unknown.len(), 1);
            assert_eq!(unknown[0].mobs, vec![99]);
            let rows = table(&m, env);
            assert_eq!(rows.len(), 2);
            assert_eq!((rows[0].mob, rows[0].areas, rows[0].total, rows[0].level), (11, 2, 4, Some(5)));
            assert!((rows[0].share - 0.8).abs() < 1e-6 && rows[1].level.is_none());
        });
    }

    #[test]
    fn never_spawned_counts_named_leveled_unused_records() {
        use shaiya_clientdata::sdata::Monster;
        let rec = |name: &str, level: i16| Monster { name: name.to_string(), model: 0, level, ai: 0, hp: 1 };
        let mons = Monsters { records: vec![rec("a", 5), rec("", 7), rec("c", 0), rec("d", 9), rec("e", 3)] };
        let m = map(vec![area(0.0, 0.0, &[(0, 1)]), area(0.0, 0.0, &[(4, 1)])]);
        assert_eq!(never_spawned(&[&m], &mons), vec![3]);
        assert_eq!(never_spawned(&[], &mons), vec![0, 3, 4]);
    }

    #[test]
    fn entry_hint_reads_the_portals_that_lead_in() {
        let mk = |from: u32, to: u32, lo: u16, hi: u16| Link { from, index: 0, to, country: 0, level_min: lo, level_max: hi, pos: [0.0; 3], to_pos: [0.0; 3], back: false, dest_missing: false, dest_outside: false, dest_blocked: false };
        let links = Links { links: vec![mk(1, 5, 15, 30), mk(2, 5, 20, 40), mk(3, 5, 0, 0), mk(5, 5, 1, 99), mk(1, 6, 0, 0)], ..Default::default() };
        assert_eq!(entry_hint(&links, 5).as_deref(), Some("Entry portals: lv 15\u{2013}40 (3)"));
        assert_eq!(entry_hint(&links, 6), None);
        assert_eq!(entry_hint(&links, 9), None);
    }
}
