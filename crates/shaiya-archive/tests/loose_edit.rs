//! Changes to a loose data folder: plan, apply, journal, rollback, recovery, and the refusals before any write. Synthetic files only.

use shaiya_archive::journal::{self, Rollback, State};
use shaiya_archive::loose::{self, LooseLayout};
use shaiya_archive::loose_edit::{self as le, LooseChange, LooseJournal, LooseOptions, LoosePut};
use shaiya_core::{Cancel, Error, Job, Result, Silent};
use shaiya_testkit::{SahBuilder, TempDir};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ORIG_A: &[u8] = b"original-a";
const ORIG_B: &[u8] = b"original-b";

fn ok(_: &str, _: &[u8]) -> Result<()> {
    Ok(())
}

fn put_file(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// `c/data/{Character,Item,World}` (layout 2) with three files, and a backup folder `backups` outside it.
fn world(t: &TempDir) -> LooseLayout {
    let root = t.path().join("c");
    put_file(&root, "data/Character/Human/a.bin", ORIG_A);
    put_file(&root, "data/Item/b.txt", ORIG_B);
    put_file(&root, "data/World/w.bin", b"world");
    let mut l = loose::detect(&root).unwrap().unwrap();
    l.backups = Some(t.path().join("backups"));
    l
}

fn opts() -> LooseOptions {
    LooseOptions { label: "test".into(), confirmed_unknown_busy: true, ..LooseOptions::default() }
}

fn put(rel: &str, bytes: &[u8]) -> LoosePut {
    LoosePut { rel: rel.into(), bytes: bytes.to_vec() }
}

fn change(puts: Vec<LoosePut>, removes: &[&str]) -> LooseChange {
    LooseChange { puts, removes: removes.iter().map(|s| s.to_string()).collect() }
}

/// Every file and folder below `dir`: path -> bytes (`None` for a folder).
fn snap(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(base: &Path, at: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let Ok(rd) = std::fs::read_dir(at) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if e.file_type().unwrap().is_dir() {
                out.insert(rel, None);
                walk(base, &p, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(dir, dir, &mut m);
    m
}

fn journals(l: &LooseLayout) -> Vec<(LooseJournal, Rollback)> {
    le::list(l).unwrap()
}

fn apply_ok(l: &LooseLayout, c: LooseChange) -> le::LooseReport {
    let plan = le::plan(l, c, &ok).unwrap();
    le::apply(l, plan, &opts(), &ok, &Job::silent()).unwrap()
}

#[test]
fn a_replace_keeps_the_original_in_the_journal_and_rollback_restores_it() {
    let t = TempDir::new("w17-replace");
    let l = world(&t);
    let before = snap(&l.data);
    let r = apply_ok(&l, change(vec![put("Character/Human/a.bin", b"new-a")], &[]));
    assert_eq!((r.added.len(), r.replaced.clone(), r.removed.len()), (0, vec!["Character/Human/a.bin".to_string()], 0));
    assert_eq!(std::fs::read(l.data.join("Character/Human/a.bin")).unwrap(), b"new-a");
    // the journal holds the original, in the backup folder only
    let j = LooseJournal::read(&r.journal).unwrap();
    assert_eq!((j.kind.as_str(), j.state.clone()), ("loose-edit", State::Applied));
    assert!(r.journal.starts_with(l.backups.as_ref().unwrap()));
    assert_eq!(std::fs::read(r.journal.join("files/Character/Human/a.bin")).unwrap(), ORIG_A);
    assert!(!le::MARKER.is_empty() && !l.backups.as_ref().unwrap().join(le::MARKER).exists());
    // undo restores the original bytes and nothing else changed
    let rep = le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(rep.restored, vec!["Character/Human/a.bin".to_string()]);
    assert_eq!(snap(&l.data), before);
    assert!(matches!(LooseJournal::read(&r.journal).unwrap().state, State::RolledBack { .. }));
    // a second undo of the same change is refused
    assert!(le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).is_err());
}

#[test]
fn an_added_file_is_deleted_by_rollback() {
    let t = TempDir::new("w17-add");
    let l = world(&t);
    let before = snap(&l.data);
    let r = apply_ok(&l, change(vec![put("Effect/Deep/new.bin", b"fresh"), put("Item/c.txt", b"c")], &[]));
    assert_eq!(r.added.len(), 2);
    assert_eq!(std::fs::read(l.data.join("Effect/Deep/new.bin")).unwrap(), b"fresh");
    let rep = le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(rep.deleted.len(), 2);
    // the folders the change made are gone with it, the data folder is as before
    assert_eq!(snap(&l.data), before);
}

#[test]
fn a_removed_file_moves_to_the_journal_and_comes_back() {
    let t = TempDir::new("w17-remove");
    let l = world(&t);
    let before = snap(&l.data);
    let r = apply_ok(&l, change(vec![], &["item/B.TXT"]));
    assert_eq!(r.removed, vec!["Item/b.txt".to_string()], "the file keeps the spelling on disk");
    assert!(!l.data.join("Item/b.txt").exists());
    assert_eq!(std::fs::read(r.journal.join("files/Item/b.txt")).unwrap(), ORIG_B, "a copy is in the journal");
    le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(snap(&l.data), before);
    // a remove of a file that is not there is refused, with nothing done
    let e = le::plan(&l, change(vec![], &["Item/none.txt"]), &ok).unwrap_err();
    assert!(e.to_string().contains("nothing to remove"), "{e}");
}

#[test]
fn rollback_is_blocked_when_a_file_changed_since() {
    let t = TempDir::new("w17-changed");
    let l = world(&t);
    let r = apply_ok(&l, change(vec![put("Character/Human/a.bin", b"new-a"), put("Item/c.txt", b"c")], &[]));
    std::fs::write(l.data.join("Character/Human/a.bin"), b"someone else").unwrap();
    let all = journals(&l);
    match &all[0].1 {
        Rollback::Blocked(why) => assert!(why.contains("Character/Human/a.bin") && why.contains("changed or removed since"), "{why}"),
        other => panic!("{other:?}"),
    }
    let e = le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Refused(_)), "{e}");
    assert_eq!(std::fs::read(l.data.join("Character/Human/a.bin")).unwrap(), b"someone else", "nothing was touched");
    assert!(l.data.join("Item/c.txt").exists());
    // force: the changed file is saved inside the journal folder first and the list says which
    let rep = le::rollback(&l, &r.journal, true, &opts(), &Job::silent()).unwrap();
    assert_eq!(rep.saved, vec!["Character/Human/a.bin".to_string()]);
    assert_eq!(std::fs::read(r.journal.join("saved/Character/Human/a.bin")).unwrap(), b"someone else");
    assert_eq!(std::fs::read(l.data.join("Character/Human/a.bin")).unwrap(), ORIG_A);
    assert!(!l.data.join("Item/c.txt").exists());
}

#[test]
fn only_the_newest_change_rolls_back() {
    let t = TempDir::new("w17-newest");
    let l = world(&t);
    let first = apply_ok(&l, change(vec![put("Item/b.txt", b"v2")], &[]));
    let second = apply_ok(&l, change(vec![put("Item/b.txt", b"v3")], &[]));
    assert_ne!(first.journal, second.journal);
    let all = journals(&l);
    assert_eq!(all.len(), 2);
    match &all[0].1 {
        Rollback::Blocked(why) => assert!(why.contains("roll back") && why.contains(&second.journal.file_name().unwrap().to_string_lossy().to_string()), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(all[1].1, Rollback::Possible);
    assert!(le::rollback(&l, &first.journal, false, &opts(), &Job::silent()).is_err());
    // replacing the same file twice: the second journal holds the first change's file, so undo goes back one step at a time
    le::rollback(&l, &second.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"v2");
    le::rollback(&l, &first.journal, false, &opts(), &Job::silent()).unwrap();
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), ORIG_B);
}

#[test]
fn replacing_the_same_file_twice_in_one_change_is_refused() {
    let t = TempDir::new("w17-twice");
    let l = world(&t);
    let before = snap(&l.data);
    for (a, b) in [("Item/b.txt", "Item/b.txt"), ("Item/b.txt", "item/B.TXT"), ("New/x.bin", "NEW/x.bin")] {
        let e = le::plan(&l, change(vec![put(a, b"1"), put(b, b"2")], &[]), &ok).unwrap_err();
        assert!(e.to_string().contains("listed twice"), "{a} {b}: {e}");
    }
    // a put and a remove of the same file
    let e = le::plan(&l, change(vec![put("Item/b.txt", b"1")], &["item/b.txt"]), &ok).unwrap_err();
    assert!(e.to_string().contains("both written and removed"), "{e}");
    // a file and a folder of one name
    let e = le::plan(&l, change(vec![put("Item/b.txt/x", b"1")], &[]), &ok).unwrap_err();
    assert!(e.to_string().contains("is a file, not a folder"), "{e}");
    assert_eq!(snap(&l.data), before);
}

#[test]
fn a_name_outside_the_data_folder_refuses_the_whole_change() {
    let t = TempDir::new("w17-names");
    let l = world(&t);
    let before = snap(t.path());
    let good = put("Item/new-good.txt", b"fine");
    for bad in [
        "../outside.txt", "..\\outside.txt", "a/../../b", "/abs.txt", "\\abs.txt", "C:x.txt", "C:/x.txt", "a/./b", "a//b", "", "CON", "nul.txt", "a/aux.bin", "com1", "trail./x", "a./b", "b.txt ", "a/b ", "a<b", "a|b", "a?b", "a*b",
        "data.sah", "x/DATA.SAH/y", "x.shaiya-tmp", "x/.y.shaiya-tmp", "shaiya-toolkit-backups/x", "a\u{1}b",
    ] {
        let e = le::plan(&l, change(vec![good.clone(), put(bad, b"x")], &[]), &ok).unwrap_err();
        assert!(matches!(e, Error::Refused(_)), "{bad:?}: {e}");
        let e2 = le::plan(&l, change(vec![good.clone()], &[bad]), &ok).unwrap_err();
        assert!(matches!(e2, Error::Refused(_)), "remove {bad:?}: {e2}");
    }
    // a name that is too long, and one nested too deep
    let long = format!("Item/{}", "x".repeat(300));
    assert!(le::plan(&l, change(vec![put(&long, b"x")], &[]), &ok).is_err());
    let deep = format!("{}f.bin", "d/".repeat(le::MAX_FOLDER_DEPTH + 1));
    assert!(le::plan(&l, change(vec![put(&deep, b"x")], &[]), &ok).is_err());
    // the good file is written when the change has only good names
    assert_eq!(snap(t.path()), before, "nothing was written by a refusal");
    apply_ok(&l, change(vec![good], &[]));
}

#[test]
fn an_existing_file_keeps_its_spelling() {
    let t = TempDir::new("w17-spelling");
    let l = world(&t);
    let plan = le::plan(&l, change(vec![put("character/HUMAN/A.BIN", b"new"), put("Item/New/x.bin", b"n")], &["WORLD/w.bin"]), &ok).unwrap();
    assert_eq!(plan.puts[0].rel, "Character/Human/a.bin");
    assert!(plan.puts[0].existed && !plan.puts[1].existed);
    assert_eq!(plan.removes[0].rel, "World/w.bin");
    assert_eq!(plan.new_dirs, vec!["Item/New".to_string()]);
    le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap();
    // the same names on disk after the change: no second spelling appeared
    let names: Vec<String> = std::fs::read_dir(l.data.join("Character/Human")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["a.bin"]);
    assert_eq!(std::fs::read(l.data.join("Character/Human/a.bin")).unwrap(), b"new");
    // two new folders that differ only in letter case would be two folders on some disks: refused
    let e = le::plan(&l, change(vec![put("Fresh/a.bin", b"1"), put("FRESH/b.bin", b"2")], &[]), &ok).unwrap_err();
    assert!(e.to_string().contains("differs only in letter case"), "{e}");
}

#[test]
fn converted_media_is_refused_on_a_gx_layout_and_allowed_on_a_plain_one() {
    let t = TempDir::new("w17-gx");
    let mut l = world(&t);
    put_file(&l.data, "interface/icon_dds.png", b"png");
    put_file(&l.data, "Sound/Music/a.ogg", b"ogg");
    put_file(&l.data, "Effect/atlas_dds.png", b"png");
    let before = snap(&l.data);
    let media: Vec<LoosePut> = ["interface/new.png", "Interface/Sub/x.dds", "sound/a.wav", "Cursor/c.cur", "Effect/b.wav", "Effect/c.ogg", "Effect/n_dds.png", "Effect/atlas.dds"].iter().map(|n| put(n, b"x")).collect();
    let plain = l.clone();
    l.gx = true;
    // the whole change is refused with the list, even next to a good file
    let mut puts = media.clone();
    puts.push(put("Item/ok.txt", b"ok"));
    let e = le::plan(&l, change(puts.clone(), &["interface/icon_dds.png"]), &ok).unwrap_err().to_string();
    for n in ["interface/new.png", "Interface/Sub/x.dds", "sound/a.wav", "Cursor/c.cur", "Effect/b.wav", "Effect/c.ogg", "Effect/n_dds.png", "Effect/atlas.dds", "interface/icon_dds.png"] {
        assert!(e.contains(n) && e.contains("Shaiya-GX stores this as"), "{n} in {e}");
    }
    assert!(!e.contains("ok.txt"));
    assert_eq!(snap(&l.data), before);
    // the plain layout writes the same names
    let plan = le::plan(&plain, change(puts, &[]), &ok).unwrap();
    assert!(!plan.gx);
    le::apply(&plain, plan, &opts(), &ok, &Job::silent()).unwrap();
    assert!(plain.data.join("Sound/a.wav").exists());
    // other files of a GX layout are changed as usual
    assert!(le::plan(&l, change(vec![put("Item/b.txt", b"x")], &[]), &ok).is_ok());
}

#[test]
fn the_format_check_callback_refuses_a_bad_file() {
    let t = TempDir::new("w17-check");
    let l = world(&t);
    let before = snap(t.path());
    let seen = std::cell::RefCell::new(Vec::<(String, usize)>::new());
    let strict = |rel: &str, b: &[u8]| -> Result<()> {
        seen.borrow_mut().push((rel.to_string(), b.len()));
        if rel.to_lowercase().ends_with(".bad") {
            return Err(Error::Refused("does not read to the last byte".into()));
        }
        Ok(())
    };
    let e = le::plan(&l, change(vec![put("Item/ok.bin", b"fine"), put("Item/x.BAD", b"12345")], &[]), &strict).unwrap_err().to_string();
    assert!(e.contains("Item/x.BAD") && e.contains("does not read to the last byte"), "{e}");
    assert!(!e.contains("ok.bin"));
    assert!(seen.borrow().contains(&("Item/x.BAD".to_string(), 5)), "the callback got the path and the bytes");
    assert_eq!(snap(t.path()), before, "refused before anything was written");
}

#[test]
fn a_file_that_fails_its_reader_is_refused_before_anything_is_written() {
    // the same through the readers of the Toolkit: an `.ANI` of garbage
    let t = TempDir::new("w17-reader");
    let l = world(&t);
    let before = snap(t.path());
    let reader = |rel: &str, b: &[u8]| -> Result<()> {
        if rel.to_lowercase().ends_with(".ani") && !b.starts_with(b"ANI-OK") {
            return Err(Error::Refused(format!("{rel} does not read")));
        }
        Ok(())
    };
    assert!(le::plan(&l, change(vec![put("Character/Human/x.ANI", b"garbage")], &[]), &reader).is_err());
    assert!(le::plan(&l, change(vec![put("Character/Human/x.ANI", b"ANI-OK data")], &[]), &reader).is_ok());
    assert_eq!(snap(t.path()), before);
}

#[test]
fn a_failed_apply_leaves_the_folder_as_before() {
    let t = TempDir::new("w17-fail");
    let l = world(&t);
    let before = snap(&l.data);
    // the plan's check passes; the check of the temp file of the second put fails (injected)
    let calls = std::cell::Cell::new(0);
    let flaky = |_: &str, _: &[u8]| -> Result<()> {
        calls.set(calls.get() + 1);
        // plan: 3 calls; apply: the first put passes (call 4), the second fails (call 5)
        if calls.get() == 5 {
            return Err(Error::Refused("injected failure".into()));
        }
        Ok(())
    };
    let plan = le::plan(&l, change(vec![put("Character/Human/a.bin", b"A2"), put("Item/b.txt", b"B2"), put("Fresh/x/y.bin", b"Y")], &["World/w.bin"]), &flaky).unwrap();
    let e = le::apply(&l, plan, &opts(), &flaky, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("injected failure"), "{e}");
    assert_eq!(snap(&l.data), before, "the first file is put back, no temp file or new folder is left");
    let b = l.backups.as_ref().unwrap();
    assert!(!b.join(le::MARKER).exists());
    let all = journals(&l);
    assert_eq!(all.len(), 1);
    assert!(matches!(all[0].0.state, State::RolledBack { .. }), "the journal says the change was not made");
    assert!(snap(&l.data).keys().all(|k| !k.ends_with(".shaiya-tmp")));
    // and the folder takes the next change
    apply_ok(&l, change(vec![put("Item/b.txt", b"B3")], &[]));
}

#[test]
fn a_cancelled_apply_leaves_the_folder_as_before() {
    let t = TempDir::new("w17-cancel");
    let l = world(&t);
    let before = snap(&l.data);
    let cancel = Cancel::new();
    let c2 = cancel.clone();
    // the cancel arrives while the first file is being checked; the second file is never started
    let trip = move |_: &str, _: &[u8]| -> Result<()> {
        c2.cancel();
        Ok(())
    };
    let plan = le::plan(&l, change(vec![put("Character/Human/a.bin", b"A2"), put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    let job = Job::new(&Silent, cancel);
    let e = le::apply(&l, plan, &opts(), &trip, &job).unwrap_err();
    assert!(matches!(e, Error::Cancelled), "{e}");
    assert_eq!(snap(&l.data), before);
    assert!(!l.backups.as_ref().unwrap().join(le::MARKER).exists());
    // a cancel before the start writes nothing, not even the journal folder
    let cancel = Cancel::new();
    cancel.cancel();
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    assert!(matches!(le::apply(&l, plan, &opts(), &ok, &Job::new(&Silent, cancel)), Err(Error::Cancelled)));
    assert_eq!(snap(&l.data), before);
}

#[test]
fn an_interrupted_change_is_recovered_on_the_next_open() {
    let t = TempDir::new("w17-crash");
    let l = world(&t);
    let before = snap(&l.data);
    // a crash after the first of two puts: no undo, the marker stays, a temp file is left beside a target
    let plan = le::plan(&l, change(vec![put("Character/Human/a.bin", b"A2"), put("Fresh/n.bin", b"N")], &["Item/b.txt"]), &ok).unwrap();
    let crash = LooseOptions { crash_after_puts: Some(1), ..opts() };
    let e = le::apply(&l, plan, &crash, &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("crash"), "{e}");
    let b = l.backups.clone().unwrap();
    assert!(b.join(le::MARKER).exists(), "the marker is what a crash leaves");
    assert_eq!(std::fs::read(l.data.join("Character/Human/a.bin")).unwrap(), b"A2", "the first put had landed");
    std::fs::write(l.data.join("Fresh/.n.bin.4242.shaiya-tmp"), b"half").ok();
    // the journal survived the crash and says it did not finish
    let all = journals(&l);
    assert_eq!(all.len(), 1);
    assert!(matches!(&all[0].1, Rollback::Blocked(w) if w.contains("did not finish")), "{:?}", all[0].1);
    assert_eq!(all[0].0.state, State::Pending);
    assert!(le::unfinished(&l).is_some());
    // opening the client again recovers: the folder is as it was and the journal says so
    let notes = le::recover(&l, &opts()).unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("undid the unfinished change"), "{notes:?}");
    let mut now = snap(&l.data);
    now.remove("Fresh");
    now.remove("Fresh/.n.bin.4242.shaiya-tmp");
    assert_eq!(now, before);
    assert!(!b.join(le::MARKER).exists());
    assert!(matches!(journals(&l)[0].0.state, State::RolledBack { .. }));
    assert!(le::unfinished(&l).is_none());
    assert!(le::recover(&l, &opts()).unwrap().is_empty(), "nothing left to do");
}

#[test]
fn the_next_change_recovers_an_interrupted_one_first() {
    let t = TempDir::new("w17-crash2");
    let l = world(&t);
    let before = snap(&l.data);
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2"), put("Item/c.txt", b"C")], &[]), &ok).unwrap();
    le::apply(&l, plan, &LooseOptions { crash_after_puts: Some(1), ..opts() }, &ok, &Job::silent()).unwrap_err();
    // a change planned now is applied after the leftover was undone
    let r = apply_ok(&l, change(vec![put("Item/d.txt", b"D")], &[]));
    assert_eq!(r.recovered.len(), 1);
    let mut want = before.clone();
    want.insert("Item/d.txt".into(), Some(b"D".to_vec()));
    assert_eq!(snap(&l.data), want);
}

#[test]
fn an_interrupted_rollback_is_finished() {
    let t = TempDir::new("w17-rollback-crash");
    let l = world(&t);
    let before = snap(&l.data);
    let r = apply_ok(&l, change(vec![put("Item/b.txt", b"B2"), put("Item/c.txt", b"C")], &["World/w.bin"]));
    // a rollback that stopped half way: one file back, the marker and the "applied" state still there
    let b = l.backups.clone().unwrap();
    std::fs::write(l.data.join("Item/b.txt"), ORIG_B).unwrap();
    let marker = format!("format=2\nop=rollback\nlabel=test\njournal={}\ndata={}\n", r.journal.display(), l.data.display());
    std::fs::write(b.join(le::MARKER), marker).unwrap();
    let notes = le::recover(&l, &opts()).unwrap();
    assert!(notes[0].contains("finished the interrupted rollback"), "{notes:?}");
    assert_eq!(snap(&l.data), before);
    assert!(matches!(LooseJournal::read(&r.journal).unwrap().state, State::RolledBack { .. }));
}

#[test]
fn the_journal_is_never_inside_the_data_folder() {
    let t = TempDir::new("w17-journal-place");
    let mut l = world(&t);
    let before = snap(t.path());
    // a backup folder inside the data folder, or the data folder itself
    for bad in [l.data.join("backups"), l.data.clone(), l.data.join("Item").join("x"), l.data.join("..").join("data").join("b")] {
        l.backups = Some(bad.clone());
        let plan = le::plan(&l, change(vec![put("Item/new.txt", b"n")], &[]), &ok).unwrap();
        let e = le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap_err();
        assert!(matches!(e, Error::Refused(_)) && e.to_string().contains("outside the data folder"), "{bad:?}: {e}");
        assert!(le::list(&l).is_err() && le::recover(&l, &opts()).is_err());
    }
    assert_eq!(snap(t.path()), before, "nothing was written anywhere");
    // layout 3 (an extracted client) without a backup folder: refused, nothing written
    let three = t.path().join("three");
    put_file(&three, "Item/a.txt", b"a");
    put_file(&three, "World/b.txt", b"b");
    let mut l3 = loose::detect(&three).unwrap().unwrap();
    assert!(l3.data_is_root() && l3.backups.is_none());
    let before3 = snap(t.path());
    let plan = le::plan(&l3, change(vec![put("Item/new.txt", b"n")], &[]), &ok).unwrap();
    let e = le::apply(&l3, plan, &opts(), &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("choose a backup folder outside the data folder"), "{e}");
    assert_eq!(snap(t.path()), before3);
    // the folder itself or a folder inside it is no better
    for bad in [three.clone(), three.join("shaiya-toolkit-backups")] {
        l3.backups = Some(bad);
        let plan = le::plan(&l3, change(vec![put("Item/new.txt", b"n")], &[]), &ok).unwrap();
        assert!(le::apply(&l3, plan, &opts(), &ok, &Job::silent()).is_err());
    }
    assert_eq!(snap(t.path()), before3);
    // a sibling folder works, and the backups never land beside or in the data folder
    l3.backups = Some(t.path().join("three-backups"));
    let r = apply_ok(&l3, change(vec![put("Item/new.txt", b"n")], &[]));
    assert!(r.journal.starts_with(t.path().join("three-backups")));
}

#[test]
fn the_journal_goes_only_to_the_confirmed_backup_folder() {
    let t = TempDir::new("w17-journal-only");
    let l = world(&t);
    let before_root = snap(&l.root.join("..")).into_iter().filter(|(k, _)| !k.starts_with("c/data") && !k.starts_with("backups")).collect::<BTreeMap<_, _>>();
    let r = apply_ok(&l, change(vec![put("Item/b.txt", b"B2"), put("New/x.bin", b"x")], &["World/w.bin"]));
    le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    apply_ok(&l, change(vec![put("Item/b.txt", b"B3")], &[]));
    // nothing appeared in the install folder (beside data) or anywhere else but the data folder and the backup folder
    let after_root = snap(&l.root.join("..")).into_iter().filter(|(k, _)| !k.starts_with("c/data") && !k.starts_with("backups")).collect::<BTreeMap<_, _>>();
    assert_eq!(after_root, before_root);
    let b = snap(l.backups.as_ref().unwrap());
    assert!(b.keys().all(|k| k.split('/').next().unwrap().len() > 8), "only journal folders: {:?}", b.keys().collect::<Vec<_>>());
    // data holds no journal, marker or backup name
    assert!(snap(&l.data).keys().all(|k| !k.contains("journal.txt") && !k.contains("toolkit-pending") && !k.contains("shaiya-toolkit-backups")));
}

#[cfg(windows)]
#[test]
fn a_busy_game_exe_refuses_the_change() {
    let t = TempDir::new("w17-busy");
    let l = world(&t);
    put_file(&l.root, "game.exe", b"MZ");
    let before = snap(t.path());
    let held = std::fs::File::open(l.root.join("game.exe")).unwrap();
    assert!(matches!(le::busy_state(&l), le::Busy::Busy(_)));
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    let e = le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("game.exe") && e.to_string().contains("try again"), "{e}");
    assert_eq!(snap(t.path()), before);
    drop(held);
    // not running: no confirmation is needed
    assert_eq!(le::busy_state(&l), le::Busy::Free);
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    le::apply(&l, plan, &LooseOptions { confirmed_unknown_busy: false, ..opts() }, &ok, &Job::silent()).unwrap();
}

#[test]
fn an_unknown_busy_state_needs_the_confirmation() {
    let t = TempDir::new("w17-unknown");
    let l = world(&t);
    assert_eq!(le::busy_state(&l), le::Busy::Unknown, "no game.exe or Shaiya GX.exe in the install folder");
    let before = snap(t.path());
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    let e = le::apply(&l, plan.clone(), &LooseOptions { confirmed_unknown_busy: false, ..opts() }, &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("cannot tell whether the game is running: stop it first"), "{e}");
    assert_eq!(snap(t.path()), before, "nothing is written without the confirmation");
    le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap();
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"B2");
    // the same for an undo
    let r = journals(&l).pop().unwrap().0;
    assert!(le::rollback(&l, &r.dir, false, &LooseOptions { confirmed_unknown_busy: false, ..opts() }, &Job::silent()).is_err());
    le::rollback(&l, &r.dir, false, &opts(), &Job::silent()).unwrap();
}

#[test]
fn a_change_made_between_plan_and_apply_is_detected() {
    let t = TempDir::new("w17-stamp");
    let l = world(&t);
    // a file that the plan replaces changes (other length)
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2")], &[]), &ok).unwrap();
    std::fs::write(l.data.join("Item/b.txt"), b"changed behind our back").unwrap();
    let before = snap(t.path());
    let e = le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("changed since the change was planned"), "{e}");
    assert_eq!(snap(t.path()), before, "nothing was written, not even a journal");
    // the same length and an older time: the stamp still differs
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B3")], &["World/w.bin"]), &ok).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(l.data.join("World/w.bin")).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)).unwrap();
    drop(f);
    assert!(le::apply(&l, plan, &opts(), &ok, &Job::silent()).is_err());
    // a file that was not there appears
    let plan = le::plan(&l, change(vec![put("Item/new.txt", b"n")], &[]), &ok).unwrap();
    std::fs::write(l.data.join("Item/new.txt"), b"appeared").unwrap();
    assert!(le::apply(&l, plan, &opts(), &ok, &Job::silent()).is_err());
    assert_eq!(std::fs::read(l.data.join("Item/new.txt")).unwrap(), b"appeared");
}

