//! Terrain editing: the pure part. Brushes on the corner heights of a field map's `.wld` (`i16 * 0.02 - 200`, one
//! corner every 2 units), a stroke that remembers the first value of every corner it touched (so one undo step is a corner diff),
//! the water check, the re-drop of records that stand on the ground and the slope classification of blocked cells.
//! Nothing here reads or writes a file and nothing touches the window; `terrain_ui.rs` joins it to the tab.

use std::collections::HashMap;

use shaiya_clientdata::wld::{Placement, WldNpc, CORNER_SPACING, HEIGHT_OFFSET, HEIGHT_SCALE};
use shaiya_clientdata::wld_raw::{RawPortal, WldRaw};
use shaiya_svmap::{BitGrid, Svmap};

use crate::edit::{Op, Rec};
use crate::pair::{CKind, COp};
use crate::place::{self, PlaceList};

/// The client's own in-water test (a fixed height, not the `.wtr`): standing lower than this makes the player swim.
pub const WATER_LEVEL: f32 = -0.4;

/// Raw value of a height in world units, clamped to what the file can hold.
pub fn raw_of(h: f32) -> i16 {
    ((h - HEIGHT_OFFSET) / HEIGHT_SCALE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

/// Height in world units of a raw value.
pub fn units_of(raw: i16) -> f32 {
    raw as f32 * HEIGHT_SCALE + HEIGHT_OFFSET
}

// ---------------------------------------------------------------------------------------------------------------- heights

/// Read access to the corner grid, optionally with some corners read as they were (`over`: corner index -> old raw value).
#[derive(Clone, Copy)]
pub struct Field<'a> {
    pub n: usize,
    pub h: &'a [i16],
    pub over: Option<&'a HashMap<u32, i16>>,
}

impl<'a> Field<'a> {
    pub fn new(n: usize, h: &'a [i16]) -> Field<'a> {
        Field { n, h, over: None }
    }

    /// The same grid with the corners of `over` as they were before the stroke.
    pub fn before(n: usize, h: &'a [i16], over: &'a HashMap<u32, i16>) -> Field<'a> {
        Field { n, h, over: Some(over) }
    }

    fn raw(&self, cx: usize, cz: usize) -> i16 {
        let i = cz * self.n + cx;
        if let Some(o) = self.over {
            if let Some(v) = o.get(&(i as u32)) {
                return *v;
            }
        }
        self.h.get(i).copied().unwrap_or(0)
    }

    /// Height of corner (`cx`, `cz`), clamped into the grid (as the client's reader does).
    pub fn corner(&self, cx: usize, cz: usize) -> f32 {
        units_of(self.raw(cx.min(self.n - 1), cz.min(self.n - 1)))
    }

    /// Ground height at world (x, z): bilinear between the four corners of the cell, positions outside the map clamp to the edge
    /// (the same arithmetic as `Terrain::height_at`).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let max = (self.n - 1) as f32;
        let fx = (x / CORNER_SPACING).clamp(0.0, max);
        let fz = (z / CORNER_SPACING).clamp(0.0, max);
        let (cx, cz) = (fx.floor() as usize, fz.floor() as usize);
        let (tx, tz) = (fx - cx as f32, fz - cz as f32);
        let h00 = self.corner(cx, cz);
        let h10 = self.corner(cx + 1, cz);
        let h01 = self.corner(cx, cz + 1);
        let h11 = self.corner(cx + 1, cz + 1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * tz
    }
}

// ---------------------------------------------------------------------------------------------------------------- the diff

/// The corners one step changed: `(index, before, after)` sorted by index, `before != after` for every entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainDiff {
    pub cells: Vec<(u32, i16, i16)>,
}

impl TerrainDiff {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn invert(&self) -> TerrainDiff {
        TerrainDiff { cells: self.cells.iter().map(|(i, b, a)| (*i, *a, *b)).collect() }
    }

    /// The corner rectangle `[cx0, cz0, cx1, cz1]` (inclusive) the diff touches, for a grid `n` corners wide.
    pub fn rect(&self, n: usize) -> Option<[u32; 4]> {
        let n = n.max(1) as u32;
        let mut r: Option<[u32; 4]> = None;
        for (i, _, _) in &self.cells {
            let (cx, cz) = (i % n, i / n);
            r = Some(match r {
                None => [cx, cz, cx, cz],
                Some(o) => [o[0].min(cx), o[1].min(cz), o[2].max(cx), o[3].max(cz)],
            });
        }
        r
    }

