//! Lays a [`Scene`] out as glTF 2.0: JSON plus one binary buffer (a `.gltf` and its `.bin`, or one `.glb` holding the same buffer).
//!
//! One node per bone in file order (`Bone_NNN`), the mesh nodes after them at the scene's top with an identity transform, one primitive per
//! mesh, a material named with the texture text (no `images` unless asked), and per clip one channel per bone and path with LINEAR samplers.

use super::mesh::{Clip, Mesh3d, Scene};
use super::xform::Trs;
use super::{GltfOptions, GLTF_FORMAT};
use crate::error::decode_cp949;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

const FLOAT: u32 = 5126;
const UBYTE: u32 = 5121;
const USHORT: u32 = 5123;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

pub(crate) struct Built {
    pub json: Vec<u8>,
    pub bin: Vec<u8>,
    pub glb: Option<Vec<u8>>,
}

/// A float as the shortest JSON number that reads back to the same `f32`.
pub(crate) fn num(x: f32) -> Value {
    let v: f64 = x.to_string().parse().unwrap_or(0.0);
    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
}

fn nums(v: &[f32]) -> Value {
    Value::Array(v.iter().map(|x| num(*x)).collect())
}

struct Buf {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Buf {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.view_strided(bytes, target, None)
    }

    fn view_strided(&mut self, bytes: &[u8], target: Option<u32>, stride: Option<usize>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        if let Some(s) = stride {
            v["byteStride"] = json!(s);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, bytes: &[u8], target: Option<u32>, comp: u32, count: usize, ty: &str, min_max: Option<(Vec<f32>, Vec<f32>)>) -> usize {
        let view = self.view(bytes, target);
        let mut a = json!({"bufferView": view, "componentType": comp, "count": count, "type": ty});
        if let Some((min, max)) = min_max {
            a["min"] = nums(&min);
            a["max"] = nums(&max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats(&mut self, data: &[f32], target: Option<u32>, ty: &str, width: usize, min_max: bool) -> usize {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for c in data {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        let count = data.len() / width;
        let mm = min_max.then(|| {
            let mut min = vec![f32::INFINITY; width];
            let mut max = vec![f32::NEG_INFINITY; width];
            for e in data.chunks(width) {
                for k in 0..width {
                    min[k] = min[k].min(e[k]);
                    max[k] = max[k].max(e[k]);
                }
            }
            (min, max)
        });
        self.accessor(&bytes, target, FLOAT, count, ty, mm)
    }
}

fn flat3(v: &[[f32; 3]]) -> Vec<f32> {
    v.iter().flatten().copied().collect()
}

/// The ASCII hex of raw bytes (lower case).
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The texture name: cp949 text up to the first NUL.
fn texture_text(raw: &[u8]) -> String {
    decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[]))
}

fn uri_escape(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn trs_json(node: &mut Value, t: &Trs) {
    if t.t.iter().any(|c| *c != 0.0) {
        node["translation"] = nums(&t.t);
    }
    if t.q != [0.0, 0.0, 0.0, 1.0] {
        node["rotation"] = nums(&t.q);
    }
    if t.s.iter().any(|s| (s - 1.0).abs() > 1e-6) {
        node["scale"] = nums(&t.s);
    }
}

fn bone_name(i: usize, width: usize) -> String {
    format!("Bone_{i:0width$}")
}

/// Time accessors already written, keyed by the clip's start key and the key list: the times are `(key - start) / 30`, so two clips with
/// the same keys and different starts need their own accessor.
type TimeCache = HashMap<(i32, Vec<i32>), usize>;

fn time_accessor(b: &mut Buf, cache: &mut TimeCache, start: i32, keys: &[i32]) -> usize {
    if let Some(&a) = cache.get(&(start, keys.to_vec())) {
        return a;
    }
    let times: Vec<f32> = keys.iter().map(|k| (i64::from(*k) - i64::from(start)) as f32 / 30.0).collect();
    let a = b.floats(&times, None, "SCALAR", 1, true);
    cache.insert((start, keys.to_vec()), a);
    a
}

fn primitive(b: &mut Buf, m: &Mesh3d, material: usize) -> Value {
    let mut attrs = Map::new();
    attrs.insert("POSITION".into(), json!(b.floats(&flat3(&m.positions), Some(ARRAY_BUFFER), "VEC3", 3, true)));
    if let Some(n) = &m.normals {
        attrs.insert("NORMAL".into(), json!(b.floats(&flat3(n), Some(ARRAY_BUFFER), "VEC3", 3, false)));
    }
    let uv: Vec<f32> = m.uvs.iter().flatten().copied().collect();
    attrs.insert("TEXCOORD_0".into(), json!(b.floats(&uv, Some(ARRAY_BUFFER), "VEC2", 2, false)));
    if let Some(s) = &m.skin {
        let joints: Vec<u8> = s.joints.iter().flat_map(|j| [j[0], j[1], 0, 0]).collect();
        attrs.insert("JOINTS_0".into(), json!(b.accessor(&joints, Some(ARRAY_BUFFER), UBYTE, s.joints.len(), "VEC4", None)));
        let weights: Vec<f32> = s.weights.iter().flat_map(|w| [w[0], w[1], 0.0, 0.0]).collect();
        attrs.insert("WEIGHTS_0".into(), json!(b.floats(&weights, Some(ARRAY_BUFFER), "VEC4", 4, false)));
        // a vertex attribute's elements start on 4-byte boundaries: the 16-bit values sit in a view with a stride of 4
        let pad: Vec<u8> = s.pad.iter().flat_map(|p| [p.to_le_bytes()[0], p.to_le_bytes()[1], 0, 0]).collect();
        let view = b.view_strided(&pad, Some(ARRAY_BUFFER), Some(4));
        b.accessors.push(json!({"bufferView": view, "componentType": USHORT, "count": s.pad.len(), "type": "SCALAR"}));
        attrs.insert("_SHAIYA_PAD".into(), json!(b.accessors.len() - 1));
    }
    let idx: Vec<u8> = m.triangles.iter().flatten().flat_map(|i| i.to_le_bytes()).collect();
    let indices = b.accessor(&idx, Some(ELEMENT_ARRAY_BUFFER), USHORT, m.triangles.len() * 3, "SCALAR", None);
    json!({"attributes": attrs, "indices": indices, "material": material, "mode": 4})
}

fn animation(b: &mut Buf, cache: &mut TimeCache, clip: &Clip) -> Value {
    let mut samplers = Vec::new();
    let mut channels = Vec::new();
    for (i, bone) in clip.bones.iter().enumerate() {
        let mut add = |b: &mut Buf, keys: Vec<i32>, out: usize, path: &str| {
            let input = time_accessor(b, cache, clip.start_key, &keys);
            samplers.push(json!({"input": input, "output": out, "interpolation": "LINEAR"}));
            channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": i, "path": path}}));
        };
        if !bone.rot.is_empty() {
            let data: Vec<f32> = bone.rot.iter().flat_map(|(_, q)| *q).collect();
            let out = b.floats(&data, None, "VEC4", 4, false);
            add(b, bone.rot.iter().map(|(k, _)| *k).collect(), out, "rotation");
        }
        if !bone.trans.is_empty() {
            let data: Vec<f32> = bone.trans.iter().flat_map(|(_, v)| *v).collect();
            let out = b.floats(&data, None, "VEC3", 3, false);
            add(b, bone.trans.iter().map(|(k, _)| *k).collect(), out, "translation");
        }
        if let Some(s) = bone.scale {
            let out = b.floats(&s, None, "VEC3", 3, false);
            add(b, vec![clip.start_key], out, "scale");
        }
    }
    json!({
        "name": clip.name,
        "samplers": samplers,
        "channels": channels,
        "extras": {"shaiya": {"start_key": clip.start_key, "end_key": clip.end_key, "first_field": clip.first_field}},
    })
}

/// The GLB container: header, the JSON chunk padded with spaces, the BIN chunk padded with zeros.
fn glb_of(json: &[u8], bin: &[u8]) -> Vec<u8> {
    let mut j = json.to_vec();
    while !j.len().is_multiple_of(4) {
        j.push(b' ');
    }
    let mut b = bin.to_vec();
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    let total = 12 + 8 + j.len() + 8 + b.len();
    let mut o = Vec::with_capacity(total);
    o.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    o.extend_from_slice(&2u32.to_le_bytes());
    o.extend_from_slice(&(total as u32).to_le_bytes());
    o.extend_from_slice(&(j.len() as u32).to_le_bytes());
    o.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    o.extend_from_slice(&j);
    o.extend_from_slice(&(b.len() as u32).to_le_bytes());
    o.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    o.extend_from_slice(&b);
    o
}

pub(crate) fn write(scene: &Scene, opts: &GltfOptions, _input: &super::GltfInput) -> Built {
    let mut b = Buf { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut nodes: Vec<Value> = Vec::new();
    let mut scene_nodes: Vec<usize> = Vec::new();
    let mut skins: Vec<Value> = Vec::new();
    let mut skin_bones = 0usize;
    // bones
    if let Some(rig) = &scene.rig {
        let n = rig.parents.len();
        let width = format!("{}", n.saturating_sub(1)).len().max(3);
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, &p) in rig.parents.iter().enumerate() {
            match usize::try_from(p) {
                Ok(p) => children[p].push(i),
                Err(_) => scene_nodes.push(i),
            }
        }
        for (i, kids) in children.iter().enumerate().take(n) {
            let mut node = json!({"name": bone_name(i, width)});
            trs_json(&mut node, &rig.rest[i]);
            if !kids.is_empty() {
                node["children"] = json!(kids);
            }
            nodes.push(node);
        }
        skin_bones = n;
        // a skin's joints need one common root: with 0 or 2+ roots among the bones, a node `Shaiya_Root` (identity, last in the node list so
        // bone numbers stay node numbers) holds them
        if scene.meshes.iter().any(|m| m.skin.is_some()) && scene_nodes.len() != 1 {
            nodes.push(json!({"name": "Shaiya_Root", "children": scene_nodes}));
            scene_nodes = vec![nodes.len() - 1];
        }
    }
    let roots = scene_nodes.len();
    // materials, meshes, skins
    let mut materials = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut textures: Vec<Value> = Vec::new();
    let mut meshes = Vec::new();
    let mut skin_of_mesh: Vec<Option<usize>> = Vec::new();
    let mut skin_ibm: Vec<(usize, Vec<[f32; 16]>)> = Vec::new();
    for m in &scene.meshes {
        let name = texture_text(&m.texture);
        let mut pbr = json!({"metallicFactor": 0, "roughnessFactor": 1});
        if let (Some(base), false) = (&opts.texture_uri_base, name.is_empty()) {
            images.push(json!({"uri": format!("{}/{}.png", uri_escape(base.trim_end_matches('/')), uri_escape(&name))}));
            textures.push(json!({"source": images.len() - 1}));
            pbr["baseColorTexture"] = json!({"index": textures.len() - 1});
        }
        let mut mat = json!({"pbrMetallicRoughness": pbr, "extras": {"shaiya": {"texture_hex": hex(&m.texture)}}});
        if !name.is_empty() {
            mat["name"] = json!(name);
        }
        materials.push(mat);
        let material = materials.len() - 1;
        let prim = primitive(&mut b, m, material);
        meshes.push(json!({"name": m.name, "primitives": [prim]}));
        if let Some(sk) = &m.skin {
            let same = skin_ibm.iter().position(|(_, ibm)| ibm.len() == sk.ibm.len() && ibm.iter().flatten().zip(sk.ibm.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()));
            let which = match same {
                Some(k) => k,
                None => {
                    let data: Vec<f32> = sk.ibm.iter().flatten().copied().collect();
                    let acc = b.floats(&data, None, "MAT4", 16, false);
                    let joints: Vec<usize> = (0..sk.ibm.len()).collect();
                    let mut s = json!({"joints": joints, "inverseBindMatrices": acc});
                    if skin_bones > 0 && roots == 1 {
                        s["skeleton"] = json!(scene_nodes[0]);
                    }
                    skins.push(s);
                    skin_ibm.push((skins.len() - 1, sk.ibm.clone()));
                    skins.len() - 1
                }
            };
            skin_of_mesh.push(Some(which));
        } else {
            skin_of_mesh.push(None);
        }
    }
    for (i, m) in scene.meshes.iter().enumerate() {
        let mut node = json!({"name": m.name, "mesh": i});
        if let Some(s) = skin_of_mesh[i] {
            node["skin"] = json!(s);
        }
        nodes.push(node);
        scene_nodes.push(nodes.len() - 1);
    }
    // animations
    let mut cache = HashMap::new();
    let animations: Vec<Value> = scene.clips.iter().map(|c| animation(&mut b, &mut cache, c)).collect();
    while !b.bin.len().is_multiple_of(4) {
        b.bin.push(0);
    }
    let mut shaiya = json!({"format": GLTF_FORMAT, "source": opts.source, "layout": scene.layout, "toolkit": env!("CARGO_PKG_VERSION")});
    if let Some(c) = scene.clips.first() {
        shaiya["start_key"] = json!(c.start_key);
        shaiya["end_key"] = json!(c.end_key);
        shaiya["first_field"] = json!(c.first_field);
    }
    let mut buffer = json!({"byteLength": b.bin.len()});
    if !opts.binary {
        buffer["uri"] = json!(uri_escape(&opts.bin_name));
    }
    let mut root = json!({
        "asset": {"version": "2.0", "generator": format!("shaiya-toolkit {}", env!("CARGO_PKG_VERSION")), "extras": {"shaiya": shaiya}},
        "scene": 0,
        "scenes": [{"nodes": scene_nodes}],
        "nodes": nodes,
        "buffers": [buffer],
        "bufferViews": b.views,
        "accessors": b.accessors,
    });
    for (key, v) in [("meshes", meshes), ("materials", materials), ("skins", skins), ("animations", animations), ("images", images), ("textures", textures)] {
        if !v.is_empty() {
            root[key] = Value::Array(v);
        }
    }
    let json_bytes = serde_json::to_vec(&root).unwrap_or_default();
    let glb = opts.binary.then(|| glb_of(&json_bytes, &b.bin));
    Built { json: json_bytes, bin: b.bin, glb }
}
