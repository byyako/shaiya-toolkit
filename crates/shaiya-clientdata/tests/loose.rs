//! `LooseTree`: listing, letter case, links, caps, cancel, stamps. Synthetic folders only.

use shaiya_archive::loose::{detect, LooseLayout};
use shaiya_clientdata::loose::{split_case_duplicates, Limits, LooseTree};
use shaiya_core::{Cancel, Error, Job, Silent};
use shaiya_testkit::TempDir;
use std::path::{Path, PathBuf};

fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn layout(root: &Path) -> LooseLayout {
    // a folder that holds two marker folders is layout 3: data = root
    put(root, "Character/a.txt", b"c");
    put(root, "Item/b.txt", b"i");
    detect(root).unwrap().unwrap()
}

fn open(l: &LooseLayout) -> LooseTree {
    LooseTree::open(l, &Job::silent()).unwrap()
}

#[test]
fn a_tree_lists_its_files_in_any_letter_case() {
    let t = TempDir::new("lt-case");
    let l = layout(t.path());
    put(t.path(), "World/Maps/One.WLD", b"12345");
    put(t.path(), "name with space/%E4%ba.txt", b"x");
    let tree = open(&l);
    assert_eq!(tree.paths(), vec!["Character/a.txt", "Item/b.txt", "World/Maps/One.WLD", "name with space/%E4%ba.txt"]);
    for q in ["world/maps/one.wld", "WORLD\\Maps/ONE.WLD", "/World//Maps/One.WLD"] {
        assert_eq!(tree.len(q), Some(5), "{q}");
        assert_eq!(tree.get(q).unwrap().rel, "World/Maps/One.WLD");
    }
    assert_eq!(tree.read("world/maps/one.wld", u64::MAX, &Job::silent()).unwrap(), b"12345");
    assert_eq!(tree.read("world/maps/one.wld", 2, &Job::silent()).unwrap(), b"12");
    assert_eq!(tree.read_range("world/maps/one.wld", 1, 3).unwrap(), b"234");
    assert!(tree.read_range("world/maps/one.wld", 9, 3).unwrap().is_empty());
    // %XX names are not decoded
    assert!(tree.get("name with space/%E4%ba.txt").is_some() && tree.get("name with space/\u{e4}.txt").is_none());
    // paths that leave the folder or name no file
    for q in ["../x", "World/../Item/b.txt", "C:/x", "World/Maps", "", "World/Maps/Two.WLD", "./Item/b.txt"] {
        assert!(tree.get(q).is_none() && tree.len(q).is_none() && tree.stamp(q).is_none(), "{q}");
    }
    assert!(matches!(tree.read("../x", 5, &Job::silent()), Err(Error::Io { .. })));
    assert!(tree.notes().is_empty());
}

#[test]
fn case_only_duplicates_are_reported_and_left_out() {
    // the pure rule: every spelling of a clashing name goes out, the rest stays
    let (kept, dups) = split_case_duplicates(vec!["A.txt".into(), "a.TXT".into(), "b.txt".into(), "C".into(), "c".into(), "c".into()]);
    assert_eq!((kept, dups), (vec!["b.txt".to_string()], vec!["A.txt".to_string(), "a.TXT".into(), "C".into(), "c".into(), "c".into()]));
    // on a disk that keeps both spellings (not Windows or macOS defaults) the tree does the same
    let t = TempDir::new("lt-dups");
    let l = layout(t.path());
    put(t.path(), "World/Dup.txt", b"1");
    put(t.path(), "World/dup.TXT", b"2");
    put(t.path(), "World/ok.txt", b"3");
    let both = std::fs::read_dir(t.path().join("World")).unwrap().count() == 3;
    let tree = open(&l);
    if both {
        assert!(tree.get("World/dup.txt").is_none() && tree.get("World/ok.txt").is_some());
        let notes = tree.notes().join(" | ");
        assert!(notes.contains("World/Dup.txt") && notes.contains("World/dup.TXT") && notes.contains("letter case"), "{notes}");
        assert_eq!(tree.skipped().case_duplicates.len(), 2);
    } else {
        eprintln!("this disk folds letter case: the file case was covered by the pure rule only");
        assert!(tree.get("World/ok.txt").is_some());
    }
}

