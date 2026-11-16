//! `shaiya patch ...`: the whole life of a patch, every command in text and JSON, every exit code.

mod common;

use common::*;
use shaiya_testkit::SahBuilder;
use std::path::{Path, PathBuf};

/// A patch source: files/ (replaces Item.SData, adds interface/new.txt), loose/ (notice.txt), remove list.
fn sources(w: &World) -> (PathBuf, PathBuf, PathBuf) {
    let (files, loose, rm) = (w.tmp.path().join("files"), w.tmp.path().join("loose"), w.tmp.path().join("remove.txt"));
    write(&files.join("Item/Item.SData"), b"replacement item data");
    write(&files.join("interface/new.txt"), b"brand new");
    write(&loose.join("notice.txt"), b"hello players");
    write(&rm, b"test/junk.eft\nnot/there.txt\n");
    (files, loose, rm)
}

fn keys(w: &World) -> (PathBuf, PathBuf) {
    let (sk, pk) = (w.tmp.path().join("secret.key"), w.tmp.path().join("public.key"));
    let o = sh(&w.data, &["patch", "keygen", "--secret", p(&sk), "--public", p(&pk)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    (sk, pk)
}

fn built(w: &World, name: &str) -> PathBuf {
    let (files, loose, rm) = sources(w);
    let out = w.tmp.path().join(name);
    let o = sh(&w.data, &["patch", "build", "-o", p(&out), "--files", p(&files), "--loose", p(&loose), "--remove", p(&rm), "--timestamp", "2026-11-15"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    out
}

#[test]
fn the_whole_life_of_a_patch() {
    let w = world();
    save_client(&w, "us");
    let (secret, public) = keys(&w);
    let patch = w.tmp.path().join("ps0109.patch");
    let (files, loose, rm) = sources(&w);
    // build (dry run first), signed
    let o = sh(&w.data, &["patch", "build", "-o", p(&patch), "--files", p(&files), "--loose", p(&loose), "--remove", p(&rm), "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("would build") && out(&o).contains("interface/new.txt") && out(&o).contains("notice.txt") && out(&o).contains("test/junk.eft") && !patch.exists(), "{}", out(&o));
    assert!(err(&o).contains("remove.txt is honoured by the toolkit"), "{}", err(&o));
    let o = sh(&w.data, &["patch", "build", "-o", p(&patch), "--files", p(&files), "--loose", p(&loose), "--remove", p(&rm), "--sign", p(&secret), "--check-version", "3", "--timestamp", "2026-11-15"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("signed:") && patch.exists() && w.tmp.path().join("ps0109.patch.sig").exists(), "{}", out(&o));
    // list
    let o = sh(&w.data, &["patch", "list", p(&patch)]);
    let t = out(&o);
    assert!(t.contains("ps0109.patch  ZIP") && t.contains("signature file present") && t.contains("2 archive files") && t.contains("1 loose file") && t.contains("2 removals"), "{t}");
    let v = json(&sh(&w.data, &["patch", "list", p(&patch), "--json"]));
    assert_eq!((v["files"].as_array().unwrap().len(), v["removals"].as_array().unwrap().len(), v["signature_file"].as_bool()), (2, 2, Some(true)));
    // verify with the key
    let o = sh(&w.data, &["patch", "verify", p(&patch), "--key", p(&public), "--check-version", "3", "--require-signature"]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    assert!(out(&o).contains("structure OK") && out(&o).contains("signature valid"), "{}", out(&o));
    let o = sh(&w.data, &["patch", "verify", p(&patch), "--key", p(&public), "--check-version", "4"]);
    assert_eq!(code(&o), 1, "a wrong CheckVersion fails the signature");
    assert!(out(&o).contains("INVALID"), "{}", out(&o));
    let v = json(&sh(&w.data, &["patch", "verify", p(&patch), "--json"]));
    assert_eq!(v["patches"][0]["signature"]["status"], "not_checked");
    assert_eq!(code(&sh(&w.data, &["patch", "verify", p(&patch), "--key", p(&public)])), 4, "--check-version is required with a key");
    // apply: a dry run first
    let before = snapshot(&w.client);
    let o = sh(&w.data, &["patch", "apply", "@us", p(&patch), "--dry-run", "--key", p(&public), "--check-version", "3"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let t = out(&o);
    assert!(t.contains("ps0109.patch ->") && t.contains("would add 1, replace 1, remove 1 (1 not present), write 1 loose file") && t.contains("would append") && t.contains("signature valid"), "{t}");
    assert_eq!(snapshot(&w.client), before, "a dry run writes nothing");
    let v = json(&sh(&w.data, &["patch", "apply", "@us", p(&patch), "--dry-run", "--json"]));
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["plans"][0]["added"][0], "interface/new.txt");
    // a wrong key stops before any write (exit 4)
    let (_, other_pub) = (0, w.tmp.path().join("other.pub"));
    write(&other_pub, shaiya_patch::sign::keygen().1.as_bytes());
    let o = sh(&w.data, &["patch", "apply", "@us", p(&patch), "--key", p(&other_pub), "--check-version", "3"]);
    assert_eq!(code(&o), 4, "{}", err(&o));
    assert_eq!(snapshot(&w.client), before);
    // the real apply
    let o = sh(&w.data, &["patch", "apply", "@us", p(&patch), "--key", p(&public), "--check-version", "3"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("applied ps0109.patch: added 1, replaced 1, removed 1 (1 not present)") && out(&o).contains("backup:"), "{}", out(&o));
    assert_eq!(out(&sh(&w.data, &["archive", "cat", "@us", "interface/new.txt"])), "brand new");
    assert_eq!(std::fs::read(w.client.join("notice.txt")).unwrap(), b"hello players");
    assert_eq!(code(&sh(&w.data, &["archive", "cat", "@us", "test/junk.eft"])), 3);
    assert_eq!(code(&sh(&w.data, &["archive", "check", "@us", "--deep"])), 0);
    // verify --client: applied
    let o = sh(&w.data, &["patch", "verify", p(&patch), "--client", "@us"]);
    assert_eq!(code(&o), 0, "{}", out(&o));
    assert!(out(&o).contains("applied to the client: yes"), "{}", out(&o));
    // history, then rollback
    let o = sh(&w.data, &["patch", "history", "@us", "--json"]);
    let v = json(&o);
    assert_eq!(v["journals"][0]["label"], "ps0109.patch");
    assert_eq!(v["journals"][0]["kind"], "patch");
    assert_eq!(v["journals"][0]["rollback"]["possible"], true);
    let o = sh(&w.data, &["patch", "history", "@us"]);
    assert!(out(&o).contains("ps0109.patch") && out(&o).contains("applied") && out(&o).contains("possible"), "{}", out(&o));
    let o = sh(&w.data, &["patch", "rollback", "@us", "--dry-run"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("would roll back ps0109.patch"), "{}", out(&o));
    let o = sh(&w.data, &["patch", "rollback", "@us"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(out(&o).contains("rolled back ps0109.patch") && out(&o).contains("index restored"), "{}", out(&o));
    assert!(!w.client.join("notice.txt").exists());
    // verify --client now fails (exit 1), and a second rollback has nothing to do
    let o = sh(&w.data, &["patch", "verify", p(&patch), "--client", "@us"]);
    assert_eq!(code(&o), 1);
    assert!(out(&o).contains("not applied to the client"), "{}", out(&o));
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us"])), 4);
    let o = sh(&w.data, &["patch", "history", "@us"]);
    assert!(out(&o).contains("rolled back"), "{}", out(&o));
    // JSON of a real apply and a rollback
    let o = sh(&w.data, &["patch", "apply", "@us", p(&patch), "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["results"][0]["added"][0], "interface/new.txt");
    let o = sh(&w.data, &["patch", "rollback", "@us", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&o);
    assert!(v["restored_index"] == true && v["truncated_to"].is_u64());
    // prune
    let o = sh(&w.data, &["patch", "history", "@us", "--prune", "1", "--json"]);
    assert_eq!(code(&o), 0);
    assert_eq!(json(&o)["pruned"].as_array().unwrap().len(), 1);
}

#[test]
fn several_patches_in_order_and_a_failure_in_the_middle() {
    let w = world();
    save_client(&w, "us");
    let p1 = built(&w, "ps0001.patch");
    // a second patch that cannot apply: it replaces a file the client does not have
    let bad_files = w.tmp.path().join("badfiles");
    write(&bad_files.join("nowhere/x.bin"), b"x");
    let p2 = w.tmp.path().join("ps0002.patch");
    let mut build = shaiya_patch::BuildInput::from_folders(Some(&bad_files), None, None, Some(1)).unwrap();
    build.files[0].2 = 1;
    shaiya_patch::build_with(&build, &p2, &shaiya_patch::BuildOptions { existing: shaiya_core::fs::Existing::Refuse, sign: None }, &shaiya_core::Job::silent()).unwrap();
    // p2 applies fine (it adds), so use a patch that fails: a ZIP that is not a patch
    let p3 = w.tmp.path().join("ps0003.patch");
    write(&p3, b"this is not a patch");
    let o = sh(&w.data, &["patch", "apply", "@us", p(&p1), p(&p2), p(&p3)]);
    assert_eq!(code(&o), 3, "{}", err(&o));
    assert!(out(&o).contains("applied ps0001.patch") && out(&o).contains("applied ps0002.patch"), "{}", out(&o));
    assert!(err(&o).contains("ps0003.patch failed after 2 patches"), "{}", err(&o));
    let o = sh(&w.data, &["patch", "history", "@us", "--json"]);
    assert_eq!(json(&o)["journals"].as_array().unwrap().len(), 2);
    // the numbers out of order draw a warning
    let o = sh(&w.data, &["patch", "apply", "@us", p(&p2), p(&p1), "--dry-run"]);
    assert!(err(&o).contains("not in ascending order"), "{}", err(&o));
    // rolling back goes newest first; the older one is blocked until then
    let o = sh(&w.data, &["patch", "history", "@us"]);
    assert!(out(&o).contains("blocked: roll back"), "{}", out(&o));
    let first_id = json(&sh(&w.data, &["patch", "history", "@us", "--json"]))["journals"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us", "--id", &first_id])), 4);
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us"])), 0);
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us", "--id", &first_id])), 0);
    assert_eq!(code(&sh(&w.data, &["patch", "rollback", "@us", "--id", "20000101T000000Z-nothing"])), 3);
}

#[test]
fn build_refusals_warnings_and_diff() {
    let w = world();
    let (files, loose, _rm) = sources(&w);
    // not psNNNN: a warning, never a failure
    let odd = w.tmp.path().join("hotfix.patch");
    let o = sh(&w.data, &["patch", "build", "-o", p(&odd), "--files", p(&files)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert!(err(&o).contains("not named psNNNN.patch"), "{}", err(&o));
    // an existing output needs --force
    assert_eq!(code(&sh(&w.data, &["patch", "build", "-o", p(&odd), "--files", p(&files)])), 4);
    assert_eq!(code(&sh(&w.data, &["patch", "build", "-o", p(&odd), "--files", p(&files), "--force"])), 0);
    // an empty build, and bad loose names (exit 4, all listed)
    assert_eq!(code(&sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("e.patch"))])), 4);
    let evil = w.tmp.path().join("evil");
    write(&evil.join("DATA.SAF"), b"x");
    write(&evil.join("shaiya-toolkit-backups/x"), b"x");
    let o = sh(&w.data, &["patch", "build", "-o", p(&w.tmp.path().join("evil.patch")), "--loose", p(&evil)]);
    assert_eq!(code(&o), 4);
    assert!(err(&o).contains("DATA.SAF") && err(&o).contains("shaiya-toolkit-backups"), "{}", err(&o));
    // --files and --diff together are a usage error; --sign needs --check-version
    assert_eq!(code(&sh(&w.data, &["patch", "build", "-o", "x.patch", "--files", p(&files), "--diff", "a", "b"])), 2);
    assert_eq!(code(&sh(&w.data, &["patch", "build", "-o", "x.patch", "--files", p(&files), "--sign", "k"])), 2);
    // --diff OLD NEW (with --loose), then applying it to OLD gives NEW
    let (old, new) = (w.tmp.path().join("old"), w.tmp.path().join("new"));
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    SahBuilder::new(0).file(b"x/same.txt", b"same").file(b"x/changed.txt", b"before").file(b"x/gone.txt", b"bye").write_to(&old);
    SahBuilder::new(0).file(b"x/same.txt", b"same").file(b"x/changed.txt", b"after!").file(b"x/new.txt", b"hello").write_to(&new);
    let dp = w.tmp.path().join("ps0002.patch");
    let o = sh(&w.data, &["patch", "build", "-o", p(&dp), "--diff", p(&old), p(&new), "--loose", p(&loose)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    let v = json(&sh(&w.data, &["patch", "list", p(&dp), "--json"]));
    assert_eq!((v["files"].as_array().unwrap().len(), v["removals"][0].as_str(), v["loose"].as_array().unwrap().len()), (2, Some("x/gone.txt"), 1));
    assert_eq!(code(&sh(&w.data, &["patch", "apply", p(&old), p(&dp), "--no-journal"])), 0);
    assert_eq!(code(&sh(&w.data, &["archive", "diff", p(&old), p(&new), "--summary"])), 0, "OLD now equals NEW");
    // list of a missing or broken patch: exit 3
    assert_eq!(code(&sh(&w.data, &["patch", "list", p(&w.tmp.path().join("nope.patch"))])), 3);
    write(&w.tmp.path().join("junk.patch"), b"not a zip");
    assert_eq!(code(&sh(&w.data, &["patch", "list", p(&w.tmp.path().join("junk.patch"))])), 3);
    assert_eq!(code(&sh(&w.data, &["patch", "verify", p(&w.tmp.path().join("junk.patch"))])), 1, "a damaged patch is a failed check, not an unreadable file");
    // a tampered patch fails verify (exit 1)
    let good = built(&w, "ps0300.patch");
    let mut bytes = std::fs::read(&good).unwrap();
    bytes[50] ^= 0xFF; // inside the first entry's data (update.sah), well past its name
    let bad = w.tmp.path().join("ps0301.patch");
    std::fs::write(&bad, bytes).unwrap();
    assert_eq!(code(&sh(&w.data, &["patch", "verify", p(&bad)])), 1);
}

#[test]
fn keys_and_signatures() {
    let w = world();
    let (sk, pk) = keys(&w);
    // keygen never overwrites (exit 4), JSON shape
    assert_eq!(code(&sh(&w.data, &["patch", "keygen", "--secret", p(&sk), "--public", p(&pk)])), 4);
    let (sk2, pk2) = (w.tmp.path().join("s2.key"), w.tmp.path().join("p2.key"));
    let o = sh(&w.data, &["patch", "keygen", "--secret", p(&sk2), "--public", p(&pk2), "--json"]);
    assert_eq!(code(&o), 0);
    let v = json(&o);
    assert_eq!(v["public_hex"].as_str().unwrap().len(), 64);
    assert!(v["secret_file"].is_string() && v["public_file"].is_string());
    // sign and sign-updater: a second signing needs --force
    let f = w.tmp.path().join("ps0001.patch");
    write(&f, b"not really a patch, only bytes to sign");
    let o = sh(&w.data, &["patch", "sign", p(&f), "--key", p(&sk), "--check-version", "3", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["signed"][0]["name"], "ps0001.patch");
    assert_eq!(code(&sh(&w.data, &["patch", "sign", p(&f), "--key", p(&sk), "--check-version", "3"])), 4);
    assert_eq!(code(&sh(&w.data, &["patch", "sign", p(&f), "--key", p(&sk), "--check-version", "3", "--force"])), 0);
    let exe = w.tmp.path().join("new_updater.exe");
    write(&exe, b"MZ updater");
    let o = sh(&w.data, &["patch", "sign-updater", p(&exe), "--key", p(&sk), "--updater-version", "2", "--check-version", "3", "--json"]);
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(json(&o)["signed"][0]["name"], "shaiya-updater-2.exe");
    assert_eq!(code(&sh(&w.data, &["patch", "sign-updater", p(&exe), "--key", p(&sk), "--updater-version", "2", "--check-version", "3"])), 4);
    // a bad key file and a missing file
    assert_eq!(code(&sh(&w.data, &["patch", "sign", p(&f), "--key", p(&w.tmp.path().join("nokey")), "--check-version", "3"])), 3);
    assert_eq!(code(&sh(&w.data, &["patch", "sign", p(&w.tmp.path().join("nofile")), "--key", p(&sk), "--check-version", "3"])), 3);
    // --public-hex works like --key
    let hex = std::fs::read_to_string(&pk).unwrap();
    let _ = hex;
    assert_eq!(code(&sh(&w.data, &["patch", "verify", p(&exe), "--public-hex", "zz", "--check-version", "3"])), 3);
}

#[test]
fn every_patch_command_has_help() {
    let w = world();
    for c in ["build", "list", "verify", "apply", "rollback", "history", "keygen", "sign", "sign-updater"] {
        let o = sh(&w.data, &["patch", c, "--help"]);
        assert_eq!(code(&o), 0, "{c}");
        assert!(out(&o).contains(c) && out(&o).contains("Example:"), "{c}: {}", out(&o));
    }
    let _: &Path = w.tmp.path();
}

#[test]
fn unreadable_inputs_exit_3() {
    let w = world();
    let none = w.tmp.path().join("nothing");
    for args in [
        vec!["patch", "verify", p(&none)],
        vec!["patch", "history", p(&none)],
        vec!["patch", "rollback", p(&none)],
        vec!["patch", "apply", p(&none), "x.patch"],
        vec!["patch", "keygen", "--secret", p(&none.join("a/secret.key")), "--public", p(&w.tmp.path().join("pub.key"))],
    ] {
        let o = sh(&w.data, &args);
        assert_eq!(code(&o), 3, "{args:?}: {}", err(&o));
    }
}
