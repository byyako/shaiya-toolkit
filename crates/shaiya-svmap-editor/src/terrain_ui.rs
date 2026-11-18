//! The Terrain tool between the pure brushes of `terrain.rs` and the two viewports. A stroke draws live into the
//! tab's client model (corner by corner, with no undo step) and ends as one step: a corner diff, plus the re-drop of the records
//! that stand on the ground and the blocked cells whose slope class changed. Nothing here writes a file; the save is the normal
//! staged client save (C1).

use std::time::{Duration, Instant};

use crate::doc::Doc;
use crate::gpu3d::{OVert, OverlayGeo};
use crate::pair::COp;
use crate::terrain::{self, Brush, Field, Params, StrokeAcc};

/// The corner rectangles the preview of the re-blocked cells is worth computing for (cells, not corners).
const PREVIEW_MAX_CELLS: u64 = 300_000;
/// Most flipped cells the 2D view draws.
pub const PREVIEW_DRAW_MAX: usize = 20_000;

/// A stroke in progress.
pub struct Stroke {
    uid: u64,
    /// The client half the stroke started on (a half read again would hold other corners).
    serial: u64,
    /// The tab's edit counter at the start: an undo, a redo, a new or merged step or a reload during the stroke cancels it. (The
    /// history depth alone misses a merged step, a step pushed at the undo limit and an undo followed by a new step.)
    gen: u64,
    acc: StrokeAcc,
    brush: Brush,
    /// Flatten: the height sampled where the stroke began.
    target: f32,
    start: [f32; 2],
    last: [f32; 2],
    /// Ramp: the ground height at the start.
    ramp_a: f32,
}

/// What a finished stroke did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub corners: usize,
    pub moved: usize,
    pub flipped: usize,
    pub underwater: usize,
}

impl Report {
    pub fn message(&self) -> String {
        let mut s = format!("Terrain: {} corner{} changed", self.corners, if self.corners == 1 { "" } else { "s" });
        if self.moved > 0 {
            s += &format!(", {} record{} kept on the ground", self.moved, if self.moved == 1 { "" } else { "s" });
        }
        if self.flipped > 0 {
            s += &format!(", {} blocked cell{} changed by slope", self.flipped, if self.flipped == 1 { "" } else { "s" });
        }
        s.push('.');
        if self.underwater > 0 {
            s += &format!(" {} corner{} below -0.4: the game treats that as water.", self.underwater, if self.underwater == 1 { " is" } else { "s are" });
        }
        s
    }
}

pub struct TerrainUi {
    pub params: Params,
    /// Re-drop buildings, shapes, trees, NPCs and portals inside the stroke (a stroke moves them by the change of the ground).
    pub keep_on_ground: bool,
    /// Change the blocked cells whose slope class changed.
    pub reblock: bool,
    pub slope_deg: f32,
    pub stroke: Option<Stroke>,
    /// Where the brush ring is drawn (world x, z).
    pub hover: Option<[f32; 2]>,
    /// A ring that stays where it is when the pointer is not over the map (the pictures of `--show terrain`).
    pub pin: Option<[f32; 2]>,
    /// The lowest and highest raw height of the map the Inspector shows, with the (part serial, terrain revision) they were counted for.
    pub range: Option<((u64, u64), (i16, i16))>,
    /// The cells a stroke would flip, for the 2D view: (x, z, blocked).
    pub preview: Vec<(u32, u32, bool)>,
    preview_at: Option<Instant>,
    /// The Inspector's "client terrain: n corners changed" line, kept while a stroke is drawn.
    pub changed_line: Option<String>,
}

impl Default for TerrainUi {
    fn default() -> Self {
        TerrainUi { params: Params::default(), keep_on_ground: true, reblock: false, slope_deg: 50.0, stroke: None, hover: None, pin: None, range: None, preview: Vec::new(), preview_at: None, changed_line: None }
    }
}

impl TerrainUi {
    /// Whether the tab can be edited here, or why not.
    pub fn check(doc: &Doc) -> Result<(), String> {
        doc.client_edit_check()?;
        match doc.part().and_then(|p| p.terrain()) {
            Some(_) => Ok(()),
            None => Err("this map has no terrain (a dungeon)".to_string()),
        }
    }

    /// Starts a stroke at the world point `at`. `invert` (Shift) turns Raise into Lower and back.
    pub fn begin(&mut self, doc: &mut Doc, at: [f32; 2], invert: bool) -> Result<(), String> {
        Self::check(doc)?;
        if self.stroke.is_some() {
            return Ok(());
        }
        let Some(part) = doc.part() else { return Err("no client map".to_string()) };
        let Some(t) = part.terrain() else { return Err("this map has no terrain".to_string()) };
        let n = t.corners();
        let f = Field::new(n, &t.heights);
        let h0 = f.height_at(at[0], at[1]);
        if !(at[0].is_finite() && at[1].is_finite()) {
            return Err("the brush is not over the map".to_string());
        }
        self.params = self.params.sane();
        let brush = self.params.brush.inverted(invert);
        self.stroke = Some(Stroke { uid: doc.uid, serial: part.version().0, gen: doc.edit_gen, acc: StrokeAcc::default(), brush, target: h0, start: at, last: at, ramp_a: h0 });
        self.preview.clear();
        self.preview_at = None;
        self.drag(doc, at, 1.0);
        Ok(())
    }

    /// Whether the stroke is still on the tab and the history it started on.
    fn valid(st: &Stroke, doc: &Doc) -> bool {
        st.uid == doc.uid && doc.part().is_some_and(|p| p.version().0 == st.serial) && doc.edit_gen == st.gen
    }

