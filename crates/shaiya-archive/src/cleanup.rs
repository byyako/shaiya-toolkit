//! Clean-up of a client folder's leftovers: files are moved, never deleted, into `shaiya-toolkit-backups/cleanup-<UTC>/`, and **Put back**
//! moves them where they were.
//!
//! Order of a clean-up: `client::recover`, refuse while a pending marker remains, the in-use check, a fresh [`health::report`], the folder
//! and `manifest.json` (written atomically with every item `planned` before the first rename), then one rename per file, the manifest
//! rewritten after each. A rename is inside the client folder (one volume); a rename that fails stops the run, and nothing is copied and
//! deleted instead. An interrupted run leaves the manifest, so [`put_back`] can undo whatever was moved.

use crate::client::{self, ClientFiles};
use crate::health::{self, Health, Kind};
use crate::journal::{safe_rel, Marker, DEFAULT_DIR};
use shaiya_core::fs::write_atomic;
use shaiya_core::time::{utc_iso, utc_stamp};
use shaiya_core::{Error, Job, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const FOLDER_PREFIX: &str = "cleanup-";
pub const MANIFEST: &str = "manifest.json";
const TOOL: &str = concat!("shaiya ", env!("CARGO_PKG_VERSION"));

/// One moved (or planned) file in `manifest.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The leftover's id (`<kind>:<first file>`); a swap pair has two items with the same group.
    pub group: String,
    pub kind: String,
    /// Path in the client folder, `/` separated.
    pub from: String,
    /// Path inside the cleanup folder.
    pub to: String,
    pub bytes: u64,
    /// `planned` (written before the rename), `moved`, `put-back`.
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub created_utc: String,
    pub tool: String,
    /// `moving` while a clean-up runs or was interrupted, `moved` when it finished, `put-back` after a complete Put back.
    pub state: String,
    pub items: Vec<Item>,
}

fn bad(file: &Path, what: impl Into<String>) -> Error {
    Error::format(file.display().to_string(), None, what)
}

impl Manifest {
    pub fn to_json(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|i| serde_json::json!({"group": i.group, "kind": i.kind, "from": i.from, "to": i.to, "bytes": i.bytes, "state": i.state}))
            .collect();
        let v = serde_json::json!({"format": 1, "id": self.id, "created_utc": self.created_utc, "tool": self.tool, "state": self.state, "items": items});
        let mut s = serde_json::to_string_pretty(&v).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn parse(text: &str, file: &Path) -> Result<Manifest> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| bad(file, format!("manifest.json is not valid JSON ({e})")))?;
        let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let (Some(id), Some(state), Some(items)) = (s(&v, "id"), s(&v, "state"), v.get("items").and_then(|x| x.as_array())) else {
            return Err(bad(file, "manifest.json lacks id, state or items"));
        };
        let mut out = Vec::new();
        for i in items {
            let (Some(from), Some(to), Some(st)) = (s(i, "from"), s(i, "to"), s(i, "state")) else {
                return Err(bad(file, "a manifest item lacks from, to or state"));
            };
            out.push(Item { group: s(i, "group").unwrap_or_else(|| from.clone()), kind: s(i, "kind").unwrap_or_default(), from, to, bytes: i.get("bytes").and_then(|x| x.as_u64()).unwrap_or(0), state: st });
        }
        Ok(Manifest { id, created_utc: s(&v, "created_utc").unwrap_or_default(), tool: s(&v, "tool").unwrap_or_default(), state, items: out })
    }

    pub fn read(folder: &Path) -> Result<Manifest> {
        let p = folder.join(MANIFEST);
        let text = std::fs::read_to_string(&p).map_err(|e| Error::io(&p, e))?;
        Manifest::parse(&text, &p)
    }

    /// Atomic (temporary file in the same folder, then rename).
    pub fn write(&self, folder: &Path) -> Result<()> {
        write_atomic(&folder.join(MANIFEST), self.to_json().as_bytes())
    }
}

/// A cleanup folder as the Health view lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FolderInfo {
    pub name: String,
    /// The manifest's state, or `unreadable`.
    pub state: String,
    pub items: usize,
    /// Items that are still in the cleanup folder (state `moved`, or `planned` with the file there).
    pub to_put_back: usize,
    pub bytes: u64,
}

