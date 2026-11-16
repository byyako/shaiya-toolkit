//! The version matrix on the user's own clients. Opt-in: `SHAIYA_TEST_CLIENTS` (the matrix folder, see docs/dev/testing.md) and
//! `SHAIYA_CLIENT` (the US client). Each test skips with a message when its variable is unset.
//!
//! Run: `SHAIYA_TEST_CLIENTS=<root> SHAIYA_CLIENT=<us client> cargo test -p shaiya-matrix-tests -- --nocapture --test-threads=1`

use shaiya_matrix_tests::{differences, markdown, observe, present_clients, Cap, Observed, CAPS};
use shaiya_testkit::{env_client, env_test_clients};

fn observed_all() -> Option<Vec<Observed>> {
    let (us, root) = (env_client(), env_test_clients());
    if us.is_none() && root.is_none() {
        return None;
    }
    let clients = present_clients(us.as_deref(), root.as_deref());
    Some(clients.iter().map(|(id, dir)| observe(id, dir)).collect())
}

fn check(cap: Cap) {
    let Some(all) = observed_all() else { return };
    let diff: Vec<String> = differences(&all).into_iter().filter(|l| l.contains(cap.label())).collect();
    assert!(diff.is_empty(), "{}", diff.join("\n"));
}

#[test]
fn index_parse() {
    check(Cap::IndexParse);
}

#[test]
fn to_bytes_is_byte_identical() {
    check(Cap::ToBytesIdentical);
}

#[test]
fn inspect_on_data_sah() {
    check(Cap::InspectSah);
}

#[test]
fn seed_unwrap_with_crc_on_every_sdata() {
    check(Cap::SeedUnwrap);
}

#[test]
fn wrap_of_unwrap_is_identical_on_every_sdata() {
    check(Cap::WrapIdentity);
}

#[test]
fn clientdata_monsters() {
    check(Cap::Monsters);
}

#[test]
fn clientdata_npcquest() {
    check(Cap::NpcQuest);
}

#[test]
fn inspect_on_game_exe() {
    check(Cap::InspectPe);
}

/// Prints the observed table (run with `--nocapture`) and the timing of the SAH parse; the table is what `docs/version-matrix.md` mirrors.
#[test]
fn print_the_observed_table() {
    let Some(all) = observed_all() else { return };
    println!("\n{}", markdown(&all));
    for o in &all {
        if let (Some(files), Some(ms)) = (o.sah_files, o.sah_millis) {
            println!("SAH parse {}: {files} files in {ms:.1} ms", o.client);
        }
        for n in &o.notes {
            println!("note {}: {n}", o.client);
        }
    }
    let _ = CAPS;
}

/// Throughput of SEED and SHA-256 on the US client (spec section 12): printed, not asserted.
#[test]
fn performance_on_the_us_client() {
    let Some(us) = env_client() else { return };
    let files = shaiya_matrix_tests::sdata_files(&us).expect("SData files of the US client");
    let (mut bytes, mut secs) = (0usize, 0f64);
    for (name, raw) in &files {
        let t = std::time::Instant::now();
        let (plain, w) = shaiya_seed::unwrap(raw, name).unwrap();
        let _ = shaiya_seed::wrap(&plain, &w);
        secs += t.elapsed().as_secs_f64();
        bytes += raw.len() * 2;
    }
    println!("SEED decrypt + encrypt: {:.1} MB/s over {} files ({} bytes processed)", bytes as f64 / 1e6 / secs.max(1e-9), files.len(), bytes);
    if let Some(saf) = ["data.saf", "Data.saf", "DATA.SAF"].iter().map(|n| us.join(n)).find(|p| p.is_file()) {
        let size = std::fs::metadata(&saf).unwrap().len();
        let t = std::time::Instant::now();
        let h = shaiya_core::hash::sha256_file(&saf, &shaiya_core::Job::silent()).unwrap();
        let s = t.elapsed().as_secs_f64();
        println!("SHA-256 of data.saf: {} in {:.1} s = {:.0} MB/s ({})", shaiya_core::hash::to_hex(&h), s, size as f64 / 1e6 / s, size);
    }
}
