//! `shaiya svmap ...`: server map files (`.svmap`) and a client's `.wld` files. Thin over `shaiya_svmap_tools::batch`: it parses the
//! arguments, runs the checks clap cannot express (usage, exit 2), calls the library, prints the report as text or JSON and maps the
//! outcome to the exit codes of docs/tools/shaiya.md.

use super::util::*;
use crate::output::{emit, print_json, Report};
use crate::Ctx;
use clap::ValueEnum;
use shaiya_core::{Error, Job, Result};
use shaiya_svmap::diff::DiffKind;
use shaiya_svmap::Severity;
use shaiya_svmap_tools::batch::{self, BuildReport, BulkReport, CheckOptions, CheckReport, FileDiff, FolderDiff, RoundtripReport, SpawnsReport, WldCheckReport};
use shaiya_svmap_tools::keys::TargetServer;
use shaiya_svmap_tools::spawns::Group;
use std::io::Write;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Validate a map file or a folder of them (exit 1 when a finding is an error)
    ///
    /// A folder also runs the cross-file rules. --client adds the rules against the client's data (Monster/NpcQuest SData and
    /// world/<id>.wld); --server-data (the server's Data folder: Map.ini, MapWar.ini, NpcQuest.SData, Obelisk.ini) adds the server and
    /// client rules; --afterglow-defs adds the monster check.
    ///
    /// Example: shaiya svmap check maps/ --client "@us"
    Check(CheckArgs),
    /// Print a summary of a map file, or with --json the full map document (blocked cells as base64)
    ///
    /// Example: shaiya svmap dump 35.svmap --json
    Dump(DumpArgs),
    /// Build a map file from the JSON document `dump --json` prints
    ///
    /// The output is read back and compared before it is written. An existing output is refused unless --force.
    ///
    /// Example: shaiya svmap build 35.json -o 35.svmap
    Build(BuildArgs),
    /// Read and write every map of a folder, in binary and through JSON (exit 1 when a file does not come back identical)
    ///
    /// Example: shaiya svmap roundtrip maps/
    Roundtrip(FolderArgs),
    /// Compare two map files record by record, or two folders map by map (exit 1 when they differ)
    ///
    /// Example: shaiya svmap diff old/ new/
    Diff(DiffArgs),
    /// Read every world/*.wld of a client and write it back in memory (exit 1 when a file is not byte-identical)
    ///
    /// Example: shaiya svmap wld-check "@us"
    WldCheck(WldArgs),
    /// Spawn report of a folder: areas, monsters, kinds, level range and spawn findings per map (it reports, it does not judge)
    ///
    /// Example: shaiya svmap spawns maps/ --client "@us"
    Spawns(SpawnsArgs),
    /// What a bulk edit rule would do to the monster areas, map by map (a dry run: nothing is ever written)
    ///
    /// Example: shaiya svmap bulk maps/ --rule "mob:1 => count +1"
    Bulk(BulkArgs),
}

#[derive(Clone, Copy, ValueEnum)]
pub enum TargetArg {
    Original,
    Afterglow,
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// A .svmap file or a folder of them (not with --maps)
    #[arg(value_name = "FILE|FOLDER", required_unless_present = "maps")]
    pub target: Option<PathBuf>,
    /// A client folder, its data.sah or @name: adds the rules against the client's data
    #[arg(long, value_name = "CLIENT")]
    pub client: Option<String>,
    /// The server's Data folder (a folder of maps only): adds the rules over Map.ini, MapWar.ini, NpcQuest.SData and Obelisk.ini
    #[arg(long, value_name = "DIR")]
    pub server_data: Option<PathBuf>,
    /// Afterglow's monster definitions (needs --server-data)
    #[arg(long, value_name = "PATH", requires = "server_data")]
    pub afterglow_defs: Option<PathBuf>,
    /// The folder of maps when it is not given as the target and not `<server data>/Map` (a split server tree keeps the INIs and
    /// the maps in two folders; needs --server-data). `MapWar.ini` missing is then an info note
    #[arg(long, value_name = "MAPS", requires = "server_data", conflicts_with = "target")]
    pub maps: Option<PathBuf>,
    /// Which server the cross-file rules judge for
    #[arg(long = "target", value_enum, default_value = "original")]
    pub target_server: TargetArg,
}

