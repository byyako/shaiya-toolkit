//! The client side in the window: the staging folder, Tools > Client > Stage client map, and the two status bar chips
//! ("staged", "client map read-only"). Declared from `app.rs` with `#[path]` so it shares the window's state. The pure parts
//! (the verified save, the staging check) are in `client_files.rs`.

use std::path::PathBuf;

use egui::RichText;

use super::{App, Dialog};
use crate::client_files::{self, ClientWld};
use crate::help_text;
use crate::theme::Pal;

impl App {
    /// Gives a freshly opened client its staging folder (the `client_out` setting). A folder that now lies inside the client is
    /// refused and forgotten, so a staged file can never be read from or written into the client.
    pub(super) fn apply_staging_setting(&mut self) {
        let (Some(client), Some(out)) = (self.client.clone(), self.settings.client_out.clone()) else { return };
        let out = PathBuf::from(out);
        match self.client_path.as_deref().map_or(Ok(()), |c| client_files::check_out_dir(&out, c)) {
            Ok(()) => client.set_staging(Some(out)),
            Err(e) => {
                crate::log::error(format!("client output folder dropped: {e}"));
                self.settings.client_out = None;
            }
        }
    }

    /// The staging folder was picked or cleared in the Preferences (already checked): maps read their `.wld` again.
    pub(super) fn client_out_changed(&mut self, out: Option<PathBuf>) {
        self.settings.client_out = out.as_ref().map(|p| p.display().to_string());
        if let Some(c) = &self.client {
            c.set_staging(out);
        }
        self.slots.clear();
        self.v3.clear_scenes();
        self.kinds_started = false;
        self.kinds.clear();
        self.mark_all_for_validation();
        self.reread_client_parts();
    }

    /// The staging folder: the setting, else a folder picker (asked once). `None` when the user cancels or the folder is refused
    /// (an error dialog says why).
    pub(super) fn ensure_client_out(&mut self) -> Option<PathBuf> {
        let client = self.client_path.clone();
        let check = |out: &PathBuf| client.as_deref().map_or(Ok(()), |c| client_files::check_out_dir(out, c));
        if let Some(out) = self.settings.client_out.clone().map(PathBuf::from) {
            match check(&out) {
                Ok(()) => return Some(out),
                Err(e) => {
                    self.dialog = Some(Dialog::Error(e));
                    return None;
                }
            }
        }
        let picked = rfd::FileDialog::new().set_title("Folder for edited client files").pick_folder()?;
        if let Err(e) = check(&picked) {
            self.dialog = Some(Dialog::Error(e));
            return None;
        }
        self.settings.client_out = Some(picked.display().to_string());
        Some(picked)
    }

    /// Tools > Client > Stage client map: writes this map's client `.wld` to `<staging>/data/world/<id>.wld` through the verified
    /// save (the bytes are the ones read, since nothing edits the client map yet). A `.wld` that does not write back byte for byte
    /// is refused with the reason.
    pub(super) fn stage_client_map(&mut self) {
        let Some(id) = self.docs.get(self.active).and_then(|d| d.map_id) else {
            self.toast("This map has no number, so no client map matches it");
            return;
        };
        let Some(rules) = self.client.clone() else {
            self.toast("Open the client folder first");
            return;
        };
        if self.stage_active_part() {
            return;
        }
        let mut wld = match ClientWld::open(&rules, id) {
            Ok(w) => w,
            Err(e) => {
                self.dialog = Some(Dialog::Error(e));
                return;
            }
        };
        if wld.staged && !wld.dirty() {
            self.toast(format!("world/{id}.wld is already staged"));
            return;
        }
        let Some(out) = self.ensure_client_out() else { return };
        let backups = self.data_dir.as_ref().map(|d| (client_files::backup_dir(d, &out), self.settings.backups_keep));
        match wld.save(&out, backups.as_ref().map(|(p, k)| (p.as_path(), *k))) {
            Ok(saved) => {
                let extra = match (&saved.backup, saved.bak_made) {
                    (Some(b), true) => format!(" (replaced file kept as {} and .bak)", b.display()),
                    (Some(b), false) => format!(" (replaced file kept as {})", b.display()),
                    (None, true) => " (replaced file kept as .bak)".to_string(),
                    (None, false) => String::new(),
                };
                crate::log::info(format!("staged {}{extra}", saved.path.display()));
                rules.set_staging(Some(out));
                self.reread_client_parts();
                self.toast(format!("staged and verified {}", saved.path.display()));
            }
            Err(e) => self.dialog = Some(Dialog::Error(e)),
        }
    }

    /// The chips of the status bar for the active map: "staged" and "client map read-only". Nothing is shown before the map's
    /// client data has loaded (the first read of a `.wld` stays off the window thread).
    pub(super) fn client_chips(&self, ui: &mut egui::Ui, pal: &Pal) {
        self.client_save_chip(ui, pal);
        let Some(id) = self.docs.get(self.active).and_then(|d| d.map_id) else { return };
        let Some(rules) = self.client.as_ref() else { return };
        if self.current_assets().is_none() {
            return;
        }
        let st = rules.wld_state(id);
        if let Some(why) = &st.read_only {
            ui.label(RichText::new("client map read-only").color(pal.warning)).on_hover_text(format!("{} Reason: {why}.", help_text::tip("status-wld-readonly")));
        }
        if st.staged {
            ui.label(RichText::new("staged").color(pal.link)).on_hover_text(help_text::tip("status-staged"));
        }
    }
}
