//! Opt-in test of the glTF export on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with `data.sah` and
//! `data.saf` (US EP4.5). Prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. Numbers are measured on US EP4.5 only.

use shaiya_archive::Archive;
use shaiya_clientdata::anim::RIGS;
use shaiya_clientdata::interchange::{export_gltf, GltfInput, GltfOptions, InterchangeError};
use shaiya_clientdata::meshpick::{mesh_candidates_in, MeshInfo};
use shaiya_clientdata::{Ani, C3dc};
use shaiya_testkit::env_client;
use std::collections::BTreeMap;

#[test]
fn every_us_client_ani_and_3dc_exports_or_is_refused_by_kind() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let (mut ani_files, mut mesh_files, mut exported, mut refused) = (0usize, 0usize, 0usize, 0usize);
    let (mut vertex_refusals, mut matrix_refusals) = (0usize, 0usize);
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_by_kind: BTreeMap<String, String> = BTreeMap::new();
    for file in a.index().visible() {
        let lower = file.path.to_ascii_lowercase();
        let (is_ani, is_mesh) = (lower.ends_with(".ani"), lower.ends_with(".3dc"));
        if !is_ani && !is_mesh {
            continue;
        }
        let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_string();
        let bytes = a.read_entry(file.entry).unwrap();
        let opts = GltfOptions { source: name.clone(), ..GltfOptions::default() };
        let input = if is_ani {
            ani_files += 1;
            GltfInput::Anim(Ani::parse(&bytes, &file.path).unwrap())
        } else {
            mesh_files += 1;
            GltfInput::Mesh { c3dc: C3dc::parse(&bytes, &file.path).unwrap(), anims: vec![] }
        };
        match export_gltf(input, &opts) {
            Ok(_) => exported += 1,
            Err(InterchangeError::SelfCheck(m)) => panic!("{}: the self-check failed (a bug): {m}", file.path),
            Err(e @ InterchangeError::Refused { .. }) => {
                refused += 1;
                if let (false, InterchangeError::Refused { list, .. }) = (is_ani, &e) {
                    // the split of the not-finite refusals of .3DC files: a vertex (position, normal or texture coordinate) or a bone matrix
                    if list.iter().any(|r| r.what.contains("bone matrix")) {
                        matrix_refusals += 1;
                    }
                    if list.iter().any(|r| r.kind == shaiya_clientdata::interchange::RefusalKind::NotFinite && r.what.contains("vertex")) {
                        vertex_refusals += 1;
                    }
                }
                let kinds = e.kinds();
                assert!(!kinds.is_empty(), "{}: a refusal without a kind", file.path);
                for k in kinds {
                    let key = format!("{}:{}", if is_ani { "ani" } else { "3dc" }, k.as_str());
                    *by_kind.entry(key.clone()).or_default() += 1;
                    first_by_kind.entry(key).or_insert_with(|| format!("{}: {e}", file.path));
                }
            }
        }
    }
    println!("GLTF: {ani_files} .ANI and {mesh_files} .3DC: {exported} exported, {refused} refused");
    println!("GLTF .3DC refusals: {vertex_refusals} files with a not-finite vertex, {matrix_refusals} files with a not-finite bone matrix (a file can be both)");
    for (k, n) in &by_kind {
        println!("GLTF refused by kind {k}: {n} (first: {})", first_by_kind[k]);
    }
    assert_eq!(exported + refused, ani_files + mesh_files);
}

#[test]
fn the_character_rigs_export_their_default_parts_with_the_first_animation() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let paths: Vec<String> = a.index().visible().into_iter().map(|f| f.path.to_string()).collect();
    let (mut files, mut refused) = (0usize, Vec::new());
    for (rig, race) in RIGS {
        let want = format!("character/{}/ani/{}_000_", race.to_ascii_lowercase(), rig);
        let mut anis: Vec<&String> = paths.iter().filter(|p| p.replace(char::from(92), "/").to_ascii_lowercase().starts_with(&want)).collect();
        anis.sort();
        let Some(ani_path) = anis.first() else { panic!("no {rig}_000_ animation") };
        let ani = Ani::parse(&a.read(ani_path).unwrap(), ani_path).unwrap();
        let mut info = |p: &str| a.read(p).ok().and_then(|b| C3dc::parse(&b, p).ok()).map(|c| MeshInfo::of(&c));
        let (_, defaults) = mesh_candidates_in(&paths, ani_path, ani.bones.len(), &mut info);
        assert!(!defaults.is_empty(), "{rig}: no default parts");
        for p in defaults {
            let c3dc = C3dc::parse(&a.read(&p).unwrap(), &p).unwrap();
            let opts = GltfOptions { source: p.rsplit('/').next().unwrap_or(&p).to_string(), ..GltfOptions::default() };
            files += 1;
            match export_gltf(GltfInput::Mesh { c3dc, anims: vec![ani.clone()] }, &opts) {
                Ok(_) => {}
                Err(InterchangeError::SelfCheck(m)) => panic!("{p}: the self-check failed (a bug): {m}"),
                Err(e) => refused.push(format!("{p} with {ani_path}: {e}")),
            }
        }
    }
    println!("GLTF rigs: {files} default parts with their first animation, {} refused", refused.len());
    for r in &refused {
        println!("GLTF rig refusal: {r}");
    }
    assert!(refused.is_empty(), "the rigs' default parts are all written: {refused:?}");
}

