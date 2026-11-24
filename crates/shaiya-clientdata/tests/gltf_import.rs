//! glTF import (W13): round trips against the export, every refusal with its reason, the sampler rules and the malformed-file cases.
//! Synthetic files only; the expected values of the sampler tests are computed here in a different spelling from the library's.

use serde_json::json;
use shaiya_clientdata::anim::{locals, passes, pose, quat_matrix, Ani};
use shaiya_clientdata::interchange::gltf_read::{buffer_name, Gltf, ReadError};
use shaiya_clientdata::interchange::import::{finish, import_gltf, parse_name_map, ImportError, ImportKind, ImportOptions, ImportReport, Imported, Like};
use shaiya_clientdata::interchange::{export_gltf, GltfInput, GltfOptions, GltfOutput, RefusalKind};
use shaiya_clientdata::mesh3dc::C3dc;
use shaiya_clientdata::mesh3de::Mesh3de;
use shaiya_clientdata::mesh3do::Mesh3do;
use shaiya_testkit::anim::{ani_bytes, box_3de, box_3do, c3dc_bytes, identity_bits, BoneSpec, VertSpec};

fn bits(q: [f32; 4], t: [f32; 3]) -> [u32; 16] {
    let mut m = quat_matrix(q);
    m[12..15].copy_from_slice(&t);
    m.map(f32::to_bits)
}

fn unit(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    q.map(|c| c / n)
}

/// A four-bone skeleton (0 -> 1 -> 2 and 1 -> 3) with keys on bones 0, 1 and 3 (bone 3 with quaternions in alternating hemispheres) and none
/// on bone 2.
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

/// Vertices with two bones, equal bones, weight 0 and weight 1 (with different second bones) and a pad value.
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

fn export(input: GltfInput, source: &str) -> GltfOutput {
    export_gltf(input, &GltfOptions { source: source.to_string(), ..GltfOptions::default() }).unwrap_or_else(|e| panic!("export refused: {e}"))
}

fn opened(o: &GltfOutput) -> Gltf {
    Gltf::open(&o.json, &mut |name| if name == "model.bin" { Ok(o.bin.clone()) } else { Err(format!("no file {name}")) }).unwrap_or_else(|e| panic!("open refused: {e:?}"))
}

/// Opens a (mutated) `Gltf` again the way a file on disk would be opened: its JSON through `open`, its buffer served as `model.bin`.
fn reopen(g: &Gltf) -> Result<Gltf, ReadError> {
    let bin = g.bin.clone();
    Gltf::open(&serde_json::to_vec(&g.json).unwrap(), &mut |name| if name == "model.bin" { Ok(bin.clone()) } else { Err(format!("no file {name}")) })
}

fn run(g: &Gltf, kind: ImportKind, like: Option<&Like>, opts: &ImportOptions) -> Result<(Imported, ImportReport), ImportError> {
    import_gltf(g, kind, like, opts)
}

fn plain() -> ImportOptions {
    ImportOptions::default()
}

fn ani_of(i: Imported) -> Ani {
    match i {
        Imported::Ani(a) => a,
        other => panic!("not an animation: {other:?}"),
    }
}

fn assert_poses_equal(a: &Ani, b: &Ani, keys: &[f32], tol: f32) {
    let (la, lb) = (locals(a), locals(b));
    for &k in keys {
        let (pa, pb) = (pose(a, &la, k), pose(b, &lb, k));
        for (bone, (x, y)) in pa.iter().zip(&pb).enumerate() {
            for e in 0..16 {
                assert!((x[e] - y[e]).abs() <= tol, "key {k}, bone {bone}, element {e}: {} vs {}", x[e], y[e]);
            }
        }
    }
}

// ---- T11, T12 --------------------------------------------------------------------------------------------------------------------------

#[test]
fn a_3dc_round_trips_through_gltf_to_the_same_bytes() {
    let mesh = mesh_for_rig();
    let original = mesh.to_bytes();
    for (binary, anims) in [(false, vec![]), (true, vec![]), (false, vec![rig(0, [0, 8, 20])])] {
        let o = export_gltf(GltfInput::Mesh { c3dc: mesh.clone(), anims }, &GltfOptions { binary, source: "t.3DC".into(), ..GltfOptions::default() }).unwrap();
        let g = match &o.glb {
            Some(glb) => Gltf::open(glb, &mut |_| Err("no files".to_string())).unwrap(),
            None => opened(&o),
        };
        let (imp, report) = run(&g, ImportKind::C3dc, Some(&Like::C3dc(mesh.clone())), &plain()).unwrap();
        let bytes = finish(&imp, Some(&Like::C3dc(mesh.clone()))).unwrap();
        assert_eq!(bytes, original, "binary {binary}");
        assert_eq!((report.meshes, report.vertices), (1, 6));
    }
    // the rig's .ANI as the like file: the same matrices, texture, pad, positions and faces; the second bone of a one-bone vertex is not known
    let o = export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![] }, "t.3DC");
    let like = Like::Ani(rig(0, [0, 8, 20]));
    let (imp, _) = run(&opened(&o), ImportKind::C3dc, Some(&like), &plain()).unwrap();
    let Imported::C3dc(back) = imp else { panic!() };
    let (m, b) = (&mesh.meshes[0], &back.meshes[0]);
    assert_eq!((&m.bones, &m.texture, &m.faces), (&b.bones, &b.texture, &b.faces));
    for (v, w) in m.vertices.iter().zip(&b.vertices) {
        assert_eq!((v.pos, v.pad, v.normal, v.uv), (w.pos, w.pad, w.normal, w.uv));
    }
    assert!(finish(&Imported::C3dc(back), Some(&like)).is_ok());
}

