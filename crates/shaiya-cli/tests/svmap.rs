//! `shaiya svmap ...`: the text, the JSON documents and every exit code of docs/tools/shaiya.md that synthetic files can produce.
//! The numbers on the original server maps are opt-in (`SVMAP_ORIGINALS`, `SHAIYA_CLIENT`).

mod common;

use common::*;
use serde_json::Value;
use shaiya_svmap::{write as write_map, MobArea, MobEntry, Svmap};
use shaiya_svmap_tools::testutil::{not_lossless_wld, tiny_wld};
use shaiya_testkit::SahBuilder;
use std::path::{Path, PathBuf};

fn put(dir: &Path, id: u32, m: &Svmap) -> PathBuf {
    let p = dir.join(format!("{id}.svmap"));
    write(&p, &write_map(m));
    p
}

fn area_map(mobs: &[(u32, u32)]) -> Svmap {
    let mut m = Svmap::new(64);
    m.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [9.0, 2.0, 9.0], mobs: mobs.iter().map(|(mob, count)| MobEntry { mob: *mob, count: *count }).collect() });
    m
}

/// A world with an empty `maps/` folder next to the data folder and the client folder.
fn fixture() -> (World, PathBuf) {
    let w = world();
    let maps = w.tmp.path().join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    (w, maps)
}

/// One good map (id 5) in the folder.
fn two_maps(maps: &Path) {
    let mut m = Svmap::new(16);
    m.points_b.push([1.0, 2.0, 3.0]);
    put(maps, 5, &m);
}

fn error_map() -> Svmap {
    let mut m = Svmap::new(16);
    m.points_b.push([1.0, 2.0, 3.0]);
    m.points_b.push([99.0, 0.0, 0.0]);
    m
}

/// A client folder (extracted) with one lossless `world/1.wld`.
fn wld_client(dir: &Path) -> PathBuf {
    let c = dir.join("wldclient");
    write(&c.join("world").join("1.wld"), &tiny_wld(4).to_bytes());
    c
}

fn error_kind(v: &Value) -> &str {
    v["error"]["kind"].as_str().unwrap_or_else(|| panic!("not an error document: {v}"))
}

// ------------------------------------------------------------------------------------------------------------------ check

#[test]
fn check_text_json_and_the_exit_codes() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let m = p(&maps);
    // 0: no error finding; text has the summary lines
    let o = sh(&w.data, &["svmap", "check", m]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("checked 1 file(s)") && out(&o).contains("errors 0 (+0 unreadable)"), "{}", out(&o));
    let v = json(&sh(&w.data, &["svmap", "check", m, "--json"]));
    assert_eq!((v["kind"].as_str(), v["maps"][0]["map"].as_u64(), v["maps"][0]["read_error"].is_null()), (Some("folder"), Some(5), true));
    assert!(v["maps"][0]["findings"].is_array() && v["server_and_client"].is_null() && v["notes"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["totals"]["files"], 1);
    for k in ["unreadable", "errors", "warnings", "info"] {
        assert!(v["totals"][k].is_u64(), "totals.{k}");
    }
    // a single file
    let one = maps.join("5.svmap");
    let v = json(&sh(&w.data, &["svmap", "check", p(&one), "--json"]));
    assert_eq!(v["kind"], "file");
    // 1: an error finding
    put(&maps, 6, &error_map());
    let o = sh(&w.data, &["svmap", "check", m]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("6.svmap: error") && out(&o).contains("pos-out-of-range"), "{}", out(&o));
    let o = sh(&w.data, &["svmap", "check", m, "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    let f = &v["maps"][1]["findings"][0];
    assert!(f["severity"] == "error" && f["section"].is_string() && f["code"] == "pos-out-of-range" && f["message"].is_string() && f.get("index").is_some() && f.get("sub").is_some(), "{f}");
    assert_eq!(v["totals"]["errors"], 1);
    // 3: a garbage file inside a folder is part of the result (exit 3, the result object is printed)
    write(&maps.join("7.svmap"), &[1, 2, 3]);
    let o = sh(&w.data, &["svmap", "check", m, "--json"]);
    assert_eq!(code(&o), 3);
    let v = json(&o);
    assert!(v["maps"][2]["read_error"].as_str().is_some_and(|s| s.contains("7.svmap")), "{v}");
    assert_eq!(v["totals"]["unreadable"], 1);
    let o = sh(&w.data, &["svmap", "check", m]);
    assert_eq!(code(&o), 3);
    assert!(out(&o).contains("7.svmap: error: cannot read"), "{}", out(&o));
    // a garbage single file
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps.join("7.svmap"))])), 3);
    // 3: no result can be made (missing target, no .svmap in the folder): the error object
    let gone = w.tmp.path().join("gone");
    let o = sh(&w.data, &["svmap", "check", p(&gone), "--json"]);
    assert_eq!(code(&o), 3);
    assert_eq!(error_kind(&json(&o)), "io");
    let o = sh(&w.data, &["svmap", "check", p(&gone)]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).starts_with("error:") && out(&o).is_empty(), "{}", err(&o));
    let empty = w.tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let o = sh(&w.data, &["svmap", "check", p(&empty), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "format".to_string()));
}

