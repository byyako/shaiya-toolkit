//! Opt-in test of the list-file readers (`.MLT`, `.MON`, `.ITM`) on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a
//! client folder with `data.sah` and `data.saf` (US EP4.5). The test prints "skipped: SHAIYA_CLIENT not set" and returns when it is
//! unset. The numbers are measured on US EP4.5 only; the eight other matrix clients are index only (see the matrix tests).

use shaiya_archive::Archive;
use shaiya_clientdata::{C3dc, Itm, Mlt, Mon};
use shaiya_testkit::env_client;
use std::collections::BTreeMap;

fn ends(path: &str, ext: &str) -> bool {
    path.to_ascii_lowercase().ends_with(ext)
}

#[test]
fn every_list_file_of_the_us_client_reads_and_writes_back() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut mlt_files, mut mlt_records, mut mon_files, mut mon_records, mut mon_parts, mut itm_files, mut itm_records) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut refused, mut empty_mlt_slots) = (0usize, 0usize);
    let mut alpha: BTreeMap<i32, usize> = BTreeMap::new();
    let mut itm_formats: BTreeMap<i32, usize> = BTreeMap::new();
    let mut itm_glow: BTreeMap<i32, usize> = BTreeMap::new();
    let (mut meshes, mut named_textures) = (0usize, 0usize);
    let (mut mlt_out_of_range, mut itm_out_of_range) = (0usize, 0usize);
    for file in a.index().visible() {
        let p = file.path.as_str();
        let (is_mlt, is_mon, is_itm) = (ends(p, ".mlt"), ends(p, ".mon"), ends(p, ".itm"));
        if ends(p, ".3dc") {
            let c = C3dc::parse(&a.read_entry(file.entry).unwrap(), p).unwrap_or_else(|e| panic!("{p}: {e}"));
            meshes += 1;
            named_textures += (!c.texture_name().is_empty()) as usize;
            continue;
        }
        if !(is_mlt || is_mon || is_itm) {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        if is_mlt {
            match Mlt::parse(&bytes, p) {
                Ok(m) => {
                    assert!(m.to_bytes() == bytes, "{p}: written back differently");
                    mlt_files += 1;
                    mlt_records += m.records.len();
                    for r in &m.records {
                        *alpha.entry(r.alpha_flag).or_default() += 1;
                        mlt_out_of_range += (m.mesh_name(r).is_none() || m.texture_name(r).is_none()) as usize;
                    }
                    empty_mlt_slots += m.meshes.iter().chain(&m.textures).filter(|s| s.is_empty()).count();
                }
                Err(e) => {
                    refused += 1;
                    println!("refused: {e}");
                }
            }
        } else if is_mon {
            match Mon::parse(&bytes, p) {
                Ok(m) => {
                    assert!(m.to_bytes() == bytes, "{p}: written back differently");
                    mon_files += 1;
                    mon_records += m.records.len();
                    mon_parts += m.part_count();
                }
                Err(e) => {
                    refused += 1;
                    println!("refused: {e}");
                }
            }
        } else {
            match Itm::parse(&bytes, p) {
                Ok(m) => {
                    assert!(m.to_bytes() == bytes, "{p}: written back differently");
                    itm_files += 1;
                    itm_records += m.records.len();
                    for r in &m.records {
                        *itm_formats.entry(r.format).or_default() += 1;
                        *itm_glow.entry(r.glow).or_default() += 1;
                        itm_out_of_range += (m.model_name(r).is_none() || m.texture_name(r).is_none()) as usize;
                    }
                }
                Err(e) => {
                    refused += 1;
                    println!("refused: {e}");
                }
            }
        }
    }
    println!("MLT {mlt_files} files, {mlt_records} records, alphaFlag {alpha:?}, records with an index out of range {mlt_out_of_range}, empty names {empty_mlt_slots}");
    println!("MON {mon_files} files, {mon_records} records, {mon_parts} parts");
    println!("ITM {itm_files} files, {itm_records} records, format {itm_formats:?}, glow {itm_glow:?}, records with an index out of range {itm_out_of_range}");
    assert_eq!(refused, 0, "files refused");
    assert_eq!((mlt_files, mlt_records), (112, 2_176));
    assert_eq!((mon_files, mon_records, mon_parts), (16, 785, 2_005));
    assert_eq!((itm_files, itm_records), (17, 1_540));
    assert_eq!((alpha.get(&1).copied(), alpha.get(&0).copied()), (Some(1_538), Some(638)));
    assert_eq!(itm_glow.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(), vec![(-1, 515), (0, 431), (1, 394), (2, 100), (3, 100)], "ITM glow values");
    println!("3DC {meshes} files, {named_textures} name a texture");
    assert_eq!((meshes, meshes - named_textures), (2_431, 2_429), "3DC files with an empty texture name");
    assert_eq!((mlt_out_of_range, itm_out_of_range), (0, 0), "records pointing outside their lists");
    assert_eq!((itm_formats.get(&0).copied(), itm_formats.get(&1).copied()), (Some(462), Some(1_078)));
}
