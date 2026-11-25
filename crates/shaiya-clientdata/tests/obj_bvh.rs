//! OBJ and BVH (W14): the mapping (axes, winding, v flip, Euler order), the refusals with their reasons, the caps, and the round trips.
//! Synthetic files only. The expected values are worked out here in a different spelling from the library's (hand-written rotation
//! matrices and a hand-written forward kinematics for the BVH; the game's own `pose` for the other side).

use shaiya_clientdata::anim::{locals, mat_mul, passes, pose, quat_matrix, Ani};
use shaiya_clientdata::interchange::bvh::{export_bvh, import_bvh, BvhOptions, SYNTHETIC_ROOT};
use shaiya_clientdata::interchange::import::{finish, ImportKind, Imported};
use shaiya_clientdata::interchange::obj::{export_obj, import_obj, ObjInput, ObjOptions};
use shaiya_clientdata::interchange::{InterchangeError, RefusalKind};
use shaiya_clientdata::mesh3dc::C3dc;
use shaiya_clientdata::mesh3de::Mesh3de;
use shaiya_clientdata::mesh3do::Mesh3do;
use shaiya_testkit::anim::{ani_bytes, c3dc_bytes, mesh3de_bytes, mesh3do_bytes, BoneSpec, Mesh3deSpec, Mesh3doSpec, Vert3deSpec, Vert3doSpec, VertSpec};

// ---- OBJ ----------------------------------------------------------------------------------------------------------------------------

fn unit3(v: [f32; 3]) -> [f32; 3] {
    let n = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    v.map(|c| c / n)
}

/// Six distinct vertices, introduced by the faces in the order of their numbers, with distinct normals and texture coordinates.
fn wood_3do() -> Mesh3do {
    let v = |pos: [f32; 3], n: [f32; 3], uv: [f32; 2]| Vert3doSpec { pos, normal: unit3(n), uv };
    let bytes = mesh3do_bytes(&Mesh3doSpec {
        texture: b"wood.tga\0".to_vec(),
        vertices: vec![
            v([0.0, 0.0, 1.0], [0.0, 0.2, 1.0], [0.1, 0.9]),
            v([1.0, 0.0, 1.5], [0.3, 0.0, 1.0], [0.7, 0.8]),
            v([0.0, 1.0, 2.0], [0.0, 1.0, 0.5], [0.2, 0.15]),
            v([1.5, 1.25, -0.5], [1.0, 1.0, 0.0], [0.65, 0.3]),
            v([-0.75, 0.5, 0.25], [-1.0, 0.2, 0.2], [0.0, 1.0]),
            v([2.0, -1.0, 0.125], [0.0, -1.0, 0.1], [1.0, 0.0]),
        ],
        faces: vec![[0, 1, 2], [2, 1, 3], [3, 4, 5], [0, 4, 5]],
        frame_rate: 0,
        frames: Vec::new(),
    });
    Mesh3do::parse(&bytes, "wood.3do").unwrap()
}

fn text(o: &[u8]) -> &str {
    std::str::from_utf8(o).unwrap()
}

fn lines_with<'a>(t: &'a str, key: &str) -> Vec<Vec<&'a str>> {
    t.lines().filter(|l| l.split_whitespace().next() == Some(key)).map(|l| l.split_whitespace().skip(1).collect()).collect()
}

fn imported_3do(t: &str) -> Mesh3do {
    match import_obj(t.as_bytes(), ImportKind::Mesh3do, None).unwrap_or_else(|e| panic!("refused: {e}")).0 {
        Imported::Mesh3do(m) => m,
        other => panic!("not a .3DO: {other:?}"),
    }
}

fn refused_text(t: &str, kind: ImportKind) -> String {
    match import_obj(t.as_bytes(), kind, None) {
        Ok(_) => panic!("the OBJ was imported"),
        Err(e) => e.list.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("; "),
    }
}

#[test]
fn a_3do_round_trips_through_obj() {
    let m = wood_3do();
    let o = export_obj(ObjInput::Mesh3do(m.clone()), &ObjOptions { source: "item/3do/wood.3DO".into(), mtl_name: "wood.mtl".into() }).unwrap();
    let t = text(&o.obj);
    assert!(t.starts_with("# shaiya-toolkit obj v1\n"), "{t}");
    assert!(t.contains("\nmtllib wood.mtl\n") && t.contains("\no wood\n") && t.contains("\nusemtl wood.tga\n"), "{t}");
    assert!(t.contains("# texture-hex 776f6f642e74676100"), "the raw bytes of the stored name, NUL included");
    assert!(!t.contains("item/3do"), "the source is a file name, not a path");
    // the mapping, checked line by line against the file's own numbers
    let vs = lines_with(t, "v");
    let vts = lines_with(t, "vt");
    let vns = lines_with(t, "vn");
    assert_eq!((vs.len(), vts.len(), vns.len()), (6, 6, 6));
    for (i, v) in m.vertices.iter().enumerate() {
        let f = |s: &str| s.parse::<f32>().unwrap();
        assert_eq!([f(vs[i][0]), f(vs[i][1]), f(vs[i][2])], [v.pos[0], v.pos[1], -v.pos[2]], "vertex {i}");
        assert_eq!([f(vns[i][0]), f(vns[i][1]), f(vns[i][2])], [v.normal[0], v.normal[1], -v.normal[2]], "normal {i}");
        assert_eq!(f(vts[i][0]), v.uv[0]);
        assert!((vts[i][1].parse::<f64>().unwrap() - (1.0 - f64::from(v.uv[1]))).abs() < 1e-12, "v is 1 - v");
    }
    let mtl = text(o.mtl.as_ref().expect("a named texture makes a .mtl"));
    assert!(mtl.lines().any(|l| l == "newmtl wood.tga"));
    assert!(!mtl.contains("map_") && !mtl.contains("Kd"), "names only: no texture file is named");
    // the way back: the same mesh, the same bytes
    let (imp, report) = import_obj(&o.obj, ImportKind::Mesh3do, None).unwrap();
    assert_eq!(finish(&imp, None).unwrap(), m.to_bytes());
    assert_eq!((report.meshes, report.vertices), (1, 6));
    // without the comment line the visible name is the texture
    let plain: String = t.lines().filter(|l| !l.starts_with("# texture-hex")).collect::<Vec<_>>().join("\n");
    assert_eq!(imported_3do(&plain).texture, b"wood.tga\0");
    // a .3DE (no normals) comes back the same, its constant flag kept
    let de = Mesh3de::parse(
        &mesh3de_bytes(&Mesh3deSpec {
            texture: b"fx.tga\0".to_vec(),
            vertices: (0..4).map(|i| Vert3deSpec { pos: [i as f32, 0.5 * i as f32, -1.0 + i as f32], flag: -1, uv: [0.25 * i as f32, 0.5] }).collect(),
            faces: vec![[0, 1, 2], [1, 2, 3]],
            block: None,
        }),
        "fx.3de",
    )
    .unwrap();
    let o = export_obj(ObjInput::Mesh3de(de.clone()), &ObjOptions { source: "fx.3DE".into(), mtl_name: "fx.mtl".into() }).unwrap();
    assert!(lines_with(text(&o.obj), "vn").is_empty());
    let (imp, _) = import_obj(&o.obj, ImportKind::Mesh3de, None).unwrap();
    assert_eq!(finish(&imp, None).unwrap(), de.to_bytes());
}

