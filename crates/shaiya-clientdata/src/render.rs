//! Pictures made from parsed client data, without a GUI: the terrain hillshade and the textured model picture. The SVMap Editor
//! and the toolkit's Archive preview both call this code, so a map and a model look the same in both.

use crate::anim::{locals, mat_mul, pose, Ani, Mat4};
use crate::mesh3dc::{C3dc, C3dcMesh};
use crate::smod::Smod;
use crate::texture::Image;
use crate::wld::{Terrain, CORNER_SPACING, HEIGHT_OFFSET, HEIGHT_SCALE};

/// A point or vector in model space (x, y, z).
pub type V3 = [f32; 3];

/// The neutral tint of [`hillshade_grey`].
pub const GREY_TINT: [u8; 3] = [200, 200, 200];

/// Colours the pixels of the corners in `rect` (`[cx0, cz0, cx1, cz1]`, inclusive) of an `n` x `n` hillshade picture from the
/// corner heights. A terrain edit redraws only the corners it touched (and their neighbours) with this; the first picture is the
/// same function over the whole grid, so a partial redraw gives the pixels a full one would. Light comes from the north-west;
/// row 0 of the picture is the north edge.
pub fn shade_rect(heights: &[i16], tex_index: &[u8], n: usize, tints: &[[u8; 3]], rect: [u32; 4], rgba: &mut [u8]) {
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

/// The hillshade of a whole terrain in one neutral grey tint: `(width, height, rgba)`, one pixel per corner, north up, opaque.
pub fn hillshade_grey(t: &Terrain) -> (usize, usize, Vec<u8>) {
    let n = t.n;
    let mut rgba = vec![255u8; n * n * 4];
    if n > 0 {
        shade_rect(&t.heights, &t.tex_index, n, &[GREY_TINT], [0, 0, n as u32 - 1, n as u32 - 1], &mut rgba);
    }
    (n, n, rgba)
}

// ------------------------------------------------------------------------------------------------------------------ images

/// Textures for the 3D view and the model picture are capped at this many texels a side (halved until they fit).
pub const MAX_TEXTURE: u32 = 256;

/// An RGBA image with its mip chain (`mips[0]` is the full size; every level is half the one before, down to 1 x 1).
pub struct MipImage {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

/// Averages 2 x 2 blocks (alpha included); odd edges reuse the last row / column.
pub fn half(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
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

// ------------------------------------------------------------------------------------------------------------------ model picture

/// Size of the model picture, points.
pub const PREVIEW_SIZE: [f32; 2] = [240.0, 220.0];
/// The picture is rendered at this many pixels per point (egui's linear filter then smooths the edges).
pub const PICTURE_SCALE: f32 = 2.0;
/// Animation frames are rendered at this many pixels per point (240 x 220 pixels).
pub const ANIM_PICTURE_SCALE: f32 = 1.0;
/// At most this many frames of an animation are rendered (every key unit of an 8-second emote at 30 key units per second).
pub const MAX_ANIM_FRAMES: usize = 241;
/// At most this many triangles are rendered and this many edges drawn as the wire.
pub const PREVIEW_TRIS: usize = 60_000;
pub const PREVIEW_EDGES: usize = 90_000;

/// A rendered model picture, RGBA (row 0 = top, alpha 0 where nothing was drawn), and what was drawn.
pub struct ModelPicture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Pixels that show a texel of a texture the client has.
    pub textured_px: usize,
    /// Pixels drawn plain grey (a module without a texture name, or a texture that could not be read).
    pub plain_px: usize,
}

/// The fixed view of the preview (35 degrees yaw, 25 degrees pitch, orthographic) fitted into [`PREVIEW_SIZE`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewFit {
    pub mid: [f32; 2],
    pub scale: f32,
}

/// Model space to view space: x right, y up, z away from the viewer, who looks down from above (a rotation, so nothing is
/// mirrored: the far side of the model is higher in the picture, and a higher point is nearer).
pub fn preview_view(v: V3) -> V3 {
    let (sy, cy) = 35f32.to_radians().sin_cos();
    let (sp, cp) = 25f32.to_radians().sin_cos();
    let x = v[0] * cy + v[2] * sy;
    let z = -v[0] * sy + v[2] * cy;
    [x, v[1] * cp + z * sp, -v[1] * sp + z * cp]
}

/// The extent of points in view space, grown point by point (non-finite points are skipped); [`ViewBounds::fit`] is [`PreviewFit::of`].
#[derive(Clone, Copy, Debug)]
pub struct ViewBounds {
    lo: [f32; 2],
    hi: [f32; 2],
}

impl Default for ViewBounds {
    fn default() -> Self {
        ViewBounds::new()
    }
}

impl ViewBounds {
    pub fn new() -> ViewBounds {
        ViewBounds { lo: [f32::MAX; 2], hi: [f32::MIN; 2] }
    }

    pub fn add(&mut self, p: V3) {
        let v = preview_view(p);
        if !(v[0].is_finite() && v[1].is_finite()) {
            return;
        }
        self.lo = [self.lo[0].min(v[0]), self.lo[1].min(v[1])];
        self.hi = [self.hi[0].max(v[0]), self.hi[1].max(v[1])];
    }

    pub fn fit(&self) -> Option<PreviewFit> {
        let (lo, hi) = (self.lo, self.hi);
        if lo[0] > hi[0] {
            return None;
        }
        let scale = ((PREVIEW_SIZE[0] - 16.0) / (hi[0] - lo[0]).max(0.01)).min((PREVIEW_SIZE[1] - 16.0) / (hi[1] - lo[1]).max(0.01));
        Some(PreviewFit { mid: [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5], scale })
    }
}

impl PreviewFit {
    /// The fit of model-space points into the picture with an 8 point margin; `None` without points.
    pub fn of(points: impl Iterator<Item = V3>) -> Option<PreviewFit> {
        let mut b = ViewBounds::new();
        for p in points {
            b.add(p);
        }
        b.fit()
    }

    /// A model-space point in picture points (origin top left), with its view depth.
    pub fn to_points(self, v: V3) -> [f32; 3] {
        let q = preview_view(v);
        [PREVIEW_SIZE[0] * 0.5 + (q[0] - self.mid[0]) * self.scale, PREVIEW_SIZE[1] * 0.5 - (q[1] - self.mid[1]) * self.scale, q[2]]
    }
}

/// One triangle to render: model-space corners and texture coordinates, and the index of its texture (`None`: plain grey).
pub struct PreviewTri {
    pub pos: [V3; 3],
    pub uv: [[f32; 2]; 3],
    pub tex: Option<usize>,
}

/// Renders triangles into a picture of [`PREVIEW_SIZE`] times [`PICTURE_SCALE`]: a depth buffer, the texel of the right mip
/// level (texture coordinates repeat, as on the GPU), texels with alpha under one half cut out like the 3D view does, and a
/// flat light from the upper left on both faces. The projection is orthographic, so texture coordinates interpolate linearly.
/// Then the wire: every edge, one pixel wide, blended over the surface where no nearer surface hides it (hidden lines left
/// out, so the wire marks the shape without covering the textures).
pub fn render_picture(tris: &[PreviewTri], textures: &[MipImage], edges: &[[V3; 2]], fit: PreviewFit) -> ModelPicture {
    render_picture_at(tris, textures, edges, fit, PICTURE_SCALE)
}

/// [`render_picture`] at `scale` pixels per point (the editor and the model preview use [`PICTURE_SCALE`]; animation frames use
/// [`ANIM_PICTURE_SCALE`]).
pub fn render_picture_at(tris: &[PreviewTri], textures: &[MipImage], edges: &[[V3; 2]], fit: PreviewFit, scale: f32) -> ModelPicture {
    let (w, h) = ((PREVIEW_SIZE[0] * scale) as usize, (PREVIEW_SIZE[1] * scale) as usize);
    let mut rgba = vec![0u8; w * h * 4];
    let mut depth = vec![f32::MAX; w * h];
    // what each pixel shows: 0 nothing, 1 texture, 2 plain
    let mut kind = vec![0u8; w * h];
    let light = {
        let l = [-0.4f32, 0.6, -0.7];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    for t in tris {
        let p = t.pos.map(|v| {
            let q = fit.to_points(v);
            [q[0] * scale, q[1] * scale, q[2]]
        });
        if p.iter().flatten().any(|c| !c.is_finite()) {
            continue;
        }
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area.abs() < 1e-6 {
            continue;
        }
        // the light: a face normal in view space, turned towards the viewer (-z)
        let v = t.pos.map(preview_view);
        let (a, b) = ([v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]], [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]]);
        let mut n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        if n[2] > 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        let lit = 0.5 + 0.6 * ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / nl).clamp(0.0, 1.0);
        // the mip level: texels per pixel over the triangle
        let tex = t.tex.and_then(|i| textures.get(i)).filter(|m| !m.mips.is_empty());
        let level = tex.map_or(0, |m| {
            let (u, q) = (t.uv, [m.width as f32, m.height as f32]);
            let uv_area = ((u[1][0] - u[0][0]) * (u[2][1] - u[0][1]) - (u[2][0] - u[0][0]) * (u[1][1] - u[0][1])).abs() * q[0] * q[1];
            let ratio = uv_area / area.abs();
            if ratio.is_finite() && ratio > 1.0 { ((0.5 * ratio.log2()) as usize).min(m.mips.len() - 1) } else { 0 }
        });
        let x0 = p.iter().map(|c| c[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let x1 = (p.iter().map(|c| c[0]).fold(f32::MIN, f32::max).ceil() as isize).min(w as isize - 1);
        let y0 = p.iter().map(|c| c[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let y1 = (p.iter().map(|c| c[1]).fold(f32::MIN, f32::max).ceil() as isize).min(h as isize - 1);
        if x1 < 0 || y1 < 0 {
            continue;
        }
        for y in y0..=y1 as usize {
            for x in x0..=x1 as usize {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = |i: usize, j: usize| (p[j][0] - p[i][0]) * (py - p[i][1]) - (p[j][1] - p[i][1]) * (px - p[i][0]);
                let (w0, w1, w2) = (e(1, 2) / area, e(2, 0) / area, e(0, 1) / area);
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                let at = y * w + x;
                if z >= depth[at] {
                    continue;
                }
                let (texel, k) = match tex {
                    Some(m) => {
                        let (lw, lh) = ((m.width >> level).max(1) as usize, (m.height >> level).max(1) as usize);
                        let u = w0 * t.uv[0][0] + w1 * t.uv[1][0] + w2 * t.uv[2][0];
                        let v = w0 * t.uv[0][1] + w1 * t.uv[1][1] + w2 * t.uv[2][1];
                        let tx = ((u.rem_euclid(1.0) * lw as f32) as usize).min(lw - 1);
                        let ty = ((v.rem_euclid(1.0) * lh as f32) as usize).min(lh - 1);
                        let s = &m.mips[level][(ty * lw + tx) * 4..][..4];
                        if s[3] < 128 {
                            continue;
                        }
                        ([s[0], s[1], s[2]], 1)
                    }
                    None => ([170, 170, 170], 2),
                };
                depth[at] = z;
                kind[at] = k;
                let o = &mut rgba[at * 4..][..4];
                for c in 0..3 {
                    o[c] = (texel[c] as f32 * lit).round().clamp(0.0, 255.0) as u8;
                }
                o[3] = 255;
            }
        }
    }
    let textured_px = kind.iter().filter(|k| **k == 1).count();
    let plain_px = kind.iter().filter(|k| **k == 2).count();
    // an edge lies on its own surface, so it may be a little behind the depth there: two points of depth
    let bias = 2.0 / fit.scale.max(1e-6);
    let wire = [170u8, 200, 230];
    let mut on_wire = vec![false; w * h];
    for e in edges {
        let (a, b) = (fit.to_points(e[0]), fit.to_points(e[1]));
        let (a, b) = ([a[0] * scale, a[1] * scale, a[2]], [b[0] * scale, b[1] * scale, b[2]]);
        if a.iter().chain(&b).any(|c| !c.is_finite()) {
            continue;
        }
        let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil().clamp(1.0, 4096.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                continue;
            }
            let at = y as usize * w + x as usize;
            if !on_wire[at] && a[2] + (b[2] - a[2]) * t <= depth[at] + bias {
                on_wire[at] = true;
            }
        }
    }
    for (at, _) in on_wire.iter().enumerate().filter(|(_, on)| **on) {
        let o = &mut rgba[at * 4..][..4];
        if o[3] == 0 {
            // over nothing (a see-through part, or a model that shows nothing): the wire alone, half strong
            o.copy_from_slice(&[wire[0], wire[1], wire[2], 140]);
        } else {
            for c in 0..3 {
                o[c] = ((o[c] as u32 * 3 + wire[c] as u32 * 2) / 5) as u8;
            }
        }
    }
    ModelPicture { width: w, height: h, rgba, textured_px, plain_px }
}

/// The Models window's and the Archive preview's picture of a model: the rendered picture, the model's visible edges for a
/// wire, and what could not be textured.
#[derive(Default)]
pub struct ModelPreview {
    /// Model-space edge pairs, at most [`PREVIEW_EDGES`].
    pub edges: Vec<[V3; 2]>,
    /// The rendered picture; `None` when the model has no faces (a dungeon piece) or none that name valid vertices.
    pub picture: Option<ModelPicture>,
    /// Textures the model names that could not be read (each once); their triangles are drawn plain grey.
    pub missing: Vec<String>,
    /// Textures with see-through texels (alpha under one half: leaves, fences, railings), which the picture cuts out.
    pub see_through: Vec<String>,
    /// Textures without a single opaque texel: the game draws nothing there (collision boxes), so neither does the picture.
    pub invisible: Vec<String>,
    /// Modules in the file.
    pub modules: usize,
    /// Triangles in the file (all modules).
    pub triangles: usize,
    /// Distinct texture names the modules give (read or missing).
    pub textures: usize,
}

/// The preview of a parsed model: every distinct texture name is asked of `tex` once (the trimmed name; `None` = it cannot be
/// read), at most [`PREVIEW_TRIS`] triangles are rendered and at most [`PREVIEW_EDGES`] edges drawn as the wire. A module without a
/// texture name, or whose texture is missing, is drawn plain grey.
pub fn model_picture(sm: &Smod, tex: &mut dyn FnMut(&str) -> Option<Image>) -> ModelPreview {
    let mut edges = Vec::new();
    let mut tris = Vec::new();
    let mut textures = Vec::new();
    let mut missing = Vec::new();
    let (mut see_through, mut invisible) = (Vec::new(), Vec::new());
    let mut tex_of: std::collections::HashMap<String, Option<usize>> = std::collections::HashMap::new();
    for m in &sm.modules {
        let name = m.texture.trim();
        let ti = if name.is_empty() {
            None
        } else {
            *tex_of.entry(name.to_ascii_lowercase()).or_insert_with(|| match tex(name) {
                Some(i) => {
                    let opaque = i.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128).count();
                    if opaque * 4 < i.rgba.len() {
                        see_through.push(name.to_string());
                    }
                    if opaque == 0 {
                        invisible.push(name.to_string());
                    }
                    textures.push(mip_image_capped(&i));
                    Some(textures.len() - 1)
                }
                None => {
                    missing.push(name.to_string());
                    None
                }
            })
        };
        for f in &m.faces {
            if tris.len() >= PREVIEW_TRIS {
                break;
            }
            if let (Some(a), Some(b), Some(c)) = (m.vertices.get(f[0] as usize), m.vertices.get(f[1] as usize), m.vertices.get(f[2] as usize)) {
                tris.push(PreviewTri { pos: [a.pos, b.pos, c.pos], uv: [a.uv, b.uv, c.uv], tex: ti });
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    'all: for (k, m) in sm.modules.iter().enumerate() {
        for f in &m.faces {
            for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
                let (Some(pa), Some(pb)) = (m.vertices.get(a as usize), m.vertices.get(b as usize)) else { continue };
                if seen.insert((a.min(b), a.max(b), k)) {
                    edges.push([pa.pos, pb.pos]);
                    if edges.len() >= PREVIEW_EDGES {
                        break 'all;
                    }
                }
            }
        }
    }
    let picture = if tris.is_empty() { None } else { PreviewFit::of(tris.iter().flat_map(|t| t.pos)).map(|fit| render_picture(&tris, &textures, &edges, fit)) };
    ModelPreview { edges, picture, missing, see_through, invisible, modules: sm.modules.len(), triangles: sm.modules.iter().map(|m| m.faces.len()).sum(), textures: tex_of.len() }
}

// ------------------------------------------------------------------------------------------------------------------ skinned meshes

/// Skins the vertices of a `.3DC` mesh with the pose `world` (`World[i]` of every bone, see `anim::pose`): `M = w * (B[b1] * World[b1]) +
/// (1 - w) * (B[b2] * World[b2])` (element-wise blend), `pos' = (pos, 1) * M` (x, y, z of the product, no divide), with `B` the mesh's own
/// matrices. A vertex naming a bone at or above `min(mesh bones, animation bones)` keeps its rest position and is counted (the second
/// number). An approximation of the game's playback, validated offline.
pub fn skin(mesh: &C3dcMesh, world: &[Mat4]) -> (Vec<V3>, usize) {
    let limit = mesh.bones.len().min(world.len());
    let mut unposed = 0;
    let mut out = Vec::with_capacity(mesh.vertices.len());
    // B[i] * World[i], once per bone
    let skin_matrix: Vec<Mat4> = (0..limit).map(|i| mat_mul(&mesh.bones[i], &world[i])).collect();
    for v in &mesh.vertices {
        let (b1, b2) = (v.bone1 as usize, v.bone2 as usize);
        if b1 >= limit || b2 >= limit {
            unposed += 1;
            out.push(v.pos);
            continue;
        }
        let (m1, m2) = (&skin_matrix[b1], &skin_matrix[b2]);
        let (w, rest) = (v.weight, 1.0 - v.weight);
        let p = v.pos;
        let mut o = [0f32; 3];
        for (c, oc) in o.iter_mut().enumerate() {
            let a = p[0] * m1[c] + p[1] * m1[4 + c] + p[2] * m1[8 + c] + m1[12 + c];
            let b = p[0] * m2[c] + p[1] * m2[4 + c] + p[2] * m2[8 + c] + m2[12 + c];
            *oc = w * a + rest * b;
        }
        out.push(o);
    }
    (out, unposed)
}

/// Faces of the meshes that name existing vertices, as (mesh, corners), at most [`PREVIEW_TRIS`] over all meshes.
fn mesh_faces(meshes: &[&C3dcMesh]) -> Vec<(usize, [usize; 3])> {
    let mut out = Vec::new();
    'all: for (k, m) in meshes.iter().enumerate() {
        for f in &m.faces {
            let c = f.map(|i| i as usize);
            if c.iter().all(|&i| i < m.vertices.len()) {
                if out.len() >= PREVIEW_TRIS {
                    break 'all;
                }
                out.push((k, c));
            }
        }
    }
    out
}

/// Edges of `faces` as (mesh, vertex, vertex), each once, at most [`PREVIEW_EDGES`].
fn mesh_edges(faces: &[(usize, [usize; 3])]) -> Vec<(usize, usize, usize)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    'all: for &(k, c) in faces {
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if seen.insert((k, a.min(b), a.max(b))) {
                out.push((k, a, b));
                if out.len() >= PREVIEW_EDGES {
                    break 'all;
                }
            }
        }
    }
    out
}

