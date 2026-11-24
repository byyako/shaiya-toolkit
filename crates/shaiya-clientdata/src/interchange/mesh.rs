//! The neutral model between the game formats and a writer: meshes, a rig and clips, all already in glTF space (the mirror of
//! [`super::xform`] applied, triangles swapped). Building it is where every refusal happens; a writer only lays bytes out.

use super::xform::{decompose, decompose_lossy, make_continuous, normalized, s_matrix, s_point, s_quat, s_tri, NotTrs, Trs};
use super::{GltfOptions, Refusal, RefusalKind};
use crate::anim::{locals, mat_inverse, mat_mul, Ani, Mat4};
use crate::mesh3dc::{C3dc, C3dcLayout, C3dcMesh};
use crate::mesh3de::Mesh3de;
use crate::mesh3do::Mesh3do;

/// One mesh in glTF space.
#[derive(Clone, Debug)]
pub struct Mesh3d {
    pub name: String,
    /// The texture name as stored (raw bytes, NUL tail included).
    pub texture: Vec<u8>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    pub skin: Option<SkinData>,
}

/// Two joints and weights per vertex (the second slot is `(0, 0)` when both bones of the file are the same), the `pad` field, and the
/// inverse bind matrices (the file's matrices through the mirror, one per bone of the mesh).
#[derive(Clone, Debug)]
pub struct SkinData {
    pub joints: Vec<[u8; 2]>,
    pub weights: Vec<[f32; 2]>,
    pub pad: Vec<u16>,
    pub ibm: Vec<Mat4>,
}

/// The skeleton: parents and the rest placement of every bone, in glTF space.
#[derive(Clone, Debug)]
pub struct Rig {
    pub parents: Vec<i32>,
    pub rest: Vec<Trs>,
}

/// The keys of one bone of one clip. A track the game file does not have is one key at `start_key` holding the value the game takes from the
/// reference matrix.
#[derive(Clone, Debug)]
pub struct BoneTracks {
    pub rot: Vec<(i32, [f32; 4])>,
    pub trans: Vec<(i32, [f32; 3])>,
    /// Written (as one constant key) only when the bone's placement carries a scale.
    pub scale: Option<[f32; 3]>,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub start_key: i32,
    pub end_key: i32,
    pub first_field: i32,
    pub bones: Vec<BoneTracks>,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub layout: &'static str,
    pub meshes: Vec<Mesh3d>,
    pub rig: Option<Rig>,
    pub clips: Vec<Clip>,
    pub notes: Vec<String>,
}

/// Refusals found while building; with `--allow-lossy` the ones that can be projected are counted instead.
pub(crate) struct Collect {
    pub lossy: bool,
    pub refused: Vec<Refusal>,
    pub more: usize,
    pub overridden: Vec<Refusal>,
    pub projected: usize,
}

const LIST_CAP: usize = 200;

impl Collect {
    pub fn new(lossy: bool) -> Collect {
        Collect { lossy, refused: Vec::new(), more: 0, overridden: Vec::new(), projected: 0 }
    }

    /// Records a problem. Returns true when it is projected (`--allow-lossy` and `can_project`), false when it refuses the file.
    pub fn hit(&mut self, kind: RefusalKind, what: String, reason: String, can_project: bool) -> bool {
        let r = Refusal { kind, what, reason };
        if self.lossy && can_project {
            self.projected += 1;
            if self.overridden.len() < LIST_CAP {
                self.overridden.push(r);
            }
            true
        } else {
            if self.refused.len() < LIST_CAP {
                self.refused.push(r);
            } else {
                self.more += 1;
            }
            false
        }
    }

    pub fn failed(&self) -> bool {
        !self.refused.is_empty()
    }
}

fn not_trs_kind(e: NotTrs) -> RefusalKind {
    match e {
        NotTrs::NotFinite => RefusalKind::NotFinite,
        NotTrs::NotAffine => RefusalKind::NotAffine,
        NotTrs::Singular => RefusalKind::Singular,
        NotTrs::Shear => RefusalKind::Shear,
        NotTrs::Reflection => RefusalKind::Reflection,
    }
}

