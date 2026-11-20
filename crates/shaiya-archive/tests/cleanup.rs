//! Client health (read only) and clean-up (move, put back) on synthetic client folders.

use shaiya_archive::cleanup::{self, Manifest};
use shaiya_archive::health::{self, Kind};
use shaiya_archive::journal::{Marker, DEFAULT_DIR};
use shaiya_core::{Error, Job};
use shaiya_testkit::{SahBuilder, TempDir};
use std::collections::BTreeMap;
use std::path::Path;

const A: &str = "data.20261101T101010Z";

fn write(p: &Path, bytes: &[u8]) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, bytes).unwrap();
}

/// Every file below `dir` with its bytes (the clients here are tiny).
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(dir, dir, &mut m);
    m
}

fn client() -> TempDir {
    let d = TempDir::new("w6-client");
    SahBuilder::new(0).file(b"a/b.txt", b"hello").file(b"top.txt", b"top").write_to(d.path());
    d
}

/// A client with one leftover of each kind (the editor's install record is not there).
fn leftovers(d: &Path) {
    write(&d.join("data.sah.bak"), b"old index");
    write(&d.join(format!("{A}.sah")), b"swapped sah");
    write(&d.join(format!("{A}.saf")), b"swapped saf");
    write(&d.join("data.sah.svmap-editor"), b"first copy");
    write(&d.join("data.sah.svmap-editor-20261102T000000Z"), b"later copy");
    write(&d.join(".notice.txt.4242.shaiya-tmp"), b"half a file");
    write(&d.join("sub/.deep.1.shaiya-tmp"), b"deeper");
    write(&d.join("Version.ini"), b"[Version]\r\nCheckVersion=3\r\nStartUpdate=UPDATE_START\r\n");
}

fn marker(names: &[(&str, &str)]) -> Marker {
    Marker {
        op: "apply".into(),
        label: "ps0001.patch".into(),
        saf_len_before: 1,
        sah_sha256_before: "a".repeat(64),
        sah_sha256_after: "b".repeat(64),
        journal: None,
        staged: names.iter().map(|(s, t)| (s.to_string(), t.to_string())).collect(),
    }
}

fn kinds(h: &health::Health) -> Vec<&'static str> {
    h.leftovers.iter().map(|l| l.kind.name()).collect()
}

#[test]
fn health_lists_each_kind_and_preselects_only_what_nothing_needs() {
    let d = client();
    leftovers(d.path());
    let h = health::report(d.path()).unwrap();
    assert_eq!(kinds(&h), ["sah-bak", "stray-temp", "stray-temp", "swap-pair", "editor-backup", "editor-first-copy"]);
    let pre: Vec<(&str, bool)> = h.leftovers.iter().map(|l| (l.kind.name(), l.pre_selected)).collect();
    assert_eq!(pre, [("sah-bak", false), ("stray-temp", true), ("stray-temp", true), ("swap-pair", false), ("editor-backup", true), ("editor-first-copy", false)]);
    let swap = h.leftovers.iter().find(|l| l.kind == Kind::SwapPair).unwrap();
    assert_eq!(swap.files, [format!("{A}.sah"), format!("{A}.saf")]);
    assert_eq!(swap.bytes, 22);
    let tmp: Vec<&str> = h.leftovers.iter().filter(|l| l.kind == Kind::StrayTemp).map(|l| l.files[0].as_str()).collect();
    assert_eq!(tmp, [".notice.txt.4242.shaiya-tmp", "sub/.deep.1.shaiya-tmp"]);
    assert_eq!(h.start_update.as_deref(), Some("UPDATE_START"));
    assert!(h.index.saf_present && h.index.problems == 0 && h.index.files == 2);
    assert!(h.dead_space.is_some());
    assert!(!h.healthy());
    // a plain client has nothing to report
    assert!(health::report(client().path()).unwrap().healthy());
}

#[test]
fn health_never_writes() {
    let d = client();
    leftovers(d.path());
    write(&d.path().join(DEFAULT_DIR).join("20261101T000000Z-x/journal.txt"), b"not a readable journal");
    marker(&[(".q.1.shaiya-tmp", "q")]).write(d.path()).unwrap();
    std::fs::create_dir_all(d.path().join(DEFAULT_DIR).join("cleanup-20261103T000000Z")).unwrap();
    let before = snapshot(d.path());
    let dirs = |p: &Path| -> Vec<String> { walk_dirs(p) };
    let dirs_before = dirs(d.path());
    let h = health::report(d.path()).unwrap();
    assert!(h.findings() > 0 && h.pending_marker.is_some());
    assert_eq!(snapshot(d.path()), before, "no file was created, changed or removed");
    assert_eq!(dirs(d.path()), dirs_before, "no folder was created");
}

