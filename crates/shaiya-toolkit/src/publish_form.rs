//! "Publish for the regular updater" in the Patch builder: pick patches and a server folder, see the plan (what is copied, what the ini
//! becomes, what is refused, the file checks), then confirm and write. The plan object the confirmation describes is the one that is
//! written (`shaiya_patch::publish`). Nothing is written into a client folder.

use crate::explain;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_core::time::human_size;
use shaiya_inspect::validate::{self, Finding};
use shaiya_patch::publish::{plan_publish, publish, FileState, Layout, PublishOptions, PublishPlan, PublishReport};
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::PathBuf;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// A finished plan with the file checks of its patches.
pub struct Planned {
    pub plan: PublishPlan,
    pub findings: Vec<Finding>,
}

enum Flow {
    Idle,
    Planning(String, TaskHandle<Planned>),
    /// The "Publish anyway" question about files that look broken.
    Ask,
    Confirm,
    Running(TaskHandle<PublishReport>),
}

pub struct PublishForm {
    pub patches: Vec<PathBuf>,
    pub dest: String,
    pub layout: Layout,
    pub check_version: String,
    pub updater_version: String,
    pub new_check_version: bool,
    /// Open the section on the next frame (the Start page's button).
    open_request: bool,
    flow: Flow,
    /// The newest plan: the form stamp it was made for, and the plan or the reason it was refused.
    planned: Option<(String, Result<Planned, String>)>,
    pub result: Vec<String>,
}

impl Default for PublishForm {
    fn default() -> Self {
        PublishForm::new()
    }
}

impl PublishForm {
    pub fn new() -> PublishForm {
        PublishForm {
            patches: Vec::new(),
            dest: String::new(),
            layout: Layout::Shaiya,
            check_version: String::new(),
            updater_version: String::new(),
            new_check_version: false,
            open_request: false,
            flow: Flow::Idle,
            planned: None,
            result: Vec::new(),
        }
    }

