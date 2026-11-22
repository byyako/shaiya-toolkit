//! Checks on files before they go in (`shaiya_inspect::validate`) through `shaiya patch build` and `shaiya archive add/replace`:
//! findings are warnings (stderr, and `warnings` in `--json`), never an error, and exit codes do not change.

mod common;

use common::*;
use shaiya_testkit::tga_stub;
use std::path::PathBuf;

fn codes(v: &serde_json::Value) -> Vec<String> {
    v["warnings"].as_array().expect("a warnings array").iter().map(|w| w["code"].as_str().unwrap().to_string()).collect()
}

/// A folder of mod files with one of each finding and some that are fine.
fn mod_files(w: &World) -> PathBuf {
    let dir = w.tmp.path().join("mods");
    let plain: Vec<u8> = (0..200u32).map(|i| (i * 7 + 1) as u8).collect();
    write(&dir.join("data/Double.SData"), &shaiya_seed::encrypt(&shaiya_seed::encrypt(&plain)));
    let mut bad = shaiya_seed::encrypt(&plain);
    bad[40] ^= 1;
    write(&dir.join("data/Bad.SData"), &bad);
    write(&dir.join("data/Good.SData"), &shaiya_seed::encrypt(&plain));
    write(&dir.join("tex/cut.tga"), &tga_stub(8, 8)[..40]);
    write(&dir.join("tex/fine.tga"), &tga_stub(8, 8));
    write(&dir.join("notes/empty.txt"), b"");
    dir
}

#[test]
fn patch_build_warns_per_file_and_still_builds() {
    let w = world();
    let mods = mod_files(&w);
    let out = w.tmp.path().join("ps0200.patch");
    let o = sh(&w.data, &["patch", "build", "-o", p(&out), "--files", p(&mods)]);
    assert_eq!(code(&o), 0, "findings never change the exit code: {}", err(&o));
    let e = err(&o);
    for want in [
        "warning: data/Double.SData: ",
        "[sdata-double-encrypted]",
        "warning: data/Bad.SData: ",
        "[sdata-crc-bad]",
        "[texture-unreadable]",
        "tex/cut.tga",
        "notes/empty.txt",
        "[empty-file]",
    ] {
        assert!(e.contains(want), "missing {want:?} in {e}");
    }
    assert!(!e.contains("Good.SData") && !e.contains("fine.tga"), "clean files stay quiet: {e}");
    assert!(out.exists(), "the patch is built");
    assert_eq!(code(&sh(&w.data, &["patch", "verify", p(&out)])), 0);

    // JSON: the same findings as a `warnings` array, on a dry run and on the build itself
    let dry = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("ps0201.patch")), "--files", p(&mods), "--dry-run", "--json"]);
    assert_eq!(code(&dry), 0, "{}", err(&dry));
    let mut got = codes(&json(&dry));
    got.sort();
    assert_eq!(got, ["empty-file", "sdata-crc-bad", "sdata-double-encrypted", "texture-unreadable"]);
    let v = json(&dry);
    let first = &v["warnings"][0];
    assert!(first["path"].is_string() && first["message"].is_string() && first["code"].is_string(), "{first}");
    let built = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("ps0202.patch")), "--files", p(&mods), "--json"]);
    assert_eq!(code(&built), 0, "{}", err(&built));
    assert_eq!(codes(&json(&built)).len(), 4);

    // --quiet silences the text, not the JSON
    let q = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("ps0203.patch")), "--files", p(&mods), "--quiet", "--json"]);
    assert!(!err(&q).contains("sdata-crc-bad"), "{}", err(&q));
    assert_eq!(codes(&json(&q)).len(), 4);
}

#[test]
fn patch_build_of_clean_files_has_no_warnings() {
    let w = world();
    let dir = w.tmp.path().join("clean");
    write(&dir.join("data/Good.SData"), &shaiya_seed::encrypt(b"fine body"));
    write(&dir.join("data/Plain.SData"), b"a plain one, no copy to compare with");
    write(&dir.join("tex/fine.tga"), &tga_stub(4, 4));
    let o = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("ps0210.patch")), "--files", p(&dir), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(!err(&o).contains("warning: data/") && !err(&o).contains("warning: tex/"), "{}", err(&o));
    assert_eq!(json(&o)["warnings"], serde_json::json!([]));
}

#[test]
fn loose_files_are_checked_too() {
    let w = world();
    let loose = w.tmp.path().join("loose");
    write(&loose.join("Config.SData"), &shaiya_seed::encrypt(&shaiya_seed::encrypt(b"twice")));
    let o = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("ps0211.patch")), "--loose", p(&loose)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("Config.SData") && err(&o).contains("[sdata-double-encrypted]"), "{}", err(&o));
}

