//! Identify a Shaiya file and describe it: SAH index, SAF data, SData (SEED-encrypted or plain), patch, patch signature, model list (`.MLT`, `.MON`, `.ITM`), Windows
//! program. Detection is by content first, name second; the three model lists are the exception: a file named `.mlt`, `.mon` or `.itm` is read as that list (a valid list under another name is Unknown). Malformed input gives a `problems` entry, not a failure, unless the file cannot
//! be read at all.

pub mod validate;

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
    /// A `.MLT` character list.
    Mlt,
    /// A `.MON` monster list.
    Mon,
    /// An `.ITM` item list.
    Itm,
    /// A RIFF/WAVE sound.
    Wav,
    /// An Ogg file (Vorbis header facts only).
    Ogg,
    /// A JPEG picture.
    Jpg,
    /// A PNG picture.
    Png,
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
    /// A model list file: records, and the mesh and texture names it lists (`.MLT`, `.ITM`) or the parts of its records (`.MON`); zeros
    /// when the file did not read (a problem line says why).
    List { records: usize, meshes: usize, textures: usize, parts: usize },
    /// A sound: the format as the preview names it, channels, sample rate and playing time (zeros when the header did not read; a problem
    /// line says why).
    Sound { format: String, channels: u32, rate: u32, duration_s: f64 },
    /// A picture: what its header says ("JPEG baseline, 3 components") and its size (zeros when the header did not read).
    Picture { what: String, width: u32, height: u32 },
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
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 file` / `2 files`, with thousands separators.
pub fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", group(n), if n == 1 { one } else { many })
}

/// One line saying what the file is, the way `shaiya info` and Shaiya Toolkit show it
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
        Details::List { records, meshes, textures, parts } => match i.kind {
            Kind::Mon => format!("MON list, {}, {}, {size} bytes", counted(*records as u64, "record", "records"), counted(*parts as u64, "part", "parts")),
            _ => format!(
                "{} list, {}, {}, {}, {size} bytes",
                if i.kind == Kind::Mlt { "MLT" } else { "ITM" },
                counted(*records as u64, "record", "records"),
                counted(*meshes as u64, "mesh name", "mesh names"),
                counted(*textures as u64, "texture name", "texture names")
            ),
        },
        Details::Sound { format, channels, rate, duration_s } => {
            let ch = counted(*channels as u64, "channel", "channels");
            format!("{format}, {ch}, {rate} Hz, {duration_s:.1} s, {size} bytes")
        }
        Details::Picture { what, width, height } => format!("{what}, {width} x {height}, {size} bytes"),
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
    } else if has_ext(path, "saf") && shaiya_seed::is_encrypted(&head) {
        // A lone .saf is not SAF data (that needs its .sah) and is not an SData file either: do not run the SData checks on a whole archive.
        problems.push(format!("{name} starts like an SData file, but a .saf is archive data only beside its .sah and none was found; not checked as SData"));
        (Kind::Unknown, Details::Unknown { magic_hex: to_hex(&head[..head.len().min(16)]) })
    } else if shaiya_seed::is_encrypted(&head) {
        seed_details(path, size, &head, &name, &mut problems)
    } else if head.starts_with(b"PK\x03\x04") && zip_is_patch(path, size, &head) {
        patch_details(path, &mut problems)
    } else if let Some((machine, timestamp)) = pe_header(&head) {
        let details = Details::Pe { machine, machine_name: machine_name(machine), timestamp, timestamp_utc: utc_iso_unix(timestamp as i64) };
        (Kind::Pe, details)
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WAVE") {
        sound_details(path, size, &name, true, &mut problems)
    } else if head.starts_with(b"OggS") {
        sound_details(path, size, &name, false, &mut problems)
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) || head.starts_with(&shaiya_clientdata::texture::PNG_MAGIC) {
        picture_details(&head, &name, &mut problems)
    } else if let Some(kind) = ["mlt", "mon", "itm"].into_iter().find(|e| has_ext(path, e)) {
        list_details(path, size, &head, &name, kind, &mut problems)
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

/// A WAV or an Ogg file: its header facts (the file is read whole up to the sound preview's limit).
fn sound_details(path: &Path, size: u64, name: &str, wav: bool, problems: &mut Vec<String>) -> (Kind, Details) {
    let kind = if wav { Kind::Wav } else { Kind::Ogg };
    let zero = Details::Sound { format: if wav { "WAV".into() } else { "Ogg".into() }, channels: 0, rate: 0, duration_s: 0.0 };
    if size > shaiya_clientdata::wav::WAV_CAP as u64 {
        problems.push(format!("{name} is over {} MiB: its header is not read", shaiya_clientdata::wav::WAV_CAP >> 20));
        return (kind, zero);
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            problems.push(Error::io(path, e).to_string());
            return (kind, zero);
        }
    };
    if wav {
        match shaiya_clientdata::wav::parse_header(&bytes) {
            Ok(i) => {
                if !i.is_supported() {
                    problems.push(format!("{name}: unsupported format (tag {}, {} bits); the preview does not play it", i.tag, i.bits));
                }
                (kind, Details::Sound { format: format!("WAV, {}", i.format_name()), channels: i.channels as u32, rate: i.rate, duration_s: i.duration_s })
            }
            Err(e) => {
                problems.push(e.to_string());
                (kind, zero)
            }
        }
    } else {
        match shaiya_clientdata::ogg::ogg_info(&bytes) {
            Ok(i) => (kind, Details::Sound { format: "Ogg Vorbis".into(), channels: i.channels as u32, rate: i.rate, duration_s: i.duration_s }),
            Err(e) => {
                problems.push(e.to_string());
                (kind, zero)
            }
        }
    }
}

/// A JPEG or a PNG: the header facts from the first bytes (nothing is decoded).
fn picture_details(head: &[u8], name: &str, problems: &mut Vec<String>) -> (Kind, Details) {
    let kind = if head.starts_with(&[0xFF, 0xD8]) { Kind::Jpg } else { Kind::Png };
    match shaiya_clientdata::texture::picture_info(head) {
        Some(p) => (kind, Details::Picture { what: p.what, width: p.width, height: p.height }),
        None => {
            problems.push(format!("{name}: the {} header did not read", if kind == Kind::Jpg { "JPEG" } else { "PNG" }));
            (kind, Details::Picture { what: if kind == Kind::Jpg { "JPEG".into() } else { "PNG".into() }, width: 0, height: 0 })
        }
    }
}

/// Lists are read whole; real ones are well under a megabyte.
const LIST_LIMIT_BYTES: u64 = 64 << 20;

/// A file named `.mlt`, `.mon` or `.itm`: read by its reader; a list of another episode (or any other file) is `Unknown` with the reason.
fn list_details(path: &Path, size: u64, head: &[u8], name: &str, ext: &str, problems: &mut Vec<String>) -> (Kind, Details) {
    use shaiya_clientdata::modellist::ListKind;
    let (kind, want) = match ext {
        "mlt" => (Kind::Mlt, ListKind::Mlt),
        "mon" => (Kind::Mon, ListKind::Mon),
        _ => (Kind::Itm, ListKind::Itm),
    };
    let zero = Details::List { records: 0, meshes: 0, textures: 0, parts: 0 };
    let unknown = || Details::Unknown { magic_hex: to_hex(&head[..head.len().min(16)]) };
    let read = if size > LIST_LIMIT_BYTES { Err(format!("{name} is over {} MiB: not read", LIST_LIMIT_BYTES >> 20)) } else { std::fs::read(path).map_err(|e| e.to_string()) };
    let bytes = match read {
        Ok(b) => b,
        Err(e) => {
            problems.push(e);
            return (Kind::Unknown, unknown());
        }
    };
    let parsed = match want {
        ListKind::Mlt => shaiya_clientdata::Mlt::parse(&bytes, name).map(|m| Details::List { records: m.records.len(), meshes: m.meshes.len(), textures: m.textures.len(), parts: 0 }),
        ListKind::Mon => shaiya_clientdata::Mon::parse(&bytes, name).map(|m| Details::List { records: m.records.len(), meshes: 0, textures: 0, parts: m.part_count() }),
        ListKind::Itm => shaiya_clientdata::Itm::parse(&bytes, name).map(|m| Details::List { records: m.records.len(), meshes: m.models.len(), textures: m.textures.len(), parts: 0 }),
    };
    match parsed {
        Ok(d) => (kind, d),
        Err(e) => {
            problems.push(e.to_string());
            // a file that starts with the kind's signature is that kind, broken; any other start is not a list of this reader
            if head.starts_with(want.signature()) {
                (kind, zero)
            } else {
                (Kind::Unknown, unknown())
            }
        }
    }
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
        let a = info(&alone);
        assert_eq!(a.kind, Kind::Unknown, "{a:?}");
        assert!(a.problems.iter().any(|p| p.contains("none was found")), "{a:?}");
        // the same bytes under another name are still an SData file
        let seed = d.path().join("alone").join("x.sdata");
        std::fs::write(&seed, &first).unwrap();
        assert_eq!(info(&seed).kind, Kind::SdataSeed);
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
    fn list_files_are_named_with_their_counts() {
        use shaiya_testkit::lists::{itm_bytes, mlt_bytes, mon_bytes, ItmRec, MonRec};
        let d = TempDir::new("inspect-lists");
        let mlt = d.path().join("Player.MLT");
        std::fs::write(&mlt, mlt_bytes(&["a.3DC", "b.3DC"], &["x.DDS"], &[(0, 0, 1), (1, 0, 0), (1, 0, 1)])).unwrap();
        let m = info(&mlt);
        assert_eq!(m.kind, Kind::Mlt);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        assert!(matches!(m.details, Details::List { records: 3, meshes: 2, textures: 1, parts: 0 }), "{:?}", m.details);
        assert!(describe(&m).starts_with("MLT list, 3 records, 2 mesh names, 1 texture name, "), "{}", describe(&m));

        let mon = d.path().join("mon.mon");
        let rec = MonRec { name: "Wolf", flag: 1, slots: [""; 17], parts: &[("w.3DC", "w.DDS"), ("f.3DC", "")], size: 1.0, attachments: &[] };
        std::fs::write(&mon, mon_bytes(&[rec])).unwrap();
        let m = info(&mon);
        assert_eq!(m.kind, Kind::Mon);
        assert!(describe(&m).starts_with("MON list, 1 record, 2 parts, "), "{}", describe(&m));

        let itm = d.path().join("Item.itm");
        std::fs::write(&itm, itm_bytes(&["s.3DO"], &["s.DDS"], &[ItmRec { words: [0, 0, 0, 0, 0, 0], extra: None }])).unwrap();
        let m = info(&itm);
        assert_eq!(m.kind, Kind::Itm);
        assert!(describe(&m).starts_with("ITM list, 1 record, 1 mesh name, 1 texture name, "), "{}", describe(&m));

        // another episode's layout under the same name is unknown, with the reason
        let other = d.path().join("other.mon");
        std::fs::write(&other, b"MO4    ").unwrap();
        let o = info(&other);
        assert_eq!(o.kind, Kind::Unknown);
        assert!(o.problems.iter().any(|p| p.contains("`MO4`: a layout of another episode, not read")), "{:?}", o.problems);
        // a broken list of the right signature keeps its kind and says why
        let cut = d.path().join("cut.mlt");
        let bytes = mlt_bytes(&["a.3DC"], &[], &[(0, 0, 1)]);
        std::fs::write(&cut, &bytes[..bytes.len() - 2]).unwrap();
        let c = info(&cut);
        assert_eq!(c.kind, Kind::Mlt);
        assert_eq!(c.problems.len(), 1);
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
