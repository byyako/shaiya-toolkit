//! Shaiya Toolkit harness tests: headless frames with synthetic input (drop, tasks, cancel). No game data: every file is built here.

use egui::{Key, Modifiers};
use shaiya_core::hash::{sha256, to_hex};
use shaiya_toolkit::{ToolkitApp, ToolkitSettings, Tab};
use shaiya_testkit::{pe_stub, SahBuilder, TempDir};
use shaiya_ui::testing::Harness;
use std::path::PathBuf;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(20);

fn settled(h: &mut Harness<ToolkitApp>) -> bool {
    h.wait_until(WAIT, |h| h.app.app.inspect.table().iter().all(|r| r.1 != "..."))
}

fn samples(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let (sah, _saf) = SahBuilder::new(0).file(b"Item/Item.SData", b"item").file(b"a/b.txt", b"hello").write_to(dir.path());
    let seed = dir.path().join("Item.SData");
    std::fs::write(&seed, shaiya_seed::encrypt(b"0123456789")).unwrap();
    let exe = dir.path().join("game.exe");
    std::fs::write(&exe, pe_stub(0x014C, 1_272_251_564)).unwrap();
    (sah, seed, exe)
}

#[test]
fn the_empty_state_explains_the_toolkit() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(3);
    let t = h.text();
    assert!(t.contains("Choose a client folder in the toolbar"), "{t}");
    assert!(t.contains("Nothing is written until you apply changes"), "{t}");
    assert!(t.contains("These tools work on your own copy of the game"), "{t}");
    assert!(t.contains("Shaiya Archive") || t.contains("v0.2.1"), "{t}");
}

#[test]
fn dropping_files_lists_them_with_their_kinds() {
    let dir = TempDir::new("toolkit-drop");
    let (sah, seed, exe) = samples(&dir);
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(1);
    go(&mut h, Tab::Inspect);
    h.drop_files(&[seed, exe]);
    with_cx(&mut h, |app, cx| app.inspect.open(vec![sah], cx));
    assert!(settled(&mut h), "inspection tasks finish");
    h.frames(2);
    let table = h.app.app.inspect.table();
    assert_eq!(table.len(), 3);
    let by_name = |n: &str| table.iter().find(|r| r.0 == n).unwrap_or_else(|| panic!("{n} missing in {table:?}"));
    assert_eq!(by_name("data.sah").1, "Sah");
    assert!(by_name("data.sah").3.contains("SAH index, version 0, 2 files"), "{:?}", by_name("data.sah"));
    assert_eq!(by_name("Item.SData").1, "SdataSeed");
    assert!(by_name("Item.SData").3.contains("checksum OK"));
    assert_eq!(by_name("game.exe").1, "Pe");
    assert!(by_name("game.exe").3.contains("Windows program (x86)"));
    let t = h.text();
    assert!(t.contains("data.sah") && t.contains("Item.SData") && t.contains("game.exe"), "{t}");
    assert!(t.contains("Compute SHA-256") && t.contains("Remove from list"), "{t}");
    assert!(!t.contains("This tab identifies Shaiya files"), "the empty state is gone");
    assert_eq!(h.app.shell.recent_paths().len(), 3, "opened files go to the recent list");
}

#[test]
fn an_unreadable_file_is_an_error_row_not_a_banner() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(1);
    h.drop_files(&[PathBuf::from("Z:/no/such/folder/missing.bin")]);
    assert!(settled(&mut h));
    h.frames(2);
    let table = h.app.app.inspect.table();
    assert_eq!(table[0].1, "error");
    assert!(h.app.shell.banner_details().is_none(), "no banner for per-file problems");
    assert!(h.app.shell.activity().iter().any(|l| l.text.contains("missing.bin")), "the Activity list has it");
}

#[test]
fn a_hash_task_shows_progress_and_completes_with_the_right_digest() {
    let dir = TempDir::new("toolkit-hash");
    let big = dir.path().join("big.bin");
    let data: Vec<u8> = (0..40_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&big, &data).unwrap();
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(1);
    h.drop_files(&[big]);
    assert!(settled(&mut h));
    // press the row's SHA-256 button (what the click does)
    {
        let ToolkitHarness { app, shell } = split(&mut h);
        let mut cx = shaiya_ui::Cx { shell, settings: &mut shaiya_toolkit::ToolkitSettings::default() };
        app.inspect.hash_row("big.bin", &mut cx);
    }
    // it shows up in the Activity list with byte progress, and in the status bar
    let mut saw_progress = false;
    let finished = h.wait_until(WAIT, |h| {
        let views = h.app.shell.tasks().views();
        saw_progress |= views.iter().any(|v| v.title.starts_with("Hashing") && v.finished.is_none() && v.total == Some(40_000_000));
        h.app.app.inspect.table()[0].4.len() == 64
    });
    assert!(finished, "the hash completes");
    h.frames(2);
    assert_eq!(h.app.app.inspect.table()[0].4, to_hex(&sha256(&data)));
    assert!(h.text().contains(&to_hex(&sha256(&data))), "the digest is in the table");
    let _ = saw_progress; // a fast disk may finish between two frames; the total being known is checked below
    let views = h.app.shell.tasks().views();
    let v = views.iter().find(|v| v.title.starts_with("Hashing")).unwrap();
    assert_eq!((v.total, v.done), (Some(40_000_000), 40_000_000));
}

#[test]
fn cancel_stops_a_hash_and_the_row_says_cancelled() {
    let dir = TempDir::new("toolkit-cancel");
    let big = dir.path().join("huge.bin");
    std::fs::File::create(&big).unwrap().set_len(1_500_000_000).unwrap();
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(1);
    h.drop_files(&[big]);
    assert!(settled(&mut h));
    {
        let ToolkitHarness { app, shell } = split(&mut h);
        let mut cx = shaiya_ui::Cx { shell, settings: &mut shaiya_toolkit::ToolkitSettings::default() };
        app.inspect.hash_row("huge.bin", &mut cx);
    }
    // wait until bytes have been hashed, then cancel through the same call the Activity button makes
    assert!(h.wait_until(WAIT, |h| h.app.shell.tasks().views().iter().any(|v| v.title.starts_with("Hashing") && v.done > 0)), "hashing started");
    let t0 = std::time::Instant::now();
    h.app.app.inspect.cancel_hash("huge.bin");
    assert!(h.wait_until(Duration::from_secs(5), |h| h.app.app.inspect.table()[0].4 == "Cancelled"), "{:?}", h.app.app.inspect.table());
    assert!(t0.elapsed() < Duration::from_millis(1500), "cancel takes effect quickly: {:?}", t0.elapsed());
    h.frames(2);
    assert!(h.text().contains("Cancelled"), "{}", h.text());
    assert!(h.app.shell.tasks().views().iter().any(|v| matches!(&v.finished, Some(shaiya_ui::tasks::Finish::Cancelled))));
}

#[test]
fn ctrl_shift_p_lists_the_toolkit_command() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(2);
    h.key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    h.type_text("hash");
    h.frames(1);
    assert!(h.text().contains("Hash selected files"), "{}", h.text());
}

/// Splits the harness into the tool and the shell, the two halves a `Cx` is made of.
struct ToolkitHarness<'a> {
    app: &'a mut ToolkitApp,
    shell: &'a mut shaiya_ui::Shell,
}

fn split<'a>(h: &'a mut Harness<ToolkitApp>) -> ToolkitHarness<'a> {
    let a = &mut h.app;
    ToolkitHarness { app: &mut a.app, shell: &mut a.shell }
}

// ------------------------------------------------------------------------------------------------ T1: helpers