    /// Whether a stroke is being drawn on the tab `uid` (its live heights are not a step yet: saves and autosave wait).
    pub fn stroking(&self, uid: u64) -> bool {
        self.stroke.as_ref().is_some_and(|s| s.uid == uid)
    }
    /// Moves the brush to `at`. `steps` is the time since the last call in 1/60 s (how much a held brush does).
    pub fn drag(&mut self, doc: &mut Doc, at: [f32; 2], steps: f32) {
        let Some(st) = self.stroke.as_mut() else { return };
        if !Self::valid(st, doc) {
            self.abort(doc);
            return;
        }
        if !(at[0].is_finite() && at[1].is_finite()) {
            return;
        }
        // a frame that took very long (or a bad time value) must not become one huge dab
        let steps = if steps.is_finite() { steps.clamp(0.0, 8.0) } else { 1.0 };
        let params = Params { brush: st.brush, ..self.params.sane() };
        let Some(part) = doc.part_mut() else { return };
        let mut rect = None;
        match st.brush {
            Brush::Ramp => {
                let (acc, a, ha) = (&mut st.acc, st.start, st.ramp_a);
                part.edit_terrain_live(|t, live| {
                    let n = t.corners();
                    let hb = Field::before(n, &t.heights, &acc.before).height_at(at[0], at[1]);
                    let r = terrain::ramp(&mut t.heights, n, &params, acc, a, at, ha, hb);
                    acc.flush_into(live);
                    r
                });
                st.last = at;
            }
            _ => {
                // dabs along the way from the last point, at most a third of the radius apart, so a fast drag leaves no gaps
                let (from, to) = (st.last, at);
                let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
                let spacing = (params.radius / 3.0).max(1.0);
                let k = ((len / spacing).ceil() as usize).clamp(1, 64);
                let per = (steps / k as f32).max(0.0);
                let (acc, target) = (&mut st.acc, st.target);
                part.edit_terrain_live(|t, live| {
                    let n = t.corners();
                    for j in 1..=k {
                        let f = j as f32 / k as f32;
                        let p = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
                        if let Some(r) = terrain::dab(&mut t.heights, n, &params, acc, p, per, target) {
                            rect = Some(match rect {
                                None => r,
                                Some(o) => union(o, r),
                            });
                        }
                    }
                    acc.flush_into(live);
                    rect
                });
                st.last = at;
            }
        }
    }

    /// Ends the stroke: one undo step with everything it did. `None` when the corners came back to where they started.
    pub fn finish(&mut self, doc: &mut Doc) -> Result<Option<Report>, String> {
        let Some(mut st) = self.stroke.take() else { return Ok(None) };
        self.preview.clear();
        if !Self::valid(&st, doc) {
            // the history moved under the stroke: put back what the stroke wrote and change nothing else
            revert(doc, &mut st);
            return Err("The stroke was cancelled: the history changed while it was drawn.".to_string());
        }
        let (ops, cops, report) = {
            let Some(part) = doc.part() else { return Ok(None) };
            let Some(t) = part.terrain() else { return Ok(None) };
            let n = t.corners();
            let diff = st.acc.diff(&t.heights);
            if diff.is_empty() {
                // the corners came back: the record of the drawn corners goes too (it would put stale values back later)
                revert(doc, &mut st);
                return Ok(None);
            }
            let rect = diff.rect(n).expect("a diff has corners");
            let new = Field::new(n, &t.heights);
            let old = Field::before(n, &t.heights, &st.acc.before);
            let mut report = Report { corners: diff.cells.len(), underwater: diff.newly_underwater(), ..Default::default() };
            let mut ops = Vec::new();
            let mut cops = vec![COp::Terrain(diff.clone())];
            if self.keep_on_ground {
                let r = terrain::redrop(&doc.map, &part.wld.model, &old, &new, rect);
                report.moved = r.moved;
                ops.extend(r.ops);
                cops.extend(r.cops);
            }
            if self.reblock {
                let flips = terrain::reblock_flips(&doc.map, &old, &new, rect, self.slope_deg);
                if let Some(op) = terrain::flips_op(&doc.map, &flips) {
                    report.flipped = flips.len();
                    ops.push(op);
                }
            }
            (ops, cops, report)
        };
        // the tab may have become read-only while the button was down: nothing is committed, so nothing may stay drawn
        if let Err(e) = doc.client_edit_check() {
            revert(doc, &mut st);
            return Err(e);
        }
        // the drawn corners become the step (applying its diff writes the same values again)
        if let Some(p) = doc.part_mut() {
            p.keep_live();
        }
        if let Err(e) = doc.commit_both("terrain brush", ops, cops) {
            // cannot happen after the check above; put the corners back from the stroke's own record
            if let Some(p) = doc.part_mut() {
                p.edit_terrain_live(|_, live| {
                    live.extend(st.acc.before.iter().map(|(i, v)| (*i, *v)));
                    None
                });
            }
            revert(doc, &mut st);
            return Err(e);
        }
        Ok(Some(report))
    }

    /// Puts back what the stroke wrote (Esc, or the history changed under it).
    pub fn abort(&mut self, doc: &mut Doc) -> bool {
        let Some(mut st) = self.stroke.take() else { return false };
        self.preview.clear();
        revert(doc, &mut st);
        true
    }

    /// Called every frame: a stroke whose tab is gone, or that changed under it, or whose tool was switched away while the button
    /// was down, is dropped; a stroke whose button is no longer down (the release was not seen by the view that started it) ends
    /// as its step.
    pub fn guard(&mut self, docs: &mut [Doc], active_uid: Option<u64>, tool_is_terrain: bool, button_down: bool) -> Option<String> {
        let st = self.stroke.as_ref()?;
        let Some(d) = docs.iter_mut().find(|d| d.uid == st.uid) else {
            self.stroke = None;
            return None;
        };
        if active_uid != Some(st.uid) || !Self::valid(st, d) || !tool_is_terrain {
            self.abort(d);
            return Some("The terrain stroke was cancelled.".to_string());
        }
        if !button_down {
            return match self.finish(d) {
                Ok(Some(r)) => Some(r.message()),
                Ok(None) => None,
                Err(e) => Some(e),
            };
        }
        None
    }

    /// The cells the stroke would flip, recomputed at most every 120 ms while it goes on (for the 2D view).
    pub fn update_preview(&mut self, doc: &Doc) {
        if !self.reblock {
            self.preview.clear();
            return;
        }
        let Some(st) = self.stroke.as_ref() else { return };
        if self.preview_at.is_some_and(|t| t.elapsed() < Duration::from_millis(120)) {
            return;
        }
        self.preview_at = Some(Instant::now());
        self.preview.clear();
        let (Some(part), Some(rect)) = (doc.part(), st.acc.rect()) else { return };
        let Some(t) = part.terrain() else { return };
        let cells = ((rect[2] - rect[0] + 8) as u64 * 2) * ((rect[3] - rect[1] + 8) as u64 * 2);
        if cells > PREVIEW_MAX_CELLS {
            return;
        }
        let n = t.corners();
        let new = Field::new(n, &t.heights);
        let old = Field::before(n, &t.heights, &st.acc.before);
        self.preview = terrain::reblock_flips(&doc.map, &old, &new, rect, self.slope_deg);
    }