    /// Shows the section on the next frame.
    pub fn open(&mut self) {
        self.open_request = true;
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.flow, Flow::Idle)
    }

    /// The form as one string: a plan is only good for the form it was made from.
    fn stamp(&self) -> String {
        format!(
            "{:?}|{}|{:?}|{}|{}|{}",
            self.patches,
            self.dest.trim(),
            self.layout,
            self.check_version.trim(),
            self.updater_version.trim(),
            self.new_check_version
        )
    }

    fn number(text: &str, what: &str) -> Result<Option<u32>, String> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(None);
        }
        t.parse::<u32>().map(Some).map_err(|_| format!("{what} must be a number"))
    }

    /// Why a plan cannot be made yet.
    pub fn plan_blocked(&self) -> Option<String> {
        if self.is_busy() {
            return Some("Busy".into());
        }
        if self.patches.is_empty() {
            return Some("Add the patches to publish".into());
        }
        if self.dest.trim().is_empty() {
            return Some("Choose the server folder".into());
        }
        if let Err(e) = Self::number(&self.check_version, "CheckVersion") {
            return Some(e);
        }
        if let Err(e) = Self::number(&self.updater_version, "UpdaterVersion") {
            return Some(e);
        }
        None
    }

    /// The plan made for the current form, when there is one.
    pub fn plan(&self) -> Option<&Planned> {
        match &self.planned {
            Some((stamp, Ok(p))) if *stamp == self.stamp() => Some(p),
            _ => None,
        }
    }

    /// The reason the current form was refused, when it was.
    pub fn refusal(&self) -> Option<&str> {
        match &self.planned {
            Some((stamp, Err(m))) if *stamp == self.stamp() => Some(m),
            _ => None,
        }
    }

    pub fn publish_blocked(&self) -> Option<String> {
        if self.is_busy() {
            return Some("Busy".into());
        }
        match self.plan() {
            None => Some(if self.refusal().is_some() { "The plan was refused; change the form".to_string() } else { "Check the plan first".to_string() }),
            Some(p) if p.plan.nothing_to_do() => Some("Nothing to do: everything is published already".to_string()),
            Some(_) => None,
        }
    }

    /// "Check the plan": makes the plan (verifies every patch, hashes files, runs the file checks) as a task.
    pub fn request_plan(&mut self, client_dirs: Vec<PathBuf>, cx: &mut ToolkitCx<'_>) {
        if self.plan_blocked().is_some() {
            return;
        }
        let stamp = self.stamp();
        let opts = PublishOptions {
            layout: self.layout,
            check_version: Self::number(&self.check_version, "CheckVersion").ok().flatten(),
            updater_version: Self::number(&self.updater_version, "UpdaterVersion").ok().flatten(),
            new_check_version: self.new_check_version,
            client_dirs,
        };
        let (patches, dest) = (self.patches.clone(), PathBuf::from(self.dest.trim()));
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Checking the publish plan".into(), cancellable: true, exclusive: None }, move |job| {
            let plan = plan_publish(&patches, &dest, &opts, job)?;
            let mut findings = Vec::new();
            for item in &plan.items {
                findings.extend(validate::check_patch(&item.patch.source, job)?);
            }
            Ok(Planned { plan, findings })
        });
        self.planned = None;
        self.result.clear();
        self.flow = Flow::Planning(stamp, h);
    }

    /// "Publish": the question about broken files (only when the checks found a broken one), then the confirmation, then the write.
    pub fn begin_publish(&mut self) {
        if self.publish_blocked().is_some() {
            return;
        }
        let asks = self.plan().is_some_and(|p| validate::any_asks(&p.findings));
        self.flow = if asks { Flow::Ask } else { Flow::Confirm };
    }

    pub fn tick(&mut self, cx: &mut ToolkitCx<'_>) {
        match std::mem::replace(&mut self.flow, Flow::Idle) {
            Flow::Idle => {}
            Flow::Planning(stamp, mut h) => match h.poll() {
                None => self.flow = Flow::Planning(stamp, h),
                Some(TaskOutcome::Done(p)) => self.planned = Some((stamp, Ok(p))),
                Some(TaskOutcome::Failed(e)) => self.planned = Some((stamp, Err(e.to_string()))),
                Some(TaskOutcome::Cancelled) => cx.shell.status("Cancelled; nothing was published"),
                Some(TaskOutcome::Panicked(m)) => self.planned = Some((stamp, Err(format!("internal error (a bug): {m}")))),
            },
            Flow::Ask => {
                let findings = self.plan().map(|p| p.findings.clone()).unwrap_or_default();
                let c = Confirm {
                    title: "Some files look broken".into(),
                    text: crate::archive_view::findings_question(&findings, "publish"),
                    buttons: vec!["Publish anyway".into(), "Cancel".into()],
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-publish-findings", &c) {
                    None => self.flow = Flow::Ask,
                    Some(0) => self.flow = Flow::Confirm,
                    Some(_) => {}
                }
            }
            Flow::Confirm => {
                let Some(p) = self.plan() else { return };
                let c = Confirm {
                    title: "Publish for the regular updater".into(),
                    text: format!("Players' updaters read {} to decide what to download. A wrong value there stops every player's update, so check the rows below.", p.plan.ini_path.display()),
                    buttons: vec!["Publish".into(), "Cancel".into()],
                    danger: Some(0),
                    details: explain::publish_rows(&p.plan),
                };
                match cx.shell.confirm("toolkit-publish", &c) {
                    None => self.flow = Flow::Confirm,
                    Some(0) => self.start_publish(cx),
                    Some(_) => {}
                }
            }
            Flow::Running(mut h) => match h.poll() {
                None => self.flow = Flow::Running(h),
                Some(TaskOutcome::Done(r)) => {
                    let mut lines = vec![format!(
                        "Published: {} copied, {} already there{}; every written file was read back and matches",
                        r.copied.len(),
                        r.already_published.len(),
                        r.ini_written.as_ref().map(|_| format!(", PatchFileVersion now {}", r.patch_file_version)).unwrap_or_default()
                    )];
                    if r.ini_written.is_none() {
                        lines.push("The ini was left as it was.".into());
                    }
                    cx.shell.status(lines[0].clone());
                    self.result = lines;
                    // the folder changed: the plan is stale
                    self.planned = None;
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("The publish stopped", &e);
                    self.result.clear();
                    self.planned = None;
                }
                Some(TaskOutcome::Cancelled) => {
                    cx.shell.status("Cancelled");
                    self.planned = None;
                }
                Some(TaskOutcome::Panicked(m)) => {
                    cx.shell.error("The publish stopped", &shaiya_core::Error::Unsupported(format!("internal error (a bug): {m}")));
                    self.planned = None;
                }
            },
        }
    }

    fn start_publish(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(p) = self.plan() else { return };
        let plan = p.plan.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Publishing patches".into(), cancellable: false, exclusive: Some(plan.dir.display().to_string()) }, move |job| publish(&plan, job));
        self.flow = Flow::Running(h);
    }

    /// Draws the section. `built` is the patch the builder just made (offered as a one-click source). The owner calls [`PublishForm::tick`] every frame.
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, built: Option<&PathBuf>) {
        let pal = cx.shell.pal();
        let open = self.open_request.then_some(true);
        self.open_request = false;
        let mut add = false;
        let mut use_built = false;
        let mut pick_dest = false;
        let mut plan_now = false;
        let mut go = false;
        let mut remove: Option<usize> = None;
        let mut header = egui::CollapsingHeader::new("Publish for the regular updater").id_salt("toolkit-publish-section");
        if let Some(o) = open {
            header = header.open(Some(o));
        }
        header.show(ui, |ui| {
            ui.label(
                RichText::new("Copies psNNNN.patch files into a server folder and raises PatchFileVersion in UpdateVersion.ini, the file the original updater reads. It never writes into a client folder.")
                    .color(pal.weak),
            );
            ui.horizontal_wrapped(|ui| {
                if ui.button("Add patches...").clicked() {
                    add = true;
                }
                if ui.add_enabled(built.is_some(), egui::Button::new("Use the patch just built")).clicked() {
                    use_built = true;
                }
            });
            for (i, p) in self.patches.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(p.display().to_string());
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("Server folder");
                ui.add(egui::TextEdit::singleline(&mut self.dest).desired_width(320.0));
                if ui.button("Choose...").clicked() {
                    pick_dest = true;
                }
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.layout, Layout::Shaiya, "Shaiya/UpdateVersion.ini and Shaiya/patch/");
                ui.radio_value(&mut self.layout, Layout::Flat, "Both in the folder itself");
            });
            ui.horizontal(|ui| {
                ui.label("CheckVersion");
                ui.add(egui::TextEdit::singleline(&mut self.check_version).desired_width(50.0)).on_hover_text("Required for a new UpdateVersion.ini: the CheckVersion of your players' Version.ini");
                ui.label("UpdaterVersion");
                ui.add(egui::TextEdit::singleline(&mut self.updater_version).desired_width(50.0)).on_hover_text("For a new ini (default 1); an existing ini's value stays unless you fill this");
                ui.checkbox(&mut self.new_check_version, "Allow a different CheckVersion (players reinstall)");
            });
            ui.horizontal(|ui| {
                let blocked = self.plan_blocked();
                if ui.add_enabled(blocked.is_none(), egui::Button::new("Check the plan")).clicked() {
                    plan_now = true;
                }
                if matches!(self.flow, Flow::Planning(..)) {
                    ui.label("Checking...");
                } else if let Some(why) = blocked {
                    ui.label(RichText::new(why).color(pal.weak));
                }
            });
            if let Some(m) = self.refusal() {
                ui.label(RichText::new(format!("Refused: {m}")).color(pal.error));
            }
            if let Some(p) = self.plan() {
                for item in &p.plan.items {
                    let name = item.patch.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let state = match item.patch.state {
                        FileState::New => "new".to_string(),
                        FileState::AlreadyPublished => "already published (same file)".to_string(),
                    };
                    ui.label(format!("{name} ({}): {state} -> {}", human_size(item.patch.size), item.patch.target.strip_prefix(&p.plan.dir).unwrap_or(&item.patch.target).display()));
                }
                let ini = p.plan.ini_path.strip_prefix(&p.plan.dir).unwrap_or(&p.plan.ini_path).display().to_string();
                ui.label(if p.plan.ini_changed {
                    format!(
                        "{ini}: PatchFileVersion {} -> {} (CheckVersion {}, UpdaterVersion {})",
                        p.plan.patch_file_version_before.map_or("new file".to_string(), |v| v.to_string()),
                        p.plan.patch_file_version_after,
                        p.plan.check_version,
                        p.plan.updater_version
                    )
                } else {
                    format!("{ini}: unchanged")
                });
                for w in &p.plan.warnings {
                    ui.label(RichText::new(w).color(pal.warning));
                }
                if !p.findings.is_empty() {
                    ui.label(RichText::new(format!("Checks before files go in ({}); they only warn", p.findings.len())).strong());
                    for f in p.findings.iter().take(8) {
                        ui.label(RichText::new(f.line()).color(if f.asks() { pal.error } else { pal.warning }));
                    }
                }
            }
            ui.horizontal(|ui| {
                let blocked = self.publish_blocked();
                if ui.add_enabled(blocked.is_none(), egui::Button::new("Publish")).clicked() {
                    go = true;
                }
                if let Some(why) = blocked.filter(|_| self.plan().is_some() || self.refusal().is_some()) {
                    ui.label(RichText::new(why).color(pal.weak));
                }
            });
            for line in &self.result {
                ui.label(RichText::new(line).color(pal.ok));
            }
        });
        if let Some(i) = remove {
            self.patches.remove(i);
        }
        if add {
            let picked = cx.shell.pick_files("publish-patches", &[("Patch files", &["patch"]), ("All files", &["*"])]);
            for p in picked {
                if !self.patches.contains(&p) {
                    self.patches.push(p);
                }
            }
        }
        if use_built {
            if let Some(b) = built {
                if !self.patches.contains(b) {
                    self.patches.push(b.clone());
                }
            }
        }
        if pick_dest {
            if let Some(d) = cx.shell.pick_folder("publish-dest") {
                self.dest = d.display().to_string();
            }
        }
        if plan_now {
            let dirs = client_dirs(cx);
            self.request_plan(dirs, cx);
        }
        if go {
            self.begin_publish();
        }
    }
}

/// The saved client folders (a publish into one of them is refused).
pub fn client_dirs(cx: &ToolkitCx<'_>) -> Vec<PathBuf> {
    cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect()
}
