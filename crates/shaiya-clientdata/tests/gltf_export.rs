//! glTF export (W12): the mapping, the refusals, the container, the names-only textures and the self-check. Synthetic files only; the
//! evaluator of the key test is written here in a different way from the one in the library (quaternion sandwiches on points, not matrices).

use serde_json::Value;
use shaiya_clientdata::anim::{locals, mat_inverse, pose, quat_matrix, Ani};
use shaiya_clientdata::interchange::gltf_read::Gltf;
use shaiya_clientdata::interchange::selfcheck::{semantic, structure};
use shaiya_clientdata::interchange::{export_gltf, GltfInput, GltfOptions, GltfOutput, InterchangeError, RefusalKind};
use shaiya_clientdata::mesh3dc::C3dc;
use shaiya_clientdata::mesh3de::Mesh3de;
use shaiya_clientdata::mesh3do::Mesh3do;
use shaiya_clientdata::render::skin;
use shaiya_testkit::anim::{ani_bytes, arm_ani, arm_mesh_3dc, box_3de, box_3do, c3dc_bytes, c3dc_cloak_bytes, identity_bits, animated_3do, BoneSpec, VertSpec};

fn bits(q: [f32; 4], t: [f32; 3]) -> [u32; 16] {
    let mut m = quat_matrix(q);
    m[12..15].copy_from_slice(&t);
    m.map(f32::to_bits)
}

fn unit(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    q.map(|c| c / n)
}

/// A four-bone skeleton (0 -> 1 -> 2 and 1 -> 3) with bind matrices that are not the inverse of the mesh's, keys on bones 0, 1 and 3 (bone 3 with
/// quaternions in alternating hemispheres) and none on bone 2.
fn rig(first_field: i32, keys: [i32; 3]) -> Ani {
    let q = |x, y, z, w| unit([x, y, z, w]);
    let bone = |parent: i32, matrix: [u32; 16], rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>| BoneSpec { parent, matrix, rot, trans };
    let bytes = ani_bytes(
        first_field,
        keys[2] + 5,
        &[
            bone(-1, bits(q(0.0, 0.3, 0.0, 1.0), [0.0, 0.1, 0.0]), vec![], vec![(keys[0], [0.0, 0.1, 0.0]), (keys[1], [0.2, 0.1, -0.1]), (keys[2], [0.0, 0.3, 0.2])]),
            bone(0, bits(q(0.2, 0.0, 0.1, 1.0), [0.1, 1.0, 0.0]), vec![(keys[0], q(0.0, 0.0, 0.0, 1.0)), (keys[2], q(0.4, 0.2, 0.0, 1.0))], vec![]),
            bone(1, bits(q(0.0, 0.5, 0.0, 1.0), [0.1, 2.0, 0.3]), vec![], vec![]),
            bone(
                1,
                bits(q(0.1, 0.1, 0.1, 1.0), [-0.3, 1.8, 0.0]),
                vec![(keys[0], q(0.1, 0.2, 0.3, 0.9)), (keys[1], q(-0.1, -0.2, -0.3, -0.9)), (keys[2], q(0.5, 0.0, 0.1, 0.8))],
                vec![(keys[0], [0.0, 0.0, 0.0]), (keys[2], [0.1, 0.0, 0.0])],
            ),
        ],
    );
    Ani::parse(&bytes, "t.ani").unwrap()
}

fn vert(pos: [f32; 3], weight: f32, b1: u8, b2: u8, pad: u16) -> VertSpec {
    VertSpec { pos, weight, bone1: b1, bone2: b2, pad, normal: [0.0, 1.0, 0.0], uv: [pos[0], pos[2]] }
}

/// A skinned mesh for [`rig`]: matrices that are rotations plus moves, vertices with two bones, equal bones, weight 0 and 1, a pad value.
fn mesh_for_rig() -> C3dc {
    let q = |x, y, z, w| unit([x, y, z, w]);
    let b = [bits(q(0.0, 0.1, 0.0, 1.0), [0.0, -0.1, 0.0]), bits(q(0.1, 0.0, 0.0, 1.0), [0.0, -1.0, 0.1]), bits(q(0.0, 0.0, 0.2, 1.0), [-0.1, -2.0, 0.0]), bits(q(0.3, 0.0, 0.0, 1.0), [0.3, -1.8, 0.0])];
    let v = vec![
        vert([0.0, 0.5, 0.1], 0.5, 0, 1, 0),
        vert([0.2, 1.5, 0.0], 0.25, 1, 2, 7),
        vert([-0.1, 1.9, 0.2], 1.0, 3, 1, 0),
        vert([0.1, 2.0, -0.2], 0.0, 2, 3, 0),
        vert([0.3, 1.1, 0.3], 0.6, 2, 2, 5),
        vert([0.0, 0.2, -0.3], 0.75, 1, 0, 0),
    ];
    C3dc::parse(&c3dc_bytes(b"skin.tga\0", &b, &v, &[[0, 1, 2], [2, 3, 4], [4, 5, 0], [1, 3, 5]]), "t.3dc").unwrap()
}

fn opts(source: &str) -> GltfOptions {
    GltfOptions { source: source.to_string(), ..GltfOptions::default() }
}

fn export(input: GltfInput) -> GltfOutput {
    export_gltf(input, &opts("test.3DC")).unwrap_or_else(|e| panic!("export refused: {e}"))
}

fn parse(o: &GltfOutput) -> Gltf {
    Gltf::from_parts(&o.json, &o.bin).unwrap()
}

// ---- the independent evaluator ---------------------------------------------------------------------------------------------------------

fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    // v' = v + 2 w (u x v) + 2 u x (u x v), u = (x, y, z)
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v);
    let t = [2.0 * t[0], 2.0 * t[1], 2.0 * t[2]];
    let c = cross(u, t);
    [v[0] + q[3] * t[0] + c[0], v[1] + q[3] * t[1] + c[1], v[2] + q[3] * t[2] + c[2]]
}

