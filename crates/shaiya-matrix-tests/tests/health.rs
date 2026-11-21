//! Opt-in: client health on the user's own clients. Read only on the originals; clean-up and Put back run only on a scratch copy.
//!
//! - `SHAIYA_CLIENT` and/or `SHAIYA_TEST_CLIENTS`: `health::report` of every present matrix client (`ru-ep54` has an index and no
//!   `data.saf`: the index check and a missing-`data.saf` finding only). The numbers are printed; the folder's entries (names, sizes,
//!   times) are the same before and after.
//! - plus `SHAIYA_SCRATCH`: a copy of each client's `data.sah` with an empty `data.saf` goes to a new folder under
//!   the scratch folder, gets three leftovers (`data.sah.bak`, a stray temp file, a swap pair), is cleaned up, put back, and compared byte for byte; the copy is removed.
//!
//! Run: `SHAIYA_TEST_CLIENTS=<root> SHAIYA_CLIENT=<us client> SHAIYA_SCRATCH=<new empty folder> cargo test -p shaiya-matrix-tests --test health -- --nocapture`

use shaiya_archive::cleanup;
use shaiya_archive::health::{self, Kind};
use shaiya_core::Job;
use shaiya_matrix_tests::present_clients;
use shaiya_testkit::{env_client, env_scratch, env_test_clients};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Name, size and modification time of every entry of the folder (one level; the clients keep their data files at the top).
fn entries(dir: &Path) -> BTreeMap<String, (u64, Option<std::time::SystemTime>)> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| {
        let m = e.metadata().unwrap();
        (e.file_name().to_string_lossy().into_owned(), (m.len(), m.modified().ok()))
    }).collect()
}

fn clients() -> Option<Vec<(String, PathBuf)>> {
    let (us, root) = (env_client(), env_test_clients());
    if us.is_none() && root.is_none() {
        eprintln!("skipped: SHAIYA_CLIENT and SHAIYA_TEST_CLIENTS not set");
        return None;
    }
    Some(present_clients(us.as_deref(), root.as_deref()))
}

#[test]
fn health_of_every_present_client_reads_only() {
    let Some(all) = clients() else { return };
    assert!(!all.is_empty(), "no client found under the given folders");
    for (id, dir) in &all {
        let before = entries(dir);
        let h = health::report(dir).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(entries(dir), before, "{id}: health changed the folder");
        let dead = h.dead_space.as_ref().map(|d| d.dead_bytes.to_string()).unwrap_or_else(|| "no data.saf".to_string());
        let kinds: Vec<&str> = h.leftovers.iter().map(|l| l.kind.name()).collect();
        println!(
            "HEALTH {id}: SAH v{}, {} files, {} problems, {} warnings, saf {}, dead space {dead}, {} journals, pending {}, StartUpdate {:?}, leftovers {kinds:?}, not offered {}, cleanups {}, findings {}",
            h.index.sah_version,
            h.index.files,
            h.index.problems,
            h.index.warnings,
            h.index.saf_present,
            h.journals.count,
            h.pending_marker.is_some(),
            h.start_update,
            h.not_offered.len(),
            h.cleanups.len(),
            h.findings()
        );
    }
}

#[test]
fn clean_up_and_put_back_on_a_scratch_copy_of_each_clients_index() {
    let (Some(all), Some(scratch)) = (clients(), env_scratch()) else { return };
    for (id, dir) in &all {
        let sah = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("data.sah"))).unwrap_or_else(|| panic!("{id}: no data.sah"));
        let copy = scratch.join(format!("w6-health-{id}-{}", std::process::id()));
        std::fs::create_dir_all(&copy).unwrap();
        let guard = Remove(copy.clone());
        std::fs::copy(&sah, copy.join("data.sah")).unwrap();
        std::fs::write(copy.join("data.saf"), b"").unwrap();
        std::fs::write(copy.join("data.sah.bak"), b"old index").unwrap();
        std::fs::write(copy.join(".n.7.shaiya-tmp"), b"half").unwrap();
        std::fs::write(copy.join("data.20261101T101010Z.sah"), b"s1").unwrap();
        std::fs::write(copy.join("data.20261101T101010Z.saf"), b"s2").unwrap();
        let before: BTreeMap<String, Vec<u8>> = std::fs::read_dir(&copy).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        let h = health::report(&copy).unwrap();
        let ids = cleanup::ids_of_kinds(&h, &[Kind::SahBak, Kind::StrayTemp, Kind::SwapPair]);
        assert_eq!(ids.len(), 3, "{id}");
        let rep = cleanup::clean(&copy, &ids, &Job::silent()).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(rep.moved.len(), 4, "{id}");
        assert!(health::report(&copy).unwrap().leftovers.is_empty(), "{id}");
        let back = cleanup::put_back(&rep.path, &Job::silent()).unwrap();
        assert_eq!(back.restored.len(), 4, "{id}");
        let after: BTreeMap<String, Vec<u8>> = std::fs::read_dir(&copy).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        assert_eq!(after, before, "{id}: names and bytes are back");
        println!("CLEAN {id}: moved {} files and put them back, index {} bytes", rep.moved.len(), std::fs::metadata(copy.join("data.sah")).unwrap().len());
        drop(guard);
    }
}

struct Remove(PathBuf);
impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
