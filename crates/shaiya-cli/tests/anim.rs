//! `shaiya anim ...`: info, validate and compare over files, folders and an archive (synthetic files only; the opt-in tests at the end
//! read a real client and write only into `SHAIYA_SCRATCH`).

mod common;

use common::*;
use shaiya_testkit::anim::{ani_bytes, animated_3do, arm_ani, arm_mesh_3dc, box_3de, box_vani, c3dc_cloak_bytes, identity_bits, mani_bytes, translation_bits, BoneSpec, VertSpec};
use shaiya_testkit::{env_client, env_client_original, env_scratch, SahBuilder};
use std::path::{Path, PathBuf};

fn bone(parent: i32, y: f32) -> BoneSpec {
    BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot: Vec::new(), trans: Vec::new() }
}

/// A three-bone animation like `arm_ani` with a chosen first field.
fn chain(first_field: i32) -> Vec<u8> {
    ani_bytes(first_field, 30, &[bone(-1, 0.0), bone(0, 1.0), bone(1, 2.0)])
}

/// `n` bones in a row, bone `i` at height `i`, each the child of the one before.
fn long_chain(n: usize, matrix_shift: f32) -> Vec<u8> {
    let bones: Vec<BoneSpec> = (0..n).map(|i| bone(i as i32 - 1, i as f32 + matrix_shift)).collect();
    ani_bytes(0, 30, &bones)
}

fn client_with(w: &World, dir: &str, files: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
    let c = w.tmp.path().join(dir);
    std::fs::create_dir_all(&c).unwrap();
    let mut b = SahBuilder::new(0);
    for (name, bytes) in files {
        b = b.file(name.as_bytes(), bytes);
    }
    b.write_to(&c);
    c
}

#[test]
fn info_describes_an_ani_and_a_3dc() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("arm.ANI"), &arm_ani(30));
    write(&d.join("arm.3DC"), &arm_mesh_3dc());
    let o = sh(&w.data, &["anim", "info", p(&d.join("arm.ANI")), p(&d.join("arm.3DC"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("arm.ANI: animation, 3 bones, end key 30 (1.0 s), 2 rotation keys, 0 translation keys, 0 errors, 0 warnings"), "{t}");
    assert!(t.contains("arm.3DC: skinned mesh, 16 vertices, 24 triangles, 3 bones, highest vertex bone 2"), "{t}");
    let v = json(&sh(&w.data, &["anim", "info", p(&d.join("arm.ANI")), p(&d.join("arm.3DC")), "--json"]));
    let a = &v["files"][0];
    for key in
        ["path", "format", "kind", "rig", "race", "bones", "end_key", "seconds", "rotation_keys", "translation_keys", "first_field", "first_field_inferred", "issues"]
    {
        assert!(a.get(key).is_some(), "ani key {key}: {a}");
    }
    assert_eq!((a["format"].as_str(), a["bones"].as_u64(), a["end_key"].as_i64(), a["rotation_keys"].as_u64()), (Some("ani"), Some(3), Some(30), Some(2)));
    assert_eq!(a["first_field_inferred"], "start key");
    // a file on disk says nothing about its kind or rig (those come from an archive path)
    assert!(a["kind"].is_null() && a["rig"].is_null() && !t.contains("Character"), "{a}");
    assert!(a["issues"].as_array().unwrap().is_empty());
    let m = &v["files"][1];
    for key in ["path", "format", "layout", "vertices", "triangles", "bones", "max_vertex_bone"] {
        assert!(m.get(key).is_some(), "3dc key {key}: {m}");
    }
    assert_eq!((m["format"].as_str(), m["layout"].as_str(), m["vertices"].as_u64(), m["triangles"].as_u64()), (Some("3dc"), Some("skinned"), Some(16), Some(24)));
    assert_eq!((m["bones"].as_u64(), m["max_vertex_bone"].as_u64()), (Some(3), Some(2)));
}

#[test]
fn info_names_the_rig_of_an_archive_path_and_shows_a_first_field_as_inferred() {
    let w = world();
    let c = client_with(&w, "c", &[("Character/Human/ANI/huwm_000_x.ANI", chain(-1)), ("Monster/ANI/Mob_X_Att1.ANI", chain(0))]);
    let o = sh(&w.data, &["anim", "info", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("Monster/ANI/Mob_X_Att1.ANI: animation, Monster, 3 bones"), "{t}");
    assert!(t.contains("animation, Character rig huwm (Human), 3 bones") && t.contains("first field -1 (inferred: start key)"), "{t}");
    let v = json(&sh(&w.data, &["anim", "info", "--from", p(&c), "--json"]));
    let rig = v["files"].as_array().unwrap().iter().find(|f| f["rig"] == "huwm").expect("a huwm entry");
    assert_eq!((rig["race"].as_str(), rig["kind"].as_str(), rig["first_field"].as_i64()), (Some("Human"), Some("Character"), Some(-1)));
}

#[test]
fn info_exits_3_for_an_unreadable_file_and_reports_the_others() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("a_good.ani"), &arm_ani(30));
    let mut cut = arm_ani(30);
    cut.truncate(cut.len() - 3);
    write(&d.join("b_cut.ani"), &cut);
    write(&d.join("c_good.3dc"), &arm_mesh_3dc());
    let o = sh(&w.data, &["anim", "info", p(&d)]);
    assert_eq!(code(&o), 3, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("a_good.ani: animation") && t.contains("c_good.3dc: skinned mesh") && t.contains("b_cut.ani: FAILED:"), "{t}");
    let v = json(&sh(&w.data, &["anim", "info", p(&d), "--json"]));
    assert_eq!(v["files"].as_array().unwrap().len(), 3);
    assert!(v["files"][1]["error"].as_str().is_some_and(|e| e.contains("b_cut.ani")), "{v}");
    // a file that is not an animation or a mesh is refused by name, the others still reported
    write(&d.join("notes.txt"), b"text");
    let o = sh(&w.data, &["anim", "info", p(&d.join("a_good.ani")), p(&d.join("notes.txt"))]);
    assert_eq!(code(&o), 3);
    assert!(out(&o).contains("a_good.ani: animation") && out(&o).contains("notes.txt: FAILED: ") && out(&o).contains("not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file"), "{}", out(&o));
}

#[test]
fn info_from_an_archive_takes_patterns() {
    let w = world();
    let c = client_with(
        &w,
        "c",
        &[("Monster/ANI/Mob_A_Att1.ANI", chain(0)), ("Monster/ANI/Mob_B_Att1.ANI", chain(0)), ("Monster/3DC/Mob_A.3DC", arm_mesh_3dc()), ("Item/readme.txt", b"hello".to_vec())],
    );
    // the default: every .ani and .3dc, nothing else
    let o = sh(&w.data, &["anim", "info", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o).lines().count(), 3, "{}", out(&o));
    // patterns are case-insensitive and keep only animation files
    let o = sh(&w.data, &["anim", "info", "--from", p(&c), "monster/ani/mob_a*"]);
    assert_eq!(out(&o).lines().count(), 1, "{}", out(&o));
    assert!(out(&o).starts_with("Monster/ANI/Mob_A_Att1.ANI: animation"), "{}", out(&o));
    let o = sh(&w.data, &["anim", "info", "--from", p(&c), "*"]);
    assert_eq!(out(&o).lines().count(), 3, "a pattern that matches other files does not list them: {}", out(&o));
    assert_eq!(code(&sh(&w.data, &["anim", "info"])), 2);
}

#[test]
fn info_finds_upper_case_extensions_in_a_folder() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("sub/one.ANI"), &chain(0));
    write(&d.join("two.Ani"), &chain(0));
    write(&d.join("three.3DC"), &arm_mesh_3dc());
    write(&d.join("four.ani"), &chain(0));
    write(&d.join("five.3dc"), &arm_mesh_3dc());
    write(&d.join("skip.txt"), b"x");
    let o = sh(&w.data, &["anim", "info", p(&d)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert_eq!(t.lines().count(), 5, "{t}");
    for name in ["sub/one.ANI", "two.Ani", "three.3DC", "four.ani", "five.3dc"] {
        assert!(t.contains(name), "{name} in {t}");
    }
    assert!(!t.contains("skip.txt"));
}

#[test]
fn validate_exits_0_with_warnings_only() {
    let w = world();
    let d = w.tmp.path().join("f");
    // two keys 1 apart: a warning (keys-close), no error
    let mut b = bone(-1, 0.0);
    b.rot = vec![(0, [0.0, 0.0, 0.0, 1.0]), (1, [0.0, 0.0, 0.0, 1.0])];
    write(&d.join("warn.ani"), &ani_bytes(0, 30, &[b]));
    let o = sh(&w.data, &["anim", "validate", p(&d.join("warn.ani"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("warn.ani: keys-close: bone 0") && err(&o).starts_with("warning: "), "{}", err(&o));
    assert!(err(&o).contains("1 file: 0 errors in 0 files, 1 warning, 0 could not be read"), "{}", err(&o));
    let v = json(&sh(&w.data, &["anim", "validate", p(&d.join("warn.ani")), "--json"]));
    assert_eq!((v["errors"].as_u64(), v["warnings"].as_u64()), (Some(0), Some(1)));
    assert_eq!(v["files"][0]["issues"][0]["code"], "keys-close");
    assert_eq!(v["files"][0]["issues"][0]["severity"], "warning");
    // --quiet hides a warning line
    let o = sh(&w.data, &["--quiet", "anim", "validate", p(&d.join("warn.ani"))]);
    assert_eq!(code(&o), 0);
    assert!(!err(&o).contains("warning"), "{}", err(&o));
}

#[test]
fn validate_exits_1_on_an_error_and_names_code_and_bone() {
    let w = world();
    let d = w.tmp.path().join("f");
    let mut b1 = bone(0, 1.0);
    b1.rot = vec![(5, [0.0, 0.0, 0.0, 1.0]), (3, [0.0, 0.0, 0.0, 1.0])];
    write(&d.join("bad.ani"), &ani_bytes(0, 30, &[bone(-1, 0.0), b1]));
    write(&d.join("good.ani"), &arm_ani(30));
    let o = sh(&w.data, &["anim", "validate", p(&d)]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(err(&o).contains("error: bad.ani: keys-not-increasing: bone 1"), "{}", err(&o));
    assert!(!err(&o).contains("good.ani"), "{}", err(&o));
    // --quiet does not hide an error
    let q = sh(&w.data, &["--quiet", "anim", "validate", p(&d)]);
    assert_eq!(code(&q), 1);
    assert!(err(&q).contains("error: bad.ani: keys-not-increasing: bone 1"), "{}", err(&q));
    assert!(err(&o).contains("2 files: 1 error in 1 file"), "{}", err(&o));
    let v = json(&sh(&w.data, &["anim", "validate", p(&d), "--json"]));
    assert_eq!(v["errors"], 1);
    assert_eq!(v["files"][0]["issues"][0]["bone"], 1);
    // an unreadable file wins over a failed check: exit 3; a .3DC is not checked here
    write(&d.join("cut.ani"), &[1, 2, 3]);
    assert_eq!(code(&sh(&w.data, &["anim", "validate", p(&d)])), 3);
    write(&d.join("m.3dc"), &arm_mesh_3dc());
    let o = sh(&w.data, &["anim", "validate", p(&d.join("m.3dc"))]);
    assert_eq!(code(&o), 3);
    assert!(out(&o).contains("not an .ANI file"), "{}", out(&o));
    // from an archive the default is every .ani
    let c = client_with(&w, "c", &[("Monster/ANI/Mob_A_Att1.ANI", chain(0)), ("Monster/3DC/Mob_A.3DC", arm_mesh_3dc())]);
    let o = sh(&w.data, &["anim", "validate", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("1 file: 0 errors"), "{}", err(&o));
}

#[test]
fn compare_same_skeleton_and_first_field_passes_with_exit_0() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("target.ani"), &arm_ani(30));
    write(&d.join("new.ani"), &arm_ani(60)); // other keys, same skeleton
    let o = sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("same skeleton as ") && t.contains("3 bones, parents and reference matrices identical"), "{t}");
    assert!(t.contains("first field: same (0)"), "{t}");
    assert_eq!(t.lines().last(), Some("passes"), "{t}");
}

#[test]
fn compare_lists_each_difference_kind_and_exits_1() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("target.ani"), &chain(0));
    // a parent differs (bone 2 under the root), a matrix bit differs (bone 1), the bone count differs (4 bones)
    let mut bones = vec![bone(-1, 0.0), bone(0, 1.5), bone(0, 2.0), bone(2, 3.0)];
    bones[1].matrix[3] = identity_bits()[3] ^ 1;
    write(&d.join("new.ani"), &ani_bytes(0, 30, &bones));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("the skeleton differs from ") && !t.contains("same skeleton"), "{t}");
    assert!(t.contains("bone count: candidate 4, target 3"), "{t}");
    assert!(t.contains("bone 2: parent is 0, target 1"), "{t}");
    assert!(t.contains("bone 1: reference matrix differs"), "{t}");
    assert_eq!(t.lines().last(), Some("does not pass"), "{t}");
    // at most 10 of one kind are listed, then "and N more"
    write(&d.join("long_target.ani"), &long_chain(15, 0.0));
    write(&d.join("long_new.ani"), &long_chain(15, 0.5));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("long_new.ani")), p(&d.join("long_target.ani"))]);
    assert_eq!(code(&o), 1);
    let t = out(&o);
    assert_eq!(t.matches("reference matrix differs").count(), 10, "{t}");
    assert!(t.contains("and 5 more reference matrix differences"), "{t}");
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("long_new.ani")), p(&d.join("long_target.ani")), "--json"]));
    assert_eq!(v["differences"].as_array().unwrap().len(), 15, "the JSON lists every difference");
}

