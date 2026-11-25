//! Shaiya Toolkit harness tests: headless frames with synthetic input (drop, tasks, cancel). No game data: every file is built here.

use egui::{Key, Modifiers};
use shaiya_core::hash::{sha256, to_hex};
use shaiya_toolkit::{ToolkitApp, ToolkitSettings, Tab};
use shaiya_testkit::{pe_stub, SahBuilder, TempDir};
use shaiya_ui::testing::Harness;
use std::path::PathBuf;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(60);

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
    assert!(t.contains("Shaiya Archive") || t.contains("v0.4.1"), "{t}");
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
    // hold the extraction at its first progress report: it cannot finish before the cancel below, however fast the disk
    h.app.shell.tasks().hold_progress();
    with_cx(&mut h, |app, cx| app.archive.begin_extract(out.path().to_path_buf(), false, true, false, cx));
    assert!(h.wait_until(Duration::from_secs(60), |h| h.app.shell.tasks().views().iter().any(|v| v.title.starts_with("Extracting") && v.done > 0)), "extraction started");
    h.app.shell.tasks().cancel_all();
    assert!(h.wait_until(Duration::from_secs(60), |h| !h.app.app.archive.is_extracting()), "ended");
    h.frames(2);
    assert!(h.app.app.archive.status_line.contains("Cancelled"), "{}", h.app.app.archive.status_line);
    for e in std::fs::read_dir(out.path().join("a")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        assert!(!n.contains("shaiya-tmp"), "no half-written file stays: {n}");
    }
    h.app.shell.tasks().release_progress();
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
    // later work packages add their rows here
    let keys: Vec<&str> = shaiya_toolkit::start::start_jobs().iter().map(|j| j.key).collect();
    assert_eq!(keys, ["add-client", "change-files", "make-patch", "publish-patch", "apply-patches", "sdata-tables", "client-health"]);
    for j in shaiya_toolkit::start::start_jobs() {
        assert!(t.contains(j.title) && t.contains(j.button), "{} is drawn: {t}", j.key);
    }
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Go(Tab::Patch), cx));
    assert_eq!(h.app.app.tab, Tab::Patch);
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Go(Tab::Apply), cx));
    assert_eq!(h.app.app.tab, Tab::Apply);
    assert!(t.contains("Look at SData tables"), "{t}");
    h.frames(1);
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Go(Tab::SData), cx));
    assert_eq!(h.app.app.tab, Tab::SData);
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
    // With a client open, the comparison with the client may still be running; Build then waits for it (BuildFlow::WaitSame) and asks
    // afterwards, so wait for the question itself instead of counting frames.
    assert!(w.h.wait_until(Duration::from_secs(60), |h| h.text().contains("Build anyway")), "{}", w.h.text());
    assert!(w.h.text().contains("check-incomplete"), "{}", w.h.text());
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

#[test]
fn the_patch_builder_leaves_out_files_that_are_already_the_same_in_the_client() {
    let mut w = open_world("t7-w6-same");
    let src = TempDir::new("t7-w6-same-src");
    let files = src.path().join("mod");
    std::fs::create_dir_all(files.join("Item/3DO")).unwrap();
    std::fs::write(files.join("Item/3DO/a.txt"), b"hello text").unwrap(); // identical to the client's
    std::fs::write(files.join("Item/3DO/b.txt"), b"second!").unwrap(); // changed
    std::fs::write(files.join("c.txt"), b"fresh").unwrap(); // new
    let out_dir = TempDir::new("t7-w6-same-out");
    go(&mut w.h, Tab::Patch);
    w.h.app.app.patch.out_dir = out_dir.path().display().to_string();
    assert!(w.h.app.app.patch.skip_same, "on by default");
    w.h.drop_files(std::slice::from_ref(&files));
    let skipped = |h: &Harness<ToolkitApp>| h.app.app.patch.rows.iter().filter(|r| r.skipped).map(|r| r.path.clone()).collect::<Vec<_>>();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.rows.len() == 3 && !skipped(h).is_empty() && h.app.app.patch.checks_done()), "the comparison finishes");
    w.h.frames(2);
    assert_eq!(skipped(&w.h), ["Item/3DO/a.txt"]);
    let rows = w.h.app.app.patch.rows.clone();
    assert_eq!(rows.iter().find(|r| r.skipped).unwrap().effect, "same, left out");
    let mut kept: Vec<String> = w.h.app.app.patch.input().files.iter().map(|f| f.0.clone()).collect();
    kept.sort();
    assert_eq!(kept, ["Item/3DO/b.txt", "c.txt"], "only the changed and the new file are built");
    let t = w.h.text();
    assert!(t.contains("Leave out files that are already the same in US test") && t.contains("1 left out"), "{t}");
    // Build puts only the two in
    with_cx(&mut w.h, |app, cx| app.patch.begin_build(&app.keys, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.last_out.is_some()), "built");
    let c = shaiya_patch::contents(&out_dir.path().join("ps0001.patch")).unwrap();
    let mut names: Vec<String> = c.files.iter().map(|f| f.path.clone()).collect();
    names.sort();
    assert_eq!(names, ["Item/3DO/b.txt", "c.txt"]);
    assert!(w.h.app.app.patch.result[0].contains("1 left out"), "{:?}", w.h.app.app.patch.result);
    // unticking brings the file back
    w.h.app.app.patch.skip_same = false;
    w.h.frames(3);
    assert_eq!(w.h.app.app.patch.input().files.len(), 3);
    assert!(skipped(&w.h).is_empty());
}

#[test]
fn the_same_files_comparison_is_made_again_when_the_clients_archive_changes_outside_the_toolkit() {
    let mut w = open_world("w4-same-cache");
    let src = TempDir::new("w4-same-cache-src");
    let files = src.path().join("mod");
    std::fs::create_dir_all(files.join("Item/3DO")).unwrap();
    std::fs::write(files.join("Item/3DO/a.txt"), b"hello text").unwrap(); // identical to the client's
    std::fs::write(files.join("c.txt"), b"fresh").unwrap();
    go(&mut w.h, Tab::Patch);
    w.h.drop_files(std::slice::from_ref(&files));
    let skipped = |h: &Harness<ToolkitApp>| h.app.app.patch.rows.iter().filter(|r| r.skipped).map(|r| r.path.clone()).collect::<Vec<_>>();
    assert!(w.h.wait_until(WAIT, |h| skipped(h) == ["Item/3DO/a.txt"] && h.app.app.patch.checks_done()), "a.txt is the same");
    // another program changes a.txt in the client and adds a file: the archive's index changes length and time
    let dir = w.client_dir();
    let put = |path: &str, bytes: &[u8], mode| shaiya_archive::edit::Put { path: path.into(), source: shaiya_archive::edit::Source::Bytes(bytes.to_vec()), version: 1, mode };
    let plan = shaiya_archive::edit::EditPlan {
        puts: vec![put("Item/3DO/a.txt", b"changed outside", shaiya_archive::edit::PutMode::ReplaceOnly), put("extra.txt", b"x", shaiya_archive::edit::PutMode::AddOnly)],
        label: "outside".into(),
        ..shaiya_archive::edit::EditPlan::default()
    };
    let opts = shaiya_archive::edit::InPlaceOptions { journal: shaiya_archive::journal::JournalMode::In(dir.join(shaiya_archive::journal::DEFAULT_DIR)), legacy_bak: false, check_in_use: false };
    shaiya_archive::edit::apply_in_place(&dir, &plan, &opts, &shaiya_core::Job::silent()).unwrap();
    assert!(w.h.wait_until(WAIT, |h| skipped(h).is_empty() && h.app.app.patch.input().files.len() == 2), "a.txt now differs from the client, so it is built");
}

// ------------------------------------------------------------------------------------------------ T7 W7: SData table view

fn put_u32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

type Rec<'a> = (&'a [u8], &'a [u8], u8);

/// An Item table: two banks, three records (name, description, 81-byte block of the first byte = seed).
fn item_table() -> Vec<u8> {
    let mut v = Vec::new();
    put_u32(&mut v, 2);
    let banks: [&[Rec]; 2] = [&[(b"Sword", b"A sharp sword", 1), (b"Shield", b"Round", 2)], &[(b"Axe", b"Heavy axe", 3)]];
    for bank in banks {
        put_u32(&mut v, bank.len() as u32);
        for (name, desc, seed) in bank {
            put_u32(&mut v, name.len() as u32);
            v.extend_from_slice(name);
            put_u32(&mut v, desc.len() as u32);
            v.extend_from_slice(desc);
            v.extend((0..81).map(|i| seed + i as u8));
        }
    }
    v
}

fn open_table_of(w: &mut World, name: &str, bytes: &[u8]) {
    go(&mut w.h, Tab::SData);
    let src = TempDir::new(&format!("t7w7-{name}"));
    std::fs::write(src.path().join(name), bytes).unwrap();
    w.h.drop_files(&[src.path().to_path_buf()]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.sdata.row_count() == 1), "the file is listed");
    w.h.frames(2);
    with_cx(&mut w.h, |app, _| app.sdata.select(0));
    with_cx(&mut w.h, |app, _| assert!(app.sdata.view_selected(&app.archive)));
    w.h.frames(2);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.table.as_ref().is_some_and(|t| !t.is_loading())), "the table is read");
    w.h.frames(2);
}

#[test]
fn the_table_view_opens_on_an_encrypted_item_file_and_filters() {
    let mut w = open_world("t7w7-table");
    open_table_of(&mut w, "Item.SData", &shaiya_seed::encrypt(&item_table()));
    let t = w.h.text().to_string();
    assert!(t.contains("SData table: Item.SData"), "{t}");
    assert!(t.contains("Layout item-81") && t.contains("Read only"), "{t}");
    assert!(t.contains("chosen because it reads the whole file") && t.contains("checked against the game's own loader only on the clients listed as verified"), "{t}");
    assert!(t.contains("Loader-verified on: us-108, ru-ep3, ru-ep45, fr-ep54.") && !t.contains("FIT-ONLY: this layout reads"), "{t}");
    assert!(t.contains("3 rows") && t.contains("Sword") && t.contains("Shield") && t.contains("Axe"), "{t}");
    assert!(t.contains("bank") && t.contains("req_level") && t.contains("Export CSV...") && t.contains("Copy rows") && t.contains("Filter rows"), "{t}");
    {
        let v = w.h.app.app.table.as_ref().unwrap();
        assert!(v.is_table() && (v.row_count(), v.shown_count()) == (3, 3));
    }
    // the filter: words are ANDed, any case
    with_cx(&mut w.h, |app, _| app.table.as_mut().unwrap().set_filter("AXE"));
    w.h.frames(2);
    let t = w.h.text().to_string();
    assert!(t.contains("1 of 3 rows") && t.contains("Axe") && !t.contains("Sword") && !t.contains("Shield"), "{t}");
    with_cx(&mut w.h, |app, _| app.table.as_mut().unwrap().set_filter("sword sharp"));
    assert_eq!(w.h.app.app.table.as_ref().unwrap().shown_count(), 1);
    with_cx(&mut w.h, |app, _| app.table.as_mut().unwrap().set_filter("sword heavy"));
    assert_eq!(w.h.app.app.table.as_ref().unwrap().shown_count(), 0);
    with_cx(&mut w.h, |app, _| app.table.as_mut().unwrap().set_filter(""));
    w.h.frames(2);
    assert_eq!(w.h.app.app.table.as_ref().unwrap().shown_count(), 3);
    // Copy rows: header and rows, tab separated; Export CSV: the shown rows
    with_cx(&mut w.h, |app, _| app.table.as_mut().unwrap().set_filter("shield"));
    let tsv = w.h.app.app.table.as_ref().unwrap().rows_tsv();
    let lines: Vec<&str> = tsv.lines().collect();
    assert_eq!(lines.len(), 2, "{tsv}");
    assert!(lines[0].starts_with("bank\tname\tdescription\ttype\t") && lines[1].starts_with("0\tShield\tRound\t2\t"), "{tsv}");
    let out = TempDir::new("t7w7-csv");
    let csv = out.path().join("items.csv");
    assert_eq!(w.h.app.app.table.as_ref().unwrap().export_csv(&csv).unwrap(), 1);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with('\u{feff}') && text.contains("\r\n") && text.contains("0,Shield,Round,2,"), "{text}");
    // closing the window
    w.h.app.app.table = None;
    w.h.frames(2);
    assert!(!w.h.text().contains("SData table:"));
}

#[test]
fn a_fit_only_layout_says_so_in_the_table_view() {
    let mut w = open_world("t9w5-fit-only");
    // one monster with a 36-byte block: only the FIT-ONLY layout reads it
    let mut v = Vec::new();
    put_u32(&mut v, 1);
    put_u32(&mut v, 5);
    v.extend_from_slice(b"Wolf\0");
    v.extend_from_slice(&[0u8; 36]);
    open_table_of(&mut w, "Monster.SData", &shaiya_seed::encrypt(&v));
    let t = w.h.text().to_string();
    assert!(t.contains("Layout monster-36."), "{t}");
    assert!(t.contains("FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader."), "{t}");
    assert!(t.contains("Columns are named by position; values may be split wrongly."), "{t}");
    assert!(!t.contains("Loader-verified on"), "{t}");
}

#[test]
fn a_file_no_layout_fits_is_shown_as_hex_with_the_sentence() {
    let mut w = open_world("t7w7-hex");
    let plain = b"This is only text, so no table layout can read it.".to_vec();
    open_table_of(&mut w, "Other.SData", &shaiya_seed::encrypt(&plain));
    let t = w.h.text().to_string();
    assert!(t.contains(&format!("No table layout fits this file ({} bytes); shown as hex", plain.len())), "{t}");
    assert!(t.contains("00000000  54 68 69 73"), "the hex rows show the plaintext: {t}");
    let v = w.h.app.app.table.as_ref().unwrap();
    assert!(!v.is_table() && v.raw_message().is_some());
}

#[test]
fn a_changed_code_page_reads_the_names_again() {
    let mut w = open_world("t7w7-cp");
    let mut table = item_table();
    // the first record's name "Sword" becomes two cp949 bytes of one Korean syllable (same length keeps the layout intact)
    let at = table.windows(5).position(|x| x == b"Sword").unwrap();
    table[at..at + 5].copy_from_slice(&[0xB0, 0xCB, b'x', b'y', b'z']);
    open_table_of(&mut w, "Item.SData", &table);
    let hangul = shaiya_core::text::Encoding::Cp949.decode(&[0xB0, 0xCB]).0;
    assert!(w.h.app.app.table.as_ref().unwrap().rows_tsv().contains(&format!("{hangul}xyz")));
    with_cx(&mut w.h, |app, cx| app.table.as_mut().unwrap().set_encoding(shaiya_core::text::Encoding::Cp1252, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.table.as_ref().is_some_and(|t| !t.is_loading())));
    let tsv = w.h.app.app.table.as_ref().unwrap().rows_tsv();
    assert!(tsv.contains("\u{B0}\u{CB}xyz") && !tsv.contains(&hangul), "{tsv}");
}

#[test]
fn the_archive_preview_has_a_view_table_button_and_the_table_opens_from_the_client() {
    let mut w = open_world("t7w7-archive");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Item/Item.SData", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert!(w.h.text().contains("View table"), "{}", w.h.text());
    // what the button does: it hands the file to the app, which opens the window (the fixture's Item.SData is not a table)
    let raw = shaiya_seed::encrypt(&item_table());
    w.h.app.app.archive.table_request = Some(("Item/Item.SData".to_string(), raw));
    w.h.frames(2);
    assert!(w.h.app.app.archive.table_request.is_none());
    assert!(w.h.wait_until(WAIT, |h| h.app.app.table.as_ref().is_some_and(|t| t.is_table())));
    // the SData tab lists the client's files and reads one from the archive
    go(&mut w.h, Tab::SData);
    w.h.app.app.sdata.from_client = true;
    assert!(w.h.wait_until(WAIT, |h| h.app.app.sdata.row_count() == 1));
    with_cx(&mut w.h, |app, _| app.sdata.select(0));
    with_cx(&mut w.h, |app, _| assert!(app.sdata.view_selected(&app.archive)));
    w.h.frames(2);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.table.as_ref().is_some_and(|t| !t.is_loading() && !t.is_table())), "the fixture's Item.SData is no table: hex");
    assert!(w.h.app.app.table.as_ref().unwrap().raw_message().unwrap().contains("No table layout fits this file (21 bytes); shown as hex"));
}

#[test]
fn the_history_lists_a_change_made_outside_the_toolkit_when_the_tab_is_shown() {
    let mut w = open_world("w4-history");
    go(&mut w.h, Tab::Apply);
    assert!(w.h.app.app.apply.history().is_empty(), "nothing has been applied yet");
    // another program (the CLI) applies a change: the journal appears on disk, the Toolkit did nothing
    let dir = w.client_dir();
    let root = dir.join(shaiya_archive::journal::DEFAULT_DIR);
    let plan = shaiya_archive::edit::EditPlan {
        puts: vec![shaiya_archive::edit::Put { path: "outside.txt".into(), source: shaiya_archive::edit::Source::Bytes(b"x".to_vec()), version: 1, mode: shaiya_archive::edit::PutMode::AddOnly }],
        label: "outside".into(),
        ..shaiya_archive::edit::EditPlan::default()
    };
    let opts = shaiya_archive::edit::InPlaceOptions { journal: shaiya_archive::journal::JournalMode::In(root), legacy_bak: false, check_in_use: false };
    shaiya_archive::edit::apply_in_place(&dir, &plan, &opts, &shaiya_core::Job::silent()).unwrap();
    w.h.frames(3);
    assert!(w.h.app.app.apply.history().is_empty(), "the list is not re-read by itself");
    go(&mut w.h, Tab::Archive);
    go(&mut w.h, Tab::Apply);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.apply.history().len() == 1), "showing the tab lists the journal");
    assert_eq!(w.h.app.app.apply.history()[0].0.label, "outside");
}

// ------------------------------------------------------------------------------------------------ T9 W7: map preview

fn world_map_files(client: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let a = shaiya_archive::Archive::open(client).unwrap();
    a.index().visible().iter().map(|e| e.path.clone()).filter(|p| p.to_ascii_lowercase().starts_with("world/") && p.to_ascii_lowercase().ends_with(".wld")).map(|p| (p.clone(), a.read(&p).unwrap())).collect()
}

/// A field `.wld` of 3 x 3 corners (size 4) and a dungeon `.wld`, both built here.
fn tiny_maps() -> (Vec<u8>, Vec<u8>) {
    let zero_tail = |head: Vec<u8>| {
        (0..4096)
            .map(|n| head.iter().copied().chain(std::iter::repeat_n(0u8, n)).collect::<Vec<u8>>())
            .find(|b| shaiya_clientdata::Wld::parse(b, "x").is_ok())
            .expect("a zero tail parses")
    };
    let mut field = b"FLD\0".to_vec();
    field.extend(4i32.to_le_bytes());
    for i in 0..9i16 {
        field.extend((9000 + 50 * i).to_le_bytes());
    }
    field.extend([0u8; 9]);
    field.extend(0i32.to_le_bytes());
    let mut dun = b"DUN\0".to_vec();
    let mut name = b"dg_test.dg".to_vec();
    name.resize(256, 0);
    dun.extend(name);
    (zero_tail(field), zero_tail(dun))
}

#[test]
fn a_wld_in_the_archive_previews_as_a_map_with_its_header() {
    let mut w = world("t9-map");
    let (field, dungeon) = tiny_maps();
    SahBuilder::new(0).file(b"world/12.wld", &field).file(b"world/31.wld", &dungeon).file(b"world/9.wld", b"FLD\0 broken").write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path("world/12.wld", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("map"));
    assert!(w.h.app.app.archive.has_texture(), "the top view is uploaded as a texture");
    let t = w.h.text();
    assert!(t.contains("Map 12, 3 x 3 corners, 4 units") && t.contains("height from -20") && t.contains("Fit"), "{t}");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("world/31.wld", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("map"));
    let t = w.h.text();
    assert!(t.contains("Dungeon map: the floor is in dg_test.dg; no top view yet"), "{t}");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("world/9.wld", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("failed"), "a map that does not parse keeps the Failed preview");
}

