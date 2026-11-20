//! A read-only look at a client folder: the index check, dead space in `data.saf`, the journals, and the leftovers a clean-up may move.
//!
//! [`report`] opens files for reading only; it never writes, renames or creates anything (a test hashes every file before and after).
//! The leftovers are offered by [`crate::cleanup`], which moves them into `shaiya-toolkit-backups/cleanup-<UTC>/`; nothing is deleted.

use crate::check::check_index;
use crate::cleanup::{self, FolderInfo};
use crate::client::{read_version_ini, ClientFiles};
use crate::journal::{Marker, DEFAULT_DIR};
use crate::{compact, find_in_dir, Index};
use shaiya_core::{Error, Result};
use std::path::Path;

/// The editor's install record; while it exists the editor's own backups are in use by its Uninstall (same names as `shaiya-svmap-tools`'
/// `share` module, which this crate cannot depend on).
pub const EDITOR_RECORD: &str = "svmap-editor-install.json";
const EDITOR_FIRST: &str = "data.sah.svmap-editor";
const TEMP_SUFFIX: &str = ".shaiya-tmp";
/// Folders below the client folder that are searched for stray temporary files.
const SCAN_DEPTH: usize = 3;

/// What kind of leftover an item is. The names are what `shaiya clients clean --move` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Kind {
    /// `data.sah.bak`: the only earlier index of a change made without a journal.
    SahBak,
    /// `.<name>.<pid>.shaiya-tmp` that no pending marker names.
    StrayTemp,
    /// `data.<UTC stamp>.sah` + `.saf`: the pair a "Use another archive pair" put aside.
    SwapPair,
    /// `data.sah.svmap-editor-<stamp>` (never offered while the editor's install record exists).
    EditorBackup,
    /// `data.sah.svmap-editor`: the index from before the editor's first install (never offered while the record exists).
    EditorFirstCopy,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::SahBak, Kind::StrayTemp, Kind::SwapPair, Kind::EditorBackup, Kind::EditorFirstCopy];

    pub fn name(self) -> &'static str {
        match self {
            Kind::SahBak => "sah-bak",
            Kind::StrayTemp => "stray-temp",
            Kind::SwapPair => "swap-pair",
            Kind::EditorBackup => "editor-backup",
            Kind::EditorFirstCopy => "editor-first-copy",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s.trim()))
    }

    /// What the item is and what moving it costs, in a sentence.
    pub fn explain(self) -> &'static str {
        match self {
            Kind::SahBak => "The only earlier index of a change made without a journal (--no-journal or compacting). Keep it if you may want to go back by hand.",
            Kind::StrayTemp => "A temporary file from a write that did not finish. No pending change names it, so nothing waits for it.",
            Kind::SwapPair => "The archive pair that was in place before \"Use another archive pair\". Keep it if you may want to swap back.",
            Kind::EditorBackup => "A copy of the index made by the map editor's Install. The editor's install record is not in this folder.",
            Kind::EditorFirstCopy => "The index from before the map editor's first Install. The editor's install record is not in this folder. Keep it if you may want the original index back.",
        }
    }
}

/// One thing a clean-up may move: a file, or the two files of a swap pair.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Leftover {
    pub kind: Kind,
    /// `<kind>:<first file>`; what [`cleanup::clean`] takes.
    pub id: String,
    /// Paths relative to the client folder, `/` separated.
    pub files: Vec<String>,
    pub bytes: u64,
    /// Whether the Health view ticks it at first. Nothing an undo, a swap back or an uninstall may need is pre-selected.
    pub pre_selected: bool,
    pub note: String,
}

