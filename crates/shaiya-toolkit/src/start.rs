//! The Start page: what the toolkit does, one line per job, each with a button that opens the right place. It shows while no client
//! (and no archive) is open. A work package that ships a job adds its row to [`start_jobs`]; a test lists the rows.

use crate::Tab;
use egui::RichText;

/// What a Start page button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartAction {
    /// The "Add client folder" dialog (after the folder picker).
    AddClient,
    /// The "Open other archive" file picker.
    OpenArchive,
    /// Switch to a tab.
    Go(Tab),
    /// The Patch builder with its "Publish for the regular updater" section open.
    Publish,
}

#[derive(Clone, Copy, Debug)]
pub struct StartJob {
    /// Stable name of the row (tests list these).
    pub key: &'static str,
    pub title: &'static str,
    pub line: &'static str,
    pub button: &'static str,
    pub action: StartAction,
}

/// The jobs, in the order they are drawn.
pub fn start_jobs() -> Vec<StartJob> {
    vec![
        StartJob {
            key: "add-client",
            title: "Add a client folder",
            line: "Remember a game folder (a copy of your own client) so every toolkit program can open it.",
            button: "Add client folder...",
            action: StartAction::AddClient,
        },
        StartJob {
            key: "change-files",
            title: "Change files in a client",
            line: "Browse, preview and extract the files of an archive; with a client folder, queue changes and apply them with a backup.",
            button: "Open other archive...",
            action: StartAction::OpenArchive,
        },
        StartJob {
            key: "make-patch",
            title: "Make a patch",
            line: "Pack changed files, or the differences between two archives, into a .patch file.",
            button: "Open Patch builder",
            action: StartAction::Go(Tab::Patch),
        },
        StartJob {
            key: "publish-patch",
            title: "Publish a patch",
            line: "Copy patches into a server folder and raise PatchFileVersion in UpdateVersion.ini, for the original updater.",
            button: "Open Publish",
            action: StartAction::Publish,
        },
        StartJob {
            key: "apply-patches",
            title: "Apply patches to a test client",
            line: "Check patches, see what they would change, apply them in order, and roll them back from the history.",
            button: "Open Apply & history",
            action: StartAction::Go(Tab::Apply),
        },
    ]
}

/// Draws the rows; returns the button that was clicked.
pub fn draw(ui: &mut egui::Ui) -> Option<StartAction> {
    let mut chosen = None;
    ui.label(RichText::new("Start").heading());
    ui.add_space(6.0);
    ui.scope(|ui| {
        ui.set_max_width(640.0);
        for job in start_jobs() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(job.title).strong());
                if ui.button(job.button).clicked() {
                    chosen = Some(job.action);
                }
            });
            ui.add(egui::Label::new(RichText::new(job.line).weak()).wrap());
            ui.add_space(8.0);
        }
    });
    ui.add_space(8.0);
    chosen
}
