//! The Scatter tool's engine. Pure: no egui, no files. A region, a list of weighted monsters and a
//! few options give a set of monster areas, the same every time for one seed:
//!
//! 1. [`Pcg32`] (written here, no crate) supplies the random numbers.
//! 2. [`poisson`] places points at least `spacing` apart (Bridson, "Fast Poisson Disk Sampling in Arbitrary Dimensions", 2007:
//!    cells of r / sqrt 2, 30 candidates per point) over the region.
//! 3. A point gets a box when the box is inside the map and, with the options on, on free cells and clear of the map's areas.
//! 4. The box sits on the lowest ground under it, 2 units high, with a weighted monster and a count.

use serde::{Deserialize, Serialize};
use shaiya_svmap::{BitGrid, MobArea, MobEntry};

use crate::edit::{self, Rec};

/// Areas one scatter makes at most.
pub const MAX_AREAS: usize = 2000;
/// Points the sampler makes at most (areas dropped for blocked cells or clearance do not count against [`MAX_AREAS`]).
const MAX_POINTS: usize = 6000;
/// Candidates tried around every point (Bridson's k).
const CANDIDATES: usize = 30;
/// The share of free cells under a box that avoid-blocked asks for.
pub const MIN_FREE: f32 = 0.7;
/// The chance of a second kind in a box.
const SECOND_KIND: f32 = 0.25;

// ------------------------------------------------------------------------------------------------------------------ PCG32

/// PCG32 (XSH RR 64/32; O'Neill, pcg-random.org): 64 bits of state, 32 bits out.
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    pub fn new(seed: u64, stream: u64) -> Pcg32 {
        let mut r = Pcg32 { state: 0, inc: (stream << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6364136223846793005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// A number in `0..n` without bias (`n` > 0).
    pub fn below(&mut self, n: u32) -> u32 {
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % n;
            }
        }
    }

    /// A number in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
}

// ------------------------------------------------------------------------------------------------------------------ region

/// One piece of a region: a rectangle `[x0, z0, x1, z1]` or a polygon (even-odd rule).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect([f32; 4]),
    Poly(Vec<[f32; 2]>),
}

fn poly_contains(pts: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut inside = false;
    let mut j = pts.len().wrapping_sub(1);
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Shape {
    pub fn contains(&self, x: f32, z: f32) -> bool {
        match self {
            Shape::Rect(r) => x >= r[0] && x <= r[2] && z >= r[1] && z <= r[3],
            Shape::Poly(p) => poly_contains(p, x, z),
        }
    }

    pub fn bounds(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Rect(r) => Some(*r),
            Shape::Poly(p) => {
                let first = p.first()?;
                let mut b = [first[0], first[1], first[0], first[1]];
                for q in p {
                    b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
                }
                Some(b)
            }
        }
    }
}

/// What to fill: the union of its shapes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    pub shapes: Vec<Shape>,
}

impl Region {
    /// A rectangle from two corners in any order.
    pub fn rect(a: [f32; 2], b: [f32; 2]) -> Region {
        Region { shapes: vec![Shape::Rect([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])])] }
    }

    pub fn poly(points: Vec<[f32; 2]>) -> Region {
        Region { shapes: vec![Shape::Poly(points)] }
    }

    /// The union of boxes `[x0, z0, x1, z1]` (Fill selection).
    pub fn from_boxes(boxes: &[[f32; 4]]) -> Region {
        Region { shapes: boxes.iter().map(|b| Shape::Rect([b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])])).collect() }
    }

    pub fn contains(&self, x: f32, z: f32) -> bool {
        self.shapes.iter().any(|s| s.contains(x, z))
    }

    /// The smallest rectangle around every shape.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut out: Option<[f32; 4]> = None;
        for b in self.shapes.iter().filter_map(Shape::bounds) {
            out = Some(match out {
                None => b,
                Some(o) => [o[0].min(b[0]), o[1].min(b[1]), o[2].max(b[2]), o[3].max(b[3])],
            });
        }
        out.filter(|b| b.iter().all(|v| v.is_finite()))
    }
}

