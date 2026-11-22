//! The animation capabilities of the matrix: the `.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO` and `.3DE` files the index lists (every client, index only) and, where the client folder
//! holds its `data.saf`, whether every one reads to the last byte and writes back byte for byte (`.ANI` also: validates without an error).

use crate::{Cap, Cell, Observed};
use shaiya_archive::{Archive, Index};
use shaiya_clientdata::anim::{validate, Severity};
use shaiya_clientdata::meshpick::{mesh_candidates_in, MeshInfo};
use shaiya_clientdata::{Ani, C3dc, Mani, Mesh3de, Mesh3do, Vani};
use std::path::Path;

use Cell::{Fails as F, NotAvailable as NA, Ok as O};

fn is_ani(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ani")
}

/// Sets `AniIndexCount` from the index and `AniRoundTrip` from the contents (`n/a` without a `data.saf`).
pub(crate) fn ani_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_ani(&f.path)).collect();
    o.set(Cap::AniIndexCount, O);
    o.numbers.push((Cap::AniIndexCount, listed.len() as u64));

    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::AniRoundTrip, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("ani round trip: {e}"));
            o.set(Cap::AniRoundTrip, F);
            return;
        }
    };
    let (mut good, mut errors) = (0u64, 0u64);
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| Ani::parse(&b, &f.path).map(|a| (a, b)).map_err(|e| e.to_string())) {
            Ok((a, bytes)) => {
                if a.to_bytes() != bytes {
                    o.notes.push(format!("ani {}: written back differently", f.path));
                } else if validate(&a).iter().any(|i| i.severity == Severity::Error) {
                    errors += 1;
                    o.notes.push(format!("ani {}: validate finds an error", f.path));
                } else {
                    good += 1;
                }
            }
            Err(e) => o.notes.push(format!("ani {}: {e}", f.path)),
        }
    }
    o.set(Cap::AniRoundTrip, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::AniRoundTrip, good));
    o.facts.push(format!("animations: {} .ANI files, {good} read, written back identically and without an error ({errors} with a validation error)", listed.len()));
}

fn is_3dc(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".3dc")
}

/// Sets `C3dcIndexCount` from the index and `C3dcRoundTrip` from the contents (`n/a` without a `data.saf`).
pub(crate) fn c3dc_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_3dc(&f.path)).collect();
    o.set(Cap::C3dcIndexCount, O);
    o.numbers.push((Cap::C3dcIndexCount, listed.len() as u64));

    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::C3dcRoundTrip, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("3dc round trip: {e}"));
            o.set(Cap::C3dcRoundTrip, F);
            return;
        }
    };
    let mut good = 0u64;
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| C3dc::parse(&b, &f.path).map(|m| (m, b)).map_err(|e| e.to_string())) {
            Ok((m, bytes)) => {
                if m.to_bytes() == bytes {
                    good += 1;
                } else {
                    o.notes.push(format!("3dc {}: written back differently", f.path));
                }
            }
            Err(e) => o.notes.push(format!("3dc {}: {e}", f.path)),
        }
    }
    o.set(Cap::C3dcRoundTrip, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::C3dcRoundTrip, good));
    o.facts.push(format!("meshes: {} .3DC files, {good} read with exactly one layout and written back identically", listed.len()));
}

