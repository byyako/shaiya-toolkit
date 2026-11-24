//! Opt-in test of the texture index on a real client (read only). `SHAIYA_CLIENT` is a client folder with `data.sah` and `data.saf`
//! (US EP4.5); the test prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. The numbers are measured on US EP4.5 only.

use shaiya_archive::Archive;
use shaiya_clientdata::texindex::{Found, IndexReport, TextureIndex};
use shaiya_testkit::env_client;

fn build(a: &Archive, only: &str) -> (TextureIndex, IndexReport) {
    // every file, but only the list files of one extension, so the report is of one kind
    let paths: Vec<String> = a.index().visible().into_iter().map(|f| f.path).filter(|p| {
        let l = p.to_ascii_lowercase();
        !(l.ends_with(".mlt") || l.ends_with(".mon") || l.ends_with(".itm")) || l.ends_with(only)
    }).collect();
    TextureIndex::build(&paths, &mut |p| a.index().get(p).and_then(|e| a.read_entry(e).ok()), &mut || false).unwrap()
}

#[test]
fn the_us_client_texture_index_numbers() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let (_, mlt) = build(&a, ".mlt");
    let (_, mon) = build(&a, ".mon");
    let (itm_idx, itm) = build(&a, ".itm");
    for (k, r) in [("MLT", &mlt), ("MON", &mon), ("ITM", &itm)] {
        println!("{k}: lists {} refused {:?}, pairs {}; texture own {} elsewhere {} missing {}; mesh own {} elsewhere {} missing {}", r.lists_read, r.lists_refused, r.records, r.own, r.elsewhere, r.missing, r.mesh_own, r.mesh_elsewhere, r.mesh_missing);
    }
    let (all, rep) = build(&a, "");
    println!("all: meshes with a texture pair {}, with a found texture {}", all.mesh_count(), all.textured_mesh_count());
    assert!(mlt.lists_refused.is_empty() && mon.lists_refused.is_empty() && itm.lists_refused.is_empty());
    assert_eq!((mlt.lists_read, mon.lists_read, itm.lists_read), (112, 16, 17));
    assert_eq!((mlt.records, mlt.mesh_own, mlt.own), (2_176, 2_151, 2_087), "MLT own folder");
    assert_eq!((mon.records, mon.mesh_own, mon.own), (2_005, 1_753, 1_745), "MON own folder");
    // the report states the elsewhere and nowhere numbers for the `.MON` lists alone; the `.MLT` lists find nothing elsewhere
    assert_eq!((mon.mesh_elsewhere, mon.elsewhere, mon.mesh_missing, mon.missing), (182, 83, 70, 177), "MON: found elsewhere by bare name, found nowhere");
    assert_eq!((mlt.mesh_elsewhere, mlt.elsewhere, mlt.mesh_missing, mlt.missing), (0, 0, 25, 89), "MLT: nothing elsewhere; 3DC and DDS found nowhere");
    assert_eq!((itm.mesh_missing, itm.missing), (0, 0), "every item name resolves");
    assert_eq!((itm.mesh_elsewhere, itm.elsewhere), (0, 0), "in the item folders");
    assert!(!itm_idx.for_mesh("Item/3DO/anything.3DO").iter().any(|t| t.by == Found::Missing));
    assert_eq!(rep.records, mlt.records + mon.records + itm.records);
    println!("matrix number: {}", all.textured_mesh_count());
}
