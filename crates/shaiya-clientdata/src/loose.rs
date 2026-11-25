//! A loose data folder as a read-only set of files ([`LooseTree`]): the folder of a client that has no `data.sah`/`data.saf` (an extracted
//! client, a Shaiya-GX `data` folder). The tree is listed once; names are looked up without letter case; a file is read through the listed
//! path after every folder on the way was checked again for links. Nothing here writes a file.
//!
//! What is left out of the list, and counted so the person is told ([`LooseTree::notes`]):
//! - links (symbolic links and Windows junctions) inside the folder: a link could lead out of the folder;
//! - both spellings of a name that differs only in letter case (on a case-sensitive disk), files and folders alike;
//! - `*.shaiya-tmp` files (the toolkit's own temporary files) and, at the top of the folder, `shaiya-toolkit-backups`;
//! - names that are not text, or hold `:` or `\`, paths over [`Limits::max_path`] bytes, folders deeper than [`Limits::max_depth`];
//! - anything that is neither a file nor a folder.
//!
//! A name is taken as it is on disk (`%XX` escapes are not decoded). A file over [`Limits::max_read`] (4 GiB) is listed and never read.

use shaiya_archive::journal::DEFAULT_DIR;
use shaiya_archive::loose::LooseLayout;
use shaiya_core::{Error, Job, Result, Unit};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The caps of one tree. [`Limits::default`] is what the programs use; tests lower them to reach a cap with a few files.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// More files than this refuses the folder (a real client has about 100,000).
    pub max_files: usize,
    /// Folders nested deeper than this below the data folder are not entered.
    pub max_depth: usize,
    /// A relative path longer than this many bytes is left out.
    pub max_path: usize,
    /// A file longer than this is listed and never read.
    pub max_read: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_files: 1_000_000, max_depth: 16, max_path: 1024, max_read: 4 << 30 }
    }
}

/// One listed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LooseFile {
    /// The path below the data folder, `/` separated, spelled as on disk.
    pub rel: String,
    /// Size and modification time (nanoseconds since 1970, 0 when unknown) when the folder was listed.
    pub len: u64,
    pub mtime: u64,
    abs: PathBuf,
}

/// What a listing left out (see the module text).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    pub links: usize,
    pub case_duplicates: Vec<String>,
    pub temps: usize,
    pub bad_names: usize,
    pub too_long: usize,
    pub too_deep: usize,
    pub other: usize,
}

pub struct LooseTree {
    layout: LooseLayout,
    limits: Limits,
    files: Vec<LooseFile>,
    by_key: HashMap<String, usize>,
    skipped: Skipped,
}

fn key_of(rel: &str) -> String {
    rel.to_lowercase()
}

/// The names of one folder split into those kept and the case-only duplicates (every spelling of a clashing name goes to the second list).
/// Letter case is the Unicode lower case, as the tree's lookups use.
pub fn split_case_duplicates(names: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut groups: HashMap<String, usize> = HashMap::new();
    for n in &names {
        *groups.entry(key_of(n)).or_insert(0) += 1;
    }
    names.into_iter().partition(|n| groups.get(&key_of(n)).copied() == Some(1))
}

fn mtime_of(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)).unwrap_or(0)
}

/// A name the tree can list: text, no `:` or `\` (the name rules of `loose_name_problem`: such a name could not be looked up).
fn listable(name: &str) -> bool {
    !name.is_empty() && !name.contains([':', '\\', '/']) && name != "." && name != ".."
}

impl LooseTree {
    /// Lists the data folder of `layout`. `Err` when it is not a folder, cannot be listed, holds more than [`Limits::max_files`] files, or
    /// the job was cancelled (checked per folder).
    pub fn open(layout: &LooseLayout, job: &Job) -> Result<LooseTree> {
        Self::open_with(layout, Limits::default(), job)
    }

