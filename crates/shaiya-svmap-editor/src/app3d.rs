//! The App's side of the 3D view: switching between 2D and 3D, building and uploading the scene of a map on a worker thread,
//! the per-tab camera, the 3D commands, and the headless camera/perf helpers. The view itself is `view3d`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use super::{App, Pending, ShotOpts};
use crate::gpu3d::{Gpu3d, SceneCache, SceneGpu};
use crate::names::Lookups;
use crate::scene3d::SceneData;
use crate::view2d::Request;
use crate::view3d::{self, Cmd3d, Ctx3, GpuRefs, Ground3, View3dState};

pub enum Slot3 {
    Building(Receiver<SceneData>),
    Ready(Box<SceneGpu>),
}

pub struct Host3d {
    pub gpu: Option<Gpu3d>,
    pub scenes: SceneCache<Slot3>,
    pub states: HashMap<u64, View3dState>,
    /// Backface culling for dungeon meshes (the toolbar checkbox).
    pub cull: bool,
}

impl Default for Host3d {
    fn default() -> Self {
        Host3d { gpu: None, scenes: SceneCache::new(3), states: HashMap::new(), cull: true }
    }
}

impl Host3d {
    /// The client data changed: every scene has to be built again.
    pub fn clear_scenes(&mut self) {
        self.scenes = SceneCache::new(3);
    }

    /// A tab was closed.
    pub fn forget(&mut self, uid: u64) {
        self.scenes.remove(uid);
        self.states.remove(&uid);
    }
}

impl App {
    /// 2D <-> 3D. The selection is the document's, so it stays; the focus carries over (3D keeps the 2D centre and about the same
    /// extent, and back).
    pub(super) fn set_mode3d(&mut self, on: bool) {
        if on == self.mode3d {
            return;
        }
        let ground = self.ground_fn();
        if let Some(d) = self.docs.get_mut(self.active) {
            let st = self.v3.states.entry(d.uid).or_default();
            if on {
                // the 2D centre becomes the 3D focus (a 2D view that was never shown leaves the map framed by the 3D view)
                if d.view.fitted {
                    view3d::enter_from_2d(&mut st.cam, &d.view, self.vp.size, &ground);
                    st.framed = true;
                }
            } else if st.framed {
                view3d::leave_to_2d(&st.cam, &mut d.view, st.size);
            }
        }
        self.mode3d = on;
        if on && matches!(self.tool, crate::view2d::Tool::Paint | crate::view2d::Tool::Scatter) {
            self.tool = crate::view2d::Tool::Select;
        }
    }

    /// Whether the active map's 3D scene is built and uploaded.
    pub(super) fn scene3d_ready(&self) -> bool {
        self.docs.get(self.active).is_some_and(|d| matches!(self.v3.scenes.items.get(&d.uid), Some(Slot3::Ready(_))))
    }

    /// Moves finished scene builds onto the GPU.
    pub(super) fn poll_scenes(&mut self, ctx: &egui::Context) {
        let Some(rs) = self.rs.clone() else { return };
        let ids: Vec<u64> = self.v3.scenes.items.iter().filter(|(_, s)| matches!(s, Slot3::Building(_))).map(|(k, _)| *k).collect();
        for id in ids {
            let data = match self.v3.scenes.items.get(&id) {
                Some(Slot3::Building(rx)) => rx.try_recv().ok(),
                _ => None,
            };
            if let Some(data) = data {
                let gpu = self.v3.gpu.get_or_insert_with(|| Gpu3d::new(&rs.device));
                let scene = gpu.upload(&rs.device, &rs.queue, data);
                if !scene.problems.is_empty() {
                    eprintln!("3D scene of tab {id}: {} problems, first: {}", scene.problems.len(), scene.problems[0]);
                }
                self.v3.scenes.insert(id, Slot3::Ready(Box::new(scene)));
                ctx.request_repaint();
            }
        }
    }

