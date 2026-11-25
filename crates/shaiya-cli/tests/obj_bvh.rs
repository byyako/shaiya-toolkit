//! `shaiya anim export --format obj|bvh` and `shaiya anim import IN.obj|IN.bvh` (synthetic files only): the files written, the output rules
//! (exit 2, 3, 4), the refusals with their reasons, exit 1 for an `.ANI` that does not pass, and the help text.

mod common;

use common::*;
use shaiya_testkit::anim::{ani_bytes, animated_3do, arm_ani, arm_mesh_3dc, box_3de, box_3do, box_vani, translation_bits, BoneSpec};
use shaiya_testkit::SahBuilder;
use std::path::{Path, PathBuf};

const LOSSY: &str = "glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes";

fn export(w: &World, args: &[&str]) -> std::process::Output {
    let mut a = vec!["anim", "export"];
    a.extend_from_slice(args);
    sh(&w.data, &a)
}

fn import(w: &World, args: &[&str]) -> std::process::Output {
    let mut a = vec!["anim", "import"];
    a.extend_from_slice(args);
    sh(&w.data, &a)
}

/// `work/` with the arm rig as `arm.ANI`, `arm.3DC`, a `.3DO` with a texture name (`box.3DO`), a plain one (`plain.3DO`) and a `.3DE`.
fn work(w: &World) -> PathBuf {
    let d = w.tmp.path().join("work");
    write(&d.join("arm.ANI"), &arm_ani(30));
    write(&d.join("arm.3DC"), &arm_mesh_3dc());
    write(&d.join("box.3DO"), &animated_3do(1, 20));
    write(&d.join("plain.3DO"), &box_3do());
    write(&d.join("fx.3DE"), &box_3de(None));
    std::fs::create_dir_all(d.join("out")).unwrap();
    d
}

fn bone(parent: i32, y: f32) -> BoneSpec {
    BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot: vec![], trans: vec![] }
}

#[test]
fn an_ani_exports_as_bvh_and_the_meshes_as_obj() {
    let w = world();
    let d = work(&w);
    let bvh = d.join("arm.bvh");
    let o = export(&w, &[p(&d.join("arm.ANI")), "--format", "bvh", "-o", p(&bvh)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).starts_with(&format!("wrote {}: BVH, 3 bones, 31 frames", p(&bvh))), "{}", out(&o));
    assert!(err(&o).contains(LOSSY), "{}", err(&o));
    let t = std::fs::read_to_string(&bvh).unwrap();
    assert!(t.starts_with("HIERARCHY\nROOT Bone_000\n") && t.contains("Frames: 31\nFrame Time: 0.0333333\n"), "{t}");
    assert!(!t.contains(p(w.tmp.path())), "no folder path in the file");
    // --scale and --json
    let big = d.join("big.bvh");
    let v = json(&export(&w, &[p(&d.join("arm.ANI")), "--format", "bvh", "--scale", "100", "-o", p(&big), "--json"]));
    assert_eq!((v["format"].as_str(), v["bones"].as_u64(), v["frames"].as_u64()), (Some("bvh"), Some(3), Some(31)));
    assert!(std::fs::read_to_string(&big).unwrap().contains("OFFSET 0.000000 100.000000 0.000000"));
    // a .3DO with a texture name: an .obj and a .mtl of names
    let obj = d.join("box.obj");
    let o = export(&w, &[p(&d.join("box.3DO")), "--format", "obj", "-o", p(&obj)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).starts_with(&format!("wrote {} and {}: OBJ, 1 meshes, 8 vertices", p(&obj), p(&d.join("box.mtl")))), "{}", out(&o));
    let t = std::fs::read_to_string(&obj).unwrap();
    assert!(t.contains("mtllib box.mtl") && t.contains("usemtl box.tga") && t.contains("\no box\n"), "{t}");
    assert!(std::fs::read_to_string(d.join("box.mtl")).unwrap().contains("newmtl box.tga"));
    assert!(err(&o).contains(LOSSY));
    // a .3DO with no texture name: one file, and --json names the (absent) .mtl
    let plain = d.join("plain.obj");
    let v = json(&export(&w, &[p(&d.join("plain.3DO")), "--format", "obj", "-o", p(&plain), "--json"]));
    assert_eq!((v["format"].as_str(), v["mtl"].is_null(), v["vertices"].as_u64()), (Some("obj"), true, Some(8)));
    assert!(!d.join("plain.mtl").exists());
    // a .3DE and a .3DC at rest
    for (name, want) in [("fx.3DE", "fx.obj"), ("arm.3DC", "arm.obj")] {
        let o = export(&w, &[p(&d.join(name)), "--format", "obj", "-o", p(&d.join(want))]);
        assert_eq!(code(&o), 0, "{name}: {}", err(&o));
    }
    assert!(std::fs::read_to_string(d.join("arm.obj")).unwrap().lines().filter(|l| l.starts_with("v ")).count() == 16);
}

