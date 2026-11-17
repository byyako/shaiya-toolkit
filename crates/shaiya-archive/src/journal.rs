//! Journals (the backup of one change to a client folder), the pending marker, rollback and recovery. Both are small text files.
//!
//! A journal is a folder `shaiya-toolkit-backups/<UTC stamp>-<label>/` holding `journal.txt`, a copy of the old `data.sah` and backups of
//! the loose files the change overwrote. `data.toolkit-pending` in the client folder exists only while a change (or a rollback) is
//! running or was interrupted.

use crate::client::{self, ClientFiles};
use shaiya_core::fs::{write_atomic, write_atomic_from_with, Sync};
use shaiya_core::hash::{sha256_file, to_hex};
use shaiya_core::time::{utc_iso, utc_stamp};
use shaiya_core::{Error, Job, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DEFAULT_DIR: &str = "shaiya-toolkit-backups";
pub(crate) const MARKER: &str = "data.toolkit-pending";
const JOURNAL_FILE: &str = "journal.txt";
const TOOL: &str = concat!("shaiya ", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalMode {
    None,
    /// A new folder per change under this root.
    In(PathBuf),
}

/// `Pending`: written before the first append (5.2 step 2); only recovery or a finished apply leaves this state.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum State {
    Pending,
    Applied,
    RolledBack { at_utc: String },
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Applied => "applied",
            State::RolledBack { .. } => "rolled-back",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LooseRecord {
    pub rel: String,
    pub existed: bool,
    pub backup: Option<String>,
    pub sha256_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Journal {
    pub dir: PathBuf,
    pub id: String,
    /// `patch` or `archive-edit`.
    pub kind: String,
    pub label: String,
    pub created_utc: String,
    pub tool: String,
    pub sah_name: String,
    pub saf_name: String,
    pub saf_len_before: u64,
    pub saf_len_after: u64,
    pub sah_sha256_before: String,
    pub sah_sha256_after: String,
    pub loose: Vec<LooseRecord>,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Rollback {
    Possible,
    Blocked(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RollbackReport {
    pub journal: PathBuf,
    pub restored_index: bool,
    pub truncated_to: u64,
    pub restored_loose: Vec<String>,
    pub deleted_loose: Vec<String>,
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn bad_text(file: &str, what: impl Into<String>) -> Error {
    Error::format(file, None, what)
}

/// Shortest slug of a label for folder names: `ps0109.patch` -> `ps0109`.
pub(crate) fn slug(label: &str) -> String {
    let base = match label.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => label,
    };
    let mut s = String::new();
    for c in base.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').chars().take(40).collect::<String>();
    if s.is_empty() { "change".to_string() } else { s }
}

impl Journal {
    pub fn to_text(&self) -> String {
        let mut t = String::new();
        let mut kv = |k: &str, v: &str| {
            t.push_str(k);
            t.push('=');
            t.push_str(v);
            t.push('\n');
        };
        kv("format", "1");
        kv("id", &self.id);
        kv("kind", &self.kind);
        kv("label", &one_line(&self.label));
        kv("created_utc", &self.created_utc);
        kv("tool", &one_line(&self.tool));
        kv("sah_name", &self.sah_name);
        kv("saf_name", &self.saf_name);
        kv("saf_len_before", &self.saf_len_before.to_string());
        kv("saf_len_after", &self.saf_len_after.to_string());
        kv("sah_sha256_before", &self.sah_sha256_before);
        kv("sah_sha256_after", &self.sah_sha256_after);
        kv("state", self.state.name());
        kv("rolled_back_utc", if let State::RolledBack { at_utc } = &self.state { at_utc } else { "" });
        for l in &self.loose {
            let line = format!("{}\t{}\t{}\t{}", u8::from(l.existed), l.sha256_after.as_deref().unwrap_or("-"), l.backup.as_deref().unwrap_or("-"), one_line(&l.rel));
            kv("loose", &line);
        }
        t
    }

    pub fn parse(dir: &Path, text: &str) -> Result<Journal> {
        let file = dir.join(JOURNAL_FILE).display().to_string();
        let mut j = Journal {
            dir: dir.to_path_buf(),
            id: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            kind: String::new(),
            label: String::new(),
            created_utc: String::new(),
            tool: String::new(),
            sah_name: "data.sah".into(),
            saf_name: "data.saf".into(),
            saf_len_before: 0,
            saf_len_after: 0,
            sah_sha256_before: String::new(),
            sah_sha256_after: String::new(),
            loose: Vec::new(),
            state: State::Pending,
        };
        let (mut seen_state, mut seen_before, mut seen_after) = (false, false, false);
        let mut rolled_at = String::new();
        let mut state_name = String::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "id" if !v.is_empty() => j.id = v.to_string(),
                "kind" => j.kind = v.to_string(),
                "label" => j.label = v.to_string(),
                "created_utc" => j.created_utc = v.to_string(),
                "tool" => j.tool = v.to_string(),
                "sah_name" if !v.is_empty() => j.sah_name = v.to_string(),
                "saf_name" if !v.is_empty() => j.saf_name = v.to_string(),
                "saf_len_before" => j.saf_len_before = v.trim().parse().map_err(|_| bad_text(&file, "saf_len_before is not a number"))?,
                "saf_len_after" => j.saf_len_after = v.trim().parse().map_err(|_| bad_text(&file, "saf_len_after is not a number"))?,
                "sah_sha256_before" => {
                    j.sah_sha256_before = v.trim().to_string();
                    seen_before = true;
                }
                "sah_sha256_after" => {
                    j.sah_sha256_after = v.trim().to_string();
                    seen_after = true;
                }
                "state" => {
                    state_name = v.trim().to_string();
                    seen_state = true;
                }
                "rolled_back_utc" => rolled_at = v.to_string(),
                "loose" => {
                    let mut p = v.splitn(4, '\t');
                    let (existed, after, backup, rel) = (p.next(), p.next(), p.next(), p.next());
                    let (Some(existed), Some(after), Some(backup), Some(rel)) = (existed, after, backup, rel) else {
                        return Err(bad_text(&file, "a loose= line needs four tab-separated fields"));
                    };
                    j.loose.push(LooseRecord {
                        rel: rel.to_string(),
                        existed: existed == "1",
                        backup: (backup != "-").then(|| backup.to_string()),
                        sha256_after: (after != "-").then(|| after.to_string()),
                    });
                }
                _ => {}
            }
        }
        if !(seen_state && seen_before && seen_after) {
            return Err(bad_text(&file, "journal.txt lacks state or checksums"));
        }
        j.state = match state_name.as_str() {
            "pending" => State::Pending,
            "applied" => State::Applied,
            "rolled-back" => State::RolledBack { at_utc: rolled_at },
            other => return Err(bad_text(&file, format!("unknown state {other:?}"))),
        };
        for h in [&j.sah_sha256_before, &j.sah_sha256_after] {
            if h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad_text(&file, "a checksum is not 64 hex digits"));
            }
        }
        Ok(j)
    }

    pub fn read(dir: &Path) -> Result<Journal> {
        let p = dir.join(JOURNAL_FILE);
        let text = std::fs::read_to_string(&p).map_err(|e| Error::io(&p, e))?;
        Journal::parse(dir, &text)
    }

    pub fn write(&self) -> Result<()> {
        let p = self.dir.join(JOURNAL_FILE);
        write_atomic_from_with(&p, &mut self.to_text().as_bytes(), Sync::Yes, &Job::silent()).map(|_| ())
    }

    /// The journal's own copy of the old `data.sah`.
    pub fn sah_copy(&self) -> PathBuf {
        self.dir.join("data.sah")
    }
}

/// Creates `root/<stamp>-<slug>/` (a counter is added when the folder exists) and returns its path.
pub(crate) fn create_dir(root: &Path, label: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
    let base = format!("{}-{}", utc_stamp(SystemTime::now()), slug(label));
    for n in 1u32.. {
        let name = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let dir = root.join(&name);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(&dir, e)),
        }
    }
    unreachable!()
}

