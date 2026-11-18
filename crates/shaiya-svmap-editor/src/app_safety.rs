//! Parts of the window: autosave and crash recovery, the Problems tab with
//! fixes, grouping and the folder scope, the Changes tab with the history and compare and revert, the changed-on-disk bar,
//! backups, Save all and the tab menu. Declared from `app.rs` (`#[path]`), so it shares the app's private state.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use egui::{RichText, TextureHandle, TextureOptions};
use shaiya_svmap::diff::DiffKind;
use shaiya_svmap::{Finding, SectionId, Severity, Svmap};

use super::{App, Dialog, Pending, PickKind, Picker};
use crate::layout::DockTab;
use crate::backups::{self, Backup};
use crate::compare::{self, Cmp, Row};
use crate::doc::{stamp_of, Doc};
use crate::edit::{self};
use crate::fixes::{self, FixEnv, FixKind};
use crate::help_text;
use crate::log;
use crate::names::{self, Lookups};
use crate::problems::{self, FolderResult, Source};
use crate::recovery;
use crate::settings::{CompareAgainst, ProblemsScope};
use crate::sync;
use crate::view2d::{self, Tool};

/// The target of a fix in progress: the map id, the (size, mtime) of the file it was made from and the sync target.
type FixTarget = (Option<u32>, (u64, u64), Arc<sync::Target>);

// ------------------------------------------------------------------------------------------------------------------ state

/// What one open document has in the recovery folder.
pub struct AutoState {
    /// The `edit_gen` the entry was written for.
    gen: u64,
    key: String,
    /// Whether this window wrote (or restored) the entry under `key`: only then may it remove it. An entry under the same key
    /// that this window does not own (work an earlier session left behind and the user kept for later, or another window's) stays.
    has_entry: bool,
    meta: Option<recovery::Meta>,
    /// An entry this tab took over under another key (restored from `new-<pid>-<uid>` or a parked `<key>-<stamp>`): removed once
    /// the tab's own entry is written, or when the tab is clean, saved or discarded.
    stale: Option<String>,
    /// The version of the client half whose `.wld` and `.txt` the entry holds (`ClientPart::version`). The 5 MB of a field map
    /// are written again only when the half changed: an edit of the svmap alone leaves them.
    client: Option<(u64, u64)>,
}

impl AutoState {
    fn new(key: String) -> AutoState {
        AutoState { gen: 0, key, has_entry: false, meta: None, stale: None, client: None }
    }

    /// Removes the entries this tab owns.
    fn remove_owned(&mut self, data: &Path) {
        if self.has_entry {
            recovery::remove_entry(data, &self.key);
        }
        if let Some(old) = self.stale.take() {
            recovery::remove_entry(data, &old);
        }
        self.has_entry = false;
        self.meta = None;
        self.client = None;
    }
}

pub struct RecoveryWin {
    pub entries: Vec<recovery::Entry>,
}

#[derive(Default)]
pub struct FolderCheck {
    rx: Option<Receiver<FolderResult>>,
    started_gens: Vec<(u64, u64)>,
    pub result: Option<FolderResult>,
    last_start: Option<Instant>,
    /// The server data the last check read, with its key (reused until the setting changes or Check again).
    server_cache: Option<(String, Arc<crate::server_data::ServerData>)>,
}

pub struct BackupRow {
    pub backup: Backup,
    /// What differs from the open map, per section.
    pub summary: String,
}

pub struct BackupWin {
    pub uid: u64,
    pub id: u32,
    pub dir: PathBuf,
    pub rows: Vec<BackupRow>,
}

/// Everything the Q2 features keep between frames.
pub struct Safety {
    autosave: HashMap<u64, AutoState>,
    pub(super) last_autosave: Option<Instant>,
    /// "autosave failed: ..." while the last attempt failed.
    pub autosave_error: Option<String>,
    pub recovery: Option<RecoveryWin>,
    pub cmp: HashMap<u64, Cmp>,
    cmp_tex: HashMap<u64, (u64, TextureHandle)>,
    fix_target: Option<FixTarget>,
    pub group_problems: bool,
    pub problems_filter: String,
    collapsed_rules: HashSet<String>,
    pub folder_check: FolderCheck,
    /// Tabs whose file changed on disk since the editor last looked (the yellow bar).
    pub disk_changed: HashSet<u64>,
    last_disk_check: Instant,
    save_queue: VecDeque<u64>,
    pub backup_win: Option<BackupWin>,
}

impl FolderCheck {
    /// The folder-wide check is running on its worker thread.
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }
}

impl Safety {
    pub fn new() -> Safety {
        Safety {
            autosave: HashMap::new(),
            last_autosave: None,
            autosave_error: None,
            recovery: None,
            cmp: HashMap::new(),
            cmp_tex: HashMap::new(),
            fix_target: None,
            group_problems: false,
            problems_filter: String::new(),
            collapsed_rules: HashSet::new(),
            folder_check: FolderCheck::default(),
            disk_changed: HashSet::new(),
            last_disk_check: Instant::now(),
            save_queue: VecDeque::new(),
            backup_win: None,
        }
    }

    /// Forget everything kept for a tab that was closed.
    pub fn forget(&mut self, uid: u64) {
        self.autosave.remove(&uid);
        self.cmp.remove(&uid);
        self.cmp_tex.remove(&uid);
        self.disk_changed.remove(&uid);
        self.save_queue.retain(|u| *u != uid);
    }

    /// The client or the folder changed: derived results are stale.
    pub fn invalidate(&mut self) {
        self.fix_target = None;
        self.folder_check.result = None;
        self.folder_check.rx = None;
        self.folder_check.server_cache = None;
    }
}

/// One row of the folder-wide Problems list.
#[derive(Clone, Copy)]
enum FolderRow {
    /// The "Server and client" header with its count.
    Head(usize),
    /// `result.cross[k]`.
    Cross(usize),
    /// A map's finding (`usize::MAX`: the file could not be read).
    Map(u32, usize),
}

/// What a tab's context menu asks for.
pub enum TabAct {
    Close(usize),
    CloseOthers(usize),
    CloseSaved,
    CopyPath(usize),
    Reveal(usize),
    OpenReadOnly(usize),
    OpenReference(usize),
    CompareReference(usize),
}

// ------------------------------------------------------------------------------------------------------------------ recovery

pub(super) fn meta_for(d: &Doc, now_ms: u64) -> recovery::Meta {
    recovery::Meta {
        path: d.path.as_ref().map(|p| p.display().to_string()),
        map_id: d.map_id,
        title: d.title(),
        base_stamp: d.stamp.as_ref().map(recovery::stamp_ms),
        saved_at_unix_ms: now_ms,
        steps: d.undo.len(),
        heartbeat_unix_ms: now_ms,
        client: d.client_dirty(),
    }
}

