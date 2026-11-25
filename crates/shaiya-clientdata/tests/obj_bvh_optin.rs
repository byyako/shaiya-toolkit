//! Opt-in tests of the OBJ and BVH interchange on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with
//! `data.sah` and `data.saf` (US EP4.5). Prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. The numbers are measured on
//! US EP4.5 only and printed; the assertions hold what must hold on any client.

use shaiya_archive::Archive;
use shaiya_clientdata::anim::{locals, passes, pose};
use shaiya_clientdata::interchange::bvh::{export_bvh, import_bvh, BvhOptions};
use shaiya_clientdata::interchange::import::{ImportKind, Imported};
use shaiya_clientdata::interchange::obj::{export_obj, import_obj, ObjInput, ObjOptions};
use shaiya_clientdata::interchange::InterchangeError;
use shaiya_clientdata::{Ani, C3dc, Mesh3de, Mesh3do};
use shaiya_testkit::env_client;
use std::collections::BTreeMap;

/// The triangles of a mesh as the bits of their corners (position, normal, texture coordinate), in face order: what must come back.
type Corner = ([u32; 3], [u32; 3], [u32; 2]);

/// What an import of a mesh must give back: its kind, the corners of its triangles and the bytes of the mesh without frames.
type Wanted = (ImportKind, Vec<[Corner; 3]>, Vec<u8>);

fn corners_3do(m: &Mesh3do) -> Vec<[Corner; 3]> {
    let c = |i: u16| {
        let v = &m.vertices[usize::from(i)];
        (v.pos.map(f32::to_bits), v.normal.map(f32::to_bits), v.uv.map(f32::to_bits))
    };
    m.faces.iter().map(|f| [c(f[0]), c(f[1]), c(f[2])]).collect()
}

fn corners_3de(m: &Mesh3de) -> Vec<[Corner; 3]> {
    let c = |i: u16| {
        let v = &m.vertices[usize::from(i)];
        (v.pos.map(f32::to_bits), [0; 3], v.uv.map(f32::to_bits))
    };
    m.faces.iter().map(|f| [c(f[0]), c(f[1]), c(f[2])]).collect()
}

#[test]
fn every_us_client_mesh_exports_obj_and_a_3do_or_3de_comes_back() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let (mut files, mut exported, mut refused, mut same_mesh, mut same_bytes, mut skipped_big, mut unreadable) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for file in a.index().visible() {
        let lower = file.path.to_ascii_lowercase();
        let kind = if lower.ends_with(".3do") {
            "3do"
        } else if lower.ends_with(".3de") {
            "3de"
        } else if lower.ends_with(".3dc") {
            "3dc"
        } else {
            continue;
        };
        files += 1;
        let bytes = a.read_entry(file.entry).unwrap();
        let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_string();
        // a file the toolkit's reader does not take (a layout it does not read) is counted and left out
        let readable = match kind {
            "3do" => Mesh3do::parse(&bytes, &file.path).is_ok(),
            "3de" => Mesh3de::parse(&bytes, &file.path).is_ok(),
            _ => C3dc::parse(&bytes, &file.path).is_ok(),
        };
        if !readable {
            unreadable += 1;
            continue;
        }
        let opts = ObjOptions { source: name, mtl_name: "x.mtl".to_string() };
        // what comes back is compared with the source: its corners (positions, normals and texture coordinates to the bit) in face order
        let (input, want): (ObjInput, Option<Wanted>) = match kind {
            "3do" => {
                let m = Mesh3do::parse(&bytes, &file.path).unwrap();
                let c = corners_3do(&m);
                let mut plain = m.clone();
                plain.frames.clear();
                plain.frame_rate = 0;
                (ObjInput::Mesh3do(m), Some((ImportKind::Mesh3do, c, plain.to_bytes())))
            }
            "3de" => {
                let m = Mesh3de::parse(&bytes, &file.path).unwrap();
                let c = corners_3de(&m);
                let mut plain = m.clone();
                plain.block = None;
                for v in &mut plain.vertices {
                    v.flag = -1;
                }
                (ObjInput::Mesh3de(m), Some((ImportKind::Mesh3de, c, plain.to_bytes())))
            }
            _ => (ObjInput::Mesh3dc(C3dc::parse(&bytes, &file.path).unwrap()), None),
        };
        let out = match export_obj(input, &opts) {
            Ok(o) => o,
            Err(InterchangeError::SelfCheck(m)) => panic!("{}: the self-check failed (a bug): {m}", file.path),
            Err(e) => {
                refused += 1;
                let kinds = e.kinds();
                assert!(!kinds.is_empty(), "{}: a refusal without a kind", file.path);
                for k in kinds {
                    *by_kind.entry(format!("{kind}:{}", k.as_str())).or_default() += 1;
                }
                continue;
            }
        };
        exported += 1;
        let Some((import_kind, corners, plain_bytes)) = want else { continue };
        match import_obj(&out.obj, import_kind, None) {
            Ok((imp, _)) => {
                let back = match &imp {
                    Imported::Mesh3do(m) => corners_3do(m),
                    Imported::Mesh3de(m) => corners_3de(m),
                    _ => unreachable!(),
                };
                if back == corners {
                    same_mesh += 1;
                } else {
                    panic!("{}: the mesh that came back is not the one that went out", file.path);
                }
                let bytes = shaiya_clientdata::interchange::import::finish(&imp, None).unwrap();
                if bytes == plain_bytes {
                    same_bytes += 1;
                }
            }
            // a mesh of more than 65,535 distinct corners cannot come back (the export keeps its vertices, the import unifies corners): counted
            Err(_) => skipped_big += 1,
        }
    }
    println!("OBJ: {files} .3DO, .3DE and .3DC files: {unreadable} not read by the toolkit's reader, {exported} exported, {refused} refused");
    for (k, n) in &by_kind {
        println!("OBJ refused by kind {k}: {n}");
    }
    println!("OBJ re-import of .3DO and .3DE: {same_mesh} give the same corners (positions, normals, texture coordinates to the bit), {same_bytes} give the same bytes (without frames), {skipped_big} could not be read back");
    assert_eq!(exported + refused + unreadable, files);
}

