//! "Apply to several test clients...": the same patches on each chosen saved client, one after the other. Every client gets its own
//! confirmation (Changes, Writes, Undo, Game), then its own apply through the one apply path of Apply & history (`apply_with` per patch:
//! recover first, the game-running check, one journal per patch). The run stops at the first failure and says which clients were
//! changed (with their journals) and which were not touched.

use crate::apply_tab::{apply_patches_to, Applied};
use crate::archive_view::ArchiveView;
use crate::clients::{self, ClientInfo};
use crate::explain::{self, Rows};
use crate::keys::KeyForm;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::client::ClientFiles;
use shaiya_archive::Archive;
use shaiya_core::Error;
use shaiya_patch::{self as patch, Plan, VerifyKey};
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// A client that got (some of) the patches.
#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub client: String,
    pub dir: PathBuf,
    /// How many of the patches were applied (all of them unless this client is the one that failed).
    pub applied: usize,
    pub of: usize,
    /// The journal of each applied patch, in the client folder.
    pub journals: Vec<PathBuf>,
}

/// How a run ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub changed: Vec<Changed>,
    /// Clients whose files were not touched: the one that failed before its first patch was applied, and those that were never started.
    pub not_touched: Vec<String>,
    /// The client the run stopped at and why (an error, or "stopped at your request").
    pub stopped: Option<(String, String)>,
}

impl Outcome {
    /// The lines shown after a run (and the text of the status line).
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        for c in &self.changed {
            let names: Vec<String> = c.journals.iter().map(|j| j.strip_prefix(&c.dir).unwrap_or(j).display().to_string()).collect();
            let journals = if names.is_empty() { String::new() } else { format!("; journals: {}", names.join(", ")) };
            let part = if c.applied < c.of { format!("only {} of {} applied", c.applied, shaiya_inspect::counted(c.of as u64, "patch", "patches")) } else { format!("{} applied", shaiya_inspect::counted(c.applied as u64, "patch", "patches")) };
            v.push(format!("Changed: {} ({part}{journals})", c.client));
        }
        if let Some((client, why)) = &self.stopped {
            v.push(format!("Stopped at {client}: {why}"));
        }
        if !self.not_touched.is_empty() {
            v.push(format!("Not touched: {}", self.not_touched.join(", ")));
        }
        if self.stopped.is_none() {
            v.push("Done: every chosen client was changed.".to_string());
        }
        v
    }
}

enum Stage {
    /// The dry run of the patches against this client (it is what the confirmation shows).
    Planning(TaskHandle<Vec<Plan>>),
    Confirm(Rows, String),
    /// Waits until nothing holds the archive handle any more (the open client's).
    Closing(Option<Arc<Archive>>),
    Running(TaskHandle<Applied>),
}

struct Cur {
    name: String,
    dir: PathBuf,
    key: Option<VerifyKey>,
    stage: Stage,
}

struct Run {
    patches: Vec<PathBuf>,
    /// Clients not started yet, the next one first.
    todo: Vec<(String, PathBuf)>,
    total: usize,
    cur: Option<Cur>,
    outcome: Outcome,
}

#[derive(Default)]
pub struct MultiApply {
    pub open: bool,
    patches: Vec<(PathBuf, bool)>,
    clients: Vec<(String, PathBuf, bool)>,
    run: Option<Run>,
    /// How the last run ended.
    pub last: Option<Outcome>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

impl MultiApply {
    /// Opens the dialog with the patches of the Apply & history list (all picked) and the saved clients that are found (none picked).
    pub fn open_dialog(&mut self, patches: Vec<PathBuf>, cx: &mut ToolkitCx<'_>) {
        if self.is_running() {
            self.open = true;
            return;
        }
        self.patches = patches.into_iter().map(|p| (p, true)).collect();
        self.clients = cx.shell.shared().clients.iter().filter(|c| has_sah(Path::new(&c.path))).map(|c| (c.name.clone(), PathBuf::from(&c.path), false)).collect();
        self.last = None;
        self.open = true;
    }

