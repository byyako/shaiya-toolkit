//! A loose data folder as a client: the Add, Manage and Archive tab behaviour, read only. Synthetic folders only.

use shaiya_testkit::TempDir;
use shaiya_toolkit::clients::{self, FolderKind};
use shaiya_toolkit::{ToolkitApp, ToolkitSettings};
use shaiya_ui::testing::Harness;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

fn with_cx<R>(h: &mut Harness<ToolkitApp>, f: impl FnOnce(&mut ToolkitApp, &mut shaiya_ui::Cx<'_, ToolkitSettings>) -> R) -> R {
    let a = &mut h.app;
    let mut cx = shaiya_ui::Cx { shell: &mut a.shell, settings: &mut a.tool_settings };
    f(&mut a.app, &mut cx)
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A client folder with `data/` inside it (layout 2): a text file, a TGA, an animation and its mesh.
fn gx_like(tag: &str) -> TempDir {
    let t = TempDir::new(tag);
    let data = t.path().join("data");
    put(&data, "Item/3DO/a.txt", b"hello text");
    put(&data, "Item/3DO/b.txt", b"second");
    put(&data, "interface/a.tga", &shaiya_testkit::tga_stub(8, 4));
    put(&data, "Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60));
    put(&data, "Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc());
    put(&data, "World/Maps/1.txt", b"map");
    put(&data, "Item/.a.txt.12.shaiya-tmp", b"left over");
    put(&data, "Sound/Music/a.ogg", b"OggS");
    t
}

struct World {
    h: Harness<ToolkitApp>,
    data: TempDir,
    client: TempDir,
}

fn world(tag: &str) -> World {
    let data = TempDir::new(&format!("{tag}-appdata"));
    let client = gx_like(&format!("{tag}-client"));
    let h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    World { h, data, client }
}

fn open_loose(tag: &str) -> World {
    let mut w = world(tag);
    w.h.frames(2);
    let dir = w.client.path().to_path_buf();
    with_cx(&mut w.h, |app, cx| app.add_client("Loose test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the loose folder opens");
    w.h.frames(2);
    w
}

fn preview_ready(h: &mut Harness<ToolkitApp>) -> bool {
    h.wait_until(WAIT, |h| h.app.app.archive.preview_ready())
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, v: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, v);
            } else {
                v.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

#[test]
fn a_loose_client_browses_and_previews_like_an_archive() {
    let mut w = open_loose("w16-browse");
    let a = &w.h.app.app.archive;
    assert!(a.is_loose(), "the source is a loose folder");
    assert_eq!(a.visible_count(), 7);
    let names = a.table_names();
    assert!(names.contains(&"Item/".to_string()) && names.contains(&"interface/".to_string()), "{names:?}");
    // saved as a loose client of the folder with its data child, no backup folder yet
    let c = w.h.app.shell.shared().clients[0].clone();
    assert_eq!(c.kind, shaiya_core::config::ClientKind::Loose);
    assert_eq!(c.path, w.client.path().display().to_string());
    assert_eq!(c.data.as_deref(), Some(w.client.path().join("data").to_str().unwrap()));
    assert!(c.backups.is_none());
    // search and folders, letter case ignored
    w.h.app.app.archive.search = "TXT".into();
    w.h.frames(2);
    let mut found = w.h.app.app.archive.shown_paths();
    found.sort();
    assert_eq!(found, vec!["Item/3DO/a.txt", "Item/3DO/b.txt", "World/Maps/1.txt"]);
    w.h.app.app.archive.search.clear();
    w.h.frames(1);
    // a text file, a TGA and an animation preview
    with_cx(&mut w.h, |app, cx| app.archive.select_path("item/3do/A.TXT", cx));
    assert!(preview_ready(&mut w.h));
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("text"));
    with_cx(&mut w.h, |app, cx| app.archive.select_path("interface/a.tga", cx));
    assert!(preview_ready(&mut w.h));
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("image"));
    w.h.frames(2);
    assert!(w.h.app.app.archive.has_texture());
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/ANI/huwm_117_test.ANI", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("anim"));
    let t = w.h.text();
    assert!(t.contains("loose folder, 7 files"), "{t}");
    assert!(w.h.app.shell.activity().iter().any(|l| l.text.contains("temporary files")), "what the listing left out is said in the Activity list");
    // extract copies files out and leaves the folder alone
    let before = tree(w.client.path());
    let out = TempDir::new("w16-browse-out");
    with_cx(&mut w.h, |app, cx| app.archive.begin_extract(out.path().to_path_buf(), false, true, false, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_extracting() && !h.app.app.archive.extract_asking()));
    assert_eq!(tree(out.path()).len(), 7);
    assert_eq!(std::fs::read(out.path().join("Item/3DO/a.txt")).unwrap(), b"hello text");
    assert_eq!(tree(w.client.path()), before, "the loose folder is as it was");
}

#[test]
fn archive_only_actions_are_off_for_a_loose_client_with_the_reason() {
    let mut w = open_loose("w16-off");
    let a = &w.h.app.app.archive;
    for action in ["compact", "save-new", "check", "swap", "archive-health"] {
        assert_eq!(a.loose_off_reason(action), Some("a loose folder has no archive"), "{action}");
    }
    // the queue actions are off until a backup folder outside the data folder is confirmed (W17: with one they are on, see loose_write.rs)
    for action in ["add", "replace", "remove", "apply"] {
        assert_eq!(a.loose_off_reason(action), Some("choose a backup folder outside the data folder"), "{action}");
    }
    assert_eq!(a.loose_off_reason("extract"), None);
    let on = |id: &str| a.commands().into_iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is listed")).enabled;
    for id in ["archive-compact", "archive-add", "archive-apply", "archive-save-new", "archive-check", "archive-remove"] {
        assert!(!on(id), "{id} is off");
    }
    assert!(on("archive-extract-all"), "extract works on a loose folder");
    // the queue cannot be filled
    let extra = TempDir::new("w16-off-extra");
    put(extra.path(), "x.txt", b"x");
    w.h.app.app.archive.begin_add(vec![extra.path().join("x.txt")]);
    w.h.app.app.archive.queue_removes(vec!["Item/3DO/a.txt".into()]);
    assert!(w.h.app.app.archive.pending.is_empty());
    // the archive Health window is not opened for it: the loose folder's own window is (W17)
    with_cx(&mut w.h, |app, cx| app.open_health(Some("Loose test"), cx));
    w.h.frames(2);
    assert!(w.h.app.app.loose_health.open && !w.h.app.app.health.open, "the loose health window opens, not the archive one");
}

#[test]
fn export_json_works_from_a_loose_client() {
    let mut w = open_loose("w16-json");
    let out_dir = TempDir::new("w16-json-out");
    let out = out_dir.path().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/ANI/huwm_117_test.ANI", cx));
    assert!(preview_ready(&mut w.h));
    assert!(w.h.app.app.archive.offers_export_json("Character/Human/ANI/huwm_117_test.ANI"));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_exporting_json()));
    let text = std::fs::read_to_string(&out).unwrap();
    let (format, bytes) = shaiya_clientdata::modeljson::import(text.as_bytes()).unwrap();
    assert_eq!(format, shaiya_clientdata::modeljson::ModelFormat::Ani);
    assert_eq!(bytes, shaiya_testkit::anim::arm_ani(60), "the JSON imports to the bytes of the loose file");
    // the other exports are offered too
    assert!(w.h.app.app.archive.offers_export_gltf("Character/Human/3DC/huwm_torso001.3DC"));
}

#[test]
fn save_into_a_loose_client_folder_is_refused() {
    let mut w = open_loose("w16-save");
    let backups = TempDir::new("w16-save-backups");
    let (root, data) = (w.client.path().to_path_buf(), w.client.path().join("data"));
    // a backup folder outside the root, confirmed: nothing may be exported into it either
    clients::set_backups(&mut w.h.app.shell, "Loose test", backups.path()).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/ANI/huwm_117_test.ANI", cx));
    assert!(preview_ready(&mut w.h));
    let before = (tree(&root), tree(backups.path()));
    for (i, target) in [root.join("x.json"), data.join("x.json"), data.join("Item").join("x.json"), backups.path().join("x.json")].into_iter().enumerate() {
        with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&target, cx));
        assert!(!w.h.app.app.archive.is_exporting_json() && !target.exists(), "{i}: nothing starts");
        let d = w.h.app.shell.banner_details().expect("the refusal is shown").to_string();
        assert!(d.contains("Choose a folder outside the client:"), "{i}: {d}");
    }
    assert_eq!((tree(&root), tree(backups.path())), before, "nothing was written in the client or its backup folder");
    // a folder export is refused the same way
    with_cx(&mut w.h, |app, cx| app.archive.export_folder_json_to("Character", data.join("out"), cx));
    assert!(!data.join("out").exists());
    // an outside folder works
    let outside = TempDir::new("w16-save-outside");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&outside.path().join("ok.json"), cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_exporting_json()));
    assert!(outside.path().join("ok.json").is_file());
}

