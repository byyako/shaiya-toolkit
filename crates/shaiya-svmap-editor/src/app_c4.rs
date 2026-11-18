//! The client side in the window: the Terrain tool. The toolbar options, the inspector page, the picture and the ground a
//! tab with terrain edits sees, and the picture for `--show terrain`. Declared from `app.rs` with `#[path]`. The pure parts are in
//! `terrain.rs` (brushes, re-drop, slope) and `terrain_ui.rs` (stroke, one undo step).

use std::sync::Arc;

use egui::{ColorImage, RichText, TextureHandle, TextureOptions};
use shaiya_clientdata::wld::Terrain;
use shaiya_clientdata::wld_raw::RawTerrain;

use super::App;
use crate::assets::{self, MapAssets};
use crate::pair::{ClientPart, TerrainDirty};
use crate::terrain::{self, Brush};
use crate::terrain_ui::TerrainUi;
use crate::view2d::Tool;

/// What a tab with terrain edits shows instead of the shared map assets: its own hillshade picture (redrawn where the heights
/// changed) and assets whose ground follows the edits. Made when the tab first has terrain edits, dropped when its client half goes.
pub struct TerrView {
    serial: u64,
    /// The terrain revision the picture shows.
    seen: u64,
    rgba: Vec<u8>,
    n: usize,
    handle: Option<TextureHandle>,
    pub assets: Arc<MapAssets>,
    assets_rev: u64,
}

fn picture_options() -> TextureOptions {
    TextureOptions { magnification: egui::TextureFilter::Linear, minification: egui::TextureFilter::Linear, wrap_mode: egui::TextureWrapMode::ClampToEdge, mipmap_mode: Some(egui::TextureFilter::Linear) }
}

impl TerrView {
    /// Whether this view belongs to the client half `p` (a half read again has other corners).
    pub fn is_for(&self, p: &ClientPart) -> bool {
        self.serial == p.version().0
    }

    fn new(serial: u64, base: &MapAssets, ctx: &egui::Context) -> TerrView {
        let (rgba, n, handle) = match &base.terrain {
            Some(p) => (p.rgba.clone(), p.width, Some(ctx.load_texture("terrain-edit", ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba), picture_options()))),
            None => (Vec::new(), 0, None),
        };
        TerrView { serial, seen: 0, rgba, n, handle, assets: Arc::new(base.with_ground(Terrain { size: 0, n: 2, heights: vec![0; 4], tex_index: vec![0; 4] })), assets_rev: u64::MAX }
    }

    /// Recolours the corners of `rect` (and their neighbours: the shade reads them), or everything for `None`, and sends only
    /// that part of the picture to the graphics card.
    fn redraw(&mut self, t: &RawTerrain, tints: &[[u8; 3]], rect: Option<[u32; 4]>) {
        let n = self.n;
        if n == 0 || n != t.corners() || self.rgba.len() != n * n * 4 {
            return;
        }
        let last = n as u32 - 1;
        let r = match rect {
            Some(r) => [r[0].saturating_sub(1), r[1].saturating_sub(1), (r[2] + 1).min(last), (r[3] + 1).min(last)],
            None => [0, 0, last, last],
        };
        assets::shade_rect(&t.heights, &t.tex_index, n, tints, r, &mut self.rgba);
        let Some(h) = self.handle.as_mut() else { return };
        if rect.is_none() {
            h.set(ColorImage::from_rgba_unmultiplied([n, n], &self.rgba), picture_options());
            return;
        }
        // pixel (cx, n - 1 - cz) is corner (cx, cz): the rectangle's rows run from the top for the largest z
        let (x0, x1) = (r[0] as usize, r[2] as usize);
        let (y0, y1) = (n - 1 - r[3] as usize, n - 1 - r[1] as usize);
        let (w, hgt) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut sub = Vec::with_capacity(w * hgt * 4);
        for y in y0..=y1 {
            sub.extend_from_slice(&self.rgba[(y * n + x0) * 4..(y * n + x1 + 1) * 4]);
        }
        h.set_partial([x0, y0], ColorImage::from_rgba_unmultiplied([w, hgt], &sub), picture_options());
    }
}

