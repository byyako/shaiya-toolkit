//! Spawn work (spawn insight): the heatmap layer and the Spawns tab of the Dock.
//! Declared from `app.rs` with `#[path]`, so it shares the window's state.
//!
//! Nothing here edits a map except selecting records; the one thing it writes is the settings (heatmap options, the tolerance and
//! the per-map level ranges).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Align2, FontId, RichText, Sense, TextureHandle, TextureOptions, Vec2};
use shaiya_svmap::SectionId;

use super::App;
use crate::heat::{self, HeatPair, Mode};
use crate::icons::{self, Icon};
use crate::layout::DockTab;
use crate::names::Lookups;
use crate::search::Hit;
use crate::spawns::{self, Group, Report, SpawnFinding, SpawnRow, Totals};
use crate::theme;
use crate::view2d::{self, HeatDraw};

/// A recomputed heatmap waits this long after the last edit.
const HEAT_DEBOUNCE: Duration = Duration::from_millis(200);

/// (tab, edits, mode, tile, level filter, monster areas shown, client data (its address: a reload makes a new one), scale, Scatter preview)
type HeatKey = (u64, u64, Mode, u32, Option<(u32, u32)>, bool, usize, u32, u64);

struct HeatCache {
    key: HeatKey,
    pair: HeatPair,
    tex: TextureHandle,
    scale_max: f32,
    /// The picture's pixels (tiles per side, RGBA) for the 3D view, and a number that changes with them.
    rgba: Vec<u8>,
    n: u32,
    rev: u64,
}

/// What the Spawns tab knows about every map (recomputed when a map, the client, the tolerance or a range changes).
pub struct SpawnData {
    key: u64,
    pub report: Arc<Report>,
    pub totals: Totals,
    /// Monsters no area spawns: None without client monsters or while the folder still loads.
    pub never: Option<Arc<Vec<u32>>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FolderView {
    #[default]
    Maps,
    Findings,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Level,
    Areas,
    Count,
    Share,
}

/// The Spawns tab's own state.
pub struct SpawnsUi {
    pub all_maps: bool,
    pub view: FolderView,
    pub sort: (Col, bool),
    pub collapsed: HashSet<Group>,
    /// The map and the two numbers being typed for its range (Edit is open). Another map on screen closes it.
    pub band_edit: Option<(u32, u32, u32)>,
    pub never_filter: String,
    pub never_lo: u32,
    pub never_hi: u32,
}

impl Default for SpawnsUi {
    fn default() -> Self {
        SpawnsUi { all_maps: false, view: FolderView::Maps, sort: (Col::Count, true), collapsed: HashSet::new(), band_edit: None, never_filter: String::new(), never_lo: 0, never_hi: 0 }
    }
}

#[derive(Default)]
pub struct R2 {
    heat: Option<HeatCache>,
    heat_since: Option<Instant>,
    heat_rev: u64,
    pub data: Option<SpawnData>,
    pub ui: SpawnsUi,
}

impl R2 {
    /// Tests: the settling time of the heat picture is over now, so the next frame redraws it (no waiting for the clock).
    #[cfg(test)]
    pub fn debounce_due(&mut self) {
        self.heat_since = Some(Instant::now() - HEAT_DEBOUNCE - Duration::from_millis(1));
    }

    /// The heatmap picture of the last `heat_frame` for the 3D view (the caller checks that the heatmap is on).
    pub fn heat_params(&self, tile: u32, opacity: f32) -> Option<crate::gpu3d::HeatParams<'_>> {
        let c = self.heat.as_ref()?;
        Some(crate::gpu3d::HeatParams { rgba: &c.rgba, n: c.n, tile: tile as f32, opacity, rev: c.rev })
    }
}

#[cfg(test)]
impl R2 {
    pub fn heat_is_none(&self) -> bool {
        self.heat.is_none()
    }

    /// The sum of the density tiles of the picture on screen.
    pub fn heat_sum(&self) -> Option<f32> {
        self.heat.as_ref().map(|c| c.pair.density.sum())
    }
}

