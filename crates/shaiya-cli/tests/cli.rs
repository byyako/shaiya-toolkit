//! CLI tests: run the real `shaiya` binary on synthetic files (built by `shaiya-testkit`, `shaiya-seed`, `shaiya-patch`).

use shaiya_testkit::{pe_stub, SahBuilder, TempDir};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn shaiya(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_shaiya")).args(args).output().expect("run shaiya")
}

fn run(args: &[&str]) -> Output {
    let os: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    shaiya(&os)
}

fn with_paths(args: &[&str], paths: &[&Path]) -> Output {
    let mut os: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    os.extend(paths.iter().map(|p| p.as_os_str()));
    shaiya(&os)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

struct Samples {
    _dir: TempDir,
    sah: PathBuf,
    saf: PathBuf,
    seed: PathBuf,
    plain: PathBuf,
    patch: PathBuf,
    exe: PathBuf,
    unknown: PathBuf,
    missing: PathBuf,
}

fn samples() -> Samples {
    let dir = TempDir::new("cli");
    let d = dir.path();
    let (sah, saf) = SahBuilder::new(0).file(b"Item/Item.SData", b"item").file(b"a/b.txt", b"hello").write_to(d);
    let seed = d.join("Item.SData");
    std::fs::write(&seed, shaiya_seed::encrypt(b"0123456789")).unwrap();
    let plain = d.join("Plain.sdata");
    std::fs::write(&plain, b"plain text").unwrap();
    let files = d.join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("x.txt"), b"x").unwrap();
    let patch = d.join("ps0001.patch");
    shaiya_patch::build(Some(&files), None, None, &patch).unwrap();
    let exe = d.join("game.exe");
    std::fs::write(&exe, pe_stub(0x014C, 1_272_251_564)).unwrap();
    let unknown = d.join("notes.bin");
    std::fs::write(&unknown, [1u8, 2, 3, 4]).unwrap();
    let missing = d.join("nothing-here.bin");
    Samples { _dir: dir, sah, saf, seed, plain, patch, exe, unknown, missing }
}

