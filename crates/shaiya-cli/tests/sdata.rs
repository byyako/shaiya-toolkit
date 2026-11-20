//! `shaiya sdata ...`: info, check, decrypt, encrypt over files, folders and an archive.

mod common;

use common::*;
use shaiya_seed::{encrypt, is_encrypted, unwrap, wrap, Wrapper};

fn odd_seed(plain: &[u8]) -> Vec<u8> {
    wrap(plain, &Wrapper::Seed { crc: 0, size: plain.len() as u32, padding: [9; 16], tail: vec![7; (16 - plain.len() % 16) % 16] })
}

#[test]
fn info_and_check() {
    let w = world();
    let dir = w.tmp.path().join("sd");
    write(&dir.join("Item.SData"), &encrypt(b"0123456789"));
    write(&dir.join("GuildHouse.SData"), b"plain text file");
    write(&dir.join("notes.txt"), b"not an sdata file");
    // files and a folder (only *.SData from folders)
    let o = sh(&w.data, &["sdata", "info", p(&dir)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("SEED, plaintext 10 bytes, CRC 0x") && t.contains("OK, header padding zero, tail zero") && t.contains("not encrypted, 15 bytes"), "{t}");
    assert!(!t.contains("notes.txt"));
    assert!(out(&sh(&w.data, &["sdata", "info", p(&dir), "--all-files"])).contains("notes.txt"));
    // an explicit file is never filtered by name
    assert!(out(&sh(&w.data, &["sdata", "info", p(&dir.join("notes.txt"))])).contains("not encrypted, 17 bytes"));
    // from an archive, by default every *.sdata
    let o = sh(&w.data, &["sdata", "info", "--from", p(&w.client)]);
    assert_eq!(code(&o), 0);
    assert_eq!(out(&o).lines().count(), 3, "{}", out(&o));
    let v = json(&sh(&w.data, &["sdata", "check", "--from", p(&w.client), "Item/*", "--json"]));
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["status"], "crc_ok");
    assert_eq!(v["summary"]["crc_ok"], 1);
    // a bad checksum: check exits 1, info shows FAILED
    let mut bad = encrypt(b"abcdefghij");
    bad[40] ^= 1;
    write(&dir.join("Bad.SData"), &bad);
    let o = sh(&w.data, &["sdata", "check", p(&dir)]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("Bad.SData") && out(&o).contains("FAILED"), "{}", out(&o));
    assert_eq!(code(&sh(&w.data, &["sdata", "info", p(&dir)])), 0, "info reports, it does not judge");
    // a missing input is an input error, no input at all is a usage error
    assert_eq!(code(&sh(&w.data, &["sdata", "info", p(&w.tmp.path().join("nothing.SData"))])), 3);
    assert_eq!(code(&sh(&w.data, &["sdata", "info"])), 2);
}

