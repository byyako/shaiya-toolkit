//! `shaiya archive ...`: list, stats, check, index, extract, cat, add, replace, remove, compact, diff, pack, swap.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use shaiya_archive::check::{check_archive, check_index, CheckOptions, Severity};
use shaiya_archive::client::ClientFiles;
use shaiya_archive::diff::{diff, summarize, Change, DiffOptions, Side};
use shaiya_archive::edit::{self, EditPlan, EditReport, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::extract::{self, ExtractOptions};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_archive::pack::{self, PackOptions};
use shaiya_archive::{compact, swap, Archive, FileRef, Index};
use shaiya_core::fs::Existing;
use shaiya_core::glob::{parse_list, Filter};
use shaiya_core::source::{ByteSource, FolderSource};
use shaiya_core::text::parse_size;
use shaiya_core::time::parse_utc;
use shaiya_core::{Error, Job, Result};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// List the files of an archive
    ///
    /// Shows the files the game can see (when two entries have the same name in one folder, the later one wins and the earlier is
    /// "shadowed"; add --include-shadowed to see both). Patterns are globs, case-insensitive: `*` and `?` stay inside a path segment,
    /// `**` crosses segments, a pattern without `/` matches the file name in any folder, a trailing `/` means everything under that folder.
    ///
    /// Example: shaiya archive list "@us" "*.SData" --long
    List(ListArgs),
    /// Counts, sizes and unused space of an archive
    ///
    /// Example: shaiya archive stats data.orig.sah
    Stats(StatsArgs),
    /// Check an archive for problems (entries outside data.saf, overlaps, shadowed names, unreadable data)
    ///
    /// Exit 0 when there are no problems, 1 when there are. --deep reads every file once.
    ///
    /// Example: shaiya archive check "@us" --deep
    Check(CheckArgs),
    /// Parse data.sah, write it back in memory and compare with the file
    ///
    /// Exit 0 when the rewrite is byte-identical, 1 when it is not.
    ///
    /// Example: shaiya archive index data.sah
    Index(IndexArgs),
    /// Extract files from an archive into a folder
    ///
    /// Patterns are globs (case-insensitive; `*` and `?` stay inside a path segment, `**` crosses segments, a pattern without `/`
    /// matches the file name in any folder). Of entries with the same name in one folder the later one is what the game loads; earlier
    /// ("shadowed") ones are skipped unless --include-shadowed, and then get a `~shadowed-N` name. Names Windows cannot hold are changed
    /// (a `:` becomes `%3A`), never written outside the output folder.
    ///
    /// Example: shaiya archive extract "@us" "*.dds" -o out/
    Extract(ExtractArgs),
    /// Write one file of an archive to a file or to standard output
    ///
    /// Example: shaiya archive cat "@us" Item/Item.SData -o Item.SData
    Cat(CatArgs),
    /// Add files to an archive (in the client folder with a backup, or into a new archive pair)
    ///
    /// A file goes to PREFIX/<file name>; a folder's files go to PREFIX/<path inside the folder>. The change is appended to data.saf and
    /// undone with `shaiya patch rollback`.
    ///
    /// Example: shaiya archive add "@us" mods/ --in-place --replace
    Add(AddArgs),
    /// Replace files that are already in an archive
    ///
    /// Example: shaiya archive replace "@us" fixed/Item.SData --at Item --in-place
    Replace(ReplaceArgs),
    /// Remove files from an archive
    ///
    /// Example: shaiya archive remove "@us" test/junk.eft --in-place
    Remove(RemoveArgs),
    /// Rewrite data.saf without unused space (and drop shadowed entries and the free list)
    ///
    /// With --in-place this replaces data.saf and the index and makes no journal: the compaction itself cannot be undone (data.sah.bak
    /// keeps only the old index) and earlier changes can no longer be rolled back. -o writes a compacted copy and leaves the client alone.
    ///
    /// Example: shaiya archive compact "@us" --in-place
    Compact(CompactArgs),
    /// Compare two archives
    ///
    /// Exit 0 when they are the same, 1 when they differ.
    ///
    /// Example: shaiya archive diff data.orig.sah data.sah --quick --summary
    Diff(DiffArgs),
    /// Build a new archive pair from a folder
    ///
    /// Example: shaiya archive pack files/ -o build/
    Pack(PackArgs),
    /// Put another archive pair in place of a client's own (the old pair is kept)
    ///
    /// Example: shaiya archive swap "@us" D:\backups\us-stock
    Swap(SwapArgs),
}

/// Pattern options shared by commands that choose files.
#[derive(clap::Args, Clone)]
pub struct Select {
    /// Leave out files matching this pattern (repeatable)
    #[arg(long, value_name = "PAT")]
    pub exclude: Vec<String>,
    /// Read paths to include from a file (one per line; the first tab-separated column; `#` lines and a `path` header are skipped)
    #[arg(long, value_name = "FILE")]
    pub list: Option<PathBuf>,
    /// Only files of at least this size (1500, 64k, 2MiB, ...)
    #[arg(long, value_name = "N")]
    pub min_size: Option<String>,
    /// Only files of at most this size
    #[arg(long, value_name = "N")]
    pub max_size: Option<String>,
    /// Include entries a later entry of the same name hides
    #[arg(long)]
    pub include_shadowed: bool,
}

