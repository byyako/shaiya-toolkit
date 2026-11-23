//! Client folders in Shaiya Toolkit: the toolbar picker's data, the Add dialog, the Manage dialog and "Use another archive pair" (swap). The
//! list is the `clients` key of `shared.json`, shared with the command line (`shaiya clients`); every change goes through
//! `shaiya_core::config::update_clients`, so another program's changes are kept.

use crate::archive_view::ArchiveView;
use crate::explain::{self, Rows};
use crate::multi_lock::lock_hover;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_archive::client::{read_version_ini, ClientFiles};
use shaiya_archive::swap::{swap, SwapReport};
use shaiya_archive::Archive;
use shaiya_core::config::{self, ClientFolder};
use shaiya_core::{Error, Result};
use shaiya_ui::{Client, Confirm, Cx, Shell, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// The client the tabs work on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub dir: PathBuf,
    pub check_version: Option<u32>,
    pub current_version: Option<u32>,
}

/// What the Manage dialog shows about one folder.
#[derive(Clone, Debug, Default)]
pub struct ClientStatus {
    pub found: bool,
    pub sah_version: Option<i32>,
    pub files: usize,
    pub saf_present: bool,
    pub check_version: Option<u32>,
    pub current_version: Option<u32>,
    pub problem: Option<String>,
}

/// Reads the index and `Version.ini` of a client folder (index only; fast).
pub fn status_of(dir: &Path) -> ClientStatus {
    let mut s = ClientStatus::default();
    if !dir.is_dir() {
        s.problem = Some("folder not found".into());
        return s;
    }
    match Archive::open_index(dir) {
        Ok((sah, index)) => {
            s.found = true;
            s.sah_version = Some(index.version);
            s.files = index.file_count();
            s.saf_present = shaiya_archive::saf_of(&sah).is_file();
        }
        Err(e) => s.problem = Some(e.to_string()),
    }
    if let Ok(f) = ClientFiles::locate(dir) {
        if let Some(v) = read_version_ini(&f) {
            s.check_version = v.check_version;
            s.current_version = v.current_version;
        }
    }
    s
}

/// `Version.ini` numbers of a folder, when it has a pair and the file.
pub fn version_numbers(dir: &Path) -> (Option<u32>, Option<u32>) {
    match ClientFiles::locate(dir).ok().and_then(|f| read_version_ini(&f)) {
        Some(v) => (v.check_version, v.current_version),
        None => (None, None),
    }
}

fn to_ui(c: &ClientFolder) -> Client {
    Client { name: c.name.clone(), path: c.path.display().to_string(), added_utc: c.added_utc.clone(), note: c.note.clone() }
}

fn from_ui(c: &Client) -> ClientFolder {
    ClientFolder { name: c.name.clone(), path: PathBuf::from(&c.path), added_utc: c.added_utc.clone(), note: c.note.clone() }
}

/// The toolkit data folder (the parent of the tool's own folder), when settings are stored.
pub fn data_root(shell: &Shell) -> Option<PathBuf> {
    shell.data_dir().and_then(|d| d.parent()).map(Path::to_path_buf)
}

/// Changes the list through the shared read-modify-write and mirrors the result into the shell.
pub fn update(shell: &mut Shell, f: &mut dyn FnMut(&mut Vec<ClientFolder>) -> Result<()>) -> Result<()> {
    let list = match data_root(shell) {
        Some(root) => config::update_clients(&root, f)?,
        None => {
            let mut list: Vec<ClientFolder> = shell.shared().clients.iter().map(from_ui).collect();
            f(&mut list)?;
            list
        }
    };
    shell.shared_mut().clients = list.iter().map(to_ui).collect();
    Ok(())
}