impl App {
    /// Writes the unsaved work of every dirty tab into the recovery folder (only the tabs that changed since their last entry and
    /// have been quiet for 2 s, unless `force`), refreshes the heartbeat of the entries this window owns and removes the entries
    /// of tabs that are clean again. Runs every `autosave_secs`; a no-op without a data folder or with autosave off.
    pub(super) fn autosave_tick(&mut self, force: bool) {
        let Some(data) = self.data_dir.clone() else { return };
        let secs = self.settings.autosave_secs;
        if secs == 0 {
            return;
        }
        let due = force || self.safety.last_autosave.is_none_or(|t| t.elapsed() >= Duration::from_secs(secs as u64));
        if !due {
            return;
        }
        self.safety.last_autosave = Some(Instant::now());
        let now = SystemTime::now();
        let now_ms = log::unix_ms(now);
        let mut error: Option<String> = None;
        for d in &self.docs {
            if d.read_only || self.terrain.stroking(d.uid) {
                // a terrain stroke in progress is no step yet: its tab is written once the stroke has ended
                continue;
            }
            let key = recovery::entry_key(d.path.as_deref(), d.uid);
            let st = self.safety.autosave.entry(d.uid).or_insert_with(|| AutoState::new(key.clone()));
            if st.has_entry && st.key != key {
                // the entry this tab owns has another key (a restored entry): it goes once the tab's own entry is written, so a
                // crash in between still leaves the work on disk
                if let Some(old) = st.stale.replace(st.key.clone()) {
                    recovery::remove_entry(&data, &old);
                }
                st.has_entry = false;
                st.meta = None;
                st.gen = 0;
                st.client = None;
            }
            st.key = key.clone();
            if !d.is_dirty() {
                st.remove_owned(&data);
                st.gen = d.edit_gen;
                continue;
            }
            let idle = d.edit_at.elapsed() >= Duration::from_secs(2);
            if st.gen != d.edit_gen && (force || idle) {
                if !st.has_entry {
                    recovery::park_existing(&data, &key, &log::utc_stamp(now));
                }
                let meta = meta_for(d, now_ms);
                // the client files first: the description that says they exist is written last
                let part = d.part().filter(|_| d.client_dirty());
                let version = part.map(|p| p.version());
                let client = match part {
                    _ if st.has_entry && st.client == version => Ok(()),
                    Some(p) => recovery::write_client(&data, &key, Some((&p.wld.model.to_bytes(), &p.txt))),
                    None => recovery::write_client(&data, &key, None),
                };
                let written = client.and_then(|()| recovery::write_entry(&data, &key, &d.map, &meta));
                match written {
                    Ok(()) => {
                        st.client = version;
                        st.gen = d.edit_gen;
                        st.has_entry = true;
                        st.meta = Some(meta);
                        if let Some(old) = st.stale.take() {
                            recovery::remove_entry(&data, &old);
                        }
                    }
                    Err(e) => error = Some(format!("{}: {e}", d.title())),
                }
            } else if let (true, Some(m)) = (st.has_entry, st.meta.as_mut()) {
                m.heartbeat_unix_ms = now_ms;
                if let Err(e) = recovery::write_meta(&data, &key, m) {
                    error = Some(format!("{}: {e}", d.title()));
                }
            }
        }
        if error.is_some() != self.safety.autosave_error.is_some() {
            match &error {
                Some(e) => log::error(format!("autosave failed: {e}")),
                None => log::info("autosave works again"),
            }
        }
        self.safety.autosave_error = error;
    }

    /// Removes the recovery entry of a tab (it was saved, discarded or closed). Only an entry this window wrote or restored goes:
    /// one under the same file name that an earlier session left (kept with "Later") or another window owns stays.
    pub(super) fn recovery_remove(&mut self, uid: u64) {
        let Some(data) = self.data_dir.clone() else { return };
        if let Some(st) = self.safety.autosave.get_mut(&uid) {
            st.remove_owned(&data);
        }
    }

    /// A clean exit (or a confirmed discard): the entries this window owns go.
    pub(super) fn recovery_remove_all(&mut self) {
        let Some(data) = self.data_dir.clone() else { return };
        for st in self.safety.autosave.values_mut() {
            st.remove_owned(&data);
        }
    }

    /// On start: entries of earlier sessions (and not of another running window) are offered back.
    pub(super) fn scan_recovery(&mut self) {
        let Some(data) = self.data_dir.clone() else { return };
        let now_ms = log::unix_ms(SystemTime::now());
        let entries = recovery::scan(&data, now_ms, recovery::live_window_ms(self.settings.autosave_secs));
        if !entries.is_empty() {
            log::info(format!("{} recovery entr{} found", entries.len(), if entries.len() == 1 { "y" } else { "ies" }));
            self.safety.recovery = Some(RecoveryWin { entries });
        }
    }

    /// Keys of the entries on offer (a session tab for the same file is left to the recovery dialog).
    pub(super) fn recovery_keys(&self) -> HashSet<String> {
        self.safety.recovery.as_ref().map(|r| r.entries.iter().map(|e| e.key.clone()).collect()).unwrap_or_default()
    }