#[test]
fn a_3dc_goes_out_at_rest_and_is_not_imported() {
    let bone = shaiya_testkit::anim::identity_bits();
    let v = |pos: [f32; 3]| VertSpec { pos, weight: 0.5, bone1: 0, bone2: 1, pad: 3, normal: [0.0, 1.0, 0.0], uv: [pos[0], pos[2]] };
    let c = C3dc::parse(&c3dc_bytes(b"skin.tga\0", &[bone, bone], &[v([0.0, 0.0, 1.0]), v([1.0, 0.0, 2.0]), v([0.0, 1.0, 3.0])], &[[0, 1, 2]]), "t.3dc").unwrap();
    let o = export_obj(ObjInput::Mesh3dc(c), &ObjOptions { source: "t.3DC".into(), mtl_name: "t.mtl".into() }).unwrap();
    let t = text(&o.obj);
    assert_eq!(lines_with(t, "v").len(), 3);
    assert!(lines_with(t, "v")[2] == ["0", "1", "-3"], "{t}");
    assert!(o.report.notes.iter().any(|n| n.contains("bones")), "{:?}", o.report.notes);
    let why = refused_text(t, ImportKind::C3dc);
    assert!(why.contains(".3DO or .3DE"), "{why}");
    assert!(import_obj(t.as_bytes(), ImportKind::Ani, None).is_err());
}

#[test]
fn obj_flips_v_and_winding_and_back() {
    // one triangle, clockwise-front in the game: the cross product of its first two edges is the outward normal (0, 0, 1)
    let m = Mesh3do::parse(
        &mesh3do_bytes(&Mesh3doSpec {
            texture: Vec::new(),
            vertices: vec![
                Vert3doSpec { pos: [0.0, 0.0, 1.0], normal: [0.0, 0.0, 1.0], uv: [0.25, 0.1] },
                Vert3doSpec { pos: [1.0, 0.0, 1.0], normal: [0.0, 0.0, 1.0], uv: [0.5, 0.1] },
                Vert3doSpec { pos: [0.0, 1.0, 1.0], normal: [0.0, 0.0, 1.0], uv: [0.25, 0.3] },
            ],
            faces: vec![[0, 1, 2]],
            frame_rate: 0,
            frames: Vec::new(),
        }),
        "tri.3do",
    )
    .unwrap();
    let o = export_obj(ObjInput::Mesh3do(m.clone()), &ObjOptions::default()).unwrap();
    let t = text(&o.obj);
    assert!(o.mtl.is_none() && !t.contains("mtllib") && !t.contains("usemtl"), "no texture name: no material");
    // the face swaps its last two corners
    assert_eq!(lines_with(t, "f"), vec![vec!["1/1/1", "3/3/3", "2/2/2"]]);
    // in OBJ space (z mirrored) the corners turn counter-clockwise round the mirrored outward normal
    let p: Vec<[f64; 3]> = lines_with(t, "v").iter().map(|l| [l[0].parse().unwrap(), l[1].parse().unwrap(), l[2].parse().unwrap()]).collect();
    let n: Vec<[f64; 3]> = lines_with(t, "vn").iter().map(|l| [l[0].parse().unwrap(), l[1].parse().unwrap(), l[2].parse().unwrap()]).collect();
    let (a, b, c) = (p[0], p[2], p[1]);
    let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    let cross = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    assert!(cross[0] * n[0][0] + cross[1] * n[0][1] + cross[2] * n[0][2] > 0.0, "counter-clockwise and outward: {cross:?} vs {:?}", n[0]);
    // v is 1 - v, and back
    assert_eq!(lines_with(t, "vt")[0][0], "0.25");
    assert!((lines_with(t, "vt")[0][1].parse::<f64>().unwrap() - 0.9).abs() < 1e-6);
    let back = imported_3do(t);
    assert_eq!(back.faces, m.faces);
    for (x, y) in back.vertices.iter().zip(&m.vertices) {
        assert_eq!(x.uv, y.uv, "the texture coordinates come back bit for bit");
        assert_eq!(x.pos, y.pos);
    }
    // an OBJ made by another program says `0.9`, not a long number: v = 0.1 within an f32
    let other = "v 0 0 -1\nv 1 0 -1\nv 0 1 -1\nvt 0.25 0.9\nvt 0.5 0.9\nvt 0.25 0.7\nf 1/1 3/3 2/2\n";
    let m2 = imported_3do(other);
    assert!((m2.vertices[0].uv[1] - 0.1).abs() < 1e-6 && (m2.vertices[2].uv[1] - 0.3).abs() < 1e-6);
    assert_eq!(m2.faces, vec![[0, 1, 2]]);
}

#[test]
fn quads_and_negative_indices_import() {
    // a quad whose diagonal 2-4 (indices 1 and 3) is the shorter one
    let quad = "v 0 0 0\nv 4 0 0\nv 4.5 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 -1\nf -4/-4/-1 -3/-3/-1 -2/-2/-1 -1/-1/-1\n";
    let m = imported_3do(quad);
    assert_eq!(m.vertices.len(), 4);
    assert_eq!(m.faces.len(), 2, "a quad is two triangles");
    let d = |a: usize, b: usize| {
        let (p, q) = (m.vertices[a].pos, m.vertices[b].pos);
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
    };
    let (d02, d13) = (d(0, 2), d(1, 3));
    assert!(d13 < d02, "the fixture's diagonal 1-3 is the shorter: {d13} {d02}");
    // split on it: the triangles (0, 1, 3) and (1, 2, 3) of the OBJ, each with two corners swapped back for the game
    assert_eq!(m.faces, vec![[0, 3, 1], [1, 3, 2]]);
    // the mirror was undone: the third corner is (4.5, 1, 0)
    assert_eq!(m.vertices[2].pos[..2], [4.5, 1.0]);
    // triangles with a negative index and without normals; relative indices count from the end at the time of the face
    let tri = "v 0 0 0\nv 1 0 0\nvt 0 0\nvt 1 0\nv 0 1 0\nvt 0 1\nf -3/-3 -2/-2 -1/-1\n";
    let t = imported_3do(tri);
    assert_eq!(t.faces.len(), 1);
    // an index past the list is a refusal naming the line, as is 0
    assert!(refused_text("v 0 0 0\nvt 0 0\nf 1/1 2/1 1/1\n", ImportKind::Mesh3do).contains("line 3"));
    assert!(refused_text("v 0 0 0\nvt 0 0\nf 0/1 1/1 1/1\n", ImportKind::Mesh3do).contains("index"));
    // more than four corners
    assert!(refused_text("v 0 0 0\nvt 0 0\nf 1/1 1/1 1/1 1/1 1/1\n", ImportKind::Mesh3do).contains("more than 4 corners"));
}

#[test]
fn an_unused_vertex_is_dropped_and_the_vertex_numbers_follow_the_v_lines() {
    // vertex 2 (9 9 9) is in no face; the others keep the order of their `v` lines, not the order of the face that first names them
    let m = imported_3do("v 5 0 0
v 9 9 9
v 1 0 0
v 0 1 0
vt 0 0
f 4/1 3/1 1/1
");
    assert_eq!(m.vertices.iter().map(|v| v.pos[0]).collect::<Vec<_>>(), vec![5.0, 1.0, 0.0]);
    assert_eq!(m.faces, vec![[2, 0, 1]]);
}

#[test]
fn missing_normals_are_computed_and_said() {
    let t = "v 0 0 -1\nv 1 0 -1\nv 0 1 -1\nvt 0 0\nvt 1 0\nvt 0 1\nf 1/1 3/3 2/2\n";
    let (imp, report) = import_obj(t.as_bytes(), ImportKind::Mesh3do, None).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("normals were computed")), "{:?}", report.notes);
    let Imported::Mesh3do(m) = imp else { panic!() };
    // the game-space corners (0,0,1), (1,0,1), (0,1,1) in the game's clockwise order have the outward normal (0, 0, 1)
    for v in &m.vertices {
        assert!((v.normal[0]).abs() < 1e-6 && (v.normal[1]).abs() < 1e-6 && (v.normal[2] - 1.0).abs() < 1e-6, "{:?}", v.normal);
    }
    // a file with the normals keeps them as they are; one with only some computes all of them and says how many were missing
    let some = "v 0 0 -1\nv 1 0 -1\nv 0 1 -1\nvt 0 0\nvt 1 0\nvt 0 1\nvn 0 0 -1\nf 1/1/1 3/3/1 2/2\n";
    let (_, report) = import_obj(some.as_bytes(), ImportKind::Mesh3do, None).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("1 of 3 corners had no normal")), "{:?}", report.notes);
    // a .3DE holds none: nothing is computed
    let (imp, report) = import_obj(t.as_bytes(), ImportKind::Mesh3de, None).unwrap();
    assert!(matches!(imp, Imported::Mesh3de(_)) && !report.notes.iter().any(|n| n.contains("computed")));
}

