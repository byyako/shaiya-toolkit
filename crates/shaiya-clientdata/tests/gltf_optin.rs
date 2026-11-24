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
}