#[test]
fn the_backup_folder_and_temps_are_not_listed() {
    let t = TempDir::new("lt-skip");
    let l = layout(t.path());
    put(t.path(), "shaiya-toolkit-backups/20260101-x/journal.txt", b"j");
    put(t.path(), "World/.a.wld.12.shaiya-tmp", b"t");
    put(t.path(), "World/B.SHAIYA-TMP", b"t");
    put(t.path(), "World/a.wld", b"w");
    // only the top-level backup folder is the toolkit's
    put(t.path(), "World/shaiya-toolkit-backups/keep.txt", b"k");
    let tree = open(&l);
    assert_eq!(tree.paths(), vec!["Character/a.txt", "Item/b.txt", "World/a.wld", "World/shaiya-toolkit-backups/keep.txt"]);
    assert_eq!(tree.skipped().temps, 2);
    assert!(tree.notes().iter().any(|n| n.contains("temporary files")));
}

#[test]
fn caps_refuse_or_leave_out_and_say_so() {
    let t = TempDir::new("lt-caps");
    let l = layout(t.path());
    put(t.path(), "World/a/b/c/d.txt", b"d");
    put(t.path(), "World/long-name-for-a-file.txt", b"l");
    let job = Job::silent();
    // too many files: refused, not truncated
    let e = LooseTree::open_with(&l, Limits { max_files: 3, ..Limits::default() }, &job).err().unwrap();
    assert!(matches!(e, Error::Refused(_)) && e.to_string().contains("more than 3"), "{e}");
    // depth: World/a (1.. ) the folder `a` is at depth 2, `b` at 3: max_depth 2 enters World and a only
    let tree = LooseTree::open_with(&l, Limits { max_depth: 2, ..Limits::default() }, &job).unwrap();
    assert!(tree.get("World/a/b/c/d.txt").is_none() && tree.skipped().too_deep == 1);
    assert!(tree.notes().iter().any(|n| n.contains("not entered")));
    // path length
    let tree = LooseTree::open_with(&l, Limits { max_path: 20, ..Limits::default() }, &job).unwrap();
    assert!(tree.get("World/long-name-for-a-file.txt").is_none() && tree.skipped().too_long >= 1);
    assert!(tree.get("Item/b.txt").is_some());
    // a file over the read cap is listed with its size and never read
    put(t.path(), "World/big.bin", &[7u8; 100]);
    let tree = LooseTree::open_with(&l, Limits { max_read: 50, ..Limits::default() }, &job).unwrap();
    assert_eq!(tree.len("World/big.bin"), Some(100));
    let e = tree.read("World/big.bin", 10, &job).unwrap_err();
    assert!(matches!(e, Error::Refused(_)), "{e}");
    assert!(tree.read_range("World/big.bin", 0, 10).is_err());
}

#[test]
fn the_number_of_folders_and_the_entries_of_one_folder_are_capped() {
    let t = TempDir::new("lt-caps-dirs");
    let l = layout(t.path());
    let job = Job::silent();
    for n in 0..6 {
        put(t.path(), &format!("Many/d{n}/f.txt"), b"x");
    }
    let e = match LooseTree::open_with(&l, Limits { max_dirs: 4, ..Limits::default() }, &job) { Err(e) => e.to_string(), Ok(_) => panic!("refused") };
    assert!(e.contains("more than 4 folders"), "{e}");
    assert!(LooseTree::open_with(&l, Limits { max_dirs: 500, ..Limits::default() }, &job).is_ok());
    // one folder with more entries than the file cap is refused while it is being collected
    let e = match LooseTree::open_with(&l, Limits { max_files: 5, ..Limits::default() }, &job) { Err(e) => e.to_string(), Ok(_) => panic!("refused") };
    assert!(e.contains("more than 5"), "{e}");
}

#[test]
fn a_cancelled_listing_and_read_end_cancelled() {
    let t = TempDir::new("lt-cancel");
    let l = layout(t.path());
    let cancel = Cancel::new();
    cancel.cancel();
    let job = Job::new(&Silent, cancel);
    assert!(matches!(LooseTree::open(&l, &job), Err(Error::Cancelled)));
    let tree = open(&l);
    assert!(matches!(tree.read("Item/b.txt", 10, &job), Err(Error::Cancelled)));
}

