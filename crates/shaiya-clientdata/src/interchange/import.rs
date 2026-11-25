//! glTF 2.0 into the model of a new game file: an `.ANI` (the skeleton, reference matrices and first field come from the file it will
//! replace, `like`), a `.3DC` (a like `.3DC` or the rig's `.ANI`), or a static `.3DO` / `.3DE`. The reading is [`super::gltf_read`]; this
//! module maps a bone node to a bone by name (`Bone_NNN` of any width, or an explicit list), samples the animation at every whole key (or
//! at the file's own keys with `keys_exact`), undoes the one mirror of [`super::xform`], and refuses everything the game formats cannot
//! hold with a reason that names the node, bone or vertex. Every refusal of a file is collected (at most 200 are listed); nothing is
//! written here. The result is only a model: [`finish`] builds its bytes (re-parsed) and the callers run the same rig check as
//! `shaiya anim import` before they write.

use super::gltf_read::{Accessor, Gltf};
use super::xform::{decompose, make_continuous, normalized, s_matrix, s_point, s_quat, s_tri, Trs};
use super::{Refusal, RefusalKind};
use crate::anim::{locals, mat_inverse, quat_matrix, Ani, AniBone, Mat4, RotKey, TransKey};
use crate::mesh3de::{Mesh3de, Vertex3de};
use crate::mesh3dc::{C3dc, C3dcLayout, C3dcMesh, C3dcVertex};
use crate::mesh3do::{Mesh3do, Vertex3do};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;

/// The most refusals listed; the rest are counted.
const LIST_CAP: usize = 200;
/// The most accessor values (elements times components) one import decodes in all: an accessor that many samplers or primitives name is
/// decoded once and counted once.
pub const MAX_IMPORT_VALUES: usize = 1 << 25;
/// The most key times one animation sampler may hold (eight times the keys one animation is sampled at).
pub const MAX_SAMPLER_KEYS: usize = MAX_KEYS * 8;
/// The most whole keys one animation is sampled at.
pub const MAX_KEYS: usize = 20_000;
/// The most vertices of one mesh of a game file (indices are 16 bit).
pub const MAX_VERTICES: usize = 65_535;
/// The longest texture name the importer writes (bytes, with its NUL).
pub const MAX_TEXTURE: usize = 255;
/// Two values closer than this are the same pose (rotation, translation, reduced keys).
const POSE_TOL: f64 = 1e-5;
/// A scale within this of 1 is no scale.
const SCALE_TOL: f64 = 1e-4;

/// What to read the file as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportKind {
    Ani,
    C3dc,
    Mesh3do,
    Mesh3de,
}

impl ImportKind {
    /// The kind of an output name (`.ani`, `.3dc`, `.3do`, `.3de`, any letter case).
    pub fn of_name(name: &str) -> Option<ImportKind> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "ani" => Some(ImportKind::Ani),
            "3dc" => Some(ImportKind::C3dc),
            "3do" => Some(ImportKind::Mesh3do),
            "3de" => Some(ImportKind::Mesh3de),
            _ => None,
        }
    }

    pub fn needs_like(self) -> bool {
        matches!(self, ImportKind::Ani | ImportKind::C3dc)
    }
}

/// The file the new one will replace (or the rig it moves with).
#[derive(Clone, Debug)]
pub enum Like {
    Ani(Ani),
    C3dc(C3dc),
}

#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// `node name = bone index` pairs (see [`parse_name_map`]); nodes not listed are read as `Bone_NNN`.
    pub map: Vec<(String, usize)>,
    /// A vertex with more than two influences keeps its two largest (renormalised) instead of refusing the file.
    pub reduce_influences: bool,
    /// Drop the keys of an animation that the keys around them reproduce within 1e-5.
    pub reduce: bool,
    /// Use the file's own key times (each a multiple of 1/30 s) and its linear values; no resampling.
    pub keys_exact: bool,
    /// Checked once per bone and once per 1024 vertices: when set, the import stops with a refusal that says "cancelled" and nothing is made.
    pub cancel: Option<shaiya_core::job::Cancel>,
}

impl ImportOptions {
    fn stopped(&self) -> bool {
        self.cancel.as_ref().is_some_and(shaiya_core::job::Cancel::is_cancelled)
    }
}

#[derive(Clone, Debug)]
pub enum Imported {
    Ani(Ani),
    C3dc(C3dc),
    Mesh3do(Mesh3do),
    Mesh3de(Mesh3de),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    pub notes: Vec<String>,
    /// The name of the animation read.
    pub animation: Option<String>,
    pub bones: usize,
    /// Rotation and translation keys written.
    pub keys: usize,
    pub meshes: usize,
    pub vertices: usize,
    /// Vertices that had more than two influences and were reduced.
    pub influences_reduced: usize,
    /// The largest weight dropped by `reduce_influences`.
    pub max_weight_dropped: f32,
}

/// Every reason a file cannot be imported (at most 200 listed).
#[derive(Clone, Debug)]
pub struct ImportError {
    pub list: Vec<Refusal>,
    pub more: usize,
}

impl ImportError {
    pub fn one(kind: RefusalKind, what: impl Into<String>, reason: impl Into<String>) -> ImportError {
        ImportError { list: vec![Refusal { kind, what: what.into(), reason: reason.into() }], more: 0 }
    }

    /// The kinds of the refusals, each once, in order of first appearance.
    pub fn kinds(&self) -> Vec<RefusalKind> {
        let mut out = Vec::new();
        for r in &self.list {
            if !out.contains(&r.kind) {
                out.push(r.kind);
            }
        }
        out
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<String> = self.list.iter().take(3).map(|r| r.to_string()).collect();
        write!(f, "cannot be imported: {}", shown.join("; "))?;
        let rest = self.list.len().saturating_sub(3) + self.more;
        if rest > 0 {
            write!(f, "; and {rest} more")?;
        }
        Ok(())
    }
}

impl std::error::Error for ImportError {}

#[derive(Default)]
struct Col {
    list: Vec<Refusal>,
    more: usize,
    /// Accessor values decoded so far (against [`MAX_IMPORT_VALUES`]).
    decoded: usize,
}

impl Col {
    fn add(&mut self, kind: RefusalKind, what: impl Into<String>, reason: impl Into<String>) {
        if self.list.len() < LIST_CAP {
            self.list.push(Refusal { kind, what: what.into(), reason: reason.into() });
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

// ---- the name map -------------------------------------------------------------------------------------------------------------------

/// The lines of a `--map` file: `node name = bone index` (blank lines and lines starting with `#` are skipped; the name is everything
/// before the last `=`, trimmed). A node named twice, or two nodes for one bone, is an error.
pub fn parse_name_map(text: &str) -> Result<Vec<(String, usize)>, String> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, idx)) = line.rsplit_once('=') else { return Err(format!("line {}: expected `node name = bone index`", n + 1)) };
        let name = name.trim();
        let idx: usize = idx.trim().parse().map_err(|_| format!("line {}: {:?} is not a bone number", n + 1, idx.trim()))?;
        if name.is_empty() {
            return Err(format!("line {}: the node name is empty", n + 1));
        }
        if out.iter().any(|(m, _)| m == name) {
            return Err(format!("line {}: the node {name:?} is listed twice", n + 1));
        }
        if let Some((m, _)) = out.iter().find(|(_, i)| *i == idx) {
            return Err(format!("line {}: bone {idx} is already given to {m:?}", n + 1));
        }
        out.push((name.to_string(), idx));
    }
    Ok(out)
}

