//! Build-script helper for the toolkit programs. Used from each binary crate's `build.rs`:
//!
//! ```ignore
//! fn main() {
//!     shaiya_build::build_info();
//!     shaiya_build::windows_resources(&shaiya_build::WinRes {
//!         file_description: "Shaiya command line",
//!         original_filename: "shaiya.exe",
//!         icon: Some("../../assets/icons/shaiya.ico"),
//!         gui: false,
//!     });
//! }
//! ```

use std::path::{Path, PathBuf};

pub struct WinRes<'a> {
    pub file_description: &'a str,
    pub original_filename: &'a str,
    /// Path of an `.ico` from the crate directory.
    pub icon: Option<&'a str>,
    pub gui: bool,
}

/// Four numbers `a,b,c,d` for the VERSIONINFO from a Cargo version such as `0.1.0` or `1.2.3-rc.1`.
pub fn version_numbers(version: &str) -> [u16; 4] {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let mut out = [0u16; 4];
    for (i, part) in core.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

/// Escapes a path for an .rc string literal (forward slashes need no escaping).
fn rc_path(p: &Path) -> String {
    p.display().to_string().replace('\\', "/").replace('"', "")
}

fn rc_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The text of the `.rc` file for these resources.
pub fn rc_source(res: &WinRes, version: &str, icon: Option<&Path>, manifest: &Path) -> String {
    let [a, b, c, d] = version_numbers(version);
    let mut s = String::new();
    if let Some(icon) = icon {
        s.push_str(&format!("1 ICON \"{}\"\n", rc_path(icon)));
    }
    s.push_str(&format!("1 24 \"{}\"\n", rc_path(manifest)));
    s.push_str(&format!(
        "1 VERSIONINFO\nFILEVERSION {a},{b},{c},{d}\nPRODUCTVERSION {a},{b},{c},{d}\nFILEOS 0x4\nFILETYPE 0x1\n{{\nBLOCK \"StringFileInfo\"\n{{\nBLOCK \"040904B0\"\n{{\n"
    ));
    for (k, v) in [
        ("CompanyName", "byyako"),
        ("FileDescription", res.file_description),
        ("FileVersion", version),
        ("InternalName", res.original_filename),
        ("LegalCopyright", "Copyright (c) 2026 byyako. MIT License."),
        ("OriginalFilename", res.original_filename),
        ("ProductName", "Shaiya Toolkit"),
        ("ProductVersion", version),
    ] {
        s.push_str(&format!("VALUE \"{k}\", \"{}\"\n", rc_text(v)));
    }
    s.push_str("}\n}\nBLOCK \"VarFileInfo\"\n{\nVALUE \"Translation\", 0x0409, 0x04B0\n}\n}\n");
    s
}

/// No-op unless `CARGO_CFG_TARGET_OS` is `windows`. Writes `OUT_DIR/res.rc` with VERSIONINFO (version from `CARGO_PKG_VERSION`,
/// ProductName "Shaiya Toolkit", LegalCopyright "Copyright (c) 2026 byyako. MIT License."), the icon and `assets/windows/app.manifest`,
/// then `embed_resource::compile`. Works with MSVC (`rc.exe`) and gnullvm (`x86_64-w64-mingw32-windres` from llvm-mingw).
pub fn windows_resources(res: &WinRes) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // assets/windows/app.manifest sits two levels above every crate directory (crates/<name>)
    let manifest = manifest_dir.join("../../assets/windows/app.manifest");
    let icon = res.icon.map(|i| manifest_dir.join(i));
    println!("cargo:rerun-if-changed={}", manifest.display());
    if let Some(i) = &icon {
        println!("cargo:rerun-if-changed={}", i.display());
    }
    let rc = out_dir.join("res.rc");
    std::fs::write(&rc, rc_source(res, &version, icon.as_deref(), &manifest)).expect("write res.rc");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_required().expect("compile Windows resources");
}

/// `cargo:rustc-env=SHAIYA_GIT_HASH` (`git -c safe.directory=* rev-parse --short=12 HEAD`, `unknown` on failure) and `SHAIYA_TARGET`.
pub fn build_info() {
    let hash = std::process::Command::new("git")
        .args(["-c", "safe.directory=*", "rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SHAIYA_GIT_HASH={hash}");
    println!("cargo:rustc-env=SHAIYA_TARGET={}", std::env::var("TARGET").unwrap_or_default());
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_become_four_numbers() {
        assert_eq!(version_numbers("0.1.0"), [0, 1, 0, 0]);
        assert_eq!(version_numbers("1.2.3-rc.1+build"), [1, 2, 3, 0]);
        assert_eq!(version_numbers("junk"), [0, 0, 0, 0]);
    }

    #[test]
    fn the_rc_names_icon_manifest_and_strings() {
        let res = WinRes { file_description: "Shaiya Toolkit", original_filename: "shaiya-toolkit.exe", icon: Some("x.ico"), gui: true };
        let s = rc_source(&res, "0.1.0", Some(Path::new("C:\\a\\x.ico")), Path::new("C:\\a\\app.manifest"));
        assert!(s.contains("1 ICON \"C:/a/x.ico\""), "{s}");
        assert!(s.contains("1 24 \"C:/a/app.manifest\""), "{s}");
        assert!(s.contains("FILEVERSION 0,1,0,0"), "{s}");
        assert!(s.contains("VALUE \"FileDescription\", \"Shaiya Toolkit\""), "{s}");
        assert!(s.contains("VALUE \"LegalCopyright\", \"Copyright (c) 2026 byyako. MIT License.\""), "{s}");
        assert!(s.contains("VALUE \"ProductName\", \"Shaiya Toolkit\""), "{s}");
        let none = rc_source(&res, "0.1.0", None, Path::new("m"));
        assert!(!none.contains("ICON"));
    }
}
