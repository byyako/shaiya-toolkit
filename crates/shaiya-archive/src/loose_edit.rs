//! Changing a loose data folder: add, replace and remove files with a journal, undo and recovery. The loose twin of `edit` + `journal`
//! for a client that has no `data.sah` (see [`crate::loose`]). It needs no file reader: the caller passes a [`FormatCheck`] that says
//! whether the bytes of a file of a known type read to the last byte.
//!
//! The rules, in the order they are applied:
//! 1. [`plan`] checks everything before any byte is written and refuses the whole change with the list when one name is not allowed:
//!    names that leave the data folder, Windows-hostile names, case-only twins, a link on the way, converted Shaiya-GX media, a file that
//!    fails its reader, a put and a remove of the same file. An existing file keeps ITS spelling (letter case) on disk.
//! 2. [`apply`] needs a person-confirmed backup folder outside the data folder (`LooseLayout::backups`), a game that is not running
//!    (or a confirmation when this system cannot tell), and files that are as they were when the change was planned. It writes the
//!    journal (copies of every file it overwrites or removes, with checksums) and the pending marker first, then each put through
//!    `write_atomic_checked` (the temp file is read back and checked before it replaces the target), then the removes. A failure or a
//!    cancel puts everything back before it returns.
//! 3. [`rollback`] undoes the newest applied change of the journal folder only, and only while the files are as the change left
//!    them (`force` restores anyway and saves what was there first, inside the journal folder).
//! 4. [`recover`] finishes what an interrupted change or rollback left: an unfinished apply is undone, an unfinished rollback is finished.
//!
//! Nothing is written in any folder other than the data folder (the change itself) and the backup folder (journal, copies, marker).
//! Nothing is deleted from the data folder without a copy in the journal.

use crate::journal::{self, safe_rel, State};
use crate::loose::{backup_folder_problem, LooseLayout};
use shaiya_core::fs::{out_name_problem, same_file, write_atomic_checked, write_atomic_from_with, Sync};
use shaiya_core::hash::{sha256, sha256_file, to_hex, Hasher};
use shaiya_core::time::utc_iso;
use shaiya_core::{Error, Job, Result, Unit};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Says whether the bytes of the file `rel` (below the data folder) are acceptable: `Ok` for a type it does not know, an error that names
/// the problem for a known model, effect or SData file that does not read to the last byte. Built by the caller from the readers.
pub type FormatCheck<'a> = &'a dyn Fn(&str, &[u8]) -> Result<()>;

/// The marker that exists in the backup folder only while a change or a rollback is running or was interrupted.
pub const MARKER: &str = "loose.toolkit-pending";
const JOURNAL_FILE: &str = "journal.txt";
const TOOL: &str = concat!("shaiya ", env!("CARGO_PKG_VERSION"));

/// Most files (puts and removes together) one change may hold.
pub const MAX_CHANGE_FILES: usize = 20_000;
/// Longest file one put may hold (the bytes are in memory).
pub const MAX_PUT_BYTES: u64 = 512 << 20;
/// Most bytes all puts of one change may hold together.
pub const MAX_CHANGE_BYTES: u64 = 1 << 30;
/// The same limits the loose tree lists with: a longer path or a deeper folder would be written and then not seen.
pub const MAX_PATH_BYTES: usize = 1024;
pub const MAX_FOLDER_DEPTH: usize = 16;
const MAX_NAME_BYTES: usize = 255;
const MAX_RECORDS: usize = 100_000;
const MAX_JOURNAL_BYTES: u64 = 64 << 20;

/// The top-level folders of a Shaiya-GX data folder that hold converted media (`interface/` and `Cursor/` PNG, `Sound/` OGG), which this
/// toolkit never writes. Read from Shaiya-GX's data-format document.
const GX_MEDIA_FOLDERS: [&str; 3] = ["interface", "sound", "cursor"];

/// One file to write: its path below the data folder (`/` or `\`) and its whole content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoosePut {
    pub rel: String,
    pub bytes: Vec<u8>,
}

/// What a change does: files to write (added when missing, replaced when there) and files to remove.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LooseChange {
    pub puts: Vec<LoosePut>,
    pub removes: Vec<String>,
}

/// What a file looked like when the change was planned; [`apply`] refuses the change when a file is not like this any more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub exists: bool,
    pub len: u64,
    /// Nanoseconds since 1970 (0 when unknown).
    pub mtime: u64,
}

impl Stamp {
    fn of(path: &Path) -> Stamp {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_file() => Stamp {
                exists: true,
                len: m.len(),
                mtime: m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)).unwrap_or(0),
            },
            _ => Stamp::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PlannedPut {
    /// The path below the data folder as it will be written (an existing file keeps its spelling).
    pub rel: String,
    /// The file is there now (it will be replaced); else it is added.
    pub existed: bool,
    pub len: u64,
    #[cfg_attr(feature = "serde", serde(skip))]
    stamp: Stamp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PlannedRemove {
    pub rel: String,
    pub len: u64,
    #[cfg_attr(feature = "serde", serde(skip))]
    stamp: Stamp,
}

/// A checked change, ready for [`apply`]. It holds the bytes and what each target looked like.
#[derive(Clone, Debug)]
pub struct LoosePlan {
    pub puts: Vec<PlannedPut>,
    pub removes: Vec<PlannedRemove>,
    /// Folders the change creates (paths below the data folder, shallowest first).
    pub new_dirs: Vec<String>,
    /// The folder is a Shaiya-GX layout.
    pub gx: bool,
    bytes: Vec<Vec<u8>>,
}

impl LoosePlan {
    /// Added, replaced and removed paths, for a confirmation.
    pub fn counts(&self) -> (usize, usize, usize) {
        let replaced = self.puts.iter().filter(|p| p.existed).count();
        (self.puts.len() - replaced, replaced, self.removes.len())
    }

    /// Bytes the puts write.
    pub fn bytes_written(&self) -> u64 {
        self.puts.iter().map(|p| p.len).sum()
    }
}

/// Whether the game might be using the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Busy {
    Free,
    Busy(String),
    /// This system cannot tell, or neither `Shaiya GX.exe` nor `game.exe` is in the install folder.
    Unknown,
}