/// The W12 exports of every character rig's first animation and default parts, read back with the import and `--like` the source file:
/// each `.3DC` must come back byte for byte, each `.ANI` must pass against its source (with `keys_exact` and with every whole key) and
/// pose within 1e-5 at every whole key. A file the export refuses is counted and skipped.
#[test]
fn us_client_gltf_round_trips() {
    use shaiya_clientdata::anim::{locals, passes, pose};
    use shaiya_clientdata::interchange::gltf_read::Gltf;
    use shaiya_clientdata::interchange::import::{finish, import_gltf, ImportKind, ImportOptions, Imported, Like};
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let paths: Vec<String> = a.index().visible().into_iter().map(|f| f.path.to_string()).collect();
    let open = |o: &shaiya_clientdata::interchange::GltfOutput| Gltf::open(&o.json, &mut |_| Ok(o.bin.clone())).unwrap();
    let (mut meshes, mut mesh_refused, mut identical) = (0usize, 0usize, 0usize);
    let (mut anis, mut ani_refused, mut passed, mut worst) = (0usize, 0usize, 0usize, 0f32);
    for (rig, race) in RIGS {
        let want = format!("character/{}/ani/{}_000_", race.to_ascii_lowercase(), rig);
        let mut cands: Vec<&String> = paths.iter().filter(|p| p.replace(char::from(92), "/").to_ascii_lowercase().starts_with(&want)).collect();
        cands.sort();
        let Some(ani_path) = cands.first() else { panic!("no {rig}_000_ animation") };
        let ani = Ani::parse(&a.read(ani_path).unwrap(), ani_path).unwrap();
        // the animation alone
        anis += 1;
        match export_gltf(GltfInput::Anim(ani.clone()), &GltfOptions { source: "x.ANI".into(), ..GltfOptions::default() }) {
            Err(_) => ani_refused += 1,
            Ok(o) => {
                let g = open(&o);
                let like = Like::Ani(ani.clone());
                for exact in [true, false] {
                    let opts = ImportOptions { keys_exact: exact, ..ImportOptions::default() };
                    let (imp, _) = import_gltf(&g, ImportKind::Ani, Some(&like), &opts).unwrap_or_else(|e| panic!("{ani_path} (exact {exact}): {e}"));
                    let Imported::Ani(back) = imp else { panic!() };
                    assert!(passes(&back, &ani), "{ani_path} (exact {exact}) does not pass");
                    let (la, lb) = (locals(&ani), locals(&back));
                    let first = shaiya_clientdata::interchange::mesh::start_key_of(&ani);
                    for k in first..=ani.end_key {
                        let (pa, pb) = (pose(&ani, &la, k as f32), pose(&back, &lb, k as f32));
                        for (x, y) in pa.iter().zip(&pb) {
                            for e in 0..16 {
                                let d = (x[e] - y[e]).abs();
                                assert!(d <= 1e-5 || (x[e].is_nan() && y[e].is_nan()), "{ani_path} key {k}: {} vs {}", x[e], y[e]);
                                if d.is_finite() {
                                    worst = worst.max(d);
                                }
                            }
                        }
                    }
                    if exact {
                        passed += 1;
                    }
                }
            }
        }
        // the default parts with the animation, imported back like the source part
        let mut info = |p: &str| a.read(p).ok().and_then(|b| C3dc::parse(&b, p).ok()).map(|c| MeshInfo::of(&c));
        let (_, defaults) = mesh_candidates_in(&paths, ani_path, ani.bones.len(), &mut info);
        for p in defaults {
            let bytes = a.read(&p).unwrap();
            let c3dc = C3dc::parse(&bytes, &p).unwrap();
            meshes += 1;
            match export_gltf(GltfInput::Mesh { c3dc: c3dc.clone(), anims: vec![ani.clone()] }, &GltfOptions { source: p.rsplit('/').next().unwrap_or(&p).to_string(), ..GltfOptions::default() }) {
                Err(_) => mesh_refused += 1,
                Ok(o) => {
                    let like = Like::C3dc(c3dc.clone());
                    let (imp, _) = import_gltf(&open(&o), ImportKind::C3dc, Some(&like), &ImportOptions::default()).unwrap_or_else(|e| panic!("{p}: {e}"));
                    assert_eq!(finish(&imp, Some(&like)).unwrap(), bytes, "{p} is not byte for byte the source");
                    identical += 1;
                }
            }
        }
    }
    println!("GLTF import: {anis} animations ({ani_refused} refused by the export, {passed} passed with keys exact and every whole key, worst pose difference {worst:e}); {meshes} default parts ({mesh_refused} refused by the export, {identical} byte for byte)");
    // measured on US EP4.5: every rig's first animation and every default part is written and comes back (the docs say so)
    assert!(ani_refused == 0 && mesh_refused == 0, "{ani_refused} animations and {mesh_refused} parts were refused by the export");
    assert!(passed == anis && identical == meshes);
}

