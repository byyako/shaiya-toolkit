//! The shell: window chrome, menus, Activity panel, status bar, error banner, modals, dialogs, drag and drop, shortcuts, settings
//! persistence and the frame loop that every toolkit GUI shares.

use crate::args::{self, Parsed, ShellOptions};
use crate::backups::Backups;
use crate::fonts;
use crate::log;
use crate::palette::{self, Palette};
use crate::recovery::Recovery;
use crate::settings::{self, Env, Os, Paths, Shared, ShellSettings, ToolFile};
use crate::tasks::{LogLine, TaskView, Tasks};
use crate::theme::{self, Pal, Theme};
use crate::{AppInfo, Cx, Menu, ToolApp, RELEASES_URL, THIRD_PARTY_NOTICES, YOUR_OWN_COPY};
use egui::{Align2, Color32, Key, KeyboardShortcut, Modifiers, RichText};
use shaiya_core::time::human_size;
use shaiya_core::{Level, Unit};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const STATUS_SECS: u64 = 6;
const ACTIVITY_LINES: usize = 500;

/// A modal question. Buttons are verbs ("Overwrite", "Cancel"), never "OK".
#[derive(Clone, Debug)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub buttons: Vec<String>,
    /// Index of the button that does something destructive (drawn in the error colour).
    pub danger: Option<usize>,
}

struct Banner {
    title: String,
    summary: String,
    details: String,
    details_open: bool,
}

enum CjkState {
    Off,
    Loading(Receiver<Option<fonts::LoadedFont>>),
    Loaded(PathBuf),
    None,
}

/// What the tools see of the window: status bar, error banner, confirmations, file dialogs, tasks, settings, log.
pub struct Shell {
    ctx: egui::Context,
    info: AppInfo,
    paths: Option<Paths>,
    shared: Shared,
    shared_base: serde_json::Value,
    pub(crate) settings: ShellSettings,
    tasks: Tasks,
    status: Option<(String, Instant)>,
    banner: Option<Banner>,
    confirms: Vec<(String, Confirm)>,
    answers: HashMap<String, usize>,
    pending_open: Vec<PathBuf>,
    activity: Vec<LogLine>,
    activity_filter: Level,
    cjk: CjkState,
    cjk_hint: bool,
    cjk_hint_shown: bool,
    dialogs: bool,
    quit_requested: bool,
}

impl Shell {
    pub fn tasks(&mut self) -> &mut Tasks {
        &mut self.tasks
    }

    /// A message in the status bar for 6 seconds.
    pub fn status(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
        self.ctx.request_repaint_after(Duration::from_secs(STATUS_SECS) + Duration::from_millis(50));
    }

