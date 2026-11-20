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
    assert!(out(&o).contains("use it as \"@US 108\""), "{}", out(&o));
    let o = sh(&w.data, &["clients", "list"]);
    assert!(out(&o).contains("US 108") && out(&o).contains("found") && out(&o).contains("stock US"), "{}", out(&o));
    let v = json(&sh(&w.data, &["clients", "list", "--json"]));
    assert_eq!(v["clients"][0]["name"], "US 108");
    assert_eq!(v["clients"][0]["exists"], true);
    assert!(v["clients"][0]["added_utc"].as_str().unwrap().ends_with('Z'));
    // the shared file is the one Shaiya Toolkit reads: shared.json under the data folder, other keys untouched
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
    assert_eq!(err(&o).matches("hint: shaiya clients list").count(), 1, "the hint is printed once: {}", err(&o));
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
    assert!(out(&o).trim_end().ends_with("other output goes only where you say."), "{}", out(&o));
}

// ------------------------------------------------------------------------------------------------ T9 W6: health and clean-up

fn with_leftovers(w: &World) {
    write(&w.client.join("data.sah.bak"), b"old index");
    write(&w.client.join(".n.7.shaiya-tmp"), b"half");
    write(&w.client.join("data.20261101T101010Z.sah"), b"s1");
    write(&w.client.join("data.20261101T101010Z.saf"), b"s2");
}

#[test]
fn health_reports_read_only_with_the_exit_codes_of_the_spec() {
    let w = world();
    save_client(&w, "us");
    let o = sh(&w.data, &["clients", "health", "us"]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("healthy: nothing to report") && out(&o).contains("SAH version 0, 7 files, 0 problems"), "{}", out(&o));
    with_leftovers(&w);
    let before = snapshot(&w.client);
    let o = sh(&w.data, &["clients", "health", "@us"]);
    assert_eq!(code(&o), 1, "{}", out(&o));
    let t = out(&o);
    assert!(t.contains("sah-bak") && t.contains("stray-temp") && t.contains("swap-pair") && t.contains("not pre-selected") && t.contains("3 findings to look at"), "{t}");
    let v = json(&sh(&w.data, &["clients", "health", "us", "--json"]));
    assert_eq!((v["healthy"].as_bool(), v["findings"].as_u64()), (Some(false), Some(3)));
    assert_eq!(v["leftovers"].as_array().unwrap().len(), 3);
    assert_eq!((v["leftovers"][1]["kind"].as_str(), v["leftovers"][1]["pre_selected"].as_bool()), (Some("stray-temp"), Some(true)));
    assert_eq!(v["index"]["files"], 7);
    assert!(v["dead_space"]["dead_bytes"].is_u64() && v["journals"]["count"] == 0);
    // a folder path works like a name
    assert_eq!(code(&sh(&w.data, &["clients", "health", p(&w.client)])), 1);
    assert_eq!(snapshot(&w.client), before, "health wrote nothing");
    // unreadable: no data.sah, an unknown name, a damaged index
    let empty = w.tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(code(&sh(&w.data, &["clients", "health", p(&empty)])), 3);
    assert_eq!(code(&sh(&w.data, &["clients", "health", "nobody"])), 3);
    write(&empty.join("data.sah"), b"not an index");
    assert_eq!(code(&sh(&w.data, &["clients", "health", p(&empty)])), 3);
}

