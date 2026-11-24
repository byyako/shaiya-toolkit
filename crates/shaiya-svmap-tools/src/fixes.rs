//! Fixes for Problems findings. Pure: a finding and what the map and the client know go in, a
//! list of fixes comes out, each one either ops to commit (one undo step) or a request the app answers with a picker, the Sync
//! window or a delete. Nothing here changes how many portals or regions a map has, or their order: those changes belong to Sync
//! from client, which asks first.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use shaiya_clientdata::wld::Wld;
use shaiya_svmap::{Aabb, BitGrid, Finding, NpcPoint, SectionId, Svmap};

use crate::doc::Sel;
use crate::edit::{self, Op, Rec};
use crate::sync::{self, Target};

/// How far (in cells, each way) the nearest-free-cell search looks.
pub const FREE_CELL_RADIUS: i64 = 256;

pub enum FixKind {
    /// Ops to commit as one undo step.
    Ops(Vec<Op>),
    /// Open the monster picker for entry `entry` of the area (an entry past the end adds one).
    PickMob { area: usize, entry: usize },
    PickNpc { npc: usize },
    /// Open "Pick on map" for the portal's destination.
    PickDest { portal: usize },
    /// Open Tools > Sync from client.
    OpenSync,
    Delete(Sel),
}

pub struct Fix {
    pub title: String,
    pub kind: FixKind,
    /// A "Fix all N" over every finding of the rule may use it: the same click gives the same result every time.
    pub bulk: bool,
}

impl Fix {
    fn ops(title: impl Into<String>, op: Option<Op>) -> Option<Fix> {
        op.map(|o| Fix { title: title.into(), kind: FixKind::Ops(vec![o]), bulk: true })
    }

    fn ask(title: impl Into<String>, kind: FixKind) -> Fix {
        Fix { title: title.into(), kind, bulk: false }
    }
}

/// What the fixes may look at.
#[derive(Clone, Copy)]
pub struct FixCtx<'a> {
    pub map: &'a Svmap,
    pub map_id: Option<u32>,
    /// The other maps of the open folder (for destinations).
    pub folder: &'a HashMap<u32, Arc<Svmap>>,
    pub wld: Option<&'a Wld>,
    /// `sync::target_from_wld(wld)`, made once.
    pub target: Option<&'a Target>,
    /// Where a new point or box goes (x, z).
    pub view_centre: [f32; 2],
    pub ground: &'a dyn Fn(f32, f32) -> f32,
}

impl<'a> FixCtx<'a> {
    /// The map `id` portal destinations point to: this one when it is the same map, else the folder's copy.
    fn dest_map(&self, id: u32) -> Option<&'a Svmap> {
        if Some(id) == self.map_id {
            Some(self.map)
        } else {
            self.folder.get(&id).map(|a| &**a)
        }
    }
}

/// Owned inputs of a [`FixCtx`] (the app keeps one of these per frame; the context borrows it).
pub struct FixEnv {
    pub folder: HashMap<u32, Arc<Svmap>>,
    pub wld: Option<Arc<Wld>>,
    pub target: Option<Arc<Target>>,
    pub view_centre: [f32; 2],
    pub ground: Box<dyn Fn(f32, f32) -> f32>,
}

impl FixEnv {
    pub fn ctx<'a>(&'a self, map: &'a Svmap, map_id: Option<u32>) -> FixCtx<'a> {
        FixCtx { map, map_id, folder: &self.folder, wld: self.wld.as_deref(), target: self.target.as_deref(), view_centre: self.view_centre, ground: &*self.ground }
    }
}

// ------------------------------------------------------------------------------------------------------------------ helpers

fn changed(i: usize, before: Rec, after: Rec) -> Option<Op> {
    if edit::same_bits(&before, &after) {
        None
    } else {
        Some(Op::replace(i, before, after))
    }
}

fn clamp_point(p: &mut [f32; 3], size: u32) {
    let hi = (size as f32).next_down();
    for axis in [0, 2] {
        if p[axis].is_finite() {
            p[axis] = p[axis].clamp(0.0, hi);
        }
    }
}

fn clamp_record(r: &mut Rec, size: u32) {
    match r {
        Rec::A(a) => clamp_point(&mut a.pos, size),
        Rec::B(p) => clamp_point(p, size),
        Rec::Mob(a) => {
            clamp_point(&mut a.min, size);
            clamp_point(&mut a.max, size);
        }
        Rec::Npc(n) => n.points.iter_mut().for_each(|p| clamp_point(&mut p.pos, size)),
        Rec::Portal(p) => clamp_point(&mut p.pos, size),
        Rec::Bind(b) => {
            for x in &mut b.boxes {
                clamp_point(&mut x.min, size);
                clamp_point(&mut x.max, size);
            }
        }
        Rec::Region(g) => {
            clamp_point(&mut g.min, size);
            clamp_point(&mut g.max, size);
        }
    }
}

