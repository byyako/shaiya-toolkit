//! A loose client folder on the command line: `clients add`, `info`, `anim --from @name`, and the archive commands refusing it (exit 2).
//! Synthetic files only.

mod common;

use common::*;
use shaiya_testkit::anim::{arm_ani, arm_mesh_3dc};
use std::path::Path;

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// `loose/data/...` (layout 2) with an animation, its mesh and a Shaiya-GX sign (an .ogg under Sound).
fn loose_world() -> (World, std::path::PathBuf) {
    let w = world();
    let root = w.tmp.path().join("loose");
    let data = root.join("data");
    put(&data, "Character/Human/ANI/huwm_117_test.ANI", &arm_ani(60));
    put(&data, "Character/Human/3DC/huwm_torso001.3DC", &arm_mesh_3dc());
    put(&data, "Item/3DO/a.txt", b"hello");
    put(&data, "Sound/Music/a.ogg", b"OggS");
    (w, root)
}

#[test]
fn from_a_loose_client_reads_the_file() {
    let (w, root) = loose_world();
    let o = sh(&w.data, &["clients", "add", "gx", p(&root), "--note", "loose test"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    // the saved entry is a loose client with its data folder and no backup folder
    let v = json(&sh(&w.data, &["clients", "list", "--json"]));
    assert_eq!(v["clients"][0]["kind"], "loose");
    assert!(v["clients"][0]["data"].as_str().unwrap().ends_with("data") && v["clients"][0].get("backups").is_none(), "{v}");
    assert!(out(&sh(&w.data, &["clients", "list"])).contains("[loose]"));
    let o = sh(&w.data, &["clients", "show", "gx"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("loose folder") && t.contains("4 files") && t.contains("Shaiya-GX layout (converted media)") && t.contains("read-only"), "{t}");
    // anim info reads through the loose tree, any letter case
    let o = sh(&w.data, &["anim", "info", "--from", "@gx", "character/human/ani/*"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("huwm_117_test.ANI") && out(&o).contains("3 bones, end key 60"), "{}", out(&o));
    // export one file by its exact path and import it back to the same bytes
    let json_out = w.tmp.path().join("x.json");
    let o = sh(&w.data, &["anim", "export", "Character/Human/ANI/huwm_117_test.ANI", "--from", "@gx", "-o", p(&json_out)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let back = w.tmp.path().join("back.ani");
    let o = sh(&w.data, &["anim", "import", p(&json_out), "-o", p(&back), "--like", "Character/Human/ANI/huwm_117_test.ANI", "--from", "@gx"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(&back).unwrap(), arm_ani(60));
    // a file that is not there is an input error, not a panic
    let o = sh(&w.data, &["anim", "export", "Character/Human/ANI/none.ani", "--from", "@gx", "-o", p(&w.tmp.path().join("n.json"))]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    // a folder works the same without a saved name
    let o = sh(&w.data, &["anim", "validate", "--from", p(&root)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    // shaiya info names the layout (by folder and by @name)
    for arg in [p(&root), "@gx"] {
        let o = sh(&w.data, &["info", arg]);
        assert_eq!(code(&o), 0, "{arg}: {}", err(&o));
        assert!(out(&o).contains("Loose client folder, 4 files") && out(&o).contains("Shaiya-GX layout (converted media)"), "{}", out(&o));
    }
    let v = json(&sh(&w.data, &["info", "@gx", "--json"]));
    let d = &v["files"][0]["details"];
    assert_eq!((v["files"][0]["kind"].as_str(), d["files"].as_u64(), d["gx"].as_bool()), (Some("loose_client"), Some(4), Some(true)), "{v}");
    // nothing in the folder changed and no backup folder was made
    let names: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["data"]);
}

#[test]
fn archive_commands_refuse_a_loose_client_exit_2() {
    let (w, root) = loose_world();
    assert_eq!(code(&sh(&w.data, &["clients", "add", "gx", p(&root)])), 0);
    let before = snapshot(&root);
    for args in [
        vec!["archive", "list", "@gx"],
        vec!["archive", "check", "@gx"],
        vec!["archive", "extract", "@gx", "-o", p(&w.tmp.path().join("o"))],
        vec!["patch", "history", "@gx"],
        vec!["clients", "health", "gx"],
    ] {
        let o = sh(&w.data, &args);
        assert_eq!(code(&o), 2, "{args:?}: {}", err(&o));
        assert!(err(&o).contains("is a loose folder") , "{args:?}: {}", err(&o));
    }
    // the same for the folder itself
    let o = sh(&w.data, &["archive", "list", p(&root)]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert_eq!(snapshot(&root), before, "nothing was written");
    // an archive client is not affected
    assert_eq!(code(&sh(&w.data, &["archive", "check", p(&w.client)])), 0);
}

#[test]
fn a_backup_folder_inside_data_is_refused_when_adding() {
    let (w, root) = loose_world();
    let o = sh(&w.data, &["clients", "add", "gx", p(&root), "--backups", p(&root.join("data").join("b"))]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("outside the data folder"), "{}", err(&o));
    let o = sh(&w.data, &["clients", "add", "gx", p(&root), "--backups", p(&w.tmp.path().join("my-backups"))]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&sh(&w.data, &["clients", "show", "gx", "--json"]));
    assert!(v["backups"].as_str().unwrap().ends_with("my-backups") && v["read_only_reason"].is_null(), "{v}");
    // --backups on an archive folder is refused
    let o = sh(&w.data, &["clients", "add", "us", p(&w.client), "--backups", p(&w.tmp.path().join("b2"))]);
    assert_eq!(code(&o), 4, "{}", err(&o));
}
