//! Spawn work (spawn editing): the Scatter tool's options, preview and Apply, and
//! the Bulk edit spawns window with saved rules. Declared from `app.rs` with `#[path]`, so it shares the window's state.
//!
//! Every edit goes through `Doc::commit` (one undo step), which refuses on a read-only tab. Scatter adds areas to the open map.
//! Bulk edit changes one map per undo step and skips a map whose only tab is read-only. Nothing here saves a map.

use std::collections::HashSet;
use std::sync::Arc;

use egui::containers::menu::{MenuButton, MenuConfig};
use egui::{PopupCloseBehavior, RichText};
use shaiya_svmap::{MobArea, SectionId};

use super::App;
use crate::bulk::{self, Action, CountOp, Opts, Plan, Rule};
use crate::edit::{self, Op, Rec};
use crate::icons::{self, Icon};
use crate::layout::DockTab;
use crate::names::Lookups;
use crate::scatter::{self, Preset, Region};
use crate::search::{Hit, Query};
use crate::settings::BulkRule;
use crate::theme;
use crate::view2d::Tool;

// ------------------------------------------------------------------------------------------------------------------ state

/// (tab, its edits, the region's revision, the options, client data, ground data)
type ScatterKey = (u64, u64, u64, scatter::ScatterSettings, usize, usize);

pub struct ScatterCache {
    key: ScatterKey,
    pub preview: scatter::Preview,
    /// The line shown over the map: what the preview holds.
    pub note: String,
    /// Changes with every recompute (the heatmap waits for it).
    gen: u64,
}

#[derive(Default)]
pub struct ScatterUi {
    pub cache: Option<ScatterCache>,
    counter: u64,
}

impl ScatterUi {
    /// The areas the heatmap counts besides the map's own.
    pub fn preview_areas(&self) -> &[MobArea] {
        self.cache.as_ref().map_or(&[], |c| &c.preview.areas)
    }

    pub fn gen_for_heat(&self) -> u64 {
        self.cache.as_ref().map_or(0, |c| c.gen)
    }
}

/// Which monster field the picker fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickTarget {
    ScatterAdd,
    BulkFrom,
    BulkTo,
    BulkMob,
}

pub struct PickWin {
    pub target: PickTarget,
    pub query: String,
    /// The id typed when there is no client list.
    pub id: u32,
}

/// The Bulk edit spawns window.
pub struct BulkWin {
    pub text: String,
    pub allow_high: bool,
    pub save_name: String,
}

type PlanKey = (String, bool, u64, usize);

pub struct PlanCache {
    key: PlanKey,
    pub result: Result<(Rule, Plan), String>,
}

#[derive(Default)]
pub struct R3 {
    pub scatter: ScatterUi,
    pub bulk: Option<BulkWin>,
    bulk_plan: Option<Arc<PlanCache>>,
    pub pick: Option<PickWin>,
}

/// What a click in the Bulk window asked for (done after the window is drawn).
enum BAct {
    Apply,
    SaveRule,
    LoadRule(usize),
    DeleteRule(usize),
    Open(Option<u32>, usize),
    Pick(PickTarget),
    Text(String),
}

const KINDS: [&str; 11] = ["count +", "count -", "count =", "count *", "replace", "add", "remove", "delete", "grow", "shrink", "slab"];

fn kind_of(a: &Action) -> usize {
    match a {
        Action::Count(CountOp::Add(_)) => 0,
        Action::Count(CountOp::Sub(_)) => 1,
        Action::Count(CountOp::Set(_)) => 2,
        Action::Count(CountOp::Mul(_)) => 3,
        Action::Replace { .. } => 4,
        Action::Add { .. } => 5,
        Action::Remove(_) => 6,
        Action::Delete => 7,
        Action::Grow(d) if *d >= 0.0 => 8,
        Action::Grow(_) => 9,
        Action::Slab(_) => 10,
    }
}

fn default_action(kind: usize, mob: u32) -> Action {
    match kind {
        0 => Action::Count(CountOp::Add(1)),
        1 => Action::Count(CountOp::Sub(1)),
        2 => Action::Count(CountOp::Set(2)),
        3 => Action::Count(CountOp::Mul(1.5)),
        4 => Action::Replace { from: mob, to: mob + 1 },
        5 => Action::Add { mob, count: 1 },
        6 => Action::Remove(mob),
        7 => Action::Delete,
        8 => Action::Grow(1.0),
        9 => Action::Grow(-1.0),
        _ => Action::Slab(2.0),
    }
}

/// The monster an action names first (a new kind of action starts from it).
fn mob_of(a: &Action) -> Option<u32> {
    match a {
        Action::Replace { from, .. } => Some(*from),
        Action::Add { mob, .. } | Action::Remove(mob) => Some(*mob),
        _ => None,
    }
}