/// The scale, rotation and translation of `m` in the game's space, or the refusal (projected with `--allow-lossy`).
fn trs_of(c: &mut Collect, m: &Mat4, what: String, from: &str) -> Option<Trs> {
    match decompose(m) {
        Ok(t) => Some(t),
        Err(e) => c.hit(not_trs_kind(e), what, format!("{from}: {}", e.reason()), true).then(|| decompose_lossy(m)),
    }
}

fn first_bad(v: &[f32]) -> Option<usize> {
    v.iter().position(|c| !c.is_finite())
}

const AXES: [char; 3] = ['x', 'y', 'z'];

/// Checks the finite floats of a vertex; the first bad one is a refusal naming the vertex and the component.
fn check_vertex(c: &mut Collect, mesh: &str, i: usize, pos: &[f32; 3], normal: Option<&[f32; 3]>, uv: &[f32; 2]) -> bool {
    for (label, v, names) in [("position", &pos[..], ['x', 'y', 'z']), ("texture coordinate", &uv[..], ['u', 'v', ' '])] {
        if let Some(k) = first_bad(v) {
            c.hit(RefusalKind::NotFinite, format!("{mesh}, vertex {i}"), format!("vertex {i} holds NaN or an infinity in its {label} {}", names[k]), false);
            return false;
        }
    }
    if let Some(n) = normal {
        if let Some(k) = first_bad(n) {
            c.hit(RefusalKind::NotFinite, format!("{mesh}, vertex {i}"), format!("vertex {i} holds NaN or an infinity in its normal {}", AXES[k]), false);
            return false;
        }
    }
    true
}

fn check_faces(c: &mut Collect, mesh: &str, faces: &[[u16; 3]], vertices: usize) -> bool {
    if vertices == 0 || faces.is_empty() {
        c.hit(RefusalKind::Empty, mesh.to_string(), "the mesh has no vertices or no faces".to_string(), false);
        return false;
    }
    if let Some((k, f)) = faces.iter().enumerate().find(|(_, f)| f.iter().any(|i| *i as usize >= vertices)) {
        c.hit(RefusalKind::Index, format!("{mesh}, face {k}"), format!("face {k} names vertex {} but the mesh has {vertices}", f.iter().max().copied().unwrap_or(0)), false);
        return false;
    }
    true
}

fn triangles(faces: &[[u16; 3]]) -> Vec<[u16; 3]> {
    faces.iter().map(|f| s_tri(*f)).collect()
}

fn from_3do(m: &Mesh3do, c: &mut Collect, name: &str) -> Scene {
    let mut ok = check_faces(c, name, &m.faces, m.vertices.len());
    for (i, v) in m.vertices.iter().enumerate() {
        ok &= check_vertex(c, name, i, &v.pos, Some(&v.normal), &v.uv);
        if !ok {
            break;
        }
    }
    let mut notes = Vec::new();
    if !m.frames.is_empty() {
        notes.push(format!("the {} vertex frames of the .3DO are not written (the mesh is the rest shape)", m.frames.len()));
    }
    let mesh = Mesh3d {
        name: name.to_string(),
        texture: m.texture.clone(),
        positions: m.vertices.iter().map(|v| s_point(v.pos)).collect(),
        normals: Some(m.vertices.iter().map(|v| s_point(v.normal)).collect()),
        uvs: m.vertices.iter().map(|v| v.uv).collect(),
        triangles: triangles(&m.faces),
        skin: None,
    };
    Scene { layout: "3do", meshes: vec![mesh], rig: None, clips: Vec::new(), notes }
}

