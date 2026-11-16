//! `ClientSource` in archive mode on a synthetic `data.sah`/`data.saf`, and the opt-in tests on the user's own client (`SHAIYA_CLIENT`).

use shaiya_clientdata::seed;
use shaiya_clientdata::{ClientSource, Error};
use shaiya_testkit::{SahBuilder, TempDir};

fn utf16(s: &str) -> Vec<u8> {
    let mut v = vec![0xff, 0xfe];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

#[test]
fn archive_mode_reads_entries_case_insensitively() {
    let d = TempDir::new("cd-archive");
    let enc = shaiya_seed::encrypt(b"not a real monster file");
    SahBuilder::new(0)
        .file(b"world/2.txt", &utf16("0\tFirst\r\n"))
        .file(b"Monster/Monster.SData", &enc)
        .file(b"sysmsg-uni.txt", &utf16("5068\t\t\"D-Water Borderland (Border)\"\r\n"))
        .write_to(d.path());
    let c = ClientSource::open(d.path()).unwrap();
    assert!(c.is_archive());
    assert!(c.exists("WORLD/2.TXT") && !c.exists("world/3.txt"));
    assert!(matches!(c.read("world/3.txt"), Err(Error::NotFound(_))));
    assert_eq!(c.area_names(2).unwrap().get(0), Some("First"));
    assert_eq!(c.map_name(0), "D-Water Borderland (Border)");
    let mut files = c.list();
    files.sort();
    assert_eq!(files, vec!["Monster/Monster.SData", "sysmsg-uni.txt", "world/2.txt"]);
    // the SData decrypts through the seed shim, then the (fake) body fails to parse as a Monster table
    assert!(c.monsters().is_err());
    assert_eq!(seed::decrypt(&c.read("Monster/Monster.SData").unwrap(), "m").unwrap(), b"not a real monster file");
    // an archive without its data file, and a folder that is neither
    std::fs::remove_file(d.path().join("data.saf")).unwrap();
    assert!(ClientSource::open(d.path()).is_err());
}

#[test]
fn opt_in_client_sdata_round_trip_and_readers() {
    let Some(client) = shaiya_testkit::env_client() else { return };
    if !["data.sah", "Data.sah", "DATA.SAH"].iter().any(|n| client.join(n).is_file()) {
        eprintln!("skipped: no data.sah in SHAIYA_CLIENT");
        return;
    }
    let src = ClientSource::open(&client).unwrap();
    assert!(src.is_archive());
    let sdata: Vec<String> = src.list().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sdata")).collect();
    assert!(!sdata.is_empty(), "the client has SData files");
    let (mut bytes, mut seed_secs) = (0usize, 0f64);
    let mut encrypted = 0;
    for p in &sdata {
        let raw = src.read(p).unwrap();
        let t = std::time::Instant::now();
        let (plain, wrapper) = shaiya_seed::unwrap(&raw, p).unwrap_or_else(|e| panic!("{p}: {e}"));
        seed_secs += t.elapsed().as_secs_f64();
        bytes += raw.len();
        assert_eq!(shaiya_seed::wrap(&plain, &wrapper), raw, "{p}: wrap(unwrap(x)) must equal x");
        if matches!(wrapper, shaiya_seed::Wrapper::Seed { .. }) {
            encrypted += 1;
        }
    }
    eprintln!("SData: {} files ({encrypted} SEED-encrypted), {bytes} bytes, decrypt {:.1} MB/s", sdata.len(), bytes as f64 / 1e6 / seed_secs.max(1e-9));
    let monsters = src.monsters().unwrap();
    let quests = src.npc_quest().unwrap();
    eprintln!("Monster records: {}, NpcQuest parsed", monsters.records.len());
    let _ = quests;
}