#[test]
fn prune_keeps_the_newest_and_never_a_pending_one() {
    let t = TempDir::new("w17-prune");
    let l = world(&t);
    let a = apply_ok(&l, change(vec![put("Item/b.txt", b"1")], &[]));
    let b = apply_ok(&l, change(vec![put("Item/b.txt", b"2")], &[]));
    let c = apply_ok(&l, change(vec![put("Item/b.txt", b"3")], &[]));
    let gone = le::prune(&l, 1).unwrap();
    assert_eq!(gone.len(), 2);
    assert!(!a.journal.exists() && !b.journal.exists() && c.journal.exists());
    assert!(le::rollback(&l, &c.journal, false, &opts(), &Job::silent()).is_ok());
    // a journal that did not finish is kept
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"4")], &[]), &ok).unwrap();
    le::apply(&l, plan, &LooseOptions { crash_after_puts: Some(0), ..opts() }, &ok, &Job::silent()).unwrap_err();
    // with its marker the prune is refused as a whole (see prune_refuses_while_a_change_or_rollback_did_not_finish); a journal that was
    // prepared and never started (no marker) is kept while the rolled-back one goes
    std::fs::remove_file(l.backups.as_ref().unwrap().join(le::MARKER)).unwrap();
    assert_eq!(le::prune(&l, 0).unwrap().len(), 1, "only the rolled-back one");
    assert_eq!(journals(&l).len(), 1);
}

