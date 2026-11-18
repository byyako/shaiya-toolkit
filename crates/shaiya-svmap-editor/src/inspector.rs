//! The inspector: typed fields for the selected record, with a "?" tooltip per field saying what the server does with it.
//! Edits go to the document as undo steps; numeric edits of one field merge into one step.

use egui::{DragValue, Ui};
use shaiya_svmap::{Aabb, Bind, MobArea, MobEntry, NpcEntry, NpcPoint, Portal, RawA, Region, SectionId};

use crate::client_rules::PORTAL_USE_RANGE;
use crate::doc::Doc;
use crate::edit::{self, Op, Rec};
use crate::icons::{self, Icon};
use crate::names::{self, Lookups};

/// Things the inspector asks the app for (pickers and confirmations need app-level state).
pub enum InspReq {
    PickMob { area: usize, entry: usize },
    PickNpc { npc: usize },
    PickDest { portal: usize },
    DropToGround,
    MovePortal { idx: usize, up: bool },
    DeleteSelected,
    Duplicate,
    FrameSelection,
    /// Run fix number `.1` of this finding.
    Fix(shaiya_svmap::Finding, usize),
    /// Select every record similar to the selection (same section and key).
    SelectSimilar,
    /// Open the destination map of this portal and show where it leads.
    GoToDest { portal: usize },
    /// Set the portal destination's y from the destination map's ground.
    DropDestToGround { portal: usize },
    /// Ask for a name and keep this monster area as a template.
    SaveTemplate { area: usize },
    /// Open map `map` and select its portal `index` (from "Portals leading here").
    OpenPortal { map: u32, index: usize },
    /// Something to tell the user (a client edit that was refused).
    Toast(String),
}

/// A portal of another map that leads to the open map.
pub struct Incoming {
    pub from: u32,
    pub index: usize,
    pub label: String,
}

pub struct InspCtx<'a> {
    pub look: Lookups<'a>,
    /// (id, name) of the maps of the open folder.
    pub maps: &'a [(u32, String)],
    pub map_name: &'a dyn Fn(u32) -> String,
    /// Show the how-to texts (Preferences > Show hints).
    pub hints: bool,
    /// The findings of the selected record with the titles of their fixes (index = the fix number), from `fixes::fixes_for`.
    pub fixes: &'a [(shaiya_svmap::Finding, Vec<String>)],
    /// The snap step (Snap to grid in the multi-selection panel).
    pub snap: f32,
    /// Portals of other maps that lead to this map (the map card lists them).
    pub incoming: &'a [Incoming],
    /// Which collapsing sections the user opened or closed (section id to open); the app keeps it in the settings.
    pub sections: &'a std::cell::RefCell<std::collections::BTreeMap<String, bool>>,
}

#[derive(Default)]
struct Ch {
    key: Option<String>,
    structural: bool,
}

impl Ch {
    fn mark(&mut self, k: &str) {
        if self.key.is_none() {
            self.key = Some(k.to_string());
        }
    }

    fn structural(&mut self, k: &str) {
        self.mark(k);
        self.structural = true;
    }
}

fn round2(v: f64) -> f32 {
    ((v * 100.0).round() / 100.0) as f32
}

/// One float field; only a field the user touched is rewritten, and then rounded to 0.01.
fn fnum(ui: &mut Ui, ch: &mut Ch, key: &str, v: &mut f32) {
    let mut t = *v;
    if ui.add(DragValue::new(&mut t).speed(0.1).max_decimals(2)).changed() {
        *v = round2(t as f64);
        ch.mark(key);
    }
}

fn unum(ui: &mut Ui, ch: &mut Ch, key: &str, v: &mut u32, range: std::ops::RangeInclusive<u32>) {
    if ui.add(DragValue::new(v).clamp_existing_to_range(false).range(range)).changed() {
        ch.mark(key);
    }
}

fn row_label(ui: &mut Ui, name: &str, tip: &str) {
    ui.horizontal(|ui| {
        ui.label(name);
        if !tip.is_empty() {
            ui.label("?").on_hover_text(tip);
        }
    });
}

fn finish(doc: &mut Doc, sec: SectionId, idx: usize, before: Rec, after: Rec, ch: Ch) {
    let Some(key) = ch.key else { return };
    if before == after {
        return;
    }
    let label = format!("edit {} #{idx}", sec.label());
    let ops = vec![Op::replace(idx, before, after)];
    if ch.structural {
        doc.commit(label, ops);
    } else {
        doc.commit_coalesced(&format!("{sec:?}:{idx}:{key}"), label, ops);
    }
}

// ---------------------------------------------------------------------------------------------------------------- texts

pub const TIP_FRAME: &str = "Frame the selection (F).";
pub const TIP_DROP: &str = "Drop to ground (G).";
pub const TIP_DUPLICATE: &str = "Duplicate (Ctrl+D).";
pub const TIP_DELETE: &str = "Delete (Del).";
pub const TIP_SIMILAR: &str = "Select similar: same section and key.";
pub const TIP_OPEN_PORTAL: &str = "Open that map and select the portal.";
pub const TIP_UNSAVED: &str = "Not saved yet.";
pub const TIP_Y: &str = "Height. New records get the ground height.";
pub const TIP_FROM: &str = "Lower corner of the box.";
pub const TIP_TO: &str = "Upper corner of the box. It must not be below the lower corner.";
pub const TIP_PICK_MOB: &str = "Pick a monster by name.";
pub const TIP_COUNT: &str = "How many of this monster the box keeps alive (1-8 in the originals).";
pub const TIP_REMOVE_ENTRY: &str = "Remove this entry.";
pub const TIP_TEMPLATE: &str = "Keep this area's monsters, size and height for the Box tool.";
pub const TIP_NPC_TYPE: &str = "1 merchant ... 13 combat commander. Type 0 is a placeholder (58 in the originals).";
pub const TIP_NPC_ID: &str = "The id-th record (1-based) of that type's list in NpcQuest.SData.";
pub const TIP_PICK_NPC: &str = "Pick by name.";
pub const TIP_POINTS: &str = "Point 1 is where the NPC stands. More points are a walk path (guards).";
pub const TIP_SELECT_POINT: &str = "Select this point in the viewport.";
pub const TIP_DIR: &str = "The way the NPC faces, in degrees.";
pub const TIP_INSERT_POINT: &str = "Insert a point after this one.";
pub const TIP_REMOVE_POINT: &str = "Remove this point.";
pub const TIP_RAW_DIR: &str = "The stored facing, in radians.";
pub const TIP_PORTAL_POS: &str = "Where the portal stands. Keep it within 40 units of the client's portal box centre.";
pub const TIP_PORTAL_COUNTRY: &str = "0 any faction, 1 Light, 2 Fury, 1001-1007 instance ids.";
pub const TIP_PORTAL_LEVELS: &str = "0 means no limit. The server checks the player's level.";
pub const TIP_PORTAL_MAP: &str = "Map id. 200 is the guild ranking and has no file.";
pub const TIP_PORTAL_DEST: &str = "Where the player arrives on the destination map.";
pub const TIP_PICK_DEST: &str = "Click the destination on the destination map.";
pub const TIP_PORTAL_DEST_NAME: &str = "The destination name the client shows. The client's .wld stores it.";
pub const TIP_GO_DEST: &str = "Open the destination map and show where this portal leads.";
pub const TIP_DROP_DEST: &str = "Set the destination's y from the destination map's ground. Needs the client.";
pub const TIP_MOVE_PORTAL: &str = "Changes this portal's index, which the client sends. Asks first.";
pub const TIP_BIND_COUNTRY: &str = "0 both, 1 Light, 2 Fury, 3 both.";
pub const TIP_ATTR: &str = "Always 0 in the originals.";
pub const TIP_REMOVE_BOX: &str = "Remove this box.";
pub const TIP_REGION_NAME: &str = "The client's name for this area: line <index> of world/<id>.txt. Enter applies it.";
pub const TIP_REGION_KIND: &str = "The client's area mode. 2 is a picture caption (<id>_NN.tga).";
pub const TIP_REGION_B: &str = "Raw. Original tails are mostly (0, 0, 0) and (2, 0, 0). A few are (1, 3, 100).";
pub const TIP_REGION_C: &str = "Raw u8.";
pub const TIP_REGION_D: &str = "Raw u16. 100 in the few original regions that use it.";
// batch fields
pub const TIP_B_COUNT: &str = "The count of each monster entry. Empty means the entries differ.";
pub const TIP_B_SLAB: &str = "y2 = y1 + height. The originals use 2.0.";
pub const TIP_B_GROW: &str = "Move each side of every box out or in by this many units. A box stays 1 unit wide.";
pub const TIP_B_FACING: &str = "The way the NPC faces (its first point), in degrees. 0 is north (+z), 90 is east.";
pub const TIP_B_NPC_TYPE: &str = "NPC type (0-13).";
pub const TIP_B_NPC_ID: &str = "The position in that type's list in NpcQuest.SData.";
pub const TIP_B_LEVEL: &str = "0 means no limit.";
pub const TIP_B_KIND: &str = "The client's area mode. 2 is a picture caption.";
// align
pub const TIP_ALIGN: &str = "Line the selection up by its bounds. Heights stay.";
pub const TIP_DISTRIBUTE: &str = "Equal gaps between the centres. The two outermost records stay.";
pub const TIP_DISTRIBUTE_NEED: &str = "Needs three or more records.";
pub const TIP_SAME_SIZE: &str = "Every box takes the width and depth of the first one, around its own centre.";
pub const TIP_SAME_SIZE_NEED: &str = "The first selected record must be a monster area or a region.";