pub(crate) fn new_journal(dir: &Path, kind: &str, label: &str, files: &ClientFiles, sah_before: &str, sah_after: &str, saf_before: u64, saf_after: u64) -> Journal {
    Journal {
        dir: dir.to_path_buf(),
        id: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        kind: kind.to_string(),
        label: label.to_string(),
        created_utc: utc_iso(SystemTime::now()),
        tool: TOOL.to_string(),
        sah_name: files.sah.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.sah".into()),
        saf_name: files.saf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.saf".into()),
        saf_len_before: saf_before,
        saf_len_after: saf_after,
        sah_sha256_before: sah_before.to_string(),
        sah_sha256_after: sah_after.to_string(),
        loose: Vec::new(),
        state: State::Pending,
    }
}

// ---------------------------------------------------------------------------------------------- the pending marker

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    /// `apply` or `rollback`.
    pub op: String,
    pub label: String,
    pub saf_len_before: u64,
    pub sah_sha256_before: String,
    pub sah_sha256_after: String,
    /// The journal folder, or `None` (`-`).
    pub journal: Option<PathBuf>,
    /// `(staged file, its target)`, both relative to the client folder.
    pub staged: Vec<(String, String)>,
}

impl Marker {
    pub fn to_text(&self) -> String {
        let mut t = format!(
            "format=1\nop={}\nlabel={}\nsaf_len_before={}\nsah_sha256_before={}\nsah_sha256_after={}\njournal={}\n",
            self.op,
            one_line(&self.label),
            self.saf_len_before,
            self.sah_sha256_before,
            self.sah_sha256_after,
            self.journal.as_ref().map(|p| one_line(&p.display().to_string())).unwrap_or_else(|| "-".into())
        );
        for (s, target) in &self.staged {
            t.push_str(&format!("staged={}\t{}\n", one_line(s), one_line(target)));
        }
        t
    }