#[test]
fn the_caps_hold_before_anything_is_allocated() {
    let t = TempDir::new("w17-caps");
    let l = world(&t);
    let many: Vec<LoosePut> = (0..le::MAX_CHANGE_FILES + 1).map(|i| LoosePut { rel: format!("Item/f{i}.bin"), bytes: Vec::new() }).collect();
    let e = le::plan(&l, change(many, &[]), &ok).unwrap_err();
    assert!(e.to_string().contains("at most"), "{e}");
    assert!(le::plan(&l, LooseChange::default(), &ok).is_err(), "an empty change is refused");
    // a file over the cap is not read
    let big = t.path().join("big.bin");
    std::fs::write(&big, [0u8; 11]).unwrap();
    assert!(le::read_capped(&big, 10).is_err());
    assert_eq!(le::read_capped(&big, 11).unwrap().len(), 11);
    assert!(le::read_capped(&t.path().join("none"), 11).is_err());
}

#[test]
fn a_damaged_journal_or_copy_is_an_error_not_a_panic_and_blocks_the_rollback() {
    let t = TempDir::new("w17-damaged");
    let l = world(&t);
    let r = apply_ok(&l, change(vec![put("Item/b.txt", b"B2")], &["World/w.bin"]));
    let text = std::fs::read_to_string(r.journal.join("journal.txt")).unwrap();
    let j = LooseJournal::read(&r.journal).unwrap();
    assert_eq!(LooseJournal::parse(&r.journal, &j.to_text()).unwrap(), j);
    for n in 0..text.len() {
        if text.is_char_boundary(n) {
            let _ = LooseJournal::parse(&r.journal, &text[..n]);
        }
    }
    for at in 0..text.len() {
        let mut b = text.as_bytes().to_vec();
        b[at] ^= 1;
        if let Ok(s) = String::from_utf8(b) {
            let _ = LooseJournal::parse(&r.journal, &s);
        }
    }
    assert!(LooseJournal::parse(&r.journal, "state=applied\n").is_err());
    assert!(LooseJournal::parse(&r.journal, &text.replace("kind=loose-edit", "kind=patch")).is_err());
    assert!(LooseJournal::parse(&r.journal, &text.replace("format=2", "format=1")).is_err());
    // a copy that was changed: the rollback is blocked and nothing is touched
    std::fs::write(r.journal.join("files/Item/b.txt"), b"damaged").unwrap();
    let before = snap(&l.data);
    assert!(matches!(&journals(&l)[0].1, Rollback::Blocked(w) if w.contains("missing or damaged")));
    assert!(le::rollback(&l, &r.journal, true, &opts(), &Job::silent()).is_err());
    assert_eq!(snap(&l.data), before);
    // a journal that names a path outside the folder is refused
    let evil = text.replace("Item/b.txt", "../../outside.txt");
    std::fs::write(r.journal.join("journal.txt"), evil).unwrap();
    assert!(matches!(&journals(&l)[0].1, Rollback::Blocked(w) if w.contains("unsafe path")));
}

