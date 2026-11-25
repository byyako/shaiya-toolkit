//! BVH (Biovision hierarchy) as an animation-only reference: an `.ANI` goes out, and a BVH laid out as the toolkit writes it comes back.
//!
//! The export writes one joint per bone (`Bone_NNN`, a synthetic `Shaiya_Root` above them when the file has two or more roots), the
//! reference translation of a bone as its `OFFSET`, the root's position and rotations (order Z, X, Y) and the rotations of every other
//! joint, one frame per whole key from the start key to the end key, `Frame Time: 0.0333333`, in the same mirrored space as the glTF export
//! (`x y -z`, quaternion `(-x, -y, z, w)`). A bone that moves relative to its parent has no BVH form (a BVH holds constant offsets): the
//! export refuses it unless asked to leave its motion out. The import reads only that layout (see [`import_bvh`]); it is minimal on purpose.
//! Everything here is lossy for bones that translate and for the scale a reference matrix may carry.

use super::import::{bone_number, ImportError, ImportReport, MAX_KEYS};
use super::mesh::{not_trs_kind, parents_of, start_key_of, Collect};
use super::xform::{decompose, make_continuous, normalized, s_point, s_quat};
use super::{InterchangeError, RefusalKind};
use crate::anim::{locals, quat_matrix, segment, slerp, Ani, AniBone, RotKey, TransKey};

/// The most bytes of a BVH the import reads.
pub const MAX_BVH_BYTES: usize = 64 << 20;
/// The most frames the export writes.
pub const MAX_EXPORT_FRAMES: i64 = 100_000;
/// The most numbers the export writes (frames times channels).
pub const MAX_EXPORT_VALUES: u64 = 16_000_000;
/// The most joints the import reads.
const MAX_JOINTS: usize = 70_000;
/// The most refusals listed; the rest are counted.
const LIST_CAP: usize = 200;
/// The synthetic root of a skeleton with two or more roots.
pub const SYNTHETIC_ROOT: &str = "Shaiya_Root";
const FRAME_TIME_TEXT: &str = "0.0333333";
const ROOT_CHANNELS: [&str; 6] = ["Xposition", "Yposition", "Zposition", "Zrotation", "Xrotation", "Yrotation"];
const JOINT_CHANNELS: [&str; 3] = ["Zrotation", "Xrotation", "Yrotation"];
/// A translation closer than this to the reference one does not move the bone.
const MOVE_TOL: f64 = 1e-5;
/// Two offsets this close (absolute, plus a relative part) are the same.
const OFFSET_TOL: f64 = 1e-4;

#[derive(Clone, Debug)]
pub struct BvhOptions {
    /// Leave out the motion of a non-root bone that translates (reported) instead of refusing the file.
    pub drop_translation: bool,
    /// Multiplies every offset and position (100 for tools that expect centimetres); the import reads scale 1 only.
    pub scale: f64,
}

