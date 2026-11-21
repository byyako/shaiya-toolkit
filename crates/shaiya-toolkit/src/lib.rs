//! Shaiya Toolkit: the archive browser with queued changes, patch builder, apply with history, signing, SData
//! shortcuts and the file inspector, on the shared GUI shell. Every change to a client folder goes through the same crash-safe
//! procedure as the command line (`shaiya_archive::edit`, `shaiya_patch::apply_with`), after a confirmation.

pub mod apply_tab;
pub mod archive_view;
pub mod clients;
pub mod explain;
pub mod health;
pub mod inspect;
pub mod keys;
pub mod multi_apply;
pub mod patch_builder;
pub mod preview;
pub mod publish_form;
pub mod sdata_tab;
pub mod sdata_table;
pub mod shot;
pub mod sign_tab;
pub mod start;

use apply_tab::ApplyTab;
use archive_view::{ArchiveView, OpenSource, Queue, TabRequest};
use multi_apply::MultiApply;
use start::StartAction;
use clients::{ClientInfo, ClientsAction, ClientsUi};
use health::HealthAction;
use egui::{Key, Modifiers, RichText};
use inspect::InspectTab;
use keys::KeyForm;
use patch_builder::{PatchBuilder, PatchRequest};
use sdata_tab::SDataTab;
use sdata_table::TableView as SDataTableView;
use sign_tab::{Section, SignTab};
use serde::{Deserialize, Serialize};
use shaiya_ui::{AppInfo, Command, Confirm, Cx, Menu, ToolApp};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where Help > Support the project goes (opened in the default browser on click, never automatically).
pub const SUPPORT_URL: &str = "https://buymeacoffee.com/tylernol";
const HELP: &str = "Shaiya Toolkit\n\nPick a client folder in the toolbar (Add client folder... remembers it for every toolkit program), or open any data.sah with Open other archive...\n\nArchive: browse, search and preview the files of the archive; extract them; queue changes (add, replace, remove) and apply them to the client with a backup. Nothing is written until you press Apply to client.\n\nPatch builder, Apply & history, Sign & verify: make, check, sign and apply patches; changes journaled in the client folder can usually be rolled back from Apply & history (not after Compact data.saf, which makes no journal and ends rollback of earlier changes).\n\nSData: decrypt or encrypt SData files into a folder.\n\nInspect: identify files and compute SHA-256.\n\nKeys: Ctrl+1 to Ctrl+6 switch tabs, Ctrl+F search, Delete queues a removal, Insert adds files, Ctrl+E extracts, Ctrl+Enter applies, Ctrl+Z / Ctrl+Y undo and redo queued changes, F5 reloads. All actions are in the command palette (Ctrl+Shift+P).";

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
pub struct ToolkitSettings {
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

impl Default for ToolkitSettings {
    fn default() -> Self {
        ToolkitSettings {
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
    Patch,
    Apply,
    Sign,
    SData,
    Inspect,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Archive, Tab::Patch, Tab::Apply, Tab::Sign, Tab::SData, Tab::Inspect];

    pub fn key(self) -> &'static str {
        match self {
            Tab::Archive => "archive",
            Tab::Patch => "patch",
            Tab::Apply => "apply",
            Tab::Sign => "sign",
            Tab::SData => "sdata",
            Tab::Inspect => "inspect",
        }
    }

