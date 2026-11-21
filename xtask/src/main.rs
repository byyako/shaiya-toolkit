//! Development tasks of the Shaiya toolkit: `cargo xtask <command>`.

mod deps;
mod dist;
#[cfg(test)]
mod docs_formats;
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
  icons                  regenerate assets/icons/
  demo-client DIR        write a synthetic client folder (made-up files only) into the empty folder DIR, for the screenshots
                         of Shaiya Toolkit (see docs/dev/docs-site.md)";

fn opt_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result: Result<(), String> = match args.first().map(String::as_str) {
        Some("demo-client") => match rest.first() {
            Some(d) => demo_client(std::path::Path::new(d)),
            None => Err("demo-client needs a folder".into()),
        },
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

/// `cargo xtask demo-client DIR`: a synthetic client in an empty (or new) folder. Refuses a folder that holds anything.
fn demo_client(dir: &std::path::Path) -> Result<(), String> {
    if dir.exists() && std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.next().is_some() {
        return Err(format!("{} is not empty; give an empty or new folder", dir.display()));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let files = shaiya_testkit::demo_client(dir);
    println!("wrote a demo client with {} archive files to {}", files.len(), dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_client_writes_a_marked_client_and_refuses_a_folder_with_files() {
        let t = shaiya_testkit::TempDir::new("xtask-demo");
        let dir = t.path().join("demo");
        demo_client(&dir).unwrap();
        assert!(dir.join("data.sah").is_file() && dir.join("data.saf").is_file());
        assert!(dir.join(shaiya_testkit::demo::DEMO_MARKER).is_file());
        let again = demo_client(&dir).unwrap_err();
        assert!(again.contains("is not empty"), "{again}");
    }
}