fn from_3de(m: &Mesh3de, c: &mut Collect, name: &str) -> Scene {
    let mut ok = check_faces(c, name, &m.faces, m.vertices.len());
    for (i, v) in m.vertices.iter().enumerate() {
        ok &= check_vertex(c, name, i, &v.pos, None, &v.uv);
        if !ok {
            break;
        }
    }
    let mut notes = Vec::new();
    if m.frame_count() > 0 {
        notes.push(format!("the {} frames of the .3DE are not written (the mesh is the rest shape)", m.frame_count()));
    }
    let mesh = Mesh3d {
        name: name.to_string(),
        texture: m.texture.clone(),
        positions: m.vertices.iter().map(|v| s_point(v.pos)).collect(),
        normals: None,
        uvs: m.vertices.iter().map(|v| v.uv).collect(),
        triangles: triangles(&m.faces),
        skin: None,
    };
    Scene { layout: "3de", meshes: vec![mesh], rig: None, clips: Vec::new(), notes }
}

/// One `.3DC` mesh: the vertices, the faces and (for a skinned layout) the skin with `bones` joints.
fn c3dc_mesh(m: &C3dcMesh, c: &mut Collect, name: &str, skinned: bool, limit: usize) -> Mesh3d {
    let mut ok = check_faces(c, name, &m.faces, m.vertices.len());
    // the joints are the mesh's bones the skeleton also has (what the game's skinning poses)
    let nb = m.bones.len().min(limit);
    let mut skin = skinned.then(|| SkinData { joints: Vec::new(), weights: Vec::new(), pad: Vec::new(), ibm: Vec::new() });
    let mut bad = 0usize;
    for (i, v) in m.vertices.iter().enumerate() {
        if bad >= 10 {
            break;
        }
        let good = check_vertex(c, name, i, &v.pos, Some(&v.normal), &v.uv);
        if !good {
            bad += 1;
            continue;
        }
        if let Some(sk) = skin.as_mut() {
            let (b1, b2) = (v.bone1, v.bone2);
            if !(v.weight >= 0.0 && v.weight <= 1.0) {
                c.hit(RefusalKind::Weight, format!("{name}, vertex {i}"), format!("vertex {i} has the weight {} (outside 0 to 1)", v.weight), false);
                bad += 1;
                continue;
            }
            if usize::from(b1) >= nb || usize::from(b2) >= nb {
                c.hit(RefusalKind::JointRange, format!("{name}, vertex {i}"), format!("vertex {i} names bone {} but the mesh and the skeleton share only {nb} bones", b1.max(b2)), false);
                bad += 1;
                continue;
            }
            // a joint is listed only when it carries weight (the validator warns about a used joint with weight 0); unused slots are joint 0
            if b1 == b2 || v.weight == 1.0 {
                sk.joints.push([b1, 0]);
                sk.weights.push([1.0, 0.0]);
            } else if v.weight == 0.0 {
                sk.joints.push([b2, 0]);
                sk.weights.push([1.0, 0.0]);
            } else {
                sk.joints.push([b1, b2]);
                sk.weights.push([v.weight, 1.0 - v.weight]);
            }
            sk.pad.push(v.pad);
        }
    }
    ok &= bad == 0;
    if let Some(sk) = skin.as_mut() {
        for (i, b) in m.bones.iter().take(nb).enumerate() {
            if let Some(k) = first_bad(b) {
                c.hit(RefusalKind::NotFinite, format!("{name}, bone matrix {i}"), format!("the matrix of bone {i} holds NaN or an infinity (element {k})"), false);
                ok = false;
            }
            sk.ibm.push(s_matrix(b));
        }
    }
    if !ok {
        skin = None;
    }
    Mesh3d {
        name: name.to_string(),
        texture: m.texture.clone(),
        positions: m.vertices.iter().map(|v| s_point(v.pos)).collect(),
        normals: Some(m.vertices.iter().map(|v| s_point(v.normal)).collect()),
        uvs: m.vertices.iter().map(|v| v.uv).collect(),
        triangles: triangles(&m.faces),
        skin,
    }
}

