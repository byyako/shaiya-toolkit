//! The 2D top-down viewport: layers, picking, and the mouse tools (select/move/resize/rotate, draw a box, place an NPC or a
//! portal, measure). World x runs to the right and z up (north up), like the client's map board.

use std::collections::HashMap;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextureHandle, Vec2};
use shaiya_svmap::{Bind, Finding, MobArea, NpcEntry, NpcPoint, Region, SectionId, Severity, Svmap};

use crate::assets::{MapAssets, ObjKind};
use crate::client_rules::PORTAL_USE_RANGE;
use crate::compare;
use crate::doc::{move_ops, Doc, Sel};
use crate::edit::{self, Op, Rec};
use crate::names::{self, Lookups};
use crate::settings::{Layers, Locks, Template};

// pure helpers that moved to the library with the checks; the callers here and in the other windows keep these paths
pub use shaiya_svmap_tools::edit::{anchor_of, snap_to};

/// The ghost boxes of the client file: area boxes, zone groups, portal boxes and NPC positions.
type Ghosts<'a> = (Vec<([f32; 3], [f32; 3])>, &'a [shaiya_clientdata::wld::ZoneGroup], Vec<([f32; 3], [f32; 3])>, Vec<[f32; 3]>);

pub const MIN_ZOOM: f32 = 0.05;
pub const MAX_ZOOM: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Box,
    Npc,
    Portal,
    Paint,
    Measure,
    Scatter,
    /// Select, move, turn, add and delete the buildings, shapes and trees of the client map (C3).
    Place,
    /// Raise, lower, flatten, smooth, set and ramp the corner heights of the client map (C4).
    Terrain,
}

impl Tool {
    pub const ALL: [Tool; 9] = [Tool::Select, Tool::Box, Tool::Npc, Tool::Portal, Tool::Paint, Tool::Measure, Tool::Scatter, Tool::Place, Tool::Terrain];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Box => "Box",
            Tool::Npc => "NPC",
            Tool::Portal => "Portal",
            Tool::Paint => "Paint-blocked",
            Tool::Measure => "Measure",
            Tool::Scatter => "Scatter",
            Tool::Place => "Placements",
            Tool::Terrain => "Terrain",
        }
    }
}

/// What the Box tool draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawTarget {
    MobArea,
    Bind,
    Region,
    PointB,
}

impl DrawTarget {
    pub const ALL: [DrawTarget; 4] = [DrawTarget::MobArea, DrawTarget::Bind, DrawTarget::Region, DrawTarget::PointB];

    pub fn label(self) -> &'static str {
        match self {
            DrawTarget::MobArea => "monster area",
            DrawTarget::Bind => "bind area",
            DrawTarget::Region => "region",
            DrawTarget::PointB => "section B point (click)",
        }
    }
}

/// What the Paint-blocked tool does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintMode {
    /// Round brush, drag to paint.
    Brush,
    /// Drag a rectangle (cell-aligned) and fill it.
    Rect,
    /// Click: 4-connected flood fill of the cells that share the clicked cell's state.
    Fill,
    /// Drag a rectangle and invert every cell in it.
    Invert,
}

impl PaintMode {
    pub const ALL: [PaintMode; 4] = [PaintMode::Brush, PaintMode::Rect, PaintMode::Fill, PaintMode::Invert];

    pub fn label(self) -> &'static str {
        match self {
            PaintMode::Brush => "brush",
            PaintMode::Rect => "rectangle",
            PaintMode::Fill => "flood fill",
            PaintMode::Invert => "invert rectangle",
        }
    }
}

/// Settings of the Paint-blocked tool (left button applies `block`, right button the opposite).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintSettings {
    pub mode: PaintMode,
    /// Brush radius in cells, 0 (one cell) to [`MAX_BRUSH`].
    pub radius: f32,
    /// What the left button does: block (true) or free (false) cells.
    pub block: bool,
}

pub const MAX_BRUSH: f32 = 32.0;

impl Default for PaintSettings {
    fn default() -> Self {
        PaintSettings { mode: PaintMode::Brush, radius: 3.0, block: true }
    }
}

/// How the heatmap is drawn and what its legend shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeatDraw {
    pub mode: crate::heat::Mode,
    pub opacity: f32,
    /// The density at the top of the colour ramp.
    pub max: f32,
    pub tile: u32,
}

/// Texture handles of the current map (all optional).
#[derive(Default)]
pub struct Textures {
    pub terrain: Option<TextureHandle>,
    pub minimap: Option<TextureHandle>,
    pub dungeon: Option<TextureHandle>,
    pub blocked: Option<TextureHandle>,
    /// Overlay of the cells a generated bitmap would change (green frees, red blocks).
    pub preview: Option<TextureHandle>,
    /// The blocked cells that differ from the compare reference (purple).
    pub compare: Option<TextureHandle>,
    /// The heatmap picture (one pixel per tile), when the heatmap is on.
    pub heat: Option<TextureHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub sec: SectionId,
    pub idx: usize,
    pub sub: Option<usize>,
}

pub enum Drag {
    None,
    Move { snaps: Vec<(Sel, Rec)>, press: [f32; 2], anchor0: [f32; 2] },
    MovePoint { sel: Sel, point: usize, before: Rec },
    Resize { sel: Sel, sub: usize, handle: usize, before: Rec },
    Rotate { sel: Sel, before: Rec },
    Band { start: [f32; 2], add: bool },
    Draw { start: [f32; 2] },
    /// A measuring drag: one segment, or (`circle`) a radius from `start`.
    Measure { start: [f32; 2], circle: bool },
    /// A brush stroke in progress: the cells are already painted in the map; `before` is the grid at the start.
    Paint { value: bool, last: [f32; 2], before: Box<shaiya_svmap::BitGrid> },
    /// A rectangle being dragged for Paint-blocked (rectangle fill or invert).
    PaintRect { start: [f32; 2], value: bool, invert: bool },
}

pub struct VpState {
    pub drag: Drag,
    /// The Measure tool's measurement (it stays until Esc, a new one or a tool change).
    pub measure: crate::measure::Measure,
    /// Radius mode of the Measure tool (Alt+drag does the same for one drag).
    pub radius_mode: bool,
    /// Size of the viewport rectangle of the last frame.
    pub size: Vec2,
    pub hover: Option<[f32; 2]>,
    /// The viewport rectangle of the last frame (picture export crops to it).
    pub rect: Rect,
    /// Where and when the right button went down (a short release without movement opens the context menu).
    pub rpress: Option<(Pos2, f64)>,
    pub menu: Option<CtxMenu>,
    /// A drag that started inside the overview inset is in progress.
    pub ov_drag: bool,
    /// The Scatter tool's region while it is drawn (it belongs to this tab's view).
    pub scatter: crate::scatter::Input,
    /// The drag in progress started with Alt held on a tab with a client half: it changes the svmap only.
    pub alt_drag: bool,
}

impl Default for VpState {
    fn default() -> Self {
        VpState { drag: Drag::None, measure: Default::default(), radius_mode: false, size: Vec2::new(900.0, 600.0), hover: None, rect: Rect::NOTHING, rpress: None, menu: None, ov_drag: false, scatter: Default::default(), alt_drag: false }
    }
}

pub struct Ctx<'a> {
    pub doc: &'a mut Doc,
    pub layers: &'a Layers,
    pub locks: &'a Locks,
    pub look: Lookups<'a>,
    pub tex: &'a Textures,
    pub tool: Tool,
    pub snap: f32,
    pub drop_on_move: bool,
    pub draw_target: DrawTarget,
    pub npc_template: (u32, u32),
    pub paint: PaintSettings,
    pub minimap_alpha: f32,
    /// A banner text when the next click should pick a portal destination.
    pub pick_banner: Option<String>,
    pub map_name: &'a dyn Fn(u32) -> String,
    /// A crosshair to draw (the position Go to just moved to).
    pub marker: Option<[f32; 2]>,
    /// What differs from the compare reference (the compare layer draws it).
    pub compare: Option<&'a compare::Overlay>,
    /// The record an outliner, Problems or Find row is hovered over: it gets a white outline.
    pub hover_rec: Option<Sel>,
    /// Monster areas whose mean level is outside this range are dimmed and cannot be picked.
    pub level_filter: Option<(f32, f32)>,
    /// The overview inset (hidden while the whole map is in view).
    pub overview: bool,
    /// The spawn template the Box tool draws or stamps with.
    pub template: Option<&'a Template>,
    /// Whether the app has records to paste (the context menu's "Paste here").
    pub can_paste: bool,
    /// A picture export is running: no handles, hover outline, overview or legend in the picture.
    pub clean: bool,
    /// The findings of the record under the open context menu with the titles of their fixes (index = the fix number).
    pub menu_fixes: &'a [(Finding, Vec<String>)],
    /// The heatmap, when it is on (its picture is in `tex.heat`); the legend shows it instead of the level legend.
    pub heat: Option<HeatDraw>,
    /// Rulers along the top and left edge.
    pub rulers: bool,
    /// The Scatter tool's preview.
    pub scatter: Option<ScatterDraw<'a>>,
    /// Monster areas a Bulk edit rule matches (cyan outline).
    pub bulk_outline: &'a [usize],
    /// The Terrain tool's state (brush, stroke, ring).
    pub terrain: &'a mut crate::terrain_ui::TerrainUi,
    /// The Placements tool's state (armed model, drag, bounds cache).
    pub place: &'a mut crate::place_ui::PlaceUi,
    /// The page new placement names are written in.
    pub codepage: shaiya_clientdata::wld_raw::Codepage,
}

/// What the Scatter tool shows on the map besides the region: the areas it would add and a line about them.
pub struct ScatterDraw<'a> {
    pub areas: &'a [shaiya_svmap::MobArea],
    pub note: &'a str,
}

/// What a viewport context menu entry asks the app to do (the record entries act on the selection, which the menu made sure holds
/// the record that was clicked).
#[derive(Clone, Debug, PartialEq)]
pub enum CtxAct {
    Frame,
    Copy,
    Duplicate,
    Delete,
    DropToGround,
    SelectSimilar,
    Fix(Finding, usize),
    CopyPosition([f32; 2]),
    CopyGm([f32; 2]),
    GoToDest(usize),
    SaveTemplate(usize),
    PasteHere([f32; 2]),
    AddArea([f32; 2]),
    AddNpc([f32; 2]),
    AddBookmark([f32; 2]),
    GoTo,
}

/// What the context menu needs to know (the 2D and the 3D view both open it).
pub struct MenuIn<'a> {
    pub read_only: bool,
    /// The app has records to paste.
    pub can_paste: bool,
    pub template: Option<&'a Template>,
    /// The findings of the clicked record with the titles of their fixes (the index is the fix number).
    pub fixes: &'a [(Finding, Vec<String>)],
}

/// An open viewport context menu.
#[derive(Clone, Debug)]
pub struct CtxMenu {
    pub pos: Pos2,
    pub world: [f32; 2],
    pub hit: Option<Hit>,
}

/// Things the viewport asks the app to do (they need dialogs or other maps).
pub enum Request {
    Menu(CtxAct),
    AddPortal([f32; 3]),
    PickDone([f32; 3]),
    CancelPick,
    Toast(String),
}

#[derive(Default)]
pub struct Out {
    pub requests: Vec<Request>,
    pub cursor: Option<[f32; 2]>,
    pub hover_hit: Option<Hit>,
}

/// World <-> screen.
#[derive(Clone, Copy)]
pub struct Xf {
    centre: Pos2,
    cx: f32,
    cz: f32,
    pub zoom: f32,
}

impl Xf {
    pub fn new(rect: Rect, cx: f32, cz: f32, zoom: f32) -> Xf {
        Xf { centre: rect.center(), cx, cz, zoom }
    }

    pub fn to_screen(self, x: f32, z: f32) -> Pos2 {
        Pos2::new(self.centre.x + (x - self.cx) * self.zoom, self.centre.y - (z - self.cz) * self.zoom)
    }

    pub fn to_world(self, p: Pos2) -> [f32; 2] {
        [self.cx + (p.x - self.centre.x) / self.zoom, self.cz - (p.y - self.centre.y) / self.zoom]
    }

    /// Screen rect of a world box (x, z).
    pub fn rect(&self, min: [f32; 3], max: [f32; 3]) -> Rect {
        let a = self.to_screen(min[0].min(max[0]), min[2].max(max[2]));
        let b = self.to_screen(min[0].max(max[0]), min[2].min(max[2]));
        Rect::from_min_max(a, b)
    }
}

pub fn fit_view(doc: &mut Doc, size: Vec2, map_size: f32) {
    let zoom = (size.x.min(size.y) / map_size * 0.96).clamp(MIN_ZOOM, MAX_ZOOM);
    doc.view.zoom = zoom;
    doc.view.cx = map_size * 0.5;
    doc.view.cz = map_size * 0.5;
    doc.view.fitted = true;
}

/// Frames a world box (min x, min z, max x, max z) in a viewport of `size`.
pub fn frame_box(doc: &mut Doc, size: Vec2, b: (f32, f32, f32, f32)) {
    let w = (b.2 - b.0).max(16.0);
    let h = (b.3 - b.1).max(16.0);
    let zoom = (size.x / (w * 1.5)).min(size.y / (h * 1.5)).clamp(MIN_ZOOM, 24.0);
    doc.view.zoom = zoom;
    doc.view.cx = (b.0 + b.2) * 0.5;
    doc.view.cz = (b.1 + b.3) * 0.5;
    doc.view.fitted = true;
}

fn col(c: [u8; 3], a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], a)
}

pub fn visible(l: &Layers, s: SectionId) -> bool {
    match s {
        SectionId::MobAreas => l.mob_areas,
        SectionId::Npcs => l.npcs,
        SectionId::Portals => l.portals,
        SectionId::Binds => l.binds,
        SectionId::Regions => l.regions,
        SectionId::SectionB => l.points_b,
        SectionId::SectionA => l.section_a,
        _ => false,
    }
}

pub fn locked(l: &Locks, s: SectionId) -> bool {
    match s {
        SectionId::MobAreas => l.mob_areas,
        SectionId::Npcs => l.npcs,
        SectionId::Portals => l.portals,
        SectionId::Binds => l.binds,
        SectionId::Regions => l.regions,
        SectionId::SectionB => l.points_b,
        SectionId::SectionA => l.section_a,
        _ => true,
    }
}

fn selectable(c: &Ctx, s: SectionId) -> bool {
    visible(c.layers, s) && !locked(c.locks, s)
}

/// Whether the level filter hides this monster area (its rounded mean level is outside the range). An area whose level is not known
/// (no client data, or monsters the client lacks) is never hidden.
fn level_hidden(c: &Ctx, a: &MobArea) -> bool {
    outside_range(c.level_filter, c.look.mean_level(a))
}

/// The rule of [`level_hidden`] on its own: the rounded level is outside the filter range.
pub fn outside_range(filter: Option<(f32, f32)>, level: Option<f32>) -> bool {
    match (filter, level) {
        (Some((lo, hi)), Some(l)) => {
            let l = l.round();
            l < lo || l > hi
        }
        _ => false,
    }
}

const NPC_COLOURS: [[u8; 3]; 14] = [
    [150, 150, 150],
    [250, 200, 60],
    [120, 200, 255],
    [255, 140, 80],
    [255, 90, 90],
    [220, 120, 255],
    [110, 230, 140],
    [255, 255, 255],
    [255, 80, 160],
    [170, 120, 80],
    [150, 255, 255],
    [200, 200, 100],
    [120, 120, 120],
    [255, 60, 60],
];

pub fn npc_rgb(kind: u32) -> [u8; 3] {
    NPC_COLOURS[(kind as usize).min(13)]
}

fn npc_colour(kind: u32) -> Color32 {
    col(NPC_COLOURS[(kind as usize).min(13)], 255)
}

/// Box of a record that has handles (single box): (min, max) for areas, regions and one bind box.
fn box_of(r: &Rec, sub: usize) -> Option<([f32; 3], [f32; 3])> {
    match r {
        Rec::Mob(a) => Some((a.min, a.max)),
        Rec::Region(g) => Some((g.min, g.max)),
        Rec::Bind(b) => b.boxes.get(sub).map(|x| (x.min, x.max)),
        _ => None,
    }
}

fn set_box(r: &mut Rec, sub: usize, min: [f32; 3], max: [f32; 3]) {
    match r {
        Rec::Mob(a) => {
            a.min = min;
            a.max = max;
        }
        Rec::Region(g) => {
            g.min = min;
            g.max = max;
        }
        Rec::Bind(b) => {
            if let Some(x) = b.boxes.get_mut(sub) {
                x.min = min;
                x.max = max;
            }
        }
        _ => {}
    }
}

/// The 8 handles of a screen rect: TL, T, TR, R, BR, B, BL, L.
fn handle_points(r: Rect) -> [Pos2; 8] {
    let c = r.center();
    [r.left_top(), Pos2::new(c.x, r.top()), r.right_top(), Pos2::new(r.right(), c.y), r.right_bottom(), Pos2::new(c.x, r.bottom()), r.left_bottom(), Pos2::new(r.left(), c.y)]
}

fn handle_edges(h: usize) -> (bool, bool, bool, bool) {
    // (left, right, top = larger z, bottom = smaller z)
    (matches!(h, 0 | 6 | 7), matches!(h, 2..=4), matches!(h, 0..=2), matches!(h, 4..=6))
}

/// The selected single record's box/handles, with the bind box in `doc.sub`.
fn handle_target(doc: &Doc) -> Option<(Sel, usize, Rec)> {
    let sel = doc.single()?;
    let rec = edit::get(&doc.map, sel.0, sel.1)?;
    match &rec {
        Rec::Mob(_) | Rec::Region(_) => Some((sel, 0, rec)),
        Rec::Bind(b) if !b.boxes.is_empty() => Some((sel, doc.sub.unwrap_or(0).min(b.boxes.len() - 1), rec)),
        _ => None,
    }
}

const ROT_RING: f32 = 24.0;

