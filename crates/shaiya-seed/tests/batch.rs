//! Batch SData work: every status, `--like`, outputs, existing files, in-place backups.

use shaiya_core::fs::Existing;
use shaiya_core::source::{ByteSource, FolderSource, MemorySource};
use shaiya_core::{glob::Filter, Error, Job};
use shaiya_seed::batch::{run, BatchOptions, ItemResult, Op, Output, Status};
use shaiya_seed::{encrypt, is_encrypted, unwrap, wrap, Wrapper};
use shaiya_testkit::TempDir;
use std::path::Path;

fn mem(items: &[(&str, Vec<u8>)]) -> MemorySource {
    MemorySource { label: "test".into(), items: items.iter().map(|(n, d)| (n.to_string(), d.clone())).collect() }
}

fn opts<'a>(op: Op<'a>, output: Output) -> BatchOptions<'a> {
    BatchOptions { op, output, existing: Existing::Refuse, pass_through: true, dry_run: false }
}

fn by<'a>(r: &'a [ItemResult], name: &str) -> &'a ItemResult {
    r.iter().find(|i| i.name == name).unwrap()
}

/// An encrypted file with a non-zero header padding and a non-zero tail, so wrappers are told apart from the default.
fn odd_seed(plain: &[u8]) -> Vec<u8> {
    let w = Wrapper::Seed { crc: 0, size: plain.len() as u32, padding: [9; 16], tail: vec![7; (16 - plain.len() % 16) % 16] };
    wrap(plain, &w)
}

#[test]
fn info_and_check_report_every_state_without_writing() {
    let good = encrypt(b"plaintext of some length");
    let mut bad = encrypt(b"plaintext of some length");
    bad[40] ^= 1; // the CRC field
    let odd = odd_seed(b"0123456789");
    let src = mem(&[("good.sdata", good), ("bad.sdata", bad), ("plain.sdata", b"not encrypted".to_vec()), ("odd.sdata", odd)]);
    let r = run(&src, &opts(Op::Info, Output::None), &Job::silent()).unwrap();
    assert_eq!(by(&r, "good.sdata").status, Status::Encrypted);
    assert_eq!((by(&r, "good.sdata").crc_ok, by(&r, "good.sdata").padding_zero, by(&r, "good.sdata").tail_zero, by(&r, "good.sdata").plain_size), (Some(true), Some(true), Some(true), Some(24)));
    assert_eq!(by(&r, "bad.sdata").crc_ok, Some(false));
    assert_eq!(by(&r, "plain.sdata").status, Status::Plain);
    assert_eq!((by(&r, "odd.sdata").padding_zero, by(&r, "odd.sdata").tail_zero), (Some(false), Some(false)));
    let c = run(&src, &opts(Op::Check, Output::None), &Job::silent()).unwrap();
    assert_eq!((by(&c, "good.sdata").status, by(&c, "bad.sdata").status, by(&c, "plain.sdata").status), (Status::CrcOk, Status::CrcBad, Status::Plain));
    // Info and Check never write, whatever the output says
    let d = TempDir::new("batch-nowrite");
    run(&src, &opts(Op::Check, Output::Dir(d.path().join("o"))), &Job::silent()).unwrap();
    assert!(!d.path().join("o").exists());
}

