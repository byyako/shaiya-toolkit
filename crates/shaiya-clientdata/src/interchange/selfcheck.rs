//! The toolkit's own check of every glTF it makes, run before anything is returned: a strict structural pass (the list the Khronos validator
//! is known to enforce for what the exporter writes: ranges, alignment, finite numbers, weights, quaternions, joints, the node forest, the
//! `.glb` container) and a semantic pass that reads the file back through [`super::gltf_read`], poses the skin by the glTF rules at ten
//! times and compares the result with `render::skin` of the source (1e-5 of the model's size).

use super::gltf_read::{component_size, Accessor, Gltf};
use super::mesh::start_key_of;
use super::{GltfInput, InterchangeError};
use crate::anim::{locals, pose, Ani};
use crate::mesh3dc::C3dc;
use crate::render::skin;
use serde_json::Value;
use std::collections::HashMap;

const MAX_ERRORS: usize = 20;

struct Errs(Vec<String>);

impl Errs {
    fn add(&mut self, m: String) {
        if self.0.len() < MAX_ERRORS {
            self.0.push(m);
        }
    }
}

fn idx(v: &Value, key: &str) -> Option<usize> {
    v.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

fn floats(v: &Value, key: &str) -> Option<Vec<f64>> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect())
}

/// Every problem of the structure of `g` (at most 20).
pub fn structure(g: &Gltf) -> Vec<String> {
    let mut e = Errs(Vec::new());
    if g.json.get("asset").and_then(|a| a.get("version")).and_then(Value::as_str) != Some("2.0") {
        e.add("asset.version is not 2.0".into());
    }
    buffers(g, &mut e);
    let accessors = accessors(g, &mut e);
    nodes(g, &mut e);
    skins(g, &accessors, &mut e);
    meshes(g, &accessors, &mut e);
    animations(g, &accessors, &mut e);
    e.0
}

fn buffers(g: &Gltf, e: &mut Errs) {
    let bufs = g.array("buffers");
    if bufs.len() != 1 {
        e.add(format!("{} buffers (the exporter writes one)", bufs.len()));
    } else if idx(&bufs[0], "byteLength") != Some(g.bin.len()) {
        e.add("buffers[0].byteLength is not the length of the buffer".into());
    }
    if !g.bin.len().is_multiple_of(4) {
        e.add("the buffer length is not a multiple of 4".into());
    }
    for (i, v) in g.array("bufferViews").iter().enumerate() {
        let (off, len) = (idx(v, "byteOffset").unwrap_or(0), idx(v, "byteLength").unwrap_or(0));
        if idx(v, "buffer") != Some(0) {
            e.add(format!("bufferView {i} does not use buffer 0"));
        }
        if len == 0 || off.checked_add(len).is_none_or(|end| end > g.bin.len()) {
            e.add(format!("bufferView {i} is empty or runs past the buffer"));
        }
        if !off.is_multiple_of(4) {
            e.add(format!("bufferView {i} does not start on a multiple of 4"));
        }
    }
}

fn accessors(g: &Gltf, e: &mut Errs) -> Vec<Option<Accessor>> {
    let mut out = Vec::new();
    for (i, a) in g.array("accessors").iter().enumerate() {
        let acc = match g.accessor(i) {
            Ok(acc) => acc,
            Err(m) => {
                e.add(m);
                out.push(None);
                continue;
            }
        };
        let csize = component_size(u64::from(acc.component_type)).unwrap_or(1);
        if !idx(a, "byteOffset").unwrap_or(0).is_multiple_of(csize) {
            e.add(format!("accessor {i}: byteOffset is not a multiple of the component size"));
        }
        if acc.component_type == 5126 {
            if let Some(k) = acc.data.iter().position(|x| !x.is_finite()) {
                e.add(format!("accessor {i}: element {} is not finite", k / acc.width));
            }
            for (key, pick) in [("min", true), ("max", false)] {
                if let Some(listed) = floats(a, key) {
                    for (c, want) in listed.iter().enumerate() {
                        let got = acc.data.iter().skip(c).step_by(acc.width).map(|x| *x as f32).fold(if pick { f32::INFINITY } else { f32::NEG_INFINITY }, |m, x| if pick { m.min(x) } else { m.max(x) });
                        if got != *want as f32 {
                            e.add(format!("accessor {i}: {key}[{c}] is {want} but the data says {got}"));
                        }
                    }
                }
            }
        }
        out.push(Some(acc));
    }
    out
}

