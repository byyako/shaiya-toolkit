//! `cargo xtask dist --platform <windows-x86_64|linux-x86_64|macos-universal> [--target T]... [--out dist]`: builds the release programs and
//! packs the release assets: one zip per tool and one everything zip, each holding a folder named like the zip with
//! the program(s), `README.txt`, `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt`; plus `SHA256SUMS-<platform>.txt`. Zips are deterministic
//! (sorted entries, fixed timestamps from the commit time). On macOS each GUI is an app bundle (ad-hoc signed), the CLI a universal binary.

use crate::meta::{root, workspace_version};
use crate::tools::{Tool, TOOLS};
use shaiya_core::hash::{sha256, to_hex};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PLATFORMS: [&str; 3] = ["windows-x86_64", "linux-x86_64", "macos-universal"];

pub struct Entry {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
}

pub fn zip_name(asset: &str, version: &str, platform: &str) -> String {
    format!("{asset}-{version}-{platform}.zip")
}

/// When the run is for a tag, the tag must equal the workspace version.
pub fn check_tag(version: &str, ref_type: Option<&str>, ref_name: Option<&str>) -> Result<(), String> {
    if ref_type == Some("tag") {
        let tag = ref_name.unwrap_or("");
        if tag != format!("v{version}") {
            return Err(format!("the tag {tag:?} does not match the workspace version (expected v{version})"));
        }
    }
    Ok(())
}

/// The platform paragraph of a README.txt. `has_gui` is whether the zip holds a window program: a command-line-only zip says nothing about
/// desktops, drivers or apps.
pub fn platform_notes(platform: &str, has_gui: bool) -> &'static str {
    match (platform, has_gui) {
        ("windows-x86_64", _) => "Windows: the programs are not code signed, so SmartScreen may warn the first time (\"Windows protected your PC\"): choose More info, then Run anyway.",
        ("linux-x86_64", false) => "Linux: if the file is not executable, run: chmod +x shaiya",
        ("linux-x86_64", true) => "Linux: the GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver. If the file is not executable, run: chmod +x <program>",
        (_, false) => "macOS: the program is not notarised. If macOS blocks it, run: xattr -d com.apple.quarantine shaiya",
        (_, true) => "macOS: the programs are not notarised (the GUI is untested on macOS until a tester confirms it). If macOS says the developer is unidentified, right-click the app and choose Open, or run: xattr -dr com.apple.quarantine <app>",
    }
}

fn program_name(t: &Tool, platform: &str) -> String {
    match (platform, t.gui) {
        ("macos-universal", true) => format!("{}.app", t.title),
        ("windows-x86_64", _) => format!("{}.exe", t.bin),
        _ => t.bin.to_string(),
    }
}

/// What Shaiya SVMap Editor writes, derived from the list in the spec (section 1.1); shown in the README.txt of a zip that holds it.
pub const EDITOR_SENTENCE: &str = "Shaiya SVMap Editor saves server map files, Map.ini and MapWar.ini where you open them (keeping a .bak of the first version), writes client files to a staging folder you pick, writes patches and folder exports only where you say, and changes data.sah/data.saf only of a test client you pick with Install, which keeps its own copy of the index there (no journal) so Uninstall can restore it; Copy loose adds new files to that test client. It never writes the client it reads from, and never edits game.exe or a DLL. Everything else is written only to a path you name, apart from temporary files in the system temp folder.";