pub const TEXT_AREA: &str = "Spawn box. The server spawns the listed monsters inside it.";
pub const TEXT_BIND: &str = "Respawn area. The server picks the nearest one of the player's country.";
pub const TEXT_REGION: &str = "Named area. The client has the same list in its .wld, in the same order.";
pub const TEXT_SECTION_A: &str = "Section A: a raw row. None exist in the original files.";
pub const TEXT_SECTION_B: &str = "Section B point (community name: ladders). Its use is not known.";
pub const HINT_NOTHING: &str = "Click a record. Drag on empty ground to select several. Ctrl+F filters the outliner.";

/// Every tooltip of the inspector, for the string lint test.
#[cfg(test)]
pub const TIPS: &[&str] = &[
    TIP_UNSAVED, TIP_FRAME, TIP_DROP, TIP_DUPLICATE, TIP_DELETE, TIP_SIMILAR, TIP_OPEN_PORTAL, TIP_Y, TIP_FROM, TIP_TO, TIP_PICK_MOB, TIP_COUNT, TIP_REMOVE_ENTRY, TIP_TEMPLATE, TIP_NPC_TYPE, TIP_NPC_ID,
    TIP_PICK_NPC, TIP_POINTS, TIP_SELECT_POINT, TIP_DIR, TIP_INSERT_POINT, TIP_REMOVE_POINT, TIP_RAW_DIR, TIP_PORTAL_POS, TIP_PORTAL_COUNTRY, TIP_PORTAL_LEVELS, TIP_PORTAL_MAP, TIP_PORTAL_DEST,
    TIP_PICK_DEST, TIP_PORTAL_DEST_NAME, TIP_REGION_NAME, TIP_GO_DEST, TIP_DROP_DEST, TIP_MOVE_PORTAL, TIP_BIND_COUNTRY, TIP_ATTR, TIP_REMOVE_BOX, TIP_REGION_KIND, TIP_REGION_B, TIP_REGION_C, TIP_REGION_D, TIP_B_COUNT, TIP_B_SLAB,
    TIP_B_GROW, TIP_B_FACING, TIP_B_NPC_TYPE, TIP_B_NPC_ID, TIP_B_LEVEL, TIP_B_KIND, TIP_ALIGN, TIP_DISTRIBUTE, TIP_DISTRIBUTE_NEED, TIP_SAME_SIZE, TIP_SAME_SIZE_NEED,
];

/// The explaining lines of the inspector (labels, not tooltips), for the same lint.
#[cfg(test)]
pub const TEXTS: &[&str] = &[TEXT_AREA, TEXT_BIND, TEXT_REGION, TEXT_SECTION_A, TEXT_SECTION_B, HINT_NOTHING];

pub fn show(ui: &mut Ui, doc: &mut Doc, c: &InspCtx) -> Vec<InspReq> {
    let mut reqs = Vec::new();
    match doc.selection.len() {
        0 => nothing(ui, doc, c, &mut reqs),
        1 => {
            let (sec, idx) = doc.selection[0];
            if let Some(rec) = edit::get(&doc.map, sec, idx) {
                single(ui, doc, c, sec, idx, rec, &mut reqs);
            } else {
                doc.clear_selection();
            }
        }
        n => egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| multi(ui, doc, c, n, &mut reqs)).inner,
    }
    reqs
}

// ---------------------------------------------------------------------------------------------------------------- sections

/// A collapsing section whose open state is remembered in the settings under `id`. `default_open` applies until the user clicks.
fn section(ui: &mut Ui, c: &InspCtx, id: &str, title: &str, default_open: bool, body: impl FnOnce(&mut Ui)) {
    let open = c.sections.borrow().get(id).copied().unwrap_or(default_open);
    let r = egui::CollapsingHeader::new(title).id_salt(("insp-section", id)).open(Some(open)).show(ui, body);
    if r.header_response.clicked() {
        c.sections.borrow_mut().insert(id.to_string(), !open);
    }
}

/// The record actions as one row of icon buttons with tooltips.
fn action_row(ui: &mut Ui, reqs: &mut Vec<InspReq>) {
    ui.horizontal(|ui| {
        for (icon, tip, req) in [
            (Icon::Box, TIP_FRAME, InspReq::FrameSelection),
            (Icon::Chevron, TIP_DROP, InspReq::DropToGround),
            (Icon::Compare, TIP_DUPLICATE, InspReq::Duplicate),
            (Icon::Close, TIP_DELETE, InspReq::DeleteSelected),
            (Icon::Scatter, TIP_SIMILAR, InspReq::SelectSimilar),
        ] {
            if icons::icon_button_sized(ui, icon, false, tip, 24.0, true).clicked() {
                reqs.push(req);
            }
        }
    });
}

/// Several records selected: the actions, the batch fields when they are all of one section, and align and distribute.
fn multi(ui: &mut Ui, doc: &mut Doc, c: &InspCtx, n: usize, reqs: &mut Vec<InspReq>) {
    ui.heading(format!("{n} records selected"));
    let mut counts: Vec<(SectionId, usize)> = Vec::new();
    for (s, _) in &doc.selection {
        match counts.iter_mut().find(|(x, _)| x == s) {
            Some(e) => e.1 += 1,
            None => counts.push((*s, 1)),
        }
    }
    for (s, k) in &counts {
        ui.label(format!("{k} x {}", s.label()));
    }
    action_row(ui, reqs);
    if let [(sec, _)] = counts.as_slice() {
        let sec = *sec;
        let idxs: Vec<usize> = doc.selection.iter().map(|(_, i)| *i).collect();
        section(ui, c, "batch", "Batch", true, |ui| batch_fields(ui, doc, sec, &idxs));
    } else {
        ui.label(egui::RichText::new("Several sections are selected: batch fields need records of one section.").weak().small());
    }
    section(ui, c, "align", "Align", false, |ui| align_panel(ui, doc, c.snap));
}

// ---------------------------------------------------------------------------------------------------------------- batch edit

/// The selected records of a section (clones), with their indices.
fn picked(doc: &Doc, sec: SectionId, idxs: &[usize]) -> Vec<(usize, Rec)> {
    idxs.iter().filter_map(|i| edit::get(&doc.map, sec, *i).map(|r| (*i, r))).collect()
}

/// Applies `f` to each record; the ones it changes become one undo step (`coalesce`: typing a number is one step).
fn commit_each(doc: &mut Doc, sec: SectionId, idxs: &[usize], label: &str, coalesce: Option<&str>, f: &mut dyn FnMut(&mut Rec) -> bool) {
    let mut ops = Vec::new();
    for (i, before) in picked(doc, sec, idxs) {
        let mut after = before.clone();
        if f(&mut after) && after != before {
            ops.push(Op::replace(i, before, after));
        }
    }
    match coalesce {
        Some(k) => doc.commit_coalesced(&format!("batch:{sec:?}:{k}"), label, ops),
        None => doc.commit(label, ops),
    }
}

