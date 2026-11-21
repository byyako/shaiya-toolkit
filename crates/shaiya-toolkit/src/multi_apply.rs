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
use std::collections::BTreeSet;
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
    /// The apply ended in an internal error: how many patches went in is not known, so the client may be partly changed.
    pub unknown: bool,
}

/// How a run ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub changed: Vec<Changed>,
    /// Clients whose files were not touched: the one that failed before its first patch was applied, and those that were never started.
    pub not_touched: Vec<String>,
    /// The client the run stopped at and why (an error, or "stopped at your request").
    pub stopped: Option<(String, String)>,
    /// Clients left out because they share the folder of an earlier chosen client: (client, the client whose folder it is).
    pub same_folder: Vec<(String, String)>,
    /// The run stopped on a client whose apply had started: the recover step of the apply may have written to it before it failed.
    pub recover_may_have_written: bool,
}

impl Outcome {
    /// The lines shown after a run (and the text of the status line).
    pub fn lines(&self) -> Vec<String> {
        let mut v = Vec::new();
        for c in &self.changed {
            let names: Vec<String> = c.journals.iter().map(|j| j.strip_prefix(&c.dir).unwrap_or(j).display().to_string()).collect();
            let journals = if names.is_empty() { String::new() } else { format!("; journals: {}", names.join(", ")) };
            if c.unknown {
                v.push(format!("May be partly changed: {} (an internal error stopped it; check its history{journals})", c.client));
                continue;
            }
            let part = if c.applied < c.of { format!("only {} of {} applied", c.applied, shaiya_inspect::counted(c.of as u64, "patch", "patches")) } else { format!("{} applied", shaiya_inspect::counted(c.applied as u64, "patch", "patches")) };
            v.push(format!("Changed: {} ({part}{journals})", c.client));
        }
        if let Some((client, why)) = &self.stopped {
            v.push(format!("Stopped at {client}: {why}"));
        }
        if !self.not_touched.is_empty() {
            v.push(format!("Not touched: {}", self.not_touched.join(", ")));
            if self.recover_may_have_written {
                v.push("A client that failed while applying may still have had an unfinished earlier change recovered (written) before its patches started; check its history.".to_string());
            }
        }
        for (c, of) in &self.same_folder {
            if self.changed.iter().any(|x| x.client == *of) {
                v.push(format!("{c} is the same folder as {of}: the patches were applied there once."));
            } else {
                v.push(format!("{c} was left out because {of} (the same folder) was not applied."));
            }
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
    /// The journal files of the client when its apply started (`None` before that).
    journals_before: Option<BTreeSet<PathBuf>>,
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
        // a folder is applied once, however many saved clients point at it
        let mut todo: Vec<(String, PathBuf)> = Vec::new();
        let mut outcome = Outcome::default();
        for (n, d) in clients {
            match todo.iter().find(|(_, d2)| clients::same_folder(d2, &d)) {
                Some((first, _)) => outcome.same_folder.push((n, first.clone())),
                None => todo.push((n, d)),
            }
        }
        let total = todo.len();
        self.last = None;
        self.run = Some(Run { patches, todo, total, cur: None, outcome });
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

    /// Why Start must wait: the Archive tab or the Apply tab is writing to a client (`None` when it can start).
    pub fn blocked(archive_applying: bool, apply_tab_busy: bool) -> Option<&'static str> {
        if archive_applying {
            Some("Wait for the change in the Archive tab to finish.")
        } else if apply_tab_busy {
            Some("Wait for the apply in the Apply tab to finish.")
        } else {
            None
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
        run.cur = Some(Cur { name, dir, key: keys.verify_key(), stage: Stage::Planning(h), journals_before: None });
    }

    /// The run stops here: the current client (if it was not changed) and every client not started are "not touched".
    fn stop(&mut self, why: String, cx: &mut ToolkitCx<'_>) {
        let Some(mut run) = self.run.take() else { return };
        if let Some(c) = run.cur.take() {
            if !run.outcome.changed.iter().any(|x| x.dir == c.dir) {
                run.outcome.not_touched.push(c.name.clone());
            }
            run.outcome.recover_may_have_written = c.journals_before.is_some();
            run.outcome.stopped = Some((c.name, why.clone()));
        } else {
            run.outcome.stopped = Some((String::new(), why.clone()));
        }
        run.outcome.not_touched.extend(run.todo.iter().map(|(n, _)| n.clone()));
        // a client left out as the same folder is as untouched as the client whose folder it is
        let more: Vec<String> = run.outcome.same_folder.iter().filter(|(_, of)| run.outcome.not_touched.contains(of)).map(|(n, _)| n.clone()).collect();
        run.outcome.not_touched.extend(more);
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
                    let rows = match confirm_rows(&plans, ClientFiles::locate(&dir)) {
                        Ok(rows) => rows,
                        Err(why) => {
                            self.stop(why, cx);
                            return;
                        }
                    };
                    let list: Vec<String> = run.patches.iter().map(|p| file_name(p)).collect();
                    let list = if list.len() <= 3 { list.join(" and ") } else { format!("{} patches", list.len()) };
                    let sigs = if cur.key.is_some() { "a valid signature is required and checked" } else { "not used (unsigned patches)" };
                    let which = run.total - run.todo.len();
                    let text = format!("Apply {list} to {name} ({})? Client {which} of {}. Signatures: {sigs}.", cx.shell.folder_text(&name, &dir), run.total);
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
                    cur.journals_before = Some(journal_files(&dir));
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
                    let mut unknown = false;
                    let (applied, err): (Applied, Option<String>) = match outcome {
                        TaskOutcome::Done(a) => {
                            let e = a.error.as_ref().map(|e| if matches!(e, Error::Cancelled) { "cancelled".to_string() } else { e.to_string() });
                            (a, e)
                        }
                        TaskOutcome::Failed(e) => (Applied { reports: Vec::new(), error: None }, Some(e.to_string())),
                        TaskOutcome::Cancelled => (Applied { reports: Vec::new(), error: None }, Some("cancelled".to_string())),
                        TaskOutcome::Panicked(m) => {
                            unknown = true;
                            (Applied { reports: Vec::new(), error: None }, Some(format!("internal error (a bug): {m}")))
                        }
                    };
                    if unknown {
                        // the patches that went in before the panic are not known: the client may be partly changed, with the journals that appeared
                        let before = cur.journals_before.take().unwrap_or_default();
                        let journals: Vec<PathBuf> = journal_files(&dir).into_iter().filter(|j| !before.contains(j)).collect();
                        run.outcome.changed.push(Changed { client: name.clone(), dir: dir.clone(), applied: 0, of, journals, unknown: true });
                        archive.changed_counter += 1;
                    } else if !applied.reports.is_empty() {
                        run.outcome.changed.push(Changed { client: name.clone(), dir: dir.clone(), applied: applied.reports.len(), of, journals: applied.reports.iter().filter_map(|r| r.journal.clone()).collect(), unknown: false });
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

    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView, apply_busy: bool) {
        self.tick(cx, archive);
        if !self.open {
            return;
        }
        let pal = cx.shell.pal();
        let mut open = true;
        let mut start = false;
        let mut add = false;
        let running = self.is_running();
        let busy = Self::blocked(archive.is_applying(), apply_busy);
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
            let ready = !running && busy.is_none() && self.patches.iter().any(|(_, on)| *on) && self.clients.iter().any(|(_, _, on)| *on);
            ui.horizontal(|ui| {
                if ui.add_enabled(ready, egui::Button::new("Start")).clicked() {
                    start = true;
                }
                if running {
                    ui.label(RichText::new("Working...").color(pal.weak));
                } else if let Some(why) = busy {
                    ui.label(RichText::new(why).color(pal.weak));
                }
                if !running {
                    let chosen: Vec<&PathBuf> = self.clients.iter().filter(|(_, _, on)| *on).map(|(_, d, _)| d).collect();
                    if chosen.iter().enumerate().any(|(i, d)| chosen[..i].iter().any(|e| clients::same_folder(e, d))) {
                        ui.label(RichText::new("Clients that share a folder get the patches once.").color(pal.weak));
                    }
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

/// What the confirmation shows for the plan: the Changes, Writes, Undo and Game rows. When the client folder cannot be read the run stops
/// with the reason; a confirmation without the rows is never shown.
fn confirm_rows(plans: &[Plan], files: Result<ClientFiles, Error>) -> Result<Rows, String> {
    match files {
        Ok(files) => Ok(explain::patch_rows(plans, &files)),
        Err(e) => Err(format!("cannot show what would change, so nothing was applied there: {e}")),
    }
}

/// The files of the client's journal folder (best effort: none when it does not exist yet).
fn journal_files(dir: &Path) -> BTreeSet<PathBuf> {
    std::fs::read_dir(dir.join(shaiya_archive::journal::DEFAULT_DIR)).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

fn has_sah(dir: &Path) -> bool {
    dir.is_dir() && std::fs::read_dir(dir).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_client_gives_an_error_and_no_rows() {
        let e = confirm_rows(&[], Err(Error::Cancelled)).unwrap_err();
        assert!(e.contains("nothing was applied there"), "{e}");
    }

    #[test]
    fn start_waits_for_either_apply() {
        assert!(MultiApply::blocked(false, false).is_none());
        assert!(MultiApply::blocked(true, false).unwrap().contains("Archive tab"));
        assert!(MultiApply::blocked(false, true).unwrap().contains("Apply tab"));
    }

    #[test]
    fn the_report_says_partly_changed_and_the_recover_note() {
        let o = Outcome {
            changed: vec![Changed { client: "A".into(), dir: PathBuf::from("a"), applied: 0, of: 2, journals: vec![PathBuf::from("a/j/1.json")], unknown: true }],
            not_touched: vec!["B".into()],
            stopped: Some(("A".into(), "internal error (a bug): x".into())),
            same_folder: vec![("A2".into(), "A".into())],
            recover_may_have_written: true,
        };
        let l = o.lines().join("\n");
        assert!(l.contains("May be partly changed: A") && l.contains("check its history") && l.contains("journals:"), "{l}");
        assert!(!l.contains("Changed: A"), "{l}");
        assert!(l.contains("Not touched: B") && l.contains("unfinished earlier change recovered"), "{l}");
        assert!(l.contains("A2 is the same folder as A"), "{l}");
    }

    #[test]
    fn a_same_folder_client_of_a_client_that_was_not_applied_is_left_out_not_applied_once() {
        let o = Outcome {
            not_touched: vec!["A".into(), "A2".into()],
            stopped: Some(("A".into(), "stopped at your request".into())),
            same_folder: vec![("A2".into(), "A".into())],
            ..Outcome::default()
        };
        let l = o.lines().join("\n");
        assert!(!l.contains("applied there once"), "{l}");
        assert!(l.contains("A2 was left out because A (the same folder) was not applied"), "{l}");
    }

    #[test]
    fn the_same_folder_line_is_only_true_when_the_first_client_was_applied() {
        let o = Outcome {
            changed: vec![Changed { client: "A".into(), dir: PathBuf::from("a"), applied: 1, of: 1, journals: Vec::new(), unknown: false }],
            same_folder: vec![("A2".into(), "A".into())],
            ..Outcome::default()
        };
        let l = o.lines().join("\n");
        assert!(l.contains("A2 is the same folder as A: the patches were applied there once."), "{l}");
        assert!(!l.contains("left out"), "{l}");
    }

    #[test]
    fn two_spellings_of_one_folder_are_one_folder() {
        let base = std::env::temp_dir().join(format!("shaiya-test-spell-{}", std::process::id()));
        let sub = base.join("c");
        std::fs::create_dir_all(&sub).unwrap();
        let other = if cfg!(windows) { PathBuf::from(sub.to_string_lossy().to_uppercase()) } else { base.join("c").join(".").join("..").join("c") };
        assert!(clients::same_folder(&sub, &other));
        assert!(!clients::same_folder(&sub, &base));
        assert_eq!(clients::queued_in(&[("A".to_string(), sub.clone(), 2usize)], &other), 2);
        let _ = std::fs::remove_dir_all(&base);
    }
}