/// The action after `=>` as the builder shows it. A replace whose two monsters are still the same (Find's Replace monster...
/// opens the window that way) is not a valid rule yet, but the builder shows it as a replace so the second monster can be picked.
fn loose_action(text: &str) -> Option<Action> {
    let right = text.split_once("=>").map(|(_, r)| r.trim())?;
    Action::parse(right).ok().or_else(|| {
        let mut w = right.split_whitespace();
        if !w.next()?.eq_ignore_ascii_case("replace") {
            return None;
        }
        let from: u32 = w.next()?.parse().ok()?;
        let to = w.nth(1).and_then(|s| s.parse().ok()).unwrap_or(from);
        Some(Action::Replace { from, to })
    })
}

/// The part of a rule before `=>`.
fn where_of(text: &str) -> String {
    text.split("=>").next().unwrap_or("").trim().to_string()
}

impl App {
    // -------------------------------------------------------------------------------------------------------------- scatter

    /// The monster names and levels as the chips and the picker show them.
    fn mob_chip(&self, id: u32, weight: u32) -> String {
        let (name, level) = self.with_env(|env| (env.mob_name(id), (env.mob)(id).map(|(_, l)| l)));
        match level.filter(|l| *l > 0) {
            Some(l) => format!("{name} lv {l} \u{d7}{weight}"),
            None => format!("{name} \u{d7}{weight}"),
        }
    }

