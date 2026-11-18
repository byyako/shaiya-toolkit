//! The GUI shell of the Shaiya toolkit programs. A program implements [`ToolApp`] and calls [`run`]; the shell provides the window, the
//! menus (File, Edit, View, Tools, Help), settings and the data folder, the Activity panel and status bar, background tasks with
//! progress and cancel, the error banner, modal confirmations, file dialogs, drag and drop, the command palette, autosave helpers,
//! undo, the log, and a headless test harness (feature `testing`).
//!
//! Design notes are in `docs/dev/gui-shell.md`. The patterns come from the svmap editor (theme, settings, log, recovery, backups, undo,
//! palette, headless harness).

pub mod args;
pub mod backups;
pub mod fonts;
pub mod log;
pub mod palette;
pub mod recovery;
pub mod settings;
mod shell;
pub mod tasks;
pub mod theme;
pub mod undo;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod shell_tests;

pub use args::ShellOptions;
pub use settings::{Client, Shared};
pub use shell::{run, sibling_path, Confirm, Shell, ShellApp};
pub use tasks::{TaskHandle, TaskOutcome, TaskSpec, Tasks};

/// Third-party notices of the whole toolkit, shown by Help > About.
pub const THIRD_PARTY_NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.txt");

/// The statement every tool shows to first-time users.
pub const YOUR_OWN_COPY: &str = "These tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. Changes to a client folder are recorded in a journal so they can usually be rolled back, but not changes made with --no-journal or archive compact --in-place, which keep at most the previous index (data.sah.bak). Everything else is written only to a path you name.";

pub const RELEASES_URL: &str = "https://github.com/byyako/shaiya-toolkit/releases";

#[derive(Clone, Copy)]
pub struct AppInfo {
    /// `"shaiya-toolkit"`: settings folder, log file, window class.
    pub id: &'static str,
    /// `"Shaiya Toolkit"`
    pub name: &'static str,
    /// `env!("CARGO_PKG_VERSION")`
    pub version: &'static str,
    /// `env!("SHAIYA_GIT_HASH")` from `shaiya_build::build_info()`.
    pub git: &'static str,
    /// `env!("SHAIYA_TARGET")`.
    pub target: &'static str,
    /// `include_bytes!("../../../assets/icons/<id>-64.rgba")`, 64 x 64.
    pub icon_rgba: &'static [u8],
    pub icon_size: u32,
    /// File dialog filters: (label, extensions without dot).
    pub open_filters: &'static [(&'static str, &'static [&'static str])],
    /// The text of F1.
    pub help: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    File,
    Edit,
    View,
    Tools,
    Help,
}

/// A tool command: listed in the command palette (and run through [`ToolApp::run_command`]).
pub struct Command {
    pub id: &'static str,
    pub label: String,
    pub shortcut: Option<egui::KeyboardShortcut>,
    pub enabled: bool,
}

/// What a tool gets in every call: the shell and its own settings (stored in `<tool-id>/settings.json` under `tool`).
pub struct Cx<'a, S> {
    pub shell: &'a mut Shell,
    pub settings: &'a mut S,
}

pub trait ToolApp: 'static {
    type Settings: serde::Serialize + serde::de::DeserializeOwned + Default;

    fn info() -> AppInfo
    where
        Self: Sized;
    fn new(cx: &mut Cx<'_, Self::Settings>) -> Self
    where
        Self: Sized;
    /// The central panel.
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut Cx<'_, Self::Settings>);
    /// Files to open: from the dialog, drag and drop, the command line, the recent list.
    fn open(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Cx<'_, Self::Settings>);
    /// Extra items per menu, after the shell's own.
    fn menu(&mut self, _menu: Menu, _ui: &mut egui::Ui, _cx: &mut Cx<'_, Self::Settings>) {}
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }
    fn run_command(&mut self, _id: &str, _cx: &mut Cx<'_, Self::Settings>) {}
    fn undo(&mut self, _cx: &mut Cx<'_, Self::Settings>) -> bool {
        false
    }
    fn redo(&mut self, _cx: &mut Cx<'_, Self::Settings>) -> bool {
        false
    }
    /// Names of unsaved documents.
    fn unsaved(&self) -> Vec<String> {
        Vec::new()
    }
    fn save_all(&mut self, _cx: &mut Cx<'_, Self::Settings>) -> Result<(), String> {
        Ok(())
    }
    /// The tool's section of the Preferences window.
    fn prefs(&mut self, _ui: &mut egui::Ui, _cx: &mut Cx<'_, Self::Settings>) {}
    /// Whether the tool has documents (then File shows Save, Save as... and Close).
    fn has_documents(&self) -> bool {
        false
    }
    fn save(&mut self, _cx: &mut Cx<'_, Self::Settings>) {}
    fn save_as(&mut self, _cx: &mut Cx<'_, Self::Settings>) {}
    fn close_document(&mut self, _cx: &mut Cx<'_, Self::Settings>) {}
}
