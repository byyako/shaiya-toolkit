//! The 3D viewport: an orbit / fly camera, picking of svmap records by ray, ground drag, the overlay geometry of the records,
//! and the commands behind the 3D keys. The GPU side is `gpu3d`; the scene data comes from `scene3d`.
//!
//! Everything that decides something (camera maths, rays, picking, the drag) is a plain function, so the tests run it without a
//! window; `show` only wires egui input to those functions and puts the rendered texture on screen.

use egui::{Align2, Color32, FontId, Key, PointerButton, Pos2, Rect, Sense, Stroke, Vec2};
use shaiya_svmap::{SectionId, Severity, Svmap};

use crate::assets::{Ground, MapAssets};
use crate::client_rules::PORTAL_USE_RANGE;
use crate::doc::{move_ops, Doc, Sel};
use crate::edit::{self, Rec};
use crate::gpu3d::{FrameParams, FrameStats, Gpu3d, OVert, OverlayGeo, SceneGpu};
use crate::math3d::{self, Mat4, Ray, V3};
use crate::names::{self, Lookups};
use crate::settings::{Layers, Locks};
use crate::view2d::{self, DrawTarget, Request, Tool};

pub const FOV_Y: f32 = 50.0 * std::f32::consts::PI / 180.0;
const FAR: f32 = 16000.0;
const MIN_PITCH: f32 = -89.0 * std::f32::consts::PI / 180.0;
const MAX_ORBIT_PITCH: f32 = -3.0 * std::f32::consts::PI / 180.0;
const MAX_FLY_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;

// pick shapes, in world units
const NPC_R: f32 = 1.6;
const NPC_H: f32 = 4.5;
const PATH_R: f32 = 1.2;
const PORTAL_R: f32 = 2.2;
const PORTAL_H: f32 = 9.0;
const POINT_R: f32 = 1.8;

const YELLOW: [u8; 4] = [255, 235, 60, 255];
const WHITE: [u8; 4] = [255, 255, 255, 230];

// ------------------------------------------------------------------------------------------------------------------ camera

/// An orbit camera around `focus` (left drag rotates, the wheel changes `dist`) that can switch to a free-flying one
/// (`fly`, WASD). `yaw` is the heading of the view direction from north towards east, `pitch` its elevation (negative = down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3d {
    pub focus: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fly: bool,
    pub fly_eye: V3,
    pub fly_speed: f32,
}

impl Default for Camera3d {
    fn default() -> Self {
        Camera3d { focus: [0.0, 0.0, 0.0], yaw: 0.0, pitch: -50f32.to_radians(), dist: 500.0, fly: false, fly_eye: [0.0, 50.0, 0.0], fly_speed: 60.0 }
    }
}

impl Camera3d {
    pub fn forward(&self) -> V3 {
        [self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), self.yaw.cos() * self.pitch.cos()]
    }

    pub fn eye(&self) -> V3 {
        if self.fly {
            self.fly_eye
        } else {
            math3d::sub(self.focus, math3d::scale(self.forward(), self.dist))
        }
    }

    pub fn right(&self) -> V3 {
        // horizontal and independent of the pitch (the pitch is clamped short of straight down)
        [self.yaw.cos(), 0.0, -self.yaw.sin()]
    }

    pub fn near(&self) -> f32 {
        if self.fly {
            0.3
        } else {
            (self.dist * 0.02).clamp(0.3, 30.0)
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = math3d::look_dir(self.eye(), self.forward(), [0.0, 1.0, 0.0]);
        math3d::mul(&math3d::perspective(FOV_Y, aspect, self.near(), FAR), &view)
    }

    /// The ray through a point of the viewport given in normalised device coordinates (x right, y up, both -1..1).
    pub fn ray(&self, ndc: [f32; 2], aspect: f32) -> Ray {
        let f = self.forward();
        let r = self.right();
        let u = math3d::cross(f, r);
        let t = (FOV_Y * 0.5).tan();
        let dir = math3d::norm(math3d::add(f, math3d::add(math3d::scale(r, ndc[0] * t * aspect), math3d::scale(u, ndc[1] * t))));
        Ray { origin: self.eye(), dir }
    }

    /// Mouse drag in pixels: orbit around the focus (the scene follows the mouse), or look around when flying.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        if self.fly {
            self.yaw += dx * 0.004;
            self.pitch = (self.pitch - dy * 0.004).clamp(MIN_PITCH, MAX_FLY_PITCH);
        } else {
            self.yaw -= dx * 0.005;
            self.pitch = (self.pitch - dy * 0.005).clamp(MIN_PITCH, MAX_ORBIT_PITCH);
        }
    }

    /// Pan along the ground: the point under the mouse stays under it. `viewport_h` is the viewport height in pixels.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        if self.fly {
            self.orbit(dx, dy);
            return;
        }
        let per_px = self.dist * 2.0 * (FOV_Y * 0.5).tan() / viewport_h.max(1.0);
        let r = self.right();
        let flat_fwd = [self.yaw.sin(), 0.0, self.yaw.cos()];
        let tilt = self.pitch.sin().abs().max(0.25);
        self.focus = math3d::add(self.focus, math3d::add(math3d::scale(r, -dx * per_px), math3d::scale(flat_fwd, dy * per_px / tilt)));
    }

    /// Wheel: dolly towards the focus, or change the flying speed.
    pub fn zoom(&mut self, scroll: f32) {
        if self.fly {
            self.fly_speed = (self.fly_speed * (scroll * 0.002).exp()).clamp(2.0, 2000.0);
        } else {
            self.dist = (self.dist * (-scroll * 0.0016).exp()).clamp(3.0, 8000.0);
        }
    }

    /// Flying: move by `local` = (right, up, forward) in camera terms for `dt` seconds (up is world up).
    pub fn fly_move(&mut self, local: V3, dt: f32, fast: bool) {
        if !self.fly {
            return;
        }
        let k = self.fly_speed * dt * if fast { 4.0 } else { 1.0 };
        let d = math3d::add(math3d::add(math3d::scale(self.right(), local[0]), [0.0, local[1], 0.0]), math3d::scale(self.forward(), local[2]));
        self.fly_eye = math3d::add(self.fly_eye, math3d::scale(d, k));
    }

    /// Shift+F: fly mode on or off. Leaving it puts the orbit focus where the camera was looking.
    pub fn toggle_fly(&mut self) {
        if self.fly {
            self.fly = false;
            self.focus = math3d::add(self.fly_eye, math3d::scale(self.forward(), self.dist));
            self.pitch = self.pitch.min(MAX_ORBIT_PITCH);
        } else {
            self.fly_eye = self.eye();
            self.fly = true;
        }
    }

    /// Looks at the box from the current direction, close enough to see all of it.
    pub fn frame_box(&mut self, min: V3, max: V3) {
        let c = math3d::scale(math3d::add(min, max), 0.5);
        let r = (math3d::len(math3d::sub(max, min)) * 0.5).max(6.0);
        self.focus = c;
        self.dist = (r / (FOV_Y * 0.5).sin() * 1.05).clamp(12.0, 8000.0);
        if self.fly {
            self.fly_eye = math3d::sub(c, math3d::scale(self.forward(), self.dist));
        }
    }

    /// The whole map in view from the south-west-ish default direction.
    pub fn frame_map(&mut self, min: V3, max: V3) {
        self.fly = false;
        self.yaw = 0.0;
        self.pitch = -50f32.to_radians();
        let c = math3d::scale(math3d::add(min, max), 0.5);
        let half = ((max[0] - min[0]).max(max[2] - min[2]) * 0.5).max(16.0);
        self.focus = c;
        self.dist = (half / (FOV_Y * 0.5).tan() * 1.02).clamp(40.0, 8000.0);
    }

    pub fn look_north(&mut self) {
        self.yaw = 0.0;
    }

    /// The start of an orbit around the ground point `g`: the focus moves to the depth of `g` on the view axis, and nothing on
    /// the screen moves. Does nothing while flying or when `g` is closer than 3 units in front of the eye.
    pub fn orbit_around(&mut self, g: V3) {
        if self.fly {
            return;
        }
        let (eye, f) = (self.eye(), self.forward());
        let depth = math3d::dot(math3d::sub(g, eye), f);
        if !depth.is_finite() || depth < 3.0 {
            return;
        }
        self.dist = depth.min(8000.0);
        self.focus = math3d::add(eye, math3d::scale(f, self.dist));
    }

    /// A drag of `dx`, `dy` pixels that turns the camera around `pivot`: the same turn as [`Camera3d::orbit`], with the eye
    /// carried round the pivot, so the pivot stays where it is on the screen.
    pub fn orbit_about(&mut self, pivot: V3, dx: f32, dy: f32) {
        if self.fly {
            self.orbit(dx, dy);
            return;
        }
        let (yaw0, pitch0, right0, eye0) = (self.yaw, self.pitch, self.right(), self.eye());
        self.orbit(dx, dy);
        let (dyaw, dpitch) = (self.yaw - yaw0, self.pitch - pitch0);
        // a turn of -a about the right axis raises the pitch by a; a turn about world up adds to the yaw
        let o = rotate_about(math3d::sub(eye0, pivot), right0, -dpitch);
        let o = rotate_about(o, [0.0, 1.0, 0.0], dyaw);
        let eye = math3d::add(pivot, o);
        if eye.iter().all(|v| v.is_finite()) {
            self.focus = math3d::add(eye, math3d::scale(self.forward(), self.dist));
        }
    }

    /// The part of the camera a smooth move changes.
    pub fn pose(&self) -> CamPose {
        CamPose { focus: self.focus, yaw: self.yaw, pitch: self.pitch, dist: self.dist }
    }

    pub fn set_pose(&mut self, p: CamPose) {
        self.focus = p.focus;
        self.yaw = p.yaw;
        self.pitch = p.pitch;
        self.dist = p.dist;
    }
}

/// `v` turned by `a` radians about the unit axis `k` (Rodrigues).
fn rotate_about(v: V3, k: V3, a: f32) -> V3 {
    let (s, c) = a.sin_cos();
    let kv = math3d::dot(k, v);
    math3d::add(math3d::add(math3d::scale(v, c), math3d::scale(math3d::cross(k, v), s)), math3d::scale(k, kv * (1.0 - c)))
}

/// Where an orbit camera looks from: focus, direction and distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamPose {
    pub focus: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

/// How long Frame selection, Frame map, Go to and the compass take.
pub const SMOOTH_SECS: f64 = 0.2;

/// Fast at the start, slow at the end (`t` 0 to 1).
pub fn ease_out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// A camera move in progress. The yaw turns the short way round.
#[derive(Clone, Copy, Debug)]
pub struct CamAnim {
    from: CamPose,
    to: CamPose,
    yaw_turn: f32,
    t0: f64,
}

impl CamAnim {
    pub fn new(from: CamPose, to: CamPose, t0: f64) -> CamAnim {
        let tau = std::f32::consts::TAU;
        let mut turn = (to.yaw - from.yaw).rem_euclid(tau);
        if turn > tau * 0.5 {
            turn -= tau;
        }
        CamAnim { from, to, yaw_turn: turn, t0 }
    }

    /// The pose at time `now` and whether the move is over (then it is exactly the target).
    pub fn at(&self, now: f64) -> (CamPose, bool) {
        let t = (now - self.t0) / SMOOTH_SECS;
        if t >= 1.0 {
            return (self.to, true);
        }
        let k = ease_out_cubic(t.max(0.0)) as f32;
        let l = |a: f32, b: f32| a + (b - a) * k;
        let (a, b) = (self.from, self.to);
        (CamPose { focus: [l(a.focus[0], b.focus[0]), l(a.focus[1], b.focus[1]), l(a.focus[2], b.focus[2])], yaw: a.yaw + self.yaw_turn * k, pitch: l(a.pitch, b.pitch), dist: l(a.dist, b.dist) }, false)
    }
}

// ------------------------------------------------------------------------------------------------------------------ compass

/// Radius of the compass ring in the top-right corner of the viewport.
pub const COMPASS_R: f32 = 24.0;

pub fn compass_centre(rect: Rect) -> Pos2 {
    Pos2::new(rect.right() - COMPASS_R - 14.0, rect.top() + COMPASS_R + 14.0)
}

/// Whether a click at `p` is on the compass (it turns the camera to face north).
pub fn compass_hit(rect: Rect, p: Pos2) -> bool {
    compass_centre(rect).distance(p) <= COMPASS_R + 4.0
}

/// Where a compass bearing (0 = north, 90 = east, in degrees) lies on the ring for a camera heading `yaw`, as a unit vector on
/// the screen (x right, y down). The camera's heading points up.
pub fn compass_dir(yaw: f32, bearing_deg: f32) -> Vec2 {
    let a = bearing_deg.to_radians() - yaw;
    Vec2::new(a.sin(), -a.cos())
}

/// Camera for a 2D view that is being left for 3D: the focus is the 2D centre (on the ground), the distance gives about the
/// ground area the 2D view showed.
pub fn enter_from_2d(cam: &mut Camera3d, view: &crate::doc::View, viewport: Vec2, ground: &dyn Fn(f32, f32) -> f32) {
    cam.fly = false;
    cam.focus = [view.cx, ground(view.cx, view.cz), view.cz];
    let visible_h = viewport.y.max(1.0) / view.zoom.max(0.01);
    cam.dist = (visible_h / (2.0 * (FOV_Y * 0.5).tan()) * 1.1).clamp(20.0, 8000.0);
}

/// The reverse: the 2D view centres on the 3D focus and keeps about the same extent.
pub fn leave_to_2d(cam: &Camera3d, view: &mut crate::doc::View, viewport: Vec2) {
    let focus = if cam.fly { math3d::add(cam.fly_eye, math3d::scale(cam.forward(), cam.dist)) } else { cam.focus };
    view.cx = focus[0];
    view.cz = focus[2];
    let visible_h = cam.dist * 2.0 * (FOV_Y * 0.5).tan() / 1.1;
    view.zoom = (viewport.y.max(1.0) / visible_h.max(1.0)).clamp(view2d::MIN_ZOOM, view2d::MAX_ZOOM);
    view.fitted = true;
}

// ------------------------------------------------------------------------------------------------------------------ commands (the 3D keys)

/// What the 3D keys do. `key_command` maps a key press to one; `Camera3d`/`apply_command` carry it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd3d {
    /// F
    FrameSelection,
    /// Home
    FrameMap,
    /// Shift+F
    ToggleFly,
    /// N: turn to face north
    LookNorth,
}

/// The 3D command a key press means (no modifiers other than Shift), if any. The caller decides when keys are live (no text
/// field focused, no dialog).
pub fn key_command(key: Key, shift: bool) -> Option<Cmd3d> {
    match (key, shift) {
        (Key::F, true) => Some(Cmd3d::ToggleFly),
        (Key::F, false) => Some(Cmd3d::FrameSelection),
        (Key::Home, _) => Some(Cmd3d::FrameMap),
        (Key::N, false) => Some(Cmd3d::LookNorth),
        _ => None,
    }
}

/// 3D box around the selected records, if any is selected.
pub fn selection_bounds(doc: &Doc) -> Option<(V3, V3)> {
    let mut b: Option<(V3, V3)> = None;
    for (s, i) in &doc.selection {
        if let Some(r) = edit::get(&doc.map, *s, *i) {
            let (mn, mx) = record_bounds(&r);
            b = Some(match b {
                None => (mn, mx),
                Some((a0, a1)) => ([a0[0].min(mn[0]), a0[1].min(mn[1]), a0[2].min(mn[2])], [a1[0].max(mx[0]), a1[1].max(mx[1]), a1[2].max(mx[2])]),
            });
        }
    }
    b
}

/// 3D bounds of a record (points get a small box so they can be framed).
pub fn record_bounds(r: &Rec) -> (V3, V3) {
    let pad = |p: V3, k: f32| ([p[0] - k, p[1], p[2] - k], [p[0] + k, p[1] + 4.0, p[2] + k]);
    let union = |a: (V3, V3), b: (V3, V3)| ([a.0[0].min(b.0[0]), a.0[1].min(b.0[1]), a.0[2].min(b.0[2])], [a.1[0].max(b.1[0]), a.1[1].max(b.1[1]), a.1[2].max(b.1[2])]);
    let boxed = |min: V3, max: V3| ([min[0].min(max[0]), min[1].min(max[1]), min[2].min(max[2])], [min[0].max(max[0]), min[1].max(max[1]), min[2].max(max[2])]);
    match r {
        Rec::A(a) => pad(a.pos, 2.0),
        Rec::B(p) => pad(*p, 2.0),
        Rec::Mob(a) => boxed(a.min, a.max),
        Rec::Npc(n) => n.points.iter().map(|p| pad(p.pos, 2.0)).reduce(union).unwrap_or(pad([0.0; 3], 2.0)),
        Rec::Portal(p) => pad(p.pos, PORTAL_USE_RANGE),
        Rec::Bind(b) => b.boxes.iter().map(|x| boxed(x.min, x.max)).reduce(union).unwrap_or(pad([0.0; 3], 2.0)),
        Rec::Region(g) => boxed(g.min, g.max),
    }
}

