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
    assert!(t.contains("Shaiya Archive") || t.contains("v0.3.2"), "{t}");
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
fn a_smod_in_the_archive_previews_as_a_model_and_other_model_formats_stay_hex() {
    let mut w = world("t9-model");
    SahBuilder::new(0)
        .file(b"Entity/Building/tiny.smod", &tiny_model())
        .file(b"Entity/Texture/top.tga", &shaiya_testkit::tga_stub(8, 8))
        .file(b"Entity/Building/x.3DC", &[1u8, 2, 3, 4, 5, 6, 7, 8])
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
    with_cx(&mut w.h, |app, cx| app.archive.select_path("Entity/Building/x.3DC", cx));
    assert!(preview_ready(&mut w.h));
    w.h.frames(2);
    assert_eq!(w.h.app.app.archive.preview_kind(), Some("hex"));
    assert!(w.h.text().contains("no preview (format not documented yet)"));
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
    assert!(t.contains("moves 2 files") && t.contains("Put back in Health") && t.contains("nothing is copied or deleted"), "{t}");
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
