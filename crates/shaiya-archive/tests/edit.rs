//! In-place edits, journals, rollback, recovery, write_new, in-use detection and the index helpers, on synthetic archives.

use shaiya_archive::check::{check_archive, CheckOptions};
use shaiya_archive::client::{self, ClientFiles, InUse};
use shaiya_archive::edit::{self, EditPlan, InPlaceOptions, LooseWrite, Put, PutMode, Source};
use shaiya_archive::journal::{self, Journal, JournalMode, Marker, Rollback, State};
use shaiya_archive::{compact, encode_name_strict, path_key, swap, Archive, Index};
use shaiya_core::fs::Existing;
use shaiya_core::hash::{sha256, to_hex};
use shaiya_core::{Cancel, Error, Job, Progress};
use shaiya_testkit::{SahBuilder, TempDir};
use std::path::{Path, PathBuf};

fn make_client(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    SahBuilder::new(0)
        .file(b"Item/Item.SData", b"item data!")
        .file(b"interface/a.txt", b"aaaa")
        .file(b"test/junk.eft", b"junk")
        .file(b"world/map.dg", b"mapmapmap")
        .free(b"rest space", 5000, 7)
        .write_to(dir);
}

fn snap(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    (std::fs::read(dir.join("data.sah")).unwrap(), std::fs::read(dir.join("data.saf")).unwrap())
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn put(path: &str, bytes: &[u8], mode: PutMode) -> Put {
    Put { path: path.into(), source: Source::Bytes(bytes.to_vec()), version: 1_274_079_412, mode }
}

fn opts(root: &Path) -> InPlaceOptions {
    InPlaceOptions { journal: JournalMode::In(root.to_path_buf()), legacy_bak: true, check_in_use: true }
}

fn stage(client: &Path, rel: &str, bytes: &[u8]) -> LooseWrite {
    let target = client.join(rel);
    let staged = target.with_file_name(format!(".{}.1.shaiya-tmp", target.file_name().unwrap().to_string_lossy()));
    std::fs::write(&staged, bytes).unwrap();
    LooseWrite { rel: rel.into(), staged }
}

fn read(client: &Path, path: &str) -> Vec<u8> {
    Archive::open(client).unwrap().read(path).unwrap()
}

#[test]
fn entries_visible_and_shadowed_folders() {
    let mut idx = Index::new();
    let en = |n: &str, o: u64, l: u32| shaiya_archive::Entry { name: n.as_bytes().to_vec(), offset: o, length: l, version: 0 };
    idx.root.files = vec![en("top.txt", 0, 1)];
    idx.root.dirs = vec![
        shaiya_archive::Dir { name: b"World".to_vec(), files: vec![en("only-in-old.dg", 10, 1), en("A.dg", 1, 1), en("a.DG", 2, 1)], dirs: vec![] },
        shaiya_archive::Dir { name: b"Item".to_vec(), files: vec![en("x.txt", 3, 1)], dirs: vec![shaiya_archive::Dir { name: b"sub".to_vec(), files: vec![en("y.txt", 4, 2)], dirs: vec![] }] },
        shaiya_archive::Dir { name: b"WORLD".to_vec(), files: vec![en("new.dg", 20, 5)], dirs: vec![] },
    ];
    let all = idx.entries();
    assert_eq!(all.len(), 7);
    let shadowed: Vec<&str> = all.iter().filter(|f| f.shadowed).map(|f| f.path.as_str()).collect();
    // the earlier World folder is shadowed as a whole (the client finds the later one), and A.dg is shadowed by a.DG
    assert_eq!(shadowed, vec!["World/only-in-old.dg", "World/A.dg", "World/a.DG"]);
    assert_eq!(idx.visible().len(), 4);
    assert_eq!(idx.dir_count(), 4);
    assert_eq!(idx.data_end(), 25);
    assert_eq!(Index::new().data_end(), 0);
    assert!(idx.get("world/new.dg").is_some() && idx.get("world/a.dg").is_none());
    let v = idx.visible();
    let y = v.iter().find(|f| f.path == "Item/sub/y.txt").unwrap();
    assert_eq!(y.raw_path, b"Item/sub/y.txt".to_vec());
    assert_eq!(y.segments.len(), 3);
    assert_eq!(path_key(b"Item/Sub/Y.TXT"), b"item/sub/y.txt".to_vec());
    // strict encoding refuses what code page 949 cannot hold
    assert!(encode_name_strict("\u{1F600}").is_err());
    assert_eq!(encode_name_strict("10051\u{C6D0}.3do").unwrap().len(), 5 + 2 + 4);
    assert_eq!(encode_name_strict("plain.txt").unwrap(), b"plain.txt".to_vec());
}

#[test]
fn open_index_works_without_a_saf_and_ranges_are_bounds_checked() {
    let d = TempDir::new("openidx");
    make_client(d.path());
    std::fs::remove_file(d.path().join("data.saf")).unwrap();
    let (sah, idx) = Archive::open_index(d.path()).unwrap();
    assert!(sah.ends_with("data.sah"));
    assert_eq!(idx.file_count(), 4);
    assert!(Archive::open(d.path()).is_err());
    std::fs::write(d.path().join("data.saf"), b"item data!aaaajunkmapmapmap").unwrap();
    let a = Archive::open(d.path()).unwrap();
    let e = a.index().get("world/map.dg").unwrap();
    assert_eq!(a.read_range(e, 3, 4).unwrap(), b"mapm");
    assert_eq!(a.read_range(e, 0, 0).unwrap(), b"");
    assert!(a.read_range(e, 5, 5).is_err(), "past the end of the entry");
    assert!(a.read_range(e, u64::MAX, 1).is_err());
    let src = shaiya_archive::ArchiveSource { archive: &a, filter: shaiya_core::glob::Filter::from_args(&["*.txt".into()], &[], None, None).unwrap(), include_shadowed: false };
    use shaiya_core::source::ByteSource;
    assert_eq!(src.names(), vec!["interface/a.txt"]);
    assert_eq!(src.read("INTERFACE/A.TXT").unwrap(), b"aaaa");
    assert_eq!(src.size("interface/a.txt"), Some(4));
    assert!(src.label().starts_with("archive "));
}

#[test]
fn preview_lists_every_problem_at_once() {
    let d = TempDir::new("preview");
    make_client(d.path());
    let idx = Index::read(&d.path().join("data.sah")).unwrap();
    let plan = EditPlan {
        puts: vec![
            put("Item/Item.SData", b"x", PutMode::AddOnly),
            put("nothing/here.txt", b"x", PutMode::ReplaceOnly),
            put("../escape.txt", b"x", PutMode::AddOrReplace),
            put("emoji/\u{1F600}.txt", b"x", PutMode::AddOrReplace),
            put("ok/new.txt", b"x", PutMode::AddOnly),
            put("OK/NEW.txt", b"x", PutMode::AddOrReplace),
        ],
        removes: vec!["gone/file.txt".into(), "interface/a.txt".into()],
        loose: vec![LooseWrite { rel: "DATA.SAF".into(), staged: PathBuf::from("x") }],
        ..EditPlan::default()
    };
    let e = edit::preview(&idx, &plan).unwrap_err();
    let Error::Refused(m) = e else { panic!("{e:?}") };
    for needle in ["Item/Item.SData", "nothing/here.txt", "../escape.txt", "\u{1F600}", "OK/NEW.txt", "gone/file.txt", "DATA.SAF"] {
        assert!(m.contains(needle), "{needle} missing from:\n{m}");
    }
    assert!(!m.contains("interface/a.txt"), "a valid removal is not a problem:\n{m}");
    // ignore_missing turns the missing removal into a note
    let ok = EditPlan { puts: vec![put("ok/new.txt", b"xyz", PutMode::AddOnly), put("Item/Item.SData", b"1234567", PutMode::ReplaceOnly)], removes: vec!["gone/file.txt".into(), "test/junk.eft".into()], ignore_missing: true, ..EditPlan::default() };
    let r = edit::preview(&idx, &ok).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len(), r.missing.len(), r.bytes_appended), (1, 1, 1, 1, 10));
    // a put whose path is removed in the same plan is an add (removals first)
    let re = EditPlan { puts: vec![put("test/junk.eft", b"new", PutMode::AddOnly)], removes: vec!["test/junk.eft".into()], ..EditPlan::default() };
    let r = edit::preview(&idx, &re).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len()), (1, 0, 1));
}