    pub fn parse(text: &str) -> Result<Marker> {
        let file = MARKER;
        let mut m = Marker { op: String::new(), label: String::new(), saf_len_before: 0, sah_sha256_before: String::new(), sah_sha256_after: String::new(), journal: None, staged: Vec::new() };
        let mut have = 0;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "op" => {
                    m.op = v.trim().to_string();
                    have |= 1;
                }
                "label" => m.label = v.to_string(),
                "saf_len_before" => {
                    m.saf_len_before = v.trim().parse().map_err(|_| bad_text(file, "saf_len_before is not a number"))?;
                    have |= 2;
                }
                "sah_sha256_before" => {
                    m.sah_sha256_before = v.trim().to_string();
                    have |= 4;
                }
                "sah_sha256_after" => {
                    m.sah_sha256_after = v.trim().to_string();
                    have |= 8;
                }
                "journal" => m.journal = (v != "-" && !v.is_empty()).then(|| PathBuf::from(v)),
                "staged" => {
                    let (s, t) = v.split_once('\t').unwrap_or((v, v));
                    m.staged.push((s.to_string(), t.to_string()));
                }
                _ => {}
            }
        }
        if have != 15 || !matches!(m.op.as_str(), "apply" | "rollback") {
            return Err(bad_text(file, "the pending marker is incomplete"));
        }
        Ok(m)
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(MARKER)
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        let p = Marker::path(dir);
        write_atomic_from_with(&p, &mut self.to_text().as_bytes(), Sync::Yes, &Job::silent()).map(|_| ())
    }

    pub fn remove(dir: &Path) {
        let _ = std::fs::remove_file(Marker::path(dir));
    }
}

fn safe_rel(rel: &str) -> bool {
    !rel.is_empty() && !rel.starts_with(['/', '\\']) && !rel.contains(':') && rel.split(['/', '\\']).all(|p| p != "..")
}

fn join_rel(dir: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|p| !p.is_empty()).fold(dir.to_path_buf(), |p, s| p.join(s))
}

fn hash_of(path: &Path) -> Result<String> {
    Ok(to_hex(&sha256_file(path, &Job::silent())?))
}

fn truncate_to(saf: &Path, len: u64) -> Result<()> {
    let f = std::fs::OpenOptions::new().write(true).open(saf).map_err(|e| Error::io(saf, e))?;
    let cur = f.metadata().map_err(|e| Error::io(saf, e))?.len();
    if cur > len {
        f.set_len(len).map_err(|e| Error::io(saf, e))?;
    }
    f.sync_all().map_err(|e| Error::io(saf, e))
}

fn is_journal_dir(p: &Path) -> bool {
    p.is_dir() && p.join(JOURNAL_FILE).is_file()
}

/// Commits staged loose files (rename over targets) and returns their `(rel, sha256)`.
pub(crate) fn commit_staged(dir: &Path, staged: &[(String, String)]) -> Result<Vec<(String, String)>> {
    let mut done = Vec::new();
    for (s, t) in staged {
        if !safe_rel(s) || !safe_rel(t) {
            return Err(Error::Refused(format!("the pending marker names an unsafe path ({s:?} -> {t:?})")));
        }
        let (sp, tp) = (join_rel(dir, s), join_rel(dir, t));
        if sp.exists() {
            if let Some(parent) = tp.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            std::fs::rename(&sp, &tp).map_err(|e| Error::io(&tp, e))?;
        }
        if tp.is_file() {
            done.push((t.clone(), hash_of(&tp)?));
        }
    }
    Ok(done)
}