#[test]
fn archive_rollback_refuses_a_loose_journal() {
    let t = TempDir::new("w17-archive-refuses");
    let l = world(&t);
    let r = apply_ok(&l, change(vec![put("Item/b.txt", b"B2")], &[]));
    // an archive client and the archive rollback, given the loose journal
    let client = t.path().join("archive-client");
    std::fs::create_dir_all(&client).unwrap();
    SahBuilder::new(0).file(b"a", b"1").write_to(&client);
    let e = journal::rollback(&client, &r.journal, false, &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Refused(_)) && e.to_string().contains("loose data folder"), "{e}");
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"B2", "nothing was undone");
    // the archive history does not list it
    let listed = journal::list(&client, l.backups.as_ref().unwrap()).unwrap();
    assert!(listed.is_empty());
    // and the loose list does not show an archive journal: the two kinds stay apart
    assert_eq!(journals(&l).len(), 1);
}

// ---------------------------------------------------------------------------------------------- links

/// A folder on an NTFS volume (`%LOCALAPPDATA%\Temp`) for the junction tests: the scratch drive is exFAT and has no links. `None` where
/// there is none: the test then FAILS (see [`no_links`]), it is never a silent pass.
#[cfg(windows)]
fn ntfs_temp(tag: &str) -> Option<TempNtfs> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?.join("Temp");
    let dir = base.join(format!("shaiya-w17-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(TempNtfs(dir))
}

#[cfg(windows)]
struct TempNtfs(PathBuf);

#[cfg(windows)]
impl Drop for TempNtfs {
    fn drop(&mut self) {
        // junctions are removed as links (remove_dir), never followed
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                let _ = std::fs::remove_dir(e.path().join("data").join("Link"));
                let _ = std::fs::remove_dir(e.path());
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A link `at` -> `target` (junction on Windows, symlink elsewhere); `false` when it could not be made.
fn make_link(at: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(at).arg(target).output().map(|o| o.status.success()).unwrap_or(false)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at).is_ok()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (at, target);
        false
    }
}

/// Where a link test cannot make its links it fails loudly, with the reason; only a person who knows that this machine cannot hold links
/// sets `SHAIYA_LINKS_SKIP_OK` (the test then prints that it skipped and passes). A test that passes without having tried the link is
/// the thing this function exists to prevent.
fn no_links(why: &str) {
    if std::env::var_os("SHAIYA_LINKS_SKIP_OK").is_some() {
        eprintln!("SKIPPED (SHAIYA_LINKS_SKIP_OK is set, the link was NOT tried): {why}");
    } else {
        panic!("this test needs a folder that can hold links and could not make one: {why} (set SHAIYA_LINKS_SKIP_OK=1 only on a machine that has none)");
    }
}

fn link_base(tag: &str) -> Option<(PathBuf, Box<dyn std::any::Any>)> {
    #[cfg(windows)]
    {
        let t = ntfs_temp(tag)?;
        let p = t.0.clone();
        Some((p, Box::new(t)))
    }
    #[cfg(not(windows))]
    {
        let t = TempDir::new(tag);
        let p = t.path().to_path_buf();
        Some((p, Box::new(t)))
    }
}

#[test]
fn a_junction_or_symlink_in_the_way_refuses_the_change_and_nothing_leaves_the_folder() {
    let Some((base, _guard)) = link_base("link-way") else {
        return no_links("no folder that can hold links");
    };
    let root = base.join("c");
    put_file(&root, "data/Character/a.bin", b"a");
    put_file(&root, "data/Item/b.txt", b"b");
    let outside = base.join("outside");
    put_file(&outside, "victim.txt", b"must stay");
    if !make_link(&root.join("data").join("Link"), &outside) {
        return no_links("this folder cannot hold a link");
    }
    let mut l = loose::detect(&root).unwrap().unwrap();
    l.backups = Some(base.join("backups"));
    let before_out = snap(&outside);
    // puts and removes through the link, existing or not, are refused
    for c in [
        change(vec![put("Link/new.txt", b"x")], &[]),
        change(vec![put("Link/victim.txt", b"x")], &[]),
        change(vec![put("link/sub/deep.txt", b"x")], &[]),
        change(vec![put("Item/ok.txt", b"ok")], &["Link/victim.txt"]),
    ] {
        let e = le::plan(&l, c, &ok).unwrap_err();
        assert!(e.to_string().contains("is a link"), "{e}");
    }
    assert_eq!(snap(&outside), before_out, "nothing outside the folder was touched");
    // a link that appears between the plan and the apply: the plan said a new folder, the apply finds a link
    let l2_dir = base.join("c2");
    put_file(&l2_dir, "data/Character/a.bin", b"a");
    put_file(&l2_dir, "data/Item/b.txt", b"b");
    let mut l2 = loose::detect(&l2_dir).unwrap().unwrap();
    l2.backups = Some(base.join("backups2"));
    let plan = le::plan(&l2, change(vec![put("Sneaky/x.bin", b"x"), put("Item/ok.txt", b"ok")], &[]), &ok).unwrap();
    assert!(make_link(&l2.data.join("Sneaky"), &outside));
    let e = le::apply(&l2, plan, &opts(), &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("link") || e.to_string().contains("changed since"), "{e}");
    assert_eq!(snap(&outside), before_out);
    assert!(!l2.data.join("Item/ok.txt").exists(), "the whole change was refused");
    let _ = std::fs::remove_dir(l2.data.join("Sneaky"));
}

#[test]
fn a_backup_folder_that_is_a_junction_into_the_data_folder_is_refused() {
    let Some((base, _guard)) = link_base("link-backups") else {
        return no_links("no folder that can hold links");
    };
    let root = base.join("c");
    put_file(&root, "data/Character/a.bin", b"a");
    put_file(&root, "data/Item/b.txt", b"b");
    let mut l = loose::detect(&root).unwrap().unwrap();
    let hidden = base.join("looks-outside");
    if !make_link(&hidden, &l.data) {
        return no_links("this folder cannot hold a link");
    }
    l.backups = Some(hidden.clone());
    let before = snap(&l.data);
    let plan = le::plan(&l, change(vec![put("Item/new.txt", b"n")], &[]), &ok).unwrap();
    let e = le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap_err();
    assert!(e.to_string().contains("outside the data folder"), "{e}");
    assert_eq!(snap(&l.data), before, "no journal landed inside the data folder");
    let _ = std::fs::remove_dir(&hidden);
}

#[test]
fn a_data_folder_that_is_a_link_is_refused() {
    let Some((base, _guard)) = link_base("link-data") else {
        return no_links("no folder that can hold links");
    };
    let real = base.join("real");
    put_file(&real, "Character/a.bin", b"a");
    put_file(&real, "Item/b.txt", b"b");
    let root = base.join("c");
    std::fs::create_dir_all(&root).unwrap();
    if !make_link(&root.join("data"), &real) {
        return no_links("this folder cannot hold a link");
    }
    let l = LooseLayout { root: root.clone(), data: root.join("data"), gx: false, backups: Some(base.join("backups")) };
    let e = le::plan(&l, change(vec![put("Item/n.txt", b"n")], &[]), &ok).unwrap_err();
    assert!(e.to_string().contains("data folder") && e.to_string().contains("link"), "{e}");
    assert!(!real.join("Item/n.txt").exists());
    let _ = std::fs::remove_dir(root.join("data"));
}

#[cfg(unix)]
#[test]
fn case_only_duplicates_on_disk_refuse_the_change() {
    let t = TempDir::new("w17-dups");
    let l = world(&t);
    put_file(&l.data, "Item/Twin.txt", b"1");
    put_file(&l.data, "Item/TWIN.txt", b"2");
    if std::fs::read_dir(l.data.join("Item")).unwrap().count() < 3 {
        return; // a case-insensitive disk holds one of them
    }
    let e = le::plan(&l, change(vec![put("Item/twin.txt", b"x")], &[]), &ok).unwrap_err();
    assert!(e.to_string().contains("differ only in letter case"), "{e}");
}

#[test]
fn a_temp_file_never_stays_and_the_listing_never_sees_one() {
    let t = TempDir::new("w17-temps");
    let l = world(&t);
    let r = apply_ok(&l, change(vec![put("Item/b.txt", b"B2"), put("New/x.bin", b"x")], &[]));
    assert!(snap(&l.data).keys().chain(snap(l.backups.as_ref().unwrap()).keys()).all(|k| !k.ends_with(".shaiya-tmp")));
    le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap();
    assert!(snap(&l.data).keys().all(|k| !k.ends_with(".shaiya-tmp")));
}

/// The UTC stamp part of a journal folder name (`<stamp>-<NNNN>-<slug>`): everything before the counter.
fn stamp_of(id: &str) -> String {
    id.split('-').next().unwrap_or("").to_string()
}

#[test]
fn changes_made_in_one_second_keep_their_order_whatever_their_labels() {
    // "zeta" sorts after "alpha" by name, but alpha was made later: the newest is the one that rolls back. The test only tests the NNNN
    // counter when the three changes share one second, so it tries again (a new folder each time) until they do, and asserts that they did.
    let mut same_second = false;
    for attempt in 0..30 {
        let t = TempDir::new(&format!("w17-order-{attempt}"));
        let l = world(&t);
        for (label, bytes) in [("zeta", b"v2"), ("alpha", b"v3"), ("mid", b"v4")] {
            let plan = le::plan(&l, change(vec![put("Item/b.txt", bytes)], &[]), &ok).unwrap();
            le::apply(&l, plan, &LooseOptions { label: label.into(), ..opts() }, &ok, &Job::silent()).unwrap();
        }
        let all = journals(&l);
        // the order holds whether or not the second turned over
        assert_eq!(all.iter().map(|(j, _)| j.label.as_str()).collect::<Vec<_>>(), ["zeta", "alpha", "mid"], "the history is in the order the changes were made");
        assert_eq!(all[2].1, Rollback::Possible);
        assert!(matches!(all[1].1, Rollback::Blocked(_)) && matches!(all[0].1, Rollback::Blocked(_)));
        let stamps: Vec<String> = all.iter().map(|(j, _)| stamp_of(&j.id)).collect();
        if stamps[0] == stamps[1] && stamps[1] == stamps[2] {
            same_second = true;
            let counters: Vec<&str> = all.iter().map(|(j, _)| j.id.split('-').nth(1).unwrap_or("")).collect();
            assert_eq!(counters, ["0001", "0002", "0003"], "one second: the counter makes the order {:?}", all.iter().map(|(j, _)| &j.id).collect::<Vec<_>>());
            le::rollback(&l, &all[2].0.dir, false, &opts(), &Job::silent()).unwrap();
            assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"v3");
            break;
        }
    }
    assert!(same_second, "in 30 tries the three changes never fell into one second: the counter path was not reached");
}