#[test]
fn apply_then_rollback_restores_everything_byte_for_byte() {
    let d = TempDir::new("apply");
    let c = d.path().join("c");
    make_client(&c);
    std::fs::write(c.join("game.exe"), b"old exe").unwrap();
    let before = snap(&c);
    let root = c.join(journal::DEFAULT_DIR);
    let plan = EditPlan {
        puts: vec![put("interface/new.txt", b"brand new", PutMode::AddOnly), put("Item/Item.SData", b"REPLACED-ITEM", PutMode::ReplaceOnly)],
        removes: vec!["test/junk.eft".into()],
        loose: vec![stage(&c, "game.exe", b"new exe"), stage(&c, "notice.txt", b"hello")],
        label: "ps0109.patch".into(),
        ..EditPlan::default()
    };
    let r = edit::apply_in_place(&c, &plan, &opts(&root), &Job::silent()).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len()), (1, 1, 1));
    assert_eq!(r.bytes_appended, 9 + 13);
    assert_eq!((r.saf_len_before, r.saf_len_after), (before.1.len() as u64, before.1.len() as u64 + 22));
    assert_eq!(r.loose.iter().map(|l| (l.rel.as_str(), l.overwrote)).collect::<Vec<_>>(), vec![("game.exe", true), ("notice.txt", false)]);
    // the archive and loose files changed; the marker and temp files are gone
    assert_eq!(read(&c, "interface/new.txt"), b"brand new");
    assert_eq!(read(&c, "item/item.sdata"), b"REPLACED-ITEM");
    assert!(Archive::open(&c).unwrap().index().get("test/junk.eft").is_none());
    assert_eq!(std::fs::read(c.join("game.exe")).unwrap(), b"new exe");
    assert_eq!(std::fs::read(c.join("notice.txt")).unwrap(), b"hello");
    assert!(!c.join("data.toolkit-pending").exists());
    assert!(names(&c).iter().all(|n| !n.ends_with(".shaiya-tmp")), "{:?}", names(&c));
    assert_eq!(std::fs::read(c.join("data.sah.bak")).unwrap(), before.0, "the legacy backup holds the old index");
    // the free list was kept by the in-place edit
    assert_eq!(Index::read(&c.join("data.sah")).unwrap().free.len(), 1);
    // the journal
    let list = journal::list(&c, &root).unwrap();
    assert_eq!(list.len(), 1);
    let (j, rb) = &list[0];
    assert_eq!((j.kind.as_str(), j.label.as_str(), &j.state, rb), ("patch", "ps0109.patch", &State::Applied, &Rollback::Possible));
    assert!(j.id.ends_with("-ps0109"), "{}", j.id);
    assert_eq!(j.sah_sha256_before, to_hex(&sha256(&before.0)));
    assert_eq!(j.loose.len(), 2);
    assert!(j.loose[0].existed && j.loose[0].backup.is_some() && j.loose[0].sha256_after.is_some());
    assert!(!j.loose[1].existed);
    assert_eq!(std::fs::read(j.dir.join("data.sah")).unwrap(), before.0);

    let rep = journal::rollback(&c, &j.dir, false, &Job::silent()).unwrap();
    assert_eq!(rep.truncated_to, before.1.len() as u64);
    assert_eq!((rep.restored_loose, rep.deleted_loose), (vec!["game.exe".to_string()], vec!["notice.txt".to_string()]));
    assert_eq!(snap(&c), before, "data.sah and data.saf are byte-identical again");
    assert_eq!(std::fs::read(c.join("game.exe")).unwrap(), b"old exe");
    assert!(!c.join("notice.txt").exists());
    assert!(!c.join("data.toolkit-pending").exists());
    let again = journal::list(&c, &root).unwrap();
    assert!(matches!(&again[0].0.state, State::RolledBack { at_utc } if at_utc.ends_with('Z')));
    assert!(matches!(&again[0].1, Rollback::Blocked(w) if w.contains("already")));
    assert!(journal::rollback(&c, &j.dir, false, &Job::silent()).is_err());
}