fn walk_dirs(p: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        if e.path().is_dir() {
            v.push(e.file_name().to_string_lossy().into_owned());
            v.extend(walk_dirs(&e.path()).into_iter().map(|c| format!("{}/{c}", e.file_name().to_string_lossy())));
        }
    }
    v.sort();
    v
}

#[test]
fn a_temp_file_a_pending_marker_names_is_never_offered() {
    let d = client();
    write(&d.path().join(".named.1.shaiya-tmp"), b"staged");
    write(&d.path().join("sub/.named2.1.shaiya-tmp"), b"staged too");
    write(&d.path().join(".stray.2.shaiya-tmp"), b"stray");
    marker(&[(".named.1.shaiya-tmp", "named"), ("sub\\.named2.1.shaiya-tmp", "sub/named2")]).write(d.path()).unwrap();
    let h = health::report(d.path()).unwrap();
    let offered: Vec<&str> = h.leftovers.iter().map(|l| l.files[0].as_str()).collect();
    assert_eq!(offered, [".stray.2.shaiya-tmp"]);
    let held: Vec<&str> = h.not_offered.iter().map(|n| n.file.as_str()).collect();
    assert_eq!(held, [".named.1.shaiya-tmp", "sub/.named2.1.shaiya-tmp"]);
    assert_eq!(h.pending_marker.as_ref().map(|m| (m.op.as_str(), m.readable)), Some(("apply", true)));
    // an id of a named file is refused by the plan
    assert!(matches!(cleanup::plan(&h, &["stray-temp:.named.1.shaiya-tmp".to_string()]), Err(Error::Refused(_))));
    // a marker that cannot be read: no temporary file is offered
    write(&d.path().join("data.toolkit-pending"), b"garbage");
    let h = health::report(d.path()).unwrap();
    assert!(h.leftovers.is_empty() && h.not_offered.len() == 3 && h.pending_marker.as_ref().is_some_and(|m| !m.readable), "{h:?}");
}

#[test]
fn editor_backups_are_not_offered_while_the_install_record_exists() {
    let d = client();
    leftovers(d.path());
    write(&d.path().join("svmap-editor-install.json"), b"{}");
    let h = health::report(d.path()).unwrap();
    assert!(!kinds(&h).iter().any(|k| k.starts_with("editor")), "{:?}", kinds(&h));
    let held: Vec<&str> = h.not_offered.iter().map(|n| n.file.as_str()).collect();
    assert_eq!(held, ["data.sah.svmap-editor", "data.sah.svmap-editor-20261102T000000Z"]);
    let ids = vec!["editor-first-copy:data.sah.svmap-editor".to_string()];
    assert!(matches!(cleanup::plan(&h, &ids), Err(Error::Refused(_))));
    // the other kinds are still offered
    assert_eq!(cleanup::ids_of_kinds(&h, &[Kind::SahBak]).len(), 1);
}