/// The `cleanup-*` folders of a client, oldest first.
pub fn list_folders(client: &Path) -> Vec<FolderInfo> {
    let root = client.join(DEFAULT_DIR);
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(FOLDER_PREFIX) || !e.path().is_dir() {
            continue;
        }
        match Manifest::read(&e.path()) {
            Ok(m) => {
                let there = |i: &Item| i.state != "put-back" && e.path().join(path_of(&i.to)).exists();
                v.push(FolderInfo { name, state: m.state.clone(), items: m.items.len(), to_put_back: m.items.iter().filter(|i| there(i)).count(), bytes: m.items.iter().map(|i| i.bytes).sum() });
            }
            Err(_) => v.push(FolderInfo { name, state: "unreadable".into(), items: 0, to_put_back: 0, bytes: 0 }),
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn path_of(rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|p| !p.is_empty()).collect()
}

fn present(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// What a clean-up would move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<health::Leftover>,
    pub files: usize,
    pub bytes: u64,
}

/// The ids of every offered leftover of these kinds.
pub fn ids_of_kinds(h: &Health, kinds: &[Kind]) -> Vec<String> {
    h.leftovers.iter().filter(|l| kinds.contains(&l.kind)).map(|l| l.id.clone()).collect()
}

/// Looks the ids up among the offered leftovers; an id that is not offered (for instance an editor backup while the editor's install
/// record exists) is `Error::Refused`, so nothing outside the offer is ever moved.
pub fn plan(h: &Health, ids: &[String]) -> Result<Plan> {
    let mut items = Vec::new();
    for id in ids {
        match h.leftovers.iter().find(|l| &l.id == id) {
            Some(l) => {
                if !items.iter().any(|i: &health::Leftover| i.id == l.id) {
                    items.push(l.clone());
                }
            }
            None => return Err(Error::Refused(format!("{id} is not offered for clean-up (see `shaiya clients health`)"))),
        }
    }
    let files = items.iter().map(|i| i.files.len()).sum();
    let bytes = items.iter().map(|i| i.bytes).sum();
    Ok(Plan { items, files, bytes })
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CleanReport {
    /// The cleanup folder's name (inside `shaiya-toolkit-backups`); empty when there was nothing to move and no folder was made.
    pub folder: String,
    pub path: PathBuf,
    pub moved: Vec<String>,
    pub bytes: u64,
    /// What recovery did first.
    pub notes: Vec<String>,
}

fn unique_folder(root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
    let base = format!("{FOLDER_PREFIX}{}", utc_stamp(SystemTime::now()));
    for n in 1u32.. {
        let dir = root.join(if n == 1 { base.clone() } else { format!("{base}-{n}") });
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(&dir, e)),
        }
    }
    unreachable!()
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    if present(to) {
        return Err(Error::Refused(format!("{} already exists", to.display())));
    }
    std::fs::rename(from, to).map_err(|e| Error::io(from, e))
}

/// [`clean_by`] with the ids already known (the Health view's ticked rows).
pub fn clean(client: &Path, ids: &[String], job: &Job) -> Result<CleanReport> {
    clean_by(client, &|_| Ok(ids.to_vec()), job)
}