    /// How many corners went from at or above the water level to below it.
    pub fn newly_underwater(&self) -> usize {
        let w = raw_of(WATER_LEVEL);
        self.cells.iter().filter(|(_, b, a)| *a < w && *b >= w).count()
    }
}

// ---------------------------------------------------------------------------------------------------------------- brushes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    Raise,
    Lower,
    Flatten,
    Smooth,
    Set,
    Ramp,
}

impl Brush {
    pub const ALL: [Brush; 6] = [Brush::Raise, Brush::Lower, Brush::Flatten, Brush::Smooth, Brush::Set, Brush::Ramp];

    pub fn label(self) -> &'static str {
        match self {
            Brush::Raise => "Raise",
            Brush::Lower => "Lower",
            Brush::Flatten => "Flatten",
            Brush::Smooth => "Smooth",
            Brush::Set => "Set height",
            Brush::Ramp => "Ramp",
        }
    }

    /// Shift turns Raise into Lower and back.
    pub fn inverted(self, invert: bool) -> Brush {
        match (self, invert) {
            (Brush::Raise, true) => Brush::Lower,
            (Brush::Lower, true) => Brush::Raise,
            (b, _) => b,
        }
    }
}

/// What a brush does. `radius` is in world units; `falloff` 0 (a hard edge) to 1 (the whole radius fades); `amount` 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub brush: Brush,
    pub radius: f32,
    pub falloff: f32,
    pub amount: f32,
    /// The height Set height drives the corners to.
    pub height: f32,
}

impl Default for Params {
    fn default() -> Self {
        Params { brush: Brush::Raise, radius: 12.0, falloff: 0.6, amount: 0.5, height: 0.0 }
    }
}

impl Params {
    /// The same values inside the ranges the toolbar allows; a value that is not a number takes the default (a NaN amount would
    /// otherwise write raw 0, that is -200, into every corner under the brush).
    pub fn sane(self) -> Params {
        let d = Params::default();
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        Params {
            brush: self.brush,
            radius: fix(self.radius, MIN_RADIUS, MAX_RADIUS, d.radius),
            falloff: fix(self.falloff, 0.0, 1.0, d.falloff),
            amount: fix(self.amount, 0.0, 1.0, d.amount),
            height: fix(self.height, units_of(i16::MIN), units_of(i16::MAX), d.height),
        }
    }
}

pub const MIN_RADIUS: f32 = 2.0;
pub const MAX_RADIUS: f32 = 80.0;

/// World units one dab of weight 1 and amount 1 raises a corner.
const RAISE_PER_STEP: f32 = 0.7;
const FLATTEN_RATE: f32 = 0.35;
const SMOOTH_RATE: f32 = 0.6;
const SET_RATE: f32 = 1.0;

/// How strongly a corner at distance `d` from the centre is moved: 1 inside the hard core, fading to 0 at the radius.
pub fn weight(d: f32, radius: f32, falloff: f32) -> f32 {
    if radius <= 0.0 || d >= radius {
        return 0.0;
    }
    let inner = radius * (1.0 - falloff.clamp(0.0, 1.0));
    if d <= inner {
        return 1.0;
    }
    let x = 1.0 - (d - inner) / (radius - inner);
    x * x * (3.0 - 2.0 * x)
}

/// `1 - (1 - p)^k`: the share of the way to the target after `k` steps of `p` each.
fn blend(p: f32, k: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    if p >= 1.0 {
        return 1.0;
    }
    (1.0 - (1.0 - p).powf(k.max(0.0))).clamp(0.0, 1.0)
}

/// A stroke in progress: the first value of every corner it touched, and the exact (unrounded) heights of the corners it moves
/// by small amounts (a corner is 0.02 units apart, so a slow raise would otherwise round to nothing).
#[derive(Clone, Debug, Default)]
pub struct StrokeAcc {
    pub before: HashMap<u32, i16>,
    exact: HashMap<u32, f32>,
    rect: Option<[u32; 4]>,
    /// Corners touched for the first time since the last [`flush_into`](Self::flush_into).
    fresh: Vec<u32>,
}

