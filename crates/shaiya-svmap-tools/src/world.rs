//! World view: every map of the folder as a card with its numbers, the portal links between them
//! and two ways to arrange the cards. Pure: no egui, no files; the drawing is in `app_world.rs`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use shaiya_svmap::Svmap;

use crate::scatter::thousands;
use crate::search::Env;
use crate::spawns;

/// Card size in world units (pixels at zoom 1); the thumbnail is the top 180 x 180.
pub const CARD_W: f32 = 180.0;
pub const CARD_H: f32 = 230.0;
pub const THUMB: f32 = 180.0;
/// Space between cards in the by-id grid.
pub const GAP: f32 = 30.0;
/// The guild ranking: a portal target with no map file.
pub const GUILD_RANKING: u32 = 200;

/// Which cards and details the World view shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldFilter {
    pub fields: bool,
    pub dungeons: bool,
    /// Maps with an id of 100 or more.
    pub high_ids: bool,
    /// One dot per monster area on the thumbnail.
    pub spawn_dots: bool,
}

impl Default for WorldFilter {
    fn default() -> Self {
        WorldFilter { fields: true, dungeons: true, high_ids: true, spawn_dots: true }
    }
}

impl WorldFilter {
    /// Whether the card of a map shows (`dungeon`: None when the client does not say).
    pub fn shows(&self, id: u32, dungeon: Option<bool>) -> bool {
        if id >= 100 && !self.high_ids {
            return false;
        }
        match dungeon {
            Some(true) => self.dungeons,
            Some(false) => self.fields,
            None => true,
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ stats

/// The numbers on a card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardStats {
    pub id: u32,
    pub size: u32,
    pub areas: usize,
    /// Total count of monsters.
    pub monsters: u32,
    /// The map's own level range (count-weighted 10th to 90th percentile), when levels are known.
    pub level: Option<(u32, u32)>,
    /// Portals that lead to another map, and portals of other maps that lead here.
    pub out: usize,
    pub inn: usize,
}

pub fn stats(id: u32, map: &Svmap, env: &Env) -> CardStats {
    CardStats {
        id,
        size: map.size,
        areas: map.mob_areas.len(),
        monsters: map.mob_areas.iter().flat_map(|a| a.mobs.iter()).map(|e| e.count).sum(),
        level: spawns::derived_band(map, env),
        out: map.portals.iter().filter(|p| p.to_map != id).count(),
        inn: 0,
    }
}

/// Fills `inn` of every card from the links.
pub fn count_in(cards: &mut [CardStats], links: &[Link]) {
    for c in cards.iter_mut() {
        c.inn = links.iter().filter(|l| l.to == c.id && l.from != c.id).map(|l| l.portals.len()).sum();
    }
}

/// The three text lines of a card: `1  Erina (Alliance of Light)`, `F 2048 · 835 areas · 1,747 monsters`, `lv 1-9 · 7 out · 6 in`.
pub fn card_lines(s: &CardStats, name: &str, dungeon: Option<bool>) -> [String; 3] {
    let kind = match dungeon {
        Some(true) => "D",
        Some(false) => "F",
        None => "-",
    };
    let l1 = if name.is_empty() { format!("{}", s.id) } else { format!("{}  {name}", s.id) };
    let l2 = format!("{kind} {} \u{b7} {} areas \u{b7} {} monsters", s.size, thousands(s.areas), thousands(s.monsters as usize));
    let lv = s.level.map_or(String::new(), |(a, b)| format!("lv {a}\u{2013}{b} \u{b7} "));
    let l3 = format!("{lv}{} out \u{b7} {} in", s.out, s.inn);
    [l1, l2, l3]
}

// ------------------------------------------------------------------------------------------------------------------ links

/// All portals of map `from` that lead to map `to` (one directed pair).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub from: u32,
    pub to: u32,
    /// Indices of the portals on `from`.
    pub portals: Vec<usize>,
    /// The destination has a portal that leads back to `from`.
    pub back: bool,
}

/// One directed link per pair of maps (a portal to the same map is a link with `from == to`), in id order. Destinations that are
/// not in `maps` are kept: the view draws a stub for them.
pub fn links(maps: &[(u32, &Svmap)]) -> Vec<Link> {
    let by_id: BTreeMap<u32, &Svmap> = maps.iter().copied().collect();
    let mut pairs: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (id, m) in &by_id {
        for (i, p) in m.portals.iter().enumerate() {
            pairs.entry((*id, p.to_map)).or_default().push(i);
        }
    }
    pairs
        .into_iter()
        .map(|((from, to), portals)| {
            let back = from != to && by_id.get(&to).is_some_and(|d| d.portals.iter().any(|q| q.to_map == from));
            Link { from, to, portals, back }
        })
        .collect()
}

/// A line the view draws: a pair of maps with the portals each way (a two-way pair is one line with two heads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The smaller id first (a loop has both equal).
    pub a: u32,
    pub b: u32,
    pub a_to_b: Vec<usize>,
    pub b_to_a: Vec<usize>,
}

