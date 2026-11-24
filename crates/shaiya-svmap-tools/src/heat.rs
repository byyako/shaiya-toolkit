//! The heatmap of a map's monster areas. Pure: no egui, no files. The map is cut into square
//! tiles; every monster area spreads its monsters over the tiles it covers, by overlap.
//!
//! * **Density**: monsters per tile. An area with total count `c` adds `c * overlap(area, tile) / area` to each tile.
//! * **Level**: the count-weighted mean level of the entries with a known level, with the same overlap weights. A tile whose
//!   weight is below [`MIN_WEIGHT`] is empty.
//!
//! An area with zero width or depth counts as a point at its centre. The part of an area outside the map adds nothing.

use serde::{Deserialize, Serialize};
use shaiya_svmap::{MobArea, Svmap};

use crate::names;

/// A tile whose level weight is below this has no level.
pub const MIN_WEIGHT: f32 = 0.05;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    Density,
    Level,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off",
            Mode::Density => "Density",
            Mode::Level => "Level",
        }
    }

    /// H: off, density, level, off.
    pub fn next(self) -> Mode {
        match self {
            Mode::Off => Mode::Density,
            Mode::Density => Mode::Level,
            Mode::Level => Mode::Off,
        }
    }
}

/// The tile sides the heatmap offers.
pub const TILES: [u32; 3] = [16, 32, 64];

/// The heatmap's settings (`heat` in the settings file).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatSettings {
    pub mode: Mode,
    pub tile: u32,
    pub opacity: f32,
    /// The density at the top of the colour ramp; None = the 95th percentile of this map's non-zero tiles.
    pub max: Option<f32>,
}

impl Default for HeatSettings {
    fn default() -> Self {
        HeatSettings { mode: Mode::Off, tile: 32, opacity: 0.6, max: None }
    }
}

impl HeatSettings {
    /// Settings read from a file or typed in: a tile side from [`TILES`], an opacity in 0.2 to 0.9, a positive maximum.
    pub fn clamped(mut self) -> HeatSettings {
        if !TILES.contains(&self.tile) {
            self.tile = 32;
        }
        self.opacity = if self.opacity.is_finite() { self.opacity.clamp(0.2, 0.9) } else { 0.6 };
        self.max = self.max.filter(|m| m.is_finite() && *m > 0.0);
        self
    }
}

/// The tile values of one map: `values[iz * n + ix]`, tile (0, 0) at the world origin, z growing up.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatGrid {
    pub n: usize,
    pub tile: f32,
    pub size: f32,
    pub values: Vec<f32>,
}

impl HeatGrid {
    pub fn empty(size: f32, tile: f32) -> HeatGrid {
        let n = tiles_per_side(size, tile);
        HeatGrid { n, tile, size, values: vec![0.0; n * n] }
    }