impl StrokeAcc {
    fn touch(&mut self, h: &[i16], n: usize, cx: usize, cz: usize) {
        let i = (cz * n + cx) as u32;
        if let std::collections::hash_map::Entry::Vacant(e) = self.before.entry(i) {
            e.insert(h[i as usize]);
            self.fresh.push(i);
        }
        self.rect = Some(match self.rect {
            None => [cx as u32, cz as u32, cx as u32, cz as u32],
            Some(o) => [o[0].min(cx as u32), o[1].min(cz as u32), o[2].max(cx as u32), o[3].max(cz as u32)],
        });
    }

    /// The corner rectangle (inclusive) the stroke touched so far.
    pub fn rect(&self) -> Option<[u32; 4]> {
        self.rect
    }

    /// The diff to the grid as it is now: corners whose value differs from the first one.
    pub fn diff(&self, h: &[i16]) -> TerrainDiff {
        let mut cells: Vec<(u32, i16, i16)> = self.before.iter().filter_map(|(i, b)| h.get(*i as usize).filter(|a| *a != b).map(|a| (*i, *b, *a))).collect();
        cells.sort_unstable_by_key(|c| c.0);
        TerrainDiff { cells }
    }

    /// Adds the corners touched for the first time since the last call, with their first value, to `live` (the client half's
    /// record of what a stroke in progress drew: it puts them back on Esc, on a cancel, or before an undo arrives during the stroke).
    pub fn flush_into(&mut self, live: &mut HashMap<u32, i16>) {
        for i in self.fresh.drain(..) {
            if let Some(v) = self.before.get(&i) {
                live.entry(i).or_insert(*v);
            }
        }
    }
}

fn corner_range(c: f32, r: f32, n: usize) -> (usize, usize) {
    let lo = ((c - r) / CORNER_SPACING).floor().max(0.0) as usize;
    let hi = (((c + r) / CORNER_SPACING).ceil().max(0.0) as usize).min(n - 1);
    (lo.min(n - 1), hi)
}

/// One dab of the brush at world (x, z). `steps` is the number of 1/60 s frames it stands for (a click is 1). Raise and Lower
/// move the corners by `amount`; Flatten (to `target`), Smooth and Set drive them toward their goal. Returns the corner rectangle
/// that changed (`None` when nothing did).
pub fn dab(h: &mut [i16], n: usize, p: &Params, acc: &mut StrokeAcc, at: [f32; 2], steps: f32, target: f32) -> Option<[u32; 4]> {
    if n < 2 || h.len() != n * n {
        return None;
    }
    let (x0, x1) = corner_range(at[0], p.radius, n);
    let (z0, z1) = corner_range(at[1], p.radius, n);
    // Smooth reads the neighbours as they were before this dab
    let snapshot: Option<Vec<i16>> = (p.brush == Brush::Smooth).then(|| h.to_vec());
    let mut changed: Option<[u32; 4]> = None;
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let d = ((cx as f32 * CORNER_SPACING - at[0]).powi(2) + (cz as f32 * CORNER_SPACING - at[1]).powi(2)).sqrt();
            let w = weight(d, p.radius, p.falloff);
            if w <= 0.0 {
                continue;
            }
            let i = (cz * n + cx) as u32;
            let cur = acc.exact.get(&i).copied().filter(|e| raw_of(*e) == h[i as usize]).unwrap_or_else(|| units_of(h[i as usize]));
            let new = match p.brush {
                Brush::Raise => cur + RAISE_PER_STEP * p.amount * steps * w,
                Brush::Lower => cur - RAISE_PER_STEP * p.amount * steps * w,
                Brush::Flatten => cur + (target - cur) * blend(w * FLATTEN_RATE * p.amount.max(0.05) * 2.0, steps),
                Brush::Set => cur + (p.height - cur) * blend(w * SET_RATE, steps),
                Brush::Smooth => {
                    let s = snapshot.as_ref().expect("smooth keeps a snapshot");
                    let get = |x: isize, z: isize| units_of(s[(z.clamp(0, n as isize - 1) as usize) * n + x.clamp(0, n as isize - 1) as usize]);
                    let (xi, zi) = (cx as isize, cz as isize);
                    let mut sum = 0.0;
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            sum += get(xi + dx, zi + dz);
                        }
                    }
                    let avg = sum / 9.0;
                    cur + (avg - cur) * blend(w * SMOOTH_RATE * p.amount.max(0.05) * 2.0, steps)
                }
                Brush::Ramp => continue,
            };
            if !new.is_finite() {
                continue;
            }
            // the exact value stays inside what the file holds, so a raise held at the top does not pile up height that a lower
            // in the same stroke would first have to take off again
            let new = new.clamp(units_of(i16::MIN), units_of(i16::MAX));
            let raw = raw_of(new);
            if raw == h[i as usize] && (new - cur).abs() < 1e-6 {
                continue;
            }
            acc.touch(h, n, cx, cz);
            acc.exact.insert(i, new);
            if raw != h[i as usize] {
                h[i as usize] = raw;
                changed = Some(match changed {
                    None => [cx as u32, cz as u32, cx as u32, cz as u32],
                    Some(o) => [o[0].min(cx as u32), o[1].min(cz as u32), o[2].max(cx as u32), o[3].max(cz as u32)],
                });
            }
        }
    }
    changed
}