#[test]
fn compare_a_first_field_difference_is_its_own_line_and_does_not_pass() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("target.ani"), &chain(-1));
    write(&d.join("new.ani"), &chain(0));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("same skeleton as "), "{t}");
    assert!(t.contains("first field differs: candidate 0, target -1 (inferred: the start key; not checked in the game)"), "{t}");
    assert_eq!(t.lines().last(), Some("does not pass"), "{t}");
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani")), "--json"]));
    assert_eq!((v["same_skeleton"].as_bool(), v["same_first_field"].as_bool(), v["passes"].as_bool()), (Some(true), Some(false), Some(false)));
    assert_eq!(v["differences"][0]["kind"], "first_field");
    assert_eq!(v["differences"][0]["skeleton"], false);
    assert_eq!(v["differences"][0]["inferred"], "start key");
}

#[test]
fn compare_a_candidate_with_an_error_does_not_pass() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("target.ani"), &chain(0));
    let mut b1 = bone(0, 1.0);
    b1.rot = vec![(5, [0.0, 0.0, 0.0, 1.0]), (3, [0.0, 0.0, 0.0, 1.0])];
    write(&d.join("new.ani"), &ani_bytes(0, 30, &[bone(-1, 0.0), b1, bone(1, 2.0)]));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("same skeleton as ") && t.contains("first field: same (0)"), "{t}");
    assert!(t.contains("the candidate has 1 validation error:") && t.contains("keys-not-increasing: bone 1"), "{t}");
    assert_eq!(t.lines().last(), Some("does not pass"), "{t}");
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani")), "--json"]));
    assert_eq!((v["same_skeleton"].as_bool(), v["same_first_field"].as_bool(), v["passes"].as_bool()), (Some(true), Some(true), Some(false)));
    assert_eq!(v["candidate_issues"][0]["code"], "keys-not-increasing");
}

#[test]
fn compare_with_from_reads_the_target_in_the_archive() {
    let w = world();
    let c = client_with(&w, "c", &[("Character/Human/ANI/huwm_000_x.ANI", arm_ani(30))]);
    let d = w.tmp.path().join("f");
    write(&d.join("new.ani"), &arm_ani(90));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), "Character/Human/ANI/huwm_000_x.ANI", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("same skeleton as Character/Human/ANI/huwm_000_x.ANI: 3 bones"), "{}", out(&o));
    // a path that is not in the archive, and a candidate that does not read, are input errors
    assert_eq!(code(&sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), "Character/none.ANI", "--from", p(&c)])), 3);
    write(&d.join("cut.ani"), &[1, 2, 3, 4]);
    assert_eq!(code(&sh(&w.data, &["anim", "compare", p(&d.join("cut.ani")), "Character/Human/ANI/huwm_000_x.ANI", "--from", p(&c)])), 3);
}

#[test]
fn compare_json_has_the_documented_keys() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("target.ani"), &chain(0));
    write(&d.join("new.ani"), &long_chain(2, 0.0));
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("new.ani")), p(&d.join("target.ani")), "--json"]));
    for key in ["candidate", "target", "same_skeleton", "same_first_field", "passes", "differences", "candidate_issues"] {
        assert!(v.get(key).is_some(), "key {key}: {v}");
    }
    let diff = &v["differences"][0];
    for key in ["kind", "skeleton", "bone", "candidate", "target"] {
        assert!(diff.get(key).is_some(), "difference key {key}: {diff}");
    }
    assert_eq!((diff["kind"].as_str(), diff["candidate"].as_u64(), diff["target"].as_u64()), (Some("bone_count"), Some(2), Some(3)));
}

#[test]
fn help_lists_anim() {
    let w = world();
    let o = sh(&w.data, &["--help"]);
    assert!(out(&o).contains("anim"), "{}", out(&o));
    let o = sh(&w.data, &["anim", "--help"]);
    assert_eq!(code(&o), 0);
    for sub in ["info", "validate", "compare"] {
        assert!(out(&o).contains(sub), "{sub}: {}", out(&o));
    }
}