/// Sets `AniPreview` on a client that holds its `data.saf`: the `.ANI` files that have at least one compatible `.3DC` (found by name,
/// `meshpick`, the same function the toolkit's Archive tab uses). Without a `data.saf` it is `n/a`: the index alone says nothing about
/// which bones a mesh names.
pub(crate) fn ani_preview_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_ani(&f.path)).collect();
    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::AniPreview, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("ani preview: {e}"));
            o.set(Cap::AniPreview, F);
            return;
        }
    };
    let paths: Vec<String> = idx.visible().into_iter().map(|f| f.path).filter(|p| is_3dc(p)).collect();
    // each .3DC is parsed once: the cache holds the two facts, not the bytes
    let mut cache: std::collections::BTreeMap<String, Option<MeshInfo>> = std::collections::BTreeMap::new();
    let (mut with_mesh, mut unread) = (0u64, 0u64);
    for f in &listed {
        let ani = match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| Ani::parse(&b, &f.path).map_err(|e| e.to_string())) {
            Ok(a) => a,
            Err(e) => {
                unread += 1;
                o.notes.push(format!("ani preview {}: {e}", f.path));
                continue;
            }
        };
        let (compatible, _) = mesh_candidates_in(&paths, &f.path, ani.bones.len(), &mut |p| {
            *cache.entry(p.to_string()).or_insert_with(|| C3dc::parse(&archive.read_entry(archive.index().get(p)?).ok()?, p).ok().map(|c| MeshInfo::of(&c)))
        });
        if !compatible.is_empty() {
            with_mesh += 1;
        }
    }
    o.set(Cap::AniPreview, if unread == 0 && with_mesh > 0 { O } else { F });
    o.numbers.push((Cap::AniPreview, with_mesh));
    o.facts.push(format!("animation previews: {with_mesh} of {} .ANI files have at least one compatible .3DC ({unread} do not read)", listed.len()));
}

fn is_vani(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".vani")
}

fn is_mani(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".mani")
}

fn has_data_file(dir: &Path) -> bool {
    std::fs::read_dir(dir).ok().is_some_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.saf")))
}

/// Sets `VaniIndexCount` from the index and `VaniRoundTrip` from the contents (`n/a` without a `data.saf`).
pub(crate) fn vani_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_vani(&f.path)).collect();
    o.set(Cap::VaniIndexCount, O);
    o.numbers.push((Cap::VaniIndexCount, listed.len() as u64));
    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::VaniRoundTrip, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("vani round trip: {e}"));
            o.set(Cap::VaniRoundTrip, F);
            return;
        }
    };
    let mut good = 0u64;
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| Vani::parse(&b, &f.path).map(|m| (m, b)).map_err(|e| e.to_string())) {
            Ok((m, bytes)) if m.to_bytes() == bytes => good += 1,
            Ok(_) => o.notes.push(format!("vani {}: written back differently", f.path)),
            Err(e) => o.notes.push(format!("vani {}: {e}", f.path)),
        }
    }
    o.set(Cap::VaniRoundTrip, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::VaniRoundTrip, good));
    o.facts.push(format!("vertex-animated models: {} .VANI files, {good} read and written back identically", listed.len()));
}

/// Sets `ManiIndexCount` and `ManiIndexSize` from the index (every client; the size is the index's length of each entry, no contents) and
/// `ManiRoundTrip` from the contents (`n/a` without a `data.saf`).
pub(crate) fn mani_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_mani(&f.path)).collect();
    o.set(Cap::ManiIndexCount, O);
    o.numbers.push((Cap::ManiIndexCount, listed.len() as u64));
    let sized = listed.iter().filter(|f| f.entry.length as usize == shaiya_clientdata::mani::MANI_SIZE).count() as u64;
    o.set(Cap::ManiIndexSize, if sized == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::ManiIndexSize, sized));
    if sized != listed.len() as u64 {
        o.notes.push(format!("mani sizes: {} of {} .MANI files are not 108 bytes", listed.len() as u64 - sized, listed.len()));
    }
    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::ManiRoundTrip, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("mani round trip: {e}"));
            o.set(Cap::ManiRoundTrip, F);
            return;
        }
    };
    let mut good = 0u64;
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| Mani::parse(&b, &f.path).map(|m| (m, b)).map_err(|e| e.to_string())) {
            Ok((m, bytes)) if m.to_bytes() == bytes => good += 1,
            Ok(_) => o.notes.push(format!("mani {}: written back differently", f.path)),
            Err(e) => o.notes.push(format!("mani {}: {e}", f.path)),
        }
    }
    o.set(Cap::ManiRoundTrip, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::ManiRoundTrip, good));
    o.facts.push(format!("building motion: {} .MANI files, {good} read and written back identically", listed.len()));
}

