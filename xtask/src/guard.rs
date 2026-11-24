//! `cargo xtask guard`: no game files in the repository. Fails when a tracked file is over 1 MiB (except `Cargo.lock`), has a game-file
//! extension, starts with `SAH`, the SEED signature or `MZ` (outside `xtask/`), lives in a `fixtures` folder without a README entry,
//! or is a `.json` file that starts like a model exported to JSON (a `"format": "shaiya-toolkit-<ext>-v<n>"` key: the exported JSON of a game file must not be committed).

use crate::meta::root;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const MAX_SIZE: u64 = 1 << 20;
const FIXTURE_MAX: u64 = 64 << 10;
const BAD_EXTENSIONS: &[&str] = &["sah", "saf", "sdata", "exe", "dll", "patch", "pdb", "mdf", "bak", "ani", "3dc", "3do", "3de", "vani", "mani", "smod", "mlt", "mon", "itm"];
const SEED_SIGNATURE: &[u8] = b"0001CBCEBC5B2784D3FC9A2A9DB84D1C3FEB6E99";

const MODEL_EXTENSIONS: [&str; 6] = ["ani", "3dc", "vani", "mani", "3do", "3de"];

/// True when the head of a file is the start of a model exported to JSON: a `"format"` key (whitespace allowed before and after the
/// colon) whose value is `"shaiya-toolkit-<ext>-v<digit>..."`.
fn looks_like_model_json(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    text.match_indices("\"format\"").any(|(at, key)| {
        let rest = text[at + key.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else { return false };
        let Some(rest) = rest.trim_start().strip_prefix("\"shaiya-toolkit-") else { return false };
        MODEL_EXTENSIONS.iter().any(|e| rest.strip_prefix(e).and_then(|r| r.strip_prefix("-v")).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())))
    })
}

/// Problems with one file, from what is known about it. `head` is its first bytes.
pub fn check_file(path: &str, size: u64, head: &[u8], readme_lists: &dyn Fn(&str, &str) -> Option<bool>) -> Vec<String> {
    let mut out = Vec::new();
    let lower = path.to_ascii_lowercase();
    if size > MAX_SIZE && path != "Cargo.lock" {
        out.push(format!("{path}: {size} bytes is over the 1 MiB limit"));
    }
    if let Some(ext) = Path::new(&lower).extension().and_then(|e| e.to_str()) {
        if BAD_EXTENSIONS.contains(&ext) {
            out.push(format!("{path}: the extension .{ext} is a game-file or binary type"));
        }
    }
    if lower.ends_with(".json") && looks_like_model_json(head) {
        out.push(format!("{path}: looks like a model exported to JSON: game data must not be committed"));
    }
    if !path.starts_with("xtask/") {
        if head.starts_with(b"SAH") {
            out.push(format!("{path}: starts with SAH (a data.sah index)"));
        }
        if head.starts_with(SEED_SIGNATURE) {
            out.push(format!("{path}: starts with the SEED signature (an encrypted SData file)"));
        }
        if head.starts_with(b"MZ") {
            out.push(format!("{path}: starts with MZ (a Windows program)"));
        }
    }
    if let Some((dir, name)) = lower.rsplit_once('/').filter(|(d, _)| d.split('/').any(|p| p == "fixtures")) {
        let orig_name = &path[path.len() - name.len()..];
        if !name.eq_ignore_ascii_case("readme.md") {
            if size > FIXTURE_MAX {
                out.push(format!("{path}: a fixture may be at most 64 KiB"));
            }
            match readme_lists(&path[..dir.len()], orig_name) {
                Some(true) => {}
                Some(false) => out.push(format!("{path}: not listed in the README.md of its fixtures folder")),
                None => out.push(format!("{path}: its fixtures folder has no README.md listing its origin")),
            }
        }
    }
    out
}

