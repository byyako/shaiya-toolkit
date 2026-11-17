//! Opt-in tests on the author's own client (spec section 10.2).
//!
//! - `SHAIYA_CLIENT` (read only): the SData files extracted equal `Archive::read`, `sdata check --from` finds no bad checksum, the quick diff
//!   of the stock and current pairs, timings printed.
//! - `SHAIYA_CLIENT` + `SHAIYA_SCRATCH` (a folder outside every repo with room for several GB): a copy of the pair (and `Version.ini`)
//!   goes to `SHAIYA_SCRATCH/t1-us-copy-<pid>`; a patch (SData round trip, an added file, a removal, a loose file) is built, applied with a
//!   journal, verified, checked and rolled back to the original hashes; then compact, a content diff against the original, and a swap to a
//!   rewritten pair and back. The copy is removed at the end, also on failure. MB/s printed for apply, compact, extract.

use shaiya_archive::check::{check_archive, CheckOptions};
use shaiya_archive::diff::{diff, summarize, Change, DiffOptions, Side};
use shaiya_archive::edit::{self, EditPlan, Source};
use shaiya_archive::extract::{extract, ExtractOptions};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_archive::{compact, swap, Archive, ArchiveSource};
use shaiya_core::fs::Existing;
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::hash::{sha256_file, to_hex};
use shaiya_core::source::ByteSource;
use shaiya_core::Job;
use shaiya_patch::{apply_with, build_with, plan, verify, ApplyOptions, BuildInput, BuildOptions};
use shaiya_seed::batch::{self, BatchOptions, Op, Output, Status};
use shaiya_testkit::{env_client, env_scratch, TempDir};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn sdata_filter(pattern: &str) -> Filter {
    Filter { include: vec![Pattern::new(pattern).expect("pattern")], ..Filter::default() }
}

fn mbs(bytes: u64, since: Instant) -> String {
    format!("{:.1} MB/s ({} MB in {:.2} s)", bytes as f64 / 1e6 / since.elapsed().as_secs_f64().max(1e-9), bytes / 1_000_000, since.elapsed().as_secs_f64())
}