#[test]
fn missing_uv_or_two_materials_are_refused() {
    let no_uv = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let why = refused_text(no_uv, ImportKind::Mesh3do);
    assert!(why.contains("texture coordinate") && why.contains("line 4"), "{why}");
    let uv_on_two_of_three = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nf 1/1 2/1 3\n";
    assert!(refused_text(uv_on_two_of_three, ImportKind::Mesh3de).contains("texture coordinate"));
    let two = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nusemtl a.tga\nf 1/1 2/1 3/1\nusemtl b.tga\nf 2/1 3/1 4/1\n";
    let why = refused_text(two, ImportKind::Mesh3do);
    assert!(why.contains("2 materials") && why.contains("a.tga") && why.contains("b.tga") && why.contains("one texture name"), "{why}");
    // the same material twice, and a usemtl no face follows, are one material
    let same = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nusemtl a.tga\nf 1/1 2/1 3/1\nusemtl a.tga\nf 2/1 3/1 4/1\nusemtl b.tga\n";
    assert_eq!(imported_3do(same).texture, b"a.tga\0");
    // faces before and after a usemtl are two materials (no material and a.tga)
    let mixed = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nf 1/1 2/1 3/1\nusemtl a.tga\nf 2/1 3/1 4/1\n";
    assert!(refused_text(mixed, ImportKind::Mesh3do).contains("(no material)"));
    // no faces, and an empty file
    assert!(refused_text("v 0 0 0\n", ImportKind::Mesh3do).contains("no faces"));
    assert!(refused_text("", ImportKind::Mesh3do).contains("no faces"));
}

fn grid_obj(vertices: usize) -> String {
    // every face brings three new positions: `vertices` must be a multiple of 3
    let mut s = String::with_capacity(vertices * 24);
    for i in 0..vertices {
        s.push_str(&format!("v {} {} 0\n", i, i % 7));
    }
    s.push_str("vt 0 0\n");
    for f in 0..vertices / 3 {
        s.push_str(&format!("f {}/1 {}/1 {}/1\n", 3 * f + 1, 3 * f + 2, 3 * f + 3));
    }
    s
}

#[test]
fn more_than_65535_vertices_are_refused() {
    // 65,535 distinct corners fit (the game's index is 16 bit); 65,538 do not
    let (imp, report) = import_obj(grid_obj(65_535).as_bytes(), ImportKind::Mesh3de, None).unwrap();
    assert_eq!(report.vertices, 65_535);
    assert!(finish(&imp, None).is_ok());
    let e = import_obj(grid_obj(65_538).as_bytes(), ImportKind::Mesh3de, None).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::Limit]);
    assert!(e.list[0].reason.contains("65535") && e.list[0].what.starts_with("line "), "{}", e.list[0]);
    // the caps on the input: its size, and each list
    let big = vec![b' '; shaiya_clientdata::interchange::obj::MAX_OBJ_BYTES + 1];
    assert_eq!(import_obj(&big, ImportKind::Mesh3do, None).unwrap_err().kinds(), vec![RefusalKind::Limit]);
    // a cancelled import stops
    let cancel = shaiya_core::job::Cancel::new();
    cancel.cancel();
    assert!(import_obj(grid_obj(30).as_bytes(), ImportKind::Mesh3do, Some(&cancel)).unwrap_err().list[0].reason.contains("cancelled"));
}

#[test]
fn obj_import_never_opens_mtllib() {
    // the importer is a function of bytes: the library file has no file access at all
    let source = include_str!("../src/interchange/obj.rs");
    for word in ["std::fs", "File::open", "read_to_string", "OpenOptions", "std::io::Read"] {
        assert!(!source.contains(word), "obj.rs mentions {word}");
    }
    // a path that would reach out of the folder, a name no file has: the import still works and says it did not open it
    for lib in ["mtllib ../../outside/secret.mtl", "mtllib C:\\Windows\\system.mtl", "mtllib ..\\..\\x.mtl", "mtllib missing.mtl other.mtl"] {
        let t = format!("{lib}\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nusemtl wood.tga\nf 1/1 2/1 3/1\n");
        let (imp, report) = import_obj(t.as_bytes(), ImportKind::Mesh3de, None).unwrap();
        let Imported::Mesh3de(m) = imp else { panic!() };
        assert_eq!(m.texture, b"wood.tga\0", "the texture is the usemtl name");
        assert!(report.notes.iter().any(|n| n.contains("was not opened")), "{:?}", report.notes);
    }
}

#[test]
fn the_texture_comes_from_the_name_or_the_hex_and_a_conflict_says_so() {
    let base = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\n";
    let with = |extra: &str| format!("{base}{extra}f 1/1 2/1 3/1\n");
    // the raw bytes (a NUL tail the name does not show) win when they name the same texture
    assert_eq!(imported_3do(&with("# texture-hex 612e74676100beef\nusemtl a.tga\n")).texture, b"a.tga\0\xbe\xef");
    // another texture in the hex: the visible name is used, and a note says so
    let (imp, report) = import_obj(with("# texture-hex 622e74676100\nusemtl a.tga\n").as_bytes(), ImportKind::Mesh3do, None).unwrap();
    let Imported::Mesh3do(m) = imp else { panic!() };
    assert_eq!(m.texture, b"a.tga\0");
    assert!(report.notes.iter().any(|n| n.contains("texture-hex")));
    // the hex alone is read; a name over 255 bytes, text that is not hex and two textures are refused
    assert_eq!(imported_3do(&with("# texture-hex 6100\n")).texture, b"a\0");
    assert!(refused_text(&with(&format!("usemtl {}\n", "x".repeat(300))), ImportKind::Mesh3do).contains("at most 255"));
    assert!(refused_text(&with("# texture-hex zz\n"), ImportKind::Mesh3do).contains("hexadecimal"));
    assert!(refused_text(&with("# texture-hex 6100\n# texture-hex 6200\n"), ImportKind::Mesh3do).contains("more than one texture"));
    // characters cp949 cannot hold
    assert!(refused_text(&with("usemtl \u{1F600}.tga\n"), ImportKind::Mesh3do).contains("cp949"));
}