#[test]
fn a_layout_3_client_is_read_only_until_a_backup_folder_is_chosen() {
    let mut w = world("w16-l3");
    w.h.frames(2);
    // layout 3: the folder holds the data folders itself
    let l3 = TempDir::new("w16-l3-client");
    put(l3.path(), "Character/a.txt", b"c");
    put(l3.path(), "Item/b.txt", b"i");
    let dir = l3.path().to_path_buf();
    let FolderKind::Loose(layout) = clients::detect_folder(&dir) else { panic!("layout 3 is detected") };
    assert!(layout.data_is_root() && shaiya_archive::loose::proposed_backups(&layout).is_none(), "no proposal for layout 3");
    with_cx(&mut w.h, |app, cx| app.add_client("L3", &dir, false, cx)).unwrap();
    let c = w.h.app.shell.shared().clients[0].clone();
    assert_eq!(clients::loose_write_problem(&c).as_deref(), Some("choose a backup folder outside the data folder"));
    assert_eq!(clients::status_for(&c).loose_read_only.as_deref(), Some("choose a backup folder outside the data folder"));
    // inside the data folder (the folder itself) is refused; the entry stays read-only
    assert!(clients::set_backups(&mut w.h.app.shell, "L3", &dir).is_err());
    assert!(clients::set_backups(&mut w.h.app.shell, "L3", &dir.join("shaiya-toolkit-backups")).is_err());
    assert!(clients::loose_write_problem(&w.h.app.shell.shared().clients[0].clone()).is_some());
    // a folder outside the data folder is accepted and stored
    let outside = TempDir::new("w16-l3-backups");
    clients::set_backups(&mut w.h.app.shell, "L3", outside.path()).unwrap();
    let c = w.h.app.shell.shared().clients[0].clone();
    assert!(clients::loose_write_problem(&c).is_none());
    w.h.app.flush_settings(true);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(w.data.path().join("shared.json")).unwrap()).unwrap();
    assert_eq!(v["clients"][0]["kind"], "loose");
    assert!(v["clients"][0]["backups"].as_str().is_some() && v["clients"][0].get("data").is_none(), "{v}");
    // layouts 1 and 2 are proposed a folder outside data; the Add path refuses one inside it
    let two = gx_like("w16-l3-two");
    let FolderKind::Loose(l2) = clients::detect_folder(two.path()) else { panic!("layout 2") };
    assert!(clients::add_loose_client(&mut w.h.app.shell, "Two", &l2, Some(&l2.data.join("b"))).is_err());
    assert!(clients::add_loose_client(&mut w.h.app.shell, "Two", &l2, shaiya_archive::loose::proposed_backups(&l2).as_deref()).is_ok());
}

