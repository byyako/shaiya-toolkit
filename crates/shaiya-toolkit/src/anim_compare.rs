//! "Compare with...": the poses of two `.ANI` files side by side. The other file is another `.ANI` of the open archive, or the same path in
//! another saved client (that client's archive is opened read-only on the worker). The worker reads both files, runs
//! [`shaiya_clientdata::anim::diff`] and hands back the numbers; nothing is written anywhere.

use crate::ToolkitSettings;
use shaiya_archive::Archive;
use shaiya_clientdata::anim::{self, AniDiff, RigDiff};
use shaiya_clientdata::modeljson::MAX_MODEL_BYTES;
use shaiya_clientdata::Ani;
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::PathBuf;
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// How many bones the table lists.
pub const BONES_LISTED: usize = 20;

/// The other side of a comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Other {
    /// Another file of the open archive, by archive path.
    File(String),
    /// The same path in a saved client (its name and folder).
    Client { name: String, dir: PathBuf },
}

impl Other {
    /// What the choice is called in the box.
    pub fn label(&self) -> String {
        match self {
            Other::File(p) => p.rsplit('/').next().unwrap_or(p).to_string(),
            Other::Client { name, .. } => format!("the same file in {name}"),
        }
    }
}

/// A finished comparison: what each side was called and the numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub a: String,
    pub b: String,
    pub diff: AniDiff,
}

fn read_ani(archive: &Archive, path: &str, label: &str) -> Result<Ani> {
    let entry = archive.index().get(path).ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, format!("not in {label}"))))?;
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    let bytes = archive.read_entry(entry)?;
    Ani::parse(&bytes, path).map_err(|e| match e {
        shaiya_clientdata::Error::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
        other => Error::format(path, None, other.to_string()),
    })
}

/// What the worker does: read `path` from `archive` and the other side, and diff them.
pub fn compare(archive: &Archive, path: &str, other: &Other, job: &shaiya_core::Job) -> Result<Shown> {
    job.check()?;
    let a = read_ani(archive, path, "the open archive")?;
    job.check()?;
    let (b, b_name) = match other {
        Other::File(p) => (read_ani(archive, p, "the open archive")?, p.clone()),
        Other::Client { name, dir } => {
            let theirs = Archive::open(dir)?;
            job.check()?;
            (read_ani(&theirs, path, name)?, format!("{path} in {name}"))
        }
    };
    job.check()?;
    Ok(Shown { a: path.to_string(), b: b_name, diff: anim::diff(&a, &b) })
}

/// The window of one comparison.
pub struct AnimCompare {
    path: String,
    archive: Arc<Archive>,
    siblings: Vec<String>,
    clients: Vec<(String, PathBuf)>,
    pick: Option<Other>,
    handle: Option<TaskHandle<Shown>>,
    /// The last result (or the error text).
    pub result: Option<std::result::Result<Shown, String>>,
    pub open: bool,
}

impl AnimCompare {
    /// A window comparing the `.ANI` at `path` of `archive`; `siblings` are the other `.ANI` paths it can be compared with (archive paths),
    /// `clients` the saved clients (name, folder) whose archive has the same path to offer.
    pub fn new(path: String, archive: Arc<Archive>, siblings: Vec<String>, clients: Vec<(String, PathBuf)>) -> AnimCompare {
        AnimCompare { path, archive, siblings, clients, pick: None, handle: None, result: None, open: true }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Starts the comparison with `other` on a worker.
    pub fn start(&mut self, other: Other, cx: &mut ToolkitCx<'_>) {
        if self.handle.is_some() {
            return;
        }
        self.pick = Some(other.clone());
        self.result = None;
        let (archive, path) = (self.archive.clone(), self.path.clone());
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.handle = Some(cx.shell.tasks().spawn(TaskSpec { title: format!("Compare {name}"), cancellable: true, exclusive: None }, move |job| compare(&archive, &path, &other, job)));
    }

    /// Takes in the worker's answer.
    pub fn poll(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(h) = &mut self.handle else { return };
        let Some(outcome) = h.poll() else { return };
        self.handle = None;
        self.result = Some(match outcome {
            TaskOutcome::Done(shown) => Ok(shown),
            TaskOutcome::Failed(e) => Err(e.to_string()),
            TaskOutcome::Cancelled => Err("cancelled".to_string()),
            TaskOutcome::Panicked(m) => Err(format!("internal error (a bug): {m}")),
        });
        if let Some(Ok(s)) = &self.result {
            cx.shell.status(summary(&s.diff));
        }
    }

    /// Draws the window; closing it drops the comparison.
    pub fn draw(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        let mut open = self.open;
        let mut start: Option<Other> = None;
        egui::Window::new("Compare animation").open(&mut open).default_size([560.0, 420.0]).show(ctx, |ui| {
            ui.label(egui::RichText::new(&self.path).strong());
            ui.label(egui::RichText::new("Compared at every key from the start to the larger end key; nothing is written.").color(pal.weak));
            ui.horizontal(|ui| {
                ui.label("Compare with");
                let shown = self.pick.as_ref().map(|o| o.label()).unwrap_or_else(|| "(choose a file or a client)".to_string());
                egui::ComboBox::from_id_salt("toolkit-anim-compare-other").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (name, dir) in &self.clients {
                            let o = Other::Client { name: name.clone(), dir: dir.clone() };
                            if ui.selectable_label(self.pick.as_ref() == Some(&o), o.label()).clicked() {
                                self.pick = Some(o);
                            }
                        }
                        for p in &self.siblings {
                            let o = Other::File(p.clone());
                            if ui.selectable_label(self.pick.as_ref() == Some(&o), o.label()).clicked() {
                                self.pick = Some(o);
                            }
                        }
                    });
                });
                let ready = self.pick.is_some() && self.handle.is_none();
                if ui.add_enabled(ready, egui::Button::new("Compare")).clicked() {
                    start = self.pick.clone();
                }
            });
            if self.handle.is_some() {
                ui.label("Comparing...");
                return;
            }
            match &self.result {
                None => {}
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(e).color(pal.error));
                }
                Some(Ok(s)) => show(ui, s, &pal),
            }
        });
        self.open = open;
        if let Some(o) = start {
            self.start(o, cx);
        }
    }
}