#[test]
fn decrypt_and_encrypt_round_trip_with_like() {
    let w = world();
    let plain = w.tmp.path().join("plain");
    // decrypt from the archive: names kept, plain files copied
    let o = sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "-o", p(&plain)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("Item/Item.SData  decrypted") && out(&o).contains("Cn/Plain.SData  plain, copied"), "{}", out(&o));
    assert_eq!(std::fs::read(plain.join("Item/Item.SData")).unwrap(), ITEM_PLAIN);
    assert!(err(&o).contains("3 files"), "{}", err(&o));
    // existing outputs: exit 4; --skip-existing and --force
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "-o", p(&plain)])), 4);
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "-o", p(&plain), "--skip-existing"])), 0);
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "-o", p(&plain), "--force"])), 0);
    // --skip-plain leaves the plain file out
    let only = w.tmp.path().join("only");
    sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "-o", p(&only), "--skip-plain"]);
    assert!(only.join("Item/Item.SData").exists() && !only.join("Cn/Plain.SData").exists());
    // edit, then encrypt back --like the client: unchanged files are byte-identical
    write(&plain.join("Monster/Monster.SData"), b"monster plaintext EDITED");
    let enc = w.tmp.path().join("enc");
    let o = sh(&w.data, &["sdata", "encrypt", p(&plain), "-o", p(&enc), "--like", p(&w.client)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let orig = |n: &str| {
        let o = sh(&w.data, &["archive", "cat", p(&w.client), n]);
        o.stdout
    };
    assert_eq!(std::fs::read(enc.join("Item/Item.SData")).unwrap(), orig("Item/Item.SData"), "unchanged and byte-identical");
    assert_eq!(std::fs::read(enc.join("Cn/Plain.SData")).unwrap(), orig("Cn/Plain.SData"), "plain stays plain");
    let m = std::fs::read(enc.join("Monster/Monster.SData")).unwrap();
    assert!(is_encrypted(&m));
    assert_eq!(unwrap(&m, "m").unwrap().0, b"monster plaintext EDITED");
    // --like @name works too, and a dry run writes nothing
    save_client(&w, "us");
    let enc2 = w.tmp.path().join("enc2");
    let o = sh(&w.data, &["sdata", "encrypt", p(&plain), "-o", p(&enc2), "--like", "@us", "--dry-run", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["dry_run"], true);
    assert!(!enc2.exists());
    // encrypt without --like, a single file to a single file
    let one = w.tmp.path().join("one.txt");
    write(&one, b"just some text");
    let outf = w.tmp.path().join("one.SData");
    let o = sh(&w.data, &["sdata", "encrypt", p(&one), "-o", p(&outf), "--all-files"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(unwrap(&std::fs::read(&outf).unwrap(), "x").unwrap().0, b"just some text");
}

#[test]
fn in_place_checksums_and_refusals() {
    let w = world();
    let dir = w.tmp.path().join("sd");
    let original = odd_seed(b"in place test data");
    write(&dir.join("A.SData"), &original);
    // -o and --in-place together: a usage error; neither: refused
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", p(&dir), "-o", "x", "--in-place"])), 2);
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", p(&dir)])), 4);
    // -o naming the input folder is refused without --in-place
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", p(&dir), "-o", p(&dir)])), 4);
    // in place: the backup holds the original
    let o = sh(&w.data, &["sdata", "decrypt", p(&dir), "--in-place"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(std::fs::read(dir.join("A.SData")).unwrap(), b"in place test data");
    assert_eq!(std::fs::read(dir.join("A.SData.bak")).unwrap(), original);
    // --in-place from an archive is refused
    assert_eq!(code(&sh(&w.data, &["sdata", "decrypt", "--from", p(&w.client), "--in-place"])), 4);
    // a damaged file: strict decrypt fails that item (exit 3), --unchecked writes it (exit 1)
    let mut bad = encrypt(b"damaged plaintext");
    bad[40] ^= 1;
    let bd = w.tmp.path().join("bad");
    write(&bd.join("Bad.SData"), &bad);
    let o = sh(&w.data, &["sdata", "decrypt", p(&bd), "-o", p(&w.tmp.path().join("o1"))]);
    assert_eq!(code(&o), 3);
    assert!(out(&o).contains("FAILED"), "{}", out(&o));
    assert!(!w.tmp.path().join("o1").exists());
    let o = sh(&w.data, &["sdata", "decrypt", p(&bd), "-o", p(&w.tmp.path().join("o2")), "--unchecked"]);
    assert_eq!(code(&o), 1);
    assert_eq!(std::fs::read(w.tmp.path().join("o2")).unwrap(), b"damaged plaintext");
    // encrypt of an already encrypted file copies it unless --skip-encrypted
    let o = sh(&w.data, &["sdata", "encrypt", p(&bd), "-o", p(&w.tmp.path().join("o3"))]);
    assert_eq!(code(&o), 0);
    assert!(out(&o).contains("encrypted, copied"), "{}", out(&o));
    let o = sh(&w.data, &["sdata", "encrypt", p(&bd), "-o", p(&w.tmp.path().join("o4")), "--skip-encrypted"]);
    assert!(out(&o).contains("skipped") && !w.tmp.path().join("o4").exists(), "{}", out(&o));
    // nothing to encrypt
    assert_eq!(code(&sh(&w.data, &["sdata", "encrypt", "-o", "x"])), 4);
}

#[test]
fn every_sdata_command_has_help() {
    let w = world();
    for c in ["info", "check", "decrypt", "encrypt"] {
        let o = sh(&w.data, &["sdata", c, "--help"]);
        assert_eq!(code(&o), 0, "{c}");
        assert!(out(&o).contains("Example:"), "{c}: {}", out(&o));
    }
}

fn put_u32(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

fn put_str(v: &mut Vec<u8>, b: &[u8]) {
    put_u32(v, b.len() as u32);
    v.extend_from_slice(b);
}

/// A Monster table: the names, each with a 31-byte block whose first field (model) is 100 + the index.
fn monster_table(names: &[&[u8]]) -> Vec<u8> {
    let mut v = Vec::new();
    put_u32(&mut v, names.len() as u32);
    for (i, n) in names.iter().enumerate() {
        put_str(&mut v, n);
        let mut b = [0u8; 31];
        b[0..2].copy_from_slice(&(100 + i as i16).to_le_bytes());
        v.extend_from_slice(&b);
    }
    v
}

/// An NpcQuest table with one merchant and one quest (a table with no quests would fit both quest shapes).
fn npcquest_table() -> Vec<u8> {
    let mut v = Vec::new();
    put_u32(&mut v, 1);
    v.extend_from_slice(&[0u8; 20]);
    put_str(&mut v, b"Smith");
    put_str(&mut v, b"Hello");
    for _ in 0..3 {
        put_u32(&mut v, 0);
    }
    for _ in 0..12 {
        put_u32(&mut v, 0);
    }
    v.extend(std::iter::repeat_n(0u8, 65536 * 8));
    put_u32(&mut v, 1);
    v.extend_from_slice(&1u16.to_le_bytes());
    put_str(&mut v, b"Title");
    put_str(&mut v, b"Text");
    v.extend_from_slice(&[0u8; 165]);
    for _ in 0..7 {
        put_str(&mut v, b"");
    }
    v
}

#[test]
fn table_writes_csv_and_refuses_to_overwrite() {
    let w = world();
    let f = w.tmp.path().join("Monster.SData");
    write(&f, &encrypt(&monster_table(&[b"Wolf\0", b"=Bad,name\0"])));
    let csv = w.tmp.path().join("out/monsters.csv");
    write(&w.tmp.path().join("out/keep.txt"), b"x");
    let o = sh(&w.data, &["sdata", "table", p(&f), "--csv", p(&csv)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("2 rows"), "{}", err(&o));
    let bytes = std::fs::read(&csv).unwrap();
    assert!(bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "UTF-8 byte-order mark");
    let t = String::from_utf8(bytes).unwrap();
    let lines: Vec<&str> = t.trim_start_matches('\u{feff}').split("\r\n").collect();
    assert_eq!(lines[0].split(',').take(3).collect::<Vec<_>>(), ["id", "name", "model"]);
    assert!(lines[1].starts_with("0,Wolf,100,"), "{}", lines[1]);
    assert!(lines[2].starts_with("1,\"'=Bad,name\",101,"), "formula-escaped and quoted: {}", lines[2]);
    // an existing output is refused (exit 4) and left alone; --force replaces it
    std::fs::write(&csv, b"mine").unwrap();
    let o = sh(&w.data, &["sdata", "table", p(&f), "--csv", p(&csv)]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(std::fs::read(&csv).unwrap(), b"mine");
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&f), "--csv", p(&csv), "--force"])), 0);
    assert!(std::fs::read(&csv).unwrap().starts_with(&[0xEF, 0xBB, 0xBF]));
    // a plain file works the same
    let plain = w.tmp.path().join("Plain.SData");
    write(&plain, &monster_table(&[b"Wolf\0"]));
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&plain), "--layout", "monster", "--csv", p(&w.tmp.path().join("p.csv"))])), 0);
}