/// What the person confirmed and the test hooks of [`apply`] and [`rollback`].
#[derive(Clone, Debug, Default)]
pub struct LooseOptions {
    /// The label of the journal folder and the history line (a patch's file name, "loose edit").
    pub label: String,
    /// The journal kind is `loose-patch` instead of `loose-edit`.
    pub patch: bool,
    /// The person was told "the toolkit cannot tell whether the game is running: stop it first" and said it is stopped.
    pub confirmed_unknown_busy: bool,
    /// Test hook: stop after this many puts as a crash would (no undo, the marker stays) and return an error.
    pub crash_after_puts: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LooseReport {
    pub journal: PathBuf,
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub removed: Vec<String>,
    /// What [`recover`] did before the change (a leftover of an earlier one).
    pub recovered: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RollbackReport {
    pub journal: PathBuf,
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
    /// Files that had been changed since and were saved (inside the journal folder) before they were put back; only with `force`.
    pub saved: Vec<String>,
}

/// One file of a journal.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LooseRec {
    pub rel: String,
    /// The file was there before the change (replaced or removed); else it was created.
    pub existed: bool,
    /// SHA-256 of the copy kept in the journal.
    pub sha256_before: Option<String>,
    /// SHA-256 of what the change wrote; `None` for a removed file.
    pub sha256_after: Option<String>,
    /// The copy, relative to the journal folder.
    pub backup: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LooseJournal {
    pub dir: PathBuf,
    pub id: String,
    /// `loose-edit` or `loose-patch`.
    pub kind: String,
    pub label: String,
    pub created_utc: String,
    pub tool: String,
    /// The data folder the change was made in.
    pub data: String,
    pub state: State,
    pub records: Vec<LooseRec>,
    /// Folders the change created (shallowest first).
    pub dirs: Vec<String>,
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn bad_text(file: &str, what: impl Into<String>) -> Error {
    Error::format(file, None, what)
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl LooseJournal {
    pub fn to_text(&self) -> String {
        let mut t = String::new();
        let mut kv = |k: &str, v: &str| {
            t.push_str(k);
            t.push('=');
            t.push_str(v);
            t.push('\n');
        };
        kv("format", "2");
        kv("id", &self.id);
        kv("kind", &self.kind);
        kv("label", &one_line(&self.label));
        kv("created_utc", &self.created_utc);
        kv("tool", &one_line(&self.tool));
        kv("data", &one_line(&self.data));
        kv("state", match &self.state {
            State::Pending => "pending",
            State::Applied => "applied",
            State::RolledBack { .. } => "rolled-back",
        });
        kv("rolled_back_utc", if let State::RolledBack { at_utc } = &self.state { at_utc } else { "" });
        for d in &self.dirs {
            kv("dir", &one_line(d));
        }
        for r in &self.records {
            let line = format!(
                "{}\t{}\t{}\t{}\t{}",
                u8::from(r.existed),
                r.sha256_before.as_deref().unwrap_or("-"),
                r.sha256_after.as_deref().unwrap_or("-"),
                r.backup.as_deref().unwrap_or("-"),
                one_line(&r.rel)
            );
            kv("loose", &line);
        }
        t
    }

    pub fn parse(dir: &Path, text: &str) -> Result<LooseJournal> {
        let file = dir.join(JOURNAL_FILE).display().to_string();
        let mut j = LooseJournal {
            dir: dir.to_path_buf(),
            id: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            kind: String::new(),
            label: String::new(),
            created_utc: String::new(),
            tool: String::new(),
            data: String::new(),
            state: State::Pending,
            records: Vec::new(),
            dirs: Vec::new(),
        };
        let (mut format, mut seen_state) = (String::new(), false);
        let (mut state_name, mut rolled_at) = (String::new(), String::new());
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "format" => format = v.trim().to_string(),
                "id" if !v.is_empty() => j.id = v.to_string(),
                "kind" => j.kind = v.to_string(),
                "label" => j.label = v.to_string(),
                "created_utc" => j.created_utc = v.to_string(),
                "tool" => j.tool = v.to_string(),
                "data" => j.data = v.to_string(),
                "state" => {
                    state_name = v.trim().to_string();
                    seen_state = true;
                }
                "rolled_back_utc" => rolled_at = v.to_string(),
                "dir" => {
                    if j.dirs.len() >= MAX_RECORDS {
                        return Err(bad_text(&file, "too many dir lines"));
                    }
                    j.dirs.push(v.to_string());
                }
                "loose" => {
                    if j.records.len() >= MAX_RECORDS {
                        return Err(bad_text(&file, "too many loose lines"));
                    }
                    let mut p = v.splitn(5, '\t');
                    let (existed, before, after, backup, rel) = (p.next(), p.next(), p.next(), p.next(), p.next());
                    let (Some(existed), Some(before), Some(after), Some(backup), Some(rel)) = (existed, before, after, backup, rel) else {
                        return Err(bad_text(&file, "a loose= line needs five tab-separated fields"));
                    };
                    for h in [before, after] {
                        if h != "-" && !hex64(h) {
                            return Err(bad_text(&file, "a checksum is not 64 hex digits"));
                        }
                    }
                    j.records.push(LooseRec {
                        rel: rel.to_string(),
                        existed: existed == "1",
                        sha256_before: (before != "-").then(|| before.to_string()),
                        sha256_after: (after != "-").then(|| after.to_string()),
                        backup: (backup != "-").then(|| backup.to_string()),
                    });
                }
                _ => {}
            }
        }
        if format != "2" {
            return Err(bad_text(&file, "not a loose journal (format=2 expected)"));
        }
        if !matches!(j.kind.as_str(), "loose-edit" | "loose-patch") {
            return Err(bad_text(&file, "not a loose journal (kind)"));
        }
        if !seen_state {
            return Err(bad_text(&file, "journal.txt lacks state"));
        }
        j.state = match state_name.as_str() {
            "pending" => State::Pending,
            "applied" => State::Applied,
            "rolled-back" => State::RolledBack { at_utc: rolled_at },
            other => return Err(bad_text(&file, format!("unknown state {other:?}"))),
        };
        Ok(j)
    }

    pub fn read(dir: &Path) -> Result<LooseJournal> {
        let p = dir.join(JOURNAL_FILE);
        let len = std::fs::metadata(&p).map_err(|e| Error::io(&p, e))?.len();
        if len > MAX_JOURNAL_BYTES {
            return Err(bad_text(&p.display().to_string(), "the journal is larger than the toolkit reads"));
        }
        let text = std::fs::read_to_string(&p).map_err(|e| Error::io(&p, e))?;
        LooseJournal::parse(dir, &text)
    }

    pub fn write(&self) -> Result<()> {
        let p = self.dir.join(JOURNAL_FILE);
        write_atomic_from_with(&p, &mut self.to_text().as_bytes(), Sync::Yes, &Job::silent()).map(|_| ())
    }

    fn unsafe_path(&self) -> Option<&str> {
        self.records
            .iter()
            .flat_map(|r| std::iter::once(r.rel.as_str()).chain(r.backup.as_deref()))
            .chain(self.dirs.iter().map(String::as_str))
            .find(|p| !safe_rel(p))
    }
}

// ---------------------------------------------------------------------------------------------- names and links

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn join_rel(base: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|p| !p.is_empty()).fold(base.to_path_buf(), |p, s| p.join(s))
}

/// The refusal text for one or many problems.
fn refusal(mut problems: Vec<String>) -> Error {
    Error::Refused(if problems.len() == 1 { problems.remove(0) } else { format!("{} problems:\n  {}", problems.len(), problems.join("\n  ")) })
}

/// Why `rel` may not be written, removed or looked up as a loose file of a client, or `None`: the name rules of `edit::loose_name_problem`
/// plus the names Windows cannot hold, a name over 255 bytes, a path over [`MAX_PATH_BYTES`] and a folder deeper than [`MAX_FOLDER_DEPTH`].
pub fn name_problem(rel: &str) -> Option<String> {
    if let Some(why) = crate::edit::loose_name_problem(rel) {
        return Some(why);
    }
    let n = rel.replace('\\', "/");
    if let Some(why) = out_name_problem(&n) {
        return Some(format!("{rel:?}: {why}"));
    }
    if n.len() > MAX_PATH_BYTES {
        return Some(format!("{rel:?} is longer than the {MAX_PATH_BYTES} bytes the toolkit lists"));
    }
    let parts: Vec<&str> = n.split('/').collect();
    if parts.len() - 1 > MAX_FOLDER_DEPTH {
        return Some(format!("{rel:?} is nested deeper than the {MAX_FOLDER_DEPTH} folders the toolkit lists"));
    }
    if parts.iter().any(|p| p.len() > MAX_NAME_BYTES) {
        return Some(format!("{rel:?} has a name longer than {MAX_NAME_BYTES} bytes"));
    }
    if parts.iter().any(|p| p.chars().any(char::is_control)) {
        return Some(format!("{rel:?} has a control character in it"));
    }
    None
}

/// The converted-media rule of a Shaiya-GX layout: why `rel` is never written or removed there, or `None`.
fn gx_problem(rel: &str) -> Option<String> {
    let n = lower(&rel.replace('\\', "/"));
    let first = n.split('/').next().unwrap_or("");
    let name = n.rsplit('/').next().unwrap_or("");
    let converted = GX_MEDIA_FOLDERS.contains(&first) || name.ends_with(".wav") || name.ends_with(".ogg") || name.ends_with("_dds.png");
    converted.then(|| format!("{rel}: Shaiya-GX stores this as PNG/OGG: not changed"))
}

struct Resolved {
    rel: String,
    existed: bool,
    stamp: Stamp,
    new_dirs: Vec<String>,
}

/// Looks `rel` up below the data folder one part at a time without letter case: an existing part keeps its spelling, every existing part
/// is looked at for a link, parts that are not there yet are taken as written (their folders are `new_dirs`).
fn resolve(layout: &LooseLayout, rel: &str, problems: &mut Vec<String>) -> Option<Resolved> {
    let norm = rel.replace('\\', "/");
    let parts: Vec<&str> = norm.split('/').collect();
    let mut cur = layout.data.clone();
    let mut spelled: Vec<String> = Vec::new();
    let mut exists = true;
    let mut new_dirs = Vec::new();
    let mut existed = false;
    let mut stamp = Stamp::default();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if !exists {
            spelled.push((*part).to_string());
            if !last {
                new_dirs.push(spelled.join("/"));
            }
            continue;
        }
        let rd = match std::fs::read_dir(&cur) {
            Ok(rd) => rd,
            Err(e) => {
                problems.push(format!("{rel}: {}: {e}", cur.display()));
                return None;
            }
        };
        let key = lower(part);
        let hits: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| lower(n) == key).collect();
        match hits.len() {
            0 => {
                exists = false;
                spelled.push((*part).to_string());
                if !last {
                    new_dirs.push(spelled.join("/"));
                }
            }
            1 => {
                let p = cur.join(&hits[0]);
                if is_link(&p) {
                    problems.push(format!("{rel}: {} is a link (a link could lead out of the data folder)", p.display()));
                    return None;
                }
                let is_dir = p.is_dir();
                if last {
                    if is_dir {
                        problems.push(format!("{rel}: {} is a folder, not a file", p.display()));
                        return None;
                    }
                    existed = true;
                    stamp = Stamp::of(&p);
                } else if !is_dir {
                    problems.push(format!("{rel}: {} is a file, not a folder", p.display()));
                    return None;
                }
                spelled.push(hits[0].clone());
                cur = p;
            }
            _ => {
                problems.push(format!("{rel}: {} holds {} names that differ only in letter case ({}); the toolkit lists none of them", cur.display(), hits.len(), hits.join(", ")));
                return None;
            }
        }
    }
    Some(Resolved { rel: spelled.join("/"), existed, stamp, new_dirs })
}