#[test]
fn decrypt_reaches_every_status() {
    let d = TempDir::new("batch-decrypt");
    let out = d.path().join("plain");
    let good = encrypt(b"good plaintext");
    let mut bad = encrypt(b"bad plaintext!");
    bad[40] ^= 1;
    let src = mem(&[("a/good.sdata", good.clone()), ("bad.sdata", bad.clone()), ("plain.sdata", b"already plain".to_vec())]);
    // strict: the bad one fails, the batch continues; the plain one is copied
    let r = run(&src, &opts(Op::Decrypt { unchecked: false }, Output::Dir(out.clone())), &Job::silent()).unwrap();
    assert_eq!(by(&r, "a/good.sdata").status, Status::Decrypted);
    assert_eq!(by(&r, "bad.sdata").status, Status::Failed);
    assert!(by(&r, "bad.sdata").message.as_ref().unwrap().contains("checksum"));
    assert_eq!(by(&r, "plain.sdata").status, Status::PassedThrough);
    assert_eq!(std::fs::read(out.join("a/good.sdata")).unwrap(), b"good plaintext");
    assert_eq!(std::fs::read(out.join("plain.sdata")).unwrap(), b"already plain");
    assert!(!out.join("bad.sdata").exists());
    // unchecked writes the damaged one and says so
    let out2 = d.path().join("plain2");
    let r = run(&src, &BatchOptions { pass_through: false, ..opts(Op::Decrypt { unchecked: true }, Output::Dir(out2.clone())) }, &Job::silent()).unwrap();
    assert_eq!(by(&r, "bad.sdata").status, Status::DecryptedCrcBad);
    assert_eq!(by(&r, "plain.sdata").status, Status::SkippedUnchanged);
    assert_eq!(std::fs::read(out2.join("bad.sdata")).unwrap(), b"bad plaintext!");
    assert!(!out2.join("plain.sdata").exists(), "skipped, not copied");
    // existing: Refuse stops before any write, Skip leaves the file, Overwrite replaces it
    let e = run(&src, &opts(Op::Decrypt { unchecked: false }, Output::Dir(out.clone())), &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Refused(_)));
    std::fs::write(out.join("a/good.sdata"), b"mine").unwrap();
    let r = run(&src, &BatchOptions { existing: Existing::Skip, ..opts(Op::Decrypt { unchecked: false }, Output::Dir(out.clone())) }, &Job::silent()).unwrap();
    assert_eq!(by(&r, "a/good.sdata").status, Status::SkippedExisting);
    assert_eq!(std::fs::read(out.join("a/good.sdata")).unwrap(), b"mine");
    let r = run(&src, &BatchOptions { existing: Existing::Overwrite, ..opts(Op::Decrypt { unchecked: false }, Output::Dir(out.clone())) }, &Job::silent()).unwrap();
    assert_eq!(by(&r, "a/good.sdata").status, Status::Decrypted);
    assert_eq!(std::fs::read(out.join("a/good.sdata")).unwrap(), b"good plaintext");
    // a dry run reports the same statuses and writes nothing
    let out3 = d.path().join("plain3");
    let r = run(&src, &BatchOptions { dry_run: true, ..opts(Op::Decrypt { unchecked: false }, Output::Dir(out3.clone())) }, &Job::silent()).unwrap();
    assert_eq!(by(&r, "a/good.sdata").status, Status::Decrypted);
    assert_eq!(by(&r, "a/good.sdata").out, Some(out3.join("a").join("good.sdata")));
    assert!(!out3.exists());
}

#[test]
fn encrypt_default_and_like_round_trip_byte_for_byte() {
    let d = TempDir::new("batch-like");
    // originals with odd headers and tails, plus a plain one (cn style)
    let originals = mem(&[("Item/Item.SData", odd_seed(b"item data here")), ("Monster/M.SData", odd_seed(b"0123456789abcdef")), ("Cn/Plain.SData", b"plain cn data".to_vec())]);
    let plain_dir = d.path().join("plain");
    let r = run(&originals, &opts(Op::Decrypt { unchecked: false }, Output::Dir(plain_dir.clone())), &Job::silent()).unwrap();
    assert!(r.iter().all(|i| i.status != Status::Failed));
    // encrypt back with --like: identical to the originals
    let src = FolderSource::new(&[plain_dir.clone()], &Filter::default()).unwrap();
    let enc = d.path().join("enc");
    let r = run(&src, &opts(Op::Encrypt { like: Some(&originals) }, Output::Dir(enc.clone())), &Job::silent()).unwrap();
    assert_eq!(by(&r, "Item/Item.SData").status, Status::EncryptedNew);
    assert_eq!(by(&r, "Cn/Plain.SData").status, Status::PassedThrough, "plain stays plain");
    for (n, orig) in &originals.items {
        assert_eq!(&std::fs::read(enc.join(n)).unwrap(), orig, "{n} must be byte-identical");
    }
    // without --like the header is the default one: it decrypts to the same plaintext but is not byte-identical for odd originals
    let enc2 = d.path().join("enc2");
    run(&src, &opts(Op::Encrypt { like: None }, Output::Dir(enc2.clone())), &Job::silent()).unwrap();
    let new = std::fs::read(enc2.join("Item/Item.SData")).unwrap();
    assert!(is_encrypted(&new));
    assert_eq!(unwrap(&new, "x").unwrap().0, b"item data here");
    assert_ne!(new, originals.items[0].1);
    // a file-name match works when the folder layout differs; an ambiguous or missing original falls back to the default header
    let flat = mem(&[("Item.sdata", b"item data here".to_vec()), ("Unknown.SData", b"x".to_vec())]);
    let r = run(&flat, &opts(Op::Encrypt { like: Some(&originals) }, Output::None), &Job::silent()).unwrap();
    assert_eq!(by(&r, "Item.sdata").status, Status::EncryptedNew);
    assert!(by(&r, "Item.sdata").message.is_none(), "found by file name");
    assert!(by(&r, "Unknown.SData").message.as_ref().unwrap().contains("no original"));
    // encrypted input is passed through or skipped
    let again = mem(&[("e.sdata", encrypt(b"abc"))]);
    assert_eq!(run(&again, &opts(Op::Encrypt { like: None }, Output::Dir(d.path().join("o3"))), &Job::silent()).unwrap()[0].status, Status::PassedThrough);
    assert_eq!(run(&again, &BatchOptions { pass_through: false, ..opts(Op::Encrypt { like: None }, Output::Dir(d.path().join("o4"))) }, &Job::silent()).unwrap()[0].status, Status::SkippedUnchanged);
}

