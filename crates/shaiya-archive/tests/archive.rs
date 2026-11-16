//! Index and archive tests on synthetic data (built by `shaiya-testkit`, which writes the format by hand), plus the opt-in test on the
//! user's own client (`SHAIYA_CLIENT`).

use shaiya_archive::{decode_name, Archive, Entry, Index};
use shaiya_core::{Cancel, Error, Job, Silent};
use shaiya_testkit::{flip, truncate, SahBuilder, TempDir};

/// cp949 bytes of the Korean word for "monster" (몬스터).
const MONSTER: &[u8] = &[0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD];

fn path_with(parts: &[&[u8]]) -> Vec<u8> {
    parts.join(&b'/')
}

fn sample(version: i32) -> SahBuilder {
    SahBuilder::new(version)
        .file(b"Item/Item.SData", b"item bytes")
        .file(b"interface/icon/29.dds", b"icon")
        .file(&path_with(&[b"Monster", MONSTER, b"a.dds"]), b"korean folder")
        .file(&path_with(&[b"Monster", &[MONSTER, b".txt"].concat()]), b"korean name")
        .file(b"top.txt", b"")
        .free(b"rest space", 1000, 24)
        .trailer(&[0, 0, 0, 0])
}

#[test]
fn parse_then_to_bytes_is_identical_for_versions_0_and_1() {
    for version in [0, 1] {
        let (sah, _) = sample(version).build();
        let idx = Index::parse(&sah).unwrap();
        assert_eq!(idx.version, version);
        assert_eq!(idx.file_count(), 5);
        assert_eq!(idx.free.len(), 1);
        assert_eq!(idx.to_bytes(), sah, "version {version}");
    }
}

#[test]
fn an_odd_trailer_survives() {
    let (sah, _) = sample(0).trailer(&[9, 8, 7, 6, 5, 4]).build();
    let idx = Index::parse(&sah).unwrap();
    assert_eq!(idx.trailer, vec![9, 8, 7, 6, 5, 4]);
    assert_eq!(idx.to_bytes(), sah);
}

#[test]
fn archive_reads_every_file_and_korean_names_are_shown_decoded() {
    let dir = TempDir::new("archive");
    let (sah_path, saf_path) = sample(0).write_to(dir.path());
    for open in [dir.path().to_path_buf(), sah_path.clone()] {
        let a = Archive::open(&open).unwrap();
        assert_eq!(a.sah_path(), sah_path);
        assert_eq!(a.saf_path(), saf_path);
        assert_eq!(a.saf_len(), std::fs::metadata(&saf_path).unwrap().len());
        assert_eq!(a.read("item/ITEM.SDATA").unwrap(), b"item bytes");
        assert_eq!(a.read(r"Interface\Icon\29.dds").unwrap(), b"icon");
        assert_eq!(a.read("top.txt").unwrap(), b"");
        let korean = decode_name(MONSTER);
        assert_eq!(a.read(&format!("monster/{korean}/a.dds")).unwrap(), b"korean folder");
        assert_eq!(a.read(&format!("Monster/{korean}.txt")).unwrap(), b"korean name");
        assert!(a.index().files().iter().any(|(p, _)| p == &format!("Monster/{korean}.txt")));
        let err = a.read("nope/missing.bin").unwrap_err();
        assert_eq!(err.kind(), shaiya_core::ErrorKind::Io);
    }
}

#[test]
fn copy_entry_streams_and_cancels() {
    let dir = TempDir::new("copy");
    let big: Vec<u8> = (0..2_300_000u32).map(|i| (i % 251) as u8).collect();
    SahBuilder::new(0).file(b"a/small", b"xyz").file(b"a/big.bin", &big).write_to(dir.path());
    let a = Archive::open(dir.path()).unwrap();
    let mut out = Vec::new();
    let n = a.copy_entry(a.index().get("a/big.bin").unwrap(), &mut out, &Job::silent()).unwrap();
    assert_eq!((n, out == big), (2_300_000, true));
    let c = Cancel::new();
    c.cancel();
    let mut sink = Vec::new();
    assert!(matches!(a.copy_entry(a.index().get("a/big.bin").unwrap(), &mut sink, &Job::new(&Silent, c)), Err(Error::Cancelled)));
    assert!(sink.is_empty());
}