/// Adds a client folder: the name per `validate_client_name`, the folder must hold `data.sah`.
pub fn add_client(shell: &mut Shell, name: &str, path: &Path) -> Result<()> {
    config::validate_client_name(name)?;
    Archive::open_index(path)?;
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let name = name.to_string();
    update(shell, &mut |list| {
        if config::find_client(list, &name).is_some() {
            return Err(Error::Refused(format!("there is already a client named {name:?}")));
        }
        list.push(ClientFolder { name: name.clone(), path: abs.clone(), added_utc: shaiya_core::time::utc_iso(std::time::SystemTime::now()), note: String::new() });
        Ok(())
    })
}

pub fn remove_client(shell: &mut Shell, name: &str) -> Result<()> {
    update(shell, &mut |list| {
        list.retain(|c| !c.name.eq_ignore_ascii_case(name));
        Ok(())
    })
}

pub fn rename_client(shell: &mut Shell, name: &str, new_name: &str) -> Result<()> {
    config::validate_client_name(new_name)?;
    update(shell, &mut |list| {
        if !name.eq_ignore_ascii_case(new_name) && config::find_client(list, new_name).is_some() {
            return Err(Error::Refused(format!("there is already a client named {new_name:?}")));
        }
        if let Some(c) = list.iter_mut().find(|c| c.name.eq_ignore_ascii_case(name)) {
            c.name = new_name.to_string();
        }
        Ok(())
    })
}

pub fn set_note(shell: &mut Shell, name: &str, note: &str) -> Result<()> {
    update(shell, &mut |list| {
        if let Some(c) = list.iter_mut().find(|c| c.name.eq_ignore_ascii_case(name)) {
            c.note = note.to_string();
        }
        Ok(())
    })
}

