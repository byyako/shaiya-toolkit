//! A client whose data is a folder of loose files instead of `data.sah`/`data.saf`: where its folders are, and the backup folder rule.
//! Path logic only (no file format is read here), so the crates that write loose files and the crates that read them share one answer.
//!
//! A chosen folder `P` is looked at in this order (a folder that holds `data.sah` in any letter case is an archive client and is never
//! loose):
//! 1. `P` is named `data` and holds at least two of `Character`, `Item`, `Monster`, `World`, `Effect`: `data = P`, `root = parent(P)`;
//! 2. `P/data` holds at least two of them: `data = P/data`, `root = P`;
//! 3. `P` itself holds at least two of them: `data = root = P` (an extracted client).
//!
//! Nothing here writes a file.

use crate::journal;
use shaiya_core::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// The folders whose presence (two of them) says "this is a client's data folder". Compared without letter case.
pub const MARKER_FOLDERS: [&str; 5] = ["Character", "Item", "Monster", "World", "Effect"];

/// Most entries (files and folders) the Shaiya-GX probe looks at before it gives up (and says "not a GX layout").
pub const GX_PROBE_LIMIT: usize = 20_000;
/// Folders deeper than this below `data` are not entered by the Shaiya-GX probe.
pub const GX_PROBE_DEPTH: usize = 4;

/// Where a loose client's folders are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseLayout {
    /// The install folder: where `game.exe` or `Shaiya GX.exe` would sit. For layout 3 it is `data`.
    pub root: PathBuf,
    /// The folder with `Character/`, `Item/`, ... in it.
    pub data: PathBuf,
    /// Shaiya-GX layout: converted media (`Sound/*.ogg`, `*_dds.png`) was found. Read from the Shaiya-GX repository, docs/data-formats.md lines 18, 79 and 89 of the copy read for this release (cited, not copied).
    pub gx: bool,
    /// The backup folder the person confirmed (never inside `data`); `None` until one is chosen.
    pub backups: Option<PathBuf>,
}

impl LooseLayout {
    /// `true` when `data` is the folder the person chose and also the root (an extracted client): there is no folder next to it that
    /// could hold backups, so none is proposed.
    pub fn data_is_root(&self) -> bool {
        self.data == self.root
    }

    /// One line for the Add dialog, `shaiya clients` and `shaiya info`: which of the three shapes was found and whether it is a Shaiya-GX layout.
    pub fn describe(&self) -> String {
        let shape = if self.data_is_root() {
            "the folder itself holds the data folders (an extracted client)".to_string()
        } else {
            format!("data folder {} under {}", self.data.display(), self.root.display())
        };
        if self.gx {
            format!("{shape}; Shaiya-GX layout (converted media)")
        } else {
            shape
        }
    }
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// The child of `dir` named `name` in any letter case (a real folder, not a link), when there is one.
pub fn child_dir_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    // listed, so the path has the spelling stored on disk
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path()).find(|p| is_real_dir(p))
}

/// How many of the marker folders `dir` holds (real folders, any letter case; a link does not count).
fn marker_count(dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let mut seen = [false; 5];
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(i) = MARKER_FOLDERS.iter().position(|m| m.eq_ignore_ascii_case(&name)) {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                seen[i] = true;
            }
        }
    }
    seen.iter().filter(|s| **s).count()
}

fn holds_data_sah(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah")))
}

/// Finds the loose layout of the chosen folder `dir` (rules above), or `Ok(None)` when it is not one. `Err` when `dir` is not a folder
/// that can be listed. `backups` of the result is always `None` (the person confirms it); `gx` is the bounded probe of [`probe_gx`].
pub fn detect(dir: &Path) -> Result<Option<LooseLayout>> {
    let dir = absolute(dir);
    if !dir.is_dir() {
        return Err(Error::io(&dir, std::io::Error::new(std::io::ErrorKind::NotFound, "not a folder")));
    }
    std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
    if holds_data_sah(&dir) {
        return Ok(None);
    }
    let named_data = dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("data"));
    let found = if named_data && marker_count(&dir) >= 2 {
        let root = dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.clone());
        Some((root, dir.clone()))
    } else if let Some(child) = child_dir_ci(&dir, "data").filter(|c| marker_count(c) >= 2) {
        Some((dir.clone(), child))
    } else if marker_count(&dir) >= 2 {
        Some((dir.clone(), dir.clone()))
    } else {
        None
    };
    Ok(found.map(|(root, data)| {
        let gx = probe_gx(&data);
        LooseLayout { root, data, gx, backups: None }
    }))
}

