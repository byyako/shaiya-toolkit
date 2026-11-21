//! The animation capabilities of the matrix: the `.ANI` files the index lists (every client, index only) and, where the client folder holds
//! its `data.saf`, whether every `.ANI` reads to the last byte, writes back byte for byte and validates without an error.

use crate::{Cap, Cell, Observed};
use shaiya_archive::{Archive, Index};
use shaiya_clientdata::anim::{validate, Severity};
use shaiya_clientdata::Ani;
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

    let has_saf = std::fs::read_dir(dir).ok().is_some_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.saf")));
    if !has_saf || listed.is_empty() {
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
