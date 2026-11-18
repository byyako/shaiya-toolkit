//! The client side in the window: Tools > Client > Share client changes. Export patch, export folder, install into a test
//! client, uninstall and copy loose. Declared from `app.rs` with `#[path]`, so it shares the window's state. The file work is in
//! `share.rs`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::RichText;

use super::App;
use crate::help_text;
use crate::share::{self, Guard, LooseVerdict, Staged};

#[derive(Default)]
pub struct C6 {
    pub win: Option<ShareWin>,
}

pub struct ShareWin {
    pub number: u32,
    pub staged: Vec<Staged>,
    pub target: Option<PathBuf>,
    /// What the chosen folder holds: a line of text.
    pub status: String,
    pub confirm: bool,
    /// `(is an error, text)` of the last action.
    pub msg: Vec<(bool, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    ExportPatch,
    ExportFolder,
    Pick,
    Install,
    Uninstall,
    CopyLoose,
    Refresh,
}

fn size_text(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} bytes")
    }
}

/// A folder as the messages name it: the last two parts, never a drive or a user folder.
fn short(p: &Path) -> String {
    let parts: Vec<String> = p.components().filter_map(|c| match c {
        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }).collect();
    let n = parts.len();
    parts[n.saturating_sub(2)..].join("/")
}

impl App {
    fn share_paths(&self) -> Option<(PathBuf, PathBuf)> {
        Some((self.settings.client_out.clone().map(PathBuf::from)?, self.client_path.clone()?))
    }

    pub(super) fn open_share(&mut self) {
        if self.client_path.is_none() {
            self.toast("Open the client folder first (File > Open client folder...)");
            return;
        }
        if self.c6.win.is_some() {
            return;
        }
        let staged = self.settings.client_out.as_deref().map(|o| share::staged_files(Path::new(o)).unwrap_or_default()).unwrap_or_default();
        self.c6.win = Some(ShareWin { number: 109, staged, target: None, status: String::new(), confirm: false, msg: Vec::new() });
    }

    pub(super) fn demo_share(&mut self, arg: &str) {
        self.open_share();
        if let Some(w) = self.c6.win.as_mut() {
            if !arg.is_empty() {
                w.target = Some(PathBuf::from(arg));
            }
        }
        self.share_status();
    }

    fn share_status(&mut self) {
        let Some(w) = self.c6.win.as_mut() else { return };
        w.status = match &w.target {
            None => String::new(),
            Some(t) => match share::read_record(t) {
                Ok(Some(r)) => format!("Installed by this editor: {} install(s). Uninstall restores the index from before the first one.", r.installs.len()),
                Ok(None) if t.join(share::FIRST_BACKUP).exists() => format!("{} is there without an install record. Move it away before installing.", share::FIRST_BACKUP),
                Ok(None) => "Nothing installed by this editor yet.".to_string(),
                Err(e) => e,
            },
        };
    }

    /// Folders never installed into besides the opened client: the main client from `SHAIYA_CLIENT`, and the client folder kept in
    /// the settings (when the opened client is a different one, the settings folder is most likely the one the user plays on).
    fn share_guard_also(&self) -> Vec<PathBuf> {
        let mut also = main_client_guard();
        also.extend(self.settings.client_folder.as_deref().map(PathBuf::from).filter(|p| p.is_dir()));
        also
    }

