//! Compaction: rewrite `data.saf` without the dead space that replaced and removed files leave behind.
//!
//! Progress and cancellation go through a `shaiya_core::Job`; `finish_compact` completes or discards an interrupted run.

use crate::{Dir, Index};
use shaiya_core::{Error, Job, Result, Unit};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const COMPACT_SAH: &str = "data.sah.compact";
const COMPACT_SAF: &str = "data.saf.compact";
/// Exists only between "both compacted files are complete and synced" and "both are in place".
const COMPACT_COMMIT: &str = "data.compact-commit";

/// `(data.sah, data.saf)` of a client folder (found in any case; the plain names when they are not there yet).
fn pair(client: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    match crate::client::ClientFiles::locate(client) {
        Ok(f) => (f.sah, f.saf),
        Err(_) => (client.join("data.sah"), client.join("data.saf")),
    }
}

fn read_index(client: &Path) -> Result<Index> {
    Index::read(&pair(client).0)
}

fn size_of(path: &Path) -> Result<u64> {
    fs::metadata(path).map(|m| m.len()).map_err(|e| Error::io(path, e))
}

/// `data.saf` size and the bytes the index still uses (shared ranges counted once). `size - live` is dead space.
pub fn usage(client: &Path) -> Result<(u64, u64)> {
    let idx = read_index(client)?;
    let size = size_of(&pair(client).1)?;
    let mut ranges: Vec<(u64, u64)> = idx.files().iter().map(|(_, e)| (e.offset, e.offset + e.length as u64)).collect();
    ranges.sort_unstable();
    let (mut live, mut end) = (0u64, 0u64);
    for (a, b) in ranges {
        let a = a.max(end);
        if b > a {
            live += b - a;
            end = b;
        }
    }
    Ok((size, live))
}

#[derive(Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Compacted {
    pub before: u64,
    pub after: u64,
    /// Shadowed duplicate entries dropped (the client never read them).
    pub duplicates: Vec<String>,
}

/// Rewrite `data.saf` with only the bytes the index uses (in their old order), drop shadowed duplicates and the free list, and point
/// the index at the new offsets. Needs free disk space for one more copy of the live data.
///
/// Crash safety: both new files are written as `*.compact` and synced, then `data.compact-commit` is created and the two are renamed
/// over the old ones. [`finish_compact`] (run it before touching the archive) rolls an interrupted compaction forward when the commit
/// marker exists and discards the half-written files when it does not, so the index and `data.saf` always match. On an error (a full
/// disk) or when `job` is cancelled the temp files are removed and the archive is left as it was (`Error::Cancelled`).
pub fn compact(client: &Path, job: &Job) -> Result<Compacted> {
    crate::client::recover(client)?;
    let r = compact_inner(client, job);
    if r.is_err() {
        let _ = fs::remove_file(client.join(COMPACT_SAH));
        let _ = fs::remove_file(client.join(COMPACT_SAF));
    }
    r
}