#[test]
fn rollback_is_blocked_after_other_changes_and_force_saves_changed_loose_files() {
    let d = TempDir::new("blocked");
    let c = d.path().join("c");
    make_client(&c);
    let root = c.join(journal::DEFAULT_DIR);
    let one = EditPlan { puts: vec![put("a/one.txt", b"1", PutMode::AddOnly)], loose: vec![stage(&c, "my.txt", b"v1")], label: "one".into(), ..EditPlan::default() };
    edit::apply_in_place(&c, &one, &opts(&root), &Job::silent()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100)); // journal folders are stamped to the second
    let two = EditPlan { puts: vec![put("a/two.txt", b"2", PutMode::AddOnly)], label: "two".into(), ..EditPlan::default() };
    edit::apply_in_place(&c, &two, &opts(&root), &Job::silent()).unwrap();
    let list = journal::list(&c, &root).unwrap();
    assert_eq!(list.len(), 2);
    assert!(matches!(&list[0].1, Rollback::Blocked(w) if w.contains("roll back") && w.contains("two")), "{:?}", list[0].1);
    assert_eq!(list[1].1, Rollback::Possible);
    let e = journal::rollback(&c, &list[0].0.dir, false, &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Refused(_)), "{e:?}");
    journal::rollback(&c, &list[1].0.dir, false, &Job::silent()).unwrap();
    // the older one can go now; a loose file changed since blocks it unless forced
    std::fs::write(c.join("my.txt"), b"edited by hand").unwrap();
    let list = journal::list(&c, &root).unwrap();
    assert!(matches!(&list[0].1, Rollback::Blocked(w) if w.contains("my.txt")), "{:?}", list[0].1);
    assert!(journal::rollback(&c, &list[0].0.dir, false, &Job::silent()).is_err());
    journal::rollback(&c, &list[0].0.dir, true, &Job::silent()).unwrap();
    assert!(!c.join("my.txt").exists());
    assert!(names(&c).iter().any(|n| n.starts_with("my.txt.") && n.ends_with(".bak")), "the changed file was saved first: {:?}", names(&c));
    assert!(Archive::open(&c).unwrap().index().get("a/one.txt").is_none());
    // prune keeps the newest N
    let gone = journal::prune(&c, &root, 1).unwrap();
    assert_eq!(gone.len(), 1);
    assert_eq!(journal::list(&c, &root).unwrap().len(), 1);
}

