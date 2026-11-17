//! `psNNNN.patch`: a ZIP (deflate) holding `update.sah` + `update.saf` (an archive of files to merge into `data.sah/saf`), optionally
//! `remove.txt` (archive paths to drop, one per line), and loose files that are copied into the client folder (e.g. `game.exe`).
//!
//! Compaction is in `shaiya_archive::compact`. Also here: streaming reproducible builds, a dry-run [`plan`], [`apply_with`] over `shaiya_archive::edit::apply_in_place` (journal, rollback,
//! in-use check), [`verify`] and signature file helpers ([`sign`]).

pub mod sign;

use shaiya_archive::client::{self, ClientFiles};
use shaiya_archive::diff::{diff, Change, DiffOptions, Side};
use shaiya_archive::edit::{self, loose_name_problem, EditPlan, InPlaceOptions, LooseWrite, Put, PutMode, Source};
use shaiya_archive::journal::JournalMode;
use shaiya_archive::{encode_name_strict, split_path, Archive, Entry, Index};
use shaiya_core::fs::{write_atomic_from_with, Existing, Sync};
use shaiya_core::glob::{parse_list, Filter};
use shaiya_core::hash::{sha256_file, to_hex, Hasher};
use shaiya_core::{Error, Job, Unit};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub use shaiya_core::Result;