impl Line {
    pub fn total(&self) -> usize {
        self.a_to_b.len() + self.b_to_a.len()
    }

    pub fn is_loop(&self) -> bool {
        self.a == self.b
    }

    pub fn two_way(&self) -> bool {
        !self.a_to_b.is_empty() && !self.b_to_a.is_empty() && !self.is_loop()
    }

    /// Line width in pixels: 1 to 4 by the number of portals.
    pub fn width(&self) -> f32 {
        (self.total() as f32).clamp(1.0, 4.0)
    }

    pub fn touches(&self, id: u32) -> bool {
        self.a == id || self.b == id
    }

    /// `1 -> 18: 2 portals (#0, #3)`, both ways joined with `; ` for a two-way line.
    pub fn text(&self) -> String {
        let one = |from: u32, to: u32, v: &[usize]| {
            let list: Vec<String> = v.iter().map(|i| format!("#{i}")).collect();
            format!("{from} \u{2192} {to}: {} portal{} ({})", v.len(), if v.len() == 1 { "" } else { "s" }, list.join(", "))
        };
        let mut parts = Vec::new();
        if !self.a_to_b.is_empty() {
            parts.push(one(self.a, self.b, &self.a_to_b));
        }
        if !self.b_to_a.is_empty() {
            parts.push(one(self.b, self.a, &self.b_to_a));
        }
        parts.join("; ")
    }
}

/// The links merged into lines.
pub fn lines(links: &[Link]) -> Vec<Line> {
    let mut by: BTreeMap<(u32, u32), Line> = BTreeMap::new();
    for l in links {
        let (a, b) = (l.from.min(l.to), l.from.max(l.to));
        let e = by.entry((a, b)).or_insert_with(|| Line { a, b, a_to_b: Vec::new(), b_to_a: Vec::new() });
        if l.from == a {
            e.a_to_b.extend(&l.portals);
        } else {
            e.b_to_a.extend(&l.portals);
        }
    }
    by.into_values().collect()
}

// ------------------------------------------------------------------------------------------------------------------ arrange

/// Card positions (top-left corner, world units) as the settings keep them: map id to [x, y].
pub type Positions = BTreeMap<u32, [f32; 2]>;

/// Fields first, then dungeons on their own rows, rows of 10 by id.
pub fn arrange_by_id(ids: &[u32], is_dungeon: &dyn Fn(u32) -> bool) -> Positions {
    let mut fields: Vec<u32> = ids.iter().copied().filter(|i| !is_dungeon(*i)).collect();
    let mut dungeons: Vec<u32> = ids.iter().copied().filter(|i| is_dungeon(*i)).collect();
    fields.sort_unstable();
    dungeons.sort_unstable();
    let mut out = Positions::new();
    let mut row = 0usize;
    for group in [fields, dungeons] {
        for (k, id) in group.iter().enumerate() {
            out.insert(*id, [(k % 10) as f32 * (CARD_W + GAP), (row + k / 10) as f32 * (CARD_H + GAP)]);
        }
        row += group.len().div_ceil(10);
    }
    out
}

