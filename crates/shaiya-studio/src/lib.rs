//! Shaiya Archive & Patch Studio (T1): the archive browser with queued changes, patch builder, apply with history, signing, SData
//! shortcuts and the T0 file inspector, on the shared GUI shell. Every change to a client folder goes through the same crash-safe
//! procedure as the command line (`shaiya_archive::edit`, `shaiya_patch::apply_with`), after a confirmation.

pub mod archive_view;
pub mod clients;
pub mod inspect;
pub mod preview;

use archive_view::{ArchiveView, OpenSource, TabRequest};
use clients::{ClientInfo, ClientsAction, ClientsUi};
use egui::{Key, Modifiers, RichText};
use inspect::InspectTab;
use serde::{Deserialize, Serialize};
use shaiya_ui::{AppInfo, Command, Confirm, Cx, Menu, ToolApp};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const HELP: &str = "Shaiya Archive & Patch Studio\n\nPick a client folder in the toolbar (Add client folder... remembers it for every toolkit program), or open any data.sah with Open other archive...\n\nArchive: browse, search and preview the files of the archive; extract them; queue changes (add, replace, remove) and apply them to the client with a backup. Nothing is written until you press Apply to client.\n\nPatch builder, Apply & history, Sign & verify: make, check, sign and apply patches; every applied change can be rolled back from Apply & history.\n\nSData: decrypt or encrypt SData files into a folder.\n\nInspect: identify files and compute SHA-256.\n\nKeys: Ctrl+1 to Ctrl+6 switch tabs, Ctrl+F search, Delete queues a removal, Insert adds files, Ctrl+E extracts, Ctrl+Enter applies, Ctrl+Z / Ctrl+Y undo and redo queued changes, F5 reloads. All actions are in the command palette (Ctrl+Shift+P).";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct KeySettings {
    pub public_key_path: String,
    pub public_hex: String,
    pub check_version: String,
    /// Only stored when "Remember where my secret key is" is ticked.
    pub secret_key_path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct StudioSettings {
    pub tab: String,
    pub last_client: Option<String>,
    pub tree_width: f32,
    pub preview_width: f32,
    /// Column widths of the Inspect tab: Name, Kind, Size, Details.
    pub columns: Vec<f32>,
    pub show_shadowed: bool,
    pub text_encoding: String,
    pub extract_keep_times: bool,
    /// Per client name: signature settings.
    pub keys: BTreeMap<String, KeySettings>,
}

impl Default for StudioSettings {
    fn default() -> Self {
        StudioSettings {
            tab: "archive".into(),
            last_client: None,
            tree_width: 220.0,
            preview_width: 360.0,
            columns: Vec::new(),
            show_shadowed: false,
            text_encoding: String::new(),
            extract_keep_times: true,
            keys: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Archive,
    Inspect,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Archive, Tab::Inspect];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Archive => "archive",
            Tab::Inspect => "inspect",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Archive => "Archive",
            Tab::Inspect => "Inspect",
        }
    }

    pub fn from_key(k: &str) -> Tab {
        Tab::ALL.iter().copied().find(|t| t.key() == k).unwrap_or(Tab::Archive)
    }
}

/// A dropped folder that holds a `data.sah`: what to do with it.
struct FolderPrompt {
    dir: PathBuf,
    archive_open: bool,
}

pub struct StudioApp {
    pub tab: Tab,
    pub archive: ArchiveView,
    pub inspect: InspectTab,
    pub clients: ClientsUi,
    /// The saved client (or opened folder) the tabs work on.
    pub current: Option<ClientInfo>,
    ini_stamp: Option<(PathBuf, u64)>,
    folder_prompt: Option<FolderPrompt>,
    was_focused: bool,
}

type StudioCx<'a> = Cx<'a, StudioSettings>;