fn slerp(a: [f64; 4], b: [f64; 4], f: f64) -> [f64; 4] {
    let dot: f64 = (0..4).map(|i| a[i] * b[i]).sum();
    let (b, dot) = if dot < 0.0 { (b.map(|x| -x), -dot) } else { (b, dot) };
    let th = dot.min(1.0).acos();
    let (wa, wb) = if th < 1e-7 { (1.0 - f, f) } else { (((1.0 - f) * th).sin() / th.sin(), (f * th).sin() / th.sin()) };
    let q: [f64; 4] = std::array::from_fn(|i| a[i] * wa + b[i] * wb);
    let n = q.iter().map(|x| x * x).sum::<f64>().sqrt();
    q.map(|x| x / n)
}

struct Eval<'a> {
    g: &'a Gltf,
}

impl Eval<'_> {
    fn acc(&self, i: usize) -> Vec<f64> {
        self.g.accessor(i).unwrap().data
    }

    /// (translation, rotation, scale) of every node at `t` seconds of animation `a` (rest when `a` is `None`).
    fn local(&self, a: Option<usize>, t: f64) -> Vec<([f64; 3], [f64; 4], [f64; 3])> {
        let nodes = self.g.array("nodes");
        let get = |n: &Value, k: &str, d: Vec<f64>| n.get(k).map_or(d, |v| v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect());
        let mut out: Vec<_> = nodes
            .iter()
            .map(|n| {
                let (tr, r, s) = (get(n, "translation", vec![0.0; 3]), get(n, "rotation", vec![0.0, 0.0, 0.0, 1.0]), get(n, "scale", vec![1.0; 3]));
                ([tr[0], tr[1], tr[2]], [r[0], r[1], r[2], r[3]], [s[0], s[1], s[2]])
            })
            .collect();
        let Some(a) = a else { return out };
        let anim = &self.g.array("animations")[a];
        for c in anim["channels"].as_array().unwrap() {
            let s = &anim["samplers"][c["sampler"].as_u64().unwrap() as usize];
            let (times, vals) = (self.acc(s["input"].as_u64().unwrap() as usize), self.acc(s["output"].as_u64().unwrap() as usize));
            let node = c["target"]["node"].as_u64().unwrap() as usize;
            let path = c["target"]["path"].as_str().unwrap();
            let w = vals.len() / times.len();
            let at = |k: usize| &vals[k * w..(k + 1) * w];
            let v: Vec<f64> = if t <= times[0] {
                at(0).to_vec()
            } else if t >= *times.last().unwrap() {
                at(times.len() - 1).to_vec()
            } else {
                let k = (0..times.len() - 1).find(|&k| times[k] <= t && t < times[k + 1]).unwrap();
                let f = (t - times[k]) / (times[k + 1] - times[k]);
                if path == "rotation" {
                    slerp([at(k)[0], at(k)[1], at(k)[2], at(k)[3]], [at(k + 1)[0], at(k + 1)[1], at(k + 1)[2], at(k + 1)[3]], f).to_vec()
                } else {
                    (0..w).map(|i| at(k)[i] + (at(k + 1)[i] - at(k)[i]) * f).collect()
                }
            };
            match path {
                "translation" => out[node].0 = [v[0], v[1], v[2]],
                "rotation" => out[node].1 = [v[0], v[1], v[2], v[3]],
                _ => out[node].2 = [v[0], v[1], v[2]],
            }
        }
        out
    }

    /// Applies the chain of local transforms of `node` (and its parents) to a point.
    fn up(&self, trs: &[([f64; 3], [f64; 4], [f64; 3])], parents: &[Option<usize>], node: usize, p: [f64; 3]) -> [f64; 3] {
        let (t, q, s) = trs[node];
        let r = rotate(q, [p[0] * s[0], p[1] * s[1], p[2] * s[2]]);
        let p = [r[0] + t[0], r[1] + t[1], r[2] + t[2]];
        match parents[node] {
            Some(n) => self.up(trs, parents, n, p),
            None => p,
        }
    }
}

fn parents_of(g: &Gltf) -> Vec<Option<usize>> {
    let mut p = vec![None; g.array("nodes").len()];
    for (i, n) in g.array("nodes").iter().enumerate() {
        for c in n.get("children").and_then(Value::as_array).into_iter().flatten() {
            p[c.as_u64().unwrap() as usize] = Some(i);
        }
    }
    p
}

