//! The Archive tab: a client's (or any) archive as a folder tree, a searchable table and a preview, with queued edits (pending changes)
//! that are applied to the client folder only after a confirmation, through the same crash-safe procedure as the command line.

use crate::explain::{self, Rows};
use crate::anim_view::{self, AnimPlayer};
use crate::preview::{self, MeshChoice, PreviewData};
use crate::multi_lock::lock_hover;
use crate::ToolkitSettings;
use egui::{Color32, RichText};
use shaiya_archive::check::{check_archive, CheckOptions, CheckReport, Severity};
use shaiya_archive::client::{self, ClientFiles};
use shaiya_archive::diff::{diff, summarize, Change, DiffEntry, DiffOptions, DiffSummary, Side};
use shaiya_archive::edit::{self, EditPlan, EditReport, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::extract::{self, ExtractOptions, ExtractReport};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_archive::{compact, Archive, Entry, Index};
use shaiya_inspect::validate::{self, Finding};
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

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

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
    /// What the `.ANI` previews learned about the `.3DC` candidates of this archive (new whenever an archive is loaded).
    pub mesh_cache: preview::MeshInfoCache,
    /// The texture index of the model list files, built by the first preview that needs it (new whenever an archive is loaded).
    pub texture_index: preview::TextureIndexCache,
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
    Ok(Opened { loaded: Loaded { label, sah, client_dir, archive, mesh_cache: preview::MeshInfoCache::default(), texture_index: preview::TextureIndexCache::default(), rows, tree, by_folder, sah_version, visible, saf_len, dead }, notes })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Packed,
    Type,
}

impl SortKey {
    /// The name the settings keep.
    pub fn key(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Packed => "packed",
            SortKey::Type => "type",
        }
    }

    /// The key of a settings name; anything else is `Name`.
    pub fn from_key(k: &str) -> SortKey {
        [SortKey::Name, SortKey::Size, SortKey::Packed, SortKey::Type].into_iter().find(|s| s.key() == k).unwrap_or(SortKey::Name)
    }
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
    /// File checks for the rows (see `shaiya_inspect::validate`); filled by a background task, valid when `checked`.
    pub findings: Vec<Finding>,
    pub checked: bool,
    /// The "Add anyway / Cancel" question is open.
    pub asking: bool,
    check_gen: u64,
    checking: Option<(u64, TaskHandle<Vec<Finding>>)>,
}

impl AddDialog {
    /// The file-check task for the rows now shown has ended. Whatever its outcome, the dialog is "checked" and `findings` says what is
    /// known: a check that did not finish (cancelled, failed, crashed) holds the finding that makes "Queue changes" ask first.
    pub fn finish_check(&mut self, outcome: TaskOutcome<Vec<Finding>>) {
        self.findings = check_outcome(outcome);
        self.checked = true;
    }

    /// "Queue changes" pressed. True: queue now. False: nothing yet (the checks are running, or the "Add anyway" question is open
    /// because a file is double-encrypted or fails its checksum).
    pub fn press_queue(&mut self) -> bool {
        if self.rows.is_empty() || !self.checked {
            return false;
        }
        if validate::any_asks(&self.findings) {
            self.asking = true;
            return false;
        }
        true
    }
}

/// A put to queue: archive path, the file, its pack time.
type QueuedPut = (String, PathBuf, i32);

/// One file to check: archive path, the file on disk, and the entry already in the archive at that path.
type CheckItem = (String, PathBuf, Option<Entry>);

/// The file checks for files about to be queued; an SData or `.ani` file is compared with the archive's copy of the same path.
fn check_items(items: &[CheckItem], archive: Option<&Archive>) -> Vec<Finding> {
    let mut out = Vec::new();
    for (path, source, entry) in items {
        let copy = if validate::is_sdata_name(path) || validate::is_anim_name(path) { archive.zip(entry.as_ref()).and_then(|(a, e)| a.read_entry(e).ok()) } else { None };
        out.extend(validate::check_path(path, source, copy.as_deref()));
    }
    out
}

/// What a finished file-check task means: its findings, or, when it did not finish (cancelled, failed, crashed), the one finding that
/// makes the Toolkit ask. A check that did not run never lets files through as if they were clean.
pub(crate) fn check_outcome(outcome: TaskOutcome<Vec<Finding>>) -> Vec<Finding> {
    match outcome {
        TaskOutcome::Done(f) => f,
        TaskOutcome::Failed(e) => vec![Finding::incomplete(&e.to_string())],
        TaskOutcome::Cancelled => vec![Finding::incomplete("cancelled")],
        TaskOutcome::Panicked(m) => vec![Finding::incomplete(&format!("internal error (a bug): {m}"))],
    }
}