/// Carries out a 3D command on a camera. `bounds` is the scene's box (for `FrameMap`).
pub fn apply_command(cam: &mut Camera3d, cmd: Cmd3d, doc: &Doc, bounds: (V3, V3)) {
    match cmd {
        Cmd3d::FrameSelection => {
            if let Some((mn, mx)) = selection_bounds(doc) {
                cam.frame_box(mn, mx);
            }
        }
        Cmd3d::FrameMap => cam.frame_map(bounds.0, bounds.1),
        Cmd3d::ToggleFly => cam.toggle_fly(),
        Cmd3d::LookNorth => cam.look_north(),
    }
}

/// Runs a 3D command on a view: eased over 200 ms when `smooth` is on and the camera orbits, at once otherwise (flying and the
/// fly toggle never ease).
pub fn start_command(st: &mut View3dState, cmd: Cmd3d, doc: &Doc, bounds: (V3, V3), smooth: bool) {
    st.anim = None;
    st.goto = None;
    let mut target = st.cam;
    apply_command(&mut target, cmd, doc, bounds);
    if smooth && !st.cam.fly && cmd != Cmd3d::ToggleFly {
        st.goto = Some(target.pose());
    } else {
        st.cam = target;
    }
}

// ------------------------------------------------------------------------------------------------------------------ ground

/// The ground the rays are tested against: its height under (x, z) (`None` = no floor there) on a map of `size` units.
pub struct Ground3<'a> {
    pub size: f32,
    pub h: &'a dyn Fn(f32, f32) -> Option<f32>,
}

impl Ground3<'_> {
    pub fn at(&self, x: f32, z: f32) -> f32 {
        (self.h)(x, z).unwrap_or(0.0)
    }

    /// First point where the ray meets the ground (marching, then bisecting): `None` when it never does.
    pub fn ray(&self, ray: &Ray) -> Option<V3> {
        let (t0, t1) = slab_range(ray, [0.0, -1.0e5, 0.0], [self.size, 1.0e5, self.size])?;
        let t_end = t1.min(30000.0);
        let mut t = t0.max(0.0);
        let mut prev: Option<(f32, bool)> = None;
        loop {
            let p = ray.at(t);
            let above = (self.h)(p[0].clamp(0.0, self.size - 0.001), p[2].clamp(0.0, self.size - 0.001)).is_none_or(|g| p[1] >= g);
            if let Some((pt, true)) = prev {
                if !above {
                    let (mut lo, mut hi) = (pt, t);
                    for _ in 0..28 {
                        let mid = 0.5 * (lo + hi);
                        let q = ray.at(mid);
                        if (self.h)(q[0].clamp(0.0, self.size - 0.001), q[2].clamp(0.0, self.size - 0.001)).is_none_or(|g| q[1] >= g) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let q = ray.at(hi);
                    return Some([q[0], self.at(q[0], q[2]), q[2]]);
                }
            }
            prev = Some((t, above));
            if t >= t_end {
                return None;
            }
            t = (t + (0.004 * t).clamp(0.3, 1.5)).min(t_end);
        }
    }
}

/// Entry and exit distance of the ray through a box (`None` when it misses).
fn slab_range(ray: &Ray, min: V3, max: V3) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for a in 0..3 {
        if ray.dir[a].abs() < 1e-9 {
            if ray.origin[a] < min[a] || ray.origin[a] > max[a] {
                return None;
            }
        } else {
            let inv = 1.0 / ray.dir[a];
            let (mut a0, mut a1) = ((min[a] - ray.origin[a]) * inv, (max[a] - ray.origin[a]) * inv);
            if a0 > a1 {
                std::mem::swap(&mut a0, &mut a1);
            }
            t0 = t0.max(a0);
            t1 = t1.min(a1);
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((t0, t1))
}

/// y where the ray crosses the vertical plane through `point` with horizontal normal `n`.
fn ray_vertical_plane_y(ray: &Ray, point: V3, n: V3) -> Option<f32> {
    let denom = math3d::dot(ray.dir, n);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = math3d::dot(math3d::sub(point, ray.origin), n) / denom;
    (t > 0.0).then(|| ray.at(t)[1])
}

// ------------------------------------------------------------------------------------------------------------------ picking

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit3 {
    pub sec: SectionId,
    pub idx: usize,
    /// Path point of an NPC, or the bind box.
    pub sub: Option<usize>,
    /// Distance along the ray.
    pub t: f32,
}

fn priority(s: SectionId) -> u8 {
    match s {
        SectionId::Npcs => 0,
        SectionId::Portals => 1,
        SectionId::SectionB => 2,
        SectionId::Binds => 3,
        SectionId::MobAreas => 4,
        SectionId::Regions => 5,
        _ => 6,
    }
}

fn pickable(layers: &Layers, locks: &Locks, s: SectionId) -> bool {
    view2d::visible(layers, s) && !view2d::locked(locks, s)
}

/// Every record the ray passes through, best first: section order like the 2D view (NPC, portal, B, bind, monster area,
/// region, A), then nearest.
pub fn pick_records(map: &Svmap, layers: &Layers, locks: &Locks, ray: &Ray) -> Vec<Hit3> {
    let mut out: Vec<Hit3> = Vec::new();
    let mut push = |sec: SectionId, idx: usize, sub: Option<usize>, t: Option<f32>| {
        if let Some(t) = t {
            out.push(Hit3 { sec, idx, sub, t });
        }
    };
    let up = |p: V3, d: f32| [p[0], p[1] + d, p[2]];
    if pickable(layers, locks, SectionId::Npcs) {
        for (i, n) in map.npcs.iter().enumerate() {
            let mut best: Option<(f32, usize)> = None;
            for (k, pt) in n.points.iter().enumerate() {
                let t = if k == 0 { math3d::ray_cylinder(ray, pt.pos, NPC_R, NPC_H) } else { math3d::ray_sphere(ray, up(pt.pos, 1.0), PATH_R) };
                if let Some(t) = t {
                    if best.is_none_or(|(b, bk)| (k == 0 && bk != 0) || (k == 0) == (bk == 0) && t < b) {
                        best = Some((t, k));
                    }
                }
            }
            if let Some((t, k)) = best {
                push(SectionId::Npcs, i, Some(k), Some(t));
            }
        }
    }
    if pickable(layers, locks, SectionId::Portals) {
        for (i, p) in map.portals.iter().enumerate() {
            push(SectionId::Portals, i, None, math3d::ray_cylinder(ray, p.pos, PORTAL_R, PORTAL_H));
        }
    }
    if pickable(layers, locks, SectionId::SectionB) {
        for (i, p) in map.points_b.iter().enumerate() {
            push(SectionId::SectionB, i, None, math3d::ray_sphere(ray, up(*p, 1.0), POINT_R));
        }
    }
    let slab = |min: V3, max: V3| {
        let (a, b) = ([min[0].min(max[0]), min[1].min(max[1]) - 0.25, min[2].min(max[2])], [min[0].max(max[0]), min[1].max(max[1]) + 0.25, min[2].max(max[2])]);
        math3d::ray_box(ray, a, b)
    };
    if pickable(layers, locks, SectionId::Binds) {
        for (i, b) in map.binds.iter().enumerate() {
            let mut best: Option<(f32, usize)> = None;
            for (k, x) in b.boxes.iter().enumerate() {
                if let Some(t) = slab(x.min, x.max) {
                    if best.is_none_or(|(bt, _)| t < bt) {
                        best = Some((t, k));
                    }
                }
            }
            if let Some((t, k)) = best {
                push(SectionId::Binds, i, Some(k), Some(t));
            }
        }
    }
    if pickable(layers, locks, SectionId::MobAreas) {
        for (i, a) in map.mob_areas.iter().enumerate() {
            push(SectionId::MobAreas, i, None, slab(a.min, a.max));
        }
    }
    if pickable(layers, locks, SectionId::Regions) {
        for (i, g) in map.regions.iter().enumerate() {
            push(SectionId::Regions, i, None, slab(g.min, g.max));
        }
    }
    if pickable(layers, locks, SectionId::SectionA) {
        for (i, a) in map.section_a.iter().enumerate() {
            push(SectionId::SectionA, i, None, math3d::ray_sphere(ray, up(a.pos, 1.0), POINT_R));
        }
    }
    out.sort_by(|a, b| priority(a.sec).cmp(&priority(b.sec)).then(a.t.total_cmp(&b.t)));
    out
}

/// A click in the viewport selects (like the 2D view): plain click = only that record, Shift toggles, Alt cycles through the
/// records under the cursor, a click on nothing clears (unless Shift).
pub fn click_select(doc: &mut Doc, hits: &[Hit3], shift: bool, alt: bool) {
    if hits.is_empty() {
        if !shift {
            doc.clear_selection();
        }
        return;
    }
    let mut pick = hits[0];
    if alt {
        if let Some(cur) = hits.iter().position(|h| doc.is_selected(h.sec, h.idx)) {
            pick = hits[(cur + 1) % hits.len()];
        }
    }
    if shift {
        doc.toggle(pick.sec, pick.idx);
    } else {
        doc.select_only(pick.sec, pick.idx);
        doc.sub = pick.sub;
    }
}

// ------------------------------------------------------------------------------------------------------------------ dragging

pub enum Drag3d {
    None,
    /// Left drag on empty space: orbit the camera.
    Orbit,
    /// Records follow the ground under the cursor (y follows the ground); with Ctrl only y changes.
    Move { snaps: Vec<(Sel, Rec)>, press_xz: [f32; 2], anchor0: [f32; 2], plane_pt: V3, plane_n: V3, y_press: f32 },
    /// One path point of an NPC.
    MovePoint { sel: Sel, point: usize, before: Rec },
    /// The Box tool: a box on the ground from the press point (snapped x, z).
    Draw { start: [f32; 2] },
}

/// Starts moving what is under the cursor: selects it first when it is not selected (Shift adds). `None` when the ray hits no
/// record. The ground drag needs the ground under the press (or the record's own position when the ray misses the ground).
pub fn begin_move(doc: &mut Doc, hits: &[Hit3], ray: &Ray, cam_forward: V3, ground: &Ground3, shift: bool) -> Option<Drag3d> {
    if doc.read_only {
        return None; // a tab to look at: records do not move
    }
    let hit = *hits.first()?;
    if hit.sec == SectionId::Npcs {
        if let Some(k) = hit.sub.filter(|k| *k > 0) {
            let rec = edit::get(&doc.map, hit.sec, hit.idx)?;
            doc.select_only(hit.sec, hit.idx);
            doc.sub = Some(k);
            return Some(Drag3d::MovePoint { sel: (hit.sec, hit.idx), point: k, before: rec });
        }
    }
    if !doc.is_selected(hit.sec, hit.idx) {
        if shift {
            doc.toggle(hit.sec, hit.idx);
        } else {
            doc.select_only(hit.sec, hit.idx);
        }
    }
    if hit.sec == SectionId::Binds {
        doc.sub = hit.sub;
    }
    let snaps: Vec<(Sel, Rec)> = doc.selection.iter().filter_map(|s| edit::get(&doc.map, s.0, s.1).map(|r| (*s, r))).collect();
    let grabbed = snaps.iter().find(|(s, _)| *s == (hit.sec, hit.idx)).map(|(_, r)| r.clone())?;
    let anchor0 = view2d::anchor_of(&grabbed);
    let press_xz = ground.ray(ray).map_or(anchor0, |g| [g[0], g[2]]);
    let (bmin, _) = record_bounds(&grabbed);
    let plane_pt = [anchor0[0], bmin[1], anchor0[1]];
    let flat = math3d::norm([cam_forward[0], 0.0, cam_forward[2]]);
    let y_press = ray_vertical_plane_y(ray, plane_pt, flat).unwrap_or(plane_pt[1]);
    Some(Drag3d::Move { snaps, press_xz, anchor0, plane_pt, plane_n: flat, y_press })
}

/// Applies the drag for the cursor ray. Records are written into the map directly (the undo step is made by `end_drag`).
pub fn update_drag(doc: &mut Doc, drag: &Drag3d, ray: &Ray, ground: &Ground3, snap: f32, ctrl: bool) {
    match drag {
        Drag3d::Move { snaps, press_xz, anchor0, plane_pt, plane_n, y_press } => {
            if ctrl {
                let Some(y) = ray_vertical_plane_y(ray, *plane_pt, *plane_n) else { return };
                let dy = view2d::snap_to(y - y_press, 0.1);
                for ((_, i), before) in snaps {
                    let mut r = before.clone();
                    edit::translate_y(&mut r, dy);
                    edit::set(&mut doc.map, *i, r);
                }
            } else {
                let Some(g) = ground.ray(ray) else { return };
                let want = [view2d::snap_to(anchor0[0] + (g[0] - press_xz[0]), snap), view2d::snap_to(anchor0[1] + (g[2] - press_xz[1]), snap)];
                let (dx, dz) = (want[0] - anchor0[0], want[1] - anchor0[1]);
                for ((_, i), before) in snaps {
                    let mut r = before.clone();
                    edit::translate(&mut r, dx, dz);
                    edit::drop_to_ground(&mut r, &|x, z| (ground.h)(x, z).unwrap_or(f32::NAN));
                    edit::set(&mut doc.map, *i, r);
                }
            }
        }
        Drag3d::MovePoint { sel, point, before } => {
            let Some(g) = ground.ray(ray) else { return };
            let (x, z) = (view2d::snap_to(g[0], snap), view2d::snap_to(g[2], snap));
            let mut r = before.clone();
            if let Rec::Npc(n) = &mut r {
                if let Some(pt) = n.points.get_mut(*point) {
                    pt.pos = [x, (ground.h)(x, z).unwrap_or(pt.pos[1]), z];
                }
            }
            edit::set(&mut doc.map, sel.1, r);
        }
        Drag3d::Orbit | Drag3d::Draw { .. } | Drag3d::None => {}
    }
}

/// Puts the records back as they were when the drag began.
pub fn cancel_drag(doc: &mut Doc, drag: &Drag3d) {
    match drag {
        Drag3d::Move { snaps, .. } => {
            for ((_, i), r) in snaps {
                edit::set(&mut doc.map, *i, r.clone());
            }
        }
        Drag3d::MovePoint { sel, before, .. } => {
            edit::set(&mut doc.map, sel.1, before.clone());
        }
        _ => {}
    }
}

/// Ends the drag: the change becomes one undo step. Returns whether anything changed.
pub fn end_drag(doc: &mut Doc, drag: &Drag3d) -> bool {
    match drag {
        Drag3d::Move { snaps, .. } => {
            let ops = move_ops(snaps, &doc.map);
            if ops.is_empty() {
                return false;
            }
            let label = if snaps.len() == 1 { format!("move {} (3D)", snaps[0].0 .0.label()) } else { format!("move {} records (3D)", snaps.len()) };
            doc.commit(label, ops);
            true
        }
        Drag3d::MovePoint { sel, point, before } => match edit::get(&doc.map, sel.0, sel.1) {
            Some(after) if after != *before => {
                doc.commit(format!("move NPC point {point} (3D)"), vec![edit::Op::replace(sel.1, before.clone(), after)]);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// The ground point (snapped x, z) a ray meets, `None` when it meets none.
pub fn snapped_ground(ground: &Ground3, ray: &Ray, snap: f32) -> Option<[f32; 2]> {
    ground.ray(ray).map(|g| [view2d::snap_to(g[0], snap), view2d::snap_to(g[2], snap)])
}

/// The Box tool's drag ended in 3D: the box between the press point and the ground under `ray` (else `last`, the last ground
/// point of the drag) becomes a monster area, bind area or region on the lowest ground under it, as one undo step. Returns a
/// toast text for the user when there is one.
#[allow(clippy::too_many_arguments)]
pub fn end_box_draw(doc: &mut Doc, start: [f32; 2], ray: Option<&Ray>, last: Option<[f32; 2]>, ground: &Ground3, snap: f32, target: DrawTarget, template: Option<&crate::settings::Template>) -> Option<&'static str> {
    if doc.read_only {
        return None;
    }
    let end = ray.and_then(|r| snapped_ground(ground, r, snap)).or(last)?;
    view2d::commit_drawn_box(doc, &|x, z| ground.at(x, z), start, end, target, template)
}

/// The measuring point for a ground point: a record anchor within 8 screen pixels (Ctrl turns that off), else Shift snaps the
/// step from `from` to 15 degrees. `world_per_px` is how many units one pixel covers at that distance.
pub fn measure_point_3d(map: &Svmap, g: [f32; 2], from: Option<[f32; 2]>, shift: bool, ctrl: bool, world_per_px: f32) -> [f32; 2] {
    if !ctrl {
        if let Some(a) = crate::measure::snap_in_map(map, g, 1.0 / world_per_px.max(1e-6), crate::measure::SNAP_PX) {
            return a;
        }
    }
    match from {
        Some(f) if shift => crate::measure::snap_angle(f, g, 15.0),
        _ => g,
    }
}

/// Units per screen pixel at a point (for snapping in 3D).
fn world_per_px(cam: &Camera3d, p: V3, viewport_h: f32) -> f32 {
    math3d::len(math3d::sub(p, cam.eye())) * 2.0 * (FOV_Y * 0.5).tan() / viewport_h.max(1.0)
}

/// A right click without a drag opens the context menu on the record under the cursor (it gets selected), or on the ground
/// point when there is no record. `None` when the ray meets neither.
pub fn menu_at(doc: &mut Doc, layers: &Layers, locks: &Locks, ray: &Ray, ground: &Ground3, pos: Pos2) -> Option<view2d::CtxMenu> {
    let hit = pick_records(&doc.map, layers, locks, ray).first().copied();
    let g = ground.ray(ray);
    let world = match (hit, g) {
        (_, Some(g)) => [g[0], g[2]],
        (Some(h), None) => {
            let rec = edit::get(&doc.map, h.sec, h.idx)?;
            let (mn, mx) = record_bounds(&rec);
            [(mn[0] + mx[0]) * 0.5, (mn[2] + mx[2]) * 0.5]
        }
        (None, None) => return None,
    };
    let hit = hit.map(|h| {
        if !doc.is_selected(h.sec, h.idx) {
            doc.select_only(h.sec, h.idx);
            doc.sub = h.sub;
        }
        view2d::Hit { sec: h.sec, idx: h.idx, sub: h.sub }
    });
    Some(view2d::CtxMenu { pos, world, hit })
}

/// The size label of the selection: the box of the selected monster areas, bind areas and regions as "12.0 x 8.0 x 2.0" (x, z,
/// y extents) and its top centre. `None` when none of those is selected.
pub fn selection_size(doc: &Doc) -> Option<(String, V3)> {
    let mut b: Option<(V3, V3)> = None;
    for (s, i) in &doc.selection {
        if !matches!(s, SectionId::MobAreas | SectionId::Binds | SectionId::Regions) {
            continue;
        }
        if let Some(r) = edit::get(&doc.map, *s, *i) {
            let (mn, mx) = record_bounds(&r);
            b = Some(match b {
                None => (mn, mx),
                Some((a0, a1)) => ([a0[0].min(mn[0]), a0[1].min(mn[1]), a0[2].min(mn[2])], [a1[0].max(mx[0]), a1[1].max(mx[1]), a1[2].max(mx[2])]),
            });
        }
    }
    let (mn, mx) = b?;
    Some((format!("{:.1} x {:.1} x {:.1}", mx[0] - mn[0], mx[2] - mn[2], mx[1] - mn[1]), [(mn[0] + mx[0]) * 0.5, mx[1], (mn[2] + mx[2]) * 0.5]))
}

// ------------------------------------------------------------------------------------------------------------------ overlay geometry

fn rgba(c: [u8; 3], a: u8) -> [u8; 4] {
    [c[0], c[1], c[2], a]
}

struct Draw<'a> {
    o: &'a mut OverlayGeo,
    eye: V3,
    ground: &'a dyn Fn(f32, f32) -> f32,
}

impl Draw<'_> {
    fn v(p: V3, col: [u8; 4]) -> OVert {
        OVert { pos: p, col }
    }

    /// Marker scale: records far from the camera are drawn larger so they stay visible.
    fn scale_at(&self, p: V3) -> f32 {
        (math3d::len(math3d::sub(p, self.eye)) / 140.0).clamp(1.0, 12.0)
    }

    fn line(&mut self, a: V3, b: V3, col: [u8; 4], top: bool) {
        let v = if top { &mut self.o.lines_top } else { &mut self.o.lines };
        v.push(Self::v(a, col));
        v.push(Self::v(b, col));
    }

    /// A line between two ground points that follows the terrain (a point every 3 units at most), lifted a little.
    fn ground_line(&mut self, a: [f32; 2], b: [f32; 2], lift: f32, col: [u8; 4], top: bool) {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let n = ((len / 3.0).ceil() as usize).clamp(1, 96);
        let at = |t: f32, s: &Self| {
            let (x, z) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            [x, (s.ground)(x, z) + lift, z]
        };
        let mut prev = at(0.0, self);
        for i in 1..=n {
            let p = at(i as f32 / n as f32, self);
            self.line(prev, p, col, top);
            prev = p;
        }
    }

    fn tri(&mut self, a: V3, b: V3, c: V3, col: [u8; 4], solid: bool) {
        let v = if solid { &mut self.o.tris_solid } else { &mut self.o.tris_blend };
        v.push(Self::v(a, col));
        v.push(Self::v(b, col));
        v.push(Self::v(c, col));
    }

    fn corners(min: V3, max: V3) -> [V3; 8] {
        let (a, b) = ([min[0].min(max[0]), min[1].min(max[1]), min[2].min(max[2])], [min[0].max(max[0]), min[1].max(max[1]), min[2].max(max[2])]);
        [[a[0], a[1], a[2]], [b[0], a[1], a[2]], [b[0], a[1], b[2]], [a[0], a[1], b[2]], [a[0], b[1], a[2]], [b[0], b[1], a[2]], [b[0], b[1], b[2]], [a[0], b[1], b[2]]]
    }

    fn box_edges(&mut self, min: V3, max: V3, col: [u8; 4], top: bool) {
        let c = Self::corners(min, max);
        for (i, j) in [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
            self.line(c[i], c[j], col, top);
        }
    }

    fn box_faces(&mut self, min: V3, max: V3, col: [u8; 4]) {
        let c = Self::corners(min, max);
        for q in [[0, 1, 2, 3], [4, 7, 6, 5], [0, 4, 5, 1], [1, 5, 6, 2], [2, 6, 7, 3], [3, 7, 4, 0]] {
            self.tri(c[q[0]], c[q[1]], c[q[2]], col, false);
            self.tri(c[q[0]], c[q[2]], c[q[3]], col, false);
        }
    }

    /// A circle on the ground (follows the terrain, lifted a little).
    fn ring(&mut self, cx: f32, cz: f32, r: f32, col: [u8; 4], top: bool, segs: usize) {
        let pts: Vec<V3> = (0..=segs)
            .map(|i| {
                let a = i as f32 / segs as f32 * std::f32::consts::TAU;
                let (x, z) = (cx + a.cos() * r, cz + a.sin() * r);
                [x, (self.ground)(x, z) + 0.3, z]
            })
            .collect();
        for w in pts.windows(2) {
            self.line(w[0], w[1], col, top);
        }
    }

    /// A translucent wall of height `h` standing on that ring.
    fn ring_wall(&mut self, cx: f32, cz: f32, r: f32, h: f32, col: [u8; 4], segs: usize) {
        let pt = |s: &Self, i: usize| {
            let a = i as f32 / segs as f32 * std::f32::consts::TAU;
            let (x, z) = (cx + a.cos() * r, cz + a.sin() * r);
            [x, (s.ground)(x, z) + 0.3, z]
        };
        for i in 0..segs {
            let (a, b) = (pt(self, i), pt(self, i + 1));
            let (a2, b2) = ([a[0], a[1] + h, a[2]], [b[0], b[1] + h, b[2]]);
            self.tri(a, b, b2, col, false);
            self.tri(a, b2, a2, col, false);
        }
    }

    fn octa(&mut self, c: V3, r: f32, col: [u8; 4]) {
        let p = |dx: f32, dy: f32, dz: f32| [c[0] + dx * r, c[1] + dy * r, c[2] + dz * r];
        let (px, nx, py, ny, pz, nz) = (p(1.0, 0.0, 0.0), p(-1.0, 0.0, 0.0), p(0.0, 1.0, 0.0), p(0.0, -1.0, 0.0), p(0.0, 0.0, 1.0), p(0.0, 0.0, -1.0));
        for (a, b) in [(px, pz), (pz, nx), (nx, nz), (nz, px)] {
            self.tri(py, a, b, col, true);
            self.tri(ny, b, a, col, true);
        }
    }

    fn pillar(&mut self, base: V3, r: f32, h: f32, col: [u8; 4]) {
        let n = 8;
        let ring = |i: usize, y: f32| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [base[0] + a.cos() * r, base[1] + y, base[2] + a.sin() * r]
        };
        for i in 0..n {
            let (a, b) = (i, i + 1);
            self.tri(ring(a, 0.0), ring(b, 0.0), ring(b, h), col, true);
            self.tri(ring(a, 0.0), ring(b, h), ring(a, h), col, true);
            self.tri([base[0], base[1] + h, base[2]], ring(a, h), ring(b, h), col, true);
        }
    }

    fn cross(&mut self, c: V3, r: f32, col: [u8; 4], top: bool) {
        self.line([c[0] - r, c[1], c[2]], [c[0] + r, c[1], c[2]], col, top);
        self.line([c[0], c[1] - r, c[2]], [c[0], c[1] + r, c[2]], col, top);
        self.line([c[0], c[1], c[2] - r], [c[0], c[1], c[2] + r], col, top);
    }

    /// Outline of a record that is selected or hovered, drawn over everything.
    fn outline(&mut self, r: &Rec, sub: Option<usize>, col: [u8; 4]) {
        match r {
            Rec::Mob(a) => self.box_edges(a.min, a.max, col, true),
            Rec::Region(g) => self.box_edges(g.min, g.max, col, true),
            Rec::Bind(b) => {
                for (k, x) in b.boxes.iter().enumerate() {
                    let c = if sub == Some(k) { WHITE } else { col };
                    self.box_edges(x.min, x.max, c, true);
                }
            }
            Rec::Npc(n) => {
                for (k, p) in n.points.iter().enumerate() {
                    let s = self.scale_at(p.pos);
                    self.ring(p.pos[0], p.pos[2], (if k == 0 { 2.0 } else { 1.2 }) * s, if sub == Some(k) && k > 0 { WHITE } else { col }, true, 20);
                }
                if let Some(p) = n.points.first() {
                    let s = self.scale_at(p.pos);
                    self.line(p.pos, [p.pos[0], p.pos[1] + 5.0 * s, p.pos[2]], col, true);
                }
            }
            Rec::Portal(p) => {
                self.ring(p.pos[0], p.pos[2], PORTAL_USE_RANGE, col, true, 64);
                let s = self.scale_at(p.pos);
                self.line(p.pos, [p.pos[0], p.pos[1] + PORTAL_H * s, p.pos[2]], col, true);
            }
            Rec::B(p) => {
                let s = self.scale_at(*p);
                self.cross([p[0], p[1] + 1.0, p[2]], 2.6 * s, col, true);
            }
            Rec::A(a) => {
                let s = self.scale_at(a.pos);
                self.cross([a.pos[0], a.pos[1] + 1.0, a.pos[2]], 2.6 * s, col, true);
            }
        }
    }
}

/// Everything `build_overlay` needs from the app.
pub struct OverlayIn<'a> {
    pub doc: &'a Doc,
    pub layers: &'a Layers,
    pub look: &'a Lookups<'a>,
    pub ground: &'a dyn Fn(f32, f32) -> f32,
    pub eye: V3,
    pub hover: Option<Hit3>,
    /// Ground point under the cursor.
    pub cursor: Option<V3>,
    pub map_id: Option<u32>,
    /// A record an outliner, Problems or Find row is hovered over (white outline like the viewport hover).
    pub hover_rec: Option<Sel>,
    /// The box the Box tool is dragging on the ground: two corners (x, z).
    pub box_preview: Option<([f32; 2], [f32; 2])>,
    /// The Measure tool's points (x, z), the live one last, and whether the polygon is closed.
    pub measure: &'a [[f32; 2]],
}

