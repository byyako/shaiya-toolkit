//! Client-derived pictures and lookups for one map, built on a worker thread: the hillshaded terrain, the client minimap, the
//! dungeon floor raster, the `.wld` placements and the ground-height function. Nothing here touches the GPU; the app turns
//! the images into egui textures.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use shaiya_clientdata::text::AreaNames;
use shaiya_clientdata::wld::{Terrain, Wld, CORNER_SPACING};

use crate::client_rules::ClientRules;

/// Average colour per terrain texture file, shared by all maps (decoding 60 textures once is enough).
#[derive(Default)]
pub struct TintCache(Mutex<HashMap<String, [u8; 3]>>);

impl TintCache {
    fn get(&self, rules: &ClientRules, name: &str, problems: &mut Vec<String>) -> [u8; 3] {
        if name.trim().is_empty() {
            return [128, 128, 128]; // an unused slot of the texture list
        }
        let key = name.to_ascii_lowercase();
        if let Some(c) = self.0.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return *c;
        }
        let colour = match rules.source.texture_flex("Terrain/Detail", name) {
            Ok(img) => img.average_colour(),
            Err(e) => {
                problems.push(e.to_string());
                [128, 128, 128]
            }
        };
        self.0.lock().unwrap_or_else(|p| p.into_inner()).insert(key, colour);
        colour
    }
}

/// What an object placement is (for the footprint dots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjKind {
    Building,
    Shape,
    Tree,
}

/// Height under a world position.
pub enum Ground {
    None,
    Field(Terrain),
    /// Dungeon floor raster: one height per world unit cell, NaN where there is no floor.
    Grid { size: u32, h: Vec<f32> },
}

impl Ground {
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        match self {
            Ground::None => None,
            Ground::Field(t) => Some(t.height_at(x, z)),
            Ground::Grid { size, h } => {
                if x < 0.0 || z < 0.0 || x >= *size as f32 || z >= *size as f32 {
                    return None;
                }
                let v = h[z as usize * *size as usize + x as usize];
                if v.is_nan() {
                    None
                } else {
                    Some(v)
                }
            }
        }
    }
}

pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// RGBA, row 0 = north (largest z).
    pub rgba: Vec<u8>,
}

pub struct MapAssets {
    pub wld: Option<Arc<Wld>>,
    pub area_names: AreaNames,
    pub terrain: Option<Picture>,
    pub minimap: Option<Picture>,
    pub dungeon: Option<Picture>,
    pub objects: Vec<(f32, f32, ObjKind)>,
    pub ground: Ground,
    pub is_dungeon: bool,
    pub problems: Vec<String>,
    /// The average colour of each terrain texture (what the hillshade is tinted with).
    pub tex_tints: Vec<[u8; 3]>,
}

impl MapAssets {
    /// Nothing known about the client side of this map (no client folder open).
    pub fn empty() -> MapAssets {
        MapAssets {
            wld: None,
            area_names: AreaNames::default(),
            terrain: None,
            minimap: None,
            dungeon: None,
            objects: Vec::new(),
            ground: Ground::None,
            is_dungeon: false,
            problems: Vec::new(),
            tex_tints: Vec::new(),
        }
    }

    /// The same map with other terrain heights under it (a tab with terrain edits): the ground lookups follow the edits, the
    /// pictures stay with the original assets.
    pub fn with_ground(&self, t: Terrain) -> MapAssets {
        MapAssets {
            wld: self.wld.clone(),
            area_names: self.area_names.clone(),
            terrain: None,
            minimap: None,
            dungeon: None,
            objects: self.objects.clone(),
            ground: Ground::Field(t),
            is_dungeon: self.is_dungeon,
            problems: self.problems.clone(),
            tex_tints: self.tex_tints.clone(),
        }
    }

    pub fn ground_at(&self, x: f32, z: f32) -> f32 {
        self.ground.height(x, z).unwrap_or(0.0)
    }
}

