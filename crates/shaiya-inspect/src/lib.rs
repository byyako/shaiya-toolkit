//! Identify a Shaiya file and describe it: SAH index, SAF data, SData (SEED-encrypted or plain), patch, patch signature, Windows
//! program. Detection is by content first, name second. Malformed input gives a `problems` entry, not a failure, unless the file cannot
//! be read at all.

use serde::Serialize;
use shaiya_core::hash::{sha256_file, to_hex};
use shaiya_core::time::utc_iso_unix;
use shaiya_core::{Error, Job, Result};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// SEED files over this size are not decrypted (a problem line is reported instead).
const SEED_LIMIT: u64 = 512 << 20;
/// SAH indexes are read whole; real ones are under 2 MB.
const SAH_LIMIT: u64 = 256 << 20;
/// How much of the start of a file detection looks at.
const HEAD: usize = 64 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Sah,
    Saf,
    SdataSeed,
    SdataPlain,
    Patch,
    PatchSignature,
    Pe,
    Unknown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Details {
    Sah { version: i32, files: usize, dirs: usize, free_entries: usize, trailer_len: usize, data_end: u64 },
    /// From a sibling .sah when present.
    Saf { index_files: Option<usize>, live_bytes: Option<u64> },
    SdataSeed { crc: u32, plain_size: u32, crc_ok: bool },
    SdataPlain,
    Patch { files: usize, loose: usize, removals: usize, signature_file: bool },
    PatchSignature { hex_len: usize },
    Pe { machine: u16, machine_name: &'static str, timestamp: u32, timestamp_utc: String },
    /// First 16 bytes.
    Unknown { magic_hex: String },
}

#[derive(Debug, Serialize)]
pub struct Info {
    pub path: String,
    pub size: u64,
    pub kind: Kind,
    pub details: Details,
    pub sha256: Option<String>,
    pub problems: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub hash: bool,
}

fn machine_name(m: u16) -> &'static str {
    match m {
        0x014C => "x86",
        0x8664 => "x86_64",
        0x01C0 | 0x01C4 => "ARM",
        0xAA64 => "ARM64",
        0x0200 => "IA-64",
        _ => "unknown",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().expect("4 bytes")))
}

/// `(machine, timestamp)` when `head` starts with a valid MZ + PE header.
fn pe_header(head: &[u8]) -> Option<(u16, u32)> {
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let pe = le32(head, 0x3C)? as usize;
    if head.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let machine = u16::from_le_bytes(head.get(pe + 4..pe + 6)?.try_into().ok()?);
    Some((machine, le32(head, pe + 8)?))
}

/// True when the ZIP at `path` holds `update.sah` or `remove.txt`: those names are stored uncompressed in the local header of the first
/// entry and in the central directory at the end.
fn zip_is_patch(path: &Path, size: u64, head: &[u8]) -> bool {
    let has = |b: &[u8]| {
        let lower: Vec<u8> = b.iter().map(u8::to_ascii_lowercase).collect();
        [&b"update.sah"[..], &b"remove.txt"[..]].iter().any(|n| lower.windows(n.len()).any(|w| w == *n))
    };
    if has(&head[..head.len().min(4096)]) {
        return true;
    }
    let tail_len = size.min(HEAD as u64);
    let Ok(mut f) = File::open(path) else { return false };
    use std::io::{Seek, SeekFrom};
    if f.seek(SeekFrom::Start(size - tail_len)).is_err() {
        return false;
    }
    let mut tail = vec![0u8; tail_len as usize];
    f.read_exact(&mut tail).is_ok() && has(&tail)
}

fn count_dirs(d: &shaiya_archive::Dir) -> usize {
    d.dirs.iter().map(|c| 1 + count_dirs(c)).sum()
}

/// Bytes of `data.saf` the index uses (shared ranges counted once).
fn live_bytes(idx: &shaiya_archive::Index) -> u64 {
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
    live
}

fn sibling_with_ext(path: &Path, ext: &str) -> Option<PathBuf> {
    let direct = path.with_extension(ext);
    if direct.is_file() {
        return Some(direct);
    }
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&stem)) && has_ext(p, ext))
}

/// `1,234,567`
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", group(n), if n == 1 { one } else { many })
}