/// A number field for a value that several records hold: it shows the shared value, or is empty with the hint "mixed". Returns the
/// number typed (when it parses and differs). While the field has the focus its text is kept as typed.
fn mixed_field(ui: &mut Ui, key: &str, current: Option<u32>) -> Option<u32> {
    let id = batch_field_id(key);
    let shown = current.map_or(String::new(), |v| v.to_string());
    let focused = ui.memory(|m| m.has_focus(id));
    let mut text = if focused { ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.clone()) } else { shown };
    let r = ui.add(egui::TextEdit::singleline(&mut text).id(id).hint_text("mixed").desired_width(60.0));
    let out = if r.changed() { text.trim().parse::<u32>().ok().filter(|v| current != Some(*v)) } else { None };
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove_temp::<String>(id));
    }
    out
}

/// The widget id of a batch field (tests focus a field through it).
pub fn batch_field_id(key: &str) -> egui::Id {
    egui::Id::unique(("svmap-batch", key.to_string()))
}

/// One labelled batch field over a `u32` of every record: `get` reads it, `set` writes it.
#[allow(clippy::too_many_arguments)]
fn batch_u32(ui: &mut Ui, doc: &mut Doc, sec: SectionId, idxs: &[usize], name: &str, tip: &str, key: &str, get: &dyn Fn(&Rec) -> Option<u32>, set: &dyn Fn(&mut Rec, u32)) {
    let recs = picked(doc, sec, idxs);
    let cur = edit::common(recs.iter().filter_map(|(_, r)| get(r)));
    let shown = if recs.iter().all(|(_, r)| get(r).is_some()) { cur } else { None };
    ui.horizontal(|ui| {
        row_label(ui, name, tip);
        if let Some(v) = mixed_field(ui, &format!("{sec:?}-{key}"), shown) {
            commit_each(doc, sec, idxs, &format!("batch: set {name}"), Some(key), &mut |r| {
                set(r, v);
                true
            });
        }
    });
}

/// Runs `f` on the monster area `r` holds (false for any other record).
fn mob_do(r: &mut Rec, f: impl FnOnce(&mut MobArea) -> bool) -> bool {
    match r {
        Rec::Mob(a) => f(a),
        _ => false,
    }
}

/// A persistent number next to an action button (the monster ids and counts of the area actions).
fn temp_u32(ui: &mut Ui, key: &str, default: u32, range: std::ops::RangeInclusive<u32>) -> u32 {
    let id = egui::Id::unique(("svmap-batch-temp", key.to_string()));
    let mut v = ui.data(|d| d.get_temp::<u32>(id)).unwrap_or(default);
    if ui.add(DragValue::new(&mut v).clamp_existing_to_range(false).range(range)).changed() {
        ui.data_mut(|d| d.insert_temp(id, v));
    }
    v
}

fn temp_f32(ui: &mut Ui, key: &str, default: f32) -> f32 {
    let id = egui::Id::unique(("svmap-batch-temp", key.to_string()));
    let mut v = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(default);
    if ui.add(DragValue::new(&mut v).speed(0.1).max_decimals(2)).changed() {
        ui.data_mut(|d| d.insert_temp(id, v));
    }
    v
}

/// The shared count of every monster entry of the selected areas (None when they differ or there are no entries).
pub fn mob_counts_common(m: &shaiya_svmap::Svmap, idxs: &[usize]) -> Option<u32> {
    edit::common(idxs.iter().filter_map(|i| m.mob_areas.get(*i)).flat_map(|a| a.mobs.iter().map(|e| e.count)))
}

