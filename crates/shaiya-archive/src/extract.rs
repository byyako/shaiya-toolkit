//! Extracting archive entries into a folder, safely: every name segment passes `portable_name`, nothing leaves the chosen folder.

use crate::{decode_name, Archive, Entry, Index};
use shaiya_core::fs::{portable_name, write_atomic_from_with, Existing, Sync};
use shaiya_core::glob::Filter;
use shaiya_core::{Error, Job, Result, Unit};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ExtractOptions {
    pub filter: Filter,
    pub include_shadowed: bool,
    pub existing: Existing,
    pub keep_times: bool,
}

#[derive(Clone, Debug)]
pub struct Planned {
    pub path: String,
    pub entry: Entry,
    pub target: PathBuf,
    /// A name segment was changed by `portable_name` (or renamed to avoid a clash).
    pub escaped: bool,
    pub exists: bool,
    pub shadowed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ExtractReport {
    pub written: usize,
    pub bytes: u64,
    pub skipped_existing: Vec<String>,
    /// `(archive path, relative path written)` for names that had to be changed.
    pub escaped: Vec<(String, String)>,
    pub shadowed_skipped: usize,
}

fn with_suffix(name: &str, suffix: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}{suffix}{}", &name[..i], &name[i..]),
        _ => format!("{name}{suffix}"),
    }
}

/// Targets: `out/<segments>`, each segment through `portable_name`; a shadowed entry (with `include_shadowed`) gets `~shadowed-<n>`
/// before its extension. Two entries mapping to the same target (case-insensitively) -> the later visible one keeps the name, the other
/// is renamed like a shadowed one. Sorted by SAF offset (sequential reads).
pub fn plan(index: &Index, out: &Path, opts: &ExtractOptions) -> Result<Vec<Planned>> {
    struct Cand<'a> {
        path: String,
        entry: &'a Entry,
        segs: Vec<String>,
        escaped: bool,
        shadowed: bool,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for f in index.entries() {
        if f.shadowed && !opts.include_shadowed {
            continue;
        }
        if !opts.filter.matches(&f.path, f.entry.length as u64) {
            continue;
        }
        let mut escaped = false;
        let segs: Vec<String> = f
            .segments
            .iter()
            .map(|raw| {
                let s = decode_name(raw);
                match portable_name(&s) {
                    Some(p) => {
                        escaped = true;
                        p
                    }
                    None => s,
                }
            })
            .collect();
        cands.push(Cand { path: f.path, entry: f.entry, segs, escaped, shadowed: f.shadowed });
    }
    // group by case-folded target; the last visible one keeps the name
    let key = |c: &Cand| c.segs.join("/").to_lowercase();
    let mut winner: HashMap<String, usize> = HashMap::new();
    for (i, c) in cands.iter().enumerate() {
        if !c.shadowed {
            winner.insert(key(c), i);
        }
    }
    for (i, c) in cands.iter().enumerate() {
        winner.entry(key(c)).or_insert(i);
    }
    let mut counter: HashMap<String, u32> = HashMap::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut planned = Vec::with_capacity(cands.len());
    for (i, mut c) in cands.into_iter().enumerate() {
        let k = key(&c);
        let mut rename = winner[&k] != i;
        if rename {
            let n = counter.entry(k.clone()).or_insert(0);
            *n += 1;
            let last = c.segs.last_mut().expect("a file has a name");
            *last = with_suffix(last, &format!("~shadowed-{n}"));
            c.escaped = true;
        }
        // the renamed name could clash with a real file: add numbers until free
        while !taken.insert(key(&c)) {
            let n = counter.entry(k.clone()).or_insert(0);
            *n += 1;
            let last = c.segs.last_mut().expect("a file has a name");
            *last = with_suffix(last, &format!("~dup-{n}"));
            rename = true;
        }
        let _ = rename;
        let mut target = out.to_path_buf();
        for s in &c.segs {
            target.push(s);
        }
        let exists = target.exists();
        planned.push(Planned { path: c.path, entry: c.entry.clone(), target, escaped: c.escaped, exists, shadowed: c.shadowed });
    }
    planned.sort_by_key(|p| p.entry.offset);
    Ok(planned)
}