#[test]
fn rollback_is_blocked_after_compact_and_swap() {
    let d = TempDir::new("blocked2");
    let c = d.path().join("c");
    make_client(&c);
    let root = c.join(journal::DEFAULT_DIR);
    let plan = EditPlan { puts: vec![put("a/one.txt", b"1", PutMode::AddOnly)], removes: vec!["test/junk.eft".into()], label: "x".into(), ..EditPlan::default() };
    edit::apply_in_place(&c, &plan, &opts(&root), &Job::silent()).unwrap();
    compact::compact(&c, &Job::silent()).unwrap();
    let list = journal::list(&c, &root).unwrap();
    assert!(matches!(&list[0].1, Rollback::Blocked(w) if w.contains("changed since")), "{:?}", list[0].1);
    assert!(journal::rollback(&c, &list[0].0.dir, true, &Job::silent()).is_err());
    // swap: another pair in, the journal's checks fail too
    let o = d.path().join("o");
    make_client(&o);
    swap::swap(&c, &o, &Job::silent()).unwrap();
    assert!(matches!(&journal::list(&c, &root).unwrap()[0].1, Rollback::Blocked(_)));
}

#[test]
fn a_failure_during_the_append_leaves_everything_as_it_was() {
    let d = TempDir::new("fail");
    let c = d.path().join("c");
    make_client(&c);
    let before = snap(&c);
    let ghost = d.path().join("ghost.bin");
    std::fs::write(&ghost, vec![1u8; 100]).unwrap();
    let plan = EditPlan {
        puts: vec![put("a/one.txt", b"first, appended", PutMode::AddOnly), Put { path: "a/two.bin".into(), source: Source::File(ghost.clone()), version: 1, mode: PutMode::AddOnly }],
        loose: vec![stage(&c, "notice.txt", b"x")],
        label: "doomed".into(),
        ..EditPlan::default()
    };
    // plan validation sees the file; then it disappears before the append reaches it
    std::fs::remove_file(&ghost).unwrap();
    let e = edit::apply_in_place(&c, &plan, &opts(&c.join(journal::DEFAULT_DIR)), &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Io { .. }), "{e:?}");
    assert_eq!(snap(&c), before);
    assert!(!c.join("data.toolkit-pending").exists());
    assert!(!c.join(journal::DEFAULT_DIR).exists() || std::fs::read_dir(c.join(journal::DEFAULT_DIR)).unwrap().count() == 0);
}