#[test]
fn from_an_archive_the_source_is_the_archive_path() {
    let w = world();
    let c = w.tmp.path().join("cl");
    std::fs::create_dir_all(&c).unwrap();
    SahBuilder::new(0).file(b"Monster/ANI/Mob_A_Att1.ANI", &arm_ani(30)).file(b"Item/3DO/Box.3DO", &animated_3do(1, 20)).write_to(&c);
    let target = w.tmp.path().join("o.bvh");
    assert_eq!(code(&export(&w, &["Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c), "--format", "bvh", "-o", p(&target)])), 0);
    let obj = w.tmp.path().join("o.obj");
    let o = export(&w, &["Item/3DO/Box.3DO", "--from", p(&c), "--format", "obj", "-o", p(&obj)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = std::fs::read_to_string(&obj).unwrap();
    assert!(t.contains("# source Box.3DO") && !t.contains(p(w.tmp.path())), "{t}");
}

#[test]
fn the_export_outputs_follow_the_json_rules() {
    let w = world();
    let d = work(&w);
    let (ani, m3do) = (p(&d.join("arm.ANI")).to_string(), p(&d.join("box.3DO")).to_string());
    // the extension must fit (exit 2), -o is required, one input, no --out-dir, glTF options and BVH options in the wrong place
    for (args, words) in [
        (vec![&ani[..], "--format", "bvh", "-o", p(&d.join("x.obj"))], "must end in .bvh"),
        (vec![&m3do[..], "--format", "obj", "-o", p(&d.join("x.bvh"))], "must end in .obj"),
        (vec![&ani[..], "--format", "bvh"], "give -o"),
        (vec![&ani[..], &ani[..], "--format", "bvh", "-o", p(&d.join("x.bvh"))], "more than one input"),
        (vec![&ani[..], "--format", "bvh", "--out-dir", p(&d.join("o"))], "no --out-dir"),
        (vec![&ani[..], "--format", "bvh", "--allow-lossy", "-o", p(&d.join("x.bvh"))], "--format gltf or glb"),
        (vec![&ani[..], "--format", "bvh", "--anim", &ani[..], "-o", p(&d.join("x.bvh"))], "--anim"),
        (vec![&m3do[..], "--format", "obj", "--scale", "2", "-o", p(&d.join("x.obj"))], "--format bvh"),
        (vec![&ani[..], "--format", "gltf", "--drop-translation", "-o", p(&d.join("x.gltf"))], "--format bvh"),
        (vec![&ani[..], "-o", p(&d.join("x.json")), "--scale", "3"], "--format bvh"),
    ] {
        let o = export(&w, &args);
        assert_eq!(code(&o), 2, "{args:?}: {}", err(&o));
        assert!(err(&o).contains(words), "{args:?}: {}", err(&o));
    }
    assert!(!d.join("x.bvh").exists() && !d.join("x.obj").exists());
    // a file of the wrong kind is exit 3
    assert_eq!(code(&export(&w, &[&m3do[..], "--format", "bvh", "-o", p(&d.join("w.bvh"))])), 3);
    assert_eq!(code(&export(&w, &[&ani[..], "--format", "obj", "-o", p(&d.join("w.obj"))])), 3);
    let v = d.join("b.vani");
    write(&v, &box_vani(4, 66));
    assert_eq!(code(&export(&w, &[p(&v), "--format", "obj", "-o", p(&d.join("v.obj"))])), 3);
    // an existing output is exit 4 (also the .mtl), --force replaces
    let target = d.join("a.bvh");
    assert_eq!(code(&export(&w, &[&ani[..], "--format", "bvh", "-o", p(&target)])), 0);
    assert_eq!(code(&export(&w, &[&ani[..], "--format", "bvh", "-o", p(&target)])), 4);
    assert_eq!(code(&export(&w, &[&ani[..], "--format", "bvh", "-o", p(&target), "--force"])), 0);
    let obj = d.join("a.obj");
    assert_eq!(code(&export(&w, &[&m3do[..], "--format", "obj", "-o", p(&obj)])), 0);
    std::fs::remove_file(&obj).unwrap();
    let o = export(&w, &[&m3do[..], "--format", "obj", "-o", p(&obj)]);
    assert_eq!(code(&o), 4, "the .mtl exists: {}", err(&o));
    assert!(!obj.exists(), "nothing is written when the .mtl would be replaced");
    // inside a client folder is exit 4
    let inside = w.client.join("a.bvh");
    assert_eq!(code(&export(&w, &[&ani[..], "--format", "bvh", "-o", p(&inside)])), 4);
    assert!(!inside.exists());
    // a rig that cannot be written: exit 3 and the bone in the reason
    let moving = d.join("moving.ANI");
    let mut b1 = bone(0, 1.0);
    b1.trans = vec![(0, [0.0, 1.0, 0.0]), (10, [0.0, 1.0, 1.0])];
    write(&moving, &ani_bytes(0, 10, &[bone(-1, 0.0), b1]));
    let o = export(&w, &[p(&moving), "--format", "bvh", "-o", p(&d.join("m.bvh"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("bone 1") && err(&o).contains("--drop-translation"), "{}", err(&o));
    assert!(!d.join("m.bvh").exists());
    let o = export(&w, &[p(&moving), "--format", "bvh", "--drop-translation", "-o", p(&d.join("m.bvh"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("left out"), "{}", err(&o));
}

#[test]
fn the_help_carries_the_lossy_view_sentence() {
    let w = world();
    for cmd in ["export", "import"] {
        let o = sh(&w.data, &["anim", cmd, "--help"]);
        let t = out(&o);
        let flat = t.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(LOSSY), "{cmd}: {t}");
        assert!(flat.contains(".bvh") && flat.contains(".obj"), "{cmd}: {t}");
    }
    let flat = out(&sh(&w.data, &["anim", "export", "--help"])).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(flat.contains("--drop-translation") && flat.contains("--scale"), "{flat}");
}

// ---- BVH import ---------------------------------------------------------------------------------------------------------------------

fn bvh_of(w: &World, d: &Path, name: &str) -> PathBuf {
    let bvh = d.join(format!("{name}.bvh"));
    assert_eq!(code(&export(w, &[p(&d.join(format!("{name}.ANI"))), "--format", "bvh", "-o", p(&bvh)])), 0);
    bvh
}

#[test]
fn a_bvh_comes_back_as_an_ani_that_passes() {
    let w = world();
    let d = work(&w);
    let bvh = bvh_of(&w, &d, "arm");
    let back = d.join("out").join("arm.ANI");
    let o = import(&w, &[p(&bvh), "-o", p(&back), "--like", p(&d.join("arm.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("same skeleton as") && out(&o).contains("passes") && out(&o).contains(".ANI, 3 bones"), "{}", out(&o));
    assert!(err(&o).contains(LOSSY), "{}", err(&o));
    let cmp = sh(&w.data, &["anim", "diff", p(&d.join("arm.ANI")), p(&back)]);
    assert_eq!(code(&cmp), 0, "{}", out(&cmp));
    // the like file in an archive, and --json
    let c = w.tmp.path().join("cl");
    std::fs::create_dir_all(&c).unwrap();
    SahBuilder::new(0).file(b"Monster/ANI/Mob_A_Att1.ANI", &arm_ani(30)).write_to(&c);
    let v = json(&import(&w, &[p(&bvh), "-o", p(&d.join("out").join("arc.ANI")), "--like", "Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c), "--json"]));
    assert_eq!((v["written"].as_bool(), v["passes"].as_bool(), v["format"].as_str(), v["bones"].as_u64()), (Some(true), Some(true), Some("ani"), Some(3)));
}

#[test]
fn bvh_import_without_like_exits_2() {
    let w = world();
    let d = work(&w);
    let bvh = bvh_of(&w, &d, "arm");
    let target = d.join("out").join("x.ANI");
    let o = import(&w, &[p(&bvh), "-o", p(&target)]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert!(err(&o).contains("--like is required") && !target.exists(), "{}", err(&o));
    // other mistakes of the command line: exit 2, nothing written
    let like = p(&d.join("arm.ANI")).to_string();
    let mesh = p(&d.join("arm.3DC")).to_string();
    for (name, extra, words) in [
        ("x.3DO", vec![], "-o of a BVH import must end in .ani"),
        ("x.json", vec![], "-o of a BVH import must end in .ani"),
        ("x.ANI", vec!["--like", &mesh], "must be an .ANI file"),
        ("x.ANI", vec!["--like", &like, "--reduce"], "for a glTF import"),
        ("x.ANI", vec!["--like", &like, "--keys", "exact"], "for a glTF import"),
        ("x.ANI", vec!["--like", &like, "--map", "m.txt"], "for a glTF import"),
    ] {
        let t = d.join("out").join(name);
        let mut a = vec![p(&bvh), "-o", p(&t)];
        a.extend(extra.iter().copied());
        let o = import(&w, &a);
        assert_eq!(code(&o), 2, "{name} {extra:?}: {}", err(&o));
        assert!(err(&o).contains(words), "{name} {extra:?}: {}", err(&o));
        assert!(!t.exists());
    }
}

#[test]
fn a_bvh_import_that_does_not_pass_exits_1_and_writes_nothing() {
    let w = world();
    let d = work(&w);
    // a like file that has a parent after its child (the validator calls it an error): the skeleton is the like file's and the check on the
    // result fails, so nothing is written
    let odd = d.join("odd.ANI");
    write(&odd, &ani_bytes(0, 30, &[bone(-1, 0.0), bone(2, 1.0), bone(-1, 2.0)]));
    let bvh = bvh_of(&w, &d, "odd");
    let target = d.join("out").join("n.ANI");
    let o = import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&odd)]);
    assert_eq!(code(&o), 1, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("parent-order") && out(&o).contains("does not pass") && out(&o).contains("nothing written"), "{}", out(&o));
    assert!(!target.exists());
    let v = json(&import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&odd), "--json"]));
    assert_eq!((v["passes"].as_bool(), v["written"].as_bool()), (Some(false), Some(false)));
    // a like file with another skeleton: exit 3 (the file does not name its bones the same way), nothing written
    let o = import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&d.join("arm.ANI"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("another parent") || err(&o).contains("names"), "{}", err(&o));
    assert!(!target.exists());
    // a file that is not a BVH of this layout is exit 3 with the reason
    let bad = d.join("bad.bvh");
    let bvh = bvh_of(&w, &d, "arm");
    let text = std::fs::read_to_string(&bvh).unwrap().replace("Zrotation Xrotation Yrotation", "Xrotation Yrotation Zrotation");
    write(&bad, text.as_bytes());
    let o = import(&w, &[p(&bad), "-o", p(&target), "--like", p(&d.join("arm.ANI"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("Zrotation Xrotation Yrotation") && !target.exists(), "{}", err(&o));
}

#[test]
fn bvh_import_outputs_follow_the_import_rules() {
    let w = world();
    let d = work(&w);
    let like = d.join("arm.ANI");
    let bvh = bvh_of(&w, &d, "arm");
    let target = d.join("out").join("o.ANI");
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&like)])), 0);
    // an existing output is exit 4, --force replaces
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&like)])), 4);
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&target), "--like", p(&like), "--force"])), 0);
    // the like file cannot be overwritten, even with --force
    let before = std::fs::read(&like).unwrap();
    let o = import(&w, &[p(&bvh), "-o", p(&like), "--like", p(&like), "--force"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(std::fs::read(&like).unwrap(), before);
    // inside a client folder: exit 4 before anything is read
    let inside = w.client.join("n.ANI");
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&inside), "--like", p(&like)])), 4);
    assert!(!inside.exists());
    // a missing folder is exit 3; a name Windows cannot hold is exit 4
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&d.join("nope").join("n.ANI")), "--like", p(&like)])), 3);
    assert_eq!(code(&import(&w, &[p(&bvh), "-o", p(&d.join("out").join("CON.ANI")), "--like", p(&like)])), 4);
    // a missing input is exit 3
    assert_eq!(code(&import(&w, &[p(&d.join("missing.bvh")), "-o", p(&d.join("out").join("m.ANI")), "--like", p(&like)])), 3);
    // no temporary file stays
    let mut names: Vec<String> = std::fs::read_dir(d.join("out")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, vec!["o.ANI".to_string()]);
}