/// `existing == Refuse` and any target exists -> `Error::Refused` before anything is written. Writes with `Sync::No` (temp + rename: no
/// half-written target); `keep_times` sets each file's modification time to the entry version when it is > 0.
pub fn extract(archive: &Archive, out: &Path, opts: &ExtractOptions, job: &Job) -> Result<ExtractReport> {
    job.stage("extracting");
    let planned = plan(archive.index(), out, opts)?;
    if opts.existing == Existing::Refuse {
        let clash: Vec<String> = planned.iter().filter(|p| p.exists).map(|p| p.target.display().to_string()).collect();
        if !clash.is_empty() {
            let shown: Vec<&str> = clash.iter().take(5).map(String::as_str).collect();
            return Err(Error::Refused(format!(
                "{} of the files to extract already exist in {} (for example {}); use --force to overwrite or --skip-existing to leave them",
                clash.len(),
                out.display(),
                shown.join(", ")
            )));
        }
    }
    let mut rep = ExtractReport::default();
    if !opts.include_shadowed {
        rep.shadowed_skipped = archive.index().entries().iter().filter(|f| f.shadowed && opts.filter.matches(&f.path, f.entry.length as u64)).count();
    }
    let total: u64 = planned.iter().filter(|p| !(p.exists && opts.existing == Existing::Skip)).map(|p| p.entry.length as u64).sum();
    job.total(Some(total), Unit::Bytes);
    let mut made: HashSet<PathBuf> = HashSet::new();
    for p in &planned {
        job.check()?;
        if p.exists && opts.existing == Existing::Skip {
            rep.skipped_existing.push(p.path.clone());
            continue;
        }
        if let Some(dir) = p.target.parent() {
            if made.insert(dir.to_path_buf()) {
                std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
            }
        }
        if p.target.is_dir() {
            return Err(Error::Refused(format!("{} is a folder; cannot write the file {}", p.target.display(), p.path)));
        }
        let mut r = archive.reader(&p.entry)?;
        let n = write_atomic_from_with(&p.target, &mut r, Sync::No, job)?;
        rep.written += 1;
        rep.bytes += n;
        if opts.keep_times && p.entry.version > 0 {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(p.entry.version as u64);
            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p.target) {
                let _ = f.set_modified(t);
            }
        }
        if p.escaped {
            let rel = p.target.strip_prefix(out).unwrap_or(&p.target).to_string_lossy().replace('\\', "/");
            rep.escaped.push((p.path.clone(), rel));
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_core::glob::Pattern;
    use shaiya_testkit::{SahBuilder, TempDir};

    fn opts() -> ExtractOptions {
        ExtractOptions { filter: Filter::default(), include_shadowed: false, existing: Existing::Refuse, keep_times: false }
    }

    fn open(d: &Path, b: SahBuilder) -> Archive {
        b.write_to(d);
        Archive::open(d).unwrap()
    }

    #[test]
    fn extracts_visible_entries_with_folders() {
        let d = TempDir::new("ex1");
        let a = open(d.path(), SahBuilder::new(0).file(b"Item/Item.SData", b"item").file(b"a/b/c.txt", b"deep").file(b"top.txt", b""));
        let out = d.path().join("out");
        let r = extract(&a, &out, &opts(), &Job::silent()).unwrap();
        assert_eq!((r.written, r.bytes), (3, 8));
        assert_eq!(std::fs::read(out.join("Item/Item.SData")).unwrap(), b"item");
        assert_eq!(std::fs::read(out.join("a/b/c.txt")).unwrap(), b"deep");
        assert_eq!(std::fs::read(out.join("top.txt")).unwrap(), b"");
        // nothing but the files themselves (no temp leftovers)
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 3);
    }

    #[test]
    fn patterns_filter_and_existing_files_are_handled() {
        let d = TempDir::new("ex2");
        let a = open(d.path(), SahBuilder::new(0).file(b"Item/Item.SData", b"item").file(b"x/y.dds", b"dds").file(b"x/z.txt", b"z"));
        let out = d.path().join("out");
        let mut o = opts();
        o.filter = Filter { include: vec![Pattern::new("*.dds").unwrap(), Pattern::new("*.sdata").unwrap()], ..Filter::default() };
        assert_eq!(extract(&a, &out, &o, &Job::silent()).unwrap().written, 2);
        assert!(!out.join("x/z.txt").exists());
        // existing: refuse writes nothing new, skip keeps the old bytes, overwrite replaces
        std::fs::write(out.join("x/y.dds"), b"mine").unwrap();
        let mut all = opts();
        let e = extract(&a, &out, &all, &Job::silent()).unwrap_err();
        assert!(matches!(e, Error::Refused(_)), "{e:?}");
        assert!(!out.join("x/z.txt").exists(), "refusal happens before any write");
        all.existing = Existing::Skip;
        let r = extract(&a, &out, &all, &Job::silent()).unwrap();
        assert_eq!((r.skipped_existing.len(), r.written), (2, 1));
        assert_eq!(std::fs::read(out.join("x/y.dds")).unwrap(), b"mine");
        std::fs::remove_file(out.join("x/y.dds")).unwrap();
        let r = extract(&a, &out, &all, &Job::silent()).unwrap();
        assert_eq!((r.written, r.skipped_existing.len()), (1, 2));
        all.existing = Existing::Overwrite;
        let r = extract(&a, &out, &all, &Job::silent()).unwrap();
        assert_eq!(r.written, 3);
        assert_eq!(std::fs::read(out.join("x/y.dds")).unwrap(), b"dds");
    }

    #[test]
    fn shadowed_entries_get_their_own_names() {
        let d = TempDir::new("ex3");
        let a = open(d.path(), SahBuilder::new(0).file(b"w/A.dg", b"old").file(b"w/other.txt", b"o").file(b"w/a.DG", b"new"));
        let out = d.path().join("out");
        let r = extract(&a, &out, &opts(), &Job::silent()).unwrap();
        assert_eq!((r.written, r.shadowed_skipped), (2, 1));
        assert_eq!(std::fs::read(out.join("w/a.DG")).unwrap(), b"new");
        let out2 = d.path().join("out2");
        let mut o = opts();
        o.include_shadowed = true;
        let r = extract(&a, &out2, &o, &Job::silent()).unwrap();
        assert_eq!((r.written, r.shadowed_skipped), (3, 0));
        assert_eq!(std::fs::read(out2.join("w/a.DG")).unwrap(), b"new");
        assert_eq!(std::fs::read(out2.join("w/A~shadowed-1.dg")).unwrap(), b"old");
        assert_eq!(r.escaped, vec![("w/A.dg".to_string(), "w/A~shadowed-1.dg".to_string())]);
    }

    #[test]
    fn names_are_escaped_and_never_leave_the_folder() {
        let d = TempDir::new("ex4");
        // a ":" in a name, a reserved device name, a trailing dot and ".." as a file name
        let a = open(
            d.path(),
            SahBuilder::new(0).file(b"f/a:b.txt", b"1").file(b"f/CON.txt", b"2").file(b"f/dot.", b"3").file(b"f/..", b"4").file(b"f/ok.txt", b"5"),
        );
        let out = d.path().join("out");
        let r = extract(&a, &out, &opts(), &Job::silent()).unwrap();
        assert_eq!(r.written, 5);
        assert_eq!(std::fs::read(out.join("f/a%3Ab.txt")).unwrap(), b"1");
        assert_eq!(std::fs::read(out.join("f/%43ON.txt")).unwrap(), b"2");
        assert_eq!(std::fs::read(out.join("f/dot%2E")).unwrap(), b"3");
        assert_eq!(std::fs::read(out.join("f/.%2E")).unwrap(), b"4");
        assert_eq!(r.escaped.len(), 4);
        // nothing outside `out`
        let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| n == "out" || n.starts_with("data.")), "{names:?}");
    }

    #[test]
    fn korean_names_and_times() {
        let d = TempDir::new("ex5");
        let korean: &[u8] = &[0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD];
        let a = open(d.path(), SahBuilder::new(0).file_versioned(&[b"m/", korean, b".txt"].concat(), b"k", 1_274_079_412).file_versioned(b"m/z", b"z", 0));
        let out = d.path().join("out");
        let mut o = opts();
        o.keep_times = true;
        extract(&a, &out, &o, &Job::silent()).unwrap();
        let p = out.join("m").join(format!("{}.txt", decode_name(korean)));
        let m = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(m.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), 1_274_079_412);
        // version 0 keeps the write time (not 1970)
        let z = std::fs::metadata(out.join("m/z")).unwrap().modified().unwrap();
        assert!(z.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() > 1_000_000_000);
    }

    #[test]
    fn cancel_midway_leaves_complete_files_only() {
        let d = TempDir::new("ex6");
        let big = vec![7u8; 3_000_000];
        let a = open(d.path(), SahBuilder::new(0).file(b"a/1.bin", &big).file(b"a/2.bin", &big).file(b"a/3.bin", &big));
        let out = d.path().join("out");
        struct CancelSoon(shaiya_core::Cancel, std::cell::Cell<u64>);
        impl shaiya_core::Progress for CancelSoon {
            fn advance(&self, n: u64) {
                self.1.set(self.1.get() + n);
                if self.1.get() > 4_000_000 {
                    self.0.cancel();
                }
            }
        }
        let c = shaiya_core::Cancel::new();
        let sink = CancelSoon(c.clone(), std::cell::Cell::new(0));
        let r = extract(&a, &out, &opts(), &Job::new(&sink, c));
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
        let mut seen = 0;
        for e in std::fs::read_dir(out.join("a")).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(!name.ends_with(".shaiya-tmp"), "temp file left: {name}");
            assert_eq!(e.metadata().unwrap().len(), 3_000_000, "{name} is complete");
            seen += 1;
        }
        assert!(seen >= 1 && seen < 3, "{seen}");
    }
}
