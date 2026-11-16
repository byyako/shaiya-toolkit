//! `psNNNN.patch`: a ZIP (deflate) holding `update.sah` + `update.saf` (an archive of files to merge into `data.sah/saf`), optionally
//! `remove.txt` (archive paths to drop, one per line), and loose files that are copied into the client folder (e.g. `game.exe`).
//!
//! Moved from `rust/archive/src/patch.rs` of the Shaiya EP4 Client repo (compaction is in `shaiya_archive::compact`). Signatures are in
//! [`sign`].

pub mod sign;

use shaiya_archive::{split_path, Entry, Index};
use shaiya_core::fs::{write_atomic, write_atomic_from};
use shaiya_core::{Error, Job};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub use shaiya_core::Result;

const SAH: &str = "update.sah";
const SAF: &str = "update.saf";
const REMOVE: &str = "remove.txt";

#[derive(Debug, Default)]
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

/// List a patch: (archive paths with their lengths, loose file names).
pub fn list(patch: &Path) -> Result<(Vec<(String, u32)>, Vec<String>)> {
    let mut z = open_zip(patch)?;
    let idx = read_index(&mut z)?;
    let files = idx.files().into_iter().map(|(p, e)| (p, e.length)).collect();
    Ok((files, loose_names(&mut z)))
}

/// The archive paths a patch removes (`remove.txt`; none when it has no such file).
pub fn removals(patch: &Path) -> Result<Vec<String>> {
    read_removals(&mut open_zip(patch)?)
}

fn read_removals(z: &mut ZipArchive<File>) -> Result<Vec<String>> {
    let Some(i) = find(z, REMOVE) else { return Ok(Vec::new()) };
    let mut text = String::new();
    z.by_index(i).map_err(zip_err)?.take(16 << 20).read_to_string(&mut text)?;
    Ok(parse_list(&text))
}

/// Archive paths from a list: the first tab-separated column of each line; blank lines, `#` comments and a `path` header are skipped
/// (so `docs/cleanup-candidates.tsv` can be used as it is).
fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('\t').next().unwrap_or("").trim().replace('\\', "/"))
        .filter(|p| !p.is_empty() && !p.starts_with('#') && !p.eq_ignore_ascii_case("path"))
        .collect()
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

fn loose_names(z: &mut ZipArchive<File>) -> Vec<String> {
    (0..z.len()).filter_map(|i| z.name_for_index(i).map(str::to_string)).filter(|n| !n.ends_with('/') && !reserved(n)).collect()
}