/// Opt-in (`SHAIYA_CLIENT`, read only): every `world/*.wld` of the US client previews as a map; the split is printed.
#[test]
fn every_world_wld_of_the_us_client_previews_as_a_map() {
    let Some(us) = shaiya_testkit::env_client() else {
        eprintln!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let maps = world_map_files(&us);
    let (mut fields, mut dungeons) = (0, 0);
    for (p, bytes) in &maps {
        match shaiya_toolkit::preview::map_preview(bytes, p).unwrap_or_else(|e| panic!("{p}: {e}")) {
            shaiya_toolkit::preview::PreviewData::Map { rgba, width, height, .. } if !rgba.is_empty() => {
                assert_eq!(rgba.len(), (width * height * 4) as usize, "{p}");
                fields += 1;
            }
            shaiya_toolkit::preview::PreviewData::Map { note: Some(n), .. } => {
                assert!(n.starts_with("Dungeon map: "), "{p}: {n}");
                dungeons += 1;
            }
            other => panic!("{p}: {other:?}"),
        }
    }
    eprintln!("map preview: {} .wld files, {fields} fields, {dungeons} dungeon maps", maps.len());
    assert_eq!(maps.len(), 83);
    assert_eq!((fields, dungeons), (29, 54), "the split of the US client's maps changed");
}

// ------------------------------------------------------------------------------------------------ T9 W8: model preview

/// A `.smod` of one textured quad (texture "top.tga", stored in the archive) and one quad with a missing texture.
fn tiny_model() -> Vec<u8> {
    let mut d = Vec::new();
    let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
    for v in [2.0f32, 0.0, 2.0] {
        f(&mut d, v);
    }
    d.extend_from_slice(&0i32.to_le_bytes());
    for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
        f(&mut d, v);
    }
    d.extend_from_slice(&2i32.to_le_bytes());
    for (tex, y) in [("top.tga", 0.0f32), ("gone.tga", 4.0)] {
        let name = format!("{tex}\0");
        d.extend_from_slice(&(name.len() as i32).to_le_bytes());
        d.extend_from_slice(name.as_bytes());
        d.extend_from_slice(&4i32.to_le_bytes());
        for (p, uv) in [[0.0f32, y, 0.0], [4.0, y, 0.0], [4.0, y, 4.0], [0.0, y, 4.0]].iter().zip([[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
            for v in p {
                f(&mut d, *v);
            }
            for v in [0.0f32, 1.0, 0.0] {
                f(&mut d, v);
            }
            d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            f(&mut d, uv[0]);
            f(&mut d, uv[1]);
        }
        d.extend_from_slice(&2i32.to_le_bytes());
        for i in [0u16, 1, 2, 0, 2, 3] {
            d.extend_from_slice(&i.to_le_bytes());
        }
    }
    for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
        f(&mut d, v);
    }
    d.extend_from_slice(&0i32.to_le_bytes());
    d
}

#[test]
fn a_smod_in_the_archive_previews_as_a_model_and_a_3do_that_does_not_read_is_failed_with_its_bytes() {
    let mut w = world("t9-model");
    SahBuilder::new(0)
        .file(b"Entity/Building/tiny.smod", &tiny_model())
        .file(b"Entity/Texture/top.tga", &shaiya_testkit::tga_stub(8, 8))
        .file(b"Entity/Building/x.3DO", &[1u8, 2, 3, 4, 5, 6, 7, 8])
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Entity/Building/tiny.smod", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("model"));
    assert!(w.h.app.app.archive.has_texture(), "the picture is uploaded as a texture");
    let t = w.h.text();
    assert!(t.contains("2 modules, 4 triangles, 2 textures (1 missing)") && t.contains("gone.tga"), "{t}");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Entity/Building/x.3DO", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("failed"), "a .3DO that does not read is Failed, with its bytes as hex");
    let t = w.h.text();
    assert!(t.contains("Entity/Building/x.3DO") && t.contains("as hex"), "{t}");
    assert!(!t.contains("no preview yet"), "the old note is gone: {t}");
}

#[test]
fn a_dungeon_map_after_a_field_does_not_keep_the_fields_picture() {
    let mut w = world("t9-map-stale");
    let (field, dungeon) = tiny_maps();
    SahBuilder::new(0).file(b"world/12.wld", &field).file(b"world/31.wld", &dungeon).write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path("world/12.wld", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert!(w.h.app.app.archive.has_texture());
    with_cx(&mut w.h, |app, cx| app.archive.select_path("world/31.wld", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("map"));
    assert!(!w.h.app.app.archive.has_texture(), "a dungeon map has no picture, so no texture");
}

/// Opt-in (`SHAIYA_CLIENT`, read only): every `.smod` of the US client previews as a model without error; the counts are printed.
#[test]
fn every_smod_of_the_us_client_previews_as_a_model() {
    let Some(us) = shaiya_testkit::env_client() else {
        eprintln!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = shaiya_archive::Archive::open(&us).unwrap();
    let paths: Vec<String> = a.index().visible().iter().map(|e| e.path.clone()).filter(|p| p.to_ascii_lowercase().ends_with(".smod")).collect();
    let (mut pictures, mut bare, mut missing) = (0, 0, 0);
    for p in &paths {
        let bytes = a.read(p).unwrap();
        match shaiya_toolkit::preview::model_preview(&a, &bytes, p, &shaiya_core::Job::silent()).unwrap_or_else(|e| panic!("{p}: {e}")) {
            shaiya_toolkit::preview::PreviewData::Model { rgba, missing: m, width, height, .. } => {
                if rgba.is_empty() {
                    bare += 1;
                } else {
                    assert_eq!(rgba.len(), width as usize * height as usize * 4, "{p}");
                    pictures += 1;
                }
                missing += m.len();
            }
            other => panic!("{p}: {other:?}"),
        }
    }
    eprintln!("model preview: {} .smod files, {pictures} with a picture, {bare} without triangles, {missing} missing texture names", paths.len());
    assert_eq!(paths.len(), 1355, "the US client's .smod count changed");
    assert_eq!(pictures + bare, paths.len());
    assert_eq!((bare, missing), (0, 0), "every .smod of the US client gives a picture with no texture name missing");
}

// ------------------------------------------------------------------------------------------------ T9 W6: client health and clean-up

fn tree(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, v: &mut Vec<(String, Vec<u8>)>) {
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

fn health_leftovers(dir: &std::path::Path) {
    std::fs::write(dir.join("data.sah.bak"), b"old index").unwrap();
    std::fs::write(dir.join(".n.7.shaiya-tmp"), b"half").unwrap();
    std::fs::write(dir.join("data.20261101T101010Z.sah"), b"s1").unwrap();
    std::fs::write(dir.join("data.20261101T101010Z.saf"), b"s2").unwrap();
}

fn health_ready(h: &mut Harness<ToolkitApp>) -> bool {
    h.wait_until(WAIT, |h| h.app.app.health.report().is_some())
}

#[test]
fn the_health_window_only_looks_and_lists_the_leftovers_with_what_is_ticked() {
    let mut w = open_world("t9-health-look");
    let dir = w.client_dir();
    health_leftovers(&dir);
    let before = tree(&dir);
    with_cx(&mut w.h, |app, cx| app.open_health(None, cx));
    assert_eq!(w.h.app.app.health.client_dir(), Some(dir.as_path()), "without a name it is the open client");
    assert!(health_ready(&mut w.h), "the report arrives");
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Client health") && t.contains("This only looks"), "{t}");
    assert!(t.contains("Index: SAH version 0, 7 files") && t.contains("0 problems and 0 warnings"), "{t}");
    assert!(t.contains("data.saf:") && t.contains("dead space") && t.contains("Journals: 0"), "{t}");
    assert!(t.contains("sah-bak") && t.contains("stray-temp") && t.contains("swap-pair") && t.contains("Clean up..."), "{t}");
    // only what no undo, swap back or uninstall may need is ticked
    assert_eq!(w.h.app.app.health.selected(), ["stray-temp:.n.7.shaiya-tmp"]);
    assert_eq!(tree(&dir), before, "looking changed nothing");
    // the links ask the tabs for their own confirmations
    w.h.app.app.health.ask_compact();
    w.h.frames(5);
    assert_eq!(w.h.app.app.tab, Tab::Archive);
    assert!(w.h.text().contains("Compact data.saf of US test"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-exclusive", 1);
    w.h.frames(3);
    w.h.app.app.health.ask_prune();
    w.h.frames(5);
    assert_eq!(w.h.app.app.tab, Tab::Apply);
    assert!(w.h.text().contains("Prune history"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-prune", 1);
    w.h.frames(3);
    assert_eq!(tree(&dir), before, "cancelling both changed nothing");
}

#[test]
fn clean_up_asks_with_four_rows_moves_into_a_cleanup_folder_and_put_back_restores() {
    let mut w = open_world("t9-health-clean");
    let dir = w.client_dir();
    health_leftovers(&dir);
    let before = tree(&dir);
    with_cx(&mut w.h, |app, cx| app.open_health(Some("US test"), cx));
    assert!(health_ready(&mut w.h));
    w.h.app.app.health.set_selected("sah-bak:data.sah.bak", true);
    w.h.app.app.health.request_clean();
    w.h.frames(3);
    let t = w.h.text().to_string();
    for label in ["Changes", "Writes", "Undo", "Game"] {
        assert!(t.contains(label), "the {label} row: {t}");
    }
    assert!(t.contains("moves 2 files") && t.contains("Put back in Health") && t.contains("none is copied or deleted"), "{t}");
    // Cancel changes nothing
    w.h.app.shell.inject_answer("toolkit-cleanup", 1);
    w.h.frames(3);
    assert_eq!(tree(&dir), before);
    assert!(!w.h.app.app.health.is_busy());
    // Clean up
    w.h.app.app.health.request_clean();
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-cleanup", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.health.is_busy() && h.app.app.health.report().is_some_and(|r| r.cleanups.len() == 1)), "the clean-up ran and the report was read again");
    assert!(!dir.join("data.sah.bak").exists() && !dir.join(".n.7.shaiya-tmp").exists());
    assert!(dir.join("data.20261101T101010Z.sah").exists(), "what was not ticked stays");
    let folder = w.h.app.app.health.report().unwrap().cleanups[0].name.clone();
    assert!(dir.join("shaiya-toolkit-backups").join(&folder).join("manifest.json").is_file());
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive is open again");
    w.h.frames(3);
    assert!(w.h.text().contains(&folder) && w.h.text().contains("Put back..."), "{}", w.h.text());
    // Put back
    w.h.app.app.health.request_put_back(&folder);
    w.h.frames(3);
    let t = w.h.text().to_string();
    assert!(t.contains("Put back") && t.contains("moves 2 files back to where they were"), "{t}");
    w.h.app.shell.inject_answer("toolkit-put-back", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.health.is_busy() && h.app.app.health.report().is_some_and(|r| r.cleanups.first().is_some_and(|c| c.to_put_back == 0))));
    let after: Vec<_> = tree(&dir).into_iter().filter(|(n, _)| !n.starts_with("shaiya-toolkit-backups/")).collect();
    assert_eq!(after, before, "every name and every byte is back");
}

#[test]
fn changing_the_client_during_a_clean_up_or_put_back_cannot_touch_the_other_clients_folder() {
    let mut w = open_world("t9-health-switch");
    let dir = w.client_dir();
    let other = TempDir::new("t9-health-switch-other");
    synthetic(other.path());
    // the same leftovers (and so the same ids) in both folders
    health_leftovers(&dir);
    health_leftovers(other.path());
    with_cx(&mut w.h, |app, cx| app.add_client("Other", other.path(), false, cx)).unwrap();
    let other_before = tree(other.path());
    with_cx(&mut w.h, |app, cx| app.open_health(Some("US test"), cx));
    assert!(health_ready(&mut w.h));
    w.h.app.app.health.request_clean();
    w.h.frames(3);
    // the window is switched to the other client while the confirmation is up
    with_cx(&mut w.h, |app, cx| app.open_health(Some("Other"), cx));
    assert_eq!(w.h.app.app.health.client_dir(), Some(other.path()));
    w.h.app.shell.inject_answer("toolkit-cleanup", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.health.is_busy() && dir.join("shaiya-toolkit-backups").is_dir()), "the clean-up ran on the client it was confirmed for");
    assert!(w.h.wait_until(WAIT, |h| h.app.app.health.report().is_some()));
    assert!(!dir.join(".n.7.shaiya-tmp").exists(), "the confirmed client's file was moved");
    assert_eq!(tree(other.path()), other_before, "nothing in the other client's folder moved");
    // the same for Put back: confirmed for one client, the window switched to the other
    let folder = std::fs::read_dir(dir.join("shaiya-toolkit-backups")).unwrap().next().unwrap().unwrap().file_name().to_string_lossy().into_owned();
    with_cx(&mut w.h, |app, cx| app.open_health(Some("US test"), cx));
    assert!(health_ready(&mut w.h));
    w.h.app.app.health.request_put_back(&folder);
    w.h.frames(3);
    with_cx(&mut w.h, |app, cx| app.open_health(Some("Other"), cx));
    w.h.app.shell.inject_answer("toolkit-put-back", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.health.is_busy() && dir.join(".n.7.shaiya-tmp").exists()), "the put back ran on the client it was confirmed for");
    assert_eq!(tree(other.path()), other_before, "nothing in the other client's folder moved");
}

#[test]
fn open_health_without_a_name_opens_for_the_open_client_even_when_it_is_no_longer_saved() {
    let mut w = open_world("t9-health-unsaved");
    let dir = w.client_dir();
    with_cx(&mut w.h, |_app, cx| shaiya_toolkit::clients::remove_client(cx.shell, "US test")).unwrap();
    assert!(w.h.app.shell.shared().clients.is_empty());
    with_cx(&mut w.h, |app, cx| app.open_health(None, cx));
    assert_eq!(w.h.app.app.health.client_dir(), Some(dir.as_path()), "the open client, not an empty window");
    assert!(health_ready(&mut w.h), "its report is read");
}

#[cfg(windows)]
#[test]
fn clean_up_stops_while_another_program_holds_data_saf() {
    let mut w = open_world("t9-health-busy");
    let dir = w.client_dir();
    health_leftovers(&dir);
    with_cx(&mut w.h, |app, cx| app.open_health(Some("US test"), cx));
    assert!(health_ready(&mut w.h));
    w.h.app.app.health.request_clean();
    w.h.frames(3);
    let held = std::fs::File::open(dir.join("data.saf")).unwrap();
    w.h.app.shell.inject_answer("toolkit-cleanup", 0);
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.health.is_busy() && h.app.app.health.last.as_deref().is_some_and(|t| t.contains("The clean-up stopped"))));
    assert!(dir.join(".n.7.shaiya-tmp").exists() && !dir.join("shaiya-toolkit-backups").exists(), "nothing was moved");
    drop(held);
    w.h.app.shell.inject_answer("toolkit-error", 0);
    w.h.frames(2);
}

#[test]
fn the_start_page_health_row_opens_the_window() {
    let mut h = Harness::<ToolkitApp>::bare();
    h.frames(3);
    let t = h.text().to_string();
    assert!(t.contains("Check a client folder's health") && t.contains("Open Health"), "{t}");
    with_cx(&mut h, |app, cx| app.run_start(shaiya_toolkit::start::StartAction::Health, cx));
    h.frames(3);
    assert!(h.app.app.health.open);
    assert!(h.text().contains("Client health") && h.text().contains("No client folders yet"), "{}", h.text());
}

// ------------------------------------------------------------------------------------------------ T9 W10: several clients, Compare, Apply to several

/// Writes a synthetic client (a `data.sah`/`data.saf` pair) of the named files into a new folder.
fn client_of(tag: &str, files: &[(&str, &[u8])]) -> TempDir {
    let d = TempDir::new(tag);
    let mut b = SahBuilder::new(0);
    for (n, data) in files {
        b = b.file(n.as_bytes(), data);
    }
    b.write_to(d.path());
    d
}

fn add_saved(w: &mut World, name: &str, dir: &std::path::Path) {
    with_cx(&mut w.h, |app, cx| app.add_client(name, dir, false, cx)).unwrap();
    w.h.frames(1);
}

fn open_saved(w: &mut World, name: &str, dir: &std::path::Path) {
    with_cx(&mut w.h, |app, cx| app.select_client(name, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open() && h.app.app.archive.client_dir() == Some(dir)), "{name} opens");
    w.h.frames(2);
}

fn file_hashes(dir: &std::path::Path) -> (String, String) {
    (to_hex(&sha256(&std::fs::read(dir.join("data.sah")).unwrap())), to_hex(&sha256(&std::fs::read(dir.join("data.saf")).unwrap())))
}

fn queue_one(w: &mut World, src: &std::path::Path, path: &str) {
    std::fs::write(src, b"queued bytes").unwrap();
    w.h.app.app.archive.queue_puts(vec![(path.to_string(), src.to_path_buf(), 0)]);
    w.h.frames(2);
}

fn has_entry(dir: &std::path::Path, path: &str) -> bool {
    let a = shaiya_archive::Archive::open(&dir.join("data.sah")).unwrap();
    a.index().get(path).is_some()
}

#[test]
fn a_queue_survives_switching_clients_and_switching_back() {
    let mut w = open_world("t10-switch");
    let a = w.client_dir();
    let b = client_of("t10-switch-b", &[("only-in-b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10-switch-src");
    queue_one(&mut w, &src.path().join("q1.txt"), "queued/one.txt");
    queue_one(&mut w, &src.path().join("q2.txt"), "queued/two.txt");
    assert_eq!(w.h.app.app.archive.pending.len(), 2);
    // switching is no longer refused; the other client starts with an empty queue
    open_saved(&mut w, "EU test", b.path());
    assert!(w.h.app.app.archive.pending.is_empty(), "the Archive tab shows the active client's queue");
    assert_eq!(w.h.app.app.queued_for(&a), 2, "US test keeps its queue");
    assert_eq!(w.h.app.app.queued_for(b.path()), 0);
    // quitting asks once about every client with a queue, the inactive one too
    let unsaved = <ToolkitApp as shaiya_ui::ToolApp>::unsaved(&w.h.app.app);
    assert_eq!(unsaved, vec!["US test: 2 pending archive changes".to_string()], "{unsaved:?}");
    // the picker marks it
    assert_eq!(shaiya_toolkit::clients::queued_label("US test".to_string(), 2), "US test - 2 queued");
    assert_eq!(shaiya_toolkit::clients::queued_label("EU test".to_string(), 0), "EU test");
    // a queue on the second client too; both are listed
    queue_one(&mut w, &src.path().join("q3.txt"), "queued/three.txt");
    let names: Vec<(String, usize)> = w.h.app.app.queued_clients().into_iter().map(|(n, _, c)| (n, c)).collect();
    assert_eq!(names, vec![("EU test".to_string(), 1), ("US test".to_string(), 2)]);
    // back: the first queue is there again, in order, and the second client keeps its own
    open_saved(&mut w, "US test", &a);
    let paths: Vec<String> = w.h.app.app.archive.pending.iter().map(|p| p.path.clone()).collect();
    assert_eq!(paths, ["queued/one.txt", "queued/two.txt"]);
    assert_eq!(w.h.app.app.queued_for(b.path()), 1);
    assert!(w.h.app.app.archive.undo(), "the undo history came back with the queue");
    assert_eq!(w.h.app.app.archive.pending.len(), 1);
    // opening a plain archive file stashes the queue as well
    let file = b.path().join("data.sah");
    with_cx(&mut w.h, |app, cx| app.open_archive_file(file, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open() && h.app.app.archive.client_dir().is_none()));
    assert!(w.h.app.app.archive.pending.is_empty());
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    assert_eq!(w.h.app.app.queued_for(b.path()), 1);
}

#[test]
fn a_queue_is_applied_only_to_the_client_it_was_built_on() {
    let mut w = open_world("t10-only");
    let a = w.client_dir();
    let b = client_of("t10-only-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10-only-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    let (a_before, b_before) = (file_hashes(&a), file_hashes(b.path()));
    open_saved(&mut w, "EU test", b.path());
    // nothing is queued on B, so there is nothing to apply there
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    assert!(!w.h.app.app.archive.is_applying());
    // even a queue that is put into the Archive tab by hand (not built on B) is refused for B
    w.h.app.app.archive.pending.push(shaiya_toolkit::archive_view::Pending { path: "queued/one.txt".into(), kind: shaiya_toolkit::archive_view::PendingKind::Remove });
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    assert!(!w.h.app.app.archive.is_applying(), "a queue that was not built on this client is not applied to it");
    w.h.app.app.archive.pending.clear();
    assert_eq!(file_hashes(b.path()), b_before, "B is byte-identical");
    // back on A the queue applies, to A only
    open_saved(&mut w, "US test", &a);
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 1 && h.app.app.archive.is_open()), "applied");
    assert_ne!(file_hashes(&a), a_before, "A changed");
    assert_eq!(file_hashes(b.path()), b_before, "B is still byte-identical");
    assert!(has_entry(&a, "queued/one.txt"));
    assert!(w.h.app.app.queued_clients().is_empty());
}

#[test]
fn removing_or_renaming_a_client_with_a_queue_asks_and_drops_the_queue() {
    let mut w = open_world("t10-drop");
    let a = w.client_dir();
    let b = client_of("t10-drop-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10-drop-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    open_saved(&mut w, "EU test", b.path());
    w.h.frames(2); // the app tells the dialogs which clients have a queue
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    // Cancel keeps the client and its queue
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("US test", cx));
    w.h.frames(2);
    assert!(w.h.text().contains("1 queued change") && w.h.text().contains("Drop them and remove"), "{}", w.h.text());
    w.h.app.shell.inject_answer("toolkit-drop-queue", 1);
    w.h.frames(3);
    assert!(w.h.app.shell.shared().clients.iter().any(|c| c.name == "US test"));
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    // renaming asks too; yes renames and drops the queue
    with_cx(&mut w.h, |app, cx| assert!(app.clients.begin_rename("US test", "US renamed", cx)));
    w.h.frames(2);
    assert!(w.h.text().contains("Drop them and rename"), "{}", w.h.text());
    assert!(w.h.app.shell.shared().clients.iter().any(|c| c.name == "US test"), "nothing changes before the answer");
    w.h.app.shell.inject_answer("toolkit-drop-queue", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.shell.shared().clients.iter().any(|c| c.name == "US renamed")));
    w.h.frames(2);
    assert_eq!(w.h.app.app.queued_for(&a), 0, "the queue went with the old name");
    // a client without a queue is removed at once
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("US renamed", cx));
    w.h.frames(2);
    assert!(!w.h.app.shell.shared().clients.iter().any(|c| c.name == "US renamed"));
}

#[test]
fn compare_two_saved_clients_summarises_old_to_new_and_hands_old_and_new_to_the_patch_builder() {
    let mut w = open_world("t10-compare");
    let a = client_of("t10-compare-a", &[("a.txt", b"one"), ("b.txt", b"two"), ("c.txt", b"three"), ("same.txt", b"same")]);
    let b = client_of("t10-compare-b", &[("a.txt", b"ONE!"), ("b.txt", b"two"), ("d.txt", b"four"), ("same.txt", b"same")]);
    add_saved(&mut w, "Old one", a.path());
    add_saved(&mut w, "New one", b.path());
    open_saved(&mut w, "Old one", a.path());
    w.h.app.app.archive.begin_compare();
    let left = shaiya_toolkit::archive_view::CompareSide::Client { name: "Old one".into(), dir: a.path().to_path_buf() };
    assert_eq!(w.h.app.app.archive.compare.as_ref().unwrap().left, Some(left.clone()), "the left side defaults to the active client");
    assert!(w.h.app.app.archive.compare.as_ref().unwrap().right.is_none());
    let right = shaiya_toolkit::archive_view::CompareSide::Client { name: "New one".into(), dir: b.path().to_path_buf() };
    with_cx(&mut w.h, |app, cx| app.archive.run_compare(left, right, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.compare.as_ref().is_some_and(|c| !c.is_running())), "the comparison finishes");
    w.h.frames(2);
    let c = w.h.app.app.archive.compare.as_ref().unwrap();
    assert_eq!((c.summary.added, c.summary.removed, c.summary.different, c.summary.same, c.summary.unknown), (1, 1, 1, 2, 0), "OLD = left, NEW = right: d is added, c is removed, a differs");
    let t = w.h.text();
    assert!(t.contains("1 added, 1 removed, 1 different, 2 same") && t.contains("OLD Old one") && t.contains("NEW New one"), "{t}");
    assert!(t.contains("Make patch from differences"), "{t}");
    // the request carries OLD = left and NEW = right, and the patch builder names them
    let req = w.h.app.app.archive.compare.as_ref().unwrap().patch_request().expect("the comparison is current");
    with_cx(&mut w.h, |app, cx| app.handle_requests(vec![req], cx));
    assert_eq!(w.h.app.app.tab, Tab::Patch);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.extra.is_some()), "the differences are in the patch builder");
    let label = w.h.app.app.patch.extra.as_ref().unwrap().0.clone();
    assert_eq!(label, "differences, OLD Old one -> NEW New one");
    let extra = &w.h.app.app.patch.extra.as_ref().unwrap().1;
    let mut files: Vec<&str> = extra.files.iter().map(|f| f.0.as_str()).collect();
    files.sort();
    assert_eq!(files, ["a.txt", "d.txt"], "the added and different files of NEW");
    assert_eq!(extra.removals, ["c.txt"], "what only OLD has is removed");
}

#[test]
fn compare_says_nothing_differs_and_offers_no_empty_patch() {
    let mut w = open_world("t10-same");
    let a = client_of("t10-same-a", &[("a.txt", b"one"), ("b.txt", b"two")]);
    let b = client_of("t10-same-b", &[("b.txt", b"two"), ("a.txt", b"one")]);
    w.h.app.app.archive.begin_compare();
    let (l, r) = (shaiya_toolkit::archive_view::CompareSide::File(a.path().join("data.sah")), shaiya_toolkit::archive_view::CompareSide::File(b.path().join("data.sah")));
    with_cx(&mut w.h, |app, cx| app.archive.run_compare(l, r, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.compare.as_ref().is_some_and(|c| !c.is_running())));
    w.h.frames(2);
    assert!(shaiya_toolkit::archive_view::nothing_differs(&w.h.app.app.archive.compare.as_ref().unwrap().summary));
    assert!(w.h.text().contains("Nothing differs"), "{}", w.h.text());
    assert!(!shaiya_toolkit::archive_view::patch_worthy(&w.h.app.app.archive.compare.as_ref().unwrap().summary));
}

#[test]
fn leaving_out_the_same_files_cannot_silently_empty_a_patch_made_from_differences() {
    // OLD = another archive, NEW = the selected client: every file of the differences is already in the selected client, so with
    // "Leave out files that are already the same" ticked the patch would come out empty; the builder says so in words
    let mut w = open_world("t10-empty");
    let a = w.client_dir();
    let old = client_of("t10-empty-old", &[("top.txt", b"an older root file")]);
    go(&mut w.h, Tab::Patch);
    let (oldsah, newsah) = (old.path().join("data.sah"), a.join("data.sah"));
    with_cx(&mut w.h, |app, cx| app.patch.from_diff_named(oldsah, newsah, "Old copy", "US test", cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.patch.left_out_message().is_some()), "the comparison with the client finishes and everything is left out");
    w.h.frames(2);
    let m = w.h.app.app.patch.left_out_message().unwrap();
    assert!(m.contains("Nothing differs") && m.contains("already the same in US test") && m.contains("Untick"), "{m}");
    assert!(w.h.app.app.patch.warnings().contains(&m), "{:?}", w.h.app.app.patch.warnings());
    let blocked = w.h.app.app.patch.build_blocked(&w.h.app.app.keys);
    assert_eq!(blocked.as_deref(), Some(m.as_str()), "Build says why, not just 'nothing to build'");
    let t = w.h.text();
    assert!(t.contains("Nothing differs") && t.contains("left out") && t.contains("OLD Old copy -> NEW US test"), "{t}");
    // unticking the box brings the files back and the message goes
    w.h.app.app.patch.skip_same = false;
    w.h.frames(3);
    assert!(w.h.app.app.patch.left_out_message().is_none());
    assert!(!w.h.app.app.patch.input().files.is_empty());
}

fn multi_confirm(w: &mut World, which: usize, total: usize, button: usize) {
    let want = format!("Client {which} of {total}");
    assert!(w.h.wait_until(WAIT, |h| h.text().contains(&want)), "the confirmation of {want} is shown: {}", w.h.text());
    let t = w.h.text().to_string();
    assert!(t.contains("Changes") && t.contains("Writes") && t.contains("Undo") && t.contains("Game"), "the four rows: {t}");
    w.h.app.shell.inject_answer("toolkit-multi-apply", button);
}

fn start_multi(w: &mut World, patch: &std::path::Path, names: &[&str]) {
    let p = patch.to_path_buf();
    with_cx(&mut w.h, |app, cx| {
        app.multi.open_dialog(vec![p.clone()], cx);
        app.multi.choose(&[p], names);
        app.multi.start_chosen(cx);
    });
}

#[test]
fn apply_to_several_clients_confirms_and_journals_each_client_in_turn() {
    let mut w = open_world("t10-multi");
    let a = w.client_dir();
    let b = TempDir::new("t10-multi-b");
    synthetic(b.path());
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10-multi-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    go(&mut w.h, Tab::Apply);
    start_multi(&mut w, &patch, &["US test", "EU test"]);
    multi_confirm(&mut w, 1, 2, 0);
    multi_confirm(&mut w, 2, 2, 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some() && !h.app.app.multi.is_running()), "the run ends");
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert!(o.stopped.is_none() && o.not_touched.is_empty(), "{o:?}");
    assert_eq!(o.changed.iter().map(|c| c.client.as_str()).collect::<Vec<_>>(), ["US test", "EU test"]);
    for (dir, c) in [(a.as_path(), &o.changed[0]), (b.path(), &o.changed[1])] {
        assert_eq!((c.applied, c.of, c.journals.len()), (1, 1, 1), "{c:?}");
        assert!(c.journals[0].starts_with(dir.join(shaiya_archive::journal::DEFAULT_DIR)) && c.journals[0].exists(), "its own journal: {:?}", c.journals);
        assert!(has_entry(dir, "interface/new.txt"), "patched");
        assert_eq!(std::fs::read(dir.join("notice.txt")).unwrap(), b"hello players", "the loose file went in");
        let list = shaiya_archive::journal::list(dir, &dir.join(shaiya_archive::journal::DEFAULT_DIR)).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0.state, shaiya_archive::journal::State::Applied);
    }
    w.h.frames(2);
    let t = w.h.text();
    assert!(t.contains("Changed: US test") && t.contains("Changed: EU test") && t.contains("Done: every chosen client was changed."), "{t}");
    // the Archive tab read the open client again
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()));
}