// ---------------------------------------------------------------------------------------------- one change at a time, prune, shared folders

/// A client of its own folder `name` below `t`, with `backups` as its backup folder.
fn world_named(t: &TempDir, name: &str, backups: &Path) -> LooseLayout {
    let root = t.path().join(name);
    put_file(&root, "data/Character/Human/a.bin", ORIG_A);
    put_file(&root, "data/Item/b.txt", ORIG_B);
    put_file(&root, "data/World/w.bin", b"world");
    let mut l = loose::detect(&root).unwrap().unwrap();
    l.backups = Some(backups.to_path_buf());
    l
}

#[test]
fn a_running_change_is_never_recovered_pruned_or_rolled_back_by_another() {
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;
    let t = TempDir::new("w17fix-running");
    let l = world(&t);
    let first = apply_ok(&l, change(vec![put("Item/b.txt", b"B1")], &[]));
    let plan = le::plan(&l, change(vec![put("Item/c.txt", b"C"), put("Item/d.txt", b"D")], &[]), &ok).unwrap();
    let other_plan = le::plan(&l, change(vec![put("Item/e.txt", b"E")], &[]), &ok).unwrap();
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (reached_tx, go_rx) = (Mutex::new(reached_tx), Mutex::new(go_rx));
    // the change stops inside its first write (the temp file is checked before it replaces the target) until the test lets it go
    let blocking = |rel: &str, _: &[u8]| -> Result<()> {
        if rel == "Item/c.txt" {
            reached_tx.lock().unwrap().send(()).unwrap();
            go_rx.lock().unwrap().recv_timeout(Duration::from_secs(120)).map_err(|_| Error::Refused("the test did not let the change go".into()))?;
        }
        Ok(())
    };
    let b = l.backups.clone().unwrap();
    std::thread::scope(|s| {
        let running = s.spawn(|| le::apply(&l, plan, &opts(), &blocking, &Job::silent()));
        reached_rx.recv_timeout(Duration::from_secs(120)).expect("the change reached its first write");
        assert!(b.join(le::MARKER).exists(), "the running change has its marker");
        assert!(le::change_running(&l), "its lock is held");
        assert!(le::unfinished(&l).is_none(), "a running change is not an interrupted one");
        let snapshot = snap(&l.data);
        // every other writer is refused with the same sentence, and nothing is put back, deleted or marked
        let refused = [
            le::recover(&l, &opts()).unwrap_err().to_string(),
            le::apply(&l, other_plan, &opts(), &ok, &Job::silent()).unwrap_err().to_string(),
            le::rollback(&l, &first.journal, false, &opts(), &Job::silent()).unwrap_err().to_string(),
            le::prune(&l, 0).unwrap_err().to_string(),
        ];
        for r in &refused {
            assert!(r.contains("is running now"), "{r}");
        }
        assert_eq!(snap(&l.data), snapshot, "nothing of the running change was touched");
        assert!(b.join(le::MARKER).exists(), "the marker of the running change is still its own");
        assert!(first.journal.exists());
        assert!(matches!(LooseJournal::read(&first.journal).unwrap().state, State::Applied));
        go_tx.send(()).unwrap();
        running.join().unwrap().unwrap();
    });
    assert!(!b.join(le::MARKER).exists() && !le::change_running(&l));
    assert_eq!(std::fs::read(l.data.join("Item/c.txt")).unwrap(), b"C");
    assert_eq!(std::fs::read(l.data.join("Item/d.txt")).unwrap(), b"D");
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"B1");
    // once it has finished the others work again
    assert!(le::recover(&l, &opts()).unwrap().is_empty());
    assert_eq!(le::prune(&l, 5).unwrap().len(), 0);
}

