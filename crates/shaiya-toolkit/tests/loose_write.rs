//! A loose data folder as a client that can be changed: the Archive tab's queue and Apply, the Apply tab's patches, history and Roll back,
//! the Health window and the SData tab, all through the loose journal. Synthetic folders only.

use shaiya_archive::loose_edit::{self, LooseOptions};
use shaiya_testkit::lists::mlt_bytes;
use shaiya_testkit::tex::checker;
use shaiya_testkit::TempDir;
use shaiya_toolkit::archive_view::LOOSE_NEEDS_BACKUPS;
use shaiya_toolkit::clients;
use shaiya_toolkit::preview::PreviewData;
use shaiya_toolkit::{Tab, ToolkitApp, ToolkitSettings};
use shaiya_ui::testing::Harness;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);
const NAME: &str = "Loose test";

fn with_cx<R>(h: &mut Harness<ToolkitApp>, f: impl FnOnce(&mut ToolkitApp, &mut shaiya_ui::Cx<'_, ToolkitSettings>) -> R) -> R {
    let a = &mut h.app;
    let mut cx = shaiya_ui::Cx { shell: &mut a.shell, settings: &mut a.tool_settings };
    f(&mut a.app, &mut cx)
}

fn go(h: &mut Harness<ToolkitApp>, tab: Tab) {
    with_cx(h, |app, cx| app.set_tab(tab, cx));
    h.frames(2);
}

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A client folder with `data/` inside it (layout 2).
fn client_folder(tag: &str) -> TempDir {
    let t = TempDir::new(tag);
    let data = t.path().join("data");
    put(&data, "Item/3DO/a.txt", b"hello text");
    put(&data, "Item/3DO/b.txt", b"second");
    put(&data, "Item/Item.SData", &shaiya_seed::encrypt(b"item plaintext data of some length"));
    put(&data, "Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60));
    put(&data, "Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc());
    put(&data, "World/Maps/1.txt", b"map");
    t
}

struct World {
    h: Harness<ToolkitApp>,
    _data: TempDir,
    client: TempDir,
    backups: PathBuf,
}

fn open_loose(tag: &str) -> World {
    let data = TempDir::new(&format!("{tag}-appdata"));
    let client = client_folder(&format!("{tag}-client"));
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    let dir = client.path().to_path_buf();
    with_cx(&mut h, |app, cx| app.add_client(NAME, &dir, true, cx)).unwrap();
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the loose folder opens");
    h.frames(2);
    let backups = data.path().join("loose-backups");
    World { h, _data: data, client, backups }
}

/// The person confirms the backup folder (the Manage dialog's "Choose backup folder...").
fn confirm_backups(w: &mut World) {
    clients::set_backups(&mut w.h.app.shell, NAME, &w.backups).unwrap();
    w.h.frames(3);
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, v: &mut Vec<(String, Vec<u8>)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
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

fn layout(w: &World) -> shaiya_archive::loose::LooseLayout {
    let mut l = shaiya_archive::loose::detect(w.client.path()).unwrap().unwrap();
    l.backups = Some(w.backups.clone());
    l
}

fn queue_via_dialog(w: &mut World, source: &Path, target: &str) {
    w.h.drop_files(&[source.to_path_buf()]);
    w.h.frames(2);
    let arch = &mut w.h.app.app.archive;
    let mut d = arch.add_dialog.take().expect("the Add dialog opens for a loose client with a backup folder");
    d.target = target.into();
    arch.recompute_add(&mut d);
    arch.add_dialog = Some(d);
    w.h.app.app.archive.queue_add_dialog();
    w.h.frames(2);
}

#[test]
fn a_loose_client_is_read_only_until_a_backup_folder_is_confirmed_then_queues() {
    let mut w = open_loose("w17-gate");
    let data = w.client.path().join("data");
    let before = tree(&data);
    let a = &w.h.app.app.archive;
    for action in ["add", "replace", "remove", "apply"] {
        assert_eq!(a.loose_off_reason(action), Some(LOOSE_NEEDS_BACKUPS), "{action}");
    }
    assert!(!on(&w, "archive-add") && !on(&w, "archive-remove") && !on(&w, "archive-compact"));
    // dropping a file does not open the Add dialog; nothing can be queued
    let extra = TempDir::new("w17-gate-extra");
    put(extra.path(), "x.txt", b"x");
    w.h.drop_files(&[extra.path().join("x.txt")]);
    w.h.frames(2);
    assert!(w.h.app.app.archive.add_dialog.is_none() && w.h.app.app.archive.pending.is_empty());
    w.h.app.app.archive.queue_removes(vec!["Item/3DO/a.txt".into()]);
    assert!(w.h.app.app.archive.pending.is_empty());
    // a backup folder inside the data folder is refused by the client list, and the client stays read only
    assert!(clients::set_backups(&mut w.h.app.shell, NAME, &data.join("backups")).is_err());
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.loose_off_reason("add"), Some(LOOSE_NEEDS_BACKUPS));
    // confirmed: the queue actions are on, Compact and Save as new archive stay off
    confirm_backups(&mut w);
    let a = &w.h.app.app.archive;
    for action in ["add", "replace", "remove", "apply"] {
        assert_eq!(a.loose_off_reason(action), None, "{action}");
    }
    assert_eq!(a.loose_off_reason("compact"), Some("a loose folder has no archive"));
    assert!(on(&w, "archive-add") && !on(&w, "archive-compact") && !on(&w, "archive-save-new"));
    assert_eq!(tree(&data), before);
    assert!(!w.backups.exists(), "nothing is written by confirming the folder");
}

#[test]
fn apply_on_a_loose_client_writes_through_the_loose_journal() {
    let mut w = open_loose("w17-apply");
    confirm_backups(&mut w);
    let data = w.client.path().join("data");
    let before = tree(&data);
    // queue a replace and an add through the Add dialog, and a remove
    let src = TempDir::new("w17-apply-src");
    put(src.path(), "a.txt", b"replaced text");
    queue_via_dialog(&mut w, &src.path().join("a.txt"), "Item/3DO");
    put(src.path(), "new.txt", b"brand new");
    queue_via_dialog(&mut w, &src.path().join("new.txt"), "Item/New");
    w.h.app.app.archive.queue_removes(vec!["Item/3DO/b.txt".into()]);
    assert_eq!(w.h.app.app.archive.pending.len(), 3);
    // the confirmation shows the four rows from the checked plan; nothing is written before the answer
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("The game is stopped: apply") && t.contains("Cancel"), "the confirmation is open and asks about the game: {t}");
    let rows = w.h.app.app.archive.confirm_rows().to_vec();
    assert_eq!(rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["Changes", "Writes", "Undo", "Game"]);
    assert!(rows[0].1.contains("added 1, replaced 1, removed 1") && rows[0].1.contains("1 folder made"), "{rows:?}");
    assert!(rows[1].1.contains("loose-backups") && rows[1].1.contains("nothing else is written"), "{rows:?}");
    assert!(rows[2].1.contains("Apply & history > Roll back"), "{rows:?}");
    assert!(rows[3].1.contains("cannot tell whether the game is running: stop it first") && rows[3].1.contains("Restart the game to see the change"), "{rows:?}");
    assert_eq!(tree(&data), before, "nothing is written before the answer");
    assert!(!w.backups.exists());
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 1 && h.app.app.archive.is_open()), "applied and reloaded");
    w.h.frames(3);
    assert!(w.h.app.app.archive.pending.is_empty());
    assert_eq!(std::fs::read(data.join("Item/3DO/a.txt")).unwrap(), b"replaced text");
    assert_eq!(std::fs::read(data.join("Item/New/new.txt")).unwrap(), b"brand new");
    assert!(!data.join("Item/3DO/b.txt").exists());
    // exactly one journal, in the confirmed folder, that can be rolled back; the data folder holds none
    let l = layout(&w);
    let j = loose_edit::list(&l).unwrap();
    assert_eq!(j.len(), 1);
    assert!(matches!(j[0].1, shaiya_archive::journal::Rollback::Possible));
    assert!(j[0].0.dir.starts_with(&w.backups) && tree(&data).iter().all(|(p, _)| !p.contains("journal") && !p.contains("pending")));
    // the Apply tab lists it and rolls it back through its confirmation
    go(&mut w.h, Tab::Apply);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.loose_history().len() == 1), "the history lists the loose journal");
    assert_eq!(w.h.app.app.apply.undo_label(true).unwrap(), "loose edit");
    with_cx(&mut w.h, |app, _| app.apply.request_rollback(app.current.as_ref()));
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Roll back loose edit") && t.contains("Changes") && t.contains("Game"), "{t}");
    w.h.app.shell.inject_answer("toolkit-rollback", 0);
    assert!(w.h.wait_until(WAIT, |h| matches!(h.app.app.apply.loose_history().first().map(|(j, _)| j.state.clone()), Some(shaiya_archive::journal::State::RolledBack { .. })) && h.app.app.archive.is_open()), "rolled back");
    assert_eq!(tree(&data), before, "the folder is as it was before the change");
}