fn unswap(min: &mut [f32; 3], max: &mut [f32; 3]) {
    for a in 0..3 {
        if min[a] > max[a] {
            std::mem::swap(&mut min[a], &mut max[a]);
        }
    }
}

/// The cell nearest to (x, z) that is free, searching 4-connected over the grid up to `radius` cells away. `None` when (x, z) is
/// outside the map or nothing in range is free. A free start cell is its own answer.
pub fn nearest_free(grid: &BitGrid, size: u32, x: f32, z: f32, radius: i64) -> Option<(u32, u32)> {
    if !(x.is_finite() && z.is_finite()) || x < 0.0 || z < 0.0 || x >= size as f32 || z >= size as f32 || grid.size() != size {
        return None;
    }
    let (sx, sz) = (x as i64, z as i64);
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    let mut q: VecDeque<(i64, i64)> = VecDeque::new();
    q.push_back((sx, sz));
    seen.insert((sx, sz));
    while let Some((cx, cz)) = q.pop_front() {
        if !grid.get(cx as u32, cz as u32) {
            return Some((cx as u32, cz as u32));
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (nx, nz) = (cx + dx, cz + dz);
            if nx < 0 || nz < 0 || nx >= size as i64 || nz >= size as i64 || (nx - sx).abs() > radius || (nz - sz).abs() > radius {
                continue;
            }
            if seen.insert((nx, nz)) {
                q.push_back((nx, nz));
            }
        }
    }
    None
}

/// The shift that puts an anchor at the centre of the nearest free cell of `grid`.
fn free_shift(grid: &BitGrid, size: u32, anchor: [f32; 2]) -> Option<(f32, f32)> {
    let (cx, cz) = nearest_free(grid, size, anchor[0], anchor[1], FREE_CELL_RADIUS)?;
    Some((cx as f32 + 0.5 - anchor[0], cz as f32 + 0.5 - anchor[1]))
}

const ASK_CELL: &str = "Move to the nearest free cell";

