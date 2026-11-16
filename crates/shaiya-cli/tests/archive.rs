//! `shaiya archive ...` on synthetic clients: every command in text and JSON, every exit code, dry runs that write nothing.

mod common;

use common::*;
use shaiya_testkit::SahBuilder;

#[test]
fn list_in_every_shape() {
    let w = world();
    save_client(&w, "us");
    // text, patterns, exclude, size filters, sort
    let o = sh(&w.data, &["archive", "list", "@us", "*.SData"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("Item/Item.SData") && t.contains("Monster/Monster.SData") && t.contains("Cn/Plain.SData"), "{t}");
    assert!(!t.contains("a.dds"));
    assert!(err(&o).contains("3 of 7 files"), "{}", err(&o));
    let o = sh(&w.data, &["archive", "list", "@us", "--names-only", "--exclude", "*.SData", "--exclude", "test/"]);
    assert_eq!(out(&o).lines().collect::<Vec<_>>(), vec!["interface/a.dds", "interface/icon/b.txt", "world/map.dg"]);
    let o = sh(&w.data, &["archive", "list", "@us", "--names-only", "--min-size", "100", "--sort", "size", "--reverse"]);
    let names = out(&o);
    assert!(names.starts_with("Monster/Monster.SData") || names.starts_with("Item/Item.SData") || names.starts_with("Cn/Plain.SData") || names.starts_with("world/map.dg"), "{names}");
    assert!(!names.contains("a.dds"), "below the minimum size");
    let o = sh(&w.data, &["archive", "list", "@us", "--long", "Item/Item.SData"]);
    let line = out(&o);
    assert!(line.starts_with("0x0000000000") && line.contains("Item/Item.SData") && line.contains("1970-01-01T00:00:01Z"), "{line}");
    // a --list file with a header and a comment
    let lf = w.tmp.path().join("paths.tsv");
    write(&lf, b"path\tbytes\n# comment\nworld/map.dg\t200\n");
    let o = sh(&w.data, &["archive", "list", "@us", "--names-only", "--list", p(&lf)]);
    assert_eq!(out(&o).trim(), "world/map.dg");
    // folders and tree
    let o = sh(&w.data, &["archive", "list", "@us", "--folders"]);
    let t = out(&o);
    assert!(t.contains("interface/") && t.contains("  icon/") && t.contains("Item/"), "{t}");
    let o = sh(&w.data, &["archive", "list", "@us", "--tree", "interface/"]);
    assert!(out(&o).contains("a.dds"), "{}", out(&o));
    // JSON
    let o = sh(&w.data, &["archive", "list", "@us", "--json", "--folders"]);
    let v = json(&o);
    assert_eq!(v["archive"]["files"], 7);
    assert_eq!(v["archive"]["sah_version"], 0);
    assert_eq!(v["shown"], 7);
    assert_eq!(v["shadowed_hidden"], 0);
    let e = &v["entries"][0];
    for k in ["path", "size", "offset", "version", "version_utc", "shadowed"] {
        assert!(e.get(k).is_some(), "{k} missing in {e}");
    }
    assert!(v["folders"].as_array().unwrap().iter().any(|f| f["path"] == "interface"));
    // exit 3: no such archive, unknown @name (with the hint)
    assert_eq!(code(&sh(&w.data, &["archive", "list", p(&w.tmp.path().join("nothing"))])), 3);
    let o = sh(&w.data, &["archive", "list", "@nobody"]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("shaiya clients list"), "{}", err(&o));
    // exit 2: bad flag value
    assert_eq!(code(&sh(&w.data, &["archive", "list", "@us", "--sort", "colour"])), 2);
    // a bad pattern is an input error
    assert_eq!(code(&sh(&w.data, &["archive", "list", "@us", "[abc"])), 3);
}

#[test]
fn shadowed_entries_are_hidden_unless_asked_for() {
    let w = world();
    SahBuilder::new(0).file(b"w/A.dg", b"old").file(b"w/a.DG", b"newer").file(b"x.txt", b"x").write_to(&w.client);
    let o = sh(&w.data, &["archive", "list", p(&w.client), "--names-only"]);
    assert_eq!(out(&o).lines().collect::<Vec<_>>(), vec!["x.txt", "w/a.DG"]);
    assert!(err(&o).contains("1 shadowed hidden"), "{}", err(&o));
    let o = sh(&w.data, &["archive", "list", p(&w.client), "--names-only", "--include-shadowed"]);
    assert_eq!(out(&o).lines().count(), 3);
    let v = json(&sh(&w.data, &["archive", "list", p(&w.client), "--json"]));
    assert_eq!((v["shown"].as_u64(), v["shadowed_hidden"].as_u64()), (Some(2), Some(1)));
    // check: a warning, exit 0; with --strict the warning counts and the exit is 1
    let o = sh(&w.data, &["archive", "check", p(&w.client)]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("shadowed") && out(&o).contains("w/A.dg"), "{}", out(&o));
    assert_eq!(code(&sh(&w.data, &["archive", "check", p(&w.client), "--strict"])), 1);
}

#[test]
fn stats_check_and_index() {
    let w = world();
    let o = sh(&w.data, &["archive", "stats", p(&w.client), "--top", "3"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("SAH version 0, 7 files (0 shadowed)") && out(&o).contains("By extension"), "{}", out(&o));
    let v = json(&sh(&w.data, &["archive", "stats", p(&w.client), "--json"]));
    for k in ["sah_version", "files", "visible", "shadowed", "dirs", "free_entries", "trailer_hex", "data_end", "saf_len", "live_bytes", "dead_bytes", "zero_length", "by_folder", "by_extension"] {
        assert!(v.get(k).is_some(), "{k}");
    }
    assert_eq!(v["files"], 7);
    // check: clean, json
    let o = sh(&w.data, &["archive", "check", p(&w.client)]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("OK: 0 problems, 0 warnings"), "{}", out(&o));
    let o = sh(&w.data, &["archive", "check", p(&w.client), "--json", "--deep"]);
    let v = json(&o);
    assert_eq!((v["ok"].as_bool(), v["problems"].as_u64(), v["warnings"].as_u64()), (Some(true), Some(0), Some(0)));
    assert!(v["findings"].is_array() && v["layout"].is_object());
    // a SAF cut short: out_of_bounds problems, exit 1, also with --deep
    let cut = w.tmp.path().join("cut");
    std::fs::create_dir_all(&cut).unwrap();
    std::fs::copy(w.client.join("data.sah"), cut.join("data.sah")).unwrap();
    std::fs::write(cut.join("data.saf"), b"short").unwrap();
    for args in [vec!["archive", "check", p(&cut)], vec!["archive", "check", p(&cut), "--deep"]] {
        let o = sh(&w.data, &args);
        assert_eq!(code(&o), 1, "{}", out(&o));
        assert!(out(&o).contains("out_of_bounds") && out(&o).contains("FAILED"), "{}", out(&o));
    }
    // index-only: no .saf at all
    std::fs::remove_file(cut.join("data.saf")).unwrap();
    let o = sh(&w.data, &["archive", "check", p(&cut.join("data.sah"))]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    assert!(err(&o).contains("working from the index only"), "{}", err(&o));
    assert_eq!(code(&sh(&w.data, &["archive", "list", p(&cut.join("data.sah"))])), 0);
    // index: identical, then not identical
    let o = sh(&w.data, &["archive", "index", p(&w.client.join("data.sah"))]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("rewrite identical: yes"), "{}", out(&o));
    let mut sah = std::fs::read(w.client.join("data.sah")).unwrap();
    sah[20] = 0xFF; // inside the 40 bytes of header padding, which a rewrite zeroes
    let odd = w.tmp.path().join("odd.sah");
    std::fs::write(&odd, &sah).unwrap();
    let o = sh(&w.data, &["archive", "index", p(&odd), "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!((v["identical"].as_bool(), v["first_difference"].as_u64()), (Some(false), Some(20)));
    assert_eq!(code(&sh(&w.data, &["archive", "index", p(&w.tmp.path().join("missing.sah"))])), 3);
}

#[test]
fn extract_cat_and_their_exit_codes() {
    let w = world();
    let out_dir = w.tmp.path().join("out");
    let o = sh(&w.data, &["archive", "extract", p(&w.client), "*.dds", "interface/icon/", "-o", p(&out_dir)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("extracted 2 files"), "{}", out(&o));
    assert_eq!(std::fs::read(out_dir.join("interface/a.dds")).unwrap(), b"DDS fake bytes");
    assert_eq!(std::fs::read(out_dir.join("interface/icon/b.txt")).unwrap(), b"hello");
    // targets exist: exit 4; --skip-existing keeps them; --force replaces
    std::fs::write(out_dir.join("interface/a.dds"), b"mine").unwrap();
    assert_eq!(code(&sh(&w.data, &["archive", "extract", p(&w.client), "*.dds", "-o", p(&out_dir)])), 4);
    let o = sh(&w.data, &["archive", "extract", p(&w.client), "*.dds", "-o", p(&out_dir), "--skip-existing"]);
    assert_eq!(code(&o), 0);
    assert_eq!(std::fs::read(out_dir.join("interface/a.dds")).unwrap(), b"mine");
    assert_eq!(code(&sh(&w.data, &["archive", "extract", p(&w.client), "*.dds", "-o", p(&out_dir), "--force"])), 0);
    assert_eq!(std::fs::read(out_dir.join("interface/a.dds")).unwrap(), b"DDS fake bytes");
    // dry run writes nothing
    let fresh = w.tmp.path().join("fresh");
    let o = sh(&w.data, &["archive", "extract", p(&w.client), "-o", p(&fresh), "--dry-run"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("would extract 7 files") && !fresh.exists(), "{}", out(&o));
    let v = json(&sh(&w.data, &["archive", "extract", p(&w.client), "world/", "-o", p(&fresh), "--json"]));
    assert_eq!((v["written"].as_u64(), v["bytes"].as_u64()), (Some(1), Some(200)));
    assert!(v["out"].is_string());
    // a missing saf: exit 3
    let nosaf = w.tmp.path().join("nosaf");
    std::fs::create_dir_all(&nosaf).unwrap();
    std::fs::copy(w.client.join("data.sah"), nosaf.join("data.sah")).unwrap();
    assert_eq!(code(&sh(&w.data, &["archive", "extract", p(&nosaf), "-o", p(&fresh)])), 3);
    // cat: to a file, to a pipe (stdout here is a pipe), not found, existing output
    let f = w.tmp.path().join("b.txt");
    let o = sh(&w.data, &["archive", "cat", p(&w.client), "interface/icon/b.txt", "-o", p(&f)]);
    assert_eq!(code(&o), 0);
    assert_eq!(std::fs::read(&f).unwrap(), b"hello");
    let o = sh(&w.data, &["archive", "cat", p(&w.client), "INTERFACE/ICON/B.TXT"]);
    assert_eq!((code(&o), o.stdout.clone()), (0, b"hello".to_vec()));
    assert_eq!(code(&sh(&w.data, &["archive", "cat", p(&w.client), "no/such.txt"])), 3);
    assert_eq!(code(&sh(&w.data, &["archive", "cat", p(&w.client), "interface/icon/b.txt", "-o", p(&f)])), 4);
    let o = sh(&w.data, &["archive", "cat", p(&w.client), "interface/icon/b.txt", "-o", p(&f), "--force", "--json"]);
    assert_eq!(code(&o), 0);
    assert_eq!(json(&o)["size"], 5);
}

#[test]
fn add_replace_remove_in_place_with_a_journal_and_undo() {
    let w = world();
    save_client(&w, "us");
    let before = snapshot(&w.client);
    let mods = w.tmp.path().join("mods");
    write(&mods.join("interface/t1.txt"), b"new file");
    write(&mods.join("Item/Item.SData"), b"replacement item data");
    // exit 4: neither --in-place nor -o; the file exists without --replace
    let o = sh(&w.data, &["archive", "add", "@us", p(&mods)]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("--in-place"), "{}", err(&o));
    let o = sh(&w.data, &["archive", "add", "@us", p(&mods), "--in-place"]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("Item/Item.SData") && err(&o).contains("already"), "{}", err(&o));
    assert_eq!(snapshot(&w.client), before, "refusals change nothing");
    // dry run: the plan, no files touched (no marker, no journal)
    let o = sh(&w.data, &["archive", "add", "@us", p(&mods), "--in-place", "--replace", "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("would add 1, replace 1, remove 0") && out(&o).contains("+ interface/t1.txt") && out(&o).contains("~ Item/Item.SData"), "{}", out(&o));
    assert_eq!(snapshot(&w.client), before);
    // the real change
    let o = sh(&w.data, &["archive", "add", "@us", p(&mods), "--in-place", "--replace"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("added 1, replaced 1") && out(&o).contains("backup:") && out(&o).contains("shaiya patch rollback @us"), "{}", out(&o));
    assert_eq!(out(&sh(&w.data, &["archive", "cat", "@us", "interface/t1.txt"])), "new file");
    assert_eq!(out(&sh(&w.data, &["archive", "cat", "@us", "Item/Item.SData"])), "replacement item data");
    // history shows it, rollback restores everything byte for byte
    let o = sh(&w.data, &["patch", "history", "@us"]);
    assert!(out(&o).contains("archive-edit") && out(&o).contains("possible"), "{}", out(&o));
    let o = sh(&w.data, &["patch", "rollback", "@us"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let after: Vec<_> = snapshot(&w.client).into_iter().filter(|(n, _)| !n.starts_with("shaiya-toolkit-backups/") && !n.ends_with(".bak")).collect();
    let orig: Vec<_> = before.iter().filter(|(n, _)| !n.ends_with(".bak")).cloned().collect();
    assert_eq!(after, orig, "data.sah and data.saf are byte-identical after the rollback");
    // replace: refuses a path that is not there, --add-missing allows it; --as and --at
    let one = w.tmp.path().join("one.bin");
    write(&one, b"1");
    assert_eq!(code(&sh(&w.data, &["archive", "replace", "@us", p(&one), "--as", "new/place.bin", "--in-place"])), 4);
    let o = sh(&w.data, &["archive", "replace", "@us", p(&one), "--as", "new/place.bin", "--add-missing", "--in-place", "--no-journal"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("no journal"), "{}", err(&o));
    let o = sh(&w.data, &["archive", "add", "@us", p(&one), "--at", "deep/er", "--in-place", "--timestamp", "2026-11-15", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!(v["added"][0], "deep/er/one.bin");
    assert!(v["journal"].is_string() && v["bytes_appended"] == 1);
    let o = sh(&w.data, &["archive", "list", "@us", "deep/", "--long"]);
    assert!(out(&o).contains("2026-11-15T00:00:00Z"), "{}", out(&o));
    // remove: a missing path stops (exit 4) unless --ignore-missing; a list file works
    assert_eq!(code(&sh(&w.data, &["archive", "remove", "@us", "no/such.txt", "--in-place"])), 4);
    let rl = w.tmp.path().join("rm.txt");
    write(&rl, b"path\ntest/junk.eft\nno/such.txt\n");
    let o = sh(&w.data, &["archive", "remove", "@us", "--list", p(&rl), "--ignore-missing", "--in-place"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("remove 1") || out(&o).contains("removed 0, replaced 0, removed 1") || out(&o).contains("- test/junk.eft"), "{}", out(&o));
    assert_eq!(code(&sh(&w.data, &["archive", "cat", "@us", "test/junk.eft"])), 3);
    // a .sah that is not a client's data.sah cannot be changed in place
    let stray = w.tmp.path().join("stray.sah");
    std::fs::copy(w.client.join("data.sah"), &stray).unwrap();
    let o = sh(&w.data, &["archive", "remove", p(&stray), "world/map.dg", "--in-place"]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("use -o"), "{}", err(&o));
    // both --in-place and -o: a usage error
    assert_eq!(code(&sh(&w.data, &["archive", "remove", "@us", "x", "--in-place", "-o", "y"])), 2);
    // nothing to remove
    assert_eq!(code(&sh(&w.data, &["archive", "remove", "@us", "--in-place"])), 4);
}

#[test]
fn edits_into_a_new_pair_leave_the_input_alone() {
    let w = world();
    let before = snapshot(&w.client);
    let new = w.tmp.path().join("new");
    let add = w.tmp.path().join("add.txt");
    write(&add, b"added");
    let o = sh(&w.data, &["archive", "add", p(&w.client), p(&add), "--at", "extra", "-o", p(&new)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(snapshot(&w.client), before);
    assert_eq!(out(&sh(&w.data, &["archive", "cat", p(&new), "extra/add.txt"])), "added");
    assert_eq!(code(&sh(&w.data, &["archive", "check", p(&new), "--deep"])), 0);
    // an existing output needs --force
    assert_eq!(code(&sh(&w.data, &["archive", "add", p(&w.client), p(&add), "-o", p(&new)])), 4);
    assert_eq!(code(&sh(&w.data, &["archive", "add", p(&w.client), p(&add), "-o", p(&new), "--force"])), 0);
    // a .sah path names the index
    let sah_out = w.tmp.path().join("build/x.sah");
    let o = sh(&w.data, &["archive", "remove", p(&w.client), "test/junk.eft", "-o", p(&sah_out), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert!(v["out_sah"].as_str().unwrap().ends_with("x.sah") && v["out_saf"].as_str().unwrap().ends_with("x.saf"));
    assert!(w.tmp.path().join("build/x.saf").exists());
    // the input as its own output is refused
    assert_eq!(code(&sh(&w.data, &["archive", "remove", p(&w.client), "test/junk.eft", "-o", p(&w.client.join("data.sah")), "--force"])), 4);
    // dry run with -o writes nothing
    let nope = w.tmp.path().join("nope");
    let o = sh(&w.data, &["archive", "remove", p(&w.client), "test/junk.eft", "-o", p(&nope), "--dry-run"]);
    assert_eq!(code(&o), 0);
    assert!(!nope.join("data.sah").exists());
}

#[test]
fn compact_in_place_and_to_a_new_pair() {
    let w = world();
    save_client(&w, "us");
    // dead space: replace a big file twice
    let big = w.tmp.path().join("big.bin");
    write(&big, &vec![9u8; 50_000]);
    for _ in 0..2 {
        let o = sh(&w.data, &["archive", "add", "@us", p(&big), "--as", "world/map.dg", "--replace", "--in-place", "--no-journal"]);
        assert_eq!(code(&o), 0, "{}", err(&o));
    }
    let o = sh(&w.data, &["archive", "stats", "@us", "--json"]);
    let dead = json(&o)["dead_bytes"].as_u64().unwrap();
    assert!(dead > 40_000, "{dead}");
    assert_eq!(code(&sh(&w.data, &["archive", "compact", "@us"])), 4, "neither --in-place nor -o");
    // dry run and --min-dead write nothing
    let before = snapshot(&w.client);
    let o = sh(&w.data, &["archive", "compact", "@us", "--in-place", "--dry-run"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("would rewrite data.saf") && out(&o).contains("needs about"), "{}", out(&o));
    let o = sh(&w.data, &["archive", "compact", "@us", "--in-place", "--min-dead", "1GiB"]);
    assert!(out(&o).contains("nothing to do"), "{}", out(&o));
    assert_eq!(snapshot(&w.client), before);
    // a compacted copy leaves the client alone
    let copy = w.tmp.path().join("compacted");
    let o = sh(&w.data, &["archive", "compact", "@us", "-o", p(&copy)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(snapshot(&w.client), before);
    assert_eq!(json(&sh(&w.data, &["archive", "stats", p(&copy), "--json"]))["dead_bytes"], 0);
    // the real thing
    let o = sh(&w.data, &["archive", "compact", "@us", "--in-place", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(json(&o)["after"].as_u64().unwrap() < json(&o)["before"].as_u64().unwrap());
    assert_eq!(json(&sh(&w.data, &["archive", "stats", "@us", "--json"]))["dead_bytes"], 0);
    assert_eq!(sh(&w.data, &["archive", "cat", "@us", "world/map.dg"]).stdout, vec![9u8; 50_000]);
}

#[test]
fn diff_summary_exit_codes_and_filters() {
    let w = world();
    let other = w.tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    SahBuilder::new(0)
        .file(b"Item/Item.SData", &shaiya_seed::encrypt(ITEM_PLAIN))
        .file(b"Monster/Monster.SData", &shaiya_seed::encrypt(b"monster CHANGED!!"))
        .file(b"Cn/Plain.SData", b"plain sdata of a client that does not encrypt")
        .file(b"interface/a.dds", b"DDS fake bytes")
        .file(b"interface/icon/b.txt", b"hello")
        .file(b"world/map.dg", &[7u8; 200])
        .file(b"brand/new.txt", b"n")
        .write_to(&other);
    // the same archive: exit 0
    let o = sh(&w.data, &["archive", "diff", p(&w.client), p(&w.client), "--quick", "--summary"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o).trim(), "0 removed, 0 added, 0 different, 7 same, 0 unknown");
    // different: exit 1, markers, --only, --show-same, patterns
    let o = sh(&w.data, &["archive", "diff", p(&w.client), p(&other)]);
    assert_eq!(code(&o), 1);
    let t = out(&o);
    assert!(t.contains("+ brand/new.txt") && t.contains("- test/junk.eft") && t.contains("~ Monster/Monster.SData"), "{t}");
    assert!(!t.contains("= "), "same files are not listed by default");
    let o = sh(&w.data, &["archive", "diff", p(&w.client), p(&other), "--only", "added", "--show-same"]);
    assert!(out(&o).contains("+ brand/new.txt") && out(&o).contains("= world/map.dg") && !out(&o).contains("- test"), "{}", out(&o));
    let o = sh(&w.data, &["archive", "diff", p(&w.client), p(&other), "Monster/*", "--summary"]);
    assert_eq!(out(&o).trim(), "0 removed, 0 added, 1 different, 0 same, 0 unknown");
    let v = json(&sh(&w.data, &["archive", "diff", p(&w.client), p(&other), "--json"]));
    assert_eq!((v["summary"]["added"].as_u64(), v["summary"]["removed"].as_u64(), v["summary"]["different"].as_u64()), (Some(1), Some(1), Some(1)));
    assert!(v["entries"].as_array().unwrap().iter().all(|e| e["change"] != "same"));
    // index-only sides switch to the index comparison with a note
    std::fs::remove_file(other.join("data.saf")).unwrap();
    let o = sh(&w.data, &["archive", "diff", p(&w.client), p(&other.join("data.sah")), "--summary"]);
    assert_eq!(code(&o), 1);
    assert!(err(&o).contains("index only"), "{}", err(&o));
    assert_eq!(code(&sh(&w.data, &["archive", "diff", p(&w.client), p(&w.tmp.path().join("none"))])), 3);
}

#[test]
fn pack_swap_and_their_refusals() {
    let w = world();
    let src = w.tmp.path().join("src");
    write(&src.join("Item/Item.SData"), b"item");
    write(&src.join("top.txt"), b"top");
    write(&src.join("Thumbs.db"), b"junk");
    let dst = w.tmp.path().join("packed");
    let o = sh(&w.data, &["archive", "pack", p(&src), "-o", p(&dst), "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("would pack 2 files") && !dst.exists(), "{}", out(&o));
    let o = sh(&w.data, &["archive", "pack", p(&src), "-o", p(&dst), "--timestamp", "1274079412", "--sah-version", "1"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("packed 2 files in 1 folder"), "{}", out(&o));
    assert!(err(&o).contains("Thumbs.db"), "{}", err(&o));
    let v = json(&sh(&w.data, &["archive", "list", p(&dst), "--json"]));
    assert_eq!(v["archive"]["sah_version"], 1);
    assert_eq!(v["entries"][0]["version"], 1_274_079_412);
    assert_eq!(code(&sh(&w.data, &["archive", "pack", p(&src), "-o", p(&dst)])), 4, "existing output");
    assert_eq!(code(&sh(&w.data, &["archive", "pack", p(&src), "-o", p(&dst), "--force", "--include-all", "--json"])), 0);
    write(&src.join("\u{1F600}.txt"), b"x");
    assert_eq!(code(&sh(&w.data, &["archive", "pack", p(&src), "-o", p(&w.tmp.path().join("p2"))])), 4, "a name cp949 cannot hold");
    assert_eq!(code(&sh(&w.data, &["archive", "pack", p(&w.tmp.path().join("missing")), "-o", p(&dst), "--force"])), 3);
    // swap: dry run shows four renames, the real swap keeps the old pair, and swapping back works
    save_client(&w, "us");
    let before = snapshot(&w.client);
    let o = sh(&w.data, &["archive", "swap", "@us", p(&dst), "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(out(&o).matches("would rename").count(), 4, "{}", out(&o));
    assert_eq!(snapshot(&w.client), before);
    let o = sh(&w.data, &["archive", "swap", "@us", p(&dst)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("the old pair is kept as data."), "{}", out(&o));
    assert_eq!(json(&sh(&w.data, &["archive", "stats", "@us", "--json"]))["files"], 3);
    let kept = std::fs::read_dir(&w.client).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("data.2") && n.ends_with(".sah")).unwrap();
    let o = sh(&w.data, &["archive", "swap", "@us", p(&w.client.join(&kept)), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&sh(&w.data, &["archive", "stats", "@us", "--json"]))["files"], 7);
    assert_eq!(code(&sh(&w.data, &["archive", "swap", "@us", p(&w.tmp.path().join("nothing"))])), 3);
}

#[test]
fn every_archive_command_has_help() {
    let w = world();
    for c in ["list", "stats", "check", "index", "extract", "cat", "add", "replace", "remove", "compact", "diff", "pack", "swap"] {
        let o = sh(&w.data, &["archive", c, "--help"]);
        assert_eq!(code(&o), 0, "{c}");
        assert!(out(&o).contains(&format!("shaiya archive {c}")) || out(&o).contains("Usage"), "{c}: {}", out(&o));
        assert!(out(&o).contains("Example:") || c == "stats" || out(&o).len() > 100, "{c}");
        let o = sh(&w.data, &["help", "archive", c]);
        assert_eq!(code(&o), 0, "help archive {c}");
    }
    assert!(out(&sh(&w.data, &["archive", "extract", "--help"])).contains("shadowed"));
    assert_eq!(code(&sh(&w.data, &["archive"])), 2, "a group alone shows help as a usage error");
}

#[test]
fn unreadable_inputs_exit_3() {
    let w = world();
    let none = w.tmp.path().join("nothing");
    for args in [
        vec!["archive", "stats", p(&none)],
        vec!["archive", "check", p(&none)],
        vec!["archive", "add", p(&w.client), p(&none), "--in-place"],
        vec!["archive", "replace", p(&w.client), p(&none), "-o", "x"],
        vec!["archive", "remove", p(&none), "a/b.txt", "--in-place"],
        vec!["archive", "compact", p(&none), "-o", "x"],
        vec!["archive", "extract", p(&none), "-o", "x"],
        vec!["archive", "cat", p(&none), "a"],
    ] {
        assert_eq!(code(&sh(&w.data, &args)), 3, "{args:?}");
    }
}