/// The bone a node name stands for: the explicit map first, else `Bone_` and decimal digits of any width (`Bone_5` is `Bone_005`).
fn bone_of_name(name: &str, map: &HashMap<&str, usize>) -> Option<usize> {
    if let Some(&b) = map.get(name) {
        return Some(b);
    }
    let digits = name.strip_prefix("Bone_")?;
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The bone number a node or joint name stands for (`Bone_NNN` of any width), with no explicit map.
pub(crate) fn bone_number(name: &str) -> Option<usize> {
    bone_of_name(name, &HashMap::new())
}

// ---- nodes --------------------------------------------------------------------------------------------------------------------------

struct Node {
    name: String,
    children: Vec<usize>,
    parent: Option<usize>,
    /// The node's own placement in glTF space.
    trs: Trs,
    mesh: Option<usize>,
    skin: Option<usize>,
}

fn label(i: usize, name: &str) -> String {
    format!("node {i} ({name:?})")
}

fn floats_of(v: &Value, key: &str, n: usize) -> Result<Option<Vec<f32>>, String> {
    let Some(a) = v.get(key) else { return Ok(None) };
    let Some(a) = a.as_array().filter(|a| a.len() == n) else { return Err(format!("{key} is not a list of {n} numbers")) };
    let mut out = Vec::with_capacity(n);
    for x in a {
        let f = x.as_f64().ok_or_else(|| format!("{key} holds something that is not a number"))? as f32;
        if !f.is_finite() {
            return Err(format!("{key} holds a number that is not finite"));
        }
        out.push(f);
    }
    Ok(Some(out))
}

fn index_of(v: &Value, key: &str, limit: usize) -> Result<Option<usize>, String> {
    match v.get(key) {
        None => Ok(None),
        Some(x) => match x.as_u64() {
            Some(i) if (i as usize) < limit => Ok(Some(i as usize)),
            _ => Err(format!("{key} is not an index of an existing entry")),
        },
    }
}

fn read_nodes(g: &Gltf, col: &mut Col) -> Vec<Node> {
    let raw = g.array("nodes");
    let n = raw.len();
    let mut nodes: Vec<Node> = Vec::with_capacity(n);
    for (i, v) in raw.iter().enumerate() {
        let name = v.get("name").and_then(Value::as_str).map_or_else(|| format!("node{i}"), str::to_string);
        let what = label(i, &name);
        let mut trs = Trs::IDENTITY;
        let parsed = (|| -> Result<(), String> {
            let t = floats_of(v, "translation", 3)?;
            let r = floats_of(v, "rotation", 4)?;
            let s = floats_of(v, "scale", 3)?;
            let m = floats_of(v, "matrix", 16)?;
            if let Some(m) = m {
                if t.is_some() || r.is_some() || s.is_some() {
                    return Err("has both a matrix and translation, rotation or scale".to_string());
                }
                let mut a = [0f32; 16];
                a.copy_from_slice(&m);
                // the matrix is column-major in glTF space; the same 16 floats through the mirror are the game's row-vector matrix
                let game = decompose(&s_matrix(&a)).map_err(|e| format!("its matrix is not a scale, rotation and move: {}", e.reason()))?;
                trs = game.mirrored();
            } else {
                if let Some(t) = t {
                    trs.t = [t[0], t[1], t[2]];
                }
                if let Some(r) = r {
                    trs.q = normalized([r[0], r[1], r[2], r[3]]).ok_or("its rotation has length 0")?;
                }
                if let Some(s) = s {
                    trs.s = [s[0], s[1], s[2]];
                }
            }
            Ok(())
        })();
        if let Err(m) = parsed {
            col.add(RefusalKind::Transform, what.clone(), m);
        }
        let mut children = Vec::new();
        match v.get("children") {
            None => {}
            Some(c) => match c.as_array() {
                Some(a) => {
                    for x in a {
                        match x.as_u64() {
                            Some(k) if (k as usize) < n => children.push(k as usize),
                            _ => col.add(RefusalKind::Format, what.clone(), "lists a child that is not a node"),
                        }
                    }
                }
                None => col.add(RefusalKind::Format, what.clone(), "children is not a list"),
            },
        }
        let mesh = index_of(v, "mesh", g.array("meshes").len()).unwrap_or_else(|m| {
            col.add(RefusalKind::Format, what.clone(), m);
            None
        });
        let skin = index_of(v, "skin", g.array("skins").len()).unwrap_or_else(|m| {
            col.add(RefusalKind::Format, what.clone(), m);
            None
        });
        nodes.push(Node { name, children, parent: None, trs, mesh, skin });
    }
    for i in 0..n {
        for c in nodes[i].children.clone() {
            if let Some(p) = nodes[c].parent {
                if p != i {
                    col.add(RefusalKind::Format, label(c, &nodes[c].name), format!("is a child of both node {p} and node {i}"));
                }
            } else {
                nodes[c].parent = Some(i);
            }
        }
    }
    // a cycle: walking up from a node never ends
    for i in 0..n {
        let mut at = i;
        let mut steps = 0;
        while let Some(p) = nodes[at].parent {
            at = p;
            steps += 1;
            if steps > n {
                col.add(RefusalKind::Format, label(i, &nodes[i].name), "is part of a cycle of nodes");
                break;
            }
        }
    }
    nodes
}

// ---- accessors ----------------------------------------------------------------------------------------------------------------------

const FLOAT: u32 = 5126;

/// Reads the accessor `idx` as `what` wants it: the component types and the number of components, else a refusal that says why.
fn get_acc(g: &Gltf, idx: &Value, what: &str, types: &[u32], width: usize, col: &mut Col) -> Option<Accessor> {
    let a = decode_acc(g, idx, what, col)?;
    fits(&a, idx.as_u64().unwrap_or(0), what, types, width, col).then_some(a)
}

/// Decodes the accessor `idx` names, counting its values against the import's budget before anything is allocated.
fn decode_acc(g: &Gltf, idx: &Value, what: &str, col: &mut Col) -> Option<Accessor> {
    let Some(i) = idx.as_u64() else {
        col.add(RefusalKind::Format, what, "names an accessor that is not an index");
        return None;
    };
    if let Some(n) = g.accessor_values(i as usize) {
        if col.decoded.saturating_add(n) > MAX_IMPORT_VALUES {
            col.add(RefusalKind::Limit, what, format!("the accessors read so far and accessor {i} hold more than {MAX_IMPORT_VALUES} values together"));
            return None;
        }
    }
    match g.accessor(i as usize) {
        Err(m) => {
            let kind = if m.contains("sparse") || m.contains("normalized") { RefusalKind::Unsupported } else if m.contains("more than") { RefusalKind::Limit } else { RefusalKind::Format };
            col.add(kind, what, m);
            None
        }
        Ok(a) => {
            col.decoded = col.decoded.saturating_add(a.data.len());
            Some(a)
        }
    }
}

/// True when `a` holds `width` numbers of one of `types` per element; else a refusal that says why.
fn fits(a: &Accessor, i: u64, what: &str, types: &[u32], width: usize, col: &mut Col) -> bool {
    if !types.contains(&a.component_type) || a.width != width {
        let want = if types == [FLOAT] { "32-bit floats".to_string() } else { "unsigned integers".to_string() };
        col.add(RefusalKind::Unsupported, what, format!("accessor {i} does not hold {width} {want} per element (component type {}, {} per element): quantized data is not read", a.component_type, a.width));
        return false;
    }
    true
}

/// The animation accessors already decoded, by index: many samplers may name one accessor and it is decoded once.
type AccCache = HashMap<usize, Rc<Accessor>>;

fn get_acc_shared(g: &Gltf, cache: &mut AccCache, idx: &Value, what: &str, width: usize, col: &mut Col) -> Option<Rc<Accessor>> {
    let i = idx.as_u64().unwrap_or(0);
    let a = match idx.as_u64().and_then(|i| cache.get(&(i as usize))) {
        Some(a) => Rc::clone(a),
        None => {
            let a = Rc::new(decode_acc(g, idx, what, col)?);
            cache.insert(i as usize, Rc::clone(&a));
            a
        }
    };
    fits(&a, i, what, &[FLOAT], width, col).then_some(a)
}

// ---- textures -----------------------------------------------------------------------------------------------------------------------

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

/// The texture name bytes of a material: `extras.shaiya.texture_hex` as written, else the material's name (cp949, NUL-terminated), else none.
/// An image uri is never read.
fn texture_of(g: &Gltf, material: Option<usize>, what: &str, col: &mut Col) -> Vec<u8> {
    let Some(m) = material.and_then(|i| g.array("materials").get(i)) else { return Vec::new() };
    if let Some(h) = m.get("extras").and_then(|e| e.get("shaiya")).and_then(|s| s.get("texture_hex")) {
        return match h.as_str().and_then(unhex) {
            Some(b) if b.len() <= MAX_TEXTURE => b,
            Some(b) => {
                col.add(RefusalKind::Limit, what, format!("the texture name is {} bytes (at most {MAX_TEXTURE})", b.len()));
                Vec::new()
            }
            None => {
                col.add(RefusalKind::Format, what, "extras.shaiya.texture_hex is not hexadecimal text");
                Vec::new()
            }
        };
    }
    let Some(name) = m.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { return Vec::new() };
    let (bytes, _, bad) = encoding_rs::EUC_KR.encode(name);
    if bad {
        col.add(RefusalKind::Format, what, format!("the material name {name:?} has characters that cp949 cannot hold (the code page the toolkit assumes for names, validated on file contents of us-108)"));
        return Vec::new();
    }
    if bytes.len() + 1 > MAX_TEXTURE {
        col.add(RefusalKind::Limit, what, format!("the texture name is {} bytes (at most {MAX_TEXTURE})", bytes.len() + 1));
        return Vec::new();
    }
    let mut out = bytes.into_owned();
    out.push(0);
    out
}

// ---- meshes -------------------------------------------------------------------------------------------------------------------------

/// One primitive, still in glTF space.
struct Prim {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Vec<[f32; 2]>,
    tris: Vec<[u16; 3]>,
    /// Up to two sets of four joints and weights.
    joints: Vec<Vec<f64>>,
    weights: Vec<Vec<f64>>,
    pad: Option<Vec<u16>>,
    material: Option<usize>,
}

fn v3(d: &[f64]) -> Vec<[f32; 3]> {
    d.as_chunks::<3>().0.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect()
}

fn read_prim(g: &Gltf, mesh: usize, what: &str, col: &mut Col) -> Option<Prim> {
    let m = &g.array("meshes")[mesh];
    let prims = m.get("primitives").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    if prims.len() != 1 {
        col.add(RefusalKind::Unsupported, what, format!("mesh {mesh} has {} primitives (the .3DO and .3DE files read here hold one texture and one primitive)", prims.len()));
        return None;
    }
    let p = &prims[0];
    if p.get("targets").is_some_and(|t| t.as_array().is_none_or(|a| !a.is_empty())) {
        col.add(RefusalKind::Morph, what, "has morph targets (shape keys); the game formats hold none");
        return None;
    }
    let mode = p.get("mode").and_then(Value::as_u64).unwrap_or(4);
    if mode != 4 {
        col.add(RefusalKind::Unsupported, what, format!("its primitive mode is {mode}, not 4 (triangles)"));
        return None;
    }
    let Some(attrs) = p.get("attributes").and_then(Value::as_object) else {
        col.add(RefusalKind::Format, what, "the primitive has no attributes");
        return None;
    };
    let before = col.list.len();
    let pos = match attrs.get("POSITION") {
        Some(a) => get_acc(g, a, &format!("{what}, POSITION"), &[FLOAT], 3, col),
        None => {
            col.add(RefusalKind::Format, what, "the primitive has no POSITION");
            None
        }
    };
    let pos = pos?;
    if pos.count > MAX_VERTICES {
        col.add(RefusalKind::Limit, what, format!("{} vertices; a game mesh holds at most {MAX_VERTICES}", pos.count));
        return None;
    }
    if let Some(k) = pos.data.iter().position(|x| !x.is_finite()) {
        col.add(RefusalKind::NotFinite, format!("{what}, vertex {}", k / 3), format!("vertex {} holds NaN or an infinity", k / 3));
        return None;
    }
    let n = pos.count;
    let normals = match attrs.get("NORMAL") {
        Some(a) => get_acc(g, a, &format!("{what}, NORMAL"), &[FLOAT], 3, col),
        None => None,
    };
    if let Some(a) = &normals {
        if a.count != n {
            col.add(RefusalKind::Format, what, format!("NORMAL has {} entries for {n} vertices", a.count));
        } else if let Some(k) = a.data.iter().position(|x| !x.is_finite()) {
            col.add(RefusalKind::NotFinite, format!("{what}, vertex {}", k / 3), format!("vertex {} holds NaN or an infinity in its normal", k / 3));
        }
    }
    let uv = match attrs.get("TEXCOORD_0") {
        Some(a) => get_acc(g, a, &format!("{what}, TEXCOORD_0"), &[FLOAT], 2, col),
        None => {
            col.add(RefusalKind::Format, what, "the primitive has no TEXCOORD_0 (texture coordinates)");
            None
        }
    };
    if let Some(a) = &uv {
        if a.count != n {
            col.add(RefusalKind::Format, what, format!("TEXCOORD_0 has {} entries for {n} vertices", a.count));
        } else if let Some(k) = a.data.iter().position(|x| !x.is_finite()) {
            col.add(RefusalKind::NotFinite, format!("{what}, vertex {}", k / 2), format!("vertex {} holds NaN or an infinity in its texture coordinates", k / 2));
        }
    }
    let idx = match p.get("indices") {
        Some(a) => get_acc(g, a, &format!("{what}, indices"), &[5121, 5123, 5125], 1, col),
        None => {
            col.add(RefusalKind::Format, what, "the primitive has no indices");
            None
        }
    };
    let mut tris = Vec::new();
    if let Some(a) = &idx {
        if a.count % 3 != 0 {
            col.add(RefusalKind::Unsupported, what, format!("{} indices are not a multiple of 3", a.count));
        } else if let Some(k) = a.data.iter().position(|x| *x >= n as f64) {
            col.add(RefusalKind::Format, what, format!("index {k} names vertex {}, but the mesh has {n}", a.data[k]));
        } else {
            tris = a.data.as_chunks::<3>().0.iter().map(|c| [c[0] as u16, c[1] as u16, c[2] as u16]).collect();
        }
    }
    let mut joints = Vec::new();
    let mut weights = Vec::new();
    for set in 0..2 {
        let (jk, wk) = (format!("JOINTS_{set}"), format!("WEIGHTS_{set}"));
        match (attrs.get(&jk), attrs.get(&wk)) {
            (None, None) => break,
            (Some(j), Some(w)) => {
                let j = get_acc(g, j, &format!("{what}, {jk}"), &[5121, 5123], 4, col);
                let w = get_acc(g, w, &format!("{what}, {wk}"), &[FLOAT], 4, col);
                if let (Some(j), Some(w)) = (j, w) {
                    if j.count != n || w.count != n {
                        col.add(RefusalKind::Format, what, format!("{jk} and {wk} must have one entry per vertex ({n})"));
                    } else {
                        joints.push(j.data);
                        weights.push(w.data);
                    }
                }
            }
            _ => col.add(RefusalKind::Format, what, format!("has {jk} or {wk} but not both")),
        }
    }
    if attrs.contains_key("JOINTS_2") || attrs.contains_key("WEIGHTS_2") {
        col.add(RefusalKind::Influences, what, "has more than 8 joint influences per vertex (JOINTS_2)");
    }
    let pad = match attrs.get("_SHAIYA_PAD") {
        Some(a) => get_acc(g, a, &format!("{what}, _SHAIYA_PAD"), &[5123], 1, col).and_then(|a| {
            if a.count == n {
                Some(a.data.iter().map(|x| *x as u16).collect::<Vec<u16>>())
            } else {
                col.add(RefusalKind::Format, what, format!("_SHAIYA_PAD has {} entries for {n} vertices", a.count));
                None
            }
        }),
        None => None,
    };
    if col.list.len() > before {
        return None;
    }
    let uv = uv?;
    Some(Prim {
        positions: v3(&pos.data),
        normals: normals.map(|a| v3(&a.data)),
        uvs: uv.data.as_chunks::<2>().0.iter().map(|c| [c[0] as f32, c[1] as f32]).collect(),
        tris,
        joints,
        weights,
        pad,
        material: p.get("material").and_then(Value::as_u64).map(|m| m as usize),
    })
}

/// Vertex normals of a mesh in game space (area-weighted face normals; the game's clockwise faces have the cross product of their
/// first two edges as the outward normal), for a file that has none.
pub(crate) fn computed_normals(pos: &[[f32; 3]], faces: &[[u16; 3]]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0f64; 3]; pos.len()];
    for f in faces {
        let (a, b, c) = (pos[f[0] as usize], pos[f[1] as usize], pos[f[2] as usize]);
        let u = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1]), f64::from(b[2] - a[2])];
        let v = [f64::from(c[0] - a[0]), f64::from(c[1] - a[1]), f64::from(c[2] - a[2])];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for i in f {
            for k in 0..3 {
                acc[*i as usize][k] += n[k];
            }
        }
    }
    acc.iter()
        .map(|n| {
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if l < 1e-12 {
                [0.0, 1.0, 0.0]
            } else {
                [(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32]
            }
        })
        .collect()
}