/// What a click in the Spawns tab asked for (done after the lists are drawn).
enum SAct {
    SelectMob(u32),
    FindMob(u32),
    Sort(Col),
    Open(usize),
    SelectGroup(Group),
    ToggleGroup(Group),
    OpenMap(u32),
    CopyIds(Vec<u32>),
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn range_text(b: Option<(u32, u32)>) -> String {
    b.map_or("-".to_string(), |(a, b)| format!("{a}\u{2013}{b}"))
}

impl App {
    // -------------------------------------------------------------------------------------------------------------- heatmap

    /// Whether the client gives monster levels (level mode needs them).
    pub(super) fn have_levels(&self) -> bool {
        self.client.as_deref().is_some_and(|r| r.monsters.is_some())
    }

    /// Tells one loaded client data apart from the next (Reload client data makes a new one): 0 without it.
    pub(super) fn client_addr(&self) -> usize {
        self.client.as_ref().map_or(0, |c| Arc::as_ptr(c) as usize)
    }

    /// The heatmap mode the viewport shows: level falls back to density when the client data is gone.
    pub(super) fn heat_mode(&self) -> Mode {
        match self.settings.heat.mode {
            Mode::Level if !self.have_levels() => Mode::Density,
            m => m,
        }
    }

    pub(super) fn set_heat_mode(&mut self, mode: Mode) {
        self.settings.heat.mode = mode;
        self.toast(format!("Heatmap: {}", mode.label().to_lowercase()));
    }

    /// H: off, density, level, off (level is skipped without the client data).
    pub(super) fn cycle_heat(&mut self) {
        let mut next = self.settings.heat.mode.next();
        if next == Mode::Level && !self.have_levels() {
            next = next.next();
        }
        self.set_heat_mode(next);
    }

    /// The heatmap picture and how to draw it for the active map; recomputed when the map's records, the mode, the tile side, the
    /// filters or the scale change (an edit waits 200 ms so a drag does not recompute every frame).
    pub(super) fn heat_frame(&mut self, ctx: &egui::Context) -> Option<(TextureHandle, HeatDraw)> {
        let h = self.settings.heat;
        let mode = self.heat_mode();
        let d = self.docs.get(self.active)?;
        if mode == Mode::Off {
            self.r2.heat = None;
            self.r2.heat_since = None;
            return None;
        }
        let levels = self.have_levels();
        let filter = self.settings.level_filter.map(|[a, b]| (a, b));
        let key: HeatKey = (d.uid, d.edit_gen.wrapping_add(d.base_gen.wrapping_mul(1_000_003)), mode, h.tile, filter, self.settings.layers.mob_areas, self.client_addr(), h.max.map_or(0, f32::to_bits), self.r3.scatter.gen_for_heat());
        let draw_of = |c: &HeatCache| (c.tex.clone(), HeatDraw { mode, opacity: h.opacity, max: c.scale_max, tile: h.tile });
        if let Some(c) = &self.r2.heat {
            if c.key == key {
                self.r2.heat_since = None;
                return Some(draw_of(c));
            }
            // only the map's records changed: show the old picture until the edits have settled
            let same_but_gen = c.key.0 == key.0 && c.key.2 == key.2 && c.key.3 == key.3 && c.key.4 == key.4 && c.key.5 == key.5 && c.key.6 == key.6 && c.key.7 == key.7 && c.key.8 == key.8;
            if same_but_gen {
                let since = *self.r2.heat_since.get_or_insert_with(Instant::now);
                if since.elapsed() < HEAT_DEBOUNCE {
                    ctx.request_repaint_after(HEAT_DEBOUNCE - since.elapsed() + Duration::from_millis(10));
                    return Some(draw_of(c));
                }
            }
        }
        self.r2.heat_since = None;
        let rules = self.client.clone();
        let level = |id: u32| rules.as_deref().and_then(|r| r.monsters.as_ref()).and_then(|m| m.get(id)).map(|m| m.level as f32).filter(|l| *l > 0.0);
        let look = Lookups { rules: rules.as_deref(), assets: None, map_id: d.map_id, names: d.part().map(|p| p.names()) };
        let filter_f = filter.map(|(a, b)| (a as f32, b as f32));
        let on = self.settings.layers.mob_areas;
        let include = |i: usize| on && d.map.mob_areas.get(i).is_some_and(|a| !view2d::outside_range(filter_f, look.mean_level(a)));
        let tile = h.tile as f32;
        let extra = self.r3.scatter.preview_areas();
        let density = heat::grid_with(&d.map, extra, &level, tile, Mode::Density, &include);
        let level_grid = levels.then(|| heat::grid_with(&d.map, extra, &level, tile, Mode::Level, &include));
        let scale_max = match h.max {
            Some(m) => m,
            None => {
                let a = density.auto_max();
                if a > 0.0 { a } else { 1.0 }
            }
        };
        let shown = if mode == Mode::Level { level_grid.as_ref().unwrap_or(&density) } else { &density };
        let rgba = shown.rgba(mode, scale_max);
        let shown_n = shown.n;
        let img = egui::ColorImage::from_rgba_unmultiplied([shown.n, shown.n], &rgba);
        let opts = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Nearest, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: None };
        let tex = ctx.load_texture("heat", img, opts);
        self.r2.heat_rev += 1;
        let cache = HeatCache { key, pair: HeatPair { density, level: level_grid }, tex, scale_max, n: shown_n as u32, rgba, rev: self.r2.heat_rev };
        let out = draw_of(&cache);
        self.r2.heat = Some(cache);
        Some(out)
    }