#[test]
fn an_entry_without_kind_that_detects_as_loose_is_offered_for_redetection() {
    // an older version listed the loose folder as an archive client and dropped its kind
    let data = TempDir::new("w16-redetect-appdata");
    let client = gx_like("w16-redetect-client");
    let shared = serde_json::json!({"format": 1, "clients": [{"name": "Old", "path": client.path().display().to_string()}]});
    std::fs::write(data.path().join("shared.json"), serde_json::to_vec_pretty(&shared).unwrap()).unwrap();
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    let c = h.app.shell.shared().clients[0].clone();
    assert_eq!(c.kind, shaiya_core::config::ClientKind::Archive);
    let offer = clients::redetection_offer(&c).expect("offered");
    assert_eq!(offer.data, client.path().join("data"));
    let st = clients::status_for(&c);
    assert!(!st.found && st.redetect.is_some(), "{st:?}");
    // a real archive client is not offered
    let arch = TempDir::new("w16-redetect-arch");
    shaiya_testkit::SahBuilder::new(0).file(b"a.txt", b"x").write_to(arch.path());
    let ac = shaiya_ui::Client { name: "A".into(), path: arch.path().display().to_string(), ..Default::default() };
    assert!(clients::redetection_offer(&ac).is_none());
    // taking the offer saves kind and data, with no backup folder: read-only
    clients::redetect_client(&mut h.app.shell, "Old").unwrap();
    let c = h.app.shell.shared().clients[0].clone();
    assert_eq!((c.kind, c.data.as_deref(), c.backups.as_deref()), (shaiya_core::config::ClientKind::Loose, Some(client.path().join("data").to_str().unwrap()), None));
    assert!(clients::loose_write_problem(&c).is_some());
    let on_disk: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.path().join("shared.json")).unwrap()).unwrap();
    assert_eq!(on_disk["clients"][0]["kind"], "loose");
    // a second offer does not exist
    assert!(clients::redetect_client(&mut h.app.shell, "Old").is_err());
}

#[test]
fn a_dropped_loose_folder_asks_to_add_it() {
    let mut w = world("w16-drop");
    w.h.frames(2);
    let dir = w.client.path().to_path_buf();
    assert!(w.h.app.app.clients.add.is_none());
    w.h.drop_files(std::slice::from_ref(&dir));
    w.h.frames(3);
    let add = w.h.app.app.clients.add.as_ref().expect("the Add dialog opens for the dropped folder");
    assert_eq!(PathBuf::from(&add.path), dir);
    assert!(w.h.app.shell.shared().clients.is_empty(), "nothing is saved until the person confirms");
    let t = w.h.text();
    assert!(t.contains("Loose data folder") && t.contains("Backup folder"), "{t}");
    // a folder that is neither is not offered
    let junk = TempDir::new("w16-drop-junk");
    put(junk.path(), "x.txt", b"x");
    w.h.app.app.clients.add = None;
    w.h.drop_files(&[junk.path().to_path_buf()]);
    w.h.frames(2);
    assert!(w.h.app.app.clients.add.is_none());
}