impl App {
    /// Every frame: drops a stroke that lost its tab or its history, forgets the views of closed tabs and, for the active tab with
    /// terrain edits, redraws the picture where the heights changed and rebuilds the ground its lookups use.
    pub(super) fn sync_terrain_views(&mut self, ctx: &egui::Context) {
        let active_uid = self.docs.get(self.active).map(|d| d.uid);
        let button_down = ctx.input(|i| i.pointer.primary_down());
        if let Some(msg) = self.terrain.guard(&mut self.docs, active_uid, self.tool == Tool::Terrain, button_down) {
            self.toast(msg);
        }
        let live: std::collections::HashMap<u64, u64> = self.docs.iter().filter_map(|d| Some((d.uid, d.part()?.version().0))).collect();
        self.terr_views.retain(|uid, v| live.get(uid) == Some(&v.serial));
        let Some(d) = self.docs.get(self.active) else { return };
        let (uid, map_id) = (d.uid, d.map_id);
        let Some(p) = d.part() else { return };
        let Some(t) = p.terrain() else { return };
        let rev = p.terrain_rev();
        if rev == 0 && !self.terr_views.contains_key(&uid) {
            return;
        }
        let Some(base) = self.ready_for(map_id).map(|r| r.assets.clone()) else { return };
        let v = self.terr_views.entry(uid).or_insert_with(|| TerrView::new(p.version().0, &base, ctx));
        if v.seen < rev {
            match p.terrain_dirty_since(v.seen) {
                TerrainDirty::Nothing => {}
                TerrainDirty::Rect(r) => v.redraw(t, &base.tex_tints, Some(r)),
                TerrainDirty::Full => v.redraw(t, &base.tex_tints, None),
            }
            v.seen = rev;
        }
        if v.assets_rev != rev {
            v.assets = Arc::new(base.with_ground(Terrain { size: t.size as u32, n: t.corners(), heights: t.heights.clone(), tex_index: t.tex_index.clone() }));
            v.assets_rev = rev;
        }
    }

    /// The redrawn terrain picture of tab `i`, when it has one.
    pub(super) fn terrain_picture(&self, i: usize) -> Option<TextureHandle> {
        let d = self.docs.get(i)?;
        let v = self.terr_views.get(&d.uid)?;
        if v.is_for(d.part()?) {
            v.handle.clone()
        } else {
            None
        }
    }