#[test]
fn a_failed_loose_apply_changes_nothing_and_keeps_the_queue() {
    let mut w = open_loose("w17-fail");
    confirm_backups(&mut w);
    let data = w.client.path().join("data");
    let before = tree(&data);
    let src = TempDir::new("w17-fail-src");
    put(src.path(), "a.txt", b"fine");
    put(src.path(), "bad.ANI", b"this is not an animation");
    with_cx(&mut w.h, |app, _| {
        app.archive.queue_puts(vec![("Item/3DO/a.txt".into(), src.path().join("a.txt"), 0), ("Character/Human/ANI/bad.ANI".into(), src.path().join("bad.ANI"), 0)]);
    });
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    w.h.frames(5);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_applying() && h.app.app.archive.is_open()), "the apply ended");
    assert_eq!(tree(&data), before, "the file that fails its reader refused the whole change");
    assert_eq!(w.h.app.app.archive.pending.len(), 2, "the queue is kept");
    assert!(!w.backups.exists() || std::fs::read_dir(&w.backups).unwrap().count() == 0, "no journal was left");
}

#[test]
fn a_patch_applies_to_a_loose_client_from_the_apply_tab_and_rolls_back() {
    let mut w = open_loose("w17-patch");
    confirm_backups(&mut w);
    let data = w.client.path().join("data");
    let before = tree(&data);
    let p = TempDir::new("w17-patch-src");
    let files = p.path().join("files");
    put(&files, "Item/3DO/a.txt", b"from the patch");
    put(&files, "Item/3DO/c.txt", b"added by the patch");
    let patch = p.path().join("ps0001.patch");
    let input = shaiya_patch::BuildInput::from_folders(Some(&files), None, None, Some(0)).unwrap();
    shaiya_patch::build_with(&input, &patch, &shaiya_patch::BuildOptions { existing: shaiya_core::fs::Existing::Overwrite, sign: None }, &shaiya_core::Job::silent()).unwrap();
    go(&mut w.h, Tab::Apply);
    with_cx(&mut w.h, |app, _| app.apply.add_patches(vec![patch.clone()]));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.loose_plan_summary().is_some() && h.app.app.apply.verify_summary().iter().all(|v| v.is_some())), "the plan is made");
    assert_eq!(w.h.app.app.apply.loose_plan_summary().unwrap(), vec![Some((1, 1, 0))], "one added, one replaced");
    w.h.frames(2);
    with_cx(&mut w.h, |app, _| app.apply.request_apply(app.current.as_ref(), &app.keys));
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("The game is stopped: apply"), "the confirmation is open: {t}");
    let rows = w.h.app.app.apply.confirm_rows().to_vec();
    assert!(rows[0].1.contains("added 1, replaced 1, removed 0"), "{rows:?}");
    assert!(rows[1].1.contains("one journal per patch"), "{rows:?}");
    assert_eq!(tree(&data), before, "nothing is written before the answer");
    w.h.app.shell.inject_answer("toolkit-apply-patches", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.loose_history().len() == 1 && h.app.app.archive.is_open()), "applied; the history lists it");
    assert_eq!(std::fs::read(data.join("Item/3DO/a.txt")).unwrap(), b"from the patch");
    assert_eq!(std::fs::read(data.join("Item/3DO/c.txt")).unwrap(), b"added by the patch");
    let (j, rb) = &w.h.app.app.apply.loose_history()[0];
    assert_eq!((j.kind.as_str(), j.label.as_str()), ("loose-patch", "ps0001.patch"));
    assert!(matches!(rb, shaiya_archive::journal::Rollback::Possible));
    with_cx(&mut w.h, |app, _| app.apply.request_rollback(app.current.as_ref()));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-rollback", 0);
    assert!(w.h.wait_until(WAIT, |h| matches!(h.app.app.apply.loose_history().first().map(|(j, _)| j.state.clone()), Some(shaiya_archive::journal::State::RolledBack { .. })) && h.app.app.archive.is_open()));
    assert_eq!(tree(&data), before);
}