/// The fixes of one finding (empty when there is none to offer).
pub fn fixes_for(f: &Finding, cx: &FixCtx) -> Vec<Fix> {
    let mut out: Vec<Fix> = Vec::new();
    let sync_all = || Fix::ask("Open Sync from client...", FixKind::OpenSync);
    let Some(i) = f.index else {
        if matches!(f.code, "portal-count" | "npc-count" | "region-count" | "bind-count") && cx.wld.is_some() {
            out.push(sync_all());
        }
        return out;
    };
    let Some(rec) = edit::get(cx.map, f.section, i) else { return out };
    let size = cx.map.size;
    let sub = f.sub;
    let take_wld = |out: &mut Vec<Fix>| {
        if let Some(t) = cx.target {
            if let Ok(Some(op)) = sync::single_record_op(cx.map, t, f.section, i) {
                out.push(Fix { title: "Take the .wld value".into(), kind: FixKind::Ops(vec![op]), bulk: true });
            }
        }
    };
    match (f.code, &rec) {
        ("pos-out-of-range" | "region-out-of-range", r) => {
            let mut after = r.clone();
            clamp_record(&mut after, size);
            out.extend(Fix::ops("Clamp inside the map", changed(i, r.clone(), after)));
        }
        ("box-inverted", Rec::Mob(a)) => {
            let mut b = a.clone();
            unswap(&mut b.min, &mut b.max);
            out.extend(Fix::ops("Swap min and max", changed(i, rec.clone(), Rec::Mob(b))));
        }
        ("box-inverted", Rec::Region(g)) => {
            let mut b = *g;
            unswap(&mut b.min, &mut b.max);
            out.extend(Fix::ops("Swap min and max", changed(i, rec.clone(), Rec::Region(b))));
        }
        ("box-inverted", Rec::Bind(b)) => {
            let mut nb = b.clone();
            if let Some(x) = nb.boxes.get_mut(sub.unwrap_or(0)) {
                unswap(&mut x.min, &mut x.max);
            }
            out.extend(Fix::ops("Swap min and max", changed(i, rec.clone(), Rec::Bind(nb))));
        }
        ("area-empty", Rec::Mob(_)) => {
            out.push(Fix::ask("Pick a monster...", FixKind::PickMob { area: i, entry: 0 }));
            out.push(Fix::ask("Delete the area", FixKind::Delete((SectionId::MobAreas, i))));
        }
        ("area-count-zero" | "area-count-high", Rec::Mob(a)) => {
            let (to, title) = if f.code == "area-count-zero" { (1, "Set the count to 1") } else { (8, "Set the count to 8") };
            let mut b = a.clone();
            if let Some(e) = b.mobs.get_mut(sub.unwrap_or(0)) {
                e.count = to;
            }
            out.extend(Fix::ops(title, changed(i, rec.clone(), Rec::Mob(b))));
        }
        ("area-mob-zero" | "mob-unknown", Rec::Mob(_)) => {
            out.push(Fix::ask("Pick a monster...", FixKind::PickMob { area: i, entry: sub.unwrap_or(0) }));
        }
        ("area-on-blocked", Rec::Mob(a)) => {
            let c = a.centre();
            if let Some((dx, dz)) = free_shift(&cx.map.blocked, size, [c[0], c[2]]) {
                let mut after = rec.clone();
                edit::translate(&mut after, dx, dz);
                out.extend(Fix::ops(ASK_CELL, changed(i, rec.clone(), after)));
            }
        }
        ("npc-no-points", Rec::Npc(n)) => {
            let [x, z] = cx.view_centre;
            let mut b = n.clone();
            let y = (cx.ground)(x, z);
            b.points.push(NpcPoint { pos: [x, if y.is_finite() { y } else { 0.0 }, z], dir: 0.0 });
            out.push(Fix { title: "Add a point at the view centre".into(), kind: FixKind::Ops(vec![Op::replace(i, rec.clone(), Rec::Npc(b))]), bulk: false });
            out.push(Fix::ask("Delete the NPC", FixKind::Delete((SectionId::Npcs, i))));
        }
        ("npc-on-blocked", Rec::Npc(n)) => {
            if let Some(p) = n.points.first() {
                if let Some((dx, dz)) = free_shift(&cx.map.blocked, size, [p.pos[0], p.pos[2]]) {
                    let mut after = rec.clone();
                    edit::translate(&mut after, dx, dz);
                    out.extend(Fix::ops(ASK_CELL, changed(i, rec.clone(), after)));
                }
            }
        }
        ("npc-unknown", Rec::Npc(_)) => out.push(Fix::ask("Pick an NPC...", FixKind::PickNpc { npc: i })),
        ("portal-levels", Rec::Portal(p)) => {
            let mut b = *p;
            std::mem::swap(&mut b.level_min, &mut b.level_max);
            out.extend(Fix::ops("Swap the two levels", changed(i, rec.clone(), Rec::Portal(b))));
        }
        ("portal-country", Rec::Portal(p)) => {
            let mut b = *p;
            b.country = 0;
            out.extend(Fix::ops("Set the country to 0 (any)", changed(i, rec.clone(), Rec::Portal(b))));
        }
        ("portal-map-missing", Rec::Portal(_)) => out.push(Fix::ask("Pick the destination...", FixKind::PickDest { portal: i })),
        ("portal-dest-outside", Rec::Portal(p)) => {
            if let Some(dest) = cx.dest_map(p.to_map) {
                let mut b = *p;
                clamp_point(&mut b.to_pos, dest.size);
                out.extend(Fix::ops("Clamp the destination inside its map", changed(i, rec.clone(), Rec::Portal(b))));
            }
            out.push(Fix::ask("Pick the destination...", FixKind::PickDest { portal: i }));
        }
        ("portal-dest-blocked", Rec::Portal(p)) => {
            if let Some(dest) = cx.dest_map(p.to_map) {
                if let Some((dx, dz)) = free_shift(&dest.blocked, dest.size, [p.to_pos[0], p.to_pos[2]]) {
                    let mut b = *p;
                    b.to_pos[0] += dx;
                    b.to_pos[2] += dz;
                    out.extend(Fix::ops("Move the destination to the nearest free cell", changed(i, rec.clone(), Rec::Portal(b))));
                }
            }
        }
        ("portal-far", Rec::Portal(p)) => {
            if let Some(w) = cx.wld.and_then(|w| w.portals.get(i)) {
                let mut b = *p;
                b.pos = sync::portal_from_wld(w).pos;
                out.extend(Fix::ops("Move to the .wld portal box centre", changed(i, rec.clone(), Rec::Portal(b))));
            }
        }
        ("bind-no-box", Rec::Bind(b)) => {
            let [x, z] = cx.view_centre;
            let y = (cx.ground)(x, z);
            let y = if y.is_finite() { y } else { 0.0 };
            let mut nb = b.clone();
            nb.boxes.push(Aabb { min: [x - 8.0, y, z - 8.0], max: [x + 8.0, y + 2.0, z + 8.0] });
            out.push(Fix { title: "Add a box at the view centre".into(), kind: FixKind::Ops(vec![Op::replace(i, rec.clone(), Rec::Bind(nb))]), bulk: false });
            out.push(Fix::ask("Delete the bind area", FixKind::Delete((SectionId::Binds, i))));
        }
        ("bind-country", Rec::Bind(b)) => {
            let mut nb = b.clone();
            nb.country = 0;
            out.extend(Fix::ops("Set the country to 0", changed(i, rec.clone(), Rec::Bind(nb))));
        }
        ("bind-on-blocked", Rec::Bind(b)) => {
            let k = sub.unwrap_or(0);
            if let Some(x) = b.boxes.get(k) {
                let c = x.centre();
                if let Some((dx, dz)) = free_shift(&cx.map.blocked, size, [c[0], c[2]]) {
                    let mut nb = b.clone();
                    let bx = &mut nb.boxes[k];
                    for p in [&mut bx.min, &mut bx.max] {
                        p[0] += dx;
                        p[2] += dz;
                    }
                    out.extend(Fix::ops(ASK_CELL, changed(i, rec.clone(), Rec::Bind(nb))));
                }
            }
        }
        ("portal-field", Rec::Portal(_)) | ("npc-differs", Rec::Npc(_)) | ("region-differs", Rec::Region(_)) | ("bind-differs", Rec::Bind(_)) => take_wld(&mut out),
        ("npc-not-in-client", Rec::Npc(_)) if cx.wld.is_some() => out.push(sync_all()),
        _ => {}
    }
    out
}