#[test]
fn malformed_obj_is_an_error_not_a_panic() {
    for t in [
        "v 1 2\n",
        "v a b c\nf 1 2 3\n",
        "v NaN 0 0\n",
        "v 0 0 inf\n",
        "vt 0\n",
        "vn 0 0\n",
        "f 1\n",
        "f 1 2\n",
        "f 1/2/3/4 1 1\n",
        "f //1 1 1\n",
        "f 99999999999999999999/1 1 1\n",
        "f -9223372036854775808/1 1 1\n",
        "v 0 0 0 \\\nv 1 1 1\n",
        "usemtl\nv 0 0 0\n",
    ] {
        let r = import_obj(t.as_bytes(), ImportKind::Mesh3do, None);
        assert!(r.is_err(), "{t:?} was imported");
    }
    // bytes that are not text, a byte order mark, a file of only comments
    assert!(import_obj(&[0xff, 0xfe, 0x00, 0x80], ImportKind::Mesh3do, None).is_err());
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend_from_slice(grid_obj(3).as_bytes());
    assert!(import_obj(&bom, ImportKind::Mesh3do, None).is_ok());
    assert!(import_obj(b"# nothing\n# here\n", ImportKind::Mesh3do, None).is_err());
    // every prefix of a good file and a pseudo-random mix of its lines: an answer, never a panic
    let good = String::from_utf8(export_obj(ObjInput::Mesh3do(wood_3do()), &ObjOptions::default()).unwrap().obj).unwrap();
    for cut in (0..good.len()).step_by(7) {
        let _ = import_obj(&good.as_bytes()[..cut], ImportKind::Mesh3do, None);
    }
    let lines: Vec<&str> = good.lines().collect();
    let mut seed = 12345u64;
    for _ in 0..200 {
        let mut s = String::new();
        for _ in 0..12 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s.push_str(lines[(seed >> 33) as usize % lines.len()]);
            s.push('\n');
        }
        let _ = import_obj(s.as_bytes(), ImportKind::Mesh3do, None);
    }
    // a line of millions of words costs nothing (only its first words are read), and a corner of millions of slashes is refused
    let wide = format!("v 0 0 0{}
v 1 0 0
v 0 1 0
vt 0 0
f 1/1 2/1 3/1
", " 1".repeat(5_000_000));
    assert!(import_obj(wide.as_bytes(), ImportKind::Mesh3do, None).is_ok());
    let slashes = format!("v 0 0 0
vt 0 0
f 1/1 1/1 1{}
", "/1".repeat(1_000_000));
    assert!(import_obj(slashes.as_bytes(), ImportKind::Mesh3do, None).is_err());
    // the list of refusals is capped, the rest counted
    let many = "v x\n".repeat(500);
    let e = import_obj(many.as_bytes(), ImportKind::Mesh3do, None).unwrap_err();
    assert_eq!(e.list.len(), 200);
    assert!(e.more > 0 || e.list.len() == 200);
}

#[test]
fn an_obj_note_says_what_was_left_out() {
    // the frames of a .3DO and a .3DE, and a vertex flag other than -1
    let animated = Mesh3do::parse(&shaiya_testkit::anim::animated_3do(3, 20), "a.3do").unwrap();
    let o = export_obj(ObjInput::Mesh3do(animated), &ObjOptions::default()).unwrap();
    assert!(o.report.notes.iter().any(|n| n.contains("3 vertex frames") && n.contains("not written")), "{:?}", o.report.notes);
    let moving = Mesh3de::parse(&shaiya_testkit::anim::box_3de(Some(4)), "m.3de").unwrap();
    let o = export_obj(ObjInput::Mesh3de(moving), &ObjOptions::default()).unwrap();
    assert!(o.report.notes.iter().any(|n| n.contains("4 frames") && n.contains("not written")), "{:?}", o.report.notes);
    let mut flagged = Mesh3de::parse(&shaiya_testkit::anim::box_3de(None), "f.3de").unwrap();
    flagged.vertices[0].flag = 7;
    flagged.vertices[1].flag = 9;
    let o = export_obj(ObjInput::Mesh3de(flagged), &ObjOptions::default()).unwrap();
    assert!(o.report.notes.iter().any(|n| n.contains("2 vertices have a flag other than -1")), "{:?}", o.report.notes);
}

#[test]
fn lines_that_are_not_read_are_counted() {
    // o and g lines are one mesh; s, l and p lines and an unknown statement are counted in notes
    let t = "o first
g second
v 0 0 0
v 1 0 0
v 0 1 0
vt 0 0
s 1
s off
l 1 2
p 1
foo bar
o third
f 1/1 2/1 3/1
";
    let (imp, report) = import_obj(t.as_bytes(), ImportKind::Mesh3do, None).unwrap();
    let Imported::Mesh3do(m) = imp else { panic!() };
    assert_eq!((m.vertices.len(), m.faces.len()), (3, 1));
    for want in ["2 `s` lines were not read", "1 `l` lines were not read", "1 `p` lines were not read", "1 `foo` lines were not read"] {
        assert!(report.notes.iter().any(|n| n == want), "{want}: {:?}", report.notes);
    }
}

#[test]
fn an_unwritable_mesh_is_refused_with_its_vertex() {
    let mut m = wood_3do();
    m.vertices[3].pos[1] = f32::NAN;
    let e = export_obj(ObjInput::Mesh3do(m), &ObjOptions::default()).unwrap_err();
    assert!(matches!(e, InterchangeError::Refused { format: "OBJ", .. }));
    assert!(e.to_string().contains("cannot be written as OBJ") && e.to_string().contains("vertex 3"), "{e}");
}

// ---- BVH ----------------------------------------------------------------------------------------------------------------------------

fn unit4(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    q.map(|c| c / n)
}

/// The quaternion of a turn of `deg` degrees about the game's axis `axis` (0 = x, 1 = y, 2 = z).
fn about(axis: usize, deg: f32) -> [f32; 4] {
    let (s, c) = ((deg.to_radians() / 2.0).sin(), (deg.to_radians() / 2.0).cos());
    let mut q = [0.0, 0.0, 0.0, c];
    q[axis] = s;
    q
}

/// The matrix of a placement: the rotation of `q` (a `Local` rotation), the translation `t`.
fn place(q: [f32; 4], t: [f32; 3]) -> [f32; 16] {
    let mut m = quat_matrix(unit4(q));
    m[12..15].copy_from_slice(&t);
    m
}

struct B {
    parent: i32,
    local: [f32; 16],
    rot: Vec<(i32, [f32; 4])>,
    trans: Vec<(i32, [f32; 3])>,
}

/// An `.ANI` from `Local` placements: `Bind[i] = Local[i] * Bind[parent]` (rows), so that `locals()` gives the placements back.
fn ani_of(first_field: i32, end_key: i32, bones: &[B]) -> Ani {
    let mut binds: Vec<[f32; 16]> = Vec::new();
    for (i, b) in bones.iter().enumerate() {
        binds.push(match usize::try_from(b.parent).ok().filter(|&p| p < i) {
            Some(p) => mat_mul(&b.local, &binds[p]),
            None => b.local,
        });
    }
    let specs: Vec<BoneSpec> = bones.iter().zip(&binds).map(|(b, m)| BoneSpec { parent: b.parent, matrix: m.map(f32::to_bits), rot: b.rot.clone(), trans: b.trans.clone() }).collect();
    Ani::parse(&ani_bytes(first_field, end_key, &specs), "t.ani").unwrap()
}

fn tracks_rot(keys: &[(i32, [f32; 4])]) -> Vec<(i32, [f32; 4])> {
    keys.iter().map(|(k, q)| (*k, unit4(*q))).collect()
}

/// root (moves and turns), a child that turns, a grandchild with no rotation track (its `Local` rotation holds) and a second child that
/// turns only between keys 5 and 15.
fn pose_rig() -> Ani {
    ani_of(
        0,
        20,
        &[
            B {
                parent: -1,
                local: place([0.0, 0.0, 0.0, 1.0], [0.5, 1.0, -0.25]),
                rot: tracks_rot(&[(0, [0.1, 0.2, 0.3, 0.9]), (10, [0.4, -0.1, 0.5, 0.7]), (20, [-0.3, 0.6, 0.1, 0.5])]),
                trans: vec![(0, [0.5, 1.0, -0.25]), (10, [1.0, 1.5, 0.5]), (20, [0.0, 2.0, 1.0])],
            },
            B { parent: 0, local: place([0.2, 0.1, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: tracks_rot(&[(0, [0.5, 0.1, -0.2, 0.8]), (20, [0.0, 0.7, 0.3, 0.6])]), trans: vec![] },
            B { parent: 1, local: place([0.1, 0.2, 0.3, 0.9], [1.0, 0.5, 0.25]), rot: vec![], trans: vec![] },
            B { parent: 1, local: place([0.0, 0.3, 0.0, 1.0], [-1.0, 0.5, 0.0]), rot: tracks_rot(&[(5, [0.3, 0.3, 0.0, 0.9]), (15, [0.0, 0.5, 0.5, 0.7])]), trans: vec![] },
        ],
    )
}

struct Bvh {
    names: Vec<String>,
    parents: Vec<Option<usize>>,
    offsets: Vec<[f64; 3]>,
    channels: Vec<Vec<String>>,
    frames: Vec<Vec<f64>>,
    frame_time: f64,
}

/// A BVH reader written for this test, independent of the library's.
fn read_bvh(t: &str) -> Bvh {
    let toks: Vec<&str> = t.split_whitespace().collect();
    let mut i = 1; // HIERARCHY
    let mut b = Bvh { names: vec![], parents: vec![], offsets: vec![], channels: vec![], frames: vec![], frame_time: 0.0 };
    let mut stack: Vec<usize> = vec![];
    while toks[i] != "MOTION" {
        match toks[i] {
            "ROOT" | "JOINT" => {
                b.names.push(toks[i + 1].to_string());
                b.parents.push(stack.last().copied());
                b.offsets.push([0.0; 3]);
                b.channels.push(vec![]);
                stack.push(b.names.len() - 1);
                assert_eq!(toks[i + 2], "{");
                i += 3;
            }
            "End" => i += 8, // End Site { OFFSET x y z }
            "OFFSET" => {
                let j = *stack.last().unwrap();
                b.offsets[j] = [toks[i + 1].parse().unwrap(), toks[i + 2].parse().unwrap(), toks[i + 3].parse().unwrap()];
                i += 4;
            }
            "CHANNELS" => {
                let n: usize = toks[i + 1].parse().unwrap();
                let j = *stack.last().unwrap();
                b.channels[j] = toks[i + 2..i + 2 + n].iter().map(|s| s.to_string()).collect();
                i += 2 + n;
            }
            "}" => {
                stack.pop();
                i += 1;
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(toks[i + 1], "Frames:");
    let n: usize = toks[i + 2].parse().unwrap();
    assert_eq!((toks[i + 3], toks[i + 4]), ("Frame", "Time:"));
    b.frame_time = toks[i + 5].parse().unwrap();
    let width: usize = b.channels.iter().map(Vec::len).sum();
    let vals: Vec<f64> = toks[i + 6..].iter().map(|s| s.parse().unwrap()).collect();
    assert_eq!(vals.len(), n * width, "the frames are complete");
    b.frames = vals.chunks(width).map(<[f64]>::to_vec).collect();
    b
}

type M3 = [[f64; 3]; 3];

fn mm(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            o[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

fn rz(d: f64) -> M3 {
    let (s, c) = d.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn rx(d: f64) -> M3 {
    let (s, c) = d.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn ry(d: f64) -> M3 {
    let (s, c) = d.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn apply(m: &M3, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| (0..3).map(|c| m[r][c] * p[c]).sum())
}

/// The world position and rotation of every joint of frame `f`, by the textbook rule: `R = Rz * Rx * Ry`, child position = parent position +
/// parent world rotation * offset; the root takes its position channels as they are.
fn fk(b: &Bvh, f: usize) -> Vec<([f64; 3], M3)> {
    let row = &b.frames[f];
    let mut col = 0;
    let mut out: Vec<([f64; 3], M3)> = Vec::new();
    for j in 0..b.names.len() {
        let has_pos = b.channels[j].iter().any(|c| c.ends_with("position"));
        let pos_ch = if has_pos { Some([row[col], row[col + 1], row[col + 2]]) } else { None };
        let a = col + if has_pos { 3 } else { 0 };
        let local = mm(&mm(&rz(row[a]), &rx(row[a + 1])), &ry(row[a + 2]));
        col += b.channels[j].len();
        out.push(match b.parents[j] {
            None => (pos_ch.unwrap_or(b.offsets[j]), local),
            Some(p) => {
                let (pp, pr) = out[p];
                let o = apply(&pr, b.offsets[j]);
                ([pp[0] + o[0], pp[1] + o[1], pp[2] + o[2]], mm(&pr, &local))
            }
        });
    }
    out
}

fn bone_of(name: &str) -> Option<usize> {
    name.strip_prefix("Bone_")?.parse().ok()
}

#[test]
fn a_known_three_joint_pose_writes_the_expected_channels_zxy() {
    // a chain turned 90 degrees about the game's z, x and y: the mirror (z -> -z) keeps the z turn, reverses the x and y turns
    let ani = ani_of(
        0,
        2,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]), rot: vec![(0, about(2, 90.0))], trans: vec![(0, [0.0, 0.0, 0.0]), (2, [0.0, 0.0, 2.0])] },
            B { parent: 0, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![(0, about(0, 90.0))], trans: vec![] },
            B { parent: 1, local: place([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), rot: vec![(0, about(1, 90.0))], trans: vec![] },
        ],
    );
    let o = export_bvh(&ani, &BvhOptions::default()).unwrap();
    let t = text(&o.text);
    assert!(t.starts_with("HIERARCHY\nROOT Bone_000\n{\n"), "{t}");
    assert!(t.contains("CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\n"));
    assert_eq!(t.matches("CHANNELS 3 Zrotation Xrotation Yrotation\n").count(), 2);
    assert!(t.contains("MOTION\nFrames: 3\nFrame Time: 0.0333333\n"), "{t}");
    let b = read_bvh(t);
    assert_eq!(b.names, ["Bone_000", "Bone_001", "Bone_002"]);
    assert_eq!(b.parents, [None, Some(0), Some(1)]);
    // offsets: the reference translations through the mirror (x, y, -z)
    assert_eq!(b.offsets, [[0.0; 3], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]]);
    // frame 0: root at the origin, Z = 90; bone 1: X = -90; bone 2: Y = -90 (worked out by hand: see the comment above)
    let want = [0.0, 0.0, 0.0, 90.0, 0.0, 0.0, 0.0, -90.0, 0.0, 0.0, 0.0, -90.0];
    assert_eq!(b.frames[0].len(), 12);
    for (g, w) in b.frames[0].iter().zip(&want) {
        assert!((g - w).abs() < 1e-4, "frame 0: {:?} vs {want:?}", b.frames[0]);
    }
    // the root's position moves 0 -> 2 in z, mirrored to -2, one third of the way at key 1... linearly: -1 at key 1
    assert!((b.frames[1][2] + 1.0).abs() < 1e-5 && (b.frames[2][2] + 2.0).abs() < 1e-5, "{:?}", b.frames.iter().map(|f| f[2]).collect::<Vec<_>>());
    assert_eq!(o.report.frames, 3);
}

/// The forward kinematics written out here (R = Rz * Rx * Ry) against the game's own pose at every key, with a point in each bone's frame.
fn assert_bvh_poses_like_the_game(ani: &Ani, tol: f64) {
    let o = export_bvh(ani, &BvhOptions::default()).unwrap();
    let b = read_bvh(text(&o.text));
    let locs = locals(ani);
    let p = [0.3f32, -0.2, 0.5];
    assert_eq!(b.frames.len(), o.report.frames);
    for f in 0..b.frames.len() {
        let world = pose(ani, &locs, (o.report.start_key + f as i32) as f32);
        let j = fk(&b, f);
        for (jn, name) in b.names.iter().enumerate() {
            let bone = bone_of(name).unwrap();
            // the game: p * World (row vector), then the mirror
            let w = &world[bone];
            let g: [f32; 3] = std::array::from_fn(|c| p[0] * w[c] + p[1] * w[4 + c] + p[2] * w[8 + c] + w[12 + c]);
            let want = [f64::from(g[0]), f64::from(g[1]), -f64::from(g[2])];
            // the BVH: pos + R * (mirrored p)
            let got = apply(&j[jn].1, [f64::from(p[0]), f64::from(p[1]), -f64::from(p[2])]);
            let got = [got[0] + j[jn].0[0], got[1] + j[jn].0[1], got[2] + j[jn].0[2]];
            for a in 0..3 {
                assert!((got[a] - want[a]).abs() < tol, "frame {f}, {name}: {got:?} vs {want:?}");
            }
        }
    }
}

#[test]
fn the_bvh_poses_like_the_games_pose() {
    let ani = pose_rig();
    assert_eq!(export_bvh(&ani, &BvhOptions::default()).unwrap().report.frames, 21);
    assert_bvh_poses_like_the_game(&ani, 2e-4);
}

#[test]
fn poses_close_to_gimbal_lock_are_still_reproduced() {
    let qm = |a: [f32; 4], b: [f32; 4]| -> [f32; 4] {
        [
            a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
            a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
            a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ]
    };
    // the second joint pitches to 90 degrees and past it (89.9 ... 90.1, in steps down to 1e-4), with a turn about y and z on top
    let degs = [88.0f32, 89.9, 89.99, 89.999, 89.9999, 90.0, 90.0001, 90.001, 90.01, 90.1, 92.0];
    let rot: Vec<(i32, [f32; 4])> = degs.iter().enumerate().map(|(k, d)| (k as i32 * 2, unit4(qm(qm(about(2, 20.0 + k as f32), about(0, *d)), about(1, 35.0 - 3.0 * k as f32))))).collect();
    let ani = ani_of(
        0,
        20,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![], trans: vec![] },
            B { parent: 0, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.5]), rot, trans: vec![] },
            B { parent: 1, local: place([0.0, 0.0, 0.0, 1.0], [0.5, 1.0, 0.0]), rot: vec![], trans: vec![] },
        ],
    );
    assert_bvh_poses_like_the_game(&ani, 1e-4);
    let (back, _) = import_bvh(&export_bvh(&ani, &BvhOptions::default()).unwrap().text, &ani, None).unwrap();
    assert_poses_equal(&ani, &back, 0..=20, 1e-4);
}

#[test]
fn euler_unwrap_across_180() {
    // a turn about z from 170 to 190 degrees (the second key as the negated quaternion: the other hemisphere)
    let ani = ani_of(
        0,
        20,
        &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]), rot: vec![(0, about(2, 170.0)), (20, about(2, 190.0).map(|c| -c))], trans: vec![] }],
    );
    let b = read_bvh(text(&export_bvh(&ani, &BvhOptions::default()).unwrap().text));
    let z: Vec<f64> = b.frames.iter().map(|f| f[3]).collect();
    for w in z.windows(2) {
        assert!((w[1] - w[0]).abs() < 1.5, "no jump between frames: {w:?}");
    }
    // the import makes the quaternions continuous again: no key is in the other hemisphere than the one before
    let (back, _) = import_bvh(&export_bvh(&ani, &BvhOptions::default()).unwrap().text, &ani, None).unwrap();
    let q: Vec<[f32; 4]> = back.bones[0].rot.iter().map(|k| k.q).collect();
    assert_eq!(q.len(), 21);
    assert!(q.windows(2).all(|w| w[0].iter().zip(&w[1]).map(|(a, b)| a * b).sum::<f32>() > 0.0), "{q:?}");
    assert_poses_equal(&ani, &back, 0..=20, 1e-4);
    assert!((z[0] - 170.0).abs() < 1e-3 && (z[20] - 190.0).abs() < 1e-3, "{} {}", z[0], z[20]);
    // the same through the x and y channels: a long turn about x (0 to 200 degrees in steps below 180) stays continuous
    let ani = ani_of(0, 40, &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: (0..=4).map(|k| (k * 10, about(1, k as f32 * 50.0))).collect(), trans: vec![] }]);
    let b = read_bvh(text(&export_bvh(&ani, &BvhOptions::default()).unwrap().text));
    let y: Vec<f64> = b.frames.iter().map(|f| f[5]).collect();
    for w in y.windows(2) {
        assert!((w[1] - w[0]).abs() < 10.0, "{w:?}");
    }
    // gimbal lock (x = 90 degrees) is counted and the angles stay finite and continuous
    let ani = ani_of(0, 10, &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![(0, about(0, -90.0)), (10, about(0, -90.0))], trans: vec![] }]);
    let o = export_bvh(&ani, &BvhOptions::default()).unwrap();
    assert!(o.report.gimbal_frames > 0 && o.report.notes.iter().any(|n| n.contains("gimbal")));
    assert!(read_bvh(text(&o.text)).frames.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn two_roots_get_a_synthetic_root() {
    let ani = ani_of(
        0,
        4,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), rot: vec![(0, about(2, 30.0)), (4, about(2, 60.0))], trans: vec![] },
            B { parent: 0, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![], trans: vec![] },
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 3.0]), rot: vec![(0, about(0, 10.0))], trans: vec![] },
        ],
    );
    let o = export_bvh(&ani, &BvhOptions::default()).unwrap();
    let t = text(&o.text);
    assert!(t.starts_with(&format!("HIERARCHY\nROOT {SYNTHETIC_ROOT}\n")), "{t}");
    let b = read_bvh(t);
    assert_eq!(b.names, [SYNTHETIC_ROOT, "Bone_000", "Bone_001", "Bone_002"]);
    assert_eq!(b.parents, [None, Some(0), Some(1), Some(0)]);
    assert_eq!(b.channels[0].len(), 6);
    assert!(b.channels[1..].iter().all(|c| c.len() == 3), "the real roots are joints now");
    assert_eq!(b.frames[0].len(), 6 + 9);
    assert!(b.frames.iter().all(|f| f[..6].iter().all(|v| *v == 0.0)), "the synthetic root does not move");
    assert_eq!(b.offsets[3], [0.0, 0.0, -3.0], "a real root's offset is its reference translation, mirrored");
    assert!(o.report.notes.iter().any(|n| n.contains(SYNTHETIC_ROOT)));
    // the same poses through the synthetic root
    let world = pose(&ani, &locals(&ani), 2.0);
    let j = fk(&b, 2);
    let w = &world[1];
    let want = [f64::from(w[12]), f64::from(w[13]), -f64::from(w[14])];
    for a in 0..3 {
        assert!((j[2].0[a] - want[a]).abs() < 1e-4, "{:?} vs {want:?}", j[2].0);
    }
}