    /// The start of the ramp being drawn.
    pub fn stroke_ramp(&self) -> Option<[f32; 2]> {
        self.stroke.as_ref().filter(|s| s.brush == Brush::Ramp).map(|s| s.start)
    }

    /// The ring of the brush in the 3D view: the radius, and the part with full strength inside it. Follows the ground.
    pub fn overlay_3d(&self, o: &mut OverlayGeo, ground: &dyn Fn(f32, f32) -> f32) {
        let Some(c) = self.hover else { return };
        let p = &self.params;
        let ring = |o: &mut OverlayGeo, r: f32, col: [u8; 4]| {
            if r < 0.5 {
                return;
            }
            let segs = 64;
            let pts: Vec<[f32; 3]> = (0..=segs)
                .map(|i| {
                    let a = i as f32 / segs as f32 * std::f32::consts::TAU;
                    let (x, z) = (c[0] + a.cos() * r, c[1] + a.sin() * r);
                    [x, ground(x, z) + 0.4, z]
                })
                .collect();
            for w in pts.windows(2) {
                o.lines_top.push(OVert { pos: w[0], col });
                o.lines_top.push(OVert { pos: w[1], col });
            }
        };
        ring(o, p.radius, [255, 235, 60, 255]);
        ring(o, p.radius * (1.0 - p.falloff.clamp(0.0, 1.0)), [255, 255, 255, 170]);
    }
}