#[test]
fn a_second_mesh_layout_round_trips_too() {
    let m = mesh_for_rig();
    let mut two = m.clone();
    two.layout = shaiya_clientdata::mesh3dc::C3dcLayout::SkinnedWithSecondMesh;
    two.meshes.push(m.meshes[0].clone());
    let original = two.to_bytes();
    let o = export(GltfInput::Mesh { c3dc: two.clone(), anims: vec![] }, "t.3DC");
    let (imp, _) = run(&opened(&o), ImportKind::C3dc, Some(&Like::C3dc(two.clone())), &plain()).unwrap();
    assert_eq!(finish(&imp, Some(&Like::C3dc(two))).unwrap(), original);
}

#[test]
fn an_ani_round_trips_with_keys_exact_to_the_same_poses() {
    for (first, keys) in [(0, [0, 8, 20]), (-1, [-4, 8, 20])] {
        let ani = rig(first, keys);
        let o = export(GltfInput::Anim(ani.clone()), "t.ANI");
        let g = opened(&o);
        let like = Like::Ani(ani.clone());
        let exact = ImportOptions { keys_exact: true, ..plain() };
        let back = ani_of(run(&g, ImportKind::Ani, Some(&like), &exact).unwrap().0);
        assert!(passes(&back, &ani), "the imported animation passes against its like file");
        assert_eq!(back.end_key, ani.end_key);
        // the same keys, and the same poses at whole and half keys
        for (a, b) in ani.bones.iter().zip(&back.bones) {
            assert_eq!(a.rot.iter().map(|k| k.key).collect::<Vec<_>>(), b.rot.iter().map(|k| k.key).collect::<Vec<_>>());
            assert_eq!(a.trans.iter().map(|k| k.key).collect::<Vec<_>>(), b.trans.iter().map(|k| k.key).collect::<Vec<_>>());
        }
        let sample: Vec<f32> = (0..=((keys[2] - keys[0]) * 2)).map(|i| keys[0] as f32 + i as f32 / 2.0).collect();
        assert_poses_equal(&ani, &back, &sample, 1e-5);
        // the default (every whole key), with and without --reduce
        let dense = ani_of(run(&g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
        assert!(passes(&dense, &ani));
        let whole: Vec<f32> = (keys[0]..=keys[2]).map(|k| k as f32).collect();
        assert_poses_equal(&ani, &dense, &whole, 1e-5);
        let reduced = ani_of(run(&g, ImportKind::Ani, Some(&like), &ImportOptions { reduce: true, ..plain() }).unwrap().0);
        assert!(passes(&reduced, &ani));
        assert_poses_equal(&ani, &reduced, &whole, 1e-4);
        let count = |a: &Ani| a.bones.iter().map(|b| b.rot.len() + b.trans.len()).sum::<usize>();
        assert!(count(&reduced) < count(&dense), "{} vs {}", count(&reduced), count(&dense));
        // the bytes read back
        assert!(Ani::parse(&back.try_to_bytes().unwrap(), "x.ani").is_ok());
    }
}

#[test]
fn static_meshes_round_trip_through_gltf() {
    let m3do = Mesh3do::parse(&box_3do(), "b.3do").unwrap();
    let o = export(GltfInput::Mesh3do(m3do.clone()), "b.3DO");
    let (imp, _) = run(&opened(&o), ImportKind::Mesh3do, None, &plain()).unwrap();
    assert_eq!(finish(&imp, None).unwrap(), m3do.to_bytes());
    let m3de = Mesh3de::parse(&box_3de(None), "b.3de").unwrap();
    let o = export(GltfInput::Mesh3de(m3de.clone()), "b.3DE");
    let (imp, _) = run(&opened(&o), ImportKind::Mesh3de, None, &plain()).unwrap();
    assert_eq!(finish(&imp, None).unwrap(), m3de.to_bytes());
    // a .3DO that is a .3DE's glTF has no normals: they are computed and said
    let (imp, report) = run(&opened(&o), ImportKind::Mesh3do, None, &plain()).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("no normals")), "{:?}", report.notes);
    let Imported::Mesh3do(m) = imp else { panic!() };
    assert!(m.vertices.iter().all(|v| (v.normal.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-5));
}

// ---- T13 -------------------------------------------------------------------------------------------------------------------------------

fn expect(r: Result<(Imported, ImportReport), ImportError>, kind: RefusalKind, words: &str, case: &str) {
    match r {
        Ok(_) => panic!("{case}: was imported"),
        Err(e) => {
            assert!(e.kinds().contains(&kind), "{case}: kinds {:?} for {e}", e.kinds());
            assert!(e.to_string().contains(words) || e.list.iter().any(|r| r.reason.contains(words)), "{case}: {e} does not say {words:?}");
        }
    }
}

fn expect_open(r: Result<Gltf, ReadError>, kind: RefusalKind, words: &str, case: &str) {
    match r {
        Ok(_) => panic!("{case}: was opened"),
        Err((k, m)) => {
            assert_eq!(k, kind, "{case}: {m}");
            assert!(m.contains(words), "{case}: {m} does not say {words:?}");
        }
    }
}

fn node_index(g: &Gltf, name: &str) -> usize {
    g.array("nodes").iter().position(|n| n["name"] == name).unwrap_or_else(|| panic!("no node {name}"))
}

/// Appends float data as a new buffer view and accessor; returns the accessor.
fn push_accessor(g: &mut Gltf, data: &[f32], ty: &str) -> usize {
    while !g.bin.len().is_multiple_of(4) {
        g.bin.push(0);
    }
    let off = g.bin.len();
    for f in data {
        g.bin.extend_from_slice(&f.to_le_bytes());
    }
    let width = match ty {
        "SCALAR" => 1,
        "VEC3" => 3,
        _ => 4,
    };
    let len = g.bin.len();
    g.json["buffers"][0]["byteLength"] = json!(len);
    let views = g.json["bufferViews"].as_array_mut().unwrap();
    views.push(json!({"buffer": 0, "byteOffset": off, "byteLength": data.len() * 4}));
    let view = views.len() - 1;
    let accs = g.json["accessors"].as_array_mut().unwrap();
    accs.push(json!({"bufferView": view, "componentType": 5126, "count": data.len() / width, "type": ty}));
    accs.len() - 1
}

/// Adds a channel on `node` (`path` rotation/translation/scale/weights) with its own sampler.
fn push_channel(g: &mut Gltf, node: usize, path: &str, times: &[f32], values: &[f32]) {
    let ty = if path == "rotation" { "VEC4" } else if path == "weights" { "SCALAR" } else { "VEC3" };
    let (i, o) = (push_accessor(g, times, "SCALAR"), push_accessor(g, values, ty));
    let a = &mut g.json["animations"][0];
    let s = a["samplers"].as_array().unwrap().len();
    a["samplers"].as_array_mut().unwrap().push(json!({"input": i, "output": o, "interpolation": "LINEAR"}));
    a["channels"].as_array_mut().unwrap().push(json!({"sampler": s, "target": {"node": node, "path": path}}));
}

#[test]
fn each_refusal_names_its_reason() {
    let ani = rig(0, [0, 8, 20]);
    let mesh = mesh_for_rig();
    let ani_g = opened(&export(GltfInput::Anim(ani.clone()), "t.ANI"));
    let mesh_g = opened(&export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![] }, "t.3DC"));
    let (la, lm) = (Like::Ani(ani.clone()), Like::C3dc(mesh.clone()));
    // the clean files import
    run(&ani_g, ImportKind::Ani, Some(&la), &plain()).unwrap();
    run(&mesh_g, ImportKind::C3dc, Some(&lm), &plain()).unwrap();
    let prim = |g: &Gltf| g.array("meshes")[0]["primitives"][0].clone();
    let attr = |g: &Gltf, n: &str| prim(g)["attributes"][n].as_u64().unwrap() as usize;

    // --- more than two influences, and what --reduce-influences does
    let mut g = mesh_g.clone();
    let (j, w) = (attr(&g, "JOINTS_0"), attr(&g, "WEIGHTS_0"));
    let view = |g: &Gltf, acc: usize| {
        let a = &g.array("accessors")[acc];
        g.array("bufferViews")[a["bufferView"].as_u64().unwrap() as usize]["byteOffset"].as_u64().unwrap() as usize
    };
    let (jo, wo) = (view(&g, j), view(&g, w));
    g.bin[jo..jo + 4].copy_from_slice(&[0, 1, 2, 3]);
    for (k, x) in [0.4f32, 0.3, 0.2, 0.1].iter().enumerate() {
        g.bin[wo + k * 4..wo + k * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Influences, "more than 2 bone influences (first: vertex 0)", "three influences");
    let (imp, report) = run(&g, ImportKind::C3dc, Some(&lm), &ImportOptions { reduce_influences: true, ..plain() }).unwrap();
    assert_eq!(report.influences_reduced, 1);
    assert!((report.max_weight_dropped - 0.2).abs() < 1e-6, "{}", report.max_weight_dropped);
    let Imported::C3dc(c) = imp else { panic!() };
    let v = &c.meshes[0].vertices[0];
    assert_eq!((v.bone1, v.bone2), (0, 1));
    assert!((v.weight - 0.4 / 0.7).abs() < 1e-6, "{}", v.weight);

    // --- the mesh's own limits
    let mut big = mesh.clone();
    big.meshes[0].bones = vec![shaiya_clientdata::anim::IDENTITY; 257];
    expect(run(&mesh_g, ImportKind::C3dc, Some(&Like::C3dc(big)), &plain()), RefusalKind::Like, "256", "257 bones");
    let mut g = mesh_g.clone();
    let pos = attr(&g, "POSITION");
    let n = 70_000usize;
    let off = g.bin.len();
    g.bin.resize(off + n * 12, 0);
    let len = g.bin.len();
    g.json["buffers"][0]["byteLength"] = json!(len);
    let views = g.json["bufferViews"].as_array_mut().unwrap();
    views.push(json!({"buffer": 0, "byteOffset": off, "byteLength": n * 12}));
    let vi = views.len() - 1;
    g.json["accessors"][pos]["bufferView"] = json!(vi);
    g.json["accessors"][pos]["count"] = json!(n);
    g.json["accessors"][pos].as_object_mut().unwrap().remove("min");
    g.json["accessors"][pos].as_object_mut().unwrap().remove("max");
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Limit, "at most 65535", "70,000 vertices");
    let mut g = mesh_g.clone();
    g.json["meshes"][0]["primitives"][0]["mode"] = json!(1);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Unsupported, "mode is 1", "lines");
    let mut g = mesh_g.clone();
    g.json["meshes"][0]["primitives"][0]["targets"] = json!([{"POSITION": 0}]);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Morph, "morph targets", "morph targets");
    let mut g = mesh_g.clone();
    let pos = attr(&g, "POSITION");
    g.json["accessors"][pos]["sparse"] = json!({"count": 1, "indices": {"bufferView": 0, "componentType": 5123}, "values": {"bufferView": 0}});
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Unsupported, "sparse", "sparse accessor");
    let mut g = mesh_g.clone();
    let uv = attr(&g, "TEXCOORD_0");
    g.json["accessors"][uv]["normalized"] = json!(true);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Unsupported, "normalized", "normalized attribute");
    let mut g = mesh_g.clone();
    let pos = attr(&g, "POSITION");
    g.json["accessors"][pos]["componentType"] = json!(5122);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Unsupported, "quantized", "16-bit positions");
    let mut g = mesh_g.clone();
    let mn = g.array("nodes").iter().position(|n| n.get("mesh").is_some()).unwrap();
    g.json["nodes"][mn]["translation"] = json!([0.0, 1.0, 0.0]);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Transform, "identity", "mesh node with a transform");
    let mut g = mesh_g.clone();
    let mn = g.array("nodes").iter().position(|n| n.get("mesh").is_some()).unwrap();
    g.json["nodes"][mn].as_object_mut().unwrap().remove("skin");
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Format, "no skin", "no skin");
    let mut g = mesh_g.clone();
    let hex = "00".repeat(300);
    g.json["materials"][0]["extras"]["shaiya"]["texture_hex"] = json!(hex);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Limit, "at most 255", "a texture name of 300 bytes");

    // --- the skeleton: names and hierarchy
    let mut g = ani_g.clone();
    let n1 = node_index(&g, "Bone_001");
    g.json["nodes"][n1]["name"] = json!("Elbow");
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Names, "no node (Bone_NNN or --map)", "a renamed bone");
    let mut g = ani_g.clone();
    let n1 = node_index(&g, "Bone_001");
    g.json["nodes"][n1]["name"] = json!("Bone_099");
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Names, "does not have", "an extra bone");
    let mut g = ani_g.clone();
    let (n2, n3) = (node_index(&g, "Bone_002"), node_index(&g, "Bone_003"));
    g.json["nodes"][n3]["children"] = json!([]);
    g.json["nodes"][n2]["children"] = json!([]);
    let n1 = node_index(&g, "Bone_001");
    g.json["nodes"][n1]["children"] = json!([n3]);
    // bone 2 lost its parent
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Names, "another parent", "another hierarchy");
    // the map file gives a node a bone
    let mut g = ani_g.clone();
    let n1 = node_index(&g, "Bone_001");
    g.json["nodes"][n1]["name"] = json!("Elbow");
    let map = parse_name_map("# the elbow\nElbow = 1\n").unwrap();
    run(&g, ImportKind::Ani, Some(&la), &ImportOptions { map, ..plain() }).unwrap();
    let map = parse_name_map("Hand = 1\n").unwrap();
    expect(run(&g, ImportKind::Ani, Some(&la), &ImportOptions { map, ..plain() }), RefusalKind::Names, "no node of this name", "a map entry without a node");
    // Bone_NNN of any width
    let mut g = ani_g.clone();
    let n1 = node_index(&g, "Bone_001");
    g.json["nodes"][n1]["name"] = json!("Bone_1");
    run(&g, ImportKind::Ani, Some(&la), &plain()).unwrap();
    g.json["nodes"][n1]["name"] = json!("Bone_00001");
    run(&g, ImportKind::Ani, Some(&la), &plain()).unwrap();
    assert!(parse_name_map("a = 1\nb = 1\n").is_err() && parse_name_map("a = 1\na = 2\n").is_err() && parse_name_map("nonsense\n").is_err());

    // --- like files
    expect(run(&ani_g, ImportKind::Ani, None, &plain()), RefusalKind::Like, "needs a like file", "no like for .ANI");
    expect(run(&mesh_g, ImportKind::C3dc, None, &plain()), RefusalKind::Like, "needs a like file", "no like for .3DC");
    expect(run(&mesh_g, ImportKind::Mesh3do, Some(&lm), &plain()), RefusalKind::Like, "takes no like", "a like for .3DO");
    let cloak = {
        let mut c = mesh.clone();
        c.layout = shaiya_clientdata::mesh3dc::C3dcLayout::Cloak;
        c.meshes[0].bones.clear();
        c
    };
    expect(run(&mesh_g, ImportKind::C3dc, Some(&Like::C3dc(cloak)), &plain()), RefusalKind::Like, "cloak", "a cloak like");
    let mut two = mesh.clone();
    two.layout = shaiya_clientdata::mesh3dc::C3dcLayout::SkinnedWithSecondMesh;
    two.meshes.push(two.meshes[0].clone());
    expect(run(&mesh_g, ImportKind::C3dc, Some(&Like::C3dc(two)), &plain()), RefusalKind::Unsupported, "mesh nodes", "one mesh for a two-mesh like");

    // --- animation channels
    let mut g = ani_g.clone();
    let n1 = node_index(&g, "Bone_001");
    push_channel(&mut g, n1, "scale", &[0.0, 0.5], &[2.0, 2.0, 2.0, 2.0, 2.0, 2.0]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Scale, "a scale would be lost", "a scale channel");
    let mut g = ani_g.clone();
    push_channel(&mut g, n1, "weights", &[0.0, 0.5], &[0.0, 1.0]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Morph, "morph weights", "morph weights");
    let mut g = ani_g.clone();
    let a0 = g.json["animations"][0].clone();
    g.json["animations"].as_array_mut().unwrap().push(a0);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Animation, "holds 2 animations", "two animations");
    let mut g = ani_g.clone();
    g.json.as_object_mut().unwrap().remove("animations");
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Animation, "no animation", "no animation");
    let mut g = ani_g.clone();
    let mut extra = g.json["nodes"].as_array().unwrap().clone();
    extra.push(json!({"name": "Camera"}));
    g.json["nodes"] = json!(extra);
    let cam = g.array("nodes").len() - 1;
    push_channel(&mut g, cam, "translation", &[0.0], &[0.0, 0.0, 0.0]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Animation, "not a bone of the rig", "another node animated");
    let mut g = ani_g.clone();
    let first = g.json["animations"][0]["samplers"][0]["input"].as_u64().unwrap() as usize;
    let o = view(&g, first);
    g.bin[o..o + 4].copy_from_slice(&(-1.0f32).to_le_bytes());
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Format, "negative", "a negative time");

    // --- the node graph
    let mut g = ani_g.clone();
    let (n0, n1) = (node_index(&g, "Bone_000"), node_index(&g, "Bone_001"));
    g.json["nodes"][n1]["children"] = json!([n0]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Format, "cycle", "cyclic nodes");
    let mut g = ani_g.clone();
    let (n2, n3) = (node_index(&g, "Bone_002"), node_index(&g, "Bone_003"));
    g.json["nodes"][n2]["children"] = json!([n3]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Format, "child of both", "a node with two parents");
    let mut g = ani_g.clone();
    g.json["nodes"][n1]["matrix"] = json!([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    expect(run(&g, ImportKind::Ani, Some(&la), &plain()), RefusalKind::Transform, "both a matrix", "matrix and rotation");

    // --- what the file says it needs and where it reads from (the open step)
    let mut g = ani_g.clone();
    g.json["extensionsRequired"] = json!(["VENDOR_something"]);
    expect_open(reopen(&g), RefusalKind::Extension, "does not know", "an unknown required extension");
    g.json["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    expect_open(reopen(&g), RefusalKind::Extension, "compressed", "draco");
    g.json["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
    expect_open(reopen(&g), RefusalKind::Extension, "quantized", "quantization");
    g.json["extensionsRequired"] = json!([]);
    reopen(&g).unwrap();
    g.json["extensionsUsed"] = json!(["KHR_materials_unlit"]);
    reopen(&g).unwrap();
    for (uri, words) in [
        ("../model.bin", "outside the file's folder"),
        ("sub/model.bin", "outside the file's folder"),
        ("..%2Fmodel.bin", "outside the file's folder"),
        ("/etc/passwd", "outside the file's folder"),
        ("C:\\model.bin", "outside the file's folder"),
        ("c:/model.bin", "outside the file's folder"),
        ("model.bin:stream", "URL"),
        ("http://example.com/model.bin", "URL"),
        ("https://example.com/model.bin", "URL"),
        ("file:///etc/passwd", "URL"),
        ("//server/share/model.bin", "URL"),
        ("", "empty"),
        ("data:application/octet-stream,AAAA", "not base64"),
    ] {
        let mut g = ani_g.clone();
        g.json["buffers"][0]["uri"] = json!(uri);
        expect_open(reopen(&g), RefusalKind::Buffer, words, uri);
    }
    assert_eq!(buffer_name("my%20model.bin").unwrap(), "my model.bin");
    // a data uri is read, and one that holds more than the buffer says is not decoded
    let mut g = ani_g.clone();
    let b64 = {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in g.bin.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..4 {
                if k <= c.len() {
                    s.push(T[((n >> (18 - 6 * k)) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    };
    g.json["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{b64}"));
    let from_data = reopen(&g).unwrap();
    assert_eq!(from_data.bin, g.bin);
    g.json["buffers"][0]["byteLength"] = json!(g.bin.len() / 2);
    g.json["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{b64}"));
    expect_open(reopen(&g), RefusalKind::Buffer, "more bytes than", "an oversized data uri");
    let mut g = ani_g.clone();
    g.json["buffers"][0]["byteLength"] = json!(g.bin.len() + 100);
    expect_open(reopen(&g), RefusalKind::Buffer, "holds", "a buffer shorter than its byteLength");
    let mut g = ani_g.clone();
    g.json["asset"]["version"] = json!("1.0");
    expect_open(reopen(&g), RefusalKind::Format, "not 2.0", "glTF 1");
}

// ---- T14 -------------------------------------------------------------------------------------------------------------------------------

/// A two-bone file built by hand: node 0 `Bone_000` with child node 1 `Bone_001`, no mesh, and the animation the caller adds.
struct Tiny {
    g: Gltf,
}

impl Tiny {
    fn new() -> Tiny {
        let json = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "Bone_000", "children": [1]}, {"name": "Bone_001"}],
            "buffers": [{"byteLength": 4, "uri": "model.bin"}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 4}],
            "accessors": [],
            "animations": [{"samplers": [], "channels": []}],
        });
        Tiny { g: Gltf { json, bin: vec![0; 4], more: Vec::new() } }
    }

    /// A channel on node 1 with its own sampler (`interp` names the interpolation).
    fn channel(&mut self, path: &str, interp: &str, times: &[f32], values: &[f32]) -> &mut Tiny {
        let ty = if path == "rotation" { "VEC4" } else { "VEC3" };
        let (i, o) = (push_accessor(&mut self.g, times, "SCALAR"), push_accessor(&mut self.g, values, ty));
        let a = &mut self.g.json["animations"][0];
        let s = a["samplers"].as_array().unwrap().len();
        a["samplers"].as_array_mut().unwrap().push(json!({"input": i, "output": o, "interpolation": interp}));
        a["channels"].as_array_mut().unwrap().push(json!({"sampler": s, "target": {"node": 1, "path": path}}));
        self
    }
}

fn two_bone_like() -> Ani {
    let b = |parent: i32| BoneSpec { parent, matrix: identity_bits(), rot: vec![], trans: vec![] };
    Ani::parse(&ani_bytes(0, 30, &[b(-1), b(0)]), "like.ani").unwrap()
}

fn rot_at(a: &Ani, bone: usize, key: i32) -> [f32; 4] {
    a.bones[bone].rot.iter().find(|k| k.key == key).unwrap_or_else(|| panic!("no rotation key {key}")).q
}

fn trans_at(a: &Ani, bone: usize, key: i32) -> [f32; 3] {
    a.bones[bone].trans.iter().find(|k| k.key == key).unwrap_or_else(|| panic!("no translation key {key}")).v
}

#[test]
fn sampler_rules_step_cubic_and_24_fps_resampling() {
    let like = Like::Ani(two_bone_like());
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let id = [0.0, 0.0, 0.0, 1.0];
    let quarter = [0.0, 0.0, s, s];
    // STEP: the value of the last key at or before the time holds
    let mut t = Tiny::new();
    t.channel("rotation", "STEP", &[0.0, 0.5, 1.0], &[id, quarter, id].concat());
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
    let near = |a: [f32; 4], b: [f32; 4]| (0..4).map(|i| (a[i] - b[i]).abs()).sum::<f32>() < 1e-5 || (0..4).map(|i| (a[i] + b[i]).abs()).sum::<f32>() < 1e-5;
    assert!(near(rot_at(&a, 1, 14), id) && near(rot_at(&a, 1, 15), quarter) && near(rot_at(&a, 1, 29), quarter) && near(rot_at(&a, 1, 30), id));
    // LINEAR slerps: halfway between the identity and a quarter turn is an eighth of a turn
    let mut t = Tiny::new();
    t.channel("rotation", "LINEAR", &[0.0, 1.0], &[id, quarter].concat());
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
    let eighth = [0.0, 0.0, (std::f32::consts::FRAC_PI_8).sin(), (std::f32::consts::FRAC_PI_8).cos()];
    assert!(near(rot_at(&a, 1, 15), eighth), "{:?}", rot_at(&a, 1, 15));
    // CUBICSPLINE: three values per key (in tangent, value, out tangent); Hermite with the time step between the keys
    let (m, n) = (3.0f32, -3.0f32);
    let mut t = Tiny::new();
    t.channel("translation", "CUBICSPLINE", &[0.0, 1.0], &[0.0, 0.0, 0.0, /* key 0 */ 0.0, 0.0, 0.0, m, 0.0, 0.0, /* key 1 */ n, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
    for k in [0, 7, 15, 22, 30] {
        let s = f64::from(k) / 30.0;
        // the cubic through (0 with out tangent m) and (3 with in tangent n)
        let want = (2.0 * s * s * s - 3.0 * s * s + 1.0) * 0.0 + (s * s * s - 2.0 * s * s + s) * f64::from(m) + (-2.0 * s * s * s + 3.0 * s * s) * 3.0 + (s * s * s - s * s) * f64::from(n);
        assert!((f64::from(trans_at(&a, 1, k)[0]) - want).abs() < 1e-5, "key {k}: {} vs {want}", trans_at(&a, 1, k)[0]);
    }
    assert!((trans_at(&a, 1, 15)[0] - 2.25).abs() < 1e-5);
    // 24 fps input: sampled at every whole key of 30, and said
    let times: Vec<f32> = (0..=24).map(|j| j as f32 / 24.0).collect();
    let values: Vec<f32> = (0..=24).flat_map(|j| [j as f32, 0.0, 0.0]).collect();
    let mut t = Tiny::new();
    t.channel("translation", "LINEAR", &times, &values);
    let (a, report) = run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap();
    let a = ani_of(a);
    assert!(report.notes.iter().any(|n| n.contains("not multiples of 1/30")), "{:?}", report.notes);
    for k in [0, 1, 10, 29, 30] {
        assert!((trans_at(&a, 1, k)[0] - 0.8 * k as f32).abs() < 1e-5, "key {k}");
    }
    assert_eq!(a.bones[1].trans.len(), 31);
    // --reduce keeps the two ends of a straight line, --keys exact refuses a time that is not a whole key
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &ImportOptions { reduce: true, ..plain() }).unwrap().0);
    assert_eq!(a.bones[1].trans.iter().map(|k| k.key).collect::<Vec<_>>(), vec![0, 30]);
    expect(run(&t.g, ImportKind::Ani, Some(&like), &ImportOptions { keys_exact: true, ..plain() }), RefusalKind::Animation, "not a multiple of 1/30", "24 fps with --keys exact");
    // --keys exact refuses a step or a spline, and keeps the file's own keys of a linear channel
    let mut t = Tiny::new();
    t.channel("translation", "STEP", &[0.0, 0.5], &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    expect(run(&t.g, ImportKind::Ani, Some(&like), &ImportOptions { keys_exact: true, ..plain() }), RefusalKind::Animation, "LINEAR", "a step with --keys exact");
    let mut t = Tiny::new();
    t.channel("translation", "LINEAR", &[0.0, 0.5, 1.0], &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 4.0, 0.0, 0.0]);
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &ImportOptions { keys_exact: true, ..plain() }).unwrap().0);
    assert_eq!(a.bones[1].trans.iter().map(|k| k.key).collect::<Vec<_>>(), vec![0, 15, 30]);
    // a scale of 1 within 1e-4 is no scale; a bone that does not move and sits where the like file has it gets no track
    let mut t = Tiny::new();
    t.channel("scale", "LINEAR", &[0.0, 1.0], &[1.00005, 1.0, 1.0, 1.0, 1.0, 0.99995]);
    t.channel("translation", "LINEAR", &[0.0, 1.0], &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
    assert!(a.bones.iter().all(|b| b.rot.is_empty() && b.trans.is_empty()));
    // a mirrored z: a translation in z comes back negated
    let mut t = Tiny::new();
    t.channel("translation", "LINEAR", &[0.0, 1.0], &[0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    let a = ani_of(run(&t.g, ImportKind::Ani, Some(&like), &plain()).unwrap().0);
    assert_eq!(trans_at(&a, 1, 30), [1.0, 2.0, -3.0]);
    // a sparse accessor in the animation is refused
    let mut t = Tiny::new();
    t.channel("translation", "LINEAR", &[0.0, 1.0], &[0.0, 0.0, 0.0, 1.0, 2.0, 3.0]);
    t.g.json["accessors"][1]["sparse"] = json!({"count": 1});
    expect(run(&t.g, ImportKind::Ani, Some(&like), &plain()), RefusalKind::Unsupported, "sparse", "a sparse animation accessor");
}

// ---- T15 -------------------------------------------------------------------------------------------------------------------------------

#[test]
fn malformed_files_are_errors_not_panics() {
    let ani = rig(0, [0, 8, 20]);
    let mesh = mesh_for_rig();
    let o = export_gltf(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![ani.clone()] }, &GltfOptions { binary: true, ..GltfOptions::default() }).unwrap();
    let glb = o.glb.clone().unwrap();
    let no_files = |_: &str| -> Result<Vec<u8>, String> { Err("no files".to_string()) };
    let try_all = |g: &Gltf| {
        for kind in [ImportKind::Mesh3do, ImportKind::Mesh3de] {
            let _ = import_gltf(g, kind, None, &plain());
        }
        let _ = import_gltf(g, ImportKind::Ani, Some(&Like::Ani(ani.clone())), &plain());
        let _ = import_gltf(g, ImportKind::C3dc, Some(&Like::C3dc(mesh.clone())), &plain());
        let _ = import_gltf(g, ImportKind::C3dc, Some(&Like::Ani(ani.clone())), &plain());
    };
    // every truncation of a .glb is refused (the length field no longer matches), none panics
    for cut in (0..glb.len()).step_by(7).chain([glb.len() - 1, 11, 12, 19, 20, 27, 28]) {
        assert!(Gltf::open(&glb[..cut], &mut |n| no_files(n)).is_err(), "cut at {cut}");
    }
    // a chunk length that runs past the file, and one that is not a multiple of 4
    let mut bad = glb.clone();
    bad[12..16].copy_from_slice(&(glb.len() as u32).to_le_bytes());
    assert!(Gltf::open(&bad, &mut |n| no_files(n)).is_err());
    let mut bad = glb.clone();
    let l = u32::from_le_bytes(bad[12..16].try_into().unwrap());
    bad[12..16].copy_from_slice(&(l - 1).to_le_bytes());
    assert!(Gltf::open(&bad, &mut |n| no_files(n)).is_err());
    let mut bad = glb.clone();
    bad[8..12].copy_from_slice(&(glb.len() as u32 + 4).to_le_bytes());
    assert!(Gltf::open(&bad, &mut |n| no_files(n)).is_err());
    // text that is not JSON, JSON that is not an object, a JSON nested far too deep
    for text in [&b""[..], b"{", b"[]", b"null", b"{\"asset\": 1}", b"\xff\xfe"] {
        assert!(Gltf::open(text, &mut |n| no_files(n)).is_err());
    }
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(Gltf::open(deep.as_bytes(), &mut |n| no_files(n)).is_err());
    // an accessor past its view, past the buffer, and with counts that would need gigabytes
    let base = Gltf::open(&glb, &mut |n| no_files(n)).unwrap();
    let pos = base.array("meshes")[0]["primitives"][0]["attributes"]["POSITION"].as_u64().unwrap() as usize;
    for count in [1_000u64, 1 << 30, u64::from(u32::MAX), u64::MAX, 0] {
        let mut g = base.clone();
        g.json["accessors"][pos]["count"] = json!(count);
        let t = std::time::Instant::now();
        try_all(&g);
        assert!(g.accessor(pos).is_err(), "count {count}");
        assert!(t.elapsed().as_secs() < 30, "count {count} took long");
    }
    let mut g = base.clone();
    g.json["bufferViews"][0]["byteOffset"] = json!(u64::MAX);
    try_all(&g);
    let mut g = base.clone();
    g.json["bufferViews"][0]["byteStride"] = json!(u64::MAX);
    try_all(&g);
    let mut g = base.clone();
    g.json["accessors"][pos]["byteOffset"] = json!(u64::MAX);
    try_all(&g);
    let mut g = base.clone();
    g.json["buffers"][0]["byteLength"] = json!(u64::MAX);
    assert!(reopen(&g).is_err());
    // cyclic nodes, a child that does not exist, a node pointing at itself
    for children in [json!([0]), json!([99]), json!(["x"]), json!(-1)] {
        let mut g = base.clone();
        g.json["nodes"][0]["children"] = children;
        try_all(&g);
        assert!(import_gltf(&g, ImportKind::Ani, Some(&Like::Ani(ani.clone())), &plain()).is_err());
    }
    // every skin index, mesh index, sampler and accessor index out of range
    for (path, v) in [("nodes/0/mesh", 99), ("nodes/0/skin", 99), ("skins/0/inverseBindMatrices", 999), ("animations/0/channels/0/sampler", 99), ("animations/0/samplers/0/input", 999), ("animations/0/channels/0/target/node", 999)] {
        let mut g = base.clone();
        let mut cur = &mut g.json;
        for p in path.split('/') {
            cur = match p.parse::<usize>() {
                Ok(i) => &mut cur[i],
                Err(_) => &mut cur[p],
            };
        }
        *cur = json!(v);
        try_all(&g);
    }
    // a few hundred single-byte changes of the container: opened or refused, never a panic
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..400 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut bytes = glb.clone();
        let at = (seed % bytes.len() as u64) as usize;
        bytes[at] ^= 1 << ((seed >> 40) % 8);
        if let Ok(g) = Gltf::open(&bytes, &mut |n| no_files(n)) {
            try_all(&g);
        }
    }
}