    pub fn command_id(self) -> &'static str {
        match self {
            Tab::Archive => "tab-archive",
            Tab::Patch => "tab-patch",
            Tab::Apply => "tab-apply",
            Tab::Sign => "tab-sign",
            Tab::SData => "tab-sdata",
            Tab::Inspect => "tab-inspect",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Archive => "Archive",
            Tab::Patch => "Patch builder",
            Tab::Apply => "Apply & history",
            Tab::Sign => "Sign & verify",
            Tab::SData => "SData",
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

pub struct ToolkitApp {
    pub tab: Tab,
    pub archive: ArchiveView,
    pub patch: PatchBuilder,
    pub apply: ApplyTab,
    pub sign: SignTab,
    pub sdata: SDataTab,
    pub inspect: InspectTab,
    pub keys: KeyForm,
    pub clients: ClientsUi,
    pub health: health::HealthView,
    /// The saved client (or opened folder) the tabs work on.
    pub current: Option<ClientInfo>,
    /// The queued changes of the clients that are not the active one, by client folder (memory only; the active client's queue is `archive.pending`).
    queues: BTreeMap<PathBuf, (String, Queue)>,
    /// "Apply to several test clients...".
    pub multi: MultiApply,
    ini_stamp: Option<(PathBuf, u64)>,
    folder_prompt: Option<FolderPrompt>,
    was_focused: bool,
    /// The SData table window, when one is open.
    pub table: Option<SDataTableView>,
    /// The `--shot` scene that is being set up (see `shot.rs`).
    pub(crate) scene: Option<shot::SceneRun>,
}

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

fn has_sah(dir: &Path) -> bool {
    dir.is_dir() && std::fs::read_dir(dir).map(|r| r.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or(false)
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

impl ToolkitApp {
    // ---------------------------------------------------------------------------------------- client handling

    /// Opens a saved client by name (a missing folder is reported, not opened). The queue of the client that was active stays with it.
    pub fn select_client(&mut self, name: &str, cx: &mut ToolkitCx<'_>) {
        if self.archive.is_applying() {
            cx.shell.status("Wait for the change in the Archive tab to finish first");
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

    /// Keeps the active client's queue aside, under its folder, before another source is opened.
    fn stash_queue(&mut self) {
        let name = self.current.as_ref().map(|c| c.name.clone()).or_else(|| self.archive.source_name()).unwrap_or_default();
        if let Some(q) = self.archive.take_queue() {
            self.queues.insert(q.dir.clone(), (name, q));
        }
    }

    /// The number of queued changes of the client in `dir`, whether it is the active one or not.
    pub fn queued_for(&self, dir: &Path) -> usize {
        if self.archive.queue_dir().is_some_and(|d| clients::same_folder(d, dir)) {
            self.archive.pending.len()
        } else {
            self.queues.iter().find(|(d, _)| clients::same_folder(d, dir)).map(|(_, (_, q))| q.len()).unwrap_or(0)
        }
    }

    /// The line of a saved client in the Client picker: name, folder, and the queued changes of that folder ("US test - 3 queued").
    pub fn picker_label_of(&self, c: &shaiya_ui::Client, found: bool, cx: &ToolkitCx<'_>) -> String {
        clients::queued_label(clients::picker_label(c, &cx.shell.folder_text(&c.name, Path::new(&c.path)), found), self.queued_for(Path::new(&c.path)))
    }

    /// Every client that has queued changes: (name, folder, count), the active one first.
    pub fn queued_clients(&self) -> Vec<(String, PathBuf, usize)> {
        let mut v = Vec::new();
        if let Some(dir) = self.archive.queue_dir() {
            let name = self.current.as_ref().map(|c| c.name.clone()).or_else(|| self.archive.source_name()).unwrap_or_default();
            v.push((name, dir.to_path_buf(), self.archive.pending.len()));
        }
        for (dir, (name, q)) in &self.queues {
            if !q.is_empty() {
                v.push((name.clone(), dir.clone(), q.len()));
            }
        }
        v
    }

    /// A client was removed from the list or renamed: the queued changes of its folder are dropped (the question was asked before).
    pub fn drop_queue_of(&mut self, dir: &Path) {
        if self.archive.queue_dir().is_some_and(|d| clients::same_folder(d, dir)) {
            self.archive.drop_queue();
        }
        self.queues.retain(|d, _| !clients::same_folder(d, dir));
    }

    fn open_client_dir(&mut self, name: &str, dir: PathBuf, cx: &mut ToolkitCx<'_>) {
        self.stash_queue();
        let (check_version, current_version) = clients::version_numbers(&dir);
        self.current = Some(ClientInfo { name: name.to_string(), dir: dir.clone(), check_version, current_version });
        self.ini_stamp = None;
        self.archive.open(OpenSource::Client { name: name.to_string(), dir: dir.clone() }, cx);
        if let Some((_, q)) = self.queues.remove(&dir) {
            self.archive.put_queue(q);
        }
        self.set_tab(Tab::Archive, cx);
    }

    /// Opens any archive file read only (its own folder's changes are not offered). The active client's queue stays with that client.
    pub fn open_archive_file(&mut self, path: PathBuf, cx: &mut ToolkitCx<'_>) {
        if self.archive.is_applying() {
            cx.shell.status("Wait for the change in the Archive tab to finish first");
            return;
        }
        self.stash_queue();
        self.current = None;
        cx.shell.add_recent(&path);
        self.archive.open(OpenSource::File(path), cx);
        self.set_tab(Tab::Archive, cx);
    }

    /// Adds a client folder to the shared list (and opens it when asked).
    pub fn add_client(&mut self, name: &str, path: &Path, open: bool, cx: &mut ToolkitCx<'_>) -> shaiya_core::Result<()> {
        clients::add_client(cx.shell, name, path)?;
        if open {
            self.select_client(name, cx);
        }
        Ok(())
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut ToolkitCx<'_>) {
        if tab == Tab::Apply && self.tab != Tab::Apply {
            // a change made outside the Toolkit (the CLI, another window) is listed when the tab is shown
            self.apply.refresh_history();
        }
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

    /// Opens the SData table window for a file (read on a worker thread). A window that is already open is replaced.
    pub fn open_table(&mut self, title: String, input: sdata_table::Input, cx: &mut ToolkitCx<'_>) {
        self.table = Some(SDataTableView::open(title, input, cx));
    }

    /// What a Start page button does.
    pub fn run_start(&mut self, action: StartAction, cx: &mut ToolkitCx<'_>) {
        match action {
            StartAction::AddClient => {
                let picked = cx.shell.pick_folder("client");
                self.clients.open_add(picked.as_deref());
            }
            StartAction::OpenArchive => {
                if let Some(p) = cx.shell.pick_file("open-archive", &[("SAH index", &["sah"])]) {
                    self.open_archive_file(p, cx);
                }
            }
            StartAction::Go(tab) => self.set_tab(tab, cx),
            StartAction::Publish => {
                self.patch.publish.open();
                self.set_tab(Tab::Patch, cx);
            }
            StartAction::Health => self.open_health(None, cx),
        }
    }

    /// Opens the Health window for a saved client by name; `None` is the open client (even when it is no longer in the saved list), else
    /// the first saved client that is there.
    pub fn open_health(&mut self, name: Option<&str>, cx: &mut ToolkitCx<'_>) {
        if name.is_none() {
            if let Some(c) = &self.current {
                let (n, d) = (c.name.clone(), c.dir.clone());
                self.health.open_for(&n, d, cx);
                return;
            }
        }
        let saved = cx.shell.shared().clients.clone();
        let pick = match name {
            Some(n) => saved.iter().find(|c| c.name.eq_ignore_ascii_case(n)),
            None => saved.iter().find(|c| has_sah(Path::new(&c.path))),
        };
        match pick {
            Some(c) => {
                let (n, d) = (c.name.clone(), PathBuf::from(&c.path));
                self.health.open_for(&n, d, cx);
            }
            None => {
                self.health.open = true;
                if saved.is_empty() {
                    cx.shell.status("Add a client folder first");
                }
            }
        }
    }

    /// Carries out what the Archive tab asked for (a patch from the queue or from Compare, a Start page button).
    pub fn handle_requests(&mut self, requests: Vec<TabRequest>, cx: &mut ToolkitCx<'_>) {
        for r in requests {
            match r {
                TabRequest::PatchFromPending => {
                    self.patch.from_pending(self.archive.pending_build_input());
                    self.patch.prefill_signing(&self.keys);
                    self.set_tab(Tab::Patch, cx);
                }
                TabRequest::Start(action) => self.run_start(action, cx),
                TabRequest::PatchFromDiff { old, new, old_label, new_label } => {
                    // OLD = the left side of Compare, NEW = the right side
                    self.patch.from_diff_named(old, new, &old_label, &new_label, cx);
                    self.set_tab(Tab::Patch, cx);
                }
            }
        }
    }

    fn handle_patch_requests(&mut self, requests: Vec<PatchRequest>, cx: &mut ToolkitCx<'_>) {
        for r in requests {
            match r {
                PatchRequest::ApplyPatch(p) => {
                    self.apply.add_patches(vec![p]);
                    self.set_tab(Tab::Apply, cx);
                }
                PatchRequest::VerifyPatch(p) => {
                    self.sign.add_to(Section::Verify, vec![p]);
                    self.set_tab(Tab::Sign, cx);
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------- drops

    fn dropped(&mut self, paths: Vec<PathBuf>, cx: &mut ToolkitCx<'_>) {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut patches: Vec<PathBuf> = Vec::new();
        for p in paths {
            if p.is_dir() && has_sah(&p) {
                self.folder_prompt = Some(FolderPrompt { dir: p, archive_open: self.archive.is_open() });
            } else if ext_of(&p) == "sah" {
                self.open_archive_file(p, cx);
            } else if ext_of(&p) == "patch" {
                patches.push(p);
            } else {
                files.push(p);
            }
        }
        if !patches.is_empty() {
            self.apply.add_patches(patches);
            self.set_tab(Tab::Apply, cx);
        }
        if files.is_empty() {
            return;
        }
        match self.tab {
            Tab::Archive if self.archive.is_open() && self.archive.client_dir().is_some() => self.archive.begin_add(files),
            Tab::Patch => self.patch.drop_paths(files),
            Tab::Sign => self.sign.add_files(files),
            Tab::SData => self.sdata.add_paths(files),
            Tab::Apply => cx.shell.status("Drop .patch files here to apply them"),
            Tab::Archive | Tab::Inspect => {
                self.set_tab(Tab::Inspect, cx);
                self.inspect.open(files, cx);
            }
        }
    }

    fn folder_prompt_ui(&mut self, cx: &mut ToolkitCx<'_>) {
        let Some(p) = &self.folder_prompt else { return };
        let (dir, archive_open) = (p.dir.clone(), p.archive_open);
        let second = if archive_open { "Add its files" } else { "Remember as client" };
        let c = Confirm {
            title: "Dropped folder".into(),
            text: format!("{} holds a data.sah. What should Shaiya Toolkit do with it?", dir.display()),
            buttons: vec!["Open this client".into(), second.into(), "Cancel".into()],
            danger: None,
            ..Default::default()
        };
        match cx.shell.confirm("toolkit-drop-folder", &c) {
            None => {}
            Some(0) => {
                self.folder_prompt = None;
                if self.archive.is_applying() {
                    cx.shell.status("Wait for the change in the Archive tab to finish first");
                } else {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "client".into());
                    let known = cx.shell.shared().clients.iter().find(|c| Path::new(&c.path) == dir).map(|c| c.name.clone());
                    match known {
                        Some(n) => self.select_client(&n, cx),
                        None => self.open_client_dir(&name, dir, cx),
                    }
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

    fn toolbar(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
        enum A {
            Select(String),
            Add,
            Other,
            Manage,
            Undo,
            Tab(Tab),
        }
        let mut act: Option<A> = None;
        let pal = cx.shell.pal();
        let clients: Vec<shaiya_ui::Client> = cx.shell.shared().clients.clone();
        let current = self.current.as_ref().map(|c| c.name.clone());
        let undo = self.undo_state();
        let shown = match (&current, self.archive.source_name()) {
            (Some(n), _) => n.clone(),
            (None, Some(f)) => f,
            (None, None) => "(none)".to_string(),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("Client");
            egui::ComboBox::from_id_salt("toolkit-client").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                for c in &clients {
                    let found = has_sah(Path::new(&c.path));
                    let label = self.picker_label_of(c, found, cx);
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
            let tip = match &undo {
                Ok(label) => format!("Roll back {label}, the newest change the toolkit made. It shows what is restored and asks first."),
                Err(_) => String::new(),
            };
            let b = ui.add_enabled(undo.is_ok(), egui::Button::new("Undo last change"));
            let clicked = match &undo {
                Ok(_) => b.on_hover_text(tip).clicked(),
                Err(why) => b.on_disabled_hover_text(why).clicked(),
            };
            if clicked {
                act = Some(A::Undo);
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
            Some(A::Undo) => self.undo_last_change(cx),
            Some(A::Tab(t)) => self.set_tab(t, cx),
            None => {}
        }
    }

    /// The label of the change "Undo last change" would roll back, or why it cannot be done now (the text of the disabled hover).
    pub fn undo_state(&self) -> Result<String, String> {
        if self.archive.is_applying() {
            return Err("Wait for the change in the Archive tab to finish.".to_string());
        }
        self.apply.undo_target(self.current.is_some()).map(|j| j.label.clone())
    }

    /// Edit > Undo last applied change, the toolbar button and the palette command: the Roll back confirmation of the newest change.
    fn undo_last_change(&mut self, cx: &mut ToolkitCx<'_>) {
        if self.undo_state().is_err() {
            return;
        }
        self.set_tab(Tab::Apply, cx);
        self.apply.request_rollback(self.current.as_ref());
    }

    fn tab_keys(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
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

impl ToolApp for ToolkitApp {
    type Settings = ToolkitSettings;

    fn info() -> AppInfo {
        AppInfo {
            id: "shaiya-toolkit",
            name: "Shaiya Toolkit",
            version: env!("CARGO_PKG_VERSION"),
            git: env!("SHAIYA_GIT_HASH"),
            target: env!("SHAIYA_TARGET"),
            icon_rgba: include_bytes!("../../../assets/icons/shaiya-toolkit-64.rgba"),
            icon_size: 64,
            open_filters: &[("Shaiya files", &["sah", "saf", "sdata", "patch", "sig", "exe"]), ("All files", &["*"])],
            help: HELP,
        }
    }

    fn new(cx: &mut ToolkitCx<'_>) -> ToolkitApp {
        let mut app = ToolkitApp {
            tab: Tab::from_key(&cx.settings.tab),
            archive: ArchiveView::new(),
            patch: PatchBuilder::new(),
            apply: ApplyTab::new(),
            sign: SignTab::new(),
            sdata: SDataTab::new(),
            inspect: InspectTab::new(),
            keys: KeyForm::default(),
            clients: ClientsUi::default(),
            health: health::HealthView::default(),
            current: None,
            queues: BTreeMap::new(),
            multi: MultiApply::default(),
            ini_stamp: None,
            folder_prompt: None,
            was_focused: true,
            table: None,
            scene: None,
        };
        app.archive.show_shadowed = cx.settings.show_shadowed;
        // the last used client opens at start, as a task; a client whose folder is gone is not opened
        // (a `--shot` run never does: its scene decides what opens)
        if let Some(name) = cx.settings.last_client.clone().filter(|_| !cx.shell.hide_paths()) {
            app.select_client(&name, cx);
        }
        app
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
        let ctx = ui.ctx().clone();
        // the client list is re-read when the window gains focus (other toolkit programs may have changed it)
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if focused && !self.was_focused {
            clients::reload(cx.shell);
            self.apply.refresh_history();
        }
        self.was_focused = focused;
        self.refresh_ini();
        self.toolbar(ui, cx);
        ui.separator();
        self.tab_keys(&ctx, cx);
        let mut requests = Vec::new();
        let mut patch_requests = Vec::new();
        let client = self.current.as_ref();
        match self.tab {
            Tab::Archive => requests = self.archive.ui(ui, cx),
            Tab::Patch => patch_requests = self.patch.ui(ui, cx, client, &self.archive, &mut self.keys),
            Tab::Apply => self.apply.ui(ui, cx, client, &mut self.archive, &mut self.keys),
            Tab::Sign => self.sign.ui(ui, cx, client, &mut self.keys),
            Tab::SData => self.sdata.ui(ui, cx, &self.archive),
            Tab::Inspect => self.inspect.ui(ui, cx),
        }
        // what goes on in the background (checks, plans, applies, builds, questions) goes on whichever tab is shown
        if self.tab != Tab::Archive {
            self.archive.poll(cx);
        }
        if self.tab != Tab::Patch {
            self.patch.tick(cx);
        }
        if self.tab != Tab::Apply {
            self.apply.tick(cx, client, &mut self.archive, &mut self.keys);
        }
        if self.tab != Tab::Sign {
            self.sign.tick(&self.keys, cx);
        }
        if self.tab != Tab::SData {
            self.sdata.tick(&self.archive, cx);
        }
        self.clients.queued = self.queued_clients();
        let actions = self.clients.ui(&ctx, cx, &mut self.archive);
        for a in actions {
            match a {
                ClientsAction::DropQueue(d) => self.drop_queue_of(&d),
                ClientsAction::Open(n) => self.select_client(&n, cx),
                ClientsAction::Health(n) => self.open_health(Some(&n), cx),
                ClientsAction::Reload(dir) => {
                    if self.archive.client_dir() == Some(dir.as_path()) || self.archive.source_dir_is(&dir) {
                        self.archive.reopen(cx);
                    }
                }
            }
        }
        if std::mem::take(&mut self.apply.want_multi) {
            self.multi.open_dialog(self.apply.patch_paths(), cx);
        }
        self.multi.ui(&ctx, cx, &mut self.archive, self.apply.is_busy());
        for a in self.health.ui(&ctx, cx, &mut self.archive) {
            match a {
                HealthAction::Reload(dir) => {
                    if self.archive.client_dir() == Some(dir.as_path()) || self.archive.source_dir_is(&dir) {
                        self.archive.reopen(cx);
                    }
                }
                HealthAction::Compact(dir) => {
                    if self.archive.client_dir() == Some(dir.as_path()) {
                        self.set_tab(Tab::Archive, cx);
                        self.archive.request_compact();
                    } else {
                        cx.shell.status("Open this client first (toolbar), then use Compact... again");
                    }
                }
                HealthAction::Prune(dir) => {
                    if self.current.as_ref().is_some_and(|c| c.dir == dir) {
                        self.set_tab(Tab::Apply, cx);
                        self.apply.request_prune(self.current.as_ref());
                    } else {
                        cx.shell.status("Open this client first (toolbar), then use Prune... again");
                    }
                }
            }
        }
        self.folder_prompt_ui(cx);
        if let Some((title, bytes)) = self.archive.table_request.take() {
            self.open_table(title, sdata_table::Input::Bytes(bytes), cx);
        }
        if let Some((title, loader)) = self.sdata.table_request.take() {
            self.open_table(title, sdata_table::Input::Load(loader), cx);
        }
        if let Some(t) = &mut self.table {
            if !t.ui(&ctx, cx) {
                self.table = None;
            }
        }
        self.handle_requests(requests, cx);
        self.handle_patch_requests(patch_requests, cx);
    }

    fn open(&mut self, paths: Vec<PathBuf>, cx: &mut ToolkitCx<'_>) {
        self.dropped(paths, cx);
    }

    fn menu(&mut self, menu: Menu, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
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
                if ui.button("Client health...").clicked() {
                    ui.close();
                    self.open_health(None, cx);
                }
            }
            Menu::Edit => {
                ui.separator();
                let undo = self.undo_state();
                let b = ui.add_enabled(undo.is_ok(), egui::Button::new("Undo last applied change (roll back)..."));
                let clicked = match &undo {
                    Ok(label) => b.on_hover_text(format!("Roll back {label}, the newest change the toolkit made")).clicked(),
                    Err(why) => b.on_disabled_hover_text(why).clicked(),
                };
                if clicked {
                    ui.close();
                    self.undo_last_change(cx);
                }
                if ui.add_enabled(!self.inspect.is_empty(), egui::Button::new("Hash selected")).clicked() {
                    ui.close();
                    self.inspect.hash_selected(cx);
                }
            }
            Menu::Help => {
                // Opens the page in the default browser only when clicked; nothing is fetched or sent by the program itself.
                if ui.button("Support the project").clicked() {
                    ui.close();
                    ui.ctx().open_url(egui::OpenUrl::new_tab(SUPPORT_URL));
                }
            }
            _ => {}
        }
    }

    fn commands(&self) -> Vec<Command> {
        let mut v: Vec<Command> = Vec::new();
        for (i, t) in Tab::ALL.iter().enumerate() {
            v.push(Command {
                id: t.command_id(),
                label: format!("Go to {}", t.label()),
                shortcut: Some(egui::KeyboardShortcut::new(Modifiers::COMMAND, [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6][i])),
                enabled: true,
            });
        }
        v.extend(self.archive.commands());
        let client = self.current.as_ref();
        v.push(Command { id: "patch-build", label: "Build the patch".into(), shortcut: None, enabled: self.patch.build_blocked(&self.keys).is_none() });
        v.push(Command { id: "apply-patches", label: "Apply the listed patches...".into(), shortcut: None, enabled: self.apply.apply_blocked(client).is_none() });
        v.push(Command { id: "apply-multi", label: "Apply patches to several test clients...".into(), shortcut: None, enabled: !self.apply.is_busy() });
        v.push(Command { id: "history-rollback", label: "Undo last applied change (roll back)...".into(), shortcut: None, enabled: self.undo_state().is_ok() });
        v.push(Command { id: "hash-selected", label: "Hash selected files".into(), shortcut: None, enabled: !self.inspect.is_empty() });
        v.push(Command { id: "client-health", label: "Client health...".into(), shortcut: None, enabled: true });
        v
    }

    fn run_command(&mut self, id: &str, cx: &mut ToolkitCx<'_>) {
        match id {
            "tab-archive" | "tab-patch" | "tab-apply" | "tab-sign" | "tab-sdata" | "tab-inspect" => {
                if let Some(t) = Tab::ALL.iter().find(|t| t.command_id() == id) {
                    self.set_tab(*t, cx);
                }
            }
            "patch-build" => {
                self.set_tab(Tab::Patch, cx);
                self.patch.begin_build(&self.keys, cx);
            }
            "apply-patches" => {
                self.set_tab(Tab::Apply, cx);
                self.apply.request_apply(self.current.as_ref(), &self.keys);
            }
            "apply-multi" => {
                self.set_tab(Tab::Apply, cx);
                self.multi.open_dialog(self.apply.patch_paths(), cx);
            }
            "history-rollback" => self.undo_last_change(cx),
            "hash-selected" => self.inspect.hash_selected(cx),
            "client-health" => self.open_health(None, cx),
            other => {
                self.set_tab(Tab::Archive, cx);
                let requests = self.archive.run_command(other, cx);
                self.handle_requests(requests, cx);
            }
        }
    }

    fn undo(&mut self, _cx: &mut ToolkitCx<'_>) -> bool {
        self.tab == Tab::Archive && self.archive.undo()
    }

    fn redo(&mut self, _cx: &mut ToolkitCx<'_>) -> bool {
        self.tab == Tab::Archive && self.archive.redo()
    }

    fn unsaved(&self) -> Vec<String> {
        self.queued_clients().into_iter().map(|(name, _, n)| format!("{name}: {}", shaiya_inspect::counted(n as u64, "pending archive change", "pending archive changes"))).collect()
    }

    fn save_all(&mut self, cx: &mut ToolkitCx<'_>) -> Result<(), String> {
        if self.unsaved().is_empty() {
            return Ok(());
        }
        self.set_tab(Tab::Archive, cx);
        Err("apply or discard the pending changes in the Archive tab".to_string())
    }

    fn shot_scene(&mut self, name: &str, cx: &mut ToolkitCx<'_>) -> Result<(), String> {
        self.begin_scene(name, cx)
    }

    fn shot_step(&mut self, cx: &mut ToolkitCx<'_>) -> bool {
        self.scene_step(cx)
    }

    fn prefs(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>) {
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
