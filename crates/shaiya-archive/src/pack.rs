//! Building a new archive pair from a folder tree.

use crate::edit::{check_outputs, pair_for, temp_next_to};
use crate::{encode_name_strict, path_key, split_path, Entry, Index};
use shaiya_core::fs::Existing;
use shaiya_core::glob::Filter;
use shaiya_core::{Error, Job, Result, Unit};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PackOptions {
    pub sah_version: i32,
    /// Entry version for every file; `None` = the file's modification time.
    pub timestamp: Option<i32>,
    pub filter: Filter,
    /// Keep `Thumbs.db`, `desktop.ini`, `.DS_Store` and `._*` files.
    pub include_all: bool,
    pub existing: Existing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PackReport {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
    /// `(path, reason)`.
    pub skipped: Vec<(String, String)>,
}

fn is_junk(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "thumbs.db" || l == "desktop.ini" || l == ".ds_store" || name.starts_with("._")
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, skipped: &mut Vec<(String, String)>) -> Result<()> {
    for e in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let e = e.map_err(|e| Error::io(dir, e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let link = std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
        if link.file_type().is_symlink() && path.is_dir() {
            skipped.push((r, "symbolic link to a folder is not followed".to_string()));
        } else if link.is_dir() {
            walk(&path, &r, out, skipped)?;
        } else {
            out.push((r, path));
        }
    }
    Ok(())
}

/// New pair from a folder tree: paths sorted (byte order of the cp949 names), folders created in that order, contents back to back,
/// empty free list, trailer 4 zero bytes. Entry version = file mtime (seconds, clamped to i32) or `timestamp`. Names cp949-strict: any
/// unencodable name -> `Error::Refused` listing up to 20. Without `include_all`, junk files are skipped (reported). A file over
/// `u32::MAX` bytes -> `Error::Refused`.
pub fn pack_folder(src: &Path, out_sah: &Path, opts: &PackOptions, job: &Job) -> Result<PackReport> {
    let (out_sah, out_saf) = pair_for(out_sah);
    if !src.is_dir() {
        return Err(Error::io(src, std::io::Error::new(std::io::ErrorKind::NotFound, "not a folder")));
    }
    check_outputs(&[src], &[&out_sah, &out_saf], opts.existing)?;
    job.stage("packing");
    let mut report = PackReport::default();
    let mut found = Vec::new();
    walk(src, "", &mut found, &mut report.skipped)?;
    let mut files: Vec<(Vec<u8>, String, PathBuf, u64, i32)> = Vec::new();
    let mut bad_names: Vec<String> = Vec::new();
    let mut too_big: Vec<String> = Vec::new();
    for (rel, path) in found {
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        if !opts.include_all && is_junk(name) {
            report.skipped.push((rel, "junk file".to_string()));
            continue;
        }
        let meta = std::fs::metadata(&path).map_err(|e| Error::io(&path, e))?;
        if !opts.filter.matches(&rel, meta.len()) {
            continue;
        }
        let raw: std::result::Result<Vec<Vec<u8>>, Error> = rel.split('/').map(encode_name_strict).collect();
        match raw {
            Ok(parts) => {
                if meta.len() > u32::MAX as u64 {
                    too_big.push(rel);
                    continue;
                }
                let version = opts.timestamp.unwrap_or_else(|| {
                    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs().min(i32::MAX as u64) as i32).unwrap_or(0)
                });
                files.push((parts.join(&b'/'), rel, path, meta.len(), version));
            }
            Err(_) => bad_names.push(rel),
        }
    }
    if !bad_names.is_empty() {
        let shown: Vec<&str> = bad_names.iter().take(20).map(String::as_str).collect();
        return Err(Error::Refused(format!(
            "{} names cannot be written in an archive (code page 949 has no character for them): {}{}",
            bad_names.len(),
            shown.join(", "),
            if bad_names.len() > 20 { ", ..." } else { "" }
        )));
    }
    if !too_big.is_empty() {
        return Err(Error::Refused(format!("files over 4 GiB - 1 cannot be archive entries: {}", too_big.join(", "))));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut seen = std::collections::HashMap::new();
    for (raw, rel, ..) in &files {
        if let Some(other) = seen.insert(path_key(raw), rel.clone()) {
            return Err(Error::Refused(format!("{rel} and {other} differ only in letter case; the client sees them as one file")));
        }
    }
    let total: u64 = files.iter().map(|f| f.3).sum();
    job.total(Some(total), Unit::Bytes);
    let (tmp_sah, tmp_saf) = (temp_next_to(&out_sah), temp_next_to(&out_saf));
    let built = (|| -> Result<Index> {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp_saf).map_err(|e| Error::io(&tmp_saf, e))?);
        let mut idx = Index::new();
        idx.version = opts.sah_version;
        let mut pos = 0u64;
        for (raw, rel, path, len, version) in &files {
            job.check()?;
            let source = crate::edit::Source::File(path.clone());
            let n = source.copy_to(&mut w, job)?;
            if n != *len {
                return Err(Error::format(rel.clone(), None, format!("the file changed while it was packed ({len} bytes expected, {n} read)")));
            }
            let name = raw.rsplit(|&b| b == b'/').next().unwrap_or(raw).to_vec();
            split_path(rel).ok_or_else(|| Error::format(rel.clone(), None, "not a valid archive path"))?;
            idx.put(rel, Entry { name, offset: pos, length: *len as u32, version: *version })?;
            pos += n;
        }
        w.into_inner().map_err(|e| Error::io(&tmp_saf, e.into_error()))?.sync_all().map_err(|e| Error::io(&tmp_saf, e))?;
        Ok(idx)
    })();
    let done = built.and_then(|idx| {
        let mut f = File::create(&tmp_sah).map_err(|e| Error::io(&tmp_sah, e))?;
        f.write_all(&idx.to_bytes()).and_then(|_| f.sync_all()).map_err(|e| Error::io(&tmp_sah, e))?;
        drop(f);
        std::fs::rename(&tmp_saf, &out_saf).map_err(|e| Error::io(&out_saf, e))?;
        std::fs::rename(&tmp_sah, &out_sah).map_err(|e| Error::io(&out_sah, e))?;
        Ok(idx)
    });
    match done {
        Ok(idx) => {
            report.files = files.len();
            report.dirs = idx.dir_count();
            report.bytes = total;
            Ok(report)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_saf);
            let _ = std::fs::remove_file(&tmp_sah);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Archive;
    use shaiya_testkit::TempDir;

    fn opts() -> PackOptions {
        PackOptions { sah_version: 0, timestamp: Some(1_274_079_412), filter: Filter::default(), include_all: false, existing: Existing::Refuse }
    }

    fn tree(d: &Path) -> PathBuf {
        let src = d.join("src");
        std::fs::create_dir_all(src.join("Item/3DO")).unwrap();
        std::fs::create_dir_all(src.join("interface")).unwrap();
        std::fs::write(src.join("Item/Item.SData"), b"item").unwrap();
        std::fs::write(src.join("Item/3DO/a.3do"), b"3do").unwrap();
        std::fs::write(src.join("interface/t.txt"), b"").unwrap();
        std::fs::write(src.join("top.txt"), b"top").unwrap();
        std::fs::write(src.join("Thumbs.db"), b"junk").unwrap();
        std::fs::write(src.join("._mac"), b"junk").unwrap();
        src
    }

    #[test]
    fn packs_sorted_with_cp949_names_and_skips_junk() {
        let d = TempDir::new("pack1");
        let src = tree(d.path());
        let korean = "10051\u{C6D0}.3do";
        std::fs::write(src.join("Item/3DO").join(korean), b"korean").unwrap();
        let out = d.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let r = pack_folder(&src, &out.join("data.sah"), &opts(), &Job::silent()).unwrap();
        assert_eq!((r.files, r.dirs, r.bytes), (5, 3, 4 + 3 + 6 + 3));
        assert_eq!(r.skipped.len(), 2);
        let a = Archive::open(&out).unwrap();
        assert_eq!(a.read("item/item.sdata").unwrap(), b"item");
        assert_eq!(a.read(&format!("Item/3DO/{korean}")).unwrap(), b"korean");
        assert_eq!(a.read("interface/t.txt").unwrap(), b"");
        // index order follows creation order: root files, then folders in first-use order
        let names: Vec<String> = a.index().entries().iter().map(|f| f.path.clone()).collect();
        assert_eq!(names, vec!["top.txt".to_string(), "Item/Item.SData".into(), format!("Item/3DO/{korean}"), "Item/3DO/a.3do".into(), "interface/t.txt".into()]);
        // contents are back to back in sorted order: "Item/3DO/1..." < "Item/3DO/a" < "Item/Item" < "interface" < "top"
        let off = |p: &str| a.index().get(p).unwrap().offset;
        assert_eq!(off(&format!("Item/3DO/{korean}")), 0);
        assert_eq!(off("Item/3DO/a.3do"), 6);
        assert_eq!(off("Item/Item.SData"), 9);
        assert_eq!(off("top.txt"), 13);
        assert!(a.index().entries().iter().all(|f| f.entry.version == 1_274_079_412));
        assert_eq!(a.index().free.len(), 0);
        assert_eq!(a.index().trailer, vec![0; 4]);
        assert_eq!(a.saf_len(), r.bytes);
        assert_eq!(a.index().data_end(), a.saf_len());
    }

    #[test]
    fn include_all_keeps_junk_and_filter_limits() {
        let d = TempDir::new("pack2");
        let src = tree(d.path());
        let mut o = opts();
        o.include_all = true;
        let out = d.path().join("all.sah");
        let r = pack_folder(&src, &out, &o, &Job::silent()).unwrap();
        assert_eq!((r.files, r.skipped.len()), (6, 0));
        let mut f = opts();
        f.filter = Filter { include: vec![shaiya_core::glob::Pattern::new("*.sdata").unwrap()], ..Filter::default() };
        let r = pack_folder(&src, &d.path().join("one.sah"), &f, &Job::silent()).unwrap();
        assert_eq!(r.files, 1);
        // an existing output is refused, and --force overwrites
        assert!(matches!(pack_folder(&src, &out, &o, &Job::silent()), Err(Error::Refused(_))));
        o.existing = Existing::Overwrite;
        pack_folder(&src, &out, &o, &Job::silent()).unwrap();
    }

    #[test]
    fn unencodable_names_are_refused() {
        let d = TempDir::new("pack3");
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ok.txt"), b"1").unwrap();
        std::fs::write(src.join("\u{1F600}.txt"), b"2").unwrap();
        let e = pack_folder(&src, &d.path().join("o.sah"), &opts(), &Job::silent()).unwrap_err();
        assert!(matches!(&e, Error::Refused(m) if m.contains('\u{1F600}')), "{e}");
        assert!(!d.path().join("o.sah").exists() && !d.path().join("o.saf").exists());
        assert!(pack_folder(&d.path().join("nope"), &d.path().join("o.sah"), &opts(), &Job::silent()).is_err());
    }

    #[test]
    fn real_timestamps_come_from_the_files() {
        let d = TempDir::new("pack4");
        let src = d.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"1").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(src.join("a.txt")).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)).unwrap();
        drop(f);
        let mut o = opts();
        o.timestamp = None;
        pack_folder(&src, &d.path().join("o.sah"), &o, &Job::silent()).unwrap();
        let a = Archive::open(&d.path().join("o.sah")).unwrap();
        assert_eq!(a.index().get("a.txt").unwrap().version, 1_000_000_000);
    }

    #[test]
    fn a_cancelled_pack_leaves_no_output() {
        let d = TempDir::new("pack5");
        let src = tree(d.path());
        let c = shaiya_core::Cancel::new();
        c.cancel();
        let r = pack_folder(&src, &d.path().join("o.sah"), &opts(), &Job::new(&shaiya_core::Silent, c));
        assert!(matches!(r, Err(Error::Cancelled)));
        let left: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["src"]);
    }
}