/// The Ramp brush: the corners within the radius of the segment `a`-`b` move toward the plane that rises from `ha` at `a` to `hb`
/// at `b`. It is recomputed from the heights the stroke started with, so dragging on only reshapes the ramp. Returns the corner
/// rectangle that changed (the old and the new ramp).
#[allow(clippy::too_many_arguments)]
pub fn ramp(h: &mut [i16], n: usize, p: &Params, acc: &mut StrokeAcc, a: [f32; 2], b: [f32; 2], ha: f32, hb: f32) -> Option<[u32; 4]> {
    if n < 2 || h.len() != n * n {
        return None;
    }
    let mut changed: Option<[u32; 4]> = None;
    let note = |c: &mut Option<[u32; 4]>, i: u32| {
        let (cx, cz) = ((i as usize % n) as u32, (i as usize / n) as u32);
        *c = Some(match *c {
            None => [cx, cz, cx, cz],
            Some(o) => [o[0].min(cx), o[1].min(cz), o[2].max(cx), o[3].max(cz)],
        });
    };
    // the old ramp goes back first
    for (i, v) in acc.before.iter() {
        if h[*i as usize] != *v {
            h[*i as usize] = *v;
            note(&mut changed, *i);
        }
    }
    let (min_x, max_x) = (a[0].min(b[0]), a[0].max(b[0]));
    let (min_z, max_z) = (a[1].min(b[1]), a[1].max(b[1]));
    let (x0, _) = corner_range(min_x, p.radius, n);
    let (_, x1) = corner_range(max_x, p.radius, n);
    let (z0, _) = corner_range(min_z, p.radius, n);
    let (_, z1) = corner_range(max_z, p.radius, n);
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dz * dz;
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let (px, pz) = (cx as f32 * CORNER_SPACING, cz as f32 * CORNER_SPACING);
            let t = if len2 > 1e-6 { (((px - a[0]) * dx + (pz - a[1]) * dz) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (qx, qz) = (a[0] + dx * t, a[1] + dz * t);
            let d = ((px - qx).powi(2) + (pz - qz).powi(2)).sqrt();
            let w = weight(d, p.radius, p.falloff);
            if w <= 0.0 {
                continue;
            }
            let i = (cz * n + cx) as u32;
            let first = acc.before.get(&i).copied().unwrap_or(h[i as usize]);
            let goal = ha + (hb - ha) * t;
            let cur = units_of(first);
            let raw = raw_of(cur + (goal - cur) * w);
            if raw != first {
                acc.touch_with(i, first, n);
                if h[i as usize] != raw {
                    h[i as usize] = raw;
                    note(&mut changed, i);
                }
            }
        }
    }
    changed
}

impl StrokeAcc {
    fn touch_with(&mut self, i: u32, first: i16, n: usize) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.before.entry(i) {
            e.insert(first);
            self.fresh.push(i);
        }
        let (cx, cz) = ((i as usize % n) as u32, (i as usize / n) as u32);
        self.rect = Some(match self.rect {
            None => [cx, cz, cx, cz],
            Some(o) => [o[0].min(cx), o[1].min(cz), o[2].max(cx), o[3].max(cz)],
        });
    }
}

// ---------------------------------------------------------------------------------------------------------------- re-drop

/// Records that stand on the ground, moved by the change of the ground height under them.
#[derive(Debug, Default)]
pub struct Redrop {
    /// svmap side: NPC points and portals.
    pub ops: Vec<Op>,
    /// client side: buildings, shapes, trees, NPCs and portal boxes.
    pub cops: Vec<COp>,
    pub moved: usize,
}

fn shifted(old: &Field, new: &Field, x: f32, z: f32) -> Option<f32> {
    let dy = new.height_at(x, z) - old.height_at(x, z);
    (dy.abs() > 1e-4).then_some(dy)
}