pub fn readme(title: &str, tools: &[&Tool], platform: &str, version: &str) -> String {
    let mut s = format!("{title} {version} ({platform})\n{}\n\n", "=".repeat(title.len() + version.len() + platform.len() + 5));
    s.push_str("Part of the Shaiya Toolkit (https://github.com/byyako/shaiya-toolkit).\n\n");
    s.push_str("Programs in this folder:\n");
    for t in tools {
        s.push_str(&format!("  {:<34} {}\n", program_name(t, platform), if t.gui { "window program: double-click it, or drop files on it" } else { "command line: open a terminal here and run it with --help" }));
    }
    let editor = tools.iter().any(|t| t.bin == "shaiya-svmap-editor");
    let changes = if editor { "Changes the toolkit's other tools make to a client folder" } else { "Changes to a client folder" };
    let tail = if editor { EDITOR_SENTENCE } else { "Everything else is written only to a path you name." };
    s.push_str(&format!("\nThese tools work on your own copy of the game. The repository and the releases contain no game files, and the tools never download any. {changes} are recorded in a journal so they can usually be rolled back, but not changes made with --no-journal or archive compact --in-place, which keep at most the previous index (data.sah.bak). {tail}\n\n"));
    s.push_str(platform_notes(platform, tools.iter().any(|t| t.gui)));
    s.push_str("\n\nCheck the download against SHA256SUMS.txt from the release page.\n\nDocumentation: https://github.com/byyako/shaiya-toolkit/tree/main/docs\nLicence: MIT (LICENSE.txt). Third-party components: THIRD-PARTY-NOTICES.txt\nMade by byyako.\n");
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn plist(t: &Tool, version: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleName</key><string>{title}</string>\n\t<key>CFBundleDisplayName</key><string>{title}</string>\n\t<key>CFBundleIdentifier</key><string>{id}</string>\n\t<key>CFBundleVersion</key><string>{version}</string>\n\t<key>CFBundleShortVersionString</key><string>{version}</string>\n\t<key>CFBundleExecutable</key><string>{bin}</string>\n\t<key>CFBundleIconFile</key><string>{asset}</string>\n\t<key>CFBundlePackageType</key><string>APPL</string>\n\t<key>NSHighResolutionCapable</key><true/>\n\t<key>LSMinimumSystemVersion</key><string>11.0</string>\n</dict>\n</plist>\n",
        title = xml_escape(t.title),
        id = t.bundle_id.unwrap_or("io.github.byyako.shaiya"),
        bin = t.bin,
        asset = t.asset
    )
}

/// `<sha256>  <file>` lines, sorted by file name.
pub fn sums(files: &[(String, Vec<u8>)]) -> String {
    let mut lines: Vec<(String, String)> = files.iter().map(|(n, d)| (n.clone(), to_hex(&sha256(d)))).collect();
    lines.sort();
    lines.into_iter().map(|(n, h)| format!("{h}  {n}\n")).collect()
}

/// A deterministic zip: entries sorted by path, one fixed timestamp, Unix modes kept.
pub fn build_zip(entries: &[Entry], when: zip::DateTime) -> Result<Vec<u8>, String> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for e in sorted {
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).last_modified_time(when).unix_permissions(e.mode);
        z.start_file(e.path.clone(), opts).map_err(|e| e.to_string())?;
        z.write_all(&e.data).map_err(|e| e.to_string())?;
    }
    Ok(z.finish().map_err(|e| e.to_string())?.into_inner())
}

/// A zip timestamp from seconds since the Unix epoch (the tagged commit's time).
pub fn zip_time(unix: i64) -> zip::DateTime {
    let (y, mo, d, h, mi, s) = shaiya_core::time::unix_parts(unix);
    zip::DateTime::from_date_and_time(y.clamp(1980, 2107) as u16, mo as u8, d as u8, h as u8, mi as u8, s as u8).unwrap_or_default()
}

fn commit_time() -> i64 {
    Command::new("git")
        .args(["-c", "safe.directory=*", "log", "-1", "--format=%ct"])
        .current_dir(root())
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(1_767_225_600) // 2026-01-01
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn run_ok(cmd: &mut Command, what: &str) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("cannot run {what}: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({st})"))
    }
}

fn mode_of(p: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o644)
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        0o644
    }
}

fn read_tree(base: &Path, prefix: &str, out: &mut Vec<Entry>) -> Result<(), String> {
    let mut names: Vec<_> = std::fs::read_dir(base).map_err(|e| format!("{}: {e}", base.display()))?.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let name = e.file_name().to_string_lossy().into_owned();
        let p = e.path();
        let path = format!("{prefix}/{name}");
        if p.is_dir() {
            read_tree(&p, &path, out)?;
        } else {
            out.push(Entry { path, data: std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?, mode: mode_of(&p) });
        }
    }
    Ok(())
}