/// Re-reads the list from disk (when the window gains focus).
pub fn reload(shell: &mut Shell) {
    if let Some(root) = data_root(shell) {
        if let Ok(list) = config::load_clients(&root) {
            let ui: Vec<Client> = list.iter().map(to_ui).collect();
            if ui != shell.shared().clients {
                shell.shared_mut().clients = ui;
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ dialogs

pub struct AddClient {
    pub name: String,
    pub path: String,
    pub error: Option<String>,
    /// Open it right after adding.
    pub open_after: bool,
}

enum EditKind {
    Rename,
    Note,
}

struct EditClient {
    name: String,
    kind: EditKind,
    text: String,
}

/// The hover text of "Use another archive pair..." while the client's folder has queued changes.
pub const SWAP_BLOCKED_HOVER: &str = "Apply or discard this client's queued changes first";

/// The names of the other saved clients (not `name`) whose folder is `dir`, in the saved list's order.
fn other_clients_on(list: &[Client], name: &str, dir: &Path) -> Vec<String> {
    list.iter().filter(|c| !c.name.eq_ignore_ascii_case(name) && same_folder(Path::new(&c.path), dir)).map(|c| c.name.clone()).collect()
}

/// The queued changes of the folder `dir`, whichever saved client (or opened folder) they belong to.
pub(crate) fn queued_in(queued: &[(String, PathBuf, usize)], dir: &Path) -> usize {
    queued.iter().find(|(_, d, _)| same_folder(d, dir)).map(|(_, _, c)| *c).unwrap_or(0)
}

/// Whether two folders are the same place (compared as absolute paths, resolved when they exist; on Windows the case does not matter).
pub(crate) fn same_folder(a: &Path, b: &Path) -> bool {
    // (path, whether it was resolved on disk); a resolved path already has the on-disk case and is compared exactly
    let norm = |p: &Path| match canonical_cached(p) {
        Some(c) => (c, true),
        None => (std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()), false),
    };
    let ((a, a_real), (b, b_real)) = (norm(a), norm(b));
    if a_real && b_real {
        a == b
    } else if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `std::fs::canonicalize` with a small process-wide memo of successes (the callers run every frame, and the drive may be a network share).
/// Failures are not kept: a missing folder may appear later.
fn canonical_cached(p: &Path) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(c) = memo.lock().ok().and_then(|m| m.get(p).cloned()) {
        return Some(c);
    }
    let c = std::fs::canonicalize(p).ok()?;
    if let Ok(mut m) = memo.lock() {
        if m.len() > 256 {
            m.clear();
        }
        m.insert(p.to_path_buf(), c.clone());
    }
    Some(c)
}

/// A removal or rename waiting for the answer about the queue.
struct DropAsk {
    name: String,
    /// The client's folder: the queue belongs to the folder, not to the name.
    dir: PathBuf,
    queued: usize,
    /// `None`: remove from the list; `Some(new)`: rename.
    rename_to: Option<String>,
}

enum SwapFlow {
    Idle,
    Confirm { client: String, dir: PathBuf, other: PathBuf },
    /// Waits until nothing holds the archive handle any more.
    Closing { client: String, dir: PathBuf, other: PathBuf, arc: Option<Arc<Archive>> },
    Running { client: String, dir: PathBuf, handle: TaskHandle<SwapReport> },
    Done { client: String, dir: PathBuf, kept: PathBuf },
}

/// What a frame of the dialogs asks the app frame to do.
pub enum ClientsAction {
    /// Open this saved client.
    Open(String),
    /// Open the Health window for this saved client.
    Health(String),
    /// A swap finished or failed: reload the open archive when it is this folder.
    Reload(PathBuf),
    /// A client was removed from the list or renamed: drop the queued changes of its folder.
    DropQueue(PathBuf),
    /// A client was removed or renamed while another saved client stays on its folder: the queue of that folder is kept and, where its
    /// owner name was `old_name` (the open client's name or the stashed queue's name), the owner becomes `new_name`.
    QueueOwner { dir: PathBuf, old_name: String, new_name: String },
}

pub struct ClientsUi {
    pub add: Option<AddClient>,
    pub manage: bool,
    statuses: Vec<(String, ClientStatus)>,
    edit: Option<EditClient>,
    swap: SwapFlow,
    /// Folders with queued changes: (client name, folder, how many). Set by the app every frame.
    pub queued: Vec<(String, PathBuf, usize)>,
    /// Removing or renaming a client that has queued changes asks first.
    drop_ask: Option<DropAsk>,
    /// The Changes / Writes / Undo / Game rows of the open swap confirmation (made when the swap was asked for).
    swap_rows: Rows,
    /// True while "Apply to several test clients" runs (set by the app every frame): "Use another archive pair..." is off.
    pub multi_running: bool,
    /// Actions made outside a frame of the dialogs (a removal or rename that needed no question); handed out by the next `ui`.
    later: Vec<ClientsAction>,
}

impl Default for ClientsUi {
    fn default() -> Self {
        ClientsUi { add: None, manage: false, statuses: Vec::new(), edit: None, swap: SwapFlow::Idle, queued: Vec::new(), drop_ask: None, swap_rows: Vec::new(), multi_running: false, later: Vec::new() }
    }
}

impl ClientsUi {
    pub fn open_add(&mut self, path: Option<&Path>) {
        let name = path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.add = Some(AddClient { name, path: path.map(|p| p.display().to_string()).unwrap_or_default(), error: None, open_after: true });
    }

    pub fn open_manage(&mut self, shell: &Shell) {
        self.manage = true;
        self.refresh(shell);
    }

    fn refresh(&mut self, shell: &Shell) {
        self.statuses = shell.shared().clients.iter().map(|c| (c.name.clone(), status_of(Path::new(&c.path)))).collect();
    }

    pub fn is_swapping(&self) -> bool {
        !matches!(self.swap, SwapFlow::Idle | SwapFlow::Done { .. })
    }

    /// Starts "Use another archive pair" for a saved client (the test entry point; the dialog's button calls it too).
    pub fn begin_swap(&mut self, client: &str, dir: PathBuf, other: PathBuf) {
        self.ask_swap(client.to_string(), dir, other);
    }

    /// False while a multi-client apply runs: "Use another archive pair..." is off then.
    pub fn write_actions_enabled(&self) -> bool {
        !self.multi_running
    }

    fn ask_swap(&mut self, client: String, dir: PathBuf, other: PathBuf) {
        if !self.write_actions_enabled() {
            return;
        }
        self.swap_rows = match ClientFiles::locate(&dir) {
            Ok(files) => explain::swap_rows(&files, &other),
            Err(_) => Vec::new(),
        };
        self.swap = SwapFlow::Confirm { client, dir, other };
    }

    /// The text of the swap confirmation (prompt 6 of 7.8).
    fn swap_text(client: &str, other: &Path) -> String {
        let pair = other.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| other.display().to_string());
        format!("Put {pair} in place of {client}'s data.sah/data.saf?")
    }

    /// Draws the Add and Manage dialogs and advances the swap. Returns what the frame must do.
    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView) -> Vec<ClientsAction> {
        let mut out = Vec::new();
        let pal = cx.shell.pal();
        // Add dialog
        if let Some(mut d) = self.add.take() {
            let mut keep = true;
            let mut submit = false;
            egui::Window::new("Add client folder").collapsible(false).resizable(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Folder");
                    ui.add(egui::TextEdit::singleline(&mut d.path).desired_width(360.0));
                    if ui.button("Browse...").clicked() {
                        if let Some(p) = cx.shell.pick_folder("client") {
                            if d.name.trim().is_empty() {
                                d.name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            }
                            d.path = p.display().to_string();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(200.0));
                    ui.checkbox(&mut d.open_after, "Open it");
                });
                if let Some(e) = &d.error {
                    ui.label(RichText::new(e).color(pal.error));
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.path.trim().is_empty() && !d.name.trim().is_empty(), egui::Button::new("Add client")).clicked() {
                        submit = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
            if submit {
                match add_client(cx.shell, d.name.trim(), Path::new(d.path.trim())) {
                    Ok(()) => {
                        keep = false;
                        cx.shell.status(format!("Added client {}", d.name.trim()));
                        if d.open_after {
                            out.push(ClientsAction::Open(d.name.trim().to_string()));
                        }
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
            }
            if keep {
                self.add = Some(d);
            }
        }
        // Manage dialog
        if self.manage {
            let mut open = true;
            let mut remove: Option<String> = None;
            let mut open_client: Option<String> = None;
            let mut health_for: Option<String> = None;
            let mut edit: Option<(String, EditKind)> = None;
            let mut swap_for: Option<(String, PathBuf)> = None;
            let clients = cx.shell.shared().clients.clone();
            let shown: Vec<String> = clients.iter().map(|c| cx.shell.folder_text(&c.name, Path::new(&c.path))).collect();
            let queued = self.queued.clone();
            let queued_of = move |dir: &str| queued_in(&queued, Path::new(dir));
            let multi_locked = !self.write_actions_enabled();
            egui::Window::new("Manage client folders").open(&mut open).default_width(720.0).show(ctx, |ui| {
                if clients.is_empty() {
                    ui.label("No client folders yet. Use Add client folder... in the toolbar.");
                }
                egui::Grid::new("toolkit-clients-grid").striped(true).num_columns(4).show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Folder and status").strong());
                    ui.label(RichText::new("Note").strong());
                    ui.label("");
                    ui.end_row();
                    for (c, shown_path) in clients.iter().zip(&shown) {
                        let st = self.statuses.iter().find(|(n, _)| n == &c.name).map(|(_, s)| s.clone()).unwrap_or_default();
                        ui.label(&c.name);
                        ui.vertical(|ui| {
                            ui.label(shown_path);
                            if st.found {
                                let ini = match (st.check_version, st.current_version) {
                                    (Some(a), Some(b)) => format!(", Version.ini CheckVersion {a}, CurrentVersion {b}"),
                                    (Some(a), None) => format!(", Version.ini CheckVersion {a}"),
                                    (None, Some(b)) => format!(", Version.ini CurrentVersion {b}"),
                                    _ => String::new(),
                                };
                                let saf = if st.saf_present { "" } else { ", data.saf missing" };
                                ui.label(RichText::new(format!("found, SAH v{}, {} {}{ini}{saf}", st.sah_version.unwrap_or(0), shaiya_inspect::group(st.files as u64), if st.files == 1 { "file" } else { "files" })).color(pal.weak));
                            } else {
                                ui.label(RichText::new(format!("not found{}", st.problem.map(|p| format!(": {p}")).unwrap_or_default())).color(pal.error));
                            }
                        });
                        ui.label(&c.note);
                        ui.horizontal(|ui| {
                            if ui.add_enabled(st.found, egui::Button::new("Open")).clicked() {
                                open_client = Some(c.name.clone());
                            }
                            if ui.add_enabled(st.found, egui::Button::new("Health...")).on_hover_text("Look at leftovers, dead space and unfinished changes; nothing is changed until you confirm a clean-up").clicked() {
                                health_for = Some(c.name.clone());
                            }
                            if ui.button("Rename...").clicked() {
                                edit = Some((c.name.clone(), EditKind::Rename));
                            }
                            if ui.button("Edit note...").clicked() {
                                edit = Some((c.name.clone(), EditKind::Note));
                            }
                            let swap_btn = ui.add_enabled(st.found && queued_of(&c.path) == 0 && !multi_locked, egui::Button::new("Use another archive pair...")).on_hover_text(if queued_of(&c.path) > 0 { SWAP_BLOCKED_HOVER } else { "Swap in another data.sah/data.saf; the current pair is kept" });
                            if lock_hover(swap_btn, multi_locked).clicked() {
                                swap_for = Some((c.name.clone(), PathBuf::from(&c.path)));
                            }
                            if ui.button("Remove from list").on_hover_text("The files are not touched").clicked() {
                                remove = Some(c.name.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
                if let SwapFlow::Done { client, dir, kept } = &self.swap {
                    ui.separator();
                    ui.label(format!("{client}: the previous pair is kept as {}", kept.display()));
                    if lock_hover(ui.add_enabled(!multi_locked, egui::Button::new("Swap back")), multi_locked).clicked() {
                        swap_for = Some((client.clone(), kept.clone()));
                        let _ = dir;
                    }
                }
            });
            if !open {
                self.manage = false;
            }
            if let Some(n) = health_for {
                out.push(ClientsAction::Health(n));
            }
            if let Some(n) = open_client {
                out.push(ClientsAction::Open(n));
            }
            if let Some(n) = remove {
                self.begin_remove(&n, cx);
            }
            if let Some((n, kind)) = edit {
                let text = match kind {
                    EditKind::Rename => n.clone(),
                    EditKind::Note => clients.iter().find(|c| c.name == n).map(|c| c.note.clone()).unwrap_or_default(),
                };
                self.edit = Some(EditClient { name: n, kind, text });
            }
            if let Some((n, path)) = swap_for {
                // either a saved client's own "Use another archive pair" (pick the other pair) or "Swap back" (the kept pair is known)
                let dir = clients.iter().find(|c| c.name == n).map(|c| PathBuf::from(&c.path)).unwrap_or_default();
                if path == dir {
                    let other = cx.shell.pick_file("swap", &[("SAH index", &["sah"])]).or_else(|| cx.shell.pick_folder("swap"));
                    if let Some(o) = other {
                        self.ask_swap(n, dir, o);
                    }
                } else {
                    self.ask_swap(n, dir, path);
                }
            }
        }
        // rename / note
        if let Some(mut e) = self.edit.take() {
            let mut keep = true;
            let mut apply = false;
            let title = match e.kind {
                EditKind::Rename => "Rename client",
                EditKind::Note => "Edit note",
            };
            egui::Window::new(title).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.add(egui::TextEdit::singleline(&mut e.text).desired_width(320.0));
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
            if apply {
                match e.kind {
                    EditKind::Rename => {
                        keep = !self.begin_rename(&e.name, e.text.trim(), cx);
                    }
                    EditKind::Note => {
                        match set_note(cx.shell, &e.name, &e.text) {
                            Ok(()) => keep = false,
                            Err(err) => cx.shell.error("Cannot change the client list", &err),
                        }
                        self.refresh(cx.shell);
                    }
                }
            }
            if keep {
                self.edit = Some(e);
            }
        }
        self.poll_drop_ask(cx, &mut out);
        self.poll_swap(cx, archive, &mut out);
        out.append(&mut self.later);
        out
    }

    /// True while the folder has queued changes of any client (the active one's or a stashed one's): the archive pair cannot be swapped then.
    pub fn swap_blocked(&self, dir: &Path) -> bool {
        queued_in(&self.queued, dir) > 0
    }

    /// The queued changes of the folder of the saved client `name` (the queue belongs to the folder), with that folder.
    fn queued_of(&self, name: &str, cx: &ToolkitCx<'_>) -> (PathBuf, usize) {
        let Some(c) = cx.shell.shared().clients.iter().find(|c| c.name.eq_ignore_ascii_case(name)) else { return (PathBuf::new(), 0) };
        let dir = PathBuf::from(&c.path);
        let n = queued_in(&self.queued, &dir);
        (dir, n)
    }

    /// Remove from the list: at once, or after the question when the client has queued changes and no other saved client stays on its
    /// folder. With another saved client on the folder the queue is kept and that client (the first by the list's order) owns it.
    pub fn begin_remove(&mut self, name: &str, cx: &mut ToolkitCx<'_>) {
        let (dir, q) = self.queued_of(name, cx);
        let heirs = other_clients_on(cx.shell.shared().clients.as_slice(), name, &dir);
        if q > 0 && heirs.is_empty() {
            self.drop_ask = Some(DropAsk { name: name.to_string(), dir, queued: q, rename_to: None });
        } else {
            match remove_client(cx.shell, name) {
                Ok(()) => {
                    if q > 0 {
                        self.later.push(ClientsAction::QueueOwner { dir, old_name: name.to_string(), new_name: heirs[0].clone() });
                    }
                }
                Err(e) => cx.shell.error("Cannot change the client list", &e),
            }
            self.refresh(cx.shell);
        }
    }

    /// Rename: at once, or after the question when the client has queued changes and no other saved client stays on its folder (then the
    /// queue is kept; its owner name follows the rename where it was the old name). False when the new name was refused.
    pub fn begin_rename(&mut self, name: &str, new_name: &str, cx: &mut ToolkitCx<'_>) -> bool {
        let (dir, q) = self.queued_of(name, cx);
        let shared = !other_clients_on(cx.shell.shared().clients.as_slice(), name, &dir).is_empty();
        if q > 0 && new_name != name && !shared {
            self.drop_ask = Some(DropAsk { name: name.to_string(), dir, queued: q, rename_to: Some(new_name.to_string()) });
            return true;
        }
        let r = rename_client(cx.shell, name, new_name);
        match &r {
            Ok(()) => {
                if q > 0 && new_name != name {
                    self.later.push(ClientsAction::QueueOwner { dir, old_name: name.to_string(), new_name: new_name.to_string() });
                }
            }
            Err(e) => cx.shell.error("Cannot change the client list", e),
        }
        self.refresh(cx.shell);
        r.is_ok()
    }

    /// Asks before a removal or rename throws away the queued changes of that client.
    fn poll_drop_ask(&mut self, cx: &mut ToolkitCx<'_>, out: &mut Vec<ClientsAction>) {
        let Some(a) = self.drop_ask.take() else { return };
        let what = if a.rename_to.is_some() { "Renaming" } else { "Removing" };
        let c = Confirm {
            title: "Queued changes".into(),
            text: format!("{} has {} that were not applied. {what} it drops them (the files of the client are not touched). Drop them and continue?", a.name, shaiya_inspect::counted(a.queued as u64, "queued change", "queued changes")),
            buttons: vec![if a.rename_to.is_some() { "Drop them and rename".into() } else { "Drop them and remove".into() }, "Cancel".into()],
            danger: Some(0),
            ..Default::default()
        };
        match cx.shell.confirm("toolkit-drop-queue", &c) {
            None => self.drop_ask = Some(a),
            Some(0) => {
                let r = match &a.rename_to {
                    Some(new) => rename_client(cx.shell, &a.name, new),
                    None => remove_client(cx.shell, &a.name),
                };
                match r {
                    Ok(()) => out.push(ClientsAction::DropQueue(a.dir.clone())),
                    Err(e) => cx.shell.error("Cannot change the client list", &e),
                }
                self.refresh(cx.shell);
            }
            Some(_) => {}
        }
    }

    fn poll_swap(&mut self, cx: &mut ToolkitCx<'_>, archive: &mut ArchiveView, out: &mut Vec<ClientsAction>) {
        match std::mem::replace(&mut self.swap, SwapFlow::Idle) {
            SwapFlow::Idle => {}
            SwapFlow::Done { client, dir, kept } => self.swap = SwapFlow::Done { client, dir, kept },
            SwapFlow::Confirm { client, dir, other } => {
                let c = Confirm { title: "Swap archives".into(), text: Self::swap_text(&client, &other), buttons: vec!["Swap archives".into(), "Cancel".into()], danger: None, details: self.swap_rows.clone() };
                match cx.shell.confirm("toolkit-swap", &c) {
                    None => self.swap = SwapFlow::Confirm { client, dir, other },
                    Some(0) => {
                        let arc = if archive.client_dir() == Some(dir.as_path()) { archive.release_handle() } else { None };
                        self.swap = SwapFlow::Closing { client, dir, other, arc };
                    }
                    Some(_) => {}
                }
            }
            SwapFlow::Closing { client, dir, other, arc } => {
                if arc.as_ref().map(|a| Arc::strong_count(a) == 1).unwrap_or(true) {
                    drop(arc);
                    let (d2, o2) = (dir.clone(), other.clone());
                    let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Swapping archives of {client}"), cancellable: false, exclusive: Some(dir.display().to_string()) }, move |job| swap(&d2, &o2, job));
                    self.swap = SwapFlow::Running { client, dir, handle };
                } else {
                    cx.shell.ctx().request_repaint();
                    self.swap = SwapFlow::Closing { client, dir, other, arc };
                }
            }
            SwapFlow::Running { client, dir, mut handle } => match handle.poll() {
                None => self.swap = SwapFlow::Running { client, dir, handle },
                Some(TaskOutcome::Done(r)) => {
                    cx.shell.status(format!("{client}: archives swapped"));
                    self.swap = SwapFlow::Done { client, dir: dir.clone(), kept: r.kept_as.0 };
                    out.push(ClientsAction::Reload(dir));
                    self.refresh(cx.shell);
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("The archives were not swapped", &e);
                    out.push(ClientsAction::Reload(dir));
                }
                Some(_) => out.push(ClientsAction::Reload(dir)),
            },
        }
    }
}

/// The picker line with the number of queued changes appended ("US test - 3 queued"); unchanged when nothing is queued.
pub fn queued_label(label: String, queued: usize) -> String {
    if queued == 0 {
        label
    } else {
        format!("{label} - {queued} queued")
    }
}

/// A one-line name for a client in the picker: its name and where it is. `shown` is the folder as `Shell::folder_text` gives it (the
/// path, or under `--shot` the name, so that the name is not repeated).
pub fn picker_label(c: &Client, shown: &str, found: bool) -> String {
    let line = if shown == c.name { c.name.clone() } else { format!("{}  ({shown})", c.name) };
    if found {
        line
    } else {
        format!("{line} - not found")
    }
}
