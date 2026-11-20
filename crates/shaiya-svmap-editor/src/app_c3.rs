//! The client side in the window: the Placements tool. The toolbar options, the model browser, the inspector and the
//! Delete and Duplicate commands while the tool is active. Declared from `app.rs` with `#[path]`. The pure parts are in `place.rs`
//! (operations, picking) and `place_ui.rs` (selection, drag, overlay).

use egui::RichText;

use super::App;
use crate::place::{self, PlaceList};
use crate::place_ui as pu;
use crate::view2d::Tool;

impl App {
    pub(super) fn place_delete(&mut self) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        match pu::delete_selected(d) {
            Ok(0) => {}
            Ok(n) => self.toast(format!("Deleted {n} placement(s). The names stay in the list.")),
            Err(e) => self.toast(e),
        }
    }

    pub(super) fn place_duplicate(&mut self) {
        let Some(d) = self.docs.get_mut(self.active) else { return };
        match pu::duplicate_selected(d) {
            Ok(0) => {}
            Ok(n) => self.toast(format!("Duplicated {n} placement(s), 2 units aside.")),
            Err(e) => self.toast(e),
        }
    }

    fn place_snap(&mut self) {
        let assets = self.current_assets();
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if let Err(e) = pu::snap_selected(d, pu::field_ground(assets.as_deref())) {
            self.toast(e);
        }
    }

    /// Toolbar options of the Placements tool.
    pub(super) fn place_toolbar(&mut self, ui: &mut egui::Ui) {
        if ui.selectable_label(self.place.browser_open, "Models...").on_hover_text(crate::help_text::tip("place-models")).clicked() {
            self.place.browser_open = !self.place.browser_open;
        }
        match self.place.armed.clone() {
            Some((l, n)) => {
                ui.label(RichText::new(format!("adding {} {n}", l.label())).strong());
                ui.add(egui::DragValue::new(&mut self.place.armed_yaw).speed(1.0).range(-360.0..=360.0).suffix(" deg")).on_hover_text(crate::help_text::tip("place-yaw"));
                if ui.small_button("Stop").clicked() {
                    self.place.armed = None;
                }
            }
            None => {
                ui.label("pick a model to add, or click a placement").on_hover_text(crate::help_text::tip("place-select"));
            }
        }
    }

    /// The model browser window: a list per kind with a filter, and a textured picture of the chosen model with its wire.
    pub(super) fn place_browser(&mut self, ctx: &egui::Context) {
        self.place.follow_client(self.client_path.as_ref());
        if !self.place.browser_open || self.tool != Tool::Place {
            return;
        }
        let (Some(rules), Some(key)) = (self.client.clone(), self.client_path.clone()) else {
            self.place.browser_open = false;
            self.toast("Open the client folder first.".to_string());
            return;
        };
        self.place.ensure_models(&key, &rules.source);
        if self.place.preview.is_none() {
            let want = self.docs.get(self.active).and_then(|d| {
                let m = &d.part()?.wld.model;
                let s = *d.place_sel.first()?;
                Some((s.0, place::model_name(s.0.list(m), &place::get(m, s)?)?))
            });
            if let Some((l, n)) = want {
                self.place.preview = pu::build_preview(Some(&rules.source), l, &n);
            }
        }
        let mut open = true;
        let mut arm: Option<(PlaceList, String)> = None;
        egui::Window::new("Models").open(&mut open).default_pos([560.0, 300.0]).default_width(520.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for l in PlaceList::ALL {
                    if ui.selectable_label(self.place.list == l, l.plural()).clicked() {
                        self.place.list = l;
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut self.place.filter).hint_text("filter").desired_width(120.0));
            });
            let names: Vec<String> = match &self.place.models {
                Some((_, v)) => {
                    let f = self.place.filter.to_ascii_lowercase();
                    v[self.place.list.slot()].iter().filter(|n| f.is_empty() || n.to_ascii_lowercase().contains(&f)).cloned().collect()
                }
                None => Vec::new(),
            };
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(NAME_W + 24.0);
                egui::ScrollArea::vertical().id_salt("place-models").max_height(300.0).min_scrolled_width(NAME_W).show_rows(ui, 18.0, names.len(), |ui, range| {
                    for n in &names[range] {
                        let on = self.place.armed.as_ref().is_some_and(|(l, a)| *l == self.place.list && a.eq_ignore_ascii_case(n));
                        // a fixed width whatever the longest visible name is: a long name is cut with an ellipsis, the tooltip has it whole
                        let cut = name_galley_w(ui, n) > NAME_W - 14.0;
                        let shown = if cut { fit_name(ui, n, NAME_W - 14.0) } else { n.clone() };
                        let r = ui.add_sized([NAME_W, 18.0], egui::Button::selectable(on, shown));
                        let r = if cut { r.on_hover_text(n) } else { r };
                        if r.clicked() {
                            arm = Some((self.place.list, n.clone()));
                        }
                    }
                });
                });
                ui.vertical(|ui| {
                    ui.label(format!("{} models", names.len()));
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(pu::PREVIEW_SIZE[0], pu::PREVIEW_SIZE[1]), egui::Sense::hover());
                    draw_preview(ui.painter_at(rect), rect, self.place.preview.as_mut());
                    if let Some(t) = self.place.preview.as_ref().and_then(preview_tip) {
                        resp.on_hover_text(t);
                    }
                    if let Some(p) = &self.place.preview {
                        if let Some(b) = p.bounds {
                            ui.label(format!("{} x {} x {} units", (b.max[0] - b.min[0]).round(), (b.max[1] - b.min[1]).round(), (b.max[2] - b.min[2]).round()));
                        }
                        if let Some(t) = preview_note(p) {
                            ui.label(RichText::new(t).weak().small());
                        }
                    }
                    ui.label("Click a name, then click the map to add it.");
                });
            });
        });
        if let Some((l, n)) = arm {
            self.place.arm(l, &n, Some(&rules.source));
        }
        if !open {
            self.place.browser_open = false;
        }
    }

    /// The inspector of the Placements tool: the selected placement's list, model, position and heading, with the actions.
    pub(super) fn place_inspector(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.docs.get(self.active) else { return };
        ui.heading("Placements");
        let Some(part) = d.part() else {
            ui.label("This tab has no editable client map. Open the client folder; a read-only .wld cannot be edited.");
            return;
        };
        let sel = d.place_sel.clone();
        let m = &part.wld.model;
        ui.label(format!("{} buildings, {} shapes, {} trees, {} dungeon pieces", m.building.items.len(), m.shape.items.len(), m.tree.items.len(), m.dungeon.items.len()));
        ui.label(format!("not edited here: {} grass, {} vani, {} effects, {} sounds", m.grass.items.len(), m.vani.items.len() + m.vani2.items.len(), m.effects.len(), m.sound.points.len() + m.sound.bgm_zones.len() + m.sound.wav_zones.len()));
        ui.separator();
        let Some(first) = sel.first().copied() else {
            ui.label("Nothing selected. Click a placement in the map.");
            return;
        };
        if sel.len() > 1 {
            ui.label(format!("{} placements selected", sel.len()));
        }
        let Some(p) = place::get(m, first) else { return };
        ui.label(format!("{} #{}: {}", first.0.label(), first.1, place::model_name(first.0.list(m), &p).unwrap_or_else(|| "(no model)".to_string())));
        let mut q = p;
        let mut yaw = place::yaw_deg(&p);
        let yaw_before = yaw;
        let mut changed = false;
        ui.horizontal(|ui| {
            for (k, name) in ["x", "y", "z"].iter().enumerate() {
                ui.label(*name);
                changed |= ui.add(egui::DragValue::new(&mut q.pos[k]).speed(0.1)).changed();
            }
        });
        ui.horizontal(|ui| {
            ui.label("heading");
            changed |= ui.add(egui::DragValue::new(&mut yaw).speed(1.0).suffix(" deg")).on_hover_text(crate::help_text::tip("place-yaw")).changed();
        });
        let mut action = None;
        ui.horizontal(|ui| {
            if ui.button("Snap to ground").on_hover_text(crate::help_text::tip("place-snap")).clicked() {
                action = Some(0);
            }
            if ui.button("Duplicate").clicked() {
                action = Some(1);
            }
            if ui.button("Delete").clicked() {
                action = Some(2);
            }
        });
        let mut toast = None;
        if changed && sel.len() == 1 {
            if let Some(d) = self.docs.get_mut(self.active) {
                if yaw != yaw_before {
                    q = place::with_yaw(&q, yaw);
                }
                let ops = place::replace_ops(&d.part().expect("checked").wld.model, &[(first, q)]);
                if let Err(e) = d.commit_client_coalesced("place-edit", "edit placement", ops) {
                    toast = Some(e);
                }
            }
        }
        match action {
            Some(0) => self.place_snap(),
            Some(1) => self.place_duplicate(),
            Some(2) => self.place_delete(),
            _ => {}
        }
        if let Some(t) = toast {
            self.toast(t);
        }
    }
}