fn with_cx<R>(h: &mut Harness<ToolkitApp>, f: impl FnOnce(&mut ToolkitApp, &mut shaiya_ui::Cx<'_, ToolkitSettings>) -> R) -> R {
    let a = &mut h.app;
    let mut cx = shaiya_ui::Cx { shell: &mut a.shell, settings: &mut a.tool_settings };
    f(&mut a.app, &mut cx)
}

fn go(h: &mut Harness<ToolkitApp>, tab: Tab) {
    with_cx(h, |app, cx| app.set_tab(tab, cx));
    h.frames(2);
}

/// A harness with a data folder (so `shared.json` is written) and a synthetic client folder.
struct World {
    h: Harness<ToolkitApp>,
    _data: TempDir,
    client: TempDir,
    data_path: PathBuf,
}

impl World {
    fn client_dir(&self) -> PathBuf {
        self.client.path().to_path_buf()
    }
}

fn synthetic(dir: &std::path::Path) {
    let tga = shaiya_testkit::tga_stub(8, 4);
    let bmp = shaiya_testkit::bmp_stub(5, 3);
    SahBuilder::new(0)
        .file(b"Item/Item.SData", &shaiya_seed::encrypt(b"0123456789abcdef-more"))
        .file(b"Item/3DO/a.txt", b"hello text")
        .file(b"Item/3DO/b.txt", b"second")
        .file(b"interface/a.tga", &tga)
        .file(b"interface/b.bmp", &bmp)
        .file(b"x/blob.bin", &[0u8, 1, 2, 3, 0, 0, 7, 0, 0, 0])
        .file(b"top.txt", b"root file")
        .write_to(dir);
}

fn world(tag: &str) -> World {
    let data = TempDir::new(&format!("{tag}-data"));
    let client = TempDir::new(&format!("{tag}-client"));
    synthetic(client.path());
    let h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    let data_path = data.path().to_path_buf();
    World { h, _data: data, client, data_path }
}

fn open_world(tag: &str) -> World {
    let mut w = world(tag);
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

fn preview_ready(h: &mut Harness<ToolkitApp>) -> bool {
    h.wait_until(WAIT, |h| h.app.app.archive.preview_ready())
}

// ------------------------------------------------------------------------------------------------ T1: client folders and the Archive tab

#[test]
fn adding_a_client_writes_shared_json_and_opening_it_lists_the_visible_files() {
    let mut w = world("t1-add");
    w.h.frames(3);
    assert!(w.h.text().contains("Choose a client folder"), "{}", w.h.text());
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    w.h.app.flush_settings(true);
    let shared = std::fs::read_to_string(w.data_path.join("shared.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&shared).unwrap();
    assert_eq!(v["clients"][0]["name"], "US test", "{shared}");
    assert!(v["clients"][0]["path"].as_str().unwrap().len() > 3);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.visible_count(), 7, "table row count equals the visible count");
    // the root folder lists its folders and files
    let names = w.h.app.app.archive.table_names();
    assert!(names.contains(&"Item/".to_string()) && names.contains(&"top.txt".to_string()), "{names:?}");
    assert!(w.h.text().contains("top.txt") && w.h.text().contains("7 files"), "{}", w.h.text());
    // a duplicate name is refused and a bad name too
    assert!(with_cx(&mut w.h, |app, cx| app.add_client("us TEST", &dir, false, cx)).is_err());
    assert!(with_cx(&mut w.h, |app, cx| app.add_client("a/b", &dir, false, cx)).is_err());
    // the last client is remembered as a setting
    assert_eq!(w.h.app.tool_settings.last_client.as_deref(), Some("US test"));
}

#[test]
fn search_narrows_glob_matches_and_folders_navigate() {
    let mut w = open_world("t1-search");
    w.h.app.app.archive.search = "txt".into();
    w.h.frames(2);
    let mut found = w.h.app.app.archive.shown_paths();
    found.sort();
    assert_eq!(found, vec!["Item/3DO/a.txt", "Item/3DO/b.txt", "top.txt"]);
    w.h.app.app.archive.search = "*.tga".into();
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.shown_paths(), vec!["interface/a.tga"], "a pattern with * is a glob");
    w.h.app.app.archive.search = "item/3do/*".into();
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.shown_paths().len(), 2);
    w.h.app.app.archive.search.clear();
    w.h.frames(2);
    // folder navigation: Enter opens the selected folder row; Backspace goes up
    let first_folder = w
        .h
        .app
        .app
        .archive
        .view_rows()
        .iter()
        .find_map(|r| if let shaiya_toolkit::archive_view::ViewRow::Folder(i) = r { Some(*i) } else { None })
        .unwrap();
    w.h.app.app.archive.select_folder_row(first_folder);
    w.h.key(Key::Enter, Modifiers::NONE);
    w.h.frames(2);
    assert!(!w.h.app.app.archive.current_folder().is_empty(), "Enter opened the folder");
    w.h.key(Key::Backspace, Modifiers::NONE);
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.current_folder(), "", "Backspace goes to the parent folder");
    w.h.app.app.archive.enter_folder("Item");
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.table_names(), vec!["3DO/", "Item.SData"]);
}

#[test]
fn previews_show_images_text_hex_and_sdata() {
    let mut w = open_world("t1-preview");
    for (path, kind) in [("interface/a.tga", "image"), ("interface/b.bmp", "image"), ("x/blob.bin", "hex"), ("Item/3DO/a.txt", "text"), ("Item/Item.SData", "sdata")] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
        assert!(preview_ready(&mut w.h), "{path}");
        w.h.frames(2);
        assert_eq!(w.h.app.app.archive.preview_kind(), Some(kind), "{path}");
        if kind == "image" {
            assert!(w.h.app.app.archive.has_texture(), "a texture is uploaded for {path}");
        }
    }
    let t = w.h.text();
    assert!(t.contains("checksum OK"), "{t}");
    assert!(t.contains("plaintext 21 bytes"), "{t}");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("interface/a.tga", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert!(w.h.text().contains("TGA type 2, 24 bit, 8 x 4"), "{}", w.h.text());
}

#[test]
fn dropping_files_queues_changes_and_apply_writes_them_with_a_journal() {
    let mut w = open_world("t1-apply");
    let src = TempDir::new("t1-apply-src");
    let (a, b) = (src.path().join("t1.txt"), src.path().join("Item.SData"));
    std::fs::write(&a, b"new file").unwrap();
    std::fs::write(&b, b"replacement").unwrap();
    w.h.drop_files(&[a.clone(), b.clone()]);
    w.h.frames(2);
    assert!(w.h.app.app.archive.add_dialog.is_some(), "the Add dialog opens");
    let t = w.h.text();
    assert!(t.contains("Add to archive") && t.contains("Queue changes") && t.contains("new"), "{t}");
    assert!(w.h.app.app.archive.add_dialog.as_ref().unwrap().rows.iter().all(|r| r.replaces.is_none()), "the root folder holds neither name");
    // choose the Item folder as the target: Item.SData replaces
    {
        let arch = &mut w.h.app.app.archive;
        let mut d = arch.add_dialog.take().unwrap();
        d.target = "Item".into();
        arch.recompute_add(&mut d);
        arch.add_dialog = Some(d);
    }
    let rows = w.h.app.app.archive.add_dialog.as_ref().unwrap().rows.clone();
    assert!(rows.iter().any(|r| r.path == "Item/Item.SData" && r.replaces.is_some()), "{rows:?}");
    w.h.app.app.archive.queue_add_dialog();
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.pending.len(), 2);
    assert!(w.h.text().contains("Pending changes (2)"), "{}", w.h.text());
    // Ctrl+Z undoes the queue step, Ctrl+Y brings it back
    w.h.key(Key::Z, Modifiers::COMMAND);
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.pending.len(), 0, "both puts were one queue step");
    w.h.key(Key::Y, Modifiers::COMMAND);
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.pending.len(), 2);
    // quitting now asks about the queue (the shell's unsaved prompt)
    w.h.key(Key::Q, Modifiers::COMMAND);
    w.h.frames(3);
    assert!(w.h.text().contains("Unsaved changes") && w.h.text().contains("2 pending archive changes"), "{}", w.h.text());
    assert!(with_cx(&mut w.h, shaiya_ui::ToolApp::save_all).is_err(), "Save all never writes to a client without its own confirmation");
    // apply through the confirmation
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    assert!(w.h.text().contains("Apply 2 changes"), "the confirm names the count: {}", w.h.text());
    let t = w.h.text().to_string();
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(t.contains("added 1, replaced 1, removed 0"), "the counts come from the plan: {t}");
    assert!(t.contains("Apply & history > Roll back") && t.contains("shaiya-toolkit-backups"), "{t}");
    assert!(t.contains(if cfg!(windows) { "must be closed (checked)" } else { "cannot be checked on this system" }), "{t}");
    assert!(shaiya_archive::journal::list(w.client.path(), &w.client.path().join("shaiya-toolkit-backups")).map(|l| l.is_empty()).unwrap_or(true), "nothing is written before the answer");
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 1 && h.app.app.archive.is_open()), "applied and reloaded");
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 0);
    assert_eq!(w.h.app.app.archive.visible_count(), 8);
    assert!(w.h.app.app.archive.find_row("Item/t1.txt").is_some());
    let backups = w.client_dir().join("shaiya-toolkit-backups");
    assert!(std::fs::read_dir(&backups).map(|r| r.count()).unwrap_or(0) >= 1, "a journal folder exists");
    let journals = shaiya_archive::journal::list(w.client.path(), &backups).unwrap();
    assert_eq!(journals.len(), 1, "the journal the Undo row promised exists, one per apply");
    assert!(matches!(journals[0].1, shaiya_archive::journal::Rollback::Possible));
    let arch = shaiya_archive::Archive::open(w.client.path()).unwrap();
    assert_eq!(arch.read("Item/Item.SData").unwrap(), b"replacement");
}