fn tris_of(faces: &[(usize, [usize; 3])], meshes: &[&C3dcMesh], pos: &[Vec<V3>]) -> Vec<PreviewTri> {
    faces.iter().map(|&(k, c)| PreviewTri { pos: c.map(|i| pos[k][i]), uv: c.map(|i| meshes[k].vertices[i].uv), tex: None }).collect()
}

fn edges_of(edges: &[(usize, usize, usize)], pos: &[Vec<V3>]) -> Vec<[V3; 2]> {
    edges.iter().map(|&(k, a, b)| [pos[k][a], pos[k][b]]).collect()
}

/// The first mesh of a `.3DC` at rest (its raw vertex positions) as a plain grey picture with the wire, at [`PICTURE_SCALE`]; `None` when
/// the file has no mesh or no face that names existing vertices.
pub fn c3dc_picture(c: &C3dc) -> Option<ModelPicture> {
    let mesh = c.meshes.first()?;
    let meshes = [mesh];
    let faces = mesh_faces(&meshes);
    if faces.is_empty() {
        return None;
    }
    let pos = vec![mesh.vertices.iter().map(|v| v.pos).collect::<Vec<V3>>()];
    let tris = tris_of(&faces, &meshes, &pos);
    let edges = edges_of(&mesh_edges(&faces), &pos);
    let fit = PreviewFit::of(tris.iter().flat_map(|t| t.pos))?;
    Some(render_picture(&tris, &[], &edges, fit))
}

