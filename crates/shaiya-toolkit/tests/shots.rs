//! T9 W9: the `--shot` scenes of Shaiya Toolkit, run headless on the synthetic demo client: each scene leaves the tab and the data the
//! documentation pictures show, and no scene prints an absolute path. The real window and the PNG are checked by `--shot` itself.

use shaiya_testkit::{demo_client, TempDir};
use shaiya_toolkit::shot::{CLIENT_NAME, SCENES};
use shaiya_toolkit::{Tab, ToolkitApp};
use shaiya_ui::testing::Harness;
use shaiya_ui::{ShellOptions, ShotOptions};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

struct Shot {
    h: Harness<ToolkitApp>,
    dir: TempDir,
}

fn options(dir: &std::path::Path, scene: &str) -> ShellOptions {
    ShellOptions {
        files: vec![dir.to_path_buf()],
        no_settings: true,
        shot: Some(ShotOptions { out: dir.join("never-written.png"), size: None, scene: scene.to_string() }),
        ..Default::default()
    }
}

/// Runs a scene to the moment the real `--shot` would take its picture, then two more frames.
fn scene(name: &str) -> Shot {
    let dir = TempDir::new(&format!("w9-{name}"));
    demo_client(dir.path());
    let mut h = Harness::<ToolkitApp>::new(options(dir.path(), name));
    h.wait_scene(WAIT).unwrap_or_else(|e| panic!("{name}: {e}"));
    h.frames(2);
    Shot { h, dir }
}

/// No absolute path, no temp folder, no folder name of the demo client's real place on screen.
fn assert_no_path(s: &Shot, name: &str) {
    let t = s.h.text();
    let dir = s.dir.path().display().to_string();
    let temp = std::env::temp_dir().display().to_string();
    for bad in [dir.as_str(), temp.as_str(), "shaiya-test-", ":\\", ":/", "/home/", "/Users/"] {
        assert!(!t.contains(bad), "scene {name} shows {bad:?}:\n{t}");
    }
    let leaf = s.dir.path().file_name().unwrap().to_string_lossy().to_string();
    assert!(!t.contains(&leaf), "scene {name} shows the name of the real folder {leaf}:\n{t}");
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(!t.contains(&version), "scene {name} shows the version {version}:\n{t}");
}

fn wait_preview(s: &mut Shot) {
    assert!(s.h.wait_until(WAIT, |h| h.app.app.archive.preview_ready()), "the preview is ready");
    s.h.frames(2);
}

#[test]
fn the_start_scene_shows_the_start_page_with_the_demo_client_saved_by_name() {
    let s = scene("start");
    assert!(!s.h.app.app.archive.is_open());
    assert_eq!(s.h.app.shell.shared().clients.len(), 1);
    assert_eq!(s.h.app.shell.shared().clients[0].name, CLIENT_NAME);
    let t = s.h.text();
    assert!(t.contains("Start") && t.contains("Add a client folder") && t.contains("Check a client folder's health"), "{t}");
    assert_no_path(&s, "start");
}

#[test]
fn the_archive_scene_opens_the_demo_client_with_a_picture_selected() {
    let mut s = scene("archive");
    wait_preview(&mut s);
    assert_eq!(s.h.app.app.tab, Tab::Archive);
    assert!(s.h.app.app.archive.is_open() && s.h.app.app.archive.has_texture());
    let t = s.h.text();
    assert!(t.contains(CLIENT_NAME) && t.contains("data.sah v0, 15 files"), "{t}");
    assert_no_path(&s, "archive");
}

#[test]
fn the_patch_builder_scene_lists_the_two_queued_changes() {
    let s = scene("patch-builder");
    assert_eq!(s.h.app.app.tab, Tab::Patch);
    assert_eq!(s.h.app.app.archive.pending.len(), 2);
    let t = s.h.text();
    assert!(t.contains("2 pending changes of the Archive tab") && t.contains("Demo/new-file.txt") && t.contains("Demo/readme.txt"), "{t}");
    assert_no_path(&s, "patch-builder");
}