fn pick_all(c: &Ctx, xf: &Xf, p: Pos2) -> Vec<Hit> {
    let m = &c.doc.map;
    let mut out = Vec::new();
    let near = |q: Pos2, r: f32| (q.x - p.x).powi(2) + (q.y - p.y).powi(2) <= r * r;
    if selectable(c, SectionId::Npcs) {
        for (i, n) in m.npcs.iter().enumerate() {
            // path points first (they are on top of the line), then the pin
            let mut hit = None;
            for (k, pt) in n.points.iter().enumerate() {
                if near(xf.to_screen(pt.pos[0], pt.pos[2]), if k == 0 { 8.0 } else { 6.0 }) {
                    hit = Some(k);
                    if k == 0 {
                        break;
                    }
                }
            }
            if let Some(k) = hit {
                out.push(Hit { sec: SectionId::Npcs, idx: i, sub: Some(k) });
            }
        }
    }
    if selectable(c, SectionId::Portals) {
        for (i, pt) in m.portals.iter().enumerate() {
            if near(xf.to_screen(pt.pos[0], pt.pos[2]), 9.0) {
                out.push(Hit { sec: SectionId::Portals, idx: i, sub: None });
            }
        }
    }
    if selectable(c, SectionId::SectionB) {
        for (i, pt) in m.points_b.iter().enumerate() {
            if near(xf.to_screen(pt[0], pt[2]), 8.0) {
                out.push(Hit { sec: SectionId::SectionB, idx: i, sub: None });
            }
        }
    }
    let inside = |r: Rect| r.expand(1.0).contains(p);
    if selectable(c, SectionId::Binds) {
        let mut v = Vec::new();
        for (i, b) in m.binds.iter().enumerate() {
            for (k, x) in b.boxes.iter().enumerate() {
                let r = xf.rect(x.min, x.max);
                if inside(r) {
                    v.push((r.width() * r.height(), i, k));
                }
            }
        }
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(v.into_iter().map(|(_, i, k)| Hit { sec: SectionId::Binds, idx: i, sub: Some(k) }));
    }
    if selectable(c, SectionId::MobAreas) {
        let mut v = Vec::new();
        for (i, a) in m.mob_areas.iter().enumerate() {
            let r = xf.rect(a.min, a.max);
            if inside(r) && !level_hidden(c, a) {
                v.push((r.width() * r.height(), i));
            }
        }
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(v.into_iter().map(|(_, i)| Hit { sec: SectionId::MobAreas, idx: i, sub: None }));
    }
    if selectable(c, SectionId::Regions) {
        let mut v = Vec::new();
        for (i, g) in m.regions.iter().enumerate() {
            let r = xf.rect(g.min, g.max);
            if inside(r) {
                v.push((r.width() * r.height(), i));
            }
        }
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.extend(v.into_iter().map(|(_, i)| Hit { sec: SectionId::Regions, idx: i, sub: None }));
    }
    if selectable(c, SectionId::SectionA) {
        for (i, a) in m.section_a.iter().enumerate() {
            if near(xf.to_screen(a.pos[0], a.pos[2]), 8.0) {
                out.push(Hit { sec: SectionId::SectionA, idx: i, sub: None });
            }
        }
    }
    out
}

/// What the Placements tool shows: the selected placements as turned footprints with a heading line, the drag preview, the armed
/// model at the cursor, and a note when the tab has no editable client map.
#[allow(clippy::too_many_arguments)]
fn draw_places(p: &egui::Painter, rect: Rect, xf: &Xf, doc: &Doc, ui: &mut crate::place_ui::PlaceUi, src: Option<&shaiya_clientdata::ClientSource>, snap: f32, ground: Option<&crate::assets::Ground>, hover: Option<Pos2>) {
    use crate::place::{self, STAND_IN};
    let Some(part) = doc.part() else {
        let why = match &doc.client {
            crate::doc::ClientSlot::ReadOnly(w) => format!("The client map is read-only: {w}"),
            _ => "This tab has no client map to edit (open the client folder first).".to_string(),
        };
        p.text(rect.left_top() + Vec2::new(12.0, 40.0), Align2::LEFT_TOP, why, FontId::proportional(13.0), Color32::from_rgb(255, 190, 90));
        return;
    };
    let m = &part.wld.model;
    let quad = |q: [[f32; 2]; 4], col: Color32, w: f32| {
        let pts: Vec<Pos2> = q.iter().map(|v| xf.to_screen(v[0], v[1])).collect();
        p.add(egui::Shape::closed_line(pts, Stroke::new(w, col)));
    };
    let heading = |pl: &shaiya_clientdata::wld::Placement, col: Color32| {
        let a = xf.to_screen(pl.pos[0], pl.pos[2]);
        let b = xf.to_screen(pl.pos[0] + pl.dir[0] * 4.0, pl.pos[2] + pl.dir[2] * 4.0);
        p.line_segment([a, b], Stroke::new(2.0, col));
    };
    let mut shapes: Vec<(shaiya_clientdata::wld::Placement, place::Bounds, Color32)> = Vec::new();
    for s in &doc.place_sel {
        if let Some(pl) = place::get(m, *s) {
            let name = place::model_name(s.0.list(m), &pl).unwrap_or_default();
            let b = ui.cache.get(src, s.0, &name).unwrap_or(STAND_IN);
            shapes.push((pl, b, Color32::from_rgb(255, 235, 60)));
        }
    }
    if let Some(d) = &ui.drag {
        for op in crate::place_ui::drag_ops(m, d, snap, ground) {
            if let crate::pair::COp::Place(l, _, crate::pair::CKind::Replace { after, .. }) = op {
                let name = place::model_name(l.list(m), &after).unwrap_or_default();
                let b = ui.cache.get(src, l, &name).unwrap_or(STAND_IN);
                shapes.push((after, b, Color32::from_rgb(255, 150, 40)));
            }
        }
    }
    if let (Some((l, name)), Some(h)) = (ui.armed.clone(), hover) {
        let w = xf.to_world(h);
        let b = ui.cache.get(src, l, &name).unwrap_or(STAND_IN);
        shapes.push((place::upright(0, [w[0], 0.0, w[1]], ui.armed_yaw), b, Color32::from_rgb(120, 255, 120)));
    }
    for (pl, b, col) in &shapes {
        quad(place::footprint(pl, b), *col, 2.0);
        heading(pl, *col);
    }
}

fn dashed_rect(p: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.extend(Shape::dashed_line(&pts, stroke, 5.0, 4.0));
}

fn arrow(p: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    p.line_segment([from, to], stroke);
    let d = to - from;
    let len = d.length();
    if len > 6.0 {
        let u = d / len;
        let n = Vec2::new(-u.y, u.x);
        let tip = to;
        p.add(Shape::convex_polygon(vec![tip, tip - u * 9.0 + n * 4.0, tip - u * 9.0 - n * 4.0], stroke.color, Stroke::NONE));
    }
}

/// Findings per record, for outlines and tooltips.
pub fn finding_flags(doc: &Doc) -> HashMap<(SectionId, usize), Severity> {
    let mut m: HashMap<(SectionId, usize), Severity> = HashMap::new();
    for f in &doc.findings {
        if let Some(i) = f.index {
            let e = m.entry((f.section, i)).or_insert(f.severity);
            if f.severity > *e {
                *e = f.severity;
            }
        }
    }
    m
}