fn compact_inner(client: &Path, job: &Job) -> Result<Compacted> {
    let saf_path = pair(client).1;
    let mut idx = read_index(client)?;
    let mut report = Compacted { before: size_of(&saf_path)?, ..Compacted::default() };
    report.duplicates = idx.dedupe();
    idx.free.clear();

    // Old (offset, length) -> new offset; an exact range shared by several entries is copied once.
    let mut ranges: Vec<(u64, u32)> = idx.files().iter().map(|(_, e)| (e.offset, e.length)).collect();
    ranges.sort_unstable();
    ranges.dedup();
    let total: u64 = ranges.iter().map(|&(_, l)| l as u64).sum();
    job.stage("Rewriting data.saf");
    job.total(Some(total), Unit::Bytes);
    let mut moved = std::collections::HashMap::with_capacity(ranges.len());
    {
        let new_saf = client.join(COMPACT_SAF);
        let mut src = File::open(&saf_path).map_err(|e| Error::io(&saf_path, e))?;
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&new_saf).map_err(|e| Error::io(&new_saf, e))?);
        let mut buf = vec![0u8; 1 << 20];
        let mut pos = 0u64;
        for &(off, len) in &ranges {
            src.seek(SeekFrom::Start(off)).map_err(|e| Error::io(&saf_path, e))?;
            let mut left = len as u64;
            while left > 0 {
                job.check()?;
                let n = left.min(buf.len() as u64) as usize;
                src.read_exact(&mut buf[..n]).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        Error::format("data.saf", Some(off), "data.saf is shorter than its index")
                    } else {
                        Error::io(&saf_path, e)
                    }
                })?;
                out.write_all(&buf[..n]).map_err(|e| Error::io(&new_saf, e))?;
                left -= n as u64;
                job.advance(n as u64);
            }
            moved.insert((off, len), pos);
            pos += len as u64;
        }
        out.into_inner().map_err(|e| Error::io(&new_saf, e.into_error()))?.sync_all().map_err(|e| Error::io(&new_saf, e))?;
        report.after = pos;
    }
    fn relocate(d: &mut Dir, moved: &std::collections::HashMap<(u64, u32), u64>) {
        for f in &mut d.files {
            f.offset = moved[&(f.offset, f.length)];
        }
        for c in &mut d.dirs {
            relocate(c, moved);
        }
    }
    relocate(&mut idx.root, &moved);
    job.check()?;
    {
        let new_sah = client.join(COMPACT_SAH);
        let mut f = File::create(&new_sah).map_err(|e| Error::io(&new_sah, e))?;
        f.write_all(&idx.to_bytes()).map_err(|e| Error::io(&new_sah, e))?;
        f.sync_all().map_err(|e| Error::io(&new_sah, e))?;
    }
    let sah_path = pair(client).0;
    let mut bak = sah_path.clone().into_os_string();
    bak.push(".bak");
    fs::copy(&sah_path, std::path::PathBuf::from(bak)).map_err(|e| Error::io(&sah_path, e))?;
    let marker = client.join(COMPACT_COMMIT);
    File::create(&marker).and_then(|f| f.sync_all()).map_err(|e| Error::io(&marker, e))?;
    finish_compact(client)?;
    Ok(report)
}