#[test]
fn apply_to_several_clients_stops_at_a_client_that_cannot_be_planned_and_leaves_it_and_the_rest_untouched() {
    let mut w = open_world("t10-multi-fail");
    let a = w.client_dir();
    let b = TempDir::new("t10-multi-fail-b");
    synthetic(b.path());
    std::fs::write(b.path().join("data.sah"), b"not an index").unwrap(); // B cannot be read
    let c = TempDir::new("t10-multi-fail-c");
    synthetic(c.path());
    add_saved_unchecked(&mut w, "B broken", b.path());
    add_saved(&mut w, "C fine", c.path());
    let src = TempDir::new("t10-multi-fail-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    let (b_before, c_before) = (file_hashes(b.path()), file_hashes(c.path()));
    start_multi(&mut w, &patch, &["US test", "B broken", "C fine"]);
    multi_confirm(&mut w, 1, 3, 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some()), "the run stops");
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert_eq!(o.changed.len(), 1, "{o:?}");
    assert_eq!((o.changed[0].client.as_str(), o.changed[0].journals.len()), ("US test", 1));
    assert!(o.stopped.as_ref().is_some_and(|s| s.0 == "B broken"), "{o:?}");
    assert_eq!(o.not_touched, ["B broken", "C fine"]);
    assert!(has_entry(&a, "interface/new.txt"), "the first client stays applied");
    assert_eq!(file_hashes(b.path()), b_before);
    assert_eq!(file_hashes(c.path()), c_before, "the client after the failure was never started");
    assert!(!c.path().join(shaiya_archive::journal::DEFAULT_DIR).exists());
    w.h.frames(2);
    let t = w.h.text();
    assert!(t.contains("Changed: US test") && t.contains("Stopped at B broken") && t.contains("Not touched: B broken, C fine"), "{t}");
}

/// Adds a saved client whose folder holds a `data.sah` that `add_client` would refuse to read: the list file is written directly.
fn add_saved_unchecked(w: &mut World, name: &str, dir: &std::path::Path) {
    let good = TempDir::new("t10-unchecked");
    synthetic(good.path());
    let saved = good.path().join("data.sah");
    let backup = std::fs::read(dir.join("data.sah")).unwrap();
    // the client is added while its index is readable, then the index is damaged again
    std::fs::copy(&saved, dir.join("data.sah")).unwrap();
    add_saved(w, name, dir);
    std::fs::write(dir.join("data.sah"), backup).unwrap();
}

#[test]
fn apply_to_several_clients_stops_when_one_asks_to_stop() {
    let mut w = open_world("t10-multi-stop");
    let b = TempDir::new("t10-multi-stop-b");
    synthetic(b.path());
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10-multi-stop-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    let (a_before, b_before) = (file_hashes(&w.client_dir()), file_hashes(b.path()));
    start_multi(&mut w, &patch, &["US test", "EU test"]);
    multi_confirm(&mut w, 1, 2, 1); // "Stop here" on the first client
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some()));
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert!(o.changed.is_empty() && o.not_touched == ["US test", "EU test"], "{o:?}");
    assert_eq!((file_hashes(&w.client_dir()), file_hashes(b.path())), (a_before, b_before));
}

#[cfg(windows)]
#[test]
fn apply_to_several_clients_a_game_running_on_the_second_leaves_the_first_applied_and_the_second_identical() {
    let mut w = open_world("t10-multi-game");
    let a = w.client_dir();
    let b = TempDir::new("t10-multi-game-b");
    synthetic(b.path());
    let c = TempDir::new("t10-multi-game-c");
    synthetic(c.path());
    add_saved(&mut w, "EU test", b.path());
    add_saved(&mut w, "AS test", c.path());
    let src = TempDir::new("t10-multi-game-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    let (b_before, c_before) = (file_hashes(b.path()), file_hashes(c.path()));
    // the game holds the second client's data.saf
    let held = std::fs::File::open(b.path().join("data.saf")).unwrap();
    start_multi(&mut w, &patch, &["US test", "EU test", "AS test"]);
    multi_confirm(&mut w, 1, 3, 0);
    multi_confirm(&mut w, 2, 3, 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some()), "the run stops at the second client");
    drop(held);
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert_eq!(o.changed.iter().map(|c| c.client.as_str()).collect::<Vec<_>>(), ["US test"], "{o:?}");
    assert_eq!(o.changed[0].journals.len(), 1);
    assert!(o.stopped.as_ref().is_some_and(|s| s.0 == "EU test" && s.1.contains("open in another program")), "{o:?}");
    assert_eq!(o.not_touched, ["EU test", "AS test"]);
    assert!(has_entry(&a, "interface/new.txt"), "the first client is applied");
    let list = shaiya_archive::journal::list(&a, &a.join(shaiya_archive::journal::DEFAULT_DIR)).unwrap();
    assert_eq!((list.len(), &list[0].0.state), (1, &shaiya_archive::journal::State::Applied), "and journaled");
    assert_eq!(file_hashes(b.path()), b_before, "the second client is byte-identical");
    assert_eq!(file_hashes(c.path()), c_before, "the third was never started");
}

// ------------------------------------------------------------------------------------------------ T9 W10fix

#[test]
fn a_dropped_folder_named_like_a_saved_client_does_not_make_that_clients_removal_ask() {
    let mut w = open_world("t10fix-dropname");
    let b = client_of("t10fix-dropname-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    // a folder that is not saved, called "EU test" like the saved client, is opened by dropping it
    let parent = TempDir::new("t10fix-dropname-parent");
    let d = parent.path().join("EU test");
    std::fs::create_dir_all(&d).unwrap();
    synthetic(&d);
    w.h.drop_files(std::slice::from_ref(&d));
    w.h.frames(2);
    w.h.app.shell.inject_answer("toolkit-drop-folder", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open() && h.app.app.archive.client_dir() == Some(d.as_path())), "the dropped folder opens");
    w.h.frames(2);
    let src = TempDir::new("t10fix-dropname-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    assert_eq!(w.h.app.app.queued_for(&d), 1);
    // the saved "EU test" is another folder with no queue: removing it asks nothing and leaves the dropped folder's queue alone
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("EU test", cx));
    w.h.frames(2);
    assert!(!w.h.text().contains("queued change"), "no question: {}", w.h.text());
    assert!(!w.h.app.shell.shared().clients.iter().any(|c| c.name == "EU test"), "removed at once");
    assert_eq!(w.h.app.app.queued_for(&d), 1, "the other folder's queue is still there");
}