/// Loads everything for map `map_id` of side `size` (the svmap's size: the dungeon raster covers it).
pub fn load(rules: &ClientRules, tints: &TintCache, map_id: u32, size: u32) -> MapAssets {
    let mut problems = Vec::new();
    let wld = rules.wld(map_id);
    let area_names = rules.area_names(map_id).unwrap_or_else(|e| {
        problems.push(e.to_string());
        AreaNames::default()
    });
    let mut a = MapAssets {
        wld: wld.clone(),
        area_names,
        terrain: None,
        minimap: None,
        dungeon: None,
        objects: Vec::new(),
        ground: Ground::None,
        is_dungeon: false,
        problems,
        tex_tints: Vec::new(),
    };
    if wld.is_none() {
        a.problems.push(format!("world/{map_id}.wld could not be read"));
    }
    if let Some(w) = &wld {
        a.is_dungeon = matches!(w.kind, shaiya_clientdata::wld::WldKind::Dungeon);
        for (list, kind) in [(&w.building, ObjKind::Building), (&w.shape, ObjKind::Shape), (&w.tree, ObjKind::Tree)] {
            for p in &list.items {
                if p.index >= 0 {
                    a.objects.push((p.pos[0], p.pos[2], kind));
                }
            }
        }
        if let Some(t) = &w.terrain {
            let (pic, tex_tints) = hillshade(rules, tints, w, t, &mut a.problems);
            a.terrain = Some(pic);
            a.tex_tints = tex_tints;
            a.ground = Ground::Field(t.clone());
        } else if !w.dg.is_empty() {
            match rules.source.dg(&w.dg) {
                Ok(dg) => {
                    let flat = dg.flatten();
                    let (pic, h) = dungeon_floor(&flat.collision_points, &flat.collision_indices, size);
                    a.dungeon = Some(pic);
                    a.ground = Ground::Grid { size, h };
                }
                Err(e) => a.problems.push(e.to_string()),
            }
        }
    }
    // not every map has a minimap
    if let Ok(img) = rules.source.texture(&format!("interface/MiniMap/{map_id}.tga")) {
        a.minimap = Some(Picture { width: img.width as usize, height: img.height as usize, rgba: img.rgba });
    }
    a
}

/// Hillshade of the terrain heights (light from the north-west), tinted by each corner's terrain texture. One pixel per
/// corner; pixel (cx, n - 1 - cz) is the corner at world (2 cx, 2 cz).
fn hillshade(rules: &ClientRules, tints: &TintCache, w: &Wld, t: &Terrain, problems: &mut Vec<String>) -> (Picture, Vec<[u8; 3]>) {
    let n = t.n;
    let tex_colours: Vec<[u8; 3]> = w.textures.iter().map(|tx| tints.get(rules, &tx.name, problems)).collect();
    let mut rgba = vec![255u8; n * n * 4];
    shade_rect(&t.heights, &t.tex_index, n, &tex_colours, [0, 0, n as u32 - 1, n as u32 - 1], &mut rgba);
    (Picture { width: n, height: n, rgba }, tex_colours)
}