#[test]
fn clean_moves_then_put_back_restores_names_and_bytes_and_nothing_is_deleted() {
    let d = client();
    leftovers(d.path());
    let before = snapshot(d.path());
    let h = health::report(d.path()).unwrap();
    let ids = cleanup::ids_of_kinds(&h, &Kind::ALL);
    assert_eq!(ids.len(), 6);
    let rep = cleanup::clean(d.path(), &ids, &Job::silent()).unwrap();
    assert_eq!(rep.moved.len(), 7, "the swap pair counts two files");
    assert!(rep.folder.starts_with("cleanup-") && rep.path.starts_with(d.path().join(DEFAULT_DIR)), "{rep:?}");
    for rel in &rep.moved {
        assert!(!d.path().join(rel).exists(), "{rel} left the client folder");
        assert_eq!(std::fs::read(rep.path.join(rel)).unwrap(), before[rel], "{rel} is in the cleanup folder with its bytes");
    }
    // everything that was there is still somewhere below the client folder
    let after = snapshot(d.path());
    let m = Manifest::read(&rep.path).unwrap();
    assert_eq!((m.state.as_str(), m.items.len(), m.items.iter().all(|i| i.state == "moved")), ("moved", 7, true));
    for (rel, bytes) in &before {
        let kept = after.get(rel).or_else(|| after.get(&format!("{DEFAULT_DIR}/{}/{rel}", rep.folder)));
        assert_eq!(kept, Some(bytes), "{rel} is kept");
    }
    // the client does not report them again, and lists the folder
    let h2 = health::report(d.path()).unwrap();
    assert!(h2.leftovers.is_empty() && h2.cleanups.len() == 1 && h2.cleanups[0].to_put_back == 7 && h2.cleanups[0].state == "moved", "{h2:?}");
    assert_eq!(h2.journals.count, 0, "a cleanup folder is not a journal");
    let root = d.path().join(DEFAULT_DIR);
    assert!(shaiya_archive::journal::list(d.path(), &root).unwrap().is_empty(), "history does not list it");
    assert!(shaiya_archive::journal::prune(d.path(), &root, 0).unwrap().is_empty() && rep.path.join("manifest.json").is_file(), "prune leaves it alone");
    // put back
    let back = cleanup::put_back(&rep.path, &Job::silent()).unwrap();
    assert_eq!(back.restored.len(), 7);
    let again = snapshot(d.path());
    for (rel, bytes) in &before {
        assert_eq!(again.get(rel), Some(bytes), "{rel} is back with its bytes");
    }
    assert_eq!(Manifest::read(&rep.path).unwrap().state, "put-back");
    assert_eq!(health::report(d.path()).unwrap().cleanups[0].to_put_back, 0);
    // and a second Put back has nothing left to do
    assert!(cleanup::put_back(&rep.path, &Job::silent()).unwrap().restored.is_empty());
}

#[test]
fn put_back_refuses_when_a_destination_exists_and_moves_nothing() {
    let d = client();
    leftovers(d.path());
    let h = health::report(d.path()).unwrap();
    let rep = cleanup::clean(d.path(), &cleanup::ids_of_kinds(&h, &[Kind::SahBak, Kind::StrayTemp]), &Job::silent()).unwrap();
    write(&d.path().join("data.sah.bak"), b"a new file with the old name");
    let r = cleanup::put_back(&rep.path, &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)) && r.to_string().contains("data.sah.bak"), "{r}");
    assert_eq!(std::fs::read(d.path().join("data.sah.bak")).unwrap(), b"a new file with the old name", "the file in place is untouched");
    assert!(rep.path.join("data.sah.bak").exists() && !d.path().join(".notice.txt.4242.shaiya-tmp").exists(), "nothing was moved back");
    // after the name is free again it works
    std::fs::remove_file(d.path().join("data.sah.bak")).unwrap();
    assert_eq!(cleanup::put_back(&rep.path, &Job::silent()).unwrap().restored.len(), 3);
}

#[test]
fn an_interrupted_clean_up_can_be_put_back() {
    let d = client();
    write(&d.path().join("data.sah.bak"), b"old index");
    write(&d.path().join(".a.1.shaiya-tmp"), b"tmp a");
    write(&d.path().join(".b.1.shaiya-tmp"), b"tmp b");
    // the state a crash leaves: folder and manifest written, the first file moved and recorded, the second moved but not yet recorded,
    // the third not moved
    let folder = d.path().join(DEFAULT_DIR).join("cleanup-20261104T000000Z");
    let item = |from: &str, state: &str| cleanup::Item { group: format!("stray-temp:{from}"), kind: "stray-temp".into(), from: from.into(), to: from.into(), bytes: 5, state: state.into() };
    let m = Manifest { id: "cleanup-20261104T000000Z".into(), created_utc: "2026-11-04T00:00:00Z".into(), tool: "t".into(), state: "moving".into(), items: vec![item(".a.1.shaiya-tmp", "moved"), item(".b.1.shaiya-tmp", "planned"), item("data.sah.bak", "planned")] };
    std::fs::create_dir_all(&folder).unwrap();
    m.write(&folder).unwrap();
    std::fs::rename(d.path().join(".a.1.shaiya-tmp"), folder.join(".a.1.shaiya-tmp")).unwrap();
    std::fs::rename(d.path().join(".b.1.shaiya-tmp"), folder.join(".b.1.shaiya-tmp")).unwrap();
    let h = health::report(d.path()).unwrap();
    assert_eq!((h.cleanups[0].state.as_str(), h.cleanups[0].to_put_back), ("moving", 2));
    let rep = cleanup::put_back(&folder, &Job::silent()).unwrap();
    assert_eq!((rep.restored.len(), rep.already_in_place.len(), rep.missing.len()), (2, 1, 0), "{rep:?}");
    assert_eq!(std::fs::read(d.path().join(".a.1.shaiya-tmp")).unwrap(), b"tmp a");
    assert_eq!(std::fs::read(d.path().join(".b.1.shaiya-tmp")).unwrap(), b"tmp b");
    assert_eq!(std::fs::read(d.path().join("data.sah.bak")).unwrap(), b"old index");
    assert_eq!(Manifest::read(&folder).unwrap().state, "put-back");
}