pub fn run() -> Result<(), String> {
    let root = root();
    let out = Command::new("git")
        .args(["-c", "safe.directory=*", "ls-files", "-z"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let files: BTreeSet<String> = String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let readme_lists = |dir: &str, name: &str| -> Option<bool> {
        let text = std::fs::read_to_string(root.join(dir).join("README.md")).ok()?;
        Some(text.contains(name))
    };
    let mut problems = Vec::new();
    for f in &files {
        let p = root.join(f);
        let Ok(meta) = std::fs::metadata(&p) else { continue }; // a tracked file that is deleted in the working tree
        let mut head = vec![0u8; 256];
        let n = std::fs::File::open(&p).and_then(|mut h| std::io::Read::read(&mut h, &mut head)).unwrap_or(0);
        head.truncate(n);
        problems.extend(check_file(f, meta.len(), &head, &readme_lists));
    }
    if problems.is_empty() {
        println!("guard: {} tracked files checked, no game files", files.len());
        Ok(())
    } else {
        Err(format!("guard failed:\n  {}", problems.join("\n  ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str, _: &str) -> Option<bool> {
        None
    }

    #[test]
    fn clean_files_pass() {
        assert!(check_file("crates/x/src/lib.rs", 5000, b"//! hello", &none).is_empty());
        assert!(check_file("Cargo.lock", 3_000_000, b"# This file", &none).is_empty());
        assert!(check_file("assets/icons/shaiya.png", 3000, b"\x89PNG", &none).is_empty());
    }

    #[test]
    fn game_files_are_caught_by_size_extension_and_content() {
        assert_eq!(check_file("big.txt", 2 << 20, b"x", &none).len(), 1);
        for name in ["data.sah", "DATA.SAF", "Item.SData", "game.EXE", "a.dll", "ps0001.patch", "x.pdb", "x.mdf", "data.sah.bak"] {
            assert!(!check_file(name, 10, b"x", &none).is_empty(), "{name}");
        }
        assert_eq!(check_file("a/innocent.txt", 10, b"SAH\0\0\0", &none).len(), 1);
        assert_eq!(check_file("a/innocent.txt", 10, SEED_SIGNATURE, &none).len(), 1);
        assert_eq!(check_file("a/innocent.txt", 10, b"MZ\x90\0", &none).len(), 1);
        // the xtask sources may contain these markers
        assert!(check_file("xtask/src/guard.rs", 10, b"MZ", &none).is_empty());
    }

    #[test]
    fn animation_and_model_extensions_are_refused() {
        for ext in ["ani", "3dc", "3do", "3de", "vani", "mani", "smod"] {
            for name in [format!("a/b.{ext}"), format!("a/B.{}", ext.to_ascii_uppercase()), format!("x.{ext}")] {
                assert_eq!(check_file(&name, 10, b"x", &none).len(), 1, "{name}");
            }
        }
        // a neighbouring name that only contains the text is not refused
        assert!(check_file("docs/formats/ani.md", 10, b"x", &none).is_empty());
    }

    #[test]
    fn list_files_are_refused_by_the_guard() {
        for ext in ["mlt", "mon", "itm"] {
            for name in [format!("a/b.{ext}"), format!("a/B.{}", ext.to_ascii_uppercase()), format!("x.{ext}")] {
                assert_eq!(check_file(&name, 10, b"x", &none).len(), 1, "{name}");
            }
        }
        // the format pages have a different name and stay allowed
        assert!(check_file("docs/formats/mlt.md", 10, b"x", &none).is_empty());
    }

    #[test]
    fn exported_model_json_is_refused() {
        for ext in ["ani", "3dc", "vani", "mani", "3do", "3de"] {
            let head = format!("{{
  \"format\": \"shaiya-toolkit-{ext}-v1\",
  \"source\": \"a\"");
            assert_eq!(check_file("notes/x.json", 100, head.as_bytes(), &none).len(), 1, "{ext}");
            assert_eq!(check_file("notes/X.JSON", 100, head.as_bytes(), &none).len(), 1, "{ext}");
            // only a .json file is looked at: a doc or a test may quote the text
            assert!(check_file("docs/x.md", 100, head.as_bytes(), &none).is_empty(), "{ext}");
        }
        // layout files of the sdata crate and other JSON pass
        assert!(check_file("crates/shaiya-sdata/layouts/item-134.json", 100, b"{
  \"name\": \"item\",
  \"size\": 134", &none).is_empty());
        assert!(check_file("a.json", 100, b"{\"format\": \"other\"}", &none).is_empty());
        assert!(check_file("a.json", 100, b"{\"format\": \"shaiya-toolkit-future-v1\"}", &none).is_empty());
        // whitespace around the colon does not get past it; a name that only starts like a format does not trip it
        assert_eq!(check_file("a.json", 100, b"{\"format\" : \"shaiya-toolkit-ani-v1\"}", &none).len(), 1);
        assert_eq!(check_file("a.json", 100, b"{\"format\"
:
\"shaiya-toolkit-3do-v12\"}", &none).len(), 1);
        assert!(check_file("a.json", 100, b"{\"format\": \"shaiya-toolkit-ani-viewer\"}", &none).is_empty());
        assert!(check_file("a.json", 100, b"{\"other\": \"shaiya-toolkit-ani-v1\"}", &none).is_empty());
    }

    #[test]
    fn fixtures_need_a_listing() {
        let listed = |_: &str, name: &str| Some(name == "ok.bin");
        assert!(check_file("crates/c/tests/fixtures/ok.bin", 100, b"x", &listed).is_empty());
        assert_eq!(check_file("crates/c/tests/fixtures/other.bin", 100, b"x", &listed).len(), 1);
        assert_eq!(check_file("crates/c/tests/fixtures/ok.bin", 100_000, b"x", &listed).len(), 1, "over 64 KiB");
        assert_eq!(check_file("crates/c/tests/fixtures/a.bin", 100, b"x", &none).len(), 1, "no README");
        assert!(check_file("crates/c/tests/fixtures/README.md", 100, b"#", &none).is_empty());
    }
}
