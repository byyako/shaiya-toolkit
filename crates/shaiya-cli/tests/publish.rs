//! `shaiya patch publish`: plan, --dry-run, the write, refusals (exit 4), bad input (exit 3), JSON shape.

mod common;

use common::*;
use std::path::{Path, PathBuf};

/// `ps<number>.patch` built by the CLI from one file whose content depends on `tag`.
fn build_patch(w: &World, number: u32, tag: &str, content: &[u8]) -> PathBuf {
    let files = w.tmp.path().join(format!("files-{number}-{tag}"));
    write(&files.join("Item/Item.SData"), content);
    let dir = w.tmp.path().join(format!("src-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join(format!("ps{number:04}.patch"));
    let o = sh(&w.data, &["patch", "build", "-o", p(&out), "--files", p(&files)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    out
}

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    if dir.exists() {
        snapshot(dir)
    } else {
        Vec::new()
    }
}

#[test]
fn dry_run_shows_the_plan_and_writes_nothing() {
    let w = world();
    let patch = build_patch(&w, 110, "a", b"item bytes");
    let dest = w.tmp.path().join("server");
    let o = sh(&w.data, &["patch", "publish", p(&patch), "--to", p(&dest), "--check-version", "3", "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("would publish 1 patch") && t.contains("ps0110.patch") && t.contains("PatchFileVersion new file -> 110"), "{t}");
    assert!(!dest.exists(), "a dry run creates nothing");
    let o = sh(&w.data, &["--json", "patch", "publish", p(&patch), "--to", p(&dest), "--check-version", "3", "--dry-run"]);
    let v = json(&o);
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["plan"]["patch_file_version_after"], 110);
    assert_eq!(v["plan"]["items"][0]["patch"]["state"], "new");
    assert!(v.get("result").is_none());
    assert!(!dest.exists());
}

#[test]
fn publishing_writes_the_patch_and_the_ini_and_reads_them_back() {
    let w = world();
    let patch = build_patch(&w, 110, "a", b"item bytes");
    let dest = w.tmp.path().join("server");
    let o = sh(&w.data, &["--json", "patch", "publish", p(&patch), "--to", p(&dest), "--check-version", "3", "--updater-version", "2"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert_eq!(v["result"]["read_back"], true);
    assert_eq!(v["result"]["patch_file_version"], 110);
    assert_eq!(std::fs::read(dest.join("Shaiya/patch/ps0110.patch")).unwrap(), std::fs::read(&patch).unwrap());
    assert_eq!(std::fs::read(dest.join("Shaiya/UpdateVersion.ini")).unwrap(), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=2\r\nPatchFileVersion=110\r\n");
    // again: nothing to do, exit 0, nothing changes
    let before = tree(&dest);
    let o = sh(&w.data, &["patch", "publish", p(&patch), "--to", p(&dest)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("already published") && out(&o).contains("unchanged"), "{}", out(&o));
    assert_eq!(tree(&dest), before);
}

#[test]
fn a_gap_a_changed_check_version_and_a_client_destination_are_refused_with_exit_4() {
    let w = world();
    let dest = w.tmp.path().join("server");
    let first = build_patch(&w, 109, "a", b"one");
    assert_eq!(code(&sh(&w.data, &["patch", "publish", p(&first), "--to", p(&dest), "--check-version", "3"])), 0);
    let before = tree(&dest);

    let gap = build_patch(&w, 111, "b", b"gap");
    let o = sh(&w.data, &["patch", "publish", p(&gap), "--to", p(&dest)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("gap") && err(&o).contains("ps0110.patch"), "{}", err(&o));

    let next = build_patch(&w, 110, "c", b"next");
    let o = sh(&w.data, &["patch", "publish", p(&next), "--to", p(&dest), "--check-version", "4"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("reinstall"), "{}", err(&o));
    let o = sh(&w.data, &["--json", "patch", "publish", p(&next), "--to", p(&dest), "--check-version", "4"]);
    assert_eq!(json(&o)["error"]["kind"], "refused");
    assert_eq!(tree(&dest), before, "a refusal writes nothing");

    // a saved client and a folder that holds archive files
    save_client(&w, "mine");
    let inside = w.client.join("web");
    let o = sh(&w.data, &["patch", "publish", p(&next), "--to", p(&inside), "--check-version", "3"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("client folder"), "{}", err(&o));
    assert!(!inside.exists());

    // a client in DIR/Shaiya: the default layout would write into it
    let parent = w.tmp.path().join("parent");
    let nested = parent.join("Shaiya");
    std::fs::create_dir_all(&nested).unwrap();
    shaiya_testkit::SahBuilder::new(0).file(b"a/b.txt", b"hi").write_to(&nested);
    let o = sh(&w.data, &["patch", "publish", p(&next), "--to", p(&parent), "--check-version", "3"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("client folder"), "{}", err(&o));
    assert!(!nested.join("UpdateVersion.ini").exists() && !nested.join("patch").exists());

    // the flag allows the change on purpose
    let o = sh(&w.data, &["patch", "publish", p(&next), "--to", p(&dest), "--check-version", "4", "--new-check-version"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("told to reinstall"), "{}", err(&o));
}

#[test]
fn bad_input_is_exit_3_and_a_new_ini_without_check_version_is_refused() {
    let w = world();
    let dest = w.tmp.path().join("server");
    let o = sh(&w.data, &["patch", "publish", p(&w.tmp.path().join("ps0001.patch")), "--to", p(&dest), "--check-version", "3"]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    let patch = build_patch(&w, 1, "a", b"x");
    let o = sh(&w.data, &["patch", "publish", p(&patch), "--to", p(&dest)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert!(err(&o).contains("--check-version"), "{}", err(&o));
    assert!(!dest.exists());
}

#[test]
fn the_flat_layout_and_file_findings_as_warnings() {
    let w = world();
    let plain: Vec<u8> = (0..200u32).map(|i| (i * 7 + 1) as u8).collect();
    let patch = build_patch(&w, 5, "a", &shaiya_seed::encrypt(&shaiya_seed::encrypt(&plain)));
    let dest = w.tmp.path().join("flat");
    let o = sh(&w.data, &["--json", "patch", "publish", p(&patch), "--to", p(&dest), "--check-version", "1", "--layout", "flat"]);
    assert_eq!(code(&o), 0, "findings never block: {}", err(&o));
    let v = json(&o);
    assert_eq!(v["warnings"][0]["code"], "sdata-double-encrypted");
    assert!(dest.join("ps0005.patch").is_file() && dest.join("UpdateVersion.ini").is_file());
    assert!(!dest.join("Shaiya").exists());
}