#[test]
fn entries_outside_the_saf_are_refused() {
    let dir = TempDir::new("bounds");
    SahBuilder::new(0).file(b"f", b"abcd").write_to(dir.path());
    let a = Archive::open(dir.path()).unwrap();
    let bad = Entry { name: b"f".to_vec(), offset: 2, length: 100, version: 0 };
    assert!(matches!(a.read_entry(&bad), Err(Error::Format { offset: Some(2), .. })));
    let huge = Entry { name: b"f".to_vec(), offset: u64::MAX - 1, length: 10, version: 0 };
    assert!(a.read_entry(&huge).is_err());
    assert!(a.copy_entry(&bad, &mut Vec::new(), &Job::silent()).is_err());
}

#[test]
fn a_folder_without_an_index_or_saf_is_an_io_error() {
    let dir = TempDir::new("missing");
    assert_eq!(Archive::open(dir.path()).err().map(|e| e.kind()), Some(shaiya_core::ErrorKind::Io));
    let (sah, _) = SahBuilder::new(0).file(b"f", b"1").build();
    std::fs::write(dir.path().join("data.sah"), sah).unwrap();
    assert_eq!(Archive::open(dir.path()).err().map(|e| e.kind()), Some(shaiya_core::ErrorKind::Io));
}

#[test]
fn every_truncation_of_a_sah_is_an_error() {
    let (sah, _) = sample(0).build();
    for cut in 0..sah.len() {
        let t = truncate(&sah, cut);
        // the free list and trailer are optional trailing parts: cutting exactly before them (or inside the trailer) still parses
        // as an index with fewer parts, and then must not equal the original
        match Index::parse(&t) {
            Err(_) => {}
            Ok(idx) => assert_ne!(idx.to_bytes(), sah, "cut at {cut} parsed as the full index"),
        }
    }
    // cuts inside the file tree must fail
    for cut in 0..=60 {
        assert!(Index::parse(&truncate(&sah, cut)).is_err(), "cut at {cut}");
    }
}

#[test]
fn a_bit_flip_never_panics() {
    let (sah, _) = sample(1).build();
    for i in 0..sah.len() {
        let _ = Index::parse(&flip(&sah, i));
        let mut m = sah.clone();
        m[i] = 0xFF;
        let _ = Index::parse(&m);
    }
}

#[test]
fn a_huge_count_is_refused_without_allocating() {
    let (mut sah, _) = sample(0).build();
    // the root's file count sits right after the 51-byte header and the 5-byte root name
    sah[56..60].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(Index::parse(&sah), Err(Error::Format { .. })));
}

#[test]
fn the_declared_file_count_must_match() {
    let (mut sah, _) = sample(0).build();
    sah[7] ^= 1;
    let e = Index::parse(&sah).unwrap_err();
    assert!(e.to_string().contains("header says"), "{e}");
}

#[test]
fn opt_in_client_rewrite_is_byte_identical() {
    let Some(client) = shaiya_testkit::env_client() else { return };
    let sah_path = ["data.sah", "Data.sah", "DATA.SAH"].iter().map(|n| client.join(n)).find(|p| p.is_file());
    let Some(sah_path) = sah_path else {
        eprintln!("skipped: no data.sah in SHAIYA_CLIENT");
        return;
    };
    let raw = std::fs::read(&sah_path).unwrap();
    let t = std::time::Instant::now();
    let idx = Index::parse(&raw).unwrap();
    let parse = t.elapsed();
    assert_eq!(idx.to_bytes(), raw, "rewriting the client index must be byte-identical");
    eprintln!("SAH parse: {} files, {} bytes, {parse:?}", idx.file_count(), raw.len());
    let a = Archive::open(&client).unwrap();
    let (_, e) = idx.files().into_iter().next().unwrap();
    let bytes = a.read_entry(e).unwrap();
    assert_eq!(bytes.len(), e.length as usize);
}