    /// Why the Terrain tool cannot edit the active tab (`None` when it can).
    pub(super) fn terrain_blocked(&self) -> Option<&'static str> {
        let d = self.docs.get(self.active)?;
        match &d.client {
            crate::doc::ClientSlot::Part(p) if p.terrain().is_some() => None,
            crate::doc::ClientSlot::Part(_) => Some("dungeon map: no terrain"),
            crate::doc::ClientSlot::ReadOnly(_) => Some("the client map is read-only"),
            crate::doc::ClientSlot::None => Some("needs the client map (open the client folder)"),
        }
    }

    /// Toolbar options of the Terrain tool.
    pub(super) fn terrain_toolbar(&mut self, ui: &mut egui::Ui) {
        let t = &mut self.terrain;
        egui::ComboBox::from_id_salt("terrain-brush").selected_text(t.params.brush.label()).width(96.0).show_ui(ui, |ui| {
            for b in Brush::ALL {
                ui.selectable_value(&mut t.params.brush, b, b.label());
            }
        }).response.on_hover_text(crate::help_text::tip("terrain-brush"));
        ui.label("radius").on_hover_text(crate::help_text::tip("terrain-radius"));
        ui.add(egui::DragValue::new(&mut t.params.radius).speed(0.5).range(terrain::MIN_RADIUS..=terrain::MAX_RADIUS));
        ui.label("falloff").on_hover_text(crate::help_text::tip("terrain-falloff"));
        ui.add(egui::DragValue::new(&mut t.params.falloff).speed(0.01).range(0.0..=1.0).fixed_decimals(2));
        match t.params.brush {
            Brush::Raise | Brush::Lower | Brush::Flatten | Brush::Smooth => {
                ui.label("amount").on_hover_text(crate::help_text::tip("terrain-amount"));
                ui.add(egui::DragValue::new(&mut t.params.amount).speed(0.01).range(0.05..=1.0).fixed_decimals(2));
            }
            Brush::Set => {
                ui.label("height").on_hover_text(crate::help_text::tip("terrain-height"));
                ui.add(egui::DragValue::new(&mut t.params.height).speed(0.1).range(terrain::units_of(i16::MIN)..=terrain::units_of(i16::MAX)).fixed_decimals(1));
            }
            Brush::Ramp => {}
        }
        ui.checkbox(&mut t.keep_on_ground, "Keep on ground").on_hover_text(crate::help_text::tip("terrain-keep"));
        ui.checkbox(&mut t.reblock, "Re-block by slope").on_hover_text(crate::help_text::tip("terrain-reblock"));
        if t.reblock {
            ui.add(egui::DragValue::new(&mut t.slope_deg).speed(0.5).range(5.0..=89.0).suffix(" deg"));
        }
    }

    /// The inspector page of the Terrain tool.
    pub(super) fn terrain_inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("Terrain");
        if let Some(why) = self.terrain_blocked() {
            ui.label(format!("The Terrain tool cannot edit this tab: {why}."));
            return;
        }
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(t) = d.part().and_then(|p| p.terrain()) else { return };
        // the range is counted again only when the heights changed (a scan of a million corners is not a per-frame job)
        let key = d.part().map(|p| (p.version().0, p.terrain_rev())).unwrap_or_default();
        let (lo, hi) = match self.terrain.range {
            Some((k, r)) if k == key || self.terrain.stroke.is_some() => r,
            _ => {
                let r = t.heights.iter().fold((i16::MAX, i16::MIN), |(lo, hi), h| (lo.min(*h), hi.max(*h)));
                self.terrain.range = Some((key, r));
                r
            }
        };
        ui.label(format!("{0} x {0} corners, one every 2 units", t.corners()));
        ui.label(format!("heights {:.1} to {:.1}", terrain::units_of(lo), terrain::units_of(hi)));
        // the count compares a million corners with the saved ones: while a stroke is drawn (new heights every frame) the line
        // keeps its last value
        if self.terrain.stroke.is_none() {
            self.terrain.changed_line = d.part().and_then(|p| p.changes().into_iter().find(|l| l.starts_with("client terrain")));
        }
        if let Some(l) = &self.terrain.changed_line {
            ui.label(RichText::new(l).strong());
        }
        ui.separator();
        ui.label("Drag on the map to paint. Shift turns Raise into Lower. [ and ] change the radius. Esc cancels the stroke.");
        ui.label(format!("Lowering a corner below {} makes it water in the game.", terrain::WATER_LEVEL));
        ui.label("One stroke is one undo step. Ctrl+S writes the client map to the staging folder.");
    }

    /// `--show terrain`: the Terrain tool after a hill and a flat top: the stroke is one step, the ring is shown at the hill.
    pub(super) fn demo_terrain(&mut self) {
        let Some(uid) = self.docs.get(self.active).map(|d| d.uid) else { return };
        self.attach_client_now(uid);
        self.tool = Tool::Terrain;
        let Some(d) = self.docs.get_mut(self.active) else { return };
        if TerrainUi::check(d).is_err() {
            eprintln!("--show terrain needs a field map with an editable client .wld (open the client folder)");
            return;
        }
        let size = d.map.size as f32;
        let at = [size * 0.5, size * 0.5];
        let ground = d.part().and_then(|p| p.terrain()).map(|t| terrain::Field::new(t.corners(), &t.heights).height_at(at[0], at[1])).unwrap_or(0.0);
        let t = &mut self.terrain;
        t.params = terrain::Params { brush: Brush::Set, radius: 30.0, falloff: 0.7, amount: 0.5, height: ground + 22.0 };
        if t.begin(d, at, false).is_ok() {
            let _ = t.finish(d);
        }
        t.params = terrain::Params { brush: Brush::Raise, radius: 16.0, falloff: 0.7, amount: 1.0, height: 0.0 };
        if t.begin(d, [at[0] + 18.0, at[1] + 6.0], false).is_ok() {
            for _ in 0..7 {
                t.drag(d, [at[0] + 18.0, at[1] + 6.0], 4.0);
            }
            let _ = t.finish(d);
        }
        t.params.brush = Brush::Smooth;
        t.pin = Some([at[0] - 6.0, at[1] - 6.0]);
        d.view.cx = at[0];
        d.view.cz = at[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picture a tab redraws for an edit (the corners and their neighbours) is the picture a full redraw gives.
    #[test]
    fn a_redrawn_rectangle_equals_a_full_redraw() {
        let n = 24usize;
        let mut t = RawTerrain { size: (n as i32 - 1) * 2, heights: (0..n * n).map(|i| (9000 + (i * 29) % 60) as i16).collect(), tex_index: (0..n * n).map(|i| (i % 2) as u8).collect() };
        let tints = [[220u8, 90, 40], [40, 180, 120]];
        let mut v = TerrView { serial: 1, seen: 0, rgba: vec![255; n * n * 4], n, handle: None, assets: Arc::new(MapAssets::empty()), assets_rev: 0 };
        v.redraw(&t, &tints, None);
        for cz in 8..13 {
            for cx in 5..11 {
                t.heights[cz * n + cx] += 500;
            }
        }
        let mut full = TerrView { serial: 1, seen: 0, rgba: vec![255; n * n * 4], n, handle: None, assets: Arc::new(MapAssets::empty()), assets_rev: 0 };
        full.redraw(&t, &tints, None);
        assert_ne!(v.rgba, full.rgba);
        v.redraw(&t, &tints, Some([5, 8, 10, 12]));
        assert_eq!(v.rgba, full.rgba);
    }
}