#[test]
fn the_apply_history_scene_shows_one_real_journal() {
    let s = scene("apply-history");
    assert_eq!(s.h.app.app.tab, Tab::Apply);
    assert_eq!(s.h.app.app.apply.history().len(), 1);
    assert_eq!(s.h.app.app.apply.history()[0].0.label, "Demo: add a changelog");
    let t = s.h.text();
    assert!(t.contains("Demo: add a changelog"), "{t}");
    // the history shows the fixed demo time, never the real one
    assert_eq!(s.h.app.app.apply.history()[0].0.created_utc, "2010-01-01T00:00:00Z");
    assert!(t.contains("2010-01-01 00:00:00 UTC"), "{t}");
    assert_no_path(&s, "apply-history");
}

#[test]
fn the_sdata_table_scene_shows_the_demo_items_as_a_table() {
    let s = scene("sdata-table");
    let table = s.h.app.app.table.as_ref().expect("the table window is open");
    assert!(table.is_table(), "the demo Item file fits a layout");
    assert_eq!(table.layout_name(), Some("item-81"));
    let t = s.h.text();
    assert!(t.contains("Demo Training Sword") && t.contains("Item/Item.SData"), "{t}");
    assert_no_path(&s, "sdata-table");
}

#[test]
fn the_health_scene_opens_the_report_of_the_demo_client() {
    let s = scene("health");
    assert!(s.h.app.app.health.open);
    let r = s.h.app.app.health.report().expect("the report is read");
    assert!(r.dead_space.as_ref().is_some_and(|d| d.dead_bytes >= 48 * 1024), "the demo client has some unused space in data.saf");
    let t = s.h.text();
    assert!(t.contains("Client health") && t.contains("dead space") && t.contains(CLIENT_NAME), "{t}");
    assert_no_path(&s, "health");
}

#[test]
fn the_map_preview_scene_shows_the_demo_field_map() {
    let mut s = scene("map-preview");
    wait_preview(&mut s);
    assert_eq!(s.h.app.app.archive.preview_kind(), Some("map"));
    assert!(s.h.app.app.archive.has_texture());
    let t = s.h.text();
    assert!(t.contains("Map 7, 17 x 17 corners, 32 units"), "{t}");
    assert_no_path(&s, "map-preview");
}

#[test]
fn the_model_preview_scene_shows_the_textured_demo_box() {
    let mut s = scene("model-preview");
    wait_preview(&mut s);
    assert_eq!(s.h.app.app.archive.preview_kind(), Some("model"));
    assert!(s.h.app.app.archive.has_texture());
    let t = s.h.text();
    assert!(t.contains("1 module, 12 triangles, 1 texture (0 missing)"), "{t}");
    assert_no_path(&s, "model-preview");
}

#[test]
fn the_anim_preview_scene_shows_the_paused_player() {
    let mut s = scene("anim-preview");
    wait_preview(&mut s);
    assert_eq!(s.h.app.app.tab, Tab::Archive);
    assert_eq!(s.h.app.app.archive.preview_kind(), Some("anim"));
    assert!(s.h.app.app.archive.has_texture());
    let p = &s.h.app.app.archive.preview.player;
    assert_eq!(p.t, 30.0, "the player is at key 30");
    assert!(!p.playing, "the player is paused");
    assert!(p.show_bones, "Bones is on");
    let t = s.h.text();
    assert!(t.contains("key 30 of 60, 1.0 s") && t.contains("Mesh: Default parts (1)") && t.contains("Bones (3)"), "{t}");
    assert_no_path(&s, "anim-preview");
}

#[test]
fn the_demo_animation_reads_and_has_a_matching_mesh() {
    // the two made-up files parse to the last byte ...
    let ani = shaiya_clientdata::anim::Ani::parse(&shaiya_testkit::anim::arm_ani(60), "demo_wave.ANI").expect("the demo animation reads");
    assert_eq!((ani.bones.len(), ani.end_key), (3, 60));
    let mesh = shaiya_clientdata::mesh3dc::C3dc::parse(&shaiya_testkit::anim::arm_mesh_3dc(), "demo_arm.3DC").expect("the demo mesh reads");
    assert!(!mesh.meshes.is_empty());
    // ... and the mesh is offered by name for the animation in the demo archive (the 3DC folder next to the ANI folder)
    let mut s = scene("anim-preview");
    wait_preview(&mut s);
    let Some(shaiya_toolkit::preview::PreviewData::Anim { mesh_choices, .. }) = s.h.app.app.archive.preview_data() else { panic!("not an animation preview") };
    let labels: Vec<&str> = mesh_choices.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(labels, ["Skeleton only", "Default parts (1)", "demo_arm.3DC"]);
}

