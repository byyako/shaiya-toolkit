//! `shaiya clients ...` and the `@name` shortcut.

mod common;

use common::*;

#[test]
fn add_list_show_remove() {
    let w = world();
    // empty list
    let o = sh(&w.data, &["clients", "list"]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("No saved client folders"), "{}", out(&o));
    assert_eq!(json(&sh(&w.data, &["clients", "list", "--json"]))["clients"].as_array().unwrap().len(), 0);
    // add
    let o = sh(&w.data, &["clients", "add", "US 108", p(&w.client), "--note", "stock US"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("use it as @US 108"), "{}", out(&o));
    let o = sh(&w.data, &["clients", "list"]);
    assert!(out(&o).contains("US 108") && out(&o).contains("found") && out(&o).contains("stock US"), "{}", out(&o));
    let v = json(&sh(&w.data, &["clients", "list", "--json"]));
    assert_eq!(v["clients"][0]["name"], "US 108");
    assert_eq!(v["clients"][0]["exists"], true);
    assert!(v["clients"][0]["added_utc"].as_str().unwrap().ends_with('Z'));
    // the shared file is the one Workbench reads: shared.json under the data folder, other keys untouched
    let shared: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.data.join("shared.json")).unwrap()).unwrap();
    assert_eq!(shared["clients"][0]["name"], "US 108");
    // show
    let o = sh(&w.data, &["clients", "show", "us 108"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("SAH version 0, 7 files") && t.contains("CheckVersion 3, CurrentVersion 108") && t.contains("game.exe:  present") && t.contains("changes made by the toolkit: 0"), "{t}");
    let v = json(&sh(&w.data, &["clients", "show", "@US 108", "--json"]));
    assert_eq!((v["check_version"].as_u64(), v["current_version"].as_u64(), v["game_exe"].as_bool(), v["files"].as_u64()), (Some(3), Some(108), Some(true), Some(7)));
    // @name works as an ARCHIVE and a CLIENT argument, case-insensitively
    assert_eq!(code(&sh(&w.data, &["archive", "check", "@US 108"])), 0);
    assert_eq!(code(&sh(&w.data, &["archive", "stats", "@us 108"])), 0);
    // remove: files untouched
    let o = sh(&w.data, &["clients", "remove", "us 108"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(w.client.join("data.sah").exists());
    assert!(out(&sh(&w.data, &["clients", "list"])).contains("No saved"));
}

#[test]
fn bad_names_duplicates_and_unknowns() {
    let w = world();
    // exit 4: bad or duplicate names
    for bad in ["", "a/b", "a\\b", "@x", &"n".repeat(41)] {
        assert_eq!(code(&sh(&w.data, &["clients", "add", bad, p(&w.client)])), 4, "{bad:?}");
    }
    assert_eq!(code(&sh(&w.data, &["clients", "add", "us", p(&w.client)])), 0);
    let o = sh(&w.data, &["clients", "add", "US", p(&w.client)]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("already saved"), "{}", err(&o));
    // exit 3: a folder with no data.sah, an unknown name (with the hint), a path that went away
    assert_eq!(code(&sh(&w.data, &["clients", "add", "empty", p(&w.tmp.path().join("data"))])), 3);
    let o = sh(&w.data, &["clients", "show", "nobody"]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("shaiya clients list"), "{}", err(&o));
    assert_eq!(code(&sh(&w.data, &["clients", "remove", "nobody"])), 3);
    let o = sh(&w.data, &["archive", "list", "@nobody"]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("hint: shaiya clients list"), "{}", err(&o));
    let gone = w.tmp.path().join("gone");
    std::fs::rename(&w.client, &gone).unwrap();
    assert!(out(&sh(&w.data, &["clients", "list"])).contains("missing"));
    assert_eq!(code(&sh(&w.data, &["clients", "show", "us"])), 3);
    assert_eq!(code(&sh(&w.data, &["archive", "list", "@us"])), 3);
}

#[test]
fn the_list_survives_other_keys_and_concurrent_additions() {
    let w = world();
    std::fs::write(w.data.join("shared.json"), r#"{"format": 1, "theme": "light", "future_key": {"a": 1}, "clients": [{"name": "other", "path": "Z:/elsewhere"}]}"#).unwrap();
    assert_eq!(code(&sh(&w.data, &["clients", "add", "us", p(&w.client)])), 0);
    let shared: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.data.join("shared.json")).unwrap()).unwrap();
    assert_eq!((shared["theme"].as_str(), shared["future_key"]["a"].as_u64()), (Some("light"), Some(1)));
    assert_eq!(shared["clients"].as_array().unwrap().len(), 2);
    assert_eq!(shared["clients"][0]["name"], "other");
    // a damaged shared.json is an error, never overwritten
    std::fs::write(w.data.join("shared.json"), "garbage").unwrap();
    assert_eq!(code(&sh(&w.data, &["clients", "add", "x", p(&w.client)])), 3);
    assert_eq!(std::fs::read_to_string(w.data.join("shared.json")).unwrap(), "garbage");
}

#[test]
fn every_group_and_command_has_help() {
    let w = world();
    for c in ["list", "add", "remove", "show"] {
        let o = sh(&w.data, &["clients", c, "--help"]);
        assert_eq!(code(&o), 0, "{c}");
        assert!(out(&o).contains("Example:"), "{c}");
    }
    for g in ["archive", "patch", "sdata", "clients"] {
        assert_eq!(code(&sh(&w.data, &[g, "--help"])), 0, "{g}");
        assert_eq!(code(&sh(&w.data, &["help", g])), 0, "{g}");
    }
    let o = sh(&w.data, &["--help"]);
    assert!(out(&o).trim_end().ends_with("these tools never download game files and never change a file without a backup or an explicit output."), "{}", out(&o));
}
