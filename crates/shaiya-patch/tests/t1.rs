//! T1 patch tests: contents, plans, apply with journals, name rules, deterministic streaming builds, verify, signatures, keys.

use shaiya_archive::edit::Source;
use shaiya_archive::journal::{self, JournalMode, Rollback};
use shaiya_archive::{Archive, Entry, Index};
use shaiya_core::fs::Existing;
use shaiya_core::glob::Filter;
use shaiya_core::{Error, Job};
use shaiya_patch::sign;
use shaiya_patch::{
    apply, apply_with, build_with, contents, input_problems, patch_name, plan, plan_seq, verify, ApplyOptions, BuildInput, BuildOptions, SigStatus, VerifyKey,
};
use shaiya_testkit::{flip, truncate, SahBuilder, TempDir, ZipBuilder};
use std::fs;
use std::path::{Path, PathBuf};

fn client_in(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    SahBuilder::new(0).file(b"Item/Item.SData", b"old item data").file(b"keep/me.txt", b"keep").file(b"drop/gone.txt", b"drop me").write_to(dir);
}

fn snap(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(dir.join("data.sah")).unwrap(), fs::read(dir.join("data.saf")).unwrap())
}

/// A patch from folders: replaces Item.SData, adds new/file.bin, removes drop/gone.txt, loose notice.txt and game.exe.
fn make_patch(base: &Path, name: &str, item: &[u8]) -> PathBuf {
    let files = base.join(format!("files-{name}"));
    fs::create_dir_all(files.join("Item")).unwrap();
    fs::create_dir_all(files.join("new")).unwrap();
    fs::write(files.join("Item/Item.SData"), item).unwrap();
    fs::write(files.join("new/file.bin"), [1u8, 2, 3]).unwrap();
    let loose = base.join(format!("loose-{name}"));
    fs::create_dir_all(&loose).unwrap();
    fs::write(loose.join("notice.txt"), format!("notice {name}")).unwrap();
    fs::write(loose.join("game.exe"), b"new exe").unwrap();
    let rm = base.join(format!("rm-{name}.txt"));
    fs::write(&rm, "drop/gone.txt\nnot/there.txt\n").unwrap();
    let out = base.join(name);
    let input = BuildInput::from_folders(Some(&files), Some(&loose), Some(&rm), Some(1_274_079_412)).unwrap();
    build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).unwrap();
    out
}

fn handmade_patch(path: &Path, files: &[(&str, &[u8])], saf_override: Option<&[u8]>, extra: &[(&str, &[u8])]) {
    let mut idx = Index::new();
    let mut saf = Vec::new();
    for (p, data) in files {
        let name = p.rsplit('/').next().unwrap();
        idx.put(p, Entry { name: name.as_bytes().to_vec(), offset: saf.len() as u64, length: data.len() as u32, version: 5 }).unwrap();
        saf.extend_from_slice(data);
    }
    let mut z = ZipBuilder::new().add("update.sah", &idx.to_bytes()).add("update.saf", saf_override.unwrap_or(&saf));
    for (n, b) in extra {
        z = z.add(n, b);
    }
    z.write_to(path);
}

fn opts(root: &Path) -> ApplyOptions {
    ApplyOptions { protect: vec![], journal: JournalMode::In(root.to_path_buf()), legacy_bak: true, require_signature: None, check_in_use: true }
}

#[test]
fn contents_describes_a_patch_without_unpacking_it() {
    let t = TempDir::new("p-contents");
    let p = make_patch(t.path(), "ps0109.patch", b"new item data!");
    let c = contents(&p).unwrap();
    assert_eq!(c.sah_version, 0);
    let mut files: Vec<_> = c.files.iter().map(|f| (f.path.as_str(), f.length, f.version)).collect();
    files.sort();
    assert_eq!(files, vec![("Item/Item.SData", 14, 1_274_079_412), ("new/file.bin", 3, 1_274_079_412)]);
    let mut loose = c.loose.clone();
    loose.sort();
    assert_eq!(loose, vec![("game.exe".to_string(), 7), ("notice.txt".to_string(), 19)]);
    assert_eq!(c.removals, vec!["drop/gone.txt", "not/there.txt"]);
    assert_eq!(c.zip_size, fs::metadata(&p).unwrap().len());
    assert!(c.unpacked_size >= 14 + 3 + 7 + 19);
    assert!(!c.signature_file);
    fs::write(sign_path(&p), b"x").unwrap();
    assert!(contents(&p).unwrap().signature_file);
}