fn hash(p: &Path) -> String {
    to_hex(&sha256_file(p, &Job::silent()).unwrap_or_else(|e| panic!("hash {}: {e}", p.display())))
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn readonly_checks_on_the_us_client() {
    let Some(us) = env_client() else {
        eprintln!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&us).unwrap();
    let job = Job::silent();
    let all = ArchiveSource { archive: &a, filter: sdata_filter("*.sdata"), include_shadowed: false };
    let names = all.names();
    assert!(!names.is_empty(), "the US client has SData files");

    // extract of every *.SData equals Archive::read
    let tmp = TempDir::new("us-extract");
    let t = Instant::now();
    let rep = extract(&a, tmp.path(), &ExtractOptions { filter: sdata_filter("*.sdata"), include_shadowed: false, existing: Existing::Refuse, keep_times: false }, &job).unwrap();
    eprintln!("extract *.SData: {} files, {}", rep.written, mbs(rep.bytes, t));
    assert_eq!(rep.written, names.len());
    for n in &names {
        let got = std::fs::read(n.split('/').fold(tmp.path().to_path_buf(), |p, s| p.join(s))).unwrap();
        assert_eq!(got, a.read(n).unwrap(), "{n}");
    }

    // sdata check --from: no bad checksum
    let items = batch::run(&all, &BatchOptions { op: Op::Check, output: Output::None, existing: Existing::Refuse, pass_through: false, dry_run: false }, &job).unwrap();
    let bad: Vec<_> = items.iter().filter(|i| matches!(i.status, Status::Failed | Status::CrcBad)).map(|i| i.name.clone()).collect();
    eprintln!("sdata check --from: {} files, {} CRC OK, {} plain, {} bad", items.len(), items.iter().filter(|i| i.status == Status::CrcOk).count(), items.iter().filter(|i| i.status == Status::Plain).count(), bad.len());
    assert!(bad.is_empty(), "{bad:?}");

    // timings: list (index), check, diff --quick stock vs current
    let t = Instant::now();
    let n = a.index().visible().len();
    eprintln!("list: {n} visible entries in {:?}", t.elapsed());
    let t = Instant::now();
    let r = check_archive(&a, &CheckOptions::default(), &job).unwrap();
    eprintln!("check: {} problems, {} warnings in {:?}", r.problems(), r.warnings(), t.elapsed());
    assert_eq!(r.problems(), 0);
    if let Some(stock) = find_ci(&us, "data.orig.sah") {
        let s = Archive::open(&stock).unwrap();
        let t = Instant::now();
        let d = diff(&Side::Archive(&s), &Side::Archive(&a), &DiffOptions::default(), &job).unwrap();
        eprintln!("diff --quick stock -> current: {:?} in {:?}", summarize(&d), t.elapsed());
    } else {
        eprintln!("skipped: no data.orig.sah in the client folder");
    }
}

#[test]
fn us_copy_patch_compact_swap_and_back() {
    let (Some(us), Some(scratch)) = (env_client(), env_scratch()) else {
        eprintln!("skipped: SHAIYA_CLIENT or SHAIYA_SCRATCH not set");
        return;
    };
    let work = scratch.join(format!("t1-us-copy-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let _cleanup = Cleanup(work.clone());
    let job = Job::silent();

    // the copy
    let client = work.join("client");
    std::fs::create_dir_all(&client).unwrap();
    let t = Instant::now();
    let mut copied = 0u64;
    for name in ["data.sah", "data.saf", "Version.ini"] {
        match find_ci(&us, name) {
            Some(src) => copied += std::fs::copy(&src, client.join(name)).unwrap(),
            None => assert_eq!(name, "Version.ini", "{name} is missing from the client"),
        }
    }
    eprintln!("copy: {}", mbs(copied, t));
    let (sah, saf) = (client.join("data.sah"), client.join("data.saf"));
    let (h_sah, h_saf) = (hash(&sah), hash(&saf));
    assert_eq!((h_sah.clone(), h_saf.clone()), (hash(&find_ci(&us, "data.sah").unwrap()), hash(&find_ci(&us, "data.saf").unwrap())));

    // the patch: Item.SData through decrypt + encrypt --like itself, one added file, one removal, one loose file
    let original = Archive::open(&client).unwrap();
    let item = ArchiveSource { archive: &original, filter: sdata_filter("Item/Item.SData"), include_shadowed: false };
    let (plain_dir, enc_dir) = (work.join("rt-plain"), work.join("rt-enc"));
    let dec = BatchOptions { op: Op::Decrypt { unchecked: false }, output: Output::Dir(plain_dir.clone()), existing: Existing::Refuse, pass_through: true, dry_run: false };
    assert!(batch::run(&item, &dec, &job).unwrap().iter().all(|i| i.status == Status::Decrypted), "Item.SData decrypts");
    let plains = shaiya_core::source::FolderSource::new(&[plain_dir], &Filter::default()).unwrap();
    let enc = BatchOptions { op: Op::Encrypt { like: Some(&item) }, output: Output::Dir(enc_dir.clone()), existing: Existing::Refuse, pass_through: true, dry_run: false };
    batch::run(&plains, &enc, &job).unwrap();
    let item_enc = enc_dir.join("Item").join("Item.SData");
    assert_eq!(std::fs::read(&item_enc).unwrap(), original.read("Item/Item.SData").unwrap(), "encrypt --like of the decrypted file is byte-identical");
    let item_entry = original.index().get("Item/Item.SData").unwrap().clone();
    let remove = original
        .index()
        .visible()
        .into_iter()
        .map(|f| f.path)
        .find(|p| p != "Item/Item.SData" && p.split('/').all(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))))
        .expect("a plain path to remove");
    let loose = work.join("t1-loose.txt");
    std::fs::write(&loose, b"t1 loose file").unwrap();
    let input = BuildInput {
        files: vec![("Item/Item.SData".to_string(), Source::File(item_enc), item_entry.version), ("interface/t1-test.txt".to_string(), Source::Bytes(b"t1 test file".to_vec()), 1_700_000_000)],
        loose: vec![("t1-loose.txt".to_string(), loose)],
        removals: vec![remove.clone()],
    };
    let patch = work.join("ps0109.patch");
    build_with(&input, &patch, &BuildOptions { existing: Existing::Refuse, sign: None }, &job).unwrap();
    drop(item);
    drop(original);

    // dry run plan, then apply with a journal
    let p = plan(&patch, &client, &[]).unwrap();
    assert_eq!((p.added.len(), p.replaced.len(), p.removed.len(), p.loose.len()), (1, 1, 1, 1), "{p:?}");
    assert_eq!(hash(&sah), h_sah, "the plan wrote nothing");
    let structure = verify(&patch, None, None, &job).unwrap();
    assert!(structure.problems.is_empty(), "{:?}", structure.problems);
    let opts = ApplyOptions { protect: Vec::new(), journal: JournalMode::In(client.join("shaiya-toolkit-backups")), legacy_bak: false, require_signature: None, check_in_use: true };
    let t = Instant::now();
    let rep = apply_with(&patch, &client, &opts, &job).unwrap();
    eprintln!("apply: {}", mbs(rep.bytes, t));
    let applied = verify(&patch, None, Some(&client), &job).unwrap();
    assert_eq!(applied.not_applied, Some(Vec::new()), "verify --client after apply: {:?}", applied.not_applied);
    let a = Archive::open(&client).unwrap();
    let r = check_archive(&a, &CheckOptions::default(), &job).unwrap();
    assert_eq!(r.problems(), 0);
    let all = ArchiveSource { archive: &a, filter: sdata_filter("*.sdata"), include_shadowed: false };
    let items = batch::run(&all, &BatchOptions { op: Op::Check, output: Output::None, existing: Existing::Refuse, pass_through: false, dry_run: false }, &job).unwrap();
    assert!(!items.iter().any(|i| matches!(i.status, Status::Failed | Status::CrcBad)));
    drop(all);
    drop(a);

    // rollback: the original hashes
    let journal_dir = rep.journal.clone().expect("a journal folder");
    journal::rollback(&client, &journal_dir, false, &job).unwrap();
    assert_eq!((hash(&sah), hash(&saf)), (h_sah.clone(), h_saf.clone()), "data.sah and data.saf equal the originals after rollback");
    assert!(!client.join("t1-loose.txt").exists());
    eprintln!("rollback: data.sah {h_sah}, data.saf {h_saf} (equal to the originals)");

    // a fresh apply, then compact in place and a content diff against the original
    let rep2 = apply_with(&patch, &client, &opts, &job).unwrap();
    assert!(rep2.journal.is_some());
    let t = Instant::now();
    let c = compact::compact(&client, &job).unwrap();
    eprintln!("compact: {} -> {} bytes, {}", c.before, c.after, mbs(c.after, t));
    let (orig, compacted) = (Archive::open(&us).unwrap(), Archive::open(&client).unwrap());
    let t = Instant::now();
    let d = diff(&Side::Archive(&orig), &Side::Archive(&compacted), &DiffOptions { content: true, ..DiffOptions::default() }, &job).unwrap();
    let s = summarize(&d);
    eprintln!("content diff original -> compacted copy: {s:?}, {}", mbs(orig.saf_len(), t));
    assert_eq!((s.different, s.unknown), (0, 0), "compact keeps every visible entry's bytes");
    assert_eq!((s.added, s.removed), (1, 1));
    assert!(d.iter().any(|e| e.change == Change::Removed && e.path == remove));
    drop(orig);
    drop(compacted);

    // swap to a rewritten pair and back
    let (h1_sah, h1_saf) = (hash(&sah), hash(&saf));
    let rewrite_dir = work.join("rewrite");
    std::fs::create_dir_all(&rewrite_dir).unwrap();
    let live = Archive::open(&client).unwrap();
    let t = Instant::now();
    edit::write_new(&live, &EditPlan::default(), &rewrite_dir.join("data.sah"), Existing::Refuse, &job).unwrap();
    eprintln!("write_new (compacted rewrite): {}", mbs(live.saf_len(), t));
    drop(live);
    let (h2_sah, h2_saf) = (hash(&rewrite_dir.join("data.sah")), hash(&rewrite_dir.join("data.saf")));
    let first = swap::swap(&client, &rewrite_dir.join("data.sah"), &job).unwrap();
    assert_eq!((hash(&sah), hash(&saf)), (h2_sah, h2_saf), "the rewritten pair is installed");
    assert_eq!((hash(&first.kept_as.0), hash(&first.kept_as.1)), (h1_sah.clone(), h1_saf.clone()), "the old pair is kept");
    swap::swap(&client, &first.kept_as.0, &job).unwrap();
    assert_eq!((hash(&sah), hash(&saf)), (h1_sah, h1_saf), "swapped back to the compacted pair");
    let r = check_archive(&Archive::open(&client).unwrap(), &CheckOptions::default(), &job).unwrap();
    assert_eq!(r.problems(), 0);
    eprintln!("swap and back: ok");
}