    pub fn open_with(layout: &LooseLayout, limits: Limits, job: &Job) -> Result<LooseTree> {
        let data = &layout.data;
        if !data.is_dir() {
            return Err(Error::io(data, std::io::Error::new(std::io::ErrorKind::NotFound, "the data folder is not there")));
        }
        job.stage("listing files");
        let mut files: Vec<LooseFile> = Vec::new();
        let mut skipped = Skipped::default();
        // (folder, its path below data with a trailing `/`, depth of its entries)
        let mut stack: Vec<(PathBuf, String, usize)> = vec![(data.clone(), String::new(), 0)];
        while let Some((dir, prefix, depth)) = stack.pop() {
            job.check()?;
            let rd = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
            let mut entries: Vec<(String, std::fs::DirEntry)> = Vec::new();
            for e in rd {
                let e = e.map_err(|e| Error::io(&dir, e))?;
                match e.file_name().to_str() {
                    Some(n) if listable(n) => entries.push((n.to_string(), e)),
                    _ => skipped.bad_names += 1,
                }
            }
            let (kept, dups) = split_case_duplicates(entries.iter().map(|(n, _)| n.clone()).collect());
            if !dups.is_empty() {
                skipped.case_duplicates.extend(dups.iter().map(|n| format!("{prefix}{n}")));
            }
            let kept: std::collections::HashSet<String> = kept.into_iter().collect();
            job.advance(entries.len() as u64);
            for (name, e) in entries {
                if !kept.contains(&name) {
                    continue;
                }
                let Ok(t) = e.file_type() else {
                    skipped.other += 1;
                    continue;
                };
                if t.is_symlink() {
                    skipped.links += 1;
                } else if t.is_dir() {
                    if depth == 0 && name.eq_ignore_ascii_case(DEFAULT_DIR) {
                        continue;
                    }
                    if depth + 1 > limits.max_depth {
                        skipped.too_deep += 1;
                    } else {
                        stack.push((e.path(), format!("{prefix}{name}/"), depth + 1));
                    }
                } else if t.is_file() {
                    if name.to_ascii_lowercase().ends_with(".shaiya-tmp") {
                        skipped.temps += 1;
                        continue;
                    }
                    let rel = format!("{prefix}{name}");
                    if rel.len() > limits.max_path {
                        skipped.too_long += 1;
                        continue;
                    }
                    if files.len() >= limits.max_files {
                        return Err(Error::Refused(format!("{} holds more than {} files; the toolkit lists at most that many", data.display(), limits.max_files)));
                    }
                    let (len, mtime) = match e.metadata() {
                        Ok(m) => (m.len(), mtime_of(&m)),
                        Err(_) => {
                            skipped.other += 1;
                            continue;
                        }
                    };
                    files.push(LooseFile { rel, len, mtime, abs: e.path() });
                } else {
                    skipped.other += 1;
                }
            }
        }
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        skipped.case_duplicates.sort();
        let by_key = files.iter().enumerate().map(|(i, f)| (key_of(&f.rel), i)).collect();
        Ok(LooseTree { layout: layout.clone(), limits, files, by_key, skipped })
    }

    pub fn layout(&self) -> &LooseLayout {
        &self.layout
    }

    pub fn files(&self) -> &[LooseFile] {
        &self.files
    }

    pub fn skipped(&self) -> &Skipped {
        &self.skipped
    }

    /// Every listed path, sorted, spelled as on disk.
    pub fn paths(&self) -> Vec<String> {
        self.files.iter().map(|f| f.rel.clone()).collect()
    }

    /// What the listing left out, as sentences for the person (empty when nothing was left out).
    pub fn notes(&self) -> Vec<String> {
        let s = &self.skipped;
        let mut out = Vec::new();
        let n = |c: usize, one: &str, many: &str| format!("{c} {}", if c == 1 { one } else { many });
        if s.links > 0 {
            out.push(format!("{} left out (a link could lead out of the folder)", n(s.links, "link", "links")));
        }
        if !s.case_duplicates.is_empty() {
            let shown: Vec<&str> = s.case_duplicates.iter().take(5).map(String::as_str).collect();
            let more = if s.case_duplicates.len() > 5 { format!(" and {} more", s.case_duplicates.len() - 5) } else { String::new() };
            out.push(format!("{} that differ only in letter case left out (all spellings): {}{more}", n(s.case_duplicates.len(), "name", "names"), shown.join(", ")));
        }
        if s.temps > 0 {
            out.push(format!("{} left out (the toolkit's temporary files)", n(s.temps, "*.shaiya-tmp file", "*.shaiya-tmp files")));
        }
        if s.bad_names > 0 {
            out.push(format!("{} left out (not text, or with a : or \\ in it)", n(s.bad_names, "name", "names")));
        }
        if s.too_long > 0 {
            out.push(format!("{} left out (over {} bytes)", n(s.too_long, "path", "paths"), self.limits.max_path));
        }
        if s.too_deep > 0 {
            out.push(format!("{} not entered (deeper than {} levels)", n(s.too_deep, "folder", "folders"), self.limits.max_depth));
        }
        if s.other > 0 {
            out.push(format!("{} left out (neither a file nor a folder, or not readable)", n(s.other, "entry", "entries")));
        }
        out
    }