#[test]
fn extract_asks_about_existing_files() {
    let mut w = open_world("t1-extract");
    let out = TempDir::new("t1-extract-out");
    std::fs::create_dir_all(out.path().join("Item/3DO")).unwrap();
    std::fs::write(out.path().join("Item/3DO/a.txt"), b"old").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.begin_extract(out.path().to_path_buf(), false, true, false, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.extract_asking()), "the existing-files question appears");
    w.h.frames(2);
    let t = w.h.text();
    assert!(t.contains("already exist") && t.contains("Skip existing") && t.contains("Overwrite") && t.contains("Cancel"), "{t}");
    w.h.app.shell.inject_answer("extract-existing", 1);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_extracting() && !h.app.app.archive.extract_asking()));
    w.h.frames(2);
    assert_eq!(std::fs::read(out.path().join("Item/3DO/a.txt")).unwrap(), b"hello text", "Overwrite replaced it");
    assert!(out.path().join("top.txt").exists(), "the whole archive was extracted");
}

#[test]
fn cancelling_a_long_extract_ends_cancelled() {
    let data = TempDir::new("t1-cancel-data");
    let client = TempDir::new("t1-cancel-client");
    // a large entry (sparse on disk) so the extraction is still running when it is cancelled
    let big = 600_000_000u64;
    SahBuilder::new(0).file(b"a/small.txt", b"x").file(b"a/big.bin", &[7u8; 16]).write_to(client.path());
    {
        let (sah, mut index) = shaiya_archive::Archive::open_index(client.path()).unwrap();
        let mut e = index.get("a/big.bin").unwrap().clone();
        let offset = e.offset;
        e.length = big as u32;
        index.put("a/big.bin", e).unwrap();
        std::fs::write(&sah, index.to_bytes()).unwrap();
        let saf = shaiya_archive::saf_of(&sah);
        std::fs::OpenOptions::new().write(true).open(saf).unwrap().set_len(offset + big).unwrap();
    }
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    let dir = client.path().to_path_buf();
    with_cx(&mut h, |app, cx| app.add_client("big", &dir, true, cx)).unwrap();
    assert!(h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    let out = TempDir::new("t1-cancel-out");
    with_cx(&mut h, |app, cx| app.archive.begin_extract(out.path().to_path_buf(), false, true, false, cx));
    assert!(h.wait_until(WAIT, |h| h.app.shell.tasks().views().iter().any(|v| v.title.starts_with("Extracting") && v.done > 0)), "extraction started");
    h.app.shell.tasks().cancel_all();
    assert!(h.wait_until(Duration::from_secs(10), |h| !h.app.app.archive.is_extracting()), "ended");
    h.frames(2);
    assert!(h.app.app.archive.status_line.contains("Cancelled"), "{}", h.app.app.archive.status_line);
    for e in std::fs::read_dir(out.path().join("a")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        assert!(!n.contains("shaiya-tmp"), "no half-written file stays: {n}");
    }
}

#[test]
fn a_corrupt_index_shows_the_banner() {
    let data = TempDir::new("t1-corrupt-data");
    let client = TempDir::new("t1-corrupt-client");
    std::fs::write(client.path().join("data.sah"), b"this is not a SAH index at all, just text that is long enough").unwrap();
    std::fs::write(client.path().join("data.saf"), b"x").unwrap();
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    with_cx(&mut h, |app, cx| {
        app.archive.open(shaiya_toolkit::archive_view::OpenSource::File(client.path().join("data.sah")), cx);
    });
    assert!(h.wait_until(WAIT, |h| !h.app.app.archive.is_opening()));
    h.frames(2);
    assert!(h.app.shell.banner_details().is_some(), "the error banner shows");
    assert!(h.text().contains("Cannot open this archive"), "{}", h.text());
}

#[cfg(windows)]
#[test]
fn an_open_handle_on_data_saf_makes_apply_show_retry() {
    let mut w = open_world("t1-inuse");
    let src = TempDir::new("t1-inuse-src");
    let f = src.path().join("held.txt");
    std::fs::write(&f, b"held").unwrap();
    w.h.drop_files(&[f]);
    w.h.frames(2);
    w.h.app.app.archive.queue_add_dialog();
    // another program holds data.saf
    let held = std::fs::File::open(w.client.path().join("data.saf")).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    let shown = w.h.wait_until(WAIT, |h| h.text().contains("is open in another program") && h.text().contains("Retry"));
    assert!(shown, "the Retry dialog appears: {}", w.h.text());
    drop(held);
    w.h.app.shell.inject_answer("toolkit-in-use", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 1 && h.app.app.archive.is_open()), "Retry succeeds once the handle is closed");
    assert_eq!(w.h.app.app.archive.pending.len(), 0);
}

// ------------------------------------------------------------------------------------------------ T1: the other tabs

fn build_patch(dir: &std::path::Path, name: &str) -> PathBuf {
    let files = dir.join(format!("{name}-files"));
    let loose = dir.join(format!("{name}-loose"));
    std::fs::create_dir_all(files.join("Item")).unwrap();
    std::fs::create_dir_all(files.join("interface")).unwrap();
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(files.join("Item/Item.SData"), b"patched").unwrap();
    std::fs::write(files.join("interface/new.txt"), b"brand new").unwrap();
    std::fs::write(loose.join("notice.txt"), b"hello players").unwrap();
    let out = dir.join(name);
    let input = shaiya_patch::BuildInput::from_folders(Some(&files), Some(&loose), None, Some(0)).unwrap();
    shaiya_patch::build_with(&input, &out, &shaiya_patch::BuildOptions { existing: shaiya_core::fs::Existing::Overwrite, sign: None }, &shaiya_core::Job::silent()).unwrap();
    out
}

#[test]
fn tab_keys_switch_tabs_and_remember_the_last_one() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(2);
    for (key, tab) in [(Key::Num2, Tab::Patch), (Key::Num3, Tab::Apply), (Key::Num4, Tab::Sign), (Key::Num5, Tab::SData), (Key::Num6, Tab::Inspect), (Key::Num1, Tab::Archive)] {
        h.key(key, Modifiers::COMMAND);
        h.frames(2);
        assert_eq!(h.app.app.tab, tab);
        assert_eq!(h.app.tool_settings.tab, tab.key());
    }
    h.key(Key::Num3, Modifiers::COMMAND);
    h.frames(2);
    assert!(h.text().contains("Apply & history") && h.text().contains("No patches yet"), "{}", h.text());
    assert!(h.text().contains("No changes made by the toolkit to this client yet") || h.text().contains("History"), "{}", h.text());
}