fn has_sah(dir: &Path) -> bool {
    dir.is_dir() && std::fs::read_dir(dir).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or(false)
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// A `file:` URL of a folder, for "Show folder" (the same mechanism as Help > Open data folder).
pub fn file_url(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/").replace(' ', "%20");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

impl StudioApp {
    // ---------------------------------------------------------------------------------------- client handling

    /// Opens a saved client by name (a missing folder is reported, not opened).
    pub fn select_client(&mut self, name: &str, cx: &mut StudioCx<'_>) {
        if !self.archive.pending.is_empty() {
            cx.shell.status("Apply or discard the pending changes first");
            return;
        }
        let Some(c) = cx.shell.shared().clients.iter().find(|c| c.name.eq_ignore_ascii_case(name)).cloned() else {
            cx.shell.status(format!("No client named {name}"));
            return;
        };
        let dir = PathBuf::from(&c.path);
        if !has_sah(&dir) {
            cx.shell.status(format!("{} was not found at {}", c.name, c.path));
            return;
        }
        self.open_client_dir(&c.name, dir, cx);
        cx.settings.last_client = Some(c.name);
    }

    fn open_client_dir(&mut self, name: &str, dir: PathBuf, cx: &mut StudioCx<'_>) {
        let (check_version, current_version) = clients::version_numbers(&dir);
        self.current = Some(ClientInfo { name: name.to_string(), dir: dir.clone(), check_version, current_version });
        self.ini_stamp = None;
        self.archive.open(OpenSource::Client { name: name.to_string(), dir }, cx);
        self.set_tab(Tab::Archive, cx);
    }

    /// Opens any archive file read only (its own folder's changes are not offered).
    pub fn open_archive_file(&mut self, path: PathBuf, cx: &mut StudioCx<'_>) {
        if !self.archive.pending.is_empty() {
            cx.shell.status("Apply or discard the pending changes first");
            return;
        }
        self.current = None;
        cx.shell.add_recent(&path);
        self.archive.open(OpenSource::File(path), cx);
        self.set_tab(Tab::Archive, cx);
    }

    /// Adds a client folder to the shared list (and opens it when asked).
    pub fn add_client(&mut self, name: &str, path: &Path, open: bool, cx: &mut StudioCx<'_>) -> shaiya_core::Result<()> {
        clients::add_client(cx.shell, name, path)?;
        if open {
            self.select_client(name, cx);
        }
        Ok(())
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut StudioCx<'_>) {
        self.tab = tab;
        cx.settings.tab = tab.key().to_string();
    }

    /// Keeps the current client's `Version.ini` numbers fresh (read again after every change of the archive).
    fn refresh_ini(&mut self) {
        let Some(cur) = &mut self.current else { return };
        let stamp = (cur.dir.clone(), self.archive.changed_counter);
        if self.ini_stamp.as_ref() != Some(&stamp) {
            let (c, v) = clients::version_numbers(&cur.dir);
            cur.check_version = c;
            cur.current_version = v;
            self.ini_stamp = Some(stamp);
        }
    }

    fn handle_requests(&mut self, requests: Vec<TabRequest>, cx: &mut StudioCx<'_>) {
        for r in requests {
            match r {
                TabRequest::PatchFromPending | TabRequest::PatchFromDiff(_) => {
                    cx.shell.status("Patch builder is not available yet");
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------- drops

    fn dropped(&mut self, paths: Vec<PathBuf>, cx: &mut StudioCx<'_>) {
        let mut files: Vec<PathBuf> = Vec::new();
        for p in paths {
            if p.is_dir() && has_sah(&p) {
                self.folder_prompt = Some(FolderPrompt { dir: p, archive_open: self.archive.is_open() });
            } else if ext_of(&p) == "sah" {
                self.open_archive_file(p, cx);
            } else {
                files.push(p);
            }
        }
        if files.is_empty() {
            return;
        }
        if self.tab == Tab::Archive && self.archive.is_open() && self.archive.client_dir().is_some() {
            self.archive.begin_add(files);
        } else {
            self.set_tab(Tab::Inspect, cx);
            self.inspect.open(files, cx);
        }
    }

    fn folder_prompt_ui(&mut self, cx: &mut StudioCx<'_>) {
        let Some(p) = &self.folder_prompt else { return };
        let (dir, archive_open) = (p.dir.clone(), p.archive_open);
        let second = if archive_open { "Add its files" } else { "Remember as client" };
        let c = Confirm {
            title: "Dropped folder".into(),
            text: format!("{} holds a data.sah. What should Studio do with it?", dir.display()),
            buttons: vec!["Open this client".into(), second.into(), "Cancel".into()],
            danger: None,
        };
        match cx.shell.confirm("studio-drop-folder", &c) {
            None => {}
            Some(0) => {
                self.folder_prompt = None;
                if self.archive.pending.is_empty() {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "client".into());
                    let known = cx.shell.shared().clients.iter().find(|c| Path::new(&c.path) == dir).map(|c| c.name.clone());
                    match known {
                        Some(n) => self.select_client(&n, cx),
                        None => self.open_client_dir(&name, dir, cx),
                    }
                } else {
                    cx.shell.status("Apply or discard the pending changes first");
                }
            }
            Some(1) => {
                self.folder_prompt = None;
                if archive_open {
                    self.archive.begin_add(vec![dir]);
                } else {
                    self.clients.open_add(Some(&dir));
                }
            }
            Some(_) => self.folder_prompt = None,
        }
    }

    // ---------------------------------------------------------------------------------------- drawing

    fn toolbar(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>) {
        enum A {
            Select(String),
            Add,
            Other,
            Manage,
            Tab(Tab),
        }
        let mut act: Option<A> = None;
        let pal = cx.shell.pal();
        let clients: Vec<shaiya_ui::Client> = cx.shell.shared().clients.clone();
        let current = self.current.as_ref().map(|c| c.name.clone());
        let shown = match (&current, self.archive.source_name()) {
            (Some(n), _) => n.clone(),
            (None, Some(f)) => f,
            (None, None) => "(none)".to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("Client");
            egui::ComboBox::from_id_salt("studio-client").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                for c in &clients {
                    let found = has_sah(Path::new(&c.path));
                    let label = clients::picker_label(c, found);
                    let text = if found { RichText::new(label) } else { RichText::new(label).color(pal.weak) };
                    if ui.add_enabled(found, egui::Button::selectable(current.as_deref() == Some(c.name.as_str()), text)).clicked() {
                        act = Some(A::Select(c.name.clone()));
                        ui.close();
                    }
                }
                if !clients.is_empty() {
                    ui.separator();
                }
                if ui.button("Add client folder...").clicked() {
                    act = Some(A::Add);
                    ui.close();
                }
                if ui.button("Open other archive...").clicked() {
                    act = Some(A::Other);
                    ui.close();
                }
            });
            if ui.button("Manage...").clicked() {
                act = Some(A::Manage);
            }
            ui.separator();
            for t in Tab::ALL {
                if ui.selectable_label(self.tab == t, t.label()).clicked() {
                    act = Some(A::Tab(t));
                }
            }
        });
        match act {
            Some(A::Select(n)) => self.select_client(&n, cx),
            Some(A::Add) => {
                let picked = cx.shell.pick_folder("client");
                self.clients.open_add(picked.as_deref());
            }
            Some(A::Other) => {
                if let Some(p) = cx.shell.pick_file("open-archive", &[("SAH index", &["sah"])]) {
                    self.open_archive_file(p, cx);
                }
            }
            Some(A::Manage) => self.clients.open_manage(cx.shell),
            Some(A::Tab(t)) => self.set_tab(t, cx),
            None => {}
        }
    }

    fn tab_keys(&mut self, ctx: &egui::Context, cx: &mut StudioCx<'_>) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6];
        for (i, k) in keys.iter().enumerate() {
            if ctx.input(|inp| inp.modifiers.command && !inp.modifiers.shift && inp.key_pressed(*k)) {
                if let Some(t) = Tab::ALL.get(i) {
                    self.set_tab(*t, cx);
                }
            }
        }
    }
}

impl ToolApp for StudioApp {
    type Settings = StudioSettings;

    fn info() -> AppInfo {
        AppInfo {
            id: "shaiya-studio",
            name: "Shaiya Archive & Patch Studio",
            version: env!("CARGO_PKG_VERSION"),
            git: env!("SHAIYA_GIT_HASH"),
            target: env!("SHAIYA_TARGET"),
            icon_rgba: include_bytes!("../../../assets/icons/shaiya-studio-64.rgba"),
            icon_size: 64,
            open_filters: &[("Shaiya files", &["sah", "saf", "sdata", "patch", "sig", "exe"]), ("All files", &["*"])],
            help: HELP,
        }
    }

    fn new(cx: &mut StudioCx<'_>) -> StudioApp {
        let mut app = StudioApp {
            tab: Tab::from_key(&cx.settings.tab),
            archive: ArchiveView::new(),
            inspect: InspectTab::new(),
            clients: ClientsUi::default(),
            current: None,
            ini_stamp: None,
            folder_prompt: None,
            was_focused: true,
        };
        app.archive.show_shadowed = cx.settings.show_shadowed;
        // the last used client opens at start, as a task; a client whose folder is gone is not opened
        if let Some(name) = cx.settings.last_client.clone() {
            app.select_client(&name, cx);
        }
        app
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>) {
        let ctx = ui.ctx().clone();
        // the client list is re-read when the window gains focus (other toolkit programs may have changed it)
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if focused && !self.was_focused {
            clients::reload(cx.shell);
        }
        self.was_focused = focused;
        self.refresh_ini();
        self.toolbar(ui, cx);
        ui.separator();
        self.tab_keys(&ctx, cx);
        let mut requests = Vec::new();
        match self.tab {
            Tab::Archive => requests = self.archive.ui(ui, cx),
            Tab::Inspect => self.inspect.ui(ui, cx),
        }
        // the archive keeps polling its tasks while another tab is shown
        if self.tab != Tab::Archive {
            self.archive.poll(cx);
        }
        let actions = self.clients.ui(&ctx, cx, &mut self.archive);
        for a in actions {
            match a {
                ClientsAction::Open(n) => self.select_client(&n, cx),
                ClientsAction::Reload(dir) => {
                    if self.archive.client_dir() == Some(dir.as_path()) || self.archive.source_dir_is(&dir) {
                        self.archive.reopen(cx);
                    }
                }
            }
        }
        self.folder_prompt_ui(cx);
        self.handle_requests(requests, cx);
    }

    fn open(&mut self, paths: Vec<PathBuf>, cx: &mut StudioCx<'_>) {
        self.dropped(paths, cx);
    }

    fn menu(&mut self, menu: Menu, ui: &mut egui::Ui, cx: &mut StudioCx<'_>) {
        match menu {
            Menu::File => {
                ui.separator();
                if ui.button("Add client folder...").clicked() {
                    ui.close();
                    let picked = cx.shell.pick_folder("client");
                    self.clients.open_add(picked.as_deref());
                }
                if ui.button("Manage client folders...").clicked() {
                    ui.close();
                    self.clients.open_manage(cx.shell);
                }
            }
            Menu::Edit => {
                ui.separator();
                if ui.add_enabled(!self.inspect.is_empty(), egui::Button::new("Hash selected")).clicked() {
                    ui.close();
                    self.inspect.hash_selected(cx);
                }
            }
            _ => {}
        }
    }

    fn commands(&self) -> Vec<Command> {
        let mut v: Vec<Command> = Vec::new();
        for (i, t) in Tab::ALL.iter().enumerate() {
            v.push(Command {
                id: match t {
                    Tab::Archive => "tab-archive",
                    Tab::Inspect => "tab-inspect",
                },
                label: format!("Go to {}", t.label()),
                shortcut: Some(egui::KeyboardShortcut::new(Modifiers::COMMAND, [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6][i])),
                enabled: true,
            });
        }
        v.extend(self.archive.commands());
        v.push(Command { id: "hash-selected", label: "Hash selected files".into(), shortcut: None, enabled: !self.inspect.is_empty() });
        v
    }

    fn run_command(&mut self, id: &str, cx: &mut StudioCx<'_>) {
        match id {
            "tab-archive" => self.set_tab(Tab::Archive, cx),
            "tab-inspect" => self.set_tab(Tab::Inspect, cx),
            "hash-selected" => self.inspect.hash_selected(cx),
            other => {
                self.set_tab(Tab::Archive, cx);
                let requests = self.archive.run_command(other, cx);
                self.handle_requests(requests, cx);
            }
        }
    }

    fn undo(&mut self, _cx: &mut StudioCx<'_>) -> bool {
        self.tab == Tab::Archive && self.archive.undo()
    }

    fn redo(&mut self, _cx: &mut StudioCx<'_>) -> bool {
        self.tab == Tab::Archive && self.archive.redo()
    }

    fn unsaved(&self) -> Vec<String> {
        self.archive.unsaved()
    }

    fn save_all(&mut self, cx: &mut StudioCx<'_>) -> Result<(), String> {
        if self.archive.unsaved().is_empty() {
            return Ok(());
        }
        self.set_tab(Tab::Archive, cx);
        Err("apply or discard the pending changes in the Archive tab".to_string())
    }

    fn prefs(&mut self, ui: &mut egui::Ui, cx: &mut StudioCx<'_>) {
        ui.checkbox(&mut cx.settings.extract_keep_times, "Extracted files keep the archive's pack time");
        ui.label("Inspect tab column widths");
        let mut cols = inspect::columns(cx.settings);
        for (i, name) in ["Name", "Kind", "Size", "Details"].iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(*name);
                ui.add(egui::DragValue::new(&mut cols[i]).range(40.0..=2000.0).suffix(" px"));
            });
        }
        cx.settings.columns = cols.to_vec();
        if ui.button("Reset columns").clicked() {
            cx.settings.columns.clear();
        }
    }
}