fn country_rgb(c: u32) -> [u8; 3] {
    match c {
        1 => [90, 160, 255],
        2 => [255, 100, 100],
        _ => [200, 120, 255],
    }
}

/// The svmap records of the open map as coloured triangles and lines: translucent boxes (monster areas, bind areas, regions),
/// pins with facing arrows and path lines (NPCs), use-range cylinders (portals), crosses (sections A and B), the `.wld` ghosts.
pub fn build_overlay(i: &OverlayIn) -> OverlayGeo {
    let mut o = OverlayGeo::default();
    let mut d = Draw { o: &mut o, eye: i.eye, ground: i.ground };
    let (map, l) = (&i.doc.map, i.layers);
    let flags = view2d::finding_flags(i.doc);
    let flag = |s: SectionId, idx: usize| match flags.get(&(s, idx)) {
        Some(Severity::Error) => Some([255, 70, 70]),
        Some(Severity::Warning) => Some([255, 170, 40]),
        _ => None,
    };

    // the client's own boxes, hollow and grey
    if l.ghosts {
        if let Some(part) = i.doc.part() {
            // the tab's own client half, so a paired edit moves its box at once
            for p in &part.wld.model.portals {
                d.box_edges(p.min, p.max, [150, 150, 150, 150], false);
            }
            for a in &part.wld.model.areas {
                d.box_edges(a.min, a.max, [120, 120, 120, 90], false);
            }
        } else if let Some(w) = i.look.assets.and_then(|a| a.wld.as_ref()) {
            for p in &w.portals {
                d.box_edges(p.min, p.max, [150, 150, 150, 150], false);
            }
            for a in &w.areas {
                d.box_edges(a.min, a.max, [120, 120, 120, 90], false);
            }
        }
    }
    if l.regions {
        for (idx, g) in map.regions.iter().enumerate() {
            let c = flag(SectionId::Regions, idx).unwrap_or([120, 200, 190]);
            d.box_faces(g.min, g.max, rgba(c, 12));
            d.box_edges(g.min, g.max, rgba(c, 170), false);
        }
    }
    if l.binds {
        for (idx, b) in map.binds.iter().enumerate() {
            let c = flag(SectionId::Binds, idx).unwrap_or_else(|| country_rgb(b.country));
            for x in &b.boxes {
                d.box_faces(x.min, x.max, rgba(country_rgb(b.country), 36));
                d.box_edges(x.min, x.max, rgba(c, 220), false);
            }
        }
    }
    if l.mob_areas {
        for (idx, a) in map.mob_areas.iter().enumerate() {
            let level = names::level_colour(i.look.mean_level(a));
            d.box_faces(a.min, a.max, rgba(level, 70));
            d.box_edges(a.min, a.max, rgba(flag(SectionId::MobAreas, idx).unwrap_or(level), 230), false);
        }
    }
    if l.points_b {
        for (idx, p) in map.points_b.iter().enumerate() {
            let s = d.scale_at(*p);
            let c = rgba(flag(SectionId::SectionB, idx).unwrap_or([255, 160, 60]), 255);
            d.cross([p[0], p[1] + 1.0, p[2]], 1.6 * s, c, false);
            d.line(*p, [p[0], p[1] + 1.0, p[2]], c, false);
        }
    }
    if l.section_a {
        for a in &map.section_a {
            let s = d.scale_at(a.pos);
            d.cross([a.pos[0], a.pos[1] + 1.0, a.pos[2]], 1.6 * s, [200, 200, 200, 255], false);
        }
    }
    if l.portals {
        for (idx, p) in map.portals.iter().enumerate() {
            let base = match p.country {
                1 => [110, 170, 255],
                2 => [255, 120, 120],
                _ => [120, 230, 255],
            };
            let c = flag(SectionId::Portals, idx).unwrap_or(base);
            let s = d.scale_at(p.pos);
            d.ring_wall(p.pos[0], p.pos[2], PORTAL_USE_RANGE, 5.0, rgba(base, 20), 48);
            d.ring(p.pos[0], p.pos[2], PORTAL_USE_RANGE, rgba(c, 170), false, 64);
            d.pillar(p.pos, 1.1 * s, 7.0 * s, rgba(base, 235));
            if i.map_id == Some(p.to_map) {
                let (a, b) = ([p.pos[0], p.pos[1] + 5.0, p.pos[2]], [p.to_pos[0], p.to_pos[1] + 5.0, p.to_pos[2]]);
                d.line(a, b, rgba(base, 200), false);
                d.line(b, [b[0], b[1] - 5.0, b[2]], rgba(base, 200), false);
            }
        }
    }
    if l.npcs {
        for (idx, n) in map.npcs.iter().enumerate() {
            let Some(first) = n.points.first() else { continue };
            let base = view2d::npc_rgb(n.kind);
            let c = rgba(base, 255);
            let s = d.scale_at(first.pos);
            d.pillar(first.pos, 0.35 * s, 3.2 * s, c);
            d.octa([first.pos[0], first.pos[1] + 3.8 * s, first.pos[2]], 0.9 * s, c);
            let fwd = [first.dir.sin(), 0.0, first.dir.cos()];
            let tip = [first.pos[0] + fwd[0] * 3.5 * s, first.pos[1] + 0.4, first.pos[2] + fwd[2] * 3.5 * s];
            let from = [first.pos[0], first.pos[1] + 0.4, first.pos[2]];
            d.line(from, tip, c, false);
            let side = [fwd[2], 0.0, -fwd[0]];
            for k in [-1.0f32, 1.0] {
                let back = [tip[0] - fwd[0] * 0.9 * s + side[0] * 0.5 * s * k, tip[1], tip[2] - fwd[2] * 0.9 * s + side[2] * 0.5 * s * k];
                d.line(tip, back, c, false);
            }
            if let Some(fc) = flag(SectionId::Npcs, idx) {
                d.ring(first.pos[0], first.pos[2], 1.8 * s, rgba(fc, 255), false, 16);
            }
            let mut prev = first.pos;
            for p in n.points.iter().skip(1) {
                let ps = d.scale_at(p.pos);
                d.octa([p.pos[0], p.pos[1] + 0.8, p.pos[2]], 0.55 * ps, c);
                d.line([prev[0], prev[1] + 0.6, prev[2]], [p.pos[0], p.pos[1] + 0.6, p.pos[2]], rgba(base, 170), false);
                prev = p.pos;
            }
        }
    }

    // selection and hover over everything
    for (s, idx) in &i.doc.selection {
        if let Some(r) = edit::get(&i.doc.map, *s, *idx) {
            d.outline(&r, i.doc.sub.filter(|_| i.doc.selection.len() == 1), YELLOW);
        }
    }
    if let Some(h) = i.hover {
        if !i.doc.is_selected(h.sec, h.idx) {
            if let Some(r) = edit::get(&i.doc.map, h.sec, h.idx) {
                d.outline(&r, h.sub, WHITE);
            }
        }
    }
    // an outliner or Problems row under the pointer
    if let Some((s, idx)) = i.hover_rec {
        if !i.doc.is_selected(s, idx) && i.hover.is_none_or(|h| (h.sec, h.idx) != (s, idx)) {
            if let Some(r) = edit::get(&i.doc.map, s, idx) {
                d.outline(&r, None, WHITE);
            }
        }
    }
    // the box being drawn, on the ground
    if let Some((a, b)) = i.box_preview {
        let green = [80, 230, 130, 255];
        let (x0, x1, z0, z1) = (a[0].min(b[0]), a[0].max(b[0]), a[1].min(b[1]), a[1].max(b[1]));
        for (p, q) in [([x0, z0], [x1, z0]), ([x1, z0], [x1, z1]), ([x1, z1], [x0, z1]), ([x0, z1], [x0, z0])] {
            d.ground_line(p, q, 0.4, green, true);
        }
    }
    // the measurement, on the ground
    let mc = [255, 120, 255, 255];
    for w in i.measure.windows(2) {
        d.ground_line(w[0], w[1], 0.5, mc, true);
    }
    for p in i.measure {
        let y = (d.ground)(p[0], p[1]);
        let s = d.scale_at([p[0], y, p[1]]);
        d.cross([p[0], y + 0.6, p[1]], 0.9 * s, mc, true);
    }
    // a small cross where the cursor meets the ground
    if let Some(c) = i.cursor {
        let s = d.scale_at(c);
        d.cross([c[0], c[1] + 0.3, c[2]], 0.8 * s, [255, 255, 255, 200], true);
    }
    o
}