    /// Runs a 3D key command on the active tab's camera (eased over 200 ms when the preference is on).
    pub(super) fn run_cmd3d(&mut self, cmd: Cmd3d) {
        let smooth = self.settings.smooth_camera;
        self.run_cmd3d_with(cmd, smooth);
    }

    /// `run_cmd3d` with the choice of easing (a screenshot takes the camera at once).
    pub(super) fn run_cmd3d_with(&mut self, cmd: Cmd3d, smooth: bool) {
        let Some(d) = self.docs.get(self.active) else { return };
        let uid = d.uid;
        let bounds = match self.v3.scenes.items.get(&uid) {
            Some(Slot3::Ready(s)) => (s.bounds_min, s.bounds_max),
            _ => return,
        };
        if let Some(st) = self.v3.states.get_mut(&uid) {
            view3d::start_command(st, cmd, d, bounds, smooth);
        }
    }

    /// The 3D central panel: tabs, the scene (built on a worker thread the first time), the viewport.
    pub(super) fn central3d(&mut self, ui: &mut egui::Ui) {
        self.tab_bar(ui);
        ui.separator();
        self.disk_bar(ui);
        let ctx = ui.ctx().clone();
        let Some(rs) = self.rs.clone() else {
            ui.label("The 3D view needs the wgpu renderer.");
            return;
        };
        let active = self.active;
        let (uid, size) = (self.docs[active].uid, self.docs[active].map.size);
        let Some(assets) = self.current_assets() else {
            ui.label("loading client data...");
            return;
        };
        if self.v3.gpu.is_none() {
            self.v3.gpu = Some(Gpu3d::new(&rs.device));
        }
        match self.v3.scenes.items.get(&uid) {
            None => {
                let (tx, rx) = channel();
                let rules = self.client.clone();
                let a = assets.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let data = SceneData::build(rules.as_ref().map(|r| &r.source), a.wld.as_deref(), size);
                    let _ = tx.send(data);
                    ctx2.request_repaint();
                });
                self.v3.scenes.insert(uid, Slot3::Building(rx));
                ui.label("building the 3D scene...");
                return;
            }
            Some(Slot3::Building(_)) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("building the 3D scene (terrain, textures)...");
                });
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Some(Slot3::Ready(_)) => {}
        }
        self.v3.scenes.touch(uid);

        let rules = self.client.clone();
        let sys = self.sysmsg.clone();
        let map_name = move |id: u32| shaiya_clientdata::map_name(id, sys.as_deref());
        let pick_banner = self.pick_banner_text(active);
        let ground_h = view3d::ground_lookup(Some(&*assets));
        let tool = self.effective_tool();
        let _ = self.heat_frame(&ctx);
        let heat_on = self.heat_mode() != crate::heat::Mode::Off;
        let hover_rec = self.hover_rec.take();
        let template = self.active_template().cloned();
        let can_paste = !self.clipboard.is_empty();
        // the fixes of the findings of the record under an open context menu
        let menu_fixes: Vec<(shaiya_svmap::Finding, Vec<String>)> = match self.v3.states.get(&uid).and_then(|s| s.menu.as_ref()).and_then(|m| m.hit) {
            Some(h) => {
                let env = self.fix_env();
                let d = &self.docs[active];
                let cx = env.ctx(&d.map, d.map_id);
                d.findings.iter().filter(|f| f.section == h.sec && f.index == Some(h.idx)).map(|f| (f.clone(), crate::fixes::fixes_for(f, &cx).into_iter().map(|x| x.title).collect())).collect()
            }
            None => Vec::new(),
        };
        let out = {
            let d = &mut self.docs[active];
            let map_id = d.map_id;
            let host = &mut self.v3;
            let cull = host.cull;
            let Some(gpu) = host.gpu.as_mut() else { return };
            let Some(Slot3::Ready(scene)) = host.scenes.items.get_mut(&uid) else { return };
            let st = host.states.entry(uid).or_default();
            st.backface_cull = cull;
            if let Some(pt) = d.part() {
                let v = (pt.version().0, pt.place_rev());
                // once synced, a half read again (place_rev back at 0) is followed too, so the scene never keeps an old half's edits
                if (pt.place_rev() > 0 || scene.placements_synced()) && !scene.placements_current(v) {
                    let m = &pt.wld.model;
                    scene.sync_placements([&m.building, &m.shape, &m.tree], v);
                }
                // a model the scene does not hold yet (added from the browser) is loaded now, one per frame, so the placement
                // shows as the real model; a model that cannot be loaded stays a box with its name
                let m = &pt.wld.model;
                if let (Some(r), Some((class, name))) = (rules.as_deref(), scene.next_missing_model([&m.building, &m.shape, &m.tree])) {
                    match crate::scene3d::load_prop_model(&r.source, class, &name) {
                        Ok((model, textures)) => {
                            gpu.add_prop_model(&rs.device, &rs.queue, scene, class, &name, model, &textures);
                            scene.invalidate_placements();
                        }
                        Err(_) => {
                            scene.props_failed.insert((class, name));
                        }
                    }
                    ctx.request_repaint();
                }
                // the terrain mesh follows the tab's corner heights: only the chunks a change touched are rebuilt and written
                if let Some(t) = pt.terrain() {
                    let (serial, rev) = (pt.version().0, pt.terrain_rev());
                    let seen = scene.terrain_seen();
                    if seen != (serial, rev) {
                        use crate::pair::TerrainDirty;
                        let dirty = if seen.0 == serial { pt.terrain_dirty_since(seen.1) } else if seen.0 == 0 && rev == 0 { TerrainDirty::Nothing } else { TerrainDirty::Full };
                        let rect = match dirty {
                            TerrainDirty::Nothing => None,
                            TerrainDirty::Rect(r) => Some(Some(r)),
                            TerrainDirty::Full => Some(None),
                        };
                        if let Some(r) = rect {
                            scene.update_terrain(&rs.queue, &t.heights, t.corners(), r, (serial, rev));
                        } else {
                            scene.set_terrain_seen((serial, rev));
                        }
                    }
                }
            }
            let mut c = Ctx3 {
                doc: d,
                layers: &self.settings.layers,
                prop_distance: self.settings.prop_distance,
                locks: &self.settings.locks,
                look: Lookups { rules: rules.as_deref(), assets: Some(&*assets), map_id, names: None },
                tool,
                snap: self.settings.snap,
                draw_target: self.draw_target,
                npc_template: self.npc_template,
                pick_banner,
                map_name: &map_name,
                ground: Ground3 { size: size as f32, h: &ground_h },
                show_stats: self.settings.show_3d_stats,
                template: template.as_ref(),
                hover_rec,
                can_paste,
                menu_fixes: &menu_fixes,
                orbit_at_cursor: self.settings.orbit_at_cursor,
                heat: if heat_on { self.r2.heat_params(self.settings.heat.tile, self.settings.heat.opacity) } else { None },
                smooth_camera: self.settings.smooth_camera,
                place: &mut self.place,
                terrain: &mut self.terrain,
                codepage: shaiya_clientdata::wld_raw::Codepage::from_key(&self.settings.client_codepage).unwrap_or_default(),
            };
            view3d::show(ui, &mut c, st, GpuRefs { rs: &rs, gpu, scene })
        };
        self.vp.hover = out.cursor;
        for r in out.requests {
            match r {
                Request::AddPortal(p) => self.request(Pending::AddPortal(p)),
                Request::Toast(t) => self.toast(t),
                Request::CancelPick => self.cancel_picks(),
                Request::PickDone(p) => self.finish_pick(p),
                Request::Menu(a) => self.do_ctx_act(a, &ctx),
            }
        }
    }

    // ------------------------------------------------------------------------------------------------------------ headless

    /// `--select` frames the selection first; `--cam` then overrides the direction, distance and focus.
    pub(super) fn apply_shot_cam(&mut self, opts: &ShotOpts) {
        if opts.select.is_some() {
            self.run_cmd3d_with(Cmd3d::FrameSelection, false);
        }
        let ground = self.ground_fn();
        let Some(d) = self.docs.get(self.active) else { return };
        let Some(st) = self.v3.states.get_mut(&d.uid) else { return };
        if let Some(c) = &opts.cam {
            st.cam.yaw = c[0].to_radians();
            st.cam.pitch = c[1].to_radians().clamp(-89f32.to_radians(), 60f32.to_radians());
            if let Some(dist) = c.get(2) {
                st.cam.dist = *dist;
            }
            if c.len() == 5 {
                st.cam.focus = [c[3], ground(c[3], c[4]), c[4]];
            }
        }
    }

    /// Camera for a 3D perf scenario: `sc.1` is the distance (0 = the whole map); the camera turns a little every frame so the
    /// culling work changes.
    pub(super) fn apply_scenario3d(&mut self, sc: &(String, f32), step: usize) {
        let Some(d) = self.docs.get(self.active) else { return };
        let uid = d.uid;
        let Some(Slot3::Ready(scene)) = self.v3.scenes.items.get(&uid) else { return };
        let (bmin, bmax) = (scene.bounds_min, scene.bounds_max);
        let Some(st) = self.v3.states.get_mut(&uid) else { return };
        if sc.1 == 0.0 {
            let aspect = if st.size.y > 1.0 { st.size.x / st.size.y } else { 16.0 / 9.0 };
            st.cam.frame_map(bmin, bmax, aspect);
        } else {
            st.cam.fly = false;
            st.cam.dist = sc.1;
            st.cam.pitch = -35f32.to_radians();
            let t = step as f32 * 0.04;
            let s = d.map.size as f32;
            st.cam.focus = [s * 0.5 + t.sin() * s * 0.2, 20.0, s * 0.5 + (t * 0.7).cos() * s * 0.2];
        }
        st.cam.yaw += step as f32 * 0.01;
    }
}