/// What to move after a stroke: every record whose ground height changed keeps its height above the ground (its y moves by the
/// difference). `rect` is the stroke's corner rectangle (records outside it are not looked at). Buildings, shapes and trees, the
/// NPCs' points and the portal boxes of the client map, and the NPC points and portals of the svmap. Each side is moved by its own
/// difference, so a pair that differed by a little before differs by the same little after.
pub fn redrop(map: &Svmap, model: &WldRaw, old: &Field, new: &Field, rect: [u32; 4]) -> Redrop {
    let pad = 2.0 * CORNER_SPACING;
    let (xa, za) = (rect[0] as f32 * CORNER_SPACING - pad, rect[1] as f32 * CORNER_SPACING - pad);
    let (xb, zb) = (rect[2] as f32 * CORNER_SPACING + pad, rect[3] as f32 * CORNER_SPACING + pad);
    let inside = |x: f32, z: f32| x >= xa && x <= xb && z >= za && z <= zb;
    let mut r = Redrop::default();

    // client buildings, shapes, trees
    let mut edits: Vec<(place::PSel, Placement)> = Vec::new();
    for l in [PlaceList::Building, PlaceList::Shape, PlaceList::Tree] {
        for (i, p) in l.list(model).items.iter().enumerate() {
            if p.index < 0 || !inside(p.pos[0], p.pos[2]) {
                continue;
            }
            if let Some(dy) = shifted(old, new, p.pos[0], p.pos[2]) {
                let mut q = *p;
                q.pos[1] += dy;
                edits.push(((l, i), q));
            }
        }
    }
    r.moved += edits.len();
    r.cops.extend(place::replace_ops(model, &edits));

    // client NPCs: the first point and the extra points
    for (i, n) in model.npcs.iter().enumerate() {
        let mut q: WldNpc = n.clone();
        let mut any = false;
        let lift = |p: &mut [f32; 3]| {
            if inside(p[0], p[2]) {
                if let Some(dy) = shifted(old, new, p[0], p[2]) {
                    p[1] += dy;
                    return true;
                }
            }
            false
        };
        any |= lift(&mut q.pos);
        for e in q.extra.iter_mut() {
            any |= lift(e);
        }
        if any {
            r.moved += 1;
            r.cops.push(COp::Npc(i, CKind::Replace { before: n.clone(), after: q }));
        }
    }

    // client portal boxes: the whole box moves by the difference at its centre
    for (i, p) in model.portals.iter().enumerate() {
        let (cx, cz) = ((p.min[0] + p.max[0]) / 2.0, (p.min[2] + p.max[2]) / 2.0);
        if !inside(cx, cz) {
            continue;
        }
        if let Some(dy) = shifted(old, new, cx, cz) {
            let mut q: RawPortal = p.clone();
            q.min[1] += dy;
            q.max[1] += dy;
            r.moved += 1;
            r.cops.push(COp::Portal(i, CKind::Replace { before: p.clone(), after: q }));
        }
    }

    // svmap NPC points and portals
    for (i, n) in map.npcs.iter().enumerate() {
        let mut q = n.clone();
        let mut any = false;
        for pt in q.points.iter_mut() {
            if inside(pt.pos[0], pt.pos[2]) {
                if let Some(dy) = shifted(old, new, pt.pos[0], pt.pos[2]) {
                    pt.pos[1] += dy;
                    any = true;
                }
            }
        }
        if any {
            r.moved += 1;
            r.ops.push(Op::replace(i, Rec::Npc(n.clone()), Rec::Npc(q)));
        }
    }
    for (i, p) in map.portals.iter().enumerate() {
        if !inside(p.pos[0], p.pos[2]) {
            continue;
        }
        if let Some(dy) = shifted(old, new, p.pos[0], p.pos[2]) {
            let mut q = *p;
            q.pos[1] += dy;
            r.moved += 1;
            r.ops.push(Op::replace(i, Rec::Portal(*p), Rec::Portal(q)));
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------------------------- slope

/// Whether the terrain around the centre of cell (x, z) is steeper than `max_slope_deg` (the same test as
/// `generate::slope_blocked`).
pub fn cell_steep(f: &Field, x: u32, z: u32, max_slope_deg: f32) -> bool {
    let limit = max_slope_deg.to_radians().tan();
    let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
    let dx = (f.height_at(cx + 1.0, cz) - f.height_at(cx - 1.0, cz)) / 2.0;
    let dz = (f.height_at(cx, cz + 1.0) - f.height_at(cx, cz - 1.0)) / 2.0;
    (dx * dx + dz * dz).sqrt() > limit
}

/// The cells a stroke over the corner rectangle `rect` flips: a cell whose slope class changed (steeper than the angle before and
/// not after, or the other way) gets the class it has now. A cell whose class did not change keeps what the map has, so blocked
/// cells of the original (walls, edges) are never freed by a stroke that does not change their slope. `(x, z, blocked)`.
pub fn reblock_flips(map: &Svmap, old: &Field, new: &Field, rect: [u32; 4], max_slope_deg: f32) -> Vec<(u32, u32, bool)> {
    let size = map.blocked.size();
    if size == 0 {
        return Vec::new();
    }
    // a corner at index c touches the cells from 2c - 2 to 2c + 2 (the slope looks one unit either side of a cell centre)
    let lo = |c: u32| (c * 2).saturating_sub(3);
    let hi = |c: u32| (c * 2 + 3).min(size - 1);
    let mut out = Vec::new();
    for z in lo(rect[1])..=hi(rect[3]) {
        for x in lo(rect[0])..=hi(rect[2]) {
            let (a, b) = (cell_steep(old, x, z, max_slope_deg), cell_steep(new, x, z, max_slope_deg));
            if a != b && map.blocked.get(x, z) != b {
                out.push((x, z, b));
            }
        }
    }
    out
}

/// The blocked-cell op for a list of flips, `None` when there are none.
pub fn flips_op(map: &Svmap, flips: &[(u32, u32, bool)]) -> Option<Op> {
    if flips.is_empty() {
        return None;
    }
    let mut after: BitGrid = map.blocked.clone();
    for (x, z, v) in flips {
        after.set(*x, *z, *v);
    }
    map.blocked.diff_runs(&after).filter(|r| !r.is_empty()).map(Op::cells)
}

// ---------------------------------------------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(n: usize, raw: i16) -> Vec<i16> {
        vec![raw; n * n]
    }

    #[test]
    fn raw_and_units_agree() {
        for r in [0i16, 1, 9000, i16::MAX, i16::MIN] {
            assert_eq!(raw_of(units_of(r)), r);
        }
        assert_eq!(raw_of(1.0e6), i16::MAX);
        assert_eq!(raw_of(-1.0e6), i16::MIN);
    }

    #[test]
    fn weight_is_one_inside_and_fades_to_zero() {
        assert_eq!(weight(0.0, 10.0, 0.5), 1.0);
        assert_eq!(weight(5.0, 10.0, 0.5), 1.0);
        assert!(weight(7.5, 10.0, 0.5) < 1.0 && weight(7.5, 10.0, 0.5) > 0.0);
        assert_eq!(weight(10.0, 10.0, 0.5), 0.0);
        assert_eq!(weight(9.0, 10.0, 0.0), 1.0, "no falloff is a hard edge");
        assert!(weight(1.0, 10.0, 1.0) < 1.0, "full falloff fades from the centre");
    }

    #[test]
    fn raise_accumulates_below_one_raw_step() {
        let n = 33;
        let mut h = flat(n, 9000);
        let mut acc = StrokeAcc::default();
        let p = Params { brush: Brush::Raise, radius: 6.0, falloff: 0.0, amount: 0.02, height: 0.0 };
        // one dab moves a corner by 0.014 units, less than the 0.02 step
        for _ in 0..40 {
            dab(&mut h, n, &p, &mut acc, [32.0, 32.0], 1.0, 0.0);
        }
        let c = 16 * n + 16;
        assert!(h[c] > 9000, "forty small dabs add up: {}", h[c]);
        // dab again on a copy of the exact value stays consistent with the stored raw value
        assert_eq!(acc.before[&(c as u32)], 9000);
    }

    #[test]
    fn flatten_set_and_smooth_reach_their_goals() {
        let n = 33;
        let mut h = flat(n, 9000);
        h[16 * n + 16] = 9500;
        let mut acc = StrokeAcc::default();
        let p = Params { brush: Brush::Smooth, radius: 6.0, falloff: 0.0, amount: 1.0, height: 0.0 };
        for _ in 0..30 {
            dab(&mut h, n, &p, &mut acc, [32.0, 32.0], 1.0, 0.0);
        }
        assert!(h[16 * n + 16] < 9100, "the spike is smoothed: {}", h[16 * n + 16]);
        let target = units_of(9000);
        let pf = Params { brush: Brush::Flatten, ..p };
        for _ in 0..100 {
            dab(&mut h, n, &pf, &mut acc, [32.0, 32.0], 1.0, target);
        }
        assert!((h[16 * n + 16] - 9000).abs() <= 1, "flatten reaches the sampled height");
        let ps = Params { brush: Brush::Set, height: 12.0, ..p };
        for _ in 0..3 {
            dab(&mut h, n, &ps, &mut acc, [32.0, 32.0], 1.0, 0.0);
        }
        assert_eq!(h[16 * n + 16], raw_of(12.0));
    }

    #[test]
    fn corners_outside_the_radius_and_the_grid_stay() {
        let n = 33;
        let mut h = flat(n, 100);
        let mut acc = StrokeAcc::default();
        let p = Params { brush: Brush::Raise, radius: 4.0, falloff: 0.3, amount: 1.0, height: 0.0 };
        // a brush over the corner of the map reaches only corners that exist
        dab(&mut h, n, &p, &mut acc, [0.0, 0.0], 1.0, 0.0);
        dab(&mut h, n, &p, &mut acc, [64.0, 64.0], 1.0, 0.0);
        let far = h[16 * n + 16];
        assert_eq!(far, 100);
        assert!(h[0] > 100 && h[n * n - 1] > 100);
        assert!(acc.before.keys().all(|i| (*i as usize) < n * n));
    }

    #[test]
    fn terrain_clamp_i16() {
        let n = 17;
        let mut h = flat(n, i16::MAX - 2);
        let mut acc = StrokeAcc::default();
        let up = Params { brush: Brush::Raise, radius: 8.0, falloff: 0.0, amount: 1.0, height: 0.0 };
        for _ in 0..50 {
            dab(&mut h, n, &up, &mut acc, [16.0, 16.0], 5.0, 0.0);
        }
        assert_eq!(h[8 * n + 8], i16::MAX, "raising stops at the largest value the file holds");
        let mut low = flat(n, i16::MIN + 2);
        let mut acc2 = StrokeAcc::default();
        let down = Params { brush: Brush::Lower, ..up };
        for _ in 0..50 {
            dab(&mut low, n, &down, &mut acc2, [16.0, 16.0], 5.0, 0.0);
        }
        assert_eq!(low[8 * n + 8], i16::MIN);
        // Set height beyond the range clamps too
        let mut s = flat(n, 0);
        let mut acc3 = StrokeAcc::default();
        let set = Params { brush: Brush::Set, height: 1.0e6, ..up };
        dab(&mut s, n, &set, &mut acc3, [16.0, 16.0], 1.0, 0.0);
        assert_eq!(s[8 * n + 8], i16::MAX);
        assert_eq!(raw_of(f32::NAN), 0, "a NaN height is not written as garbage");
    }

    #[test]
    fn terrain_water_warning() {
        let n = 17;
        let dry = raw_of(5.0);
        let mut h = flat(n, dry);
        let mut acc = StrokeAcc::default();
        let p = Params { brush: Brush::Set, radius: 6.0, falloff: 0.0, amount: 1.0, height: -3.0 };
        dab(&mut h, n, &p, &mut acc, [16.0, 16.0], 1.0, 0.0);
        let d = acc.diff(&h);
        assert!(!d.is_empty());
        assert_eq!(d.newly_underwater(), d.cells.len(), "every corner went below -0.4");
        // corners that were already under water are not counted again, and a raise does not warn
        let mut acc2 = StrokeAcc::default();
        let p2 = Params { height: -5.0, ..p };
        dab(&mut h, n, &p2, &mut acc2, [16.0, 16.0], 1.0, 0.0);
        assert_eq!(acc2.diff(&h).newly_underwater(), 0);
        let mut acc3 = StrokeAcc::default();
        let up = Params { brush: Brush::Set, height: 20.0, ..p };
        dab(&mut h, n, &up, &mut acc3, [16.0, 16.0], 1.0, 0.0);
        assert_eq!(acc3.diff(&h).newly_underwater(), 0);
        // exactly at the water level is not under it
        let at = raw_of(WATER_LEVEL);
        assert_eq!(TerrainDiff { cells: vec![(0, dry, at)] }.newly_underwater(), 0);
        assert_eq!(TerrainDiff { cells: vec![(0, dry, at - 1)] }.newly_underwater(), 1);
    }

    #[test]
    fn a_stroke_that_comes_back_is_no_change() {
        let n = 17;
        let mut h = flat(n, 9000);
        let mut acc = StrokeAcc::default();
        let up = Params { brush: Brush::Set, radius: 6.0, falloff: 0.0, amount: 1.0, height: 30.0 };
        dab(&mut h, n, &up, &mut acc, [16.0, 16.0], 1.0, 0.0);
        assert!(!acc.diff(&h).is_empty());
        let back = Params { height: units_of(9000), ..up };
        dab(&mut h, n, &back, &mut acc, [16.0, 16.0], 1.0, 0.0);
        assert!(acc.diff(&h).is_empty(), "the corners are where they started: the stroke has no diff");
    }

    #[test]
    fn ramp_follows_the_segment_and_can_be_redrawn() {
        let n = 65;
        let mut h = flat(n, raw_of(0.0));
        let mut acc = StrokeAcc::default();
        let p = Params { brush: Brush::Ramp, radius: 6.0, falloff: 0.0, amount: 1.0, height: 0.0 };
        ramp(&mut h, n, &p, &mut acc, [20.0, 64.0], [100.0, 64.0], 0.0, 20.0);
        let mid = h[32 * n + 30];
        assert!((units_of(mid) - 10.0).abs() < 0.3, "half way up the ramp: {}", units_of(mid));
        assert!(units_of(h[32 * n + 10]) < 0.01, "before the start nothing moves");
        // dragging the end back shortens it: the corners of the first ramp return
        ramp(&mut h, n, &p, &mut acc, [20.0, 64.0], [40.0, 64.0], 0.0, 20.0);
        assert!(units_of(h[32 * n + 40]) < 0.01, "the old ramp is gone beyond the new end: {}", units_of(h[32 * n + 40]));
        // and a ramp flat to its start leaves no diff
        ramp(&mut h, n, &p, &mut acc, [20.0, 64.0], [40.0, 64.0], 0.0, 0.0);
        assert!(acc.diff(&h).is_empty());
    }

    #[test]
    fn the_first_value_of_every_touched_corner_is_recorded_once() {
        // (review: replaces `revert_own`, which told the stroke's corners from an undo's by value and so put back a corner whose
        // undone value happened to equal what the stroke wrote; the client half now keeps the record and reverts before an undo)
        let n = 17;
        let mut h = flat(n, 9000);
        let mut acc = StrokeAcc::default();
        let mut live = HashMap::new();
        let up = Params { brush: Brush::Raise, radius: 6.0, falloff: 0.0, amount: 1.0, height: 0.0 };
        dab(&mut h, n, &up, &mut acc, [16.0, 16.0], 3.0, 0.0);
        acc.flush_into(&mut live);
        let first = live.len();
        assert!(first > 0 && live.values().all(|v| *v == 9000));
        dab(&mut h, n, &up, &mut acc, [16.0, 16.0], 3.0, 0.0);
        acc.flush_into(&mut live);
        assert_eq!(live.len(), first, "a corner touched again is not recorded again");
        assert!(live.values().all(|v| *v == 9000), "the value before the stroke, not the one of the first dab");
        for (i, v) in &live {
            h[*i as usize] = *v;
        }
        assert_eq!(h, flat(n, 9000), "putting the record back gives the grid before the stroke");
    }

    #[test]
    fn height_field_matches_the_decoded_terrain() {
        use shaiya_clientdata::wld::Terrain;
        let n = 9;
        let h: Vec<i16> = (0..n * n).map(|i| (9000 + (i * 37) % 300) as i16).collect();
        let t = Terrain { size: 16, n, heights: h.clone(), tex_index: vec![0; n * n] };
        let f = Field::new(n, &h);
        for (x, z) in [(0.0, 0.0), (3.3, 5.1), (15.9, 0.2), (16.0, 16.0), (40.0, -5.0)] {
            assert!((f.height_at(x, z) - t.height_at(x, z)).abs() < 1e-5, "{x} {z}");
        }
        let mut over = HashMap::new();
        over.insert(4u32, 100i16);
        let g = Field::before(n, &h, &over);
        assert_eq!(g.corner(4, 0), units_of(100));
        assert_eq!(g.corner(5, 0), f.corner(5, 0));
    }
}