fn make_filter(patterns: &[String], s: &Select) -> Result<Filter> {
    let mut include = patterns.to_vec();
    if let Some(f) = &s.list {
        include.extend(parse_list(&std::fs::read_to_string(f).map_err(|e| Error::io(f, e))?));
    }
    Filter::from_args(&include, &s.exclude, s.min_size.as_deref(), s.max_size.as_deref())
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum SortBy {
    Name,
    Size,
    Offset,
    Version,
}

#[derive(clap::Args)]
pub struct ListArgs {
    /// A client folder, a .sah file, or @name
    pub archive: String,
    /// Only files matching one of these patterns
    pub patterns: Vec<String>,
    #[command(flatten)]
    pub select: Select,
    /// Show offset and pack time too
    #[arg(long)]
    pub long: bool,
    /// Group files under their folders
    #[arg(long)]
    pub tree: bool,
    /// Show folders only (with file counts and sizes)
    #[arg(long)]
    pub folders: bool,
    /// Print only the paths
    #[arg(long)]
    pub names_only: bool,
    /// Sort order (default: the order in the index)
    #[arg(long, value_enum, value_name = "KEY")]
    pub sort: Option<SortBy>,
    /// Reverse the sort order
    #[arg(long)]
    pub reverse: bool,
}

#[derive(clap::Args)]
pub struct StatsArgs {
    pub archive: String,
    /// Rows per table
    #[arg(long, default_value_t = 15, value_name = "N")]
    pub top: usize,
}

#[derive(clap::Args)]
pub struct CheckArgs {
    pub archive: String,
    /// Read every visible file once and report what cannot be read
    #[arg(long)]
    pub deep: bool,
    /// Count warnings as problems
    #[arg(long)]
    pub strict: bool,
    /// How many of the largest gaps to print
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub gaps: usize,
}

#[derive(clap::Args)]
pub struct IndexArgs {
    /// A data.sah file (or a client folder)
    pub sah: String,
}

#[derive(clap::Args)]
pub struct ExtractArgs {
    pub archive: String,
    pub patterns: Vec<String>,
    /// The folder to extract into (created)
    #[arg(short, long, value_name = "DIR")]
    pub out: PathBuf,
    #[command(flatten)]
    pub select: Select,
    /// Overwrite files that already exist
    #[arg(long, conflicts_with = "skip_existing")]
    pub force: bool,
    /// Leave files that already exist
    #[arg(long)]
    pub skip_existing: bool,
    /// Do not set file times from the pack time
    #[arg(long)]
    pub no_times: bool,
    /// Show what would be extracted and write nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct CatArgs {
    pub archive: String,
    /// The path of the file inside the archive
    pub path: String,
    /// Write to this file (required when standard output is a terminal)
    #[arg(short, long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Replace the output file if it exists
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args, Clone)]
pub struct Target {
    /// Change the client folder's own data.sah/data.saf (journaled and backed up, so it can usually be rolled back; `compact` is the exception)
    #[arg(long)]
    pub in_place: bool,
    /// Write a new archive pair instead: a .sah path (the .saf gets the same stem) or a folder
    #[arg(short, long, value_name = "OUT", conflicts_with = "in_place")]
    pub out: Option<PathBuf>,
    /// Show what would change and write nothing
    #[arg(long)]
    pub dry_run: bool,
    /// Replace existing output files
    #[arg(long)]
    pub force: bool,
    /// Make no journal (the change cannot be rolled back; data.sah.bak is still written)
    #[arg(long)]
    pub no_journal: bool,
    /// Where journals go (default: shaiya-toolkit-backups in the client folder)
    #[arg(long, value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct AddArgs {
    pub archive: String,
    /// Files and folders to add
    #[arg(required = true, value_name = "SRC")]
    pub src: Vec<PathBuf>,
    /// Archive folder to put them under
    #[arg(long, value_name = "PREFIX", default_value = "")]
    pub at: String,
    /// The exact archive path of a single file
    #[arg(long = "as", value_name = "PATH")]
    pub as_path: Option<String>,
    /// Replace files that already exist
    #[arg(long)]
    pub replace: bool,
    /// Entry time: Unix seconds, 2026-11-15 or 2026-11-15T12:00:00Z (default: each file's modification time)
    #[arg(long, value_name = "T")]
    pub timestamp: Option<String>,
    #[command(flatten)]
    pub target: Target,
}

#[derive(clap::Args)]
pub struct ReplaceArgs {
    pub archive: String,
    #[arg(required = true, value_name = "SRC")]
    pub src: Vec<PathBuf>,
    #[arg(long, value_name = "PREFIX", default_value = "")]
    pub at: String,
    #[arg(long = "as", value_name = "PATH")]
    pub as_path: Option<String>,
    /// Add files that are not in the archive yet
    #[arg(long)]
    pub add_missing: bool,
    #[arg(long, value_name = "T")]
    pub timestamp: Option<String>,
    #[command(flatten)]
    pub target: Target,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    pub archive: String,
    /// Archive paths to remove
    pub paths: Vec<String>,
    /// Read paths from a file (one per line)
    #[arg(long, value_name = "FILE")]
    pub list: Option<PathBuf>,
    /// Do not stop at paths that are not in the archive
    #[arg(long)]
    pub ignore_missing: bool,
    #[command(flatten)]
    pub target: Target,
}

#[derive(clap::Args)]
pub struct CompactArgs {
    pub archive: String,
    #[command(flatten)]
    pub target: Target,
    /// Do nothing when fewer than N bytes are unused
    #[arg(long, value_name = "N", default_value = "0")]
    pub min_dead: String,
}

#[derive(clap::Args)]
pub struct DiffArgs {
    pub old: String,
    pub new: String,
    pub patterns: Vec<String>,
    /// Compare the indexes only (equal length and pack time count as the same)
    #[arg(long)]
    pub quick: bool,
    #[arg(long)]
    pub include_shadowed: bool,
    /// Only these kinds of difference: added, removed, different, unknown
    #[arg(long, value_delimiter = ',', value_name = "KINDS")]
    pub only: Vec<String>,
    /// Also list files that are the same
    #[arg(long)]
    pub show_same: bool,
    /// Print only the counts
    #[arg(long)]
    pub summary: bool,
}