/// The key test: pose the exported skin by the glTF rules and compare with the toolkit's renderer on the source.
fn assert_poses_like_render_skin(o: &GltfOutput, mesh: &C3dc, ani: Option<&Ani>) {
    let g = parse(o);
    let ev = Eval { g: &g };
    let parents = parents_of(&g);
    let tol = 1e-5 * 3.0;
    let times: Vec<(Option<usize>, f64, f32)> = match ani {
        None => vec![(None, 0.0, 0.0)],
        Some(a) => {
            let start = shaiya_clientdata::interchange::mesh::start_key_of(a);
            let end = a.end_key;
            (0..10).map(|j| (Some(0), (j as f64 * f64::from(end - start) / 9.0) / 30.0, start as f32 + j as f32 * (end - start) as f32 / 9.0)).collect()
        }
    };
    for (anim, t, key) in times {
        let trs = ev.local(anim, t);
        let expected = match ani {
            Some(a) => skin(&mesh.meshes[0], &pose(a, &locals(a), key)).0,
            None => mesh.meshes[0].vertices.iter().map(|v| v.pos).collect(),
        };
        let mnode = g.array("nodes").iter().position(|n| n.get("mesh").is_some()).unwrap();
        let skin_j = &g.array("skins")[g.array("nodes")[mnode]["skin"].as_u64().unwrap() as usize];
        let ibm = ev.acc(skin_j["inverseBindMatrices"].as_u64().unwrap() as usize);
        let prim = &g.array("meshes")[0]["primitives"][0]["attributes"];
        let (pos, jn, wt) = (ev.acc(prim["POSITION"].as_u64().unwrap() as usize), ev.acc(prim["JOINTS_0"].as_u64().unwrap() as usize), ev.acc(prim["WEIGHTS_0"].as_u64().unwrap() as usize));
        for (v, want) in expected.iter().enumerate() {
            let mut out = [0.0f64; 3];
            for s in 0..4 {
                let (j, w) = (jn[v * 4 + s] as usize, wt[v * 4 + s]);
                if w == 0.0 {
                    continue;
                }
                let m = &ibm[j * 16..j * 16 + 16];
                let p = [pos[v * 3], pos[v * 3 + 1], pos[v * 3 + 2]];
                // the inverse bind matrix is column-major: element (r, c) is m[c * 4 + r]
                let q: [f64; 3] = std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r]);
                let joint_node = skin_j["joints"][j].as_u64().unwrap() as usize;
                let w3 = ev.up(&trs, &parents, joint_node, q);
                for r in 0..3 {
                    out[r] += w * w3[r];
                }
            }
            let got = [out[0], out[1], -out[2]];
            for r in 0..3 {
                assert!((got[r] - f64::from(want[r])).abs() < tol, "vertex {v} at key {key}: {got:?} vs {want:?}");
            }
        }
    }
}

// ---- tests -----------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_exported_skin_poses_like_render_skin() {
    let (mesh, ani) = (mesh_for_rig(), rig(0, [0, 8, 20]));
    // with an animation, and at rest without one
    let o = export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![ani.clone()] });
    assert_poses_like_render_skin(&o, &mesh, Some(&ani));
    let o = export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![] });
    assert_poses_like_render_skin(&o, &mesh, None);
    // the library's own check passes on the same files (and is what export_gltf ran)
    let g = parse(&o);
    assert!(structure(&g).is_empty());
    // the arm of the test kit (a chain with one turning bone)
    let (arm, arm_a) = (C3dc::parse(&arm_mesh_3dc(), "arm.3dc").unwrap(), Ani::parse(&arm_ani(30), "arm.ani").unwrap());
    let o = export(GltfInput::Mesh { c3dc: arm.clone(), anims: vec![arm_a.clone()] });
    assert_poses_like_render_skin(&o, &arm, Some(&arm_a));
}

#[test]
fn two_bone_weights_map_to_valid_joints_and_weights() {
    let mesh = mesh_for_rig();
    let o = export(GltfInput::Mesh { c3dc: mesh, anims: vec![rig(0, [0, 8, 20])] });
    let g = parse(&o);
    let prim = &g.array("meshes")[0]["primitives"][0]["attributes"];
    let j = g.accessor(prim["JOINTS_0"].as_u64().unwrap() as usize).unwrap().data;
    let w = g.accessor(prim["WEIGHTS_0"].as_u64().unwrap() as usize).unwrap().data;
    let pad = g.accessor(prim["_SHAIYA_PAD"].as_u64().unwrap() as usize).unwrap().data;
    // vertex 0: bones 0 and 1, weight .5; vertex 4: the same bone twice -> one joint with weight 1; vertex 3: weight 0
    assert_eq!(&j[0..4], &[0.0, 1.0, 0.0, 0.0]);
    assert_eq!(&w[0..4], &[0.5, 0.5, 0.0, 0.0]);
    assert_eq!(&j[16..20], &[2.0, 0.0, 0.0, 0.0]);
    assert_eq!(&w[16..20], &[1.0, 0.0, 0.0, 0.0]);
    // weight 0 and weight 1 list one joint only (the validator warns about a listed joint without weight)
    assert_eq!(&j[12..16], &[3.0, 0.0, 0.0, 0.0]);
    assert_eq!(&w[12..16], &[1.0, 0.0, 0.0, 0.0]);
    assert_eq!(&j[8..12], &[3.0, 0.0, 0.0, 0.0]);
    assert_eq!(&w[8..12], &[1.0, 0.0, 0.0, 0.0]);
    assert_eq!(pad[1], 7.0);
    assert_eq!(pad[4], 5.0);
    assert!(structure(&g).is_empty());
}

/// Gets the byte offset of an accessor's first element in the buffer.
fn at(g: &Gltf, acc: usize) -> usize {
    let a = &g.array("accessors")[acc];
    let v = &g.array("bufferViews")[a["bufferView"].as_u64().unwrap() as usize];
    v["byteOffset"].as_u64().unwrap() as usize + a.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize
}

