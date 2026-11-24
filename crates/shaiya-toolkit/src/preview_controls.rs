//! The mouse controls of a model preview and "Save frame as PNG...": dragging turns the view (yaw by x, pitch by y), the wheel zooms,
//! a double click or "Reset view" returns to the default view. The pictures are made on the preview worker, so a drag or a wheel only
//! changes the view numbers; when the drag ends (or one frame after the last wheel event) the preview is made again with the new view, and
//! the picture on screen is scaled meanwhile. The PNG is encoded and written on a task worker, outside every client folder.

use crate::ToolkitSettings;
use shaiya_clientdata::render::View;
use shaiya_core::fs::write_atomic_checked;
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// Degrees of turn per pixel of drag.
const DEG_PER_PIXEL: f32 = 0.5;
/// Pixels at the left and right of the picture where a press does not turn the view (the pane's edge is grabbed there).
const GRIP: f32 = 12.0;
/// Zoom factor per scroll point (`exp(delta * this)`).
const ZOOM_PER_POINT: f32 = 0.002;

/// The view numbers of a drag and wheel session, and whether the preview has to be made again.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewControl {
    pub view: View,
    pub dragging: bool,
    wheel_pending: bool,
    render_wanted: bool,
}

impl ViewControl {
    pub fn new(view: View) -> ViewControl {
        ViewControl { view: view.sanitized(), dragging: false, wheel_pending: false, render_wanted: false }
    }

    /// Turns the view by a drag of `dx`, `dy` pixels.
    pub fn drag(&mut self, dx: f32, dy: f32) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        self.dragging = true;
        self.view = View { yaw_deg: self.view.yaw_deg + dx * DEG_PER_PIXEL, pitch_deg: self.view.pitch_deg + dy * DEG_PER_PIXEL, ..self.view }.sanitized();
    }

    /// The drag ended: the preview is made again with the turned view.
    pub fn release(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.render_wanted = true;
        }
    }

    /// A wheel event of `delta` scroll points: zooms and waits for the wheel to stop.
    pub fn wheel(&mut self, delta: f32) {
        if !delta.is_finite() || delta == 0.0 {
            return;
        }
        self.view = View { zoom: self.view.zoom * (delta * ZOOM_PER_POINT).exp(), ..self.view }.sanitized();
        self.wheel_pending = true;
    }

    /// Called once per drawn frame with whether the wheel moved in it: the frame after the last wheel event asks for the new picture.
    pub fn end_frame(&mut self, wheel_seen: bool) {
        if self.wheel_pending && !wheel_seen {
            self.wheel_pending = false;
            self.render_wanted = true;
        }
    }

    /// Back to the default view; true when that is a change.
    pub fn reset(&mut self) -> bool {
        self.dragging = false;
        self.wheel_pending = false;
        if self.view == View::DEFAULT {
            return false;
        }
        self.view = View::DEFAULT;
        self.render_wanted = true;
        true
    }

    /// True once when the preview has to be made again with `self.view`.
    pub fn take_render(&mut self) -> bool {
        std::mem::take(&mut self.render_wanted)
    }
}

/// What the preview pane keeps of the controls: the live view, the view of the picture on screen, and the save task.
pub struct PreviewControls {
    pub ctl: ViewControl,
    /// The view the picture on screen was made with.
    pub shown: View,
    save_wanted: bool,
    armed: bool,
    saving: Option<(PathBuf, TaskHandle<()>)>,
}

impl Default for PreviewControls {
    fn default() -> Self {
        PreviewControls { ctl: ViewControl::new(View::DEFAULT), shown: View::DEFAULT, save_wanted: false, armed: false, saving: None }
    }
}

impl PreviewControls {
    /// Takes the view of the settings (a new preview starts: the picture is made with it).
    pub fn load(&mut self, view: View) {
        self.ctl = ViewControl::new(view);
        self.shown = self.ctl.view;
    }