#[test]
fn a_marker_with_a_free_lock_is_an_interrupted_change() {
    // what a program that stopped leaves: the marker, and no lock held by anybody
    let t = TempDir::new("w17fix-free-lock");
    let l = world(&t);
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B2"), put("Item/c.txt", b"C")], &[]), &ok).unwrap();
    le::apply(&l, plan, &LooseOptions { crash_after_puts: Some(1), ..opts() }, &ok, &Job::silent()).unwrap_err();
    assert!(l.backups.as_ref().unwrap().join(le::MARKER).exists());
    assert!(!le::change_running(&l), "the lock went with the call that returned");
    assert!(le::unfinished(&l).is_some());
    assert_eq!(le::recover(&l, &opts()).unwrap().len(), 1);
}

#[test]
fn prune_refuses_while_a_change_or_rollback_did_not_finish() {
    let t = TempDir::new("w17fix-prune-marker");
    let l = world(&t);
    let j1 = apply_ok(&l, change(vec![put("Item/b.txt", b"B1")], &[]));
    let j2 = apply_ok(&l, change(vec![put("Item/b.txt", b"B2"), put("Item/c.txt", b"C")], &[]));
    let b = l.backups.clone().unwrap();
    // a rollback of j2 that stopped half way: one file put back, the marker and the "applied" state still there
    std::fs::write(l.data.join("Item/b.txt"), b"B1").unwrap();
    std::fs::write(b.join(le::MARKER), format!("format=2\nop=rollback\nlabel=test\njournal={}\ndata={}\n", j2.journal.display(), l.data.display())).unwrap();
    for keep in [0, 1] {
        let e = le::prune(&l, keep).unwrap_err().to_string();
        assert!(e.contains("did not finish") && e.contains("Nothing was deleted"), "{e}");
    }
    assert!(j1.journal.exists() && j2.journal.exists(), "the journal the recovery needs is still there");
    // recovery finishes the rollback, and then the prune works
    let notes = le::recover(&l, &opts()).unwrap();
    assert!(notes[0].contains("finished the interrupted rollback"), "{notes:?}");
    assert!(!l.data.join("Item/c.txt").exists());
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"B1", "the rollback of the second change leaves what the first one made");
    assert_eq!(le::prune(&l, 1).unwrap().len(), 1);
    // an interrupted change keeps its journal the same way
    let plan = le::plan(&l, change(vec![put("Item/b.txt", b"B3")], &[]), &ok).unwrap();
    le::apply(&l, plan, &LooseOptions { crash_after_puts: Some(0), ..opts() }, &ok, &Job::silent()).unwrap_err();
    assert!(le::prune(&l, 0).unwrap_err().to_string().contains("did not finish"));
    assert_eq!(journals(&l).len(), 2);
}

