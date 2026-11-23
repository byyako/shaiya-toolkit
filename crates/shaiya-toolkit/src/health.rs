//! The Health window: what `shaiya clients health` reports for a saved client (index check, dead space, journals, leftovers), and the
//! clean-up that moves leftovers into `shaiya-toolkit-backups/cleanup-<UTC>/` (it deletes none of them) and Put back. The report is read only; both
//! changes go through `shaiya_archive::cleanup` after a confirmation with the Changes / Writes / Undo / Game rows.

use crate::archive_view::ArchiveView;
use crate::explain::{self, Rows};
use crate::multi_lock::lock_hover;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::cleanup::{self, CleanReport, FolderInfo, Manifest, Plan, PutBackReport};
use shaiya_archive::health::{self, Health};
use shaiya_archive::Archive;
use shaiya_core::time::human_size;
use shaiya_core::Result;
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// What a frame of the window asks the app frame to do.
pub enum HealthAction {
    /// A clean-up or Put back ran (or failed): reload the open archive when it is this folder.
    Reload(PathBuf),
    /// The "Compact..." link: the Archive tab's Compact confirmation for this client.
    Compact(PathBuf),
    /// The "Prune..." link: the Apply & history tab's Prune confirmation for this client.
    Prune(PathBuf),
}

enum Load {
    Idle,
    Loading(TaskHandle<Health>),
    Ready(Box<Health>),
    Failed(String),
}

/// The client a clean-up or Put back was confirmed for. A flow carries it from the confirmation to the end, so choosing another client
/// meanwhile (the picker is disabled while busy, but `open_for` can still be reached) never moves anything in that other folder.
#[derive(Clone)]
struct Target {
    name: String,
    dir: PathBuf,
}

enum Flow {
    Idle,
    Confirm(Target, Plan),
    /// Waits until nothing holds the archive handle any more (the in-use check opens the pair exclusively).
    Closing(Target, Vec<String>, Option<Arc<Archive>>),
    Running(Target, TaskHandle<CleanReport>),
    ConfirmPutBack(Target, String, Manifest),
    PuttingBack(Target, String, TaskHandle<PutBackReport>),
}

pub struct HealthView {
    pub open: bool,
    client: Option<(String, PathBuf)>,
    load: Load,
    selected: BTreeSet<String>,
    flow: Flow,
    rows: Rows,
    /// Links of the window that the app frame carries out (Compact..., Prune...).
    queued: Vec<HealthAction>,
    /// The last thing a clean-up or Put back did (or why it did not).
    pub last: Option<String>,
    /// True while "Apply to several test clients" runs (set by the app every frame): Clean up, Put back, Compact and Prune are off.
    multi_running: bool,
}

impl Default for HealthView {
    fn default() -> Self {
        HealthView { open: false, client: None, load: Load::Idle, selected: BTreeSet::new(), flow: Flow::Idle, rows: Vec::new(), queued: Vec::new(), last: None, multi_running: false }
    }
}

impl HealthView {
    /// Opens the window for a saved client and reads its report on a worker thread.
    pub fn open_for(&mut self, name: &str, dir: PathBuf, cx: &mut ToolkitCx<'_>) {
        self.open = true;
        self.client = Some((name.to_string(), dir));
        self.last = None;
        self.refresh(cx);
    }

    pub fn client_dir(&self) -> Option<&Path> {
        self.client.as_ref().map(|(_, d)| d.as_path())
    }

    pub fn refresh(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some((name, dir)) = self.client.clone() else { return };
        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Checking {name}"), cancellable: false, exclusive: None }, move |_| health::report(&dir));
        self.load = Load::Loading(handle);
    }

    /// The finished report, once it is there.
    pub fn report(&self) -> Option<&Health> {
        match &self.load {
            Load::Ready(h) => Some(h),
            _ => None,
        }
    }