/// The program as zip entries below `folder/`: a plain file, or on macOS a signed app bundle for GUIs.
fn program_entries(t: &Tool, platform: &str, binaries: &[PathBuf], version: &str, staging: &Path, folder: &str) -> Result<Vec<Entry>, String> {
    let bin_path = if binaries.len() > 1 {
        // universal binary
        let out = staging.join(t.bin);
        let mut c = Command::new("lipo");
        c.arg("-create").arg("-output").arg(&out).args(binaries);
        run_ok(&mut c, "lipo")?;
        out
    } else {
        binaries[0].clone()
    };
    let data = std::fs::read(&bin_path).map_err(|e| format!("{}: {e}", bin_path.display()))?;
    if platform == "macos-universal" && t.gui {
        let app = staging.join(format!("{}.app", t.title));
        let _ = std::fs::remove_dir_all(&app);
        let macos = app.join("Contents").join("MacOS");
        let res = app.join("Contents").join("Resources");
        std::fs::create_dir_all(&macos).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&res).map_err(|e| e.to_string())?;
        std::fs::write(macos.join(t.bin), &data).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(macos.join(t.bin), std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        std::fs::write(app.join("Contents").join("Info.plist"), plist(t, version)).map_err(|e| e.to_string())?;
        // .icns from the 512 px PNG with sips and iconutil
        let iconset = staging.join(format!("{}.iconset", t.asset));
        let _ = std::fs::remove_dir_all(&iconset);
        std::fs::create_dir_all(&iconset).map_err(|e| e.to_string())?;
        let src = root().join("assets").join("icons").join(format!("{}-512.png", t.asset));
        for (n, name) in [(16, "icon_16x16"), (32, "icon_16x16@2x"), (32, "icon_32x32"), (64, "icon_32x32@2x"), (128, "icon_128x128"), (256, "icon_128x128@2x"), (256, "icon_256x256"), (512, "icon_256x256@2x"), (512, "icon_512x512")] {
            let mut c = Command::new("sips");
            c.args(["-z", &n.to_string(), &n.to_string()]).arg(&src).arg("--out").arg(iconset.join(format!("{name}.png")));
            run_ok(&mut c, "sips")?;
        }
        let mut c = Command::new("iconutil");
        c.args(["-c", "icns"]).arg(&iconset).arg("-o").arg(res.join(format!("{}.icns", t.asset)));
        run_ok(&mut c, "iconutil")?;
        let mut c = Command::new("codesign");
        c.args(["--force", "--deep", "-s", "-"]).arg(&app);
        run_ok(&mut c, "codesign")?;
        let mut out = Vec::new();
        read_tree(&app, &format!("{folder}/{}.app", t.title), &mut out)?;
        return Ok(out);
    }
    Ok(vec![Entry { path: format!("{folder}/{}", program_name(t, platform)), data, mode: 0o755 }])
}

pub struct Args {
    pub platform: String,
    pub targets: Vec<String>,
    pub out: PathBuf,
}

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let (mut platform, mut targets, mut out) = (None, Vec::new(), PathBuf::from("dist"));
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--platform" => platform = it.next().cloned(),
            "--target" => targets.push(it.next().cloned().ok_or("--target needs a value")?),
            "--out" => out = PathBuf::from(it.next().ok_or("--out needs a folder")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let platform = platform.ok_or("--platform is required")?;
    if !PLATFORMS.contains(&platform.as_str()) {
        return Err(format!("unknown platform {platform}; use one of {}", PLATFORMS.join(", ")));
    }
    Ok(Args { platform, targets, out })
}