fn is_identity(t: &Trs) -> bool {
    t.t.iter().all(|c| c.abs() < 1e-6) && (t.q[3].abs() - 1.0).abs() < 1e-6 && t.q[..3].iter().all(|c| c.abs() < 1e-6) && t.s.iter().all(|s| (s - 1.0).abs() < 1e-6)
}

// ---- the skeleton -------------------------------------------------------------------------------------------------------------------

/// `bone_node[b]` is the node of bone `b`, `node_bone[n]` the bone of node `n`. Refuses extra nodes named like bones, nodes that map to one
/// bone twice and (with `all`) bones that have no node.
fn match_bones(nodes: &[Node], bones: usize, map: &[(String, usize)], all: bool, col: &mut Col) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let by_name: HashMap<&str, usize> = map.iter().map(|(n, b)| (n.as_str(), *b)).collect();
    for (name, b) in map {
        if !nodes.iter().any(|n| &n.name == name) {
            col.add(RefusalKind::Names, format!("map entry {name:?}"), format!("no node of this name (the map gives it bone {b})"));
        }
    }
    let mut bone_node: Vec<Option<usize>> = vec![None; bones];
    let mut node_bone: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut extra = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let Some(b) = bone_of_name(&n.name, &by_name) else { continue };
        if b >= bones {
            extra.push(format!("{:?} (bone {b})", n.name));
            continue;
        }
        match bone_node[b] {
            Some(other) => col.add(RefusalKind::Names, label(i, &n.name), format!("and node {other} ({:?}) both stand for bone {b}", nodes[other].name)),
            None => {
                bone_node[b] = Some(i);
                node_bone[i] = Some(b);
            }
        }
    }
    if !extra.is_empty() {
        let shown: Vec<&str> = extra.iter().take(10).map(String::as_str).collect();
        col.add(RefusalKind::Names, "joints", format!("{} nodes name bones the like file does not have ({} bones): {}{}", extra.len(), bones, shown.join(", "), if extra.len() > 10 { ", ..." } else { "" }));
    }
    // a node above the skeleton that is no bone moves every bone below it; the game file has no place for that placement, so it must be
    // the identity (apply it in the program that made the file)
    let mut flagged: Vec<usize> = Vec::new();
    for (i, nb) in node_bone.iter().enumerate() {
        if nb.is_none() {
            continue;
        }
        let mut at = nodes[i].parent;
        let mut steps = 0;
        while let Some(p) = at {
            steps += 1;
            if steps > nodes.len() {
                break;
            }
            if node_bone[p].is_none() && !is_identity(&nodes[p].trs) && !flagged.contains(&p) {
                flagged.push(p);
                col.add(
                    RefusalKind::Transform,
                    label(p, &nodes[p].name),
                    "is above the skeleton and has a transform (a rotation, scale or move) that the game file cannot hold: apply it in the program that made the file",
                );
            }
            at = nodes[p].parent;
        }
    }
    if all {
        let missing: Vec<usize> = (0..bones).filter(|b| bone_node[*b].is_none()).collect();
        if !missing.is_empty() {
            let shown: Vec<String> = missing.iter().take(10).map(|b| format!("bone {b}")).collect();
            col.add(RefusalKind::Names, "skeleton", format!("{} bones of the like file have no node (Bone_NNN or --map): {}{}", missing.len(), shown.join(", "), if missing.len() > 10 { ", ..." } else { "" }));
        }
    }
    (bone_node, node_bone)
}