// ------------------------------------------------------------------------------------------------------------------ the view

pub struct View3dState {
    pub cam: Camera3d,
    pub drag: Drag3d,
    pub hover: Option<Hit3>,
    pub cursor: Option<V3>,
    pub size: Vec2,
    pub stats: FrameStats,
    /// Camera initialised from the map's bounds.
    pub framed: bool,
    pub backface_cull: bool,
    /// The viewport rectangle of the last frame (egui points), for the scripted headless drag.
    pub rect: Rect,
    /// A camera move that is asked for and starts with the next frame, and the one in progress.
    pub goto: Option<CamPose>,
    pub anim: Option<CamAnim>,
    /// The Measure tool's measurement on the ground.
    pub measure: crate::measure::Measure,
    /// Where the Box tool's drag is now (snapped x, z).
    pub draw_end: Option<[f32; 2]>,
    /// Where and when the right button went down.
    pub rpress: Option<(Pos2, f64)>,
    pub menu: Option<view2d::CtxMenu>,
    /// The ground point an orbit drag turns around (orbit at the cursor).
    pub orbit_pivot: Option<V3>,
}

impl Default for View3dState {
    fn default() -> Self {
        View3dState {
            cam: Camera3d::default(),
            drag: Drag3d::None,
            hover: None,
            cursor: None,
            size: Vec2::new(900.0, 600.0),
            stats: FrameStats::default(),
            framed: false,
            backface_cull: true,
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0)),
            goto: None,
            anim: None,
            measure: Default::default(),
            draw_end: None,
            rpress: None,
            menu: None,
            orbit_pivot: None,
        }
    }
}

impl View3dState {
    /// One frame of a smooth camera move: starts a requested one and advances the one in progress. True while it still runs.
    pub fn step_anim(&mut self, now: f64) -> bool {
        if let Some(to) = self.goto.take() {
            self.anim = Some(CamAnim::new(self.cam.pose(), to, now));
        }
        if let Some(a) = self.anim {
            let (pose, done) = a.at(now);
            self.cam.set_pose(pose);
            if done {
                self.anim = None;
            }
        }
        self.anim.is_some()
    }

    /// The user took the camera: a move in progress or asked for stops.
    pub fn cancel_anim(&mut self) {
        self.anim = None;
        self.goto = None;
    }
}

pub struct Ctx3<'a> {
    pub doc: &'a mut Doc,
    pub layers: &'a Layers,
    /// Cut-off distance of the 3D buildings, shapes and trees.
    pub prop_distance: f32,
    pub locks: &'a Locks,
    pub look: Lookups<'a>,
    pub tool: Tool,
    pub snap: f32,
    pub draw_target: DrawTarget,
    pub npc_template: (u32, u32),
    pub pick_banner: Option<String>,
    pub map_name: &'a dyn Fn(u32) -> String,
    pub ground: Ground3<'a>,
    /// The statistics box (View > Layers > 3D stats).
    pub show_stats: bool,
    /// The spawn template the Box tool draws or stamps with.
    pub template: Option<&'a crate::settings::Template>,
    /// The record a panel row is hovered over.
    pub hover_rec: Option<Sel>,
    /// The app has records to paste (the context menu's "Paste here").
    pub can_paste: bool,
    /// The fixes of the findings of the record under the open context menu.
    pub menu_fixes: &'a [(shaiya_svmap::Finding, Vec<String>)],
    /// A left drag that starts on the ground orbits around that point.
    pub orbit_at_cursor: bool,
    /// The heatmap draped on the terrain.
    pub heat: Option<crate::gpu3d::HeatParams<'a>>,
    /// Frame, Go to and the compass ease over 200 ms.
    pub smooth_camera: bool,
    /// The Placements tool's state.
    pub place: &'a mut crate::place_ui::PlaceUi,
    /// The Terrain tool's state (brush, stroke, ring).
    pub terrain: &'a mut crate::terrain_ui::TerrainUi,
    pub codepage: shaiya_clientdata::wld_raw::Codepage,
}

#[derive(Default)]
pub struct Out3 {
    pub requests: Vec<Request>,
    /// Ground (x, z) under the cursor.
    pub cursor: Option<[f32; 2]>,
}

pub struct GpuRefs<'a> {
    pub rs: &'a egui_wgpu::RenderState,
    pub gpu: &'a mut Gpu3d,
    pub scene: &'a mut SceneGpu,
}

/// The ground-height lookup the 3D view uses for a map: the terrain or dungeon floor of the client data, or a flat plane at
/// y = 0 when there is none.
pub fn ground_lookup(a: Option<&MapAssets>) -> impl Fn(f32, f32) -> Option<f32> + '_ {
    move |x, z| match a {
        None => Some(0.0),
        Some(a) => match &a.ground {
            Ground::None => Some(0.0),
            g => g.height(x, z),
        },
    }
}

fn ndc_of(rect: Rect, p: Pos2) -> [f32; 2] {
    [(p.x - rect.left()) / rect.width().max(1.0) * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height().max(1.0) * 2.0]
}

/// Screen position of a world point, `None` behind the camera.
pub fn to_screen(vp: &Mat4, rect: Rect, p: V3) -> Option<Pos2> {
    let c = math3d::project(vp, p);
    if c[3] <= 1e-4 {
        return None;
    }
    Some(Pos2::new(rect.left() + (c[0] / c[3] * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - c[1] / c[3] * 0.5) * rect.height()))
}