fn nodes(g: &Gltf, e: &mut Errs) {
    let nodes = g.array("nodes");
    let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        for c in n.get("children").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64) {
            let c = c as usize;
            if c >= nodes.len() {
                e.add(format!("node {i} has the child {c}, which does not exist"));
            } else if parent[c].replace(i).is_some() {
                e.add(format!("node {c} has two parents"));
            }
        }
        if let Some(q) = floats(n, "rotation") {
            let len = q.iter().map(|x| x * x).sum::<f64>().sqrt();
            if q.len() != 4 || (len - 1.0).abs() > 1e-5 {
                e.add(format!("node {i}: the rotation is not a unit quaternion"));
            }
        }
        for key in ["translation", "scale"] {
            if floats(n, key).is_some_and(|v| v.iter().any(|x| !x.is_finite())) {
                e.add(format!("node {i}: {key} is not finite"));
            }
        }
    }
    let scene = idx(&g.json, "scene").unwrap_or(0);
    let roots: Vec<usize> = g.array("scenes").get(scene).and_then(|s| s.get("nodes")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default();
    let mut seen = vec![false; nodes.len()];
    let mut stack = roots.clone();
    for r in &roots {
        if *r >= nodes.len() {
            e.add(format!("the scene names node {r}, which does not exist"));
        } else if parent[*r].is_some() {
            e.add(format!("the scene names node {r}, which has a parent"));
        }
    }
    while let Some(n) = stack.pop() {
        if n >= nodes.len() {
            continue;
        }
        if std::mem::replace(&mut seen[n], true) {
            e.add(format!("node {n} is reached twice (a cycle or a repeated child)"));
            break;
        }
        stack.extend(nodes[n].get("children").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|c| c as usize));
    }
    if let Some(n) = seen.iter().position(|s| !s) {
        e.add(format!("node {n} is not in the scene"));
    }
}

fn skins(g: &Gltf, acc: &[Option<Accessor>], e: &mut Errs) {
    let nodes = g.array("nodes").len();
    for (i, s) in g.array("skins").iter().enumerate() {
        let joints: Vec<usize> = s.get("joints").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default();
        let mut sorted = joints.clone();
        sorted.sort_unstable();
        sorted.dedup();
        if joints.is_empty() || sorted.len() != joints.len() || joints.iter().any(|j| *j >= nodes) {
            e.add(format!("skin {i}: the joints are empty, repeated or not nodes"));
        }
        match idx(s, "inverseBindMatrices").and_then(|a| acc.get(a)).and_then(Option::as_ref) {
            Some(a) if a.width == 16 && a.component_type == 5126 && a.count == joints.len() => {
                for (k, m) in a.data.chunks(16).enumerate() {
                    if m[3].abs() > 1e-6 || m[7].abs() > 1e-6 || m[11].abs() > 1e-6 || (m[15] - 1.0).abs() > 1e-6 {
                        e.add(format!("skin {i}: the matrix of joint {k} is not affine (its last row is not 0 0 0 1)"));
                    }
                }
            }
            _ => e.add(format!("skin {i}: inverseBindMatrices is missing, not MAT4 floats, or not one per joint")),
        }
    }
}