/// Moves the leftovers `pick` chooses (it sees the fresh report taken after recovery) into a new cleanup folder. Refused (nothing moved) when recovery leaves a pending marker, when the
/// game or another program holds the archive, or when an id is not offered.
pub fn clean_by(client: &Path, pick: &dyn Fn(&Health) -> Result<Vec<String>>, job: &Job) -> Result<CleanReport> {
    let files = ClientFiles::locate(client)?;
    let notes = client::recover(client)?;
    if Marker::path(client).exists() {
        return Err(Error::Refused("a pending change is still recorded (data.toolkit-pending); open the client in Shaiya Toolkit or see docs/guides/undo-and-recovery.md".to_string()));
    }
    client::ensure_not_in_use(&files)?;
    let h = health::report(client)?;
    let ids = pick(&h)?;
    let plan = plan(&h, &ids)?;
    if plan.items.is_empty() {
        return Ok(CleanReport { folder: String::new(), path: PathBuf::new(), moved: Vec::new(), bytes: 0, notes });
    }
    job.stage("moving");
    let folder = unique_folder(&client.join(DEFAULT_DIR))?;
    let mut m = Manifest {
        id: folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        created_utc: utc_iso(SystemTime::now()),
        tool: TOOL.to_string(),
        state: "moving".into(),
        items: Vec::new(),
    };
    for l in &plan.items {
        for f in &l.files {
            let bytes = std::fs::metadata(client.join(path_of(f))).map(|x| x.len()).unwrap_or(0);
            m.items.push(Item { group: l.id.clone(), kind: l.kind.name().to_string(), from: f.clone(), to: f.clone(), bytes, state: "planned".into() });
        }
    }
    m.write(&folder)?;
    let mut moved = Vec::new();
    for i in 0..m.items.len() {
        let (from, to) = (client.join(path_of(&m.items[i].from)), folder.join(path_of(&m.items[i].to)));
        if let Err(e) = rename(&from, &to) {
            return Err(Error::Refused(format!(
                "{} was not moved ({e}); {} moved before it; Put back in {} restores them",
                m.items[i].from,
                moved.len(),
                folder.display()
            )));
        }
        m.items[i].state = "moved".into();
        m.write(&folder)?;
        moved.push(m.items[i].from.clone());
    }
    m.state = "moved".into();
    m.write(&folder)?;
    Ok(CleanReport { folder: m.id, path: folder, moved, bytes: plan.bytes, notes })
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PutBackReport {
    pub restored: Vec<String>,
    /// Items whose file was already back in place (never moved, or put back by hand).
    pub already_in_place: Vec<String>,
    /// Items found neither in the cleanup folder nor in place.
    pub missing: Vec<String>,
}

/// The client folder a cleanup folder belongs to (`<client>/shaiya-toolkit-backups/cleanup-<UTC>`), or `Error::Refused`.
pub fn client_of(folder: &Path) -> Result<PathBuf> {
    let abs = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let name = abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let root = abs.parent();
    let in_backups = root.and_then(|r| r.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(DEFAULT_DIR));
    match (name.starts_with(FOLDER_PREFIX), in_backups, root.and_then(Path::parent)) {
        (true, true, Some(client)) => Ok(client.to_path_buf()),
        _ => Err(Error::Refused(format!("{} is not a cleanup folder ({DEFAULT_DIR}/{FOLDER_PREFIX}<UTC>)", folder.display()))),
    }
}

/// Moves everything the manifest lists back to its old name. Refused, with nothing moved, when any destination already exists. The
/// manifest is rewritten after each file, so an interrupted Put back can be run again.
pub fn put_back(folder: &Path, job: &Job) -> Result<PutBackReport> {
    let client = client_of(folder)?;
    let mut m = Manifest::read(folder)?;
    for i in &m.items {
        if !safe_rel(&i.from) || !safe_rel(&i.to) {
            return Err(Error::Refused(format!("the manifest names an unsafe path ({:?} -> {:?}); nothing was moved", i.from, i.to)));
        }
    }
    let mut report = PutBackReport::default();
    let mut conflicts = Vec::new();
    for i in m.items.iter().filter(|i| i.state != "put-back") {
        let (src, dest) = (folder.join(path_of(&i.to)), client.join(path_of(&i.from)));
        if present(&src) && present(&dest) {
            conflicts.push(i.from.clone());
        }
    }
    if !conflicts.is_empty() {
        return Err(Error::Refused(format!("{} already exist{} in the client folder, so nothing was put back: {}", conflicts.len(), if conflicts.len() == 1 { "s" } else { "" }, conflicts.join(", "))));
    }
    job.stage("putting back");
    for i in 0..m.items.len() {
        if m.items[i].state == "put-back" {
            continue;
        }
        let (src, dest) = (folder.join(path_of(&m.items[i].to)), client.join(path_of(&m.items[i].from)));
        if present(&src) {
            rename(&src, &dest)?;
            report.restored.push(m.items[i].from.clone());
        } else if present(&dest) {
            report.already_in_place.push(m.items[i].from.clone());
        } else {
            report.missing.push(m.items[i].from.clone());
            continue;
        }
        m.items[i].state = "put-back".into();
        m.write(folder)?;
    }
    if m.items.iter().all(|i| i.state == "put-back") {
        m.state = "put-back".into();
        m.write(folder)?;
    }
    Ok(report)
}
