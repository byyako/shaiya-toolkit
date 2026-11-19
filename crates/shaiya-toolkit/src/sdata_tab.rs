//! The SData tab: files, folders or the open client's `*.SData` listed with their wrapper facts, and decrypt, encrypt and check
//! shortcuts that always write to a chosen folder (in-place conversion is left to `shaiya sdata`).

use crate::archive_view::ArchiveView;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::{Archive, ArchiveSource};
use shaiya_core::fs::Existing;
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::source::{ByteSource, FolderSource};
use shaiya_core::{Error, Result};
use shaiya_seed::batch::{run, BatchOptions, ItemResult, Op, Output, Status};
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Check,
    Decrypt,
    Encrypt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Like {
    No,
    Client,
    Folder(PathBuf),
}

/// One listed item: where it came from and what it looks like.
#[derive(Clone, Debug)]
pub struct Row {
    pub source: String,
    pub item: ItemResult,
}

#[derive(Clone)]
struct Inputs {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
    archive: Option<Arc<Archive>>,
}

fn sdata_filter() -> Filter {
    Filter { include: vec![Pattern::new("*.sdata").expect("a fixed pattern")], ..Filter::default() }
}

/// Runs one operation over every source of `inputs`.
fn run_all(inputs: &Inputs, action: Action, out: Option<&Path>, existing: Existing, like: &Like, job: &shaiya_core::Job) -> Result<Vec<Row>> {
    let like_folder = match like {
        Like::Folder(p) => Some(FolderSource::new(std::slice::from_ref(p), &sdata_filter())?),
        _ => None,
    };
    let like_client = match (like, &inputs.archive) {
        (Like::Client, Some(a)) => Some(ArchiveSource { archive: a, filter: sdata_filter(), include_shadowed: false }),
        _ => None,
    };
    let like_ref: Option<&dyn ByteSource> = match like {
        Like::No => None,
        Like::Client => like_client.as_ref().map(|s| s as &dyn ByteSource),
        Like::Folder(_) => like_folder.as_ref().map(|s| s as &dyn ByteSource),
    };
    let op = || match action {
        Action::Check => Op::Check,
        Action::Decrypt => Op::Decrypt { unchecked: false },
        Action::Encrypt => Op::Encrypt { like: like_ref },
    };
    let output = || match (action, out) {
        (Action::Check, _) | (_, None) => Output::None,
        (_, Some(d)) => Output::Dir(d.to_path_buf()),
    };
    let mut rows = Vec::new();
    let mut go = |label: String, src: &dyn ByteSource| -> Result<()> {
        let opts = BatchOptions { op: op(), output: output(), existing, pass_through: action != Action::Check, dry_run: false };
        for item in run(src, &opts, job)? {
            rows.push(Row { source: label.clone(), item });
        }
        Ok(())
    };
    if !inputs.files.is_empty() {
        go("file".into(), &FolderSource::new(&inputs.files, &Filter::default())?)?;
    }
    if !inputs.dirs.is_empty() {
        go("folder".into(), &FolderSource::new(&inputs.dirs, &sdata_filter())?)?;
    }
    if let Some(a) = &inputs.archive {
        go("client".into(), &ArchiveSource { archive: a, filter: sdata_filter(), include_shadowed: false })?;
    }
    Ok(rows)
}

/// Reads the bytes of one listed file from the source it came from (the table view's loader).
fn read_row(inputs: &Inputs, source: &str, name: &str) -> Result<Vec<u8>> {
    match source {
        "file" => FolderSource::new(&inputs.files, &Filter::default())?.read(name),
        "folder" => FolderSource::new(&inputs.dirs, &sdata_filter())?.read(name),
        _ => {
            let a = inputs.archive.as_ref().ok_or_else(|| Error::Refused("Open a client with a data.saf first".to_string()))?;
            ArchiveSource { archive: a, filter: sdata_filter(), include_shadowed: false }.read(name)
        }
    }
}

/// What the View table button asks the app for: the file's name and a loader for its bytes.
pub type TableRequest = (String, crate::sdata_table::Loader);

struct Request {
    action: Action,
    out: PathBuf,
    clashes: usize,
}

enum Flow {
    Idle,
    Confirm(Request),
    Running(Action, TaskHandle<Vec<Row>>),
}

