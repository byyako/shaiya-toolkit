//! the World view and Compare folders (special tabs) and the glue for Take from another folder. Declared from
//! `app.rs` with `#[path]` so it shares the window's state. The pure parts are in `world.rs` and `folderdiff.rs`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use shaiya_svmap::Svmap;

use super::App;
use crate::doc::Doc;
use crate::folderdiff::{self, Kind, Row, Status};
use crate::icons::{self, Icon};
use crate::layout::DockTab;
use crate::names;
use crate::settings::{path_key, CompareAgainst};
use crate::theme::{self, Pal};
use crate::world::{self, CardStats, Line, Link, Positions, CARD_H, CARD_W, GUILD_RANKING, THUMB};

/// The 2D spawn dots of every map: `(x, z)` and the level when known.
type SpawnDots = HashMap<u32, Vec<([f32; 2], Option<f32>)>>;

pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 2.0;
const STUB_W: f32 = 150.0;
const STUB_H: f32 = 44.0;

/// Which tab of the window is showing: a map, the World view or Compare folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Showing {
    #[default]
    Map,
    World,
    Compare,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    ById,
    ByLinks,
}

/// The numbers the cards and links are drawn from (recomputed when a map, the client or the checks change).
struct Cache {
    key: (u64, usize, bool, bool),
    cards: BTreeMap<u32, CardStats>,
    dots: SpawnDots,
    links: Vec<Link>,
    lines: Vec<Line>,
    /// (errors, warnings) per map when the checks ran, open tabs live.
    problems: HashMap<u32, (usize, usize)>,
    /// Findings of the Spawns tab per map when it ran.
    spawn_findings: HashMap<u32, usize>,
    dirty: BTreeSet<u32>,
}

/// The World tab's state.
#[derive(Default)]
pub struct WorldUi {
    /// World units per screen pixel scale; 0 until the first frame frames all cards.
    pub zoom: f32,
    /// The world point at the canvas's top-left corner.
    pub pan: [f32; 2],
    pub positions: Positions,
    pos_key: Option<(String, usize, usize)>,
    pub selected: Option<u32>,
    pub filter: String,
    /// The card being dragged and where the pointer holds it.
    drag: Option<(u32, [f32; 2])>,
    thumbs: HashMap<u32, TextureHandle>,
    thumb_rx: Option<Receiver<(u32, Vec<u8>)>>,
    thumb_key: Option<String>,
    cache: Option<Cache>,
    /// Arrange waits for "Replace the card positions?".
    pub ask: Option<Arrange>,
    /// The canvas of the last frame (framing, tests).
    pub canvas: Option<Rect>,
    /// Cards drawn in the last frame, and the link lines.
    pub drawn_cards: usize,
    pub drawn_lines: usize,
    /// The card and line under the pointer in the last frame.
    pub hover: Option<u32>,
    pub hover_line: Option<String>,
    /// A card was dropped this frame: the positions are saved.
    dropped: bool,
    /// `--show world:<id>@<zoom>`: frame this card at this zoom once.
    pub focus: Option<(u32, f32)>,
    /// `--show world:links`: arrange once the cards and the map kinds are known.
    pub pending_arrange: Option<Arrange>,
}

impl WorldUi {
    pub fn loading(&self) -> bool {
        self.thumb_rx.is_some()
    }

    #[cfg(test)]
    pub fn thumb_count(&self) -> usize {
        self.thumbs.len()
    }

    /// The card's rectangle in world units.
    pub fn card_rect(&self, id: u32) -> Option<Rect> {
        self.positions.get(&id).map(|p| Rect::from_min_size(Pos2::new(p[0], p[1]), Vec2::new(CARD_W, CARD_H)))
    }
}

/// Compare folders' state.
#[derive(Default)]
pub struct FolderUi {
    /// The other folder as typed.
    pub other: String,
    pub started: bool,
    pub rows: Vec<Row>,
    rx: Option<Receiver<(PathBuf, Vec<Row>)>>,
    /// The folders the rows belong to: (this, other).
    pub scanned: Option<(PathBuf, PathBuf)>,
    /// A kind turned off hides its rows.
    pub hidden: Vec<Kind>,
    pub message: Option<String>,
}

impl FolderUi {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }
}

/// Everything R5 keeps between frames.
#[derive(Default)]
pub struct R5 {
    pub showing: Showing,
    pub world_open: bool,
    pub compare_open: bool,
    pub world: WorldUi,
    pub fd: FolderUi,
}

impl R5 {
    pub fn loading(&self) -> bool {
        self.showing == Showing::World && (self.world.loading() || self.world.pending_arrange.is_some())
    }
}

enum Act {
    Select(Option<u32>),
    Open(u32),
    Arrange(Arrange),
    Frame,
}

fn plate(p: &egui::Painter, pos: Pos2, anchor: Align2, text: impl ToString, font: FontId, color: Color32) {
    let galley = p.layout_no_wrap(text.to_string(), font, color);
    let rect = anchor.anchor_size(pos, galley.size());
    p.rect_filled(rect.expand(2.0), 2.0, theme::pal(p.ctx()).overlay_bg);
    p.galley(rect.min, galley, color);
}

/// Where the segment from the centre of `r` towards `to` leaves `r`.
fn edge_point(r: Rect, to: Pos2) -> Pos2 {
    let c = r.center();
    let d = to - c;
    if d.x == 0.0 && d.y == 0.0 {
        return c;
    }
    let tx = if d.x != 0.0 { (r.width() / 2.0) / d.x.abs() } else { f32::MAX };
    let ty = if d.y != 0.0 { (r.height() / 2.0) / d.y.abs() } else { f32::MAX };
    c + d * tx.min(ty).min(1.0)
}