#[test]
fn check_usage_errors_exit_2() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let file = maps.join("5.svmap");
    let server = w.tmp.path().join("server");
    std::fs::create_dir_all(&server).unwrap();
    // --afterglow-defs without --server-data (clap)
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--afterglow-defs", "defs.txt"]);
    assert_eq!(code(&o), 2);
    let o = sh(&w.data, &["--json", "svmap", "check", p(&maps), "--afterglow-defs", "defs.txt"]);
    assert_eq!(code(&o), 2);
    assert_eq!(error_kind(&json(&o)), "usage");
    // --server-data with a single file (checked at run time)
    let o = sh(&w.data, &["svmap", "check", p(&file), "--server-data", p(&server)]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).starts_with("error:") && out(&o).is_empty(), "{}", err(&o));
    let o = sh(&w.data, &["svmap", "check", p(&file), "--server-data", p(&server), "--json"]);
    assert_eq!(code(&o), 2);
    assert_eq!(error_kind(&json(&o)), "usage");
    // a bad flag value and a missing argument
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps), "--target", "bogus"])), 2);
    assert_eq!(code(&sh(&w.data, &["svmap", "check"])), 2);
    // --target is accepted with a server data folder; a missing server data folder is exit 3
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps), "--server-data", p(&server), "--target", "afterglow"])), 0);
    let v = json(&sh(&w.data, &["svmap", "check", p(&maps), "--server-data", p(&server), "--json"]));
    assert!(v["server_and_client"].is_array());
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps), "--server-data", p(&server.join("nope"))])), 3);
}

#[test]
fn client_arguments_resolve_like_the_other_commands() {
    let (w, maps) = fixture();
    two_maps(&maps);
    // the world's client holds a data.sah; the editor-style notes (no Monster.SData) come out as `note:` lines and in `notes`
    let c = p(&w.client);
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", c]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("note: Monster.SData did not read"), "{}", err(&o));
    let v = json(&sh(&w.data, &["svmap", "check", p(&maps), "--client", c, "--json"]));
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n.as_str().is_some_and(|s| s.starts_with("Monster.SData did not read"))), "{v}");
    // -q silences the notes (and the JSON still carries them)
    let o = sh(&w.data, &["-q", "svmap", "check", p(&maps), "--client", c]);
    assert_eq!(code(&o), 0);
    assert!(err(&o).is_empty(), "{}", err(&o));
    // a folder's data.sah means its folder; any other file is refused (4); a missing path is exit 3
    let sah = w.client.join("data.sah");
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&sah)])), 0);
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&w.client.join("game.exe"))]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("is not a client folder's data.sah"), "{}", err(&o));
    let gone = w.tmp.path().join("gone");
    assert_eq!(code(&sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&gone)])), 3);
    // @name after `clients add`; an unknown name is exit 3 with the hint
    save_client(&w, "us");
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", "@us"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("note:"), "{}", err(&o));
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", "@nobody"]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("shaiya clients list"), "{}", err(&o));
    // the same for spawns, bulk and wld-check
    assert_eq!(code(&sh(&w.data, &["svmap", "spawns", p(&maps), "--client", p(&w.client.join("game.exe"))])), 4);
    assert_eq!(code(&sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "mob:1 => count +1", "--client", p(&w.client.join("game.exe"))])), 4);
    assert_eq!(code(&sh(&w.data, &["svmap", "wld-check", p(&w.client.join("game.exe"))])), 4);
    assert_eq!(code(&sh(&w.data, &["svmap", "spawns", p(&maps), "--client", p(&gone)])), 3);
    assert_eq!(code(&sh(&w.data, &["svmap", "wld-check", p(&gone)])), 3);
}

// ------------------------------------------------------------------------------------------------------------------ dump, build