/// The region being drawn in the viewport (it lives with the tab's view state).
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub region: Option<Region>,
    /// The polygon points clicked so far.
    pub poly: Vec<[f32; 2]>,
    /// A rectangle being dragged: start and the cursor.
    pub drag: Option<([f32; 2], [f32; 2])>,
    /// Counts every change of the region (the preview recomputes when it moves).
    pub rev: u64,
}

impl Input {
    /// A region, a polygon or a drag is there.
    pub fn active(&self) -> bool {
        self.region.is_some() || !self.poly.is_empty() || self.drag.is_some()
    }

    pub fn clear(&mut self) {
        if self.active() {
            self.rev += 1;
        }
        self.region = None;
        self.poly.clear();
        self.drag = None;
    }

    pub fn set(&mut self, r: Region) {
        self.region = Some(r);
        self.poly.clear();
        self.drag = None;
        self.rev += 1;
    }

    /// Closes the polygon of 3 or more points into the region.
    pub fn close_poly(&mut self) -> bool {
        if self.poly.len() >= 3 {
            let pts = std::mem::take(&mut self.poly);
            self.set(Region::poly(pts));
            true
        } else {
            false
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ options

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Sparse,
    #[default]
    Normal,
    Dense,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Sparse, Preset::Normal, Preset::Dense, Preset::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Sparse => "Sparse",
            Preset::Normal => "Normal",
            Preset::Dense => "Dense",
            Preset::Custom => "Custom",
        }
    }

    /// (spacing, box side, count min, count max); the three numbers sets come from the 77 original maps (4.10).
    pub fn values(self) -> Option<(f32, f32, u32, u32)> {
        match self {
            Preset::Sparse => Some((28.0, 8.0, 1, 2)),
            Preset::Normal => Some((20.0, 9.0, 2, 2)),
            Preset::Dense => Some((12.0, 12.0, 2, 3)),
            Preset::Custom => None,
        }
    }
}

/// The options as the settings file keeps them (`scatter`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScatterSettings {
    pub preset: Preset,
    /// The Custom values (the other presets use their own).
    pub spacing: f32,
    pub size: f32,
    pub count_min: u32,
    pub count_max: u32,
    pub avoid_blocked: bool,
    pub keep_clear: bool,
    pub two_kinds: bool,
    pub seed: u32,
    /// (monster id, weight 1 to 9).
    pub mobs: Vec<[u32; 2]>,
}

impl Default for ScatterSettings {
    fn default() -> Self {
        ScatterSettings { preset: Preset::Normal, spacing: 20.0, size: 9.0, count_min: 2, count_max: 2, avoid_blocked: true, keep_clear: true, two_kinds: false, seed: 1, mobs: Vec::new() }
    }
}

/// The monsters one scatter lists at most.
pub const MAX_MOBS: usize = 12;

impl ScatterSettings {
    /// Values a hand-edited file could break put back in range.
    pub fn clamped(mut self) -> ScatterSettings {
        let fin = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.spacing = fin(self.spacing, 4.0, 200.0, 20.0);
        self.size = fin(self.size, 1.0, 64.0, 9.0);
        self.count_min = self.count_min.clamp(1, 8);
        self.count_max = self.count_max.clamp(self.count_min, 8);
        self.mobs.truncate(MAX_MOBS);
        for m in &mut self.mobs {
            m[1] = m[1].clamp(1, 9);
        }
        self
    }

    /// The numbers one scatter uses: the preset's, or the Custom ones.
    pub fn params(&self) -> Params {
        let s = self.clone().clamped();
        let (spacing, size, count_min, count_max) = s.preset.values().unwrap_or((s.spacing, s.size, s.count_min, s.count_max));
        Params { spacing, size, count_min, count_max, avoid_blocked: s.avoid_blocked, keep_clear: s.keep_clear, two_kinds: s.two_kinds, seed: s.seed, mobs: s.mobs.iter().map(|m| (m[0], m[1])).collect() }
    }

    /// Adds a monster with weight 1, or raises the weight of one that is there.
    pub fn add_mob(&mut self, id: u32) {
        if let Some(m) = self.mobs.iter_mut().find(|m| m[0] == id) {
            m[1] = (m[1] + 1).min(9);
        } else if self.mobs.len() < MAX_MOBS {
            self.mobs.push([id, 1]);
        }
    }
}

