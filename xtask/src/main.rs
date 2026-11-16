//! Development tasks of the Shaiya toolkit: `cargo xtask <command>`.

mod icons;
mod tools;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>\n  icons    regenerate assets/icons/";

/// The repository root (the folder above `xtask/`).
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the repository").to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("icons") => match icons::write_all(&root().join("assets/icons")) {
            Ok(files) => {
                println!("wrote {} files to assets/icons", files.len());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