fn meshes(g: &Gltf, acc: &[Option<Accessor>], e: &mut Errs) {
    // the joint count of the skin of the node that uses each mesh
    let mut joints_of_mesh: HashMap<usize, usize> = HashMap::new();
    for n in g.array("nodes") {
        if let (Some(m), Some(s)) = (idx(n, "mesh"), idx(n, "skin")) {
            if let Some(j) = g.array("skins").get(s).and_then(|s| s.get("joints")).and_then(Value::as_array) {
                joints_of_mesh.insert(m, j.len());
            }
        }
    }
    let get = |i: Option<usize>| i.and_then(|i| acc.get(i)).and_then(Option::as_ref);
    for (mi, m) in g.array("meshes").iter().enumerate() {
        for p in m.get("primitives").and_then(Value::as_array).into_iter().flatten() {
            if p.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
                e.add(format!("mesh {mi}: the primitive mode is not triangles"));
            }
            let attrs = p.get("attributes").and_then(Value::as_object);
            let attr = |k: &str| attrs.and_then(|a| a.get(k)).and_then(Value::as_u64).map(|x| x as usize);
            let Some(pos) = get(attr("POSITION")) else {
                e.add(format!("mesh {mi}: POSITION is missing"));
                continue;
            };
            let pos_json = attr("POSITION").and_then(|i| g.array("accessors").get(i));
            if pos_json.is_none_or(|a| a.get("min").is_none() || a.get("max").is_none()) {
                e.add(format!("mesh {mi}: POSITION has no min and max"));
            }
            for (k, v) in attrs.into_iter().flatten() {
                let a = get(v.as_u64().map(|x| x as usize));
                if a.is_none_or(|a| a.count != pos.count) {
                    e.add(format!("mesh {mi}: {k} does not have one element per vertex"));
                }
            }
            match get(idx(p, "indices")) {
                Some(ix) if ix.count % 3 == 0 && ix.width == 1 => {
                    if let Some(bad) = ix.data.iter().find(|v| **v >= pos.count as f64) {
                        e.add(format!("mesh {mi}: an index ({bad}) is not below the vertex count {}", pos.count));
                    }
                }
                _ => e.add(format!("mesh {mi}: indices are missing or not a multiple of 3")),
            }
            if idx(p, "material").is_none_or(|m| m >= g.array("materials").len()) {
                e.add(format!("mesh {mi}: the material does not exist"));
            }
            if let (Some(j), Some(w)) = (get(attr("JOINTS_0")), get(attr("WEIGHTS_0"))) {
                let njoints = joints_of_mesh.get(&mi).copied().unwrap_or(0);
                for v in 0..j.count.min(w.count) {
                    let (jv, wv) = (&j.data[v * 4..v * 4 + 4], &w.data[v * 4..v * 4 + 4]);
                    if jv.iter().any(|x| *x >= njoints as f64) {
                        e.add(format!("mesh {mi}: vertex {v} names a joint outside the skin ({njoints} joints)"));
                        break;
                    }
                    let sum: f64 = wv.iter().sum();
                    if wv.iter().any(|x| *x < 0.0) || (sum - 1.0).abs() > 1e-5 {
                        e.add(format!("mesh {mi}: vertex {v} has weights that are negative or do not add up to 1 ({sum})"));
                        break;
                    }
                    if (0..4).any(|a| (a + 1..4).any(|b| jv[a] == jv[b] && wv[a] > 0.0 && wv[b] > 0.0)) {
                        e.add(format!("mesh {mi}: vertex {v} names a joint twice with weight"));
                        break;
                    }
                }
            } else if attr("JOINTS_0").is_some() != attr("WEIGHTS_0").is_some() {
                e.add(format!("mesh {mi}: JOINTS_0 and WEIGHTS_0 come together"));
            }
        }
    }
}