/// Runs one frame of the 3D view: input, picking, drag, drawing. Returns the requests for the app (they need dialogs).
pub fn show(ui: &mut egui::Ui, c: &mut Ctx3, st: &mut View3dState, g: GpuRefs) -> Out3 {
    let mut out = Out3::default();
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
    st.size = rect.size();
    st.rect = rect;
    let aspect = (rect.width() / rect.height().max(1.0)).max(0.05);
    let ppp = ui.ctx().pixels_per_point();
    let bounds = (g.scene.bounds_min, g.scene.bounds_max);
    if !st.framed {
        st.cam.frame_map(bounds.0, bounds.1);
        st.framed = true;
    }
    if c.tool != Tool::Measure && st.measure.active() {
        st.measure.clear(); // a measurement stays until Esc, a new one or a tool change
    }
    let now = ui.input(|i| i.time);
    if st.step_anim(now) {
        ui.ctx().request_repaint();
    }
    let menu_open = st.menu.is_some();

    // ---- keys while flying (held keys move the camera)
    let (dt, shift) = ui.input(|i| (i.stable_dt.min(0.1), i.modifiers.shift));
    if st.cam.fly && !ui.ctx().egui_wants_keyboard_input() {
        let held = ui.input(|i| {
            let k = |a: Key| if i.key_down(a) { 1.0 } else { 0.0 };
            [k(Key::D) - k(Key::A), k(Key::E) - k(Key::Q), k(Key::W) - k(Key::S)]
        });
        if held != [0.0; 3] {
            st.cancel_anim();
            st.cam.fly_move(held, dt, shift);
            ui.ctx().request_repaint();
        }
    }

    // ---- camera: wheel, pan, orbit
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            st.cancel_anim();
            st.cam.zoom(scroll);
        }
    }
    let pan_drag = resp.dragged_by(PointerButton::Middle) || resp.dragged_by(PointerButton::Secondary);
    if pan_drag {
        st.cancel_anim();
        let d = resp.drag_delta();
        st.cam.pan(d.x, d.y, rect.height());
        if !st.cam.fly {
            let f = st.cam.focus;
            st.cam.focus[1] = c.ground.at(f[0], f[2]);
        }
    }

    // ---- pointer
    let ctrl = ui.input(|i| i.modifiers.command);
    let alt = ui.input(|i| i.modifiers.alt);
    let ray_at = |cam: &Camera3d, p: Pos2| cam.ray(ndc_of(rect, p), aspect);
    let primary = PointerButton::Primary;
    let latest = ui.input(|i| i.pointer.latest_pos());
    let banner = c.pick_banner.is_some();
    let forward = st.cam.forward();

    // the context menu: the right button released within 4 px and 400 ms of the press (anything else stays a pan)
    {
        let (pressed, released, pos, time) = ui.input(|i| (i.pointer.button_pressed(PointerButton::Secondary), i.pointer.button_released(PointerButton::Secondary), i.pointer.latest_pos(), i.time));
        if pressed && resp.hovered() && !menu_open {
            st.rpress = pos.map(|p| (p, time));
        }
        if released {
            if let (Some((p0, t0)), Some(p)) = (st.rpress.take(), pos) {
                if !banner && matches!(st.drag, Drag3d::None) && view2d::is_menu_click(p0, t0, p, time) {
                    let ray = ray_at(&st.cam, p);
                    st.menu = menu_at(c.doc, c.layers, c.locks, &ray, &c.ground, p);
                }
            }
        }
    }

    if resp.drag_started_by(primary) && !menu_open {
        if let Some(p0) = ui.input(|i| i.pointer.press_origin()) {
            if !compass_hit(rect, p0) {
                let ray = ray_at(&st.cam, p0);
                st.drag = Drag3d::Orbit;
                st.orbit_pivot = None;
                if c.tool == Tool::Select && !banner {
                    let hits = pick_records(&c.doc.map, c.layers, c.locks, &ray);
                    if let Some(d) = begin_move(c.doc, &hits, &ray, forward, &c.ground, shift) {
                        st.drag = d;
                    }
                } else if c.tool == Tool::Place && !banner && c.place.armed.is_none() {
                    let src = c.look.rules.map(|r| &r.source);
                    let hits = crate::place_ui::pick_ray(c.doc, c.place, src, ray.origin, ray.dir);
                    if let (Some(h), Some(g)) = (hits.first(), c.ground.ray(&ray)) {
                        crate::place_ui::begin_drag(c.doc, c.place, *h, [g[0], g[2]], shift);
                        if c.place.drag.is_some() {
                            st.drag = Drag3d::None;
                        }
                    }
                } else if c.tool == Tool::Terrain && !banner && !alt {
                    // a press on the ground starts a stroke; a press in the sky still orbits
                    if let Some(g) = c.ground.ray(&ray) {
                        if c.terrain.begin(c.doc, [g[0], g[2]], shift).is_ok() {
                            st.drag = Drag3d::None;
                        }
                    }
                } else if c.tool == Tool::Box && !banner && c.draw_target != DrawTarget::PointB && !c.doc.read_only {
                    if let Some(s) = snapped_ground(&c.ground, &ray, c.snap) {
                        st.drag = Drag3d::Draw { start: s };
                        st.draw_end = Some(s);
                    }
                }
                if matches!(st.drag, Drag3d::Orbit) {
                    st.cancel_anim();
                    if c.orbit_at_cursor {
                        if let Some(gp) = c.ground.ray(&ray).filter(|_| !st.cam.fly) {
                            st.cam.orbit_around(gp);
                            st.orbit_pivot = Some(gp);
                        }
                    }
                }
            }
        }
    }
    if resp.dragged_by(primary) && c.place.drag.is_some() {
        if let (Some(p), Some(d)) = (latest, c.place.drag.as_mut()) {
            if let Some(g) = c.ground.ray(&ray_at(&st.cam, p)) {
                d.delta = [g[0] - d.press[0], g[2] - d.press[1]];
            }
        }
    }
    if resp.dragged_by(primary) && c.terrain.stroke.is_some() {
        if let Some(p) = latest {
            if let Some(g) = c.ground.ray(&ray_at(&st.cam, p)) {
                c.terrain.drag(c.doc, [g[0], g[2]], (dt * 60.0).clamp(0.25, 4.0));
            }
            ui.ctx().request_repaint();
        }
    }
    if resp.dragged_by(primary) {
        match &st.drag {
            Drag3d::Orbit => {
                let d = resp.drag_delta();
                match st.orbit_pivot {
                    Some(pv) => st.cam.orbit_about(pv, d.x, d.y),
                    None => st.cam.orbit(d.x, d.y),
                }
            }
            Drag3d::None => {}
            Drag3d::Draw { .. } => {
                if let Some(p) = latest {
                    st.draw_end = snapped_ground(&c.ground, &ray_at(&st.cam, p), c.snap).or(st.draw_end);
                }
            }
            drag => {
                if let Some(p) = latest {
                    update_drag(c.doc, drag, &ray_at(&st.cam, p), &c.ground, c.snap, ctrl);
                }
            }
        }
    }
    if resp.drag_stopped_by(primary) {
        let d = std::mem::replace(&mut st.drag, Drag3d::None);
        st.orbit_pivot = None;
        if c.place.drag.is_some() {
            let ground = crate::place_ui::field_ground(c.look.assets);
            if let Some(why) = crate::place_ui::finish_drag(c.doc, c.place, c.snap, ground) {
                out.requests.push(Request::Toast(why));
            }
        }
        if c.terrain.stroke.is_some() {
            match c.terrain.finish(c.doc) {
                Ok(Some(rep)) => out.requests.push(Request::Toast(rep.message())),
                Ok(None) => {}
                Err(e) => out.requests.push(Request::Toast(e)),
            }
        }
        if let Drag3d::Draw { start } = d {
            // a release over the sky keeps the last ground point the preview showed
            let ray = latest.map(|p| ray_at(&st.cam, p));
            if let Some(t) = end_box_draw(c.doc, start, ray.as_ref(), st.draw_end, &c.ground, c.snap, c.draw_target, c.template) {
                out.requests.push(Request::Toast(t.to_string()));
            }
            st.draw_end = None;
        } else {
            end_drag(c.doc, &d);
        }
    }
    if c.tool == Tool::Place && !ui.ctx().egui_wants_keyboard_input() {
        // Ctrl+R is Rulers
        if ui.input(|i| i.key_pressed(Key::R) && !i.modifiers.command && !i.modifiers.alt) {
            if let Err(why) = crate::place_ui::rotate_selected(c.doc, if shift { -15.0 } else { 15.0 }) {
                out.requests.push(Request::Toast(why));
            }
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) && c.place.drag.take().is_none() && c.place.armed.take().is_none() {
            c.doc.place_sel.clear();
        }
    }
    if c.tool == Tool::Terrain && !ui.ctx().egui_wants_keyboard_input() {
        let (open, close) = ui.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
        if open || close {
            let r = &mut c.terrain.params.radius;
            *r = (*r * if close { 1.25 } else { 0.8 }).clamp(crate::terrain::MIN_RADIUS, crate::terrain::MAX_RADIUS);
        }
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        c.terrain.abort(c.doc);
        let d = std::mem::replace(&mut st.drag, Drag3d::None);
        cancel_drag(c.doc, &d);
        st.draw_end = None;
        st.measure.clear();
    }
    if c.tool == Tool::Measure && !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            st.measure.finish();
        }
        if ui.input(|i| i.key_pressed(Key::Backspace)) {
            st.measure.pop();
        }
    }

    // ---- hover: what is under the cursor, and where the cursor meets the ground
    let hover_pos = resp.hover_pos();
    st.cursor = None;
    st.hover = None;
    if let Some(p) = hover_pos {
        let ray = ray_at(&st.cam, p);
        st.cursor = c.ground.ray(&ray);
        if matches!(st.drag, Drag3d::None) && c.tool == Tool::Select && !banner && !menu_open && !compass_hit(rect, p) {
            st.hover = pick_records(&c.doc.map, c.layers, c.locks, &ray).first().copied();
        }
    }
    out.cursor = st.cursor.map(|p| [p[0], p[2]]);

    // ---- clicks
    if resp.clicked_by(primary) && !menu_open {
        if let Some(p) = resp.interact_pointer_pos() {
            let ray = ray_at(&st.cam, p);
            let ground = c.ground.ray(&ray);
            let snap = c.snap;
            if compass_hit(rect, p) {
                let doc: &Doc = c.doc;
                start_command(st, Cmd3d::LookNorth, doc, bounds, c.smooth_camera);
            } else if banner {
                if let Some(gp) = ground {
                    let (x, z) = (view2d::snap_to(gp[0], snap), view2d::snap_to(gp[2], snap));
                    out.requests.push(Request::PickDone([x, c.ground.at(x, z), z]));
                }
            } else {
                match c.tool {
                    Tool::Select => {
                        let hits = pick_records(&c.doc.map, c.layers, c.locks, &ray);
                        click_select(c.doc, &hits, shift, alt);
                    }
                    Tool::Npc => {
                        if let Some(gp) = ground {
                            let (x, z) = (view2d::snap_to(gp[0], snap), view2d::snap_to(gp[2], snap));
                            let n = shaiya_svmap::NpcEntry { kind: c.npc_template.0, type_id: c.npc_template.1, points: vec![shaiya_svmap::NpcPoint { pos: [x, c.ground.at(x, z), z], dir: 0.0 }] };
                            let idx = c.doc.map.npcs.len();
                            c.doc.commit("add NPC", vec![edit::Op::insert(idx, Rec::Npc(n))]);
                            c.doc.select_only(SectionId::Npcs, idx);
                        }
                    }
                    Tool::Portal => {
                        if let Some(gp) = ground {
                            let (x, z) = (view2d::snap_to(gp[0], snap), view2d::snap_to(gp[2], snap));
                            out.requests.push(Request::AddPortal([x, c.ground.at(x, z), z]));
                        }
                    }
                    Tool::Box if c.draw_target == DrawTarget::PointB => {
                        if let Some(gp) = ground {
                            let (x, z) = (view2d::snap_to(gp[0], snap), view2d::snap_to(gp[2], snap));
                            let idx = c.doc.map.points_b.len();
                            c.doc.commit("add section B point", vec![edit::Op::insert(idx, Rec::B([x, c.ground.at(x, z), z]))]);
                            c.doc.select_only(SectionId::SectionB, idx);
                        }
                    }
                    Tool::Box => {
                        // a click with a template stamps a box of its size on the ground
                        if let (DrawTarget::MobArea, Some(t), Some(gp)) = (c.draw_target, c.template, ground) {
                            let gr = &c.ground;
                            view2d::stamp_template(c.doc, &|x, z| gr.at(x, z), [gp[0], gp[2]], snap, t);
                        }
                    }
                    Tool::Measure => {
                        if let Some(gp) = ground {
                            let wpp = world_per_px(&st.cam, gp, rect.height());
                            let building = !st.measure.done && st.measure.circle.is_none();
                            let vp = st.cam.view_proj(aspect);
                            let closes = building
                                && st.measure.points.len() >= 3
                                && to_screen(&vp, rect, ground_point(&c.ground, st.measure.points[0])).is_some_and(|s| s.distance(p) <= crate::measure::SNAP_PX);
                            if closes {
                                st.measure.close(c.doc.map.size);
                            } else {
                                let from = if building { st.measure.points.last().copied() } else { None };
                                let q = measure_point_3d(&c.doc.map, [gp[0], gp[2]], from, shift, ctrl, wpp);
                                // the second click of a double click lands where the first one did
                                let again = building && from.is_some_and(|l| to_screen(&vp, rect, ground_point(&c.ground, l)).is_some_and(|s| s.distance(p) < 3.0));
                                if !again {
                                    st.measure.click(q);
                                }
                                if resp.double_clicked_by(primary) {
                                    st.measure.finish();
                                }
                            }
                        }
                    }
                    Tool::Place => {
                        let ground_ref = crate::place_ui::field_ground(c.look.assets);
                        if c.place.armed.is_some() {
                            if let Some(gp) = ground {
                                if let Err(why) = crate::place_ui::add_at(c.doc, c.place, [gp[0], gp[2]], Some(gp[1]), snap, ground_ref, c.codepage) {
                                    out.requests.push(Request::Toast(why));
                                }
                            }
                        } else {
                            let src = c.look.rules.map(|r| &r.source);
                            let hits = crate::place_ui::pick_ray(c.doc, c.place, src, ray.origin, ray.dir);
                            crate::place_ui::click_select(c.doc, &hits, shift, alt);
                        }
                    }
                    Tool::Terrain => {
                        // a click is one dab of the brush
                        if let Some(gp) = ground {
                            match c.terrain.begin(c.doc, [gp[0], gp[2]], shift) {
                                Ok(()) => match c.terrain.finish(c.doc) {
                                    Ok(Some(rep)) => out.requests.push(Request::Toast(rep.message())),
                                    Ok(None) => {}
                                    Err(e) => out.requests.push(Request::Toast(e)),
                                },
                                Err(e) => out.requests.push(Request::Toast(e)),
                            }
                        }
                    }
                    Tool::Paint | Tool::Scatter => out.requests.push(Request::Toast("This tool draws in the 2D view (Tab switches).".to_string())),
                }
            }
        }
    }
    if banner && ui.input(|i| i.key_pressed(Key::Escape)) {
        out.requests.push(Request::CancelPick);
    }
    if resp.hovered() || !matches!(st.drag, Drag3d::None) {
        let on_compass = hover_pos.is_some_and(|p| compass_hit(rect, p));
        let icon = match (&st.drag, st.hover) {
            (Drag3d::Move { .. } | Drag3d::MovePoint { .. }, _) => egui::CursorIcon::Grabbing,
            (Drag3d::Orbit, _) => egui::CursorIcon::AllScroll,
            (Drag3d::Draw { .. }, _) => egui::CursorIcon::Crosshair,
            _ if on_compass => egui::CursorIcon::PointingHand,
            (_, Some(_)) => egui::CursorIcon::PointingHand,
            _ if c.tool != Tool::Select || banner => egui::CursorIcon::Crosshair,
            _ => egui::CursorIcon::Default,
        };
        ui.ctx().set_cursor_icon(icon);
    }

    // ---- what the tools show on the ground
    let box_preview = match (&st.drag, st.draw_end) {
        (Drag3d::Draw { start }, Some(e)) => Some((*start, e)),
        _ => None,
    };
    let mut measure_pts = st.measure.chain();
    let mut live: Option<[f32; 2]> = None;
    if c.tool == Tool::Measure && !st.measure.done && st.measure.circle.is_none() && matches!(st.drag, Drag3d::None) && !menu_open {
        if let (Some(last), Some(cur)) = (st.measure.points.last().copied(), st.cursor) {
            let q = measure_point_3d(&c.doc.map, [cur[0], cur[2]], Some(last), shift, ctrl, world_per_px(&st.cam, cur, rect.height()));
            measure_pts.push(q);
            live = Some(q);
        }
    }

    // ---- draw: overlay geometry, the scene into its texture, the texture into the panel
    if c.terrain.stroke.is_some() && !ui.input(|i| i.pointer.primary_down()) {
        // the button went up where the view did not see it
        match c.terrain.finish(c.doc) {
            Ok(Some(rep)) => out.requests.push(Request::Toast(rep.message())),
            Ok(None) => {}
            Err(e) => out.requests.push(Request::Toast(e)),
        }
    }
    c.terrain.hover = if c.tool == Tool::Terrain { st.cursor.map(|p| [p[0], p[2]]).or(c.terrain.pin) } else { None };
    let eye = st.cam.eye();
    let vp = st.cam.view_proj(aspect);
    let ground_fn = |x: f32, z: f32| c.ground.at(x, z);
    let mut overlay = build_overlay(&OverlayIn { doc: c.doc, layers: c.layers, look: &c.look, ground: &ground_fn, eye, hover: st.hover, cursor: st.cursor, map_id: c.look.map_id, hover_rec: c.hover_rec, box_preview, measure: &measure_pts });
    {
        let src = c.look.rules.map(|r| &r.source);
        crate::place_ui::overlay_3d(&mut overlay, c.doc, c.place, src, &g.scene.props_missing, st.cursor, c.tool == Tool::Place, c.snap, crate::place_ui::field_ground(c.look.assets));
    }
    if c.tool == Tool::Terrain {
        c.terrain.overlay_3d(&mut overlay, &ground_fn);
    }
    let size_px = [(rect.width() * ppp).round().max(8.0) as u32, (rect.height() * ppp).round().max(8.0) as u32];
    let far_fog = (st.cam.dist * 14.0).max(1500.0);
    let params = FrameParams {
        view_proj: vp,
        eye,
        size_px,
        fog_start: if st.cam.fly { 400.0 } else { st.cam.dist * 2.5 },
        fog_end: far_fog,
        blocked: c.layers.blocked.then_some(&c.doc.map.blocked),
        blocked_rev: c.doc.blocked_rev,
        backface_cull: st.backface_cull,
        draw_ground: c.layers.terrain,
        draw_dungeon: c.layers.dungeon,
        draw_buildings: c.layers.buildings3d,
        draw_trees: c.layers.trees3d,
        prop_distance: c.prop_distance,
        heat: c.heat,
    };
    let (tex, stats) = g.gpu.render(g.rs, g.scene, &params, &overlay);
    st.stats = stats;
    let painter = ui.painter_at(rect);
    painter.image(tex, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

    draw_labels(&painter, c, st, &vp, rect);
    draw_missing_models(&painter, c, &vp, rect, &g.scene.props_missing, &g.scene.props_failed);
    draw_selection_size(&painter, c.doc, &vp, rect);
    draw_tool_labels(&painter, c, st, &vp, rect, &measure_pts, live, box_preview);
    draw_hud(&painter, c, st, rect, g.scene);
    draw_compass(&painter, rect, st.cam.yaw, hover_pos.is_some_and(|p| compass_hit(rect, p)));
    if let Some(b) = &c.pick_banner {
        let r = Rect::from_min_size(rect.left_top() + Vec2::new(8.0, 8.0), Vec2::new(rect.width() - 16.0, 24.0));
        painter.rect_filled(r, 4.0, Color32::from_rgba_unmultiplied(160, 110, 0, 230));
        painter.text(r.center(), Align2::CENTER_CENTER, b, FontId::proportional(13.0), Color32::WHITE);
    }

    // the context menu
    if let Some(m) = st.menu.clone() {
        let menu_in = view2d::MenuIn { read_only: c.doc.read_only, can_paste: c.can_paste, template: c.template, fixes: c.menu_fixes };
        let mut close = false;
        if let Some(a) = view2d::context_menu(ui.ctx(), &menu_in, &m, &mut close) {
            out.requests.push(Request::Menu(a));
        }
        if close {
            st.menu = None;
        }
    }

    // hover tooltip like the 2D view
    if let (Some(h), Drag3d::None, false) = (st.hover, &st.drag, menu_open) {
        let mut text = format!("{} #{}: {}", h.sec.label(), h.idx, names::label(&c.look, &c.doc.map, h.sec, h.idx, c.map_name));
        for f in c.doc.findings.iter().filter(|f| f.section == h.sec && f.index == Some(h.idx)) {
            text.push_str(&format!("\n{}: {}", f.severity.label(), f.message));
        }
        resp.clone().on_hover_text_at_pointer(text);
    }
    out
}

/// The point (x, z) of a measurement with the ground under it as y.
fn ground_point(ground: &Ground3, p: [f32; 2]) -> V3 {
    [p[0], ground.at(p[0], p[1]), p[1]]
}

/// A label on a dark plate.
fn plate(p: &egui::Painter, pos: Pos2, anchor: Align2, text: String, col: Color32) {
    let pal = crate::theme::pal(p.ctx());
    let galley = p.layout_no_wrap(text, FontId::proportional(11.0), col);
    let r = anchor.anchor_size(pos, galley.size());
    p.rect_filled(r.expand(2.0), 2.0, pal.overlay_bg);
    p.galley(r.min, galley, col);
}

/// The compass: a ring with N, E, S and W that turns with the camera. A click on it faces north.
fn draw_compass(p: &egui::Painter, rect: Rect, yaw: f32, hot: bool) {
    let pal = crate::theme::pal(p.ctx());
    let c = compass_centre(rect);
    p.circle_filled(c, COMPASS_R, pal.overlay_bg);
    p.circle_stroke(c, COMPASS_R, Stroke::new(if hot { 2.0 } else { 1.0 }, pal.overlay_text));
    for (deg, name) in [(0.0, "N"), (90.0, "E"), (180.0, "S"), (270.0, "W")] {
        let d = compass_dir(yaw, deg);
        let col = if deg == 0.0 { Color32::from_rgb(255, 110, 100) } else { pal.overlay_text };
        p.text(c + d * (COMPASS_R - 9.0), Align2::CENTER_CENTER, name, FontId::proportional(if deg == 0.0 { 12.0 } else { 10.0 }), col);
    }
    // a tick towards north outside the letters
    let n = compass_dir(yaw, 0.0);
    p.line_segment([c + n * (COMPASS_R - 2.0), c + n * (COMPASS_R + 4.0)], Stroke::new(2.0, Color32::from_rgb(255, 110, 100)));
}

/// The size of the selected boxes above them.
fn draw_selection_size(p: &egui::Painter, doc: &Doc, vp: &Mat4, rect: Rect) {
    if let Some((text, at)) = selection_size(doc) {
        if let Some(s) = to_screen(vp, rect, [at[0], at[1] + 1.5, at[2]]) {
            if rect.contains(s) {
                plate(p, s, Align2::CENTER_BOTTOM, text, Color32::from_rgb(255, 235, 60));
            }
        }
    }
}

/// The labels of the Measure tool (a length at each segment's middle, the summary at the end) and the Box tool's size.
#[allow(clippy::too_many_arguments)]
fn draw_tool_labels(p: &egui::Painter, c: &Ctx3, st: &View3dState, vp: &Mat4, rect: Rect, pts: &[[f32; 2]], live: Option<[f32; 2]>, box_preview: Option<([f32; 2], [f32; 2])>) {
    let text = Color32::from_rgb(255, 160, 255);
    for w in pts.windows(2) {
        let mid = [(w[0][0] + w[1][0]) * 0.5, (w[0][1] + w[1][1]) * 0.5];
        if let Some(s) = to_screen(vp, rect, ground_point(&c.ground, mid)) {
            if rect.contains(s) {
                plate(p, s + Vec2::new(0.0, -8.0), Align2::CENTER_BOTTOM, format!("{:.1}", crate::measure::dist(w[0], w[1])), text);
            }
        }
    }
    let mut shown = st.measure.clone();
    if let Some(l) = live {
        shown.points.push(l);
    }
    if let (Some(last), Some(sum)) = (pts.last(), shown.summary(&|x, z| c.ground.at(x, z))) {
        if let Some(s) = to_screen(vp, rect, ground_point(&c.ground, *last)) {
            if rect.contains(s) {
                plate(p, s + Vec2::new(10.0, 10.0), Align2::LEFT_TOP, sum, Color32::WHITE);
            }
        }
    }
    if let Some((a, b)) = box_preview {
        if let Some(s) = to_screen(vp, rect, ground_point(&c.ground, b)) {
            plate(p, s + Vec2::new(8.0, 8.0), Align2::LEFT_TOP, format!("{:.1} x {:.1}", (b[0] - a[0]).abs(), (b[1] - a[1]).abs()), Color32::WHITE);
        }
    }
}