fn is_3do(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".3do")
}

fn is_3de(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".3de")
}

/// Sets `C3doIndexCount` from the index and `C3doRoundTrip` from the contents (`n/a` without a `data.saf`).
pub(crate) fn c3do_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_3do(&f.path)).collect();
    o.set(Cap::C3doIndexCount, O);
    o.numbers.push((Cap::C3doIndexCount, listed.len() as u64));
    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::C3doRoundTrip, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("3do round trip: {e}"));
            o.set(Cap::C3doRoundTrip, F);
            return;
        }
    };
    let (mut good, mut with_frames) = (0u64, 0u64);
    for f in &listed {
        match archive.read_entry(f.entry).map_err(|e| e.to_string()).and_then(|b| Mesh3do::parse(&b, &f.path).map(|m| (m, b)).map_err(|e| e.to_string())) {
            Ok((m, bytes)) if m.to_bytes() == bytes => {
                good += 1;
                with_frames += !m.frames.is_empty() as u64;
            }
            Ok(_) => o.notes.push(format!("3do {}: written back differently", f.path)),
            Err(e) => o.notes.push(format!("3do {}: {e}", f.path)),
        }
    }
    o.set(Cap::C3doRoundTrip, if good == listed.len() as u64 { O } else { F });
    o.numbers.push((Cap::C3doRoundTrip, good));
    o.facts.push(format!("3DO meshes: {} .3DO files, {good} read and written back identically, {with_frames} with frames", listed.len()));
}

/// Sets `C3deIndexCount` from the index and `C3deRead` from the contents (`n/a` without a `data.saf`). A `.3DE` that follows neither
/// layout is refused with the reason and named in the facts; the cell is `OK` when every file either reads and writes back identically or
/// is refused that way (an empty file is counted apart), and the number is the files that read.
pub(crate) fn c3de_caps(o: &mut Observed, dir: &Path, idx: &Index) {
    let listed: Vec<_> = idx.visible().into_iter().filter(|f| is_3de(&f.path)).collect();
    o.set(Cap::C3deIndexCount, O);
    o.numbers.push((Cap::C3deIndexCount, listed.len() as u64));
    if !has_data_file(dir) || listed.is_empty() {
        o.set(Cap::C3deRead, NA);
        return;
    }
    let archive = match Archive::open(dir) {
        Ok(a) => a,
        Err(e) => {
            o.notes.push(format!("3de read: {e}"));
            o.set(Cap::C3deRead, F);
            return;
        }
    };
    let (mut good, mut broken) = (0u64, 0u64);
    let (mut refused, mut empty): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for f in &listed {
        let bytes = match archive.read_entry(f.entry) {
            Ok(b) => b,
            Err(e) => {
                broken += 1;
                o.notes.push(format!("3de {}: {e}", f.path));
                continue;
            }
        };
        if bytes.is_empty() {
            empty.push(f.path.clone());
            continue;
        }
        match Mesh3de::parse(&bytes, &f.path) {
            Ok(m) if m.to_bytes() == bytes => good += 1,
            Ok(_) => {
                broken += 1;
                o.notes.push(format!("3de {}: written back differently", f.path));
            }
            Err(e) if e.to_string().contains(shaiya_clientdata::mesh3de::NOT_THE_LAYOUT) => refused.push(f.path.clone()),
            Err(e) => {
                broken += 1;
                o.notes.push(format!("3de {}: {e}", f.path));
            }
        }
    }
    o.set(Cap::C3deRead, if broken == 0 { O } else { F });
    o.numbers.push((Cap::C3deRead, good));
    o.facts.push(format!("effect meshes: {} .3DE files, {good} read and written back identically, {} refused as another layout {refused:?}, {} empty {empty:?}", listed.len(), refused.len(), empty.len()));
}