/// The pre-rendered frames of an animation: one picture per `step` key units, the same view in every frame.
#[derive(Clone, Debug)]
pub struct AnimFrames {
    /// Key units between two frames; frame `i` is the pose at `i * step`.
    pub step: f32,
    /// Size of a frame in pixels ([`PREVIEW_SIZE`] times [`ANIM_PICTURE_SCALE`]).
    pub width: usize,
    pub height: usize,
    /// RGBA pictures, one per frame; empty when there is no mesh (or no face to draw).
    pub frames: Vec<Vec<u8>>,
    /// Per frame, the picture points (origin top left, the size of [`PREVIEW_SIZE`]) of every bone origin; NaN for a bone whose pose is not finite.
    pub bones: Vec<Vec<[f32; 2]>>,
    pub parents: Vec<i32>,
    /// Vertices that name a bone the animation (or the mesh) lacks and so stay at their rest position.
    pub unposed_vertices: usize,
    /// Bones whose `Local` matrix holds NaN or infinity (a singular or non-finite reference matrix).
    pub nonfinite_bones: usize,
    /// Triangles drawn in each frame.
    pub triangles: usize,
}

/// Pre-renders an animation on `meshes`: times `0, step, 2 step, ...` up to the end key with `step = max(1, ceil((end_key + 1) /
/// MAX_ANIM_FRAMES))`; an end key of 0 or less gives one frame at 0. Pass 1 poses and skins every frame and fits one view over all the
/// finite points (vertices and bone origins), so the view does not move; pass 2 renders each frame at [`ANIM_PICTURE_SCALE`] and projects
/// the bone origins. Without meshes there are no pictures, only bones (the view fits the bone origins). `cancelled` is asked once per frame
/// in both passes; `None` when it said yes.
pub fn anim_frames(ani: &Ani, meshes: &[&C3dcMesh], cancelled: &mut dyn FnMut() -> bool) -> Option<AnimFrames> {
    let end = i64::from(ani.end_key.max(0));
    let max_frames = MAX_ANIM_FRAMES as i64;
    let step = ((end + max_frames) / max_frames).max(1);
    let count = (end / step + 1) as usize;
    let time = |i: usize| (i as i64 * step) as f32;
    let local = locals(ani);
    let nonfinite_bones = local.iter().filter(|m| m.iter().any(|c| !c.is_finite())).count();
    let faces = mesh_faces(meshes);
    let edges = mesh_edges(&faces);
    let skin_all = |world: &[Mat4]| -> (Vec<Vec<V3>>, usize) {
        let mut unposed = 0;
        let pos = meshes
            .iter()
            .map(|m| {
                let (p, u) = skin(m, world);
                unposed += u;
                p
            })
            .collect();
        (pos, unposed)
    };

    // pass 1: the view over every frame
    let mut bounds = ViewBounds::new();
    let mut unposed_vertices = 0;
    for i in 0..count {
        if cancelled() {
            return None;
        }
        let world = pose(ani, &local, time(i));
        for w in &world {
            bounds.add([w[12], w[13], w[14]]);
        }
        let (pos, unposed) = skin_all(&world);
        if i == 0 {
            unposed_vertices = unposed;
        }
        for v in pos.iter().flatten() {
            bounds.add(*v);
        }
    }
    let fit = bounds.fit().unwrap_or(PreviewFit { mid: [0.0, 0.0], scale: 1.0 });

    // pass 2: the pictures and the bone points
    let mut frames = Vec::new();
    let mut bones = Vec::with_capacity(count);
    let (mut width, mut height) = ((PREVIEW_SIZE[0] * ANIM_PICTURE_SCALE) as usize, (PREVIEW_SIZE[1] * ANIM_PICTURE_SCALE) as usize);
    for i in 0..count {
        if cancelled() {
            return None;
        }
        let world = pose(ani, &local, time(i));
        bones.push(
            world
                .iter()
                .map(|w| {
                    let p = fit.to_points([w[12], w[13], w[14]]);
                    if p[0].is_finite() && p[1].is_finite() {
                        [p[0], p[1]]
                    } else {
                        [f32::NAN; 2]
                    }
                })
                .collect(),
        );
        if !faces.is_empty() {
            let (pos, _) = skin_all(&world);
            let pic = render_picture_at(&tris_of(&faces, meshes, &pos), &[], &edges_of(&edges, &pos), fit, ANIM_PICTURE_SCALE);
            (width, height) = (pic.width, pic.height);
            frames.push(pic.rgba);
        }
    }
    Some(AnimFrames { step: step as f32, width, height, frames, bones, parents: ani.bones.iter().map(|b| b.parent).collect(), unposed_vertices, nonfinite_bones, triangles: faces.len() })
}