const SAH: &str = "update.sah";
const SAF: &str = "update.saf";
const REMOVE: &str = "remove.txt";

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Report {
    /// Archive files added or replaced, as paths.
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    /// Archive paths dropped by `remove.txt`, and the listed ones that were not in this client's archive.
    pub removed: Vec<String>,
    pub missing: Vec<String>,
    /// Loose files written to the client folder (with ".new" appended when the target was protected).
    pub loose: Vec<String>,
    pub bytes: u64,
    /// The journal folder of this change, when one was made.
    pub journal: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PatchFile {
    pub path: String,
    pub length: u32,
    pub version: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Contents {
    pub sah_version: i32,
    pub files: Vec<PatchFile>,
    /// `(name, size)`.
    pub loose: Vec<(String, u64)>,
    pub removals: Vec<String>,
    pub zip_size: u64,
    pub unpacked_size: u64,
    pub signature_file: bool,
}

fn zip_err(e: zip::result::ZipError) -> Error {
    match e {
        zip::result::ZipError::Io(e) => Error::Io { path: None, source: e },
        other => Error::format("patch", None, format!("zip: {other}")),
    }
}

fn open_zip(patch: &Path) -> Result<ZipArchive<File>> {
    let f = File::open(patch).map_err(|e| Error::io(patch, e))?;
    ZipArchive::new(f).map_err(|e| match e {
        zip::result::ZipError::Io(e) => Error::io(patch, e),
        other => Error::format(patch.display().to_string(), None, format!("not a valid patch: {other}")),
    })
}

fn io_at<T>(path: &Path, r: io::Result<T>) -> Result<T> {
    r.map_err(|e| Error::io(path, e))
}

fn reserved(name: &str) -> bool {
    [SAH, SAF, REMOVE].iter().any(|r| name.eq_ignore_ascii_case(r))
}

fn find(z: &mut ZipArchive<File>, name: &str) -> Option<usize> {
    (0..z.len()).find(|&i| z.name_for_index(i).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn read_index(z: &mut ZipArchive<File>) -> Result<Index> {
    let i = find(z, SAH).ok_or_else(|| Error::format("patch", None, "patch has no update.sah"))?;
    // Real indexes are under 1 MB; cap what is read from the ZIP stream.
    let mut buf = Vec::new();
    z.by_index(i).map_err(zip_err)?.take(64 << 20).read_to_end(&mut buf)?;
    Index::parse_named(&buf, SAH)
}

fn read_removals(z: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let Some(i) = find(z, REMOVE) else { return Ok(Vec::new()) };
    let mut text = String::new();
    z.by_index(i).map_err(zip_err)?.take(16 << 20).read_to_string(&mut text)?;
    Ok(parse_list(&text))
}

/// `(index, name, size)` of the loose files (everything but folders and the three reserved names).
fn loose_entries(z: &mut ZipArchive<File>) -> Vec<(usize, String, u64)> {
    (0..z.len())
        .filter_map(|i| z.by_index_raw(i).ok().map(|f| (i, f.name().to_string(), f.size(), f.is_dir())))
        .filter(|(_, n, _, dir)| !dir && !n.ends_with('/') && !reserved(n))
        .map(|(i, n, s, _)| (i, n, s))
        .collect()
}

/// `<patch>.sig`.
pub fn signature_path(patch: &Path) -> PathBuf {
    sign::signature_path(patch)
}

/// `"ps0109.patch"` -> 109 (case-insensitive).
pub fn patch_number(file_name: &str) -> Option<u32> {
    let l = file_name.to_ascii_lowercase();
    let digits = l.strip_prefix("ps")?.strip_suffix(".patch")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 109 -> `"ps0109.patch"`.
pub fn patch_name(number: u32) -> String {
    format!("ps{number:04}.patch")
}

/// List a patch: (archive paths with their lengths, loose file names).
pub fn list(patch: &Path) -> Result<(Vec<(String, u32)>, Vec<String>)> {
    let c = contents(patch)?;
    Ok((c.files.into_iter().map(|f| (f.path, f.length)).collect(), c.loose.into_iter().map(|(n, _)| n).collect()))
}

/// The archive paths a patch removes (`remove.txt`; none when it has no such file).
pub fn removals(patch: &Path) -> Result<Vec<String>> {
    read_removals(&mut open_zip(patch)?)
}

/// Everything in a patch, read from its index and ZIP directory (no unpacking).
pub fn contents(patch: &Path) -> Result<Contents> {
    let mut z = open_zip(patch)?;
    let idx = read_index(&mut z)?;
    let files = idx.files().into_iter().map(|(path, e)| PatchFile { path, length: e.length, version: e.version }).collect();
    let loose = loose_entries(&mut z).into_iter().map(|(_, n, s)| (n, s)).collect();
    let unpacked_size = (0..z.len()).filter_map(|i| z.by_index_raw(i).ok().map(|f| f.size())).sum();
    let zip_size = io_at(patch, fs::metadata(patch))?.len();
    Ok(Contents { sah_version: idx.version, files, loose, removals: read_removals(&mut z)?, zip_size, unpacked_size, signature_file: signature_path(patch).is_file() })
}

// ------------------------------------------------------------------------------------------------ loose file names

/// Loose names of a ZIP: unsafe paths are `Error::Format` (as always); toolkit and archive names and case twins are `Error::Refused`.
fn checked_loose(patch: &Path, z: &mut ZipArchive<File>) -> Result<Vec<(usize, String, PathBuf)>> {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(zip_err)?;
        let name = f.name().to_string();
        if f.is_dir() || reserved(&name) {
            continue;
        }
        let Some(rel) = f.enclosed_name() else {
            return Err(Error::format(patch.display().to_string(), None, format!("unsafe path in patch: {name:?}")));
        };
        if let Some(why) = loose_name_problem(&name) {
            problems.push(why);
            continue;
        }
        let key = name.replace('\\', "/").to_lowercase();
        if let Some(other) = seen.insert(key, name.clone()) {
            problems.push(format!("{name:?} and {other:?} differ only in letter case"));
            continue;
        }
        out.push((i, name, rel));
    }
    if !problems.is_empty() {
        return Err(Error::Refused(format!("the patch holds loose files that may not be written:\n  {}", problems.join("\n  "))));
    }
    Ok(out)
}

// ------------------------------------------------------------------------------------------------ plan

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct LoosePlan {
    pub name: String,
    /// Where it will be written (the name, or `<name>.new` when protected).
    pub target: String,
    pub overwrites: bool,
    pub protected: bool,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Plan {
    pub patch: PathBuf,
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub removed: Vec<String>,
    pub missing: Vec<String>,
    pub loose: Vec<LoosePlan>,
    /// Appended to `data.saf`.
    pub bytes: u64,
    /// The unpacked `update.saf` (temporary space).
    pub temp_bytes: u64,
}

fn edit_plan_of(patch: &Path, z: &mut ZipArchive<File>, idx: &Index) -> Result<EditPlan> {
    let puts = idx
        .files()
        .into_iter()
        .map(|(path, e)| Put { path, source: Source::Range { file: patch.to_path_buf(), offset: e.offset, length: e.length }, version: e.version, mode: PutMode::AddOrReplace })
        .collect();
    Ok(EditPlan { puts, removes: read_removals(z)?, ignore_missing: true, loose: Vec::new(), label: patch.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() })
}

fn needed_of(idx: &Index) -> u64 {
    idx.files().iter().map(|(_, e)| e.offset + e.length as u64).max().unwrap_or(0)
}

/// Several patches in order, each planned as if the earlier ones were applied.
pub fn plan_seq(patches: &[PathBuf], client: &Path, protect: &[String]) -> Result<Vec<Plan>> {
    let files = ClientFiles::locate(client)?;
    let mut idx = Index::read(&files.sah)?;
    let mut written: HashSet<String> = HashSet::new();
    let mut plans = Vec::new();
    for patch in patches {
        let mut z = open_zip(patch)?;
        let pidx = read_index(&mut z)?;
        let loose = checked_loose(patch, &mut z)?;
        let ep = edit_plan_of(patch, &mut z, &pidx)?;
        let (next, rep) = edit::simulate(&idx, &ep, 0)?;
        let mut lp = Vec::new();
        for (i, name, _) in &loose {
            let protected = protect.iter().any(|p| p.eq_ignore_ascii_case(name));
            let target = if protected { format!("{name}.new") } else { name.clone() };
            let size = z.by_index_raw(*i).map_err(zip_err)?.size();
            let on_disk = target.replace('\\', "/").split('/').fold(client.to_path_buf(), |p, s| p.join(s)).exists();
            lp.push(LoosePlan { name: name.clone(), target: target.clone(), overwrites: on_disk || written.contains(&target.to_lowercase()), protected, size });
        }
        written.extend(lp.iter().map(|l| l.target.to_lowercase()));
        plans.push(Plan {
            patch: patch.clone(),
            added: rep.added,
            replaced: rep.replaced,
            removed: rep.removed,
            missing: rep.missing,
            loose: lp,
            bytes: rep.bytes_appended,
            temp_bytes: needed_of(&pidx),
        });
        idx = next;
    }
    Ok(plans)
}

/// Dry run against the client as it is now.
pub fn plan(patch: &Path, client: &Path, protect: &[String]) -> Result<Plan> {
    Ok(plan_seq(&[patch.to_path_buf()], client, protect)?.remove(0))
}

// ------------------------------------------------------------------------------------------------ apply

#[derive(Clone, Debug)]
pub struct VerifyKey {
    pub public_hex: String,
    pub check_version: u32,
}

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub protect: Vec<String>,
    pub journal: JournalMode,
    pub legacy_bak: bool,
    /// The patch's `<patch>.sig` must verify before anything is written, else `Error::Refused`.
    pub require_signature: Option<VerifyKey>,
    /// Refuse while the game or another program holds the archive files (the updater passes `false`: it checks the game itself).
    pub check_in_use: bool,
}

fn temp_saf_name() -> String {
    format!(".update.saf.{}.shaiya-tmp", std::process::id())
}

/// Unpacks `update.saf` to a temp file in the client folder (`.update.saf.<pid>.shaiya-tmp`, never more than the index refers to), stages
/// loose files next to their targets (protected names as `<name>.new`), then `shaiya_archive::edit::apply_in_place`. Temps are removed
/// on every path out.
pub fn apply_with(patch: &Path, client: &Path, opts: &ApplyOptions, job: &Job) -> Result<Report> {
    // the signature first: nothing is read from the client when it fails
    if let Some(k) = &opts.require_signature {
        sign::verify_file(&k.public_hex, k.check_version, patch, None, job).map_err(|e| match e {
            Error::Cancelled => Error::Cancelled,
            Error::Refused(m) => Error::Refused(m),
            other => Error::Refused(format!("signature check failed for {}: {other}", patch.display())),
        })?;
    }
    client::recover(client)?;
    let files = ClientFiles::locate(client)?;
    if opts.check_in_use {
        client::ensure_not_in_use(&files)?;
    }
    job.stage("unpacking");
    let mut z = open_zip(patch)?;
    let pidx = read_index(&mut z)?;
    let loose = checked_loose(patch, &mut z)?;
    let mut ep = edit_plan_of(patch, &mut z, &pidx)?;
    // everything is validated against the client before anything is unpacked
    let current = Index::read(&files.sah)?;
    edit::simulate(&current, &ep, 0)?;
    let tmp_saf = client.join(temp_saf_name());
    let mut staged: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<Report> {
        if !pidx.files().is_empty() {
            let i = find(&mut z, SAF).ok_or_else(|| Error::format("patch", None, "patch has no update.saf"))?;
            let needed = needed_of(&pidx);
            let entry = z.by_index(i).map_err(zip_err)?;
            if entry.size() < needed {
                return Err(Error::format("update.saf", Some(entry.size()), "update.saf is shorter than update.sah says"));
            }
            job.total(Some(needed), Unit::Bytes);
            let mut cap = entry.take(needed);
            let n = write_atomic_from_with(&tmp_saf, &mut cap, Sync::No, job)?;
            if n != needed {
                return Err(Error::format("update.saf", Some(n), "update.saf is truncated"));
            }
            for p in &mut ep.puts {
                if let Source::Range { file, .. } = &mut p.source {
                    *file = tmp_saf.clone();
                }
            }
        }
        job.stage("staging");
        let mut written_names = Vec::new();
        for (i, name, rel) in &loose {
            job.check()?;
            let protected = opts.protect.iter().any(|p| p.eq_ignore_ascii_case(name));
            let mut target_rel = rel.to_string_lossy().replace('\\', "/");
            if protected {
                target_rel.push_str(".new");
            }
            let target = target_rel.split('/').fold(client.to_path_buf(), |p, s| p.join(s));
            if let Some(parent) = target.parent() {
                io_at(parent, fs::create_dir_all(parent))?;
            }
            let tname = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let st = target.with_file_name(format!(".{tname}.{}.shaiya-tmp", std::process::id()));
            let mut f = z.by_index(*i).map_err(zip_err)?;
            write_atomic_from_with(&st, &mut f, Sync::Yes, job)?;
            staged.push(st.clone());
            ep.loose.push(LooseWrite { rel: target_rel.clone(), staged: st });
            written_names.push(target_rel);
        }
        let rep = edit::apply_in_place(client, &ep, &InPlaceOptions { journal: opts.journal.clone(), legacy_bak: opts.legacy_bak, check_in_use: false }, job)?;
        Ok(Report { added: rep.added, replaced: rep.replaced, removed: rep.removed, missing: rep.missing, loose: written_names, bytes: rep.bytes_appended, journal: rep.journal })
    })();
    let _ = fs::remove_file(&tmp_saf);
    for s in &staged {
        let _ = fs::remove_file(s);
    }
    result
}

/// Merge a patch into `client/data.sah` + `client/data.saf`, then write its loose files: the original call, kept for the updater. Writes
/// `data.sah.bak`, makes no journal, checks no signature and does not look for a running game.
///
/// `protect`: loose file names (case-insensitive) that must not be overwritten, e.g. the running updater; they are written as
/// `<name>.new` instead.
pub fn apply(patch: &Path, client: &Path, protect: &[String]) -> Result<Report> {
    apply_with(
        patch,
        client,
        &ApplyOptions { protect: protect.to_vec(), journal: JournalMode::None, legacy_bak: true, require_signature: None, check_in_use: false },
        &Job::silent(),
    )
}

// ------------------------------------------------------------------------------------------------ build

#[derive(Clone, Debug, Default)]
pub struct BuildInput {
    /// Archive path, content, entry version.
    pub files: Vec<(String, Source, i32)>,
    /// Client-relative name, file.
    pub loose: Vec<(String, PathBuf)>,
    pub removals: Vec<String>,
}

/// Files under `root` as ("a/b/c", full path), sorted for reproducible output.
fn walk(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in io_at(&dir, fs::read_dir(&dir))? {
            let p = io_at(&dir, e)?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn mtime(path: &Path) -> i32 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs().min(i32::MAX as u64) as i32)
        .unwrap_or(0)
}

impl BuildInput {
    /// `files/` mirrors archive paths, `loose/` holds client files, `remove` is a path list. Versions from file times or `timestamp`.
    pub fn from_folders(files: Option<&Path>, loose: Option<&Path>, remove: Option<&Path>, timestamp: Option<i32>) -> Result<BuildInput> {
        let mut input = BuildInput::default();
        if let Some(root) = files {
            for (rel, full) in walk(root)? {
                let v = timestamp.unwrap_or_else(|| mtime(&full));
                input.files.push((rel, Source::File(full), v));
            }
        }
        if let Some(root) = loose {
            input.loose = walk(root)?;
        }
        if let Some(list) = remove {
            input.removals = parse_list(&io_at(list, fs::read_to_string(list))?);
        }
        Ok(input)
    }

    /// `Added` and `Different` visible entries of `new` (content compare) become files (`Source::Range` into `new`'s SAF, `new`'s
    /// versions); `Removed` ones become removals. Both must be full archives.
    pub fn from_diff(old: &Archive, new: &Archive, filter: &Filter, job: &Job) -> Result<BuildInput> {
        let opts = DiffOptions { content: true, filter: filter.clone(), include_shadowed: false };
        let d = diff(&Side::Archive(old), &Side::Archive(new), &opts, job)?;
        let mut input = BuildInput::default();
        for e in d {
            match e.change {
                Change::Added | Change::Different => {
                    let n = e.new.expect("added and different entries have a new side");
                    input.files.push((e.path, Source::Range { file: new.saf_path().to_path_buf(), offset: n.offset, length: n.length }, n.version));
                }
                Change::Removed => input.removals.push(e.path),
                Change::Same | Change::Unknown => {}
            }
        }
        Ok(input)
    }
}

/// Everything wrong with a build input, all at once (the loose-name rules and the archive rules).
pub fn input_problems(input: &BuildInput) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen: HashMap<String, String> = HashMap::new();
    for (path, src, _) in &input.files {
        let Some((dirs, name)) = split_path(path) else {
            problems.push(format!("{path}: not a valid archive path"));
            continue;
        };
        if let Some(bad) = dirs.iter().chain(std::iter::once(&name)).find(|p| encode_name_strict(p).is_err()) {
            problems.push(format!("{path}: the name {bad:?} cannot be written in code page 949"));
            continue;
        }
        let key = path.replace('\\', "/").trim_matches('/').to_ascii_lowercase();
        if let Some(other) = seen.insert(key, path.clone()) {
            problems.push(format!("{path} and {other} are the same file in the archive (case is ignored)"));
        }
        match src.len() {
            Ok(n) if n > u32::MAX as u64 => problems.push(format!("{path}: {n} bytes is more than an archive entry can hold (4 GiB - 1)")),
            Ok(_) => {}
            Err(e) => problems.push(format!("{path}: {e}")),
        }
    }
    let mut lseen: HashMap<String, String> = HashMap::new();
    for (name, _) in &input.loose {
        if reserved(name) {
            problems.push(format!("loose file {name:?} has a name the patch format reserves"));
        }
        if let Some(why) = loose_name_problem(name) {
            problems.push(why);
        }
        if let Some(other) = lseen.insert(name.replace('\\', "/").to_lowercase(), name.clone()) {
            problems.push(format!("loose files {name:?} and {other:?} differ only in letter case"));
        }
    }
    for p in &input.removals {
        if split_path(p).is_none() {
            problems.push(format!("bad path in the removal list: {p:?}"));
        }
    }
    problems
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub existing: Existing,
    /// `(secret key hex, check version)`: writes `<out>.sig` after the build.
    pub sign: Option<(String, u32)>,
}

fn zip_options(large: bool) -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).last_modified_time(zip::DateTime::default()).large_file(large)
}