// ---- OBJ import ---------------------------------------------------------------------------------------------------------------------

#[test]
fn an_obj_comes_back_as_the_same_3do_and_3de() {
    let w = world();
    let d = work(&w);
    // the plain box: the vertices come back in the file's order, so the bytes are the same
    let obj = d.join("plain.obj");
    assert_eq!(code(&export(&w, &[p(&d.join("plain.3DO")), "--format", "obj", "-o", p(&obj)])), 0);
    let back = d.join("out").join("plain.3DO");
    let o = import(&w, &[p(&obj), "-o", p(&back)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&back).unwrap(), box_3do());
    assert!(out(&o).starts_with(&format!("wrote {}: .3DO, 1 meshes, 8 vertices", p(&back))), "{}", out(&o));
    assert!(err(&o).contains(LOSSY), "{}", err(&o));
    // a .3DE with a texture name
    let obj = d.join("fx.obj");
    assert_eq!(code(&export(&w, &[p(&d.join("fx.3DE")), "--format", "obj", "-o", p(&obj)])), 0);
    let back = d.join("out").join("fx.3de");
    assert_eq!(code(&import(&w, &[p(&obj), "-o", p(&back)])), 0);
    assert_eq!(std::fs::read(&back).unwrap(), box_3de(None));
    // --json
    let v = json(&import(&w, &[p(&obj), "-o", p(&d.join("out").join("j.3DE")), "--json"]));
    assert_eq!((v["written"].as_bool(), v["format"].as_str(), v["vertices"].as_u64()), (Some(true), Some("3de"), Some(8)));
}