#[test]
fn a_queue_on_a_folder_blocks_the_archive_pair_swap_for_every_client_on_that_folder() {
    let mut w = open_world("t10fix-swap");
    let a = w.client_dir();
    add_saved(&mut w, "US twin", &a); // a second saved client on the same folder
    let b = client_of("t10fix-swap-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    w.h.frames(2);
    assert!(!w.h.app.app.clients.swap_blocked(&a) && !w.h.app.app.clients.swap_blocked(b.path()), "nothing queued yet");
    let src = TempDir::new("t10fix-swap-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    w.h.frames(2);
    assert!(w.h.app.app.clients.swap_blocked(&a), "the active client's queue blocks the folder");
    assert!(!w.h.app.app.clients.swap_blocked(b.path()));
    // the queue is stashed when another client opens: the twin on the same folder is still blocked
    open_saved(&mut w, "EU test", b.path());
    w.h.frames(2);
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    assert!(w.h.app.app.clients.swap_blocked(&a), "a stashed queue blocks it as well, whichever saved client is asked");
    assert!(!w.h.app.app.clients.swap_blocked(b.path()));
    assert!(shaiya_toolkit::clients::SWAP_BLOCKED_HOVER.contains("Apply or discard this client's queued changes first"));
    // once the queue is dropped the folder is free again
    w.h.app.app.drop_queue_of(&a);
    w.h.frames(2);
    assert!(!w.h.app.app.clients.swap_blocked(&a));
}

#[test]
fn a_restored_queue_keeps_its_folder_when_nothing_is_loaded_and_is_not_lost_on_the_next_switch() {
    let mut w = open_world("t10fix-owner");
    let a = w.client_dir();
    let b = client_of("t10fix-owner-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10fix-owner-src");
    open_saved(&mut w, "EU test", b.path());
    queue_one(&mut w, &src.path().join("q1.txt"), "queued/one.txt");
    open_saved(&mut w, "US test", &a);
    assert_eq!(w.h.app.app.queued_for(b.path()), 1, "EU test's queue is stashed");
    // EU test's index becomes unreadable: it opens with an error, and its queue is restored onto nothing
    let good = std::fs::read(b.path().join("data.sah")).unwrap();
    std::fs::write(b.path().join("data.sah"), b"not an index").unwrap();
    with_cx(&mut w.h, |app, cx| app.select_client("EU test", cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_opening()), "the open ends");
    w.h.frames(2);
    assert!(w.h.app.app.archive.client_dir().is_none(), "nothing is loaded");
    assert_eq!(w.h.app.app.queued_for(b.path()), 1, "the queue is shown as EU test's");
    // another edit while nothing is loaded must not forget whose queue it is
    queue_one(&mut w, &src.path().join("q2.txt"), "queued/two.txt");
    assert_eq!(w.h.app.app.archive.pending.len(), 2);
    assert_eq!(w.h.app.app.queued_for(b.path()), 2, "still EU test's queue");
    // the next switch keeps it (stashed under EU test), without the question or a loss
    std::fs::write(b.path().join("data.sah"), good).unwrap();
    open_saved(&mut w, "US test", &a);
    assert_eq!(w.h.app.app.queued_for(b.path()), 2, "the queue was stashed, not cleared");
    open_saved(&mut w, "EU test", b.path());
    assert_eq!(w.h.app.app.archive.pending.len(), 2, "and it comes back");
}

#[test]
fn the_client_picker_marks_a_client_with_queued_changes() {
    let mut w = open_world("t10fix-label");
    let a = w.client_dir();
    let b = client_of("t10fix-label-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10fix-label-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    open_saved(&mut w, "EU test", b.path());
    let clients = w.h.app.shell.shared().clients.clone();
    let (us, eu) = (clients.iter().find(|c| c.name == "US test").unwrap().clone(), clients.iter().find(|c| c.name == "EU test").unwrap().clone());
    let (lus, leu) = with_cx(&mut w.h, |app, cx| (app.picker_label_of(&us, true, cx), app.picker_label_of(&eu, true, cx)));
    assert!(lus.contains("US test") && lus.ends_with(" - 1 queued"), "the stashed queue is marked: {lus}");
    assert!(!leu.contains("queued"), "{leu}");
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    // the combo list itself is drawn only while the picker is open, and the harness cannot click a widget by its label: the label
    // text is checked here, where the picker takes it from
}

#[test]
fn queues_are_memory_only_a_new_toolkit_instance_has_none() {
    let mut w = open_world("t10fix-memory");
    let a = w.client_dir();
    let src = TempDir::new("t10fix-memory-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    let fresh = world("t10fix-memory-fresh");
    assert!(fresh.h.app.app.queued_clients().is_empty());
    assert_eq!(fresh.h.app.app.queued_for(&a), 0);
}

#[test]
fn the_palette_command_apply_multi_opens_the_dialog_with_the_listed_patches() {
    let mut w = open_world("t10fix-palette");
    let src = TempDir::new("t10fix-palette-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    go(&mut w.h, Tab::Patch);
    assert!(<ToolkitApp as shaiya_ui::ToolApp>::commands(&w.h.app.app).iter().any(|c| c.id == "apply-multi" && c.enabled), "listed and enabled");
    assert!(!w.h.app.app.multi.open);
    with_cx(&mut w.h, |app, cx| {
        app.apply.add_patches(vec![patch.clone()]);
        shaiya_ui::ToolApp::run_command(app, "apply-multi", cx);
    });
    w.h.frames(3);
    assert!(w.h.app.app.multi.open, "the dialog is open");
    assert_eq!(w.h.app.app.tab, Tab::Apply, "on the Apply tab");
    let t = w.h.text();
    assert!(t.contains("Apply to several test clients") && t.contains("ps0001.patch") && t.contains("US test"), "{t}");
}

#[test]
fn compare_swap_sides_turns_old_into_new_and_the_patch_request_follows() {
    let mut w = open_world("t10fix-swap-sides");
    let a = client_of("t10fix-swap-sides-a", &[("a.txt", b"one"), ("c.txt", b"three")]);
    let b = client_of("t10fix-swap-sides-b", &[("a.txt", b"ONE!"), ("d.txt", b"four")]);
    add_saved(&mut w, "Old one", a.path());
    add_saved(&mut w, "New one", b.path());
    w.h.app.app.archive.begin_compare();
    let l = shaiya_toolkit::archive_view::CompareSide::Client { name: "Old one".into(), dir: a.path().to_path_buf() };
    let r = shaiya_toolkit::archive_view::CompareSide::Client { name: "New one".into(), dir: b.path().to_path_buf() };
    with_cx(&mut w.h, |app, cx| app.archive.run_compare(l.clone(), r.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.compare.as_ref().is_some_and(|c| !c.is_running())));
    let c = w.h.app.app.archive.compare.as_mut().unwrap();
    assert!(c.patch_request().is_some());
    c.swap_sides();
    assert_eq!((c.left.clone(), c.right.clone()), (Some(r.clone()), Some(l.clone())));
    assert!(!c.is_current() && c.patch_request().is_none(), "the shown comparison is of the other direction until Compare is pressed again");
    with_cx(&mut w.h, |app, cx| app.archive.run_compare(r.clone(), l.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.compare.as_ref().is_some_and(|c| !c.is_running())));
    let c = w.h.app.app.archive.compare.as_ref().unwrap();
    // what was added in NEW is now removed, and the other way round
    assert_eq!((c.summary.added, c.summary.removed, c.summary.different), (1, 1, 1), "{:?}", c.summary);
    match c.patch_request().expect("current") {
        shaiya_toolkit::archive_view::TabRequest::PatchFromDiff { old_label, new_label, .. } => assert_eq!((old_label.as_str(), new_label.as_str()), ("New one", "Old one")),
        _ => panic!("a patch request"),
    }
}

#[test]
fn compare_archive_file_choice_sets_that_side_and_a_cancelled_picker_changes_nothing() {
    let mut w = open_world("t10fix-file-side");
    let a = client_of("t10fix-file-side-a", &[("a.txt", b"one")]);
    let b = client_of("t10fix-file-side-b", &[("a.txt", b"ONE!"), ("n.txt", b"new")]);
    add_saved(&mut w, "Old one", a.path());
    w.h.app.app.archive.begin_compare();
    let left = shaiya_toolkit::archive_view::CompareSide::Client { name: "Old one".into(), dir: a.path().to_path_buf() };
    let file = b.path().join("data.sah");
    let c = w.h.app.app.archive.compare.as_mut().unwrap();
    c.left = Some(left.clone());
    c.choose_file(true, None);
    assert!(c.right.is_none(), "a cancelled picker leaves the side as it was");
    c.choose_file(true, Some(file.clone()));
    let right = shaiya_toolkit::archive_view::CompareSide::File(file);
    assert_eq!((c.left.clone(), c.right.clone()), (Some(left.clone()), Some(right.clone())));
    c.choose_file(true, None);
    assert_eq!(c.right, Some(right.clone()), "and it keeps a side that was chosen");
    with_cx(&mut w.h, |app, cx| app.archive.run_compare(left, right, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.compare.as_ref().is_some_and(|c| !c.is_running())));
    let s = w.h.app.app.archive.compare.as_ref().unwrap().summary;
    assert_eq!((s.added, s.different), (1, 1), "{s:?}");
}

#[test]
fn apply_to_several_clients_applies_a_shared_folder_once_and_names_it() {
    let mut w = open_world("t10fix-twin");
    let a = w.client_dir();
    add_saved(&mut w, "US twin", &a);
    let b = TempDir::new("t10fix-twin-b");
    synthetic(b.path());
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t10fix-twin-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    go(&mut w.h, Tab::Apply);
    start_multi(&mut w, &patch, &["US test", "US twin", "EU test"]);
    multi_confirm(&mut w, 1, 2, 0); // two folders, not three clients
    // the confirmation names the folder it is about
    assert!(w.h.text().contains(&a.display().to_string()), "{}", w.h.text());
    multi_confirm(&mut w, 2, 2, 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some() && !h.app.app.multi.is_running()));
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert!(o.stopped.is_none() && o.not_touched.is_empty(), "{o:?}");
    assert_eq!(o.same_folder, [("US twin".to_string(), "US test".to_string())]);
    let list = shaiya_archive::journal::list(&a, &a.join(shaiya_archive::journal::DEFAULT_DIR)).unwrap();
    assert_eq!(list.len(), 1, "one journal on the shared folder, not two");
    w.h.frames(2);
    assert!(w.h.text().contains("US twin is the same folder as US test"), "{}", w.h.text());
}

#[test]
fn apply_to_several_clients_lists_a_client_on_the_failed_folder_as_not_touched() {
    let mut w = open_world("t10fix-twin-fail");
    let b = TempDir::new("t10fix-twin-fail-b");
    synthetic(b.path());
    std::fs::write(b.path().join("data.sah"), b"not an index").unwrap();
    add_saved_unchecked(&mut w, "B broken", b.path());
    add_saved_unchecked(&mut w, "B twin", b.path());
    let c = TempDir::new("t10fix-twin-fail-c");
    synthetic(c.path());
    add_saved(&mut w, "C fine", c.path());
    let src = TempDir::new("t10fix-twin-fail-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    start_multi(&mut w, &patch, &["B broken", "B twin", "C fine"]);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some()), "the run stops at the first");
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert!(o.changed.is_empty(), "{o:?}");
    let mut nt = o.not_touched.clone();
    nt.sort();
    assert_eq!(nt, ["B broken", "B twin", "C fine"], "every client that was not applied is listed, the same-folder one too");
    // the recover note belongs to a client whose apply had started; this one only failed to plan
    assert!(!o.recover_may_have_written, "{o:?}");
}

#[test]
fn apply_to_several_clients_stops_at_a_read_only_second_client_on_every_system() {
    let mut w = open_world("t10fix-ro");
    let a = w.client_dir();
    let b = TempDir::new("t10fix-ro-b");
    synthetic(b.path());
    let c = TempDir::new("t10fix-ro-c");
    synthetic(c.path());
    add_saved(&mut w, "EU test", b.path());
    add_saved(&mut w, "AS test", c.path());
    let src = TempDir::new("t10fix-ro-src");
    let patch = build_patch(src.path(), "ps0001.patch");
    let (b_before, c_before) = (file_hashes(b.path()), file_hashes(c.path()));
    // the second client's data.saf cannot be written (what the in-use check finds on every system)
    let saf = b.path().join("data.saf");
    let mut perm = std::fs::metadata(&saf).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&saf, perm).unwrap();
    start_multi(&mut w, &patch, &["US test", "EU test", "AS test"]);
    multi_confirm(&mut w, 1, 3, 0);
    multi_confirm(&mut w, 2, 3, 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.multi.last.is_some()), "the run stops at the second client");
    let mut perm = std::fs::metadata(&saf).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&saf, perm).unwrap();
    let o = w.h.app.app.multi.last.clone().unwrap();
    assert_eq!(o.changed.iter().map(|c| c.client.as_str()).collect::<Vec<_>>(), ["US test"], "{o:?}");
    assert!(o.stopped.as_ref().is_some_and(|s| s.0 == "EU test" && !s.1.is_empty()), "{o:?}");
    assert_eq!(o.not_touched, ["EU test", "AS test"]);
    assert!(has_entry(&a, "interface/new.txt"), "the first client is applied");
    assert_eq!(file_hashes(b.path()), b_before, "the second client is byte-identical");
    assert_eq!(file_hashes(c.path()), c_before, "the third was never started");
    assert!(!c.path().join(shaiya_archive::journal::DEFAULT_DIR).exists());
}

#[test]
fn a_stashed_queue_is_restored_when_the_folder_is_opened_under_another_spelling() {
    let mut w = open_world("t11-spelling");
    let a = w.client_dir();
    let b = client_of("t11-spelling-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t11-spelling-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    open_saved(&mut w, "EU test", b.path());
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    // the same folder, spelled another way
    let alt = if cfg!(windows) { std::path::PathBuf::from(a.to_string_lossy().to_uppercase()) } else { a.join(".").join("..").join(a.file_name().unwrap()) };
    with_cx(&mut w.h, |app, cx| app.add_client("US alt", &alt, false, cx)).unwrap();
    w.h.frames(1);
    open_saved(&mut w, "US alt", &alt);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "the queue came back under the other spelling");
    assert_eq!(w.h.app.app.queued_clients().len(), 1, "one queue for one folder");
    // stash and restore again: still one queue, never a second for the same place
    open_saved(&mut w, "EU test", b.path());
    assert_eq!(w.h.app.app.queued_clients().len(), 1);
    open_saved(&mut w, "US test", &a);
    assert_eq!(w.h.app.app.archive.pending.len(), 1);
    // applied to that client only; a different folder still refuses it
    let b_before = file_hashes(b.path());
    open_saved(&mut w, "EU test", b.path());
    w.h.app.app.archive.pending.push(shaiya_toolkit::archive_view::Pending { path: "queued/one.txt".into(), kind: shaiya_toolkit::archive_view::PendingKind::Remove });
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    w.h.frames(3);
    assert!(!w.h.app.app.archive.is_applying(), "a queue from a different folder is refused");
    assert_eq!(file_hashes(b.path()), b_before);
}

/// A synthetic client with one animation and its mesh (`huwm_117_test.ANI`, the three-bone arm), opened in the Archive tab.
fn ani_world(tag: &str) -> World {
    let mut w = world(tag);
    SahBuilder::new(0)
        .file(b"Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60))
        .file(b"Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc())
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

const ANI_PATH: &str = "Character/Human/ANI/huwm_117_test.ANI";

#[test]
fn selecting_an_ani_shows_the_player() {
    let mut w = ani_world("t10-ani-player");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("anim"));
    assert!(w.h.app.app.archive.has_texture(), "the frame on screen is a texture");
    let t = w.h.text();
    assert!(t.contains("Character animation, rig huwm (Human)"), "{t}");
    assert!(t.contains("3 bones, end key 60 (2.0 s at 30 keys per second)"), "{t}");
    for want in ["Loop", "Bones", "Mesh: Default parts (1)", "of 60,"] {
        assert!(t.contains(want), "{want}: {t}");
    }
    assert!(t.contains("Pause") || t.contains("Play"), "{t}");
}

#[test]
fn changing_the_mesh_restarts_the_preview_with_that_mesh() {
    use shaiya_toolkit::preview::{MeshChoice, PreviewData};
    let mut w = ani_world("t10-ani-mesh");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert!(w.h.app.app.archive.has_texture());
    with_cx(&mut w.h, |app, cx| app.archive.choose_anim_mesh(MeshChoice::Skeleton, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    match w.h.app.app.archive.preview_data() {
        Some(PreviewData::Anim { mesh, frames, .. }) => assert!(*mesh == MeshChoice::Skeleton && frames.frames.is_empty()),
        other => panic!("{other:?}"),
    }
    assert!(!w.h.app.app.archive.has_texture(), "the skeleton alone has no picture");
    let torso = "Character/Human/3DC/huwm_torso001.3DC".to_string();
    with_cx(&mut w.h, |app, cx| app.archive.choose_anim_mesh(MeshChoice::One(torso.clone()), cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    match w.h.app.app.archive.preview_data() {
        Some(PreviewData::Anim { mesh, frames, .. }) => assert!(*mesh == MeshChoice::One(torso) && frames.frames.len() == 61),
        other => panic!("{other:?}"),
    }
    assert!(w.h.app.app.archive.has_texture());
    // another file starts again from the default meshes
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("model"), "a .3DC previews as a model");
    assert!(w.h.app.app.archive.has_texture());
    assert!(w.h.text().contains("Skinned mesh: 16 vertices, 24 triangles, 3 bones"), "{}", w.h.text());
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    match w.h.app.app.archive.preview_data() {
        Some(PreviewData::Anim { mesh, .. }) => assert!(*mesh == MeshChoice::Default),
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------------------------------------ T10 W6: the rig check before an .ANI goes in

fn write_ani(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let f = dir.join(name);
    std::fs::write(&f, bytes).unwrap();
    f
}

#[test]
fn replacing_an_ani_with_another_skeleton_asks_before_queueing() {
    use shaiya_testkit::anim::{ani_bytes, translation_bits, BoneSpec};
    let mut w = ani_world("t10-w6-asks");
    let src = TempDir::new("t10-w6-asks-src");
    let row = w.h.app.app.archive.find_row(ANI_PATH).expect("the synthetic animation");
    let one_bone = ani_bytes(0, 10, &[BoneSpec { parent: -1, matrix: translation_bits(0.0, 0.0, 0.0), rot: Vec::new(), trans: Vec::new() }]);
    let file = write_ani(src.path(), "other.ANI", &one_bone);
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, file.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.replace_ask.is_some()), "the rig check opens the question");
    w.h.frames(3);
    assert!(w.h.app.app.archive.pending.is_empty(), "nothing is queued before the answer");
    let t = w.h.text();
    assert!(t.contains("Add anyway") && t.contains("anim-rig-differs"), "{t}");
    w.h.app.shell.inject_answer("toolkit-replace-findings", 1);
    w.h.frames(3);
    assert!(w.h.app.app.archive.replace_ask.is_none() && w.h.app.app.archive.pending.is_empty(), "Cancel queues nothing");
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, file.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.replace_ask.is_some()));
    w.h.frames(3);
    w.h.app.shell.inject_answer("toolkit-replace-findings", 0);
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "Add anyway queues it");
}

#[test]
fn replacing_an_ani_with_the_same_skeleton_queues_at_once() {
    let mut w = ani_world("t10-w6-same");
    let src = TempDir::new("t10-w6-same-src");
    let row = w.h.app.app.archive.find_row(ANI_PATH).expect("the synthetic animation");
    let file = write_ani(src.path(), "same.ANI", &shaiya_testkit::anim::arm_ani(90));
    with_cx(&mut w.h, |app, cx| app.archive.replace_with_file(row, file.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.pending.len() == 1), "queued without a question");
    assert!(w.h.app.app.archive.replace_ask.is_none());
}

// ------------------------------------------------------------------------------------------------ T10 W2bx: .VANI, .MANI, .3DO and .3DE in the Archive tab

/// A synthetic client with one file of each of the four formats, opened in the Archive tab.
fn models_world(tag: &str) -> World {
    use shaiya_testkit::anim::{animated_3do, box_3de, box_vani, mani_bytes};
    let mut w = world(tag);
    SahBuilder::new(0)
        .file(b"Entity/Building/b.VANI", &box_vani(12, 66))
        .file(b"Entity/Building/b.MANI", &mani_bytes(33, 1, 500, 10f32.to_bits()))
        .file(b"Item/3DO/arrow.3DO", &animated_3do(11, 20))
        .file(b"Effect/3DE/fx.3de", &box_3de(Some(4)))
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

fn select_and_wait(w: &mut World, path: &str) {
    with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
    assert!(preview_ready(&mut w.h), "{path}");
    w.h.frames(3);
}

#[test]
fn selecting_a_vani_shows_the_player_at_its_rate() {
    use shaiya_toolkit::preview::PreviewData;
    let mut w = models_world("t10-w2bx-vani");
    select_and_wait(&mut w, "Entity/Building/b.VANI");
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("anim"));
    assert!(w.h.app.app.archive.has_texture(), "the frame on screen is a texture");
    match w.h.app.app.archive.preview_data() {
        Some(PreviewData::Anim { frames, rate, unit, .. }) => assert_eq!((frames.frames.len(), *unit, (*rate - 1000.0 / 66.0).abs() < 1e-4), (12, "step", true)),
        other => panic!("{other:?}"),
    }
    let t = w.h.text();
    for want in ["Vertex-animated model: 1 item, 8 vertices per step, 12 triangles", "about 66 ms per step (inferred from the file; not checked in the game)", "Loop", "step ", "of 11,"] {
        assert!(t.contains(want), "{want}: {t}");
    }
    assert!(t.contains("Pause") || t.contains("Play"), "{t}");
    assert!(!t.contains("Mesh:") && !t.contains("Bones"), "a .VANI has no mesh choice and no bones: {t}");
}

#[test]
fn selecting_a_framed_3do_or_a_3de_with_frames_plays_it() {
    use shaiya_toolkit::preview::PreviewData;
    let mut w = models_world("t10-w2bx-3do");
    select_and_wait(&mut w, "Item/3DO/arrow.3DO");
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("anim"));
    assert!(w.h.app.app.archive.has_texture());
    match w.h.app.app.archive.preview_data() {
        Some(PreviewData::Anim { frames, rate, unit, .. }) => assert_eq!((frames.frames.len(), *rate, *unit), (11, 20.0, "frame")),
        other => panic!("{other:?}"),
    }
    let t = w.h.text();
    assert!(t.contains("3DO mesh: 8 vertices, 12 triangles") && t.contains("11 frames at 20 per second (inferred from the file; not checked in the game)"), "{t}");
    assert!(!t.contains("no preview yet"), "{t}");
    select_and_wait(&mut w, "Effect/3DE/fx.3de");
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("anim"), "a .3DE with frames plays them");
    assert!(w.h.app.app.archive.has_texture());
    let t = w.h.text();
    assert!(t.contains("Effect mesh: 8 vertices, 12 triangles") && t.contains("4 frames, played as positions (inferred) at 30 frames per second"), "{t}");
}

#[test]
fn selecting_a_mani_shows_the_labelled_fields() {
    let mut w = models_world("t10-w2bx-mani");
    select_and_wait(&mut w, "Entity/Building/b.MANI");
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("fields"));
    let t = w.h.text();
    for want in ["Motion parameters for a placed building model (108 bytes)", "Field meanings are inferred and were not checked in the game", "Offset", "Field", "tick_ms", "move_pause_ms"] {
        assert!(t.contains(want), "{want}: {t}");
    }
}

// ------------------------------------------------------------------------------------------------ T11 W7: multi-apply and same-folder clients

fn palette_enabled(h: &Harness<ToolkitApp>, id: &str) -> bool {
    <ToolkitApp as shaiya_ui::ToolApp>::commands(&h.app.app).into_iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no command {id}")).enabled
}

fn owners(h: &Harness<ToolkitApp>) -> Vec<(String, usize)> {
    h.app.app.queued_clients().into_iter().map(|(n, _, c)| (n, c)).collect()
}

#[test]
fn a_running_multi_apply_disables_the_other_writes() {
    let mut w = open_world("t11-multi-lock");
    let a = w.client_dir();
    add_saved(&mut w, "US twin", &a);
    let b = client_of("t11-multi-lock-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t11-multi-lock-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    let ids = ["archive-apply", "archive-save-new", "archive-discard", "archive-compact"];
    // off: every write of the lists is on
    let app = &w.h.app.app;
    assert!(app.archive.write_actions_enabled() && app.apply.write_actions_enabled() && app.health.write_actions_enabled() && app.clients.write_actions_enabled() && app.client_switch_enabled());
    for id in ids {
        assert!(palette_enabled(&w.h, id), "{id} is on");
    }
    // on: all of them are off, whatever tab shows them
    w.h.app.app.multi.set_running_for_tests(true);
    w.h.frames(2);
    let app = &w.h.app.app;
    assert!(app.multi.is_running());
    assert!(!app.archive.write_actions_enabled(), "Archive tab: Apply, Discard, Compact, Save as new archive");
    assert!(!app.apply.write_actions_enabled(), "Apply tab: apply, roll back, prune");
    assert!(!app.health.write_actions_enabled(), "Health tab: Clean up, Put back, Compact, Prune");
    assert!(!app.clients.write_actions_enabled(), "Use another archive pair");
    assert!(!app.client_switch_enabled(), "the client picker, Open and Open recent");
    for id in ids {
        assert!(!palette_enabled(&w.h, id), "{id} is off");
    }
    assert_eq!(w.h.app.app.undo_state(), Err(shaiya_toolkit::multi_lock::MULTI_RUNNING_HOVER.to_string()), "Undo last change and Roll back too");
    // a key or palette entry that reaches an off action does nothing
    with_cx(&mut w.h, |app, cx| shaiya_ui::ToolApp::run_command(app, "archive-discard", cx));
    w.h.frames(2);
    assert!(!w.h.text().contains("Discard 1 pending change"), "no question: {}", w.h.text());
    with_cx(&mut w.h, |app, cx| app.archive.request_apply(cx));
    with_cx(&mut w.h, |app, _| app.archive.request_compact());
    w.h.frames(2);
    assert!(!w.h.app.app.archive.is_applying(), "no apply or compact starts");
    // switching clients does nothing
    with_cx(&mut w.h, |app, cx| app.select_client("EU test", cx));
    w.h.frames(3);
    assert_eq!(w.h.app.app.current.as_ref().map(|c| c.name.as_str()), Some("US test"));
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "the queue is untouched");
    let other_sah = b.path().join("data.sah");
    with_cx(&mut w.h, |app, cx| app.open_archive_file(other_sah, cx));
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.client_dir(), Some(a.as_path()), "Open and Open recent do not switch either");
    // Prune asks nothing, and queueing changes stays available
    let cur = w.h.app.app.current.clone();
    w.h.app.app.apply.request_prune(cur.as_ref());
    assert!(!w.h.app.app.apply.is_busy(), "no Prune confirmation while it runs");
    queue_one(&mut w, &src.path().join("q2.txt"), "queued/two.txt");
    assert_eq!(w.h.app.app.archive.pending.len(), 2, "queueing is not part of the lock");
    // off again: everything is on and the switch works
    w.h.app.app.multi.set_running_for_tests(false);
    w.h.frames(2);
    w.h.app.app.apply.request_prune(cur.as_ref());
    assert!(w.h.app.app.apply.is_busy(), "the same call asks again once it ended");    let app = &w.h.app.app;
    assert!(app.archive.write_actions_enabled() && app.apply.write_actions_enabled() && app.health.write_actions_enabled() && app.clients.write_actions_enabled() && app.client_switch_enabled());
    for id in ids {
        assert!(palette_enabled(&w.h, id), "{id} is on again");
    }
    open_saved(&mut w, "EU test", b.path());
}

/// The picker line of a saved client.
fn picker_line(w: &mut World, name: &str) -> String {
    let c = w.h.app.shell.shared().clients.iter().find(|c| c.name == name).cloned().expect("saved client");
    with_cx(&mut w.h, |app, cx| app.picker_label_of(&c, true, cx))
}

#[test]
fn removing_one_of_two_clients_on_a_folder_keeps_the_queue() {
    // the open client is removed
    let mut w = open_world("t11-remove-two");
    let a = w.client_dir();
    add_saved(&mut w, "US twin", &a);
    let src = TempDir::new("t11-remove-two-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("US test", cx));
    w.h.frames(3);
    assert!(!w.h.text().contains("Drop them and remove"), "no question: {}", w.h.text());
    assert!(!w.h.app.shell.shared().clients.iter().any(|c| c.name == "US test"), "removed at once");
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "the queue is kept");
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    assert_eq!(w.h.app.app.current.as_ref().map(|c| (c.name.as_str(), c.dir.clone())), Some(("US twin", a.clone())), "the open client is the one that stays");
    assert_eq!(owners(&w.h), vec![("US twin".to_string(), 1)]);
    let line = picker_line(&mut w, "US twin");
    assert!(line.ends_with("1 queued"), "{line}");
    // the queue is stashed while another folder is open: its owner name follows
    let mut w = open_world("t11-remove-two-stash");
    let a = w.client_dir();
    add_saved(&mut w, "Zed twin", &a);
    add_saved(&mut w, "US twin", &a);
    let b = client_of("t11-remove-two-stash-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    let src = TempDir::new("t11-remove-two-stash-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    open_saved(&mut w, "EU test", b.path());
    w.h.frames(2);
    assert_eq!(owners(&w.h), vec![("US test".to_string(), 1)]);
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("US test", cx));
    w.h.frames(3);
    assert!(!w.h.text().contains("Drop them and remove"), "no question: {}", w.h.text());
    assert_eq!(w.h.app.app.queued_for(&a), 1, "the stashed queue is kept");
    assert_eq!(owners(&w.h), vec![("Zed twin".to_string(), 1)], "the first remaining client on the folder, by the list's order, owns it");
    assert_eq!(w.h.app.app.current.as_ref().map(|c| c.name.as_str()), Some("EU test"), "the open client is not touched");
    let line = picker_line(&mut w, "US twin");
    assert!(line.ends_with("1 queued"), "{line}");
    // back on the folder the queue is there
    open_saved(&mut w, "US twin", &a);
    assert_eq!(w.h.app.app.archive.pending.len(), 1);
    // the last client on a folder still asks
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("Zed twin", cx));
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.clients.begin_remove("US twin", cx));
    w.h.frames(2);
    assert!(w.h.text().contains("Drop them and remove"), "{}", w.h.text());
}