    /// The status bar text of the heatmap tile under the cursor ("3.2 per tile · lv 14").
    pub(super) fn heat_status(&self, p: [f32; 2]) -> Option<String> {
        if self.heat_mode() == Mode::Off {
            return None;
        }
        let uid = self.docs.get(self.active)?.uid;
        let c = self.r2.heat.as_ref().filter(|c| c.key.0 == uid)?;
        heat::status_text(&c.pair, p[0], p[1])
    }

    /// The heatmap rows of the layers popover and View > Layers: mode, tile side, opacity, scale, rulers.
    pub(super) fn heat_controls(&mut self, ui: &mut egui::Ui) {
        let levels = self.have_levels();
        ui.horizontal(|ui| {
            ui.label("heatmap");
            for m in [Mode::Off, Mode::Density, Mode::Level] {
                let r = ui.add_enabled(m != Mode::Level || levels, egui::Button::new(m.label()).selected(self.settings.heat.mode == m));
                let r = if m == Mode::Level && !levels { r.on_disabled_hover_text("Level needs the client data.") } else { r.on_hover_text(help_hint(m)) };
                if r.clicked() {
                    self.settings.heat.mode = m;
                }
            }
        });
        if self.settings.heat.mode != Mode::Off {
            ui.horizontal(|ui| {
                ui.label("tile");
                for t in heat::TILES {
                    ui.selectable_value(&mut self.settings.heat.tile, t, t.to_string()).on_hover_text("Side of one heatmap tile in units.");
                }
            });
            ui.add(egui::Slider::new(&mut self.settings.heat.opacity, 0.2..=0.9).text("heat opacity")).on_hover_text("How opaque the heatmap is.");
            let mut auto = self.settings.heat.max.is_none();
            if ui.checkbox(&mut auto, "auto scale").on_hover_text("The top of the colour ramp is the 95th percentile of this map's tiles.").changed() {
                self.settings.heat.max = if auto { None } else { Some(self.r2.heat.as_ref().map_or(5.0, |c| c.scale_max).max(1.0)) };
            }
            if let Some(m) = self.settings.heat.max.as_mut() {
                ui.add(egui::DragValue::new(m).range(0.5..=500.0).speed(0.1).prefix("max ")).on_hover_text("Monsters per tile at the top of the ramp.");
            }
        }
        ui.checkbox(&mut self.settings.rulers, "rulers").on_hover_text("Rulers along the top and left of the 2D view (Ctrl+R).");
    }

    // -------------------------------------------------------------------------------------------------------------- spawn data

    /// `<folder key>#<map id>`, the key of a map's level range in the settings.
    fn band_key(&self, id: u32) -> String {
        format!("{}#{}", crate::settings::path_key(&self.settings.svmap_folder.clone().unwrap_or_default()), id)
    }