/// What `detect` looked for, for the message when it finds nothing.
pub fn detect_hint() -> String {
    format!(
        "looked for a folder named data, or one with a data folder inside, or one itself holding at least two of {} (no data.sah)",
        MARKER_FOLDERS.join(", ")
    )
}

/// Whether `data` holds a `*.ogg` file under `Sound/` or a `*_dds.png` file anywhere: the files Shaiya-GX converts (read from Shaiya-GX's
/// docs/data-formats.md of the Shaiya-GX repository, lines 18, 79 and 89 of the copy read for this release; cited, not copied). The probe lists at most [`GX_PROBE_LIMIT`] entries, enters no folder deeper than [`GX_PROBE_DEPTH`] and follows
/// no link; `Sound/` and `interface/` are looked at first. Running out of entries is "not found".
pub fn probe_gx(data: &Path) -> bool {
    struct St {
        left: usize,
    }
    fn walk(dir: &Path, depth: usize, in_sound: bool, st: &mut St) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        let mut subs = Vec::new();
        for e in rd.flatten() {
            if st.left == 0 {
                return false;
            }
            st.left -= 1;
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                if depth < GX_PROBE_DEPTH {
                    subs.push(e.path());
                }
            } else if t.is_file() && ((in_sound && name.ends_with(".ogg")) || name.ends_with("_dds.png")) {
                return true;
            }
        }
        subs.into_iter().any(|p| walk(&p, depth + 1, in_sound, st))
    }
    let mut st = St { left: GX_PROBE_LIMIT };
    // Sound and interface first: that is where the converted files are, and a big Character folder must not use the whole budget
    for first in ["sound", "interface"] {
        if let Some(c) = child_dir_ci(data, first) {
            if walk(&c, 2, first == "sound", &mut st) {
                return true;
            }
        }
    }
    let Ok(rd) = std::fs::read_dir(data) else { return false };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if name == "sound" || name == "interface" {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_file() && name.ends_with("_dds.png") {
            return true;
        }
        if t.is_dir() && walk(&e.path(), 2, false, &mut st) {
            return true;
        }
    }
    false
}

/// The backup folder proposed for a layout: `<root>/shaiya-toolkit-backups` when `data` is not the root (layouts 1 and 2: outside `data`),
/// and **none** for an extracted client (layout 3), where the person must pick a folder outside `data`.
pub fn proposed_backups(layout: &LooseLayout) -> Option<PathBuf> {
    if layout.data_is_root() {
        None
    } else {
        Some(layout.root.join(journal::DEFAULT_DIR))
    }
}