pub fn run(args: &Args) -> Result<(), String> {
    let version = workspace_version()?;
    check_tag(&version, std::env::var("GITHUB_REF_TYPE").ok().as_deref(), std::env::var("GITHUB_REF_NAME").ok().as_deref())?;
    let root = root();
    let platform = args.platform.as_str();
    let targets: Vec<Option<String>> = if args.targets.is_empty() { vec![None] } else { args.targets.iter().cloned().map(Some).collect() };
    if platform == "macos-universal" && targets.len() != 2 {
        return Err("macos-universal needs two --target values (aarch64-apple-darwin and x86_64-apple-darwin)".into());
    }
    for t in &targets {
        let mut c = Command::new(cargo());
        c.current_dir(&root).args(["build", "--release", "--locked"]);
        for tool in TOOLS {
            c.args(["-p", tool.package]);
        }
        if let Some(t) = t {
            c.args(["--target", t]);
        }
        run_ok(&mut c, "cargo build --release")?;
    }
    let out = if args.out.is_absolute() { args.out.clone() } else { root.join(&args.out) };
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let staging = out.join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let windows = platform.starts_with("windows");
    let license = std::fs::read(root.join("LICENSE")).map_err(|e| format!("LICENSE: {e}"))?;
    let notices = std::fs::read(root.join("THIRD-PARTY-NOTICES.txt")).map_err(|e| format!("THIRD-PARTY-NOTICES.txt: {e}"))?;
    let when = zip_time(commit_time());
    let mut written: Vec<(String, Vec<u8>)> = Vec::new();

    // the command line alone, and the desktop app together with the command line (named after the app)
    let mut groups: Vec<(String, String, Vec<&Tool>)> = TOOLS.iter().filter(|t| !t.gui).map(|t| (t.asset.to_string(), t.title.to_string(), vec![t])).collect();
    groups.push(("shaiya-toolkit".into(), "Shaiya Toolkit".into(), TOOLS.iter().collect()));
    for (asset, title, tools) in groups {
        let name = zip_name(&asset, &version, platform);
        let folder = name.trim_end_matches(".zip").to_string();
        let mut entries = Vec::new();
        for t in &tools {
            let binaries: Vec<PathBuf> = targets.iter().map(|tg| crate::sizes::binary_path(t.bin, windows, tg.as_deref())).collect();
            entries.extend(program_entries(t, platform, &binaries, &version, &staging, &folder)?);
        }
        entries.push(Entry { path: format!("{folder}/README.txt"), data: readme(&title, &tools, platform, &version).into_bytes(), mode: 0o644 });
        entries.push(Entry { path: format!("{folder}/LICENSE.txt"), data: license.clone(), mode: 0o644 });
        entries.push(Entry { path: format!("{folder}/THIRD-PARTY-NOTICES.txt"), data: notices.clone(), mode: 0o644 });
        let bytes = build_zip(&entries, when)?;
        std::fs::write(out.join(&name), &bytes).map_err(|e| format!("{name}: {e}"))?;
        println!("dist: {name} ({} bytes, {} entries)", bytes.len(), list_zip(&bytes)?.len());
        if windows {
            let limit = crate::sizes::zip_budget(&asset);
            let ok = crate::sizes::within(bytes.len() as u64, limit);
            println!("dist: {name} budget {limit} bytes: {}", if ok { "budget ok" } else { "budget OVER" });
            if !ok {
                return Err(format!("{name} is {} bytes, over the {limit} budget", bytes.len()));
            }
        }
        written.push((name, bytes));
    }
    let sums_name = format!("SHA256SUMS-{platform}.txt");
    std::fs::write(out.join(&sums_name), sums(&written)).map_err(|e| format!("{sums_name}: {e}"))?;
    let _ = std::fs::remove_dir_all(&staging);
    println!("dist: wrote {sums_name}");
    Ok(())
}

