//! The 3D view's wgpu side. egui-wgpu's own render pass has no depth attachment, so the scene is drawn into a texture of its
//! own (4x MSAA colour + depth, resolved into an `Rgba8Unorm` texture) and egui shows that texture as an image.
//!
//! Colours are handled in gamma space like the original client and like egui (no sRGB conversions anywhere).

use std::collections::HashMap;

use egui_wgpu::RenderState;
use wgpu::util::DeviceExt;

use crate::math3d::{self, Mat4, V3};
use crate::scene3d::{self, DgVert, MipImage, PropClass, PropInstance, PropVert, SceneData, TerrainVertex, CHUNK_VERTS};

pub const MSAA: u32 = 4;
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_LAYERS: usize = 256;

const SHADER: &str = r#"
struct U {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    fog: vec4<f32>,
    params: vec4<f32>,   // x fog start, y fog end, z blocked tint on, w blocked tint strength
    map: vec4<f32>,      // x map size, y terrain corners per side, z terrain layer count
    light: vec4<f32>,    // xyz: unit vector towards the light
    heat: vec4<f32>,     // x heat on, y opacity, z tile size, w tiles per side
    tiling: array<vec4<f32>, 64>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var blocked: texture_2d<f32>;
@group(0) @binding(2) var heat: texture_2d<f32>;

fn fog_mix(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let d = distance(world, u.eye.xyz);
    let f = clamp((d - u.params.x) / max(u.params.y - u.params.x, 1.0), 0.0, 1.0);
    return mix(c, u.fog.rgb, f * 0.85);
}

fn blocked_tint(c: vec3<f32>, world: vec3<f32>, up: f32) -> vec3<f32> {
    if (u.params.z > 0.5 && up > 0.5) {
        let dims = vec2<i32>(textureDimensions(blocked));
        let cell = vec2<i32>(floor(world.xz));
        if (cell.x >= 0 && cell.y >= 0 && cell.x < dims.x && cell.y < dims.y) {
            if (textureLoad(blocked, cell, 0).r > 0.5) {
                return mix(c, vec3<f32>(1.0, 0.12, 0.10), u.params.w);
            }
        }
    }
    return c;
}

fn heat_tint(c: vec3<f32>, world: vec3<f32>, up: f32) -> vec3<f32> {
    if (u.heat.x > 0.5 && up > 0.2) {
        let n = i32(u.heat.w);
        let cell = vec2<i32>(floor(world.xz / u.heat.z));
        if (cell.x >= 0 && cell.y >= 0 && cell.x < n && cell.y < n) {
            let t = textureLoad(heat, vec2<i32>(cell.x, n - 1 - cell.y), 0);
            return mix(c, t.rgb, t.a * u.heat.y);
        }
    }
    return c;
}

// ---- terrain: 65 x 65 vertex chunks; the texture of each corner is blended bilinearly in the fragment shader

struct TIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec4<f32>,
};
struct TOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) n: vec3<f32>,
};
@group(1) @binding(0) var layers: texture_2d_array<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var corner_idx: texture_2d<u32>;

@vertex
fn vs_terrain(i: TIn) -> TOut {
    var o: TOut;
    o.clip = u.view_proj * vec4<f32>(i.pos, 1.0);
    o.world = i.pos;
    o.n = i.normal.xyz;
    return o;
}

fn layer_colour(k: u32, p: vec2<f32>) -> vec3<f32> {
    let kk = min(k, u32(u.map.z) - 1u);
    let t = u.tiling[kk >> 2u][kk & 3u];
    return textureSample(layers, samp, p * (t / 64.0), i32(kk)).rgb;
}

@fragment
fn fs_terrain(in: TOut) -> @location(0) vec4<f32> {
    let n = i32(u.map.y);
    let g = in.world.xz * 0.5;
    let f = floor(g);
    let t = g - f;
    let hi = vec2<i32>(n - 1, n - 1);
    let c0 = clamp(vec2<i32>(f), vec2<i32>(0, 0), hi);
    let c1 = min(c0 + vec2<i32>(1, 1), hi);
    let i00 = textureLoad(corner_idx, vec2<i32>(c0.x, c0.y), 0).r;
    let i10 = textureLoad(corner_idx, vec2<i32>(c1.x, c0.y), 0).r;
    let i01 = textureLoad(corner_idx, vec2<i32>(c0.x, c1.y), 0).r;
    let i11 = textureLoad(corner_idx, vec2<i32>(c1.x, c1.y), 0).r;
    let p = in.world.xz;
    var col = layer_colour(i00, p) * ((1.0 - t.x) * (1.0 - t.y));
    col = col + layer_colour(i10, p) * (t.x * (1.0 - t.y));
    col = col + layer_colour(i01, p) * ((1.0 - t.x) * t.y);
    col = col + layer_colour(i11, p) * (t.x * t.y);
    let nn = normalize(in.n);
    let lit = 0.50 + 0.60 * clamp(dot(nn, u.light.xyz), 0.0, 1.0);
    var c = col * lit;
    c = heat_tint(c, in.world, nn.y);
    c = blocked_tint(c, in.world, nn.y);
    return vec4<f32>(fog_mix(c, in.world), 1.0);
}

// ---- dungeon: texture x lightmap per batch

struct DIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) luv: vec2<f32>,
    @location(3) normal: vec3<f32>,
};
struct DOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) luv: vec2<f32>,
    @location(3) n: vec3<f32>,
};
@group(1) @binding(0) var dtex: texture_2d<f32>;
@group(1) @binding(1) var dlm: texture_2d<f32>;
@group(1) @binding(2) var dsamp: sampler;

@vertex
fn vs_dungeon(i: DIn) -> DOut {
    var o: DOut;
    o.clip = u.view_proj * vec4<f32>(i.pos, 1.0);
    o.world = i.pos;
    o.uv = i.uv;
    o.luv = i.luv;
    o.n = i.normal;
    return o;
}

@fragment
fn fs_dungeon(in: DOut) -> @location(0) vec4<f32> {
    let a = textureSample(dtex, dsamp, in.uv);
    let lm = textureSample(dlm, dsamp, in.luv).rgb;
    if (a.a < 0.5) {
        discard;
    }
    var c = clamp(a.rgb * lm * 2.0, vec3<f32>(0.0), vec3<f32>(1.0));
    c = blocked_tint(c, in.world, normalize(in.n).y);
    return vec4<f32>(fog_mix(c, in.world), 1.0);
}

// ---- props: `.smod` models drawn instanced; every instance brings its model-to-world matrix (x, y, z axes and position)

struct PIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) c0: vec4<f32>,
    @location(4) c1: vec4<f32>,
    @location(5) c2: vec4<f32>,
    @location(6) c3: vec4<f32>,
};
struct POut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
@group(1) @binding(0) var ptex: texture_2d<f32>;
@group(1) @binding(1) var psamp: sampler;

@vertex
fn vs_prop(i: PIn) -> POut {
    var o: POut;
    let w = i.c0.xyz * i.pos.x + i.c1.xyz * i.pos.y + i.c2.xyz * i.pos.z + i.c3.xyz;
    o.clip = u.view_proj * vec4<f32>(w, 1.0);
    o.world = w;
    o.n = i.c0.xyz * i.normal.x + i.c1.xyz * i.normal.y + i.c2.xyz * i.normal.z;
    o.uv = i.uv;
    return o;
}

@fragment
fn fs_prop(in: POut) -> @location(0) vec4<f32> {
    let a = textureSample(ptex, psamp, in.uv);
    if (a.a < 0.5) {
        discard;
    }
    // two-sided: the normal is turned towards the viewer, so the winding of the model does not matter
    var n = normalize(in.n);
    if (dot(n, u.eye.xyz - in.world) < 0.0) {
        n = -n;
    }
    let lit = 0.50 + 0.60 * clamp(dot(n, u.light.xyz), 0.0, 1.0);
    return vec4<f32>(fog_mix(clamp(a.rgb * lit, vec3<f32>(0.0), vec3<f32>(1.0)), in.world), 1.0);
}

// ---- overlays: coloured triangles and lines

struct OIn {
    @location(0) pos: vec3<f32>,
    @location(1) col: vec4<f32>,
};
struct OOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) col: vec4<f32>,
};

@vertex
fn vs_overlay(i: OIn) -> OOut {
    var o: OOut;
    o.clip = u.view_proj * vec4<f32>(i.pos, 1.0);
    o.col = i.col;
    return o;
}