fn head(p: &egui::Painter, tip: Pos2, from: Pos2, color: Color32, size: f32) {
    let d = tip - from;
    let len = d.length();
    if len < 1.0 {
        return;
    }
    let u = d / len;
    let n = Vec2::new(-u.y, u.x);
    p.add(Shape::convex_polygon(vec![tip, tip - u * size + n * size * 0.45, tip - u * size - n * size * 0.45], color, Stroke::NONE));
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 < 1e-6 {
        return (p - a).length();
    }
    let t = (((p - a).x * ab.x + (p - a).y * ab.y) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

impl App {
    // -------------------------------------------------------------------------------------------------------------- tabs

    /// View > World view: opens the tab and shows it.
    pub(super) fn open_world(&mut self) {
        if self.folder.is_none() {
            self.toast("Open a map folder first.");
            return;
        }
        self.r5.world_open = true;
        self.r5.showing = Showing::World;
    }

    /// Tools > Maps > Compare folders...: opens the tab, with the other folder from the settings.
    pub(super) fn open_compare_folders(&mut self) {
        if self.folder.is_none() {
            self.toast("Open a map folder first.");
            return;
        }
        self.r5.compare_open = true;
        self.r5.showing = Showing::Compare;
        if self.r5.fd.other.is_empty() {
            self.r5.fd.other = self.settings.other_folder.clone().or_else(|| self.settings.reference_folder.clone()).unwrap_or_default();
        }
    }

    pub(super) fn close_special(&mut self, which: Showing) {
        match which {
            Showing::World => self.r5.world_open = false,
            Showing::Compare => self.r5.compare_open = false,
            Showing::Map => {}
        }
        if self.r5.showing == which {
            self.r5.showing = Showing::Map;
        }
    }

    /// The special tabs, drawn before the map tabs. Returns the one that was clicked and the one whose x was clicked.
    pub(super) fn special_tabs(&self, ui: &mut egui::Ui) -> (Option<Showing>, Option<Showing>) {
        let (mut pick, mut close) = (None, None);
        let pal = theme::pal_ui(ui);
        for (open, which, icon, name) in [(self.r5.world_open, Showing::World, Icon::World, "World"), (self.r5.compare_open, Showing::Compare, Icon::Compare, "Compare")] {
            if !open {
                continue;
            }
            let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            icons::paint(ui.painter(), r, icon, pal.text);
            if ui.selectable_label(self.r5.showing == which, name).on_hover_text(if which == Showing::World { "Every map of the folder as a card." } else { "This folder against another folder." }).clicked() {
                pick = Some(which);
            }
            if ui.small_button("x").on_hover_text("Close this tab.").clicked() {
                close = Some(which);
            }
            ui.separator();
        }
        (pick, close)
    }

    /// The central area while a special tab shows.
    pub(super) fn central_special(&mut self, ui: &mut egui::Ui) {
        match self.r5.showing {
            Showing::World => self.central_world(ui),
            Showing::Compare => self.central_compare(ui),
            Showing::Map => {}
        }
    }

    // -------------------------------------------------------------------------------------------------------------- world: data

    fn world_key(&self) -> (u64, usize, bool, bool) {
        (self.data_gen(), self.kinds.len(), self.safety.folder_check.result.is_some(), self.r2.data.is_some())
    }

    /// Cards, links and the numbers on them, recomputed when something they read changed.
    fn world_refresh(&mut self) {
        let key = self.world_key();
        if self.r5.world.cache.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let ins = self.all_map_ins();
        let maps: Vec<(u32, &Svmap)> = ins.iter().filter_map(|m| m.id.map(|i| (i, m.map))).collect();
        let links = world::links(&maps);
        let lines = world::lines(&links);
        let (cards, dots) = self.with_env(|env| {
            let mut cards: Vec<CardStats> = maps.iter().map(|(id, m)| world::stats(*id, m, env)).collect();
            world::count_in(&mut cards, &links);
            let level = |id: u32| (env.mob)(id).map(|(_, l)| l as f32).filter(|l| *l > 0.0);
            let dots: SpawnDots = maps.iter().map(|(id, m)| (*id, world::spawn_dots(m, &level))).collect();
            (cards, dots)
        });
        let mut problems: HashMap<u32, (usize, usize)> = HashMap::new();
        if let Some(r) = &self.safety.folder_check.result {
            for (id, m) in &r.maps {
                let (e, w, _) = shaiya_svmap::tally(&m.findings);
                problems.insert(*id, (e + m.read_error.is_some() as usize, w));
            }
        }
        for d in self.docs.iter().filter(|d| !d.read_only) {
            if let Some(id) = d.map_id.filter(|_| !d.findings.is_empty() || problems.contains_key(&d.map_id.unwrap_or(u32::MAX))) {
                let (e, w, _) = shaiya_svmap::tally(&d.findings);
                problems.insert(id, (e, w));
            }
        }
        let mut spawn_findings: HashMap<u32, usize> = HashMap::new();
        if let Some(sd) = &self.r2.data {
            for m in &sd.report.maps {
                if let Some(id) = m.id {
                    let n: usize = m.counts.iter().sum();
                    if n > 0 {
                        spawn_findings.insert(id, n);
                    }
                }
            }
        }
        let dirty: BTreeSet<u32> = self.docs.iter().filter(|d| d.is_dirty() && !d.read_only).filter_map(|d| d.map_id).collect();
        let cards: BTreeMap<u32, CardStats> = cards.into_iter().map(|c| (c.id, c)).collect();
        drop(ins);
        self.r5.world.cache = Some(Cache { key, cards, dots, links, lines, problems, spawn_findings, dirty });
    }

    /// Card positions for the open folder: saved ones, else the by-id grid. Rebuilt when the folder or the set of maps changes.
    fn world_positions(&mut self) {
        let Some(folder_path) = self.settings.svmap_folder.clone() else { return };
        let key = path_key(&folder_path);
        let ids: Vec<u32> = match &self.folder {
            Some(f) => f.maps.keys().copied().collect(),
            None => return,
        };
        let pk = (key.clone(), ids.len(), self.kinds.len());
        if self.r5.world.pos_key.as_ref() == Some(&pk) || self.r5.world.drag.is_some() {
            return;
        }
        let saved = self.settings.world_layout.get(&key).cloned().unwrap_or_default();
        let kinds = self.kinds.clone();
        self.r5.world.positions = world::positions(&ids, &saved, &|id| kinds.get(&id).copied().unwrap_or(false));
        self.r5.world.pos_key = Some(pk);
        self.r5.world.zoom = 0.0;
    }

    fn save_positions(&mut self) {
        let Some(folder_path) = self.settings.svmap_folder.clone() else { return };
        let key = path_key(&folder_path);
        self.settings.world_layout.insert(key, world::to_saved(&self.r5.world.positions));
    }

    pub(super) fn apply_arrange(&mut self, how: Arrange) {
        let ids: Vec<u32> = self.r5.world.positions.keys().copied().collect();
        let kinds = self.kinds.clone();
        let is_d = move |id: u32| kinds.get(&id).copied().unwrap_or(false);
        let grid = world::arrange_by_id(&ids, &is_d);
        let new = match how {
            Arrange::ById => grid,
            Arrange::ByLinks => {
                let lines = self.r5.world.cache.as_ref().map(|c| c.lines.clone()).unwrap_or_default();
                world::arrange_by_links(&ids, &lines, &grid)
            }
        };
        self.r5.world.positions = new;
        self.save_positions();
        self.r5.world.zoom = 0.0;
    }

    /// Whether any card position was ever dragged or arranged for this folder.
    pub(super) fn world_has_saved(&self) -> bool {
        self.settings.svmap_folder.as_deref().map(path_key).is_some_and(|k| self.settings.world_layout.get(&k).is_some_and(|m| !m.is_empty()))
    }

    fn world_thumbs(&mut self, ctx: &egui::Context) {
        // pictures that arrived
        if let Some(rx) = &self.r5.world.thumb_rx {
            let mut got = Vec::new();
            let mut done = false;
            loop {
                match rx.try_recv() {
                    Ok(v) => got.push(v),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
            let opts = TextureOptions::LINEAR;
            for (id, px) in got {
                let img = egui::ColorImage::from_rgba_unmultiplied([THUMB as usize, THUMB as usize], &px);
                let t = ctx.load_texture(format!("world-{id}"), img, opts);
                self.r5.world.thumbs.insert(id, t);
            }
            if done {
                self.r5.world.thumb_rx = None;
            }
            ctx.request_repaint();
        }
        let Some(f) = &self.folder else { return };
        if f.rx.is_some() {
            return;
        }
        let key = format!("{}|{}", self.settings.svmap_folder.clone().unwrap_or_default(), self.client_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default());
        if self.r5.world.thumb_key.as_ref() == Some(&key) {
            return;
        }
        self.r5.world.thumb_key = Some(key);
        self.r5.world.thumbs.clear();
        let maps: Vec<(u32, Arc<Svmap>)> = f.maps.iter().filter_map(|(id, m)| m.svmap.clone().map(|s| (*id, s))).collect();
        let client = self.client.clone();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let side = THUMB as usize;
            for (id, m) in maps {
                let mini = client.as_deref().and_then(|c| c.source.texture(&format!("interface/MiniMap/{id}.tga")).ok());
                let px = match mini {
                    Some(img) if img.width > 0 && img.height > 0 => world::downscale_square(&img.rgba, img.width as usize, img.height as usize, side),
                    _ => world::blocked_picture(&m, side),
                };
                if tx.send((id, px)).is_err() {
                    return;
                }
                ctx2.request_repaint();
            }
        });
        self.r5.world.thumb_rx = Some(rx);
    }

    // -------------------------------------------------------------------------------------------------------------- world: view

    fn central_world(&mut self, ui: &mut egui::Ui) {
        self.tab_bar(ui);
        ui.separator();
        if self.folder.is_none() {
            ui.label("Open a map folder first.");
            return;
        }
        let ctx = ui.ctx().clone();
        self.world_positions();
        self.world_refresh();
        self.world_thumbs(&ctx);
        if let Some(how) = self.r5.world.pending_arrange {
            let kinds_ready = self.client.is_none() || self.folder.as_ref().is_some_and(|f| self.kinds.len() >= f.maps.len());
            if kinds_ready && self.r5.world.cache.is_some() && !self.r5.world.positions.is_empty() {
                self.r5.world.pending_arrange = None;
                self.apply_arrange(how);
            }
        }
        let mut acts: Vec<Act> = Vec::new();
        // ---- the row of options
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.r5.world.filter).desired_width(120.0).hint_text("id or name"));
            let f = &mut self.settings.world_filter;
            ui.checkbox(&mut f.fields, "Fields");
            ui.checkbox(&mut f.dungeons, "Dungeons");
            ui.checkbox(&mut f.high_ids, "Ids from 100");
            ui.checkbox(&mut f.spawn_dots, "Monster dots");
            ui.separator();
            if ui.button("By id").on_hover_text("Fields first, then dungeons, in rows of 10.").clicked() {
                acts.push(Act::Arrange(Arrange::ById));
            }
            if ui.button("By links").on_hover_text("Place linked maps close together.").clicked() {
                acts.push(Act::Arrange(Arrange::ByLinks));
            }
            if ui.button("Frame all").on_hover_text("Fit every card in the view (Home).").clicked() {
                acts.push(Act::Frame);
            }
        });
        if let Some(how) = self.r5.world.ask {
            let mut answer = None;
            ui.horizontal(|ui| {
                ui.label("Replace the card positions?");
                if ui.button("Replace").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
            match answer {
                Some(true) => {
                    self.r5.world.ask = None;
                    self.apply_arrange(how);
                }
                Some(false) => self.r5.world.ask = None,
                None => {}
            }
        }
        let pal = theme::pal_ui(ui);
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let mut w = std::mem::take(&mut self.r5.world);
        w.canvas = Some(rect);
        self.draw_world(ui, rect, &resp, &mut w, &pal, &mut acts);
        let moved = std::mem::take(&mut w.dropped);
        self.r5.world = w;
        if moved {
            self.save_positions();
        }
        for a in acts {
            match a {
                Act::Select(s) => self.r5.world.selected = s,
                Act::Open(id) => {
                    self.r5.world.selected = Some(id);
                    self.open_map_id(id);
                    self.r5.showing = Showing::Map;
                }
                Act::Arrange(how) => {
                    if self.world_has_saved() {
                        self.r5.world.ask = Some(how);
                    } else {
                        self.apply_arrange(how);
                    }
                }
                Act::Frame => self.r5.world.zoom = 0.0,
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::Home)) && !ctx.egui_wants_keyboard_input() {
            self.r5.world.zoom = 0.0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_world(&self, ui: &egui::Ui, rect: Rect, resp: &egui::Response, w: &mut WorldUi, pal: &Pal, acts: &mut Vec<Act>) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        let filt = self.settings.world_filter.clone();
        let Some(cache) = w.cache.take() else { return };
        // the cards that show
        let shown: Vec<u32> = w.positions.keys().copied().filter(|id| filt.shows(*id, self.kinds.get(id).copied())).collect();
        let shown_set: BTreeSet<u32> = shown.iter().copied().collect();
        // stubs: map 200 and portal targets that have no file
        let all_ids: BTreeSet<u32> = w.positions.keys().copied().collect();
        let (missing, guild) = world::stubs(&all_ids, &cache.links);
        let mut stubs: Vec<(u32, Rect, bool)> = Vec::new();
        {
            let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
            for p in w.positions.values() {
                min_x = min_x.min(p[0]);
                min_y = min_y.min(p[1]);
            }
            if min_x == f32::MAX {
                min_x = 0.0;
                min_y = 0.0;
            }
            let mut x = min_x;
            let y = min_y - STUB_H - 40.0;
            let mut ids: Vec<(u32, bool)> = Vec::new();
            if guild {
                ids.push((GUILD_RANKING, true));
            }
            ids.extend(missing.iter().map(|m| (*m, false)));
            for (id, g) in ids {
                stubs.push((id, Rect::from_min_size(Pos2::new(x, y), Vec2::new(STUB_W, STUB_H)), g));
                x += STUB_W + 20.0;
            }
        }
        // ---- the first frame, Home and the arrange buttons frame everything
        let mut bounds: Option<Rect> = None;
        for id in &shown {
            if let Some(r) = w.card_rect(*id) {
                bounds = Some(bounds.map_or(r, |b| b.union(r)));
            }
        }
        for (_, r, _) in &stubs {
            bounds = Some(bounds.map_or(*r, |b| b.union(*r)));
        }
        if w.zoom <= 0.0 {
            if let Some(b) = bounds {
                let z = ((rect.width() - 24.0) / b.width()).min((rect.height() - 24.0) / b.height()).clamp(MIN_ZOOM, MAX_ZOOM);
                w.zoom = if z.is_finite() { z } else { 1.0 };
                let c = b.center();
                w.pan = [c.x - rect.width() / 2.0 / w.zoom, c.y - rect.height() / 2.0 / w.zoom];
            } else {
                w.zoom = 1.0;
                w.pan = [0.0, 0.0];
            }
        }
        let kinds_ready = self.client.is_none() || self.folder.as_ref().is_none_or(|f| self.kinds.len() >= f.maps.len());
        if let (true, Some((id, z))) = (kinds_ready, w.focus) {
            w.focus = None;
            if let Some(r) = w.card_rect(id) {
                w.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
                let c = r.center();
                w.pan = [c.x - rect.width() / 2.0 / w.zoom, c.y - rect.height() / 2.0 / w.zoom];
            }
        }
        // ---- camera: wheel zooms at the cursor, middle or right drag pans
        let to_world = |zoom: f32, pan: [f32; 2], p: Pos2| [pan[0] + (p.x - rect.min.x) / zoom, pan[1] + (p.y - rect.min.y) / zoom];
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if let Some(p) = resp.hover_pos() {
                    let before = to_world(w.zoom, w.pan, p);
                    w.zoom = (w.zoom * (scroll * 0.0035).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
                    w.pan = [before[0] - (p.x - rect.min.x) / w.zoom, before[1] - (p.y - rect.min.y) / w.zoom];
                }
            }
        }
        if resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary) {
            let d = resp.drag_delta();
            w.pan[0] -= d.x / w.zoom;
            w.pan[1] -= d.y / w.zoom;
        }
        let (zoom, pan) = (w.zoom, w.pan);
        let to_screen = |p: [f32; 2]| Pos2::new(rect.min.x + (p[0] - pan[0]) * zoom, rect.min.y + (p[1] - pan[1]) * zoom);
        let srect = |r: Rect| Rect::from_min_max(to_screen([r.min.x, r.min.y]), to_screen([r.max.x, r.max.y]));
        // ---- the card under the pointer, and the drag
        let order: Vec<u32> = {
            let mut o: Vec<u32> = shown.clone();
            if let Some(s) = w.selected.filter(|s| shown_set.contains(s)) {
                o.retain(|x| *x != s);
                o.push(s);
            }
            if let Some((d, _)) = w.drag {
                o.retain(|x| *x != d);
                o.push(d);
            }
            o
        };
        let positions = w.positions.clone();
        let card_at = |p: Pos2| order.iter().rev().copied().find(|id| positions.get(id).is_some_and(|c| srect(Rect::from_min_size(Pos2::new(c[0], c[1]), Vec2::new(CARD_W, CARD_H))).contains(p)));
        let hover_pos = resp.hover_pos();
        w.hover = hover_pos.and_then(card_at);
        if resp.drag_started_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(id) = card_at(p) {
                    let wp = to_world(zoom, pan, p);
                    if let Some(c) = w.positions.get(&id) {
                        w.drag = Some((id, [wp[0] - c[0], wp[1] - c[1]]));
                        acts.push(Act::Select(Some(id)));
                    }
                }
            }
        }
        if let Some((id, off)) = w.drag {
            if resp.dragged_by(PointerButton::Primary) {
                if let Some(p) = resp.interact_pointer_pos() {
                    let wp = to_world(zoom, pan, p);
                    w.positions.insert(id, [(wp[0] - off[0]).round(), (wp[1] - off[1]).round()]);
                }
            }
            if resp.drag_stopped() {
                w.drag = None;
                w.dropped = true;
            }
        }
        if resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                acts.push(Act::Select(card_at(p)));
            }
        }
        if resp.double_clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                if let Some(id) = card_at(p) {
                    acts.push(Act::Open(id));
                }
            }
        }
        // ---- links
        let hovered = w.hover;
        let neighbours: BTreeSet<u32> = match hovered {
            Some(h) => cache.lines.iter().filter(|l| l.touches(h)).flat_map(|l| [l.a, l.b]).collect(),
            None => BTreeSet::new(),
        };
        let stub_rect = |id: u32| stubs.iter().find(|s| s.0 == id).map(|s| s.1);
        let anchor = |id: u32| -> Option<Rect> { if shown_set.contains(&id) { w.card_rect(id) } else { stub_rect(id) } };
        let mut hover_line: Option<(f32, String)> = None;
        let mut drawn_lines = 0;
        for l in &cache.lines {
            let (Some(ra), Some(rb)) = (anchor(l.a).map(srect), anchor(l.b).map(srect)) else { continue };
            let lit = hovered.is_none_or(|h| l.touches(h));
            let fade = if lit { 1.0 } else { 0.18 };
            if l.is_loop() {
                let c = Pos2::new(ra.right() - 6.0 * zoom.max(0.6), ra.top() + 6.0 * zoom.max(0.6));
                painter.circle_stroke(c, 7.0 * zoom.max(0.6), Stroke::new(1.5, pal.weak.gamma_multiply(fade)));
                if zoom >= 0.5 {
                    painter.text(c, Align2::CENTER_CENTER, l.total().to_string(), FontId::proportional(9.0), pal.text.gamma_multiply(fade));
                }
                drawn_lines += 1;
                continue;
            }
            let (ca, cb) = (ra.center(), rb.center());
            let (pa, pb) = (edge_point(ra, cb), edge_point(rb, ca));
            let stub_b = stubs.iter().any(|s| s.0 == l.b);
            let stub_a = stubs.iter().any(|s| s.0 == l.a);
            let colour = if stub_a || stub_b {
                let sid = if stub_b { l.b } else { l.a };
                if sid == GUILD_RANKING { pal.weak } else { pal.error }
            } else if l.two_way() {
                pal.weak
            } else {
                pal.warning
            };
            let colour = colour.gamma_multiply(fade);
            let stroke = Stroke::new(l.width(), colour);
            if l.two_way() {
                painter.line_segment([pa, pb], stroke);
                head(&painter, pb, pa, colour, 9.0);
                head(&painter, pa, pb, colour, 9.0);
            } else {
                painter.extend(Shape::dashed_line(&[pa, pb], stroke, 7.0, 4.0));
                if l.a_to_b.is_empty() {
                    head(&painter, pa, pb, colour, 9.0);
                } else {
                    head(&painter, pb, pa, colour, 9.0);
                }
            }
            drawn_lines += 1;
            if let Some(h) = hover_pos {
                let d = dist_to_segment(h, pa, pb);
                if d <= 5.0 + l.width() && hover_line.as_ref().is_none_or(|x| d < x.0) {
                    hover_line = Some((d, l.text()));
                }
            }
        }
        w.drawn_lines = drawn_lines;
        // ---- stubs
        for (id, r, g) in &stubs {
            let sr = srect(*r);
            let c = if *g { pal.weak } else { pal.error };
            painter.rect_filled(sr, 4.0, c.gamma_multiply(0.18));
            painter.rect_stroke(sr, 4.0, Stroke::new(1.5, c), StrokeKind::Inside);
            let t = if *g { "guild ranking".to_string() } else { format!("map {id} (missing)") };
            painter.with_clip_rect(sr).text(sr.center(), Align2::CENTER_CENTER, t, FontId::proportional((12.0 * zoom).clamp(8.0, 20.0)), c);
        }
        // ---- cards
        let filter = w.filter.trim().to_lowercase();
        let mut drawn = 0;
        for id in &order {
            let Some(wr) = w.card_rect(*id) else { continue };
            let r = srect(wr);
            if !r.intersects(rect) {
                continue;
            }
            drawn += 1;
            let stats = cache.cards.get(id);
            let name = self.map_name(*id);
            let dungeon = self.kinds.get(id).copied();
            let matches = filter.is_empty() || id.to_string().contains(&filter) || name.to_lowercase().contains(&filter);
            let dim = !matches || hovered.is_some_and(|h| h != *id && !neighbours.contains(id));
            let a = if dim { 0.35 } else { 1.0 };
            painter.rect_filled(r, 4.0, ui.visuals().panel_fill.gamma_multiply(a));
            let thumb = Rect::from_min_size(r.min, Vec2::splat(THUMB * zoom));
            match w.thumbs.get(id) {
                Some(t) => {
                    painter.image(t.id(), thumb, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE.gamma_multiply(a));
                }
                None => {
                    painter.rect_filled(thumb, 0.0, Color32::from_gray(60).gamma_multiply(a));
                }
            }
            if filt.spawn_dots && zoom >= 0.35 {
                if let Some(dots) = cache.dots.get(id) {
                    let clip = painter.with_clip_rect(thumb.intersect(rect));
                    for (p, lv) in dots {
                        let c = names::level_colour(*lv);
                        let pos = Pos2::new(thumb.min.x + p[0] * thumb.width(), thumb.min.y + (1.0 - p[1]) * thumb.height());
                        clip.circle_filled(pos, (2.0 * zoom).max(1.2), Color32::from_rgb(c[0], c[1], c[2]).gamma_multiply(0.9 * a));
                    }
                }
            }
            // badges
            let font = FontId::proportional((10.5 * zoom).clamp(8.0, 16.0));
            if cache.dirty.contains(id) {
                painter.circle_filled(thumb.min + Vec2::splat(9.0 * zoom.max(0.6)), 5.0 * zoom.max(0.6), pal.warning);
            }
            if zoom >= 0.5 {
                if let Some((e, wn)) = cache.problems.get(id).copied().filter(|(e, wn)| e + wn > 0) {
                    let t = match (e > 0, wn > 0) {
                        (true, true) => format!("{e} E \u{b7} {wn} W"),
                        (true, false) => format!("{e} E"),
                        _ => format!("{wn} W"),
                    };
                    plate(&painter, thumb.right_top() + Vec2::new(-4.0, 4.0), Align2::RIGHT_TOP, t, font.clone(), pal.overlay_text);
                }
                if let Some(n) = cache.spawn_findings.get(id) {
                    plate(&painter, thumb.right_bottom() + Vec2::new(-4.0, -4.0), Align2::RIGHT_BOTTOM, format!("spawns {n}"), font.clone(), pal.overlay_text);
                }
            }
            // text
            let text_clip = painter.with_clip_rect(r.intersect(rect));
            if zoom >= 0.45 {
                if let Some(s) = stats {
                    let lines = world::card_lines(s, &name, dungeon);
                    let f = FontId::proportional((10.0 * zoom).clamp(8.0, 22.0));
                    let mut y = thumb.max.y + 4.0 * zoom;
                    for (k, line) in lines.iter().enumerate() {
                        let c = if k == 0 { pal.text } else { pal.weak };
                        text_clip.text(Pos2::new(r.min.x + 4.0 * zoom, y), Align2::LEFT_TOP, line, f.clone(), c.gamma_multiply(a));
                        y += 16.0 * zoom;
                    }
                }
            } else {
                text_clip.text(r.center(), Align2::CENTER_CENTER, id.to_string(), FontId::proportional((40.0 * zoom).clamp(9.0, 30.0)), Color32::WHITE);
            }
            let sel = w.selected == Some(*id);
            let stroke = if sel { Stroke::new(3.0, pal.selection) } else if hovered == Some(*id) { Stroke::new(2.0, pal.link) } else { Stroke::new(1.0, pal.weak.gamma_multiply(0.5 * a)) };
            painter.rect_stroke(r, 4.0, stroke, StrokeKind::Inside);
        }
        w.drawn_cards = drawn;
        // ---- tooltip of the hovered card and link
        w.hover_line = None;
        if let (Some(h), None) = (hover_pos, hovered) {
            if let Some((_, t)) = hover_line {
                plate(&painter, h + Vec2::new(12.0, 14.0), Align2::LEFT_TOP, &t, FontId::proportional(12.0), pal.overlay_text);
                w.hover_line = Some(t);
            }
        }
        if drawn == 0 {
            painter.text(rect.center(), Align2::CENTER_CENTER, "No card matches. Turn on Fields or Dungeons.", FontId::proportional(14.0), pal.weak);
        }
        w.cache = Some(cache);
    }

    /// The map card in the Inspector while the World tab shows.
    pub(super) fn world_card(&mut self, ui: &mut egui::Ui) {
        let pal = theme::pal_ui(ui);
        let Some(id) = self.r5.world.selected else {
            ui.label("Click a card to see its map here. Double-click opens it.");
            return;
        };
        let name = self.map_name(id);
        let dungeon = self.kinds.get(&id).copied();
        ui.heading(format!("{id}  {name}"));
        let Some(c) = self.r5.world.cache.as_ref() else { return };
        if let Some(s) = c.cards.get(&id) {
            let lines = world::card_lines(s, "", dungeon);
            ui.label(&lines[1]);
            ui.label(&lines[2]);
        }
        if c.dirty.contains(&id) {
            ui.label(RichText::new("Unsaved changes in a tab.").color(pal.warning));
        }
        if let Some((e, w)) = c.problems.get(&id).copied().filter(|(e, w)| e + w > 0) {
            ui.label(format!("Problems: {e} errors, {w} warnings."));
        }
        if let Some(n) = c.spawn_findings.get(&id) {
            ui.label(format!("Spawn findings: {n}."));
        }
        let (outs, ins): (Vec<&Link>, Vec<&Link>) = (c.links.iter().filter(|l| l.from == id && l.to != id).collect(), c.links.iter().filter(|l| l.to == id && l.from != id).collect());
        if !outs.is_empty() {
            ui.separator();
            ui.label(RichText::new("Portals out").strong());
            for l in outs {
                ui.label(format!("{} {} ({})", l.to, if l.back { self.map_name(l.to) } else { format!("{} (one way)", self.map_name(l.to)) }, l.portals.len()));
            }
        }
        if !ins.is_empty() {
            ui.separator();
            ui.label(RichText::new("Portals in").strong());
            for l in ins {
                ui.label(format!("{} {} ({})", l.from, self.map_name(l.from), l.portals.len()));
            }
        }
        ui.separator();
        if ui.button("Open map").clicked() {
            self.open_map_id(id);
            self.r5.showing = Showing::Map;
        }
    }

    // -------------------------------------------------------------------------------------------------------------- compare folders

    fn start_scan(&mut self, ctx: &egui::Context) {
        let Some(here) = self.settings.svmap_folder.clone().map(PathBuf::from) else { return };
        let other = PathBuf::from(self.r5.fd.other.trim());
        if self.r5.fd.other.trim().is_empty() || !other.is_dir() {
            self.r5.fd.message = Some("Pick the other folder first.".to_string());
            return;
        }
        let same = |a: &Path, b: &Path| a == b || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y);
        if same(&other, &here) {
            self.r5.fd.message = Some("That is this folder. Pick another one.".to_string());
            return;
        }
        self.r5.fd.message = None;
        let other_text = other.display().to_string();
        if self.settings.other_folder.as_deref() != Some(other_text.as_str()) {
            // the Changes tabs read the other folder: a new one must not reuse copies read from the old one
            self.safety.cmp.clear();
        }
        self.settings.other_folder = Some(other_text);
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        let o2 = other.clone();
        std::thread::spawn(move || {
            let rows = folderdiff::scan(&here, &o2);
            let _ = tx.send((o2, rows));
            ctx2.request_repaint();
        });
        self.r5.fd.rx = Some(rx);
        self.r5.fd.started = true;
    }

    fn poll_scan(&mut self) {
        let got = match &self.r5.fd.rx {
            Some(rx) => match rx.try_recv() {
                Ok(v) => Some(Some(v)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(None),
            },
            None => None,
        };
        match got {
            Some(Some((other, rows))) => {
                let here = self.settings.svmap_folder.clone().map(PathBuf::from).unwrap_or_default();
                self.r5.fd.rows = rows;
                self.r5.fd.scanned = Some((here, other));
                self.r5.fd.rx = None;
            }
            Some(None) => self.r5.fd.rx = None,
            None => {}
        }
    }

    pub(super) fn central_compare(&mut self, ui: &mut egui::Ui) {
        self.tab_bar(ui);
        ui.separator();
        let ctx = ui.ctx().clone();
        let Some(here) = self.settings.svmap_folder.clone() else {
            ui.label("Open a map folder first.");
            return;
        };
        if !self.r5.fd.started && !self.r5.fd.other.trim().is_empty() && Path::new(self.r5.fd.other.trim()).is_dir() {
            self.start_scan(&ctx);
        }
        self.poll_scan();
        let pal = theme::pal_ui(ui);
        ui.horizontal(|ui| {
            ui.label("This folder");
            ui.label(RichText::new(&here).weak());
        });
        let mut go = false;
        ui.horizontal(|ui| {
            ui.label("Other folder");
            let r = ui.add(egui::TextEdit::singleline(&mut self.r5.fd.other).desired_width(360.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            if ui.button("Browse...").clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.r5.fd.other = p.display().to_string();
                    go = true;
                }
            }
            if ui.add_enabled(!self.r5.fd.running(), egui::Button::new("Compare")).clicked() {
                go = true;
            }
        });
        if go {
            self.start_scan(&ctx);
        }
        if let Some(m) = &self.r5.fd.message {
            ui.label(RichText::new(m).color(pal.warning));
        }
        if self.r5.fd.running() {
            ui.label("Comparing...");
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.r5.fd.scanned.is_none() {
            ui.label("Pick the other folder. Rows show how each map differs from this folder. Take brings records from the other folder in.");
            return;
        }
        let totals = folderdiff::totals(&self.r5.fd.rows);
        ui.label(RichText::new(totals.text()).strong());
        ui.horizontal(|ui| {
            for k in Kind::ALL {
                let n = self.r5.fd.rows.iter().filter(|r| r.status.kind() == k).count();
                let on = !self.r5.fd.hidden.contains(&k);
                if ui.selectable_label(on, format!("{} {n}", k.label())).clicked() {
                    if on {
                        self.r5.fd.hidden.push(k);
                    } else {
                        self.r5.fd.hidden.retain(|x| *x != k);
                    }
                }
            }
        });
        ui.separator();
        enum RowAct {
            Pair(u32),
            Copy(u32),
            ReadOnly(u32),
            Open(u32),
        }
        let mut act: Option<RowAct> = None;
        let rows: Vec<Row> = self.r5.fd.rows.iter().filter(|r| !self.r5.fd.hidden.contains(&r.status.kind())).cloned().collect();
        egui::ScrollArea::vertical().id_salt("folder-diff").auto_shrink([false, false]).show_rows(ui, 22.0, rows.len(), |ui, range| {
            for r in &rows[range] {
                ui.horizontal(|ui| {
                    let col = match r.status {
                        Status::Same => pal.weak,
                        Status::Differs(_) => pal.warning,
                        Status::OnlyHere | Status::OnlyThere => pal.link,
                        Status::Unreadable(_) => pal.error,
                    };
                    // a double-click on the number, the name or the status opens a differing pair
                    let a = ui.add(egui::Label::new(RichText::new(format!("{:>4}", r.id)).monospace()).sense(Sense::click()));
                    let b = ui.add_sized([170.0, 18.0], egui::Label::new(self.map_name(r.id)).truncate().sense(Sense::click()));
                    let c = ui.add_sized([80.0, 18.0], egui::Label::new(RichText::new(r.status.label()).color(col)).sense(Sense::click()));
                    if matches!(r.status, Status::Differs(_)) && (a.double_clicked() || b.double_clicked() || c.double_clicked()) {
                        act = Some(RowAct::Pair(r.id));
                    }
                    match &r.status {
                        Status::Differs(c) => {
                            if ui.button("Open pair").on_hover_text("Open the map with the Changes tab against the other folder.").clicked() {
                                act = Some(RowAct::Pair(r.id));
                            }
                            ui.add(egui::Label::new(RichText::new(c.text()).weak()).truncate());
                        }
                        Status::OnlyThere => {
                            if ui.button("Copy into this folder").on_hover_text("A new unsaved map with this number. Save writes it into this folder.").clicked() {
                                act = Some(RowAct::Copy(r.id));
                            }
                            if ui.button("Open read-only").clicked() {
                                act = Some(RowAct::ReadOnly(r.id));
                            }
                        }
                        Status::OnlyHere => {
                            if ui.button("Open").clicked() {
                                act = Some(RowAct::Open(r.id));
                            }
                        }
                        Status::Unreadable(e) => {
                            ui.add(egui::Label::new(RichText::new(e).color(pal.error)).truncate());
                        }
                        Status::Same => {}
                    }
                });
            }
        });
        match act {
            Some(RowAct::Pair(id)) => self.open_pair(id),
            Some(RowAct::Copy(id)) => self.copy_into_folder(id),
            Some(RowAct::ReadOnly(id)) => self.open_other_read_only(id),
            Some(RowAct::Open(id)) => {
                self.open_map_id(id);
                self.r5.showing = Showing::Map;
            }
            None => {}
        }
    }

    /// The other folder's path of this scan.
    fn scanned_other(&self) -> Option<PathBuf> {
        self.r5.fd.scanned.as_ref().map(|s| s.1.clone())
    }

    /// Opens the map's editable tab with its Changes tab against the other folder.
    pub(super) fn open_pair(&mut self, id: u32) {
        let Some(other) = self.scanned_other() else { return };
        let other_text = other.display().to_string();
        if self.settings.other_folder.as_deref() != Some(other_text.as_str()) {
            self.safety.cmp.clear();
        }
        self.settings.other_folder = Some(other_text);
        self.open_map_id(id);
        if self.docs.get(self.active).and_then(|d| d.map_id) != Some(id) {
            return;
        }
        self.r5.showing = Showing::Map;
        self.settings.compare_against = CompareAgainst::Other;
        self.settings.layers.compare = true;
        self.open_dock(DockTab::Changes);
    }

    /// A map only the other folder has: a new unsaved tab with its content. Save writes `<this folder>/<id>.svmap`.
    pub(super) fn copy_into_folder(&mut self, id: u32) {
        let Some(other) = self.scanned_other() else { return };
        let path = crate::compare::reference_path(&other, id);
        let map = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| shaiya_svmap::read(&b).map_err(|e| e.to_string()));
        match map {
            Ok(m) => {
                let uid = self.new_uid();
                self.docs.push(Doc::new_unsaved(uid, Some(id), m));
                self.active = self.docs.len() - 1;
                self.vp = crate::view2d::VpState::default();
                self.r5.showing = Showing::Map;
                self.toast(format!("map {id} copied. Save it to write it into this folder."));
            }
            Err(e) => self.toast(format!("cannot read map {id}: {e}")),
        }
    }

    pub(super) fn open_other_read_only(&mut self, id: u32) {
        let Some(other) = self.scanned_other() else { return };
        let path = crate::compare::reference_path(&other, id);
        self.open_read_only(&path, Some(format!("{id} other folder")), Some(id));
        self.r5.showing = Showing::Map;
    }
}

