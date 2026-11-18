//! Parts of the window: Find in all maps with select similar / matching / all and
//! Replace monster across maps, bookmarks and the name prompt, spawn templates, the portal links window, Go to destination and Drop
//! destination to ground, the viewport context menu's actions, the JSON clipboard, the level filter button and the exports. Declared
//! from `app.rs` (`#[path]`), so it shares the app's private state.
//!
//! Every edit goes through `Doc::commit` (one undo step), which refuses on a read-only tab; the paths that touch other tabs
//! (Replace monster across maps) skip a map whose only tab is read-only, and nothing here ever saves a map.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Key, RichText};
use shaiya_clientdata::AreaNames;
use shaiya_svmap::{SectionId, Svmap};

use super::{gm_move_text, position_text, App, Pending, Place, Slot};
use crate::layout::DockTab;
use crate::commands::Cmd;
use crate::doc::Sel;
use crate::edit::{self, Op, Rec};
use crate::export;
use crate::inspector::Incoming;
use crate::log;
use crate::palette::Resolved;
use crate::search::{self, Env, Hit, MapIn, Query};
use crate::settings::{Bookmark, Template};
use crate::view2d::{self, CtxAct};

/// What the last search ran with: the query text, its switches, the editing generation, the view box and the folder stamp.
type SearchKey = (String, bool, bool, u64, [i32; 4], Option<u64>);

// ------------------------------------------------------------------------------------------------------------------ state

/// The Find tab.
#[derive(Default)]
pub struct FindState {
    pub query: String,
    /// Scope: false = all maps, true = this map.
    pub this_map: bool,
    /// With "this map": only the records inside the current view.
    pub inside_view: bool,
    pub focus: bool,
    pub results: Vec<Hit>,
    key: Option<SearchKey>,
    pub selected: Option<usize>,
    /// What the last Replace monster did (with a Save all button).
    pub summary: Option<String>,
    pub note: String,
    pub ms: f32,
}

pub enum PromptKind {
    AddBookmark { x: f32, z: f32, zoom: f32 },
    /// The bookmark list it belongs to (folder and map) and the name it has now.
    RenameBookmark { key: String, old: String },
    SaveTemplate { uid: u64, area: usize },
}

/// A small window asking for one line of text (a bookmark or template name).
pub struct TextPrompt {
    pub kind: PromptKind,
    pub text: String,
    first: bool,
}

impl TextPrompt {
    /// Renames the bookmark `old` of the bookmark list `key` (a folder and a map).
    pub fn rename_bookmark(key: String, old: String) -> TextPrompt {
        TextPrompt { text: old.clone(), kind: PromptKind::RenameBookmark { key, old }, first: true }
    }

    /// A rename that is already typed (tests finish it without a window).
    #[cfg(test)]
    pub fn rename_bookmark_to(key: String, old: String, new: String) -> TextPrompt {
        TextPrompt { text: new, kind: PromptKind::RenameBookmark { key, old }, first: false }
    }
}

enum PngStage {
    /// The picture is taken after a few frames without handles, hover outline, overview or legend.
    Clean(u32),
    Wait(Arc<Mutex<Option<Arc<egui::ColorImage>>>>, Arc<AtomicBool>),
}

pub struct PngJob {
    path: PathBuf,
    stage: PngStage,
}

/// A Drop destination to ground waiting for the destination map's heights to load.
pub struct DropJob {
    uid: u64,
    portal: usize,
    dest: u32,
}

#[derive(Default)]
pub struct Q3 {
    pub find: FindState,
    pub prompt: Option<TextPrompt>,
    /// The name of the template the Box tool uses.
    pub template: Option<String>,
    pub links_open: bool,
    pub links_only_problems: bool,
    links: Option<(u64, Arc<search::Links>)>,
    /// Region names of the maps the search has looked at (read from the client once).
    pub names: RefCell<HashMap<u32, Option<Arc<AreaNames>>>>,
    png: Option<PngJob>,
    pub(super) drops: Vec<DropJob>,
}

impl Q3 {
    pub fn forget_client(&mut self) {
        self.names.borrow_mut().clear();
        self.links = None;
        self.find.key = None;
    }
}

impl App {
    // -------------------------------------------------------------------------------------------------------------- env

    /// A counter that moves whenever anything a search or a link table reads changes.
    pub(super) fn data_gen(&self) -> u64 {
        let mut g = self.docs.len() as u64;
        for d in &self.docs {
            g = g.wrapping_mul(1_000_003).wrapping_add(d.uid * 7919 + d.edit_gen + d.base_gen);
        }
        if let Some(f) = &self.folder {
            g = g.wrapping_add(f.maps.values().filter(|m| m.svmap.is_some()).count() as u64 * 31);
        }
        g ^ ((self.client.is_some() as u64) << 40) ^ ((self.safety.folder_check.result.is_some() as u64) << 41)
    }

    /// The names the search and the exports use, from the client and the sysmsg when they are open.
    pub(super) fn with_env<R>(&self, f: impl FnOnce(&Env) -> R) -> R {
        let rules = self.client.clone();
        let mob = |id: u32| rules.as_deref().and_then(|r| r.monsters.as_ref()).and_then(|m| m.get(id)).map(|m| (m.name.clone(), m.level.max(0) as u32));
        let npc = |k: u32, id: u32| rules.as_deref().and_then(|r| r.npcs.as_ref()).and_then(|n| n.get(k, id)).map(|n| n.name.clone());
        let sys = self.sysmsg.clone();
        let map_name = |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        let names = &self.q3.names;
        let region = |map: u32, i: usize| {
            let mut c = names.borrow_mut();
            let e = c.entry(map).or_insert_with(|| rules.as_deref().and_then(|r| r.area_names(map).ok()).map(Arc::new));
            e.as_ref().and_then(|a| a.get(i)).filter(|s| !s.is_empty()).map(String::from)
        };
        f(&Env { mob: &mob, npc: &npc, map_name: &map_name, region: &region })
    }

    // -------------------------------------------------------------------------------------------------------------- find

