//! The Patch builder tab: three sources (archive files, loose files, removals), or the pending changes of the Archive tab, or the
//! differences of two archives, a merged contents table with problems and effects on the selected client, and a streaming build
//! that can sign the result.

use crate::archive_view::ArchiveView;
use crate::clients::ClientInfo;
use crate::keys::KeyForm;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::Archive;
use shaiya_core::fs::Existing;
use shaiya_core::glob::Filter;
use shaiya_core::time::{human_size, utc_iso_unix};
use shaiya_core::Error;
use shaiya_inspect::validate::{self, Finding};
use shaiya_patch::{BuildInput, BuildOptions, Report};
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// What the builder asks the app frame to do after a build.
pub enum PatchRequest {
    ApplyPatch(PathBuf),
    VerifyPatch(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: &'static str,
    pub path: String,
    pub size: u64,
    pub version: i32,
    pub effect: String,
    pub problem: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Files,
    Loose,
    Remove,
}

struct BuildDone {
    report: Report,
    out: PathBuf,
    size: u64,
    signed: bool,
}

enum BuildFlow {
    Idle,
    /// Build pressed while the file checks of the current contents are still running.
    WaitCheck(PathBuf),
    /// The "Build anyway" question about files that look broken.
    Ask(PathBuf),
    Confirm(PathBuf),
    Running(PathBuf, TaskHandle<BuildDone>),
}

pub struct PatchBuilder {
    pub files_dir: Option<PathBuf>,
    pub loose_dir: Option<PathBuf>,
    pub remove_list: Option<PathBuf>,
    /// Pending changes or differences, as an extra source: (label, input).
    pub extra: Option<(String, BuildInput)>,
    input: BuildInput,
    pub rows: Vec<Row>,
    pub problems: Vec<String>,
    pub resolve_error: Option<String>,
    dirty: bool,
    pub out_name: String,
    name_edited: bool,
    name_client: Option<String>,
    pub out_dir: String,
    pub fixed_date: bool,
    pub date_text: String,
    pub sign: bool,
    pub secret_path: String,
    pub check_version: String,
    flow: BuildFlow,
    pub result: Vec<String>,
    pub last_out: Option<PathBuf>,
    diff: Option<TaskHandle<(String, BuildInput)>>,
    effect_stamp: String,
    /// File checks of the current contents (see `shaiya_inspect::validate`); valid when `checked_gen == Some(input_gen)`.
    findings: Vec<Finding>,
    check: Option<(u64, TaskHandle<Vec<Finding>>)>,
    input_gen: u64,
    checked_gen: Option<u64>,
}

impl Default for PatchBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn walk_count(p: &Path) -> usize {
    fn go(p: &Path, n: &mut usize) {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    go(&path, n);
                } else {
                    *n += 1;
                }
            }
        }
    }
    let mut n = 0;
    go(p, &mut n);
    n
}

impl PatchBuilder {
    pub fn new() -> PatchBuilder {
        PatchBuilder {
            files_dir: None,
            loose_dir: None,
            remove_list: None,
            extra: None,
            input: BuildInput::default(),
            rows: Vec::new(),
            problems: Vec::new(),
            resolve_error: None,
            dirty: true,
            out_name: shaiya_patch::patch_name(1),
            name_edited: false,
            name_client: None,
            out_dir: String::new(),
            fixed_date: false,
            date_text: "2026-01-01".into(),
            sign: false,
            secret_path: String::new(),
            check_version: String::new(),
            flow: BuildFlow::Idle,
            result: Vec::new(),
            last_out: None,
            diff: None,
            effect_stamp: String::new(),
            findings: Vec::new(),
            check: None,
            input_gen: 1,
            checked_gen: None,
        }
    }

    // ---------------------------------------------------------------------------------------- sources

    pub fn is_busy(&self) -> bool {
        !matches!(self.flow, BuildFlow::Idle) || self.diff.is_some()
    }

    pub fn set_card(&mut self, card: Card, path: Option<PathBuf>) {
        match card {
            Card::Files => self.files_dir = path,
            Card::Loose => self.loose_dir = path,
            Card::Remove => self.remove_list = path,
        }
        self.dirty = true;
    }

