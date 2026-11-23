//! Opt-in tests of the `.ANI` preview on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with `data.sah`
//! and `data.saf` (US EP4.5). Each test prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. Run with
//! `-- --test-threads=2` (the frame sets are large). The numbers are measured on US EP4.5 only; nothing from the client is kept.

use shaiya_archive::Archive;
use shaiya_clientdata::anim::RIGS;
use shaiya_clientdata::Ani;
use shaiya_core::Job;
use shaiya_testkit::env_client;
use shaiya_toolkit::preview::{anim_preview, mesh_candidates_in, MeshChoice, MeshInfo, MeshInfoCache, PreviewData};
use std::collections::BTreeMap;

fn client() -> Option<Archive> {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return None;
    };
    Some(Archive::open(&dir).unwrap())
}

fn is_ani(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ani")
}

#[test]
fn every_ani_of_the_us_client_finds_a_mesh_choice() {
    let Some(a) = client() else { return };
    let visible = a.index().visible();
    let paths_3dc: Vec<String> = visible.iter().map(|f| f.path.clone()).filter(|p| p.to_ascii_lowercase().ends_with(".3dc")).collect();
    // each .3DC is parsed once: the cache holds the layout and the highest bone, not the bytes
    let mut cache: BTreeMap<String, Option<MeshInfo>> = BTreeMap::new();
    let mut parses = 0usize;
    let (mut total, mut with_mesh): (BTreeMap<String, usize>, BTreeMap<String, usize>) = Default::default();
    let mut rig_defaults: BTreeMap<&str, usize> = BTreeMap::new();
    let mut rig_files: BTreeMap<&str, usize> = BTreeMap::new();
    for f in visible.iter().filter(|f| is_ani(&f.path)) {
        let ani = Ani::parse(&a.read_entry(f.entry).unwrap(), &f.path).unwrap();
        let (compatible, defaults) = mesh_candidates_in(&paths_3dc, &f.path, ani.bones.len(), &mut |p| {
            *cache
                .entry(p.to_string())
                .or_insert_with(|| {
                    parses += 1;
                    let c = shaiya_clientdata::C3dc::parse(&a.read_entry(a.index().get(p)?).ok()?, p).ok()?;
                    Some(MeshInfo { skinned: c.layout != shaiya_clientdata::mesh3dc::C3dcLayout::Cloak, highest_bone: c.meshes.iter().flat_map(|m| m.vertices.iter().map(|v| v.bone1.max(v.bone2))).max() })
                })
        });
        let kind = f.path.split('/').next().unwrap_or("").to_string();
        *total.entry(kind.clone()).or_default() += 1;
        if !compatible.is_empty() {
            *with_mesh.entry(kind).or_default() += 1;
        }
        let info = shaiya_clientdata::anim::rig_of(&f.path);
        if let Some((rig, _)) = info.rig {
            *rig_files.entry(rig).or_default() += 1;
            if !defaults.is_empty() {
                *rig_defaults.entry(rig).or_default() += 1;
            }
        }
    }
    println!("{} .3DC parsed (each once)", parses);
    for (kind, n) in &total {
        println!("{kind}: {} of {n} animations have at least one compatible mesh", with_mesh.get(kind).copied().unwrap_or(0));
    }
    println!("{} .ANI files in all", total.values().sum::<usize>());
    for (rig, _) in RIGS {
        println!("rig {rig}: {} of {} files have default parts", rig_defaults.get(rig).copied().unwrap_or(0), rig_files.get(rig).copied().unwrap_or(0));
        assert!(rig_defaults.get(rig).copied().unwrap_or(0) > 0, "rig {rig} has no default parts");
    }
}

#[test]
fn one_file_per_rig_builds_its_full_preview() {
    let Some(a) = client() else { return };
    let visible = a.index().visible();
    for (rig, race) in RIGS {
        let prefix = format!("character/{race}/ani/{rig}_").to_ascii_lowercase();
        let mut anis: Vec<(String, &shaiya_archive::Entry)> = visible.iter().filter(|f| f.path.to_ascii_lowercase().starts_with(&prefix) && is_ani(&f.path)).map(|f| (f.path.clone(), f.entry)).collect();
        anis.sort_by_key(|(p, _)| p.to_ascii_lowercase());
        let pick = anis.iter().find(|(p, _)| p.to_ascii_lowercase().contains(&format!("{rig}_000_"))).or(anis.first()).unwrap_or_else(|| panic!("rig {rig} has no .ANI"));
        let data = a.read_entry(pick.1).unwrap();
        // one frame set at a time: it is dropped before the next rig starts
        match anim_preview(&a, &data, &pick.0, &MeshChoice::Default, &MeshInfoCache::default(), &Job::silent()).unwrap() {
            PreviewData::Anim { frames, mesh_choices, header, .. } => {
                assert_eq!(frames.unposed_vertices, 0, "rig {rig}");
                assert!(!frames.frames.is_empty(), "rig {rig}: default parts are drawn");
                println!("rig {rig}: {} frames, {} triangles, {} mesh choices; {}", frames.frames.len(), frames.triangles, mesh_choices.len(), header[1]);
            }
            other => panic!("rig {rig}: {other:?}"),
        }
    }
}
