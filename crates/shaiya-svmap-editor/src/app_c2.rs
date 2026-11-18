//! The client side in the window: paired documents. Every map tab gets the client half of its map (the lossless `.wld`
//! model and `world/<id>.txt`) once the client is open; Ctrl+S writes the svmap and then the staged client files. Declared from
//! `app.rs` with `#[path]` so it shares the window's state. The pure parts are in `pair.rs` (mirroring) and `doc.rs` (the steps).

use egui::RichText;
use shaiya_clientdata::wld_raw::Codepage;

use super::{App, Dialog};
use crate::client_files;
use crate::doc::ClientSlot;
use crate::help_text;
use crate::pair::{Attach, ClientPart};
use crate::theme::Pal;

impl App {
    /// Gives every editable map tab its client half, one tab per frame (reading a `.wld` takes a moment), and takes it away from a
    /// tab with no unsaved client edits when the client folder is closed or another one is opened. A tab with unsaved client edits
    /// keeps its half. The page of the setting `client_codepage` is passed on to the halves every frame.
    pub(super) fn ensure_client_parts(&mut self) {
        let cp = Codepage::from_key(&self.settings.client_codepage).unwrap_or_default();
        let key = self.client_path.clone();
        let rules = self.client.clone();
        let mut budget = 1;
        for d in &mut self.docs {
            if let Some(p) = d.part_mut() {
                p.cp = cp;
            }
            let (Some(rules), Some(id)) = (rules.as_ref(), d.map_id) else {
                if d.client_for.is_some() && !d.client_dirty() {
                    d.set_client(ClientSlot::None);
                    d.client_for = None;
                }
                continue;
            };
            if d.read_only || d.label.is_some() {
                continue;
            }
            // a tab saved under another number reads the client map of that number
            if d.part().is_some_and(|p| Some(p.wld.id) != d.map_id) && !d.client_dirty() {
                d.client_for = None;
            }
            if d.client_for == key || d.client_dirty() || budget == 0 {
                continue;
            }
            budget -= 1;
            let slot = match ClientPart::attach(rules, id, cp) {
                Attach::Part(p) => ClientSlot::Part(p),
                Attach::ReadOnly(why) => ClientSlot::ReadOnly(why),
                Attach::Absent => ClientSlot::None,
            };
            d.set_client(slot);
            d.client_for = key.clone();
        }
    }

    /// Reads the client half of every tab without unsaved client edits again (the staging folder changed, or a client file was
    /// written by another command).
    pub(super) fn reread_client_parts(&mut self) {
        for d in &mut self.docs {
            if !d.client_dirty() {
                d.client_for = None;
            }
        }
    }

    /// The tab's client half is read at once (a restored tab, a test).
    pub(super) fn attach_client_now(&mut self, uid: u64) {
        let cp = Codepage::from_key(&self.settings.client_codepage).unwrap_or_default();
        let (Some(rules), Some(i)) = (self.client.clone(), self.docs.iter().position(|d| d.uid == uid)) else { return };
        let d = &mut self.docs[i];
        let Some(id) = d.map_id.filter(|_| !d.read_only && d.label.is_none()) else { return };
        let slot = match ClientPart::attach(&rules, id, cp) {
            Attach::Part(p) => ClientSlot::Part(p),
            Attach::ReadOnly(why) => ClientSlot::ReadOnly(why),
            Attach::Absent => ClientSlot::None,
        };
        d.set_client(slot);
        d.client_for = self.client_path.clone();
    }

    /// Writes the unsaved client half of a tab to the staging folder (asked on the first save). True when there was nothing to
    /// write or the write was verified; on a failure the tab keeps its client edits, `client_error` says why and a dialog opens.
    pub(super) fn save_client_part(&mut self, uid: u64) -> bool {
        let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { return true };
        // a terrain stroke in progress holds heights that are no step yet: they must not reach the file (C4)
        if self.terrain.stroking(uid) {
            self.terrain.abort(&mut self.docs[i]);
            self.toast("The terrain stroke was cancelled: the map was saved without it.");
        }
        if !self.docs[i].client_dirty() {
            return true;
        }
        let Some(out) = self.ensure_client_out() else {
            // cancelled, or the folder was refused (a dialog says why)
            self.docs[i].client_error = Some("no staging folder".to_string());
            return false;
        };
        let backups = self.data_dir.as_ref().map(|d| (client_files::backup_dir(d, &out), self.settings.backups_keep));
        let keep = self.settings.backups_keep;
        let d = &mut self.docs[i];
        let title = d.title();
        let result = match d.part_mut() {
            Some(p) => p.save(&out, backups.as_ref().map(|(dir, _)| (dir.as_path(), keep)), false),
            None => return true,
        };
        match result {
            Ok(written) => {
                d.mark_client_saved();
                let names: Vec<String> = written.iter().map(|p| p.display().to_string()).collect();
                crate::log::info(format!("saved and verified client files of {title}: {}", names.join(", ")));
                if let Some(c) = &self.client {
                    c.set_staging(Some(out));
                }
                self.toast(format!("client map saved and verified: {}", names.join(", ")));
                true
            }
            Err(e) => {
                crate::log::error(format!("client save failed: {e}"));
                d.client_error = Some(e.clone());
                self.cancel_save_all();
                self.dialog = Some(Dialog::Error(format!("The map was saved. The client map was not, and it stays modified: {e}")));
                false
            }
        }
    }

    /// The chips of the status bar for the client half of the active map: unsaved, or the reason the last write failed.
    pub(super) fn client_save_chip(&self, ui: &mut egui::Ui, pal: &Pal) {
        let Some(d) = self.docs.get(self.active) else { return };
        if let Some(e) = &d.client_error {
            ui.label(RichText::new("client map not saved").color(pal.error)).on_hover_text(format!("{} {e}", help_text::tip("status-client-unsaved")));
        } else if d.client_dirty() {
            ui.label(RichText::new("client map unsaved").color(pal.warning)).on_hover_text(help_text::tip("status-client-unsaved"));
        }
    }
}

impl App {
    /// Tools > Client > Stage client map on a tab that holds its client half: the model with its edits goes through the same
    /// verified save as Ctrl+S, and a tab without edits writes the map as read. True when the tab had a client half.
    pub(super) fn stage_active_part(&mut self) -> bool {
        let Some(d) = self.docs.get(self.active) else { return false };
        if d.part().is_none() {
            return false;
        }
        let uid = d.uid;
        if d.client_dirty() {
            self.save_client_part(uid);
            return true;
        }
        let id = d.map_id.unwrap_or(0);
        if d.part().is_some_and(|p| p.wld.staged) {
            self.toast(format!("world/{id}.wld is already staged"));
            return true;
        }
        let Some(out) = self.ensure_client_out() else { return true };
        let backups = self.data_dir.as_ref().map(|d| client_files::backup_dir(d, &out));
        let keep = self.settings.backups_keep;
        let Some(i) = self.docs.iter().position(|d| d.uid == uid) else { return true };
        let result = match self.docs[i].part_mut() {
            Some(p) => p.save(&out, backups.as_deref().map(|dir| (dir, keep)), true),
            None => return true,
        };
        match result {
            Ok(written) => {
                let names: Vec<String> = written.iter().map(|p| p.display().to_string()).collect();
                crate::log::info(format!("staged {}", names.join(", ")));
                if let Some(c) = &self.client {
                    c.set_staging(Some(out));
                }
                self.reread_client_parts();
                self.toast(format!("staged and verified {}", names.join(", ")));
            }
            Err(e) => self.dialog = Some(Dialog::Error(e)),
        }
        true
    }
}
