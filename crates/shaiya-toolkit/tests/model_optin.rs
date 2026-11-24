//! Opt-in tests of the `.VANI`, `.MANI`, `.3DO` and `.3DE` previews on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a
//! client folder with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. Run
//! with `-- --test-threads=2` (the frame sets are large; each preview is dropped before the next). The numbers are measured on US EP4.5
//! only; nothing from the client is kept.

use shaiya_archive::Archive;
use shaiya_clientdata::render::{self, AlphaMode, MipImage, View};
use shaiya_core::Job;
use shaiya_testkit::env_client;
use shaiya_toolkit::preview::{make_preview, make_preview_look, MeshChoice, MeshInfo, MeshInfoCache, PreviewData, TextureIndexCache};
use shaiya_toolkit::preview_tex::{decode_mip, Look};
use shaiya_toolkit::source::FileSource;
use std::time::{Duration, Instant};

fn client() -> Option<Archive> {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return None;
    };
    Some(Archive::open(&dir).unwrap())
}

fn paths(a: &Archive, ext: &str) -> Vec<String> {
    a.index().visible().into_iter().map(|f| f.path).filter(|p| p.to_ascii_lowercase().ends_with(ext)).collect()
}

fn preview(a: &Archive, path: &str) -> PreviewData {
    make_preview(a, path, false, 0, &Job::silent()).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn every_vani_of_the_us_client_builds_its_preview() {
    let Some(a) = client() else { return };
    let (mut n, mut steps_total, mut at_cap) = (0usize, 0usize, 0usize);
    for p in paths(&a, ".vani") {
        match preview(&a, &p) {
            PreviewData::Anim { frames, rate, unit, end_key, header, .. } => {
                assert_eq!(unit, "step", "{p}");
                assert!(rate > 0.0, "{p}: {rate}");
                assert_eq!(frames.bones.len(), (end_key as usize) / frames.step as usize + 1, "{p}");
                assert!(!frames.frames.is_empty() && frames.frames.len() <= 241, "{p}: {} pictures", frames.frames.len());
                assert!(header[1].contains("(inferred from the file; not checked in the game)"), "{p}: {header:?}");
                steps_total += end_key as usize + 1;
                at_cap += (frames.frames.len() == 241) as usize;
            }
            other => panic!("{p}: {other:?}"),
        }
        n += 1;
    }
    println!(".VANI previews {n}; steps in all {steps_total}; at the 241 cap {at_cap}");
    assert_eq!(n, 66);
}

#[test]
fn every_mani_of_the_us_client_shows_its_fields() {
    let Some(a) = client() else { return };
    let (mut n, mut with_note) = (0usize, 0usize);
    for p in paths(&a, ".mani") {
        match preview(&a, &p) {
            PreviewData::Fields { header, notes, rows } => {
                assert!(header.contains("Field meanings are inferred"), "{p}");
                assert_eq!(rows.len(), 20, "{p}");
                assert!(rows.iter().all(|r| r.inferred), "{p}");
                with_note += !notes.is_empty() as usize;
            }
            other => panic!("{p}: {other:?}"),
        }
        n += 1;
    }
    println!(".MANI previews {n}; of the integer generation (noted) {with_note}");
    assert_eq!(n, 23);
    assert_eq!(with_note, 12, "the integer generation");
}

#[test]
fn every_3do_of_the_us_client_previews_and_the_framed_one_plays() {
    let Some(a) = client() else { return };
    let (mut n, mut played) = (0usize, Vec::new());
    for p in paths(&a, ".3do") {
        match preview(&a, &p) {
            PreviewData::Model { rgba, .. } => assert!(!rgba.is_empty(), "{p}: nothing drawn"),
            PreviewData::Anim { frames, rate, unit, .. } => {
                assert_eq!(unit, "frame", "{p}");
                played.push((p.clone(), frames.frames.len(), rate));
            }
            other => panic!("{p}: {other:?}"),
        }
        n += 1;
    }
    println!(".3DO previews {n}; played {played:?}");
    assert_eq!(n, 271);
    assert_eq!(played.len(), 1, "{played:?}");
    assert_eq!((played[0].0.to_ascii_lowercase().as_str(), played[0].1, played[0].2), ("item/3do/arrow.3do", 11, 20.0));
}

#[test]
fn every_3de_of_the_us_client_previews_or_is_failed_with_the_reason() {
    let Some(a) = client() else { return };
    let (mut shown, mut played, mut failed) = (0usize, 0usize, Vec::new());
    for p in paths(&a, ".3de") {
        match preview(&a, &p) {
            PreviewData::Model { notes, .. } => {
                assert!(notes.iter().any(|n| n.contains("frame")), "{p}: {notes:?}");
                shown += 1;
            }
            PreviewData::Anim { frames, unit, .. } => {
                assert_eq!(unit, "frame", "{p}");
                assert!(!frames.frames.is_empty() && frames.frames.len() <= 241, "{p}");
                played += 1;
            }
            PreviewData::Failed { message, bytes, .. } => {
                assert!(message.contains(shaiya_clientdata::mesh3de::NOT_THE_LAYOUT), "{p}: {message}");
                assert!(!bytes.is_empty(), "{p}: the bytes are shown as hex");
                failed.push(p.to_ascii_lowercase());
            }
            PreviewData::Empty => panic!("{p}: empty"),
            other => panic!("{p}: {other:?}"),
        }
    }
    failed.sort();
    println!(".3DE previews: {shown} single pictures, {played} played; failed with the reason {failed:?}");
    assert_eq!(shown + played, 467);
    assert_eq!(played, 349, "the files with at least one frame play");
    assert_eq!(failed, ["effect/3de/004.3de", "effect/3de/deathknight007.3de"]);
}

/// The caches of the one client of the run (a preview of any file shares them).
fn caches() -> &'static (MeshInfoCache, TextureIndexCache) {
    static C: std::sync::OnceLock<(MeshInfoCache, TextureIndexCache)> = std::sync::OnceLock::new();
    C.get_or_init(|| (MeshInfoCache::default(), TextureIndexCache::default()))
}

