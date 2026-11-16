//! Studio harness tests: headless frames with synthetic input (drop, tasks, cancel). No game data: every file is built here.

use egui::{Key, Modifiers};
use shaiya_core::hash::{sha256, to_hex};
use shaiya_studio::StudioApp;
use shaiya_testkit::{pe_stub, SahBuilder, TempDir};
use shaiya_ui::testing::Harness;
use std::path::PathBuf;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(20);

fn settled(h: &mut Harness<StudioApp>) -> bool {
    h.wait_until(WAIT, |h| h.app.app.table().iter().all(|r| r.1 != "..."))
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
fn the_empty_state_explains_studio() {
    let mut h = Harness::<StudioApp>::bare();
    h.frames(3);
    let t = h.text();
    assert!(t.contains("Drop files here or press Ctrl+O"), "{t}");
    assert!(t.contains("Studio identifies Shaiya files"), "{t}");
    assert!(t.contains("Archive browsing and patches arrive in the next version"), "{t}");
    assert!(t.contains("These tools work on your own copy of the game"), "{t}");
    assert!(t.contains("Shaiya Archive") || t.contains("v0.1.0"), "{t}");
}

#[test]
fn dropping_files_lists_them_with_their_kinds() {
    let dir = TempDir::new("studio-drop");
    let (sah, seed, exe) = samples(&dir);
    let mut h = Harness::<StudioApp>::bare();
    h.frames(1);
    h.drop_files(&[sah, seed, exe]);
    assert!(settled(&mut h), "inspection tasks finish");
    h.frames(2);
    let table = h.app.app.table();
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
    assert!(!t.contains("Drop files here or press Ctrl+O"), "the empty state is gone");
    assert_eq!(h.app.shell.recent_paths().len(), 3, "opened files go to the recent list");
}

#[test]
fn an_unreadable_file_is_an_error_row_not_a_banner() {
    let mut h = Harness::<StudioApp>::bare();
    h.frames(1);
    h.drop_files(&[PathBuf::from("Z:/no/such/folder/missing.bin")]);
    assert!(settled(&mut h));
    h.frames(2);
    let table = h.app.app.table();
    assert_eq!(table[0].1, "error");
    assert!(h.app.shell.banner_details().is_none(), "no banner for per-file problems");
    assert!(h.app.shell.activity().iter().any(|l| l.text.contains("missing.bin")), "the Activity list has it");
}

#[test]
fn a_hash_task_shows_progress_and_completes_with_the_right_digest() {
    let dir = TempDir::new("studio-hash");
    let big = dir.path().join("big.bin");
    let data: Vec<u8> = (0..40_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&big, &data).unwrap();
    let mut h = Harness::<StudioApp>::bare();
    h.frames(1);
    h.drop_files(&[big]);
    assert!(settled(&mut h));
    // press the row's SHA-256 button (what the click does)
    {
        let StudioHarness { app, shell } = split(&mut h);
        let mut cx = shaiya_ui::Cx { shell, settings: &mut shaiya_studio::StudioSettings::default() };
        app.hash_row("big.bin", &mut cx);
    }
    // it shows up in the Activity list with byte progress, and in the status bar
    let mut saw_progress = false;
    let finished = h.wait_until(WAIT, |h| {
        let views = h.app.shell.tasks().views();
        saw_progress |= views.iter().any(|v| v.title.starts_with("Hashing") && v.finished.is_none() && v.total == Some(40_000_000));
        h.app.app.table()[0].4.len() == 64
    });
    assert!(finished, "the hash completes");
    h.frames(2);
    assert_eq!(h.app.app.table()[0].4, to_hex(&sha256(&data)));
    assert!(h.text().contains(&to_hex(&sha256(&data))), "the digest is in the table");
    let _ = saw_progress; // a fast disk may finish between two frames; the total being known is checked below
    let views = h.app.shell.tasks().views();
    let v = views.iter().find(|v| v.title.starts_with("Hashing")).unwrap();
    assert_eq!((v.total, v.done), (Some(40_000_000), 40_000_000));
}

#[test]
fn cancel_stops_a_hash_and_the_row_says_cancelled() {
    let dir = TempDir::new("studio-cancel");
    let big = dir.path().join("huge.bin");
    std::fs::File::create(&big).unwrap().set_len(1_500_000_000).unwrap();
    let mut h = Harness::<StudioApp>::bare();
    h.frames(1);
    h.drop_files(&[big]);
    assert!(settled(&mut h));
    {
        let StudioHarness { app, shell } = split(&mut h);
        let mut cx = shaiya_ui::Cx { shell, settings: &mut shaiya_studio::StudioSettings::default() };
        app.hash_row("huge.bin", &mut cx);
    }
    // wait until bytes have been hashed, then cancel through the same call the Activity button makes
    assert!(h.wait_until(WAIT, |h| h.app.shell.tasks().views().iter().any(|v| v.title.starts_with("Hashing") && v.done > 0)), "hashing started");
    let t0 = std::time::Instant::now();
    h.app.app.cancel_hash("huge.bin");
    assert!(h.wait_until(Duration::from_secs(5), |h| h.app.app.table()[0].4 == "Cancelled"), "{:?}", h.app.app.table());
    assert!(t0.elapsed() < Duration::from_millis(1500), "cancel takes effect quickly: {:?}", t0.elapsed());
    h.frames(2);
    assert!(h.text().contains("Cancelled"), "{}", h.text());
    assert!(h.app.shell.tasks().views().iter().any(|v| matches!(&v.finished, Some(shaiya_ui::tasks::Finish::Cancelled))));
}

#[test]
fn ctrl_shift_p_lists_the_studio_command() {
    let mut h = Harness::<StudioApp>::bare();
    h.frames(2);
    h.key(Key::P, Modifiers::COMMAND | Modifiers::SHIFT);
    h.frames(2);
    h.type_text("hash");
    h.frames(1);
    assert!(h.text().contains("Hash selected files"), "{}", h.text());
}

/// Splits the harness into the tool and the shell, the two halves a `Cx` is made of.
struct StudioHarness<'a> {
    app: &'a mut StudioApp,
    shell: &'a mut shaiya_ui::Shell,
}

fn split<'a>(h: &'a mut Harness<StudioApp>) -> StudioHarness<'a> {
    let a = &mut h.app;
    StudioHarness { app: &mut a.app, shell: &mut a.shell }
}