    /// The error banner, plus the log and the Activity list.
    pub fn error(&mut self, title: impl Into<String>, err: &(dyn std::error::Error + 'static)) {
        let title = title.into();
        let mut details = String::new();
        if let Some(e) = err.downcast_ref::<shaiya_core::Error>() {
            use shaiya_core::Error;
            let kind = format!("{:?}", e.kind());
            details.push_str(&format!("kind: {kind}\n"));
            match e {
                Error::Format { file, offset, .. } => {
                    details.push_str(&format!("file: {file}\n"));
                    if let Some(o) = offset {
                        details.push_str(&format!("offset: {o}\n"));
                    }
                }
                Error::Io { path: Some(p), .. } => details.push_str(&format!("file: {}\n", p.display())),
                _ => {}
            }
        }
        details.push_str(&format!("error: {err}\n"));
        let mut src = err.source();
        while let Some(s) = src {
            details.push_str(&format!("caused by: {s}\n"));
            src = s.source();
        }
        details.push_str(&format!(
            "program: {} {} (git {}, {})\nos: {} {}\n",
            self.info.name,
            self.info.version,
            self.info.git,
            self.info.target,
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
        let summary = err.to_string();
        log::error(format!("{title}: {summary}"));
        self.push_activity(Level::Error, format!("{title}: {summary}"));
        self.banner = Some(Banner { title, summary, details, details_open: false });
    }

    /// A modal question. Call it every frame while the question is open; it returns `Some(index of the chosen button)` once answered.
    pub fn confirm(&mut self, id: &str, c: &Confirm) -> Option<usize> {
        if let Some(a) = self.answers.remove(id) {
            self.confirms.retain(|(i, _)| i != id);
            return Some(a);
        }
        if !self.confirms.iter().any(|(i, _)| i == id) {
            self.confirms.push((id.to_string(), c.clone()));
        }
        None
    }

    /// Enables or disables native file dialogs (off in the headless harness, where they would block).
    pub fn set_dialogs_enabled(&mut self, on: bool) {
        self.dialogs = on;
    }

    fn dialog(&mut self, purpose: &str) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new();
        if let Some(dir) = self.settings.last_folder.get(purpose) {
            if Path::new(dir).is_dir() {
                d = d.set_directory(dir);
            }
        }
        d
    }

    fn remember_folder(&mut self, purpose: &str, picked: &Path) {
        let dir = if picked.is_dir() { Some(picked) } else { picked.parent() };
        if let Some(dir) = dir {
            self.settings.last_folder.insert(purpose.to_string(), dir.display().to_string());
        }
    }

    pub fn pick_file(&mut self, purpose: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        if !self.dialogs {
            return None;
        }
        let mut d = self.dialog(purpose);
        for (name, exts) in filters {
            d = d.add_filter(*name, exts);
        }
        let p = d.pick_file()?;
        self.remember_folder(purpose, &p);
        Some(p)
    }

    pub fn pick_files(&mut self, purpose: &str, filters: &[(&str, &[&str])]) -> Vec<PathBuf> {
        if !self.dialogs {
            return Vec::new();
        }
        let mut d = self.dialog(purpose);
        for (name, exts) in filters {
            d = d.add_filter(*name, exts);
        }
        let v = d.pick_files().unwrap_or_default();
        if let Some(first) = v.first() {
            let first = first.clone();
            self.remember_folder(purpose, &first);
        }
        v
    }

    pub fn pick_folder(&mut self, purpose: &str) -> Option<PathBuf> {
        if !self.dialogs {
            return None;
        }
        let p = self.dialog(purpose).pick_folder()?;
        self.remember_folder(purpose, &p);
        Some(p)
    }

    pub fn save_file(&mut self, purpose: &str, default_name: &str, filters: &[(&str, &[&str])]) -> Option<PathBuf> {
        if !self.dialogs {
            return None;
        }
        let mut d = self.dialog(purpose).set_file_name(default_name);
        for (name, exts) in filters {
            d = d.add_filter(*name, exts);
        }
        let p = d.save_file()?;
        self.remember_folder(purpose, &p);
        Some(p)
    }

    /// Adds a file to the recent list (pinned entries stay at the top).
    pub fn add_recent(&mut self, path: &Path) {
        self.settings.add_recent(&path.display().to_string());
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
    }

    pub fn shared_mut(&mut self) -> &mut Shared {
        &mut self.shared
    }

    pub fn pal(&self) -> Pal {
        theme::pal(&self.ctx)
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Shows the one-time hint when a name has Hangul or CJK characters and no CJK font is loaded.
    pub fn need_cjk_font(&mut self) {
        if matches!(self.cjk, CjkState::None) && !self.cjk_hint_shown {
            self.cjk_hint = true;
        }
    }

    /// The font file that provides CJK glyphs, when one is loaded.
    pub fn cjk_font(&self) -> Option<&Path> {
        match &self.cjk {
            CjkState::Loaded(p) => Some(p),
            _ => None,
        }
    }

    pub fn log_info(&self, text: &str) {
        log::info(text);
    }

    pub fn log_warn(&self, text: &str) {
        log::warn(text);
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.paths.as_ref().map(|p| p.tool_dir.as_path())
    }

    pub fn backups(&self) -> Option<Backups> {
        self.paths.as_ref().map(|p| Backups::new(p.backups_dir()))
    }

    pub fn recovery(&self) -> Option<Recovery> {
        self.paths.as_ref().map(|p| Recovery::new(p.recovery_dir()))
    }

    /// Autosave interval and backup count the user chose in Preferences.
    pub fn autosave_secs(&self) -> u32 {
        self.settings.autosave_secs
    }

    pub fn backups_keep(&self) -> u32 {
        self.settings.backups_keep
    }

    /// Asks the shell to open these files through `ToolApp::open` (next frame).
    pub fn request_open(&mut self, paths: Vec<PathBuf>) {
        self.pending_open.extend(paths);
    }

    /// Paths of the recent list, most recent first (pinned entries are not moved).
    pub fn recent_paths(&self) -> Vec<String> {
        self.settings.recent.iter().map(|r| r.path.clone()).collect()
    }

    pub fn activity_open(&self) -> bool {
        self.settings.activity_open
    }

    pub fn window_size(&self) -> [f32; 2] {
        self.settings.window
    }

    #[doc(hidden)]
    pub fn set_window_size(&mut self, size: [f32; 2]) {
        self.settings.window = size;
    }

    /// The text of the error banner's details, when a banner is showing.
    pub fn banner_details(&self) -> Option<&str> {
        self.banner.as_ref().map(|b| b.details.as_str())
    }

    /// For tests: answers a confirmation as if its button had been clicked.
    #[doc(hidden)]
    pub fn inject_answer(&mut self, id: &str, button: usize) {
        self.answers.insert(id.to_string(), button);
    }

    /// Adds a line to the Activity list.
    pub fn push_activity(&mut self, level: Level, text: String) {
        self.activity.push(LogLine { at: std::time::SystemTime::now(), level, text });
        if self.activity.len() > ACTIVITY_LINES {
            let over = self.activity.len() - ACTIVITY_LINES;
            self.activity.drain(..over);
        }
    }

    /// Lines of the Activity list (oldest first), for tests and tools.
    pub fn activity(&self) -> &[LogLine] {
        &self.activity
    }

    /// Starts another toolkit program from the same folder (or the sibling `.app` on macOS) with these arguments. The only process this
    /// code ever starts.
    pub fn launch_sibling(&mut self, tool_id: &str, args: &[std::ffi::OsString]) -> Result<(), String> {
        let path = sibling_path(tool_id).ok_or_else(|| format!("{tool_id} is not installed next to this program; get it from {RELEASES_URL}"))?;
        let mut cmd = std::process::Command::new(&path);
        cmd.args(args);
        cmd.spawn().map(|_| ()).map_err(|e| format!("cannot start {}: {e}", path.display()))
    }

    /// The empty-state block: a hint line, the "your own copy" statement and the recent files. Tools that draw their own empty state may
    /// call it for the recent list.
    pub fn empty_state(&mut self, ui: &mut egui::Ui, hint: &str) {
        let pal = theme::pal_ui(ui);
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(8.0));
            ui.label(RichText::new(hint).size(16.0));
            ui.add_space(8.0);
            ui.label(RichText::new(YOUR_OWN_COPY).color(pal.weak));
            ui.add_space(16.0);
            let recent: Vec<(String, bool)> = self.settings.recent_sorted().iter().map(|r| (r.path.clone(), r.pinned)).collect();
            if !recent.is_empty() {
                ui.label(RichText::new("Recent").strong());
                for (path, pinned) in recent.iter().take(8) {
                    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                    let label = if *pinned { format!("pinned: {name}") } else { name };
                    if ui.link(label).on_hover_text(path).clicked() {
                        self.pending_open.push(PathBuf::from(path));
                    }
                }
            }
        });
    }
}

/// The path of a sibling toolkit program next to the running executable (the `.app` bundle on macOS), when it exists.
pub fn sibling_path(tool_id: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    sibling_in(&dir, tool_id, Os::CURRENT)
}