/// Fruchterman-Reingold from the given positions: 400 iterations, fixed order, no random numbers (the same input always gives the
/// same output). Cards that overlap afterwards are pushed apart.
pub fn arrange_by_links(ids: &[u32], lines: &[Line], start: &Positions) -> Positions {
    let mut ids: Vec<u32> = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let n = ids.len();
    if n == 0 {
        return Positions::new();
    }
    let index: BTreeMap<u32, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut pos: Vec<[f32; 2]> = ids.iter().map(|id| start.get(id).copied().unwrap_or([0.0, 0.0])).collect();
    let edges: Vec<(usize, usize, f32)> = lines.iter().filter(|l| !l.is_loop()).filter_map(|l| Some((*index.get(&l.a)?, *index.get(&l.b)?, 1.0 + (l.total() as f32).ln()))).collect();
    // the frame is the one the by-id grid of this many cards fills
    let cols = n.min(10) as f32;
    let rows = n.div_ceil(10).max(1) as f32;
    let (w, h) = (cols * (CARD_W + GAP), rows * (CARD_H + GAP));
    let k = ((w * h) / n as f32).sqrt();
    let iterations = 400;
    for it in 0..iterations {
        let t = (w.min(h) / 8.0) * (1.0 - it as f32 / iterations as f32) + 0.5;
        let mut disp = vec![[0.0f32; 2]; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let (dx, dy) = (pos[i][0] - pos[j][0], pos[i][1] - pos[j][1]);
                let mut d = (dx * dx + dy * dy).sqrt();
                let (ux, uy) = if d < 0.01 {
                    // two cards on one spot: a fixed direction that depends on the order
                    d = 0.01;
                    (((i + j) % 2) as f32 * 2.0 - 1.0, 1.0)
                } else {
                    (dx / d, dy / d)
                };
                let f = k * k / d;
                disp[i][0] += ux * f;
                disp[i][1] += uy * f;
                disp[j][0] -= ux * f;
                disp[j][1] -= uy * f;
            }
        }
        for (a, b, wgt) in &edges {
            let (dx, dy) = (pos[*a][0] - pos[*b][0], pos[*a][1] - pos[*b][1]);
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let f = d * d / k * wgt;
            let (ux, uy) = (dx / d, dy / d);
            disp[*a][0] -= ux * f;
            disp[*a][1] -= uy * f;
            disp[*b][0] += ux * f;
            disp[*b][1] += uy * f;
        }
        for i in 0..n {
            let d = (disp[i][0] * disp[i][0] + disp[i][1] * disp[i][1]).sqrt().max(0.01);
            let m = d.min(t);
            pos[i][0] += disp[i][0] / d * m;
            pos[i][1] += disp[i][1] / d * m;
            pos[i][0] = pos[i][0].clamp(-w * 0.25, w * 1.25);
            pos[i][1] = pos[i][1].clamp(-h * 0.25, h * 1.25);
        }
    }
    separate(&mut pos);
    // shift so the top-left card sits at the origin
    let (mx, my) = pos.iter().fold((f32::MAX, f32::MAX), |a, p| (a.0.min(p[0]), a.1.min(p[1])));
    ids.iter().zip(pos).map(|(id, p)| (*id, [(p[0] - mx).round(), (p[1] - my).round()])).collect()
}

/// Pushes overlapping cards apart along the axis of least overlap, in a fixed order, until none overlap (or 200 rounds).
fn separate(pos: &mut [[f32; 2]]) {
    let (w, h) = (CARD_W + 10.0, CARD_H + 10.0);
    for _ in 0..200 {
        let mut moved = false;
        for i in 0..pos.len() {
            for j in (i + 1)..pos.len() {
                let (dx, dy) = (pos[j][0] - pos[i][0], pos[j][1] - pos[i][1]);
                let (ox, oy) = (w - dx.abs(), h - dy.abs());
                if ox > 0.0 && oy > 0.0 {
                    moved = true;
                    if ox < oy {
                        let s = if dx >= 0.0 { 1.0 } else { -1.0 };
                        pos[i][0] -= s * ox / 2.0 + 0.5;
                        pos[j][0] += s * ox / 2.0 + 0.5;
                    } else {
                        let s = if dy >= 0.0 { 1.0 } else { -1.0 };
                        pos[i][1] -= s * oy / 2.0 + 0.5;
                        pos[j][1] += s * oy / 2.0 + 0.5;
                    }
                }
            }
        }
        if !moved {
            break;
        }
    }
}

/// Positions for every id: saved ones win, the rest come from the by-id grid (shifted below the saved cards when some are saved).
pub fn positions(ids: &[u32], saved: &BTreeMap<String, [f32; 2]>, is_dungeon: &dyn Fn(u32) -> bool) -> Positions {
    let grid = arrange_by_id(ids, is_dungeon);
    let mut out = Positions::new();
    for id in ids {
        match saved.get(&id.to_string()).filter(|p| p[0].is_finite() && p[1].is_finite()) {
            Some(p) => {
                out.insert(*id, *p);
            }
            None => {
                if let Some(p) = grid.get(id) {
                    out.insert(*id, *p);
                }
            }
        }
    }
    out
}

pub fn to_saved(p: &Positions) -> BTreeMap<String, [f32; 2]> {
    p.iter().map(|(id, v)| (id.to_string(), *v)).collect()
}