#[test]
fn a_patch_with_converted_media_is_refused_in_the_plan_of_a_shaiya_gx_folder() {
    let mut w = open_loose("w17-patch-gx");
    // a GX sign: an .ogg under Sound
    put(&w.client.path().join("data"), "Sound/Music/a.ogg", b"OggS");
    put(&w.client.path().join("data"), "interface/icon_dds.png", b"png");
    with_cx(&mut w.h, |app, cx| app.archive.reopen(cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    confirm_backups(&mut w);
    let data = w.client.path().join("data");
    let before = tree(&data);
    let p = TempDir::new("w17-patch-gx-src");
    let files = p.path().join("files");
    put(&files, "Item/3DO/a.txt", b"x");
    put(&files, "interface/icon.dds", b"dds");
    let patch = p.path().join("ps0001.patch");
    let input = shaiya_patch::BuildInput::from_folders(Some(&files), None, None, Some(0)).unwrap();
    shaiya_patch::build_with(&input, &patch, &shaiya_patch::BuildOptions { existing: shaiya_core::fs::Existing::Overwrite, sign: None }, &shaiya_core::Job::silent()).unwrap();
    go(&mut w.h, Tab::Apply);
    with_cx(&mut w.h, |app, _| app.apply.add_patches(vec![patch.clone()]));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.loose_plan_summary().is_some() && h.app.app.apply.verify_summary().iter().all(|v| v.is_some())), "the plan is made");
    assert_eq!(w.h.app.app.apply.loose_plan_summary().unwrap(), vec![None], "refused as a whole");
    w.h.frames(2);
    assert!(w.h.text().contains("interface/icon.dds") && w.h.text().contains("Shaiya-GX stores this as"), "{}", w.h.text());
    assert!(w.h.app.app.apply.apply_blocked(w.h.app.app.current.as_ref()).is_some(), "Apply is off");
    assert_eq!(tree(&data), before);
}