fn union(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn revert(doc: &mut Doc, st: &mut Stroke) {
    if doc.uid != st.uid {
        return;
    }
    let Some(part) = doc.part_mut() else { return };
    if part.version().0 != st.serial {
        return;
    }
    // the half keeps the record of the drawn corners (an undo that arrived during the stroke has already put them back)
    part.revert_live();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::paired_doc;
    use shaiya_clientdata::wld::HEIGHT_OFFSET;
    use shaiya_clientdata::wld_raw::WldRaw;
    use shaiya_svmap::{NpcEntry, NpcPoint};
    use crate::terrain::TerrainDiff;

    fn bytes(d: &Doc) -> Vec<u8> {
        d.part().unwrap().wld.model.to_bytes()
    }

    fn heights(d: &Doc) -> Vec<i16> {
        d.part().unwrap().terrain().unwrap().heights.clone()
    }

    /// A stroke of `steps` dabs of `brush` from (a) to (b).
    fn stroke(ui: &mut TerrainUi, d: &mut Doc, a: [f32; 2], b: [f32; 2], frames: usize) -> Result<Option<Report>, String> {
        ui.begin(d, a, false)?;
        for i in 1..=frames {
            let f = i as f32 / frames as f32;
            ui.drag(d, [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f], 1.0);
        }
        ui.finish(d)
    }

    #[test]
    fn terrain_stroke_undo_bytes() {
        let mut d = paired_doc();
        let before = bytes(&d);
        let h0 = heights(&d);
        let mut ui = TerrainUi { keep_on_ground: false, ..Default::default() };
        ui.params = Params { brush: Brush::Raise, radius: 10.0, falloff: 0.5, amount: 1.0, height: 0.0 };
        let r = stroke(&mut ui, &mut d, [20.0, 20.0], [40.0, 24.0], 12).unwrap().expect("the stroke changed corners");
        assert!(r.corners > 20, "{r:?}");
        // one stroke = one undo step, and the file stays the same length with the changes only in the heights
        assert_eq!(d.undo.len(), 1);
        let after = bytes(&d);
        assert_eq!(after.len(), before.len(), "the written length does not change (a short .wld inside the archive reads garbage)");
        let first = before.iter().zip(&after).position(|(a, b)| a != b).unwrap();
        let last = before.iter().zip(&after).rposition(|(a, b)| a != b).unwrap();
        let t = d.part().unwrap().terrain().unwrap();
        let n = t.corners();
        let diffs = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert!(diffs <= 2 * r.corners, "only corner heights changed: {diffs} bytes for {} corners", r.corners);
        assert!(last - first < 2 * n * n, "the changes lie inside the height block");
        assert_ne!(heights(&d), h0);
        assert!(d.client_dirty() && !d.svmap_dirty(), "a terrain stroke dirties the client map only");
        // the step is on the stack: undo gives the original bytes, redo the edited ones
        d.undo();
        assert_eq!(bytes(&d), before, "undo gives back the exact original file");
        assert!(!d.client_dirty());
        d.redo();
        assert_eq!(bytes(&d), after);
        d.undo();
        assert_eq!(bytes(&d), before);
        // the model still validates and reads back
        let mut again = paired_doc();
        let _ = &mut again;
        let back = WldRaw::parse(&after, "edited").unwrap();
        assert!(back.validate().is_empty());
        assert_eq!(back.to_bytes(), after);
    }

    #[test]
    fn a_click_is_one_step_and_a_stroke_back_to_the_start_is_none() {
        let mut d = paired_doc();
        let mut ui = TerrainUi::default();
        ui.params.brush = Brush::Raise;
        ui.begin(&mut d, [30.0, 30.0], false).unwrap();
        assert!(ui.finish(&mut d).unwrap().is_some());
        assert_eq!(d.undo.len(), 1);
        // set down and set back to where it was in one stroke: nothing to commit
        ui.params = Params { brush: Brush::Set, radius: 6.0, falloff: 0.0, amount: 1.0, height: 40.0 };
        let h = heights(&d);
        ui.begin(&mut d, [10.0, 10.0], false).unwrap();
        assert_ne!(heights(&d), h, "the stroke draws live");
        assert_eq!(d.undo.len(), 1, "but no step is made before it ends");
        ui.abort(&mut d);
        assert_eq!(heights(&d), h, "Esc puts the corners back");
        assert_eq!(d.undo.len(), 1);
    }

    #[test]
    fn an_undo_during_a_stroke_cancels_it_and_changes_nothing_else() {
        let mut d = paired_doc();
        let mut ui = TerrainUi { params: Params { brush: Brush::Raise, radius: 8.0, falloff: 0.5, amount: 1.0, height: 0.0 }, ..Default::default() };
        stroke(&mut ui, &mut d, [20.0, 20.0], [24.0, 20.0], 3).unwrap();
        let first = heights(&d);
        ui.begin(&mut d, [40.0, 40.0], false).unwrap();
        ui.drag(&mut d, [44.0, 40.0], 1.0);
        d.undo(); // the user presses Ctrl+Z while the button is down
        let err = ui.finish(&mut d).unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
        assert!(ui.stroke.is_none());
        // the first stroke is undone, the second one is gone: the terrain is the original
        let orig = paired_doc();
        assert_eq!(heights(&d), heights(&orig));
        assert_ne!(first, heights(&orig));
        d.redo();
        assert_eq!(heights(&d), first, "redo of the first stroke still gives exactly that stroke");
    }

    #[test]
    fn terrain_redrop_npcs() {
        let mut d = paired_doc();
        // an svmap NPC on the first one's place, standing on the ground (height 9000 raw = -20), and a second far away
        let g = terrain::units_of(9000);
        assert!((g - (9000.0 * 0.02 + HEIGHT_OFFSET)).abs() < 1e-3);
        d.map.npcs.push(NpcEntry { kind: 1, type_id: 9, points: vec![NpcPoint { pos: [22.0, g + 2.0, 20.0], dir: 0.0 }] });
        let far = d.map.npcs.len();
        d.map.npcs.push(NpcEntry { kind: 1, type_id: 9, points: vec![NpcPoint { pos: [60.0, g, 60.0], dir: 0.0 }] });
        let _ = far;
        let svmap_before = d.map.npcs.clone();
        let client_before = d.part().unwrap().wld.model.npcs.clone();
        let portal_before = d.part().unwrap().wld.model.portals[0].clone();
        let svmap_before_portals = d.map.portals.clone();
        let mut ui = TerrainUi { params: Params { brush: Brush::Set, radius: 16.0, falloff: 0.0, amount: 1.0, height: g + 8.0 }, ..Default::default() };
        let r = stroke(&mut ui, &mut d, [20.0, 20.0], [20.0, 20.0], 1).unwrap().unwrap();
        assert!(r.moved >= 2, "the NPC records inside the stroke moved: {r:?}");
        assert_eq!(d.undo.len(), 1, "terrain and the records are one step");
        // the client NPC (pos [20, 1, 20], path points on y = 1 at x 22 and 24) rose by the ground's rise, 8 units, each point on its own
        let c = &d.part().unwrap().wld.model.npcs[1];
        assert!((c.pos[1] - (client_before[1].pos[1] + 8.0)).abs() < 0.05, "{:?}", c.pos);
        assert!((c.extra[0][1] - (client_before[1].extra[0][1] + 8.0)).abs() < 0.05);
        // svmap points moved the same way; the far one stayed
        let i = d.map.npcs.len() - 2;
        assert!((d.map.npcs[i].points[0].pos[1] - (svmap_before[i].points[0].pos[1] + 8.0)).abs() < 0.05, "stays 2 above the new ground");
        assert_eq!(d.map.npcs.last().unwrap().points[0].pos, svmap_before.last().unwrap().points[0].pos);
        // the portal box (10..14 and 10..14, its centre is inside the stroke) rose as a whole by the ground's rise, its size kept,
        // and the svmap portal with it
        let m = &d.part().unwrap().wld.model;
        assert!((m.portals[0].min[1] - (portal_before.min[1] + 8.0)).abs() < 0.05 && (m.portals[0].max[1] - m.portals[0].min[1] - 4.0).abs() < 1e-4);
        assert!((d.map.portals[0].pos[1] - (svmap_before_portals[0].pos[1] + 8.0)).abs() < 0.05);
        assert_eq!(d.map.portals[2].pos, svmap_before_portals[2].pos, "the portal outside the stroke stays");
        // undo gives everything back
        d.undo();
        assert_eq!(d.part().unwrap().wld.model.npcs, client_before);
        assert_eq!(d.map.npcs, svmap_before);
        // with the option off only the terrain changes
        ui.keep_on_ground = false;
        stroke(&mut ui, &mut d, [20.0, 20.0], [20.0, 20.0], 1).unwrap().unwrap();
        assert_eq!(d.part().unwrap().wld.model.npcs, client_before);
        assert_eq!(d.map.npcs, svmap_before);
    }

    #[test]
    fn placements_follow_the_ground_and_names_stay() {
        let mut d = paired_doc();
        {
            let m = &mut d.part_mut().unwrap().wld.model;
            let mut nm = [0u8; 256];
            nm[..5].copy_from_slice(b"a.smo");
            m.building.names = vec![nm];
            m.building.items = vec![crate::place::upright(0, [20.0, -19.0, 20.0], 0.0), crate::place::upright(0, [50.0, -19.0, 50.0], 0.0)];
            let p = d.part_mut().unwrap();
            p.base = p.lists();
        }
        let names = d.part().unwrap().wld.model.building.names.clone();
        let far = d.part().unwrap().wld.model.building.items[1];
        let mut ui = TerrainUi { params: Params { brush: Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: terrain::units_of(9000) + 5.0 }, ..Default::default() };
        stroke(&mut ui, &mut d, [20.0, 20.0], [20.0, 20.0], 1).unwrap().unwrap();
        let m = &d.part().unwrap().wld.model;
        assert!((m.building.items[0].pos[1] - (-19.0 + 5.0)).abs() < 0.05, "{:?}", m.building.items[0].pos);
        assert_eq!(m.building.items[1], far);
        assert_eq!(m.building.names, names, "no name moves or changes");
    }

    #[test]
    fn slope_flips_only_cells_whose_class_changed() {
        let mut d = paired_doc();
        let size = d.map.size;
        // a blocked cell far from the stroke and one inside it that is blocked for another reason
        d.map.blocked.set(60, 60, true);
        d.map.blocked.set(20, 20, true);
        let mut ui = TerrainUi { reblock: true, keep_on_ground: false, slope_deg: 30.0, ..Default::default() };
        // a hill with a steep wall: set height far above the flat ground
        ui.params = Params { brush: Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: terrain::units_of(9000) + 40.0 };
        let r = stroke(&mut ui, &mut d, [24.0, 24.0], [24.0, 24.0], 1).unwrap().unwrap();
        assert!(r.flipped > 0, "the rim of the hill is steep now: {r:?}");
        assert!(d.map.blocked.get(60, 60), "far cells keep their value");
        assert!(d.map.blocked.get(20, 20), "a cell blocked for another reason keeps its value when its slope class did not change");
        // the top of the hill is flat: not blocked by slope, and it was not steep before either, so its value stays
        assert!(!d.map.blocked.get(24, 24));
        let steps = d.undo.len();
        assert_eq!(steps, 1, "terrain and cells are one step");
        d.undo();
        assert!(d.map.blocked.get(20, 20) && d.map.blocked.get(60, 60));
        let rim = (0..size).flat_map(|x| (0..size).map(move |z| (x, z))).filter(|(x, z)| d.map.blocked.get(*x, *z)).count();
        assert_eq!(rim, 2, "undo gives back exactly the cells there were");
    }


    #[test]
    fn a_tab_made_read_only_during_a_stroke_keeps_nothing() {
        let mut d = paired_doc();
        let h0 = heights(&d);
        let mut ui = TerrainUi { params: Params { brush: Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: 30.0 }, ..Default::default() };
        ui.begin(&mut d, [20.0, 20.0], false).unwrap();
        assert_ne!(heights(&d), h0);
        d.read_only = true;
        assert!(ui.finish(&mut d).is_err());
        assert_eq!(heights(&d), h0, "what was drawn is taken back");
        assert!(d.undo.is_empty());
    }
    #[test]
    fn a_dungeon_or_a_read_only_tab_refuses() {
        let mut d = paired_doc();
        d.read_only = true;
        let mut ui = TerrainUi::default();
        assert!(ui.begin(&mut d, [10.0, 10.0], false).is_err());
        assert!(ui.stroke.is_none());
        let mut e = paired_doc();
        e.part_mut().unwrap().wld.model.terrain = None;
        assert!(TerrainUi::check(&e).unwrap_err().contains("no terrain"));
    }

    fn real_doc(rules: &crate::client_rules::ClientRules, id: u32) -> Option<Doc> {
        let crate::pair::Attach::Part(part) = crate::pair::ClientPart::attach(rules, id, shaiya_clientdata::wld_raw::Codepage::Cp949) else { return None };
        let size = part.terrain()?.size as u32;
        let mut d = Doc::new(1, Some(id), None, shaiya_svmap::Svmap::new(size), None);
        d.set_client(crate::doc::ClientSlot::Part(part));
        Some(d)
    }

    /// Real maps (needs `SHAIYA_CLIENT`): strokes of every brush on four field maps keep the model valid, the written length exact,
    /// and undo of everything gives the original file byte for byte; a dungeon refuses. The times are printed (the large map is
    /// 2048 x 2048 units, 1.05 million corners).
    #[test]
    fn real_maps_terrain_strokes_are_exact() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(rules) = crate::client_rules::ClientRules::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
            return;
        };
        for id in [0u32, 1, 2, 18] {
            let Some(mut d) = real_doc(&rules, id) else { continue };
            let size = d.map.size as f32;
            let original = bytes(&d);
            assert_eq!(WldRaw::parse(&original, "orig").unwrap().to_bytes(), original, "map {id} is lossless");
            let mut ui = TerrainUi::default();
            let c = size * 0.4;
            let t0 = Instant::now();
            for (brush, falloff) in [(Brush::Raise, 0.6), (Brush::Lower, 0.3), (Brush::Flatten, 0.5), (Brush::Smooth, 0.9), (Brush::Set, 0.2), (Brush::Ramp, 0.5)] {
                ui.params = Params { brush, radius: 30.0, falloff, amount: 0.8, height: 5.0 };
                stroke(&mut ui, &mut d, [c, c], [c + 60.0, c + 20.0], 20).unwrap();
            }
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            let edited = bytes(&d);
            assert_eq!(edited.len(), original.len(), "map {id}: exact length");
            let back = WldRaw::parse(&edited, "edited").unwrap();
            assert!(back.validate().is_empty(), "map {id}");
            assert_eq!(back.to_bytes(), edited);
            let steps = d.undo.len();
            assert!((5..=6).contains(&steps), "map {id}: one step per stroke ({steps})");
            let changed = d.part().unwrap().changes();
            assert!(changed.iter().any(|l| l.starts_with("client terrain")), "{changed:?}");
            while d.undo().is_some() {}
            assert_eq!(bytes(&d), original, "map {id}: undo of everything gives the original file");
            while d.redo().is_some() {}
            assert_eq!(bytes(&d), edited, "map {id}: redo of everything gives the edited file");
            eprintln!("map {id}: {} strokes of 20 moves in {ms:.0} ms, {} bytes", steps, edited.len());
        }
        // a dungeon has no terrain
        if let Some(crate::pair::Attach::Part(p)) = Some(crate::pair::ClientPart::attach(&rules, 65, shaiya_clientdata::wld_raw::Codepage::Cp949)) {
            let mut d = Doc::new(1, Some(65), None, shaiya_svmap::Svmap::new(440), None);
            d.set_client(crate::doc::ClientSlot::Part(p));
            assert!(TerrainUi::default().begin(&mut d, [10.0, 10.0], false).is_err());
        }
    }

    /// The cost of a stroke on the largest map, measured piece by piece: one dab, the diff and re-drop at the end, the picture
    /// rectangle, the ground copy and the 3D chunks (printed; the bounds are loose so a slow machine does not fail it).
    #[test]
    fn real_map_stroke_costs() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(rules) = crate::client_rules::ClientRules::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
            return;
        };
        let Some(mut d) = real_doc(&rules, 0) else {
            eprintln!("skipped: map 0 of SHAIYA_CLIENT has no field terrain");
            return;
        };
        let n = d.part().unwrap().terrain().unwrap().corners();
        let mut ui = TerrainUi { params: Params { brush: Brush::Raise, radius: 40.0, falloff: 0.6, amount: 0.8, height: 0.0 }, ..Default::default() };
        ui.begin(&mut d, [1000.0, 1000.0], false).unwrap();
        let t0 = Instant::now();
        let k = 60;
        for i in 0..k {
            ui.drag(&mut d, [1000.0 + i as f32 * 2.0, 1000.0], 1.0);
        }
        let dab_ms = t0.elapsed().as_secs_f32() * 1000.0 / k as f32;
        let t1 = Instant::now();
        let rep = ui.finish(&mut d).unwrap().unwrap();
        let finish_ms = t1.elapsed().as_secs_f32() * 1000.0;
        let p = d.part().unwrap();
        let t = p.terrain().unwrap();
        let rect = TerrainDiff { cells: ui_diff_cells(p) }.rect(n).unwrap();
        let tints = [[100u8, 120, 90]; 8];
        let mut rgba = vec![0u8; n * n * 4];
        let t2 = Instant::now();
        crate::assets::shade_rect(&t.heights, &t.tex_index, n, &tints, [rect[0].saturating_sub(1), rect[1].saturating_sub(1), rect[2] + 1, rect[3] + 1], &mut rgba);
        let shade_ms = t2.elapsed().as_secs_f32() * 1000.0;
        let t3 = Instant::now();
        let copy = shaiya_clientdata::wld::Terrain { size: t.size as u32, n, heights: t.heights.clone(), tex_index: t.tex_index.clone() };
        let copy_ms = t3.elapsed().as_secs_f32() * 1000.0;
        let t4 = Instant::now();
        let per = crate::scene3d::chunks_per_side(n);
        let mut chunks = 0;
        for cj in (rect[1] as usize / 64).saturating_sub(1)..=((rect[3] as usize + 1) / 64).min(per - 1) {
            for ci in (rect[0] as usize / 64).saturating_sub(1)..=((rect[2] as usize + 1) / 64).min(per - 1) {
                let _ = crate::scene3d::chunk_at(&t.heights, n, ci, cj);
                chunks += 1;
            }
        }
        let chunk_ms = t4.elapsed().as_secs_f32() * 1000.0;
        let t5 = Instant::now();
        let _ = d.part().unwrap().changes();
        let changes_ms = t5.elapsed().as_secs_f32() * 1000.0;
        drop(copy);
        eprintln!("map 0 ({n} x {n} corners), radius 40, a stroke of {k} moves, {} corners: dab {dab_ms:.2} ms, finish {finish_ms:.1} ms, picture rect {shade_ms:.2} ms, ground copy {copy_ms:.2} ms, {chunks} 3D chunks {chunk_ms:.1} ms, Changes tab {changes_ms:.2} ms", rep.corners);
        // the times are printed, not asserted: a timing limit makes a test fail on a busy machine
    }

    fn hash(b: &[u8]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut h);
        h.finish()
    }

    /// A small seeded generator (xorshift64*), so a failure repeats.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn f(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }
        fn pick(&mut self, k: usize) -> usize {
            (self.next() % k as u64) as usize
        }
    }

    /// seeded random strokes on the real maps 0, 1, 2 and 18 with their original svmaps (when
    /// `SVMAP_ORIGINALS` is set): every brush, bad parameters (radius 0, 1e9, NaN; amount NaN and infinity; a Set height of
    /// infinity), strokes that start outside the map, frames of NaN or 100 steps, Esc, Ctrl+Z and a tool switch during the stroke,
    /// a missed release, undo and redo. After every action the `.wld` and the svmap are exactly the state recorded for that history
    /// depth; a stroke without Keep on ground changes no byte outside the height block (bytes 8 to 8 + 2 n^2, little-endian i16) and,
    /// without Re-block, no svmap byte; with Re-block only the blocked cells; the length never changes; undo of everything gives the
    /// original files, redo of everything the last state.
    #[test]
    fn review_random_strokes_are_byte_exact() {
        let Ok(dir) = std::env::var("SHAIYA_CLIENT") else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let Ok(rules) = crate::client_rules::ClientRules::open(std::path::Path::new(&dir)) else {
            eprintln!("skipped: SHAIYA_CLIENT is not a client folder");
            return;
        };
        let originals = std::env::var_os("SVMAP_ORIGINALS").map(std::path::PathBuf::from);
        for (id, seed) in [(0u32, 0x9E37_79B9_7F4A_7C15u64), (1, 0xD1B5_4A32_D192_ED03), (2, 0x8CB9_2BA7_2F3D_8DD7), (18, 0x5851_F42D_4C95_7F2D)] {
            let crate::pair::Attach::Part(part) = crate::pair::ClientPart::attach(&rules, id, shaiya_clientdata::wld_raw::Codepage::Cp949) else { continue };
            let size = part.terrain().unwrap().size as u32;
            let map = originals.as_ref().and_then(|o| std::fs::read(o.join(format!("{id}.svmap"))).ok()).and_then(|b| shaiya_svmap::read(&b).ok()).filter(|m| m.size == size).unwrap_or_else(|| shaiya_svmap::Svmap::new(size));
            let mut d = Doc::new(1, Some(id), None, map, None);
            d.set_client(crate::doc::ClientSlot::Part(part));
            let n = d.part().unwrap().terrain().unwrap().corners();
            let hb = 8..8 + 2 * n * n;
            let wld0 = bytes(&d);
            let sv0 = shaiya_svmap::write(&d.map);
            assert_eq!(i32::from_le_bytes(wld0[4..8].try_into().unwrap()) as u32, size, "map {id}: the size sits before the heights");
            for (k, h) in heights(&d).iter().enumerate().step_by(997) {
                assert_eq!(i16::from_le_bytes([wld0[8 + 2 * k], wld0[9 + 2 * k]]), *h, "map {id}: corner {k} is a little-endian i16 at 8 + 2k");
            }
            // the state of every history depth: (hash of the .wld, hash of the svmap)
            let mut states = vec![(hash(&wld0), hash(&sv0))];
            let mut prev_wld = wld0.clone();
            let mut prev_sv = sv0.clone();
            let mut rng = Rng(seed);
            let (mut strokes, mut cancelled, mut flips, mut moved) = (0, 0, 0, 0);
            let svmap_records = (d.map.npcs.len(), d.map.portals.len());
            let check = |d: &Doc, states: &[(u64, u64)], what: &str| {
                let depth = d.undo.len();
                assert!(!d.part().unwrap().has_live(), "map {id} {what}: no drawn corner is left over");
                let (w, s) = (bytes(d), shaiya_svmap::write(&d.map));
                assert_eq!(w.len(), wld0.len(), "map {id} {what}: the length never changes");
                assert_eq!((hash(&w), hash(&s)), states[depth], "map {id} {what}: the files are the state of depth {depth}");
                (w, s)
            };
            for step in 0..70 {
                let action = rng.pick(10);
                if action == 0 && !d.undo.is_empty() {
                    d.undo();
                    let (w, s) = check(&d, &states, &format!("step {step} undo"));
                    (prev_wld, prev_sv) = (w, s);
                    continue;
                }
                if action == 1 && !d.redo.is_empty() {
                    d.redo();
                    let (w, s) = check(&d, &states, &format!("step {step} redo"));
                    (prev_wld, prev_sv) = (w, s);
                    continue;
                }
                let mut ui = TerrainUi { keep_on_ground: rng.pick(2) == 0, reblock: rng.pick(3) == 0, slope_deg: 20.0 + rng.f() * 50.0, ..Default::default() };
                let brush = Brush::ALL[rng.pick(6)];
                let radius = [0.0, 1.0e9, f32::NAN, 2.0 + rng.f() * 78.0, 2.0 + rng.f() * 78.0][rng.pick(5)];
                let amount = [f32::NAN, f32::INFINITY, rng.f(), rng.f(), 1.0][rng.pick(5)];
                let height = [f32::INFINITY, -900.0 + rng.f() * 1400.0, -900.0 + rng.f() * 1400.0][rng.pick(3)];
                ui.params = Params { brush, radius, falloff: if rng.pick(8) == 0 { f32::NAN } else { rng.f() }, amount, height };
                let s = size as f32;
                let mut at = [-60.0 + rng.f() * (s + 120.0), -60.0 + rng.f() * (s + 120.0)];
                if ui.begin(&mut d, at, rng.pick(4) == 0).is_err() {
                    continue;
                }
                for _ in 0..rng.pick(15) {
                    at = [at[0] + (rng.f() - 0.5) * 40.0, at[1] + (rng.f() - 0.5) * 40.0];
                    let steps = [1.0, 0.25, 4.0, 100.0, f32::NAN, f32::INFINITY][rng.pick(6)];
                    ui.drag(&mut d, at, steps);
                }
                let depth0 = d.undo.len();
                let ending = rng.pick(8);
                match ending {
                    0 => {
                        assert!(ui.abort(&mut d));
                        cancelled += 1;
                    }
                    1 if !d.undo.is_empty() => {
                        // Ctrl+Z with the button down: the stroke is gone, the undo is the only change
                        d.undo();
                        ui.drag(&mut d, at, 1.0);
                        assert!(ui.finish(&mut d).unwrap_or(None).is_none());
                        cancelled += 1;
                    }
                    2 => {
                        // the tool was switched with the button down
                        let uid = d.uid;
                        assert!(ui.guard(std::slice::from_mut(&mut d), Some(uid), false, true).is_some());
                        cancelled += 1;
                    }
                    3 => {
                        // the release was missed: the next frame ends the stroke as its step
                        let uid = d.uid;
                        let _ = ui.guard(std::slice::from_mut(&mut d), Some(uid), true, false);
                    }
                    _ => {
                        let _ = ui.finish(&mut d);
                    }
                }
                assert!(ui.stroke.is_none());
                let depth = d.undo.len();
                if depth == depth0 + 1 {
                    // one stroke, one step
                    strokes += 1;
                    states.truncate(depth);
                    let (w, s) = (bytes(&d), shaiya_svmap::write(&d.map));
                    states.push((hash(&w), hash(&s)));
                    assert_eq!(w.len(), wld0.len(), "map {id} step {step}: exact length");
                    if !ui.keep_on_ground {
                        assert_eq!(w[..hb.start], prev_wld[..hb.start], "map {id} step {step}: header unchanged");
                        assert!(w[hb.end..] == prev_wld[hb.end..], "map {id} step {step}: nothing after the heights changed");
                        if !ui.reblock {
                            assert!(s == prev_sv, "map {id} step {step}: the svmap is unchanged");
                        }
                    }
                    if ui.reblock && !ui.keep_on_ground {
                        let now = shaiya_svmap::read(&s).unwrap();
                        let was = shaiya_svmap::read(&prev_sv).unwrap();
                        assert!(shaiya_svmap::Svmap { blocked: now.blocked.clone(), ..was.clone() } == now, "map {id} step {step}: only cells");
                        flips += (0..size).map(|z| (0..size).filter(|x| now.blocked.get(*x, z) != was.blocked.get(*x, z)).count()).sum::<usize>();
                    }
                    if ui.keep_on_ground && (d.map.npcs.len(), d.map.portals.len()) == svmap_records {
                        moved += 1;
                    }
                    let st = d.undo.last().unwrap();
                    assert_eq!(st.cops.iter().filter(|c| matches!(c, COp::Terrain(_))).count(), 1, "one corner diff per step");
                    (prev_wld, prev_sv) = (w, s);
                } else {
                    assert!(depth == depth0 || (ending == 1 && depth + 1 == depth0), "map {id} step {step}: no step besides the undo");
                    let (w, s) = check(&d, &states, &format!("step {step} after a stroke without a step"));
                    (prev_wld, prev_sv) = (w, s);
                }
                if step % 10 == 9 {
                    let back = WldRaw::parse(&prev_wld, "edited").unwrap();
                    assert!(back.validate().is_empty(), "map {id}");
                    assert!(back.to_bytes() == prev_wld, "map {id}: lossless after edits");
                }
            }
            let last = d.undo.len() + d.redo.len();
            while d.undo().is_some() {}
            assert!(bytes(&d) == wld0, "map {id}: undo of everything gives the original .wld");
            assert!(shaiya_svmap::write(&d.map) == sv0, "map {id}: and the original svmap");
            while d.redo().is_some() {}
            assert_eq!(d.undo.len(), last);
            check(&d, &states, "redo of everything");
            eprintln!("map {id}: {strokes} strokes, {cancelled} cancelled, {flips} cells flipped, {moved} with records kept on the ground");
            assert!(strokes >= 10, "map {id}: the run made strokes ({strokes})");
        }
    }

    /// bad numbers never reach the corners. A NaN amount, radius or falloff, an infinite height or time and a
    /// point that is not a number change nothing that a sane value would not change; a radius of 0 or 1e9 is held to 2 and 80.
    #[test]
    fn review_bad_numbers_are_held_to_the_toolbar_ranges() {
        let p = Params { brush: Brush::Raise, radius: f32::NAN, falloff: f32::INFINITY, amount: f32::NAN, height: f32::NEG_INFINITY }.sane();
        let def = Params::default();
        assert_eq!((p.radius, p.falloff, p.amount, p.height), (def.radius, def.falloff, def.amount, def.height));
        assert_eq!(Params { height: 1.0e6, ..def }.sane().height, terrain::units_of(i16::MAX));
        assert_eq!(Params { radius: 0.0, ..Params::default() }.sane().radius, terrain::MIN_RADIUS);
        assert_eq!(Params { radius: 1.0e9, ..Params::default() }.sane().radius, terrain::MAX_RADIUS);
        let mut d = paired_doc();
        let h0 = heights(&d);
        let mut ui = TerrainUi { params: Params { brush: Brush::Raise, radius: 1.0e9, falloff: 0.0, amount: f32::NAN, height: 0.0 }, ..Default::default() };
        assert!(ui.begin(&mut d, [f32::NAN, 3.0], false).is_err(), "a point that is not a number starts nothing");
        ui.begin(&mut d, [32.0, 32.0], false).unwrap();
        ui.drag(&mut d, [f32::INFINITY, 32.0], 1.0);
        ui.drag(&mut d, [33.0, 32.0], f32::NAN);
        ui.drag(&mut d, [34.0, 32.0], f32::INFINITY);
        ui.finish(&mut d).unwrap();
        let h1 = heights(&d);
        assert!(h1.iter().zip(&h0).all(|(a, b)| a >= b), "a raise only raises: no corner fell to raw 0");
        assert!(h1 != h0);
        assert_eq!(ui.params.radius, terrain::MAX_RADIUS, "the 1e9 radius was held to 80 units");
        assert!(!d.part().unwrap().has_live());
        // a raise held at the top of the range and then lowered in the same stroke comes down at once
        let n = 17;
        let mut h = vec![i16::MAX - 1; n * n];
        let mut acc = StrokeAcc::default();
        let up = Params { brush: Brush::Raise, radius: 8.0, falloff: 0.0, amount: 1.0, height: 0.0 };
        for _ in 0..200 {
            terrain::dab(&mut h, n, &up, &mut acc, [16.0, 16.0], 8.0, 0.0);
        }
        assert_eq!(h[8 * n + 8], i16::MAX);
        let down = Params { brush: Brush::Lower, ..up };
        terrain::dab(&mut h, n, &down, &mut acc, [16.0, 16.0], 1.0, 0.0);
        assert!(h[8 * n + 8] < i16::MAX - 30, "no height piled up above the top: {}", h[8 * n + 8]);
    }

    /// Ctrl+Z and then a new step in the same frame leave the history depth as it was; the stroke still notices
    /// (it follows the tab's edit counter, not the depth), and so does a merged step.
    #[test]
    fn review_an_undo_and_a_new_step_during_a_stroke_cancel_it() {
        let mut d = paired_doc();
        let mut ui = TerrainUi { keep_on_ground: false, ..Default::default() };
        ui.params = Params { brush: Brush::Set, radius: 6.0, falloff: 0.0, amount: 1.0, height: 30.0 };
        stroke(&mut ui, &mut d, [12.0, 12.0], [12.0, 12.0], 1).unwrap().unwrap();
        let after_first = heights(&d);
        ui.begin(&mut d, [40.0, 40.0], false).unwrap();
        d.undo();
        // another edit that is not terrain brings the depth back to where the stroke started
        let p = d.map.portals[0];
        let mut q = p;
        q.pos[0] += 1.0;
        d.commit("move portal", vec![crate::edit::Op::replace(0, crate::edit::Rec::Portal(p), crate::edit::Rec::Portal(q))]);
        assert_eq!(d.undo.len(), 1);
        assert!(ui.finish(&mut d).is_err(), "the stroke is cancelled");
        let h = heights(&d);
        assert_ne!(h, after_first, "the first stroke stays undone");
        assert_eq!(h, heights(&paired_doc()), "and nothing of the second one is left");
    }

    /// an undo that arrives during a stroke and writes back exactly the values the stroke drew. The builder's
    /// cancel told the stroke's corners from the undo's by value, so it put the first stroke's hill back although that stroke was
    /// undone. The client half now reverts the drawn corners before the undo applies.
    #[test]
    fn review_an_undo_that_writes_the_drawn_values_is_exact() {
        let mut d = paired_doc();
        let orig = heights(&d);
        let flat = terrain::units_of(9000);
        let mut ui = TerrainUi { keep_on_ground: false, ..Default::default() };
        ui.params = Params { brush: Brush::Set, radius: 8.0, falloff: 0.0, amount: 1.0, height: flat + 30.0 };
        stroke(&mut ui, &mut d, [30.0, 30.0], [30.0, 30.0], 1).unwrap().unwrap();
        let hill = heights(&d);
        assert_ne!(hill, orig);
        // the second stroke sets the same corners back to the flat height: what it draws equals what an undo of the first writes
        ui.params.height = flat;
        ui.begin(&mut d, [30.0, 30.0], false).unwrap();
        assert_eq!(heights(&d), orig, "the live stroke drew the flat ground again");
        d.undo();
        assert_eq!(heights(&d), orig, "the undo of the hill");
        assert!(ui.finish(&mut d).is_err(), "the stroke is cancelled");
        assert_eq!(heights(&d), orig, "and the hill does not come back");
        assert!(!d.part().unwrap().has_live());
        d.redo();
        assert_eq!(heights(&d), hill, "redo gives exactly the hill");
        // a redo during a stroke: the same
        d.undo();
        ui.params.height = flat + 55.0;
        ui.begin(&mut d, [30.0, 30.0], false).unwrap();
        d.redo();
        assert_eq!(heights(&d), hill, "the redo meets the heights of its depth");
        let _ = ui.finish(&mut d);
        assert_eq!(heights(&d), hill);
    }

    fn ui_diff_cells(p: &crate::pair::ClientPart) -> Vec<(u32, i16, i16)> {
        let (a, b) = (&p.base.terrain, &p.terrain().unwrap().heights);
        a.iter().zip(b).enumerate().filter(|(_, (x, y))| x != y).map(|(i, (x, y))| (i as u32, *x, *y)).collect()
    }
}