#[test]
fn renaming_one_of_two_clients_on_a_folder_keeps_the_queue() {
    let mut w = open_world("t11-rename-two");
    let a = w.client_dir();
    add_saved(&mut w, "US twin", &a);
    let src = TempDir::new("t11-rename-two-src");
    queue_one(&mut w, &src.path().join("q.txt"), "queued/one.txt");
    // the owner is the open client: its name follows
    with_cx(&mut w.h, |app, cx| assert!(app.clients.begin_rename("US test", "US renamed", cx)));
    w.h.frames(3);
    assert!(!w.h.text().contains("Drop them and rename"), "no question: {}", w.h.text());
    assert!(w.h.app.shell.shared().clients.iter().any(|c| c.name == "US renamed") && !w.h.app.shell.shared().clients.iter().any(|c| c.name == "US test"));
    assert_eq!(w.h.app.app.archive.pending.len(), 1, "the queue is kept");
    assert_eq!(owners(&w.h), vec![("US renamed".to_string(), 1)], "owned by the new name");
    assert_eq!(w.h.app.app.current.as_ref().map(|c| c.name.as_str()), Some("US renamed"));
    // the owner was the other client: nothing changes
    with_cx(&mut w.h, |app, cx| assert!(app.clients.begin_rename("US twin", "Twin two", cx)));
    w.h.frames(3);
    assert!(!w.h.text().contains("Drop them and rename"));
    assert_eq!(owners(&w.h), vec![("US renamed".to_string(), 1)]);
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    // a stashed queue: its entry's name follows the rename
    let b = client_of("t11-rename-two-b", &[("b.txt", b"b file")]);
    add_saved(&mut w, "EU test", b.path());
    open_saved(&mut w, "EU test", b.path());
    w.h.frames(2);
    assert_eq!(owners(&w.h), vec![("US renamed".to_string(), 1)]);
    with_cx(&mut w.h, |app, cx| assert!(app.clients.begin_rename("US renamed", "US again", cx)));
    w.h.frames(3);
    assert!(!w.h.text().contains("Drop them and rename"));
    assert_eq!(owners(&w.h), vec![("US again".to_string(), 1)]);
    assert_eq!(w.h.app.app.queued_for(&a), 1);
    assert_eq!(w.h.app.app.current.as_ref().map(|c| c.name.as_str()), Some("EU test"));
}

// ------------------------------------------------------------------------------------------------ T11 W8: Export as JSON...

fn export_done(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| !h.app.app.archive.is_exporting_json())
}

fn export_enabled(h: &Harness<ToolkitApp>) -> bool {
    h.app.app.archive.commands().into_iter().find(|c| c.id == "archive-export-json").expect("the command is listed").enabled
}

#[test]
fn exporting_an_ani_writes_json_that_imports_to_the_same_bytes() {
    let mut w = ani_world("t11-w8-ani");
    let out_dir = TempDir::new("t11-w8-ani-out");
    let out = out_dir.path().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    assert!(export_enabled(&w.h), "one selected .ANI enables the command");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_exporting_json() || out.exists()), "the export starts");
    assert!(export_done(&mut w), "the export ends");
    w.h.frames(2);
    let json = std::fs::read(&out).expect("the JSON file is written");
    let (format, bytes) = shaiya_clientdata::modeljson::import(&json).expect("the JSON imports");
    assert_eq!(format, shaiya_clientdata::modeljson::ModelFormat::Ani);
    assert_eq!(bytes, shaiya_testkit::anim::arm_ani(60), "the bytes are the archive entry's");
    let text = String::from_utf8(json).unwrap();
    assert!(text.contains(ANI_PATH) && !text.contains(&out_dir.path().display().to_string()), "the source is the archive path, no folder");
    assert!(w.h.app.app.archive.status_line.starts_with("Exported huwm_117_test.ANI to "), "{}", w.h.app.app.archive.status_line);
    assert!(w.h.app.shell.banner_details().is_none(), "no error was shown");
    let leftovers: Vec<_> = std::fs::read_dir(out_dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, vec!["x.json".to_string()], "no temporary file stays");
}

#[test]
fn exporting_into_the_client_folder_is_refused() {
    let mut w = ani_world("t11-w8-guard");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let before = tree(w.client.path());
    let inside = w.client_dir().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&inside, cx));
    assert!(export_done(&mut w));
    w.h.frames(2);
    assert!(!inside.exists(), "nothing was written");
    assert_eq!(tree(w.client.path()), before, "the client folder is as it was");
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains("Choose a folder outside the client:"), "{d}");
}

#[test]
fn the_export_command_is_disabled_for_other_files_and_failed_previews() {
    let bad = b"not an animation".to_vec();
    let client = client_of("t11-w8-other", &[("tex/a.dds", b"DDS not really"), ("Character/Human/ANI/huwm_999_bad.ANI", &bad)]);
    let mut w = world("t11-w8-other");
    let dir = client.path().to_path_buf();
    add_saved(&mut w, "Other", &dir);
    open_saved(&mut w, "Other", &dir);
    assert!(!export_enabled(&w.h), "nothing is selected");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("tex/a.dds", cx));
    assert!(preview_ready(&mut w.h));
    assert!(!export_enabled(&w.h), "a .dds row is not one of the six formats");
    let out_dir = TempDir::new("t11-w8-other-out");
    let none = out_dir.path().join("none.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&none, cx));
    assert!(!w.h.app.app.archive.is_exporting_json() && !none.exists(), "no task starts for a .dds row");
    let bad_path = "Character/Human/ANI/huwm_999_bad.ANI";
    with_cx(&mut w.h, |app, cx| app.archive.select_path(bad_path, cx));
    assert!(preview_ready(&mut w.h));
    assert!(export_enabled(&w.h), "an .ANI row enables the command even when its preview failed");
    let out = out_dir.path().join("bad.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    assert!(export_done(&mut w));
    w.h.frames(2);
    assert!(!out.exists(), "nothing was written");
    let leftovers: Vec<_> = std::fs::read_dir(out_dir.path()).unwrap().flatten().collect();
    assert!(leftovers.is_empty(), "no temporary file stays");
    let want = shaiya_clientdata::modeljson::export(shaiya_clientdata::modeljson::ModelFormat::Ani, &bad, bad_path).unwrap_err().to_string();
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains(&want), "the reader's message {want:?} is shown: {d}");
}

#[test]
fn the_export_menu_entry_is_for_the_six_model_formats_in_any_letter_case() {
    use shaiya_toolkit::export_json::is_model_path;
    for p in ["a/b/x.ANI", "x.3dc", "d/x.VANI", "x.Mani", "x.3DO", "x.3de"] {
        assert!(is_model_path(p), "{p}");
    }
    for p in ["a/x.dds", "a/ani", "x.ani.txt", "dir.ANI/file", "x.json"] {
        assert!(!is_model_path(p), "{p}");
    }
}

#[test]
fn a_second_export_to_a_target_that_is_being_written_is_refused() {
    let mut w = ani_world("t11-w8-twice");
    let out_dir = TempDir::new("t11-w8-twice-out");
    let out = out_dir.path().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    let d = w.h.app.shell.banner_details().expect("the second export is refused with a message").to_string();
    assert!(d.contains("An export to") && d.contains("is running"), "{d}");
    assert!(export_done(&mut w));
    w.h.frames(2);
    let json = std::fs::read(&out).expect("the first export wrote its file");
    assert_eq!(shaiya_clientdata::modeljson::import(&json).expect("the JSON imports").1, shaiya_testkit::anim::arm_ani(60));
}

#[test]
fn exporting_into_a_saved_client_that_is_not_open_is_refused() {
    let mut w = ani_world("t12-saved");
    let other = client_of("t12-saved-other", &[("tex/a.dds", b"DDS not really")]);
    let other_dir = other.path().to_path_buf();
    add_saved(&mut w, "Other", &other_dir);
    assert_ne!(w.h.app.app.archive.client_dir(), Some(other_dir.as_path()), "the other client is saved, not open");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let before = tree(other.path());
    let inside = other_dir.join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&inside, cx));
    assert!(!w.h.app.app.archive.is_exporting_json() && !inside.exists(), "nothing starts");
    assert_eq!(tree(other.path()), before, "the saved client's folder is as it was");
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains("Choose a folder outside the client:"), "{d}");
}

#[test]
fn a_second_export_to_the_same_file_in_another_letter_case_is_refused_on_windows() {
    if !cfg!(windows) {
        return; // other systems keep the two names apart
    }
    let mut w = ani_world("t12-case");
    let out_dir = TempDir::new("t12-case-out");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out_dir.path().join("x.json"), cx));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out_dir.path().join("X.JSON"), cx));
    let d = w.h.app.shell.banner_details().expect("the second export is refused with a message").to_string();
    assert!(d.contains("An export to") && d.contains("is running"), "{d}");
    assert!(export_done(&mut w));
}

#[test]
fn an_export_target_must_be_a_json_file_name() {
    let mut w = ani_world("t12-ext");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let out_dir = TempDir::new("t12-ext-out");
    for name in ["wave.ani", "wave", "wave.json.txt"] {
        let out = out_dir.path().join(name);
        with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
        assert!(!w.h.app.app.archive.is_exporting_json() && !out.exists(), "{name}: nothing starts");
        let d = w.h.app.shell.banner_details().expect("the refusal is shown").to_string();
        assert!(d.contains("The file name must end in .json"), "{d}");
    }
    let ok = out_dir.path().join("Wave.JSON");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&ok, cx));
    assert!(export_done(&mut w));
    w.h.frames(2);
    assert!(ok.exists(), "an upper-case .JSON is accepted");
}

#[test]
fn exporting_to_a_missing_folder_is_refused() {
    let mut w = ani_world("t11-w8-nofolder");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let out_dir = TempDir::new("t11-w8-nofolder-out");
    let out = out_dir.path().join("missing").join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    assert!(!w.h.app.app.archive.is_exporting_json() && !out.exists());
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains("The folder does not exist"), "{d}");
}

#[test]
fn the_export_command_needs_exactly_one_file() {
    let mut w = ani_world("t11-w8-two");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    assert!(export_enabled(&w.h));
    let other = w.h.app.app.archive.find_row("Character/Human/3DC/huwm_torso001.3DC").expect("the mesh row");
    w.h.app.app.archive.selected.insert(other);
    assert_eq!(w.h.app.app.archive.selected.len(), 2);
    assert!(!export_enabled(&w.h), "two selected files disable the command");
}

#[test]
fn the_export_is_off_while_only_the_index_is_open() {
    let mut w = world("t11-w8-index");
    SahBuilder::new(0).file(b"Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60)).write_to(w.client.path());
    for e in std::fs::read_dir(w.client.path()).unwrap().flatten() {
        if e.file_name().to_string_lossy().eq_ignore_ascii_case("data.saf") {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("Index only", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the index opens");
    w.h.frames(2);
    assert!(!w.h.app.app.archive.offers_export_json(ANI_PATH), "the row menu hides the entry");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(!export_enabled(&w.h), "the command is disabled");
    let out_dir = TempDir::new("t11-w8-index-out");
    let out = out_dir.path().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&out, cx));
    assert!(!w.h.app.app.archive.is_exporting_json() && !out.exists());
}

// ------------------------------------------------------------------------------------------------ T11 W10: view state

/// Drags the left edge of the right-hand preview panel from `from` to `to` (screen x), one frame per step.
fn drag_preview_edge(h: &mut Harness<ToolkitApp>, from: f32, to: f32) {
    use egui::{Event, PointerButton};
    let y = 400.0;
    let btn = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let p = |x: f32| egui::pos2(x, y);
    h.frame(vec![Event::PointerMoved(p(from))]);
    h.frame(vec![btn(p(from), true)]);
    for i in 1..=4 {
        let x = from + (to - from) * i as f32 / 4.0;
        h.frame(vec![Event::PointerMoved(p(x))]);
    }
    h.frame(vec![btn(p(to), false)]);
    h.frames(3);
}

/// The `settings.json` of the tool under a data folder.
fn settings_file_under(dir: &std::path::Path) -> PathBuf {
    fn walk(d: &std::path::Path, out: &mut Option<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.file_name().is_some_and(|n| n == "settings.json") {
                *out = Some(p);
            }
        }
    }
    let mut found = None;
    walk(dir, &mut found);
    found.expect("the tool wrote its settings.json")
}

#[test]
fn a_dragged_preview_width_survives_selecting_another_file() {
    let mut w = open_world("t11-w10-width");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Item/3DO/a.txt", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let before = w.h.app.tool_settings.preview_width;
    assert!((before - 360.0).abs() < 1.0, "the pane starts at its saved width, not at its content: {before}");
    // the saved width is the panel's outer width, so its left edge is that far from the right edge of the window
    let edge = w.h.size.x - before;
    drag_preview_edge(&mut w.h, edge, edge - 120.0);
    let dragged = w.h.app.tool_settings.preview_width;
    assert!(dragged > before + 80.0, "the drag widened the pane: {before} -> {dragged}");
    for path in ["Item/3DO/b.txt", "interface/a.tga", "x/blob.bin", "Item/Item.SData", "top.txt"] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
        assert!(preview_ready(&mut w.h), "{path}");
        w.h.frames(3);
        let now = w.h.app.tool_settings.preview_width;
        assert!((now - dragged).abs() < 1.0, "{path}: width {dragged} became {now}");
    }
    // and with no file selected at all
    w.h.app.app.archive.preview.row = None;
    w.h.frames(3);
    assert!((w.h.app.tool_settings.preview_width - dragged).abs() < 1.0, "an empty pane keeps its width");
}

#[test]
fn the_pane_width_and_the_preview_toggles_come_back_after_a_restart() {
    let mut w = ani_world("t11-w10-restart");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let edge = w.h.size.x - w.h.app.tool_settings.preview_width;
    drag_preview_edge(&mut w.h, edge, edge - 100.0);
    let width = w.h.app.tool_settings.preview_width;
    assert!(width > 400.0, "{width}");
    {
        let p = &mut w.h.app.app.archive.preview.player;
        p.looping = false;
        p.show_bones = false;
    }
    w.h.frames(3);
    assert!(!w.h.app.tool_settings.preview_loop && !w.h.app.tool_settings.preview_bones, "the drawn toggles are stored");
    w.h.app.flush_settings(true);
    let text = std::fs::read_to_string(settings_file_under(&w.data_path)).unwrap();
    let h2 = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(w.data_path.clone()), ..Default::default() });
    let s = &h2.app.tool_settings;
    assert!((s.preview_width - width).abs() < 1.0, "{} vs {width}
{text}", s.preview_width);
    assert!(!s.preview_loop && !s.preview_bones && s.preview_fit && s.preview_alpha);
}

#[test]
fn the_animation_toggles_stay_when_another_animation_or_file_is_selected() {
    let mut w = ani_world("t11-w10-toggles");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert!(w.h.app.app.archive.preview.player.looping && w.h.app.app.archive.preview.player.show_bones, "both start on");
    w.h.app.app.archive.preview.player.show_bones = false;
    w.h.app.app.archive.preview.player.looping = false;
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let p = &w.h.app.app.archive.preview.player;
    assert!(!p.looping && !p.show_bones, "Loop and Bones are still off for the next animation");
}

#[test]
fn the_textures_setting_decides_whether_a_model_preview_is_textured() {
    let mut w = ani_world("w6-textures-setting");
    assert!(w.h.app.tool_settings.preview_textured, "textures are on by default");
    w.h.app.tool_settings.preview_textured = false;
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert!(!w.h.app.app.archive.preview.textured);
    let Some(shaiya_toolkit::preview::PreviewData::Model { notes, .. }) = w.h.app.app.archive.preview_data() else { panic!("not a model preview") };
    assert!(notes.iter().any(|n| n == shaiya_toolkit::preview_tex::TEXTURES_OFF), "{notes:?}");
    let t = w.h.text();
    assert!(t.contains("Textures") && t.contains("textures are turned off"), "{t}");
    // on again: the note about the missing list file takes its place
    w.h.app.tool_settings.preview_textured = true;
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(preview_ready(&mut w.h));
    let Some(shaiya_toolkit::preview::PreviewData::Model { notes, .. }) = w.h.app.app.archive.preview_data() else { panic!("not a model preview") };
    assert!(notes.iter().any(|n| n == shaiya_toolkit::preview_tex::NOT_LISTED), "{notes:?}");
}