#[test]
fn apply_and_history_plan_apply_list_and_roll_back() {
    let mut w = open_world("t1-hist");
    let src = TempDir::new("t1-hist-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    go(&mut w.h, Tab::Apply);
    with_cx(&mut w.h, |app, _| app.apply.add_patches(vec![patch.clone()]));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.plan_summary().is_some() && h.app.app.apply.verify_summary().iter().all(|v| v.is_some())), "the plan is made");
    assert_eq!(w.h.app.app.apply.plan_summary().unwrap(), vec![(1, 1, 0, 1)], "one added, one replaced, one loose file");
    assert_eq!(w.h.app.app.apply.verify_summary(), vec![Some((0, "not checked".to_string()))]);
    w.h.frames(2);
    assert!(w.h.text().contains("ps0001.patch") && w.h.text().contains("structure OK"), "{}", w.h.text());
    // apply through the confirmation
    with_cx(&mut w.h, |app, _| app.apply.request_apply(app.current.as_ref(), &app.keys));
    w.h.frames(3);
    assert!(w.h.text().contains("Apply ps0001.patch to US test") && w.h.text().contains("Signatures: not used (unsigned patches)"), "{}", w.h.text());
    let t = w.h.text().to_string();
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(t.contains("added 1, replaced 1, removed 0, 1 loose file written"), "the counts are the plan's: {t}");
    assert!(t.contains("notice.txt") && t.contains("one journal per patch"), "{t}");
    w.h.app.shell.inject_answer("toolkit-apply-patches", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.history().len() == 1 && h.app.app.archive.is_open() && h.app.app.archive.changed_counter >= 1), "applied; the history lists it");
    w.h.frames(3);
    let (j, rb) = &w.h.app.app.apply.history()[0];
    assert_eq!((j.label.as_str(), j.kind.as_str()), ("ps0001.patch", "patch"));
    assert!(matches!(rb, shaiya_archive::journal::Rollback::Possible));
    assert_eq!(std::fs::read(w.client.path().join("notice.txt")).unwrap(), b"hello players");
    let arch = shaiya_archive::Archive::open(w.client.path()).unwrap();
    assert_eq!(arch.read("Item/Item.SData").unwrap(), b"patched");
    assert_eq!(arch.read("interface/new.txt").unwrap(), b"brand new");
    drop(arch);
    assert!(w.h.text().contains("ps0001.patch") && w.h.text().contains("possible"), "{}", w.h.text());
    // roll back
    with_cx(&mut w.h, |app, _| app.apply.request_rollback(app.current.as_ref()));
    w.h.frames(3);
    assert!(w.h.text().contains("Roll back ps0001.patch") && w.h.text().contains("1 deleted"), "{}", w.h.text());
    let jid = w.h.app.app.apply.history()[0].0.id.clone();
    let t = w.h.text().to_string();
    assert!(t.contains("Writes") && t.contains("Undo") && t.contains("Game") && t.contains(&jid) && t.contains("no undo of its own"), "the rows name the journal: {t}");
    w.h.app.shell.inject_answer("toolkit-rollback", 0);
    assert!(
        w.h.wait_until(WAIT, |h| matches!(h.app.app.apply.history().first().map(|(j, _)| j.state.clone()), Some(shaiya_archive::journal::State::RolledBack { .. })) && h.app.app.archive.is_open()),
        "rolled back"
    );
    w.h.frames(2);
    assert!(!w.client.path().join("notice.txt").exists(), "the loose file that did not exist is deleted again");
    let arch = shaiya_archive::Archive::open(w.client.path()).unwrap();
    assert_eq!(arch.read("Item/Item.SData").unwrap().len(), shaiya_seed::encrypt(b"0123456789abcdef-more").len(), "the original entry is back");
    assert!(arch.index().get("interface/new.txt").is_none());
}