fn sign_path(p: &Path) -> PathBuf {
    shaiya_patch::signature_path(p)
}

#[test]
fn plan_and_plan_seq_look_ahead_without_writing() {
    let t = TempDir::new("p-plan");
    let c = t.path().join("c");
    client_in(&c);
    fs::write(c.join("notice.txt"), b"already here").unwrap();
    let p1 = make_patch(t.path(), "ps0001.patch", b"one!");
    let p2 = make_patch(t.path(), "ps0002.patch", b"two!!");
    let before = snap(&c);
    let pl = plan(&p1, &c, &["game.exe".to_string()]).unwrap();
    assert_eq!((pl.added.clone(), pl.replaced.clone(), pl.removed.clone(), pl.missing.clone()), (vec!["new/file.bin".to_string()], vec!["Item/Item.SData".to_string()], vec!["drop/gone.txt".to_string()], vec!["not/there.txt".to_string()]));
    assert_eq!(pl.bytes, 4 + 3);
    assert_eq!(pl.temp_bytes, 7);
    let notice = pl.loose.iter().find(|l| l.name == "notice.txt").unwrap();
    assert!(notice.overwrites && !notice.protected);
    let exe = pl.loose.iter().find(|l| l.name == "game.exe").unwrap();
    assert!(exe.protected && exe.target == "game.exe.new" && !exe.overwrites);
    assert_eq!(snap(&c), before, "planning writes nothing");
    assert!(!c.join("data.toolkit-pending").exists());
    // two in a row: the second sees the first's additions as replacements and its removal as missing
    let both = plan_seq(&[p1, p2], &c, &[]).unwrap();
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].added, vec!["new/file.bin"]);
    assert_eq!((both[1].added.len(), both[1].replaced.len()), (0, 2));
    assert_eq!(both[1].removed, Vec::<String>::new());
    assert!(both[1].missing.contains(&"drop/gone.txt".to_string()));
    assert!(both[1].loose.iter().all(|l| l.overwrites), "the first patch writes the same loose files");
}

#[test]
fn apply_with_journal_then_rollback() {
    let t = TempDir::new("p-journal");
    let c = t.path().join("c");
    client_in(&c);
    fs::write(c.join("game.exe"), b"old exe").unwrap();
    let before = snap(&c);
    let p = make_patch(t.path(), "ps0109.patch", b"new item data!");
    let root = c.join(journal::DEFAULT_DIR);
    let r = apply_with(&p, &c, &opts(&root), &Job::silent()).unwrap();
    assert_eq!((r.added.len(), r.replaced.len(), r.removed.len(), r.missing.len()), (1, 1, 1, 1));
    let mut loose = r.loose.clone();
    loose.sort();
    assert_eq!(loose, vec!["game.exe", "notice.txt"]);
    assert_eq!(r.bytes, 14 + 3);
    let a = Archive::open(&c).unwrap();
    assert_eq!(a.read("item/item.sdata").unwrap(), b"new item data!");
    assert_eq!(a.read("new/file.bin").unwrap(), [1, 2, 3]);
    assert!(a.read("drop/gone.txt").is_err());
    drop(a);
    assert_eq!(fs::read(c.join("game.exe")).unwrap(), b"new exe");
    let names: Vec<String> = fs::read_dir(&c).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.ends_with(".shaiya-tmp") && !n.contains("pending")), "{names:?}");
    // the journal and its rollback
    let (j, rb) = journal::list(&c, &root).unwrap().remove(0);
    assert_eq!((j.kind.as_str(), j.label.as_str(), rb), ("patch", "ps0109.patch", Rollback::Possible));
    assert_eq!(r.journal.as_ref(), Some(&j.dir));
    journal::rollback(&c, &j.dir, false, &Job::silent()).unwrap();
    assert_eq!(snap(&c), before);
    assert_eq!(fs::read(c.join("game.exe")).unwrap(), b"old exe");
    assert!(!c.join("notice.txt").exists());
}