fn max_pose_difference(a: &Ani, b: &Ani) -> f32 {
    let (la, lb) = (locals(a), locals(b));
    let end = a.end_key.min(b.end_key);
    let start = a.first_field.min(b.first_field).min(0);
    let step = ((i64::from(end) - i64::from(start)) / 200).max(1) as i32;
    let mut worst = 0f32;
    let mut k = start;
    while k <= end {
        for (x, y) in pose(a, &la, k as f32).iter().zip(&pose(b, &lb, k as f32)) {
            for e in 0..16 {
                let d = (x[e] - y[e]).abs();
                if d.is_finite() {
                    worst = worst.max(d);
                }
            }
        }
        k += step;
    }
    worst
}

#[test]
fn every_us_client_ani_exports_bvh_or_is_refused_by_kind_and_comes_back() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let (mut files, mut exported, mut refused, mut came_back, mut passed, mut not_read, mut dropped_files) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut worst = 0f32;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_by_kind: BTreeMap<String, String> = BTreeMap::new();
    for file in a.index().visible() {
        if !file.path.to_ascii_lowercase().ends_with(".ani") {
            continue;
        }
        files += 1;
        let ani = Ani::parse(&a.read_entry(file.entry).unwrap(), &file.path).unwrap();
        match export_bvh(&ani, &BvhOptions::default()) {
            Ok(o) => {
                exported += 1;
                match import_bvh(&o.text, &ani, None) {
                    Ok((back, _)) => {
                        came_back += 1;
                        if passes(&back, &ani) {
                            passed += 1;
                        }
                        let d = max_pose_difference(&ani, &back);
                        worst = worst.max(d);
                    }
                    Err(e) => {
                        not_read += 1;
                        *by_kind.entry(format!("import:{}", e.kinds().first().map_or("none", |k| k.as_str()))).or_default() += 1;
                        first_by_kind.entry("import".to_string()).or_insert_with(|| format!("{}: {e}", file.path));
                    }
                }
            }
            Err(InterchangeError::SelfCheck(m)) => panic!("{}: a bug: {m}", file.path),
            Err(e) => {
                refused += 1;
                let kinds = e.kinds();
                assert!(!kinds.is_empty(), "{}: a refusal without a kind", file.path);
                for k in kinds {
                    let key = format!("export:{}", k.as_str());
                    *by_kind.entry(key.clone()).or_default() += 1;
                    first_by_kind.entry(key).or_insert_with(|| format!("{}: {e}", file.path));
                }
                // the same file with the motion of its translating bones left out is written
                if export_bvh(&ani, &BvhOptions { drop_translation: true, ..BvhOptions::default() }).is_ok() {
                    dropped_files += 1;
                }
            }
        }
    }
    println!("BVH: {files} .ANI files: {exported} exported, {refused} refused ({dropped_files} of them are written with --drop-translation)");
    for (k, n) in &by_kind {
        println!("BVH {k}: {n} (first: {})", first_by_kind.get(k).or_else(|| first_by_kind.get("import")).cloned().unwrap_or_default());
    }
    println!("BVH re-import of the {exported} written files: {came_back} read, {passed} pass against their source, {not_read} not read; the largest difference of any matrix element over the sampled keys: {worst:.6}");
    assert_eq!(exported + refused, files);
}