/// The name of every placement whose model is not drawn (still loading, or it could not be loaded) over its cyan box.
fn draw_missing_models(p: &egui::Painter, c: &Ctx3, vp: &Mat4, rect: Rect, missing: &[(crate::place::PlaceList, usize)], failed: &std::collections::HashSet<(crate::scene3d::PropClass, String)>) {
    use crate::place::PlaceList;
    use crate::scene3d::PropClass;
    let Some(part) = c.doc.part() else { return };
    let m = &part.wld.model;
    for (l, i) in missing.iter().take(60) {
        let Some(pl) = crate::place::get(m, (*l, *i)) else { continue };
        let Some(name) = crate::place::model_name(l.list(m), &pl) else { continue };
        let class = match l {
            PlaceList::Building => PropClass::Building,
            PlaceList::Shape => PropClass::Shape,
            PlaceList::Tree => PropClass::Tree,
            PlaceList::Dungeon => continue,
        };
        let text = if failed.contains(&(class, name.to_ascii_lowercase())) { format!("{name} (not loaded)") } else { name };
        if let Some(s) = to_screen(vp, rect, [pl.pos[0], pl.pos[1] + 2.0, pl.pos[2]]) {
            if rect.contains(s) {
                plate(p, s, Align2::CENTER_BOTTOM, text, Color32::from_rgb(120, 230, 240));
            }
        }
    }
}

fn draw_labels(p: &egui::Painter, c: &Ctx3, st: &View3dState, vp: &Mat4, rect: Rect) {
    if !c.layers.names {
        return;
    }
    let eye = st.cam.eye();
    let font = FontId::proportional(11.0);
    let mut shown = 0;
    let label = |at: V3, text: String, col: Color32| {
        if let Some(s) = to_screen(vp, rect, at) {
            if rect.contains(s) {
                let galley = p.layout_no_wrap(text, font.clone(), col);
                let r = Rect::from_min_size(s + Vec2::new(8.0, -14.0), galley.size()).expand(2.0);
                p.rect_filled(r, 2.0, Color32::from_black_alpha(150));
                p.galley(r.min + Vec2::splat(2.0), galley, col);
            }
        }
    };
    // NPC names near the camera (the nearest ones), portals, then everything selected
    if c.layers.npcs {
        let mut near: Vec<(f32, usize)> = c.doc.map.npcs.iter().enumerate().filter_map(|(i, n)| n.points.first().map(|p| (math3d::len(math3d::sub(p.pos, eye)), i))).filter(|(d, _)| *d < 260.0).collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, i) in near.into_iter().take(40) {
            let n = &c.doc.map.npcs[i];
            let pos = n.points[0].pos;
            label([pos[0], pos[1] + 5.2, pos[2]], c.look.npc_name(n), Color32::from_rgb(235, 235, 235));
            shown += 1;
        }
    }
    if c.layers.portals {
        for (i, pt) in c.doc.map.portals.iter().enumerate() {
            let same = c.look.map_id == Some(pt.to_map);
            let text = if same { format!("#{i}") } else { format!("#{i} -> {}", (c.map_name)(pt.to_map)) };
            label([pt.pos[0], pt.pos[1] + 9.0, pt.pos[2]], text, Color32::from_rgb(130, 230, 255));
        }
    }
    for (s, i) in &c.doc.selection {
        if *s == SectionId::Npcs && shown > 0 {
            continue;
        }
        if let Some(r) = edit::get(&c.doc.map, *s, *i) {
            let (mn, mx) = record_bounds(&r);
            let top = [(mn[0] + mx[0]) * 0.5, mx[1] + 1.0, (mn[2] + mx[2]) * 0.5];
            label(top, names::label(&c.look, &c.doc.map, *s, *i, c.map_name), Color32::from_rgb(255, 235, 60));
        }
    }
}