/// The entries of a zip as (path, size, unix mode), for tests and the `dist` self-check.
pub fn list_zip(bytes: &[u8]) -> Result<Vec<(String, u64, Option<u32>)>, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|e| e.to_string())?;
        out.push((f.name().to_string(), f.size(), f.unix_mode()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_release_asset_scheme() {
        assert_eq!(zip_name("shaiya-toolkit", "0.1.0", "windows-x86_64"), "shaiya-toolkit-0.1.0-windows-x86_64.zip");
        assert_eq!(zip_name("shaiya-toolkit", "1.2.3", "macos-universal"), "shaiya-toolkit-1.2.3-macos-universal.zip");
    }

    #[test]
    fn a_tag_must_equal_the_workspace_version() {
        assert!(check_tag("0.1.0", Some("tag"), Some("v0.1.0")).is_ok());
        assert!(check_tag("0.1.0", Some("tag"), Some("v0.2.0")).is_err());
        assert!(check_tag("0.1.0", Some("tag"), None).is_err());
        assert!(check_tag("0.1.0", Some("branch"), Some("main")).is_ok());
        assert!(check_tag("0.1.0", None, None).is_ok());
    }

    #[test]
    fn zips_are_deterministic_sorted_and_keep_modes() {
        let when = zip_time(1_800_000_000);
        let entries = vec![
            Entry { path: "d/README.txt".into(), data: b"hello".to_vec(), mode: 0o644 },
            Entry { path: "d/shaiya".into(), data: vec![7; 5000], mode: 0o755 },
            Entry { path: "d/LICENSE.txt".into(), data: b"mit".to_vec(), mode: 0o644 },
        ];
        let a = build_zip(&entries, when).unwrap();
        let b = build_zip(&entries, when).unwrap();
        assert_eq!(a, b, "the same input gives the same bytes");
        let list = list_zip(&a).unwrap();
        assert_eq!(list.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), vec!["d/LICENSE.txt", "d/README.txt", "d/shaiya"]);
        assert_eq!(list[2].2.map(|m| m & 0o777), Some(0o755));
        assert_eq!(list[0].2.map(|m| m & 0o777), Some(0o644));
        // a different timestamp changes the bytes
        assert_ne!(a, build_zip(&entries, zip_time(1_700_000_000)).unwrap());
    }

    #[test]
    fn checksums_are_sorted_lines() {
        let s = sums(&[("b.zip".into(), b"abc".to_vec()), ("a.zip".into(), b"".to_vec())]);
        assert_eq!(s, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  a.zip\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  b.zip\n");
    }

    #[test]
    fn readme_has_the_statement_and_the_platform_note() {
        let gui = &TOOLS[1];
        let r = readme(gui.title, &[gui], "windows-x86_64", "0.1.0");
        assert!(r.contains("These tools work on your own copy of the game"));
        assert!(r.contains("SmartScreen") && r.contains("shaiya-toolkit.exe"));
        let m = readme(gui.title, &[gui], "macos-universal", "0.1.0");
        assert!(m.contains("xattr -dr com.apple.quarantine") && m.contains("Shaiya Toolkit.app"));
        let cli_linux = readme("x", &[&TOOLS[0]], "linux-x86_64", "0.1.0");
        assert!(cli_linux.contains("chmod +x shaiya") && !cli_linux.contains("X11") && !cli_linux.contains("GUI"));
        let cli_mac = readme("x", &[&TOOLS[0]], "macos-universal", "0.1.0");
        assert!(cli_mac.contains("xattr -d com.apple.quarantine shaiya") && !cli_mac.contains(".app ") && !cli_mac.contains(".app.") && !cli_mac.contains("the app") && !cli_mac.contains("right-click"));
        let cli_win = readme("x", &[&TOOLS[0]], "windows-x86_64", "0.1.0");
        assert!(cli_win.contains("SmartScreen"));
        let all: Vec<&Tool> = TOOLS.iter().collect();
        let all_linux = readme("x", &all, "linux-x86_64", "0.1.0");
        assert!(all_linux.contains("X11") && all_linux.contains("chmod +x <program>"));
        let all_mac = readme("x", &all, "macos-universal", "0.1.0");
        assert!(all_mac.contains("right-click the app") && all_mac.contains("xattr -dr com.apple.quarantine <app>"));
    }

    #[test]
    fn the_editor_sentence_is_only_in_the_zip_that_holds_the_editor() {
        let all: Vec<&Tool> = TOOLS.iter().collect();
        let r = readme("Shaiya Toolkit", &all, "windows-x86_64", "0.1.0");
        assert!(r.starts_with("Shaiya Toolkit 0.1.0"));
        assert!(r.contains(EDITOR_SENTENCE) && r.contains("shaiya-svmap-editor.exe"));
        assert!(r.contains("Map.ini and MapWar.ini") && r.contains("apart from temporary files in the system temp folder."));
        assert!(r.contains("Changes the toolkit's other tools make to a client folder are recorded in a journal"));
        let cli = readme("Shaiya command line", &[&TOOLS[0]], "windows-x86_64", "0.1.0");
        assert!(!cli.contains("SVMap Editor") && !cli.contains("Copy loose"));
        assert!(cli.contains("Changes to a client folder are recorded in a journal"));
        assert!(cli.contains("Everything else is written only to a path you name.") && !cli.contains("temp folder"));
    }

    #[test]
    fn the_plist_names_the_bundle() {
        let p = plist(&TOOLS[1], "0.1.0");
        assert!(p.contains("<string>Shaiya Toolkit</string>"));
        assert!(p.contains("io.github.byyako.shaiya-toolkit") && p.contains("<string>shaiya-toolkit</string>"));
    }

    #[test]
    fn arguments() {
        let a = parse_args(&["--platform".into(), "macos-universal".into(), "--target".into(), "a".into(), "--target".into(), "b".into(), "--out".into(), "o".into()]).unwrap();
        assert_eq!((a.platform.as_str(), a.targets.len(), a.out), ("macos-universal", 2, PathBuf::from("o")));
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--platform".into(), "beos".into()]).is_err());
    }
}