#[test]
fn obj_import_never_opens_mtllib() {
    let w = world();
    let d = work(&w);
    // the .mtl a file names is a folder, a name outside the folder and a path that does not exist: none of them is opened (or needed)
    let body = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nusemtl wood.tga\nf 1/1 2/2 3/3\n";
    std::fs::create_dir_all(d.join("a_folder.mtl")).unwrap();
    for (name, lib) in [("one", "a_folder.mtl"), ("two", "../../elsewhere/x.mtl"), ("three", "missing.mtl")] {
        let obj = d.join(format!("{name}.obj"));
        write(&obj, format!("mtllib {lib}\n{body}").as_bytes());
        let target = d.join("out").join(format!("{name}.3DO"));
        let o = import(&w, &[p(&obj), "-o", p(&target)]);
        assert_eq!(code(&o), 0, "{lib}: {}", err(&o));
        assert!(err(&o).contains("was not opened"), "{}", err(&o));
        let bytes = std::fs::read(&target).unwrap();
        assert!(bytes.windows(9).any(|x| x == b"wood.tga\0"), "the texture is the usemtl name");
    }
}

#[test]
fn a_refused_obj_is_exit_3_with_its_reason_and_nothing_is_written() {
    let w = world();
    let d = work(&w);
    let target = d.join("out").join("r.3DO");
    for (name, text, words) in [
        ("nouv", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n", "texture coordinate"),
        ("two", "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nusemtl a\nf 1/1 2/1 3/1\nusemtl b\nf 1/1 2/1 3/1\n", "2 materials"),
        ("nofaces", "v 0 0 0\n", "no faces"),
        ("five", "v 0 0 0\nvt 0 0\nf 1/1 1/1 1/1 1/1 1/1\n", "more than 4 corners"),
        ("nan", "v NaN 0 0\n", "NaN"),
    ] {
        let obj = d.join(format!("{name}.obj"));
        write(&obj, text.as_bytes());
        let o = import(&w, &[p(&obj), "-o", p(&target)]);
        assert_eq!(code(&o), 3, "{name}: {}", err(&o));
        assert!(err(&o).contains(words), "{name}: {}", err(&o));
        assert!(!target.exists());
    }
    // the kinds an OBJ cannot make: exit 2
    let obj = d.join("nouv.obj");
    for name in ["x.3DC", "x.ANI", "x.json"] {
        let t = d.join("out").join(name);
        let o = import(&w, &[p(&obj), "-o", p(&t)]);
        assert_eq!(code(&o), 2, "{name}: {}", err(&o));
        assert!(err(&o).contains("OBJ import must end in .3do or .3de"), "{name}: {}", err(&o));
    }
    let o = import(&w, &[p(&obj), "-o", p(&target), "--like", p(&d.join("arm.ANI"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    for flag in [vec!["--reduce"], vec!["--keys", "exact"], vec!["--reduce-influences"], vec!["--map", "m.txt"]] {
        let mut a = vec![p(&obj), "-o", p(&target)];
        a.extend(flag.iter().copied());
        let o = import(&w, &a);
        assert_eq!(code(&o), 2, "{flag:?}: {}", err(&o));
        assert!(err(&o).contains("glTF import") || err(&o).contains("is for"), "{flag:?}: {}", err(&o));
    }
    assert!(!target.exists());
    // the output rules: an existing output is exit 4, inside a client folder is exit 4
    let good = d.join("plain.obj");
    assert_eq!(code(&export(&w, &[p(&d.join("plain.3DO")), "--format", "obj", "-o", p(&good)])), 0);
    assert_eq!(code(&import(&w, &[p(&good), "-o", p(&target)])), 0);
    assert_eq!(code(&import(&w, &[p(&good), "-o", p(&target)])), 4);
    assert_eq!(code(&import(&w, &[p(&good), "-o", p(&w.client.join("n.3DO"))])), 4);
    assert!(!w.client.join("n.3DO").exists());
}