#[test]
fn a_translating_non_root_bone_is_refused_unless_dropped() {
    let rig = |trans: Vec<(i32, [f32; 3])>| {
        ani_of(
            0,
            10,
            &[
                B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0]), rot: vec![], trans: vec![] },
                B { parent: 0, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![(0, about(0, 5.0))], trans },
            ],
        )
    };
    let moving = rig(vec![(0, [0.0, 1.0, 0.0]), (10, [0.0, 1.0, 0.5])]);
    let e = export_bvh(&moving, &BvhOptions::default()).unwrap_err();
    let s = e.to_string();
    assert!(s.contains("bone 1") && s.contains("moves relative to its parent") && s.contains("--drop-translation"), "{s}");
    assert_eq!(e.kinds(), vec![RefusalKind::Unsupported]);
    assert!(s.starts_with("cannot be written as BVH") && !s.contains("--allow-lossy"), "{s}");
    let o = export_bvh(&moving, &BvhOptions { drop_translation: true, ..BvhOptions::default() }).unwrap();
    assert_eq!(o.report.dropped, 1);
    assert!((o.report.max_displacement - 0.5).abs() < 1e-6);
    assert!(o.report.notes.iter().any(|n| n.contains("left out")));
    // a track that stays on the offset moves nothing: no refusal
    let still = rig(vec![(0, [0.0, 1.0, 0.0]), (10, [0.0, 1.0, 0.0])]);
    assert_eq!(export_bvh(&still, &BvhOptions::default()).unwrap().report.dropped, 0);
    // with a synthetic root a real root that moves is a non-root bone too
    let two = ani_of(
        0,
        10,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![], trans: vec![(0, [0.0; 3]), (10, [0.0, 0.0, 1.0])] },
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![], trans: vec![] },
        ],
    );
    assert!(export_bvh(&two, &BvhOptions::default()).unwrap_err().to_string().contains("bone 0"));
    // other refusals: no bones, a scale, a reference matrix that is not a placement, a NaN key
    assert!(export_bvh(&Ani { first_field: 0, end_key: 0, bones: vec![] }, &BvhOptions::default()).is_err());
    assert!(export_bvh(&still, &BvhOptions { scale: 0.0, ..BvhOptions::default() }).is_err());
    assert!(export_bvh(&still, &BvhOptions { scale: f64::NAN, ..BvhOptions::default() }).is_err());
    let mut nan = still.clone();
    nan.bones[1].rot[0].q[0] = f32::NAN;
    assert!(export_bvh(&nan, &BvhOptions::default()).unwrap_err().to_string().contains("rotation key 0"));
    let mut sheared = still.clone();
    sheared.bones[0].matrix[4] = 0.5;
    sheared.bones[0].matrix[0] = 1.0;
    assert!(export_bvh(&sheared, &BvhOptions::default()).is_err(), "a bone with no rotation track needs a placement");
    // the file's frames are capped
    let long = ani_of(0, 200_000, &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![], trans: vec![] }]);
    assert_eq!(export_bvh(&long, &BvhOptions::default()).unwrap_err().kinds(), vec![RefusalKind::Limit]);
    // ... and so are frames times channels: 1,000 bones for 10,001 frames are 30 million numbers
    let chain: Vec<B> = (0..1000).map(|i| B { parent: i - 1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.01, 0.0]), rot: vec![], trans: vec![] }).collect();
    let wide = ani_of(0, 10_000, &chain);
    let e = export_bvh(&wide, &BvhOptions::default()).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::Limit]);
    assert!(e.to_string().contains("numbers"), "{e}");
    assert!(export_bvh(&ani_of(0, 100, &chain), &BvhOptions::default()).is_ok(), "the same rig with fewer frames is written");
}