/// Checks a change against the data folder and the rules in the module text. `Err(Refused)` lists every problem; nothing is written.
pub fn plan(layout: &LooseLayout, change: LooseChange, check: FormatCheck<'_>) -> Result<LoosePlan> {
    let mut problems: Vec<String> = Vec::new();
    if change.puts.is_empty() && change.removes.is_empty() {
        return Err(Error::Refused("the change holds no file".to_string()));
    }
    if change.puts.len() + change.removes.len() > MAX_CHANGE_FILES {
        return Err(Error::Refused(format!("a change may hold at most {MAX_CHANGE_FILES} files ({} given)", change.puts.len() + change.removes.len())));
    }
    let mut total = 0u64;
    for p in &change.puts {
        let n = p.bytes.len() as u64;
        total = total.saturating_add(n);
        if n > MAX_PUT_BYTES {
            problems.push(format!("{}: {n} bytes is more than the {MAX_PUT_BYTES} one file may hold", p.rel));
        }
    }
    if total > MAX_CHANGE_BYTES {
        problems.push(format!("the files hold {total} bytes together; a change may hold at most {MAX_CHANGE_BYTES}"));
    }
    match std::fs::symlink_metadata(&layout.data) {
        Ok(m) if m.file_type().is_symlink() => problems.push(format!("the data folder {} is a link; the toolkit does not write through links", layout.data.display())),
        Ok(m) if m.is_dir() => {}
        _ => problems.push(format!("the data folder {} is not there", layout.data.display())),
    }
    if !problems.is_empty() {
        return Err(refusal(problems));
    }
    let gx = layout.gx;
    let mut puts: Vec<PlannedPut> = Vec::new();
    let mut bytes: Vec<Vec<u8>> = Vec::new();
    let mut removes: Vec<PlannedRemove> = Vec::new();
    let mut new_dirs: BTreeMap<String, String> = BTreeMap::new();
    let mut put_keys: HashMap<String, String> = HashMap::new();
    let mut remove_keys: HashSet<String> = HashSet::new();
    for p in change.puts {
        if let Some(why) = name_problem(&p.rel) {
            problems.push(why);
            continue;
        }
        if gx {
            if let Some(why) = gx_problem(&p.rel) {
                problems.push(why);
                continue;
            }
        }
        let Some(r) = resolve(layout, &p.rel, &mut problems) else { continue };
        let key = lower(&r.rel);
        if let Some(other) = put_keys.insert(key.clone(), p.rel.clone()) {
            problems.push(format!("{}: listed twice (also as {other})", p.rel));
            continue;
        }
        if gx && gx_sibling_png(layout, &r.rel) {
            problems.push(format!("{}: Shaiya-GX stores this as PNG: not changed", p.rel));
            continue;
        }
        if let Err(e) = check(&r.rel, &p.bytes) {
            problems.push(format!("{}: {}", p.rel, strip_refused(&e)));
            continue;
        }
        for d in &r.new_dirs {
            // two new folders that differ only in letter case would be two folders on a case-sensitive disk
            match new_dirs.get(&lower(d)) {
                Some(first) if first != d => problems.push(format!("{}: the new folder {d:?} differs only in letter case from {first:?}", p.rel)),
                Some(_) => {}
                None => {
                    new_dirs.insert(lower(d), d.clone());
                }
            }
        }
        puts.push(PlannedPut { rel: r.rel, existed: r.existed, len: p.bytes.len() as u64, stamp: r.stamp });
        bytes.push(p.bytes);
    }
    for rel in &change.removes {
        if let Some(why) = name_problem(rel) {
            problems.push(why);
            continue;
        }
        if gx {
            if let Some(why) = gx_problem(rel) {
                problems.push(why);
                continue;
            }
        }
        let Some(r) = resolve(layout, rel, &mut problems) else { continue };
        if !r.existed {
            problems.push(format!("{rel}: not in the data folder, nothing to remove"));
            continue;
        }
        let key = lower(&r.rel);
        if !remove_keys.insert(key.clone()) {
            problems.push(format!("{rel}: listed twice"));
            continue;
        }
        if put_keys.contains_key(&key) {
            problems.push(format!("{rel}: is both written and removed in this change"));
            continue;
        }
        removes.push(PlannedRemove { rel: r.rel, len: r.stamp.len, stamp: r.stamp });
    }
    // a file where another change makes a folder
    let all_keys: Vec<&String> = put_keys.keys().collect();
    for a in &all_keys {
        if all_keys.iter().any(|b| b.len() > a.len() && b.starts_with(a.as_str()) && b.as_bytes()[a.len()] == b'/') {
            problems.push(format!("{a}: is a file here and a folder in another path of the same change"));
        }
    }
    if !problems.is_empty() {
        return Err(refusal(problems));
    }
    Ok(LoosePlan { puts, removes, new_dirs: sorted_dirs(new_dirs.into_values().collect()), gx, bytes })
}