pub fn show(ui: &mut egui::Ui, c: &mut Ctx, st: &mut VpState) -> Out {
    let mut out = Out::default();
    if c.doc.read_only && !matches!(c.tool, Tool::Select | Tool::Measure) {
        c.tool = Tool::Select; // a tab to look at: nothing to draw or paint
    }
    if c.tool != Tool::Measure && st.measure.active() {
        st.measure.clear(); // a measurement stays until Esc, a new one or a tool change
    }
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
    st.size = rect.size();
    let map_size = c.doc.map.size as f32;
    if !c.doc.view.fitted {
        fit_view(c.doc, rect.size(), map_size);
    }

    // ---- camera: wheel zoom at the cursor, middle/right drag pans
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(p) = resp.hover_pos() {
                let xf = Xf::new(rect, c.doc.view.cx, c.doc.view.cz, c.doc.view.zoom);
                let before = xf.to_world(p);
                let z = (c.doc.view.zoom * (scroll * 0.0035).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
                c.doc.view.zoom = z;
                let xf2 = Xf::new(rect, c.doc.view.cx, c.doc.view.cz, z);
                let after = xf2.to_world(p);
                c.doc.view.cx += before[0] - after[0];
                c.doc.view.cz += before[1] - after[1];
            }
        }
    }
    if resp.dragged_by(egui::PointerButton::Middle) || (resp.dragged_by(egui::PointerButton::Secondary) && c.tool != Tool::Paint) {
        let d = resp.drag_delta();
        c.doc.view.cx -= d.x / c.doc.view.zoom;
        c.doc.view.cz += d.y / c.doc.view.zoom;
    }
    st.rect = rect;
    // ---- the overview inset: a click or drag in it moves the view and is not a click on the map
    let xf0 = Xf::new(rect, c.doc.view.cx, c.doc.view.cz, c.doc.view.zoom);
    let show_overview = c.overview && !c.clean && !whole_map_in_view(rect, &xf0, map_size);
    let ov = overview_rect(rect);
    let in_overview = show_overview && overview_interact(ui, c, st, ov, map_size);
    if !show_overview {
        st.ov_drag = false;
    }
    let xf = Xf::new(rect, c.doc.view.cx, c.doc.view.cz, c.doc.view.zoom);
    let hover_pos = resp.hover_pos();
    out.cursor = hover_pos.map(|p| xf.to_world(p));
    st.hover = out.cursor;

    // ---- the context menu: the right button released within 4 px and 400 ms of the press (anything else stays a pan)
    {
        let (pressed, released, pos, time) = ui.input(|i| (i.pointer.button_pressed(egui::PointerButton::Secondary), i.pointer.button_released(egui::PointerButton::Secondary), i.pointer.latest_pos(), i.time));
        if pressed && resp.hovered() && !in_overview {
            st.rpress = pos.map(|p| (p, time));
        }
        if released {
            if let (Some((p0, t0)), Some(p)) = (st.rpress.take(), pos) {
                let paints = c.tool == Tool::Paint;
                if !paints && c.pick_banner.is_none() && matches!(st.drag, Drag::None) && is_menu_click(p0, t0, p, time) {
                    let hit = pick_all(c, &xf, p).first().copied();
                    if let Some(h) = hit {
                        if !c.doc.is_selected(h.sec, h.idx) {
                            c.doc.select_only(h.sec, h.idx);
                            c.doc.sub = h.sub;
                        }
                    }
                    st.menu = Some(CtxMenu { pos: p, world: xf.to_world(p), hit });
                }
            }
        }
    }

    // ---- interaction
    let shift = ui.input(|i| i.modifiers.shift);
    let alt = ui.input(|i| i.modifiers.alt);
    let ctrl = ui.input(|i| i.modifiers.command);
    if !in_overview && st.menu.is_none() {
        interact(ui, c, st, &xf, &resp, shift, alt, ctrl, &mut out);
    }
    // what the compare layer says about the place under the cursor
    let compare_tip = match (c.layers.compare, c.compare, hover_pos) {
        (true, Some(o), Some(hp)) => {
            let w = xf.to_world(hp);
            let slack = 6.0 / xf.zoom;
            let inside = |b: (f32, f32, f32, f32)| w[0] >= b.0 - slack && w[0] <= b.2 + slack && w[1] >= b.1 - slack && w[1] <= b.3 + slack;
            o.items.iter().find(|i| i.cur_bounds.is_some_and(inside) || i.ref_bounds.is_some_and(inside)).map(|i| i.tip.clone())
        }
        _ => None,
    };
    if let (None, Some(t), Drag::None) = (out.hover_hit, &compare_tip, &st.drag) {
        resp.clone().on_hover_text_at_pointer(t.clone());
    }
    if let (Some(h), Drag::None) = (out.hover_hit, &st.drag) {
        let mut text = format!("{} #{}: {}", h.sec.label(), h.idx, names::label(&c.look, &c.doc.map, h.sec, h.idx, c.map_name));
        for f in c.doc.findings.iter().filter(|f| f.section == h.sec && f.index == Some(h.idx)) {
            text.push_str(&format!("
{}: {}", f.severity.label(), f.message));
        }
        if let Some(t) = &compare_tip {
            text.push('\n');
            text.push_str(t);
        }
        resp.clone().on_hover_text_at_pointer(text);
    }

    // ---- drawing
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(24, 26, 30));
    draw_layers(&painter, rect, &xf, c, st, hover_pos, &mut out);

    if let Some(m) = c.marker {
        // a crosshair at the position Go to moved to
        let p = xf.to_screen(m[0], m[1]);
        let stroke = Stroke::new(2.0, Color32::from_rgb(255, 230, 60));
        painter.circle_stroke(p, 14.0, stroke);
        painter.line_segment([p + Vec2::new(-24.0, 0.0), p + Vec2::new(-8.0, 0.0)], stroke);
        painter.line_segment([p + Vec2::new(8.0, 0.0), p + Vec2::new(24.0, 0.0)], stroke);
        painter.line_segment([p + Vec2::new(0.0, -24.0), p + Vec2::new(0.0, -8.0)], stroke);
        painter.line_segment([p + Vec2::new(0.0, 8.0), p + Vec2::new(0.0, 24.0)], stroke);
    }
    if let Some(b) = &c.pick_banner {
        let r = Rect::from_min_size(rect.left_top() + Vec2::new(8.0, 8.0), Vec2::new(rect.width() - 16.0, 24.0));
        painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(160, 110, 0, 230));
        painter.text(r.center(), Align2::CENTER_CENTER, b, FontId::proportional(13.0), Color32::WHITE);
    }
    if show_overview {
        draw_overview(&painter, ov, &xf, c, rect, map_size);
    }
    if c.rulers && !c.clean {
        draw_rulers(&painter, rect, &xf, hover_pos);
    }
    if let Some(m) = st.menu.clone() {
        let mut close = false;
        let menu_in = MenuIn { read_only: c.doc.read_only, can_paste: c.can_paste, template: c.template, fixes: c.menu_fixes };
        if let Some(a) = context_menu(ui.ctx(), &menu_in, &m, &mut close) {
            out.requests.push(Request::Menu(a));
        }
        if close {
            st.menu = None;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn interact(ui: &egui::Ui, c: &mut Ctx, st: &mut VpState, xf: &Xf, resp: &egui::Response, shift: bool, alt: bool, ctrl: bool, out: &mut Out) {
    let press_origin = ui.input(|i| i.pointer.press_origin());
    let latest = ui.input(|i| i.pointer.latest_pos());
    let primary = egui::PointerButton::Primary;
    let snap = c.snap;

    if c.tool == Tool::Paint && c.pick_banner.is_none() {
        paint_interact(ui, c, st, xf, resp);
        return;
    }

    if c.tool == Tool::Scatter && c.pick_banner.is_none() {
        scatter_interact(ui, c, st, xf, resp);
        return;
    }

    if c.tool == Tool::Place && c.pick_banner.is_none() {
        place_interact(ui, c, xf, resp, shift, alt, out);
        return;
    }

    if c.tool == Tool::Terrain && c.pick_banner.is_none() {
        terrain_interact(ui, c, xf, resp, shift, out);
        return;
    }

    if c.pick_banner.is_some() {
        if resp.clicked_by(primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                let w = xf.to_world(p);
                let y = c.look.ground(w[0], w[1]);
                out.requests.push(Request::PickDone([snap_to(w[0], snap), y, snap_to(w[1], snap)]));
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out.requests.push(Request::CancelPick);
        }
        return;
    }

    // ---- drag start
    if resp.drag_started_by(primary) {
        if let Some(p0) = press_origin {
            let w0 = xf.to_world(p0);
            match c.tool {
                Tool::Select => {
                    st.alt_drag = alt && c.doc.part().is_some();
                    start_select_drag(c, st, xf, p0, w0, shift)
                }
                Tool::Box => st.drag = Drag::Draw { start: [snap_to(w0[0], snap), snap_to(w0[1], snap)] },
                Tool::Measure => {
                    let a = measure_point(c, w0, None, false, ctrl);
                    let circle = alt || st.radius_mode;
                    st.drag = Drag::Measure { start: a, circle };
                    if circle {
                        st.measure.circle_to(a, a);
                    } else {
                        st.measure.segment(a, a);
                    }
                }
                _ => {}
            }
        }
    }

    // ---- drag update
    if resp.dragged_by(primary) {
        if let Some(p) = latest {
            let w = xf.to_world(p);
            update_drag(c, st, w, shift, ctrl);
        }
    }

    // ---- drag stop
    if resp.drag_stopped_by(primary) {
        let w = latest.map(|p| xf.to_world(p));
        finish_drag(c, st, w, shift, out);
    }

    // ---- click (press + release without drag)
    if resp.clicked_by(primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let w = xf.to_world(p);
            match c.tool {
                Tool::Select => {
                    let hits = pick_all(c, xf, p);
                    click_select(c, &hits, shift, alt);
                }
                Tool::Box => {
                    if c.draw_target == DrawTarget::PointB {
                        let (x, z) = (snap_to(w[0], snap), snap_to(w[1], snap));
                        let y = c.look.ground(x, z);
                        let idx = c.doc.map.points_b.len();
                        c.doc.commit("add section B point", vec![Op::insert(idx, Rec::B([x, y, z]))]);
                        c.doc.select_only(SectionId::SectionB, idx);
                    } else if let (DrawTarget::MobArea, Some(t)) = (c.draw_target, c.template) {
                        let look = c.look;
                        stamp_template(c.doc, &|x, z| look.ground(x, z), w, snap, t);
                    }
                }
                Tool::Npc => {
                    let (x, z) = (snap_to(w[0], snap), snap_to(w[1], snap));
                    let y = c.look.ground(x, z);
                    let n = NpcEntry { kind: c.npc_template.0, type_id: c.npc_template.1, points: vec![NpcPoint { pos: [x, y, z], dir: 0.0 }] };
                    let idx = c.doc.map.npcs.len();
                    c.doc.commit("add NPC", vec![Op::insert(idx, Rec::Npc(n))]);
                    c.doc.select_only(SectionId::Npcs, idx);
                }
                Tool::Portal => {
                    let (x, z) = (snap_to(w[0], snap), snap_to(w[1], snap));
                    out.requests.push(Request::AddPortal([x, c.look.ground(x, z), z]));
                }
                Tool::Paint | Tool::Scatter | Tool::Place | Tool::Terrain => {}
                Tool::Measure => {
                    // clicks build a chain; a click on the first point of a chain of 3 closes it; a double click ends it
                    let building = !st.measure.done && st.measure.circle.is_none();
                    let closes = building && st.measure.points.len() >= 3 && xf.to_screen(st.measure.points[0][0], st.measure.points[0][1]).distance(p) <= crate::measure::SNAP_PX;
                    if closes {
                        st.measure.close(c.doc.map.size);
                    } else {
                        let from = if building { st.measure.points.last().copied() } else { None };
                        let q = measure_point(c, w, from, shift, ctrl);
                        // the second click of a double click lands where the first one did
                        let again = building && from.is_some_and(|l| xf.to_screen(l[0], l[1]).distance(p) < 3.0);
                        if !again {
                            st.measure.click(q);
                        }
                        if resp.double_clicked_by(primary) {
                            st.measure.finish();
                        }
                    }
                }
            }
        }
    }
    if c.tool == Tool::Measure && !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            st.measure.finish();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Backspace)) {
            st.measure.pop();
        }
    }

    // ---- hover cursor + hit
    if matches!(st.drag, Drag::None) {
        if let Some(p) = resp.hover_pos() {
            if c.tool == Tool::Select {
                // handles first
                if let Some((sel, sub, rec)) = handle_target(c.doc) {
                    if let Some((min, max)) = box_of(&rec, sub) {
                        let r = xf.rect(min, max);
                        let _ = sel;
                        for (h, hp) in handle_points(r).iter().enumerate() {
                            if hp.distance(p) <= 7.0 {
                                let icon = match h {
                                    0 | 4 => egui::CursorIcon::ResizeNwSe,
                                    2 | 6 => egui::CursorIcon::ResizeNeSw,
                                    1 | 5 => egui::CursorIcon::ResizeVertical,
                                    _ => egui::CursorIcon::ResizeHorizontal,
                                };
                                ui.ctx().set_cursor_icon(icon);
                            }
                        }
                    }
                }
                let hits = pick_all(c, xf, p);
                out.hover_hit = hits.first().copied();
                if out.hover_hit.is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        // cancel a drag in progress, restoring the records
        cancel_drag(c, st);
        st.measure.clear();
    }
}

fn click_select(c: &mut Ctx, hits: &[Hit], shift: bool, alt: bool) {
    if hits.is_empty() {
        if !shift {
            c.doc.clear_selection();
        }
        return;
    }
    let mut pick = hits[0];
    if alt {
        // cycle: the hit after the one that is currently selected
        if let Some(cur) = hits.iter().position(|h| c.doc.is_selected(h.sec, h.idx)) {
            pick = hits[(cur + 1) % hits.len()];
        }
    }
    if shift {
        c.doc.toggle(pick.sec, pick.idx);
    } else {
        c.doc.select_only(pick.sec, pick.idx);
        c.doc.sub = pick.sub;
    }
}

fn start_select_drag(c: &mut Ctx, st: &mut VpState, xf: &Xf, p0: Pos2, w0: [f32; 2], shift: bool) {
    // a read-only tab only selects: no handles, no moving
    let read_only = c.doc.read_only;
    // handles of a single selected box, and the rotation handle of a single selected NPC
    if let Some((sel, sub, rec)) = handle_target(c.doc).filter(|_| !read_only) {
        if let Some((min, max)) = box_of(&rec, sub) {
            let r = xf.rect(min, max);
            for (h, hp) in handle_points(r).iter().enumerate() {
                if hp.distance(p0) <= 7.0 {
                    st.drag = Drag::Resize { sel, sub, handle: h, before: rec };
                    return;
                }
            }
        }
    }
    if let Some(sel) = c.doc.single().filter(|_| !read_only) {
        if sel.0 == SectionId::Npcs {
            if let Some(Rec::Npc(n)) = edit::get(&c.doc.map, sel.0, sel.1) {
                if let Some(pt) = n.points.first() {
                    let pin = xf.to_screen(pt.pos[0], pt.pos[2]);
                    let tip = pin + Vec2::new(pt.dir.sin(), -pt.dir.cos()) * ROT_RING;
                    if tip.distance(p0) <= 8.0 {
                        st.drag = Drag::Rotate { sel, before: Rec::Npc(n) };
                        return;
                    }
                }
            }
        }
    }
    let hits = pick_all(c, xf, p0);
    let Some(hit) = hits.first().copied() else {
        st.drag = Drag::Band { start: w0, add: shift };
        return;
    };
    // a path point (not the first) of an NPC moves alone
    if hit.sec == SectionId::Npcs {
        if let Some(k) = hit.sub.filter(|k| *k > 0 && !read_only) {
            if let Some(rec) = edit::get(&c.doc.map, hit.sec, hit.idx) {
                c.doc.select_only(hit.sec, hit.idx);
                c.doc.sub = Some(k);
                st.drag = Drag::MovePoint { sel: (hit.sec, hit.idx), point: k, before: rec };
                return;
            }
        }
    }
    if !c.doc.is_selected(hit.sec, hit.idx) {
        if shift {
            c.doc.toggle(hit.sec, hit.idx);
        } else {
            c.doc.select_only(hit.sec, hit.idx);
        }
    }
    if hit.sec == SectionId::Binds {
        c.doc.sub = hit.sub;
    }
    if read_only {
        return;
    }
    let snaps: Vec<(Sel, Rec)> = c.doc.selection.iter().filter_map(|s| edit::get(&c.doc.map, s.0, s.1).map(|r| (*s, r))).collect();
    let anchor0 = snaps.iter().find(|(s, _)| *s == (hit.sec, hit.idx)).map(|(_, r)| anchor_of(r)).unwrap_or(w0);
    st.drag = Drag::Move { snaps, press: w0, anchor0 };
}

fn update_drag(c: &mut Ctx, st: &mut VpState, w: [f32; 2], shift: bool, ctrl: bool) {
    let snap = c.snap;
    let look = c.look;
    match &mut st.drag {
        Drag::Move { snaps, press, anchor0 } => {
            let want = [snap_to(anchor0[0] + (w[0] - press[0]), snap), snap_to(anchor0[1] + (w[1] - press[1]), snap)];
            let (dx, dz) = (want[0] - anchor0[0], want[1] - anchor0[1]);
            for ((s, i), before) in snaps.iter() {
                let mut r = before.clone();
                if ctrl {
                    // Ctrl: y only (the cursor's vertical movement is not meaningful in 2D; keep x/z)
                } else {
                    edit::translate(&mut r, dx, dz);
                }
                if c.drop_on_move {
                    edit::drop_to_ground(&mut r, &|x, z| look.ground(x, z));
                }
                edit::set(&mut c.doc.map, *i, r);
                let _ = s;
            }
        }
        Drag::MovePoint { sel, point, before } => {
            let (x, z) = (snap_to(w[0], snap), snap_to(w[1], snap));
            let mut r = before.clone();
            if let Rec::Npc(n) = &mut r {
                if let Some(pt) = n.points.get_mut(*point) {
                    pt.pos[0] = x;
                    pt.pos[2] = z;
                    if c.drop_on_move {
                        pt.pos[1] = look.ground(x, z);
                    }
                }
            }
            edit::set(&mut c.doc.map, sel.1, r);
        }
        Drag::Resize { sel, sub, handle, before } => {
            if let Some((min0, max0)) = box_of(before, *sub) {
                let (l, r, t, b) = handle_edges(*handle);
                let (mut x0, mut x1) = (min0[0].min(max0[0]), min0[0].max(max0[0]));
                let (mut z0, mut z1) = (min0[2].min(max0[2]), min0[2].max(max0[2]));
                if l {
                    x0 = snap_to(w[0], snap);
                }
                if r {
                    x1 = snap_to(w[0], snap);
                }
                if t {
                    z1 = snap_to(w[1], snap);
                }
                if b {
                    z0 = snap_to(w[1], snap);
                }
                let mut rec = before.clone();
                set_box(&mut rec, *sub, [x0, min0[1], z0], [x1, max0[1], z1]);
                edit::set(&mut c.doc.map, sel.1, rec);
            }
        }
        Drag::Rotate { sel, before } => {
            if let Rec::Npc(n) = before {
                let mut n = n.clone();
                if let Some(pt) = n.points.first_mut() {
                    let (dx, dz) = (w[0] - pt.pos[0], w[1] - pt.pos[2]);
                    if dx.abs() + dz.abs() > 1e-4 {
                        let mut a = dx.atan2(dz);
                        if a < 0.0 {
                            a += std::f32::consts::TAU;
                        }
                        if shift {
                            let step = 15f32.to_radians();
                            a = (a / step).round() * step;
                        }
                        pt.dir = a;
                    }
                }
                edit::set(&mut c.doc.map, sel.1, Rec::Npc(n));
            }
        }
        Drag::Measure { start, circle } => {
            let end = measure_point(c, w, Some(*start), shift, ctrl);
            if *circle {
                st.measure.circle_to(*start, end);
            } else {
                st.measure.segment(*start, end);
            }
        }
        Drag::Draw { .. } | Drag::Band { .. } | Drag::Paint { .. } | Drag::PaintRect { .. } | Drag::None => {}
    }
}

fn cancel_drag(c: &mut Ctx, st: &mut VpState) {
    st.alt_drag = false;
    match std::mem::replace(&mut st.drag, Drag::None) {
        Drag::Move { snaps, .. } => {
            for ((_, i), r) in snaps {
                edit::set(&mut c.doc.map, i, r);
            }
        }
        Drag::MovePoint { sel, before, .. } | Drag::Resize { sel, before, .. } | Drag::Rotate { sel, before } => {
            edit::set(&mut c.doc.map, sel.1, before);
        }
        Drag::Paint { before, .. } => {
            let s = c.doc.map.size;
            c.doc.map.blocked = *before;
            c.doc.mark_blocked_dirty((0, 0, s, s));
        }
        _ => {}
    }
}

/// Commits the ops of a finished drag. With Alt held the client half stays as it is, and a toast says so (only when a paired
/// section was touched).
fn commit_drag(c: &mut Ctx, alt: bool, out: &mut Out, label: String, ops: Vec<Op>) {
    let paired = alt && c.doc.part().is_some_and(|p| {
        let flags = p.paired(&c.doc.map);
        ops.iter().any(|o| crate::pair::slot(o.section()).is_some_and(|k| flags[k]))
    });
    if paired {
        c.doc.commit_server_only(label, ops);
        out.requests.push(Request::Toast("Alt+drag changed the svmap only. The client map did not move.".to_string()));
    } else {
        c.doc.commit(label, ops);
    }
}

fn finish_drag(c: &mut Ctx, st: &mut VpState, w: Option<[f32; 2]>, shift: bool, out: &mut Out) {
    let snap = c.snap;
    let alt = std::mem::take(&mut st.alt_drag);
    match std::mem::replace(&mut st.drag, Drag::None) {
        Drag::Move { snaps, .. } => {
            let ops = move_ops(&snaps, &c.doc.map);
            if !ops.is_empty() {
                let label = if snaps.len() == 1 { format!("move {}", snaps[0].0 .0.label()) } else { format!("move {} records", snaps.len()) };
                commit_drag(c, alt, out, label, ops);
            }
        }
        Drag::MovePoint { sel, point, before, .. } => {
            if let Some(after) = edit::get(&c.doc.map, sel.0, sel.1) {
                if after != before {
                    commit_drag(c, alt, out, format!("move NPC point {point}"), vec![Op::replace(sel.1, before, after)]);
                }
            }
        }
        Drag::Resize { sel, sub, before, .. } => {
            if let Some(mut after) = edit::get(&c.doc.map, sel.0, sel.1) {
                if let Some((min, max)) = box_of(&after, sub) {
                    // normalise a box dragged inside out
                    let mn = [min[0].min(max[0]), min[1].min(max[1]), min[2].min(max[2])];
                    let mx = [min[0].max(max[0]), min[1].max(max[1]), min[2].max(max[2])];
                    set_box(&mut after, sub, mn, mx);
                }
                if after != before {
                    let label = format!("resize {}", sel.0.label());
                    edit::set(&mut c.doc.map, sel.1, before.clone());
                    commit_drag(c, alt, out, label, vec![Op::replace(sel.1, before, after)]);
                }
            }
        }
        Drag::Rotate { sel, before } => {
            if let Some(after) = edit::get(&c.doc.map, sel.0, sel.1) {
                if after != before {
                    commit_drag(c, alt, out, "rotate NPC".to_string(), vec![Op::replace(sel.1, before, after)]);
                }
            }
        }
        Drag::Band { start, add } => {
            if let Some(end) = w {
                let (x0, x1) = (start[0].min(end[0]), start[0].max(end[0]));
                let (z0, z1) = (start[1].min(end[1]), start[1].max(end[1]));
                let mut found: Vec<Sel> = if add || shift { c.doc.selection.clone() } else { Vec::new() };
                for s in edit::LIST_SECTIONS {
                    if !selectable(c, s) {
                        continue;
                    }
                    for i in 0..edit::count(&c.doc.map, s) {
                        if let Some(r) = edit::get(&c.doc.map, s, i) {
                            if matches!(&r, Rec::Mob(a) if level_hidden(c, a)) {
                                continue;
                            }
                            let a = anchor_of(&r);
                            let (a0, b0, a1, b1) = edit::bounds_xz(&r);
                            let ctr = [(a0 + a1) * 0.5, (b0 + b1) * 0.5];
                            let p = if matches!(r, Rec::Mob(_) | Rec::Region(_) | Rec::Bind(_)) { ctr } else { a };
                            if p[0] >= x0 && p[0] <= x1 && p[1] >= z0 && p[1] <= z1 && !found.contains(&(s, i)) {
                                found.push((s, i));
                            }
                        }
                    }
                }
                c.doc.selection = found;
                c.doc.sub = None;
            }
        }
        Drag::Draw { start } => {
            if let Some(end) = w {
                let end = [snap_to(end[0], snap), snap_to(end[1], snap)];
                let look = c.look;
                if let Some(t) = commit_drawn_box(c.doc, &|x, z| look.ground(x, z), start, end, c.draw_target, c.template) {
                    out.requests.push(Request::Toast(t.to_string()));
                }
            }
        }
        Drag::Measure { start, circle } => {
            // the last drag update already holds the end point (with its snapping); a drag of no length leaves nothing
            let _ = (start, w);
            let empty = if circle { st.measure.circle.is_none_or(|(_, r)| r < 1e-3) } else { st.measure.total() < 1e-3 };
            if empty {
                st.measure.clear();
            } else {
                st.measure.finish();
            }
        }
        Drag::Paint { .. } | Drag::PaintRect { .. } => {}
        Drag::None => {}
    }
}

// ---------------------------------------------------------------------------------------------------------------- blocked cells

/// Marks the cells a brush dab of `radius` at (x, z) can touch as changed (for the overlay texture).
fn dirty_dab(doc: &mut Doc, x: f32, z: f32, radius: f32) {
    let s = doc.map.size as f32;
    let (x0, x1) = ((x - radius - 2.0).floor().clamp(0.0, s), (x + radius + 2.0).ceil().clamp(0.0, s));
    let (z0, z1) = ((z - radius - 2.0).floor().clamp(0.0, s), (z + radius + 2.0).ceil().clamp(0.0, s));
    doc.mark_blocked_dirty((x0 as u32, z0 as u32, x1 as u32, z1 as u32));
}

/// Paints from `from` to `to` (world x, z; one cell is one world unit) in steps of half a cell so a fast stroke leaves no gaps.
fn stroke(doc: &mut Doc, from: [f32; 2], to: [f32; 2], radius: f32, value: bool) {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    let steps = ((dx * dx + dz * dz).sqrt() / 0.5).ceil().max(1.0) as usize;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let (x, z) = (from[0] + dx * t, from[1] + dz * t);
        doc.map.blocked.fill_circle(x, z, radius, value);
        dirty_dab(doc, x, z, radius);
    }
}

/// The cells (half-open) of the rectangle dragged between two world points, aligned to whole cells.
pub fn cell_rect(a: [f32; 2], b: [f32; 2]) -> (i64, i64, i64, i64) {
    let (x0, x1) = (a[0].min(b[0]).floor() as i64, a[0].max(b[0]).floor() as i64 + 1);
    let (z0, z1) = (a[1].min(b[1]).floor() as i64, a[1].max(b[1]).floor() as i64 + 1);
    (x0, z0, x1, z1)
}

fn paint_label(value: bool) -> &'static str {
    if value {
        "block cells"
    } else {
        "free cells"
    }
}

/// The measurement: the chain with each segment's length at its middle and the summary at the end, or the circle with its radius
/// (and the faint portal-range ring while the radius is close to 40).
fn draw_measure(p: &egui::Painter, xf: &Xf, c: &Ctx, st: &VpState, hover_pos: Option<Pos2>, font: &FontId) {
    let m = &st.measure;
    let line = Color32::from_rgb(255, 120, 255);
    let text = Color32::from_rgb(255, 160, 255);
    if let Some((ctr, r)) = m.circle {
        let sc = xf.to_screen(ctr[0], ctr[1]);
        let rp = r * xf.zoom;
        p.circle_filled(sc, rp, Color32::from_rgba_unmultiplied(255, 120, 255, 22));
        if (r - crate::measure::RING).abs() <= crate::measure::RING_SLACK {
            p.circle_stroke(sc, crate::measure::RING * xf.zoom, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 80)));
        }
        p.circle_stroke(sc, rp, Stroke::new(1.5, line));
        p.line_segment([sc, sc + Vec2::new(rp, 0.0)], Stroke::new(1.0, line));
        p.circle_filled(sc, 3.0, line);
        let near = (r - crate::measure::RING).abs() <= crate::measure::RING_SLACK;
        let label = if near { format!("radius {r:.1} (portal range)") } else { format!("radius {r:.1}") };
        plate_text(p, sc + Vec2::new(rp + 6.0, 0.0), Align2::LEFT_CENTER, label, font.clone(), text);
        return;
    }
    let chain = m.chain();
    // while clicking, a rubber band runs from the last point to the cursor
    let live = match (m.done, &st.drag, hover_pos, m.points.last()) {
        (false, Drag::None, Some(hp), Some(last)) => {
            let (shift, ctrl) = p.ctx().input(|i| (i.modifiers.shift, i.modifiers.command));
            Some(measure_point(c, xf.to_world(hp), Some(*last), shift, ctrl))
        }
        _ => None,
    };
    let mut pts = chain.clone();
    if let Some(l) = live {
        pts.push(l);
    }
    let screen: Vec<Pos2> = pts.iter().map(|q| xf.to_screen(q[0], q[1])).collect();
    for (i, w) in screen.windows(2).enumerate() {
        let is_live = live.is_some() && i + 2 == screen.len();
        p.line_segment([w[0], w[1]], Stroke::new(if is_live { 1.0 } else { 1.5 }, line));
        if w[0].distance(w[1]) >= 24.0 {
            let len = crate::measure::dist(pts[i], pts[i + 1]);
            plate_text(p, Pos2::new((w[0].x + w[1].x) * 0.5, (w[0].y + w[1].y) * 0.5) + Vec2::new(6.0, -6.0), Align2::LEFT_BOTTOM, format!("{len:.1}"), font.clone(), text);
        }
    }
    for (i, s) in screen.iter().enumerate() {
        if live.is_some() && i + 1 == screen.len() {
            p.circle_stroke(*s, 3.0, Stroke::new(1.0, line));
        } else if m.closed && i + 1 == screen.len() {
            // the closing point is the first one again
        } else {
            p.circle_filled(*s, 3.0, line);
        }
    }
    let mut shown = m.clone();
    if let Some(l) = live {
        shown.points.push(l);
    }
    if let (Some(end), Some(sum)) = (screen.last(), shown.summary(&|x, z| c.look.ground(x, z))) {
        plate_text(p, *end + Vec2::new(10.0, 10.0), Align2::LEFT_TOP, sum, font.clone(), Color32::WHITE);
    }
}

/// The lowest ground under a box (its corners and centre): where a new slab sits.
fn lowest_ground(ground: &dyn Fn(f32, f32) -> f32, x0: f32, z0: f32, w: f32, d: f32) -> f32 {
    let mut lo = f32::MAX;
    for (fx, fz) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
        lo = lo.min(ground(x0 + w * fx, z0 + d * fz));
    }
    lo
}

