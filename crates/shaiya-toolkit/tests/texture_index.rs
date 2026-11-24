//! The texture index cache of a loaded archive: built once on first use, shared afterwards, nothing kept by a cancelled build, new for
//! every load. Synthetic files only.

use shaiya_archive::Archive;
use shaiya_clientdata::Found;
use shaiya_core::{Cancel, Job, Silent};
use shaiya_testkit::lists::{mon_bytes, MonRec};
use shaiya_testkit::{SahBuilder, TempDir};
use shaiya_toolkit::preview::TextureIndexCache;
use std::sync::Arc;

fn open(d: &TempDir) -> Archive {
    let mon = mon_bytes(&[MonRec { name: "Bear", flag: 0, slots: [""; 17], parts: &[("bear.3DC", "bear.dds")], size: 1.0, attachments: &[] }]);
    let b = SahBuilder::new(0).file(b"Monster/monster.MON", &mon).file(b"Monster/3DC/bear.3DC", &[0]).file(b"Monster/DDS/bear.dds", &[0]);
    b.write_to(d.path());
    Archive::open(d.path()).unwrap()
}

#[test]
fn the_index_is_built_once_and_shared() {
    let d = TempDir::new("w5-cache");
    let a = open(&d);
    let cache = TextureIndexCache::default();
    assert!(!cache.is_built());
    let first = cache.get_or_build(&a, &Job::silent()).expect("built");
    assert!(cache.is_built());
    let t = first.0.for_mesh("Monster/3DC/bear.3DC");
    assert_eq!((t.len(), t[0].by, first.1.lists_read), (1, Found::OwnFolder, 1));
    let second = cache.get_or_build(&a, &Job::silent()).expect("shared");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&first, &cache.clone().get_or_build(&a, &Job::silent()).unwrap()), "a clone of the cache shares it");
}

#[test]
fn a_cancelled_build_keeps_nothing_and_a_new_load_starts_empty() {
    let d = TempDir::new("w5-cancel");
    let a = open(&d);
    let cache = TextureIndexCache::default();
    let cancel = Cancel::new();
    cancel.cancel();
    let job = Job::new(&Silent, cancel);
    assert!(cache.get_or_build(&a, &job).is_none());
    assert!(!cache.is_built(), "a cancelled build leaves nothing behind");
    assert!(cache.get_or_build(&a, &Job::silent()).is_some());
    assert!(!TextureIndexCache::default().is_built(), "a new load makes a new cache");
}