#[test]
fn the_t0_apply_keeps_writing_the_backup_and_no_journal() {
    let t = TempDir::new("p-legacy");
    let c = t.path().join("c");
    client_in(&c);
    let before = snap(&c);
    let p = make_patch(t.path(), "ps0001.patch", b"x");
    apply(&p, &c, &[]).unwrap();
    assert_eq!(fs::read(c.join("data.sah.bak")).unwrap(), before.0);
    assert!(!c.join(journal::DEFAULT_DIR).exists());
}

#[test]
fn name_rules_hold_at_build_and_at_apply() {
    let t = TempDir::new("p-d10");
    // build: every offending name is listed at once, nothing is written
    let loose = t.path().join("loose");
    fs::create_dir_all(loose.join("shaiya-toolkit-backups")).unwrap();
    fs::write(loose.join("DATA.SAF"), b"x").unwrap();
    fs::write(loose.join("shaiya-toolkit-backups/x.txt"), b"x").unwrap();
    fs::write(loose.join("data.toolkit-pending"), b"x").unwrap();
    fs::write(loose.join("fine.txt"), b"x").unwrap();
    let out = t.path().join("p.patch");
    let input = BuildInput::from_folders(None, Some(&loose), None, None).unwrap();
    let e = build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).unwrap_err();
    let Error::Refused(m) = e else { panic!("{e:?}") };
    for needle in ["DATA.SAF", "shaiya-toolkit-backups/x.txt", "data.toolkit-pending"] {
        assert!(m.contains(needle), "{needle} not listed in:\n{m}");
    }
    assert!(!m.contains("fine.txt") && !out.exists());
    // case twins among the archive files and unencodable names
    let mut inp = BuildInput::default();
    inp.files.push(("a/B.txt".into(), Source::Bytes(vec![1]), 0));
    inp.files.push(("A/b.TXT".into(), Source::Bytes(vec![2]), 0));
    inp.files.push(("emoji/\u{1F600}.txt".into(), Source::Bytes(vec![3]), 0));
    inp.loose.push(("x.txt".into(), PathBuf::from("x")));
    inp.loose.push(("X.TXT".into(), PathBuf::from("x")));
    let p = input_problems(&inp);
    assert!(p.iter().any(|m| m.contains("same file")) && p.iter().any(|m| m.contains("code page 949")) && p.iter().any(|m| m.contains("letter case")), "{p:?}");

    // apply: a hand-made ZIP with offending loose names changes nothing
    for (evil, kind) in [("DATA.SAF", "refused"), ("sub/data.saf.shaiya-tmp", "refused"), ("../x", "format")] {
        let c = t.path().join(format!("c-{kind}-{}", evil.len()));
        client_in(&c);
        let before = snap(&c);
        let p = t.path().join(format!("evil-{}.patch", evil.len()));
        handmade_patch(&p, &[], None, &[("fine.txt", b"fine"), (evil, b"bad")]);
        let r = apply(&p, &c, &[]);
        match (kind, &r) {
            ("refused", Err(Error::Refused(_))) | ("format", Err(Error::Format { .. })) => {}
            _ => panic!("{evil}: {r:?}"),
        }
        assert_eq!(snap(&c), before);
        assert!(!c.join("fine.txt").exists(), "{evil}: nothing may be written");
    }
    // case twins in the ZIP
    let c = t.path().join("c-twins");
    client_in(&c);
    let p = t.path().join("twins.patch");
    handmade_patch(&p, &[], None, &[("a.txt", b"1"), ("A.TXT", b"2")]);
    assert!(matches!(apply(&p, &c, &[]), Err(Error::Refused(_))));
    assert!(!c.join("a.txt").exists());
}