/// A click with the Box tool and a template: a box of the template's size centred at the snapped point, on the ground. One undo
/// step; a read-only document refuses it. Shared by the 2D and 3D views.
pub fn stamp_template(doc: &mut Doc, ground: &dyn Fn(f32, f32) -> f32, w: [f32; 2], snap: f32, t: &Template) {
    let (cx, cz) = (snap_to(w[0], snap), snap_to(w[1], snap));
    let (x0, z0) = (cx - t.size[0] * 0.5, cz - t.size[1] * 0.5);
    let lo = lowest_ground(ground, x0, z0, t.size[0], t.size[1]);
    let idx = doc.map.mob_areas.len();
    doc.commit("stamp monster area from template", vec![Op::insert(idx, Rec::Mob(t.area_at(x0, lo, z0)))]);
    if doc.map.mob_areas.len() > idx {
        doc.select_only(SectionId::MobAreas, idx);
    }
}

/// The Box tool's drag ended: adds the box between two snapped corners as a monster area (with the template's entries and height
/// when there is one), a bind area or a region, on the lowest ground under it, as one undo step. Returns a toast text when the
/// user needs a word about it. Does nothing for a box under half a unit or on a read-only document (`commit` refuses it).
/// Shared by the 2D and 3D views.
pub fn commit_drawn_box(doc: &mut Doc, ground: &dyn Fn(f32, f32) -> f32, start: [f32; 2], end: [f32; 2], target: DrawTarget, template: Option<&Template>) -> Option<&'static str> {
    let (x0, x1) = (start[0].min(end[0]), start[0].max(end[0]));
    let (z0, z1) = (start[1].min(end[1]), start[1].max(end[1]));
    if x1 - x0 < 0.5 || z1 - z0 < 0.5 {
        return None;
    }
    let y0 = lowest_ground(ground, x0, z0, x1 - x0, z1 - z0);
    let (min, max) = ([x0, y0, z0], [x1, y0 + 2.0, z1]);
    match target {
        DrawTarget::MobArea => {
            let idx = doc.map.mob_areas.len();
            // with a template the drawn box takes its entries and slab height
            let (max, mobs) = match template {
                Some(t) => ([x1, y0 + t.height, z1], t.area_at(0.0, 0.0, 0.0).mobs),
                None => (max, Vec::new()),
            };
            doc.commit(if template.is_some() { "add monster area from template" } else { "add monster area" }, vec![Op::insert(idx, Rec::Mob(MobArea { min, max, mobs }))]);
            doc.select_only(SectionId::MobAreas, idx);
            None
        }
        DrawTarget::Bind => {
            let idx = doc.map.binds.len();
            doc.commit("add bind area", vec![Op::insert(idx, Rec::Bind(Bind { country: 0, attr: 0, boxes: vec![edit::aabb(min, max)] }))]);
            doc.select_only(SectionId::Binds, idx);
            None
        }
        DrawTarget::Region => {
            let idx = doc.map.regions.len();
            doc.commit("add region", vec![Op::insert(idx, Rec::Region(Region { min, max, kind: 0, b: 0, c: 0, d: 0 }))]);
            doc.select_only(SectionId::Regions, idx);
            Some("Added a region: the client's .wld must have the same number of areas")
        }
        DrawTarget::PointB => None,
    }
}

/// A right button release that counts as a click (it moved at most 4 px and lasted at most 400 ms) opens the context menu; any
/// other release ends a pan.
pub fn is_menu_click(p0: Pos2, t0: f64, p: Pos2, t: f64) -> bool {
    p.distance(p0) <= 4.0 && t - t0 <= 0.4
}

/// A point for the Measure tool: it snaps to a record anchor within 8 px (Ctrl turns that off), else Shift snaps the step from
/// `from` to 15 degrees.
fn measure_point(c: &Ctx, w: [f32; 2], from: Option<[f32; 2]>, shift: bool, ctrl: bool) -> [f32; 2] {
    if !ctrl {
        if let Some(a) = crate::measure::snap_in_map(&c.doc.map, w, c.doc.view.zoom, crate::measure::SNAP_PX) {
            return a;
        }
    }
    match from {
        Some(f) if shift => crate::measure::snap_angle(f, w, 15.0),
        _ => w,
    }
}

/// The Terrain tool in 2D: a drag draws a stroke (live, one undo step at release), a click is one dab, Esc cancels the stroke,
/// `[` and `]` change the radius. Shift turns Raise into Lower. Nothing here writes a file.
fn terrain_interact(ui: &egui::Ui, c: &mut Ctx, xf: &Xf, resp: &egui::Response, shift: bool, out: &mut Out) {
    use crate::terrain_ui::TerrainUi;
    let primary = egui::PointerButton::Primary;
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    c.terrain.hover = ui.input(|i| i.pointer.latest_pos()).filter(|p| resp.rect.contains(*p)).map(|p| xf.to_world(p)).or(c.terrain.pin);
    if TerrainUi::check(c.doc).is_err() {
        return;
    }
    let toast = |out: &mut Out, r: Result<Option<crate::terrain_ui::Report>, String>| match r {
        Ok(Some(rep)) => out.requests.push(Request::Toast(rep.message())),
        Ok(None) => {}
        Err(e) => out.requests.push(Request::Toast(e)),
    };
    if resp.drag_started_by(primary) {
        if let Some(p0) = ui.input(|i| i.pointer.press_origin()) {
            if let Err(e) = c.terrain.begin(c.doc, xf.to_world(p0), shift) {
                out.requests.push(Request::Toast(e));
            }
        }
    }
    if resp.dragged_by(primary) && c.terrain.stroke.is_some() {
        if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
            let steps = (ui.input(|i| i.stable_dt) * 60.0).clamp(0.25, 4.0);
            c.terrain.drag(c.doc, xf.to_world(p), steps);
            ui.ctx().request_repaint();
        }
    }
    if c.terrain.stroke.is_some() && (resp.drag_stopped_by(primary) || !ui.input(|i| i.pointer.primary_down())) {
        let r = c.terrain.finish(c.doc);
        toast(out, r);
    }
    if resp.clicked_by(primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            match c.terrain.begin(c.doc, xf.to_world(p), shift) {
                Ok(()) => {
                    let r = c.terrain.finish(c.doc);
                    toast(out, r);
                }
                Err(e) => out.requests.push(Request::Toast(e)),
            }
        }
    }
    if !ui.ctx().egui_wants_keyboard_input() {
        let (open, close) = ui.input(|i| (i.key_pressed(egui::Key::OpenBracket), i.key_pressed(egui::Key::CloseBracket)));
        if open || close {
            let r = &mut c.terrain.params.radius;
            *r = (*r * if close { 1.25 } else { 0.8 }).clamp(crate::terrain::MIN_RADIUS, crate::terrain::MAX_RADIUS);
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        c.terrain.abort(c.doc);
    }
}

/// What the Terrain tool shows: the brush ring (outer radius, and the core inside it that keeps full strength), the ramp line,
/// the cells a stroke would change by slope (red blocks, green frees) and why the tool cannot edit this tab.
fn draw_terrain(p: &egui::Painter, rect: Rect, xf: &Xf, doc: &Doc, terrain: &mut crate::terrain_ui::TerrainUi) {
    use crate::terrain_ui::{TerrainUi, PREVIEW_DRAW_MAX};
    if let Err(why) = TerrainUi::check(doc) {
        let r = Rect::from_min_size(rect.left_top() + Vec2::new(8.0, 8.0), Vec2::new(rect.width() - 16.0, 24.0));
        p.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(160, 110, 0, 230));
        p.text(r.center(), Align2::CENTER_CENTER, format!("Terrain: {why}."), FontId::proportional(13.0), Color32::WHITE);
        return;
    }
    terrain.update_preview(doc);
    for (x, z, blocked) in terrain.preview.iter().take(PREVIEW_DRAW_MAX) {
        let (x, z) = (*x as f32, *z as f32);
        let col = if *blocked { Color32::from_rgba_unmultiplied(230, 60, 60, 130) } else { Color32::from_rgba_unmultiplied(60, 200, 90, 130) };
        p.rect_filled(xf.rect([x, 0.0, z], [x + 1.0, 0.0, z + 1.0]), 0.0, col);
    }
    let prm = terrain.params;
    if let Some(at) = terrain.hover {
        let centre = xf.to_screen(at[0], at[1]);
        p.circle_stroke(centre, prm.radius * xf.zoom, Stroke::new(1.5, Color32::from_rgb(255, 235, 60)));
        let core = prm.radius * (1.0 - prm.falloff.clamp(0.0, 1.0)) * xf.zoom;
        if core > 2.0 {
            p.circle_stroke(centre, core, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 170)));
        }
    }
    if let (Some(st), Some(at)) = (terrain.stroke_ramp(), terrain.hover) {
        p.line_segment([xf.to_screen(st[0], st[1]), xf.to_screen(at[0], at[1])], Stroke::new(2.0, Color32::from_rgb(255, 150, 40)));
    }
}

/// The Placements tool (C3): click picks, drag moves, a click with a model armed adds it, R turns, Esc cancels or clears.
fn place_interact(ui: &egui::Ui, c: &mut Ctx, xf: &Xf, resp: &egui::Response, shift: bool, alt: bool, out: &mut Out) {
    use crate::place_ui as pu;
    let primary = egui::PointerButton::Primary;
    ui.ctx().set_cursor_icon(if c.place.armed.is_some() { egui::CursorIcon::Crosshair } else { egui::CursorIcon::Default });
    if c.doc.part().is_none() {
        return;
    }
    let src = c.look.rules.map(|r| &r.source);
    let ground = pu::field_ground(c.look.assets);
    let snap = c.snap;
    if resp.drag_started_by(primary) && c.place.armed.is_none() {
        if let Some(p0) = ui.input(|i| i.pointer.press_origin()) {
            let w0 = xf.to_world(p0);
            let hits = pu::pick_xz(c.doc, c.place, src, w0);
            if let Some(h) = hits.first() {
                pu::begin_drag(c.doc, c.place, *h, w0, shift);
            }
        }
    }
    if resp.dragged_by(primary) {
        if let (Some(p), Some(d)) = (ui.input(|i| i.pointer.latest_pos()), c.place.drag.as_mut()) {
            let w = xf.to_world(p);
            d.delta = [w[0] - d.press[0], w[1] - d.press[1]];
        }
    }
    if resp.drag_stopped_by(primary) {
        if let Some(why) = pu::finish_drag(c.doc, c.place, snap, ground) {
            out.requests.push(Request::Toast(why));
        }
    }
    if resp.clicked_by(primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let w = xf.to_world(p);
            if c.place.armed.is_some() {
                if let Err(why) = pu::add_at(c.doc, c.place, w, None, snap, ground, c.codepage) {
                    out.requests.push(Request::Toast(why));
                }
            } else {
                let hits = pu::pick_xz(c.doc, c.place, src, w);
                pu::click_select(c.doc, &hits, shift, alt);
            }
        }
    }
    if !ui.ctx().egui_wants_keyboard_input() {
        // Ctrl+R is Rulers
        let turn = ui.input(|i| i.key_pressed(egui::Key::R) && !i.modifiers.command && !i.modifiers.alt);
        if turn {
            if let Err(why) = pu::rotate_selected(c.doc, if shift { -15.0 } else { 15.0 }) {
                out.requests.push(Request::Toast(why));
            }
        }
    }
    if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Escape)) && c.place.drag.take().is_none() && c.place.armed.take().is_none() {
        c.doc.place_sel.clear();
    }
}

/// The Scatter tool: a drag draws a rectangle, clicks add polygon points (a double click or a click on the first point closes it).
/// The keys (Enter, Backspace, R, Esc) are the app's. Nothing here edits the map.
fn scatter_interact(ui: &egui::Ui, c: &mut Ctx, st: &mut VpState, xf: &Xf, resp: &egui::Response) {
    let primary = egui::PointerButton::Primary;
    let press_origin = ui.input(|i| i.pointer.press_origin());
    let latest = ui.input(|i| i.pointer.latest_pos());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let snap = c.snap;
    let world = |p: Pos2| {
        let w = xf.to_world(p);
        [snap_to(w[0], snap), snap_to(w[1], snap)]
    };
    if resp.drag_started_by(primary) {
        if let Some(p0) = press_origin {
            st.scatter.poly.clear();
            st.scatter.drag = Some((world(p0), world(p0)));
        }
    }
    if resp.dragged_by(primary) {
        if let (Some(p), Some(d)) = (latest, st.scatter.drag.as_mut()) {
            d.1 = world(p);
        }
    }
    if resp.drag_stopped_by(primary) {
        if let Some((a, b)) = st.scatter.drag.take() {
            if (a[0] - b[0]).abs() >= 1.0 && (a[1] - b[1]).abs() >= 1.0 {
                st.scatter.set(crate::scatter::Region::rect(a, b));
            }
        }
    }
    if resp.clicked_by(primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let w = world(p);
            // a click on the first point closes the polygon
            let closes = st.scatter.poly.len() >= 3 && xf.to_screen(st.scatter.poly[0][0], st.scatter.poly[0][1]).distance(p) <= 8.0;
            if closes {
                st.scatter.close_poly();
            } else {
                if st.scatter.region.is_some() {
                    st.scatter.clear(); // a click after a finished region starts another one
                }
                // the second click of a double click lands on the first one's point
                if st.scatter.poly.last() != Some(&w) {
                    st.scatter.poly.push(w);
                }
                if resp.double_clicked_by(primary) {
                    st.scatter.close_poly();
                }
            }
        }
    }
}

/// The Scatter tool's region, the polygon being clicked, the rectangle being dragged and the preview boxes; Bulk edit's matches.
fn draw_scatter(p: &egui::Painter, rect: Rect, xf: &Xf, c: &Ctx, st: &VpState, hover_pos: Option<Pos2>) {
    let font = FontId::proportional(12.0);
    let green = Color32::from_rgb(80, 220, 130);
    let fill = Color32::from_rgba_unmultiplied(80, 200, 120, 28);
    // Bulk edit: the areas the rule matches
    for &i in c.bulk_outline {
        if let Some(a) = c.doc.map.mob_areas.get(i) {
            let r = Rect::from_two_pos(xf.to_screen(a.min[0], a.min[2]), xf.to_screen(a.max[0], a.max[2]));
            let cyan = Color32::from_rgb(70, 220, 240);
            if r.width() < 8.0 && r.height() < 8.0 {
                p.circle_stroke(r.center(), 6.0, Stroke::new(1.5, cyan));
            } else {
                p.rect_stroke(r.expand(1.0), 0.0, Stroke::new(1.5, cyan), StrokeKind::Outside);
            }
        }
    }
    if c.tool != Tool::Scatter && c.scatter.is_none() {
        return;
    }
    if let Some(r) = &st.scatter.region {
        for s in &r.shapes {
            let pts: Vec<Pos2> = match s {
                crate::scatter::Shape::Rect(b) => vec![xf.to_screen(b[0], b[1]), xf.to_screen(b[2], b[1]), xf.to_screen(b[2], b[3]), xf.to_screen(b[0], b[3])],
                crate::scatter::Shape::Poly(v) => v.iter().map(|q| xf.to_screen(q[0], q[1])).collect(),
            };
            if pts.len() >= 3 {
                // a rectangle is filled; a polygon (maybe concave) is only outlined
                if matches!(s, crate::scatter::Shape::Rect(_)) {
                    p.rect_filled(Rect::from_two_pos(pts[0], pts[2]), 0.0, fill);
                }
                let mut closed = pts.clone();
                closed.push(pts[0]);
                p.extend(Shape::dashed_line(&closed, Stroke::new(1.5, green), 6.0, 4.0));
            }
        }
    }
    if let Some((a, b)) = st.scatter.drag {
        let r = Rect::from_two_pos(xf.to_screen(a[0], a[1]), xf.to_screen(b[0], b[1]));
        p.rect_filled(r, 0.0, fill);
        p.rect_stroke(r, 0.0, Stroke::new(1.5, green), StrokeKind::Inside);
        plate_text(p, r.right_bottom() + Vec2::new(4.0, 4.0), Align2::LEFT_TOP, format!("{:.1} x {:.1}", (b[0] - a[0]).abs(), (b[1] - a[1]).abs()), font.clone(), Color32::WHITE);
    }
    if !st.scatter.poly.is_empty() {
        let pts: Vec<Pos2> = st.scatter.poly.iter().map(|q| xf.to_screen(q[0], q[1])).collect();
        p.add(Shape::line(pts.clone(), Stroke::new(1.5, green)));
        if let (Some(last), Some(hp)) = (pts.last(), hover_pos) {
            p.line_segment([*last, hp], Stroke::new(1.0, Color32::from_rgba_unmultiplied(80, 220, 130, 140)));
        }
        for (k, q) in pts.iter().enumerate() {
            p.circle_filled(*q, if k == 0 { 5.0 } else { 3.0 }, green);
        }
    }
    if let Some(s) = &c.scatter {
        let stroke = Stroke::new(1.2, green);
        for a in s.areas {
            let r = Rect::from_two_pos(xf.to_screen(a.min[0], a.min[2]), xf.to_screen(a.max[0], a.max[2]));
            if r.width() < 3.0 {
                p.circle_filled(r.center(), 2.0, green);
            } else {
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
                if r.width() >= 16.0 {
                    let n: u32 = a.mobs.iter().map(|e| e.count).sum();
                    p.text(r.center(), Align2::CENTER_CENTER, n.to_string(), font.clone(), green);
                }
            }
        }
        if !s.note.is_empty() {
            plate_text(p, rect.left_top() + Vec2::new(8.0, 8.0), Align2::LEFT_TOP, s.note, font, Color32::WHITE);
        }
    }
}

