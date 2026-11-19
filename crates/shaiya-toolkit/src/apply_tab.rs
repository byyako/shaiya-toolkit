//! The Apply & history tab: a list of patches checked for structure and signature, a plan against the client as it is now, an apply
//! that keeps a journal per patch, and the history of the toolkit's changes with rollback and prune.

use crate::archive_view::ArchiveView;
use crate::clients::ClientInfo;
use crate::explain::{self, Rows};
use crate::keys::KeyForm;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::client::{self, ClientFiles};
use shaiya_archive::journal::{self, Journal, JournalMode, Rollback, State};
use shaiya_archive::Archive;
use shaiya_core::time::human_size;
use shaiya_core::Error;
use shaiya_patch::{self as patch, ApplyOptions, Plan, Report, SigStatus, VerifyKey, VerifyReport};
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::PathBuf;
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

enum Verify {
    Running(TaskHandle<VerifyReport>),
    Done(Box<VerifyReport>),
    Failed(String),
}

struct PatchRow {
    path: PathBuf,
    verify: Verify,
}

impl PatchRow {
    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string())
    }
}

enum PlanState {
    None,
    Running(String, TaskHandle<Vec<Plan>>),
    Done(String, Vec<Plan>),
    Failed(String, String),
}

/// What an apply of several patches reports when it ends.
struct Applied {
    reports: Vec<Report>,
    error: Option<Error>,
}

struct ApplyReq {
    patches: Vec<PathBuf>,
    dir: PathBuf,
    name: String,
    key: Option<VerifyKey>,
    /// Whether the client has a usable public key; only the wording of the confirmation depends on it.
    key_set: bool,
    /// Patches applied by earlier attempts of the same request (a Retry continues after them).
    done: Vec<Report>,
}

enum Flow {
    Idle,
    Confirm(ApplyReq),
    Closing(ApplyReq, Option<Arc<Archive>>),
    Running(ApplyReq, TaskHandle<Applied>),
    InUse(ApplyReq),
    RollConfirm(String, PathBuf, String),
    RollClosing(String, PathBuf, String, Option<Arc<Archive>>),
    RollRunning(PathBuf, TaskHandle<journal::RollbackReport>),
    PruneConfirm(PathBuf, usize),
}

pub struct ApplyTab {
    patches: Vec<PatchRow>,
    pub require: bool,
    require_for: Option<String>,
    verify_stamp: String,
    plan: PlanState,
    history: Vec<(Journal, Rollback)>,
    history_stamp: String,
    history_error: Option<String>,
    flow: Flow,
    pub result: Vec<String>,
    pub keep: usize,
    pub changed_counter: u64,
    /// The Changes / Writes / Undo / Game rows of the open confirmation, made once when it was asked for from the same plan the write uses.
    confirm_rows: Rows,
}

impl Default for ApplyTab {
    fn default() -> Self {
        Self::new()
    }
}

fn local_time(iso: &str) -> String {
    iso.replace('T', " ").trim_end_matches('Z').to_string() + " UTC"
}

impl ApplyTab {
    pub fn new() -> ApplyTab {
        ApplyTab {
            patches: Vec::new(),
            require: false,
            require_for: None,
            verify_stamp: String::new(),
            plan: PlanState::None,
            history: Vec::new(),
            history_stamp: String::new(),
            history_error: None,
            flow: Flow::Idle,
            result: Vec::new(),
            keep: 10,
            changed_counter: 0,
            confirm_rows: Vec::new(),
        }
    }

    // ---------------------------------------------------------------------------------------- list