fn strip_refused(e: &Error) -> String {
    match e {
        Error::Refused(s) => s.clone(),
        other => other.to_string(),
    }
}

fn sorted_dirs(mut v: Vec<String>) -> Vec<String> {
    v.sort_by(|a, b| a.matches('/').count().cmp(&b.matches('/').count()).then_with(|| a.cmp(b)));
    v
}

/// On a Shaiya-GX layout: the folder of `rel` holds `<stem>_<ext>.png` for `<stem>.<ext>` (the converted form of the file).
fn gx_sibling_png(layout: &LooseLayout, rel: &str) -> bool {
    let (dir, name) = match rel.rsplit_once('/') {
        Some((d, n)) => (Some(d), n),
        None => (None, rel),
    };
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    let want = lower(&format!("{stem}_{ext}.png"));
    let folder = dir.map_or_else(|| layout.data.clone(), |d| join_rel(&layout.data, d));
    std::fs::read_dir(folder).is_ok_and(|rd| rd.flatten().any(|e| lower(&e.file_name().to_string_lossy()) == want))
}

/// A file read in full after its size was checked against `cap`; for callers that build a [`LooseChange`] from files on disk.
pub fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if len > cap {
        return Err(Error::Refused(format!("{}: {len} bytes is more than the {cap} the toolkit holds for one file", path.display())));
    }
    let mut out = Vec::with_capacity(len as usize);
    f.take(cap + 1).read_to_end(&mut out).map_err(|e| Error::io(path, e))?;
    if out.len() as u64 != len {
        return Err(Error::Refused(format!("{} changed while it was read", path.display())));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------- the game

/// Whether the game is using the install folder. Windows: an exclusive open of `Shaiya GX.exe` or `game.exe` in the install folder, when
/// one is there (`Busy` when another program has it open, which is what a running program does). Every other case is `Unknown`.
pub fn busy_state(layout: &LooseLayout) -> Busy {
    let exes: Vec<PathBuf> = std::fs::read_dir(&layout.root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = lower(&e.file_name().to_string_lossy());
                    (n == "shaiya gx.exe" || n == "game.exe") && e.file_type().is_ok_and(|t| t.is_file())
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    probe_exes(&exes)
}

#[cfg(windows)]
fn probe_exes(exes: &[PathBuf]) -> Busy {
    use std::os::windows::fs::OpenOptionsExt;
    if exes.is_empty() {
        return Busy::Unknown;
    }
    let mut unknown = false;
    for p in exes {
        match std::fs::OpenOptions::new().write(true).share_mode(0).open(p) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(32) => {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                return Busy::Busy(format!("{name} is running or open in another program"));
            }
            Err(_) => unknown = true,
        }
    }
    if unknown {
        Busy::Unknown
    } else {
        Busy::Free
    }
}

#[cfg(not(windows))]
fn probe_exes(_exes: &[PathBuf]) -> Busy {
    Busy::Unknown
}

/// The sentence the person is asked to confirm when the state is `Unknown`.
pub const UNKNOWN_BUSY_TEXT: &str = "the toolkit cannot tell whether the game is running: stop it first";

fn ensure_not_busy(layout: &LooseLayout, opts: &LooseOptions) -> Result<()> {
    match busy_state(layout) {
        Busy::Free => Ok(()),
        Busy::Busy(why) => Err(Error::Refused(format!("{why}; close the game (and any other program using this client, Shaiya Toolkit included) and try again"))),
        Busy::Unknown if opts.confirmed_unknown_busy => Ok(()),
        Busy::Unknown => Err(Error::Refused(format!("{UNKNOWN_BUSY_TEXT} (confirm that it is stopped to go on)"))),
    }
}

// ---------------------------------------------------------------------------------------------- the backup folder, the marker

/// The confirmed backup folder of `layout`, after the rules: there is one, it is not the data folder or inside it.
fn backups_of(layout: &LooseLayout) -> Result<PathBuf> {
    let Some(b) = &layout.backups else { return Err(Error::Refused("choose a backup folder outside the data folder".to_string())) };
    if let Some(why) = backup_folder_problem(layout, b) {
        return Err(Error::Refused(why));
    }
    Ok(b.clone())
}

fn marker_path(backups: &Path) -> PathBuf {
    backups.join(MARKER)
}

fn marker_text(op: &str, label: &str, journal: &Path, data: &Path) -> String {
    format!("format=2\nop={op}\nlabel={}\njournal={}\ndata={}\n", one_line(label), one_line(&journal.display().to_string()), one_line(&data.display().to_string()))
}

/// Claims the folder: creates the marker, failing when one is there.
fn claim_marker(backups: &Path, text: &str) -> Result<()> {
    let p = marker_path(backups);
    let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::Refused("another change to this folder is running or was interrupted; open it again so the toolkit can recover it".to_string()))
        }
        Err(e) => return Err(Error::io(&p, e)),
    };
    let r = f.write_all(text.as_bytes()).and_then(|_| f.sync_all());
    if let Err(e) = r {
        drop(f);
        let _ = std::fs::remove_file(&p);
        return Err(Error::io(&p, e));
    }
    Ok(())
}