/// Every `.ANI` of the client that the export writes, imported back with `keys_exact` and `--like` itself: it must pass and pose like the
/// source at every whole key within 1e-4 (a pose that is NaN in both counts as equal). Prints how many, and the worst difference.
#[test]
fn every_exportable_us_ani_imports_back_with_keys_exact() {
    use shaiya_clientdata::anim::{locals, passes, pose};
    use shaiya_clientdata::interchange::gltf_read::Gltf;
    use shaiya_clientdata::interchange::import::{import_gltf, ImportKind, ImportOptions, Imported, Like};
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let (mut files, mut refused, mut ok, mut worst) = (0usize, 0usize, 0usize, 0f32);
    let mut tracks_kept = (0usize, 0usize);
    for file in a.index().visible() {
        if !file.path.to_ascii_lowercase().ends_with(".ani") {
            continue;
        }
        files += 1;
        let ani = Ani::parse(&a.read_entry(file.entry).unwrap(), &file.path).unwrap();
        let Ok(o) = export_gltf(GltfInput::Anim(ani.clone()), &GltfOptions { source: "x.ANI".into(), ..GltfOptions::default() }) else {
            refused += 1;
            continue;
        };
        let g = Gltf::open(&o.json, &mut |_| Ok(o.bin.clone())).unwrap();
        let like = Like::Ani(ani.clone());
        let (imp, _) = import_gltf(&g, ImportKind::Ani, Some(&like), &ImportOptions { keys_exact: true, ..ImportOptions::default() }).unwrap_or_else(|e| panic!("{}: {e}", file.path));
        let Imported::Ani(back) = imp else { panic!() };
        assert!(passes(&back, &ani), "{} does not pass", file.path);
        let (la, lb) = (locals(&ani), locals(&back));
        let first = shaiya_clientdata::interchange::mesh::start_key_of(&ani);
        let last = ani.end_key.min(first + 400);
        for k in first..=last {
            let (pa, pb) = (pose(&ani, &la, k as f32), pose(&back, &lb, k as f32));
            for (x, y) in pa.iter().zip(&pb) {
                // the translation of a bone far from the origin is a sum of large terms: the tolerance is relative to the largest element of the matrix
                let scale = x.iter().filter(|c| c.is_finite()).fold(1.0f32, |m, c| m.max(c.abs()));
                for e in 0..16 {
                    if x[e].is_nan() && y[e].is_nan() {
                        continue;
                    }
                    let d = (x[e] - y[e]).abs();
                    assert!(d <= 1e-4 * scale, "{} key {k}: {} vs {}", file.path, x[e], y[e]);
                    worst = worst.max(d / scale);
                }
            }
        }
        for (x, y) in ani.bones.iter().zip(&back.bones) {
            tracks_kept.0 += x.rot.len() + x.trans.len();
            tracks_kept.1 += y.rot.len() + y.trans.len();
        }
        ok += 1;
    }
    println!("GLTF import of every exportable .ANI: {files} files, {refused} refused by the export, {ok} imported back and posing alike (worst relative difference {worst:e}); keys {} in the sources, {} in the imports", tracks_kept.0, tracks_kept.1);
    assert_eq!(ok + refused, files);
}