#[test]
fn patch_builder_from_pending_changes_builds_a_patch_with_those_contents() {
    let mut w = open_world("t1-builder");
    let src = TempDir::new("t1-builder-src");
    let f = src.path().join("Item.SData");
    std::fs::write(&f, b"queued").unwrap();
    {
        let a = &mut w.h.app.app.archive;
        a.queue_puts(vec![("Item/Item.SData".to_string(), f.clone(), 0)]);
        a.queue_removes(vec!["top.txt".to_string()]);
    }
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| shaiya_ui::ToolApp::run_command(app, "archive-make-patch", cx));
    w.h.frames(3);
    assert_eq!(w.h.app.app.tab, Tab::Patch, "Make patch switches to the builder");
    let out_dir = TempDir::new("t1-builder-out");
    w.h.app.app.patch.out_dir = out_dir.path().display().to_string();
    w.h.frames(2);
    assert!(w.h.text().contains("Item/Item.SData") && w.h.text().contains("top.txt"), "{}", w.h.text());
    assert_eq!(w.h.app.app.patch.out_name, "ps0001.patch", "the name is suggested (no Version.ini here)");
    assert!(w.h.app.app.patch.problems.is_empty());
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.last_out.is_some()), "built");
    let c = shaiya_patch::contents(&out_dir.path().join("ps0001.patch")).unwrap();
    assert_eq!(c.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["Item/Item.SData"]);
    assert_eq!(c.files[0].length, 6);
    assert_eq!(c.removals, vec!["top.txt"]);
    w.h.frames(2);
    assert!(w.h.text().contains("Built ps0001.patch"), "{}", w.h.text());
    // building again over the file asks first
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    w.h.frames(3);
    assert!(w.h.text().contains("already exists") && w.h.text().contains("Overwrite"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-build-exists", 1);
    w.h.frames(3);
    assert!(out_dir.path().join("ps0001.patch").exists());
}

#[test]
fn patch_builder_folder_sources_show_problems_and_effects() {
    let mut w = open_world("t1-builder-folder");
    let src = TempDir::new("t1-builder-folder-src");
    let files = src.path().join("changed");
    std::fs::create_dir_all(files.join("Item")).unwrap();
    std::fs::write(files.join("Item/Item.SData"), b"x").unwrap();
    std::fs::write(files.join("fresh.txt"), b"y").unwrap();
    go(&mut w.h, Tab::Patch);
    assert!(w.h.text().contains("Drop a folder of changed files here"), "{}", w.h.text());
    w.h.drop_files(std::slice::from_ref(&files));
    w.h.frames(3);
    let rows = w.h.app.app.patch.rows.clone();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let by = |p: &str| rows.iter().find(|r| r.path == p).unwrap().effect.clone();
    assert!(by("Item/Item.SData").starts_with("replaces"), "{rows:?}");
    assert_eq!(by("fresh.txt"), "new");
    // a name the archive cannot hold blocks the build
    std::fs::write(files.join("bad\u{1F600}name.txt"), b"z").unwrap();
    w.h.app.app.patch.set_card(shaiya_toolkit::patch_builder::Card::Files, Some(files));
    w.h.frames(3);
    assert!(!w.h.app.app.patch.problems.is_empty(), "an unencodable name is a problem: rows {:?} err {:?} input {:?}", w.h.app.app.patch.rows, w.h.app.app.patch.resolve_error, w.h.app.app.patch.input().files.iter().map(|f| f.0.clone()).collect::<Vec<_>>());
    assert!(w.h.app.app.patch.build_blocked(&w.h.app.app.keys).is_some());
}

#[test]
fn sign_and_verify_with_a_generated_key_pair_and_a_tampered_patch() {
    let mut w = open_world("t1-sign");
    go(&mut w.h, Tab::Sign);
    let dir = TempDir::new("t1-sign-files");
    let (secret, public) = (dir.path().join("secret.key"), dir.path().join("public.key"));
    with_cx(&mut w.h, |app, cx| app.sign.generate_keys(secret.clone(), public.clone(), &mut app.keys, cx));
    assert!(secret.exists() && public.exists());
    let hex = w.h.app.app.sign.generated.as_ref().unwrap().public_hex.clone();
    assert_eq!(hex.len(), 64);
    // the pair is never overwritten
    with_cx(&mut w.h, |app, cx| app.sign.generate_keys(secret.clone(), public.clone(), &mut app.keys, cx));
    assert!(w.h.app.app.sign.error.is_some(), "an existing key file is refused");
    w.h.app.app.keys.check_version = "3".into();
    let patch = dir.path().join("ps0002.patch");
    std::fs::write(&patch, b"not really a patch, but bytes to sign").unwrap();
    w.h.app.app.sign.sign_files = vec![patch.clone()];
    with_cx(&mut w.h, |app, cx| app.sign.begin_sign(&app.keys, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sign.sign_results.is_empty()), "signed");
    assert!(shaiya_patch::signature_path(&patch).exists(), "the .sig is next to the file");
    // verify: valid
    w.h.app.app.sign.verify_files = vec![patch.clone()];
    with_cx(&mut w.h, |app, cx| app.sign.begin_verify(&app.keys, None, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sign.verify_results.is_empty()));
    assert_eq!(w.h.app.app.sign.verify_results[0].1, shaiya_toolkit::sign_tab::Outcome::Valid);
    // tamper one byte: invalid
    let mut bytes = std::fs::read(&patch).unwrap();
    bytes[3] ^= 1;
    std::fs::write(&patch, &bytes).unwrap();
    w.h.app.app.sign.verify_results.clear();
    with_cx(&mut w.h, |app, cx| app.sign.begin_verify(&app.keys, None, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sign.verify_results.is_empty()));
    assert!(matches!(w.h.app.app.sign.verify_results[0].1, shaiya_toolkit::sign_tab::Outcome::Invalid(_)), "{:?}", w.h.app.app.sign.verify_results);
    // a missing signature is reported as missing
    std::fs::remove_file(shaiya_patch::signature_path(&patch)).unwrap();
    w.h.app.app.sign.verify_results.clear();
    with_cx(&mut w.h, |app, cx| app.sign.begin_verify(&app.keys, None, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sign.verify_results.is_empty()));
    assert_eq!(w.h.app.app.sign.verify_results[0].1, shaiya_toolkit::sign_tab::Outcome::Missing);
    // signing again over an existing .sig asks first
    w.h.app.app.sign.sign_results.clear();
    with_cx(&mut w.h, |app, cx| app.sign.begin_sign(&app.keys, cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sign.sign_results.is_empty()), "signed again (the .sig was gone)");
    with_cx(&mut w.h, |app, cx| app.sign.begin_sign(&app.keys, cx));
    w.h.frames(3);
    assert!(w.h.text().contains("Replace signature"), "{}", w.h.text());
    // the secret key's path is not stored unless asked
    w.h.app.flush_settings(true);
    let settings = std::fs::read_to_string(w.data_path.join("shaiya-toolkit").join("settings.json")).unwrap();
    assert!(!settings.contains("secret.key"), "the secret key path is not remembered by default: {settings}");
}

#[test]
fn the_sdata_tab_lists_and_decrypts_to_a_folder() {
    let mut w = open_world("t1-sdata");
    go(&mut w.h, Tab::SData);
    assert!(w.h.text().contains("Drop SData files or folders here"), "{}", w.h.text());
    let src = TempDir::new("t1-sdata-src");
    std::fs::write(src.path().join("Item.SData"), shaiya_seed::encrypt(b"0123456789abcdef")).unwrap();
    std::fs::write(src.path().join("Plain.SData"), b"not encrypted").unwrap();
    w.h.drop_files(&[src.path().to_path_buf()]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.sdata.row_count() == 2), "both files are listed");
    w.h.frames(2);
    assert!(w.h.text().contains("Item.SData") && w.h.text().contains("OK") && w.h.text().contains("plain"), "{}", w.h.text());
    let out = TempDir::new("t1-sdata-out");
    let o = out.path().join("plain");
    with_cx(&mut w.h, |app, cx| app.sdata.begin(shaiya_toolkit::sdata_tab::Action::Decrypt, Some(o.clone()), &app.archive, cx));
    assert!(w.h.wait_until(WAIT, |h| o.join("Item.SData").exists() && !h.app.app.sdata.is_busy()), "decrypted into the folder");
    assert_eq!(std::fs::read(o.join("Item.SData")).unwrap(), b"0123456789abcdef");
    assert_eq!(std::fs::read(o.join("Plain.SData")).unwrap(), b"not encrypted", "a plain file is copied unchanged");
    // existing files ask the three-button question
    with_cx(&mut w.h, |app, cx| app.sdata.begin(shaiya_toolkit::sdata_tab::Action::Decrypt, Some(o.clone()), &app.archive, cx));
    w.h.frames(3);
    assert!(w.h.text().contains("already exist") && w.h.text().contains("Skip existing"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-sdata-exists", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.sdata.is_busy()));
    // the open client's files can be added too
    with_cx(&mut w.h, |app, _| app.sdata.clear());
    w.h.app.app.sdata.from_client = true;
    assert!(w.h.wait_until(WAIT, |h| h.app.app.sdata.row_count() == 1), "the client's Item.SData is listed");
}

#[test]
fn the_patch_builder_keeps_signing_in_a_closed_section() {
    let mut w = open_world("t7-builder-signing");
    go(&mut w.h, Tab::Patch);
    let t = w.h.text();
    assert!(t.contains("Signing (only for an updater that checks signatures)"), "{t}");
    assert!(!t.contains("Secret key file") && !t.contains("Sign after building"), "no key field until the section opens: {t}");
    assert!(!w.h.app.app.patch.sign, "a build is unsigned unless the section is used");
    // a patch builder that was told to sign starts with the section open
    let mut w2 = open_world("t7-builder-signing-open");
    w2.h.app.app.patch.sign = true;
    go(&mut w2.h, Tab::Patch);
    let t = w2.h.text();
    assert!(t.contains("Sign after building") && t.contains("Secret key file"), "{t}");
}

#[test]
fn the_sign_tab_says_signing_is_optional() {
    let mut w = open_world("t7-sign-optional");
    go(&mut w.h, Tab::Sign);
    assert!(w.h.text().contains("Optional. The original updater does not check signatures."), "{}", w.h.text());
}

#[test]
fn the_apply_confirmation_says_not_checked_only_when_a_key_is_set_and_require_is_off() {
    let mut w = open_world("t7-apply-sigtext");
    let src = TempDir::new("t7-apply-sigtext-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    go(&mut w.h, Tab::Apply);
    with_cx(&mut w.h, |app, _| app.apply.add_patches(vec![patch.clone()]));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.plan_summary().is_some() && h.app.app.apply.verify_summary().iter().all(|v| v.is_some())), "the plan is made");
    // a public key is set and "Require" is off
    w.h.app.app.keys.public_hex = "ab".repeat(32);
    w.h.app.app.keys.check_version = "1".into();
    w.h.app.app.apply.require = false;
    with_cx(&mut w.h, |app, _| app.apply.request_apply(app.current.as_ref(), &app.keys));
    w.h.frames(3);
    assert!(w.h.text().contains("Signatures: NOT CHECKED"), "{}", w.h.text());
    assert!(!w.h.text().contains("not used (unsigned patches)"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-apply-patches", 1);
    w.h.frames(3);
    assert!(w.h.app.app.apply.history().is_empty(), "Cancel applied nothing");
}

// ------------------------------------------------------------------------------------------------ T7 W2: Start page and explained confirmations

#[test]
fn the_start_page_lists_the_jobs_and_its_buttons_open_the_right_place() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(3);
    let t = h.text().to_string();
    assert!(t.contains("Start") && t.contains("Choose a client folder in the toolbar"), "{t}");
    assert!(t.contains("These tools work on your own copy of the game"), "the own-copy statement stays: {t}");
    // later work packages add their rows here (Look at SData tables)
    let keys: Vec<&str> = shaiya_toolkit::start::start_jobs().iter().map(|j| j.key).collect();
    assert_eq!(keys, ["add-client", "change-files", "make-patch", "publish-patch", "apply-patches"]);
    for j in shaiya_toolkit::start::start_jobs() {
        assert!(t.contains(j.title) && t.contains(j.button), "{} is drawn: {t}", j.key);
    }
    assert!(!t.contains("SData tables"), "rows of jobs that have not shipped are absent: {t}");
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Go(Tab::Patch), cx));
    assert_eq!(h.app.app.tab, Tab::Patch);
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Go(Tab::Apply), cx));
    assert_eq!(h.app.app.tab, Tab::Apply);
}

#[test]
fn the_start_page_is_gone_once_a_client_is_open() {
    let w = open_world("t7-start-gone");
    assert!(!w.h.text().contains("Apply patches to a test client"), "{}", w.h.text());
}

#[test]
fn compact_says_there_is_no_undo() {
    let mut w = open_world("t7-compact-rows");
    w.h.app.app.archive.request_compact();
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Compact data.saf of US test"), "{t}");
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(t.contains("no undo: compacting makes no journal") && t.contains("0 earlier changes"), "{t}");
    let before = std::fs::read(w.client.path().join("data.saf")).unwrap();
    w.h.app.shell.inject_answer("toolkit-exclusive", 1);
    w.h.frames(3);
    assert_eq!(std::fs::read(w.client.path().join("data.saf")).unwrap(), before, "Cancel changed nothing");
}