    pub fn take_save(&mut self) -> bool {
        std::mem::take(&mut self.save_wanted)
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// How much the picture on screen is scaled until the new one arrives (1.0 when no zoom is pending).
    pub fn pending_scale(&self) -> f32 {
        let s = self.ctl.view.zoom / self.shown.zoom;
        if s.is_finite() && s > 0.0 { s } else { 1.0 }
    }

    /// Reads the mouse on the picture: drag, wheel, double click. Call once per drawn frame with the picture's response.
    pub fn input(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let mut wheel = 0.0;
        if resp.hovered() {
            wheel = ui.input(|i| i.smooth_scroll_delta.y);
            self.ctl.wheel(wheel);
        }
        // a press near the picture's edge may be the grip of the pane's edge: only a press well inside the picture turns the view
        if resp.drag_started() {
            self.armed = ui.input(|i| i.pointer.press_origin()).is_some_and(|p| resp.rect.shrink2(egui::vec2(GRIP, 0.0)).contains(p));
        }
        if resp.dragged() && self.armed {
            let d = resp.drag_delta();
            self.ctl.drag(d.x, d.y);
        }
        if resp.drag_stopped() {
            if self.armed {
                self.ctl.release();
            }
            self.armed = false;
        }
        if resp.double_clicked() {
            self.ctl.reset();
        }
        self.ctl.end_frame(wheel != 0.0);
    }

    /// "Reset view" and "Save frame as PNG..." (the latter only when `can_save`).
    pub fn buttons(&mut self, ui: &mut egui::Ui, can_save: bool) {
        ui.horizontal(|ui| {
            if ui.button("Reset view").on_hover_text("Back to the default view (double click on the picture does the same)").clicked() {
                self.ctl.reset();
            }
            if ui.add_enabled(can_save && self.saving.is_none(), egui::Button::new("Save frame as PNG...")).clicked() {
                self.save_wanted = true;
            }
        });
    }

    /// Test hook: a drag of `dx`, `dy` pixels that ends at once.
    pub fn drag_for_tests(&mut self, dx: f32, dy: f32) {
        self.ctl.drag(dx, dy);
        self.ctl.release();
    }

    /// Test hook: one wheel event; the picture is asked for on the next drawn frame.
    pub fn wheel_for_tests(&mut self, delta: f32) {
        self.ctl.wheel(delta);
    }

    /// Starts writing the PNG of `rgba` (`w` x `h`) to `out`. A target inside a client folder is refused and nothing is written.
    pub fn save(&mut self, out: PathBuf, w: u32, h: u32, rgba: Vec<u8>, client_dir: Option<&Path>, cx: &mut ToolkitCx<'_>) -> std::result::Result<(), String> {
        let fail = |cx: &mut ToolkitCx<'_>, text: String| {
            cx.shell.error("Save failed", &Error::Refused(text.clone()));
            Err(text)
        };
        if !out.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")) {
            return fail(cx, format!("The file name must end in .png: {}", out.display()));
        }
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        if let Some(text) = crate::export_json::refusal(&out, client_dir, &saved) {
            return fail(cx, text);
        }
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
            return fail(cx, "There is no picture to save".to_string());
        }
        if self.saving.is_some() {
            return fail(cx, "A picture is being saved".to_string());
        }
        if let Some(dir) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            cx.settings.last_export_dir = dir.display().to_string();
        }
        let target = out.clone();
        let handle = cx.shell.tasks().spawn(TaskSpec { title: "Save frame as PNG".into(), cancellable: false, exclusive: None }, move |_job| write_png(&out, w, h, &rgba));
        self.saving = Some((target, handle));
        Ok(())
    }

    /// Finishes a save that is done; returns the status line of a success.
    pub fn poll_save(&mut self, cx: &mut ToolkitCx<'_>) -> Option<String> {
        let (out, mut h) = self.saving.take()?;
        match h.poll() {
            None => {
                self.saving = Some((out, h));
                None
            }
            Some(TaskOutcome::Done(())) => {
                let line = format!("Saved the frame to {}", out.display());
                cx.shell.status(line.clone());
                Some(line)
            }
            Some(TaskOutcome::Failed(e)) => {
                cx.shell.error("Save failed", &e);
                None
            }
            Some(TaskOutcome::Cancelled) => {
                cx.shell.status("Save cancelled; nothing was written");
                None
            }
            Some(TaskOutcome::Panicked(m)) => {
                cx.shell.error("Save failed", &Error::Unsupported(format!("internal error (a bug): {m}")));
                None
            }
        }
    }
}

/// Encodes and writes the PNG; the file written is read back and must carry the PNG signature and the picture's size in its header.
fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let bytes = shaiya_ui::png::png_encode(w, h, rgba);
    write_atomic_checked(out, &bytes, &|tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|e| Error::io(tmp, e))?;
        if png_size(&back) == Some((w, h)) {
            Ok(())
        } else {
            Err(Error::Unsupported("internal error (a bug): the written PNG does not read back with its size".to_string()))
        }
    })
}

/// The size in the header of a PNG, or `None` when the signature or the header chunk is wrong.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() < 24 || bytes[..8] != SIG || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(bytes[16..20].try_into().ok()?), u32::from_be_bytes(bytes[20..24].try_into().ok()?)))
}

/// A settings view as the renderer's: a value out of range or not a number is the default view.
pub fn view_of(v: [f32; 3]) -> View {
    if v.iter().all(|x| x.is_finite()) && v[2] > 0.0 {
        View { yaw_deg: v[0], pitch_deg: v[1], zoom: v[2] }.sanitized()
    } else {
        View::DEFAULT
    }
}

/// The view as the settings keep it.
pub fn view_array(v: View) -> [f32; 3] {
    [v.yaw_deg, v.pitch_deg, v.zoom]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_turns_the_view_and_asks_for_the_picture_when_it_ends() {
        let mut c = ViewControl::new(View::DEFAULT);
        c.drag(20.0, -10.0);
        assert!((c.view.yaw_deg - 45.0).abs() < 1e-4 && (c.view.pitch_deg - 20.0).abs() < 1e-4, "{:?}", c.view);
        assert!(!c.take_render(), "not while the button is down");
        c.release();
        assert!(c.take_render() && !c.take_render());
    }

    #[test]
    fn the_wheel_asks_one_frame_after_the_last_event() {
        let mut c = ViewControl::new(View::DEFAULT);
        c.wheel(100.0);
        c.end_frame(true);
        c.wheel(100.0);
        c.end_frame(true);
        assert!(!c.take_render());
        c.end_frame(false);
        assert!(c.take_render());
        assert!(c.view.zoom > 1.4 && c.view.zoom < 1.6, "{}", c.view.zoom);
    }

    #[test]
    fn a_settings_view_out_of_range_is_the_default() {
        assert_eq!(view_of([f32::NAN, 0.0, 1.0]), View::DEFAULT);
        assert_eq!(view_of([10.0, 10.0, 0.0]), View::DEFAULT);
        assert_eq!(view_of([10.0, 10.0, -2.0]), View::DEFAULT);
        assert_eq!(view_of([10.0, 20.0, 2.0]), View { yaw_deg: 10.0, pitch_deg: 20.0, zoom: 2.0 });
    }
}