#[test]
fn put_back_refuses_what_is_not_a_cleanup_folder_or_names_an_unsafe_path() {
    let d = client();
    let not = d.path().join("elsewhere");
    std::fs::create_dir_all(&not).unwrap();
    assert!(matches!(cleanup::put_back(&not, &Job::silent()), Err(Error::Refused(_))));
    let folder = d.path().join(DEFAULT_DIR).join("cleanup-20261105T000000Z");
    std::fs::create_dir_all(&folder).unwrap();
    let evil = cleanup::Item { group: "g".into(), kind: "stray-temp".into(), from: "../outside".into(), to: "x".into(), bytes: 0, state: "moved".into() };
    Manifest { id: "x".into(), created_utc: String::new(), tool: String::new(), state: "moved".into(), items: vec![evil] }.write(&folder).unwrap();
    write(&folder.join("x"), b"x");
    let r = cleanup::put_back(&folder, &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)) && r.to_string().contains("unsafe"), "{r}");
    assert!(folder.join("x").exists());
}

/// Makes `link` a directory link to `target`: a symlink where that is allowed, on Windows else a junction (which needs an NTFS volume).
/// False when this system or volume cannot make either (the test then says so and stops).
fn make_dir_link(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return true;
        }
        std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

#[cfg(any(windows, unix))]
#[test]
fn put_back_refuses_a_destination_below_a_link_and_moves_nothing() {
    let d = client();
    let outside = TempDir::new("w6-outside");
    if !make_dir_link(&d.path().join("sub"), outside.path()) {
        eprintln!("skipped: this volume cannot make a symlink or junction");
        return;
    }
    let folder = d.path().join(DEFAULT_DIR).join("cleanup-20261106T000000Z");
    let item = cleanup::Item { group: "g".into(), kind: "stray-temp".into(), from: "sub/x.txt".into(), to: "sub/x.txt".into(), bytes: 1, state: "moved".into() };
    std::fs::create_dir_all(&folder).unwrap();
    Manifest { id: "cleanup-20261106T000000Z".into(), created_utc: String::new(), tool: String::new(), state: "moved".into(), items: vec![item] }.write(&folder).unwrap();
    write(&folder.join("sub/x.txt"), b"x");
    let r = cleanup::put_back(&folder, &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)) && r.to_string().contains("link"), "{r}");
    assert!(folder.join("sub/x.txt").exists(), "the file stays in the cleanup folder");
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing was written through the link");
}

#[test]
fn clean_refuses_an_id_that_is_not_offered_and_a_pending_change_and_moves_nothing() {
    let d = client();
    leftovers(d.path());
    let before = snapshot(d.path());
    let r = cleanup::clean(d.path(), &["sah-bak:data.sah".to_string()], &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)), "{r}");
    // a marker recovery cannot settle stays: refused, and no cleanup folder is made
    write(&d.path().join("data.toolkit-pending"), b"garbage");
    let r = cleanup::clean(d.path(), &["sah-bak:data.sah.bak".to_string()], &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)), "{r}");
    std::fs::remove_file(d.path().join("data.toolkit-pending")).unwrap();
    assert_eq!(snapshot(d.path()), before);
    assert!(!d.path().join(DEFAULT_DIR).exists());
}

#[test]
fn a_leftover_older_index_is_listed_for_a_client_without_data_saf() {
    // an index-only folder (as one of the test clients is): the check still runs, the missing data.saf is a finding
    let d = TempDir::new("w6-index-only");
    let (sah, saf) = SahBuilder::new(0).file(b"a.txt", b"x").write_to(d.path());
    std::fs::remove_file(saf).unwrap();
    assert!(sah.exists());
    let h = health::report(d.path()).unwrap();
    assert!(!h.index.saf_present && h.dead_space.is_none() && h.findings() == 1, "{h:?}");
    assert!(cleanup::clean(d.path(), &[], &Job::silent()).is_err(), "clean-up needs the pair to check that nothing holds it");
    assert!(health::report(&d.path().join("missing")).is_err());
}

