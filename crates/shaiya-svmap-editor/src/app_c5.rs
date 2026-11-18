//! The client side in the window: the Map.ini editor (Tools > Maps > Map.ini editor) and the new map wizard
//! (Tools > Maps > Add a map). Declared from `app.rs` with `#[path]`, so it shares the window's state. The pure parts are in
//! `mapini.rs`, `ini_doc.rs` and `newmap.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::RichText;

use super::{App, Dialog};
use crate::help_text;
use crate::ini_doc::{self, IniDoc, IniKind};
use crate::mapini::{self, MapIni};
use crate::newmap::{self, Known, Source, SourceInfo, Wanted};
use crate::settings::TargetServer;
use crate::view2d::Tool;

/// State of both windows and of "pick on map".
#[derive(Default)]
pub struct C5 {
    pub ini: Option<IniWin>,
    pub add: Option<AddWin>,
    pub pick: Option<IniPick>,
}

impl C5 {
    /// Names of the files with unsaved edits (the exit question lists them).
    pub fn dirty_names(&self) -> Vec<String> {
        self.ini.iter().flat_map(|w| w.docs.iter().filter(|d| d.dirty()).map(|d| d.kind.file_name().to_string())).collect()
    }
}

/// The rebirth position being picked on a map.
pub struct IniPick {
    pub zone: u32,
    pub key: &'static str,
    pub map: u32,
}

pub struct IniWin {
    pub dir: PathBuf,
    pub docs: Vec<IniDoc>,
    pub which: usize,
    pub zone: Option<u32>,
    pub filter: String,
    /// Typed text of the form's fields, by `<zone>/<key>`; dropped when the file changes under it.
    pub buf: HashMap<String, String>,
    pub buf_key: (usize, u64),
    /// `(is an error, text)` of the last action.
    pub msg: Option<(bool, String)>,
    pub add_id: u32,
    pub add_dungeon: bool,
    /// Closing was asked while there are unsaved edits.
    pub close_ask: bool,
}

pub struct AddWin {
    pub server_dir: PathBuf,
    pub map_dir: PathBuf,
    pub map_ini: Option<MapIni>,
    pub war_ini: Option<MapIni>,
    pub staged: Vec<u32>,
    pub id: u32,
    pub copy: bool,
    pub copy_id: Option<u32>,
    pub filter: String,
    pub size: u32,
    pub look: u32,
    pub minimap: bool,
    pub name: String,
    pub info: HashMap<u32, Result<SourceInfo, String>>,
    /// After Create: what was written (label and relative path), or what went wrong with what was written before it.
    pub done: Option<Done>,
}

pub struct Done {
    pub written: Vec<(String, String)>,
    pub error: Option<String>,
}