#[test]
fn a_source_that_shrinks_after_planning_is_caught_and_undone() {
    let d = TempDir::new("shrink");
    let c = d.path().join("c");
    make_client(&c);
    let before = snap(&c);
    let range_file = d.path().join("src.bin");
    std::fs::write(&range_file, vec![9u8; 10]).unwrap();
    let plan = EditPlan {
        puts: vec![put("a/one.txt", b"abc", PutMode::AddOnly), Put { path: "a/r.bin".into(), source: Source::Range { file: range_file, offset: 5, length: 100 }, version: 1, mode: PutMode::AddOnly }],
        label: "short".into(),
        ..EditPlan::default()
    };
    let e = edit::apply_in_place(&c, &plan, &opts(&c.join(journal::DEFAULT_DIR)), &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Format { .. }), "{e:?}");
    assert_eq!(snap(&c), before);
    assert!(!c.join("data.toolkit-pending").exists());
}

#[test]
fn a_cancel_during_the_append_truncates_back() {
    let d = TempDir::new("cancel");
    let c = d.path().join("c");
    make_client(&c);
    let before = snap(&c);
    let big = vec![5u8; 3 << 20];
    let plan = EditPlan { puts: vec![put("a/big.bin", &big, PutMode::AddOnly)], loose: vec![stage(&c, "n.txt", b"x")], label: "cancelled".into(), ..EditPlan::default() };
    struct CancelOnFirst(Cancel);
    impl Progress for CancelOnFirst {
        fn advance(&self, _n: u64) {
            self.0.cancel();
        }
    }
    let cancel = Cancel::new();
    let sink = CancelOnFirst(cancel.clone());
    let e = edit::apply_in_place(&c, &plan, &opts(&c.join(journal::DEFAULT_DIR)), &Job::new(&sink, cancel)).unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e:?}");
    assert_eq!(snap(&c), before);
    assert!(!c.join("data.toolkit-pending").exists());
    let left = c.join(journal::DEFAULT_DIR);
    assert!(!left.exists() || std::fs::read_dir(left).unwrap().count() == 0, "the journal folder was removed");
}

