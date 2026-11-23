//! The `.3DC` candidate cache of the `.ANI` preview, the note for bones that cannot be posed, and the title of the Replace question.
//! Synthetic files only.

use shaiya_archive::Archive;
use shaiya_core::Job;
use shaiya_inspect::validate::{code, Finding};
use shaiya_testkit::anim::{ani_bytes, arm_ani, arm_mesh_3dc, identity_bits, translation_bits, BoneSpec};
use shaiya_testkit::{SahBuilder, TempDir};
use shaiya_toolkit::preview::{anim_preview, mesh_candidates, MeshChoice, MeshInfoCache, PreviewData, MESH_CANDIDATE_LIMIT};
use shaiya_toolkit::replace_title::findings_title;

const ANI: &str = "Character/Human/ANI/huwm_000_a.ANI";
const MESH: &str = "Character/Human/3DC/huwm_torso001.3DC";

fn open(d: &TempDir, files: &[(&str, Vec<u8>)]) -> Archive {
    let mut b = SahBuilder::new(0);
    for (p, data) in files {
        b = b.file(p.as_bytes(), data);
    }
    b.write_to(d.path());
    Archive::open(d.path()).unwrap()
}

fn spec(parent: i32, matrix: [u32; 16]) -> BoneSpec {
    BoneSpec { parent, matrix, rot: Vec::new(), trans: Vec::new() }
}