#[test]
fn clients_that_share_a_backup_folder_never_see_or_touch_each_others_journals() {
    let t = TempDir::new("w17fix-shared");
    let shared = t.path().join("shared-backups");
    let (a, b) = (world_named(&t, "a", &shared), world_named(&t, "b", &shared));
    let a1 = apply_ok(&a, change(vec![put("Item/b.txt", b"A1")], &[]));
    apply_ok(&a, change(vec![put("Item/b.txt", b"A2")], &[]));
    let b1 = apply_ok(&b, change(vec![put("Item/b.txt", b"B1")], &[]));
    // each history holds its own changes only, and B's newer change does not block A's rollback
    assert_eq!(journals(&a).len(), 2);
    assert_eq!(journals(&b).len(), 1);
    assert_eq!(journals(&a)[1].1, Rollback::Possible);
    // A cannot roll back B's journal and cannot restore it into its own folder
    let e = le::rollback(&a, &b1.journal, false, &opts(), &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("another data folder"), "{e}");
    assert_eq!(std::fs::read(a.data.join("Item/b.txt")).unwrap(), b"A2");
    assert_eq!(std::fs::read(b.data.join("Item/b.txt")).unwrap(), b"B1");
    // pruning A deletes none of B's
    assert_eq!(le::prune(&a, 0).unwrap().len(), 2);
    assert!(b1.journal.exists() && !a1.journal.exists());
    assert_eq!(journals(&b).len(), 1);
    // an unfinished change of B in the shared folder: A is refused (the marker is B's) and B's files are not touched
    let plan = le::plan(&b, change(vec![put("Item/b.txt", b"B2"), put("Item/n.txt", b"N")], &[]), &ok).unwrap();
    le::apply(&b, plan, &LooseOptions { crash_after_puts: Some(1), ..opts() }, &ok, &Job::silent()).unwrap_err();
    let (a_before, b_before) = (snap(&a.data), snap(&b.data));
    let plan_a = le::plan(&a, change(vec![put("Item/z.txt", b"Z")], &[]), &ok).unwrap();
    let e = le::apply(&a, plan_a, &opts(), &ok, &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("another data folder"), "{e}");
    // the marker cut short: the one pending journal is B's, never A's to undo
    std::fs::write(shared.join(le::MARKER), "format=2\nop=app").unwrap();
    assert!(le::recover(&a, &opts()).is_err());
    assert_eq!(snap(&a.data), a_before);
    assert_eq!(snap(&b.data), b_before, "B's files were not restored into anything");
    // B recovers its own change (its marker is cut short and its one pending journal is its own)
    let notes = le::recover(&b, &opts()).unwrap();
    assert!(notes[0].contains("undid the unfinished change"), "{notes:?}");
    assert_eq!(std::fs::read(b.data.join("Item/b.txt")).unwrap(), b"B1");
    assert!(!b.data.join("Item/n.txt").exists());
}