#[test]
fn a_stored_zip_from_the_testkit_applies() {
    let t = TempDir::new("p-stored");
    let c = t.path().join("c");
    client_in(&c);
    let p = t.path().join("ps0003.patch");
    handmade_patch(&p, &[("Item/Item.SData", b"stored!"), ("interface/new.dds", b"dds")], None, &[("notice.txt", b"hello"), ("remove.txt", b"keep/me.txt\n")]);
    let r = apply(&p, &c, &[]).unwrap();
    assert_eq!((r.replaced.len(), r.added.len(), r.removed.len()), (1, 1, 1));
    let a = Archive::open(&c).unwrap();
    assert_eq!(a.read("Item/Item.SData").unwrap(), b"stored!");
    assert_eq!(a.read("interface/new.dds").unwrap(), b"dds");
    assert!(a.index().get("keep/me.txt").is_none());
    assert_eq!(fs::read(c.join("notice.txt")).unwrap(), b"hello");
}

#[test]
fn builds_are_deterministic_and_zip_times_are_fixed() {
    let t = TempDir::new("p-det");
    let files = t.path().join("files");
    fs::create_dir_all(files.join("a")).unwrap();
    fs::write(files.join("a/x.bin"), vec![7u8; 5000]).unwrap();
    fs::write(files.join("a/y.bin"), b"yy").unwrap();
    let mk = |name: &str, ts: Option<i32>| {
        let out = t.path().join(name);
        let input = BuildInput::from_folders(Some(&files), None, None, ts).unwrap();
        build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).unwrap();
        fs::read(out).unwrap()
    };
    let a = mk("one.patch", Some(100));
    let b = mk("two.patch", Some(100));
    assert_eq!(a, b, "the same inputs give the same bytes");
    assert_ne!(a, mk("three.patch", Some(101)), "the entry version is part of the content");
    // ZIP DOS time of every entry is 1980-01-01 00:00:00 (date 0x0021, time 0)
    let mut at = 0;
    let mut n = 0;
    while let Some(p) = a[at..].windows(4).position(|w| w == [0x50, 0x4b, 0x03, 0x04]) {
        let h = at + p;
        assert_eq!((u16::from_le_bytes([a[h + 10], a[h + 11]]), u16::from_le_bytes([a[h + 12], a[h + 13]])), (0, 0x0021));
        n += 1;
        at = h + 4;
    }
    assert_eq!(n, 2, "update.sah and update.saf");
    // existing outputs are refused unless overwriting
    let input = BuildInput::from_folders(Some(&files), None, None, Some(100)).unwrap();
    let out = t.path().join("one.patch");
    assert!(matches!(build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()), Err(Error::Refused(_))));
    build_with(&input, &out, &BuildOptions { existing: Existing::Overwrite, sign: None }, &Job::silent()).unwrap();
    assert_eq!(fs::read(&out).unwrap(), a);
    assert_eq!(fs::read_dir(t.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".shaiya-tmp")).count(), 0);
}

fn peak_rss_kb() -> Option<u64> {
    let s = fs::read_to_string("/proc/self/status").ok()?;
    s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse().ok())
}

#[test]
fn a_large_build_streams_with_flat_memory() {
    let t = TempDir::new("p-big");
    // a repeating-pattern source file written once and used three times (Source::Range into it)
    let big = t.path().join("big.bin");
    let mb: u64 = if cfg!(target_os = "linux") { 300 } else { 24 };
    {
        use std::io::Write;
        let mut f = fs::File::create(&big).unwrap();
        let block: Vec<u8> = (0..1 << 20).map(|i: u32| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        for _ in 0..mb {
            f.write_all(&block).unwrap();
        }
    }
    let third = ((mb << 20) / 3) as u32;
    let mut input = BuildInput::default();
    for i in 0..3u64 {
        input.files.push((format!("big/part{i}.bin"), Source::Range { file: big.clone(), offset: i * third as u64, length: third }, 1));
    }
    let out = t.path().join("big.patch");
    let r = build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).unwrap();
    assert_eq!(r.added.len(), 3);
    assert_eq!(r.bytes, third as u64 * 3);
    let c = contents(&out).unwrap();
    assert_eq!(c.files.len(), 3);
    assert!(c.unpacked_size >= third as u64 * 3);
    if let Some(kb) = peak_rss_kb() {
        assert!(kb < 128 * 1024, "peak RSS {kb} kB while building a {mb} MB patch");
    }
}

