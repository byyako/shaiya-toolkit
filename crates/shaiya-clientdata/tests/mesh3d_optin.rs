//! Opt-in tests of the `.3DO` and `.3DE` readers on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder
//! with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: <VAR> not set" and returns when its variable is unset. The
//! numbers are measured on US EP4.5 only; the eight other matrix clients are index only (see the matrix tests).

use shaiya_archive::Archive;
use shaiya_clientdata::mesh3de::{De3Layout, NOT_THE_LAYOUT};
use shaiya_clientdata::{Mesh3de, Mesh3do};
use shaiya_testkit::env_client;

fn ends(path: &str, ext: &str) -> bool {
    path.to_ascii_lowercase().ends_with(ext)
}

#[test]
fn every_3do_of_the_us_client_reads_and_writes_back() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut n, mut named_textures) = (0usize, 0usize);
    let mut animated: Vec<(String, i32, usize)> = Vec::new();
    for file in a.index().visible() {
        if !ends(&file.path, ".3do") {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        let m = Mesh3do::parse(&bytes, &file.path).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        assert!(m.to_bytes() == bytes, "{}: written back differently", file.path);
        n += 1;
        named_textures += !m.texture_name().is_empty() as usize;
        if !m.frames.is_empty() {
            animated.push((file.path.clone(), m.frame_rate, m.frames.len()));
        }
    }
    println!("3DO files {n}; naming a texture {named_textures}; with frames {animated:?}");
    assert_eq!(n, 271);
    assert_eq!(named_textures, 3, "files that name a texture");
    assert_eq!(animated.len(), 1, "{animated:?}");
    assert_eq!(animated[0].0.to_ascii_lowercase(), "item/3do/arrow.3do");
    assert_eq!((animated[0].1, animated[0].2), (20, 11));
}

#[test]
fn every_3de_of_the_us_client_reads_or_is_named() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut listed, mut standard, mut statics, mut with_frames, mut flags_not_minus_one) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut refused, mut empty): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let mut statics_named: Vec<String> = Vec::new();
    let mut folders: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for file in a.index().visible() {
        if !ends(&file.path, ".3de") {
            continue;
        }
        listed += 1;
        let folder = file.path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        *folders.entry(folder).or_default() += 1;
        let bytes = a.read_entry(file.entry).unwrap();
        if bytes.is_empty() {
            empty.push(file.path.clone());
            continue;
        }
        match Mesh3de::parse(&bytes, &file.path) {
            Ok(m) => {
                assert!(m.to_bytes() == bytes, "{}: written back differently", file.path);
                match m.layout() {
                    De3Layout::Standard => standard += 1,
                    De3Layout::Static => {
                        statics += 1;
                        statics_named.push(file.path.clone());
                    }
                }
                with_frames += (m.frame_count() > 0) as usize;
                flags_not_minus_one += m.vertices.iter().any(|v| v.flag != -1) as usize;
            }
            Err(e) => {
                assert!(e.to_string().contains(NOT_THE_LAYOUT), "{}: {e}", file.path);
                refused.push(file.path.clone());
            }
        }
    }
    println!("3DE files listed {listed}: read {} (standard {standard}, of which with frames {with_frames}; static {statics} {statics_named:?}), refused {refused:?}, empty {empty:?}; files with a vertex field other than -1: {flags_not_minus_one}", standard + statics);
    assert_eq!(listed, 469);
    assert_eq!((folders.get("Effect/3DE").copied(), folders.get("Effect/DDS").copied(), folders.len()), (Some(468), Some(1), 2), "folders of the .3DE files: {folders:?}");
    assert_eq!(with_frames, 349, "standard files with at least one frame");
    assert_eq!(standard + statics, 467, "files that read");
    assert_eq!(statics, 1);
    assert_eq!(refused.len(), 2, "{refused:?}");
    let mut names: Vec<String> = refused.iter().map(|p| p.to_ascii_lowercase()).collect();
    names.sort();
    assert_eq!(names, ["effect/3de/004.3de", "effect/3de/deathknight007.3de"], "the two refused files, full paths");
    let statics_lower: Vec<String> = statics_named.iter().map(|p| p.to_ascii_lowercase()).collect();
    assert_eq!(statics_lower, ["effect/3de/d_dragonstone03.3de"], "the one static file");
    assert_eq!(flags_not_minus_one, 0, "the vertex field is -1 in every file read");
}