/// The pending-marker part of `client::recover`.
pub(crate) fn recover_marker(dir: &Path) -> Result<Option<String>> {
    let mp = Marker::path(dir);
    let text = match std::fs::read_to_string(&mp) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&mp, e)),
    };
    let stuck = || Error::Refused("data.sah changed after an interrupted toolkit change; see data.toolkit-pending".to_string());
    let m = Marker::parse(&text).map_err(|_| Error::Refused("data.toolkit-pending is damaged; see docs/guides/undo-and-recovery.md".to_string()))?;
    let files = ClientFiles::locate(dir)?;
    let now = hash_of(&files.sah)?;
    match m.op.as_str() {
        "apply" => {
            if now == m.sah_sha256_before {
                truncate_to(&files.saf, m.saf_len_before)?;
                for (s, _) in &m.staged {
                    if safe_rel(s) {
                        let _ = std::fs::remove_file(join_rel(dir, s));
                    }
                }
                if let Some(j) = &m.journal {
                    if is_journal_dir(j) {
                        let _ = std::fs::remove_dir_all(j);
                    }
                }
                Marker::remove(dir);
                Ok(Some(format!("removed the unfinished change {}", m.label)))
            } else if now == m.sah_sha256_after {
                let done = commit_staged(dir, &m.staged)?;
                if let Some(jd) = m.journal.as_ref().filter(|j| is_journal_dir(j)) {
                    if let Ok(mut j) = Journal::read(jd) {
                        j.state = State::Applied;
                        for rec in &mut j.loose {
                            if let Some((_, h)) = done.iter().find(|(r, _)| r.eq_ignore_ascii_case(&rec.rel)) {
                                rec.sha256_after = Some(h.clone());
                            }
                        }
                        j.write()?;
                    }
                }
                Marker::remove(dir);
                Ok(Some(format!("finished the interrupted change {}; `shaiya patch history CLIENT` shows it and `shaiya patch rollback CLIENT` can undo it", m.label)))
            } else {
                Err(stuck())
            }
        }
        "rollback" => {
            if now == m.sah_sha256_before {
                Marker::remove(dir);
                Ok(Some(format!("an interrupted rollback of {} had not started; nothing was changed", m.label)))
            } else if now == m.sah_sha256_after {
                let jd = m.journal.clone().filter(|j| is_journal_dir(j)).ok_or_else(|| Error::Refused("the journal of an interrupted rollback is missing; see data.toolkit-pending".to_string()))?;
                let j = Journal::read(&jd)?;
                finish_rollback(dir, &files, &j)?;
                Marker::remove(dir);
                Ok(Some(format!("finished the interrupted rollback of {}", m.label)))
            } else {
                Err(stuck())
            }
        }
        _ => Err(stuck()),
    }
}

/// Everything after the index is back: truncate `data.saf`, restore or delete loose files, mark the journal rolled back.
fn finish_rollback(dir: &Path, files: &ClientFiles, j: &Journal) -> Result<RollbackReport> {
    truncate_to(&files.saf, j.saf_len_before)?;
    let mut rep = RollbackReport { journal: j.dir.clone(), restored_index: true, truncated_to: j.saf_len_before, ..RollbackReport::default() };
    for rec in &j.loose {
        if !safe_rel(&rec.rel) {
            continue;
        }
        let target = join_rel(dir, &rec.rel);
        match (&rec.backup, rec.existed) {
            (Some(b), true) => {
                let src = j.dir.join(b);
                let mut f = std::fs::File::open(&src).map_err(|e| Error::io(&src, e))?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                write_atomic_from_with(&target, &mut f, Sync::Yes, &Job::silent())?;
                rep.restored_loose.push(rec.rel.clone());
            }
            (_, false) => {
                if target.is_file() {
                    std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
                    rep.deleted_loose.push(rec.rel.clone());
                }
            }
            (None, true) => {}
        }
    }
    let mut j2 = j.clone();
    j2.state = State::RolledBack { at_utc: utc_iso(SystemTime::now()) };
    j2.write()?;
    Ok(rep)
}

