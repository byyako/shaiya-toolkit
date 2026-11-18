//! The tool windows of the editor: the Paint-blocked toolbar, Generate blocked cells (with its preview overlay), New from client
//! map and Sync from client. Declared from `app.rs` (`#[path]`), so it shares the app's private state.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};

use egui::{RichText, TextureHandle, TextureOptions};
use shaiya_clientdata::wld::WldKind;
use shaiya_svmap::{BitGrid, SectionId, Svmap};

use super::App;
use crate::doc::Doc;
use crate::generate::{self, Agreement, DungeonParams};
use crate::help_text;
use crate::sync::{self, RowKind, SyncPlan};
use crate::view2d::{PaintMode, MAX_BRUSH};

// ------------------------------------------------------------------------------------------------------------------ generate

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenAlgo {
    /// Free where an upward-facing `.dg` collision triangle covers the cell centre.
    Dungeon,
    /// Blocked where the terrain is steeper than the chosen angle.
    Slope,
}

pub struct GenPreview {
    pub target: BitGrid,
    pub runs: Vec<shaiya_svmap::Run>,
    pub agreement: Agreement,
    pub to_free: u64,
    pub to_block: u64,
    pub tex: Option<TextureHandle>,
    /// The document's undo depth the preview was made for; a different depth means the map changed under it.
    pub undo_len: usize,
}

pub enum GenState {
    Idle,
    Computing(Receiver<Result<BitGrid, String>>),
    Ready(Box<GenPreview>),
    Failed(String),
}

pub struct GenWin {
    pub uid: u64,
    pub algo: GenAlgo,
    pub slope_deg: f32,
    pub state: GenState,
}

fn preview_of(doc: &Doc, target: BitGrid, ctx: &egui::Context) -> Result<GenPreview, String> {
    let runs = doc.map.blocked.diff_runs(&target).ok_or("the generated bitmap has another size than the map")?;
    let agreement = generate::agreement(&target, &doc.map.blocked).ok_or("the generated bitmap has another size than the map")?;
    let s = doc.map.size as usize;
    let mut rgba = vec![0u8; s * s * 4];
    let (mut to_free, mut to_block) = (0u64, 0u64);
    for r in &runs {
        for cell in r.start..r.start + r.len {
            let (x, z) = ((cell % s as u64) as usize, (cell / s as u64) as usize);
            let was_blocked = doc.map.blocked.get(x as u32, z as u32);
            let o = ((s - 1 - z) * s + x) * 4;
            if was_blocked {
                to_free += 1;
                rgba[o..o + 4].copy_from_slice(&[60, 230, 100, 150]);
            } else {
                to_block += 1;
                rgba[o..o + 4].copy_from_slice(&[255, 60, 60, 170]);
            }
        }
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([s, s], &rgba);
    let opts = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: None };
    let tex = Some(ctx.load_texture("generate-preview", img, opts));
    Ok(GenPreview { target, runs, agreement, to_free, to_block, tex, undo_len: doc.undo.len() })
}

// ------------------------------------------------------------------------------------------------------------------ new map

/// What the New-from-client dialog shows about a client map.
#[derive(Clone, Debug)]
pub struct NewInfo {
    pub dungeon: bool,
    /// The size to create (fields: the `.wld` terrain; dungeons: the `.dg` bounds rule, a suggestion).
    pub size: u32,
    pub note: String,
    pub counts: String,
}

pub struct NewMapWin {
    pub filter: String,
    pub selected: Option<u32>,
    pub size: u32,
    pub sync: bool,
    pub generate: bool,
    pub info: HashMap<u32, Result<NewInfo, String>>,
}

impl NewMapWin {
    fn new() -> NewMapWin {
        NewMapWin { filter: String::new(), selected: None, size: 0, sync: true, generate: true, info: HashMap::new() }
    }
}

// ------------------------------------------------------------------------------------------------------------------ sync

pub struct SyncWin {
    pub uid: u64,
    pub map_id: u32,
    pub plan: SyncPlan,
    pub undo_len: usize,
}

// ------------------------------------------------------------------------------------------------------------------ App impl