/// The one-line summary for the status line.
pub fn summary(d: &AniDiff) -> String {
    if d.is_identical() {
        "The two animations are identical (same skeleton, end key and poses)".to_string()
    } else {
        format!("The animations differ: {} differ in pose, end keys {} and {}", shaiya_inspect::counted(d.bones.len() as u64, "bone", "bones"), d.end_key.0, d.end_key.1)
    }
}

fn show(ui: &mut egui::Ui, s: &Shown, pal: &shaiya_ui::theme::Pal) {
    ui.label(format!("A: {}", s.a));
    ui.label(format!("B: {}", s.b));
    let d = &s.diff;
    if d.is_identical() {
        ui.label(egui::RichText::new(format!("Identical: same skeleton, end key {}, and the same poses at every key.", d.end_key.0)).color(pal.ok));
        return;
    }
    let mut lines: Vec<String> = Vec::new();
    for x in &d.skeleton {
        lines.push(match x {
            RigDiff::BoneCount { candidate, target } => format!("bone count: A {candidate}, B {target}"),
            RigDiff::Parent { bone, candidate, target } => format!("bone {bone}: parent is {candidate} in A, {target} in B"),
            RigDiff::Matrix { bone } => format!("bone {bone}: reference matrix differs"),
            RigDiff::FirstField { candidate, target } => format!("first field differs: A {candidate}, B {target} (inferred: the start key)"),
        });
    }
    if d.end_key.0 != d.end_key.1 {
        lines.push(format!("end key differs: A {}, B {}", d.end_key.0, d.end_key.1));
    }
    if !lines.is_empty() {
        ui.label(egui::RichText::new("Skeleton and length").strong());
        egui::ScrollArea::vertical().id_salt("toolkit-anim-compare-skeleton").max_height(100.0).show(ui, |ui| {
            for l in &lines {
                ui.label(l);
            }
        });
    }
    if d.bones.is_empty() {
        ui.label("No bone pose differs.");
        return;
    }
    ui.label(egui::RichText::new(format!("{} differ in pose (largest turn first)", shaiya_inspect::counted(d.bones.len() as u64, "bone", "bones"))).strong());
    egui::Grid::new("toolkit-anim-compare-bones").striped(true).show(ui, |ui| {
        for h in ["Bone", "Turn (degrees)", "Move (units)", "Keys in A", "Keys in B"] {
            ui.label(egui::RichText::new(h).strong());
        }
        ui.end_row();
        for b in d.bones.iter().take(BONES_LISTED) {
            ui.label(b.bone.to_string());
            ui.label(format!("{:.2}", b.max_angle_deg));
            ui.label(format!("{:.3}", b.max_offset));
            ui.label(b.keys.0.to_string());
            ui.label(b.keys.1.to_string());
            ui.end_row();
        }
    });
    if d.bones.len() > BONES_LISTED {
        ui.label(format!("and {} more", d.bones.len() - BONES_LISTED));
    }
}
