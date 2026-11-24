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