/// Complete or discard an interrupted [`compact`]; does nothing when none was running.
pub fn finish_compact(client: &Path) -> Result<()> {
    let (sah, saf) = (client.join(COMPACT_SAH), client.join(COMPACT_SAF));
    let step = |from: &Path, to: &Path| fs::rename(from, to).map_err(|e| Error::io(to, e));
    if client.join(COMPACT_COMMIT).exists() {
        // Committed: each rename replaces its target; one may already be done.
        if saf.exists() {
            step(&saf, &pair(client).1)?;
        }
        if sah.exists() {
            step(&sah, &pair(client).0)?;
        }
        let marker = client.join(COMPACT_COMMIT);
        fs::remove_file(&marker).map_err(|e| Error::io(&marker, e))?;
    } else {
        for p in [&sah, &saf] {
            if p.exists() {
                fs::remove_file(p).map_err(|e| Error::io(p, e))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Entry;
    use shaiya_core::{Cancel, Silent};
    use shaiya_testkit::TempDir;

    /// A small client archive: `junk` at 0, a shadowed duplicate `a.dg` at 4, `keep.dds` at 8, dead space at 12..20, the client's
    /// `A.DG` at 20.
    fn messy_client(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let mut idx = Index::new();
        let en = |n: &str, o: u64| Entry { name: n.as_bytes().to_vec(), offset: o, length: 4, version: 1 };
        idx.root.dirs.push(Dir { name: b"world".to_vec(), files: vec![en("a.dg", 4), en("keep.dds", 8), en("A.DG", 20)], dirs: vec![] });
        idx.root.dirs.push(Dir { name: b"test".to_vec(), files: vec![en("junk.eft", 0)], dirs: vec![] });
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

    #[test]
    fn interrupted_compaction_recovers() {
        let base = TempDir::new("recover");
        let base = base.path();
        // stopped before the commit: the half-written files are discarded, the archive is untouched
        let a = base.join("a");
        messy_client(&a);
        let before = (fs::read(a.join("data.sah")).unwrap(), fs::read(a.join("data.saf")).unwrap());
        fs::write(a.join(COMPACT_SAF), b"half").unwrap();
        fs::write(a.join(COMPACT_SAH), b"half").unwrap();
        finish_compact(&a).unwrap();
        assert_eq!((fs::read(a.join("data.sah")).unwrap(), fs::read(a.join("data.saf")).unwrap()), before);
        assert!(!a.join(COMPACT_SAF).exists() && !a.join(COMPACT_SAH).exists());

        // stopped after the commit with only data.saf renamed: finishing puts the new index in place too
        let b = base.join("b");
        messy_client(&b);
        compact(&b, &Job::silent()).unwrap();
        let (sah, saf) = (fs::read(b.join("data.sah")).unwrap(), fs::read(b.join("data.saf")).unwrap());
        let c = base.join("c");
        messy_client(&c);
        fs::write(c.join("data.saf"), &saf).unwrap();
        fs::write(c.join(COMPACT_SAH), &sah).unwrap();
        fs::write(c.join(COMPACT_COMMIT), b"").unwrap();
        finish_compact(&c).unwrap();
        assert_eq!(fs::read(c.join("data.sah")).unwrap(), sah);
        assert_eq!(read(&c, "world/A.DG"), b"NEW!");
        assert!(!c.join(COMPACT_COMMIT).exists());
    }

    #[test]
    fn compact_drops_dead_space_and_duplicates() {
        let d = TempDir::new("compact");
        messy_client(d.path());
        assert_eq!(usage(d.path()).unwrap(), (24, 16)); // junk, keep.dds, a.dg (shadowed, still counted) and A.DG
        let progress = std::cell::RefCell::new(Vec::new());
        let p = shaiya_core::FnProgress::new(|done, total| progress.borrow_mut().push((done, total)));
        let c = compact(d.path(), &Job::new(&p, Cancel::new())).unwrap();
        assert_eq!((c.before, c.after), (24, 12));
        assert_eq!(c.duplicates, vec!["world/a.dg"]);
        assert_eq!(read(d.path(), "world/A.DG"), b"NEW!");
        assert_eq!(read(d.path(), "world/keep.dds"), b"keep");
        assert_eq!(read(d.path(), "test/junk.eft"), b"JUNK");
        let idx = Index::parse(&fs::read(d.path().join("data.sah")).unwrap()).unwrap();
        assert!(idx.free.is_empty());
        assert_eq!(idx.file_count(), 3);
        assert_eq!(fs::metadata(d.path().join("data.saf")).unwrap().len(), 12);
        assert!(d.path().join("data.sah.bak").exists());
        assert_eq!(progress.borrow().last(), Some(&(12, Some(12))));
        assert!(!d.path().join(COMPACT_SAF).exists() && !d.path().join(COMPACT_SAH).exists() && !d.path().join(COMPACT_COMMIT).exists());
        // nothing left to compact
        assert_eq!(usage(d.path()).unwrap(), (12, 12));
    }

    #[test]
    fn a_cancelled_compaction_leaves_both_files_byte_identical() {
        let d = TempDir::new("cancel");
        messy_client(d.path());
        let before = (fs::read(d.path().join("data.sah")).unwrap(), fs::read(d.path().join("data.saf")).unwrap());
        // cancel after the first copied chunk
        struct CancelAfterFirst(Cancel);
        impl shaiya_core::Progress for CancelAfterFirst {
            fn advance(&self, _n: u64) {
                self.0.cancel();
            }
        }
        let c = Cancel::new();
        let sink = CancelAfterFirst(c.clone());
        let r = compact(d.path(), &Job::new(&sink, c));
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        assert_eq!((fs::read(d.path().join("data.sah")).unwrap(), fs::read(d.path().join("data.saf")).unwrap()), before);
        let names: Vec<_> = fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".compact") && !n.contains("commit")), "{names:?}");
        // cancelled before it starts
        let c2 = Cancel::new();
        c2.cancel();
        assert!(matches!(compact(d.path(), &Job::new(&Silent, c2)), Err(Error::Cancelled)));
    }
}