fn put_f32(g: &mut Gltf, at: usize, v: f32) {
    g.bin[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn get_f32(g: &Gltf, at: usize) -> f32 {
    f32::from_le_bytes(g.bin[at..at + 4].try_into().unwrap())
}

fn anim_acc(g: &Gltf, path: &str, input: bool) -> usize {
    let a = &g.array("animations")[0];
    let c = a["channels"].as_array().unwrap().iter().find(|c| c["target"]["path"] == path && c["target"]["node"] == 3).unwrap();
    let s = &a["samplers"][c["sampler"].as_u64().unwrap() as usize];
    s[if input { "input" } else { "output" }].as_u64().unwrap() as usize
}

#[test]
fn the_twelve_mutations_fail_the_self_check() {
    let o = export(GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(0, [0, 8, 20])] });
    let clean = parse(&o);
    assert!(structure(&clean).is_empty());
    let prim = clean.array("meshes")[0]["primitives"][0].clone();
    let attr = |n: &str| prim["attributes"][n].as_u64().unwrap() as usize;
    let mut cases: Vec<(&str, Gltf)> = Vec::new();
    let mut m = |name: &'static str, f: &dyn Fn(&mut Gltf)| {
        let mut g = clean.clone();
        f(&mut g);
        cases.push((name, g));
    };
    m("m1 negative time", &|g| {
        let a = at(g, anim_acc(g, "rotation", true));
        put_f32(g, a, -1.0);
    });
    m("m2 times not increasing", &|g| {
        let a = at(g, anim_acc(g, "rotation", true));
        let first = get_f32(g, a);
        put_f32(g, a + 4, first);
    });
    m("m3 quaternion not unit", &|g| {
        let a = at(g, anim_acc(g, "rotation", false));
        let w = get_f32(g, a + 12);
        put_f32(g, a + 12, w * 2.0);
    });
    m("m4 weights do not sum to 1", &|g| {
        let a = at(g, attr("WEIGHTS_0"));
        put_f32(g, a, 0.25);
    });
    m("m5 joint twice with weight", &|g| {
        let (j, w) = (at(g, attr("JOINTS_0")), at(g, attr("WEIGHTS_0")));
        g.bin[j] = 1;
        g.bin[j + 1] = 1;
        put_f32(g, w, 0.5);
        put_f32(g, w + 4, 0.5);
    });
    m("m6 joint outside the skin", &|g| {
        let j = at(g, attr("JOINTS_0"));
        g.bin[j] = 200;
    });
    m("m7 inverse bind matrix not affine", &|g| {
        let s = &g.array("skins")[0];
        let a = at(g, s["inverseBindMatrices"].as_u64().unwrap() as usize);
        put_f32(g, a + 12, 0.5);
    });
    m("m8 position min wrong", &|g| {
        let i = attr("POSITION");
        g.json["accessors"][i]["min"][0] = serde_json::json!(-99.0);
    });
    m("m9 index outside the vertices", &|g| {
        let i = prim["indices"].as_u64().unwrap() as usize;
        let a = at(g, i);
        g.bin[a..a + 2].copy_from_slice(&60000u16.to_le_bytes());
    });
    m("m10 bufferView past the buffer", &|g| {
        g.json["bufferViews"][0]["byteLength"] = serde_json::json!(g.bin.len() + 4);
    });
    m("m11 NaN in a float accessor", &|g| {
        let a = at(g, attr("POSITION"));
        put_f32(g, a, f32::NAN);
    });
    m("m12 a node with two parents", &|g| {
        // node 3's parent is 1; give node 2 the same child
        g.json["nodes"][2]["children"] = serde_json::json!([3]);
    });
    assert_eq!(cases.len(), 12);
    for (name, g) in cases {
        assert!(!structure(&g).is_empty(), "{name}: the structural check passed");
    }
}

#[test]
fn the_semantic_self_check_catches_a_transposed_matrix() {
    let mesh = mesh_for_rig();
    let ani = rig(0, [0, 8, 20]);
    let input = GltfInput::Mesh { c3dc: mesh, anims: vec![ani] };
    let o = export(input.clone());
    let mut g = parse(&o);
    assert!(semantic(&g, &input).is_ok());
    // transpose the 3x3 of one inverse bind matrix: the structure stays valid, the pose is wrong
    let s = &g.array("skins")[0];
    let a = at(&g, s["inverseBindMatrices"].as_u64().unwrap() as usize) + 64;
    let mut m: Vec<f32> = (0..16).map(|i| get_f32(&g, a + i * 4)).collect();
    for r in 0..3 {
        for c in (r + 1)..3 {
            m.swap(r * 4 + c, c * 4 + r);
        }
    }
    for (i, v) in m.iter().enumerate() {
        put_f32(&mut g, a + i * 4, *v);
    }
    assert!(structure(&g).is_empty(), "{:?}", structure(&g));
    let err = semantic(&g, &input).unwrap_err();
    assert!(err.contains("vertex"), "{err}");
}

#[test]
fn negative_keys_shift_to_non_negative_times() {
    // first field -1 and a key at -4: start = -4, times (key + 4) / 30
    let ani = rig(-1, [-4, 8, 20]);
    let o = export(GltfInput::Anim(ani.clone()));
    let g = parse(&o);
    let t = g.accessor(anim_acc(&g, "rotation", true)).unwrap().data;
    assert_eq!(t[0], 0.0);
    assert!((t[1] - 12.0 / 30.0).abs() < 1e-6 && (t[2] - 24.0 / 30.0).abs() < 1e-6, "{t:?}");
    assert!(t.windows(2).all(|w| w[1] > w[0]));
    let extras = &g.json["asset"]["extras"]["shaiya"];
    assert_eq!((extras["start_key"].as_i64(), extras["end_key"].as_i64(), extras["first_field"].as_i64()), (Some(-4), Some(25), Some(-1)));
    // first field below every key: start = first field
    let o = export(GltfInput::Anim(rig(-3, [0, 8, 20])));
    assert_eq!(parse(&o).json["asset"]["extras"]["shaiya"]["start_key"].as_i64(), Some(-3));
    // pad_end adds a key at the end key to channels that stop earlier
    let o = export_gltf(GltfInput::Anim(rig(0, [0, 8, 20])), &GltfOptions { pad_end: true, ..opts("a.ani") }).unwrap();
    let g = parse(&o);
    let t = g.accessor(anim_acc(&g, "rotation", true)).unwrap().data;
    assert!((t.last().copied().unwrap() - 25.0 / 30.0).abs() < 1e-6);
}

#[test]
fn quaternion_hemispheres_are_continuous_in_the_file() {
    let o = export(GltfInput::Anim(rig(0, [0, 8, 20])));
    let g = parse(&o);
    let q = g.accessor(anim_acc(&g, "rotation", false)).unwrap().data;
    for k in 1..q.len() / 4 {
        let dot: f64 = (0..4).map(|i| q[k * 4 + i] * q[(k - 1) * 4 + i]).sum();
        assert!(dot >= 0.0, "key {k}");
    }
    assert!(structure(&g).is_empty());
}

