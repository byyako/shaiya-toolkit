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
        assert!(err(&o).contains("is a loose folder") && err(&o).contains("use shaiya loose"), "{args:?}: {}", err(&o));
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

#[test]
fn loose_replace_then_rollback_restores_the_bytes() {
    let (w, root) = loose_world();
    let data = root.join("data");
    let before = snapshot(&data);
    // a loose client with no backup folder is read-only: refused, nothing written anywhere
    assert_eq!(code(&sh(&w.data, &["clients", "add", "nob", p(&root)])), 0);
    let src = w.tmp.path().join("new.txt");
    write(&src, b"replaced");
    let o = sh(&w.data, &["loose", "replace", "@nob", p(&src), "--as", "Item/3DO/a.txt", "--yes-game-stopped"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("choose a backup folder outside the data folder"), "{}", err(&o));
    assert_eq!(snapshot(&data), before);
    // with a confirmed backup folder
    let backups = w.tmp.path().join("my-backups");
    assert_eq!(code(&sh(&w.data, &["clients", "add", "gx", p(&root), "--backups", p(&backups)])), 0);
    // a dry run shows the change and writes nothing, not even the backup folder
    let o = sh(&w.data, &["loose", "replace", "@gx", p(&src), "--as", "item/3do/A.TXT", "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("Changes: 0 added, 1 replaced, 0 removed") && err(&o).contains("Undo:") && err(&o).contains("Game:    restart the game"), "{}", err(&o));
    assert!(!backups.exists() && snapshot(&data) == before);
    // this system cannot tell whether the game runs (no terminal in the test): refused without the confirmation
    let o = sh(&w.data, &["loose", "replace", "@gx", p(&src), "--as", "Item/3DO/a.txt"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("cannot tell whether the game is running: stop it first"), "{}", err(&o));
    assert_eq!(snapshot(&data), before);
    // add refuses a file that is there, replace refuses one that is not
    let o = sh(&w.data, &["loose", "add", "@gx", p(&src), "--as", "Item/3DO/a.txt", "--yes-game-stopped"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("use shaiya loose replace"), "{}", err(&o));
    let o = sh(&w.data, &["loose", "replace", "@gx", p(&src), "--as", "Item/3DO/none.txt", "--yes-game-stopped"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    // the replace itself
    let o = sh(&w.data, &["loose", "replace", "@gx", p(&src), "--as", "item/3do/A.TXT", "--yes-game-stopped"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(data.join("Item/3DO/a.txt")).unwrap(), b"replaced", "the file kept its own spelling");
    // an add of a new file under --at, then the history
    let o = sh(&w.data, &["loose", "add", "@gx", p(&src), "--at", "Item/3DO/new", "--yes-game-stopped"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(data.join("Item/3DO/new/new.txt")).unwrap(), b"replaced");
    let h = out(&sh(&w.data, &["loose", "history", "@gx"]));
    assert!(h.contains("loose replace") && h.contains("loose add") && h.contains("rollback possible"), "{h}");
    // undo, newest first: the add, then the replace, and the folder is as it was
    assert_eq!(code(&sh(&w.data, &["loose", "rollback", "@gx", "--yes-game-stopped"])), 0);
    assert!(!data.join("Item/3DO/new").exists());
    let o = sh(&w.data, &["loose", "undo", "@gx", "--yes-game-stopped"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(snapshot(&data), before);
    // nothing is left to undo
    let o = sh(&w.data, &["loose", "rollback", "@gx", "--yes-game-stopped"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    // the folder is clean, and so is its health
    let o = sh(&w.data, &["loose", "health", "@gx"]);
    assert_eq!(code(&o), 0, "{}{}", out(&o), err(&o));
    assert!(out(&o).contains("4 files listed") && out(&o).contains("nothing needs attention"), "{}", out(&o));
    // a file that fails its reader is refused before anything is written
    let bad = w.tmp.path().join("bad.ANI");
    write(&bad, b"not an animation");
    let o = sh(&w.data, &["loose", "add", "@gx", p(&bad), "--at", "Character/Human/ANI", "--yes-game-stopped"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("bad.ANI"), "{}", err(&o));
    assert_eq!(snapshot(&data), before);
    // a patch applies through the same journal
    let files = w.tmp.path().join("pf");
    write(&files.join("Item/3DO/patched.txt"), b"from a patch");
    let patch = w.tmp.path().join("ps0001.patch");
    let o = sh(&w.data, &["patch", "build", "-o", p(&patch), "--files", p(&files)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let o = sh(&w.data, &["loose", "apply", "@gx", p(&patch), "--yes-game-stopped"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(data.join("Item/3DO/patched.txt")).unwrap(), b"from a patch");
    assert_eq!(code(&sh(&w.data, &["loose", "rollback", "@gx", "--yes-game-stopped"])), 0);
    assert_eq!(snapshot(&data), before);
}

#[test]
fn loose_commands_refuse_an_archive_client() {
    let w = world();
    save_client(&w, "us");
    let before = snapshot(&w.client);
    for args in [vec!["loose", "list", "@us"], vec!["loose", "history", "@us"], vec!["loose", "health", "@us"], vec!["loose", "remove", "@us", "a.txt"]] {
        let o = sh(&w.data, &args);
        assert_eq!(code(&o), 2, "{args:?}: {}", err(&o));
        assert!(err(&o).contains("is not a loose data folder"), "{args:?}: {}", err(&o));
    }
    let o = sh(&w.data, &["loose", "list", p(&w.client)]);
    assert_eq!(code(&o), 2, "{}", err(&o));
    assert_eq!(snapshot(&w.client), before, "nothing was written");
}