#[derive(clap::Args)]
pub struct PackArgs {
    /// The folder to pack
    pub folder: PathBuf,
    /// A .sah path or a folder
    #[arg(short, long, value_name = "OUT")]
    pub out: PathBuf,
    /// SAH version to write (0 or 1)
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub sah_version: i32,
    #[arg(long, value_name = "T")]
    pub timestamp: Option<String>,
    /// Keep Thumbs.db, desktop.ini and similar files
    #[arg(long)]
    pub include_all: bool,
    #[arg(long, value_name = "PAT")]
    pub exclude: Vec<String>,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct SwapArgs {
    /// The client folder (or @name)
    pub client: String,
    /// A folder with data.sah and data.saf, or a .sah file
    pub other: String,
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &Job) -> i32 {
    let r = match cmd {
        Cmd::List(a) => list(ctx, a),
        Cmd::Stats(a) => stats(ctx, a),
        Cmd::Check(a) => check(ctx, a, job),
        Cmd::Index(a) => index(ctx, a),
        Cmd::Extract(a) => extract_cmd(ctx, a, job),
        Cmd::Cat(a) => cat(ctx, a, job),
        Cmd::Add(a) => edit_cmd(ctx, Kind::Add(a.replace), &a.archive, &a.src, &a.at, a.as_path.as_deref(), a.timestamp.as_deref(), &[], false, &a.target, job),
        Cmd::Replace(a) => edit_cmd(ctx, Kind::Replace(a.add_missing), &a.archive, &a.src, &a.at, a.as_path.as_deref(), a.timestamp.as_deref(), &[], false, &a.target, job),
        Cmd::Remove(a) => remove_cmd(ctx, a, job),
        Cmd::Compact(a) => compact_cmd(ctx, a, job),
        Cmd::Diff(a) => diff_cmd(ctx, a, job),
        Cmd::Pack(a) => pack_cmd(ctx, a, job),
        Cmd::Swap(a) => swap_cmd(ctx, a, job),
    };
    match r {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
}

// ---------------------------------------------------------------------------------------------------------- list

fn folder_rows(rows: &[&FileRef]) -> std::collections::BTreeMap<(String, String), (usize, u64)> {
    let mut m: std::collections::BTreeMap<(String, String), (usize, u64)> = std::collections::BTreeMap::new();
    for f in rows {
        let segs: Vec<&str> = f.path.split('/').collect();
        for n in 1..segs.len() {
            let p = segs[..n].join("/");
            let e = m.entry((p.to_lowercase(), p)).or_insert((0, 0));
            e.0 += 1;
            e.1 += f.entry.length as u64;
        }
    }
    m
}

fn list(ctx: &Ctx, a: &ListArgs) -> Result<i32> {
    let o = open_read(ctx, &a.archive)?;
    let filter = make_filter(&a.patterns, &a.select)?;
    let idx = o.index();
    let all = idx.entries();
    let shadowed_hidden = if a.select.include_shadowed { 0 } else { all.iter().filter(|f| f.shadowed && filter.matches(&f.path, f.entry.length as u64)).count() };
    let mut rows: Vec<&FileRef> = all.iter().filter(|f| (a.select.include_shadowed || !f.shadowed) && filter.matches(&f.path, f.entry.length as u64)).collect();
    match a.sort {
        Some(SortBy::Name) => rows.sort_by_key(|f| f.path.to_lowercase()),
        Some(SortBy::Size) => rows.sort_by_key(|f| f.entry.length),
        Some(SortBy::Offset) => rows.sort_by_key(|f| f.entry.offset),
        Some(SortBy::Version) => rows.sort_by_key(|f| f.entry.version),
        None => {}
    }
    if a.reverse {
        rows.reverse();
    }
    let total: u64 = rows.iter().map(|f| f.entry.length as u64).sum();
    let folders = folder_rows(&rows);
    if ctx.json {
        let entries: Vec<Value> = rows
            .iter()
            .map(|f| {
                let mut v = json!({"path": f.path, "size": f.entry.length, "offset": f.entry.offset, "version": f.entry.version, "version_utc": iso_version(f.entry.version), "shadowed": f.shadowed});
                if encoding_invalid(&f.raw_path) {
                    v["raw_hex"] = json!(shaiya_core::hash::to_hex(&f.raw_path));
                }
                v
            })
            .collect();
        let mut v = json!({
            "archive": {"sah": o.sah().display().to_string(), "saf": o.saf_path().map(|p| p.display().to_string()), "sah_version": idx.version, "files": all.len()},
            "entries": entries, "shown": rows.len(), "shadowed_hidden": shadowed_hidden, "total_size": total
        });
        if a.tree || a.folders {
            v["folders"] = json!(folders.iter().map(|((_, p), (n, b))| json!({"path": p, "files": n, "bytes": b})).collect::<Vec<_>>());
        }
        print_json(&v);
    } else {
        let mut w = out();
        if a.folders || a.tree {
            let mut printed: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut sorted: Vec<&&FileRef> = rows.iter().collect();
            sorted.sort_by_key(|f| f.path.to_lowercase());
            let mut header = |w: &mut dyn Write, segs: &[&str]| {
                for n in 1..segs.len() {
                    let p = segs[..n].join("/");
                    if printed.insert(p.to_lowercase()) {
                        let (files, bytes) = folders.get(&(p.to_lowercase(), p.clone())).copied().unwrap_or((0, 0));
                        let label = format!("{}{}/", "  ".repeat(n - 1), segs[n - 1]);
                        outln!(w, "{label:<34}{:>14} {:>10}", count(files, "file"), size(bytes));
                    }
                }
            };
            if a.folders {
                for f in &sorted {
                    let segs: Vec<&str> = f.path.split('/').collect();
                    header(&mut w, &segs);
                }
            } else {
                for f in &sorted {
                    let segs: Vec<&str> = f.path.split('/').collect();
                    header(&mut w, &segs);
                    outln!(w, "{}{:>12}  {}", "  ".repeat(segs.len() - 1), num(f.entry.length as u64), segs[segs.len() - 1]);
                }
            }
        } else {
            for f in &rows {
                if a.names_only {
                    outln!(w, "{}", f.path);
                } else if a.long {
                    outln!(w, "0x{:010x} {:>11}  {:<20}  {}{}", f.entry.offset, num(f.entry.length as u64), iso_version(f.entry.version).unwrap_or_else(|| "-".into()), f.path, if f.shadowed { "  (shadowed)" } else { "" });
                } else {
                    outln!(w, "{:>12}  {}{}", num(f.entry.length as u64), f.path, if f.shadowed { "  (shadowed)" } else { "" });
                }
            }
        }
        let _ = w.flush();
        let mut s = format!("{} of {}, {}", num(rows.len() as u64), count(all.len(), "file"), size(total));
        if shadowed_hidden > 0 {
            s.push_str(&format!(" ({shadowed_hidden} shadowed hidden; --include-shadowed shows them)"));
        }
        note(ctx, &s);
    }
    Ok(0)
}

fn encoding_invalid(raw: &[u8]) -> bool {
    shaiya_core::text::Encoding::Cp949.decode(raw).1
}

// ---------------------------------------------------------------------------------------------------------- stats

fn stats(ctx: &Ctx, a: &StatsArgs) -> Result<i32> {
    let o = open_read(ctx, &a.archive)?;
    let idx = o.index();
    let report = check_index(idx, o.saf_len());
    let all = idx.entries();
    let visible: Vec<&FileRef> = all.iter().filter(|f| !f.shadowed).collect();
    let mut by_folder: std::collections::BTreeMap<String, (usize, u64)> = std::collections::BTreeMap::new();
    let mut by_ext: std::collections::BTreeMap<String, (usize, u64)> = std::collections::BTreeMap::new();
    for f in &visible {
        let top = f.path.split_once('/').map(|(t, _)| t.to_string()).unwrap_or_else(|| "(root)".to_string());
        let e = by_folder.entry(top).or_insert((0, 0));
        e.0 += 1;
        e.1 += f.entry.length as u64;
        let name = f.path.rsplit('/').next().unwrap_or("");
        let ext = name.rsplit_once('.').map(|(_, e)| format!(".{}", e.to_lowercase())).unwrap_or_else(|| "(none)".to_string());
        let e = by_ext.entry(ext).or_insert((0, 0));
        e.0 += 1;
        e.1 += f.entry.length as u64;
    }
    let top = |m: std::collections::BTreeMap<String, (usize, u64)>| {
        let mut v: Vec<(String, usize, u64)> = m.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
        v.sort_by(|x, y| y.2.cmp(&x.2).then(x.0.cmp(&y.0)));
        v.truncate(a.top);
        v
    };
    let (folders, exts) = (top(by_folder), top(by_ext));
    let l = &report.layout;
    if ctx.json {
        print_json(&json!({
            "sah_version": idx.version, "files": all.len(), "visible": visible.len(), "shadowed": all.len() - visible.len(), "dirs": idx.dir_count(),
            "free_entries": idx.free.len(), "trailer_hex": shaiya_core::hash::to_hex(&idx.trailer), "data_end": l.data_end, "saf_len": l.saf_len,
            "live_bytes": l.live_bytes, "dead_bytes": l.dead_bytes, "zero_length": l.zero_length,
            "by_folder": folders.iter().map(|(n, f, b)| json!({"name": n, "files": f, "bytes": b})).collect::<Vec<_>>(),
            "by_extension": exts.iter().map(|(n, f, b)| json!({"ext": n, "files": f, "bytes": b})).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    let mut w = out();
    let sah_name = file_name(o.sah());
    outln!(
        w,
        "{sah_name}  SAH version {}, {} ({} shadowed) in {}, {}, trailer {}",
        idx.version,
        count(all.len(), "file"),
        all.len() - visible.len(),
        count(idx.dir_count(), "folder"),
        if idx.free.is_empty() { "no free list".to_string() } else { count(idx.free.len(), "free-list entry") },
        shaiya_core::hash::to_hex(&idx.trailer)
    );
    match (l.saf_len, l.dead_bytes) {
        (Some(n), Some(dead)) => {
            let pct = if n > 0 { dead as f64 * 100.0 / n as f64 } else { 0.0 };
            let saf_name = o.saf_path().map(|p| file_name(&p)).unwrap_or_default();
            outln!(w, "{saf_name}  {}, {} live, {} unused ({pct:.1} %) in {}", count(n as usize, "byte"), num(l.live_bytes), num(dead), count(l.gap_count, "gap"));
        }
        _ => outln!(w, "data.saf  not found; data ends at {}", num(l.data_end)),
    }
    outln!(w);
    outln!(w, "By folder (largest {})", a.top);
    for (n, f, b) in &folders {
        outln!(w, "  {n:<28}{:>15} {:>10}", count(*f, "file"), size(*b));
    }
    outln!(w);
    outln!(w, "By extension (largest {})", a.top);
    for (n, f, b) in &exts {
        outln!(w, "  {n:<28}{:>15} {:>10}", count(*f, "file"), size(*b));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- check

fn check(ctx: &Ctx, a: &CheckArgs, job: &Job) -> Result<i32> {
    let o = open_read(ctx, &a.archive)?;
    let report = match o.archive() {
        Some(ar) => check_archive(ar, &CheckOptions { deep: a.deep }, job)?,
        None => check_index(o.index(), None),
    };
    let problems = report.problems() + if a.strict { report.warnings() } else { 0 };
    let ok = problems == 0;
    if ctx.json {
        let mut v = serde_json::to_value(&report).unwrap_or(Value::Null);
        v["ok"] = json!(ok);
        v["problems"] = json!(report.problems());
        v["warnings"] = json!(report.warnings());
        print_json(&v);
        return Ok(if ok { 0 } else { 1 });
    }
    let l = &report.layout;
    let mut w = out();
    let sah_name = file_name(o.sah());
    outln!(
        w,
        "{sah_name}  version {}, {}, {} shadowed, {}, {}",
        report.sah_version,
        count(report.files, "file"),
        report.files - report.visible,
        count(report.dirs, "folder"),
        if report.free_entries == 0 { "no free list".to_string() } else { count(report.free_entries as usize, "free-list entry") }
    );
    if let (Some(n), Some(dead), Some(p)) = (l.saf_len, l.dead_bytes, o.saf_path()) {
        let referenced = if dead == 0 { "all referenced".to_string() } else { format!("{} unused", num(dead)) };
        outln!(w, "{}  {}, {referenced}, {}, {}", file_name(&p), count(n as usize, "byte"), count(l.gap_count, "gap"), count(l.overlaps, "overlap"));
    }
    for f in report.findings.iter().filter(|f| f.severity != Severity::Info) {
        let sev = if f.severity == Severity::Problem { "problem" } else { "warning" };
        match &f.path {
            Some(p) => outln!(w, "{sev:<9} {}: {p}: {}", f.code, f.message),
            None => outln!(w, "{sev:<9} {}: {}", f.code, f.message),
        }
    }
    for f in report.findings.iter().filter(|f| f.severity == Severity::Info) {
        match &f.path {
            Some(p) => outln!(w, "info      {}: {p}: {}", f.code, f.message),
            None => outln!(w, "info      {}: {}", f.code, f.message),
        }
    }
    for (off, bytes) in l.largest_gaps.iter().take(a.gaps) {
        outln!(w, "gap       {} at offset {} (0x{off:x})", count(*bytes as usize, "byte"), num(*off));
    }
    outln!(w, "{}: {}, {}", if ok { "OK" } else { "FAILED" }, count(report.problems(), "problem"), count(report.warnings(), "warning"));
    Ok(if ok { 0 } else { 1 })
}

// ---------------------------------------------------------------------------------------------------------- index

fn index(ctx: &Ctx, a: &IndexArgs) -> Result<i32> {
    let path = resolve(ctx, &a.sah)?;
    let (sah, idx) = Archive::open_index(&path)?;
    let original = std::fs::read(&sah).map_err(|e| Error::io(&sah, e))?;
    let rewritten = idx.to_bytes();
    let first = original.iter().zip(&rewritten).position(|(x, y)| x != y).or_else(|| (original.len() != rewritten.len()).then(|| original.len().min(rewritten.len())));
    let identical = first.is_none();
    if ctx.json {
        print_json(&json!({
            "sah": sah.display().to_string(), "sah_version": idx.version, "files": idx.file_count(), "free_entries": idx.free.len(),
            "trailer_hex": shaiya_core::hash::to_hex(&idx.trailer), "identical": identical, "first_difference": first
        }));
    } else {
        let mut w = out();
        outln!(
            w,
            "{}: version {}, {}, {}, trailer {} bytes ({}); rewrite identical: {}{}",
            file_name(&sah),
            idx.version,
            count(idx.file_count(), "file"),
            count(idx.free.len(), "free-list entry"),
            idx.trailer.len(),
            shaiya_core::hash::to_hex(&idx.trailer),
            if identical { "yes" } else { "no" },
            first.map(|p| format!(" (first difference at byte {})", num(p as u64))).unwrap_or_default()
        );
    }
    Ok(if identical { 0 } else { 1 })
}

// ---------------------------------------------------------------------------------------------------------- extract

fn extract_cmd(ctx: &Ctx, a: &ExtractArgs, job: &Job) -> Result<i32> {
    let archive = open_full(ctx, &a.archive)?;
    let filter = make_filter(&a.patterns, &a.select)?;
    let opts = ExtractOptions {
        filter,
        include_shadowed: a.select.include_shadowed,
        existing: if a.force { Existing::Overwrite } else if a.skip_existing { Existing::Skip } else { Existing::Refuse },
        keep_times: !a.no_times,
    };
    if a.dry_run {
        let plan = extract::plan(archive.index(), &a.out, &opts)?;
        let bytes: u64 = plan.iter().map(|p| p.entry.length as u64).sum();
        let exist = plan.iter().filter(|p| p.exists).count();
        if ctx.json {
            print_json(&json!({"dry_run": true, "out": a.out.display().to_string(), "files": plan.len(), "bytes": bytes, "existing": exist,
                "paths": plan.iter().map(|p| json!({"path": p.path, "target": p.target.display().to_string(), "exists": p.exists, "escaped": p.escaped, "shadowed": p.shadowed})).collect::<Vec<_>>()}));
        } else {
            let mut w = out();
            for p in &plan {
                outln!(w, "would extract {} -> {}{}", p.path, p.target.display(), if p.exists { "  (exists)" } else { "" });
            }
            outln!(w, "would extract {} ({}) to {}", count(plan.len(), "file"), size(bytes), a.out.display());
        }
        if exist > 0 && opts.existing == Existing::Refuse {
            note(ctx, &format!("{exist} of these files already exist; the real run would refuse (use --force or --skip-existing)"));
            return Ok(4);
        }
        return Ok(0);
    }
    let r = extract::extract(&archive, &a.out, &opts, job)?;
    if ctx.json {
        let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
        v["out"] = json!(a.out.display().to_string());
        print_json(&v);
    } else {
        let mut w = out();
        outln!(w, "extracted {} ({}) to {}", count(r.written, "file"), size(r.bytes), a.out.display());
        let _ = w.flush();
        if !r.skipped_existing.is_empty() {
            note(ctx, &format!("note: {} left alone because they exist already", count(r.skipped_existing.len(), "file")));
        }
        if r.shadowed_skipped > 0 {
            note(ctx, &format!("note: {} hidden by later entries of the same name not extracted (--include-shadowed)", count(r.shadowed_skipped, "entry")));
        }
        if !r.escaped.is_empty() {
            note(ctx, &format!("note: {} got names that Windows accepts (a character such as ':' became '%3A')", count(r.escaped.len(), "file")));
            if ctx.verbose > 0 {
                for (from, to) in &r.escaped {
                    note(ctx, &format!("  {from} -> {to}"));
                }
            }
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- cat

fn cat(ctx: &Ctx, a: &CatArgs, job: &Job) -> Result<i32> {
    let archive = open_full(ctx, &a.archive)?;
    let entry = archive
        .index()
        .get(&a.path)
        .ok_or_else(|| Error::io(&a.path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive")))?
        .clone();
    match &a.out {
        Some(p) => {
            let existing = if a.force { Existing::Overwrite } else { Existing::Refuse };
            if p.exists() && existing == Existing::Refuse {
                return Err(Error::Refused(format!("{} already exists; use --force to replace it", p.display())));
            }
            let mut r = archive.reader(&entry)?;
            job.total(Some(entry.length as u64), shaiya_core::Unit::Bytes);
            let n = shaiya_core::fs::write_atomic_from_with(p, &mut r, shaiya_core::fs::Sync::Yes, job)?;
            if ctx.json {
                print_json(&json!({"path": a.path, "size": n, "out": p.display().to_string()}));
            } else {
                note(ctx, &format!("wrote {} ({}) to {}", a.path, size(n), p.display()));
            }
            Ok(0)
        }
        None => {
            if std::io::stdout().is_terminal() {
                return Err(Error::Refused("refusing to write file data to a terminal; use -o FILE or redirect the output".to_string()));
            }
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            match archive.copy_entry(&entry, &mut lock, job) {
                Ok(_) => {}
                Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(e),
            }
            let _ = lock.flush();
            Ok(0)
        }
    }
}

// ---------------------------------------------------------------------------------------------------------- add / replace / remove

#[derive(Clone, Copy)]
enum Kind {
    /// `true` = replace existing files
    Add(bool),
    /// `true` = add missing files
    Replace(bool),
    Remove,
}

fn join_prefix(prefix: &str, rel: &str) -> String {
    let p = prefix.trim_matches(['/', '\\']);
    if p.is_empty() { rel.to_string() } else { format!("{p}/{rel}") }
}

fn mtime(p: &std::path::Path) -> i32 {
    std::fs::metadata(p)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs().min(i32::MAX as u64) as i32)
        .unwrap_or(0)
}

fn build_puts(kind: Kind, src: &[PathBuf], at: &str, as_path: Option<&str>, timestamp: Option<&str>) -> Result<Vec<Put>> {
    let ts = timestamp.map(parse_utc).transpose()?.map(|t| t.clamp(0, i32::MAX as i64) as i32);
    let mode = match kind {
        Kind::Add(replace) => if replace { PutMode::AddOrReplace } else { PutMode::AddOnly },
        Kind::Replace(add_missing) => if add_missing { PutMode::AddOrReplace } else { PutMode::ReplaceOnly },
        Kind::Remove => return Ok(Vec::new()),
    };
    let fs_src = FolderSource::new(src, &Filter::default())?;
    for s in fs_src.skipped() {
        eprintln!("warning: skipped {s}");
    }
    let names = fs_src.names();
    if let Some(p) = as_path {
        if src.len() != 1 || src[0].is_dir() || names.len() != 1 {
            return Err(Error::Refused("--as names the archive path of exactly one file".to_string()));
        }
        let path = fs_src.file_path(&names[0]).expect("a file on disk");
        return Ok(vec![Put { path: p.to_string(), source: Source::File(path.clone()), version: ts.unwrap_or_else(|| mtime(&path)), mode }]);
    }
    let mut puts = Vec::new();
    for n in names {
        let path = fs_src.file_path(&n).expect("a file on disk");
        puts.push(Put { path: join_prefix(at, &n), source: Source::File(path.clone()), version: ts.unwrap_or_else(|| mtime(&path)), mode });
    }
    Ok(puts)
}

fn remove_cmd(ctx: &Ctx, a: &RemoveArgs, job: &Job) -> Result<i32> {
    let mut paths = a.paths.clone();
    if let Some(f) = &a.list {
        paths.extend(parse_list(&std::fs::read_to_string(f).map_err(|e| Error::io(f, e))?));
    }
    if paths.is_empty() {
        return Err(Error::Refused("nothing to remove: give paths or --list FILE".to_string()));
    }
    edit_cmd(ctx, Kind::Remove, &a.archive, &[], "", None, None, &paths, a.ignore_missing, &a.target, job)
}

/// The file checks for the files an add or replace would put in; the copy already in the archive (when it opens and has the path)
/// is what an SData file is compared with. Findings only warn.
fn put_findings(ctx: &Ctx, archive: &str, puts: &[Put]) -> Vec<shaiya_inspect::validate::Finding> {
    use shaiya_inspect::validate::{check_source, is_sdata_name};
    let existing = if puts.iter().any(|p| is_sdata_name(&p.path)) { open_full(ctx, archive).ok() } else { None };
    let mut out = Vec::new();
    for p in puts {
        let copy = existing.as_ref().filter(|_| is_sdata_name(&p.path)).and_then(|a| a.read(&p.path).ok());
        out.extend(check_source(&p.path, &p.source, copy.as_deref()));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn edit_cmd(ctx: &Ctx, kind: Kind, archive: &str, src: &[PathBuf], at: &str, as_path: Option<&str>, timestamp: Option<&str>, removes: &[String], ignore_missing: bool, t: &Target, job: &Job) -> Result<i32> {
    need_target(t.in_place, &t.out)?;
    let puts = build_puts(kind, src, at, as_path, timestamp)?;
    let findings = put_findings(ctx, archive, &puts);
    warn_findings(ctx, &findings);
    let label = match kind {
        Kind::Remove => format!("archive edit: remove {}", count(removes.len(), "file")),
        _ => format!("archive edit: {}", count(puts.len(), "file")),
    };
    let plan = EditPlan { puts, removes: removes.to_vec(), ignore_missing, loose: Vec::new(), label };
    if t.in_place {
        let dir = client_dir(ctx, archive)?;
        let files = prepare_write(ctx, &dir, !t.dry_run)?;
        if t.dry_run {
            let idx = Index::read(&files.sah)?;
            let r = edit::preview(&idx, &plan)?;
            print_edit(ctx, &r, true, None, archive, t.journal_dir.as_deref(), &findings);
            return Ok(0);
        }
        if t.no_journal {
            warn(ctx, "no journal: this change cannot be rolled back with `shaiya patch rollback` (data.sah.bak keeps the old index)");
        }
        let root = journal_root(&dir, t.journal_dir.as_deref());
        let opts = InPlaceOptions { journal: if t.no_journal { JournalMode::None } else { JournalMode::In(root) }, legacy_bak: true, check_in_use: false };
        let r = edit::apply_in_place(&dir, &plan, &opts, job)?;
        print_edit(ctx, &r, false, None, archive, t.journal_dir.as_deref(), &findings);
        return Ok(0);
    }
    let out = t.out.as_ref().expect("checked by need_target");
    let a = open_full(ctx, archive)?;
    let sah = out_sah(out)?;
    if t.dry_run {
        let r = edit::preview(a.index(), &plan)?;
        print_edit(ctx, &r, true, Some(&sah), archive, t.journal_dir.as_deref(), &findings);
        return Ok(0);
    }
    let r = edit::write_new(&a, &plan, &sah, if t.force { Existing::Overwrite } else { Existing::Refuse }, job)?;
    print_edit(ctx, &r, false, Some(&sah), archive, t.journal_dir.as_deref(), &findings);
    Ok(0)
}

fn print_edit(ctx: &Ctx, r: &EditReport, dry: bool, out: Option<&std::path::Path>, client_arg: &str, journal_dir: Option<&std::path::Path>, findings: &[shaiya_inspect::validate::Finding]) {
    if ctx.json {
        let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
        v["warnings"] = findings_json(findings);
        if dry {
            v["dry_run"] = json!(true);
        }
        if let Some(o) = out {
            v["out_sah"] = json!(o.display().to_string());
            v["out_saf"] = json!(o.with_extension("saf").display().to_string());
        }
        print_json(&v);
        return;
    }
    let mut w = out_writer();
    let (verb_add, verb_rep, verb_rm) = if dry { ("would add", "replace", "remove") } else { ("added", "replaced", "removed") };
    let mut head = format!("{verb_add} {}, {verb_rep} {}, {verb_rm} {}", r.added.len(), r.replaced.len(), r.removed.len());
    if !r.missing.is_empty() {
        head.push_str(&format!(" ({} not present)", r.missing.len()));
    }
    let bytes = size(r.bytes_appended);
    head.push_str(&match (dry, out) {
        (true, None) => format!(" ({bytes} would be appended to data.saf)"),
        (false, None) => format!(" ({bytes} appended to data.saf)"),
        (true, Some(o)) => format!(" (new pair at {})", o.display()),
        (false, Some(o)) => format!(" (new pair written to {}; data.saf is {})", o.display(), size(r.saf_len_after)),
    });
    outln!(w, "{head}");
    for p in &r.added {
        outln!(w, "  + {p}");
    }
    for p in &r.replaced {
        outln!(w, "  ~ {p}");
    }
    for p in &r.removed {
        outln!(w, "  - {p}");
    }
    for p in &r.missing {
        outln!(w, "  ? {p} (not in the archive)");
    }
    if let Some(j) = &r.journal {
        outln!(w, "backup: {}  (undo: {})", j.display(), undo_hint(client_arg, journal_dir));
    }
}

fn out_writer() -> std::io::BufWriter<std::io::StdoutLock<'static>> {
    out()
}

// ---------------------------------------------------------------------------------------------------------- compact

fn compact_cmd(ctx: &Ctx, a: &CompactArgs, job: &Job) -> Result<i32> {
    let t = &a.target;
    need_target(t.in_place, &t.out)?;
    let min_dead = parse_size(&a.min_dead)?;
    if t.in_place {
        let dir = client_dir(ctx, &a.archive)?;
        let files = prepare_write(ctx, &dir, !t.dry_run)?;
        let (saf, live) = compact::usage(&dir)?;
        let dead = saf.saturating_sub(live);
        let idx = Index::read(&files.sah)?;
        let shadowed = idx.entries().iter().filter(|f| f.shadowed).count();
        let root = journal_root(&dir, t.journal_dir.as_deref());
        let blocked = journal::list(&dir, &root).map(|l| l.iter().filter(|(j, _)| j.state == journal::State::Applied).count()).unwrap_or(0);
        if t.dry_run || dead < min_dead || (dead == 0 && shadowed == 0 && idx.free.is_empty()) {
            let nothing = dead < min_dead || (dead == 0 && shadowed == 0 && idx.free.is_empty());
            if ctx.json {
                print_json(&json!({"dry_run": t.dry_run, "nothing_to_do": nothing, "size": saf, "live_bytes": live, "dead_bytes": dead, "shadowed": shadowed, "free_entries": idx.free.len(), "needs_free_bytes": live + (256 << 20)}));
            } else if nothing {
                println!("nothing to do: {} unused in data.saf", size(dead));
            } else {
                println!("would rewrite data.saf: {} now, {} live, {} unused; drops {} and the free list", size(saf), size(live), size(dead), count(shadowed, "shadowed entry"));
                println!("needs about {} free on that drive while it runs", size(live + (256 << 20)));
                if blocked > 0 {
                    println!("{} can no longer be rolled back after compacting", count(blocked, "earlier change"));
                }
            }
            return Ok(0);
        }
        if blocked > 0 {
            note(ctx, &format!("note: {} can no longer be rolled back after compacting", count(blocked, "earlier change")));
        }
        let c = compact::compact(&dir, job)?;
        if ctx.json {
            print_json(&serde_json::to_value(&c).unwrap_or(Value::Null));
        } else {
            println!("compacted data.saf: {} -> {} ({} dropped)", size(c.before), size(c.after), count(c.duplicates.len(), "shadowed entry"));
        }
        return Ok(0);
    }
    let out = t.out.as_ref().expect("checked");
    let archive = open_full(ctx, &a.archive)?;
    let sah = out_sah(out)?;
    let plan = EditPlan { label: "compact".into(), ..EditPlan::default() };
    if t.dry_run {
        let live = shaiya_archive::layout::analyse(archive.index(), Some(archive.saf_len())).live_bytes;
        println!("would write a compacted copy to {}: about {} (needs {} free)", sah.display(), size(live), size(live + (256 << 20)));
        return Ok(0);
    }
    let r = edit::write_new(&archive, &plan, &sah, if t.force { Existing::Overwrite } else { Existing::Refuse }, job)?;
    if ctx.json {
        let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
        v["out_sah"] = json!(sah.display().to_string());
        v["out_saf"] = json!(sah.with_extension("saf").display().to_string());
        print_json(&v);
    } else {
        println!("wrote a compacted copy: {} -> {} at {}", size(r.saf_len_before), size(r.saf_len_after), sah.display());
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- diff

fn diff_cmd(ctx: &Ctx, a: &DiffArgs, job: &Job) -> Result<i32> {
    let (old, new) = (open_read(ctx, &a.old)?, open_read(ctx, &a.new)?);
    let mut content = !a.quick;
    if content && (old.archive().is_none() || new.archive().is_none()) {
        note(ctx, "note: comparing the indexes only (a data.saf is missing)");
        content = false;
    }
    fn side(o: &Opened) -> Side<'_> {
        match o.archive() {
            Some(ar) => Side::Archive(ar),
            None => Side::Index(o.index()),
        }
    }
    let opts = DiffOptions { content, filter: Filter::from_args(&a.patterns, &[], None, None)?, include_shadowed: a.include_shadowed };
    let d = diff(&side(&old), &side(&new), &opts, job)?;
    let s = summarize(&d);
    let only: Vec<String> = a.only.iter().map(|k| k.to_lowercase()).collect();
    let shown: Vec<&shaiya_archive::diff::DiffEntry> = d
        .iter()
        .filter(|e| match e.change {
            Change::Same => a.show_same,
            Change::Added => only.is_empty() || only.iter().any(|k| k == "added"),
            Change::Removed => only.is_empty() || only.iter().any(|k| k == "removed"),
            Change::Different => only.is_empty() || only.iter().any(|k| k == "different"),
            Change::Unknown => only.is_empty() || only.iter().any(|k| k == "unknown"),
        })
        .collect();
    let line = format!("{} removed, {} added, {} different, {} same, {} unknown", num(s.removed as u64), num(s.added as u64), num(s.different as u64), num(s.same as u64), num(s.unknown as u64));
    if ctx.json {
        print_json(&json!({"summary": s, "entries": if a.summary { Vec::new() } else { shown.iter().map(|e| serde_json::to_value(e).unwrap_or(Value::Null)).collect::<Vec<_>>() }}));
    } else if a.summary {
        println!("{line}");
    } else {
        let mut w = out();
        for e in &shown {
            let mark = match e.change {
                Change::Added => '+',
                Change::Removed => '-',
                Change::Different => '~',
                Change::Unknown => '?',
                Change::Same => '=',
            };
            outln!(w, "{mark} {}", e.path);
        }
        let _ = w.flush();
        note(ctx, &line);
    }
    Ok(if s.added + s.removed + s.different + s.unknown == 0 { 0 } else { 1 })
}

// ---------------------------------------------------------------------------------------------------------- pack / swap

fn pack_cmd(ctx: &Ctx, a: &PackArgs, job: &Job) -> Result<i32> {
    let ts = a.timestamp.as_deref().map(parse_utc).transpose()?.map(|t| t.clamp(0, i32::MAX as i64) as i32);
    let opts = PackOptions {
        sah_version: a.sah_version,
        timestamp: ts,
        filter: Filter::from_args(&[], &a.exclude, None, None)?,
        include_all: a.include_all,
        existing: if a.force { Existing::Overwrite } else { Existing::Refuse },
        dry_run: a.dry_run,
    };
    let sah = if a.dry_run { if a.out.extension().is_some_and(|e| e.eq_ignore_ascii_case("sah")) { a.out.clone() } else { a.out.join("data.sah") } } else { out_sah(&a.out)? };
    let r = pack::pack_folder(&a.folder, &sah, &opts, job)?;
    if ctx.json {
        let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
        v["out_sah"] = json!(sah.display().to_string());
        v["out_saf"] = json!(sah.with_extension("saf").display().to_string());
        if a.dry_run {
            v["dry_run"] = json!(true);
        }
        print_json(&v);
    } else {
        for (p, why) in &r.skipped {
            note(ctx, &format!("note: skipped {p} ({why})"));
        }
        println!("{} {} in {} ({}) -> {}", if a.dry_run { "would pack" } else { "packed" }, count(r.files, "file"), count(r.dirs, "folder"), size(r.bytes), sah.display());
    }
    Ok(0)
}

fn swap_cmd(ctx: &Ctx, a: &SwapArgs, job: &Job) -> Result<i32> {
    let dir = client_dir(ctx, &a.client)?;
    let other = resolve(ctx, &a.other)?;
    if a.dry_run {
        ClientFiles::locate(&dir)?;
        let plan = swap::plan_renames(&dir, &other)?;
        if ctx.json {
            print_json(&json!({"dry_run": true, "renames": plan.iter().map(|(f, t)| json!({"from": f.display().to_string(), "to": t.display().to_string()})).collect::<Vec<_>>()}));
        } else {
            for (f, t) in &plan {
                println!("would rename {} -> {}", f.display(), t.display());
            }
        }
        return Ok(0);
    }
    prepare_write(ctx, &dir, true)?;
    let r = swap::swap(&dir, &other, job)?;
    if ctx.json {
        print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
    } else {
        println!("installed {}; the old pair is kept as {} and {}", r.installed_from.display(), file_name(&r.kept_as.0), file_name(&r.kept_as.1));
        println!("swap back with: shaiya archive swap {} {}", shell_arg(&a.client), shell_arg(&r.kept_as.0.display().to_string()));
    }
    Ok(0)
}