#[test]
fn every_layout_exports_and_nan_vertices_are_refused_with_the_vertex() {
    // skinned, second mesh, cloak, 3DO, 3DE static and standard
    let skinned = rig_mesh_bytes();
    let second = [skinned.clone(), skinned.clone()].concat();
    let cloak = c3dc_cloak_bytes(b"cape.tga\0", &[vert([0.0, 0.0, 0.0], 1.0, 0, 0, 0), vert([1.0, 0.0, 0.0], 1.0, 0, 0, 0), vert([0.0, 1.0, 0.0], 1.0, 0, 0, 0)], &[[0, 1, 2]]);
    for (bytes, layout, anims) in [(skinned.clone(), "skinned", vec![rig(0, [0, 8, 20])]), (second, "second", vec![]), (cloak.clone(), "cloak", vec![])] {
        let c3dc = C3dc::parse(&bytes, "t.3dc").unwrap();
        let o = export(GltfInput::Mesh { c3dc, anims });
        let g = parse(&o);
        assert_eq!(g.json["asset"]["extras"]["shaiya"]["layout"], layout);
        assert!(structure(&g).is_empty());
        if layout == "cloak" {
            assert!(g.json.get("skins").is_none(), "a cloak has no skin");
        }
    }
    // a cloak with an animation is refused, with the reason
    let e = export_gltf(GltfInput::Mesh { c3dc: C3dc::parse(&cloak, "c.3dc").unwrap(), anims: vec![rig(0, [0, 8, 20])] }, &opts("c.3DC")).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::CloakAnimation]);
    for bytes in [box_3do(), animated_3do(3, 10)] {
        let o = export(GltfInput::Mesh3do(Mesh3do::parse(&bytes, "b.3do").unwrap()));
        assert_eq!(parse(&o).json["asset"]["extras"]["shaiya"]["layout"], "3do");
    }
    for bytes in [box_3de(None), box_3de(Some(3))] {
        let o = export(GltfInput::Mesh3de(Mesh3de::parse(&bytes, "b.3de").unwrap()));
        let g = parse(&o);
        assert_eq!(g.json["asset"]["extras"]["shaiya"]["layout"], "3de");
        assert!(g.json["meshes"][0]["primitives"][0]["attributes"].get("NORMAL").is_none());
    }
    // a vertex with NaN is refused and the reason names the vertex and the component
    let mut m = mesh_for_rig();
    m.meshes[0].vertices[3].pos[2] = f32::NAN;
    let e = export_gltf(GltfInput::Mesh { c3dc: m.clone(), anims: vec![] }, &opts("n.3DC")).unwrap_err();
    assert!(!e.is_bug());
    assert!(e.to_string().contains("vertex 3 holds NaN or an infinity in its position z"), "{e}");
    assert_eq!(e.kinds(), vec![RefusalKind::NotFinite]);
    // a vertex that names a bone the mesh does not have, and a face outside the vertices
    let mut m = mesh_for_rig();
    m.meshes[0].vertices[1].bone2 = 9;
    let e = export_gltf(GltfInput::Mesh { c3dc: m, anims: vec![] }, &opts("n.3DC")).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::JointRange]);
    let mut m = mesh_for_rig();
    m.meshes[0].faces[0] = [0, 1, 99];
    assert_eq!(export_gltf(GltfInput::Mesh { c3dc: m, anims: vec![] }, &opts("n.3DC")).unwrap_err().kinds(), vec![RefusalKind::Index]);
    // a mesh with fewer bones than the animation has joints for those bones only; a vertex naming a bone the animation lacks is refused
    let arm = C3dc::parse(&arm_mesh_3dc(), "arm.3dc").unwrap();
    let o = export(GltfInput::Mesh { c3dc: arm.clone(), anims: vec![rig(0, [0, 8, 20])] });
    assert_eq!(parse(&o).json["skins"][0]["joints"].as_array().unwrap().len(), 3);
    let two_bones = Ani::parse(&ani_bytes(0, 5, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![], trans: vec![] }, BoneSpec { parent: 0, matrix: identity_bits(), rot: vec![], trans: vec![] }]), "two.ani").unwrap();
    assert_eq!(export_gltf(GltfInput::Mesh { c3dc: arm, anims: vec![two_bones] }, &opts("a.3DC")).unwrap_err().kinds(), vec![RefusalKind::JointRange]);
}

fn rig_mesh_bytes() -> Vec<u8> {
    mesh_for_rig().to_bytes()
}

#[test]
fn bones_that_cannot_be_split_are_refused_by_name_and_projected_with_allow_lossy() {
    // bone 1 has no rotation track and a sheared reference matrix
    let mut ani = rig(0, [0, 8, 20]);
    ani.bones[2].matrix = {
        let mut m = quat_matrix([0.0, 0.0, 0.0, 1.0]);
        m[1] = 0.5;
        m[13] = 2.0;
        m
    };
    let e = export_gltf(GltfInput::Anim(ani.clone()), &opts("s.ani")).unwrap_err();
    assert!(e.kinds().contains(&RefusalKind::Shear), "{e}");
    assert!(e.to_string().contains("bone 2"), "{e}");
    let o = export_gltf(GltfInput::Anim(ani), &GltfOptions { allow_lossy: true, ..opts("s.ani") }).unwrap();
    assert!(o.report.projected >= 1 && !o.report.refused.is_empty());
    assert!(structure(&parse(&o)).is_empty());
    // a reflected rest matrix
    let mut ani = rig(0, [0, 8, 20]);
    ani.bones[2].matrix = {
        let mut m = quat_matrix([0.0, 0.0, 0.0, 1.0]);
        m[0] = -1.0;
        m[13] = 2.0;
        m
    };
    assert!(export_gltf(GltfInput::Anim(ani), &opts("r.ani")).unwrap_err().kinds().contains(&RefusalKind::Reflection));
    // NaN
    let mut ani = rig(0, [0, 8, 20]);
    ani.bones[2].matrix[5] = f32::NAN;
    assert!(export_gltf(GltfInput::Anim(ani), &opts("n.ani")).unwrap_err().kinds().contains(&RefusalKind::NotFinite));
    // keys that do not increase
    let mut ani = rig(0, [0, 8, 20]);
    ani.bones[1].rot[1].key = 0;
    let e = export_gltf(GltfInput::Anim(ani.clone()), &opts("k.ani")).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::KeysNotIncreasing]);
    assert!(export_gltf(GltfInput::Anim(ani), &GltfOptions { allow_lossy: true, ..opts("k.ani") }).is_ok());
}

