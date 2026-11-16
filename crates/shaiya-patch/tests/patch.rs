//! Patch tests on synthetic clients: build -> list -> apply -> read back, compaction after a patch, cancel, unsafe names, robustness.

use shaiya_archive::compact::compact;
use shaiya_archive::Archive;
use shaiya_core::{Cancel, Error, Job, Progress};
use shaiya_patch::{apply, build, list, removals};
use shaiya_testkit::{flip, truncate, SahBuilder, TempDir};
use std::fs;
use std::io::Write;
use std::path::Path;

fn client_in(dir: &Path) {
    SahBuilder::new(0).file(b"Item/Item.SData", b"old item data").file(b"keep/me.txt", b"keep").file(b"drop/gone.txt", b"drop me").write_to(dir);
}

fn make_patch(base: &Path) -> std::path::PathBuf {
    let files = base.join("files");
    fs::create_dir_all(files.join("Item")).unwrap();
    fs::create_dir_all(files.join("new")).unwrap();
    fs::write(files.join("Item/Item.SData"), b"new item data!").unwrap();
    fs::write(files.join("new/file.bin"), [1u8, 2, 3]).unwrap();
    let loose = base.join("loose");
    fs::create_dir_all(loose.join("sub")).unwrap();
    fs::write(loose.join("readme.txt"), b"hi").unwrap();
    fs::write(loose.join("sub/x.cfg"), b"cfg").unwrap();
    let remove = base.join("remove.txt");
    fs::write(&remove, "drop/gone.txt\nnot/there.txt\n").unwrap();
    let out = base.join("ps0001.patch");
    let r = build(Some(&files), Some(&loose), Some(&remove), &out).unwrap();
    assert_eq!(r.added.len(), 2);
    assert_eq!(r.loose.len(), 2);
    assert_eq!(r.removed.len(), 2);
    out
}

#[test]
fn list_apply_and_read_back_through_archive() {
    let t = TempDir::new("patch-flow");
    let client = t.path().join("client");
    fs::create_dir_all(&client).unwrap();
    client_in(&client);
    let patch = make_patch(t.path());

    let (files, loose) = list(&patch).unwrap();
    let mut names: Vec<_> = files.iter().map(|(p, l)| (p.as_str(), *l)).collect();
    names.sort();
    assert_eq!(names, vec![("Item/Item.SData", 14), ("new/file.bin", 3)]);
    assert_eq!(loose.len(), 2);
    assert_eq!(removals(&patch).unwrap(), vec!["drop/gone.txt", "not/there.txt"]);

    let before_sah = fs::read(client.join("data.sah")).unwrap();
    let r = apply(&patch, &client, &[]).unwrap();
    assert_eq!((r.replaced.len(), r.added.len(), r.removed.len(), r.missing.len(), r.loose.len()), (1, 1, 1, 1, 2));
    assert_eq!(fs::read(client.join("data.sah.bak")).unwrap(), before_sah, "the previous index is kept");
    assert_eq!(fs::read(client.join("readme.txt")).unwrap(), b"hi");
    assert_eq!(fs::read(client.join("sub/x.cfg")).unwrap(), b"cfg");
    assert!(!client.join("update.saf.tmp").exists());

    let a = Archive::open(&client).unwrap();
    assert_eq!(a.read("item/item.sdata").unwrap(), b"new item data!");
    assert_eq!(a.read("new/file.bin").unwrap(), [1, 2, 3]);
    assert_eq!(a.read("keep/me.txt").unwrap(), b"keep");
    assert!(a.read("drop/gone.txt").is_err());
    drop(a);

    // dead space is reclaimed by compaction and every file still reads back
    let c = compact(&client, &Job::silent()).unwrap();
    assert!(c.after < c.before);
    let a = Archive::open(&client).unwrap();
    assert_eq!(a.read("Item/Item.SData").unwrap(), b"new item data!");
    assert_eq!(a.read("keep/me.txt").unwrap(), b"keep");
    assert_eq!(a.saf_len(), c.after);
}

