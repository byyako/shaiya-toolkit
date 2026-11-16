//! The Archive tab: a client's (or any) archive as a folder tree, a searchable table and a preview, with queued edits (pending changes)
//! that are applied to the client folder only after a confirmation, through the same crash-safe procedure as the command line.

use crate::preview::{self, PreviewData};
use crate::StudioSettings;
use egui::{Color32, RichText};
use shaiya_archive::check::{check_archive, CheckOptions, CheckReport, Severity};
use shaiya_archive::client::{self, ClientFiles};
use shaiya_archive::diff::{diff, summarize, Change, DiffEntry, DiffOptions, DiffSummary, Side};
use shaiya_archive::edit::{self, EditPlan, EditReport, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::extract::{self, ExtractOptions, ExtractReport};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_archive::{compact, Archive, Entry, Index};
use shaiya_core::fs::{portable_name, write_atomic_from_with, Existing, Sync};
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::text::Encoding;
use shaiya_core::time::{human_size, utc_iso_unix};
use shaiya_core::{Error, Result, Unit};
use shaiya_ui::undo::{Edit, History};
use shaiya_ui::{Command, Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type StudioCx<'a> = Cx<'a, StudioSettings>;

// ------------------------------------------------------------------------------------------------ data

#[derive(Clone)]
pub struct RowData {
    pub path: String,
    pub lower: String,
    pub entry: Entry,
    pub shadowed: bool,
}

impl RowData {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
    pub fn folder(&self) -> &str {
        self.path.rsplit_once('/').map(|(f, _)| f).unwrap_or("")
    }
    pub fn ext(&self) -> String {
        self.name().rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
    }
    pub fn type_name(&self) -> String {
        match self.ext().as_str() {
            "" => "File".to_string(),
            "sdata" => "SData".to_string(),
            e => e.to_ascii_uppercase(),
        }
    }
}

pub struct Node {
    pub name: String,
    pub path: String,
    pub files: usize,
    pub bytes: u64,
    pub children: Vec<usize>,
}

pub struct Tree {
    pub nodes: Vec<Node>,
    by_path: HashMap<String, usize>,
}

impl Tree {
    fn build(rows: &[RowData]) -> Tree {
        let mut t = Tree { nodes: vec![Node { name: String::new(), path: String::new(), files: 0, bytes: 0, children: Vec::new() }], by_path: HashMap::new() };
        t.by_path.insert(String::new(), 0);
        for r in rows.iter().filter(|r| !r.shadowed) {
            let mut at = 0usize;
            let size = r.entry.length as u64;
            t.nodes[0].files += 1;
            t.nodes[0].bytes += size;
            let mut acc = String::new();
            let segs: Vec<&str> = r.path.split('/').collect();
            for seg in &segs[..segs.len() - 1] {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                let key = acc.to_lowercase();
                let idx = match t.by_path.get(&key) {
                    Some(&i) => i,
                    None => {
                        let i = t.nodes.len();
                        t.nodes.push(Node { name: seg.to_string(), path: acc.clone(), files: 0, bytes: 0, children: Vec::new() });
                        t.nodes[at].children.push(i);
                        t.by_path.insert(key, i);
                        i
                    }
                };
                t.nodes[idx].files += 1;
                t.nodes[idx].bytes += size;
                at = idx;
            }
        }
        for i in 0..t.nodes.len() {
            let mut kids = std::mem::take(&mut t.nodes[i].children);
            kids.sort_by_key(|&k| t.nodes[k].name.to_lowercase());
            t.nodes[i].children = kids;
        }
        t
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        self.by_path.get(&path.to_lowercase()).copied()
    }
}

/// An opened archive: the rows, the tree and (when there is a `data.saf`) the handle previews and extraction read through.
pub struct Loaded {
    pub label: String,
    pub sah: PathBuf,
    /// The client folder this archive belongs to (`None`: an archive opened from a file; nothing can be changed in place).
    pub client_dir: Option<PathBuf>,
    pub archive: Option<Arc<Archive>>,
    pub rows: Vec<RowData>,
    pub tree: Tree,
    by_folder: HashMap<String, Vec<usize>>,
    pub sah_version: i32,
    pub visible: usize,
    pub saf_len: Option<u64>,
    pub dead: Option<u64>,
}

impl Loaded {
    pub fn files_in(&self, folder: &str) -> &[usize] {
        self.by_folder.get(&folder.to_lowercase()).map(Vec::as_slice).unwrap_or(&[])
    }
}

struct Opened {
    loaded: Loaded,
    notes: Vec<String>,
}

fn load(path: &Path, label: String, client_dir: Option<PathBuf>, job: &shaiya_core::Job) -> Result<Opened> {
    let mut notes = Vec::new();
    if let Some(dir) = &client_dir {
        match client::recover(dir) {
            Ok(n) => notes.extend(n),
            Err(e) => notes.push(format!("warning: {e}")),
        }
    }
    job.stage("reading index");
    let (sah, index) = Archive::open_index(path)?;
    let rows: Vec<RowData> = index.entries().into_iter().map(|f| RowData { lower: f.path.to_lowercase(), path: f.path, entry: f.entry.clone(), shadowed: f.shadowed }).collect();
    let visible = rows.iter().filter(|r| !r.shadowed).count();
    let sah_version = index.version;
    let saf_ok = shaiya_archive::saf_of(&sah).is_file();
    let (archive, saf_len) = if saf_ok {
        let a = Archive::open_parsed(sah.clone(), index)?;
        let n = a.saf_len();
        (Some(Arc::new(a)), Some(n))
    } else {
        notes.push("data.saf not found: browsing the index only; preview, extract and changes are off".to_string());
        (None, None)
    };
    let dead = saf_len.map(|n| shaiya_archive::layout::analyse(archive.as_ref().map(|a| a.index()).unwrap_or(&Index::new()), Some(n)).dead_bytes.unwrap_or(0));
    let tree = Tree::build(&rows);
    let mut by_folder: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        by_folder.entry(r.folder().to_lowercase()).or_default().push(i);
    }
    Ok(Opened { loaded: Loaded { label, sah, client_dir, archive, rows, tree, by_folder, sah_version, visible, saf_len, dead }, notes })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Packed,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewRow {
    Folder(usize),
    File(usize),
}

const TYPE_CHIPS: [(&str, &[&str]); 6] = [
    ("Textures", &["dds", "tga", "bmp"]),
    ("Models", &["3do", "3dc", "smod"]),
    ("Sound", &["wav"]),
    ("World", &["wld", "dg", "svmap"]),
    ("SData", &["sdata"]),
    ("Text", &["txt", "ini", "xml", "csv"]),
];

// ------------------------------------------------------------------------------------------------ pending changes

#[derive(Clone, Debug, PartialEq)]
pub enum PendingKind {
    Put { source: PathBuf, size: u64, version: i32, replaces: bool },
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub path: String,
    pub kind: PendingKind,
}

#[derive(Clone, Debug)]
pub enum QueueEdit {
    Insert(usize, Pending),
    Delete(usize, Pending),
}

impl Edit<Vec<Pending>> for QueueEdit {
    fn apply(&self, model: &mut Vec<Pending>) {
        match self {
            QueueEdit::Insert(i, p) => model.insert((*i).min(model.len()), p.clone()),
            QueueEdit::Delete(i, _) => {
                if *i < model.len() {
                    model.remove(*i);
                }
            }
        }
    }
    fn invert(&self) -> QueueEdit {
        match self {
            QueueEdit::Insert(i, p) => QueueEdit::Delete(*i, p.clone()),
            QueueEdit::Delete(i, p) => QueueEdit::Insert(*i, p.clone()),
        }
    }
}

/// One row of the "Add to archive" dialog.
#[derive(Clone, Debug)]
pub struct AddRow {
    pub path: String,
    pub source: PathBuf,
    pub size: u64,
    pub version: i32,
    /// Size and pack time of the file it replaces.
    pub replaces: Option<(u32, i32)>,
}

pub struct AddDialog {
    pub paths: Vec<PathBuf>,
    pub target: String,
    pub keep_folder_names: bool,
    pub rows: Vec<AddRow>,
    pub error: Option<String>,
}

// ------------------------------------------------------------------------------------------------ flows

enum Excl {
    Apply(EditPlan, usize),
    Compact,
}

enum ApplyFlow {
    Idle,
    /// Waiting for the answer to the confirmation.
    Confirm(Excl),
    /// The archive handle is being let go (tasks that hold it were cancelled).
    Closing(Excl, Option<Arc<Archive>>),
    Running(Excl, TaskHandle<Done>),
    /// The game (or another program) has the files: Retry or Cancel.
    InUse(Excl),
}

enum Done {
    Edit(EditReport),
    Compacted(compact::Compacted),
}

struct ExtractPlanned {
    count: usize,
    exists: usize,
    bytes: u64,
}

#[derive(Clone)]
struct ExtractReq {
    out: PathBuf,
    filter: Filter,
    flat: bool,
    keep_times: bool,
}

enum ExtractFlow {
    Planning(ExtractReq, TaskHandle<ExtractPlanned>),
    Confirm(ExtractReq, usize),
    Running(TaskHandle<ExtractReport>),
}

pub struct CheckPanel {
    pub report: Option<CheckReport>,
    handle: Option<TaskHandle<CheckReport>>,
    pub deep: bool,
}

pub struct ComparePanel {
    pub other: String,
    pub entries: Vec<DiffEntry>,
    pub summary: DiffSummary,
    handle: Option<TaskHandle<(Vec<DiffEntry>, DiffSummary)>>,
    pub show: [bool; 4],
    pub swap: bool,
}

pub struct PreviewState {
    pub row: Option<usize>,
    handle: Option<TaskHandle<PreviewData>>,
    pub data: Option<PreviewData>,
    pub text_all: bool,
    pub encoding: Encoding,
    decoded: Option<(Encoding, String)>,
    pub plain_view: bool,
    pub fit: bool,
    pub alpha: bool,
    texture_name: Option<String>,
    goto: String,
    pub error: Option<String>,
}

pub enum OpenSource {
    Client { name: String, dir: PathBuf },
    File(PathBuf),
}

pub struct ArchiveView {
    pub loaded: Option<Loaded>,
    pub source: Option<OpenSource>,
    opening: Option<(String, TaskHandle<Opened>)>,
    pub open_error: Option<String>,
    pub notes: Vec<String>,
    pub folder: String,
    pub search: String,
    pub types: [bool; 6],
    pub show_shadowed: bool,
    pub min_size: String,
    pub max_size: String,
    pub sort_key: SortKey,
    pub sort_desc: bool,
    view: Vec<ViewRow>,
    view_stamp: String,
    pub selected: BTreeSet<usize>,
    pub selected_folder: Option<usize>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    pub preview: PreviewState,
    textures: Vec<(String, egui::TextureHandle)>,
    pub pending: Vec<Pending>,
    history: History<QueueEdit>,
    pub add_dialog: Option<AddDialog>,
    pub check: Option<CheckPanel>,
    pub compare: Option<ComparePanel>,
    extract: Option<ExtractFlow>,
    apply: ApplyFlow,
    discard_asked: bool,
    reload_asked: bool,
    saved_new: Option<TaskHandle<EditReport>>,
    want_search_focus: bool,
    want_goto_focus: bool,
    pub status_line: String,
    /// Set when the last change finished (for tests and the Apply tab to reload its history).
    pub changed_counter: u64,
}

impl Default for ArchiveView {
    fn default() -> Self {
        Self::new()
    }
}

fn lower_key(path: &str) -> String {
    path.to_lowercase()
}

fn escape_glob(s: &str) -> String {
    s.replace('[', "[[]").replace('*', "[*]").replace('?', "[?]")
}

fn mtime(p: &Path) -> i32 {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs().min(i32::MAX as u64) as i32).unwrap_or(0)
}

impl ArchiveView {
    pub fn new() -> ArchiveView {
        ArchiveView {
            loaded: None,
            source: None,
            opening: None,
            open_error: None,
            notes: Vec::new(),
            folder: String::new(),
            search: String::new(),
            types: [false; 6],
            show_shadowed: false,
            min_size: String::new(),
            max_size: String::new(),
            sort_key: SortKey::Name,
            sort_desc: false,
            view: Vec::new(),
            view_stamp: String::new(),
            selected: BTreeSet::new(),
            selected_folder: None,
            anchor: None,
            cursor: None,
            preview: PreviewState {
                row: None,
                handle: None,
                data: None,
                text_all: false,
                encoding: Encoding::Cp949,
                decoded: None,
                plain_view: false,
                fit: true,
                alpha: true,
                texture_name: None,
                goto: String::new(),
                error: None,
            },
            textures: Vec::new(),
            pending: Vec::new(),
            history: History::new(500),
            add_dialog: None,
            check: None,
            compare: None,
            extract: None,
            apply: ApplyFlow::Idle,
            discard_asked: false,
            reload_asked: false,
            saved_new: None,
            want_search_focus: false,
            want_goto_focus: false,
            status_line: String::new(),
            changed_counter: 0,
        }
    }

    // ---------------------------------------------------------------------------------------- opening

    pub fn is_open(&self) -> bool {
        self.loaded.is_some()
    }

    pub fn is_opening(&self) -> bool {
        self.opening.is_some()
    }

    /// The client folder of the open archive.
    pub fn client_dir(&self) -> Option<&Path> {
        self.loaded.as_ref().and_then(|l| l.client_dir.as_deref())
    }

    pub fn close(&mut self) {
        self.cancel_background();
        self.loaded = None;
        self.opening = None;
        self.pending.clear();
        self.history = History::new(500);
        self.selected.clear();
        self.selected_folder = None;
        self.preview.row = None;
        self.preview.data = None;
        self.view.clear();
        self.view_stamp.clear();
        self.folder.clear();
        self.open_error = None;
    }

    fn cancel_background(&mut self) {
        if let Some(h) = &self.preview.handle {
            h.cancel();
        }
        self.preview.handle = None;
        if let Some(ExtractFlow::Planning(_, h)) = &self.extract {
            h.cancel();
        }
        if let Some(ExtractFlow::Running(h)) = &self.extract {
            h.cancel();
        }
        self.extract = None;
        if let Some(c) = &self.check {
            if let Some(h) = &c.handle {
                h.cancel();
            }
        }
        self.check = None;
        self.compare = None;
    }

    /// Opens a saved client folder (changes are possible) or any archive file (read only), as a task.
    pub fn open(&mut self, source: OpenSource, cx: &mut StudioCx<'_>) {
        self.close();
        let (path, label, client_dir) = match &source {
            OpenSource::Client { name, dir } => (dir.clone(), name.clone(), Some(dir.clone())),
            OpenSource::File(p) => (p.clone(), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), None),
        };
        if shaiya_ui::fonts::needs_cjk(&path.display().to_string()) {
            cx.shell.need_cjk_font();
        }
        let key = path.display().to_string();
        let title = format!("Opening {label}");
        let l2 = label.clone();
        let handle = cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: Some(key) }, move |job| load(&path, l2, client_dir, job));
        self.opening = Some((label, handle));
        self.source = Some(source);
    }

    pub fn reopen(&mut self, cx: &mut StudioCx<'_>) {
        let src = match &self.source {
            Some(OpenSource::Client { name, dir }) => OpenSource::Client { name: name.clone(), dir: dir.clone() },
            Some(OpenSource::File(p)) => OpenSource::File(p.clone()),
            None => return,
        };
        let (folder, pending, history) = (self.folder.clone(), std::mem::take(&mut self.pending), std::mem::replace(&mut self.history, History::new(500)));
        self.open(src, cx);
        self.folder = folder;
        self.pending = pending;
        self.history = history;
    }

    // ---------------------------------------------------------------------------------------- polling

    /// Collects finished tasks; call every frame.
    pub fn poll(&mut self, cx: &mut StudioCx<'_>) {
        let mut opened: Option<std::result::Result<Opened, String>> = None;
        if let Some((label, h)) = &mut self.opening {
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(o)) => opened = Some(Ok(o)),
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error(format!("Cannot open {label}"), &e);
                    opened = Some(Err(e.to_string()));
                }
                Some(TaskOutcome::Cancelled) => opened = Some(Err("cancelled".to_string())),
                Some(TaskOutcome::Panicked(m)) => opened = Some(Err(format!("internal error (a bug): {m}"))),
            }
        }
        if let Some(r) = opened {
            self.opening = None;
            match r {
                Ok(o) => {
                    for n in &o.notes {
                        cx.shell.push_activity(if n.starts_with("warning") { shaiya_core::Level::Warn } else { shaiya_core::Level::Info }, n.clone());
                    }
                    self.notes = o.notes;
                    cx.shell.log_info(&format!("opened {}", o.loaded.sah.display()));
                    self.loaded = Some(o.loaded);
                    self.open_error = None;
                    self.view_stamp.clear();
                    if self.folder_node().is_none() {
                        self.folder.clear();
                    }
                }
                Err(e) => {
                    self.loaded = None;
                    self.open_error = Some(e);
                }
            }
        }
        self.poll_saved_new(cx);
        self.poll_preview(cx);
        self.poll_extract(cx);
        self.poll_check(cx);
        self.poll_compare(cx);
        self.poll_apply(cx);
    }

    fn poll_saved_new(&mut self, cx: &mut StudioCx<'_>) {
        let Some(h) = &mut self.saved_new else { return };
        match h.poll() {
            None => {}
            Some(TaskOutcome::Done(r)) => {
                self.saved_new = None;
                let n = r.added.len() + r.replaced.len() + r.removed.len();
                self.status_line = format!("Saved a new archive ({n} changes included)");
                cx.shell.status(self.status_line.clone());
            }
            Some(TaskOutcome::Failed(e)) => {
                self.saved_new = None;
                cx.shell.error("The new archive was not written", &e);
            }
            Some(TaskOutcome::Cancelled) => {
                self.saved_new = None;
                cx.shell.status("Cancelled; nothing was written");
            }
            Some(TaskOutcome::Panicked(m)) => {
                self.saved_new = None;
                cx.shell.error("The new archive was not written", &Error::Unsupported(format!("internal error (a bug): {m}")));
            }
        }
    }

    /// The name shown in the toolbar for an archive opened from a file.
    pub fn source_name(&self) -> Option<String> {
        self.source_label()
    }

    /// Whether the open source is this folder (a saved client or an opened client folder).
    pub fn source_dir_is(&self, dir: &Path) -> bool {
        matches!(&self.source, Some(OpenSource::Client { dir: d, .. }) if d == dir)
    }

    /// Studio's handle on `data.saf` (for tasks that read it; they must not outlive a write).
    pub fn archive_handle(&self) -> Option<Arc<Archive>> {
        self.loaded.as_ref().and_then(|l| l.archive.clone())
    }

    pub fn sah_path(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|l| l.sah.as_path())
    }

    /// Size and pack time of a visible entry (the patch builder's "replaces ..." column).
    pub fn lookup(&self, path: &str) -> Option<(u32, i32)> {
        let i = self.find_row(path)?;
        self.loaded.as_ref().map(|l| (l.rows[i].entry.length, l.rows[i].entry.version))
    }

    fn folder_node(&self) -> Option<usize> {
        self.loaded.as_ref().and_then(|l| l.tree.find(&self.folder))
    }

    fn poll_preview(&mut self, cx: &mut StudioCx<'_>) {
        let Some(h) = &mut self.preview.handle else { return };
        match h.poll() {
            None => {}
            Some(TaskOutcome::Done(d)) => {
                self.preview.handle = None;
                self.preview.error = None;
                self.preview.decoded = None;
                if let PreviewData::Text { guess, .. } = &d {
                    let want = preview::encoding_by_name(&cx.settings.text_encoding);
                    self.preview.encoding = want.unwrap_or(*guess);
                }
                if let PreviewData::Image { width, height, rgba, .. } = &d {
                    let name = self.preview.row.and_then(|i| self.loaded.as_ref().map(|l| l.rows[i].path.clone())).unwrap_or_default();
                    let img = egui::ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], rgba);
                    let tex = cx.shell.ctx().load_texture(format!("preview-{name}"), img, egui::TextureOptions::LINEAR);
                    self.textures.retain(|(n, _)| n != &name);
                    self.textures.push((name.clone(), tex));
                    while self.textures.len() > 8 {
                        drop(self.textures.remove(0));
                    }
                    self.preview.texture_name = Some(name);
                }
                self.preview.data = Some(d);
            }
            Some(TaskOutcome::Failed(e)) => {
                self.preview.handle = None;
                self.preview.error = Some(e.to_string());
                self.preview.data = None;
            }
            Some(TaskOutcome::Cancelled) => self.preview.handle = None,
            Some(TaskOutcome::Panicked(m)) => {
                self.preview.handle = None;
                self.preview.error = Some(format!("internal error (a bug): {m}"));
            }
        }
    }

    fn poll_extract(&mut self, cx: &mut StudioCx<'_>) {
        match self.extract.take() {
            None => {}
            Some(ExtractFlow::Planning(req, mut h)) => match h.poll() {
                None => self.extract = Some(ExtractFlow::Planning(req, h)),
                Some(TaskOutcome::Done(p)) => {
                    if p.count == 0 {
                        cx.shell.status("Nothing to extract");
                    } else if p.exists > 0 {
                        self.extract = Some(ExtractFlow::Confirm(req, p.exists));
                    } else {
                        self.start_extract(req, Existing::Refuse, cx);
                    }
                    let _ = p.bytes;
                }
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Cannot plan the extraction", &e),
                Some(_) => {}
            },
            Some(ExtractFlow::Confirm(req, n)) => {
                let c = Confirm {
                    title: "Files already exist".into(),
                    text: format!("{n} files already exist in {}.", req.out.display()),
                    buttons: vec!["Skip existing".into(), "Overwrite".into(), "Cancel".into()],
                    danger: Some(1),
                };
                match cx.shell.confirm("extract-existing", &c) {
                    None => self.extract = Some(ExtractFlow::Confirm(req, n)),
                    Some(0) => self.start_extract(req, Existing::Skip, cx),
                    Some(1) => self.start_extract(req, Existing::Overwrite, cx),
                    Some(_) => cx.shell.status("Extraction cancelled"),
                }
            }
            Some(ExtractFlow::Running(mut h)) => match h.poll() {
                None => self.extract = Some(ExtractFlow::Running(h)),
                Some(TaskOutcome::Done(r)) => {
                    let msg = format!("Extracted {} files ({})", r.written, human_size(r.bytes));
                    self.status_line = msg.clone();
                    cx.shell.status(msg);
                }
                Some(TaskOutcome::Cancelled) => {
                    self.status_line = "Cancelled".to_string();
                    cx.shell.status("Cancelled after some files (the files written so far stay)");
                    cx.shell.push_activity(shaiya_core::Level::Warn, "Extraction cancelled; the files written so far stay".to_string());
                }
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Extraction failed", &e),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("Extraction failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            },
        }
    }

    fn poll_check(&mut self, cx: &mut StudioCx<'_>) {
        let Some(c) = &mut self.check else { return };
        let Some(h) = &mut c.handle else { return };
        match h.poll() {
            None => {}
            Some(TaskOutcome::Done(r)) => {
                c.report = Some(r);
                c.handle = None;
            }
            Some(TaskOutcome::Failed(e)) => {
                cx.shell.error("Check failed", &e);
                c.handle = None;
            }
            Some(_) => c.handle = None,
        }
    }

    fn poll_compare(&mut self, cx: &mut StudioCx<'_>) {
        let Some(c) = &mut self.compare else { return };
        let Some(h) = &mut c.handle else { return };
        match h.poll() {
            None => {}
            Some(TaskOutcome::Done((entries, summary))) => {
                c.entries = entries;
                c.summary = summary;
                c.handle = None;
            }
            Some(TaskOutcome::Failed(e)) => {
                cx.shell.error("Comparison failed", &e);
                self.compare = None;
            }
            Some(_) => self.compare = None,
        }
    }

    fn poll_apply(&mut self, cx: &mut StudioCx<'_>) {
        match std::mem::replace(&mut self.apply, ApplyFlow::Idle) {
            ApplyFlow::Idle => {}
            ApplyFlow::Confirm(ex) => {
                let (title, text, button) = self.confirm_text(&ex, cx);
                let c = Confirm { title, text, buttons: vec![button, "Cancel".into()], danger: Some(0) };
                match cx.shell.confirm("studio-exclusive", &c) {
                    None => self.apply = ApplyFlow::Confirm(ex),
                    Some(0) => self.begin_closing(ex),
                    Some(_) => {}
                }
            }
            ApplyFlow::Closing(ex, arc) => {
                // wait until nothing else (a preview, an extraction) holds the archive, then let go of it and start
                let free = arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true);
                if free {
                    drop(arc);
                    self.start_exclusive(ex, cx);
                } else {
                    cx.shell.ctx().request_repaint();
                    self.apply = ApplyFlow::Closing(ex, arc);
                }
            }
            ApplyFlow::Running(ex, mut h) => match h.poll() {
                None => self.apply = ApplyFlow::Running(ex, h),
                Some(TaskOutcome::Done(done)) => {
                    match done {
                        Done::Edit(r) => {
                            let n = r.added.len() + r.replaced.len() + r.removed.len();
                            self.status_line = format!("Applied {n} changes (undo in Apply & history)");
                            cx.shell.status(self.status_line.clone());
                            self.pending.clear();
                            self.history = History::new(500);
                        }
                        Done::Compacted(c) => {
                            self.status_line = format!("Compacted: {} -> {}", human_size(c.before), human_size(c.after));
                            cx.shell.status(self.status_line.clone());
                        }
                    }
                    self.changed_counter += 1;
                    self.reopen(cx);
                }
                Some(TaskOutcome::Failed(e)) => {
                    let msg = e.to_string();
                    if msg.contains("open in another program") || msg.contains("is running") {
                        self.apply = ApplyFlow::InUse(ex);
                    } else {
                        cx.shell.error("The change was not made", &e);
                        self.reopen(cx);
                    }
                }
                Some(TaskOutcome::Cancelled) => {
                    cx.shell.status("Cancelled; the archive is as it was");
                    self.reopen(cx);
                }
                Some(TaskOutcome::Panicked(m)) => {
                    cx.shell.error("The change was not made", &Error::Unsupported(format!("internal error (a bug): {m}")));
                    self.reopen(cx);
                }
            },
            ApplyFlow::InUse(ex) => {
                let name = self.loaded.as_ref().map(|l| l.label.clone()).or_else(|| self.source_label()).unwrap_or_default();
                let c = Confirm {
                    title: "The archive is in use".into(),
                    text: format!("{name}'s data.saf is open in another program (is the game running?). Close it and press Retry."),
                    buttons: vec!["Retry".into(), "Cancel".into()],
                    danger: None,
                };
                match cx.shell.confirm("studio-in-use", &c) {
                    None => self.apply = ApplyFlow::InUse(ex),
                    Some(0) => self.start_exclusive(ex, cx),
                    Some(_) => self.reopen(cx),
                }
            }
        }
    }

    fn source_label(&self) -> Option<String> {
        match &self.source {
            Some(OpenSource::Client { name, .. }) => Some(name.clone()),
            Some(OpenSource::File(p)) => p.file_name().map(|n| n.to_string_lossy().into_owned()),
            None => None,
        }
    }

    fn confirm_text(&self, ex: &Excl, cx: &StudioCx<'_>) -> (String, String, String) {
        let name = self.source_label().unwrap_or_default();
        let dir = self.client_dir().map(|d| d.display().to_string()).unwrap_or_default();
        let last = if cfg!(windows) { "The game must be closed." } else { "Make sure the game is not running from this folder; this cannot be checked on this system." };
        let _ = cx;
        match ex {
            Excl::Apply(plan, n) => {
                let bytes: u64 = plan.puts.iter().map(|p| p.source.len().unwrap_or(0)).sum();
                (
                    "Apply changes".into(),
                    format!(
                        "Apply {n} changes to {name} ({dir})? {} will be appended to data.saf. A backup of the index (and of files it overwrites) goes to shaiya-toolkit-backups\\...; Apply & history can roll it back. {last}",
                        human_size(bytes)
                    ),
                    format!("Apply {n} changes"),
                )
            }
            Excl::Compact => {
                let live = self.loaded.as_ref().map(|l| l.saf_len.unwrap_or(0).saturating_sub(l.dead.unwrap_or(0))).unwrap_or(0);
                let shadowed = self.loaded.as_ref().map(|l| l.rows.iter().filter(|r| r.shadowed).count()).unwrap_or(0);
                let later = self.client_dir().and_then(|d| journal::list(d, &d.join(journal::DEFAULT_DIR)).ok()).map(|l| l.iter().filter(|(j, _)| j.state == journal::State::Applied).count()).unwrap_or(0);
                (
                    "Compact".into(),
                    format!(
                        "Compact data.saf of {name}? It rewrites {} (needs about {} free on that drive) and drops {shadowed} shadowed entries and the free list. {later} earlier changes can no longer be rolled back afterwards.",
                        human_size(live),
                        human_size(live + (256 << 20))
                    ),
                    "Compact".into(),
                )
            }
        }
    }

    /// Cancels every task that reads the archive and hands out Studio's handle on `data.saf`, so a write (apply, compact, swap, patch
    /// apply) can take the files exclusively. The caller waits until `Arc::strong_count` is 1 (a cancelled task may still hold a clone for
    /// a moment) and drops it. `reopen` reads the archive again afterwards.
    pub fn release_handle(&mut self) -> Option<Arc<Archive>> {
        if let Some(h) = &self.preview.handle {
            h.cancel();
        }
        self.preview.handle = None;
        match &self.extract {
            Some(ExtractFlow::Running(h)) => h.cancel(),
            Some(ExtractFlow::Planning(_, h)) => h.cancel(),
            _ => {}
        }
        self.extract = None;
        if let Some(c) = &self.check {
            if let Some(h) = &c.handle {
                h.cancel();
            }
        }
        self.check = None;
        if let Some(c) = &self.compare {
            if let Some(h) = &c.handle {
                h.cancel();
            }
        }
        self.compare = None;
        self.loaded.as_mut().and_then(|l| l.archive.take())
    }

    fn begin_closing(&mut self, ex: Excl) {
        let arc = self.release_handle();
        self.apply = ApplyFlow::Closing(ex, arc);
    }

    fn start_exclusive(&mut self, ex: Excl, cx: &mut StudioCx<'_>) {
        let Some(dir) = self.client_dir().map(Path::to_path_buf) else { return };
        let key = dir.display().to_string();
        let ex2 = match &ex {
            Excl::Apply(p, n) => Excl::Apply(p.clone(), *n),
            Excl::Compact => Excl::Compact,
        };
        let handle = match ex {
            Excl::Apply(plan, _) => {
                let root = dir.join(journal::DEFAULT_DIR);
                cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {} changes", plan.puts.len() + plan.removes.len()), cancellable: true, exclusive: Some(key) }, move |job| {
                    let opts = InPlaceOptions { journal: JournalMode::In(root), legacy_bak: true, check_in_use: true };
                    edit::apply_in_place(&dir, &plan, &opts, job).map(Done::Edit)
                })
            }
            Excl::Compact => cx.shell.tasks().spawn(TaskSpec { title: "Compacting data.saf".into(), cancellable: true, exclusive: Some(key) }, move |job| {
                let files = ClientFiles::locate(&dir)?;
                client::ensure_not_in_use(&files)?;
                compact::compact(&dir, job).map(Done::Compacted)
            }),
        };
        self.apply = ApplyFlow::Running(ex2, handle);
    }

    // ---------------------------------------------------------------------------------------- pending changes

    pub fn unsaved(&self) -> Vec<String> {
        if self.pending.is_empty() {
            Vec::new()
        } else {
            vec![format!("{} pending archive changes", self.pending.len())]
        }
    }

    fn push_edit(&mut self, label: &str, edits: Vec<QueueEdit>) {
        let mut model = std::mem::take(&mut self.pending);
        self.history.push(&mut model, label, edits, None);
        self.pending = model;
    }

    pub fn undo(&mut self) -> bool {
        let mut m = std::mem::take(&mut self.pending);
        let r = self.history.undo(&mut m).is_some();
        self.pending = m;
        r
    }

    pub fn redo(&mut self) -> bool {
        let mut m = std::mem::take(&mut self.pending);
        let r = self.history.redo(&mut m).is_some();
        self.pending = m;
        r
    }

    pub fn find_row(&self, path: &str) -> Option<usize> {
        let l = self.loaded.as_ref()?;
        let key = lower_key(path);
        l.rows.iter().rposition(|r| !r.shadowed && r.lower == key)
    }

    /// Queues puts (path, source file); a path queued before is replaced.
    pub fn queue_puts(&mut self, items: Vec<(String, PathBuf, i32)>) {
        let mut edits = Vec::new();
        let mut working = self.pending.clone();
        for (path, source, version) in items {
            let size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
            let replaces = self.find_row(&path).is_some();
            let p = Pending { path: path.clone(), kind: PendingKind::Put { source, size, version, replaces } };
            if let Some(i) = working.iter().position(|q| lower_key(&q.path) == lower_key(&path)) {
                let old = working.remove(i);
                edits.push(QueueEdit::Delete(i, old));
            }
            edits.push(QueueEdit::Insert(working.len(), p.clone()));
            working.push(p);
        }
        self.push_edit("add files", edits);
    }

    pub fn queue_removes(&mut self, paths: Vec<String>) {
        let mut edits = Vec::new();
        let mut working = self.pending.clone();
        for path in paths {
            if let Some(i) = working.iter().position(|q| lower_key(&q.path) == lower_key(&path)) {
                let old = working.remove(i);
                edits.push(QueueEdit::Delete(i, old));
            }
            if self.find_row(&path).is_some() {
                let p = Pending { path: path.clone(), kind: PendingKind::Remove };
                edits.push(QueueEdit::Insert(working.len(), p.clone()));
                working.push(p);
            }
        }
        self.push_edit("remove files", edits);
    }

    pub fn unqueue(&mut self, index: usize) {
        if let Some(p) = self.pending.get(index).cloned() {
            self.push_edit("remove from the queue", vec![QueueEdit::Delete(index, p)]);
        }
    }

    pub fn discard_pending(&mut self) {
        let edits: Vec<QueueEdit> = self.pending.iter().cloned().enumerate().rev().map(|(i, p)| QueueEdit::Delete(i, p)).collect();
        self.push_edit("discard", edits);
    }

    /// The queue as an edit plan (puts replace or add, removals ignore missing).
    pub fn pending_plan(&self, label: &str) -> EditPlan {
        let mut plan = EditPlan { label: label.to_string(), ignore_missing: true, ..EditPlan::default() };
        for p in &self.pending {
            match &p.kind {
                PendingKind::Put { source, version, .. } => plan.puts.push(Put { path: p.path.clone(), source: Source::File(source.clone()), version: *version, mode: PutMode::AddOrReplace }),
                PendingKind::Remove => plan.removes.push(p.path.clone()),
            }
        }
        plan
    }

    /// The queue as a patch builder input.
    pub fn pending_build_input(&self) -> shaiya_patch::BuildInput {
        let mut input = shaiya_patch::BuildInput::default();
        for p in &self.pending {
            match &p.kind {
                PendingKind::Put { source, version, .. } => input.files.push((p.path.clone(), Source::File(source.clone()), *version)),
                PendingKind::Remove => input.removals.push(p.path.clone()),
            }
        }
        input
    }

    /// "Apply to client…": the confirmation, then the exclusive apply.
    pub fn request_apply(&mut self, cx: &mut StudioCx<'_>) {
        if self.pending.is_empty() || self.client_dir().is_none() || !matches!(self.apply, ApplyFlow::Idle) {
            return;
        }
        let plan = self.pending_plan("archive edit");
        let n = self.pending.len();
        let _ = cx;
        self.apply = ApplyFlow::Confirm(Excl::Apply(plan, n));
    }

    pub fn request_compact(&mut self) {
        if self.client_dir().is_none() || !matches!(self.apply, ApplyFlow::Idle) {
            return;
        }
        self.apply = ApplyFlow::Confirm(Excl::Compact);
    }

    pub fn is_applying(&self) -> bool {
        !matches!(self.apply, ApplyFlow::Idle)
    }

    /// "Save as new archive…": `write_new` of the loaded archive plus the queue.
    pub fn save_as_new(&mut self, out_sah: PathBuf, cx: &mut StudioCx<'_>) {
        let Some(a) = self.loaded.as_ref().and_then(|l| l.archive.clone()) else { return };
        let plan = self.pending_plan("save as new archive");
        let title = format!("Writing {}", out_sah.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let h = cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: None }, move |job| edit::write_new(&a, &plan, &out_sah, Existing::Refuse, job));
        // the result is reported through the Activity list and the status line when it finishes
        self.saved_new = Some(h);
    }

    // ---------------------------------------------------------------------------------------- filtering and view

    fn filter_stamp(&self) -> String {
        format!(
            "{}|{}|{}|{:?}|{}|{}|{:?}|{}|{}|{}",
            self.folder,
            self.search,
            self.show_shadowed,
            self.types,
            self.min_size,
            self.max_size,
            self.sort_key,
            self.sort_desc,
            self.loaded.as_ref().map(|l| l.rows.len()).unwrap_or(0),
            self.loaded.as_ref().map(|l| l.sah.display().to_string()).unwrap_or_default()
        )
    }

    fn type_ok(&self, ext: &str) -> bool {
        if !self.types.iter().any(|&t| t) {
            return true;
        }
        let known = TYPE_CHIPS.iter().position(|(_, e)| e.contains(&ext));
        match known {
            Some(i) => self.types[i],
            None => false,
        }
    }

    fn size_filters(&self) -> (Option<u64>, Option<u64>) {
        (shaiya_core::text::parse_size(&self.min_size).ok(), shaiya_core::text::parse_size(&self.max_size).ok())
    }

    /// Recomputes the table rows when an input changed.
    pub fn refresh_view(&mut self) {
        let stamp = self.filter_stamp();
        if stamp == self.view_stamp {
            return;
        }
        self.view_stamp = stamp;
        self.view.clear();
        let Some(l) = &self.loaded else { return };
        let (min, max) = self.size_filters();
        let ok_size = |s: u32| min.is_none_or(|m| s as u64 >= m) && max.is_none_or(|m| s as u64 <= m);
        let mut files: Vec<usize> = Vec::new();
        let searching = !self.search.trim().is_empty();
        if searching {
            let q = self.search.trim();
            let glob = if q.contains(['*', '?', '[']) { Pattern::new(q).ok() } else { None };
            let ql = q.to_lowercase();
            for (i, r) in l.rows.iter().enumerate() {
                if r.shadowed && !self.show_shadowed {
                    continue;
                }
                let hit = match &glob {
                    Some(g) => g.matches(&r.path),
                    None => r.lower.contains(&ql),
                };
                if hit && ok_size(r.entry.length) && self.type_ok(&r.ext()) {
                    files.push(i);
                }
            }
        } else {
            if let Some(n) = l.tree.find(&self.folder) {
                for &c in &l.tree.nodes[n].children {
                    self.view.push(ViewRow::Folder(c));
                }
            }
            for &i in l.files_in(&self.folder) {
                let r = &l.rows[i];
                if (r.shadowed && !self.show_shadowed) || !ok_size(r.entry.length) || !self.type_ok(&r.ext()) {
                    continue;
                }
                files.push(i);
            }
        }
        let key = self.sort_key;
        files.sort_by(|&a, &b| {
            let (x, y) = (&l.rows[a], &l.rows[b]);
            let o = match key {
                SortKey::Name => x.lower.cmp(&y.lower),
                SortKey::Size => x.entry.length.cmp(&y.entry.length),
                SortKey::Packed => x.entry.version.cmp(&y.entry.version),
                SortKey::Type => x.ext().cmp(&y.ext()).then(x.lower.cmp(&y.lower)),
            };
            if self.sort_desc { o.reverse() } else { o }
        });
        self.view.extend(files.into_iter().map(ViewRow::File));
    }

    pub fn view_rows(&self) -> &[ViewRow] {
        &self.view
    }

    /// Paths of the rows shown (files only).
    pub fn shown_paths(&self) -> Vec<String> {
        let Some(l) = &self.loaded else { return Vec::new() };
        self.view.iter().filter_map(|v| if let ViewRow::File(i) = v { Some(l.rows[*i].path.clone()) } else { None }).collect()
    }

    pub fn visible_count(&self) -> usize {
        self.loaded.as_ref().map(|l| l.visible).unwrap_or(0)
    }

    pub fn enter_folder(&mut self, path: &str) {
        self.folder = path.to_string();
        self.search.clear();
        self.selected.clear();
        self.selected_folder = None;
        self.cursor = None;
    }

    pub fn parent_folder(&mut self) {
        let parent = self.folder.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default();
        self.enter_folder(&parent);
    }

    // ---------------------------------------------------------------------------------------- selection and preview

    /// Selects one file by path (test and keyboard entry point) and starts its preview.
    pub fn select_path(&mut self, path: &str, cx: &mut StudioCx<'_>) {
        let Some(i) = self.find_row(path) else { return };
        let folder = self.loaded.as_ref().map(|l| l.rows[i].folder().to_string()).unwrap_or_default();
        if self.search.is_empty() {
            self.folder = folder;
        }
        self.selected.clear();
        self.selected.insert(i);
        self.anchor = Some(i);
        self.start_preview(i, cx);
    }

    pub fn select_folder_row(&mut self, node: usize) {
        self.selected.clear();
        self.selected_folder = Some(node);
    }

    fn start_preview(&mut self, row: usize, cx: &mut StudioCx<'_>) {
        if let Some(h) = &self.preview.handle {
            h.cancel();
        }
        self.preview.handle = None;
        self.preview.row = Some(row);
        self.preview.data = None;
        self.preview.error = None;
        self.preview.decoded = None;
        self.preview.plain_view = false;
        let Some(l) = &self.loaded else { return };
        let Some(a) = l.archive.clone() else { return };
        let r = &l.rows[row];
        let (entry, path, all) = (r.entry.clone(), r.path.clone(), self.preview.text_all);
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Preview {}", r.name()), cancellable: true, exclusive: None }, move |job| preview::make_preview(&a, &entry, &path, all, 0, job));
        self.preview.handle = Some(h);
    }

    /// The kind of the current preview ("image", "text", "sdata", "hex", "empty", "failed"), once it is ready.
    pub fn preview_kind(&self) -> Option<&'static str> {
        self.preview.data.as_ref().map(|d| d.kind())
    }

    pub fn preview_ready(&self) -> bool {
        self.preview.handle.is_none() && (self.preview.data.is_some() || self.preview.error.is_some() || self.preview.row.is_none())
    }

    pub fn preview_data(&self) -> Option<&PreviewData> {
        self.preview.data.as_ref()
    }

    pub fn has_texture(&self) -> bool {
        self.preview.texture_name.as_ref().is_some_and(|n| self.textures.iter().any(|(t, _)| t == n)) && matches!(self.preview.data, Some(PreviewData::Image { .. }))
    }

    // ---------------------------------------------------------------------------------------- extraction

    fn selection_filter(&self, all_shown: bool) -> Filter {
        let mut f = Filter::default();
        let Some(l) = &self.loaded else { return f };
        let mut add = |s: String| {
            if let Ok(p) = Pattern::new(&s) {
                f.include.push(p);
            }
        };
        if all_shown {
            for v in &self.view {
                match v {
                    ViewRow::File(i) => add(format!("/{}", escape_glob(&l.rows[*i].path))),
                    ViewRow::Folder(n) => add(format!("/{}/", escape_glob(&l.tree.nodes[*n].path))),
                }
            }
        } else {
            for &i in &self.selected {
                add(format!("/{}", escape_glob(&l.rows[i].path)));
            }
            if let Some(n) = self.selected_folder {
                add(format!("/{}/", escape_glob(&l.tree.nodes[n].path)));
            }
        }
        f
    }

    /// Plans an extraction as a task, then asks about existing files, then extracts.
    pub fn begin_extract(&mut self, out: PathBuf, all_shown: bool, whole_archive: bool, flat: bool, cx: &mut StudioCx<'_>) {
        let Some(a) = self.loaded.as_ref().and_then(|l| l.archive.clone()) else { return };
        let filter = if whole_archive { Filter::default() } else { self.selection_filter(all_shown) };
        if !whole_archive && filter.include.is_empty() {
            cx.shell.status("Select something to extract first");
            return;
        }
        let req = ExtractReq { out, filter, flat, keep_times: cx.settings.extract_keep_times };
        let r2 = req.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Planning the extraction".into(), cancellable: true, exclusive: None }, move |_job| {
            let opts = ExtractOptions { filter: r2.filter.clone(), include_shadowed: false, existing: Existing::Overwrite, keep_times: r2.keep_times };
            let plan = extract::plan(a.index(), &r2.out, &opts)?;
            Ok(ExtractPlanned { count: plan.len(), exists: plan.iter().filter(|p| p.exists).count(), bytes: plan.iter().map(|p| p.entry.length as u64).sum() })
        });
        self.extract = Some(ExtractFlow::Planning(req, h));
    }

    fn start_extract(&mut self, req: ExtractReq, existing: Existing, cx: &mut StudioCx<'_>) {
        let Some(a) = self.loaded.as_ref().and_then(|l| l.archive.clone()) else { return };
        let title = format!("Extracting to {}", req.out.display());
        let h = cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: None }, move |job| {
            let opts = ExtractOptions { filter: req.filter.clone(), include_shadowed: false, existing, keep_times: req.keep_times };
            if !req.flat {
                return extract::extract(&a, &req.out, &opts, job);
            }
            // flat: every file straight into the folder, by its own name
            let plan = extract::plan(a.index(), &req.out, &opts)?;
            std::fs::create_dir_all(&req.out).map_err(|e| Error::io(&req.out, e))?;
            job.stage("extracting");
            job.total(Some(plan.iter().map(|p| p.entry.length as u64).sum()), Unit::Bytes);
            let mut rep = ExtractReport::default();
            let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
            for p in &plan {
                job.check()?;
                let mut name = p.target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
                let mut n = 1;
                while !used.insert(name.to_lowercase()) {
                    n += 1;
                    name = match p.target.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default().rsplit_once('.') {
                        Some((s, e)) => format!("{s}~{n}.{e}"),
                        None => format!("{}~{n}", p.target.display()),
                    };
                }
                let name = portable_name(&name).unwrap_or(name);
                let target = req.out.join(&name);
                if target.exists() && existing == Existing::Skip {
                    rep.skipped_existing.push(p.path.clone());
                    continue;
                }
                let mut r = a.reader(&p.entry)?;
                rep.bytes += write_atomic_from_with(&target, &mut r, Sync::No, job)?;
                rep.written += 1;
            }
            Ok(rep)
        });
        self.extract = Some(ExtractFlow::Running(h));
    }

    pub fn is_extracting(&self) -> bool {
        matches!(self.extract, Some(ExtractFlow::Running(_)) | Some(ExtractFlow::Planning(..)))
    }

    pub fn cancel_extract(&mut self) {
        if let Some(ExtractFlow::Running(h)) = &self.extract {
            h.cancel();
        }
    }

    /// Whether the existing-files question is open (tests answer it through the shell).
    pub fn extract_asking(&self) -> bool {
        matches!(self.extract, Some(ExtractFlow::Confirm(..)))
    }

    // ---------------------------------------------------------------------------------------- check / compare / add

    pub fn run_check(&mut self, deep: bool, cx: &mut StudioCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let (archive, label) = (l.archive.clone(), l.label.clone());
        let handle = match archive {
            Some(a) => cx.shell.tasks().spawn(TaskSpec { title: format!("Checking {label}{}", if deep { " (deep)" } else { "" }), cancellable: true, exclusive: None }, move |job| check_archive(&a, &CheckOptions { deep }, job)),
            None => {
                let idx = l.rows.len();
                let _ = idx;
                cx.shell.status("The check needs data.saf");
                return;
            }
        };
        self.check = Some(CheckPanel { report: None, handle: Some(handle), deep });
    }

    pub fn run_compare(&mut self, other: PathBuf, cx: &mut StudioCx<'_>) {
        let Some(a) = self.loaded.as_ref().and_then(|l| l.archive.clone()) else { return };
        let label = other.display().to_string();
        let o2 = other.clone();
        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Comparing with {label}"), cancellable: true, exclusive: None }, move |job| {
            let b = Archive::open(&o2)?;
            let d = diff(&Side::Archive(&b), &Side::Archive(&a), &DiffOptions { content: true, ..Default::default() }, job)?;
            let s = summarize(&d);
            Ok((d, s))
        });
        self.compare = Some(ComparePanel { other: label, entries: Vec::new(), summary: DiffSummary::default(), handle: Some(handle), show: [true, true, true, true], swap: false });
    }

    /// Opens the "Add to archive" dialog for dropped or chosen files and folders.
    pub fn begin_add(&mut self, paths: Vec<PathBuf>) {
        let target = self.folder.clone();
        let mut d = AddDialog { paths, target, keep_folder_names: false, rows: Vec::new(), error: None };
        self.recompute_add(&mut d);
        self.add_dialog = Some(d);
    }

    pub fn recompute_add(&self, d: &mut AddDialog) {
        d.rows.clear();
        d.error = None;
        let mut inputs = Vec::new();
        let mut prefix_names: Vec<(PathBuf, String)> = Vec::new();
        for p in &d.paths {
            if p.is_dir() && d.keep_folder_names {
                prefix_names.push((p.clone(), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
            } else {
                inputs.push(p.clone());
            }
        }
        let join = |t: &str, rel: &str| {
            let t = t.trim_matches(['/', '\\']);
            if t.is_empty() { rel.to_string() } else { format!("{t}/{rel}") }
        };
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        let mut add_source = |paths: &[PathBuf], extra: &str, d: &mut AddDialog| match shaiya_core::source::FolderSource::new(paths, &Filter::default()) {
            Ok(s) => {
                use shaiya_core::source::ByteSource;
                for n in s.names() {
                    if let Some(p) = s.file_path(&n) {
                        let rel = if extra.is_empty() { n } else { format!("{extra}/{n}") };
                        found.push((join(&d.target, &rel), p));
                    }
                }
            }
            Err(e) => d.error = Some(e.to_string()),
        };
        add_source(&inputs, "", d);
        for (p, name) in &prefix_names {
            add_source(std::slice::from_ref(p), name, d);
        }
        for (path, source) in found {
            let size = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
            let replaces = self.find_row(&path).and_then(|i| self.loaded.as_ref().map(|l| (l.rows[i].entry.length, l.rows[i].entry.version)));
            d.rows.push(AddRow { path, version: mtime(&source), source, size, replaces });
        }
    }

    pub fn queue_add_dialog(&mut self) {
        if let Some(d) = self.add_dialog.take() {
            let items = d.rows.iter().map(|r| (r.path.clone(), r.source.clone(), r.version)).collect();
            self.queue_puts(items);
        }
    }

    // ---------------------------------------------------------------------------------------- texts for tests

    /// The names in the table, folders with a trailing slash.
    pub fn table_names(&self) -> Vec<String> {
        let Some(l) = &self.loaded else { return Vec::new() };
        self.view
            .iter()
            .map(|v| match v {
                ViewRow::Folder(n) => format!("{}/", l.tree.nodes[*n].name),
                ViewRow::File(i) => l.rows[*i].name().to_string(),
            })
            .collect()
    }

    pub fn current_folder(&self) -> &str {
        &self.folder
    }
}


// ------------------------------------------------------------------------------------------------ drawing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTo {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

enum Act {
    Folder(String),
    Up,
    Click { view: usize, ctrl: bool, shift: bool },
    Open(String),
    Extract { all: bool, flat: bool },
    AddFiles,
    QueueRemove,
    CopyPaths,
    ReplaceWith(usize),
    DecryptTo(usize),
    Sort(SortKey),
    TreeSelect(String),
    ApplyPending,
    SaveNew,
    Discard,
    Unqueue(usize),
    Check(bool),
    Compare,
    Compact,
    MakePatch,
    ClearSearch,
    Reload,
    SelectAll,
    Move(MoveTo),
}

impl ArchiveView {
    fn color_for(&self, r: &RowData, pal: &shaiya_ui::theme::Pal) -> Color32 {
        let key = lower_key(&r.path);
        match self.pending.iter().find(|p| lower_key(&p.path) == key).map(|p| &p.kind) {
            Some(PendingKind::Remove) => pal.error,
            Some(PendingKind::Put { .. }) => pal.warning,
            None => {
                if r.shadowed {
                    pal.weak
                } else {
                    pal.text
                }
            }
        }
    }

    /// Palette entries of the Archive tab, each with its shortcut.
    pub fn commands(&self) -> Vec<Command> {
        use egui::{Key, KeyboardShortcut as K, Modifiers as M};
        let open = self.loaded.as_ref().is_some_and(|l| l.archive.is_some());
        let edit = open && self.client_dir().is_some();
        let busy = self.is_applying();
        let c = |id: &'static str, label: &str, shortcut: Option<K>, enabled: bool| Command { id, label: label.to_string(), shortcut, enabled };
        vec![
            c("archive-search", "Search the archive", Some(K::new(M::COMMAND, Key::F)), self.is_open()),
            c("archive-extract", "Extract selected files...", Some(K::new(M::COMMAND, Key::E)), open && (!self.selected.is_empty() || self.selected_folder.is_some())),
            c("archive-extract-all", "Extract all shown files...", Some(K::new(M::COMMAND | M::SHIFT, Key::E)), open),
            c("archive-add", "Add files to the archive...", Some(K::new(M::NONE, Key::Insert)), edit),
            c("archive-remove", "Queue removal of selected files", Some(K::new(M::NONE, Key::Delete)), edit && !self.selected.is_empty()),
            c("archive-apply", "Apply pending changes to the client...", Some(K::new(M::COMMAND, Key::Enter)), edit && !busy && !self.pending.is_empty()),
            c("archive-save-new", "Save as new archive...", None, open && !self.pending.is_empty()),
            c("archive-make-patch", "Make patch from pending changes", None, !self.pending.is_empty()),
            c("archive-discard", "Discard pending changes", None, !self.pending.is_empty()),
            c("archive-check", "Check the archive", None, open),
            c("archive-compare", "Compare with another archive...", None, open),
            c("archive-compact", "Compact data.saf...", None, edit && !busy),
            c("archive-reload", "Reload the archive", Some(K::new(M::NONE, Key::F5)), self.source.is_some()),
            c("archive-copy-paths", "Copy selected paths", Some(K::new(M::COMMAND, Key::C)), !self.selected.is_empty()),
            c("archive-up", "Parent folder", Some(K::new(M::NONE, Key::Backspace)), self.is_open()),
            c("archive-goto", "Go to offset (hex preview)", Some(K::new(M::COMMAND, Key::G)), self.is_open()),
        ]
    }

    /// Runs a palette entry of [`ArchiveView::commands`].
    pub fn run_command(&mut self, id: &str, cx: &mut StudioCx<'_>) -> Vec<TabRequest> {
        let mut requests = Vec::new();
        let act = match id {
            "archive-search" => {
                self.want_search_focus = true;
                None
            }
            "archive-extract" => Some(Act::Extract { all: false, flat: false }),
            "archive-extract-all" => Some(Act::Extract { all: true, flat: false }),
            "archive-add" => Some(Act::AddFiles),
            "archive-remove" => Some(Act::QueueRemove),
            "archive-apply" => Some(Act::ApplyPending),
            "archive-save-new" => Some(Act::SaveNew),
            "archive-make-patch" => Some(Act::MakePatch),
            "archive-discard" => Some(Act::Discard),
            "archive-check" => Some(Act::Check(false)),
            "archive-compare" => Some(Act::Compare),
            "archive-compact" => Some(Act::Compact),
            "archive-reload" => Some(Act::Reload),
            "archive-copy-paths" => Some(Act::CopyPaths),
            "archive-up" => Some(Act::Up),
            "archive-goto" => {
                self.want_goto_focus = true;
                None
            }
            _ => None,
        };
        if let Some(a) = act {
            self.run_acts(vec![a], cx, &mut requests);
        }
        requests
    }

    /// Draws the tab. Returns the actions the Studio frame handles that need the shell (dialogs and tab switches).
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>) -> Vec<TabRequest> {
        let mut requests = Vec::new();
        self.poll(cx);
        self.refresh_view();
        let pal = cx.shell.pal();
        if self.loaded.is_none() {
            if self.opening.is_some() {
                ui.label(RichText::new("Opening...").color(pal.weak));
            } else if let Some(e) = &self.open_error {
                ui.label(RichText::new(format!("Cannot open this archive: {e}")).color(pal.error));
            } else {
                cx.shell.empty_state(ui, "Choose a client folder in the toolbar, or drop a client folder, data.sah or patch here. Nothing is written until you apply changes.");
            }
            self.draw_dialogs(ui, cx, &mut requests);
            return requests;
        }
        let mut acts: Vec<Act> = Vec::new();
        let has_saf = self.loaded.as_ref().is_some_and(|l| l.archive.is_some());
        let can_edit = self.client_dir().is_some() && has_saf;
        // header
        {
            let l = self.loaded.as_ref().expect("checked");
            let saf = match (l.saf_len, l.dead) {
                (Some(n), Some(d)) => format!("data.saf {}, {} unused", human_size(n), human_size(d)),
                _ => "data.saf not found".to_string(),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(l.sah.parent().map(|p| p.display().to_string()).unwrap_or_default()).strong());
                ui.label(format!("data.sah v{}, {} files ({} shadowed)  ·  {saf}", l.sah_version, l.visible, l.rows.len() - l.visible));
                if !has_saf {
                    ui.label(RichText::new("browsing the index only: preview, extract and changes are off").color(pal.warning));
                }
                if ui.add_enabled(has_saf, egui::Button::new("Check")).clicked() {
                    acts.push(Act::Check(false));
                }
                if ui.add_enabled(has_saf, egui::Button::new("Compare...")).clicked() {
                    acts.push(Act::Compare);
                }
                if ui.add_enabled(can_edit && !self.is_applying(), egui::Button::new("Compact...")).clicked() {
                    acts.push(Act::Compact);
                }
            });
            for n in &self.notes {
                ui.label(RichText::new(n).color(if n.starts_with("warning") { pal.warning } else { pal.weak }));
            }
        }
        // search line
        ui.horizontal_wrapped(|ui| {
            ui.label("Search");
            let r = ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(180.0).hint_text("text or *.dds").id(egui::Id::unique("studio-search")));
            if self.want_search_focus {
                r.request_focus();
                self.want_search_focus = false;
            }
            if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                acts.push(Act::ClearSearch);
            }
            for (i, (name, _)) in TYPE_CHIPS.iter().enumerate() {
                ui.toggle_value(&mut self.types[i], *name);
            }
            ui.add(egui::TextEdit::singleline(&mut self.min_size).desired_width(52.0).hint_text("min"));
            ui.add(egui::TextEdit::singleline(&mut self.max_size).desired_width(52.0).hint_text("max"));
            ui.checkbox(&mut self.show_shadowed, "shadowed");
            cx.settings.show_shadowed = self.show_shadowed;
            ui.label(RichText::new(if self.search.is_empty() { format!("/{}", self.folder) } else { "search".to_string() }).color(pal.weak));
            if ui.add_enabled(can_edit, egui::Button::new("Add...")).on_hover_text("Insert").clicked() {
                acts.push(Act::AddFiles);
            }
            let any = !self.selected.is_empty() || self.selected_folder.is_some();
            if ui.add_enabled(has_saf && any, egui::Button::new("Extract...")).on_hover_text("Ctrl+E").clicked() {
                acts.push(Act::Extract { all: false, flat: false });
            }
            if ui.add_enabled(has_saf, egui::Button::new("Extract all shown...")).clicked() {
                acts.push(Act::Extract { all: true, flat: false });
            }
        });
        ui.separator();
        // pending strip first (bottom), then the side panels, then the table
        if !self.pending.is_empty() {
            egui::Panel::bottom("studio-pending").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Pending changes ({}):", self.pending.len())).strong());
                    for (i, p) in self.pending.iter().enumerate() {
                        let mark = match &p.kind {
                            PendingKind::Put { replaces: true, .. } => "~",
                            PendingKind::Put { .. } => "+",
                            PendingKind::Remove => "-",
                        };
                        if ui.small_button(format!("{mark} {}  x", p.path)).on_hover_text("Remove from the queue").clicked() {
                            acts.push(Act::Unqueue(i));
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let busy = self.is_applying();
                    if ui.add_enabled(can_edit && !busy, egui::Button::new("Apply to client...")).clicked() {
                        acts.push(Act::ApplyPending);
                    }
                    if ui.add_enabled(has_saf, egui::Button::new("Save as new archive...")).clicked() {
                        acts.push(Act::SaveNew);
                    }
                    if ui.button("Make patch...").clicked() {
                        acts.push(Act::MakePatch);
                    }
                    if ui.button("Discard").clicked() {
                        acts.push(Act::Discard);
                    }
                });
            });
        }
        let tree_w = cx.settings.tree_width.clamp(120.0, 600.0);
        let prev_w = cx.settings.preview_width.clamp(200.0, 800.0);
        egui::Panel::left("studio-tree").resizable(true).default_size(tree_w).show(ui, |ui| {
            cx.settings.tree_width = ui.available_width();
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                let l = self.loaded.as_ref().expect("checked");
                draw_tree(ui, l, 0, &self.folder, &mut acts);
            });
        });
        egui::Panel::right("studio-preview").resizable(true).default_size(prev_w).show(ui, |ui| {
            cx.settings.preview_width = ui.available_width();
            self.draw_preview(ui, cx, &mut acts);
        });
        egui::CentralPanel::default().show(ui, |ui| self.draw_table(ui, cx, &mut acts));
        self.draw_dialogs(ui, cx, &mut requests);
        self.handle_keys(ui, &mut acts);
        self.run_acts(acts, cx, &mut requests);
        requests
    }

    fn handle_keys(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        use egui::Key;
        let ctx = ui.ctx().clone();
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |k: Key, cmd: bool, shift: bool, alt: bool| ctx.input(|i| i.key_pressed(k) && i.modifiers.command == cmd && i.modifiers.shift == shift && i.modifiers.alt == alt);
        if pressed(Key::Enter, false, false, false) {
            match (self.selected_folder, self.selected.len()) {
                (Some(n), _) => {
                    if let Some(l) = &self.loaded {
                        acts.push(Act::Folder(l.tree.nodes[n].path.clone()));
                    }
                }
                // on a file: focus the preview (the hex view's offset field, when there is one)
                (None, 1) => self.want_goto_focus = true,
                _ => {}
            }
        }
        if pressed(Key::Backspace, false, false, false) || pressed(Key::ArrowUp, false, false, true) {
            acts.push(Act::Up);
        }
        for (k, m) in [(Key::ArrowUp, MoveTo::Up), (Key::ArrowDown, MoveTo::Down), (Key::PageUp, MoveTo::PageUp), (Key::PageDown, MoveTo::PageDown), (Key::Home, MoveTo::Home), (Key::End, MoveTo::End)] {
            if pressed(k, false, false, false) && !self.view.is_empty() {
                acts.push(Act::Move(m));
            }
        }
        if pressed(Key::Delete, false, false, false) {
            acts.push(Act::QueueRemove);
        }
        if pressed(Key::Insert, false, false, false) {
            acts.push(Act::AddFiles);
        }
        if pressed(Key::E, true, false, false) {
            acts.push(Act::Extract { all: false, flat: false });
        }
        if pressed(Key::E, true, true, false) {
            acts.push(Act::Extract { all: true, flat: false });
        }
        if pressed(Key::Enter, true, false, false) {
            acts.push(Act::ApplyPending);
        }
        if pressed(Key::C, true, false, false) {
            acts.push(Act::CopyPaths);
        }
        if pressed(Key::A, true, false, false) {
            acts.push(Act::SelectAll);
        }
        if pressed(Key::F5, false, false, false) {
            acts.push(Act::Reload);
        }
        if pressed(Key::F, true, false, false) {
            self.want_search_focus = true;
        }
        if pressed(Key::G, true, false, false) {
            self.want_goto_focus = true;
        }
    }

    fn draw_table(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>, acts: &mut Vec<Act>) {
        let pal = cx.shell.pal();
        let searching = !self.search.trim().is_empty();
        let cols = [260.0, 90.0, 100.0, 70.0];
        ui.horizontal(|ui| {
            for (i, (name, key)) in [("Name", SortKey::Name), ("Size", SortKey::Size), ("Packed", SortKey::Packed), ("Type", SortKey::Type)].iter().enumerate() {
                let arrow = if self.sort_key == *key { if self.sort_desc { " v" } else { " ^" } } else { "" };
                if ui.add_sized([cols[i], 18.0], egui::Button::new(RichText::new(format!("{name}{arrow}")).strong()).frame(false)).clicked() {
                    acts.push(Act::Sort(*key));
                }
            }
            if searching {
                ui.label(RichText::new("Path").strong());
            }
        });
        let count = self.view.len();
        let l = self.loaded.as_ref().expect("checked");
        let mut clicks: Vec<(usize, bool, bool)> = Vec::new();
        let mut open: Option<String> = None;
        let mut menu: Option<(usize, MenuChoice)> = None;
        egui::ScrollArea::both().auto_shrink([false, false]).show_rows(ui, 20.0, count, |ui, range| {
            for vi in range {
                let v = self.view[vi];
                let selected = match v {
                    ViewRow::File(i) => self.selected.contains(&i),
                    ViewRow::Folder(n) => self.selected_folder == Some(n),
                };
                ui.horizontal(|ui| {
                    let (label, size, packed, ty, color, path) = match v {
                        ViewRow::Folder(n) => {
                            let nd = &l.tree.nodes[n];
                            (format!("{}/", nd.name), human_size(nd.bytes), format!("{} files", nd.files), "folder".to_string(), pal.text, nd.path.clone())
                        }
                        ViewRow::File(i) => {
                            let r = &l.rows[i];
                            let mut label = r.name().to_string();
                            if r.shadowed {
                                label.push_str("  [shadowed]");
                            }
                            let key = lower_key(&r.path);
                            if let Some(p) = self.pending.iter().find(|p| lower_key(&p.path) == key) {
                                label = match p.kind {
                                    PendingKind::Remove => format!("~~{label}~~"),
                                    PendingKind::Put { .. } => format!("~ {label}"),
                                };
                            }
                            (label, human_size(r.entry.length as u64), r.entry.version.gt(&0).then(|| utc_iso_unix(r.entry.version as i64)[..10].to_string()).unwrap_or_else(|| "-".into()), r.type_name(), self.color_for(r, &pal), r.path.clone())
                        }
                    };
                    let resp = ui.add_sized([cols[0], 20.0], egui::Button::selectable(selected, RichText::new(label).color(color)).truncate());
                    if resp.clicked() {
                        let m = ui.input(|i| i.modifiers);
                        clicks.push((vi, m.command, m.shift));
                    }
                    if resp.double_clicked() {
                        if let ViewRow::Folder(_) = v {
                            open = Some(path.clone());
                        }
                    }
                    resp.context_menu(|ui| {
                        if matches!(v, ViewRow::File(_)) {
                            for (label, choice) in [
                                ("Extract...", MenuChoice::Extract),
                                ("Extract with folders...", MenuChoice::ExtractFolders),
                                ("Copy path", MenuChoice::CopyPath),
                                ("Copy as JSON", MenuChoice::CopyJson),
                                ("Replace with file...", MenuChoice::Replace),
                                ("Remove", MenuChoice::Remove),
                                ("Decrypt to file...", MenuChoice::Decrypt),
                            ] {
                                if ui.button(label).clicked() {
                                    menu = Some((vi, choice));
                                    ui.close();
                                }
                            }
                        } else if ui.button("Open").clicked() {
                            open = Some(path.clone());
                            ui.close();
                        }
                    });
                    ui.add_sized([cols[1], 20.0], egui::Label::new(size));
                    ui.add_sized([cols[2], 20.0], egui::Label::new(packed));
                    ui.add_sized([cols[3], 20.0], egui::Label::new(ty));
                    if searching {
                        if let ViewRow::File(i) = v {
                            ui.label(RichText::new(l.rows[i].folder()).color(pal.weak));
                        }
                    }
                });
            }
        });
        if let Some(f) = open {
            acts.push(Act::Folder(f));
        }
        for (view, ctrl, shift) in clicks {
            acts.push(Act::Click { view, ctrl, shift });
        }
        if let Some((vi, choice)) = menu {
            // a right click acts on the clicked row (selecting it first when it is not selected)
            if let ViewRow::File(i) = self.view[vi] {
                if !self.selected.contains(&i) {
                    acts.push(Act::Click { view: vi, ctrl: false, shift: false });
                }
            }
            match choice {
                MenuChoice::Extract => acts.push(Act::Extract { all: false, flat: true }),
                MenuChoice::ExtractFolders => acts.push(Act::Extract { all: false, flat: false }),
                MenuChoice::CopyPath | MenuChoice::CopyJson => acts.push(Act::CopyPaths),
                MenuChoice::Replace => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::ReplaceWith(i));
                    }
                }
                MenuChoice::Remove => acts.push(Act::QueueRemove),
                MenuChoice::Decrypt => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::DecryptTo(i));
                    }
                }
            }
        }
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>, acts: &mut Vec<Act>) {
        let pal = cx.shell.pal();
        let Some(row) = self.preview.row else {
            ui.label(RichText::new("Select a file to preview it.").color(pal.weak));
            return;
        };
        let Some(l) = &self.loaded else { return };
        let Some(r) = l.rows.get(row) else { return };
        ui.label(RichText::new(r.name()).strong());
        ui.label(RichText::new(format!("{}  ·  {}", r.path, human_size(r.entry.length as u64))).color(pal.weak));
        if self.preview.handle.is_some() {
            ui.label("Loading...");
            return;
        }
        if let Some(e) = &self.preview.error {
            ui.label(RichText::new(e).color(pal.error));
            return;
        }
        let ctx = ui.ctx().clone();
        let _ = &ctx;
        match &self.preview.data {
            None => {}
            Some(PreviewData::Empty) => {
                ui.label("empty file");
            }
            Some(PreviewData::Image { width, height, header, downscaled, .. }) => {
                ui.label(header);
                if *downscaled {
                    ui.label(RichText::new(format!("shown at {width} x {height} (downscaled)")).color(pal.weak));
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.preview.fit, true, "Fit");
                    ui.selectable_value(&mut self.preview.fit, false, "1:1");
                    ui.checkbox(&mut self.preview.alpha, "alpha");
                });
                if let Some(name) = &self.preview.texture_name {
                    if let Some((_, tex)) = self.textures.iter().find(|(n, _)| n == name) {
                        let avail = ui.available_size();
                        let size = tex.size_vec2();
                        let scale = if self.preview.fit { (avail.x / size.x).min(avail.y.max(100.0) / size.y).min(8.0) } else { 1.0 };
                        let dim = size * scale;
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                            let (rect, _) = ui.allocate_exact_size(dim, egui::Sense::hover());
                            let p = ui.painter();
                            let cell: f32 = 8.0;
                            let mut y = 0.0;
                            let mut odd = false;
                            while y < dim.y {
                                let mut x = 0.0;
                                let mut on = odd;
                                while x < dim.x {
                                    let c = if on { Color32::from_gray(90) } else { Color32::from_gray(60) };
                                    let rr = egui::Rect::from_min_size(rect.min + egui::vec2(x, y), egui::vec2(cell.min(dim.x - x), cell.min(dim.y - y)));
                                    p.rect_filled(rr, 0.0, c);
                                    on = !on;
                                    x += cell;
                                }
                                odd = !odd;
                                y += cell;
                            }
                            let tint = if self.preview.alpha { Color32::WHITE } else { Color32::from_rgb(255, 255, 255) };
                            p.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
                        });
                    }
                }
            }
            Some(PreviewData::Text { bytes, total, truncated, guess }) => {
                ui.horizontal(|ui| {
                    let mut chosen = self.preview.encoding;
                    egui::ComboBox::from_id_salt("studio-enc").selected_text(preview::encoding_name(chosen)).show_ui(ui, |ui| {
                        for e in [Encoding::Cp949, Encoding::Cp1252, Encoding::Cp1251, Encoding::Cp936, Encoding::Utf8, Encoding::Utf16Le] {
                            ui.selectable_value(&mut chosen, e, preview::encoding_name(e));
                        }
                    });
                    if chosen != self.preview.encoding {
                        self.preview.encoding = chosen;
                        cx.settings.text_encoding = preview::encoding_name(chosen).to_string();
                        self.preview.decoded = None;
                    }
                    ui.label(RichText::new(format!("guessed {}", preview::encoding_name(*guess))).color(pal.weak));
                });
                if *truncated {
                    ui.horizontal(|ui| {
                        ui.label(format!("first {} of {}", human_size(bytes.len() as u64), human_size(*total)));
                        if ui.button("Show all").clicked() {
                            self.preview.text_all = true;
                            acts.push(Act::Open("reload-preview".into()));
                        }
                    });
                }
                if self.preview.decoded.as_ref().map(|(e, _)| *e) != Some(self.preview.encoding) {
                    self.preview.decoded = Some((self.preview.encoding, preview::decode_text(bytes, self.preview.encoding)));
                }
                let text = self.preview.decoded.as_ref().map(|(_, t)| t.as_str()).unwrap_or("");
                if text.chars().any(|c| (c as u32) > 0x2E80) {
                    cx.shell.need_cjk_font();
                }
                egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text).monospace()).selectable(true));
                });
            }
            Some(PreviewData::Sdata { summary, plain, raw }) => {
                if summary.encrypted {
                    ui.label(format!(
                        "SData, SEED, checksum {}",
                        match summary.crc_ok {
                            Some(true) => "OK",
                            _ => "FAILED",
                        }
                    ));
                    ui.label(format!("plaintext {} bytes", num(summary.plain_size)));
                    ui.label(format!(
                        "header padding {}, tail {}",
                        if summary.padding_zero == Some(true) { "zero" } else { "not zero" },
                        if summary.tail_zero == Some(true) { "zero" } else { "not zero" }
                    ));
                } else {
                    ui.label("SData, not encrypted");
                }
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.preview.plain_view, "Show plaintext");
                    if summary.encrypted && ui.button("Decrypt to file...").clicked() {
                        acts.push(Act::DecryptTo(row));
                    }
                });
                let bytes = if self.preview.plain_view { plain } else { raw };
                hex_view(ui, bytes, 0);
            }
            Some(PreviewData::Hex { bytes, base, total }) => {
                if *total > bytes.len() as u64 {
                    ui.label(RichText::new(format!("showing {} from offset {}", human_size(bytes.len() as u64), base)).color(pal.weak));
                }
                ui.horizontal(|ui| {
                    ui.label("Go to offset");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.preview.goto).desired_width(90.0).hint_text("0x10"));
                    if self.want_goto_focus {
                        r.request_focus();
                        self.want_goto_focus = false;
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        acts.push(Act::Open("goto".into()));
                    }
                });
                hex_view(ui, bytes, *base);
            }
            Some(PreviewData::Failed { message, bytes, total }) => {
                ui.label(RichText::new(message).color(pal.warning));
                ui.label(RichText::new(format!("showing the first {} of {} as hex", human_size(bytes.len() as u64), human_size(*total))).color(pal.weak));
                hex_view(ui, bytes, 0);
            }
        }
    }

    fn draw_dialogs(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>, requests: &mut Vec<TabRequest>) {
        let ctx = ui.ctx().clone();
        // Add to archive
        let mut action: Option<&'static str> = None;
        if let Some(mut d) = self.add_dialog.take() {
            let mut changed = false;
            egui::Window::new("Add to archive").collapsible(false).default_width(560.0).show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Target folder");
                    changed |= ui.add(egui::TextEdit::singleline(&mut d.target).desired_width(300.0)).changed();
                    changed |= ui.checkbox(&mut d.keep_folder_names, "Keep dropped folder names").changed();
                });
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(cx.shell.pal().error));
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for r in &d.rows {
                        let what = match r.replaces {
                            Some((s, v)) => format!("replaces {}{}", human_size(s as u64), if v > 0 { format!(", {}", &utc_iso_unix(v as i64)[..10]) } else { String::new() }),
                            None => "new".to_string(),
                        };
                        ui.label(format!("{}  ({}, {what})", r.path, human_size(r.size)));
                    }
                    if d.rows.is_empty() {
                        ui.label("Nothing to add.");
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.rows.is_empty(), egui::Button::new("Queue changes")).clicked() {
                        action = Some("queue");
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some("cancel");
                    }
                });
            });
            if changed {
                let mut d2 = d;
                self.recompute_add(&mut d2);
                d = d2;
            }
            match action {
                Some("queue") => {
                    self.add_dialog = Some(d);
                    self.queue_add_dialog();
                }
                Some(_) => {}
                None => self.add_dialog = Some(d),
            }
        }
        // Check
        if let Some(c) = &self.check {
            let mut open = true;
            let mut deep = false;
            let mut select: Option<String> = None;
            egui::Window::new("Check").open(&mut open).default_size([560.0, 380.0]).show(&ctx, |ui| match &c.report {
                None => {
                    ui.label("Checking...");
                }
                Some(r) => {
                    ui.label(format!("{} problems, {} warnings", r.problems(), r.warnings()));
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for f in &r.findings {
                            let col = match f.severity {
                                Severity::Problem => cx.shell.pal().error,
                                Severity::Warning => cx.shell.pal().warning,
                                Severity::Info => cx.shell.pal().weak,
                            };
                            match &f.path {
                                Some(p) => {
                                    if ui.link(RichText::new(format!("{}: {p}: {}", f.code, f.message)).color(col)).clicked() {
                                        select = Some(p.clone());
                                    }
                                }
                                None => {
                                    ui.label(RichText::new(format!("{}: {}", f.code, f.message)).color(col));
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Deep check").clicked() {
                            deep = true;
                        }
                        if ui.button("Copy report").clicked() {
                            let text = r.findings.iter().map(|f| format!("{:?} {} {} {}", f.severity, f.code, f.path.clone().unwrap_or_default(), f.message)).collect::<Vec<_>>().join("\n");
                            ctx.copy_text(text);
                        }
                    });
                }
            });
            if !open {
                self.check = None;
            }
            if deep {
                self.run_check(true, cx);
            }
            if let Some(p) = select {
                self.select_path(&p, cx);
            }
        }
        // Compare
        if self.compare.is_some() {
            let mut open = true;
            let mut make_patch = false;
            let mut select: Option<String> = None;
            let pal = cx.shell.pal();
            let c = self.compare.as_mut().expect("checked");
            egui::Window::new("Compare").open(&mut open).default_size([560.0, 420.0]).show(&ctx, |ui| {
                ui.label(format!("{} -> this archive", c.other));
                if c.handle.is_some() {
                    ui.label("Comparing...");
                    return;
                }
                let s = c.summary;
                ui.label(format!("{} added, {} removed, {} different, {} same", s.added, s.removed, s.different, s.same));
                ui.horizontal(|ui| {
                    for (i, name) in ["added", "removed", "different", "unknown"].iter().enumerate() {
                        ui.checkbox(&mut c.show[i], *name);
                    }
                });
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for e in &c.entries {
                        let (idx, col, mark) = match e.change {
                            Change::Added => (0, pal.ok, "+"),
                            Change::Removed => (1, pal.error, "-"),
                            Change::Different => (2, pal.warning, "~"),
                            Change::Unknown => (3, pal.weak, "?"),
                            Change::Same => continue,
                        };
                        if c.show[idx] && ui.link(RichText::new(format!("{mark} {}", e.path)).color(col)).clicked() {
                            select = Some(e.path.clone());
                        }
                    }
                });
                if ui.button("Make patch from differences").clicked() {
                    make_patch = true;
                }
            });
            if make_patch {
                requests.push(TabRequest::PatchFromDiff(PathBuf::from(&c.other)));
            }
            if !open {
                self.compare = None;
            }
            if let Some(p) = select {
                self.select_path(&p, cx);
            }
        }
        // F5 with pending changes
        if self.reload_asked {
            let c = Confirm { title: "Reload the archive".into(), text: format!("Reload the archive and discard {} pending changes?", self.pending.len()), buttons: vec!["Reload".into(), "Keep".into()], danger: Some(0) };
            match cx.shell.confirm("studio-reload", &c) {
                None => {}
                Some(0) => {
                    self.reload_asked = false;
                    self.pending.clear();
                    self.history = History::new(500);
                    self.reopen(cx);
                }
                Some(_) => self.reload_asked = false,
            }
        }
        // the discard question
        if self.discard_asked {
            let c = Confirm { title: "Discard pending changes".into(), text: format!("Discard {} pending changes?", self.pending.len()), buttons: vec!["Discard".into(), "Keep".into()], danger: Some(0) };
            match cx.shell.confirm("studio-discard", &c) {
                None => {}
                Some(0) => {
                    self.discard_pending();
                    self.discard_asked = false;
                }
                Some(_) => self.discard_asked = false,
            }
        }
    }

    fn run_acts(&mut self, acts: Vec<Act>, cx: &mut StudioCx<'_>, requests: &mut Vec<TabRequest>) {
        for a in acts {
            match a {
                Act::Folder(p) => self.enter_folder(&p),
                Act::Up => self.parent_folder(),
                Act::TreeSelect(p) => self.enter_folder(&p),
                Act::ClearSearch => self.search.clear(),
                Act::Click { view, ctrl, shift } => {
                    let Some(v) = self.view.get(view).copied() else { continue };
                    self.cursor = Some(view);
                    match v {
                        ViewRow::Folder(n) => {
                            self.selected.clear();
                            self.selected_folder = Some(n);
                        }
                        ViewRow::File(i) => {
                            self.selected_folder = None;
                            if shift {
                                if let Some(a) = self.anchor.and_then(|a| self.view.iter().position(|v| *v == ViewRow::File(a))) {
                                    let (lo, hi) = (a.min(view), a.max(view));
                                    self.selected.clear();
                                    for v in &self.view[lo..=hi] {
                                        if let ViewRow::File(j) = v {
                                            self.selected.insert(*j);
                                        }
                                    }
                                }
                            } else if ctrl {
                                if !self.selected.remove(&i) {
                                    self.selected.insert(i);
                                }
                                self.anchor = Some(i);
                            } else {
                                self.selected.clear();
                                self.selected.insert(i);
                                self.anchor = Some(i);
                            }
                            if self.selected.len() == 1 {
                                self.start_preview(i, cx);
                            }
                        }
                    }
                }
                Act::Open(what) => {
                    if what == "reload-preview" {
                        if let Some(r) = self.preview.row {
                            self.start_preview(r, cx);
                        }
                    }
                    if what == "goto" {
                        let t = self.preview.goto.trim().to_string();
                        let v = t.strip_prefix("0x").map(|h| u64::from_str_radix(h, 16).ok()).unwrap_or_else(|| t.parse().ok());
                        if let (Some(off), Some(row)) = (v, self.preview.row) {
                            if let (Some(l), true) = (&self.loaded, true) {
                                if let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) {
                                    let (entry, path) = (r.entry.clone(), r.path.clone());
                                    let h = cx.shell.tasks().spawn(TaskSpec { title: "Preview".into(), cancellable: true, exclusive: None }, move |job| preview::make_preview(&a, &entry, &path, false, off, job));
                                    self.preview.handle = Some(h);
                                    self.preview.data = None;
                                }
                            }
                        }
                    }
                }
                Act::Sort(k) => {
                    if self.sort_key == k {
                        self.sort_desc = !self.sort_desc;
                    } else {
                        self.sort_key = k;
                        self.sort_desc = false;
                    }
                }
                Act::Extract { all, flat } => {
                    if let Some(dir) = cx.shell.pick_folder("extract") {
                        self.begin_extract(dir, all, false, flat, cx);
                    }
                }
                Act::AddFiles => {
                    let files = cx.shell.pick_files("add", &[("All files", &["*"])]);
                    if !files.is_empty() {
                        self.begin_add(files);
                    }
                }
                Act::QueueRemove => {
                    let Some(l) = &self.loaded else { continue };
                    let paths: Vec<String> = self.selected.iter().map(|&i| l.rows[i].path.clone()).collect();
                    if !paths.is_empty() && self.client_dir().is_some() {
                        self.queue_removes(paths);
                    }
                }
                Act::CopyPaths => {
                    let Some(l) = &self.loaded else { continue };
                    let text = self.selected.iter().map(|&i| l.rows[i].path.clone()).collect::<Vec<_>>().join("\n");
                    cx.shell.ctx().copy_text(text);
                    cx.shell.status("Copied");
                }
                Act::ReplaceWith(i) => {
                    if let Some(f) = cx.shell.pick_file("replace", &[("All files", &["*"])]) {
                        if let Some(l) = &self.loaded {
                            let path = l.rows[i].path.clone();
                            let v = mtime(&f);
                            self.queue_puts(vec![(path, f, v)]);
                        }
                    }
                }
                Act::DecryptTo(i) => {
                    let Some(l) = &self.loaded else { continue };
                    let (Some(a), r) = (l.archive.clone(), &l.rows[i]) else { continue };
                    if let Some(out) = cx.shell.save_file("decrypt", r.name(), &[("All files", &["*"])]) {
                        let (entry, path) = (r.entry.clone(), r.path.clone());
                        cx.shell.tasks().spawn(TaskSpec { title: format!("Decrypting {}", r.name()), cancellable: true, exclusive: None }, move |_job| {
                            let data = a.read_entry(&entry)?;
                            let plain = shaiya_seed::unwrap(&data, &path)?.0;
                            shaiya_core::fs::write_atomic(&out, &plain)
                        });
                    }
                }
                Act::ApplyPending => self.request_apply(cx),
                Act::SaveNew => {
                    if let Some(p) = cx.shell.save_file("save-archive", "data.sah", &[("SAH index", &["sah"])]) {
                        self.save_as_new(p, cx);
                    }
                }
                Act::Discard => self.discard_asked = true,
                Act::Unqueue(i) => self.unqueue(i),
                Act::Check(d) => self.run_check(d, cx),
                Act::Compare => {
                    if let Some(p) = cx.shell.pick_file("compare", &[("SAH index", &["sah"])]) {
                        self.run_compare(p, cx);
                    }
                }
                Act::Compact => self.request_compact(),
                Act::Reload => {
                    if self.pending.is_empty() {
                        self.reopen(cx);
                    } else {
                        self.reload_asked = true;
                    }
                }
                Act::SelectAll => {
                    self.selected_folder = None;
                    let rows: Vec<usize> = self.view.iter().filter_map(|v| if let ViewRow::File(i) = v { Some(*i) } else { None }).collect();
                    self.selected = rows.into_iter().collect();
                }
                Act::Move(m) => {
                    let last = self.view.len().saturating_sub(1);
                    let cur = self.cursor.unwrap_or(0);
                    let next = match m {
                        MoveTo::Up => cur.saturating_sub(1),
                        MoveTo::Down => if self.cursor.is_none() { 0 } else { (cur + 1).min(last) },
                        MoveTo::PageUp => cur.saturating_sub(15),
                        MoveTo::PageDown => (cur + 15).min(last),
                        MoveTo::Home => 0,
                        MoveTo::End => last,
                    };
                    let shift = cx.shell.ctx().input(|i| i.modifiers.shift);
                    self.run_acts(vec![Act::Click { view: next, ctrl: false, shift }], cx, requests);
                }
                Act::MakePatch => requests.push(TabRequest::PatchFromPending),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum MenuChoice {
    Extract,
    ExtractFolders,
    CopyPath,
    CopyJson,
    Replace,
    Remove,
    Decrypt,
}

/// What the Archive tab asks the Studio frame to do on its behalf.
pub enum TabRequest {
    PatchFromPending,
    PatchFromDiff(PathBuf),
}

fn num(n: u64) -> String {
    shaiya_inspect::group(n)
}

fn hex_view(ui: &mut egui::Ui, bytes: &[u8], base: u64) {
    let rows = preview::hex_rows(bytes.len());
    egui::ScrollArea::both().auto_shrink([false, false]).id_salt("studio-hex").show_rows(ui, 14.0, rows, |ui, range| {
        for r in range {
            ui.add(egui::Label::new(RichText::new(preview::hex_row(bytes, base, r)).monospace().size(11.0)).selectable(true));
        }
    });
}

fn draw_tree(ui: &mut egui::Ui, l: &Loaded, node: usize, current: &str, acts: &mut Vec<Act>) {
    let n = &l.tree.nodes[node];
    let label = if node == 0 { format!("(root)  {}", human_size(n.bytes)) } else { format!("{}  {}", n.name, human_size(n.bytes)) };
    let selected = n.path.eq_ignore_ascii_case(current);
    if n.children.is_empty() {
        if ui.selectable_label(selected, label).clicked() {
            acts.push(Act::TreeSelect(n.path.clone()));
        }
        return;
    }
    let id = ui.make_persistent_id(("studio-tree", &n.path));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, node == 0)
        .show_header(ui, |ui| {
            if ui.selectable_label(selected, label).clicked() {
                acts.push(Act::TreeSelect(n.path.clone()));
            }
        })
        .body(|ui| {
            for &c in &n.children {
                draw_tree(ui, l, c, current, acts);
            }
        });
}