// ---- animation ----------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Interp {
    Linear,
    Step,
    Cubic,
}

/// The numbers of a decoded accessor, shared by every channel that reads them.
#[derive(Clone)]
struct Data(Rc<Accessor>);

impl Deref for Data {
    type Target = [f64];
    fn deref(&self) -> &[f64] {
        &self.0.data
    }
}

#[derive(Clone)]
struct Chan {
    times: Data,
    vals: Data,
    width: usize,
    interp: Interp,
}

impl Chan {
    fn value(&self, k: usize) -> &[f64] {
        let w = self.width;
        match self.interp {
            Interp::Cubic => &self.vals[(3 * k + 1) * w..(3 * k + 2) * w],
            _ => &self.vals[k * w..(k + 1) * w],
        }
    }
}

fn slerp(a: &[f64], b: &[f64], f: f64) -> [f64; 4] {
    let mut dot: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    let sign = if dot < 0.0 {
        dot = -dot;
        -1.0
    } else {
        1.0
    };
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - f, f * sign)
    } else {
        let th = dot.min(1.0).acos();
        let s = th.sin();
        (((1.0 - f) * th).sin() / s, sign * (f * th).sin() / s)
    };
    let q: [f64; 4] = std::array::from_fn(|i| a[i] * wa + b[i] * wb);
    unit4(q)
}

fn unit4(q: [f64; 4]) -> [f64; 4] {
    let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    if n < 1e-12 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        q.map(|x| x / n)
    }
}

/// The channel's value at `t` seconds by the glTF rules for its interpolation; outside its keys the nearest key's value holds.
fn sample(c: &Chan, t: f64, rotation: bool) -> Vec<f64> {
    let (w, n) = (c.width, c.times.len());
    if t <= c.times[0] {
        return c.value(0).to_vec();
    }
    if t >= c.times[n - 1] {
        return c.value(n - 1).to_vec();
    }
    let k = c.times.partition_point(|x| *x <= t) - 1;
    let (t0, t1) = (c.times[k], c.times[k + 1]);
    let s = (t - t0) / (t1 - t0);
    match c.interp {
        Interp::Step => c.value(k).to_vec(),
        Interp::Linear => {
            if rotation {
                slerp(c.value(k), c.value(k + 1), s).to_vec()
            } else {
                (0..w).map(|i| c.value(k)[i] + (c.value(k + 1)[i] - c.value(k)[i]) * s).collect()
            }
        }
        Interp::Cubic => {
            let td = t1 - t0;
            let (v0, v1) = (c.value(k), c.value(k + 1));
            let out_tangent = &c.vals[(3 * k + 2) * w..(3 * k + 3) * w];
            let in_tangent = &c.vals[3 * (k + 1) * w..(3 * (k + 1) + 1) * w];
            let (s2, s3) = (s * s, s * s * s);
            let r: Vec<f64> = (0..w).map(|i| (2.0 * s3 - 3.0 * s2 + 1.0) * v0[i] + (s3 - 2.0 * s2 + s) * td * out_tangent[i] + (-2.0 * s3 + 3.0 * s2) * v1[i] + (s3 - s2) * td * in_tangent[i]).collect();
            if rotation {
                unit4([r[0], r[1], r[2], r[3]]).to_vec()
            } else {
                r
            }
        }
    }
}

#[derive(Default)]
struct BoneChans {
    rot: Option<Chan>,
    trans: Option<Chan>,
    scale: Option<Chan>,
}

struct AnimData {
    name: String,
    chans: Vec<BoneChans>,
    start_key: i32,
    end_key: Option<i32>,
    tmin: f64,
    tmax: f64,
}

fn extras_i32(v: &Value, key: &str) -> Option<i32> {
    v.get("extras")?.get("shaiya")?.get(key)?.as_i64().and_then(|x| i32::try_from(x).ok())
}

fn read_animation(g: &Gltf, nodes: &[Node], node_bone: &[Option<usize>], bones: usize, col: &mut Col) -> Option<AnimData> {
    let anims = g.array("animations");
    if anims.is_empty() {
        col.add(RefusalKind::Animation, "animations", "the file holds no animation");
        return None;
    }
    if anims.len() > 1 {
        let names: Vec<String> = anims.iter().enumerate().take(8).map(|(i, a)| a.get("name").and_then(Value::as_str).map_or_else(|| format!("#{i}"), str::to_string)).collect();
        col.add(RefusalKind::Animation, "animations", format!("the file holds {} animations ({}{}); an .ANI holds one: export one action", anims.len(), names.join(", "), if anims.len() > 8 { ", ..." } else { "" }));
        return None;
    }
    let a = &anims[0];
    let name = a.get("name").and_then(Value::as_str).unwrap_or("animation").to_string();
    let samplers = a.get("samplers").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let channels = a.get("channels").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    if channels.is_empty() {
        col.add(RefusalKind::Animation, &name, "the animation has no channels");
        return None;
    }
    let before = col.list.len();
    let mut chans: Vec<BoneChans> = (0..bones).map(|_| BoneChans::default()).collect();
    let (mut tmin, mut tmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut cache: HashMap<usize, Option<Chan>> = HashMap::new();
    let mut accs: AccCache = HashMap::new();
    for (ci, c) in channels.iter().enumerate() {
        let what = format!("{name}, channel {ci}");
        let target = c.get("target");
        let (Some(node), Some(path)) = (target.and_then(|t| t.get("node")).and_then(Value::as_u64), target.and_then(|t| t.get("path")).and_then(Value::as_str)) else {
            col.add(RefusalKind::Unsupported, what, "has no target node (an extension target is not read)");
            continue;
        };
        let node = node as usize;
        if node >= nodes.len() {
            col.add(RefusalKind::Format, what, format!("targets node {node}, which does not exist"));
            continue;
        }
        let Some(bone) = node_bone[node] else {
            col.add(RefusalKind::Animation, what, format!("animates {}, which is not a bone of the rig (the import reads only the animation of the bones)", label(node, &nodes[node].name)));
            continue;
        };
        let width = match path {
            "translation" | "scale" => 3,
            "rotation" => 4,
            "weights" => {
                col.add(RefusalKind::Morph, what, format!("animates morph weights on {}; the game formats hold no shape keys", label(node, &nodes[node].name)));
                continue;
            }
            other => {
                col.add(RefusalKind::Unsupported, what, format!("animates {other:?}, which is not read"));
                continue;
            }
        };
        let Some(si) = c.get("sampler").and_then(Value::as_u64).map(|s| s as usize).filter(|s| *s < samplers.len()) else {
            col.add(RefusalKind::Format, what, "names a sampler that does not exist");
            continue;
        };
        // the same sampler may serve several channels, but its width must fit each
        let key = si * 8 + width;
        let ch = match cache.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => v.insert(read_sampler(g, &mut accs, &samplers[si], width, &format!("{name}, sampler {si}"), col)),
        };
        let Some(ch) = ch.as_ref() else { continue };
        let copy = ch.clone();
        tmin = tmin.min(copy.times[0]);
        tmax = tmax.max(copy.times[copy.times.len() - 1]);
        let slot = match path {
            "rotation" => &mut chans[bone].rot,
            "translation" => &mut chans[bone].trans,
            _ => &mut chans[bone].scale,
        };
        if slot.is_some() {
            col.add(RefusalKind::Animation, what, format!("a second {path} channel on {}", label(node, &nodes[node].name)));
            continue;
        }
        *slot = Some(copy);
    }
    if col.list.len() > before {
        return None;
    }
    let start_key = extras_i32(a, "start_key").or_else(|| g.json.get("asset").and_then(|x| extras_i32(x, "start_key"))).unwrap_or(0);
    let end_key = extras_i32(a, "end_key").or_else(|| g.json.get("asset").and_then(|x| extras_i32(x, "end_key")));
    Some(AnimData { name, chans, start_key, end_key, tmin, tmax })
}