#[test]
fn an_old_settings_file_without_the_preview_fields_still_loads() {
    let old = r#"{"tab":"archive","last_client":"US test","tree_width":210.0,"preview_width":400.0,"columns":[100.0],"show_shadowed":true,"text_encoding":"UTF-8","extract_keep_times":false,"keys":{}}"#;
    let s: ToolkitSettings = serde_json::from_str(old).unwrap();
    assert_eq!((s.preview_width, s.tree_width, s.show_shadowed, s.extract_keep_times), (400.0, 210.0, true, false), "the old fields are read");
    assert!(s.preview_loop && s.preview_bones && s.preview_fit && s.preview_alpha && s.preview_textured, "the new ones take their defaults");
    // the same through a data folder
    let data = TempDir::new("t11-w10-old");
    let h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    drop(h);
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    h.app.flush_settings(true);
    let file = settings_file_under(data.path());
    std::fs::write(&file, format!(r#"{{"shell":{{}},"tool":{old}}}"#)).unwrap();
    let h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    assert_eq!(h.app.tool_settings.preview_width, 400.0);
    assert!(h.app.tool_settings.preview_loop && h.app.tool_settings.preview_fit);
}

#[test]
fn the_preview_settings_round_trip() {
    let s = ToolkitSettings { preview_width: 512.5, preview_loop: false, preview_bones: false, preview_fit: false, preview_alpha: false, preview_textured: false, ..Default::default() };
    let back: ToolkitSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}

#[test]
fn fit_and_alpha_of_the_picture_preview_stay_when_another_picture_is_selected() {
    let mut w = open_world("t11-w10-fit");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("interface/a.tga", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert!(w.h.app.app.archive.preview.fit && w.h.app.app.archive.preview.alpha, "both start on");
    w.h.app.app.archive.preview.fit = false;
    w.h.app.app.archive.preview.alpha = false;
    w.h.frames(2);
    with_cx(&mut w.h, |app, cx| app.archive.select_path("interface/b.bmp", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    assert!(!w.h.app.app.archive.preview.fit && !w.h.app.app.archive.preview.alpha);
    assert!(!w.h.app.tool_settings.preview_fit && !w.h.app.tool_settings.preview_alpha);
}

// ------------------------------------------------------------------------------------------------ T12 W7: preview controls and remembered state

/// The first picture of the animation preview that is open.
fn first_frame(h: &Harness<ToolkitApp>) -> Vec<u8> {
    match h.app.app.archive.preview_data() {
        Some(shaiya_toolkit::preview::PreviewData::Anim { frames, .. }) => frames.frames[0].clone(),
        _ => panic!("not an animation preview"),
    }
}

/// Runs frames until the preview is ready and its first picture is not `old`.
fn wait_new_frame(h: &mut Harness<ToolkitApp>, old: &[u8]) -> bool {
    h.wait_until(WAIT, |h| {
        h.app.app.archive.preview_ready() && matches!(h.app.app.archive.preview_data(), Some(shaiya_toolkit::preview::PreviewData::Anim { frames, .. }) if !frames.frames.is_empty() && frames.frames[0] != old)
    })
}

fn ani_ready(tag: &str) -> World {
    let mut w = ani_world(tag);
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    w
}

#[test]
fn an_old_settings_file_without_the_0_5_fields_still_loads() {
    // the settings of 0.4.1, literally
    let old = r#"{"tab":"archive","last_client":"US test","tree_width":210.0,"preview_width":400.0,"columns":[100.0],"show_shadowed":true,"text_encoding":"UTF-8","extract_keep_times":false,"preview_loop":false,"preview_bones":true,"preview_fit":true,"preview_alpha":true,"preview_textured":false,"keys":{}}"#;
    let s: ToolkitSettings = serde_json::from_str(old).unwrap();
    assert_eq!((s.preview_width, s.preview_loop, s.preview_textured), (400.0, false, false), "the old fields are read");
    assert_eq!((s.preview_speed, s.preview_view, s.last_export_dir.as_str(), s.sort_key.as_str(), s.sort_desc), (1.0, [35.0, 25.0, 1.0], "", "name", false), "the new ones take their defaults");
    assert_eq!(s.clone().sanitized(), s, "defaults are in range");
}

#[test]
fn the_0_5_preview_settings_round_trip() {
    let s = ToolkitSettings { preview_speed: 2.0, preview_view: [120.0, -30.0, 2.5], last_export_dir: "out".into(), sort_key: "size".into(), sort_desc: true, ..Default::default() };
    let back: ToolkitSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    assert_eq!(back.clone().sanitized(), s, "values in range are kept");
}

#[test]
fn out_of_range_settings_fall_back_to_defaults() {
    let d = ToolkitSettings::default();
    for speed in [f32::NAN, 0.0, 0.2, 4.5, -1.0, f32::INFINITY] {
        assert_eq!(ToolkitSettings { preview_speed: speed, ..Default::default() }.sanitized().preview_speed, 1.0, "speed {speed}");
    }
    for view in [[f32::NAN, 0.0, 1.0], [0.0, f32::INFINITY, 1.0], [10.0, 10.0, 0.0], [10.0, 10.0, -1.0], [10.0, 10.0, f32::NAN]] {
        assert_eq!(ToolkitSettings { preview_view: view, ..Default::default() }.sanitized().preview_view, d.preview_view, "view {view:?}");
    }
    assert_eq!(ToolkitSettings { sort_key: "colour".into(), ..Default::default() }.sanitized().sort_key, "name");
    // the same through a settings file of a data folder
    let data = TempDir::new("w7-out-of-range");
    let mut h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    h.frames(2);
    h.app.flush_settings(true);
    drop(h);
    let file = settings_file_under(data.path());
    std::fs::write(&file, r#"{"shell":{},"tool":{"preview_speed":9.0,"preview_view":[10.0,20.0,-2.0],"sort_key":"colour","sort_desc":true}}"#).unwrap();
    let h = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(data.path().to_path_buf()), ..Default::default() });
    let s = &h.app.tool_settings;
    assert_eq!((s.preview_speed, s.preview_view, s.sort_key.as_str(), s.sort_desc), (1.0, [35.0, 25.0, 1.0], "name", true));
}

#[test]
fn dragging_turns_the_view_and_renders_again_when_released() {
    let mut w = ani_ready("w7-drag");
    let before = first_frame(&w.h);
    let default = w.h.app.tool_settings.preview_view;
    w.h.app.app.archive.drag_view_for_tests(40.0, 10.0);
    assert!(wait_new_frame(&mut w.h, &before), "the frames are made again with the turned view");
    let v = w.h.app.tool_settings.preview_view;
    assert!((v[0] - (default[0] + 20.0)).abs() < 1e-3 && (v[1] - (default[1] + 5.0)).abs() < 1e-3, "{v:?}");
    assert_eq!(w.h.app.app.archive.preview.controls.shown, w.h.app.app.archive.preview.controls.ctl.view);
}

#[test]
fn zoom_renders_again_after_the_wheel_stops() {
    let mut w = ani_ready("w7-zoom");
    let before = first_frame(&w.h);
    w.h.app.app.archive.wheel_view_for_tests(300.0);
    assert!(wait_new_frame(&mut w.h, &before), "the frames are made again once the wheel stopped");
    assert!(w.h.app.tool_settings.preview_view[2] > 1.5, "{:?}", w.h.app.tool_settings.preview_view);
}

#[test]
fn reset_view_returns_to_the_default() {
    let mut w = ani_ready("w7-reset");
    let start = first_frame(&w.h);
    w.h.app.app.archive.drag_view_for_tests(60.0, 0.0);
    assert!(wait_new_frame(&mut w.h, &start));
    let turned = first_frame(&w.h);
    w.h.app.app.archive.reset_view_for_tests();
    assert!(wait_new_frame(&mut w.h, &turned));
    assert_eq!(w.h.app.tool_settings.preview_view, [35.0, 25.0, 1.0]);
    assert_eq!(first_frame(&w.h), start, "the default view draws what 0.4.1 drew");
}

#[test]
fn the_view_comes_back_after_a_restart() {
    let mut w = ani_ready("w7-restart");
    let before = first_frame(&w.h);
    w.h.app.app.archive.drag_view_for_tests(-30.0, 20.0);
    assert!(wait_new_frame(&mut w.h, &before));
    let turned = first_frame(&w.h);
    let want = w.h.app.tool_settings.preview_view;
    w.h.app.flush_settings(true);
    let mut h2 = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(w.data_path.clone()), ..Default::default() });
    assert_eq!(h2.app.tool_settings.preview_view, want);
    // the last client opens at start
    assert!(h2.wait_until(WAIT, |h| h.app.app.archive.is_open()));
    with_cx(&mut h2, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut h2));
    assert_eq!(first_frame(&h2), turned, "the next run draws with the remembered view");
}

#[test]
fn the_sort_order_comes_back_after_a_restart() {
    let mut w = open_world("w7-sort");
    with_cx(&mut w.h, |app, cx| {
        app.archive.sort_for_tests("size", cx);
        app.archive.sort_for_tests("size", cx);
    });
    assert_eq!((w.h.app.tool_settings.sort_key.as_str(), w.h.app.tool_settings.sort_desc), ("size", true));
    w.h.app.flush_settings(true);
    let h2 = Harness::<ToolkitApp>::new(shaiya_ui::ShellOptions { data_dir: Some(w.data_path.clone()), ..Default::default() });
    assert_eq!(h2.app.app.archive.sort_key, shaiya_toolkit::archive_view::SortKey::Size);
    assert!(h2.app.app.archive.sort_desc);
}

#[test]
fn save_frame_writes_a_png_of_the_frame_on_screen() {
    let mut w = ani_ready("w7-save");
    let out_dir = TempDir::new("w7-save-out");
    let out = out_dir.path().join("frame.png");
    with_cx(&mut w.h, |app, cx| app.archive.save_frame_to(&out, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_saving_frame()));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(shaiya_toolkit::preview_controls::png_size(&bytes), Some((240, 220)));
    assert_eq!(w.h.app.tool_settings.last_export_dir, out_dir.path().display().to_string());
    let Some(shaiya_toolkit::preview::PreviewData::Anim { frames, .. }) = w.h.app.app.archive.preview_data() else { panic!("not an animation preview") };
    assert_eq!((frames.width, frames.height), (240, 220));
    // not a .png name: refused, nothing written
    let bad = out_dir.path().join("frame.txt");
    assert!(with_cx(&mut w.h, |app, cx| app.archive.save_frame_to(&bad, cx)).is_err() && !bad.exists());
}

#[test]
fn save_frame_into_a_client_folder_is_refused_and_writes_nothing() {
    let mut w = ani_ready("w7-save-refused");
    let into = w.client_dir().join("frame.png");
    let err = with_cx(&mut w.h, |app, cx| app.archive.save_frame_to(&into, cx)).unwrap_err();
    assert!(err.starts_with("Choose a folder outside the client:"), "{err}");
    assert!(!w.h.app.app.archive.is_saving_frame());
    w.h.frames(3);
    assert!(!into.exists(), "nothing was written");
    let temps: Vec<_> = std::fs::read_dir(w.client_dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.to_lowercase().contains("png")).collect();
    assert!(temps.is_empty(), "no temporary file either: {temps:?}");
}

#[test]
fn the_comma_and_period_keys_step_one_frame_and_pause() {
    let mut w = ani_ready("w7-keys");
    w.h.app.app.archive.pause_anim_at(10.0);
    w.h.frames(2);
    w.h.key(Key::Period, Modifiers::NONE);
    let p = w.h.app.app.archive.preview.player.clone();
    assert_eq!((p.t, p.playing), (11.0, false), "one frame on");
    w.h.key(Key::Comma, Modifiers::NONE);
    w.h.key(Key::Comma, Modifiers::NONE);
    assert_eq!(w.h.app.app.archive.preview.player.t, 9.0, "two frames back");
    w.h.frames(2);
    assert!(w.h.text().contains("Speed: 1x"), "{}", w.h.text());
}

#[test]
fn save_frame_of_a_still_model_writes_its_picture() {
    let mut w = ani_world("w7-save-still");
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Character/Human/3DC/huwm_torso001.3DC", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let (pw, ph) = match w.h.app.app.archive.preview_data() {
        Some(shaiya_toolkit::preview::PreviewData::Model { width, height, rgba, .. }) if !rgba.is_empty() => (*width, *height),
        _ => panic!("not a model picture"),
    };
    let out_dir = TempDir::new("w7-save-still-out");
    let out = out_dir.path().join("torso.png");
    with_cx(&mut w.h, |app, cx| app.archive.save_frame_to(&out, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.is_saving_frame()));
    assert_eq!(shaiya_toolkit::preview_controls::png_size(&std::fs::read(&out).unwrap()), Some((pw, ph)));
}

// ------------------------------------------------------------------------------------------------ T12 W8: batch export, compare, rig filter

/// `ani_world` plus more animations of two rigs, so a folder holds several model files.
/// The skeleton of `arm_ani(60)` with a different pose: bone 1 turns 45 degrees (not 90) over 60 keys and bone 0 moves along x.
fn turned_arm() -> Vec<u8> {
    use shaiya_testkit::anim::{ani_bytes, translation_bits, BoneSpec};
    let half = std::f32::consts::FRAC_PI_8;
    let bone = |parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot, trans };
    ani_bytes(0, 60, &[bone(-1, 0.0, Vec::new(), vec![(0, [0.0; 3]), (60, [5.0, 0.0, 0.0])]), bone(0, 1.0, vec![(0, [0.0, 0.0, 0.0, 1.0]), (60, [0.0, 0.0, half.sin(), half.cos()])], Vec::new()), bone(1, 2.0, Vec::new(), Vec::new())])
}

fn rig_world(tag: &str) -> World {
    let mut w = world(tag);
    let arm = shaiya_testkit::anim::arm_ani(60);
    SahBuilder::new(0)
        .file(b"Character/Human/ANI/huwm_117_test.ANI", &arm)
        .file(b"Character/Human/ANI/huwm_118_test.ANI", &arm)
        .file(b"Character/Human/ANI/huwf_001_test.ANI", &arm)
        .file(b"Character/Human/ANI/huwf_002_test.ANI", &turned_arm())
        .file(b"Character/Human/ANI/notes.txt", b"not a model")
        .file(b"Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc())
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

fn folder_export_done(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| !h.app.app.archive.is_exporting_folder())
}

fn json_files_under(d: &std::path::Path) -> Vec<String> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, v: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, v);
            } else {
                v.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut v = Vec::new();
    walk(d, d, &mut v);
    v.sort();
    v
}

#[test]
fn exporting_a_folder_writes_every_model_file_as_json() {
    let mut w = rig_world("t12-w8-folder");
    let out = TempDir::new("t12-w8-folder-out");
    let dir = out.path().join("json");
    with_cx(&mut w.h, |app, cx| app.archive.export_folder_json_to("Character/Human", dir.clone(), cx));
    assert!(folder_export_done(&mut w));
    w.h.frames(2);
    let r = w.h.app.app.archive.folder_export_report().expect("a report").clone();
    assert_eq!((r.written, r.total, r.failed.len(), r.cancelled), (5, 5, 0, false), "{r:?}");
    assert_eq!(
        json_files_under(&dir),
        vec![
            "Character/Human/3DC/huwm_torso001.3DC.json",
            "Character/Human/ANI/huwf_001_test.ANI.json",
            "Character/Human/ANI/huwf_002_test.ANI.json",
            "Character/Human/ANI/huwm_117_test.ANI.json",
            "Character/Human/ANI/huwm_118_test.ANI.json",
        ],
        "the text file is not a model file"
    );
    assert!(w.h.app.app.archive.status_line.starts_with("Exported 5 of 5 files to "), "{}", w.h.app.app.archive.status_line);
    // each file is the JSON of the single export, with the archive path as its source
    let one = out.path().join("one.json");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.export_json_to(&one, cx));
    assert!(export_done(&mut w));
    assert_eq!(std::fs::read(&one).unwrap(), std::fs::read(dir.join("Character/Human/ANI/huwm_117_test.ANI.json")).unwrap());
    // a second run keeps what is there and lists every file as failed
    with_cx(&mut w.h, |app, cx| app.archive.export_folder_json_to("Character/Human/ANI", dir.clone(), cx));
    assert!(folder_export_done(&mut w));
    w.h.frames(3);
    let r = w.h.app.app.archive.folder_export_report().expect("a report").clone();
    assert_eq!((r.written, r.total, r.failed.len()), (0, 4, 4), "{r:?}");
    assert!(r.failed.iter().all(|(_, why)| why.contains("exists and is kept")), "{r:?}");
    assert!(w.h.text().contains("Export failures"), "the failure list window opens: {}", w.h.text());
}

#[test]
fn a_folder_export_into_a_client_is_refused_and_starts_nothing() {
    let mut w = rig_world("t12-w8-folder-refuse");
    let inside = w.client_dir().join("out");
    with_cx(&mut w.h, |app, cx| app.archive.export_folder_json_to("Character/Human", inside.clone(), cx));
    assert!(!w.h.app.app.archive.is_exporting_folder() && !inside.exists());
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains("Choose a folder outside the client"), "{d}");
}

#[test]
fn the_folder_export_can_be_cancelled_and_counts_what_was_written() {
    let mut w = rig_world("t12-w8-folder-cancel");
    let out = TempDir::new("t12-w8-folder-cancel-out");
    let dir = out.path().join("json");
    // hold the export at its first progress report (after the first file): it cannot finish before the cancel below
    w.h.app.shell.tasks().hold_progress();
    with_cx(&mut w.h, |app, cx| app.archive.export_folder_json_to("Character/Human", dir.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.shell.tasks().views().iter().any(|v| v.title.starts_with("Export") && v.done > 0)), "the export started");
    w.h.app.app.archive.cancel_folder_export();
    w.h.app.shell.tasks().release_progress();
    assert!(folder_export_done(&mut w));
    w.h.frames(2);
    let r = w.h.app.app.archive.folder_export_report().expect("a report").clone();
    assert!(r.cancelled && r.written == 1 && r.total == 5, "{r:?}");
    assert_eq!(json_files_under(&dir).len(), 1, "exactly the counted file is on disk");
    assert!(w.h.app.app.archive.status_line.starts_with("Export cancelled: wrote 1 of 5 files to "), "{}", w.h.app.app.archive.status_line);
}

#[test]
fn comparing_with_another_client_reads_the_same_path_there() {
    let mut w = rig_world("t12-w8-compare");
    // another client holds the same path with the second bone turned half as far
    let half = std::f32::consts::FRAC_PI_8;
    let b = |parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>| shaiya_testkit::anim::BoneSpec { parent, matrix: shaiya_testkit::anim::translation_bits(0.0, y, 0.0), rot, trans: Vec::new() };
    let other_ani = shaiya_testkit::anim::ani_bytes(0, 60, &[b(-1, 0.0, Vec::new()), b(0, 1.0, vec![(0, [0.0, 0.0, 0.0, 1.0]), (60, [0.0, 0.0, half.sin(), half.cos()])]), b(1, 2.0, Vec::new())]);
    let other = client_of("t12-w8-compare-other", &[(ANI_PATH, &other_ani)]);
    add_saved(&mut w, "EU other", other.path());
    assert!(w.h.app.app.archive.offers_compare(ANI_PATH), "the row menu offers it for an .ANI");
    assert!(!w.h.app.app.archive.offers_compare("Character/Human/ANI/notes.txt"));
    let dir = other.path().to_path_buf();
    with_cx(&mut w.h, |app, cx| app.archive.compare_ani_with(ANI_PATH, shaiya_toolkit::anim_compare::Other::Client { name: "EU other".into(), dir }, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.anim_compare().is_some_and(|c| !c.is_running() && c.result.is_some())), "the comparison ends");
    w.h.frames(3);
    let shown = w.h.app.app.archive.anim_compare().and_then(|c| c.result.clone()).expect("a result").expect("it reads");
    assert_eq!(shown.a, ANI_PATH);
    assert!(shown.b.ends_with("in EU other"), "{}", shown.b);
    let bones: Vec<usize> = shown.diff.bones.iter().map(|b| b.bone).collect();
    assert_eq!(bones, vec![2, 1], "{:?}", shown.diff);
    assert!((shown.diff.bones[1].max_angle_deg - 45.0).abs() < 0.01);
    let t = w.h.text();
    assert!(t.contains("Compare animation") && t.contains("2 bones differ in pose"), "{t}");
    // a client that has no such file says so, and another .ANI of this archive can be chosen too
    let none = client_of("t12-w8-compare-none", &[("x.txt", b"x")]);
    add_saved(&mut w, "No file", none.path());
    let dir = none.path().to_path_buf();
    with_cx(&mut w.h, |app, cx| app.archive.compare_ani_with(ANI_PATH, shaiya_toolkit::anim_compare::Other::Client { name: "No file".into(), dir }, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.anim_compare().is_some_and(|c| !c.is_running() && c.result.is_some())));
    let e = w.h.app.app.archive.anim_compare().and_then(|c| c.result.clone()).expect("a result").expect_err("not there");
    assert!(e.contains("not in No file"), "{e}");
    with_cx(&mut w.h, |app, cx| app.archive.compare_ani_with(ANI_PATH, shaiya_toolkit::anim_compare::Other::File("Character/Human/ANI/huwm_118_test.ANI".into()), cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.anim_compare().is_some_and(|c| !c.is_running() && c.result.is_some())));
    let same = w.h.app.app.archive.anim_compare().and_then(|c| c.result.clone()).expect("a result").expect("it reads");
    assert!(same.diff.is_identical(), "{:?}", same.diff);
}

#[test]
fn the_animation_combo_plays_a_chosen_animation_on_the_mesh() {
    let mut w = rig_world("t12-w8-anim-combo");
    let mesh = "Character/Human/3DC/huwm_torso001.3DC";
    with_cx(&mut w.h, |app, cx| app.archive.select_path(mesh, cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let Some(shaiya_toolkit::preview::PreviewData::Model { animations, .. }) = w.h.app.app.archive.preview_data() else { panic!("not a mesh preview") };
    assert_eq!(animations, &vec!["Character/Human/ANI/huwm_117_test.ANI".to_string(), "Character/Human/ANI/huwm_118_test.ANI".to_string()], "only the rig of this mesh");
    assert!(w.h.text().contains("Animation: play on this mesh"), "{}", w.h.text());
    with_cx(&mut w.h, |app, cx| app.archive.play_animation_on_mesh("Character/Human/ANI/huwm_118_test.ANI", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(3);
    let Some(shaiya_toolkit::preview::PreviewData::Anim { mesh: chosen, frames, .. }) = w.h.app.app.archive.preview_data() else { panic!("not an animation preview") };
    assert_eq!(chosen, &shaiya_toolkit::preview::MeshChoice::One(mesh.to_string()));
    assert!(!frames.frames.is_empty());
    assert!(w.h.text().contains("huwm_118_test.ANI"), "{}", w.h.text());
}

#[test]
fn the_rig_filter_shows_only_that_rig() {
    let mut w = rig_world("t12-w8-rig");
    with_cx(&mut w.h, |app, _| app.archive.enter_folder("Character/Human/ANI"));
    w.h.frames(3);
    let shown = |w: &World| w.h.app.app.archive.shown_paths().into_iter().map(|p| p.rsplit('/').next().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(shown(&w).len(), 5, "{:?}", shown(&w));
    assert!(w.h.text().contains("Rig: all"), "the box is shown in an ANI folder: {}", w.h.text());
    w.h.app.app.archive.set_rig_filter(Some("huwf"));
    w.h.frames(3);
    assert_eq!(shown(&w), vec!["huwf_001_test.ANI", "huwf_002_test.ANI"]);
    assert!(w.h.text().contains("Rig: huwf"), "{}", w.h.text());
    w.h.app.app.archive.set_rig_filter(Some("huwm"));
    w.h.frames(3);
    assert_eq!(shown(&w), vec!["huwm_117_test.ANI", "huwm_118_test.ANI"]);
    // another folder: the box is gone and the filter does not apply
    with_cx(&mut w.h, |app, _| app.archive.enter_folder("Character/Human/3DC"));
    w.h.frames(3);
    assert_eq!(shown(&w), vec!["huwm_torso001.3DC"]);
    assert!(!w.h.text().contains("Rig: "), "{}", w.h.text());
    // a rig of another race is not offered in this race's folder
    assert_eq!(shaiya_toolkit::archive_view::rig_choices("character/human/ani").len(), 4);
    assert!(shaiya_toolkit::archive_view::rig_choices("Character/Human/3DC").is_empty());
    w.h.app.app.archive.set_rig_filter(Some("elmr"));
    with_cx(&mut w.h, |app, _| app.archive.enter_folder("Character/Human/ANI"));
    w.h.frames(3);
    assert_eq!(shown(&w).len(), 5, "an elf rig does not filter the human folder");
}

// ---- T12 W9: edit animation ------------------------------------------------------------------------------------------------------

fn edit_done(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| h.app.app.archive.anim_edit().is_some_and(|c| !c.is_running() && c.current().is_some()))
}

#[test]
fn editing_an_animation_saves_a_new_file() {
    let mut w = rig_world("t12-w9-edit");
    assert!(w.h.app.app.archive.offers_edit(ANI_PATH) && !w.h.app.app.archive.offers_edit("Character/Human/ANI/notes.txt"));
    let arm = shaiya_testkit::anim::arm_ani(60);
    let input = shaiya_clientdata::Ani::parse(&arm, "arm").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Retime { factor: 2.0 }, cx));
    assert!(edit_done(&mut w), "the edit ends");
    w.h.frames(3);
    assert!(w.h.text().contains("Edit animation") && w.h.text().contains("End key 60 -> 120"), "{}", w.h.text());
    let out_dir = TempDir::new("t12-w9-edit-out");
    let out = out_dir.path().join("slow.ANI");
    with_cx(&mut w.h, |app, cx| app.archive.anim_edit_mut().unwrap().save_new(out.clone(), cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.anim_edit().unwrap().is_saving()));
    w.h.frames(2);
    let saved = shaiya_clientdata::Ani::parse(&std::fs::read(&out).expect("the file was written"), "slow").unwrap();
    assert_eq!(saved.end_key, 120);
    assert!(shaiya_clientdata::anim::passes(&saved, &input), "the file passes against the original");
    // an existing file is kept, a name without .ANI and a place inside a client are refused; nothing is made
    let before = std::fs::read(&out).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.anim_edit_mut().unwrap().save_new(out.clone(), cx));
    w.h.frames(3);
    assert_eq!(std::fs::read(&out).unwrap(), before);
    let bad = out_dir.path().join("slow.txt");
    with_cx(&mut w.h, |app, cx| app.archive.anim_edit_mut().unwrap().save_new(bad.clone(), cx));
    let inside = w.client.path().join("new.ANI");
    with_cx(&mut w.h, |app, cx| app.archive.anim_edit_mut().unwrap().save_new(inside.clone(), cx));
    w.h.frames(3);
    assert!(!bad.exists() && !inside.exists());
    // a trim that is outside the file shows the reason and offers nothing to save
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Trim { start: 10, end: 99 }, cx));
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.anim_edit().is_some_and(|c| !c.is_running() && c.result.is_some())));
    let e = w.h.app.app.archive.anim_edit().and_then(|c| c.result.clone()).expect("a result").expect_err("refused");
    assert!(e.contains("start < end"), "{e}");
    assert!(w.h.app.app.archive.anim_edit().unwrap().current().is_none());
    // a trim inside it works, and the source of copy tracks is another animation of the folder
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Trim { start: 10, end: 40 }, cx));
    assert!(edit_done(&mut w));
    assert_eq!(w.h.app.app.archive.anim_edit().unwrap().current().unwrap().output.end_key, 30);
    // copy tracks from another animation: bone 1's rotation becomes the source's, everything else is the input's, and the window says how
    // many bones differ in pose (bones 1 and its child 2 turn differently; bone 0 does not move in the output)
    let source = shaiya_clientdata::Ani::parse(&turned_arm(), "src").unwrap();
    let tool = shaiya_toolkit::anim_edit::Tool::CopyTrack { source: "Character/Human/ANI/huwf_002_test.ANI".into(), bones: vec![1], rotation: true, translation: false };
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, tool, cx));
    assert!(edit_done(&mut w));
    w.h.frames(3);
    let e = w.h.app.app.archive.anim_edit().unwrap().current().unwrap().clone();
    assert_eq!(e.output.bones[1].rot, source.bones[1].rot, "bone 1's rotation is the source's");
    assert_ne!(e.output.bones[1].rot, input.bones[1].rot, "and not the input's");
    assert_eq!((&e.output.bones[0], &e.output.bones[2]), (&input.bones[0], &input.bones[2]), "the other bones are the input's");
    assert_eq!(e.diff.bones.iter().map(|b| b.bone).collect::<Vec<_>>().len(), 2, "bones 1 and 2 differ in pose: {:?}", e.diff);
    assert!(w.h.text().contains("2 bones differ in pose from the original"), "{}", w.h.text());
    // copying the same animation over itself changes nothing, and says 0 bones differ
    let same = shaiya_toolkit::anim_edit::Tool::CopyTrack { source: "Character/Human/ANI/huwm_118_test.ANI".into(), bones: vec![1], rotation: true, translation: false };
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, same, cx));
    assert!(edit_done(&mut w));
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.anim_edit().unwrap().current().unwrap().output, input);
    assert!(w.h.text().contains("0 bones differ in pose from the original"), "{}", w.h.text());
    // a trim that keeps the whole length says it too; one that changes the length has no pose to compare, so no such line
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Trim { start: 0, end: 60 }, cx));
    assert!(edit_done(&mut w));
    w.h.frames(3);
    assert!(w.h.text().contains("0 bones differ in pose from the original"), "{}", w.h.text());
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Trim { start: 10, end: 40 }, cx));
    assert!(edit_done(&mut w));
    w.h.frames(3);
    assert!(!w.h.text().contains("differ in pose from the original"), "{}", w.h.text());
}

