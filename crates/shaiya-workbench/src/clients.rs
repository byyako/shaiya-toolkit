//! Client folders in Workbench: the toolbar picker's data, the Add dialog, the Manage dialog and "Use another archive pair" (swap). The
//! list is the `clients` key of `shared.json`, shared with the command line (`shaiya clients`); every change goes through
//! `shaiya_core::config::update_clients`, so another program's changes are kept.

use crate::archive_view::ArchiveView;
use crate::WorkbenchSettings;
use egui::RichText;
use shaiya_archive::client::{read_version_ini, ClientFiles};
use shaiya_archive::swap::{swap, SwapReport};
use shaiya_archive::Archive;
use shaiya_core::config::{self, ClientFolder};
use shaiya_core::{Error, Result};
use shaiya_ui::{Client, Confirm, Cx, Shell, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type WorkbenchCx<'a> = Cx<'a, WorkbenchSettings>;

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
    if let Some(f) = ClientFiles::locate(dir).ok() {
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

enum SwapFlow {
    Idle,
    Confirm { client: String, dir: PathBuf, other: PathBuf },
    /// Waits until nothing holds the archive handle any more.
    Closing { client: String, dir: PathBuf, other: PathBuf, arc: Option<Arc<Archive>> },
    Running { client: String, dir: PathBuf, handle: TaskHandle<SwapReport> },
    Done { client: String, dir: PathBuf, kept: PathBuf },
}

/// What a frame of the dialogs asks the Workbench frame to do.
pub enum ClientsAction {
    /// Open this saved client.
    Open(String),
    /// A swap finished or failed: reload the open archive when it is this folder.
    Reload(PathBuf),
}

pub struct ClientsUi {
    pub add: Option<AddClient>,
    pub manage: bool,
    statuses: Vec<(String, ClientStatus)>,
    edit: Option<EditClient>,
    swap: SwapFlow,
}

impl Default for ClientsUi {
    fn default() -> Self {
        ClientsUi { add: None, manage: false, statuses: Vec::new(), edit: None, swap: SwapFlow::Idle }
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
        self.swap = SwapFlow::Confirm { client: client.to_string(), dir, other };
    }

    /// The text of the swap confirmation (prompt 6 of 7.8).
    fn swap_text(client: &str, other: &Path) -> String {
        let pair = other.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| other.display().to_string());
        format!("Put {pair} in place of {client}'s data.sah/data.saf? The current pair is kept as data.<stamp>.sah/saf in the same folder.")
    }

    /// Draws the Add and Manage dialogs and advances the swap. Returns what the frame must do.
    pub fn ui(&mut self, ctx: &egui::Context, cx: &mut WorkbenchCx<'_>, archive: &mut ArchiveView) -> Vec<ClientsAction> {
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
            let mut edit: Option<(String, EditKind)> = None;
            let mut swap_for: Option<(String, PathBuf)> = None;
            let clients = cx.shell.shared().clients.clone();
            let busy = !archive.pending.is_empty();
            egui::Window::new("Manage client folders").open(&mut open).default_width(720.0).show(ctx, |ui| {
                if clients.is_empty() {
                    ui.label("No client folders yet. Use Add client folder... in the toolbar.");
                }
                egui::Grid::new("workbench-clients-grid").striped(true).num_columns(4).show(ui, |ui| {
                    ui.label(RichText::new("Name").strong());
                    ui.label(RichText::new("Folder and status").strong());
                    ui.label(RichText::new("Note").strong());
                    ui.label("");
                    ui.end_row();
                    for c in &clients {
                        let st = self.statuses.iter().find(|(n, _)| n == &c.name).map(|(_, s)| s.clone()).unwrap_or_default();
                        ui.label(&c.name);
                        ui.vertical(|ui| {
                            ui.label(&c.path);
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
                            if ui.button("Rename...").clicked() {
                                edit = Some((c.name.clone(), EditKind::Rename));
                            }
                            if ui.button("Edit note...").clicked() {
                                edit = Some((c.name.clone(), EditKind::Note));
                            }
                            if ui.add_enabled(st.found && !busy, egui::Button::new("Use another archive pair...")).on_hover_text("Swap in another data.sah/data.saf; the current pair is kept").clicked() {
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
                    if ui.button("Swap back").clicked() {
                        swap_for = Some((client.clone(), kept.clone()));
                        let _ = dir;
                    }
                }
            });
            if !open {
                self.manage = false;
            }
            if let Some(n) = open_client {
                out.push(ClientsAction::Open(n));
            }
            if let Some(n) = remove {
                if let Err(e) = remove_client(cx.shell, &n) {
                    cx.shell.error("Cannot change the client list", &e);
                }
                self.refresh(cx.shell);
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
                        self.swap = SwapFlow::Confirm { client: n, dir, other: o };
                    }
                } else {
                    self.swap = SwapFlow::Confirm { client: n, dir, other: path };
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
                let r = match e.kind {
                    EditKind::Rename => rename_client(cx.shell, &e.name, e.text.trim()),
                    EditKind::Note => set_note(cx.shell, &e.name, &e.text),
                };
                match r {
                    Ok(()) => keep = false,
                    Err(err) => cx.shell.error("Cannot change the client list", &err),
                }
                self.refresh(cx.shell);
            }
            if keep {
                self.edit = Some(e);
            }
        }
        self.poll_swap(cx, archive, &mut out);
        out
    }

    fn poll_swap(&mut self, cx: &mut WorkbenchCx<'_>, archive: &mut ArchiveView, out: &mut Vec<ClientsAction>) {
        match std::mem::replace(&mut self.swap, SwapFlow::Idle) {
            SwapFlow::Idle => {}
            SwapFlow::Done { client, dir, kept } => self.swap = SwapFlow::Done { client, dir, kept },
            SwapFlow::Confirm { client, dir, other } => {
                let c = Confirm { title: "Swap archives".into(), text: Self::swap_text(&client, &other), buttons: vec!["Swap archives".into(), "Cancel".into()], danger: None };
                match cx.shell.confirm("workbench-swap", &c) {
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

/// A one-line name for a path in the picker.
pub fn picker_label(c: &Client, found: bool) -> String {
    if found {
        format!("{}  ({})", c.name, c.path)
    } else {
        format!("{}  ({}) - not found", c.name, c.path)
    }
}
