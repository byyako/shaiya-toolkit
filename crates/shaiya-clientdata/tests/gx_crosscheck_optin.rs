//! Opt-in re-measurements of the Shaiya-GX cross-check on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client
//! folder with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: <VAR> not set" and returns when its variable is unset.
//! The numbers are measured on US EP4.5 only.

use shaiya_archive::Archive;
use shaiya_clientdata::{C3dc, Mesh3de, Vani};
use shaiya_testkit::env_client;

fn ends(path: &str, ext: &str) -> bool {
    path.to_ascii_lowercase().ends_with(ext)
}

#[test]
fn the_3de_frame_entries_are_absolute_positions() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut files, mut entries, mut equal, mut zero, mut equal_off_origin) = (0usize, 0usize, 0usize, 0usize, 0usize);
    for file in a.index().visible() {
        if !ends(&file.path, ".3de") {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        let Ok(m) = Mesh3de::parse(&bytes, &file.path) else {
            continue;
        };
        let Some(block) = &m.block else {
            continue;
        };
        if block.frames.is_empty() {
            continue;
        }
        files += 1;
        for f in &block.frames {
            assert_eq!(f.vertices.len(), m.vertices.len(), "{}", file.path);
            for (e, v) in f.vertices.iter().zip(&m.vertices) {
                entries += 1;
                if e.delta.iter().all(|c| *c == 0.0) {
                    zero += 1;
                }
                if e.delta.iter().zip(&v.pos).all(|(x, y)| (x - y).abs() <= 1e-6) {
                    equal += 1;
                    if v.pos.iter().any(|c| *c != 0.0) {
                        equal_off_origin += 1;
                    }
                }
            }
        }
    }
    println!("files with frames {files}; entries {entries}; equal to the vertex position {equal} (off the origin {equal_off_origin}); exactly (0,0,0) {zero}");
    assert_eq!(files, 349, "files with at least one frame");
    assert_eq!(entries, 609_051, "frame entries");
    assert_eq!(equal, 153_048, "entries equal to their vertex position");
    assert_eq!(equal_off_origin, 152_951, "of them at a vertex off the origin (R-gx counted 151,224: a difference in the definition of off the origin)");
    assert_eq!(zero, 97, "entries exactly (0,0,0)");
}

#[test]
fn the_3dc_pad_byte_18_is_a_valid_bone_when_set() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut files, mut skinned, mut b18, mut b18_files, mut b19) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut b18_valid, mut b19_valid, mut cloak_pad) = (0usize, 0usize, 0usize);
    for file in a.index().visible() {
        if !ends(&file.path, ".3dc") {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        let Ok(m) = C3dc::parse(&bytes, &file.path) else {
            continue;
        };
        files += 1;
        let mut hit = false;
        for mesh in &m.meshes {
            for v in &mesh.vertices {
                let (lo, hi) = ((v.pad & 0xFF) as usize, (v.pad >> 8) as usize);
                if mesh.bones.is_empty() {
                    cloak_pad += (v.pad != 0) as usize;
                    continue;
                }
                skinned += 1;
                if lo != 0 {
                    b18 += 1;
                    hit = true;
                    b18_valid += (lo < mesh.bones.len()) as usize;
                }
                if hi != 0 {
                    b19 += 1;
                    b19_valid += (hi < mesh.bones.len()) as usize;
                }
            }
        }
        b18_files += hit as usize;
    }
    println!("3DC files {files}; skinned vertices {skinned}; byte 18 non-zero {b18} in {b18_files} files (valid bone {b18_valid}); byte 19 non-zero {b19} (below the bone count {b19_valid}); pad non-zero in cloaks {cloak_pad}");
    assert_eq!(files, 2431);
    assert_eq!(skinned, 1_328_909, "vertices of meshes with a bone block (R-gx counted 1,323,308)");
    assert_eq!((b18, b18_files), (1185, 69), "byte 18 non-zero: vertices, files");
    assert_eq!(b18_valid, b18, "byte 18, when set, is below the bone count of its mesh");
    assert_eq!(b19, 191, "byte 19 non-zero");
    assert_eq!(b19_valid, b19, "byte 19, when set, is below the bone count too");
}

#[test]
fn every_mani_starts_with_33() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let mut n = 0usize;
    for file in a.index().visible() {
        if !ends(&file.path, ".mani") {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        assert_eq!(bytes.len(), 108, "{}", file.path);
        assert_eq!(i32::from_le_bytes(bytes[0..4].try_into().unwrap()), 33, "{}: offset 0", file.path);
        n += 1;
    }
    println!("MANI files {n}");
    assert_eq!(n, 23);
}

#[test]
fn the_vani_vertex_word_has_alpha_ff_almost_everywhere() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (mut total, mut alpha_ff, mut zero, mut files_with_zero) = (0usize, 0usize, 0usize, 0usize);
    for file in a.index().visible() {
        if !ends(&file.path, ".vani") {
            continue;
        }
        let bytes = a.read_entry(file.entry).unwrap();
        let m = Vani::parse(&bytes, &file.path).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        let mut z = 0usize;
        for item in &m.items {
            for v in &item.vertices {
                total += 1;
                alpha_ff += (v.diffuse >> 24 == 0xFF) as usize;
                z += (v.diffuse == 0) as usize;
            }
        }
        zero += z;
        files_with_zero += (z > 0) as usize;
    }
    println!("VANI vertices {total}; alpha 0xFF {alpha_ff}; word 0 {zero} in {files_with_zero} files");
    assert_eq!(total, 549_055);
    assert_eq!(alpha_ff, 548_801);
    assert_eq!((zero, files_with_zero), (254, 4), "word 0: vertices, files (R-gx said one file)");
}