#[test]
fn frame_time_is_one_thirtieth() {
    // first field -1 and a key at -4: frame 0 is key -4 and every key to the end key has a frame
    let ani = ani_of(-1, 6, &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![(-4, about(2, 0.0)), (6, about(2, 100.0))], trans: vec![] }]);
    let o = export_bvh(&ani, &BvhOptions::default()).unwrap();
    let b = read_bvh(text(&o.text));
    assert!((b.frame_time - 1.0 / 30.0).abs() < 1e-6, "{}", b.frame_time);
    assert_eq!(b.frames.len(), 11);
    assert_eq!((o.report.start_key, o.report.frames), (-4, 11));
    assert!((b.frames[10][3] - 100.0).abs() < 1e-3 && b.frames[0][3].abs() < 1e-3);
    // --scale multiplies the offsets and the positions only
    let o = export_bvh(&pose_rig(), &BvhOptions { scale: 100.0, ..BvhOptions::default() }).unwrap();
    let b = read_bvh(text(&o.text));
    assert_eq!(b.offsets[1], [0.0, 100.0, 0.0]);
    assert!((b.frames[0][0] - 50.0).abs() < 1e-4 && (b.frames[0][2] - 25.0).abs() < 1e-4, "{:?}", &b.frames[0][..3]);
}

