//! CPU side of the 3D view: the terrain mesh chunks and texture layers of a field, the textured batches of a dungeon, the
//! fog colour. Built on a worker thread from the client data (no GPU access here); `gpu3d` uploads it.

use std::collections::HashMap;

use shaiya_clientdata::smod::placement_basis;
use shaiya_clientdata::texture::Image;
use shaiya_clientdata::wld::{PlacedList, Terrain, Wld, WldKind, CORNER_SPACING, HEIGHT_OFFSET, HEIGHT_SCALE};
use shaiya_clientdata::ClientSource;

use crate::math3d::V3;

/// Cells per terrain chunk side (a chunk is 65 x 65 corners and one draw call).
pub const CHUNK_CELLS: usize = 64;
pub const CHUNK_VERTS: usize = (CHUNK_CELLS + 1) * (CHUNK_CELLS + 1);
/// Terrain textures are resampled to one square size so they fit one texture array; dungeon textures are capped at this.
pub const MAX_TEXTURE: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainVertex {
    pub pos: [f32; 3],
    /// Unit normal as snorm8 (the fourth byte is padding).
    pub normal: [i8; 4],
}

pub struct Chunk {
    pub verts: Vec<TerrainVertex>,
    pub min: V3,
    pub max: V3,
}

/// An RGBA image with its mip chain (`mips[0]` is the full size; every level is half the one before, down to 1 x 1).
pub struct MipImage {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

pub struct TerrainData {
    pub n: usize,
    pub chunks: Vec<Chunk>,
    /// Square edge of every layer.
    pub layer_size: u32,
    /// One mip chain per terrain texture (at least one layer).
    pub layers: Vec<Vec<Vec<u8>>>,
    /// Texture repeats per 64 world units, per layer.
    pub tiling: Vec<f32>,
    /// Texture layer of every corner, `cz * n + cx`.
    pub corner_idx: Vec<u8>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DgVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub luv: [f32; 2],
    pub normal: [f32; 3],
}

pub struct DgBatchData {
    /// Index into [`DungeonData::textures`]; `None` = checker.
    pub texture: Option<usize>,
    /// Index into [`DungeonData::lightmaps`]; `None` = neutral grey.
    pub lightmap: Option<usize>,
    pub verts: Vec<DgVert>,
    pub indices: Vec<u32>,
    pub min: V3,
    pub max: V3,
}

pub struct DungeonData {
    pub batches: Vec<DgBatchData>,
    pub textures: Vec<MipImage>,
    pub lightmaps: Vec<MipImage>,
    pub triangles: usize,
}

// ------------------------------------------------------------------------------------------------------------------ props

/// 32 bytes: position, normal, uv of a `.smod` vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PropVert {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// One textured part of a model (a `.smod` module).
pub struct PropBatch {
    /// Index into [`PropData::textures`]; `None` = checker.
    pub texture: Option<usize>,
    pub verts: Vec<PropVert>,
    pub indices: Vec<u16>,
}

pub struct PropModel {
    pub batches: Vec<PropBatch>,
    /// Bounding sphere in model space (centre of the model's box, half its diagonal, at least 2 like the client).
    pub centre: V3,
    pub radius: f32,
    pub triangles: usize,
}

/// Which `.wld` list an instance came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropClass {
    Building,
    Shape,
    Tree,
}

/// A placed model: the columns of its model-to-world matrix (x, y, z axes and position) plus its world bounding sphere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropInstance {
    pub model: u32,
    pub class: PropClass,
    pub centre: V3,
    pub radius: f32,
    pub axes: [V3; 3],
    pub pos: V3,
}

/// The buildings, shapes and trees of a map: each distinct model once, every placement as an instance (sorted by model).
#[derive(Default)]
pub struct PropData {
    pub models: Vec<PropModel>,
    /// What each model is: its class and its file name in lower case (the lookup key of an edited placement list).
    pub keys: Vec<(PropClass, String)>,
    pub textures: Vec<MipImage>,
    pub instances: Vec<PropInstance>,
    /// Triangles of all instances together (what drawing everything would cost).
    pub triangles: u64,
}

impl PropData {
    pub fn count(&self, class: PropClass) -> usize {
        self.instances.iter().filter(|i| i.class == class).count()
    }
}