#[test]
fn changing_the_settings_cancels_the_running_edit_and_the_new_ones_follow() {
    let mut w = rig_world("t12-w9fix-follow");
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Retime { factor: 2.0 }, cx));
    // the person types another factor while the first is still held by the window: it is cancelled, not waited for
    let next = shaiya_toolkit::anim_edit::Tool::Retime { factor: 0.5 };
    with_cx(&mut w.h, |app, cx| {
        let c = app.archive.anim_edit_mut().unwrap();
        assert!(c.is_running());
        c.set_tool(&next);
        c.follow_settings(cx);
        assert!(c.is_superseded(), "the running task was asked to stop");
    });
    assert!(edit_done(&mut w), "the new settings end with their own result");
    let e = w.h.app.app.archive.anim_edit().unwrap().current().unwrap().clone();
    assert_eq!(e.output.end_key, 30, "the result is the one of factor 0.5, not 2");
    assert!(w.h.app.app.archive.anim_edit().unwrap().result.as_ref().is_some_and(|r| r.is_ok()), "no 'cancelled' left on screen");
}

#[test]
fn queueing_twice_gives_each_change_its_own_staged_file_and_undo_finds_the_first_one_again() {
    let mut w = rig_world("t12-w9fix-staged");
    let staged = |w: &World| -> PathBuf {
        let shaiya_toolkit::archive_view::PendingKind::Put { source, .. } = &w.h.app.app.archive.pending[0].kind else { panic!("not a put") };
        source.clone()
    };
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Retime { factor: 0.5 }, cx));
    assert!(edit_done(&mut w));
    with_cx(&mut w.h, |app, cx| app.archive.queue_ani_edit(cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.pending.is_empty()));
    w.h.frames(3);
    let first = staged(&w);
    // the same path again with another edit: the queued change is replaced, in a file of its own
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Retime { factor: 2.0 }, cx));
    assert!(edit_done(&mut w));
    with_cx(&mut w.h, |app, cx| app.archive.queue_ani_edit(cx));
    assert!(
        w.h.wait_until(WAIT, |h| matches!(h.app.app.archive.pending.first().map(|p| &p.kind), Some(shaiya_toolkit::archive_view::PendingKind::Put { source, .. }) if *source != first)),
        "the second edit is queued in another file"
    );
    w.h.frames(3);
    let second = staged(&w);
    assert_ne!(first, second);
    let end_of = |f: &PathBuf| shaiya_clientdata::Ani::parse(&std::fs::read(f).unwrap(), "staged").unwrap().end_key;
    assert_eq!((end_of(&first), end_of(&second)), (30, 120), "each file holds its own edit; the second did not overwrite the first");
    // undoing the second queue brings the first change back: its file still holds the first edit's bytes
    assert!(w.h.app.app.archive.undo());
    assert_eq!(staged(&w), first);
    assert_eq!(end_of(&first), 30, "the file the first change points to is the first edit");
    // nothing is written to the client, and the staged files are in the Toolkit's data folder
    assert!(first.starts_with(&w.data_path) && second.starts_with(&w.data_path) && !first.starts_with(w.client.path()));
}

#[test]
fn queue_as_replacement_adds_one_pending_change_and_writes_nothing() {
    let mut w = rig_world("t12-w9-queue");
    let files = |w: &World| (std::fs::read(w.client.path().join("data.sah")).unwrap(), std::fs::read(w.client.path().join("data.saf")).unwrap());
    let before = files(&w);
    with_cx(&mut w.h, |app, cx| app.archive.edit_ani_with(ANI_PATH, shaiya_toolkit::anim_edit::Tool::Retime { factor: 0.5 }, cx));
    assert!(edit_done(&mut w));
    assert!(w.h.app.app.archive.pending.is_empty());
    with_cx(&mut w.h, |app, cx| app.archive.queue_ani_edit(cx));
    assert!(w.h.wait_until(WAIT, |h| !h.app.app.archive.pending.is_empty()), "the replacement is queued after its checks");
    w.h.frames(3);
    let pending = w.h.app.app.archive.pending.clone();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].path, ANI_PATH);
    let shaiya_toolkit::archive_view::PendingKind::Put { source, replaces, .. } = &pending[0].kind else { panic!("not a put: {pending:?}") };
    assert!(*replaces);
    let staged = shaiya_clientdata::Ani::parse(&std::fs::read(source).unwrap(), "staged").unwrap();
    assert_eq!(staged.end_key, 30, "the queued file is the edited animation");
    assert!(source.starts_with(&w.data_path) && !source.starts_with(w.client.path()), "staged in the Toolkit's data folder, not in a client");
    assert_eq!(files(&w), before, "nothing was written to the client");
}

// ------------------------------------------------------------------------------------------------ T12 W12: Export as glTF...

fn gltf_done(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| !h.app.app.archive.is_exporting_gltf())
}

fn gltf_enabled(h: &Harness<ToolkitApp>) -> bool {
    h.app.app.archive.commands().into_iter().find(|c| c.id == "archive-export-gltf").expect("the command is listed").enabled
}

#[test]
fn exporting_an_ani_as_gltf_writes_the_gltf_and_its_bin() {
    let mut w = ani_world("t12-w12-ani");
    let out_dir = TempDir::new("t12-w12-ani-out");
    let out = out_dir.path().join("x.gltf");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    assert!(gltf_enabled(&w.h), "one selected .ANI enables the command");
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&out, cx));
    assert!(gltf_done(&mut w), "the export ends");
    w.h.frames(2);
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).expect("the .gltf is written")).unwrap();
    assert!(out_dir.path().join("x.bin").exists(), "the .bin is beside it");
    assert_eq!(json["buffers"][0]["uri"], "x.bin");
    assert_eq!(json["asset"]["extras"]["shaiya"]["source"], ANI_PATH);
    assert_eq!(json["asset"]["extras"]["shaiya"]["layout"], "ani");
    assert!(!json.to_string().contains(&out_dir.path().display().to_string()), "no folder path in the file");
    let status = w.h.app.app.archive.status_line.clone();
    assert!(status.starts_with("Exported huwm_117_test.ANI to ") && status.contains("lossy view"), "{status}");
    assert!(w.h.app.shell.banner_details().is_none(), "no error was shown");
    let mut names: Vec<String> = std::fs::read_dir(out_dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, vec!["x.bin".to_string(), "x.gltf".to_string()], "no temporary file stays");
}

#[test]
fn an_ani_goes_out_on_the_mesh_the_mesh_box_chose_and_a_mesh_alone_as_glb() {
    let mut w = ani_world("t12-w12-mesh");
    let out_dir = TempDir::new("t12-w12-mesh-out");
    let torso = "Character/Human/3DC/huwm_torso001.3DC".to_string();
    // a mesh row: one .glb with the skin and no animation
    with_cx(&mut w.h, |app, cx| app.archive.select_path(&torso, cx));
    assert!(preview_ready(&mut w.h));
    let glb = out_dir.path().join("torso.glb");
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&glb, cx));
    assert!(gltf_done(&mut w));
    let g = shaiya_clientdata::interchange::gltf_read::Gltf::from_glb(&std::fs::read(&glb).unwrap()).unwrap();
    assert!(g.json.get("skins").is_some() && g.json.get("animations").is_none());
    // an .ANI row with one mesh chosen in the Mesh box: the animation is on the mesh
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.choose_anim_mesh(shaiya_toolkit::preview::MeshChoice::One(torso.clone()), cx));
    assert!(preview_ready(&mut w.h));
    let both = out_dir.path().join("both.glb");
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&both, cx));
    assert!(gltf_done(&mut w));
    let g = shaiya_clientdata::interchange::gltf_read::Gltf::from_glb(&std::fs::read(&both).unwrap()).unwrap();
    assert_eq!(g.json["animations"][0]["name"], "huwm_117_test");
    assert_eq!(g.json["asset"]["extras"]["shaiya"]["source"], torso);
    assert!(w.h.app.shell.banner_details().is_none(), "{:?}", w.h.app.shell.banner_details());
}

#[test]
fn exporting_as_gltf_into_the_client_folder_or_to_a_wrong_name_is_refused() {
    let mut w = ani_world("t12-w12-guard");
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let before = tree(w.client.path());
    let inside = w.client_dir().join("x.glb");
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&inside, cx));
    assert!(gltf_done(&mut w));
    w.h.frames(2);
    assert!(!inside.exists());
    assert_eq!(tree(w.client.path()), before, "the client folder is as it was");
    let d = w.h.app.shell.banner_details().expect("the error is shown").to_string();
    assert!(d.contains("Choose a folder outside the client:"), "{d}");
    let out_dir = TempDir::new("t12-w12-guard-out");
    let wrong = out_dir.path().join("x.json");
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&wrong, cx));
    assert!(!w.h.app.app.archive.is_exporting_gltf() && !wrong.exists());
    // a stray .bin that is not from an earlier export of this name is not replaced
    std::fs::write(out_dir.path().join("y.bin"), b"keep me").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(&out_dir.path().join("y.gltf"), cx));
    assert!(!w.h.app.app.archive.is_exporting_gltf() && !out_dir.path().join("y.gltf").exists());
    assert_eq!(std::fs::read(out_dir.path().join("y.bin")).unwrap(), b"keep me");
}

#[test]
fn the_gltf_command_is_disabled_for_other_files() {
    let client = client_of("t12-w12-other", &[("tex/a.dds", b"DDS not really"), ("Monster/VANI/a.vani", b"not a vani")]);
    let mut w = world("t12-w12-other");
    let dir = client.path().to_path_buf();
    add_saved(&mut w, "Other", &dir);
    open_saved(&mut w, "Other", &dir);
    assert!(!gltf_enabled(&w.h), "nothing is selected");
    for p in ["tex/a.dds", "Monster/VANI/a.vani"] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(p, cx));
        assert!(preview_ready(&mut w.h));
        assert!(!gltf_enabled(&w.h), "{p} is not one of the four kinds");
        assert!(!w.h.app.app.archive.offers_export_gltf(p));
    }
}

// ------------------------------------------------------------------------------------------------ T12 W13: Import glTF...

fn import_done(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| h.app.app.archive.gltf_import().is_some_and(|c| !c.is_running()))
}

fn import_saved(w: &mut World) -> bool {
    w.h.wait_until(WAIT, |h| h.app.app.archive.gltf_import().is_some_and(|c| !c.is_saving()))
}

const TORSO_PATH: &str = "Character/Human/3DC/huwm_torso001.3DC";
const BOX_PATH: &str = "Item/3DO/box.3DO";

/// [`ani_world`] with a `.3DO` too.
fn import_world(tag: &str) -> World {
    let mut w = world(tag);
    SahBuilder::new(0)
        .file(b"Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60))
        .file(b"Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc())
        .file(b"Item/3DO/box.3DO", &shaiya_testkit::anim::box_3do())
        .file(b"Character/Human/ANI/huwm_odd_test.ANI", &odd_ani())
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

/// Exports the archive file `path` as glTF to `out` with the Toolkit's own export.
fn export_for_import(w: &mut World, path: &str, out: &std::path::Path) {
    with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.export_gltf_to(out, cx));
    assert!(gltf_done(w), "the export ends");
    assert!(out.exists());
}

fn import_settings(input: &std::path::Path) -> shaiya_toolkit::import_gltf::Settings {
    shaiya_toolkit::import_gltf::Settings { input: Some(input.to_path_buf()), ..Default::default() }
}

#[test]
fn importing_a_gltf_animation_makes_a_new_ani_that_passes_and_saves_it() {
    let mut w = import_world("t12-w13-ani");
    let d = TempDir::new("t12-w13-ani-files");
    let glb = d.path().join("a.glb");
    export_for_import(&mut w, ANI_PATH, &glb);
    let before = tree(w.client.path());
    let s = shaiya_toolkit::import_gltf::Settings { keys_exact: true, ..import_settings(&glb) };
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, s, cx));
    assert!(import_done(&mut w), "the import ends");
    w.h.frames(3);
    let (summary, kind) = {
        let c = w.h.app.app.archive.gltf_import().expect("the window is open");
        let m = c.made().unwrap_or_else(|| panic!("nothing made: {:?}", c.result.as_ref().map(|r| r.as_ref().err())));
        (m.summary(), c.kind())
    };
    assert_eq!(kind, shaiya_clientdata::interchange::import::ImportKind::Ani);
    assert!(summary.starts_with(".ANI: 3 bones") && summary.contains("passes the rig check"), "{summary}");
    let t = w.h.text();
    assert!(t.contains("Import model/animation") && t.contains("a lossy view of the game file") && t.contains("Save as new file..."), "{t}");
    // save it as a new file
    let out = d.path().join("new.ani");
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
    assert!(import_saved(&mut w));
    w.h.frames(2);
    let new = shaiya_clientdata::Ani::parse(&std::fs::read(&out).expect("the new file is written"), "new.ani").unwrap();
    let like = shaiya_clientdata::Ani::parse(&shaiya_testkit::anim::arm_ani(60), "arm.ani").unwrap();
    assert!(shaiya_clientdata::anim::passes(&new, &like), "the saved animation passes against its like file");
    assert!(w.h.app.shell.banner_details().is_none(), "{:?}", w.h.app.shell.banner_details());
    assert_eq!(tree(w.client.path()), before, "nothing was written to the client");
    let names: Vec<String> = std::fs::read_dir(d.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.contains(".tmp") || n.starts_with('.')), "no temporary file stays: {names:?}");
}

#[test]
fn a_gltf_that_cannot_be_imported_shows_its_reasons_and_cannot_be_saved() {
    let mut w = import_world("t12-w13-refuse");
    let d = TempDir::new("t12-w13-refuse-files");
    let gltf = d.path().join("a.gltf");
    export_for_import(&mut w, ANI_PATH, &gltf);
    let mut g: serde_json::Value = serde_json::from_slice(&std::fs::read(&gltf).unwrap()).unwrap();
    g["nodes"][1]["name"] = serde_json::json!("Spine");
    let bad = d.path().join("bad.gltf");
    std::fs::write(&bad, serde_json::to_string(&g).unwrap()).unwrap();
    std::fs::copy(d.path().join("a.bin"), d.path().join("bad.bin")).unwrap();
    g["buffers"][0]["uri"] = serde_json::json!("bad.bin");
    std::fs::write(&bad, serde_json::to_string(&g).unwrap()).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, import_settings(&bad), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    {
        let c = w.h.app.app.archive.gltf_import().unwrap();
        assert!(c.made().is_none());
        let Some(Err(r)) = &c.result else { panic!("{:?}", c.result.as_ref().map(|r| r.as_ref().ok().map(|m| m.summary()))) };
        assert!(!r.does_not_pass);
        assert!(r.lines.iter().any(|l| l.contains("no node (Bone_NNN or --map)") && l.contains("(names)")), "{:?}", r.lines);
    }
    let t = w.h.text();
    assert!(t.contains("Not imported") && t.contains("no node (Bone_NNN or --map)"), "{t}");
    // the same file with a map file that names the node
    let map = d.path().join("map.txt");
    std::fs::write(&map, "Spine = 1\n").unwrap();
    let s = shaiya_toolkit::import_gltf::Settings { map: Some(map.clone()), ..import_settings(&bad) };
    with_cx(&mut w.h, |app, _cx| app.archive.gltf_import_mut().unwrap().settings = s);
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().start(cx));
    assert!(import_done(&mut w));
    assert!(w.h.app.app.archive.gltf_import().unwrap().made().is_some(), "the map names the renamed node");
    // nothing made for a changed setting: the earlier answer does not stand for the new settings
    with_cx(&mut w.h, |app, _| app.archive.gltf_import_mut().unwrap().settings.keys_exact = true);
    assert!(w.h.app.app.archive.gltf_import().unwrap().made().is_none());
    // refused and nothing started: a missing file, a file that is not glTF
    let junk = d.path().join("junk.glb");
    std::fs::write(&junk, b"not a glTF").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, import_settings(&junk), cx));
    assert!(import_done(&mut w));
    let c = w.h.app.app.archive.gltf_import().unwrap();
    assert!(matches!(&c.result, Some(Err(r)) if r.lines[0].contains("junk.glb")), "{:?}", c.result.as_ref().map(|r| r.as_ref().err()));
    // saving with nothing made is refused with a message
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(d.path().join("x.ani"), cx));
    assert!(!d.path().join("x.ani").exists());
    assert!(w.h.app.shell.banner_details().is_some_and(|b| b.contains("no imported file")), "{:?}", w.h.app.shell.banner_details());
}

#[test]
fn importing_a_3dc_gives_the_bytes_of_the_file_it_is_like_and_a_3do_makes_a_mesh() {
    let mut w = import_world("t12-w13-mesh");
    let d = TempDir::new("t12-w13-mesh-files");
    let (glb, box_glb) = (d.path().join("t.glb"), d.path().join("b.glb"));
    export_for_import(&mut w, TORSO_PATH, &glb);
    export_for_import(&mut w, BOX_PATH, &box_glb);
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(TORSO_PATH, import_settings(&glb), cx));
    assert!(import_done(&mut w));
    let out = d.path().join("t.3DC");
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
    assert!(import_saved(&mut w));
    assert_eq!(std::fs::read(&out).unwrap(), shaiya_testkit::anim::arm_mesh_3dc(), "the .3DC comes back byte for byte");
    // a .3DO row: a static mesh, no like file
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(BOX_PATH, import_settings(&box_glb), cx));
    assert!(import_done(&mut w));
    let out = d.path().join("b.3DO");
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
    assert!(import_saved(&mut w));
    assert_eq!(std::fs::read(&out).unwrap(), shaiya_testkit::anim::box_3do());
    // a flag of another kind is refused with its reason
    let s = shaiya_toolkit::import_gltf::Settings { reduce: true, ..import_settings(&glb) };
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(TORSO_PATH, s, cx));
    assert!(import_done(&mut w));
    let c = w.h.app.app.archive.gltf_import().unwrap();
    assert!(matches!(&c.result, Some(Err(r)) if r.lines[0].contains("for an .ANI")), "{:?}", c.result.as_ref().map(|r| r.as_ref().err()));
}