    /// The listed file of `path` (`/` or `\`, any letter case). A path with `..`, `.` or `:` in it names nothing.
    pub fn get(&self, path: &str) -> Option<&LooseFile> {
        let mut parts = Vec::new();
        for p in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
            if p == ".." || p == "." || p.contains(':') {
                return None;
            }
            parts.push(p);
        }
        if parts.is_empty() {
            return None;
        }
        self.by_key.get(&key_of(&parts.join("/"))).map(|&i| &self.files[i])
    }

    /// The path of a listed file after every folder between the data folder and it was looked at again: a link that appeared since the
    /// listing refuses the read.
    fn checked<'a>(&self, f: &'a LooseFile) -> Result<&'a Path> {
        let rel = f.abs.strip_prefix(&self.layout.data).unwrap_or(&f.abs);
        let mut at = self.layout.data.clone();
        for part in rel.components() {
            at.push(part);
            let m = std::fs::symlink_metadata(&at).map_err(|e| Error::io(&at, e))?;
            if m.file_type().is_symlink() {
                return Err(Error::Refused(format!("{}: a link is in the way now; the folder was changed since it was listed", at.display())));
            }
        }
        Ok(&f.abs)
    }

    /// The file as it is now: size and modification time, `None` when it is gone, is a link now or is not a file.
    fn live(&self, path: &str) -> Option<(u64, u64)> {
        let f = self.get(path)?;
        let p = self.checked(f).ok()?;
        let m = std::fs::symlink_metadata(p).ok()?;
        m.is_file().then(|| (m.len(), mtime_of(&m)))
    }

    /// The size of the file now, `None` when there is no such file.
    pub fn len(&self, path: &str) -> Option<u64> {
        self.live(path).map(|(len, _)| len)
    }

    /// `(modification time in nanoseconds, size)` of the file now. A file changed on disk changes its stamp (a change that keeps both the
    /// time and the size is not seen, as with any such stamp).
    pub fn stamp(&self, path: &str) -> Option<(u64, u64)> {
        self.live(path).map(|(len, mtime)| (mtime, len))
    }

    fn open_file(&self, path: &str) -> Result<(std::fs::File, u64)> {
        let f = self.get(path).ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the folder")))?;
        let p = self.checked(f)?;
        let file = std::fs::File::open(p).map_err(|e| Error::io(p, e))?;
        let m = file.metadata().map_err(|e| Error::io(p, e))?;
        if !m.is_file() {
            return Err(Error::Refused(format!("{}: not a file", f.rel)));
        }
        if m.len() > self.limits.max_read {
            return Err(Error::Refused(format!("{}: {} bytes is over the {} the toolkit reads from one file", f.rel, m.len(), self.limits.max_read)));
        }
        Ok((file, m.len()))
    }

    /// The first `cap` bytes of the file (all of it when it is smaller), in pieces of 1 MiB with the job checked between them. The length
    /// comes from the opened file, and no more than `cap` bytes are ever read.
    pub fn read(&self, path: &str, cap: u64, job: &Job) -> Result<Vec<u8>> {
        let (mut file, len) = self.open_file(path)?;
        let n = len.min(cap);
        job.total(Some(n), Unit::Bytes);
        let mut out: Vec<u8> = Vec::with_capacity(n.min(64 << 20) as usize);
        let mut left = n;
        let mut buf = vec![0u8; 1 << 20];
        while left > 0 {
            job.check()?;
            let want = left.min(buf.len() as u64) as usize;
            let got = file.read(&mut buf[..want]).map_err(|e| Error::io(path, e))?;
            if got == 0 {
                break;
            }
            out.extend_from_slice(&buf[..got]);
            left -= got as u64;
            job.advance(got as u64);
        }
        Ok(out)
    }

    /// `n` bytes from `from` (fewer at the end of the file).
    pub fn read_range(&self, path: &str, from: u64, n: usize) -> Result<Vec<u8>> {
        let (mut file, len) = self.open_file(path)?;
        if from >= len {
            return Ok(Vec::new());
        }
        let n = (n as u64).min(len - from);
        file.seek(SeekFrom::Start(from)).map_err(|e| Error::io(path, e))?;
        let mut out = Vec::with_capacity(n.min(64 << 20) as usize);
        file.take(n).read_to_end(&mut out).map_err(|e| Error::io(path, e))?;
        Ok(out)
    }
}

impl LooseTree {
    /// The file opened for streaming (a copy out): checked like [`read`](Self::read) (links on the way, a file, under the read cap).
    pub fn open_reader(&self, path: &str) -> Result<std::fs::File> {
        self.open_file(path).map(|(f, _)| f)
    }
}