    /// The value of the tile under a world position (None outside the map).
    pub fn at(&self, x: f32, z: f32) -> Option<f32> {
        if !(x >= 0.0 && z >= 0.0 && x < self.size && z < self.size) {
            return None;
        }
        let (ix, iz) = ((x / self.tile) as usize, (z / self.tile) as usize);
        self.values.get(iz.min(self.n - 1) * self.n + ix.min(self.n - 1)).copied()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn sum(&self) -> f32 {
        self.values.iter().sum()
    }

    /// The 95th percentile of the non-zero tiles (the colour scale's "auto" maximum); 0 when every tile is empty.
    pub fn auto_max(&self) -> f32 {
        let mut v: Vec<f32> = self.values.iter().copied().filter(|v| *v > 0.0).collect();
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        v[((v.len() - 1) as f32 * 0.95).floor() as usize]
    }

    /// The picture: `n * n` RGBA pixels, the top row is the highest z. Density uses [`density_rgba`] against `max`, level
    /// uses `names::level_colour`; an empty tile is transparent. Opacity is applied when the picture is drawn.
    pub fn rgba(&self, mode: Mode, max: f32) -> Vec<u8> {
        let mut out = vec![0u8; self.n * self.n * 4];
        for iz in 0..self.n {
            for ix in 0..self.n {
                let v = self.values[iz * self.n + ix];
                let px = match mode {
                    Mode::Off => [0, 0, 0, 0],
                    Mode::Density => density_rgba(if max > 0.0 { v / max } else { 0.0 }),
                    Mode::Level if v > 0.0 => {
                        let c = names::level_colour(Some(v));
                        [c[0], c[1], c[2], 255]
                    }
                    Mode::Level => [0, 0, 0, 0],
                };
                let o = ((self.n - 1 - iz) * self.n + ix) * 4;
                out[o..o + 4].copy_from_slice(&px);
            }
        }
        out
    }
}

pub fn tiles_per_side(size: f32, tile: f32) -> usize {
    ((size / tile).ceil() as usize).max(1)
}

/// The part of the heat picture that covers the map, as `[u0, v0, u1, v1]`. The `n` tiles span `n * tile` units, more than the
/// map when its side is not a multiple of the tile (most dungeons: 120, 136, 848...); the extra strip is at high x and high z,
/// which is the right edge and the top row of the picture.
pub fn texture_uv(size: f32, tile: f32) -> [f32; 4] {
    let span = tiles_per_side(size, tile) as f32 * tile;
    let s = if span > 0.0 { (size / span).clamp(0.0, 1.0) } else { 1.0 };
    [0.0, 1.0 - s, s, 1.0]
}

/// The stops of the density ramp, magma-like: `#3b0f70`, `#8c2981`, `#de4968`, `#fe9f6d`, `#fcfdbf`.
pub const RAMP: [[u8; 3]; 5] = [[0x3b, 0x0f, 0x70], [0x8c, 0x29, 0x81], [0xde, 0x49, 0x68], [0xfe, 0x9f, 0x6d], [0xfc, 0xfd, 0xbf]];

/// The colour of a density at `t` = value / maximum (0 = transparent, 1 and above = the top of the ramp).
pub fn density_rgba(t: f32) -> [u8; 4] {
    if t.is_nan() || t <= 0.0 {
        return [0, 0, 0, 0];
    }
    let t = t.min(1.0);
    let f = t * 4.0;
    let k = (f.floor() as usize).min(3);
    let u = f - k as f32;
    let c = |i: usize| (RAMP[k][i] as f32 + (RAMP[k + 1][i] as f32 - RAMP[k][i] as f32) * u).round() as u8;
    // the first stretch fades in from transparent so a nearly empty tile does not show as a solid purple square
    let a = (t * 8.0).min(1.0);
    [c(0), c(1), c(2), (a * 255.0).round() as u8]
}

/// Calls `f(tile index, fraction)` for every tile a monster area covers; the fractions add up to 1 for an area inside the map.
fn spread(a: &MobArea, size: f32, tile: f32, n: usize, f: &mut dyn FnMut(usize, f64)) {
    let (x0, x1) = (a.min[0].min(a.max[0]) as f64, a.min[0].max(a.max[0]) as f64);
    let (z0, z1) = (a.min[2].min(a.max[2]) as f64, a.min[2].max(a.max[2]) as f64);
    let (w, d) = (x1 - x0, z1 - z0);
    let (size, t) = (size as f64, tile as f64);
    let tile_of = |v: f64| ((v / t).floor().max(0.0) as usize).min(n - 1);
    if w <= 1e-6 || d <= 1e-6 {
        let (cx, cz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
        if cx >= 0.0 && cz >= 0.0 && cx <= size && cz <= size {
            f(tile_of(cz) * n + tile_of(cx), 1.0);
        }
        return;
    }
    let (ix0, ix1) = (tile_of(x0.max(0.0)), tile_of(x1.min(size)));
    let (iz0, iz1) = (tile_of(z0.max(0.0)), tile_of(z1.min(size)));
    if x1 <= 0.0 || z1 <= 0.0 || x0 >= size || z0 >= size {
        return;
    }
    for iz in iz0..=iz1 {
        let oz = (z1.min((iz + 1) as f64 * t).min(size) - z0.max(iz as f64 * t)).max(0.0) / d;
        if oz <= 0.0 {
            continue;
        }
        for ix in ix0..=ix1 {
            let ox = (x1.min((ix + 1) as f64 * t).min(size) - x0.max(ix as f64 * t)).max(0.0) / w;
            if ox > 0.0 {
                f(iz * n + ix, ox * oz);
            }
        }
    }
}

/// The heatmap of `map`. `level(mob)` is the monster's level (None = unknown); `include(i)` says whether monster area `i`
/// counts (hidden layer, level filter). `Mode::Off` gives an empty grid.
#[cfg(any(test, feature = "testing"))]
pub fn grid(map: &Svmap, level: &dyn Fn(u32) -> Option<f32>, tile: f32, mode: Mode, include: &dyn Fn(usize) -> bool) -> HeatGrid {
    grid_with(map, &[], level, tile, mode, include)
}

/// [`grid`] with more areas counted besides the map's (the Scatter tool's preview): they always count.
pub fn grid_with(map: &Svmap, extra: &[MobArea], level: &dyn Fn(u32) -> Option<f32>, tile: f32, mode: Mode, include: &dyn Fn(usize) -> bool) -> HeatGrid {
    let areas: Vec<&MobArea> = map.mob_areas.iter().enumerate().filter(|(i, _)| include(*i)).map(|(_, a)| a).chain(extra.iter()).collect();
    let size = map.size as f32;
    let mut g = HeatGrid::empty(size, tile);
    let n = g.n;
    match mode {
        Mode::Off => {}
        Mode::Density => {
            let mut acc = vec![0f64; n * n];
            for a in &areas {
                let total: f64 = a.mobs.iter().map(|e| e.count as f64).sum();
                if total <= 0.0 {
                    continue;
                }
                spread(a, size, tile, n, &mut |t, fr| acc[t] += total * fr);
            }
            g.values = acc.into_iter().map(|v| v as f32).collect();
        }
        Mode::Level => {
            let mut wsum = vec![0f64; n * n];
            let mut lsum = vec![0f64; n * n];
            for a in &areas {
                let (mut w, mut wl) = (0f64, 0f64);
                for e in &a.mobs {
                    if let Some(l) = level(e.mob).filter(|l| *l > 0.0) {
                        w += e.count as f64;
                        wl += e.count as f64 * l as f64;
                    }
                }
                if w <= 0.0 {
                    continue;
                }
                spread(a, size, tile, n, &mut |t, fr| {
                    wsum[t] += w * fr;
                    lsum[t] += wl * fr;
                });
            }
            for t in 0..n * n {
                if wsum[t] >= MIN_WEIGHT as f64 {
                    g.values[t] = (lsum[t] / wsum[t]) as f32;
                }
            }
        }
    }
    g
}

/// The two grids the viewport and the status bar need (the colour layer shows `mode`, the status bar reads both).
#[derive(Clone, Debug, PartialEq)]
pub struct HeatPair {
    pub density: HeatGrid,
    pub level: Option<HeatGrid>,
}

/// The status bar text of a tile: "3.2 per tile · lv 14" (the level part only with client levels).
pub fn status_text(p: &HeatPair, x: f32, z: f32) -> Option<String> {
    let d = p.density.at(x, z)?;
    let mut s = format!("{d:.1} per tile");
    if let Some(l) = p.level.as_ref().and_then(|g| g.at(x, z)).filter(|l| *l > 0.0) {
        s.push_str(&format!(" · lv {}", l.round() as i32));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::MobEntry;

    fn area(x0: f32, z0: f32, x1: f32, z1: f32, mobs: &[(u32, u32)]) -> MobArea {
        MobArea { min: [x0, 0.0, z0], max: [x1, 2.0, z1], mobs: mobs.iter().map(|(m, c)| MobEntry { mob: *m, count: *c }).collect() }
    }

    fn map_with(areas: Vec<MobArea>) -> Svmap {
        let mut m = Svmap::new(128);
        m.mob_areas = areas;
        m
    }

    #[test]
    fn heat_density_conserves_count() {
        let m = map_with(vec![
            area(30.0, 30.0, 34.0, 34.0, &[(1, 3), (2, 1)]), // across four 32-unit tiles
            area(5.0, 5.0, 25.0, 25.0, &[(1, 5)]),            // inside one tile
            area(100.0, 10.0, 100.0, 40.0, &[(3, 2)]),        // zero width: a point at its centre
            area(60.0, 60.0, 90.0, 120.0, &[(2, 7)]),         // spans several tiles
        ]);
        let g = grid(&m, &|_| None, 32.0, Mode::Density, &|_| true);
        let counted: f32 = 4.0 + 5.0 + 2.0 + 7.0;
        assert!((g.sum() - counted).abs() < 1e-3, "sum {} vs {counted}", g.sum());
        // the first area straddles the corner (32, 32): 2 x 2 units in each of the four tiles
        for (ix, iz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let want = 4.0 * 0.25 + if (ix, iz) == (0, 0) { 5.0 } else { 0.0 };
            let got = g.values[iz * g.n + ix];
            // tile (1, 1) also holds part of the fourth area (60..90 x 60..120 starts in tile 1)
            if (ix, iz) == (1, 1) {
                assert!(got > want);
            } else {
                assert!((got - want).abs() < 1e-4, "tile ({ix},{iz}) {got} vs {want}");
            }
        }
        // the point-like area sits in the tile of its centre (100, 25): tile (3, 0)
        assert!((g.values[3] - 2.0).abs() < 1e-5);
        // an area partly outside the map adds only the part inside
        let m = map_with(vec![area(100.0, 10.0, 156.0, 30.0, &[(1, 4)])]);
        let g = grid(&m, &|_| None, 32.0, Mode::Density, &|_| true);
        assert!((g.sum() - 2.0).abs() < 1e-3, "{}", g.sum());
    }

    #[test]
    fn heat_level_weighted() {
        // one tile: monster 1 (level 10) x3 and monster 2 (level 30) x1 in the same area: (30 + 30) / 4 = 15
        let m = map_with(vec![area(2.0, 2.0, 12.0, 12.0, &[(1, 3), (2, 1)]), area(40.0, 2.0, 50.0, 12.0, &[(3, 4)])]);
        let level = |id: u32| match id {
            1 => Some(10.0),
            2 => Some(30.0),
            _ => None, // monster 3 has no level
        };
        let g = grid(&m, &level, 32.0, Mode::Level, &|_| true);
        assert!((g.values[0] - 15.0).abs() < 1e-4, "{}", g.values[0]);
        // the unknown monster adds nothing: that tile is empty
        assert_eq!(g.values[1], 0.0);
        // a second area in the same tile averages by weight: 15 * 4 + 40 * 4 over 8 = 27.5
        let m = map_with(vec![area(2.0, 2.0, 12.0, 12.0, &[(1, 3), (2, 1)]), area(14.0, 2.0, 24.0, 12.0, &[(4, 4)])]);
        let level = |id: u32| match id {
            1 => Some(10.0),
            2 => Some(30.0),
            4 => Some(40.0),
            _ => None,
        };
        let g = grid(&m, &level, 32.0, Mode::Level, &|_| true);
        assert!((g.values[0] - 27.5).abs() < 1e-4, "{}", g.values[0]);
        // a tile with almost no weight is empty (a tiny sliver of an area)
        let m = map_with(vec![area(31.99, 2.0, 63.99, 12.0, &[(1, 1)])]);
        let g = grid(&m, &level, 32.0, Mode::Level, &|_| true);
        assert_eq!(g.values[0], 0.0, "weight {} is below the minimum", 0.01 / 32.0);
        assert!(g.values[1] > 0.0);
    }

    #[test]
    fn heat_respects_filters() {
        let m = map_with(vec![area(2.0, 2.0, 12.0, 12.0, &[(1, 3)]), area(40.0, 2.0, 50.0, 12.0, &[(2, 4)])]);
        let level = |id: u32| Some(if id == 1 { 10.0 } else { 50.0 });
        // a hidden layer counts nothing
        let g = grid(&m, &level, 32.0, Mode::Density, &|_| false);
        assert_eq!(g.sum(), 0.0);
        // a level filter that keeps area 1 only (as the viewport's filter would)
        let keep = |i: usize| i == 0;
        let g = grid(&m, &level, 32.0, Mode::Density, &keep);
        assert!((g.sum() - 3.0).abs() < 1e-5);
        let g = grid(&m, &level, 32.0, Mode::Level, &keep);
        assert!((g.values[0] - 10.0).abs() < 1e-5 && g.values[1] == 0.0);
        // Off gives nothing
        assert_eq!(grid(&m, &level, 32.0, Mode::Off, &|_| true).sum(), 0.0);
    }

    #[test]
    fn ramp_and_scale() {
        assert_eq!(density_rgba(0.0)[3], 0);
        assert_eq!(density_rgba(1.0), [0xfc, 0xfd, 0xbf, 255]);
        assert_eq!(density_rgba(5.0), density_rgba(1.0), "above the maximum clamps");
        assert_eq!(density_rgba(0.5)[..3], [0xde, 0x49, 0x68]);
        let mut g = HeatGrid::empty(128.0, 32.0);
        assert_eq!(g.auto_max(), 0.0);
        for (i, v) in g.values.iter_mut().enumerate().take(10) {
            *v = (i + 1) as f32;
        }
        // ten non-zero tiles 1..10: the 95th percentile index is floor(9 * 0.95) = 8, the value 9
        assert_eq!(g.auto_max(), 9.0);
        // the picture is flipped: tile row 0 (z = 0) is the bottom row of pixels
        let px = g.rgba(Mode::Density, 10.0);
        let bottom_left = ((g.n - 1) * g.n) * 4;
        assert!(px[bottom_left + 3] > 0 && px[3] == 0);
        assert_eq!(g.at(1.0, 1.0), Some(1.0));
        assert_eq!(g.at(-1.0, 5.0), None);
        assert_eq!(HeatSettings { tile: 33, opacity: 7.0, max: Some(-1.0), ..Default::default() }.clamped(), HeatSettings { mode: Mode::Off, tile: 32, opacity: 0.9, max: None });
        assert_eq!(Mode::Off.next().next().next(), Mode::Off);
    }

    /// Review: a map whose side is not a multiple of the tile (120 at tile 32: 4 tiles = 128 units) draws only the part of the
    /// picture that covers the map, so a tile sits over its own ground.
    #[test]
    fn heat_texture_fits_maps_that_are_not_a_multiple_of_the_tile() {
        assert_eq!(texture_uv(2048.0, 32.0), [0.0, 0.0, 1.0, 1.0]);
        let [u0, v0, u1, v1] = texture_uv(120.0, 32.0);
        assert_eq!((u0, v1), (0.0, 1.0));
        assert!((u1 - 0.9375).abs() < 1e-6 && (v0 - 0.0625).abs() < 1e-6, "{u1} {v0}");
        // x = 100 (tile 3) lands in pixel column 3; z = 119 (tile row 3, the top row of the picture) in pixel row 0
        let n = tiles_per_side(120.0, 32.0) as f32;
        assert!(((u0 + (u1 - u0) * 100.0 / 120.0) * n - 3.125).abs() < 1e-4);
        let row = (v0 + (v1 - v0) * (1.0 - 119.0 / 120.0)) * n;
        assert!((0.0..1.0).contains(&row), "{row}");
        // and z = 1 (tile row 0) in the bottom pixel row
        assert!((v0 + (v1 - v0) * (1.0 - 1.0 / 120.0)) * n > n - 1.0);
        // a map smaller than one tile
        let [_, v0, u1, _] = texture_uv(16.0, 64.0);
        assert!((u1 - 0.25).abs() < 1e-6 && (v0 - 0.75).abs() < 1e-6);
    }

    /// Map 1 of the originals at tile 32: the time is printed (about 10 ms in release, 60 ms in a debug build), not asserted.
    #[test]
    fn heat_map1_fast() {
        let Some(dir) = shaiya_testkit::env_var_path("SVMAP_ORIGINALS") else {
            return;
        };
        let bytes = std::fs::read(std::path::Path::new(&dir).join("1.svmap")).unwrap();
        let m = shaiya_svmap::read(&bytes).unwrap();
        assert!(!m.mob_areas.is_empty());
        let level = |id: u32| Some(1.0 + (id % 80) as f32);

        for mode in [Mode::Density, Mode::Level] {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let g = grid(&m, &level, 32.0, mode, &|_| true);
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(g.n, 64);
            }
            println!("heat {mode:?} at tile 32: best of 5 {best:.2} ms (no timing assertion)");
        }
        let counted: f64 = m.mob_areas.iter().map(|a| a.mobs.iter().map(|e| e.count as f64).sum::<f64>()).sum();
        let g = grid(&m, &level, 32.0, Mode::Density, &|_| true);
        assert!((g.sum() as f64 - counted).abs() < 1.0 + counted * 1e-4, "{} vs {counted}", g.sum());
    }
}