fn sibling_in(dir: &Path, tool_id: &str, os: Os) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = match os {
        Os::Windows => vec![dir.join(format!("{tool_id}.exe"))],
        Os::Linux => vec![dir.join(tool_id)],
        Os::Mac => {
            // inside a bundle the program sits in X.app/Contents/MacOS; the sibling bundle is next to X.app
            let outer = dir.ancestors().find(|a| a.extension().is_some_and(|e| e == "app")).and_then(|a| a.parent()).unwrap_or(dir);
            vec![outer.join(format!("{tool_id}.app")).join("Contents").join("MacOS").join(tool_id), dir.join(tool_id)]
        }
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// The other toolkit GUIs the Tools menu lists.
const SIBLINGS: [(&str, &str); 3] = [
    ("shaiya-workbench", "Shaiya Workbench"),
    ("shaiya-sdata-editor", "Shaiya SData Editor"),
    ("shaiya-log-viewer", "Shaiya Log Viewer"),
];

enum Quit {
    None,
    /// Running tasks.
    Tasks(usize),
    Unsaved(Vec<String>),
}

enum Action {
    OpenDialog,
    OpenPathField,
    OpenRecent(String),
    TogglePin(String),
    ClearRecent,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Prefs,
    SetTheme(Theme),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Activity,
    Palette,
    Help,
    About,
    OpenLog,
    OpenDataDir,
    Sibling(&'static str),
    Tool(String),
}

struct Smoke {
    report: Option<PathBuf>,
}

/// The eframe app that wraps a tool: owns the shell, the tool's settings and the tool.
pub struct ShellApp<A: ToolApp> {
    pub shell: Shell,
    pub tool_settings: A::Settings,
    pub app: A,
    tool_extra: serde_json::Map<String, serde_json::Value>,
    last_saved_text: String,
    last_shared_saved: Shared,
    last_write: Instant,
    palette: Option<Palette>,
    prefs_open: bool,
    about_open: bool,
    help_open: bool,
    open_path: Option<String>,
    quit: Quit,
    quit_confirmed: bool,
    smoke: Option<Smoke>,
    adapter: Option<String>,
    frame_no: u64,
    applied_look: Option<(Theme, f32)>,
    first_frame_done: bool,
}

fn shortcut(mods: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(mods, key)
}

fn cmd() -> Modifiers {
    Modifiers::COMMAND
}

fn cmd_shift() -> Modifiers {
    Modifiers::COMMAND | Modifiers::SHIFT
}

impl<A: ToolApp> ShellApp<A> {
    /// Builds the shell and the tool. `adapter` is the GPU line for `--smoke` (None in the headless harness).
    pub fn new(ctx: egui::Context, opts: &ShellOptions, adapter: Option<String>) -> ShellApp<A> {
        let info = A::info();
        let paths = if opts.no_settings {
            None
        } else {
            match &opts.data_dir {
                Some(d) => Some(Paths::new(d.clone(), info.id)),
                None => Env::from_process().root(Os::CURRENT).map(|r| Paths::new(r, info.id)),
            }
        };
        let (shared, shared_base) = paths.as_ref().map_or_else(|| (Shared::default(), serde_json::Value::Null), |p| settings::load_shared(&p.shared_file()));
        let file: ToolFile<A::Settings> = paths.as_ref().map_or_else(ToolFile::default, |p| ToolFile::load(&p.settings_file()));
        if let Some(p) = &paths {
            log::init(&p.tool_dir);
            shaiya_core::fs::cleanup_stale_temps(&p.tool_dir);
            shaiya_core::fs::cleanup_stale_temps(&p.root);
            log::info(format!("start {} {}", info.name, info.version));
        }
        let cjk = if opts.load_fonts {
            let env_font = std::env::var("SHAIYA_TOOLKIT_FONT").ok();
            CjkState::Loading(fonts::load_in_background(fonts::candidates(env_font.as_deref(), shared.extra_font.as_deref(), Os::CURRENT)))
        } else {
            CjkState::Off
        };
        let mut shell = Shell {
            ctx: ctx.clone(),
            info,
            paths,
            shared: shared.clone(),
            shared_base,
            settings: file.shell.clone(),
            tasks: Tasks::new(Some(ctx.clone())),
            status: None,
            banner: None,
            confirms: Vec::new(),
            answers: HashMap::new(),
            pending_open: opts.files.clone(),
            activity: Vec::new(),
            activity_filter: Level::Info,
            cjk,
            cjk_hint: false,
            cjk_hint_shown: false,
            dialogs: true,
            quit_requested: false,
        };
        let mut tool_settings = file.tool;
        let app = {
            let mut cx = Cx { shell: &mut shell, settings: &mut tool_settings };
            A::new(&mut cx)
        };
        let mut this = ShellApp {
            shell,
            tool_settings,
            app,
            tool_extra: file.extra,
            last_saved_text: String::new(),
            last_shared_saved: shared,
            last_write: Instant::now(),
            palette: None,
            prefs_open: false,
            about_open: false,
            help_open: false,
            open_path: None,
            quit: Quit::None,
            quit_confirmed: false,
            smoke: opts.smoke.clone().map(|report| Smoke { report }),
            adapter,
            frame_no: 0,
            applied_look: None,
            first_frame_done: false,
        };
        this.last_saved_text = this.settings_text();
        this
    }

    fn settings_text(&self) -> String {
        let f = ToolFile { shell: self.shell.settings.clone(), tool: serde_json::to_value(&self.tool_settings).unwrap_or_default(), extra: self.tool_extra.clone() };
        f.to_json().unwrap_or_default()
    }

    /// Writes the settings when they changed (at most once a second unless `force`).
    pub fn flush_settings(&mut self, force: bool) {
        if self.shell.paths.is_none() {
            return;
        }
        if !force && self.last_write.elapsed() < Duration::from_secs(1) {
            return;
        }
        let text = self.settings_text();
        let paths = self.shell.paths.clone().expect("checked above");
        if text != self.last_saved_text {
            match shaiya_core::fs::write_atomic(&paths.settings_file(), text.as_bytes()) {
                Ok(()) => self.last_saved_text = text,
                Err(e) => log::error(format!("cannot write settings: {e}")),
            }
            self.last_write = Instant::now();
        }
        if self.shell.shared != self.last_shared_saved {
            match settings::save_shared(&paths.shared_file(), &self.shell.shared, &self.shell.shared_base) {
                Ok(merged) => {
                    self.shell.shared_base = merged;
                    self.last_shared_saved = self.shell.shared.clone();
                }
                Err(e) => log::error(format!("cannot write shared settings: {e}")),
            }
            self.last_write = Instant::now();
        }
    }

    fn cx(&mut self) -> (Cx<'_, A::Settings>, &mut A) {
        (Cx { shell: &mut self.shell, settings: &mut self.tool_settings }, &mut self.app)
    }

    // ------------------------------------------------------------------------------------------------------------ look

    fn apply_look(&mut self, ctx: &egui::Context) {
        let want = (self.shell.shared.theme, self.shell.shared.ui_scale);
        if self.applied_look != Some(want) {
            ctx.set_theme(want.0.preference());
            ctx.set_zoom_factor(want.1);
            self.applied_look = Some(want);
        }
        // a CJK font that finished loading
        let loaded = match &self.shell.cjk {
            CjkState::Loading(rx) => rx.try_recv().ok(),
            _ => None,
        };
        if let Some(found) = loaded {
            match found {
                Some(f) => {
                    ctx.set_fonts(fonts::definitions_with(f.bytes));
                    log::info(format!("CJK font: {}", f.path.display()));
                    self.shell.cjk = CjkState::Loaded(f.path);
                }
                None => {
                    log::info("no CJK font found");
                    self.shell.cjk = CjkState::None;
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------- actions

    fn palette_entries(&self) -> Vec<(String, String, bool, Action)> {
        let ctx = &self.shell.ctx;
        let key = |s: KeyboardShortcut| ctx.format_shortcut(&s);
        let has_docs = self.app.has_documents();
        let mut v: Vec<(String, String, bool, Action)> = vec![
            ("Open...".into(), key(shortcut(cmd(), Key::O)), true, Action::OpenDialog),
            ("Open path...".into(), key(shortcut(cmd_shift(), Key::O)), true, Action::OpenPathField),
            ("Save".into(), key(shortcut(cmd(), Key::S)), has_docs, Action::Save),
            ("Save as...".into(), key(shortcut(cmd_shift(), Key::S)), has_docs, Action::SaveAs),
            ("Close".into(), key(shortcut(cmd(), Key::W)), has_docs, Action::Close),
            ("Undo".into(), key(shortcut(cmd(), Key::Z)), true, Action::Undo),
            ("Redo".into(), key(shortcut(cmd(), Key::Y)), true, Action::Redo),
            ("Preferences...".into(), key(shortcut(cmd(), Key::Comma)), true, Action::Prefs),
            ("Theme: Dark".into(), String::new(), true, Action::SetTheme(Theme::Dark)),
            ("Theme: Light".into(), String::new(), true, Action::SetTheme(Theme::Light)),
            ("Theme: System".into(), String::new(), true, Action::SetTheme(Theme::System)),
            ("Zoom in".into(), key(shortcut(cmd(), Key::Equals)), true, Action::ZoomIn),
            ("Zoom out".into(), key(shortcut(cmd(), Key::Minus)), true, Action::ZoomOut),
            ("Zoom reset".into(), key(shortcut(cmd(), Key::Num0)), true, Action::ZoomReset),
            ("Activity panel".into(), key(shortcut(cmd(), Key::J)), true, Action::Activity),
            ("Help".into(), "F1".into(), true, Action::Help),
            ("About".into(), String::new(), true, Action::About),
            ("Open log file".into(), String::new(), self.shell.paths.is_some(), Action::OpenLog),
            ("Open data folder".into(), String::new(), self.shell.paths.is_some(), Action::OpenDataDir),
            ("Quit".into(), key(shortcut(cmd(), Key::Q)), true, Action::Quit),
        ];
        for c in self.app.commands() {
            let sc = c.shortcut.map(|s| ctx.format_shortcut(&s)).unwrap_or_default();
            v.push((c.label, sc, c.enabled, Action::Tool(c.id.to_string())));
        }
        v
    }

    fn run_action(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::OpenDialog => {
                let filters = self.shell.info.open_filters;
                let files = self.shell.pick_files("open", filters);
                self.shell.pending_open.extend(files);
            }
            Action::OpenPathField => self.open_path = Some(String::new()),
            Action::OpenRecent(p) => self.shell.pending_open.push(PathBuf::from(p)),
            Action::TogglePin(p) => self.shell.settings.toggle_pin(&p),
            Action::ClearRecent => self.shell.settings.clear_unpinned_recent(),
            Action::Save => {
                let (mut cx, app) = self.cx();
                app.save(&mut cx);
            }
            Action::SaveAs => {
                let (mut cx, app) = self.cx();
                app.save_as(&mut cx);
            }
            Action::Close => {
                let (mut cx, app) = self.cx();
                app.close_document(&mut cx);
            }
            Action::Quit => self.shell.quit_requested = true,
            Action::Undo => {
                let (mut cx, app) = self.cx();
                if app.undo(&mut cx) {
                    cx.shell.status("Undone");
                }
            }
            Action::Redo => {
                let (mut cx, app) = self.cx();
                if app.redo(&mut cx) {
                    cx.shell.status("Redone");
                }
            }
            Action::Prefs => self.prefs_open = true,
            Action::SetTheme(t) => self.shell.shared.theme = t,
            Action::ZoomIn => self.shell.shared.ui_scale = ((self.shell.shared.ui_scale + 0.1) * 10.0).round() / 10.0,
            Action::ZoomOut => self.shell.shared.ui_scale = ((self.shell.shared.ui_scale - 0.1) * 10.0).round() / 10.0,
            Action::ZoomReset => self.shell.shared.ui_scale = 1.0,
            Action::Activity => self.shell.settings.activity_open = !self.shell.settings.activity_open,
            Action::Palette => self.palette = Some(Palette::new()),
            Action::Help => self.help_open = true,
            Action::About => self.about_open = true,
            Action::OpenLog => {
                if let Some(p) = &self.shell.paths {
                    ctx.open_url(egui::OpenUrl::new_tab(file_url(&p.log_file())));
                }
            }
            Action::OpenDataDir => {
                if let Some(p) = &self.shell.paths {
                    ctx.open_url(egui::OpenUrl::new_tab(file_url(&p.tool_dir)));
                }
            }
            Action::Sibling(id) => {
                if let Err(e) = self.shell.launch_sibling(id, &[]) {
                    self.shell.status(e);
                }
            }
            Action::Tool(id) => {
                let (mut cx, app) = self.cx();
                app.run_command(&id, &mut cx);
            }
        }
        self.shell.settings.activity_height = self.shell.settings.activity_height.clamp(80.0, 600.0);
        self.shell.shared.ui_scale = self.shell.shared.ui_scale.clamp(0.5, 3.0);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) -> Vec<Action> {
        let mut out = Vec::new();
        let typing = ctx.egui_wants_keyboard_input();
        // most specific first: a consumed Ctrl+Shift+X must not also fire Ctrl+X
        let table: [(KeyboardShortcut, fn() -> Action, bool); 17] = [
            (shortcut(cmd_shift(), Key::P), || Action::Palette, false),
            (shortcut(cmd_shift(), Key::O), || Action::OpenPathField, false),
            (shortcut(cmd_shift(), Key::S), || Action::SaveAs, false),
            (shortcut(cmd_shift(), Key::Z), || Action::Redo, true),
            (shortcut(cmd(), Key::O), || Action::OpenDialog, false),
            (shortcut(cmd(), Key::S), || Action::Save, false),
            (shortcut(cmd(), Key::W), || Action::Close, false),
            (shortcut(cmd(), Key::Q), || Action::Quit, false),
            (shortcut(cmd(), Key::Z), || Action::Undo, true),
            (shortcut(cmd(), Key::Y), || Action::Redo, true),
            (shortcut(cmd(), Key::Comma), || Action::Prefs, false),
            (shortcut(cmd(), Key::Equals), || Action::ZoomIn, false),
            (shortcut(cmd(), Key::Plus), || Action::ZoomIn, false),
            (shortcut(cmd(), Key::Minus), || Action::ZoomOut, false),
            (shortcut(cmd(), Key::Num0), || Action::ZoomReset, false),
            (shortcut(cmd(), Key::J), || Action::Activity, false),
            (shortcut(Modifiers::NONE, Key::F1), || Action::Help, false),
        ];
        for (sc, make, text_conflict) in table {
            if text_conflict && typing {
                continue;
            }
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                out.push(make());
            }
        }
        for c in self.app.commands() {
            if let Some(sc) = c.shortcut {
                if c.enabled && ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                    out.push(Action::Tool(c.id.to_string()));
                }
            }
        }
        out
    }

    // ---------------------------------------------------------------------------------------------------------- the frame

    /// One frame of the whole window. eframe's `ui` calls it; the tests call it through `egui::Context::run_ui`.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        self.apply_look(&ctx);

        // lines the tasks and the tools added to the Activity log
        for l in self.shell.tasks.take_log() {
            self.shell.activity.push(l);
        }
        if self.shell.activity.len() > ACTIVITY_LINES {
            let over = self.shell.activity.len() - ACTIVITY_LINES;
            self.shell.activity.drain(..over);
        }

        if !self.first_frame_done {
            self.first_frame_done = true;
            if !self.shell.settings.first_run_done {
                self.shell.settings.first_run_done = true;
                self.shell.status("Welcome. Drop files on the window or press Ctrl+O to open one.");
            }
        }

        // the window: close requests, size
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.shell.quit_requested = true;
        }
        ctx.input(|i| {
            let vp = i.viewport();
            if let Some(m) = vp.maximized {
                self.shell.settings.maximized = m;
            }
            if !vp.maximized.unwrap_or(false) {
                if let Some(r) = vp.inner_rect {
                    let (w, h) = (r.width().round(), r.height().round());
                    if w >= 100.0 && h >= 100.0 {
                        self.shell.settings.window = [w.clamp(settings::WINDOW_MIN[0], settings::WINDOW_MAX[0]), h.clamp(settings::WINDOW_MIN[1], settings::WINDOW_MAX[1])];
                    }
                }
            }
        });

        // dropped files and shortcuts
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        self.shell.pending_open.extend(dropped);
        let mut actions = if self.palette.is_some() { Vec::new() } else { self.handle_shortcuts(&ctx) };

        // quit logic
        if self.shell.quit_requested {
            self.shell.quit_requested = false;
            let running = self.shell.tasks.running();
            let unsaved = self.app.unsaved();
            if running > 0 {
                self.quit = Quit::Tasks(running);
            } else if !unsaved.is_empty() {
                self.quit = Quit::Unsaved(unsaved);
            } else {
                self.finish_quit(&ctx);
            }
        }

        // files to open
        if !self.shell.pending_open.is_empty() {
            let files = std::mem::take(&mut self.shell.pending_open);
            let (mut cx, app) = self.cx();
            app.open(files, &mut cx);
        }

        self.shell.confirms.clear();

        // panels
        egui::Panel::top("shaiya-menu").show(ui, |ui| self.menu_bar(ui, &ctx, &mut actions));
        self.banner_ui(ui, &ctx);
        egui::Panel::bottom("shaiya-status").show(ui, |ui| self.status_bar(ui));
        if self.shell.settings.activity_open {
            egui::Panel::bottom("shaiya-activity").resizable(true).default_size(self.shell.settings.activity_height).show(ui, |ui| {
                self.shell.settings.activity_height = ui.available_height().max(80.0) + 24.0;
                self.activity_ui(ui);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let (mut cx, app) = self.cx();
            app.ui(ui, &mut cx);
        });

        // overlays and modals
        self.drop_overlay(&ctx);
        self.windows(&ctx, &mut actions);
        self.confirm_modals(&ctx);
        self.quit_modal(&ctx);
        self.palette_ui(&ctx, &mut actions);

        for a in actions {
            self.run_action(a, &ctx);
        }

        self.flush_settings(false);
        if self.shell.tasks.running() > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some((_, at)) = &self.shell.status {
            if at.elapsed() > Duration::from_secs(STATUS_SECS) {
                self.shell.status = None;
            }
        }
        self.smoke_step(&ctx);
    }

    fn finish_quit(&mut self, ctx: &egui::Context) {
        self.quit_confirmed = true;
        self.flush_settings(true);
        log::info("exit");
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn smoke_step(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.smoke else { return };
        ctx.request_repaint();
        if self.frame_no < 5 {
            return;
        }
        let (ok, line) = match &self.adapter {
            Some(a) => (true, format!("ok {a}")),
            None => (false, "error: no GPU adapter information (the window was not created with the wgpu renderer)".to_string()),
        };
        if let Some(p) = &s.report {
            let _ = std::fs::write(p, format!("{line}\n"));
        }
        println!("{line}");
        std::process::exit(if ok { 0 } else { 1 });
    }

    // ------------------------------------------------------------------------------------------------------------ menus

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let has_docs = self.app.has_documents();
        let recent: Vec<(String, bool)> = self.shell.settings.recent_sorted().iter().map(|r| (r.path.clone(), r.pinned)).collect();
        egui::MenuBar::new().ui(ui, |ui| {
            let item = |ui: &mut egui::Ui, label: &str, sc: Option<KeyboardShortcut>, enabled: bool, act: Action, actions: &mut Vec<Action>| {
                let mut b = egui::Button::new(label);
                if let Some(sc) = sc {
                    b = b.shortcut_text(ctx.format_shortcut(&sc));
                }
                if ui.add_enabled(enabled, b).clicked() {
                    ui.close();
                    actions.push(act);
                }
            };
            ui.menu_button("File", |ui| {
                item(ui, "Open...", Some(shortcut(cmd(), Key::O)), true, Action::OpenDialog, actions);
                item(ui, "Open path...", Some(shortcut(cmd_shift(), Key::O)), true, Action::OpenPathField, actions);
                ui.menu_button("Open recent", |ui| {
                    if recent.is_empty() {
                        ui.label("(none)");
                    }
                    for (path, pinned) in &recent {
                        ui.horizontal(|ui| {
                            if ui.small_button(if *pinned { "unpin" } else { "pin" }).on_hover_text("Pinned files stay at the top").clicked() {
                                actions.push(Action::TogglePin(path.clone()));
                            }
                            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                            if ui.button(name).on_hover_text(path).clicked() {
                                ui.close();
                                actions.push(Action::OpenRecent(path.clone()));
                            }
                        });
                    }
                    if !recent.is_empty() {
                        ui.separator();
                        if ui.button("Clear unpinned").clicked() {
                            actions.push(Action::ClearRecent);
                        }
                    }
                });
                if has_docs {
                    ui.separator();
                    item(ui, "Save", Some(shortcut(cmd(), Key::S)), true, Action::Save, actions);
                    item(ui, "Save as...", Some(shortcut(cmd_shift(), Key::S)), true, Action::SaveAs, actions);
                    item(ui, "Close", Some(shortcut(cmd(), Key::W)), true, Action::Close, actions);
                }
                {
                    let (mut cx, app) = self.cx();
                    app.menu(Menu::File, ui, &mut cx);
                }
                ui.separator();
                item(ui, "Quit", Some(shortcut(cmd(), Key::Q)), true, Action::Quit, actions);
            });
            ui.menu_button("Edit", |ui| {
                item(ui, "Undo", Some(shortcut(cmd(), Key::Z)), true, Action::Undo, actions);
                item(ui, "Redo", Some(shortcut(cmd(), Key::Y)), true, Action::Redo, actions);
                {
                    let (mut cx, app) = self.cx();
                    app.menu(Menu::Edit, ui, &mut cx);
                }
                ui.separator();
                item(ui, "Preferences...", Some(shortcut(cmd(), Key::Comma)), true, Action::Prefs, actions);
            });
            ui.menu_button("View", |ui| {
                ui.menu_button("Theme", |ui| {
                    for t in Theme::ALL {
                        if ui.radio(self.shell.shared.theme == t, t.label()).clicked() {
                            actions.push(Action::SetTheme(t));
                        }
                    }
                });
                item(ui, "Zoom in", Some(shortcut(cmd(), Key::Equals)), true, Action::ZoomIn, actions);
                item(ui, "Zoom out", Some(shortcut(cmd(), Key::Minus)), true, Action::ZoomOut, actions);
                item(ui, "Zoom reset", Some(shortcut(cmd(), Key::Num0)), true, Action::ZoomReset, actions);
                item(ui, "Activity panel", Some(shortcut(cmd(), Key::J)), true, Action::Activity, actions);
                let (mut cx, app) = self.cx();
                app.menu(Menu::View, ui, &mut cx);
            });
            ui.menu_button("Tools", |ui| {
                item(ui, "Command palette", Some(shortcut(cmd_shift(), Key::P)), true, Action::Palette, actions);
                {
                    let (mut cx, app) = self.cx();
                    app.menu(Menu::Tools, ui, &mut cx);
                }
                ui.separator();
                for (id, name) in SIBLINGS {
                    if id == self.shell.info.id {
                        continue;
                    }
                    if sibling_path(id).is_some() {
                        item(ui, name, None, true, Action::Sibling(id), actions);
                    } else {
                        ui.add_enabled(false, egui::Button::new(format!("{name} (not installed)"))).on_disabled_hover_text(format!("Releases: {RELEASES_URL}"));
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                item(ui, "Help", Some(shortcut(Modifiers::NONE, Key::F1)), true, Action::Help, actions);
                item(ui, "Open log file", None, self.shell.paths.is_some(), Action::OpenLog, actions);
                item(ui, "Open data folder", None, self.shell.paths.is_some(), Action::OpenDataDir, actions);
                {
                    let (mut cx, app) = self.cx();
                    app.menu(Menu::Help, ui, &mut cx);
                }
                ui.separator();
                item(ui, "About", None, true, Action::About, actions);
            });
        });
    }

    // ---------------------------------------------------------------------------------------------------- banner, status

    fn banner_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = theme::pal(ctx);
        let Some(b) = &mut self.shell.banner else { return };
        let mut dismiss = false;
        let mut copy = false;
        egui::Panel::top("shaiya-banner").show(ui, |ui| {
            egui::Frame::new().fill(pal.error.gamma_multiply(0.18)).inner_margin(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{258C}").color(pal.error));
                    ui.label(RichText::new(format!("{}: {}", b.title, b.summary)).color(pal.text));
                    if ui.button(if b.details_open { "Details \u{25BE}" } else { "Details \u{25B8}" }).clicked() {
                        b.details_open = !b.details_open;
                    }
                    if ui.button("Copy").clicked() {
                        copy = true;
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss = true;
                    }
                });
                if b.details_open {
                    ui.add(egui::Label::new(RichText::new(&b.details).monospace()).selectable(true));
                }
            });
        });
        if copy {
            ctx.copy_text(b.details.clone());
        }
        if dismiss {
            self.shell.banner = None;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let views = self.shell.tasks.views();
        let running: Vec<&TaskView> = views.iter().filter(|v| v.finished.is_none()).collect();
        let mut open_activity = false;
        ui.horizontal(|ui| {
            if let Some((text, _)) = &self.shell.status {
                ui.label(text);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("v{}", self.shell.info.version)).weak());
                if !running.is_empty() {
                    let pct = running.iter().find_map(|v| v.fraction()).map(|f| format!(" {}%", (f * 100.0) as u32)).unwrap_or_default();
                    let text = format!("{} task{}{pct}  \u{b7}", running.len(), if running.len() == 1 { "" } else { "s" });
                    if ui.link(text).on_hover_text("Show the Activity panel").clicked() {
                        open_activity = true;
                    }
                }
            });
        });
        if open_activity {
            self.shell.settings.activity_open = true;
        }
    }

    fn activity_ui(&mut self, ui: &mut egui::Ui) {
        let pal = theme::pal_ui(ui);
        let views = self.shell.tasks.views();
        let mut cancel: Vec<u64> = Vec::new();
        ui.horizontal(|ui| {
            if ui.button("Activity \u{25BE}").clicked() {
                self.shell.settings.activity_open = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Copy all").clicked() {
                    let text: String = self.shell.activity.iter().map(|l| format!("{}  {:?}  {}\n", shaiya_core::time::utc_iso(l.at), l.level, l.text)).collect();
                    ui.ctx().copy_text(text);
                }
                egui::ComboBox::from_id_salt("activity-level").selected_text(format!("{:?}", self.shell.activity_filter)).show_ui(ui, |ui| {
                    for l in [Level::Info, Level::Warn, Level::Error] {
                        ui.selectable_value(&mut self.shell.activity_filter, l, format!("{l:?}"));
                    }
                });
            });
        });
        for v in views.iter().filter(|v| v.finished.is_none()) {
            ui.horizontal(|ui| {
                ui.label(format!("{}{}", v.title, if v.waiting { " (waiting)".to_string() } else if v.stage.is_empty() { String::new() } else { format!(": {}", v.stage) }));
                let text = progress_text(v);
                let bar = match v.fraction() {
                    Some(f) => egui::ProgressBar::new(f).text(text),
                    None => egui::ProgressBar::new(0.0).animate(true).text(text),
                };
                ui.add(bar.desired_width(260.0));
                if v.cancellable && ui.button("Cancel").clicked() {
                    cancel.push(v.id);
                }
            });
        }
        for id in cancel {
            self.shell.tasks.cancel(id);
        }
        ui.separator();
        let lines: Vec<&LogLine> = self.shell.activity.iter().filter(|l| l.level >= self.shell.activity_filter).collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show_rows(ui, 16.0, lines.len(), |ui, range| {
            for l in &lines[range] {
                let (_, _, _, _, _, _) = shaiya_core::time::utc_parts(l.at);
                let t = shaiya_core::time::utc_iso(l.at);
                let color = match l.level {
                    Level::Info => pal.text,
                    Level::Warn => pal.warning,
                    Level::Error => pal.error,
                };
                ui.label(RichText::new(format!("{}  {:<5}  {}", &t[11..19], format!("{:?}", l.level).to_lowercase(), l.text)).color(color).monospace());
            }
        });
    }

    // ---------------------------------------------------------------------------------------------------- overlays, modals

    fn drop_overlay(&mut self, ctx: &egui::Context) {
        let n = ctx.input(|i| i.raw.hovered_files.len());
        if n == 0 {
            return;
        }
        egui::Area::new(egui::Id::unique("shaiya-drop-overlay")).order(egui::Order::Foreground).fixed_pos(egui::Pos2::ZERO).interactable(false).show(ctx, |ui| {
            let r = ctx.content_rect();
            ui.painter().rect_filled(r, 0.0, Color32::from_black_alpha(170));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, format!("Drop to open {n} file{}", if n == 1 { "" } else { "s" }), egui::FontId::proportional(26.0), Color32::WHITE);
        });
    }

    fn windows(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let _ = actions;
        // Open path
        if let Some(text) = &mut self.open_path {
            let mut close = false;
            let mut open: Option<String> = None;
            egui::Window::new("Open path").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, -80.0]).show(ctx, |ui| {
                ui.label("Type or paste the path of a file:");
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(460.0).hint_text("C:\\path\\to\\data.sah"));
                if !r.has_focus() && text.is_empty() {
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                ui.horizontal(|ui| {
                    if (ui.button("Open").clicked() || enter) && !text.trim().is_empty() {
                        open = Some(text.trim().trim_matches('"').to_string());
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                close = true;
            }
            if let Some(p) = open {
                self.shell.pending_open.push(PathBuf::from(p));
                close = true;
            }
            if close {
                self.open_path = None;
            }
        }
        // Help
        if self.help_open {
            let mut open = true;
            let help = self.shell.info.help;
            egui::Window::new("Help").open(&mut open).default_size([520.0, 400.0]).show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| ui.label(help));
            });
            self.help_open = open;
        }
        // About
        if self.about_open {
            let mut open = true;
            let info = self.shell.info;
            egui::Window::new(format!("About {}", info.name)).open(&mut open).default_size([560.0, 460.0]).show(ctx, |ui| {
                ui.heading(info.name);
                ui.label(format!("Version {} (git {}, {})", info.version, info.git, info.target));
                ui.label("Copyright (c) 2026 byyako. MIT License.");
                ui.add_space(6.0);
                ui.label(YOUR_OWN_COPY);
                ui.add_space(6.0);
                ui.label(format!("Releases: {RELEASES_URL}"));
                ui.separator();
                ui.label(RichText::new("Third-party notices").strong());
                // the notices file is long (every dependency's licence text): draw only the visible lines
                static LINES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
                let lines = LINES.get_or_init(|| THIRD_PARTY_NOTICES.lines().collect());
                egui::ScrollArea::both().max_height(220.0).auto_shrink([false, false]).show_rows(ui, 14.0, lines.len(), |ui, range| {
                    for l in &lines[range] {
                        ui.add(egui::Label::new(RichText::new(*l).monospace().small()).extend());
                    }
                });
            });
            self.about_open = open;
        }
        // Preferences
        if self.prefs_open {
            let mut open = true;
            egui::Window::new("Preferences").open(&mut open).default_size([460.0, 420.0]).show(ctx, |ui| self.prefs_ui(ui));
            self.prefs_open = open;
        }
        // the CJK hint, once
        if self.shell.cjk_hint {
            egui::Window::new("No font for Korean or Chinese text").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("Some names in these files use Hangul or CJK characters, and no font for them was found on this computer. They are shown as boxes.");
                ui.label("Choose a font file under Edit > Preferences (for example malgun.ttf), or set SHAIYA_TOOLKIT_FONT.");
                if ui.button("Understood").clicked() {
                    self.shell.cjk_hint = false;
                    self.shell.cjk_hint_shown = true;
                }
            });
        }
    }

    fn prefs_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Shell");
        ui.horizontal(|ui| {
            ui.label("Theme");
            for t in Theme::ALL {
                if ui.radio(self.shell.shared.theme == t, t.label()).clicked() {
                    self.shell.shared.theme = t;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("UI scale");
            ui.add(egui::Slider::new(&mut self.shell.shared.ui_scale, 0.5..=3.0).step_by(0.05));
        });
        ui.horizontal(|ui| {
            ui.label("Autosave every");
            ui.add(egui::DragValue::new(&mut self.shell.settings.autosave_secs).range(0..=3600).suffix(" s"));
            ui.label("(0 = off, at least 15)");
        });
        ui.horizontal(|ui| {
            ui.label("Backups kept per file");
            ui.add(egui::DragValue::new(&mut self.shell.settings.backups_keep).range(0..=200));
            ui.label("(0 = off)");
        });
        ui.checkbox(&mut self.shell.settings.restore_session, "Restore the last session at start");
        ui.separator();
        ui.label(RichText::new("Font for Korean and Chinese text").strong());
        match &self.shell.cjk {
            CjkState::Loaded(p) => ui.label(format!("Loaded: {}", p.display())),
            CjkState::Loading(_) => ui.label("Looking for a font..."),
            CjkState::None => ui.label("none found"),
            CjkState::Off => ui.label("not loaded"),
        };
        ui.horizontal(|ui| {
            let mut font = self.shell.shared.extra_font.clone().unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut font).desired_width(300.0).hint_text("font file (.ttf, .ttc, .otf)")).changed() {
                self.shell.shared.extra_font = (!font.trim().is_empty()).then_some(font);
            }
            if ui.button("Choose...").clicked() {
                if let Some(p) = self.shell.pick_file("font", &[("Fonts", &["ttf", "ttc", "otf"])]) {
                    self.shell.shared.extra_font = Some(p.display().to_string());
                }
            }
        });
        ui.label(RichText::new("A changed font is used the next time the program starts.").small().weak());
        ui.separator();
        ui.heading("This tool");
        let (mut cx, app) = self.cx();
        app.prefs(ui, &mut cx);
    }

    fn confirm_modals(&mut self, ctx: &egui::Context) {
        let pending: Vec<(String, Confirm)> = self.shell.confirms.clone();
        let pal = theme::pal(ctx);
        for (id, c) in pending.iter().take(1) {
            let mut chosen = None;
            egui::Modal::new(egui::Id::unique(("shaiya-confirm", id))).show(ctx, |ui| {
                ui.heading(&c.title);
                ui.label(&c.text);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for (i, b) in c.buttons.iter().enumerate() {
                        let mut text = RichText::new(b);
                        if c.danger == Some(i) {
                            text = text.color(pal.error);
                        }
                        if ui.button(text).clicked() {
                            chosen = Some(i);
                        }
                    }
                });
            });
            if let Some(i) = chosen {
                self.shell.answers.insert(id.clone(), i);
            }
        }
        // keep asking: the tool calls `confirm` again next frame and gets the answer
        self.shell.confirms = pending;
    }

    fn quit_modal(&mut self, ctx: &egui::Context) {
        let mut next = Quit::None;
        let mut done = false;
        match std::mem::replace(&mut self.quit, Quit::None) {
            Quit::None => {}
            Quit::Tasks(n) => {
                let mut choice = 0;
                egui::Modal::new(egui::Id::unique("shaiya-quit-tasks")).show(ctx, |ui| {
                    ui.heading("Tasks are still running");
                    ui.label(format!("{n} task{} {} still running.", if n == 1 { "" } else { "s" }, if n == 1 { "is" } else { "are" }));
                    ui.horizontal(|ui| {
                        if ui.button(format!("Cancel {n} task{} and quit", if n == 1 { "" } else { "s" })).clicked() {
                            choice = 1;
                        }
                        if ui.button("Keep working").clicked() {
                            choice = 2;
                        }
                    });
                });
                match choice {
                    1 => {
                        self.shell.tasks.cancel_all();
                        self.shell.tasks.wait_all(Duration::from_secs(5));
                        let unsaved = self.app.unsaved();
                        if unsaved.is_empty() {
                            done = true;
                        } else {
                            next = Quit::Unsaved(unsaved);
                        }
                    }
                    2 => {}
                    _ => next = Quit::Tasks(n),
                }
            }
            Quit::Unsaved(names) => {
                let mut choice = 0;
                egui::Modal::new(egui::Id::unique("shaiya-quit-unsaved")).show(ctx, |ui| {
                    ui.heading("Unsaved changes");
                    ui.label("These documents have changes that are not saved:");
                    for n in &names {
                        ui.label(format!("  {n}"));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save all").clicked() {
                            choice = 1;
                        }
                        if ui.button("Quit without saving").clicked() {
                            choice = 2;
                        }
                        if ui.button("Cancel").clicked() {
                            choice = 3;
                        }
                    });
                });
                match choice {
                    1 => {
                        let r = {
                            let (mut cx, app) = self.cx();
                            app.save_all(&mut cx)
                        };
                        match r {
                            Ok(()) => done = true,
                            Err(e) => {
                                self.shell.status(format!("Not saved: {e}"));
                                self.shell.push_activity(Level::Error, format!("Not saved: {e}"));
                            }
                        }
                    }
                    2 => done = true,
                    3 => {}
                    _ => next = Quit::Unsaved(names),
                }
            }
        }
        self.quit = next;
        if done {
            self.finish_quit(ctx);
        }
    }

    fn palette_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.palette.is_none() {
            return;
        }
        let mut entries = self.palette_entries();
        let Some(p) = &mut self.palette else { return };
        let labels: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        let order = palette::rank(&p.query, &labels);
        let rows: Vec<palette::Row> = order
            .iter()
            .map(|&i| palette::Row { text: entries[i].0.clone(), right: entries[i].1.clone(), enabled: entries[i].2, why: (!entries[i].2).then(|| "Not available right now".to_string()) })
            .collect();
        match palette::show(ctx, p, &rows) {
            palette::Outcome::None => {}
            palette::Outcome::Close => self.palette = None,
            palette::Outcome::Pick(n) => {
                let (_, _, _, act) = entries.swap_remove(order[n]);
                actions.push(act);
                self.palette = None;
            }
        }
    }
}