// ---------------------------------------------------------------------------------------------------------------- scripted drag

/// `--drag3d`: synthetic mouse events through egui's raw input, so the real `view3d::show` wiring (press, drag, release) is
/// exercised in a headless run. The run prints what moved and whether y stayed on the ground.
pub struct Script3d {
    sec: shaiya_svmap::SectionId,
    idx: usize,
    delta: [f32; 2],
    stage: u32,
    p0: egui::Pos2,
    before: Option<crate::edit::Rec>,
    undo_before: usize,
}

impl Script3d {
    pub fn new((sec, idx, delta): (shaiya_svmap::SectionId, usize, [f32; 2])) -> Script3d {
        Script3d { sec, idx, delta, stage: 0, p0: egui::Pos2::ZERO, before: None, undo_before: 0 }
    }
}

impl App {
    /// Called from `raw_input_hook` every frame: advances the script one step once the shot is at its capture stage.
    pub(super) fn script3d_events(&mut self, raw: &mut egui::RawInput) {
        use crate::edit::Rec;
        let Some(mut sc) = self.script3d.take() else { return };
        let at_capture = matches!(self.shot.as_ref().map(|s| &s.stage), Some(super::ShotStage::Capture { .. }));
        let Some(d) = self.docs.get(self.active) else {
            self.script3d = Some(sc);
            return;
        };
        let uid = d.uid;
        let framed = self.v3.states.get(&uid).is_some_and(|s| s.framed);
        if !(at_capture && self.mode3d && self.scene3d_ready() && framed) {
            self.script3d = Some(sc);
            return;
        }
        let st = &self.v3.states[&uid];
        let pointer = |pos: egui::Pos2, pressed: Option<bool>| match pressed {
            None => egui::Event::PointerMoved(pos),
            Some(p) => egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: p, modifiers: egui::Modifiers::NONE },
        };
        const STEPS: u32 = 6;
        let delta = egui::Vec2::new(sc.delta[0], sc.delta[1]);
        match sc.stage {
            0 => {
                let Some(rec) = crate::edit::get(&d.map, sc.sec, sc.idx) else {
                    eprintln!("script3d: {} #{} does not exist", sc.sec.label(), sc.idx);
                    return;
                };
                let (mn, mx) = view3d::record_bounds(&rec);
                let aim = match &rec {
                    Rec::Npc(n) if !n.points.is_empty() => [n.points[0].pos[0], n.points[0].pos[1] + 2.0, n.points[0].pos[2]], // the pin
                    _ => [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5],
                };
                let vp = st.cam.view_proj(st.rect.width() / st.rect.height().max(1.0));
                let Some(p) = view3d::to_screen(&vp, st.rect, aim) else {
                    eprintln!("script3d: the record is behind the camera");
                    return;
                };
                sc.p0 = p;
                sc.before = Some(rec);
                sc.undo_before = d.undo.len();
                raw.events.push(pointer(p, None));
            }
            1 => raw.events.push(pointer(sc.p0, Some(true))),
            s if s <= 1 + STEPS => raw.events.push(pointer(sc.p0 + delta * ((s - 1) as f32 / STEPS as f32), None)),
            s if s == 2 + STEPS => raw.events.push(pointer(sc.p0 + delta, Some(false))),
            _ => {
                // a frame after the release: report
                let ground = self.ground_fn();
                let floor = |x: f32, z: f32| self.current_assets().and_then(|a| a.ground.height(x, z));
                let after = crate::edit::get(&d.map, sc.sec, sc.idx);
                let mut ok = true;
                let mut lines = Vec::new();
                match (&sc.before, &after) {
                    (Some(Rec::Npc(a)), Some(Rec::Npc(b))) => {
                        for (k, (p, q)) in a.points.iter().zip(&b.points).enumerate() {
                            let g = ground(q.pos[0], q.pos[2]);
                            ok &= (q.pos[1] - g).abs() <= 0.01;
                            lines.push(format!("point {k}: ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2}), ground there {:.2}", p.pos[0], p.pos[1], p.pos[2], q.pos[0], q.pos[1], q.pos[2], g));
                        }
                    }
                    (Some(Rec::Mob(a)), Some(Rec::Mob(b))) => {
                        let lowest = (0..=8).flat_map(|i| (0..=8).map(move |j| (i, j))).filter_map(|(i, j)| floor(b.min[0] + (b.max[0] - b.min[0]) * i as f32 / 8.0, b.min[2] + (b.max[2] - b.min[2]) * j as f32 / 8.0)).fold(f32::MAX, f32::min);
                        ok &= (b.min[1] - lowest).abs() <= 0.01;
                        lines.push(format!("area: ({:.2}, {:.2}, {:.2}) -> ({:.2}, {:.2}, {:.2}), lowest ground under it {:.2}", a.min[0], a.min[1], a.min[2], b.min[0], b.min[1], b.min[2], lowest));
                    }
                    (Some(a), Some(b)) => lines.push(format!("{a:?} -> {b:?}")),
                    _ => ok = false,
                }
                let moved = sc.before != after;
                let steps = d.undo.len().saturating_sub(sc.undo_before);
                ok &= moved && steps == 1;
                println!("script3d: {} #{} dragged by ({}, {}) px: {}; undo steps added {steps}; selected {:?}", sc.sec.label(), sc.idx, sc.delta[0], sc.delta[1], if moved { "moved" } else { "DID NOT MOVE" }, d.selection);
                for l in lines {
                    println!("script3d:   {l}");
                }
                println!("script3d: {}", if ok { "OK (y = ground within 0.01, one undo step)" } else { "FAIL" });
                return; // the script is finished (not put back)
            }
        }
        sc.stage += 1;
        self.script3d = Some(sc);
    }
}