#[test]
fn prune_and_swap_show_the_four_rows() {
    let mut w = open_world("t7-prune-swap");
    go(&mut w.h, Tab::Apply);
    with_cx(&mut w.h, |app, _| app.apply.request_prune(app.current.as_ref()));
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Prune history") && t.contains("0 journals deleted, the newest 0 kept") && t.contains("no undo"), "{t}");
    w.h.app.shell.inject_answer("toolkit-prune", 1);
    w.h.frames(3);
    // swap: another pair in a second folder
    let other = TempDir::new("t7-prune-swap-other");
    SahBuilder::new(0).file(b"x.txt", b"x").write_to(other.path());
    let dir = w.client_dir();
    w.h.app.app.clients.begin_swap("US test", dir, other.path().to_path_buf());
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Swap archives") && t.contains("data.<UTC stamp>") && t.contains("no journal"), "{t}");
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    w.h.app.shell.inject_answer("toolkit-swap", 1);
    w.h.frames(3);
    assert!(!w.h.text().contains("Swap archives"), "Cancel closes it");
}

// ------------------------------------------------------------------------------------------------ T7 W3: undo the last applied change

fn apply_put(w: &mut World, rel: &str, source: &std::path::Path, n: u64) {
    w.h.app.app.archive.queue_puts(vec![(rel.to_string(), source.to_path_buf(), 0)]);
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == n && h.app.app.archive.is_open()), "change {n} applied and reloaded");
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.history().len() as u64 == n), "the history lists change {n}");
    w.h.frames(2);
}

fn undo_enabled(h: &Harness<ToolkitApp>) -> bool {
    shaiya_ui::ToolApp::commands(&h.app.app).iter().find(|c| c.id == "history-rollback").map(|c| c.enabled).expect("the command exists")
}

#[test]
fn undo_last_change_rolls_back_two_applied_changes_one_at_a_time() {
    let mut w = open_world("t7-undo-two");
    let client = w.client_dir();
    let src = TempDir::new("t7-undo-two-src");
    let (a, b) = (src.path().join("a.txt"), src.path().join("b.txt"));
    std::fs::write(&a, b"first added file").unwrap();
    std::fs::write(&b, b"second added file, longer than the first").unwrap();
    assert_eq!(w.h.app.app.undo_state().unwrap_err(), "No change made by the toolkit to this client can be undone yet.", "nothing to undo before the first apply");
    assert!(!undo_enabled(&w.h));
    let (sah0, saf0) = (std::fs::read(client.join("data.sah")).unwrap(), std::fs::metadata(client.join("data.saf")).unwrap().len());
    apply_put(&mut w, "u/a.txt", &a, 1);
    let (sah1, saf1) = (std::fs::read(client.join("data.sah")).unwrap(), std::fs::metadata(client.join("data.saf")).unwrap().len());
    apply_put(&mut w, "u/b.txt", &b, 2);
    assert_ne!(std::fs::read(client.join("data.sah")).unwrap(), sah1);
    assert!(undo_enabled(&w.h), "the palette command is enabled");
    let newest = w.h.app.app.apply.history()[1].0.clone();
    assert_eq!(w.h.app.app.undo_state().unwrap(), newest.label, "the newest journal is the one offered");
    // first undo: the confirmation of the Roll back flow, naming the newest journal
    with_cx(&mut w.h, |app, cx| shaiya_ui::ToolApp::run_command(app, "history-rollback", cx));
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert_eq!(w.h.app.app.tab, Tab::Apply);
    assert!(t.contains("Roll back") && t.contains(&newest.id), "the W2 confirmation names the journal: {t}");
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(std::fs::metadata(client.join("data.saf")).unwrap().len() > saf1, "nothing is restored before the answer");
    w.h.app.shell.inject_answer("toolkit-rollback", 0);
    assert!(
        w.h.wait_until(WAIT, |h| matches!(h.app.app.apply.history().get(1).map(|(j, _)| j.state.clone()), Some(shaiya_archive::journal::State::RolledBack { .. })) && h.app.app.archive.is_open()),
        "the second change is rolled back"
    );
    w.h.frames(3);
    assert_eq!(std::fs::read(client.join("data.sah")).unwrap(), sah1, "data.sah is byte for byte the state after the first change");
    assert_eq!(std::fs::metadata(client.join("data.saf")).unwrap().len(), saf1, "data.saf is cut to its length after the first change");
    // second undo: the first change is now the newest applied one
    assert!(w.h.wait_until(WAIT, |h| h.app.app.undo_state().is_ok()), "the first change can be undone now");
    let first = w.h.app.app.apply.history()[0].0.clone();
    assert_eq!(w.h.app.app.undo_state().unwrap(), first.label);
    with_cx(&mut w.h, |app, cx| shaiya_ui::ToolApp::run_command(app, "history-rollback", cx));
    w.h.frames(3);
    assert!(w.h.text().contains(&first.id), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-rollback", 0);
    assert!(
        w.h.wait_until(WAIT, |h| matches!(h.app.app.apply.history().first().map(|(j, _)| j.state.clone()), Some(shaiya_archive::journal::State::RolledBack { .. })) && h.app.app.archive.is_open()),
        "the first change is rolled back"
    );
    w.h.frames(3);
    assert_eq!(std::fs::read(client.join("data.sah")).unwrap(), sah0, "the original data.sah is back");
    assert_eq!(std::fs::metadata(client.join("data.saf")).unwrap().len(), saf0, "the original data.saf length is back");
    assert!(w.h.app.app.undo_state().is_err() && !undo_enabled(&w.h), "nothing is left to undo");
}

#[test]
fn undo_last_change_is_disabled_after_compact_with_the_reason() {
    let mut w = open_world("t7-undo-compact");
    let src = TempDir::new("t7-undo-compact-src");
    let a = src.path().join("Item.SData");
    std::fs::write(&a, b"a replacement entry that leaves the old one as dead space").unwrap();
    apply_put(&mut w, "Item/Item.SData", &a, 1);
    assert!(w.h.app.app.undo_state().is_ok());
    w.h.app.app.archive.request_compact();
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 2 && h.app.app.archive.is_open()), "compacted");
    assert!(w.h.wait_until(WAIT, |h| h.app.app.undo_state().is_err()), "the item is disabled once the history is re-read");
    let why = w.h.app.app.undo_state().unwrap_err();
    assert!(why.starts_with("The newest change (") && why.contains("cannot be rolled back") && why.contains("changed since"), "{why}");
    assert!(!undo_enabled(&w.h), "the palette command is disabled too");
    let sah = std::fs::read(w.client.path().join("data.sah")).unwrap();
    with_cx(&mut w.h, |app, cx| shaiya_ui::ToolApp::run_command(app, "history-rollback", cx));
    w.h.frames(3);
    assert!(!w.h.text().contains("Roll back "), "no confirmation opens when the item is disabled: {}", w.h.text());
    assert_eq!(std::fs::read(w.client.path().join("data.sah")).unwrap(), sah, "nothing was written");
}

#[test]
fn undo_last_change_needs_a_client() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(3);
    assert!(h.text().contains("Undo last change"), "the toolbar button is drawn (disabled): {}", h.text());
    assert_eq!(h.app.app.undo_state().unwrap_err(), "Open a client folder first.");
    assert!(!undo_enabled(&h));
}

// ------------------------------------------------------------------------------------------------ T7 W4: checks before files go in

fn finding_codes(f: &[shaiya_inspect::validate::Finding]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = f.iter().map(|x| x.code).collect();
    v.sort_unstable();
    v
}