/// The ops of "Fix all": every finding in `findings` (one rule, one map) gets its first bulk fix, each worked out on the map as the
/// earlier ones left it, so a record with two findings is fixed once. Nothing is applied here.
pub fn fix_all(findings: &[&Finding], cx: &FixCtx) -> Vec<Op> {
    let mut work = cx.map.clone();
    let mut ops = Vec::new();
    for f in findings {
        let c = FixCtx { map: &work, ..*cx };
        let Some(fix) = fixes_for(f, &c).into_iter().find(|x| x.bulk) else { continue };
        if let FixKind::Ops(o) = fix.kind {
            for op in &o {
                op.apply(&mut work);
            }
            ops.extend(o);
        }
    }
    ops
}


/// The rules whose fix is the same one-click change for every finding, so the Problems group offers "Fix all".
pub const BULK_CODES: &[&str] = &[
    "pos-out-of-range",
    "region-out-of-range",
    "box-inverted",
    "area-count-zero",
    "area-count-high",
    "area-on-blocked",
    "npc-on-blocked",
    "portal-levels",
    "portal-country",
    "portal-dest-outside",
    "portal-dest-blocked",
    "portal-far",
    "bind-country",
    "bind-on-blocked",
    "portal-field",
    "npc-differs",
    "region-differs",
    "bind-differs",
];

pub fn is_bulk(code: &str) -> bool {
    BULK_CODES.contains(&code)
}

