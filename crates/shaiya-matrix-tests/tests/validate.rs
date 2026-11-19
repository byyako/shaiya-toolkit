//! File checks (`shaiya_inspect::validate`) over every SData, `.dds` and `.tga` of each matrix client. Opt-in and read-only:
//! `SHAIYA_TEST_CLIENTS` and `SHAIYA_CLIENT` (see docs/dev/testing.md); nothing is written. Prints the finding counts per client and
//! asserts that no original file is reported as SEED-encrypted twice (the one finding that makes the Toolkit ask).
//!
//! Run: `SHAIYA_TEST_CLIENTS=<root> SHAIYA_CLIENT=<us client> cargo test -p shaiya-matrix-tests --test validate -- --nocapture`

use shaiya_archive::Archive;
use shaiya_inspect::validate::{self, code};
use shaiya_matrix_tests::{present_clients, sdata_files};
use shaiya_testkit::{env_client, env_test_clients};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default)]
struct Tally {
    sdata: usize,
    textures: usize,
    unreadable: Vec<String>,
    other: Vec<(String, &'static str)>,
    counts: BTreeMap<&'static str, usize>,
}

impl Tally {
    fn add(&mut self, name: &str, bytes: &[u8]) {
        if validate::is_sdata_name(name) {
            self.sdata += 1;
        } else {
            self.textures += 1;
        }
        for f in validate::check_file(name, bytes, None) {
            *self.counts.entry(f.code).or_default() += 1;
            if f.code == code::TEXTURE_UNREADABLE {
                self.unreadable.push(name.to_string());
            } else {
                self.other.push((name.to_string(), f.code));
            }
        }
    }
}

fn find_ci(dir: &Path, name: &str) -> bool {
    std::fs::read_dir(dir).ok().is_some_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)))
}

fn tally(dir: &Path) -> Result<Tally, String> {
    let mut t = Tally::default();
    if find_ci(dir, "data.saf") {
        let a = Archive::open(dir).map_err(|e| e.to_string())?;
        for (path, e) in a.index().files() {
            if validate::wants_bytes(&path) {
                let bytes = a.read_entry(e).map_err(|e| format!("{path}: {e}"))?;
                t.add(&path, &bytes);
            }
        }
    } else {
        // an index-only client: the SData files extracted next to it, when there are any
        for (path, bytes) in sdata_files(dir)? {
            t.add(&path, &bytes);
        }
    }
    Ok(t)
}

#[test]
fn file_checks_over_every_sdata_and_texture_of_each_client() {
    let (us, root) = (env_client(), env_test_clients());
    if us.is_none() && root.is_none() {
        eprintln!("skipped: SHAIYA_CLIENT and SHAIYA_TEST_CLIENTS not set");
        return;
    }
    let mut failures = Vec::new();
    for (id, dir) in present_clients(us.as_deref(), root.as_deref()) {
        let t = match tally(&dir) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{id}: not checked ({e})");
                continue;
            }
        };
        let per_code: Vec<String> = code::ALL.iter().map(|c| format!("{c} {}", t.counts.get(c).copied().unwrap_or(0))).collect();
        eprintln!("{id}: {} SData, {} textures; {}", t.sdata, t.textures, per_code.join(", "));
        for (n, c) in t.other.iter().take(12) {
            eprintln!("  {c}: {n}");
        }
        for n in t.unreadable.iter().take(12) {
            eprintln!("  texture-unreadable: {n}");
        }
        if t.counts.get(code::SDATA_DOUBLE_ENCRYPTED).copied().unwrap_or(0) > 0 {
            failures.push(format!("{id}: an original SData is reported as double-encrypted"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