#[derive(clap::Args)]
pub struct DumpArgs {
    /// A .svmap file
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
}

#[derive(clap::Args)]
pub struct BuildArgs {
    /// The JSON document (from `dump --json`)
    #[arg(value_name = "JSON")]
    pub input: PathBuf,
    /// The .svmap file to write
    #[arg(short, long, value_name = "OUT.svmap")]
    pub out: PathBuf,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct FolderArgs {
    /// A folder of .svmap files
    #[arg(value_name = "FOLDER")]
    pub folder: PathBuf,
}

#[derive(clap::Args)]
pub struct DiffArgs {
    /// The old side: a file or a folder
    #[arg(value_name = "A")]
    pub a: PathBuf,
    /// The new side: a file or a folder (the same kind as A)
    #[arg(value_name = "B")]
    pub b: PathBuf,
}

#[derive(clap::Args)]
pub struct WldArgs {
    /// A client folder (extracted or with data.sah), its data.sah or @name
    #[arg(value_name = "CLIENT")]
    pub client: String,
}

#[derive(clap::Args)]
pub struct SpawnsArgs {
    /// A folder of .svmap files
    #[arg(value_name = "FOLDER")]
    pub folder: PathBuf,
    /// A client folder, its data.sah or @name: monster names and levels, and the monsters no area spawns
    #[arg(long, value_name = "CLIENT")]
    pub client: Option<String>,
}

#[derive(clap::Args)]
pub struct BulkArgs {
    /// A folder of .svmap files
    #[arg(value_name = "FOLDER")]
    pub folder: PathBuf,
    /// The rule, `<where> => <action>`
    #[arg(long, value_name = "RULE")]
    pub rule: String,
    /// A client folder, its data.sah or @name (monster names and levels for the rule)
    #[arg(long, value_name = "CLIENT")]
    pub client: Option<String>,
    /// Counts may go above 8
    #[arg(long)]
    pub allow_above_8: bool,
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &Job) -> i32 {
    let r = match cmd {
        Cmd::Check(a) => check(ctx, a, job),
        Cmd::Dump(a) => dump(ctx, a, job),
        Cmd::Build(a) => build(ctx, a, job),
        Cmd::Roundtrip(a) => roundtrip(ctx, a, job),
        Cmd::Diff(a) => diff(ctx, a, job),
        Cmd::WldCheck(a) => wld_check(ctx, a, job),
        Cmd::Spawns(a) => spawns(ctx, a, job),
        Cmd::Bulk(a) => bulk(ctx, a, job),
    };
    match r {
        Ok(code) => code,
        Err(Failure::Usage(m)) => usage(ctx, &m),
        Err(Failure::Error(e)) if batch::is_internal(&e) => internal(ctx, &e),
        Err(Failure::Error(e)) => fail(ctx, &e),
    }
}

/// Why a command stopped before it had a result.
enum Failure {
    /// A combination of arguments clap cannot reject (exit 2).
    Usage(String),
    Error(Error),
}

impl From<Error> for Failure {
    fn from(e: Error) -> Failure {
        Failure::Error(e)
    }
}

type Res = std::result::Result<i32, Failure>;

fn usage(ctx: &Ctx, message: &str) -> i32 {
    if !ctx.json || !ctx.quiet {
        eprintln!("error: {message}");
    }
    if ctx.json {
        print_json(&serde_json::json!({ "error": { "kind": "usage", "message": message } }));
    }
    2
}

/// The output did not read back to the map it was made from: a bug of ours, exit 5, nothing written.
fn internal(ctx: &Ctx, e: &Error) -> i32 {
    eprintln!("error: internal error (this is a bug): {e}");
    eprintln!("hint: please report it at https://github.com/byyako/shaiya-toolkit/issues and say which command you ran");
    if ctx.json {
        print_json(&serde_json::json!({ "error": { "kind": "internal", "message": e.to_string() } }));
    }
    5
}

fn notes(ctx: &Ctx, lines: &[String]) {
    for n in lines {
        note(ctx, &format!("note: {n}"));
    }
}

fn skipped(ctx: &Ctx, lines: &[String]) {
    for n in lines {
        note(ctx, &format!("note: skipped: {n}"));
    }
}

fn client_arg(ctx: &Ctx, arg: &Option<String>) -> Result<Option<PathBuf>> {
    arg.as_deref().map(|a| client_dir(ctx, a)).transpose()
}

// ------------------------------------------------------------------------------------------------------------------ check

fn check(ctx: &Ctx, a: &CheckArgs, job: &Job) -> Res {
    let Some(target) = a.target.as_ref().or(a.maps.as_ref()) else {
        return Err(Failure::Usage("give the maps to check: a file or folder, or --maps".into()));
    };
    if a.server_data.is_some() && target.is_file() {
        return Err(Failure::Usage("--server-data needs a folder of maps, not a single file".into()));
    }
    let client = client_arg(ctx, &a.client)?;
    let opts = CheckOptions {
        client,
        server_data: a.server_data.clone(),
        afterglow_defs: a.afterglow_defs.clone(),
        war_optional: a.maps.is_some(),
        target: match a.target_server {
            TargetArg::Afterglow => TargetServer::Afterglow,
            TargetArg::Original => TargetServer::Original,
        },
    };
    let r = batch::check(target, &opts, job)?;
    notes(ctx, &r.notes);
    emit(ctx, &r);
    Ok(r.exit_code())
}

impl Report for CheckReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for r in &self.maps {
            if let Some(e) = &r.read_error {
                writeln!(w, "{}: error: cannot read: {e}", r.file)?;
            }
            for f in r.findings.iter().filter(|f| f.severity != Severity::Info) {
                writeln!(w, "{}: {}", r.file, batch::finding_line(f))?;
            }
        }
        if let Some(cross) = &self.server_and_client {
            if !cross.is_empty() {
                writeln!(w)?;
                writeln!(w, "Server and client:")?;
                for c in cross.iter().filter(|c| c.finding.severity != Severity::Info) {
                    writeln!(w, "  {}: {}", c.finding.severity.label(), batch::cross_line(c))?;
                }
            }
        }
        writeln!(w)?;
        writeln!(w, "checked {} file(s)", self.totals.files)?;
        for t in self.code_totals() {
            let list: Vec<String> = t.maps.iter().map(|(m, c)| if *c == 1 { m.clone() } else { format!("{m}x{c}") }).collect();
            writeln!(w, "  {:<7} {:<22} {:>5}   maps {}", t.severity.label(), t.code, t.count, list.join(","))?;
        }
        let t = &self.totals;
        writeln!(w, "errors {} (+{} unreadable), warnings {}, info {}", t.errors, t.unreadable, t.warnings, t.info)
    }
}