// ------------------------------------------------------------------------------------------------------------------ vertex-frame models

/// The rest picture of one mesh given as vertices and faces (plain grey with the wire), at [`PICTURE_SCALE`]; `None` when no face names
/// existing vertices. Used for the `.3DO` and `.3DE` previews.
pub fn plain_mesh_picture(pos: &[V3], uv: &[[f32; 2]], faces: &[[u16; 3]]) -> Option<ModelPicture> {
    let n = pos.len().min(uv.len());
    let valid: Vec<[usize; 3]> = faces.iter().map(|f| f.map(|i| i as usize)).filter(|c| c.iter().all(|&i| i < n)).take(PREVIEW_TRIS).collect();
    if valid.is_empty() {
        return None;
    }
    let tris: Vec<PreviewTri> = valid.iter().map(|c| PreviewTri { pos: c.map(|i| pos[i]), uv: c.map(|i| uv[i]), tex: None }).collect();
    let tagged: Vec<(usize, [usize; 3])> = valid.into_iter().map(|c| (0, c)).collect();
    let edges: Vec<[V3; 2]> = mesh_edges(&tagged).into_iter().map(|(_, a, b)| [pos[a], pos[b]]).collect();
    let fit = PreviewFit::of(tris.iter().flat_map(|t| t.pos))?;
    Some(render_picture(&tris, &[], &edges, fit))
}

/// One item of a model that is drawn from vertex positions that change from frame to frame: the texture name (may be empty), the faces
/// (shared by every frame) and the texture coordinates of the item's vertices.
pub struct MorphItem<'a> {
    pub texture: String,
    pub faces: &'a [[u16; 3]],
    pub uv: Vec<[f32; 2]>,
}

/// What [`morph_frames`] made.
pub struct MorphFrames {
    /// `bones` holds one empty list per frame (there is no skeleton); `step` is the number of source frames between two pictures.
    pub frames: AnimFrames,
    /// Textures the items name that could not be read (each once); their triangles are drawn plain grey.
    pub missing: Vec<String>,
}