#[test]
fn a_file_changed_on_disk_changes_its_stamp() {
    let t = TempDir::new("lt-stamp");
    let l = layout(t.path());
    put(t.path(), "World/a.bin", b"1234");
    let tree = open(&l);
    let first = tree.stamp("world/A.BIN").unwrap();
    assert_eq!(first.1, 4);
    assert_eq!(tree.stamp("world/A.BIN"), Some(first), "unchanged: same stamp");
    // same length, a later modification time (set by hand: no waiting)
    let f = std::fs::OpenOptions::new().write(true).open(t.path().join("World/a.bin")).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(first.0) + std::time::Duration::from_secs(5)).unwrap();
    drop(f);
    let second = tree.stamp("world/a.bin").unwrap();
    assert_ne!(second, first);
    assert_eq!(second.1, 4);
    // a different length
    std::fs::write(t.path().join("World/a.bin"), b"123456").unwrap();
    assert_eq!(tree.stamp("world/a.bin").unwrap().1, 6);
    assert_eq!(tree.len("world/a.bin"), Some(6));
    // a file that is gone has no stamp and does not read
    std::fs::remove_file(t.path().join("World/a.bin")).unwrap();
    assert!(tree.stamp("world/a.bin").is_none() && tree.read("world/a.bin", 9, &Job::silent()).is_err());
}

// ---- links ---------------------------------------------------------------------------------------------------------------------------

/// A new folder on a file system that can hold links (NTFS under `%LOCALAPPDATA%\Temp` on Windows; the scratch folder is exFAT and cannot).
struct LinkDir(PathBuf);

impl LinkDir {
    fn new(tag: &str) -> LinkDir {
        let base = std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Temp")).filter(|p| p.is_dir()).unwrap_or_else(std::env::temp_dir);
        let d = base.join(format!("shaiya-w16-{tag}-{}", std::process::id()));
        LinkDir::clear(&d);
        std::fs::create_dir_all(&d).unwrap();
        LinkDir(d)
    }

    /// Links are removed as links first so the target's files are never touched.
    fn clear(d: &Path) {
        fn walk(d: &Path) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                    let _ = std::fs::remove_dir(&p).or_else(|_| std::fs::remove_file(&p));
                } else if p.is_dir() {
                    walk(&p);
                }
            }
        }
        walk(d);
        let _ = std::fs::remove_dir_all(d);
    }
}

impl Drop for LinkDir {
    fn drop(&mut self) {
        LinkDir::clear(&self.0);
    }
}

fn link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output().is_ok_and(|o| o.status.success());
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

fn link_file(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link).is_ok();
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link).is_ok();
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

#[test]
fn a_symlink_inside_the_tree_is_refused() {
    let t = LinkDir::new("links");
    let (data, outside) = (t.0.join("data"), t.0.join("outside"));
    std::fs::create_dir_all(&data).unwrap();
    put(&outside, "secret.txt", b"secret");
    let l = layout(&data);
    let dir_ok = link_dir(&outside, &data.join("World"));
    let file_ok = link_file(&outside.join("secret.txt"), &data.join("Item").join("link.txt"));
    if !dir_ok && !file_ok {
        eprintln!("skipped: links cannot be made here");
        return;
    }
    let tree = open(&l);
    assert!(tree.get("World/secret.txt").is_none(), "nothing is listed through a folder link");
    assert!(tree.get("Item/link.txt").is_none() || !file_ok, "a file link is not listed");
    assert_eq!(tree.skipped().links, usize::from(dir_ok) + usize::from(file_ok));
    assert!(tree.notes().iter().any(|n| n.contains("link")));
    assert!(tree.get("Item/b.txt").is_some());
}

#[test]
fn a_link_that_appears_after_the_listing_refuses_the_read() {
    let t = LinkDir::new("late");
    let (data, outside) = (t.0.join("data"), t.0.join("outside"));
    std::fs::create_dir_all(&data).unwrap();
    put(&outside, "x.txt", b"outside");
    let l = layout(&data);
    put(&data, "World/x.txt", b"inside");
    let tree = open(&l);
    assert_eq!(tree.read("World/x.txt", 99, &Job::silent()).unwrap(), b"inside");
    // swap the folder for a link to another folder holding a file of the same name
    std::fs::remove_dir_all(data.join("World")).unwrap();
    if !link_dir(&outside, &data.join("World")) {
        eprintln!("skipped: links cannot be made here");
        return;
    }
    assert!(tree.read("World/x.txt", 99, &Job::silent()).is_err(), "the read does not follow the new link");
    assert!(tree.read_range("World/x.txt", 0, 5).is_err());
    assert!(tree.len("World/x.txt").is_none() && tree.stamp("World/x.txt").is_none());
}