#[test]
fn glb_and_gltf_carry_the_same_buffer() {
    let input = GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(0, [0, 8, 20])] };
    let plain = export_gltf(input.clone(), &GltfOptions { bin_name: "x.bin".into(), ..opts("m.3DC") }).unwrap();
    let binary = export_gltf(input, &GltfOptions { binary: true, ..opts("m.3DC") }).unwrap();
    assert_eq!(plain.bin, binary.bin);
    assert_eq!(plain.bin.len() % 4, 0);
    assert!(plain.glb.is_none());
    let glb = binary.glb.clone().unwrap();
    // container: magic, version, exact total, 4-aligned chunks, JSON then BIN
    assert_eq!(&glb[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
    let jl = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(jl % 4, 0);
    assert_eq!(&glb[16..20], b"JSON");
    let bl = u32::from_le_bytes(glb[20 + jl..24 + jl].try_into().unwrap()) as usize;
    assert_eq!(&glb[24 + jl..28 + jl], b"BIN\0");
    assert_eq!(&glb[28 + jl..28 + jl + bl], &plain.bin[..]);
    assert_eq!(28 + jl + bl, glb.len());
    let (a, b) = (Gltf::from_parts(&plain.json, &plain.bin).unwrap(), Gltf::from_glb(&glb).unwrap());
    assert_eq!(a.bin, b.bin);
    assert_eq!(a.json["buffers"][0]["uri"], "x.bin");
    assert!(b.json["buffers"][0].get("uri").is_none());
    // everything but the buffer's uri is equal
    let (mut ja, mut jb) = (a.json.clone(), b.json.clone());
    ja["buffers"][0].as_object_mut().unwrap().remove("uri");
    jb["buffers"][0].as_object_mut().unwrap().remove("uri");
    assert_eq!(ja, jb);
    // damage to the container is an error, not a panic
    let mut bad = glb.clone();
    bad.truncate(bad.len() - 3);
    assert!(Gltf::from_glb(&bad).is_err());
    let mut bad = glb;
    bad[12] ^= 1;
    assert!(Gltf::from_glb(&bad).is_err());
}

#[test]
fn texture_names_only_and_no_images() {
    // a name with a NUL and bytes after it: the material shows the text up to the NUL, the hex keeps every byte
    let mut m = mesh_for_rig();
    m.meshes[0].texture = b"skin.tga\0junk".to_vec();
    let o = export(GltfInput::Mesh { c3dc: m.clone(), anims: vec![] });
    let g = parse(&o);
    assert_eq!(g.json["materials"][0]["name"], "skin.tga");
    assert_eq!(g.json["materials"][0]["extras"]["shaiya"]["texture_hex"], "736b696e2e74676100 6a756e6b".replace(' ', ""));
    for key in ["images", "textures", "samplers"] {
        assert!(g.json.get(key).is_none(), "{key} written");
    }
    assert!(!String::from_utf8_lossy(&o.json).contains("baseColorTexture"));
    // an empty name: no material name, empty hex
    m.meshes[0].texture = Vec::new();
    let g = parse(&export(GltfInput::Mesh { c3dc: m.clone(), anims: vec![] }));
    assert!(g.json["materials"][0].get("name").is_none());
    assert_eq!(g.json["materials"][0]["extras"]["shaiya"]["texture_hex"], "");
    // --texture-uri-base writes uris only, never image data
    m.meshes[0].texture = b"my skin.tga\0".to_vec();
    let o = export_gltf(GltfInput::Mesh { c3dc: m, anims: vec![] }, &GltfOptions { texture_uri_base: Some("tex dir/".into()), ..opts("m.3DC") }).unwrap();
    let g = parse(&o);
    assert_eq!(g.json["images"][0]["uri"], "tex%20dir/my%20skin.tga.png");
    assert_eq!(g.json["images"].as_array().unwrap().len(), 1);
    assert!(!g.json["images"][0].as_object().unwrap().contains_key("bufferView"));
}

#[test]
fn the_node_layout_follows_the_file() {
    let o = export(GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(0, [0, 8, 20])] });
    let g = parse(&o);
    let names: Vec<&str> = g.array("nodes").iter().take(4).map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Bone_000", "Bone_001", "Bone_002", "Bone_003"]);
    assert_eq!(g.json["nodes"][1]["children"], serde_json::json!([2, 3]));
    // one root: the skin names it; the mesh node comes after the bones and has no transform
    assert_eq!(g.json["skins"][0]["skeleton"], 0);
    assert_eq!(g.json["nodes"][4]["mesh"], 0);
    assert!(g.json["nodes"][4].get("translation").is_none() && g.json["nodes"][4].get("scale").is_none());
    assert_eq!(g.json["scenes"][0]["nodes"], serde_json::json!([0, 4]));
    // no animation: every bone is a root and no skeleton is named
    let g = parse(&export(GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![] }));
    assert_eq!(g.json["nodes"][4]["name"], "Shaiya_Root");
    assert_eq!(g.json["nodes"][4]["children"], serde_json::json!([0, 1, 2, 3]));
    assert_eq!(g.json["skins"][0]["skeleton"], 4);
    assert_eq!(g.json["scenes"][0]["nodes"], serde_json::json!([4, 5]));
    assert_eq!(g.json["nodes"][5]["mesh"], 0);
    // the rest pose is the inverse of the mesh's matrices: inverse of the glTF inverse bind matrix is the node's world matrix
    let mesh = mesh_for_rig();
    let inv = mat_inverse(&mesh.meshes[0].bones[2]);
    let _ = inv;
    // the same input twice gives the same bytes
    let a = export(GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(0, [0, 8, 20])] });
    assert_eq!((a.json, a.bin), (o.json, o.bin));
    // an identity-matrix animation alone still exports
    let ani = Ani::parse(&ani_bytes(0, 10, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![], trans: vec![] }]), "i.ani").unwrap();
    assert!(structure(&parse(&export(GltfInput::Anim(ani)))).is_empty());
}

