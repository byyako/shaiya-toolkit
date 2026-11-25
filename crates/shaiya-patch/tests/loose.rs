//! A patch applied to a loose data folder: one change by path through the loose journal, undo, and the refusals. Synthetic files only.

use shaiya_archive::journal::State;
use shaiya_archive::loose::{self, LooseLayout};
use shaiya_archive::loose_edit::{self as le, LooseJournal, LooseOptions};
use shaiya_core::{Error, Job, Result};
use shaiya_patch::loose::{apply_loose, plan_loose};
use shaiya_patch::build;
use shaiya_testkit::TempDir;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn ok(_: &str, _: &[u8]) -> Result<()> {
    Ok(())
}

fn put_file(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn snap(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, at: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let Ok(rd) = fs::read_dir(at) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if e.file_type().unwrap().is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(fs::read(&p).unwrap()));
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(dir, dir, &mut m);
    m
}

fn world(t: &TempDir) -> LooseLayout {
    let root = t.path().join("c");
    put_file(&root, "data/Item/Item.SData", b"old item data");
    put_file(&root, "data/Item/gone.txt", b"drop me");
    put_file(&root, "data/Character/Human/a.bin", b"a");
    put_file(&root, "data/World/w.bin", b"w");
    let mut l = loose::detect(&root).unwrap().unwrap();
    l.backups = Some(t.path().join("backups"));
    l
}

fn opts() -> LooseOptions {
    LooseOptions { confirmed_unknown_busy: true, ..LooseOptions::default() }
}

/// `files/` (archive paths), `loose/` (files for a folder of the data folder) and `remove.txt`, built into `ps0001.patch`.
fn make_patch(base: &Path, files: &[(&str, &[u8])], loose_files: &[(&str, &[u8])], removes: &str) -> PathBuf {
    let (fd, ld) = (base.join("files"), base.join("loose"));
    for (p, b) in files {
        put_file(&fd, p, b);
    }
    for (p, b) in loose_files {
        put_file(&ld, p, b);
    }
    let rm = base.join("remove.txt");
    fs::write(&rm, removes).unwrap();
    let out = base.join("ps0001.patch");
    build((!files.is_empty()).then_some(fd.as_path()), (!loose_files.is_empty()).then_some(ld.as_path()), (!removes.is_empty()).then_some(rm.as_path()), &out).unwrap();
    out
}

#[test]
fn a_patch_applies_to_a_plain_loose_folder_and_rolls_back() {
    let t = TempDir::new("w17-patch");
    let l = world(&t);
    let before = snap(&l.data);
    let patch = make_patch(&t.path().join("p"), &[("Item/Item.SData", b"new item data!"), ("Character/New/file.bin", &[1, 2, 3])], &[("Item/readme.txt", b"hi")], "Item/gone.txt\nnot/there.txt\n");
    // the plan: one change, the missing removal is left out and named
    let (plan, missing) = plan_loose(&patch, &l, &ok, &Job::silent()).unwrap();
    assert_eq!(plan.counts(), (2, 1, 1));
    assert_eq!(missing, vec!["not/there.txt".to_string()]);
    assert_eq!(snap(&l.data), before, "planning writes nothing");
    let r = apply_loose(&patch, &l, &opts(), &ok, &Job::silent()).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len()), (2, 1, 1));
    assert_eq!(fs::read(l.data.join("Item/Item.SData")).unwrap(), b"new item data!");
    assert_eq!(fs::read(l.data.join("Character/New/file.bin")).unwrap(), [1, 2, 3]);
    assert_eq!(fs::read(l.data.join("Item/readme.txt")).unwrap(), b"hi");
    assert!(!l.data.join("Item/gone.txt").exists());
    // one journal of kind loose-patch, labelled with the patch, holding the originals
    let j = LooseJournal::read(&r.journal).unwrap();
    assert_eq!((j.kind.as_str(), j.label.as_str(), j.state.clone()), ("loose-patch", "ps0001.patch", State::Applied));
    assert_eq!(fs::read(r.journal.join("files/Item/Item.SData")).unwrap(), b"old item data");
    assert_eq!(fs::read(r.journal.join("files/Item/gone.txt")).unwrap(), b"drop me");
    // undo restores the folder byte for byte
    le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(snap(&l.data), before);
}

#[test]
fn a_patch_with_interface_entries_is_refused_on_a_gx_layout() {
    let t = TempDir::new("w17-patch-gx");
    let mut l = world(&t);
    put_file(&l.data, "interface/icon_dds.png", b"png");
    put_file(&l.data, "Sound/Music/a.ogg", b"ogg");
    let before = snap(&l.data);
    let patch = make_patch(&t.path().join("p"), &[("Item/Item.SData", b"new"), ("interface/icon.dds", b"dds"), ("Sound/Music/b.wav", b"wav")], &[], "");
    // the plain layout takes the same patch
    let plain = l.clone();
    l.gx = true;
    let e = apply_loose(&patch, &l, &opts(), &ok, &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("interface/icon.dds") && e.contains("Sound/Music/b.wav") && e.contains("Shaiya-GX stores this as"), "{e}");
    assert!(!e.contains("Item/Item.SData"), "only the converted media is listed: {e}");
    assert_eq!(snap(&l.data), before, "the whole patch was refused before anything was written");
    assert!(!l.backups.as_ref().unwrap().exists(), "not even a backup folder was made");
    apply_loose(&patch, &plain, &opts(), &ok, &Job::silent()).unwrap();
    assert_eq!(fs::read(plain.data.join("Item/Item.SData")).unwrap(), b"new");
}

#[test]
fn a_loose_file_of_a_patch_needs_a_folder_the_data_folder_has() {
    let t = TempDir::new("w17-patch-install");
    let l = world(&t);
    let patch = make_patch(&t.path().join("p"), &[("Item/Item.SData", b"new")], &[("game.exe", b"MZ"), ("Version.ini", b"[Version]")], "");
    let before = snap(t.path());
    let e = apply_loose(&patch, &l, &opts(), &ok, &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("game.exe") && e.contains("Version.ini") && e.contains("install folder"), "{e}");
    assert_eq!(snap(t.path()), before, "nothing was written anywhere");
}

#[test]
fn a_patch_file_that_fails_the_format_check_refuses_the_whole_patch_and_an_empty_entry_is_kept() {
    let t = TempDir::new("w17-patch-names");
    let l = world(&t);
    let before = snap(&l.data);
    // a format check that refuses `.bad`
    let strict = |rel: &str, _: &[u8]| -> Result<()> {
        if rel.to_lowercase().ends_with(".bad") { Err(Error::Refused("does not read".into())) } else { Ok(()) }
    };
    let patch = make_patch(&t.path().join("p"), &[("Item/Item.SData", b"new"), ("Item/x.bad", b"zz")], &[], "");
    let e = apply_loose(&patch, &l, &opts(), &strict, &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("Item/x.bad") && e.contains("does not read"), "{e}");
    assert_eq!(snap(&l.data), before);
    // an entry that is an empty file is a put of no bytes, not a silent drop
    let patch = make_patch(&t.path().join("q"), &[("Item/empty.txt", b"")], &[], "");
    let r = apply_loose(&patch, &l, &opts(), &ok, &Job::silent()).unwrap();
    assert_eq!(r.added, vec!["Item/empty.txt".to_string()]);
    assert_eq!(fs::read(l.data.join("Item/empty.txt")).unwrap(), b"");
}