// ------------------------------------------------------------------------------------------------------------------ pictures

/// A picture of the blocked grid as RGBA, `side` pixels square, row 0 = north (the largest z): free cells light, blocked dark.
pub fn blocked_picture(map: &Svmap, side: usize) -> Vec<u8> {
    let n = map.size as usize;
    let mut out = Vec::with_capacity(side * side * 4);
    if n == 0 || side == 0 {
        return vec![0; side * side * 4];
    }
    const SUB: usize = 3;
    for py in 0..side {
        for px in 0..side {
            let mut blocked = 0usize;
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let x = ((px * SUB + sx) * n / (side * SUB)).min(n - 1);
                    // pixel row 0 is the largest z
                    let z = n - 1 - ((py * SUB + sy) * n / (side * SUB)).min(n - 1);
                    if map.blocked.get(x as u32, z as u32) {
                        blocked += 1;
                    }
                }
            }
            let t = blocked as f32 / (SUB * SUB) as f32;
            let free = [214.0, 220.0, 226.0];
            let dark = [52.0, 58.0, 70.0];
            for i in 0..3 {
                out.push((free[i] + (dark[i] - free[i]) * t) as u8);
            }
            out.push(255);
        }
    }
    out
}

/// One dot per monster area: its centre as a fraction of the map (x right, z up) and the area's mean level.
pub fn spawn_dots(map: &Svmap, level_of: &dyn Fn(u32) -> Option<f32>) -> Vec<([f32; 2], Option<f32>)> {
    let size = map.size.max(1) as f32;
    map.mob_areas
        .iter()
        .map(|a| {
            let c = a.centre();
            let (mut sum, mut w) = (0.0f32, 0.0f32);
            for e in &a.mobs {
                if let Some(l) = level_of(e.mob) {
                    sum += l * e.count.max(1) as f32;
                    w += e.count.max(1) as f32;
                }
            }
            ([(c[0] / size).clamp(0.0, 1.0), (c[2] / size).clamp(0.0, 1.0)], (w > 0.0).then(|| sum / w))
        })
        .collect()
}

/// Scales an RGBA picture to `side` x `side` pixels by averaging the source pixels each one covers.
pub fn downscale_square(rgba: &[u8], w: usize, h: usize, side: usize) -> Vec<u8> {
    let mut out = vec![0u8; side * side * 4];
    if w == 0 || h == 0 || side == 0 || rgba.len() < w * h * 4 {
        return out;
    }
    for py in 0..side {
        let (y0, y1) = (py * h / side, ((py + 1) * h / side).max(py * h / side + 1).min(h));
        for px in 0..side {
            let (x0, x1) = (px * w / side, ((px + 1) * w / side).max(px * w / side + 1).min(w));
            let mut acc = [0u32; 4];
            let mut count = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    for c in 0..4 {
                        acc[c] += rgba[i + c] as u32;
                    }
                    count += 1;
                }
            }
            let o = (py * side + px) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / count.max(1)) as u8;
            }
        }
    }
    out
}