/// One line saying what the file is, the way `shaiya info` and Workbench show it
/// (`SData, SEED-encrypted, plaintext 4,512,768 bytes, checksum OK`).
pub fn describe(i: &Info) -> String {
    let size = group(i.size);
    match &i.details {
        Details::Sah { version, files, dirs, free_entries, data_end, .. } => format!(
            "SAH index, version {version}, {} in {}, {}, data ends at {}",
            counted(*files as u64, "file", "files"),
            counted(*dirs as u64, "folder", "folders"),
            counted(*free_entries as u64, "free entry", "free entries"),
            group(*data_end)
        ),
        Details::Saf { index_files, live_bytes } => match (index_files, live_bytes) {
            (Some(n), Some(l)) => format!("SAF data, {size} bytes, its index lists {} using {} bytes", counted(*n as u64, "file", "files"), group(*l)),
            _ => format!("SAF data, {size} bytes (no .sah next to it)"),
        },
        Details::SdataSeed { plain_size, crc_ok, .. } => {
            format!("SData, SEED-encrypted, plaintext {} bytes, checksum {}", group(*plain_size as u64), if *crc_ok { "OK" } else { "FAILED" })
        }
        Details::SdataPlain => format!("SData, plain (not encrypted), {size} bytes"),
        Details::Patch { files, loose, removals, signature_file } => format!(
            "Patch, {}, {}, {}, signature file {}, {size} bytes",
            counted(*files as u64, "archive file", "archive files"),
            counted(*loose as u64, "loose file", "loose files"),
            counted(*removals as u64, "removal", "removals"),
            if *signature_file { "present" } else { "not found" }
        ),
        Details::PatchSignature { hex_len } => format!("Patch signature, {hex_len} hex digits"),
        Details::Pe { machine_name, timestamp_utc, .. } => {
            format!("Windows program ({machine_name}), linked {} UTC, {size} bytes", timestamp_utc.replace('T', " ").trim_end_matches('Z'))
        }
        Details::Unknown { magic_hex } => format!("Unknown file type, starts with {magic_hex}, {size} bytes"),
    }
}

pub fn inspect(path: &Path, opts: &Options, job: &Job) -> Result<Info> {
    let mut f = File::open(path).map_err(|e| Error::io(path, e))?;
    let size = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if f.metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(Error::io(path, std::io::Error::new(std::io::ErrorKind::InvalidInput, "is a folder, not a file")));
    }
    let mut head = Vec::with_capacity(HEAD.min(size as usize));
    (&mut f).take(HEAD as u64).read_to_end(&mut head).map_err(|e| Error::io(path, e))?;
    drop(f);
    let name = file_name(path);
    let mut problems = Vec::new();

    // A <stem>.saf with its <stem>.sah beside it is SAF data whatever its first bytes look like (an archive can start with an SData file).
    let (kind, details) = if has_ext(path, "saf") && sibling_with_ext(path, "sah").is_some() {
        saf_details(path, size, &mut problems)
    } else if head.starts_with(b"SAH") {
        sah_details(path, size, &name, &mut problems)
    } else if shaiya_seed::is_encrypted(&head) {
        seed_details(path, size, &head, &name, &mut problems)
    } else if head.starts_with(b"PK\x03\x04") && zip_is_patch(path, size, &head) {
        patch_details(path, &mut problems)
    } else if let Some((machine, timestamp)) = pe_header(&head) {
        let details = Details::Pe { machine, machine_name: machine_name(machine), timestamp, timestamp_utc: utc_iso_unix(timestamp as i64) };
        (Kind::Pe, details)
    } else if has_ext(path, "saf") {
        saf_details(path, size, &mut problems)
    } else if name.to_ascii_lowercase().ends_with(".patch.sig") || (has_ext(path, "sig") && looks_like_hex_signature(&head)) {
        let text = String::from_utf8_lossy(&head);
        let hex_len = text.trim().len();
        if hex_len != 128 {
            problems.push(format!("a patch signature is 128 hex digits, this has {hex_len}"));
        }
        (Kind::PatchSignature, Details::PatchSignature { hex_len })
    } else if has_ext(path, "sdata") {
        (Kind::SdataPlain, Details::SdataPlain)
    } else {
        if head.starts_with(b"MZ") {
            problems.push("starts with MZ but has no valid PE header".to_string());
        }
        (Kind::Unknown, Details::Unknown { magic_hex: to_hex(&head[..head.len().min(16)]) })
    };

    let sha256 = if opts.hash { Some(to_hex(&sha256_file(path, job)?)) } else { None };
    Ok(Info { path: path.display().to_string(), size, kind, details, sha256, problems })
}