/// `path` made absolute, `.` and `..` resolved by name, and the deepest existing part resolved through links (so a junction into `data`
/// does not hide it); parts that do not exist yet are appended as written.
fn resolved(path: &Path) -> Vec<String> {
    let abs = absolute(path);
    let mut lexical = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }
    // the deepest existing ancestor, canonical; the rest appended as written
    let mut existing = lexical;
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
            (Some(n), Some(p)) => {
                rest.push(n);
                existing = p;
            }
            _ => break,
        }
    }
    let mut base = std::fs::canonicalize(&existing).unwrap_or(existing);
    for n in rest.into_iter().rev() {
        base.push(n);
    }
    let text = base.to_string_lossy().into_owned();
    let text = text.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).or_else(|| text.strip_prefix(r"\\?\").map(str::to_string)).unwrap_or(text);
    text.replace('\\', "/").split('/').filter(|s| !s.is_empty()).map(|s| if cfg!(windows) { s.to_lowercase() } else { s.to_string() }).collect()
}

/// Whether `child` is `parent` or inside it (letter case ignored on Windows; links resolved where the folders exist).
pub fn is_inside(child: &Path, parent: &Path) -> bool {
    let (c, p) = (resolved(child), resolved(parent));
    c.len() >= p.len() && c[..p.len()] == p[..]
}

/// Why `folder` may not be the backup folder of `layout` (`None` when it is fine): inside `data` or equal to it, or empty.
pub fn backup_folder_problem(layout: &LooseLayout, folder: &Path) -> Option<String> {
    if folder.as_os_str().is_empty() {
        return Some("choose a backup folder outside the data folder".to_string());
    }
    if is_inside(folder, &layout.data) {
        return Some(format!("the backup folder {} is the data folder or inside it: choose a backup folder outside the data folder", folder.display()));
    }
    None
}

/// Another saved loose client, as far as the backup folder rule needs it: its name, its data folder and its backup folder (if it has one).
#[derive(Clone, Debug)]
pub struct OtherClient {
    pub name: String,
    pub data: PathBuf,
    pub backups: Option<PathBuf>,
}

/// Why `folder` may not be the backup folder of a client when the saved clients `others` exist (`None` when it is fine): it is another
/// client's backup folder (two clients would share one journal folder and one marker) or it is inside another client's data folder.
pub fn backup_folder_shared_problem(folder: &Path, others: &[OtherClient]) -> Option<String> {
    let same = |a: &Path, b: &Path| is_inside(a, b) && is_inside(b, a);
    for o in others {
        if o.backups.as_deref().is_some_and(|b| same(folder, b)) {
            return Some(format!("the backup folder {} is already the backup folder of the client {}: every client needs a folder of its own", folder.display(), o.name));
        }
        if is_inside(folder, &o.data) {
            return Some(format!("the backup folder {} is inside the data folder of the client {}: choose a backup folder outside every client's data folder", folder.display(), o.name));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::TempDir;

    #[test]
    fn a_backup_folder_of_another_client_or_inside_its_data_is_refused() {
        let t = TempDir::new("loose-shared-backups");
        let (a, b) = (t.path().join("a").join("data"), t.path().join("b").join("data"));
        let others = [
            OtherClient { name: "A".into(), data: a.clone(), backups: Some(t.path().join("a-backups")) },
            OtherClient { name: "B".into(), data: b.clone(), backups: None },
        ];
        for bad in [t.path().join("a-backups"), t.path().join("A-BACKUPS").join("."), a.join("x"), b.clone()] {
            let why = backup_folder_shared_problem(&bad, &others);
            // letter case counts only where the disk ignores it
            if cfg!(windows) || !bad.to_string_lossy().contains("A-BACKUPS") {
                assert!(why.is_some(), "{bad:?}");
            }
        }
        for ok in [t.path().join("b-backups"), t.path().join("a-backups2"), t.path().join("a")] {
            assert!(backup_folder_shared_problem(&ok, &others).is_none(), "{ok:?}");
        }
    }

    fn dirs(root: &Path, names: &[&str]) {
        for n in names {
            std::fs::create_dir_all(root.join(n)).unwrap();
        }
    }

    #[test]
    fn detect_finds_the_three_layouts_and_the_gx_flag() {
        let t = TempDir::new("loose-detect");
        // 1: the chosen folder is named data (any case)
        let one = t.path().join("c1").join("DATA");
        dirs(&one, &["character", "ITEM"]);
        let l = detect(&one).unwrap().unwrap();
        assert_eq!((l.data.clone(), l.root.clone(), l.gx, l.backups.clone()), (one.clone(), one.parent().unwrap().to_path_buf(), false, None));
        // 2: the chosen folder has a data child
        let two = t.path().join("c2");
        dirs(&two.join("Data"), &["World", "Effect"]);
        let l = detect(&two).unwrap().unwrap();
        assert_eq!((l.data, l.root), (two.join("Data"), two.clone()));
        // 3: the chosen folder holds the folders itself
        let three = t.path().join("c3");
        dirs(&three, &["Monster", "world"]);
        let l = detect(&three).unwrap().unwrap();
        assert!(l.data_is_root() && l.root == three && l.data == three);
        // one marker folder is not enough, in any of the three places
        let few = t.path().join("c4");
        dirs(&few.join("data"), &["Item"]);
        dirs(&few, &["Monster"]);
        assert!(detect(&few).unwrap().is_none());
        // a file named like a marker is not a folder
        let files = t.path().join("c5");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("Item"), b"x").unwrap();
        std::fs::write(files.join("World"), b"x").unwrap();
        assert!(detect(&files).unwrap().is_none());
        // the GX flag: an .ogg under Sound/, or a *_dds.png anywhere; an .ogg elsewhere is not enough
        assert!(!detect(&three).unwrap().unwrap().gx);
        dirs(&three, &["Sound/Music"]);
        std::fs::write(three.join("Sound/Music/a.OGG"), b"").unwrap();
        assert!(detect(&three).unwrap().unwrap().gx);
        let other = t.path().join("c6");
        dirs(&other, &["Item", "World", "Effect/deep"]);
        std::fs::write(other.join("Effect/deep/x.ogg"), b"").unwrap();
        assert!(!detect(&other).unwrap().unwrap().gx, "an .ogg outside Sound is not the GX sign");
        std::fs::write(other.join("Effect/deep/atlas_dds.png"), b"").unwrap();
        assert!(detect(&other).unwrap().unwrap().gx);
        // not a folder at all
        assert!(detect(&t.path().join("nothing")).is_err());
    }

    #[test]
    fn a_folder_with_data_sah_is_not_loose() {
        let t = TempDir::new("loose-sah");
        let d = t.path().join("c");
        dirs(&d, &["data/Character", "data/Item", "Monster", "World"]);
        assert!(detect(&d).unwrap().is_some());
        std::fs::write(d.join("DATA.SAH"), b"x").unwrap();
        assert!(detect(&d).unwrap().is_none(), "any letter case of data.sah");
        // also when the chosen folder is the data folder and the index sits in it
        let e = t.path().join("e").join("data");
        dirs(&e, &["Character", "Item"]);
        std::fs::write(e.join("data.sah"), b"x").unwrap();
        assert!(detect(&e).unwrap().is_none());
    }

    #[test]
    fn the_proposed_backup_folder_follows_the_layout_rule() {
        let t = TempDir::new("loose-proposed");
        let (one, two, three) = (t.path().join("a").join("data"), t.path().join("b"), t.path().join("c"));
        dirs(&one, &["Character", "Item"]);
        dirs(&two.join("data"), &["Character", "Item"]);
        dirs(&three, &["Character", "Item"]);
        let l1 = detect(&one).unwrap().unwrap();
        assert_eq!(proposed_backups(&l1), Some(one.parent().unwrap().join("shaiya-toolkit-backups")), "layout 1: beside the data folder, in its parent");
        let l2 = detect(&two).unwrap().unwrap();
        assert_eq!(proposed_backups(&l2), Some(two.join("shaiya-toolkit-backups")), "layout 2: inside the chosen folder, outside data");
        let l3 = detect(&three).unwrap().unwrap();
        assert_eq!(proposed_backups(&l3), None, "layout 3: no proposal");
        for l in [&l1, &l2] {
            let p = proposed_backups(l).unwrap();
            assert!(backup_folder_problem(l, &p).is_none(), "a proposal is never refused");
        }
    }

    #[test]
    fn a_backup_folder_inside_data_is_refused() {
        let t = TempDir::new("loose-backup-problem");
        let two = t.path().join("b");
        dirs(&two.join("data"), &["Character", "Item"]);
        let l = detect(&two).unwrap().unwrap();
        for bad in [l.data.clone(), l.data.join("backups"), l.data.join("Item").join("x").join("y"), l.data.join("..").join("data").join("z"), PathBuf::new()] {
            assert!(backup_folder_problem(&l, &bad).is_some(), "{bad:?}");
        }
        for ok in [two.join("backups"), t.path().join("elsewhere"), t.path().join("b").join("data2"), two.clone()] {
            assert!(backup_folder_problem(&l, &ok).is_none(), "{ok:?}");
        }
        // layout 3: the data folder is the root, so the folder itself is refused and a sibling is fine
        let three = t.path().join("c");
        dirs(&three, &["World", "Item"]);
        let l3 = detect(&three).unwrap().unwrap();
        assert!(backup_folder_problem(&l3, &three).is_some() && backup_folder_problem(&l3, &three.join("shaiya-toolkit-backups")).is_some());
        assert!(backup_folder_problem(&l3, &t.path().join("c-backups")).is_none());
        if cfg!(windows) {
            assert!(backup_folder_problem(&l, &PathBuf::from(l.data.to_string_lossy().to_uppercase()).join("x")).is_some(), "letter case is ignored on Windows");
        }
    }
}