#[test]
fn from_diff_builds_what_turns_one_archive_into_the_other() {
    let t = TempDir::new("p-diff");
    let (a, b) = (t.path().join("old"), t.path().join("new"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    SahBuilder::new(0).file(b"x/same.txt", b"same").file(b"x/changed.txt", b"before").file(b"x/gone.txt", b"bye").write_to(&a);
    SahBuilder::new(0).file(b"x/same.txt", b"same").file(b"x/changed.txt", b"after!").file(b"x/new.txt", b"hello").write_to(&b);
    let (old, new) = (Archive::open(&a).unwrap(), Archive::open(&b).unwrap());
    let input = BuildInput::from_diff(&old, &new, &Filter::default(), &Job::silent()).unwrap();
    let mut files: Vec<&str> = input.files.iter().map(|f| f.0.as_str()).collect();
    files.sort();
    assert_eq!(files, vec!["x/changed.txt", "x/new.txt"]);
    assert_eq!(input.removals, vec!["x/gone.txt"]);
    let out = t.path().join("ps0002.patch");
    build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).unwrap();
    drop((old, new));
    // applying it to the old client gives the new archive
    apply(&out, &a, &[]).unwrap();
    let r = Archive::open(&a).unwrap();
    assert_eq!(r.read("x/changed.txt").unwrap(), b"after!");
    assert_eq!(r.read("x/new.txt").unwrap(), b"hello");
    assert!(r.index().get("x/gone.txt").is_none());
    assert_eq!(r.read("x/same.txt").unwrap(), b"same");
}

#[test]
fn verify_finds_structure_problems_and_signatures_and_the_applied_state() {
    let t = TempDir::new("p-verify");
    let p = make_patch(t.path(), "ps0109.patch", b"new item data!");
    let (secret, public) = sign::keygen();
    let key = VerifyKey { public_hex: public.clone(), check_version: 3 };
    // structure OK, signature states
    let r = verify(&p, Some(&key), None, &Job::silent()).unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(r.signature, SigStatus::Missing);
    assert_eq!(verify(&p, None, None, &Job::silent()).unwrap().signature, SigStatus::NotChecked);
    let sig = sign::sign_file(&secret, 3, &p, None, Existing::Refuse, &Job::silent()).unwrap();
    assert_eq!(sig, sign_path(&p));
    assert_eq!(verify(&p, Some(&key), None, &Job::silent()).unwrap().signature, SigStatus::Valid);
    let wrong_version = VerifyKey { public_hex: public.clone(), check_version: 4 };
    assert!(matches!(verify(&p, Some(&wrong_version), None, &Job::silent()).unwrap().signature, SigStatus::Invalid(_)));
    let (_, other_pub) = sign::keygen();
    assert!(matches!(verify(&p, Some(&VerifyKey { public_hex: other_pub, check_version: 3 }), None, &Job::silent()).unwrap().signature, SigStatus::Invalid(_)));
    // a renamed copy does not verify with the old signature (the name is signed)
    let renamed = t.path().join("ps0110.patch");
    fs::copy(&p, &renamed).unwrap();
    fs::copy(&sig, sign_path(&renamed)).unwrap();
    assert!(matches!(verify(&renamed, Some(&key), None, &Job::silent()).unwrap().signature, SigStatus::Invalid(_)));
    // sign_file never replaces unless told to
    assert!(matches!(sign::sign_file(&secret, 3, &p, None, Existing::Refuse, &Job::silent()), Err(Error::Refused(_))));
    sign::sign_file(&secret, 3, &p, None, Existing::Overwrite, &Job::silent()).unwrap();
    assert!(sign::verify_file(&public, 3, &p, None, &Job::silent()).is_ok());

    // structure failures: truncated ZIP, update.saf shorter than the index, a bad remove path
    let bytes = fs::read(&p).unwrap();
    let cut = t.path().join("cut.patch");
    fs::write(&cut, truncate(&bytes, bytes.len() / 2)).unwrap();
    assert!(!verify(&cut, None, None, &Job::silent()).unwrap().problems.is_empty());
    let short = t.path().join("short.patch");
    handmade_patch(&short, &[("a/b.bin", b"0123456789")], Some(b"0123"), &[]);
    let r = verify(&short, None, None, &Job::silent()).unwrap();
    assert!(r.problems.iter().any(|m| m.contains("update.saf")), "{:?}", r.problems);
    let badrm = t.path().join("badrm.patch");
    handmade_patch(&badrm, &[], None, &[("remove.txt", b"../../etc/x\nok/file.txt\n")]);
    assert!(verify(&badrm, None, None, &Job::silent()).unwrap().problems.iter().any(|m| m.contains("remove.txt")));
    let nosah = t.path().join("nosah.patch");
    ZipBuilder::new().add("hello.txt", b"x").write_to(&nosah);
    assert!(!verify(&nosah, None, None, &Job::silent()).unwrap().problems.is_empty());
    assert!(verify(&t.path().join("missing.patch"), None, None, &Job::silent()).is_err());

    // the applied check: not applied before, applied after, and a replaced file shows up
    let c = t.path().join("c");
    client_in(&c);
    let before = verify(&p, None, Some(&c), &Job::silent()).unwrap().not_applied.unwrap();
    assert!(before.iter().any(|m| m.contains("Item/Item.SData")) && before.iter().any(|m| m.contains("gone.txt")), "{before:?}");
    apply_with(&p, &c, &opts(&c.join(journal::DEFAULT_DIR)), &Job::silent()).unwrap();
    let after = verify(&p, None, Some(&c), &Job::silent()).unwrap().not_applied.unwrap();
    assert!(after.is_empty(), "{after:?}");
    // a later patch replaces one file with different bytes: it is listed
    let p2 = make_patch(t.path(), "ps0111.patch", b"other item!!");
    apply(&p2, &c, &[]).unwrap();
    let late = verify(&p, None, Some(&c), &Job::silent()).unwrap().not_applied.unwrap();
    // the replaced item and the loose notice.txt that the later patch also rewrote
    assert_eq!(late.len(), 2, "{late:?}");
    assert!(late.iter().any(|m| m.contains("Item/Item.SData")) && late.iter().any(|m| m.contains("notice.txt")));
    // a loose file edited by hand is listed too
    fs::write(c.join("notice.txt"), b"hand edited").unwrap();
    let late = verify(&p2, None, Some(&c), &Job::silent()).unwrap().not_applied.unwrap();
    assert!(late.iter().any(|m| m.contains("notice.txt")), "{late:?}");
}

#[test]
fn a_required_signature_is_checked_before_anything_is_written() {
    let t = TempDir::new("p-reqsig");
    let c = t.path().join("c");
    client_in(&c);
    let before = snap(&c);
    let p = make_patch(t.path(), "ps0109.patch", b"new item data!");
    let (secret, public) = sign::keygen();
    let key = VerifyKey { public_hex: public, check_version: 3 };
    let mut o = opts(&c.join(journal::DEFAULT_DIR));
    o.require_signature = Some(key.clone());
    // missing, wrong version, tampered
    assert!(matches!(apply_with(&p, &c, &o, &Job::silent()), Err(Error::Refused(_))));
    sign::sign_file(&secret, 4, &p, None, Existing::Refuse, &Job::silent()).unwrap();
    assert!(matches!(apply_with(&p, &c, &o, &Job::silent()), Err(Error::Refused(_))));
    sign::sign_file(&secret, 3, &p, None, Existing::Overwrite, &Job::silent()).unwrap();
    let good = fs::read(&p).unwrap();
    fs::write(&p, flip(&good, good.len() / 3)).unwrap();
    assert!(matches!(apply_with(&p, &c, &o, &Job::silent()), Err(Error::Refused(_))));
    assert_eq!(snap(&c), before);
    assert!(!c.join(journal::DEFAULT_DIR).exists());
    // the real file applies
    fs::write(&p, &good).unwrap();
    apply_with(&p, &c, &o, &Job::silent()).unwrap();
    assert_eq!(Archive::open(&c).unwrap().read("item/item.sdata").unwrap(), b"new item data!");
}

#[test]
fn building_with_signing_writes_a_verifying_sig_and_refuses_to_replace_it() {
    let t = TempDir::new("p-buildsign");
    let files = t.path().join("files");
    fs::create_dir_all(&files).unwrap();
    fs::write(files.join("a.txt"), b"a").unwrap();
    let (secret, public) = sign::keygen();
    let input = BuildInput::from_folders(Some(&files), None, None, Some(1)).unwrap();
    let out = t.path().join(patch_name(7));
    build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: Some((secret.clone(), 3)) }, &Job::silent()).unwrap();
    assert!(sign::verify_file(&public, 3, &out, None, &Job::silent()).is_ok());
    // a second build over it with Overwrite re-signs; Refuse stops on the output first
    assert!(build_with(&input, &out, &BuildOptions { existing: Existing::Refuse, sign: Some((secret, 3)) }, &Job::silent()).is_err());
}