/// Writes to a temp name next to `out`, renames at the end. `update.sah` is computed from file sizes first, then file contents stream
/// into `update.saf`; ZIP entry times are fixed at 1980-01-01, so the same inputs give the same bytes. `Report.added` lists every
/// archive file.
pub fn build_with(input: &BuildInput, out: &Path, opts: &BuildOptions, job: &Job) -> Result<Report> {
    let problems = input_problems(input);
    if !problems.is_empty() {
        return Err(Error::Refused(if problems.len() == 1 { problems[0].clone() } else { format!("{} problems:\n  {}", problems.len(), problems.join("\n  ")) }));
    }
    if out.exists() && opts.existing != Existing::Overwrite {
        return Err(Error::Refused(format!("{} already exists; use --force to overwrite it", out.display())));
    }
    let sig_path = signature_path(out);
    if opts.sign.is_some() && sig_path.exists() && opts.existing != Existing::Overwrite {
        return Err(Error::Refused(format!("{} already exists; use --force to replace it (never re-sign a published patch number)", sig_path.display())));
    }
    job.stage("packing");
    // the index first, from the sizes
    let mut report = Report::default();
    let mut idx = Index::new();
    let mut sizes = Vec::with_capacity(input.files.len());
    let mut pos = 0u64;
    for (path, src, version) in &input.files {
        let len = src.len()?;
        let (_, name) = split_path(path).ok_or_else(|| Error::format(path.clone(), None, "bad path"))?;
        idx.put(path, Entry { name: encode_name_strict(name)?, offset: pos, length: len as u32, version: *version })?;
        sizes.push(len);
        pos += len;
        report.added.push(path.clone());
    }
    report.bytes = pos;
    let loose_total: u64 = input.loose.iter().map(|(_, p)| fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum();
    job.total(Some(pos + loose_total), Unit::Bytes);
    let tmp = edit::temp_next_to(out);
    let built = (|| -> Result<()> {
        let mut z = ZipWriter::new(io_at(&tmp, File::create(&tmp))?);
        z.start_file(SAH, zip_options(false)).map_err(zip_err)?;
        z.write_all(&idx.to_bytes())?;
        z.start_file(SAF, zip_options(pos > u32::MAX as u64 - 1)).map_err(zip_err)?;
        for ((path, src, _), want) in input.files.iter().zip(&sizes) {
            job.check()?;
            let n = src.copy_to(&mut z, job)?;
            if n != *want {
                return Err(Error::format(path.clone(), None, format!("the file changed while the patch was built ({want} bytes expected, {n} read)")));
            }
        }
        if !input.removals.is_empty() {
            z.start_file(REMOVE, zip_options(false)).map_err(zip_err)?;
            z.write_all(input.removals.join("\n").as_bytes())?;
            report.removed = input.removals.clone();
        }
        let mut loose: Vec<&(String, PathBuf)> = input.loose.iter().collect();
        loose.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in loose {
            job.check()?;
            let len = io_at(path, fs::metadata(path))?.len();
            z.start_file(name.replace('\\', "/"), zip_options(len > u32::MAX as u64 - 1)).map_err(zip_err)?;
            Source::File(path.clone()).copy_to(&mut z, job)?;
            report.loose.push(name.clone());
        }
        let f = z.finish().map_err(zip_err)?;
        io_at(&tmp, f.sync_all())?;
        drop(f);
        io_at(out, fs::rename(&tmp, out))
    })();
    if let Err(e) = built {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some((secret, cv)) = &opts.sign {
        sign::sign_file(secret, *cv, out, None, Existing::Overwrite, job)?;
    }
    Ok(report)
}

/// The original call: `from_folders(.., None)` + `build_with(Existing::Overwrite, no signing)`.
pub fn build(files: Option<&Path>, loose: Option<&Path>, remove: Option<&Path>, out: &Path) -> Result<Report> {
    let input = BuildInput::from_folders(files, loose, remove, None)?;
    build_with(&input, out, &BuildOptions { existing: Existing::Overwrite, sign: None }, &Job::silent())
}

// ------------------------------------------------------------------------------------------------ verify

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigStatus {
    NotChecked,
    Missing,
    Valid,
    Invalid(String),
}

#[cfg(feature = "serde")]
impl serde::Serialize for SigStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        let (status, reason) = match self {
            SigStatus::NotChecked => ("not_checked", None),
            SigStatus::Missing => ("missing", None),
            SigStatus::Valid => ("valid", None),
            SigStatus::Invalid(r) => ("invalid", Some(r.as_str())),
        };
        m.serialize_entry("status", status)?;
        if let Some(r) = reason {
            m.serialize_entry("reason", r)?;
        }
        m.end()
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct VerifyReport {
    pub patch: PathBuf,
    pub problems: Vec<String>,
    pub signature: SigStatus,
    /// With a client given: what is not applied to it (empty = fully applied).
    pub not_applied: Option<Vec<String>>,
}

fn drain(r: &mut dyn Read, job: &Job) -> io::Result<u64> {
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        if job.check().is_err() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
    }
}

/// Structure: the ZIP opens; `update.sah` parses; `update.saf` covers every entry; every ZIP entry is read to its end (CRC checked by the
/// zip reader); `remove.txt` paths are valid; loose names pass the loose-name rules; no case-insensitive duplicates. Signature with `key`. `client`: every
/// patch file present and byte-equal in the client (visible entry), every removal absent, every loose file equal by SHA-256 (protected
/// ones compared at `<name>.new`); mismatches listed in `not_applied`.
pub fn verify(patch: &Path, key: Option<&VerifyKey>, client: Option<&Path>, job: &Job) -> Result<VerifyReport> {
    let mut rep = VerifyReport { patch: patch.to_path_buf(), problems: Vec::new(), signature: SigStatus::NotChecked, not_applied: None };
    if !patch.is_file() {
        return Err(Error::io(patch, io::Error::new(io::ErrorKind::NotFound, "no such patch file")));
    }
    job.stage("verifying");
    let mut z = match open_zip(patch) {
        Ok(z) => z,
        Err(Error::Format { what, .. }) => {
            rep.problems.push(what);
            sign_status(&mut rep, key, patch, job)?;
            return Ok(rep);
        }
        Err(e) => return Err(e),
    };
    let idx = match read_index(&mut z) {
        Ok(i) => Some(i),
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(e) => {
            rep.problems.push(e.to_string());
            None
        }
    };
    let removals = match read_removals(&mut z) {
        Ok(r) => r,
        Err(e) => {
            rep.problems.push(format!("remove.txt: {e}"));
            Vec::new()
        }
    };
    for p in &removals {
        if split_path(p).is_none() {
            rep.problems.push(format!("remove.txt: bad path {p:?}"));
        }
    }
    if let Some(idx) = &idx {
        let needed = needed_of(idx);
        match find(&mut z, SAF) {
            None if !idx.files().is_empty() => rep.problems.push("the patch has files in update.sah but no update.saf".to_string()),
            Some(i) => {
                let size = z.by_index_raw(i).map(|f| f.size()).unwrap_or(0);
                if size < needed {
                    rep.problems.push(format!("update.saf holds {size} bytes but update.sah refers to {needed}"));
                }
            }
            None => {}
        }
        for (path, _) in idx.files() {
            if let Some((dirs, name)) = split_path(&path) {
                if dirs.iter().chain(std::iter::once(&name)).any(|p| encode_name_strict(p).is_err()) {
                    rep.problems.push(format!("{path}: name cannot be written in code page 949"));
                }
            }
        }
    }
    // loose names and duplicates
    let mut seen: HashMap<String, String> = HashMap::new();
    for i in 0..z.len() {
        let Ok(f) = z.by_index_raw(i) else { continue };
        let name = f.name().to_string();
        if f.is_dir() {
            continue;
        }
        if let Some(other) = seen.insert(name.to_lowercase(), name.clone()) {
            rep.problems.push(format!("{name:?} and {other:?} differ only in letter case"));
        }
        if reserved(&name) {
            continue;
        }
        if f.enclosed_name().is_none() {
            rep.problems.push(format!("unsafe path in patch: {name:?}"));
        } else if let Some(why) = loose_name_problem(&name) {
            rep.problems.push(why);
        }
    }
    // every entry read to its end
    for i in 0..z.len() {
        job.check()?;
        match z.by_index(i) {
            Ok(mut f) => {
                let name = f.name().to_string();
                match drain(&mut f, job) {
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(Error::Cancelled),
                    Err(e) => rep.problems.push(format!("{name}: {e}")),
                }
            }
            Err(e) => rep.problems.push(format!("entry {i}: {e}")),
        }
    }
    sign_status(&mut rep, key, patch, job)?;
    if let (Some(client), Some(idx)) = (client, &idx) {
        let mut na = Vec::new();
        applied_check(&mut z, idx, &removals, client, &mut na, job)?;
        rep.not_applied = Some(na);
    }
    Ok(rep)
}

fn sign_status(rep: &mut VerifyReport, key: Option<&VerifyKey>, patch: &Path, job: &Job) -> Result<()> {
    let Some(k) = key else { return Ok(()) };
    if !signature_path(patch).is_file() {
        rep.signature = SigStatus::Missing;
        return Ok(());
    }
    rep.signature = match sign::verify_file(&k.public_hex, k.check_version, patch, None, job) {
        Ok(()) => SigStatus::Valid,
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(e) => SigStatus::Invalid(e.to_string()),
    };
    Ok(())
}

fn applied_check(z: &mut ZipArchive<File>, idx: &Index, removals: &[String], client: &Path, out: &mut Vec<String>, job: &Job) -> Result<()> {
    let archive = Archive::open(client)?;
    let cidx = archive.index();
    // archive files: compare ranges of update.saf with the client's entries, in one pass over update.saf
    let mut tasks: Vec<(String, Entry, Entry)> = Vec::new();
    let saf_i = find(z, SAF);
    for (path, e) in idx.files() {
        let Some(ce) = cidx.get(&path) else {
            out.push(format!("{path}: not in the client's archive"));
            continue;
        };
        if ce.length != e.length {
            out.push(format!("{path}: the client's file has {} bytes, the patch's has {}", ce.length, e.length));
        } else if e.length > 0 {
            tasks.push((path, e.clone(), ce.clone()));
        }
    }
    tasks.sort_by_key(|t| t.1.offset);
    let mut next = 0;
    let (mut buf_a, mut buf_b) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    while next < tasks.len() {
        let Some(si) = saf_i else {
            out.push("the patch has no update.saf to compare with".to_string());
            break;
        };
        let mut zf = z.by_index(si).map_err(zip_err)?;
        let mut pos = 0u64;
        while next < tasks.len() {
            let (path, e, ce) = &tasks[next];
            if e.offset < pos {
                break; // overlapping ranges: reopen the stream
            }
            job.check()?;
            let mut skip = e.offset - pos;
            while skip > 0 {
                let n = zf.read(&mut buf_a[..skip.min(1 << 20) as usize])?;
                if n == 0 {
                    break;
                }
                skip -= n as u64;
                pos += n as u64;
            }
            let mut reader = archive.reader(ce)?;
            let (mut left, mut same) = (e.length as usize, true);
            while left > 0 {
                let n = left.min(buf_a.len());
                if zf.read_exact(&mut buf_a[..n]).is_err() || reader.read_exact(&mut buf_b[..n]).is_err() {
                    same = false;
                    break;
                }
                pos += n as u64;
                left -= n;
                if buf_a[..n] != buf_b[..n] {
                    same = false;
                    // keep the stream position right for the next entry
                    while left > 0 {
                        let m = zf.read(&mut buf_a[..left.min(1 << 20)])?;
                        if m == 0 {
                            break;
                        }
                        left -= m;
                        pos += m as u64;
                    }
                    break;
                }
            }
            if !same {
                out.push(format!("{path}: differs from the patch's file"));
            }
            next += 1;
        }
    }
    for r in removals {
        if cidx.get(r).is_some() {
            out.push(format!("{r}: should have been removed"));
        }
    }
    // loose files by SHA-256
    for (i, name, _) in loose_entries(z) {
        job.check()?;
        let mut h = Hasher::new();
        {
            let mut f = z.by_index(i).map_err(zip_err)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
        }
        let want = to_hex(&h.finish());
        let rel = name.replace('\\', "/");
        let path = rel.split('/').fold(client.to_path_buf(), |p, s| p.join(s));
        let ok = |p: &Path| p.is_file() && sha256_file(p, job).map(|d| to_hex(&d) == want).unwrap_or(false);
        let mut new = path.clone().into_os_string();
        new.push(".new");
        if !(ok(&path) || ok(Path::new(&new))) {
            out.push(format!("{name}: the loose file in the client differs or is missing"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_archive::compact::{compact, usage};

    fn tmpdir(name: &str) -> shaiya_testkit::TempDir {
        shaiya_testkit::TempDir::new(name)
    }

    #[test]
    fn build_then_apply() {
        let tmp = tmpdir("apply");
        let base = tmp.path();
        // a tiny client: one file in the archive
        let client = base.join("client");
        fs::create_dir_all(&client).unwrap();
        let mut idx = Index::new();
        idx.put("Item/Item.SData", Entry { name: b"Item.SData".to_vec(), offset: 0, length: 3, version: 1 }).unwrap();
        fs::write(client.join("data.sah"), idx.to_bytes()).unwrap();
        fs::write(client.join("data.saf"), b"old").unwrap();
        // a patch replacing it (different case) and adding a file and a loose file
        let files = base.join("files");
        fs::create_dir_all(files.join("item")).unwrap();
        fs::create_dir_all(files.join("interface/icon")).unwrap();
        fs::write(files.join("item/ITEM.SDATA"), b"new!").unwrap();
        fs::write(files.join("interface/icon/29.dds"), b"icon").unwrap();
        let loose = base.join("loose");
        fs::create_dir_all(&loose).unwrap();
        fs::write(loose.join("notice.txt"), b"hello").unwrap();
        fs::write(loose.join("updater.exe"), b"exe").unwrap();
        let p = base.join("ps0109.patch");
        build(Some(&files), Some(&loose), None, &p).unwrap();

        let r = apply(&p, &client, &["updater.exe".into()]).unwrap();
        assert_eq!(r.replaced, vec!["item/ITEM.SDATA"]);
        assert_eq!(r.added, vec!["interface/icon/29.dds"]);
        let idx = Index::parse(&fs::read(client.join("data.sah")).unwrap()).unwrap();
        assert_eq!(idx.file_count(), 2);
        let saf = fs::read(client.join("data.saf")).unwrap();
        let e = idx.get("Item/Item.SData").unwrap();
        assert_eq!(&saf[e.offset as usize..][..e.length as usize], b"new!");
        let e = idx.get("interface/icon/29.dds").unwrap();
        assert_eq!(&saf[e.offset as usize..][..e.length as usize], b"icon");
        assert_eq!(fs::read(client.join("notice.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(client.join("updater.exe.new")).unwrap(), b"exe");
        assert!(!client.join("updater.exe").exists());
        assert!(client.join("data.sah.bak").exists());
    }

    /// A small client archive: `junk` at 0 (later removed), a shadowed duplicate `a.dg` at 4, `keep.dds` at 8, dead space at 12..20,
    /// the client's `A.DG` at 20.
    fn messy_client(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let mut idx = Index::new();
        let en = |n: &str, o: u64| Entry { name: n.as_bytes().to_vec(), offset: o, length: 4, version: 1 };
        idx.root.dirs.push(shaiya_archive::Dir { name: b"world".to_vec(), files: vec![en("a.dg", 4), en("keep.dds", 8), en("A.DG", 20)], dirs: vec![] });
        idx.root.dirs.push(shaiya_archive::Dir { name: b"test".to_vec(), files: vec![en("junk.eft", 0)], dirs: vec![] });
        idx.free.push(en("rest space", 12));
        fs::write(dir.join("data.sah"), idx.to_bytes()).unwrap();
        fs::write(dir.join("data.saf"), b"JUNKold!keep........NEW!").unwrap();
    }

    fn read(dir: &Path, path: &str) -> Vec<u8> {
        let idx = Index::parse(&fs::read(dir.join("data.sah")).unwrap()).unwrap();
        let saf = fs::read(dir.join("data.saf")).unwrap();
        let e = idx.get(path).unwrap();
        saf[e.offset as usize..][..e.length as usize].to_vec()
    }

    fn list_loose(p: &Path) -> Vec<String> {
        list(p).unwrap().1
    }

    #[test]
    fn remove_list_then_compact() {
        let tmp = tmpdir("compact");
        let base = tmp.path();
        let client = base.join("client");
        messy_client(&client);
        // remove.txt from a TSV with a header and a comment; one path is not in this archive
        let list = base.join("list.tsv");
        fs::write(&list, "path\tbytes\n# test files\nTEST/junk.eft\t4\nEffect/never.eft\t1\n").unwrap();
        let p = base.join("ps0109.patch");
        build(None, None, Some(&list), &p).unwrap();
        assert_eq!(removals(&p).unwrap(), vec!["TEST/junk.eft", "Effect/never.eft"]);
        assert!(list_loose(&p).is_empty());
        let r = apply(&p, &client, &[]).unwrap();
        assert_eq!(r.removed, vec!["TEST/junk.eft"]);
        assert_eq!(r.missing, vec!["Effect/never.eft"]);
        assert_eq!(usage(&client).unwrap(), (24, 12)); // a.dg, keep.dds, A.DG are still referenced

        let c = compact(&client, &Job::silent()).unwrap();
        assert_eq!((c.before, c.after), (24, 8));
        assert_eq!(c.duplicates, vec!["world/a.dg"]);
        assert_eq!(fs::read(client.join("data.saf")).unwrap(), b"keepNEW!".to_vec());
        assert_eq!(read(&client, "world/a.dg"), b"NEW!");
        assert_eq!(read(&client, "world/keep.dds"), b"keep");
        let idx = Index::parse(&fs::read(client.join("data.sah")).unwrap()).unwrap();
        assert!(idx.free.is_empty());
        assert_eq!(idx.file_count(), 2);
        assert!(!client.join("data.saf.compact").exists() && !client.join("data.sah.compact").exists() && !client.join("data.compact-commit").exists());
    }

    #[test]
    fn patch_names() {
        assert_eq!(patch_number("ps0109.patch"), Some(109));
        assert_eq!(patch_number("PS0001.PATCH"), Some(1));
        assert_eq!(patch_number("ps12345.patch"), Some(12345));
        for bad in ["ps.patch", "ps01a.patch", "patch.zip", "ps0109.patch.sig", "x0109.patch", ""] {
            assert_eq!(patch_number(bad), None, "{bad}");
        }
        assert_eq!(patch_name(109), "ps0109.patch");
        assert_eq!(patch_name(12345), "ps12345.patch");
    }
}
