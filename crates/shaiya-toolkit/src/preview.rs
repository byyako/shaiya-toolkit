//! What the Archive tab shows for one entry: a decoded texture, text, an SData summary, or hex. Built on a worker thread from the
//! entry's bytes (never on the UI thread).

use shaiya_archive::{Archive, Entry};
use shaiya_core::text::Encoding;
use shaiya_core::{Error, Job, Result, Unit};
use std::path::Path;

pub const TEXT_FIRST: u64 = 1 << 20;
pub const TEXT_ALL: u64 = 16 << 20;
pub const HEX_MAX: u64 = 64 << 20;
pub const IMAGE_SIDE_MAX: u32 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdataSummary {
    pub encrypted: bool,
    pub crc: Option<u32>,
    pub crc_ok: Option<bool>,
    pub plain_size: u64,
    pub padding_zero: Option<bool>,
    pub tail_zero: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum PreviewData {
    Empty,
    Image { width: u32, height: u32, rgba: Vec<u8>, header: String, downscaled: bool },
    /// A `.wld` map: the grey hillshade top view of a field (`rgba` is empty for a dungeon map, which has `note` instead).
    Map { header: String, height_range: Option<(f32, f32)>, note: Option<String>, width: u32, height: u32, rgba: Vec<u8>, downscaled: bool },
    /// The raw bytes (decoded for display with an encoding chosen by the person).
    Text { bytes: Vec<u8>, total: u64, truncated: bool, guess: Encoding },
    /// `plain` is the decrypted bytes (or the file itself when it is not encrypted).
    Sdata { summary: SdataSummary, plain: Vec<u8>, raw: Vec<u8> },
    /// `bytes` start at entry offset `base`.
    Hex { bytes: Vec<u8>, base: u64, total: u64 },
    /// A decoder said no: the message and the beginning of the bytes (hex fallback).
    Failed { message: String, bytes: Vec<u8>, total: u64 },
}

impl PreviewData {
    /// A short name for tests and the status line.
    pub fn kind(&self) -> &'static str {
        match self {
            PreviewData::Empty => "empty",
            PreviewData::Image { .. } => "image",
            PreviewData::Map { .. } => "map",
            PreviewData::Text { .. } => "text",
            PreviewData::Sdata { .. } => "sdata",
            PreviewData::Hex { .. } => "hex",
            PreviewData::Failed { .. } => "failed",
        }
    }
}

fn ext_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Text when under 1 % of the first 64 KiB are control bytes other than tab, CR and LF.
pub fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    if head.starts_with(&[0xFF, 0xFE]) {
        return true;
    }
    let ctl = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\r' | b'\n')).count();
    ctl * 100 < head.len()
}

pub fn guess_encoding(bytes: &[u8]) -> Encoding {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if std::str::from_utf8(bytes).is_ok() || (bytes.len() > 3 && std::str::from_utf8(&bytes[..bytes.len() - 3]).is_ok()) {
        Encoding::Utf8
    } else {
        Encoding::Cp949
    }
}

/// Decodes preview text with the chosen encoding (a UTF-16 byte-order mark is skipped).
pub fn decode_text(bytes: &[u8], enc: Encoding) -> String {
    let b = if enc == Encoding::Utf16Le && bytes.starts_with(&[0xFF, 0xFE]) { &bytes[2..] } else { bytes };
    // a cut multi-byte tail would show as a replacement character; trim UTF-16 to an even length
    let b = if enc == Encoding::Utf16Le && b.len() % 2 == 1 { &b[..b.len() - 1] } else { b };
    enc.decode(b).0
}

pub fn encoding_by_name(name: &str) -> Option<Encoding> {
    Some(match name.to_ascii_lowercase().as_str() {
        "cp949" => Encoding::Cp949,
        "cp1252" => Encoding::Cp1252,
        "cp1251" => Encoding::Cp1251,
        "cp936" => Encoding::Cp936,
        "utf-8" | "utf8" => Encoding::Utf8,
        "utf-16le" | "utf16le" => Encoding::Utf16Le,
        _ => return None,
    })
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Cp949 => "cp949",
        Encoding::Cp1252 => "cp1252",
        Encoding::Cp1251 => "cp1251",
        Encoding::Cp936 => "cp936",
        Encoding::Utf8 => "UTF-8",
        Encoding::Utf16Le => "UTF-16LE",
    }
}

