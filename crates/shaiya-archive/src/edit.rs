//! Changing an archive: queued edits applied to a client folder in place (append, then swap the index; journal, marker and recovery per
//! spec section 5) or written to a new pair.

use crate::client::{self, ClientFiles};
use crate::journal::{self, Journal, JournalMode, LooseRecord, Marker, State};
use crate::{encode_name_strict, split_path, Archive, Dir, Entry, Index};
use shaiya_core::fs::{same_file, write_atomic, Existing};
use shaiya_core::hash::{sha256, to_hex, Hasher};
use shaiya_core::{Error, Job, Result, Unit};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Where new content comes from. `Range`: `length` bytes at `offset` of a file (another SAF, a patch's unpacked `update.saf`).
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Bytes(Vec<u8>),
    Range { file: PathBuf, offset: u64, length: u32 },
}

impl Source {
    pub fn len(&self) -> Result<u64> {
        match self {
            Source::File(p) => Ok(std::fs::metadata(p).map_err(|e| Error::io(p, e))?.len()),
            Source::Bytes(b) => Ok(b.len() as u64),
            Source::Range { length, .. } => Ok(*length as u64),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len().map(|n| n == 0).unwrap_or(false)
    }

    /// Streams the content into `w` (1 MiB buffer, `job.check()` between chunks, byte progress). Returns the bytes written.
    pub fn copy_to(&self, w: &mut dyn Write, job: &Job) -> Result<u64> {
        match self {
            Source::Bytes(b) => {
                for chunk in b.chunks(1 << 20) {
                    job.check()?;
                    w.write_all(chunk).map_err(Error::from)?;
                    job.advance(chunk.len() as u64);
                }
                Ok(b.len() as u64)
            }
            Source::File(p) => {
                let mut f = File::open(p).map_err(|e| Error::io(p, e))?;
                copy_stream(&mut f, u64::MAX, w, job, p)
            }
            Source::Range { file, offset, length } => {
                let mut f = File::open(file).map_err(|e| Error::io(file, e))?;
                f.seek(SeekFrom::Start(*offset)).map_err(|e| Error::io(file, e))?;
                let n = copy_stream(&mut f, *length as u64, w, job, file)?;
                if n != *length as u64 {
                    return Err(Error::format(file.display().to_string(), Some(*offset), format!("expected {length} bytes, the file ends after {n}")));
                }
                Ok(n)
            }
        }
    }
}

fn copy_stream(src: &mut dyn Read, limit: u64, w: &mut dyn Write, job: &Job, name: &Path) -> Result<u64> {
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    while total < limit {
        job.check()?;
        let want = (limit - total).min(buf.len() as u64) as usize;
        let n = src.read(&mut buf[..want]).map_err(|e| Error::io(name, e))?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n]).map_err(Error::from)?;
        total += n as u64;
        job.advance(n as u64);
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutMode {
    AddOnly,
    ReplaceOnly,
    AddOrReplace,
}

#[derive(Clone, Debug)]
pub struct Put {
    pub path: String,
    pub source: Source,
    pub version: i32,
    pub mode: PutMode,
}

/// A file for the client folder, already written next to its target (same folder) so committing it is a rename.
#[derive(Clone, Debug)]
pub struct LooseWrite {
    pub rel: String,
    pub staged: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct EditPlan {
    pub puts: Vec<Put>,
    pub removes: Vec<String>,
    pub ignore_missing: bool,
    pub loose: Vec<LooseWrite>,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LooseResult {
    pub rel: String,
    pub overwrote: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct EditReport {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub removed: Vec<String>,
    pub missing: Vec<String>,
    pub loose: Vec<LooseResult>,
    pub bytes_appended: u64,
    pub saf_len_before: u64,
    pub saf_len_after: u64,
    pub journal: Option<PathBuf>,
}

/// Why a loose file name may not be written by a patch or an edit (spec D10); `None` when it is fine. Names that would overwrite the
/// archive, toolkit files, paths that are not enclosed.
pub fn loose_name_problem(name: &str) -> Option<String> {
    let n = name.replace('\\', "/");
    if n.is_empty() || n.starts_with('/') || n.contains(':') {
        return Some(format!("{name:?} is not a path inside the client folder"));
    }
    let parts: Vec<&str> = n.split('/').collect();
    if parts.iter().any(|p| p.is_empty() || *p == ".." || *p == ".") {
        return Some(format!("{name:?} is not a path inside the client folder"));
    }
    let first = parts[0].to_ascii_lowercase();
    if first == journal::DEFAULT_DIR {
        return Some(format!("{name:?} is inside the toolkit's backup folder"));
    }
    if parts.len() == 1
        && matches!(
            first.as_str(),
            "data.sah" | "data.saf" | "data.toolkit-pending" | "data.compact-commit" | "data.swap-journal" | "data.sah.compact" | "data.saf.compact"
        )
    {
        return Some(format!("{name:?} would overwrite the archive or a toolkit file"));
    }
    if parts.iter().any(|p| p.to_ascii_lowercase().ends_with(".shaiya-tmp")) {
        return Some(format!("{name:?} has a name the toolkit uses for temporary files"));
    }
    None
}

fn display_key(path: &str) -> String {
    path.split(['/', '\\']).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("/").to_ascii_lowercase()
}

/// The core of every edit: removals first, then puts; `offset_for(put index, length)` decides where each put's bytes will be. All
/// problems are collected and reported at once.
pub(crate) fn build_new_index(old: &Index, plan: &EditPlan, offset_for: &mut dyn FnMut(usize, u64) -> u64) -> Result<(Index, EditReport)> {
    let mut problems: Vec<String> = Vec::new();
    let mut idx = old.clone();
    let mut rep = EditReport::default();
    for path in &plan.removes {
        if split_path(path).is_none() {
            problems.push(format!("remove {path}: not a valid archive path"));
            continue;
        }
        if idx.remove(path)?.is_empty() {
            rep.missing.push(path.clone());
            if !plan.ignore_missing {
                problems.push(format!("remove {path}: not in the archive (use --ignore-missing)"));
            }
        } else {
            rep.removed.push(path.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    for (i, put) in plan.puts.iter().enumerate() {
        let Some((dirs, name)) = split_path(&put.path) else {
            problems.push(format!("{}: not a valid archive path", put.path));
            continue;
        };
        if let Some(bad) = dirs.iter().chain(std::iter::once(&name)).find(|p| encode_name_strict(p).is_err()) {
            problems.push(format!("{}: the name {bad:?} cannot be written in code page 949", put.path));
            continue;
        }
        if !seen.insert(display_key(&put.path)) {
            problems.push(format!("{}: listed twice", put.path));
            continue;
        }
        let exists = idx.get(&put.path).is_some();
        match put.mode {
            PutMode::AddOnly if exists => {
                problems.push(format!("add {}: already in the archive (use --replace)", put.path));
                continue;
            }
            PutMode::ReplaceOnly if !exists => {
                problems.push(format!("replace {}: not in the archive (use --add-missing)", put.path));
                continue;
            }
            _ => {}
        }
        let len = put.source.len()?;
        if len > u32::MAX as u64 {
            problems.push(format!("{}: {len} bytes is more than an archive entry can hold (4 GiB - 1)", put.path));
            continue;
        }
        let entry = Entry { name: encode_name_strict(name)?, offset: offset_for(i, len), length: len as u32, version: put.version };
        match idx.put(&put.path, entry)? {
            Some(_) => rep.replaced.push(put.path.clone()),
            None => rep.added.push(put.path.clone()),
        }
        rep.bytes_appended += len;
    }
    for l in &plan.loose {
        if let Some(why) = loose_name_problem(&l.rel) {
            problems.push(why);
        }
        rep.loose.push(LooseResult { rel: l.rel.clone(), overwrote: false });
    }
    if !problems.is_empty() {
        return Err(Error::Refused(if problems.len() == 1 { problems.remove(0) } else { format!("{} problems:\n  {}", problems.len(), problems.join("\n  ")) }));
    }
    Ok((idx, rep))
}

/// The index after the plan with sequential offsets from `base` (where the first put would land) and the report; for planning several
/// patches in a row. Same checks as [`preview`].
pub fn simulate(index: &Index, plan: &EditPlan, base: u64) -> Result<(Index, EditReport)> {
    let mut at = base;
    build_new_index(index, plan, &mut |_, len| {
        let o = at;
        at += len;
        o
    })
}

/// Validates without writing: paths (`split_path` + `encode_name_strict`), `AddOnly` on an existing visible path, `ReplaceOnly` on a
/// missing one, removals of missing paths unless `ignore_missing` -> `Error::Refused` listing every offending path (all at once, not the
/// first). Removals are applied before puts, as in the original patch tool.
pub fn preview(index: &Index, plan: &EditPlan) -> Result<EditReport> {
    let mut at = 0u64;
    let (_, rep) = build_new_index(index, plan, &mut |_, len| {
        let o = at;
        at += len;
        o
    })?;
    Ok(rep)
}

#[derive(Clone, Debug)]
pub struct InPlaceOptions {
    pub journal: JournalMode,
    pub legacy_bak: bool,
    pub check_in_use: bool,
}

fn rel_in(client: &Path, p: &Path) -> Result<String> {
    let abs = |x: &Path| std::path::absolute(x).unwrap_or_else(|_| x.to_path_buf());
    let (c, p) = (abs(client), abs(p));
    p.strip_prefix(&c)
        .map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
        .map_err(|_| Error::Refused(format!("{} is not inside the client folder {}", p.display(), c.display())))
}

fn join_rel(dir: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|p| !p.is_empty()).fold(dir.to_path_buf(), |p, s| p.join(s))
}

/// Undo of a failed or cancelled change before the index was swapped.
fn abandon(client: &Path, files: &ClientFiles, saf_len: u64, journal: Option<&Path>) {
    if let Ok(f) = OpenOptions::new().write(true).open(&files.saf) {
        let _ = f.set_len(saf_len);
        let _ = f.sync_all();
    }
    Marker::remove(client);
    if let Some(j) = journal {
        let _ = std::fs::remove_dir_all(j);
    }
}

/// Section 5.2's procedure on a client folder's `data.sah`/`data.saf`.
pub fn apply_in_place(client: &Path, plan: &EditPlan, opts: &InPlaceOptions, job: &Job) -> Result<EditReport> {
    client::recover(client)?;
    let files = ClientFiles::locate(client)?;
    if opts.check_in_use {
        client::ensure_not_in_use(&files)?;
    }
    job.stage("reading index");
    let sah_bytes = std::fs::read(&files.sah).map_err(|e| Error::io(&files.sah, e))?;
    let index = Index::parse_named(&sah_bytes, &files.sah.display().to_string())?;
    let saf_len = std::fs::metadata(&files.saf).map_err(|e| Error::io(&files.saf, e))?.len();
    let mut next = saf_len;
    let mut offsets: Vec<u64> = vec![0; plan.puts.len()];
    let (new_index, mut report) = build_new_index(&index, plan, &mut |i, len| {
        let o = next;
        next += len;
        offsets[i] = o;
        o
    })?;
    for l in &plan.loose {
        if !l.staged.is_file() {
            return Err(Error::Refused(format!("the staged file {} for {} is missing", l.staged.display(), l.rel)));
        }
    }
    let staged: Vec<(String, String)> = plan.loose.iter().map(|l| Ok((rel_in(client, &l.staged)?, l.rel.replace('\\', "/")))).collect::<Result<_>>()?;
    let new_bytes = new_index.to_bytes();
    let (sah_before, sah_after) = (to_hex(&sha256(&sah_bytes)), to_hex(&sha256(&new_bytes)));
    report.saf_len_before = saf_len;
    report.saf_len_after = saf_len + report.bytes_appended;

    // 2. the journal
    let label = if plan.label.is_empty() { "archive edit".to_string() } else { plan.label.clone() };
    let mut journal: Option<Journal> = None;
    if let JournalMode::In(root) = &opts.journal {
        let dir = journal::create_dir(root, &label)?;
        let made = (|| -> Result<Journal> {
            let kind = if label.to_ascii_lowercase().ends_with(".patch") { "patch" } else { "archive-edit" };
            let mut j = journal::new_journal(&dir, kind, &label, &files, &sah_before, &sah_after, saf_len, report.saf_len_after);
            std::fs::write(j.sah_copy(), &sah_bytes).map_err(|e| Error::io(j.sah_copy(), e))?;
            for l in &plan.loose {
                let target = join_rel(client, &l.rel);
                let existed = target.is_file();
                let backup = if existed {
                    let rel = format!("loose/{}", l.rel.replace('\\', "/"));
                    let dest = join_rel(&dir, &rel);
                    if let Some(p) = dest.parent() {
                        std::fs::create_dir_all(p).map_err(|e| Error::io(p, e))?;
                    }
                    std::fs::copy(&target, &dest).map_err(|e| Error::io(&dest, e))?;
                    Some(rel)
                } else {
                    None
                };
                if let Some(r) = report.loose.iter_mut().find(|r| r.rel == l.rel) {
                    r.overwrote = existed;
                }
                j.loose.push(LooseRecord { rel: l.rel.replace('\\', "/"), existed, backup, sha256_after: None });
            }
            j.write()?;
            Ok(j)
        })();
        match made {
            Ok(j) => journal = Some(j),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        }
    } else {
        for l in &plan.loose {
            if let Some(r) = report.loose.iter_mut().find(|r| r.rel == l.rel) {
                r.overwrote = join_rel(client, &l.rel).is_file();
            }
        }
    }
    let jdir = journal.as_ref().map(|j| j.dir.clone());
    report.journal = jdir.clone();

    // 3. the pending marker
    let marker = Marker {
        op: "apply".into(),
        label: label.clone(),
        saf_len_before: saf_len,
        sah_sha256_before: sah_before.clone(),
        sah_sha256_after: sah_after.clone(),
        journal: jdir.as_ref().map(|d| std::path::absolute(d).unwrap_or_else(|_| d.clone())),
        staged: staged.clone(),
    };
    if let Err(e) = marker.write(client) {
        abandon(client, &files, saf_len, jdir.as_deref());
        return Err(e);
    }

    // 4. append
    let appended = (|| -> Result<()> {
        job.stage("appending");
        job.total(Some(report.bytes_appended), Unit::Bytes);
        let mut saf = OpenOptions::new().write(true).open(&files.saf).map_err(|e| Error::io(&files.saf, e))?;
        let end = saf.seek(SeekFrom::End(0)).map_err(|e| Error::io(&files.saf, e))?;
        if end != saf_len {
            return Err(Error::Refused("data.saf changed size while the change was being prepared".to_string()));
        }
        {
            let mut w = BufWriter::with_capacity(1 << 20, &mut saf);
            for (i, put) in plan.puts.iter().enumerate() {
                // puts that were refused never reach here; every put in the plan has an offset
                let want = put.source.len()?;
                let n = put.source.copy_to(&mut w, job)?;
                if n != want {
                    return Err(Error::format(put.path.clone(), Some(offsets[i]), format!("the source changed while it was copied ({want} bytes expected, {n} read)")));
                }
            }
            w.flush().map_err(|e| Error::io(&files.saf, e))?;
        }
        saf.sync_all().map_err(|e| Error::io(&files.saf, e))?;
        Ok(())
    })();
    if let Err(e) = appended {
        abandon(client, &files, saf_len, jdir.as_deref());
        return Err(e);
    }

    // 5. the index (not cancellable from here)
    let swapped = (|| -> Result<()> {
        if sah_before == sah_after {
            return Ok(()); // loose files only: the index is unchanged, leave it (and its backup) alone
        }
        if opts.legacy_bak {
            let mut bak = files.sah.clone().into_os_string();
            bak.push(".bak");
            std::fs::copy(&files.sah, PathBuf::from(&bak)).map_err(|e| Error::io(&files.sah, e))?;
        }
        job.stage("writing index");
        write_atomic(&files.sah, &new_bytes)
    })();
    if let Err(e) = swapped {
        abandon(client, &files, saf_len, jdir.as_deref());
        return Err(e);
    }

    // 6. loose files, 7. the journal and the marker
    let done = journal::commit_staged(client, &staged)?;
    if let Some(mut j) = journal {
        j.state = State::Applied;
        for rec in &mut j.loose {
            if let Some((_, h)) = done.iter().find(|(r, _)| r == &rec.rel) {
                rec.sha256_after = Some(h.clone());
            }
        }
        j.write()?;
    }
    Marker::remove(client);
    Ok(report)
}

const SENTINEL: u64 = 1 << 62;

fn walk_mut(d: &mut Dir, f: &mut dyn FnMut(&mut Entry, &str) -> Result<()>, prefix: &str) -> Result<()> {
    let here = if d.name.is_empty() { prefix.to_string() } else { format!("{prefix}{}/", crate::decode_name(&d.name)) };
    for e in &mut d.files {
        let p = format!("{here}{}", crate::decode_name(&e.name));
        f(e, &p)?;
    }
    for c in &mut d.dirs {
        walk_mut(c, f, &here)?;
    }
    Ok(())
}

pub fn temp_next_to(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    path.with_file_name(format!(".{name}.{}.shaiya-tmp", std::process::id()))
}

/// Where the pair for `out_sah` goes: `(sah, saf)` with the same stem.
pub(crate) fn pair_for(out_sah: &Path) -> (PathBuf, PathBuf) {
    (out_sah.to_path_buf(), out_sah.with_extension("saf"))
}

/// Refuses outputs that are the inputs or exist (unless overwriting).
pub(crate) fn check_outputs(inputs: &[&Path], outs: &[&Path], existing: Existing) -> Result<()> {
    for o in outs {
        for i in inputs {
            if same_file(i, o) {
                return Err(Error::Refused(format!("the output {} is the input; a new archive never overwrites its input", o.display())));
            }
        }
    }
    if existing != Existing::Overwrite {
        for o in outs {
            if o.exists() {
                return Err(Error::Refused(format!("{} already exists; use --force to overwrite it", o.display())));
            }
        }
    }
    Ok(())
}

/// Writes a complete new pair (`out_sah` + sibling `.saf` with the same stem) holding the edited archive: shadowed entries and the free
/// list dropped, contents back to back in the original index order, trailer and SAH version kept; then re-reads every written entry
/// range and compares it with its source hash. The input pair is only read. Temp names, renamed at the end.
pub fn write_new(archive: &Archive, plan: &EditPlan, out_sah: &Path, existing: Existing, job: &Job) -> Result<EditReport> {
    let (out_sah, out_saf) = pair_for(out_sah);
    check_outputs(&[archive.sah_path(), archive.saf_path()], &[&out_sah, &out_saf], existing)?;
    let mut base = archive.index().clone();
    base.dedupe();
    base.free.clear();
    let (mut idx, mut report) = build_new_index(&base, plan, &mut |i, _| SENTINEL + i as u64)?;
    report.saf_len_before = archive.saf_len();
    let mut total = 0u64;
    {
        let mut sizes = Vec::new();
        walk_mut(&mut idx.root, &mut |e, _| {
            sizes.push(e.length as u64);
            Ok(())
        }, "")?;
        total += sizes.iter().sum::<u64>();
    }
    let (tmp_sah, tmp_saf) = (temp_next_to(&out_sah), temp_next_to(&out_saf));
    let result = (|| -> Result<(u64, Vec<(u64, u32, [u8; 32], String)>)> {
        job.stage("copying");
        job.total(Some(total), Unit::Bytes);
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp_saf).map_err(|e| Error::io(&tmp_saf, e))?);
        let mut pos = 0u64;
        let mut written: Vec<(u64, u32, [u8; 32], String)> = Vec::new();
        walk_mut(&mut idx.root, &mut |e, path| {
            job.check()?;
            let src_off = e.offset;
            let mut hw = HashWriter { inner: &mut w, hasher: Hasher::new() };
            let n = if src_off >= SENTINEL {
                let put = &plan.puts[(src_off - SENTINEL) as usize];
                put.source.copy_to(&mut hw, job)?
            } else if e.length == 0 {
                0
            } else {
                let mut r = archive.reader(e)?;
                copy_stream(&mut r, e.length as u64, &mut hw, job, archive.saf_path())?
            };
            if n != e.length as u64 {
                return Err(Error::format(path.to_string(), Some(src_off), format!("expected {} bytes, copied {n}", e.length)));
            }
            let digest = hw.hasher.finish();
            written.push((pos, e.length, digest, path.to_string()));
            e.offset = pos;
            pos += n;
            Ok(())
        }, "")?;
        w.into_inner().map_err(|e| Error::io(&tmp_saf, e.into_error()))?.sync_all().map_err(|e| Error::io(&tmp_saf, e))?;
        Ok((pos, written))
    })();
    let verify_and_commit = (|| -> Result<u64> {
        let (len, written) = result?;
        // verification: re-read every written range and compare with the hash taken while writing
        job.stage("verifying");
        job.total(Some(len), Unit::Bytes);
        let mut f = File::open(&tmp_saf).map_err(|e| Error::io(&tmp_saf, e))?;
        let mut buf = vec![0u8; 1 << 20];
        for (off, length, digest, path) in &written {
            f.seek(SeekFrom::Start(*off)).map_err(|e| Error::io(&tmp_saf, e))?;
            let mut h = Hasher::new();
            let mut left = *length as u64;
            while left > 0 {
                job.check()?;
                let n = left.min(buf.len() as u64) as usize;
                f.read_exact(&mut buf[..n]).map_err(|e| Error::io(&tmp_saf, e))?;
                h.update(&buf[..n]);
                left -= n as u64;
                job.advance(n as u64);
            }
            if &h.finish() != digest {
                return Err(Error::format(path.clone(), Some(*off), "verification failed: the written data differs from its source"));
            }
        }
        drop(f);
        let bytes = idx.to_bytes();
        let mut sf = File::create(&tmp_sah).map_err(|e| Error::io(&tmp_sah, e))?;
        sf.write_all(&bytes).and_then(|_| sf.sync_all()).map_err(|e| Error::io(&tmp_sah, e))?;
        drop(sf);
        std::fs::rename(&tmp_saf, &out_saf).map_err(|e| Error::io(&out_saf, e))?;
        std::fs::rename(&tmp_sah, &out_sah).map_err(|e| Error::io(&out_sah, e))?;
        Ok(len)
    })();
    match verify_and_commit {
        Ok(len) => {
            report.saf_len_after = len;
            report.bytes_appended = len;
            Ok(report)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_saf);
            let _ = std::fs::remove_file(&tmp_sah);
            Err(e)
        }
    }
}

/// Writer that hashes what passes through.
pub(crate) struct HashWriter<'a, W: Write> {
    pub inner: &'a mut W,
    pub hasher: Hasher,
}

impl<W: Write> Write for HashWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

