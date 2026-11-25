//! The Apply & history tab: a list of patches checked for structure and signature, a plan against the client as it is now, an apply
//! that keeps a journal per patch, and the history of the toolkit's changes with rollback and prune.

use crate::archive_view::ArchiveView;
use crate::clients::ClientInfo;
use crate::explain::{self, Rows};
use crate::keys::KeyForm;
use crate::multi_lock::{lock_hover, MULTI_RUNNING_HOVER};
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::client::{self, ClientFiles};
use shaiya_archive::journal::{self, Journal, JournalMode, Rollback, State};
use shaiya_archive::loose::LooseLayout;
use shaiya_archive::loose_edit::{self, Busy, LooseJournal, LooseOptions};
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
pub(crate) struct Applied {
    pub(crate) reports: Vec<Report>,
    pub(crate) error: Option<Error>,
}

/// Applies `patches` to the client in `dir`, one `apply_with` call each, in order, stopping at the first error. `apply_with` recovers the
/// client first, checks that the game is not using it and writes one journal per patch; this is the one place the Toolkit calls it, for
/// Apply & history and for "Apply to several test clients".
pub(crate) fn apply_patches_to(patches: &[PathBuf], dir: &std::path::Path, key: Option<VerifyKey>, job: &shaiya_core::Job<'_>) -> Applied {
    let root = dir.join(journal::DEFAULT_DIR);
    let mut reports = Vec::new();
    let mut error = None;
    for p in patches {
        job.stage(&format!("applying {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let opts = ApplyOptions { protect: Vec::new(), journal: JournalMode::In(root.clone()), legacy_bak: true, require_signature: key.clone(), check_in_use: true };
        match patch::apply_with(p, dir, &opts, job) {
            Ok(r) => reports.push(r),
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    Applied { reports, error }
}

/// Applies `patches` to a loose folder, one `apply_loose` call each, in order, stopping at the first error; each patch has its own journal. The
/// reports are shaped like the archive ones so the flow that follows is the same.
pub(crate) fn apply_patches_loose(patches: &[PathBuf], layout: &LooseLayout, unknown_busy_confirmed: bool, job: &shaiya_core::Job<'_>) -> Applied {
    let mut reports = Vec::new();
    let mut error = None;
    for p in patches {
        job.stage(&format!("applying {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let opts = LooseOptions { label: String::new(), patch: true, confirmed_unknown_busy: unknown_busy_confirmed, crash_after_puts: None };
        match patch::loose::apply_loose(p, layout, &opts, &shaiya_inspect::loose::format_check, job) {
            Ok(r) => reports.push(Report { added: r.added, replaced: r.replaced, removed: r.removed, missing: Vec::new(), loose: Vec::new(), bytes: 0, journal: Some(r.journal) }),
            Err(e) => {
                error = Some(e);
                break;
            }
        }
    }
    Applied { reports, error }
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
    /// A loose client: its layout with the confirmed backup folder (the patches go through the loose journal).
    loose: Option<Box<LooseLayout>>,
    /// The person confirms that the game is stopped because this system cannot tell.
    unknown_busy: bool,
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
    LooseRollConfirm(String, PathBuf, Box<LooseLayout>, bool),
    LooseRollRunning(PathBuf, TaskHandle<loose_edit::RollbackReport>),
    LoosePruneConfirm(Box<LooseLayout>, usize),
}

/// What one patch would do to a loose folder (planned against the folder as it is now).
#[derive(Clone, Debug)]
pub struct LoosePatchPlan {
    pub name: String,
    pub summary: explain::LoosePatchSummary,
    pub error: Option<String>,
}

enum LoosePlanState {
    None,
    Running(String, TaskHandle<Vec<LoosePatchPlan>>),
    Done(String, Vec<LoosePatchPlan>),
    Failed(String, String),
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
    /// Set by the "Apply to several test clients..." button; the app takes it and opens that dialog.
    pub want_multi: bool,
    /// True while "Apply to several test clients" runs (set by the app every frame): apply, roll back and prune are off.
    multi_running: bool,
    pub changed_counter: u64,
    /// The Changes / Writes / Undo / Game rows of the open confirmation, made once when it was asked for, from the dry-run plan the Apply tab shows (the write itself, `apply_with`, plans again when it runs).
    confirm_rows: Rows,
    /// The confirmed backup folder of the open loose client (set by the app every frame).
    loose_backups: Option<PathBuf>,
    /// The open client as a loose folder, with its backup folder (set by `tick`); `None` for an archive client.
    loose: Option<LooseLayout>,
    loose_plan: LoosePlanState,
    loose_history: Vec<(LooseJournal, Rollback)>,
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
            want_multi: false,
            multi_running: false,
            changed_counter: 0,
            confirm_rows: Vec::new(),
            loose_backups: None,
            loose: None,
            loose_plan: LoosePlanState::None,
            loose_history: Vec::new(),
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

    /// The listed patches, in the order they are applied.
    pub fn patch_paths(&self) -> Vec<PathBuf> {
        self.patches.iter().map(|r| r.path.clone()).collect()
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

    /// The app tells the tab every frame whether "Apply to several test clients" runs.
    pub fn set_multi_running(&mut self, on: bool) {
        self.multi_running = on;
    }

    /// The app tells the tab every frame the confirmed backup folder of the open client (`None` when it is not a loose client or has none).
    pub fn set_loose_backups(&mut self, backups: Option<PathBuf>) {
        self.loose_backups = backups;
    }

    /// The open client as a loose folder whose backup folder the rules accept; `Err(reason)` when it is loose and read only.
    pub fn loose_write(&self) -> Option<Result<LooseLayout, &'static str>> {
        let l = self.loose.as_ref()?;
        Some(match &l.backups {
            Some(b) if shaiya_archive::loose::backup_folder_problem(l, b).is_none() => Ok(l.clone()),
            _ => Err(crate::archive_view::LOOSE_NEEDS_BACKUPS),
        })
    }

    /// False while a multi-client apply runs: Apply, Roll back (also Undo last change) and Prune are off then.
    pub fn write_actions_enabled(&self) -> bool {
        !self.multi_running
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

    /// The plan of the listed patches against the open loose folder (each patch as the folder is now; nothing is written).
    fn start_loose_plan(&mut self, archive_stamp: u64, cx: &mut ToolkitCx<'_>) {
        let Some(layout) = self.loose.clone() else { return };
        if self.patches.is_empty() {
            self.loose_plan = LoosePlanState::None;
            return;
        }
        let stamp = format!("{}|{}|{}|{}", self.patches.iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>().join(";"), layout.data.display(), archive_stamp, self.changed_counter);
        let current = match &self.loose_plan {
            LoosePlanState::Running(s, _) | LoosePlanState::Done(s, _) | LoosePlanState::Failed(s, _) => Some(s.clone()),
            LoosePlanState::None => None,
        };
        if current.as_deref() == Some(stamp.as_str()) {
            return;
        }
        let paths: Vec<PathBuf> = self.patches.iter().map(|r| r.path.clone()).collect();
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Planning the patches".into(), cancellable: true, exclusive: None }, move |job| {
            let mut out = Vec::new();
            for p in &paths {
                job.check()?;
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                match patch::loose::plan_loose(p, &layout, &shaiya_inspect::loose::format_check, job) {
                    Ok((plan, missing)) => {
                        let (added, replaced, removed) = plan.counts();
                        let names = plan.puts.iter().map(|x| x.rel.clone()).chain(plan.removes.iter().map(|x| x.rel.clone())).collect();
                        out.push(LoosePatchPlan { name, summary: explain::LoosePatchSummary { added, replaced, removed, missing: missing.len(), bytes: plan.bytes_written(), names }, error: None });
                    }
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => out.push(LoosePatchPlan { name, summary: explain::LoosePatchSummary::default(), error: Some(e.to_string()) }),
                }
            }
            Ok(out)
        });
        self.loose_plan = LoosePlanState::Running(stamp, h);
    }

    fn loose_plans(&self) -> Option<&Vec<LoosePatchPlan>> {
        match &self.loose_plan {
            LoosePlanState::Done(_, p) => Some(p),
            _ => None,
        }
    }

    fn poll(&mut self, client: Option<&ClientInfo>, archive: &mut ArchiveView, cx: &mut ToolkitCx<'_>) {
        if let LoosePlanState::Running(stamp, h) = &mut self.loose_plan {
            let stamp = stamp.clone();
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(p)) => self.loose_plan = LoosePlanState::Done(stamp, p),
                Some(TaskOutcome::Failed(e)) => self.loose_plan = LoosePlanState::Failed(stamp, e.to_string()),
                Some(TaskOutcome::Cancelled) => self.loose_plan = LoosePlanState::Failed(stamp, "cancelled".into()),
                Some(TaskOutcome::Panicked(m)) => self.loose_plan = LoosePlanState::Failed(stamp, format!("internal error (a bug): {m}")),
            }
        }
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
        if let (Some(c), Some(layout)) = (client, self.loose.clone()) {
            let stamp = format!("{}|{}|{}|{:?}", c.dir.display(), archive.changed_counter, self.changed_counter, layout.backups);
            if stamp != self.history_stamp && !self.is_busy() {
                self.history_stamp = stamp;
                self.history.clear();
                match (&layout.backups, self.loose_write()) {
                    (Some(_), Some(Ok(_))) => match loose_edit::list(&layout) {
                        Ok(l) => {
                            self.loose_history = l;
                            self.history_error = None;
                        }
                        Err(e) => {
                            self.loose_history.clear();
                            self.history_error = Some(e.to_string());
                        }
                    },
                    _ => {
                        self.loose_history.clear();
                        self.history_error = None;
                    }
                }
            }
        } else if let Some(c) = client {
            self.loose_history.clear();
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
        if !self.write_actions_enabled() {
            return Some(MULTI_RUNNING_HOVER.into());
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
        if let Some(w) = self.loose_write() {
            if let Err(why) = w {
                return Some(why.to_string());
            }
            return match &self.loose_plan {
                LoosePlanState::Done(_, plans) => plans.iter().find_map(|p| p.error.as_ref().map(|e| format!("{}: {e}", p.name))),
                LoosePlanState::Failed(_, e) => Some(format!("The plan failed: {e}")),
                _ => Some("The plan is being made".into()),
            };
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
        let (mut loose, mut unknown_busy) = (None, false);
        if let Some(Ok(layout)) = self.loose_write() {
            let busy = loose_edit::busy_state(&layout);
            unknown_busy = busy == Busy::Unknown;
            let summaries: Vec<explain::LoosePatchSummary> = self.loose_plans().map(|v| v.iter().map(|p| p.summary.clone()).collect()).unwrap_or_default();
            self.confirm_rows = explain::loose_patch_rows(&summaries, &layout, &busy);
            loose = Some(Box::new(layout));
        } else {
            self.confirm_rows = match (ClientFiles::locate(&c.dir), self.plans()) {
                (Ok(files), Some(plans)) => explain::patch_rows(plans, &files),
                _ => Vec::new(),
            };
        }
        let req = ApplyReq { patches: self.patches.iter().map(|r| r.path.clone()).collect(), dir: c.dir.clone(), name: c.name.clone(), key: if self.require { keys.verify_key() } else { None }, key_set: keys.verify_key().is_some(), done: Vec::new(), loose, unknown_busy };
        self.flow = Flow::Confirm(req);
    }

    fn start_apply(&mut self, req: ApplyReq, cx: &mut ToolkitCx<'_>) {
        let remaining: Vec<PathBuf> = req.patches[req.done.len()..].to_vec();
        let (dir, key) = (req.dir.clone(), req.key.clone());
        let n = remaining.len();
        if let Some(layout) = req.loose.clone() {
            let unknown = req.unknown_busy;
            let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {n} patch{}", if n == 1 { "" } else { "es" }), cancellable: true, exclusive: Some(req.dir.display().to_string()) }, move |job| Ok(apply_patches_loose(&remaining, &layout, unknown, job)));
            self.flow = Flow::Running(req, handle);
            return;
        }
        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {n} patch{}", if n == 1 { "" } else { "es" }), cancellable: true, exclusive: Some(req.dir.display().to_string()) }, move |job| {
            Ok(apply_patches_to(&remaining, &dir, key.clone(), job))
        });
        self.flow = Flow::Running(req, handle);
    }

    /// The newest applied change when it can be rolled back now, otherwise why it cannot. This is the one rule behind the Roll back
    /// button, Edit > Undo last applied change and the toolbar button: the reason is `journal::list`'s own `Rollback::Blocked` text.
    pub fn undo_target(&self, client_open: bool) -> Result<&Journal, String> {
        if !client_open {
            return Err("Open a client folder first.".to_string());
        }
        if self.is_busy() {
            return Err("Wait for the running task to finish.".to_string());
        }
        if !self.write_actions_enabled() {
            return Err(MULTI_RUNNING_HOVER.to_string());
        }
        match self.history.iter().rev().find(|(j, _)| j.state == State::Applied) {
            None => Err("No change made by the toolkit to this client can be undone yet.".to_string()),
            Some((j, Rollback::Possible)) => Ok(j),
            Some((j, Rollback::Blocked(why))) => Err(format!("The newest change ({}) cannot be rolled back: {why}", j.label)),
        }
    }

    /// Like [`undo_target`](Self::undo_target) for either kind of client: the label of the change that would be rolled back, or why not.
    pub fn undo_label(&self, client_open: bool) -> Result<String, String> {
        if self.loose.is_none() {
            return self.undo_target(client_open).map(|j| j.label.clone());
        }
        if !client_open {
            return Err("Open a client folder first.".to_string());
        }
        if self.is_busy() {
            return Err("Wait for the running task to finish.".to_string());
        }
        if !self.write_actions_enabled() {
            return Err(MULTI_RUNNING_HOVER.to_string());
        }
        if let Some(Err(why)) = self.loose_write() {
            return Err(why.to_string());
        }
        match self.loose_history.iter().rev().find(|(j, _)| j.state == State::Applied) {
            None => Err("No change made by the toolkit to this client can be undone yet.".to_string()),
            Some((j, Rollback::Possible)) => Ok(j.label.clone()),
            Some((j, Rollback::Blocked(why))) => Err(format!("The newest change ({}) cannot be rolled back: {why}", j.label)),
        }
    }

    /// The rows of the open confirmation (Changes, Writes, Undo, Game), for tests.
    pub fn confirm_rows(&self) -> &[(String, String)] {
        &self.confirm_rows
    }

    fn has_history(&self) -> bool {
        !self.history.is_empty() || !self.loose_history.is_empty()
    }

    fn can_roll_back(&self) -> bool {
        if self.loose.is_some() {
            self.loose_history.iter().any(|(j, r)| j.state == State::Applied && matches!(r, Rollback::Possible))
        } else {
            self.newest_rollback().is_some()
        }
    }

    /// The rows of the history table, newest first: (created, label, kind, state, rollback).
    fn history_rows(&self) -> Vec<(String, String, String, State, Rollback)> {
        let archive = self.history.iter().rev().map(|(j, r)| (j.created_utc.clone(), j.label.clone(), j.kind.clone(), j.state.clone(), r.clone()));
        let loose = self.loose_history.iter().rev().map(|(j, r)| (j.created_utc.clone(), j.label.clone(), j.kind.clone(), j.state.clone(), r.clone()));
        archive.chain(loose).collect()
    }

    /// The loose history, oldest first (empty for an archive client).
    pub fn loose_history(&self) -> &[(LooseJournal, Rollback)] {
        &self.loose_history
    }

    /// The newest journal that can be rolled back now.
    pub fn newest_rollback(&self) -> Option<&Journal> {
        self.history.iter().rev().find(|(j, r)| j.state == State::Applied && matches!(r, Rollback::Possible)).map(|(j, _)| j)
    }

    /// Reads the history again on the next frame (a journal may have been written outside the Toolkit).
    pub fn refresh_history(&mut self) {
        self.history_stamp.clear();
    }

    pub fn history(&self) -> &[(Journal, Rollback)] {
        &self.history
    }

    pub fn request_rollback(&mut self, client: Option<&ClientInfo>) {
        let Some(c) = client else { return };
        if self.is_busy() {
            return;
        }
        if self.loose.is_some() {
            if self.undo_label(true).is_err() {
                return;
            }
            let (Some(Ok(layout)), Some((j, _))) = (self.loose_write(), self.loose_history.iter().rev().find(|(j, r)| j.state == State::Applied && matches!(r, Rollback::Possible))) else { return };
            let busy = loose_edit::busy_state(&layout);
            self.confirm_rows = explain::loose_rollback_rows(j, &busy);
            let text = format!("Roll back {} (applied {})?", j.label, local_time(&j.created_utc));
            self.flow = Flow::LooseRollConfirm(text, j.dir.clone(), Box::new(layout), busy == Busy::Unknown);
            return;
        }
        let Ok(j) = self.undo_target(true) else { return };
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
            if !self.is_busy() && self.write_actions_enabled() {
                if let Some(Ok(layout)) = self.loose_write() {
                    let finished: Vec<String> = self.loose_history.iter().filter(|(j, _)| j.state != State::Pending).map(|(j, _)| j.id.clone()).collect();
                    let gone: Vec<String> = finished.iter().take(finished.len().saturating_sub(self.keep)).cloned().collect();
                    self.confirm_rows = vec![
                        (explain::CHANGES.to_string(), format!("{} deleted, the newest {} kept", shaiya_inspect::counted(gone.len() as u64, "journal", "journals"), self.keep)),
                        (explain::WRITES.to_string(), if gone.is_empty() { "nothing in the backup folder".to_string() } else { format!("the backup folder loses {}", gone.join(", ")) }),
                        (explain::UNDO.to_string(), "no undo: the deleted journals cannot be brought back, so those changes can no longer be rolled back".to_string()),
                        (explain::GAME.to_string(), "not involved: only backup folders are deleted, no game file is touched".to_string()),
                    ];
                    self.flow = Flow::LoosePruneConfirm(Box::new(layout), self.keep);
                    return;
                }
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
                let (text, button) = if req.loose.is_some() && req.unknown_busy {
                    (format!("{} {}.", self.confirm_text(&req), loose_edit::UNKNOWN_BUSY_TEXT), "The game is stopped: apply".to_string())
                } else {
                    (self.confirm_text(&req), format!("Apply {n} patch{}", if n == 1 { "" } else { "es" }))
                };
                let c = Confirm { title: "Apply patches".into(), text, buttons: vec![button, "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
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
                let loose = req.loose.is_some();
                let c = Confirm {
                    title: if loose { "The game is running".into() } else { "The archive is in use".into() },
                    text: if loose {
                        format!("The game of {} is running, or one of its files is open in another program. Close it and press Retry.", req.name)
                    } else {
                        format!("{}'s data.saf is open in another program (is the game running?). Close it and press Retry.", req.name)
                    },
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
            Flow::LooseRollConfirm(text, jdir, layout, unknown) => {
                let button = if unknown { "The game is stopped: roll back" } else { "Roll back" };
                let text2 = if unknown { format!("{text} {}.", loose_edit::UNKNOWN_BUSY_TEXT) } else { text.clone() };
                let c = Confirm { title: "Roll back".into(), text: text2, buttons: vec![button.into(), "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-rollback", &c) {
                    None => self.flow = Flow::LooseRollConfirm(text, jdir, layout, unknown),
                    Some(0) => {
                        let (l2, j2) = ((*layout).clone(), jdir.clone());
                        let h = cx.shell.tasks().spawn(TaskSpec { title: "Rolling back".into(), cancellable: false, exclusive: Some(layout.root.display().to_string()) }, move |job| {
                            loose_edit::rollback(&l2, &j2, false, &LooseOptions { confirmed_unknown_busy: unknown, ..LooseOptions::default() }, job)
                        });
                        self.flow = Flow::LooseRollRunning(jdir, h);
                    }
                    Some(_) => {}
                }
            }
            Flow::LooseRollRunning(jdir, mut h) => match h.poll() {
                None => self.flow = Flow::LooseRollRunning(jdir, h),
                Some(TaskOutcome::Done(r)) => {
                    cx.shell.status(format!("Rolled back: {} restored, {} deleted", r.restored.len(), r.deleted.len()));
                    self.result = vec![format!("Rolled back: {} restored, {} deleted", shaiya_inspect::counted(r.restored.len() as u64, "file", "files"), r.deleted.len())];
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
            Flow::LoosePruneConfirm(layout, keep) => {
                let c = Confirm { title: "Prune history".into(), text: format!("Delete the backups of all but the newest {}? Those changes can then no longer be rolled back.", shaiya_inspect::counted((keep) as u64, "change", "changes")), buttons: vec!["Prune".into(), "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-prune", &c) {
                    None => self.flow = Flow::LoosePruneConfirm(layout, keep),
                    Some(0) => {
                        match loose_edit::prune(&layout, keep) {
                            Ok(gone) => cx.shell.status(format!("Pruned {}", shaiya_inspect::counted(gone.len() as u64, "entry", "entries"))),
                            Err(e) => cx.shell.error("Cannot prune the history", &e),
                        }
                        self.changed_counter += 1;
                    }
                    Some(_) => {}
                }
            }
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

    /// The plan of the listed patches against an open loose folder, once it is ready: (added, replaced, removed) per patch; a patch the
    /// rules refuse is `None` (its reason is in the plan list and blocks Apply).
    pub fn loose_plan_summary(&self) -> Option<Vec<Option<(usize, usize, usize)>>> {
        self.loose_plans().map(|v| v.iter().map(|p| p.error.is_none().then_some((p.summary.added, p.summary.replaced, p.summary.removed))).collect())
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
        self.loose = archive.loose_layout(self.loose_backups.clone());
        if self.loose.is_some() {
            self.start_loose_plan(archive.changed_counter, cx);
        } else {
            self.loose_plan = LoosePlanState::None;
            self.start_plan(client, archive.changed_counter, cx);
        }
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
            if self.loose.is_some() {
                match (self.loose_write(), &self.loose_plan) {
                    (Some(Err(why)), _) => {
                        ui.label(RichText::new(format!("This loose folder is read only: {why}")).color(pal.warning));
                    }
                    (_, LoosePlanState::None) => {
                        ui.label(RichText::new(if client.is_none() { "Choose a client folder to see what the patches would change." } else { "Add patches to see the plan." }).color(pal.weak));
                    }
                    (_, LoosePlanState::Running(..)) => {
                        ui.label("Planning...");
                    }
                    (_, LoosePlanState::Failed(_, e)) => {
                        ui.label(RichText::new(e).color(pal.error));
                    }
                    (_, LoosePlanState::Done(_, plans)) => {
                        for p in plans {
                            match &p.error {
                                Some(e) => {
                                    ui.label(RichText::new(format!("{}: refused, nothing would be written: {e}", p.name)).color(pal.error));
                                }
                                None => {
                                    let s = &p.summary;
                                    ui.label(RichText::new(format!("{}: add {}, replace {}, remove {} ({} not present); writes {}", p.name, s.added, s.replaced, s.removed, s.missing, human_size(s.bytes))).strong());
                                }
                            }
                        }
                        ui.label(RichText::new("Each patch is planned against the folder as it is now and checked again when it is applied.").color(pal.weak));
                    }
                }
            }
            if self.loose.is_none() {
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
                if ui.add_enabled(!self.is_busy(), egui::Button::new("Apply to several test clients...")).on_hover_text("Pick patches and saved clients; each client gets its own confirmation and journal, one after the other").clicked() {
                    self.want_multi = true;
                }
            });
            for line in &self.result {
                ui.label(line);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("History");
                let locked = !self.write_actions_enabled();
                if lock_hover(ui.add_enabled(client.is_some() && self.can_roll_back() && !self.is_busy() && !locked, egui::Button::new("Roll back...")).on_hover_text("Undo the newest change the toolkit made"), locked).clicked() {
                    rollback = true;
                }
                ui.add(egui::DragValue::new(&mut self.keep).range(0..=1000).prefix("keep "));
                if lock_hover(ui.add_enabled(client.is_some() && self.has_history() && !self.is_busy() && !locked, egui::Button::new("Prune...")), locked).clicked() {
                    prune = true;
                }
            });
            if let Some(e) = &self.history_error {
                ui.label(RichText::new(e).color(pal.error));
            }
            if let Some(Err(why)) = self.loose_write() {
                ui.label(RichText::new(format!("No history: {why}")).color(pal.weak));
            } else if !self.has_history() && self.history_error.is_none() {
                ui.label(RichText::new("No changes made by the toolkit to this client yet.").color(pal.weak));
            }
            egui::Grid::new("toolkit-history").striped(true).show(ui, |ui| {
                if self.has_history() {
                    for h in ["Time", "Label", "Kind", "State", "Rollback"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                }
                for (created, label, kind, state, rb) in self.history_rows() {
                    ui.label(local_time(&created));
                    ui.label(&label);
                    ui.label(&kind);
                    ui.label(match &state {
                        State::Pending => "pending".to_string(),
                        State::Applied => "applied".to_string(),
                        State::RolledBack { at_utc } => format!("rolled back {}", local_time(at_utc)),
                    });
                    match &rb {
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