/// Builds the state a crash leaves, from a real change: returns (client, journal folder, before, after, marker).
fn crashed_apply(d: &TempDir) -> (PathBuf, PathBuf, (Vec<u8>, Vec<u8>), (Vec<u8>, Vec<u8>), Marker) {
    let c = d.path().join("c");
    make_client(&c);
    let before = snap(&c);
    let root = c.join(journal::DEFAULT_DIR);
    let plan = EditPlan { puts: vec![put("a/new.txt", b"appended!", PutMode::AddOnly)], loose: vec![stage(&c, "notice.txt", b"loose")], label: "ps0200.patch".into(), ..EditPlan::default() };
    edit::apply_in_place(&c, &plan, &opts(&root), &Job::silent()).unwrap();
    let after = snap(&c);
    let (j, _) = journal::list(&c, &root).unwrap().remove(0);
    let marker = Marker {
        op: "apply".into(),
        label: "ps0200.patch".into(),
        saf_len_before: before.1.len() as u64,
        sah_sha256_before: j.sah_sha256_before.clone(),
        sah_sha256_after: j.sah_sha256_after.clone(),
        journal: Some(j.dir.clone()),
        staged: vec![(".notice.txt.1.shaiya-tmp".into(), "notice.txt".into())],
    };
    (c, j.dir, before, after, marker)
}

#[test]
fn recovery_before_the_index_swap_removes_the_unfinished_change() {
    let d = TempDir::new("crash-a");
    let (c, jdir, before, _after, marker) = crashed_apply(&d);
    // crash state: saf appended, old index still in place, staged file present, journal pending, marker there
    std::fs::write(c.join("data.sah"), &before.0).unwrap();
    std::fs::remove_file(c.join("notice.txt")).unwrap();
    std::fs::write(c.join(".notice.txt.1.shaiya-tmp"), b"loose").unwrap();
    let mut j = Journal::read(&jdir).unwrap();
    j.state = State::Pending;
    j.write().unwrap();
    marker.write(&c).unwrap();
    // an opening Archive or any write now first recovers
    let notes = client::recover(&c).unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("unfinished change ps0200.patch"), "{notes:?}");
    assert_eq!(snap(&c), before, "the appended bytes are cut off");
    assert!(!c.join(".notice.txt.1.shaiya-tmp").exists() && !c.join("data.toolkit-pending").exists() && !jdir.exists());
    assert!(client::recover(&c).unwrap().is_empty());
}

#[test]
fn recovery_after_the_index_swap_finishes_the_change() {
    let d = TempDir::new("crash-b");
    let (c, jdir, _before, after, marker) = crashed_apply(&d);
    // crash state: new index in place, the staged loose file not yet renamed, journal still pending
    std::fs::remove_file(c.join("notice.txt")).unwrap();
    std::fs::write(c.join(".notice.txt.1.shaiya-tmp"), b"loose").unwrap();
    let mut j = Journal::read(&jdir).unwrap();
    j.state = State::Pending;
    for l in &mut j.loose {
        l.sha256_after = None;
    }
    j.write().unwrap();
    marker.write(&c).unwrap();
    let notes = client::recover(&c).unwrap();
    assert!(notes[0].contains("finished the interrupted change ps0200.patch"), "{notes:?}");
    assert_eq!(snap(&c), after);
    assert_eq!(std::fs::read(c.join("notice.txt")).unwrap(), b"loose");
    let j = Journal::read(&jdir).unwrap();
    assert_eq!(j.state, State::Applied);
    assert!(j.loose[0].sha256_after.is_some());
    assert!(!c.join("data.toolkit-pending").exists());
    // and it can still be rolled back
    let rb = journal::list(&c, &jdir.parent().unwrap().to_path_buf()).unwrap();
    assert_eq!(rb[0].1, Rollback::Possible);
}