/// The Paint-blocked tool: the left button applies the toolbar value, the right button the opposite one; middle drag still pans.
fn paint_interact(ui: &egui::Ui, c: &mut Ctx, st: &mut VpState, xf: &Xf, resp: &egui::Response) {
    use egui::PointerButton::{Primary, Secondary};
    let press_origin = ui.input(|i| i.pointer.press_origin());
    let latest = ui.input(|i| i.pointer.latest_pos());
    let size = c.doc.map.size;
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    for (button, value) in [(Primary, c.paint.block), (Secondary, !c.paint.block)] {
        if resp.drag_started_by(button) && matches!(st.drag, Drag::None) {
            if let Some(p0) = press_origin {
                let w0 = xf.to_world(p0);
                match c.paint.mode {
                    PaintMode::Brush => {
                        let before = Box::new(c.doc.map.blocked.clone());
                        c.doc.map.blocked.fill_circle(w0[0], w0[1], c.paint.radius, value);
                        dirty_dab(c.doc, w0[0], w0[1], c.paint.radius);
                        st.drag = Drag::Paint { value, last: w0, before };
                    }
                    PaintMode::Rect | PaintMode::Invert => st.drag = Drag::PaintRect { start: w0, value, invert: c.paint.mode == PaintMode::Invert },
                    PaintMode::Fill => {}
                }
            }
        }
        if resp.dragged_by(button) {
            if let (Some(p), Drag::Paint { value, last, .. }) = (latest, &mut st.drag) {
                let w = xf.to_world(p);
                let (from, v) = (*last, *value);
                *last = w;
                stroke(c.doc, from, w, c.paint.radius, v);
            }
        }
        if resp.drag_stopped_by(button) {
            match std::mem::replace(&mut st.drag, Drag::None) {
                Drag::Paint { value, before, .. } => {
                    // put the grid back to where the stroke started and commit the difference, so undo is exact
                    let after = std::mem::replace(&mut c.doc.map.blocked, *before);
                    c.doc.commit_blocked(paint_label(value), &after);
                    c.doc.mark_blocked_dirty((0, 0, size, size));
                }
                Drag::PaintRect { start, value, invert } => {
                    if let Some(p) = latest {
                        let (x0, z0, x1, z1) = cell_rect(start, xf.to_world(p));
                        let mut g = c.doc.map.blocked.clone();
                        if invert {
                            g.invert_rect(x0, z0, x1, z1);
                        } else {
                            g.fill_rect(x0, z0, x1, z1, value);
                        }
                        c.doc.commit_blocked(if invert { "invert rectangle" } else if value { "block rectangle" } else { "free rectangle" }, &g);
                    }
                }
                other => st.drag = other,
            }
        }
        if resp.clicked_by(button) {
            if let Some(p) = resp.interact_pointer_pos() {
                let w = xf.to_world(p);
                let mut g = c.doc.map.blocked.clone();
                let (cx, cz) = (w[0].floor() as i64, w[1].floor() as i64);
                let label = match c.paint.mode {
                    PaintMode::Brush => {
                        g.fill_circle(w[0], w[1], c.paint.radius, value);
                        paint_label(value)
                    }
                    PaintMode::Rect => {
                        g.fill_rect(cx, cz, cx + 1, cz + 1, value);
                        paint_label(value)
                    }
                    PaintMode::Invert => {
                        g.invert_rect(cx, cz, cx + 1, cz + 1);
                        "invert cell"
                    }
                    PaintMode::Fill => {
                        if cx >= 0 && cz >= 0 {
                            g.flood_fill(cx as u32, cz as u32, value);
                        }
                        "flood fill"
                    }
                };
                c.doc.commit_blocked(label, &g);
            }
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel_drag(c, st);
    }
}

// ---------------------------------------------------------------------------------------------------------------- drawing

/// Text on the map with a translucent backplate, so a label reads over any terrain in both themes.
fn plate_text(p: &egui::Painter, pos: Pos2, anchor: Align2, text: impl ToString, font: FontId, color: Color32) {
    let galley = p.layout_no_wrap(text.to_string(), font, color);
    let rect = anchor.anchor_size(pos, galley.size());
    p.rect_filled(rect.expand(2.0), 2.0, crate::theme::pal(p.ctx()).overlay_bg);
    p.galley(rect.min, galley, color);
}

fn draw_layers(p: &egui::Painter, rect: Rect, xf: &Xf, c: &mut Ctx, st: &VpState, hover_pos: Option<Pos2>, out: &mut Out) {
    let size = c.doc.map.size as f32;
    let l = c.layers;
    let world_rect = Rect::from_min_max(xf.to_screen(0.0, size), xf.to_screen(size, 0.0));
    let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));

    // map backdrop
    p.rect_filled(world_rect, 0.0, Color32::from_rgb(34, 38, 44));
    if l.terrain {
        if let Some(t) = &c.tex.terrain {
            // one pixel per corner: the picture spans corner 0 .. corner n-1, i.e. half a pixel in from each edge
            let n = t.size()[0] as f32;
            let uv = Rect::from_min_max(Pos2::new(0.5 / n, 0.5 / n), Pos2::new(1.0 - 0.5 / n, 1.0 - 0.5 / n));
            p.image(t.id(), world_rect, uv, Color32::WHITE);
        }
    }
    if l.dungeon {
        if let Some(t) = &c.tex.dungeon {
            p.image(t.id(), world_rect, full_uv, Color32::WHITE);
        }
    }
    if l.minimap {
        if let Some(t) = &c.tex.minimap {
            let a = (c.minimap_alpha.clamp(0.0, 1.0) * 255.0) as u8;
            p.image(t.id(), world_rect, full_uv, Color32::from_white_alpha(a));
        }
    }
    if let (Some(h), Some(t)) = (&c.heat, &c.tex.heat) {
        // one pixel per tile, nearest filtering: the tiles stay square at every zoom; only the part over the map is drawn
        let a = (h.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        let [u0, v0, u1, v1] = crate::heat::texture_uv(size, h.tile as f32);
        p.image(t.id(), world_rect, Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)), Color32::from_white_alpha(a));
    }
    if l.blocked {
        if let Some(t) = &c.tex.blocked {
            p.image(t.id(), world_rect, full_uv, Color32::WHITE);
        }
    }
    if l.compare {
        if let Some(t) = &c.tex.compare {
            p.image(t.id(), world_rect, full_uv, Color32::WHITE);
        }
    }
    if let Some(t) = &c.tex.preview {
        p.image(t.id(), world_rect, full_uv, Color32::WHITE);
    }
    p.rect_stroke(world_rect, 0.0, Stroke::new(1.0, Color32::from_gray(120)), StrokeKind::Outside);

    if l.grid {
        draw_grid(p, rect, xf, size);
    }

    let flags = finding_flags(c.doc);
    let flag_colour = |s: SectionId, i: usize| match flags.get(&(s, i)) {
        Some(Severity::Error) => Some(Color32::from_rgb(255, 70, 70)),
        Some(Severity::Warning) => Some(Color32::from_rgb(255, 170, 40)),
        _ => None,
    };
    let sel_stroke = Stroke::new(2.0, Color32::from_rgb(255, 235, 60));
    let view_world = {
        let a = xf.to_world(rect.left_bottom());
        let b = xf.to_world(rect.right_top());
        (a[0], a[1], b[0], b[1])
    };
    let in_view = |x0: f32, z0: f32, x1: f32, z1: f32| x1 >= view_world.0 && x0 <= view_world.2 && z1 >= view_world.1 && z0 <= view_world.3;
    let zoom = xf.zoom;
    let font = FontId::proportional(11.0);
    let map = &c.doc.map;

    // objects (footprint dots); a tab whose placements were edited draws its own lists
    if l.objects && c.doc.part().is_some_and(|pt| pt.place_rev() > 0) {
        if let Some(pt) = c.doc.part() {
            let m = &pt.wld.model;
            for (list, k) in [(&m.building, ObjKind::Building), (&m.shape, ObjKind::Shape), (&m.tree, ObjKind::Tree)] {
                for it in &list.items {
                    if it.index < 0 || it.pos[0] < view_world.0 || it.pos[0] > view_world.2 || it.pos[2] < view_world.1 || it.pos[2] > view_world.3 {
                        continue;
                    }
                    let colour = match k {
                        ObjKind::Building => Color32::from_rgb(230, 190, 120),
                        ObjKind::Shape => Color32::from_rgb(170, 170, 200),
                        ObjKind::Tree => Color32::from_rgb(90, 200, 100),
                    };
                    p.circle_filled(xf.to_screen(it.pos[0], it.pos[2]), if k == ObjKind::Building { 2.5 } else { 1.5 }, colour);
                }
            }
        }
    } else if l.objects {
        if let Some(a) = c.look.assets {
            for (x, z, k) in &a.objects {
                if *x < view_world.0 || *x > view_world.2 || *z < view_world.1 || *z > view_world.3 {
                    continue;
                }
                let colour = match k {
                    ObjKind::Building => Color32::from_rgb(230, 190, 120),
                    ObjKind::Shape => Color32::from_rgb(170, 170, 200),
                    ObjKind::Tree => Color32::from_rgb(90, 200, 100),
                };
                p.circle_filled(xf.to_screen(*x, *z), if *k == ObjKind::Building { 2.5 } else { 1.5 }, colour);
            }
        }
    }

    // added or moved placements are dots at once, with the Objects layer off too (with it on they are in the list above)
    if !l.objects {
        if let Some(pt) = c.doc.part().filter(|pt| pt.place_rev() > 0) {
            for (x, z, k) in c.place.changed_dots(pt) {
                if *x < view_world.0 || *x > view_world.2 || *z < view_world.1 || *z > view_world.3 {
                    continue;
                }
                let colour = match k {
                    0 => Color32::from_rgb(230, 190, 120),
                    1 => Color32::from_rgb(170, 170, 200),
                    _ => Color32::from_rgb(90, 200, 100),
                };
                p.circle_filled(xf.to_screen(*x, *z), if *k == 0 { 3.0 } else { 2.0 }, colour);
            }
        }
    }

    if c.tool == Tool::Place {
        let src = c.look.rules.map(|r| &r.source);
        draw_places(p, rect, xf, &*c.doc, &mut *c.place, src, c.snap, crate::place_ui::field_ground(c.look.assets), hover_pos);
    }

    if c.tool == Tool::Terrain {
        draw_terrain(p, rect, xf, &*c.doc, &mut *c.terrain);
    }

    // ghosts: the client's own portal boxes, NPC markers and areas, hollow and grey
    if l.ghosts {
        if let Some(a) = c.look.assets {
            draw_ghosts(p, xf, a, c.doc.part(), &in_view, zoom);
        }
    }

    // regions: dashed boxes with the client's names
    if l.regions {
        for (i, g) in map.regions.iter().enumerate() {
            let (x0, x1) = (g.min[0].min(g.max[0]), g.min[0].max(g.max[0]));
            let (z0, z1) = (g.min[2].min(g.max[2]), g.min[2].max(g.max[2]));
            if !in_view(x0, z0, x1, z1) {
                continue;
            }
            let r = xf.rect(g.min, g.max);
            let selected = c.doc.is_selected(SectionId::Regions, i);
            let stroke = if selected { sel_stroke } else { Stroke::new(1.0, flag_colour(SectionId::Regions, i).unwrap_or(Color32::from_rgb(120, 200, 190))) };
            if r.width().min(r.height()) >= 24.0 {
                dashed_rect(p, r, stroke);
            } else {
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
            }
            // the name: the tab's own client half (with its renames) first, else the client's file
            let own = c.doc.part().and_then(|cp| cp.names().get(i)).filter(|n| !n.is_empty());
            if l.names && r.width() >= 50.0 && r.height() >= 14.0 && (own.is_some() || c.look.region_name(i).is_some()) {
                let name = own.map_or_else(|| c.look.region_label(i, g), str::to_string);
                p.with_clip_rect(r).text(r.left_top() + Vec2::new(3.0, 2.0), Align2::LEFT_TOP, name, font.clone(), Color32::from_rgb(150, 230, 220));
            }
        }
    }

    // bind areas
    if l.binds {
        for (i, b) in map.binds.iter().enumerate() {
            let base = match b.country {
                1 => [90, 160, 255],
                2 => [255, 100, 100],
                _ => [200, 120, 255],
            };
            let selected = c.doc.is_selected(SectionId::Binds, i);
            for (k, x) in b.boxes.iter().enumerate() {
                let (x0, x1) = (x.min[0].min(x.max[0]), x.min[0].max(x.max[0]));
                let (z0, z1) = (x.min[2].min(x.max[2]), x.min[2].max(x.max[2]));
                if !in_view(x0, z0, x1, z1) {
                    continue;
                }
                let r = xf.rect(x.min, x.max);
                p.rect_filled(r, 0.0, col(base, 40));
                let stroke = if selected { sel_stroke } else { Stroke::new(1.5, flag_colour(SectionId::Binds, i).unwrap_or(col(base, 230))) };
                p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
                if selected && c.doc.sub == Some(k) {
                    p.rect_stroke(r.shrink(3.0), 0.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
                }
                if l.names && r.width() >= 40.0 {
                    plate_text(p, r.center(), Align2::CENTER_CENTER, format!("bind {}", names::country_name(b.country)), font.clone(), col(base, 255));
                }
            }
        }
    }

    // monster areas
    if l.mob_areas {
        for (i, a) in map.mob_areas.iter().enumerate() {
            let (x0, x1) = (a.min[0].min(a.max[0]), a.min[0].max(a.max[0]));
            let (z0, z1) = (a.min[2].min(a.max[2]), a.min[2].max(a.max[2]));
            if !in_view(x0, z0, x1, z1) {
                continue;
            }
            let r = xf.rect(a.min, a.max);
            let colour = names::level_colour(c.look.mean_level(a));
            let selected = c.doc.is_selected(SectionId::MobAreas, i);
            // outside the level filter: 15 % of the usual opacity
            let dim = level_hidden(c, a);
            let k = |alpha: u8| if dim { (alpha as f32 * 0.15) as u8 } else { alpha };
            p.rect_filled(r, 0.0, col(colour, k(if selected { 120 } else { 80 })));
            let stroke = if selected { sel_stroke } else if dim { Stroke::new(1.0, col(colour, k(220))) } else { Stroke::new(1.0, flag_colour(SectionId::MobAreas, i).unwrap_or(col(colour, 220))) };
            p.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
            if !dim && zoom >= 1.0 && r.width() >= 18.0 && r.height() >= 12.0 {
                let total: u32 = a.mobs.iter().map(|e| e.count).sum();
                p.with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, total, font.clone(), Color32::WHITE);
            }
        }
    }

    // section B crosses and section A diamonds
    if l.points_b {
        for (i, pt) in map.points_b.iter().enumerate() {
            if !in_view(pt[0], pt[2], pt[0], pt[2]) {
                continue;
            }
            let s = xf.to_screen(pt[0], pt[2]);
            let colour = if c.doc.is_selected(SectionId::SectionB, i) { Color32::from_rgb(255, 235, 60) } else { Color32::from_rgb(255, 160, 60) };
            let stroke = Stroke::new(2.0, colour);
            p.line_segment([s + Vec2::new(-5.0, -5.0), s + Vec2::new(5.0, 5.0)], stroke);
            p.line_segment([s + Vec2::new(-5.0, 5.0), s + Vec2::new(5.0, -5.0)], stroke);
        }
    }
    if l.section_a {
        for (i, a) in map.section_a.iter().enumerate() {
            let s = xf.to_screen(a.pos[0], a.pos[2]);
            let colour = if c.doc.is_selected(SectionId::SectionA, i) { Color32::from_rgb(255, 235, 60) } else { Color32::from_rgb(200, 200, 200) };
            p.add(Shape::convex_polygon(vec![s + Vec2::new(0.0, -5.0), s + Vec2::new(5.0, 0.0), s + Vec2::new(0.0, 5.0), s + Vec2::new(-5.0, 0.0)], colour, Stroke::NONE));
        }
    }

    // portals: use-range ring, pin, destination arrow
    if l.portals {
        for (i, pt) in map.portals.iter().enumerate() {
            let s = xf.to_screen(pt.pos[0], pt.pos[2]);
            if !in_view(pt.pos[0] - PORTAL_USE_RANGE, pt.pos[2] - PORTAL_USE_RANGE, pt.pos[0] + PORTAL_USE_RANGE, pt.pos[2] + PORTAL_USE_RANGE) {
                continue;
            }
            let selected = c.doc.is_selected(SectionId::Portals, i);
            let colour = match pt.country {
                1 => Color32::from_rgb(110, 170, 255),
                2 => Color32::from_rgb(255, 120, 120),
                _ => Color32::from_rgb(120, 230, 255),
            };
            let ring = PORTAL_USE_RANGE * zoom;
            p.circle_stroke(s, ring, Stroke::new(1.0, Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), if selected { 200 } else { 70 })));
            let stroke = if selected { sel_stroke } else { Stroke::new(2.0, flag_colour(SectionId::Portals, i).unwrap_or(colour)) };
            p.add(Shape::convex_polygon(vec![s + Vec2::new(0.0, -7.0), s + Vec2::new(6.0, 0.0), s + Vec2::new(0.0, 7.0), s + Vec2::new(-6.0, 0.0)], Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 200), stroke));
            let same_map = c.look.map_id == Some(pt.to_map);
            if same_map {
                arrow(p, s, xf.to_screen(pt.to_pos[0], pt.to_pos[2]), Stroke::new(1.5, Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 180)));
            }
            if l.names && zoom >= 0.25 {
                let text = if same_map { format!("#{i}") } else { format!("#{i} -> {}", (c.map_name)(pt.to_map)) };
                plate_text(p, s + Vec2::new(9.0, -9.0), Align2::LEFT_BOTTOM, text, font.clone(), colour);
            }
        }
    }

    // NPCs
    if l.npcs {
        for (i, n) in map.npcs.iter().enumerate() {
            let Some(first) = n.points.first() else { continue };
            let mut visible_pt = false;
            let (mut bx0, mut bz0, mut bx1, mut bz1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for pt in &n.points {
                bx0 = bx0.min(pt.pos[0]);
                bz0 = bz0.min(pt.pos[2]);
                bx1 = bx1.max(pt.pos[0]);
                bz1 = bz1.max(pt.pos[2]);
            }
            if in_view(bx0 - 2.0, bz0 - 2.0, bx1 + 2.0, bz1 + 2.0) {
                visible_pt = true;
            }
            if !visible_pt {
                continue;
            }
            let selected = c.doc.is_selected(SectionId::Npcs, i);
            let colour = npc_colour(n.kind);
            if n.points.len() > 1 {
                let pts: Vec<Pos2> = n.points.iter().map(|q| xf.to_screen(q.pos[0], q.pos[2])).collect();
                p.line(pts.clone(), Stroke::new(1.0, Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 150)));
                for (k, q) in pts.iter().enumerate().skip(1) {
                    let sub_sel = selected && c.doc.sub == Some(k);
                    p.circle_filled(*q, if sub_sel { 4.0 } else { 2.5 }, if sub_sel { Color32::from_rgb(255, 235, 60) } else { colour });
                }
            }
            let s = xf.to_screen(first.pos[0], first.pos[2]);
            let tip = s + Vec2::new(first.dir.sin(), -first.dir.cos()) * 13.0;
            p.line_segment([s, tip], Stroke::new(1.5, colour));
            let outline = if selected { Color32::from_rgb(255, 235, 60) } else { flag_colour(SectionId::Npcs, i).unwrap_or(Color32::BLACK) };
            p.circle_filled(s, 4.5, colour);
            p.circle_stroke(s, 4.5, Stroke::new(if selected { 2.0 } else { 1.0 }, outline));
            if selected && c.doc.single().is_some() {
                p.circle_stroke(s, ROT_RING, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 235, 60, 120)));
                let h = s + Vec2::new(first.dir.sin(), -first.dir.cos()) * ROT_RING;
                p.circle_filled(h, 4.0, Color32::from_rgb(255, 235, 60));
            }
            if l.names && zoom >= 0.8 {
                plate_text(p, s + Vec2::new(7.0, -2.0), Align2::LEFT_BOTTOM, c.look.npc_name(n), font.clone(), Color32::from_rgb(235, 235, 235));
            }
        }
    }

    // compare layer: what differs from the reference (Changes tab)
    if l.compare {
        if let Some(o) = c.compare {
            draw_compare(p, xf, o, &in_view);
        }
    }

    // handles of the single selected box
    if let Some((_, sub, rec)) = handle_target(c.doc).filter(|_| !c.clean) {
        if let Some((min, max)) = box_of(&rec, sub) {
            let r = xf.rect(min, max);
            for hp in handle_points(r) {
                let hr = Rect::from_center_size(hp, Vec2::splat(7.0));
                p.rect_filled(hr, 0.0, Color32::WHITE);
                p.rect_stroke(hr, 0.0, Stroke::new(1.0, Color32::BLACK), StrokeKind::Outside);
            }
        }
    }

    // the record an outliner, Problems or Find row is hovered over: a white outline
    if let (Some((s, i)), false) = (c.hover_rec, c.clean) {
        if let Some(r) = edit::get(map, s, i) {
            let b = edit::bounds_xz(&r);
            let sr = Rect::from_two_pos(xf.to_screen(b.0, b.1), xf.to_screen(b.2, b.3));
            let stroke = Stroke::new(2.0, Color32::WHITE);
            if sr.width() < 14.0 && sr.height() < 14.0 {
                p.circle_stroke(sr.center(), 11.0, stroke);
            } else {
                p.rect_stroke(sr.expand(3.0), 0.0, stroke, StrokeKind::Outside);
            }
        }
    }

    // the Box tool with a template: the box a click would stamp
    if let (Tool::Box, DrawTarget::MobArea, Some(t), Some(hp), Drag::None) = (c.tool, c.draw_target, c.template, hover_pos, &st.drag) {
        let w = xf.to_world(hp);
        let (cx, cz) = (snap_to(w[0], c.snap), snap_to(w[1], c.snap));
        let r = Rect::from_two_pos(xf.to_screen(cx - t.size[0] * 0.5, cz - t.size[1] * 0.5), xf.to_screen(cx + t.size[0] * 0.5, cz + t.size[1] * 0.5));
        dashed_rect(p, r, Stroke::new(1.5, Color32::from_rgb(80, 220, 130)));
        plate_text(p, r.right_bottom() + Vec2::new(4.0, 4.0), Align2::LEFT_TOP, format!("{} ({:.0} x {:.0})", t.name, t.size[0], t.size[1]), font.clone(), Color32::WHITE);
    }

    if !c.clean {
        draw_scatter(p, rect, xf, c, st, hover_pos);
    }

    // level legend: the colour of a monster area by mean level (only when the client gives levels)
    if let (Some(h), false) = (&c.heat, c.clean) {
        draw_heat_legend(p, rect, h, c.level_filter);
    } else if l.mob_areas && !c.clean && c.look.rules.is_some_and(|r| r.monsters.is_some()) {
        draw_legend(p, rect, c.level_filter);
    }

    // Paint-blocked: the brush outline (or the cell under the cursor) and the rectangle being dragged
    if c.tool == Tool::Paint {
        if let Some(hp) = hover_pos {
            let w = xf.to_world(hp);
            let value = match &st.drag {
                Drag::Paint { value, .. } => *value,
                _ => c.paint.block,
            };
            let colour = if value { Color32::from_rgb(255, 90, 90) } else { Color32::from_rgb(90, 230, 120) };
            let brush = c.paint.mode == PaintMode::Brush;
            if brush && c.paint.radius >= 0.5 {
                p.circle_stroke(xf.to_screen(w[0], w[1]), c.paint.radius * zoom, Stroke::new(1.5, colour));
            } else {
                let (cx, cz) = (w[0].floor(), w[1].floor());
                p.rect_stroke(Rect::from_two_pos(xf.to_screen(cx, cz), xf.to_screen(cx + 1.0, cz + 1.0)), 0.0, Stroke::new(1.5, colour), StrokeKind::Outside);
            }
        }
    }
    if let Drag::PaintRect { start, value, invert } = &st.drag {
        if let Some(cur) = hover_pos.or_else(|| p.ctx().input(|i| i.pointer.latest_pos())) {
            let w = xf.to_world(cur);
            let (x0, z0, x1, z1) = cell_rect(*start, w);
            let r = Rect::from_two_pos(xf.to_screen(x0 as f32, z0 as f32), xf.to_screen(x1 as f32, z1 as f32));
            let colour = if *invert { Color32::from_rgb(255, 220, 80) } else if *value { Color32::from_rgb(255, 90, 90) } else { Color32::from_rgb(90, 230, 120) };
            p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 50));
            p.rect_stroke(r, 0.0, Stroke::new(1.5, colour), StrokeKind::Inside);
            plate_text(p, r.right_bottom() + Vec2::new(4.0, 4.0), Align2::LEFT_TOP, format!("{} x {} cells", x1 - x0, z1 - z0), font.clone(), Color32::WHITE);
        }
    }

    // rubber bands
    match &st.drag {
        Drag::Band { start, .. } | Drag::Draw { start } => {
            if let Some(cur) = hover_pos.or_else(|| p.ctx().input(|i| i.pointer.latest_pos())) {
                let w = xf.to_world(cur);
                let w = if matches!(st.drag, Drag::Draw { .. }) { [snap_to(w[0], c.snap), snap_to(w[1], c.snap)] } else { w };
                let a = xf.to_screen(start[0], start[1]);
                let b = xf.to_screen(w[0], w[1]);
                let r = Rect::from_two_pos(a, b);
                let drawing = matches!(st.drag, Drag::Draw { .. });
                p.rect_filled(r, 0.0, if drawing { Color32::from_rgba_unmultiplied(80, 200, 120, 50) } else { Color32::from_rgba_unmultiplied(120, 170, 255, 40) });
                p.rect_stroke(r, 0.0, Stroke::new(1.0, if drawing { Color32::from_rgb(80, 220, 130) } else { Color32::from_rgb(130, 180, 255) }), StrokeKind::Inside);
                if drawing {
                    plate_text(p, r.right_bottom() + Vec2::new(4.0, 4.0), Align2::LEFT_TOP, format!("{:.1} x {:.1}", (w[0] - start[0]).abs(), (w[1] - start[1]).abs()), font.clone(), Color32::WHITE);
                }
            }
        }
        _ => {}
    }
    if st.measure.active() && c.tool == Tool::Measure {
        draw_measure(p, xf, c, st, hover_pos, &font);
    }
    // hover tooltip text for the status bar is returned through `out.hover_hit`; nothing more to paint here
    let _ = out;
}