    /// Recomputes the preview when the region, the options or the map changed (called before the viewport is drawn).
    pub(super) fn scatter_update(&mut self) {
        let want = self.tool == Tool::Scatter && !self.mode3d && self.docs.get(self.active).is_some_and(|d| !d.read_only) && self.vp.scatter.region.is_some();
        if !want {
            self.r3.scatter.cache = None;
            return;
        }
        let assets = self.current_assets();
        let rules = self.client.clone();
        let d = &self.docs[self.active];
        let key: ScatterKey = (d.uid, d.edit_gen.wrapping_add(d.base_gen.wrapping_mul(1_000_003)), self.vp.scatter.rev, self.settings.scatter.clone(), self.client_addr(), assets.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize));
        if self.r3.scatter.cache.as_ref().is_some_and(|c| c.key == key) {
            return;
        }
        let Some(region) = self.vp.scatter.region.clone() else { return };
        let params = self.settings.scatter.params();
        let look = Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
        let ground = |x: f32, z: f32| look.ground(x, z);
        let existing: Vec<[f32; 4]> = d.map.mob_areas.iter().map(|a| [a.min[0].min(a.max[0]), a.min[2].min(a.max[2]), a.min[0].max(a.max[0]), a.min[2].max(a.max[2])]).collect();
        let site = scatter::Site { size: d.map.size, blocked: &d.map.blocked, existing: &existing, ground: &ground };
        let preview = scatter::generate(&params, &region, &site);
        let level = |id: u32| rules.as_deref().and_then(|r| r.monsters.as_ref()).and_then(|m| m.get(id)).map(|m| m.level as f32);
        let note = if params.mobs.is_empty() { "Add a monster first.".to_string() } else { preview.summary(&level) };
        self.r3.scatter.counter += 1;
        let gen = self.r3.scatter.counter;
        self.r3.scatter.cache = Some(ScatterCache { key, preview, note, gen });
    }

    /// The Scatter tool's controls in the toolbar row.
    pub(super) fn scatter_toolbar(&mut self, ui: &mut egui::Ui) {
        let chips: Vec<String> = self.settings.scatter.mobs.clone().iter().map(|m| self.mob_chip(m[0], m[1])).collect();
        egui::ComboBox::from_id_salt("scatter-preset").selected_text(self.settings.scatter.preset.label()).show_ui(ui, |ui| {
            for p in Preset::ALL {
                ui.selectable_value(&mut self.settings.scatter.preset, p, p.label());
            }
        });
        ui.label("seed").on_hover_text(crate::help_text::tip("scatter-seed"));
        ui.add(egui::DragValue::new(&mut self.settings.scatter.seed).range(0..=999_999)).on_hover_text(crate::help_text::tip("scatter-seed"));
        if icons::icon_button_sized(ui, Icon::Dice, false, "Reseed (R). Places the areas another way.", 22.0, true).clicked() {
            self.scatter_reseed();
        }
        ui.separator();
        // the monsters: chips (a click edits the weight or removes), then +
        let popover = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
        let show = chips.len().min(2);
        for (i, label) in chips.iter().enumerate().take(show) {
            self.mob_chip_menu(ui, i, label, &popover);
        }
        if chips.len() > show {
            let (r, _) = MenuButton::new(format!("+{} more", chips.len() - show)).config(popover.clone()).ui(ui, |ui| {
                for (i, label) in chips.iter().enumerate().skip(show) {
                    self.mob_chip_menu(ui, i, label, &popover);
                }
            });
            r.on_hover_text("More monsters in this scatter.");
        }
        if ui.small_button("+").on_hover_text(crate::help_text::tip("scatter-add")).clicked() {
            self.r3.pick = Some(PickWin { target: PickTarget::ScatterAdd, query: String::new(), id: 1 });
        }
        let (r, _) = MenuButton::new("Options").config(popover).ui(ui, |ui| {
            let s = &mut self.settings.scatter;
            if s.preset == Preset::Custom {
                ui.horizontal(|ui| {
                    ui.label("spacing");
                    ui.add(egui::DragValue::new(&mut s.spacing).range(4.0..=200.0).speed(0.5));
                    ui.label("box");
                    ui.add(egui::DragValue::new(&mut s.size).range(1.0..=64.0).speed(0.25));
                });
                ui.horizontal(|ui| {
                    ui.label("count");
                    ui.add(egui::DragValue::new(&mut s.count_min).range(1..=8));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut s.count_max).range(1..=8));
                });
            }
            ui.checkbox(&mut s.avoid_blocked, "avoid blocked").on_hover_text(crate::help_text::tip("scatter-avoid"));
            ui.checkbox(&mut s.keep_clear, "keep clear of areas").on_hover_text(crate::help_text::tip("scatter-clear"));
            ui.checkbox(&mut s.two_kinds, "two kinds per box").on_hover_text(crate::help_text::tip("scatter-two"));
            if !self.settings.templates.is_empty() {
                ui.separator();
                ui.label(RichText::new("From template").small().strong());
                let mut pick = None;
                for (i, t) in self.settings.templates.iter().enumerate() {
                    if ui.button(&t.name).on_hover_text(crate::help_text::tip("scatter-template")).clicked() {
                        pick = Some(i);
                    }
                }
                if let Some(i) = pick {
                    self.scatter_from_template(i);
                    ui.close();
                }
            }
        });
        r.on_hover_text("Box size, counts and the three switches.");
        if ui.button("Fill selection").on_hover_text(crate::help_text::tip("scatter-fill")).clicked() {
            self.scatter_fill_selection();
        }
        let n = self.r3.scatter.cache.as_ref().map_or(0, |c| c.preview.areas.len());
        let why = if self.settings.scatter.mobs.is_empty() {
            "Add a monster first."
        } else if self.vp.scatter.region.is_none() {
            "Draw a region first."
        } else {
            "No area fits here."
        };
        let r = ui.add_enabled(n > 0, egui::Button::new(format!("Apply {n}")));
        let r = if n > 0 { r.on_hover_text("Add the previewed areas to the map as one undo step (Enter).") } else { r.on_disabled_hover_text(why) };
        if r.clicked() {
            self.scatter_apply();
        }
    }

    /// One monster chip: its text; the popover holds the weight and Remove.
    fn mob_chip_menu(&mut self, ui: &mut egui::Ui, i: usize, label: &str, popover: &MenuConfig) {
        let (r, _) = MenuButton::new(label).config(popover.clone()).ui(ui, |ui| {
            if let Some(m) = self.settings.scatter.mobs.get_mut(i) {
                ui.horizontal(|ui| {
                    ui.label("weight");
                    ui.add(egui::DragValue::new(&mut m[1]).range(1..=9));
                });
            }
            if ui.button("Remove").clicked() {
                if i < self.settings.scatter.mobs.len() {
                    self.settings.scatter.mobs.remove(i);
                }
                ui.close();
            }
        });
        r.on_hover_text(crate::help_text::tip("scatter-chip"));
    }

    /// R: another seed.
    pub(super) fn scatter_reseed(&mut self) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let old = self.settings.scatter.seed;
        let mut s = (t ^ old.wrapping_mul(2_654_435_761)) % 1_000_000;
        if s == old {
            s = (s + 1) % 1_000_000;
        }
        self.settings.scatter.seed = s;
    }

    /// Fill selection: the boxes of the selected monster areas, bind areas and regions are the region.
    pub(super) fn scatter_fill_selection(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let mut boxes: Vec<[f32; 4]> = Vec::new();
        for (s, i) in &d.selection {
            let b = |min: [f32; 3], max: [f32; 3]| [min[0], min[2], max[0], max[2]];
            match edit::get(&d.map, *s, *i) {
                Some(Rec::Mob(a)) => boxes.push(b(a.min, a.max)),
                Some(Rec::Region(g)) => boxes.push(b(g.min, g.max)),
                Some(Rec::Bind(x)) => boxes.extend(x.boxes.iter().map(|y| b(y.min, y.max))),
                _ => {}
            }
        }
        if boxes.is_empty() {
            self.toast("Select a monster area, bind area or region first.");
            return;
        }
        self.vp.scatter.set(Region::from_boxes(&boxes));
    }

    /// From template: the template's monsters, counts as weights.
    pub(super) fn scatter_from_template(&mut self, i: usize) {
        let Some(t) = self.settings.templates.get(i) else { return };
        let mut mobs: Vec<[u32; 2]> = Vec::new();
        for e in &t.entries {
            if mobs.len() < scatter::MAX_MOBS && !mobs.iter().any(|m| m[0] == e[0]) {
                mobs.push([e[0], e[1].clamp(1, 9)]);
            }
        }
        if mobs.is_empty() {
            self.toast("That template has no monsters.");
            return;
        }
        self.settings.scatter.mobs = mobs;
    }

    /// The Scatter keys: Enter closes a polygon and then applies, Backspace takes the last polygon point back, R reseeds.
    pub(super) fn scatter_keys(&mut self, ctx: &egui::Context) {
        let (enter, back, reseed) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::R) && !i.modifiers.command && !i.modifiers.alt));
        if back && self.vp.scatter.region.is_none() {
            self.vp.scatter.poly.pop();
        }
        if enter {
            if self.vp.scatter.region.is_none() {
                self.vp.scatter.close_poly();
            } else {
                self.scatter_apply();
            }
        }
        if reseed && self.vp.scatter.region.is_some() {
            self.scatter_reseed();
        }
    }

    /// Apply: the previewed areas are added to the map as one undo step and selected. A read-only tab refuses.
    pub(super) fn scatter_apply(&mut self) {
        self.scatter_update();
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if d.read_only {
            self.toast("This tab is read-only.");
            return;
        }
        let Some(c) = self.r3.scatter.cache.as_ref() else {
            self.toast("Draw a region first.");
            return;
        };
        let areas = c.preview.areas.clone();
        if areas.is_empty() {
            self.toast(if self.settings.scatter.mobs.is_empty() { "Add a monster first." } else { "No area fits here." });
            return;
        }
        let first = d.map.mob_areas.len();
        let n = areas.len();
        let ops: Vec<Op> = areas.into_iter().enumerate().map(|(k, a)| Op::insert(first + k, Rec::Mob(a))).collect();
        d.commit(format!("scatter {n} area{}", if n == 1 { "" } else { "s" }), ops);
        if d.map.mob_areas.len() == first + n {
            d.selection = (first..first + n).map(|i| (SectionId::MobAreas, i)).collect();
            d.sub = None;
            self.vp.scatter.clear();
            self.r3.scatter.cache = None;
            self.toast(format!("Added {n} monster area{}.", if n == 1 { "" } else { "s" }));
        }
    }

    // -------------------------------------------------------------------------------------------------------------- monster picker

    /// Monsters of the client list matching `query` (name or id), at most 200.
    fn monster_list(&self, query: &str) -> Vec<(u32, String)> {
        let q = query.trim().to_lowercase();
        match self.client.as_deref().and_then(|c| c.monsters.as_ref()) {
            Some(m) => m.records.iter().enumerate().map(|(i, r)| (i as u32, format!("{i:>5}  {}  (lv {})", r.name.trim(), r.level))).filter(|(_, t)| q.is_empty() || t.to_lowercase().contains(&q)).take(200).collect(),
            None => Vec::new(),
        }
    }

    /// The small window that picks a monster for Scatter or for the Bulk rule.
    pub(super) fn monster_pick_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.r3.pick.take() else { return };
        let list = self.monster_list(&w.query);
        let have_list = self.client.as_deref().is_some_and(|c| c.monsters.is_some());
        let mut chosen: Option<u32> = None;
        let mut open = true;
        egui::Window::new("Pick a monster").collapsible(false).open(&mut open).default_size([360.0, 380.0]).show(ctx, |ui| {
            if have_list {
                ui.add(egui::TextEdit::singleline(&mut w.query).hint_text("name or id").desired_width(f32::INFINITY));
                egui::ScrollArea::vertical().max_height(280.0).id_salt("pick-list").show_rows(ui, 18.0, list.len(), |ui, range| {
                    for i in range {
                        if ui.selectable_label(false, RichText::new(&list[i].1).monospace()).clicked() {
                            chosen = Some(list[i].0);
                        }
                    }
                });
            } else {
                ui.label("No client data. Type the monster id.");
                ui.add(egui::DragValue::new(&mut w.id).range(0..=65535));
                if ui.button("Use this id").clicked() {
                    chosen = Some(w.id);
                }
            }
        });
        match chosen {
            Some(id) => self.apply_pick(w.target, id),
            None if open => self.r3.pick = Some(w),
            None => {}
        }
    }

    pub(super) fn apply_pick(&mut self, target: PickTarget, id: u32) {
        match target {
            PickTarget::ScatterAdd => self.settings.scatter.add_mob(id),
            _ => {
                let Some(w) = self.r3.bulk.as_mut() else { return };
                let cur = loose_action(&w.text);
                let new = match (target, cur) {
                    (PickTarget::BulkFrom, Some(Action::Replace { to, .. })) => Action::Replace { from: id, to },
                    (PickTarget::BulkTo, Some(Action::Replace { from, .. })) => Action::Replace { from, to: id },
                    (PickTarget::BulkMob, Some(Action::Add { count, .. })) => Action::Add { mob: id, count },
                    (PickTarget::BulkMob, Some(Action::Remove(_))) => Action::Remove(id),
                    (PickTarget::BulkTo, _) => Action::Replace { from: id, to: id + 1 },
                    _ => Action::Remove(id),
                };
                w.text = format!("{} => {}", where_of(&w.text), new.text());
            }
        }
    }

    // -------------------------------------------------------------------------------------------------------------- bulk edit

    /// Tools > Spawns > Bulk edit...: opens the window (with a rule typed in, when given).
    pub(super) fn open_bulk(&mut self, text: Option<String>) {
        match self.r3.bulk.as_mut() {
            Some(w) => {
                if let Some(t) = text {
                    w.text = t;
                }
            }
            None => self.r3.bulk = Some(BulkWin { text: text.unwrap_or_default(), allow_high: false, save_name: String::new() }),
        }
    }

    /// Find's Replace monster... and Bulk edit... buttons: the window with Find's query as the condition.
    pub(super) fn open_bulk_from_find(&mut self, replace: Option<u32>) {
        let q = Query::parse(&self.q3.find.query).text();
        let scope = match (self.q3.find.this_map, self.docs.get(self.active).and_then(|d| d.map_id)) {
            (true, Some(id)) => format!("map:{id} "),
            _ => String::new(),
        };
        let left = format!("{scope}{q}").trim().to_string();
        let action = match replace {
            Some(m) => format!("replace {m} with {m}"),
            None => "count +1".to_string(),
        };
        self.open_bulk(Some(if left.is_empty() { String::new() } else { format!("{left} => {action}") }));
    }

    /// The plan of the window's rule, recomputed when the rule, the option or any map changed.
    fn bulk_refresh(&mut self, text: &str, allow_high: bool) -> Arc<PlanCache> {
        let key: PlanKey = (text.to_string(), allow_high, self.data_gen(), self.client_addr());
        if let Some(c) = &self.r3.bulk_plan {
            if c.key == key {
                return c.clone();
            }
        }
        let result = match Rule::parse(text) {
            Err(e) => Err(e),
            Ok(rule) => {
                let ins = self.all_map_ins();
                let plan = self.with_env(|env| bulk::plan(&rule, &ins, env, Opts { allow_above_8: allow_high }, None));
                Ok((rule, plan))
            }
        };
        let c = Arc::new(PlanCache { key, result });
        self.r3.bulk_plan = Some(c.clone());
        c
    }

    /// The monster areas of the open map that the window's rule matches (the cyan outline).
    pub(super) fn bulk_outline(&self) -> Vec<usize> {
        let (Some(c), Some(id)) = (&self.r3.bulk_plan, self.docs.get(self.active).and_then(|d| d.map_id)) else { return Vec::new() };
        if self.r3.bulk.is_none() {
            return Vec::new();
        }
        match &c.result {
            Ok((_, p)) => p.per_map.iter().find(|m| m.id == Some(id)).map(|m| m.matched.clone()).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Runs a rule on every map it matches: each map in an editable tab (opened from the folder when needed), one undo step per map
    /// labelled "bulk: <rule>". A map whose only tab is read-only is skipped; nothing is saved. `allow` limits the areas (Replace
    /// monster from Find). Returns the summary.
    pub(super) fn bulk_apply(&mut self, rule: &Rule, opts: Opts, allow: Option<&HashSet<(Option<u32>, usize)>>) -> String {
        let plan = {
            let ins = self.all_map_ins();
            self.with_env(|env| bulk::plan(rule, &ins, env, opts, allow))
        };
        let back = self.docs.get(self.active).map(|d| d.uid);
        let (mut areas, mut maps, mut capped, mut emptied) = (0usize, 0usize, 0usize, 0usize);
        let (mut skipped, mut failed) = (Vec::new(), Vec::new());
        let label = format!("bulk: {}", rule.text());
        for mp in plan.per_map.iter().filter(|m| !m.areas.is_empty()) {
            // the tab to edit: an editable tab of the map (the active one for a map without a number)
            let pos = match mp.id {
                None => self.docs.get(self.active).filter(|d| !d.read_only && d.map_id.is_none()).map(|_| self.active),
                Some(id) => self.docs.iter().position(|d| d.map_id == Some(id) && !d.read_only),
            };
            let pos = match pos {
                Some(p) => p,
                None => {
                    let Some(id) = mp.id else {
                        skipped.push("the unnumbered map".to_string());
                        continue;
                    };
                    if self.docs.iter().any(|d| d.map_id == Some(id) && d.read_only) {
                        skipped.push(format!("{id} (read-only tab)"));
                        continue;
                    }
                    let before = self.docs.len();
                    self.open_map_id(id);
                    if self.docs.len() == before {
                        failed.push(id.to_string());
                        continue;
                    }
                    self.docs.len() - 1
                }
            };
            // the plan of the tab as it is now (an opened map may differ from the folder's copy)
            let mine = self.with_env(|env| bulk::plan_map(rule, mp.id, &self.docs[pos].map, env, opts, &mp.matched));
            let ops = mine.ops();
            if ops.is_empty() {
                continue;
            }
            areas += mine.areas.len();
            maps += 1;
            capped += mine.capped;
            emptied += mine.emptied;
            self.docs[pos].commit(label.clone(), ops);
        }
        if let Some(p) = back.and_then(|u| self.docs.iter().position(|d| d.uid == u)) {
            self.active = p;
        }
        let head = match &rule.action {
            Action::Replace { from, to } => format!("Replaced monster {from} with {to}"),
            _ => format!("Bulk edit ({})", rule.action.text()),
        };
        let mut s = format!("{head} in {areas} area{} on {maps} map{}; nothing is saved yet.", if areas == 1 { "" } else { "s" }, if maps == 1 { "" } else { "s" });
        if capped > 0 {
            s.push_str(&format!(" Held at 8: {capped}."));
        }
        if emptied > 0 {
            s.push_str(&format!(" Left empty: {emptied}."));
        }
        if !skipped.is_empty() {
            s.push_str(&format!(" Skipped: {}.", skipped.join(", ")));
        }
        if !failed.is_empty() {
            s.push_str(&format!(" Could not open: {}.", failed.join(", ")));
        }
        crate::log::info(s.clone());
        s
    }

    /// The Bulk edit spawns window: the rule, a builder for its action, the preview per map and the saved rules.
    pub(super) fn bulk_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.r3.bulk.take() else { return };
        let cache = self.bulk_refresh(&w.text, w.allow_high);
        let sys = self.sysmsg.clone();
        let read_only_only = |id: Option<u32>, docs: &[crate::doc::Doc]| id.is_some_and(|i| docs.iter().any(|d| d.map_id == Some(i) && d.read_only) && !docs.iter().any(|d| d.map_id == Some(i) && !d.read_only));
        let have_levels = self.have_levels();
        let mut acts: Vec<BAct> = Vec::new();
        let mut open = true;
        let pal = theme::pal(ctx);
        let rules: Vec<BulkRule> = self.settings.bulk_rules.clone();
        egui::Window::new("Bulk edit spawns").collapsible(false).open(&mut open).default_size([620.0, 560.0]).show(ctx, |ui| {
            ui.label(RichText::new("where => action. Example: map:1-5 mob:12 => count +1").weak().small());
            ui.add(egui::TextEdit::singleline(&mut w.text).hint_text("map:1-5 mob:12 => count +1").desired_width(f32::INFINITY).font(egui::TextStyle::Monospace)).on_hover_text(crate::help_text::tip("bulk-rule"));
            // the action builder: it rewrites the text after =>
            let cur = loose_action(&w.text).or_else(|| cache.result.as_ref().ok().map(|(r, _)| r.action.clone()));
            let mut new_action: Option<Action> = None;
            ui.horizontal_wrapped(|ui| {
                let kind = cur.as_ref().map_or(0, kind_of);
                let mut k = kind;
                egui::ComboBox::from_id_salt("bulk-kind").selected_text(KINDS[kind]).show_ui(ui, |ui| {
                    for (i, name) in KINDS.iter().enumerate() {
                        ui.selectable_value(&mut k, i, *name);
                    }
                });
                let mut a = cur.clone().unwrap_or_else(|| default_action(0, 1));
                if k != kind {
                    let mob = cur.as_ref().and_then(mob_of).or_else(|| Query::parse(&where_of(&w.text)).mobs().first().copied()).unwrap_or(1);
                    a = default_action(k, mob);
                    new_action = Some(a.clone());
                }
                let mut changed = false;
                match &mut a {
                    Action::Count(CountOp::Add(n) | CountOp::Sub(n) | CountOp::Set(n)) => changed |= ui.add(egui::DragValue::new(n).range(1..=99)).changed(),
                    Action::Count(CountOp::Mul(f)) => changed |= ui.add(egui::DragValue::new(f).range(0.1..=20.0).speed(0.05)).changed(),
                    Action::Replace { from, to } => {
                        changed |= ui.add(egui::DragValue::new(from).range(0..=65535)).changed();
                        if ui.small_button("Pick").on_hover_text(crate::help_text::tip("pick-monster")).clicked() {
                            acts.push(BAct::Pick(PickTarget::BulkFrom));
                        }
                        ui.label("with");
                        changed |= ui.add(egui::DragValue::new(to).range(0..=65535)).changed();
                        if ui.small_button("Pick").on_hover_text(crate::help_text::tip("pick-monster")).clicked() {
                            acts.push(BAct::Pick(PickTarget::BulkTo));
                        }
                    }
                    Action::Add { mob, count } => {
                        changed |= ui.add(egui::DragValue::new(mob).range(0..=65535)).changed();
                        if ui.small_button("Pick").on_hover_text(crate::help_text::tip("pick-monster")).clicked() {
                            acts.push(BAct::Pick(PickTarget::BulkMob));
                        }
                        ui.label("count");
                        changed |= ui.add(egui::DragValue::new(count).range(1..=8)).changed();
                    }
                    Action::Remove(m) => {
                        changed |= ui.add(egui::DragValue::new(m).range(0..=65535)).changed();
                        if ui.small_button("Pick").on_hover_text(crate::help_text::tip("pick-monster")).clicked() {
                            acts.push(BAct::Pick(PickTarget::BulkMob));
                        }
                    }
                    Action::Delete => {
                        ui.label(RichText::new("Removes the matched areas. Undo brings them back.").weak());
                    }
                    Action::Grow(d) => {
                        let mut v = d.abs();
                        if ui.add(egui::DragValue::new(&mut v).range(0.1..=500.0).speed(0.1).suffix(" each side")).changed() {
                            *d = if *d < 0.0 { -v } else { v };
                            changed = true;
                        }
                    }
                    Action::Slab(h) => changed |= ui.add(egui::DragValue::new(h).range(0.0..=500.0).speed(0.1).prefix("height ")).changed(),
                }
                if changed {
                    new_action = Some(a);
                }
                ui.checkbox(&mut w.allow_high, "allow above 8").on_hover_text(crate::help_text::tip("bulk-high"));
            });
            if let Some(a) = new_action {
                acts.push(BAct::Text(format!("{} => {}", where_of(&w.text), a.text())));
            }
            ui.separator();
            match &cache.result {
                Err(e) => {
                    if !w.text.trim().is_empty() {
                        ui.label(RichText::new(e).color(pal.error));
                    } else {
                        ui.label(RichText::new("Type a rule. map:1-5 mob:12 => count +1").weak());
                    }
                }
                Ok((rule, plan)) => {
                    let (n, m) = (plan.matched(), plan.per_map.len());
                    let (c, cm) = (plan.changed(), plan.maps_changed());
                    let mut line = format!("{n} area{} match on {m} map{}. {c} to change on {cm} map{}. {} entr{} changed.", if n == 1 { "" } else { "s" }, if m == 1 { "" } else { "s" }, if cm == 1 { "" } else { "s" }, plan.entries(), if plan.entries() == 1 { "y" } else { "ies" });
                    if plan.capped() > 0 {
                        line.push_str(&format!(" Held at 8: {}.", plan.capped()));
                    }
                    if plan.emptied() > 0 {
                        line.push_str(&format!(" Left empty: {}.", plan.emptied()));
                    }
                    ui.label(line);
                    if matches!(rule.action, Action::Count(_)) && !have_levels && rule.query.tokens.iter().any(|t| matches!(t, crate::search::Token::Level(..))) {
                        ui.label(RichText::new("lv: needs the client data.").color(pal.warning));
                    }
                    egui::ScrollArea::vertical().max_height(260.0).id_salt("bulk-preview").show(ui, |ui| {
                        for mp in &plan.per_map {
                            let name = mp.id.map_or("this map".to_string(), |i| format!("{i} {}", shaiya_clientdata::map_name(i, sys.as_deref())));
                            let mut head = format!("{name}: {} matched, {} to change, monsters {} to {}", mp.matched.len(), mp.areas.len(), mp.before, mp.after);
                            if mp.capped > 0 {
                                head.push_str(&format!(", held at 8: {}", mp.capped));
                            }
                            if mp.emptied > 0 {
                                head.push_str(&format!(", left empty: {}", mp.emptied));
                            }
                            if read_only_only(mp.id, &self.docs) {
                                head.push_str(", skipped: read-only tab");
                            }
                            egui::CollapsingHeader::new(head).id_salt(("bulk-map", mp.id)).show(ui, |ui| {
                                for a in mp.areas.iter().take(200) {
                                    let text = format!("#{} {}, now {}", a.index, a.before_text, a.after_text);
                                    if ui.selectable_label(false, RichText::new(text).monospace()).clicked() {
                                        acts.push(BAct::Open(mp.id, a.index));
                                    }
                                }
                                if mp.areas.len() > 200 {
                                    ui.label(RichText::new(format!("and {} more", mp.areas.len() - 200)).weak());
                                }
                            });
                        }
                    });
                }
            }
            ui.separator();
            let (can, n_areas, n_maps) = match &cache.result {
                Ok((_, p)) => (p.changed() > 0, p.changed(), p.maps_changed()),
                Err(_) => (false, 0, 0),
            };
            ui.horizontal(|ui| {
                let r = ui.add_enabled(can, egui::Button::new(format!("Apply to {n_areas} area{} on {n_maps} map{}", if n_areas == 1 { "" } else { "s" }, if n_maps == 1 { "" } else { "s" })));
                let r = if can { r.on_hover_text("Each map opens in a tab. One undo step per map. Nothing is saved.") } else { r.on_disabled_hover_text("The rule must be valid and change something.") };
                if r.clicked() {
                    acts.push(BAct::Apply);
                }
                ui.add(egui::TextEdit::singleline(&mut w.save_name).hint_text("rule name").desired_width(140.0));
                let ok = cache.result.is_ok() && !w.save_name.trim().is_empty();
                let r = ui.add_enabled(ok, egui::Button::new("Save rule"));
                if (if ok { r.on_hover_text(crate::help_text::tip("bulk-save")) } else { r.on_disabled_hover_text("Type a name and a valid rule.") }).clicked() {
                    acts.push(BAct::SaveRule);
                }
            });
            if !rules.is_empty() {
                egui::CollapsingHeader::new(format!("Saved rules ({})", rules.len())).id_salt("bulk-saved").default_open(true).show(ui, |ui| {
                    for (i, r) in rules.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button(&r.name).on_hover_text(&r.text).clicked() {
                                acts.push(BAct::LoadRule(i));
                            }
                            ui.label(RichText::new(&r.text).weak().small());
                            if ui.small_button("x").on_hover_text("Delete this saved rule.").clicked() {
                                acts.push(BAct::DeleteRule(i));
                            }
                        });
                    }
                });
            }
        });
        let mut keep = open;
        for a in acts {
            match a {
                BAct::Text(t) => w.text = t,
                BAct::Pick(t) => self.r3.pick = Some(PickWin { target: t, query: String::new(), id: 1 }),
                BAct::Open(map, index) => self.open_hit(&Hit { map, section: SectionId::MobAreas, index, label: String::new(), what: String::new() }),
                BAct::LoadRule(i) => {
                    if let Some(r) = rules.get(i) {
                        w.text = r.text.clone();
                        w.save_name = r.name.clone();
                    }
                }
                BAct::DeleteRule(i) => {
                    if i < self.settings.bulk_rules.len() {
                        self.settings.bulk_rules.remove(i);
                    }
                }
                BAct::SaveRule => {
                    let name = w.save_name.trim().to_string();
                    if let Ok(rule) = Rule::parse(&w.text) {
                        let text = rule.text();
                        match self.settings.bulk_rules.iter_mut().find(|r| r.name == name) {
                            Some(r) => r.text = text,
                            None => self.settings.bulk_rules.push(BulkRule { name, text }),
                        }
                        self.toast("Rule saved.");
                    }
                }
                BAct::Apply => {
                    if let Ok((rule, _)) = Rule::parse(&w.text).map(|r| (r, ())) {
                        let s = self.bulk_apply(&rule, Opts { allow_above_8: w.allow_high }, None);
                        self.q3.find.summary = Some(s.clone());
                        self.open_dock(DockTab::Find);
                        self.toast(s);
                        keep = false;
                    }
                }
            }
        }
        if keep {
            self.r3.bulk = Some(w);
        }
    }

    /// Replace monster (Find): the same engine with the Find query as the condition, limited to the areas Find lists.
    /// Kept as the entry point the tests use; the window builds the same rule (Find's Replace monster... opens it filled in).
    #[cfg(test)]
    pub(super) fn replace_monster(&mut self, from: u32, to: u32) -> String {
        // the results must describe the maps as they are now (an edit since the last run moves indices)
        self.run_find();
        let allow: HashSet<(Option<u32>, usize)> = self.q3.find.results.iter().filter(|h| h.section == SectionId::MobAreas).map(|h| (h.map, h.index)).collect();
        let rule = Rule { query: Query::parse(&self.q3.find.query), action: Action::Replace { from, to } };
        self.bulk_apply(&rule, Opts::default(), Some(&allow))
    }
}