    /// Dropped paths: a folder fills "Archive files", a text or TSV file fills "Removals", other files go to "Loose files" (their
    /// folder), as the three drop cards cannot be told apart without a pointer position.
    pub fn drop_paths(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if p.is_dir() {
                self.set_card(Card::Files, Some(p));
            } else {
                let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if matches!(ext.as_str(), "txt" | "tsv" | "csv" | "lst") {
                    self.set_card(Card::Remove, Some(p));
                } else if let Some(parent) = p.parent() {
                    self.set_card(Card::Loose, Some(parent.to_path_buf()));
                }
            }
        }
    }

    pub fn clear_all(&mut self) {
        self.files_dir = None;
        self.loose_dir = None;
        self.remove_list = None;
        self.extra = None;
        self.dirty = true;
    }

    /// "From pending changes": the Archive tab's queue as the extra source.
    pub fn from_pending(&mut self, input: BuildInput) {
        let n = input.files.len() + input.removals.len();
        self.extra = Some((format!("{} of the Archive tab", shaiya_inspect::counted((n) as u64, "pending change", "pending changes")), input));
        self.dirty = true;
    }

    /// "From differences": added and different files of `new` over `old` (content compare) as a task.
    pub fn from_diff(&mut self, old: PathBuf, new: Option<PathBuf>, archive: &ArchiveView, cx: &mut ToolkitCx<'_>) {
        let Some(newer) = new.or_else(|| archive.sah_path().map(Path::to_path_buf)) else {
            cx.shell.status("Open an archive in the Archive tab first");
            return;
        };
        let label = format!("differences {} -> {}", old.display(), newer.display());
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Comparing archives for a patch".into(), cancellable: true, exclusive: None }, move |job| {
            let a = Archive::open(&old)?;
            let b = Archive::open(&newer)?;
            let input = BuildInput::from_diff(&a, &b, &Filter::default(), job)?;
            Ok((label, input))
        });
        self.diff = Some(h);
    }

    /// Merges the cards and the extra source into one build input (called when something changed).
    fn resolve(&mut self) {
        self.dirty = false;
        self.resolve_error = None;
        let mut input = match BuildInput::from_folders(self.files_dir.as_deref(), self.loose_dir.as_deref(), self.remove_list.as_deref(), None) {
            Ok(i) => i,
            Err(e) => {
                self.resolve_error = Some(e.to_string());
                BuildInput::default()
            }
        };
        if let Some((_, extra)) = &self.extra {
            input.files.extend(extra.files.iter().cloned());
            input.loose.extend(extra.loose.iter().cloned());
            input.removals.extend(extra.removals.iter().cloned());
        }
        if self.fixed_date {
            if let Ok(t) = shaiya_core::time::parse_utc(&self.date_text) {
                let t = t.clamp(0, i32::MAX as i64) as i32;
                for f in &mut input.files {
                    f.2 = t;
                }
            }
        }
        self.problems = shaiya_patch::input_problems(&input);
        self.input_gen += 1;
        self.input = input;
        self.effect_stamp.clear();
    }

    fn refresh_rows(&mut self, client: Option<&ClientInfo>, archive: &ArchiveView) {
        let stamp = format!("{}|{}|{}", client.map(|c| c.dir.display().to_string()).unwrap_or_default(), archive.changed_counter, self.input.files.len() + self.input.loose.len() + self.input.removals.len());
        if stamp == self.effect_stamp {
            return;
        }
        self.effect_stamp = stamp;
        let on_client = client.is_some() && archive.client_dir() == client.map(|c| c.dir.as_path());
        let bad = |p: &str| self.problems.iter().any(|m| m.contains(p));
        let mut rows = Vec::new();
        for (path, src, version) in &self.input.files {
            let size = src.len().unwrap_or(0);
            let effect = if !on_client {
                String::new()
            } else {
                match archive.lookup(path) {
                    Some((s, _)) => format!("replaces {}", human_size(s as u64)),
                    None => "new".into(),
                }
            };
            rows.push(Row { kind: "archive", path: path.clone(), size, version: *version, effect, problem: bad(path) });
        }
        for (name, file) in &self.input.loose {
            let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            let effect = match client {
                Some(c) => {
                    let target = name.replace('\\', "/").split('/').fold(c.dir.clone(), |p, s| p.join(s));
                    if target.exists() { "overwrites".to_string() } else { "new".to_string() }
                }
                None => String::new(),
            };
            rows.push(Row { kind: "loose", path: name.clone(), size, version: 0, effect, problem: bad(name) });
        }
        for p in &self.input.removals {
            let effect = if !on_client { String::new() } else if archive.lookup(p).is_some() { "removes".to_string() } else { "not present".to_string() };
            rows.push(Row { kind: "remove", path: p.clone(), size: 0, version: 0, effect, problem: bad(p) });
        }
        self.rows = rows;
    }

    // ---------------------------------------------------------------------------------------- output

    fn suggest_name(&mut self, client: Option<&ClientInfo>) {
        let key = client.map(|c| c.name.clone());
        if self.name_edited || self.name_client == key {
            return;
        }
        self.name_client = key;
        self.out_name = shaiya_patch::patch_name(client.and_then(|c| c.current_version).map(|v| v + 1).unwrap_or(1));
    }

    pub fn out_path(&self) -> Option<PathBuf> {
        let dir = self.out_dir.trim();
        let name = self.out_name.trim();
        if dir.is_empty() || name.is_empty() {
            None
        } else {
            Some(Path::new(dir).join(name))
        }
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if shaiya_patch::patch_number(self.out_name.trim()).is_none() {
            w.push("The updater only fetches psNNNN.patch; use that name for patches you publish.".to_string());
        }
        if !self.input.removals.is_empty() {
            w.push("remove.txt is honoured by the toolkit and the replacement updater, not by the original Updater.exe.".to_string());
        }
        if self.rows.is_empty() {
            w.push("The patch is empty.".to_string());
        }
        w
    }

    pub fn build_blocked(&self, keys: &KeyForm) -> Option<String> {
        if self.is_busy() {
            return Some("Busy".into());
        }
        if self.rows.is_empty() {
            return Some("Nothing to build yet".into());
        }
        if !self.problems.is_empty() {
            return Some(format!("{} in the contents", shaiya_inspect::counted((self.problems.len()) as u64, "problem", "problems")));
        }
        if self.out_path().is_none() {
            return Some("Choose the output folder and file name".into());
        }
        if self.sign {
            if self.secret_path.trim().is_empty() {
                return Some("Choose the secret key file to sign with".into());
            }
            if self.check_version.trim().parse::<u32>().is_err() {
                return Some("Enter the CheckVersion (a number)".into());
            }
        }
        let _ = keys;
        None
    }

    /// "Build": the overwrite question when the output exists, then the build task.
    pub fn begin_build(&mut self, keys: &KeyForm, cx: &mut ToolkitCx<'_>) {
        if self.build_blocked(keys).is_some() {
            return;
        }
        let Some(out) = self.out_path() else { return };
        self.after_checks(out, cx);
    }

    /// The file checks of the current contents have finished.
    fn checks_current(&self) -> bool {
        self.checked_gen == Some(self.input_gen)
    }

    /// Next step of a build once the checks are known: the question about broken files, then the overwrite question, then the build.
    fn after_checks(&mut self, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        if !self.checks_current() {
            self.flow = BuildFlow::WaitCheck(out);
        } else if validate::any_asks(&self.findings) {
            self.flow = BuildFlow::Ask(out);
        } else {
            self.after_questions(out, cx);
        }
    }

    fn after_questions(&mut self, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        if out.exists() {
            self.flow = BuildFlow::Confirm(out);
        } else {
            self.start_build(out, Existing::Refuse, cx);
        }
    }

    /// Keeps `findings` in step with the contents: a background task when a file needs reading, directly when only lengths are needed.
    fn run_checks(&mut self, cx: &mut ToolkitCx<'_>) {
        if let Some((g, mut h)) = self.check.take() {
            match h.poll() {
                None => self.check = Some((g, h)),
                Some(outcome) => {
                    if g == self.input_gen {
                        self.findings = if let TaskOutcome::Done(f) = outcome { f } else { Vec::new() };
                        self.checked_gen = Some(g);
                    }
                }
            }
        }
        if self.check.is_some() || self.checks_current() || self.dirty {
            return;
        }
        let needs_reading = self.input.files.iter().map(|f| f.0.as_str()).chain(self.input.loose.iter().map(|f| f.0.as_str())).any(validate::wants_bytes);
        if needs_reading {
            let input = self.input.clone();
            let h = cx.shell.tasks().spawn(TaskSpec { title: "Checking the files".to_string(), cancellable: true, exclusive: None }, move |_job| Ok(validate::check_input(&input)));
            self.check = Some((self.input_gen, h));
        } else {
            self.findings = validate::check_input(&self.input);
            self.checked_gen = Some(self.input_gen);
        }
    }

    /// The file checks of the contents (valid once `checks_done`).
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    pub fn checks_done(&self) -> bool {
        self.checks_current()
    }

    fn start_build(&mut self, out: PathBuf, existing: Existing, cx: &mut ToolkitCx<'_>) {
        let input = self.input.clone();
        let sign = if self.sign { Some((PathBuf::from(self.secret_path.trim()), self.check_version.trim().parse::<u32>().unwrap_or(0))) } else { None };
        let o2 = out.clone();
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Building {name}"), cancellable: true, exclusive: Some(out.display().to_string()) }, move |job| {
            let sign = match sign {
                Some((p, v)) => Some((shaiya_patch::sign::read_key(&p)?, v)),
                None => None,
            };
            let signed = sign.is_some();
            let report = shaiya_patch::build_with(&input, &o2, &BuildOptions { existing, sign }, job)?;
            let size = std::fs::metadata(&o2).map(|m| m.len()).unwrap_or(0);
            Ok(BuildDone { report, out: o2, size, signed })
        });
        self.flow = BuildFlow::Running(out, h);
    }

    pub fn tick(&mut self, cx: &mut ToolkitCx<'_>) {
        self.run_checks(cx);
        if let Some(h) = &mut self.diff {
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done((label, input))) => {
                    self.extra = Some((label, input));
                    self.dirty = true;
                    self.diff = None;
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("Cannot compare the archives", &e);
                    self.diff = None;
                }
                Some(_) => self.diff = None,
            }
        }
        match std::mem::replace(&mut self.flow, BuildFlow::Idle) {
            BuildFlow::Idle => {}
            BuildFlow::WaitCheck(out) => {
                if self.checks_current() {
                    self.after_checks(out, cx);
                } else {
                    self.flow = BuildFlow::WaitCheck(out);
                }
            }
            BuildFlow::Ask(out) => {
                let c = Confirm {
                    title: "Some files look broken".into(),
                    text: crate::archive_view::findings_question(&self.findings, "put"),
                    buttons: vec!["Build anyway".into(), "Cancel".into()],
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-build-findings", &c) {
                    None => self.flow = BuildFlow::Ask(out),
                    Some(0) => self.after_questions(out, cx),
                    Some(_) => {}
                }
            }
            BuildFlow::Confirm(out) => {
                let c = Confirm { title: "Overwrite the patch".into(), text: format!("{} already exists.", out.display()), buttons: vec!["Overwrite".into(), "Cancel".into()], danger: Some(0), ..Default::default() };
                match cx.shell.confirm("toolkit-build-exists", &c) {
                    None => self.flow = BuildFlow::Confirm(out),
                    Some(0) => self.start_build(out, Existing::Overwrite, cx),
                    Some(_) => {}
                }
            }
            BuildFlow::Running(out, mut h) => match h.poll() {
                None => self.flow = BuildFlow::Running(out, h),
                Some(TaskOutcome::Done(d)) => {
                    self.result = vec![format!(
                        "Built {} ({}): {} archive files, {} loose, {} removals{}",
                        d.out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        human_size(d.size),
                        d.report.added.len(),
                        d.report.loose.len(),
                        self.input.removals.len(),
                        if d.signed { ", signed" } else { ", unsigned" }
                    )];
                    cx.shell.status(self.result[0].clone());
                    self.last_out = Some(d.out);
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("The patch was not built", &e);
                    self.result.clear();
                }
                Some(TaskOutcome::Cancelled) => cx.shell.status("Cancelled; no patch was written"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("The patch was not built", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            },
        }
    }

    // ---------------------------------------------------------------------------------------- tests

    pub fn input(&self) -> &BuildInput {
        &self.input
    }

    // ---------------------------------------------------------------------------------------- drawing

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, client: Option<&ClientInfo>, archive: &ArchiveView, keys: &mut KeyForm) -> Vec<PatchRequest> {
        let mut requests = Vec::new();
        self.tick(cx);
        self.suggest_name(client);
        if self.dirty {
            self.resolve();
        }
        self.refresh_rows(client, archive);
        let pal = cx.shell.pal();
        let mut choose: Option<Card> = None;
        let mut clear: Option<Card> = None;
        let mut build = false;
        let mut changed_date = false;
        let mut pick_out = false;
        let mut diff_pick = false;
        let mut pending_pick = false;
        let mut pick_secret = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (card, title, hint) in [
                    (Card::Files, "Archive files", "folder mirrors archive paths"),
                    (Card::Loose, "Loose files", "client folder"),
                    (Card::Remove, "Removals", "path list"),
                ] {
                    ui.group(|ui| {
                        ui.set_width(250.0);
                        ui.label(RichText::new(title).strong());
                        ui.label(RichText::new(hint).color(pal.weak));
                        let (path, count) = match card {
                            Card::Files => (self.files_dir.clone(), self.files_dir.as_ref().map(|p| walk_count(p))),
                            Card::Loose => (self.loose_dir.clone(), self.loose_dir.as_ref().map(|p| walk_count(p))),
                            Card::Remove => (self.remove_list.clone(), self.input.removals.len().checked_sub(self.extra.as_ref().map(|e| e.1.removals.len()).unwrap_or(0))),
                        };
                        match path {
                            Some(p) => {
                                ui.label(p.display().to_string());
                                ui.label(if card == Card::Remove { shaiya_inspect::counted((count.unwrap_or(0)) as u64, "path", "paths") } else { shaiya_inspect::counted((count.unwrap_or(0)) as u64, "file", "files") });
                            }
                            None => {
                                ui.label(RichText::new("empty").color(pal.weak));
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Choose...").clicked() {
                                choose = Some(card);
                            }
                            if ui.button("Clear").clicked() {
                                clear = Some(card);
                            }
                        });
                    });
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("From differences...").on_hover_text("Compare two archives; the open archive is the new one").clicked() {
                    diff_pick = true;
                }
                if ui.add_enabled(!archive.pending.is_empty(), egui::Button::new("From pending changes")).clicked() {
                    pending_pick = true;
                }
                if let Some((label, _)) = &self.extra {
                    ui.label(RichText::new(format!("Also included: {label}")).color(pal.weak));
                    if ui.small_button("Remove").clicked() {
                        self.extra = None;
                        self.dirty = true;
                    }
                }
                if self.diff.is_some() {
                    ui.label("Comparing...");
                }
            });
            if let Some(e) = &self.resolve_error {
                ui.label(RichText::new(e).color(pal.error));
            }
            if self.rows.is_empty() {
                ui.add_space(10.0);
                ui.label(RichText::new("Drop a folder of changed files here. Files inside it keep their paths in the archive (for example Item/Item.SData). Loose files go into the client folder itself. Nothing is written until you press Build.").color(pal.weak));
            } else {
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Contents ({})", self.rows.len())).strong());
                egui::Grid::new("toolkit-patch-rows-head").show(ui, |ui| {
                    for h in ["Kind", "Path", "Size", "Packed", "Effect on the client"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                });
                let n = self.rows.len();
                let findings = &self.findings;
                egui::ScrollArea::vertical().id_salt("toolkit-patch-rows").max_height(260.0).show_rows(ui, 18.0, n, |ui, range| {
                    for r in &self.rows[range] {
                        ui.horizontal(|ui| {
                            let col = if r.problem { pal.error } else { pal.text };
                            ui.add_sized([60.0, 16.0], egui::Label::new(RichText::new(r.kind).color(col)));
                            ui.add_sized([340.0, 16.0], egui::Label::new(RichText::new(&r.path).color(col)).truncate());
                            ui.add_sized([80.0, 16.0], egui::Label::new(human_size(r.size)));
                            ui.add_sized([90.0, 16.0], egui::Label::new(if r.version > 0 && r.kind == "archive" { utc_iso_unix(r.version as i64)[..10].to_string() } else { "-".to_string() }));
                            if r.problem {
                                ui.label(RichText::new("problem").color(pal.error));
                            } else {
                                ui.label(&r.effect);
                                if findings.iter().any(|f| f.path == r.path) {
                                    ui.label(RichText::new("check").color(pal.warning)).on_hover_text("See Checks before files go in, below the table");
                                }
                            }
                        });
                    }
                });
                for p in self.problems.iter().take(8) {
                    ui.label(RichText::new(p).color(pal.error));
                }
                if self.problems.len() > 8 {
                    ui.label(RichText::new(format!("... and {} more", self.problems.len() - 8)).color(pal.error));
                }
                if !self.checks_current() {
                    ui.label(RichText::new("Checking the files...").color(pal.weak));
                }
                if !self.findings.is_empty() {
                    ui.label(RichText::new(format!("Checks before files go in ({}); they only warn", self.findings.len())).strong());
                    for f in self.findings.iter().take(8) {
                        ui.label(RichText::new(f.line()).color(if f.asks() { pal.error } else { pal.warning }));
                    }
                    if self.findings.len() > 8 {
                        ui.label(RichText::new(format!("... and {} more", self.findings.len() - 8)).color(pal.warning));
                    }
                }
            }
            ui.add_space(8.0);
            ui.separator();
            ui.heading("Output");
            ui.horizontal(|ui| {
                ui.label("File name");
                if ui.add(egui::TextEdit::singleline(&mut self.out_name).desired_width(160.0)).changed() {
                    self.name_edited = true;
                }
                ui.label("Folder");
                ui.add(egui::TextEdit::singleline(&mut self.out_dir).desired_width(320.0));
                if ui.button("Choose...").clicked() {
                    pick_out = true;
                }
            });
            ui.horizontal(|ui| {
                changed_date |= ui.radio_value(&mut self.fixed_date, false, "File times").changed();
                changed_date |= ui.radio_value(&mut self.fixed_date, true, "Fixed date").changed();
                if self.fixed_date {
                    changed_date |= ui.add(egui::TextEdit::singleline(&mut self.date_text).desired_width(110.0)).changed();
                }
            });
            egui::CollapsingHeader::new("Signing (only for an updater that checks signatures)").default_open(self.sign).show(ui, |ui| {
                ui.label(RichText::new("Optional. The original updater does not check signatures, so a patch is built unsigned unless you tick this.").color(pal.weak));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.sign, "Sign after building");
                    if self.sign {
                        ui.label("Secret key file");
                        ui.add(egui::TextEdit::singleline(&mut self.secret_path).desired_width(260.0));
                        if ui.button("Choose...").clicked() {
                            pick_secret = true;
                        }
                        ui.label("CheckVersion");
                        ui.add(egui::TextEdit::singleline(&mut self.check_version).desired_width(50.0));
                    }
                });
            });
            for w in self.warnings() {
                ui.label(RichText::new(w).color(pal.warning));
            }
            ui.horizontal(|ui| {
                let blocked = self.build_blocked(keys);
                if ui.add_enabled(blocked.is_none(), egui::Button::new("Build")).on_hover_text("Ctrl+Enter").clicked() {
                    build = true;
                }
                if let Some(why) = blocked {
                    ui.label(RichText::new(why).color(pal.weak));
                }
            });
            for line in &self.result {
                ui.label(RichText::new(line).color(pal.ok));
            }
            if let Some(out) = self.last_out.clone() {
                ui.horizontal(|ui| {
                    if ui.button("Show folder").clicked() {
                        if let Some(parent) = out.parent() {
                            if let Err(e) = shaiya_ui::reveal::open_path(parent) {
                                cx.shell.status(e);
                            }
                        }
                    }
                    if ui.add_enabled(client.is_some(), egui::Button::new("Apply to a client...")).clicked() {
                        requests.push(PatchRequest::ApplyPatch(out.clone()));
                    }
                    if ui.button("Verify").clicked() {
                        requests.push(PatchRequest::VerifyPatch(out.clone()));
                    }
                });
            }
        });
        // keyboard: Ctrl+Enter builds
        if !ui.ctx().egui_wants_keyboard_input() && ui.ctx().input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            build = true;
        }
        if let Some(c) = clear {
            self.set_card(c, None);
        }
        if let Some(c) = choose {
            let picked = match c {
                Card::Remove => cx.shell.pick_file("removals", &[("Path lists", &["txt", "tsv", "csv"]), ("All files", &["*"])]),
                _ => cx.shell.pick_folder("patch-source"),
            };
            if picked.is_some() {
                self.set_card(c, picked);
            }
        }
        if changed_date {
            self.dirty = true;
        }
        if pick_out {
            if let Some(d) = cx.shell.pick_folder("patch-out") {
                self.out_dir = d.display().to_string();
            }
        }
        if pick_secret {
            if let Some(p) = cx.shell.pick_file("secret-key", &[("Key files", &["key", "txt"]), ("All files", &["*"])]) {
                self.secret_path = p.display().to_string();
            }
        }
        if diff_pick {
            if let Some(old) = cx.shell.pick_file("compare", &[("SAH index", &["sah"])]) {
                self.from_diff(old, None, archive, cx);
            }
        }
        if pending_pick {
            self.from_pending(archive.pending_build_input());
        }
        if build {
            self.begin_build(keys, cx);
        }
        requests
    }

    /// Pre-fills the signing fields from the client's key settings (the secret key path only when remembered).
    pub fn prefill_signing(&mut self, keys: &KeyForm) {
        if self.secret_path.is_empty() {
            self.secret_path = keys.secret_path.clone();
        }
        if self.check_version.is_empty() {
            self.check_version = keys.check_version.clone();
        }
    }
}