fn image_header(data: &[u8], w: u32, h: u32) -> String {
    let what = if data.starts_with(b"DDS ") && data.len() >= 88 {
        let fourcc = &data[84..88];
        if fourcc.iter().all(|&b| b == 0) { "DDS uncompressed".to_string() } else { format!("DDS {}", String::from_utf8_lossy(fourcc).trim_end_matches('\0')) }
    } else if data.starts_with(b"BM") && data.len() >= 30 {
        format!("BMP {} bit", u16::from_le_bytes([data[28], data[29]]))
    } else if data.len() >= 18 {
        format!("TGA type {}, {} bit", data[2], data[16])
    } else {
        "image".to_string()
    };
    format!("{what}, {w} x {h}")
}

/// Keeps every `k`th pixel so neither side is over [`IMAGE_SIDE_MAX`].
fn shrink(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let k = w.max(h).div_ceil(IMAGE_SIDE_MAX);
    if k <= 1 {
        return (w, h, rgba.to_vec());
    }
    let (nw, nh) = (w / k, h / k);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let i = (((y * k) * w + x * k) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (nw, nh, out)
}

/// The preview of a `.wld` map from its bytes: a field gets the grey hillshade (one pixel per corner, north up, shrunk to
/// [`IMAGE_SIDE_MAX`]) and its height range; a dungeon map has no terrain and names its `.dg` instead. A file that does not parse
/// is an error (the caller shows it as Failed).
pub fn map_preview(data: &[u8], path: &str) -> shaiya_clientdata::Result<PreviewData> {
    let wld = shaiya_clientdata::Wld::parse(data, path)?;
    let file = path.rsplit('/').next().unwrap_or(path);
    let id = file.rsplit_once('.').map_or(file, |(s, _)| s);
    Ok(match &wld.terrain {
        Some(t) => {
            let (w, h, rgba) = shaiya_clientdata::render::hillshade_grey(t);
            let (sw, sh, rgba) = shrink(w as u32, h as u32, &rgba);
            PreviewData::Map {
                header: format!("Map {id}, {} x {} corners, {} units", t.n, t.n, t.size),
                height_range: Some(t.height_range()),
                note: None,
                width: sw,
                height: sh,
                downscaled: sw != w as u32,
                rgba,
            }
        }
        None => {
            let note = if wld.dg.is_empty() { "Dungeon map: no dungeon file is named; no top view yet".to_string() } else { format!("Dungeon map: the floor is in {}; no top view yet", wld.dg) };
            PreviewData::Map { header: format!("Map {id}, dungeon"), height_range: None, note: Some(note), width: 0, height: 0, rgba: Vec::new(), downscaled: false }
        }
    })
}

fn read_all(archive: &Archive, entry: &Entry, cap: u64, job: &Job) -> Result<Vec<u8>> {
    let n = (entry.length as u64).min(cap) as usize;
    let mut out = Vec::with_capacity(n);
    job.total(Some(n as u64), Unit::Bytes);
    let mut done = 0usize;
    while done < n {
        job.check()?;
        let m = (n - done).min(1 << 20);
        out.extend_from_slice(&archive.read_range(entry, done as u64, m)?);
        job.advance(m as u64);
        done += m;
    }
    Ok(out)
}

/// Builds the preview of `entry` (`path` decides what it should be, the bytes can overrule). `text_all`: read up to 16 MiB of text
/// instead of 1 MiB. `hex_from`: where the hex window starts for entries over 64 MiB.
pub fn make_preview(archive: &Archive, entry: &Entry, path: &str, text_all: bool, hex_from: u64, job: &Job) -> Result<PreviewData> {
    if entry.length == 0 {
        return Ok(PreviewData::Empty);
    }
    let total = entry.length as u64;
    let ext = ext_of(path);
    let head = archive.read_range(entry, 0, total.min(64 << 10) as usize)?;
    let is_image = matches!(ext.as_str(), "dds" | "tga" | "bmp") || head.starts_with(b"DDS ") || (head.starts_with(b"BM") && ext == "dds");
    if is_image {
        let data = read_all(archive, entry, 128 << 20, job)?;
        return Ok(match shaiya_clientdata::texture::decode(&data, path) {
            Ok(img) => {
                let header = image_header(&data, img.width, img.height);
                let (w, h, rgba) = shrink(img.width, img.height, &img.rgba);
                PreviewData::Image { width: w, height: h, downscaled: w != img.width, rgba, header }
            }
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    if ext == "sdata" || shaiya_seed::is_encrypted(&head) {
        let data = read_all(archive, entry, 128 << 20, job)?;
        let size = data.len() as u64;
        return Ok(match shaiya_seed::info(&data, path) {
            Ok(Some(h)) => {
                let (plain, ok) = shaiya_seed::decrypt_unchecked(&data, path)?;
                let tail_zero = {
                    // the bytes after the plaintext in the last block are part of the file; zero when the writer zero-padded
                    match shaiya_seed::unwrap(&data, path) {
                        Ok((_, shaiya_seed::Wrapper::Seed { tail, .. })) => Some(tail.iter().all(|&b| b == 0)),
                        _ => None,
                    }
                };
                PreviewData::Sdata {
                    summary: SdataSummary { encrypted: true, crc: Some(h.crc), crc_ok: Some(ok), plain_size: plain.len() as u64, padding_zero: Some(h.padding.iter().all(|&b| b == 0)), tail_zero },
                    plain,
                    raw: data,
                }
            }
            Ok(None) => PreviewData::Sdata { summary: SdataSummary { encrypted: false, crc: None, crc_ok: None, plain_size: size, padding_zero: None, tail_zero: None }, plain: data.clone(), raw: data },
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    if ext == "wld" && (head.starts_with(b"FLD\0") || head.starts_with(b"DUN\0")) {
        let data = read_all(archive, entry, 128 << 20, job)?;
        return Ok(map_preview(&data, path).unwrap_or_else(|e| PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total }));
    }
    let by_ext = matches!(ext.as_str(), "txt" | "ini" | "xml" | "csv" | "cfg" | "lst" | "log");
    if by_ext || looks_like_text(&head) {
        let cap = if text_all { TEXT_ALL } else { TEXT_FIRST };
        let bytes = read_all(archive, entry, cap, job)?;
        let guess = guess_encoding(&bytes);
        return Ok(PreviewData::Text { truncated: total > bytes.len() as u64, total, bytes, guess });
    }
    // hex: the whole entry up to 64 MiB, else a window of 64 MiB at `hex_from`
    if total <= HEX_MAX {
        let bytes = read_all(archive, entry, HEX_MAX, job)?;
        return Ok(PreviewData::Hex { bytes, base: 0, total });
    }
    let base = hex_from.min(total - 1) / 16 * 16;
    let n = (total - base).min(16 << 20) as usize;
    let bytes = archive.read_range(entry, base, n).map_err(|e| match e {
        Error::Cancelled => Error::Cancelled,
        other => other,
    })?;
    Ok(PreviewData::Hex { bytes, base, total })
}

/// One hex row: `00000010  64 00 00 00 ...  d...`.
pub fn hex_row(bytes: &[u8], base: u64, row: usize) -> String {
    let start = row * 16;
    let chunk = &bytes[start.min(bytes.len())..(start + 16).min(bytes.len())];
    let mut s = format!("{:08x}  ", base + start as u64);
    for i in 0..16 {
        match chunk.get(i) {
            Some(b) => s.push_str(&format!("{b:02x} ")),
            None => s.push_str("   "),
        }
        if i == 7 {
            s.push(' ');
        }
    }
    s.push(' ');
    s.extend(chunk.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
    s
}

pub fn hex_rows(len: usize) -> usize {
    len.div_ceil(16)
}

/// Used by the Archive tab's image drawing: a short file name.
pub fn file_name(path: &str) -> &str {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{bmp_stub, tga_stub, SahBuilder, TempDir};

    fn open(d: &TempDir, files: &[(&str, Vec<u8>)]) -> Archive {
        let mut b = SahBuilder::new(0);
        for (p, data) in files {
            b = b.file(p.as_bytes(), data);
        }
        b.write_to(d.path());
        Archive::open(d.path()).unwrap()
    }

    fn pv(a: &Archive, path: &str) -> PreviewData {
        make_preview(a, a.index().get(path).unwrap(), path, false, 0, &Job::silent()).unwrap()
    }

    #[test]
    fn every_kind_of_preview() {
        let d = TempDir::new("preview-kinds");
        let a = open(
            &d,
            &[
                ("i/a.tga", tga_stub(8, 4)),
                ("i/b.bmp", bmp_stub(5, 3)),
                ("i/bad.dds", b"not an image at all, but named dds".to_vec()),
                ("t/readme.txt", "hello, world".as_bytes().to_vec()),
                ("t/ko.ini", vec![0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD, b'\n']),
                ("s/Item.SData", shaiya_seed::encrypt(b"0123456789abcdef-more")),
                ("s/Plain.SData", b"not encrypted".to_vec()),
                ("x/blob.bin", vec![0u8, 1, 2, 3, 0, 0, 7, 0, 0, 0]),
                ("x/empty.bin", vec![]),
            ],
        );
        match pv(&a, "i/a.tga") {
            PreviewData::Image { width, height, rgba, header, downscaled } => {
                assert_eq!((width, height, rgba.len(), downscaled), (8, 4, 8 * 4 * 4, false));
                assert!(header.contains("TGA type 2, 24 bit") && header.contains("8 x 4"), "{header}");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "i/b.bmp") {
            PreviewData::Image { width, height, header, .. } => assert_eq!((width, height, header.as_str()), (5, 3, "BMP 24 bit, 5 x 3")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(pv(&a, "i/bad.dds"), PreviewData::Failed { .. }), "a decode failure falls back to hex");
        match pv(&a, "t/readme.txt") {
            PreviewData::Text { bytes, guess, truncated, .. } => assert_eq!((bytes, guess, truncated), (b"hello, world".to_vec(), Encoding::Utf8, false)),
            other => panic!("{other:?}"),
        }
        match pv(&a, "t/ko.ini") {
            PreviewData::Text { bytes, guess, .. } => {
                assert_eq!(guess, Encoding::Cp949);
                assert_eq!(decode_text(&bytes, guess).trim(), "\u{BAAC}\u{C2A4}\u{D130}");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "s/Item.SData") {
            PreviewData::Sdata { summary, plain, .. } => {
                assert_eq!((summary.encrypted, summary.crc_ok, summary.plain_size, summary.padding_zero, summary.tail_zero), (true, Some(true), 21, Some(true), Some(true)));
                assert_eq!(plain, b"0123456789abcdef-more");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "s/Plain.SData") {
            PreviewData::Sdata { summary, .. } => assert!(!summary.encrypted && summary.plain_size == 13),
            other => panic!("{other:?}"),
        }
        match pv(&a, "x/blob.bin") {
            PreviewData::Hex { bytes, base, total } => assert_eq!((bytes.len(), base, total), (10, 0, 10)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(pv(&a, "x/empty.bin"), PreviewData::Empty));
    }

    #[test]
    fn text_detection_encodings_and_hex_rows() {
        assert!(looks_like_text(b"plain text\r\nwith lines\t"));
        assert!(!looks_like_text(&[0u8; 100]));
        assert!(!looks_like_text(&[]));
        let mut mostly = vec![b'a'; 1000];
        mostly[5] = 1;
        assert!(looks_like_text(&mostly), "under 1 % control bytes");
        mostly[6..20].fill(2);
        assert!(!looks_like_text(&mostly));
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0];
        assert_eq!(guess_encoding(&utf16), Encoding::Utf16Le);
        assert_eq!(decode_text(&utf16, Encoding::Utf16Le), "hi");
        assert_eq!(guess_encoding("h\u{E9}llo".as_bytes()), Encoding::Utf8);
        assert_eq!(encoding_by_name("CP949"), Some(Encoding::Cp949));
        assert_eq!(encoding_by_name("nope"), None);
        let row = hex_row(&(0u8..20).collect::<Vec<_>>(), 0x100, 1);
        assert!(row.starts_with("00000110  10 11 12 13 ") && row.ends_with("...."), "{row}");
        assert_eq!(hex_rows(33), 3);
        assert_eq!(hex_rows(0), 0);
    }

    #[test]
    fn big_images_are_shrunk_and_text_is_capped() {
        let (w, h, rgba) = shrink(8192, 100, &vec![5u8; 8192 * 100 * 4]);
        assert_eq!((w, h, rgba.len()), (4096, 50, 4096 * 50 * 4));
        assert_eq!(shrink(100, 100, &[1u8; 100 * 100 * 4]).0, 100);
        let d = TempDir::new("preview-text");
        let big = vec![b'x'; 3 << 20];
        let a = open(&d, &[("t/big.txt", big)]);
        match pv(&a, "t/big.txt") {
            PreviewData::Text { bytes, truncated, total, .. } => assert_eq!((bytes.len() as u64, truncated, total), (TEXT_FIRST, true, 3 << 20)),
            other => panic!("{other:?}"),
        }
        let e = a.index().get("t/big.txt").unwrap();
        match make_preview(&a, e, "t/big.txt", true, 0, &Job::silent()).unwrap() {
            PreviewData::Text { bytes, truncated, .. } => assert_eq!((bytes.len(), truncated), (3 << 20, false)),
            other => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    /// A `.wld` head followed by zeros until it parses to its last byte (every later count, name and number is 0).
    fn zero_tail(head: Vec<u8>) -> Vec<u8> {
        zero_tail_len(&head, 0)
    }

    /// The same, starting the search at `from` zeros (the length does not depend on the size of the terrain).
    fn zero_tail_len(head: &[u8], from: usize) -> Vec<u8> {
        for n in from..4096 {
            let mut b = head.to_vec();
            b.extend(std::iter::repeat_n(0u8, n));
            if shaiya_clientdata::Wld::parse(&b, "x").is_ok() {
                return b;
            }
        }
        panic!("no zero tail makes a parsable .wld");
    }

    fn tail_len() -> usize {
        let small = field_head(32);
        zero_tail_len(&small, 0).len() - small.len()
    }

    fn field_head(size: u32) -> Vec<u8> {
        let n = size as usize / 2 + 1;
        let mut h = b"FLD\0".to_vec();
        h.extend((size as i32).to_le_bytes());
        for i in 0..n * n {
            let (x, z) = ((i % n) as i32, (i / n) as i32);
            h.extend(((9000 + 20 * x - 20 * z) as i16).to_le_bytes());
        }
        h.extend(vec![0u8; n * n]);
        h.extend(0i32.to_le_bytes()); // no terrain textures
        h
    }

    /// A field with a ramp: height rises towards the east and falls towards the north.
    pub fn field_wld(size: u32) -> Vec<u8> {
        let mut h = field_head(size);
        let t = tail_len();
        h.extend(std::iter::repeat_n(0u8, t));
        assert!(shaiya_clientdata::Wld::parse(&h, "x").is_ok());
        h
    }

    pub fn dungeon_wld(dg: &str) -> Vec<u8> {
        let mut h = b"DUN\0".to_vec();
        let mut name = dg.as_bytes().to_vec();
        name.resize(256, 0);
        h.extend(name);
        zero_tail(h)
    }

    #[test]
    fn a_field_previews_as_a_map_with_its_header_and_height_range() {
        let d = TempDir::new("preview-map-field");
        SahBuilder::new(0).file(b"world/12.wld", &field_wld(32)).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        match make_preview(&a, a.index().get("world/12.wld").unwrap(), "world/12.wld", false, 0, &Job::silent()).unwrap() {
            PreviewData::Map { header, height_range, note, width, height, rgba, downscaled } => {
                assert_eq!(header, "Map 12, 17 x 17 corners, 32 units");
                assert_eq!((width, height, rgba.len(), downscaled, note), (17, 17, 17 * 17 * 4, false, None));
                let (lo, hi) = height_range.unwrap();
                assert!((lo - (9000.0 - 20.0 * 16.0) * 0.02 + 200.0).abs() < 1e-3 && (hi - (9000.0 + 20.0 * 16.0) * 0.02 + 200.0).abs() < 1e-3, "{lo} {hi}");
                assert!(rgba.chunks(4).all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]), "grey and opaque");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_dungeon_map_names_its_floor_file_and_has_no_picture() {
        match map_preview(&dungeon_wld("dg_cave.dg"), "world/31.wld").unwrap() {
            PreviewData::Map { header, height_range, note, rgba, .. } => {
                assert_eq!(header, "Map 31, dungeon");
                assert_eq!(note.as_deref(), Some("Dungeon map: the floor is in dg_cave.dg; no top view yet"));
                assert!(height_range.is_none() && rgba.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_wld_that_does_not_parse_is_failed_with_hex() {
        let d = TempDir::new("preview-map-bad");
        let mut bad = b"FLD\0".to_vec();
        bad.extend(7i32.to_le_bytes()); // an odd map size
        bad.extend([1u8; 40]);
        SahBuilder::new(0).file(b"world/9.wld", &bad).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        match make_preview(&a, a.index().get("world/9.wld").unwrap(), "world/9.wld", false, 0, &Job::silent()).unwrap() {
            PreviewData::Failed { message, bytes, total } => assert!(!message.is_empty() && bytes.len() == 48 && total == 48),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_largest_map_is_shrunk_to_the_image_limit() {
        match map_preview(&field_wld(8192), "world/1.wld").unwrap() {
            PreviewData::Map { header, width, height, downscaled, rgba, .. } => {
                assert_eq!(header, "Map 1, 4097 x 4097 corners, 8192 units");
                assert!(downscaled && width <= IMAGE_SIDE_MAX && height <= IMAGE_SIDE_MAX);
                assert_eq!(rgba.len(), (width * height * 4) as usize);
            }
            other => panic!("{other:?}"),
        }
    }
}