fn drop_marker(backups: &Path) {
    let _ = std::fs::remove_file(marker_path(backups));
}

struct MarkerData {
    op: String,
    journal: PathBuf,
    data: PathBuf,
}

fn read_marker(backups: &Path) -> Result<Option<MarkerData>> {
    let p = marker_path(backups);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&p, e)),
    };
    let (mut op, mut journal, mut data) = (String::new(), None, None);
    for line in text.lines() {
        match line.split_once('=') {
            Some(("op", v)) => op = v.trim().to_string(),
            Some(("journal", v)) if !v.is_empty() => journal = Some(PathBuf::from(v)),
            Some(("data", v)) if !v.is_empty() => data = Some(PathBuf::from(v)),
            _ => {}
        }
    }
    match (op.as_str(), journal, data) {
        ("apply" | "rollback", Some(journal), Some(data)) => Ok(Some(MarkerData { op, journal, data })),
        _ => Err(Error::Refused(format!("{} is damaged; see docs/guides/undo-and-recovery.md", p.display()))),
    }
}

/// Whether a change or rollback of this folder was interrupted: the sentence for the person, or `None`.
pub fn unfinished(layout: &LooseLayout) -> Option<String> {
    let backups = layout.backups.as_ref()?;
    if marker_path(backups).exists() {
        return Some("an earlier change to this folder was interrupted; it will be put right before the next change (Health or shaiya loose recover does it now)".to_string());
    }
    read_journals(backups).iter().any(|j| j.state == State::Pending).then(|| "a change was prepared but did not finish; recovery marks it so".to_string())
}

/// Makes `<backups>/<UTC stamp>-<NNNN>-<slug>/`: NNNN counts up within one stamp (one second), so the folder names sort in the order the changes
/// were made whatever their labels are (the newest-only rule of the rollback relies on it).
fn create_journal_dir(backups: &Path, label: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(backups).map_err(|e| Error::io(backups, e))?;
    let stamp = shaiya_core::time::utc_stamp(SystemTime::now());
    let prefix = format!("{stamp}-");
    let mut seq = 0u32;
    for e in std::fs::read_dir(backups).map_err(|e| Error::io(backups, e))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_prefix(&prefix).and_then(|r| r.split('-').next()).and_then(|d| d.parse::<u32>().ok()) {
            seq = seq.max(n);
        }
    }
    for n in seq + 1.. {
        let dir = backups.join(format!("{prefix}{n:04}-{}", journal::slug(label)));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(&dir, e)),
        }
    }
    unreachable!()
}

// ---------------------------------------------------------------------------------------------- hashing and copying

fn hash_of(path: &Path) -> Result<String> {
    Ok(to_hex(&sha256_file(path, &Job::silent())?))
}

/// Copies `src` to the new file `dest` (parents made) and returns the SHA-256 of what was copied.
fn copy_hashed(src: &Path, dest: &Path, job: &Job) -> Result<String> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| Error::io(p, e))?;
    }
    let mut from = std::fs::File::open(src).map_err(|e| Error::io(src, e))?;
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(dest).map_err(|e| Error::io(dest, e))?;
    let mut h = Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        job.check()?;
        let n = from.read(&mut buf).map_err(|e| Error::io(src, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        to.write_all(&buf[..n]).map_err(|e| Error::io(dest, e))?;
        job.advance(n as u64);
    }
    to.sync_all().map_err(|e| Error::io(dest, e))?;
    Ok(to_hex(&h.finish()))
}

/// The path of `rel` below the data folder after every existing part was looked at again for a link.
fn checked_target(layout: &LooseLayout, rel: &str) -> Result<PathBuf> {
    let mut at = layout.data.clone();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        at.push(part);
        if is_link(&at) {
            return Err(Error::Refused(format!("{}: a link is in the way; the folder was changed since it was planned", at.display())));
        }
    }
    Ok(at)
}

/// Removes this toolkit's temp files of `target` (`.<name>.<pid>.shaiya-tmp`) that an interrupted write left beside it.
fn clean_temps_of(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name().map(|n| n.to_string_lossy().into_owned())) else { return };
    let prefix = format!(".{name}.");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(&prefix) && n.ends_with(".shaiya-tmp") && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// ---------------------------------------------------------------------------------------------- apply