fn animations(g: &Gltf, acc: &[Option<Accessor>], e: &mut Errs) {
    let nodes = g.array("nodes").len();
    let get = |i: Option<usize>| i.and_then(|i| acc.get(i)).and_then(Option::as_ref);
    for (ai, a) in g.array("animations").iter().enumerate() {
        let samplers = a.get("samplers").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for (si, s) in samplers.iter().enumerate() {
            let (Some(input), Some(out)) = (get(idx(s, "input")), get(idx(s, "output"))) else {
                e.add(format!("animation {ai}: sampler {si} names an accessor that does not exist"));
                continue;
            };
            if input.width != 1 || input.component_type != 5126 || input.data.is_empty() || input.data[0] < 0.0 || input.data.windows(2).any(|w| w[1] <= w[0]) {
                e.add(format!("animation {ai}: sampler {si}: the times are not float, non-negative and strictly increasing"));
            }
            if out.count != input.count {
                e.add(format!("animation {ai}: sampler {si}: {} outputs for {} times", out.count, input.count));
            }
            if s.get("interpolation").and_then(Value::as_str).unwrap_or("LINEAR") != "LINEAR" {
                e.add(format!("animation {ai}: sampler {si} is not LINEAR"));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for c in a.get("channels").and_then(Value::as_array).into_iter().flatten() {
            let (Some(node), Some(path)) = (c.get("target").and_then(|t| idx(t, "node")), c.get("target").and_then(|t| t.get("path")).and_then(Value::as_str)) else {
                e.add(format!("animation {ai}: a channel has no target"));
                continue;
            };
            if node >= nodes {
                e.add(format!("animation {ai}: a channel targets node {node}, which does not exist"));
            }
            if !seen.insert((node, path.to_string())) {
                e.add(format!("animation {ai}: node {node} has two {path} channels"));
            }
            let Some(s) = idx(c, "sampler").and_then(|s| samplers.get(s)) else {
                e.add(format!("animation {ai}: a channel names a sampler that does not exist"));
                continue;
            };
            let Some(out) = get(idx(s, "output")) else { continue };
            let want = match path {
                "rotation" => 4,
                "translation" | "scale" => 3,
                _ => {
                    e.add(format!("animation {ai}: the path {path} is not written by the exporter"));
                    continue;
                }
            };
            if out.width != want || out.component_type != 5126 {
                e.add(format!("animation {ai}: node {node}: {path} output is not float x {want}"));
            } else if path == "rotation" && out.data.chunks(4).any(|q| (q.iter().map(|x| x * x).sum::<f64>().sqrt() - 1.0).abs() > 1e-5) {
                e.add(format!("animation {ai}: node {node}: a rotation key is not a unit quaternion"));
            }
        }
    }
}

// ---- the semantic pass --------------------------------------------------------------------------------------------------------------

type M4 = [[f64; 4]; 4];

/// One pose to compare: (the glTF animation and its time in seconds, the game animation and its key); `None` is the rest pose.
type Case = (Option<(usize, f64)>, Option<(usize, f64)>);

fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [[0.0; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            o[r][c] = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

/// `T * R * S` as a column-vector matrix.
fn from_trs(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> M4 {
    let [x, y, z, w] = q;
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let mut m = [[0.0; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = r[i][j] * s[j];
        }
        m[i][3] = t[i];
    }
    m[3][3] = 1.0;
    m
}

fn slerp(a: &[f64], b: &[f64], f: f64) -> Vec<f64> {
    let mut dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    dot = dot.abs().min(1.0);
    let angle = dot.acos();
    let (wa, wb) = if angle < 1e-6 { (1.0 - f, f) } else { (((1.0 - f) * angle).sin() / angle.sin(), (f * angle).sin() / angle.sin()) };
    let q: Vec<f64> = a.iter().zip(b).map(|(x, y)| x * wa + y * sign * wb).collect();
    let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    q.iter().map(|x| x / n).collect()
}

/// The value of a LINEAR channel at `t` seconds (held before the first and after the last key).
fn sample(times: &[f64], vals: &[f64], width: usize, t: f64, rotation: bool) -> Vec<f64> {
    let n = times.len();
    let at = |k: usize| &vals[k * width..(k + 1) * width];
    if t <= times[0] {
        return at(0).to_vec();
    }
    if t >= times[n - 1] {
        return at(n - 1).to_vec();
    }
    let k = times.partition_point(|x| *x <= t) - 1;
    let f = (t - times[k]) / (times[k + 1] - times[k]);
    if rotation {
        slerp(at(k), at(k + 1), f)
    } else {
        at(k).iter().zip(at(k + 1)).map(|(a, b)| a + (b - a) * f).collect()
    }
}

struct Scene3<'a> {
    g: &'a Gltf,
    parent: Vec<Option<usize>>,
}

impl<'a> Scene3<'a> {
    fn new(g: &'a Gltf) -> Scene3<'a> {
        let nodes = g.array("nodes");
        let mut parent = vec![None; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            for c in n.get("children").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64) {
                if let Some(p) = parent.get_mut(c as usize) {
                    *p = Some(i);
                }
            }
        }
        Scene3 { g, parent }
    }

    /// Global matrices of every node; `anim` is an animation index, `t` seconds (`None` = rest).
    fn worlds(&self, anim: Option<(usize, f64)>, acc: &[Accessor]) -> Result<Vec<M4>, String> {
        let nodes = self.g.array("nodes");
        let mut trs: Vec<([f64; 3], [f64; 4], [f64; 3])> = nodes
            .iter()
            .map(|n| {
                let t = floats(n, "translation").filter(|v| v.len() == 3).map_or([0.0; 3], |v| [v[0], v[1], v[2]]);
                let q = floats(n, "rotation").filter(|v| v.len() == 4).map_or([0.0, 0.0, 0.0, 1.0], |v| [v[0], v[1], v[2], v[3]]);
                let s = floats(n, "scale").filter(|v| v.len() == 3).map_or([1.0; 3], |v| [v[0], v[1], v[2]]);
                (t, q, s)
            })
            .collect();
        if let Some((ai, t)) = anim {
            let a = self.g.array("animations").get(ai).ok_or("the animation does not exist")?;
            let samplers = a.get("samplers").and_then(Value::as_array).ok_or("no samplers")?;
            for c in a.get("channels").and_then(Value::as_array).into_iter().flatten() {
                let (Some(node), Some(path), Some(s)) = (c.get("target").and_then(|t| idx(t, "node")), c.get("target").and_then(|t| t.get("path")).and_then(Value::as_str), idx(c, "sampler").and_then(|s| samplers.get(s))) else {
                    return Err("a channel is incomplete".into());
                };
                let (input, out) = (acc.get(idx(s, "input").ok_or("no input")?).ok_or("bad input")?, acc.get(idx(s, "output").ok_or("no output")?).ok_or("bad output")?);
                let v = sample(&input.data, &out.data, out.width, t, path == "rotation");
                let slot = trs.get_mut(node).ok_or("a channel targets a missing node")?;
                match path {
                    "rotation" => slot.1 = [v[0], v[1], v[2], v[3]],
                    "translation" => slot.0 = [v[0], v[1], v[2]],
                    _ => slot.2 = [v[0], v[1], v[2]],
                }
            }
        }
        let mut world: Vec<Option<M4>> = vec![None; nodes.len()];
        for i in 0..nodes.len() {
            self.world_of(i, &trs, &mut world, 0)?;
        }
        Ok(world.into_iter().flatten().collect())
    }

    fn world_of(&self, i: usize, trs: &[([f64; 3], [f64; 4], [f64; 3])], memo: &mut Vec<Option<M4>>, depth: usize) -> Result<M4, String> {
        if let Some(m) = memo[i] {
            return Ok(m);
        }
        if depth > trs.len() {
            return Err("the node graph has a cycle".into());
        }
        let local = from_trs(trs[i].0, trs[i].1, trs[i].2);
        let w = match self.parent[i] {
            Some(p) => mul(&self.world_of(p, trs, memo, depth + 1)?, &local),
            None => local,
        };
        memo[i] = Some(w);
        Ok(w)
    }
}

fn all_accessors(g: &Gltf) -> Result<Vec<Accessor>, String> {
    (0..g.array("accessors").len()).map(|i| g.accessor(i)).collect()
}

fn model_size(c: &C3dc) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for v in c.meshes.iter().flat_map(|m| &m.vertices) {
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(v.pos[k]));
            hi[k] = hi[k].max(f64::from(v.pos[k]));
        }
    }
    let d: f64 = (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
    if d.is_finite() && d > 1e-6 { d } else { 1.0 }
}

/// The key range sampled for an animation: ten keys from the start to the later of the end key and the last key.
fn sample_keys(a: &Ani) -> Vec<f64> {
    let start = start_key_of(a);
    let end = a.end_key.max(a.last_key().unwrap_or(start)).max(start);
    (0..10).map(|j| f64::from(start) + f64::from(end - start) * (j as f64) / 9.0).collect()
}

/// Poses the skins of `c3dc` as glTF says (global joint matrix times the inverse bind matrix) at rest and, for each animation, at ten
/// times, and compares with `render::skin` of the source.
fn skinned_pose(g: &Gltf, c3dc: &C3dc, anims: &[Ani]) -> Result<(), String> {
    let acc = all_accessors(g)?;
    let sc = Scene3::new(g);
    let tol = 1e-5 * model_size(c3dc);
    // which node holds mesh k
    let mesh_node: Vec<usize> = (0..c3dc.meshes.len()).map(|k| g.array("nodes").iter().position(|n| idx(n, "mesh") == Some(k)).ok_or_else(|| format!("no node holds mesh {k}"))).collect::<Result<_, _>>()?;
    let mut cases: Vec<Case> = vec![(None, None)];
    for (ai, a) in anims.iter().enumerate() {
        let start = f64::from(start_key_of(a));
        for key in sample_keys(a) {
            cases.push((Some((ai, (key - start) / 30.0)), Some((ai, key))));
        }
    }
    // the exporter takes the rest placement of a bone from the first mesh that has it, so a mesh whose own matrices differ from those
    // has its own skin and is not expected at its source positions at rest (every animated pose is still compared exactly)
    let rest_ok: Vec<bool> = c3dc
        .meshes
        .iter()
        .map(|mesh| {
            mesh.bones.iter().enumerate().all(|(i, b)| {
                let owner = c3dc.meshes.iter().find(|m| m.bones.len() > i).map_or(b, |m| &m.bones[i]);
                b.iter().zip(owner).all(|(x, y)| x.to_bits() == y.to_bits())
            })
        })
        .collect();
    for (gl, game) in cases {
        let worlds = sc.worlds(gl, &acc)?;
        for (k, mesh) in c3dc.meshes.iter().enumerate() {
            if game.is_none() && !rest_ok[k] {
                continue;
            }
            let node = &g.array("nodes")[mesh_node[k]];
            let skin_i = idx(node, "skin").ok_or("a skinned mesh node has no skin")?;
            let skin_j = &g.array("skins")[skin_i];
            let joints: Vec<usize> = skin_j.get("joints").and_then(Value::as_array).ok_or("no joints")?.iter().filter_map(Value::as_u64).map(|x| x as usize).collect();
            let ibm = &acc[idx(skin_j, "inverseBindMatrices").ok_or("no inverseBindMatrices")?];
            let prim = &g.array("meshes")[k].get("primitives").and_then(Value::as_array).and_then(|p| p.first()).ok_or("no primitive")?;
            let attr = |n: &str| prim.get("attributes").and_then(|a| idx(a, n)).and_then(|i| acc.get(i)).ok_or_else(|| format!("{n} is missing"));
            let (pos, jn, wt) = (attr("POSITION")?, attr("JOINTS_0")?, attr("WEIGHTS_0")?);
            let jm: Vec<M4> = joints
                .iter()
                .enumerate()
                .map(|(j, &n)| {
                    let mut ib = [[0.0; 4]; 4];
                    for (c, col) in ibm.data[j * 16..j * 16 + 16].chunks(4).enumerate() {
                        for (r, v) in col.iter().enumerate() {
                            ib[r][c] = *v;
                        }
                    }
                    mul(&worlds[n], &ib)
                })
                .collect();
            let expected: Vec<[f32; 3]> = match game {
                None => mesh.vertices.iter().map(|v| v.pos).collect(),
                Some((ai, key)) => {
                    let a = &anims[ai];
                    skin(mesh, &pose(a, &locals(a), key as f32)).0
                }
            };
            for (v, want) in expected.iter().enumerate() {
                let p = [pos.data[v * 3], pos.data[v * 3 + 1], pos.data[v * 3 + 2], 1.0];
                let mut out = [0.0f64; 3];
                for s in 0..4 {
                    let (j, w) = (jn.data[v * 4 + s] as usize, wt.data[v * 4 + s]);
                    if w == 0.0 {
                        continue;
                    }
                    for r in 0..3 {
                        out[r] += w * (0..4).map(|c| jm[j][r][c] * p[c]).sum::<f64>();
                    }
                }
                let got = [out[0], out[1], -out[2]];
                if (0..3).any(|r| (got[r] - f64::from(want[r])).abs() > tol) {
                    return Err(format!("mesh {k}, vertex {v}: the glTF skin poses it at {got:?}, the toolkit's renderer at {want:?} ({})", game.map_or("at rest".to_string(), |(a, key)| format!("animation {a}, key {key}"))));
                }
            }
        }
    }
    Ok(())
}

/// An animation alone: the world matrix of every node against the game's `World`, mirrored.
fn rig_pose(g: &Gltf, a: &Ani) -> Result<(), String> {
    let acc = all_accessors(g)?;
    let sc = Scene3::new(g);
    let locs = locals(a);
    let start = f64::from(start_key_of(a));
    for key in sample_keys(a) {
        let want = pose(a, &locs, key as f32);
        let worlds = sc.worlds(Some((0, (key - start) / 30.0)), &acc)?;
        let size = want.iter().flat_map(|m| m[12..15].iter()).fold(1.0f32, |m, x| m.max(x.abs())) as f64;
        for (i, w) in want.iter().enumerate() {
            let m = super::xform::s_matrix(w);
            for r in 0..4 {
                for c in 0..4 {
                    let got = worlds[i][r][c];
                    if (got - f64::from(m[c * 4 + r])).abs() > 1e-5 * size.max(1.0) {
                        return Err(format!("bone {i}, key {key}: the glTF world matrix differs from the toolkit's pose at [{r}][{c}] ({got} against {})", m[c * 4 + r]));
                    }
                }
            }
        }
    }
    Ok(())
}

/// True for the inputs [`semantic`] poses (a skinned `.3DC` and an animation alone).
pub fn has_pose_check(input: &GltfInput) -> bool {
    match input {
        GltfInput::Mesh { c3dc, .. } => c3dc.layout != crate::mesh3dc::C3dcLayout::Cloak,
        GltfInput::Anim(_) => true,
        _ => false,
    }
}

/// The semantic pass for `input`.
pub fn semantic(g: &Gltf, input: &GltfInput) -> Result<(), String> {
    match input {
        GltfInput::Mesh { c3dc, anims } if c3dc.layout != crate::mesh3dc::C3dcLayout::Cloak => skinned_pose(g, c3dc, anims),
        GltfInput::Anim(a) => rig_pose(g, a),
        _ => Ok(()),
    }
}

/// The check `export_gltf` runs: the container, the structure and (when nothing was projected) the pose.
pub(crate) fn check_output(json: &[u8], bin: &[u8], glb: Option<&[u8]>, input: &GltfInput, with_pose: bool) -> Result<(), InterchangeError> {
    let bug = |m: String| InterchangeError::SelfCheck(m);
    let g = Gltf::from_parts(json, bin).map_err(bug)?;
    if let Some(glb) = glb {
        let back = Gltf::from_glb(glb).map_err(bug)?;
        if back.json != g.json || back.bin != g.bin {
            return Err(InterchangeError::SelfCheck("the .glb does not hold the same JSON and buffer as the .gltf".into()));
        }
    }
    let problems = structure(&g);
    if !problems.is_empty() {
        return Err(InterchangeError::SelfCheck(problems.join("; ")));
    }
    if with_pose {
        semantic(&g, input).map_err(InterchangeError::SelfCheck)?;
    }
    Ok(())
}
