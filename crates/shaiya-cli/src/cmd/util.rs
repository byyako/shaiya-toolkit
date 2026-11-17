//! Helpers shared by the command groups: errors, notes, `@name` resolution, opening archives, client folders, number formatting.

use crate::output::error_json;
use crate::Ctx;
use shaiya_archive::client::{self, ClientFiles, InUse};
use shaiya_archive::journal;
use shaiya_archive::{saf_of, Archive, Index};
use shaiya_core::time::{human_size, utc_iso_unix};
use shaiya_core::{Error, Result};
use std::io::{BufWriter, StdoutLock};
use std::path::{Path, PathBuf};

pub use shaiya_inspect::group as num;

pub fn out() -> BufWriter<StdoutLock<'static>> {
    BufWriter::with_capacity(1 << 16, std::io::stdout().lock())
}

/// Prints the error (and the JSON error document with `--json`) and returns the exit code.
pub fn fail(ctx: &Ctx, e: &Error) -> i32 {
    if !ctx.json || !ctx.quiet {
        eprintln!("error: {e}");
    }
    if let Error::Io { path: Some(p), .. } = e {
        if p.to_string_lossy().starts_with('@') {
            eprintln!("hint: shaiya clients list");
        }
    }
    if ctx.json {
        crate::output::print_json(&error_json(e));
    }
    crate::exit_code_for(e.kind()) as i32
}

/// A message that is not an error (stderr, silenced by `--quiet`).
pub fn note(ctx: &Ctx, text: &str) {
    if !ctx.quiet {
        eprintln!("{text}");
    }
}

pub fn warn(ctx: &Ctx, text: &str) {
    if !ctx.quiet {
        eprintln!("warning: {text}");
    }
}

pub fn size(n: u64) -> String {
    human_size(n)
}

pub fn resolve(ctx: &Ctx, arg: &str) -> Result<PathBuf> {
    crate::config::resolve(ctx.data_dir.as_deref(), arg)
}

/// `1,234 files` / `1 file`.
pub fn count(n: usize, what: &str) -> String {
    let plural = if n == 1 {
        what.to_string()
    } else if what.ends_with('y') {
        format!("{}ies", &what[..what.len() - 1])
    } else if what.ends_with("ch") || what.ends_with('s') {
        format!("{what}es")
    } else {
        format!("{what}s")
    };
    format!("{} {plural}", num(n as u64))
}

pub fn iso_version(v: i32) -> Option<String> {
    (v > 0).then(|| utc_iso_unix(v as i64))
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// An archive opened for reading: with its `data.saf`, or the index alone.
pub enum Opened {
    Full(Archive),
    IndexOnly { sah: PathBuf, index: Index },
}

impl Opened {
    pub fn index(&self) -> &Index {
        match self {
            Opened::Full(a) => a.index(),
            Opened::IndexOnly { index, .. } => index,
        }
    }
    pub fn sah(&self) -> &Path {
        match self {
            Opened::Full(a) => a.sah_path(),
            Opened::IndexOnly { sah, .. } => sah,
        }
    }
    pub fn saf_len(&self) -> Option<u64> {
        match self {
            Opened::Full(a) => Some(a.saf_len()),
            Opened::IndexOnly { .. } => None,
        }
    }
    pub fn saf_path(&self) -> Option<PathBuf> {
        match self {
            Opened::Full(a) => Some(a.saf_path().to_path_buf()),
            Opened::IndexOnly { .. } => None,
        }
    }
    pub fn archive(&self) -> Option<&Archive> {
        match self {
            Opened::Full(a) => Some(a),
            Opened::IndexOnly { .. } => None,
        }
    }
}

/// ARCHIVE argument: a client folder, a `.sah` file, or `@name`. A missing `.saf` gives index-only mode with a note.
pub fn open_read(ctx: &Ctx, arg: &str) -> Result<Opened> {
    let path = resolve(ctx, arg)?;
    let (sah, index) = Archive::open_index(&path)?;
    if saf_of(&sah).is_file() {
        return Ok(Opened::Full(Archive::open_parsed(sah, index)?));
    }
    note(ctx, "note: data.saf not found; working from the index only");
    Ok(Opened::IndexOnly { sah, index })
}

/// Like [`open_read`] but the `.saf` must be there.
pub fn open_full(ctx: &Ctx, arg: &str) -> Result<Archive> {
    let path = resolve(ctx, arg)?;
    Archive::open(&path)
}

/// A CLIENT argument (a folder, a folder's `data.sah` in any case, or `@name`) as the folder.
pub fn client_dir(ctx: &Ctx, arg: &str) -> Result<PathBuf> {
    let p = resolve(ctx, arg)?;
    if p.is_dir() {
        return Ok(p);
    }
    if p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("data.sah")) {
        return Ok(p.parent().map(Path::to_path_buf).filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from(".")));
    }
    if p.is_file() {
        return Err(Error::Refused(format!(
            "{} is not a client folder's data.sah; in-place changes work on a folder's data.sah/data.saf (hint: use -o to write a new pair)",
            p.display()
        )));
    }
    Err(Error::io(p, std::io::Error::new(std::io::ErrorKind::NotFound, "no such client folder")))
}

/// The first steps of every write (locate, recover (printing the notes), the in-use check).
pub fn prepare_write(ctx: &Ctx, dir: &Path, check_in_use: bool) -> Result<ClientFiles> {
    let files = ClientFiles::locate(dir)?;
    for n in client::recover(dir)? {
        note(ctx, &format!("note: {n}"));
    }
    if check_in_use && client::ensure_not_in_use(&files)? == InUse::Unknown {
        warn(ctx, "cannot check whether the game is running on this system; make sure it is closed");
    }
    Ok(files)
}

pub fn journal_root(dir: &Path, custom: Option<&Path>) -> PathBuf {
    custom.map(Path::to_path_buf).unwrap_or_else(|| dir.join(journal::DEFAULT_DIR))
}

/// The command a person can run to undo the newest change.
pub fn undo_hint(client_arg: &str) -> String {
    let c = if client_arg.contains(' ') { format!("\"{client_arg}\"") } else { client_arg.to_string() };
    format!("shaiya patch rollback {c}")
}

/// `-o OUT` for a new archive pair: a path ending in `.sah` names the index, any other path is a folder that gets `data.sah`.
pub fn out_sah(out: &Path) -> Result<PathBuf> {
    if out.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("sah")) {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        Ok(out.to_path_buf())
    } else {
        std::fs::create_dir_all(out).map_err(|e| Error::io(out, e))?;
        Ok(out.join("data.sah"))
    }
}

/// The `--in-place`/`-o` rule: neither is a refusal.
pub fn need_target(in_place: bool, out: &Option<PathBuf>) -> Result<()> {
    if !in_place && out.is_none() {
        return Err(Error::Refused(
            "this would change a file: give -o OUT for a new archive pair, or --in-place to change the client folder (a backup is made)".to_string(),
        ));
    }
    Ok(())
}