#[test]
fn a_cloak_is_described_without_bones() {
    let w = world();
    let d = w.tmp.path().join("f");
    let v = VertSpec { pos: [0.0; 3], weight: 1.0, bone1: 0, bone2: 0, pad: 0, normal: [0.0, 1.0, 0.0], uv: [0.0; 2] };
    write(&d.join("cloak.3dc"), &c3dc_cloak_bytes(b"c.tga\0", &[v, v, v], &[[0, 1, 2]]));
    let o = sh(&w.data, &["anim", "info", p(&d.join("cloak.3dc"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("cloak mesh without bones, 3 vertices, 1 triangle"), "{}", out(&o));
    let j = json(&sh(&w.data, &["anim", "info", p(&d.join("cloak.3dc")), "--json"]));
    assert_eq!((j["files"][0]["layout"].as_str(), j["files"][0]["bones"].as_u64()), (Some("cloak"), Some(0)));
}

#[test]
fn a_skinned_mesh_with_a_second_mesh_is_described_as_such() {
    let w = world();
    let d = w.tmp.path().join("f");
    let mut two = arm_mesh_3dc();
    two.extend_from_slice(&arm_mesh_3dc());
    write(&d.join("two.3dc"), &two);
    let o = sh(&w.data, &["anim", "info", p(&d.join("two.3dc"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("two.3dc: skinned mesh with a second mesh, 32 vertices, 48 triangles, 3 bones, highest vertex bone 2"), "{}", out(&o));
    let j = json(&sh(&w.data, &["anim", "info", p(&d.join("two.3dc")), "--json"]));
    assert_eq!((j["files"][0]["layout"].as_str(), j["files"][0]["vertices"].as_u64()), (Some("skinned-with-second-mesh"), Some(32)));
}

#[test]
fn a_named_file_with_the_wrong_extension_is_refused_without_being_read() {
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("data.saf"), b"not an animation");
    write(&d.join("m.3dc"), &arm_mesh_3dc());
    // on Windows the files are held open with no sharing, so any attempt to read them fails with a read error: the extension refusal
    // below can only come from a check that runs before the read
    #[cfg(windows)]
    let _held = {
        use std::os::windows::fs::OpenOptionsExt;
        [d.join("data.saf"), d.join("m.3dc")].map(|f| std::fs::OpenOptions::new().read(true).share_mode(0).open(f).unwrap())
    };
    let o = sh(&w.data, &["anim", "info", p(&d.join("data.saf"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(out(&o).contains("data.saf: FAILED: ") && out(&o).contains("not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file"), "{}", out(&o));
    // validate refuses a .3DC by name too
    let o = sh(&w.data, &["anim", "validate", p(&d.join("m.3dc"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(out(&o).contains("not an .ANI file"), "{}", out(&o));
}

// ---- .VANI, .MANI, .3DO, .3DE -----------------------------------------------------------------------------------------------------

#[test]
fn info_describes_a_vani_a_mani_a_3do_and_a_3de() {
    use shaiya_testkit::anim::{animated_3do, box_3de, box_vani, mani_bytes};
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("b.VANI"), &box_vani(12, 66));
    write(&d.join("m.MANI"), &mani_bytes(33, 1, 500, 10f32.to_bits()));
    write(&d.join("i.3DO"), &animated_3do(11, 20));
    write(&d.join("e.3DE"), &box_3de(Some(4)));
    write(&d.join("s.3de"), &box_3de(None));
    let names = ["b.VANI", "m.MANI", "i.3DO", "e.3DE", "s.3de"];
    let mut args = vec!["anim", "info"];
    let paths: Vec<String> = names.iter().map(|n| p(&d.join(n)).to_string()).collect();
    args.extend(paths.iter().map(|s| s.as_str()));
    let o = sh(&w.data, &args);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("b.VANI: vertex-animated model, 1 item, 8 vertices per step, 12 triangles, 12 steps, about 66 ms per step (inferred)"), "{t}");
    assert!(t.contains("m.MANI: building motion, 108 bytes, 20 fields (every meaning inferred), tick 33 ms"), "{t}");
    assert!(t.contains("i.3DO: 3DO mesh, 8 vertices, 12 triangles, 11 frames at 20 per second (inferred)"), "{t}");
    assert!(t.contains("e.3DE: effect mesh, 8 vertices, 12 triangles, 4 frames"), "{t}");
    assert!(t.contains("s.3de: effect mesh, 8 vertices, 12 triangles, static, no frame block"), "{t}");
    args.push("--json");
    let v = json(&sh(&w.data, &args));
    let f = |i: usize| &v["files"][i];
    for key in ["path", "format", "items", "vertices_per_step", "triangles", "steps", "frame_ms", "frame_ms_inferred", "textures", "collision_mesh"] {
        assert!(f(0).get(key).is_some(), "vani key {key}: {}", f(0));
    }
    assert_eq!((f(0)["format"].as_str(), f(0)["steps"].as_u64(), f(0)["frame_ms"].as_i64(), f(0)["collision_mesh"].as_bool()), (Some("vani"), Some(12), Some(66), Some(false)));
    assert_eq!(f(0)["textures"][0], "box.tga");
    for key in ["path", "format", "size", "fields"] {
        assert!(f(1).get(key).is_some(), "mani key {key}: {}", f(1));
    }
    assert_eq!((f(1)["format"].as_str(), f(1)["size"].as_u64(), f(1)["fields"].as_array().map(|a| a.len())), (Some("mani"), Some(108), Some(20)));
    let dist = &f(1)["fields"][19];
    assert_eq!((dist["name"].as_str(), dist["offset"].as_u64(), dist["value"].as_f64(), dist["raw"].as_str(), dist["inferred"].as_bool()), (Some("move_distance"), Some(104), Some(10.0), Some("00002041"), Some(true)));
    assert!(f(1)["fields"].as_array().unwrap().iter().all(|x| x["inferred"] == true && x["meaning"].as_str().is_some_and(|m| !m.is_empty())));
    for key in ["path", "format", "vertices", "triangles", "frames", "frame_rate", "frame_rate_inferred", "texture"] {
        assert!(f(2).get(key).is_some(), "3do key {key}: {}", f(2));
    }
    assert_eq!((f(2)["format"].as_str(), f(2)["frames"].as_u64(), f(2)["frame_rate"].as_i64(), f(2)["frame_rate_inferred"].as_str()), (Some("3do"), Some(11), Some(20), Some("frames per second")));
    for key in ["path", "format", "layout", "vertices", "triangles", "frames", "max_key", "max_key_inferred", "texture"] {
        assert!(f(3).get(key).is_some(), "3de key {key}: {}", f(3));
    }
    assert_eq!((f(3)["layout"].as_str(), f(3)["frames"].as_u64(), f(3)["max_key"].as_i64(), f(3)["max_key_inferred"].as_bool()), (Some("standard"), Some(4), Some(3), Some(true)));
    assert_eq!((f(4)["layout"].as_str(), f(4)["frames"].as_u64()), (Some("static"), Some(0)));
    assert!(f(4)["max_key"].is_null());
}

#[test]
fn info_finds_the_new_formats_in_a_folder_and_in_an_archive_in_any_letter_case() {
    use shaiya_testkit::anim::{animated_3do, box_3de, box_vani, mani_bytes};
    let w = world();
    let d = w.tmp.path().join("f");
    let files: [(&str, Vec<u8>); 8] = [
        ("sub/one.VANI", box_vani(3, 33)),
        ("two.Vani", box_vani(3, 33)),
        ("three.MANI", mani_bytes(33, 0, 0, 0)),
        ("four.mani", mani_bytes(33, 0, 0, 0)),
        ("five.3DO", animated_3do(2, 10)),
        ("six.3do", animated_3do(2, 10)),
        ("seven.3DE", box_3de(Some(1))),
        ("eight.3de", box_3de(None)),
    ];
    for (n, b) in &files {
        write(&d.join(n), b);
    }
    write(&d.join("skip.txt"), b"x");
    let o = sh(&w.data, &["anim", "info", p(&d)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert_eq!(t.lines().count(), 8, "{t}");
    for (n, _) in &files {
        assert!(t.contains(n), "{n} in {t}");
    }
    assert!(!t.contains("skip.txt"));
    let c = client_with(&w, "c", &[("Entity/B.VANI", box_vani(3, 33)), ("Entity/B.MANI", mani_bytes(33, 0, 0, 0)), ("Item/3DO/A.3DO", animated_3do(2, 10)), ("Effect/3DE/E.3DE", box_3de(Some(1))), ("Item/readme.txt", b"x".to_vec())]);
    // the default takes all six formats; patterns are case-insensitive
    let o = sh(&w.data, &["anim", "info", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o).lines().count(), 4, "{}", out(&o));
    let o = sh(&w.data, &["anim", "info", "--from", p(&c), "*.vani", "effect/3de/*"]);
    assert_eq!(out(&o).lines().count(), 2, "{}", out(&o));
    assert!(out(&o).contains("Entity/B.VANI: vertex-animated model") && out(&o).contains("Effect/3DE/E.3DE: effect mesh"), "{}", out(&o));
}

#[test]
fn info_exits_3_for_a_file_that_does_not_read_in_the_new_formats_and_reports_the_others() {
    use shaiya_testkit::anim::{box_3de, box_vani, mani_bytes};
    let w = world();
    let d = w.tmp.path().join("f");
    let mut odd = box_3de(Some(4));
    odd.extend_from_slice(&[1, 2, 3]);
    write(&d.join("a_good.VANI"), &box_vani(3, 33));
    write(&d.join("b_short.MANI"), &mani_bytes(33, 0, 0, 0)[..107]);
    write(&d.join("c_odd.3DE"), &odd);
    write(&d.join("d_cut.VANI"), &box_vani(3, 33)[..40]);
    let o = sh(&w.data, &["anim", "info", p(&d)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("a_good.VANI: vertex-animated model"), "{t}");
    assert!(t.contains("b_short.MANI: FAILED: ") && t.contains("a .MANI is 108 bytes"), "{t}");
    assert!(t.contains("c_odd.3DE: FAILED: ") && t.contains("does not follow the .3DE layout the toolkit reads"), "{t}");
    assert!(t.contains("d_cut.VANI: FAILED: "), "{t}");
    let v = json(&sh(&w.data, &["anim", "info", p(&d), "--json"]));
    assert_eq!(v["files"].as_array().unwrap().len(), 4);
    assert!(v["files"][2]["error"].as_str().is_some_and(|e| e.contains("c_odd.3DE")), "{v}");
}

#[test]
fn validate_and_compare_stay_ani_only_and_say_so() {
    use shaiya_testkit::anim::{box_3de, box_vani, mani_bytes};
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("a.vani"), &box_vani(3, 33));
    write(&d.join("m.mani"), &mani_bytes(33, 0, 0, 0));
    write(&d.join("e.3de"), &box_3de(None));
    write(&d.join("good.ani"), &chain(0));
    write(&d.join("x.bin"), &chain(0)); // valid .ANI bytes under another extension: refused by name, never parsed as an .ANI
    for name in ["a.vani", "m.mani", "e.3de", "x.bin"] {
        let o = sh(&w.data, &["anim", "validate", p(&d.join(name))]);
        assert_eq!(code(&o), 3, "{name}: {}", err(&o));
        assert!(out(&o).contains("not an .ANI file (validate and compare read .ANI files only"), "{name}: {}", out(&o));
        let o = sh(&w.data, &["anim", "compare", p(&d.join(name)), p(&d.join("good.ani"))]);
        assert_eq!(code(&o), 3, "{name}: {}", err(&o));
        assert!(err(&o).contains(&format!("{name}: FAILED: ")) && err(&o).contains("not an .ANI file (validate and compare read .ANI files only"), "{name}: {}", err(&o));
        let o = sh(&w.data, &["anim", "compare", p(&d.join("good.ani")), p(&d.join(name))]);
        assert_eq!(code(&o), 3, "{name}: {}", err(&o));
        assert!(err(&o).contains(&format!("{name}: FAILED: ")) && err(&o).contains("not an .ANI file (validate and compare read .ANI files only"), "{name}: {}", err(&o));
    }
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("x.bin")), p(&d.join("good.ani")), "--json"]));
    assert!(v["error"].as_str().is_some_and(|e| e.contains("not an .ANI file")), "{v}");
    // a folder is searched for .ani only by validate, so the other formats are not even listed
    let o = sh(&w.data, &["anim", "validate", p(&d)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("1 file: 0 errors"), "{}", err(&o));
    let h = sh(&w.data, &["anim", "info", "--help"]);
    assert!(out(&h).contains("`validate` and `compare` read .ANI files only"), "{}", out(&h));
}

#[test]
fn info_does_not_call_a_non_positive_frame_time_a_time_per_step() {
    use shaiya_testkit::anim::box_vani;
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("z.VANI"), &box_vani(3, 0));
    write(&d.join("g.VANI"), &box_vani(3, 33));
    let o = sh(&w.data, &["anim", "info", p(&d.join("z.VANI")), p(&d.join("g.VANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("z.VANI: vertex-animated model, 1 item, 8 vertices per step, 12 triangles, 3 steps, frame_ms 0 is not a positive time"), "{t}");
    assert!(!t.contains("about 0 ms") && t.contains("g.VANI: ") && t.contains("about 33 ms per step (inferred)"), "{t}");
}

// ---- export and import --------------------------------------------------------------------------------------------------------------

/// One synthetic file of each of the six formats, named as the archive would name them (mixed letter case on purpose).
fn all_six() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("arm.ANI", arm_ani(30)),
        ("arm.3DC", arm_mesh_3dc()),
        ("box.vani", box_vani(4, 66)),
        ("bld.Mani", mani_bytes(33, 1, 200, 500)),
        ("box.3do", animated_3do(3, 20)),
        ("fx.3DE", box_3de(Some(3))),
    ]
}

fn export(w: &World, input: &Path, out: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["anim", "export", p(input), "-o", p(out)];
    args.extend_from_slice(extra);
    sh(&w.data, &args)
}

fn import(w: &World, input: &Path, out: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["anim", "import", p(input), "-o", p(out)];
    args.extend_from_slice(extra);
    sh(&w.data, &args)
}

/// The JSON file as a value, edited by `f`, written back to `to`.
fn edit_json(from: &Path, to: &Path, f: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(from).unwrap()).unwrap();
    f(&mut v);
    std::fs::write(to, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

fn names_in(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// A work folder with `arm.ANI` (the three-bone arm) and its JSON in it.
fn arm_work(w: &World) -> (PathBuf, PathBuf, PathBuf) {
    let d = w.tmp.path().join("work");
    let ani = d.join("arm.ANI");
    write(&ani, &arm_ani(30));
    let json = d.join("arm.json");
    let o = export(w, &ani, &json, &[]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    (d, ani, json)
}

#[test]
fn export_and_import_give_back_each_format() {
    let w = world();
    let d = w.tmp.path().join("six");
    for (name, bytes) in all_six() {
        let input = d.join("in").join(name);
        write(&input, &bytes);
        let json = d.join("json").join(format!("{name}.json"));
        std::fs::create_dir_all(json.parent().unwrap()).unwrap();
        let o = export(&w, &input, &json, &[]);
        assert_eq!(code(&o), 0, "{name}: {}", err(&o));
        assert!(out(&o).starts_with(&format!("wrote {}: ", p(&json))), "{name}: {}", out(&o));
        let back = d.join("back").join(name);
        std::fs::create_dir_all(back.parent().unwrap()).unwrap();
        let like = [String::from("--like"), p(&input).to_string()];
        let extra: Vec<&str> = if name.ends_with(".ANI") { like.iter().map(String::as_str).collect() } else { Vec::new() };
        let o = import(&w, &json, &back, &extra);
        assert_eq!(code(&o), 0, "{name}: {}{}", out(&o), err(&o));
        assert_eq!(std::fs::read(&back).unwrap(), bytes, "{name}: the file made from the JSON differs from the original");
    }
}

#[test]
fn export_source_is_the_file_name_only() {
    let w = world();
    let (d, _, json) = arm_work(&w);
    let text = std::fs::read_to_string(&json).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["source"], "arm.ANI");
    assert!(!text.contains(p(&d)) && !text.contains(p(w.tmp.path())), "the JSON holds a folder path: {text}");
    // from an archive the source is the archive path
    let c = client_with(&w, "c", &[("Monster/ANI/Mob_A_Att1.ANI", chain(0))]);
    let out_json = w.tmp.path().join("fromarchive.json");
    let o = sh(&w.data, &["anim", "export", "Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c), "-o", p(&out_json)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let text = std::fs::read_to_string(&out_json).unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap()["source"], "Monster/ANI/Mob_A_Att1.ANI");
    assert!(!text.contains(p(w.tmp.path())), "{text}");
}

#[test]
fn export_from_an_archive_takes_one_exact_path() {
    let w = world();
    let c = client_with(&w, "c", &[("Monster/ANI/Mob_A_Att1.ANI", chain(0)), ("Monster/ANI/Mob_B_Att1.ANI", chain(0)), ("Monster/3DC/Mob_A.3DC", arm_mesh_3dc())]);
    let d = w.tmp.path().join("o");
    std::fs::create_dir_all(&d).unwrap();
    let ok = d.join("a.json");
    let o = sh(&w.data, &["anim", "export", "Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c), "-o", p(&ok)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("as shaiya-toolkit-ani-v1"), "{}", out(&o));
    for (arg, why) in [("Monster/ANI/Mob_*.ANI", "not in the archive"), ("Monster/ANI/Mob_*", "not an .ANI"), ("Monster/ANI/Nope.ANI", "not in the archive")] {
        let bad = d.join("bad.json");
        let o = sh(&w.data, &["anim", "export", arg, "--from", p(&c), "-o", p(&bad)]);
        assert_eq!(code(&o), 3, "{arg}: {}{}", out(&o), err(&o));
        assert!(err(&o).contains(why), "{arg}: {}", err(&o));
        assert!(!bad.exists());
    }
    assert_eq!(names_in(&d), vec!["a.json"]);
}

#[test]
fn import_ani_with_the_same_skeleton_writes_and_exits_0() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    let new = d.join("new.ani");
    let o = import(&w, &json, &new, &["--like", p(&ani)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains(&format!("same skeleton as {}: 3 bones, parents and reference matrices identical", p(&ani))), "{t}");
    assert!(t.contains("first field: same (0)") && t.contains("passes\nwrote "), "{t}");
    assert!(t.trim_end().ends_with(&format!("wrote {}: .ANI, {} bytes", p(&new), std::fs::metadata(&ani).unwrap().len())), "{t}");
    assert_eq!(std::fs::read(&new).unwrap(), arm_ani(30));
    // with --from the target is an archive path
    let c = client_with(&w, "c", &[("Monster/ANI/Mob_A_Att1.ANI", arm_ani(30))]);
    let new2 = d.join("new2.ANI");
    let o = import(&w, &json, &new2, &["--like", "Monster/ANI/Mob_A_Att1.ANI", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("same skeleton as Monster/ANI/Mob_A_Att1.ANI: 3 bones"), "{}", out(&o));
    assert!(new2.exists());
}

#[test]
fn import_ani_with_a_changed_parent_matrix_bit_or_bone_count_exits_1_and_writes_nothing() {
    let w = world();
    let (d, ani, jf) = arm_work(&w);
    type Edit = fn(&mut serde_json::Value);
    let cases: [(&str, Edit, &str); 3] = [
        ("parent", |v| v["bones"][2]["parent"] = serde_json::json!(0), "  bone 2: parent is 0, target 1"),
        (
            "matrix",
            |v| {
                let hex = v["bones"][1]["matrix_hex"].as_str().unwrap().to_string();
                let flipped = match hex.strip_prefix('0') {
                    Some(rest) => format!("1{rest}"),
                    None => format!("0{}", &hex[1..]),
                };
                v["bones"][1]["matrix_hex"] = serde_json::json!(flipped);
            },
            "  bone 1: reference matrix differs",
        ),
        ("bones", |v| drop(v["bones"].as_array_mut().unwrap().pop()), "  bone count: candidate 2, target 3"),
    ];
    for (what, edit, line) in cases {
        let edited = d.join(format!("{what}.json"));
        edit_json(&jf, &edited, edit);
        let new = d.join(format!("{what}.ani"));
        let o = import(&w, &edited, &new, &["--like", p(&ani)]);
        assert_eq!(code(&o), 1, "{what}: {}{}", out(&o), err(&o));
        let t = out(&o);
        assert!(t.contains(&format!("the skeleton differs from {}:", p(&ani))) && t.contains(line), "{what}: {t}");
        assert!(t.trim_end().ends_with("does not pass\nnothing written"), "{what}: {t}");
        assert!(!new.exists(), "{what}: the output was written");
        let v = json(&import(&w, &edited, &new, &["--like", p(&ani), "--json"]));
        assert_eq!((v["passes"].as_bool(), v["written"].as_bool()), (Some(false), Some(false)), "{what}: {v}");
        assert_eq!(v["candidate"].as_str(), Some(p(&edited)));
        assert_eq!(v["target"].as_str(), Some(p(&ani)));
        assert!(!new.exists());
    }
    // no temp file is left behind
    assert!(names_in(&d).iter().all(|n| !n.ends_with(".shaiya-tmp")), "{:?}", names_in(&d));
}

#[test]
fn import_ani_with_another_first_field_exits_1() {
    let w = world();
    let (d, ani, jf) = arm_work(&w);
    let edited = d.join("ff.json");
    edit_json(&jf, &edited, |v| v["first_field"] = serde_json::json!(-1));
    let new = d.join("ff.ani");
    let o = import(&w, &edited, &new, &["--like", p(&ani)]);
    assert_eq!(code(&o), 1, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("first field differs: candidate -1, target 0 (inferred: the start key; not checked in the game)"), "{}", out(&o));
    assert!(out(&o).contains("does not pass") && !new.exists());
}

#[test]
fn import_ani_with_two_keys_swapped_exits_1_and_writes_nothing() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    let edited = d.join("swap.json");
    // bone 1: the keys 0 and 30 of the rotation track trade places, so they no longer increase
    edit_json(&json, &edited, |v| {
        v["bones"][1]["rotation_keys"][0][0] = serde_json::json!(30);
        v["bones"][1]["rotation_keys"][1][0] = serde_json::json!(0);
    });
    let new = d.join("swap.ani");
    let o = import(&w, &edited, &new, &["--like", p(&ani)]);
    assert_eq!(code(&o), 1, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("does not pass") && out(&o).contains("nothing written"), "{}", out(&o));
    assert!(!new.exists());
}

#[test]
fn import_ani_with_an_edited_key_passes_and_writes_the_edit() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    let edited = d.join("key.json");
    // the second rotation key of bone 1 moves from key 30 to key 25 (the end key stays 30)
    edit_json(&json, &edited, |v| v["bones"][1]["rotation_keys"][1][0] = serde_json::json!(25));
    let new = d.join("key.ani");
    let o = import(&w, &edited, &new, &["--like", p(&ani)]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    let written = std::fs::read(&new).unwrap();
    assert_ne!(written, arm_ani(30));
    assert_eq!(written.len(), arm_ani(30).len());
    // read back through export: the edit is in the file, the rest is as it was
    let again = d.join("key-again.json");
    assert_eq!(code(&export(&w, &new, &again, &[])), 0);
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&again).unwrap()).unwrap();
    assert_eq!(v["bones"][1]["rotation_keys"][1][0], 25);
    assert_eq!(v["bones"][1]["rotation_keys"][0][0], 0);
}

#[test]
fn import_without_like_for_ani_exits_2() {
    let w = world();
    let (d, _, json) = arm_work(&w);
    let new = d.join("n.ani");
    let o = import(&w, &json, &new, &[]);
    assert_eq!(code(&o), 2, "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("--like is required for .ANI: the file it will replace"), "{}", err(&o));
    assert!(!new.exists());
}

#[test]
fn like_for_another_format_exits_2() {
    let w = world();
    let d = w.tmp.path().join("work");
    let input = d.join("arm.3DC");
    write(&input, &arm_mesh_3dc());
    let json = d.join("arm3dc.json");
    assert_eq!(code(&export(&w, &input, &json, &[])), 0);
    let new = d.join("new.3DC");
    let o = import(&w, &json, &new, &["--like", p(&input)]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert!(err(&o).contains("--like is for .ANI only"), "{}", err(&o));
    let o = import(&w, &json, &new, &["--from", p(&w.client)]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert!(err(&o).contains("--from needs --like"), "{}", err(&o));
    assert!(!new.exists());
}

#[test]
fn export_output_must_be_json_exits_2() {
    let w = world();
    let (d, ani, _) = arm_work(&w);
    for bad in ["out.txt", "out", "out.ANI"] {
        let o = export(&w, &ani, &d.join(bad), &[]);
        assert_eq!(code(&o), 2, "{bad}: {}", err(&o));
        assert!(err(&o).contains("must end in .json"), "{}", err(&o));
        assert!(!d.join(bad).exists());
    }
    // any letter case of the extension is accepted
    assert_eq!(code(&export(&w, &ani, &d.join("up.JSON"), &[])), 0);
}

/// A file of `len` bytes made with `set_len` (a sparse file: nothing is written), closed before it is used.
fn sized_file(path: &Path, len: u64) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = std::fs::File::create(path).unwrap();
    f.set_len(len).unwrap();
    drop(f);
}

#[test]
fn import_refuses_a_json_over_256_mib() {
    let w = world();
    let d = w.tmp.path().join("big");
    let json = d.join("huge.json");
    sized_file(&json, (256 << 20) + 1);
    let out = d.join("out.ANI");
    let o = import(&w, &json, &out, &[]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("larger than 256 MiB: not read"), "{}", err(&o));
    assert!(!out.exists());
}

#[test]
fn a_model_file_over_128_mib_is_not_read_by_info_or_export() {
    let w = world();
    let d = w.tmp.path().join("big");
    let ani = d.join("huge.ani");
    sized_file(&ani, (128 << 20) + 1);
    let o = sh(&w.data, &["anim", "info", p(&ani)]);
    assert_eq!(code(&o), 3, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("FAILED") && out(&o).contains("larger than 128 MiB: not read"), "{}", out(&o));
    let json = d.join("huge.json");
    let o = export(&w, &ani, &json, &[]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("larger than 128 MiB: not read"), "{}", err(&o));
    assert!(!json.exists());
}

#[test]
fn import_of_an_unknown_or_newer_format_exits_3() {
    let w = world();
    let d = w.tmp.path().join("work");
    for (name, text, why) in [
        ("newer.json", r#"{"format": "shaiya-toolkit-ani-v2"}"#, "made by a newer toolkit (v2)"),
        ("foreign.json", r#"{"format": "something-else"}"#, "not a toolkit model JSON"),
        ("nojson.json", "this is not json", "line 1"),
    ] {
        let f = d.join(name);
        write(&f, text.as_bytes());
        let new = d.join("n.ANI");
        let o = import(&w, &f, &new, &["--like", p(&f)]);
        assert_eq!(code(&o), 3, "{name}: {}{}", out(&o), err(&o));
        assert!(err(&o).contains(why), "{name}: {}", err(&o));
        assert!(!new.exists());
    }
    let missing = import(&w, &d.join("missing.json"), &d.join("n.ANI"), &[]);
    assert_eq!(code(&missing), 3, "{}", err(&missing));
}

#[test]
fn import_with_a_mismatched_extension_exits_3() {
    let w = world();
    let d = w.tmp.path().join("work");
    let input = d.join("arm.3DC");
    write(&input, &arm_mesh_3dc());
    let json = d.join("arm3dc.json");
    assert_eq!(code(&export(&w, &input, &json, &[])), 0);
    for bad in ["out.ANI", "out.bin", "out"] {
        let o = import(&w, &json, &d.join(bad), &[]);
        assert_eq!(code(&o), 3, "{bad}: {}{}", out(&o), err(&o));
        assert!(err(&o).contains("holds shaiya-toolkit-3dc-v1, but ") && err(&o).contains("asks for"), "{bad}: {}", err(&o));
        assert!(!d.join(bad).exists());
    }
    assert!(err(&import(&w, &json, &d.join("out.ANI"), &[])).contains("asks for .ANI"));
    // any letter case of the right extension is accepted
    assert_eq!(code(&import(&w, &json, &d.join("out.3dc"), &[])), 0);
}

#[test]
fn an_existing_output_exits_4_and_force_replaces_it() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    // export
    let before = std::fs::read(&json).unwrap();
    write(&json, b"mine");
    let o = export(&w, &ani, &json, &[]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains(&format!("{} exists; use --force to replace it", p(&json))), "{}", err(&o));
    assert_eq!(std::fs::read(&json).unwrap(), b"mine");
    let o = export(&w, &ani, &json, &["--force"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&json).unwrap(), before);
    // import
    let new = d.join("new.ANI");
    write(&new, b"mine");
    let o = import(&w, &json, &new, &["--like", p(&ani)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("exists; use --force to replace it"));
    assert_eq!(std::fs::read(&new).unwrap(), b"mine");
    let o = import(&w, &json, &new, &["--like", p(&ani), "--force"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&new).unwrap(), arm_ani(30));
    assert!(names_in(&d).iter().all(|n| !n.ends_with(".shaiya-tmp")), "{:?}", names_in(&d));
}

#[test]
fn an_output_inside_a_client_folder_exits_4() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    // a folder with data.sah (any letter case) and a subfolder of it
    let cf = w.tmp.path().join("cf");
    let sub = cf.join("sub").join("deeper");
    std::fs::create_dir_all(&sub).unwrap();
    write(&cf.join("DATA.SAH"), b"x");
    for dir in [&cf, &sub] {
        let o = export(&w, &ani, &dir.join("o.json"), &[]);
        assert_eq!(code(&o), 4, "{}", err(&o));
        assert!(err(&o).contains("is inside the client folder") && err(&o).contains("put it in with Replace or archive replace"), "{}", err(&o));
        assert!(!dir.join("o.json").exists());
        let o = import(&w, &json, &dir.join("o.ANI"), &["--like", p(&ani)]);
        assert_eq!(code(&o), 4, "{}", err(&o));
        assert!(!dir.join("o.ANI").exists());
    }
    assert_eq!(names_in(&cf), vec!["DATA.SAH", "sub"]);
    // --force does not lift it
    assert_eq!(code(&export(&w, &ani, &cf.join("o.json"), &["--force"])), 4);
    // a saved client folder counts even when it holds no data.sah (any more)
    let saved = w.tmp.path().join("saved");
    std::fs::create_dir_all(&saved).unwrap();
    std::fs::copy(w.client.join("data.sah"), saved.join("data.sah")).unwrap();
    std::fs::copy(w.client.join("data.saf"), saved.join("data.saf")).unwrap();
    assert_eq!(code(&sh(&w.data, &["clients", "add", "mine", p(&saved)])), 0);
    std::fs::remove_file(saved.join("data.sah")).unwrap();
    std::fs::remove_file(saved.join("data.saf")).unwrap();
    let o = export(&w, &ani, &saved.join("o.json"), &[]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("is inside the client folder"), "{}", err(&o));
    assert!(names_in(&saved).is_empty());
    // a folder that does not exist is an input error
    let o = export(&w, &ani, &d.join("nope").join("o.json"), &[]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("folder does not exist"), "{}", err(&o));
}

#[test]
fn output_equal_to_an_input_exits_4() {
    let w = world();
    let (d, ani, json) = arm_work(&w);
    // the --like file of an .ANI import cannot be overwritten (the other inputs cannot be named by -o: their extensions differ)
    let o = import(&w, &json, &ani, &["--like", p(&ani), "--force"]);
    assert_eq!(code(&o), 4, "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("is the input"), "{}", err(&o));
    assert_eq!(std::fs::read(&ani).unwrap(), arm_ani(30));
    // the same file through another spelling of the path
    let spelled = d.join(".").join("arm.ANI");
    let o = import(&w, &json, &spelled, &["--like", p(&ani), "--force"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(std::fs::read(&ani).unwrap(), arm_ani(30));
}

#[test]
fn export_of_a_nan_key_exits_3_and_writes_nothing() {
    let w = world();
    let d = w.tmp.path().join("work");
    let nan = ani_bytes(0, 30, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(0, [0.0, 0.0, f32::NAN, 1.0])], trans: Vec::new() }]);
    let input = d.join("nan.ANI");
    write(&input, &nan);
    let json = d.join("nan.json");
    let o = export(&w, &input, &json, &[]);
    assert_eq!(code(&o), 3, "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("finite"), "{}", err(&o));
    assert!(!json.exists());
    assert_eq!(names_in(&d), vec!["nan.ANI"]);
}

#[test]
fn export_of_a_3de_in_another_layout_exits_3_with_the_reason() {
    let w = world();
    let d = w.tmp.path().join("work");
    let bytes = box_3de(Some(3));
    let input = d.join("odd.3DE");
    write(&input, &bytes[..bytes.len() - 3]);
    let json = d.join("odd.json");
    let o = export(&w, &input, &json, &[]);
    assert_eq!(code(&o), 3, "{}{}", out(&o), err(&o));
    assert!(err(&o).contains("does not follow the .3DE layout the toolkit reads"), "{}", err(&o));
    assert!(!json.exists());
    // a file that is not one of the six is refused by name, before it is read
    let text = d.join("notes.txt");
    write(&text, b"x");
    let o = export(&w, &text, &d.join("n.json"), &[]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file"), "{}", err(&o));
}

#[test]
fn the_json_of_export_and_import_has_the_documented_keys() {
    let w = world();
    let (d, ani, json_file) = arm_work(&w);
    let keys = |v: &serde_json::Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let e = d.join("e.json");
    let v = json(&sh(&w.data, &["anim", "export", p(&ani), "-o", p(&e), "--json"]));
    assert_eq!(keys(&v), ["bytes", "format", "input", "out", "schema"]);
    assert_eq!((v["format"].as_str(), v["schema"].as_str(), v["bytes"].as_u64()), (Some("ani"), Some("shaiya-toolkit-ani-v1"), Some(std::fs::metadata(&e).unwrap().len())));
    // an .ANI import: the compare keys and the write keys
    let new = d.join("j.ani");
    let v = json(&sh(&w.data, &["anim", "import", p(&json_file), "-o", p(&new), "--like", p(&ani), "--json"]));
    assert_eq!(
        keys(&v),
        ["bytes", "candidate", "candidate_issues", "differences", "format", "input", "out", "passes", "same_first_field", "same_skeleton", "target", "written"]
    );
    assert_eq!((v["written"].as_bool(), v["passes"].as_bool(), v["bytes"].as_u64()), (Some(true), Some(true), Some(arm_ani(30).len() as u64)));
    assert_eq!((v["candidate"].as_str(), v["target"].as_str(), v["format"].as_str()), (Some(p(&json_file)), Some(p(&ani)), Some("ani")));
    // another format: the write keys only
    let mesh = d.join("m.3DC");
    write(&mesh, &arm_mesh_3dc());
    let mj = d.join("m.json");
    assert_eq!(code(&export(&w, &mesh, &mj, &[])), 0);
    let out_mesh = d.join("m-out.3DC");
    let v = json(&sh(&w.data, &["anim", "import", p(&mj), "-o", p(&out_mesh), "--json"]));
    assert_eq!(keys(&v), ["bytes", "format", "input", "out", "written"]);
    assert_eq!((v["written"].as_bool(), v["format"].as_str()), (Some(true), Some("3dc")));
}

#[test]
fn compare_output_is_unchanged() {
    // the exact text of `compare`, which `import` shares: the lines are asserted whole (the other compare tests above stay as they were)
    let w = world();
    let d = w.tmp.path().join("f");
    write(&d.join("same.ani"), &chain(0));
    write(&d.join("target.ani"), &chain(0));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("same.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 0);
    assert_eq!(out(&o), format!("same skeleton as {}: 3 bones, parents and reference matrices identical\nfirst field: same (0)\npasses\n", p(&d.join("target.ani"))));
    write(&d.join("other.ani"), &chain(-1));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("other.ani")), p(&d.join("target.ani"))]);
    assert_eq!(code(&o), 1);
    assert_eq!(
        out(&o),
        format!(
            "same skeleton as {}: 3 bones, parents and reference matrices identical\nfirst field differs: candidate -1, target 0 (inferred: the start key; not checked in the game)\ndoes not pass\n",
            p(&d.join("target.ani"))
        )
    );
    let v = json(&sh(&w.data, &["anim", "compare", p(&d.join("other.ani")), p(&d.join("target.ani")), "--json"]));
    let mut k: Vec<&String> = v.as_object().unwrap().keys().collect();
    k.sort();
    assert_eq!(k, ["candidate", "candidate_issues", "differences", "passes", "same_first_field", "same_skeleton", "target"]);
}

#[test]
fn help_lists_export_and_import() {
    let w = world();
    let o = sh(&w.data, &["anim", "--help"]);
    assert_eq!(code(&o), 0);
    for sub in ["export", "import"] {
        assert!(out(&o).contains(sub), "{sub}: {}", out(&o));
    }
    let o = sh(&w.data, &["anim", "import", "--help"]);
    assert!(out(&o).contains("--like") && out(&o).contains("--force") && out(&o).contains("-o, --out"), "{}", out(&o));
    let o = sh(&w.data, &["--help"]);
    assert!(out(&o).contains("export any of them to JSON and import it back"), "{}", out(&o));
}

// ---- export --out-dir and diff ----------------------------------------------------------------------------------------------------

/// A client folder with two animations, a mesh and a texture-like file under `Monster`.
fn monster_client(w: &World) -> PathBuf {
    client_with(
        w,
        "mc",
        &[("Monster/ANI/Mob_A_Att1.ANI", chain(0)), ("Monster/ANI/Mob_B_Att1.ANI", arm_ani(30)), ("Monster/3DC/Mob_A.3DC", arm_mesh_3dc()), ("Monster/DDS/Mob_A.dds", b"DDS fake".to_vec())],
    )
}

fn files_under(d: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, v: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(base, &path, v);
            } else {
                v.push(path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut v = Vec::new();
    if d.exists() {
        walk(d, d, &mut v);
    }
    v.sort();
    v
}

#[test]
fn export_out_dir_writes_one_json_per_file_under_the_archive_folders() {
    let w = world();
    let c = monster_client(&w);
    let dir = w.tmp.path().join("json-out");
    let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).trim_end().ends_with("wrote 3 of 3 (0 failed)"), "{}", out(&o));
    assert_eq!(files_under(&dir), vec!["Monster/3DC/Mob_A.3DC.json", "Monster/ANI/Mob_A_Att1.ANI.json", "Monster/ANI/Mob_B_Att1.ANI.json"], "the .dds is not a model file");
    // each file is the same JSON `-o` gives, with the archive path as its source
    let one = w.tmp.path().join("one.json");
    let o = sh(&w.data, &["anim", "export", "Monster/ANI/Mob_B_Att1.ANI", "--from", p(&c), "-o", p(&one)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&one).unwrap(), std::fs::read(dir.join("Monster/ANI/Mob_B_Att1.ANI.json")).unwrap());
    let text = std::fs::read_to_string(dir.join("Monster/3DC/Mob_A.3DC.json")).unwrap();
    assert!(text.contains("\"Monster/3DC/Mob_A.3DC\"") && !text.contains(p(w.tmp.path())), "the source is the archive path, no folder of this machine");
    // disk inputs: a folder keeps its inner paths, a named file is written under its file name
    let src = w.tmp.path().join("src");
    write(&src.join("inner/arm.ANI"), &arm_ani(30));
    write(&src.join("top.3dc"), &arm_mesh_3dc());
    let named = w.tmp.path().join("elsewhere").join("solo.ANI");
    write(&named, &arm_ani(10));
    let dir2 = w.tmp.path().join("json2");
    let o = sh(&w.data, &["anim", "export", p(&src), p(&named), "--out-dir", p(&dir2)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(files_under(&dir2), vec!["inner/arm.ANI.json", "solo.ANI.json", "top.3dc.json"]);
    // the JSON report
    let dir3 = w.tmp.path().join("json3");
    let v = json(&sh(&w.data, &["anim", "export", "Monster/ANI/*", "--from", p(&c), "--out-dir", p(&dir3), "--json"]));
    assert_eq!(v["written"].as_array().unwrap().len(), 2, "{v}");
    assert!(v["failed"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn export_out_dir_inside_a_client_folder_exits_4_and_reads_nothing() {
    let w = world();
    let c = monster_client(&w);
    let before = snapshot(&c);
    for dir in [c.join("out"), c.join("deeper").join("out")] {
        let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir)]);
        assert_eq!(code(&o), 4, "{}", err(&o));
        assert!(err(&o).contains("is inside the client folder"), "{}", err(&o));
        assert!(!dir.exists());
    }
    // the folder is checked before the archive is opened: an archive that cannot be opened is not even reached
    let cf = w.tmp.path().join("cf");
    std::fs::create_dir_all(&cf).unwrap();
    write(&cf.join("data.SAH"), b"x");
    let o = sh(&w.data, &["anim", "export", "*", "--from", p(&w.tmp.path().join("no-such-archive")), "--out-dir", p(&cf.join("o"))]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(!cf.join("o").exists());
    assert_eq!(snapshot(&c), before);
}

/// Makes a link to a folder (a symlink on Unix, a junction on Windows); false, with the reason printed, when it cannot be made.
fn link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        match std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(link).arg(target).output() {
            Ok(o) if o.status.success() && link.exists() => true,
            Ok(o) => {
                eprintln!("skipped: mklink /J failed: {}", String::from_utf8_lossy(&o.stdout));
                false
            }
            Err(e) => {
                eprintln!("skipped: mklink /J could not run: {e}");
                false
            }
        }
    }
}

/// A fresh folder that can hold links: on Windows a junction needs NTFS (the scratch drive is exFAT), so the profile's temp folder is used.
fn link_home(tag: &str) -> PathBuf {
    let base = if cfg!(windows) { std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Temp")).unwrap_or_else(std::env::temp_dir) } else { std::env::temp_dir() };
    let d = base.join(format!("shaiya-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn drop_link(link: &Path) {
    let _ = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
}

#[test]
fn export_out_dir_through_a_link_into_a_client_exits_4_and_makes_no_folder() {
    let w = world();
    let c = monster_client(&w);
    std::fs::create_dir_all(c.join("sub")).unwrap();
    let before = snapshot(&c);
    let home = link_home("cli-link");
    let elsewhere = home.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    // DIR = J/new where J leads into a client subfolder
    let j = elsewhere.join("J");
    if link_dir(&c.join("sub"), &j) {
        let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&j.join("new"))]);
        assert_eq!(code(&o), 4, "{}", err(&o));
        assert_eq!(snapshot(&c), before);
        assert!(!c.join("sub").join("new").exists(), "no folder was made in the client");
        drop_link(&j);
    }
    // a link that is already inside DIR
    let dir = home.join("json-out");
    std::fs::create_dir_all(&dir).unwrap();
    if link_dir(&c, &dir.join("Monster")) {
        let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir)]);
        assert_eq!(code(&o), 4, "{}", err(&o));
        assert!(err(&o).contains("is inside the client folder"), "{}", err(&o));
        assert_eq!(snapshot(&c), before);
        assert!(!c.join("Monster").exists(), "no folder was made in the client");
        drop_link(&dir.join("Monster"));
    }
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn export_out_dir_keeps_existing_files_without_force() {
    let w = world();
    let c = monster_client(&w);
    let dir = w.tmp.path().join("json-out");
    write(&dir.join("Monster/ANI/Mob_A_Att1.ANI.json"), b"keep me");
    let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(out(&o).contains("wrote 2 of 3 (1 failed)"), "{}", out(&o));
    assert!(err(&o).contains("Mob_A_Att1.ANI: FAILED:") && err(&o).contains("exists; use --force"), "{}", err(&o));
    assert_eq!(std::fs::read(dir.join("Monster/ANI/Mob_A_Att1.ANI.json")).unwrap(), b"keep me");
    assert_eq!(files_under(&dir).len(), 3, "the other two were written");
    let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir), "--force"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_ne!(std::fs::read(dir.join("Monster/ANI/Mob_A_Att1.ANI.json")).unwrap(), b"keep me");
}

#[test]
fn a_file_that_does_not_read_is_reported_and_the_others_written() {
    let w = world();
    let mut cut = arm_ani(30);
    cut.truncate(cut.len() - 3);
    let c = client_with(&w, "bc", &[("Monster/ANI/A_good.ANI", arm_ani(30)), ("Monster/ANI/B_cut.ANI", cut), ("Monster/ANI/C_good.ANI", chain(0))]);
    let dir = w.tmp.path().join("o");
    let o = sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&dir)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(out(&o).contains("wrote 2 of 3 (1 failed)") && err(&o).contains("B_cut.ANI: FAILED:"), "{}{}", out(&o), err(&o));
    assert_eq!(files_under(&dir), vec!["Monster/ANI/A_good.ANI.json", "Monster/ANI/C_good.ANI.json"]);
    let v = json(&sh(&w.data, &["anim", "export", "Monster/", "--from", p(&c), "--out-dir", p(&w.tmp.path().join("o2")), "--json"]));
    assert_eq!(v["written"].as_array().unwrap().len(), 2);
    assert_eq!(v["failed"][0]["input"], "Monster/ANI/B_cut.ANI", "{v}");
    assert!(v["failed"][0]["reason"].as_str().is_some_and(|r| !r.is_empty()), "{v}");
    // names the archive writers refuse, or that would make DIR look like a client folder, are reported, not written
    let c2 = client_with(&w, "nc", &[("data.sah/x.ANI", arm_ani(30)), ("bad.shaiya-tmp/z.ANI", arm_ani(30)), ("ok/y.ANI", arm_ani(30))]);
    let d2 = w.tmp.path().join("o3");
    let o = sh(&w.data, &["anim", "export", "*", "--from", p(&c2), "--out-dir", p(&d2)]);
    assert_eq!(code(&o), 3, "{}{}", out(&o), err(&o));
    assert_eq!(files_under(&d2), vec!["ok/y.ANI.json"]);
}

#[test]
fn two_inputs_with_the_same_output_name_write_the_first_and_exit_4() {
    let w = world();
    let (d1, d2) = (w.tmp.path().join("one"), w.tmp.path().join("two"));
    write(&d1.join("same.ANI"), &arm_ani(30));
    write(&d2.join("SAME.ani"), &arm_ani(60));
    let dir = w.tmp.path().join("o");
    let o = sh(&w.data, &["anim", "export", p(&d1.join("same.ANI")), p(&d2.join("SAME.ani")), "--out-dir", p(&dir)]);
    // a file named on the command line is written as `<file name>.json`: the second has another letter case only
    assert_eq!(code(&o), 4, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("wrote 1 of 2 (1 failed)") && err(&o).contains("same output name"), "{}{}", out(&o), err(&o));
    assert_eq!(files_under(&dir), vec!["same.ANI.json"], "only the first was written");
    assert!(std::fs::read_to_string(dir.join("same.ANI.json")).unwrap().contains("\"end_key\": 30"), "the first input won");
}

#[test]
fn an_archive_path_with_dotdot_or_a_windows_name_is_reported_exit_3_and_not_written() {
    let w = world();
    let c = client_with(
        &w,
        "tr",
        &[
            ("a/../b.ANI", arm_ani(30)),
            ("../c.ANI", arm_ani(30)),
            ("CON/d.ANI", arm_ani(30)),
            ("e./f.ANI", arm_ani(30)),
            ("data.sah./g.ANI", arm_ani(30)),
            ("nul.ANI", arm_ani(30)),
            ("ok/h.ANI", arm_ani(30)),
        ],
    );
    let d = w.tmp.path().join("o");
    let o = sh(&w.data, &["anim", "export", "*", "--from", p(&c), "--out-dir", p(&d)]);
    assert_eq!(code(&o), 3, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("wrote 1 of 7 (6 failed)"), "{}{}", out(&o), err(&o));
    assert_eq!(files_under(&d), vec!["ok/h.ANI.json"]);
    assert!(!w.tmp.path().join("c.ANI.json").exists() && !w.tmp.path().join("b.ANI.json").exists(), "nothing was written above DIR");
}

#[test]
fn more_than_one_input_needs_out_dir_exit_2() {
    let w = world();
    let d = w.tmp.path().join("m");
    write(&d.join("a.ANI"), &arm_ani(30));
    write(&d.join("b.ANI"), &arm_ani(30));
    let o = sh(&w.data, &["anim", "export", p(&d.join("a.ANI")), p(&d.join("b.ANI")), "-o", p(&d.join("x.json"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert!(err(&o).contains("--out-dir"), "{}", err(&o));
    let o = sh(&w.data, &["anim", "export", p(&d.join("a.ANI")), "-o", p(&d.join("x.json")), "--out-dir", p(&d.join("o"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    let o = sh(&w.data, &["anim", "export", p(&d.join("a.ANI"))]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert_eq!(files_under(&d), vec!["a.ANI", "b.ANI"]);
}

/// The three-bone arm with the second bone turned to the quaternion `(0, 0, sin, cos)` at the last key.
fn arm_turned(sin: f32, cos: f32, end_key: i32) -> Vec<u8> {
    let b = |parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot, trans: Vec::new() };
    ani_bytes(0, end_key, &[b(-1, 0.0, Vec::new()), b(0, 1.0, vec![(0, [0.0, 0.0, 0.0, 1.0]), (end_key, [0.0, 0.0, sin, cos])]), b(1, 2.0, Vec::new())])
}

#[test]
fn diff_json_gives_null_for_a_pose_that_is_not_a_finite_number() {
    let w = world();
    let d = w.tmp.path().join("d");
    write(&d.join("a.ANI"), &arm_ani(30));
    write(&d.join("nan.ANI"), &arm_turned(f32::NAN, f32::NAN, 30));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("nan.ANI")), "--json"]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    let v = json(&o);
    let bones = v["bones"].as_array().unwrap();
    assert!(!bones.is_empty(), "{v}");
    assert!(bones.iter().all(|b| b["max_angle_deg"].is_null() && b["max_offset"].is_null()), "an infinite value is null: {v}");
}

#[test]
fn diff_of_a_file_with_itself_exits_0() {
    let w = world();
    let d = w.tmp.path().join("d");
    write(&d.join("a.ANI"), &arm_ani(30));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("a.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).starts_with("identical:"), "{}", out(&o));
    let v = json(&sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("a.ANI")), "--json"]));
    assert_eq!(v["identical"], true);
    assert!(v["bones"].as_array().unwrap().is_empty() && v["skeleton"].as_array().unwrap().is_empty(), "{v}");
    // the same file read from an archive and from disk
    let c = client_with(&w, "dc", &[("Monster/ANI/A.ANI", arm_ani(30))]);
    let o = sh(&w.data, &["anim", "diff", "Monster/ANI/A.ANI", "Monster/ANI/A.ANI", "--from", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), "Monster/ANI/A.ANI", "--from-b", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
}

#[test]
fn diff_names_the_changed_bone_and_its_angle_exits_1() {
    let w = world();
    let d = w.tmp.path().join("d");
    write(&d.join("a.ANI"), &arm_ani(30));
    let half = std::f32::consts::FRAC_PI_8;
    write(&d.join("b.ANI"), &arm_turned(half.sin(), half.cos(), 30));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("b.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("2 bones differ in pose") && t.contains("bone 1: up to 45.00 degrees") && t.contains("bone 2: up to 45.00 degrees"), "{t}");
    assert!(!t.contains("bone 0:"), "{t}");
    let v = json(&sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("b.ANI")), "--json"]));
    assert_eq!(v["identical"], false);
    assert_eq!(v["bones"][0]["bone"], 2, "the bone that moves farthest comes first at equal turns: {v}");
    assert!((v["bones"][1]["max_angle_deg"].as_f64().unwrap() - 45.0).abs() < 0.01, "{v}");
    // a longer animation with the same poses is a difference too
    write(&d.join("c.ANI"), &arm_ani(60));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("c.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(out(&o).contains("end key differs: A 30, B 60"), "{}", out(&o));
    // unreadable and not-an-ANI inputs are exit 3
    write(&d.join("cut.ANI"), &arm_ani(30)[..20]);
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("cut.ANI"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    write(&d.join("notes.txt"), b"x");
    assert_eq!(code(&sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("notes.txt"))])), 3);
}

#[test]
fn diff_of_two_skeletons_lists_the_rig_differences() {
    let w = world();
    let d = w.tmp.path().join("d");
    write(&d.join("a.ANI"), &chain(0));
    // another bone count
    write(&d.join("two.ANI"), &ani_bytes(0, 30, &[bone(-1, 0.0), bone(0, 1.0)]));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("two.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(out(&o).contains("the skeletons differ:") && out(&o).contains("bone count: A 3, B 2"), "{}", out(&o));
    // another parent and another reference matrix, and a first field
    write(&d.join("b.ANI"), &ani_bytes(-1, 30, &[bone(-1, 0.0), bone(-1, 1.0), bone(1, 5.0)]));
    let o = sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("b.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("bone 1: parent is 0 in A, -1 in B") && t.contains("bone 2: reference matrix differs") && t.contains("first field differs: A 0, B -1"), "{t}");
    let v = json(&sh(&w.data, &["anim", "diff", p(&d.join("a.ANI")), p(&d.join("b.ANI")), "--json"]));
    assert!(v["skeleton"].as_array().unwrap().iter().any(|x| x["kind"] == "parent" && x["bone"] == 1), "{v}");
}

// ---- opt-in: a real client (read only; outputs go to SHAIYA_SCRATCH) ---------------------------------------------------------------

#[test]
fn validate_from_the_us_client_exits_0() {
    let Some(client) = env_client() else { return };
    let w = world();
    let o = sh(&w.data, &["anim", "validate", "--from", p(&client), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    println!("{} files, {} errors, {} warnings", v["files"].as_array().unwrap().len(), v["errors"], v["warnings"]);
    assert_eq!(v["files"].as_array().unwrap().len(), 3509);
    assert_eq!(v["errors"], 0);
}

#[test]
fn compare_of_the_custom_emotes_passes() {
    let (Some(client), Some(orig), Some(scratch)) = (env_client(), env_client_original(), env_scratch()) else { return };
    // the guard is declared before the archives, so it drops after them (every handle closed first: exFAT), also on a failed assert
    let out_dir = scratch.join(format!("anim-compare-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let _cleanup = RemoveDirOnDrop(out_dir.clone());
    let (now, stock) = (shaiya_archive::Archive::open(&client).unwrap(), shaiya_archive::Archive::open(&orig).unwrap());
    let w = world();
    let mut changed = Vec::new();
    for f in now.index().visible() {
        if !f.path.to_ascii_lowercase().ends_with(".ani") {
            continue;
        }
        let Some(e) = stock.index().get(&f.path) else { continue };
        let new = now.read_entry(f.entry).unwrap();
        if stock.read_entry(e).unwrap() != new {
            let file = out_dir.join(format!("changed{}.ani", changed.len()));
            std::fs::write(&file, &new).unwrap();
            changed.push((f.path.clone(), file));
        }
    }
    let names: Vec<&String> = changed.iter().map(|c| &c.0).collect();
    println!("{} changed .ani files between the two archives: {names:?}", changed.len());
    // the expected count (SHAIYA_EXPECT_CHANGED_ANI) is asserted first, before any early exit
    if let Some(expected) = shaiya_testkit::env_usize("SHAIYA_EXPECT_CHANGED_ANI") {
        assert_eq!(changed.len(), expected, "SHAIYA_EXPECT_CHANGED_ANI={expected}: {names:?}");
    }
    if changed.is_empty() {
        println!("no changed .ani files between the two archives: nothing compared");
        return;
    }
    for (path, file) in &changed {
        let o = sh(&w.data, &["anim", "compare", p(file), path, "--from", p(&orig)]);
        assert_eq!(code(&o), 0, "{path}: {}{}", out(&o), err(&o));
        assert_eq!(out(&o).lines().last(), Some("passes"));
    }
}

/// Removes a scratch folder when dropped (after the values declared later, which hold open files, are gone).
struct RemoveDirOnDrop(PathBuf);

impl Drop for RemoveDirOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Opt-in: through the binary, one file per format of the US client (the first by sorted path; a `.3DE` the reader refuses is skipped), plus a
/// file with NaN matrices and a `.3DC` written with `bits:`. Export, import (an `.ANI` with `--like` the same archive path), bytes compared with
/// the archive entries. Outputs go to a new folder under `SHAIYA_SCRATCH`, removed by a guard.
#[test]
fn export_and_import_of_us_client_files_give_the_same_bytes() {
    let (Some(client), Some(scratch)) = (env_client(), env_scratch()) else { return };
    let out_dir = scratch.join(format!("anim-json-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let _cleanup = RemoveDirOnDrop(out_dir.clone());
    let archive = shaiya_archive::Archive::open(&client).unwrap();
    let w = world();
    let mut paths: Vec<String> = archive.index().visible().iter().map(|f| f.path.clone()).collect();
    paths.sort();
    // the two `.3DE` files the reader refuses (named on the format page) are not picked
    let refused = ["004.3de", "deathknight007.3de"];
    let mut picks: Vec<String> = Vec::new();
    for ext in [".ani", ".3dc", ".vani", ".mani", ".3do", ".3de"] {
        let first = paths
            .iter()
            .find(|p| {
                let l = p.to_ascii_lowercase();
                l.ends_with(ext) && !refused.iter().any(|r| l.ends_with(r))
            })
            .unwrap_or_else(|| panic!("no {ext} file in the client"));
        picks.push(first.clone());
    }
    picks.push("Monster/ANI/Mob_Scorp01_Idle.ANI".to_string());
    picks.push("Monster/3DC/Mob_Rashiel_03.3DC".to_string());
    for (i, path) in picks.iter().enumerate() {
        let want = archive.read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let ext = Path::new(path).extension().unwrap().to_string_lossy().into_owned();
        let json_file = out_dir.join(format!("f{i}.json"));
        let o = sh(&w.data, &["anim", "export", path, "--from", p(&client), "-o", p(&json_file)]);
        assert_eq!(code(&o), 0, "{path}: {}", err(&o));
        let back = out_dir.join(format!("f{i}.{ext}"));
        let mut args = vec!["anim", "import", p(&json_file), "-o", p(&back)];
        if ext.eq_ignore_ascii_case("ani") {
            args.extend(["--like", path.as_str(), "--from", p(&client)]);
        }
        let o = sh(&w.data, &args);
        assert_eq!(code(&o), 0, "{path}: {}{}", out(&o), err(&o));
        assert_eq!(std::fs::read(&back).unwrap(), want, "{path}: the file made from the JSON differs");
        println!("{path}: {} bytes, {} bytes of JSON", want.len(), std::fs::metadata(&json_file).unwrap().len());
    }
}

// ---- edit (T12 W9) ----------------------------------------------------------------------------------------------------------------

#[test]
fn edit_retime_writes_a_file_that_passes_compare() {
    let w = world();
    let d = w.tmp.path().join("e");
    write(&d.join("a.ANI"), &arm_ani(30));
    let o = sh(&w.data, &["anim", "edit", "retime", p(&d.join("a.ANI")), "--factor", "2", "-o", p(&d.join("slow.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("end key 30 -> 60 (1.00 s -> 2.00 s)"), "{}", out(&o));
    let o = sh(&w.data, &["anim", "compare", p(&d.join("slow.ANI")), p(&d.join("a.ANI"))]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    let v = json(&sh(&w.data, &["anim", "info", p(&d.join("slow.ANI")), "--json"]));
    assert_eq!(v["files"][0]["end_key"], 60);
    // --seconds says the same thing as the matching factor
    let o = sh(&w.data, &["anim", "edit", "retime", p(&d.join("a.ANI")), "--seconds", "2", "-o", p(&d.join("two.ANI")), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!((v["written"].clone(), v["tool"].clone(), v["end_key"]["after"].clone()), (true.into(), "retime".into(), 60.into()), "{v}");
    assert_eq!(std::fs::read(d.join("two.ANI")).unwrap(), std::fs::read(d.join("slow.ANI")).unwrap());
    // trim and copy-track write files that pass too
    let o = sh(&w.data, &["anim", "edit", "trim", p(&d.join("a.ANI")), "--start", "10", "--end", "20", "-o", p(&d.join("cut.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(code(&sh(&w.data, &["anim", "compare", p(&d.join("cut.ANI")), p(&d.join("a.ANI"))])), 0);
    let o = sh(&w.data, &["anim", "edit", "copy-track", p(&d.join("a.ANI")), "--source", p(&d.join("slow.ANI")), "--bones", "1", "--rotation", "-o", p(&d.join("mix.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(code(&sh(&w.data, &["anim", "compare", p(&d.join("mix.ANI")), p(&d.join("a.ANI"))])), 0);
    // the copied rotation of bone 1 runs to key 60, so the end key is 60 (the larger of the two), and the track is the source's
    let v = json(&sh(&w.data, &["anim", "info", p(&d.join("mix.ANI")), "--json"]));
    assert_eq!(v["files"][0]["end_key"], 60, "{v}");
    let (src, mixed) = (std::fs::read(d.join("slow.ANI")).unwrap(), std::fs::read(d.join("mix.ANI")).unwrap());
    let (src, mixed) = (shaiya_clientdata::Ani::parse(&src, "s").unwrap(), shaiya_clientdata::Ani::parse(&mixed, "m").unwrap());
    assert_eq!(mixed.bones[1].rot, src.bones[1].rot);
    // from an archive
    let c = client_with(&w, "ec", &[("Monster/ANI/A.ANI", arm_ani(30))]);
    let o = sh(&w.data, &["anim", "edit", "retime", "Monster/ANI/A.ANI", "--from", p(&c), "--factor", "0.5", "-o", p(&d.join("fast.ANI"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("end key 30 -> 15"), "{}", out(&o));
}

fn sh_owned(data: &Path, args: &[String]) -> std::process::Output {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    sh(data, &refs)
}

#[test]
fn edit_outputs_follow_the_import_rules() {
    let w = world();
    let d = w.tmp.path().join("r");
    write(&d.join("a.ANI"), &arm_ani(30));
    let a_path = d.join("a.ANI");
    let a = p(&a_path);
    fn retime_args<'a>(a: &'a str, out: &'a str) -> Vec<&'a str> {
        vec!["anim", "edit", "retime", a, "--factor", "2", "-o", out]
    }
    let base = |out: &str| retime_args(a, out).into_iter().map(str::to_string).collect::<Vec<String>>();
    // exists: 4, kept; --force replaces
    write(&d.join("x.ANI"), b"old");
    assert_eq!(code(&sh_owned(&w.data, &base(p(&d.join("x.ANI"))))), 4);
    assert_eq!(std::fs::read(d.join("x.ANI")).unwrap(), b"old");
    let mut forced = base(p(&d.join("x.ANI")));
    forced.push("--force".to_string());
    assert_eq!(code(&sh_owned(&w.data, &forced)), 0);
    assert_ne!(std::fs::read(d.join("x.ANI")).unwrap(), b"old");
    // the same file as IN: 4, even with --force
    let mut same = base(a);
    same.push("--force".to_string());
    assert_eq!(code(&sh_owned(&w.data, &same)), 4);
    assert_eq!(std::fs::read(a).unwrap(), arm_ani(30));
    // the same file as SRC: 4, even with --force, and SRC is not changed
    write(&d.join("src.ANI"), &arm_ani(60));
    let o = sh(&w.data, &["anim", "edit", "copy-track", a, "--source", p(&d.join("src.ANI")), "--bones", "1", "--rotation", "-o", p(&d.join("src.ANI")), "--force"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(std::fs::read(d.join("src.ANI")).unwrap(), arm_ani(60));
    // --seconds that needs a factor outside 0.1 to 10 is a bad argument too: 2, nothing written
    for s in ["1000", "0.01"] {
        let o = sh(&w.data, &["anim", "edit", "retime", a, "--seconds", s, "-o", p(&d.join("sec.ANI"))]);
        assert_eq!(code(&o), 2, "--seconds {s}: {}{}", out(&o), err(&o));
        assert!(!d.join("sec.ANI").exists());
    }
    // inside a client folder: 4, nothing made
    let o = sh_owned(&w.data, &base(p(&w.client.join("new.ANI"))));
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(!w.client.join("new.ANI").exists());
    // a folder that does not exist: 3
    assert_eq!(code(&sh_owned(&w.data, &base(p(&d.join("nope").join("y.ANI"))))), 3);
    // a bad argument: 2, nothing written
    for args in [
        vec!["anim", "edit", "retime", a, "--factor", "0", "-o", p(&d.join("z.ANI"))],
        vec!["anim", "edit", "retime", a, "--factor", "11", "-o", p(&d.join("z.ANI"))],
        vec!["anim", "edit", "trim", a, "--start", "20", "--end", "10", "-o", p(&d.join("z.ANI"))],
        vec!["anim", "edit", "copy-track", a, "--source", a, "--bones", "1", "-o", p(&d.join("z.ANI"))],
        vec!["anim", "edit", "copy-track", a, "--source", a, "--bones", "9", "--rotation", "-o", p(&d.join("z.ANI"))],
    ] {
        assert_eq!(code(&sh(&w.data, &args)), 2, "{args:?}");
    }
    // a trim outside the animation does not pass: 1, nothing written
    let o = sh(&w.data, &["anim", "edit", "trim", a, "--start", "10", "--end", "99", "-o", p(&d.join("z.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(out(&o).contains("nothing written") && !d.join("z.ANI").exists());
    // another skeleton as the source: 1, nothing written
    write(&d.join("long.ANI"), &long_chain(4, 0.0));
    let o = sh(&w.data, &["anim", "edit", "copy-track", a, "--source", p(&d.join("long.ANI")), "--all", "--rotation", "-o", p(&d.join("z.ANI"))]);
    assert_eq!(code(&o), 1, "{}", err(&o));
    assert!(err(&o).contains("do not have the same skeleton") && !d.join("z.ANI").exists(), "{}", err(&o));
    // not an .ANI or not readable: 3
    write(&d.join("n.txt"), b"x");
    assert_eq!(code(&sh(&w.data, &["anim", "edit", "retime", p(&d.join("n.txt")), "--factor", "2", "-o", p(&d.join("z.ANI"))])), 3);
}