    /// Runs the query when it, the scope or the maps changed since the last run.
    pub(super) fn run_find(&mut self) {
        let f = &self.q3.find;
        let view_key = match (f.this_map && f.inside_view, self.docs.get(self.active)) {
            (true, Some(d)) => [d.view.cx.round() as i32, d.view.cz.round() as i32, (d.view.zoom * 100.0) as i32, (self.vp.size.x as i32) ^ (self.vp.size.y as i32) << 12],
            _ => [0; 4],
        };
        // the active tab is part of the key: "this map" results (and their indices) belong to the tab they were found on
        let active = if f.this_map { self.docs.get(self.active).map(|d| d.uid) } else { None };
        let key = (f.query.clone(), f.this_map, f.inside_view, self.data_gen(), view_key, active);
        if f.key.as_ref() == Some(&key) {
            return;
        }
        let t = Instant::now();
        let q = Query::parse(&self.q3.find.query);
        let (hits, note) = self.find_hits(&q);
        let f = &mut self.q3.find;
        f.results = hits;
        f.note = note;
        f.ms = t.elapsed().as_secs_f32() * 1000.0;
        f.key = Some(key);
        if f.selected.is_some_and(|i| i >= f.results.len()) {
            f.selected = None;
        }
    }

    /// Every map the app knows, in map order: the open tabs with their unsaved edits (editable ones first), the folder's copies of
    /// the others, then read-only tabs of maps that are not otherwise there.
    pub(super) fn all_map_ins(&self) -> Vec<MapIn<'_>> {
        let folder_result = self.safety.folder_check.result.as_ref();
        let mut ins: Vec<MapIn> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for d in self.docs.iter().filter(|d| !d.read_only) {
            if let Some(id) = d.map_id {
                if seen.insert(id) {
                    ins.push(MapIn { id: Some(id), map: &d.map, findings: Some(&d.findings) });
                }
            }
        }
        if let Some(fo) = &self.folder {
            for (id, m) in &fo.maps {
                if seen.contains(id) {
                    continue;
                }
                if let Some(s) = &m.svmap {
                    seen.insert(*id);
                    ins.push(MapIn { id: Some(*id), map: s, findings: folder_result.and_then(|r| r.maps.get(id)).map(|m| m.findings.as_slice()) });
                }
            }
        }
        for d in &self.docs {
            if let Some(id) = d.map_id {
                if seen.insert(id) {
                    ins.push(MapIn { id: Some(id), map: &d.map, findings: Some(&d.findings) });
                }
            }
        }
        ins.sort_by_key(|m| m.id);
        ins
    }

    fn find_hits(&self, q: &Query) -> (Vec<Hit>, String) {
        let this_map = self.q3.find.this_map;
        let mut ins: Vec<MapIn> = Vec::new();
        if this_map {
            if let Some(d) = self.docs.get(self.active) {
                ins.push(MapIn { id: d.map_id, map: &d.map, findings: Some(&d.findings) });
            }
        } else {
            ins = self.all_map_ins();
        }
        let mut note = String::new();
        if q.has_code() {
            let unknown = ins.iter().filter(|m| m.findings.is_none()).count();
            if unknown > 0 {
                note = format!("code: the findings of {unknown} map{} are not known; open them or switch Problems to all maps", if unknown == 1 { "" } else { "s" });
            }
        }
        let mut hits = self.with_env(|env| search::search(q, &ins, env, 50_000));
        if this_map && self.q3.find.inside_view {
            if let Some(d) = self.docs.get(self.active) {
                let (hw, hh) = (self.vp.size.x / d.view.zoom * 0.5, self.vp.size.y / d.view.zoom * 0.5);
                let (x0, x1, z0, z1) = (d.view.cx - hw, d.view.cx + hw, d.view.cz - hh, d.view.cz + hh);
                hits.retain(|h| {
                    edit::get(&d.map, h.section, h.index).is_some_and(|r| {
                        let b = edit::bounds_xz(&r);
                        let c = [(b.0 + b.2) * 0.5, (b.1 + b.3) * 0.5];
                        c[0] >= x0 && c[0] <= x1 && c[1] >= z0 && c[1] <= z1
                    })
                });
            }
        }
        (hits, note)
    }

    /// Opens the map of a hit (a tab that is open, else from the folder) and selects and frames the record.
    pub(super) fn open_hit(&mut self, h: &Hit) {
        if let Some(id) = h.map {
            if self.docs.get(self.active).and_then(|d| d.map_id) != Some(id) {
                match self.docs.iter().position(|d| d.map_id == Some(id) && !d.read_only).or_else(|| self.docs.iter().position(|d| d.map_id == Some(id))) {
                    Some(i) => self.switch_to(i),
                    None => {
                        self.open_map_id(id);
                        if self.docs.get(self.active).and_then(|d| d.map_id) != Some(id) {
                            return;
                        }
                    }
                }
            }
        }
        self.do_goto(Resolved::Select { section: h.section, index: h.index });
    }

    /// The hits on the open map become the selection ("Select all on this map" / Select matching).
    pub(super) fn select_hits_here(&mut self) {
        let id = self.docs.get(self.active).and_then(|d| d.map_id);
        let this = self.q3.find.this_map;
        let sel: Vec<Sel> = self.q3.find.results.iter().filter(|h| this || h.map == id).map(|h| (h.section, h.index)).collect();
        let n = sel.len();
        if let Some(d) = self.docs.get_mut(self.active) {
            d.selection = sel.into_iter().filter(|(s, i)| *i < edit::count(&d.map, *s)).collect();
            d.sub = None;
        }
        self.toast(format!("selected {n} record{}", if n == 1 { "" } else { "s" }));
    }

    /// Ctrl+A: every record of the sections of the selection, else of every visible and unlocked section.
    pub(super) fn select_all(&mut self) {
        let layers = self.settings.layers.clone();
        let locks = self.settings.locks.clone();
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let mut secs: Vec<SectionId> = d.selection.iter().map(|(s, _)| *s).collect();
        secs.sort();
        secs.dedup();
        if secs.is_empty() {
            secs = edit::LIST_SECTIONS.iter().copied().filter(|s| view2d::visible(&layers, *s) && !view2d::locked(&locks, *s)).collect();
        }
        let mut sel = Vec::new();
        for s in edit::LIST_SECTIONS {
            if secs.contains(&s) {
                sel.extend((0..edit::count(&d.map, s)).map(|i| (s, i)));
            }
        }
        d.selection = sel;
        d.sub = None;
    }

    /// Edit > Select similar: every record of the same section and key as the selected ones.
    pub(super) fn select_similar(&mut self) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let sel = search::select_similar(&d.map, &d.selection);
        let n = sel.len();
        if n > 0 {
            d.selection = sel;
            d.sub = None;
        }
        self.toast(format!("{n} similar record{}", if n == 1 { "" } else { "s" }));
    }

    pub(super) fn show_find(&mut self, this_map: bool) {
        self.open_dock(DockTab::Find);
        self.q3.find.focus = true;
        if this_map {
            self.q3.find.this_map = true;
        }
    }

    /// The Find tab: query, scope, the results and what to do with them.
    pub(super) fn find_body(&mut self, ui: &mut egui::Ui) {
        self.run_find();
        let mut do_open: Option<usize> = None;
        let mut select_here = false;
        let mut open_replace = false;
        let mut open_bulk = false;
        let mut save_all = false;
        ui.horizontal_wrapped(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut self.q3.find.query).id(egui::Id::unique("find-query")).hint_text("thief   mob:2008   npc:1/159   portal:18   region:artlum   code:portal-far   lv:10-20   map:1-5   count:2-3").desired_width(380.0));
            edit.clone().on_hover_text(crate::help_text::tip("find-query"));
            if std::mem::take(&mut self.q3.find.focus) {
                edit.request_focus();
            }
            ui.selectable_value(&mut self.q3.find.this_map, false, "all maps").on_hover_text(crate::help_text::tip("find-scope"));
            ui.selectable_value(&mut self.q3.find.this_map, true, "this map").on_hover_text(crate::help_text::tip("find-scope"));
            if self.q3.find.this_map {
                ui.checkbox(&mut self.q3.find.inside_view, "inside view").on_hover_text(crate::help_text::tip("find-view"));
            }
            if ui.add_enabled(!self.q3.find.results.is_empty() && !self.docs.is_empty(), egui::Button::new("Select all on this map")).on_hover_text("Select the results that are on the open map").clicked() {
                select_here = true;
            }
            let q = Query::parse(&self.q3.find.query);
            let mob = q.mob();
            let has_areas = self.q3.find.results.iter().any(|h| h.section == SectionId::MobAreas);
            let r = ui.add_enabled(mob.is_some() && has_areas, egui::Button::new("Replace monster..."));
            let r = if mob.is_some() && has_areas { r.on_hover_text("Open Bulk edit with a replace rule for these results.") } else { r.on_disabled_hover_text("Type mob:<id> and find areas with that monster first.") };
            if r.clicked() {
                open_replace = true;
            }
            if ui.add_enabled(!self.q3.find.query.trim().is_empty(), egui::Button::new("Bulk edit...")).on_hover_text("Change the matching monster areas of many maps with one rule.").clicked() {
                open_bulk = true;
            }
        });
        let f = &self.q3.find;
        if let Some(s) = &f.summary {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(s).color(crate::theme::pal_ui(ui).ok));
                if ui.button("Save all").on_hover_text("File > Save all: every map with unsaved changes, each through the normal checks").clicked() {
                    save_all = true;
                }
            });
        }
        let n = f.results.len();
        let maps = search::per_map(&f.results).len();
        let status = if f.query.trim().is_empty() {
            "Type a query: free words match names, mob:<id>, npc:<type>/<id>, npctype:<n>, portal:<map>, region:<text>, code:<rule>, lv:<a>-<b>, map:<ids>, count:<a>-<b>. A plain number matches monster, NPC and portal destination.".to_string()
        } else {
            format!("{n} result{} on {maps} map{} ({:.1} ms){}", if n == 1 { "" } else { "s" }, if maps == 1 { "" } else { "s" }, f.ms, if f.note.is_empty() { String::new() } else { format!(" · {}", f.note) })
        };
        ui.label(RichText::new(status).weak().small());
        let active_id = self.docs.get(self.active).and_then(|d| d.map_id);
        let mut hover: Option<Sel> = None;
        let mut pick: Option<usize> = None;
        let rows = self.q3.find.results.len();
        let selected = self.q3.find.selected;
        let sys = self.sysmsg.clone();
        egui::ScrollArea::vertical().id_salt("find-results").auto_shrink([false, false]).show_rows(ui, 18.0, rows, |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for i in range {
                let h = &self.q3.find.results[i];
                let map = h.map.map_or("-".to_string(), |m| format!("{m:>3} {}", shaiya_clientdata::map_name(m, sys.as_deref())));
                let text = format!("{map:<22} {:<13} #{:<4} {}  ·  {}", h.section.label(), h.index, h.label, h.what);
                let resp = ui.selectable_label(selected == Some(i), RichText::new(text).monospace());
                if resp.hovered() && (h.map == active_id || self.q3.find.this_map) {
                    hover = Some((h.section, h.index));
                }
                if resp.clicked() {
                    pick = Some(i);
                }
                if resp.double_clicked() {
                    do_open = Some(i);
                }
            }
        });
        if hover.is_some() {
            self.hover_rec = hover;
        }
        if let Some(i) = pick {
            self.q3.find.selected = Some(i);
        }
        if let Some(i) = do_open {
            let h = self.q3.find.results[i].clone();
            self.q3.find.selected = Some(i);
            self.open_hit(&h);
        }
        if select_here {
            self.select_hits_here();
        }
        if open_replace {
            if let Some(from) = Query::parse(&self.q3.find.query).mob() {
                self.open_bulk_from_find(Some(from));
            }
        }
        if open_bulk {
            self.open_bulk_from_find(None);
        }
        if save_all {
            self.start_save_all();
        }
    }

    // -------------------------------------------------------------------------------------------------------------- bookmarks, prompt

    pub(super) fn bookmark_key(&self) -> Option<String> {
        let d = self.docs.get(self.active)?;
        Some(format!("{}#{}", crate::settings::path_key(&self.settings.svmap_folder.clone().unwrap_or_default()), d.map_id.unwrap_or(0)))
    }

    /// Ctrl+B and the context menu: asks for a name (the region at the position, else its x z) and keeps the view.
    pub(super) fn add_bookmark_at(&mut self, at: Option<[f32; 2]>) {
        let Some(d) = self.docs.get(self.active) else { return };
        let p = at.unwrap_or([d.view.cx, d.view.cz]);
        let assets = self.current_assets();
        let look = crate::names::Lookups { rules: self.client.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
        let name = match view2d::region_at(&d.map, p[0], p[1]) {
            Some(i) if look.region_name(i).is_some() => look.region_label(i, &d.map.regions[i]),
            _ => format!("{:.0} {:.0}", p[0], p[1]),
        };
        let zoom = d.view.zoom.max(0.5);
        self.q3.prompt = Some(TextPrompt { kind: PromptKind::AddBookmark { x: p[0], z: p[1], zoom }, text: name, first: true });
    }

    pub(super) fn bookmarks_menu(&mut self, ui: &mut egui::Ui) {
        let marks = self.bookmarks_here();
        let (mut go, mut rename, mut del): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
        ui.menu_button("Bookmarks", |ui| {
            if self.docs.is_empty() {
                ui.label("no map open");
            } else if marks.is_empty() {
                ui.label("none yet: Ctrl+B adds one for this map");
            }
            for (i, b) in marks.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.button(&b.name).on_hover_text(format!("x {:.0}  z {:.0}  zoom {:.2}", b.x, b.z, b.zoom)).clicked() {
                        go = Some(i);
                        ui.close();
                    }
                    if ui.small_button("rename").clicked() {
                        rename = Some(i);
                        ui.close();
                    }
                    if ui.small_button("x").on_hover_text("Delete this bookmark").clicked() {
                        del = Some(i);
                    }
                });
            }
        });
        if let Some(i) = go {
            if let Some(b) = marks.get(i) {
                self.do_goto(Resolved::Position { map: None, x: b.x, y: None, z: b.z, zoom: Some(b.zoom) });
            }
        }
        if let Some(i) = rename {
            if let Some(b) = marks.get(i) {
                self.q3.prompt = Some(TextPrompt::rename_bookmark(self.bookmark_key().unwrap_or_default(), b.name.clone()));
            }
        }
        if let (Some(i), Some(key)) = (del, self.bookmark_key()) {
            if let Some(list) = self.settings.bookmarks.get_mut(&key) {
                if i < list.len() {
                    list.remove(i);
                }
                if list.is_empty() {
                    self.settings.bookmarks.remove(&key);
                }
            }
        }
    }

    pub(super) fn prompt_window(&mut self, ctx: &egui::Context) {
        let Some(mut p) = self.q3.prompt.take() else { return };
        // a bookmark rename belongs to the map it was opened on: a tab switch closes it
        if let PromptKind::RenameBookmark { key, .. } = &p.kind {
            if self.bookmark_key().as_deref() != Some(key.as_str()) {
                return;
            }
        }
        let title = match p.kind {
            PromptKind::AddBookmark { .. } => "Add bookmark",
            PromptKind::RenameBookmark { .. } => "Rename bookmark",
            PromptKind::SaveTemplate { .. } => "Save as template",
        };
        let mut done: Option<bool> = None;
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label("Name");
            let r = ui.add(egui::TextEdit::singleline(&mut p.text).desired_width(260.0));
            if std::mem::take(&mut p.first) {
                r.request_focus();
            }
            ui.horizontal(|ui| {
                if ui.button("OK").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    done = Some(false);
                }
            });
        });
        match done {
            Some(true) => {
                if p.text.trim().is_empty() {
                    self.toast("a name is needed");
                    self.q3.prompt = Some(p);
                } else {
                    self.finish_prompt(p);
                }
            }
            Some(false) => {}
            None => self.q3.prompt = Some(p),
        }
    }

    pub(super) fn finish_prompt(&mut self, p: TextPrompt) {
        let name = p.text.trim().to_string();
        match p.kind {
            PromptKind::AddBookmark { x, z, zoom } => {
                if let Some(key) = self.bookmark_key() {
                    self.settings.bookmarks.entry(key).or_default().push(Bookmark { name: name.clone(), x, z, zoom });
                    self.toast(format!("bookmark \"{name}\" added"));
                }
            }
            PromptKind::RenameBookmark { key, old } => {
                if let Some(b) = self.settings.bookmarks.get_mut(&key).and_then(|l| l.iter_mut().find(|b| b.name == old)) {
                    b.name = name;
                }
            }
            PromptKind::SaveTemplate { uid, area } => {
                let Some(d) = self.docs.iter().find(|d| d.uid == uid) else { return };
                let Some(a) = d.map.mob_areas.get(area) else { return };
                let t = Template { name: name.clone(), size: [(a.max[0] - a.min[0]).abs(), (a.max[2] - a.min[2]).abs()], height: (a.max[1] - a.min[1]).abs(), entries: a.mobs.iter().map(|e| [e.mob, e.count]).collect() }.clamped();
                match self.settings.templates.iter_mut().find(|x| x.name == name) {
                    Some(x) => *x = t,
                    None => self.settings.templates.push(t),
                }
                self.q3.template = Some(name.clone());
                self.toast(format!("template \"{name}\" saved; the Box tool uses it now"));
            }
        }
    }

    // -------------------------------------------------------------------------------------------------------------- templates

    pub(super) fn active_template(&self) -> Option<&Template> {
        let n = self.q3.template.as_deref()?;
        self.settings.templates.iter().find(|t| t.name == n)
    }

    /// The Box tool's template list (shown when the tool draws monster areas).
    pub(super) fn template_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label("template").on_hover_text(crate::help_text::tip("template"));
        let labels: Vec<(String, String)> = {
            let look = crate::names::Lookups { rules: self.client.as_deref(), assets: None, map_id: None, names: None };
            self.settings
                .templates
                .iter()
                .map(|t| {
                    let lv = look.mean_level(&t.area_at(0.0, 0.0, 0.0)).map_or(String::new(), |l| format!(", lv {}", l.round() as i32));
                    (t.name.clone(), format!("{} ({:.0} x {:.0}, {} monster{}{lv})", t.name, t.size[0], t.size[1], t.entries.len(), if t.entries.len() == 1 { "" } else { "s" }))
                })
                .collect()
        };
        let cur = self.active_template().map(|t| t.name.clone());
        egui::ComboBox::from_id_salt("template").width(150.0).selected_text(cur.clone().unwrap_or_else(|| "none".to_string())).show_ui(ui, |ui| {
            if ui.selectable_label(cur.is_none(), "none").clicked() {
                self.q3.template = None;
            }
            for (name, label) in &labels {
                if ui.selectable_label(cur.as_deref() == Some(name.as_str()), label).clicked() {
                    self.q3.template = Some(name.clone());
                }
            }
            if labels.is_empty() {
                ui.label(RichText::new("none saved: select a monster area, Save as template...").weak());
            }
        }).response.on_hover_text(crate::help_text::tip("template"));
        if let Some(name) = cur {
            if ui.small_button("x").on_hover_text("Delete this template").clicked() {
                self.settings.templates.retain(|t| t.name != name);
                self.q3.template = None;
            }
        }
    }

    pub(super) fn save_template_prompt(&mut self, area: usize) {
        let Some(d) = self.docs.get(self.active) else { return };
        if d.map.mob_areas.get(area).is_none() {
            return;
        }
        let uid = d.uid;
        let n = self.settings.templates.len() + 1;
        self.q3.prompt = Some(TextPrompt { kind: PromptKind::SaveTemplate { uid, area }, text: format!("template {n}"), first: true });
    }

    // -------------------------------------------------------------------------------------------------------------- level filter

    pub(super) fn level_filter_button(&mut self, ui: &mut egui::Ui) {
        let label = match self.settings.level_filter {
            Some([a, b]) => format!("lv {a}-{b}"),
            None => "lv".to_string(),
        };
        ui.menu_button(label, |ui| {
            let mut on = self.settings.level_filter.is_some();
            let mut r = self.settings.level_filter.unwrap_or([1, 80]);
            let mut changed = ui.checkbox(&mut on, "only monster areas in this level range").changed();
            ui.horizontal(|ui| {
                ui.label("from");
                changed |= ui.add(egui::DragValue::new(&mut r[0]).range(1..=255)).changed();
                ui.label("to");
                changed |= ui.add(egui::DragValue::new(&mut r[1]).range(1..=255)).changed();
            });
            if ui.button("off").clicked() {
                on = false;
                changed = true;
            }
            if changed {
                if r[0] > r[1] {
                    r.swap(0, 1);
                }
                self.settings.level_filter = on.then_some(r);
            }
            ui.label(RichText::new("Areas outside the range are dimmed and cannot be picked. Needs the client's monster levels.").weak().small());
        }).response.on_hover_text(crate::help_text::tip("level-filter"));
    }

    // -------------------------------------------------------------------------------------------------------------- portal links

    /// The link table of the open folder (open tabs with their live maps); cached until something changes.
    pub(super) fn links_now(&mut self) -> Option<Arc<search::Links>> {
        self.folder.as_ref()?;
        let gen = self.data_gen();
        if let Some((g, l)) = &self.q3.links {
            if *g == gen {
                return Some(l.clone());
            }
        }
        let l = {
            let mut maps: Vec<(u32, &Svmap)> = Vec::new();
            let mut seen = HashSet::new();
            for d in self.docs.iter().filter(|d| !d.read_only) {
                if let Some(id) = d.map_id {
                    if seen.insert(id) {
                        maps.push((id, &d.map));
                    }
                }
            }
            if let Some(f) = &self.folder {
                for (id, m) in &f.maps {
                    if let (false, Some(s)) = (seen.contains(id), &m.svmap) {
                        maps.push((*id, s));
                    }
                }
            }
            maps.sort_by_key(|(id, _)| *id);
            Arc::new(search::portal_links(&maps))
        };
        self.q3.links = Some((gen, l.clone()));
        Some(l)
    }

    /// The portals of other maps that lead to the open map (for the map card).
    pub(super) fn incoming_portals(&mut self) -> Vec<Incoming> {
        let Some(id) = self.docs.get(self.active).and_then(|d| d.map_id) else { return Vec::new() };
        let Some(links) = self.links_now() else { return Vec::new() };
        search::leading_here(&links, id).into_iter().map(|l| Incoming { from: l.from, index: l.index, label: format!("{} {}  #{}  (arrives at {:.0}, {:.0})", l.from, self.map_name(l.from), l.index, l.to_pos[0], l.to_pos[2]) }).collect()
    }

    pub(super) fn links_window(&mut self, ctx: &egui::Context) {
        if !self.q3.links_open {
            return;
        }
        let links = self.links_now();
        let mut open = true;
        let mut go: Option<(u32, usize)> = None;
        let files: HashSet<u32> = self.folder.as_ref().map(|f| f.maps.keys().copied().collect()).unwrap_or_default();
        let has_file = |id: u32| files.contains(&id);
        let sys = self.sysmsg.clone();
        let map_name = |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        let n_maps = files.len();
        let mut only = self.q3.links_only_problems;
        egui::Window::new("Portal links").open(&mut open).default_size([760.0, 520.0]).show(ctx, |ui| {
            let Some(l) = &links else {
                ui.label("Open the server map folder first (File > Open folder...): the table covers every map of the folder.");
                return;
            };
            ui.label(format!("{} portals on {} maps. {} lead to a map that has no portal back to the source.", l.links.len(), n_maps, l.one_way));
            let list = |ids: &[u32]| if ids.is_empty() { "none".to_string() } else { ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ") };
            ui.label(RichText::new(format!("Maps no portal leads to: {}", list(&l.no_in))).small());
            ui.label(RichText::new(format!("Maps without a portal out: {}", list(&l.no_out))).small());
            ui.checkbox(&mut only, "only links with no way back, a missing map, or a destination outside or on a blocked cell");
            ui.separator();
            ui.label(RichText::new(format!("{:>3} {:<14} {:>3} {:<34} {:<10} {:<8} {:<5} flags", "map", "name", "#", "to", "country", "levels", "back")).monospace().weak());
            let rows: Vec<&search::Link> = l.links.iter().filter(|x| !only || !x.back || x.dest_missing || x.dest_outside || x.dest_blocked).collect();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, rows.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for i in range {
                    let x = rows[i];
                    let name = map_name(x.from);
                    let dest = search::dest_label(x.to, has_file(x.to), &map_name(x.to));
                    let mut flags = Vec::new();
                    if x.dest_missing {
                        flags.push("map missing");
                    }
                    if x.dest_outside {
                        flags.push("outside");
                    }
                    if x.dest_blocked {
                        flags.push("blocked");
                    }
                    let lv = if x.level_min == 0 && x.level_max == 0 { "-".to_string() } else { format!("{}-{}", x.level_min, x.level_max) };
                    let back = if x.dest_missing || x.to == 200 && !has_file(200) { "n/a" } else if x.back { "yes" } else { "no" };
                    let text = format!("{:>3} {name:<14.14} {:>3} {dest:<34.34} {:<10} {lv:<8} {back:<5} {}", x.from, x.index, crate::names::portal_country(x.country), flags.join(", "));
                    let resp = ui.selectable_label(false, RichText::new(text).monospace());
                    if resp.double_clicked() {
                        go = Some((x.from, x.index));
                    }
                    resp.on_hover_text("Double-click: open the map and select the portal");
                }
            });
        });
        self.q3.links_only_problems = only;
        if !open {
            self.q3.links_open = false;
        }
        if let Some((map, index)) = go {
            self.open_hit(&Hit { map: Some(map), section: SectionId::Portals, index, label: String::new(), what: String::new() });
        }
    }

    /// Go to destination: opens the destination map and shows the arrival point with the crosshair.
    pub(super) fn go_to_dest(&mut self, portal: usize) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(p) = d.map.portals.get(portal).copied() else { return };
        let has_tab = self.docs.iter().any(|x| x.map_id == Some(p.to_map));
        let has_file = self.folder.as_ref().is_some_and(|f| f.maps.contains_key(&p.to_map));
        if p.to_map == 200 && !has_tab && !has_file {
            self.toast("map 200 is the guild ranking: it has no file");
            return;
        }
        if !has_tab && !has_file {
            self.toast(format!("map {} is not in the open folder", p.to_map));
            return;
        }
        if let Some(i) = self.docs.iter().position(|x| x.map_id == Some(p.to_map) && !x.read_only).or_else(|| self.docs.iter().position(|x| x.map_id == Some(p.to_map))) {
            self.switch_to(i);
        }
        self.do_goto(Resolved::Position { map: Some(p.to_map), x: p.to_pos[0], y: Some(p.to_pos[1]), z: p.to_pos[2], zoom: None });
    }

    /// Drop destination to ground: sets the portal's `to_pos.y` from the destination map's heights (the client's assets of that
    /// map; they load in the background when the map has not been looked at yet).
    pub(super) fn drop_dest_to_ground(&mut self, portal: usize, ctx: &egui::Context) {
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(p) = d.map.portals.get(portal).copied() else { return };
        if d.read_only {
            self.toast("read-only tab: use Copy, then paste into an editable tab");
            return;
        }
        if self.client.is_none() {
            self.toast("the ground comes from the client data (File > Open client folder...)");
            return;
        }
        let dest = p.to_map;
        let size = self.docs.iter().find(|x| x.map_id == Some(dest)).map(|x| x.map.size).or_else(|| self.folder.as_ref().and_then(|f| f.maps.get(&dest)).and_then(|m| m.svmap.as_ref().map(|s| s.size)));
        let Some(size) = size else {
            self.toast(format!("map {dest} is not in the open folder, so its ground is not known"));
            return;
        };
        let uid = d.uid;
        self.start_assets_load(dest, size, ctx);
        self.q3.drops.push(DropJob { uid, portal, dest });
        self.process_drops();
    }

    /// Applies the Drop destination to ground jobs whose destination map has loaded.
    pub(super) fn process_drops(&mut self) {
        if self.q3.drops.is_empty() {
            return;
        }
        let jobs = std::mem::take(&mut self.q3.drops);
        for j in jobs {
            let ready = match self.slots.get(&j.dest) {
                Some(Slot::Ready(r)) => Some(r.assets.clone()),
                Some(Slot::Loading(_)) => None,
                None => {
                    // the slot was cleared (the client was reloaded): give up
                    continue;
                }
            };
            let Some(assets) = ready else {
                self.q3.drops.push(j);
                continue;
            };
            let Some(d) = self.docs.iter_mut().find(|d| d.uid == j.uid) else { continue };
            let Some(Rec::Portal(before)) = edit::get(&d.map, SectionId::Portals, j.portal) else { continue };
            if before.to_map != j.dest {
                // the portal was deleted, moved in the list or pointed elsewhere while the map loaded
                self.toast("drop destination to ground: the portal changed while the destination loaded; nothing set");
                continue;
            }
            match assets.ground.height(before.to_pos[0], before.to_pos[2]) {
                Some(h) => {
                    let mut after = before;
                    after.to_pos[1] = h;
                    if after != before {
                        d.commit("drop portal destination to ground", vec![Op::replace(j.portal, Rec::Portal(before), Rec::Portal(after))]);
                    }
                    self.toast(format!("destination y set to {h:.1}"));
                }
                None => self.toast("the destination map has no ground at that position"),
            }
        }
    }

    // -------------------------------------------------------------------------------------------------------------- clipboard

    /// Copy: the records stay in the app's clipboard and go to the OS clipboard as JSON (other editor windows, chat and git can take them).
    pub(super) fn copy_selection_to(&mut self, ctx: &egui::Context) {
        let Some(d) = self.docs.get(self.active) else { return };
        let mut sel = d.selection.clone();
        sel.sort();
        let recs: Vec<Rec> = sel.iter().filter_map(|(s, i)| edit::get(&d.map, *s, *i)).collect();
        if recs.is_empty() {
            return;
        }
        let n = recs.len();
        match edit::clipboard_json(d.map_id, &recs) {
            Some(json) => ctx.copy_text(json),
            None => {
                // a NaN cannot travel as JSON; the in-app clipboard still works
                self.toast(format!("copied {n} record{} (not to the system clipboard: a number is NaN)", if n == 1 { "" } else { "s" }));
                self.clipboard = recs;
                return;
            }
        }
        self.clipboard = recs;
        self.toast(format!("copied {n} record{}", if n == 1 { "" } else { "s" }));
    }

    /// A paste event (Ctrl+V): the records of svmap clipboard text go in at the cursor; other text is ignored.
    pub(super) fn paste_text(&mut self, text: &str) {
        if self.docs.is_empty() || self.r5.showing != super::Showing::Map {
            return;
        }
        match edit::parse_clipboard(text) {
            Some(recs) => {
                let at = match self.vp.hover {
                    Some(p) => Place::Cursor(p),
                    None => Place::Offset,
                };
                self.request(Pending::Paste(recs, at));
            }
            None => self.toast("the clipboard text is not svmap records (copy records in the editor first)"),
        }
    }

    // -------------------------------------------------------------------------------------------------------------- context menu

    /// Runs what a context menu entry chose.
    pub(super) fn do_ctx_act(&mut self, a: CtxAct, ctx: &egui::Context) {
        match a {
            CtxAct::Frame => self.frame_selection(),
            CtxAct::Copy => self.copy_selection_to(ctx),
            CtxAct::Duplicate => self.duplicate(),
            CtxAct::Delete => self.delete(),
            CtxAct::DropToGround => self.drop_selection_to_ground(),
            CtxAct::SelectSimilar => self.select_similar(),
            CtxAct::Fix(f, k) => self.apply_fix(&f, k),
            CtxAct::CopyPosition(p) => {
                let t = position_text(p);
                ctx.copy_text(t.clone());
                self.toast(format!("copied {t}"));
            }
            CtxAct::CopyGm(p) => {
                if let Some(id) = self.docs.get(self.active).and_then(|d| d.map_id) {
                    let t = gm_move_text(p, id);
                    ctx.copy_text(t.clone());
                    self.toast(format!("copied {t}"));
                } else {
                    self.toast("the map has no number, so there is no GM move command");
                }
            }
            CtxAct::GoToDest(i) => self.go_to_dest(i),
            CtxAct::SaveTemplate(i) => self.save_template_prompt(i),
            CtxAct::PasteHere(p) => {
                if !self.clipboard.is_empty() {
                    let recs = self.clipboard.clone();
                    self.request(Pending::Paste(recs, Place::Cursor(p)));
                }
            }
            CtxAct::AddArea(p) => self.add_area_at(p),
            CtxAct::AddNpc(p) => self.add_npc_at(p),
            CtxAct::AddBookmark(p) => self.add_bookmark_at(Some(p)),
            CtxAct::GoTo => self.run(Cmd::GoTo, ctx),
        }
    }

    /// "Add monster area here": the Box tool's template stamped at the snapped position, else 16 x 16 with no monsters.
    fn add_area_at(&mut self, p: [f32; 2]) {
        let g = self.ground_fn();
        let snap = self.settings.snap;
        let (cx, cz) = (view2d::snap_to(p[0], snap), view2d::snap_to(p[1], snap));
        let t = self.active_template().cloned();
        let size = t.as_ref().map_or([16.0, 16.0], |t| t.size);
        let (x0, z0) = (cx - size[0] * 0.5, cz - size[1] * 0.5);
        let y = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)].iter().map(|(fx, fz)| g(x0 + size[0] * fx, z0 + size[1] * fz)).fold(f32::MAX, f32::min);
        let area = match t {
            Some(t) => t.area_at(x0, y, z0),
            None => shaiya_svmap::MobArea { min: [x0, y, z0], max: [x0 + size[0], y + 2.0, z0 + size[1]], mobs: Vec::new() },
        };
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let idx = d.map.mob_areas.len();
        d.commit("add monster area", vec![Op::insert(idx, Rec::Mob(area))]);
        if d.map.mob_areas.len() > idx {
            d.select_only(SectionId::MobAreas, idx);
        }
    }

    fn add_npc_at(&mut self, p: [f32; 2]) {
        let g = self.ground_fn();
        let snap = self.settings.snap;
        let (x, z) = (view2d::snap_to(p[0], snap), view2d::snap_to(p[1], snap));
        let n = shaiya_svmap::NpcEntry { kind: self.npc_template.0, type_id: self.npc_template.1, points: vec![shaiya_svmap::NpcPoint { pos: [x, g(x, z), z], dir: 0.0 }] };
        let Some(d) = self.docs.get_mut(self.active) else { return };
        let idx = d.map.npcs.len();
        d.commit("add NPC", vec![Op::insert(idx, Rec::Npc(n))]);
        if d.map.npcs.len() > idx {
            d.select_only(SectionId::Npcs, idx);
        }
    }

    // -------------------------------------------------------------------------------------------------------------- export

    /// Writes an export file after [`export::check_target`]: never a map or client file, never a map that is open or in the folder.
    pub(crate) fn export_write(&self, path: &Path, data: &[u8]) -> Result<(), String> {
        let mut protected: Vec<&Path> = self.docs.iter().filter_map(|d| d.path.as_deref()).collect();
        if let Some(f) = &self.folder {
            protected.extend(f.maps.values().map(|m| m.path.as_path()));
        }
        // every client folder the editor reads: the opened one, the settings one and the main one from SHAIYA_CLIENT
        let mut clients = self.share_guard_also();
        clients.extend(self.client_path.clone());
        export::check_target(path, &protected, &clients)?;
        std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn export_id(&self) -> String {
        self.docs.get(self.active).and_then(|d| d.map_id).map_or("map".to_string(), |i| i.to_string())
    }

    /// Writes one section of the open map as CSV.
    pub(crate) fn export_section_csv(&mut self, kind: export::Kind, path: &Path) -> Result<(), String> {
        let d = self.docs.get(self.active).ok_or("no map open")?;
        let text = self.with_env(|env| export::section_csv(kind, &d.map, d.map_id, env));
        self.export_write(path, text.as_bytes())
    }

    /// Writes every section of the open map as its own CSV file into `dir` (`<id>-<section>.csv`).
    pub(crate) fn export_all_csv(&mut self, dir: &Path) -> Result<usize, String> {
        let id = self.export_id();
        for k in export::Kind::ALL {
            self.export_section_csv(k, &dir.join(format!("{id}-{}.csv", k.file_part())))?;
        }
        Ok(export::Kind::ALL.len())
    }

    /// Every map's monster areas in one CSV (the `map` column first); open tabs count with their unsaved edits.
    pub(crate) fn export_folder_areas_csv(&mut self, path: &Path) -> Result<usize, String> {
        let mut maps: Vec<(u32, &Svmap)> = Vec::new();
        let mut seen = HashSet::new();
        for d in self.docs.iter().filter(|d| !d.read_only) {
            if let Some(id) = d.map_id {
                if seen.insert(id) {
                    maps.push((id, &d.map));
                }
            }
        }
        if let Some(f) = &self.folder {
            if f.rx.is_some() {
                return Err("the folder is still being read".to_string());
            }
            for (id, m) in &f.maps {
                if let (false, Some(s)) = (seen.contains(id), &m.svmap) {
                    maps.push((*id, s));
                }
            }
        }
        maps.sort_by_key(|(id, _)| *id);
        let n = maps.len();
        let text = self.with_env(|env| export::folder_areas_csv(&maps, env));
        self.export_write(path, text.as_bytes())?;
        Ok(n)
    }

    pub(crate) fn export_json_to(&mut self, path: &Path) -> Result<(), String> {
        let d = self.docs.get(self.active).ok_or("no map open")?;
        let text = export::map_json(&d.map)?;
        self.export_write(path, text.as_bytes())
    }

    /// The export commands: ask for a file or folder, write it, say so.
    pub(super) fn run_export(&mut self, cmd: Cmd) {
        let id = self.export_id();
        let dir = self.settings.svmap_folder.clone().map(PathBuf::from).filter(|p| p.is_dir());
        let save = |name: &str, ext: &str, label: &str| {
            let mut dlg = rfd::FileDialog::new().add_filter(label, &[ext]).set_file_name(name);
            if let Some(d) = &dir {
                dlg = dlg.set_directory(d);
            }
            dlg.save_file()
        };
        let result: Result<String, String> = match cmd {
            Cmd::ExportAreasCsv | Cmd::ExportNpcsCsv | Cmd::ExportPortalsCsv | Cmd::ExportBindsCsv | Cmd::ExportRegionsCsv => {
                let kind = match cmd {
                    Cmd::ExportAreasCsv => export::Kind::Areas,
                    Cmd::ExportNpcsCsv => export::Kind::Npcs,
                    Cmd::ExportPortalsCsv => export::Kind::Portals,
                    Cmd::ExportBindsCsv => export::Kind::Binds,
                    _ => export::Kind::Regions,
                };
                let Some(p) = save(&format!("{id}-{}.csv", kind.file_part()), "csv", "CSV") else { return };
                self.export_section_csv(kind, &p).map(|_| format!("exported {}", p.display()))
            }
            Cmd::ExportAllCsv => {
                let Some(p) = rfd::FileDialog::new().pick_folder() else { return };
                self.export_all_csv(&p).map(|n| format!("exported {n} CSV files into {}", p.display()))
            }
            Cmd::ExportFolderAreasCsv => {
                let Some(p) = save("folder-monster-areas.csv", "csv", "CSV") else { return };
                self.export_folder_areas_csv(&p).map(|n| format!("exported the monster areas of {n} maps to {}", p.display()))
            }
            Cmd::ExportJson => {
                let Some(p) = save(&format!("{id}.json"), "json", "JSON") else { return };
                self.export_json_to(&p).map(|_| format!("exported {}", p.display()))
            }
            Cmd::ExportPng => {
                let Some(p) = save(&format!("{id}.png"), "png", "PNG") else { return };
                self.q3.png = Some(PngJob { path: p, stage: PngStage::Clean(0) });
                return;
            }
            _ => return,
        };
        match result {
            Ok(t) => {
                log::info(t.clone());
                self.toast(t);
            }
            Err(e) => {
                log::error(format!("export failed: {e}"));
                self.dialog = Some(super::Dialog::Error(format!("Export failed: {e}")));
            }
        }
    }

    /// Whether a picture export is running (the viewport draws without handles, hover outline, overview and legend).
    pub(super) fn png_clean(&self) -> bool {
        self.q3.png.is_some()
    }

    /// View as PNG: a few clean frames, then a screenshot of the window cropped to the viewport.
    pub(super) fn png_tick(&mut self, ctx: &egui::Context) {
        let Some(job) = self.q3.png.take() else { return };
        ctx.request_repaint();
        let stage = match job.stage {
            PngStage::Clean(n) if n < 3 => PngStage::Clean(n + 1),
            PngStage::Clean(_) => {
                let slot = Arc::new(Mutex::new(None));
                let done = Arc::new(AtomicBool::new(false));
                let (s2, d2) = (slot.clone(), done.clone());
                ctx.request_screenshot(move |img| {
                    *s2.lock().unwrap_or_else(|p| p.into_inner()) = Some(img);
                    d2.store(true, Ordering::SeqCst);
                });
                PngStage::Wait(slot, done)
            }
            PngStage::Wait(slot, done) => {
                if !done.load(Ordering::SeqCst) {
                    PngStage::Wait(slot, done)
                } else {
                    let img = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
                    let r = self.vp.rect;
                    let screen = ctx.input(|i| i.content_rect());
                    let msg = match img {
                        Some(img) => {
                            let (w, h) = (img.size[0], img.size[1]);
                            let scale = w as f32 / screen.width().max(1.0);
                            let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
                            for px in &img.pixels {
                                rgba.extend_from_slice(&px.to_array());
                            }
                            match export::crop_rgba(&rgba, w, h, [r.left(), r.top(), r.right(), r.bottom()], scale) {
                                Some((cw, ch, crop)) => match self.export_write(&job.path, &crate::png_encode(cw as u32, ch as u32, &crop)) {
                                    Ok(()) => format!("exported {} ({cw} x {ch})", job.path.display()),
                                    Err(e) => format!("could not write {e}"),
                                },
                                None => "the view picture was empty".to_string(),
                            }
                        }
                        None => "no picture came back from the window".to_string(),
                    };
                    log::info(msg.clone());
                    self.toast(msg);
                    return;
                }
            }
        };
        self.q3.png = Some(PngJob { path: job.path, stage });
    }
}