pub(super) enum IniAct {
    Set(u32, String, String),
    Remove(u32, String),
    Pick(u32, &'static str),
    AddZone(u32, &'static str),
    Save,
    SaveAll,
    Undo,
    Redo,
    Reload,
    File(usize),
    Discard,
}

/// The keys the form shows, in groups. Keys of a zone that are in none of them are shown last, as plain text rows.
const GROUPS: [(&str, &[&str]); 4] = [
    ("Type", &["MapType", "MapName"]),
    ("Instance", &["CreateType", "CreateTime", "CreateMinUser", "EnterMaxUser", "ExpireTime", "WarType"]),
    ("Rebirth", &["RebirthMapPos", "RebirthMapPos1", "RebirthMapPos2"]),
    ("Weather", &["WeatherState", "WeatherRate", "WeatherPower", "WeatherDelay", "WeatherNoneDelay"]),
];

const CREATE_TYPES: [&str; 6] = ["S", "F", "P", "G", "R", "H"];

fn is_rebirth(key: &str) -> bool {
    mapini::REBIRTH_KEYS.contains(&key)
}

/// The text of a rebirth value as the form shows its state.
fn rebirth_state(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if mapini::numbers(t, 4).len() == 4 {
        Some("ok")
    } else {
        Some("needs four numbers: map, x, y, z")
    }
}

impl App {
    // ---------------------------------------------------------------------------------------------------------- Map.ini editor

    /// The server data folder when it holds a `Map.ini`.
    pub(super) fn ini_dir(&self) -> Option<PathBuf> {
        self.server_cfg().map(|c| c.dir).filter(|d| d.join("Map.ini").is_file())
    }

    pub(super) fn open_ini_editor(&mut self) {
        let Some(dir) = self.ini_dir() else {
            self.toast("Set the server data folder first (Edit > Preferences > Folders). It must hold Map.ini.");
            return;
        };
        if self.c5.ini.is_some() {
            return;
        }
        let mut docs = Vec::new();
        match IniDoc::open(IniKind::Map, &dir.join("Map.ini")) {
            Ok(d) => docs.push(d),
            Err(e) => {
                self.dialog = Some(Dialog::Error(e));
                return;
            }
        }
        // MapWar.ini is edited when it exists
        if dir.join("MapWar.ini").is_file() {
            match IniDoc::open(IniKind::War, &dir.join("MapWar.ini")) {
                Ok(d) => docs.push(d),
                Err(e) => crate::log::error(format!("MapWar.ini is not edited: {e}")),
            }
        }
        let zone = docs[0].ini.zone_ids().first().copied();
        let add_id = docs[0].ini.map_count().unwrap_or(0);
        self.c5.ini = Some(IniWin { dir, docs, which: 0, zone, filter: String::new(), buf: HashMap::new(), buf_key: (0, 0), msg: None, add_id, add_dungeon: false, close_ask: false });
    }

    fn ini_known(&self, w: &IniWin) -> Known {
        let map = w.docs.iter().find(|d| d.kind == IniKind::Map).map(|d| &d.ini);
        let war = w.docs.iter().find(|d| d.kind == IniKind::War).map(|d| &d.ini);
        Known::from_parts(map, war, self.folder.iter().flat_map(|f| f.maps.keys().copied()), std::iter::empty())
    }

    /// Writes one file of the editor (`which`). The client folder is never written.
    pub(super) fn ini_save(&mut self, w: &mut IniWin, which: usize) {
        let backups = self.data_dir.as_ref().map(|d| (ini_doc::backup_dir(d, &w.dir), self.settings.backups_keep));
        let forbid = self.client_path.clone();
        let Some(doc) = w.docs.get_mut(which) else { return };
        match doc.save(backups.as_ref().map(|(p, k)| (p.as_path(), *k)), forbid.as_deref()) {
            Ok(s) => {
                crate::log::info(format!("saved {}", s.path.display()));
                w.msg = Some((false, format!("saved and verified {}", doc.kind.file_name())));
                self.safety.invalidate();
                self.mark_all_for_validation();
            }
            Err(e) => w.msg = Some((true, e)),
        }
    }

    /// The tab (and its 2D view) for `map` that a "pick on map" click needs; opens it from the map folder when it is closed.
    fn tab_for_pick(&mut self, map: u32) -> bool {
        if let Some(i) = self.docs.iter().position(|d| d.map_id == Some(map)) {
            self.active = i;
        } else if self.folder.as_ref().is_some_and(|f| f.maps.contains_key(&map)) {
            self.open_map_id(map);
        } else {
            self.toast(format!("map {map} is not in the open map folder; open its folder first"));
            return false;
        }
        self.vp = crate::view2d::VpState::default();
        self.mode3d = false;
        self.tool = Tool::Select;
        true
    }

    pub(super) fn finish_ini_pick(&mut self, pos: [f32; 3]) {
        let Some(p) = self.c5.pick.take() else { return };
        let value = format!("{}, {:.2}, {:.2}, {:.2}", p.map, pos[0], pos[1], pos[2]);
        match self.c5.ini.as_mut() {
            Some(w) => {
                let which = w.which;
                match w.docs.get_mut(which).map(|d| d.set_key(p.zone, p.key, &value)) {
                    Some(Ok(_)) => w.msg = Some((false, format!("{} of zone {} is now {value}", p.key, p.zone))),
                    Some(Err(e)) => w.msg = Some((true, e)),
                    None => {}
                }
            }
            None => self.toast("The Map.ini editor was closed; the position was not kept"),
        }
    }

    /// Runs what the form asked for.
    pub(super) fn ini_act(&mut self, w: &mut IniWin, act: IniAct) {
        let which = w.which;
        match act {
            IniAct::Set(zone, key, value) => {
                if let Some(d) = w.docs.get_mut(which) {
                    if let Err(e) = d.set_key(zone, &key, &value) {
                        w.msg = Some((true, e));
                    } else {
                        w.msg = None;
                    }
                }
            }
            IniAct::Remove(zone, key) => {
                if let Some(d) = w.docs.get_mut(which) {
                    if let Err(e) = d.remove_key(zone, &key) {
                        w.msg = Some((true, e));
                    }
                }
            }
            IniAct::Pick(zone, key) => {
                let Some(d) = w.docs.get(which) else { return };
                let map = d.own_value(zone, key).and_then(|t| mapini::numbers(&t, 1).first().copied()).filter(|m| *m >= 0.0).map_or(zone, |m| m as u32);
                if self.tab_for_pick(map) {
                    self.c5.pick = Some(IniPick { zone, key, map });
                }
            }
            IniAct::AddZone(id, map_type) => {
                let known = self.ini_known(w);
                let war_count = w.docs.iter().find(|d| d.kind == IniKind::War).and_then(|d| d.ini.map_count()).unwrap_or(0);
                let verdict = newmap::check_new_zone(w.docs[which].kind, self.settings.target_server, id, &known, war_count, self.folder.is_some());
                if let Some(why) = verdict.refuse {
                    w.msg = Some((true, why));
                    return;
                }
                if let Some(d) = w.docs.get_mut(which) {
                    match d.add_zone(id, &[("MapType", map_type)]) {
                        Ok(()) => {
                            w.zone = Some(id);
                            w.msg = Some((false, format!("zone {id} added; MapCount is {}", d.ini.map_count().unwrap_or(0))));
                            w.add_id = id + 1;
                        }
                        Err(e) => w.msg = Some((true, e)),
                    }
                }
            }
            IniAct::Save => self.ini_save(w, which),
            IniAct::SaveAll => {
                for i in 0..w.docs.len() {
                    if w.docs[i].dirty() {
                        self.ini_save(w, i);
                    }
                }
            }
            IniAct::Undo => {
                if let Some(d) = w.docs.get_mut(which) {
                    d.undo();
                }
            }
            IniAct::Redo => {
                if let Some(d) = w.docs.get_mut(which) {
                    d.redo();
                }
            }
            IniAct::Reload => {
                if let Some(d) = w.docs.get_mut(which) {
                    w.msg = Some(match d.reload() {
                        Ok(()) => (false, format!("{} read again", d.kind.file_name())),
                        Err(e) => (true, e),
                    });
                }
            }
            IniAct::File(i) => {
                if i < w.docs.len() {
                    w.which = i;
                    w.zone = w.docs[i].ini.zone_ids().first().copied();
                    w.add_id = match w.docs[i].kind {
                        IniKind::Map => w.docs[i].ini.map_count().unwrap_or(0),
                        IniKind::War => mapini::WAR_FIRST + w.docs[i].ini.map_count().unwrap_or(0),
                    };
                    w.msg = None;
                }
            }
            IniAct::Discard => {
                for d in &mut w.docs {
                    if d.dirty() {
                        let _ = d.reload();
                    }
                }
            }
        }
    }

    pub(super) fn ini_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.c5.ini.take() else { return };
        if w.docs.is_empty() {
            return;
        }
        let known = self.ini_known(&w);
        let target = self.settings.target_server;
        let folder_known = self.folder.is_some();
        let names: HashMap<u32, String> = w.docs[w.which].ini.zone_ids().into_iter().map(|id| (id, self.map_name(id))).collect();
        let picking = self.c5.pick.as_ref().map(|p| (p.zone, p.key));
        let key = (w.which, w.docs[w.which].version);
        if w.buf_key != key {
            w.buf.clear();
            w.buf_key = key;
        }
        let war_count = w.docs.iter().find(|d| d.kind == IniKind::War).and_then(|d| d.ini.map_count()).unwrap_or(0);
        let mut acts: Vec<IniAct> = Vec::new();
        let mut open = true;
        let mut close_now = false;
        {
            let IniWin { docs, which, zone, filter, buf, msg, add_id, add_dungeon, close_ask, .. } = &mut w;
            egui::Window::new("Map.ini editor").open(&mut open).default_size([760.0, 560.0]).resizable(true).show(ctx, |ui| {
                let pal = crate::theme::pal_ui(ui);
                ui.horizontal(|ui| {
                    for (i, d) in docs.iter().enumerate() {
                        let label = format!("{}{}", d.kind.file_name(), if d.dirty() { " *" } else { "" });
                        if ui.selectable_label(*which == i, label).clicked() && *which != i {
                            acts.push(IniAct::File(i));
                        }
                    }
                    ui.separator();
                    let d = &docs[*which];
                    let need = d.ini.needed_map_count(d.kind.is_war());
                    ui.label(format!("MapCount {}  (zones need {need})", d.ini.map_count().unwrap_or(0)));
                });
                if !*close_ask && docs.iter().any(|d| d.dirty()) {
                    ui.label(RichText::new("Unsaved changes. Save writes the file in the server data folder, after a copy of the old one.").color(pal.warning));
                }
                ui.horizontal(|ui| {
                    let d = &docs[*which];
                    if ui.add_enabled(d.dirty(), egui::Button::new("Save")).on_hover_text(help_text::tip("ini-save")).clicked() {
                        acts.push(IniAct::Save);
                    }
                    if ui.add_enabled(d.can_undo(), egui::Button::new("Undo")).on_hover_text(d.undo_label().unwrap_or("nothing to undo").to_string()).clicked() {
                        acts.push(IniAct::Undo);
                    }
                    if ui.add_enabled(d.can_redo(), egui::Button::new("Redo")).on_hover_text(d.redo_label().unwrap_or("nothing to redo").to_string()).clicked() {
                        acts.push(IniAct::Redo);
                    }
                    if ui.button("Reload").on_hover_text(help_text::tip("ini-reload")).clicked() {
                        acts.push(IniAct::Reload);
                    }
                });
                if let Some((bad, text)) = msg.as_ref() {
                    ui.colored_label(if *bad { pal.error } else { pal.link }, text);
                }
                if *close_ask {
                    ui.separator();
                    ui.label(RichText::new("Close with unsaved changes?").strong());
                    ui.horizontal(|ui| {
                        if ui.button("Save and close").clicked() {
                            acts.push(IniAct::SaveAll);
                            *close_ask = false;
                            close_now = true;
                        }
                        if ui.button("Discard and close").clicked() {
                            acts.push(IniAct::Discard);
                            *close_ask = false;
                            close_now = true;
                        }
                        if ui.button("Keep editing").clicked() {
                            *close_ask = false;
                        }
                    });
                }
                ui.separator();
                let doc = &docs[*which];
                let ids = doc.ini.zone_ids();
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.add(egui::TextEdit::singleline(filter).hint_text("filter by id or name").desired_width(200.0));
                        let f = filter.to_lowercase();
                        let list: Vec<u32> = ids.iter().copied().filter(|id| f.is_empty() || id.to_string().contains(&f) || names.get(id).is_some_and(|n| n.to_lowercase().contains(&f))).collect();
                        egui::ScrollArea::vertical().id_salt("ini-zones").max_height(360.0).auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                            for r in range {
                                let id = list[r];
                                let text = format!("{id:>3}  {}", names.get(&id).cloned().unwrap_or_default());
                                if ui.selectable_label(*zone == Some(id), RichText::new(text).monospace()).clicked() {
                                    *zone = Some(id);
                                }
                            }
                        });
                        ui.separator();
                        ui.label(RichText::new("Add a zone").strong());
                        let fixed = target == TargetServer::Original || doc.kind.is_war();
                        if fixed {
                            *add_id = match doc.kind {
                                IniKind::Map => doc.ini.map_count().unwrap_or(0),
                                IniKind::War => mapini::WAR_FIRST + war_count,
                            };
                            ui.label(format!("next id: {}", *add_id));
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("id");
                                ui.add(egui::DragValue::new(add_id).range(0..=newmap::AFTERGLOW_MAX_ID));
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.label("type");
                            ui.selectable_value(add_dungeon, false, "F field");
                            ui.selectable_value(add_dungeon, true, "D dungeon");
                        });
                        let v = newmap::check_new_zone(doc.kind, target, *add_id, &known, war_count, folder_known);
                        if let Some(why) = &v.refuse {
                            ui.colored_label(pal.error, why);
                        }
                        for wn in &v.warnings {
                            ui.colored_label(pal.warning, wn);
                        }
                        if ui.add_enabled(v.ok(), egui::Button::new(format!("Add zone {}", *add_id))).on_hover_text(help_text::tip("ini-add-zone")).clicked() {
                            acts.push(IniAct::AddZone(*add_id, if *add_dungeon { "D" } else { "F" }));
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| match *zone {
                        Some(z) if doc.ini.zone(z).is_some() => zone_form(ui, doc, z, buf, &mut acts, picking, &pal),
                        _ => {
                            ui.label("Pick a zone on the left.");
                        }
                    });
                });
            });
        }
        if close_now {
            open = false;
        }
        let mut again = true;
        for a in acts {
            self.ini_act(&mut w, a);
        }
        if !open {
            if w.docs.iter().any(|d| d.dirty()) {
                w.close_ask = true;
            } else {
                again = false;
            }
        }
        if w.close_ask && w.docs.iter().all(|d| !d.dirty()) {
            w.close_ask = false;
        }
        if again {
            self.c5.ini = Some(w);
        } else if self.c5.pick.is_some() {
            self.c5.pick = None;
        }
    }

    /// `--show ini[:zone]`: the Map.ini editor, on a zone.
    pub(super) fn demo_ini(&mut self, arg: &str) {
        self.open_ini_editor();
        if let (Some(w), Ok(z)) = (self.c5.ini.as_mut(), arg.parse::<u32>()) {
            w.zone = Some(z);
        }
    }

    /// `--show add-map[:source]`: the Add a map window with a client map picked as the source.
    pub(super) fn demo_add_map(&mut self, arg: &str) {
        self.open_add_map();
        if let (Some(w), Ok(id)) = (self.c5.add.as_mut(), arg.parse::<u32>()) {
            w.copy_id = Some(id);
        }
    }

    // ---------------------------------------------------------------------------------------------------------- Add a map

    pub(super) fn open_add_map(&mut self) {
        if self.client.is_none() {
            self.toast("Open the client folder first (File > Open client folder...)");
            return;
        }
        let Some(dir) = self.ini_dir() else {
            self.toast("Set the server data folder first (Edit > Preferences > Folders). It must hold Map.ini.");
            return;
        };
        let Some(map_dir) = self.settings.svmap_folder.clone().map(PathBuf::from).filter(|p| p.is_dir()) else {
            self.toast("Open the server map folder first (File > Open folder...)");
            return;
        };
        if self.c5.add.is_some() {
            return;
        }
        let read = |name: &str| std::fs::read(dir.join(name)).ok().map(|b| MapIni::parse(&b));
        let (map_ini, war_ini) = (read("Map.ini"), read("MapWar.ini"));
        let staged = self.settings.client_out.as_deref().map(|o| newmap::staged_ids(Path::new(o))).unwrap_or_default();
        let mut w = AddWin {
            server_dir: dir,
            map_dir,
            map_ini,
            war_ini,
            staged,
            id: 0,
            copy: true,
            copy_id: None,
            filter: String::new(),
            size: newmap::BLANK_SIZES[0],
            look: 0,
            minimap: true,
            name: String::new(),
            info: HashMap::new(),
            done: None,
        };
        let known = self.add_known(&w);
        w.id = self.add_default_id(&known);
        self.c5.add = Some(w);
    }

    fn add_known(&self, w: &AddWin) -> Known {
        let client: Vec<u32> = self.client.as_ref().map(|c| c.source.map_ids()).unwrap_or_default().into_iter().chain(w.staged.iter().copied()).collect();
        Known::from_parts(w.map_ini.as_ref(), w.war_ini.as_ref(), self.folder.iter().flat_map(|f| f.maps.keys().copied()), client)
    }

    fn add_default_id(&self, k: &Known) -> u32 {
        match self.settings.target_server {
            TargetServer::Original => newmap::next_original_id(k).unwrap_or(0),
            TargetServer::Afterglow => newmap::first_free_afterglow(k).unwrap_or(0),
        }
    }

    fn has_client_name(&self, id: u32) -> bool {
        self.map_name(id) != format!("Map {id}")
    }

    pub(super) fn add_info(&mut self, w: &mut AddWin, id: u32) {
        if let std::collections::hash_map::Entry::Vacant(slot) = w.info.entry(id) {
            let i = match self.client.as_ref() {
                Some(c) => newmap::source_info(c, id),
                None => Err("no client folder".to_string()),
            };
            slot.insert(i);
        }
    }

    /// What Create would write, as lines the window lists (relative names only).
    fn add_plan_lines(&self, w: &AddWin, dungeon: bool) -> Vec<String> {
        let id = w.id;
        let mut v = vec![format!("staging: data/world/{id}.wld"), format!("staging: data/world/{id}.txt")];
        if w.minimap && !dungeon {
            v.push(format!("staging: data/interface/MiniMap/{id}.tga"));
        }
        if !dungeon {
            v.push(format!("staging: data/world/Field/{id}/ (terrain light map and texture masks per chunk; the client needs them)"));
        }
        v.push(format!("staging: svmap-editor-new-map-{id}.txt (what is not done)"));
        v.push(format!("map folder: {id}.svmap"));
        let next = w.map_ini.as_ref().and_then(MapIni::map_count).unwrap_or(0).max(id + 1);
        v.push(format!("Map.ini: [SET_ZONE_{id}], MapCount {next}"));
        v
    }

    pub(super) fn add_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.c5.add.take() else { return };
        if let (true, Some(id)) = (w.copy, w.copy_id) {
            self.add_info(&mut w, id);
        }
        let known = self.add_known(&w);
        let target = self.settings.target_server;
        let verdict = newmap::check_new_id(target, w.id, &known);
        let has_name = self.has_client_name(w.id);
        let ids: Vec<u32> = self.client.as_ref().map(|c| c.source.map_ids()).unwrap_or_default();
        let filter = w.filter.to_lowercase();
        let list: Vec<(u32, String)> = ids.iter().map(|id| (*id, self.map_name(*id))).filter(|(id, name)| filter.is_empty() || name.to_lowercase().contains(&filter) || id.to_string().contains(&filter)).collect();
        let src_info = w.copy_id.and_then(|id| w.info.get(&id).cloned());
        let dungeon = w.copy && matches!(&src_info, Some(Ok(i)) if i.dungeon);
        let plan = self.add_plan_lines(&w, dungeon);
        let cannot = newmap::cannot_do(w.id, has_name, dungeon, target);
        let source_ok = if w.copy { matches!(&src_info, Some(Ok(_))) } else { true };
        let mut open = true;
        let mut create = false;
        let mut new_target: Option<TargetServer> = None;
        egui::Window::new("Add a map").open(&mut open).default_size([600.0, 820.0]).resizable(true).show(ctx, |ui| {
            let pal = crate::theme::pal_ui(ui);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label("Makes a whole new map: client map, area names, minimap, server map and a Map.ini zone. Client files go to the staging folder.");
                if let Some(d) = &w.done {
                    for (what, path) in &d.written {
                        ui.label(format!("written: {what} ({path})"));
                    }
                    match &d.error {
                        Some(e) => {
                            ui.colored_label(pal.error, e);
                            ui.label("The files listed here are new and stay. Delete them or fix the cause and try again.");
                        }
                        None => {
                            ui.colored_label(pal.link, "Done. The new map is open in a tab.");
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Server");
                    egui::ComboBox::from_id_salt("add-target").selected_text(target.label()).show_ui(ui, |ui| {
                        for t in TargetServer::ALL {
                            if ui.selectable_label(target == t, t.label()).clicked() && target != t {
                                new_target = Some(t);
                            }
                        }
                    });
                    ui.label(RichText::new(format!("Map.ini MapCount {}", known.map_count.unwrap_or(0))).weak());
                });
                ui.horizontal(|ui| {
                    ui.label("Map id");
                    match target {
                        TargetServer::Original => {
                            w.id = known.map_count.unwrap_or(0);
                            ui.label(format!("{} (the next id; the original server needs a map file for every lower id)", w.id));
                        }
                        TargetServer::Afterglow => {
                            ui.add(egui::DragValue::new(&mut w.id).range(0..=newmap::AFTERGLOW_MAX_ID));
                            if ui.small_button("first free").clicked() {
                                w.id = newmap::first_free_afterglow(&known).unwrap_or(w.id);
                            }
                        }
                    }
                });
                if let Some(why) = &verdict.refuse {
                    ui.colored_label(pal.error, why);
                }
                for x in &verdict.warnings {
                    ui.colored_label(pal.warning, x);
                }
                if has_name {
                    ui.label(format!("The client names this map: {}.", self.map_name(w.id)));
                } else {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut w.name).hint_text("optional").desired_width(240.0)).on_hover_text(help_text::tip("add-name"));
                    });
                    ui.label(RichText::new(format!("The client shows \"Map {}\" for this id. The editor writes your name only into the notes file. Adding it to the game is your choice.", w.id)).weak());
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut w.copy, true, "Copy a client map");
                    ui.selectable_value(&mut w.copy, false, "Blank field");
                });
                if w.copy {
                    ui.add(egui::TextEdit::singleline(&mut w.filter).hint_text("filter by id or name").desired_width(f32::INFINITY));
                    egui::ScrollArea::vertical().id_salt("add-list").max_height(170.0).auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                        for r in range {
                            let (id, name) = &list[r];
                            if ui.selectable_label(w.copy_id == Some(*id), RichText::new(format!("{id:>3}  {name}")).monospace()).clicked() {
                                w.copy_id = Some(*id);
                            }
                        }
                    });
                    match &src_info {
                        None => {
                            ui.label("Pick a map.");
                        }
                        Some(Err(e)) => {
                            ui.colored_label(pal.error, e);
                        }
                        Some(Ok(i)) => {
                            ui.label(format!("{}, size {}; {} are copied", if i.dungeon { "dungeon" } else { "field" }, i.size, i.counts));
                            if w.id == newmap::FORCED_SIZE_ID && i.size != newmap::FORCED_SIZE {
                                ui.colored_label(pal.warning, format!("Map {} is {} wide here. The client wants {}.", w.id, i.size, newmap::FORCED_SIZE));
                            }
                        }
                    }
                } else {
                    ui.horizontal(|ui| {
                        ui.label("size");
                        for s in newmap::BLANK_SIZES {
                            ui.selectable_value(&mut w.size, s, s.to_string());
                        }
                        ui.label("look from map");
                        ui.add(egui::DragValue::new(&mut w.look).range(0..=999)).on_hover_text(help_text::tip("add-look"));
                    });
                    ui.label(RichText::new("Flat ground at height 0. Textures, water, sky and fog come from the look map. No buildings, NPCs or portals.").weak());
                    if w.id == newmap::FORCED_SIZE_ID && w.size != newmap::FORCED_SIZE {
                        ui.colored_label(pal.warning, format!("The client wants {} for map {}.", newmap::FORCED_SIZE, w.id));
                    }
                }
                ui.checkbox(&mut w.minimap, "minimap picture (field maps)").on_hover_text(help_text::tip("add-minimap"));
                ui.separator();
                ui.label(RichText::new("Create writes").strong());
                for l in &plan {
                    ui.label(format!("  {l}"));
                }
                ui.label(RichText::new("The zone is written last, so the server never lists a map whose file is missing.").weak());
                ui.label(RichText::new("The editor cannot do").strong());
                for l in &cannot {
                    ui.label(format!("  {l}"));
                }
                ui.separator();
                if ui.add_enabled(verdict.ok() && source_ok && w.done.is_none(), egui::Button::new("Create")).clicked() {
                    create = true;
                }
            });
        });
        if let Some(t) = new_target {
            self.settings.target_server = t;
            self.safety.invalidate();
            let k = self.add_known(&w);
            w.id = self.add_default_id(&k);
            w.done = None;
        }
        if create {
            self.add_map_create(ctx, &mut w);
        }
        if open {
            self.c5.add = Some(w);
        }
    }

    /// Create: builds the map in memory, then writes it in order. The staging folder is asked for once.
    pub(super) fn add_map_create(&mut self, ctx: &egui::Context, w: &mut AddWin) {
        let target = self.settings.target_server;
        let known = self.add_known(w);
        let verdict = newmap::check_new_id(target, w.id, &known);
        if let Some(why) = verdict.refuse {
            w.done = Some(Done { written: Vec::new(), error: Some(why) });
            return;
        }
        let Some(rules) = self.client.clone() else { return };
        let source = if w.copy {
            let Some(id) = w.copy_id else {
                w.done = Some(Done { written: Vec::new(), error: Some("Pick the client map to copy.".to_string()) });
                return;
            };
            Source::Copy(id)
        } else {
            Source::Blank { size: w.size, look: w.look }
        };
        let Some(out) = self.ensure_client_out() else { return };
        let Some(client) = self.client_path.clone() else { return };
        let wanted = Wanted { id: w.id, source, minimap: w.minimap, name: w.name.clone(), target };
        let has_name = self.has_client_name(w.id);
        let built = match newmap::build(&rules, &wanted, has_name) {
            Ok(b) => b,
            Err(e) => {
                w.done = Some(Done { written: Vec::new(), error: Some(e) });
                return;
            }
        };
        let keep = self.settings.backups_keep;
        let dirs = newmap::Dirs {
            out: &out,
            client: &client,
            map_dir: &w.map_dir,
            server_dir: &w.server_dir,
            client_backups: self.data_dir.as_ref().map(|d| (crate::client_files::backup_dir(d, &out), keep)),
            server_backups: self.data_dir.as_ref().map(|d| (ini_doc::backup_dir(d, &w.server_dir), keep)),
        };
        let (report, error) = match newmap::execute(&built, &dirs) {
            Ok(r) => (r, None),
            Err((r, e)) => (r, Some(e)),
        };
        let rel = |p: &Path| -> String {
            if let Ok(r) = p.strip_prefix(&out) {
                format!("staging/{}", r.display()).replace('\\', "/")
            } else {
                p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            }
        };
        let written: Vec<(String, String)> = report.written.iter().map(|(what, p)| (what.clone(), rel(p))).collect();
        for (what, p) in &report.written {
            crate::log::info(format!("new map {}: wrote {what}: {}", w.id, p.display()));
        }
        if let Some(e) = &error {
            crate::log::error(format!("new map {}: {e}", w.id));
        }
        let svmap_written = report.written.iter().any(|(_, p)| p.extension().is_some_and(|x| x == "svmap"));
        if !report.written.is_empty() {
            // the client reads the staged files; the map folder and the server data changed
            self.client_out_changed(Some(out.clone()));
            let dir = w.map_dir.clone();
            self.open_folder(&dir, ctx);
            self.safety.invalidate();
            if let Some(d) = self.c5.ini.as_mut() {
                // unsaved edits stay: their next save then says the file changed on disk
                for doc in d.docs.iter_mut().filter(|doc| !doc.dirty()) {
                    let _ = doc.reload();
                }
            }
        }
        w.map_ini = std::fs::read(w.server_dir.join("Map.ini")).ok().map(|b| MapIni::parse(&b));
        w.staged = newmap::staged_ids(&out);
        if svmap_written && error.is_none() {
            self.open_map_id(w.id);
            self.toast(format!("new map {} made: {} files written", w.id, written.len()));
        }
        w.done = Some(Done { written, error });
    }
}