/// What [`generate`] works from.
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// Smallest distance between two box centres.
    pub spacing: f32,
    /// Side of a box.
    pub size: f32,
    pub count_min: u32,
    pub count_max: u32,
    pub avoid_blocked: bool,
    pub keep_clear: bool,
    pub two_kinds: bool,
    pub seed: u32,
    /// (monster id, weight).
    pub mobs: Vec<(u32, u32)>,
}

/// What the map under the region looks like.
pub struct Site<'a> {
    pub size: u32,
    pub blocked: &'a BitGrid,
    /// The map's monster areas as `[x0, z0, x1, z1]`.
    pub existing: &'a [[f32; 4]],
    /// Ground height (NaN where there is none).
    pub ground: &'a dyn Fn(f32, f32) -> f32,
}

// ------------------------------------------------------------------------------------------------------------------ sampling

/// Points at least `r` apart inside `rect` (`[x0, z0, x1, z1]`) that `accept` allows, at most `max`. `seeds` are the rectangles a
/// new run may start in, so a region of several parts gets points in every part.
pub fn poisson(rng: &mut Pcg32, rect: [f32; 4], seeds: &[[f32; 4]], r: f32, accept: &dyn Fn(f32, f32) -> bool, max: usize) -> Vec<[f32; 2]> {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    if !(w >= 0.0 && h >= 0.0 && r > 0.0) || seeds.is_empty() {
        return Vec::new();
    }
    let cell = r / std::f32::consts::SQRT_2;
    let (gw, gh) = ((w / cell) as usize + 1, (h / cell) as usize + 1);
    if gw.saturating_mul(gh) > 4_000_000 {
        return Vec::new();
    }
    let mut grid: Vec<u32> = vec![u32::MAX; gw * gh];
    let mut pts: Vec<[f32; 2]> = Vec::new();
    let mut active: Vec<usize> = Vec::new();
    let cell_of = |p: [f32; 2]| (((p[0] - rect[0]) / cell) as usize).min(gw - 1) + (((p[1] - rect[1]) / cell) as usize).min(gh - 1) * gw;
    let far_enough = |pts: &[[f32; 2]], grid: &[u32], p: [f32; 2]| -> bool {
        let (cx, cz) = ((((p[0] - rect[0]) / cell) as isize).min(gw as isize - 1), (((p[1] - rect[1]) / cell) as isize).min(gh as isize - 1));
        for iz in (cz - 2).max(0)..=(cz + 2).min(gh as isize - 1) {
            for ix in (cx - 2).max(0)..=(cx + 2).min(gw as isize - 1) {
                let k = grid[iz as usize * gw + ix as usize];
                if k != u32::MAX {
                    let q = pts[k as usize];
                    if (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) < r * r {
                        return false;
                    }
                }
            }
        }
        true
    };
    let inside = |p: [f32; 2]| p[0] >= rect[0] && p[0] <= rect[2] && p[1] >= rect[1] && p[1] <= rect[3];
    let mut restarts = 0;
    'outer: loop {
        if active.is_empty() {
            // a new run: a random spot in one of the seed rectangles that is allowed and free
            restarts += 1;
            if restarts > 64 {
                break;
            }
            let mut found = None;
            for _ in 0..200 {
                let s = seeds[rng.below(seeds.len() as u32) as usize];
                let p = [s[0] + rng.unit() * (s[2] - s[0]), s[1] + rng.unit() * (s[3] - s[1])];
                if inside(p) && accept(p[0], p[1]) && far_enough(&pts, &grid, p) {
                    found = Some(p);
                    break;
                }
            }
            let Some(p) = found else { break };
            grid[cell_of(p)] = pts.len() as u32;
            active.push(pts.len());
            pts.push(p);
        }
        while !active.is_empty() {
            if pts.len() >= max {
                break 'outer;
            }
            let ai = rng.below(active.len() as u32) as usize;
            let base = pts[active[ai]];
            let mut placed = false;
            for _ in 0..CANDIDATES {
                let (rad, ang) = (r * (1.0 + rng.unit()), rng.unit() * std::f32::consts::TAU);
                let p = [base[0] + rad * ang.cos(), base[1] + rad * ang.sin()];
                if inside(p) && accept(p[0], p[1]) && far_enough(&pts, &grid, p) {
                    grid[cell_of(p)] = pts.len() as u32;
                    active.push(pts.len());
                    pts.push(p);
                    placed = true;
                    break;
                }
            }
            if !placed {
                active.swap_remove(ai);
            }
        }
    }
    pts
}

