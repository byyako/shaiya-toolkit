//! Publishing patches for the regular updater: copies `psNNNN.patch` files into a folder that a web server hands out and raises
//! `PatchFileVersion` in that folder's `UpdateVersion.ini`.
//!
//! A wrong `UpdateVersion.ini` stops every player's update, so [`plan_publish`] decides everything first (and refuses what looks wrong),
//! [`publish`] writes nothing that the plan did not name, puts the patch files in place before the ini (so the ini never points at a
//! missing file), and reads every written file back.
//!
//! Folder layouts: [`Layout::Shaiya`] is `DIR/Shaiya/UpdateVersion.ini` with `DIR/Shaiya/patch/psNNNN.patch` (the original US EP4.5
//! updater's); [`Layout::Flat`] puts the ini and the patches in `DIR` itself.
//!
//! The ini is edited line by line: lines that do not need to change stay byte for byte, a new key goes after the last line of
//! `[Version]`, and new lines end with CRLF (an existing file's last line ending is used when it has one).

use crate::{open_zip, patch_name, patch_number, signature_path, verify};
use shaiya_archive::client::ClientFiles;
use shaiya_core::fs::{write_atomic, write_atomic_from_with, Sync};
use shaiya_core::hash::{sha256_file, to_hex};
use shaiya_core::{Error, Job, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The name of the server file.
pub const INI_NAME: &str = "UpdateVersion.ini";
/// `UpdaterVersion` written into a new ini when none is given (the caller says so in a note).
pub const DEFAULT_UPDATER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Layout {
    /// `DIR/Shaiya/UpdateVersion.ini`, `DIR/Shaiya/patch/psNNNN.patch`.
    Shaiya,
    /// Both in `DIR`.
    Flat,
}

impl Layout {
    pub fn parse(s: &str) -> Option<Layout> {
        match s.to_ascii_lowercase().as_str() {
            "shaiya" => Some(Layout::Shaiya),
            "flat" => Some(Layout::Flat),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublishOptions {
    pub layout: Layout,
    /// The client generation (Version.ini `CheckVersion`). Required for a new ini; with an existing ini it must equal the ini's
    /// unless `new_check_version`.
    pub check_version: Option<u32>,
    /// `UpdaterVersion` for a new ini (default [`DEFAULT_UPDATER_VERSION`]); an existing ini's value is changed only when this is given.
    pub updater_version: Option<u32>,
    /// Allow a `CheckVersion` that differs from the existing ini's (every player is then told to reinstall).
    pub new_check_version: bool,
    /// Folders that are clients (the saved client list): the destination may not be one of them or inside one.
    pub client_dirs: Vec<PathBuf>,
}

impl Default for PublishOptions {
    fn default() -> Self {
        PublishOptions { layout: Layout::Shaiya, check_version: None, updater_version: None, new_check_version: false, client_dirs: Vec::new() }
    }
}

/// The three keys of `[Version]` in `UpdateVersion.ini`, as found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct IniValues {
    pub check_version: Option<u32>,
    pub updater_version: Option<u32>,
    pub patch_file_version: Option<u32>,
    /// Keys of the three that are present but not a number.
    pub not_numbers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum FileState {
    /// Not in the folder yet: will be copied.
    New,
    /// The same bytes (SHA-256) are already there: nothing to copy.
    AlreadyPublished,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PlanFile {
    pub source: PathBuf,
    pub target: PathBuf,
    pub state: FileState,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PlanItem {
    pub number: u32,
    pub patch: PlanFile,
    /// The `.sig` beside the patch, copied too when there is one.
    pub signature: Option<PlanFile>,
    /// The patch carries `remove.txt` (the original updater ignores removals).
    pub has_removals: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PublishPlan {
    pub layout: Layout,
    pub dir: PathBuf,
    pub ini_path: PathBuf,
    pub patch_dir: PathBuf,
    pub items: Vec<PlanItem>,
    /// What the existing ini holds; `None` when there is no ini yet.
    pub ini_before: Option<IniValues>,
    pub check_version: u32,
    pub updater_version: u32,
    pub patch_file_version_before: Option<u32>,
    pub patch_file_version_after: u32,
    /// `CheckVersion` differs from the existing ini's (allowed by `new_check_version`): players are told to reinstall.
    pub check_version_changed: bool,
    pub updater_version_changed: bool,
    /// The new ini's `UpdaterVersion` is the default 1 because none was given.
    pub updater_version_defaulted: bool,
    /// The ini as it will be written; equal to what is there when nothing changes.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub ini_text: Vec<u8>,
    /// The ini bytes the plan was made against (`None` = no file); [`publish`] refuses when the file changed since.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub ini_bytes_before: Option<Vec<u8>>,
    pub ini_changed: bool,
    pub warnings: Vec<String>,
}

impl PublishPlan {
    /// Nothing to copy and the ini stays as it is.
    pub fn nothing_to_do(&self) -> bool {
        !self.ini_changed && self.items.iter().all(|i| i.patch.state == FileState::AlreadyPublished && i.signature.as_ref().is_none_or(|s| s.state == FileState::AlreadyPublished))
    }

    /// Files that will be copied.
    pub fn copies(&self) -> impl Iterator<Item = &PlanFile> {
        self.items.iter().flat_map(|i| std::iter::once(&i.patch).chain(i.signature.as_ref())).filter(|f| f.state == FileState::New)
    }
}

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PublishReport {
    pub copied: Vec<PathBuf>,
    pub already_published: Vec<PathBuf>,
    pub ini_written: Option<PathBuf>,
    pub patch_file_version: u32,
    /// Every written file was read back and matched.
    pub read_back: bool,
}

// ------------------------------------------------------------------------------------------------ ini text

struct Line {
    start: usize,
    /// End of the text, before the line terminator.
    text_end: usize,
    /// End including the terminator.
    end: usize,
}

fn split_lines(b: &[u8]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < b.len() {
        let nl = b[start..].iter().position(|&c| c == b'\n').map(|p| start + p);
        let (end, mut text_end) = match nl {
            Some(p) => (p + 1, p),
            None => (b.len(), b.len()),
        };
        if text_end > start && b[text_end - 1] == b'\r' {
            text_end -= 1;
        }
        out.push(Line { start, text_end, end });
        start = end;
    }
    out
}

fn trim_ws(b: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = b.len();
    while s < e && matches!(b[s], b' ' | b'\t') {
        s += 1;
    }
    while e > s && matches!(b[e - 1], b' ' | b'\t' | b'\r') {
        e -= 1;
    }
    &b[s..e]
}

fn strip_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)
}

/// `Some(name)` for a `[name]` line.
fn section_of(text: &[u8]) -> Option<String> {
    let t = trim_ws(strip_bom(text));
    let inner = t.strip_prefix(b"[")?.strip_suffix(b"]")?;
    Some(String::from_utf8_lossy(inner).trim().to_string())
}

fn is_comment(text: &[u8]) -> bool {
    matches!(trim_ws(strip_bom(text)).first(), Some(b';') | Some(b'#'))
}

/// `(key lowercase, index of '=')` of a key line.
fn key_of(text: &[u8]) -> Option<(String, usize)> {
    if is_comment(text) || section_of(text).is_some() {
        return None;
    }
    let eq = text.iter().position(|&c| c == b'=')?;
    let key = String::from_utf8_lossy(trim_ws(strip_bom(&text[..eq]))).to_ascii_lowercase();
    Some((key, eq))
}

const KEYS: [&str; 3] = ["checkversion", "updaterversion", "patchfileversion"];

/// The line range `[first_line, end_line)` after the `[Version]` header; `Err` when there are several `[Version]` sections.
fn version_section(b: &[u8], lines: &[Line]) -> Result<Option<(usize, usize)>> {
    let mut found: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let t = &b[lines[i].start..lines[i].text_end];
        if section_of(t).is_some_and(|s| s.eq_ignore_ascii_case("version")) {
            if found.is_some() {
                return Err(Error::Refused("UpdateVersion.ini has more than one [Version] section; fix it by hand first (the toolkit will not guess which one the updater reads)".into()));
            }
            let mut j = i + 1;
            while j < lines.len() && section_of(&b[lines[j].start..lines[j].text_end]).is_none() {
                j += 1;
            }
            found = Some((i + 1, j));
            i = j;
        } else {
            i += 1;
        }
    }
    Ok(found)
}

/// The three keys of `[Version]`. A key that appears twice is refused (the updater's reader and ours could disagree).
pub fn parse_update_ini(b: &[u8]) -> Result<IniValues> {
    let lines = split_lines(b);
    let mut out = IniValues::default();
    let Some((from, to)) = version_section(b, &lines)? else { return Ok(out) };
    let mut seen: Vec<&str> = Vec::new();
    for l in &lines[from..to] {
        let t = &b[l.start..l.text_end];
        let Some((key, eq)) = key_of(t) else { continue };
        let Some(&k) = KEYS.iter().find(|k| **k == key) else { continue };
        if seen.contains(&k) {
            return Err(Error::Refused(format!("UpdateVersion.ini has the key {k} twice in [Version]; fix it by hand first")));
        }
        seen.push(k);
        let v = String::from_utf8_lossy(trim_ws(&t[eq + 1..])).to_string();
        let n = v.parse::<u32>().ok();
        if n.is_none() {
            out.not_numbers.push(k.to_string());
        }
        match k {
            "checkversion" => out.check_version = n,
            "updaterversion" => out.updater_version = n,
            _ => out.patch_file_version = n,
        }
    }
    Ok(out)
}

fn canonical_key(k: &str) -> &'static str {
    match k {
        "checkversion" => "CheckVersion",
        "updaterversion" => "UpdaterVersion",
        _ => "PatchFileVersion",
    }
}

/// A new `UpdateVersion.ini`: `[Version]`, CRLF, ASCII.
pub fn new_update_ini(check_version: u32, updater_version: u32, patch_file_version: u32) -> Vec<u8> {
    format!("[Version]\r\nCheckVersion={check_version}\r\nUpdaterVersion={updater_version}\r\nPatchFileVersion={patch_file_version}\r\n").into_bytes()
}

/// Sets keys of `[Version]` in an existing ini. A key that is there gets only its value replaced (everything else on the line stays);
/// a key that is not there is added after the last line of the section; a file without `[Version]` gets the section at its end. Every
/// other line is kept byte for byte.
pub fn set_update_ini_keys(existing: &[u8], sets: &[(&str, u32)]) -> Result<Vec<u8>> {
    let lines = split_lines(existing);
    let section = version_section(existing, &lines)?;
    let mut out: Vec<u8> = Vec::with_capacity(existing.len() + 64);
    let mut missing: Vec<(&str, u32)> = Vec::new();
    // which line each present key is on
    let mut replace: Vec<(usize, u32)> = Vec::new();
    for (k, v) in sets {
        let k = k.to_ascii_lowercase();
        let mut at = None;
        if let Some((from, to)) = section {
            for (i, l) in lines.iter().enumerate().take(to).skip(from) {
                if key_of(&existing[l.start..l.text_end]).is_some_and(|(key, _)| key == k) {
                    at = Some(i);
                    break;
                }
            }
        }
        match at {
            Some(i) => replace.push((i, *v)),
            None => missing.push((canonical_key(&k), *v)),
        }
    }
    let ending = |l: &Line| -> &[u8] { &existing[l.text_end..l.end] };
    let default_end: &[u8] = b"\r\n";
    // line after which missing keys go
    let insert_after: Option<usize> = section.map(|(from, to)| {
        let mut last = from.saturating_sub(1);
        for i in from..to {
            if !trim_ws(&existing[lines[i].start..lines[i].text_end]).is_empty() {
                last = i;
            }
        }
        last
    });
    for (i, l) in lines.iter().enumerate() {
        let text = &existing[l.start..l.text_end];
        if let Some((_, v)) = replace.iter().find(|(li, _)| *li == i) {
            let eq = text.iter().position(|&c| c == b'=').unwrap_or(0);
            let mut vs = eq + 1;
            while vs < text.len() && matches!(text[vs], b' ' | b'\t') {
                vs += 1;
            }
            let mut ve = text.len();
            while ve > vs && matches!(text[ve - 1], b' ' | b'\t') {
                ve -= 1;
            }
            out.extend_from_slice(&text[..vs]);
            out.extend_from_slice(v.to_string().as_bytes());
            out.extend_from_slice(&text[ve..]);
            out.extend_from_slice(ending(l));
        } else {
            out.extend_from_slice(&existing[l.start..l.end]);
        }
        if insert_after == Some(i) && !missing.is_empty() {
            let end = if ending(l).is_empty() { default_end } else { ending(l) };
            if ending(l).is_empty() {
                out.extend_from_slice(end);
            }
            for (k, v) in &missing {
                out.extend_from_slice(format!("{k}={v}").as_bytes());
                out.extend_from_slice(end);
            }
            missing.clear();
        }
    }
    if !missing.is_empty() {
        // no [Version] section, or an empty one at the very end
        if let Some(last) = lines.last() {
            if ending(last).is_empty() {
                out.extend_from_slice(default_end);
            }
        }
        if section.is_none() {
            out.extend_from_slice(b"[Version]\r\n");
        }
        for (k, v) in &missing {
            out.extend_from_slice(format!("{k}={v}\r\n").as_bytes());
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ folders

/// The child of `dir` named `name`, found without regard to letter case (a server folder made on Windows may say `shaiya`).
fn child_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path())
}

fn absolute(p: &Path) -> PathBuf {
    if let Ok(c) = fs::canonicalize(p) {
        return c;
    }
    // not there yet: canonicalize the nearest existing parent and put the rest back
    let mut rest = Vec::new();
    let mut cur = p.to_path_buf();
    while !cur.exists() {
        match (cur.file_name().map(|n| n.to_os_string()), cur.parent().map(Path::to_path_buf)) {
            (Some(n), Some(parent)) => {
                rest.push(n);
                cur = parent;
            }
            _ => break,
        }
    }
    let mut base = fs::canonicalize(&cur).unwrap_or(cur);
    for n in rest.into_iter().rev() {
        base.push(n);
    }
    base
}

/// A client folder the destination is at or inside, if any: one of `clients`, or any folder from `dir` upwards that holds `data.sah` and
/// `data.saf`.
fn client_around(dir: &Path, clients: &[PathBuf]) -> Option<PathBuf> {
    let d = absolute(dir);
    for c in clients {
        if d.starts_with(absolute(c)) {
            return Some(c.clone());
        }
    }
    let mut cur = Some(d.as_path());
    while let Some(p) = cur {
        if p.is_dir() && ClientFiles::locate(p).is_ok() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

// ------------------------------------------------------------------------------------------------ plan

fn plan_file(source: &Path, target_dir: &Path, name: &str, job: &Job) -> Result<PlanFile> {
    let size = fs::metadata(source).map_err(|e| Error::io(source, e))?.len();
    let sha = to_hex(&sha256_file(source, job)?);
    let existing = child_ci(target_dir, name);
    let (target, state) = match existing {
        Some(t) if t.is_file() => {
            if to_hex(&sha256_file(&t, job)?) == sha {
                (t, FileState::AlreadyPublished)
            } else {
                return Err(Error::Refused(format!(
                    "{} is already published and is a different file. A published patch is never replaced (players may have it already); build your changes as the next number instead",
                    t.display()
                )));
            }
        }
        Some(t) => return Err(Error::Refused(format!("{} exists and is not a file", t.display()))),
        None => (target_dir.join(name), FileState::New),
    };
    Ok(PlanFile { source: source.to_path_buf(), target, state, size, sha256: sha })
}

/// Decides everything [`publish`] will do, writes nothing. Refuses (`Error::Refused`) when: no patch is given; a name is not
/// `psNNNN.patch` (four digits at least); two patches have the same number; a patch fails the structure checks of [`verify`]; the
/// destination is a client folder or inside one; the existing ini cannot be read as `[Version]` with `CheckVersion` and
/// `PatchFileVersion`; `CheckVersion` differs from the existing ini's without `new_check_version`; a new ini is made without a
/// `CheckVersion`; a patch file with that name is already in the folder with different content; the new numbers leave a gap after
/// `PatchFileVersion`.
pub fn plan_publish(patches: &[PathBuf], dir: &Path, opts: &PublishOptions, job: &Job) -> Result<PublishPlan> {
    if patches.is_empty() {
        return Err(Error::Refused("no patch given to publish".into()));
    }
    if let Some(c) = client_around(dir, &opts.client_dirs) {
        return Err(Error::Refused(format!("{} is a client folder or inside one ({}); publish into a separate server folder, never into a client", dir.display(), c.display())));
    }
    // names, numbers, structure
    let mut numbered: Vec<(u32, PathBuf)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for p in patches {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match patch_number(&name) {
            Some(n) if patch_name(n).eq_ignore_ascii_case(&name) => numbered.push((n, p.clone())),
            Some(n) => problems.push(format!("{name}: the updater fetches {} (four digits); rename the file", patch_name(n))),
            None => problems.push(format!("{name}: the updater only fetches files named psNNNN.patch")),
        }
    }
    if !problems.is_empty() {
        return Err(Error::Refused(problems.join("\n")));
    }
    numbered.sort_by_key(|(n, _)| *n);
    if let Some(w) = numbered.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(Error::Refused(format!("{} and {} have the same number", w[0].1.display(), w[1].1.display())));
    }
    let mut warnings: Vec<String> = Vec::new();
    let mut has_removals: Vec<bool> = Vec::new();
    for (_, p) in &numbered {
        if !p.is_file() {
            return Err(Error::io(p, std::io::Error::new(std::io::ErrorKind::NotFound, "no such patch file")));
        }
        let rep = verify(p, None, None, job)?;
        if !rep.problems.is_empty() {
            return Err(Error::Refused(format!("{} is not a sound patch:\n  {}", p.display(), rep.problems.join("\n  "))));
        }
        let has = crate::removals(p).map(|r| !r.is_empty()).unwrap_or(false);
        if has {
            warnings.push(format!("{} removes files (remove.txt); the original updater ignores removals, so players on it keep those files", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        }
        has_removals.push(has);
        // reading the zip once more guards against a half-written file the structure check accepted
        open_zip(p)?;
    }
    // folders and the existing ini
    let (patch_dir, ini_dir) = match opts.layout {
        Layout::Flat => (dir.to_path_buf(), dir.to_path_buf()),
        Layout::Shaiya => {
            let sh = child_ci(dir, "Shaiya").unwrap_or_else(|| dir.join("Shaiya"));
            let pd = child_ci(&sh, "patch").unwrap_or_else(|| sh.join("patch"));
            (pd, sh)
        }
    };
    let ini_existing = child_ci(&ini_dir, INI_NAME).filter(|p| p.is_file());
    let ini_path = ini_existing.clone().unwrap_or_else(|| ini_dir.join(INI_NAME));
    let ini_bytes_before = match &ini_existing {
        Some(p) => Some(fs::read(p).map_err(|e| Error::io(p, e))?),
        None => None,
    };
    let ini_before = match &ini_bytes_before {
        Some(b) => {
            let v = parse_update_ini(b)?;
            if !v.not_numbers.is_empty() {
                return Err(Error::Refused(format!("{}: {} is not a number; fix it by hand first", ini_path.display(), v.not_numbers.join(", "))));
            }
            if v.check_version.is_none() || v.patch_file_version.is_none() {
                return Err(Error::Refused(format!(
                    "{} has no [Version] CheckVersion and PatchFileVersion; the toolkit will not guess them. Fix the file by hand, or move it away to start a new one",
                    ini_path.display()
                )));
            }
            Some(v)
        }
        None => None,
    };
    // CheckVersion
    let mut check_version_changed = false;
    let check_version = match (&ini_before, opts.check_version) {
        (Some(v), None) => v.check_version.unwrap_or(0),
        (Some(v), Some(n)) if v.check_version == Some(n) => n,
        (Some(v), Some(n)) => {
            if !opts.new_check_version {
                return Err(Error::Refused(format!(
                    "the existing {INI_NAME} has CheckVersion {} and you gave {n}. A different CheckVersion makes every player's updater tell them to reinstall the game. If that is what you mean, say so (--new-check-version); otherwise use {}",
                    v.check_version.unwrap_or(0),
                    v.check_version.unwrap_or(0)
                )));
            }
            check_version_changed = true;
            warnings.push(format!("CheckVersion changes from {} to {n}: every player will be told to reinstall the game", v.check_version.unwrap_or(0)));
            n
        }
        (None, Some(n)) => n,
        (None, None) => {
            return Err(Error::Refused(format!(
                "a new {INI_NAME} needs the client generation: give --check-version N (the CheckVersion in your players' Version.ini); the toolkit does not guess it"
            )))
        }
    };
    // UpdaterVersion
    let mut updater_version_changed = false;
    let mut updater_version_defaulted = false;
    let updater_version = match (ini_before.as_ref().and_then(|v| v.updater_version), opts.updater_version) {
        (Some(cur), None) => cur,
        (Some(cur), Some(n)) if cur == n => n,
        (Some(cur), Some(n)) => {
            updater_version_changed = true;
            warnings.push(format!("UpdaterVersion changes from {cur} to {n}: players' updaters will look for updater version {n} on the server (publishing never writes an updater file)"));
            n
        }
        (None, Some(n)) => n,
        (None, None) => {
            updater_version_defaulted = true;
            DEFAULT_UPDATER_VERSION
        }
    };
    // numbers
    let pfv_before = ini_before.as_ref().and_then(|v| v.patch_file_version);
    let mut expected_next = pfv_before.map(|p| p + 1);
    let mut items = Vec::new();
    for (idx, (n, p)) in numbered.iter().enumerate() {
        let name = patch_name(*n);
        let patch = plan_file(p, &patch_dir, &name, job)?;
        let sig_src = signature_path(p);
        let signature = if sig_src.is_file() { Some(plan_file(&sig_src, &patch_dir, &format!("{name}.sig"), job)?) } else { None };
        match (pfv_before, expected_next) {
            (Some(pfv), _) if *n <= pfv => {
                if patch.state == FileState::New {
                    warnings.push(format!("{name} is at or below PatchFileVersion {pfv} but is not in the folder: it fills a missing number"));
                }
            }
            (_, Some(exp)) => {
                if *n != exp {
                    return Err(Error::Refused(format!(
                        "{name} leaves a gap: the next number after PatchFileVersion {} is {} (a player at that version asks for every number in turn and would stop at the missing one). Publish {} first",
                        exp - 1,
                        exp,
                        patch_name(exp)
                    )));
                }
                expected_next = Some(n + 1);
            }
            (_, None) => {
                if idx == 0 && *n > 1 {
                    warnings.push(format!(
                        "a new {INI_NAME} starts at {name}: players whose CurrentVersion is below {} will ask for earlier patches that this folder does not hold",
                        n - 1
                    ));
                }
                expected_next = Some(n + 1);
            }
        }
        items.push(PlanItem { number: *n, patch, signature, has_removals: has_removals[idx] });
    }
    let highest = items.iter().map(|i| i.number).max().unwrap_or(0);
    let pfv_after = pfv_before.map_or(highest, |p| p.max(highest));
    // the ini text
    let ini_text = match &ini_bytes_before {
        None => new_update_ini(check_version, updater_version, pfv_after),
        Some(b) => {
            let v = ini_before.as_ref().expect("parsed with the bytes");
            let mut sets: Vec<(&str, u32)> = Vec::new();
            if v.check_version != Some(check_version) {
                sets.push(("CheckVersion", check_version));
            }
            if v.updater_version != Some(updater_version) {
                sets.push(("UpdaterVersion", updater_version));
            }
            if v.patch_file_version != Some(pfv_after) {
                sets.push(("PatchFileVersion", pfv_after));
            }
            if sets.is_empty() {
                b.clone()
            } else {
                set_update_ini_keys(b, &sets)?
            }
        }
    };
    let ini_changed = ini_bytes_before.as_deref() != Some(ini_text.as_slice());
    if ini_before.is_none() && updater_version_defaulted {
        warnings.push(format!("UpdaterVersion is set to {DEFAULT_UPDATER_VERSION} because none was given; set it to the version of the updater your server offers (--updater-version)"));
    }
    Ok(PublishPlan {
        layout: opts.layout,
        dir: dir.to_path_buf(),
        ini_path,
        patch_dir,
        items,
        ini_before,
        check_version,
        updater_version,
        patch_file_version_before: pfv_before,
        patch_file_version_after: pfv_after,
        check_version_changed,
        updater_version_changed,
        updater_version_defaulted,
        ini_text,
        ini_bytes_before,
        ini_changed,
        warnings,
    })
}

// ------------------------------------------------------------------------------------------------ write

fn copy_checked(f: &PlanFile, job: &Job) -> Result<()> {
    if let Some(parent) = f.target.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    if f.target.exists() {
        return Err(Error::Refused(format!("{} appeared since the plan was made; nothing is replaced", f.target.display())));
    }
    if to_hex(&sha256_file(&f.source, job)?) != f.sha256 {
        return Err(Error::Refused(format!("{} changed since the plan was made", f.source.display())));
    }
    let name = f.target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stage = f.target.with_file_name(format!(".{name}.publish.shaiya-tmp"));
    let result = (|| -> Result<()> {
        let mut src = fs::File::open(&f.source).map_err(|e| Error::io(&f.source, e))?;
        write_atomic_from_with(&stage, &mut src, Sync::Yes, job)?;
        if to_hex(&sha256_file(&stage, job)?) != f.sha256 {
            return Err(Error::Refused(format!("the copy of {name} does not match the original; nothing was published")));
        }
        if f.target.exists() {
            return Err(Error::Refused(format!("{} appeared while copying; nothing is replaced", f.target.display())));
        }
        fs::rename(&stage, &f.target).map_err(|e| Error::io(&f.target, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&stage);
    }
    result?;
    if to_hex(&sha256_file(&f.target, job)?) != f.sha256 {
        return Err(Error::Refused(format!("{} was read back and does not match the original", f.target.display())));
    }
    Ok(())
}

/// Writes what `plan` says: the patch files (and their `.sig`) first, each through a temporary file in the destination, compared by
/// SHA-256 and renamed into place, then the ini (atomic replace), then reads the ini back and compares its three numbers. Nothing in
/// the destination is replaced except the ini; refuses when the ini changed since the plan.
pub fn publish(plan: &PublishPlan, job: &Job) -> Result<PublishReport> {
    let now = match &plan.ini_bytes_before {
        Some(_) => fs::read(&plan.ini_path).ok(),
        None => None,
    };
    if now != plan.ini_bytes_before || (plan.ini_bytes_before.is_none() && plan.ini_path.exists()) {
        return Err(Error::Refused(format!("{} changed since the plan was made; plan again", plan.ini_path.display())));
    }
    let mut rep = PublishReport { patch_file_version: plan.patch_file_version_after, ..Default::default() };
    for item in &plan.items {
        for f in std::iter::once(&item.patch).chain(item.signature.as_ref()) {
            job.check()?;
            match f.state {
                FileState::New => {
                    copy_checked(f, job)?;
                    rep.copied.push(f.target.clone());
                }
                FileState::AlreadyPublished => rep.already_published.push(f.target.clone()),
            }
        }
    }
    if plan.ini_changed {
        job.check()?;
        if let Some(parent) = plan.ini_path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        write_atomic(&plan.ini_path, &plan.ini_text)?;
        let back = fs::read(&plan.ini_path).map_err(|e| Error::io(&plan.ini_path, e))?;
        let v = parse_update_ini(&back)?;
        if back != plan.ini_text || v.patch_file_version != Some(plan.patch_file_version_after) || v.check_version != Some(plan.check_version) || v.updater_version != Some(plan.updater_version) {
            return Err(Error::Refused(format!("{} was read back and does not hold the planned values; check it by hand before players update", plan.ini_path.display())));
        }
        rep.ini_written = Some(plan.ini_path.clone());
    }
    rep.read_back = true;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_ini_is_crlf_ascii() {
        assert_eq!(new_update_ini(3, 1, 110), b"[Version]\r\nCheckVersion=3\r\nUpdaterVersion=1\r\nPatchFileVersion=110\r\n");
    }

    #[test]
    fn parse_is_case_insensitive_and_finds_the_numbers() {
        let v = parse_update_ini(b"\xEF\xBB\xBF[version]\r\ncheckversion = 3\r\nUPDATERVERSION=2\r\nPatchFileVersion=109\r\n").unwrap();
        assert_eq!((v.check_version, v.updater_version, v.patch_file_version), (Some(3), Some(2), Some(109)));
        let v = parse_update_ini(b"[Version]\r\nCheckVersion=x\r\n").unwrap();
        assert_eq!(v.not_numbers, vec!["checkversion"]);
        assert!(parse_update_ini(b"[Version]\r\nCheckVersion=1\r\nCheckVersion=2\r\n").is_err());
        assert!(parse_update_ini(b"[Version]\r\nA=1\r\n[Version]\r\nB=2\r\n").is_err());
    }

    #[test]
    fn editing_keeps_every_other_line_byte_for_byte() {
        let src = b"; server file\r\n[Other]\r\nPatchFileVersion=1\r\n\r\n[Version]\r\n; note\r\nCheckVersion = 3\r\nExtra=keep me  \r\nPatchFileVersion  =  109   \r\nUpdaterVersion=1\r\n\r\n[Tail]\r\nx=y\r\n";
        let out = set_update_ini_keys(src, &[("PatchFileVersion", 110)]).unwrap();
        assert_eq!(out, b"; server file\r\n[Other]\r\nPatchFileVersion=1\r\n\r\n[Version]\r\n; note\r\nCheckVersion = 3\r\nExtra=keep me  \r\nPatchFileVersion  =  110   \r\nUpdaterVersion=1\r\n\r\n[Tail]\r\nx=y\r\n".to_vec());
    }

    #[test]
    fn a_missing_key_goes_after_the_last_line_of_the_section() {
        let src = b"[Version]\r\nCheckVersion=3\r\nPatchFileVersion=9\r\n\r\n[Tail]\r\nx=y\r\n";
        let out = set_update_ini_keys(src, &[("UpdaterVersion", 1)]).unwrap();
        assert_eq!(out, b"[Version]\r\nCheckVersion=3\r\nPatchFileVersion=9\r\nUpdaterVersion=1\r\n\r\n[Tail]\r\nx=y\r\n".to_vec());
        // LF files get LF; a last line without terminator is closed first
        let out = set_update_ini_keys(b"[Version]\nCheckVersion=3\nPatchFileVersion=9", &[("UpdaterVersion", 2)]).unwrap();
        assert_eq!(out, b"[Version]\nCheckVersion=3\nPatchFileVersion=9\r\nUpdaterVersion=2\r\n".to_vec());
        let out = set_update_ini_keys(b"x=1\r\n", &[("CheckVersion", 3)]).unwrap();
        assert_eq!(out, b"x=1\r\n[Version]\r\nCheckVersion=3\r\n".to_vec());
    }
}