fn read_sampler(g: &Gltf, accs: &mut AccCache, s: &Value, width: usize, what: &str, col: &mut Col) -> Option<Chan> {
    let interp = match s.get("interpolation").and_then(Value::as_str).unwrap_or("LINEAR") {
        "LINEAR" => Interp::Linear,
        "STEP" => Interp::Step,
        "CUBICSPLINE" => Interp::Cubic,
        other => {
            col.add(RefusalKind::Unsupported, what, format!("the interpolation {other:?} is not read"));
            return None;
        }
    };
    let (Some(input), Some(output)) = (s.get("input"), s.get("output")) else {
        col.add(RefusalKind::Format, what, "the sampler has no input or output");
        return None;
    };
    // the key times are looked at before the output is decoded: a sampler of more keys than an animation can use is refused cheaply
    if input.as_u64().and_then(|i| g.accessor_values(i as usize)).is_some_and(|n| n > MAX_SAMPLER_KEYS) {
        col.add(RefusalKind::Limit, what, format!("the sampler has more than {MAX_SAMPLER_KEYS} key times (a game animation is sampled at most {MAX_KEYS} keys long)"));
        return None;
    }
    let input = get_acc_shared(g, accs, input, &format!("{what}, input"), 1, col)?;
    let output = get_acc_shared(g, accs, output, &format!("{what}, output"), width, col)?;
    let n = input.count;
    let want = if interp == Interp::Cubic { 3 * n } else { n };
    if output.count != want || (interp == Interp::Cubic && n < 2) {
        col.add(RefusalKind::Format, what, format!("{n} times and {} values do not fit the {} interpolation", output.count, if interp == Interp::Cubic { "cubic spline (3 values per time, at least 2 times)" } else { "chosen" }));
        return None;
    }
    if input.data.iter().any(|t| !t.is_finite() || *t < 0.0) {
        col.add(RefusalKind::Format, what, "a key time is negative or not finite");
        return None;
    }
    if input.data.windows(2).any(|w| w[1] <= w[0]) {
        col.add(RefusalKind::Format, what, "the key times do not increase");
        return None;
    }
    if output.data.iter().any(|x| !x.is_finite()) {
        col.add(RefusalKind::NotFinite, what, "a key value is NaN or an infinity");
        return None;
    }
    if width == 4 {
        for k in 0..output.count {
            let q = &output.data[k * 4..k * 4 + 4];
            if q.iter().map(|x| x * x).sum::<f64>() < 1e-12 && !(interp == Interp::Cubic && k % 3 != 1) {
                col.add(RefusalKind::NotFinite, what, "a rotation key has length 0");
                return None;
            }
        }
    }
    Some(Chan { times: Data(input), vals: Data(output), width, interp })
}

/// Half a key of tolerance in whole keys for a time of `x` keys: 1e-6 seconds plus the precision of a 32-bit time.
fn key_tol(x: f64) -> f64 {
    3e-5 + 2.4e-7 * x.abs()
}

fn snap(x: f64) -> f64 {
    let r = x.round();
    if (x - r).abs() <= key_tol(x) {
        r
    } else {
        x
    }
}

fn spread(vals: &[[f64; 4]]) -> f64 {
    vals.iter().map(|v| (0..4).map(|i| (v[i] - vals[0][i]).abs()).fold(0.0, f64::max)).fold(0.0, f64::max)
}

fn spread3(vals: &[[f64; 3]]) -> f64 {
    vals.iter().map(|v| (0..3).map(|i| (v[i] - vals[0][i]).abs()).fold(0.0, f64::max)).fold(0.0, f64::max)
}

/// Keeps the first and last key and every key that the keys kept around it cannot reproduce within [`POSE_TOL`] by interpolation.
fn reduce_keys<T: Copy>(keys: &[i32], vals: &[T], interp: &dyn Fn(&T, &T, f64) -> T, dist: &dyn Fn(&T, &T) -> f64) -> Vec<bool> {
    let n = keys.len();
    let mut keep = vec![false; n];
    if n == 0 {
        return keep;
    }
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let span = f64::from(keys[b] - keys[a]);
        let mut worst = (0usize, 0f64);
        for i in a + 1..b {
            let f = f64::from(keys[i] - keys[a]) / span;
            let d = dist(&vals[i], &interp(&vals[a], &vals[b], f));
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > POSE_TOL {
            keep[worst.0] = true;
            stack.push((a, worst.0));
            stack.push((worst.0, b));
        }
    }
    keep
}

fn quat_dist(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let dot: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    (0..4).map(|i| (a[i] - s * b[i]).abs()).fold(0.0, f64::max)
}