// ---- scale, and the checks of `finish` --------------------------------------------------------------------------------------------------

#[test]
fn a_scaled_bone_without_a_rotation_track_survives_and_a_moving_one_is_refused() {
    let q = unit([0.0, 0.3, 0.0, 1.0]);
    let mut m = quat_matrix(q);
    for c in 0..3 {
        m[4 + c] *= 2.0;
    }
    m[12..15].copy_from_slice(&[0.0, 1.0, 0.0]);
    let b = |parent: i32, matrix: [u32; 16], rot: Vec<(i32, [f32; 4])>| BoneSpec { parent, matrix, rot, trans: vec![] };
    // bone 1 carries a scale in its reference matrix and has no rotation track: the game keeps the scale
    let still = Ani::parse(&ani_bytes(0, 30, &[b(-1, identity_bits(), vec![]), b(0, m.map(f32::to_bits), vec![])]), "s.ani").unwrap();
    let g = opened(&export(GltfInput::Anim(still.clone()), "s.ANI"));
    assert!(g.array("animations")[0]["channels"].as_array().unwrap().iter().any(|c| c["target"]["path"] == "scale"), "the file carries the scale");
    let back = ani_of(run(&g, ImportKind::Ani, Some(&Like::Ani(still.clone())), &ImportOptions { keys_exact: true, ..plain() }).unwrap().0);
    assert!(passes(&back, &still));
    assert!(back.bones[1].rot.is_empty() && back.bones[1].trans.is_empty());
    assert_poses_equal(&still, &back, &[0.0, 10.0, 30.0], 1e-5);
    // the same bone with a rotation track that moves it: the exported scale is 1 there, so it imports
    let moving = Ani::parse(&ani_bytes(0, 30, &[b(-1, identity_bits(), vec![]), b(0, m.map(f32::to_bits), vec![(0, [0.0, 0.0, 0.0, 1.0]), (30, unit([0.0, 0.0, 0.3, 1.0]))])]), "s.ani").unwrap();
    let g = opened(&export(GltfInput::Anim(moving.clone()), "s.ANI"));
    let back = ani_of(run(&g, ImportKind::Ani, Some(&Like::Ani(moving.clone())), &ImportOptions { keys_exact: true, ..plain() }).unwrap().0);
    assert!(passes(&back, &moving));
    assert_poses_equal(&moving, &back, &[0.0, 10.0, 30.0], 1e-5);
    // a scale put on a bone that the like file does not scale is refused (the game would drop it)
    let mut g2 = opened(&export(GltfInput::Anim(two_bone_like()), "t.ANI"));
    let n1 = node_index(&g2, "Bone_001");
    push_channel(&mut g2, n1, "scale", &[0.0, 1.0], &[1.5, 1.5, 1.5, 1.5, 1.5, 1.5]);
    expect(run(&g2, ImportKind::Ani, Some(&Like::Ani(two_bone_like())), &plain()), RefusalKind::Scale, "bone 1", "a scale the like file lacks");
}