@fragment
fn fs_overlay(in: OOut) -> @location(0) vec4<f32> {
    return in.col;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    fog: [f32; 4],
    params: [f32; 4],
    map: [f32; 4],
    light: [f32; 4],
    heat: [f32; 4],
    tiling: [[f32; 4]; 64],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OVert {
    pub pos: [f32; 3],
    pub col: [u8; 4],
}

/// The svmap objects of the frame as coloured primitives.
#[derive(Default)]
pub struct OverlayGeo {
    pub tris_solid: Vec<OVert>,
    pub tris_blend: Vec<OVert>,
    pub lines: Vec<OVert>,
    /// Lines drawn over everything (selection, hover).
    pub lines_top: Vec<OVert>,
}

/// What changes per frame.
pub struct FrameParams<'a> {
    pub view_proj: Mat4,
    pub eye: V3,
    pub size_px: [u32; 2],
    pub fog_start: f32,
    pub fog_end: f32,
    pub blocked: Option<&'a shaiya_svmap::BitGrid>,
    pub blocked_rev: u64,
    pub backface_cull: bool,
    pub draw_ground: bool,
    pub draw_dungeon: bool,
    /// Buildings and shapes / trees (`.smod` placements), and how far from the eye they are drawn.
    pub draw_buildings: bool,
    pub draw_trees: bool,
    pub prop_distance: f32,
    /// The heatmap on the terrain (None = off).
    pub heat: Option<HeatParams<'a>>,
}

/// The heatmap draped on the terrain: the tile colours (`n * n` RGBA pixels, the top row is the highest z) of one map.
#[derive(Clone, Copy)]
pub struct HeatParams<'a> {
    pub rgba: &'a [u8],
    /// Tiles per side (at most [`HEAT_TEX`]).
    pub n: u32,
    /// Side of a tile in world units.
    pub tile: f32,
    pub opacity: f32,
    /// Changes whenever `rgba` does.
    pub rev: u64,
}

/// Side of the heat texture of a scene (a 2048 map at tile 16 needs 128).
pub const HEAT_TEX: u32 = 256;

/// The colour the scene is cleared to: the map's fog colour, 20 percent darker, so the horizon reads against the fogged ground.
pub fn clear_colour(fog: [f32; 3]) -> [f32; 3] {
    [fog[0] * 0.8, fog[1] * 0.8, fog[2] * 0.8]
}

/// Instances smaller than this on the screen (radius in pixels) are not drawn.
pub const PROP_MIN_PIXELS: f32 = 1.5;

/// The per-instance vertex data of a prop: the columns of the model-to-world matrix.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRaw {
    pub cols: [[f32; 4]; 4],
}

impl InstanceRaw {
    pub fn of(i: &PropInstance) -> InstanceRaw {
        InstanceRaw { cols: [[i.axes[0][0], i.axes[0][1], i.axes[0][2], 0.0], [i.axes[1][0], i.axes[1][1], i.axes[1][2], 0.0], [i.axes[2][0], i.axes[2][1], i.axes[2][2], 0.0], [i.pos[0], i.pos[1], i.pos[2], 1.0]] }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub draw_calls: u32,
    pub triangles: u64,
    pub chunks_drawn: u32,
    pub chunks_total: u32,
    /// Placed buildings, shapes and trees drawn / in the map (drawn = inside the cut-off and the view, big enough on screen).
    pub props_drawn: u32,
    pub props_total: u32,
}

struct GrowBuf {
    buf: Option<wgpu::Buffer>,
    cap: u64,
}

impl GrowBuf {
    fn new() -> GrowBuf {
        GrowBuf { buf: None, cap: 0 }
    }