/// Merge a patch into `client/data.sah` + `client/data.saf`, then write its loose files.
///
/// Crash safety: new file contents are appended to `data.saf` and synced before the index is replaced (written to a temp file, then
/// renamed; the previous index is kept as `data.sah.bak`). If the process stops early, the old index still describes valid data; only
/// unreferenced bytes are left at the end of `data.saf`. Replaced and removed files leave dead space behind; `shaiya_archive::compact`
/// reclaims it.
///
/// `protect`: loose file names (case-insensitive) that must not be overwritten, e.g. the running updater; they are written as
/// `<name>.new` instead.
///
/// Every loose file name is checked (`enclosed_name`: no `..`, no absolute paths) before anything is changed.
pub fn apply(patch: &Path, client: &Path, protect: &[String]) -> Result<Report> {
    let mut z = open_zip(patch)?;
    let pidx = read_index(&mut z)?;
    let mut report = Report::default();

    // 0. Refuse unsafe loose file names before touching anything.
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(zip_err)?;
        if !f.is_dir() && !reserved(f.name()) && f.enclosed_name().is_none() {
            return Err(Error::format(patch.display().to_string(), None, format!("unsafe path in patch: {:?}", f.name())));
        }
    }

    // 1. Unpack update.saf next to the archive so it can be read at offsets.
    let saf_tmp = client.join("update.saf.tmp");
    if !pidx.files().is_empty() {
        let i = find(&mut z, SAF).ok_or_else(|| Error::format("patch", None, "patch has no update.saf"))?;
        // Never unpack more than the index refers to (guards against a decompression bomb).
        let needed = pidx.files().iter().map(|(_, e)| e.offset + e.length as u64).max().unwrap_or(0);
        let mut out = io_at(&saf_tmp, File::create(&saf_tmp))?;
        io::copy(&mut z.by_index(i).map_err(zip_err)?.take(needed), &mut out)?;
    }

    // 2. Drop the listed files, then append the new ones to data.saf and update the index. Removals come first, so a patch that lists
    //    a path and also carries it ends with its new copy.
    let sah_path = client.join("data.sah");
    let mut idx = Index::read(&sah_path)?;
    for path in read_removals(&mut z)? {
        if idx.remove(&path)?.is_empty() {
            report.missing.push(path);
        } else {
            report.removed.push(path);
        }
    }
    if !pidx.files().is_empty() {
        let mut src = io_at(&saf_tmp, File::open(&saf_tmp))?;
        let saf_path = client.join("data.saf");
        let mut saf = io_at(&saf_path, OpenOptions::new().read(true).write(true).open(&saf_path))?;
        let mut pos = io_at(&saf_path, saf.seek(SeekFrom::End(0)))?;
        for (path, e) in pidx.files() {
            io_at(&saf_tmp, src.seek(SeekFrom::Start(e.offset)))?;
            let mut take = (&mut src).take(e.length as u64);
            let n = io_at(&saf_path, io::copy(&mut take, &mut saf))?;
            if n != e.length as u64 {
                return Err(Error::format("update.saf", Some(e.offset), format!("{path}: update.saf is truncated")));
            }
            let entry = Entry { name: e.name.clone(), offset: pos, length: e.length, version: e.version };
            pos += n;
            report.bytes += n;
            match idx.put(&path, entry)? {
                Some(_) => report.replaced.push(path),
                None => report.added.push(path),
            }
        }
        io_at(&saf_path, saf.sync_all())?;
        drop(src);
        io_at(&saf_tmp, fs::remove_file(&saf_tmp))?;
    }

    // 3. Replace the index.
    if !pidx.files().is_empty() || !report.removed.is_empty() {
        io_at(&sah_path, fs::copy(&sah_path, client.join("data.sah.bak")))?;
        write_atomic(&sah_path, &idx.to_bytes())?;
    }

    // 4. Loose files.
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(zip_err)?;
        let name = f.name().to_string();
        if f.is_dir() || reserved(&name) {
            continue;
        }
        let rel = f.enclosed_name().ok_or_else(|| Error::format(patch.display().to_string(), None, format!("unsafe path in patch: {name:?}")))?;
        let mut target = client.join(&rel);
        if protect.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            target = PathBuf::from(format!("{}.new", target.display()));
        }
        if let Some(parent) = target.parent() {
            io_at(parent, fs::create_dir_all(parent))?;
        }
        write_atomic_from(&target, &mut f, &Job::silent())?;
        report.loose.push(target.strip_prefix(client).unwrap_or(&target).display().to_string());
    }
    Ok(report)
}

/// Build a patch. `files` mirrors archive paths (e.g. `files/Item/Item.SData`); `loose` holds files for the client folder (e.g.
/// `loose/game.exe`); `remove` is a list of archive paths to drop (see [`parse_list`]). Any may be None. Entry versions are the files'
/// modification times.
pub fn build(files: Option<&Path>, loose: Option<&Path>, remove: Option<&Path>, out: &Path) -> Result<Report> {
    let mut report = Report::default();
    let mut idx = Index::new();
    let mut saf: Vec<u8> = Vec::new();
    if let Some(root) = files {
        for (rel, full) in walk(root)? {
            let data = io_at(&full, fs::read(&full))?;
            let version = fs::metadata(&full)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i32)
                .unwrap_or(0);
            let (_, name) = split_path(&rel).ok_or_else(|| Error::format(full.display().to_string(), None, format!("bad path {rel}")))?;
            let entry = Entry { name: shaiya_archive::encode_name(name), offset: saf.len() as u64, length: data.len() as u32, version };
            saf.extend_from_slice(&data);
            report.bytes += data.len() as u64;
            idx.put(&rel, entry)?;
            report.added.push(rel);
        }
    }
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut z = ZipWriter::new(io_at(out, File::create(out))?);
    z.start_file(SAH, opts).map_err(zip_err)?;
    z.write_all(&idx.to_bytes())?;
    z.start_file(SAF, opts).map_err(zip_err)?;
    z.write_all(&saf)?;
    if let Some(list) = remove {
        let paths = parse_list(&io_at(list, fs::read_to_string(list))?);
        for p in &paths {
            split_path(p).ok_or_else(|| Error::format(list.display().to_string(), None, format!("bad path in the removal list: {p:?}")))?;
        }
        z.start_file(REMOVE, opts).map_err(zip_err)?;
        z.write_all(paths.join("\n").as_bytes())?;
        report.removed = paths;
    }
    if let Some(root) = loose {
        for (rel, full) in walk(root)? {
            if reserved(&rel) {
                return Err(Error::format(full.display().to_string(), None, format!("loose file {rel:?} has a name the patch format reserves")));
            }
            z.start_file(rel.clone(), opts).map_err(zip_err)?;
            z.write_all(&io_at(&full, fs::read(&full))?)?;
            report.loose.push(rel);
        }
    }
    z.finish().map_err(zip_err)?;
    Ok(report)
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
}