#[test]
fn clean_dry_run_moves_and_puts_back_with_the_exit_codes_of_the_spec() {
    let w = world();
    save_client(&w, "us");
    with_leftovers(&w);
    let before = snapshot(&w.client);
    // dry run: says what, changes nothing
    let o = sh(&w.data, &["clients", "clean", "us", "--move", "stray-temp,sah-bak", "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("would move stray-temp") && out(&o).contains("would move sah-bak") && !out(&o).contains("swap-pair") && out(&o).contains("nothing was changed"), "{}", out(&o));
    let v = json(&sh(&w.data, &["clients", "clean", "us", "--move", "swap-pair", "--dry-run", "--json"]));
    assert_eq!((v["dry_run"].as_bool(), v["files"].as_u64()), (Some(true), Some(2)));
    assert_eq!(snapshot(&w.client), before);
    assert!(!w.client.join("shaiya-toolkit-backups").exists());
    // the real run
    let o = sh(&w.data, &["clients", "clean", "us", "--move", "stray-temp", "--move", "sah-bak", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!(v["moved"].as_array().unwrap().len(), 2);
    let folder = std::path::PathBuf::from(v["path"].as_str().unwrap());
    assert!(folder.join("data.sah.bak").is_file() && !w.client.join("data.sah.bak").exists() && w.client.join("data.20261101T101010Z.sah").exists());
    assert!(folder.join("manifest.json").is_file());
    assert_eq!(code(&sh(&w.data, &["clients", "health", "us"])), 1, "the swap pair is still there");
    // put back: a taken destination is exit 4 and nothing moves
    write(&w.client.join("data.sah.bak"), b"new");
    let o = sh(&w.data, &["clients", "clean", "--put-back", p(&folder)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("data.sah.bak") && folder.join("data.sah.bak").exists());
    std::fs::remove_file(w.client.join("data.sah.bak")).unwrap();
    let o = sh(&w.data, &["clients", "clean", "--put-back", p(&folder)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("put back 2 files"), "{}", out(&o));
    assert_eq!(snapshot(&w.client).iter().filter(|(n, _)| !n.starts_with("shaiya-toolkit-backups/")).cloned().collect::<Vec<_>>(), before);
}

#[test]
fn clean_refuses_with_exit_4_and_usage_errors_are_exit_2() {
    let w = world();
    save_client(&w, "us");
    with_leftovers(&w);
    write(&w.client.join("data.sah.svmap-editor"), b"first");
    write(&w.client.join("svmap-editor-install.json"), b"{}");
    // the editor's backups are not offered while its record exists
    let o = sh(&w.data, &["clients", "clean", "us", "--move", "editor-first-copy"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("svmap-editor-install.json") && w.client.join("data.sah.svmap-editor").exists());
    // a pending marker recovery cannot settle
    write(&w.client.join("data.toolkit-pending"), b"garbage");
    let o = sh(&w.data, &["clients", "clean", "us", "--move", "stray-temp"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(w.client.join(".n.7.shaiya-tmp").exists() && !w.client.join("shaiya-toolkit-backups").exists());
    // usage
    assert_eq!(code(&sh(&w.data, &["clients", "clean", "us"])), 2);
    assert_eq!(code(&sh(&w.data, &["clients", "clean", "us", "--move", "everything"])), 2);
    assert_eq!(code(&sh(&w.data, &["clients", "clean", "us", "--move", "swap-pair", "--put-back", "x"])), 2);
}

#[cfg(windows)]
#[test]
fn clean_is_exit_4_while_data_saf_is_held_open() {
    let w = world();
    save_client(&w, "us");
    with_leftovers(&w);
    let held = std::fs::File::open(w.client.join("data.saf")).unwrap();
    let o = sh(&w.data, &["clients", "clean", "us", "--move", "stray-temp"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("data.saf") && w.client.join(".n.7.shaiya-tmp").exists());
    drop(held);
    assert_eq!(code(&sh(&w.data, &["clients", "clean", "us", "--move", "stray-temp"])), 0);
}

#[test]
fn put_back_says_so_with_exit_3_when_a_file_is_in_neither_place() {
    let w = world();
    save_client(&w, "us");
    with_leftovers(&w);
    let v = json(&sh(&w.data, &["clients", "clean", "us", "--move", "sah-bak,stray-temp", "--json"]));
    let folder = std::path::PathBuf::from(v["path"].as_str().unwrap());
    std::fs::remove_file(folder.join("data.sah.bak")).unwrap();
    let o = sh(&w.data, &["clients", "clean", "--put-back", p(&folder), "--json"]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(err(&o).contains("data.sah.bak"), "{}", err(&o));
    let v = json(&o);
    assert_eq!((v["put_back"]["restored"].as_array().unwrap().len(), v["put_back"]["missing"].as_array().unwrap().len()), (1, 1));
    assert!(w.client.join(".n.7.shaiya-tmp").exists());
}