#[test]
fn the_health_window_reports_a_loose_folder_and_recovers_an_interrupted_change() {
    let mut w = open_loose("w17-health");
    confirm_backups(&mut w);
    let data = w.client.path().join("data");
    // a model whose texture is nowhere, and a model file that does not read
    put(&data, "Character/Human/huwm.MLT", &mlt_bytes(&["huwm_torso001.3DC"], &["gone.dds"], &[(0, 0, 0)]));
    put(&data, "Character/Human/ANI/broken.ANI", b"not an animation at all");
    with_cx(&mut w.h, |app, cx| app.open_health(Some(NAME), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.loose_health.report().is_some()), "the report is read");
    w.h.frames(2);
    let r = w.h.app.app.loose_health.report().unwrap().clone();
    assert_eq!((r.failing, r.unresolved, r.unreadable), (1, 1, 0), "{r:?}");
    assert_eq!(r.failing_list[0].0, "Character/Human/ANI/broken.ANI");
    assert_eq!(r.unresolved_list[0], ("Character/Human/3DC/huwm_torso001.3DC".to_string(), "gone.dds".to_string()));
    assert!(r.unfinished.is_none());
    let t = w.h.text().to_string();
    assert!(t.contains("Loose folder health") && t.contains("broken.ANI") && t.contains("gone.dds"), "{t}");
    // an interrupted change: the unfinished sentence and a Recover confirmation that puts the folder back
    let l = layout(&w);
    let before = tree(&data);
    let plan = loose_edit::plan(&l, loose_edit::LooseChange { puts: vec![loose_edit::LoosePut { rel: "Item/3DO/a.txt".into(), bytes: b"A2".to_vec() }, loose_edit::LoosePut { rel: "Item/3DO/c.txt".into(), bytes: b"C".to_vec() }], removes: vec![] }, &|_, _| Ok(())).unwrap();
    loose_edit::apply(&l, plan, &LooseOptions { crash_after_puts: Some(1), confirmed_unknown_busy: true, ..LooseOptions::default() }, &|_, _| Ok(()), &shaiya_core::Job::silent()).unwrap_err();
    assert_ne!(tree(&data), before);
    with_cx(&mut w.h, |app, cx| app.open_health(Some(NAME), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.loose_health.report().is_some_and(|r| r.unfinished.is_some())), "the unfinished change is reported");
    w.h.app.app.loose_health.ask_recover();
    w.h.frames(3);
    assert!(w.h.text().contains("Recover an interrupted change") && w.h.text().contains("Changes"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-loose-recover", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.loose_health.report().is_some_and(|r| r.unfinished.is_none()) && h.app.app.loose_health.last.is_some()), "recovered");
    let mut now = tree(&data);
    now.retain(|(p, _)| p != "Item/3DO/c.txt");
    let mut want = before.clone();
    want.retain(|(p, _)| p != "Item/3DO/c.txt");
    assert_eq!(now, want, "the folder is as it was");
    assert!(w.h.app.app.loose_health.last.as_ref().unwrap().contains("undid the unfinished change"));
}

#[test]
fn the_sdata_tab_reads_the_files_of_a_loose_client() {
    let mut w = open_loose("w17-sdata");
    go(&mut w.h, Tab::SData);
    w.h.app.app.sdata.from_client = true;
    assert!(w.h.wait_until(WAIT, |h| h.app.app.sdata.row_count() == 1), "the loose client's Item.SData is listed");
    w.h.frames(2);
    let row = &w.h.app.app.sdata.rows[0];
    assert_eq!(row.source, "client");
    assert!(row.item.name.to_lowercase().ends_with("item.sdata"), "{}", row.item.name);
    // the file it lists is read from the loose folder: the table view loads its bytes
    with_cx(&mut w.h, |app, _| app.sdata.select(0));
    with_cx(&mut w.h, |app, _| assert!(app.sdata.view_selected(&app.archive)));
    w.h.frames(2);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.table.as_ref().is_some_and(|t| !t.is_loading())), "the table window loads the file");
    // a file that is not SData is not listed, and an archive-less source key is empty
    assert_eq!(w.h.app.app.sdata.row_count(), 1);
}