// ------------------------------------------------------------------------------------------------------------------ dump

fn dump(ctx: &Ctx, a: &DumpArgs, _job: &Job) -> Res {
    let d = batch::dump(&a.file)?;
    if ctx.json {
        // the document itself, exactly what `build` reads back
        let text = d.json(&a.file)?;
        let stdout = std::io::stdout();
        let mut w = stdout.lock();
        let _ = writeln!(w, "{text}");
        let _ = w.flush();
    } else {
        let stdout = std::io::stdout();
        let mut w = stdout.lock();
        let _ = write!(w, "{}", d.summary());
        let _ = w.flush();
    }
    Ok(0)
}

// ------------------------------------------------------------------------------------------------------------------ build

fn build(ctx: &Ctx, a: &BuildArgs, job: &Job) -> Res {
    let r = batch::build(&a.input, &a.out, a.force, job)?;
    emit(ctx, &r);
    Ok(0)
}

impl Report for BuildReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "wrote {} ({} bytes)", self.out, self.bytes)
    }
}

// ------------------------------------------------------------------------------------------------------------------ roundtrip

fn roundtrip(ctx: &Ctx, a: &FolderArgs, job: &Job) -> Res {
    let r = batch::roundtrip(&a.folder, job)?;
    emit(ctx, &r);
    Ok(r.exit_code())
}