fn import_ani(g: &Gltf, nodes: &[Node], like: &Ani, opts: &ImportOptions, col: &mut Col, report: &mut ImportReport) -> Option<Ani> {
    let bones = like.bones.len();
    if bones == 0 {
        col.add(RefusalKind::Like, "like", "the like animation has no bones");
        return None;
    }
    let (bone_node, node_bone) = match_bones(nodes, bones, &opts.map, true, col);
    // the hierarchy must be the like file's
    let mut wrong = Vec::new();
    for (b, bn) in bone_node.iter().enumerate() {
        let Some(n) = bn else { continue };
        let want = like.bones[b].parent;
        let got = nodes[*n].parent.and_then(|p| node_bone[p]);
        let ok = if want >= 0 { got == Some(want as usize) } else { got.is_none() };
        if !ok {
            wrong.push(format!("bone {b}: parent is {}, the like file's is {want}", got.map_or(-1, |p| p as i64)));
        }
    }
    if !wrong.is_empty() {
        let shown: Vec<&str> = wrong.iter().take(10).map(String::as_str).collect();
        col.add(RefusalKind::Names, "hierarchy", format!("{} bones have another parent than in the like file: {}{}", wrong.len(), shown.join("; "), if wrong.len() > 10 { "; ..." } else { "" }));
    }
    let anim = read_animation(g, nodes, &node_bone, bones, col);
    if col.failed() {
        return None;
    }
    let anim = anim?;
    report.animation = Some(anim.name.clone());
    let start = f64::from(anim.start_key);
    let locs = locals(like);
    // the keys the animation is sampled at
    let (kmin, kmax) = {
        let (a, b) = (snap(start + anim.tmin * 30.0), snap(start + anim.tmax * 30.0));
        (a.ceil(), b.floor())
    };
    if kmax < kmin && !opts.keys_exact {
        col.add(RefusalKind::Animation, &anim.name, "the animation is shorter than one whole key (1/30 s)");
        return None;
    }
    // the span is checked as a float: a huge time must not wrap or saturate before the check
    if !opts.keys_exact && kmax - kmin + 1.0 > MAX_KEYS as f64 {
        col.add(RefusalKind::Limit, &anim.name, format!("the animation spans {:.0} keys; at most {MAX_KEYS} are sampled", kmax - kmin + 1.0));
        return None;
    }
    if !opts.keys_exact && (kmin.abs() > f64::from(i32::MAX / 2) || kmax.abs() > f64::from(i32::MAX / 2)) {
        col.add(RefusalKind::Limit, &anim.name, "the key numbers are out of range");
        return None;
    }
    let mut off_grid = false;
    let mut out_bones: Vec<AniBone> = like.bones.iter().map(|b| AniBone { parent: b.parent, matrix: b.matrix, rot: Vec::new(), trans: Vec::new() }).collect();
    let mut last_key = if opts.keys_exact { i32::MIN } else { kmax as i32 };
    let mut first_key = if opts.keys_exact { i32::MAX } else { kmin as i32 };
    // exact keys of every channel, checked first
    let exact_keys = |c: &Chan, what: &str, col: &mut Col| -> Option<Vec<i32>> {
        if c.interp != Interp::Linear {
            col.add(RefusalKind::Animation, what, "--keys exact needs LINEAR interpolation (a step or spline would not be reproduced by its keys)");
            return None;
        }
        let mut keys: Vec<i32> = Vec::with_capacity(c.times.len());
        for t in c.times.iter() {
            let x = start + t * 30.0;
            let r = x.round();
            if (x - r).abs() > key_tol(x) || r.abs() > f64::from(i32::MAX / 2) {
                col.add(RefusalKind::Animation, what, format!("the time {t} s is not a multiple of 1/30 s (--keys exact does not resample)"));
                return None;
            }
            keys.push(r as i32);
        }
        if keys.windows(2).any(|w| w[1] <= w[0]) {
            col.add(RefusalKind::Animation, what, "two key times fall on the same game key");
            return None;
        }
        Some(keys)
    };
    for b in 0..bones {
        if opts.stopped() {
            col.add(RefusalKind::Limit, "import", "cancelled");
            return None;
        }
        let n = bone_node[b].unwrap_or(0);
        let what = format!("bone {b} ({:?})", nodes[n].name);
        let rest = nodes[n].trs;
        let ch = &anim.chans[b];
        // the keys of the three kinds of channel for this bone
        let (rot_keys, trans_keys, scale_keys): (Vec<i32>, Vec<i32>, Vec<i32>) = if opts.keys_exact {
            let mut get = |c: &Option<Chan>, label: &str| -> Option<Vec<i32>> {
                match c {
                    Some(c) => exact_keys(c, &format!("{what}, {label}"), col),
                    None => Some(Vec::new()),
                }
            };
            let (Some(r), Some(t), Some(s)) = (get(&ch.rot, "rotation"), get(&ch.trans, "translation"), get(&ch.scale, "scale")) else { continue };
            (r, t, s)
        } else {
            let all: Vec<i32> = (kmin as i32..=kmax as i32).collect();
            for c in [&ch.rot, &ch.trans, &ch.scale].into_iter().flatten() {
                if c.times.iter().any(|t| {
                    let x = start + t * 30.0;
                    (x - x.round()).abs() > key_tol(x)
                }) {
                    off_grid = true;
                }
            }
            (all.clone(), all.clone(), all)
        };
        // values (glTF space) at those keys, or the node's own value held constant when the bone has no such channel
        let at = |k: i32| (f64::from(k) - start) / 30.0;
        let rot_vals: Vec<[f64; 4]> = match &ch.rot {
            Some(c) if opts.keys_exact => (0..c.times.len()).map(|k| unit4([c.vals[k * 4], c.vals[k * 4 + 1], c.vals[k * 4 + 2], c.vals[k * 4 + 3]])).collect(),
            Some(c) => rot_keys.iter().map(|k| unit4(sample(c, at(*k), true).try_into().unwrap_or([0.0, 0.0, 0.0, 1.0]))).collect(),
            None => vec![rest.q.map(f64::from)],
        };
        let trans_vals: Vec<[f64; 3]> = match &ch.trans {
            Some(c) if opts.keys_exact => (0..c.times.len()).map(|k| [c.vals[k * 3], c.vals[k * 3 + 1], c.vals[k * 3 + 2]]).collect(),
            Some(c) => trans_keys.iter().map(|k| sample(c, at(*k), false).try_into().unwrap_or([0.0; 3])).collect(),
            None => vec![rest.t.map(f64::from)],
        };
        let scale_vals: Vec<[f64; 3]> = match &ch.scale {
            Some(c) if opts.keys_exact => (0..c.times.len()).map(|k| [c.vals[k * 3], c.vals[k * 3 + 1], c.vals[k * 3 + 2]]).collect(),
            Some(c) => scale_keys.iter().map(|k| sample(c, at(*k), false).try_into().unwrap_or([1.0; 3])).collect(),
            None => vec![rest.s.map(f64::from)],
        };
        // continuity (the game slerps the short way; a viewer needs one hemisphere)
        let mut q32: Vec<[f32; 4]> = rot_vals.iter().map(|q| q.map(|c| c as f32)).collect();
        make_continuous(&mut q32);
        // from here on everything is in the game's space (the mirror undone once), where the like file's matrices are
        let rot_vals: Vec<[f64; 4]> = q32.iter().map(|q| s_quat(*q).map(f64::from)).collect();
        let q32: Vec<[f32; 4]> = q32.iter().map(|q| s_quat(*q)).collect();
        let trans_vals: Vec<[f64; 3]> = trans_vals.iter().map(|v| [v[0], v[1], -v[2]]).collect();
        // a scale: only when the bone's placement keeps it (the like file's own matrix has it and nothing moves)
        let scaled = scale_vals.iter().any(|s| s.iter().any(|x| (x - 1.0).abs() >= SCALE_TOL));
        let local = &locs[b];
        let rot_matches_local = {
            let m = quat_matrix(q32[0]);
            (0..3).all(|r| (0..3).all(|c| (f64::from(m[r * 4 + c]) - f64::from(local[r * 4 + c])).abs() <= POSE_TOL))
        };
        let (rot_varies, trans_varies) = (spread(&rot_vals) > POSE_TOL, spread3(&trans_vals) > POSE_TOL);
        let mut keep_rot_track = rot_varies || !rot_matches_local;
        if scaled {
            let s = scale_vals[0];
            let constant = spread3(&scale_vals) <= SCALE_TOL && !rot_varies;
            let m = quat_matrix(q32[0]);
            let same = (0..3).all(|r| (0..3).all(|c| (f64::from(m[r * 4 + c]) * s[r] - f64::from(local[r * 4 + c])).abs() <= SCALE_TOL));
            if !(constant && same) {
                col.add(
                    RefusalKind::Scale,
                    &what,
                    format!("has the scale ({:.4}, {:.4}, {:.4}): the game replaces the 3 x 3 of a bone with a rotation, so a scale would be lost", s[0], s[1], s[2]),
                );
                continue;
            }
            keep_rot_track = false;
        }
        let tl = [f64::from(local[12]), f64::from(local[13]), f64::from(local[14])];
        let trans_matches_local = tl.iter().all(|x| x.is_finite()) && (0..3).all(|i| (trans_vals[0][i] - tl[i]).abs() <= POSE_TOL);
        let keep_trans_track = trans_varies || !trans_matches_local;
        let base_key = if opts.keys_exact {
            let firsts: Vec<i32> = [&rot_keys, &trans_keys, &scale_keys].iter().filter_map(|k| k.first().copied()).collect();
            firsts.into_iter().min().unwrap_or(kmin as i32)
        } else {
            kmin as i32
        };
        // the tracks
        let mut rot: Vec<(i32, [f64; 4])> = Vec::new();
        if keep_rot_track {
            if rot_varies {
                let keys = if ch.rot.is_some() { rot_keys.clone() } else { vec![base_key] };
                rot = keys.iter().copied().zip(rot_vals.iter().copied()).collect();
            } else {
                rot.push((if ch.rot.is_some() { rot_keys[0] } else { base_key }, rot_vals[0]));
            }
            if opts.reduce && rot.len() > 2 {
                let (k, v): (Vec<i32>, Vec<[f64; 4]>) = rot.iter().copied().unzip();
                let keep = reduce_keys(&k, &v, &|a, b, f| slerp(a, b, f), &quat_dist);
                rot = rot.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect();
            }
        }
        let mut trans: Vec<(i32, [f64; 3])> = Vec::new();
        if keep_trans_track {
            if trans_varies {
                let keys = if ch.trans.is_some() { trans_keys.clone() } else { vec![base_key] };
                trans = keys.iter().copied().zip(trans_vals.iter().copied()).collect();
            } else {
                trans.push((if ch.trans.is_some() { trans_keys[0] } else { base_key }, trans_vals[0]));
            }
            if opts.reduce && trans.len() > 2 {
                let (k, v): (Vec<i32>, Vec<[f64; 3]>) = trans.iter().copied().unzip();
                let lerp = |a: &[f64; 3], b: &[f64; 3], f: f64| -> [f64; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f) };
                let dist = |a: &[f64; 3], b: &[f64; 3]| (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max);
                let keep = reduce_keys(&k, &v, &lerp, &dist);
                trans = trans.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect();
            }
        }
        let ob = &mut out_bones[b];
        ob.rot = rot.iter().map(|(k, q)| RotKey { key: *k, q: q.map(|c| c as f32) }).collect();
        ob.trans = trans.iter().map(|(k, v)| TransKey { key: *k, v: v.map(|c| c as f32) }).collect();
        for k in ob.rot.iter().map(|k| k.key).chain(ob.trans.iter().map(|k| k.key)) {
            last_key = last_key.max(k);
            first_key = first_key.min(k);
        }
        report.keys += ob.rot.len() + ob.trans.len();
    }
    if col.failed() {
        return None;
    }
    if off_grid {
        report.notes.push("the file's key times are not multiples of 1/30 s: the animation was sampled at every whole key".to_string());
    }
    if first_key > last_key {
        // no track at all: the file's pose is the like file's
        last_key = kmax.max(kmin) as i32;
    }
    let end_key = anim.end_key.filter(|e| *e >= last_key).unwrap_or(last_key);
    report.bones = bones;
    Some(Ani { first_field: like.first_field, end_key, bones: out_bones })
}

// ---- meshes to game files ------------------------------------------------------------------------------------------------------------

struct SkinInfo {
    /// The bone of each joint.
    joint_bone: Vec<usize>,
    /// The mesh's matrix of each joint (game space).
    ibm: Vec<Mat4>,
}

fn read_skin(g: &Gltf, skin: usize, nodes: &[Node], node_bone: &[Option<usize>], what: &str, col: &mut Col) -> Option<SkinInfo> {
    let s = &g.array("skins")[skin];
    let Some(joints) = s.get("joints").and_then(Value::as_array).filter(|j| !j.is_empty()) else {
        col.add(RefusalKind::Format, what, format!("skin {skin} has no joints"));
        return None;
    };
    let before = col.list.len();
    let mut joint_bone = Vec::with_capacity(joints.len());
    for (j, x) in joints.iter().enumerate() {
        match x.as_u64().map(|n| n as usize).filter(|n| *n < nodes.len()) {
            Some(n) => match node_bone[n] {
                Some(b) => {
                    if joint_bone.contains(&b) {
                        col.add(RefusalKind::Joint, what, format!("joint {j} ({:?}) is bone {b} again", nodes[n].name));
                    }
                    joint_bone.push(b);
                }
                None => {
                    col.add(RefusalKind::Joint, what, format!("joint {j} is {}, which is not a bone of the rig", label(n, &nodes[n].name)));
                    joint_bone.push(0);
                }
            },
            None => {
                col.add(RefusalKind::Format, what, format!("joint {j} is not a node"));
                joint_bone.push(0);
            }
        }
    }
    let ibm = match s.get("inverseBindMatrices") {
        Some(a) => {
            let a = get_acc(g, a, &format!("{what}, inverseBindMatrices"), &[FLOAT], 16, col)?;
            if a.count < joints.len() {
                col.add(RefusalKind::Format, what, format!("{} inverse bind matrices for {} joints", a.count, joints.len()));
                return None;
            }
            if a.data.iter().any(|x| !x.is_finite()) {
                col.add(RefusalKind::NotFinite, what, "an inverse bind matrix holds NaN or an infinity");
                return None;
            }
            a.data.as_chunks::<16>().0.iter().take(joints.len()).map(|c| s_matrix(&c.map(|x| x as f32))).collect()
        }
        None => vec![s_matrix(&crate::anim::IDENTITY); joints.len()],
    };
    (col.list.len() == before).then_some(SkinInfo { joint_bone, ibm })
}