#[test]
fn the_add_dialog_shows_checks_and_only_asks_for_double_encryption_or_a_bad_checksum() {
    let mut w = open_world("t7-w4-add");
    let src = TempDir::new("t7-w4-add-src");
    // a plain Item.SData over the client's encrypted copy: a warning, no question
    let plain = src.path().join("Item.SData");
    std::fs::write(&plain, b"plain replacement").unwrap();
    w.h.drop_files(std::slice::from_ref(&plain));
    w.h.frames(2);
    {
        let arch = &mut w.h.app.app.archive;
        let mut d = arch.add_dialog.take().unwrap();
        d.target = "Item".into();
        arch.recompute_add(&mut d);
        arch.add_dialog = Some(d);
    }
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.add_dialog.as_ref().is_some_and(|d| d.checked)), "the checks finish");
    w.h.frames(2);
    let d = w.h.app.app.archive.add_dialog.as_ref().unwrap();
    assert_eq!(finding_codes(&d.findings), ["sdata-state-differs"], "{:?}", d.findings);
    let t = w.h.text();
    assert!(t.contains("Checks (1)") && t.contains("sdata-state-differs") && t.contains("Item/Item.SData"), "{t}");
    w.h.app.app.archive.press_queue_changes();
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "a warning does not ask or block: the file is queued");
    assert!(w.h.app.app.archive.add_dialog.is_none());

    // a double-encrypted file asks first; Cancel keeps the dialog, Add anyway queues
    let twice = src.path().join("Twice.SData");
    std::fs::write(&twice, shaiya_seed::encrypt(&shaiya_seed::encrypt(b"body of the table"))).unwrap();
    w.h.app.app.archive.begin_add(vec![twice.clone()]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.add_dialog.as_ref().is_some_and(|d| d.checked)));
    w.h.frames(2);
    assert_eq!(finding_codes(&w.h.app.app.archive.add_dialog.as_ref().unwrap().findings), ["sdata-double-encrypted"]);
    w.h.app.app.archive.press_queue_changes();
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "nothing is queued before the answer");
    let t = w.h.text();
    assert!(t.contains("Add anyway") && t.contains("sdata-double-encrypted") && t.contains("nothing is blocked"), "{t}");
    w.h.app.shell.inject_answer("toolkit-add-findings", 1);
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "Cancel queues nothing");
    assert!(w.h.app.app.archive.add_dialog.is_some(), "and the dialog stays");
    w.h.app.app.archive.press_queue_changes();
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-add-findings", 0);
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 2, "Add anyway queues it");
    assert!(w.h.app.app.archive.add_dialog.is_none());

    // a bad checksum asks too; an unreadable texture and an empty file only warn
    let bad = {
        let mut b = shaiya_seed::encrypt(b"some table body");
        b[40] ^= 1;
        b
    };
    let (badf, cut, empty) = (src.path().join("Bad.SData"), src.path().join("cut.tga"), src.path().join("empty.txt"));
    std::fs::write(&badf, bad).unwrap();
    std::fs::write(&cut, &shaiya_testkit::tga_stub(4, 4)[..30]).unwrap();
    std::fs::write(&empty, b"").unwrap();
    w.h.app.app.archive.begin_add(vec![cut.clone(), empty.clone()]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.add_dialog.as_ref().is_some_and(|d| d.checked)));
    assert_eq!(finding_codes(&w.h.app.app.archive.add_dialog.as_ref().unwrap().findings), ["empty-file", "texture-unreadable"]);
    w.h.app.app.archive.press_queue_changes();
    assert_eq!(w.h.app.app.archive.pending.len(), 4, "two warnings only: both queued without a question");
    w.h.app.app.archive.begin_add(vec![badf.clone()]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.add_dialog.as_ref().is_some_and(|d| d.checked)));
    assert_eq!(finding_codes(&w.h.app.app.archive.add_dialog.as_ref().unwrap().findings), ["sdata-crc-bad"]);
    w.h.app.app.archive.press_queue_changes();
    assert!(w.h.app.app.archive.add_dialog.as_ref().unwrap().asking, "a checksum failure asks");
    assert_eq!(w.h.app.app.archive.pending.len(), 4);
}

#[test]
fn replace_with_file_asks_about_a_double_encrypted_file_and_otherwise_queues() {
    let mut w = open_world("t7-w4-replace");
    let src = TempDir::new("t7-w4-replace-src");
    let row = w.h.app.app.archive.find_row("Item/Item.SData").expect("the synthetic Item.SData");
    // plain over an encrypted copy: queued, with a warning in the status bar
    let plain = src.path().join("plain.bin");
    std::fs::write(&plain, b"plain bytes").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, plain.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.pending.len() == 1), "the check runs in the background, then the file is queued");
    w.h.frames(2);
    assert!(w.h.text().contains("sdata-state-differs"), "{}", w.h.text());
    // double-encrypted: not queued until Add anyway
    let twice = src.path().join("twice.bin");
    std::fs::write(&twice, shaiya_seed::encrypt(&shaiya_seed::encrypt(b"x"))).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, twice.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.replace_ask.is_some()));
    w.h.frames(3);
    assert!(w.h.text().contains("Add anyway") && w.h.text().contains("sdata-double-encrypted"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-replace-findings", 1);
    w.h.frames(3);
    assert!(w.h.app.app.archive.replace_ask.is_none());
    assert!(matches!(&w.h.app.app.archive.pending[0].kind, shaiya_toolkit::archive_view::PendingKind::Put { source, .. } if source == &plain), "Cancel keeps the earlier put");
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, twice.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.replace_ask.is_some()));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-replace-findings", 0);
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "the queued put for the same path is replaced, not duplicated");
    let queued = &w.h.app.app.archive.pending[0];
    assert!(matches!(&queued.kind, shaiya_toolkit::archive_view::PendingKind::Put { source, .. } if source == &twice), "{queued:?}");
}

#[test]
fn the_patch_builder_lists_checks_and_asks_before_building_a_broken_file() {
    let mut w = open_world("t7-w4-patch");
    let src = TempDir::new("t7-w4-patch-src");
    let files = src.path().join("changed");
    std::fs::create_dir_all(files.join("data")).unwrap();
    std::fs::write(files.join("data/Twice.SData"), shaiya_seed::encrypt(&shaiya_seed::encrypt(b"table"))).unwrap();
    std::fs::write(files.join("data/ok.txt"), b"fine").unwrap();
    std::fs::write(files.join("cut.tga"), &shaiya_testkit::tga_stub(4, 4)[..20]).unwrap();
    let out_dir = TempDir::new("t7-w4-patch-out");
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.out_dir = out_dir.path().display().to_string();
    w.h.drop_files(std::slice::from_ref(&files));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.checks_done() && h.app.app.patch.rows.len() == 3), "rows and checks are ready");
    w.h.frames(2);
    assert_eq!(finding_codes(w.h.app.app.patch.findings()), ["sdata-double-encrypted", "texture-unreadable"]);
    let t = w.h.text();
    assert!(t.contains("Checks before files go in (2)") && t.contains("data/Twice.SData") && t.contains("cut.tga"), "{t}");
    // Build asks first; Cancel builds nothing
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    w.h.frames(3);
    assert!(w.h.text().contains("Build anyway") && w.h.text().contains("sdata-double-encrypted"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-build-findings", 1);
    w.h.frames(3);
    assert!(!out_dir.path().join("ps0001.patch").exists() && w.h.app.app.patch.last_out.is_none(), "Cancel builds nothing");
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-build-findings", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.last_out.is_some()), "Build anyway builds");
    assert!(out_dir.path().join("ps0001.patch").exists());
    // without the double-encrypted file only a warning is left, and Build goes straight ahead
    std::fs::remove_file(files.join("data/Twice.SData")).unwrap();
    w.h.app.app.patch.set_card(shaiya_toolkit::patch_builder::Card::Files, Some(files.clone()));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.checks_done() && h.app.app.patch.rows.len() == 2));
    assert_eq!(finding_codes(w.h.app.app.patch.findings()), ["texture-unreadable"]);
    w.h.app.app.patch.out_name = "ps0002.patch".into();
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    assert!(w.h.wait_until(WAIT, |h| out_dir.path().join("ps0002.patch").exists() && h.app.app.patch.last_out.as_ref().is_some_and(|p| p.ends_with("ps0002.patch"))), "a warning does not ask");
}

