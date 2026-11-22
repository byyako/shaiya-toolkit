//! Opt-in tests of the `.VANI` and `.MANI` readers on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder
//! with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: <VAR> not set" and returns when its variable is unset. The
//! numbers are measured on US EP4.5 only; the eight other matrix clients are index only (see the matrix tests).

use shaiya_archive::Archive;
use shaiya_clientdata::mani::{ManiValue, MANI_FIELDS, MANI_SIZE};
use shaiya_clientdata::{Mani, Vani};
use shaiya_testkit::env_client;
use std::collections::{BTreeMap, BTreeSet};

fn ends(path: &str, ext: &str) -> bool {
    path.to_ascii_lowercase().ends_with(ext)
}

#[test]
fn every_vani_of_the_us_client_reads_and_writes_back() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let mut frame_ms: BTreeMap<i32, usize> = BTreeMap::new();
    let (mut n, mut other_colours, mut with_collision, mut sphere_centre, mut sphere_radius) = (0usize, BTreeSet::new(), 0usize, 0usize, 0usize);
    let (mut items, mut steps) = (BTreeSet::new(), BTreeSet::new());
    let mut named_textures = 0usize;
    for file in a.index().visible() {
        if !ends(&file.path, ".vani") {
            continue;
        }
        assert!(file.path.to_ascii_lowercase().starts_with("entity/vani/"), "{}: not in Entity/VAni", file.path);
        let bytes = a.read_entry(file.entry).unwrap();
        let m = Vani::parse(&bytes, &file.path).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        assert!(m.to_bytes() == bytes, "{}: written back differently", file.path);
        n += 1;
        *frame_ms.entry(m.frame_ms).or_default() += 1;
        if m.has_vertex_colours() {
            other_colours.insert(file.path.clone());
        }
        with_collision += m.collision.is_some() as usize;
        items.insert(m.items.len());
        steps.insert(m.step_count);
        named_textures += (0..m.items.len()).filter(|i| !m.texture_name(*i).is_empty()).count();
        // the sphere: its centre is the box centre and its radius half the box diagonal (within a float rounding)
        let close = |x: f32, y: f32| (x - y).abs() <= 1e-3 * (1.0 + x.abs().max(y.abs()));
        if (0..3).all(|k| close(m.centre[k], (m.min[k] + m.max[k]) / 2.0)) {
            sphere_centre += 1;
        }
        let half_diagonal = ((0..3).map(|k| (m.max[k] - m.min[k]).powi(2)).sum::<f32>()).sqrt() / 2.0;
        if close(m.radius, half_diagonal) {
            sphere_radius += 1;
        }
    }
    println!("VANI files {n}; frame_ms {frame_ms:?}; files with a vertex colour other than 0xFFFFFFFF {}; with a collision mesh {with_collision}", other_colours.len());
    println!("items per file {items:?}; steps per file {:?} to {:?}; items naming a texture {named_textures}; sphere centre = box centre in {sphere_centre}, radius = half the box diagonal in {sphere_radius}", steps.first(), steps.last());
    assert_eq!(n, 66);
    assert_eq!((frame_ms.get(&33).copied(), frame_ms.get(&66).copied(), frame_ms.get(&100).copied(), frame_ms.len()), (Some(8), Some(47), Some(11), 3));
    assert_eq!(other_colours.len(), 16, "files with a vertex colour other than 0xFFFFFFFF");
    assert_eq!(with_collision, 0, "no file uses the collision branch");
    assert_eq!(items, BTreeSet::from([1, 2, 3, 4, 5, 7, 8]), "item counts per file");
    assert_eq!((steps.first().copied(), steps.last().copied()), (Some(11), Some(101)), "steps per file");
    assert_eq!((sphere_centre, sphere_radius), (66, 66));
}

#[test]
fn every_mani_of_the_us_client_reads_and_writes_back() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut n, mut integer_gen, mut float_gen) = (0usize, 0usize, 0usize);
    let mut pairs: BTreeMap<(u16, u16), usize> = BTreeMap::new();
    for file in a.index().visible() {
        if !ends(&file.path, ".mani") {
            continue;
        }
        assert!(file.path.to_ascii_lowercase().starts_with("entity/mani/"), "{}: not in Entity/MAni", file.path);
        let bytes = a.read_entry(file.entry).unwrap();
        assert_eq!(bytes.len(), MANI_SIZE, "{}", file.path);
        let m = Mani::parse(&bytes, &file.path).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        assert!(m.to_bytes() == bytes, "{}: written back differently", file.path);
        assert_eq!(m.fields().len(), MANI_FIELDS.len());
        n += 1;
        if m.is_integer_generation() {
            integer_gen += 1;
            assert_eq!(m.distance_as_integer(), Some(500), "{}", file.path);
        } else {
            float_gen += 1;
            let ManiValue::U16(on) = m.value(&MANI_FIELDS[14]) else { panic!() };
            let ManiValue::U16(pause) = m.value(&MANI_FIELDS[15]) else { panic!() };
            *pairs.entry((on, pause)).or_default() += 1;
        }
    }
    println!("MANI files {n}: integer 500 at offset 104 with zero bytes at 80 to 83 in {integer_gen}; two u16 at 80 and a float at 104 in {float_gen}; (u16, u16) pairs of the second kind {pairs:?}");
    assert_eq!(n, 23);
    assert_eq!((integer_gen, float_gen), (12, 11));
    assert_eq!(pairs, BTreeMap::from([((0, 500), 8), ((1, 0), 3)]), "(u16, u16) pairs of the second kind");
}