    fn spawn_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.data_gen().hash(&mut h);
        self.settings.spawn_tolerance.hash(&mut h);
        self.settings.spawn_bands.hash(&mut h);
        self.settings.svmap_folder.hash(&mut h);
        self.have_levels().hash(&mut h);
        self.client_addr().hash(&mut h);
        self.folder.as_ref().is_some_and(|f| f.rx.is_some()).hash(&mut h);
        h.finish()
    }

    /// The spawn checks over every map (folder copies, open tabs live), computed again only when something they read changed.
    pub(super) fn ensure_spawn_data(&mut self) {
        let key = self.spawn_key();
        if self.r2.data.as_ref().is_some_and(|d| d.key == key) {
            return;
        }
        let tolerance = self.settings.spawn_tolerance;
        let levels = self.have_levels();
        let bands = self.settings.spawn_bands.clone();
        let folder_key = crate::settings::path_key(&self.settings.svmap_folder.clone().unwrap_or_default());
        let over = move |id: u32| bands.get(&format!("{folder_key}#{id}")).map(|b| (b[0], b[1]));
        let loading = self.folder.as_ref().is_some_and(|f| f.rx.is_some());
        let (report, totals, never) = {
            let ins = self.all_map_ins();
            self.with_env(|env| {
                let r = spawns::analyze(&ins, env, &spawns::Opts { tolerance, overrides: &over, levels });
                let t = r.totals(&ins);
                let never = match self.client.as_deref().and_then(|c| c.monsters.as_ref()) {
                    Some(m) if !loading && !ins.is_empty() => Some(Arc::new(spawns::never_spawned(&ins.iter().map(|m| m.map).collect::<Vec<_>>(), m))),
                    _ => None,
                };
                (r, t, never)
            })
        };
        self.r2.data = Some(SpawnData { key, report: Arc::new(report), totals, never });
    }

    /// The folder totals as the Spawns tab shows them (the CLI test compares with `svmap-editor spawns --json`).
    #[cfg(test)]
    pub(super) fn spawn_totals(&mut self) -> Option<(Totals, Option<usize>)> {
        self.ensure_spawn_data();
        let d = self.r2.data.as_ref()?;
        Some((d.totals.clone(), d.never.as_ref().map(|n| n.len())))
    }

    /// The findings count of the active map for the Dock's tab name (None before the tab has run once).
    pub(super) fn spawn_tab_count(&self) -> Option<usize> {
        let id = self.docs.get(self.active)?.map_id;
        let d = self.r2.data.as_ref()?;
        Some(d.report.findings_of(id).count())
    }

    // -------------------------------------------------------------------------------------------------------------- the tab

    pub(super) fn spawns_body(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() && self.folder.is_none() {
            ui.label("No map or map folder open.");
            return;
        }
        self.ensure_spawn_data();
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.r2.ui.all_maps, false, "this map").on_hover_text("The spawn table and findings of the open map.");
            ui.selectable_value(&mut self.r2.ui.all_maps, true, "all maps").on_hover_text("Every map of the folder (open tabs with their edits).");
            ui.label("tolerance");
            ui.add(egui::DragValue::new(&mut self.settings.spawn_tolerance).range(0..=40).suffix(" lv")).on_hover_text("Levels up to this far past the map's range count as inside it.");
            if !self.have_levels() {
                ui.label(RichText::new("No client data: levels are not checked.").weak());
            }
        });
        if self.r2.ui.all_maps {
            self.spawns_folder(ui);
        } else if self.docs.is_empty() {
            ui.label("No map open.");
        } else {
            self.spawns_map(ui);
        }
    }

    /// The level range row of a map: derived or set, with a pencil that opens two numbers.
    fn band_row(&mut self, ui: &mut egui::Ui, id: Option<u32>, report: &Report) {
        let s = report.summary(id).cloned();
        let hint = match id {
            Some(i) => self.links_now().and_then(|l| spawns::entry_hint(&l, i)),
            None => None,
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Level range").strong());
            let (text, tip) = match &s {
                Some(s) if s.band.is_some() => {
                    let how = if s.overridden { "set by you" } else { "from this map's spawns" };
                    (format!("{} ({how})", range_text(s.band)), "The 10th to 90th percentile of the monster levels on this map, by count.")
                }
                _ => ("none (fewer than 5 monsters with a level)".to_string(), "A range needs at least 5 monster entries with a level. Set one yourself with the pencil."),
            };
            ui.label(text).on_hover_text(match &hint {
                Some(h) => format!("{tip}\n{h}"),
                None => tip.to_string(),
            });
            // the numbers belong to the map they were opened on: another map (or none with an id) closes them
            if self.r2.ui.band_edit.is_some_and(|(m, _, _)| Some(m) != id) {
                self.r2.ui.band_edit = None;
            }
            let Some(i) = id else { return };
            if self.r2.ui.band_edit.is_none() && ui.small_button("Edit").on_hover_text("Set this map's level range yourself.").clicked() {
                let (lo, hi) = s.as_ref().and_then(|s| s.band).unwrap_or((1, 80));
                self.r2.ui.band_edit = Some((i, lo, hi));
            }
            if let Some((m, mut lo, mut hi)) = self.r2.ui.band_edit {
                ui.add(egui::DragValue::new(&mut lo).range(1..=99).prefix("from "));
                ui.add(egui::DragValue::new(&mut hi).range(1..=99).prefix("to "));
                self.r2.ui.band_edit = Some((m, lo, hi));
                if ui.small_button("Set").on_hover_text("Keep this range for this map.").clicked() {
                    let key = self.band_key(m);
                    self.settings.spawn_bands.insert(key, [lo.min(hi), lo.max(hi)]);
                    self.r2.ui.band_edit = None;
                }
                if ui.small_button("Cancel").clicked() {
                    self.r2.ui.band_edit = None;
                }
            }
            if s.as_ref().is_some_and(|s| s.overridden) && ui.small_button("Reset").on_hover_text("Go back to the range from the spawns.").clicked() {
                let key = self.band_key(i);
                self.settings.spawn_bands.remove(&key);
            }
        });
    }

    fn spawns_map(&mut self, ui: &mut egui::Ui) {
        let Some(report) = self.r2.data.as_ref().map(|d| d.report.clone()) else { return };
        let (id, map_ok) = match self.docs.get(self.active) {
            Some(d) => (d.map_id, true),
            None => (None, false),
        };
        if !map_ok {
            return;
        }
        self.band_row(ui, id, &report);
        let mut rows: Vec<SpawnRow> = self.with_env(|env| self.docs.get(self.active).map(|d| spawns::table(&d.map, env)).unwrap_or_default());
        let (col, desc) = self.r2.ui.sort;
        rows.sort_by(|a, b| {
            let o = match col {
                Col::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                Col::Level => a.level.cmp(&b.level),
                Col::Areas => a.areas.cmp(&b.areas),
                Col::Count => a.total.cmp(&b.total),
                Col::Share => a.share.total_cmp(&b.share),
            };
            let o = o.then(a.mob.cmp(&b.mob));
            if desc { o.reverse() } else { o }
        });
        let mine: Vec<(usize, &SpawnFinding)> = report.findings.iter().enumerate().filter(|(_, f)| f.map == id).collect();
        let pal = theme::pal_ui(ui);
        let mut acts: Vec<SAct> = Vec::new();
        ui.separator();
        ui.columns(2, |cols| {
            // ---- the spawn table
            {
                let ui = &mut cols[0];
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click());
                let head = [("Monster", Col::Name, 4.0, false), ("Lv", Col::Level, 222.0, true), ("Areas", Col::Areas, 272.0, true), ("Count", Col::Count, 330.0, true), ("Share", Col::Share, 384.0, true)];
                let p = ui.painter();
                for (title, c, x, right) in head {
                    let arrow = if c == col { if desc { " v" } else { " ^" } } else { "" };
                    let (anchor, pos) = if right { (Align2::RIGHT_CENTER, rect.left() + x) } else { (Align2::LEFT_CENTER, rect.left() + x) };
                    p.text(egui::pos2(pos, rect.center().y), anchor, format!("{title}{arrow}"), FontId::proportional(12.0), pal.weak);
                }
                if resp.on_hover_text("Click a column name to sort.").clicked() {
                    if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                        let x = pos.x - rect.left();
                        let target = if x < 200.0 { Col::Name } else if x < 247.0 { Col::Level } else if x < 301.0 { Col::Areas } else if x < 357.0 { Col::Count } else { Col::Share };
                        acts.push(SAct::Sort(target));
                    }
                }
                if rows.is_empty() {
                    ui.label("This map has no monster areas.");
                }
                egui::ScrollArea::vertical().id_salt("spawn-table").auto_shrink([false, false]).show_rows(ui, 18.0, rows.len(), |ui, range| {
                    for i in range {
                        let r = &rows[i];
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 2.0, pal.hover);
                        }
                        let font = FontId::proportional(12.0);
                        let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 200.0, rect.bottom())));
                        clip.text(egui::pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, format!("{} ({})", r.name, r.mob), font.clone(), pal.text);
                        let right = |x: f32, t: String| ui.painter().text(egui::pos2(rect.left() + x, rect.center().y), Align2::RIGHT_CENTER, t, font.clone(), pal.text);
                        right(222.0, r.level.map_or("-".to_string(), |l| l.to_string()));
                        right(272.0, r.areas.to_string());
                        right(330.0, r.total.to_string());
                        right(384.0, format!("{:.1}%", r.share * 100.0));
                        let mut flags: Vec<Group> = Group::ALL.iter().copied().filter(|g| mine.iter().any(|(_, f)| f.group == *g && f.mobs.contains(&r.mob))).collect();
                        flags.dedup();
                        for (k, g) in flags.iter().enumerate() {
                            let colour = match g {
                                Group::LevelOutside | Group::Stray => pal.warning,
                                Group::Duplicate | Group::Mixed => pal.info,
                                Group::UnknownLevel => pal.weak,
                            };
                            let at = egui::Rect::from_center_size(egui::pos2(rect.left() + 400.0 + k as f32 * 15.0, rect.center().y), Vec2::splat(13.0));
                            icons::paint(ui.painter(), at, Icon::WarnTriangle, colour);
                        }
                        let tip = if flags.is_empty() { format!("{} (id {}). Click selects its areas. Double-click finds it in all maps.", r.name, r.mob) } else { format!("{} (id {}). Findings: {}.", r.name, r.mob, flags.iter().map(|g| g.label()).collect::<Vec<_>>().join(", ")) };
                        let resp = resp.on_hover_text(tip);
                        if resp.double_clicked() {
                            acts.push(SAct::FindMob(r.mob));
                        } else if resp.clicked() {
                            acts.push(SAct::SelectMob(r.mob));
                        }
                    }
                });
            }
            // ---- the findings of this map
            {
                let ui = &mut cols[1];
                self.findings_list(ui, &report, &mine.iter().map(|(i, _)| *i).collect::<Vec<_>>(), false, &mut acts);
            }
        });
        self.do_spawn_acts(acts, ui.ctx());
    }

    /// The findings grouped by kind, each group collapsible with a Select all; `idx` indexes `report.findings`.
    fn findings_list(&mut self, ui: &mut egui::Ui, report: &Report, idx: &[usize], with_map: bool, acts: &mut Vec<SAct>) {
        let pal = theme::pal_ui(ui);
        if idx.is_empty() {
            ui.label(RichText::new("No findings.").color(pal.ok));
            return;
        }
        let sys = self.sysmsg.clone();
        egui::ScrollArea::vertical().id_salt(if with_map { "spawn-findings-all" } else { "spawn-findings" }).auto_shrink([false, false]).show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for g in Group::ALL {
                let of: Vec<usize> = idx.iter().copied().filter(|i| report.findings[*i].group == g).collect();
                if of.is_empty() {
                    continue;
                }
                let closed = self.r2.ui.collapsed.contains(&g);
                ui.horizontal(|ui| {
                    let arrow = if closed { ">" } else { "v" };
                    if ui.selectable_label(false, RichText::new(format!("{arrow} {} · {}", g.label(), of.len())).strong()).on_hover_text(g.tip()).clicked() {
                        acts.push(SAct::ToggleGroup(g));
                    }
                    if ui.small_button("Select all").on_hover_text("Select these areas on the open map.").clicked() {
                        acts.push(SAct::SelectGroup(g));
                    }
                });
                if closed {
                    continue;
                }
                for i in of {
                    let f = &report.findings[i];
                    let text = if with_map {
                        let m = f.map.map_or("-".to_string(), |m| format!("{m:>3} {}", shaiya_clientdata::map_name(m, sys.as_deref())));
                        format!("{m}: {}", f.text)
                    } else {
                        f.text.clone()
                    };
                    let r = ui.selectable_label(false, text).on_hover_text("Click to select and frame it.");
                    if r.clicked() {
                        acts.push(SAct::Open(i));
                    }
                }
            }
        });
    }

    fn spawns_folder(&mut self, ui: &mut egui::Ui) {
        let Some((report, totals, never)) = self.r2.data.as_ref().map(|d| (d.report.clone(), d.totals.clone(), d.never.clone())) else { return };
        let loading = self.folder.as_ref().map(|f| (f.rx.is_some(), f.maps.values().filter(|m| m.svmap.is_some()).count(), f.maps.len()));
        let pal = theme::pal_ui(ui);
        let all: usize = totals.findings.iter().sum();
        ui.horizontal(|ui| {
            ui.label(format!("{} maps · {} areas · {} monsters · {} kinds · {} findings", totals.maps, thousands(totals.areas), thousands(totals.monsters as usize), thousands(totals.kinds), thousands(all)));
            if let Some((true, n, m)) = loading {
                ui.label(RichText::new(format!("loading maps: {n} of {m}")).color(pal.warning));
            }
        });
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.r2.ui.view, FolderView::Maps, "Maps");
            ui.selectable_value(&mut self.r2.ui.view, FolderView::Findings, format!("Findings ({all})"));
            let n = never.as_ref().map_or(String::new(), |n| format!(" ({})", n.len()));
            ui.selectable_value(&mut self.r2.ui.view, FolderView::Never, format!("Never spawned{n}")).on_hover_text("Monsters in Monster.SData that no monster area spawns.");
        });
        ui.separator();
        let mut acts: Vec<SAct> = Vec::new();
        match self.r2.ui.view {
            FolderView::Maps => {
                let sys = self.sysmsg.clone();
                let mono = |t: String| RichText::new(t).monospace();
                ui.selectable_label(false, mono(format!("{:>4} {:<22} {:>6} {:>8} {:>5} {:>9} {:>9}", "map", "name", "areas", "monsters", "kinds", "range", "findings"))).on_hover_text("Click a map to open it.");
                egui::ScrollArea::vertical().id_salt("spawn-maps").auto_shrink([false, false]).show_rows(ui, 18.0, report.maps.len(), |ui, range| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    for i in range {
                        let m = &report.maps[i];
                        let n: usize = m.counts.iter().sum();
                        let name = m.id.map_or(String::new(), |id| shaiya_clientdata::map_name(id, sys.as_deref()));
                        let text = format!("{:>4} {:<22.22} {:>6} {:>8} {:>5} {:>9} {:>9}", m.id.map_or("-".to_string(), |i| i.to_string()), name, m.areas, m.monsters, m.kinds, range_text(m.band), n);
                        let tip = Group::ALL.iter().map(|g| format!("{} {}", g.label(), m.counts[g.index()])).collect::<Vec<_>>().join("\n");
                        let r = ui.selectable_label(false, mono(text)).on_hover_text(tip);
                        if r.clicked() {
                            if let Some(id) = m.id {
                                acts.push(SAct::OpenMap(id));
                            }
                        }
                    }
                });
            }
            FolderView::Findings => {
                let idx: Vec<usize> = (0..report.findings.len()).collect();
                self.findings_list(ui, &report, &idx, true, &mut acts);
            }
            FolderView::Never => self.never_list(ui, never, &mut acts),
        }
        self.do_spawn_acts(acts, ui.ctx());
    }

    fn never_list(&mut self, ui: &mut egui::Ui, never: Option<Arc<Vec<u32>>>, acts: &mut Vec<SAct>) {
        let Some(never) = never else {
            ui.label(if self.have_levels() { "Waiting for the maps to load." } else { "Needs the client data (File > Open client folder...)." });
            return;
        };
        let rules = self.client.clone();
        let info = |id: u32| rules.as_deref().and_then(|r| r.monsters.as_ref()).and_then(|m| m.get(id)).map(|m| (m.name.trim().to_string(), m.level.max(0) as u32));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.r2.ui.never_filter).hint_text("filter by id or name").desired_width(160.0));
            ui.add(egui::DragValue::new(&mut self.r2.ui.never_lo).range(0..=99).prefix("from lv "));
            ui.add(egui::DragValue::new(&mut self.r2.ui.never_hi).range(0..=99).prefix("to lv ")).on_hover_text("0 = no upper limit.");
            ui.label(RichText::new("Bosses, obelisks and scripted spawns live in other files.").weak());
        });
        let q = self.r2.ui.never_filter.trim().to_lowercase();
        let (lo, hi) = (self.r2.ui.never_lo, self.r2.ui.never_hi);
        let shown: Vec<u32> = never
            .iter()
            .copied()
            .filter(|id| {
                let Some((name, l)) = info(*id) else { return false };
                (q.is_empty() || name.to_lowercase().contains(&q) || id.to_string() == q) && l >= lo && (hi == 0 || l <= hi)
            })
            .collect();
        ui.horizontal(|ui| {
            ui.label(format!("{} of {}", shown.len(), never.len()));
            if ui.small_button("Copy ids").on_hover_text("Copy the ids shown, separated by commas.").clicked() {
                acts.push(SAct::CopyIds(shown.clone()));
            }
        });
        egui::ScrollArea::vertical().id_salt("spawn-never").auto_shrink([false, false]).show_rows(ui, 18.0, shown.len(), |ui, range| {
            for i in range {
                let id = shown[i];
                let (name, l) = info(id).unwrap_or_default();
                ui.label(RichText::new(format!("{id:>5}  lv {l:>3}  {name}")).monospace());
            }
        });
    }

    fn do_spawn_acts(&mut self, acts: Vec<SAct>, ctx: &egui::Context) {
        for a in acts {
            match a {
                SAct::SelectMob(m) => {
                    if let Some(d) = self.docs.get_mut(self.active) {
                        d.selection = d.map.mob_areas.iter().enumerate().filter(|(_, a)| a.mobs.iter().any(|e| e.mob == m)).map(|(i, _)| (SectionId::MobAreas, i)).collect();
                        d.sub = None;
                    }
                }
                SAct::FindMob(m) => {
                    self.show_find(false);
                    self.q3.find.query = format!("mob:{m}");
                }
                SAct::Sort(c) => {
                    let (cur, desc) = self.r2.ui.sort;
                    self.r2.ui.sort = if cur == c { (c, !desc) } else { (c, c != Col::Name) };
                }
                SAct::Open(i) => self.open_spawn_finding(i),
                SAct::SelectGroup(g) => {
                    let id = self.docs.get(self.active).map(|d| d.map_id);
                    let Some(report) = self.r2.data.as_ref().map(|d| d.report.clone()) else { continue };
                    if let (Some(id), Some(d)) = (id, self.docs.get_mut(self.active)) {
                        let n = d.map.mob_areas.len();
                        let mut sel: Vec<(SectionId, usize)> = report.findings_of(id).filter(|f| f.group == g).flat_map(|f| f.areas.iter().copied()).filter(|i| *i < n).map(|i| (SectionId::MobAreas, i)).collect();
                        sel.sort();
                        sel.dedup();
                        d.selection = sel;
                        d.sub = None;
                    }
                    self.frame_selection();
                }
                SAct::ToggleGroup(g) => {
                    if !self.r2.ui.collapsed.remove(&g) {
                        self.r2.ui.collapsed.insert(g);
                    }
                }
                SAct::OpenMap(id) => self.open_map_id(id),
                SAct::CopyIds(ids) => {
                    let text = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
                    ctx.copy_text(text);
                    self.toast(format!("copied {} id{}", ids.len(), if ids.len() == 1 { "" } else { "s" }));
                }
            }
        }
    }

    /// Opens the map of a finding and selects and frames its areas.
    pub(super) fn open_spawn_finding(&mut self, i: usize) {
        let Some(f) = self.r2.data.as_ref().and_then(|d| d.report.findings.get(i).cloned()) else { return };
        let Some(&first) = f.areas.first() else { return };
        self.open_hit(&Hit { map: f.map, section: SectionId::MobAreas, index: first, label: String::new(), what: String::new() });
        if f.areas.len() > 1 {
            if let Some(d) = self.docs.get_mut(self.active) {
                if d.map_id == f.map {
                    let n = d.map.mob_areas.len();
                    d.selection = f.areas.iter().filter(|a| **a < n).map(|a| (SectionId::MobAreas, *a)).collect();
                    d.sub = None;
                }
            }
            self.frame_selection();
        }
    }

    /// Show the Dock on the Spawns tab (Tools > Spawns > Spawn check).
    pub(super) fn spawn_check(&mut self) {
        self.open_dock(DockTab::Spawns);
    }
}

fn help_hint(m: Mode) -> &'static str {
    match m {
        Mode::Off => "Hide the heatmap.",
        Mode::Density => "Monsters per tile.",
        Mode::Level => "Mean monster level per tile.",
    }
}