/// The compare layer: a removed record is a red dashed box (where the reference has it), an added one a green outline, a changed one
/// an amber outline with the reference's box hollow behind it and a thin line from the old place to the new one.
fn draw_compare(p: &egui::Painter, xf: &Xf, o: &compare::Overlay, in_view: &dyn Fn(f32, f32, f32, f32) -> bool) {
    let red = Color32::from_rgb(255, 90, 90);
    let green = Color32::from_rgb(90, 230, 120);
    let amber = Color32::from_rgb(255, 190, 60);
    let hollow = Color32::from_rgba_unmultiplied(220, 220, 230, 140);
    let shape = |b: (f32, f32, f32, f32), colour: Color32, width: f32, dashed: bool| {
        let r = Rect::from_two_pos(xf.to_screen(b.0, b.1), xf.to_screen(b.2, b.3));
        if r.width() < 10.0 && r.height() < 10.0 {
            p.circle_stroke(r.center(), 7.0, Stroke::new(width, colour));
        } else if dashed {
            dashed_rect(p, r, Stroke::new(width, colour));
        } else {
            p.rect_stroke(r, 0.0, Stroke::new(width, colour), StrokeKind::Outside);
        }
    };
    for it in &o.items {
        let seen = [it.ref_bounds, it.cur_bounds].into_iter().flatten().any(|b| in_view(b.0 - 2.0, b.1 - 2.0, b.2 + 2.0, b.3 + 2.0));
        if !seen {
            continue;
        }
        match it.kind {
            shaiya_svmap::diff::DiffKind::Removed => {
                if let Some(b) = it.ref_bounds {
                    shape(b, red, 2.0, true);
                }
            }
            shaiya_svmap::diff::DiffKind::Added => {
                if let Some(b) = it.cur_bounds {
                    shape(b, green, 2.0, false);
                }
            }
            shaiya_svmap::diff::DiffKind::Changed => {
                if let Some(b) = it.ref_bounds {
                    shape(b, hollow, 1.0, false);
                }
                if let Some(b) = it.cur_bounds {
                    shape(b, amber, 2.0, false);
                }
                if let Some((a, b)) = compare::move_line(it) {
                    p.line_segment([xf.to_screen(a[0], a[1]), xf.to_screen(b[0], b[1])], Stroke::new(1.0, amber));
                }
            }
        }
    }
}

/// The level legend at the bottom-left of the viewport: the blue to red bar of `names::level_colour` with ticks at 1, 20, 40, 60, 80,
/// and the level filter's range outlined on it.
fn draw_legend(p: &egui::Painter, rect: Rect, filter: Option<(f32, f32)>) {
    let (w, h) = (160.0, 10.0);
    let bg = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.bottom() - 40.0), Vec2::new(w + 16.0, 34.0));
    p.rect_filled(bg, 4.0, Color32::from_rgba_unmultiplied(20, 22, 26, 200));
    let bar = Rect::from_min_size(bg.left_top() + Vec2::new(8.0, 6.0), Vec2::new(w, h));
    let steps = 80;
    for k in 0..steps {
        let lv = 1.0 + 79.0 * (k as f32 + 0.5) / steps as f32;
        let x0 = bar.left() + w * k as f32 / steps as f32;
        let x1 = bar.left() + w * (k + 1) as f32 / steps as f32;
        p.rect_filled(Rect::from_min_max(Pos2::new(x0, bar.top()), Pos2::new(x1 + 0.5, bar.bottom())), 0.0, col(names::level_colour(Some(lv)), 255));
    }
    let x_of = |lv: f32| bar.left() + w * ((lv - 1.0) / 79.0).clamp(0.0, 1.0);
    for lv in [1.0, 20.0, 40.0, 60.0, 80.0] {
        let x = x_of(lv);
        p.line_segment([Pos2::new(x, bar.bottom()), Pos2::new(x, bar.bottom() + 3.0)], Stroke::new(1.0, Color32::from_gray(200)));
        p.text(Pos2::new(x, bar.bottom() + 3.0), Align2::CENTER_TOP, format!("{lv:.0}"), FontId::proportional(9.0), Color32::from_gray(210));
    }
    if let Some((lo, hi)) = filter {
        let r = Rect::from_min_max(Pos2::new(x_of(lo), bar.top() - 2.0), Pos2::new(x_of(hi), bar.bottom() + 2.0));
        p.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
    }
}

/// The heatmap legend at the bottom-left of the viewport (it takes the place of the level legend): the colour ramp with 0 and the
/// scale maximum, or the level bar.
fn draw_heat_legend(p: &egui::Painter, rect: Rect, h: &HeatDraw, filter: Option<(f32, f32)>) {
    use crate::heat::{density_rgba, Mode};
    if h.mode == Mode::Level {
        draw_legend(p, rect, filter);
        return;
    }
    let (w, bh) = (160.0, 10.0);
    let bg = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.bottom() - 52.0), Vec2::new(w + 16.0, 46.0));
    p.rect_filled(bg, 4.0, Color32::from_rgba_unmultiplied(20, 22, 26, 200));
    let bar = Rect::from_min_size(bg.left_top() + Vec2::new(8.0, 6.0), Vec2::new(w, bh));
    p.rect_filled(bar, 0.0, Color32::from_gray(30));
    let steps = 64;
    for k in 0..steps {
        let t = (k as f32 + 0.5) / steps as f32;
        let c = density_rgba(t.max(0.2));
        let x0 = bar.left() + w * k as f32 / steps as f32;
        let x1 = bar.left() + w * (k + 1) as f32 / steps as f32;
        p.rect_filled(Rect::from_min_max(Pos2::new(x0, bar.top()), Pos2::new(x1 + 0.5, bar.bottom())), 0.0, Color32::from_rgb(c[0], c[1], c[2]));
    }
    let tick = |x: f32, align: Align2, text: String| {
        p.line_segment([Pos2::new(x, bar.bottom()), Pos2::new(x, bar.bottom() + 3.0)], Stroke::new(1.0, Color32::from_gray(200)));
        p.text(Pos2::new(x, bar.bottom() + 3.0), align, text, FontId::proportional(9.0), Color32::from_gray(210));
    };
    tick(bar.left(), Align2::LEFT_TOP, "0".to_string());
    tick(bar.right(), Align2::RIGHT_TOP, if h.max >= 10.0 { format!("{:.0}", h.max) } else { format!("{:.1}", h.max) });
    p.text(bg.left_bottom() + Vec2::new(8.0, -5.0), Align2::LEFT_BOTTOM, format!("monsters per {0} x {0}", h.tile), FontId::proportional(9.0), Color32::from_gray(210));
}

/// The ruler step in world units: 1, 2 or 5 times a power of ten, the smallest with at least 60 pixels between two ticks.
pub fn ruler_step(zoom: f32) -> f32 {
    let zoom = zoom.max(1e-6);
    let mut e = -3i32;
    loop {
        for m in [1.0f32, 2.0, 5.0] {
            let step = m * 10f32.powi(e);
            if step * zoom >= 60.0 {
                return step;
            }
        }
        e += 1;
        if e > 8 {
            return 1e8;
        }
    }
}

/// Edge rulers: 18 pixel bands along the top (x) and the left (z) of the viewport, ticks at 1-2-5 steps, the cursor marked.
fn draw_rulers(p: &egui::Painter, rect: Rect, xf: &Xf, cursor: Option<Pos2>) {
    const BAND: f32 = 18.0;
    let bg = Color32::from_rgba_unmultiplied(20, 22, 26, 215);
    let ink = Color32::from_gray(205);
    let mark = Color32::from_rgb(255, 230, 60);
    let step = ruler_step(xf.zoom);
    let decimals = if step >= 1.0 { 0 } else { (-step.log10()).ceil() as usize };
    let font = FontId::proportional(9.0);
    let top = Rect::from_min_size(rect.left_top(), Vec2::new(rect.width(), BAND));
    let left = Rect::from_min_size(rect.left_top(), Vec2::new(BAND, rect.height()));
    p.rect_filled(top, 0.0, bg);
    p.rect_filled(left, 0.0, bg);
    let w0 = xf.to_world(rect.left_bottom());
    let w1 = xf.to_world(rect.right_top());
    let mut x = (w0[0] / step).ceil() * step;
    while x <= w1[0] {
        let sx = xf.to_screen(x, 0.0).x;
        p.line_segment([Pos2::new(sx, top.bottom() - 6.0), Pos2::new(sx, top.bottom())], Stroke::new(1.0, ink));
        p.text(Pos2::new(sx + 2.0, top.top() + 1.0), Align2::LEFT_TOP, format!("{x:.decimals$}"), font.clone(), ink);
        x += step;
    }
    let mut z = (w0[1] / step).ceil() * step;
    while z <= w1[1] {
        let sy = xf.to_screen(0.0, z).y;
        p.line_segment([Pos2::new(left.right() - 6.0, sy), Pos2::new(left.right(), sy)], Stroke::new(1.0, ink));
        p.text(Pos2::new(left.left() + 1.0, sy - 1.0), Align2::LEFT_BOTTOM, format!("{z:.decimals$}"), font.clone(), ink);
        z += step;
    }
    if let Some(c) = cursor {
        p.line_segment([Pos2::new(c.x, top.top()), Pos2::new(c.x, top.bottom())], Stroke::new(1.5, mark));
        p.line_segment([Pos2::new(left.left(), c.y), Pos2::new(left.right(), c.y)], Stroke::new(1.5, mark));
    }
}

pub const OVERVIEW_SIZE: f32 = 180.0;

/// The overview inset: a square at the bottom-right of the viewport.
pub fn overview_rect(rect: Rect) -> Rect {
    Rect::from_min_size(Pos2::new(rect.right() - OVERVIEW_SIZE - 10.0, rect.bottom() - OVERVIEW_SIZE - 10.0), Vec2::splat(OVERVIEW_SIZE))
}

/// Whether the whole map is inside the viewport (then the overview has nothing to add and is hidden).
pub fn whole_map_in_view(rect: Rect, xf: &Xf, size: f32) -> bool {
    let a = xf.to_world(rect.left_bottom());
    let b = xf.to_world(rect.right_top());
    a[0] <= 0.0 && a[1] <= 0.0 && b[0] >= size && b[1] >= size
}

/// The world position under a point of the overview inset.
fn overview_world(ov: Rect, p: Pos2, size: f32) -> [f32; 2] {
    [((p.x - ov.left()) / ov.width()).clamp(0.0, 1.0) * size, ((ov.bottom() - p.y) / ov.height()).clamp(0.0, 1.0) * size]
}