fn saf_details(path: &Path, size: u64, problems: &mut Vec<String>) -> (Kind, Details) {
    let sah = sibling_with_ext(path, "sah");
    let idx = sah.as_deref().and_then(|p| shaiya_archive::Index::read(p).ok());
    let details = Details::Saf { index_files: idx.as_ref().map(|i| i.file_count()), live_bytes: idx.as_ref().map(live_bytes) };
    if let Some(i) = &idx {
        let end = i.files().iter().map(|(_, e)| e.offset + e.length as u64).max().unwrap_or(0);
        if end > size {
            problems.push(format!("the index {} refers to bytes up to {end}, but this file has only {size}", sah.as_deref().map(file_name).unwrap_or_default()));
        }
    }
    (Kind::Saf, details)
}

fn looks_like_hex_signature(head: &[u8]) -> bool {
    let t = String::from_utf8_lossy(head);
    let t = t.trim();
    !t.is_empty() && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

fn sah_details(path: &Path, size: u64, name: &str, problems: &mut Vec<String>) -> (Kind, Details) {
    let empty = |version| Details::Sah { version, files: 0, dirs: 0, free_entries: 0, trailer_len: 0, data_end: 0 };
    if size > SAH_LIMIT {
        problems.push(format!("{name}: {size} bytes is too large for an index; not parsed"));
        return (Kind::Sah, empty(0));
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            problems.push(Error::io(path, e).to_string());
            return (Kind::Sah, empty(0));
        }
    };
    let version = data.get(3..7).map(|b| i32::from_le_bytes(b.try_into().expect("4 bytes"))).unwrap_or(0);
    match shaiya_archive::Index::parse_named(&data, name) {
        Ok(idx) => {
            let data_end = idx.files().iter().map(|(_, e)| e.offset + e.length as u64).max().unwrap_or(0);
            if idx.version > 1 {
                problems.push(format!("SAH version {} is newer than this build knows (0 and 1)", idx.version));
            }
            let details = Details::Sah {
                version: idx.version,
                files: idx.file_count(),
                dirs: count_dirs(&idx.root),
                free_entries: idx.free.len(),
                trailer_len: idx.trailer.len(),
                data_end,
            };
            (Kind::Sah, details)
        }
        Err(e) => {
            problems.push(e.to_string());
            (Kind::Sah, empty(version))
        }
    }
}

fn seed_details(path: &Path, size: u64, head: &[u8], name: &str, problems: &mut Vec<String>) -> (Kind, Details) {
    let crc = le32(head, 40).unwrap_or(0);
    let plain_size = le32(head, 44).unwrap_or(0);
    let mut details = Details::SdataSeed { crc, plain_size, crc_ok: false };
    if size > SEED_LIMIT {
        problems.push(format!("{name}: {size} bytes is over the {} MiB limit; checksum not checked", SEED_LIMIT >> 20));
        return (Kind::SdataSeed, details);
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            problems.push(Error::io(path, e).to_string());
            return (Kind::SdataSeed, details);
        }
    };
    match shaiya_seed::decrypt_unchecked(&data, name) {
        Ok((_, ok)) => {
            details = Details::SdataSeed { crc, plain_size, crc_ok: ok };
            if !ok {
                problems.push(format!("{name}: SEED checksum failed (damaged file or a different key)"));
            }
        }
        Err(e) => problems.push(e.to_string()),
    }
    (Kind::SdataSeed, details)
}

