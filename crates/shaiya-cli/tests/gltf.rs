//! `shaiya anim export --format gltf|glb` (synthetic files only): the files written, the output rules (exit 2, 3, 4), the refusals with
//! the bone in the reason, `--out-dir`, and the help text.

mod common;

use common::*;
use shaiya_testkit::anim::{ani_bytes, animated_3do, arm_ani, arm_mesh_3dc, box_3de, box_vani, translation_bits, BoneSpec};
use shaiya_testkit::SahBuilder;
use std::path::{Path, PathBuf};

fn export(w: &World, args: &[&str]) -> std::process::Output {
    let mut a = vec!["anim", "export"];
    a.extend_from_slice(args);
    sh(&w.data, &a)
}

fn work(w: &World) -> (PathBuf, PathBuf, PathBuf) {
    let d = w.tmp.path().join("work");
    let (mesh, ani) = (d.join("arm.3DC"), d.join("arm.ANI"));
    write(&mesh, &arm_mesh_3dc());
    write(&ani, &arm_ani(30));
    (d, mesh, ani)
}

fn gltf_of(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_mesh_with_an_animation_becomes_gltf_and_bin_or_one_glb() {
    let w = world();
    let (d, mesh, ani) = work(&w);
    let out_file = d.join("arm.gltf");
    let o = export(&w, &[p(&mesh), "--format", "gltf", "--anim", p(&ani), "-o", p(&out_file)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.starts_with(&format!("wrote {} and {}: glTF 2.0, 3 bones, 1 meshes, 16 vertices, 1 animations", p(&out_file), p(&d.join("arm.bin")))), "{t}");
    let g = gltf_of(&out_file);
    assert_eq!(g["buffers"][0]["uri"], "arm.bin");
    assert_eq!(g["animations"][0]["name"], "arm");
    assert_eq!(g["asset"]["extras"]["shaiya"]["format"], "shaiya-toolkit-gltf-v1");
    // names only: the file name, no folder path, no images
    assert_eq!(g["asset"]["extras"]["shaiya"]["source"], "arm.3DC");
    let text = std::fs::read_to_string(&out_file).unwrap();
    assert!(!text.contains(p(w.tmp.path())) && g.get("images").is_none(), "{text}");
    let bin = std::fs::read(d.join("arm.bin")).unwrap();
    assert_eq!(bin.len() as u64, g["buffers"][0]["byteLength"].as_u64().unwrap());
    // the same as one .glb: the same buffer inside
    let glb = d.join("arm.glb");
    let o = export(&w, &[p(&mesh), "--format", "glb", "--anim", p(&ani), "-o", p(&glb)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let b = std::fs::read(&glb).unwrap();
    assert_eq!(&b[0..4], b"glTF");
    assert!(b.windows(bin.len()).any(|x| x == &bin[..]), "the buffer is inside the .glb");
    assert!(out(&o).contains("arm.glb: glTF 2.0"), "{}", out(&o));
    // an animation alone, a .3DO and a .3DE
    for (name, bytes) in [("arm.ANI", arm_ani(30)), ("box.3do", animated_3do(3, 20)), ("fx.3DE", box_3de(Some(3)))] {
        let input = d.join("in").join(name);
        write(&input, &bytes);
        let target = d.join(format!("{name}.gltf"));
        let o = export(&w, &[p(&input), "--format", "gltf", "-o", p(&target)]);
        assert_eq!(code(&o), 0, "{name}: {}", err(&o));
        assert!(target.exists() && d.join(format!("{name}.bin")).exists(), "{name}");
    }
    // --json
    let out2 = d.join("j.glb");
    let v = json(&export(&w, &[p(&mesh), "--format", "glb", "-o", p(&out2), "--json"]));
    assert_eq!((v["format"].as_str(), v["bones"].as_u64(), v["animations"].as_u64()), (Some("glb"), Some(3), Some(0)));
}

#[test]
fn from_an_archive_the_source_is_the_archive_path() {
    let w = world();
    let c = w.tmp.path().join("cl");
    std::fs::create_dir_all(&c).unwrap();
    SahBuilder::new(0).file(b"Monster/3DC/Mob_A.3DC", &arm_mesh_3dc()).file(b"Monster/ANI/Mob_A_Att1.ANI", &arm_ani(30)).write_to(&c);
    let target = w.tmp.path().join("o.glb");
    let o = export(&w, &["Monster/3DC/Mob_A.3DC", "--from", p(&c), "--format", "glb", "--anim", "Monster/ANI/Mob_A_Att1.ANI", "-o", p(&target)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let b = std::fs::read(&target).unwrap();
    let text = String::from_utf8_lossy(&b);
    assert!(text.contains("\"source\":\"Monster/3DC/Mob_A.3DC\"") && text.contains("Mob_A_Att1"), "{text}");
    assert!(!text.contains(p(w.tmp.path())));
}

#[test]
fn the_output_rules_are_those_of_the_json_export() {
    let w = world();
    let (d, mesh, _) = work(&w);
    // the extension must fit the format (exit 2); nothing is read or written
    let o = export(&w, &[p(&mesh), "--format", "gltf", "-o", p(&d.join("x.glb"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    let o = export(&w, &[p(&mesh), "--format", "glb", "-o", p(&d.join("x.gltf"))]);
    assert_eq!(code(&o), 2);
    // glTF options need glTF; --anim needs a .3DC
    let o = export(&w, &[p(&mesh), "--allow-lossy", "-o", p(&d.join("x.json"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    let o = export(&w, &[p(&d.join("arm.ANI")), "--format", "glb", "--anim", p(&d.join("arm.ANI")), "-o", p(&d.join("y.glb"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    // an existing output is exit 4 (also the .bin), --force replaces
    let target = d.join("a.gltf");
    assert_eq!(code(&export(&w, &[p(&mesh), "--format", "gltf", "-o", p(&target)])), 0);
    let o = export(&w, &[p(&mesh), "--format", "gltf", "-o", p(&target)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    std::fs::remove_file(&target).unwrap();
    let o = export(&w, &[p(&mesh), "--format", "gltf", "-o", p(&target)]);
    assert_eq!(code(&o), 4, "the .bin exists: {}", err(&o));
    assert!(!target.exists(), "nothing is written when the .bin would be replaced");
    assert_eq!(code(&export(&w, &[p(&mesh), "--format", "gltf", "-o", p(&target), "--force"])), 0);
    // inside a client folder (one that holds data.sah) is exit 4
    let inside = w.client.join("a.glb");
    let o = export(&w, &[p(&mesh), "--format", "glb", "-o", p(&inside)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(!inside.exists());
    // a file that is not one of the four kinds is exit 3
    let v = d.join("b.vani");
    write(&v, &box_vani(4, 66));
    assert_eq!(code(&export(&w, &[p(&v), "--format", "glb", "-o", p(&d.join("v.glb"))])), 3);
}

#[test]
fn a_file_that_cannot_be_written_is_exit_3_and_the_reason_names_the_bone() {
    let w = world();
    let d = w.tmp.path().join("bad");
    let mut nan = translation_bits(0.0, 1.0, 0.0);
    nan[5] = f32::NAN.to_bits();
    let ani = d.join("nan.ANI");
    let root = BoneSpec { parent: -1, matrix: translation_bits(0.0, 0.0, 0.0), rot: vec![], trans: vec![] };
    write(&ani, &ani_bytes(0, 10, &[root.clone(), BoneSpec { parent: 0, matrix: nan, rot: vec![], trans: vec![] }]));
    let target = d.join("nan.glb");
    let o = export(&w, &[p(&ani), "--format", "glb", "-o", p(&target)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("bone 1") && err(&o).contains("not-finite"), "{}", err(&o));
    assert!(!target.exists());
    // a NaN vertex names the vertex
    let mut mesh = arm_mesh_3dc();
    // vertex 0 position z: after the texture name (4 + 8), the bone count (4) and the 3 matrices (192), the vertex count (4), x and y (8)
    let at = 4 + 8 + 4 + 192 + 4 + 8;
    mesh[at..at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    let m = d.join("nan.3DC");
    write(&m, &mesh);
    let o = export(&w, &[p(&m), "--format", "glb", "-o", p(&d.join("m.glb"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("vertex 0 holds NaN or an infinity in its position z"), "{}", err(&o));
    // --allow-lossy writes what can be projected and says so
    let mut shear = translation_bits(0.0, 1.0, 0.0);
    shear[1] = 0.5f32.to_bits();
    let s = d.join("shear.ANI");
    write(&s, &ani_bytes(0, 10, &[root, BoneSpec { parent: 0, matrix: shear, rot: vec![], trans: vec![] }]));
    let o = export(&w, &[p(&s), "--format", "glb", "-o", p(&d.join("s.glb"))]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("shear"), "{}", err(&o));
    let o = export(&w, &[p(&s), "--format", "glb", "--allow-lossy", "-o", p(&d.join("s.glb"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("--allow-lossy projected"), "{}", err(&o));
}

#[test]
fn out_dir_writes_one_gltf_and_bin_per_file_and_reports_the_rest() {
    let w = world();
    let d = w.tmp.path().join("many");
    for (name, bytes) in [("arm.ANI", arm_ani(30)), ("arm.3DC", arm_mesh_3dc()), ("box.vani", box_vani(4, 66)), ("box.3do", animated_3do(3, 20))] {
        write(&d.join("in").join(name), &bytes);
    }
    let dest = w.tmp.path().join("dest");
    let o = export(&w, &[p(&d.join("in")), "--format", "gltf", "--out-dir", p(&dest)]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("wrote 3 of 3 (0 failed)"), "{}", out(&o));
    let mut names: Vec<String> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["arm.3DC.bin", "arm.3DC.gltf", "arm.ANI.bin", "arm.ANI.gltf", "box.3do.bin", "box.3do.gltf"]);
    // a second run: every output exists, exit 4 and nothing is replaced
    let o = export(&w, &[p(&d.join("in")), "--format", "gltf", "--out-dir", p(&dest)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    // inside a client folder: refused before anything is read
    let o = export(&w, &[p(&d.join("in")), "--format", "glb", "--out-dir", p(&w.client.join("sub"))]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    // --anim does not go with --out-dir
    assert_eq!(code(&export(&w, &[p(&d.join("in")), "--format", "glb", "--anim", "x.ANI", "--out-dir", p(&w.tmp.path().join("z"))])), 2);
}

#[test]
fn the_help_carries_the_lossy_view_sentence() {
    let w = world();
    let o = sh(&w.data, &["anim", "export", "--help"]);
    let t = out(&o);
    let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("glTF, OBJ and BVH are a lossy view of the game file"), "{t}");
    assert!(flat.contains("the game file's own round trip (JSON) stays byte for byte"), "{t}");
    assert!(flat.contains("an import builds a new file and runs the same checks as `shaiya anim import` before it writes"), "{t}");
    assert!(flat.contains("--format"), "{t}");
}

// ---- import (W13) ------------------------------------------------------------------------------------------------------------------------

fn import(w: &World, args: &[&str]) -> std::process::Output {
    let mut a = vec!["anim", "import"];
    a.extend_from_slice(args);
    sh(&w.data, &a)
}

/// The arm exported as `arm.gltf` (+ `arm.bin`) and `arm.glb` in `d`, the mesh with its animation.
fn exported(w: &World) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let (d, mesh, ani) = work(w);
    let (gltf, glb) = (d.join("arm.gltf"), d.join("arm.glb"));
    std::fs::create_dir_all(d.join("out")).unwrap();
    assert_eq!(code(&export(w, &[p(&mesh), "--format", "gltf", "--anim", p(&ani), "-o", p(&gltf)])), 0);
    assert_eq!(code(&export(w, &[p(&mesh), "--format", "glb", "--anim", p(&ani), "-o", p(&glb)])), 0);
    (d, mesh, ani, gltf, glb)
}

#[test]
fn a_gltf_comes_back_as_the_same_3dc_and_an_ani_that_passes() {
    let w = world();
    let (d, mesh, ani, gltf, glb) = exported(&w);
    // .3DC: byte for byte (from the .gltf with its .bin and from the .glb), with the .3DC and with the rig's .ANI as the like file
    for (input, name) in [(&gltf, "a.3DC"), (&glb, "b.3DC")] {
        let back = d.join("out").join(name);
        let o = import(&w, &[p(input), "-o", p(&back), "--like", p(&mesh)]);
        assert_eq!(code(&o), 0, "{}", err(&o));
        assert_eq!(std::fs::read(&back).unwrap(), arm_mesh_3dc(), "{name}");
        assert!(out(&o).starts_with(&format!("wrote {}: .3DC, 1 meshes, 16 vertices", p(&back))), "{}", out(&o));
        assert!(err(&o).contains("a lossy view of the game file"), "{}", err(&o));
    }
    let o = import(&w, &[p(&glb), "-o", p(&d.join("out").join("c.3DC")), "--like", p(&ani)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    // .ANI: the same poses and it passes (the rig check lines are those of `anim import` of a JSON)
    for extra in [&["--keys", "exact"][..], &[][..], &["--reduce"][..]] {
        let back = d.join("out").join(format!("x{}.ANI", extra.len()));
        let mut args = vec![p(&gltf), "-o", p(&back), "--like", p(&ani)];
        args.extend_from_slice(extra);
        let o = import(&w, &args);
        assert_eq!(code(&o), 0, "{extra:?}: {}", err(&o));
        assert!(out(&o).contains("same skeleton as") && out(&o).contains("passes") && out(&o).contains(".ANI, 3 bones"), "{}", out(&o));
        let cmp = sh(&w.data, &["anim", "diff", p(&ani), p(&back)]);
        assert_eq!(code(&cmp), 0, "{}", out(&cmp));
    }
    // the static kinds
    let box3 = d.join("in").join("box.3do");
    write(&box3, &animated_3do(1, 20));
    let bg = d.join("box.glb");
    assert_eq!(code(&export(&w, &[p(&box3), "--format", "glb", "-o", p(&bg)])), 0);
    let o = import(&w, &[p(&bg), "-o", p(&d.join("out").join("box.3DO"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let de = d.join("in").join("fx.3DE");
    write(&de, &box_3de(None));
    let dg = d.join("fx.glb");
    assert_eq!(code(&export(&w, &[p(&de), "--format", "glb", "-o", p(&dg)])), 0);
    let o = import(&w, &[p(&dg), "-o", p(&d.join("out").join("fx.3de"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(d.join("out").join("fx.3de")).unwrap(), box_3de(None));
    // --json
    let v = json(&import(&w, &[p(&glb), "-o", p(&d.join("out").join("j.ANI")), "--like", p(&ani), "--json"]));
    assert_eq!((v["written"].as_bool(), v["passes"].as_bool(), v["format"].as_str(), v["bones"].as_u64()), (Some(true), Some(true), Some("ani"), Some(3)));
}

#[test]
fn import_without_like_for_ani_or_3dc_exits_2() {
    let w = world();
    let (d, mesh, ani, _, glb) = exported(&w);
    for name in ["x.ANI", "x.3DC"] {
        let target = d.join("out").join(name);
        let o = import(&w, &[p(&glb), "-o", p(&target)]);
        assert_eq!(code(&o), 2, "{name}: {}", err(&o));
        assert!(err(&o).contains("--like is required"), "{}", err(&o));
        assert!(!target.exists());
    }
    // a like file for a static kind, flags of another kind, an output of an unknown kind, a like of the wrong kind: exit 2, nothing written
    let like_m = p(&mesh).to_string();
    let like_a = p(&ani).to_string();
    let cases: Vec<(&str, Vec<&str>, &str)> = vec![
        ("x.3DO", vec!["--like", &like_m], "--like is for .ANI and .3DC only"),
        ("x.ANI", vec!["--like", &like_m], "must be an .ANI file"),
        ("x.3DC", vec!["--like", &like_m, "--reduce"], "--reduce is for .ANI only"),
        ("x.3DC", vec!["--like", &like_m, "--keys", "exact"], "--keys exact is for .ANI only"),
        ("x.ANI", vec!["--like", &like_a, "--reduce-influences"], "--reduce-influences is for .3DC only"),
        ("x.ANI", vec!["--like", &like_a, "--reduce", "--keys", "exact"], "cannot be used together"),
        ("x.3DO", vec!["--map", "m.txt"], "--map"),
        ("x.json", vec![], "-o of a glTF import must end in"),
    ];
    for (name, extra, words) in cases {
        let target = d.join("out").join(name);
        let mut a = vec![p(&glb), "-o", p(&target)];
        a.extend(extra.iter().copied());
        let o = import(&w, &a);
        assert_eq!(code(&o), 2, "{name} {extra:?}: {}", err(&o));
        assert!(err(&o).contains(words), "{name} {extra:?}: {}", err(&o));
        assert!(!target.exists());
    }
}

#[test]
fn an_imported_ani_that_does_not_pass_exits_1_and_writes_nothing() {
    let w = world();
    let (d, _, _, gltf, _) = exported(&w);
    // a like file that has a parent after its child (the game files do not: the validator calls it an error), and a glTF with the same
    // tree: the skeleton is the like file's and the check on the result fails, so nothing is written
    let bone = |parent: i32, y: f32| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot: vec![], trans: vec![] };
    let odd = d.join("in").join("odd.ANI");
    write(&odd, &ani_bytes(0, 30, &[bone(-1, 0.0), bone(2, 1.0), bone(-1, 2.0)]));
    let mut g = gltf_of(&gltf);
    g["nodes"][0].as_object_mut().unwrap().remove("children");
    g["nodes"][1].as_object_mut().unwrap().remove("children");
    g["nodes"][2]["children"] = serde_json::json!([1]);
    g["scenes"][0]["nodes"] = serde_json::json!([0, 2]);
    let oddg = d.join("odd.gltf");
    write(&oddg, serde_json::to_string(&g).unwrap().as_bytes());
    let target = d.join("out").join("n.ANI");
    let o = import(&w, &[p(&oddg), "-o", p(&target), "--like", p(&odd)]);
    assert_eq!(code(&o), 1, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("parent-order") && out(&o).contains("does not pass") && out(&o).contains("nothing written"), "{}", out(&o));
    assert!(!target.exists());
    // --json: the compare keys and written false
    let v = json(&import(&w, &[p(&oddg), "-o", p(&target), "--like", p(&odd), "--json"]));
    assert_eq!((v["passes"].as_bool(), v["written"].as_bool()), (Some(false), Some(false)));
    // a like file with another bone count: the file does not name its bones the same way, exit 3 and nothing written
    let two = d.join("in").join("two.ANI");
    let bone = |parent: i32, y: f32| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot: vec![], trans: vec![] };
    write(&two, &ani_bytes(0, 30, &[bone(-1, 0.0), bone(0, 1.0)]));
    let o = import(&w, &[p(&gltf), "-o", p(&target), "--like", p(&two)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("does not have") && err(&o).contains("(names)"), "{}", err(&o));
    assert!(!target.exists());
}

#[test]
fn the_import_outputs_follow_the_import_rules() {
    let w = world();
    let (d, mesh, ani, gltf, glb) = exported(&w);
    let target = d.join("out").join("o.ANI");
    assert_eq!(code(&import(&w, &[p(&glb), "-o", p(&target), "--like", p(&ani)])), 0);
    // an existing output is exit 4, --force replaces
    let o = import(&w, &[p(&glb), "-o", p(&target), "--like", p(&ani)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(code(&import(&w, &[p(&glb), "-o", p(&target), "--like", p(&ani), "--force"])), 0);
    // the like file cannot be overwritten (the input cannot be named by -o: its extension differs)
    let before = std::fs::read(&ani).unwrap();
    let o = import(&w, &[p(&glb), "-o", p(&ani), "--like", p(&ani), "--force"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(std::fs::read(&ani).unwrap(), before);
    // inside a client folder: exit 4 before anything is read
    let inside = w.client.join("n.3DC");
    let o = import(&w, &[p(&glb), "-o", p(&inside), "--like", p(&mesh)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(!inside.exists());
    // a missing folder is exit 3; a name Windows cannot hold is exit 4
    assert_eq!(code(&import(&w, &[p(&glb), "-o", p(&d.join("nope").join("n.3DC")), "--like", p(&mesh)])), 3);
    assert_eq!(code(&import(&w, &[p(&glb), "-o", p(&d.join("out").join("CON.3DC")), "--like", p(&mesh)])), 4);
    // the like file in an archive (--from)
    let c = w.tmp.path().join("cl");
    std::fs::create_dir_all(&c).unwrap();
    SahBuilder::new(0).file(b"Monster/ANI/Mob_A_Att1.ANI", &arm_ani(30)).write_to(&c);
    let t2 = d.join("out").join("arc.ANI");
    let o = import(&w, &[p(&gltf), "-o", p(&t2), "--like", "Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(code(&import(&w, &[p(&gltf), "-o", p(&d.join("out").join("z.ANI")), "--from", p(&c)])), 2);
}

#[test]
fn a_refused_gltf_is_exit_3_with_its_reason_and_nothing_is_written() {
    let w = world();
    let (d, mesh, ani, gltf, _) = exported(&w);
    let edit = |f: &dyn Fn(&mut serde_json::Value), name: &str| -> PathBuf {
        let mut g = gltf_of(&gltf);
        f(&mut g);
        let path = d.join(name);
        write(&path, serde_json::to_string(&g).unwrap().as_bytes());
        path
    };
    let cases: Vec<(PathBuf, &str)> = vec![
        (edit(&|g| g["extensionsRequired"] = serde_json::json!(["KHR_draco_mesh_compression"]), "draco.gltf"), "compressed"),
        (edit(&|g| g["buffers"][0]["uri"] = serde_json::json!("../arm.bin"), "up.gltf"), "outside the file's folder"),
        (edit(&|g| g["buffers"][0]["uri"] = serde_json::json!("http://example.com/arm.bin"), "url.gltf"), "URL"),
        (edit(&|g| g["buffers"][0]["uri"] = serde_json::json!("missing.bin"), "nofile.gltf"), "cannot be read"),
    ];
    for (input, words) in cases {
        for (name, like) in [("r.ANI", &ani), ("r.3DC", &mesh)] {
            let target = d.join("out").join(name);
            let o = import(&w, &[p(&input), "-o", p(&target), "--like", p(like)]);
            assert_eq!(code(&o), 3, "{words}/{name}: {}", err(&o));
            assert!(err(&o).contains(words), "{words}/{name}: {}", err(&o));
            assert!(!target.exists());
        }
    }
    // a renamed root: the .ANI needs every bone's node, the .3DC only reads the skin (whose joints are still named)
    let renamed = edit(&|g| g["nodes"][0]["name"] = serde_json::json!("Spine"), "renamed.gltf");
    let o = import(&w, &[p(&renamed), "-o", p(&d.join("out").join("r.ANI")), "--like", p(&ani)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("no node (Bone_NNN or --map)"), "{}", err(&o));
    assert!(!d.join("out").join("r.ANI").exists());
    // a primitive that is not triangles
    let lines = edit(&|g| g["meshes"][0]["primitives"][0]["mode"] = serde_json::json!(1), "lines.gltf");
    let o = import(&w, &[p(&lines), "-o", p(&d.join("out").join("l.3DC")), "--like", p(&mesh)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("mode is 1"), "{}", err(&o));
    // --map names a node; a map that names no node is exit 3
    let map = d.join("map.txt");
    write(&map, b"# the root\nSpine = 0\n");
    let target = d.join("out").join("m.ANI");
    let o = import(&w, &[p(&renamed), "-o", p(&target), "--like", p(&ani), "--map", p(&map)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    write(&map, b"Hand = 0\n");
    let o = import(&w, &[p(&renamed), "-o", p(&d.join("out").join("m2.ANI")), "--like", p(&ani), "--map", p(&map)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    // a file that is not glTF at all
    let junk = d.join("junk.glb");
    write(&junk, b"not a glTF");
    let o = import(&w, &[p(&junk), "-o", p(&d.join("out").join("j.3DC")), "--like", p(&mesh)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
}

#[test]
fn import_help_carries_the_lossy_view_sentence_and_the_flags() {
    let w = world();
    let o = sh(&w.data, &["anim", "import", "--help"]);
    let flat = out(&o).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("glTF, OBJ and BVH are a lossy view of the game file"), "{flat}");
    assert!(flat.contains("an import builds a new file and runs the same checks as `shaiya anim import` before it writes"), "{flat}");
    for f in ["--map", "--reduce-influences", "--reduce", "--keys"] {
        assert!(flat.contains(f), "{f}");
    }
}