/// One decimal, as the tool rounds box corners (tidy numbers in the file and in diffs).
fn tenth(v: f32) -> f32 {
    ((v as f64 * 10.0).round() / 10.0) as f32
}

/// The areas a scatter would add, and what it left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preview {
    pub areas: Vec<MobArea>,
    /// Points the sampler made.
    pub points: usize,
    /// Points left out because the box was on blocked cells.
    pub skipped_blocked: usize,
    /// Points left out because the box was too near an existing area.
    pub skipped_clear: usize,
    /// More areas would fit than [`MAX_AREAS`].
    pub capped: bool,
}

impl Preview {
    pub fn monsters(&self) -> u64 {
        self.areas.iter().flat_map(|a| a.mobs.iter()).map(|e| e.count as u64).sum()
    }

    /// "24 areas · 58 monsters · lv 12-15 · 3 points on blocked ground skipped" (the level part needs levels).
    pub fn summary(&self, level: &dyn Fn(u32) -> Option<f32>) -> String {
        let n = self.areas.len();
        let mut parts = vec![format!("{n} area{}", if n == 1 { "" } else { "s" })];
        if n > 0 {
            let m = self.monsters();
            parts.push(format!("{m} monster{}", if m == 1 { "" } else { "s" }));
            let levels: Vec<f32> = self.areas.iter().flat_map(|a| a.mobs.iter()).filter_map(|e| level(e.mob)).filter(|l| *l > 0.0).collect();
            if let (Some(lo), Some(hi)) = (levels.iter().copied().reduce(f32::min), levels.iter().copied().reduce(f32::max)) {
                parts.push(if lo.round() == hi.round() { format!("lv {}", lo.round()) } else { format!("lv {}\u{2013}{}", lo.round(), hi.round()) });
            }
        }
        for (k, what) in [(self.skipped_blocked, "on blocked ground"), (self.skipped_clear, "near existing areas")] {
            if k > 0 {
                parts.push(format!("{k} point{} {what} skipped", if k == 1 { "" } else { "s" }));
            }
        }
        if self.capped {
            parts.push(format!("Capped at {} areas.", crate::scatter::thousands(MAX_AREAS)));
        }
        parts.join(" · ")
    }
}

/// 2000 as "2,000".
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The free share of the cells under a box, and whether its centre cell is free.
fn free_share(blocked: &BitGrid, size: u32, min: [f32; 3], max: [f32; 3]) -> (f32, bool) {
    let cell = |v: f32| (v.floor().max(0.0) as u32).min(size.saturating_sub(1));
    let (x0, x1, z0, z1) = (cell(min[0]), cell(max[0] - 1e-3), cell(min[2]), cell(max[2] - 1e-3));
    let (mut free, mut total) = (0u32, 0u32);
    for z in z0..=z1 {
        for x in x0..=x1 {
            total += 1;
            if !blocked.get(x, z) {
                free += 1;
            }
        }
    }
    let centre_free = !blocked.get(cell((min[0] + max[0]) * 0.5), cell((min[2] + max[2]) * 0.5));
    (free as f32 / total.max(1) as f32, centre_free)
}

/// Whether `[min, max]` (grown by `g` on every side) overlaps a box of `existing`.
fn overlaps(existing: &[[f32; 4]], min: [f32; 3], max: [f32; 3], g: f32) -> bool {
    existing.iter().any(|e| min[0] - g < e[2] && max[0] + g > e[0] && min[2] - g < e[3] && max[2] + g > e[1])
}

