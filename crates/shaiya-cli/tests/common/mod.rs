//! Shared helpers of the CLI tests: run the real `shaiya` binary, build a small client folder, snapshot a folder.

#![allow(dead_code)]

use shaiya_testkit::{pe_stub, SahBuilder, TempDir};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn sh(data_dir: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_shaiya"));
    c.arg("--data-dir").arg(data_dir).args(args);
    c.output().expect("run shaiya")
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn out(o: &Output) -> String {
    text(&o.stdout)
}

pub fn err(o: &Output) -> String {
    text(&o.stderr)
}

pub fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

pub fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", out(o)))
}

pub fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A temp folder with a data dir and a small client folder.
pub struct World {
    pub tmp: TempDir,
    pub data: PathBuf,
    pub client: PathBuf,
}

pub const ITEM_PLAIN: &[u8] = b"item plaintext data of some length";

/// `client/` holds data.sah/data.saf (7 files), Version.ini and game.exe.
pub fn world() -> World {
    let tmp = TempDir::new("cli-t1");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let client = tmp.path().join("client");
    std::fs::create_dir_all(&client).unwrap();
    fill_client(&client);
    World { tmp, data, client }
}

pub fn fill_client(dir: &Path) {
    SahBuilder::new(0)
        .file(b"Item/Item.SData", &shaiya_seed::encrypt(ITEM_PLAIN))
        .file(b"Monster/Monster.SData", &shaiya_seed::encrypt(b"monster plaintext"))
        .file(b"Cn/Plain.SData", b"plain sdata of a client that does not encrypt")
        .file(b"interface/a.dds", b"DDS fake bytes")
        .file(b"interface/icon/b.txt", b"hello")
        .file(b"world/map.dg", &[7u8; 200])
        .file(b"test/junk.eft", b"junk")
        .write_to(dir);
    std::fs::write(dir.join("Version.ini"), "[Version]\r\nCheckVersion=3\r\nCurrentVersion=108\r\n").unwrap();
    std::fs::write(dir.join("game.exe"), pe_stub(0x014C, 1_272_251_564)).unwrap();
}

/// Every file under `dir` with its bytes, for "nothing changed" checks.
pub fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    fn walk(base: &Path, dir: &Path, v: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, v);
            } else {
                v.push((p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(dir, dir, &mut v);
    v.sort();
    v
}

pub fn write(path: &Path, bytes: &[u8]) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

/// Saves `client` under `name` in the world's data folder.
pub fn save_client(w: &World, name: &str) {
    let o = sh(&w.data, &["clients", "add", name, p(&w.client)]);
    assert_eq!(code(&o), 0, "{}", err(&o));
}