/// Pre-renders a model whose vertex positions are given per frame (`.VANI` steps, `.3DO` frames): `count` source frames, one picture per
/// `step` of them with `step = max(1, ceil(count / MAX_ANIM_FRAMES))`, so at most [`MAX_ANIM_FRAMES`] pictures. `pos(item, frame)` gives the
/// positions of an item's vertices in a source frame. Every distinct texture name is asked of `tex` once (`None`: it cannot be read, the
/// triangles are drawn grey). Pass 1 fits one view over every picture's points, pass 2 renders; `cancelled` is asked once per picture in both
/// passes and the result is `None` when it said yes. No pictures when no face names a vertex.
pub fn morph_frames(items: &[MorphItem], count: usize, pos: &mut dyn FnMut(usize, usize) -> Vec<V3>, tex: &mut dyn FnMut(&str) -> Option<Image>, cancelled: &mut dyn FnMut() -> bool) -> Option<MorphFrames> {
    let step = count.div_ceil(MAX_ANIM_FRAMES).max(1);
    let shown = if count == 0 { 0 } else { (count - 1) / step + 1 };
    // textures, once per distinct name
    let mut textures = Vec::new();
    let mut missing = Vec::new();
    let mut tex_of: std::collections::HashMap<String, Option<usize>> = std::collections::HashMap::new();
    let item_tex: Vec<Option<usize>> = items
        .iter()
        .map(|it| {
            let name = it.texture.trim();
            if name.is_empty() {
                return None;
            }
            *tex_of.entry(name.to_ascii_lowercase()).or_insert_with(|| match tex(name) {
                Some(i) => {
                    textures.push(mip_image_capped(&i));
                    Some(textures.len() - 1)
                }
                None => {
                    missing.push(name.to_string());
                    None
                }
            })
        })
        .collect();
    // faces that name existing vertices, at most PREVIEW_TRIS over all items; edges once each
    let mut faces: Vec<(usize, [usize; 3])> = Vec::new();
    'all: for (k, it) in items.iter().enumerate() {
        for f in it.faces {
            let c = f.map(|i| i as usize);
            if c.iter().all(|&i| i < it.uv.len()) {
                if faces.len() >= PREVIEW_TRIS {
                    break 'all;
                }
                faces.push((k, c));
            }
        }
    }
    let edges = mesh_edges(&faces);
    let source = |i: usize| i * step;
    let positions = |pos: &mut dyn FnMut(usize, usize) -> Vec<V3>, i: usize| -> Vec<Vec<V3>> { (0..items.len()).map(|k| pos(k, source(i))).collect() };

    let mut bounds = ViewBounds::new();
    for i in 0..shown {
        if cancelled() {
            return None;
        }
        for p in positions(pos, i).iter().flatten() {
            bounds.add(*p);
        }
    }
    let fit = bounds.fit().unwrap_or(PreviewFit { mid: [0.0, 0.0], scale: 1.0 });

    let mut frames = Vec::new();
    let (mut width, mut height) = ((PREVIEW_SIZE[0] * ANIM_PICTURE_SCALE) as usize, (PREVIEW_SIZE[1] * ANIM_PICTURE_SCALE) as usize);
    for i in 0..shown {
        if cancelled() {
            return None;
        }
        if faces.is_empty() {
            continue;
        }
        let p = positions(pos, i);
        let tris: Vec<PreviewTri> = faces
            .iter()
            .map(|&(k, c)| PreviewTri { pos: c.map(|v| p[k].get(v).copied().unwrap_or([0.0; 3])), uv: c.map(|v| items[k].uv[v]), tex: item_tex[k] })
            .collect();
        let wire: Vec<[V3; 2]> = edges.iter().filter_map(|&(k, a, b)| Some([*p[k].get(a)?, *p[k].get(b)?])).collect();
        let pic = render_picture_at(&tris, &textures, &wire, fit, ANIM_PICTURE_SCALE);
        (width, height) = (pic.width, pic.height);
        frames.push(pic.rgba);
    }
    let anim = AnimFrames { step: step as f32, width, height, frames, bones: vec![Vec::new(); shown], parents: Vec::new(), unposed_vertices: 0, nonfinite_bones: 0, triangles: faces.len() };
    Some(MorphFrames { frames: anim, missing })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smod::{SmodModule, SmodVertex};

    fn terrain(n: usize, f: impl Fn(usize, usize) -> i16) -> Terrain {
        Terrain { size: (n as u32 - 1) * 2, n, heights: (0..n * n).map(|i| f(i % n, i / n)).collect(), tex_index: vec![0; n * n] }
    }

    #[test]
    fn a_slope_facing_north_west_is_brighter_than_one_facing_south_east() {
        // z grows to the north: height falls towards west and north, so the surface faces north-west
        let nw = terrain(9, |x, z| 9000 + 40 * x as i16 - 40 * z as i16);
        let se = terrain(9, |x, z| 9000 - 40 * x as i16 + 40 * z as i16);
        let (_, _, a) = hillshade_grey(&nw);
        let (_, _, b) = hillshade_grey(&se);
        let mid = (4 * 9 + 4) * 4;
        assert!(a[mid] > b[mid], "{} vs {}", a[mid], b[mid]);
    }

    #[test]
    fn hillshade_grey_is_one_opaque_pixel_per_corner() {
        let t = terrain(5, |_, _| 9000);
        let (w, h, px) = hillshade_grey(&t);
        assert_eq!((w, h, px.len()), (5, 5, 5 * 5 * 4));
        assert!(px.chunks(4).all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]));
    }

    #[test]
    fn mips_halve_to_one_pixel_and_big_images_are_capped() {
        let rgba: Vec<u8> = (0..8 * 4 * 4).map(|i| (i % 251) as u8).collect();
        let sizes: Vec<usize> = mip_chain(8, 4, rgba).iter().map(|l| l.len() / 4).collect();
        assert_eq!(sizes, vec![32, 8, 2, 1]);
        let m = mip_image_capped(&Image { width: 1024, height: 512, rgba: vec![200; 1024 * 512 * 4] });
        assert_eq!((m.width, m.height, m.mips.len()), (256, 128, 9));
        assert!(m.mips.iter().all(|l| l.iter().all(|v| *v == 200)));
    }

    /// Two vertices-and-faces quads of a box face: `corners` in order around the face.
    fn face(m: &mut SmodModule, corners: [V3; 4]) {
        let base = m.vertices.len() as u16;
        for (c, uv) in corners.iter().zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
            m.vertices.push(SmodVertex { pos: *c, normal: [0.0, 1.0, 0.0], colour: 0xFFFF_FFFF, uv });
        }
        m.faces.push([base, base + 1, base + 2]);
        m.faces.push([base, base + 2, base + 3]);
    }

    /// A 4 x 4 x 4 box, as three modules: the top and the two sides facing along x with the texture "wall.tga", the two sides
    /// along z and the bottom with a texture that does not exist, and one extra triangle without a texture name.
    fn box_smod() -> Smod {
        let module = |texture: &str| SmodModule { texture: texture.to_string(), vertices: Vec::new(), faces: Vec::new() };
        let (mut a, mut b, mut c) = (module(" wall.tga "), module("gone.tga"), module(""));
        face(&mut a, [[0.0, 4.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0], [0.0, 4.0, 4.0]]);
        face(&mut a, [[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [0.0, 4.0, 4.0], [0.0, 4.0, 0.0]]);
        face(&mut a, [[4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0], [4.0, 0.0, 4.0]]);
        face(&mut b, [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 4.0, 0.0]]);
        face(&mut b, [[0.0, 0.0, 4.0], [0.0, 4.0, 4.0], [4.0, 4.0, 4.0], [4.0, 0.0, 4.0]]);
        face(&mut b, [[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, 0.0]]);
        c.vertices.extend([[1.0f32, 1.0, 1.0], [2.0, 1.0, 1.0], [1.0, 2.0, 1.0]].map(|pos| SmodVertex { pos, normal: [0.0; 3], colour: 0xFFFF_FFFF, uv: [0.0; 2] }));
        c.faces.push([0, 1, 2]);
        Smod { centre: [2.0; 3], min: [0.0; 3], max: [4.0; 3], modules: vec![a, b, c], box2: ([0.0; 3], [4.0; 3]), collision: Vec::new() }
    }

    #[test]
    fn a_box_model_gets_a_textured_picture_and_names_its_missing_texture() {
        let sm = box_smod();
        let mut asked = Vec::new();
        let p = model_picture(&sm, &mut |n| {
            asked.push(n.to_string());
            (n == "wall.tga").then(|| Image { width: 2, height: 2, rgba: [200, 50, 50, 255].repeat(4) })
        });
        assert_eq!(asked, ["wall.tga", "gone.tga"], "each name once, trimmed, and never the empty name");
        assert_eq!((p.modules, p.triangles, p.textures), (3, 13, 2));
        assert_eq!((p.missing.as_slice(), p.see_through.len(), p.invisible.len()), (&["gone.tga".to_string()][..], 0, 0));
        assert_eq!(p.edges.len(), 5 * 3 + 5 * 3 + 3, "four sides and a diagonal per quad, three for the lone triangle");
        let pic = p.picture.expect("a picture");
        assert_eq!((pic.width, pic.height), ((PREVIEW_SIZE[0] * PICTURE_SCALE) as usize, (PREVIEW_SIZE[1] * PICTURE_SCALE) as usize));
        assert!(pic.textured_px > 500, "the textured sides show: {}", pic.textured_px);
        assert!(pic.plain_px > 500, "the missing texture is plain grey: {}", pic.plain_px);
        assert!(pic.rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255 && px[0] > px[2]), "the texture's red shows");
    }

    #[test]
    fn a_see_through_and_an_invisible_texture_are_listed() {
        let sm = box_smod();
        let p = model_picture(&sm, &mut |n| match n {
            "wall.tga" => Some(Image { width: 2, height: 1, rgba: vec![9, 9, 9, 255, 9, 9, 9, 0] }),
            _ => Some(Image { width: 1, height: 1, rgba: vec![1, 1, 1, 0] }),
        });
        assert_eq!((p.see_through, p.invisible, p.missing.len()), (vec!["wall.tga".to_string(), "gone.tga".to_string()], vec!["gone.tga".to_string()], 0));
    }

    #[test]
    fn a_model_without_faces_has_no_picture() {
        let mut sm = box_smod();
        for m in &mut sm.modules {
            m.faces.clear();
        }
        let p = model_picture(&sm, &mut |_| None);
        assert!(p.picture.is_none() && p.edges.is_empty());
        assert_eq!((p.modules, p.triangles), (3, 0));
    }

    // ---- poses, skinning and frames -----------------------------------------------------------------------------------------------

    use crate::anim::IDENTITY;
    use crate::mesh3dc::C3dcVertex;
    use shaiya_testkit::anim::{ani_bytes, arm_ani, arm_mesh_3dc, identity_bits, translation_bits, BoneSpec};

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn spec(parent: i32, rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>) -> BoneSpec {
        BoneSpec { parent, matrix: identity_bits(), rot, trans }
    }

    fn vertex(pos: V3, weight: f32, bone1: u8, bone2: u8) -> C3dcVertex {
        C3dcVertex { pos, weight, bone1, bone2, pad: 0, normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }
    }

    fn mesh(bones: usize, vertices: Vec<C3dcVertex>) -> C3dcMesh {
        C3dcMesh { texture: Vec::new(), bones: vec![IDENTITY; bones], vertices, faces: Vec::new() }
    }

    fn arm() -> (Ani, C3dc) {
        (Ani::parse(&arm_ani(60), "arm.ani").unwrap(), C3dc::parse(&arm_mesh_3dc(), "arm.3dc").unwrap())
    }

    fn drawn(rgba: &[u8]) -> usize {
        rgba.chunks(4).filter(|p| p[3] == 255).count()
    }

    #[test]
    fn a_root_rotation_moves_a_skinned_vertex() {
        let (s, c) = 45f32.to_radians().sin_cos();
        let ani = Ani::parse(&ani_bytes(0, 10, &[spec(-1, vec![(0, [0.0, 0.0, 0.0, 1.0]), (10, [0.0, s, 0.0, c])], Vec::new())]), "t.ani").unwrap();
        let m = mesh(1, vec![vertex([1.0, 0.0, 0.0], 1.0, 0, 0)]);
        let (p, unposed) = skin(&m, &pose(&ani, &locals(&ani), 10.0));
        assert_eq!(unposed, 0);
        assert!(near(p[0][0], 0.0, 1e-5) && near(p[0][1], 0.0, 1e-5) && near(p[0][2], -1.0, 1e-5), "{:?}", p[0]);
    }

    #[test]
    fn a_half_weight_vertex_blends_two_bones() {
        let bones = [spec(-1, Vec::new(), vec![(0, [2.0, 0.0, 0.0])]), spec(-1, Vec::new(), vec![(0, [0.0, 4.0, 0.0])])];
        let ani = Ani::parse(&ani_bytes(0, 10, &bones), "t.ani").unwrap();
        let m = mesh(2, vec![vertex([0.0; 3], 0.5, 0, 1), vertex([0.0; 3], 0.25, 0, 1)]);
        let (p, _) = skin(&m, &pose(&ani, &locals(&ani), 0.0));
        assert!(near(p[0][0], 1.0, 1e-6) && near(p[0][1], 2.0, 1e-6), "{:?}", p[0]);
        assert!(near(p[1][0], 0.5, 1e-6) && near(p[1][1], 3.0, 1e-6), "{:?}", p[1]);
    }

    #[test]
    fn a_vertex_with_a_bone_the_animation_lacks_stays_unposed() {
        let ani = Ani::parse(&ani_bytes(0, 10, &[spec(-1, Vec::new(), vec![(0, [5.0, 0.0, 0.0])])]), "t.ani").unwrap();
        let m = mesh(4, vec![vertex([1.0, 2.0, 3.0], 1.0, 0, 0), vertex([1.0, 2.0, 3.0], 0.5, 0, 3), vertex([7.0, 8.0, 9.0], 1.0, 2, 2)]);
        let (p, unposed) = skin(&m, &pose(&ani, &locals(&ani), 0.0));
        assert_eq!(unposed, 2);
        assert_eq!(p[0], [6.0, 2.0, 3.0]);
        assert_eq!((p[1], p[2]), ([1.0, 2.0, 3.0], [7.0, 8.0, 9.0]));
    }

    #[test]
    fn a_nan_reference_matrix_does_not_panic_and_other_bones_draw() {
        // the arm's three bones sit at y = 0, 1, 2; bone 2 gets a NaN
        let mut bad = translation_bits(0.0, 2.0, 0.0);
        bad[13] = 0xFFC0_0000;
        let mut bones = vec![spec(-1, Vec::new(), Vec::new()), spec(0, Vec::new(), Vec::new()), spec(1, Vec::new(), Vec::new())];
        bones[1].matrix = translation_bits(0.0, 1.0, 0.0);
        bones[2].matrix = bad;
        let ani = Ani::parse(&ani_bytes(0, 20, &bones), "t.ani").unwrap();
        let c = C3dc::parse(&arm_mesh_3dc(), "arm.3dc").unwrap();
        let f = anim_frames(&ani, &[&c.meshes[0]], &mut || false).unwrap();
        assert_eq!(f.nonfinite_bones, 1);
        assert!(f.frames.iter().all(|px| drawn(px) > 0), "the first box does not use bone 2");
        assert!(f.bones.iter().all(|b| b[2][0].is_nan() && b[0][0].is_finite()));
    }

    #[test]
    fn frame_step_and_count_follow_the_end_key() {
        for (end, step, count) in [(240, 1.0, 241), (1000, 5.0, 201), (0, 1.0, 1), (-1, 1.0, 1), (60, 1.0, 61), (241, 2.0, 121)] {
            let ani = Ani::parse(&arm_ani(end), "t.ani").unwrap();
            let f = anim_frames(&ani, &[], &mut || false).unwrap();
            assert_eq!((f.step, f.bones.len()), (step, count), "end key {end}");
            assert!(f.frames.is_empty(), "no mesh, no pictures");
        }
    }

    #[test]
    fn the_view_is_the_same_for_every_frame() {
        let (ani, c) = arm();
        let f = anim_frames(&ani, &[&c.meshes[0]], &mut || false).unwrap();
        assert_eq!(f.frames.len(), 61);
        // the root never moves, so it projects to the same point in every frame (one fit for all of them)
        assert!(f.bones.iter().all(|b| b[0] == f.bones[0][0]));
        // the tip of the arm does move
        assert!(f.bones[0][2] != f.bones[60][2]);
        assert_eq!((f.width, f.height), (240, 220));
    }

    #[test]
    fn cancel_stops_frame_rendering() {
        let (ani, c) = arm();
        let mut asked = 0;
        assert!(anim_frames(&ani, &[&c.meshes[0]], &mut || {
            asked += 1;
            asked >= 3
        })
        .is_none());
        assert_eq!(asked, 3, "stops at the first yes (pass 1)");
        let mut asked = 0;
        assert!(anim_frames(&ani, &[&c.meshes[0]], &mut || {
            asked += 1;
            asked > 61 + 5
        })
        .is_none());
        assert_eq!(asked, 61 + 6, "stops in pass 2 after five frames");
        assert!(anim_frames(&ani, &[&c.meshes[0]], &mut || false).is_some());
    }

    #[test]
    fn render_picture_at_2_is_render_picture() {
        let (_, c) = arm();
        let m = &c.meshes[0];
        let faces = mesh_faces(&[m]);
        let pos = vec![m.vertices.iter().map(|v| v.pos).collect::<Vec<V3>>()];
        let tris = tris_of(&faces, &[m], &pos);
        let edges = edges_of(&mesh_edges(&faces), &pos);
        let fit = PreviewFit::of(tris.iter().flat_map(|t| t.pos)).unwrap();
        let (a, b) = (render_picture(&tris, &[], &edges, fit), render_picture_at(&tris, &[], &edges, fit, 2.0));
        assert_eq!((a.width, a.height), (480, 440));
        assert!(a.rgba == b.rgba && (a.plain_px, a.textured_px) == (b.plain_px, b.textured_px));
        let one = render_picture_at(&tris, &[], &edges, fit, ANIM_PICTURE_SCALE);
        assert_eq!((one.width, one.height), (240, 220));
    }

    #[test]
    fn c3dc_picture_draws_the_rest_pose_grey() {
        let (_, c) = arm();
        let p = c3dc_picture(&c).unwrap();
        assert_eq!((p.width, p.height), (480, 440));
        assert!(p.plain_px > 100 && p.textured_px == 0, "{} {}", p.plain_px, p.textured_px);
        let empty = C3dc { layout: c.layout, meshes: Vec::new() };
        assert!(c3dc_picture(&empty).is_none());
    }

    /// The vertices the recorded table names: three of the first box and five of the second.
    const NAMED: [usize; 8] = [1, 3, 7, 9, 10, 11, 14, 15];

    #[test]
    fn the_arm_poses_match_the_recorded_numbers() {
        // Recorded from the code at first build (rounded to 1e-3), not derived by hand; compared within 2e-3 so that a one-ulp
        // difference of a platform's sin or acos cannot fail it. Rows: t = 0, 10, 20, 30, 40, 50, 60; columns: the NAMED vertices.
        const RECORDED: [[[f32; 3]; 8]; 7] = [
            [[0.100, 0.000, -0.100], [0.100, 1.000, -0.100], [0.100, 1.000, 0.100], [0.100, 1.000, -0.100], [-0.100, 2.000, -0.100], [0.100, 2.000, -0.100], [-0.100, 2.000, 0.100], [0.100, 2.000, 0.100]],
            [[0.100, 0.000, -0.100], [0.098, 1.013, -0.100], [0.098, 1.013, 0.100], [0.097, 1.026, -0.100], [-0.355, 1.940, -0.100], [-0.162, 1.992, -0.100], [-0.355, 1.940, 0.100], [-0.162, 1.992, 0.100]],
            [[0.100, 0.000, -0.100], [0.093, 1.025, -0.100], [0.093, 1.025, 0.100], [0.087, 1.050, -0.100], [-0.587, 1.816, -0.100], [-0.413, 1.916, -0.100], [-0.587, 1.816, 0.100], [-0.413, 1.916, 0.100]],
            [[0.100, 0.000, -0.100], [0.085, 1.035, -0.100], [0.085, 1.035, 0.100], [0.071, 1.071, -0.100], [-0.778, 1.636, -0.100], [-0.636, 1.778, -0.100], [-0.778, 1.636, 0.100], [-0.636, 1.778, 0.100]],
            [[0.100, 0.000, -0.100], [0.075, 1.043, -0.100], [0.075, 1.043, 0.100], [0.050, 1.087, -0.100], [-0.916, 1.413, -0.100], [-0.816, 1.587, -0.100], [-0.916, 1.413, 0.100], [-0.816, 1.587, 0.100]],
            [[0.100, 0.000, -0.100], [0.063, 1.048, -0.100], [0.063, 1.048, 0.100], [0.026, 1.097, -0.100], [-0.992, 1.162, -0.100], [-0.940, 1.355, -0.100], [-0.992, 1.162, 0.100], [-0.940, 1.355, 0.100]],
            [[0.100, 0.000, -0.100], [0.050, 1.050, -0.100], [0.050, 1.050, 0.100], [0.000, 1.100, -0.100], [-1.000, 0.900, -0.100], [-1.000, 1.100, -0.100], [-1.000, 0.900, 0.100], [-1.000, 1.100, 0.100]],
        ];
        let (ani, c) = arm();
        let local = locals(&ani);
        for (row, t) in RECORDED.iter().zip([0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0]) {
            let (p, unposed) = skin(&c.meshes[0], &pose(&ani, &local, t));
            assert_eq!(unposed, 0);
            for (want, &i) in row.iter().zip(&NAMED) {
                for k in 0..3 {
                    assert!(near(p[i][k], want[k], 2e-3), "t {t}, vertex {i}, axis {k}: {} vs {}", p[i][k], want[k]);
                }
            }
        }
    }

    #[test]
    fn the_arm_frames_draw_about_the_recorded_pixel_counts() {
        // Recorded from the code at first build, not derived: the number of opaque pixels of each of the 61 frames, compared within 1 %.
        // No pixel hash: a hash would depend on the platform's libm.
        const DRAWN: [usize; 61] = [
            5898, 5848, 5866, 5886, 5903, 5923, 5930, 5924, 5921, 5925,
            5938, 5936, 5939, 5933, 5928, 5922, 5924, 5919, 5919, 5919,
            5914, 5912, 5891, 5902, 5896, 5890, 5884, 5886, 5880, 5876,
            5866, 5852, 5841, 5825, 5814, 5798, 5789, 5774, 5760, 5743,
            5724, 5705, 5678, 5650, 5628, 5604, 5579, 5554, 5526, 5500,
            5470, 5438, 5404, 5358, 5339, 5306, 5268, 5231, 5197, 5158,
            5119,
        ];
        let (ani, c) = arm();
        let f = anim_frames(&ani, &[&c.meshes[0]], &mut || false).unwrap();
        assert_eq!(f.frames.len(), 61);
        for (i, px) in f.frames.iter().enumerate() {
            let n = drawn(px);
            assert!(n >= 1, "frame {i} draws something");
            let tol = (DRAWN[i] as f32 * 0.01).ceil() as usize;
            assert!(n.abs_diff(DRAWN[i]) <= tol, "frame {i}: {n} drawn, recorded {}", DRAWN[i]);
        }
        assert!(f.frames[0] != f.frames[60], "the arm moves");
        assert_eq!((f.unposed_vertices, f.nonfinite_bones, f.triangles), (0, 0, 24));
    }

    fn box_vani_items(v: &crate::vani::Vani) -> Vec<MorphItem<'_>> {
        v.items.iter().map(|it| MorphItem { texture: String::new(), faces: &it.faces, uv: v.step_vertices(0, 0).iter().map(|x| x.uv).collect() }).collect()
    }

    #[test]
    fn morph_frames_step_and_count_follow_the_source_frames() {
        let v = crate::vani::Vani::parse(&shaiya_testkit::anim::box_vani(5, 33), "b.vani").unwrap();
        let items = box_vani_items(&v);
        for (count, step, shown) in [(0usize, 1.0f32, 0usize), (1, 1.0, 1), (5, 1.0, 5), (241, 1.0, 241), (242, 2.0, 121), (500, 3.0, 167)] {
            let m = morph_frames(&items, count, &mut |_, f| v.step_vertices(0, f.min(4)).iter().map(|x| x.pos).collect(), &mut |_| None, &mut || false).unwrap();
            assert_eq!((m.frames.step, m.frames.bones.len(), m.frames.frames.len()), (step, shown, shown), "count {count}");
            assert!(m.frames.frames.len() <= MAX_ANIM_FRAMES);
        }
    }

    #[test]
    fn morph_frames_draw_one_view_and_cancel_in_both_passes() {
        let v = crate::vani::Vani::parse(&shaiya_testkit::anim::box_vani(5, 33), "b.vani").unwrap();
        let items = box_vani_items(&v);
        let mut pos = |_: usize, f: usize| -> Vec<V3> { v.step_vertices(0, f).iter().map(|x| x.pos).collect() };
        let m = morph_frames(&items, 5, &mut pos, &mut |_| None, &mut || false).unwrap();
        assert_eq!((m.frames.width, m.frames.height), (240, 220));
        assert!(m.frames.frames.iter().all(|f| drawn(f) > 0));
        assert!(m.frames.frames[0] != m.frames.frames[4], "the box rises");
        assert_eq!(m.frames.triangles, 12);
        let mut asked = 0;
        assert!(morph_frames(&items, 5, &mut pos, &mut |_| None, &mut || {
            asked += 1;
            asked >= 2
        })
        .is_none());
        assert_eq!(asked, 2);
        let mut asked = 0;
        assert!(morph_frames(&items, 5, &mut pos, &mut |_| None, &mut || {
            asked += 1;
            asked > 5 + 2
        })
        .is_none());
        assert_eq!(asked, 5 + 3, "stops in pass 2 after two pictures");
    }

    #[test]
    fn morph_frames_ask_for_each_texture_once_and_name_the_missing_one() {
        let v = crate::vani::Vani::parse(&shaiya_testkit::anim::box_vani(2, 33), "b.vani").unwrap();
        let mut items = box_vani_items(&v);
        items.push(MorphItem { texture: "BOX.TGA".to_string(), faces: &v.items[0].faces, uv: items[0].uv.clone() });
        items[0].texture = "box.tga".to_string();
        let mut asked = Vec::new();
        let m = morph_frames(&items, 2, &mut |_, f| v.step_vertices(0, f).iter().map(|x| x.pos).collect(), &mut |n| {
            asked.push(n.to_string());
            None
        }, &mut || false)
        .unwrap();
        assert_eq!(asked, ["box.tga"], "the same name in another letter case is asked once");
        assert_eq!(m.missing, ["box.tga"]);
        assert!(m.frames.frames.iter().all(|f| drawn(f) > 0), "missing textures are drawn grey");
    }

    #[test]
    fn plain_mesh_picture_draws_grey_and_refuses_faces_without_vertices() {
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let uv = [[0.0, 0.0]; 3];
        let p = plain_mesh_picture(&pos, &uv, &[[0, 1, 2]]).unwrap();
        assert_eq!((p.width, p.height), (480, 440));
        assert!(p.plain_px > 100 && p.textured_px == 0, "{} {}", p.plain_px, p.textured_px);
        assert!(plain_mesh_picture(&pos, &uv, &[[0, 1, 3]]).is_none(), "an index past the vertices is not drawn");
        assert!(plain_mesh_picture(&pos, &uv, &[]).is_none());
        assert!(plain_mesh_picture(&[], &[], &[[0, 0, 0]]).is_none());
    }
}