fn look(a: &Archive, path: &str, textured: bool) -> PreviewData {
    let c = caches();
    make_preview_look(a, path, false, 0, &MeshChoice::Default, &c.0, &c.1, &Look { textured, texture: 0, ..Look::default() }, &Job::silent()).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn pictures(p: &PreviewData) -> Vec<&[u8]> {
    match p {
        PreviewData::Anim { frames, .. } => frames.frames.iter().map(|f| f.as_slice()).collect(),
        PreviewData::Model { rgba, .. } => vec![rgba.as_slice()],
        other => panic!("{other:?}"),
    }
}

/// Checks that a model preview with textures differs from the grey one and holds colour; returns the time the textured one took.
fn textured_differs(a: &Archive, path: &str) -> Duration {
    let t = Instant::now();
    let on = look(a, path, true);
    let took = t.elapsed();
    let off = look(a, path, false);
    let (on, off) = (pictures(&on), pictures(&off));
    assert!(!on.is_empty() && on.iter().zip(&off).any(|(x, y)| x != y), "{path}: the textured pictures equal the grey ones");
    took
}

#[test]
fn textured_previews_of_us_client_models() {
    let Some(a) = client() else { return };
    let mut all = a.paths();
    all.sort_by_key(|p| p.to_ascii_lowercase());
    let first = |pred: &dyn Fn(&str) -> bool| all.iter().find(|p| pred(&p.to_ascii_lowercase())).cloned();
    // the default parts of the first huwm animation
    let huwm = first(&|p| p.starts_with("character/human/ani/huwm_") && p.ends_with(".ani")).expect("a huwm animation");
    println!("huwm animation preview with textures: {:?}", textured_differs(&a, &huwm));
    // one monster animation whose parts a monster list textures
    let index = caches().1.get_or_build(&a, &Job::silent()).unwrap();
    let mon = all
        .iter()
        .filter(|p| p.to_ascii_lowercase().starts_with("monster/ani/") && p.to_ascii_lowercase().ends_with(".ani"))
        .find(|p| {
            let (on, off) = (look(&a, p, true), look(&a, p, false));
            matches!(&on, PreviewData::Anim { .. }) && pictures(&on) != pictures(&off)
        })
        .expect("a textured monster animation")
        .clone();
    println!("monster animation preview with textures: {:?}", textured_differs(&a, &mon));
    // one item model with an .ITM texture
    let item = all.iter().find(|p| p.to_ascii_lowercase().starts_with("item/3do/") && index.0.for_mesh(p).iter().any(|t| t.found)).expect("an item model with a texture").clone();
    println!("item model preview with texture: {:?}", textured_differs(&a, &item));
    // one .3DE whose own texture is in Effect/DDS
    let de = all
        .iter()
        .filter(|p| p.to_ascii_lowercase().ends_with(".3de"))
        .find(|p| {
            let Ok(c) = FileSource::read(&a, p, 1 << 20, &Job::silent()) else { return false };
            let Ok(m) = shaiya_clientdata::Mesh3de::parse(&c, p) else { return false };
            index.0.for_effect_mesh(p, &m.texture_name()).is_some()
        })
        .expect("a .3DE with a texture")
        .clone();
    println!("effect mesh preview with texture: {:?}", textured_differs(&a, &de));
    // the largest texture of the monster folders: its decode time
    let big = all.iter().filter(|p| p.to_ascii_lowercase().starts_with("monster/dds/")).max_by_key(|p| a.len(p).unwrap_or(0)).expect("a monster texture").clone();
    let t = Instant::now();
    let mip = decode_mip(&a, &big, &Job::silent()).expect("the texture decodes");
    println!("largest monster texture: {} bytes, {} x {}, read and decoded in {:?}", a.len(&big).unwrap_or(0), mip.width, mip.height, t.elapsed());
}

/// What the worst zoom-8 render costs: the largest monster model on the longest animation it fits, up to 241 frames at zoom 8, textured, on
/// this thread (the job's cancel is asked once per frame in each of the two passes). Prints the numbers; asserts only the number of asks,
/// never a time.
#[test]
fn the_zoom_8_textured_render_of_the_largest_monster() {
    let Some(a) = client() else { return };
    let mut paths = a.paths();
    paths.sort_by_key(|p| p.to_ascii_lowercase());
    let lower = |p: &String| p.to_ascii_lowercase();
    let index = caches().1.get_or_build(&a, &Job::silent()).unwrap();
    // the monster model with the most triangles that has a texture
    let mut best: Option<(usize, String, shaiya_clientdata::C3dc)> = None;
    for p in paths.iter().filter(|p| lower(p).starts_with("monster/3dc/") && lower(p).ends_with(".3dc")) {
        let Ok(bytes) = FileSource::read(&a, p, 32 << 20, &Job::silent()) else { continue };
        let Ok(c) = shaiya_clientdata::C3dc::parse(&bytes, p) else { continue };
        let tris: usize = c.meshes.iter().map(|m| m.faces.len()).sum();
        if tris > best.as_ref().map_or(0, |b| b.0) && index.0.for_mesh(p).first().is_some_and(|t| t.found) {
            best = Some((tris, p.clone(), c));
        }
    }
    let (tris, mesh_path, c3dc) = best.expect("a textured monster model");
    // the longest monster animation the model fits
    let highest = MeshInfo::of(&c3dc).highest_bone;
    let mut anim: Option<(i32, String, shaiya_clientdata::Ani)> = None;
    for p in paths.iter().filter(|p| lower(p).starts_with("monster/ani/") && lower(p).ends_with(".ani")) {
        let Ok(bytes) = FileSource::read(&a, p, 32 << 20, &Job::silent()) else { continue };
        let Ok(ani) = shaiya_clientdata::Ani::parse(&bytes, p) else { continue };
        if highest.is_some_and(|h| (h as usize) < ani.bones.len()) && ani.end_key > anim.as_ref().map_or(0, |b| b.0) {
            anim = Some((ani.end_key, p.clone(), ani));
        }
    }
    let (end_key, ani_path, ani) = anim.expect("a compatible animation");
    let tex = index.0.for_mesh(&mesh_path)[0].clone();
    let mip: MipImage = decode_mip(&a, &tex.texture, &Job::silent()).expect("the texture decodes");
    let meshes: Vec<(&shaiya_clientdata::mesh3dc::C3dcMesh, Option<usize>)> = c3dc.meshes.iter().map(|m| (m, Some(0))).collect();
    let mut asks = 0usize;
    let (mut last, mut worst_gap) = (Instant::now(), Duration::ZERO);
    let start = Instant::now();
    let frames = render::anim_frames_with(&ani, &meshes, std::slice::from_ref(&mip), &[AlphaMode::Cutout], View { yaw_deg: 35.0, pitch_deg: 25.0, zoom: 8.0 }, &mut || {
        let now = Instant::now();
        worst_gap = worst_gap.max(now - last);
        last = now;
        asks += 1;
        false
    })
    .expect("not cancelled");
    let total = start.elapsed();
    worst_gap = worst_gap.max(Instant::now() - last);
    println!(
        "zoom 8, textured, nearest: {tris} triangles, {} frames (end key {end_key}, step {}), total {total:?}, cancel asked {asks} times, longest time between two asks {worst_gap:?}",
        frames.frames.len(),
        frames.step
    );
    println!("model {mesh_path}, animation {ani_path}");
    assert_eq!(asks, 2 * frames.bones.len(), "the cancel is asked once per frame in each of the two passes");
}