#[test]
fn key_files_are_never_overwritten() {
    let t = TempDir::new("p-keys");
    let (sk, pk) = (t.path().join("secret.key"), t.path().join("public.key"));
    let public = sign::write_key_pair(&sk, &pk).unwrap();
    assert_eq!(sign::read_key(&pk).unwrap(), public);
    assert_eq!(sign::read_key(&sk).unwrap().len(), 64);
    let before = fs::read(&sk).unwrap();
    assert!(matches!(sign::write_key_pair(&sk, &pk), Err(Error::Refused(_))));
    assert!(matches!(sign::write_key_pair(&sk, &t.path().join("other.key")), Err(Error::Refused(_))));
    assert!(!t.path().join("other.key").exists());
    assert_eq!(fs::read(&sk).unwrap(), before);
    fs::write(t.path().join("bad.key"), "not a key").unwrap();
    assert!(matches!(sign::read_key(&t.path().join("bad.key")), Err(Error::Format { .. })));
    assert!(sign::read_key(&t.path().join("none.key")).is_err());
    // the pair works
    let secret = sign::read_key(&sk).unwrap();
    let f = t.path().join("file.bin");
    fs::write(&f, b"data").unwrap();
    sign::sign_file(&secret, 3, &f, Some("ps0001.patch"), Existing::Refuse, &Job::silent()).unwrap();
    assert!(sign::verify_file(&public, 3, &f, Some("ps0001.patch"), &Job::silent()).is_ok());
    assert!(sign::verify_file(&public, 3, &f, None, &Job::silent()).is_err(), "signed under another name");
}

#[test]
fn damaged_patch_indexes_and_zips_never_panic() {
    let t = TempDir::new("p-robust");
    let p = t.path().join("small.patch");
    handmade_patch(&p, &[("a/b.bin", b"0123456789"), ("c.txt", b"x")], None, &[("n.txt", b"loose"), ("remove.txt", b"z/y.txt\n")]);
    let bytes = fs::read(&p).unwrap();
    let probe = t.path().join("probe.patch");
    for at in 0..bytes.len() {
        for mutated in [flip(&bytes, at), truncate(&bytes, at)] {
            fs::write(&probe, &mutated).unwrap();
            let _ = contents(&probe);
            let _ = verify(&probe, None, None, &Job::silent());
        }
    }
}
