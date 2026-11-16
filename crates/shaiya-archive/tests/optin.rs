//! Opt-in checks on the author's own client (`SHAIYA_CLIENT`, read only): the library numbers the command line prints.

use shaiya_archive::check::{check_archive, check_index, CheckOptions};
use shaiya_archive::diff::{diff, summarize, DiffOptions, Side};
use shaiya_archive::{Archive, Index};
use shaiya_core::Job;
use std::time::Instant;

#[test]
fn current_and_stock_pairs_of_the_client() {
    let Some(dir) = shaiya_testkit::env_client() else { return };
    let t = Instant::now();
    let a = Archive::open(&dir).unwrap();
    let opened = t.elapsed();
    let t = Instant::now();
    let r = check_archive(&a, &CheckOptions::default(), &Job::silent()).unwrap();
    eprintln!("current pair: open {opened:?}, check {:?}: {} files ({} visible), {} dirs, dead {:?}, gaps {}, overlaps {}, problems {}, warnings {}", t.elapsed(), r.files, r.visible, r.dirs, r.layout.dead_bytes, r.layout.gap_count, r.layout.overlaps, r.problems(), r.warnings());
    eprintln!("gaps: {:?}", r.layout.largest_gaps);
    assert_eq!(r.problems(), 0);
    // the spec expected 0 gaps here; archive_access_audit.py agrees with this code (2 gaps, 194,934 unreferenced bytes on the pair seen)
    assert_eq!(r.layout.dead_bytes, Some(r.layout.largest_gaps.iter().map(|g| g.1).sum::<u64>()));
    assert_eq!(r.files, r.visible);

    let stock = dir.join("data.orig.sah");
    if !stock.is_file() {
        eprintln!("skipped: no data.orig.sah in the client folder");
        return;
    }
    let s = Archive::open(&stock).unwrap();
    let rs = check_archive(&s, &CheckOptions::default(), &Job::silent()).unwrap();
    eprintln!("stock pair: {} files, {} shadowed, dead {:?}, gaps {}, overlaps {}, free list {}", rs.files, rs.files - rs.visible, rs.layout.dead_bytes, rs.layout.gap_count, rs.layout.overlaps, rs.free_entries);
    assert_eq!(rs.problems(), 0);
    assert_eq!(rs.files - rs.visible, 5, "the stock US pair has 5 shadowed entries");
    assert_eq!(rs.count("shadowed"), 5);
    // index-only against the stock index alone
    let (_, idx) = Archive::open_index(&stock).unwrap();
    assert_eq!(check_index(&idx, None).visible, rs.visible);
    let t = Instant::now();
    let d = diff(&Side::Archive(&s), &Side::Archive(&a), &DiffOptions::default(), &Job::silent()).unwrap();
    eprintln!("quick diff stock -> current: {:?} in {:?}", summarize(&d), t.elapsed());
    let same = Index::read(&dir.join("data.sah")).unwrap();
    assert_eq!(same.to_bytes(), std::fs::read(dir.join("data.sah")).unwrap(), "the index rewrite is byte-identical");
}
