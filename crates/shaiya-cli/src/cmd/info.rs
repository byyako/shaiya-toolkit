//! `shaiya info [--hash] <PATH>...`

use crate::output::{error_json, group, Report};
use crate::Ctx;
use serde::Serialize;
use shaiya_core::{Error, Job};
use shaiya_inspect::{Details, Info, Kind, Options};
use std::io::Write;
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    /// Files to identify (data.sah, data.saf, SData, patches, game.exe, ...)
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// Also compute the SHA-256 of each file
    #[arg(long)]
    pub hash: bool,
}

#[derive(Serialize)]
pub struct InfoReport {
    pub files: Vec<Info>,
    /// Paths that could not be read at all (only present when there are some).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<serde_json::Value>,
}

fn describe(i: &Info) -> String {
    let size = group(i.size);
    match &i.details {
        Details::Sah { version, files, dirs, free_entries, data_end, .. } => format!(
            "SAH index, version {version}, {} files in {} folders, {} free entries, data ends at {}",
            group(*files as u64),
            group(*dirs as u64),
            group(*free_entries as u64),
            group(*data_end)
        ),
        Details::Saf { index_files, live_bytes } => match (index_files, live_bytes) {
            (Some(n), Some(l)) => format!("SAF data, {size} bytes, its index lists {} files using {} bytes", group(*n as u64), group(*l)),
            _ => format!("SAF data, {size} bytes (no .sah next to it)"),
        },
        Details::SdataSeed { plain_size, crc_ok, .. } => {
            format!("SData, SEED-encrypted, plaintext {} bytes, checksum {}", group(*plain_size as u64), if *crc_ok { "OK" } else { "FAILED" })
        }
        Details::SdataPlain => format!("SData, plain (not encrypted), {size} bytes"),
        Details::Patch { files, loose, removals, signature_file } => format!(
            "Patch, {files} archive files, {loose} loose files, {removals} removals, signature file {}, {size} bytes",
            if *signature_file { "present" } else { "not found" }
        ),
        Details::PatchSignature { hex_len } => format!("Patch signature, {hex_len} hex digits"),
        Details::Pe { machine_name, timestamp_utc, .. } => {
            format!("Windows program ({machine_name}), linked {} UTC, {size} bytes", timestamp_utc.replace('T', " ").trim_end_matches('Z'))
        }
        Details::Unknown { magic_hex } => format!("Unknown file type, starts with {magic_hex}, {size} bytes"),
    }
}

impl Report for InfoReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let names: Vec<String> =
            self.files.iter().map(|i| std::path::Path::new(&i.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| i.path.clone())).collect();
        let width = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
        for (i, name) in self.files.iter().zip(&names) {
            writeln!(w, "{name:<width$}  {}", describe(i))?;
            let pad = " ".repeat(width + 2);
            if i.kind == Kind::Sah {
                writeln!(w, "{pad}{} bytes", group(i.size))?;
            }
            if let Some(h) = &i.sha256 {
                writeln!(w, "{pad}sha256 {h}")?;
            }
            for p in &i.problems {
                writeln!(w, "{pad}problem: {p}")?;
            }
        }
        Ok(())
    }
}

/// Returns the exit code: 0, or 3 when a path could not be read or has problems.
pub fn run(ctx: &Ctx, args: &Args, job: &Job) -> i32 {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    let mut code = 0;
    for path in &args.paths {
        let started = std::time::Instant::now();
        job.stage(&format!("hashing {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        match shaiya_inspect::inspect(path, &Options { hash: args.hash }, job) {
            Ok(info) => {
                if ctx.verbose > 0 {
                    eprintln!("{}: read in {:.0} ms", path.display(), started.elapsed().as_secs_f64() * 1000.0);
                }
                if !info.problems.is_empty() {
                    code = 3;
                }
                files.push(info);
            }
            Err(e) => {
                code = code.max(crate::exit_code_for(e.kind()) as i32);
                report_error(ctx, &e);
                errors.push(error_json(&e)["error"].clone());
            }
        }
    }
    crate::output::emit(ctx, &InfoReport { files, errors });
    code
}

fn report_error(ctx: &Ctx, e: &Error) {
    if !ctx.json || !ctx.quiet {
        eprintln!("error: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_lines_follow_the_documented_shape() {
        let info = Info {
            path: "x/data.sah".into(),
            size: 771_851,
            kind: Kind::Sah,
            details: Details::Sah { version: 0, files: 21_523, dirs: 1_204, free_entries: 12, trailer_len: 4, data_end: 1_991_954_802 },
            sha256: Some("ab".repeat(32)),
            problems: vec![],
        };
        let mut out = Vec::new();
        InfoReport { files: vec![info], errors: vec![] }.human(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "data.sah  SAH index, version 0, 21,523 files in 1,204 folders, 12 free entries, data ends at 1,991,954,802");
        assert_eq!(lines[1], "          771,851 bytes");
        assert!(lines[2].starts_with("          sha256 abab"));
    }
}