    /// The ids of the leftovers that are ticked.
    pub fn selected(&self) -> Vec<String> {
        self.selected.iter().cloned().collect()
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.flow, Flow::Idle)
    }

    /// The app tells the window every frame whether "Apply to several test clients" runs.
    pub fn set_multi_running(&mut self, on: bool) {
        self.multi_running = on;
    }

    /// False while a multi-client apply runs: Clean up, Put back, Compact and Prune are off then (the buttons and their requests).
    pub fn write_actions_enabled(&self) -> bool {
        !self.multi_running
    }

    /// Ticks or unticks one leftover (the checkbox calls this).
    pub fn set_selected(&mut self, id: &str, on: bool) {
        if on {
            self.selected.insert(id.to_string());
        } else {
            self.selected.remove(id);
        }
    }

    /// "Clean up...": the confirmation for the ticked leftovers.
    pub fn request_clean(&mut self) {
        let Load::Ready(h) = &self.load else { return };
        let Some((name, dir)) = self.client.clone() else { return };
        if self.is_busy() || !self.write_actions_enabled() {
            return;
        }
        let ids: Vec<String> = self.selected.iter().cloned().collect();
        if let Ok(plan) = cleanup::plan(h, &ids) {
            if !plan.items.is_empty() {
                self.rows = explain::cleanup_rows(&plan);
                self.flow = Flow::Confirm(Target { name, dir }, plan);
            }
        }
    }

    /// "Put back...": the confirmation for one cleanup folder.
    pub fn request_put_back(&mut self, folder: &str) {
        let Some((name, dir)) = self.client.clone() else { return };
        if self.is_busy() || !self.write_actions_enabled() {
            return;
        }
        let path = dir.join(shaiya_archive::journal::DEFAULT_DIR).join(folder);
        match Manifest::read(&path) {
            Ok(m) => {
                self.rows = explain::put_back_rows(&m);
                self.flow = Flow::ConfirmPutBack(Target { name, dir }, folder.to_string(), m);
            }
            Err(e) => self.last = Some(format!("{folder} cannot be put back: {e}")),
        }
    }

    fn poll_load(&mut self) {
        let Load::Loading(h) = &mut self.load else { return };
        let Some(outcome) = h.poll() else { return };
        self.load = match outcome {
            TaskOutcome::Done(h) => {
                self.selected = h.leftovers.iter().filter(|l| l.pre_selected).map(|l| l.id.clone()).collect();
                Load::Ready(Box::new(h))
            }
            TaskOutcome::Failed(e) => Load::Failed(e.to_string()),
            TaskOutcome::Cancelled => Load::Failed("cancelled".to_string()),
            TaskOutcome::Panicked(m) => Load::Failed(m),
        };
    }

    /// Draws the window and advances a clean-up or Put back. Returns what the app frame must do.
    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView) -> Vec<HealthAction> {
        let mut actions = Vec::new();
        self.poll_load();
        if matches!(self.load, Load::Loading(_)) {
            ctx.request_repaint();
        }
        if self.open {
            self.window(ctx, cx);
        }
        self.poll_flow(cx, archive, &mut actions);
        actions.append(&mut self.queued);
        actions
    }

    fn window(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        let clients = cx.shell.shared().clients.clone();
        let busy = self.is_busy();
        let locked = !self.write_actions_enabled();
        let mut open = self.open;
        let mut pick: Option<(String, PathBuf)> = None;
        let (mut refresh, mut clean, mut compact, mut prune) = (false, false, false, false);
        let mut put_back: Option<String> = None;
        let mut toggled: Vec<(String, bool)> = Vec::new();
        let current = self.client.as_ref().map(|(n, _)| n.clone());
        egui::Window::new("Client health").open(&mut open).default_width(760.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Client");
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt("health-client").selected_text(current.clone().unwrap_or_else(|| "(choose)".to_string())).show_ui(ui, |ui| {
                        for c in &clients {
                            if ui.selectable_label(current.as_deref() == Some(c.name.as_str()), &c.name).clicked() {
                                pick = Some((c.name.clone(), PathBuf::from(&c.path)));
                            }
                        }
                    });
                });
                if ui.add_enabled(current.is_some() && !busy, egui::Button::new("Check again")).clicked() {
                    refresh = true;
                }
            });
            ui.label(RichText::new("This only looks. Nothing in the client folder is changed until you confirm a clean-up.").color(pal.weak));
            ui.separator();
            match &self.load {
                Load::Idle => {
                    ui.label(if clients.is_empty() { "No client folders yet. Use Add client folder... in the toolbar." } else { "Choose a client above." });
                }
                Load::Loading(_) => {
                    ui.label("Checking...");
                }
                Load::Failed(why) => {
                    ui.label(RichText::new(format!("The index cannot be read: {why}")).color(pal.error));
                }
                Load::Ready(h) => {
                    let ix = &h.index;
                    ui.label(format!("Index: SAH version {}, {} files; the check found {} and {}.", ix.sah_version, shaiya_inspect::group(ix.files as u64), plural(ix.problems, "problem"), plural(ix.warnings, "warning")));
                    match &h.dead_space {
                        Some(d) => {
                            ui.horizontal(|ui| {
                                ui.label(format!("data.saf: {} on disk, {} in use, {} dead space.", human_size(d.saf_bytes), human_size(d.live_bytes), human_size(d.dead_bytes)));
                                if lock_hover(ui.add_enabled(!busy && !locked && d.dead_bytes > 0, egui::Button::new("Compact...")).on_hover_text("Rewrite data.saf without the dead space. It makes no journal, so earlier changes can no longer be rolled back"), locked).clicked() {
                                    compact = true;
                                }
                            });
                        }
                        None => {
                            ui.label(RichText::new("data.saf is missing.").color(pal.error));
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!("Journals: {} ({}).", h.journals.count, human_size(h.journals.bytes)));
                        if lock_hover(ui.add_enabled(!busy && !locked && h.journals.count > 0, egui::Button::new("Prune...")).on_hover_text("Delete the oldest journals; they ask first"), locked).clicked() {
                            prune = true;
                        }
                    });
                    if let Some(m) = &h.pending_marker {
                        let what = if m.readable { format!("A change ({} of {}) did not finish.", m.op, m.label) } else { "data.toolkit-pending cannot be read.".to_string() };
                        ui.label(RichText::new(format!("{what} Opening the client in Shaiya Toolkit, or the next write, finishes or removes it; its temporary files are not offered below.")).color(pal.error));
                    }
                    if let Some(s) = &h.start_update {
                        ui.label(format!("Version.ini has StartUpdate={s}: an update by the original updater did not finish. Shaiya Toolkit never edits Version.ini; run the original updater again or set the value yourself."));
                    }
                    ui.separator();
                    ui.label(RichText::new("Leftovers").strong());
                    if h.leftovers.is_empty() {
                        ui.label("Nothing to clean up.");
                    }
                    for l in &h.leftovers {
                        let mut on = self.selected.contains(&l.id);
                        if ui.add_enabled(!busy, egui::Checkbox::new(&mut on, format!("{}  {}  ({})", l.kind.name(), l.files.join(" + "), human_size(l.bytes)))).changed() {
                            toggled.push((l.id.clone(), on));
                        }
                        ui.add(egui::Label::new(RichText::new(format!("{}{}", l.note, if l.pre_selected { "" } else { " Not ticked at first." })).color(pal.weak)).wrap());
                    }
                    for n in &h.not_offered {
                        ui.add(egui::Label::new(RichText::new(format!("Not offered: {} ({})", n.file, n.why)).color(pal.weak)).wrap());
                    }
                    let ticked = h.leftovers.iter().filter(|l| self.selected.contains(&l.id)).count();
                    ui.horizontal(|ui| {
                        if lock_hover(ui.add_enabled(!busy && !locked && ticked > 0, egui::Button::new("Clean up...")).on_hover_text("Moves the ticked files into a cleanup folder; none of them is deleted"), locked).clicked() {
                            clean = true;
                        }
                        ui.label(RichText::new(format!("{ticked} ticked")).color(pal.weak));
                    });
                    if !h.cleanups.is_empty() {
                        ui.separator();
                        ui.label(RichText::new("Cleanup folders").strong());
                        for f in &h.cleanups {
                            ui.horizontal(|ui| {
                                ui.label(folder_line(f));
                                if lock_hover(ui.add_enabled(!busy && !locked && f.to_put_back > 0, egui::Button::new("Put back...")), locked).clicked() {
                                    put_back = Some(f.name.clone());
                                }
                            });
                        }
                    }
                }
            }
            if let Some(t) = &self.last {
                ui.separator();
                ui.label(t);
            }
        });
        self.open = open;
        for (id, on) in toggled {
            self.set_selected(&id, on);
        }
        if let Some((n, d)) = pick {
            self.open_for(&n, d, cx);
        }
        if refresh {
            self.refresh(cx);
        }
        if clean {
            self.request_clean();
        }
        if let Some(f) = put_back {
            self.request_put_back(&f);
        }
        if compact {
            self.ask_compact();
        }
        if prune {
            self.ask_prune();
        }
    }

    /// The "Compact..." link.
    pub fn ask_compact(&mut self) {
        if let (Some((_, d)), true) = (&self.client, self.write_actions_enabled()) {
            self.queued.push(HealthAction::Compact(d.clone()));
        }
    }

    /// The "Prune..." link.
    pub fn ask_prune(&mut self) {
        if let (Some((_, d)), true) = (&self.client, self.write_actions_enabled()) {
            self.queued.push(HealthAction::Prune(d.clone()));
        }
    }

    fn poll_flow(&mut self, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView, actions: &mut Vec<HealthAction>) {
        match std::mem::replace(&mut self.flow, Flow::Idle) {
            Flow::Idle => {}
            Flow::Confirm(t, plan) => {
                let Target { name, dir } = &t;
                let c = Confirm {
                    title: "Clean up".into(),
                    text: format!("Move {} of {name} into a cleanup folder? None of them is deleted.", plural(plan.files, "file")),
                    buttons: vec!["Clean up".into(), "Cancel".into()],
                    danger: None,
                    details: self.rows.clone(),
                };
                match cx.shell.confirm("toolkit-cleanup", &c) {
                    None => self.flow = Flow::Confirm(t, plan),
                    Some(0) => {
                        let arc = if archive.client_dir() == Some(dir.as_path()) { archive.release_handle() } else { None };
                        self.flow = Flow::Closing(t, plan.items.iter().map(|l| l.id.clone()).collect(), arc);
                    }
                    Some(_) => {}
                }
            }
            Flow::Closing(t, ids, arc) => {
                let Target { name, dir } = &t;
                if arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true) {
                    drop(arc);
                    let d2 = dir.clone();
                    let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Cleaning up {name}"), cancellable: false, exclusive: Some(dir.display().to_string()) }, move |job| cleanup::clean(&d2, &ids, job));
                    self.flow = Flow::Running(t, handle);
                } else {
                    cx.shell.ctx().request_repaint();
                    self.flow = Flow::Closing(t, ids, arc);
                }
            }
            Flow::Running(t, mut handle) => {
                let (name, dir) = (t.name.clone(), t.dir.clone());
                match handle.poll() {
                None => self.flow = Flow::Running(t, handle),
                Some(TaskOutcome::Done(r)) => {
                    self.last = Some(if r.moved.is_empty() {
                        "Nothing was moved.".to_string()
                    } else {
                        format!("Moved {} into {}. Put back below undoes it.", plural(r.moved.len(), "file"), r.folder)
                    });
                    cx.shell.status(format!("{name}: {}", self.last.clone().unwrap_or_default()));
                    actions.push(HealthAction::Reload(dir));
                    self.refresh(cx);
                }
                Some(TaskOutcome::Failed(e)) => {
                    self.last = Some(format!("The clean-up stopped: {e}"));
                    cx.shell.error("The clean-up stopped", &e);
                    actions.push(HealthAction::Reload(dir));
                    self.refresh(cx);
                }
                Some(_) => {
                    actions.push(HealthAction::Reload(dir));
                    self.refresh(cx);
                }
                }
            }
            Flow::ConfirmPutBack(t, folder, m) => {
                let Target { name, dir } = &t;
                let c = Confirm {
                    title: "Put back".into(),
                    text: format!("Move the files of {folder} back into {name}?"),
                    buttons: vec!["Put back".into(), "Cancel".into()],
                    danger: None,
                    details: self.rows.clone(),
                };
                match cx.shell.confirm("toolkit-put-back", &c) {
                    None => self.flow = Flow::ConfirmPutBack(t, folder, m),
                    Some(0) => {
                        let path = dir.join(shaiya_archive::journal::DEFAULT_DIR).join(&folder);
                        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Putting back {folder}"), cancellable: false, exclusive: Some(dir.display().to_string()) }, move |job| -> Result<PutBackReport> { cleanup::put_back(&path, job) });
                        self.flow = Flow::PuttingBack(t, folder, handle);
                    }
                    Some(_) => {}
                }
            }
            Flow::PuttingBack(t, folder, mut handle) => {
                let (name, dir) = (t.name.clone(), t.dir.clone());
                match handle.poll() {
                None => self.flow = Flow::PuttingBack(t, folder, handle),
                Some(TaskOutcome::Done(r)) => {
                    let mut t = format!("Put back {}.", plural(r.restored.len(), "file"));
                    if !r.missing.is_empty() {
                        t.push_str(&format!(" Not found in the cleanup folder or in place: {}.", r.missing.join(", ")));
                    }
                    cx.shell.status(format!("{name}: {t}"));
                    self.last = Some(t);
                    actions.push(HealthAction::Reload(dir));
                    self.refresh(cx);
                }
                Some(TaskOutcome::Failed(e)) => {
                    let path = t.dir.join(shaiya_archive::journal::DEFAULT_DIR).join(&folder);
                    self.last = Some(put_back_failure(&e.to_string(), Manifest::read(&path).ok().as_ref()));
                    cx.shell.error("Put back stopped", &e);
                    self.refresh(cx);
                }
                Some(_) => self.refresh(cx),
                }
            }
        }
    }
}

