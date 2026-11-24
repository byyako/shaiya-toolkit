//! Blocked-cell generation: a candidate bitmap computed from the client's own geometry.
//! Nothing here edits a map; the app shows the candidate as a preview and the user applies it (one undo step) or cancels.
//!
//! * dungeons: a cell is free when an upward-facing triangle of the `.dg` collision mesh covers its centre; every other cell is
//!   blocked. The originals' corridors are all free, their rooms mostly free, and the rest (rock, walls) blocked.
//! * fields: a cell is blocked when the terrain around it is steeper than a chosen angle.

use shaiya_clientdata::wld::Terrain;
use shaiya_svmap::BitGrid;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DungeonParams {
    /// A triangle steeper than this (angle of its normal from straight up, degrees) is a wall, not floor.
    pub max_slope_deg: f32,
}

impl Default for DungeonParams {
    fn default() -> Self {
        DungeonParams { max_slope_deg: 50.0 }
    }
}

/// The blocked bitmap of a dungeon: `true` everywhere no walkable collision triangle reaches. `points`/`indices` are the
/// flattened collision mesh (`DgFlat::collision_points` / `collision_indices`).
pub fn dungeon_blocked(points: &[[f32; 3]], indices: &[u32], size: u32, p: &DungeonParams) -> BitGrid {
    let s = size as i64;
    let mut blocked = BitGrid::new(size);
    blocked.fill_all(true);
    let cos_limit = p.max_slope_deg.to_radians().cos();
    for tri in indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (points.get(tri[0] as usize), points.get(tri[1] as usize), points.get(tri[2] as usize)) else {
            continue;
        };
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-9 {
            continue;
        }
        let ny = n[1] / len;
        // only triangles whose winding faces up: the other winding is a ceiling (found by experiment)
        if ny < cos_limit {
            continue;
        }
        let x0 = (a[0].min(b[0]).min(c[0]).floor() as i64).max(0);
        let x1 = (a[0].max(b[0]).max(c[0]).ceil() as i64).min(s - 1);
        let z0 = (a[2].min(b[2]).min(c[2]).floor() as i64).max(0);
        let z1 = (a[2].max(b[2]).max(c[2]).ceil() as i64).min(s - 1);
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-12 {
            continue;
        }
        for z in z0..=z1 {
            for x in x0..=x1 {
                if !blocked.get(x as u32, z as u32) {
                    continue;
                }
                let inside = |px: f32, pz: f32| {
                    let w1 = ((b[2] - c[2]) * (px - c[0]) + (c[0] - b[0]) * (pz - c[2])) / d;
                    let w2 = ((c[2] - a[2]) * (px - c[0]) + (a[0] - c[0]) * (pz - c[2])) / d;
                    let w3 = 1.0 - w1 - w2;
                    w1 >= 0.0 && w2 >= 0.0 && w3 >= 0.0
                };
                let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
                let hit = inside(cx, cz);
                if hit {
                    blocked.set(x as u32, z as u32, false);
                }
            }
        }
    }
    blocked
}

/// The blocked bitmap of a field from the terrain's slope: a cell is blocked when the terrain's gradient around its centre is
/// steeper than `max_slope_deg` (the gradient is taken over one cell either side of the centre).
pub fn slope_blocked(t: &Terrain, size: u32, max_slope_deg: f32) -> BitGrid {
    let mut g = BitGrid::new(size);
    let limit = max_slope_deg.to_radians().tan();
    for z in 0..size {
        for x in 0..size {
            let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
            let dx = (t.height_at(cx + 1.0, cz) - t.height_at(cx - 1.0, cz)) / 2.0;
            let dz = (t.height_at(cx, cz + 1.0) - t.height_at(cx, cz - 1.0)) / 2.0;
            if (dx * dx + dz * dz).sqrt() > limit {
                g.set(x, z, true);
            }
        }
    }
    g
}

/// How a generated bitmap compares with a reference (the original file's bitmap).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Agreement {
    pub cells: u64,
    pub equal: u64,
    /// Blocked cells in the reference / in the candidate.
    pub ref_blocked: u64,
    pub gen_blocked: u64,
    /// Candidate-blocked cells that are blocked in the reference.
    pub both_blocked: u64,
}