#[test]
fn a_damaged_journal_is_listed_and_blocks_the_rollback_of_older_changes() {
    let t = TempDir::new("w17fix-damaged-listed");
    let l = world(&t);
    let j1 = apply_ok(&l, change(vec![put("Item/b.txt", b"B1")], &[]));
    assert!(le::damaged_journals(&l).is_empty());
    let newer = l.backups.as_ref().unwrap().join("99991231T235959Z-0001-newer");
    std::fs::create_dir_all(&newer).unwrap();
    std::fs::write(newer.join("journal.txt"), "this is not a journal").unwrap();
    assert_eq!(le::damaged_journals(&l), vec!["99991231T235959Z-0001-newer".to_string()]);
    let e = le::rollback(&l, &j1.journal, false, &opts(), &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("cannot be read"), "{e}");
    assert!(matches!(&journals(&l)[0].1, Rollback::Blocked(w) if w.contains("cannot be read")));
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), b"B1");
}

#[test]
fn a_plan_whose_public_fields_were_changed_is_refused_at_apply() {
    let t = TempDir::new("w17fix-tampered");
    let l = world(&t);
    let plan = le::plan(&l, change(vec![put("Item/new.txt", b"n"), put("Made/deep/x.bin", b"x")], &[]), &ok).unwrap();
    let before = snap(&l.data);
    // a name that leaves the data folder
    let mut bad = plan.clone();
    bad.puts[0].rel = "../escape.txt".into();
    let e = le::apply(&l, bad, &opts(), &ok, &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("escape.txt"), "{e}");
    assert!(!t.path().join("c").join("escape.txt").exists() && !t.path().join("c").join("data").join("escape.txt").exists());
    // a folder list that is not the files' folders
    let tweaks: [fn(&mut le::LoosePlan); 3] = [|p| p.new_dirs.push("../x".into()), |p| p.new_dirs.clear(), |p| p.new_dirs[0] = "Other".into()];
    for tweak in tweaks {
        let mut bad = plan.clone();
        tweak(&mut bad);
        let e = le::apply(&l, bad, &opts(), &ok, &Job::silent()).unwrap_err().to_string();
        assert!(e.contains("folders the plan makes"), "{e}");
    }
    // a converted-media name on a GX layout
    let mut gx = l.clone();
    gx.gx = true;
    let mut bad = plan.clone();
    bad.puts[0].rel = "Sound/new.ogg".into();
    assert!(le::apply(&gx, bad, &opts(), &ok, &Job::silent()).is_err());
    assert_eq!(snap(&l.data), before, "nothing was written in the data folder");
    // the unchanged plan still applies
    le::apply(&l, plan, &opts(), &ok, &Job::silent()).unwrap();
    assert_eq!(std::fs::read(l.data.join("Made/deep/x.bin")).unwrap(), b"x");
}

#[test]
fn a_windows_short_name_is_not_taken_for_a_new_folder() {
    // CHARAC~1 is the short alias of Character on an NTFS volume: written as a "new" folder it would land in Character
    let t = TempDir::new("w17fix-short");
    let l = world(&t);
    for rel in ["CHARAC~1/x.3dc", "Item/ITEM~2.txt", "Item/NAME~1"] {
        let r = le::plan(&l, change(vec![put(rel, b"x")], &[]), &ok);
        if cfg!(windows) {
            assert!(r.unwrap_err().to_string().contains("short (8.3) name"), "{rel}");
        } else {
            assert!(r.is_ok(), "{rel}: other systems have no such alias");
        }
    }
    // names that merely contain a tilde are fine
    for rel in ["Item/a~b.txt", "Item/very-long-name~1.txt", "Item/~1.txt", "Item/x~.txt"] {
        assert!(le::plan(&l, change(vec![put(rel, b"x")], &[]), &ok).is_ok(), "{rel}");
    }
}

#[test]
fn a_tga_atlas_that_gx_holds_as_a_png_is_not_written() {
    let t = TempDir::new("w17fix-tga");
    let mut l = world(&t);
    put_file(&l.data, "Effect/12.png", b"png");
    put_file(&l.data, "Effect/map_tga.png", b"png");
    l.gx = true;
    for rel in ["Effect/12.tga", "Effect/MAP.tga"] {
        let e = le::plan(&l, change(vec![put(rel, b"x")], &[]), &ok).unwrap_err().to_string();
        assert!(e.contains("stores this as PNG"), "{rel}: {e}");
    }
    for rel in ["Effect/13.tga", "Effect/other.tga"] {
        assert!(le::plan(&l, change(vec![put(rel, b"x")], &[]), &ok).is_ok(), "{rel}");
    }
    l.gx = false;
    assert!(le::plan(&l, change(vec![put("Effect/12.tga", b"x")], &[]), &ok).is_ok(), "a plain layout writes it");
}

#[test]
fn force_restores_a_removed_file_that_was_created_again() {
    let t = TempDir::new("w17fix-force-removed");
    let l = world(&t);
    let r = apply_ok(&l, change(vec![], &["Item/b.txt"]));
    std::fs::write(l.data.join("Item/b.txt"), b"somebody made it again").unwrap();
    let e = le::rollback(&l, &r.journal, false, &opts(), &Job::silent()).unwrap_err().to_string();
    assert!(e.contains("changed or removed since"), "{e}");
    let rep = le::rollback(&l, &r.journal, true, &opts(), &Job::silent()).unwrap();
    assert_eq!(rep.saved, vec!["Item/b.txt".to_string()], "what was there is saved first");
    assert_eq!(std::fs::read(l.data.join("Item/b.txt")).unwrap(), ORIG_B);
    assert_eq!(std::fs::read(r.journal.join("saved/Item/b.txt")).unwrap(), b"somebody made it again");
}
