//! The version matrix on the user's own clients. Opt-in: `SHAIYA_TEST_CLIENTS` (the matrix folder, see docs/dev/testing.md) and
//! `SHAIYA_CLIENT` (the US client). Each test skips with a message when its variable is unset.
//!
//! Run: `SHAIYA_TEST_CLIENTS=<root> SHAIYA_CLIENT=<us client> cargo test -p shaiya-matrix-tests -- --nocapture --test-threads=1`

use shaiya_matrix_tests::{differences, markdown, observe, present_clients, Cap, Observed, CAPS};
use shaiya_testkit::{env_client, env_test_clients};
use std::sync::OnceLock;

/// Observed once per test binary: every client is measured (including the writes on stub copies) a single time.
fn observed_all() -> Option<&'static Vec<Observed>> {
    static ALL: OnceLock<Option<Vec<Observed>>> = OnceLock::new();
    ALL.get_or_init(|| {
        let (us, root) = (env_client(), env_test_clients());
        if us.is_none() && root.is_none() {
            eprintln!("skipped: SHAIYA_CLIENT and SHAIYA_TEST_CLIENTS not set");
            return None;
        }
        let clients = present_clients(us.as_deref(), root.as_deref());
        Some(clients.iter().map(|(id, dir)| observe(id, dir)).collect())
    })
    .as_ref()
}

fn check(cap: Cap) {
    let Some(all) = observed_all() else { return };
    let diff: Vec<String> = differences(all).into_iter().filter(|l| l.contains(cap.label())).collect();
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

#[test]
fn archive_visible_count() {
    check(Cap::ArchiveVisibleCount);
}

#[test]
fn archive_check_index() {
    check(Cap::ArchiveCheckIndex);
}

#[test]
fn archive_free_list() {
    check(Cap::ArchiveFreeList);
}

#[test]
fn archive_data_end() {
    check(Cap::ArchiveDataEnd);
}

#[test]
fn archive_diff_self() {
    check(Cap::ArchiveDiffSelf);
}

#[test]
fn archive_edit_rollback_stub() {
    check(Cap::ArchiveEditRollbackStub);
}

#[test]
fn patch_apply_rollback_stub() {
    check(Cap::PatchApplyRollbackStub);
}

#[test]
fn sdata_batch_check() {
    check(Cap::SdataBatchCheck);
}

#[test]
fn sdata_like_roundtrip() {
    check(Cap::SdataLikeRoundtrip);
}

#[test]
fn clientdata_wld_parse() {
    check(Cap::WldParse);
}

#[test]
fn wld_raw_is_lossless() {
    check(Cap::WldLossless);
}

#[test]
fn wld_map_preview_data() {
    check(Cap::WldPreview);
}

#[test]
fn area_txt_is_lossless() {
    check(Cap::AreaTxtRoundtrip);
}

#[test]
fn npcquest_gatekeeper_gates() {
    check(Cap::NpcQuestGates);
}

#[test]
fn sysmsg_has_the_map_name_ids() {
    check(Cap::MapNameIds);
}

#[test]
fn sdata_item_layout_fits() {
    check(Cap::SdataItemFits);
}

#[test]
fn sdata_skill_layout_fits() {
    check(Cap::SdataSkillFits);
}

#[test]
fn sdata_monster_layout_fits() {
    check(Cap::SdataMonsterFits);
}

#[test]
fn sdata_npcquest_layout_fits() {
    check(Cap::SdataNpcQuestFits);
}

/// The consistency check: for every client in the run, the layout `auto` picks for each of its SData files lists that client in the
/// layout's `evidence` (`loader` or `fit_only`); a layout that reads a client's file without naming the client fails here.
#[test]
fn every_layout_that_reads_a_client_lists_that_client() {
    let Some(all) = observed_all() else { return };
    let bad: Vec<String> = all.iter().flat_map(|o| o.unlisted.iter().map(move |u| format!("{}: {u}", o.client))).collect();
    assert!(bad.is_empty(), "add the client to the layout's evidence (loader or fit_only), or fix the layout:\n{}", bad.join("\n"));
}

#[test]
fn sdata_auto_picks_exactly_one_layout_or_none() {
    check(Cap::SdataAutoPicks);
}

/// Prints the observed table (run with `--nocapture`) and the timing of the SAH parse; the table is what `docs/version-matrix.md` mirrors.
#[test]
fn print_the_observed_table() {
    let Some(all) = observed_all() else { return };
    println!("\n{}", markdown(all));
    for o in all {
        if let (Some(files), Some(ms)) = (o.sah_files, o.sah_millis) {
            println!("SAH parse {}: {files} files in {ms:.1} ms", o.client);
        }
        for f in &o.facts {
            println!("fact {}: {f}", o.client);
        }
        for n in &o.notes {
            println!("note {}: {n}", o.client);
        }
    }
    let _ = CAPS;
}

/// Throughput of SEED and SHA-256 on the US client: printed, not asserted.
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

#[test]
fn smod_model_preview_data() {
    check(Cap::ModelPreview);
}

#[test]
fn ani_index_count() {
    check(Cap::AniIndexCount);
}

#[test]
fn ani_round_trip() {
    check(Cap::AniRoundTrip);
}

#[test]
fn ani_json_round_trip() {
    check(Cap::AniJson);
}

#[test]
fn c3dc_index_count() {
    check(Cap::C3dcIndexCount);
}

#[test]
fn c3dc_round_trip() {
    check(Cap::C3dcRoundTrip);
}

#[test]
fn ani_preview() {
    check(Cap::AniPreview);
}

#[test]
fn vani_index_count() {
    check(Cap::VaniIndexCount);
}

#[test]
fn vani_round_trip() {
    check(Cap::VaniRoundTrip);
}

#[test]
fn mani_index_count() {
    check(Cap::ManiIndexCount);
}

#[test]
fn mani_sizes_in_every_index_are_108() {
    check(Cap::ManiIndexSize);
}

#[test]
fn mani_round_trip() {
    check(Cap::ManiRoundTrip);
}

#[test]
fn c3do_index_count() {
    check(Cap::C3doIndexCount);
}

#[test]
fn c3do_round_trip() {
    check(Cap::C3doRoundTrip);
}

#[test]
fn c3de_index_count() {
    check(Cap::C3deIndexCount);
}

#[test]
fn c3de_read_or_refused_with_the_reason() {
    check(Cap::C3deRead);
}

#[test]
fn c3dc_json_round_trip() {
    check(Cap::C3dcJson);
}

#[test]
fn vani_json_round_trip() {
    check(Cap::VaniJson);
}

#[test]
fn mani_json_round_trip() {
    check(Cap::ManiJson);
}

#[test]
fn c3do_json_round_trip() {
    check(Cap::C3doJson);
}

#[test]
fn c3de_json_round_trip_or_refused() {
    check(Cap::C3deJson);
}

#[test]
fn mlt_index_count() {
    check(Cap::MltIndexCount);
}

#[test]
fn mlt_round_trip() {
    check(Cap::MltRoundTrip);
}

#[test]
fn mon_index_count() {
    check(Cap::MonIndexCount);
}

#[test]
fn mon_round_trip() {
    check(Cap::MonRoundTrip);
}

#[test]
fn itm_index_count() {
    check(Cap::ItmIndexCount);
}

#[test]
fn itm_round_trip() {
    check(Cap::ItmRoundTrip);
}

#[test]
fn texture_index_found() {
    check(Cap::TextureIndexFound);
}