#[test]
fn a_cancelled_compaction_after_a_patch_changes_nothing() {
    let t = TempDir::new("patch-cancel");
    let client = t.path().join("client");
    fs::create_dir_all(&client).unwrap();
    client_in(&client);
    apply(&make_patch(t.path()), &client, &[]).unwrap();
    let before = (fs::read(client.join("data.sah")).unwrap(), fs::read(client.join("data.saf")).unwrap());
    struct StopNow(Cancel);
    impl Progress for StopNow {
        fn total(&self, _: Option<u64>, _: shaiya_core::Unit) {
            self.0.cancel();
        }
    }
    let c = Cancel::new();
    let sink = StopNow(c.clone());
    assert!(matches!(compact(&client, &Job::new(&sink, c)), Err(Error::Cancelled)));
    assert_eq!((fs::read(client.join("data.sah")).unwrap(), fs::read(client.join("data.saf")).unwrap()), before);
}

/// Writes a patch whose ZIP holds the given (name, bytes) entries after a valid empty archive.
fn zip_with(path: &Path, extra: &[(&str, &[u8])]) {
    let (sah, saf) = (shaiya_archive::Index::new().to_bytes(), Vec::<u8>::new());
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    z.start_file("update.sah", o).unwrap();
    z.write_all(&sah).unwrap();
    z.start_file("update.saf", o).unwrap();
    z.write_all(&saf).unwrap();
    for (n, b) in extra {
        z.start_file(*n, o).unwrap();
        z.write_all(b).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn unsafe_loose_names_are_refused_before_anything_changes() {
    for evil in ["../escape.txt", "sub/../../escape.txt", "/abs.txt"] {
        let t = TempDir::new("patch-evil");
        let client = t.path().join("client");
        fs::create_dir_all(&client).unwrap();
        client_in(&client);
        let before = (fs::read(client.join("data.sah")).unwrap(), fs::read(client.join("data.saf")).unwrap());
        let patch = t.path().join("evil.patch");
        zip_with(&patch, &[("good.txt", b"fine"), (evil, b"bad")]);
        let r = apply(&patch, &client, &[]);
        assert!(matches!(r, Err(Error::Format { .. })), "{evil}: {r:?}");
        assert_eq!((fs::read(client.join("data.sah")).unwrap(), fs::read(client.join("data.saf")).unwrap()), before);
        assert!(!client.join("good.txt").exists(), "{evil}: nothing may be written");
        assert!(!t.path().join("escape.txt").exists() && !client.join("escape.txt").exists());
    }
}

#[test]
fn a_loose_file_may_not_use_a_reserved_name() {
    let t = TempDir::new("patch-reserved");
    let loose = t.path().join("loose");
    fs::create_dir_all(&loose).unwrap();
    fs::write(loose.join("update.sah"), b"x").unwrap();
    assert!(build(None, Some(&loose), None, &t.path().join("p.patch")).is_err());
}

#[test]
fn every_truncation_of_a_patch_is_an_error_and_flips_never_panic() {
    let t = TempDir::new("patch-robust");
    let patch = make_patch(t.path());
    let bytes = fs::read(&patch).unwrap();
    let probe = t.path().join("probe.patch");
    for cut in 0..bytes.len() {
        fs::write(&probe, truncate(&bytes, cut)).unwrap();
        assert!(list(&probe).is_err(), "cut at {cut} must fail");
    }
    let client = t.path().join("client");
    for at in 0..bytes.len() {
        fs::write(&probe, flip(&bytes, at)).unwrap();
        let _ = list(&probe);
        let _ = removals(&probe);
        // apply on a fresh client (every 6th byte, file work is slow on some disks): it may fail, it must not panic
        if at % 6 == 0 {
            let _ = fs::remove_dir_all(&client);
            fs::create_dir_all(&client).unwrap();
            client_in(&client);
            let _ = apply(&probe, &client, &[]);
        }
    }
}

#[test]
fn a_missing_or_foreign_file_is_an_error_not_a_panic() {
    let t = TempDir::new("patch-bad");
    assert_eq!(list(&t.path().join("nope.patch")).err().map(|e| e.kind()), Some(shaiya_core::ErrorKind::Io));
    let junk = t.path().join("junk.patch");
    fs::write(&junk, b"this is not a zip file").unwrap();
    assert_eq!(list(&junk).err().map(|e| e.kind()), Some(shaiya_core::ErrorKind::Format));
}