fn pick(rng: &mut Pcg32, mobs: &[(u32, u32)], skip: Option<u32>) -> Option<u32> {
    let total: u32 = mobs.iter().filter(|m| Some(m.0) != skip).map(|m| m.1.max(1)).sum();
    if total == 0 {
        return None;
    }
    let mut r = rng.below(total);
    for m in mobs.iter().filter(|m| Some(m.0) != skip) {
        let w = m.1.max(1);
        if r < w {
            return Some(m.0);
        }
        r -= w;
    }
    None
}

/// The areas a scatter makes: the same for the same inputs. No monsters, no region or a region outside the map give none.
pub fn generate(p: &Params, region: &Region, site: &Site) -> Preview {
    let mut out = Preview::default();
    let (Some(rb), false) = (region.bounds(), p.mobs.is_empty()) else { return out };
    let half = p.size * 0.5;
    let map = site.size as f32;
    // a box this close to the edge would round out of the map
    let (lo, hi) = (half + 0.1, map - half - 0.1);
    if hi < lo {
        return out;
    }
    let rect = [rb[0].max(lo), rb[1].max(lo), rb[2].min(hi), rb[3].min(hi)];
    let seeds: Vec<[f32; 4]> = region.shapes.iter().filter_map(Shape::bounds).map(|b| [b[0].max(rect[0]), b[1].max(rect[1]), b[2].min(rect[2]), b[3].min(rect[3])]).filter(|b| b[0] <= b[2] && b[1] <= b[3]).collect();
    let mut rng = Pcg32::new(p.seed as u64, 1);
    let accept = |x: f32, z: f32| region.contains(tenth(x), tenth(z));
    let pts = poisson(&mut rng, rect, &seeds, p.spacing, &accept, MAX_POINTS);
    out.points = pts.len();
    let mut rng2 = Pcg32::new(p.seed as u64, 2);
    for q in pts {
        let (cx, cz) = (tenth(q[0]), tenth(q[1]));
        let (min, max) = ([tenth(cx - half), 0.0, tenth(cz - half)], [tenth(cx + half), 2.0, tenth(cz + half)]);
        if p.avoid_blocked {
            let (share, centre) = free_share(site.blocked, site.size, min, max);
            if !centre || share < MIN_FREE {
                out.skipped_blocked += 1;
                continue;
            }
        }
        if p.keep_clear && overlaps(site.existing, min, max, p.spacing * 0.5) {
            out.skipped_clear += 1;
            continue;
        }
        if out.areas.len() >= MAX_AREAS {
            out.capped = true;
            break;
        }
        let Some(first) = pick(&mut rng2, &p.mobs, None) else { break };
        let count = p.count_min + rng2.below(p.count_max.saturating_sub(p.count_min) + 1);
        let mut mobs = vec![MobEntry { mob: first, count }];
        if p.two_kinds && rng2.unit() < SECOND_KIND {
            if let Some(second) = pick(&mut rng2, &p.mobs, Some(first)) {
                mobs.push(MobEntry { mob: second, count: 1 });
            }
        }
        let mut rec = Rec::Mob(MobArea { min, max, mobs });
        edit::drop_to_ground(&mut rec, site.ground);
        if let Rec::Mob(a) = rec {
            out.areas.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: f32, _: f32) -> f32 {
        0.0
    }

    #[allow(clippy::field_reassign_with_default)]
    fn normal_params() -> Params {
        let mut s = ScatterSettings::default();
        s.mobs = vec![[5, 2], [6, 1]];
        s.params()
    }

    #[test]
    fn pcg_and_poisson_deterministic() {
        // the reference output of pcg32 with seed 42 and stream 54 (the demo of the PCG C library)
        let mut r = Pcg32::new(42, 54);
        let first: Vec<u32> = (0..6).map(|_| r.next_u32()).collect();
        assert_eq!(first, vec![0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e]);
        // below() stays in range and unit() in [0, 1)
        let mut r = Pcg32::new(7, 1);
        assert!((0..1000).all(|_| r.below(7) < 7));
        assert!((0..1000).all(|_| (0.0..1.0).contains(&r.unit())));
        // the same seed gives the same points, another seed other points
        let rect = [0.0, 0.0, 200.0, 200.0];
        let run = |seed: u64| poisson(&mut Pcg32::new(seed, 1), rect, &[rect], 20.0, &|_, _| true, 10_000);
        let a = run(3);
        assert_eq!(a, run(3));
        assert_ne!(a, run(4));
        // every pair is at least r apart, every point is inside
        for (i, p) in a.iter().enumerate() {
            assert!(p[0] >= 0.0 && p[0] <= 200.0 && p[1] >= 0.0 && p[1] <= 200.0);
            for q in &a[i + 1..] {
                assert!((p[0] - q[0]).hypot(p[1] - q[1]) >= 20.0 - 1e-3, "{p:?} {q:?}");
            }
        }
        // the Normal preset on a free 200 x 200 square
        let blocked = BitGrid::new(512);
        let site = Site { size: 512, blocked: &blocked, existing: &[], ground: &flat };
        let region = Region::rect([100.0, 100.0], [300.0, 300.0]);
        let mut sizes = Vec::new();
        for seed in 1..=10u32 {
            let mut p = normal_params();
            p.seed = seed;
            let pv = generate(&p, &region, &site);
            assert_eq!(pv, generate(&p, &region, &site), "deterministic");
            assert!((50..=115).contains(&pv.areas.len()), "seed {seed}: {} areas", pv.areas.len());
            sizes.push(pv.areas.len());
        }
        println!("Normal on 200 x 200: {sizes:?}");
        let pv = generate(&normal_params(), &region, &site);
        for a in &pv.areas {
            assert!((a.max[1] - a.min[1] - 2.0).abs() < 1e-4, "slab 2");
            assert_eq!(a.mobs.len(), 1);
            assert_eq!(a.mobs[0].count, 2, "Normal is 2 per box");
            assert!(a.max[0] - a.min[0] > 8.9 && a.max[0] - a.min[0] < 9.1, "box side 9");
        }
    }

    #[test]
    fn scatter_respects_region_blocked_and_clearance() {
        let mut blocked = BitGrid::new(256);
        // the left half of the square is blocked
        blocked.fill_rect(0, 0, 100, 256, true);
        let existing = [[130.0f32, 130.0, 150.0, 150.0]];
        let site = Site { size: 256, blocked: &blocked, existing: &existing, ground: &flat };
        // an L: the lower bar and the left bar of a 100 x 100 square
        let l = Region::poly(vec![[60.0, 60.0], [160.0, 60.0], [160.0, 100.0], [100.0, 100.0], [100.0, 160.0], [60.0, 160.0]]);
        let mut p = normal_params();
        p.keep_clear = true;
        let pv = generate(&p, &l, &site);
        assert!(!pv.areas.is_empty());
        for a in &pv.areas {
            let (cx, cz) = ((a.min[0] + a.max[0]) * 0.5, (a.min[2] + a.max[2]) * 0.5);
            assert!(l.contains(cx, cz), "centre ({cx}, {cz}) is outside the L");
            let (share, centre) = free_share(&blocked, 256, a.min, a.max);
            assert!(centre && share >= 0.7, "box at ({cx}, {cz}) has {share} free");
            assert!(!overlaps(&existing, a.min, a.max, 10.0), "box at ({cx}, {cz}) is too near the existing area");
        }
        assert!(pv.skipped_blocked > 0, "the blocked half skipped some points");
        // with both options off nothing is skipped and there are more areas
        p.avoid_blocked = false;
        p.keep_clear = false;
        let all = generate(&p, &l, &site);
        assert_eq!((all.skipped_blocked, all.skipped_clear), (0, 0));
        assert!(all.areas.len() > pv.areas.len());
        // the preview says what it did
        let s = pv.summary(&|id| Some(id as f32 * 3.0));
        assert!(s.contains(" areas") && s.contains("monsters") && s.contains("lv 15\u{2013}18") && s.contains("on blocked ground skipped"), "{s}");
        // a region off the map or no monsters give nothing
        assert!(generate(&p, &Region::rect([400.0, 400.0], [500.0, 500.0]), &site).areas.is_empty());
        let mut none = p.clone();
        none.mobs.clear();
        assert!(generate(&none, &l, &site).areas.is_empty());
        // a box never leaves the map
        let edge = generate(&p, &Region::rect([-50.0, -50.0], [400.0, 400.0]), &site);
        assert!(edge.areas.iter().all(|a| a.min[0] >= 0.0 && a.min[2] >= 0.0 && a.max[0] <= 256.0 && a.max[2] <= 256.0));
    }

    #[test]
    fn a_region_of_two_parts_gets_points_in_both() {
        let blocked = BitGrid::new(512);
        let site = Site { size: 512, blocked: &blocked, existing: &[], ground: &flat };
        let r = Region::from_boxes(&[[20.0, 20.0, 80.0, 80.0], [300.0, 300.0, 360.0, 360.0]]);
        let pv = generate(&normal_params(), &r, &site);
        let left = pv.areas.iter().filter(|a| a.min[0] < 100.0).count();
        assert!(left > 0 && pv.areas.len() > left, "{left} of {}", pv.areas.len());
    }

    #[test]
    fn counts_weights_second_kinds_presets_and_the_cap() {
        let blocked = BitGrid::new(2048);
        let site = Site { size: 2048, blocked: &blocked, existing: &[], ground: &flat };
        // weights: 9 to 1 gives mostly the first monster
        let mut s = ScatterSettings { preset: Preset::Dense, mobs: vec![[5, 9], [6, 1]], two_kinds: true, ..Default::default() };
        let r = Region::rect([100.0, 100.0], [500.0, 500.0]);
        let pv = generate(&s.params(), &r, &site);
        let first = pv.areas.iter().filter(|a| a.mobs[0].mob == 5).count();
        assert!(first * 10 > pv.areas.len() * 7, "{first} of {}", pv.areas.len());
        assert!(pv.areas.iter().all(|a| (2..=3).contains(&a.mobs[0].count)), "Dense is 2 to 3");
        let two = pv.areas.iter().filter(|a| a.mobs.len() == 2).count();
        assert!(two > 0 && two * 2 < pv.areas.len(), "{two} two-kind areas of {}", pv.areas.len());
        assert!(pv.areas.iter().filter(|a| a.mobs.len() == 2).all(|a| a.mobs[0].mob != a.mobs[1].mob && a.mobs[1].count == 1));
        // the cap
        s.preset = Preset::Custom;
        s.spacing = 4.0;
        let big = generate(&s.params(), &Region::rect([20.0, 20.0], [2000.0, 2000.0]), &site);
        assert_eq!(big.areas.len(), MAX_AREAS);
        assert!(big.capped && big.summary(&|_| None).contains("Capped at 2,000 areas."));
        // presets and clamping
        assert_eq!(Preset::Normal.values(), Some((20.0, 9.0, 2, 2)));
        let c = ScatterSettings { spacing: f32::NAN, size: 900.0, count_min: 0, count_max: 99, mobs: vec![[1, 0]; 40], ..Default::default() }.clamped();
        assert_eq!((c.spacing, c.size, c.count_min, c.count_max, c.mobs.len(), c.mobs[0][1]), (20.0, 64.0, 1, 8, MAX_MOBS, 1));
        let mut a = ScatterSettings::default();
        a.add_mob(7);
        a.add_mob(7);
        assert_eq!(a.mobs, vec![[7, 2]]);
        // settings written and read back
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<ScatterSettings>(&text).unwrap(), c);
        assert_eq!(serde_json::from_str::<ScatterSettings>("{}").unwrap(), ScatterSettings::default());
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn polygons_use_the_even_odd_rule_and_input_closes() {
        let sq = Region::poly(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        assert!(sq.contains(5.0, 5.0) && !sq.contains(11.0, 5.0) && !sq.contains(5.0, -1.0));
        let mut i = Input::default();
        i.poly = vec![[0.0, 0.0], [5.0, 0.0]];
        assert!(i.active() && !i.close_poly(), "two points are not a polygon");
        i.poly.push([5.0, 5.0]);
        let rev = i.rev;
        assert!(i.close_poly() && i.region.is_some() && i.poly.is_empty() && i.rev > rev);
        i.clear();
        assert!(!i.active());
        assert_eq!(Region::rect([5.0, 9.0], [1.0, 2.0]).bounds(), Some([1.0, 2.0, 5.0, 9.0]));
    }
}