/// The textured parts of one `.smod`. Textures go into `textures` (`tex_of` maps a lower-case name to its index there).
fn build_model(source: &ClientSource, sm: &shaiya_clientdata::smod::Smod, textures: &mut Vec<MipImage>, tex_of: &mut HashMap<String, usize>, problems: &mut Vec<String>) -> PropModel {
    let mut batches = Vec::new();
    let mut triangles = 0;
    for module in &sm.modules {
        if module.faces.is_empty() || module.vertices.is_empty() {
            continue;
        }
        let texture = if module.texture.trim().is_empty() {
            None
        } else {
            let k = module.texture.to_ascii_lowercase();
            Some(*tex_of.entry(k).or_insert_with(|| {
                let img = match source.texture_flex("Entity/Texture", &module.texture) {
                    Ok(i) => mip_image_capped(&i),
                    Err(e) => {
                        problems.push(e.to_string());
                        mip_image_capped(&Image::checker(64))
                    }
                };
                textures.push(img);
                textures.len() - 1
            }))
        };
        let verts = module.vertices.iter().map(|v| PropVert { pos: v.pos, normal: v.normal, uv: v.uv }).collect();
        // faces that point past the vertex list are dropped (the GPU would read garbage)
        let n = module.vertices.len();
        let indices: Vec<u16> = module.faces.iter().filter(|f| f.iter().all(|&i| (i as usize) < n)).flatten().copied().collect();
        triangles += indices.len() / 3;
        batches.push(PropBatch { texture, verts, indices });
    }
    let (mn, mx) = (sm.min, sm.max);
    let centre = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5];
    let radius = (((mx[0] - mn[0]).powi(2) + (mx[1] - mn[1]).powi(2) + (mx[2] - mn[2]).powi(2)).sqrt() * 0.5).max(2.0);
    PropModel { batches, centre, radius, triangles }
}

/// One model loaded on its own, for a placement added after the scene was built: the model and the textures its batches index.
pub fn load_prop_model(source: &ClientSource, class: PropClass, name: &str) -> Result<(PropModel, Vec<MipImage>), String> {
    let dir = match class {
        PropClass::Building => "Building",
        PropClass::Shape => "Shape",
        PropClass::Tree => "Tree",
    };
    let sm = source.smod(dir, name).map_err(|e| e.to_string())?;
    let mut textures = Vec::new();
    let mut problems = Vec::new();
    let m = build_model(source, &sm, &mut textures, &mut HashMap::new(), &mut problems);
    if m.batches.is_empty() {
        return Err(format!("{name} has no faces"));
    }
    Ok((m, textures))
}

fn props_data(source: &ClientSource, wld: &Wld, problems: &mut Vec<String>) -> PropData {
    let mut d = PropData::default();
    let mut tex_of: HashMap<String, usize> = HashMap::new();
    let mut model_of: HashMap<(u8, String), Option<u32>> = HashMap::new();
    let lists: [(PropClass, &str, &PlacedList); 3] = [(PropClass::Building, "Building", &wld.building), (PropClass::Shape, "Shape", &wld.shape), (PropClass::Tree, "Tree", &wld.tree)];
    for (class, dir, list) in lists {
        for it in &list.items {
            // index -1 (and anything past the name list) means skip, as in the client
            let Some(name) = usize::try_from(it.index).ok().and_then(|i| list.names.get(i)) else { continue };
            let key = (class as u8, name.to_ascii_lowercase());
            let model = match model_of.get(&key) {
                Some(m) => *m,
                None => {
                    let m = match source.smod(dir, name) {
                        Ok(sm) => {
                            d.models.push(build_model(source, &sm, &mut d.textures, &mut tex_of, problems));
                            d.keys.push((class, key.1.clone()));
                            Some(d.models.len() as u32 - 1)
                        }
                        Err(e) => {
                            problems.push(e.to_string());
                            None
                        }
                    };
                    model_of.insert(key, m);
                    m
                }
            };
            let Some(model) = model else { continue };
            let m = &d.models[model as usize];
            let b = placement_basis(it.dir, it.up);
            let c = m.centre;
            let centre = [
                it.pos[0] + b[0][0] * c[0] + b[1][0] * c[1] + b[2][0] * c[2],
                it.pos[1] + b[0][1] * c[0] + b[1][1] * c[1] + b[2][1] * c[2],
                it.pos[2] + b[0][2] * c[0] + b[1][2] * c[1] + b[2][2] * c[2],
            ];
            if !centre.iter().all(|v| v.is_finite()) {
                continue;
            }
            d.triangles += m.triangles as u64;
            d.instances.push(PropInstance { model, class, centre, radius: m.radius, axes: b, pos: it.pos });
        }
    }
    d.instances.sort_by_key(|i| i.model);
    d
}

