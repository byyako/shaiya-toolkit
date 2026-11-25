//! The check that runs before a file is written to a loose data folder (`shaiya_inspect::loose::format_check`), and the read-only health of a
//! loose folder. These call the real readers (no closures): a broken model, texture or SEED wrapper must be refused by them. Synthetic files.

use shaiya_archive::loose;
use shaiya_clientdata::LooseTree;
use shaiya_core::{Error, Job};
use shaiya_inspect::loose::{format_check, health, is_known_type};
use shaiya_testkit::anim::{arm_ani, arm_mesh_3dc};
use shaiya_testkit::tex::dds_dxt1_solid;
use shaiya_testkit::TempDir;

fn refused(rel: &str, bytes: &[u8]) -> String {
    match format_check(rel, bytes) {
        Err(Error::Refused(s)) => s,
        other => panic!("{rel}: expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_model_that_does_not_read_to_its_last_byte_is_refused_by_the_real_reader() {
    let good = arm_mesh_3dc();
    assert!(format_check("Character/Human/arm.3DC", &good).is_ok(), "a good mesh passes");
    assert!(format_check("Character/Human/ARM.3dc", &good).is_ok(), "letter case of the extension does not matter");
    // garbage, a cut-short file and a file with a byte too many
    assert!(refused("Character/Human/arm.3DC", b"this is not a mesh").contains("does not read to its last byte"));
    assert!(refused("Character/Human/arm.3DC", &good[..good.len() - 3]).contains("does not read to its last byte"));
    let mut longer = good.clone();
    longer.push(0);
    assert!(format_check("Character/Human/arm.3DC", &longer).is_err(), "trailing bytes are not a mesh");
    // an animation the same way
    let ani = arm_ani(10);
    assert!(format_check("Character/Human/ANI/x.ANI", &ani).is_ok());
    assert!(format_check("Character/Human/ANI/x.ANI", &ani[..ani.len() - 1]).is_err());
    assert!(format_check("Character/Human/ANI/x.ANI", b"garbage").is_err());
    // an empty file of a known type
    assert!(refused("Item/x.3DC", b"").contains("empty"));
}

#[test]
fn a_texture_that_does_not_decode_is_refused_by_the_real_decoder() {
    let good = dds_dxt1_solid(8, 8, 0xF800);
    assert!(format_check("Item/tex.dds", &good).is_ok());
    assert!(refused("Item/tex.dds", b"DDS not really").contains("cannot read this texture"));
    assert!(refused("Item/tex.dds", &good[..40]).contains("cannot read this texture"));
    assert!(format_check("Item/tex.tga", b"not a tga").is_err());
}

#[test]
fn an_sdata_file_is_checked_for_its_seed_wrapper_only() {
    let plain = b"a table the toolkit does not read here: 12345678".to_vec();
    let wrapped = shaiya_seed::encrypt(&plain);
    assert!(shaiya_seed::is_encrypted(&wrapped));
    assert!(format_check("Item/Item.SData", &wrapped).is_ok(), "a wrapper with a good checksum");
    // the body of a plain file is NOT read: any bytes pass (the claim of the docs is exactly this)
    assert!(format_check("Item/Item.SData", &plain).is_ok());
    // a damaged body: the checksum fails
    let mut bad = wrapped.clone();
    bad[shaiya_seed::HEADER_SIZE + 5] ^= 0x55;
    let why = refused("Item/Item.SData", &bad);
    assert!(why.contains("checksum") || why.contains("SEED"), "{why}");
    // wrapped twice
    let twice = shaiya_seed::encrypt(&wrapped);
    assert!(refused("Item/Item.SData", &twice).contains("twice"));
    assert!(refused("Item/Item.SData", b"").contains("empty"));
    // a name the toolkit does not read is not looked at
    assert!(!is_known_type("Item/notes.txt") && format_check("Item/notes.txt", b"anything").is_ok());
    assert!(is_known_type("Item/Item.SData") && is_known_type("a/b.3DC") && is_known_type("a/b.dds"));
}

#[test]
fn health_counts_what_the_readers_refuse() {
    let t = TempDir::new("loose-health-check");
    let root = t.path().join("c");
    for (rel, bytes) in [
        ("data/Character/Human/good.3DC", arm_mesh_3dc()),
        ("data/Character/Human/bad.3DC", b"garbage".to_vec()),
        ("data/Item/Item.SData", shaiya_seed::encrypt(b"table table table table")),
        ("data/World/w.txt", b"x".to_vec()),
    ] {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    // a wrapped SData file with a damaged body
    let mut damaged = shaiya_seed::encrypt(b"another table another table");
    damaged[shaiya_seed::HEADER_SIZE + 3] ^= 0x33;
    std::fs::write(root.join("data/Item/Bad.SData"), damaged).unwrap();
    let mut layout = loose::detect(&root).unwrap().unwrap();
    layout.backups = Some(t.path().join("backups"));
    let tree = LooseTree::open(&layout, &Job::silent()).unwrap();
    let h = health(&layout, &tree, &Job::silent()).unwrap();
    let mut failing: Vec<&str> = h.failing_list.iter().map(|(p, _)| p.as_str()).collect();
    failing.sort();
    assert_eq!(failing, ["Character/Human/bad.3DC", "Item/Bad.SData"], "{:?}", h.failing_list);
    assert_eq!(h.failing, 2);
    assert!(!h.running && h.damaged_journals.is_empty() && h.unfinished.is_none());
    assert!(!h.is_clean(), "files that fail their reader need attention");
}
