//! Shaiya Archive & Patch Studio, T0: a file inspector on the shared GUI shell. Open or drop files; Studio identifies them (SAH index,
//! SAF data, SData, patches, game.exe) and hashes them as cancellable tasks. T1 replaces the central panel with the archive browser.

use egui::RichText;
use serde::{Deserialize, Serialize};
use shaiya_core::hash::{sha256_file, to_hex};
use shaiya_inspect::{describe, Info, Options};
use shaiya_ui::{AppInfo, Command, Cx, Menu, TaskHandle, TaskOutcome, TaskSpec, ToolApp};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HELP: &str = "Shaiya Archive & Patch Studio (inspector)\n\nOpen files with File > Open (Ctrl+O), drop them on the window, or pass them on the command line. Studio identifies Shaiya files (data.sah, data.saf, SData files, patches, game.exe) and shows what they are.\n\nSHA-256: press \"SHA-256\" in a row, or select rows and use Hash selected. Hashing runs in the background and can be cancelled in the Activity panel (Ctrl+J).\n\nCopy row as text or JSON puts the details on the clipboard, handy for bug reports. Never attach game files to a report; hashes are enough.\n\nArchive browsing and patches arrive in the next version.";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct StudioSettings {
    /// Column widths: Name, Kind, Size, Details.
    pub columns: Vec<f32>,
}

const DEFAULT_COLUMNS: [f32; 4] = [180.0, 130.0, 110.0, 420.0];

enum RowState {
    Loading(TaskHandle<Info>),
    Ready(Box<Info>),
    Failed(String),
}

enum HashState {
    None,
    Running(TaskHandle<String>),
    Done(String),
    Cancelled,
    Failed(String),
}

struct Row {
    id: u64,
    path: PathBuf,
    state: RowState,
    hash: HashState,
}

impl Row {
    fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.display().to_string())
    }
}

pub struct StudioApp {
    rows: Vec<Row>,
    next_id: u64,
    selected: HashSet<u64>,
}