fn batch_fields(ui: &mut Ui, doc: &mut Doc, sec: SectionId, idxs: &[usize]) {
    match sec {
        SectionId::MobAreas => {
            let has_entries = idxs.iter().filter_map(|i| doc.map.mob_areas.get(*i)).any(|a| !a.mobs.is_empty());
            if has_entries {
                ui.horizontal(|ui| {
                    row_label(ui, "count (every entry)", TIP_B_COUNT);
                    if let Some(v) = mixed_field(ui, "MobAreas-count", mob_counts_common(&doc.map, idxs)) {
                        commit_each(doc, sec, idxs, "batch: set count", Some("count"), &mut |r| mob_do(r, |a| edit::mob_set_count(a, None, v)));
                    }
                });
            }
            let heights = edit::common(picked(doc, sec, idxs).iter().filter_map(|(_, r)| match r {
                Rec::Mob(a) => Some((a.max[1] - a.min[1]).to_bits()),
                _ => None,
            }));
            ui.horizontal(|ui| {
                row_label(ui, "slab height", TIP_B_SLAB);
                let h = temp_f32(ui, "slab", heights.map_or(2.0, f32::from_bits));
                if ui.button("Set").clicked() {
                    commit_each(doc, sec, idxs, "batch: set slab height", None, &mut |r| match r {
                        Rec::Mob(a) => {
                            edit::mob_set_height(a, h);
                            true
                        }
                        _ => false,
                    });
                }
            });
            ui.horizontal(|ui| {
                row_label(ui, "grow / shrink", TIP_B_GROW);
                let d = temp_f32(ui, "grow", 2.0).abs();
                for (text, sign) in [("Grow", 1.0), ("Shrink", -1.0)] {
                    if ui.button(text).clicked() {
                        commit_each(doc, sec, idxs, &format!("batch: {} areas by {d}", text.to_lowercase()), None, &mut |r| match r {
                            Rec::Mob(a) => {
                                edit::mob_grow(a, d * sign);
                                true
                            }
                            _ => false,
                        });
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("replace monster");
                let x = temp_u32(ui, "rep-x", 1, 0..=65535);
                ui.label("with");
                let y = temp_u32(ui, "rep-y", 2, 0..=65535);
                if ui.button("Replace").clicked() {
                    commit_each(doc, sec, idxs, &format!("batch: replace monster {x} with {y}"), None, &mut |r| mob_do(r, |a| edit::mob_replace(a, x, y)));
                }
            });
            ui.horizontal(|ui| {
                ui.label("set count of monster");
                let x = temp_u32(ui, "cnt-x", 1, 0..=65535);
                ui.label("to");
                let n = temp_u32(ui, "cnt-n", 1, 0..=65535);
                if ui.button("Set").clicked() {
                    commit_each(doc, sec, idxs, &format!("batch: set count of monster {x} to {n}"), None, &mut |r| mob_do(r, |a| edit::mob_set_count(a, Some(x), n)));
                }
            });
            ui.horizontal(|ui| {
                ui.label("add monster");
                let x = temp_u32(ui, "add-x", 1, 0..=65535);
                ui.label("x");
                let n = temp_u32(ui, "add-n", 1, 1..=255);
                if ui.button("Add to all").clicked() {
                    commit_each(doc, sec, idxs, &format!("batch: add monster {x} x{n}"), None, &mut |r| match r {
                        Rec::Mob(a) => {
                            a.mobs.push(MobEntry { mob: x, count: n });
                            true
                        }
                        _ => false,
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label("remove monster");
                let x = temp_u32(ui, "rem-x", 1, 0..=65535);
                if ui.button("Remove from all").clicked() {
                    commit_each(doc, sec, idxs, &format!("batch: remove monster {x}"), None, &mut |r| mob_do(r, |a| edit::mob_remove(a, x)));
                }
            });
        }
        SectionId::Npcs => {
            batch_u32(ui, doc, sec, idxs, "type", TIP_B_NPC_TYPE, "kind", &|r| if let Rec::Npc(n) = r { Some(n.kind) } else { None }, &|r, v| {
                if let Rec::Npc(n) = r {
                    n.kind = v;
                }
            });
            batch_u32(ui, doc, sec, idxs, "type id", TIP_B_NPC_ID, "tid", &|r| if let Rec::Npc(n) = r { Some(n.type_id) } else { None }, &|r, v| {
                if let Rec::Npc(n) = r {
                    n.type_id = v;
                }
            });
            ui.horizontal(|ui| {
                row_label(ui, "facing", TIP_B_FACING);
                let deg = temp_f32(ui, "facing", 0.0);
                if ui.button("Set").clicked() {
                    commit_each(doc, sec, idxs, &format!("batch: set facing {deg}"), None, &mut |r| match r {
                        Rec::Npc(n) => match n.points.first_mut() {
                            Some(p) => {
                                p.dir = round2(deg as f64).to_radians();
                                true
                            }
                            None => false,
                        },
                        _ => false,
                    });
                }
            });
        }
        SectionId::Portals => {
            batch_u32(ui, doc, sec, idxs, "country", TIP_PORTAL_COUNTRY, "country", &|r| if let Rec::Portal(p) = r { Some(p.country) } else { None }, &|r, v| {
                if let Rec::Portal(p) = r {
                    p.country = v;
                }
            });
            batch_u32(ui, doc, sec, idxs, "level min", TIP_B_LEVEL, "lvmin", &|r| if let Rec::Portal(p) = r { Some(p.level_min as u32) } else { None }, &|r, v| {
                if let Rec::Portal(p) = r {
                    p.level_min = v.min(u16::MAX as u32) as u16;
                }
            });
            batch_u32(ui, doc, sec, idxs, "level max", TIP_B_LEVEL, "lvmax", &|r| if let Rec::Portal(p) = r { Some(p.level_max as u32) } else { None }, &|r, v| {
                if let Rec::Portal(p) = r {
                    p.level_max = v.min(u16::MAX as u32) as u16;
                }
            });
        }
        SectionId::Binds => {
            batch_u32(ui, doc, sec, idxs, "country", TIP_BIND_COUNTRY, "country", &|r| if let Rec::Bind(b) = r { Some(b.country) } else { None }, &|r, v| {
                if let Rec::Bind(b) = r {
                    b.country = v;
                }
            });
            batch_u32(ui, doc, sec, idxs, "attr", TIP_ATTR, "attr", &|r| if let Rec::Bind(b) = r { Some(b.attr) } else { None }, &|r, v| {
                if let Rec::Bind(b) = r {
                    b.attr = v;
                }
            });
        }
        SectionId::Regions => {
            batch_u32(ui, doc, sec, idxs, "kind", TIP_B_KIND, "kind", &|r| if let Rec::Region(g) = r { Some(g.kind) } else { None }, &|r, v| {
                if let Rec::Region(g) = r {
                    g.kind = v;
                }
            });
        }
        _ => {
            ui.label(egui::RichText::new("Nothing to batch-edit for this section; move, drop to ground, duplicate, delete and align work.").weak().small());
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- align

/// Replaces the selected records with `new` (same order as the selection); the ones that changed become one undo step.
fn apply_recs(doc: &mut Doc, label: &str, sel: &[(SectionId, usize)], new: Vec<Rec>) {
    let mut ops = Vec::new();
    for ((s, i), after) in sel.iter().zip(new) {
        if let Some(before) = edit::get(&doc.map, *s, *i) {
            if before != after {
                ops.push(Op::replace(*i, before, after));
            }
        }
    }
    doc.commit(label, ops);
}

fn align_panel(ui: &mut Ui, doc: &mut Doc, snap: f32) {
    let sel = doc.selection.clone();
    let recs: Vec<Rec> = sel.iter().filter_map(|(s, i)| edit::get(&doc.map, *s, *i)).collect();
    if recs.len() != sel.len() || recs.len() < 2 {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for a in edit::Align::ALL {
            if ui.button(a.label()).on_hover_text(TIP_ALIGN).clicked() {
                apply_recs(doc, &format!("align {}", a.label()), &sel, edit::align(&recs, a));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let three = recs.len() >= 3;
        for (text, on_x) in [("distribute x", true), ("distribute z", false)] {
            let r = ui.add_enabled(three, egui::Button::new(text));
            let r = if three { r.on_hover_text(TIP_DISTRIBUTE) } else { r.on_disabled_hover_text(TIP_DISTRIBUTE_NEED) };
            if r.clicked() {
                apply_recs(doc, &format!("distribute {}", if on_x { "x" } else { "z" }), &sel, edit::distribute(&recs, on_x));
            }
        }
        if ui.button("snap to grid").on_hover_text(format!("Round each record's corner (an NPC's first point, a portal's position) to the snap step, now {snap}")).clicked() {
            apply_recs(doc, "snap to grid", &sel, edit::snap_to_grid(&recs, snap));
        }
        let boxed = edit::same_size(&recs);
        let r = ui.add_enabled(boxed.is_some(), egui::Button::new("same size as first"));
        let r = if boxed.is_some() { r.on_hover_text(TIP_SAME_SIZE) } else { r.on_disabled_hover_text(TIP_SAME_SIZE_NEED) };
        if r.clicked() {
            if let Some(n) = boxed {
                apply_recs(doc, "same size as first", &sel, n);
            }
        }
    });
}

fn nothing(ui: &mut Ui, doc: &Doc, c: &InspCtx, reqs: &mut Vec<InspReq>) {
    let hints = c.hints;
    let title = ui.heading("Map");
    if let Some(p) = &doc.path {
        title.on_hover_text(p.display().to_string());
    } else {
        title.on_hover_text(TIP_UNSAVED);
    }
    section(ui, c, "map-counts", "Counts", true, |ui| {
        egui::Grid::new("map-info").num_columns(2).show(ui, |ui| {
            let m = &doc.map;
            ui.label("size");
            ui.label(format!("{0} x {0}", m.size));
            ui.end_row();
            ui.label("blocked cells");
            let n = m.blocked.count();
            ui.label(format!("{n} ({:.1} %)", n as f64 * 100.0 / (m.size as f64 * m.size as f64).max(1.0)));
            ui.end_row();
            for (name, n) in [
                ("monster areas", m.mob_areas.len()),
                ("NPCs", m.npcs.len()),
                ("portals", m.portals.len()),
                ("bind areas", m.binds.len()),
                ("regions", m.regions.len()),
                ("section B", m.points_b.len()),
                ("section A", m.section_a.len()),
            ] {
                ui.label(name);
                ui.label(n.to_string());
                ui.end_row();
            }
        });
    });
    if !c.incoming.is_empty() {
        section(ui, c, "map-incoming", &format!("Portals leading here ({})", c.incoming.len()), false, |ui| {
            egui::ScrollArea::vertical().id_salt("incoming").max_height(150.0).show(ui, |ui| {
                for l in c.incoming {
                    if ui.link(&l.label).on_hover_text(TIP_OPEN_PORTAL).clicked() {
                        reqs.push(InspReq::OpenPortal { map: l.from, index: l.index });
                    }
                }
            });
        });
    }
    ui.add_space(6.0);
    ui.label("Nothing selected.");
    if hints {
        ui.label(egui::RichText::new(HINT_NOTHING).weak());
    }
}

fn single(ui: &mut Ui, doc: &mut Doc, c: &InspCtx, sec: SectionId, idx: usize, rec: Rec, reqs: &mut Vec<InspReq>) {
    ui.heading(format!("{} #{idx}", sec.label()));
    ui.label(egui::RichText::new(names::label(&c.look, &doc.map, sec, idx, c.map_name)).weak());
    chain_row(ui, doc, sec);
    action_row(ui, reqs);
    // the findings of this record, with their fixes
    let mine: Vec<shaiya_svmap::Finding> = doc.findings.iter().filter(|f| f.section == sec && f.index == Some(idx)).cloned().collect();
    if !mine.is_empty() {
        let pal = crate::theme::pal_ui(ui);
        section(ui, c, "problems", &format!("Problems ({})", mine.len()), true, |ui| {
            for f in &mine {
                let col = crate::theme::severity(&pal, f.severity);
                let titles: &[String] = c.fixes.iter().find(|(x, _)| x == f).map_or(&[], |(_, t)| t.as_slice());
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(format!("{}: {}", f.severity.label(), f.message)).color(col).small()).on_hover_text(crate::help_text::rule_tip(f.code));
                    match titles.len() {
                        0 => {}
                        1 => {
                            if ui.small_button("Fix").on_hover_text(titles[0].as_str()).clicked() {
                                reqs.push(InspReq::Fix(f.clone(), 0));
                            }
                        }
                        _ => {
                            let r = icons::text_chevron_button(ui, "Fix");
                            egui::Popup::menu(&r).show(|ui| {
                                for (k, t) in titles.iter().enumerate() {
                                    if ui.button(t).clicked() {
                                        reqs.push(InspReq::Fix(f.clone(), k));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                });
            }
        });
    }
    ui.separator();
    let before = rec.clone();
    let mut after = rec;
    let mut ch = Ch::default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match &mut after {
        Rec::Mob(a) => mob_area(ui, a, idx, c, &mut ch, reqs),
        Rec::Npc(n) => npc(ui, doc, n, idx, c, &mut ch, reqs),
        Rec::Portal(p) => portal(ui, doc, p, idx, c, &mut ch, reqs),
        Rec::Bind(b) => bind(ui, doc, b, c, &mut ch),
        Rec::Region(r) => region(ui, doc, r, idx, c, &mut ch, reqs),
        Rec::B(p) => {
            ui.label(TEXT_SECTION_B);
            xz(ui, &mut ch, "p", p);
            section(ui, c, "height", "Height", false, |ui| y_field(ui, &mut ch, "py", "y", &mut p[1]));
        }
        Rec::A(a) => section_a(ui, a, c, &mut ch),
    });
    finish(doc, sec, idx, before, after, ch);
}

// ---------------------------------------------------------------------------------------------------------------- client half

/// The chain row under the heading of a record: whether the section is edited together with the client map.
fn chain_row(ui: &mut Ui, doc: &Doc, sec: SectionId) {
    use crate::doc::ClientSlot;
    let Some(k) = crate::pair::slot(sec) else { return };
    let pal = crate::theme::pal_ui(ui);
    match &doc.client {
        ClientSlot::Part(p) => {
            let linked = p.paired(&doc.map)[k];
            ui.horizontal(|ui| {
                if linked {
                    icons::mark(ui, Icon::Link, pal.link, 14.0, crate::help_text::tip("pair-linked"));
                    ui.label(egui::RichText::new("linked to the client map").weak().small());
                } else {
                    icons::mark(ui, Icon::Link, pal.weak.gamma_multiply(0.6), 14.0, crate::help_text::tip("pair-unpaired"));
                    ui.label(egui::RichText::new(format!("not linked: the client map has {} here", p.count(sec))).weak().small());
                }
            });
        }
        ClientSlot::ReadOnly(why) => {
            ui.label(egui::RichText::new(format!("client map read-only: {why}")).color(pal.warning).small());
        }
        ClientSlot::None => {}
    }
}

/// A text field for a client-only value. The text being typed is kept until Enter or a click elsewhere; the result is the new
/// text when it differs from `current`. Escape drops the typing.
fn client_text(ui: &mut Ui, key: impl std::hash::Hash + std::fmt::Debug, current: &str, enabled: bool) -> Option<String> {
    let id = egui::Id::unique(("client-text", key));
    let (mut text, base) = ui.data(|d| d.get_temp::<(String, String)>(id)).filter(|(_, b)| b == current).unwrap_or_else(|| (current.to_string(), current.to_string()));
    let r = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut text).id(id).desired_width(170.0));
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(id, (text.clone(), base)));
    }
    let escape = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escape {
        ui.data_mut(|d| d.remove_temp::<(String, String)>(id));
        return None;
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove_temp::<(String, String)>(id));
        if text != current {
            return Some(text);
        }
    }
    None
}

/// The destination name of the portal's client box (248 bytes in the `.wld`, shown by the client).
fn dest_name_row(ui: &mut Ui, doc: &mut Doc, idx: usize, reqs: &mut Vec<InspReq>) {
    let Some(part) = doc.part() else { return };
    let Some(current) = part.dest_name(idx) else { return };
    let typed = ui
        .horizontal(|ui| {
            row_label(ui, "client destination name", TIP_PORTAL_DEST_NAME);
            client_text(ui, ("dest-name", doc.uid, idx), &current, !doc.read_only)
        })
        .inner;
    let Some(text) = typed else { return };
    let Some(before) = part.wld.model.portals.get(idx).cloned() else { return };
    let mut after = before.clone();
    match part.cp.set_fixed(&mut after.dest_name, &text) {
        Ok(()) => {
            let cop = crate::pair::COp::Portal(idx, crate::pair::CKind::Replace { before, after });
            if let Err(e) = doc.commit_client(format!("rename portal #{idx} destination"), vec![cop]) {
                reqs.push(InspReq::Toast(e));
            }
        }
        Err(e) => reqs.push(InspReq::Toast(format!("destination name: {e}"))),
    }
}

/// The name of a region's client area (`world/<id>.txt`, line `idx`).
fn area_name_row(ui: &mut Ui, doc: &mut Doc, idx: usize, current: &str, reqs: &mut Vec<InspReq>) {
    let Some(part) = doc.part() else { return };
    let typed = ui
        .horizontal(|ui| {
            row_label(ui, "client name", TIP_REGION_NAME);
            client_text(ui, ("area-name", doc.uid, idx), current, !doc.read_only)
        })
        .inner;
    let Some(text) = typed else { return };
    match crate::pair::txt_with_name(&part.txt, idx as u32, &text) {
        Ok(after) => {
            let cop = crate::pair::COp::Txt { before: part.txt.clone(), after };
            if let Err(e) = doc.commit_client(format!("rename region #{idx}"), vec![cop]) {
                reqs.push(InspReq::Toast(e));
            }
        }
        Err(e) => reqs.push(InspReq::Toast(format!("name: {e}"))),
    }
}

/// x and z of a position on one row.
fn xz(ui: &mut Ui, ch: &mut Ch, key: &str, v: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        for (i, n) in [(0usize, "x"), (2usize, "z")] {
            ui.label(n);
            fnum(ui, ch, &format!("{key}{n}"), &mut v[i]);
        }
    });
}

/// One height value with its name.
fn y_field(ui: &mut Ui, ch: &mut Ch, key: &str, name: &str, v: &mut f32) {
    ui.horizontal(|ui| {
        row_label(ui, name, TIP_Y);
        fnum(ui, ch, key, v);
    });
}

/// The x and z of a box's two corners.
fn box_xz(ui: &mut Ui, ch: &mut Ch, key: &str, min: &mut [f32; 3], max: &mut [f32; 3]) {
    egui::Grid::new(format!("box-{key}")).num_columns(2).show(ui, |ui| {
        row_label(ui, "from (x, z)", TIP_FROM);
        xz(ui, ch, &format!("{key}min"), min);
        ui.end_row();
        row_label(ui, "to (x, z)", TIP_TO);
        xz(ui, ch, &format!("{key}max"), max);
        ui.end_row();
    });
}

/// The bottom and top y of a box.
fn box_y(ui: &mut Ui, ch: &mut Ch, key: &str, min: &mut [f32; 3], max: &mut [f32; 3]) {
    y_field(ui, ch, &format!("{key}miny"), "bottom y", &mut min[1]);
    y_field(ui, ch, &format!("{key}maxy"), "top y", &mut max[1]);
}

fn mob_area(ui: &mut Ui, a: &mut MobArea, idx: usize, c: &InspCtx, ch: &mut Ch, reqs: &mut Vec<InspReq>) {
    ui.label(TEXT_AREA);
    box_xz(ui, ch, "area", &mut a.min, &mut a.max);
    if let Some(l) = c.look.mean_level(a) {
        ui.label(format!("mean level {l:.1} (Monster.SData)"));
    }
    ui.add_space(4.0);
    ui.label("Monsters");
    let mut remove = None;
    let mut swap = None;
    let n = a.mobs.len();
    for (k, e) in a.mobs.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{k}"));
            unum(ui, ch, &format!("mob{k}"), &mut e.mob, 0..=65535);
            if ui.button("...").on_hover_text(TIP_PICK_MOB).clicked() {
                reqs.push(InspReq::PickMob { area: idx, entry: k });
            }
            ui.label("x").on_hover_text(TIP_COUNT);
            unum(ui, ch, &format!("cnt{k}"), &mut e.count, 1..=8);
            if ui.small_button("up").clicked() && k > 0 {
                swap = Some((k - 1, k));
            }
            if ui.small_button("dn").clicked() && k + 1 < n {
                swap = Some((k, k + 1));
            }
            if ui.small_button("x").on_hover_text(TIP_REMOVE_ENTRY).clicked() {
                remove = Some(k);
            }
        });
        let name = match c.look.mob(e.mob) {
            Some(m) => format!("    {} (lv {})", m.name.trim(), m.level),
            None => "    (not in Monster.SData)".to_string(),
        };
        ui.label(egui::RichText::new(name).weak().small());
    }
    if let Some(k) = remove {
        a.mobs.remove(k);
        ch.structural("remove-entry");
    }
    if let Some((i, j)) = swap {
        a.mobs.swap(i, j);
        ch.structural("swap-entry");
    }
    if ui.button("Add monster").clicked() {
        a.mobs.push(MobEntry { mob: 1, count: 1 });
        ch.structural("add-entry");
    }
    if ui.button("Save as template...").on_hover_text(TIP_TEMPLATE).clicked() {
        reqs.push(InspReq::SaveTemplate { area: idx });
    }
    section(ui, c, "height", "Height", false, |ui| box_y(ui, ch, "area", &mut a.min, &mut a.max));
}

fn npc(ui: &mut Ui, doc: &mut Doc, n: &mut NpcEntry, idx: usize, c: &InspCtx, ch: &mut Ch, reqs: &mut Vec<InspReq>) {
    egui::Grid::new("npc-grid").num_columns(2).show(ui, |ui| {
        row_label(ui, "type", TIP_NPC_TYPE);
        ui.horizontal(|ui| {
            unum(ui, ch, "kind", &mut n.kind, 0..=13);
            ui.label(names::npc_kind_name(n.kind));
        });
        ui.end_row();
        row_label(ui, "type id", TIP_NPC_ID);
        ui.horizontal(|ui| {
            unum(ui, ch, "tid", &mut n.type_id, 0..=65535);
            if ui.button("...").on_hover_text(TIP_PICK_NPC).clicked() {
                reqs.push(InspReq::PickNpc { npc: idx });
            }
        });
        ui.end_row();
    });
    match c.look.npc(n.kind, n.type_id) {
        Some(x) => {
            ui.label(egui::RichText::new(x.name.trim().to_string()).strong());
            if !x.welcome.trim().is_empty() {
                ui.label(egui::RichText::new(x.welcome.trim()).weak().small());
            }
        }
        None if n.kind == 0 => {
            ui.label(egui::RichText::new("placeholder entry").weak());
        }
        None => {
            ui.label(egui::RichText::new("not in NpcQuest.SData").weak());
        }
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Points");
        ui.label("?").on_hover_text(TIP_POINTS);
    });
    let mut remove = None;
    let mut insert_after = None;
    let np = n.points.len();
    for (k, p) in n.points.iter_mut().enumerate() {
        let selected = doc.sub == Some(k);
        ui.horizontal(|ui| {
            if ui.selectable_label(selected, format!("{}", k + 1)).on_hover_text(TIP_SELECT_POINT).clicked() {
                doc.sub = Some(k);
            }
            xz(ui, ch, &format!("p{k}"), &mut p.pos);
        });
        ui.horizontal(|ui| {
            ui.label("    facing");
            let mut deg = p.dir.to_degrees();
            if ui.add(DragValue::new(&mut deg).speed(0.5).max_decimals(2).suffix(" deg")).on_hover_text(TIP_DIR).changed() {
                p.dir = round2(deg as f64).to_radians();
                ch.mark(&format!("d{k}"));
            }
            if ui.small_button("+").on_hover_text(TIP_INSERT_POINT).clicked() {
                insert_after = Some(k);
            }
            if np > 1 && ui.small_button("x").on_hover_text(TIP_REMOVE_POINT).clicked() {
                remove = Some(k);
            }
        });
    }
    if let Some(k) = insert_after {
        let p = n.points[k];
        n.points.insert(k + 1, NpcPoint { pos: [p.pos[0] + 2.0, p.pos[1], p.pos[2]], dir: p.dir });
        doc.sub = Some(k + 1);
        ch.structural("insert-point");
    }
    if let Some(k) = remove {
        n.points.remove(k);
        doc.sub = None;
        ch.structural("remove-point");
    }
    if ui.button("Add path point").clicked() {
        let last = n.points.last().copied().unwrap_or(NpcPoint { pos: [0.0; 3], dir: 0.0 });
        n.points.push(NpcPoint { pos: [last.pos[0] + 2.0, last.pos[1], last.pos[2]], dir: last.dir });
        doc.sub = Some(n.points.len() - 1);
        ch.structural("add-point");
    }
    let path: Vec<[f32; 3]> = n.points.iter().map(|p| p.pos).collect();
    if let Some(t) = crate::measure::path_text(&path) {
        ui.label(egui::RichText::new(t).weak());
    }
    section(ui, c, "height", "Height", false, |ui| {
        for (k, p) in n.points.iter_mut().enumerate() {
            y_field(ui, ch, &format!("p{k}y"), &format!("point {} y", k + 1), &mut p.pos[1]);
        }
    });
    section(ui, c, "more-npc", "More", false, |ui| {
        for (k, p) in n.points.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                row_label(ui, &format!("point {} dir (radians)", k + 1), TIP_RAW_DIR);
                if ui.add(DragValue::new(&mut p.dir).speed(0.01).max_decimals(4)).changed() {
                    ch.mark(&format!("rd{k}"));
                }
            });
        }
    });
}