#[test]
fn dump_prints_a_summary_or_the_document() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let f = maps.join("5.svmap");
    let o = sh(&w.data, &["svmap", "dump", p(&f)]);
    assert_eq!(code(&o), 0);
    let t = out(&o);
    assert!(t.starts_with("map 5: size 16") && t.contains("section A: 0 records, section B: 1 points") && t.contains("regions: 0"), "{t}");
    let v = json(&sh(&w.data, &["svmap", "dump", p(&f), "--json"]));
    assert_eq!(v["size"], 16);
    assert!(v["blocked"].is_string() && v["points_b"][0][2] == 3.0 && v["portals"].is_array() && v["regions"].is_array(), "{v}");
    // 3: unreadable, not a map, a value JSON cannot carry
    let gone = w.tmp.path().join("gone.svmap");
    let o = sh(&w.data, &["svmap", "dump", p(&gone), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
    write(&maps.join("7.svmap"), &[1, 2, 3]);
    let o = sh(&w.data, &["svmap", "dump", p(&maps.join("7.svmap")), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "format".to_string()));
    assert!(json(&o)["error"]["offset"].is_u64());
    let mut nan = Svmap::new(8);
    nan.points_b.push([f32::NAN, 0.0, 0.0]);
    let n = put(&maps, 8, &nan);
    assert_eq!(code(&sh(&w.data, &["svmap", "dump", p(&n)])), 0, "the text summary needs no JSON");
    let o = sh(&w.data, &["svmap", "dump", p(&n), "--json"]);
    assert_eq!(code(&o), 3);
    assert_eq!(error_kind(&json(&o)), "format");
    assert_eq!(code(&sh(&w.data, &["svmap", "dump"])), 2);
}

#[test]
fn build_writes_atomically_and_refuses_overwrites() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let orig = maps.join("5.svmap");
    // dump --json | build gives the same bytes
    let doc = w.tmp.path().join("5.json");
    write(&doc, &sh(&w.data, &["svmap", "dump", p(&orig), "--json"]).stdout);
    let outp = w.tmp.path().join("built.svmap");
    let o = sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&outp)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).starts_with("wrote ") && out(&o).contains("bytes"), "{}", out(&o));
    assert_eq!(std::fs::read(&outp).unwrap(), std::fs::read(&orig).unwrap());
    // 4: the output exists; nothing changes
    write(&outp, b"keep me");
    let o = sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&outp)]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("use --force"), "{}", err(&o));
    assert_eq!(std::fs::read(&outp).unwrap(), b"keep me");
    // --force replaces it; the JSON report has the documented keys
    let v = json(&sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&outp), "--force", "--json"]));
    assert_eq!(std::fs::read(&outp).unwrap(), std::fs::read(&orig).unwrap());
    assert_eq!((v["size"].as_u64(), v["blocked"].as_u64(), v["bytes"].as_u64()), (Some(16), Some(0), Some(std::fs::read(&orig).unwrap().len() as u64)));
    assert!(v["out"].is_string());
    for k in ["section_a", "points_b", "mob_areas", "npcs", "portals", "binds", "regions"] {
        assert!(v["counts"][k].is_u64(), "counts.{k}");
    }
    assert_eq!(v["counts"]["points_b"], 1);
    // 4: the output is the input, also with --force; the input is untouched
    let before = std::fs::read(&doc).unwrap();
    assert_eq!(code(&sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&doc)])), 4);
    assert_eq!(code(&sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&doc), "--force"])), 4);
    assert_eq!(std::fs::read(&doc).unwrap(), before);
    // 3: input missing, JSON invalid; no output is created and no temporary file is left
    let fresh = w.tmp.path().join("fresh.svmap");
    let bad = w.tmp.path().join("bad.json");
    write(&bad, b"{");
    assert_eq!(code(&sh(&w.data, &["svmap", "build", p(&bad), "-o", p(&fresh)])), 3);
    let o = sh(&w.data, &["svmap", "build", p(&w.tmp.path().join("none.json")), "-o", p(&fresh), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
    assert!(!fresh.exists());
    let leftovers: Vec<String> = snapshot(w.tmp.path()).into_iter().map(|(n, _)| n).filter(|n| n.ends_with(".shaiya-tmp")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // 2: -o is required
    assert_eq!(code(&sh(&w.data, &["svmap", "build", p(&doc)])), 2);
}

// ------------------------------------------------------------------------------------------------------------------ roundtrip, diff

#[test]
fn roundtrip_reports_and_exit_codes() {
    let (w, maps) = fixture();
    two_maps(&maps);
    put(&maps, 6, &area_map(&[(1, 2)]));
    let o = sh(&w.data, &["svmap", "roundtrip", p(&maps)]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    assert!(out(&o).contains("2 files: binary roundtrip identical 2, JSON roundtrip identical 2, differs 0, unreadable 0"), "{}", out(&o));
    let v = json(&sh(&w.data, &["svmap", "roundtrip", p(&maps), "--json"]));
    assert_eq!(v["files"][0]["file"], "5.svmap");
    assert_eq!((v["files"][0]["binary"].as_str(), v["files"][0]["json"].as_str(), v["files"][0]["map"].as_u64()), (Some("identical"), Some("identical"), Some(5)));
    for k in ["first_difference", "length_in", "length_out", "message"] {
        assert!(v["files"][0].get(k).is_some(), "files[0].{k}");
    }
    assert_eq!((v["totals"]["files"].as_u64(), v["totals"]["binary_identical"].as_u64(), v["totals"]["json_identical"].as_u64()), (Some(2), Some(2), Some(2)));
    // 1: a map that reads but whose JSON trip fails (a NaN point)
    let mut nan = Svmap::new(8);
    nan.points_b.push([f32::NAN, 0.0, 0.0]);
    put(&maps, 7, &nan);
    let o = sh(&w.data, &["svmap", "roundtrip", p(&maps), "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!((v["files"][2]["binary"].as_str(), v["files"][2]["json"].as_str()), (Some("identical"), Some("failed")));
    assert_eq!(v["totals"]["differs"], 1);
    let o = sh(&w.data, &["svmap", "roundtrip", p(&maps)]);
    assert!(out(&o).contains("7.svmap: JSON roundtrip failed"), "{}", out(&o));
    // 3: an unreadable file wins over the difference
    write(&maps.join("8.svmap"), &[1, 2, 3]);
    let o = sh(&w.data, &["svmap", "roundtrip", p(&maps), "--json"]);
    assert_eq!(code(&o), 3);
    assert_eq!(json(&o)["files"][3]["binary"], "unreadable");
    // 3: no folder, no .svmap
    let o = sh(&w.data, &["svmap", "roundtrip", p(&w.tmp.path().join("gone")), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
    let empty = w.tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(code(&sh(&w.data, &["svmap", "roundtrip", p(&empty)])), 3);
    assert_eq!(code(&sh(&w.data, &["svmap", "roundtrip"])), 2);
}

#[test]
fn diff_of_files() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let a = maps.join("5.svmap");
    let b = put(&maps, 6, &error_map());
    let o = sh(&w.data, &["svmap", "diff", p(&a), p(&a)]);
    assert_eq!((code(&o), out(&o)), (0, "identical\n".to_string()));
    let v = json(&sh(&w.data, &["svmap", "diff", p(&a), p(&a), "--json"]));
    assert_eq!((v["kind"].as_str(), v["identical"].as_bool()), (Some("files"), Some(true)));
    let o = sh(&w.data, &["svmap", "diff", p(&a), p(&b)]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("section B: 1 added, 0 removed, 0 changed") && out(&o).contains("  + #1"), "{}", out(&o));
    let o = sh(&w.data, &["svmap", "diff", p(&a), p(&b), "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!((v["kind"].as_str(), v["identical"].as_bool()), (Some("files"), Some(false)));
    assert!(v["a"].is_string() && v["b"].is_string());
    assert_eq!((v["sections"][0]["added"].as_u64(), v["sections"][0]["removed"].as_u64(), v["sections"][0]["changed"].as_u64()), (Some(1), Some(0), Some(0)));
    let e = &v["entries"][0];
    assert!(e["section"].is_string() && e["kind"] == "added" && e["index"] == 1 && e.get("index_b").is_some() && e["text"].is_string(), "{e}");
    // 3: unreadable or missing input
    write(&maps.join("7.svmap"), &[1]);
    assert_eq!(code(&sh(&w.data, &["svmap", "diff", p(&a), p(&maps.join("7.svmap"))])), 3);
    assert_eq!(code(&sh(&w.data, &["svmap", "diff", p(&a), p(&maps.join("none.svmap"))])), 3);
}

#[test]
fn diff_of_folders() {
    let w = world();
    let (fa, fb) = (w.tmp.path().join("a"), w.tmp.path().join("b"));
    put(&fa, 1, &area_map(&[(1, 1)]));
    put(&fb, 1, &area_map(&[(1, 1)]));
    put(&fa, 2, &Svmap::new(16));
    put(&fb, 2, &Svmap::new(16));
    // 0: all same
    let o = sh(&w.data, &["svmap", "diff", p(&fa), p(&fb)]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    assert!(out(&o).contains("2 same"), "{}", out(&o));
    let v = json(&sh(&w.data, &["svmap", "diff", p(&fa), p(&fb), "--json"]));
    assert_eq!(v["kind"], "folders");
    assert_eq!((v["totals"]["same"].as_u64(), v["rows"].as_array().unwrap().len()), (Some(2), 2));
    // 1: a map differs, a map is only on one side
    put(&fb, 1, &area_map(&[(1, 2)]));
    put(&fa, 3, &Svmap::new(16));
    let o = sh(&w.data, &["svmap", "diff", p(&fa), p(&fb)]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("1 differ") && out(&o).contains("1 only here"), "{}", out(&o));
    let o = sh(&w.data, &["svmap", "diff", p(&fa), p(&fb), "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    let status: Vec<&str> = v["rows"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(status, ["differs", "same", "only_a"]);
    let c = &v["rows"][0]["counts"];
    assert!(c["sections"][0]["section"].is_string() && c["sections"][0]["differ"] == 1, "{c}");
    for k in ["only_a", "only_b", "differ"] {
        assert!(c["sections"][0].get(k).is_some(), "sections.{k}");
    }
    assert!(c["cells"].is_u64() && c.get("size").is_some());
    assert!(v["rows"][0]["message"].is_null() && v["rows"][1]["counts"].is_null());
    for k in ["same", "differs", "only_a", "only_b", "unreadable"] {
        assert!(v["totals"][k].is_u64(), "totals.{k}");
    }
    // only on the other side
    let o = sh(&w.data, &["svmap", "diff", p(&fb), p(&fa), "--json"]);
    assert_eq!(code(&o), 1);
    assert!(json(&o)["rows"].as_array().unwrap().iter().any(|r| r["status"] == "only_b"));
    // 3: an unreadable map, a missing folder
    write(&fb.join("3.svmap"), &[9]);
    let o = sh(&w.data, &["svmap", "diff", p(&fa), p(&fb), "--json"]);
    assert_eq!(code(&o), 3);
    assert!(json(&o)["rows"].as_array().unwrap().iter().any(|r| r["status"] == "unreadable" && r["message"].is_string()));
    assert_eq!(code(&sh(&w.data, &["svmap", "diff", p(&fa), p(&w.tmp.path().join("gone"))])), 3);
    // 2: one file and one folder, either way round
    let file = fa.join("1.svmap");
    for args in [[p(&file), p(&fb)], [p(&fb), p(&file)]] {
        let o = sh(&w.data, &["--json", "svmap", "diff", args[0], args[1]]);
        assert_eq!(code(&o), 2);
        assert_eq!(error_kind(&json(&o)), "usage");
    }
}

// ------------------------------------------------------------------------------------------------------------------ wld-check

#[test]
fn wld_check_exit_codes_and_json() {
    let w = world();
    let c = wld_client(w.tmp.path());
    let o = sh(&w.data, &["svmap", "wld-check", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("world/1.wld: byte-identical") && out(&o).contains("1 files: byte-identical 1, differs 0, unreadable 0") && out(&o).contains("Every .wld writes back byte for byte.") && !out(&o).contains("can edit"), "{}", out(&o));
    let v = json(&sh(&w.data, &["svmap", "wld-check", p(&c), "--json"]));
    assert!(v["client"].is_string());
    let f = &v["files"][0];
    assert!(f["path"] == "world/1.wld" && f["bytes"].is_u64() && f["status"] == "identical" && f["first_difference"].is_null() && f["written"].is_u64() && f["reason"].is_null(), "{f}");
    assert_eq!((v["totals"]["files"].as_u64(), v["totals"]["identical"].as_u64(), v["totals"]["differs"].as_u64(), v["totals"]["unreadable"].as_u64()), (Some(1), Some(1), Some(0), Some(0)));
    // 1: a file that reads but does not write back identically
    write(&c.join("world").join("2.wld"), &not_lossless_wld(4));
    let o = sh(&w.data, &["svmap", "wld-check", p(&c)]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("world/2.wld: differs, first difference at byte") && out(&o).contains("stay read-only"), "{}", out(&o));
    let o = sh(&w.data, &["svmap", "wld-check", p(&c), "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert!(v["files"][1]["status"] == "differs" && v["files"][1]["first_difference"].is_u64(), "{v}");
    // 3: a junk .wld (an unknown layout counts)
    write(&c.join("world").join("3.wld"), b"junk");
    let o = sh(&w.data, &["svmap", "wld-check", p(&c), "--json"]);
    assert_eq!(code(&o), 3);
    let v = json(&o);
    assert!(v["files"][2]["status"] == "unreadable" && v["files"][2]["reason"].is_string(), "{v}");
    // 3: no world/*.wld (and so no client files at all); a client that cannot be opened
    let empty = w.tmp.path().join("noworld");
    write(&empty.join("world").join("1.txt"), b"x");
    let o = sh(&w.data, &["svmap", "wld-check", p(&empty), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
    assert_eq!(code(&sh(&w.data, &["svmap", "wld-check", p(&w.tmp.path().join("gone"))])), 3);
    assert_eq!(code(&sh(&w.data, &["svmap", "wld-check"])), 2);
}

#[test]
fn wld_check_reads_an_archive_through_a_saved_name() {
    let w = world();
    let c = w.tmp.path().join("archive-client");
    std::fs::create_dir_all(&c).unwrap();
    SahBuilder::new(0).file(b"world/1.wld", &tiny_wld(4).to_bytes()).file(b"world/9.wld", &tiny_wld(4).to_bytes()).write_to(&c);
    let o = sh(&w.data, &["svmap", "wld-check", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("2 files: byte-identical 2"), "{}", out(&o));
    // the data.sah file means its folder
    assert_eq!(code(&sh(&w.data, &["svmap", "wld-check", p(&c.join("data.sah"))])), 0);
    // @name
    assert_eq!(code(&sh(&w.data, &["clients", "add", "wlds", p(&c)])), 0);
    let o = sh(&w.data, &["svmap", "wld-check", "@wlds", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["totals"]["identical"], 2);
}

// ------------------------------------------------------------------------------------------------------------------ spawns, bulk

#[test]
fn spawns_reports_without_judging() {
    let (w, maps) = fixture();
    put(&maps, 1, &area_map(&[(1, 2), (2, 3)]));
    put(&maps, 2, &Svmap::new(16));
    let o = sh(&w.data, &["svmap", "spawns", p(&maps)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("2 maps, 1 areas, 5 monsters, 2 kinds") && t.contains("never spawned: needs --client"), "{t}");
    let v = json(&sh(&w.data, &["svmap", "spawns", p(&maps), "--json"]));
    assert_eq!(v["maps"][0]["map"], 1);
    assert_eq!((v["maps"][0]["areas"].as_u64(), v["maps"][0]["monsters"].as_u64(), v["maps"][0]["kinds"].as_u64()), (Some(1), Some(5), Some(2)));
    assert!(v["maps"][0]["range"].is_null() && v["maps"][0]["findings"].is_object());
    let t = &v["totals"];
    assert_eq!((t["maps"].as_u64(), t["areas"].as_u64(), t["monsters"].as_u64(), t["kinds"].as_u64()), (Some(2), Some(1), Some(5), Some(2)));
    assert!(t["findings"].is_object() && t["never_spawned"].is_null());
    assert!(v["findings"].is_array() && v["never_spawned"].is_null() && v["unreadable"].as_array().unwrap().is_empty() && v["notes"].as_array().unwrap().is_empty());
    // with a client: the notes (a Monster.SData that does not read) go to stderr and into the document
    let o = sh(&w.data, &["svmap", "spawns", p(&maps), "--client", p(&w.client)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("note: Monster.SData did not read"), "{}", err(&o));
    assert!(out(&o).contains("with a readable Monster.SData"), "{}", out(&o));
    let o = sh(&w.data, &["-q", "svmap", "spawns", p(&maps), "--client", p(&w.client), "--json"]);
    assert!(err(&o).is_empty(), "{}", err(&o));
    assert!(!json(&o)["notes"].as_array().unwrap().is_empty());
    // 3: an unreadable file is listed and raises the exit code; the report is still made
    write(&maps.join("3.svmap"), &[1, 2]);
    let o = sh(&w.data, &["svmap", "spawns", p(&maps), "--json"]);
    assert_eq!(code(&o), 3);
    assert_eq!(json(&o)["unreadable"].as_array().unwrap().len(), 1);
    assert!(err(&sh(&w.data, &["svmap", "spawns", p(&maps)])).contains("note: skipped:"));
    // 3: no folder, no .svmap
    assert_eq!(code(&sh(&w.data, &["svmap", "spawns", p(&w.tmp.path().join("gone"))])), 3);
    let empty = w.tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(code(&sh(&w.data, &["svmap", "spawns", p(&empty)])), 3);
}

#[test]
fn bulk_is_a_dry_run_and_a_bad_rule_is_usage() {
    let (w, maps) = fixture();
    put(&maps, 1, &area_map(&[(1, 2), (2, 3)]));
    put(&maps, 2, &area_map(&[(3, 1)]));
    let before = snapshot(&maps);
    let rule = "mob:1 => count +1";
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", rule]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("rule: ") && t.contains("1 area match on 1 map") && t.contains("dry run: nothing was written") && t.contains("5 -> 6"), "{t}");
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", rule, "--json"]);
    assert_eq!(code(&o), 0);
    let v = json(&o);
    assert_eq!(v["dry_run"], true);
    assert!(v["rule"].is_string());
    let m = &v["maps"][0];
    assert_eq!(m["map"], 1);
    for k in ["matched", "changed", "entries", "monsters_before", "monsters_after", "held_at_8", "emptied", "deleted"] {
        assert!(m[k].is_u64(), "maps[0].{k}");
    }
    assert_eq!((m["monsters_before"].as_u64(), m["monsters_after"].as_u64()), (Some(5), Some(6)));
    let t = &v["totals"];
    assert_eq!((t["matched"].as_u64(), t["maps_matched"].as_u64(), t["changed"].as_u64(), t["maps_changed"].as_u64(), t["entries"].as_u64()), (Some(1), Some(1), Some(1), Some(1), Some(1)));
    assert!(t["held_at_8"].is_u64() && t["emptied"].is_u64() && v["unreadable"].is_array() && v["notes"].is_array());
    // counts above 8 are held unless allowed
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "mob:2 => count +10", "--json"]);
    assert_eq!(json(&o)["totals"]["held_at_8"], 1);
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "mob:2 => count +10", "--allow-above-8", "--json"]);
    assert_eq!(json(&o)["totals"]["held_at_8"], 0);
    // 2: the rule does not parse (text and JSON), a rule is required
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "nonsense"]);
    assert_eq!(code(&o), 2);
    assert!(err(&o).starts_with("error: rule:") && out(&o).is_empty(), "{}", err(&o));
    let o = sh(&w.data, &["--json", "svmap", "bulk", p(&maps), "--rule", "nonsense"]);
    assert_eq!(code(&o), 2);
    assert_eq!(error_kind(&json(&o)), "usage");
    assert_eq!(code(&sh(&w.data, &["svmap", "bulk", p(&maps)])), 2);
    // 3: unreadable file (listed, report made), no folder, no .svmap
    write(&maps.join("9.svmap"), &[1]);
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", rule, "--json"]);
    assert_eq!(code(&o), 3);
    assert_eq!(json(&o)["unreadable"].as_array().unwrap().len(), 1);
    assert_eq!(code(&sh(&w.data, &["svmap", "bulk", p(&w.tmp.path().join("gone")), "--rule", rule])), 3);
    // nothing was ever written: every file as before, plus the garbage file made above
    std::fs::remove_file(maps.join("9.svmap")).unwrap();
    assert_eq!(snapshot(&maps), before);
}

// ------------------------------------------------------------------------------------------------------------------ help, every path one JSON value

#[test]
fn every_failure_with_json_prints_exactly_one_json_value() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let gone = p(&w.tmp.path().join("gone")).to_string();
    let m = p(&maps).to_string();
    let cases: Vec<Vec<&str>> = vec![
        vec!["svmap", "check", &gone],
        vec!["svmap", "dump", &gone],
        vec!["svmap", "build", &gone, "-o", &gone],
        vec!["svmap", "roundtrip", &gone],
        vec!["svmap", "diff", &gone, &gone],
        vec!["svmap", "wld-check", &gone],
        vec!["svmap", "spawns", &gone],
        vec!["svmap", "bulk", &gone, "--rule", "mob:1 => count +1"],
        vec!["svmap", "bulk", &m, "--rule", "x"],
        vec!["svmap", "check", &m, "--bogus"],
    ];
    for args in cases {
        let mut a = vec!["--json"];
        a.extend(&args);
        let o = sh(&w.data, &a);
        assert_ne!(code(&o), 0, "{args:?}");
        let v = json(&o);
        assert!(v["error"]["kind"].is_string() && v["error"]["message"].is_string(), "{args:?}: {v}");
    }
}

#[test]
fn svmap_help_covers_the_group_and_every_subcommand() {
    let w = world();
    let o = sh(&w.data, &["--help"]);
    assert!(out(&o).contains("svmap"), "{}", out(&o));
    let o = sh(&w.data, &["svmap", "--help"]);
    assert_eq!(code(&o), 0);
    for c in ["check", "dump", "build", "roundtrip", "diff", "wld-check", "spawns", "bulk"] {
        assert!(out(&o).contains(c), "{c}: {}", out(&o));
        let o = sh(&w.data, &["svmap", c, "--help"]);
        assert_eq!(code(&o), 0, "{c}");
        assert!(out(&o).contains("Usage:") && out(&o).contains("svmap"), "{c}: {}", out(&o));
    }
    // no subcommand: help as a usage error
    assert_eq!(code(&sh(&w.data, &["svmap"])), 2);
    assert_eq!(code(&sh(&w.data, &["svmap", "no-such"])), 2);
}

// ------------------------------------------------------------------------------------------------------------------ opt-in: the original server maps

fn env_dir(var: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => {
            eprintln!("skipped: {var} not set");
            None
        }
    }
}

#[test]
fn the_original_server_maps_through_the_binary() {
    let (Some(orig), Some(client)) = (env_dir("SVMAP_ORIGINALS"), env_dir("SHAIYA_CLIENT")) else { return };
    let w = world();
    let before = snapshot(&orig);
    let o = sh(&w.data, &["svmap", "roundtrip", p(&orig), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!((v["totals"]["files"].as_u64(), v["totals"]["binary_identical"].as_u64(), v["totals"]["json_identical"].as_u64()), (Some(77), Some(77), Some(77)));

    let o = sh(&w.data, &["svmap", "check", p(&orig), "--client", p(&client), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = json(&o)["totals"].clone();
    assert_eq!((t["errors"].as_u64(), t["warnings"].as_u64(), t["info"].as_u64()), (Some(0), Some(146), Some(123)));

    let server = orig.parent().expect("the maps folder has a parent").to_path_buf();
    let o = sh(&w.data, &["svmap", "check", p(&orig), "--client", p(&client), "--server-data", p(&server), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = json(&o)["totals"].clone();
    assert_eq!((t["errors"].as_u64(), t["warnings"].as_u64(), t["info"].as_u64()), (Some(0), Some(156), Some(142)));

    let o = sh(&w.data, &["svmap", "wld-check", p(&client), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["totals"]["identical"], 83);

    let o = sh(&w.data, &["svmap", "bulk", p(&orig), "--rule", "mob:1 => count +1", "--client", p(&client), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["totals"]["matched"], 30);
    assert_eq!(snapshot(&orig), before, "nothing was written to the maps");
}

// ------------------------------------------------------------------------------------------------------------------ notes

/// A readable `.svmap` whose file name is not a map id is left out of `spawns`, `bulk` and a folder `diff`: each says so as a
/// `note:` line and in the `notes` array, never silently.
#[test]
fn a_map_file_without_an_id_name_is_noted_not_skipped_silently() {
    let (w, maps) = fixture();
    put(&maps, 1, &area_map(&[(1, 2)]));
    write(&maps.join("extra.svmap"), &write_map(&area_map(&[(2, 3)])));
    let want = "extra.svmap: the file name is not a map id";
    // spawns
    let o = sh(&w.data, &["svmap", "spawns", p(&maps)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains(&format!("note: {want}")), "{}", err(&o));
    let o = sh(&w.data, &["-q", "svmap", "spawns", p(&maps), "--json"]);
    assert!(err(&o).is_empty(), "{}", err(&o));
    let v = json(&o);
    assert_eq!(v["totals"]["maps"], 1, "only the map with an id is reported");
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with(want)), "{v}");
    // bulk
    let o = sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "mob:1 => count +1"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains(&format!("note: {want}")), "{}", err(&o));
    let v = json(&sh(&w.data, &["svmap", "bulk", p(&maps), "--rule", "mob:1 => count +1", "--json"]));
    assert!(v["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with(want)), "{v}");
    // folder diff: the odd file in B is named with its side
    let other = w.tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    put(&other, 1, &area_map(&[(1, 2)]));
    write(&other.join("odd.svmap"), &write_map(&Svmap::new(16)));
    let o = sh(&w.data, &["svmap", "diff", p(&maps), p(&other)]);
    assert_eq!(code(&o), 0, "{}{}", err(&o), out(&o));
    assert!(err(&o).contains("note: extra.svmap (folder A): the file name is not a map id") && err(&o).contains("note: odd.svmap (folder B): the file name is not a map id"), "{}", err(&o));
    let v = json(&sh(&w.data, &["svmap", "diff", p(&maps), p(&other), "--json"]));
    assert_eq!(v["notes"].as_array().unwrap().len(), 2, "{v}");
}

/// `build -o <its own input>` is refused (exit 4) with a message that does not offer a `--in-place` build does not have.
#[test]
fn build_over_its_own_input_does_not_offer_in_place() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let doc = w.tmp.path().join("doc.json");
    let d = sh(&w.data, &["svmap", "dump", p(&maps.join("5.svmap")), "--json"]);
    assert_eq!(code(&d), 0);
    write(&doc, out(&d).as_bytes());
    let o = sh(&w.data, &["svmap", "build", p(&doc), "-o", p(&doc)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("is the input file") && !err(&o).contains("--in-place"), "{}", err(&o));
}

// ------------------------------------------------------------------------------------------------------------------ final review

#[test]
fn diff_names_a_missing_path_whatever_the_other_side_is() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let gone = w.tmp.path().join("gone");
    let file = maps.join("5.svmap");
    for (a, b) in [(&maps, &gone), (&gone, &maps), (&file, &gone), (&gone, &file)] {
        let o = sh(&w.data, &["svmap", "diff", p(a), p(b)]);
        assert_eq!(code(&o), 3, "{}", err(&o));
        assert!(err(&o).contains("gone") && !err(&o).contains("Access is denied"), "{}", err(&o));
    }
    let o = sh(&w.data, &["svmap", "diff", p(&maps), p(&gone), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
}

#[test]
fn an_empty_client_folder_cannot_be_opened_and_missing_tables_are_called_missing() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let empty = w.tmp.path().join("noclient");
    std::fs::create_dir_all(&empty).unwrap();
    for cmd in [vec!["svmap", "check", p(&maps), "--client", p(&empty)], vec!["svmap", "spawns", p(&maps), "--client", p(&empty)], vec!["svmap", "wld-check", p(&empty)]] {
        let o = sh(&w.data, &cmd);
        assert_eq!(code(&o), 3, "{cmd:?}: {}{}", out(&o), err(&o));
        assert!(err(&o).contains("no client files"), "{}", err(&o));
    }
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&empty), "--json"]);
    assert_eq!((code(&o), error_kind(&json(&o)).to_string()), (3, "io".to_string()));
    // a client that has a map but no tables: "missing", not "did not read"
    let c = wld_client(w.tmp.path());
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("note: Monster.SData is missing") && err(&o).contains("note: NpcQuest.SData is missing") && !err(&o).contains("did not read"), "{}", err(&o));
    // present but junk: "did not read"
    write(&c.join("Monster").join("Monster.SData"), b"junk");
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--client", p(&c)]);
    assert!(err(&o).contains("note: Monster.SData did not read") && err(&o).contains("note: NpcQuest.SData is missing"), "{}", err(&o));
}

#[test]
fn an_empty_server_data_folder_is_said_in_notes_without_changing_the_exit_code() {
    let (w, maps) = fixture();
    two_maps(&maps);
    let server = w.tmp.path().join("server-empty");
    std::fs::create_dir_all(&server).unwrap();
    let o = sh(&w.data, &["svmap", "check", p(&maps), "--server-data", p(&server)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    for f in ["Map.ini", "NpcQuest.SData", "Obelisk.ini"] {
        assert!(err(&o).contains(&format!("note: {f}")), "{f}: {}", err(&o));
    }
    let v = json(&sh(&w.data, &["svmap", "check", p(&maps), "--server-data", p(&server), "--json"]));
    let notes: Vec<&str> = v["notes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(notes.iter().any(|n| n.starts_with("Map.ini: not found")) && notes.iter().any(|n| n.starts_with("Obelisk.ini")), "{notes:?}");
}
