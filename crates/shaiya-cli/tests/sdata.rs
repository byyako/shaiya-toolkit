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
