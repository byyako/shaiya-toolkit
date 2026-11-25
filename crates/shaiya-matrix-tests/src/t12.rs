//! The list-file capabilities of the matrix: the `.MLT`, `.MON` and `.ITM` files the index lists (every client, index only) and, where
//! the client folder holds its `data.saf`, whether every one reads to the last byte and writes back byte for byte.

use crate::{Cap, Cell, Observed};
use shaiya_archive::{Archive, Index};
use shaiya_clientdata::{Itm, Mlt, Mon};
use std::path::Path;

use Cell::{Fails as F, NotAvailable as NA, Ok as O};

fn has_data_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).ok().is_some_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.saf")))
}

/// Parses one file and says whether it wrote back identically.
type Reads<'a> = &'a dyn Fn(&[u8], &str) -> Result<bool, String>;

/// Sets the index-count and round-trip cell of one list kind.
fn kind_caps(o: &mut Observed, dir: &Path, idx: &Index, ext: &str, count: Cap, round: Cap, reads: Reads) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| f.path.to_ascii_lowercase().ends_with(ext)).collect();
    o.set(count, O);
    o.numbers.push((count, listed.len() as u64));
    if !has_data_file(dir) || listed.is_empty() {
        o.set(round, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("{ext} round trip: {e}"));
            o.set(round, F);
            return;
        }
    };
    let mut good = 0u64;
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| reads(&b, &f.path)) {
            Ok(true) => good += 1,
            Ok(false) => o.notes.push(format!("{ext} {}: written back differently", f.path)),
            Err(e) => o.notes.push(format!("{ext} {}: {e}", f.path)),
        }
    }
    o.set(round, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((round, good));
    o.facts.push(format!("list files {ext}: {} listed, {good} read to the last byte and written back identically", listed.len()));
}

/// Sets `MltIndexCount`, `MltRoundTrip`, `MonIndexCount`, `MonRoundTrip`, `ItmIndexCount` and `ItmRoundTrip`.
pub(crate) fn list_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    kind_caps(o, dir, idx, ".mlt", Cap::MltIndexCount, Cap::MltRoundTrip, &|b, p| Mlt::parse(b, p).map(|m| m.to_bytes() == b).map_err(|e| e.to_string()));
    kind_caps(o, dir, idx, ".mon", Cap::MonIndexCount, Cap::MonRoundTrip, &|b, p| Mon::parse(b, p).map(|m| m.to_bytes() == b).map_err(|e| e.to_string()));
    kind_caps(o, dir, idx, ".itm", Cap::ItmIndexCount, Cap::ItmRoundTrip, &|b, p| Itm::parse(b, p).map(|m| m.to_bytes() == b).map_err(|e| e.to_string()));
}

/// Sets `WavIndexCount` (every client, index only), `WavHeaders` and `JpgDecode` (where `data.saf` is present: every `.wav` header reads, every
/// `.jpg` decodes).
pub(crate) fn media_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let visible = idx.visible();
    let with_ext = |ext: &str| -> Vec<_> { visible.iter().filter(|f| f.path.to_ascii_lowercase().ends_with(ext)).collect() };
    let (wavs, jpgs) = (with_ext(".wav"), with_ext(".jpg"));
    o.set(Cap::WavIndexCount, O);
    o.numbers.push((Cap::WavIndexCount, wavs.len() as u64));
    if !has_data_file(dir) {
        o.set(Cap::WavHeaders, NA);
        o.set(Cap::JpgDecode, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("media: {e}"));
            o.set(Cap::WavHeaders, F);
            o.set(Cap::JpgDecode, F);
            return;
        }
    };
    let mut good = 0u64;
    for f in &wavs {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| shaiya_clientdata::wav::parse_header(&b).map_err(|e| e.to_string())) {
            Ok(_) => good += 1,
            Err(e) => o.notes.push(format!("wav {}: {e}", f.path)),
        }
    }
    o.set(Cap::WavHeaders, if good == wavs.len() as u64 && good > 0 { O } else { F });
    o.numbers.push((Cap::WavHeaders, good));
    o.facts.push(format!("sounds: {} .wav listed, {good} headers read", wavs.len()));
    let mut decoded = 0u64;
    for f in &jpgs {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| shaiya_clientdata::texture::decode(&b, &f.path).map_err(|e| e.to_string())) {
            Ok(_) => decoded += 1,
            Err(e) => o.notes.push(format!("jpg {}: {e}", f.path)),
        }
    }
    o.set(Cap::JpgDecode, if decoded == jpgs.len() as u64 && decoded > 0 { O } else { F });
    o.numbers.push((Cap::JpgDecode, decoded));
    o.facts.push(format!("pictures: {} .jpg listed, {decoded} decoded", jpgs.len()));
}

/// Sets `TextureIndexFound`: the meshes the list files pair with a texture that exists (needs the file contents).
pub(crate) fn texture_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let cap = Cap::TextureIndexFound;
    if !has_data_file(dir) {
        o.set(cap, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("texture index: {e}"));
            o.set(cap, F);
            return;
        }
    };
    let paths: Vec<String> = idx.visible().into_iter().map(|f| f.path).collect();
    match shaiya_clientdata::TextureIndex::build(&paths, &mut |p| idx.get(p).and_then(|e| archive.read_entry(e).ok()), &mut || false) {
        Some((index, report)) if report.lists_refused.is_empty() => {
            let n = index.textured_mesh_count() as u64;
            o.set(cap, if n > 0 { O } else { F });
            o.numbers.push((cap, n));
            o.facts.push(format!("texture index: {} list files read, {n} meshes with a found texture, {} texture names missing", report.lists_read, report.missing));
        }
        Some((_, report)) => {
            o.notes.push(format!("texture index: {} list files refused", report.lists_refused.len()));
            o.set(cap, F);
        }
        None => o.set(cap, F),
    }
}