impl App {
    pub(super) fn gen_busy(&self) -> bool {
        self.gen.as_ref().is_some_and(|g| matches!(g.state, GenState::Computing(_)))
    }

    pub(super) fn gen_preview_texture(&self, uid: u64) -> Option<TextureHandle> {
        match &self.gen {
            Some(g) if g.uid == uid => match &g.state {
                GenState::Ready(p) => p.tex.clone(),
                _ => None,
            },
            _ => None,
        }
    }

    // ---- Paint-blocked toolbar -----------------------------------------------------------------------------------------

    pub(super) fn paint_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label("paint");
        egui::ComboBox::from_id_salt("paint-mode").width(110.0).selected_text(self.paint.mode.label()).show_ui(ui, |ui| {
            for m in PaintMode::ALL {
                ui.selectable_value(&mut self.paint.mode, m, m.label());
            }
        });
        if self.paint.mode == PaintMode::Brush {
            ui.label("radius");
            ui.add(egui::DragValue::new(&mut self.paint.radius).range(0.0..=MAX_BRUSH).speed(0.2).max_decimals(0)).on_hover_text("Brush radius in cells, 0 = one cell. [ and ] change it.");
        }
        let left = if self.paint.block { "left: block, right: free" } else { "left: free, right: block" };
        if ui.button(left).on_hover_text("Click to swap what the left and right mouse buttons do").clicked() {
            self.paint.block = !self.paint.block;
        }
        let have = self.docs.get(self.active).is_some();
        if ui.add_enabled(have, egui::Button::new("fill map")).on_hover_text("Block every cell (one undo step)").clicked() {
            if let Some(d) = self.docs.get_mut(self.active) {
                let mut g = d.map.blocked.clone();
                g.fill_all(true);
                d.commit_blocked("fill map (all blocked)", &g);
            }
        }
        if ui.add_enabled(have, egui::Button::new("clear map")).on_hover_text("Free every cell (one undo step)").clicked() {
            if let Some(d) = self.docs.get_mut(self.active) {
                let mut g = d.map.blocked.clone();
                g.fill_all(false);
                d.commit_blocked("clear map (all free)", &g);
            }
        }
        if let Some(d) = self.docs.get(self.active) {
            let cells = d.map.size as u64 * d.map.size as u64;
            let n = d.map.blocked.count();
            ui.label(RichText::new(format!("{n} blocked ({:.1} %)", if cells == 0 { 0.0 } else { n as f64 * 100.0 / cells as f64 })).weak());
        }
    }

    /// Writes the dirty part of the active map's blocked grid into its overlay texture (or builds the texture).
    pub(super) fn ensure_blocked_texture(&mut self, ctx: &egui::Context) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if !self.settings.layers.blocked {
            return;
        }
        let s = d.map.size;
        let opts = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: None };
        match self.blocked_tex.get_mut(&d.uid) {
            None => {
                if d.map.blocked.count() == 0 && d.blocked_dirty.is_none() {
                    return;
                }
                let img = blocked_image(&d.map, (0, 0, s, s));
                let h = ctx.load_texture("blocked", img, opts);
                self.blocked_tex.insert(d.uid, h);
                d.blocked_dirty = None;
            }
            Some(h) => {
                if let Some(r) = d.blocked_dirty.take() {
                    let r = (r.0.min(s), r.1.min(s), r.2.min(s), r.3.min(s));
                    if r.0 < r.2 && r.1 < r.3 {
                        // texture row 0 is the north edge (largest z)
                        let pos = [r.0 as usize, (s - r.3) as usize];
                        h.set_partial(pos, blocked_image(&d.map, r), opts);
                    }
                }
            }
        }
    }

    // ---- Generate blocked cells ---------------------------------------------------------------------------------------

    pub(super) fn open_generate(&mut self, algo: Option<GenAlgo>) {
        let Some(d) = self.docs.get(self.active) else { return };
        let is_dungeon = self.current_assets().is_some_and(|a| a.is_dungeon);
        let algo = algo.unwrap_or(if is_dungeon { GenAlgo::Dungeon } else { GenAlgo::Slope });
        self.gen = Some(GenWin { uid: d.uid, algo, slope_deg: 50.0, state: GenState::Idle });
    }

    pub(super) fn start_generation(&mut self) {
        let Some(g) = &mut self.gen else { return };
        let Some(d) = self.docs.iter().find(|d| d.uid == g.uid) else { return };
        let wld = self.docs.iter().find(|x| x.uid == g.uid).and_then(|x| x.map_id).and_then(|id| match self.slots.get(&id) {
            Some(super::Slot::Ready(r)) => r.assets.wld.clone(),
            _ => None,
        });
        let Some(wld) = wld else {
            g.state = GenState::Failed("No client data for this map: open the client folder and wait for it to load.".to_string());
            return;
        };
        let (size, algo, deg) = (d.map.size, g.algo, g.slope_deg);
        let rules = self.client.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r: Result<BitGrid, String> = match algo {
                GenAlgo::Dungeon => match (&rules, wld.kind == WldKind::Dungeon && !wld.dg.is_empty()) {
                    (Some(rules), true) => rules.source.dg(&wld.dg).map_err(|e| e.to_string()).map(|dg| {
                        let flat = dg.flatten();
                        generate::dungeon_blocked(&flat.collision_points, &flat.collision_indices, size, &DungeonParams::default())
                    }),
                    _ => Err("This map has no dungeon geometry (.dg); use Field from slope for a field.".to_string()),
                },
                GenAlgo::Slope => match &wld.terrain {
                    Some(t) => Ok(generate::slope_blocked(t, size, deg)),
                    None => Err("This map has no terrain heights; use Dungeon from collision for a dungeon.".to_string()),
                },
            };
            let _ = tx.send(r);
        });
        g.state = GenState::Computing(rx);
    }

    pub(super) fn generate_window(&mut self, ctx: &egui::Context) {
        let Some(mut g) = self.gen.take() else { return };
        let Some(di) = self.docs.iter().position(|d| d.uid == g.uid) else { return };
        // collect a finished computation
        if let GenState::Computing(rx) = &g.state {
            match rx.try_recv() {
                Ok(Ok(grid)) => {
                    g.state = match preview_of(&self.docs[di], grid, ctx) {
                        Ok(p) => GenState::Ready(Box::new(p)),
                        Err(e) => GenState::Failed(e),
                    };
                }
                Ok(Err(e)) => g.state = GenState::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => g.state = GenState::Failed("the generator stopped".to_string()),
            }
        }
        // the map changed while the preview was up: redo the overlay against the new grid
        if let GenState::Ready(p) = &g.state {
            if p.undo_len != self.docs[di].undo.len() {
                let target = p.target.clone();
                g.state = match preview_of(&self.docs[di], target, ctx) {
                    Ok(p) => GenState::Ready(Box::new(p)),
                    Err(e) => GenState::Failed(e),
                };
            }
        }
        let assets = self.current_assets();
        let is_dungeon = assets.as_ref().is_some_and(|a| a.is_dungeon);
        let has_terrain = assets.as_ref().is_some_and(|a| a.wld.as_ref().is_some_and(|w| w.terrain.is_some()));
        let has_client = self.client.is_some();
        let mut open = true;
        let (mut compute, mut apply) = (false, false);
        egui::Window::new("Generate blocked cells").default_pos([300.0, 90.0]).collapsible(false).open(&mut open).default_width(420.0).show(ctx, |ui| {
            if !has_client {
                ui.label("Open the client folder first: both generators read the client's geometry.");
            }
            let before = (g.algo, g.slope_deg);
            ui.add_enabled_ui(has_client, |ui| {
                ui.radio_value(&mut g.algo, GenAlgo::Dungeon, "Dungeon from collision (experimental)")
                    .on_hover_text("Free where an upward-facing triangle of the .dg collision mesh covers the cell centre; everything else blocked.");
                ui.radio_value(&mut g.algo, GenAlgo::Slope, "Field from slope").on_hover_text("Block cells whose terrain slope is steeper than the angle. The original field maps block almost nothing.");
                if g.algo == GenAlgo::Slope {
                    ui.horizontal(|ui| {
                        ui.label("steeper than");
                        ui.add(egui::DragValue::new(&mut g.slope_deg).range(5.0..=89.0).speed(0.5).suffix(" deg"));
                    });
                }
            });
            if before != (g.algo, g.slope_deg) {
                g.state = GenState::Idle;
            }
            match g.algo {
                GenAlgo::Dungeon if !is_dungeon && has_client => {
                    ui.colored_label(crate::theme::pal_ui(ui).warning, "This map is a field: it has no .dg collision mesh.");
                }
                GenAlgo::Slope if !has_terrain && has_client => {
                    ui.colored_label(crate::theme::pal_ui(ui).warning, "This map is a dungeon: it has no terrain heights.");
                }
                _ => {}
            }
            if g.algo == GenAlgo::Dungeon {
                ui.label(RichText::new("Against the original maps this agreed with 99.5 % of the cells on map 3, 97.8 % on map 65 and 93.1 % on map 13: it can leave floor free that the originals block (round rooms inside a square floor). Always check the preview.").weak().small());
            }
            ui.horizontal(|ui| {
                let can = has_client && !matches!(g.state, GenState::Computing(_));
                if ui.add_enabled(can, egui::Button::new("Compute preview")).clicked() {
                    compute = true;
                }
                if matches!(g.state, GenState::Computing(_)) {
                    ui.spinner();
                    ui.label("computing...");
                }
            });
            match &g.state {
                GenState::Failed(e) => {
                    ui.colored_label(crate::theme::pal_ui(ui).error, e);
                }
                GenState::Ready(p) => {
                    ui.separator();
                    ui.label(RichText::new("Preview on the map: green = will be freed, red = will be blocked").strong());
                    ui.label(format!("{} cells change ({} freed, {} blocked).", p.to_free + p.to_block, p.to_free, p.to_block));
                    let a = &p.agreement;
                    ui.label(format!("Same state as the map's current bitmap: {:.2} % of {} cells (an all-blocked map would score {:.2} %).", a.percent(), a.cells, a.all_blocked_percent()));
                    ui.label(format!("Of the cells the current bitmap leaves free, {:.1} % stay free; of the cells generated free, {:.1} % were free.", a.free_recall(), a.free_precision()));
                    ui.horizontal(|ui| {
                        let any = !p.runs.is_empty();
                        if ui.add_enabled(any, egui::Button::new("Apply (one undo step)")).clicked() {
                            apply = true;
                        }
                        if !any {
                            ui.label("nothing would change");
                        }
                    });
                }
                _ => {}
            }
        });
        if compute {
            self.gen = Some(g);
            self.start_generation();
            return;
        }
        if apply {
            if let GenState::Ready(p) = &g.state {
                let label = match g.algo {
                    GenAlgo::Dungeon => "generate blocked cells (dungeon from collision)".to_string(),
                    GenAlgo::Slope => format!("generate blocked cells (field slope > {:.0} deg)", g.slope_deg),
                };
                let n = self.docs[di].commit_blocked(&label, &p.target);
                self.toast(format!("changed {n} cells"));
            }
            return;
        }
        if open {
            self.gen = Some(g);
        }
    }

    // ---- New from client map ------------------------------------------------------------------------------------------

    pub(super) fn open_new_from_client(&mut self) {
        if self.client.is_none() {
            self.toast("Open the client folder first (File > Open client folder...)");
            return;
        }
        self.newmap = Some(NewMapWin::new());
    }

    /// Size and counts the dialog shows for a client map.
    pub(super) fn new_info(&self, id: u32) -> Result<NewInfo, String> {
        let rules = self.client.as_ref().ok_or("no client folder")?;
        let w = rules.wld(id).ok_or_else(|| format!("world/{id}.wld could not be read"))?;
        let counts = format!("{} NPCs, {} portals, {} regions, {} bind areas in the .wld", w.npcs.len(), w.portals.len(), w.areas.len(), w.zone_groups.len());
        match (&w.terrain, w.kind == WldKind::Dungeon) {
            (Some(t), _) => Ok(NewInfo { dungeon: false, size: t.size, note: format!("field, size {} from the .wld terrain", t.size), counts }),
            (None, true) => {
                let dg = rules.source.dg(&w.dg).map_err(|e| e.to_string())?;
                let size = shaiya_clientdata::dg::dungeon_map_size(&dg.bounds);
                Ok(NewInfo {
                    dungeon: true,
                    size,
                    note: format!(
                        "dungeon, suggested size {size} (the .dg bounds {:.0} x {:.0} rounded up to a multiple of 8). 46 of the 50 original dungeon maps follow this rule; maps 3, 27, 48 and 49 use 1024, so the size can be changed.",
                        dg.bounds.max[0], dg.bounds.max[2]
                    ),
                    counts,
                })
            }
            _ => Err("this .wld has neither terrain nor a dungeon".to_string()),
        }
    }

    pub(super) fn new_map_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.newmap.take() else { return };
        let ids: Vec<u32> = self.client.as_ref().map(|c| c.source.map_ids()).unwrap_or_default();
        let filter = w.filter.to_lowercase();
        let list: Vec<(u32, String)> = ids
            .iter()
            .map(|id| (*id, self.map_name(*id)))
            .filter(|(id, name)| filter.is_empty() || name.to_lowercase().contains(&filter) || id.to_string().contains(&filter))
            .collect();
        if let Some(id) = w.selected {
            if !w.info.contains_key(&id) {
                let info = self.new_info(id);
                if let Ok(i) = &info {
                    w.size = i.size;
                }
                w.info.insert(id, info);
            }
        }
        let exists_in_folder = |id: u32| self.folder.as_ref().is_some_and(|f| f.maps.contains_key(&id));
        let mut open = true;
        let mut create: Option<(u32, u32, bool, bool)> = None;
        egui::Window::new("New map from a client map").default_pos([300.0, 90.0]).collapsible(false).open(&mut open).default_size([440.0, 520.0]).show(ctx, |ui| {
            ui.label("Starts a new, unsaved map with the size, NPCs, portals, regions and bind areas of the client's own copy (world/<id>.wld). Monster areas, section A and B stay empty.");
            ui.add(egui::TextEdit::singleline(&mut w.filter).hint_text("filter by id or name").desired_width(f32::INFINITY));
            egui::ScrollArea::vertical().id_salt("newmap-list").max_height(220.0).auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                for r in range {
                    let (id, name) = &list[r];
                    let text = format!("{id:>3}  {name}{}", if exists_in_folder(*id) { "   (has an .svmap in the open folder)" } else { "" });
                    if ui.selectable_label(w.selected == Some(*id), RichText::new(text).monospace()).clicked() {
                        w.selected = Some(*id);
                        if let Some(Ok(i)) = w.info.get(id) {
                            w.size = i.size;
                        }
                    }
                }
            });
            ui.separator();
            match w.selected.and_then(|id| w.info.get(&id).map(|i| (id, i.clone()))) {
                None => {
                    ui.label("Pick a map.");
                }
                Some((_, Err(e))) => {
                    ui.colored_label(crate::theme::pal_ui(ui).error, e);
                }
                Some((id, Ok(i))) => {
                    ui.label(&i.note);
                    ui.label(&i.counts);
                    if i.dungeon {
                        ui.horizontal(|ui| {
                            ui.label("size");
                            ui.add(egui::DragValue::new(&mut w.size).range(8..=2048).speed(8));
                            if ui.small_button("suggested").clicked() {
                                w.size = i.size;
                            }
                        });
                    } else {
                        w.size = i.size;
                    }
                    ui.checkbox(&mut w.sync, "fill NPCs, portals, regions and bind areas from the client");
                    if i.dungeon {
                        ui.checkbox(&mut w.generate, "then preview blocked cells generated from the collision mesh").on_hover_text("Fields start with every cell free; dungeons usually need a generated bitmap.");
                    }
                    if exists_in_folder(id) {
                        ui.colored_label(crate::theme::pal_ui(ui).warning, format!("The open folder already has {id}.svmap. This map is a new document: Save as asks where to write it."));
                    }
                    if ui.button("Create").clicked() {
                        create = Some((id, w.size, w.sync, w.generate && i.dungeon));
                    }
                }
            }
        });
        if let Some((id, size, sync, generate)) = create {
            self.create_from_client(id, size, sync, generate);
            return;
        }
        if open {
            self.newmap = Some(w);
        }
    }

    pub(super) fn create_from_client(&mut self, id: u32, size: u32, sync: bool, generate: bool) {
        let uid = self.new_uid();
        let mut d = Doc::new_unsaved(uid, Some(id), Svmap::new(size));
        if sync {
            if let Some(w) = self.client.as_ref().and_then(|c| c.wld(id)) {
                let t = sync::target_from_wld(&w);
                let p = sync::plan(&d.map, &t);
                d.commit(sync::step_label(&p), p.ops(&d.map));
            }
        }
        self.slots.remove(&id); // the dungeon floor raster is sized by the map: rebuild for the new size
        self.docs.push(d);
        self.active = self.docs.len() - 1;
        self.vp = crate::view2d::VpState::default();
        self.r5.showing = super::Showing::Map;
        if generate {
            self.open_generate(Some(GenAlgo::Dungeon));
        }
        self.toast(format!("new map {id}, size {size}: not saved yet (File > Save as...)"));
    }

    // ---- Sync from client ---------------------------------------------------------------------------------------------

    pub(super) fn open_sync(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(id) = d.map_id else {
            self.toast("This map has no id (its file name is not a number), so no client map matches it");
            return;
        };
        let Some(w) = self.sync_wld(self.active) else {
            self.toast(format!("No client data for map {id}: open the client folder first"));
            return;
        };
        let d = &self.docs[self.active];
        let plan = sync::plan(&d.map, &sync::target_from_wld(&w));
        self.sync = Some(SyncWin { uid: d.uid, map_id: id, plan, undo_len: d.undo.len() });
    }

    /// The client map a sync compares with: the tab's own client half (with its unsaved edits) when it has one, else the client's
    /// file.
    fn sync_wld(&self, di: usize) -> Option<std::sync::Arc<shaiya_clientdata::wld::Wld>> {
        let d = self.docs.get(di)?;
        match d.part() {
            Some(p) => Some(p.wld_arc()),
            None => self.client.as_ref().and_then(|c| c.wld(d.map_id?)),
        }
    }

    /// Tools > Sync selected record from client: the one selected NPC, portal, region or bind area takes its `.wld` value
    /// (`sync::sync_record`; one undo step, nothing else changes).
    pub(super) fn sync_selected_record(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some((section, index)) = d.single() else {
            self.toast("Select exactly one record first");
            return;
        };
        let Some(id) = d.map_id else {
            self.toast("This map has no id (its file name is not a number), so no client map matches it");
            return;
        };
        let Some(w) = self.sync_wld(self.active) else {
            self.toast(format!("No client data for map {id}: open the client folder first"));
            return;
        };
        let d = &mut self.docs[self.active];
        let msg = match sync::sync_record(d, &w, section, index) {
            Ok(true) => format!("{} #{index} now has the client's value", section.label()),
            Ok(false) => format!("{} #{index} already agrees with the client", section.label()),
            Err(e) => e,
        };
        self.toast(msg);
    }

    pub(super) fn sync_window(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.sync.take() else { return };
        let Some(di) = self.docs.iter().position(|d| d.uid == s.uid) else { return };
        if self.docs[di].undo.len() != s.undo_len {
            // the map changed under the dialog: compare again
            if let Some(w) = self.sync_wld(di) {
                s.plan = sync::plan(&self.docs[di].map, &sync::target_from_wld(&w));
                s.undo_len = self.docs[di].undo.len();
            }
        }
        let mut open = true;
        let mut apply = false;
        egui::Window::new(format!("Sync map {} with the client", s.map_id)).default_pos([300.0, 90.0]).collapsible(false).open(&mut open).default_size([620.0, 520.0]).show(ctx, |ui| {
            ui.label("Each section is compared with world/<id>.wld. Tick the rows to take over; applying is one undo step. Records the .wld does not have start unticked (deleting is opt-in).");
            let two_way = self.docs[di].part().is_some();
            if two_way {
                ui.label(RichText::new(help_text::tip("sync-take-client")).weak().small());
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 40.0).show(ui, |ui| {
                for (k, sec) in sync::SECTIONS.iter().enumerate() {
                    let n_rows = s.plan.rows_of(*sec).count();
                    let head = format!("{}: map {}, client {}, in agreement {}, differences {}", sec.label(), s.plan.have_count[k], s.plan.want_count[k], s.plan.same[k], n_rows);
                    egui::CollapsingHeader::new(RichText::new(head).strong()).id_salt(("sync", k)).default_open(n_rows > 0).show(ui, |ui| {
                        if n_rows == 0 {
                            ui.label("identical");
                            return;
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("tick all").clicked() {
                                s.plan.set_section(*sec, true);
                            }
                            if ui.small_button("untick all").clicked() {
                                s.plan.set_section(*sec, false);
                            }
                            if two_way {
                                if ui.small_button("all take client").clicked() {
                                    s.plan.set_section_dir(*sec, sync::Take::Client);
                                }
                                if ui.small_button("all take server").clicked() {
                                    s.plan.set_section_dir(*sec, sync::Take::Server);
                                }
                            }
                        });
                        let rows: Vec<usize> = s.plan.rows_of(*sec).map(|(i, _)| i).collect();
                        for ri in rows {
                            let r = &mut s.plan.rows[ri];
                            let (word, col) = match r.kind {
                                RowKind::Added => ("add", crate::theme::pal_ui(ui).compare_add),
                                RowKind::Removed => ("delete", crate::theme::pal_ui(ui).compare_remove),
                                RowKind::Changed => ("change", crate::theme::pal_ui(ui).compare_change),
                            };
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut r.checked, "");
                                ui.label(RichText::new(format!("{word} #{}", r.index)).color(col).monospace());
                                if two_way {
                                    let label = if r.dir == sync::Take::Client { "take client" } else { "take server" };
                                    egui::ComboBox::from_id_salt(("sync-dir", ri)).width(92.0).selected_text(label).show_ui(ui, |ui| {
                                        ui.selectable_value(&mut r.dir, sync::Take::Client, "take client");
                                        ui.selectable_value(&mut r.dir, sync::Take::Server, "take server");
                                    });
                                }
                                ui.add(egui::Label::new(&r.text).truncate());
                            });
                        }
                    });
                }
                if s.plan.is_empty() {
                    ui.label("The map already matches the client in all four sections.");
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let n = s.plan.checked_count();
                let to_client = s.plan.server_count();
                let extra = if to_client > 0 { format!(" ({to_client} to the client map)") } else { String::new() };
                if ui.add_enabled(n > 0, egui::Button::new(format!("Apply {n} change{}{extra}", if n == 1 { "" } else { "s" }))).clicked() {
                    apply = true;
                }
                if s.plan.rows.iter().any(|r| r.section == SectionId::Portals && r.checked) {
                    ui.label(RichText::new("portal order and count are what the client sends: check the destinations after").weak().small());
                }
            });
        });
        if apply {
            let ops = s.plan.ops(&self.docs[di].map);
            let cops = match self.docs[di].part() {
                Some(p) if s.plan.server_count() > 0 => s.plan.client_ops(&mut p.lists()),
                _ => Vec::new(),
            };
            let label = sync::step_label(&s.plan);
            let n = ops.len() + cops.len();
            // neither side is mirrored: each row says which side it changes
            match self.docs[di].commit_both(label, ops, cops) {
                Ok(()) => self.toast(format!("synced with the client: {n} operation(s), one undo step")),
                Err(e) => self.toast(e),
            }
            return;
        }
        if open {
            self.sync = Some(s);
        }
    }
}

/// The overlay picture of the cells in the half-open rectangle (x0, z0, x1, z1): row 0 is the north edge (largest z).
fn blocked_image(m: &Svmap, r: (u32, u32, u32, u32)) -> egui::ColorImage {
    let (w, h) = ((r.2 - r.0) as usize, (r.3 - r.1) as usize);
    let mut rgba = vec![0u8; w * h * 4];
    for z in r.1..r.3 {
        let row = (r.3 - 1 - z) as usize;
        for x in r.0..r.2 {
            if m.blocked.get(x, z) {
                let o = (row * w + (x - r.0) as usize) * 4;
                rgba[o..o + 4].copy_from_slice(&[230, 40, 40, 102]);
            }
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}
