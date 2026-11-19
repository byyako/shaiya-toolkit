//! `cargo xtask sizes [--platform P] [--target T]`: release binary sizes against the budgets in docs/dev/releasing.md; a non-zero exit
//! when one is over.

use crate::meta::root;
use crate::tools::TOOLS;
use std::path::PathBuf;

pub const CLI_BUDGET: u64 = 6_000_000;
pub const GUI_BUDGET: u64 = 20_000_000;
pub const ZIP_BUDGET_ALL_IN_ONE: u64 = 17_000_000;
pub const ZIP_BUDGET_CLI: u64 = 3_000_000;

/// The zip budget (Windows) for a release asset name: `shaiya` is the command line zip, anything else the all-in-one.
pub fn zip_budget(asset: &str) -> u64 {
    if asset == "shaiya" {
        ZIP_BUDGET_CLI
    } else {
        ZIP_BUDGET_ALL_IN_ONE
    }
}

/// True when `size` does not exceed `limit`.
pub fn within(size: u64, limit: u64) -> bool {
    size <= limit
}

pub fn budget(gui: bool) -> u64 {
    if gui {
        GUI_BUDGET
    } else {
        CLI_BUDGET
    }
}

/// The cargo target directory: `CARGO_TARGET_DIR` when it is set and not empty (a relative value counts from the repository root), else
/// `<repo>/target`.
pub fn target_dir() -> PathBuf {
    target_dir_from(std::env::var_os("CARGO_TARGET_DIR"))
}

fn target_dir_from(env: Option<std::ffi::OsString>) -> PathBuf {
    match env {
        Some(v) if !v.is_empty() => root().join(v),
        _ => root().join("target"),
    }
}

/// Where a release binary is: `<target dir>/release` or `<target dir>/<triple>/release`.
pub fn binary_path(bin: &str, windows: bool, target: Option<&str>) -> PathBuf {
    binary_path_in(target_dir(), bin, windows, target)
}

fn binary_path_in(base: PathBuf, bin: &str, windows: bool, target: Option<&str>) -> PathBuf {
    let mut p = base;
    if let Some(t) = target {
        p.push(t);
    }
    p.push("release");
    p.push(if windows { format!("{bin}.exe") } else { bin.to_string() });
    p
}

pub fn run(platform: Option<&str>, target: Option<&str>) -> Result<(), String> {
    let windows = match platform {
        Some(p) => p.starts_with("windows"),
        None => cfg!(windows),
    };
    let mut over = Vec::new();
    for t in TOOLS {
        let p = binary_path(t.bin, windows, target);
        let size = std::fs::metadata(&p).map_err(|e| format!("{}: {e} (build with cargo build --release first)", p.display()))?.len();
        let limit = budget(t.gui);
        let mark = if size > limit { "OVER" } else { "ok" };
        println!("{:<16} {:>12} bytes  ({:.2} MB)  budget {:>2} MB  {mark}", t.bin, size, size as f64 / 1e6, limit / 1_000_000);
        if size > limit {
            over.push(format!("{} is {size} bytes, over the {limit} budget", t.bin));
        }
    }
    if over.is_empty() {
        Ok(())
    } else {
        Err(over.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zip_budget_check() {
        assert_eq!(zip_budget("shaiya"), ZIP_BUDGET_CLI);
        assert_eq!(zip_budget("shaiya-toolkit"), ZIP_BUDGET_ALL_IN_ONE);
        assert!(within(1000, 1000) && !within(1001, 1000) && within(0, 5));
    }

    #[test]
    fn budgets_and_paths() {
        assert_eq!(budget(false), 6_000_000);
        assert_eq!(budget(true), 20_000_000);
        assert!(binary_path_in(target_dir_from(None), "shaiya", true, None).ends_with("target/release/shaiya.exe") || binary_path_in(target_dir_from(None), "shaiya", true, None).ends_with("target\\release\\shaiya.exe"));
        let p = binary_path_in(target_dir_from(None), "shaiya", false, Some("x86_64-unknown-linux-gnu")).display().to_string().replace('\\', "/");
        assert!(p.ends_with("target/x86_64-unknown-linux-gnu/release/shaiya"), "{p}");
    }

    #[test]
    fn cargo_target_dir_is_honoured() {
        use std::ffi::OsString;
        assert_eq!(target_dir_from(None), root().join("target"));
        assert_eq!(target_dir_from(Some(OsString::new())), root().join("target"));
        let abs = std::env::temp_dir().join("elsewhere");
        assert_eq!(target_dir_from(Some(abs.clone().into_os_string())), abs);
        assert_eq!(target_dir_from(Some("rel-target".into())), root().join("rel-target"));
        let p = binary_path_in(abs.clone(), "shaiya", true, None);
        assert_eq!(p, abs.join("release").join("shaiya.exe"));
    }
}