    fn write<T: bytemuck::Pod>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[T]) -> u32 {
        if data.is_empty() {
            return 0;
        }
        let bytes: &[u8] = bytemuck::cast_slice(data);
        if self.buf.is_none() || self.cap < bytes.len() as u64 {
            self.cap = (bytes.len() as u64).next_power_of_two().max(4096);
            self.buf = Some(device.create_buffer(&wgpu::BufferDescriptor { label: Some("dynamic vertices"), size: self.cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
        queue.write_buffer(self.buf.as_ref().unwrap(), 0, bytes);
        data.len() as u32
    }
}

struct Target {
    size: [u32; 2],
    ms_view: wgpu::TextureView,
    /// Kept for the readback of the tests (the view keeps the texture alive on its own).
    #[cfg_attr(not(test), allow(dead_code))]
    resolve_tex: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// The egui texture id of the resolved image (kept across resizes) and whether it points at an outdated texture.
    egui_id: Option<egui::TextureId>,
    egui_stale: bool,
}

pub struct Gpu3d {
    uni: wgpu::Buffer,
    bgl_scene: wgpu::BindGroupLayout,
    bgl_terrain: wgpu::BindGroupLayout,
    bgl_dungeon: wgpu::BindGroupLayout,
    bgl_prop: wgpu::BindGroupLayout,
    terrain_pipe: wgpu::RenderPipeline,
    prop_pipe: wgpu::RenderPipeline,
    dungeon_pipe: wgpu::RenderPipeline,
    dungeon_pipe_nocull: wgpu::RenderPipeline,
    tri_blend: wgpu::RenderPipeline,
    tri_solid: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    lines_top: wgpu::RenderPipeline,
    chunk_ib: wgpu::Buffer,
    chunk_index_count: u32,
    samp_repeat: wgpu::Sampler,
    ov_solid: GrowBuf,
    ov_blend: GrowBuf,
    ov_lines: GrowBuf,
    ov_top: GrowBuf,
    prop_inst: GrowBuf,
    target: Option<Target>,
}

struct TerrainGpu {
    vb: wgpu::Buffer,
    chunk_bounds: Vec<(V3, V3)>,
    bind: wgpu::BindGroup,
}

struct DungeonBatchGpu {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    index_count: u32,
    min: V3,
    max: V3,
    bind: wgpu::BindGroup,
}

struct PropBatchGpu {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    index_count: u32,
    bind: wgpu::BindGroup,
}

struct PropModelGpu {
    batches: Vec<PropBatchGpu>,
}

/// The uploaded models of a map and the CPU list of its placements (sorted by model, `ranges` index into it).
struct PropsGpu {
    models: Vec<PropModelGpu>,
    instances: Vec<PropInstance>,
    ranges: Vec<(usize, std::ops::Range<usize>)>,
    /// What each model is (class and lower-case file name) and its bounding sphere in model space: an edited placement list is
    /// turned into instances of the models already uploaded.
    keys: Vec<(PropClass, String)>,
    sphere: Vec<(V3, f32)>,
}

/// One map's uploaded scene.
pub struct SceneGpu {
    pub size: u32,
    pub flat: bool,
    pub is_dungeon: bool,
    pub fog: [f32; 3],
    pub bounds_min: V3,
    pub bounds_max: V3,
    pub problems: Vec<String>,
    terrain: Option<TerrainGpu>,
    terrain_n: u32,
    terrain_layers: u32,
    tiling: Vec<f32>,
    dungeon: Vec<DungeonBatchGpu>,
    pub dungeon_triangles: usize,
    props: Option<PropsGpu>,
    /// Buildings, shapes and trees in the map (for the HUD and the layers menu).
    pub prop_counts: [usize; 3],
    /// The placement state the instances were last built from ([`SceneGpu::sync_placements`]); `None` = as the scene was built.
    props_rev: Option<(u64, u64)>,
    /// The terrain state the vertices were last built from ([`SceneGpu::update_terrain`]): (part serial, terrain revision); (0, 0) = as the scene was built.
    terrain_seen: (u64, u64),
    /// Placements of the edited lists whose model is not in this scene (a model added from the browser): the view draws their boxes.
    pub props_missing: Vec<(crate::place::PlaceList, usize)>,
    /// Models (class, lower-case file name) that could not be loaded live; their placements stay boxes with the name.
    pub props_failed: std::collections::HashSet<(PropClass, String)>,
    blocked_tex: wgpu::Texture,
    blocked_rev: Option<u64>,
    heat_tex: wgpu::Texture,
    heat_rev: Option<u64>,
    bind_scene: wgpu::BindGroup,
}

impl SceneGpu {
    /// Whether the instances already follow the placement state `version` (part serial, placement revision).
    pub fn placements_current(&self, version: (u64, u64)) -> bool {
        self.props_rev == Some(version)
    }

    /// The (part serial, terrain revision) the vertices follow; (0, 0) until a tab's terrain was first synced.
    pub fn terrain_seen(&self) -> (u64, u64) {
        self.terrain_seen
    }

    /// Notes that the vertices already match this state (nothing to write).
    pub fn set_terrain_seen(&mut self, seen: (u64, u64)) {
        self.terrain_seen = seen;
    }

    /// Rebuilds the vertices of the chunks the corner rectangle `rect` (`[cx0, cz0, cx1, cz1]`, `None` = all) touches, from the
    /// tab's corner heights, and writes only those into the vertex buffer. The normals of the border corners read their
    /// neighbours, so the rectangle is taken one corner wider. Returns the number of chunks written.
    pub fn update_terrain(&mut self, queue: &wgpu::Queue, heights: &[i16], n: usize, rect: Option<[u32; 4]>, seen: (u64, u64)) -> usize {
        self.terrain_seen = seen;
        let Some(t) = self.terrain.as_mut() else { return 0 };
        if n != self.terrain_n as usize || heights.len() != n * n {
            return 0;
        }
        let per = scene3d::chunks_per_side(n);
        let (c0, c1, r0, r1) = match rect {
            None => (0, per - 1, 0, per - 1),
            Some(r) => {
                let lo = |c: u32| (c as usize).saturating_sub(1).saturating_sub(1) / scene3d::CHUNK_CELLS;
                let hi = |c: u32| ((c as usize + 1) / scene3d::CHUNK_CELLS).min(per - 1);
                (lo(r[0]).min(per - 1), hi(r[2]), lo(r[1]).min(per - 1), hi(r[3]))
            }
        };
        let stride = (CHUNK_VERTS * std::mem::size_of::<TerrainVertex>()) as u64;
        let mut written = 0;
        for cj in r0..=r1 {
            for ci in c0..=c1 {
                let ch = scene3d::chunk_at(heights, n, ci, cj);
                let idx = cj * per + ci;
                queue.write_buffer(&t.vb, idx as u64 * stride, bytemuck::cast_slice(&ch.verts));
                if let Some(b) = t.chunk_bounds.get_mut(idx) {
                    *b = (ch.min, ch.max);
                }
                written += 1;
            }
        }
        written
    }

    /// The first placement model that is not in the scene yet and has not failed: `(class, file name)`.
    pub fn next_missing_model(&self, lists: [&shaiya_clientdata::wld_raw::RawPlaced; 3]) -> Option<(PropClass, String)> {
        use crate::place::{model_name, PlaceList};
        for (l, i) in &self.props_missing {
            let (class, k) = match l {
                PlaceList::Building => (PropClass::Building, 0),
                PlaceList::Shape => (PropClass::Shape, 1),
                PlaceList::Tree => (PropClass::Tree, 2),
                PlaceList::Dungeon => continue,
            };
            let Some(name) = lists[k].items.get(*i).and_then(|p| model_name(lists[k], p)) else { continue };
            let name = name.to_ascii_lowercase();
            if !self.props_failed.contains(&(class, name.clone())) {
                return Some((class, name));
            }
        }
        None
    }

    /// Makes the next [`SceneGpu::placements_current`] check fail, so the instances are rebuilt (after a model was added).
    pub fn invalidate_placements(&mut self) {
        if self.props_rev.is_some() {
            self.props_rev = Some((u64::MAX, u64::MAX));
        }
    }

    /// Whether the instances were ever rebuilt from a tab's lists (then they keep following them).
    pub fn placements_synced(&self) -> bool {
        self.props_rev.is_some()
    }

    /// Makes the drawn buildings, shapes and trees follow the edited lists of the tab's client half: every placement whose model is
    /// in this scene becomes an instance, the others go to `props_missing`. Nothing is read or uploaded (the models are the ones
    /// the scene was built with), so this is a pass over the lists.
    pub fn sync_placements(&mut self, lists: [&shaiya_clientdata::wld_raw::RawPlaced; 3], version: (u64, u64)) {
        use crate::place::{model_name, PlaceList};
        let classes = [(PropClass::Building, PlaceList::Building), (PropClass::Shape, PlaceList::Shape), (PropClass::Tree, PlaceList::Tree)];
        self.props_rev = Some(version);
        self.props_missing.clear();
        let Some(pg) = self.props.as_mut() else {
            for (k, (_, pl)) in classes.iter().enumerate() {
                for (i, p) in lists[k].items.iter().enumerate() {
                    if model_name(lists[k], p).is_some() {
                        self.props_missing.push((*pl, i));
                    }
                }
            }
            self.prop_counts = [0; 3];
            return;
        };
        let index: std::collections::HashMap<(PropClass, &str), usize> = pg.keys.iter().enumerate().map(|(i, (c, n))| ((*c, n.as_str()), i)).collect();
        let mut inst: Vec<PropInstance> = Vec::new();
        for (k, (class, pl)) in classes.iter().enumerate() {
            let l = lists[k];
            for (i, p) in l.items.iter().enumerate() {
                let Some(name) = model_name(l, p) else { continue };
                let Some(&model) = index.get(&(*class, name.to_ascii_lowercase().as_str())) else {
                    self.props_missing.push((*pl, i));
                    continue;
                };
                let b = shaiya_clientdata::smod::placement_basis(p.dir, p.up);
                let (c, radius) = pg.sphere[model];
                let centre = [
                    p.pos[0] + b[0][0] * c[0] + b[1][0] * c[1] + b[2][0] * c[2],
                    p.pos[1] + b[0][1] * c[0] + b[1][1] * c[1] + b[2][1] * c[2],
                    p.pos[2] + b[0][2] * c[0] + b[1][2] * c[1] + b[2][2] * c[2],
                ];
                if !centre.iter().all(|v| v.is_finite()) {
                    continue;
                }
                inst.push(PropInstance { model: model as u32, class: *class, centre, radius, axes: b, pos: p.pos });
            }
        }
        inst.sort_by_key(|i| i.model);
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < inst.len() {
            let model = inst[start].model;
            let end = start + inst[start..].iter().take_while(|i| i.model == model).count();
            ranges.push((model as usize, start..end));
            start = end;
        }
        self.prop_counts = [inst.iter().filter(|i| i.class == PropClass::Building).count(), inst.iter().filter(|i| i.class == PropClass::Shape).count(), inst.iter().filter(|i| i.class == PropClass::Tree).count()];
        pg.instances = inst;
        pg.ranges = ranges;
    }
}

fn pipeline_depth(write: bool, compare: wgpu::CompareFunction, bias: i32) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias != 0 { -1.5 } else { 0.0 }, clamp: 0.0 },
    }
}

fn mip_count(w: u32, h: u32) -> u32 {
    32 - w.max(h).leading_zeros()
}

fn write_mips(queue: &wgpu::Queue, tex: &wgpu::Texture, layer: u32, w: u32, h: u32, mips: &[Vec<u8>]) {
    for (m, data) in mips.iter().enumerate() {
        let (mw, mh) = ((w >> m).max(1), (h >> m).max(1));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: m as u32, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(mw * 4), rows_per_image: Some(mh) },
            wgpu::Extent3d { width: mw, height: mh, depth_or_array_layers: 1 },
        );
    }
}

fn texture_2d(device: &wgpu::Device, queue: &wgpu::Queue, img: &MipImage, label: &str) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        mip_level_count: mip_count(img.width, img.height).min(img.mips.len() as u32),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    write_mips(queue, &tex, 0, img.width, img.height, &img.mips);
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