/// The label of the undo step for one fix: `fix: <title> (#idx)`.
pub fn step_label(title: &str, f: &Finding) -> String {
    match f.index {
        Some(i) => format!("fix: {} (#{i})", title.to_lowercase()),
        None => format!("fix: {}", title.to_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use shaiya_svmap::{validate, validate_cross, Bind, MobArea, MobEntry, NpcEntry, Portal, Region};

    fn area(x0: f32, z0: f32, x1: f32, z1: f32, mobs: Vec<(u32, u32)>) -> MobArea {
        MobArea { min: [x0, 0.0, z0], max: [x1, 2.0, z1], mobs: mobs.into_iter().map(|(mob, count)| MobEntry { mob, count }).collect() }
    }

    fn findings(m: &Svmap, folder: &HashMap<u32, Arc<Svmap>>, own: u32) -> Vec<Finding> {
        let mut f = validate(m);
        f.extend(validate_cross(m, &|id| if id == own { Some(m) } else { folder.get(&id).map(|a| &**a) }));
        f
    }

    fn count(fs: &[Finding], code: &str) -> usize {
        fs.iter().filter(|f| f.code == code).count()
    }

    /// Applies the first bulk fix of every finding with `code`, as a document would (one step), checks the finding is gone and
    /// that undo gives the original bytes. Returns the map after.
    fn fixed(m: Svmap, folder: &HashMap<u32, Arc<Svmap>>, code: &str) -> Svmap {
        let ground = |_: f32, _: f32| 0.0;
        let before = findings(&m, folder, 1);
        let hits: Vec<&Finding> = before.iter().filter(|f| f.code == code).collect();
        assert!(!hits.is_empty(), "the map has no {code} finding to start with: {before:?}");
        let cx = FixCtx { map: &m, map_id: Some(1), folder, wld: None, target: None, view_centre: [32.0, 32.0], ground: &ground };
        let ops = fix_all(&hits, &cx);
        assert!(!ops.is_empty(), "{code}: no fix was offered");
        let orig_bytes = shaiya_svmap::write(&m);
        let mut d = Doc::new(1, Some(1), None, m, None);
        d.commit(step_label("test", hits[0]), ops);
        assert_eq!(d.undo.len(), 1, "one undo step");
        assert_eq!(count(&findings(&d.map, folder, 1), code), 0, "{code} is gone after the fix");
        let after = d.map.clone();
        d.undo();
        assert_eq!(shaiya_svmap::write(&d.map), orig_bytes, "undo restores the bytes");
        after
    }

    #[test]
    fn fixes_clamp_and_swap() {
        let none = HashMap::new();
        let mut m = Svmap::new(100);
        m.mob_areas.push(area(-5.0, 10.0, 20.0, 150.0, vec![(1, 1)]));
        let after = fixed(m, &none, "pos-out-of-range");
        assert_eq!(after.mob_areas[0].min[0], 0.0);
        assert!(after.mob_areas[0].max[2] < 100.0 && after.mob_areas[0].max[2] > 99.9);

        let mut m = Svmap::new(100);
        m.mob_areas.push(area(30.0, 10.0, 20.0, 40.0, vec![(1, 1)]));
        m.regions.push(Region { min: [50.0, 0.0, 60.0], max: [40.0, 2.0, 30.0], kind: 0, b: 0, c: 0, d: 0 });
        m.binds.push(Bind { country: 0, attr: 0, boxes: vec![Aabb { min: [9.0, 0.0, 1.0], max: [2.0, 2.0, 5.0] }] });
        let after = fixed(m, &none, "box-inverted");
        assert_eq!((after.mob_areas[0].min[0], after.mob_areas[0].max[0]), (20.0, 30.0));
        assert_eq!((after.regions[0].min[2], after.regions[0].max[2]), (30.0, 60.0));
        assert_eq!((after.binds[0].boxes[0].min[0], after.binds[0].boxes[0].max[0]), (2.0, 9.0));
        // a region corner past the edge is a warning with its own code and the same fix
        let mut m = Svmap::new(100);
        m.regions.push(Region { min: [-16.0, 0.0, 10.0], max: [20.0, 2.0, 30.0], kind: 0, b: 0, c: 0, d: 0 });
        let after = fixed(m, &none, "region-out-of-range");
        assert_eq!(after.regions[0].min[0], 0.0);
    }

    #[test]
    fn fixes_counts_levels_countries() {
        let none = HashMap::new();
        let mut m = Svmap::new(100);
        m.mob_areas.push(area(10.0, 10.0, 20.0, 20.0, vec![(1, 0), (2, 5)]));
        let after = fixed(m, &none, "area-count-zero");
        assert_eq!(after.mob_areas[0].mobs[0].count, 1);
        assert_eq!(after.mob_areas[0].mobs[1].count, 5);
        let mut m = Svmap::new(100);
        m.mob_areas.push(area(10.0, 10.0, 20.0, 20.0, vec![(1, 12)]));
        assert_eq!(fixed(m, &none, "area-count-high").mob_areas[0].mobs[0].count, 8);
        let mut m = Svmap::new(100);
        m.portals.push(Portal { pos: [5.0, 0.0, 5.0], country: 1, level_min: 40, level_max: 10, to_map: 1, to_pos: [9.0, 0.0, 9.0] });
        m.portals.push(Portal { pos: [6.0, 0.0, 6.0], country: 7, level_min: 1, level_max: 10, to_map: 1, to_pos: [9.0, 0.0, 9.0] });
        let after = fixed(m.clone(), &none, "portal-levels");
        assert_eq!((after.portals[0].level_min, after.portals[0].level_max), (10, 40));
        assert_eq!(fixed(m, &none, "portal-country").portals[1].country, 0);
        let mut m = Svmap::new(100);
        m.binds.push(Bind { country: 9, attr: 0, boxes: vec![Aabb { min: [1.0, 0.0, 1.0], max: [5.0, 2.0, 5.0] }] });
        assert_eq!(fixed(m, &none, "bind-country").binds[0].country, 0);
    }

    #[test]
    fn nearest_free_cell() {
        // a wall (column x = 10) with the start cell in it, free ground two cells to the right
        let mut g = BitGrid::new(32);
        for z in 0..32 {
            for x in 8..13 {
                g.set(x, z, true);
            }
        }
        assert_eq!(nearest_free(&g, 32, 9.4, 5.5, 256), Some((7, 5)), "west is nearer");
        assert_eq!(nearest_free(&g, 32, 10.4, 5.5, 256), Some((13, 5)), "a tie goes east: the search tries +x first");
        assert_eq!(nearest_free(&g, 32, 11.4, 5.5, 256), Some((13, 5)));
        assert_eq!(nearest_free(&g, 32, 3.0, 3.0, 256), Some((3, 3)), "a free start is its own answer");
        assert_eq!(nearest_free(&g, 32, -1.0, 3.0, 256), None, "outside the map");
        // radius: nothing free within 1 cell of the middle of the wall
        assert_eq!(nearest_free(&g, 32, 10.5, 5.5, 1), None);
        // all blocked: no fix
        let mut full = BitGrid::new(16);
        full.fill_all(true);
        assert_eq!(nearest_free(&full, 16, 5.0, 5.0, 256), None);
    }

    #[test]
    fn fixes_on_blocked_cells_land_on_free_cell_centres() {
        let none = HashMap::new();
        let mut m = Svmap::new(64);
        for z in 0..64 {
            for x in 28..36 {
                m.blocked.set(x, z, true);
            }
        }
        // an area centred in the wall, an NPC and a bind box too
        m.mob_areas.push(area(29.0, 10.0, 35.0, 16.0, vec![(1, 1)])); // centre (32, 13)
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [30.2, 1.0, 20.7], dir: 0.0 }, NpcPoint { pos: [40.0, 1.0, 25.0], dir: 0.0 }] });
        m.binds.push(Bind { country: 0, attr: 0, boxes: vec![Aabb { min: [30.0, 0.0, 40.0], max: [34.0, 2.0, 44.0] }] });
        let after = fixed(m.clone(), &none, "area-on-blocked");
        let c = after.mob_areas[0].centre();
        assert!(!after.blocked.get(c[0] as u32, c[2] as u32));
        assert_eq!(((c[0] - c[0].floor()) * 10.0).round(), 5.0, "the centre is a cell centre");
        assert_eq!(after.mob_areas[0].max[0] - after.mob_areas[0].min[0], 6.0, "the box keeps its size");
        let after = fixed(m.clone(), &none, "npc-on-blocked");
        let p = after.npcs[0].points[0].pos;
        assert!(!after.blocked.get(p[0] as u32, p[2] as u32));
        assert!((after.npcs[0].points[1].pos[0] - p[0] - (40.0 - 30.2)).abs() < 1e-3, "the path moves with the first point");
        let after = fixed(m, &none, "bind-on-blocked");
        let c = after.binds[0].boxes[0].centre();
        assert!(!after.blocked.get(c[0] as u32, c[2] as u32));
    }

    #[test]
    fn destination_fixes_use_the_destination_map() {
        let mut dest = Svmap::new(40);
        for z in 0..40 {
            for x in 0..40 {
                if (10..20).contains(&x) {
                    dest.blocked.set(x, z, true);
                }
            }
        }
        let mut folder = HashMap::new();
        folder.insert(2, Arc::new(dest));
        let mut m = Svmap::new(100);
        m.portals.push(Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map: 2, to_pos: [14.5, 0.0, 20.5] }); // blocked
        m.portals.push(Portal { pos: [6.0, 0.0, 6.0], country: 0, level_min: 0, level_max: 0, to_map: 2, to_pos: [90.0, 0.0, 90.0] }); // outside map 2
        let after = fixed(m.clone(), &folder, "portal-dest-blocked");
        let p = after.portals[0].to_pos;
        assert_eq!((p[0], p[2]), (9.5, 20.5), "the destination moved west to the nearest free cell centre");
        let after = fixed(m, &folder, "portal-dest-outside");
        assert!(after.portals[1].to_pos[0] < 40.0 && after.portals[1].to_pos[2] < 40.0);
    }

    #[test]
    fn points_and_boxes_are_added_at_the_view_centre() {
        let mut m = Svmap::new(100);
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![] });
        m.binds.push(Bind { country: 0, attr: 0, boxes: vec![] });
        let ground = |_: f32, _: f32| 7.0;
        let none = HashMap::new();
        let cx = FixCtx { map: &m, map_id: Some(1), folder: &none, wld: None, target: None, view_centre: [40.0, 50.0], ground: &ground };
        let fs = findings(&m, &none, 1);
        let f = fs.iter().find(|f| f.code == "npc-no-points").unwrap();
        let fx = fixes_for(f, &cx);
        assert_eq!(fx.len(), 2);
        assert!(!fx[0].bulk, "adding at the view centre is not a bulk fix");
        let FixKind::Ops(ops) = &fx[0].kind else { panic!() };
        let mut after = m.clone();
        ops[0].apply(&mut after);
        assert_eq!(after.npcs[0].points[0].pos, [40.0, 7.0, 50.0]);
        assert!(matches!(fx[1].kind, FixKind::Delete((SectionId::Npcs, 0))));
        let f = fs.iter().find(|f| f.code == "bind-no-box").unwrap();
        let fx = fixes_for(f, &cx);
        let FixKind::Ops(ops) = &fx[0].kind else { panic!() };
        ops[0].apply(&mut after);
        assert_eq!(after.binds[0].boxes[0].min, [32.0, 7.0, 42.0]);
        assert_eq!(count(&findings(&after, &none, 1), "bind-no-box"), 0);
    }

    #[test]
    fn asking_fixes_and_the_ones_that_do_not_exist() {
        let mut m = Svmap::new(100);
        m.mob_areas.push(area(10.0, 10.0, 20.0, 20.0, vec![]));
        m.mob_areas.push(area(10.0, 10.0, 20.0, 20.0, vec![(0, 1)]));
        m.portals.push(Portal { pos: [5.0, 0.0, 5.0], country: 0, level_min: 0, level_max: 0, to_map: 9, to_pos: [9.0, 0.0, 9.0] });
        m.npcs.push(NpcEntry { kind: 20, type_id: 1, points: vec![NpcPoint { pos: [1.0, 0.0, 1.0], dir: 0.0 }] });
        let none = HashMap::new();
        let ground = |_: f32, _: f32| 0.0;
        let cx = FixCtx { map: &m, map_id: Some(1), folder: &none, wld: None, target: None, view_centre: [0.0, 0.0], ground: &ground };
        let fs = findings(&m, &none, 1);
        let kinds = |code: &str| fs.iter().filter(|f| f.code == code).flat_map(|f| fixes_for(f, &cx)).map(|x| x.kind).collect::<Vec<_>>();
        let k = kinds("area-empty");
        assert!(matches!(k[0], FixKind::PickMob { area: 0, entry: 0 }) && matches!(k[1], FixKind::Delete((SectionId::MobAreas, 0))));
        assert!(matches!(kinds("area-mob-zero")[0], FixKind::PickMob { area: 1, entry: 0 }));
        assert!(matches!(kinds("portal-map-missing")[0], FixKind::PickDest { portal: 0 }));
        // no fix for these
        assert!(kinds("npc-type-invalid").is_empty());
        // a bulk Fix all never includes asking fixes
        let hits: Vec<&Finding> = fs.iter().filter(|f| f.code == "area-empty").collect();
        assert!(fix_all(&hits, &cx).is_empty());
        // step label
        assert_eq!(step_label("Swap min and max", hits[0]), "fix: swap min and max (#0)");
    }

    #[test]
    fn fix_all_fixes_a_record_with_two_findings_once() {
        // both corners are outside: two pos-out-of-range findings for the same area, one fix op
        let none = HashMap::new();
        let mut m = Svmap::new(100);
        m.mob_areas.push(area(-5.0, 10.0, 150.0, 20.0, vec![(1, 1)]));
        m.mob_areas.push(area(-1.0, 10.0, 20.0, 20.0, vec![(1, 1)]));
        let fs = findings(&m, &none, 1);
        let hits: Vec<&Finding> = fs.iter().filter(|f| f.code == "pos-out-of-range").collect();
        assert_eq!(hits.len(), 3);
        let ground = |_: f32, _: f32| 0.0;
        let cx = FixCtx { map: &m, map_id: Some(1), folder: &none, wld: None, target: None, view_centre: [0.0, 0.0], ground: &ground };
        assert_eq!(fix_all(&hits, &cx).len(), 2);
        assert_eq!(count(&findings(&fixed(m, &none, "pos-out-of-range"), &none, 1), "pos-out-of-range"), 0);
    }

    #[test]
    fn fix_all_portal_far() {
        // `[both vars]`: three portals of the real map 2 moved 60 units away give three portal-far errors; Fix all clears them
        let (Some(o), Some(c)) = (shaiya_testkit::env_var_path("SVMAP_ORIGINALS"), shaiya_testkit::env_client()) else {
            return;
        };
        let (o, c) = (o, c);
        let rules = crate::client_rules::ClientRules::open(&c).unwrap();
        let mut m = shaiya_svmap::read(&std::fs::read(o.join("2.svmap")).unwrap()).unwrap();
        let wld = rules.wld(2).expect("map 2 has a .wld");
        assert!(m.portals.len() >= 3);
        let original = m.clone();
        for p in m.portals.iter_mut().take(3) {
            p.pos[0] += if p.pos[0] + 60.0 < m.size as f32 { 60.0 } else { -60.0 };
        }
        let far = |m: &Svmap| rules.check(Some(2), m).into_iter().filter(|f| f.code == "portal-far").collect::<Vec<_>>();
        let hits = far(&m);
        assert_eq!(hits.len(), 3);
        let target = sync::target_from_wld(&wld);
        let none = HashMap::new();
        let ground = |_: f32, _: f32| 0.0;
        let cx = FixCtx { map: &m, map_id: Some(2), folder: &none, wld: Some(&wld), target: Some(&target), view_centre: [0.0, 0.0], ground: &ground };
        let refs: Vec<&Finding> = hits.iter().collect();
        let ops = fix_all(&refs, &cx);
        assert_eq!(ops.len(), 3);
        let mut d = Doc::new(1, Some(2), None, m, None);
        d.commit("fix all", ops);
        assert_eq!(d.undo.len(), 1, "one undo step");
        assert_eq!(far(&d.map).len(), 0);
        for (i, w) in wld.portals.iter().enumerate().take(3) {
            let at = sync::portal_from_wld(w).pos;
            assert!((d.map.portals[i].pos[0] - at[0]).abs() < 0.01 && (d.map.portals[i].pos[2] - at[2]).abs() < 0.01, "portal {i}");
        }
        assert_eq!(d.map.portals.len(), original.portals.len(), "the portal count never changes");
    }

    #[test]
    fn fixes_take_the_wld_value() {
        // `[both vars]`: the last deterministic kind of 3.8, "take the .wld value(s)": portal 0 of map 2 gets another country and
        // level range than its .wld box (two portal-field findings on one record); Fix all takes the .wld fields, one undo step
        let (Some(o), Some(c)) = (shaiya_testkit::env_var_path("SVMAP_ORIGINALS"), shaiya_testkit::env_client()) else {
            return;
        };
        let (o, c) = (o, c);
        let rules = crate::client_rules::ClientRules::open(&c).unwrap();
        let bytes = std::fs::read(o.join("2.svmap")).unwrap();
        let mut m = shaiya_svmap::read(&bytes).unwrap();
        let wld = rules.wld(2).expect("map 2 has a .wld");
        let field = |m: &Svmap| rules.check(Some(2), m).into_iter().filter(|f| f.code == "portal-field" && f.index == Some(0)).collect::<Vec<_>>();
        assert!(field(&m).is_empty(), "portal 0 of the original map 2 agrees with its .wld");
        let orig = m.portals[0];
        m.portals[0].country = (orig.country + 1) % 3;
        m.portals[0].level_min = orig.level_min + 5;
        let hits = field(&m);
        assert_eq!(hits.len(), 2, "{hits:?}");
        let target = sync::target_from_wld(&wld);
        let none = HashMap::new();
        let ground = |_: f32, _: f32| 0.0;
        let cx = FixCtx { map: &m, map_id: Some(2), folder: &none, wld: Some(&wld), target: Some(&target), view_centre: [0.0, 0.0], ground: &ground };
        let fx = fixes_for(&hits[0], &cx);
        assert!(fx.iter().any(|x| x.title == "Take the .wld value" && x.bulk));
        let refs: Vec<&Finding> = hits.iter().collect();
        let ops = fix_all(&refs, &cx);
        assert_eq!(ops.len(), 1, "a record with two findings is fixed once");
        let edited = shaiya_svmap::write(&m);
        let mut d = Doc::new(1, Some(2), None, m, None);
        d.commit("fix all", ops);
        assert_eq!(d.undo.len(), 1);
        assert!(field(&d.map).is_empty());
        assert_eq!((d.map.portals[0].country, d.map.portals[0].level_min), (orig.country, orig.level_min), "the .wld values are the original ones");
        assert_eq!(d.map.portals.len(), shaiya_svmap::read(&bytes).unwrap().portals.len());
        d.undo();
        assert_eq!(shaiya_svmap::write(&d.map), edited, "undo restores the bytes");
    }
}