impl Report for RoundtripReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for f in &self.files {
            if f.binary == "unreadable" {
                writeln!(w, "{}: {}", f.file, f.message.as_deref().unwrap_or("cannot be read"))?;
                continue;
            }
            if f.binary == "differs" {
                writeln!(
                    w,
                    "{}: binary roundtrip differs (first difference at byte {}, lengths {} -> {})",
                    f.file,
                    f.first_difference.unwrap_or(0),
                    f.length_in.unwrap_or(0),
                    f.length_out.unwrap_or(0)
                )?;
            }
            match f.json {
                "differs" => writeln!(w, "{}: JSON roundtrip differs", f.file)?,
                "failed" => writeln!(w, "{}: JSON roundtrip failed: {}", f.file, f.message.as_deref().unwrap_or(""))?,
                _ => {}
            }
        }
        let t = &self.totals;
        writeln!(
            w,
            "{} files: binary roundtrip identical {}, JSON roundtrip identical {}, differs {}, unreadable {}",
            t.files, t.binary_identical, t.json_identical, t.differs, t.unreadable
        )
    }
}

// ------------------------------------------------------------------------------------------------------------------ diff

fn diff(ctx: &Ctx, a: &DiffArgs, job: &Job) -> Res {
    // a missing path is named as such (exit 3), whatever the other side is; any other metadata error (access denied) is reported as it is
    for p in [&a.a, &a.b] {
        if let Err(e) = std::fs::metadata(p) {
            return Err(Failure::Error(Error::io(p.clone(), e)));
        }
    }
    if (a.a.is_dir() && a.b.is_file()) || (a.a.is_file() && a.b.is_dir()) {
        return Err(Failure::Usage("diff compares two files or two folders, not one of each".into()));
    }
    if a.a.is_dir() && a.b.is_dir() {
        let r = batch::diff_folders(&a.a, &a.b, job)?;
        notes(ctx, &r.notes);
        emit(ctx, &r);
        Ok(r.exit_code())
    } else {
        let r = batch::diff_files(&a.a, &a.b, job)?;
        emit(ctx, &r);
        Ok(r.exit_code())
    }
}

impl Report for FileDiff {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.identical {
            return writeln!(w, "identical");
        }
        let mut current = None;
        for e in &self.entries {
            if current != Some(e.section) {
                current = Some(e.section);
                if let Some(s) = self.sections.iter().find(|s| s.section == e.section) {
                    writeln!(w, "{}: {} added, {} removed, {} changed", e.section.label(), s.added, s.removed, s.changed)?;
                }
            }
            let (sign, idx) = match e.kind {
                DiffKind::Added => ('+', format!("#{}", e.index)),
                DiffKind::Removed => ('-', format!("#{}", e.index)),
                DiffKind::Changed => (
                    '~',
                    match e.index_b {
                        Some(j) if j != e.index => format!("#{} -> #{j}", e.index),
                        _ => format!("#{}", e.index),
                    },
                ),
            };
            writeln!(w, "  {sign} {idx}  {}", e.text)?;
        }
        Ok(())
    }
}