#[test]
fn info_text_for_every_kind() {
    let s = samples();
    let o = with_paths(&["info"], &[&s.sah, &s.saf, &s.seed, &s.plain, &s.patch, &s.exe, &s.unknown]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert!(out.contains("data.sah"), "{out}");
    assert!(out.contains("SAH index, version 0, 2 files in 2 folders, 0 free entries, data ends at 9"), "{out}");
    assert!(out.contains("SAF data"), "{out}");
    assert!(out.contains("SData, SEED-encrypted, plaintext 10 bytes, checksum OK"), "{out}");
    assert!(out.contains("SData, plain (not encrypted)"), "{out}");
    assert!(out.contains("Patch, 1 archive file, 0 loose files, 0 removals"), "{out}");
    assert!(out.contains("Windows program (x86), linked 2010-04-26 03:12:44 UTC"), "{out}");
    assert!(out.contains("Unknown file type, starts with 01020304"), "{out}");
}

#[test]
fn info_names_the_three_list_kinds_with_counts() {
    use shaiya_testkit::lists::{itm_bytes, mlt_bytes, mon_bytes, ItmRec, MonRec};
    let d = TempDir::new("cli-info-lists");
    let (mlt, mon, itm) = (d.path().join("a.mlt"), d.path().join("a.mon"), d.path().join("a.itm"));
    std::fs::write(&mlt, mlt_bytes(&["a.3DC"], &["x.DDS", "y.DDS"], &[(0, 1, 1)])).unwrap();
    let rec = MonRec { name: "Wolf", flag: 0, slots: [""; 17], parts: &[("w.3DC", "w.DDS")], size: 1.0, attachments: &[] };
    std::fs::write(&mon, mon_bytes(&[rec])).unwrap();
    std::fs::write(&itm, itm_bytes(&["s.3DO"], &["s.DDS"], &[ItmRec { words: [0; 6], extra: None }, ItmRec { words: [0, 0, 0, 0, 1, 0], extra: Some([0; 4]) }])).unwrap();
    let o = with_paths(&["info"], &[&mlt, &mon, &itm]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert!(out.contains("MLT list, 1 record, 1 mesh name, 2 texture names"), "{out}");
    assert!(out.contains("MON list, 1 record, 1 part"), "{out}");
    assert!(out.contains("ITM list, 2 records, 1 mesh name, 1 texture name"), "{out}");
    let o = with_paths(&["info", "--json"], &[&mon]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((v["files"][0]["kind"].as_str(), v["files"][0]["details"]["parts"].as_u64()), (Some("mon"), Some(1)));
}

#[test]
fn info_json_parses_and_hash_is_correct() {
    let s = samples();
    let o = with_paths(&["info", "--json", "--hash"], &[&s.seed, &s.exe]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("stdout is one JSON document");
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["kind"], "sdata_seed");
    assert_eq!(files[0]["details"]["crc_ok"], true);
    assert_eq!(files[0]["details"]["plain_size"], 10);
    assert_eq!(files[1]["kind"], "pe");
    assert_eq!(files[1]["details"]["timestamp_utc"], "2010-04-26T03:12:44Z");
    let want = reference_sha256(&std::fs::read(&s.exe).unwrap());
    assert_eq!(files[1]["sha256"], want.as_str());
    assert!(v.get("errors").is_none());
}

/// An independent SHA-256 so the CLI test does not rely on the code under test.
fn reference_sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[test]
fn hash_line_in_text_mode() {
    let s = samples();
    let o = with_paths(&["info", "--hash"], &[&s.plain]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains(&format!("sha256 {}", reference_sha256(b"plain text"))), "{}", text(&o.stdout));
}

#[test]
fn unreadable_and_damaged_inputs_exit_3_but_the_rest_are_reported() {
    let s = samples();
    let o = with_paths(&["info"], &[&s.exe, &s.missing]);
    assert_eq!(o.status.code(), Some(3));
    assert!(text(&o.stdout).contains("Windows program"), "the readable file is still reported");
    assert!(text(&o.stderr).contains("error:"), "{}", text(&o.stderr));
    // JSON: the result object keeps the good file and lists the error
    let o = with_paths(&["info", "--json"], &[&s.exe, &s.missing]);
    assert_eq!(o.status.code(), Some(3));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["files"].as_array().unwrap().len(), 1);
    assert_eq!(v["errors"][0]["kind"], "io");
    // a damaged SEED file is readable but has a problem: exit 3
    let bad = s.seed.with_file_name("Bad.SData");
    let mut bytes = std::fs::read(&s.seed).unwrap();
    bytes[70] ^= 1;
    std::fs::write(&bad, bytes).unwrap();
    let o = with_paths(&["info"], &[&bad]);
    assert_eq!(o.status.code(), Some(3));
    assert!(text(&o.stdout).contains("checksum FAILED"), "{}", text(&o.stdout));
    assert!(text(&o.stdout).contains("problem:"), "{}", text(&o.stdout));
}

#[test]
fn version_forms() {
    let o = run(&["version"]);
    assert_eq!(o.status.code(), Some(0));
    let line = text(&o.stdout);
    assert!(line.starts_with("shaiya 0.5.0 (git "), "{line}");
    assert!(line.trim_end().ends_with(')'), "{line}");
    assert_eq!(text(&run(&["-V"]).stdout), line);
    let o = run(&["version", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["version"], "0.5.0");
    assert!(v["git"].as_str().is_some_and(|g| !g.is_empty()));
    assert!(v["target"].as_str().is_some_and(|t| t.contains('-')));
    // global flags work before and after the command
    let o = run(&["--json", "version"]);
    assert!(serde_json::from_slice::<serde_json::Value>(&o.stdout).is_ok());
}

#[test]
fn usage_errors_exit_2() {
    let o = run(&["no-such-command"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stdout).is_empty());
    assert_eq!(run(&["info"]).status.code(), Some(2), "info needs a path");
    assert_eq!(run(&["info", "--no-such-flag", "x"]).status.code(), Some(2));
    assert_eq!(run(&[]).status.code(), Some(2), "no arguments shows help as a usage error");
    // with --json the error is also a JSON document on stdout
    let o = run(&["--json", "no-such-command"]);
    assert_eq!(o.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["kind"], "usage");
    // help is success
    let o = run(&["--help"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains("info"));
    let help = text(&o.stdout);
    assert!(help.contains("svmap"), "{help}");
    for sub in ["check", "dump", "build", "roundtrip", "diff", "wld-check", "spawns", "bulk"] {
        let o = run(&["svmap", sub, "--help"]);
        assert_eq!(o.status.code(), Some(0), "svmap {sub} --help");
    }
    assert!(help.contains("usually be rolled back") && help.contains("archive compact --in-place"), "{help}");
    assert!(!help.contains("can be undone"), "{help}");
}

#[test]
fn info_on_data_saf_beside_its_sah_says_saf_even_when_it_starts_like_an_sdata() {
    let d = TempDir::new("cli-info-saf-sdata");
    let mut first = shaiya_seed::SIGNATURE.to_vec();
    first.resize(300, 1);
    let (_sah, saf) = SahBuilder::new(1).file(b"Cash.SData", &first).file(b"a.txt", b"x").write_to(d.path());
    let o = with_paths(&["info"], &[&saf]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let t = text(&o.stdout);
    assert!(t.contains("SAF data") && t.contains("2 files") && !t.contains("FAILED"), "{t}");
}

#[test]
fn quiet_and_progress_flags_are_accepted() {
    let s = samples();
    let o = with_paths(&["-q", "--progress", "never", "info", "--hash"], &[&s.plain]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stderr).is_empty(), "{}", text(&o.stderr));
    let o = with_paths(&["-vv", "--progress", "always", "info"], &[&s.plain]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stderr).contains("read in"), "{}", text(&o.stderr));
}