/// Colours the pixels of the corners in `rect` (`[cx0, cz0, cx1, cz1]`, inclusive) of an `n` x `n` hillshade picture from the
/// corner heights. A terrain edit redraws only the corners it touched (and their neighbours) with this; the first picture is the
/// same function over the whole grid, so a partial redraw gives the pixels a full one would.
pub fn shade_rect(heights: &[i16], tex_index: &[u8], n: usize, tints: &[[u8; 3]], rect: [u32; 4], rgba: &mut [u8]) {
    use shaiya_clientdata::wld::{HEIGHT_OFFSET, HEIGHT_SCALE};
    if n == 0 || heights.len() != n * n || rgba.len() != n * n * 4 {
        return;
    }
    let corner = |cx: usize, cz: usize| heights[cz.min(n - 1) * n + cx.min(n - 1)] as f32 * HEIGHT_SCALE + HEIGHT_OFFSET;
    // light: from the north-west, 45 degrees up
    let l = {
        let v = [-0.5f32, 0.7, 0.5];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    };
    let (x1, z1) = ((rect[2] as usize).min(n - 1), (rect[3] as usize).min(n - 1));
    for cz in (rect[1] as usize).min(n - 1)..=z1 {
        for cx in (rect[0] as usize).min(n - 1)..=x1 {
            let hx = corner(cx + 1, cz) - corner(cx.saturating_sub(1), cz);
            let hz = corner(cx, cz + 1) - corner(cx, cz.saturating_sub(1));
            let span = 2.0 * CORNER_SPACING;
            // surface normal of y = h(x, z): (-dh/dx, 1, -dh/dz)
            let (nx, ny, nz) = (-hx / span, 1.0f32, -hz / span);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let lam = ((nx * l[0] + ny * l[1] + nz * l[2]) / len).max(0.0);
            let shade = 0.30 + 0.85 * lam;
            let ti = tex_index.get(cz * n + cx).copied().unwrap_or(0) as usize;
            let base = tints.get(ti).copied().unwrap_or([128, 128, 128]);
            let o = ((n - 1 - cz) * n + cx) * 4;
            for k in 0..3 {
                rgba[o + k] = (base[k] as f32 * shade).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Top-down raster of the dungeon's collision mesh: cells covered by an upward-facing triangle (normal y >= cos 50 degrees)
/// get the floor height; the picture is shaded by that height. 1 pixel = 1 world unit.
fn dungeon_floor(points: &[[f32; 3]], indices: &[u32], size: u32) -> (Picture, Vec<f32>) {
    let s = size as usize;
    let mut h = vec![f32::NAN; s * s];
    let cos50 = 50f32.to_radians().cos();
    for tri in indices.as_chunks::<3>().0.iter() {
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
        // floors face up: with this winding (b-a)x(c-a) has y > 0 for a floor and y < 0 for a ceiling, so
        // ceilings do not hide the floor under them
        if n[1] / len < cos50 {
            continue;
        }
        let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let max_x = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(s.saturating_sub(1));
        let min_z = a[2].min(b[2]).min(c[2]).floor().max(0.0) as usize;
        let max_z = (a[2].max(b[2]).max(c[2]).ceil().max(0.0) as usize).min(s.saturating_sub(1));
        // barycentric in (x, z)
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-9 {
            continue;
        }
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let pz = z as f32 + 0.5;
                let w1 = ((b[2] - c[2]) * (px - c[0]) + (c[0] - b[0]) * (pz - c[2])) / d;
                let w2 = ((c[2] - a[2]) * (px - c[0]) + (a[0] - c[0]) * (pz - c[2])) / d;
                let w3 = 1.0 - w1 - w2;
                if w1 >= 0.0 && w2 >= 0.0 && w3 >= 0.0 {
                    let y = w1 * a[1] + w2 * b[1] + w3 * c[1];
                    let cell = &mut h[z * s + x];
                    if cell.is_nan() || y > *cell {
                        *cell = y;
                    }
                }
            }
        }
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in &h {
        if !v.is_nan() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    let span = (hi - lo).max(1.0);
    let mut rgba = vec![0u8; s * s * 4];
    for z in 0..s {
        for x in 0..s {
            let v = h[z * s + x];
            if v.is_nan() {
                continue;
            }
            let t = ((v - lo) / span).clamp(0.0, 1.0);
            let o = ((s - 1 - z) * s + x) * 4;
            // dark teal (low) to warm sand (high)
            rgba[o] = (60.0 + 150.0 * t) as u8;
            rgba[o + 1] = (84.0 + 120.0 * t) as u8;
            rgba[o + 2] = (96.0 + 40.0 * t) as u8;
            rgba[o + 3] = 255;
        }
    }
    (Picture { width: s, height: s, rgba }, h)
}

#[cfg(test)]
mod tests {
    use super::*;


    /// A terrain edit redraws a rectangle of the picture (the corners and their neighbours): the result is the picture a full
    /// redraw of the edited heights gives, and pixels outside the rectangle are not touched.
    #[test]
    fn a_partial_hillshade_redraw_equals_a_full_one() {
        let n = 40usize;
        let mut h: Vec<i16> = (0..n * n).map(|i| (9000 + ((i * 131) % 97) as i32 * 5) as i16).collect();
        let tex: Vec<u8> = (0..n * n).map(|i| (i % 3) as u8).collect();
        let tints = [[200u8, 100, 50], [30, 160, 90], [90, 90, 220]];
        let mut pic = vec![255u8; n * n * 4];
        shade_rect(&h, &tex, n, &tints, [0, 0, n as u32 - 1, n as u32 - 1], &mut pic);
        let old = pic.clone();
        // edit a patch of corners
        for cz in 10..16 {
            for cx in 20..27 {
                h[cz * n + cx] += 300;
            }
        }
        let mut full = vec![255u8; n * n * 4];
        shade_rect(&h, &tex, n, &tints, [0, 0, n as u32 - 1, n as u32 - 1], &mut full);
        shade_rect(&h, &tex, n, &tints, [19, 9, 27, 16], &mut pic);
        assert_eq!(pic, full, "the corners of the patch and one more all round give the full picture");
        assert_ne!(pic, old);
        // a rectangle at the map edge stays inside the picture
        shade_rect(&h, &tex, n, &tints, [n as u32 - 3, n as u32 - 3, n as u32 + 5, n as u32 + 5], &mut pic);
        assert_eq!(pic, full);
    }
    #[test]
    fn floor_raster_covers_a_flat_triangle_pair() {
        let pts = vec![[2.0, 5.0, 2.0], [10.0, 5.0, 2.0], [10.0, 5.0, 10.0], [2.0, 5.0, 10.0]];
        let idx = vec![0, 2, 1, 0, 3, 2];
        let (pic, h) = dungeon_floor(&pts, &idx, 16);
        assert_eq!(pic.width, 16);
        assert_eq!(h[5 * 16 + 5], 5.0);
        assert!(h[0].is_nan());
        assert!(h[12 * 16 + 12].is_nan());
        // a wall (vertical triangle) adds nothing
        let wall = vec![[0.0, 0.0, 8.0], [8.0, 0.0, 8.0], [8.0, 9.0, 8.0]];
        let (_, h2) = dungeon_floor(&wall, &[0, 1, 2], 16);
        assert!(h2.iter().all(|v| v.is_nan()));
    }

    #[test]
    fn ground_grid_lookup() {
        let g = Ground::Grid { size: 4, h: vec![f32::NAN, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0] };
        assert_eq!(g.height(1.5, 0.5), Some(1.0));
        assert_eq!(g.height(0.5, 0.5), None);
        assert_eq!(g.height(9.0, 0.5), None);
    }
}