    /// Runs one action and writes its result lines to the window.
    pub(super) fn share_act(&mut self, act: Act, picked: Option<PathBuf>) {
        let Some(mut w) = self.c6.win.take() else { return };
        let paths = self.share_paths();
        let also = self.share_guard_also();
        let mut msg: Vec<(bool, String)> = Vec::new();
        if let Some((out, source)) = paths.as_ref() {
            w.staged = share::staged_files(out).unwrap_or_default();
            let guard = Guard { source, also: &also, out: Some(out) };
            let target = w.target.clone();
            match act {
                Act::ExportPatch | Act::ExportFolder => {
                    let Some(dest) = picked else {
                        self.c6.win = Some(w);
                        return;
                    };
                    let r = if act == Act::ExportPatch { share::export_patch(out, source, &dest, w.number) } else { share::export_folder(out, source, &dest) };
                    match r {
                        Ok(e) => {
                            msg.push((false, format!("Wrote {} with {} file(s).", short(&e.path), e.files.len())));
                            for f in &e.files {
                                msg.push((false, format!("  {} ({})", f.rel, size_text(f.size))));
                            }
                            if e.unsigned {
                                msg.push((false, share::UNSIGNED_NOTE.to_string()));
                            }
                            crate::log::info(format!("export: wrote {} ({} files){}", short(&e.path), e.files.len(), if e.unsigned { ", unsigned" } else { "" }));
                        }
                        Err(e) => {
                            crate::log::error(format!("export: {e}"));
                            msg.push((true, e));
                        }
                    }
                }
                Act::Pick => {
                    w.target = picked;
                    w.confirm = false;
                }
                Act::Install | Act::Uninstall | Act::CopyLoose => {
                    if let Some(t) = target {
                        match act {
                            Act::Install => match share::install(out, &t, &guard, SystemTime::now()) {
                                Ok(i) => {
                                    msg.push((false, format!("Installed {} new and {} replaced file(s), {}.", i.added.len(), i.replaced.len(), size_text(i.bytes))));
                                    msg.push((false, format!("Backup of the old index: {} (first install: {}).", i.timestamped.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), if i.first { share::FIRST_BACKUP } else { "kept" })));
                                    msg.push((false, "Checked: the files read back from data.saf, no other entry moved, the old part of data.saf is the same.".to_string()));
                                    for n in &i.notes {
                                        msg.push((false, n.clone()));
                                        crate::log::info(format!("install into {}: {n}", short(&t)));
                                    }
                                    crate::log::info(format!("install into {}: {} added, {} replaced", short(&t), i.added.len(), i.replaced.len()));
                                }
                                Err(e) => {
                                    crate::log::error(format!("install into {}: {e}", short(&t)));
                                    msg.push((true, e));
                                }
                            },
                            Act::Uninstall => match share::uninstall(&t, &guard) {
                                Ok(u) => {
                                    msg.push((false, format!("Uninstalled {} install(s). data.sah is the index from before the first one{}.", u.installs, if u.saf_cut { ", data.saf is cut back to its old length" } else { ". data.saf changed since, so its extra bytes stay (harmless)" })));
                                    for n in &u.notes {
                                        msg.push((false, n.clone()));
                                        crate::log::info(format!("uninstall from {}: {n}", short(&t)));
                                    }
                                    crate::log::info(format!("uninstall from {}", short(&t)));
                                }
                                Err(e) => {
                                    crate::log::error(format!("uninstall from {}: {e}", short(&t)));
                                    msg.push((true, e));
                                }
                            },
                            _ => match share::copy_loose(out, &t, &guard) {
                                Ok(plan) => {
                                    for (s, v) in &plan {
                                        let (bad, text) = match v {
                                            LooseVerdict::Copy => (false, format!("copied {} to data/{}", s.rel, s.rel)),
                                            LooseVerdict::AlreadyThere => (false, format!("{} is already there with the same bytes", s.rel)),
                                            LooseVerdict::InArchive => (true, format!("refused {}: the archive has this path, and the game reads the archive first, so a loose copy is ignored. Use Install or Export patch", s.rel)),
                                            LooseVerdict::Exists => (true, format!("refused {}: a different file is already there; it is not replaced", s.rel)),
                                        };
                                        msg.push((bad, text));
                                    }
                                    if plan.is_empty() {
                                        msg.push((true, "Nothing is staged.".to_string()));
                                    }
                                    crate::log::info(format!("copy loose into {}: {} file(s) planned", short(&t), plan.len()));
                                }
                                Err(e) => {
                                    crate::log::error(format!("copy loose into {}: {e}", short(&t)));
                                    msg.push((true, e));
                                }
                            },
                        }
                    } else {
                        msg.push((true, "Choose the test client folder first.".to_string()));
                    }
                }
                Act::Refresh => {}
            }
        } else {
            msg.push((true, "Save an edited client map first: the staging folder is not set.".to_string()));
        }
        match act {
            Act::Refresh => {}
            Act::Pick => w.msg.clear(),
            _ => w.msg = msg,
        }
        self.c6.win = Some(w);
        self.share_status();
    }

    pub(super) fn share_window(&mut self, ctx: &egui::Context) {
        let Some(w) = self.c6.win.as_ref() else { return };
        let (staged, number, target, status, confirm, msgs) = (w.staged.clone(), w.number, w.target.clone(), w.status.clone(), w.confirm, w.msg.clone());
        let mut num = number;
        let mut confirm_now = confirm;
        let mut act: Option<(Act, Option<PathBuf>)> = None;
        let mut open = true;
        egui::Window::new("Share client changes").open(&mut open).default_size([620.0, 700.0]).resizable(true).show(ctx, |ui| {
            let pal = crate::theme::pal_ui(ui);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label("Everything here uses the files in the staging folder. The editor never writes into the client you opened.");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let total: u64 = staged.iter().map(|s| s.size).sum();
                    ui.label(RichText::new(format!("Staged files: {} ({})", staged.len(), size_text(total))).strong());
                    if ui.small_button("Refresh").clicked() {
                        act = Some((Act::Refresh, None));
                    }
                });
                if staged.is_empty() {
                    ui.label("Nothing is staged. Save an edited client map, or make a map with Add a map.");
                }
                for s in staged.iter().take(40) {
                    ui.label(format!("  {} ({})", s.rel, size_text(s.size)));
                }
                if staged.len() > 40 {
                    ui.label(format!("  and {} more", staged.len() - 40));
                }
                ui.separator();
                ui.heading("Export");
                ui.horizontal(|ui| {
                    ui.label("Patch number");
                    ui.add(egui::DragValue::new(&mut num).range(1..=9999));
                    ui.label(share::patch_name(num).unwrap_or_default());
                });
                ui.horizontal(|ui| {
                    let on = !staged.is_empty();
                    if ui.add_enabled(on, egui::Button::new("Export patch...")).on_hover_text(help_text::tip("share-patch")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Folder for the patch").pick_folder() {
                            act = Some((Act::ExportPatch, Some(p)));
                        }
                    }
                    if ui.add_enabled(on, egui::Button::new("Export folder...")).on_hover_text(help_text::tip("share-folder")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Empty folder for the copy").pick_folder() {
                            act = Some((Act::ExportFolder, Some(p)));
                        }
                    }
                });
                ui.colored_label(pal.warning, share::UNSIGNED_NOTE);
                ui.separator();
                ui.heading("Install into a test client");
                ui.label("This changes data.sah and data.saf of the folder you pick. Use a copy of the client, with the game closed. Install is not proven in the game yet. While it runs it needs free space in that folder of about the staged bytes (a temporary .update.saf file, removed afterwards).");
                ui.horizontal(|ui| {
                    if ui.button("Choose test client folder...").on_hover_text(help_text::tip("share-target")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Test client folder").pick_folder() {
                            act = Some((Act::Pick, Some(p)));
                        }
                    }
                    ui.label(target.as_ref().map(|t| short(t)).unwrap_or_else(|| "no folder chosen".to_string()));
                });
                if !status.is_empty() {
                    ui.label(&status);
                }
                ui.checkbox(&mut confirm_now, "The game is closed and this folder is a test copy");
                ui.horizontal(|ui| {
                    let on = confirm_now && target.is_some();
                    if ui.add_enabled(on && !staged.is_empty(), egui::Button::new("Install")).on_hover_text(help_text::tip("share-install")).clicked() {
                        act = Some((Act::Install, None));
                    }
                    if ui.add_enabled(on, egui::Button::new("Uninstall")).on_hover_text(help_text::tip("share-uninstall")).clicked() {
                        act = Some((Act::Uninstall, None));
                    }
                    if ui.add_enabled(on && !staged.is_empty(), egui::Button::new("Copy loose")).on_hover_text(help_text::tip("share-loose")).clicked() {
                        act = Some((Act::CopyLoose, None));
                    }
                });
                ui.separator();
                for (bad, text) in &msgs {
                    if *bad {
                        ui.colored_label(pal.error, text);
                    } else {
                        ui.label(text);
                    }
                }
            });
        });
        if let Some(w) = self.c6.win.as_mut() {
            w.number = num;
            w.confirm = confirm_now;
        }
        if !open {
            self.c6.win = None;
            return;
        }
        if let Some((a, picked)) = act {
            self.share_act(a, picked);
        }
    }
}

/// The main client folder the editor never installs into, from `SHAIYA_CLIENT` (when set and a folder).
fn main_client_guard() -> Vec<PathBuf> {
    std::env::var_os("SHAIYA_CLIENT").map(PathBuf::from).filter(|p| p.is_dir()).into_iter().collect()
}