/// Every map id the cards stand for plus the destinations of portals that have no file (for the stubs): (missing ids, map 200 present).
pub fn stubs(ids: &BTreeSet<u32>, links: &[Link]) -> (Vec<u32>, bool) {
    let missing: BTreeSet<u32> = links.iter().map(|l| l.to).filter(|t| !ids.contains(t) && *t != GUILD_RANKING).collect();
    let guild = links.iter().any(|l| l.to == GUILD_RANKING && !ids.contains(&GUILD_RANKING));
    (missing.into_iter().collect(), guild)
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::{MobArea, MobEntry, Portal};

    fn portal(to: u32) -> Portal {
        Portal { pos: [0.0; 3], country: 0, level_min: 0, level_max: 0, to_map: to, to_pos: [0.0; 3] }
    }

    #[allow(clippy::type_complexity)]
    fn env_none() -> (impl Fn(u32) -> Option<(String, u32)>, impl Fn(u32, u32) -> Option<String>, impl Fn(u32) -> String, impl Fn(u32, usize) -> Option<String>) {
        (|_| None, |_, _| None, |id| format!("map {id}"), |_, _| None)
    }

    #[test]
    fn links_lines_and_stubs() {
        let mut a = Svmap::new(64);
        let mut b = Svmap::new(64);
        a.portals = vec![portal(2), portal(2), portal(200), portal(77), portal(1)];
        b.portals = vec![portal(1)];
        let ma = [(1u32, &a), (2u32, &b)];
        let l = links(&ma);
        assert_eq!(l.iter().map(|x| (x.from, x.to, x.portals.len(), x.back)).collect::<Vec<_>>(), vec![(1, 1, 1, false), (1, 2, 2, true), (1, 77, 1, false), (1, 200, 1, false), (2, 1, 1, true)]);
        let total: usize = l.iter().map(|x| x.portals.len()).sum();
        assert_eq!(total, 6);
        let ls = lines(&l);
        assert_eq!(ls.len(), 4);
        let pair = ls.iter().find(|x| x.a == 1 && x.b == 2).unwrap();
        assert!(pair.two_way());
        assert_eq!(pair.text(), "1 \u{2192} 2: 2 portals (#0, #1); 2 \u{2192} 1: 1 portal (#0)");
        assert_eq!(pair.width(), 3.0);
        assert!(ls.iter().any(|x| x.is_loop() && x.total() == 1));
        let ids: BTreeSet<u32> = [1, 2].into();
        assert_eq!(stubs(&ids, &l), (vec![77], true));
        let mut cards = vec![stats(1, &a, &Env { mob: &|_| None, npc: &|_, _| None, map_name: &|i| i.to_string(), region: &|_, _| None }), stats(2, &b, &Env { mob: &|_| None, npc: &|_, _| None, map_name: &|i| i.to_string(), region: &|_, _| None })];
        count_in(&mut cards, &l);
        assert_eq!((cards[0].out, cards[0].inn), (4, 1));
        assert_eq!((cards[1].out, cards[1].inn), (1, 2));
    }

    #[test]
    fn card_text_and_filter() {
        let mut m = Svmap::new(2048);
        m.mob_areas.push(MobArea { min: [0.0; 3], max: [2.0, 2.0, 2.0], mobs: vec![MobEntry { mob: 1, count: 1500 }] });
        let (mob, npc, name, region) = env_none();
        let env = Env { mob: &mob, npc: &npc, map_name: &name, region: &region };
        let s = stats(1, &m, &env);
        assert_eq!((s.areas, s.monsters, s.level), (1, 1500, None));
        let t = card_lines(&s, "Erina", Some(false));
        assert_eq!(t[0], "1  Erina");
        assert_eq!(t[1], "F 2048 \u{b7} 1 areas \u{b7} 1,500 monsters");
        assert_eq!(t[2], "0 out \u{b7} 0 in");
        let f = WorldFilter { dungeons: false, high_ids: false, ..Default::default() };
        assert!(f.shows(1, Some(false)) && !f.shows(1, Some(true)) && !f.shows(100, None) && f.shows(5, None));
    }

    #[test]
    fn by_id_grid_puts_fields_first_in_rows_of_ten() {
        let ids: Vec<u32> = (0..25).collect();
        let p = arrange_by_id(&ids, &|i| i >= 20);
        assert_eq!(p[&0], [0.0, 0.0]);
        assert_eq!(p[&9], [9.0 * (CARD_W + GAP), 0.0]);
        assert_eq!(p[&10], [0.0, CARD_H + GAP]);
        // 20 fields = 2 rows, dungeons start on row 2
        assert_eq!(p[&20], [0.0, 2.0 * (CARD_H + GAP)]);
    }

    #[test]
    fn by_links_is_deterministic_and_separates_cards() {
        let ids: Vec<u32> = (0..30).collect();
        let ls: Vec<Line> = (0..29).map(|i| Line { a: i, b: i + 1, a_to_b: vec![0], b_to_a: vec![0] }).chain([Line { a: 0, b: 29, a_to_b: vec![1], b_to_a: vec![] }]).collect();
        let start = arrange_by_id(&ids, &|_| false);
        let a = arrange_by_links(&ids, &ls, &start);
        let b = arrange_by_links(&ids, &ls, &start);
        assert_eq!(a, b);
        assert_eq!(a.len(), 30);
        for (i, p) in &a {
            for (j, q) in &a {
                if i < j {
                    assert!((p[0] - q[0]).abs() >= CARD_W || (p[1] - q[1]).abs() >= CARD_H, "cards {i} and {j} overlap");
                }
            }
        }
        // linked cards end closer than the average pair
        let d = |x: u32, y: u32| ((a[&x][0] - a[&y][0]).powi(2) + (a[&x][1] - a[&y][1]).powi(2)).sqrt();
        let linked: f32 = (0..29).map(|i| d(i, i + 1)).sum::<f32>() / 29.0;
        let far: f32 = (0..15).map(|i| d(i, i + 15)).sum::<f32>() / 15.0;
        assert!(linked < far, "{linked} {far}");
        // saved positions win over the grid
        let mut saved = BTreeMap::new();
        saved.insert("3".to_string(), [5.0, 6.0]);
        let p = positions(&ids, &saved, &|_| false);
        assert_eq!(p[&3], [5.0, 6.0]);
        assert_eq!(p[&4], arrange_by_id(&ids, &|_| false)[&4]);
        assert_eq!(to_saved(&p)["3"], [5.0, 6.0]);
    }

    fn originals() -> Option<Vec<(u32, Svmap)>> {
        let dir = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?;
        let files = crate::folderdiff::list(std::path::Path::new(&dir));
        Some(files.into_iter().map(|(id, p)| (id, shaiya_svmap::read(&std::fs::read(p).unwrap()).unwrap())).collect())
    }

    /// The card totals are the areas and monsters summed directly over the 77 files.
    #[test]
    fn world_stats_match_folder() {
        let Some(maps) = originals() else { return };
        assert_eq!(maps.len(), 77);
        let (mob, npc, name, region) = env_none();
        let env = Env { mob: &mob, npc: &npc, map_name: &name, region: &region };
        let (mut areas, mut monsters) = (0usize, 0u64);
        for (_, m) in &maps {
            for a in &m.mob_areas {
                areas += 1;
                for e in &a.mobs {
                    monsters += e.count as u64;
                }
            }
        }
        let cards: Vec<CardStats> = maps.iter().map(|(id, m)| stats(*id, m, &env)).collect();
        assert_eq!(cards.iter().map(|c| c.areas).sum::<usize>(), areas);
        assert_eq!(cards.iter().map(|c| c.monsters as u64).sum::<u64>(), monsters);
        assert_eq!((areas, monsters), (12_062, 22_106), "the numbers of the 77 originals");
    }

    /// Every portal is in exactly one link (a portal to its own map is a loop), and map 200 is a stub.
    #[test]
    fn world_links_originals() {
        let Some(maps) = originals() else { return };
        let refs: Vec<(u32, &Svmap)> = maps.iter().map(|(i, m)| (*i, m)).collect();
        let l = links(&refs);
        let portals: usize = maps.iter().map(|(_, m)| m.portals.len()).sum();
        let in_links: usize = l.iter().map(|x| x.portals.len()).sum();
        let loops: usize = l.iter().filter(|x| x.from == x.to).map(|x| x.portals.len()).sum();
        assert_eq!(in_links, portals);
        assert_eq!(portals, 265, "loops {loops}");
        let ids: BTreeSet<u32> = maps.iter().map(|(i, _)| *i).collect();
        let (missing, guild) = stubs(&ids, &l);
        assert!(guild, "map 200 has a stub");
        let ls = lines(&l);
        assert_eq!(ls.iter().map(|x| x.total()).sum::<usize>(), portals);
        eprintln!("world links: {} directed links, {} lines, {loops} loops, missing destinations {missing:?}", l.len(), ls.len());
    }

    #[test]
    fn pictures() {
        let mut m = Svmap::new(64);
        m.blocked.fill_rect(0, 0, 32, 64, true);
        let p = blocked_picture(&m, 8);
        assert_eq!(p.len(), 8 * 8 * 4);
        // the left half is blocked (dark), the right half free (light)
        assert!(p[0] < 100 && p[7 * 4] > 180);
        let dots = spawn_dots(
            &{
                let mut s = Svmap::new(100);
                s.mob_areas.push(MobArea { min: [10.0, 0.0, 20.0], max: [30.0, 2.0, 40.0], mobs: vec![MobEntry { mob: 5, count: 2 }, MobEntry { mob: 6, count: 2 }] });
                s
            },
            &|id| Some(id as f32 * 10.0),
        );
        assert_eq!(dots.len(), 1);
        assert_eq!(dots[0].0, [0.2, 0.3]);
        assert_eq!(dots[0].1, Some(55.0));
        let src: Vec<u8> = (0..4 * 4).flat_map(|i| [i as u8 * 10, 0, 0, 255]).collect();
        let d = downscale_square(&src, 4, 4, 2);
        assert_eq!(d.len(), 16);
        assert_eq!(d[0], ((10 + 40 + 50) / 4) as u8);
    }
}