/// Applies a checked plan: journal and marker first, then the writes, then the removes. Returns the report; any failure or cancel puts the
/// folder back as it was before it returns the error (or, when even that fails, leaves the marker for [`recover`] and says so).
pub fn apply(layout: &LooseLayout, plan: LoosePlan, opts: &LooseOptions, check: FormatCheck<'_>, job: &Job) -> Result<LooseReport> {
    let backups = backups_of(layout)?;
    ensure_not_busy(layout, opts)?;
    // a leftover of an earlier change is put right first; the folder is only made once everything else is known to be fine
    let recovered = if backups.is_dir() { recover_unchecked(layout, &backups)? } else { Vec::new() };
    job.check()?;
    // everything is as planned: the same names, the same files, no link
    revalidate(layout, &plan)?;
    std::fs::create_dir_all(&backups).map_err(|e| Error::io(&backups, e))?;
    // the folder may be a junction into the data folder that only exists now
    if let Some(why) = backup_folder_problem(layout, &backups) {
        return Err(Error::Refused(why));
    }
    let label = if opts.label.trim().is_empty() { "loose edit".to_string() } else { opts.label.clone() };
    let kind = if opts.patch { "loose-patch" } else { "loose-edit" };
    job.stage("saving the originals");
    job.total(Some(plan.puts.iter().filter(|p| p.existed).map(|p| p.stamp.len).sum::<u64>() + plan.removes.iter().map(|r| r.len).sum::<u64>() + plan.bytes_written()), Unit::Bytes);
    let jdir = create_journal_dir(&backups, &label)?;
    let made = (|| -> Result<LooseJournal> {
        let mut j = LooseJournal {
            dir: jdir.clone(),
            id: jdir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            kind: kind.to_string(),
            label: label.clone(),
            created_utc: utc_iso(SystemTime::now()),
            tool: TOOL.to_string(),
            data: layout.data.display().to_string(),
            state: State::Pending,
            records: Vec::new(),
            dirs: plan.new_dirs.clone(),
        };
        for (i, p) in plan.puts.iter().enumerate() {
            let after = to_hex(&sha256(&plan.bytes[i]));
            let (backup, before) = if p.existed {
                let rel = format!("files/{}", p.rel);
                let sum = copy_hashed(&checked_target(layout, &p.rel)?, &join_rel(&jdir, &rel), job)?;
                (Some(rel), Some(sum))
            } else {
                (None, None)
            };
            j.records.push(LooseRec { rel: p.rel.clone(), existed: p.existed, sha256_before: before, sha256_after: Some(after), backup });
        }
        for r in &plan.removes {
            let rel = format!("files/{}", r.rel);
            let sum = copy_hashed(&checked_target(layout, &r.rel)?, &join_rel(&jdir, &rel), job)?;
            j.records.push(LooseRec { rel: r.rel.clone(), existed: true, sha256_before: Some(sum), sha256_after: None, backup: Some(rel) });
        }
        j.write()?;
        claim_marker(&backups, &marker_text("apply", &label, &jdir, &layout.data))?;
        Ok(j)
    })();
    let mut journal = match made {
        Ok(j) => j,
        Err(e) => {
            // nothing in the data folder was touched yet
            let _ = std::fs::remove_dir_all(&jdir);
            return Err(e);
        }
    };
    job.stage("writing");
    let executed = execute(layout, &plan, &journal, opts, check, job);
    match executed {
        Ok(()) => {
            journal.state = State::Applied;
            journal.write()?;
            drop_marker(&backups);
            Ok(LooseReport {
                journal: jdir,
                added: plan.puts.iter().filter(|p| !p.existed).map(|p| p.rel.clone()).collect(),
                replaced: plan.puts.iter().filter(|p| p.existed).map(|p| p.rel.clone()).collect(),
                removed: plan.removes.iter().map(|r| r.rel.clone()).collect(),
                recovered,
            })
        }
        Err(Failure::Crash(e)) => Err(e),
        Err(Failure::Error(e)) => match restore(layout, &journal, true) {
            Ok(_) => {
                journal.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
                let _ = journal.write();
                drop_marker(&backups);
                Err(e)
            }
            Err(undo) => Err(Error::Refused(format!("{e}; and the files could not be put back ({undo}); open the folder again so the toolkit can recover it"))),
        },
    }
}

enum Failure {
    /// A test hook stopped the change as a crash would.
    Crash(Error),
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        Failure::Error(e)
    }
}

/// Everything in the plan is as it was when the plan was made.
fn revalidate(layout: &LooseLayout, plan: &LoosePlan) -> Result<()> {
    let mut problems = Vec::new();
    let now = |rel: &str, want: &Stamp, existed: bool, problems: &mut Vec<String>| match resolve(layout, rel, problems) {
        None => {}
        Some(r) if r.rel != rel || r.existed != existed || (existed && r.stamp != *want) => problems.push(format!("{rel}: changed since the change was planned (plan it again)")),
        Some(_) => {}
    };
    for p in &plan.puts {
        now(&p.rel, &p.stamp, p.existed, &mut problems);
    }
    for r in &plan.removes {
        now(&r.rel, &r.stamp, true, &mut problems);
    }
    if problems.is_empty() { Ok(()) } else { Err(refusal(problems)) }
}