impl Default for BvhOptions {
    fn default() -> Self {
        BvhOptions { drop_translation: false, scale: 1.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BvhReport {
    pub bones: usize,
    pub frames: usize,
    /// The key of frame 0.
    pub start_key: i32,
    /// Bones whose translation track was left out (`drop_translation`).
    pub dropped: usize,
    /// The largest distance a dropped track moved a bone from its offset.
    pub max_displacement: f64,
    /// Angle sets close to gimbal lock (X near 90 degrees), where Y follows the frame before.
    pub gimbal_frames: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BvhOutput {
    pub text: Vec<u8>,
    pub report: BvhReport,
}

// ---- angles -------------------------------------------------------------------------------------------------------------------------

type Q = [f64; 4];

fn qmul(a: Q, b: Q) -> Q {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// The quaternion of the BVH rotation `Rz(z) * Rx(x) * Ry(y)` (column vectors, right-handed, radians).
fn quat_of_zxy(z: f64, x: f64, y: f64) -> Q {
    let (qz, qx, qy) = ([0.0, 0.0, (z / 2.0).sin(), (z / 2.0).cos()], [(x / 2.0).sin(), 0.0, 0.0, (x / 2.0).cos()], [0.0, (y / 2.0).sin(), 0.0, (y / 2.0).cos()]);
    qmul(qmul(qz, qx), qy)
}

type M3 = [[f64; 3]; 3];

fn m3mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            o[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

fn rot_x(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// How close to gimbal lock (`cos x`, with `x` the middle angle) the angles are taken from the frame before.
const GIMBAL: f64 = 1e-3;

/// The angles `(z, x, y)` (radians) with `R = Rz(z) * Rx(x) * Ry(y)` for the unit quaternion `q`, the triple nearest to `prev` (every angle
/// shifted by whole turns, the second solution `(z + pi, pi - x, y + pi)` considered). `x` comes from `atan2` (accurate at 90 degrees, where an
/// `asin` is not). `z` is always solved from `R` once `x` and `y` are chosen, so a poorly determined `y` costs nothing: near gimbal lock
/// (`cos x` below [`GIMBAL`], the second value), `y` is the previous frame's when the matrix cannot tell them apart (within the noise of
/// 32-bit numbers, `4e-7 / cos x`), else the one the matrix gives.
fn euler_zxy(q: Q, prev: Option<[f64; 3]>) -> ([f64; 3], bool) {
    let [x, y, z, w] = q;
    let r: M3 = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let turn = std::f64::consts::TAU;
    let pi = std::f64::consts::PI;
    let near = |a: f64, p: f64| a + turn * ((p - a) / turn).round();
    let h = r[2][0].hypot(r[2][2]);
    let ax = r[2][1].atan2(h);
    let mut ay = (-r[2][0]).atan2(r[2][2]);
    let gimbal = h < GIMBAL;
    if gimbal {
        if let Some(p) = prev {
            if (near(ay, p[2]) - p[2]).abs() <= 4e-7 / h.max(1e-12) {
                ay = p[2];
            }
        }
    }
    let solve = |x: f64, y: f64| -> [f64; 3] {
        let m = m3mul(&m3mul(&r, &rot_y(-y)), &rot_x(-x));
        [m[1][0].atan2(m[0][0]), x, y]
    };
    let candidates = if gimbal { vec![solve(ax, ay)] } else { vec![solve(ax, ay), solve(pi - ax, ay + pi)] };
    let base = prev.unwrap_or([0.0; 3]);
    let mut best: Option<([f64; 3], f64)> = None;
    for cand in candidates {
        let t = [near(cand[0], base[0]), near(cand[1], base[1]), near(cand[2], base[2])];
        let d: f64 = (0..3).map(|i| (t[i] - base[i]).abs()).sum();
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((t, d));
        }
    }
    (best.map_or([0.0; 3], |b| b.0), gimbal)
}

fn num(x: f64) -> String {
    let s = format!("{x:.6}");
    if s == "-0.000000" {
        "0.000000".to_string()
    } else {
        s
    }
}

fn indent(depth: usize) -> String {
    " ".repeat(depth.min(32) * 2)
}

// ---- the export ---------------------------------------------------------------------------------------------------------------------

fn sample_rot(keys: &[(i32, [f32; 4])], t: f32) -> [f32; 4] {
    let q = match segment(|k| keys[k].0, keys.len(), t) {
        Err(k) => keys[k].1,
        Ok((k, f)) => slerp(keys[k].1, keys[k + 1].1, f),
    };
    normalized(q).unwrap_or(keys[0].1)
}

fn sample_trans(keys: &[(i32, [f32; 3])], t: f32) -> [f32; 3] {
    match segment(|k| keys[k].0, keys.len(), t) {
        Err(k) => keys[k].1,
        Ok((k, f)) => {
            let (a, c) = (keys[k].1, keys[k + 1].1);
            [a[0] + (c[0] - a[0]) * f, a[1] + (c[1] - a[1]) * f, a[2] + (c[2] - a[2]) * f]
        }
    }
}

/// Writes `ani` as BVH text. A bone that translates relative to its parent, a reference matrix that is not a placement, a key that is not
/// finite or more frames than [`MAX_EXPORT_FRAMES`] are refused (with the bone) and nothing is returned.
pub fn export_bvh(ani: &Ani, opts: &BvhOptions) -> Result<BvhOutput, InterchangeError> {
    let refuse = |kind, what: &str, reason: String| Err(InterchangeError::Refused { format: "BVH", list: vec![super::Refusal { kind, what: what.to_string(), reason }], more: 0 });
    if !(opts.scale.is_finite() && opts.scale > 0.0 && opts.scale <= 1e6) {
        return refuse(RefusalKind::Limit, "scale", "the scale must be a number above 0 and at most 1000000".to_string());
    }
    let n = ani.bones.len();
    if n == 0 {
        return refuse(RefusalKind::Empty, "skeleton", "the animation has no bones".to_string());
    }
    let parents = parents_of(ani);
    let roots: Vec<usize> = (0..n).filter(|&i| parents[i] < 0).collect();
    let synthetic = roots.len() >= 2;
    let start = start_key_of(ani);
    let frames = (i64::from(ani.end_key) - i64::from(start) + 1).max(1);
    let channels = 3 * n as u64 + if synthetic { 6 } else { 3 };
    if frames > MAX_EXPORT_FRAMES || (frames as u64).saturating_mul(channels) > MAX_EXPORT_VALUES {
        return refuse(RefusalKind::Limit, "keys", format!("{frames} frames of {channels} numbers: a BVH of at most {MAX_EXPORT_FRAMES} frames and {MAX_EXPORT_VALUES} numbers is written"));
    }
    let locs = locals(ani);
    let mut c = Collect::new(false);
    let mut report = BvhReport { bones: n, frames: frames as usize, start_key: start, ..BvhReport::default() };
    let mut rots: Vec<Vec<(i32, [f32; 4])>> = Vec::with_capacity(n);
    let mut transs: Vec<Vec<(i32, [f32; 3])>> = Vec::with_capacity(n);
    let mut still: Vec<[f32; 4]> = Vec::with_capacity(n);
    let mut scaled = 0usize;
    for (i, b) in ani.bones.iter().enumerate() {
        let what = format!("bone {i}");
        let l = &locs[i];
        if l[12..15].iter().any(|x| !x.is_finite()) {
            c.hit(RefusalKind::NotFinite, what.clone(), "the translation of its reference matrix holds NaN or an infinity".to_string(), false);
        }
        let mut rk = Vec::with_capacity(b.rot.len());
        for (k, key) in b.rot.iter().enumerate() {
            match normalized(key.q) {
                Some(q) => rk.push((key.key, q)),
                None => {
                    c.hit(RefusalKind::NotFinite, format!("{what}, rotation key {k}"), "the quaternion is not finite or has length 0".to_string(), false);
                    break;
                }
            }
        }
        let mut tk = Vec::with_capacity(b.trans.len());
        for (k, key) in b.trans.iter().enumerate() {
            if key.v.iter().any(|x| !x.is_finite()) {
                c.hit(RefusalKind::NotFinite, format!("{what}, translation key {k}"), "the translation holds NaN or an infinity".to_string(), false);
                break;
            }
            tk.push((key.key, key.v));
        }
        // the rotation of a bone with no rotation track is its reference matrix's
        let mut q = [0.0, 0.0, 0.0, 1.0];
        if b.rot.is_empty() {
            match decompose(l) {
                Ok(t) => {
                    if t.has_scale() {
                        scaled += 1;
                    }
                    q = t.q;
                }
                Err(e) => {
                    c.hit(not_trs_kind(e), what.clone(), format!("the rotation from its reference matrix: {}", e.reason()), false);
                }
            }
        }
        // a bone below another one holds its offset constant: a translation track that moves it has no BVH form
        let child = parents[i] >= 0 || synthetic;
        if child && !tk.is_empty() && l[12..15].iter().all(|x| x.is_finite()) {
            let disp = tk.iter().map(|(_, v)| (0..3).map(|a| f64::from(v[a] - l[12 + a]).abs()).fold(0.0, f64::max)).fold(0.0, f64::max);
            if disp > MOVE_TOL {
                if opts.drop_translation {
                    report.dropped += 1;
                    report.max_displacement = report.max_displacement.max(disp);
                } else {
                    c.hit(
                        RefusalKind::Unsupported,
                        what,
                        format!("moves relative to its parent (its translation track leaves the offset by up to {disp:.4}); a BVH keeps only constant offsets (--drop-translation leaves the motion out)"),
                        false,
                    );
                }
            }
        }
        rots.push(rk);
        transs.push(tk);
        still.push(q);
    }
    if c.failed() {
        return Err(InterchangeError::Refused { format: "BVH", list: c.refused, more: c.more });
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, &p) in parents.iter().enumerate() {
        if let Ok(p) = usize::try_from(p) {
            children[p].push(i);
        }
    }
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(b) = stack.pop() {
        order.push(b);
        stack.extend(children[b].iter().rev());
    }
    // the hierarchy
    let scale = opts.scale;
    let off = |i: usize| {
        let p = s_point([locs[i][12], locs[i][13], locs[i][14]]);
        [f64::from(p[0]) * scale, f64::from(p[1]) * scale, f64::from(p[2]) * scale]
    };
    let name = |i: usize| format!("Bone_{i:03}");
    let mut text = String::new();
    text.push_str("HIERARCHY\n");
    enum Step {
        Open(usize, usize),
        Close(usize),
    }
    let mut steps: Vec<Step> = Vec::new();
    let mut depth0 = 0;
    if synthetic {
        text.push_str(&format!("ROOT {SYNTHETIC_ROOT}\n{{\n  OFFSET {} {} {}\n  CHANNELS 6 {}\n", num(0.0), num(0.0), num(0.0), ROOT_CHANNELS.join(" ")));
        depth0 = 1;
    }
    for &r in roots.iter().rev() {
        steps.push(Step::Open(r, depth0));
    }
    while let Some(s) = steps.pop() {
        match s {
            Step::Open(b, d) => {
                let is_root = !synthetic && parents[b] < 0;
                let (pad, o) = (indent(d), off(b));
                text.push_str(&format!("{pad}{} {}\n{pad}{{\n{pad}  OFFSET {} {} {}\n", if is_root { "ROOT" } else { "JOINT" }, name(b), num(o[0]), num(o[1]), num(o[2])));
                let chans: &[&str] = if is_root { &ROOT_CHANNELS } else { &JOINT_CHANNELS };
                text.push_str(&format!("{pad}  CHANNELS {} {}\n", chans.len(), chans.join(" ")));
                steps.push(Step::Close(d));
                if children[b].is_empty() {
                    text.push_str(&format!("{pad}  End Site\n{pad}  {{\n{pad}    OFFSET {} {} {}\n{pad}  }}\n", num(0.0), num(0.0), num(0.0)));
                }
                for &ch in children[b].iter().rev() {
                    steps.push(Step::Open(ch, d + 1));
                }
            }
            Step::Close(d) => text.push_str(&format!("{}}}\n", indent(d))),
        }
    }
    if synthetic {
        text.push_str("}\n");
    }
    text.push_str(&format!("MOTION\nFrames: {frames}\nFrame Time: {FRAME_TIME_TEXT}\n"));
    // the frames
    let mut prev: Vec<Option<[f64; 3]>> = vec![None; n];
    let mut eul: Vec<[f64; 3]> = vec![[0.0; 3]; n];
    for f in 0..frames {
        let key = start + f as i32;
        let t = key as f32;
        let mut pos = [0.0f64; 3];
        for i in 0..n {
            let q = if rots[i].is_empty() { still[i] } else { sample_rot(&rots[i], t) };
            let g = s_quat(q);
            let (angles, gimbal) = euler_zxy([f64::from(g[0]), f64::from(g[1]), f64::from(g[2]), f64::from(g[3])], prev[i]);
            if gimbal {
                report.gimbal_frames += 1;
            }
            prev[i] = Some(angles);
            eul[i] = angles;
            if !synthetic && parents[i] < 0 {
                let v = if transs[i].is_empty() { [locs[i][12], locs[i][13], locs[i][14]] } else { sample_trans(&transs[i], t) };
                let p = s_point(v);
                pos = [f64::from(p[0]) * scale, f64::from(p[1]) * scale, f64::from(p[2]) * scale];
            }
        }
        let mut row: Vec<String> = Vec::with_capacity(channels as usize);
        if synthetic {
            row.extend(std::iter::repeat_n(num(0.0), 6));
        }
        for &b in &order {
            if !synthetic && parents[b] < 0 {
                row.extend(pos.iter().map(|p| num(*p)));
            }
            row.extend(eul[b].iter().map(|a| num(a.to_degrees())));
        }
        text.push_str(&row.join(" "));
        text.push('\n');
    }
    if synthetic {
        report.notes.push(format!("the skeleton has {} roots: a synthetic root {SYNTHETIC_ROOT} holds them (the import drops it)", roots.len()));
    }
    if report.dropped > 0 {
        report.notes.push(format!("{} bones move relative to their parent; their motion was left out (the largest distance from the offset: {:.4})", report.dropped, report.max_displacement));
    }
    if scaled > 0 {
        report.notes.push(format!("{scaled} bones carry a scale in their reference matrix; a BVH cannot hold it, so only the rotation is written"));
    }
    if report.gimbal_frames > 0 {
        report.notes.push(format!("{} angle sets were close to gimbal lock (X near 90 degrees): Y follows the frame before there", report.gimbal_frames));
    }
    if opts.scale != 1.0 {
        report.notes.push(format!("offsets and positions are multiplied by {}; the import reads scale 1 only", opts.scale));
    }
    report.notes.push("BVH is an animation-only reference: it holds no mesh, no bind pose and no scale".to_string());
    Ok(BvhOutput { text: text.into_bytes(), report })
}

// ---- the import ---------------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Refs {
    list: Vec<super::Refusal>,
    more: usize,
}

impl Refs {
    fn add(&mut self, kind: RefusalKind, what: impl Into<String>, reason: impl Into<String>) {
        if self.list.len() < LIST_CAP {
            self.list.push(super::Refusal { kind, what: what.into(), reason: reason.into() });
        } else {
            self.more += 1;
        }
    }

    fn failed(&self) -> bool {
        !self.list.is_empty()
    }

    fn done(self) -> ImportError {
        ImportError { list: self.list, more: self.more }
    }
}

struct Joint {
    name: String,
    parent: Option<usize>,
    offset: [f64; 3],
    channels: Vec<String>,
}

fn short(t: &str) -> String {
    t.chars().take(40).collect()
}

/// Reads the hierarchy: the joints in file order. `Err` is the one reason the text is not a hierarchy this reader takes.
fn read_hierarchy<'a>(it: &mut std::str::SplitWhitespace<'a>) -> Result<Vec<Joint>, (RefusalKind, String)> {
    let bad = |m: String| (RefusalKind::Format, m);
    if it.next() != Some("HIERARCHY") {
        return Err(bad("the file does not start with HIERARCHY".to_string()));
    }
    let mut joints: Vec<Joint> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut roots = 0usize;
    let mut done = false;
    let float = |it: &mut std::str::SplitWhitespace<'a>, what: &str| -> Result<f64, (RefusalKind, String)> {
        let t = it.next().ok_or_else(|| bad(format!("the file ends inside {what}")))?;
        match t.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(x),
            _ => Err(bad(format!("{} is not a number ({what})", short(t)))),
        }
    };
    loop {
        let Some(tok) = it.next() else { return Err(bad("the file ends before MOTION".to_string())) };
        match tok {
            "ROOT" | "JOINT" => {
                if tok == "ROOT" {
                    if roots > 0 || done {
                        return Err((RefusalKind::Unsupported, "more than one ROOT: the toolkit reads a hierarchy with one".to_string()));
                    }
                    roots += 1;
                } else if open.is_empty() {
                    return Err(bad("a JOINT outside any ROOT".to_string()));
                }
                if joints.len() >= MAX_JOINTS {
                    return Err((RefusalKind::Limit, format!("more than {MAX_JOINTS} joints")));
                }
                let name = it.next().ok_or_else(|| bad("the file ends after ROOT or JOINT".to_string()))?;
                if it.next() != Some("{") {
                    return Err(bad(format!("{} is not followed by {{", short(name))));
                }
                joints.push(Joint { name: name.to_string(), parent: open.last().copied(), offset: [0.0; 3], channels: Vec::new() });
                open.push(joints.len() - 1);
            }
            "End" => {
                if open.is_empty() || it.next() != Some("Site") || it.next() != Some("{") || it.next() != Some("OFFSET") {
                    return Err(bad("an End Site is not `End Site { OFFSET x y z }`".to_string()));
                }
                for _ in 0..3 {
                    float(it, "an End Site offset")?;
                }
                if it.next() != Some("}") {
                    return Err(bad("an End Site is not closed".to_string()));
                }
            }
            "OFFSET" => {
                let Some(&j) = open.last() else { return Err(bad("an OFFSET outside any joint".to_string())) };
                let o = [float(it, "an OFFSET")?, float(it, "an OFFSET")?, float(it, "an OFFSET")?];
                joints[j].offset = o;
            }
            "CHANNELS" => {
                let Some(&j) = open.last() else { return Err(bad("CHANNELS outside any joint".to_string())) };
                let n: usize = it.next().and_then(|t| t.parse().ok()).ok_or_else(|| bad("CHANNELS is not followed by a count".to_string()))?;
                if n > 6 {
                    return Err((RefusalKind::Unsupported, format!("{} lists {n} channels; at most 6 are read", short(&joints[j].name))));
                }
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(it.next().ok_or_else(|| bad("the file ends inside CHANNELS".to_string()))?.to_string());
                }
                joints[j].channels = v;
            }
            "}" => {
                if open.pop().is_none() {
                    return Err(bad("a } with no open joint".to_string()));
                }
                if open.is_empty() {
                    done = true;
                }
            }
            "MOTION" => {
                if !open.is_empty() || joints.is_empty() {
                    return Err(bad("MOTION before the hierarchy is complete".to_string()));
                }
                return Ok(joints);
            }
            other => return Err(bad(format!("{} is not part of a hierarchy", short(other)))),
        }
    }
}

/// Reads `bytes` as the BVH the toolkit writes, as a new animation of the skeleton of `like` (the file it will replace). Frame `f` is key
/// `start + f`, where `start` is the like file's start key (the smaller of its first field and its smallest key); the end key is the last
/// frame's. A joint is a bone by its name `Bone_NNN` (any number of digits); a synthetic `Shaiya_Root` is dropped. Refused, with the list:
/// a joint the rig lacks or names twice, a bone with no joint, a joint with another parent than the like file's, a channel order other than
/// the ZXY the export writes (the root: `Xposition Yposition Zposition Zrotation Xrotation Yrotation`), a position channel on a joint that is
/// not the root, a `Frame Time` that is not 1/30 within 1e-6, an offset that differs from the like file's reference translation, more than
/// [`MAX_KEYS`] frames, a number that is not finite, a moved `Shaiya_Root` or one with an offset other than 0. A rotation track equal to what the
/// like file's bone is posed with (its matrix's rotation, or its unit key rotation when a scaled bone has keys) and a translation track
/// equal to the like file's placement within 1e-5 at every frame are left out. Whether the result passes against `like` is the caller's check (`anim::passes`).
pub fn import_bvh(bytes: &[u8], like: &Ani, cancel: Option<&shaiya_core::job::Cancel>) -> Result<(Ani, ImportReport), ImportError> {
    let one = |kind, what: &str, reason: String| Err(ImportError::one(kind, what, reason));
    if bytes.len() > MAX_BVH_BYTES {
        return one(RefusalKind::Limit, "BVH", format!("{} bytes is over the {MAX_BVH_BYTES}-byte limit", bytes.len()));
    }
    let nb = like.bones.len();
    if nb == 0 {
        return one(RefusalKind::Like, "like", "the like animation has no bones".to_string());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    let Ok(text) = std::str::from_utf8(bytes) else { return one(RefusalKind::Format, "BVH", "the file is not text".to_string()) };
    let mut it = text.split_whitespace();
    let joints = match read_hierarchy(&mut it) {
        Ok(j) => j,
        Err((k, m)) => return one(k, "hierarchy", m),
    };
    // the motion header
    let mut refs = Refs::default();
    let frames: usize = {
        let (a, b) = (it.next(), it.next());
        match (a, b.and_then(|t| t.parse::<usize>().ok())) {
            (Some("Frames:"), Some(n)) => n,
            _ => return one(RefusalKind::Format, "MOTION", "`Frames: N` is missing".to_string()),
        }
    };
    let frame_time: f64 = {
        let (a, b, c) = (it.next(), it.next(), it.next());
        match (a, b, c.and_then(|t| t.parse::<f64>().ok())) {
            (Some("Frame"), Some("Time:"), Some(x)) if x.is_finite() => x,
            _ => return one(RefusalKind::Format, "MOTION", "`Frame Time: T` is missing".to_string()),
        }
    };
    if (frame_time - 1.0 / 30.0).abs() > 1e-6 {
        refs.add(RefusalKind::Format, "Frame Time", format!("{frame_time} s is not 1/30 s (0.0333333): the toolkit reads files written with the game's 30 keys per second"));
    }
    if frames == 0 {
        refs.add(RefusalKind::Format, "Frames", "the file has no frames");
    } else if frames > MAX_KEYS {
        refs.add(RefusalKind::Limit, "Frames", format!("{frames} frames; at most {MAX_KEYS} are read"));
    }
    // names: Bone_NNN, the synthetic root dropped
    let mut joint_bone: Vec<Option<usize>> = vec![None; joints.len()];
    let mut seen: Vec<Option<usize>> = vec![None; nb];
    let mut lacking: Vec<String> = Vec::new();
    let mut twice: Vec<String> = Vec::new();
    for (j, jt) in joints.iter().enumerate() {
        if j == 0 && jt.name == SYNTHETIC_ROOT {
            continue;
        }
        match bone_number(&jt.name) {
            Some(b) if b < nb => {
                if let Some(first) = seen[b] {
                    twice.push(format!("{} and {}", short(&joints[first].name), short(&jt.name)));
                } else {
                    seen[b] = Some(j);
                    joint_bone[j] = Some(b);
                }
            }
            _ => lacking.push(short(&jt.name)),
        }
    }
    let list = |v: &[String]| format!("{}{}", v.iter().take(10).cloned().collect::<Vec<_>>().join(", "), if v.len() > 10 { ", ..." } else { "" });
    if !lacking.is_empty() {
        refs.add(RefusalKind::Names, "joints", format!("{} joints are no bone of the like file ({nb} bones, joints are named Bone_NNN): {}", lacking.len(), list(&lacking)));
    }
    if !twice.is_empty() {
        refs.add(RefusalKind::Names, "joints", format!("{} bones are named twice: {}", twice.len(), list(&twice)));
    }
    let missing: Vec<String> = (0..nb).filter(|&b| seen[b].is_none()).map(|b| format!("Bone_{b:03}")).collect();
    if !missing.is_empty() && lacking.is_empty() && twice.is_empty() {
        refs.add(RefusalKind::Names, "joints", format!("{} bones of the like file have no joint: {}", missing.len(), list(&missing)));
    }
    // parents (the synthetic root skipped) against the like file's
    let eff_parent = |j: usize| -> Option<usize> {
        let mut p = joints[j].parent;
        while let Some(x) = p {
            if joint_bone[x].is_some() {
                return joint_bone[x];
            }
            p = joints[x].parent;
        }
        None
    };
    let mut wrong: Vec<String> = Vec::new();
    for (j, jb) in joint_bone.iter().enumerate() {
        let Some(b) = jb else { continue };
        let want = like.bones[*b].parent;
        let want = usize::try_from(want).ok().filter(|&p| p < *b);
        if eff_parent(j) != want {
            wrong.push(format!("bone {b}: parent is {}, the like file's is {}", eff_parent(j).map_or(-1, |p| p as i64), want.map_or(-1, |p| p as i64)));
        }
    }
    if !wrong.is_empty() {
        refs.add(RefusalKind::Names, "hierarchy", format!("{} bones have another parent than in the like file: {}", wrong.len(), list(&wrong)));
    }
    // channels
    let mut cols: Vec<usize> = Vec::with_capacity(joints.len());
    let mut total_cols = 0usize;
    for (j, jt) in joints.iter().enumerate() {
        cols.push(total_cols);
        total_cols += jt.channels.len();
        let root = j == 0;
        let want: &[&str] = if root { &ROOT_CHANNELS } else { &JOINT_CHANNELS };
        if jt.channels.iter().map(String::as_str).eq(want.iter().copied()) {
            continue;
        }
        if !root && jt.channels.iter().any(|c| c.ends_with("position")) {
            refs.add(RefusalKind::Unsupported, short(&jt.name), "has a position channel: only the root moves in a BVH the toolkit reads (a bone that translates relative to its parent cannot be imported)");
        } else {
            refs.add(
                RefusalKind::Unsupported,
                short(&jt.name),
                format!("its channels are `{}`; the toolkit reads only `{}` (the order its export writes)", jt.channels.join(" "), want.join(" ")),
            );
        }
    }
    // offsets against the like file's reference translations (a scale or an edit would otherwise pass unseen)
    let locs = locals(like);
    let mut moved: Vec<String> = Vec::new();
    for (j, jb) in joint_bone.iter().enumerate() {
        let Some(b) = jb else { continue };
        let want = s_point([locs[*b][12], locs[*b][13], locs[*b][14]]);
        let off = joints[j].offset;
        let ok = (0..3).all(|a| {
            let w = f64::from(want[a]);
            (off[a] - w).abs() <= OFFSET_TOL + 1e-5 * w.abs()
        });
        if !ok {
            moved.push(format!("{} ({:.4} {:.4} {:.4}, the like file's {:.4} {:.4} {:.4})", short(&joints[j].name), off[0], off[1], off[2], want[0], want[1], want[2]));
        }
    }
    if !moved.is_empty() {
        refs.add(
            RefusalKind::Transform,
            "offsets",
            format!("{} joints have another offset than the like file's reference translation (a file exported with --scale other than 1 is not read): {}", moved.len(), list(&moved)),
        );
    }
    if refs.failed() {
        return Err(refs.done());
    }
    // the numbers: counted before anything is allocated
    let Some(expected) = frames.checked_mul(total_cols).filter(|n| *n as u64 <= MAX_EXPORT_VALUES) else {
        return one(RefusalKind::Limit, "MOTION", format!("{frames} frames of {total_cols} channels: at most {MAX_EXPORT_VALUES} numbers are read"));
    };
    let present = it.clone().count();
    if present != expected {
        return one(RefusalKind::Format, "MOTION", format!("the file has {present} numbers after Frame Time, {frames} frames of {total_cols} channels need {expected}"));
    }
    let mut values: Vec<f64> = Vec::with_capacity(expected);
    for (n, t) in it.enumerate() {
        if n % 65_536 == 0 && cancel.is_some_and(shaiya_core::job::Cancel::is_cancelled) {
            return one(RefusalKind::Limit, "BVH", "cancelled".to_string());
        }
        match t.parse::<f64>() {
            Ok(x) if x.is_finite() => values.push(x),
            _ => return Err(ImportError::one(RefusalKind::NotFinite, format!("frame {}, value {}", n / total_cols.max(1), n % total_cols.max(1)), format!("{} is not a finite number", short(t)))),
        }
    }
    let start = start_key_of(like);
    let Some(last) = i32::try_from(frames - 1).ok().and_then(|f| start.checked_add(f)) else { return one(RefusalKind::Limit, "Frames", "the key numbers are out of range".to_string()) };
    let at = |f: usize, col: usize| values[f * total_cols + col];
    // a synthetic root that moves would be dropped with its motion
    if joint_bone[0].is_none() {
        if joints[0].offset.iter().any(|v| v.abs() > OFFSET_TOL) {
            return one(RefusalKind::Transform, SYNTHETIC_ROOT, format!("the synthetic root has an offset ({:.4} {:.4} {:.4}); the toolkit drops it, so a file that moved the whole rig cannot be read", joints[0].offset[0], joints[0].offset[1], joints[0].offset[2]));
        }
        let moved = (0..frames).any(|f| (0..6).any(|c| at(f, cols[0] + c).abs() > 1e-6));
        if moved {
            return one(RefusalKind::Transform, SYNTHETIC_ROOT, "the synthetic root moves; the toolkit drops it, so a file that moves it cannot be read".to_string());
        }
    }
    let mut out: Vec<AniBone> = like.bones.iter().map(|b| AniBone { parent: b.parent, matrix: b.matrix, rot: Vec::new(), trans: Vec::new() }).collect();
    let mut report = ImportReport { bones: nb, ..ImportReport::default() };
    for (j, jb) in joint_bone.iter().enumerate() {
        let Some(b) = *jb else { continue };
        let root = j == 0;
        let rcol = cols[j] + if root { 3 } else { 0 };
        let mut qs: Vec<[f32; 4]> = (0..frames)
            .map(|f| {
                let q = quat_of_zxy(at(f, rcol).to_radians(), at(f, rcol + 1).to_radians(), at(f, rcol + 2).to_radians());
                normalized([q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]).unwrap_or([0.0, 0.0, 0.0, 1.0])
            })
            .collect();
        make_continuous(&mut qs);
        let game: Vec<[f32; 4]> = qs.iter().map(|q| s_quat(*q)).collect();
        // the like file's own reference 3x3 leaves the track out
        let l = &locs[b];
        // a bone of the like file with rotation keys is posed with the key's unit rotation (its matrix's scale is replaced), a bone with
        // none is posed with the full matrix: a track is left out only when that same pose comes back, so a scaled matrix with constant
        // keys keeps its track (leaving it out would put the scale back) and a scaled matrix with no keys keeps its matrix
        let rest = match decompose(l) {
            Ok(t) if t.has_scale() && !like.bones[b].rot.is_empty() => None,
            Ok(t) => Some(quat_matrix(t.q)),
            Err(_) => Some(*l),
        };
        let same = rest.is_some_and(|rest| {
            game.iter().all(|q| {
                let m = quat_matrix(*q);
                (0..3).all(|r| (0..3).all(|c| f64::from((m[r * 4 + c] - rest[r * 4 + c]).abs()) <= 1e-5))
            })
        });
        if !same {
            out[b].rot = game.iter().enumerate().map(|(f, q)| RotKey { key: start + f as i32, q: *q }).collect();
        }
        if root {
            let ts: Vec<[f32; 3]> = (0..frames).map(|f| s_point([at(f, cols[j]) as f32, at(f, cols[j] + 1) as f32, at(f, cols[j] + 2) as f32])).collect();
            let still = ts.iter().all(|t| (0..3).all(|a| f64::from((t[a] - l[12 + a]).abs()) <= 1e-5));
            if !still {
                out[b].trans = ts.iter().enumerate().map(|(f, v)| TransKey { key: start + f as i32, v: *v }).collect();
            }
        }
        report.keys += out[b].rot.len() + out[b].trans.len();
    }
    report.notes.push(format!("frame 0 is key {start} (the like file's start key); the end key is {last}"));
    Ok((Ani { first_field: like.first_field, end_key: last, bones: out }, report))
}