pub struct SceneData {
    pub size: u32,
    pub terrain: Option<TerrainData>,
    pub dungeon: Option<DungeonData>,
    pub props: Option<PropData>,
    /// The terrain is a flat stand-in (no client data for this map).
    pub flat: bool,
    pub is_dungeon: bool,
    /// Sky / fog colour, 0..1.
    pub fog: [f32; 3],
    pub bounds_min: V3,
    pub bounds_max: V3,
    pub problems: Vec<String>,
}

// ------------------------------------------------------------------------------------------------------------------ images

/// Averages 2 x 2 blocks (alpha included); odd edges reuse the last row / column.
fn half(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let (x0, y0) = ((x * 2).min(w - 1), (y * 2).min(h - 1));
            let (x1, y1) = ((x * 2 + 1).min(w - 1), (y * 2 + 1).min(h - 1));
            for c in 0..4 {
                let s = |xx: u32, yy: u32| rgba[((yy * w + xx) * 4) as usize + c] as u32;
                out[((y * nw + x) * 4) as usize + c] = ((s(x0, y0) + s(x1, y0) + s(x0, y1) + s(x1, y1) + 2) / 4) as u8;
            }
        }
    }
    (nw, nh, out)
}

/// Full mip chain of an image.
pub fn mip_chain(w: u32, h: u32, rgba: Vec<u8>) -> Vec<Vec<u8>> {
    let mut mips = vec![rgba];
    let (mut cw, mut ch) = (w, h);
    while cw > 1 || ch > 1 {
        let (nw, nh, next) = half(cw, ch, mips.last().unwrap());
        mips.push(next);
        cw = nw;
        ch = nh;
    }
    mips
}