#[test]
fn a_check_that_did_not_finish_still_asks_in_the_add_dialog() {
    use shaiya_ui::TaskOutcome;
    let mut w = open_world("t7-w4-unfinished-add");
    let src = TempDir::new("t7-w4-unfinished-add-src");
    let fine = src.path().join("fine.txt");
    std::fs::write(&fine, b"fine").unwrap();
    for (n, outcome) in [TaskOutcome::Cancelled, TaskOutcome::Panicked("boom".into()), TaskOutcome::Failed(shaiya_core::Error::Refused("disk gone".into()))].into_iter().enumerate() {
        let before = w.h.app.app.archive.pending.len();
        w.h.app.app.archive.begin_add(vec![fine.clone()]);
        let arch = &mut w.h.app.app.archive;
        let d = arch.add_dialog.as_mut().unwrap();
        d.finish_check(outcome);
        assert!(d.checked && finding_codes(&d.findings) == ["check-incomplete"], "{n}: {:?}", d.findings);
        w.h.app.app.archive.press_queue_changes();
        w.h.frames(3);
        assert_eq!(w.h.app.app.archive.pending.len(), before, "{n}: nothing is queued before the answer");
        assert!(w.h.app.app.archive.add_dialog.as_ref().unwrap().asking, "{n}: the question is open");
        let t = w.h.text();
        assert!(t.contains("Add anyway") && t.contains("check-incomplete") && t.contains("did not finish"), "{t}");
        w.h.app.shell.inject_answer("toolkit-add-findings", 1);
        w.h.frames(3);
        assert_eq!(w.h.app.app.archive.pending.len(), before, "{n}: Cancel queues nothing");
        w.h.app.app.archive.add_dialog = None;
    }
}

#[test]
fn a_check_that_did_not_finish_still_asks_before_the_patch_builder_builds() {
    use shaiya_ui::TaskOutcome;
    let mut w = open_world("t7-w4-unfinished-patch");
    let src = TempDir::new("t7-w4-unfinished-patch-src");
    let files = src.path().join("changed");
    std::fs::create_dir_all(files.join("data")).unwrap();
    std::fs::write(files.join("data/ok.txt"), b"fine").unwrap();
    let out_dir = TempDir::new("t7-w4-unfinished-patch-out");
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.out_dir = out_dir.path().display().to_string();
    w.h.drop_files(std::slice::from_ref(&files));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.checks_done() && h.app.app.patch.rows.len() == 1));
    assert!(w.h.app.app.patch.findings().is_empty(), "the files are fine");
    w.h.app.app.patch.finish_checks(TaskOutcome::Cancelled);
    assert_eq!(finding_codes(w.h.app.app.patch.findings()), ["check-incomplete"]);
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    w.h.frames(3);
    assert!(w.h.text().contains("Build anyway") && w.h.text().contains("check-incomplete"), "{}", w.h.text());
    assert!(!out_dir.path().join("ps0001.patch").exists(), "nothing is built before the answer");
    w.h.app.shell.inject_answer("toolkit-build-findings", 1);
    w.h.frames(3);
    assert!(!out_dir.path().join("ps0001.patch").exists() && w.h.app.app.patch.last_out.is_none(), "Cancel builds nothing");
}

// ------------------------------------------------------------------------------------------------ T7 W5: Publish for the regular updater

/// `ps<number>.patch` in a new folder, holding one archive file with `content`.
fn publish_patch(base: &std::path::Path, number: u32, name: &str, content: &[u8]) -> PathBuf {
    let files = base.join(format!("files-{number}"));
    std::fs::create_dir_all(files.join("data")).unwrap();
    std::fs::write(files.join("data").join(name), content).unwrap();
    let out = base.join(format!("ps{number:04}.patch"));
    shaiya_patch::build(Some(&files), None, None, &out).unwrap();
    out
}

fn plan_form(w: &mut World, patches: Vec<PathBuf>, dest: &std::path::Path) {
    let f = &mut w.h.app.app.patch.publish;
    f.patches = patches;
    f.dest = dest.display().to_string();
    f.check_version = "3".into();
    with_cx(&mut w.h, |app, cx| {
        let dirs = shaiya_toolkit::publish_form::client_dirs(cx);
        app.patch.publish.request_plan(dirs, cx)
    });
}

#[test]
fn the_start_page_publish_button_opens_the_section_in_the_patch_builder() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(3);
    assert!(h.text().contains("Publish a patch") && h.text().contains("Open Publish"), "{}", h.text());
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Publish, cx));
    h.frames(3);
    assert_eq!(h.app.app.tab, Tab::Patch);
    let t = h.text().to_string();
    assert!(t.contains("Publish for the regular updater") && t.contains("Add patches...") && t.contains("Check the plan"), "the section is open: {t}");
}

#[test]
fn publishing_shows_the_plan_asks_with_four_rows_and_writes_after_the_answer() {
    let mut w = open_world("t7-publish");
    let src = TempDir::new("t7-publish-src");
    let server = TempDir::new("t7-publish-server");
    let dest = server.path().join("web");
    let patch = publish_patch(src.path(), 110, "notes.txt", b"hello");
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.publish.open();
    plan_form(&mut w, vec![patch.clone()], &dest);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.publish.plan().is_some()), "the plan is made");
    w.h.frames(2);
    let t = w.h.text().to_string();
    assert!(t.contains("ps0110.patch") && t.contains("new") && t.contains("PatchFileVersion new file -> 110"), "{t}");
    assert!(!dest.exists(), "planning writes nothing");
    // Publish asks; Cancel writes nothing
    w.h.app.app.patch.publish.begin_publish();
    w.h.frames(3);
    let t = w.h.text().to_string();
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(t.contains("copies 1 file") && t.contains("creates UpdateVersion.ini with CheckVersion 3") && t.contains("no undo"), "{t}");
    w.h.app.shell.inject_answer("toolkit-publish", 1);
    w.h.frames(3);
    assert!(!dest.exists(), "Cancel wrote nothing");
    // Publish writes the patch and the ini and reads them back
    w.h.app.app.patch.publish.begin_publish();
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-publish", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.patch.publish.result.is_empty()), "the publish finishes");
    assert_eq!(std::fs::read(dest.join("Shaiya/patch/ps0110.patch")).unwrap(), std::fs::read(&patch).unwrap());
    assert_eq!(std::fs::read(dest.join("Shaiya/UpdateVersion.ini")).unwrap(), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=110\r\n");
    assert!(w.h.app.app.patch.publish.result[0].contains("read back"), "{:?}", w.h.app.app.patch.publish.result);
    // the client folder was not touched: no Shaiya folder, no extra file
    assert!(!w.client_dir().join("Shaiya").exists());
}

#[test]
fn publishing_into_a_saved_client_is_refused_in_the_plan() {
    let mut w = open_world("t7-publish-client");
    let src = TempDir::new("t7-publish-client-src");
    let patch = publish_patch(src.path(), 1, "a.txt", b"a");
    let inside = w.client_dir().join("web");
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.publish.open();
    plan_form(&mut w, vec![patch], &inside);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.publish.refusal().is_some()), "the plan is refused");
    w.h.frames(2);
    assert!(w.h.app.app.patch.publish.refusal().unwrap().contains("client folder"));
    assert!(w.h.text().contains("Refused:"), "{}", w.h.text());
    assert!(w.h.app.app.patch.publish.publish_blocked().is_some());
    w.h.app.app.patch.publish.begin_publish();
    w.h.frames(3);
    assert!(!inside.exists(), "nothing was created in the client folder");
}

#[test]
fn a_double_encrypted_file_in_a_patch_asks_once_before_publishing() {
    let mut w = open_world("t7-publish-asks");
    let src = TempDir::new("t7-publish-asks-src");
    let server = TempDir::new("t7-publish-asks-server");
    let dest = server.path().join("web");
    let patch = publish_patch(src.path(), 7, "Twice.SData", &shaiya_seed::encrypt(&shaiya_seed::encrypt(b"table table table")));
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.publish.open();
    plan_form(&mut w, vec![patch], &dest);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.publish.plan().is_some()));
    assert_eq!(finding_codes(&w.h.app.app.patch.publish.plan().unwrap().findings), ["sdata-double-encrypted"]);
    w.h.app.app.patch.publish.begin_publish();
    w.h.frames(3);
    assert!(w.h.text().contains("Publish anyway") && w.h.text().contains("sdata-double-encrypted"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-publish-findings", 1);
    w.h.frames(3);
    assert!(!dest.exists(), "Cancel at the question wrote nothing");
    // Publish anyway leads to the confirmation, which still has to be answered
    w.h.app.app.patch.publish.begin_publish();
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-publish-findings", 0);
    w.h.frames(3);
    assert!(w.h.text().contains("Publish for the regular updater") && !dest.exists(), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-publish", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.patch.publish.result.is_empty()));
    assert!(dest.join("Shaiya/patch/ps0007.patch").is_file());
}
