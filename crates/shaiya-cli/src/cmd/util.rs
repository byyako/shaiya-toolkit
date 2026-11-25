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
    if ctx.json {
        crate::output::print_json(&error_json(e));
    }
    if loose_refused() {
        return 2;
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

/// Prints each finding of the file checks as a warning (stderr, silenced by `--quiet`); never changes an exit code.
pub fn warn_findings(ctx: &Ctx, findings: &[shaiya_inspect::validate::Finding]) {
    for f in findings {
        warn(ctx, &f.line());
    }
}

/// The findings as the `warnings` array of a `--json` result.
pub fn findings_json(findings: &[shaiya_inspect::validate::Finding]) -> serde_json::Value {
    serde_json::to_value(findings).unwrap_or(serde_json::Value::Null)
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
    } else if let Some(stem) = what.strip_suffix('y') {
        format!("{stem}ies")
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
    refuse_loose(arg, &path)?;
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
    refuse_loose(arg, &path)?;
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

/// An argument as it should be printed in a command a person will paste (cmd.exe, PowerShell 5.1 and PowerShell 7).
///
/// - A bare `@name` is PowerShell's splatting syntax, so `@name`, anything with white space and anything with a shell character is quoted.
/// - Double quotes are the default: they work in all three shells. `%` stays literal there unless it spells a variable that is defined.
/// - `$` and a backtick are special inside PowerShell's double quotes, so those arguments use single quotes (`'` doubled), which PowerShell
///   keeps literal.
/// - A backslash at the end of a quoted argument is dropped (PowerShell 5.1 and 7 pass it to programs differently, and `C:\dir\` is
///   `C:\dir`); a drive root becomes `C:\.`.
pub fn shell_arg(arg: &str) -> String {
    let special = |c: char| c.is_whitespace() || "&|<>()^;,'`$%\"{}[]!#=".contains(c);
    if !(arg.is_empty() || arg.starts_with('@') || arg.contains(special)) {
        return arg.to_string();
    }
    let trimmed = arg.trim_end_matches(['\\', '/']);
    let a = if trimmed.is_empty() {
        arg.to_string() // only separators: keep as given
    } else if trimmed.len() == 2 && trimmed.ends_with(':') {
        format!("{trimmed}\\.") // a drive root
    } else {
        trimmed.to_string()
    };
    if a.contains(['$', '`']) {
        format!("'{}'", a.replace('\'', "''"))
    } else {
        format!("\"{}\"", a.replace('"', "\\\""))
    }
}

/// The CLIENT argument and `--journal-dir` as the commands that read journals need them.
pub fn client_args(client_arg: &str, journal_dir: Option<&Path>) -> String {
    match journal_dir {
        Some(d) => format!("{} --journal-dir {}", shell_arg(client_arg), shell_arg(&d.display().to_string())),
        None => shell_arg(client_arg),
    }
}

/// The command a person can run to undo the newest change.
pub fn undo_hint(client_arg: &str, journal_dir: Option<&Path>) -> String {
    format!("shaiya patch rollback {}", client_args(client_arg, journal_dir))
}

/// The command a person can run to list the changes made to a client.
pub fn history_hint(client_arg: &str, journal_dir: Option<&Path>) -> String {
    format!("shaiya patch history {}", client_args(client_arg, journal_dir))
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


/// Set when a command refused a loose client: the exit code is 2 (usage) then, not the 4 of a safety refusal.
static LOOSE_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the last refusal was "this is a loose folder" (exit 2).
pub fn loose_refused() -> bool {
    LOOSE_REFUSED.load(std::sync::atomic::Ordering::SeqCst)
}

/// The loose layout of `path` when it is a folder with no `data.sah` that passes the loose detection (a saved loose client, or a folder).
pub fn loose_layout(path: &Path) -> Result<Option<shaiya_archive::loose::LooseLayout>> {
    if !path.is_dir() {
        return Ok(None);
    }
    let has_sah = std::fs::read_dir(path).is_ok_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("data.sah")));
    if has_sah {
        return Ok(None);
    }
    shaiya_archive::loose::detect(path)
}

/// The refusal an archive command gives for a loose client (`arg` as the person typed it): exit 2.
pub fn loose_refusal(arg: &str) -> Error {
    LOOSE_REFUSED.store(true, std::sync::atomic::Ordering::SeqCst);
    Error::Refused(format!("{arg} is a loose folder, not an archive: the archive commands do not work on it (shaiya anim and shaiya info read its files)"))
}

/// Refuses `arg` (already resolved to `path`) when it is a loose client.
pub fn refuse_loose(arg: &str, path: &Path) -> Result<()> {
    match loose_layout(path)? {
        Some(_) => Err(loose_refusal(arg)),
        None => Ok(()),
    }
}

/// Where `--from` reads files: an archive pair, or a loose folder (a saved loose client `@name`, or a folder that passes the detection).
pub enum Store {
    Archive(Archive),
    Loose(shaiya_clientdata::LooseTree),
}

impl Store {
    /// Every file path (archive: the visible ones) with its size.
    pub fn visible(&self) -> Vec<(String, u64)> {
        match self {
            Store::Archive(a) => a.index().visible().into_iter().map(|f| (f.path, f.entry.length as u64)).collect(),
            Store::Loose(t) => t.files().iter().map(|f| (f.rel.clone(), f.len)).collect(),
        }
    }

    /// The size of the file, `None` when there is no such file.
    pub fn len(&self, path: &str) -> Option<u64> {
        match self {
            Store::Archive(a) => a.index().get(path).map(|e| e.length as u64),
            Store::Loose(t) => t.len(path),
        }
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self {
            Store::Archive(a) => a.read(path),
            Store::Loose(t) => t.read(path, u64::MAX, &shaiya_core::Job::silent()),
        }
    }
}

/// Opens the `--from` argument for reading files: like [`open_full`], or the files of a loose client (with the notes of what was left out).
pub fn open_store(ctx: &Ctx, arg: &str) -> Result<Store> {
    let path = resolve(ctx, arg)?;
    if let Some(layout) = loose_layout(&path)? {
        let tree = shaiya_clientdata::LooseTree::open(&layout, &shaiya_core::Job::silent())?;
        for n in tree.notes() {
            note(ctx, &format!("note: {n}"));
        }
        return Ok(Store::Loose(tree));
    }
    Ok(Store::Archive(Archive::open(&path)?))
}

/// [`client_dir`] for the commands that work on a client's archive pair (`archive`, `patch`, `clients health|clean`): a loose client is
/// refused (exit 2). `svmap` keeps [`client_dir`]: an extracted client folder is a normal `--client` there.
pub fn archive_client_dir(ctx: &Ctx, arg: &str) -> Result<PathBuf> {
    let p = resolve(ctx, arg)?;
    refuse_loose(arg, &p)?;
    client_dir(ctx, arg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_arg_quotes_for_cmd_and_powershell() {
        assert_eq!(shell_arg("plain"), "plain");
        assert_eq!(shell_arg(r"C:\Games\Shaiya"), r"C:\Games\Shaiya");
        assert_eq!(shell_arg("@us"), "\"@us\"");
        assert_eq!(shell_arg(r"C:\My Games\Shaiya"), r#""C:\My Games\Shaiya""#);
        assert_eq!(shell_arg("a&b"), "\"a&b\"");
        assert_eq!(shell_arg("100%"), "\"100%\"");
        // PowerShell expands $ and the backtick inside double quotes: single quotes, with ' doubled
        assert_eq!(shell_arg(r"C:\a $x b"), r"'C:\a $x b'");
        assert_eq!(shell_arg("it's $x"), "'it''s $x'");
        assert_eq!(shell_arg("a`b"), "'a`b'");
        // a trailing backslash would escape the closing quote in cmd and PowerShell 5.1: dropped; a drive root keeps a dot
        assert_eq!(shell_arg(r"C:\My Games\"), r#""C:\My Games""#);
        assert_eq!(shell_arg(r"C:\My Games\\"), r#""C:\My Games""#);
        assert_eq!(shell_arg(r"D:\"), r"D:\");
        assert_eq!(shell_arg(r"&:\"), r#""&:\.""#);
        assert_eq!(shell_arg(""), "\"\"");
    }

    #[test]
    fn hints_name_the_client_and_keep_the_journal_folder() {
        assert_eq!(undo_hint("@us", None), "shaiya patch rollback \"@us\"");
        assert_eq!(history_hint(r"C:\My Games\Shaiya", None), r#"shaiya patch history "C:\My Games\Shaiya""#);
        let j = Path::new(r"D:\my journals");
        assert_eq!(undo_hint("@us", Some(j)), r#"shaiya patch rollback "@us" --journal-dir "D:\my journals""#);
        assert_eq!(history_hint("@us", Some(Path::new("J"))), "shaiya patch history \"@us\" --journal-dir J");
    }
}