fn file_url(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// `62%  1.2 GB / 2.0 GB  140 MB/s`
fn progress_text(v: &TaskView) -> String {
    let secs = v.elapsed.as_secs_f64().max(0.001);
    let amount = |n: u64| match v.unit {
        Unit::Bytes => human_size(n),
        Unit::Files => format!("{n} files"),
        Unit::Records => format!("{n} records"),
        Unit::Items => format!("{n} items"),
    };
    let mut s = String::new();
    if let (Some(f), Some(t)) = (v.fraction(), v.total) {
        s.push_str(&format!("{}%  {} / {}", (f * 100.0) as u32, amount(v.done), amount(t)));
    } else {
        s.push_str(&amount(v.done));
    }
    if v.unit == Unit::Bytes && v.done > 0 {
        s.push_str(&format!("  {}/s", human_size((v.done as f64 / secs) as u64)));
    }
    s
}

// ------------------------------------------------------------------------------------------------------------------ eframe

struct Wrapper<A: ToolApp>(ShellApp<A>);

impl<A: ToolApp> eframe::App for Wrapper<A> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.frame(ui);
    }
}

/// Parses the shell flags, loads settings and fonts, opens the window. Returns the process exit code.
pub fn run<A: ToolApp>() -> ExitCode {
    let parsed = match args::parse(std::env::args().skip(1)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}\n{}", args::USAGE);
            return ExitCode::from(2);
        }
    };
    let info = A::info();
    let opts = match parsed {
        Parsed::Help => {
            println!("{}\n\n{}", info.name, args::USAGE);
            return ExitCode::SUCCESS;
        }
        Parsed::Version => {
            println!("{} {} (git {}, {})", info.name, info.version, info.git, info.target);
            return ExitCode::SUCCESS;
        }
        Parsed::Run(o) => o,
    };
    if !opts.no_settings {
        log::install_panic_hook();
    }
    // the window size comes from the settings of the previous run
    let size = {
        let paths = if opts.no_settings {
            None
        } else {
            match &opts.data_dir {
                Some(d) => Some(Paths::new(d.clone(), info.id)),
                None => Env::from_process().root(Os::CURRENT).map(|r| Paths::new(r, info.id)),
            }
        };
        let f: ToolFile<serde_json::Value> = paths.map_or_else(ToolFile::default, |p| ToolFile::load(&p.settings_file()));
        (f.shell.window, f.shell.maximized)
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(size.0)
        .with_min_inner_size(settings::WINDOW_MIN)
        .with_title(info.name)
        .with_app_id(info.id)
        .with_drag_and_drop(true)
        .with_maximized(size.1);
    if info.icon_rgba.len() == (info.icon_size * info.icon_size * 4) as usize {
        viewport = viewport.with_icon(egui::IconData { rgba: info.icon_rgba.to_vec(), width: info.icon_size, height: info.icon_size });
    }
    let native = eframe::NativeOptions { viewport, ..Default::default() };
    let smoke_report = opts.smoke.clone();
    let result = eframe::run_native(
        info.name,
        native,
        Box::new(move |cc| {
            let adapter = cc.wgpu_render_state.as_ref().map(|rs| {
                let i = rs.adapter.get_info();
                format!("{} {:?}", i.name, i.backend)
            });
            Ok(Box::new(Wrapper(ShellApp::<A>::new(cc.egui_ctx.clone(), &opts, adapter))))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let line = format!("error: {e}");
            eprintln!("{line}");
            if let Some(Some(p)) = smoke_report {
                let _ = std::fs::write(p, format!("{line}\n"));
            }
            ExitCode::from(1)
        }
    }
}