#[test]
fn the_anim_preview_scene_draws_the_demo_arm_with_its_texture() {
    let mut s = scene("anim-preview");
    wait_preview(&mut s);
    let Some(shaiya_toolkit::preview::PreviewData::Anim { frames, notes, .. }) = s.h.app.app.archive.preview_data() else { panic!("not an animation preview") };
    let px = |f: &[u8]| f.chunks(4).filter(|p| p[3] > 0 && (p[0] as i32 - p[2] as i32).abs() > 40).count();
    assert!(px(&frames.frames[30]) > 0, "the texture of the monster list is on the arm");
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn the_demo_lists_name_the_demo_textures_in_their_own_folders() {
    use shaiya_clientdata::{Found, TextureIndex};
    let d = TempDir::new("w6-demo-index");
    demo_client(d.path());
    let a = shaiya_archive::Archive::open(d.path()).unwrap();
    let paths: Vec<String> = a.index().visible().into_iter().map(|f| f.path).collect();
    let (idx, report) = TextureIndex::build(&paths, &mut |p| a.index().get(p).and_then(|e| a.read_entry(e).ok()), &mut || false).unwrap();
    assert_eq!((report.lists_read, report.lists_refused.len()), (2, 0));
    for (mesh, tex) in [("Monster/3DC/demo_arm.3DC", "Monster/DDS/demo_arm.dds"), ("Item/3DO/demo_crate.3DO", "Item/DDS/demo_crate.dds")] {
        let t = idx.for_mesh(mesh);
        assert_eq!((t.len(), t[0].found, t[0].by, t[0].texture.as_str()), (1, true, Found::OwnFolder, tex));
    }
}

#[test]
fn the_inspect_scene_lists_the_loose_files_of_the_demo_client() {
    let s = scene("inspect");
    assert_eq!(s.h.app.app.tab, Tab::Inspect);
    let table = s.h.app.app.inspect.table();
    let names: Vec<&str> = table.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(names, ["data.sah", "Version.ini", "readme.txt", "new-file.txt"]);
    assert!(table[0].3.contains("SAH index"), "{table:?}");
    assert_no_path(&s, "inspect");
}

#[test]
fn every_documented_scene_runs_and_shows_no_path() {
    for name in SCENES {
        let mut s = scene(name);
        s.h.frames(3);
        assert_no_path(&s, name);
        assert!(s.h.app.shot_result().is_some_and(|r| r.is_ok()), "{name}");
    }
}

#[test]
fn a_scene_that_cannot_be_shown_fails_with_the_reason() {
    // unknown scene
    let dir = TempDir::new("w9-unknown");
    demo_client(dir.path());
    let mut h = Harness::<ToolkitApp>::new(options(dir.path(), "nope"));
    let e = h.wait_scene(WAIT).unwrap_err();
    assert!(e.contains("unknown scene") && e.contains("start, archive, patch-builder"), "{e}");
    // no folder given
    let mut h = Harness::<ToolkitApp>::new(ShellOptions { no_settings: true, shot: Some(ShotOptions { out: "x.png".into(), size: None, scene: "archive".into() }), ..Default::default() });
    let e = h.wait_scene(WAIT).unwrap_err();
    assert!(e.contains("name the demo client folder"), "{e}");
    // a folder that is not a demo client is never touched: the apply-history scene would change it
    let other = TempDir::new("w9-other");
    shaiya_testkit::SahBuilder::new(0).file(b"a.txt", b"x").write_to(other.path());
    let before = std::fs::read(other.path().join("data.sah")).unwrap();
    let mut h = Harness::<ToolkitApp>::new(options(other.path(), "apply-history"));
    let e = h.wait_scene(WAIT).unwrap_err();
    assert!(e.contains("not a demo client"), "{e}");
    assert_eq!(std::fs::read(other.path().join("data.sah")).unwrap(), before);
    assert!(!other.path().join("shaiya-toolkit-backups").exists());
}

#[test]
fn a_shot_run_does_not_open_its_files_and_keeps_no_settings() {
    let dir = TempDir::new("w9-noopen");
    demo_client(dir.path());
    let mut h = Harness::<ToolkitApp>::new(options(dir.path(), "start"));
    h.wait_scene(WAIT).unwrap();
    assert!(h.app.shell.hide_paths());
    assert_eq!(h.app.shell.shot_inputs(), [dir.path().to_path_buf()]);
    assert!(h.app.shell.data_dir().is_none(), "no settings folder");
    assert!(!h.app.app.archive.is_open() && h.app.app.inspect.is_empty(), "the folder is an input of the scene, not a dropped file");
}

#[test]
fn the_picker_shows_a_saved_client_by_name_under_shot_and_by_path_otherwise() {
    let c = shaiya_ui::Client { name: "Demo client".into(), path: "/somewhere/else".into(), ..Default::default() };
    assert_eq!(shaiya_toolkit::clients::picker_label(&c, "Demo client", true), "Demo client");
    assert_eq!(shaiya_toolkit::clients::picker_label(&c, "Demo client", false), "Demo client - not found");
    assert_eq!(shaiya_toolkit::clients::picker_label(&c, "/somewhere/else", true), "Demo client  (/somewhere/else)");
}

/// A data folder that already holds the saved clients `saved` (name, folder), the way an earlier normal run leaves it.
fn data_dir_with(tag: &str, saved: &[(&str, &std::path::Path)]) -> TempDir {
    let data = TempDir::new(tag);
    let mut h = Harness::<ToolkitApp>::new(ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    for (name, dir) in saved {
        let a = &mut h.app;
        let mut cx = shaiya_ui::Cx { shell: &mut a.shell, settings: &mut a.tool_settings };
        a.app.add_client(name, dir, true, &mut cx).unwrap();
    }
    h.frames(2);
    data
}

fn shot_with_data(data: &std::path::Path, dir: &std::path::Path, scene: &str) -> Harness<ToolkitApp> {
    let mut o = options(dir, scene);
    o.no_settings = false;
    o.data_dir = Some(data.to_path_buf());
    Harness::<ToolkitApp>::new(o)
}

#[test]
fn a_saved_demo_client_that_is_another_folder_is_refused_under_shot() {
    let demo = TempDir::new("w9fix-demo");
    demo_client(demo.path());
    let elsewhere = TempDir::new("w9fix-elsewhere");
    shaiya_testkit::SahBuilder::new(0).file(b"a.txt", b"x").write_to(elsewhere.path());
    let before = std::fs::read(elsewhere.path().join("data.sah")).unwrap();
    let data = data_dir_with("w9fix-data-a", &[(CLIENT_NAME, elsewhere.path())]);
    let mut h = shot_with_data(data.path(), demo.path(), "apply-history");
    let e = h.wait_scene(WAIT).unwrap_err();
    assert!(e.contains("Demo client") && e.contains("not the demo folder"), "{e}");
    assert!(!h.app.app.archive.is_open(), "the other folder was not opened");
    assert_eq!(std::fs::read(elsewhere.path().join("data.sah")).unwrap(), before);
    assert!(!elsewhere.path().join("shaiya-toolkit-backups").exists());
}

#[test]
fn a_shot_run_with_a_data_dir_does_not_reopen_the_last_client() {
    let demo = TempDir::new("w9fix-demo2");
    demo_client(demo.path());
    let other = TempDir::new("w9fix-other2");
    shaiya_testkit::SahBuilder::new(0).file(b"a.txt", b"x").write_to(other.path());
    // the last client is "Other"; the saved "Demo client" is this very demo folder
    let data = data_dir_with("w9fix-data-b", &[(CLIENT_NAME, demo.path()), ("Other", other.path())]);
    let mut h = shot_with_data(data.path(), demo.path(), "start");
    h.wait_scene(WAIT).unwrap();
    h.frames(3);
    assert!(!h.app.app.archive.is_open(), "the last client is not reopened under --shot");
    assert!(h.app.shell.hide_paths());
}
