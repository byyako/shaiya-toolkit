//! The shell flags of every GUI program: `[FILES...]`, `--data-dir DIR`, `--no-settings`, `--smoke [REPORT_FILE]`, `--shot OUT.png [--size WxH] [--scene NAME]`.

use std::path::PathBuf;

/// `--shot OUT.png [--size WxH] [--scene NAME]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotOptions {
    pub out: PathBuf,
    /// The window size in points; `None` keeps the default window size.
    pub size: Option<[u32; 2]>,
    /// The tool's scene (`start` when not given).
    pub scene: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellOptions {
    pub files: Vec<PathBuf>,
    /// Settings, log, recovery and backups live here instead of the default data folder.
    pub data_dir: Option<PathBuf>,
    /// Nothing is read or written (used by tests).
    pub no_settings: bool,
    /// Open the real window, render 5 frames, write `ok <adapter> <backend>` (or the error) to the report file if given, exit 0 or 1.
    pub smoke: Option<Option<PathBuf>>,
    /// Open the real window, show a scene of the tool, save a PNG of the window, exit 0 or 1. Implies `no_settings` unless `data_dir` is given.
    pub shot: Option<ShotOptions>,
    /// Load a CJK system font (off in the headless tests).
    pub load_fonts: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(ShellOptions),
    Help,
    Version,
}

pub const USAGE: &str = "usage: <program> [FILES...] [--data-dir DIR] [--no-settings] [--smoke [REPORT_FILE]] [--shot OUT.png [--size WxH] [--scene NAME]]\n\n  FILES              open these files\n  --data-dir DIR     keep settings, log, recovery and backups in DIR\n  --no-settings      read and write no settings at all\n  --smoke [FILE]     open the window, render 5 frames, write \"ok <adapter> <backend>\" (or the error) to FILE, and exit 0 or 1\n  --shot OUT.png     open the window, show a scene (default start), save a PNG of the window to OUT.png, and exit 0 or 1; no settings are used unless --data-dir is given\n  --size WxH         the window size of --shot (default 1280x800)\n  --scene NAME       the scene of --shot (each program documents its scenes)\n  -h, --help         show this help\n  -V, --version      show the version";

/// The window size of `--shot` when `--size` is not given.
pub const SHOT_DEFAULT_SIZE: [u32; 2] = [1280, 800];

/// `WxH` in points, each side 320 to 4096.
fn parse_size(s: &str) -> Result<[u32; 2], String> {
    let bad = || format!("--size {s:?} is not WxH (for example 1280x800, each side 320 to 4096)");
    let (w, h) = s.split_once(['x', 'X']).ok_or_else(bad)?;
    let (w, h) = (w.parse::<u32>().map_err(|_| bad())?, h.parse::<u32>().map_err(|_| bad())?);
    if !(320..=4096).contains(&w) || !(320..=4096).contains(&h) {
        return Err(bad());
    }
    Ok([w, h])
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, String> {
    let mut o = ShellOptions { load_fonts: true, ..Default::default() };
    let (mut shot_out, mut shot_size, mut shot_scene) = (None, None, None);
    let mut it = args.into_iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--no-settings" => o.no_settings = true,
            "--data-dir" => o.data_dir = Some(it.next().ok_or("--data-dir needs a folder")?.into()),
            "--smoke" => {
                let report = it.next_if(|n| !n.starts_with("--")).map(PathBuf::from);
                o.smoke = Some(report);
            }
            "--shot" => shot_out = Some(PathBuf::from(it.next().ok_or("--shot needs a file name")?)),
            "--size" => shot_size = Some(parse_size(&it.next().ok_or("--size needs WxH")?)?),
            "--scene" => shot_scene = Some(it.next().ok_or("--scene needs a name")?),
            "--" => o.files.extend(it.by_ref().map(PathBuf::from)),
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f => o.files.push(PathBuf::from(f)),
        }
    }
    match shot_out {
        Some(out) => {
            o.shot = Some(ShotOptions { out, size: shot_size, scene: shot_scene.unwrap_or_else(|| "start".to_string()) });
            // a screenshot never reads or writes the user's settings (and never shows their client list) unless --data-dir is given
            o.no_settings |= o.data_dir.is_none();
        }
        None if shot_size.is_some() || shot_scene.is_some() => return Err("--size and --scene need --shot".to_string()),
        None => {}
    }
    Ok(Parsed::Run(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Parsed, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    fn run(args: &[&str]) -> ShellOptions {
        match p(args).unwrap() {
            Parsed::Run(o) => o,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn files_and_flags() {
        let o = run(&["a.sah", "--data-dir", "d", "b.exe", "--no-settings"]);
        assert_eq!(o.files, vec![PathBuf::from("a.sah"), PathBuf::from("b.exe")]);
        assert_eq!(o.data_dir, Some(PathBuf::from("d")));
        assert!(o.no_settings && o.load_fonts);
        assert_eq!(run(&[]), ShellOptions { load_fonts: true, ..Default::default() });
    }

    #[test]
    fn smoke_takes_an_optional_report_file() {
        assert_eq!(run(&["--smoke"]).smoke, Some(None));
        assert_eq!(run(&["--smoke", "r.txt"]).smoke, Some(Some(PathBuf::from("r.txt"))));
        let o = run(&["--smoke", "--no-settings"]);
        assert_eq!(o.smoke, Some(None));
        assert!(o.no_settings);
    }

    #[test]
    fn shot_takes_a_file_a_size_and_a_scene_and_implies_no_settings() {
        let o = run(&["--shot", "a.png"]);
        assert_eq!(o.shot, Some(ShotOptions { out: PathBuf::from("a.png"), size: None, scene: "start".into() }));
        assert!(o.no_settings, "a screenshot reads no settings");
        let o = run(&["--scene", "archive", "--size", "1000x700", "--shot", "b.png", "dir"]);
        assert_eq!(o.shot, Some(ShotOptions { out: PathBuf::from("b.png"), size: Some([1000, 700]), scene: "archive".into() }));
        assert_eq!(o.files, vec![PathBuf::from("dir")]);
        let o = run(&["--shot", "a.png", "--data-dir", "d"]);
        assert!(!o.no_settings, "--data-dir keeps the settings of that folder");
        assert_eq!(o.data_dir, Some(PathBuf::from("d")));
        assert_eq!(run(&["--no-settings"]).shot, None);
    }

    #[test]
    fn shot_flags_are_checked() {
        assert!(p(&["--shot"]).is_err());
        assert!(p(&["--shot", "a.png", "--size"]).is_err());
        assert!(p(&["--shot", "a.png", "--scene"]).is_err());
        for bad in ["1280", "axb", "100x800", "1280x5000", "1280x", "-1x800"] {
            assert!(p(&["--shot", "a.png", "--size", bad]).is_err(), "{bad}");
        }
        assert_eq!(p(&["--size", "800x600"]), Err("--size and --scene need --shot".to_string()));
        assert_eq!(p(&["--scene", "start"]), Err("--size and --scene need --shot".to_string()));
    }

    #[test]
    fn errors_and_help() {
        assert_eq!(p(&["--nope"]), Err("unknown option --nope".to_string()));
        assert!(p(&["--data-dir"]).is_err());
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
        assert_eq!(p(&["x", "-V"]), Ok(Parsed::Version));
        // after `--` everything is a file name
        assert_eq!(run(&["--", "--weird"]).files, vec![PathBuf::from("--weird")]);
    }
}