#[test]
fn a_nan_reference_matrix_shows_a_note() {
    let mut bad = translation_bits(0.0, 2.0, 0.0);
    bad[13] = 0xFFC0_0000;
    let bones = [spec(-1, identity_bits()), spec(0, translation_bits(0.0, 1.0, 0.0)), spec(1, bad)];
    let d = TempDir::new("w6-nan");
    let a = open(&d, &[(ANI, ani_bytes(0, 20, &bones)), (MESH, arm_mesh_3dc())]);
    let data = a.read_entry(a.index().get(ANI).unwrap()).unwrap();
    match anim_preview(&a, &data, ANI, &MeshChoice::Default, &MeshInfoCache::default(), &Job::silent()).unwrap() {
        PreviewData::Anim { frames, notes, .. } => {
            assert_eq!(frames.nonfinite_bones, 1);
            assert!(notes.iter().any(|n| n == "1 bone cannot be posed (a reference matrix holds NaN or infinity, or cannot be inverted) and is not drawn"), "{notes:?}");
        }
        other => panic!("{other:?}"),
    }
    // a file without such a bone has no such note
    let d = TempDir::new("w6-nan-none");
    let a = open(&d, &[(ANI, arm_ani(20)), (MESH, arm_mesh_3dc())]);
    let data = a.read_entry(a.index().get(ANI).unwrap()).unwrap();
    match anim_preview(&a, &data, ANI, &MeshChoice::Default, &MeshInfoCache::default(), &Job::silent()).unwrap() {
        PreviewData::Anim { notes, .. } => assert!(!notes.iter().any(|n| n.contains("cannot be posed")), "{notes:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn mesh_candidates_are_parsed_once_per_archive() {
    let d = TempDir::new("w6-once");
    let a = open(&d, &[(ANI, arm_ani(60)), (MESH, arm_mesh_3dc()), ("Character/Human/3DC/huwm_hand001.3DC", arm_mesh_3dc())]);
    let cache = MeshInfoCache::default();
    let reads = std::cell::Cell::new(0usize);
    let mut read = |p: &str| {
        reads.set(reads.get() + 1);
        a.index().get(p).and_then(|e| a.read_entry(e).ok())
    };
    let (first, _) = mesh_candidates(a.index(), ANI, 3, &cache, &mut read);
    assert_eq!(first.len(), 2);
    assert_eq!(reads.get(), 2);
    let (second, _) = mesh_candidates(a.index(), ANI, 3, &cache, &mut read);
    assert_eq!(second, first);
    assert_eq!(reads.get(), 2, "the second call reads and parses no candidate again");
    assert_eq!(cache.len(), 2);
    // a different bone count reuses the same answers
    let (none, _) = mesh_candidates(a.index(), ANI, 2, &cache, &mut read);
    assert!(none.is_empty());
    assert_eq!(reads.get(), 2);
    // the preview on the same cache still reads and draws the chosen meshes
    let data = a.read_entry(a.index().get(ANI).unwrap()).unwrap();
    for _ in 0..2 {
        match anim_preview(&a, &data, ANI, &MeshChoice::Default, &cache, &Job::silent()).unwrap() {
            PreviewData::Anim { frames, mesh_choices, .. } => {
                assert!(!frames.frames.is_empty(), "the chosen meshes are read for rendering");
                assert_eq!(mesh_choices.len(), 4);
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(cache.len(), 2);
}

#[test]
fn a_changed_entry_is_read_again() {
    let cache = MeshInfoCache::default();
    let reads = std::cell::Cell::new(0usize);
    let d1 = TempDir::new("w6-changed-a");
    let a1 = open(&d1, &[(ANI, arm_ani(60)), (MESH, arm_mesh_3dc())]);
    let (c1, _) = mesh_candidates(a1.index(), ANI, 3, &cache, &mut |p| {
        reads.set(reads.get() + 1);
        a1.index().get(p).and_then(|e| a1.read_entry(e).ok())
    });
    assert_eq!((c1.len(), reads.get()), (1, 1));
    // the same path at another offset (a file now sits before it): read again
    let d2 = TempDir::new("w6-changed-b");
    let a2 = open(&d2, &[("A/first.txt", b"some bytes before the mesh".to_vec()), (ANI, arm_ani(60)), (MESH, arm_mesh_3dc())]);
    let (c2, _) = mesh_candidates(a2.index(), ANI, 3, &cache, &mut |p| {
        reads.set(reads.get() + 1);
        a2.index().get(p).and_then(|e| a2.read_entry(e).ok())
    });
    assert_eq!((c2.len(), reads.get()), (1, 2), "a different offset is read again");
    // the same path, same offset, another length: read again
    let d3 = TempDir::new("w6-changed-c");
    let mut longer = arm_mesh_3dc();
    longer.extend_from_slice(&[0; 8]);
    let a3 = open(&d3, &[("A/first.txt", b"some bytes before the mesh".to_vec()), (ANI, arm_ani(60)), (MESH, longer)]);
    let _ = mesh_candidates(a3.index(), ANI, 3, &cache, &mut |p| {
        reads.set(reads.get() + 1);
        a3.index().get(p).and_then(|e| a3.read_entry(e).ok())
    });
    assert_eq!(reads.get(), 3, "a different length is read again");
    // unchanged again: served from the cache
    let _ = mesh_candidates(a3.index(), ANI, 3, &cache, &mut |_| {
        reads.set(reads.get() + 1);
        None
    });
    assert_eq!(reads.get(), 3);
}

#[test]
fn a_candidate_over_the_size_cap_is_not_read_or_offered() {
    let d = TempDir::new("w6-cap");
    let big = vec![0u8; MESH_CANDIDATE_LIMIT as usize + 1];
    let a = open(&d, &[(ANI, arm_ani(60)), (MESH, arm_mesh_3dc()), ("Character/Human/3DC/huwm_big001.3DC", big)]);
    let mut asked = Vec::new();
    let (all, _) = mesh_candidates(a.index(), ANI, 3, &MeshInfoCache::default(), &mut |p| {
        asked.push(p.to_string());
        a.index().get(p).and_then(|e| a.read_entry(e).ok())
    });
    assert_eq!(all, vec![MESH.to_string()]);
    assert_eq!(asked, vec![MESH.to_string()], "the large entry is never read");
}

fn finding(code: &'static str) -> Finding {
    Finding { path: "x".into(), code, message: "m".into() }
}

#[test]
fn the_replace_question_names_a_skeleton_difference() {
    assert_eq!(findings_title(&[finding(code::ANIM_RIG_DIFFERS)]), "The skeleton differs from the file it replaces");
    // findings that only warn do not change the title
    assert_eq!(findings_title(&[finding(code::ANIM_RIG_DIFFERS), finding(code::ANIM_FIRST_FIELD_DIFFERS)]), "The skeleton differs from the file it replaces");
    assert_eq!(findings_title(&[finding(code::ANIM_RIG_DIFFERS), finding(code::SDATA_CRC_BAD)]), "This file looks broken");
    assert_eq!(findings_title(&[finding(code::SDATA_CRC_BAD)]), "This file looks broken");
    assert_eq!(findings_title(&[]), "This file looks broken");
}