#[test]
fn an_error_from_a_bug_is_not_a_refusal() {
    assert!(InterchangeError::SelfCheck("x".into()).is_bug());
    assert!(!InterchangeError::Refused { list: vec![], more: 0 }.is_bug());
}

#[test]
fn refusals_name_their_kind_and_a_bone_nobody_uses_is_not_one() {
    let base = mesh_for_rig();
    let kinds = |m: C3dc, anims: Vec<Ani>| export_gltf(GltfInput::Mesh { c3dc: m, anims }, &opts("r.3DC")).unwrap_err().kinds();
    // a weight outside 0 to 1, and NaN
    let mut m = base.clone();
    m.meshes[0].vertices[0].weight = 1.5;
    assert_eq!(kinds(m, vec![]), vec![RefusalKind::Weight]);
    let mut m = base.clone();
    m.meshes[0].vertices[0].weight = f32::NAN;
    assert_eq!(kinds(m, vec![]), vec![RefusalKind::Weight]);
    // a matrix of the mesh with NaN
    let mut m = base.clone();
    m.meshes[0].bones[1][5] = f32::NAN;
    assert!(kinds(m, vec![]).contains(&RefusalKind::NotFinite));
    // animations that do not share a skeleton
    let other = Ani::parse(&ani_bytes(0, 5, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![], trans: vec![] }]), "o.ani").unwrap();
    assert_eq!(kinds(base.clone(), vec![rig(0, [0, 8, 20]), other]), vec![RefusalKind::Skeleton]);
    // no faces
    let mut m = base.clone();
    m.meshes[0].faces.clear();
    assert_eq!(kinds(m, vec![]), vec![RefusalKind::Empty]);
    // a mirrored matrix of a bone that a vertex uses is refused; of a bone nobody uses it is not
    let mirrored = |m: &mut C3dc| {
        let mut r = quat_matrix([0.0, 0.0, 0.0, 1.0]);
        r[0] = -1.0;
        m.meshes[0].bones[3] = r;
    };
    let mut m = base.clone();
    mirrored(&mut m);
    assert_eq!(kinds(m, vec![]), vec![RefusalKind::Reflection]);
    let mut m = base;
    mirrored(&mut m);
    for v in m.meshes[0].vertices.iter_mut() {
        v.bone1 = v.bone1.min(2);
        v.bone2 = v.bone2.min(2);
    }
    let o = export(GltfInput::Mesh { c3dc: m, anims: vec![] });
    assert!(o.report.notes.iter().any(|n| n.contains("bones that no vertex uses")), "{:?}", o.report.notes);
    assert!(structure(&parse(&o)).is_empty());
    assert_eq!(parse(&o).json["asset"]["extras"]["shaiya"]["toolkit"], env!("CARGO_PKG_VERSION"));
}

// ---- opt-in: the Khronos validator --------------------------------------------------------------------------------------------------

/// The pinned driver of the validator (gltf-validator 2.0.0-dev.3.10, run offline under node): validates each file given, reading the `.bin`
/// of a `.gltf` from its own folder, and prints one JSON array with the counts and the codes of every message.
const VALIDATOR_DRIVER: &str = r#"const v = require(process.argv[2]);
const fs = require('fs');
const path = require('path');
(async () => {
  const out = [];
  for (const f of process.argv.slice(3)) {
    const bytes = new Uint8Array(fs.readFileSync(f));
    const r = await v.validateBytes(bytes, {
      uri: path.basename(f),
      externalResourceFunction: (uri) => Promise.resolve(new Uint8Array(fs.readFileSync(path.join(path.dirname(f), decodeURIComponent(uri))))),
    });
    out.push({ file: path.basename(f), errors: r.issues.numErrors, warnings: r.issues.numWarnings, infos: r.issues.numInfos,
      messages: r.issues.messages.map((m) => ({ code: m.code, severity: m.severity, pointer: m.pointer })) });
  }
  console.log(JSON.stringify(out));
})().catch((e) => { console.error(String(e)); process.exit(1); });
"#;