// ---------------------------------------------------------------------------------------------- listing and rollback

fn read_all(root: &Path) -> Vec<Journal> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut v: Vec<Journal> = rd.flatten().map(|e| e.path()).filter(|p| is_journal_dir(p)).filter_map(|p| Journal::read(&p).ok()).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

fn status(files: &ClientFiles, j: &Journal, all: &[Journal], force: bool) -> Rollback {
    let blocked = |s: &str| Rollback::Blocked(s.to_string());
    match &j.state {
        State::RolledBack { .. } => return blocked("already rolled back"),
        State::Pending => return blocked("this change did not finish; open the client again so the toolkit can recover it"),
        State::Applied => {}
    }
    if let Some(newer) = all.iter().filter(|o| o.id > j.id && o.state == State::Applied).map(|o| o.id.clone()).next_back() {
        return Rollback::Blocked(format!("roll back {newer} first"));
    }
    match hash_of(&files.sah) {
        Ok(h) if h == j.sah_sha256_after => {}
        Ok(_) => return blocked("data.sah changed since: compacted, swapped, patched by the updater or another tool"),
        Err(e) => return Rollback::Blocked(e.to_string()),
    }
    match std::fs::metadata(&files.saf) {
        Ok(m) if m.len() == j.saf_len_after => {}
        Ok(_) => return blocked("data.saf changed length since this change"),
        Err(e) => return Rollback::Blocked(e.to_string()),
    }
    match hash_of(&j.sah_copy()) {
        Ok(h) if h == j.sah_sha256_before => {}
        _ => return blocked("the journal's copy of data.sah is missing or damaged"),
    }
    if !force {
        for rec in &j.loose {
            if let Some(want) = &rec.sha256_after {
                let p = join_rel(&files.dir, &rec.rel);
                match hash_of(&p) {
                    Ok(h) if &h == want => {}
                    _ => return Rollback::Blocked(format!("{} was changed or removed since (use --force to restore it anyway; the changed file is saved first)", rec.rel)),
                }
            }
        }
    }
    Rollback::Possible
}

/// Journals under `root` for this client, oldest first, each with whether it can be rolled back now.
pub fn list(client: &Path, root: &Path) -> Result<Vec<(Journal, Rollback)>> {
    let files = ClientFiles::locate(client)?;
    let all = read_all(root);
    Ok(all.iter().map(|j| (j.clone(), status(&files, j, &all, false))).collect())
}

/// Undo one journal. `force` restores loose files even when they changed since (each changed file is first saved with `make_backup`).
pub fn rollback(client: &Path, journal_dir: &Path, force: bool, job: &Job) -> Result<RollbackReport> {
    client::recover(client)?;
    let files = ClientFiles::locate(client)?;
    client::ensure_not_in_use(&files)?;
    let j = Journal::read(journal_dir)?;
    let root = journal_dir.parent().unwrap_or(Path::new("."));
    let all = read_all(root);
    if let Rollback::Blocked(why) = status(&files, &j, &all, force) {
        return Err(Error::Refused(format!("cannot roll back {}: {why}", j.id)));
    }
    job.stage("restoring");
    job.check()?;
    if force {
        for rec in &j.loose {
            if let Some(want) = &rec.sha256_after {
                let p = join_rel(&files.dir, &rec.rel);
                if p.is_file() && hash_of(&p).ok().as_ref() != Some(want) {
                    shaiya_core::fs::make_backup(&p)?;
                }
            }
        }
    }
    let m = Marker {
        op: "rollback".into(),
        label: j.label.clone(),
        saf_len_before: j.saf_len_before,
        sah_sha256_before: j.sah_sha256_after.clone(),
        sah_sha256_after: j.sah_sha256_before.clone(),
        journal: Some(std::path::absolute(&j.dir).unwrap_or_else(|_| j.dir.clone())),
        staged: Vec::new(),
    };
    m.write(&files.dir)?;
    let old = std::fs::read(j.sah_copy()).map_err(|e| Error::io(j.sah_copy(), e))?;
    write_atomic(&files.sah, &old)?;
    let rep = finish_rollback(&files.dir, &files, &j)?;
    Marker::remove(&files.dir);
    Ok(rep)
}