impl StudioApp {
    /// Adds files (a file opened twice is inspected again).
    fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_, StudioSettings>) {
        for path in paths {
            if shaiya_ui::fonts::needs_cjk(&path.display().to_string()) {
                cx.shell.need_cjk_font();
            }
            self.rows.retain(|r| r.path != path);
            let id = self.next_id;
            self.next_id += 1;
            let p = path.clone();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Inspecting {name}"), cancellable: true, exclusive: None }, move |job| {
                shaiya_inspect::inspect(&p, &Options { hash: false }, job)
            });
            cx.shell.add_recent(&path);
            cx.shell.log_info(&format!("opened {}", path.display()));
            self.rows.push(Row { id, path, state: RowState::Loading(handle), hash: HashState::None });
        }
    }

    /// Starts the SHA-256 task of one row (a running one is left alone).
    fn start_hash(&mut self, id: u64, cx: &mut Cx<'_, StudioSettings>) {
        let Some(row) = self.rows.iter_mut().find(|r| r.id == id) else { return };
        if matches!(row.hash, HashState::Running(_)) {
            return;
        }
        let path = row.path.clone();
        let key = path.display().to_string();
        let name = row.name();
        let handle = cx.shell.tasks().spawn(TaskSpec { title: format!("Hashing {name}"), cancellable: true, exclusive: Some(key) }, move |job| {
            job.stage("SHA-256");
            sha256_file(&path, job).map(|d| to_hex(&d))
        });
        row.hash = HashState::Running(handle);
    }

    /// Hashes the selected rows (all rows when nothing is selected).
    pub fn hash_selected(&mut self, cx: &mut Cx<'_, StudioSettings>) {
        let ids: Vec<u64> = if self.selected.is_empty() { self.rows.iter().map(|r| r.id).collect() } else { self.rows.iter().filter(|r| self.selected.contains(&r.id)).map(|r| r.id).collect() };
        for id in ids {
            self.start_hash(id, cx);
        }
    }

    /// Collects the results of finished tasks.
    fn poll(&mut self, cx: &mut Cx<'_, StudioSettings>) {
        for row in &mut self.rows {
            if let RowState::Loading(h) = &mut row.state {
                match h.poll() {
                    None => {}
                    Some(TaskOutcome::Done(info)) => {
                        if !info.problems.is_empty() {
                            for p in &info.problems {
                                cx.shell.push_activity(shaiya_core::Level::Warn, format!("{}: {p}", row.path.display()));
                            }
                        }
                        row.state = RowState::Ready(Box::new(info));
                    }
                    Some(TaskOutcome::Failed(e)) => {
                        cx.shell.push_activity(shaiya_core::Level::Error, e.to_string());
                        row.state = RowState::Failed(e.to_string());
                    }
                    Some(TaskOutcome::Cancelled) => row.state = RowState::Failed("cancelled".into()),
                    Some(TaskOutcome::Panicked(m)) => row.state = RowState::Failed(format!("internal error (a bug): {m}")),
                }
            }
            if let HashState::Running(h) = &mut row.hash {
                match h.poll() {
                    None => {}
                    Some(TaskOutcome::Done(hex)) => {
                        if let RowState::Ready(info) = &mut row.state {
                            info.sha256 = Some(hex.clone());
                        }
                        row.hash = HashState::Done(hex);
                    }
                    Some(TaskOutcome::Cancelled) => row.hash = HashState::Cancelled,
                    Some(TaskOutcome::Failed(e)) => row.hash = HashState::Failed(e.to_string()),
                    Some(TaskOutcome::Panicked(m)) => row.hash = HashState::Failed(format!("internal error (a bug): {m}")),
                }
            }
        }
    }

    fn row_text(&self, row: &Row) -> String {
        let mut s = format!("{}\n", row.path.display());
        match &row.state {
            RowState::Ready(info) => s.push_str(&describe(info)),
            RowState::Failed(e) => s.push_str(&format!("error: {e}")),
            RowState::Loading(_) => s.push_str("reading..."),
        }
        if let HashState::Done(h) = &row.hash {
            s.push_str(&format!("\nsha256 {h}"));
        }
        s
    }

    fn row_json(&self, row: &Row) -> String {
        let v = match &row.state {
            RowState::Ready(info) => serde_json::to_value(info.as_ref()).unwrap_or_default(),
            RowState::Failed(e) => serde_json::json!({ "path": row.path.display().to_string(), "error": e }),
            RowState::Loading(_) => serde_json::json!({ "path": row.path.display().to_string() }),
        };
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    /// The rows as the table shows them: (name, kind, size, details, sha256 or status), for tests.
    pub fn table(&self) -> Vec<(String, String, String, String, String)> {
        self.rows
            .iter()
            .map(|r| {
                let (kind, size, details) = match &r.state {
                    RowState::Ready(i) => (format!("{:?}", i.kind), shaiya_core::time::human_size(i.size), describe(i)),
                    RowState::Failed(e) => ("error".into(), String::new(), e.clone()),
                    RowState::Loading(_) => ("...".into(), String::new(), String::new()),
                };
                let hash = match &r.hash {
                    HashState::None => String::new(),
                    HashState::Running(_) => "hashing".into(),
                    HashState::Done(h) => h.clone(),
                    HashState::Cancelled => "Cancelled".into(),
                    HashState::Failed(e) => format!("error: {e}"),
                };
                (r.name(), kind, size, details, hash)
            })
            .collect()
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Cancels the running hash of a row (what the Cancel button in the Activity panel does).
    pub fn cancel_hash(&mut self, name: &str) {
        for r in &self.rows {
            if r.name() == name {
                if let HashState::Running(h) = &r.hash {
                    h.cancel();
                }
            }
        }
    }

    /// Starts the hash of the row with this file name (what the row's SHA-256 button does).
    pub fn hash_row(&mut self, name: &str, cx: &mut Cx<'_, StudioSettings>) {
        if let Some(id) = self.rows.iter().find(|r| r.name() == name).map(|r| r.id) {
            self.start_hash(id, cx);
        }
    }

    pub fn path_of(&self, name: &str) -> Option<&Path> {
        self.rows.iter().find(|r| r.name() == name).map(|r| r.path.as_path())
    }
}

fn columns(s: &StudioSettings) -> [f32; 4] {
    let mut c = DEFAULT_COLUMNS;
    for (i, w) in s.columns.iter().take(4).enumerate() {
        if w.is_finite() {
            c[i] = w.clamp(40.0, 2000.0);
        }
    }
    c
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

    fn new(_cx: &mut Cx<'_, StudioSettings>) -> StudioApp {
        StudioApp { rows: Vec::new(), next_id: 1, selected: HashSet::new() }
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_, StudioSettings>) {
        self.poll(cx);
        let pal = cx.shell.pal();
        if self.rows.is_empty() {
            cx.shell.empty_state(
                ui,
                "Drop files here or press Ctrl+O. Studio identifies Shaiya files (data.sah, data.saf, SData, patches, game.exe) and hashes them. Archive browsing and patches arrive in the next version.",
            );
            return;
        }
        // toolbar
        let mut act: Option<&'static str> = None;
        ui.horizontal(|ui| {
            if ui.button("Compute SHA-256").on_hover_text("For the selected rows, or all rows when none is selected").clicked() {
                act = Some("hash");
            }
            if ui.button("Remove from list").clicked() {
                act = Some("remove");
            }
            if ui.button("Copy as text").clicked() {
                act = Some("copy-text");
            }
            if ui.button("Copy as JSON").clicked() {
                act = Some("copy-json");
            }
        });
        ui.separator();
        let cols = columns(cx.settings);
        let header = ["Name", "Kind", "Size", "Details", "SHA-256"];
        ui.horizontal(|ui| {
            for (i, h) in header.iter().enumerate() {
                let w = if i < 4 { cols[i] } else { 520.0 };
                ui.add_sized([w, 18.0], egui::Label::new(RichText::new(*h).strong()));
            }
        });
        let mut hash_click: Option<u64> = None;
        let mut select: Option<(u64, bool)> = None;
        let count = self.rows.len();
        egui::ScrollArea::both().auto_shrink([false, false]).show_rows(ui, 20.0, count, |ui, range| {
            for row in &self.rows[range] {
                let selected = self.selected.contains(&row.id);
                ui.horizontal(|ui| {
                    let (kind, size, details, details_color) = match &row.state {
                        RowState::Ready(i) => (format!("{:?}", i.kind), shaiya_core::time::human_size(i.size), describe(i), if i.problems.is_empty() { pal.text } else { pal.warning }),
                        RowState::Failed(e) => ("error".to_string(), String::new(), e.clone(), pal.error),
                        RowState::Loading(_) => ("...".to_string(), String::new(), "reading...".to_string(), pal.weak),
                    };
                    let name = ui.add_sized([cols[0], 20.0], egui::Button::selectable(selected, row.name()).truncate());
                    if name.clicked() {
                        select = Some((row.id, ui.input(|i| i.modifiers.command)));
                    }
                    ui.add_sized([cols[1], 20.0], egui::Label::new(kind).truncate());
                    ui.add_sized([cols[2], 20.0], egui::Label::new(size).truncate());
                    ui.add_sized([cols[3], 20.0], egui::Label::new(RichText::new(details).color(details_color)).truncate());
                    match &row.hash {
                        HashState::None => {
                            if ui.small_button("SHA-256").clicked() {
                                hash_click = Some(row.id);
                            }
                        }
                        HashState::Running(_) => {
                            ui.label(RichText::new("hashing...").color(pal.weak));
                        }
                        HashState::Done(h) => {
                            ui.add(egui::Label::new(RichText::new(h).monospace()).selectable(true));
                        }
                        HashState::Cancelled => {
                            ui.label(RichText::new("Cancelled").color(pal.warning));
                            if ui.small_button("Retry").clicked() {
                                hash_click = Some(row.id);
                            }
                        }
                        HashState::Failed(e) => {
                            ui.label(RichText::new(e).color(pal.error));
                        }
                    }
                });
            }
        });
        if let Some((id, add)) = select {
            if !add {
                self.selected.clear();
            }
            if !self.selected.remove(&id) || !add {
                self.selected.insert(id);
            }
        }
        if let Some(id) = hash_click {
            self.start_hash(id, cx);
        }
        match act {
            Some("hash") => self.hash_selected(cx),
            Some("remove") => {
                let sel = std::mem::take(&mut self.selected);
                self.rows.retain(|r| !sel.contains(&r.id));
            }
            Some(kind @ ("copy-text" | "copy-json")) => {
                let rows: Vec<&Row> = self.rows.iter().filter(|r| self.selected.is_empty() || self.selected.contains(&r.id)).collect();
                let text = if kind == "copy-text" { rows.iter().map(|r| self.row_text(r)).collect::<Vec<_>>().join("\n\n") } else { format!("[{}]", rows.iter().map(|r| self.row_json(r)).collect::<Vec<_>>().join(",\n")) };
                cx.shell.ctx().copy_text(text);
                cx.shell.status("Copied");
            }
            _ => {}
        }
    }

    fn open(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<'_, StudioSettings>) {
        self.add_files(paths, cx);
    }

    fn menu(&mut self, menu: Menu, ui: &mut egui::Ui, cx: &mut Cx<'_, StudioSettings>) {
        if menu == Menu::Edit {
            ui.separator();
            if ui.add_enabled(!self.rows.is_empty(), egui::Button::new("Hash selected")).clicked() {
                ui.close();
                self.hash_selected(cx);
            }
        }
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command { id: "hash-selected", label: "Hash selected files".into(), shortcut: None, enabled: !self.rows.is_empty() }]
    }

    fn run_command(&mut self, id: &str, cx: &mut Cx<'_, StudioSettings>) {
        if id == "hash-selected" {
            self.hash_selected(cx);
        }
    }

    fn prefs(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_, StudioSettings>) {
        ui.label("Column widths");
        let mut cols = columns(cx.settings);
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
