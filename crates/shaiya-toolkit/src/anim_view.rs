//! The `.ANI` player of the Archive tab: the frames are already rendered (on the worker thread, see `preview::anim_preview`), so this
//! only moves the time, swaps one texture and draws the bone lines over it. Time runs at 30 key units per second.

use crate::preview::{AnimBoneRow, FieldRow, MeshChoice, PreviewData};
use egui::{Color32, RichText, Stroke, TextureHandle};
use shaiya_ui::theme::Pal;

/// Key units per second of an `.ANI` (fixed). A `.VANI` plays one step per `frame_ms`, a framed `.3DO` at its own rate: see `AnimPlayer::rate`.
pub const KEYS_PER_SECOND: f32 = 30.0;

/// The longest step the clock takes in one frame, in seconds (a stalled window does not jump).
const DT_MAX: f32 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub struct AnimPlayer {
    /// The time in key units.
    pub t: f32,
    pub playing: bool,
    pub looping: bool,
    pub show_bones: bool,
    /// Source frames per second: [`KEYS_PER_SECOND`] for an `.ANI`, `1000 / frame_ms` for a `.VANI`, the file's frame rate for a framed `.3DO`.
    pub rate: f32,
}

impl Default for AnimPlayer {
    fn default() -> Self {
        AnimPlayer { t: 0.0, playing: true, looping: true, show_bones: true, rate: KEYS_PER_SECOND }
    }
}

impl AnimPlayer {
    /// Moves the time on by `dt` seconds while playing. Past the end key the time wraps when looping, else it stops there and pauses.
    pub fn advance(&mut self, dt: f32, end_key: i32) {
        if !self.playing {
            return;
        }
        let end = end_key.max(0) as f32;
        let rate = if self.rate.is_finite() && self.rate > 0.0 { self.rate } else { KEYS_PER_SECOND };
        self.t += dt * rate;
        if self.t > end {
            if self.looping && end > 0.0 {
                self.t %= end;
            } else {
                self.t = end;
                self.playing = false;
            }
        }
    }

    /// Jumps to `t` (clamped to `0..=end_key`) and pauses.
    pub fn scrub(&mut self, t: f32, end_key: i32) {
        self.t = if t.is_finite() { t.clamp(0.0, end_key.max(0) as f32) } else { 0.0 };
        self.playing = false;
    }

    /// The frame to show: `min(floor(t / step), n - 1)` (0 when there are no frames).
    pub fn frame_index(&self, step: f32, n: usize) -> usize {
        if n == 0 || step <= 0.0 || !self.t.is_finite() {
            return 0;
        }
        ((self.t / step).floor().max(0.0) as usize).min(n - 1)
    }
}

/// "key 120 of 240, 4.0 s".
pub fn time_text(t: f32, end_key: i32) -> String {
    time_text_at("key", KEYS_PER_SECOND, t, end_key)
}

/// "step 12 of 60, 0.8 s": the source frame `t` of `end_key` and the seconds it is at `rate` frames per second.
pub fn time_text_at(unit: &str, rate: f32, t: f32, end_key: i32) -> String {
    let rate = if rate.is_finite() && rate > 0.0 { rate } else { KEYS_PER_SECOND };
    format!("{unit} {} of {}, {:.1} s", t.floor() as i64, end_key.max(0), t / rate)
}