/// The width of the name column of the Models window, points.
const NAME_W: f32 = 230.0;

fn name_galley_w(ui: &egui::Ui, s: &str) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE).size().x
}

/// `name`, cut with an ellipsis so it is not wider than `width`.
fn fit_name(ui: &egui::Ui, name: &str, width: f32) -> String {
    let chars: Vec<char> = name.chars().collect();
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "...";
        if name_galley_w(ui, &s) <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "..."
}

/// A line under the picture when part of the model is not drawn textured: textures the client lacks (plain grey), or
/// textures without an opaque texel (the game draws nothing there, so only the wire shows them).
fn preview_note(p: &pu::Preview) -> Option<String> {
    let mut parts = Vec::new();
    if !p.missing.is_empty() {
        parts.push(format!("grey: texture not in the client ({})", p.missing.join(", ")));
    }
    if !p.invisible.is_empty() {
        parts.push(format!("wire only: texture fully see-through ({})", p.invisible.join(", ")));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The hover text of the picture: how much of it is textured, and which textures have see-through texels (cut out).
fn preview_tip(p: &pu::Preview) -> Option<String> {
    let pic = p.picture.as_ref()?;
    let total = (pic.textured_px + pic.plain_px).max(1) as f32;
    let mut t = format!("{:.0} % textured, {:.0} % plain grey", pic.textured_px as f32 * 100.0 / total, pic.plain_px as f32 * 100.0 / total);
    if !p.see_through.is_empty() {
        t += &format!("\nSee-through texels cut out of: {}", p.see_through.join(", "));
    }
    Some(t)
}

/// The model's picture (textured, lit, with its visible edges as a wire; rendered once by `place_ui::render_picture`) in the
/// middle of the rect. A model without a picture (no faces: a dungeon piece, or a file that cannot be read) shows its edges, or
/// else its box, as a wire in the same view.
fn draw_preview(p: egui::Painter, rect: egui::Rect, pv: Option<&mut pu::Preview>) {
    p.rect_filled(rect, 4.0, egui::Color32::from_rgb(28, 30, 36));
    let Some(pv) = pv else {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "no model chosen", egui::FontId::proportional(12.0), egui::Color32::GRAY);
        return;
    };
    // the picture keeps the preview's size and sits in the middle of the rect
    let origin = rect.center() - egui::vec2(pu::PREVIEW_SIZE[0], pu::PREVIEW_SIZE[1]) * 0.5;
    if let Some(pic) = &pv.picture {
        let tex = pv.tex.get_or_insert_with(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied([pic.width, pic.height], &pic.rgba);
            p.ctx().load_texture("model-preview", img, egui::TextureOptions::LINEAR)
        });
        let r = egui::Rect::from_min_size(origin, egui::vec2(pu::PREVIEW_SIZE[0], pu::PREVIEW_SIZE[1]));
        p.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        return;
    }
    let mut edges = pv.edges.clone();
    if edges.is_empty() {
        if let Some(b) = pv.bounds {
            let c = place::world_corners(&place::upright(0, [0.0; 3], 0.0), &b);
            for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
                edges.push([c[i], c[j]]);
            }
        }
    }
    let Some(fit) = pu::PreviewFit::of(edges.iter().flatten().copied()) else { return };
    let to = |v: [f32; 3]| {
        let q = fit.to_points(v);
        origin + egui::vec2(q[0], q[1])
    };
    // one mesh of thin quads: thousands of separate line shapes cost milliseconds of tessellation every frame
    let col = egui::Color32::from_rgb(170, 200, 230);
    let mut mesh = egui::Mesh::default();
    for e in &edges {
        let (a, b) = (to(e[0]), to(e[1]));
        let d = b - a;
        let len = d.length();
        if !(len >= 0.01 && len.is_finite()) {
            continue;
        }
        let n = egui::vec2(-d.y, d.x) / len * 0.5;
        let i = mesh.vertices.len() as u32;
        for q in [a + n, a - n, b - n, b + n] {
            mesh.colored_vertex(q, col);
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_ui(f: impl FnOnce(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        let mut f = Some(f);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui);
            }
        });
        out.textures_delta.clear();
    }

    /// A long model name is cut to the fixed width (with an ellipsis); a short one is left alone, whatever else is visible.
    #[test]
    fn names_fit_the_fixed_column() {
        in_ui(|ui| {
            let long = "a_very_long_building_name_that_would_stretch_the_column_to_the_right_0123456789.smod";
            let w = NAME_W - 14.0;
            assert!(name_galley_w(ui, long) > w);
            let cut = fit_name(ui, long, w);
            assert!(cut.ends_with("...") && cut.len() < long.len() && name_galley_w(ui, &cut) <= w, "{cut}");
            assert!(name_galley_w(ui, "house.smod") <= w);
        });
    }

    /// The preview uploads its picture once (the second frame reuses the texture); a model without faces draws its box as a
    /// wire; no model draws the hint.
    #[test]
    fn textured_preview_uploads_once() {
        let tri = shaiya_clientdata::render::PreviewTri { pos: [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]], uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], tex: None };
        let fit = pu::PreviewFit::of(tri.pos.into_iter()).unwrap();
        let picture = Some(shaiya_clientdata::render::render_picture(std::slice::from_ref(&tri), &[], &[[tri.pos[0], tri.pos[1]]], fit));
        let mut pv = pu::Preview { edges: vec![[tri.pos[0], tri.pos[1]]], bounds: None, picture, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), tex: None };
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(pu::PREVIEW_SIZE[0], pu::PREVIEW_SIZE[1]));
        let ctx = egui::Context::default();
        let frame = |pv: Option<&mut pu::Preview>| {
            let mut pv = Some(pv);
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| draw_preview(ui.painter_at(rect), rect, pv.take().flatten()));
            out.textures_delta.clear();
        };
        frame(Some(&mut pv));
        let id = pv.tex.as_ref().expect("uploaded").id();
        frame(Some(&mut pv));
        assert_eq!(pv.tex.as_ref().unwrap().id(), id, "the same texture the second time");
        assert_eq!(preview_tip(&pv).as_deref(), Some("0 % textured, 100 % plain grey"));
        assert_eq!(preview_note(&pv), None);
        pv.missing.push("gone.tga".to_string());
        pv.invisible.push("box.tga".to_string());
        assert_eq!(preview_note(&pv).as_deref(), Some("grey: texture not in the client (gone.tga); wire only: texture fully see-through (box.tga)"));
        let mut boxed = pu::Preview { edges: Vec::new(), bounds: Some(place::Bounds { min: [-1.0; 3], max: [1.0; 3] }), picture: None, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), tex: None };
        frame(Some(&mut boxed));
        assert!(boxed.tex.is_none());
        frame(None);
    }
}