/// Click or drag inside the overview moves the view centre. Returns true when the pointer belongs to the inset this frame (the
/// viewport must not also treat it as a click or the start of a drag).
fn overview_interact(ui: &egui::Ui, c: &mut Ctx, st: &mut VpState, ov: Rect, size: f32) -> bool {
    let (pressed, down, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.latest_pos()));
    let mut consumed = false;
    if st.ov_drag {
        consumed = true;
        if !down {
            st.ov_drag = false;
        }
    } else if pressed && st.menu.is_none() && pos.is_some_and(|p| ov.contains(p)) {
        st.ov_drag = true;
        consumed = true;
    }
    if st.ov_drag {
        if let Some(p) = pos {
            let w = overview_world(ov, p, size);
            c.doc.view.cx = w[0];
            c.doc.view.cz = w[1];
        }
    }
    if consumed || pos.is_some_and(|p| ov.contains(p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    consumed
}

fn draw_overview(p: &egui::Painter, ov: Rect, xf: &Xf, c: &Ctx, rect: Rect, size: f32) {
    p.rect_filled(ov, 2.0, Color32::from_rgb(34, 38, 44));
    let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    // the first picture the map has: terrain, minimap, dungeon floor (else the plain grid colour above)
    if let Some(t) = &c.tex.terrain {
        let n = t.size()[0] as f32;
        let uv = Rect::from_min_max(Pos2::new(0.5 / n, 0.5 / n), Pos2::new(1.0 - 0.5 / n, 1.0 - 0.5 / n));
        p.image(t.id(), ov, uv, Color32::WHITE);
    } else if let Some(t) = &c.tex.minimap {
        p.image(t.id(), ov, full_uv, Color32::WHITE);
    } else if let Some(t) = &c.tex.dungeon {
        p.image(t.id(), ov, full_uv, Color32::WHITE);
    }
    p.rect_stroke(ov, 2.0, Stroke::new(1.0, Color32::from_gray(150)), StrokeKind::Outside);
    let to_ov = |x: f32, z: f32| Pos2::new(ov.left() + (x / size).clamp(0.0, 1.0) * ov.width(), ov.bottom() - (z / size).clamp(0.0, 1.0) * ov.height());
    // the selection as dots
    for (s, i) in &c.doc.selection {
        if let Some(r) = edit::get(&c.doc.map, *s, *i) {
            let a = anchor_of(&r);
            p.circle_filled(to_ov(a[0], a[1]), 2.5, Color32::from_rgb(255, 235, 60));
        }
    }
    // the view rectangle
    let a = xf.to_world(rect.left_bottom());
    let b = xf.to_world(rect.right_top());
    let vr = Rect::from_two_pos(to_ov(a[0], a[1]), to_ov(b[0], b[1]));
    p.rect_filled(vr, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 25));
    p.rect_stroke(vr, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
}

/// The right-click menu of the viewport: its entries and the request an entry makes. `None` while nothing is chosen. `close` is set
/// when the menu should go (an entry was chosen, Esc, or a click outside it).
pub fn context_menu(ctx: &egui::Context, c: &MenuIn, m: &CtxMenu, close: &mut bool) -> Option<CtxAct> {
    let ro = c.read_only;
    let mut act = None;
    let area = egui::Area::new(egui::Id::unique("viewport-context-menu")).order(egui::Order::Foreground).fixed_pos(m.pos).constrain(true).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(190.0);
            let mut item = |ui: &mut egui::Ui, enabled: bool, text: &str, tip: &str, a: CtxAct| {
                let r = ui.add_enabled(enabled, egui::Button::new(text).frame(false));
                let r = if !tip.is_empty() { if enabled { r.on_hover_text(tip) } else { r.on_disabled_hover_text(tip) } } else { r };
                if r.clicked() {
                    act = Some(a);
                }
            };
            let w = m.world;
            match m.hit {
                Some(h) => {
                    ui.label(egui::RichText::new(format!("{} #{}", h.sec.label(), h.idx)).weak());
                    item(ui, true, "Frame", "F", CtxAct::Frame);
                    item(ui, true, "Copy", "Ctrl+C", CtxAct::Copy);
                    item(ui, !ro, "Duplicate", "Ctrl+D", CtxAct::Duplicate);
                    item(ui, !ro, "Delete", "Del", CtxAct::Delete);
                    item(ui, !ro, "Drop to ground", "G", CtxAct::DropToGround);
                    item(ui, true, "Select similar", "Selects the records of this section with the same key (monsters, NPC type and id, destination map, country)", CtxAct::SelectSimilar);
                    for (f, titles) in c.fixes {
                        for (k, t) in titles.iter().enumerate() {
                            item(ui, !ro, &format!("Fix: {t}"), &format!("{}: {}", f.code, f.message), CtxAct::Fix(f.clone(), k));
                        }
                    }
                    ui.separator();
                    item(ui, true, "Copy position", "", CtxAct::CopyPosition(w));
                    item(ui, true, "Copy GM move command", "", CtxAct::CopyGm(w));
                    if h.sec == SectionId::Portals {
                        item(ui, true, "Go to destination", "Opens the destination map and shows where the portal leads", CtxAct::GoToDest(h.idx));
                    }
                    if h.sec == SectionId::MobAreas {
                        item(ui, true, "Save as template...", "Keep this area's monsters and size for the Box tool", CtxAct::SaveTemplate(h.idx));
                    }
                }
                None => {
                    item(ui, c.can_paste && !ro, "Paste here", "Ctrl+V", CtxAct::PasteHere(w));
                    let area_label = match c.template {
                        Some(t) => format!("Add area here ({})", t.name),
                        None => "Add area here".to_string(),
                    };
                    item(ui, !ro, &area_label, "Uses the chosen template, or a 16 x 16 box.", CtxAct::AddArea(w));
                    item(ui, !ro, "Add NPC here", "The NPC tool's type and id", CtxAct::AddNpc(w));
                    item(ui, true, "Add bookmark here", "Ctrl+B", CtxAct::AddBookmark(w));
                    ui.separator();
                    item(ui, true, "Copy position", "", CtxAct::CopyPosition(w));
                    item(ui, true, "Copy GM move command", "", CtxAct::CopyGm(w));
                    item(ui, true, "Go to...", "Ctrl+G", CtxAct::GoTo);
                }
            }
        });
    });
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
    if act.is_some() || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *close = true;
    }
    act
}

/// The grey outlines of the client's own portal boxes, areas, zone groups and NPC marks. A tab with a client half draws that half, so
/// a paired edit moves its outline at once; otherwise the lists come from the client's file.
fn draw_ghosts(p: &egui::Painter, xf: &Xf, a: &MapAssets, part: Option<&crate::pair::ClientPart>, in_view: &dyn Fn(f32, f32, f32, f32) -> bool, zoom: f32) {
    let Some(w) = &a.wld else { return };
    let (areas, zones, portals, npcs): Ghosts<'_> = match part {
        Some(cp) => {
            let m = &cp.wld.model;
            (m.areas.iter().map(|g| (g.min, g.max)).collect(), &m.zone_groups, m.portals.iter().map(|g| (g.min, g.max)).collect(), m.npcs.iter().map(|n| n.pos).collect())
        }
        None => (w.areas.iter().map(|g| (g.min, g.max)).collect(), &w.zone_groups, w.portals.iter().map(|g| (g.min, g.max)).collect(), w.npcs.iter().map(|n| n.pos).collect()),
    };
    let ghost = Stroke::new(1.0, Color32::from_rgba_unmultiplied(170, 170, 170, 120));
    for (gmin, gmax) in &areas {
        if !in_view(gmin[0], gmin[2], gmax[0], gmax[2]) {
            continue;
        }
        p.rect_stroke(xf.rect(*gmin, *gmax), 0.0, ghost, StrokeKind::Inside);
    }
    for g in zones {
        for x in &g.boxes {
            if in_view(x.min[0], x.min[2], x.max[0], x.max[2]) {
                p.rect_stroke(xf.rect(x.min, x.max), 0.0, ghost, StrokeKind::Inside);
            }
        }
    }
    for (pmin, pmax) in &portals {
        if in_view(pmin[0], pmin[2], pmax[0], pmax[2]) {
            let r = xf.rect(*pmin, *pmax);
            let r = if r.width() < 5.0 { Rect::from_center_size(r.center(), Vec2::splat(5.0)) } else { r };
            p.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::from_rgba_unmultiplied(190, 190, 190, 170)), StrokeKind::Outside);
        }
    }
    if zoom >= 0.15 {
        for pos in &npcs {
            if in_view(pos[0], pos[2], pos[0], pos[2]) {
                let s = xf.to_screen(pos[0], pos[2]);
                p.line_segment([s + Vec2::new(-3.0, -3.0), s + Vec2::new(3.0, 3.0)], ghost);
                p.line_segment([s + Vec2::new(-3.0, 3.0), s + Vec2::new(3.0, -3.0)], ghost);
            }
        }
    }
}

fn draw_grid(p: &egui::Painter, rect: Rect, xf: &Xf, size: f32) {
    let a = xf.to_world(rect.left_bottom());
    let b = xf.to_world(rect.right_top());
    let (x0, x1) = (a[0].max(0.0), b[0].min(size));
    let (z0, z1) = (a[1].max(0.0), b[1].min(size));
    if x0 >= x1 || z0 >= z1 {
        return;
    }
    let lines = |step: f32, stroke: Stroke| {
        let mut x = (x0 / step).ceil() * step;
        while x <= x1 {
            p.line_segment([xf.to_screen(x, z0), xf.to_screen(x, z1)], stroke);
            x += step;
        }
        let mut z = (z0 / step).ceil() * step;
        while z <= z1 {
            p.line_segment([xf.to_screen(x0, z), xf.to_screen(x1, z)], stroke);
            z += step;
        }
    };
    if xf.zoom >= 8.0 {
        lines(1.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 18)));
    }
    if xf.zoom >= 0.4 {
        lines(64.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 45)));
    } else {
        lines(256.0, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 40)));
    }
}