/// Deletes the oldest journal folders beyond `keep`; returns the deleted ids.
pub fn prune(client: &Path, root: &Path, keep: usize) -> Result<Vec<String>> {
    let _ = client;
    let all = read_all(root);
    let mut gone = Vec::new();
    if all.len() > keep {
        for j in &all[..all.len() - keep] {
            std::fs::remove_dir_all(&j.dir).map_err(|e| Error::io(&j.dir, e))?;
            gone.push(j.id.clone());
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Journal {
        Journal {
            dir: PathBuf::from("/x/20261115T120000Z-ps0109"),
            id: "20261115T120000Z-ps0109".into(),
            kind: "patch".into(),
            label: "ps0109.patch".into(),
            created_utc: "2026-11-15T12:00:00Z".into(),
            tool: "shaiya 0.1.0".into(),
            sah_name: "data.sah".into(),
            saf_name: "data.saf".into(),
            saf_len_before: 1_991_954_802,
            saf_len_after: 1_994_914_916,
            sah_sha256_before: "a".repeat(64),
            sah_sha256_after: "b".repeat(64),
            loose: vec![
                LooseRecord { rel: "game.exe".into(), existed: true, backup: Some("loose/game.exe".into()), sha256_after: Some("c".repeat(64)) },
                LooseRecord { rel: "my notes/with space.txt".into(), existed: false, backup: None, sha256_after: None },
            ],
            state: State::Applied,
        }
    }

    #[test]
    fn journal_text_round_trips() {
        let j = sample();
        let text = j.to_text();
        assert!(text.starts_with("format=1\nid=20261115T120000Z-ps0109\nkind=patch\n"), "{text}");
        assert!(text.contains("loose=1\t") && text.contains("\tloose/game.exe\tgame.exe\n"), "{text}");
        assert_eq!(Journal::parse(&j.dir, &text).unwrap(), j);
        let mut rb = j.clone();
        rb.state = State::RolledBack { at_utc: "2026-11-16T00:00:00Z".into() };
        assert_eq!(Journal::parse(&rb.dir, &rb.to_text()).unwrap(), rb);
        // unknown keys are ignored
        let extra = format!("{text}future_key=1\n");
        assert_eq!(Journal::parse(&j.dir, &extra).unwrap(), j);
    }

    #[test]
    fn damaged_journals_are_errors_not_panics() {
        let text = sample().to_text();
        let dir = PathBuf::from("/x/j");
        for n in 0..text.len() {
            let _ = Journal::parse(&dir, &text[..n]);
        }
        let bytes = text.as_bytes();
        for at in 0..bytes.len() {
            let mut b = bytes.to_vec();
            b[at] ^= 1;
            if let Ok(s) = String::from_utf8(b) {
                let _ = Journal::parse(&dir, &s);
            }
        }
        assert!(Journal::parse(&dir, "state=applied\n").is_err());
        assert!(Journal::parse(&dir, &text.replace("state=applied", "state=maybe")).is_err());
        assert!(Journal::parse(&dir, &text.replace(&"a".repeat(64), "zz")).is_err());
    }

    #[test]
    fn marker_text_round_trips_and_survives_damage() {
        let m = Marker {
            op: "apply".into(),
            label: "ps0109.patch".into(),
            saf_len_before: 10,
            sah_sha256_before: "a".repeat(64),
            sah_sha256_after: "b".repeat(64),
            journal: Some(PathBuf::from("/c/shaiya-toolkit-backups/x")),
            staged: vec![(".notice.txt.1.shaiya-tmp".into(), "notice.txt".into())],
        };
        let t = m.to_text();
        assert_eq!(Marker::parse(&t).unwrap(), m);
        let none = Marker { journal: None, staged: vec![], ..m.clone() };
        assert_eq!(Marker::parse(&none.to_text()).unwrap(), none);
        for n in 0..t.len() {
            let _ = Marker::parse(&t[..n]);
        }
        for at in 0..t.len() {
            let mut b = t.as_bytes().to_vec();
            b[at] ^= 1;
            if let Ok(s) = String::from_utf8(b) {
                let _ = Marker::parse(&s);
            }
        }
        assert!(Marker::parse("op=apply\n").is_err());
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("ps0109.patch"), "ps0109");
        assert_eq!(slug("archive edit: 3 files"), "archive-edit-3-files");
        assert_eq!(slug("..."), "change");
        assert_eq!(slug("A/B\\C"), "a-b-c");
        assert!(slug(&"x".repeat(100)).len() <= 40);
    }
}