/// A file that looks like a leftover but is not offered, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct NotOffered {
    pub file: String,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct IndexHealth {
    pub sah_version: i32,
    pub files: usize,
    pub saf_present: bool,
    /// Problems of the index check (`shaiya archive check` without `--deep`); warnings are counted apart and are not findings.
    pub problems: usize,
    pub warnings: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DeadSpace {
    pub saf_bytes: u64,
    pub live_bytes: u64,
    pub dead_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct JournalSummary {
    pub count: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PendingInfo {
    pub op: String,
    pub label: String,
    /// False when `data.toolkit-pending` could not be read (then no temporary file is offered).
    pub readable: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Health {
    pub index: IndexHealth,
    pub dead_space: Option<DeadSpace>,
    pub journals: JournalSummary,
    pub pending_marker: Option<PendingInfo>,
    /// `StartUpdate=UPDATE_START` in `Version.ini`: an original-updater run that did not finish. Explained, never edited.
    pub start_update: Option<String>,
    pub leftovers: Vec<Leftover>,
    pub not_offered: Vec<NotOffered>,
    pub cleanups: Vec<FolderInfo>,
}

impl Health {
    /// What `shaiya clients health` counts for its exit code: index problems, a missing `data.saf`, a pending marker, an unfinished
    /// original-updater run and every offered leftover. Dead space, journals and clean-up folders are information.
    pub fn findings(&self) -> usize {
        self.index.problems + usize::from(!self.index.saf_present) + usize::from(self.pending_marker.is_some()) + usize::from(self.start_update.is_some()) + self.leftovers.len()
    }

    pub fn healthy(&self) -> bool {
        self.findings() == 0
    }
}

fn norm(rel: &str) -> String {
    rel.replace('\\', "/").to_ascii_lowercase()
}

fn len_of(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn dir_bytes(dir: &Path, depth: usize) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            Ok(t) if t.is_dir() && depth < 8 => dir_bytes(&e.path(), depth + 1),
            _ => 0,
        })
        .sum()
}

/// Temporary files (`.<name>.<pid>.shaiya-tmp`) below `dir`, `(relative path, size)`.
fn find_temps(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(t) = e.file_type() else { continue };
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if t.is_file() {
            if name.starts_with('.') && name.ends_with(TEMP_SUFFIX) {
                out.push((child, e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        } else if t.is_dir() && depth < SCAN_DEPTH && !(rel.is_empty() && name.eq_ignore_ascii_case(DEFAULT_DIR)) {
            find_temps(&e.path(), &child, depth + 1, out);
        }
    }
}

/// `YYYYMMDDTHHMMSSZ` with an optional `-N`: the middle of `data.<stamp>.sah`.
fn is_swap_stamp(s: &str) -> bool {
    let (stamp, tail) = match s.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let b = stamp.as_bytes();
    let shaped = b.len() == 16 && b[8] == b'T' && b[15] == b'Z' && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit);
    shaped && tail.is_none_or(|t| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()))
}

fn journals_of(dir: &Path) -> JournalSummary {
    let root = dir.join(DEFAULT_DIR);
    let Ok(rd) = std::fs::read_dir(&root) else { return JournalSummary::default() };
    let mut s = JournalSummary::default();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && p.join("journal.txt").is_file() {
            s.count += 1;
            s.bytes += dir_bytes(&p, 0);
        }
    }
    s
}

/// Reads the folder and says what is there. `Error` only when the index cannot be read at all (no `data.sah`, or it does not parse).
pub fn report(dir: &Path) -> Result<Health> {
    let sah = find_in_dir(dir, "data.sah").ok_or_else(|| Error::io(dir.join("data.sah"), std::io::Error::new(std::io::ErrorKind::NotFound, "no data.sah in this folder")))?;
    let index = Index::read(&sah)?;
    let saf = find_in_dir(dir, "data.saf");
    let check = check_index(&index, saf.as_ref().map(|p| len_of(p)));
    let index_health = IndexHealth { sah_version: index.version, files: index.file_count(), saf_present: saf.is_some(), problems: check.problems(), warnings: check.warnings() };
    let dead_space = saf.as_ref().and_then(|_| compact::usage(dir).ok()).map(|(size, live)| DeadSpace { saf_bytes: size, live_bytes: live, dead_bytes: size.saturating_sub(live) });

    let files = ClientFiles { dir: dir.to_path_buf(), sah: sah.clone(), saf: saf.clone().unwrap_or_else(|| dir.join("data.saf")), game_exe: None, version_ini: find_in_dir(dir, "version.ini") };
    let start_update = read_version_ini(&files).and_then(|v| v.start_update).filter(|s| s.eq_ignore_ascii_case("UPDATE_START"));

    // the pending marker decides which temporary files may be offered
    let (pending_marker, marker_names) = match std::fs::read_to_string(Marker::path(dir)) {
        Ok(text) => match Marker::parse(&text) {
            Ok(m) => {
                let names: Vec<String> = m.staged.iter().flat_map(|(s, t)| [norm(s), norm(t)]).collect();
                (Some(PendingInfo { op: m.op, label: m.label, readable: true }), Some(names))
            }
            Err(_) => (Some(PendingInfo { op: String::new(), label: String::new(), readable: false }), None),
        },
        Err(_) => (None, Some(Vec::new())),
    };

    let mut leftovers = Vec::new();
    let mut not_offered = Vec::new();
    let record = find_in_dir(dir, EDITOR_RECORD).is_some();
    let mut swaps: Vec<(String, Vec<(String, u64)>)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut names: Vec<(String, u64)> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file())).map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0))).collect();
        names.sort();
        for (name, bytes) in names {
            let lower = name.to_ascii_lowercase();
            let one = |kind: Kind, pre: bool| Leftover { kind, id: format!("{}:{name}", kind.name()), files: vec![name.clone()], bytes, pre_selected: pre, note: kind.explain().to_string() };
            if lower == "data.sah.bak" {
                leftovers.push(one(Kind::SahBak, false));
            } else if lower == EDITOR_FIRST || lower.starts_with(&format!("{EDITOR_FIRST}-")) {
                if record {
                    not_offered.push(NotOffered { file: name.clone(), why: format!("{EDITOR_RECORD} exists, so the editor's Uninstall still uses its backups") });
                } else if lower == EDITOR_FIRST {
                    leftovers.push(one(Kind::EditorFirstCopy, false));
                } else {
                    leftovers.push(one(Kind::EditorBackup, true));
                }
            } else if let Some(mid) = lower.strip_prefix("data.").and_then(|r| r.strip_suffix(".sah").or_else(|| r.strip_suffix(".saf"))) {
                if is_swap_stamp(&mid.to_ascii_uppercase()) {
                    match swaps.iter_mut().find(|(m, _)| *m == mid) {
                        Some((_, v)) => v.push((name.clone(), bytes)),
                        None => swaps.push((mid.to_string(), vec![(name.clone(), bytes)])),
                    }
                }
            }
        }
    }
    for (_, mut files) in swaps {
        files.sort_by_key(|f| f.0.to_ascii_lowercase().ends_with(".saf"));
        let first = files[0].0.clone();
        leftovers.push(Leftover { kind: Kind::SwapPair, id: format!("swap-pair:{first}"), bytes: files.iter().map(|f| f.1).sum(), files: files.into_iter().map(|f| f.0).collect(), pre_selected: false, note: Kind::SwapPair.explain().to_string() });
    }

    let mut temps = Vec::new();
    find_temps(dir, "", 0, &mut temps);
    for (rel, bytes) in temps {
        match &marker_names {
            Some(names) if !names.contains(&norm(&rel)) => {
                leftovers.push(Leftover { kind: Kind::StrayTemp, id: format!("stray-temp:{rel}"), files: vec![rel], bytes, pre_selected: true, note: Kind::StrayTemp.explain().to_string() })
            }
            Some(_) => not_offered.push(NotOffered { file: rel, why: "the pending change names it; opening the client finishes or removes that change".to_string() }),
            None => not_offered.push(NotOffered { file: rel, why: "data.toolkit-pending cannot be read, so it is not known which files it names".to_string() }),
        }
    }
    leftovers.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));

    Ok(Health { index: index_health, dead_space, journals: journals_of(dir), pending_marker, start_update, leftovers, not_offered, cleanups: cleanup::list_folders(dir) })
}