pub struct SDataTab {
    pub files: Vec<PathBuf>,
    pub dirs: Vec<PathBuf>,
    pub from_client: bool,
    pub rows: Vec<Row>,
    list: Option<(String, TaskHandle<Vec<Row>>)>,
    list_stamp: String,
    pub listing_error: Option<String>,
    flow: Flow,
    pub like_originals: bool,
    pub like_folder: String,
    pub message: Vec<String>,
    /// The listed row the View table button works on.
    pub selected: Option<usize>,
    /// Set by View table (or a double click); the app opens the table window.
    pub table_request: Option<TableRequest>,
}

impl Default for SDataTab {
    fn default() -> Self {
        Self::new()
    }
}

impl SDataTab {
    pub fn new() -> SDataTab {
        SDataTab { files: Vec::new(), dirs: Vec::new(), from_client: false, rows: Vec::new(), list: None, list_stamp: String::new(), listing_error: None, flow: Flow::Idle, like_originals: false, like_folder: String::new(), message: Vec::new(), selected: None, table_request: None }
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            let list = if p.is_dir() { &mut self.dirs } else { &mut self.files };
            if !list.contains(&p) {
                list.push(p);
            }
        }
        self.list_stamp.clear();
    }

    pub fn clear(&mut self) {
        self.files.clear();
        self.dirs.clear();
        self.from_client = false;
        self.rows.clear();
        self.selected = None;
        self.list_stamp.clear();
    }

    /// Selects a listed row (the one View table opens).
    pub fn select(&mut self, row: usize) {
        self.selected = (row < self.rows.len()).then_some(row);
    }

    /// Asks for the table view of the selected row. False when no row is selected.
    pub fn view_selected(&mut self, archive: &ArchiveView) -> bool {
        let Some(r) = self.selected.and_then(|i| self.rows.get(i)) else { return false };
        let (inputs, source, name) = (self.inputs(archive), r.source.clone(), r.item.name.clone());
        let title = name.clone();
        self.table_request = Some((title, Box::new(move |_| read_row(&inputs, &source, &name))));
        true
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.flow, Flow::Idle)
    }

    fn inputs(&self, archive: &ArchiveView) -> Inputs {
        Inputs { files: self.files.clone(), dirs: self.dirs.clone(), archive: if self.from_client { archive.archive_handle() } else { None } }
    }

    fn like(&self) -> Like {
        if !self.like_originals {
            Like::No
        } else if !self.like_folder.trim().is_empty() {
            Like::Folder(PathBuf::from(self.like_folder.trim()))
        } else {
            Like::Client
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Starts an operation writing into `out` (the test entry point; the buttons call it after the folder dialog).
    pub fn begin(&mut self, action: Action, out: Option<PathBuf>, archive: &ArchiveView, cx: &mut ToolkitCx<'_>) {
        if self.is_busy() {
            return;
        }
        match (action, out) {
            (Action::Check, _) => self.spawn(Action::Check, None, Existing::Refuse, archive, cx),
            (a, Some(out)) => {
                let clashes = self.rows.iter().filter(|r| out.join(r.item.name.replace('\\', "/")).exists()).count();
                let req = Request { action: a, out, clashes };
                if clashes > 0 {
                    self.flow = Flow::Confirm(req);
                } else {
                    self.spawn(a, Some(req.out), Existing::Refuse, archive, cx);
                }
            }
            (_, None) => {}
        }
    }

    fn spawn(&mut self, action: Action, out: Option<PathBuf>, existing: Existing, archive: &ArchiveView, cx: &mut ToolkitCx<'_>) {
        let inputs = self.inputs(archive);
        let like = self.like();
        let title = match action {
            Action::Check => "Checking SData files",
            Action::Decrypt => "Decrypting SData files",
            Action::Encrypt => "Encrypting SData files",
        };
        let h = cx.shell.tasks().spawn(TaskSpec { title: title.into(), cancellable: true, exclusive: None }, move |job| run_all(&inputs, action, out.as_deref(), existing, &like, job));
        self.flow = Flow::Running(action, h);
    }

    pub fn tick(&mut self, archive: &ArchiveView, cx: &mut ToolkitCx<'_>) {
        // the listing
        let stamp = format!("{:?}|{:?}|{}|{}|{}", self.files, self.dirs, self.from_client, archive.changed_counter, archive.archive_handle().map(|a| a.sah_path().display().to_string()).unwrap_or_default());
        if stamp != self.list_stamp && self.list.is_none() && (!self.files.is_empty() || !self.dirs.is_empty() || self.from_client) {
            self.list_stamp = stamp.clone();
            let inputs = self.inputs(archive);
            if self.from_client && inputs.archive.is_none() {
                self.rows.retain(|r| r.source != "client");
                self.listing_error = Some("Open a client with a data.saf first".into());
            } else {
                self.listing_error = None;
                let h = cx.shell.tasks().spawn(TaskSpec { title: "Reading SData files".into(), cancellable: true, exclusive: None }, move |job| run_all(&inputs, Action::Check, None, Existing::Refuse, &Like::No, job));
                self.list = Some((stamp, h));
            }
        } else if stamp != self.list_stamp && self.files.is_empty() && self.dirs.is_empty() && !self.from_client {
            self.rows.clear();
            self.list_stamp = stamp;
        }
        if let Some((_, h)) = &mut self.list {
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(rows)) => {
                    self.rows = rows;
                    self.list = None;
                }
                Some(TaskOutcome::Failed(e)) => {
                    self.listing_error = Some(e.to_string());
                    self.list = None;
                }
                Some(_) => self.list = None,
            }
        }
        match std::mem::replace(&mut self.flow, Flow::Idle) {
            Flow::Idle => {}
            Flow::Confirm(req) => {
                let c = Confirm {
                    title: "Files already exist".into(),
                    text: format!("{} already exist in {}.", shaiya_inspect::counted((req.clashes) as u64, "file", "files"), req.out.display()),
                    buttons: vec!["Skip existing".into(), "Overwrite".into(), "Cancel".into()],
                    danger: Some(1),
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-sdata-exists", &c) {
                    None => self.flow = Flow::Confirm(req),
                    Some(0) => self.spawn(req.action, Some(req.out), Existing::Skip, archive, cx),
                    Some(1) => self.spawn(req.action, Some(req.out), Existing::Overwrite, archive, cx),
                    Some(_) => {}
                }
            }
            Flow::Running(action, mut h) => match h.poll() {
                None => self.flow = Flow::Running(action, h),
                Some(TaskOutcome::Done(rows)) => {
                    let count = |s: &[Status]| rows.iter().filter(|r| s.contains(&r.item.status)).count();
                    let failed = count(&[Status::Failed]);
                    let line = match action {
                        Action::Check => format!("{}: {} OK, {} bad, {} failed", shaiya_inspect::counted(rows.len() as u64, "file", "files"), shaiya_inspect::counted(count(&[Status::CrcOk]) as u64, "checksum", "checksums"), count(&[Status::CrcBad]), failed),
                        Action::Decrypt => format!("{}: {} decrypted, {} copied unchanged, {} skipped, {} failed", shaiya_inspect::counted((rows.len()) as u64, "file", "files"), count(&[Status::Decrypted, Status::DecryptedCrcBad]), count(&[Status::PassedThrough]), count(&[Status::SkippedExisting, Status::SkippedUnchanged]), failed),
                        Action::Encrypt => format!("{}: {} encrypted, {} copied unchanged, {} skipped, {} failed", shaiya_inspect::counted((rows.len()) as u64, "file", "files"), count(&[Status::EncryptedNew]), count(&[Status::PassedThrough]), count(&[Status::SkippedExisting, Status::SkippedUnchanged]), failed),
                    };
                    self.message = vec![line.clone()];
                    for r in rows.iter().filter(|r| r.item.status == Status::Failed) {
                        self.message.push(format!("{}: {}", r.item.name, r.item.message.clone().unwrap_or_default()));
                    }
                    cx.shell.status(line);
                    if action == Action::Check {
                        self.rows = rows;
                    }
                }
                Some(TaskOutcome::Failed(e)) => {
                    self.message = vec![e.to_string()];
                    cx.shell.error("The SData operation failed", &e);
                }
                Some(TaskOutcome::Cancelled) => cx.shell.status("Cancelled; files already written stay"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("The SData operation failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            },
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, archive: &ArchiveView) {
        self.tick(archive, cx);
        let pal = cx.shell.pal();
        let has_client = archive.archive_handle().is_some();
        let mut add_files = false;
        let mut add_folder = false;
        let mut client = false;
        let mut clear = false;
        let mut act: Option<Action> = None;
        let mut view = false;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add files...").clicked() {
                add_files = true;
            }
            if ui.button("Add folder...").clicked() {
                add_folder = true;
            }
            if ui.add_enabled(has_client, egui::Button::new("Add from client...")).on_hover_text("All visible *.SData of the open archive").clicked() {
                client = true;
            }
            if ui.add_enabled(!self.rows.is_empty() || self.from_client, egui::Button::new("Clear")).clicked() {
                clear = true;
            }
        });
        if self.rows.is_empty() && self.list.is_none() {
            ui.add_space(12.0);
            ui.label(RichText::new("Drop SData files or folders here, or add them from the open client. Decrypt and Encrypt write into a folder you choose; the originals are never changed.").color(pal.weak));
        }
        if let Some(e) = &self.listing_error {
            ui.label(RichText::new(e).color(pal.error));
        }
        ui.horizontal_wrapped(|ui| {
            let ready = !self.rows.is_empty() && !self.is_busy();
            if ui.add_enabled(ready, egui::Button::new("Check")).clicked() {
                act = Some(Action::Check);
            }
            if ui.add_enabled(ready, egui::Button::new("Decrypt to folder...")).clicked() {
                act = Some(Action::Decrypt);
            }
            if ui.add_enabled(ready, egui::Button::new("Encrypt to folder...")).clicked() {
                act = Some(Action::Encrypt);
            }
            if ui.add_enabled(self.selected.is_some() && !self.is_busy(), egui::Button::new("View table")).on_hover_text("Select a file in the list first. Shows Item, Skill, Monster and NpcQuest files as a table; read only").clicked() {
                view = true;
            }
            ui.checkbox(&mut self.like_originals, "Match the originals' headers");
            if self.like_originals {
                ui.label("originals from");
                ui.add(egui::TextEdit::singleline(&mut self.like_folder).hint_text("the open client, or a folder").desired_width(220.0));
                if ui.button("Choose folder...").clicked() {
                    if let Some(p) = cx.shell.pick_folder("sdata-originals") {
                        self.like_folder = p.display().to_string();
                    }
                }
            }
        });
        for m in &self.message {
            ui.label(m);
        }
        ui.separator();
        egui::Grid::new("toolkit-sdata-grid").striped(true).show(ui, |ui| {
            if !self.rows.is_empty() {
                for h in ["Name", "Source", "Kind", "CRC", "Plaintext", "Padding and tail"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
            }
            for (n, r) in self.rows.iter().enumerate() {
                let i = &r.item;
                let resp = ui.selectable_label(self.selected == Some(n), &i.name);
                if resp.clicked() {
                    self.selected = Some(n);
                }
                if resp.double_clicked() {
                    view = true;
                }
                ui.label(&r.source);
                let encrypted = i.crc.is_some();
                if i.status == Status::Failed {
                    ui.label(RichText::new(i.message.clone().unwrap_or_else(|| "failed".into())).color(pal.error));
                } else {
                    ui.label(if encrypted { "encrypted" } else { "plain" });
                }
                match (i.crc, i.crc_ok) {
                    (Some(c), Some(true)) => ui.label(RichText::new(format!("0x{c:08x} OK")).color(pal.ok)),
                    (Some(c), _) => ui.label(RichText::new(format!("0x{c:08x} BAD")).color(pal.error)),
                    _ => ui.label("-"),
                };
                ui.label(i.plain_size.map(shaiya_inspect::group).unwrap_or_else(|| shaiya_inspect::group(i.size)));
                ui.label(match (i.padding_zero, i.tail_zero) {
                    (Some(p), Some(t)) => format!("padding {}, tail {}", if p { "zero" } else { "not zero" }, if t { "zero" } else { "not zero" }),
                    _ => "-".to_string(),
                });
                ui.end_row();
            }
        });
        if view {
            self.view_selected(archive);
        }
        if add_files {
            let f = cx.shell.pick_files("sdata-files", &[("SData files", &["sdata"]), ("All files", &["*"])]);
            self.add_paths(f);
        }
        if add_folder {
            if let Some(d) = cx.shell.pick_folder("sdata-folder") {
                self.add_paths(vec![d]);
            }
        }
        if client {
            self.from_client = true;
            self.list_stamp.clear();
        }
        if clear {
            self.clear();
        }
        match act {
            Some(Action::Check) => self.begin(Action::Check, None, archive, cx),
            Some(a) => {
                if let Some(out) = cx.shell.pick_folder("sdata-out") {
                    self.begin(a, Some(out), archive, cx);
                }
            }
            None => {}
        }
    }
}