#[test]
fn table_json_shape_and_sections() {
    let w = world();
    let f = w.tmp.path().join("Monster.SData");
    write(&f, &encrypt(&monster_table(&[b"Wolf\0"])));
    let v = json(&sh(&w.data, &["sdata", "table", p(&f), "--json"]));
    assert_eq!((v["layout"].as_str(), v["table"].as_str()), (Some("monster-31"), Some("Monster")));
    assert_eq!(v["chosen_by"].as_str(), Some("fit"));
    let s = &v["sheets"][0];
    assert_eq!((s["name"].as_str(), s["row_count"].as_u64()), (Some("monsters"), Some(1)));
    assert_eq!(s["columns"][0], serde_json::json!({"name": "id", "kind": "int"}));
    assert_eq!((&s["rows"][0][0], &s["rows"][0][1], &s["rows"][0][2]), (&serde_json::json!(0), &serde_json::json!("Wolf"), &serde_json::json!(100)));
    // text output names the layout and the columns
    let o = sh(&w.data, &["sdata", "table", p(&f)]);
    assert!(out(&o).contains("layout monster-31 (Monster), names read as cp949") && out(&o).contains("monsters: 1 rows, "), "{}", out(&o));
    assert!(out(&o).contains("chosen because it reads the whole file") && out(&o).contains("only on the clients listed as verified"), "{}", out(&o));
    // several tables in one file
    let q = w.tmp.path().join("NpcQuest.SData");
    write(&q, &encrypt(&npcquest_table()));
    let v = json(&sh(&w.data, &["sdata", "table", p(&q), "--json"]));
    let names: Vec<&str> = v["sheets"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["merchants", "gatekeepers", "npcs", "quest_links", "quests"]);
    let v = json(&sh(&w.data, &["sdata", "table", p(&q), "--section", "merchants", "--json"]));
    assert_eq!(v["sheets"].as_array().unwrap().len(), 1);
    assert_eq!(v["sheets"][0]["row_count"], 1);
    let o = sh(&w.data, &["sdata", "table", p(&q), "--section", "nope"]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("merchants, gatekeepers"), "{}", err(&o));
    // the CSV of a many-table file is its main table, with a note
    let c = w.tmp.path().join("quests.csv");
    let o = sh(&w.data, &["sdata", "table", p(&q), "--csv", p(&c)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("this file has 5 tables; wrote quests"), "{}", err(&o));
}

#[test]
fn table_says_fit_only_or_loader_verified() {
    let w = world();
    // a 31-byte monster table: loader-verified layout
    let v = w.tmp.path().join("Monster.SData");
    write(&v, &encrypt(&monster_table(&[b"Wolf\0"])));
    let o = sh(&w.data, &["sdata", "table", p(&v)]);
    assert!(err(&o).contains("Loader-verified on: us-108, ru-ep3, ru-ep45, fr-ep54, phoenix-ep63."), "{}", err(&o));
    assert!(!err(&o).contains("FIT-ONLY: this layout reads"), "{}", err(&o));
    let j = json(&sh(&w.data, &["sdata", "table", p(&v), "--json"]));
    assert_eq!(j["status"].as_str(), Some("loader-verified"));
    assert_eq!(j["loader_verified_on"], serde_json::json!(["us-108", "ru-ep3", "ru-ep45", "fr-ep54", "phoenix-ep63"]));
    assert_eq!(j["fit_only_on"], serde_json::json!([]));
    // a 36-byte block: the FIT-ONLY layout; the sentence is on stderr, the JSON keys are additive
    let mut fo = Vec::new();
    put_u32(&mut fo, 1);
    put_str(&mut fo, b"Wolf\0");
    fo.extend_from_slice(&[0u8; 36]);
    let f = w.tmp.path().join("Monster36.SData");
    write(&f, &encrypt(&fo));
    let o = sh(&w.data, &["sdata", "table", p(&f)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("layout monster-36 (Monster)"), "{}", out(&o));
    assert!(err(&o).contains("FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly."), "{}", err(&o));
    assert!(!out(&o).contains("FIT-ONLY"), "the sentence is on stderr: {}", out(&o));
    let j = json(&sh(&w.data, &["sdata", "table", p(&f), "--json"]));
    assert_eq!((j["layout"].as_str(), j["chosen_by"].as_str(), j["status"].as_str()), (Some("monster-36"), Some("fit"), Some("fit-only")));
    assert_eq!((&j["loader_verified_on"], &j["fit_only_on"]), (&serde_json::json!([]), &serde_json::json!(["kr-ep8", "fawkes-v68"])));
    // the label is also printed when the table goes to a CSV file, and under -q
    let csv0 = w.tmp.path().join("m36-text.csv");
    let o = sh(&w.data, &["sdata", "table", p(&f), "--csv", p(&csv0)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("FIT-ONLY: this layout reads the whole file"), "{}", err(&o));
    let o = sh(&w.data, &["-q", "sdata", "table", p(&f)]);
    assert!(err(&o).contains("FIT-ONLY: this layout reads the whole file"), "{}", err(&o));
    let o = sh(&w.data, &["-q", "sdata", "table", p(&v)]);
    assert!(err(&o).contains("Loader-verified on: us-108"), "{}", err(&o));
    // the CSV summary document carries the same keys
    let csv = w.tmp.path().join("m36.csv");
    let j = json(&sh(&w.data, &["sdata", "table", p(&f), "--csv", p(&csv), "--json"]));
    assert_eq!((j["status"].as_str(), j["rows"].as_u64()), (Some("fit-only"), Some(1)));
    assert_eq!(j["fit_only_on"], serde_json::json!(["kr-ep8", "fawkes-v68"]));
}

#[test]
fn table_exit_codes() {
    let w = world();
    // a readable file no layout fits: exit 3, with the sentence the viewer uses
    let f = w.tmp.path().join("Other.SData");
    write(&f, &encrypt(b"just some bytes, not a table"));
    let o = sh(&w.data, &["sdata", "table", p(&f)]);
    assert_eq!(code(&o), 3);
    assert!(err(&o).contains("No table layout fits this file (28 bytes)"), "{}", err(&o));
    let v = json(&sh(&w.data, &["sdata", "table", p(&f), "--json"]));
    assert_eq!(v["error"]["kind"], "format");
    // a table, asked for as another table
    let m = w.tmp.path().join("Monster.SData");
    write(&m, &monster_table(&[b"Wolf\0"]));
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&m), "--layout", "skill"])), 3);
    // a damaged wrapper, a missing file
    let mut bad = encrypt(&monster_table(&[b"Wolf\0"]));
    bad[70] ^= 1;
    let b = w.tmp.path().join("Bad.SData");
    write(&b, &bad);
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&b)])), 3);
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&w.tmp.path().join("nothing.SData"))])), 3);
    // a name that is not a layout or a code page is a usage error
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&m), "--layout", "weapons"])), 2);
    assert_eq!(code(&sh(&w.data, &["sdata", "table", p(&m), "--code-page", "latin9"])), 2);
    // from an archive: the entry must exist; the fixture's Item.SData is not a table
    assert_eq!(code(&sh(&w.data, &["sdata", "table", "Item/Nothing.SData", "--from", p(&w.client)])), 3);
    assert_eq!(code(&sh(&w.data, &["sdata", "table", "Item/Item.SData", "--from", p(&w.client)])), 3);
}

#[test]
fn table_code_page() {
    let w = world();
    let f = w.tmp.path().join("Monster.SData");
    write(&f, &monster_table(&[&[0xB0, 0xCB, 0]]));
    let cp949 = json(&sh(&w.data, &["sdata", "table", p(&f), "--json"]));
    let cp1252 = json(&sh(&w.data, &["sdata", "table", p(&f), "--json", "--code-page", "cp1252"]));
    assert_ne!(cp949["sheets"][0]["rows"][0][1], cp1252["sheets"][0]["rows"][0][1]);
    assert_eq!(cp1252["sheets"][0]["rows"][0][1], "\u{B0}\u{CB}");
}