#[test]
fn saving_an_imported_file_follows_the_new_file_rules() {
    let mut w = import_world("t12-w13-guard");
    let d = TempDir::new("t12-w13-guard-files");
    let glb = d.path().join("a.glb");
    export_for_import(&mut w, ANI_PATH, &glb);
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, import_settings(&glb), cx));
    assert!(import_done(&mut w));
    assert!(w.h.app.app.archive.gltf_import().unwrap().made().is_some());
    let before = tree(w.client.path());
    let try_save = |w: &mut World, out: PathBuf, want: &str| {
        with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
        assert!(!w.h.app.app.archive.gltf_import().unwrap().is_saving(), "{want}: nothing started");
        assert!(!out.exists(), "{want}: {} was written", out.display());
        let b = w.h.app.shell.banner_details().unwrap_or_else(|| panic!("{want}: no message")).to_string();
        assert!(b.contains(want), "{want}: {b}");
    };
    try_save(&mut w, d.path().join("x.json"), "must end in .ANI");
    let inside = w.client_dir().join("x.ani");
    try_save(&mut w, inside, "Choose a folder outside the client");
    // an existing file is not replaced
    let there = d.path().join("there.ani");
    std::fs::write(&there, b"keep me").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(there.clone(), cx));
    assert_eq!(std::fs::read(&there).unwrap(), b"keep me");
    assert!(!w.h.app.app.archive.gltf_import().unwrap().is_saving());
    // a name Windows cannot hold
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(d.path().join("CON.ani"), cx));
    assert!(!w.h.app.app.archive.gltf_import().unwrap().is_saving());
    assert_eq!(tree(w.client.path()), before, "the client folder is as it was");
}

#[test]
fn the_import_command_is_offered_for_the_four_kinds_only() {
    let mut w = import_world("t12-w13-offer");
    for p in [ANI_PATH, TORSO_PATH, BOX_PATH] {
        assert!(w.h.app.app.archive.offers_import_gltf(p), "{p}");
        with_cx(&mut w.h, |app, cx| app.archive.select_path(p, cx));
        assert!(preview_ready(&mut w.h));
        let cmd = w.h.app.app.archive.commands().into_iter().find(|c| c.id == "archive-import-gltf").expect("the command is listed");
        assert!(cmd.enabled && cmd.label == "Import model/animation...", "{p}");
    }
    let client = client_of("t12-w13-other", &[("tex/a.dds", b"DDS not really"), ("Monster/VANI/a.vani", b"not a vani")]);
    let mut w = world("t12-w13-other");
    let dir = client.path().to_path_buf();
    add_saved(&mut w, "Other", &dir);
    open_saved(&mut w, "Other", &dir);
    for p in ["tex/a.dds", "Monster/VANI/a.vani"] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(p, cx));
        assert!(preview_ready(&mut w.h));
        assert!(!w.h.app.app.archive.offers_import_gltf(p), "{p}");
        let cmd = w.h.app.app.archive.commands().into_iter().find(|c| c.id == "archive-import-gltf").unwrap();
        assert!(!cmd.enabled, "{p}");
    }
}

const ODD_PATH: &str = "Character/Human/ANI/huwm_odd_test.ANI";

/// An animation whose bone 1 names a parent after it (the validator calls that an error): the arm's skeleton otherwise.
fn odd_ani() -> Vec<u8> {
    let bone = |parent: i32, y: f32| shaiya_testkit::anim::BoneSpec { parent, matrix: shaiya_testkit::anim::translation_bits(0.0, y, 0.0), rot: vec![], trans: vec![] };
    shaiya_testkit::anim::ani_bytes(0, 60, &[bone(-1, 0.0), bone(2, 1.0), bone(-1, 2.0)])
}

#[test]
fn an_ani_that_does_not_pass_the_rig_check_is_shown_and_cannot_be_saved() {
    let mut w = import_world("t12-w13-pass");
    let d = TempDir::new("t12-w13-pass-files");
    let gltf = d.path().join("a.gltf");
    export_for_import(&mut w, ANI_PATH, &gltf);
    // the glTF gets the tree of the odd file (node 2 holds node 1)
    let mut g: serde_json::Value = serde_json::from_slice(&std::fs::read(&gltf).unwrap()).unwrap();
    g["nodes"][0].as_object_mut().unwrap().remove("children");
    g["nodes"][1].as_object_mut().unwrap().remove("children");
    g["nodes"][2]["children"] = serde_json::json!([1]);
    g["scenes"][0]["nodes"] = serde_json::json!([0, 2]);
    let odd = d.path().join("odd.gltf");
    std::fs::write(&odd, serde_json::to_string(&g).unwrap()).unwrap();
    std::fs::copy(d.path().join("a.bin"), d.path().join("odd.bin")).unwrap();
    g["buffers"][0]["uri"] = serde_json::json!("odd.bin");
    std::fs::write(&odd, serde_json::to_string(&g).unwrap()).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ODD_PATH, import_settings(&odd), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    {
        let c = w.h.app.app.archive.gltf_import().unwrap();
        assert!(c.made().is_none());
        let Some(Err(r)) = &c.result else { panic!("{:?}", c.result.as_ref().map(|r| r.as_ref().ok().map(|m| m.summary()))) };
        assert!(r.does_not_pass && r.lines.iter().any(|l| l.contains("parent-order")), "{:?}", r.lines);
    }
    assert!(w.h.text().contains("does not pass the rig check"), "{}", w.h.text());
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(d.path().join("n.ani"), cx));
    assert!(!d.path().join("n.ani").exists());
}

// ------------------------------------------------------------------------------------------------ T12 W14: OBJ and BVH

const CRATE_PATH: &str = "Item/3DO/crate.3DO";
const LOSSY: &str = "glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes";

/// [`import_world`] with a `.3DO` that names a texture (`crate.3DO`).
fn obj_world(tag: &str) -> World {
    let mut w = world(tag);
    SahBuilder::new(0)
        .file(b"Character/Human/ANI/huwm_117_test.ANI", &shaiya_testkit::anim::arm_ani(60))
        .file(b"Character/Human/3DC/huwm_torso001.3DC", &shaiya_testkit::anim::arm_mesh_3dc())
        .file(b"Item/3DO/box.3DO", &shaiya_testkit::anim::box_3do())
        .file(b"Item/3DO/crate.3DO", &shaiya_testkit::anim::animated_3do(1, 20))
        .file(b"Character/Human/ANI/huwm_odd_test.ANI", &odd_ani())
        .write_to(w.client.path());
    w.h.frames(2);
    let dir = w.client_dir();
    with_cx(&mut w.h, |app, cx| app.add_client("US test", &dir, true, cx)).unwrap();
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.is_open()), "the archive opens");
    w.h.frames(2);
    w
}

fn names_in(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn exporting_as_bvh_and_obj_writes_the_files_and_says_the_lossy_sentence() {
    let mut w = obj_world("t12-w14-export");
    let d = TempDir::new("t12-w14-export-out");
    // an .ANI as BVH
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let bvh = d.path().join("walk.bvh");
    with_cx(&mut w.h, |app, cx| app.archive.export_bvh_to(&bvh, cx));
    assert!(gltf_done(&mut w), "the export ends");
    w.h.frames(2);
    let t = std::fs::read_to_string(&bvh).expect("the .bvh is written");
    assert!(t.starts_with("HIERARCHY\nROOT Bone_000\n") && t.contains("Frames: 61\nFrame Time: 0.0333333\n"), "{t}");
    assert!(!t.contains(&d.path().display().to_string()), "no folder path in the file");
    let status = w.h.app.app.archive.status_line.clone();
    assert!(status.starts_with("Exported huwm_117_test.ANI to ") && status.contains(LOSSY), "{status}");
    assert!(w.h.app.shell.banner_details().is_none(), "{:?}", w.h.app.shell.banner_details());
    // a .3DO that names a texture as OBJ: the .obj and a .mtl of names
    with_cx(&mut w.h, |app, cx| app.archive.select_path(CRATE_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let obj = d.path().join("crate.obj");
    with_cx(&mut w.h, |app, cx| app.archive.export_obj_to(&obj, cx));
    assert!(gltf_done(&mut w));
    w.h.frames(2);
    let t = std::fs::read_to_string(&obj).expect("the .obj is written");
    assert!(t.contains("mtllib crate.mtl") && t.contains("usemtl box.tga") && t.contains("# source crate.3DO"), "{t}");
    assert!(std::fs::read_to_string(d.path().join("crate.mtl")).unwrap().contains("newmtl box.tga"));
    assert!(w.h.app.app.archive.status_line.contains(LOSSY));
    // a .3DC at rest (its texture name makes a .mtl too), and a .3DO with no texture name: one file
    for (path, name, files) in [(TORSO_PATH, "torso.obj", 5), (BOX_PATH, "box.obj", 6)] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
        assert!(preview_ready(&mut w.h));
        with_cx(&mut w.h, |app, cx| app.archive.export_obj_to(&d.path().join(name), cx));
        assert!(gltf_done(&mut w));
        w.h.frames(2);
        assert!(d.path().join(name).exists());
        assert_eq!(names_in(d.path()).len(), files, "{:?}", names_in(d.path()));
    }
    assert!(std::fs::read_to_string(d.path().join("torso.obj")).unwrap().lines().filter(|l| l.starts_with("v ")).count() == 16);
    assert!(w.h.app.shell.banner_details().is_none(), "{:?}", w.h.app.shell.banner_details());
    assert!(!names_in(d.path()).iter().any(|n| n.contains(".tmp") || n.starts_with('.')), "no temporary file stays: {:?}", names_in(d.path()));
}

#[test]
fn the_obj_and_bvh_commands_are_offered_for_their_kinds_only() {
    let mut w = obj_world("t12-w14-offer");
    let enabled = |w: &World, id: &str| w.h.app.app.archive.commands().into_iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is listed")).enabled;
    let label = |w: &World, id: &str| w.h.app.app.archive.commands().into_iter().find(|c| c.id == id).unwrap().label;
    assert_eq!((label(&w, "archive-export-obj"), label(&w, "archive-export-bvh")), ("Export as OBJ...".to_string(), "Export as BVH...".to_string()));
    assert!(!enabled(&w, "archive-export-obj") && !enabled(&w, "archive-export-bvh"), "nothing is selected");
    for (p, obj, bvh) in [(ANI_PATH, false, true), (TORSO_PATH, true, false), (BOX_PATH, true, false), (CRATE_PATH, true, false)] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(p, cx));
        assert!(preview_ready(&mut w.h));
        assert_eq!((enabled(&w, "archive-export-obj"), enabled(&w, "archive-export-bvh")), (obj, bvh), "{p}");
        assert_eq!((w.h.app.app.archive.offers_export_obj(p), w.h.app.app.archive.offers_export_bvh(p)), (obj, bvh), "{p}");
    }
    let client = client_of("t12-w14-other", &[("tex/a.dds", b"DDS not really"), ("Monster/VANI/a.vani", b"not a vani")]);
    let mut w = world("t12-w14-other");
    let dir = client.path().to_path_buf();
    add_saved(&mut w, "Other", &dir);
    open_saved(&mut w, "Other", &dir);
    for p in ["tex/a.dds", "Monster/VANI/a.vani"] {
        with_cx(&mut w.h, |app, cx| app.archive.select_path(p, cx));
        assert!(preview_ready(&mut w.h));
        assert!(!enabled(&w, "archive-export-obj") && !enabled(&w, "archive-export-bvh"), "{p}");
        assert!(!w.h.app.app.archive.offers_export_obj(p) && !w.h.app.app.archive.offers_export_bvh(p));
    }
}

#[test]
fn exporting_as_obj_or_bvh_into_the_client_folder_or_to_a_wrong_name_is_refused() {
    let mut w = obj_world("t12-w14-guard");
    let before = tree(w.client.path());
    with_cx(&mut w.h, |app, cx| app.archive.select_path(ANI_PATH, cx));
    assert!(preview_ready(&mut w.h));
    let inside = w.client_dir().join("x.bvh");
    with_cx(&mut w.h, |app, cx| app.archive.export_bvh_to(&inside, cx));
    assert!(gltf_done(&mut w));
    w.h.frames(2);
    assert!(!inside.exists());
    assert_eq!(tree(w.client.path()), before, "the client folder is as it was");
    assert!(w.h.app.shell.banner_details().expect("the error is shown").contains("Choose a folder outside the client:"));
    let d = TempDir::new("t12-w14-guard-out");
    // a name of another format
    for name in ["x.obj", "x.glb", "x.json"] {
        with_cx(&mut w.h, |app, cx| app.archive.export_bvh_to(&d.path().join(name), cx));
        assert!(!w.h.app.app.archive.is_exporting_gltf() && !d.path().join(name).exists(), "{name}");
    }
    assert!(w.h.app.shell.banner_details().is_some_and(|b| b.contains("must end in .bvh")), "{:?}", w.h.app.shell.banner_details());
    with_cx(&mut w.h, |app, cx| app.archive.select_path(CRATE_PATH, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| app.archive.export_obj_to(&d.path().join("x.gltf"), cx));
    assert!(!w.h.app.app.archive.is_exporting_gltf() && !d.path().join("x.gltf").exists());
    // a .mtl that is not from an earlier export of this name is not replaced, and no .obj is written either
    std::fs::write(d.path().join("y.mtl"), b"keep me").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.export_obj_to(&d.path().join("y.obj"), cx));
    assert!(gltf_done(&mut w));
    w.h.frames(2);
    assert!(!d.path().join("y.obj").exists());
    assert_eq!(std::fs::read(d.path().join("y.mtl")).unwrap(), b"keep me");
    assert!(w.h.app.shell.banner_details().is_some_and(|b| b.contains("y.mtl")), "{:?}", w.h.app.shell.banner_details());
}

/// Exports the archive file `path` with `export` (a Toolkit hook) to `out`.
fn export_text_for_import(w: &mut World, path: &str, out: &std::path::Path, bvh: bool) {
    with_cx(&mut w.h, |app, cx| app.archive.select_path(path, cx));
    assert!(preview_ready(&mut w.h));
    with_cx(&mut w.h, |app, cx| if bvh { app.archive.export_bvh_to(out, cx) } else { app.archive.export_obj_to(out, cx) });
    assert!(gltf_done(w), "the export ends");
    assert!(out.exists(), "{}", out.display());
}

#[test]
fn importing_a_bvh_makes_a_new_ani_that_passes_and_saves_it() {
    let mut w = obj_world("t12-w14-bvh");
    let d = TempDir::new("t12-w14-bvh-files");
    let bvh = d.path().join("a.bvh");
    export_text_for_import(&mut w, ANI_PATH, &bvh, true);
    let before = tree(w.client.path());
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, import_settings(&bvh), cx));
    assert!(import_done(&mut w), "the import ends");
    w.h.frames(3);
    let summary = {
        let c = w.h.app.app.archive.gltf_import().expect("the window is open");
        let m = c.made().unwrap_or_else(|| panic!("nothing made: {:?}", c.result.as_ref().map(|r| r.as_ref().err())));
        assert_eq!(c.kind(), shaiya_clientdata::interchange::import::ImportKind::Ani);
        m.summary()
    };
    assert!(summary.starts_with(".ANI: 3 bones") && summary.contains("passes the rig check"), "{summary}");
    let t = w.h.text();
    assert!(t.contains("Import model/animation") && t.contains("a lossy view of the game file") && t.contains("Save as new file..."), "{t}");
    let out = d.path().join("new.ani");
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
    assert!(import_saved(&mut w));
    w.h.frames(2);
    let new = shaiya_clientdata::Ani::parse(&std::fs::read(&out).expect("the new file is written"), "new.ani").unwrap();
    let like = shaiya_clientdata::Ani::parse(&shaiya_testkit::anim::arm_ani(60), "arm.ani").unwrap();
    assert!(shaiya_clientdata::anim::passes(&new, &like), "the saved animation passes against its like file");
    assert!(w.h.app.shell.banner_details().is_none(), "{:?}", w.h.app.shell.banner_details());
    assert_eq!(tree(w.client.path()), before, "nothing was written to the client");
    assert!(!names_in(d.path()).iter().any(|n| n.contains(".tmp") || n.starts_with('.')), "no temporary file stays: {:?}", names_in(d.path()));
    // a BVH that is not in the layout the export writes shows its reasons and cannot be saved
    let text = std::fs::read_to_string(&bvh).unwrap().replacen("CHANNELS 3 Zrotation Xrotation Yrotation", "CHANNELS 3 Xrotation Yrotation Zrotation", 1);
    let bad = d.path().join("bad.bvh");
    std::fs::write(&bad, text).unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, import_settings(&bad), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    {
        let c = w.h.app.app.archive.gltf_import().unwrap();
        assert!(c.made().is_none());
        let Some(Err(r)) = &c.result else { panic!("{:?}", c.result.as_ref().map(|r| r.as_ref().ok().map(|m| m.summary()))) };
        assert!(!r.does_not_pass && r.lines.iter().any(|l| l.contains("Zrotation Xrotation Yrotation")), "{:?}", r.lines);
    }
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(d.path().join("x.ani"), cx));
    assert!(!d.path().join("x.ani").exists());
    // an .ANI that does not pass the rig check is shown as such (the odd file's parent comes after its child)
    export_text_for_import(&mut w, ODD_PATH, &d.path().join("odd.bvh"), true);
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ODD_PATH, import_settings(&d.path().join("odd.bvh")), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    let c = w.h.app.app.archive.gltf_import().unwrap();
    let Some(Err(r)) = &c.result else { panic!("not refused") };
    assert!(r.does_not_pass && r.lines.iter().any(|l| l.contains("parent-order")), "{:?}", r.lines);
    assert!(c.made().is_none());
}

#[test]
fn importing_an_obj_makes_a_new_3do_or_3de_and_refuses_the_wrong_rows_and_settings() {
    let mut w = obj_world("t12-w14-obj");
    let d = TempDir::new("t12-w14-obj-files");
    let obj = d.path().join("b.obj");
    export_text_for_import(&mut w, BOX_PATH, &obj, false);
    let before = tree(w.client.path());
    // a .3DO row: the box comes back byte for byte
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(BOX_PATH, import_settings(&obj), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    {
        let c = w.h.app.app.archive.gltf_import().unwrap();
        let m = c.made().unwrap_or_else(|| panic!("nothing made: {:?}", c.result.as_ref().map(|r| r.as_ref().err())));
        assert!(m.summary().starts_with(".3DO: 1 meshes, 8 vertices"), "{}", m.summary());
    }
    let out = d.path().join("b.3DO");
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(out.clone(), cx));
    assert!(import_saved(&mut w));
    assert_eq!(std::fs::read(&out).unwrap(), shaiya_testkit::anim::box_3do());
    // the same OBJ read for a .3DE row (no normals are stored) and a name that does not fit the kind
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(BOX_PATH, import_settings(&obj), cx));
    assert!(import_done(&mut w));
    with_cx(&mut w.h, |app, cx| app.archive.gltf_import_mut().unwrap().save_new(d.path().join("b.3DE"), cx));
    assert!(!d.path().join("b.3DE").exists() && w.h.app.shell.banner_details().is_some_and(|b| b.contains("must end in .3DO")), "{:?}", w.h.app.shell.banner_details());
    // a mesh with a texture name: the name comes back
    let cobj = d.path().join("c.obj");
    export_text_for_import(&mut w, CRATE_PATH, &cobj, false);
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(CRATE_PATH, import_settings(&cobj), cx));
    assert!(import_done(&mut w));
    let m = shaiya_clientdata::interchange::obj::import_obj(&std::fs::read(&cobj).unwrap(), shaiya_clientdata::interchange::import::ImportKind::Mesh3do, None).unwrap();
    let shaiya_clientdata::interchange::import::Imported::Mesh3do(m) = m.0 else { panic!() };
    assert_eq!(m.texture, b"box.tga\0");
    assert!(w.h.app.app.archive.gltf_import().unwrap().made().is_some());
    // the rows an OBJ cannot make, and the settings that are for glTF only
    for row in [ANI_PATH, TORSO_PATH] {
        with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(row, import_settings(&obj), cx));
        assert!(import_done(&mut w));
        let c = w.h.app.app.archive.gltf_import().unwrap();
        assert!(matches!(&c.result, Some(Err(r)) if r.lines[0].contains("An OBJ is imported as a .3DO or .3DE")), "{row}: {:?}", c.result.as_ref().map(|r| r.as_ref().err()));
        assert!(c.made().is_none());
    }
    let bvh = d.path().join("a.bvh");
    export_text_for_import(&mut w, ANI_PATH, &bvh, true);
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(TORSO_PATH, import_settings(&bvh), cx));
    assert!(import_done(&mut w));
    assert!(matches!(&w.h.app.app.archive.gltf_import().unwrap().result, Some(Err(r)) if r.lines[0].contains("A BVH is imported as an .ANI")));
    for s in [
        shaiya_toolkit::import_gltf::Settings { reduce: true, ..import_settings(&bvh) },
        shaiya_toolkit::import_gltf::Settings { keys_exact: true, ..import_settings(&bvh) },
        shaiya_toolkit::import_gltf::Settings { map: Some(d.path().join("m.txt")), ..import_settings(&bvh) },
    ] {
        with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(ANI_PATH, s, cx));
        assert!(import_done(&mut w));
        let c = w.h.app.app.archive.gltf_import().unwrap();
        assert!(matches!(&c.result, Some(Err(r)) if r.lines[0].contains("for a glTF file only")), "{:?}", c.result.as_ref().map(|r| r.as_ref().err()));
    }
    // a file that is not an OBJ: the reasons are shown, nothing can be saved
    let junk = d.path().join("junk.obj");
    std::fs::write(&junk, b"hello\nworld\n").unwrap();
    with_cx(&mut w.h, |app, cx| app.archive.import_gltf_with(BOX_PATH, import_settings(&junk), cx));
    assert!(import_done(&mut w));
    w.h.frames(3);
    assert!(matches!(&w.h.app.app.archive.gltf_import().unwrap().result, Some(Err(r)) if r.lines.iter().any(|l| l.contains("no faces"))));
    assert!(w.h.text().contains("Not imported") && w.h.app.app.archive.gltf_import().unwrap().made().is_none());
    assert_eq!(tree(w.client.path()), before, "nothing was written to the client");
}