/// The form of one zone.
fn zone_form(ui: &mut egui::Ui, doc: &IniDoc, zone: u32, buf: &mut HashMap<String, String>, acts: &mut Vec<IniAct>, picking: Option<(u32, &'static str)>, pal: &crate::theme::Pal) {
    let kind = if doc.ini.zone_value(zone, "MapType").as_deref().map(str::trim) == Some("D") { "dungeon" } else { "field" };
    ui.label(RichText::new(format!("Zone {zone} ({kind})")).strong());
    ui.label(RichText::new("A key the zone leaves out shows the default of [DEFAULT_SET_ZONE] in grey. Enter or a click elsewhere sets the value.").weak());
    egui::ScrollArea::vertical().id_salt("ini-form").auto_shrink([false, false]).show(ui, |ui| {
        let mut shown: Vec<&str> = Vec::new();
        for (title, keys) in GROUPS {
            ui.add_space(4.0);
            ui.label(RichText::new(title).strong());
            for key in keys {
                shown.push(key);
                key_row(ui, doc, zone, key, buf, acts, picking, pal);
            }
        }
        let own: Vec<String> = doc.ini.zone(zone).map(|z| z.keys.iter().map(|k| k.name.clone()).collect()).unwrap_or_default();
        let others: Vec<String> = {
            let mut seen: Vec<String> = Vec::new();
            for k in own {
                if !shown.iter().any(|s| s.eq_ignore_ascii_case(&k)) && !seen.contains(&k) {
                    seen.push(k);
                }
            }
            seen
        };
        if !others.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Other keys of this zone").strong());
            for k in others {
                key_row(ui, doc, zone, &k, buf, acts, picking, pal);
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn key_row(ui: &mut egui::Ui, doc: &IniDoc, zone: u32, key: &str, buf: &mut HashMap<String, String>, acts: &mut Vec<IniAct>, picking: Option<(u32, &'static str)>, pal: &crate::theme::Pal) {
    let own = doc.own_value(zone, key);
    let default = doc.default_value(key);
    ui.horizontal(|ui| {
        let l = ui.add_sized([130.0, 18.0], egui::Label::new(key));
        let tip = help_text::ini_tip(key);
        if !tip.is_empty() {
            l.on_hover_text(tip);
        }
        let hint = match &default {
            Some(d) => format!("default: {d}"),
            None => "not set".to_string(),
        };
        match key {
            "MapType" | "CreateType" => {
                let options: &[&str] = if key == "MapType" { &["F", "D"] } else { &CREATE_TYPES };
                let cur = own.as_deref().map(str::trim).unwrap_or("");
                let shown = if cur.is_empty() { hint.clone() } else { cur.to_string() };
                egui::ComboBox::from_id_salt(format!("ini-{zone}-{key}")).selected_text(RichText::new(shown).color(if cur.is_empty() { pal.weak } else { pal.text })).show_ui(ui, |ui| {
                    for o in options {
                        if ui.selectable_label(cur == *o, *o).clicked() && cur != *o {
                            acts.push(IniAct::Set(zone, key.to_string(), (*o).to_string()));
                        }
                    }
                });
            }
            _ => {
                let id = format!("{zone}/{key}");
                let text = buf.entry(id).or_insert_with(|| own.clone().unwrap_or_default());
                let r = ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(if is_rebirth(key) { 190.0 } else { 150.0 }));
                if r.lost_focus() {
                    let want = text.trim().to_string();
                    let have = own.as_deref().map(str::trim).unwrap_or("").to_string();
                    if want != have {
                        if want.is_empty() {
                            acts.push(IniAct::Remove(zone, key.to_string()));
                        } else {
                            acts.push(IniAct::Set(zone, key.to_string(), want));
                        }
                    }
                }
                if is_rebirth(key) {
                    let key_static: &'static str = mapini::REBIRTH_KEYS.iter().find(|k| **k == key).copied().unwrap_or("RebirthMapPos");
                    let on = picking == Some((zone, key_static));
                    if ui.selectable_label(on, "pick on map").on_hover_text(help_text::tip("ini-pick")).clicked() {
                        acts.push(IniAct::Pick(zone, key_static));
                    }
                    if let Some(s) = rebirth_state(text) {
                        if s != "ok" {
                            ui.colored_label(pal.error, s);
                        }
                    }
                }
            }
        }
        if own.is_some() && ui.small_button("x").on_hover_text(help_text::tip("ini-remove")).clicked() {
            acts.push(IniAct::Remove(zone, key.to_string()));
        }
    });
}
