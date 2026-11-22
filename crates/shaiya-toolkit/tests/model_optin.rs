//! Opt-in tests of the `.VANI`, `.MANI`, `.3DO` and `.3DE` previews on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a
//! client folder with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. Run
//! with `-- --test-threads=2` (the frame sets are large; each preview is dropped before the next). The numbers are measured on US EP4.5
//! only; nothing from the client is kept.

use shaiya_archive::Archive;
use shaiya_core::Job;
use shaiya_testkit::env_client;
use shaiya_toolkit::preview::{make_preview, PreviewData};

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
    make_preview(a, a.index().get(path).unwrap(), path, false, 0, &Job::silent()).unwrap_or_else(|e| panic!("{path}: {e}"))
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
    let (mut shown, mut failed) = (0usize, Vec::new());
    for p in paths(&a, ".3de") {
        match preview(&a, &p) {
            PreviewData::Model { notes, .. } => {
                assert!(notes.iter().any(|n| n.contains("frame")), "{p}: {notes:?}");
                shown += 1;
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
    println!(".3DE previews {shown}; failed with the reason {failed:?}");
    assert_eq!(shown, 467);
    assert_eq!(failed, ["effect/3de/004.3de", "effect/3de/deathknight007.3de"]);
}