    /// The test entry point of the dialog's Start button: the chosen patches onto the chosen clients, in this order.
    pub fn start(&mut self, patches: Vec<PathBuf>, clients: Vec<(String, PathBuf)>, cx: &mut ToolkitCx<'_>) {
        if self.is_running() || patches.is_empty() || clients.is_empty() {
            return;
        }
        let total = clients.len();
        self.last = None;
        self.run = Some(Run { patches, todo: clients, total, cur: None, outcome: Outcome::default() });
        self.begin_next(cx);
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Picks the patches and clients in the dialog by name (tests): `Some(true)` for the clients.
    pub fn choose(&mut self, patches: &[PathBuf], clients: &[&str]) {
        for (p, on) in &mut self.patches {
            *on = patches.contains(p);
        }
        for p in patches {
            if !self.patches.iter().any(|(q, _)| q == p) {
                self.patches.push((p.clone(), true));
            }
        }
        for (n, _, on) in &mut self.clients {
            *on = clients.iter().any(|c| c.eq_ignore_ascii_case(n));
        }
    }

    /// Presses Start with what is chosen now.
    pub fn start_chosen(&mut self, cx: &mut ToolkitCx<'_>) {
        let patches: Vec<PathBuf> = self.patches.iter().filter(|(_, on)| *on).map(|(p, _)| p.clone()).collect();
        let clients: Vec<(String, PathBuf)> = self.clients.iter().filter(|(_, _, on)| *on).map(|(n, d, _)| (n.clone(), d.clone())).collect();
        self.start(patches, clients, cx);
    }

    /// Takes the next client: the dry run first. A client that cannot even be planned stops the run before anything is written to it.
    fn begin_next(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(run) = &mut self.run else { return };
        if run.todo.is_empty() {
            self.finish(cx);
            return;
        }
        let (name, dir) = run.todo.remove(0);
        let (paths, d2) = (run.patches.clone(), dir.clone());
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Planning the patches for {name}"), cancellable: true, exclusive: None }, move |_job| patch::plan_seq(&paths, &d2, &[]));
        // the client's own signature settings decide whether a valid signature is required, as in Apply & history
        let info = ClientInfo { name: name.clone(), dir: dir.clone(), check_version: clients::version_numbers(&dir).0, current_version: clients::version_numbers(&dir).1 };
        let mut keys = KeyForm::default();
        keys.load(Some(&info), cx.settings);
        run.cur = Some(Cur { name, dir, key: keys.verify_key(), stage: Stage::Planning(h) });
    }

    /// The run stops here: the current client (if it was not changed) and every client not started are "not touched".
    fn stop(&mut self, why: String, cx: &mut ToolkitCx<'_>) {
        let Some(mut run) = self.run.take() else { return };
        if let Some(c) = run.cur.take() {
            if !run.outcome.changed.iter().any(|x| x.dir == c.dir) {
                run.outcome.not_touched.push(c.name.clone());
            }
            run.outcome.stopped = Some((c.name, why.clone()));
        } else {
            run.outcome.stopped = Some((String::new(), why.clone()));
        }
        run.outcome.not_touched.extend(run.todo.iter().map(|(n, _)| n.clone()));
        cx.shell.status(format!("Apply to several clients stopped: {why}"));
        self.last = Some(run.outcome);
    }

    fn finish(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(run) = self.run.take() else { return };
        cx.shell.status(format!("Applied the patches to {}", shaiya_inspect::counted(run.outcome.changed.len() as u64, "client", "clients")));
        self.last = Some(run.outcome);
    }

    /// Advances the run; call every frame. When the open client was changed the Archive tab reads it again.
    pub fn tick(&mut self, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView) {
        let Some(run) = &mut self.run else { return };
        let Some(cur) = &mut run.cur else { return };
        let (name, dir) = (cur.name.clone(), cur.dir.clone());
        match std::mem::replace(&mut cur.stage, Stage::Closing(None)) {
            Stage::Planning(mut h) => match h.poll() {
                None => cur.stage = Stage::Planning(h),
                Some(TaskOutcome::Done(plans)) => {
                    let rows = match ClientFiles::locate(&dir) {
                        Ok(files) => explain::patch_rows(&plans, &files),
                        Err(_) => Vec::new(),
                    };
                    let list: Vec<String> = run.patches.iter().map(|p| file_name(p)).collect();
                    let list = if list.len() <= 3 { list.join(" and ") } else { format!("{} patches", list.len()) };
                    let sigs = if cur.key.is_some() { "a valid signature is required and checked" } else { "not used (unsigned patches)" };
                    let which = run.total - run.todo.len();
                    let text = format!("Apply {list} to {name}? Client {which} of {}. Signatures: {sigs}.", run.total);
                    cur.stage = Stage::Confirm(rows, text);
                }
                Some(TaskOutcome::Failed(e)) => self.stop(format!("cannot plan the patches: {e}"), cx),
                Some(TaskOutcome::Cancelled) => self.stop("cancelled".to_string(), cx),
                Some(TaskOutcome::Panicked(m)) => self.stop(format!("internal error (a bug): {m}"), cx),
            },
            Stage::Confirm(rows, text) => {
                let c = Confirm { title: "Apply patches".into(), text: text.clone(), buttons: vec![format!("Apply to {name}"), "Stop here".into()], danger: Some(0), details: rows.clone() };
                match cx.shell.confirm("toolkit-multi-apply", &c) {
                    None => cur.stage = Stage::Confirm(rows, text),
                    Some(0) => {
                        let arc = if archive.client_dir() == Some(dir.as_path()) { archive.release_handle() } else { None };
                        cur.stage = Stage::Closing(arc);
                    }
                    Some(_) => self.stop("stopped at your request".to_string(), cx),
                }
            }
            Stage::Closing(arc) => {
                if arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true) {
                    drop(arc);
                    let (patches, key, d2) = (run.patches.clone(), cur.key.clone(), dir.clone());
                    let n = patches.len();
                    let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {} to {name}", shaiya_inspect::counted(n as u64, "patch", "patches")), cancellable: true, exclusive: Some(dir.display().to_string()) }, move |job| Ok(apply_patches_to(&patches, &d2, key, job)));
                    cur.stage = Stage::Running(h);
                } else {
                    cx.shell.ctx().request_repaint();
                    cur.stage = Stage::Closing(arc);
                }
            }
            Stage::Running(mut h) => match h.poll() {
                None => cur.stage = Stage::Running(h),
                Some(outcome) => {
                    let of = run.patches.len();
                    let (applied, err): (Applied, Option<String>) = match outcome {
                        TaskOutcome::Done(a) => {
                            let e = a.error.as_ref().map(|e| if matches!(e, Error::Cancelled) { "cancelled".to_string() } else { e.to_string() });
                            (a, e)
                        }
                        TaskOutcome::Failed(e) => (Applied { reports: Vec::new(), error: None }, Some(e.to_string())),
                        TaskOutcome::Cancelled => (Applied { reports: Vec::new(), error: None }, Some("cancelled".to_string())),
                        TaskOutcome::Panicked(m) => (Applied { reports: Vec::new(), error: None }, Some(format!("internal error (a bug): {m}"))),
                    };
                    if !applied.reports.is_empty() {
                        run.outcome.changed.push(Changed { client: name.clone(), dir: dir.clone(), applied: applied.reports.len(), of, journals: applied.reports.iter().filter_map(|r| r.journal.clone()).collect() });
                        archive.changed_counter += 1;
                    }
                    // the open client's archive is read again whether or not the apply went through
                    if archive.client_dir() == Some(dir.as_path()) || archive.source_dir_is(&dir) {
                        archive.reopen(cx);
                    }
                    match err {
                        Some(why) => self.stop(why, cx),
                        None => self.begin_next(cx),
                    }
                }
            },
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView) {
        self.tick(cx, archive);
        if !self.open {
            return;
        }
        let pal = cx.shell.pal();
        let mut open = true;
        let mut start = false;
        let mut add = false;
        let running = self.is_running();
        let busy = archive.is_applying();
        egui::Window::new("Apply to several test clients").open(&mut open).default_width(560.0).show(ctx, |ui| {
            ui.label(RichText::new("The patches are applied to each chosen client in turn. Each client gets its own confirmation (what changes, what is written, how to undo it, the game must be closed) and its own journal. If one client fails, the run stops there; the clients already done stay changed and can be rolled back from Apply & history.").color(pal.weak));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Patches");
                if ui.add_enabled(!running, egui::Button::new("Add...")).clicked() {
                    add = true;
                }
            });
            if self.patches.is_empty() {
                ui.label(RichText::new("No patches yet. Add psNNNN.patch files; they are applied in the order shown.").color(pal.weak));
            }
            for (p, on) in &mut self.patches {
                ui.add_enabled(!running, egui::Checkbox::new(on, file_name(p)));
            }
            ui.add_space(4.0);
            ui.heading("Clients");
            if self.clients.is_empty() {
                ui.label(RichText::new("No saved client folder was found. Add one with Add client folder... in the toolbar.").color(pal.weak));
            }
            for (n, _, on) in &mut self.clients {
                ui.add_enabled(!running, egui::Checkbox::new(on, n.as_str()));
            }
            ui.add_space(6.0);
            let ready = !running && !busy && self.patches.iter().any(|(_, on)| *on) && self.clients.iter().any(|(_, _, on)| *on);
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Start")).clicked() {
                    start = true;
                }
                if running {
                    ui.label(RichText::new("Working...").color(pal.weak));
                } else if busy {
                    ui.label(RichText::new("Wait for the change in the Archive tab to finish.").color(pal.weak));
                }
            });
            if let Some(run) = &self.run {
                if let Some(c) = &run.cur {
                    ui.label(format!("Now: {} (client {} of {})", c.name, run.total - run.todo.len(), run.total));
                }
            }
            if let Some(o) = &self.last {
                ui.separator();
                for l in o.lines() {
                    ui.label(RichText::new(l).color(if o.stopped.is_some() { pal.warning } else { pal.ok }));
                }
            }
        });
        if add {
            for p in cx.shell.pick_files("patches", &[("Patches", &["patch"]), ("All files", &["*"])]) {
                if !self.patches.iter().any(|(q, _)| *q == p) {
                    self.patches.push((p, true));
                }
            }
        }
        if start {
            self.start_chosen(cx);
        }
        if !open && !self.is_running() {
            self.open = false;
        }
    }
}

fn has_sah(dir: &Path) -> bool {
    dir.is_dir() && std::fs::read_dir(dir).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or(false)
}