#[test]
fn recovery_of_an_interrupted_rollback() {
    let d = TempDir::new("crash-c");
    let (c, jdir, before, after, _m) = crashed_apply(&d);
    let j = Journal::read(&jdir).unwrap();
    let rb = Marker {
        op: "rollback".into(),
        label: "ps0200.patch".into(),
        saf_len_before: j.saf_len_before,
        sah_sha256_before: j.sah_sha256_after.clone(),
        sah_sha256_after: j.sah_sha256_before.clone(),
        journal: Some(jdir.clone()),
        staged: vec![],
    };
    // crash after the marker, before anything was restored: the marker alone is dropped
    rb.write(&c).unwrap();
    let notes = client::recover(&c).unwrap();
    assert!(notes[0].contains("had not started"), "{notes:?}");
    assert_eq!(snap(&c), after);
    assert_eq!(Journal::read(&jdir).unwrap().state, State::Applied);
    // crash after the index was restored, before the truncate: recovery finishes it
    rb.write(&c).unwrap();
    std::fs::write(c.join("data.sah"), &before.0).unwrap();
    let notes = client::recover(&c).unwrap();
    assert!(notes[0].contains("finished the interrupted rollback"), "{notes:?}");
    assert_eq!(snap(&c), before);
    assert!(!c.join("notice.txt").exists());
    assert!(matches!(Journal::read(&jdir).unwrap().state, State::RolledBack { .. }));
    assert!(!c.join("data.toolkit-pending").exists());
}

#[test]
fn a_foreign_change_to_the_index_blocks_further_writes() {
    let d = TempDir::new("crash-d");
    let (c, _jdir, _before, _after, marker) = crashed_apply(&d);
    std::fs::write(c.join("data.sah"), b"SAH changed by somebody else").unwrap();
    marker.write(&c).unwrap();
    let e = client::recover(&c).unwrap_err();
    assert!(matches!(&e, Error::Refused(m) if m.contains("data.toolkit-pending")), "{e}");
    assert!(c.join("data.toolkit-pending").exists(), "the marker stays until a person looks");
    let plan = EditPlan { puts: vec![put("a/x.txt", b"x", PutMode::AddOnly)], ..EditPlan::default() };
    assert!(edit::apply_in_place(&c, &plan, &opts(&c.join("b")), &Job::silent()).is_err());
    // a damaged marker is refused too, never panics
    std::fs::write(c.join("data.toolkit-pending"), b"garbage").unwrap();
    assert!(client::recover(&c).is_err());
}

#[test]
fn recovery_finishes_interrupted_compactions_and_swaps() {
    let d = TempDir::new("crash-e");
    let c = d.path().join("c");
    make_client(&c);
    // leftovers of a compaction that never committed
    std::fs::write(c.join("data.sah.compact"), b"half").unwrap();
    std::fs::write(c.join("data.saf.compact"), b"half").unwrap();
    let notes = client::recover(&c).unwrap();
    assert!(notes[0].contains("compaction"), "{notes:?}");
    assert!(!c.join("data.sah.compact").exists());
    // a swap journal with nothing left to do
    std::fs::write(c.join("data.swap-journal"), "").unwrap();
    assert!(client::recover(&c).unwrap()[0].contains("swap"));
}