    pub fn add_patches(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if !self.patches.iter().any(|r| r.path == p) {
                self.patches.push(PatchRow { path: p, verify: Verify::Failed("not checked yet".into()) });
            }
        }
        self.verify_stamp.clear();
    }

    pub fn patch_names(&self) -> Vec<String> {
        self.patches.iter().map(|r| r.name()).collect()
    }

    pub fn clear(&mut self) {
        self.patches.clear();
        self.plan = PlanState::None;
    }

    fn move_row(&mut self, i: usize, up: bool) {
        let j = if up { i.wrapping_sub(1) } else { i + 1 };
        if i < self.patches.len() && j < self.patches.len() {
            self.patches.swap(i, j);
        }
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.flow, Flow::Idle)
    }

    // ---------------------------------------------------------------------------------------- settings

    /// The requirement switches on by itself when a client with a usable key is chosen.
    fn sync_require(&mut self, client: Option<&ClientInfo>, keys: &KeyForm) {
        let name = client.map(|c| c.name.clone());
        if self.require_for != name {
            self.require_for = name;
            self.require = keys.verify_key().is_some();
            self.verify_stamp.clear();
        }
    }

    // ---------------------------------------------------------------------------------------- polling

    fn start_verifies(&mut self, keys: &KeyForm, cx: &mut ToolkitCx<'_>) {
        let key = keys.verify_key();
        let stamp = format!(
            "{}|{}",
            self.patches.iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>().join(";"),
            key.as_ref().map(|k| format!("{}:{}", k.public_hex, k.check_version)).unwrap_or_default()
        );
        if stamp == self.verify_stamp {
            return;
        }
        self.verify_stamp = stamp;
        for r in &mut self.patches {
            let p = r.path.clone();
            let k = key.clone();
            let name = r.name();
            let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Checking {name}"), cancellable: true, exclusive: None }, move |job| patch::verify(&p, k.as_ref(), None, job));
            r.verify = Verify::Running(h);
        }
    }

    fn start_plan(&mut self, client: Option<&ClientInfo>, archive_stamp: u64, cx: &mut ToolkitCx<'_>) {
        let Some(c) = client else {
            self.plan = PlanState::None;
            return;
        };
        if self.patches.is_empty() {
            self.plan = PlanState::None;
            return;
        }
        let stamp = format!("{}|{}|{}|{}", self.patches.iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>().join(";"), c.dir.display(), archive_stamp, self.changed_counter);
        let current = match &self.plan {
            PlanState::Running(s, _) | PlanState::Done(s, _) | PlanState::Failed(s, _) => Some(s.clone()),
            PlanState::None => None,
        };
        if current.as_deref() == Some(stamp.as_str()) {
            return;
        }
        let (paths, dir) = (self.patches.iter().map(|r| r.path.clone()).collect::<Vec<_>>(), c.dir.clone());
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Planning the patches".into(), cancellable: true, exclusive: None }, move |_job| patch::plan_seq(&paths, &dir, &[]));
        self.plan = PlanState::Running(stamp, h);
    }

    fn poll(&mut self, client: Option<&ClientInfo>, archive: &mut ArchiveView, cx: &mut ToolkitCx<'_>) {
        for r in &mut self.patches {
            if let Verify::Running(h) = &mut r.verify {
                match h.poll() {
                    None => {}
                    Some(TaskOutcome::Done(v)) => r.verify = Verify::Done(Box::new(v)),
                    Some(TaskOutcome::Failed(e)) => r.verify = Verify::Failed(e.to_string()),
                    Some(TaskOutcome::Cancelled) => r.verify = Verify::Failed("cancelled".into()),
                    Some(TaskOutcome::Panicked(m)) => r.verify = Verify::Failed(format!("internal error (a bug): {m}")),
                }
            }
        }
        if let PlanState::Running(stamp, h) = &mut self.plan {
            let stamp = stamp.clone();
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(p)) => self.plan = PlanState::Done(stamp, p),
                Some(TaskOutcome::Failed(e)) => self.plan = PlanState::Failed(stamp, e.to_string()),
                Some(TaskOutcome::Cancelled) => self.plan = PlanState::Failed(stamp, "cancelled".into()),
                Some(TaskOutcome::Panicked(m)) => self.plan = PlanState::Failed(stamp, format!("internal error (a bug): {m}")),
            }
        }
        // history
        if let Some(c) = client {
            let stamp = format!("{}|{}|{}", c.dir.display(), archive.changed_counter, self.changed_counter);
            if stamp != self.history_stamp && !self.is_busy() {
                self.history_stamp = stamp;
                match journal::list(&c.dir, &c.dir.join(journal::DEFAULT_DIR)) {
                    Ok(l) => {
                        self.history = l;
                        self.history_error = None;
                    }
                    Err(e) => {
                        self.history.clear();
                        self.history_error = Some(e.to_string());
                    }
                }
            }
        } else {
            self.history.clear();
        }
        self.poll_flow(client, archive, cx);
    }

    // ---------------------------------------------------------------------------------------- apply and rollback

    fn plans(&self) -> Option<&Vec<Plan>> {
        match &self.plan {
            PlanState::Done(_, p) => Some(p),
            _ => None,
        }
    }

    /// Why Apply is not possible right now (None: it is).
    pub fn apply_blocked(&self, client: Option<&ClientInfo>) -> Option<String> {
        if client.is_none() {
            return Some("Choose a client folder first".into());
        }
        if self.patches.is_empty() {
            return Some("Add patches first".into());
        }
        if self.is_busy() {
            return Some("Another change is in progress".into());
        }
        if self.patches.iter().any(|r| matches!(r.verify, Verify::Running(_))) {
            return Some("The patches are being checked".into());
        }
        for r in &self.patches {
            match &r.verify {
                Verify::Done(v) => {
                    if !v.problems.is_empty() {
                        return Some(format!("{} has structure problems", r.name()));
                    }
                    if self.require {
                        match &v.signature {
                            SigStatus::Valid => {}
                            SigStatus::Missing => return Some(format!("{} has no signature (a valid signature is required)", r.name())),
                            SigStatus::Invalid(why) => return Some(format!("{}: signature invalid: {why}", r.name())),
                            SigStatus::NotChecked => return Some("A valid signature is required: set the public key and CheckVersion".into()),
                        }
                    }
                }
                Verify::Failed(e) => return Some(format!("{}: {e}", r.name())),
                Verify::Running(_) => {}
            }
        }
        match &self.plan {
            PlanState::Done(..) => None,
            PlanState::Failed(_, e) => Some(format!("The plan failed: {e}")),
            _ => Some("The plan is being made".into()),
        }
    }

    fn confirm_text(&self, req: &ApplyReq) -> String {
        let names: Vec<String> = req.patches.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        let list = if names.len() <= 3 { names.join(" and ") } else { format!("{} patches", names.len()) };
        let sigs = if !req.key_set {
            "not used (unsigned patches)"
        } else if req.key.is_some() && self.patches.iter().all(|r| matches!(&r.verify, Verify::Done(v) if v.signature == SigStatus::Valid)) {
            "valid"
        } else {
            "NOT CHECKED"
        };
        format!("Apply {list} to {}? Signatures: {sigs}.", req.name)
    }

    /// "Apply N patches...": the confirmation, then the exclusive apply.
    pub fn request_apply(&mut self, client: Option<&ClientInfo>, keys: &KeyForm) {
        let Some(c) = client else { return };
        if self.apply_blocked(client).is_some() {
            return;
        }
        self.confirm_rows = match (ClientFiles::locate(&c.dir), self.plans()) {
            (Ok(files), Some(plans)) => explain::patch_rows(plans, &files),
            _ => Vec::new(),
        };
        let req = ApplyReq { patches: self.patches.iter().map(|r| r.path.clone()).collect(), dir: c.dir.clone(), name: c.name.clone(), key: if self.require { keys.verify_key() } else { None }, key_set: keys.verify_key().is_some(), done: Vec::new() };
        self.flow = Flow::Confirm(req);
    }

    fn start_apply(&mut self, req: ApplyReq, cx: &mut ToolkitCx<'_>) {
        let remaining: Vec<PathBuf> = req.patches[req.done.len()..].to_vec();
        let (dir, key) = (req.dir.clone(), req.key.clone());
        let n = remaining.len();
        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {n} patch{}", if n == 1 { "" } else { "es" }), cancellable: true, exclusive: Some(req.dir.display().to_string()) }, move |job| {
            let root = dir.join(journal::DEFAULT_DIR);
            let mut reports = Vec::new();
            let mut error = None;
            for p in &remaining {
                job.stage(&format!("applying {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                let opts = ApplyOptions { protect: Vec::new(), journal: JournalMode::In(root.clone()), legacy_bak: true, require_signature: key.clone(), check_in_use: true };
                match patch::apply_with(p, &dir, &opts, job) {
                    Ok(r) => reports.push(r),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            Ok(Applied { reports, error })
        });
        self.flow = Flow::Running(req, handle);
    }

    /// The newest journal that can be rolled back now.
    pub fn newest_rollback(&self) -> Option<&Journal> {
        self.history.iter().rev().find(|(j, r)| j.state == State::Applied && matches!(r, Rollback::Possible)).map(|(j, _)| j)
    }

    pub fn history(&self) -> &[(Journal, Rollback)] {
        &self.history
    }

    pub fn request_rollback(&mut self, client: Option<&ClientInfo>) {
        let Some(c) = client else { return };
        if self.is_busy() {
            return;
        }
        let Some(j) = self.newest_rollback() else { return };
        let dir = j.dir.clone();
        let text = format!("Roll back {} (applied {})?", j.label, local_time(&j.created_utc));
        self.confirm_rows = match ClientFiles::locate(&c.dir) {
            Ok(files) => explain::rollback_rows(j, &files),
            Err(_) => Vec::new(),
        };
        self.flow = Flow::RollConfirm(text, dir, c.dir.display().to_string());
    }

    pub fn request_prune(&mut self, client: Option<&ClientInfo>) {
        if let Some(c) = client {
            if !self.is_busy() {
                let all: Vec<Journal> = self.history.iter().map(|(j, _)| j.clone()).collect();
                self.confirm_rows = explain::prune_rows(&all, self.keep);
                self.flow = Flow::PruneConfirm(c.dir.clone(), self.keep);
            }
        }
    }

    fn poll_flow(&mut self, client: Option<&ClientInfo>, archive: &mut ArchiveView, cx: &mut ToolkitCx<'_>) {
        match std::mem::replace(&mut self.flow, Flow::Idle) {
            Flow::Idle => {}
            Flow::Confirm(req) => {
                let n = req.patches.len();
                let c = Confirm { title: "Apply patches".into(), text: self.confirm_text(&req), buttons: vec![format!("Apply {n} patch{}", if n == 1 { "" } else { "es" }), "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-apply-patches", &c) {
                    None => self.flow = Flow::Confirm(req),
                    Some(0) => {
                        let arc = if archive.client_dir() == Some(req.dir.as_path()) { archive.release_handle() } else { None };
                        self.flow = Flow::Closing(req, arc);
                    }
                    Some(_) => {}
                }
            }
            Flow::Closing(req, arc) => {
                if arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true) {
                    drop(arc);
                    self.start_apply(req, cx);
                } else {
                    cx.shell.ctx().request_repaint();
                    self.flow = Flow::Closing(req, arc);
                }
            }
            Flow::Running(mut req, mut h) => match h.poll() {
                None => self.flow = Flow::Running(req, h),
                Some(TaskOutcome::Done(done)) => {
                    let Applied { reports, error } = done;
                    req.done.extend(reports);
                    match error {
                        Some(e) if req.done.is_empty() && (e.to_string().contains("open in another program") || e.to_string().contains("is running")) => self.flow = Flow::InUse(req),
                        Some(e) => {
                            self.finish_apply(&req);
                            if matches!(e, Error::Cancelled) {
                                cx.shell.status(format!("Cancelled after {} of {}; the earlier ones stay applied", req.done.len(), shaiya_inspect::counted(req.patches.len() as u64, "patch", "patches")));
                            } else {
                                cx.shell.error(format!("Stopped at patch {} of {}", req.done.len() + 1, req.patches.len()), &e);
                            }
                            self.changed_counter += 1;
                            archive.reopen(cx);
                        }
                        None => {
                            self.finish_apply(&req);
                            cx.shell.status(format!("Applied {} patch{}", req.done.len(), if req.done.len() == 1 { "" } else { "es" }));
                            self.patches.clear();
                            self.plan = PlanState::None;
                            self.changed_counter += 1;
                            archive.changed_counter += 1;
                            archive.reopen(cx);
                        }
                    }
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("The patches were not applied", &e);
                    archive.reopen(cx);
                }
                Some(TaskOutcome::Cancelled) => {
                    cx.shell.status("Cancelled; the archive is as it was");
                    archive.reopen(cx);
                }
                Some(TaskOutcome::Panicked(m)) => {
                    cx.shell.error("The patches were not applied", &Error::Unsupported(format!("internal error (a bug): {m}")));
                    archive.reopen(cx);
                }
            },
            Flow::InUse(req) => {
                let c = Confirm {
                    title: "The archive is in use".into(),
                    text: format!("{}'s data.saf is open in another program (is the game running?). Close it and press Retry.", req.name),
                    buttons: vec!["Retry".into(), "Cancel".into()],
                    danger: None,
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-in-use", &c) {
                    None => self.flow = Flow::InUse(req),
                    Some(0) => self.start_apply(req, cx),
                    Some(_) => archive.reopen(cx),
                }
            }
            Flow::RollConfirm(text, dir, dir_text) => {
                let c = Confirm { title: "Roll back".into(), text: text.clone(), buttons: vec!["Roll back".into(), "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-rollback", &c) {
                    None => self.flow = Flow::RollConfirm(text, dir, dir_text),
                    Some(0) => {
                        let arc = if archive.client_dir().map(|d| d.display().to_string()) == Some(dir_text.clone()) { archive.release_handle() } else { None };
                        self.flow = Flow::RollClosing(text, dir, dir_text, arc);
                    }
                    Some(_) => {}
                }
            }
            Flow::RollClosing(text, dir, dir_text, arc) => {
                if arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true) {
                    drop(arc);
                    let client_dir = PathBuf::from(&dir_text);
                    let jdir = dir.clone();
                    let h = cx.shell.tasks().spawn(TaskSpec { title: "Rolling back".into(), cancellable: false, exclusive: Some(dir_text) }, move |job| {
                        let files = ClientFiles::locate(&client_dir)?;
                        client::ensure_not_in_use(&files)?;
                        journal::rollback(&client_dir, &jdir, false, job)
                    });
                    self.flow = Flow::RollRunning(dir, h);
                } else {
                    cx.shell.ctx().request_repaint();
                    self.flow = Flow::RollClosing(text, dir, dir_text, arc);
                }
            }
            Flow::RollRunning(dir, mut h) => match h.poll() {
                None => self.flow = Flow::RollRunning(dir, h),
                Some(TaskOutcome::Done(r)) => {
                    cx.shell.status(format!("Rolled back: index restored, data.saf cut to {}", shaiya_inspect::counted(r.truncated_to, "byte", "bytes")));
                    self.result = vec![format!("Rolled back: {} restored, {} deleted", shaiya_inspect::counted((r.restored_loose.len()) as u64, "loose file", "loose files"), r.deleted_loose.len())];
                    self.changed_counter += 1;
                    archive.changed_counter += 1;
                    archive.reopen(cx);
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("The rollback was not made", &e);
                    archive.reopen(cx);
                }
                Some(_) => archive.reopen(cx),
            },
            Flow::PruneConfirm(dir, keep) => {
                let c = Confirm { title: "Prune history".into(), text: format!("Delete the backups of all but the newest {}? Those changes can then no longer be rolled back.", shaiya_inspect::counted((keep) as u64, "change", "changes")), buttons: vec!["Prune".into(), "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-prune", &c) {
                    None => self.flow = Flow::PruneConfirm(dir, keep),
                    Some(0) => {
                        match journal::prune(&dir, &dir.join(journal::DEFAULT_DIR), keep) {
                            Ok(gone) => cx.shell.status(format!("Pruned {}", shaiya_inspect::counted(gone.len() as u64, "entry", "entries"))),
                            Err(e) => cx.shell.error("Cannot prune the history", &e),
                        }
                        self.changed_counter += 1;
                    }
                    Some(_) => {}
                }
            }
        }
        let _ = client;
    }

    fn finish_apply(&mut self, req: &ApplyReq) {
        self.result = req
            .done
            .iter()
            .zip(&req.patches)
            .map(|(r, p)| {
                format!(
                    "{}: added {}, replaced {}, removed {}, {} loose{}",
                    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    r.added.len(),
                    r.replaced.len(),
                    r.removed.len(),
                    r.loose.len(),
                    r.journal.as_ref().map(|j| format!(" (backup {})", j.display())).unwrap_or_default()
                )
            })
            .collect();
    }

    // ---------------------------------------------------------------------------------------- tests

    /// Whether the plan is ready, and its numbers (added, replaced, removed, loose) per patch.
    pub fn plan_summary(&self) -> Option<Vec<(usize, usize, usize, usize)>> {
        self.plans().map(|v| v.iter().map(|p| (p.added.len(), p.replaced.len(), p.removed.len(), p.loose.len())).collect())
    }

    /// Per patch: the structure problems and the signature status as text ("valid", "missing", "invalid", "not checked"); `None`
    /// while a check runs.
    pub fn verify_summary(&self) -> Vec<Option<(usize, String)>> {
        self.patches
            .iter()
            .map(|r| match &r.verify {
                Verify::Done(v) => Some((
                    v.problems.len(),
                    match &v.signature {
                        SigStatus::Valid => "valid".to_string(),
                        SigStatus::Missing => "missing".to_string(),
                        SigStatus::Invalid(_) => "invalid".to_string(),
                        SigStatus::NotChecked => "not checked".to_string(),
                    },
                )),
                Verify::Failed(e) => Some((1, e.clone())),
                Verify::Running(_) => None,
            })
            .collect()
    }

    // ---------------------------------------------------------------------------------------- drawing

    /// Work that goes on whichever tab is shown: checks, the plan, history and the apply flows.
    pub fn tick(&mut self, cx: &mut ToolkitCx<'_>, client: Option<&ClientInfo>, archive: &mut ArchiveView, keys: &mut KeyForm) {
        keys.load(client, cx.settings);
        self.sync_require(client, keys);
        self.start_verifies(keys, cx);
        self.start_plan(client, archive.changed_counter, cx);
        self.poll(client, archive, cx);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, client: Option<&ClientInfo>, archive: &mut ArchiveView, keys: &mut KeyForm) {
        self.tick(cx, client, archive, keys);
        let pal = cx.shell.pal();
        let mut remove: Option<usize> = None;
        let mut mv: Option<(usize, bool)> = None;
        let mut add = false;
        let mut clear = false;
        let mut apply = false;
        let mut rollback = false;
        let mut prune = false;
        let mut settings_changed = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Patches");
                if ui.button("Add...").clicked() {
                    add = true;
                }
                if ui.add_enabled(!self.patches.is_empty(), egui::Button::new("Clear")).clicked() {
                    clear = true;
                }
                ui.label(RichText::new("Drop .patch files here. They are applied in the order shown.").color(pal.weak));
            });
            if self.patches.is_empty() {
                ui.label(RichText::new("No patches yet. Add or drop psNNNN.patch files; nothing changes until you press Apply.").color(pal.weak));
            }
            let numbers: Vec<Option<u32>> = self.patches.iter().map(|r| patch::patch_number(&r.name())).collect();
            if numbers.windows(2).any(|w| matches!((w[0], w[1]), (Some(a), Some(b)) if b <= a)) {
                ui.label(RichText::new("Warning: the patch numbers are not ascending.").color(pal.warning));
            }
            let n = self.patches.len();
            for (i, r) in self.patches.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", i + 1));
                    if ui.add_enabled(i > 0, egui::Button::new("Up").small()).clicked() {
                        mv = Some((i, true));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("Down").small()).clicked() {
                        mv = Some((i, false));
                    }
                    ui.label(RichText::new(r.name()).strong());
                    match &r.verify {
                        Verify::Running(_) => {
                            ui.label(RichText::new("checking...").color(pal.weak));
                        }
                        Verify::Failed(e) => {
                            ui.label(RichText::new(e).color(pal.error));
                        }
                        Verify::Done(v) => {
                            if v.problems.is_empty() {
                                ui.label(RichText::new("structure OK").color(pal.ok));
                            } else {
                                ui.label(RichText::new(format!("{}: {}", shaiya_inspect::counted((v.problems.len()) as u64, "problem", "problems"), v.problems[0])).color(pal.error));
                            }
                            let (text, color) = match &v.signature {
                                SigStatus::Valid => ("signature valid".to_string(), pal.ok),
                                SigStatus::Missing => ("signature missing".to_string(), pal.warning),
                                SigStatus::Invalid(why) => (format!("signature INVALID: {why}"), pal.error),
                                SigStatus::NotChecked => ("signature not checked".to_string(), pal.weak),
                            };
                            ui.label(RichText::new(text).color(color));
                        }
                    }
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.add_space(6.0);
            ui.collapsing("Signature settings", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Public key file");
                    settings_changed |= ui.add(egui::TextEdit::singleline(&mut keys.public_path).desired_width(320.0)).changed();
                    if ui.button("Choose...").clicked() {
                        if let Some(p) = cx.shell.pick_file("public-key", &[("Key files", &["key", "pub", "txt"]), ("All files", &["*"])]) {
                            keys.public_path = p.display().to_string();
                            settings_changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("or public key (hex)");
                    settings_changed |= ui.add(egui::TextEdit::singleline(&mut keys.public_hex).desired_width(420.0)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("CheckVersion");
                    settings_changed |= ui.add(egui::TextEdit::singleline(&mut keys.check_version).desired_width(60.0)).changed();
                    let was = self.require;
                    ui.checkbox(&mut self.require, "Require a valid signature");
                    if was != self.require {
                        settings_changed = true;
                    }
                });
                if self.require && keys.verify_key().is_none() {
                    ui.label(RichText::new("A key (file or hex) and a CheckVersion are needed to check signatures.").color(pal.warning));
                }
            });
            if settings_changed && keys.verify_key().is_some() && !self.require {
                // a key that was just set turns the requirement on
                self.require = true;
            }
            ui.add_space(6.0);
            ui.heading("Plan");
            match &self.plan {
                PlanState::None => {
                    ui.label(RichText::new(if client.is_none() { "Choose a client folder to see what the patches would change." } else { "Add patches to see the plan." }).color(pal.weak));
                }
                PlanState::Running(..) => {
                    ui.label("Planning...");
                }
                PlanState::Failed(_, e) => {
                    ui.label(RichText::new(e).color(pal.error));
                }
                PlanState::Done(_, plans) => {
                    for p in plans {
                        let name = p.patch.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        ui.label(RichText::new(format!(
                            "{name}: add {}, replace {}, remove {} ({} not present), {}; appends {}, needs about {} free while applying",
                            p.added.len(),
                            p.replaced.len(),
                            p.removed.len(),
                            p.missing.len(),
                            shaiya_inspect::counted(p.loose.len() as u64, "loose file", "loose files"),
                            human_size(p.bytes),
                            human_size(p.bytes + p.temp_bytes)
                        )).strong());
                        for (title, list) in [("added", &p.added), ("replaced", &p.replaced), ("removed", &p.removed), ("not present", &p.missing)] {
                            if !list.is_empty() {
                                egui::CollapsingHeader::new(format!("{title} ({})", list.len())).id_salt(format!("plan-{name}-{title}")).show(ui, |ui| {
                                    for s in list.iter().take(500) {
                                        ui.label(s);
                                    }
                                    if list.len() > 500 {
                                        ui.label(format!("... and {} more", list.len() - 500));
                                    }
                                });
                            }
                        }
                        for l in &p.loose {
                            let what = if l.overwrites { " (overwrites)" } else { "" };
                            let prot = if l.protected { format!(" as {}", l.target) } else { String::new() };
                            ui.label(format!("loose: {}{prot}, {}{what}", l.name, human_size(l.size)));
                        }
                    }
                }
            }
            ui.add_space(6.0);
            let blocked = self.apply_blocked(client);
            ui.horizontal(|ui| {
                let label = format!("Apply {} patch{}...", self.patches.len(), if self.patches.len() == 1 { "" } else { "es" });
                let r = ui.add_enabled(blocked.is_none(), egui::Button::new(label));
                if r.clicked() {
                    apply = true;
                }
                if let Some(why) = &blocked {
                    if !self.patches.is_empty() {
                        ui.label(RichText::new(why).color(pal.weak));
                    }
                }
            });
            for line in &self.result {
                ui.label(line);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("History");
                if ui.add_enabled(client.is_some() && self.newest_rollback().is_some() && !self.is_busy(), egui::Button::new("Roll back...")).on_hover_text("Undo the newest change the toolkit made").clicked() {
                    rollback = true;
                }
                ui.add(egui::DragValue::new(&mut self.keep).range(0..=1000).prefix("keep "));
                if ui.add_enabled(client.is_some() && !self.history.is_empty() && !self.is_busy(), egui::Button::new("Prune...")).clicked() {
                    prune = true;
                }
            });
            if let Some(e) = &self.history_error {
                ui.label(RichText::new(e).color(pal.error));
            }
            if self.history.is_empty() && self.history_error.is_none() {
                ui.label(RichText::new("No changes made by the toolkit to this client yet.").color(pal.weak));
            }
            egui::Grid::new("toolkit-history").striped(true).show(ui, |ui| {
                if !self.history.is_empty() {
                    for h in ["Time", "Label", "Kind", "State", "Rollback"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                }
                for (j, rb) in self.history.iter().rev() {
                    ui.label(local_time(&j.created_utc));
                    ui.label(&j.label);
                    ui.label(&j.kind);
                    ui.label(match &j.state {
                        State::Pending => "pending".to_string(),
                        State::Applied => "applied".to_string(),
                        State::RolledBack { at_utc } => format!("rolled back {}", local_time(at_utc)),
                    });
                    match rb {
                        Rollback::Possible => {
                            ui.label(RichText::new("possible").color(pal.ok));
                        }
                        Rollback::Blocked(why) => {
                            ui.label(RichText::new("blocked").color(pal.weak)).on_hover_text(why);
                        }
                    }
                    ui.end_row();
                }
            });
        });
        if settings_changed {
            keys.store(cx.settings);
        }
        if add {
            let files = cx.shell.pick_files("patches", &[("Patches", &["patch"]), ("All files", &["*"])]);
            self.add_patches(files);
        }
        if clear {
            self.clear();
        }
        if let Some(i) = remove {
            self.patches.remove(i);
            self.verify_stamp.clear();
        }
        if let Some((i, up)) = mv {
            self.move_row(i, up);
        }
        if !ui.ctx().egui_wants_keyboard_input() && ui.ctx().input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            apply = true;
        }
        if apply {
            self.request_apply(client, keys);
        }
        if rollback {
            self.request_rollback(client);
        }
        if prune {
            self.request_prune(client);
        }
    }
}
