//! Development tasks of the Shaiya toolkit: `cargo xtask <command>`.

mod deps;
mod dist;
#[cfg(test)]
mod docs_images;
mod guard;
mod icons;
mod meta;
mod notices;
mod release_notes;
mod sizes;
mod tools;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>
  dist --platform <windows-x86_64|linux-x86_64|macos-universal> [--target T]... [--out dist]
                         build the release programs and pack the release zips and checksums
  notices [--check]      regenerate THIRD-PARTY-NOTICES.txt (or check that it is up to date)
  deps-check             licence allowlist and network denylist over the shipped programs' dependencies
  guard                  no game files among the tracked files
  sizes [--platform P] [--target T]
                         release binary sizes against the budgets
  release-notes <version>  the CHANGELOG.md section of a version
  icons                  regenerate assets/icons/";

fn opt_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result: Result<(), String> = match args.first().map(String::as_str) {
        Some("icons") => icons::write_all(&meta::root().join("assets/icons")).map(|f| println!("wrote {} files to assets/icons", f.len())).map_err(|e| e.to_string()),
        Some("dist") => dist::parse_args(rest).and_then(|a| dist::run(&a)),
        Some("notices") => notices::run(rest.iter().any(|a| a == "--check")),
        Some("deps-check") => deps::run(),
        Some("guard") => guard::run(),
        Some("sizes") => sizes::run(opt_value(rest, "--platform").as_deref(), opt_value(rest, "--target").as_deref()),
        Some("release-notes") => match rest.first() {
            Some(v) => release_notes::run(v),
            None => Err("release-notes needs a version".into()),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