impl Report for FolderDiff {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for l in self.lines() {
            writeln!(w, "{l}")?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------------------------ wld-check

fn wld_check(ctx: &Ctx, a: &WldArgs, job: &Job) -> Res {
    let client = client_dir(ctx, &a.client)?;
    let r = batch::wld_check(&client, job)?;
    emit(ctx, &r);
    Ok(r.exit_code())
}

impl Report for WldCheckReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        for r in &self.files {
            match r.status {
                "identical" => writeln!(w, "{}: byte-identical ({} bytes)", r.path, r.bytes)?,
                "differs" => writeln!(
                    w,
                    "{}: differs, first difference at byte {} (file {} bytes, written {} bytes)",
                    r.path,
                    r.first_difference.unwrap_or(0),
                    r.bytes,
                    r.written.unwrap_or(0)
                )?,
                _ => writeln!(w, "{}: cannot be read: {}", r.path, r.reason.as_deref().unwrap_or(""))?,
            }
        }
        let t = &self.totals;
        writeln!(w, "{} files: byte-identical {}, differs {}, unreadable {}", t.files, t.identical, t.differs, t.unreadable)?;
        if t.identical == t.files {
            writeln!(w, "Every .wld writes back byte for byte.")
        } else {
            writeln!(w, "The files that are not identical stay read-only in the Shaiya SVMap Editor.")
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ spawns

fn spawns(ctx: &Ctx, a: &SpawnsArgs, job: &Job) -> Res {
    let client = client_arg(ctx, &a.client)?;
    let r = batch::spawns(&a.folder, client.as_deref(), job)?;
    notes(ctx, &r.notes);
    skipped(ctx, &r.unreadable);
    emit(ctx, &r);
    Ok(r.exit_code())
}

impl Report for SpawnsReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "{:>4} {:>6} {:>8} {:>6} {:>9}  {}", "map", "areas", "monsters", "kinds", "range", Group::ALL.map(|g| g.key()).join(" "))?;
        for m in &self.maps {
            let range = m.range.map_or("-".to_string(), |[a, b]| format!("{a}-{b}"));
            let counts: Vec<String> = Group::ALL.iter().map(|g| format!("{:>width$}", m.findings.get(g.key()).copied().unwrap_or(0), width = g.key().len())).collect();
            writeln!(w, "{:>4} {:>6} {:>8} {:>6} {:>9}  {}", m.map.map_or("-".to_string(), |i| i.to_string()), m.areas, m.monsters, m.kinds, range, counts.join(" "))?;
        }
        let t = &self.totals;
        writeln!(w)?;
        writeln!(w, "{} maps, {} areas, {} monsters, {} kinds", t.maps, t.areas, t.monsters, t.kinds)?;
        let parts: Vec<String> = Group::ALL.iter().map(|g| format!("{} {}", g.key(), t.findings.get(g.key()).copied().unwrap_or(0))).collect();
        writeln!(w, "findings: {}", parts.join(", "))?;
        match t.never_spawned {
            Some(n) => writeln!(w, "never spawned: {n} (bosses, obelisks and scripted spawns live in other files)"),
            None => writeln!(w, "never spawned: needs --client{}", if self.levels { "" } else { " with a readable Monster.SData" }),
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ bulk

fn bulk(ctx: &Ctx, a: &BulkArgs, job: &Job) -> Res {
    if let Err(m) = batch::parse_rule(&a.rule) {
        return Err(Failure::Usage(m));
    }
    let client = client_arg(ctx, &a.client)?;
    let opts = shaiya_svmap_tools::bulk::Opts { allow_above_8: a.allow_above_8 };
    let r = batch::bulk(&a.folder, &a.rule, client.as_deref(), opts, job)?;
    notes(ctx, &r.notes);
    skipped(ctx, &r.unreadable);
    emit(ctx, &r);
    Ok(r.exit_code())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Report for BulkReport {
    fn human(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "rule: {}", self.rule)?;
        writeln!(w, "{:>4} {:>8} {:>7} {:>7} {:>16} {:>9} {:>6} {:>7}", "map", "matched", "changed", "entries", "monsters", "held at 8", "empty", "deleted")?;
        for m in &self.maps {
            writeln!(
                w,
                "{:>4} {:>8} {:>7} {:>7} {:>16} {:>9} {:>6} {:>7}",
                m.map.map_or("-".to_string(), |i| i.to_string()),
                m.matched,
                m.changed,
                m.entries,
                format!("{} -> {}", m.monsters_before, m.monsters_after),
                m.held_at_8,
                m.emptied,
                m.deleted
            )?;
        }
        let t = &self.totals;
        writeln!(w)?;
        writeln!(
            w,
            "{} match on {}; {} would change on {}; {} changed, held at 8: {}, left empty: {}",
            plural(t.matched, "area", "areas"),
            plural(t.maps_matched, "map", "maps"),
            plural(t.changed, "area", "areas"),
            plural(t.maps_changed, "map", "maps"),
            plural(t.entries, "entry", "entries"),
            t.held_at_8,
            t.emptied
        )?;
        writeln!(w, "dry run: nothing was written")
    }
}

