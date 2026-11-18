//! Tests over the 77 original server map files. They read the folder from `SVMAP_ORIGINALS` and are skipped (not failed)
//! when it is unset.

use std::collections::BTreeMap;
use std::path::PathBuf;

use shaiya_svmap::*;

fn corpus() -> Option<BTreeMap<u32, Vec<u8>>> {
    let dir = match std::env::var_os("SVMAP_ORIGINALS") {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("SVMAP_ORIGINALS not set: skipped");
            return None;
        }
    };
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(&dir).expect("read SVMAP_ORIGINALS") {
        let p = e.unwrap().path();
        if p.extension().and_then(|s| s.to_str()) == Some("svmap") {
            let id: u32 = p.file_stem().unwrap().to_str().unwrap().parse().expect("numeric file name");
            out.insert(id, std::fs::read(&p).unwrap());
        }
    }
    Some(out)
}

#[test]
fn all_original_files_roundtrip_byte_identical() {
    let Some(files) = corpus() else { return };
    assert_eq!(files.len(), 77);
    for (id, bytes) in &files {
        let m = read(bytes).unwrap_or_else(|e| panic!("map {id}: {e}"));
        assert_eq!(&write(&m), bytes, "map {id}");
    }
}

#[test]
fn all_original_files_roundtrip_through_json_byte_identical() {
    let Some(files) = corpus() else { return };
    for (id, bytes) in &files {
        let m = read(bytes).unwrap();
        let j = to_json(&m).unwrap_or_else(|e| panic!("map {id}: {e}"));
        let back = from_json(&j).unwrap_or_else(|e| panic!("map {id}: {e}"));
        assert_eq!(back, m, "map {id}");
        assert_eq!(&write(&back), bytes, "map {id}");
    }
}

#[test]
fn originals_have_no_validation_errors() {
    let Some(files) = corpus() else { return };
    let maps: BTreeMap<u32, Svmap> = files.iter().map(|(id, b)| (*id, read(b).unwrap())).collect();
    let mut errors = Vec::new();
    for (id, m) in &maps {
        let mut f = validate(m);
        f.extend(validate_cross(m, &|to| maps.get(&to)));
        for x in f.into_iter().filter(|x| x.severity == Severity::Error) {
            errors.push(format!("map {id}: {} {:?}: {}", x.section.label(), x.index, x.message));
        }
    }
    assert!(errors.is_empty(), "{} errors:\n{}", errors.len(), errors.join("\n"));
}

#[test]
fn truncating_an_original_anywhere_is_an_error_not_a_panic() {
    let Some(files) = corpus() else { return };
    let bytes = &files[&1];
    // a full sweep of map 1 would be slow in debug builds; step through it
    let step = (bytes.len() / 4000).max(1);
    let mut cut = 0;
    while cut < bytes.len() {
        assert!(read(&bytes[..cut]).is_err());
        cut += step;
    }
}
