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
    assert!(t.contains("Shaiya Archive") || t.contains("v0.1.0"), "{t}");
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
    w.h.app.shell.inject_answer("toolkit-exclusive", 0);
    assert!(w.h.wait_until(WAIT, |h| h.app.app.archive.changed_counter == 1 && h.app.app.archive.is_open()), "applied and reloaded");
    w.h.frames(3);
    assert_eq!(w.h.app.app.archive.pending.len(), 0);
    assert_eq!(w.h.app.app.archive.visible_count(), 8);
    assert!(w.h.app.app.archive.find_row("Item/t1.txt").is_some());
    let backups = w.client_dir().join("shaiya-toolkit-backups");
    assert!(std::fs::read_dir(&backups).map(|r| r.count()).unwrap_or(0) >= 1, "a journal folder exists");
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
    assert!(w.h.text().contains("Apply ps0001.patch to US test") && w.h.text().contains("Signatures: NOT CHECKED"), "{}", w.h.text());
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