/// Draws an `Anim` preview. Returns true when the person chose another mesh (`mesh` already holds the choice; the caller restarts the
/// preview with it). `tex` holds the one texture of the frame on screen and the frame it shows.
pub(crate) fn draw(ui: &mut egui::Ui, pal: &Pal, player: &mut AnimPlayer, data: &PreviewData, tex: &mut Option<(usize, TextureHandle)>, mesh: &mut MeshChoice) -> bool {
    let PreviewData::Anim { header, issues, bones, end_key, frames, mesh_choices, notes, rate, unit, .. } = data else { return false };
    player.rate = *rate;
    let mut mesh_changed = false;
    for (i, line) in header.iter().enumerate() {
        if i == 0 {
            ui.label(line);
        } else {
            ui.label(RichText::new(line).color(pal.weak));
        }
    }
    for (text, is_error) in issues {
        ui.label(RichText::new(text).color(if *is_error { pal.warning } else { pal.weak }));
    }
    for note in notes {
        ui.label(RichText::new(note).color(pal.weak));
    }

    // time
    let ctx = ui.ctx().clone();
    if player.playing {
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, DT_MAX);
        player.advance(dt, *end_key);
        ctx.request_repaint();
    }
    let n = frames.bones.len();
    let idx = player.frame_index(frames.step, n);

    ui.horizontal(|ui| {
        if ui.button(if player.playing { "Pause" } else { "Play" }).clicked() {
            if player.playing {
                player.playing = false;
            } else {
                if player.t >= (*end_key).max(0) as f32 && !player.looping {
                    player.t = 0.0;
                }
                player.playing = true;
            }
        }
        ui.checkbox(&mut player.looping, "Loop");
        // only an `.ANI` has bones and a choice of meshes
        if !frames.parents.is_empty() {
            ui.checkbox(&mut player.show_bones, "Bones");
        }
        if !mesh_choices.is_empty() {
            let current = mesh_choices.iter().find(|(c, _)| c == mesh).map(|(_, l)| l.as_str()).unwrap_or("Skeleton only");
            egui::ComboBox::from_id_salt("toolkit-anim-mesh").selected_text(format!("Mesh: {current}")).show_ui(ui, |ui| {
                for (choice, label) in mesh_choices {
                    if ui.selectable_label(choice == mesh, label).clicked() && choice != mesh {
                        *mesh = choice.clone();
                        mesh_changed = true;
                    }
                }
            });
        }
    });
    if *end_key > 0 {
        ui.horizontal(|ui| {
            let mut t = player.t;
            let slider = ui.add(egui::Slider::new(&mut t, 0.0..=*end_key as f32).show_value(false));
            if slider.changed() {
                player.scrub(t, *end_key);
            }
            ui.label(time_text_at(unit, *rate, player.t, *end_key));
        });
    } else {
        ui.label(RichText::new(time_text_at(unit, *rate, player.t, *end_key)).color(pal.weak));
    }

    // the picture: one texture, set when the frame changes
    let scale = (ui.available_width() / frames.width.max(1) as f32).floor().clamp(1.0, 2.0);
    let size = egui::vec2(frames.width as f32, frames.height as f32) * scale;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::from_gray(40));
    if let Some(rgba) = frames.frames.get(idx) {
        // built only when uploaded (a frame change), not on every repaint
        let img = || egui::ColorImage::from_rgba_unmultiplied([frames.width, frames.height], rgba);
        match tex {
            Some((shown, handle)) => {
                if *shown != idx {
                    handle.set(img(), egui::TextureOptions::LINEAR);
                    *shown = idx;
                }
            }
            None => *tex = Some((idx, ctx.load_texture("anim-frame", img(), egui::TextureOptions::LINEAR))),
        }
        if let Some((_, handle)) = tex {
            ui.painter().image(handle.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    if player.show_bones && !frames.parents.is_empty() {
        if let Some(points) = frames.bones.get(idx) {
            let to_screen = |p: [f32; 2]| egui::pos2(rect.min.x + p[0] / shaiya_clientdata::render::PREVIEW_SIZE[0] * size.x, rect.min.y + p[1] / shaiya_clientdata::render::PREVIEW_SIZE[1] * size.y);
            let line = Stroke::new(1.5, Color32::from_rgb(255, 200, 60));
            for (i, p) in points.iter().enumerate() {
                if !(p[0].is_finite() && p[1].is_finite()) {
                    continue;
                }
                if let Some(q) = usize::try_from(frames.parents[i]).ok().and_then(|pi| points.get(pi)).filter(|q| q[0].is_finite() && q[1].is_finite()) {
                    ui.painter().line_segment([to_screen(*q), to_screen(*p)], line);
                }
                ui.painter().circle_filled(to_screen(*p), 2.0, Color32::from_rgb(255, 120, 40));
            }
        }
    }

    if !bones.is_empty() {
        bones_table(ui, bones);
    }
    mesh_changed
}

/// Draws a `Fields` preview (a `.MANI`): the header, the notes and the table of fields with their inferred meanings.
pub(crate) fn draw_fields(ui: &mut egui::Ui, pal: &Pal, data: &PreviewData) {
    let PreviewData::Fields { header, notes, rows } = data else { return };
    ui.label(header);
    for note in notes {
        ui.label(RichText::new(note).color(pal.weak));
    }
    fields_table(ui, rows);
}

fn fields_table(ui: &mut egui::Ui, rows: &[FieldRow]) {
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("toolkit-fields-grid").striped(true).show(ui, |ui| {
            for h in ["Offset", "Field", "Value", "Meaning"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for r in rows {
                ui.label(r.offset.to_string());
                ui.label(&r.name);
                ui.label(RichText::new(&r.value).monospace()).on_hover_text(&r.raw);
                ui.label(if r.inferred { format!("{} (inferred)", r.meaning) } else { r.meaning.clone() });
                ui.end_row();
            }
        });
    });
}

fn bones_table(ui: &mut egui::Ui, bones: &[AnimBoneRow]) {
    let key = |k: Option<i32>| k.map_or("-".to_string(), |k| k.to_string());
    egui::CollapsingHeader::new(format!("Bones ({})", bones.len())).id_salt("toolkit-anim-bones").show(ui, |ui| {
        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
            egui::Grid::new("toolkit-anim-bones-grid").striped(true).show(ui, |ui| {
                for h in ["Bone", "Parent", "Rotation keys", "Translation keys", "First key", "Last key"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for b in bones {
                    ui.label(b.index.to_string());
                    ui.label(b.parent.to_string());
                    ui.label(b.rot_keys.to_string());
                    ui.label(b.trans_keys.to_string());
                    ui.label(key(b.first_key));
                    ui.label(key(b.last_key));
                    ui.end_row();
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_is_30_keys_per_second() {
        let mut p = AnimPlayer::default();
        p.advance(0.5, 240);
        assert!((p.t - 15.0).abs() < 1e-4, "{}", p.t);
        p.advance(1.0, 240);
        assert!((p.t - 45.0).abs() < 1e-4, "{}", p.t);
        assert!(p.playing);
        let mut paused = AnimPlayer { playing: false, ..AnimPlayer::default() };
        paused.advance(1.0, 240);
        assert_eq!(paused.t, 0.0, "a paused player does not move");
    }

    #[test]
    fn looping_wraps_and_not_looping_stops_and_pauses() {
        let mut p = AnimPlayer { t: 29.0, ..AnimPlayer::default() };
        p.advance(0.1, 30); // 29 + 3 = 32 -> wraps to 2
        assert!((p.t - 2.0).abs() < 1e-4 && p.playing, "{} {}", p.t, p.playing);
        let mut q = AnimPlayer { t: 29.0, looping: false, ..AnimPlayer::default() };
        q.advance(0.1, 30);
        assert_eq!((q.t, q.playing), (30.0, false));
        let mut z = AnimPlayer::default();
        z.advance(1.0, 0);
        assert_eq!((z.t, z.playing), (0.0, false), "an animation with no length does not loop");
    }

    #[test]
    fn scrub_clamps_and_pauses() {
        let mut p = AnimPlayer::default();
        p.scrub(500.0, 240);
        assert_eq!((p.t, p.playing), (240.0, false));
        p.playing = true;
        p.scrub(-5.0, 240);
        assert_eq!((p.t, p.playing), (0.0, false));
        p.scrub(f32::NAN, 240);
        assert_eq!(p.t, 0.0);
    }

    #[test]
    fn frame_index_follows_the_step_and_clamps() {
        let mut p = AnimPlayer::default();
        for (t, step, n, want) in [(0.0, 1.0, 241, 0), (120.0, 1.0, 241, 120), (12.9, 5.0, 201, 2), (1000.0, 5.0, 201, 200), (240.0, 1.0, 241, 240), (50.0, 1.0, 0, 0)] {
            p.t = t;
            assert_eq!(p.frame_index(step, n), want, "t {t} step {step} n {n}");
        }
    }

    #[test]
    fn the_time_text_names_key_and_seconds() {
        assert_eq!(time_text(120.0, 240), "key 120 of 240, 4.0 s");
        assert_eq!(time_text(0.0, 0), "key 0 of 0, 0.0 s");
    }

    #[test]
    fn vani_plays_one_step_per_frame_ms() {
        // 66 ms per step: 15.15 steps per second, so 66 ms of time is one step and a second is 15.15
        let mut p = AnimPlayer { rate: 1000.0 / 66.0, ..AnimPlayer::default() };
        p.advance(0.066, 59);
        assert!((p.t - 1.0).abs() < 1e-4, "{}", p.t);
        p.advance(1.0, 59);
        assert!((p.t - (1.0 + 1000.0 / 66.0)).abs() < 1e-3, "{}", p.t);
        // 33 ms: two steps per 66 ms
        let mut q = AnimPlayer { rate: 1000.0 / 33.0, ..AnimPlayer::default() };
        q.advance(0.066, 59);
        assert!((q.t - 2.0).abs() < 1e-4, "{}", q.t);
        // and it wraps at the last step like any player
        let mut w = AnimPlayer { t: 10.9, rate: 1000.0 / 100.0, ..AnimPlayer::default() };
        w.advance(0.2, 11);
        assert!(w.playing && (w.t - 1.9).abs() < 1e-3, "{}", w.t);
    }

    #[test]
    fn a_framed_3do_plays_at_its_inferred_rate_and_a_bad_rate_falls_back() {
        let mut p = AnimPlayer { rate: 20.0, ..AnimPlayer::default() };
        p.advance(0.25, 10);
        assert!((p.t - 5.0).abs() < 1e-4, "{}", p.t);
        for bad in [0.0, -3.0, f32::NAN, f32::INFINITY] {
            let mut q = AnimPlayer { rate: bad, ..AnimPlayer::default() };
            q.advance(0.5, 240);
            assert!((q.t - 15.0).abs() < 1e-4, "rate {bad}: {}", q.t);
        }
        assert_eq!(AnimPlayer::default().rate, KEYS_PER_SECOND, "an .ANI keeps its 30 key units per second");
    }

    #[test]
    fn the_time_text_names_the_unit_and_the_rate() {
        assert_eq!(time_text_at("step", 1000.0 / 66.0, 15.0, 59), "step 15 of 59, 1.0 s");
        assert_eq!(time_text_at("frame", 20.0, 10.0, 10), "frame 10 of 10, 0.5 s");
        assert_eq!(time_text_at("frame", 0.0, 30.0, 60), "frame 30 of 60, 1.0 s");
    }
}