impl Gpu3d {
    pub fn new(device: &wgpu::Device) -> Gpu3d {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("view3d"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let vis_all = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let tex_entry = |binding: u32, sample: wgpu::TextureSampleType, dim: wgpu::TextureViewDimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: sample, view_dimension: dim, multisampled: false },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let bgl_scene = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: vis_all, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                tex_entry(1, float, wgpu::TextureViewDimension::D2),
                tex_entry(2, float, wgpu::TextureViewDimension::D2),
            ],
        });
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
        let bgl_terrain = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[tex_entry(0, float, wgpu::TextureViewDimension::D2Array), sampler_entry(1), tex_entry(2, wgpu::TextureSampleType::Uint, wgpu::TextureViewDimension::D2)],
        });
        let bgl_dungeon = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dungeon"),
            entries: &[tex_entry(0, float, wgpu::TextureViewDimension::D2), tex_entry(1, float, wgpu::TextureViewDimension::D2), sampler_entry(2)],
        });
        let bgl_prop = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("prop"), entries: &[tex_entry(0, float, wgpu::TextureViewDimension::D2), sampler_entry(1)] });
        let layout = |label: &str, second: Option<&wgpu::BindGroupLayout>| {
            let mut v = vec![Some(&bgl_scene)];
            if let Some(s) = second {
                v.push(Some(s));
            }
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &v, immediate_size: 0 })
        };
        let layout_terrain = layout("terrain", Some(&bgl_terrain));
        let layout_dungeon = layout("dungeon", Some(&bgl_dungeon));
        let layout_prop = layout("prop", Some(&bgl_prop));
        let layout_overlay = layout("overlay", None);

        let ms = wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false };
        let colour_target = |blend: Option<wgpu::BlendState>| [Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })];
        let terrain_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4];
        let dungeon_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x3];
        let overlay_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4];
        let terrain_buf = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<TerrainVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &terrain_attrs };
        let dungeon_buf = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<DgVert>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &dungeon_attrs };
        let prop_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let inst_attrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
        let prop_buf = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<PropVert>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &prop_attrs };
        let inst_buf = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<InstanceRaw>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &inst_attrs };
        let overlay_buf = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<OVert>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &overlay_attrs };

        let make = |label: &str, layout: &wgpu::PipelineLayout, vs: &str, fs: &str, buf: &wgpu::VertexBufferLayout, topology: wgpu::PrimitiveTopology, cull: Option<wgpu::Face>, depth: wgpu::DepthStencilState, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[Some(buf.clone())] },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(fs), compilation_options: Default::default(), targets: &colour_target(blend) }),
                primitive: wgpu::PrimitiveState { topology, strip_index_format: None, front_face: wgpu::FrontFace::Cw, cull_mode: cull, ..Default::default() },
                depth_stencil: Some(depth),
                multisample: ms,
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction as C;
        use wgpu::PrimitiveTopology as T;
        let terrain_pipe = make("terrain", &layout_terrain, "vs_terrain", "fs_terrain", &terrain_buf, T::TriangleList, Some(wgpu::Face::Back), pipeline_depth(true, C::Less, 0), None);
        let dungeon_pipe = make("dungeon", &layout_dungeon, "vs_dungeon", "fs_dungeon", &dungeon_buf, T::TriangleList, Some(wgpu::Face::Back), pipeline_depth(true, C::Less, 0), None);
        let dungeon_pipe_nocull = make("dungeon two-sided", &layout_dungeon, "vs_dungeon", "fs_dungeon", &dungeon_buf, T::TriangleList, None, pipeline_depth(true, C::Less, 0), None);
        // props: two vertex streams (the model, the per-instance matrix), two-sided
        let prop_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("prop"),
            layout: Some(&layout_prop),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_prop"), compilation_options: Default::default(), buffers: &[Some(prop_buf.clone()), Some(inst_buf.clone())] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_prop"), compilation_options: Default::default(), targets: &colour_target(None) }),
            primitive: wgpu::PrimitiveState { topology: T::TriangleList, strip_index_format: None, front_face: wgpu::FrontFace::Cw, cull_mode: None, ..Default::default() },
            depth_stencil: Some(pipeline_depth(true, C::Less, 0)),
            multisample: ms,
            multiview_mask: None,
            cache: None,
        });
        let tri_blend = make("overlay blend", &layout_overlay, "vs_overlay", "fs_overlay", &overlay_buf, T::TriangleList, None, pipeline_depth(false, C::LessEqual, -2), Some(wgpu::BlendState::ALPHA_BLENDING));
        let tri_solid = make("overlay solid", &layout_overlay, "vs_overlay", "fs_overlay", &overlay_buf, T::TriangleList, None, pipeline_depth(true, C::LessEqual, -2), None);
        let lines = make("overlay lines", &layout_overlay, "vs_overlay", "fs_overlay", &overlay_buf, T::LineList, None, pipeline_depth(false, C::LessEqual, 0), Some(wgpu::BlendState::ALPHA_BLENDING));
        let lines_top = make("overlay lines top", &layout_overlay, "vs_overlay", "fs_overlay", &overlay_buf, T::LineList, None, pipeline_depth(false, C::Always, 0), Some(wgpu::BlendState::ALPHA_BLENDING));

        let indices = scene3d::chunk_indices();
        let chunk_ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("chunk indices"), contents: bytemuck::cast_slice(&indices), usage: wgpu::BufferUsages::INDEX });
        let uni = device.create_buffer(&wgpu::BufferDescriptor { label: Some("view3d uniforms"), size: std::mem::size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let samp_repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        Gpu3d {
            uni,
            bgl_scene,
            bgl_terrain,
            bgl_dungeon,
            bgl_prop,
            terrain_pipe,
            prop_pipe,
            dungeon_pipe,
            dungeon_pipe_nocull,
            tri_blend,
            tri_solid,
            lines,
            lines_top,
            chunk_ib,
            chunk_index_count: indices.len() as u32,
            samp_repeat,
            ov_solid: GrowBuf::new(),
            ov_blend: GrowBuf::new(),
            ov_lines: GrowBuf::new(),
            ov_top: GrowBuf::new(),
            prop_inst: GrowBuf::new(),
            target: None,
        }
    }

    /// Uploads a scene (textures, vertex buffers, bind groups). Runs on the UI thread; a 2048 field is about 20 MB.
    /// Uploads one model that was loaded after the scene was built (a placement added from the browser) so its placements draw
    /// as the real model. The caller then rebuilds the instances ([`SceneGpu::invalidate_placements`]).
    #[allow(clippy::too_many_arguments)] // the model and its textures come with the GPU handles; a struct would only rename them
    pub fn add_prop_model(&self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &mut SceneGpu, class: PropClass, name: &str, model: scene3d::PropModel, textures: &[MipImage]) {
        let tex_views: Vec<wgpu::TextureView> = textures.iter().map(|i| texture_2d(device, queue, i, "prop texture")).collect();
        let fallback = texture_2d(device, queue, &scene3d::mip_image_capped(&shaiya_clientdata::texture::Image::checker(64)), "checker");
        let mut batches = Vec::with_capacity(model.batches.len());
        for b in &model.batches {
            let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prop vertices"), contents: bytemuck::cast_slice(&b.verts), usage: wgpu::BufferUsages::VERTEX });
            let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prop indices"), contents: bytemuck::cast_slice(&b.indices), usage: wgpu::BufferUsages::INDEX });
            let tv = b.texture.map_or(&fallback, |i| &tex_views[i]);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prop batch"),
                layout: &self.bgl_prop,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(tv) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samp_repeat) }],
            });
            batches.push(PropBatchGpu { vb, ib, index_count: b.indices.len() as u32, bind });
        }
        let pg = scene.props.get_or_insert_with(|| PropsGpu { models: Vec::new(), instances: Vec::new(), ranges: Vec::new(), keys: Vec::new(), sphere: Vec::new() });
        pg.models.push(PropModelGpu { batches });
        pg.keys.push((class, name.to_ascii_lowercase()));
        pg.sphere.push((model.centre, model.radius));
    }

    pub fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, data: SceneData) -> SceneGpu {
        let blocked_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blocked cells"),
            size: wgpu::Extent3d { width: data.size.max(1), height: data.size.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let blocked_view = blocked_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let heat_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("heat tiles"),
            size: wgpu::Extent3d { width: HEAT_TEX, height: HEAT_TEX, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let heat_view = heat_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_scene = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &self.bgl_scene,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.uni.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&blocked_view) }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&heat_view) }],
        });

        let mut terrain = None;
        let (mut terrain_n, mut terrain_layers, mut tiling) = (0, 1, Vec::new());
        if let Some(t) = &data.terrain {
            let mut bytes: Vec<u8> = Vec::with_capacity(t.chunks.len() * CHUNK_VERTS * std::mem::size_of::<TerrainVertex>());
            let mut bounds = Vec::new();
            for c in &t.chunks {
                bytes.extend_from_slice(bytemuck::cast_slice(&c.verts));
                bounds.push((c.min, c.max));
            }
            let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("terrain vertices"), contents: &bytes, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST });
            let layers = t.layers.len().min(MAX_LAYERS) as u32;
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain layers"),
                size: wgpu::Extent3d { width: t.layer_size, height: t.layer_size, depth_or_array_layers: layers },
                mip_level_count: mip_count(t.layer_size, t.layer_size),
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (l, mips) in t.layers.iter().take(MAX_LAYERS).enumerate() {
                write_mips(queue, &tex, l as u32, t.layer_size, t.layer_size, mips);
            }
            let layers_view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let idx_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("terrain corner layers"),
                size: wgpu::Extent3d { width: t.n as u32, height: t.n as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &idx_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &t.corner_idx,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(t.n as u32), rows_per_image: Some(t.n as u32) },
                wgpu::Extent3d { width: t.n as u32, height: t.n as u32, depth_or_array_layers: 1 },
            );
            let idx_view = idx_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain"),
                layout: &self.bgl_terrain,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&layers_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samp_repeat) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&idx_view) },
                ],
            });
            terrain_n = t.n as u32;
            terrain_layers = layers;
            tiling = t.tiling.clone();
            terrain = Some(TerrainGpu { vb, chunk_bounds: bounds, bind });
        }

        let mut dungeon = Vec::new();
        let mut dungeon_triangles = 0;
        if let Some(d) = &data.dungeon {
            dungeon_triangles = d.triangles;
            let tex_views: Vec<wgpu::TextureView> = d.textures.iter().map(|i| texture_2d(device, queue, i, "dungeon texture")).collect();
            let lm_views: Vec<wgpu::TextureView> = d.lightmaps.iter().map(|i| texture_2d(device, queue, i, "lightmap")).collect();
            let fallback_tex = texture_2d(device, queue, &scene3d::mip_image_capped(&shaiya_clientdata::texture::Image::checker(64)), "checker");
            let fallback_lm = texture_2d(device, queue, &MipImage { width: 1, height: 1, mips: vec![vec![128, 128, 128, 255]] }, "grey");
            for b in &d.batches {
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dungeon vertices"), contents: bytemuck::cast_slice(&b.verts), usage: wgpu::BufferUsages::VERTEX });
                let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dungeon indices"), contents: bytemuck::cast_slice(&b.indices), usage: wgpu::BufferUsages::INDEX });
                let tv = b.texture.map_or(&fallback_tex, |i| &tex_views[i]);
                let lv = b.lightmap.map_or(&fallback_lm, |i| &lm_views[i]);
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("dungeon batch"),
                    layout: &self.bgl_dungeon,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(tv) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(lv) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.samp_repeat) },
                    ],
                });
                dungeon.push(DungeonBatchGpu { vb, ib, index_count: b.indices.len() as u32, min: b.min, max: b.max, bind });
            }
        }
        let mut props = None;
        let mut prop_counts = [0usize; 3];
        if let Some(p) = &data.props {
            prop_counts = [p.count(PropClass::Building), p.count(PropClass::Shape), p.count(PropClass::Tree)];
            let tex_views: Vec<wgpu::TextureView> = p.textures.iter().map(|i| texture_2d(device, queue, i, "prop texture")).collect();
            let fallback = texture_2d(device, queue, &scene3d::mip_image_capped(&shaiya_clientdata::texture::Image::checker(64)), "checker");
            let mut models = Vec::with_capacity(p.models.len());
            for m in &p.models {
                let mut batches = Vec::with_capacity(m.batches.len());
                for b in &m.batches {
                    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prop vertices"), contents: bytemuck::cast_slice(&b.verts), usage: wgpu::BufferUsages::VERTEX });
                    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prop indices"), contents: bytemuck::cast_slice(&b.indices), usage: wgpu::BufferUsages::INDEX });
                    let tv = b.texture.map_or(&fallback, |i| &tex_views[i]);
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("prop batch"),
                        layout: &self.bgl_prop,
                        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(tv) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.samp_repeat) }],
                    });
                    batches.push(PropBatchGpu { vb, ib, index_count: b.indices.len() as u32, bind });
                }
                models.push(PropModelGpu { batches });
            }
            let mut ranges = Vec::new();
            let mut start = 0;
            while start < p.instances.len() {
                let model = p.instances[start].model;
                let end = start + p.instances[start..].iter().take_while(|i| i.model == model).count();
                ranges.push((model as usize, start..end));
                start = end;
            }
            props = Some(PropsGpu { models, instances: p.instances.clone(), ranges, keys: p.keys.clone(), sphere: p.models.iter().map(|m| (m.centre, m.radius)).collect() });
        }
        SceneGpu {
            size: data.size,
            flat: data.flat,
            is_dungeon: data.is_dungeon,
            fog: data.fog,
            bounds_min: data.bounds_min,
            bounds_max: data.bounds_max,
            problems: data.problems,
            terrain,
            terrain_n,
            terrain_seen: (0, 0),
            terrain_layers,
            tiling,
            dungeon,
            dungeon_triangles,
            props,
            prop_counts,
            props_rev: None,
            props_missing: Vec::new(),
            props_failed: Default::default(),
            blocked_tex,
            blocked_rev: None,
            heat_tex,
            heat_rev: None,
            bind_scene,
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let make = |label: &str, format: wgpu::TextureFormat, samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: extent, mip_level_count: 1, sample_count: samples, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] })
        };
        let ms = make("view3d colour (msaa)", COLOR_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = make("view3d colour", COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let depth = make("view3d depth", DEPTH_FORMAT, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let egui_id = self.target.take().and_then(|t| t.egui_id);
        self.target = Some(Target {
            size,
            ms_view: ms.create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            resolve_tex: resolve,
            depth_view: depth.create_view(&Default::default()),
            egui_id,
            egui_stale: true,
        });
    }

    /// Draws the scene and the overlays and hands the picture to egui; returns the egui texture to show and what was drawn.
    pub fn render(&mut self, rs: &RenderState, scene: &mut SceneGpu, f: &FrameParams, ov: &OverlayGeo) -> (egui::TextureId, FrameStats) {
        let stats = self.draw(&rs.device, &rs.queue, scene, f, ov);
        let t = self.target.as_mut().expect("draw makes the target");
        if t.egui_stale {
            let mut renderer = rs.renderer.write();
            match t.egui_id {
                Some(id) => renderer.update_egui_texture_from_wgpu_texture(&rs.device, &t.resolve_view, wgpu::FilterMode::Linear, id),
                None => t.egui_id = Some(renderer.register_native_texture(&rs.device, &t.resolve_view, wgpu::FilterMode::Linear)),
            }
            t.egui_stale = false;
        }
        (t.egui_id.expect("registered above"), stats)
    }

    /// Draws the scene and the overlays into the view's own texture (`f.size_px`).
    pub fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &mut SceneGpu, f: &FrameParams, ov: &OverlayGeo) -> FrameStats {
        let size = [f.size_px[0].clamp(8, 8192), f.size_px[1].clamp(8, 8192)];
        self.ensure_target(device, size);

        // blocked cells texture
        if let Some(b) = f.blocked {
            if scene.blocked_rev != Some(f.blocked_rev) {
                let s = scene.size;
                let mut bytes = vec![0u8; (s as usize) * (s as usize)];
                if b.size() == s {
                    for z in 0..s {
                        for x in 0..s {
                            if b.get(x, z) {
                                bytes[(z * s + x) as usize] = 255;
                            }
                        }
                    }
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &scene.blocked_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &bytes,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s), rows_per_image: Some(s) },
                    wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                );
                scene.blocked_rev = Some(f.blocked_rev);
            }
        }

        // heat tiles texture (the top-left n x n of it)
        if let Some(h) = f.heat.filter(|h| h.n >= 1 && h.n <= HEAT_TEX && h.rgba.len() == (h.n * h.n * 4) as usize) {
            if scene.heat_rev != Some(h.rev) {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &scene.heat_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    h.rgba,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(h.n * 4), rows_per_image: Some(h.n) },
                    wgpu::Extent3d { width: h.n, height: h.n, depth_or_array_layers: 1 },
                );
                scene.heat_rev = Some(h.rev);
            }
        }
        let heat_uniform = match f.heat {
            Some(h) if h.n >= 1 && h.n <= HEAT_TEX && h.rgba.len() == (h.n * h.n * 4) as usize => [1.0, h.opacity, h.tile, h.n as f32],
            _ => [0.0; 4],
        };

        let mut tiling = [[0.0f32; 4]; 64];
        for (i, t) in scene.tiling.iter().take(MAX_LAYERS).enumerate() {
            tiling[i / 4][i % 4] = *t;
        }
        let light = math3d::norm([-0.45, 0.75, 0.35]);
        let uniforms = Uniforms {
            view_proj: f.view_proj,
            eye: [f.eye[0], f.eye[1], f.eye[2], 1.0],
            fog: [scene.fog[0], scene.fog[1], scene.fog[2], 1.0],
            params: [f.fog_start, f.fog_end, if f.blocked.is_some() { 1.0 } else { 0.0 }, 0.45],
            map: [scene.size as f32, scene.terrain_n as f32, scene.terrain_layers as f32, 0.0],
            light: [light[0], light[1], light[2], 0.0],
            heat: heat_uniform,
            tiling,
        };
        queue.write_buffer(&self.uni, 0, bytemuck::bytes_of(&uniforms));
        let n_solid = self.ov_solid.write(device, queue, &ov.tris_solid);
        let n_blend = self.ov_blend.write(device, queue, &ov.tris_blend);
        let n_lines = self.ov_lines.write(device, queue, &ov.lines);
        let n_top = self.ov_top.write(device, queue, &ov.lines_top);

        let planes = math3d::frustum(&f.view_proj);
        let mut stats = FrameStats::default();
        // props: the instances inside the cut-off, the view and big enough on the screen, grouped by model
        let mut prop_draws: Vec<(usize, u32, u32)> = Vec::new(); // (model, first instance, count)
        if let Some(pg) = scene.props.as_ref() {
            let mut raw: Vec<InstanceRaw> = Vec::new();
            let px_per_unit = f.view_proj[1][1] * f.size_px[1] as f32 * 0.5;
            stats.props_total = pg.instances.len() as u32;
            if f.draw_buildings || f.draw_trees {
                for (model, range) in &pg.ranges {
                    let first = raw.len();
                    for inst in &pg.instances[range.clone()] {
                        let wanted = if inst.class == PropClass::Tree { f.draw_trees } else { f.draw_buildings };
                        if !wanted {
                            continue;
                        }
                        let d = math3d::len(math3d::sub(inst.centre, f.eye));
                        if d - inst.radius > f.prop_distance || inst.radius * px_per_unit < PROP_MIN_PIXELS * d {
                            continue;
                        }
                        let r = inst.radius;
                        if !math3d::box_visible(&planes, [inst.centre[0] - r, inst.centre[1] - r, inst.centre[2] - r], [inst.centre[0] + r, inst.centre[1] + r, inst.centre[2] + r]) {
                            continue;
                        }
                        raw.push(InstanceRaw::of(inst));
                    }
                    if raw.len() > first {
                        prop_draws.push((*model, first as u32, (raw.len() - first) as u32));
                    }
                }
            }
            stats.props_drawn = raw.len() as u32;
            self.prop_inst.write(device, queue, &raw);
        }
        let target = self.target.as_ref().unwrap();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("view3d") });
        {
            let sky = clear_colour(scene.fog);
            let clear = wgpu::Color { r: sky[0] as f64, g: sky[1] as f64, b: sky[2] as f64, a: 1.0 };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("view3d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target.ms_view, depth_slice: None, resolve_target: Some(&target.resolve_view), ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Discard } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &target.depth_view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &scene.bind_scene, &[]);
            if let Some(t) = scene.terrain.as_ref().filter(|_| f.draw_ground) {
                pass.set_pipeline(&self.terrain_pipe);
                pass.set_bind_group(1, &t.bind, &[]);
                pass.set_index_buffer(self.chunk_ib.slice(..), wgpu::IndexFormat::Uint16);
                let stride = (CHUNK_VERTS * std::mem::size_of::<TerrainVertex>()) as u64;
                stats.chunks_total = t.chunk_bounds.len() as u32;
                for (i, (mn, mx)) in t.chunk_bounds.iter().enumerate() {
                    if !math3d::box_visible(&planes, *mn, *mx) {
                        continue;
                    }
                    pass.set_vertex_buffer(0, t.vb.slice(i as u64 * stride..(i as u64 + 1) * stride));
                    pass.draw_indexed(0..self.chunk_index_count, 0, 0..1);
                    stats.draw_calls += 1;
                    stats.chunks_drawn += 1;
                    stats.triangles += (self.chunk_index_count / 3) as u64;
                }
            }
            if f.draw_dungeon && !scene.dungeon.is_empty() {
                pass.set_pipeline(if f.backface_cull { &self.dungeon_pipe } else { &self.dungeon_pipe_nocull });
                for b in &scene.dungeon {
                    if !math3d::box_visible(&planes, b.min, b.max) {
                        continue;
                    }
                    pass.set_bind_group(1, &b.bind, &[]);
                    pass.set_vertex_buffer(0, b.vb.slice(..));
                    pass.set_index_buffer(b.ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..b.index_count, 0, 0..1);
                    stats.draw_calls += 1;
                    stats.triangles += (b.index_count / 3) as u64;
                }
            }
            if let (Some(pg), Some(ibuf)) = (scene.props.as_ref(), self.prop_inst.buf.as_ref()) {
                if !prop_draws.is_empty() {
                    pass.set_pipeline(&self.prop_pipe);
                    pass.set_vertex_buffer(1, ibuf.slice(..));
                    for (model, first, count) in &prop_draws {
                        let m = &pg.models[*model];
                        for b in &m.batches {
                            pass.set_bind_group(1, &b.bind, &[]);
                            pass.set_vertex_buffer(0, b.vb.slice(..));
                            pass.set_index_buffer(b.ib.slice(..), wgpu::IndexFormat::Uint16);
                            pass.draw_indexed(0..b.index_count, 0, *first..*first + *count);
                            stats.draw_calls += 1;
                            stats.triangles += (b.index_count / 3) as u64 * *count as u64;
                        }
                    }
                }
            }
            let mut draw = |pipe: &wgpu::RenderPipeline, buf: &GrowBuf, n: u32| {
                if n > 0 {
                    pass.set_pipeline(pipe);
                    pass.set_vertex_buffer(0, buf.buf.as_ref().unwrap().slice(..(n as u64 * std::mem::size_of::<OVert>() as u64)));
                    pass.draw(0..n, 0..1);
                    stats.draw_calls += 1;
                }
            };
            draw(&self.tri_solid, &self.ov_solid, n_solid);
            draw(&self.tri_blend, &self.ov_blend, n_blend);
            draw(&self.lines, &self.ov_lines, n_lines);
            draw(&self.lines_top, &self.ov_top, n_top);
        }
        queue.submit([enc.finish()]);
        stats
    }
}