impl Agreement {
    /// Share of cells with the same state, 0-100.
    pub fn percent(&self) -> f64 {
        if self.cells == 0 {
            100.0
        } else {
            self.equal as f64 * 100.0 / self.cells as f64
        }
    }

    /// What an "everything blocked" bitmap would score against the same reference, 0-100 (the baseline to beat).
    pub fn all_blocked_percent(&self) -> f64 {
        if self.cells == 0 {
            100.0
        } else {
            self.ref_blocked as f64 * 100.0 / self.cells as f64
        }
    }

    /// Of the cells the reference leaves free, the share the candidate also frees (recall of "free"), 0-100.
    pub fn free_recall(&self) -> f64 {
        let free = self.cells - self.ref_blocked;
        if free == 0 {
            100.0
        } else {
            (free - (self.gen_blocked - self.both_blocked)) as f64 * 100.0 / free as f64
        }
    }

    /// Of the cells the candidate frees, the share the reference also leaves free (precision of "free"), 0-100.
    pub fn free_precision(&self) -> f64 {
        let free = self.cells - self.gen_blocked;
        if free == 0 {
            100.0
        } else {
            (free - (self.ref_blocked - self.both_blocked)) as f64 * 100.0 / free as f64
        }
    }
}

pub fn agreement(candidate: &BitGrid, reference: &BitGrid) -> Option<Agreement> {
    let differ = candidate.differing_cells(reference)?;
    let cells = candidate.size() as u64 * candidate.size() as u64;
    let (gb, rb) = (candidate.count(), reference.count());
    // blocked in both = (gb + rb - differ) / 2, because differ = (gb - both) + (rb - both)
    let both = (gb + rb - differ) / 2;
    Some(Agreement { cells, equal: cells - differ, ref_blocked: rb, gen_blocked: gb, both_blocked: both })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32, flip: bool) -> (Vec<[f32; 3]>, Vec<u32>) {
        let pts = vec![[2.0, y, 2.0], [10.0, y, 2.0], [10.0, y, 10.0], [2.0, y, 10.0]];
        let idx = if flip { vec![0, 1, 2, 0, 2, 3] } else { vec![0, 2, 1, 0, 3, 2] };
        (pts, idx)
    }

    #[test]
    fn floor_cells_are_free_everything_else_blocked() {
        // one winding faces up in this crate's convention (cross product of the edges), the other faces down
        let up = [quad(5.0, true), quad(5.0, false)].into_iter().find(|(p, i)| dungeon_blocked(p, i, 16, &DungeonParams::default()).count() < 256).unwrap();
        let g = dungeon_blocked(&up.0, &up.1, 16, &DungeonParams::default());
        assert!(!g.get(5, 5) && !g.get(2, 2) && !g.get(9, 9));
        assert!(g.get(0, 0) && g.get(12, 12) && g.get(10, 5));
        assert_eq!(g.count(), 256 - 64);
        // the other winding is a ceiling: nothing is free
        let down = if up.1 == quad(5.0, true).1 { quad(5.0, false) } else { quad(5.0, true) };
        assert_eq!(dungeon_blocked(&down.0, &down.1, 16, &DungeonParams::default()).count(), 256);
    }

    #[test]
    fn walls_do_not_free_cells() {
        let wall = vec![[0.0, 0.0, 8.0], [8.0, 0.0, 8.0], [8.0, 9.0, 8.0]];
        for flip in [false, true] {
            let idx: [u32; 3] = if flip { [0, 2, 1] } else { [0, 1, 2] };
            assert_eq!(dungeon_blocked(&wall, &idx, 16, &DungeonParams::default()).count(), 256);
        }
    }

    #[test]
    fn slope_blocks_steep_cells_only() {
        let n = 9usize;
        let mut heights = vec![0i16; n * n];
        // a ramp rising 2 units of height per 2 units of distance (45 degrees) to the east of corner column 4
        for cz in 0..n {
            for cx in 4..n {
                heights[cz * n + cx] = ((cx - 4) as f32 * 2.0 / 0.02) as i16;
            }
        }
        let t = Terrain { size: 16, n, heights, tex_index: vec![0; n * n] };
        let steep = slope_blocked(&t, 16, 30.0);
        let gentle = slope_blocked(&t, 16, 60.0);
        assert!(steep.get(10, 5) && !steep.get(2, 5));
        assert_eq!(gentle.count(), 0);
    }

    #[test]
    fn agreement_numbers() {
        let mut a = BitGrid::new(8);
        let mut b = BitGrid::new(8);
        a.fill_rect(0, 0, 8, 4, true); // 32 blocked
        b.fill_rect(0, 0, 8, 2, true); // 16 blocked, all inside a's
        let ag = agreement(&a, &b).unwrap();
        assert_eq!((ag.cells, ag.equal, ag.gen_blocked, ag.ref_blocked, ag.both_blocked), (64, 48, 32, 16, 16));
        assert_eq!(ag.percent(), 75.0);
        assert_eq!(ag.all_blocked_percent(), 25.0);
        assert!((ag.free_recall() - 100.0 * 32.0 / 48.0).abs() < 1e-9);
        assert_eq!(ag.free_precision(), 100.0);
        assert!(agreement(&a, &BitGrid::new(4)).is_none());
    }

    /// Both data sets, or `None` (test skipped).
    fn data() -> Option<(crate::client_rules::ClientRules, std::path::PathBuf)> {
        let c = shaiya_testkit::env_client()?;
        let s = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?;
        if !(c.is_dir() && s.is_dir()) {
            return None;
        }
        Some((crate::client_rules::ClientRules::open(&c).ok()?, s))
    }

    /// Acceptance: how well "dungeon from collision" reproduces the original bitmaps of maps 3, 13 and 65, and that a
    /// generated bitmap applies and undoes as one step.
    #[test]
    fn dungeon_generation_agreement_with_the_originals() {
        let Some((rules, dir)) = data() else { return };
        for (id, at_least) in [(3u32, 99.0f64), (13, 93.0), (65, 97.0)] {
            let m = shaiya_svmap::read(&std::fs::read(dir.join(format!("{id}.svmap"))).unwrap()).unwrap();
            let w = rules.wld(id).unwrap();
            let flat = rules.source.dg(&w.dg).unwrap().flatten();
            let t0 = std::time::Instant::now();
            let g = dungeon_blocked(&flat.collision_points, &flat.collision_indices, m.size, &DungeonParams::default());
            let took = t0.elapsed();
            let a = agreement(&g, &m.blocked).unwrap();
            eprintln!(
                "generation agreement map {id} (size {}): {:.2} % of {} cells equal (all-blocked baseline {:.2} %), free recall {:.2} %, free precision {:.2} %, blocked original {} generated {}, {:.0} ms (debug build)",
                m.size, a.percent(), a.cells, a.all_blocked_percent(), a.free_recall(), a.free_precision(), a.ref_blocked, a.gen_blocked, took.as_secs_f64() * 1000.0
            );
            assert!(a.percent() >= at_least, "map {id}: {:.2} %", a.percent());
            assert!(a.percent() > a.all_blocked_percent(), "map {id} does no better than blocking everything");
            // applying the candidate to a copy of the map is one undo step, undo restores the original bitmap
            let mut doc = crate::doc::Doc::new(1, Some(id), None, m.clone(), None);
            let changed = doc.commit_blocked("generate", &g);
            assert_eq!(changed, m.blocked.differing_cells(&g).unwrap());
            assert_eq!(doc.map.blocked, g);
            assert_eq!(doc.undo.len(), 1);
            doc.undo();
            assert_eq!(doc.map, m);
        }
    }

    #[test]
    fn field_slope_on_map_1_blocks_only_steep_cells() {
        let Some((rules, dir)) = data() else { return };
        let m = shaiya_svmap::read(&std::fs::read(dir.join("1.svmap")).unwrap()).unwrap();
        let w = rules.wld(1).unwrap();
        let t = w.terrain.as_ref().unwrap();
        let cells = m.size as u64 * m.size as u64;
        let mut last = u64::MAX;
        for deg in [30.0f32, 50.0, 70.0] {
            let t0 = std::time::Instant::now();
            let g = slope_blocked(t, m.size, deg);
            eprintln!("field slope map 1 > {deg} deg: {} blocked of {cells} ({:.2} %), original {}, {:.0} ms (debug build)", g.count(), g.count() as f64 * 100.0 / cells as f64, m.blocked.count(), t0.elapsed().as_secs_f64() * 1000.0);
            assert!(g.count() < last, "a steeper limit blocks fewer cells");
            last = g.count();
        }
    }
}