#[test]
fn a_single_file_output_needs_exactly_one_name() {
    let d = TempDir::new("batch-file");
    let two = mem(&[("a.sdata", encrypt(b"a")), ("b.sdata", encrypt(b"b"))]);
    let e = run(&two, &opts(Op::Decrypt { unchecked: false }, Output::File(d.path().join("x"))), &Job::silent()).unwrap_err();
    assert!(matches!(e, Error::Refused(_)));
    let one = mem(&[("a.sdata", encrypt(b"a"))]);
    let r = run(&one, &opts(Op::Decrypt { unchecked: false }, Output::File(d.path().join("x.bin"))), &Job::silent()).unwrap();
    assert_eq!(r[0].status, Status::Decrypted);
    assert_eq!(std::fs::read(d.path().join("x.bin")).unwrap(), b"a");
}

#[test]
fn in_place_makes_a_backup_first_and_needs_files_on_disk() {
    let d = TempDir::new("batch-inplace");
    let f = d.path().join("Item.SData");
    let original = encrypt(b"encrypted original");
    std::fs::write(&f, &original).unwrap();
    let src = FolderSource::new(&[f.clone()], &Filter::default()).unwrap();
    let r = run(&src, &opts(Op::Decrypt { unchecked: false }, Output::InPlace), &Job::silent()).unwrap();
    assert_eq!(r[0].status, Status::Decrypted);
    assert_eq!(std::fs::read(&f).unwrap(), b"encrypted original");
    assert_eq!(std::fs::read(d.path().join("Item.SData.bak")).unwrap(), original, "the original is kept");
    // memory has no file to change
    let m = mem(&[("a.sdata", encrypt(b"a"))]);
    let r = run(&m, &opts(Op::Decrypt { unchecked: false }, Output::InPlace), &Job::silent()).unwrap();
    assert_eq!(r[0].status, Status::Failed);
    // dry run touches nothing
    let g = d.path().join("G.SData");
    std::fs::write(&g, &original).unwrap();
    let src = FolderSource::new(&[g.clone()], &Filter::default()).unwrap();
    run(&src, &BatchOptions { dry_run: true, ..opts(Op::Decrypt { unchecked: false }, Output::InPlace) }, &Job::silent()).unwrap();
    assert_eq!(std::fs::read(&g).unwrap(), original);
    assert!(!d.path().join("G.SData.bak").exists());
}

#[test]
fn an_unreadable_source_item_is_a_failed_item_not_an_error() {
    struct Broken;
    impl ByteSource for Broken {
        fn label(&self) -> String {
            "broken".into()
        }
        fn names(&self) -> Vec<String> {
            vec!["x.sdata".into()]
        }
        fn size(&self, _: &str) -> Option<u64> {
            None
        }
        fn read(&self, name: &str) -> shaiya_core::Result<Vec<u8>> {
            Err(Error::io(Path::new(name), std::io::Error::other("disk on fire")))
        }
    }
    let r = run(&Broken, &opts(Op::Check, Output::None), &Job::silent()).unwrap();
    assert_eq!(r[0].status, Status::Failed);
    assert!(r[0].message.as_ref().unwrap().contains("disk on fire"));
    let c = shaiya_core::Cancel::new();
    c.cancel();
    assert!(matches!(run(&Broken, &opts(Op::Check, Output::None), &Job::new(&shaiya_core::Silent, c)), Err(Error::Cancelled)));
}
