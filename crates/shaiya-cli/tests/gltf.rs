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