/// Scenes by tab, newest last; at most `cap` are kept.
pub struct SceneCache<T> {
    pub items: HashMap<u64, T>,
    order: Vec<u64>,
    cap: usize,
}

impl<T> SceneCache<T> {
    pub fn new(cap: usize) -> SceneCache<T> {
        SceneCache { items: HashMap::new(), order: Vec::new(), cap }
    }

    pub fn insert(&mut self, key: u64, v: T) {
        self.order.retain(|k| *k != key);
        self.order.push(key);
        self.items.insert(key, v);
        while self.order.len() > self.cap {
            let old = self.order.remove(0);
            self.items.remove(&old);
        }
    }

    pub fn touch(&mut self, key: u64) {
        if let Some(p) = self.order.iter().position(|k| *k == key) {
            let k = self.order.remove(p);
            self.order.push(k);
        }
    }

    pub fn remove(&mut self, key: u64) {
        self.order.retain(|k| *k != key);
        self.items.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_is_the_fog_colour_20_percent_darker() {
        let c = clear_colour([0.5, 0.25, 1.0]);
        assert!((c[0] - 0.4).abs() < 1e-6 && (c[1] - 0.2).abs() < 1e-6 && (c[2] - 0.8).abs() < 1e-6, "{c:?}");
        // the map's own fog colour from the .wld stays inside 0..1 after it
        assert!(clear_colour([1.0, 1.0, 1.0]).iter().all(|v| *v <= 1.0));
    }

    #[test]
    fn scene_cache_drops_the_oldest() {
        let mut c = SceneCache::new(2);
        c.insert(1, "a");
        c.insert(2, "b");
        c.touch(1);
        c.insert(3, "c");
        assert!(c.items.contains_key(&1) && c.items.contains_key(&3) && !c.items.contains_key(&2));
        c.remove(1);
        assert_eq!(c.items.len(), 1);
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// A device without a window (None when this machine has no usable adapter).
    fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(adapter) = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok() else {
            eprintln!("skipped: this machine has no usable graphics adapter");
            return None;
        };
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
    }

    fn read_pixels(g: &Gpu3d, device: &wgpu::Device, queue: &wgpu::Queue) -> (u32, u32, Vec<u8>) {
        let t = g.target.as_ref().unwrap();
        let (w, h) = (t.size[0], t.size[1]);
        let row = (w * 4).next_multiple_of(256);
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &t.resolve_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap();
        let mut out = Vec::new();
        for y in 0..h {
            out.extend_from_slice(&data[(y * row) as usize..(y * row + w * 4) as usize]);
        }
        (w, h, out)
    }

    /// The whole pipeline without a window: a flat 128-unit scene is drawn from above with a blocked patch and one overlay
    /// triangle, and the pixels are checked (background = fog colour, ground, red tint, overlay).
    #[test]
    fn a_flat_scene_renders_ground_blocked_tint_and_overlay() {
        let Some((device, queue)) = headless() else { return };
        let mut gpu = Gpu3d::new(&device);
        let mut scene = gpu.upload(&device, &queue, SceneData::build(None, None, 128));
        let mut blocked = shaiya_svmap::BitGrid::new(128);
        blocked.fill_rect(60, 60, 68, 68, true);
        let cam = crate::view3d::Camera3d { focus: [64.0, 0.0, 64.0], yaw: 0.0, pitch: -89f32.to_radians(), dist: 200.0, ..Default::default() };
        let (w, h) = (320u32, 240u32);
        let vp = cam.view_proj(w as f32 / h as f32);
        let mut ov = OverlayGeo::default();
        for p in [[84.0, 1.0, 60.0], [84.0, 1.0, 70.0], [94.0, 1.0, 65.0]] {
            ov.tris_solid.push(OVert { pos: p, col: [0, 255, 0, 255] });
        }
        let params = FrameParams { view_proj: vp, eye: cam.eye(), size_px: [w, h], fog_start: 2000.0, fog_end: 4000.0, blocked: Some(&blocked), blocked_rev: 1, backface_cull: true, draw_ground: true, draw_dungeon: true, draw_buildings: true, draw_trees: true, prop_distance: 800.0, heat: None };
        let stats = gpu.draw(&device, &queue, &mut scene, &params, &ov);
        assert!(stats.draw_calls >= 2 && stats.chunks_drawn >= 1);
        let (pw, ph, px) = read_pixels(&gpu, &device, &queue);
        assert_eq!((pw, ph), (w, h));
        let at = |p: V3| {
            let c = math3d::project(&vp, p);
            let (x, y) = (((c[0] / c[3] * 0.5 + 0.5) * w as f32) as usize, ((0.5 - c[1] / c[3] * 0.5) * h as f32) as usize);
            let o = (y * w as usize + x) * 4;
            [px[o], px[o + 1], px[o + 2]]
        };
        let bg = [px[0], px[1], px[2]];
        assert!(bg.iter().zip([86u8, 102, 118]).all(|(a, b)| a.abs_diff(b) <= 3), "the corner shows the clear colour (the fog colour, 20 percent darker): {bg:?}");
        let ground = at([30.0, 0.0, 30.0]);
        assert!(ground != bg && ground[0].abs_diff(ground[2]) < 30, "plain ground: {ground:?}");
        let tinted = at([64.0, 0.0, 64.0]);
        assert!(tinted[0] > tinted[1] + 40 && tinted[0] > tinted[2] + 40, "blocked cells are tinted red: {tinted:?}");
        let tri = at([86.0, 1.0, 65.0]);
        assert!(tri[1] > 200 && tri[0] < 60, "the overlay triangle is drawn: {tri:?}");
        // the picture follows a changed grid
        let mut freed = blocked.clone();
        freed.fill_rect(60, 60, 68, 68, false);
        let params2 = FrameParams { blocked_rev: 2, blocked: Some(&freed), ..params };
        gpu.draw(&device, &queue, &mut scene, &params2, &OverlayGeo::default());
        let (_, _, px2) = read_pixels(&gpu, &device, &queue);
        let c = math3d::project(&vp, [64.0, 0.0, 64.0]);
        let o = (((0.5 - c[1] / c[3] * 0.5) * h as f32) as usize * w as usize + ((c[0] / c[3] * 0.5 + 0.5) * w as f32) as usize) * 4;
        assert!(px2[o].abs_diff(px2[o + 2]) < 30, "untinted after the cells are freed");
        // the heatmap on the terrain: a 4 x 4 grid of 32-unit tiles, the tile of (80, 90) is a solid blue, the ground at (30, 30) is not
        let mut rgba = vec![0u8; 4 * 4 * 4];
        let (tx, tz) = (80 / 32, 90 / 32);
        let o = ((4 - 1 - tz) * 4 + tx) * 4;
        rgba[o..o + 4].copy_from_slice(&[0, 0, 255, 255]);
        let heat = HeatParams { rgba: &rgba, n: 4, tile: 32.0, opacity: 0.9, rev: 1 };
        let params3 = FrameParams { blocked: None, heat: Some(heat), ..params2 };
        gpu.draw(&device, &queue, &mut scene, &params3, &OverlayGeo::default());
        let (_, _, px3) = read_pixels(&gpu, &device, &queue);
        let pick = |p: V3| {
            let c = math3d::project(&vp, p);
            let o = (((0.5 - c[1] / c[3] * 0.5) * h as f32) as usize * w as usize + ((c[0] / c[3] * 0.5 + 0.5) * w as f32) as usize) * 4;
            [px3[o], px3[o + 1], px3[o + 2]]
        };
        let hot = pick([80.0, 0.0, 90.0]);
        assert!(hot[2] > hot[0] + 60 && hot[2] > hot[1] + 60, "the tile is draped in blue: {hot:?}");
        let cold = pick([30.0, 0.0, 30.0]);
        assert!(cold[2] <= cold[0].max(cold[1]) + 40, "other tiles keep the ground: {cold:?}");
    }

    /// A terrain edit rewrites only the chunks it touches: a scene updated for the edited rectangle draws (within a small tolerance) what the same scene
    /// updated for the whole grid draws, and both differ from the flat ground (needs an adapter; skipped without one).
    #[test]
    fn terrain_updates_write_only_the_touched_chunks() {
        let Some((device, queue)) = headless() else { return };
        let mut gpu = Gpu3d::new(&device);
        let size = 256u32; // 129 corners a side, 2 x 2 chunks
        let n = 129usize;
        let cam = crate::view3d::Camera3d { focus: [128.0, 0.0, 128.0], yaw: 0.0, pitch: -50f32.to_radians(), dist: 300.0, ..Default::default() };
        let (w, h) = (320u32, 240u32);
        let params = FrameParams { view_proj: cam.view_proj(w as f32 / h as f32), eye: cam.eye(), size_px: [w, h], fog_start: 4000.0, fog_end: 8000.0, blocked: None, blocked_rev: 0, backface_cull: true, draw_ground: true, draw_dungeon: true, draw_buildings: true, draw_trees: true, prop_distance: 800.0, heat: None };
        let render = |gpu: &mut Gpu3d, scene: &mut SceneGpu| {
            gpu.draw(&device, &queue, scene, &params, &OverlayGeo::default());
            read_pixels(gpu, &device, &queue).2
        };
        let flat_h = vec![crate::terrain::raw_of(0.0); n * n];
        let mut hill = flat_h.clone();
        // a hill that sits in chunk (1, 1) and reaches over the border into (0, 1)
        for cz in 70..100 {
            for cx in 60..90 {
                hill[cz * n + cx] = crate::terrain::raw_of(25.0);
            }
        }
        let mut a = gpu.upload(&device, &queue, SceneData::build(None, None, size));
        let flat_px = render(&mut gpu, &mut a);
        assert_eq!(a.terrain_seen(), (0, 0));
        let written = a.update_terrain(&queue, &hill, n, Some([60, 70, 89, 99]), (7, 1));
        assert_eq!(written, 2, "the hill spans the border at corner 64 in x and lies in the second chunk row: two of the four chunks");
        assert_eq!(a.terrain_seen(), (7, 1));
        let part_px = render(&mut gpu, &mut a);
        let mut b = gpu.upload(&device, &queue, SceneData::build(None, None, size));
        assert_eq!(b.update_terrain(&queue, &hill, n, None, (7, 1)), 4);
        let full_px = render(&mut gpu, &mut b);
        // an adapter need not draw two identical frames bit for bit: compare with a tolerance and report what differed
        let diff = |a: &[u8], b: &[u8]| {
            let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
            let count = a.chunks(4).zip(b.chunks(4)).filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 2)).count();
            (count, max)
        };
        let pixels = part_px.len() / 4;
        let (shows, _) = diff(&part_px, &flat_px);
        assert!(shows > pixels / 100, "the hill shows ({shows} of {pixels} pixels differ clearly from the flat ground)");
        let (count, max) = diff(&part_px, &full_px);
        assert!(count * 2000 < pixels, "a partial update gives the picture a full one gives: {count} of {pixels} pixels differ by more than 2, largest channel difference {max}");
    }

    /// Real maps drawn without a window (needs `SHAIYA_CLIENT` and an adapter): a field and a dungeon each give a picture that is
    /// mostly not the sky colour.
    #[test]
    fn real_field_and_dungeon_render() {
        let Some(c) = crate::scene3d::tests::client() else { return };
        let Some((device, queue)) = headless() else { return };
        let mut gpu = Gpu3d::new(&device);
        for (id, size, pitch) in [(18u32, 1024u32, -50f32), (65, 440, -50.0)] {
            let w = c.wld(id).unwrap();
            let data = SceneData::build(Some(&c), Some(&w), size);
            let (bmin, bmax) = (data.bounds_min, data.bounds_max);
            let fog = data.fog;
            let mut scene = gpu.upload(&device, &queue, data);
            let mut cam = crate::view3d::Camera3d::default();
            cam.frame_map(bmin, bmax);
            cam.pitch = pitch.to_radians();
            let (w_px, h_px) = (400u32, 300u32);
            let params = FrameParams { view_proj: cam.view_proj(w_px as f32 / h_px as f32), eye: cam.eye(), size_px: [w_px, h_px], fog_start: cam.dist * 2.5, fog_end: cam.dist * 14.0, blocked: None, blocked_rev: 0, backface_cull: true, draw_ground: true, draw_dungeon: true, draw_buildings: true, draw_trees: true, prop_distance: 800.0, heat: None };
            let stats = gpu.draw(&device, &queue, &mut scene, &params, &OverlayGeo::default());
            let (_, _, px) = read_pixels(&gpu, &device, &queue);
            let dark = clear_colour(fog);
            let sky = [(dark[0] * 255.0) as u8, (dark[1] * 255.0) as u8, (dark[2] * 255.0) as u8];
            let covered = px.chunks(4).filter(|p| p[0].abs_diff(sky[0]) > 6 || p[1].abs_diff(sky[1]) > 6 || p[2].abs_diff(sky[2]) > 6).count();
            let frac = covered as f32 / (w_px * h_px) as f32;
            eprintln!("map {id}: {} draw calls, {} triangles, {:.0} % of the picture is not sky", stats.draw_calls, stats.triangles, frac * 100.0);
            assert!(stats.triangles > 10_000 && frac > 0.08, "map {id} draws something: {stats:?} {frac}");
        }
    }

    /// The town of map 35 drawn from the air: models are instanced, the layers and the cut-off switch them off, far or tiny
    /// instances are skipped, and the picture changes with the buildings in it.
    #[test]
    fn real_town_draws_its_buildings_and_the_cut_off_works() {
        let Some(c) = crate::scene3d::tests::client() else { return };
        let Some((device, queue)) = headless() else { return };
        let mut gpu = Gpu3d::new(&device);
        let w = c.wld(35).unwrap();
        let data = SceneData::build(Some(&c), Some(&w), 1024);
        let mut scene = gpu.upload(&device, &queue, data);
        assert_eq!(scene.prop_counts, [28, 184, 36]);
        let cam = crate::view3d::Camera3d { focus: [485.0, 26.0, 420.0], pitch: (-40f32).to_radians(), dist: 260.0, ..Default::default() };
        let (w_px, h_px) = (500u32, 320u32);
        let base = FrameParams { view_proj: cam.view_proj(w_px as f32 / h_px as f32), eye: cam.eye(), size_px: [w_px, h_px], fog_start: cam.dist * 2.5, fog_end: cam.dist * 14.0, blocked: None, blocked_rev: 0, backface_cull: true, draw_ground: true, draw_dungeon: true, draw_buildings: true, draw_trees: true, prop_distance: 800.0, heat: None };

        let on = gpu.draw(&device, &queue, &mut scene, &base, &OverlayGeo::default());
        let (_, _, px_on) = read_pixels(&gpu, &device, &queue);
        assert_eq!(on.props_total, 248);
        assert!(on.props_drawn > 60 && on.props_drawn <= 248, "{on:?}");

        let off = gpu.draw(&device, &queue, &mut scene, &FrameParams { draw_buildings: false, draw_trees: false, ..base }, &OverlayGeo::default());
        let (_, _, px_off) = read_pixels(&gpu, &device, &queue);
        assert_eq!(off.props_drawn, 0);
        assert!(on.triangles > off.triangles + 10_000, "{on:?} {off:?}");
        let differing = px_on.chunks(4).zip(px_off.chunks(4)).filter(|(a, b)| a != b).count();
        assert!(differing > 5_000, "the buildings change the picture ({differing} pixels differ)");

        // trees alone, buildings alone
        let trees = gpu.draw(&device, &queue, &mut scene, &FrameParams { draw_buildings: false, ..base }, &OverlayGeo::default());
        let bld = gpu.draw(&device, &queue, &mut scene, &FrameParams { draw_trees: false, ..base }, &OverlayGeo::default());
        assert!(trees.props_drawn > 0 && bld.props_drawn > 0 && trees.props_drawn + bld.props_drawn == on.props_drawn, "{trees:?} {bld:?} {on:?}");

        // a short cut-off keeps only what is close; a tiny one draws nothing
        let near = gpu.draw(&device, &queue, &mut scene, &FrameParams { prop_distance: 120.0, ..base }, &OverlayGeo::default());
        assert!(near.props_drawn < on.props_drawn, "{near:?} {on:?}");
        let none = gpu.draw(&device, &queue, &mut scene, &FrameParams { prop_distance: 1.0, ..base }, &OverlayGeo::default());
        assert_eq!(none.props_drawn, 0);

        // looking away from the town: the frustum drops (nearly) all of them
        let away = crate::view3d::Camera3d { fly: true, fly_eye: cam.eye(), yaw: cam.yaw + std::f32::consts::PI, pitch: 0.0, ..cam };
        let gone = gpu.draw(&device, &queue, &mut scene, &FrameParams { view_proj: away.view_proj(w_px as f32 / h_px as f32), eye: away.eye(), ..base }, &OverlayGeo::default());
        assert!(gone.props_drawn < on.props_drawn / 2, "{gone:?}");
        eprintln!("town: {} of {} props drawn ({} draw calls, {} triangles); buildings only {}, trees only {}; cut-off 120: {}", on.props_drawn, on.props_total, on.draw_calls, on.triangles, bld.props_drawn, trees.props_drawn, near.props_drawn);
    }

    #[test]
    fn instance_matrices_hold_axes_and_position() {
        let inst = PropInstance { model: 0, class: PropClass::Shape, centre: [0.0; 3], radius: 2.0, axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], pos: [5.0, 6.0, 7.0] };
        let raw = InstanceRaw::of(&inst);
        assert_eq!(raw.cols[3], [5.0, 6.0, 7.0, 1.0]);
        assert_eq!(raw.cols[0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(std::mem::size_of::<InstanceRaw>(), 64);
        assert_eq!(std::mem::size_of::<PropVert>(), 32);
    }

    #[test]
    fn mip_counts() {
        assert_eq!(mip_count(256, 256), 9);
        assert_eq!(mip_count(1, 1), 1);
        assert_eq!(mip_count(256, 128), 9);
        assert_eq!(mip_count(5, 3), 3);
    }

    #[test]
    fn uniform_block_is_a_multiple_of_16_bytes() {
        assert_eq!(std::mem::size_of::<Uniforms>() % 16, 0);
        assert_eq!(std::mem::size_of::<Uniforms>(), 64 + 6 * 16 + 1024);
        assert_eq!(std::mem::size_of::<OVert>(), 16);
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 16);
        assert_eq!(std::mem::size_of::<DgVert>(), 40);
    }
}