/// The key range of an animation as the exporter times it: `start = min(first_field, smallest key)`.
pub fn start_key_of(a: &Ani) -> i32 {
    let smallest = a.bones.iter().flat_map(|b| b.rot.iter().map(|k| k.key).chain(b.trans.iter().map(|k| k.key))).min();
    smallest.map_or(a.first_field, |k| k.min(a.first_field))
}

/// Keeps the keys that are strictly increasing; a key that is not is a refusal (dropped with `--allow-lossy`).
fn keep_increasing(c: &mut Collect, what: &str, keys: &[i32]) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::with_capacity(keys.len());
    for (k, key) in keys.iter().enumerate() {
        if let Some(&last) = kept.last() {
            if *key <= keys[last] {
                if !c.hit(RefusalKind::KeysNotIncreasing, what.to_string(), format!("key {k} ({key}) is not after key {} ({})", k - 1, keys[last]), true) {
                    return Vec::new();
                }
                continue;
            }
        }
        kept.push(k);
    }
    kept
}

fn clip_of(c: &mut Collect, ani: &Ani, name: &str, locs: &[Mat4], opts: &GltfOptions) -> Clip {
    let start = start_key_of(ani);
    let mut bones = Vec::with_capacity(ani.bones.len());
    let pad = |end: i32, last: i32| opts.pad_end && end > last;
    for (i, b) in ani.bones.iter().enumerate() {
        let what = format!("{name}, bone {i}");
        let mut scale = None;
        let mut rot: Vec<(i32, [f32; 4])> = Vec::new();
        if b.rot.is_empty() {
            let mut m = locs[i];
            if !b.trans.is_empty() {
                m[12..15].copy_from_slice(&[0.0; 3]);
            }
            if let Some(t) = trs_of(c, &m, what.clone(), "the placement from its reference matrix") {
                if t.has_scale() {
                    scale = Some(t.s);
                }
                rot.push((start, s_quat(t.q)));
            }
        } else {
            let keys: Vec<i32> = b.rot.iter().map(|k| k.key).collect();
            let kept = keep_increasing(c, &format!("{what}, rotation"), &keys);
            let mut qs = Vec::with_capacity(kept.len());
            let mut ok = true;
            for &k in &kept {
                match normalized(b.rot[k].q) {
                    Some(q) => qs.push(s_quat(q)),
                    None => {
                        c.hit(RefusalKind::NotFinite, format!("{what}, rotation key {k}"), "the quaternion is not finite or has length 0".to_string(), false);
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                make_continuous(&mut qs);
                rot = kept.iter().zip(qs).map(|(&k, q)| (b.rot[k].key, q)).collect();
            }
        }
        let mut trans: Vec<(i32, [f32; 3])> = Vec::new();
        if b.trans.is_empty() {
            let m = locs[i];
            let v = [m[12], m[13], m[14]];
            if first_bad(&v).is_some() {
                c.hit(RefusalKind::NotFinite, what.clone(), "the translation of its reference matrix holds NaN or an infinity".to_string(), false);
            } else {
                trans.push((start, s_point(v)));
            }
        } else {
            let keys: Vec<i32> = b.trans.iter().map(|k| k.key).collect();
            let kept = keep_increasing(c, &format!("{what}, translation"), &keys);
            for &k in &kept {
                if first_bad(&b.trans[k].v).is_some() {
                    c.hit(RefusalKind::NotFinite, format!("{what}, translation key {k}"), "the translation holds NaN or an infinity".to_string(), false);
                    trans.clear();
                    break;
                }
                trans.push((b.trans[k].key, s_point(b.trans[k].v)));
            }
        }
        if let Some(&(last, q)) = rot.last() {
            if pad(ani.end_key, last) {
                rot.push((ani.end_key, q));
            }
        }
        if let Some(&(last, v)) = trans.last() {
            if pad(ani.end_key, last) {
                trans.push((ani.end_key, v));
            }
        }
        bones.push(BoneTracks { rot, trans, scale });
    }
    Clip { name: name.to_string(), start_key: start, end_key: ani.end_key, first_field: ani.first_field, bones }
}

/// A bone whose rest placement carries a scale but whose animated placement does not (the game replaces the 3 x 3 of a bone with a
/// rotation track) gets a constant scale of 1, so the clip really plays the game's pose.
fn settle_scales(rig: &Rig, clips: &mut [Clip]) {
    for c in clips {
        for (i, b) in c.bones.iter_mut().enumerate() {
            if b.scale.is_none() && rig.rest.get(i).is_some_and(Trs::has_scale) {
                b.scale = Some([1.0; 3]);
            }
        }
    }
}

fn skeleton_text(a: &Ani) -> String {
    format!("{} bones", a.bones.len())
}

/// The clips of `anims` after checking they share one skeleton (bone count and parents).
fn clips_of(c: &mut Collect, anims: &[Ani], opts: &GltfOptions) -> Vec<Clip> {
    let mut clips = Vec::new();
    for (k, a) in anims.iter().enumerate() {
        if let Some(first) = anims.first() {
            let same = a.bones.len() == first.bones.len() && a.bones.iter().zip(&first.bones).all(|(x, y)| x.parent == y.parent);
            if !same {
                c.hit(RefusalKind::Skeleton, format!("animation {k}"), format!("it has {} and a different bone tree than the first animation ({})", skeleton_text(a), skeleton_text(first)), false);
                continue;
            }
        }
        let name = opts.anim_names.get(k).cloned().unwrap_or_else(|| format!("Animation_{k:03}"));
        clips.push(clip_of(c, a, &name, &locals(a), opts));
    }
    clips
}

fn parents_of(a: &Ani) -> Vec<i32> {
    a.bones.iter().enumerate().map(|(i, b)| if b.parent >= 0 && (b.parent as usize) < i { b.parent } else { -1 }).collect()
}

/// A `.3DC` with the animations that move it.
pub(crate) fn scene_of_c3dc(c3dc: &C3dc, anims: &[Ani], c: &mut Collect, opts: &GltfOptions) -> Scene {
    let skinned = c3dc.layout != C3dcLayout::Cloak;
    let layout = match c3dc.layout {
        C3dcLayout::Skinned => "skinned",
        C3dcLayout::SkinnedWithSecondMesh => "second",
        C3dcLayout::Cloak => "cloak",
    };
    let mut notes = Vec::new();
    if !skinned && !anims.is_empty() {
        c.hit(RefusalKind::CloakAnimation, "animation".to_string(), "a cloak has no bones; how the game moves it was never checked, so no animation is attached".to_string(), false);
    }
    let stem = if opts.source.is_empty() { "mesh" } else { opts.source.rsplit('/').next().unwrap_or(&opts.source) };
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s).to_string();
    let nodes = if let Some(first) = anims.first() { first.bones.len() } else { c3dc.meshes.iter().map(|m| m.bones.len()).max().unwrap_or(0) };
    let meshes: Vec<Mesh3d> = c3dc
        .meshes
        .iter()
        .enumerate()
        .map(|(i, m)| c3dc_mesh(m, c, &if c3dc.meshes.len() > 1 { format!("{stem}_{i}") } else { stem.clone() }, skinned, nodes))
        .collect();
    let mut rig = None;
    let mut clips = Vec::new();
    if skinned {
        if nodes == 0 {
            c.hit(RefusalKind::Empty, "skeleton".to_string(), "the mesh or the animation has no bones".to_string(), false);
        }
        let parents = match anims.first() {
            Some(a) => parents_of(a),
            None => vec![-1; nodes],
        };
        // rest = the inverse of the mesh's own matrices: node world = inv(B[i]); local = inv(B[i]) * B[parent]. A bone no vertex uses (and that
        // is no ancestor of one) keeps its rest placement when it splits, else the identity: its matrix does not move any vertex
        let mut needed = vec![false; nodes];
        for m in &c3dc.meshes {
            for v in &m.vertices {
                for b in [v.bone1, v.bone2] {
                    if let Some(n) = needed.get_mut(usize::from(b)) {
                        *n = true;
                    }
                }
            }
        }
        for i in (0..nodes).rev() {
            if let (true, Ok(p)) = (needed[i], usize::try_from(parents[i])) {
                needed[p] = true;
            }
        }
        let mut skipped = 0usize;
        let mut rest = Vec::with_capacity(nodes);
        for i in 0..nodes {
            let Some(m) = c3dc.meshes.iter().find(|m| m.bones.len() > i) else {
                rest.push(Trs::IDENTITY);
                continue;
            };
            let inv = mat_inverse(&m.bones[i]);
            let local = match usize::try_from(parents[i]).ok().and_then(|p| m.bones.get(p)) {
                Some(bp) => mat_mul(&inv, bp),
                None => inv,
            };
            if needed[i] {
                match trs_of(c, &local, format!("bone {i}"), "the rest placement from the mesh's matrices") {
                    Some(t) => rest.push(t.mirrored()),
                    None => rest.push(Trs::IDENTITY),
                }
            } else {
                match decompose(&local) {
                    Ok(t) => rest.push(t.mirrored()),
                    Err(_) => {
                        skipped += 1;
                        rest.push(Trs::IDENTITY);
                    }
                }
            }
        }
        if skipped > 0 {
            notes.push(format!("{skipped} bones that no vertex uses have matrices that are not a scale, rotation and move: they sit at the origin at rest"));
        }
        let r = Rig { parents, rest };
        clips = clips_of(c, anims, opts);
        settle_scales(&r, &mut clips);
        rig = Some(r);
        let same_ibm = meshes.len() < 2 || meshes.windows(2).all(|w| match (&w[0].skin, &w[1].skin) {
            (Some(a), Some(b)) => a.ibm.len() == b.ibm.len() && a.ibm.iter().flatten().zip(b.ibm.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()),
            _ => true,
        });
        if !same_ibm {
            notes.push("the two meshes have different inverse bind matrices: each has its own skin, and the second may look bent until an animation plays".to_string());
        }
    } else {
        notes.push("a cloak is written as a static mesh without a skin".to_string());
    }
    Scene { layout, meshes, rig, clips, notes }
}

/// An animation alone: the skeleton at its reference pose and one clip.
pub(crate) fn scene_of_anim(a: &Ani, c: &mut Collect, opts: &GltfOptions) -> Scene {
    let locs = locals(a);
    let mut rest = Vec::with_capacity(a.bones.len());
    for (i, l) in locs.iter().enumerate() {
        match trs_of(c, l, format!("bone {i}"), "the rest placement from its reference matrix") {
            Some(t) => rest.push(t.mirrored()),
            None => rest.push(Trs::IDENTITY),
        }
    }
    let name = opts.anim_names.first().cloned().unwrap_or_else(|| {
        let stem = opts.source.rsplit('/').next().unwrap_or(&opts.source);
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
        if stem.is_empty() { "Animation_000".to_string() } else { stem.to_string() }
    });
    let rig = Rig { parents: parents_of(a), rest };
    let mut clips = vec![clip_of(c, a, &name, &locs, opts)];
    settle_scales(&rig, &mut clips);
    Scene { layout: "ani", meshes: Vec::new(), rig: Some(rig), clips, notes: Vec::new() }
}

pub(crate) fn scene_of_3do(m: &Mesh3do, c: &mut Collect, opts: &GltfOptions) -> Scene {
    from_3do(m, c, &mesh_name(opts))
}

pub(crate) fn scene_of_3de(m: &Mesh3de, c: &mut Collect, opts: &GltfOptions) -> Scene {
    from_3de(m, c, &mesh_name(opts))
}

fn mesh_name(opts: &GltfOptions) -> String {
    let stem = opts.source.rsplit('/').next().unwrap_or(&opts.source);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    if stem.is_empty() { "mesh".to_string() } else { stem.to_string() }
}