/// The 3D view's overlay: a one-line notice when there is no client data, and the statistics box when View > Layers > 3D stats is
/// on. The key hints are in the status bar.
fn draw_hud(p: &egui::Painter, c: &Ctx3, st: &View3dState, rect: Rect, scene: &SceneGpu) {
    let pal = crate::theme::pal(p.ctx());
    let font = FontId::monospace(11.0);
    let mut lines = Vec::new();
    if c.show_stats {
        let e = st.cam.eye();
        lines.push(format!("3D  {}  eye ({:.0}, {:.0}, {:.0})  {}", if st.cam.fly { "fly" } else { "orbit" }, e[0], e[1], e[2], if st.cam.fly { format!("speed {:.0}", st.cam.fly_speed) } else { format!("distance {:.0}", st.cam.dist) }));
        if let Some(g) = st.cursor {
            lines.push(format!("ground x {:.1}  z {:.1}  y {:.1}", g[0], g[2], g[1]));
        }
        lines.push(format!("{} draw calls, {} triangles", st.stats.draw_calls, st.stats.triangles));
        let [nb, ns, nt] = scene.prop_counts;
        if nb + ns + nt > 0 {
            lines.push(format!("{} of {} buildings, shapes and trees drawn (cut-off {:.0})", st.stats.props_drawn, st.stats.props_total, c.prop_distance));
        }
    }
    if scene.flat {
        lines.push("No client data for this map. Showing a flat plane.".to_string());
    } else if scene.is_dungeon && c.show_stats {
        lines.push(format!("dungeon, {} triangles in the file", scene.dungeon_triangles));
    }
    if lines.is_empty() {
        return;
    }
    let text = lines.join("\n");
    let galley = p.layout_no_wrap(text, font, pal.overlay_text);
    let r = Rect::from_min_size(rect.left_top() + Vec2::new(8.0, if c.pick_banner.is_some() { 38.0 } else { 8.0 }), galley.size()).expand(4.0);
    p.rect_filled(r, 3.0, pal.overlay_bg);
    p.galley(r.min + Vec2::splat(4.0), galley, pal.overlay_text);
    let _ = Stroke::NONE;
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;
    use shaiya_clientdata::wld::Terrain;
    use shaiya_svmap::{MobArea, NpcEntry, NpcPoint, Portal, Region};

    /// A 256-unit map with a gentle hill and a ridge, as a height function.
    fn terrain() -> Terrain {
        let n = 129;
        let mut heights = Vec::new();
        for cz in 0..n {
            for cx in 0..n {
                let (x, z) = (cx as f32 * 2.0, cz as f32 * 2.0);
                let h = 20.0 + 8.0 * (x * 0.03).sin() + 6.0 * (z * 0.045).cos() + x * 0.05;
                heights.push(((h + 200.0) / 0.02).round() as i16);
            }
        }
        Terrain { size: 256, n, heights, tex_index: vec![0; n * n] }
    }

    fn doc_with(terrain: &Terrain) -> Doc {
        let mut m = Svmap::new(256);
        let g = |x: f32, z: f32| terrain.height_at(x, z);
        m.npcs.push(NpcEntry { kind: 1, type_id: 1, points: vec![NpcPoint { pos: [100.0, g(100.0, 100.0), 100.0], dir: 0.0 }, NpcPoint { pos: [108.0, g(108.0, 100.0), 100.0], dir: 0.0 }] });
        m.portals.push(Portal { pos: [60.0, g(60.0, 60.0), 60.0], country: 0, level_min: 0, level_max: 0, to_map: 1, to_pos: [10.0, 10.0, 10.0] });
        m.mob_areas.push(MobArea { min: [150.0, g(150.0, 150.0), 150.0], max: [170.0, g(150.0, 150.0) + 2.0, 170.0], mobs: Vec::new() });
        m.regions.push(Region { min: [90.0, g(100.0, 100.0) - 5.0, 90.0], max: [120.0, g(100.0, 100.0) + 30.0, 120.0], kind: 0, b: 0, c: 0, d: 0 });
        m.points_b.push([200.0, g(200.0, 200.0), 200.0]);
        Doc::new(1, Some(1), None, m, None)
    }

    fn cam_above(x: f32, z: f32) -> Camera3d {
        Camera3d { focus: [x, 20.0, z], yaw: 0.0, pitch: -60f32.to_radians(), dist: 150.0, ..Default::default() }
    }

    #[test]
    fn camera_rays_pass_through_the_projected_point() {
        let cam = cam_above(128.0, 128.0);
        let aspect = 1.6;
        let vp = cam.view_proj(aspect);
        for p in [[128.0, 20.0, 128.0], [140.0, 25.0, 150.0], [90.0, 10.0, 110.0]] {
            let c = math3d::project(&vp, p);
            let ndc = [c[0] / c[3], c[1] / c[3]];
            let ray = cam.ray(ndc, aspect);
            let t = math3d::dot(math3d::sub(p, ray.origin), ray.dir);
            let q = ray.at(t);
            assert!(math3d::len(math3d::sub(q, p)) < 0.05, "{p:?} -> {q:?}");
        }
    }

    #[test]
    fn camera_basics_orbit_pan_zoom_fly() {
        let mut cam = cam_above(128.0, 128.0);
        // looking north: east is on the right
        assert!(cam.right()[0] > 0.99);
        let e0 = cam.eye();
        assert!(e0[2] < 128.0 && e0[1] > 20.0, "the eye is south of and above the focus");
        cam.orbit(100.0, 0.0);
        assert!(cam.yaw < 0.0);
        cam.orbit(0.0, -100000.0);
        assert!(cam.pitch <= MAX_ORBIT_PITCH + 1e-6, "the orbit camera stays above the ground");
        let d = cam.dist;
        cam.zoom(120.0);
        assert!(cam.dist < d);
        // pan: dragging right moves the focus left
        let mut c2 = cam_above(128.0, 128.0);
        c2.pan(50.0, 0.0, 600.0);
        assert!(c2.focus[0] < 128.0 && (c2.focus[2] - 128.0).abs() < 1e-3);
        // fly
        let mut c3 = cam_above(128.0, 128.0);
        let eye = c3.eye();
        c3.toggle_fly();
        assert!(c3.fly && c3.eye() == eye);
        c3.fly_move([0.0, 0.0, 1.0], 1.0, false);
        assert!(math3d::len(math3d::sub(c3.eye(), eye)) > 50.0);
        c3.toggle_fly();
        assert!(!c3.fly);
    }

    #[test]
    fn frame_box_shows_the_whole_box() {
        let mut cam = Camera3d::default();
        cam.frame_box([100.0, 0.0, 100.0], [140.0, 10.0, 130.0]);
        let aspect = 1.5;
        let vp = cam.view_proj(aspect);
        for p in [[100.0, 0.0, 100.0], [140.0, 10.0, 130.0], [100.0, 10.0, 130.0], [140.0, 0.0, 100.0]] {
            let c = math3d::project(&vp, p);
            assert!(c[3] > 0.0 && (c[0] / c[3]).abs() <= 1.0 && (c[1] / c[3]).abs() <= 1.0, "{p:?} is outside the view");
        }
    }

    #[test]
    fn ground_ray_meets_the_terrain() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let cam = cam_above(128.0, 128.0);
        let ray = cam.ray([0.0, 0.0], 1.5);
        let hit = g.ray(&ray).expect("the centre ray hits the ground");
        assert!((hit[1] - t.height_at(hit[0], hit[2])).abs() < 0.01, "hit is on the surface");
        // the hit lies on the ray (the focus point itself is below the ground here, so the ray ends earlier)
        let t = math3d::dot(math3d::sub(hit, ray.origin), ray.dir);
        assert!(math3d::len(math3d::sub(ray.at(t), hit)) < 0.05);
        // looking up at the sky
        let sky = Ray { origin: [128.0, 50.0, 128.0], dir: math3d::norm([0.0, 1.0, 0.3]) };
        assert!(g.ray(&sky).is_none());
        // off the map
        let away = Ray { origin: [500.0, 50.0, 500.0], dir: math3d::norm([0.0, -1.0, 0.0]) };
        assert!(g.ray(&away).is_none());
    }

    #[test]
    fn dungeon_style_ground_with_holes() {
        // floor only where x < 100; no floor elsewhere
        let h = |x: f32, _z: f32| if x < 100.0 { Some(5.0) } else { None };
        let g = Ground3 { size: 256.0, h: &h };
        let r = Ray { origin: [50.0, 30.0, 50.0], dir: [0.0, -1.0, 0.0] };
        assert!((g.ray(&r).unwrap()[1] - 5.0).abs() < 0.01);
        let r2 = Ray { origin: [150.0, 30.0, 50.0], dir: [0.0, -1.0, 0.0] };
        assert!(g.ray(&r2).is_none());
    }

    #[test]
    fn picking_prefers_npcs_then_nearest_and_respects_layers_and_locks() {
        let t = terrain();
        let doc = doc_with(&t);
        let (layers, mut locks) = (Layers::default(), Locks::default());
        // a ray straight down through the NPC, which stands inside the region box
        let y = t.height_at(100.0, 100.0);
        let ray = Ray { origin: [100.0, y + 60.0, 100.0], dir: [0.0, -1.0, 0.0] };
        let hits = pick_records(&doc.map, &layers, &locks, &ray);
        assert_eq!(hits[0].sec, SectionId::Npcs);
        assert!(hits.iter().any(|h| h.sec == SectionId::Regions));
        assert_eq!(hits[0].sub, Some(0));
        // the NPC lock removes it
        locks.npcs = true;
        assert_eq!(pick_records(&doc.map, &layers, &locks, &ray)[0].sec, SectionId::Regions);
        // a hidden layer too
        let l2 = Layers { regions: false, ..Default::default() };
        assert!(pick_records(&doc.map, &l2, &locks, &ray).is_empty());
        // path point
        let ray2 = Ray { origin: [108.0, y + 60.0, 100.0], dir: [0.0, -1.0, 0.0] };
        let h2 = pick_records(&doc.map, &layers, &Locks::default(), &ray2);
        assert_eq!((h2[0].sec, h2[0].sub), (SectionId::Npcs, Some(1)));
        // portal, area and B are hit from above too
        let p = doc.map.portals[0].pos;
        let top = Ray { origin: [p[0], p[1] + 50.0, p[2]], dir: [0.0, -1.0, 0.0] };
        assert_eq!(pick_records(&doc.map, &layers, &Locks::default(), &top)[0].sec, SectionId::Portals);
        let a = doc.map.mob_areas[0].min;
        let atop = Ray { origin: [a[0] + 5.0, a[1] + 50.0, a[2] + 5.0], dir: [0.0, -1.0, 0.0] };
        assert_eq!(pick_records(&doc.map, &layers, &Locks::default(), &atop)[0].sec, SectionId::MobAreas);
        let b = doc.map.points_b[0];
        let btop = Ray { origin: [b[0], b[1] + 50.0, b[2]], dir: [0.0, -1.0, 0.0] };
        assert_eq!(pick_records(&doc.map, &layers, &Locks::default(), &btop)[0].sec, SectionId::SectionB);
    }

    #[test]
    fn click_selects_like_2d_and_alt_cycles() {
        let t = terrain();
        let mut doc = doc_with(&t);
        let y = t.height_at(100.0, 100.0);
        let ray = Ray { origin: [100.0, y + 60.0, 100.0], dir: [0.0, -1.0, 0.0] };
        let hits = pick_records(&doc.map, &Layers::default(), &Locks::default(), &ray);
        click_select(&mut doc, &hits, false, false);
        assert_eq!(doc.selection, vec![(SectionId::Npcs, 0)]);
        // Alt: the next record under the cursor
        click_select(&mut doc, &hits, false, true);
        assert_eq!(doc.selection, vec![(SectionId::Regions, 0)]);
        // a click on nothing clears, Shift keeps
        click_select(&mut doc, &[], true, false);
        assert_eq!(doc.selection.len(), 1);
        click_select(&mut doc, &[], false, false);
        assert!(doc.selection.is_empty());
    }

    #[allow(clippy::too_many_arguments)] // a test helper that mirrors the drag handler's inputs one for one
    fn drag_to(doc: &mut Doc, drag: &Drag3d, cam: &Camera3d, aspect: f32, rect: Rect, to: Pos2, g: &Ground3, snap: f32, ctrl: bool) {
        let ray = cam.ray(ndc_of(rect, to), aspect);
        update_drag(doc, drag, &ray, g, snap, ctrl);
    }

    #[test]
    fn ground_drag_keeps_y_on_the_ground_and_is_one_undo_step() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let before = doc.map.clone();
        let cam = cam_above(100.0, 95.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let aspect = rect.width() / rect.height();
        let vp = cam.view_proj(aspect);
        // where the NPC pin is on screen
        let pin = doc.map.npcs[0].points[0].pos;
        let at = to_screen(&vp, rect, [pin[0], pin[1] + 2.0, pin[2]]).unwrap();
        let ray = cam.ray(ndc_of(rect, at), aspect);
        let hits = pick_records(&doc.map, &Layers::default(), &Locks::default(), &ray);
        assert_eq!(hits[0].sec, SectionId::Npcs, "the pin is under the cursor");
        let drag = begin_move(&mut doc, &hits, &ray, cam.forward(), &g, false).expect("a drag starts");
        assert_eq!(doc.selection, vec![(SectionId::Npcs, 0)], "pressing selects");
        // drag over several places; every time the NPC's points are on the ground
        for dx in [10.0, 25.0, -20.0, 40.0] {
            let to = Pos2::new(at.x + dx * 3.0, at.y - dx);
            drag_to(&mut doc, &drag, &cam, aspect, rect, to, &g, 0.1, false);
            let Some(Rec::Npc(n)) = edit::get(&doc.map, SectionId::Npcs, 0) else { panic!() };
            for p in &n.points {
                assert!((p.pos[1] - t.height_at(p.pos[0], p.pos[2])).abs() <= 0.01, "y = ground at ({}, {}): {} vs {}", p.pos[0], p.pos[2], p.pos[1], t.height_at(p.pos[0], p.pos[2]));
            }
            // the path point keeps its offset from the pin
            assert!((n.points[1].pos[0] - n.points[0].pos[0] - 8.0).abs() < 0.2 + 1e-3);
        }
        assert!(end_drag(&mut doc, &drag));
        assert_eq!(doc.undo.len(), 1);
        assert_ne!(doc.map, before);
        doc.undo();
        assert_eq!(doc.map, before, "one undo returns the original map");
    }

    /// Acceptance (M5), real data (needs `SHAIYA_CLIENT` and `SVMAP_ORIGINALS`): on maps 1, 18, 35 (fields) and 65 (a dungeon)
    /// a ground drag leaves NPC points and monster areas on the ground (y = ground within 0.01), as one undo step.
    #[test]
    fn real_maps_ground_drag_keeps_y_on_the_ground() {
        let (Some(c), Some(orig)) = (std::env::var_os("SHAIYA_CLIENT"), std::env::var_os("SVMAP_ORIGINALS")) else {
            eprintln!("skipped: SHAIYA_CLIENT and SVMAP_ORIGINALS are not both set");
            return;
        };
        let rules = crate::client_rules::ClientRules::open(std::path::Path::new(&c)).unwrap();
        let tints = crate::assets::TintCache::default();
        for id in [1u32, 18, 35, 65] {
            let bytes = std::fs::read(std::path::Path::new(&orig).join(format!("{id}.svmap"))).unwrap();
            let map = shaiya_svmap::read(&bytes).unwrap();
            let size = map.size;
            let assets = crate::assets::load(&rules, &tints, id, size);
            let h = ground_lookup(Some(&assets));
            let g = Ground3 { size: size as f32, h: &h };
            let mut doc = Doc::new(1, Some(id), None, map.clone(), None);
            let (layers, locks) = (Layers::default(), Locks::default());
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
            let aspect = rect.width() / rect.height();
            let (mut moved, mut checked) = (0, 0);
            for (sec, idx) in [(SectionId::Npcs, 0usize), (SectionId::MobAreas, 0), (SectionId::MobAreas, 1)] {
                let Some(rec) = edit::get(&doc.map, sec, idx) else { continue }; // map 35 has no monster areas
                let (bmin, bmax) = record_bounds(&rec);
                let centre = [(bmin[0] + bmax[0]) * 0.5, (bmin[1] + bmax[1]) * 0.5, (bmin[2] + bmax[2]) * 0.5];
                let cam = Camera3d { focus: centre, yaw: 0.3, pitch: -60f32.to_radians(), dist: 90.0, ..Default::default() };
                let vp = cam.view_proj(aspect);
                let aim = if sec == SectionId::Npcs { [centre[0], bmin[1] + 2.0, centre[2]] } else { [centre[0], centre[1], centre[2]] };
                let at = to_screen(&vp, rect, aim).unwrap();
                let ray = cam.ray(ndc_of(rect, at), aspect);
                let mut hits = pick_records(&doc.map, &layers, &locks, &ray);
                hits.retain(|x| x.sec == sec && x.idx == idx);
                assert!(!hits.is_empty(), "map {id} {sec:?} {idx}: the ray at the record's screen position picks it");
                let before = doc.map.clone();
                let drag = begin_move(&mut doc, &hits, &ray, cam.forward(), &g, false).unwrap();
                for (dx, dy) in [(40.0, 10.0), (-90.0, 30.0), (120.0, -60.0), (10.0, 90.0)] {
                    let ray2 = cam.ray(ndc_of(rect, Pos2::new(at.x + dx, at.y + dy)), aspect);
                    update_drag(&mut doc, &drag, &ray2, &g, 0.1, false);
                    let now = edit::get(&doc.map, sec, idx).unwrap();
                    let ys: Vec<(f32, f32)> = match &now {
                        Rec::Npc(n) => n.points.iter().map(|p| (p.pos[1], h(p.pos[0], p.pos[2]).unwrap_or(p.pos[1]))).collect(),
                        Rec::Mob(a) => {
                            let lowest = (0..=8).flat_map(|i| (0..=8).map(move |j| (i, j))).map(|(i, j)| h(a.min[0] + (a.max[0] - a.min[0]) * i as f32 / 8.0, a.min[2] + (a.max[2] - a.min[2]) * j as f32 / 8.0).unwrap_or(f32::NAN)).fold(f32::MAX, f32::min);
                            vec![(a.min[1], lowest)]
                        }
                        _ => unreachable!(),
                    };
                    for (y, ground) in ys {
                        checked += 1;
                        assert!((y - ground).abs() <= 0.01, "map {id} {sec:?} {idx}: y {y} vs ground {ground}");
                    }
                    if now != rec {
                        moved += 1;
                    }
                }
                end_drag(&mut doc, &drag);
                doc.undo();
                assert_eq!(doc.map, before, "map {id}: one undo step puts the record back");
            }
            assert!(moved > 0, "map {id}: the drags moved something");
            eprintln!("map {id}: ground drag checked {checked} positions, {moved} moves, y = ground within 0.01 each time");
        }
    }

    /// Picking and the ground ray must be cheap on the biggest map (map 1: 835 areas, 286 NPCs).
    #[test]
    fn picking_map_1_is_fast() {
        let Some(orig) = std::env::var_os("SVMAP_ORIGINALS") else {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            return;
        };
        let map = shaiya_svmap::read(&std::fs::read(std::path::Path::new(&orig).join("1.svmap")).unwrap()).unwrap();
        let cam = Camera3d { focus: [1000.0, 60.0, 1000.0], yaw: 0.5, pitch: -40f32.to_radians(), dist: 300.0, ..Default::default() };
        let t0 = std::time::Instant::now();
        let n = 2000;
        let mut total = 0;
        for i in 0..n {
            let ray = cam.ray([(i as f32 / n as f32) * 2.0 - 1.0, 0.1], 1.6);
            total += pick_records(&map, &Layers::default(), &Locks::default(), &ray).len();
        }
        let per = t0.elapsed().as_secs_f64() * 1e6 / n as f64;
        eprintln!("map 1: pick_records {per:.1} us per ray ({total} hits over {n} rays, debug build)");
        // the time is printed, not asserted: a timing limit makes a test fail on a busy machine
    }

    #[test]
    fn dragging_a_monster_area_keeps_it_a_slab_on_the_ground() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let cam = cam_above(160.0, 150.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let aspect = 1.5;
        let vp = cam.view_proj(aspect);
        let a = doc.map.mob_areas[0].clone();
        let c = [(a.min[0] + a.max[0]) * 0.5, a.min[1] + 1.0, (a.min[2] + a.max[2]) * 0.5];
        let at = to_screen(&vp, rect, c).unwrap();
        let ray = cam.ray(ndc_of(rect, at), aspect);
        let hits = pick_records(&doc.map, &Layers::default(), &Locks::default(), &ray);
        assert_eq!(hits[0].sec, SectionId::MobAreas);
        let drag = begin_move(&mut doc, &hits, &ray, cam.forward(), &g, false).unwrap();
        drag_to(&mut doc, &drag, &cam, aspect, rect, Pos2::new(at.x - 60.0, at.y - 30.0), &g, 1.0, false);
        end_drag(&mut doc, &drag);
        let m = &doc.map.mob_areas[0];
        assert!((m.max[1] - m.min[1] - 2.0).abs() < 1e-3, "still a 2 unit slab");
        let lowest = (0..=8).flat_map(|i| (0..=8).map(move |j| (i, j))).map(|(i, j)| t.height_at(m.min[0] + (m.max[0] - m.min[0]) * i as f32 / 8.0, m.min[2] + (m.max[2] - m.min[2]) * j as f32 / 8.0)).fold(f32::MAX, f32::min);
        assert!((m.min[1] - lowest).abs() <= 0.01, "y1 is the lowest ground under the box");
        // snapped to whole units
        assert_eq!(m.min[0].fract(), 0.0);
        assert!((m.max[0] - m.min[0] - 20.0).abs() < 1e-3, "size unchanged");
    }

    #[test]
    fn ctrl_drag_changes_only_the_height_and_escape_restores() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let before = doc.map.clone();
        let cam = cam_above(100.0, 70.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let aspect = 1.5;
        let vp = cam.view_proj(aspect);
        let p = doc.map.points_b[0];
        let _ = p;
        let pin = doc.map.npcs[0].points[0].pos;
        let at = to_screen(&vp, rect, [pin[0], pin[1] + 2.0, pin[2]]).unwrap();
        let ray = cam.ray(ndc_of(rect, at), aspect);
        let hits = pick_records(&doc.map, &Layers::default(), &Locks::default(), &ray);
        let drag = begin_move(&mut doc, &hits, &ray, cam.forward(), &g, false).unwrap();
        drag_to(&mut doc, &drag, &cam, aspect, rect, Pos2::new(at.x, at.y - 80.0), &g, 0.1, true);
        let Some(Rec::Npc(n)) = edit::get(&doc.map, SectionId::Npcs, 0) else { panic!() };
        assert_eq!((n.points[0].pos[0], n.points[0].pos[2]), (pin[0], pin[2]), "x and z unchanged");
        assert!(n.points[0].pos[1] > pin[1] + 1.0, "moved up");
        cancel_drag(&mut doc, &drag);
        assert_eq!(doc.map, before);
        assert_eq!(doc.undo.len(), 0);
    }

    #[test]
    fn dragging_an_npc_path_point_moves_it_alone() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let ray = Ray { origin: [108.0, 100.0, 100.0], dir: [0.0, -1.0, 0.0] };
        let hits = pick_records(&doc.map, &Layers::default(), &Locks::default(), &ray);
        let drag = begin_move(&mut doc, &hits, &ray, [0.0, -0.7, 0.7], &g, false).unwrap();
        assert!(matches!(drag, Drag3d::MovePoint { point: 1, .. }));
        let to = Ray { origin: [120.3, 100.0, 90.7], dir: [0.0, -1.0, 0.0] };
        update_drag(&mut doc, &drag, &to, &g, 1.0, false);
        end_drag(&mut doc, &drag);
        let Some(Rec::Npc(n)) = edit::get(&doc.map, SectionId::Npcs, 0) else { panic!() };
        assert_eq!(n.points[0].pos[0], 100.0);
        assert_eq!((n.points[1].pos[0], n.points[1].pos[2]), (120.0, 91.0));
        assert!((n.points[1].pos[1] - t.height_at(120.0, 91.0)).abs() < 0.01);
    }

    #[test]
    fn switching_2d_3d_keeps_selection_and_focus() {
        let t = terrain();
        let mut doc = doc_with(&t);
        doc.select_only(SectionId::Portals, 0);
        let g = |x: f32, z: f32| t.height_at(x, z);
        let mut cam = Camera3d::default();
        doc.view = crate::doc::View { cx: 77.0, cz: 130.0, zoom: 2.0, fitted: true };
        enter_from_2d(&mut cam, &doc.view, Vec2::new(900.0, 600.0), &g);
        assert_eq!((cam.focus[0], cam.focus[2]), (77.0, 130.0));
        assert!((cam.focus[1] - t.height_at(77.0, 130.0)).abs() < 1e-3);
        // move the focus in 3D, go back
        cam.focus[0] = 150.0;
        cam.focus[2] = 40.0;
        leave_to_2d(&cam, &mut doc.view, Vec2::new(900.0, 600.0));
        assert_eq!((doc.view.cx, doc.view.cz), (150.0, 40.0));
        assert!((doc.view.zoom - 2.0).abs() < 0.5, "about the same extent: {}", doc.view.zoom);
        assert_eq!(doc.selection, vec![(SectionId::Portals, 0)], "selection stays");
    }

    #[test]
    fn keys_map_to_commands_and_apply() {
        assert_eq!(key_command(Key::F, false), Some(Cmd3d::FrameSelection));
        assert_eq!(key_command(Key::F, true), Some(Cmd3d::ToggleFly));
        assert_eq!(key_command(Key::Home, false), Some(Cmd3d::FrameMap));
        assert_eq!(key_command(Key::Q, false), None);
        let t = terrain();
        let mut doc = doc_with(&t);
        let mut cam = Camera3d::default();
        // nothing selected: F does nothing
        let c0 = cam;
        apply_command(&mut cam, Cmd3d::FrameSelection, &doc, ([0.0; 3], [256.0, 50.0, 256.0]));
        assert_eq!(cam, c0);
        doc.select_only(SectionId::Portals, 0);
        apply_command(&mut cam, Cmd3d::FrameSelection, &doc, ([0.0; 3], [256.0, 50.0, 256.0]));
        assert!((cam.focus[0] - 60.0).abs() < 1.0 && (cam.focus[2] - 60.0).abs() < 1.0);
        apply_command(&mut cam, Cmd3d::FrameMap, &doc, ([0.0; 3], [256.0, 50.0, 256.0]));
        assert_eq!((cam.focus[0], cam.focus[2]), (128.0, 128.0));
        apply_command(&mut cam, Cmd3d::ToggleFly, &doc, ([0.0; 3], [256.0, 50.0, 256.0]));
        assert!(cam.fly);
    }

    #[test]
    fn overlay_has_every_kind_of_primitive_and_marks_the_selection_on_top() {
        let t = terrain();
        let mut doc = doc_with(&t);
        let layers = Layers::default();
        let look = Lookups::none();
        let g = |x: f32, z: f32| t.height_at(x, z);
        let mk = |doc: &Doc, hover| build_overlay(&OverlayIn { doc, layers: &layers, look: &look, ground: &g, eye: [128.0, 120.0, 20.0], hover, cursor: Some([100.0, 20.0, 100.0]), map_id: Some(1), hover_rec: None, box_preview: None, measure: &[] });
        let o = mk(&doc, None);
        assert!(!o.tris_solid.is_empty() && !o.tris_blend.is_empty() && !o.lines.is_empty());
        assert!(o.tris_solid.len() % 3 == 0 && o.tris_blend.len() % 3 == 0 && o.lines.len() % 2 == 0 && o.lines_top.len() % 2 == 0);
        let n_top = o.lines_top.len();
        doc.select_only(SectionId::MobAreas, 0);
        let o2 = mk(&doc, None);
        assert_eq!(o2.lines_top.len(), n_top + 24, "a selected box adds its 12 edges on top");
        assert!(o2.lines_top.iter().any(|v| v.col == YELLOW));
        // hidden layers draw nothing of theirs
        let mut l2 = layers.clone();
        l2.mob_areas = false;
        l2.regions = false;
        l2.binds = false;
        l2.npcs = false;
        l2.portals = false;
        l2.points_b = false;
        let o3 = build_overlay(&OverlayIn { doc: &doc, layers: &l2, look: &look, ground: &g, eye: [0.0; 3], hover: None, cursor: None, map_id: None, hover_rec: None, box_preview: None, measure: &[] });
        assert!(o3.tris_blend.is_empty() && o3.tris_solid.is_empty() && o3.lines.is_empty());
    }

    #[test]
    fn far_records_are_drawn_larger() {
        let mut o = OverlayGeo::default();
        let g = |_: f32, _: f32| 0.0;
        let d = Draw { o: &mut o, eye: [0.0, 0.0, 0.0], ground: &g };
        assert_eq!(d.scale_at([10.0, 0.0, 0.0]), 1.0);
        assert!(d.scale_at([1400.0, 0.0, 0.0]) >= 10.0);
        assert!(d.scale_at([100000.0, 0.0, 0.0]) <= 12.0);
    }

    // ---- R4: the 3D polish

    #[test]
    fn compass_click_faces_north() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let c = compass_centre(rect);
        assert!(c.x > 800.0 && c.y < 80.0, "top right: {c:?}");
        assert!(compass_hit(rect, c) && compass_hit(rect, c + Vec2::new(COMPASS_R, 0.0)));
        assert!(!compass_hit(rect, c + Vec2::new(COMPASS_R + 10.0, 0.0)) && !compass_hit(rect, Pos2::new(100.0, 300.0)));
        // the letters turn with the camera: facing east puts north on the left, facing north puts it up
        let up = compass_dir(0.0, 0.0);
        assert!(up.x.abs() < 1e-5 && (up.y + 1.0).abs() < 1e-5, "north is up when facing north: {up:?}");
        let east = compass_dir(std::f32::consts::FRAC_PI_2, 0.0);
        assert!((east.x + 1.0).abs() < 1e-5 && east.y.abs() < 1e-5, "north is on the left when facing east: {east:?}");
        let e = compass_dir(std::f32::consts::FRAC_PI_2, 90.0);
        assert!(e.x.abs() < 1e-5 && (e.y + 1.0).abs() < 1e-5, "east is up when facing east");
        // the click is a LookNorth command: the camera turns the short way and ends at yaw 0, with the focus where it was
        let t = terrain();
        let doc = doc_with(&t);
        let mut st = View3dState { cam: Camera3d { focus: [128.0, 20.0, 128.0], yaw: 2.5, pitch: -0.7, dist: 200.0, ..Default::default() }, ..Default::default() };
        let bounds = ([0.0, 0.0, 0.0], [256.0, 60.0, 256.0]);
        start_command(&mut st, Cmd3d::LookNorth, &doc, bounds, true);
        assert!(st.step_anim(10.0), "the move starts at the next frame");
        assert!((st.cam.yaw - 2.5).abs() < 0.05);
        assert!(st.step_anim(10.1), "half way");
        assert!(st.cam.yaw > 0.0 && st.cam.yaw < 2.5);
        assert!(!st.step_anim(10.25));
        assert_eq!(st.cam.yaw, 0.0);
        assert_eq!(st.cam.focus, [128.0, 20.0, 128.0]);
        // without smoothing it is at once
        st.cam.yaw = -3.0;
        start_command(&mut st, Cmd3d::LookNorth, &doc, bounds, false);
        assert_eq!(st.cam.yaw, 0.0);
    }

    #[test]
    fn orbit_at_cursor_moves_focus() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut cam = cam_above(128.0, 128.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let aspect = rect.width() / rect.height();
        // the ground under a point off the centre of the view
        let ray = cam.ray([0.5, -0.2], aspect);
        let p = g.ray(&ray).expect("the ray meets the ground");
        let at0 = to_screen(&cam.view_proj(aspect), rect, p).unwrap();
        let eye = cam.eye();
        let before = cam.focus;
        // the press: the focus moves to the point's depth, nothing on the screen moves
        cam.orbit_around(p);
        assert!(math3d::len(math3d::sub(cam.eye(), eye)) < 1e-2, "the camera stays where it was: {:?} vs {:?}", cam.eye(), eye);
        assert!(math3d::len(math3d::sub(cam.focus, before)) > 1.0, "the focus moved");
        let depth = math3d::dot(math3d::sub(p, eye), cam.forward());
        assert!((cam.dist - depth).abs() < 1e-2, "the focus is at the depth of the point: {} vs {depth}", cam.dist);
        assert!(to_screen(&cam.view_proj(aspect), rect, p).unwrap().distance(at0) < 0.5, "no jump at the press");
        // the drag turns the camera around the point: it stays put on the screen and the eye keeps its distance to it
        let d0 = math3d::len(math3d::sub(cam.eye(), p));
        let (yaw0, pitch0) = (cam.yaw, cam.pitch);
        for (dx, dy) in [(40.0, 0.0), (0.0, 25.0), (-120.0, -30.0), (0.0, 400.0)] {
            cam.orbit_about(p, dx, dy);
            let s = to_screen(&cam.view_proj(aspect), rect, p).unwrap();
            assert!(s.distance(at0) < 1.0, "the point stays under the cursor: {s:?} vs {at0:?}");
            assert!((math3d::len(math3d::sub(cam.eye(), p)) - d0).abs() < 0.05);
            assert!(cam.focus.iter().chain(cam.eye().iter()).all(|v| v.is_finite()));
        }
        assert!((cam.yaw - yaw0).abs() > 0.1 && (cam.pitch - pitch0).abs() > 0.01, "the camera turned");
        assert!(cam.pitch >= MIN_PITCH && cam.pitch <= MAX_ORBIT_PITCH, "the pitch stays clamped");
        // the plain turn is the same turn
        let mut plain = cam_above(128.0, 128.0);
        let mut piv = cam_above(128.0, 128.0);
        plain.orbit(30.0, 10.0);
        piv.orbit_about(p, 30.0, 10.0);
        assert_eq!((plain.yaw, plain.pitch), (piv.yaw, piv.pitch));
        // a point behind or at the eye leaves the camera alone
        let mut c2 = cam_above(128.0, 128.0);
        let c0 = c2;
        c2.orbit_around(c2.eye());
        assert_eq!(c2, c0);
        // flying: nothing changes
        let mut fly = Camera3d { fly: true, ..cam_above(128.0, 128.0) };
        let f0 = fly.focus;
        fly.orbit_around(p);
        assert_eq!(fly.focus, f0);
    }

    #[test]
    fn smooth_frame_reaches_target() {
        let t = terrain();
        let mut doc = doc_with(&t);
        doc.select_only(SectionId::MobAreas, 0);
        let bounds = ([0.0, 0.0, 0.0], [256.0, 60.0, 256.0]);
        let mut st = View3dState { cam: cam_above(30.0, 30.0), ..Default::default() };
        let want = {
            let mut c = st.cam;
            apply_command(&mut c, Cmd3d::FrameSelection, &doc, bounds);
            c.pose()
        };
        start_command(&mut st, Cmd3d::FrameSelection, &doc, bounds, true);
        assert_eq!(st.cam, cam_above(30.0, 30.0), "nothing moves before the first frame");
        let start = st.cam.pose();
        assert!(st.step_anim(5.0));
        assert_eq!(st.cam.pose(), start, "t = 0");
        // the ease is fast at the start: a quarter of the time covers well over a quarter of the way
        st.step_anim(5.05);
        let total = math3d::len(math3d::sub(want.focus, start.focus));
        let done = math3d::len(math3d::sub(st.cam.focus, start.focus));
        assert!(done > total * 0.4 && done < total, "{done} of {total}");
        assert!(st.step_anim(5.15));
        assert!(!st.step_anim(5.25), "200 ms later it is over");
        assert_eq!(st.cam.pose(), want, "exactly the target");
        // the user taking the camera stops a move
        start_command(&mut st, Cmd3d::FrameMap, &doc, bounds, true);
        st.step_anim(6.0);
        st.cancel_anim();
        let there = st.cam.pose();
        assert!(!st.step_anim(6.1));
        assert_eq!(st.cam.pose(), there);
        // the ease curve
        assert_eq!((ease_out_cubic(0.0), ease_out_cubic(1.0)), (0.0, 1.0));
        assert!(ease_out_cubic(0.5) > 0.8);
        // the yaw takes the short way round: from 350 to 10 degrees is 20 degrees, not 340
        let a = CamAnim::new(CamPose { focus: [0.0; 3], yaw: 350f32.to_radians(), pitch: -0.5, dist: 100.0 }, CamPose { focus: [0.0; 3], yaw: 10f32.to_radians(), pitch: -0.5, dist: 100.0 }, 0.0);
        let (mid, _) = a.at(0.1);
        assert!(mid.yaw > 350f32.to_radians() && mid.yaw < 370f32.to_radians(), "{}", mid.yaw.to_degrees());
    }

    #[test]
    fn box_tool_3d_draws_on_ground() {
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let cam = cam_above(60.0, 200.0);
        let aspect = 1.5;
        let (r0, r1) = (cam.ray([-0.2, -0.1], aspect), cam.ray([0.3, 0.2], aspect));
        let start = snapped_ground(&g, &r0, 1.0).expect("ground at the press");
        let end = snapped_ground(&g, &r1, 1.0).expect("ground at the release");
        let n = doc.map.mob_areas.len();
        assert_eq!(end_box_draw(&mut doc, start, Some(&r1), None, &g, 1.0, DrawTarget::MobArea, None), None);
        assert_eq!(doc.map.mob_areas.len(), n + 1, "one area");
        assert_eq!(doc.undo.len(), 1, "one undo step");
        let a = &doc.map.mob_areas[n];
        assert_eq!((a.min[0], a.min[2], a.max[0], a.max[2]), (start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])));
        // y1 is the lowest ground under it (corners and centre), y2 = y1 + 2
        let mut lo = f32::MAX;
        for (fx, fz) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
            lo = lo.min(t.height_at(a.min[0] + (a.max[0] - a.min[0]) * fx, a.min[2] + (a.max[2] - a.min[2]) * fz));
        }
        assert!((a.min[1] - lo).abs() <= 0.01, "y1 {} vs lowest ground {}", a.min[1], lo);
        assert!((a.max[1] - a.min[1] - 2.0).abs() < 1e-4);
        assert_eq!(doc.selection, vec![(SectionId::MobAreas, n)]);
        doc.undo();
        assert_eq!(doc.map.mob_areas.len(), n);
        // a region says what it needs; a bind area needs no toast
        assert!(end_box_draw(&mut doc, start, Some(&r1), None, &g, 1.0, DrawTarget::Region, None).is_some());
        assert_eq!(end_box_draw(&mut doc, start, Some(&r1), None, &g, 1.0, DrawTarget::Bind, None), None);
        // a release over the sky keeps the last ground point of the drag
        let sky = Ray { origin: [128.0, 500.0, 128.0], dir: [0.0, 1.0, 0.0] };
        assert_eq!(end_box_draw(&mut doc, start, Some(&sky), Some(end), &g, 1.0, DrawTarget::MobArea, None), None);
        assert_eq!(doc.map.mob_areas.len(), n + 1, "drawn from the last ground point");
        assert_eq!(end_box_draw(&mut doc, start, Some(&sky), None, &g, 1.0, DrawTarget::MobArea, None), None);
        assert_eq!(doc.map.mob_areas.len(), n + 1, "no ground at all: nothing");
        doc.undo();
        // a box under half a unit is nothing
        let m = doc.map.clone();
        assert_eq!(end_box_draw(&mut doc, start, Some(&cam.ray([-0.2, -0.1], aspect)), None, &g, 1.0, DrawTarget::MobArea, None), None);
        assert_eq!(doc.map, m);
        // a click with a template stamps its size, on the ground
        let tpl = crate::settings::Template { name: "w".into(), size: [10.0, 6.0], height: 3.0, entries: vec![[5, 2]] };
        let gr = &g;
        view2d::stamp_template(&mut doc, &|x, z| gr.at(x, z), [80.0, 80.0], 1.0, &tpl);
        let s = doc.map.mob_areas.last().unwrap();
        assert_eq!((s.min[0], s.min[2], s.max[0], s.max[2]), (75.0, 77.0, 85.0, 83.0));
        assert_eq!(s.mobs.len(), 1);
        // a read-only document refuses the drawing and the stamp
        let mut ro = doc_with(&t);
        ro.read_only = true;
        let before = ro.map.clone();
        end_box_draw(&mut ro, start, Some(&r1), None, &g, 1.0, DrawTarget::MobArea, None);
        view2d::stamp_template(&mut ro, &|x, z| gr.at(x, z), [80.0, 80.0], 1.0, &tpl);
        assert_eq!(ro.map, before);
        assert!(ro.undo.is_empty());
    }

    #[test]
    fn hover_outline_3d() {
        let t = terrain();
        let doc = doc_with(&t);
        let layers = Layers::default();
        let look = Lookups::none();
        let g = |x: f32, z: f32| t.height_at(x, z);
        let build = |doc: &Doc, hover: Option<Hit3>, hover_rec: Option<Sel>| build_overlay(&OverlayIn { doc, layers: &layers, look: &look, ground: &g, eye: [128.0, 120.0, 20.0], hover, cursor: None, map_id: Some(1), hover_rec, box_preview: None, measure: &[] });
        let white = |o: &OverlayGeo| o.lines_top.iter().filter(|v| v.col == WHITE).count();
        assert_eq!(white(&build(&doc, None, None)), 0, "no hover, no outline");
        // a row of the outliner or Problems list outlines its record in white
        let row = build(&doc, None, Some((SectionId::MobAreas, 0)));
        assert_eq!(white(&row), 24, "the 12 edges of the box, 2 vertices each");
        // the record under the cursor does the same (as before)
        let hit = Hit3 { sec: SectionId::MobAreas, idx: 0, sub: None, t: 1.0 };
        assert_eq!(white(&build(&doc, Some(hit), None)), 24);
        // both on one record outline it once
        assert_eq!(white(&build(&doc, Some(hit), Some((SectionId::MobAreas, 0)))), 24);
        // a selected record is yellow, not white
        let mut sel = doc_with(&t);
        sel.select_only(SectionId::MobAreas, 0);
        assert_eq!(white(&build(&sel, None, Some((SectionId::MobAreas, 0)))), 0);
        // a row that points nowhere draws nothing
        assert_eq!(white(&build(&doc, None, Some((SectionId::MobAreas, 99)))), 0);
    }

    #[test]
    fn context_menu_3d_vs_pan() {
        // a short right click without a move opens the menu; a drag or a slow press is a pan
        let p0 = Pos2::new(300.0, 200.0);
        assert!(view2d::is_menu_click(p0, 1.0, p0 + Vec2::new(3.0, 0.0), 1.2));
        assert!(!view2d::is_menu_click(p0, 1.0, p0 + Vec2::new(30.0, 0.0), 1.2), "30 px is a pan");
        assert!(!view2d::is_menu_click(p0, 1.0, p0, 1.6), "600 ms is not a click");
        let t = terrain();
        let h = |x: f32, z: f32| Some(t.height_at(x, z));
        let g = Ground3 { size: 256.0, h: &h };
        let mut doc = doc_with(&t);
        let cam = cam_above(100.0, 95.0);
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        let aspect = rect.width() / rect.height();
        let vp = cam.view_proj(aspect);
        // on the NPC's pin: its record, selected, and the ground point under it
        let pin = doc.map.npcs[0].points[0].pos;
        let at = to_screen(&vp, rect, [pin[0], pin[1] + 2.0, pin[2]]).unwrap();
        let ray = cam.ray(ndc_of(rect, at), aspect);
        let m = menu_at(&mut doc, &Layers::default(), &Locks::default(), &ray, &g, at).expect("a menu");
        let hit = m.hit.expect("on a record");
        assert_eq!((hit.sec, hit.idx), (SectionId::Npcs, 0));
        assert_eq!(doc.selection, vec![(SectionId::Npcs, 0)]);
        assert_eq!(m.pos, at);
        // on empty ground: no record, the ground point is the world position
        doc.clear_selection();
        let empty = to_screen(&vp, rect, [200.0, t.height_at(200.0, 40.0), 40.0]).unwrap();
        let ray2 = cam.ray(ndc_of(rect, empty), aspect);
        let m2 = menu_at(&mut doc, &Layers::default(), &Locks::default(), &ray2, &g, empty).expect("a menu on the ground");
        assert!(m2.hit.is_none());
        assert!((m2.world[0] - 200.0).abs() < 1.5 && (m2.world[1] - 40.0).abs() < 1.5, "{:?}", m2.world);
        assert!(doc.selection.is_empty(), "the ground selects nothing");
        // a ray into the sky and onto nothing opens nothing
        let sky = Ray { origin: [0.0, 500.0, 0.0], dir: [0.0, 1.0, 0.0] };
        assert!(menu_at(&mut doc, &Layers::default(), &Locks::default(), &sky, &g, at).is_none());
    }

    #[test]
    fn selection_size_label() {
        let t = terrain();
        let mut doc = doc_with(&t);
        assert!(selection_size(&doc).is_none());
        doc.map.mob_areas[0] = MobArea { min: [150.0, 10.0, 150.0], max: [162.0, 12.0, 158.0], mobs: Vec::new() };
        doc.select_only(SectionId::MobAreas, 0);
        let (text, top) = selection_size(&doc).unwrap();
        assert_eq!(text, "12.0 x 8.0 x 2.0", "x, z and y extents");
        assert_eq!(top, [156.0, 12.0, 154.0]);
        // an NPC alone has no size; with the area the area's box counts
        doc.select_only(SectionId::Npcs, 0);
        assert!(selection_size(&doc).is_none());
        doc.selection = vec![(SectionId::Npcs, 0), (SectionId::MobAreas, 0)];
        assert_eq!(selection_size(&doc).unwrap().0, "12.0 x 8.0 x 2.0");
    }

    #[test]
    fn measuring_in_3d_snaps_and_labels_follow_the_ground() {
        let t = terrain();
        let doc = doc_with(&t);
        // 1 unit per pixel: the portal (60, 60) is an anchor within 8 px of (63, 62)
        assert_eq!(measure_point_3d(&doc.map, [63.0, 62.0], None, false, false, 1.0), [60.0, 60.0]);
        assert_eq!(measure_point_3d(&doc.map, [63.0, 62.0], None, false, true, 1.0), [63.0, 62.0], "Ctrl turns snapping off");
        // far from anchors Shift snaps the bearing to 15 degrees
        let p = measure_point_3d(&doc.map, [10.0, 220.0], Some([10.0, 200.0]), true, false, 1.0);
        assert!((p[0] - 10.0).abs() < 1e-3 && (p[1] - 220.0).abs() < 1e-3);
        // the summary includes the ground height difference
        let mut m = crate::measure::Measure::default();
        m.click([10.0, 10.0]);
        m.click([200.0, 10.0]);
        let s = m.summary(&|x, z| t.height_at(x, z)).unwrap();
        let dy = t.height_at(200.0, 10.0) - t.height_at(10.0, 10.0);
        assert!(s.contains(&format!("dy {dy:.1}")), "{s} vs {dy}");
    }
}