fn portal(ui: &mut Ui, doc: &mut Doc, p: &mut Portal, idx: usize, c: &InspCtx, ch: &mut Ch, reqs: &mut Vec<InspReq>) {
    ui.label(format!("Portal {idx}. The server checks the player is within {PORTAL_USE_RANGE} units, the faction and the levels."));
    egui::Grid::new("portal-grid").num_columns(2).show(ui, |ui| {
        row_label(ui, "position (x, z)", TIP_PORTAL_POS);
        xz(ui, ch, "pos", &mut p.pos);
        ui.end_row();
        row_label(ui, "country", TIP_PORTAL_COUNTRY);
        ui.horizontal(|ui| {
            let mut v = p.country;
            egui::ComboBox::from_id_salt("portal-country").width(110.0).selected_text(names::portal_country(v)).show_ui(ui, |ui| {
                for (n, label) in [(0u32, "any faction"), (1, "Light"), (2, "Fury")] {
                    ui.selectable_value(&mut v, n, label);
                }
            });
            if v != p.country {
                p.country = v;
                ch.mark("country");
            }
        });
        ui.end_row();
        row_label(ui, "level min / max", TIP_PORTAL_LEVELS);
        ui.horizontal(|ui| {
            let (mut a, mut b) = (p.level_min as u32, p.level_max as u32);
            let ca = ui.add(DragValue::new(&mut a).clamp_existing_to_range(false).range(0..=100)).changed();
            let cb = ui.add(DragValue::new(&mut b).clamp_existing_to_range(false).range(0..=100)).changed();
            if ca {
                p.level_min = a as u16;
                ch.mark("lvmin");
            }
            if cb {
                p.level_max = b as u16;
                ch.mark("lvmax");
            }
        });
        ui.end_row();
        row_label(ui, "destination map", TIP_PORTAL_MAP);
        ui.vertical(|ui| {
            let mut to = p.to_map;
            egui::ComboBox::from_id_salt("dest-map").selected_text(format!("{} {}", to, (c.map_name)(to))).width(200.0).show_ui(ui, |ui| {
                for (id, name) in c.maps {
                    ui.selectable_value(&mut to, *id, format!("{id} {name}"));
                }
            });
            if to != p.to_map {
                p.to_map = to;
                ch.structural("to-map");
            }
            unum(ui, ch, "to-map", &mut p.to_map, 0..=100000);
        });
        ui.end_row();
        row_label(ui, "destination (x, z)", TIP_PORTAL_DEST);
        ui.vertical(|ui| {
            xz(ui, ch, "to", &mut p.to_pos);
            if ui.button("Pick on map...").on_hover_text(TIP_PICK_DEST).clicked() {
                reqs.push(InspReq::PickDest { portal: idx });
            }
        });
        ui.end_row();
    });
    // the client's box: the tab's own client half when it has one (a paired move shows at once), else the client's file
    let box_centre = match doc.part() {
        Some(part) => part.wld.model.portals.get(idx).map(crate::pair::box_pos),
        None => c.look.assets.and_then(|a| a.wld.as_ref()).and_then(|w| w.portals.get(idx)).map(|w| w.svmap_pos()),
    };
    if let Some(ctr) = box_centre {
        let d = ((p.pos[0] - ctr[0]).powi(2) + (p.pos[2] - ctr[2]).powi(2)).sqrt();
        let text = format!("client portal box centre ({:.1}, {:.1}); distance {d:.1}", ctr[0], ctr[2]);
        if d > PORTAL_USE_RANGE {
            ui.colored_label(crate::theme::pal_ui(ui).error, format!("{text}. More than {PORTAL_USE_RANGE}: the server refuses the step."));
        } else {
            ui.label(egui::RichText::new(text).weak().small());
        }
    }
    dest_name_row(ui, doc, idx, reqs);
    ui.horizontal_wrapped(|ui| {
        if ui.button("Go to destination").on_hover_text(TIP_GO_DEST).clicked() {
            reqs.push(InspReq::GoToDest { portal: idx });
        }
        if ui.button("Drop destination to ground").on_hover_text(TIP_DROP_DEST).clicked() {
            reqs.push(InspReq::DropDestToGround { portal: idx });
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("Move up").on_hover_text(TIP_MOVE_PORTAL).clicked() {
            reqs.push(InspReq::MovePortal { idx, up: true });
        }
        if ui.button("Move down").clicked() {
            reqs.push(InspReq::MovePortal { idx, up: false });
        }
    });
    section(ui, c, "height", "Height", false, |ui| {
        y_field(ui, ch, "posy", "position y", &mut p.pos[1]);
        y_field(ui, ch, "toy", "destination y", &mut p.to_pos[1]);
    });
    section(ui, c, "more-portal", "More", false, |ui| {
        ui.horizontal(|ui| {
            row_label(ui, "raw country", TIP_PORTAL_COUNTRY);
            unum(ui, ch, "country", &mut p.country, 0..=100000);
        });
    });
}

fn bind(ui: &mut Ui, doc: &mut Doc, b: &mut Bind, c: &InspCtx, ch: &mut Ch) {
    ui.label(TEXT_BIND);
    egui::Grid::new("bind-grid").num_columns(2).show(ui, |ui| {
        row_label(ui, "country", TIP_BIND_COUNTRY);
        ui.horizontal(|ui| {
            unum(ui, ch, "country", &mut b.country, 0..=10);
            ui.label(names::country_name(b.country));
        });
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label("Boxes");
    let mut remove = None;
    for (k, x) in b.boxes.iter_mut().enumerate() {
        let selected = doc.sub == Some(k);
        ui.horizontal(|ui| {
            if ui.selectable_label(selected, format!("box {}", k + 1)).clicked() {
                doc.sub = Some(k);
            }
            if ui.small_button("x").on_hover_text(TIP_REMOVE_BOX).clicked() {
                remove = Some(k);
            }
        });
        box_xz(ui, ch, &format!("b{k}"), &mut x.min, &mut x.max);
    }
    if let Some(k) = remove {
        b.boxes.remove(k);
        doc.sub = None;
        ch.structural("remove-box");
    }
    if ui.button("Add box").clicked() {
        let base = b.boxes.last().copied().unwrap_or(Aabb { min: [0.0, 0.0, 0.0], max: [8.0, 2.0, 8.0] });
        b.boxes.push(Aabb { min: [base.min[0] + 4.0, base.min[1], base.min[2] + 4.0], max: [base.max[0] + 4.0, base.max[1], base.max[2] + 4.0] });
        doc.sub = Some(b.boxes.len() - 1);
        ch.structural("add-box");
    }
    section(ui, c, "height", "Height", false, |ui| {
        for (k, x) in b.boxes.iter_mut().enumerate() {
            box_y(ui, ch, &format!("b{k}"), &mut x.min, &mut x.max);
        }
    });
    section(ui, c, "more-bind", "More", false, |ui| {
        ui.horizontal(|ui| {
            row_label(ui, "attr", TIP_ATTR);
            unum(ui, ch, "attr", &mut b.attr, 0..=1000);
        });
    });
}

fn region(ui: &mut Ui, doc: &mut Doc, r: &mut Region, idx: usize, c: &InspCtx, ch: &mut Ch, reqs: &mut Vec<InspReq>) {
    // the name: from the tab's own client half when it has one, else the client's file
    let name = match doc.part() {
        Some(p) => p.names().get(idx).map(str::to_string),
        None => c.look.region_name(idx).map(str::to_string),
    };
    match &name {
        Some(n) if !n.is_empty() => ui.label(egui::RichText::new(n).strong()),
        _ => ui.label(egui::RichText::new("no name in the client's world/<id>.txt").weak()),
    };
    area_name_row(ui, doc, idx, name.as_deref().unwrap_or(""), reqs);
    ui.label(TEXT_REGION);
    box_xz(ui, ch, "reg", &mut r.min, &mut r.max);
    egui::Grid::new("region-grid").num_columns(2).show(ui, |ui| {
        row_label(ui, "kind", TIP_REGION_KIND);
        unum(ui, ch, "kind", &mut r.kind, 0..=100);
        ui.end_row();
    });
    section(ui, c, "height", "Height", false, |ui| box_y(ui, ch, "reg", &mut r.min, &mut r.max));
    section(ui, c, "more-region", "More", false, |ui| {
        egui::Grid::new("region-raw").num_columns(2).show(ui, |ui| {
            row_label(ui, "b", TIP_REGION_B);
            let mut b = r.b as u32;
            if ui.add(DragValue::new(&mut b).clamp_existing_to_range(false).range(0..=255)).changed() {
                r.b = b as u8;
                ch.mark("b");
            }
            ui.end_row();
            row_label(ui, "c", TIP_REGION_C);
            let mut cc = r.c as u32;
            if ui.add(DragValue::new(&mut cc).clamp_existing_to_range(false).range(0..=255)).changed() {
                r.c = cc as u8;
                ch.mark("c");
            }
            ui.end_row();
            row_label(ui, "d", TIP_REGION_D);
            let mut d = r.d as u32;
            if ui.add(DragValue::new(&mut d).clamp_existing_to_range(false).range(0..=65535)).changed() {
                r.d = d as u16;
                ch.mark("d");
            }
            ui.end_row();
        });
    });
}

fn section_a(ui: &mut Ui, a: &mut RawA, c: &InspCtx, ch: &mut Ch) {
    ui.label(TEXT_SECTION_A);
    xz(ui, ch, "p", &mut a.pos);
    section(ui, c, "height", "Height", false, |ui| y_field(ui, ch, "py", "y", &mut a.pos[1]));
    section(ui, c, "more-section-a", "More", false, |ui| {
        ui.horizontal(|ui| {
            unum(ui, ch, "a", &mut a.a, 0..=u32::MAX);
            unum(ui, ch, "b", &mut a.b, 0..=u32::MAX);
        });
    });
}


#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::{MobEntry, Svmap};

    /// Showing a record must never edit it: values outside the fields' usual ranges (level 999, count 20, ...) stay as they
    /// are until the user changes them.
    #[test]
    fn showing_out_of_range_values_does_not_edit() {
        let mut m = Svmap::new(64);
        m.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [5.0, 2.0, 5.0], mobs: vec![MobEntry { mob: 70000, count: 20 }, MobEntry { mob: 3, count: 0 }] });
        m.npcs.push(NpcEntry { kind: 99, type_id: 70000, points: vec![NpcPoint { pos: [1.0, 2.0, 3.0], dir: 7.37 }] });
        m.portals.push(Portal { pos: [1.0, 2.0, 3.0], country: 1007, level_min: 0, level_max: 999, to_map: 200, to_pos: [1.0, 2.0, 3.0] });
        m.binds.push(Bind { country: 77, attr: 5000, boxes: vec![Aabb { min: [0.0; 3], max: [1.0; 3] }] });
        m.regions.push(Region { min: [0.0; 3], max: [1.0; 3], kind: 500, b: 255, c: 255, d: 65535 });
        m.section_a.push(RawA { pos: [1.0, 2.0, 3.0], a: u32::MAX, b: 7 });
        let mut doc = Doc::new(1, Some(1), None, m, None);
        let ctx = egui::Context::default();
        let name = |id: u32| format!("Map {id}");
        let maps: Vec<(u32, String)> = Vec::new();
        let sections = [SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions, SectionId::SectionA];
        for (k, s) in sections.iter().enumerate() {
            doc.select_only(*s, 0);
            for f in 0..3 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(500.0, 900.0))), time: Some((k * 3 + f) as f64 * 0.1), ..Default::default() };
                let mut full = ctx.run_ui(input, |ui| {
                    let c = InspCtx { look: Lookups::none(), maps: &maps, map_name: &name, hints: true, fixes: &[], snap: 1.0, incoming: &[], sections: &std::cell::RefCell::new(Default::default()) };
                    show(ui, &mut doc, &c);
                });
                full.textures_delta.clear();
            }
            assert!(doc.undo.is_empty(), "showing {} edited it: {:?}", s.label(), doc.undo.iter().map(|s| s.label.clone()).collect::<Vec<_>>());
        }
    }

    /// A tiny rig: the inspector in a headless context, one frame at a time, with the text it drew.
    struct Rig {
        ctx: egui::Context,
        doc: Doc,
        t: f64,
        text: String,
        reqs: Vec<InspReq>,
    }

    impl Rig {
        fn new(doc: Doc) -> Rig {
            Rig { ctx: egui::Context::default(), doc, t: 0.0, text: String::new(), reqs: Vec::new() }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.t += 0.05;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(500.0, 900.0))), time: Some(self.t), events, ..Default::default() };
            let name = |id: u32| format!("Map {id}");
            let maps: Vec<(u32, String)> = Vec::new();
            let doc = &mut self.doc;
            let mut reqs = Vec::new();
            let mut full = self.ctx.run_ui(input, |ui| {
                let c = InspCtx { look: Lookups::none(), maps: &maps, map_name: &name, hints: true, fixes: &[], snap: 1.0, incoming: &[], sections: &std::cell::RefCell::new(Default::default()) };
                reqs = show(ui, doc, &c);
            });
            full.textures_delta.clear();
            fn walk(s: &egui::epaint::Shape, out: &mut String) {
                match s {
                    egui::epaint::Shape::Text(t) => {
                        out.push_str(t.galley.text());
                        out.push('\n');
                    }
                    egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                    _ => {}
                }
            }
            self.text.clear();
            for c in &full.shapes {
                walk(&c.shape, &mut self.text);
            }
            self.reqs = reqs;
        }

        fn type_into(&mut self, id: egui::Id, text: &str) {
            self.ctx.memory_mut(|m| m.request_focus(id));
            self.frame(vec![]);
            self.frame(vec![egui::Event::Text(text.to_string())]);
        }
    }

    fn area_with(count: u32) -> MobArea {
        MobArea { min: [0.0; 3], max: [4.0, 2.0, 4.0], mobs: vec![MobEntry { mob: 1, count }] }
    }

    #[test]
    fn batch_edit_mixed() {
        let mut m = Svmap::new(64);
        for n in 1..=3 {
            m.mob_areas.push(area_with(n));
        }
        let mut doc = Doc::new(1, Some(1), None, m, None);
        doc.selection = vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 1), (SectionId::MobAreas, 2)];
        assert_eq!(mob_counts_common(&doc.map, &[0, 1, 2]), None, "1, 2 and 3 differ");
        let mut rig = Rig::new(doc);
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert!(rig.text.contains("mixed"), "the empty field says so: {}", rig.text);
        assert!(rig.doc.undo.is_empty(), "showing the batch panel edits nothing");
        // typing 4 sets every count, as one undo step
        rig.type_into(batch_field_id("MobAreas-count"), "4");
        assert!(rig.doc.map.mob_areas.iter().all(|a| a.mobs[0].count == 4), "{:?}", rig.doc.map.mob_areas.iter().map(|a| a.mobs[0].count).collect::<Vec<_>>());
        assert_eq!(rig.doc.undo.len(), 1, "one step for all three");
        assert_eq!(mob_counts_common(&rig.doc.map, &[0, 1, 2]), Some(4));
        // the field shows the shared value now
        rig.ctx.memory_mut(|m| m.surrender_focus(batch_field_id("MobAreas-count")));
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert!(!rig.text.contains("mixed"), "{}", rig.text);
        // undo restores 1, 2, 3
        rig.doc.undo();
        assert_eq!(rig.doc.map.mob_areas.iter().map(|a| a.mobs[0].count).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn batch_edit_refuses_on_a_read_only_tab_and_other_sections_show_without_editing() {
        let mut m = Svmap::new(64);
        for n in 1..=3 {
            m.mob_areas.push(area_with(n));
        }
        let mut doc = Doc::new(1, Some(1), None, m, None);
        doc.read_only = true;
        doc.selection = vec![(SectionId::MobAreas, 0), (SectionId::MobAreas, 1)];
        let mut rig = Rig::new(doc);
        rig.frame(vec![]);
        rig.type_into(batch_field_id("MobAreas-count"), "7");
        assert!(rig.doc.undo.is_empty() && rig.doc.map.mob_areas[0].mobs[0].count == 1);
        assert!(rig.doc.refused > 0, "the refusal is counted for the toast");

        // every section's batch panel draws without touching the records
        let mut m = Svmap::new(64);
        for i in 0..3 {
            let f = i as f32 * 5.0;
            m.mob_areas.push(MobArea { min: [f, 0.0, f], max: [f + 4.0, 2.0, f + 4.0], mobs: vec![MobEntry { mob: 70000 + i, count: 20 }] });
            m.npcs.push(NpcEntry { kind: 99, type_id: 70000, points: vec![NpcPoint { pos: [f, 2.0, 3.0], dir: 7.37 }] });
            m.portals.push(Portal { pos: [f, 2.0, 3.0], country: 1007, level_min: 0, level_max: 999, to_map: 200, to_pos: [1.0, 2.0, 3.0] });
            m.binds.push(Bind { country: 77, attr: 5000, boxes: vec![Aabb { min: [0.0; 3], max: [1.0; 3] }] });
            m.regions.push(Region { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0], kind: 500, b: 255, c: 255, d: 65535 });
            m.section_a.push(RawA { pos: [f, 2.0, 3.0], a: u32::MAX, b: 7 });
            m.points_b.push([f, 1.0, 2.0]);
        }
        let mut doc = Doc::new(1, Some(1), None, m, None);
        for s in [SectionId::MobAreas, SectionId::Npcs, SectionId::Portals, SectionId::Binds, SectionId::Regions, SectionId::SectionA, SectionId::SectionB] {
            doc.selection = (0..3).map(|i| (s, i)).collect();
            let mut rig = Rig::new(std::mem::replace(&mut doc, Doc::new(2, None, None, Svmap::new(8), None)));
            rig.frame(vec![]);
            rig.frame(vec![]);
            assert!(rig.doc.undo.is_empty(), "showing the batch panel of {} edited it", s.label());
            doc = rig.doc;
        }
        // a mixed selection offers no batch fields, only the common actions
        doc.selection = vec![(SectionId::MobAreas, 0), (SectionId::Npcs, 0)];
        let mut rig = Rig::new(doc);
        rig.frame(vec![]);
        assert!(rig.text.contains("batch fields need records of one section"), "{}", rig.text);
    }

    #[test]
    fn an_npc_path_shows_its_length() {
        let mut m = Svmap::new(256);
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [10.0, 0.0, 10.0], dir: 0.0 }] });
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [0.0, 0.0, 0.0], dir: 0.0 }, NpcPoint { pos: [30.0, 9.0, 40.0], dir: 0.0 }, NpcPoint { pos: [30.0, 0.0, 90.0], dir: 0.0 }] });
        let mut doc = Doc::new(1, Some(1), None, m, None);
        doc.select_only(SectionId::Npcs, 0);
        let mut rig = Rig::new(doc);
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert!(!rig.text.contains("Path "), "a single point has no path: {}", rig.text);
        rig.doc.select_only(SectionId::Npcs, 1);
        rig.frame(vec![]);
        rig.frame(vec![]);
        assert!(rig.text.contains("Path 100.0 units (3 points)"), "{}", rig.text);
        assert!(rig.doc.undo.is_empty());
    }
}