fn execute(layout: &LooseLayout, plan: &LoosePlan, j: &LooseJournal, opts: &LooseOptions, check: FormatCheck<'_>, job: &Job) -> std::result::Result<(), Failure> {
    for (i, p) in plan.puts.iter().enumerate() {
        job.check()?;
        if opts.crash_after_puts == Some(i) {
            return Err(Failure::Crash(Error::Refused("the change was stopped as a crash would (test hook)".to_string())));
        }
        let bytes = &plan.bytes[i];
        // the folders, one at a time, each looked at for a link once it is there
        let mut at = layout.data.clone();
        let parts: Vec<&str> = p.rel.split('/').collect();
        for (n, part) in parts.iter().enumerate() {
            at.push(part);
            if n + 1 == parts.len() {
                break;
            }
            if !at.exists() {
                std::fs::create_dir(&at).map_err(|e| Error::io(&at, e))?;
            }
            if is_link(&at) {
                return Err(Error::Refused(format!("{}: a link is in the way", at.display())).into());
            }
        }
        if is_link(&at) {
            return Err(Error::Refused(format!("{}: a link is in the way", at.display())).into());
        }
        if Stamp::of(&at) != p.stamp {
            return Err(Error::Refused(format!("{}: changed while the change was running", p.rel)).into());
        }
        let rel = p.rel.clone();
        write_atomic_checked(&at, bytes, &|tmp: &Path| {
            let back = read_capped(tmp, MAX_PUT_BYTES)?;
            if back != *bytes {
                return Err(Error::Refused(format!("{rel}: the file written does not read back the same")));
            }
            check(&rel, &back)
        })?;
        job.advance(bytes.len() as u64);
    }
    for r in &plan.removes {
        job.check()?;
        let at = checked_target(layout, &r.rel)?;
        let rec = j.records.iter().find(|x| x.rel == r.rel && x.sha256_after.is_none());
        let want = rec.and_then(|x| x.sha256_before.as_deref()).unwrap_or("");
        if hash_of(&at)? != want {
            return Err(Error::Refused(format!("{}: changed while the change was running", r.rel)).into());
        }
        std::fs::remove_file(&at).map_err(|e| Error::io(&at, e))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------- restoring

/// Puts every file of the journal back as it was before the change: originals back, created files deleted, created folders removed when
/// empty. The journal's copies are verified first (nothing is changed when one is damaged). A file that is neither what the change wrote
/// nor the original is, with `save_changed`, copied to `<journal>/saved/<rel>` first. Returns `(restored, deleted, saved)`.
fn restore(layout: &LooseLayout, j: &LooseJournal, save_changed: bool) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    if let Some(bad) = j.unsafe_path() {
        return Err(Error::Refused(format!("the journal {} names an unsafe path ({bad:?}); no file was touched", j.id)));
    }
    for r in &j.records {
        if let (Some(b), Some(want)) = (&r.backup, &r.sha256_before) {
            match hash_of(&join_rel(&j.dir, b)) {
                Ok(h) if &h == want => {}
                _ => return Err(Error::Refused(format!("the journal's copy of {} is missing or damaged; no file was touched", r.rel))),
            }
        }
    }
    let (mut restored, mut deleted, mut saved) = (Vec::new(), Vec::new(), Vec::new());
    let silent = Job::silent();
    for r in j.records.iter().rev() {
        let target = checked_target(layout, &r.rel)?;
        clean_temps_of(&target);
        let now = if target.is_file() { Some(hash_of(&target)?) } else { None };
        let is_original = now.is_some() && now == r.sha256_before;
        let is_ours = now.is_some() && now == r.sha256_after;
        if now.is_some() && !is_original && !is_ours && save_changed {
            let dest = unique_path(join_rel(&j.dir.join("saved"), &r.rel));
            copy_hashed(&target, &dest, &silent)?;
            saved.push(r.rel.clone());
        }
        if r.existed {
            if is_original {
                continue;
            }
            let Some(b) = &r.backup else { continue };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            let mut f = std::fs::File::open(join_rel(&j.dir, b)).map_err(|e| Error::io(join_rel(&j.dir, b), e))?;
            write_atomic_from_with(&target, &mut f, Sync::Yes, &silent)?;
            restored.push(r.rel.clone());
        } else if now.is_some() {
            std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
            deleted.push(r.rel.clone());
        }
    }
    // the folders the change made, deepest first, only when empty
    for d in j.dirs.iter().rev() {
        if let Ok(p) = checked_target(layout, d) {
            // a folder whose last file was just deleted can stay "not empty" for a moment on some disks (exFAT): try a few times
            for _ in 0..5 {
                match std::fs::remove_dir(&p) {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                    Err(_) if std::fs::read_dir(&p).is_ok_and(|mut r| r.next().is_some()) => break,
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        }
    }
    restored.reverse();
    deleted.reverse();
    saved.reverse();
    Ok((restored, deleted, saved))
}

fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    for n in 1u32.. {
        let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(format!(".{n}"));
        let c = p.with_file_name(name);
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

// ---------------------------------------------------------------------------------------------- recovery

fn read_journals(backups: &Path) -> Vec<LooseJournal> {
    let Ok(rd) = std::fs::read_dir(backups) else { return Vec::new() };
    let mut v: Vec<LooseJournal> = rd.flatten().map(|e| e.path()).filter(|p| p.join(JOURNAL_FILE).is_file()).filter_map(|p| LooseJournal::read(&p).ok()).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

fn inside_backups(backups: &Path, journal: &Path) -> bool {
    journal.parent().is_some_and(|p| same_file(p, backups))
}

/// Finishes what an interrupted change or rollback left (see the module text). Returns what it did, one sentence each (empty when there
/// was nothing). Refuses (and changes nothing) when the marker belongs to another folder or is damaged.
pub fn recover(layout: &LooseLayout, opts: &LooseOptions) -> Result<Vec<String>> {
    let backups = backups_of(layout)?;
    if !marker_path(&backups).exists() && !read_journals(&backups).iter().any(|j| j.state == State::Pending) {
        return Ok(Vec::new());
    }
    ensure_not_busy(layout, opts)?;
    recover_unchecked(layout, &backups)
}

fn recover_unchecked(layout: &LooseLayout, backups: &Path) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let mut handled: Option<PathBuf> = None;
    match read_marker(backups) {
        Ok(Some(m)) => {
            if !same_file(&m.data, &layout.data) {
                return Err(Error::Refused(format!("the pending marker in {} belongs to another data folder ({})", backups.display(), m.data.display())));
            }
            if !inside_backups(backups, &m.journal) {
                return Err(Error::Refused(format!("the pending marker in {} names a journal outside the backup folder", backups.display())));
            }
            let mut j = LooseJournal::read(&m.journal).map_err(|e| Error::Refused(format!("the journal of the interrupted change is damaged ({e}); see docs/guides/undo-and-recovery.md")))?;
            match (m.op.as_str(), &j.state) {
                ("apply", State::Pending) => {
                    restore(layout, &j, true)?;
                    j.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
                    j.write()?;
                    notes.push(format!("undid the unfinished change {} ({}); the folder is as it was", j.label, j.id));
                }
                ("apply", State::Applied) => notes.push(format!("the change {} had finished; the marker was removed", j.label)),
                ("rollback", State::Applied) => {
                    restore(layout, &j, true)?;
                    j.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
                    j.write()?;
                    notes.push(format!("finished the interrupted rollback of {} ({})", j.label, j.id));
                }
                _ => notes.push(format!("the marker of {} was left over; it was removed", j.label)),
            }
            handled = Some(m.journal);
            drop_marker(backups);
        }
        Ok(None) => {}
        Err(e) => {
            // a marker that was cut short: when exactly one change is pending it is the one to undo
            let pending: Vec<LooseJournal> = read_journals(backups).into_iter().filter(|j| j.state == State::Pending).collect();
            if pending.len() != 1 {
                return Err(e);
            }
            let mut j = pending.into_iter().next().unwrap_or_else(|| unreachable!());
            restore(layout, &j, true)?;
            j.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
            j.write()?;
            notes.push(format!("undid the unfinished change {} ({})", j.label, j.id));
            handled = Some(j.dir.clone());
            drop_marker(backups);
        }
    }
    // prepared but never started: no marker means no file of the data folder was touched
    for mut j in read_journals(backups) {
        if j.state == State::Pending && handled.as_ref().is_none_or(|h| !same_file(h, &j.dir)) {
            j.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
            j.write()?;
            notes.push(format!("a change that was prepared but never started ({}) was marked as not made; the data folder was not touched", j.id));
        }
    }
    Ok(notes)
}

// ---------------------------------------------------------------------------------------------- history and rollback

fn status(layout: &LooseLayout, j: &LooseJournal, all: &[LooseJournal], force: bool) -> journal::Rollback {
    let blocked = |s: String| journal::Rollback::Blocked(s);
    match &j.state {
        State::RolledBack { .. } => return blocked("already rolled back".to_string()),
        State::Pending => return blocked("this change did not finish; open the folder again so the toolkit can recover it".to_string()),
        State::Applied => {}
    }
    if let Some(bad) = j.unsafe_path() {
        return blocked(format!("the journal names an unsafe path ({bad:?}); nothing was changed"));
    }
    if let Some(newer) = all.iter().filter(|o| o.id > j.id && o.state == State::Applied).map(|o| o.id.clone()).next_back() {
        return blocked(format!("roll back {newer} first"));
    }
    if !same_file(Path::new(&j.data), &layout.data) {
        return blocked(format!("this change was made in another data folder ({})", j.data));
    }
    for r in &j.records {
        if let (Some(b), Some(want)) = (&r.backup, &r.sha256_before) {
            match hash_of(&join_rel(&j.dir, b)) {
                Ok(h) if &h == want => {}
                _ => return blocked(format!("the journal's copy of {} is missing or damaged", r.rel)),
            }
        }
    }
    if !force {
        let mut changed: Vec<&str> = Vec::new();
        for r in &j.records {
            let Ok(p) = checked_target(layout, &r.rel) else {
                changed.push(&r.rel);
                continue;
            };
            let same = match &r.sha256_after {
                Some(want) => hash_of(&p).is_ok_and(|h| &h == want),
                None => !p.exists(),
            };
            if !same {
                changed.push(&r.rel);
            }
        }
        if !changed.is_empty() {
            let shown: Vec<&str> = changed.iter().take(5).copied().collect();
            let more = if changed.len() > 5 { format!(" and {} more", changed.len() - 5) } else { String::new() };
            return blocked(format!("{}{more} changed or removed since (rollback with force restores them anyway; each changed file is saved first)", shown.join(", ")));
        }
    }
    journal::Rollback::Possible
}

/// The journals of this client's backup folder, oldest first, each with whether it can be rolled back now.
pub fn list(layout: &LooseLayout) -> Result<Vec<(LooseJournal, journal::Rollback)>> {
    let backups = backups_of(layout)?;
    let all = read_journals(&backups);
    Ok(all.iter().map(|j| (j.clone(), status(layout, j, &all, false))).collect())
}

/// Undoes one journal (the newest applied one): needs the same confirmed backup folder and a stopped game as [`apply`]. Without `force`
/// it is refused when a file is not as the change left it.
pub fn rollback(layout: &LooseLayout, journal_dir: &Path, force: bool, opts: &LooseOptions, job: &Job) -> Result<RollbackReport> {
    let backups = backups_of(layout)?;
    ensure_not_busy(layout, opts)?;
    recover_unchecked(layout, &backups)?;
    if !inside_backups(&backups, journal_dir) {
        return Err(Error::Refused(format!("{} is not a journal of this client's backup folder", journal_dir.display())));
    }
    let mut j = LooseJournal::read(journal_dir)?;
    let all = read_journals(&backups);
    if let journal::Rollback::Blocked(why) = status(layout, &j, &all, force) {
        return Err(Error::Refused(format!("cannot roll back {}: {why}", j.id)));
    }
    job.stage("restoring");
    job.check()?;
    claim_marker(&backups, &marker_text("rollback", &j.label, &j.dir, &layout.data))?;
    let r = restore(layout, &j, force);
    match r {
        Ok((restored, deleted, saved)) => {
            j.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
            j.write()?;
            drop_marker(&backups);
            Ok(RollbackReport { journal: j.dir.clone(), restored, deleted, saved })
        }
        // the marker stays: the files may be half way back, and recovery finishes the rollback from the journal
        Err(e) => Err(e),
    }
}

/// Deletes the oldest journal folders beyond `keep` (never one that did not finish); returns the deleted ids.
pub fn prune(layout: &LooseLayout, keep: usize) -> Result<Vec<String>> {
    let backups = backups_of(layout)?;
    let all: Vec<LooseJournal> = read_journals(&backups).into_iter().filter(|j| j.state != State::Pending).collect();
    let mut gone = Vec::new();
    if all.len() > keep {
        for j in &all[..all.len() - keep] {
            if !inside_backups(&backups, &j.dir) {
                continue;
            }
            std::fs::remove_dir_all(&j.dir).map_err(|e| Error::io(&j.dir, e))?;
            gone.push(j.id.clone());
        }
    }
    Ok(gone)
}

/// Whether `rel` names a file that is in the data folder now (letter case ignored, no link on the way); for a caller that wants to leave
/// out the removes of files that are not there (a patch's `remove.txt`).
pub fn file_exists(layout: &LooseLayout, rel: &str) -> bool {
    let mut problems = Vec::new();
    name_problem(rel).is_none() && resolve(layout, rel, &mut problems).is_some_and(|r| r.existed) && problems.is_empty()
}

/// Whether the data folder holds a top-level folder named like the first part of `rel` (letter case ignored).
pub fn top_folder_exists(layout: &LooseLayout, rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    let first = lower(norm.split('/').next().unwrap_or(""));
    std::fs::read_dir(&layout.data).is_ok_and(|rd| rd.flatten().any(|e| lower(&e.file_name().to_string_lossy()) == first && e.file_type().is_ok_and(|t| t.is_dir())))
}