/// Region (named area) under a world position: topmost = last, for the status bar.
pub fn region_at(m: &Svmap, x: f32, z: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, g) in m.regions.iter().enumerate() {
        let (x0, x1) = (g.min[0].min(g.max[0]), g.min[0].max(g.max[0]));
        let (z0, z1) = (g.min[2].min(g.max[2]), g.min[2].max(g.max[2]));
        if x >= x0 && x <= x1 && z >= z0 && z <= z1 {
            let area = (x1 - x0) * (z1 - z0);
            if best.is_none_or(|(a, _)| area < a) {
                best = Some((area, i));
            }
        }
    }
    best.map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput};
    use shaiya_svmap::{MobEntry, Svmap};

    struct Harness {
        ctx: egui::Context,
        doc: Doc,
        st: VpState,
        t: f64,
        tool: Tool,
        paint: PaintSettings,
        layers: Layers,
        template: Option<Template>,
        overview: bool,
        draw_target: DrawTarget,
        snap: f32,
        locks: Locks,
        /// The text painted in the last frame.
        texts: Vec<String>,
        /// Held in every frame (Alt for the one-side drag).
        modifiers: Modifiers,
        /// Every request the viewport made so far (toasts).
        requests: Vec<String>,
    }

    impl Harness {
        fn new() -> Harness {
            let mut m = Svmap::new(128);
            m.mob_areas.push(MobArea { min: [40.0, 0.0, 40.0], max: [60.0, 2.0, 60.0], mobs: vec![MobEntry { mob: 1, count: 1 }] });
            m.mob_areas.push(MobArea { min: [90.0, 0.0, 90.0], max: [100.0, 2.0, 100.0], mobs: vec![] });
            m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [20.0, 0.0, 20.0], dir: 0.0 }, NpcPoint { pos: [30.0, 0.0, 20.0], dir: 0.0 }] });
            let mut doc = Doc::new(1, Some(1), None, m, None);
            // world (50, 50) at the screen centre (400, 300), 4 px per unit
            doc.view = crate::doc::View { cx: 50.0, cz: 50.0, zoom: 4.0, fitted: true };
            Harness { ctx: egui::Context::default(), doc, st: VpState::default(), t: 0.0, tool: Tool::Select, paint: PaintSettings::default(), layers: Layers::default(), template: None, overview: true, draw_target: DrawTarget::MobArea, snap: 1.0, locks: Locks::default(), texts: Vec::new(), modifiers: Modifiers::NONE, requests: Vec::new() }
        }

        fn frame(&mut self, events: Vec<Event>) -> Out {
            self.t += 0.05;
            let mut input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(self.t), events, ..Default::default() };
            input.events.insert(0, Event::ModifiersChanged(self.modifiers));
            let mut out = None;
            let name = |id: u32| format!("Map {id}");
            let (doc, st, tool, layers, locks, paint) = (&mut self.doc, &mut self.st, self.tool, &self.layers, &self.locks, self.paint);
            let (template, overview, draw_target, snap) = (self.template.as_ref(), self.overview, self.draw_target, self.snap);
            let tex = Textures::default();
            let mut place_state = crate::place_ui::PlaceUi::default();
            let mut terrain_state = crate::terrain_ui::TerrainUi::default();
            let mut full = self.ctx.run_ui(input, |ui| {
                let mut c = Ctx {
                    doc,
                    layers,
                    locks,
                    look: Lookups::none(),
                    tex: &tex,
                    tool,
                    snap,
                    drop_on_move: false,
                    draw_target,
                    npc_template: (1, 1),
                    paint,
                    minimap_alpha: 0.7,
                    pick_banner: None,
                    map_name: &name,
                    marker: None,
                    compare: None,
                    hover_rec: None,
                    level_filter: None,
                    overview,
                    template,
                    can_paste: true,
                    clean: false,
                    menu_fixes: &[],
                    heat: None,
                    rulers: false,
                    scatter: None,
                    bulk_outline: &[],
                    place: &mut place_state,
                    terrain: &mut terrain_state,
                    codepage: Default::default(),
                };
                out = Some(show(ui, &mut c, st));
            });
            self.texts.clear();
            fn collect(s: &egui::epaint::Shape, out: &mut Vec<String>) {
                match s {
                    egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_string()),
                    egui::epaint::Shape::Vec(v) => v.iter().for_each(|x| collect(x, out)),
                    _ => {}
                }
            }
            for cs in &full.shapes {
                collect(&cs.shape, &mut self.texts);
            }
            full.textures_delta.clear();
            let out = out.unwrap();
            for r in &out.requests {
                if let Request::Toast(t) = r {
                    self.requests.push(t.clone());
                }
            }
            out
        }

        fn mv(&mut self, x: f32, y: f32) {
            self.frame(vec![Event::PointerMoved(Pos2::new(x, y))]);
        }

        fn press(&mut self, x: f32, y: f32) {
            self.frame(vec![Event::PointerButton { pos: Pos2::new(x, y), button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        }

        fn release(&mut self, x: f32, y: f32) {
            self.frame(vec![Event::PointerButton { pos: Pos2::new(x, y), button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        }

        fn click(&mut self, x: f32, y: f32) {
            self.mv(x, y);
            self.press(x, y);
            self.release(x, y);
            self.frame(vec![]);
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            self.mv(from.0, from.1);
            self.press(from.0, from.1);
            let n = 4;
            for i in 1..=n {
                let f = i as f32 / n as f32;
                self.mv(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
            }
            self.release(to.0, to.1);
            self.frame(vec![]);
        }
    }

    #[test]
    fn click_selects_and_empty_click_clears() {
        let mut h = Harness::new();
        h.click(400.0, 300.0);
        assert_eq!(h.doc.selection, vec![(SectionId::MobAreas, 0)]);
        h.click(100.0, 100.0);
        assert!(h.doc.selection.is_empty());
        // the NPC pin is picked over nothing, at its screen position (world 20,20 -> x 400-120, y 300+120)
        h.click(280.0, 420.0);
        assert_eq!(h.doc.selection, vec![(SectionId::Npcs, 0)]);
    }

    #[test]
    fn drag_moves_snapped_and_undoes() {
        let mut h = Harness::new();
        h.drag((400.0, 300.0), (420.0, 316.0)); // +5 x, -4 z world units
        assert_eq!(h.doc.map.mob_areas[0].min, [45.0, 0.0, 36.0]);
        assert_eq!(h.doc.map.mob_areas[0].max, [65.0, 2.0, 56.0]);
        assert_eq!(h.doc.undo.len(), 1);
        h.doc.undo();
        assert_eq!(h.doc.map.mob_areas[0].min, [40.0, 0.0, 40.0]);
    }

    /// Alt+drag moves the svmap record only: the client half stays and a toast says so. Without Alt both move.
    #[test]
    fn alt_drag_moves_one_side_only() {
        let mut h = Harness::new();
        h.doc = crate::testutil::paired_doc();
        // portal 0 stands at world (12, 12): the screen centre
        h.doc.view = crate::doc::View { cx: 12.0, cz: 12.0, zoom: 4.0, fitted: true };
        let l0 = h.doc.part().unwrap().lists();
        h.modifiers = Modifiers::ALT;
        h.drag((400.0, 300.0), (420.0, 300.0));
        assert_eq!(h.doc.map.portals[0].pos[0], 17.0, "the svmap portal moved 5 units");
        assert_eq!(h.doc.part().unwrap().lists(), l0, "the client box did not");
        assert!(h.doc.undo.len() == 1 && h.doc.undo[0].cops.is_empty());
        assert!(h.requests.iter().any(|t| t.contains("svmap only")), "{:?}", h.requests);
        assert!(!h.doc.client_dirty());
        h.doc.undo();
        // the same drag without Alt moves both
        h.modifiers = Modifiers::NONE;
        h.requests.clear();
        h.doc.clear_selection();
        h.drag((400.0, 300.0), (420.0, 300.0));
        let c = crate::pair::box_pos(&h.doc.part().unwrap().wld.model.portals[0]);
        assert!((c[0] - 17.0).abs() < 1e-3 && (c[0] - h.doc.map.portals[0].pos[0]).abs() < 1e-3);
        assert!(h.requests.is_empty() && h.doc.client_dirty());
    }

    #[test]
    fn npc_path_point_moves_alone_and_pin_moves_all() {
        let mut h = Harness::new();
        // second path point at world (30, 20) -> screen (320, 420)
        h.drag((320.0, 420.0), (340.0, 420.0));
        let n = &h.doc.map.npcs[0];
        assert_eq!(n.points[0].pos, [20.0, 0.0, 20.0]);
        assert_eq!(n.points[1].pos, [35.0, 0.0, 20.0]);
        // the pin moves the whole NPC
        h.drag((280.0, 420.0), (280.0, 400.0));
        let n = &h.doc.map.npcs[0];
        assert_eq!(n.points[0].pos, [20.0, 0.0, 25.0]);
        assert_eq!(n.points[1].pos, [35.0, 0.0, 25.0]);
        assert_eq!(h.doc.undo.len(), 2);
    }

    #[test]
    fn resize_handle_changes_one_edge() {
        let mut h = Harness::new();
        h.click(400.0, 300.0);
        // right-middle handle at world (60, 50) -> screen (440, 300); drag to x 480 = world 70
        h.drag((440.0, 300.0), (480.0, 300.0));
        let a = &h.doc.map.mob_areas[0];
        assert_eq!(a.min, [40.0, 0.0, 40.0]);
        assert_eq!(a.max, [70.0, 2.0, 60.0]);
    }

    #[test]
    fn box_tool_draws_a_snapped_area_and_band_select_picks_inside() {
        let mut h = Harness::new();
        h.tool = Tool::Box;
        // world (10,10)..(18,14): screen x 400-200=200 .. 232, y 300+160=460 .. 444
        h.drag((240.0, 460.0), (272.0, 444.0));
        assert_eq!(h.doc.map.mob_areas.len(), 3);
        let a = &h.doc.map.mob_areas[2];
        assert_eq!((a.min[0], a.min[2], a.max[0], a.max[2]), (10.0, 10.0, 18.0, 14.0));
        assert_eq!(a.max[1] - a.min[1], 2.0);
        h.tool = Tool::Select;
        h.doc.clear_selection();
        // band over the empty top-left corner of the screen up to the two big areas' centres
        h.drag((300.0, 100.0), (700.0, 350.0));
        assert!(h.doc.is_selected(SectionId::MobAreas, 0));
        assert!(h.doc.is_selected(SectionId::MobAreas, 1));
        assert!(!h.doc.is_selected(SectionId::MobAreas, 2));
    }

    #[test]
    fn locked_section_is_not_selectable() {
        let mut h = Harness::new();
        h.locks.mob_areas = true;
        h.click(400.0, 300.0);
        assert!(h.doc.selection.is_empty());
    }

    #[test]
    fn snap_keeps_the_nearest_f32() {
        assert_eq!(snap_to(12.34, 0.1), 12.3);
        assert_eq!(snap_to(12.36, 0.1), 12.4);
        assert_eq!(snap_to(13.0, 4.0), 12.0);
    }

    // ---- Paint-blocked

    impl Harness {
        fn button(&mut self, x: f32, y: f32, button: PointerButton, pressed: bool) {
            self.frame(vec![Event::PointerButton { pos: Pos2::new(x, y), button, pressed, modifiers: Modifiers::NONE }]);
        }

        fn drag_with(&mut self, button: PointerButton, from: (f32, f32), to: (f32, f32)) {
            self.mv(from.0, from.1);
            self.button(from.0, from.1, button, true);
            for i in 1..=4 {
                let f = i as f32 / 4.0;
                self.mv(from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
            }
            self.button(to.0, to.1, button, false);
            self.frame(vec![]);
        }

        fn key(&mut self, key: egui::Key) {
            self.frame(vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        }

        fn painter() -> Harness {
            let mut h = Harness::new();
            h.tool = Tool::Paint;
            h
        }
    }

    // world (x, z) -> screen, with the harness camera (world (50, 50) at (400, 300), 4 px per unit)
    fn scr(x: f32, z: f32) -> (f32, f32) {
        (400.0 + (x - 50.0) * 4.0, 300.0 - (z - 50.0) * 4.0)
    }

    #[test]
    fn brush_stroke_blocks_cells_as_one_undo_step() {
        let mut h = Harness::painter();
        h.paint.radius = 2.0;
        let base = h.doc.map.clone();
        h.drag(scr(20.5, 30.5), scr(60.5, 30.5));
        let g = &h.doc.map.blocked;
        // a stroke 40 cells long and about 4-5 cells wide, no gaps along it
        assert!(g.count() > 150 && g.count() < 260, "{}", g.count());
        assert!((20..=60).all(|x| g.get(x, 30)), "the stroke has a gap");
        assert!(g.get(40, 32) && g.get(40, 28) && !g.get(40, 34) && !g.get(40, 26));
        assert!(!g.get(10, 30) && !g.get(70, 30));
        assert_eq!(h.doc.undo.len(), 1);
        assert_eq!(h.doc.undo[0].label, "block cells");
        assert!(h.doc.is_dirty() && h.doc.blocked_dirty.is_some());
        h.doc.undo();
        assert_eq!(h.doc.map, base);
        assert!(!h.doc.is_dirty());
        h.doc.redo();
        assert!(h.doc.map.blocked.get(40, 30));
    }

    #[test]
    fn right_button_frees_and_the_swap_flips_the_buttons() {
        let mut h = Harness::painter();
        h.paint.radius = 1.0;
        h.doc.map.blocked.fill_all(true);
        h.doc.view.fitted = true;
        h.drag_with(PointerButton::Secondary, scr(30.5, 40.5), scr(50.5, 40.5));
        assert!(!h.doc.map.blocked.get(40, 40) && h.doc.map.blocked.get(40, 45));
        assert_eq!(h.doc.undo.len(), 1);
        assert_eq!(h.doc.undo[0].label, "free cells");
        // the right button did not pan the camera
        assert_eq!((h.doc.view.cx, h.doc.view.cz), (50.0, 50.0));
        // swapped: the left button frees
        h.paint.block = false;
        h.drag((400.0, 200.0), (440.0, 200.0));
        assert!(!h.doc.map.blocked.get(55, 75));
        assert_eq!(h.doc.undo.len(), 2);
        assert_eq!(h.doc.undo[1].label, "free cells");
    }

    #[test]
    fn rectangle_invert_and_flood_fill() {
        let mut h = Harness::painter();
        h.paint.mode = PaintMode::Rect;
        // cells x 10..=14, z 10..=12 (the rectangle includes the cell under both corners)
        h.drag(scr(10.5, 10.5), scr(14.5, 12.5));
        assert_eq!(h.doc.map.blocked.count(), 5 * 3);
        assert!(h.doc.map.blocked.get(10, 10) && h.doc.map.blocked.get(14, 12) && !h.doc.map.blocked.get(15, 12));
        h.paint.mode = PaintMode::Invert;
        h.drag(scr(12.5, 11.5), scr(16.5, 11.5));
        // inverts cells x 12..=16 on row 11: 12, 13, 14 were blocked (now free), 15, 16 were free (now blocked)
        assert_eq!(h.doc.map.blocked.count(), 15 - 3 + 2);
        assert!(!h.doc.map.blocked.get(13, 11) && h.doc.map.blocked.get(16, 11));
        assert_eq!(h.doc.undo.len(), 2);
        // flood fill: a closed ring of cells, a click inside blocks the inside only
        h.doc.map.blocked.fill_all(false);
        h.doc.map.blocked.fill_rect(50, 50, 60, 60, true);
        h.doc.map.blocked.fill_rect(51, 51, 59, 59, false);
        h.paint.mode = PaintMode::Fill;
        let before = h.doc.map.blocked.count();
        h.click(scr(55.5, 55.5).0, scr(55.5, 55.5).1);
        assert_eq!(h.doc.map.blocked.count(), before + 8 * 8);
        h.click(scr(90.5, 90.5).0, scr(90.5, 90.5).1);
        // the outside is the rest of the 128 x 128 map: everything that was free is now blocked
        assert_eq!(h.doc.map.blocked.count(), 128 * 128);
        h.doc.undo();
        assert_eq!(h.doc.map.blocked.count(), before + 8 * 8);
    }

    #[test]
    fn escape_cancels_a_stroke_and_a_rectangle() {
        let mut h = Harness::painter();
        h.paint.radius = 2.0;
        h.mv(scr(30.5, 30.5).0, scr(30.5, 30.5).1);
        h.button(scr(30.5, 30.5).0, scr(30.5, 30.5).1, PointerButton::Primary, true);
        h.mv(scr(40.5, 30.5).0, scr(40.5, 30.5).1);
        h.mv(scr(50.5, 30.5).0, scr(50.5, 30.5).1);
        assert!(h.doc.map.blocked.count() > 0, "the stroke paints while dragging");
        h.key(egui::Key::Escape);
        assert_eq!(h.doc.map.blocked.count(), 0, "Esc restores the grid");
        h.button(scr(50.5, 30.5).0, scr(50.5, 30.5).1, PointerButton::Primary, false);
        assert_eq!(h.doc.undo.len(), 0);
        assert_eq!(h.doc.map.blocked.count(), 0);
    }

    #[test]
    fn a_single_click_paints_a_dab_and_radius_zero_is_one_cell() {
        let mut h = Harness::painter();
        h.paint.radius = 0.0;
        h.click(scr(70.5, 20.5).0, scr(70.5, 20.5).1);
        assert_eq!(h.doc.map.blocked.count(), 1);
        assert!(h.doc.map.blocked.get(70, 20));
        h.paint.radius = 3.0;
        h.click(scr(90.5, 90.5).0, scr(90.5, 90.5).1);
        let n = h.doc.map.blocked.count();
        assert!(n > 20 && n < 50, "{n}");
        assert_eq!(h.doc.undo.len(), 2);
    }

    // ---- Q3: templates, context menu, overview, level filter

    #[test]
    fn template_stamp() {
        let mut h = Harness::new();
        h.tool = Tool::Box;
        h.template = Some(Template { name: "wolves".into(), size: [10.0, 6.0], height: 3.0, entries: vec![[5, 3], [6, 1]] });
        // world (70.3, 30.4) is screen (481.2, 378.4); the snapped cursor is (70, 30)
        h.click(481.2, 378.4);
        assert_eq!(h.doc.map.mob_areas.len(), 3, "one area");
        let a = &h.doc.map.mob_areas[2];
        assert_eq!((a.min[0], a.min[2], a.max[0], a.max[2]), (65.0, 27.0, 75.0, 33.0), "the template's size, centred at the snapped cursor");
        assert_eq!(a.max[1] - a.min[1], 3.0);
        assert_eq!(a.mobs, vec![MobEntry { mob: 5, count: 3 }, MobEntry { mob: 6, count: 1 }]);
        assert_eq!(h.doc.undo.len(), 1, "one undo step");
        assert_eq!(h.doc.selection, vec![(SectionId::MobAreas, 2)]);
        // dragging draws a box of the dragged size with the template's entries
        h.drag((240.0, 460.0), (272.0, 444.0));
        let b = &h.doc.map.mob_areas[3];
        assert_eq!((b.min[0], b.max[0]), (10.0, 18.0));
        assert_eq!(b.mobs.len(), 2);
        // no template: a click on the ground adds nothing
        h.template = None;
        h.click(481.2, 378.4);
        assert_eq!(h.doc.map.mob_areas.len(), 4);
        // a read-only tab never gets a stamp
        h.template = Some(Template { name: "t".into(), size: [4.0, 4.0], height: 2.0, entries: vec![[1, 1]] });
        h.doc.read_only = true;
        h.click(481.2, 378.4);
        assert_eq!(h.doc.map.mob_areas.len(), 4);
    }

    #[test]
    fn context_menu_vs_pan() {
        let mut h = Harness::new();
        // a right click that does not move opens the menu on the record under it, and selects it
        h.mv(400.0, 300.0);
        h.button(400.0, 300.0, PointerButton::Secondary, true);
        h.button(400.0, 300.0, PointerButton::Secondary, false);
        h.frame(vec![]);
        let m = h.st.menu.clone().expect("the menu is open");
        assert_eq!(m.hit.map(|x| (x.sec, x.idx)), Some((SectionId::MobAreas, 0)));
        assert_eq!(h.doc.selection, vec![(SectionId::MobAreas, 0)]);
        assert_eq!(m.world, [50.0, 50.0]);
        h.key(egui::Key::Escape);
        assert!(h.st.menu.is_none(), "Esc closes it");
        // a right drag of 30 px pans and opens nothing
        let before = (h.doc.view.cx, h.doc.view.cz);
        h.drag_with(PointerButton::Secondary, (200.0, 100.0), (230.0, 100.0));
        assert!(h.st.menu.is_none());
        assert!((h.doc.view.cx - (before.0 - 30.0 / 4.0)).abs() < 0.5, "panned: {} -> {}", before.0, h.doc.view.cx);
        // a slow right click (over 400 ms) is not a menu either
        h.mv(100.0, 100.0);
        h.button(100.0, 100.0, PointerButton::Secondary, true);
        h.t += 0.6;
        h.button(100.0, 100.0, PointerButton::Secondary, false);
        assert!(h.st.menu.is_none());
        // empty ground: a menu without a record
        h.mv(100.0, 100.0);
        h.button(100.0, 100.0, PointerButton::Secondary, true);
        h.button(100.0, 100.0, PointerButton::Secondary, false);
        h.frame(vec![]);
        assert!(h.st.menu.as_ref().is_some_and(|m| m.hit.is_none()));
        // the Paint tool keeps the right button for freeing cells
        h.st.menu = None;
        h.tool = Tool::Paint;
        h.mv(100.0, 100.0);
        h.button(100.0, 100.0, PointerButton::Secondary, true);
        h.button(100.0, 100.0, PointerButton::Secondary, false);
        assert!(h.st.menu.is_none());
    }

    #[test]
    fn overview_click_moves_the_view_and_hides_when_the_whole_map_is_visible() {
        let mut h = Harness::new();
        // zoom 3: the 800 x 600 view shows 267 x 200 units, so the whole 128 map is in view and the inset is hidden
        h.doc.view.zoom = 3.0;
        h.frame(vec![]);
        let ov = overview_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0)));
        assert_eq!((ov.left(), ov.top(), ov.right(), ov.bottom()), (610.0, 410.0, 790.0, 590.0));
        h.click(700.0, 500.0);
        assert_eq!((h.doc.view.cx, h.doc.view.cz), (50.0, 50.0), "hidden: the click is a click on the map");
        // zoom 8 shows 100 x 75 units: the inset is there, and a click in it centres the view (and selects nothing)
        h.doc.view.zoom = 8.0;
        h.frame(vec![]);
        h.click(700.0, 500.0);
        assert!((h.doc.view.cx - 64.0).abs() < 0.1 && (h.doc.view.cz - 64.0).abs() < 0.1, "{:?}", (h.doc.view.cx, h.doc.view.cz));
        assert!(h.doc.selection.is_empty() && h.doc.undo.is_empty());
        // a drag from inside it follows the pointer and edits nothing
        h.drag((700.0, 500.0), (640.0, 450.0));
        assert!((h.doc.view.cx - 21.33).abs() < 0.1 && (h.doc.view.cz - 99.55).abs() < 0.1, "{:?}", (h.doc.view.cx, h.doc.view.cz));
        assert!(h.doc.undo.is_empty());
        // off: a click there is a click on the map again
        h.overview = false;
        let c0 = (h.doc.view.cx, h.doc.view.cz);
        h.click(700.0, 500.0);
        assert_eq!((h.doc.view.cx, h.doc.view.cz), c0);
    }

    #[test]
    fn ruler_steps_are_one_two_five_and_at_least_60_pixels_apart() {
        for zoom in [0.05f32, 0.3, 1.0, 4.0, 32.0] {
            let step = ruler_step(zoom);
            assert!(step * zoom >= 60.0, "zoom {zoom}: {step}");
            let m = step / 10f32.powf(step.log10().floor());
            assert!([1.0f32, 2.0, 5.0].iter().any(|k| (m - k).abs() < 1e-3), "{step} is not 1, 2 or 5 times a power of ten");
            // the next smaller step would be too close
            assert!(step * zoom < 60.0 * 2.6, "zoom {zoom}: {step} is not the smallest");
        }
        assert_eq!(ruler_step(1.0), 100.0);
        assert_eq!(ruler_step(32.0), 2.0);
    }


    #[test]
    fn rulers_ticks() {
        // at every zoom the tick step is 1, 2 or 5 times a power of ten, at least 60 px apart, and the smallest such step
        for zoom in [0.05f32, 1.0, 32.0] {
            let step = ruler_step(zoom);
            assert!(step * zoom >= 60.0, "zoom {zoom}: {step}");
            let m = step / 10f32.powf(step.log10().floor());
            assert!([1.0f32, 2.0, 5.0].iter().any(|k| (m - k).abs() < 1e-3), "{step}");
            // the previous step of the 1-2-5 series is below 60 px
            let prev = if (m - 1.0).abs() < 1e-3 || (m - 2.0).abs() < 1e-3 { step / 2.0 } else { step / 2.5 };
            assert!(prev * zoom < 60.0, "zoom {zoom}: {prev} would do too");
        }
        assert_eq!(ruler_step(0.05), 2000.0);
        assert_eq!(ruler_step(1.0), 100.0);
        assert_eq!(ruler_step(32.0), 2.0);
    }

    #[test]
    fn measure_polyline_harness() {
        let mut h = Harness::new();
        h.tool = Tool::Measure;
        // clicks on empty ground (no anchor within 8 px): (10,100) (10,70) (30,70) (30,100), 4 px per unit
        for (x, z) in [(10.0, 100.0), (10.0, 70.0), (30.0, 70.0), (30.0, 100.0)] {
            let (sx, sy) = scr(x, z);
            h.click(sx, sy);
        }
        assert_eq!(h.st.measure.points, vec![[10.0, 100.0], [10.0, 70.0], [30.0, 70.0], [30.0, 100.0]]);
        assert!(!h.st.measure.done && !h.st.measure.closed);
        // the labels: each segment's length and the summary at the end
        let all = h.texts.join(" | ");
        assert!(all.contains("30.0") && all.contains("20.0"), "segment lengths: {all}");
        assert!(all.contains("total 80.0") && all.contains("dx 20.0") && all.contains("dz 0.0") && all.contains("dy 0.0"), "summary: {all}");
        assert_eq!(h.st.measure.copy_text().as_deref(), Some("length 80.0 (3 segments)"));
        // Backspace takes the last point back, a click puts it again
        h.key(egui::Key::Backspace);
        assert_eq!(h.st.measure.points.len(), 3);
        let (sx, sy) = scr(30.0, 100.0);
        h.click(sx, sy);
        assert_eq!(h.st.measure.points.len(), 4);
        // a click on the first point closes the polygon: 4 segments, area 20 x 30
        let (fx, fy) = scr(10.0, 100.0);
        h.click(fx, fy);
        assert!(h.st.measure.closed && h.st.measure.done);
        assert_eq!(h.st.measure.copy_text().as_deref(), Some("length 100.0 (4 segments), area 600.0"));
        assert!(h.texts.join(" | ").contains("area 600.0 unit2, 600 cells"), "{:?}", h.texts);
        // a click after a finished measurement starts another; Enter ends it
        let (ax, ay) = scr(70.0, 20.0);
        h.click(ax, ay);
        assert_eq!(h.st.measure.points, vec![[70.0, 20.0]]);
        let (bx, by) = scr(70.0, 5.0);
        h.click(bx, by);
        h.key(egui::Key::Enter);
        assert!(h.st.measure.done);
        assert_eq!(h.st.measure.copy_text().as_deref(), Some("length 15.0 (1 segment)"));
        // Esc clears it
        h.key(egui::Key::Escape);
        assert!(!h.st.measure.active());
        assert!(h.doc.undo.is_empty(), "measuring never edits the map");
        // a drag is one segment; Ctrl would turn off snapping, so end it on an empty spot
        h.drag(scr(80.0, 20.0), scr(80.0, 35.0));
        assert_eq!(h.st.measure.points.len(), 2);
        assert!(h.st.measure.done);
        assert!((h.st.measure.total() - 15.0).abs() < 0.5, "{}", h.st.measure.total());
        // changing the tool drops it
        h.tool = Tool::Select;
        h.frame(vec![]);
        assert!(!h.st.measure.active());
    }

    #[test]
    fn measure_snaps_to_anchors_and_radius_mode_marks_the_portal_range() {
        let mut h = Harness::new();
        h.tool = Tool::Measure;
        // a click 1.5 px from the area's corner (40, 40) snaps to it; the area's centre (50, 50) too
        let (sx, sy) = scr(40.0, 40.0);
        h.click(sx + 1.5, sy + 1.0);
        assert_eq!(h.st.measure.points, vec![[40.0, 40.0]]);
        h.st.measure.clear();
        // radius mode: a drag from (70, 70) 160 px to the right is a circle of radius 40
        h.st.radius_mode = true;
        h.drag(scr(70.0, 70.0), (scr(70.0, 70.0).0 + 160.0, scr(70.0, 70.0).1));
        assert_eq!(h.st.measure.circle.map(|(c, r)| (c, r.round())), Some(([70.0, 70.0], 40.0)));
        assert!(h.texts.join(" | ").contains("radius 40.0 (portal range)"), "{:?}", h.texts);
        // a radius far from 40 has no ring note
        h.drag(scr(70.0, 70.0), (scr(70.0, 70.0).0 + 80.0, scr(70.0, 70.0).1));
        assert!(h.texts.join(" | ").contains("radius 20.0") && !h.texts.join(" | ").contains("portal range"), "{:?}", h.texts);
        assert_eq!(h.st.measure.copy_text().as_deref(), Some("radius 20.0"));
    }
    #[test]
    fn the_level_filter_rounds_and_leaves_unknown_levels_alone() {
        assert!(!outside_range(None, Some(5.0)));
        assert!(!outside_range(Some((10.0, 20.0)), Some(10.4)), "10.4 rounds to 10");
        assert!(outside_range(Some((10.0, 20.0)), Some(9.4)));
        assert!(outside_range(Some((10.0, 20.0)), Some(20.6)));
        assert!(!outside_range(Some((10.0, 20.0)), None), "an unknown level is not outside");
    }
}