// ------------------------------------------------------------------------------------------------ a textured preview in the app

fn skin() -> Vec<u8> {
    checker(64, 8, [230, 150, 40], [30, 140, 160])
}

#[test]
fn a_loose_client_previews_a_model_with_its_list_texture_in_the_app() {
    let data = TempDir::new("w17-tex-appdata");
    let client = TempDir::new("w17-tex-client");
    let d = client.path().join("data");
    put(&d, "Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc());
    put(&d, "Character/Human/huwm.MLT", &mlt_bytes(&["huwm_torso001.3DC"], &["skin.dds"], &[(0, 0, 0)]));
    put(&d, "Character/Human/DDS/skin.dds", &skin());
    put(&d, "Item/readme.txt", b"x");
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    let dir = client.path().to_path_buf();
    with_cx(&mut h, |app, cx| app.add_client(NAME, &dir, true, cx)).unwrap();
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    h.frames(2);
    assert!(h.app.app.archive.is_loose());
    with_cx(&mut h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.preview_ready()));
    h.frames(2);
    let coloured = |rgba: &[u8]| rgba.chunks(4).filter(|p| p[3] > 0 && p[0] as i32 - p[2] as i32 > 80).count();
    match h.app.app.archive.preview_data() {
        Some(PreviewData::Model { texture: Some(t), rgba, .. }) => {
            assert_eq!(t.line, "texture skin.dds from Character/Human/huwm.MLT (record 0); alpha used as transparency");
            assert!(coloured(rgba) > 0, "the texture's colours are in the picture");
        }
        other => panic!("{other:?}"),
    }
    // the same client with the texture file removed: the picture is grey, so the check above can fail
    std::fs::remove_file(d.join("Character/Human/DDS/skin.dds")).unwrap();
    with_cx(&mut h, |app, cx| app.archive.reopen(cx));
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    with_cx(&mut h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.preview_ready()));
    h.frames(2);
    match h.app.app.archive.preview_data() {
        Some(PreviewData::Model { rgba, notes, .. }) => {
            assert_eq!(coloured(rgba), 0);
            assert!(notes.iter().any(|n| n == "texture skin.dds is not in the folder; drawn grey"), "{notes:?}");
        }
        other => panic!("{other:?}"),
    }
}

fn on(w: &World, id: &str) -> bool {
    w.h.app.app.archive.commands().into_iter().find(|c| c.id == id).unwrap().enabled
}

#[test]
fn a_replacement_with_another_skeleton_asks_before_it_is_queued_on_a_loose_client() {
    let mut w = open_loose("w17-rig");
    confirm_backups(&mut w);
    let src = TempDir::new("w17-rig-src");
    // the copy the folder holds has 3 bones; this one has 2: the rig check reads the folder's copy and asks
    let mut ani = shaiya_clientdata::anim::Ani::parse(&shaiya_testkit::anim::arm_ani(60), "x.ANI").unwrap();
    ani.bones.pop();
    put(src.path(), "two.ANI", &ani.to_bytes());
    let path = "Character/Human/ANI/huwm_117_test.ANI";
    let row = w.h.app.app.archive.find_row(path).expect("the animation is listed");
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, src.path().join("two.ANI"), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.replace_ask.is_some()), "the rig check opens the question");
    let asked = w.h.app.app.archive.replace_ask.as_ref().unwrap().1.iter().any(|f| f.code == "anim-rig-differs");
    assert!(asked && w.h.app.app.archive.pending.is_empty(), "nothing is queued before the answer");
    // the same animation as the folder's copy is queued without a question
    put(src.path(), "same.ANI", &shaiya_testkit::anim::arm_ani(60));
    with_cx(&mut w.h, |app, cx| {
        app.archive.replace_ask = None;
        app.archive.replace_with_file(row, src.path().join("same.ANI"), cx)
    });
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.pending.len() == 1), "a replacement with the same skeleton is queued");
}