// ---- BVH import ---------------------------------------------------------------------------------------------------------------------

fn assert_poses_equal(a: &Ani, b: &Ani, keys: std::ops::RangeInclusive<i32>, tol: f32) {
    let (la, lb) = (locals(a), locals(b));
    for k in keys {
        let (pa, pb) = (pose(a, &la, k as f32), pose(b, &lb, k as f32));
        for (bone, (x, y)) in pa.iter().zip(&pb).enumerate() {
            for e in 0..16 {
                assert!((x[e] - y[e]).abs() <= tol, "key {k}, bone {bone}, element {e}: {} vs {}", x[e], y[e]);
            }
        }
    }
}

fn bvh_of(ani: &Ani) -> String {
    String::from_utf8(export_bvh(ani, &BvhOptions::default()).unwrap().text).unwrap()
}

fn refused_bvh(t: &str, like: &Ani) -> String {
    match import_bvh(t.as_bytes(), like, None) {
        Ok(_) => panic!("the BVH was imported"),
        Err(e) => e.list.iter().map(|r| r.to_string()).collect::<Vec<_>>().join("; "),
    }
}

#[test]
fn bvh_round_trips_to_ani_poses_within_1e_4() {
    // a rig with a bone that has no rotation track and a scale in its reference matrix (the export drops the scale, the import leaves the
    // track out again, so the pose is the like file's)
    let mut scaled = place([0.1, 0.2, 0.3, 0.9], [1.0, 0.5, 0.25]);
    for v in scaled.iter_mut().take(3) {
        *v *= 2.0;
    }
    let mut ani = pose_rig();
    // bone 2 as the scaled one: rebuild the rig with it
    let mut bones: Vec<B> = Vec::new();
    let locs = locals(&ani);
    for (i, b) in ani.bones.iter().enumerate() {
        bones.push(B { parent: b.parent, local: locs[i], rot: b.rot.iter().map(|k| (k.key, k.q)).collect(), trans: b.trans.iter().map(|k| (k.key, k.v)).collect() });
    }
    bones[2].local = scaled;
    ani = ani_of(0, 20, &bones);
    let o = export_bvh(&ani, &BvhOptions::default()).unwrap();
    assert!(o.report.notes.iter().any(|n| n.contains("scale")), "{:?}", o.report.notes);
    let (back, report) = import_bvh(&o.text, &ani, None).unwrap();
    assert!(passes(&back, &ani), "the imported animation passes against its like file");
    assert_eq!(back.end_key, 20);
    assert_eq!(back.first_field, ani.first_field);
    assert_poses_equal(&ani, &back, 0..=20, 1e-4);
    assert!(report.keys > 0 && report.bones == 4);
    assert!(back.bones[2].rot.is_empty() && back.bones[2].trans.is_empty(), "the bone with no track keeps its reference matrix");
    assert!(back.bones[1].trans.is_empty() && back.bones[3].trans.is_empty(), "only the root has a position track");
    assert_eq!(back.bones[0].trans.len(), 21);
    assert!(Ani::parse(&back.try_to_bytes().unwrap(), "x.ani").is_ok());
    // a track equal to the like file's placement is left out: an animation that never moves comes back with no keys
    let calm = ani_of(0, 5, &[B { parent: -1, local: place([0.0, 0.1, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![], trans: vec![] }]);
    let (b, r) = import_bvh(bvh_of(&calm).as_bytes(), &calm, None).unwrap();
    assert_eq!((r.keys, b.end_key), (0, 5));
    assert!(b.bones[0].rot.is_empty() && b.bones[0].trans.is_empty());
    // first field -1 and a key at -4: frame 0 is the like file's start key
    let neg = ani_of(-1, 6, &[B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![(-4, about(2, 0.0)), (6, about(2, 100.0))], trans: vec![] }]);
    let (back, _) = import_bvh(bvh_of(&neg).as_bytes(), &neg, None).unwrap();
    assert_eq!(back.bones[0].rot.first().map(|k| k.key), Some(-4));
    assert!(passes(&back, &neg));
    assert_poses_equal(&neg, &back, -4..=6, 1e-4);
    // a synthetic root is dropped
    let two = ani_of(
        0,
        4,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), rot: vec![(0, about(2, 30.0)), (4, about(2, 60.0))], trans: vec![] },
            B { parent: 0, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![(0, about(0, 5.0))], trans: vec![] },
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 3.0]), rot: vec![(0, about(0, 10.0)), (4, about(1, 10.0))], trans: vec![] },
        ],
    );
    let (back, _) = import_bvh(bvh_of(&two).as_bytes(), &two, None).unwrap();
    assert!(passes(&back, &two));
    assert_poses_equal(&two, &back, 0..=4, 1e-4);
}

