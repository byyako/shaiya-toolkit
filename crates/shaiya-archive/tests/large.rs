//! Offsets past 4 GiB with real reads and writes, on a sparse `data.saf`. Runs on Unix (sparse files are free there); on Windows only
//! when `SHAIYA_SCRATCH` names a folder outside every repo (the file there is really written). Skipped with a message otherwise or when
//! `set_len` fails.

use shaiya_archive::check::{check_archive, CheckOptions};
use shaiya_archive::edit::{self, EditPlan, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::extract::{extract, ExtractOptions};
use shaiya_archive::journal::{self, JournalMode, Rollback};
use shaiya_archive::{Archive, Dir, Entry, Index};
use shaiya_core::fs::Existing;
use shaiya_core::glob::Filter;
use shaiya_core::Job;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

const GIB5: u64 = 5 << 30;

fn work_dir(tag: &str) -> Option<PathBuf> {
    let base = if cfg!(unix) {
        std::env::temp_dir()
    } else {
        match shaiya_testkit::env_scratch() {
            Some(p) => p,
            None => {
                eprintln!("skipped: the large-offset test needs a sparse-file system or SHAIYA_SCRATCH");
                return None;
            }
        }
    };
    let d = base.join(format!("shaiya-large-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn peak_rss_kb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse().ok())
}

#[test]
fn entries_past_4_gib_check_extract_apply_and_roll_back() {
    let Some(dir) = work_dir("a") else { return };
    let _guard = Cleanup(dir.clone());
    let saf = dir.join("data.saf");
    let mut f = std::fs::File::create(&saf).unwrap();
    if f.set_len(GIB5 + 1000).is_err() {
        eprintln!("skipped: set_len failed on this file system");
        return;
    }
    let en = |n: &str, o: u64, l: u32| Entry { name: n.as_bytes().to_vec(), offset: o, length: l, version: 5 };
    let e1 = en("a.bin", GIB5 - 10, 20); // straddles 5 GiB
    let e2 = en("b.bin", GIB5 + 100, 100);
    let e3 = en("c.bin", (4 << 30) + 5, 50); // just past 4 GiB
    for (e, fill) in [(&e1, 0xA1u8), (&e2, 0xB2), (&e3, 0xC3)] {
        f.seek(SeekFrom::Start(e.offset)).unwrap();
        f.write_all(&vec![fill; e.length as usize]).unwrap();
    }
    f.sync_all().unwrap();
    drop(f);
    let mut idx = Index::new();
    idx.root.dirs.push(Dir { name: b"big".to_vec(), files: vec![e1, e2.clone(), e3], dirs: vec![] });
    std::fs::write(dir.join("data.sah"), idx.to_bytes()).unwrap();
    let sah_before = std::fs::read(dir.join("data.sah")).unwrap();

    let a = Archive::open(&dir).unwrap();
    assert_eq!(a.index().data_end(), GIB5 + 200);
    let r = check_archive(&a, &CheckOptions { deep: true }, &Job::silent()).unwrap();
    assert_eq!(r.problems(), 0, "{:?}", r.findings);
    assert_eq!(r.layout.data_end, GIB5 + 200);
    assert_eq!(r.layout.out_of_bounds_count, 0);
    assert_eq!(a.read_range(a.index().get("big/a.bin").unwrap(), 5, 10).unwrap(), vec![0xA1; 10]);
    let out = dir.join("out");
    let rep = extract(&a, &out, &ExtractOptions { filter: Filter::default(), include_shadowed: false, existing: Existing::Refuse, keep_times: false }, &Job::silent()).unwrap();
    assert_eq!((rep.written, rep.bytes), (3, 170));
    assert_eq!(std::fs::read(out.join("big/b.bin")).unwrap(), vec![0xB2; 100]);
    assert_eq!(std::fs::read(out.join("big/c.bin")).unwrap(), vec![0xC3; 50]);
    drop(a);

    // apply appends at 5 GiB + 1000 and rolls back by truncating
    let plan = EditPlan { puts: vec![Put { path: "big/new.bin".into(), source: Source::Bytes(vec![0xD4; 300]), version: 9, mode: PutMode::AddOnly }], label: "large".into(), ..EditPlan::default() };
    let root = dir.join("backups");
    let rep = edit::apply_in_place(&dir, &plan, &InPlaceOptions { journal: JournalMode::In(root.clone()), legacy_bak: false, check_in_use: true }, &Job::silent()).unwrap();
    assert_eq!((rep.saf_len_before, rep.saf_len_after), (GIB5 + 1000, GIB5 + 1300));
    let a = Archive::open(&dir).unwrap();
    let e = a.index().get("big/new.bin").unwrap();
    assert_eq!(e.offset, GIB5 + 1000);
    assert_eq!(a.read("big/new.bin").unwrap(), vec![0xD4; 300]);
    drop(a);
    let (j, rb) = journal::list(&dir, &root).unwrap().remove(0);
    assert_eq!(rb, Rollback::Possible);
    journal::rollback(&dir, &j.dir, false, &Job::silent()).unwrap();
    assert_eq!(std::fs::metadata(&saf).unwrap().len(), GIB5 + 1000);
    assert_eq!(std::fs::read(dir.join("data.sah")).unwrap(), sah_before);
    if let Some(kb) = peak_rss_kb() {
        assert!(kb < 128 * 1024, "peak RSS {kb} kB");
    }
}