fn patch_details(path: &Path, problems: &mut Vec<String>) -> (Kind, Details) {
    let signature_file = {
        let mut sig = path.as_os_str().to_os_string();
        sig.push(".sig");
        PathBuf::from(sig).is_file()
    };
    match (shaiya_patch::list(path), shaiya_patch::removals(path)) {
        (Ok((files, loose)), Ok(removals)) => {
            (Kind::Patch, Details::Patch { files: files.len(), loose: loose.len(), removals: removals.len(), signature_file })
        }
        (Err(e), _) | (_, Err(e)) => {
            problems.push(e.to_string());
            (Kind::Patch, Details::Patch { files: 0, loose: 0, removals: 0, signature_file })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{pe_stub, SahBuilder, TempDir};

    fn info(path: &Path) -> Info {
        inspect(path, &Options::default(), &Job::silent()).unwrap()
    }

    #[test]
    fn sah_index_is_described() {
        let d = TempDir::new("inspect-sah");
        let (sah, saf) = SahBuilder::new(1).file(b"a/b.txt", b"hello").file(b"a/c/d.bin", &[1, 2]).file(b"top", b"T").free(b"rest", 50, 10).write_to(d.path());
        let i = info(&sah);
        assert_eq!(i.kind, Kind::Sah);
        match i.details {
            Details::Sah { version, files, dirs, free_entries, trailer_len, data_end } => {
                assert_eq!((version, files, dirs, free_entries, trailer_len, data_end), (1, 3, 2, 1, 4, 8));
            }
            other => panic!("{other:?}"),
        }
        assert!(i.problems.is_empty());
        // its saf, via the sibling index
        let s = info(&saf);
        assert_eq!(s.kind, Kind::Saf);
        assert!(matches!(s.details, Details::Saf { index_files: Some(3), live_bytes: Some(8) }));
        assert!(s.problems.is_empty());
    }

    #[test]
    fn data_saf_beside_its_sah_is_saf_even_if_it_starts_like_an_sdata() {
        let d = TempDir::new("inspect-saf-sdata-first");
        let mut first = shaiya_seed::SIGNATURE.to_vec();
        first.resize(200, 7);
        let (sah, saf) = SahBuilder::new(1).file(b"Cash.SData", &first).file(b"z.txt", b"zz").write_to(d.path());
        let _ = sah;
        let s = info(&saf);
        assert_eq!(s.kind, Kind::Saf, "{s:?}");
        assert!(matches!(s.details, Details::Saf { index_files: Some(2), .. }), "{s:?}");
        assert!(s.problems.is_empty());
        // without a data.sah beside it, content sniffing still applies
        let alone = d.path().join("alone").join("data.saf");
        std::fs::create_dir_all(alone.parent().unwrap()).unwrap();
        std::fs::write(&alone, &first).unwrap();
        assert_eq!(info(&alone).kind, Kind::SdataSeed);
    }

    #[test]
    fn any_stem_saf_beside_its_sah_is_saf_data() {
        let d = TempDir::new("inspect-saf-other-stem");
        let mut first = shaiya_seed::SIGNATURE.to_vec();
        first.resize(200, 7);
        let (sah, saf) = SahBuilder::new(1).file(b"Cash.SData", &first).file(b"z.txt", b"zz").write_to(d.path());
        let new_sah = d.path().join("New.SAH");
        let new_saf = d.path().join("New.SAF");
        std::fs::rename(&sah, &new_sah).unwrap();
        std::fs::rename(&saf, &new_saf).unwrap();
        let s = info(&new_saf);
        assert_eq!(s.kind, Kind::Saf, "{s:?}");
        assert!(matches!(s.details, Details::Saf { index_files: Some(2), .. }), "{s:?}");
    }

    #[test]
    fn a_damaged_sah_is_a_problem_not_a_failure() {
        let d = TempDir::new("inspect-bad-sah");
        let (sah, _) = SahBuilder::new(0).file(b"f", b"1").build();
        let p = d.path().join("data.sah");
        std::fs::write(&p, shaiya_testkit::truncate(&sah, 60)).unwrap();
        let i = info(&p);
        assert_eq!(i.kind, Kind::Sah);
        assert_eq!(i.problems.len(), 1);
        assert!(i.problems[0].contains("at byte"), "{}", i.problems[0]);
    }

    #[test]
    fn sdata_files_seed_and_plain() {
        let d = TempDir::new("inspect-sdata");
        let enc = shaiya_seed::encrypt(b"some record bytes");
        let p = d.path().join("Item.SData");
        std::fs::write(&p, &enc).unwrap();
        let i = info(&p);
        assert_eq!(i.kind, Kind::SdataSeed);
        assert!(matches!(i.details, Details::SdataSeed { plain_size: 17, crc_ok: true, .. }));
        // a flipped body byte fails the checksum and is reported as a problem
        let mut bad = enc.clone();
        bad[70] ^= 1;
        let q = d.path().join("Bad.SData");
        std::fs::write(&q, &bad).unwrap();
        let i = info(&q);
        assert!(matches!(i.details, Details::SdataSeed { crc_ok: false, .. }));
        assert!(i.problems[0].contains("checksum"), "{:?}", i.problems);
        // plain SData is recognised by its name
        let r = d.path().join("Plain.sdata");
        std::fs::write(&r, b"plain text").unwrap();
        assert_eq!(info(&r).kind, Kind::SdataPlain);
    }

    #[test]
    fn pe_patch_signature_and_unknown() {
        let d = TempDir::new("inspect-misc");
        let exe = d.path().join("game.exe");
        std::fs::write(&exe, pe_stub(0x014C, 1_272_251_564)).unwrap();
        let i = info(&exe);
        assert_eq!(i.kind, Kind::Pe);
        match i.details {
            Details::Pe { machine, machine_name, timestamp, timestamp_utc } => {
                assert_eq!((machine, machine_name, timestamp), (0x014C, "x86", 1_272_251_564));
                assert_eq!(timestamp_utc, "2010-04-26T03:12:44Z");
            }
            other => panic!("{other:?}"),
        }
        // a patch with its signature file next to it
        let files = d.path().join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("a.txt"), b"x").unwrap();
        let patch = d.path().join("ps0001.patch");
        shaiya_patch::build(Some(&files), None, None, &patch).unwrap();
        let i = info(&patch);
        assert!(matches!(i.details, Details::Patch { files: 1, loose: 0, removals: 0, signature_file: false }));
        let sig = d.path().join("ps0001.patch.sig");
        std::fs::write(&sig, "ab".repeat(64)).unwrap();
        assert!(matches!(info(&patch).details, Details::Patch { signature_file: true, .. }));
        let s = info(&sig);
        assert_eq!(s.kind, Kind::PatchSignature);
        assert!(matches!(s.details, Details::PatchSignature { hex_len: 128 }));
        assert!(s.problems.is_empty());
        // unknown files keep their first 16 bytes
        let junk = d.path().join("notes.bin");
        std::fs::write(&junk, (0u8..40).collect::<Vec<_>>()).unwrap();
        let u = info(&junk);
        assert_eq!(u.kind, Kind::Unknown);
        assert!(matches!(&u.details, Details::Unknown { magic_hex } if magic_hex == "000102030405060708090a0b0c0d0e0f"));
        // MZ without a PE header is unknown, with a problem line
        let mz = d.path().join("mz.bin");
        std::fs::write(&mz, b"MZ junk").unwrap();
        let m = info(&mz);
        assert_eq!(m.kind, Kind::Unknown);
        assert_eq!(m.problems.len(), 1);
    }

    #[test]
    fn hashing_and_unreadable_files() {
        let d = TempDir::new("inspect-hash");
        let p = d.path().join("x.bin");
        std::fs::write(&p, b"abc").unwrap();
        let i = inspect(&p, &Options { hash: true }, &Job::silent()).unwrap();
        assert_eq!(i.sha256.as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        assert_eq!(i.size, 3);
        assert!(info(&p).sha256.is_none());
        assert!(inspect(&d.path().join("missing"), &Options::default(), &Job::silent()).is_err());
        assert!(inspect(d.path(), &Options::default(), &Job::silent()).is_err());
        // an empty file is unknown, not an error
        let e = d.path().join("empty");
        std::fs::write(&e, b"").unwrap();
        assert_eq!(info(&e).kind, Kind::Unknown);
    }

    #[test]
    fn grouping_and_descriptions() {
        assert_eq!((group(0), group(999), group(1000), group(1_991_954_802)), ("0".into(), "999".into(), "1,000".into(), "1,991,954,802".into()));
        let d = TempDir::new("inspect-describe");
        let p = d.path().join("game.exe");
        std::fs::write(&p, pe_stub(0x014C, 1_272_251_564)).unwrap();
        assert_eq!(describe(&info(&p)), "Windows program (x86), linked 2010-04-26 03:12:44 UTC, 512 bytes");
    }

    #[test]
    fn json_shape() {
        let d = TempDir::new("inspect-json");
        let p = d.path().join("g.exe");
        std::fs::write(&p, pe_stub(0x8664, 0)).unwrap();
        let v = serde_json::to_value(info(&p)).unwrap();
        assert_eq!(v["kind"], "pe");
        assert_eq!(v["details"]["kind"], "pe");
        assert_eq!(v["details"]["machine_name"], "x86_64");
        assert!(v["sha256"].is_null());
        assert_eq!(v["problems"].as_array().unwrap().len(), 0);
    }
}