#[test]
fn journals_are_counted_with_their_size() {
    let d = client();
    write(&d.path().join(DEFAULT_DIR).join("20261101T000000Z-x/journal.txt"), b"12345");
    write(&d.path().join(DEFAULT_DIR).join("20261101T000000Z-x/data.sah"), b"1234567");
    let h = health::report(d.path()).unwrap();
    assert_eq!((h.journals.count, h.journals.bytes), (1, 12));
}

#[cfg(windows)]
#[test]
fn clean_is_refused_while_data_saf_is_held_open_and_moves_nothing() {
    let d = client();
    leftovers(d.path());
    let before = snapshot(d.path());
    let h = health::report(d.path()).unwrap();
    let held = std::fs::File::open(d.path().join("data.saf")).unwrap();
    let r = cleanup::clean(d.path(), &cleanup::ids_of_kinds(&h, &Kind::ALL), &Job::silent()).unwrap_err();
    assert!(matches!(r, Error::Refused(_)) && r.to_string().contains("data.saf"), "{r}");
    drop(held);
    assert_eq!(snapshot(d.path()), before);
    assert!(!d.path().join(DEFAULT_DIR).exists(), "no cleanup folder was made");
    assert!(cleanup::clean(d.path(), &cleanup::ids_of_kinds(&h, &Kind::ALL), &Job::silent()).is_ok());
}

#[cfg(windows)]
#[test]
fn a_failed_rename_stops_with_the_manifest_written_first_and_updated_after_each() {
    use std::os::windows::fs::OpenOptionsExt;
    let d = client();
    write(&d.path().join(".a.1.shaiya-tmp"), b"tmp a");
    write(&d.path().join(".b.1.shaiya-tmp"), b"tmp b");
    write(&d.path().join(".c.1.shaiya-tmp"), b"tmp c");
    let h = health::report(d.path()).unwrap();
    // share mode 1 (read only): another program holds .b open, so its rename fails
    let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(d.path().join(".b.1.shaiya-tmp")).unwrap();
    let r = cleanup::clean(d.path(), &cleanup::ids_of_kinds(&h, &[Kind::StrayTemp]), &Job::silent()).unwrap_err();
    assert!(r.to_string().contains(".b.1.shaiya-tmp") && r.to_string().contains("1 moved before it"), "{r}");
    let folders = cleanup::list_folders(d.path());
    assert_eq!((folders.len(), folders[0].state.as_str()), (1, "moving"));
    let folder = d.path().join(DEFAULT_DIR).join(&folders[0].name);
    let m = Manifest::read(&folder).unwrap();
    let states: Vec<(&str, &str)> = m.items.iter().map(|i| (i.from.as_str(), i.state.as_str())).collect();
    assert_eq!(states, [(".a.1.shaiya-tmp", "moved"), (".b.1.shaiya-tmp", "planned"), (".c.1.shaiya-tmp", "planned")], "the manifest lists every file and says how far it got");
    assert!(folder.join(".a.1.shaiya-tmp").exists() && d.path().join(".b.1.shaiya-tmp").exists() && d.path().join(".c.1.shaiya-tmp").exists());
    drop(held);
    let rep = cleanup::put_back(&folder, &Job::silent()).unwrap();
    assert_eq!((rep.restored.len(), rep.already_in_place.len()), (1, 2));
    assert_eq!(std::fs::read(d.path().join(".a.1.shaiya-tmp")).unwrap(), b"tmp a");
}

#[test]
fn clean_finishes_an_interrupted_compaction_first_and_says_so() {
    let d = client();
    write(&d.path().join(".z.1.shaiya-tmp"), b"tmp");
    // the leftovers of a compaction that never committed: recovery removes them
    write(&d.path().join("data.sah.compact"), b"half");
    write(&d.path().join("data.saf.compact"), b"half");
    let h = health::report(d.path()).unwrap();
    let rep = cleanup::clean(d.path(), &cleanup::ids_of_kinds(&h, &[Kind::StrayTemp]), &Job::silent()).unwrap();
    assert!(rep.notes.iter().any(|n| n.contains("interrupted compaction")), "{:?}", rep.notes);
    assert!(!d.path().join("data.sah.compact").exists() && !d.path().join(".z.1.shaiya-tmp").exists());
    assert_eq!(rep.moved, [".z.1.shaiya-tmp"]);
}
