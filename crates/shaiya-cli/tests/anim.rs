//! `shaiya anim ...`: info, validate and compare over files, folders and an archive (synthetic files only; the opt-in tests at the end
//! read a real client and write only into `SHAIYA_SCRATCH`).

mod common;

use common::*;
use shaiya_testkit::anim::{ani_bytes, arm_ani, arm_mesh_3dc, c3dc_cloak_bytes, identity_bits, translation_bits, BoneSpec, VertSpec};
use shaiya_testkit::{env_client, env_client_original, env_scratch, SahBuilder};
use std::path::Path;

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
    assert!(out(&o).contains("a_good.ani: animation") && out(&o).contains("notes.txt: FAILED: ") && out(&o).contains("not an .ANI or .3DC file"), "{}", out(&o));
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
    assert!(out(&o).contains("data.saf: FAILED: ") && out(&o).contains("not an .ANI or .3DC file"), "{}", out(&o));
    // validate refuses a .3DC by name too
    let o = sh(&w.data, &["anim", "validate", p(&d.join("m.3dc"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(out(&o).contains("not an .ANI file"), "{}", out(&o));
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
    let (now, stock) = (shaiya_archive::Archive::open(&client).unwrap(), shaiya_archive::Archive::open(&orig).unwrap());
    let out_dir = scratch.join(format!("anim-compare-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
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
    assert_eq!(changed.len(), 2, "{:?}", changed.iter().map(|c| &c.0).collect::<Vec<_>>());
    for (path, file) in &changed {
        let o = sh(&w.data, &["anim", "compare", p(file), path, "--from", p(&orig)]);
        assert_eq!(code(&o), 0, "{path}: {}{}", out(&o), err(&o));
        assert_eq!(out(&o).lines().last(), Some("passes"));
    }
    // every handle is closed before the folder goes (exFAT)
    drop((now, stock));
    remove_dir(&out_dir);
}

fn remove_dir(dir: &Path) {
    std::fs::remove_dir_all(dir).unwrap();
}