/// What a stopped Put back says, from the manifest as it is now: how many files are back and how many are still in the cleanup folder.
fn put_back_failure(why: &str, manifest: Option<&Manifest>) -> String {
    match manifest {
        Some(m) => {
            let back = m.items.iter().filter(|i| i.state == "put-back").count();
            let left = m.items.len() - back;
            format!("Put back stopped: {why}. {back} of {} files are back in place, {left} still in the cleanup folder; Put back again finishes it.", m.items.len())
        }
        None => format!("Put back stopped: {why}. The cleanup folder's manifest cannot be read now; look at what is in the folder before trying again."),
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn folder_line(f: &FolderInfo) -> String {
    let how = match f.state.as_str() {
        "moving" => "interrupted; some files may be in place already",
        "put-back" => "put back",
        "unreadable" => "manifest.json cannot be read",
        _ => "moved",
    };
    format!("{}: {how}, {} still to put back ({})", f.name, f.to_put_back, human_size(f.bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_archive::cleanup::Item;

    fn item(from: &str, state: &str) -> Item {
        Item { group: from.into(), kind: "stray-temp".into(), from: from.into(), to: from.into(), bytes: 1, state: state.into() }
    }

    #[test]
    fn a_put_back_that_stopped_says_how_many_files_are_back_and_how_many_remain() {
        let m = Manifest { id: "cleanup-x".into(), created_utc: String::new(), tool: String::new(), state: "moved".into(), items: vec![item("a", "put-back"), item("b", "put-back"), item("c", "moved")] };
        let t = put_back_failure("c.txt: access denied", Some(&m));
        assert!(t.contains("2 of 3 files are back in place") && t.contains("1 still in the cleanup folder") && !t.contains("Nothing was put back"), "{t}");
        let none = put_back_failure("boom", Some(&Manifest { items: vec![item("a", "moved")], ..m.clone() }));
        assert!(none.contains("0 of 1 files are back in place"), "{none}");
        let unread = put_back_failure("boom", None);
        assert!(unread.contains("manifest cannot be read") && !unread.contains("Nothing was put back"), "{unread}");
    }
}