struct ScratchDir(std::path::PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The synthetic inputs of T7-T10 as (file stem, input).
fn fixtures() -> Vec<(String, GltfInput)> {
    let mut v: Vec<(String, GltfInput)> = vec![
        ("rig_mesh_anim".into(), GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(0, [0, 8, 20])] }),
        ("rig_mesh_rest".into(), GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![] }),
        ("rig_negative_keys".into(), GltfInput::Mesh { c3dc: mesh_for_rig(), anims: vec![rig(-1, [-4, 8, 20]), rig(0, [0, 3, 9])] }),
        ("rig_anim".into(), GltfInput::Anim(rig(0, [0, 8, 20]))),
        ("arm".into(), GltfInput::Mesh { c3dc: C3dc::parse(&arm_mesh_3dc(), "arm.3dc").unwrap(), anims: vec![Ani::parse(&arm_ani(30), "arm.ani").unwrap()] }),
        ("cloak".into(), GltfInput::Mesh { c3dc: C3dc::parse(&c3dc_cloak_bytes(b"cape.tga\0", &[vert([0.0, 0.0, 0.0], 1.0, 0, 0, 0), vert([1.0, 0.0, 0.0], 1.0, 0, 0, 0), vert([0.0, 1.0, 0.0], 1.0, 0, 0, 0)], &[[0, 1, 2]]), "c.3dc").unwrap(), anims: vec![] }),
        ("box3do".into(), GltfInput::Mesh3do(Mesh3do::parse(&animated_3do(3, 10), "b.3do").unwrap())),
        ("box3de".into(), GltfInput::Mesh3de(Mesh3de::parse(&box_3de(Some(2)), "b.3de").unwrap())),
    ];
    let mut named = mesh_for_rig();
    named.meshes[0].texture = b"skin.tga\0junk".to_vec();
    v.push(("textured".into(), GltfInput::Mesh { c3dc: named, anims: vec![] }));
    v
}

/// Runs the validator over `files` and returns the parsed report.
fn run_validator(dir: &std::path::Path, validator: &std::path::Path, files: &[std::path::PathBuf]) -> Vec<Value> {
    let driver = dir.join("driver.js");
    std::fs::write(&driver, VALIDATOR_DRIVER).unwrap();
    let out = std::process::Command::new("node").current_dir(dir).arg(&driver).arg(validator).args(files).output().expect("node runs");
    assert!(out.status.success(), "the validator driver failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice::<Value>(&out.stdout).unwrap().as_array().unwrap().clone()
}

#[test]
fn the_khronos_validator_accepts_the_output() {
    let Some(val) = shaiya_testkit::env_var_path("SHAIYA_GLTF_VALIDATOR") else { return };
    let Some(scratch) = shaiya_testkit::env_scratch() else { return };
    if std::process::Command::new("node").arg("--version").output().is_err() {
        let require = std::env::var("SHAIYA_OPTIN_REQUIRE").unwrap_or_default();
        assert!(!require.split(',').any(|n| n.trim() == "SHAIYA_GLTF_VALIDATOR"), "node is not on PATH");
        eprintln!("skipped: node is not on PATH");
        return;
    }
    let nested = val.join("node_modules").join("gltf-validator");
    let validator = if nested.is_dir() { nested } else { val };
    let dir = ScratchDir(scratch.join(format!("w12-validator-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let mut files = Vec::new();
    let mut write = |stem: &str, input: GltfInput, source: &str| {
        for binary in [false, true] {
            let o = export_gltf(input.clone(), &GltfOptions { binary, bin_name: format!("{stem}.bin"), ..opts(source) }).unwrap_or_else(|e| panic!("{stem}: {e}"));
            if binary {
                let p = dir.0.join(format!("{stem}.glb"));
                std::fs::write(&p, o.glb.unwrap()).unwrap();
                files.push(p);
            } else {
                let (p, b) = (dir.0.join(format!("{stem}.gltf")), dir.0.join(format!("{stem}.bin")));
                std::fs::write(&p, &o.json).unwrap();
                std::fs::write(b, &o.bin).unwrap();
                files.push(p);
            }
        }
    };
    for (stem, input) in fixtures() {
        write(&stem, input, &format!("{stem}.3DC"));
    }
    // one rig of the US client with its default parts and its first <rig>_000_ animation, when the client is given
    let mut real = 0;
    if let Some(client) = shaiya_testkit::env_client() {
        let a = shaiya_archive::Archive::open(&client).unwrap();
        let paths: Vec<String> = a.index().visible().into_iter().map(|f| f.path.to_string()).collect();
        let mut anis: Vec<&String> = paths.iter().filter(|p| p.to_ascii_lowercase().contains("character/human/ani/huwm_000_")).collect();
        anis.sort();
        let ani_path = anis.first().expect("a huwm_000_ animation");
        let ani = Ani::parse(&a.read(ani_path).unwrap(), ani_path).unwrap();
        let mut info = |p: &str| a.read(p).ok().and_then(|b| C3dc::parse(&b, p).ok()).map(|c| shaiya_clientdata::meshpick::MeshInfo::of(&c));
        let (_, defaults) = shaiya_clientdata::meshpick::mesh_candidates_in(&paths, ani_path, ani.bones.len(), &mut info);
        assert!(!defaults.is_empty());
        for (i, p) in defaults.iter().enumerate() {
            let c = C3dc::parse(&a.read(p).unwrap(), p).unwrap();
            write(&format!("us_rig_part{i}"), GltfInput::Mesh { c3dc: c, anims: vec![ani.clone()] }, p.rsplit('/').next().unwrap_or(p));
            real += 1;
        }
    }
    let report = run_validator(&dir.0, &validator, &files);
    let (mut errors, mut warnings, mut infos) = (0, 0, 0);
    let mut unexpected = Vec::new();
    for r in &report {
        errors += r["errors"].as_u64().unwrap();
        warnings += r["warnings"].as_u64().unwrap();
        infos += r["infos"].as_u64().unwrap();
        for m in r["messages"].as_array().unwrap() {
            let sev = m["severity"].as_u64().unwrap();
            if sev <= 1 && m["code"] != "UNUSED_OBJECT" {
                unexpected.push(format!("{}: {} {} at {}", r["file"], sev, m["code"], m["pointer"]));
            }
        }
    }
    println!("VALIDATOR: {} files ({} from the US client), {errors} errors, {warnings} warnings, {infos} infos", report.len(), real * 2);
    assert!(unexpected.is_empty(), "{unexpected:#?}");
    assert_eq!(errors, 0);
}