/// Bilinear resample to `nw` x `nh`.
fn resample(w: u32, h: u32, rgba: &[u8], nw: u32, nh: u32) -> Vec<u8> {
    if (w, h) == (nw, nh) {
        return rgba.to_vec();
    }
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        let fy = ((y as f32 + 0.5) * h as f32 / nh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as u32, fy - fy.floor());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let fx = ((x as f32 + 0.5) * w as f32 / nw as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as u32, fx - fx.floor());
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..4 {
                let p = |xx: u32, yy: u32| rgba[((yy * w + xx) * 4) as usize + c] as f32;
                let a = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * tx;
                let b = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * tx;
                out[((y * nw + x) * 4) as usize + c] = (a + (b - a) * ty).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// A texture for the 3D view: capped at [`MAX_TEXTURE`] (halved until it fits), with mips.
pub fn mip_image_capped(img: &Image) -> MipImage {
    let (mut w, mut h, mut rgba) = (img.width, img.height, img.rgba.clone());
    while w > MAX_TEXTURE || h > MAX_TEXTURE {
        let (nw, nh, next) = half(w, h, &rgba);
        w = nw;
        h = nh;
        rgba = next;
    }
    MipImage { width: w, height: h, mips: mip_chain(w, h, rgba) }
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> MipImage {
    let rgba: Vec<u8> = (0..w * h).flat_map(|_| c).collect();
    MipImage { width: w, height: h, mips: mip_chain(w, h, rgba) }
}

// ------------------------------------------------------------------------------------------------------------------ terrain

fn corner_h(h: &[i16], n: usize, cx: usize, cz: usize) -> f32 {
    h[cz.min(n - 1) * n + cx.min(n - 1)] as f32 * HEIGHT_SCALE + HEIGHT_OFFSET
}

fn normal_at(h: &[i16], n: usize, cx: usize, cz: usize) -> [i8; 4] {
    let dx = (corner_h(h, n, cx + 1, cz) - corner_h(h, n, cx.saturating_sub(1), cz)) / (2.0 * CORNER_SPACING);
    let dz = (corner_h(h, n, cx, cz + 1) - corner_h(h, n, cx, cz.saturating_sub(1))) / (2.0 * CORNER_SPACING);
    let l = (dx * dx + 1.0 + dz * dz).sqrt();
    let q = |v: f32| (v / l * 127.0).round().clamp(-127.0, 127.0) as i8;
    [q(-dx), q(1.0), q(-dz), 0]
}

/// Chunks per side of a grid of `n` corners.
pub fn chunks_per_side(n: usize) -> usize {
    (n - 1).div_ceil(CHUNK_CELLS).max(1)
}

/// The vertices of chunk (`ci`, `cj`) of a grid of `n` corners (the last chunks repeat the edge corner when the map is not a
/// multiple of 64 cells, which gives zero-area triangles). A terrain edit rebuilds only the chunks it touches with this.
pub fn chunk_at(h: &[i16], n: usize, ci: usize, cj: usize) -> Chunk {
    let mut verts = Vec::with_capacity(CHUNK_VERTS);
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for lz in 0..=CHUNK_CELLS {
        for lx in 0..=CHUNK_CELLS {
            let cx = (ci * CHUNK_CELLS + lx).min(n - 1);
            let cz = (cj * CHUNK_CELLS + lz).min(n - 1);
            let pos = [cx as f32 * CORNER_SPACING, corner_h(h, n, cx, cz), cz as f32 * CORNER_SPACING];
            for a in 0..3 {
                mn[a] = mn[a].min(pos[a]);
                mx[a] = mx[a].max(pos[a]);
            }
            verts.push(TerrainVertex { pos, normal: normal_at(h, n, cx, cz) });
        }
    }
    Chunk { verts, min: mn, max: mx }
}

/// The terrain mesh in chunks of 64 x 64 cells (65 x 65 vertices each).
pub fn terrain_chunks(t: &Terrain) -> (usize, Vec<Chunk>) {
    let per_side = chunks_per_side(t.n);
    let mut chunks = Vec::with_capacity(per_side * per_side);
    for cj in 0..per_side {
        for ci in 0..per_side {
            chunks.push(chunk_at(&t.heights, t.n, ci, cj));
        }
    }
    (per_side, chunks)
}

/// The 16-bit index list shared by every chunk: two triangles per cell, wound clockwise seen from above (the camera is
/// left-handed with north up, so this is the front face).
pub fn chunk_indices() -> Vec<u16> {
    let row = (CHUNK_CELLS + 1) as u16;
    let mut idx = Vec::with_capacity(CHUNK_CELLS * CHUNK_CELLS * 6);
    for z in 0..CHUNK_CELLS as u16 {
        for x in 0..CHUNK_CELLS as u16 {
            let a = z * row + x;
            let (b, c, d) = (a + 1, a + row, a + row + 1);
            idx.extend_from_slice(&[a, c, d, a, d, b]);
        }
    }
    idx
}

fn terrain_data(source: Option<&ClientSource>, wld: &Wld, t: &Terrain, problems: &mut Vec<String>) -> TerrainData {
    let (_, chunks) = terrain_chunks(t);
    // textures: decode, then bring them to one square size
    let mut imgs: Vec<Option<Image>> = Vec::new();
    for tx in &wld.textures {
        if tx.name.trim().is_empty() {
            imgs.push(None);
            continue;
        }
        match source.map(|s| s.texture_flex("Terrain/Detail", &tx.name)) {
            Some(Ok(i)) => imgs.push(Some(i)),
            Some(Err(e)) => {
                problems.push(e.to_string());
                imgs.push(Some(Image::checker(64)));
            }
            None => imgs.push(None),
        }
    }
    let widest = imgs.iter().flatten().map(|i| i.width.max(i.height)).max().unwrap_or(64);
    let layer_size = widest.next_power_of_two().clamp(64, 512);
    let mut layers: Vec<Vec<Vec<u8>>> = Vec::new();
    for im in &imgs {
        let rgba = match im {
            Some(i) => resample(i.width, i.height, &i.rgba, layer_size, layer_size),
            None => solid(layer_size, layer_size, [118, 112, 98, 255]).mips.swap_remove(0),
        };
        layers.push(mip_chain(layer_size, layer_size, rgba));
    }
    if layers.is_empty() {
        layers.push(solid(layer_size, layer_size, [118, 112, 98, 255]).mips);
    }
    let tiling: Vec<f32> = (0..layers.len()).map(|i| wld.textures.get(i).map_or(4.0, |x| if x.tiling > 0.0 { x.tiling } else { 4.0 })).collect();
    TerrainData { n: t.n, chunks, layer_size, layers, tiling, corner_idx: t.tex_index.clone() }
}

/// A flat stand-in terrain (no client data): one grey texture, a faint darker one on a 16-unit checker.
fn flat_terrain(size: u32) -> TerrainData {
    let n = size as usize / 2 + 1;
    let zero = (200.0 / shaiya_clientdata::wld::HEIGHT_SCALE).round() as i16;
    let t = Terrain { size, n, heights: vec![zero; n * n], tex_index: vec![0; n * n] };
    let (_, chunks) = terrain_chunks(&t);
    let mut rgba = Vec::new();
    for y in 0..64u32 {
        for x in 0..64u32 {
            let on = (x / 32 + y / 32) % 2 == 0;
            let v = if on { 92 } else { 78 };
            rgba.extend_from_slice(&[v, v, v + 4, 255]);
        }
    }
    TerrainData { n, chunks, layer_size: 64, layers: vec![mip_chain(64, 64, rgba)], tiling: vec![2.0], corner_idx: vec![0; n * n] }
}

// ------------------------------------------------------------------------------------------------------------------ dungeon

fn dungeon_data(source: &ClientSource, dg_name: &str, problems: &mut Vec<String>) -> Result<DungeonData, String> {
    let dg = source.dg(dg_name).map_err(|e| e.to_string())?;
    let flat = dg.flatten();
    let stem = dg_name.strip_suffix(".dg").or_else(|| dg_name.strip_suffix(".DG")).unwrap_or(dg_name);

    let mut textures: Vec<MipImage> = Vec::new();
    let mut tex_of: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    let mut lightmaps: Vec<MipImage> = Vec::new();
    let mut lm_of: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    let mut batches = Vec::new();
    let mut triangles = 0;
    for b in &flat.batches {
        let texture = if b.texture < 0 {
            None
        } else {
            Some(*tex_of.entry(b.texture).or_insert_with(|| {
                let name = dg.textures.get(b.texture as usize).cloned().unwrap_or_default();
                let img = match source.texture_flex("Entity/Texture", &name) {
                    Ok(i) => mip_image_capped(&i),
                    Err(e) => {
                        problems.push(e.to_string());
                        mip_image_capped(&Image::checker(64))
                    }
                };
                textures.push(img);
                textures.len() - 1
            }))
        };
        let lightmap = if b.lightmap < 0 {
            None
        } else {
            Some(*lm_of.entry(b.lightmap).or_insert_with(|| {
                let file = format!("{stem}_L{}.dds", b.lightmap);
                let img = match source.texture_flex(&format!("world/dungeon/{stem}"), &file) {
                    Ok(i) => mip_image_capped(&i),
                    Err(e) => {
                        problems.push(e.to_string());
                        solid(4, 4, [128, 128, 128, 255])
                    }
                };
                lightmaps.push(img);
                lightmaps.len() - 1
            }))
        };
        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
        let verts: Vec<DgVert> = b
            .vertices
            .iter()
            .map(|v| {
                for a in 0..3 {
                    mn[a] = mn[a].min(v.pos[a]);
                    mx[a] = mx[a].max(v.pos[a]);
                }
                DgVert { pos: v.pos, uv: v.uv, luv: v.lightmap_uv, normal: v.normal }
            })
            .collect();
        triangles += b.indices.len() / 3;
        batches.push(DgBatchData { texture, lightmap, verts, indices: b.indices.clone(), min: mn, max: mx });
    }
    Ok(DungeonData { batches, textures, lightmaps, triangles })
}

// ------------------------------------------------------------------------------------------------------------------ scene

impl SceneData {
    /// Builds the scene of one map. `source`/`wld` are `None` without client data: a flat plane then stands in for the terrain.
    pub fn build(source: Option<&ClientSource>, wld: Option<&Wld>, size: u32) -> SceneData {
        let mut problems = Vec::new();
        let mut s = SceneData {
            size,
            terrain: None,
            dungeon: None,
            props: None,
            flat: false,
            is_dungeon: false,
            fog: [0.42, 0.50, 0.58],
            bounds_min: [0.0, -5.0, 0.0],
            bounds_max: [size as f32, 60.0, size as f32],
            problems: Vec::new(),
        };
        if let Some(w) = wld {
            s.fog = [(w.colours[2][0] / 255.0).clamp(0.0, 1.0), (w.colours[2][1] / 255.0).clamp(0.0, 1.0), (w.colours[2][2] / 255.0).clamp(0.0, 1.0)];
            s.is_dungeon = w.kind == WldKind::Dungeon;
            match (&w.terrain, source) {
                (Some(t), _) => {
                    let (lo, hi) = t.height_range();
                    s.bounds_min = [0.0, lo, 0.0];
                    s.bounds_max = [t.size as f32, hi.max(lo + 10.0), t.size as f32];
                    s.terrain = Some(terrain_data(source, w, t, &mut problems));
                }
                (None, Some(src)) if w.kind == WldKind::Dungeon && !w.dg.is_empty() => match dungeon_data(src, &w.dg, &mut problems) {
                    Ok(d) => {
                        let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
                        for b in &d.batches {
                            for a in 0..3 {
                                mn[a] = mn[a].min(b.min[a]);
                                mx[a] = mx[a].max(b.max[a]);
                            }
                        }
                        if mn[0] <= mx[0] {
                            s.bounds_min = mn;
                            s.bounds_max = mx;
                        }
                        s.dungeon = Some(d);
                    }
                    Err(e) => problems.push(e),
                },
                _ => {}
            }
        }
        if let (Some(w), Some(src)) = (wld, source) {
            let p = props_data(src, w, &mut problems);
            if !p.instances.is_empty() {
                s.props = Some(p);
            }
        }
        if s.terrain.is_none() && s.dungeon.is_none() {
            s.flat = true;
            s.terrain = Some(flat_terrain(size));
        }
        s.problems = problems;
        s
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::math3d;

    #[test]
    fn mip_chain_halves_to_one_pixel() {
        let rgba: Vec<u8> = (0..8 * 4 * 4).map(|i| (i % 251) as u8).collect();
        let m = mip_chain(8, 4, rgba);
        let sizes: Vec<usize> = m.iter().map(|l| l.len() / 4).collect();
        assert_eq!(sizes, vec![32, 8, 2, 1]);
    }

    #[test]
    fn capped_images_fit_the_limit() {
        let big = Image { width: 1024, height: 512, rgba: vec![200; 1024 * 512 * 4] };
        let m = mip_image_capped(&big);
        assert_eq!((m.width, m.height), (256, 128));
        assert_eq!(m.mips.len(), 9);
        assert!(m.mips.iter().all(|l| l.iter().all(|v| *v == 200)));
    }

    #[test]
    fn resample_keeps_flat_colours() {
        let r = resample(2, 2, &[10, 20, 30, 255].repeat(4), 8, 8);
        assert_eq!(r.len(), 8 * 8 * 4);
        assert!(r.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn chunks_cover_a_map_and_clamp_partial_ones() {
        let n = 3 * CHUNK_CELLS + 1; // 3 chunks wide, size 384
        let t = Terrain { size: (n as u32 - 1) * 2, n, heights: vec![10000; n * n], tex_index: vec![0; n * n] };
        let (per, chunks) = terrain_chunks(&t);
        assert_eq!(per, 3);
        assert_eq!(chunks.len(), 9);
        assert!(chunks.iter().all(|c| c.verts.len() == CHUNK_VERTS));
        assert_eq!(chunks[8].max[0], (n - 1) as f32 * 2.0);
        // a map that is not a multiple of 64 cells: the last chunk repeats the edge corner
        let n2 = CHUNK_CELLS + 11;
        let t2 = Terrain { size: (n2 as u32 - 1) * 2, n: n2, heights: vec![10000; n2 * n2], tex_index: vec![0; n2 * n2] };
        let (per2, c2) = terrain_chunks(&t2);
        assert_eq!(per2, 2);
        assert_eq!(c2[1].max[0], (n2 - 1) as f32 * 2.0);
    }

    #[test]
    fn chunk_indices_wind_clockwise_from_above() {
        let idx = chunk_indices();
        assert_eq!(idx.len(), CHUNK_CELLS * CHUNK_CELLS * 6);
        let row = (CHUNK_CELLS + 1) as u16;
        // first cell: a (0,0), c (0,1) north, d (1,1) north-east: up then right = clockwise on a north-up map
        assert_eq!(&idx[..3], &[0, row, row + 1]);
        assert!(*idx.iter().max().unwrap() < CHUNK_VERTS as u16);
    }

    /// A terrain edit rebuilds only some chunks: each chunk of the partial update equals the one a full rebuild gives, and the
    /// chunks it leaves are the same as before.
    #[test]
    fn chunks_rebuilt_after_an_edit_equal_a_full_rebuild() {
        let n = 131usize; // 130 cells: 3 chunks per side, the last one short
        let mut h: Vec<i16> = (0..n * n).map(|i| (9000 + ((i * 7) % 41) as i32) as i16).collect();
        let t0 = Terrain { size: 130 * 2, n, heights: h.clone(), tex_index: vec![0; n * n] };
        let (per, before) = terrain_chunks(&t0);
        assert_eq!(per, chunks_per_side(n));
        // an edit at corner 64 (shared by chunks 0 and 1) in x and well inside chunk 2 in z
        for cx in 60..68 {
            h[100 * n + cx] += 200;
        }
        let t1 = Terrain { size: 130 * 2, n, heights: h.clone(), tex_index: vec![0; n * n] };
        let (_, after) = terrain_chunks(&t1);
        let changed: Vec<usize> = (0..before.len()).filter(|i| before[*i].verts != after[*i].verts).collect();
        assert!(!changed.is_empty() && changed.len() < before.len(), "{changed:?}");
        for i in &changed {
            let (ci, cj) = (i % per, i / per);
            assert_eq!(chunk_at(&h, n, ci, cj).verts, after[*i].verts);
            assert_eq!(chunk_at(&h, n, ci, cj).max, after[*i].max);
        }
        // the chunks the gpu update picks for the edited rectangle (one corner wider) include every chunk that changed
        let (c0, c1) = ((60usize.saturating_sub(2)) / CHUNK_CELLS, ((67 + 1) / CHUNK_CELLS).min(per - 1));
        let (r0, r1) = ((100usize.saturating_sub(2)) / CHUNK_CELLS, ((100 + 1) / CHUNK_CELLS).min(per - 1));
        for i in &changed {
            let (ci, cj) = (i % per, i / per);
            assert!(ci >= c0 && ci <= c1 && cj >= r0 && cj <= r1, "chunk {ci},{cj} changed outside {c0}..{c1} x {r0}..{r1}");
        }
    }

    pub fn client() -> Option<ClientSource> {
        let Some(var) = std::env::var_os("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return None;
        };
        let dir = std::path::PathBuf::from(var);
        let source = dir.is_dir().then(|| ClientSource::open(&dir).ok()).flatten();
        if source.is_none() {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
        }
        source
    }

    /// Real maps (needs `SHAIYA_CLIENT`): the scenes of the acceptance maps build with every texture found.
    #[test]
    fn real_maps_build_scenes_without_missing_textures() {
        let Some(c) = client() else { return };
        for (id, dungeon) in [(1u32, false), (2, false), (18, false), (35, false), (65, true)] {
            let w = c.wld(id).unwrap();
            let size = w.field_size().unwrap_or(440);
            let t0 = std::time::Instant::now();
            let s = SceneData::build(Some(&c), Some(&w), size);
            let ms = t0.elapsed().as_millis();
            assert!(!s.flat, "map {id} has real data");
            assert_eq!(s.is_dungeon, dungeon);
            assert!(s.problems.is_empty(), "map {id}: {:?}", s.problems);
            match (&s.terrain, &s.dungeon) {
                (Some(t), None) => eprintln!("map {id}: terrain {} chunks, {} texture layers of {} px, {} corners, built in {ms} ms", t.chunks.len(), t.layers.len(), t.layer_size, t.n * t.n),
                (None, Some(d)) => eprintln!("map {id}: dungeon {} batches, {} textures, {} lightmaps, {} triangles, built in {ms} ms", d.batches.len(), d.textures.len(), d.lightmaps.len(), d.triangles),
                _ => panic!("map {id}: terrain xor dungeon"),
            }
            if let Some(p) = &s.props {
                let tex_mb = p.textures.iter().map(|t| t.mips.iter().map(|m| m.len()).sum::<usize>()).sum::<usize>() as f32 / 1048576.0;
                eprintln!("map {id}: props {} instances of {} models, {} textures ({tex_mb:.0} MB with mips), {} triangles if all drawn", p.instances.len(), p.models.len(), p.textures.len(), p.triangles);
            }
            assert!(s.bounds_min[0] <= s.bounds_max[0] && s.bounds_min[2] < s.bounds_max[2]);
            // fog colour comes from the .wld
            assert!(s.fog.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }

    /// Map 35 (the town): every placement with a name gets a model, every model its textures, and the placements stand where the
    /// `.wld` puts them, rotated about y (up stays up in a town of upright buildings).
    #[test]
    fn town_props_are_built_from_the_wld_placements() {
        let Some(c) = client() else { return };
        let w = c.wld(35).unwrap();
        let t0 = std::time::Instant::now();
        let s = SceneData::build(Some(&c), Some(&w), 1024);
        let ms = t0.elapsed().as_millis();
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let p = s.props.as_ref().expect("map 35 has buildings");
        let wanted = [&w.building, &w.shape, &w.tree].iter().map(|l| l.items.iter().filter(|i| i.index >= 0).count()).sum::<usize>();
        assert_eq!(p.instances.len(), wanted);
        assert_eq!((p.count(PropClass::Building), p.count(PropClass::Shape), p.count(PropClass::Tree)), (28, 184, 36));
        assert!(p.instances.windows(2).all(|x| x[0].model <= x[1].model), "sorted by model");
        assert!(p.models.iter().all(|m| !m.batches.is_empty() && m.radius >= 2.0));
        assert!(p.textures.len() > 5);
        // the instance of the first building: position and a unit, perpendicular frame
        let first = w.building.items[0];
        let inst = p.instances.iter().find(|i| i.class == PropClass::Building && i.pos == first.pos).expect("first building");
        for a in inst.axes {
            assert!((math3d::len(a) - 1.0).abs() < 1e-4);
        }
        assert!(math3d::dot(inst.axes[0], inst.axes[1]).abs() < 1e-4 && math3d::dot(inst.axes[1], inst.axes[2]).abs() < 1e-4);
        // the bounding spheres stay near the map
        assert!(p.instances.iter().all(|i| i.centre[0] > -50.0 && i.centre[0] < 1100.0 && i.centre[2] > -50.0 && i.centre[2] < 1100.0));
        let tex_mb = p.textures.iter().map(|t| t.mips.iter().map(|m| m.len()).sum::<usize>()).sum::<usize>() as f32 / 1048576.0;
        eprintln!("map 35 props: {} instances of {} models, {} textures ({tex_mb:.0} MB with mips), {} triangles if all drawn, built in {ms} ms (debug)", p.instances.len(), p.models.len(), p.textures.len(), p.triangles);
    }

    /// A model point goes to world space through the instance axes like `pos + x*ax + y*ay + z*az`: a placement turned half a
    /// circle puts a point that was east of the origin to the west of it.
    #[test]
    fn placement_axes_turn_models_like_the_client() {
        let half_turn = shaiya_clientdata::smod::placement_basis([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        let to_world = |b: [V3; 3], p: V3| -> V3 { [b[0][0] * p[0] + b[1][0] * p[1] + b[2][0] * p[2], b[0][1] * p[0] + b[1][1] * p[1] + b[2][1] * p[2], b[0][2] * p[0] + b[1][2] * p[1] + b[2][2] * p[2]] };
        let w = to_world(half_turn, [10.0, 3.0, 2.0]);
        assert!((w[0] + 10.0).abs() < 1e-5 && (w[1] - 3.0).abs() < 1e-5 && (w[2] + 2.0).abs() < 1e-5, "{w:?}");
        // a quarter turn to the east (dir = +x): the model forward (+z) points east and its +x points south
        let east = shaiya_clientdata::smod::placement_basis([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let f = to_world(east, [0.0, 0.0, 1.0]);
        let r = to_world(east, [1.0, 0.0, 0.0]);
        assert!((f[0] - 1.0).abs() < 1e-5 && f[2].abs() < 1e-5, "{f:?}");
        assert!((r[2] + 1.0).abs() < 1e-5 && r[0].abs() < 1e-5, "{r:?}");
    }

    #[test]
    fn flat_scene_without_client_data() {
        let s = SceneData::build(None, None, 128);
        assert!(s.flat && s.terrain.is_some() && s.dungeon.is_none());
        let t = s.terrain.unwrap();
        assert_eq!(t.n, 65);
        assert_eq!(t.chunks[0].verts[0].pos[1], 0.0);
        assert!(s.props.is_none());
    }

    /// A model added after the scene was built is loaded on its own; a missing file is an error (the view then keeps the named box).
    #[test]
    fn a_missing_model_does_not_load() {
        let root = crate::testutil::TempDir::new("fix2-model");
        let (client, _) = crate::testutil::paired_folders(&root);
        let src = ClientSource::open(&client).unwrap();
        let e = load_prop_model(&src, PropClass::Building, "nothing.smod").err().unwrap();
        assert!(!e.is_empty());
    }
}