fn import_c3dc(g: &Gltf, nodes: &[Node], like: &Like, opts: &ImportOptions, col: &mut Col, report: &mut ImportReport) -> Option<C3dc> {
    let (like_mesh, bones_of): (Option<&C3dc>, Vec<usize>) = match like {
        Like::C3dc(c) => {
            if c.layout == C3dcLayout::Cloak {
                col.add(RefusalKind::Like, "like", "the like file is a cloak (no bones); how the game moves a cloak was never checked, so it is not imported");
                return None;
            }
            (Some(c), c.meshes.iter().map(|m| m.bones.len()).collect())
        }
        Like::Ani(a) => (None, vec![a.bones.len(); 2]),
    };
    let max_bones = bones_of.iter().copied().max().unwrap_or(0);
    if max_bones == 0 || max_bones > 256 {
        col.add(RefusalKind::Like, "like", format!("the like file has {max_bones} bones; a .3DC names bones in one byte (1 to 256)"));
        return None;
    }
    let mesh_nodes: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.mesh.is_some()).map(|(i, _)| i).collect();
    let want = like_mesh.map_or(mesh_nodes.len(), |c| c.meshes.len());
    if mesh_nodes.is_empty() || mesh_nodes.len() > 2 || mesh_nodes.len() != want {
        col.add(
            RefusalKind::Unsupported,
            "meshes",
            format!("the file has {} mesh nodes; a .3DC holds {} mesh{}", mesh_nodes.len(), if like_mesh.is_some() { want.to_string() } else { "1 or 2".to_string() }, if want == 1 { "" } else { "es" }),
        );
        return None;
    }
    let (_, node_bone) = match_bones(nodes, max_bones, &opts.map, false, col);
    let mut meshes = Vec::new();
    for (k, &ni) in mesh_nodes.iter().enumerate() {
        let node = &nodes[ni];
        let what = label(ni, &node.name);
        let nb = bones_of[k.min(bones_of.len() - 1)];
        if !is_identity(&node.trs) {
            col.add(RefusalKind::Transform, &what, "has a transform: a skinned mesh must sit at the identity (apply it in the program that made the file)");
        }
        let Some(skin) = node.skin else {
            col.add(RefusalKind::Format, &what, "has no skin: a .3DC is a skinned mesh");
            continue;
        };
        let info = read_skin(g, skin, nodes, &node_bone, &what, col);
        let prim = read_prim(g, node.mesh.unwrap_or(0), &what, col);
        let (Some(info), Some(prim)) = (info, prim) else { continue };
        if prim.joints.is_empty() {
            col.add(RefusalKind::Format, &what, "has no JOINTS_0 and WEIGHTS_0");
            continue;
        }
        if info.joint_bone.iter().any(|b| *b >= nb) {
            let b = info.joint_bone.iter().find(|b| **b >= nb).copied().unwrap_or(0);
            col.add(RefusalKind::Joint, &what, format!("a joint is bone {b}, but the like file has {nb} bones"));
            continue;
        }
        let like_m = like_mesh.and_then(|c| c.meshes.get(k));
        let mut vertices = Vec::with_capacity(prim.positions.len());
        let (mut over, mut first_over) = (0usize, 0usize);
        let mut bad = 0usize;
        for v in 0..prim.positions.len() {
            if v % 1024 == 0 && opts.stopped() {
                col.add(RefusalKind::Limit, "import", "cancelled");
                return None;
            }
            let mut inf: Vec<(usize, f32)> = Vec::with_capacity(4);
            let mut vbad = false;
            for (js, ws) in prim.joints.iter().zip(&prim.weights) {
                for s in 0..4 {
                    let w = ws[v * 4 + s];
                    if !w.is_finite() || w < 0.0 || w > 1.0 + 1e-3 {
                        if !vbad {
                            col.add(RefusalKind::Weight, format!("{what}, vertex {v}"), format!("vertex {v} has the weight {w} (outside 0 to 1)"));
                        }
                        vbad = true;
                        continue;
                    }
                    if w == 0.0 {
                        continue;
                    }
                    let j = js[v * 4 + s] as usize;
                    let Some(&bone) = info.joint_bone.get(j) else {
                        if !vbad {
                            col.add(RefusalKind::Joint, format!("{what}, vertex {v}"), format!("vertex {v} names joint {j}, but the skin has {}", info.joint_bone.len()));
                        }
                        vbad = true;
                        continue;
                    };
                    match inf.iter_mut().find(|(b, _)| *b == bone) {
                        Some(e) => e.1 += w as f32,
                        None => inf.push((bone, w as f32)),
                    }
                }
            }
            if vbad {
                bad += 1;
                if bad >= 10 {
                    break;
                }
                continue;
            }
            let sum: f32 = inf.iter().map(|e| e.1).sum();
            if inf.is_empty() || (sum - 1.0).abs() > 1e-3 {
                col.add(RefusalKind::Weight, format!("{what}, vertex {v}"), format!("the weights of vertex {v} add up to {sum} (they must add up to 1)"));
                bad += 1;
                if bad >= 10 {
                    break;
                }
                continue;
            }
            if inf.len() > 2 {
                if !opts.reduce_influences {
                    if over == 0 {
                        first_over = v;
                    }
                    over += 1;
                    continue;
                }
                let mut order: Vec<usize> = (0..inf.len()).collect();
                order.sort_by(|a, b| inf[*b].1.total_cmp(&inf[*a].1).then(a.cmp(b)));
                let dropped = order[2..].iter().map(|i| inf[*i].1).fold(0f32, f32::max);
                report.influences_reduced += 1;
                report.max_weight_dropped = report.max_weight_dropped.max(dropped);
                let (a, b) = (inf[order[0]], inf[order[1]]);
                inf = vec![a, b];
            }
            let sum2: f32 = inf.iter().map(|e| e.1).sum();
            let (b1, b2, weight) = if inf.len() == 2 {
                (inf[0].0 as u8, inf[1].0 as u8, if (sum2 - 1.0).abs() <= 1e-6 { inf[0].1 } else { inf[0].1 / sum2 })
            } else {
                // one bone: the pose does not depend on the second bone or the weight; the like file's own values are kept where they pose the same,
                // and only for the like vertex that is this vertex (the same position and texture coordinate to the bit): an edited file whose
                // vertex order changed gets (b, b, 1) and never borrows a partner bone of another vertex
                let b = inf[0].0 as u8;
                let mut r = (b, b, 1.0f32);
                let (pos, uv) = (s_point(prim.positions[v]), prim.uvs[v]);
                let same_vertex = |lv: &C3dcVertex| lv.pos.iter().zip(&pos).all(|(x, y)| x.to_bits() == y.to_bits()) && lv.uv.iter().zip(&uv).all(|(x, y)| x.to_bits() == y.to_bits());
                if let Some(lv) = like_m.and_then(|m| m.vertices.get(v)).filter(|lv| same_vertex(lv)) {
                    if lv.weight.is_finite() {
                        if lv.bone1 == b && lv.bone2 == b {
                            r.2 = lv.weight;
                        } else if lv.weight == 1.0 && lv.bone1 == b {
                            r.1 = lv.bone2;
                        } else if lv.weight == 0.0 && lv.bone2 == b {
                            r = (lv.bone1, b, 0.0);
                        }
                    }
                }
                r
            };
            let normal = prim.normals.as_ref().map(|n| s_point(n[v]));
            vertices.push((s_point(prim.positions[v]), weight, b1, b2, prim.pad.as_ref().map_or(0, |p| p[v]), normal, prim.uvs[v]));
        }
        if over > 0 {
            col.add(
                RefusalKind::Influences,
                &what,
                format!("{over} vertices have more than 2 bone influences (first: vertex {first_over}); a .3DC vertex holds two (--reduce-influences keeps the two largest)"),
            );
        }
        if bad > 0 || over > 0 {
            continue;
        }
        let faces: Vec<[u16; 3]> = prim.tris.iter().map(|t| s_tri(*t)).collect();
        let mut note_normals = false;
        let normals: Vec<[f32; 3]> = if vertices.iter().all(|v| v.5.is_some()) {
            vertices.iter().map(|v| v.5.unwrap_or_default()).collect()
        } else {
            note_normals = true;
            let pos: Vec<[f32; 3]> = vertices.iter().map(|v| v.0).collect();
            computed_normals(&pos, &faces)
        };
        if note_normals {
            report.notes.push(format!("{what}: the file has no normals; they were computed from the faces"));
        }
        let texture = texture_of(g, prim.material, &what, col);
        // the matrices: the skin's where it has the bone, else the like file's, else the inverse of the rig's reference matrix
        let mut bone_mats: Vec<Mat4> = Vec::with_capacity(nb);
        for b in 0..nb {
            let m = match info.joint_bone.iter().position(|x| *x == b) {
                Some(j) => info.ibm[j],
                None => match (like_m, like) {
                    (Some(m), _) => m.bones[b],
                    (None, Like::Ani(a)) => mat_inverse(&a.bones[b].matrix),
                    _ => crate::anim::IDENTITY,
                },
            };
            bone_mats.push(m);
        }
        if let Some(h) = vertices.iter().map(|v| v.2.max(v.3)).max() {
            if usize::from(h) >= nb {
                col.add(RefusalKind::Joint, &what, format!("a vertex names bone {h}, but the like file has {nb} bones"));
                continue;
            }
        }
        report.vertices += vertices.len();
        meshes.push(C3dcMesh {
            texture,
            bones: bone_mats,
            vertices: vertices.into_iter().zip(normals).map(|((pos, weight, bone1, bone2, pad, _, uv), normal)| C3dcVertex { pos, weight, bone1, bone2, pad, normal, uv }).collect(),
            faces,
        });
    }
    if col.failed() {
        return None;
    }
    report.meshes = meshes.len();
    report.bones = max_bones;
    let layout = match like_mesh {
        Some(c) => c.layout,
        None if meshes.len() == 2 => C3dcLayout::SkinnedWithSecondMesh,
        None => C3dcLayout::Skinned,
    };
    Some(C3dc { layout, meshes })
}

