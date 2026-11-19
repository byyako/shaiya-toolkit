//! `ClientSource` in folder mode (an extracted client) with mixed-case paths, on a temporary folder. No client data needed.

use std::fs;

use shaiya_clientdata::{ClientSource, Error};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("shaiya-clientdata-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn utf16(s: &str) -> Vec<u8> {
    let mut v = vec![0xff, 0xfe];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

#[test]
fn folder_mode_is_case_insensitive_and_lists_files() {
    let d = temp_dir("folder");
    fs::create_dir_all(d.join("World")).unwrap();
    fs::write(d.join("World").join("2.TXT"), utf16("0\tFirst\r\n3\tFourth\r\n")).unwrap();
    fs::write(d.join("Sysmsg-Uni.txt"), utf16("5068\t\t\"D-Water Borderland (Border)\"\r\n")).unwrap();
    let c = ClientSource::open(&d).unwrap();
    assert!(!c.is_archive());
    assert!(c.exists("world/2.txt") && c.exists(r"WORLD\2.txt") && !c.exists("world/3.txt"));
    assert!(matches!(c.read("world/3.txt"), Err(Error::NotFound(_))));
    assert!(!c.exists("../2.txt") && !c.exists("world/../World/2.TXT"));
    let names = c.area_names(2).unwrap();
    assert_eq!((names.get(0), names.get(3), names.get(1)), (Some("First"), Some("Fourth"), None));
    assert!(c.area_names(9).unwrap().is_empty());
    assert_eq!(c.map_name(0), "D-Water Borderland (Border)");
    assert_eq!(c.map_name(5), "Map 5");
    let mut files = c.list();
    files.sort();
    assert_eq!(files, vec!["Sysmsg-Uni.txt", "World/2.TXT"]);
    // a missing texture becomes the checker once and is logged once
    let t = c.texture_or_checker("terrain/detail/none.dds");
    c.texture_or_checker("Terrain/Detail/NONE.dds");
    assert_eq!((t.width, t.height), (32, 32));
    assert_eq!(c.problems().len(), 1);
    assert!(ClientSource::open(&d.join("missing")).is_err());
    fs::remove_dir_all(&d).unwrap();
}

#[test]
fn has_any_stops_at_the_entry_limit_and_finds_a_shallow_file() {
    use shaiya_clientdata::Probe;
    let d = temp_dir("probe");
    let deep = d.join("a").join("b");
    fs::create_dir_all(&deep).unwrap();
    for i in 0..300 {
        fs::write(deep.join(format!("f{i}.dat")), b"").unwrap();
    }
    let c = ClientSource::open(&d).unwrap();
    let wld = |p: &str| p.to_ascii_lowercase().ends_with(".wld");
    // nothing to find: gives up at the limit, having looked at no more than it
    let r = c.has_any(wld, 100);
    assert_eq!(r.probe, Probe::GaveUp);
    assert!(r.visited <= 100, "{}", r.visited);
    // a limit above the size of the tree: the whole tree was looked at
    let r = c.has_any(wld, 10_000);
    assert_eq!((r.probe, r.visited), (Probe::NotFound, 302));
    // a file at depth 2 is found
    fs::create_dir_all(d.join("world")).unwrap();
    fs::write(d.join("world").join("1.wld"), b"").unwrap();
    assert_eq!(c.has_any(wld, 10_000).probe, Probe::Found);
    fs::remove_dir_all(&d).unwrap();
}
