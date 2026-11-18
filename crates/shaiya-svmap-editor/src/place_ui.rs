//! The Placements tool between the pure operations of `place.rs` and the two viewports: the state the tool keeps (armed model,
//! drag, bounds cache, model browser), the selection and drag handling both views share, and the 3D overlay lines.
//! Nothing here writes a file; every change is a client op through `Doc::commit_client`.

use std::path::PathBuf;

use shaiya_clientdata::wld::Placement;
use shaiya_clientdata::wld_raw::Codepage;
use shaiya_clientdata::ClientSource;

use crate::assets::{Ground, MapAssets};
use crate::doc::Doc;
use crate::gpu3d::{OVert, OverlayGeo};
use crate::math3d::V3;
use crate::place::{self, BoundsCache, PSel, PlaceList, Pick, STAND_IN};

/// The 2D dots of the placements that are not in the saved base, kept for one (part serial, placement revision).
type ChangedDots = ((u64, u64), Vec<(f32, f32, u8)>);

/// A drag of the selected placements: they move by `delta` (x, z) from where they were when the drag began. The model is only
/// changed when the drag ends, so Esc cancels and the drag is one undo step.
pub struct PlaceDrag {
    pub snaps: Vec<(PSel, Placement)>,
    pub press: [f32; 2],
    pub delta: [f32; 2],
}

/// The Models window's picture of a model: a textured, lit, depth-buffered image with the model's visible edges as a wire,
/// made once when the model is chosen.
pub struct Preview {
    /// Model-space edge pairs, at most [`PREVIEW_EDGES`].
    pub edges: Vec<[V3; 2]>,
    pub bounds: Option<place::Bounds>,
    /// The rendered picture; `None` when the model has no faces (a dungeon piece, or a file that cannot be read).
    pub picture: Option<Picture>,
    /// Textures the model names that could not be read (each once); their triangles are drawn plain grey.
    pub missing: Vec<String>,
    /// Textures with see-through texels (alpha under one half: leaves, fences, railings), which the picture cuts out.
    pub see_through: Vec<String>,
    /// Textures without a single opaque texel: the game draws nothing there (collision boxes), so neither does the picture.
    pub invisible: Vec<String>,
    /// The picture uploaded to egui, made by the window the first time it draws this preview.
    pub tex: Option<egui::TextureHandle>,
}

/// Size of the Models window's picture, points.
pub const PREVIEW_SIZE: [f32; 2] = [240.0, 220.0];
/// The picture is rendered at this many pixels per point (egui's linear filter then smooths the edges).
const PICTURE_SCALE: f32 = 2.0;
/// At most this many triangles are rendered and this many edges drawn as the wire.
const PREVIEW_TRIS: usize = 60_000;
pub const PREVIEW_EDGES: usize = 90_000;