/// The text of the "Add anyway" question: the findings that ask, at most six, then what happens.
pub(crate) fn findings_question(findings: &[Finding], verb: &str) -> String {
    let mut s = String::new();
    let asking: Vec<&Finding> = findings.iter().filter(|f| f.asks()).collect();
    for f in asking.iter().take(6) {
        s.push_str(&format!("{}\n", f.line()));
    }
    if asking.len() > 6 {
        s.push_str(&format!("... and {} more\n", asking.len() - 6));
    }
    s.push_str(&format!("\nThese files may not work in the game. You can still {verb} them; nothing is blocked."));
    s
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

/// One side of Compare: a saved client or an archive file.
#[derive(Clone, Debug, PartialEq)]
pub enum CompareSide {
    Client { name: String, dir: PathBuf },
    File(PathBuf),
}

/// A short name for an archive path: its folder's name and the file name (`data.sah` alone would not say which one).
pub fn path_label(p: &Path) -> String {
    let file = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match p.parent().and_then(|d| d.file_name()) {
        Some(d) => format!("{}/{file}", d.to_string_lossy()),
        None => file,
    }
}

impl CompareSide {
    pub fn label(&self) -> String {
        match self {
            CompareSide::Client { name, .. } => name.clone(),
            CompareSide::File(p) => path_label(p),
        }
    }

    /// The `data.sah` this side reads.
    pub fn sah(&self) -> Result<PathBuf> {
        match self {
            CompareSide::Client { dir, .. } => Ok(ClientFiles::locate(dir)?.sah),
            CompareSide::File(p) => Ok(p.clone()),
        }
    }
}

/// What a finished (or running) comparison was made of.
#[derive(Clone, Debug, PartialEq)]
pub struct CompareRan {
    pub left: CompareSide,
    pub right: CompareSide,
    pub old_sah: PathBuf,
    pub new_sah: PathBuf,
}

/// Compare: OLD is the left side, NEW the right side; "added" is in NEW only, "removed" in OLD only.
pub struct ComparePanel {
    pub left: Option<CompareSide>,
    pub right: Option<CompareSide>,
    pub ran: Option<CompareRan>,
    pub entries: Vec<DiffEntry>,
    pub summary: DiffSummary,
    handle: Option<TaskHandle<(Vec<DiffEntry>, DiffSummary)>>,
    pub show: [bool; 4],
}

impl ComparePanel {
    /// "Swap sides": OLD becomes NEW and the other way round (the shown comparison is then no longer the current one).
    pub fn swap_sides(&mut self) {
        std::mem::swap(&mut self.left, &mut self.right);
    }

    /// The answer of the "Archive file..." picker for one side: a file picked is that side; no file (the picker was cancelled) leaves it as it was.
    pub fn choose_file(&mut self, is_right: bool, picked: Option<PathBuf>) {
        if let Some(p) = picked {
            let slot = if is_right { &mut self.right } else { &mut self.left };
            *slot = Some(CompareSide::File(p));
        }
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// "Make patch from differences": OLD = the left side, NEW = the right side, as compared (nothing when the comparison is not the current one).
    pub fn patch_request(&self) -> Option<TabRequest> {
        let r = self.ran.as_ref().filter(|_| self.is_current())?;
        Some(TabRequest::PatchFromDiff { old: r.old_sah.clone(), new: r.new_sah.clone(), old_label: r.left.label(), new_label: r.right.label() })
    }

    /// The comparison shown is the one of the two sides chosen now.
    pub fn is_current(&self) -> bool {
        self.handle.is_none() && self.ran.as_ref().is_some_and(|r| Some(&r.left) == self.left.as_ref() && Some(&r.right) == self.right.as_ref())
    }
}

/// Nothing differs: not one file is added, removed, different or unknown.
pub fn nothing_differs(s: &DiffSummary) -> bool {
    s.added + s.removed + s.different + s.unknown == 0
}

/// What "Make patch from differences" would put in the patch: the added, different and removed files.
pub fn patch_worthy(s: &DiffSummary) -> bool {
    s.added + s.removed + s.different > 0
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
    /// The meshes an `.ANI` preview is drawn on (back to the default when another row is selected).
    pub anim_mesh: MeshChoice,
    /// Whether models are drawn with their textures, and which of the textures a list names for a `.3DC` (0 = the default; another row resets it).
    pub textured: bool,
    pub texture_pick: usize,
    pub player: AnimPlayer,
    /// Drag, wheel, reset and "Save frame as PNG...".
    pub controls: crate::preview_controls::PreviewControls,
    /// The one texture of the `.ANI` frame on screen, with the frame it shows.
    anim_tex: Option<(usize, egui::TextureHandle)>,
}

/// The "Textures" check box of a model preview; true when it was changed (the preview is drawn again).
fn textures_switch(textured: &mut bool, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) -> bool {
    let changed = ui.checkbox(textured, "Textures").changed();
    if changed {
        cx.settings.preview_textured = *textured;
    }
    changed
}

pub enum OpenSource {
    Client { name: String, dir: PathBuf },
    File(PathBuf),
}

/// The queued changes of one client, kept aside while another client is active (memory only).
pub struct Queue {
    /// The client folder the queue was built on; it is only ever applied there.
    pub dir: PathBuf,
    pending: Vec<Pending>,
    history: History<QueueEdit>,
}

impl Queue {
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
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
    /// The client folder `pending` was built on (set by every queue edit); Apply refuses a queue that is not this client's.
    pending_owner: Option<PathBuf>,
    history: History<QueueEdit>,
    pub add_dialog: Option<AddDialog>,
    /// A replacement file whose checks ask first: the puts to queue and the findings.
    pub replace_ask: Option<(Vec<QueuedPut>, Vec<Finding>)>,
    /// "Replace with file..." checks running in the background: the put to queue and the task.
    replace_checks: Vec<(Vec<QueuedPut>, TaskHandle<Vec<Finding>>)>,
    pub check: Option<CheckPanel>,
    pub compare: Option<ComparePanel>,
    extract: Option<ExtractFlow>,
    json_exports: crate::export_json::JsonExports,
    gltf_exports: crate::export_interchange::InterchangeExports,
    folder_exports: crate::export_json::FolderExports,
    /// The "Compare with..." window of one `.ANI` (see `anim_compare`).
    anim_compare: Option<crate::anim_compare::AnimCompare>,
    /// The "Edit animation..." window of one `.ANI` (see `anim_edit`).
    anim_edit: Option<crate::anim_edit::AnimEdit>,
    /// The "Rig" filter inside `Character/<race>/ANI`: an index into `anim::RIGS`.
    pub rig: Option<usize>,
    apply: ApplyFlow,
    /// The Changes / Writes / Undo / Game rows of the open confirmation (made once, when it was asked for, from the plan the write uses).
    confirm_rows: Rows,
    discard_asked: bool,
    /// True while "Apply to several test clients" runs (set by the app every frame): the other write actions are off.
    multi_running: bool,
    reload_asked: bool,
    saved_new: Option<TaskHandle<EditReport>>,
    want_search_focus: bool,
    want_goto_focus: bool,
    /// Set by the SData preview's View table button: (the entry's path, the file as stored). The app opens the table window.
    pub table_request: Option<(String, Vec<u8>)>,
    pub status_line: String,
    /// Set when the last change finished (for tests and the Apply tab to reload its history).
    pub changed_counter: u64,
}

impl Default for ArchiveView {
    fn default() -> Self {
        Self::new()
    }
}

fn has_sah_file(dir: &Path) -> bool {
    dir.is_dir() && std::fs::read_dir(dir).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or(false)
}

fn lower_key(path: &str) -> String {
    path.to_lowercase()
}

/// The indexes (into `anim::RIGS`) of the rigs of a `Character/<race>/ANI` folder (any letter case); none for every other folder.
pub fn rig_choices(folder: &str) -> Vec<usize> {
    let segs: Vec<&str> = folder.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() != 3 || !segs[0].eq_ignore_ascii_case("character") || !segs[2].eq_ignore_ascii_case("ani") {
        return Vec::new();
    }
    shaiya_clientdata::anim::RIGS.iter().enumerate().filter(|(_, (_, race))| race.eq_ignore_ascii_case(segs[1])).map(|(i, _)| i).collect()
}

/// Whether the archive path names an `.ANI` (any letter case).
pub fn is_ani_path(path: &str) -> bool {
    path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).is_some_and(|(_, e)| e.eq_ignore_ascii_case("ani"))
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
                anim_mesh: MeshChoice::Default,
                textured: true,
                texture_pick: 0,
                player: AnimPlayer::default(),
                controls: Default::default(),
                anim_tex: None,
            },
            textures: Vec::new(),
            pending: Vec::new(),
            pending_owner: None,
            history: History::new(500),
            add_dialog: None,
            replace_ask: None,
            replace_checks: Vec::new(),
            check: None,
            compare: None,
            extract: None,
            json_exports: Default::default(),
            gltf_exports: Default::default(),
            folder_exports: Default::default(),
            anim_compare: None,
            anim_edit: None,
            rig: None,
            apply: ApplyFlow::Idle,
            confirm_rows: Vec::new(),
            discard_asked: false,
            multi_running: false,
            reload_asked: false,
            saved_new: None,
            want_search_focus: false,
            want_goto_focus: false,
            table_request: None,
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
        self.pending_owner = None;
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
        self.folder_exports.cancel();
        self.anim_compare = None;
        self.anim_edit = None;
    }

    /// Opens a saved client folder (changes are possible) or any archive file (read only), as a task.
    pub fn open(&mut self, source: OpenSource, cx: &mut ToolkitCx<'_>) {
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

    pub fn reopen(&mut self, cx: &mut ToolkitCx<'_>) {
        let src = match &self.source {
            Some(OpenSource::Client { name, dir }) => OpenSource::Client { name: name.clone(), dir: dir.clone() },
            Some(OpenSource::File(p)) => OpenSource::File(p.clone()),
            None => return,
        };
        let (folder, pending, history, owner) = (self.folder.clone(), std::mem::take(&mut self.pending), std::mem::replace(&mut self.history, History::new(500)), self.pending_owner.take());
        self.open(src, cx);
        self.folder = folder;
        self.pending = pending;
        self.history = history;
        self.pending_owner = owner;
    }

    /// Takes the queue of the open client aside (with its undo history) so that another source can be opened; `None` when nothing is queued.
    pub fn take_queue(&mut self) -> Option<Queue> {
        if self.pending.is_empty() {
            return None;
        }
        let dir = self.pending_owner.clone().or_else(|| self.client_dir().map(Path::to_path_buf))?;
        self.pending_owner = None;
        Some(Queue { dir, pending: std::mem::take(&mut self.pending), history: std::mem::replace(&mut self.history, History::new(500)) })
    }

    /// Puts a queue back after its client was opened again (call after `open`, which starts with an empty queue).
    pub fn put_queue(&mut self, q: Queue) {
        self.pending = q.pending;
        self.history = q.history;
        self.pending_owner = Some(q.dir);
    }

    /// Drops the open client's queue (its client was removed or renamed).
    pub fn drop_queue(&mut self) {
        self.pending.clear();
        self.pending_owner = None;
        self.history = History::new(500);
    }

    /// The client folder the queue was built on (`None` when nothing is queued).
    pub fn queue_dir(&self) -> Option<&Path> {
        if self.pending.is_empty() {
            None
        } else {
            self.pending_owner.as_deref()
        }
    }

    // ---------------------------------------------------------------------------------------- polling

    /// Collects finished tasks; call every frame.
    pub fn poll(&mut self, cx: &mut ToolkitCx<'_>) {
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
        if let Some(line) = self.preview.controls.poll_save(cx) {
            self.status_line = line;
        }
        self.poll_extract(cx);
        if let Some(line) = self.json_exports.poll(cx) {
            self.status_line = line;
        }
        if let Some(line) = self.gltf_exports.poll(cx) {
            self.status_line = line;
        }
        if let Some(line) = self.folder_exports.poll(cx) {
            self.status_line = line;
        }
        if let Some(c) = &mut self.anim_compare {
            c.poll(cx);
        }
        if let Some(c) = &mut self.anim_edit {
            c.poll(cx);
        }
        self.poll_check(cx);
        self.poll_compare(cx);
        self.poll_apply(cx);
    }

    fn poll_saved_new(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(h) = &mut self.saved_new else { return };
        match h.poll() {
            None => {}
            Some(TaskOutcome::Done(r)) => {
                self.saved_new = None;
                let n = r.added.len() + r.replaced.len() + r.removed.len();
                self.status_line = format!("Saved a new archive ({} included)", shaiya_inspect::counted((n) as u64, "change", "changes"));
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
        matches!(&self.source, Some(OpenSource::Client { dir: d, .. }) if crate::clients::same_folder(d, dir))
    }

    /// The app's handle on `data.saf` (for tasks that read it; they must not outlive a write).
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

    fn poll_preview(&mut self, cx: &mut ToolkitCx<'_>) {
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
                self.preview.texture_name = None;
                self.preview.anim_tex = None;
                if let PreviewData::Image { width, height, rgba, .. } | PreviewData::Map { width, height, rgba, .. } | PreviewData::Model { width, height, rgba, .. } = &d {
                    if !rgba.is_empty() {
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

    fn poll_extract(&mut self, cx: &mut ToolkitCx<'_>) {
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
                    text: format!("{} already exist in {}.", shaiya_inspect::counted((n) as u64, "file", "files"), req.out.display()),
                    buttons: vec!["Skip existing".into(), "Overwrite".into(), "Cancel".into()],
                    danger: Some(1),
                    ..Default::default()
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
                    let msg = format!("Extracted {} ({})", shaiya_inspect::counted((r.written) as u64, "file", "files"), human_size(r.bytes));
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

    fn poll_check(&mut self, cx: &mut ToolkitCx<'_>) {
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

    fn poll_compare(&mut self, cx: &mut ToolkitCx<'_>) {
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

    fn poll_apply(&mut self, cx: &mut ToolkitCx<'_>) {
        match std::mem::replace(&mut self.apply, ApplyFlow::Idle) {
            ApplyFlow::Idle => {}
            ApplyFlow::Confirm(ex) => {
                let (title, text, button) = self.confirm_text(&ex, cx);
                let c = Confirm { title, text, buttons: vec![button, "Cancel".into()], danger: Some(0), details: self.confirm_rows.clone() };
                match cx.shell.confirm("toolkit-exclusive", &c) {
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
                            self.status_line = format!("Applied {} (undo in Apply & history)", shaiya_inspect::counted((n) as u64, "change", "changes"));
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
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-in-use", &c) {
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

    fn confirm_text(&self, ex: &Excl, cx: &ToolkitCx<'_>) -> (String, String, String) {
        let name = self.source_label().unwrap_or_default();
        let dir = self.client_dir().map(|d| d.display().to_string()).unwrap_or_default();
        let _ = cx;
        match ex {
            Excl::Apply(_, n) => (
                "Apply changes".into(),
                format!("Apply {} to {name} ({dir})?", shaiya_inspect::counted(*n as u64, "change", "changes")),
                format!("Apply {}", shaiya_inspect::counted(*n as u64, "change", "changes")),
            ),
            Excl::Compact => ("Compact".into(), format!("Compact data.saf of {name}?"), "Compact".into()),
        }
    }

    /// The rows of the apply confirmation: `edit::preview` of the very plan that is applied, read against the client's current index.
    fn apply_rows_for(&self, plan: &EditPlan) -> Rows {
        let Some(dir) = self.client_dir() else { return Vec::new() };
        let Ok(files) = ClientFiles::locate(dir) else { return Vec::new() };
        match Archive::open_index(&files.sah).and_then(|(_, index)| edit::preview(&index, plan)) {
            Ok(report) => explain::apply_rows(&report, &files),
            Err(e) => vec![("Changes".into(), format!("cannot be counted: {e}")), ("Game".into(), explain::game_text(cfg!(windows)))],
        }
    }

    /// The rows of the compact confirmation.
    fn compact_rows_for(&self) -> Rows {
        let Some(dir) = self.client_dir() else { return Vec::new() };
        let Ok(files) = ClientFiles::locate(dir) else { return Vec::new() };
        let size = self.loaded.as_ref().and_then(|l| l.saf_len).unwrap_or(0);
        let live = self.loaded.as_ref().map(|l| l.saf_len.unwrap_or(0).saturating_sub(l.dead.unwrap_or(0))).unwrap_or(0);
        let shadowed = self.loaded.as_ref().map(|l| l.rows.iter().filter(|r| r.shadowed).count()).unwrap_or(0);
        let later = journal::list(dir, &dir.join(journal::DEFAULT_DIR)).map(|l| l.iter().filter(|(j, _)| j.state == journal::State::Applied).count()).unwrap_or(0);
        explain::compact_rows(size, live, shadowed, later, &files)
    }

    /// Cancels the previews, extracts, checks and compares that read the archive (a running "Export as JSON..." cannot be cancelled: the
    /// write that follows waits for it to finish) and hands out the app's handle on `data.saf`, so a write (apply, compact, swap, patch
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

    fn start_exclusive(&mut self, ex: Excl, cx: &mut ToolkitCx<'_>) {
        let Some(dir) = self.client_dir().map(Path::to_path_buf) else { return };
        let key = dir.display().to_string();
        let ex2 = match &ex {
            Excl::Apply(p, n) => Excl::Apply(p.clone(), *n),
            Excl::Compact => Excl::Compact,
        };
        let handle = match ex {
            Excl::Apply(plan, _) => {
                let root = dir.join(journal::DEFAULT_DIR);
                cx.shell.tasks().spawn(TaskSpec { title: format!("Applying {}", shaiya_inspect::counted((plan.puts.len() + plan.removes.len()) as u64, "change", "changes")), cancellable: true, exclusive: Some(key) }, move |job| {
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
            vec![shaiya_inspect::counted((self.pending.len()) as u64, "pending archive change", "pending archive changes")]
        }
    }

    /// True when the queue (if any) was built on the client folder that is open now. A queue is never applied anywhere else.
    fn queue_is_ours(&self) -> bool {
        self.pending.is_empty()
            || match (self.pending_owner.as_deref(), self.client_dir()) {
                (Some(o), Some(d)) => crate::clients::same_folder(o, d),
                (None, None) => true,
                _ => false,
            }
    }

    fn push_edit(&mut self, label: &str, edits: Vec<QueueEdit>) {
        // nothing loaded (a restored client whose open failed): the queue keeps the folder it was built on
        if let Some(d) = self.client_dir().map(Path::to_path_buf) {
            self.pending_owner = Some(d);
        }
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
    pub fn request_apply(&mut self, cx: &mut ToolkitCx<'_>) {
        if self.pending.is_empty() || self.client_dir().is_none() || !matches!(self.apply, ApplyFlow::Idle) || !self.write_actions_enabled() {
            return;
        }
        if !self.queue_is_ours() {
            cx.shell.status("These queued changes were built on another client; they are not applied here");
            return;
        }
        let plan = self.pending_plan("archive edit");
        let n = self.pending.len();
        let _ = cx;
        self.confirm_rows = self.apply_rows_for(&plan);
        self.apply = ApplyFlow::Confirm(Excl::Apply(plan, n));
    }

    pub fn request_compact(&mut self) {
        if self.client_dir().is_none() || !matches!(self.apply, ApplyFlow::Idle) || !self.write_actions_enabled() {
            return;
        }
        self.confirm_rows = self.compact_rows_for();
        self.apply = ApplyFlow::Confirm(Excl::Compact);
    }

    pub fn is_applying(&self) -> bool {
        !matches!(self.apply, ApplyFlow::Idle)
    }

    /// The app tells the tab every frame whether "Apply to several test clients" runs.
    pub fn set_multi_running(&mut self, on: bool) {
        self.multi_running = on;
    }

    /// False while a multi-client apply runs: Apply to client, Discard, Compact and Save as new archive (buttons, palette entries and
    /// keys) are off then. Queueing, Extract and the other tabs are not part of this.
    pub fn write_actions_enabled(&self) -> bool {
        !self.multi_running
    }

    /// "Save as new archive…": `write_new` of the loaded archive plus the queue.
    pub fn save_as_new(&mut self, out_sah: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(a) = self.loaded.as_ref().and_then(|l| l.archive.clone()) else { return };
        let plan = self.pending_plan("save as new archive");
        let title = format!("Writing {}", out_sah.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let h = cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: None }, move |job| edit::write_new(&a, &plan, &out_sah, Existing::Refuse, job));
        // the result is reported through the Activity list and the status line when it finishes
        self.saved_new = Some(h);
    }

    // ---------------------------------------------------------------------------------------- filtering and view


    /// The rig the "Rig" box filters by in the folder now shown (a rig of that `Character/<race>/ANI` folder's race), if any.
    pub fn active_rig(&self) -> Option<usize> {
        self.rig.filter(|i| rig_choices(&self.folder).contains(i))
    }

    /// Test hook and what the "Rig" box does: filters the `Character/<race>/ANI` folder shown to the rig `prefix` (`None`: all rigs).
    pub fn set_rig_filter(&mut self, prefix: Option<&str>) {
        self.rig = prefix.and_then(|p| shaiya_clientdata::anim::RIGS.iter().position(|(name, _)| name.eq_ignore_ascii_case(p)));
    }
    fn filter_stamp(&self) -> String {
        format!(
            "{}|{}|{}|{:?}|{}|{}|{:?}|{}|{}|{}|{:?}",
            self.folder,
            self.search,
            self.show_shadowed,
            self.types,
            self.min_size,
            self.max_size,
            self.sort_key,
            self.sort_desc,
            self.loaded.as_ref().map(|l| l.rows.len()).unwrap_or(0),
            self.loaded.as_ref().map(|l| l.sah.display().to_string()).unwrap_or_default(),
            self.active_rig()
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
            let rig = self.active_rig().map(|i| shaiya_clientdata::anim::RIGS[i]);
            for &i in l.files_in(&self.folder) {
                let r = &l.rows[i];
                if (r.shadowed && !self.show_shadowed) || !ok_size(r.entry.length) || !self.type_ok(&r.ext()) {
                    continue;
                }
                if rig.is_some() && shaiya_clientdata::anim::rig_of(&r.path).rig != rig {
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
    pub fn select_path(&mut self, path: &str, cx: &mut ToolkitCx<'_>) {
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

    fn start_preview(&mut self, row: usize, cx: &mut ToolkitCx<'_>) {
        self.start_preview_on(row, None, cx);
    }

    /// Starts the preview of `row`; `mesh` is the mesh choice of an `.ANI` preview to use instead of the default for a row that was not
    /// the open one.
    fn start_preview_on(&mut self, row: usize, mesh: Option<MeshChoice>, cx: &mut ToolkitCx<'_>) {
        if let Some(h) = &self.preview.handle {
            h.cancel();
        }
        self.preview.handle = None;
        if self.preview.row != Some(row) {
            self.preview.anim_mesh = MeshChoice::Default;
            self.preview.texture_pick = 0;
            self.preview.player = AnimPlayer { looping: cx.settings.preview_loop, show_bones: cx.settings.preview_bones, speed: cx.settings.preview_speed, ..AnimPlayer::default() };
        }
        if let Some(m) = mesh {
            self.preview.anim_mesh = m;
        }
        self.preview.controls.load(crate::preview_controls::view_of(cx.settings.preview_view));
        self.preview.fit = cx.settings.preview_fit;
        self.preview.alpha = cx.settings.preview_alpha;
        self.preview.textured = cx.settings.preview_textured;
        self.preview.row = Some(row);
        self.preview.data = None;
        self.preview.error = None;
        self.preview.decoded = None;
        self.preview.plain_view = false;
        let Some(l) = &self.loaded else { return };
        let Some(a) = l.archive.clone() else { return };
        let cache = l.mesh_cache.clone();
        let textures = l.texture_index.clone();
        let r = &l.rows[row];
        let (path, all, mesh) = (r.path.clone(), self.preview.text_all, self.preview.anim_mesh.clone());
        let look = crate::preview_tex::Look { textured: self.preview.textured, texture: self.preview.texture_pick, view: self.preview.controls.ctl.view };
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Preview {}", r.name()), cancellable: true, exclusive: None }, move |job| preview::make_preview_look(&*a, &path, all, 0, &mesh, &cache, &textures, &look, job));
        self.preview.handle = Some(h);
    }

    /// The kind of the current preview ("image", "text", "sdata", "hex", "empty", "failed"), once it is ready.
    pub fn preview_kind(&self) -> Option<&'static str> {
        self.preview.data.as_ref().map(|d| d.kind())
    }

    /// What the "Animation" box of a `.3DC` preview does: opens the `.ANI` at `ani_path` and draws it on the open mesh. Nothing happens when
    /// there is no such `.ANI` row or no mesh preview is open.
    pub fn play_animation_on_mesh(&mut self, ani_path: &str, cx: &mut ToolkitCx<'_>) {
        let Some(mesh) = self.preview.row.and_then(|r| self.loaded.as_ref()?.rows.get(r)).map(|r| r.path.clone()) else { return };
        let Some(i) = self.find_row(ani_path) else { return };
        let folder = self.loaded.as_ref().map(|l| l.rows[i].folder().to_string()).unwrap_or_default();
        if self.search.is_empty() {
            self.folder = folder;
        }
        self.selected.clear();
        self.selected.insert(i);
        self.anchor = Some(i);
        self.start_preview_on(i, Some(MeshChoice::One(mesh)), cx);
    }

    /// Draws the open `.ANI` on another choice of meshes (what the Mesh box does): the preview restarts with it.
    pub fn choose_anim_mesh(&mut self, choice: MeshChoice, cx: &mut ToolkitCx<'_>) {
        self.preview.anim_mesh = choice;
        if let Some(r) = self.preview.row {
            self.start_preview(r, cx);
        }
    }

    /// Pauses the player of an open `.ANI` preview at key `t` (the `anim-preview` scene). False when no animation preview is open.
    pub fn pause_anim_at(&mut self, t: f32) -> bool {
        let Some(PreviewData::Anim { end_key, .. }) = &self.preview.data else { return false };
        self.preview.player.scrub(t, *end_key);
        true
    }

    pub fn preview_ready(&self) -> bool {
        self.preview.handle.is_none() && (self.preview.data.is_some() || self.preview.error.is_some() || self.preview.row.is_none())
    }

    pub fn preview_data(&self) -> Option<&PreviewData> {
        self.preview.data.as_ref()
    }

    pub fn has_texture(&self) -> bool {
        if let Some(PreviewData::Anim { frames, .. }) = &self.preview.data {
            return !frames.frames.is_empty() && self.preview.anim_tex.is_some();
        }
        self.preview.texture_name.as_ref().is_some_and(|n| self.textures.iter().any(|(t, _)| t == n)) && matches!(self.preview.data, Some(PreviewData::Image { .. } | PreviewData::Map { .. } | PreviewData::Model { .. }))
    }

    /// A click on a column header: the same key flips the direction, another key starts ascending; both are kept in the settings.
    fn sort_by(&mut self, k: SortKey, cx: &mut ToolkitCx<'_>) {
        if self.sort_key == k {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort_key = k;
            self.sort_desc = false;
        }
        cx.settings.sort_key = self.sort_key.key().to_string();
        cx.settings.sort_desc = self.sort_desc;
    }

    /// Test hook: a click on the header of the `key` column ("name", "size", "packed", "type").
    #[doc(hidden)]
    pub fn sort_for_tests(&mut self, key: &str, cx: &mut ToolkitCx<'_>) {
        self.sort_by(SortKey::from_key(key), cx);
    }

    /// What the drawn controls asked for: the preview made again with the new view (kept in the settings), or the save dialog.
    fn after_controls(&mut self, cx: &mut ToolkitCx<'_>, acts: &mut Vec<Act>) {
        if self.preview.controls.ctl.take_render() {
            cx.settings.preview_view = crate::preview_controls::view_array(self.preview.controls.ctl.view);
            acts.push(Act::Open("view-change".into()));
        }
        if self.preview.controls.take_save() {
            acts.push(Act::Open("save-frame".into()));
        }
    }

    /// The pixels of the frame on screen (or the still picture) with the name its file would get: (`<stem>-<key>.png`, width, height, rgba).
    fn frame_on_screen(&self) -> Option<(String, u32, u32, Vec<u8>)> {
        let row = self.preview.row?;
        let r = self.loaded.as_ref()?.rows.get(row)?;
        let name = r.name();
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        match self.preview.data.as_ref()? {
            PreviewData::Anim { frames, .. } => {
                let idx = self.preview.player.frame_index(frames.step, frames.frames.len());
                let rgba = frames.frames.get(idx).filter(|f| !f.is_empty())?;
                Some((format!("{stem}-{}.png", self.preview.player.t.floor() as i64), frames.width as u32, frames.height as u32, rgba.clone()))
            }
            PreviewData::Model { width, height, rgba, .. } if !rgba.is_empty() => Some((format!("{stem}.png"), *width, *height, rgba.clone())),
            _ => None,
        }
    }

    fn save_frame_dialog(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some((name, ..)) = self.frame_on_screen() else { return };
        if let Some(out) = cx.shell.save_file("save-frame-png", &name, &[("PNG", &["png"])]) {
            let _ = self.save_frame_to(&out, cx);
        }
    }

    /// Writes the frame on screen (or the still picture) as a PNG to `out`; refused, with nothing written, inside a client folder.
    #[doc(hidden)]
    pub fn save_frame_to(&mut self, out: &Path, cx: &mut ToolkitCx<'_>) -> std::result::Result<(), String> {
        let Some((_, w, h, rgba)) = self.frame_on_screen() else { return Err("There is no picture to save".to_string()) };
        let dir = self.loaded.as_ref().and_then(|l| l.client_dir.clone());
        self.preview.controls.save(out.to_path_buf(), w, h, rgba, dir.as_deref(), cx)
    }

    /// Test hook: drags the picture by `dx`, `dy` pixels and lets go (the preview is made again on the next drawn frame).
    #[doc(hidden)]
    pub fn drag_view_for_tests(&mut self, dx: f32, dy: f32) {
        self.preview.controls.drag_for_tests(dx, dy);
    }

    /// Test hook: one wheel event over the picture (the preview is made again one drawn frame after).
    #[doc(hidden)]
    pub fn wheel_view_for_tests(&mut self, delta: f32) {
        self.preview.controls.wheel_for_tests(delta);
    }

    /// Test hook: "Reset view".
    #[doc(hidden)]
    pub fn reset_view_for_tests(&mut self) {
        self.preview.controls.ctl.reset();
    }

    #[doc(hidden)]
    pub fn is_saving_frame(&self) -> bool {
        self.preview.controls.is_saving()
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
    pub fn begin_extract(&mut self, out: PathBuf, all_shown: bool, whole_archive: bool, flat: bool, cx: &mut ToolkitCx<'_>) {
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

    fn start_extract(&mut self, req: ExtractReq, existing: Existing, cx: &mut ToolkitCx<'_>) {
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

    /// The one selected file when it is one of the six model formats (what "Export as JSON..." works on).
    fn export_json_row(&self) -> Option<usize> {
        let l = self.loaded.as_ref().filter(|l| l.archive.is_some())?;
        let mut it = self.selected.iter();
        let (&i, None) = (it.next()?, it.next()) else { return None };
        (self.selected_folder.is_none() && l.rows.get(i).is_some_and(|r| crate::export_json::is_model_path(&r.path))).then_some(i)
    }

    /// The one selected file when it is an `.ANI`, `.3DC`, `.3DO` or `.3DE` (what "Export as glTF..." works on).
    fn export_gltf_row(&self) -> Option<usize> {
        let l = self.loaded.as_ref().filter(|l| l.archive.is_some())?;
        let mut it = self.selected.iter();
        let (&i, None) = (it.next()?, it.next()) else { return None };
        (self.selected_folder.is_none() && l.rows.get(i).is_some_and(|r| crate::export_interchange::is_gltf_path(&r.path))).then_some(i)
    }

    fn export_gltf_row_to(&mut self, row: usize, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) else { return };
        let (entry, path) = (r.entry.clone(), r.path.clone());
        let dir = l.client_dir.clone();
        // an animation goes out on the one mesh the Mesh box shows, else alone
        let mesh = match (&self.preview.anim_mesh, is_ani_path(&path)) {
            (MeshChoice::One(m), true) => Some(m.clone()),
            _ => None,
        };
        self.gltf_exports.start(a, entry, path, mesh, out, dir.as_deref(), cx);
    }

    /// Whether the row menu of the file at `path` offers "Export as glTF..." (needs the data.saf and one of the four kinds).
    #[doc(hidden)]
    pub fn offers_export_gltf(&self, path: &str) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.archive.is_some()) && crate::export_interchange::is_gltf_path(path) && self.find_row(path).is_some()
    }

    /// Test hook: "Export as glTF..." for the selected file, writing to `path` (`.gltf` or `.glb`) without the save dialog.
    #[doc(hidden)]
    pub fn export_gltf_to(&mut self, path: &Path, cx: &mut ToolkitCx<'_>) {
        if let Some(i) = self.export_gltf_row() {
            self.export_gltf_row_to(i, path.to_path_buf(), cx);
        }
    }

    #[doc(hidden)]
    pub fn is_exporting_gltf(&self) -> bool {
        self.gltf_exports.is_running()
    }

    fn export_json_row_to(&mut self, row: usize, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) else { return };
        let (entry, path) = (r.entry.clone(), r.path.clone());
        let dir = l.client_dir.clone();
        self.json_exports.start(a, entry, path, out, dir.as_deref(), cx);
    }

    /// Whether the row menu of the file at `path` offers "Export as JSON..." (needs the data.saf and one of the six formats).
    #[doc(hidden)]
    pub fn offers_export_json(&self, path: &str) -> bool {
        let l = self.loaded.as_ref();
        l.is_some_and(|l| l.archive.is_some()) && crate::export_json::is_model_path(path) && self.find_row(path).is_some()
    }

    /// Test hook: "Export as JSON..." for the selected file, writing to `path` without the save dialog.
    #[doc(hidden)]
    pub fn export_json_to(&mut self, path: &Path, cx: &mut ToolkitCx<'_>) {
        if let Some(i) = self.export_json_row() {
            self.export_json_row_to(i, path.to_path_buf(), cx);
        }
    }

    #[doc(hidden)]
    pub fn is_exporting_json(&self) -> bool {
        self.json_exports.is_running()
    }

    /// The model files (one of the six formats) under the archive folder `folder` (every depth, shadowed copies left out), by path.
    pub fn model_files_under(&self, folder: &str) -> Vec<(String, shaiya_archive::Entry)> {
        let Some(l) = &self.loaded else { return Vec::new() };
        let prefix = format!("{}/", lower_key(folder.trim_matches('/')));
        let mut v: Vec<(String, shaiya_archive::Entry)> = l
            .rows
            .iter()
            .filter(|r| !r.shadowed && (prefix == "/" || r.lower.starts_with(&prefix)) && crate::export_json::is_model_path(&r.path))
            .map(|r| (r.path.clone(), r.entry.clone()))
            .collect();
        v.sort_by_key(|(p, _)| lower_key(p));
        v
    }

    /// "Export folder as JSON...": every model file under the archive folder `folder` is written as `<path>.json` under `out`, on a worker
    /// (an output that exists is kept and listed). Refused with a message, before anything is read, when `out` is inside a client folder.
    pub fn export_folder_json_to(&mut self, folder: &str, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let Some(a) = l.archive.clone() else { return };
        let dir = l.client_dir.clone();
        let items = self.model_files_under(folder);
        if items.is_empty() {
            cx.shell.status(format!("No model files under {}", if folder.is_empty() { "the archive" } else { folder }));
            return;
        }
        self.folder_exports.start(a, items, out, dir.as_deref(), cx);
    }

    #[doc(hidden)]
    pub fn is_exporting_folder(&self) -> bool {
        self.folder_exports.is_running()
    }

    /// Asks the running folder export to stop after the file it is on.
    pub fn cancel_folder_export(&self) {
        self.folder_exports.cancel();
    }

    /// The report of the last folder export that ended.
    pub fn folder_export_report(&self) -> Option<&crate::export_json::FolderReport> {
        self.folder_exports.last.as_ref()
    }

    /// Opens the "Compare with..." window for the `.ANI` in row `row`.
    fn open_compare(&mut self, row: usize, cx: &ToolkitCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) else { return };
        let path = r.path.clone();
        let mut siblings: Vec<String> = l.files_in(r.folder()).iter().map(|&i| &l.rows[i]).filter(|x| !x.shadowed && is_ani_path(&x.path) && x.path != path).map(|x| x.path.clone()).collect();
        siblings.sort_by_key(|p| lower_key(p));
        let open_dir = l.client_dir.clone();
        let clients: Vec<(String, PathBuf)> = cx.shell.shared().clients.iter().map(|c| (c.name.clone(), PathBuf::from(&c.path))).filter(|(_, d)| has_sah_file(d) && Some(d) != open_dir.as_ref()).collect();
        self.anim_compare = Some(crate::anim_compare::AnimCompare::new(path, a, siblings, clients));
    }

    /// Opens the "Edit animation..." window for the `.ANI` in row `row`.
    fn open_edit(&mut self, row: usize) {
        let Some(l) = &self.loaded else { return };
        let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) else { return };
        let path = r.path.clone();
        let mut siblings: Vec<String> = l.files_in(r.folder()).iter().map(|&i| &l.rows[i]).filter(|x| !x.shadowed && is_ani_path(&x.path) && x.path != path).map(|x| x.path.clone()).collect();
        siblings.sort_by_key(|p| lower_key(p));
        self.anim_edit = Some(crate::anim_edit::AnimEdit::new(path, a, siblings, l.client_dir.clone()));
    }

    /// Test hook: "Edit animation..." for the `.ANI` at `path` with `tool` chosen (the worker starts at once).
    #[doc(hidden)]
    pub fn edit_ani_with(&mut self, path: &str, tool: crate::anim_edit::Tool, cx: &mut ToolkitCx<'_>) {
        let Some(i) = self.find_row(path) else { return };
        self.open_edit(i);
        if let Some(c) = &mut self.anim_edit {
            c.set_tool(&tool);
            c.start(tool, cx);
        }
    }

    /// The window of "Edit animation...", when open.
    #[doc(hidden)]
    pub fn anim_edit(&self) -> Option<&crate::anim_edit::AnimEdit> {
        self.anim_edit.as_ref()
    }

    #[doc(hidden)]
    pub fn anim_edit_mut(&mut self) -> Option<&mut crate::anim_edit::AnimEdit> {
        self.anim_edit.as_mut()
    }

    /// Whether the row menu of the file at `path` offers "Edit animation..." (an `.ANI` with the `data.saf` open).
    #[doc(hidden)]
    pub fn offers_edit(&self, path: &str) -> bool {
        self.offers_compare(path)
    }

    /// Test hook: "Queue as replacement" in the open edit window (the staged file goes through the replace checks).
    #[doc(hidden)]
    pub fn queue_ani_edit(&mut self, cx: &mut ToolkitCx<'_>) {
        if let Some(c) = &mut self.anim_edit {
            c.want_queue();
        }
        self.finish_ani_edit_queue(cx);
    }

    fn finish_ani_edit_queue(&mut self, cx: &mut ToolkitCx<'_>) {
        // the staged files that queued changes use stay, whatever their age
        let keep: Vec<PathBuf> = self.pending.iter().filter_map(|p| if let PendingKind::Put { source, .. } = &p.kind { Some(source.clone()) } else { None }).collect();
        let Some(c) = &mut self.anim_edit else { return };
        let Some(file) = c.take_queue(cx, &keep) else { return };
        let path = c.path().to_string();
        if let Some(i) = self.find_row(&path) {
            self.replace_with_file(i, file, cx);
        }
    }

    /// Test hook: "Compare with..." for the `.ANI` at `path`, choosing `other` and pressing Compare.
    #[doc(hidden)]
    pub fn compare_ani_with(&mut self, path: &str, other: crate::anim_compare::Other, cx: &mut ToolkitCx<'_>) {
        let Some(i) = self.find_row(path) else { return };
        self.open_compare(i, cx);
        if let Some(c) = &mut self.anim_compare {
            c.start(other, cx);
        }
    }

    /// The window of "Compare with...", when open.
    #[doc(hidden)]
    pub fn anim_compare(&self) -> Option<&crate::anim_compare::AnimCompare> {
        self.anim_compare.as_ref()
    }

    /// Whether the row menu of the file at `path` offers "Compare with..." (an `.ANI` with the `data.saf` open).
    #[doc(hidden)]
    pub fn offers_compare(&self, path: &str) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.archive.is_some()) && is_ani_path(path) && self.find_row(path).is_some()
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

    pub fn run_check(&mut self, deep: bool, cx: &mut ToolkitCx<'_>) {
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

    /// Opens the Compare dialog: OLD (left) is the open client or archive, NEW (right) is chosen next.
    pub fn begin_compare(&mut self) {
        if self.compare.is_some() {
            return;
        }
        let left = match &self.source {
            Some(OpenSource::Client { name, dir }) => Some(CompareSide::Client { name: name.clone(), dir: dir.clone() }),
            Some(OpenSource::File(p)) => Some(CompareSide::File(p.clone())),
            None => None,
        };
        self.compare = Some(ComparePanel { left, right: None, ran: None, entries: Vec::new(), summary: DiffSummary::default(), handle: None, show: [true, true, true, true] });
    }

    /// Compares OLD = `left` with NEW = `right` (any saved client or archive file on each side) as a task.
    pub fn run_compare(&mut self, left: CompareSide, right: CompareSide, cx: &mut ToolkitCx<'_>) {
        let (old_sah, new_sah) = match (left.sah(), right.sah()) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                cx.shell.error("Cannot compare", &e);
                return;
            }
        };
        let title = format!("Comparing {} with {}", left.label(), right.label());
        let (o2, n2) = (old_sah.clone(), new_sah.clone());
        let handle = cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: None }, move |job| {
            let a = Archive::open(&o2)?;
            let b = Archive::open(&n2)?;
            let d = diff(&Side::Archive(&a), &Side::Archive(&b), &DiffOptions { content: true, ..Default::default() }, job)?;
            let s = summarize(&d);
            Ok((d, s))
        });
        self.compare = Some(ComparePanel {
            left: Some(left.clone()),
            right: Some(right.clone()),
            ran: Some(CompareRan { left, right, old_sah, new_sah }),
            entries: Vec::new(),
            summary: DiffSummary::default(),
            handle: Some(handle),
            show: [true, true, true, true],
        });
    }

    /// Opens the "Add to archive" dialog for dropped or chosen files and folders.
    pub fn begin_add(&mut self, paths: Vec<PathBuf>) {
        let target = self.folder.clone();
        let mut d = AddDialog { paths, target, keep_folder_names: false, rows: Vec::new(), error: None, findings: Vec::new(), checked: false, asking: false, check_gen: 0, checking: None };
        self.recompute_add(&mut d);
        self.add_dialog = Some(d);
    }

    pub fn recompute_add(&self, d: &mut AddDialog) {
        d.rows.clear();
        d.error = None;
        d.findings.clear();
        d.checked = false;
        d.asking = false;
        d.check_gen += 1;
        d.checking = None;
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

    /// "Replace with file...": checks the file against the entry it replaces in a background task (an SData file is read and
    /// decrypted), then queues it. A file the checks ask about waits for "Add anyway" (see `replace_ask`); other findings are shown in
    /// the status bar and the file is queued. A check that did not finish asks too.
    pub fn replace_with_file(&mut self, row: usize, file: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(l) = &self.loaded else { return };
        let path = l.rows[row].path.clone();
        let version = mtime(&file);
        let items = vec![(path.clone(), file.clone(), Some(l.rows[row].entry.clone()))];
        let archive = l.archive.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Checking the files".to_string(), cancellable: false, exclusive: None }, move |_job| Ok(check_items(&items, archive.as_deref())));
        self.replace_checks.push((vec![(path, file, version)], h));
    }

    /// Finishes the "Replace with file..." checks that are done: queues the file, or opens the question.
    fn poll_replace_checks(&mut self, cx: &mut ToolkitCx<'_>) {
        for (put, mut h) in std::mem::take(&mut self.replace_checks) {
            let Some(outcome) = h.poll() else {
                self.replace_checks.push((put, h));
                continue;
            };
            let findings = check_outcome(outcome);
            if validate::any_asks(&findings) {
                self.replace_ask = Some((put, findings));
                continue;
            }
            if let Some(first) = findings.first() {
                cx.shell.status(format!("Queued, with a warning: {}", first.line()));
            }
            self.queue_puts(put);
        }
    }

    /// What the file checks of the dialog look at: each row's file and the entry the archive already holds at that path.
    fn add_check_items(&self, d: &AddDialog) -> Vec<CheckItem> {
        d.rows
            .iter()
            .map(|r| {
                let entry = self.find_row(&r.path).and_then(|i| self.loaded.as_ref().map(|l| l.rows[i].entry.clone()));
                (r.path.clone(), r.source.clone(), entry)
            })
            .collect()
    }

    /// What pressing "Queue changes" does (tests press it here): queues, or opens the question.
    pub fn press_queue_changes(&mut self) {
        if self.add_dialog.as_mut().is_some_and(AddDialog::press_queue) {
            self.queue_add_dialog();
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
    ExportJson(usize),
    ExportGltf(usize),
    ExportFolderJson(usize),
    CompareAni(usize),
    EditAni(usize),
    PlayAnimation(String),
    AddFiles,
    QueueRemove,
    CopyPaths,
    CopyJson,
    ShowInTree(usize),
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
        let writes = self.write_actions_enabled();
        let c = |id: &'static str, label: &str, shortcut: Option<K>, enabled: bool| Command { id, label: label.to_string(), shortcut, enabled };
        vec![
            c("archive-search", "Search the archive", Some(K::new(M::COMMAND, Key::F)), self.is_open()),
            c("archive-extract", "Extract selected files...", Some(K::new(M::COMMAND, Key::E)), open && (!self.selected.is_empty() || self.selected_folder.is_some())),
            c("archive-extract-all", "Extract all shown files...", Some(K::new(M::COMMAND | M::SHIFT, Key::E)), open),
            c("archive-export-json", "Export as JSON...", None, self.export_json_row().is_some()),
            c("archive-export-gltf", "Export as glTF...", None, self.export_gltf_row().is_some()),
            c("archive-add", "Add files to the archive...", Some(K::new(M::NONE, Key::Insert)), edit),
            c("archive-remove", "Queue removal of selected files", Some(K::new(M::NONE, Key::Delete)), edit && !self.selected.is_empty()),
            c("archive-apply", "Apply pending changes to the client...", Some(K::new(M::COMMAND, Key::Enter)), edit && !busy && writes && !self.pending.is_empty()),
            c("archive-save-new", "Save as new archive...", None, open && writes && !self.pending.is_empty()),
            c("archive-make-patch", "Make patch from pending changes", None, !self.pending.is_empty()),
            c("archive-discard", "Discard pending changes", None, writes && !self.pending.is_empty()),
            c("archive-check", "Check the archive", None, open),
            c("archive-compare", "Compare two clients or archives...", None, true),
            c("archive-compact", "Compact data.saf...", None, edit && !busy && writes),
            c("archive-reload", "Reload the archive", Some(K::new(M::NONE, Key::F5)), self.source.is_some()),
            c("archive-copy-paths", "Copy selected paths", Some(K::new(M::COMMAND, Key::C)), !self.selected.is_empty()),
            c("archive-up", "Parent folder", Some(K::new(M::NONE, Key::Backspace)), self.is_open()),
            c("archive-goto", "Go to offset (hex preview)", Some(K::new(M::COMMAND, Key::G)), self.is_open()),
        ]
    }

    /// Runs a palette entry of [`ArchiveView::commands`].
    pub fn run_command(&mut self, id: &str, cx: &mut ToolkitCx<'_>) -> Vec<TabRequest> {
        let mut requests = Vec::new();
        let act = match id {
            "archive-search" => {
                self.want_search_focus = true;
                None
            }
            "archive-extract" => Some(Act::Extract { all: false, flat: false }),
            "archive-extract-all" => Some(Act::Extract { all: true, flat: false }),
            "archive-export-json" => self.export_json_row().map(Act::ExportJson),
            "archive-export-gltf" => self.export_gltf_row().map(Act::ExportGltf),
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

    /// Draws the tab. Returns the actions the app frame handles that need the shell (dialogs and tab switches).
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) -> Vec<TabRequest> {
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
                let mut chosen = None;
                cx.shell.empty_state_with(ui, "Choose a client folder in the toolbar, or drop a client folder, data.sah or patch here. Nothing is written until you apply changes.", |ui| chosen = crate::start::draw(ui));
                if let Some(a) = chosen {
                    requests.push(TabRequest::Start(a));
                }
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
            // under --shot a saved client is shown by its name, never by its path
            let client_name = match &self.source {
                Some(OpenSource::Client { name, .. }) => name.clone(),
                _ => String::new(),
            };
            let saf = match (l.saf_len, l.dead) {
                (Some(n), Some(d)) => format!("data.saf {}, {} unused", human_size(n), human_size(d)),
                _ => "data.saf not found".to_string(),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(l.sah.parent().map(|p| cx.shell.folder_text(&client_name, p)).unwrap_or_default()).strong());
                ui.label(format!("data.sah v{}, {} ({} shadowed)  ·  {saf}", l.sah_version, shaiya_inspect::counted((l.visible) as u64, "file", "files"), l.rows.len() - l.visible));
                if !has_saf {
                    ui.label(RichText::new("browsing the index only: preview, extract and changes are off").color(pal.warning));
                }
                if ui.add_enabled(has_saf, egui::Button::new("Check")).clicked() {
                    acts.push(Act::Check(false));
                }
                if ui.button("Compare...").on_hover_text("Compare any two saved clients or archive files: OLD on the left, NEW on the right").clicked() {
                    acts.push(Act::Compare);
                }
                if lock_hover(ui.add_enabled(can_edit && !self.is_applying() && self.write_actions_enabled(), egui::Button::new("Compact...")), !self.write_actions_enabled()).clicked() {
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
            let r = ui.add(egui::TextEdit::singleline(&mut self.search).desired_width(180.0).hint_text("text or *.dds").id(egui::Id::unique("toolkit-search")));
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
            let rigs = if self.search.is_empty() { rig_choices(&self.folder) } else { Vec::new() };
            if !rigs.is_empty() {
                let current = self.active_rig();
                let mut pick = current;
                egui::ComboBox::from_id_salt("toolkit-rig-filter")
                    .selected_text(format!("Rig: {}", current.map_or("all", |i| shaiya_clientdata::anim::RIGS[i].0)))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut pick, None, "all");
                        for i in rigs {
                            ui.selectable_value(&mut pick, Some(i), shaiya_clientdata::anim::RIGS[i].0);
                        }
                    });
                if pick != current {
                    self.rig = pick;
                }
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
            egui::Panel::bottom("toolkit-pending").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Pending changes ({}) for {}:", self.pending.len(), self.source_label().unwrap_or_default())).strong());
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
                    let writes = self.write_actions_enabled();
                    if lock_hover(ui.add_enabled(can_edit && !busy && writes, egui::Button::new("Apply to client...")), !writes).clicked() {
                        acts.push(Act::ApplyPending);
                    }
                    if lock_hover(ui.add_enabled(has_saf && writes, egui::Button::new("Save as new archive...")), !writes).clicked() {
                        acts.push(Act::SaveNew);
                    }
                    if ui.button("Make patch...").clicked() {
                        acts.push(Act::MakePatch);
                    }
                    if lock_hover(ui.add_enabled(writes, egui::Button::new("Discard")), !writes).clicked() {
                        acts.push(Act::Discard);
                    }
                });
            });
        }
        let tree_w = cx.settings.tree_width.clamp(120.0, 600.0);
        let prev_w = cx.settings.preview_width.clamp(200.0, 800.0);
        // A side panel of this egui is only as wide as its content (it does not keep the width it was dragged to), and `default_size` is the
        // OUTER width. So the content fills the width it was given (`set_min_width`), and the saved width is the panel's outer width, which
        // is what the next start passes to `default_size` (the inner width lost the frame margins at every start).
        let tree = egui::Panel::left("toolkit-tree").resizable(true).default_size(tree_w).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                let l = self.loaded.as_ref().expect("checked");
                draw_tree(ui, l, 0, &self.folder, &mut acts);
            });
        });
        let preview = egui::Panel::right("toolkit-preview").resizable(true).default_size(prev_w).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            self.draw_preview(ui, cx, &mut acts);
        });
        remember_width(&mut cx.settings.tree_width, tree.response.rect.width());
        remember_width(&mut cx.settings.preview_width, preview.response.rect.width());
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

    fn draw_table(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, acts: &mut Vec<Act>) {
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
                            (format!("{}/", nd.name), human_size(nd.bytes), shaiya_inspect::counted((nd.files) as u64, "file", "files"), "folder".to_string(), pal.text, nd.path.clone())
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
                            (label, human_size(r.entry.length as u64), if r.entry.version > 0 { utc_iso_unix(r.entry.version as i64)[..10].to_string() } else { "-".into() }, r.type_name(), self.color_for(r, &pal), r.path.clone())
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
                                ("Show in folder tree", MenuChoice::ShowInTree),
                                ("Replace with file...", MenuChoice::Replace),
                                ("Remove", MenuChoice::Remove),
                                ("Decrypt to file...", MenuChoice::Decrypt),
                                ("Export as JSON...", MenuChoice::ExportJson),
                                ("Export as glTF...", MenuChoice::ExportGltf),
                                ("Compare with...", MenuChoice::CompareWith),
                                ("Edit animation...", MenuChoice::EditAnimation),
                            ] {
                                if matches!(choice, MenuChoice::ExportJson) && !matches!(v, ViewRow::File(i) if l.archive.is_some() && crate::export_json::is_model_path(&l.rows[i].path)) {
                                    continue;
                                }
                                if matches!(choice, MenuChoice::ExportGltf) && !matches!(v, ViewRow::File(i) if l.archive.is_some() && crate::export_interchange::is_gltf_path(&l.rows[i].path)) {
                                    continue;
                                }
                                if matches!(choice, MenuChoice::EditAnimation) && !matches!(v, ViewRow::File(i) if l.archive.is_some() && is_ani_path(&l.rows[i].path)) {
                                    continue;
                                }
                                if matches!(choice, MenuChoice::CompareWith) && !matches!(v, ViewRow::File(i) if l.archive.is_some() && is_ani_path(&l.rows[i].path)) {
                                    continue;
                                }
                                let mut b = ui.button(label);
                                if matches!(choice, MenuChoice::ExportGltf) {
                                    b = b.on_hover_text(crate::export_interchange::LOSSY_SENTENCE);
                                }
                                if b.clicked() {
                                    menu = Some((vi, choice));
                                    ui.close();
                                }
                            }
                        } else {
                            if ui.button("Open").clicked() {
                                open = Some(path.clone());
                                ui.close();
                            }
                            if l.archive.is_some() && ui.button("Export folder as JSON...").clicked() {
                                menu = Some((vi, MenuChoice::ExportFolderJson));
                                ui.close();
                            }
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
                MenuChoice::CopyPath => acts.push(Act::CopyPaths),
                MenuChoice::CopyJson => acts.push(Act::CopyJson),
                MenuChoice::ShowInTree => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::ShowInTree(i));
                    }
                }
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
                MenuChoice::ExportJson => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::ExportJson(i));
                    }
                }
                MenuChoice::ExportGltf => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::ExportGltf(i));
                    }
                }
                MenuChoice::CompareWith => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::CompareAni(i));
                    }
                }
                MenuChoice::EditAnimation => {
                    if let ViewRow::File(i) = self.view[vi] {
                        acts.push(Act::EditAni(i));
                    }
                }
                MenuChoice::ExportFolderJson => {
                    if let ViewRow::Folder(n) = self.view[vi] {
                        acts.push(Act::ExportFolderJson(n));
                    }
                }
            }
        }
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, acts: &mut Vec<Act>) {
        let pal = cx.shell.pal();
        let Some(row) = self.preview.row else {
            ui.label(RichText::new("Select a file to preview it.").color(pal.weak));
            return;
        };
        let Some(l) = &self.loaded else { return };
        let Some(r) = l.rows.get(row) else { return };
        let switch = crate::preview_tex::switch_applies(&r.path);
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
                let tex = self.preview.texture_name.as_ref().and_then(|name| self.textures.iter().find(|(n, _)| n == name)).map(|(_, t)| t);
                picture_view(ui, &mut self.preview.fit, Some(&mut self.preview.alpha), tex, None);
                cx.settings.preview_alpha = self.preview.alpha;
                cx.settings.preview_fit = self.preview.fit;
            }
            Some(PreviewData::Map { header, height_range, note, width, height, downscaled, rgba }) => {
                ui.label(header);
                if let Some((lo, hi)) = height_range {
                    ui.label(format!("height from {lo:.1} to {hi:.1}"));
                }
                if let Some(note) = note {
                    ui.label(RichText::new(note).color(pal.weak));
                }
                if *downscaled {
                    ui.label(RichText::new(format!("shown at {width} x {height} (downscaled)")).color(pal.weak));
                }
                if !rgba.is_empty() {
                    let tex = self.preview.texture_name.as_ref().and_then(|name| self.textures.iter().find(|(n, _)| n == name)).map(|(_, t)| t);
                    picture_view(ui, &mut self.preview.fit, None, tex, None);
                    cx.settings.preview_fit = self.preview.fit;
                }
            }
            Some(data @ PreviewData::Anim { .. }) => {
                if switch && textures_switch(&mut self.preview.textured, ui, cx) {
                    acts.push(Act::Open("texture-choice".into()));
                }
                if anim_view::draw(ui, &pal, &mut self.preview.player, data, &mut self.preview.anim_tex, &mut self.preview.anim_mesh, &mut self.preview.controls) {
                    acts.push(Act::Open("anim-mesh".into()));
                }
                cx.settings.preview_loop = self.preview.player.looping;
                cx.settings.preview_bones = self.preview.player.show_bones;
                cx.settings.preview_speed = self.preview.player.speed;
            }
            Some(data @ PreviewData::Fields { .. }) => anim_view::draw_fields(ui, &pal, data),
            Some(PreviewData::Model { header, missing, see_through, invisible, notes, texture, animations, rgba, .. }) => {
                ui.label(header);
                if let Some(t) = texture {
                    ui.label(RichText::new(&t.line).color(pal.weak));
                    if t.choices.len() > 1 {
                        let mut pick = t.chosen;
                        egui::ComboBox::from_id_salt("toolkit-texture-pick").selected_text(format!("Texture: {}", t.choices[t.chosen])).show_ui(ui, |ui| {
                            for (i, label) in t.choices.iter().enumerate() {
                                ui.selectable_value(&mut pick, i, label);
                            }
                        });
                        if pick != t.chosen {
                            self.preview.texture_pick = pick;
                            acts.push(Act::Open("texture-choice".into()));
                        }
                    }
                }
                if switch && textures_switch(&mut self.preview.textured, ui, cx) {
                    acts.push(Act::Open("texture-choice".into()));
                }
                if !animations.is_empty() {
                    let mut chosen: Option<String> = None;
                    egui::ComboBox::from_id_salt("toolkit-mesh-animation").selected_text("Animation: play on this mesh").show_ui(ui, |ui| {
                        for p in animations {
                            if ui.selectable_label(false, preview::file_name(p)).on_hover_text(p).clicked() {
                                chosen = Some(p.clone());
                            }
                        }
                    });
                    if let Some(p) = chosen {
                        acts.push(Act::PlayAnimation(p));
                    }
                }
                for note in notes {
                    ui.label(RichText::new(note).color(pal.weak));
                }
                if !missing.is_empty() {
                    ui.label(RichText::new(format!("texture missing or unreadable, drawn grey: {}", missing.join(", "))).color(pal.warning));
                }
                if !see_through.is_empty() {
                    ui.label(RichText::new(format!("see-through texels are cut out: {}", see_through.join(", "))).color(pal.weak));
                }
                if !invisible.is_empty() {
                    ui.label(RichText::new(format!("not drawn (no opaque texel): {}", invisible.join(", "))).color(pal.weak));
                }
                if rgba.is_empty() {
                    ui.label(RichText::new("no triangles to draw").color(pal.weak));
                } else {
                    let tex = self.preview.texture_name.as_ref().and_then(|name| self.textures.iter().find(|(n, _)| n == name)).map(|(_, t)| t);
                    self.preview.controls.buttons(ui, true);
                    picture_view(ui, &mut self.preview.fit, None, tex, Some(&mut self.preview.controls));
                    cx.settings.preview_fit = self.preview.fit;
                }
            }
            Some(PreviewData::Text { bytes, total, truncated, guess }) => {
                ui.horizontal(|ui| {
                    let mut chosen = self.preview.encoding;
                    egui::ComboBox::from_id_salt("toolkit-enc").selected_text(preview::encoding_name(chosen)).show_ui(ui, |ui| {
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
                    ui.label(format!("plaintext {}", shaiya_inspect::counted(summary.plain_size, "byte", "bytes")));
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
                    if ui.button("View table").on_hover_text("Show the file as rows and columns when a table layout fits it").clicked() {
                        self.table_request = Some((r.path.to_string(), raw.clone()));
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
        self.after_controls(cx, acts);
    }

    fn draw_dialogs(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, requests: &mut Vec<TabRequest>) {
        let ctx = ui.ctx().clone();
        self.poll_replace_checks(cx);
        self.folder_exports.draw_failures(&ctx);
        if let Some(c) = &mut self.anim_compare {
            c.draw(&ctx, cx);
            if !c.open {
                self.anim_compare = None;
            }
        }
        if let Some(c) = &mut self.anim_edit {
            c.draw(&ctx, cx);
            if !c.open {
                self.anim_edit = None;
            }
        }
        self.finish_ani_edit_queue(cx);
        // Replace with a file that looks broken
        if let Some((items, findings)) = self.replace_ask.take() {
            let c = Confirm {
                title: crate::replace_title::findings_title(&findings).into(),
                text: findings_question(&findings, "queue"),
                buttons: vec!["Add anyway".into(), "Cancel".into()],
                ..Default::default()
            };
            match cx.shell.confirm("toolkit-replace-findings", &c) {
                None => self.replace_ask = Some((items, findings)),
                Some(0) => self.queue_puts(items),
                Some(_) => {}
            }
        }
        // Add to archive
        let mut action: Option<&'static str> = None;
        if let Some(mut d) = self.add_dialog.take() {
            let mut changed = false;
            // the file checks run in the background; the answer is used only when it is for the rows now shown
            if let Some((g, mut h)) = d.checking.take() {
                match h.poll() {
                    None => d.checking = Some((g, h)),
                    Some(outcome) => {
                        if g == d.check_gen {
                            d.finish_check(outcome);
                        }
                    }
                }
            }
            if d.rows.is_empty() {
                d.checked = true;
            } else if !d.checked && d.checking.is_none() {
                let items = self.add_check_items(&d);
                let archive = self.loaded.as_ref().and_then(|l| l.archive.clone());
                let h = cx.shell.tasks().spawn(TaskSpec { title: "Checking the files".to_string(), cancellable: false, exclusive: None }, move |_job| Ok(check_items(&items, archive.as_deref())));
                d.checking = Some((d.check_gen, h));
            }
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
                if !d.checked {
                    ui.label(RichText::new("Checking the files...").color(cx.shell.pal().weak));
                }
                if !d.findings.is_empty() {
                    ui.label(RichText::new(format!("Checks ({}); they only warn", d.findings.len())).strong());
                    egui::ScrollArea::vertical().id_salt("toolkit-add-findings").max_height(110.0).show(ui, |ui| {
                        for f in &d.findings {
                            ui.label(RichText::new(f.line()).color(if f.asks() { cx.shell.pal().error } else { cx.shell.pal().warning }));
                        }
                    });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.rows.is_empty() && d.checked, egui::Button::new("Queue changes")).clicked() && d.press_queue() {
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
            if d.asking {
                let c = Confirm {
                    title: "Some files look broken".into(),
                    text: findings_question(&d.findings, "add"),
                    buttons: vec!["Add anyway".into(), "Cancel".into()],
                    ..Default::default()
                };
                match cx.shell.confirm("toolkit-add-findings", &c) {
                    None => {}
                    Some(0) => action = Some("queue"),
                    Some(_) => d.asking = false,
                }
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
                    ui.label(format!("{}, {}", shaiya_inspect::counted((r.problems()) as u64, "problem", "problems"), shaiya_inspect::counted((r.warnings()) as u64, "warning", "warnings")));
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
            let mut run = false;
            let mut file_for: Option<bool> = None;
            let mut select: Option<String> = None;
            let pal = cx.shell.pal();
            let saved: Vec<shaiya_ui::Client> = cx.shell.shared().clients.clone();
            let c = self.compare.as_mut().expect("checked");
            egui::Window::new("Compare").open(&mut open).default_size([560.0, 460.0]).show(&ctx, |ui| {
                ui.label(RichText::new("OLD is the left side and NEW the right side. A patch made from the differences turns OLD into NEW.").color(pal.weak));
                for (is_right, title) in [(false, "OLD (left)"), (true, "NEW (right)")] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).strong());
                        let shown = if is_right { c.right.as_ref() } else { c.left.as_ref() }.map(|s| s.label()).unwrap_or_else(|| "(choose a client or archive file)".to_string());
                        egui::ComboBox::from_id_salt(if is_right { "toolkit-compare-right" } else { "toolkit-compare-left" }).selected_text(shown).width(260.0).show_ui(ui, |ui| {
                            for cl in &saved {
                                if !has_sah_file(Path::new(&cl.path)) {
                                    continue;
                                }
                                let side = CompareSide::Client { name: cl.name.clone(), dir: PathBuf::from(&cl.path) };
                                let slot = if is_right { &mut c.right } else { &mut c.left };
                                if ui.selectable_label(slot.as_ref() == Some(&side), &cl.name).clicked() {
                                    *slot = Some(side);
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui.button("Archive file...").clicked() {
                                file_for = Some(is_right);
                                ui.close();
                            }
                        });
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button("Swap sides").clicked() {
                        c.swap_sides();
                    }
                    let ready = c.left.is_some() && c.right.is_some() && !c.is_running();
                    if ui.add_enabled(ready, egui::Button::new("Compare")).clicked() {
                        run = true;
                    }
                });
                if c.is_running() {
                    ui.label("Comparing...");
                    return;
                }
                if !c.is_current() {
                    if c.ran.is_some() {
                        ui.label(RichText::new("The sides changed; press Compare to see the differences.").color(pal.weak));
                    }
                    return;
                }
                let s = c.summary;
                let (old, new) = c.ran.as_ref().map(|r| (r.left.label(), r.right.label())).unwrap_or_default();
                ui.label(format!("OLD {old}  ->  NEW {new}"));
                ui.label(format!("{} added, {} removed, {} different, {} same", s.added, s.removed, s.different, s.same));
                if nothing_differs(&s) {
                    ui.label(RichText::new("Nothing differs: OLD and NEW hold the same files with the same contents, so there is nothing to make a patch from.").color(pal.ok));
                }
                ui.horizontal(|ui| {
                    for (i, name) in ["added", "removed", "different", "unknown"].iter().enumerate() {
                        ui.checkbox(&mut c.show[i], *name);
                    }
                });
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
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
                let worthy = patch_worthy(&s);
                if ui
                    .add_enabled(worthy, egui::Button::new("Make patch from differences"))
                    .on_hover_text("The patch holds the added and different files of NEW (right) and removes the files only OLD (left) has")
                    .on_disabled_hover_text("Nothing differs, so the patch would be empty")
                    .clicked()
                {
                    make_patch = true;
                }
                if worthy {
                    ui.label(RichText::new(format!("OLD = {old} (left), NEW = {new} (right)")).color(pal.weak));
                }
            });
            if let Some(is_right) = file_for {
                c.choose_file(is_right, cx.shell.pick_file("compare", &[("SAH index", &["sah"])]));
            }
            if make_patch {
                requests.extend(c.patch_request());
            }
            let start = if run { c.left.clone().zip(c.right.clone()) } else { None };
            if !open {
                self.compare = None;
            }
            if let Some((l, r)) = start {
                self.run_compare(l, r, cx);
            }
            if let Some(p) = select {
                self.select_path(&p, cx);
            }
        }
        // F5 with pending changes
        if self.reload_asked {
            let c = Confirm { title: "Reload the archive".into(), text: format!("Reload the archive and discard {}?", shaiya_inspect::counted((self.pending.len()) as u64, "pending change", "pending changes")), buttons: vec!["Reload".into(), "Keep".into()], danger: Some(0), ..Default::default() };
            match cx.shell.confirm("toolkit-reload", &c) {
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
            let c = Confirm { title: "Discard pending changes".into(), text: format!("Discard {}?", shaiya_inspect::counted((self.pending.len()) as u64, "pending change", "pending changes")), buttons: vec!["Discard".into(), "Keep".into()], danger: Some(0), ..Default::default() };
            match cx.shell.confirm("toolkit-discard", &c) {
                None => {}
                Some(0) => {
                    self.discard_pending();
                    self.discard_asked = false;
                }
                Some(_) => self.discard_asked = false,
            }
        }
    }

    fn run_acts(&mut self, acts: Vec<Act>, cx: &mut ToolkitCx<'_>, requests: &mut Vec<TabRequest>) {
        for a in acts {
            // keys and palette entries reach the same acts as the buttons: they are off while a multi-client apply runs
            if !self.write_actions_enabled() && matches!(a, Act::ApplyPending | Act::SaveNew | Act::Discard | Act::Compact) {
                cx.shell.status(crate::multi_lock::MULTI_RUNNING_HOVER);
                continue;
            }
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
                    if what == "reload-preview" || what == "anim-mesh" || what == "texture-choice" || what == "view-change" {
                        if let Some(r) = self.preview.row {
                            self.start_preview(r, cx);
                        }
                    }
                    if what == "save-frame" {
                        self.save_frame_dialog(cx);
                    }
                    if what == "goto" {
                        let t = self.preview.goto.trim().to_string();
                        let v = t.strip_prefix("0x").map(|h| u64::from_str_radix(h, 16).ok()).unwrap_or_else(|| t.parse().ok());
                        if let (Some(off), Some(row)) = (v, self.preview.row) {
                            if let (Some(l), true) = (&self.loaded, true) {
                                if let (Some(a), Some(r)) = (l.archive.clone(), l.rows.get(row)) {
                                    let path = r.path.clone();
                                    let h = cx.shell.tasks().spawn(TaskSpec { title: "Preview".into(), cancellable: true, exclusive: None }, move |job| preview::make_preview(&*a, &path, false, off, job));
                                    self.preview.handle = Some(h);
                                    self.preview.data = None;
                                }
                            }
                        }
                    }
                }
                Act::Sort(k) => self.sort_by(k, cx),
                Act::Extract { all, flat } => {
                    if let Some(dir) = cx.shell.pick_folder("extract") {
                        self.begin_extract(dir, all, false, flat, cx);
                    }
                }
                Act::ExportFolderJson(n) => {
                    let folder = self.loaded.as_ref().and_then(|l| l.tree.nodes.get(n)).map(|nd| nd.path.clone());
                    if let Some(folder) = folder {
                        if let Some(out) = cx.shell.pick_folder("export-folder-json") {
                            self.export_folder_json_to(&folder, out, cx);
                        }
                    }
                }
                Act::CompareAni(i) => self.open_compare(i, cx),
                Act::EditAni(i) => self.open_edit(i),
                Act::PlayAnimation(path) => self.play_animation_on_mesh(&path, cx),
                Act::ExportJson(i) => {
                    let name = self.loaded.as_ref().and_then(|l| l.rows.get(i)).map(|r| r.name().to_string());
                    if let Some(out) = name.and_then(|n| cx.shell.save_file("export-json", &crate::export_json::default_name(&n), &[("JSON", &["json"])])) {
                        self.export_json_row_to(i, out, cx);
                    }
                }
                Act::ExportGltf(i) => {
                    let name = self.loaded.as_ref().and_then(|l| l.rows.get(i)).map(|r| r.name().to_string());
                    cx.shell.status(crate::export_interchange::LOSSY_SENTENCE);
                    if let Some(out) = name.and_then(|n| cx.shell.save_file("export-gltf", &crate::export_interchange::default_name(&n), &[("glTF with a .bin beside it", &["gltf"]), ("glTF binary (one file)", &["glb"])])) {
                        self.export_gltf_row_to(i, out, cx);
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
                Act::CopyJson => {
                    let Some(l) = &self.loaded else { continue };
                    let items: Vec<serde_json::Value> = self
                        .selected
                        .iter()
                        .map(|&i| {
                            let r = &l.rows[i];
                            serde_json::json!({ "path": r.path, "size": r.entry.length, "offset": r.entry.offset, "version": r.entry.version, "shadowed": r.shadowed })
                        })
                        .collect();
                    cx.shell.ctx().copy_text(serde_json::to_string_pretty(&items).unwrap_or_default());
                    cx.shell.status("Copied");
                }
                Act::ShowInTree(i) => {
                    let Some(l) = &self.loaded else { continue };
                    let (folder, path) = (l.rows[i].folder().to_string(), l.rows[i].path.clone());
                    self.enter_folder(&folder);
                    self.refresh_view();
                    if let Some(i) = self.find_row(&path) {
                        self.selected.insert(i);
                    }
                }
                Act::ReplaceWith(i) => {
                    if let Some(f) = cx.shell.pick_file("replace", &[("All files", &["*"])]) {
                        self.replace_with_file(i, f, cx);
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
                Act::Compare => self.begin_compare(),
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
    ShowInTree,
    Replace,
    Remove,
    Decrypt,
    ExportJson,
    ExportGltf,
    ExportFolderJson,
    CompareWith,
    EditAnimation,
}

/// What the Archive tab asks the app frame to do on its behalf.
pub enum TabRequest {
    PatchFromPending,
    /// Compare: build a patch with OLD = `old` and NEW = `new` (the labels are the names shown to the user).
    PatchFromDiff { old: PathBuf, new: PathBuf, old_label: String, new_label: String },
    /// A button of the Start page.
    Start(crate::start::StartAction),
}

fn hex_view(ui: &mut egui::Ui, bytes: &[u8], base: u64) {
    let rows = preview::hex_rows(bytes.len());
    egui::ScrollArea::both().auto_shrink([false, false]).id_salt("toolkit-hex").show_rows(ui, 14.0, rows, |ui, range| {
        for r in range {
            ui.add(egui::Label::new(RichText::new(preview::hex_row(bytes, base, r)).monospace().size(11.0)).selectable(true));
        }
    });
}

/// Stores a panel width, ignoring sub-pixel jitter so a steady layout does not rewrite the setting every frame.
pub(crate) fn remember_width(slot: &mut f32, width: f32) {
    if width.is_finite() && (*slot - width).abs() >= 0.5 {
        *slot = width;
    }
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
    let id = ui.make_persistent_id(("toolkit-tree", &n.path));
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

/// A decoded picture over a checkerboard: Fit or 1:1 (and the alpha box when `alpha` is given). Used by the image and the map preview.
fn picture_view(ui: &mut egui::Ui, fit: &mut bool, alpha: Option<&mut bool>, tex: Option<&egui::TextureHandle>, orbit: Option<&mut crate::preview_controls::PreviewControls>) {
    ui.horizontal(|ui| {
        ui.selectable_value(fit, true, "Fit");
        ui.selectable_value(fit, false, "1:1");
        if let Some(a) = alpha {
            ui.checkbox(a, "alpha");
        }
    });
    let Some(tex) = tex else { return };
    let avail = ui.available_size();
    let size = tex.size_vec2();
    let scale = if *fit { (avail.x / size.x).min(avail.y.max(100.0) / size.y).min(8.0) } else { 1.0 };
    let dim = size * scale;
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        let sense = if orbit.is_some() { egui::Sense::click_and_drag() } else { egui::Sense::hover() };
        let (rect, resp) = ui.allocate_exact_size(dim, sense);
        let p = ui.painter().with_clip_rect(rect);
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
        // a zoom that waits for its new picture scales the one on screen about its centre
        let shown = orbit.as_ref().map_or(1.0, |o| o.pending_scale());
        let img_rect = if shown == 1.0 { rect } else { egui::Rect::from_center_size(rect.center(), rect.size() * shown) };
        p.image(tex.id(), img_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        if let Some(o) = orbit {
            o.input(ui, &resp);
        }
    });
}