    /// Opens a recovered entry as a tab: the recovered map, marked as modified, with one undo step "recovered edits" back to the
    /// file as it is on disk. The entry stays until the tab is saved or discarded (autosave takes it over).
    pub(super) fn restore_recovered(&mut self, e: recovery::Entry) -> Result<(), String> {
        let map = e.map.map_err(|e| format!("damaged: {e}"))?;
        // the client half of the work (C2): needs the client folder, since it is applied to the map as the client holds it
        let client = match &e.client {
            Some((wld, txt)) => {
                if self.client.is_none() {
                    return Err("this work includes client map edits: open the client folder first, then restore it".to_string());
                }
                let raw = shaiya_clientdata::wld_raw::WldRaw::parse(wld, "the recovered .wld").map_err(|e| format!("damaged client map: {e}"))?;
                Some(crate::pair::Lists { portals: raw.portals, npcs: raw.npcs, areas: raw.areas, zones: raw.zone_groups, txt: txt.clone(), places: [raw.building, raw.shape, raw.tree, raw.dungeon], terrain: raw.terrain.map(|t| t.heights).unwrap_or_default() })
            }
            None => None,
        };
        let path = e.meta.path.as_ref().map(PathBuf::from);
        if let Some(p) = &path {
            if let Some(i) = self.docs.iter().position(|d| d.path.as_deref() == Some(p.as_path()) && !d.read_only) {
                if self.docs[i].is_dirty() {
                    return Err(format!("{} is open with unsaved changes: close it first", p.display()));
                }
                self.close_tab(i);
            }
        }
        let disk = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| shaiya_svmap::read(&b).ok());
        let uid = self.new_uid();
        let d = Doc::from_recovery(uid, e.meta.map_id, path.clone(), map, disk);
        log::info(format!("restored {} from the recovery folder", d.title()));
        // the entry on disk is this tab's now: the next autosave rewrites it in place instead of parking it (its client files too:
        // (0, 0) is no part's version)
        self.safety.autosave.insert(uid, AutoState { gen: 0, key: e.key.clone(), has_entry: true, meta: Some(e.meta.clone()), stale: None, client: e.client.as_ref().map(|_| (0, 0)) });
        self.docs.push(d);
        if let Some(after) = client {
            self.attach_client_now(uid);
            let i = self.docs.len() - 1;
            let before = self.docs[i].part().map(|p| p.lists());
            let done = match before {
                Some(before) if before != after => self.docs[i].commit_client("recovered client edits", vec![crate::pair::COp::Lists { before: Box::new(before), after: Box::new(after) }]),
                Some(_) => Ok(()),
                None => Err("the client has no editable map for this one, so its client edits cannot be restored".to_string()),
            };
            if let Err(why) = done {
                self.docs.pop();
                self.safety.autosave.remove(&uid);
                return Err(why);
            }
        }
        self.active = self.docs.len() - 1;
        self.vp = view2d::VpState::default();
        self.r5.showing = super::Showing::Map;
        Ok(())
    }

    /// The modal "Unsaved work from an earlier session".
    pub(super) fn recovery_window(&mut self, ctx: &egui::Context) {
        let Some(mut w) = self.safety.recovery.take() else { return };
        enum Act {
            Restore(usize),
            Discard(usize),
            Later(usize),
            All,
            LaterAll,
        }
        let mut act: Option<Act> = None;
        let now = SystemTime::now();
        egui::Modal::new(egui::Id::unique("recovery-modal")).show(ctx, |ui| {
            ui.set_max_width(580.0);
            ui.heading("Unsaved work from an earlier session");
            ui.label("The editor was closed (or stopped) while these maps had changes that were not saved. The real files have not been touched.");
            ui.add_space(6.0);
            for (i, e) in w.entries.iter().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&e.meta.title).strong());
                        if let Some(p) = &e.meta.path {
                            ui.label(RichText::new(p).weak().small());
                        }
                    });
                    match &e.map {
                        Ok(_) => {
                            let note = match e.file_state() {
                                recovery::FileState::NoFile => "a map that was never saved",
                                recovery::FileState::Unchanged => "",
                                recovery::FileState::Changed => "the file on disk changed since",
                                recovery::FileState::Gone => "the file is gone",
                            };
                            ui.label(format!("saved {} · {} step{}{}", log::utc_ago(e.saved_at(), now), e.meta.steps, if e.meta.steps == 1 { "" } else { "s" }, if note.is_empty() { String::new() } else { format!(" · {note}") }));
                        }
                        Err(why) => {
                            ui.label(RichText::new(format!("damaged: {why}")).color(crate::theme::pal_ui(ui).error));
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(e.map.is_ok(), egui::Button::new("Restore")).on_hover_text("Open it as a tab with the unsaved changes; Undo goes back to the file on disk").clicked() {
                            act = Some(Act::Restore(i));
                        }
                        if ui.button("Discard").on_hover_text("Delete the recovered copy").clicked() {
                            act = Some(Act::Discard(i));
                        }
                        if e.map.is_ok() && ui.button("Later").on_hover_text("Keep it for the next start").clicked() {
                            act = Some(Act::Later(i));
                        }
                    });
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Restore all").clicked() {
                    act = Some(Act::All);
                }
                if ui.button("Later").on_hover_text("Close this window and ask again at the next start").clicked() {
                    act = Some(Act::LaterAll);
                }
            });
        });
        let data = self.data_dir.clone();
        match act {
            None => {}
            Some(Act::Restore(i)) if i < w.entries.len() => {
                let e = w.entries.remove(i);
                if let Err(msg) = self.restore_recovered(e) {
                    self.toast(msg);
                }
            }
            Some(Act::Discard(i)) if i < w.entries.len() => {
                let e = w.entries.remove(i);
                if let Some(d) = &data {
                    recovery::remove_entry(d, &e.key);
                }
            }
            Some(Act::Later(i)) if i < w.entries.len() => {
                w.entries.remove(i);
            }
            Some(Act::All) => {
                let mut failed = Vec::new();
                for e in std::mem::take(&mut w.entries) {
                    if e.map.is_ok() {
                        if let Err(msg) = self.restore_recovered(e) {
                            failed.push(msg);
                        }
                    } else {
                        w.entries.push(e);
                    }
                }
                if let Some(m) = failed.first() {
                    self.toast(m.clone());
                }
            }
            Some(Act::LaterAll) => w.entries.clear(),
            Some(_) => {}
        }
        if !w.entries.is_empty() {
            self.safety.recovery = Some(w);
        }
    }

    // -------------------------------------------------------------------------------------------------------------- fixes

    /// Everything a fix may look at, for the active map. `&mut` because the client's `.wld` as a sync target is cached.
    pub(super) fn fix_env(&mut self) -> FixEnv {
        let map_id = self.docs.get(self.active).and_then(|d| d.map_id);
        // the tab's own client model (with its unsaved edits) when it has one, else the client's file
        let (wld, rev) = match self.docs.get(self.active).and_then(|d| d.part()) {
            Some(p) => (Some(p.wld_arc()), p.version()),
            None => (self.current_assets().and_then(|a| a.wld.clone()), (0, 0)),
        };
        let target = match &wld {
            Some(w) => match &self.safety.fix_target {
                Some((id, r, t)) if *id == map_id && *r == rev => Some(t.clone()),
                _ => {
                    let t = Arc::new(sync::target_from_wld(w));
                    self.safety.fix_target = Some((map_id, rev, t.clone()));
                    Some(t)
                }
            },
            None => None,
        };
        let folder = self.folder.as_ref().map(|f| f.maps.iter().filter_map(|(id, m)| m.svmap.clone().map(|s| (*id, s))).collect()).unwrap_or_default();
        let view_centre = match self.docs.get(self.active) {
            Some(d) => match self.v3.states.get(&d.uid).filter(|_| self.mode3d) {
                Some(st) => [st.cam.focus[0], st.cam.focus[2]],
                None => [d.view.cx, d.view.cz],
            },
            None => [0.0, 0.0],
        };
        FixEnv { folder, wld, target, view_centre, ground: Box::new(self.ground_fn()) }
    }

    /// Runs fix number `idx` of the finding `f` on the active map.
    pub(super) fn apply_fix(&mut self, f: &Finding, idx: usize) {
        let env = self.fix_env();
        let fix = {
            let Some(d) = self.docs.get(self.active) else { return };
            let cx = env.ctx(&d.map, d.map_id);
            fixes::fixes_for(f, &cx).into_iter().nth(idx)
        };
        let Some(fix) = fix else { return };
        let Some(d) = self.docs.get_mut(self.active) else { return };
        match fix.kind {
            FixKind::Ops(ops) => d.commit(fixes::step_label(&fix.title, f), ops),
            FixKind::PickMob { area, entry } => {
                let uid = d.uid;
                self.picker = Some(Picker { uid, kind: PickKind::Mob { area, entry }, query: String::new() });
            }
            FixKind::PickNpc { npc } => {
                let uid = d.uid;
                self.picker = Some(Picker { uid, kind: PickKind::Npc { npc }, query: String::new() });
            }
            FixKind::PickDest { portal } => self.start_pick_dest(portal),
            FixKind::OpenSync => self.open_sync(),
            FixKind::Delete((s, i)) => {
                d.select_only(s, i);
                self.request(Pending::Delete);
            }
        }
    }

    /// "Fix all N" of one rule on the active map: works the ops out, then asks.
    pub(super) fn request_fix_all(&mut self, code: &str) {
        let env = self.fix_env();
        let (ops, count, label) = {
            let Some(d) = self.docs.get(self.active) else { return };
            let cx = env.ctx(&d.map, d.map_id);
            let hits: Vec<&Finding> = d.findings.iter().filter(|f| f.code == code).collect();
            let title = help_text::rule(code).map_or(code, |r| r.title);
            (fixes::fix_all(&hits, &cx), hits.len(), format!("fix all: {} ({})", title.to_lowercase(), hits.len()))
        };
        if ops.is_empty() {
            self.toast(format!("nothing to fix for {code}"));
            return;
        }
        self.request(Pending::FixAll { ops, label, code: code.to_string(), count });
    }

    // -------------------------------------------------------------------------------------------------------------- problems

    /// The header controls of the Problems tab (severity switches, grouping, the text filter, the scope).
    pub(super) fn problems_controls(&mut self, ui: &mut egui::Ui) {
        let (e, w, i) = self.docs.get(self.active).map_or((0, 0, 0), |d| shaiya_svmap::tally(&d.findings));
        for (k, (letter, n)) in [("E", e), ("W", w), ("I", i)].into_iter().enumerate() {
            let on = self.show_sev[k];
            if ui.add(egui::Button::selectable(on, format!("{letter} {n}"))).on_hover_text(help_text::tip("problems-severity")).clicked() {
                self.show_sev[k] = !on;
            }
        }
        let g = self.safety.group_problems;
        if ui.add(egui::Button::selectable(g, "group")).on_hover_text(help_text::tip("problems-group")).clicked() {
            self.safety.group_problems = !g;
        }
        ui.add(egui::TextEdit::singleline(&mut self.safety.problems_filter).hint_text("filter").desired_width(110.0)).on_hover_text(help_text::tip("problems-filter"));
        ui.selectable_value(&mut self.settings.problems_scope, ProblemsScope::Map, "this map").on_hover_text(help_text::tip("problems-scope"));
        ui.selectable_value(&mut self.settings.problems_scope, ProblemsScope::Folder, "all maps").on_hover_text(help_text::tip("problems-scope"));
    }

    pub(super) fn problems_body(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() {
            ui.label("No map open.");
            return;
        }
        match self.settings.problems_scope {
            ProblemsScope::Map => self.problems_map_body(ui),
            ProblemsScope::Folder => self.problems_folder_body(ui),
        }
    }

    fn problems_map_body(&mut self, ui: &mut egui::Ui) {
        enum PRow<'a> {
            Header(&'a str, usize, bool),
            Item(usize),
        }
        enum PAct {
            Select(usize),
            Fix(usize, usize),
            FixAll(String),
            Toggle(String),
        }
        let env = self.fix_env();
        let show = self.show_sev;
        let group = self.safety.group_problems;
        let filter = self.safety.problems_filter.clone();
        let collapsed = self.safety.collapsed_rules.clone();
        let mut acts: Vec<PAct> = Vec::new();
        let mut hover: Option<(SectionId, usize)> = None;
        {
            let d = &self.docs[self.active];
            let cx = env.ctx(&d.map, d.map_id);
            let groups = problems::grouped(&d.findings, show, &filter, group);
            let mut rows: Vec<PRow> = Vec::new();
            for (code, idxs) in &groups {
                if group {
                    let all = d.findings.iter().filter(|f| f.code == *code).count();
                    rows.push(PRow::Header(code, all, collapsed.contains(*code)));
                    if collapsed.contains(*code) {
                        continue;
                    }
                }
                rows.extend(idxs.iter().map(|i| PRow::Item(*i)));
            }
            if rows.is_empty() {
                let hidden = d.findings.len();
                ui.label(if hidden == 0 {
                    "No problems found.".to_string()
                } else {
                    format!("No problems of the kinds that are switched on or that match the filter. Hidden: {hidden}.")
                });
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 20.0, rows.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for r in range {
                    match &rows[r] {
                        PRow::Header(code, all, is_collapsed) => {
                            ui.horizontal(|ui| {
                                let arrow = if *is_collapsed { ">" } else { "v" };
                                if ui.selectable_label(false, RichText::new(format!("{arrow} {code} · {all}")).strong()).on_hover_text(help_text::rule_tip(code)).clicked() {
                                    acts.push(PAct::Toggle(code.to_string()));
                                }
                                if let Some(h) = help_text::rule(code) {
                                    ui.label(RichText::new(h.title).weak());
                                }
                                if fixes::is_bulk(code) && ui.small_button(format!("Fix all {all}")).on_hover_text("Fix every finding of this rule on this map, as one undo step (asks first)").clicked() {
                                    acts.push(PAct::FixAll(code.to_string()));
                                }
                            });
                        }
                        PRow::Item(i) => {
                            let f = &d.findings[*i];
                            let fx = fixes::fixes_for(f, &cx);
                            ui.horizontal(|ui| {
                                match fx.len() {
                                    0 => {}
                                    1 => {
                                        if ui.small_button("Fix").on_hover_text(format!("{}\n{}", fx[0].title, help_text::tip("problems-fix"))).clicked() {
                                            acts.push(PAct::Fix(*i, 0));
                                        }
                                    }
                                    _ => {
                                        let r = crate::icons::text_chevron_button(ui, "Fix");
                                        egui::Popup::menu(&r).show(|ui| {
                                            for (k, x) in fx.iter().enumerate() {
                                                if ui.button(&x.title).clicked() {
                                                    acts.push(PAct::Fix(*i, k));
                                                    ui.close();
                                                }
                                            }
                                        });
                                    }
                                }
                                let col = crate::theme::severity(&crate::theme::pal_ui(ui), f.severity);
                                let idx = f.index.map_or(String::new(), |i| format!(" #{i}"));
                                let text = RichText::new(format!("{:<7} {}{idx}: {}", f.severity.label(), f.section.label(), f.message)).color(col).monospace();
                                let resp = ui.selectable_label(false, text).on_hover_text(help_text::rule_tip(f.code));
                                if resp.hovered() {
                                    hover = f.index.map(|i| (f.section, i));
                                }
                                if resp.clicked() {
                                    acts.push(PAct::Select(*i));
                                }
                            });
                        }
                    }
                }
            });
        }
        if hover.is_some() {
            self.hover_rec = hover;
        }
        for a in acts {
            match a {
                PAct::Select(i) => {
                    if let Some(f) = self.docs.get(self.active).and_then(|d| d.findings.get(i)).cloned() {
                        self.select_finding(&f);
                    }
                }
                PAct::Fix(i, k) => {
                    if let Some(f) = self.docs.get(self.active).and_then(|d| d.findings.get(i)).cloned() {
                        self.apply_fix(&f, k);
                    }
                }
                PAct::FixAll(code) => self.request_fix_all(&code),
                PAct::Toggle(code) => {
                    if !self.safety.collapsed_rules.remove(&code) {
                        self.safety.collapsed_rules.insert(code);
                    }
                }
            }
        }
    }

    fn problems_folder_body(&mut self, ui: &mut egui::Ui) {
        if self.folder.is_none() {
            ui.label("No map folder is open: open one to check every map.");
            return;
        }
        let mut recheck = false;
        let running = self.safety.folder_check.rx.is_some();
        ui.horizontal(|ui| match (&self.safety.folder_check.result, running) {
            (Some(r), _) => {
                let (e, w, i) = r.totals();
                ui.label(format!("all {} maps: errors {e} · warnings {w} · info {i}", r.maps.len()));
                if running {
                    ui.spinner();
                } else if ui.small_button("Check again").clicked() {
                    recheck = true;
                }
            }
            (None, true) => {
                ui.spinner();
                ui.label("checking every map...");
            }
            (None, false) => {
                ui.label("The check starts in a moment.");
            }
        });
        if recheck {
            self.safety.folder_check.result = None;
            self.safety.folder_check.server_cache = None;
        }
        let show = self.show_sev;
        let filter = self.safety.problems_filter.to_lowercase();
        let mut list: Vec<FolderRow> = Vec::new();
        if let Some(r) = &self.safety.folder_check.result {
            let sev_of = |s: Severity| match s {
                Severity::Error => 0,
                Severity::Warning => 1,
                Severity::Info => 2,
            };
            // "Server and client": the folder-level findings first, one group under one header
            let mut cross: Vec<FolderRow> = Vec::new();
            for (k, c) in r.cross.iter().enumerate() {
                let name = c.map.map_or(String::new(), |m| self.map_name(m));
                let hay = format!("{} {name} {} {} {}", c.map.map_or(String::new(), |m| m.to_string()), c.finding.code, c.finding.message, c.finding.section.label()).to_lowercase();
                if show[sev_of(c.finding.severity)] && filter.split_whitespace().all(|w| hay.contains(w)) {
                    cross.push(FolderRow::Cross(k));
                }
            }
            if !cross.is_empty() {
                list.push(FolderRow::Head(cross.len()));
                list.extend(cross);
            }
            for (id, rep) in &r.maps {
                if rep.read_error.is_some() {
                    list.push(FolderRow::Map(*id, usize::MAX));
                }
                for (k, f) in rep.findings.iter().enumerate() {
                    let hay = format!("{id} {} {} {} {}", self.map_name(*id), f.code, f.message, f.section.label()).to_lowercase();
                    if show[sev_of(f.severity)] && filter.split_whitespace().all(|w| hay.contains(w)) {
                        list.push(FolderRow::Map(*id, k));
                    }
                }
            }
        }
        if self.safety.folder_check.result.is_some() && self.server_cfg().is_none() {
            ui.label(RichText::new("No server data folder: the Server and client checks are off. Set it in Preferences > Folders.").weak().small());
        }
        let mut open: Option<(u32, Option<Finding>)> = None;
        if let Some(r) = &self.safety.folder_check.result {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                for row in range {
                    match list[row] {
                        FolderRow::Head(n) => {
                            ui.label(RichText::new(format!("Server and client \u{b7} {n}")).strong());
                        }
                        FolderRow::Cross(k) => {
                            let c = &r.cross[k];
                            let col = crate::theme::severity(&crate::theme::pal_ui(ui), c.finding.severity);
                            let who = c.map.map_or("   ".to_string(), |m| format!("{m:>3}"));
                            let name = c.map.map_or(String::new(), |m| self.map_name(m));
                            let idx = c.finding.index.map_or(String::new(), |i| format!(" #{i}"));
                            let sect = if c.finding.index.is_some() { format!("{}{idx}: ", c.finding.section.label()) } else { String::new() };
                            let text = RichText::new(format!("{who} {name}  {:<7} [{}] {sect}{}", c.finding.severity.label(), c.finding.code, c.finding.message)).color(col).monospace();
                            let resp = ui.selectable_label(false, text).on_hover_text(help_text::rule_tip(c.finding.code));
                            if resp.clicked() {
                                if let Some(m) = c.map.filter(|m| self.folder.as_ref().is_some_and(|f| f.maps.contains_key(m))) {
                                    open = Some((m, c.finding.index.map(|_| c.finding.clone())));
                                }
                            }
                        }
                        FolderRow::Map(id, k) => {
                            let name = self.map_name(id);
                            if k == usize::MAX {
                                let why = r.maps[&id].read_error.clone().unwrap_or_default();
                                if ui.selectable_label(false, RichText::new(format!("{id:>3} {name}: cannot be read: {why}")).color(crate::theme::pal_ui(ui).error).monospace()).clicked() {
                                    open = Some((id, None));
                                }
                                continue;
                            }
                            let f = &r.maps[&id].findings[k];
                            let col = crate::theme::severity(&crate::theme::pal_ui(ui), f.severity);
                            let idx = f.index.map_or(String::new(), |i| format!(" #{i}"));
                            let text = RichText::new(format!("{id:>3} {name}  {:<7} {}{idx}: {}", f.severity.label(), f.section.label(), f.message)).color(col).monospace();
                            if ui.selectable_label(false, text).on_hover_text(help_text::rule_tip(f.code)).clicked() {
                                open = Some((id, Some(f.clone())));
                            }
                        }
                    }
                }
            });
        } else {
            ui.add_space(4.0);
        }
        if let Some((id, f)) = open {
            self.open_map_id(id);
            if let Some(f) = f {
                if self.docs.get(self.active).and_then(|d| d.map_id) == Some(id) {
                    self.select_finding(&f);
                }
            }
        }
    }

    /// Starts the folder-wide check when the scope asks for it and nothing is running; picks up a finished one.
    pub(super) fn folder_check_tick(&mut self, ctx: &egui::Context) {
        let fc = &mut self.safety.folder_check;
        let got = fc.rx.as_ref().map(|rx| rx.try_recv());
        match got {
            Some(Ok(mut r)) => {
                r.gens = std::mem::take(&mut fc.started_gens);
                fc.server_cache = r.server.clone();
                fc.result = Some(r);
                fc.rx = None;
                ctx.request_repaint();
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => fc.rx = None,
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => {}
        }
        // the map list shows the badges as soon as a result exists, but only the "all maps" scope asks for one
        if self.settings.problems_scope != ProblemsScope::Folder || self.folder.as_ref().is_none_or(|f| f.rx.is_some()) {
            return;
        }
        let gens: Vec<(u64, u64)> = self.docs.iter().filter(|d| !d.read_only).map(|d| (d.uid, d.edit_gen)).collect();
        let stale = match &self.safety.folder_check.result {
            None => true,
            Some(r) => r.gens != gens,
        };
        let fc = &mut self.safety.folder_check;
        if !stale || fc.rx.is_some() || fc.last_start.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.start_folder_check(ctx, gens);
    }

    /// The server data folder for the cross-file checks: the `server_data` setting, else the parent of the svmap folder when it
    /// holds `Map.ini`. None when neither exists (the "Server and client" rules are off).
    pub(super) fn server_cfg(&self) -> Option<problems::ServerCfg> {
        let folder = self.settings.svmap_folder.as_deref().map(Path::new);
        let dir = crate::server_data::resolve_dir(self.settings.server_data.as_deref(), folder)?;
        let afterglow = self.settings.afterglow_defs_path.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from);
        Some(problems::ServerCfg { dir, afterglow, target: self.settings.target_server })
    }

    fn start_folder_check(&mut self, ctx: &egui::Context, gens: Vec<(u64, u64)>) {
        let Some(folder) = &self.folder else { return };
        let mut sources: Vec<(u32, Source)> = Vec::new();
        for (id, m) in &folder.maps {
            let live = self.docs.iter().find(|d| !d.read_only && d.path.as_deref() == Some(m.path.as_path()));
            sources.push((*id, match live {
                Some(d) => Source::Live(Arc::new(d.map.clone())),
                None => Source::File(m.path.clone()),
            }));
        }
        let rules = self.client.clone();
        let cfg = self.server_cfg();
        let cached = self.safety.folder_check.server_cache.clone();
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let r = problems::check_folder(sources, rules.as_deref(), cfg.as_ref(), cached);
            let _ = tx.send(r);
            ctx2.request_repaint();
        });
        let fc = &mut self.safety.folder_check;
        fc.rx = Some(rx);
        fc.started_gens = gens;
        fc.last_start = Some(Instant::now());
    }

    // -------------------------------------------------------------------------------------------------------------- changes

    /// The folder a comparison reads from: the reference folder, or the other folder of Compare folders (which falls back to the
    /// reference folder).
    pub(super) fn compare_folder(&self, a: CompareAgainst) -> Option<PathBuf> {
        match a {
            CompareAgainst::Reference => self.settings.reference_folder.as_deref().map(PathBuf::from),
            CompareAgainst::Other => self.settings.other_folder.as_deref().or(self.settings.reference_folder.as_deref()).map(PathBuf::from),
            _ => None,
        }
    }

    /// Keeps the compare cache of the active tab current (only when something shows it).
    pub(super) fn compare_update(&mut self) {
        let want = self.dock_showing(DockTab::Changes) || self.settings.layers.compare;
        let Some(d) = self.docs.get(self.active) else { return };
        if !want {
            return;
        }
        let folder = self.compare_folder(self.settings.compare_against);
        self.safety.cmp.entry(d.uid).or_default().update(d, self.settings.compare_against, folder.as_deref());
    }

    /// The purple picture of the blocked cells that differ, for the viewport (None when the layer is off or nothing differs).
    pub(super) fn compare_texture(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        if !self.settings.layers.compare {
            return None;
        }
        let d = self.docs.get(self.active)?;
        let (uid, size) = (d.uid, d.map.size as usize);
        let cmp = self.safety.cmp.get(&uid)?;
        let runs = cmp.cell_runs.as_ref().filter(|r| !r.is_empty());
        let Some(runs) = runs else {
            self.safety.cmp_tex.remove(&uid);
            return None;
        };
        if let Some((rev, h)) = self.safety.cmp_tex.get(&uid) {
            if *rev == cmp.rev {
                return Some(h.clone());
            }
        }
        let mut rgba = vec![0u8; size * size * 4];
        for r in runs {
            for cell in r.start..r.start + r.len {
                let (x, z) = (cell as usize % size, cell as usize / size);
                let o = ((size - 1 - z) * size + x) * 4;
                if o + 4 <= rgba.len() {
                    rgba[o..o + 4].copy_from_slice(&[170, 80, 230, 150]);
                }
            }
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba);
        let opts = TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: None };
        let h = ctx.load_texture("compare-cells", img, opts);
        let rev = cmp.rev;
        self.safety.cmp_tex.insert(uid, (rev, h.clone()));
        Some(h)
    }

    pub(super) fn changes_body(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() {
            ui.label("No map open.");
            return;
        }
        self.compare_update();
        let mut jump: Option<usize> = None;
        ui.columns(2, |cols| {
            // ---- history: the steps in the order they were made, the redo steps greyed below; a click goes there
            {
                let ui = &mut cols[0];
                let d = &self.docs[self.active];
                ui.label(RichText::new("History").strong()).on_hover_text(help_text::tip("history"));
                // what the client half of the tab changed since it was read or saved
                if d.client_dirty() {
                    if let Some(p) = d.part() {
                        for line in p.changes() {
                            ui.label(RichText::new(line).weak().small());
                        }
                    }
                }
                egui::ScrollArea::vertical().id_salt("hist").auto_shrink([false, false]).show(ui, |ui| {
                    if d.undo.is_empty() && d.redo.is_empty() {
                        ui.label("No edits since the file was opened or saved.");
                        return;
                    }
                    let ago = |s: &crate::doc::Step| log::ago_text(s.at.elapsed().as_secs());
                    if ui.selectable_label(d.undo.is_empty(), "  0  the file as opened or saved").clicked() {
                        jump = Some(0);
                    }
                    for (k, s) in d.undo.iter().enumerate() {
                        let now = k + 1 == d.undo.len();
                        let text = format!("{:>3}  {}  ·  {}{}", k + 1, s.label, ago(s), if now { "   " } else { "" });
                        let r = ui.selectable_label(now, text);
                        if now {
                            // a painted marker for the step the map is at
                            let c = crate::theme::pal_ui(ui).text;
                            crate::icons::paint(ui.painter(), egui::Rect::from_center_size(egui::pos2(r.rect.right() - 14.0, r.rect.center().y), egui::Vec2::splat(12.0)), crate::icons::Icon::Marker, c);
                        }
                        if r.clicked() {
                            jump = Some(k + 1);
                        }
                    }
                    for j in 0..d.redo.len() {
                        let s = &d.redo[d.redo.len() - 1 - j];
                        let depth = d.undo.len() + 1 + j;
                        if ui.selectable_label(false, RichText::new(format!("{depth:>3}  {}  ·  undone", s.label)).weak()).clicked() {
                            jump = Some(depth);
                        }
                    }
                });
            }
            // ---- compare
            self.changes_compare(&mut cols[1]);
        });
        if let Some(depth) = jump {
            if let Some(d) = self.docs.get_mut(self.active) {
                d.jump_to(depth);
            }
        }
    }
}