/// A rendered model picture, RGBA (row 0 = top, alpha 0 where nothing was drawn), and what was drawn.
pub struct Picture {
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

impl PreviewFit {
    /// The fit of model-space points into the picture with an 8 point margin; `None` without points.
    pub fn of(points: impl Iterator<Item = V3>) -> Option<PreviewFit> {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in points {
            let v = preview_view(p);
            if !(v[0].is_finite() && v[1].is_finite()) {
                continue;
            }
            lo = [lo[0].min(v[0]), lo[1].min(v[1])];
            hi = [hi[0].max(v[0]), hi[1].max(v[1])];
        }
        if lo[0] > hi[0] {
            return None;
        }
        let scale = ((PREVIEW_SIZE[0] - 16.0) / (hi[0] - lo[0]).max(0.01)).min((PREVIEW_SIZE[1] - 16.0) / (hi[1] - lo[1]).max(0.01));
        Some(PreviewFit { mid: [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5], scale })
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
pub fn render_picture(tris: &[PreviewTri], textures: &[crate::scene3d::MipImage], edges: &[[V3; 2]], fit: PreviewFit) -> Picture {
    let (w, h) = ((PREVIEW_SIZE[0] * PICTURE_SCALE) as usize, (PREVIEW_SIZE[1] * PICTURE_SCALE) as usize);
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
            [q[0] * PICTURE_SCALE, q[1] * PICTURE_SCALE, q[2]]
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
        let (a, b) = ([a[0] * PICTURE_SCALE, a[1] * PICTURE_SCALE, a[2]], [b[0] * PICTURE_SCALE, b[1] * PICTURE_SCALE, b[2]]);
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
    Picture { width: w, height: h, rgba, textured_px, plain_px }
}

#[derive(Default)]
pub struct PlaceUi {
    /// The model a click adds (set in the model browser; Esc clears it).
    pub armed: Option<(PlaceList, String)>,
    /// The heading a new placement gets, degrees.
    pub armed_yaw: f32,
    pub cache: BoundsCache,
    pub drag: Option<PlaceDrag>,
    pub browser_open: bool,
    pub list: PlaceList,
    pub filter: String,
    /// The model names per list, read once per client folder.
    pub models: Option<(PathBuf, Vec<Vec<String>>)>,
    pub preview: Option<Preview>,
    /// The client folder the cached bounds and model names were read from.
    pub client_key: Option<PathBuf>,
    /// The 2D dots of the placements that are not in the saved base (added or moved), `(x, z, kind 0 building 1 shape 2 tree)`,
    /// kept for one (part serial, placement revision).
    changed: Option<ChangedDots>,
}

/// The ground placements follow: the terrain of a field map. `None` on a dungeon map (the floor comes from the `.dg`, so
/// nothing snaps there) and without client data.
pub fn field_ground(a: Option<&MapAssets>) -> Option<&Ground> {
    a.map(|a| &a.ground).filter(|g| matches!(g, Ground::Field(_)))
}

impl PlaceUi {
    /// The placements added or moved since the tab's base, for the 2D view to draw as dots whatever the Objects layer says.
    pub fn changed_dots(&mut self, part: &crate::pair::ClientPart) -> &[(f32, f32, u8)] {
        let key = (part.version().0, part.place_rev());
        if self.changed.as_ref().map(|c| c.0) != Some(key) {
            let m = &part.wld.model;
            let mut out = Vec::new();
            for (k, list) in [PlaceList::Building, PlaceList::Shape, PlaceList::Tree].into_iter().enumerate() {
                let sig = |l: &shaiya_clientdata::wld_raw::RawPlaced, p: &Placement| -> Option<(String, [u32; 9])> {
                    let n = place::model_name(l, p)?.to_ascii_lowercase();
                    let f = |a: [f32; 3]| a.map(f32::to_bits);
                    let (a, b, c) = (f(p.pos), f(p.dir), f(p.up));
                    Some((n, [a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]))
                };
                let base = &part.base.places[list.slot()];
                let have: std::collections::HashSet<(String, [u32; 9])> = base.items.iter().filter_map(|p| sig(base, p)).collect();
                let cur = list.list(m);
                for p in &cur.items {
                    if let Some(s) = sig(cur, p) {
                        if !have.contains(&s) {
                            out.push((p.pos[0], p.pos[2], k as u8));
                        }
                    }
                }
            }
            self.changed = Some((key, out));
        }
        self.changed.as_ref().map(|c| c.1.as_slice()).unwrap_or(&[])
    }

    /// Drops what was read from another client folder (bounds, model names, preview), so picking never uses another client's
    /// boxes. Called every frame.
    pub fn follow_client(&mut self, key: Option<&PathBuf>) {
        if self.client_key.as_ref() != key {
            self.client_key = key.cloned();
            self.cache.forget();
            self.models = None;
            self.preview = None;
        }
    }

    /// Reads the model names of the client folder `key` once.
    pub fn ensure_models(&mut self, key: &PathBuf, src: &ClientSource) {
        if self.models.as_ref().is_some_and(|(k, _)| k == key) {
            return;
        }
        let files = src.list();
        self.models = Some((key.clone(), PlaceList::ALL.iter().map(|l| place::model_names(&files, *l)).collect()));
        self.cache.forget();
        self.preview = None;
    }

    pub fn arm(&mut self, list: PlaceList, name: &str, src: Option<&ClientSource>) {
        self.armed = Some((list, name.to_string()));
        self.preview = build_preview(src, list, name);
    }
}

/// The preview of a model: its textures read once each (the same lookup and size cap as the 3D view), the textured picture and
/// its edges for the wire (at most [`PREVIEW_EDGES`]).
pub fn build_preview(src: Option<&ClientSource>, list: PlaceList, name: &str) -> Option<Preview> {
    let src = src?;
    let bounds = place::load_bounds(src, list, name);
    let mut edges = Vec::new();
    let mut tris = Vec::new();
    let mut textures = Vec::new();
    let mut missing = Vec::new();
    let (mut see_through, mut invisible) = (Vec::new(), Vec::new());
    if list != PlaceList::Dungeon {
        if let Ok(sm) = src.smod(list.folder().rsplit('/').next().unwrap_or("Building"), name) {
            let mut tex_of: std::collections::HashMap<String, Option<usize>> = std::collections::HashMap::new();
            for m in &sm.modules {
                let tex = if m.texture.trim().is_empty() {
                    None
                } else {
                    *tex_of.entry(m.texture.trim().to_ascii_lowercase()).or_insert_with(|| match src.texture_flex("Entity/Texture", &m.texture) {
                        Ok(i) => {
                            let opaque = i.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128).count();
                            if opaque * 4 < i.rgba.len() {
                                see_through.push(m.texture.trim().to_string());
                            }
                            if opaque == 0 {
                                invisible.push(m.texture.trim().to_string());
                            }
                            textures.push(crate::scene3d::mip_image_capped(&i));
                            Some(textures.len() - 1)
                        }
                        Err(_) => {
                            missing.push(m.texture.trim().to_string());
                            None
                        }
                    })
                };
                for f in &m.faces {
                    if tris.len() >= PREVIEW_TRIS {
                        break;
                    }
                    if let (Some(a), Some(b), Some(c)) = (m.vertices.get(f[0] as usize), m.vertices.get(f[1] as usize), m.vertices.get(f[2] as usize)) {
                        tris.push(PreviewTri { pos: [a.pos, b.pos, c.pos], uv: [a.uv, b.uv, c.uv], tex });
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
        }
    }
    let picture = if tris.is_empty() { None } else { PreviewFit::of(tris.iter().flat_map(|t| t.pos)).map(|fit| render_picture(&tris, &textures, &edges, fit)) };
    Some(Preview { edges, bounds, picture, missing, see_through, invisible, tex: None })
}

// ---------------------------------------------------------------------------------------------------------------- picking

/// The placements under a ray, nearest first. Needs the client half of the tab.
pub fn pick_ray(doc: &Doc, ui: &mut PlaceUi, src: Option<&ClientSource>, origin: V3, dir: V3) -> Vec<Pick> {
    let Some(p) = doc.part() else { return Vec::new() };
    place::pick(&p.wld.model, &mut ui.cache, src, &PlaceList::ALL, origin, dir, Some(250.0))
}

/// The ray of the 2D view: straight down over (x, z).
pub fn pick_xz(doc: &Doc, ui: &mut PlaceUi, src: Option<&ClientSource>, w: [f32; 2]) -> Vec<Pick> {
    pick_ray(doc, ui, src, [w[0], 1.0e5, w[1]], [0.0, -1.0, 0.0])
}

/// A click: the nearest hit is selected (Shift toggles it, Alt takes the next one under it); nothing hit clears the selection.
pub fn click_select(doc: &mut Doc, hits: &[Pick], shift: bool, alt: bool) {
    if hits.is_empty() {
        if !shift {
            doc.place_sel.clear();
        }
        return;
    }
    let mut pick = hits[0].sel;
    if alt {
        if let Some(cur) = hits.iter().position(|h| doc.place_sel.contains(&h.sel)) {
            pick = hits[(cur + 1) % hits.len()].sel;
        }
    }
    if shift {
        match doc.place_sel.iter().position(|s| *s == pick) {
            Some(i) => {
                doc.place_sel.remove(i);
            }
            None => doc.place_sel.push(pick),
        }
    } else {
        doc.place_sel = vec![pick];
    }
}

/// Starts dragging what is under the press: the hit is selected first when it is not (Shift adds). A tab that cannot take client
/// edits starts nothing.
pub fn begin_drag(doc: &mut Doc, ui: &mut PlaceUi, hit: Pick, press: [f32; 2], shift: bool) {
    if doc.client_edit_check().is_err() {
        return;
    }
    if !doc.place_sel.contains(&hit.sel) {
        if shift {
            doc.place_sel.push(hit.sel);
        } else {
            doc.place_sel = vec![hit.sel];
        }
    }
    let Some(p) = doc.part() else { return };
    let snaps: Vec<(PSel, Placement)> = doc.place_sel.iter().filter_map(|s| place::get(&p.wld.model, *s).map(|pl| (*s, pl))).collect();
    if !snaps.is_empty() {
        ui.drag = Some(PlaceDrag { snaps, press, delta: [0.0; 2] });
    }
}

/// The ops a drag makes: x and z move by the delta (snapped to `snap`), y follows the terrain by the height difference (a field),
/// so a placement that stands above or below the ground keeps its offset.
pub fn drag_ops(m: &shaiya_clientdata::wld_raw::WldRaw, d: &PlaceDrag, snap: f32, ground: Option<&Ground>) -> Vec<crate::pair::COp> {
    let (dx, dz) = (crate::view2d::snap_to(d.delta[0], snap), crate::view2d::snap_to(d.delta[1], snap));
    let edits: Vec<(PSel, Placement)> = d
        .snaps
        .iter()
        .map(|(s, p)| {
            let mut q = *p;
            q.pos[0] += dx;
            q.pos[2] += dz;
            if let Some(g) = ground {
                if let (Some(a), Some(b)) = (g.height(p.pos[0], p.pos[2]), g.height(q.pos[0], q.pos[2])) {
                    q.pos[1] += b - a;
                }
            }
            (*s, q)
        })
        .collect();
    place::replace_ops(m, &edits)
}

/// Ends the drag: one undo step when anything moved. Returns a message when the edit was refused.
pub fn finish_drag(doc: &mut Doc, ui: &mut PlaceUi, snap: f32, ground: Option<&Ground>) -> Option<String> {
    let d = ui.drag.take()?;
    let m = &doc.part()?.wld.model;
    // The records under the drag must still be the ones it started on: an undo, a redo or a tab switch during the drag would
    // otherwise write the snapshot (with its name index) over another record.
    if d.snaps.iter().any(|(s, p)| place::get(m, *s).as_ref() != Some(p)) {
        return Some("The placements changed during the drag, so nothing was moved.".to_string());
    }
    let ops = drag_ops(m, &d, snap, ground);
    if ops.is_empty() {
        return None;
    }
    let label = if d.snaps.len() == 1 { "move placement".to_string() } else { format!("move {} placements", d.snaps.len()) };
    doc.commit_client(label, ops).err()
}

/// Adds the armed model at (x, z): on the terrain of a field, else at `y_hit` (the height the 3D ray met) or 0 (a dungeon has
/// no terrain to follow).
pub fn add_at(doc: &mut Doc, ui: &PlaceUi, xz: [f32; 2], y_hit: Option<f32>, snap: f32, ground: Option<&Ground>, cp: Codepage) -> Result<(), String> {
    let Some((list, name)) = ui.armed.clone() else { return Err("no model is chosen".to_string()) };
    doc.client_edit_check()?;
    let (x, z) = (crate::view2d::snap_to(xz[0], snap), crate::view2d::snap_to(xz[1], snap));
    let y = ground.and_then(|g| g.height(x, z)).or(y_hit.filter(|y| y.is_finite())).unwrap_or(0.0);
    let m = &doc.part().ok_or("no client map")?.wld.model;
    let (ops, at) = place::add_ops(m, list, &name, [x, y, z], ui.armed_yaw, cp)?;
    doc.commit_client(format!("add {}", list.label()), ops)?;
    doc.place_sel = vec![(list, at)];
    Ok(())
}

pub fn rotate_selected(doc: &mut Doc, deg: f32) -> Result<(), String> {
    let sels = doc.place_sel.clone();
    let ops = place::rotate_ops(&doc.part().ok_or("no client map")?.wld.model, &sels, deg);
    if ops.is_empty() {
        return Ok(());
    }
    doc.commit_client_coalesced("place-turn", "turn placements", ops)
}

pub fn delete_selected(doc: &mut Doc) -> Result<usize, String> {
    let sels = doc.place_sel.clone();
    let ops = place::delete_ops(&doc.part().ok_or("no client map")?.wld.model, &sels);
    let n = ops.len();
    if n == 0 {
        return Ok(0);
    }
    doc.commit_client(format!("delete {n} placements"), ops)?;
    doc.place_sel.clear();
    Ok(n)
}

pub fn duplicate_selected(doc: &mut Doc) -> Result<usize, String> {
    let sels = doc.place_sel.clone();
    let (ops, made) = place::duplicate_ops(&doc.part().ok_or("no client map")?.wld.model, &sels, [2.0, 0.0, 2.0]);
    let n = ops.len();
    if n == 0 {
        return Ok(0);
    }
    doc.commit_client(format!("duplicate {n} placements"), ops)?;
    doc.place_sel = made;
    Ok(n)
}

pub fn snap_selected(doc: &mut Doc, ground: Option<&Ground>) -> Result<usize, String> {
    let Some(g) = ground else { return Err("a dungeon has no terrain: nothing to snap to (the floor comes from the .dg)".to_string()) };
    let sels = doc.place_sel.clone();
    let ops = place::snap_ops(&doc.part().ok_or("no client map")?.wld.model, &sels, &|x, z| g.height(x, z));
    let n = ops.len();
    if n > 0 {
        doc.commit_client("snap placements to the ground", ops)?;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------------------------- 3D overlay

fn lines_of(c: &[V3; 8], col: [u8; 4], out: &mut Vec<OVert>) {
    for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
        out.push(OVert { pos: c[i], col });
        out.push(OVert { pos: c[j], col });
    }
}

/// Adds the placement boxes of the Placements tool to the 3D overlay: selected ones (yellow, over everything), the drag preview,
/// placements whose model is not in the scene (cyan), and the armed model at the cursor.
#[allow(clippy::too_many_arguments)]
pub fn overlay_3d(o: &mut OverlayGeo, doc: &Doc, ui: &mut PlaceUi, src: Option<&ClientSource>, missing: &[(PlaceList, usize)], cursor: Option<V3>, tool_on: bool, snap: f32, ground: Option<&Ground>) {
    let Some(part) = doc.part() else { return };
    let m = &part.wld.model;
    let bounds = |ui: &mut PlaceUi, l: PlaceList, p: &Placement| {
        let name = place::model_name(l.list(m), p).unwrap_or_default();
        ui.cache.get(src, l, &name).unwrap_or(STAND_IN)
    };
    for (l, i) in missing.iter().take(500) {
        if let Some(p) = place::get(m, (*l, *i)) {
            let b = bounds(ui, *l, &p);
            lines_of(&place::world_corners(&p, &b), [80, 220, 230, 255], &mut o.lines);
        }
    }
    if !tool_on {
        return;
    }
    let drag = ui.drag.as_ref().map(|d| (d.snaps.clone(), drag_ops(m, d, snap, ground)));
    for s in &doc.place_sel {
        if let Some(p) = place::get(m, *s) {
            let b = bounds(ui, s.0, &p);
            lines_of(&place::world_corners(&p, &b), [255, 235, 60, 255], &mut o.lines_top);
        }
    }
    if let Some((snaps, ops)) = drag {
        for op in ops {
            if let crate::pair::COp::Place(l, i, crate::pair::CKind::Replace { after, .. }) = op {
                let b = bounds(ui, l, &after);
                lines_of(&place::world_corners(&after, &b), [255, 150, 40, 255], &mut o.lines_top);
                let _ = (i, &snaps);
            }
        }
    }
    if let (Some((l, name)), Some(c)) = (ui.armed.clone(), cursor) {
        let b = ui.cache.get(src, l, &name).unwrap_or(STAND_IN);
        let p = place::upright(0, c, ui.armed_yaw);
        lines_of(&place::world_corners(&p, &b), [120, 255, 120, 255], &mut o.lines_top);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::paired_doc;
    use shaiya_clientdata::wld_raw::RawPlaced;
    use shaiya_clientdata::wld_raw::{name_text, WldRaw};
    use crate::place::{get, is_orthonormal, replace_ops, with_yaw};

    fn nm(s: &str) -> [u8; 256] {
        let mut b = [0u8; 256];
        b[..s.len()].copy_from_slice(s.as_bytes());
        b
    }

    fn doc() -> Doc {
        let mut d = paired_doc();
        let m = &mut d.part_mut().unwrap().wld.model;
        m.building = RawPlaced { names: vec![nm("a.smod")], items: vec![place::upright(0, [10.0, 1.0, 10.0], 0.0), place::upright(0, [30.0, 1.0, 30.0], 90.0)] };
        let p = d.part_mut().unwrap();
        p.base = p.lists();
        d
    }

    fn hit(i: usize) -> Pick {
        Pick { sel: (PlaceList::Building, i), t: 0.0 }
    }

    #[test]
    fn click_selects_shift_toggles_alt_cycles() {
        let mut d = doc();
        click_select(&mut d, &[hit(0), hit(1)], false, false);
        assert_eq!(d.place_sel, [(PlaceList::Building, 0)]);
        click_select(&mut d, &[hit(0), hit(1)], false, true);
        assert_eq!(d.place_sel, [(PlaceList::Building, 1)], "alt takes the next one under the cursor");
        click_select(&mut d, &[hit(0)], true, false);
        assert_eq!(d.place_sel.len(), 2);
        click_select(&mut d, &[hit(0)], true, false);
        assert_eq!(d.place_sel, [(PlaceList::Building, 1)]);
        click_select(&mut d, &[], false, false);
        assert!(d.place_sel.is_empty());
    }

    /// A drag changes nothing until it ends; then it is one undo step; Esc (dropping the drag) changes nothing.
    #[test]
    fn a_drag_is_one_step_and_cancel_changes_nothing() {
        let mut d = doc();
        let mut ui = PlaceUi::default();
        let l0 = d.part().unwrap().lists();
        begin_drag(&mut d, &mut ui, hit(1), [30.0, 30.0], false);
        ui.drag.as_mut().unwrap().delta = [5.2, -3.0];
        assert_eq!(d.part().unwrap().lists(), l0, "nothing changes during the drag");
        ui.drag = None;
        assert!(finish_drag(&mut d, &mut ui, 1.0, None).is_none() && d.undo.is_empty(), "a cancelled drag leaves no step");
        begin_drag(&mut d, &mut ui, hit(1), [30.0, 30.0], false);
        ui.drag.as_mut().unwrap().delta = [5.2, -3.0];
        assert!(finish_drag(&mut d, &mut ui, 1.0, None).is_none());
        assert_eq!(d.undo.len(), 1);
        assert_eq!(d.part().unwrap().wld.model.building.items[1].pos, [35.0, 1.0, 27.0], "snapped to 1 unit");
        d.undo();
        assert_eq!(d.part().unwrap().lists(), l0);
    }

    /// an undo during a drag (Ctrl+Z with the mouse held) changes the records under it; the drag then moves
    /// nothing instead of writing its snapshot over another record.
    #[test]
    fn a_drag_over_an_undo_moves_nothing() {
        let mut d = doc();
        let mut ui = PlaceUi::default();
        d.place_sel = vec![(PlaceList::Building, 0)];
        assert_eq!(delete_selected(&mut d), Ok(1));
        // what is now building 0 was building 1
        begin_drag(&mut d, &mut ui, hit(0), [30.0, 30.0], false);
        ui.drag.as_mut().unwrap().delta = [5.0, 5.0];
        d.undo();
        let l0 = d.part().unwrap().lists();
        assert!(finish_drag(&mut d, &mut ui, 1.0, None).is_some(), "refused with a message");
        assert_eq!(d.part().unwrap().lists(), l0, "the restored building 0 kept its place, building 1 is not doubled");
        assert!(d.undo.is_empty() && ui.drag.is_none());
    }

    /// Selecting indices are dropped when a step inserts or removes placements, so no index points at the wrong record.
    #[test]
    fn selection_does_not_survive_a_structure_change() {
        let mut d = doc();
        d.place_sel = vec![(PlaceList::Building, 1)];
        assert_eq!(delete_selected(&mut d), Ok(1));
        assert!(d.place_sel.is_empty());
        d.undo();
        assert!(d.place_sel.is_empty(), "undo of a delete clears it too");
        d.place_sel = vec![(PlaceList::Building, 0)];
        assert_eq!(duplicate_selected(&mut d), Ok(1));
        assert_eq!(d.place_sel, [(PlaceList::Building, 2)], "the copy is selected");
        rotate_selected(&mut d, 15.0).unwrap();
        assert_eq!(d.place_sel.len(), 1, "a turn keeps the selection");
    }

    #[test]
    fn nothing_snaps_without_a_field() {
        let mut d = doc();
        d.place_sel = vec![(PlaceList::Building, 0)];
        assert!(snap_selected(&mut d, None).is_err());
        let ui = PlaceUi { armed: Some((PlaceList::Tree, "t.smod".to_string())), ..Default::default() };
        add_at(&mut d, &ui, [12.4, 7.6], None, 1.0, None, Codepage::Cp949).unwrap();
        let m = &d.part().unwrap().wld.model;
        assert_eq!(m.tree.items[0].pos, [12.0, 0.0, 8.0], "no terrain: height 0");
        assert_eq!(d.place_sel, [(PlaceList::Tree, 0)]);
    }

    /// The same edits on the real map 2 of the client: the model still validates, writes back and reads back.
    #[test]
    fn real_map_edits_write_back() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(rules) = crate::client_rules::ClientRules::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
            return;
        };
        let crate::pair::Attach::Part(part) = crate::pair::ClientPart::attach(&rules, 2, Codepage::Cp949) else {
            eprintln!("skipped: map 2 of SHAIYA_CLIENT has no client half");
            return;
        };
        let mut d = Doc::new(1, Some(2), None, shaiya_svmap::Svmap::new(1024), None);
        d.set_client(crate::doc::ClientSlot::Part(part));
        let l0 = d.part().unwrap().lists();
        let nb = l0.places[0].items.len();
        assert!(nb > 0, "map 2 has buildings");
        let name = place::model_name(&l0.places[0], &l0.places[0].items[0]).unwrap();
        let ui = PlaceUi { armed: Some((PlaceList::Building, name.clone())), ..Default::default() };
        add_at(&mut d, &ui, [100.0, 100.0], None, 1.0, None, Codepage::Cp949).unwrap();
        d.place_sel = vec![(PlaceList::Building, 0), (PlaceList::Tree, 0)];
        rotate_selected(&mut d, 37.0).unwrap();
        d.place_sel = vec![(PlaceList::Building, nb)];
        delete_selected(&mut d).unwrap();
        let m = &d.part().unwrap().wld.model;
        assert!(m.validate().is_empty());
        assert_eq!(m.building.names, l0.places[0].names, "no name added: the model was in the list");
        let back = shaiya_clientdata::wld_raw::WldRaw::parse(&m.to_bytes(), "map 2").unwrap();
        assert_eq!(&back.building, &m.building);
        while d.undo().is_some() {}
        assert_eq!(d.part().unwrap().lists(), l0);
    }

    /// The 2D dots of added or moved placements: none for an untouched tab, one for an added model, and a moved one counts too.
    #[test]
    fn changed_dots_follow_added_and_moved_placements() {
        let mut d = doc();
        let mut ui = PlaceUi::default();
        assert!(ui.changed_dots(d.part().unwrap()).is_empty(), "nothing changed yet");
        ui.armed = Some((PlaceList::Tree, "new.smod".to_string()));
        add_at(&mut d, &ui, [50.0, 60.0], None, 1.0, None, Codepage::Cp949).unwrap();
        let dots = ui.changed_dots(d.part().unwrap()).to_vec();
        assert_eq!(dots.len(), 1, "{dots:?}");
        assert_eq!((dots[0].0, dots[0].1, dots[0].2), (50.0, 60.0, 2));
        d.place_sel = vec![(PlaceList::Building, 0)];
        let m = &d.part().unwrap().wld.model;
        let moved = place::replace_ops(m, &[((PlaceList::Building, 0), place::upright(0, [11.0, 1.0, 12.0], 0.0))]);
        d.commit_client("move", moved).unwrap();
        let dots = ui.changed_dots(d.part().unwrap()).to_vec();
        assert_eq!(dots.len(), 2, "{dots:?}");
        assert!(dots.iter().any(|x| (x.0, x.1, x.2) == (11.0, 12.0, 0)));
        while d.undo().is_some() {}
        assert!(ui.changed_dots(d.part().unwrap()).is_empty(), "undone: nothing differs from the base");
    }

    /// A quad with a red and blue checker texture renders both colours (the texture, not one averaged colour), texels with
    /// alpha under one half are cut out, coordinates past 1 repeat, a nearer triangle hides a farther one, and a triangle
    /// without a texture is plain grey.
    #[test]
    fn the_picture_samples_the_texture() {
        let mut rgba = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                rgba.extend_from_slice(if (x / 4 + y / 4) % 2 == 0 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        // the right half of the second texture is see-through
        let mut cut = rgba.clone();
        for y in 0..8 {
            for x in 4..8 {
                cut[(y * 8 + x) * 4 + 3] = 0;
            }
        }
        let tex = |rgba: Vec<u8>| crate::scene3d::MipImage { width: 8, height: 8, mips: crate::scene3d::mip_chain(8, 8, rgba) };
        // `z` moves the quad away from the viewer: along the model-space direction the view turns into +z (the view is a
        // rotation, so that is the third row of its matrix)
        let away = [preview_view([1.0, 0.0, 0.0])[2], preview_view([0.0, 1.0, 0.0])[2], preview_view([0.0, 0.0, 1.0])[2]];
        let quad = |z: f32, uvmax: f32, tex: Option<usize>| {
            let at = |x: f32, y: f32| [x + away[0] * z, y + away[1] * z, away[2] * z];
            let (a, b, c, d) = (at(0.0, 0.0), at(4.0, 0.0), at(4.0, 4.0), at(0.0, 4.0));
            let (ua, ub, uc, ud) = ([0.0, 0.0], [uvmax, 0.0], [uvmax, uvmax], [0.0, uvmax]);
            [PreviewTri { pos: [a, b, c], uv: [ua, ub, uc], tex }, PreviewTri { pos: [a, c, d], uv: [ua, uc, ud], tex }]
        };
        let colours = |p: &Picture| {
            let mut set = std::collections::BTreeSet::new();
            for px in p.rgba.as_chunks::<4>().0.iter().filter(|px| px[3] == 255) {
                set.insert([px[0], px[1], px[2]]);
            }
            set
        };
        let textures = [tex(rgba), tex(cut)];
        let one = quad(0.0, 1.0, Some(0));
        let fit = PreviewFit::of(one.iter().flat_map(|t| t.pos)).unwrap();
        let p = render_picture(&one, &textures, &[], fit);
        let c = colours(&p);
        assert!(c.iter().any(|c| c[0] > 100 && c[2] == 0) && c.iter().any(|c| c[2] > 100 && c[0] == 0), "red and blue texels: {c:?}");
        assert!(p.textured_px > 10_000 && p.plain_px == 0, "{} {}", p.textured_px, p.plain_px);
        // repeated twice: still the texture, and as many pixels
        let p2 = render_picture(&quad(0.0, 2.0, Some(0)), &textures, &[], fit);
        assert_eq!(p2.textured_px, p.textured_px);
        // the see-through half is not drawn
        let p3 = render_picture(&quad(0.0, 1.0, Some(1)), &textures, &[], fit);
        assert!(p3.textured_px * 10 < p.textured_px * 6 && p3.textured_px * 10 > p.textured_px * 4, "{} of {}", p3.textured_px, p.textured_px);
        // a grey quad behind the textured one is hidden, and alone it is all plain
        let mut both: Vec<PreviewTri> = quad(5.0, 1.0, None).into();
        both.extend(quad(0.0, 1.0, Some(0)));
        let p4 = render_picture(&both, &textures, &[], fit);
        assert_eq!((p4.textured_px, p4.plain_px), (p.textured_px, 0));
        let p5 = render_picture(&quad(0.0, 1.0, None), &textures, &[], fit);
        assert_eq!((p5.textured_px, p5.plain_px), (0, p.textured_px));
        assert!(colours(&p5).iter().all(|c| c[0] == c[1] && c[1] == c[2]), "plain grey");
    }

    /// The viewer looks down from above: up stays up, a higher point is nearer, and of two equal floors one above the other only
    /// the upper one is seen (a view from below showed a building's floor slab over its walls).
    #[test]
    fn the_preview_looks_down_from_above() {
        let up = preview_view([0.0, 1.0, 0.0]);
        assert!(up[1] > 0.0 && up[2] < 0.0, "{up:?}");
        let floor = |y: f32, tex: Option<usize>| {
            let (a, b, c, d) = ([0.0, y, 0.0], [4.0, y, 0.0], [4.0, y, 4.0], [0.0, y, 4.0]);
            [PreviewTri { pos: [a, b, c], uv: [[0.0; 2]; 3], tex }, PreviewTri { pos: [a, c, d], uv: [[0.0; 2]; 3], tex }]
        };
        let white = crate::scene3d::MipImage { width: 1, height: 1, mips: vec![vec![255, 255, 255, 255]] };
        let mut tris: Vec<PreviewTri> = floor(0.0, None).into();
        tris.extend(floor(0.01, Some(0)));
        let fit = PreviewFit::of(tris.iter().flat_map(|t| t.pos)).unwrap();
        let p = render_picture(&tris, std::slice::from_ref(&white), &[], fit);
        assert!(p.textured_px > 1000 && p.plain_px < p.textured_px / 50, "{} {}", p.textured_px, p.plain_px);
        // the wire: an edge half a unit over the floor is drawn, the same edge half a unit under it is hidden
        let one: Vec<PreviewTri> = floor(0.0, Some(0)).into();
        let bare = render_picture(&one, std::slice::from_ref(&white), &[], fit);
        let over = render_picture(&one, std::slice::from_ref(&white), &[[[1.5, 0.5, 1.5], [2.5, 0.5, 2.5]]], fit);
        let under = render_picture(&one, std::slice::from_ref(&white), &[[[1.5, -0.5, 1.5], [2.5, -0.5, 2.5]]], fit);
        let wire_px = |p: &Picture| p.rgba.as_chunks::<4>().0.iter().zip(bare.rgba.as_chunks::<4>().0.iter()).filter(|(a, b)| a != b).count();
        assert!(wire_px(&over) > 20 && wire_px(&under) == 0, "{} {}", wire_px(&over), wire_px(&under));
        assert_eq!(over.textured_px, bare.textured_px, "the wire does not change what the surface shows");
    }

    /// Every building, shape and tree of the client (`SHAIYA_CLIENT`): the Models window's picture shows real texels wherever
    /// the model names a texture the client has. Lists the models whose picture has no texel at all, and the models with
    /// see-through textures (leaves, fences, railings), which the earlier average-colour preview drew as solid flat blocks.
    #[test]
    fn every_client_model_previews_textured() {
        let Some(c) = std::env::var_os("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let rules = crate::client_rules::ClientRules::open(std::path::Path::new(&c)).unwrap();
        let src = &rules.source;
        let files = src.list();
        let (mut models, mut untextured, mut cutout, mut missing, mut invisible) = (0, Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for l in [PlaceList::Building, PlaceList::Shape, PlaceList::Tree] {
            for n in place::model_names(&files, l) {
                let pv = build_preview(Some(src), l, &n).expect("a client");
                let Ok(sm) = src.smod(l.folder().rsplit('/').next().unwrap(), &n) else { continue };
                if sm.modules.iter().all(|m| m.faces.is_empty()) {
                    continue;
                }
                models += 1;
                let pic = pv.picture.as_ref().unwrap_or_else(|| panic!("{n}: a model with faces has a picture"));
                // a model whose textures are all missing, unnamed or fully see-through has nothing to show texels of
                let drawable = sm.modules.iter().any(|m| !m.faces.is_empty() && !m.texture.trim().is_empty() && ![&pv.missing, &pv.invisible].iter().any(|v| v.iter().any(|t| t.eq_ignore_ascii_case(m.texture.trim()))));
                if pic.textured_px == 0 {
                    untextured.push(format!("{} {n}", l.label()));
                    assert!(!drawable, "{n}: names an opaque texture the client has, but the picture shows none");
                } else {
                    assert!(drawable, "{n}");
                }
                missing.extend(pv.missing.iter().map(|t| format!("{n}: {t}")));
                if !pv.invisible.is_empty() {
                    invisible.push(n.clone());
                }
                if !pv.see_through.is_empty() {
                    cutout.push(format!("{} {n}", l.label()));
                }
            }
        }
        eprintln!("{models} models with faces; textures missing: {missing:?}");
        eprintln!("with see-through textures, flat blocks before ({}): {cutout:?}", cutout.len());
        eprintln!("with a fully see-through texture ({}): {invisible:?}", invisible.len());
        eprintln!("pictures without any texel ({}): {untextured:?}", untextured.len());
        assert!(models > 1000, "{models}");
    }

    /// A long seeded run of every placement edit (add of a known and of a new model, delete, duplicate, turn,
    /// typed edit, drag with the ground, snap, undo, redo) on real client maps. After every step each name index is valid (or
    /// one the original already had), the original names keep their bytes and order, the NPC, portal, area and zone lists and
    /// the text are untouched, the selection points at real records; at checkpoints everything outside the four lists writes the
    /// original bytes and the model validates and reads back. Undo of everything gives the original file byte for byte.
    #[test]
    fn review_random_placement_edits_keep_the_file_valid() {
        use crate::place_ui as pu;
        use std::collections::HashSet;
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(rules) = crate::client_rules::ClientRules::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
            return;
        };
        struct Rng(u64);
        impl Rng {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                self.0 >> 33
            }
            fn below(&mut self, n: usize) -> usize {
                (self.next() % n.max(1) as u64) as usize
            }
            fn f(&mut self, lo: f32, hi: f32) -> f32 {
                lo + (self.next() % 10_000) as f32 / 10_000.0 * (hi - lo)
            }
        }
        // a field of each size, a field with dungeon pieces and a dungeon map with placements
        let mut ids: Vec<u32> = vec![2, 0];
        let mut extra = [None, None];
        for id in rules.source.map_ids() {
            let Ok(b) = rules.source.read(&format!("world/{id}.wld")) else { continue };
            let Ok(m) = WldRaw::parse(&b, "x") else { continue };
            if extra[0].is_none() && !m.dungeon.items.is_empty() && !ids.contains(&id) {
                extra[0] = Some(id);
            }
            if extra[1].is_none() && m.terrain.is_none() && PlaceList::ALL.iter().any(|l| !l.list(&m).items.is_empty()) {
                extra[1] = Some(id);
            }
        }
        ids.extend(extra.iter().flatten());
        eprintln!("review maps: {ids:?}");
        for id in ids {
            let bytes = rules.source.read(&format!("world/{id}.wld")).expect("read");
            let crate::pair::Attach::Part(part) = crate::pair::ClientPart::attach(&rules, id, Codepage::Cp949) else { continue };
            let mut d = Doc::new(1, Some(id), None, shaiya_svmap::Svmap::new(1024), None);
            d.set_client(crate::doc::ClientSlot::Part(part));
            let orig = model(&d).clone();
            assert_eq!(orig.to_bytes(), bytes, "map {id}: lossless before any edit");
            let core0 = d.part().unwrap().lists_core();
            let invalid: Vec<HashSet<i32>> = PlaceList::ALL.iter().map(|l| l.list(&orig).items.iter().map(|p| p.index).filter(|i| *i != -1 && (*i < 0 || *i as usize >= l.list(&orig).names.len())).collect()).collect();
            let ground = d.part().unwrap().wld_arc().terrain.clone().map(crate::assets::Ground::Field);
            let known: Vec<Vec<String>> = PlaceList::ALL.iter().map(|l| l.list(&orig).names.iter().map(|n| name_text(n)).filter(|n| !n.is_empty()).take(20).collect()).collect();
            let mut ui = pu::PlaceUi::default();
            // the first pick of the map reads the bounds of the models near the ray
            let t = std::time::Instant::now();
            let probe = PlaceList::ALL.iter().find_map(|l| l.list(&orig).items.iter().find(|p| p.index >= 0).map(|p| p.pos));
            if let Some(at) = probe {
                let hits = pu::pick_xz(&d, &mut ui, Some(&rules.source), [at[0], at[2]]);
                eprintln!("map {id}: first pick {:?} (debug build), {} hits", t.elapsed(), hits.len());
            }
            let check = |d: &Doc, full: bool, step: usize, what: &str| {
                let m = model(d);
                for (k, pl) in PlaceList::ALL.iter().enumerate() {
                    let (l, o) = (pl.list(m), pl.list(&orig));
                    assert!(l.names.len() >= o.names.len() && l.names[..o.names.len()] == o.names[..], "map {id} step {step} {what}: {pl:?} names changed");
                    for p in &l.items {
                        assert!(p.index == -1 || (p.index >= 0 && (p.index as usize) < l.names.len()) || invalid[k].contains(&p.index), "map {id} step {step} {what}: {pl:?} index {}", p.index);
                    }
                }
                assert!(d.part().unwrap().lists_core() == core0, "map {id} step {step} {what}: paired lists changed");
                for s in &d.place_sel {
                    assert!(get(m, *s).is_some(), "map {id} step {step} {what}: stale selection {s:?}");
                }
                if full {
                    let mut rest = m.clone();
                    for pl in PlaceList::ALL {
                        *pl.list_mut(&mut rest) = pl.list(&orig).clone();
                    }
                    assert!(rest.to_bytes() == bytes, "map {id} step {step} {what}: bytes outside the four lists changed");
                    assert!(m.validate().is_empty(), "map {id} step {step}: {:?}", m.validate());
                    let back = WldRaw::parse(&m.to_bytes(), "back").expect("reads back");
                    for pl in PlaceList::ALL {
                        assert!(pl.list(&back) == pl.list(m), "map {id} step {step}: {pl:?} reads back other");
                    }
                }
            };
            // the append rule: a second add of a new model reuses the appended name; undo of that add keeps it, undo of the
            // first takes it back
            let l = PlaceList::ALL.into_iter().find(|l| !l.list(&orig).items.is_empty()).unwrap_or(PlaceList::Building);
            let n0 = l.list(&orig).names.len();
            ui.armed = Some((l, "review_new.smod".to_string()));
            pu::add_at(&mut d, &ui, [50.0, 50.0], None, 1.0, ground.as_ref(), Codepage::Cp949).unwrap();
            pu::add_at(&mut d, &ui, [60.0, 50.0], None, 1.0, ground.as_ref(), Codepage::Cp949).unwrap();
            assert_eq!(l.list(model(&d)).names.len(), n0 + 1);
            d.undo();
            assert_eq!(l.list(model(&d)).names.len(), n0 + 1, "the second add did not append, so its undo keeps the name");
            d.undo();
            assert_eq!(l.list(model(&d)).names.len(), n0, "the first add's undo takes the name back");
            check(&d, true, 0, "append rule");

            let mut rng = Rng(0x5eed ^ id as u64);
            let steps = 240;
            for step in 1..=steps {
                // a random selection of 1 to 3 placements
                let m = model(&d);
                let mut sel = Vec::new();
                for _ in 0..1 + rng.below(3) {
                    let pl = PlaceList::ALL[rng.below(4)];
                    let n = pl.list(m).items.len();
                    if n > 0 {
                        sel.push((pl, rng.below(n)));
                    }
                }
                let action = rng.below(10);
                let what = ["add known", "add new", "delete", "duplicate", "turn", "typed edit", "drag", "snap", "undo", "redo"][action];
                match action {
                    0 | 1 => {
                        let pl = PlaceList::ALL[rng.below(4)];
                        let name = if action == 0 && !known[pl.slot()].is_empty() { known[pl.slot()][rng.below(known[pl.slot()].len())].clone() } else { format!("review_{}.smod", rng.below(5)) };
                        ui.armed = Some((pl, name));
                        ui.armed_yaw = rng.f(-180.0, 180.0);
                        let at = [rng.f(0.0, 1000.0), rng.f(0.0, 1000.0)];
                        pu::add_at(&mut d, &ui, at, Some(rng.f(-5.0, 50.0)), 0.5, ground.as_ref(), Codepage::Cp949).unwrap();
                        ui.armed = None;
                    }
                    2 => {
                        d.place_sel = sel;
                        pu::delete_selected(&mut d).unwrap();
                    }
                    3 => {
                        d.place_sel = sel;
                        pu::duplicate_selected(&mut d).unwrap();
                    }
                    4 => {
                        let was: Vec<bool> = sel.iter().map(|s| is_orthonormal(&get(model(&d), *s).unwrap())).collect();
                        d.place_sel = sel.clone();
                        pu::rotate_selected(&mut d, if rng.below(2) == 0 { 15.0 } else { -15.0 }).unwrap();
                        for (s, w) in sel.iter().zip(was) {
                            assert!(!w || is_orthonormal(&get(model(&d), *s).unwrap()), "map {id} step {step}: a turn sheared {s:?}");
                        }
                    }
                    5 => {
                        if let Some(&s) = sel.first() {
                            let mut q = get(model(&d), s).unwrap();
                            q.pos[rng.below(3)] += rng.f(-20.0, 20.0);
                            let q = with_yaw(&q, rng.f(-180.0, 180.0));
                            let ops = replace_ops(model(&d), &[(s, q)]);
                            d.commit_client_coalesced("place-edit", "edit placement", ops).unwrap();
                        }
                    }
                    6 => {
                        if let Some(&s) = sel.first() {
                            let p = get(model(&d), s).unwrap();
                            pu::begin_drag(&mut d, &mut ui, Pick { sel: s, t: 0.0 }, [p.pos[0], p.pos[2]], false);
                            if let Some(dr) = ui.drag.as_mut() {
                                dr.delta = [rng.f(-30.0, 30.0), rng.f(-30.0, 30.0)];
                            }
                            assert!(pu::finish_drag(&mut d, &mut ui, 1.0, ground.as_ref()).is_none());
                        }
                    }
                    7 => {
                        d.place_sel = sel;
                        let r = pu::snap_selected(&mut d, ground.as_ref());
                        assert_eq!(r.is_err(), ground.is_none());
                    }
                    8 => {
                        d.undo();
                    }
                    _ => {
                        d.redo();
                    }
                }
                check(&d, step % 40 == 0, step, what);
            }
            check(&d, true, steps, "end");
            let depth = d.undo.len();
            while d.undo().is_some() {}
            assert!(model(&d).to_bytes() == bytes, "map {id}: undo of everything gives the original bytes");
            assert!(!d.client_dirty(), "map {id}: clean after undo of everything");
            d.jump_to(depth);
            check(&d, true, steps, "redo all");
            while d.undo().is_some() {}
            assert!(model(&d).to_bytes() == bytes, "map {id}: and again after redo of everything");
        }
    }

    fn model(d: &Doc) -> &shaiya_clientdata::wld_raw::WldRaw {
        &d.part().unwrap().wld.model
    }
}