#[test]
fn archive_add_and_replace_compare_sdata_with_the_clients_copy() {
    let w = world();
    save_client(&w, "us");
    let before = snapshot(&w.client);
    let mods = w.tmp.path().join("m");
    // plain Item.SData where the client's copy is encrypted; encrypted Plain.SData where the client's copy is plain
    write(&mods.join("Item/Item.SData"), b"plain replacement for an encrypted item table");
    write(&mods.join("Cn/Plain.SData"), &shaiya_seed::encrypt(b"encrypted replacement"));
    write(&mods.join("Monster/Monster.SData"), &shaiya_seed::encrypt(b"same state as the copy"));
    write(&mods.join("interface/new.tga"), &tga_stub(2, 2));

    let o = sh(&w.data, &["archive", "add", "@us", p(&mods), "--in-place", "--replace", "--dry-run", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!(codes(&v), ["sdata-state-differs", "sdata-state-differs"], "{v}");
    let paths: Vec<&str> = v["warnings"].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"Item/Item.SData") && paths.contains(&"Cn/Plain.SData"), "{paths:?}");
    assert!(err(&o).contains("[sdata-state-differs]"), "{}", err(&o));
    assert_eq!(snapshot(&w.client), before, "a dry run writes nothing");

    // the real change goes ahead (a warning is not an error) and can be undone as always
    let o = sh(&w.data, &["archive", "add", "@us", p(&mods), "--in-place", "--replace"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("Item/Item.SData") && err(&o).contains("plain") && err(&o).contains("encrypted"), "{}", err(&o));
    assert_eq!(out(&sh(&w.data, &["archive", "cat", "@us", "Item/Item.SData"])), "plain replacement for an encrypted item table");
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us"])), 0);

    // replace, into a new pair: the same findings; a clean replacement has none
    let one = w.tmp.path().join("Item.SData");
    write(&one, b"plain again");
    let new = w.tmp.path().join("newpair");
    let o = sh(&w.data, &["archive", "replace", p(&w.client), p(&one), "--as", "Item/Item.SData", "-o", p(&new), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(codes(&json(&o)), ["sdata-state-differs"]);
    let clean = w.tmp.path().join("clean.SData");
    write(&clean, &shaiya_seed::encrypt(b"encrypted like the copy"));
    let o = sh(&w.data, &["archive", "replace", p(&w.client), p(&clean), "--as", "Item/Item.SData", "-o", p(&w.tmp.path().join("newpair2")), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["warnings"], serde_json::json!([]));
}

#[test]
fn archive_add_warns_about_broken_new_files() {
    let w = world();
    let mods = mod_files(&w);
    let o = sh(&w.data, &["archive", "add", p(&w.client), p(&mods), "-o", p(&w.tmp.path().join("out")), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let mut got = codes(&json(&o));
    got.sort();
    assert_eq!(got, ["empty-file", "sdata-crc-bad", "sdata-double-encrypted", "texture-unreadable"]);
}

#[test]
fn archive_replace_with_another_skeleton_warns_and_still_exits_0() {
    use shaiya_testkit::anim::{ani_bytes, arm_ani, translation_bits, BoneSpec};
    use shaiya_testkit::SahBuilder;
    let w = world();
    let client = w.tmp.path().join("anim-client");
    std::fs::create_dir_all(&client).unwrap();
    SahBuilder::new(0).file(b"Character/Human/ANI/huwm_117_test.ANI", &arm_ani(60)).write_to(&client);
    let name = "Character/Human/ANI/huwm_117_test.ANI";
    let sh_ok = sh(&w.data, &["clients", "add", "anim", p(&client)]);
    assert_eq!(code(&sh_ok), 0, "{}", err(&sh_ok));
    let one_bone = ani_bytes(0, 10, &[BoneSpec { parent: -1, matrix: translation_bits(0.0, 0.0, 0.0), rot: Vec::new(), trans: Vec::new() }]);
    let cand = w.tmp.path().join("cand.ANI");
    write(&cand, &one_bone);
    let o = sh(&w.data, &["archive", "replace", "@anim", p(&cand), "--as", name, "--in-place", "--json"]);
    assert_eq!(code(&o), 0, "a finding never changes the exit code: {}", err(&o));
    assert!(err(&o).contains("[anim-rig-differs]") && err(&o).contains("bone count 1 instead of 3"), "{}", err(&o));
    assert_eq!(codes(&json(&o)), ["anim-rig-differs"]);
    assert!(json(&o)["journal"].is_string(), "the journal is written: {}", out(&o));
    assert_eq!(sh(&w.data, &["archive", "cat", "@anim", name]).stdout, one_bone, "the replacement went in");
}