/// A click in the compare column of the Changes tab.
pub(super) enum CAct {
    Show(Row),
    Revert(Row),
    Section(SectionId),
    Cells,
    Whole,
    Against(CompareAgainst),
}

impl App {
    /// The right column of the Changes tab: what to compare against, the rows, Show and Revert.
    pub(super) fn changes_compare(&mut self, ui: &mut egui::Ui) {
        let mut acts: Vec<CAct> = Vec::new();
        let mut hover: Option<(SectionId, usize)> = None;
        let uid = self.docs[self.active].uid;
        let against = self.settings.compare_against;
        // against the other folder the buttons read Take: the other folder's records come in
        let take = against == CompareAgainst::Other;
        let word = if take { "Take" } else { "Revert" };
        // against: the three sources, each with the reason when it cannot be used
        ui.horizontal_wrapped(|ui| {
            ui.label("against").on_hover_text(help_text::tip("compare-against"));
            for a in CompareAgainst::ALL {
                let folder = self.compare_folder(a);
                let ok = compare::source_of(&self.docs[self.active], a, folder.as_deref());
                let resp = ui.add_enabled(ok.is_ok(), egui::Button::selectable(against == a, a.label()));
                let resp = match &ok {
                    Err(why) => resp.on_disabled_hover_text(*why),
                    Ok(_) => resp.on_hover_text(help_text::tip("compare-against")),
                };
                if resp.clicked() {
                    acts.push(CAct::Against(a));
                }
            }
            ui.checkbox(&mut self.settings.layers.compare, "show on the map").on_hover_text(help_text::LAYERS.iter().find(|l| l.key == "compare").map_or("", |l| l.tip));
        });
        let read_only = self.docs[self.active].read_only;
        let assets = self.current_assets();
        let rules = self.client.clone();
        let sys = self.sysmsg.clone();
        let map_name = move |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        if let Some(cmp) = self.safety.cmp.get(&uid) {
            let d = &self.docs[self.active];
            if let Some(e) = &cmp.error {
                ui.label(RichText::new(e).color(crate::theme::pal_ui(ui).warning));
            } else if let Some(reference) = cmp.reference_map(d) {
                let look = Lookups { rules: rules.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
                let total = cmp.rows.len() + (cmp.cells() > 0) as usize;
                if !cmp.any() {
                    ui.label(format!("No difference against the {}.", against.short()));
                } else {
                    ui.label(RichText::new(format!("Against the {}: {total} change(s)", against.short())).strong());
                }
                if against == CompareAgainst::Other && cmp.any() && !read_only && cmp.size_differs.is_none() && ui.button("Take whole map").on_hover_text("Make this map equal to the other folder's copy (one undo step)").clicked() {
                    acts.push(CAct::Whole);
                }
                egui::ScrollArea::vertical().id_salt("diff").auto_shrink([false, false]).show(ui, |ui| {
                    if let Some((a, b)) = cmp.size_differs {
                        ui.label(RichText::new(format!("size: {a} → {b} (the size cannot be edited, so this cannot be reverted)")).weak());
                    }
                    if cmp.cells() > 0 {
                        ui.horizontal(|ui| {
                            ui.label(format!("blocked cells: {} cells differ", cmp.cells()));
                            if ui.add_enabled(!read_only, egui::Button::new(format!("{word} cells")).small()).on_hover_text(if take { "Bring the other folder's blocked cells in (one undo step)" } else { "Put the reference's blocked cells back (one undo step)" }).clicked() {
                                acts.push(CAct::Cells);
                            }
                        });
                    }
                    let mut last: Option<SectionId> = None;
                    for r in &cmp.rows {
                        if last != Some(r.section) {
                            last = Some(r.section);
                            let n = cmp.rows.iter().filter(|x| x.section == r.section).count();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{} ({n})", r.section.label())).strong());
                                if ui.add_enabled(!read_only, egui::Button::new(format!("{word} section")).small()).on_hover_text(if take { "Make this whole section equal to the other folder's (one undo step)" } else { "Make this whole section equal to the reference's again (one undo step)" }).clicked() {
                                    acts.push(CAct::Section(r.section));
                                }
                            });
                        }
                        let (rr, cr) = (r.ref_index.and_then(|i| edit::get(reference, r.section, i)), edit::get(&d.map, r.section, r.cur_index));
                        let text = match r.kind {
                            DiffKind::Changed => {
                                let lbl = names::label(&look, &d.map, r.section, r.cur_index, &map_name);
                                let what = match (&rr, &cr) {
                                    (Some(a), Some(b)) => compare::move_text(a, b),
                                    _ => String::new(),
                                };
                                format!("changed #{}: {lbl}  ({what})", r.cur_index)
                            }
                            DiffKind::Removed => format!("removed #{}: {}", r.ref_index.unwrap_or(0), names::label(&look, reference, r.section, r.ref_index.unwrap_or(0), &map_name)),
                            DiffKind::Added => format!("added #{}: {}", r.cur_index, names::label(&look, &d.map, r.section, r.cur_index, &map_name)),
                        };
                        let row = ui.horizontal(|ui| {
                            if ui.small_button("Show").on_hover_text("Select it and frame it (a removed record is framed where the reference has it)").clicked() {
                                acts.push(CAct::Show(*r));
                            }
                            if ui.add_enabled(!read_only, egui::Button::new(word).small()).on_hover_text(if take { "Make this record equal to the other folder's (one undo step)" } else { "Put this one back the way the reference has it (one undo step)" }).clicked() {
                                acts.push(CAct::Revert(*r));
                            }
                            ui.add(egui::Label::new(RichText::new(text).monospace()).truncate());
                        });
                        if row.response.hovered() && r.kind != DiffKind::Removed {
                            hover = Some((r.section, r.cur_index));
                        }
                    }
                });
            }
        }
        if hover.is_some() {
            self.hover_rec = hover;
        }
        for a in acts {
            self.compare_act(uid, take, a);
        }
    }

    /// One click of the Changes tab's compare column (the tests call it too).
    pub(super) fn compare_act(&mut self, uid: u64, take: bool, a: CAct) {
        {
            match a {
                CAct::Against(x) => {
                    self.settings.compare_against = x;
                    if x != CompareAgainst::Saved {
                        self.settings.layers.compare = true;
                    }
                }
                CAct::Show(r) => self.compare_show(uid, r),
                CAct::Revert(r) => {
                    let Some(cmp) = self.safety.cmp.get(&uid) else { return };
                    let d = &self.docs[self.active];
                    let Some(reference) = cmp.reference_map(d) else { return };
                    if let Some((label, op)) = compare::revert_row(reference, &d.map, &r) {
                        let label = if take { label.replacen("revert", "take", 1) } else { label };
                        self.request(Pending::Revert { ops: vec![op], label });
                    }
                }
                CAct::Section(s) => {
                    let Some(cmp) = self.safety.cmp.get(&uid) else { return };
                    let d = &self.docs[self.active];
                    let Some(reference) = cmp.reference_map(d) else { return };
                    let ops = compare::revert_section(reference, &d.map, s);
                    if !ops.is_empty() {
                        self.request(Pending::Revert { ops, label: format!("{} section {}", if take { "take" } else { "revert" }, s.label()) });
                    }
                }
                CAct::Cells => {
                    let Some(cmp) = self.safety.cmp.get(&uid) else { return };
                    let Some(grid) = self.docs.get(self.active).and_then(|d| cmp.reference_map(d)).map(|m| m.blocked.clone()) else { return };
                    if let Some(d) = self.docs.get_mut(self.active) {
                        d.commit_blocked(if take { "take blocked cells" } else { "revert blocked cells" }, &grid);
                    }
                }
                CAct::Whole => {
                    let Some(cmp) = self.safety.cmp.get(&uid) else { return };
                    let d = &self.docs[self.active];
                    let Some(other) = cmp.reference_map(d) else { return };
                    match edit::diff_ops(&d.map, other) {
                        Some(ops) if !ops.is_empty() => self.request(Pending::Revert { ops, label: "take whole map".to_string() }),
                        Some(_) => {}
                        None => self.toast("the sizes differ: the size cannot be edited"),
                    }
                }
            }
        }
    }

    /// Show on a Changes row: select the record and frame it; a removed record is framed where the reference has it.
    fn compare_show(&mut self, uid: u64, r: Row) {
        let size = self.vp.size;
        let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { return };
        match r.kind {
            DiffKind::Added | DiffKind::Changed => {
                let d = &mut self.docs[i];
                if r.cur_index < edit::count(&d.map, r.section) {
                    d.select_only(r.section, r.cur_index);
                }
                self.frame_selection();
            }
            DiffKind::Removed => {
                let b = {
                    let d = &self.docs[i];
                    let Some(cmp) = self.safety.cmp.get(&uid) else { return };
                    let Some(reference) = cmp.reference_map(d) else { return };
                    r.ref_index.and_then(|k| edit::get(reference, r.section, k)).map(|rec| edit::bounds_xz(&rec))
                };
                if let Some(b) = b {
                    view2d::frame_box(&mut self.docs[i], size, b);
                }
            }
        }
    }

    // -------------------------------------------------------------------------------------------------------------- disk changes

    /// Every 2 s: which tabs' files changed on disk since the editor last looked (the yellow bar).
    pub(super) fn disk_check(&mut self, force: bool) {
        if !force && self.safety.last_disk_check.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.safety.last_disk_check = Instant::now();
        self.safety.disk_changed = self.docs.iter().filter(|d| d.path.as_deref().is_some_and(|p| stamp_of(p) != d.seen)).map(|d| d.uid).collect();
    }

    /// The yellow bar above the viewport of a tab whose file changed on disk.
    pub(super) fn disk_bar(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.docs.get(self.active) else { return };
        if !self.safety.disk_changed.contains(&d.uid) {
            return;
        }
        let uid = d.uid;
        let name = d.path.as_ref().and_then(|p| p.file_name()).map_or(d.title(), |n| n.to_string_lossy().into_owned());
        let gone = d.path.as_deref().is_some_and(|p| !p.exists());
        let steps = d.undo.len();
        let dirty = d.is_dirty();
        let mut act = 0;
        egui::Frame::new().fill(crate::theme::BANNER_BG).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(if gone { format!("{name} is gone from the disk.") } else { format!("{name} changed on disk.") }).color(crate::theme::pal_ui(ui).overlay_text));
                if !gone && ui.button("Reload").on_hover_text(help_text::tip("disk-reload")).clicked() {
                    act = 1;
                }
                if !gone && dirty {
                    ui.label(RichText::new(format!("(drops your {steps} unsaved step{})", if steps == 1 { "" } else { "s" })).color(crate::theme::pal_ui(ui).overlay_text));
                }
                if ui.button("Keep mine").on_hover_text(help_text::tip("disk-keep")).clicked() {
                    act = 2;
                }
            });
        });
        match act {
            1 => {
                let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { return };
                match self.docs[i].reload_from_disk() {
                    Ok(()) => {
                        let (id, path, copy) = (self.docs[i].map_id, self.docs[i].path.clone(), Arc::new(self.docs[i].map.clone()));
                        if let (Some(f), Some(id)) = (&mut self.folder, id) {
                            if let Some(e) = f.maps.get_mut(&id).filter(|e| Some(&e.path) == path.as_ref()) {
                                e.svmap = Some(copy);
                            }
                        }
                        self.recovery_remove(uid);
                        self.safety.disk_changed.remove(&uid);
                        log::info(format!("reloaded {name} from disk"));
                        self.toast(format!("reloaded {name}"));
                    }
                    Err(e) => self.dialog = Some(Dialog::Error(e)),
                }
            }
            2 => {
                if let Some(d) = self.docs.iter_mut().find(|d| d.uid == uid) {
                    d.seen = d.path.as_deref().and_then(stamp_of);
                }
                self.safety.disk_changed.remove(&uid);
            }
            _ => {}
        }
    }

    // -------------------------------------------------------------------------------------------------------------- backups

    /// Saves are backed up before they replace the file; the folder key and the map number come from the target.
    pub(super) fn backup_before_save(&self, doc_idx: usize, target: Option<&Path>) -> Result<Option<PathBuf>, String> {
        let Some(data) = &self.data_dir else { return Ok(None) };
        let d = &self.docs[doc_idx];
        let Some(file) = target.or(d.path.as_deref()) else { return Ok(None) };
        let Some(id) = file.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()).or(d.map_id) else { return Ok(None) };
        let Some(folder) = file.parent() else { return Ok(None) };
        backups::make(&backups::folder_dir(data, folder), id, file, self.settings.backups_keep, SystemTime::now())
    }

    pub(super) fn open_backups_window(&mut self) {
        let Some(d) = self.docs.get(self.active) else { return };
        let (Some(data), Some(id), Some(path)) = (self.data_dir.clone(), d.map_id, d.path.clone()) else { return };
        let Some(folder) = path.parent() else { return };
        let dir = backups::folder_dir(&data, folder);
        let mut rows = Vec::new();
        for b in backups::list(&dir, id) {
            let summary = match std::fs::read(&b.path).ok().and_then(|bytes| shaiya_svmap::read(&bytes).ok()) {
                Some(m) => change_summary(&m, &d.map),
                None => "cannot be read".to_string(),
            };
            rows.push(BackupRow { backup: b, summary });
        }
        self.safety.backup_win = Some(BackupWin { uid: d.uid, id, dir, rows });
    }

    pub(super) fn backups_window(&mut self, ctx: &egui::Context) {
        let Some(w) = self.safety.backup_win.take() else { return };
        enum Act {
            OpenRo(usize),
            Restore(usize),
            Folder,
        }
        let mut open = true;
        let mut act: Option<Act> = None;
        let now = SystemTime::now();
        egui::Window::new("Restore from backup").open(&mut open).default_width(560.0).collapsible(false).show(ctx, |ui| {
            ui.label("Every time you save a map that already has a file, the editor first copies that file here. Only the newest ones are kept (Edit > Preferences...).");
            if w.rows.is_empty() {
                ui.label(RichText::new("No backups of this map yet.").weak());
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (i, r) in w.rows.iter().enumerate() {
                    ui.group(|ui| {
                        ui.label(RichText::new(log::utc_ago(r.backup.when, now)).strong());
                        ui.label(format!("{} bytes · {}", r.backup.size, r.summary));
                        ui.horizontal(|ui| {
                            if ui.button("Open read-only").on_hover_text("A new tab you can look at and copy from; nothing in it can be edited").clicked() {
                                act = Some(Act::OpenRo(i));
                            }
                            if ui.button("Restore into this tab").on_hover_text("Make this map look like the backup, as one undo step (nothing is saved)").clicked() {
                                act = Some(Act::Restore(i));
                            }
                        });
                    });
                }
            });
            if ui.button("Open backups folder").clicked() {
                act = Some(Act::Folder);
            }
        });
        match act {
            Some(Act::OpenRo(i)) => {
                if let Some(r) = w.rows.get(i) {
                    let label = format!("{} backup {}", w.id, log::utc_stamp(r.backup.when));
                    self.open_read_only(&r.backup.path, Some(label), Some(w.id));
                }
            }
            Some(Act::Restore(i)) => {
                if let Some(r) = w.rows.get(i) {
                    self.restore_from_backup(w.uid, &r.backup);
                }
            }
            Some(Act::Folder) => {
                let _ = std::fs::create_dir_all(&w.dir);
                crate::prefs::reveal(&w.dir);
            }
            None => {}
        }
        if open {
            self.safety.backup_win = Some(w);
        }
    }

    /// Restore into this tab: the map becomes the backup's as one undo step; nothing is written.
    pub(super) fn restore_from_backup(&mut self, uid: u64, b: &Backup) {
        let map = match std::fs::read(&b.path).map_err(|e| e.to_string()).and_then(|bytes| shaiya_svmap::read(&bytes).map_err(|e| e.to_string())) {
            Ok(m) => m,
            Err(e) => {
                self.toast(format!("cannot read the backup: {e}"));
                return;
            }
        };
        let Some(d) = self.docs.iter_mut().find(|d| d.uid == uid) else { return };
        let stamp = log::utc_stamp(b.when);
        if d.replace_with(&format!("restore from backup {stamp}"), &map) {
            self.toast(format!("restored from the backup of {stamp}: Undo goes back, Save keeps it"));
        } else if d.map.size != map.size {
            self.toast("the backup has another size: it cannot be restored into this tab (open it read-only)");
        } else {
            self.toast("this map already equals the backup");
        }
    }

    // -------------------------------------------------------------------------------------------------------------- save all, tabs

    /// File > Save all: every tab with unsaved changes, in tab order, each through the normal checks.
    pub(super) fn start_save_all(&mut self) {
        self.safety.save_queue = self.docs.iter().filter(|d| d.is_dirty() && !d.read_only).map(|d| d.uid).collect();
        self.step_save_queue();
    }

    /// Goes on with Save all while no dialog is open; a Cancel anywhere clears the queue.
    pub(super) fn step_save_queue(&mut self) {
        while self.dialog.is_none() {
            let Some(uid) = self.safety.save_queue.pop_front() else { break };
            let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { continue };
            if !self.docs[i].is_dirty() {
                continue;
            }
            if self.docs[i].path.is_none() {
                // a map without a file asks for a name; closing that dialog stops Save all
                self.active = i;
                self.request_save(false);
                if self.docs.get(i).is_some_and(|d| d.is_dirty()) {
                    self.safety.save_queue.clear();
                }
                continue;
            }
            self.begin_save(uid, None, false);
        }
    }

    pub(super) fn cancel_save_all(&mut self) {
        self.safety.save_queue.clear();
    }

    /// Opens a file as a read-only tab (a backup, a reference copy, or this map's own file again).
    pub(super) fn open_read_only(&mut self, path: &Path, label: Option<String>, map_id: Option<u32>) {
        if let Some(i) = self.docs.iter().position(|d| d.read_only && d.path.as_deref() == Some(path)) {
            self.switch_to(i);
            return;
        }
        let uid = self.new_uid();
        match Doc::open(uid, path) {
            Ok(mut d) => {
                d.read_only = true;
                if map_id.is_some() {
                    d.map_id = map_id;
                }
                d.label = label;
                log::info(format!("opened {} read-only", path.display()));
                self.docs.push(d);
                self.active = self.docs.len() - 1;
                self.vp = view2d::VpState::default();
                self.r5.showing = super::Showing::Map;
            }
            Err(e) => self.dialog = Some(Dialog::Error(e)),
        }
    }

    /// The right-click menu of tab `i`.
    pub(super) fn tab_menu(&self, ui: &mut egui::Ui, i: usize) -> Option<TabAct> {
        let d = &self.docs[i];
        let mut act = None;
        if ui.button("Close").clicked() {
            act = Some(TabAct::Close(i));
        }
        if ui.add_enabled(self.docs.len() > 1, egui::Button::new("Close others")).on_hover_text("Closes the other tabs that have no unsaved changes").clicked() {
            act = Some(TabAct::CloseOthers(i));
        }
        if ui.button("Close saved").on_hover_text("Closes every tab without unsaved changes").clicked() {
            act = Some(TabAct::CloseSaved);
        }
        ui.separator();
        let has_path = d.path.is_some();
        if ui.add_enabled(has_path, egui::Button::new("Copy path")).clicked() {
            act = Some(TabAct::CopyPath(i));
        }
        if ui.add_enabled(has_path, egui::Button::new("Reveal in Explorer")).clicked() {
            act = Some(TabAct::Reveal(i));
        }
        ui.separator();
        if ui.add_enabled(has_path && !d.read_only, egui::Button::new("Open read-only copy")).on_hover_text("The same file again, as a tab nothing can be edited in").clicked() {
            act = Some(TabAct::OpenReadOnly(i));
        }
        let reference = self.settings.reference_folder.as_deref().map(Path::new);
        let ref_ok = compare::source_of(d, CompareAgainst::Reference, reference);
        let r = ui.add_enabled(ref_ok.is_ok(), egui::Button::new("Compare with reference"));
        let r = match &ref_ok {
            Err(why) => r.on_disabled_hover_text(*why),
            Ok(_) => r,
        };
        if r.clicked() {
            act = Some(TabAct::CompareReference(i));
        }
        let r = ui.add_enabled(ref_ok.is_ok() && !d.read_only, egui::Button::new("Open reference copy (read-only)"));
        let r = match &ref_ok {
            Err(why) => r.on_disabled_hover_text(*why),
            Ok(_) => r,
        };
        if r.clicked() {
            act = Some(TabAct::OpenReference(i));
        }
        act
    }

    pub(super) fn do_tab_act(&mut self, a: TabAct, ctx: &egui::Context) {
        match a {
            TabAct::Close(i) => self.try_close_tab(i),
            TabAct::CloseOthers(i) => {
                let keep = self.docs.get(i).map(|d| d.uid);
                let mut skipped = 0;
                let uids: Vec<u64> = self.docs.iter().filter(|d| Some(d.uid) != keep).map(|d| d.uid).collect();
                for u in uids {
                    if let Some(k) = self.docs.iter().position(|d| d.uid == u) {
                        if self.docs[k].is_dirty() {
                            skipped += 1;
                        } else {
                            self.close_tab(k);
                        }
                    }
                }
                if let Some(k) = keep.and_then(|u| self.docs.iter().position(|d| d.uid == u)) {
                    self.active = k;
                }
                if skipped > 0 {
                    self.toast(format!("{skipped} tab{} with unsaved changes {} kept open", if skipped == 1 { "" } else { "s" }, if skipped == 1 { "was" } else { "were" }));
                }
            }
            TabAct::CloseSaved => {
                let uids: Vec<u64> = self.docs.iter().filter(|d| !d.is_dirty()).map(|d| d.uid).collect();
                for u in uids {
                    if let Some(k) = self.docs.iter().position(|d| d.uid == u) {
                        self.close_tab(k);
                    }
                }
            }
            TabAct::CopyPath(i) => {
                if let Some(p) = self.docs.get(i).and_then(|d| d.path.clone()) {
                    ctx.copy_text(p.display().to_string());
                    self.toast("path copied");
                }
            }
            TabAct::Reveal(i) => {
                if let Some(p) = self.docs.get(i).and_then(|d| d.path.clone()) {
                    crate::prefs::reveal_file(&p);
                }
            }
            TabAct::OpenReadOnly(i) => {
                if let Some((p, id)) = self.docs.get(i).and_then(|d| d.path.clone().map(|p| (p, d.map_id))) {
                    self.open_read_only(&p, None, id);
                }
            }
            TabAct::OpenReference(i) => {
                let folder = self.settings.reference_folder.clone();
                if let (Some(f), Some(id)) = (folder, self.docs.get(i).and_then(|d| d.map_id)) {
                    self.open_read_only(&compare::reference_path(Path::new(&f), id), Some(format!("{id} reference")), Some(id));
                }
            }
            TabAct::CompareReference(i) => {
                self.switch_to(i);
                self.settings.compare_against = CompareAgainst::Reference;
                self.settings.layers.compare = true;
                self.open_dock(DockTab::Changes);
            }
        }
    }

    /// Toasts for edits that a read-only tab refused.
    pub(super) fn refused_toasts(&mut self) {
        let mut refused = false;
        for d in &mut self.docs {
            if d.refused > 0 {
                d.refused = 0;
                refused = true;
            }
        }
        if refused {
            self.toast("read-only tab: use Copy, then paste into an editable tab");
        }
    }

    /// The tool the viewport should use: a read-only tab only selects and measures.
    pub(super) fn effective_tool(&self) -> Tool {
        if self.docs.get(self.active).is_some_and(|d| d.read_only) && !matches!(self.tool, Tool::Select | Tool::Measure) {
            Tool::Select
        } else {
            self.tool
        }
    }
}

/// `monster areas 3 · portals 1 · blocked cells 20` for what differs between a backup and the open map.
fn change_summary(backup: &Svmap, current: &Svmap) -> String {
    let mut parts: Vec<String> = Vec::new();
    for s in compare::LIST_SECTIONS {
        let n = compare::section_rows(backup, current, s).len();
        if n > 0 {
            parts.push(format!("{} {n}", s.label()));
        }
    }
    let cells = backup.blocked.differing_cells(&current.blocked).unwrap_or(0);
    if cells > 0 {
        parts.push(format!("blocked cells {cells}"));
    }
    if backup.size != current.size {
        parts.push("another size".to_string());
    }
    if parts.is_empty() {
        "the same as the open map".to_string()
    } else {
        format!("differs from the open map: {}", parts.join(" · "))
    }
}
