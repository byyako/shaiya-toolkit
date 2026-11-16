//! The shell flags of every GUI program: `[FILES...]`, `--data-dir DIR`, `--no-settings`, `--smoke [REPORT_FILE]`.

use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellOptions {
    pub files: Vec<PathBuf>,
    /// Settings, log, recovery and backups live here instead of the default data folder.
    pub data_dir: Option<PathBuf>,
    /// Nothing is read or written (used by tests).
    pub no_settings: bool,
    /// Open the real window, render 5 frames, write `ok <adapter> <backend>` (or the error) to the report file if given, exit 0 or 1.
    pub smoke: Option<Option<PathBuf>>,
    /// Load a CJK system font (off in the headless tests).
    pub load_fonts: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(ShellOptions),
    Help,
    Version,
}

pub const USAGE: &str = "usage: <program> [FILES...] [--data-dir DIR] [--no-settings] [--smoke [REPORT_FILE]]\n\n  FILES              open these files\n  --data-dir DIR     keep settings, log, recovery and backups in DIR\n  --no-settings      read and write no settings at all\n  --smoke [FILE]     open the window, render 5 frames, write \"ok <adapter> <backend>\" (or the error) to FILE, and exit 0 or 1\n  -h, --help         show this help\n  -V, --version      show the version";

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, String> {
    let mut o = ShellOptions { load_fonts: true, ..Default::default() };
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
            "--" => o.files.extend(it.by_ref().map(PathBuf::from)),
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            f => o.files.push(PathBuf::from(f)),
        }
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
    fn errors_and_help() {
        assert_eq!(p(&["--nope"]), Err("unknown option --nope".to_string()));
        assert!(p(&["--data-dir"]).is_err());
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
        assert_eq!(p(&["x", "-V"]), Ok(Parsed::Version));
        // after `--` everything is a file name
        assert_eq!(run(&["--", "--weird"]).files, vec![PathBuf::from("--weird")]);
    }
}
