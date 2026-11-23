//! The hover text and the small helper that grey out the other write actions while "Apply to several test clients" runs.
//!
//! This is the consistency of the buttons, not a safety rule: every write task already carries its own exclusive key per client folder.

/// The hover text of a write action that is off while a multi-client apply runs.
pub const MULTI_RUNNING_HOVER: &str = "Applying patches to several clients; wait until it ends";

/// Adds [`MULTI_RUNNING_HOVER`] to a disabled widget when `locked` is true; the widget is returned unchanged otherwise.
pub fn lock_hover(r: egui::Response, locked: bool) -> egui::Response {
    if locked {
        r.on_disabled_hover_text(MULTI_RUNNING_HOVER)
    } else {
        r
    }
}