#[test]
fn bvh_joints_that_do_not_match_the_rig_are_refused() {
    let ani = pose_rig();
    let t = bvh_of(&ani);
    // a joint the rig lacks
    let why = refused_bvh(&t.replace("JOINT Bone_003", "JOINT Bone_007"), &ani);
    assert!(why.contains("1 joints are no bone of the like file") && why.contains("Bone_007"), "{why}");
    // a name that is not Bone_NNN
    assert!(refused_bvh(&t.replace("JOINT Bone_003", "JOINT Arm"), &ani).contains("Arm"));
    // a bone with no joint
    let (fewer, _) = {
        let mut one_less = ani.clone();
        one_less.bones.pop();
        (bvh_of(&one_less), ())
    };
    let why = refused_bvh(&fewer, &ani);
    assert!(why.contains("have no joint") && why.contains("Bone_003"), "{why}");
    // a bone named twice
    let why = refused_bvh(&t.replace("JOINT Bone_003", "JOINT Bone_2"), &ani);
    assert!(why.contains("named twice"), "{why}");
    // another parent than the like file's: the like file's bone 3 hangs on bone 0
    let mut other = ani.clone();
    other.bones[3].parent = 0;
    let why = refused_bvh(&t, &other);
    assert!(why.contains("another parent") && why.contains("bone 3"), "{why}");
    // a width of any number of digits is the same bone
    assert!(import_bvh(t.replace("Bone_001", "Bone_1").as_bytes(), &ani, None).is_ok());
    // every refusal names its kind
    let e = import_bvh(t.replace("JOINT Bone_003", "JOINT Arm").as_bytes(), &ani, None).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::Names]);
    // an offset that is not the like file's reference (an edited or a scaled file)
    let big = String::from_utf8(export_bvh(&ani, &BvhOptions { scale: 100.0, ..BvhOptions::default() }).unwrap().text).unwrap();
    let why = refused_bvh(&big, &ani);
    assert!(why.contains("another offset") && why.contains("--scale"), "{why}");
    // a synthetic root that moves
    let two = ani_of(
        0,
        2,
        &[
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![(0, about(2, 30.0))], trans: vec![] },
            B { parent: -1, local: place([0.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), rot: vec![], trans: vec![] },
        ],
    );
    let t = bvh_of(&two);
    let moved = t.replacen("0.000000 0.000000 0.000000 0.000000 0.000000 0.000000", "1.000000 0.000000 0.000000 0.000000 0.000000 0.000000", 1);
    assert_ne!(moved, t);
    assert!(refused_bvh(&moved, &two).contains(SYNTHETIC_ROOT));
}

#[test]
fn another_channel_order_or_frame_time_is_refused() {
    let ani = pose_rig();
    let t = bvh_of(&ani);
    let why = refused_bvh(&t.replacen("CHANNELS 3 Zrotation Xrotation Yrotation", "CHANNELS 3 Xrotation Yrotation Zrotation", 1), &ani);
    assert!(why.contains("Xrotation Yrotation Zrotation") && why.contains("Zrotation Xrotation Yrotation"), "{why}");
    let why = refused_bvh(&t.replacen("CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation", "CHANNELS 6 Xposition Yposition Zposition Xrotation Yrotation Zrotation", 1), &ani);
    assert!(why.contains("Bone_000"), "{why}");
    // the root with no position channels
    assert!(refused_bvh(&t.replacen("CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation", "CHANNELS 3 Zrotation Xrotation Yrotation", 1), &ani).contains("Bone_000"));
    // the frame time
    for ft in ["0.04", "0.0333", "0.0416667", "0"] {
        let why = refused_bvh(&t.replace("Frame Time: 0.0333333", &format!("Frame Time: {ft}")), &ani);
        assert!(why.contains("not 1/30"), "{ft}: {why}");
    }
    // 1/30 written longer is the same
    assert!(import_bvh(t.replace("Frame Time: 0.0333333", "Frame Time: 0.033333333").as_bytes(), &ani, None).is_ok());
}

#[test]
fn a_non_root_position_channel_is_refused() {
    let ani = pose_rig();
    let t = bvh_of(&ani);
    let why = refused_bvh(&t.replacen("CHANNELS 3 Zrotation Xrotation Yrotation", "CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation", 1), &ani);
    assert!(why.contains("Bone_001") && why.contains("position channel") && why.contains("only the root"), "{why}");
    let e = import_bvh(t.replacen("CHANNELS 3 Zrotation Xrotation Yrotation", "CHANNELS 3 Xposition Yposition Zposition", 1).as_bytes(), &ani, None).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::Unsupported]);
}

#[test]
fn malformed_bvh_is_an_error_not_a_panic() {
    let ani = pose_rig();
    let good = bvh_of(&ani);
    assert!(import_bvh(good.as_bytes(), &ani, None).is_ok());
    for bad in [
        String::new(),
        "HIERARCHY".to_string(),
        "ROOT Bone_000".to_string(),
        good.replace("HIERARCHY", "HIER"),
        good.replacen('{', "", 1),
        good.replacen('}', "", 1),
        good.replace("MOTION", ""),
        good.replace("Frames: 21", "Frames: x"),
        good.replace("Frames: 21", "Frames: 0"),
        good.replace("Frames: 21", "Frames: 20"),
        good.replace("Frames: 21", "Frames: 21000000"),
        good.replace("Frames: 21", "Frames: 18446744073709551615"),
        good.replace("Frames: 21", "Frames: 99999999999999"),
        good.replace("Frame Time:", "Frame Rate:"),
        good.replacen("OFFSET", "OFFSET x", 1),
        good.replacen("OFFSET 0.500000", "OFFSET NaN", 1),
        good.replacen("CHANNELS 6", "CHANNELS 99", 1),
        good.replacen("CHANNELS 6", "CHANNELS", 1),
        good.replacen("End Site", "End Site2", 1),
        format!("{good} 1.0"),
        good.replace("ROOT Bone_000", "JOINT Bone_000"),
        format!("{}ROOT Bone_009 {{ }}", good.split("MOTION").next().unwrap().trim_end().to_string() + "\n"),
    ] {
        assert!(import_bvh(bad.as_bytes(), &ani, None).is_err(), "{:?}", &bad[..bad.len().min(80)]);
    }
    // a value that is not a number, NaN and an infinity in the frames
    let last = good.lines().last().unwrap().to_string();
    for v in ["abc", "NaN", "inf"] {
        let first = last.split_whitespace().next().unwrap();
        let bad = good.replace(&last, &last.replacen(first, v, 1));
        assert!(import_bvh(bad.as_bytes(), &ani, None).is_err(), "{v}");
    }
    // not text
    assert!(import_bvh(&[0xff, 0xfe, 0, 1], &ani, None).is_err());
    // a hierarchy a hundred thousand joints deep: refused at the cap, no stack overflow and no allocation for its frames
    let mut deep = String::from("HIERARCHY\nROOT Bone_000\n{\nOFFSET 0 0 0\nCHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation\n");
    for i in 1..100_000 {
        deep.push_str(&format!("JOINT Bone_{i:06}\n{{\nOFFSET 0 0 0\nCHANNELS 3 Zrotation Xrotation Yrotation\n"));
    }
    let e = import_bvh(deep.as_bytes(), &ani, None).unwrap_err();
    assert_eq!(e.kinds(), vec![RefusalKind::Limit]);
    // a like file with no bones, and a file over the size cap
    assert!(import_bvh(good.as_bytes(), &Ani { first_field: 0, end_key: 0, bones: vec![] }, None).is_err());
    let big = vec![b' '; shaiya_clientdata::interchange::bvh::MAX_BVH_BYTES + 1];
    assert_eq!(import_bvh(&big, &ani, None).unwrap_err().kinds(), vec![RefusalKind::Limit]);
    // prefixes of a good file and a few byte flips: an answer every time
    for cut in (0..good.len()).step_by(13) {
        let _ = import_bvh(&good.as_bytes()[..cut], &ani, None);
    }
    let mut seed = 99u64;
    for _ in 0..200 {
        let mut b = good.clone().into_bytes();
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let at = (seed >> 33) as usize % b.len();
        b[at] = b"{}x-.9 \n"[(seed >> 20) as usize % 8];
        let _ = import_bvh(&b, &ani, None);
    }
    // frames times channels over the cap: refused before a number is read (a chain of 1,001 bones and 20,000 frames are 60 million)
    let chain: Vec<B> = (0..1001).map(|i| B { parent: i - 1, local: place([0.0, 0.0, 0.0, 1.0], [0.0; 3]), rot: vec![], trans: vec![] }).collect();
    let chain = ani_of(0, 5, &chain);
    let text = bvh_of(&chain).replace("Frames: 6", "Frames: 20000");
    let why = refused_bvh(&text, &chain);
    assert!(why.contains("at most 16000000 numbers"), "{why}");
    // cancel
    let cancel = shaiya_core::job::Cancel::new();
    cancel.cancel();
    assert!(import_bvh(good.as_bytes(), &ani, Some(&cancel)).unwrap_err().list[0].reason.contains("cancelled"));
}