// ---- static meshes ------------------------------------------------------------------------------------------------------------------

/// The world matrix (column-major, glTF space) of node `i`.
fn world(nodes: &[Node], i: usize) -> [f64; 16] {
    let mut chain = vec![i];
    while let Some(p) = nodes[*chain.last().unwrap_or(&i)].parent {
        chain.push(p);
        if chain.len() > nodes.len() {
            break;
        }
    }
    let mut w = ident();
    for &n in chain.iter().rev() {
        w = mul(&w, &local_matrix(&nodes[n].trs));
    }
    w
}

fn ident() -> [f64; 16] {
    let mut m = [0.0; 16];
    for i in 0..4 {
        m[i * 5] = 1.0;
    }
    m
}

fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut o = [0.0; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    o
}

/// `T * R * S` as a column-major matrix.
fn local_matrix(t: &Trs) -> [f64; 16] {
    let q = t.q.map(f64::from);
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let mut m = ident();
    for row in 0..3 {
        for col in 0..3 {
            m[col * 4 + row] = r[row][col] * f64::from(t.s[col]);
        }
        m[12 + row] = f64::from(t.t[row]);
    }
    m
}

fn import_static(g: &Gltf, nodes: &[Node], kind: ImportKind, col: &mut Col, report: &mut ImportReport) -> Option<Imported> {
    let mesh_nodes: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.mesh.is_some()).map(|(i, _)| i).collect();
    if mesh_nodes.len() != 1 {
        col.add(RefusalKind::Unsupported, "meshes", format!("the file has {} mesh nodes; a .3DO or .3DE holds exactly one", mesh_nodes.len()));
        return None;
    }
    let ni = mesh_nodes[0];
    let what = label(ni, &nodes[ni].name);
    if nodes[ni].skin.is_some() {
        col.add(RefusalKind::Unsupported, &what, "has a skin: a static .3DO or .3DE holds no bones (import it as a .3DC with --like)");
        return None;
    }
    let prim = read_prim(g, nodes[ni].mesh.unwrap_or(0), &what, col)?;
    let w = world(nodes, ni);
    let det = w[0] * (w[5] * w[10] - w[9] * w[6]) - w[4] * (w[1] * w[10] - w[9] * w[2]) + w[8] * (w[1] * w[6] - w[5] * w[2]);
    if det <= 1e-12 {
        col.add(RefusalKind::Transform, &what, "its placement mirrors or flattens the mesh (the determinant is not positive); apply it in the program that made the file");
        return None;
    }
    let identity = w.iter().zip(ident()).all(|(a, b)| (a - b).abs() < 1e-9);
    let apply = |p: &[f32; 3]| -> [f32; 3] {
        let p = p.map(f64::from);
        std::array::from_fn(|r| (w[r] * p[0] + w[4 + r] * p[1] + w[8 + r] * p[2] + w[12 + r]) as f32)
    };
    // normals follow the inverse transpose of the 3 x 3 (for a rotation with one scale that is the rotation)
    let inv_t = {
        let m = |r: usize, c: usize| w[c * 4 + r];
        let c = |r: usize, cc: usize| {
            let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (cc + 1) % 3, (cc + 2) % 3);
            m(r1, c1) * m(r2, c2) - m(r1, c2) * m(r2, c1)
        };
        // the cofactor matrix over the determinant is the inverse transpose
        std::array::from_fn::<[f64; 3], 3, _>(|r| std::array::from_fn(|cc| c(r, cc) / det))
    };
    let apply_n = |n: &[f32; 3]| -> [f32; 3] {
        let n = n.map(f64::from);
        let v: [f64; 3] = std::array::from_fn(|r| inv_t[r][0] * n[0] + inv_t[r][1] * n[1] + inv_t[r][2] * n[2]);
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if l < 1e-12 {
            [0.0, 1.0, 0.0]
        } else {
            [(v[0] / l) as f32, (v[1] / l) as f32, (v[2] / l) as f32]
        }
    };
    let pos: Vec<[f32; 3]> = prim.positions.iter().map(|p| if identity { *p } else { apply(p) }).map(s_point).collect();
    let faces: Vec<[u16; 3]> = prim.tris.iter().map(|t| s_tri(*t)).collect();
    let texture = texture_of(g, prim.material, &what, col);
    if col.failed() {
        return None;
    }
    if !identity {
        report.notes.push(format!("{what}: its placement in the file was applied to the vertices"));
    }
    report.meshes = 1;
    report.vertices = pos.len();
    Some(match kind {
        ImportKind::Mesh3de => Imported::Mesh3de(Mesh3de {
            texture,
            vertices: pos.iter().zip(&prim.uvs).map(|(p, uv)| Vertex3de { pos: *p, flag: -1, uv: *uv }).collect(),
            faces,
            block: None,
        }),
        _ => {
            let normals: Vec<[f32; 3]> = match &prim.normals {
                Some(n) => n.iter().map(|n| s_point(if identity { *n } else { apply_n(n) })).collect(),
                None => {
                    report.notes.push(format!("{what}: the file has no normals; they were computed from the faces"));
                    computed_normals(&pos, &faces)
                }
            };
            Imported::Mesh3do(Mesh3do {
                texture,
                vertices: pos.iter().zip(&normals).zip(&prim.uvs).map(|((p, n), uv)| Vertex3do { pos: *p, normal: *n, uv: *uv }).collect(),
                faces,
                frame_rate: 0,
                frames: Vec::new(),
            })
        }
    })
}

// ---- the entry points ---------------------------------------------------------------------------------------------------------------

/// Reads `g` as a new file of `kind`. `like` is required for `.ANI` ([`Like::Ani`]) and `.3DC` (either) and refused for the static kinds.
pub fn import_gltf(g: &Gltf, kind: ImportKind, like: Option<&Like>, opts: &ImportOptions) -> Result<(Imported, ImportReport), ImportError> {
    let mut col = Col::default();
    let mut report = ImportReport::default();
    match (kind, like) {
        (ImportKind::Ani, Some(Like::Ani(_))) | (ImportKind::C3dc, Some(_)) | (ImportKind::Mesh3do | ImportKind::Mesh3de, None) => {}
        (ImportKind::Ani, Some(Like::C3dc(_))) => return Err(ImportError::one(RefusalKind::Like, "like", "an .ANI is imported like an .ANI file")),
        (k, None) if k.needs_like() => return Err(ImportError::one(RefusalKind::Like, "like", "a .ANI or .3DC needs a like file (the file it will replace)")),
        _ => return Err(ImportError::one(RefusalKind::Like, "like", "a .3DO or .3DE takes no like file")),
    }
    let nodes = read_nodes(g, &mut col);
    if col.failed() {
        return Err(col.done());
    }
    let out = match (kind, like) {
        (ImportKind::Ani, Some(Like::Ani(a))) => import_ani(g, &nodes, a, opts, &mut col, &mut report).map(Imported::Ani),
        (ImportKind::C3dc, Some(l)) => import_c3dc(g, &nodes, l, opts, &mut col, &mut report).map(Imported::C3dc),
        _ => import_static(g, &nodes, kind, &mut col, &mut report),
    };
    match out {
        Some(o) if !col.failed() => Ok((o, report)),
        _ => Err(col.done()),
    }
}

/// The bytes of an imported model, read back by the game format's own parser. A `.3DC` must also fit the like file: the bone count of its
/// matrices equals the like file's, and no vertex names a bone the rig does not have. (The `.ANI` rig check is the caller's, so that it can
/// print what differs.)
pub fn finish(imp: &Imported, like: Option<&Like>) -> Result<Vec<u8>, ImportError> {
    let bad = |what: &str, e: crate::Error| ImportError::one(RefusalKind::Format, what.to_string(), e.to_string());
    match imp {
        Imported::Ani(a) => a.try_to_bytes().map_err(|e| bad("animation", e)),
        Imported::Mesh3do(m) => m.try_to_bytes().map_err(|e| bad("mesh", e)),
        Imported::Mesh3de(m) => m.try_to_bytes().map_err(|e| bad("mesh", e)),
        Imported::C3dc(c) => {
            let mut col = Col::default();
            for (k, m) in c.meshes.iter().enumerate() {
                let want = match like {
                    Some(Like::C3dc(l)) => l.meshes.get(k).map(|x| x.bones.len()),
                    Some(Like::Ani(a)) => Some(a.bones.len()),
                    None => None,
                };
                match want {
                    Some(n) if n == m.bones.len() => {}
                    Some(n) => col.add(RefusalKind::Like, format!("mesh {k}"), format!("{} bone matrices, the like file has {n} bones", m.bones.len())),
                    None => col.add(RefusalKind::Like, format!("mesh {k}"), "there is no like file to check the bones against"),
                }
                if let Some(h) = m.vertices.iter().map(|v| v.bone1.max(v.bone2)).max() {
                    if usize::from(h) >= m.bones.len() {
                        col.add(RefusalKind::Joint, format!("mesh {k}"), format!("a vertex names bone {h} but the mesh has {} matrices", m.bones.len()));
                    }
                }
            }
            if col.failed() {
                return Err(col.done());
            }
            c.try_to_bytes().map_err(|e| bad("mesh", e))
        }
    }
}