#[test]
fn finish_checks_the_bones_against_the_like_file() {
    let mesh = mesh_for_rig();
    let o = export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![] }, "t.3DC");
    let (imp, _) = run(&opened(&o), ImportKind::C3dc, Some(&Like::C3dc(mesh.clone())), &plain()).unwrap();
    assert!(finish(&imp, Some(&Like::C3dc(mesh.clone()))).is_ok());
    let mut other = mesh.clone();
    other.meshes[0].bones.pop();
    let e = finish(&imp, Some(&Like::C3dc(other))).unwrap_err();
    assert!(e.kinds().contains(&RefusalKind::Like) && e.to_string().contains("4 bone matrices, the like file has 3"), "{e}");
    let three = Ani::parse(&ani_bytes(0, 10, &(0..3).map(|i| BoneSpec { parent: i - 1, matrix: identity_bits(), rot: vec![], trans: vec![] }).collect::<Vec<_>>()), "r.ani").unwrap();
    assert!(finish(&imp, Some(&Like::Ani(three))).is_err());
}

// ---- the rest of the refusals, and the static placement ---------------------------------------------------------------------------------

#[test]
fn more_refusals_and_the_placement_of_a_static_mesh() {
    let mesh = mesh_for_rig();
    let lm = Like::C3dc(mesh.clone());
    let mesh_g = opened(&export(GltfInput::Mesh { c3dc: mesh.clone(), anims: vec![] }, "t.3DC"));
    let attr = |g: &Gltf, n: &str| g.array("meshes")[0]["primitives"][0]["attributes"][n].as_u64().unwrap() as usize;
    let offset = |g: &Gltf, acc: usize| {
        let a = &g.array("accessors")[acc];
        g.array("bufferViews")[a["bufferView"].as_u64().unwrap() as usize]["byteOffset"].as_u64().unwrap() as usize
    };
    // two primitives in one mesh
    let mut g = mesh_g.clone();
    let p = g.json["meshes"][0]["primitives"][0].clone();
    g.json["meshes"][0]["primitives"].as_array_mut().unwrap().push(p);
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Unsupported, "2 primitives", "two primitives");
    // weights that do not add up to 1
    let mut g = mesh_g.clone();
    let w = offset(&g, attr(&g, "WEIGHTS_0"));
    g.bin[w..w + 4].copy_from_slice(&0.25f32.to_le_bytes());
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::Weight, "add up to", "weights of 0.25 and 0.5");
    // a NaN position names its vertex
    let mut g = mesh_g.clone();
    let p = offset(&g, attr(&g, "POSITION")) + 12 * 3;
    g.bin[p..p + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    expect(run(&g, ImportKind::C3dc, Some(&lm), &plain()), RefusalKind::NotFinite, "vertex 3 holds NaN", "a NaN position");
    // the texture name: the hex of the extras, else the material's name with its NUL
    let mut g = mesh_g.clone();
    g.json["materials"][0].as_object_mut().unwrap().remove("extras");
    let Imported::C3dc(c) = run(&g, ImportKind::C3dc, Some(&lm), &plain()).unwrap().0 else { panic!() };
    assert_eq!(c.meshes[0].texture, b"skin.tga\0");
    g.json["materials"][0].as_object_mut().unwrap().remove("name");
    let Imported::C3dc(c) = run(&g, ImportKind::C3dc, Some(&lm), &plain()).unwrap().0 else { panic!() };
    assert!(c.meshes[0].texture.is_empty(), "no name, no texture");
    // a static mesh: the node's placement is applied (a move of 1 in y and a turn), the vertices keep their order; a mirrored one is refused
    let boxed = Mesh3do::parse(&box_3do(), "b.3do").unwrap();
    let base = opened(&export(GltfInput::Mesh3do(boxed.clone()), "b.3DO"));
    let mn = base.array("nodes").iter().position(|n| n.get("mesh").is_some()).unwrap();
    let mut g = base.clone();
    g.json["nodes"][mn]["translation"] = json!([0.0, 1.0, 0.0]);
    let (imp, report) = run(&g, ImportKind::Mesh3do, None, &plain()).unwrap();
    let Imported::Mesh3do(m) = imp else { panic!() };
    for (a, b) in boxed.vertices.iter().zip(&m.vertices) {
        assert_eq!([a.pos[0], a.pos[1] + 1.0, a.pos[2]], b.pos);
        assert_eq!(a.normal, b.normal, "a move does not change a normal");
    }
    assert!(report.notes.iter().any(|n| n.contains("placement")), "{:?}", report.notes);
    let mut g = base.clone();
    g.json["nodes"][mn]["scale"] = json!([-1.0, 1.0, 1.0]);
    expect(run(&g, ImportKind::Mesh3do, None, &plain()), RefusalKind::Transform, "mirrors", "a mirrored placement");
    // a skin on a static import is refused with the way out
    expect(run(&mesh_g, ImportKind::Mesh3do, None, &plain()), RefusalKind::Unsupported, "has a skin", "a skinned mesh as a .3DO");
}