#[test]
fn write_new_drops_shadowed_entries_and_the_free_list_and_verifies() {
    let d = TempDir::new("writenew");
    let src = d.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    SahBuilder::new(1)
        .file(b"w/A.dg", b"shadowed old")
        .file(b"w/a.DG", b"the client's")
        .file(b"Item/Item.SData", b"item")
        .file(b"empty.txt", b"")
        .free(b"rest", 100, 5)
        .trailer(&[1, 2, 3, 4])
        .write_to(&src);
    let a = Archive::open(&src).unwrap();
    let ext = d.path().join("extra.bin");
    std::fs::write(&ext, b"from a file").unwrap();
    let plan = EditPlan {
        puts: vec![Put { path: "Item/Item.SData".into(), source: Source::File(ext), version: 7, mode: PutMode::ReplaceOnly }, put("new/dir/x.txt", b"x", PutMode::AddOnly)],
        removes: vec!["empty.txt".into()],
        ..EditPlan::default()
    };
    let out = d.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let r = edit::write_new(&a, &plan, &out.join("data.sah"), Existing::Refuse, &Job::silent()).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len()), (1, 1, 1));
    let n = Archive::open(&out).unwrap();
    assert_eq!(n.index().file_count(), 3, "the shadowed entry is gone");
    assert_eq!(n.index().free.len(), 0);
    assert_eq!((n.index().version, n.index().trailer.clone()), (1, vec![1, 2, 3, 4]), "SAH version and trailer are kept");
    assert_eq!(n.read("w/a.dg").unwrap(), b"the client's");
    assert_eq!(n.read("item/item.sdata").unwrap(), b"from a file");
    assert_eq!(n.read("new/dir/x.txt").unwrap(), b"x");
    assert_eq!(n.index().get("item/item.sdata").unwrap().version, 7);
    assert_eq!(n.index().data_end(), n.saf_len(), "no gaps");
    assert_eq!(r.saf_len_after, n.saf_len());
    assert_eq!(check_archive(&n, &CheckOptions { deep: true }, &Job::silent()).unwrap().problems(), 0);
    // inputs are never overwritten; existing outputs need --force
    assert!(matches!(edit::write_new(&a, &plan, &src.join("data.sah"), Existing::Overwrite, &Job::silent()), Err(Error::Refused(_))));
    assert!(matches!(edit::write_new(&a, &plan, &out.join("data.sah"), Existing::Refuse, &Job::silent()), Err(Error::Refused(_))));
    edit::write_new(&a, &plan, &out.join("data.sah"), Existing::Overwrite, &Job::silent()).unwrap();
    // an empty plan is a compacting rewrite
    let c2 = d.path().join("c2");
    std::fs::create_dir_all(&c2).unwrap();
    let r = edit::write_new(&a, &EditPlan::default(), &c2.join("data.sah"), Existing::Refuse, &Job::silent()).unwrap();
    assert_eq!(r.added.len() + r.replaced.len(), 0);
    assert_eq!(Archive::open(&c2).unwrap().index().file_count(), 3);
    // no temp files remain
    assert!(names(&out).iter().all(|n| n.starts_with("data.")), "{:?}", names(&out));
}

#[test]
fn in_use_detection_matches_the_os() {
    let d = TempDir::new("inuse");
    make_client(d.path());
    let f = ClientFiles::locate(d.path()).unwrap();
    if cfg!(windows) {
        assert_eq!(client::in_use(&f), InUse::Free);
        let held = std::fs::File::open(&f.saf).unwrap();
        assert!(matches!(client::in_use(&f), InUse::Busy(m) if m.contains("data.saf")));
        // an edit refuses and changes nothing
        let before = snap(d.path());
        let plan = EditPlan { puts: vec![put("a/x.txt", b"x", PutMode::AddOnly)], ..EditPlan::default() };
        let e = edit::apply_in_place(d.path(), &plan, &InPlaceOptions { journal: JournalMode::None, legacy_bak: false, check_in_use: true }, &Job::silent()).unwrap_err();
        assert!(matches!(&e, Error::Refused(m) if m.contains("open in another program")), "{e}");
        drop(held);
        assert_eq!(snap(d.path()), before);
        edit::apply_in_place(d.path(), &plan, &InPlaceOptions { journal: JournalMode::None, legacy_bak: false, check_in_use: true }, &Job::silent()).unwrap();
    } else {
        assert_eq!(client::in_use(&f), InUse::Unknown);
    }
}

#[test]
fn no_journal_still_writes_the_marker_and_backup() {
    let d = TempDir::new("nojournal");
    let c = d.path().join("c");
    make_client(&c);
    let before = snap(&c);
    let plan = EditPlan { puts: vec![put("a/x.txt", b"x", PutMode::AddOnly)], ..EditPlan::default() };
    let r = edit::apply_in_place(&c, &plan, &InPlaceOptions { journal: JournalMode::None, legacy_bak: true, check_in_use: false }, &Job::silent()).unwrap();
    assert!(r.journal.is_none());
    assert_eq!(std::fs::read(c.join("data.sah.bak")).unwrap(), before.0);
    assert!(!c.join(journal::DEFAULT_DIR).exists());
    assert!(!c.join("data.toolkit-pending").exists());
}
