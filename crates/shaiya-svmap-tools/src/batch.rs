//! The entry points of `shaiya svmap`: every command works on files and returns a report that serializes to the JSON documents of the
//! command line (keys in snake_case). Nothing here prints; notes come back in the reports. Errors are `shaiya_core::Error` with the
//! kind the exit codes need: a path that is missing or cannot be read is `Io`, a file that does not parse is `Format`, an output
//! that must not be written is `Refused`. A file that cannot be read inside a folder is an entry of the report, not an error, and
//! the report's `exit_code` raises to 3 for it.
//!
//! The command line checks the flag combinations clap cannot express (a server data folder with a single file, an Afterglow
//! defs path without a server data folder, a rule that does not parse: see [`parse_rule`]) before it calls in here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use shaiya_core::{Error, Job, Result, Unit};
use shaiya_svmap::diff::{diff, DiffEntry, DiffKind};
use shaiya_svmap::{from_json, read, tally, to_json, validate, validate_cross, write, Finding, ReadError, ReadErrorKind, SectionId, Severity, Svmap};

use crate::client_rules::ClientRules;
use crate::keys::TargetServer;

// ------------------------------------------------------------------------------------------------------------------ files

/// The map id a file name stands for (`35.svmap` is 35).
pub fn map_id(path: &Path) -> Option<u32> {
    path.file_stem()?.to_str()?.parse().ok()
}

fn not_found(path: &Path, what: &str) -> Error {
    Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, what))
}

fn read_what(e: &ReadError) -> String {
    let mut s = e.section.to_string();
    if let Some(i) = e.index {
        s.push_str(&format!(" record {i}"));
    }
    s.push_str(": ");
    match &e.kind {
        ReadErrorKind::Truncated { need, have } => s.push_str(&format!("file ends early (needs {need} bytes, {have} left)")),
        ReadErrorKind::CountTooLarge { count } => s.push_str(&format!("record count {count} is above the limit")),
        ReadErrorKind::TrailingBytes { count } => s.push_str(&format!("{count} unexpected bytes after the last section")),
    }
    s
}

/// Reads one `.svmap` file. `Io` when it cannot be read, `Format` (with the byte offset) when it is not a map.
pub fn load(path: &Path) -> Result<Svmap> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    read(&bytes).map_err(|e| Error::format(path.display().to_string(), Some(e.offset as u64), read_what(&e)))
}

/// The `.svmap` files of a folder, sorted by map id (names that are not a number last, by name).
pub fn list_folder(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()).is_some_and(|s| s.eq_ignore_ascii_case("svmap")))
        .collect();
    v.sort_by_key(|p| (map_id(p).is_none(), map_id(p), p.clone()));
    Ok(v)
}

/// The files of a folder, each read: the folder must hold at least one `.svmap` (`Format` otherwise).
fn load_folder(dir: &Path, job: &Job) -> Result<Vec<(PathBuf, std::result::Result<Svmap, Error>)>> {
    let files = list_folder(dir)?;
    if files.is_empty() {
        return Err(Error::format(dir.display().to_string(), None, "no .svmap files"));
    }
    job.stage("Reading maps");
    job.total(Some(files.len() as u64), Unit::Files);
    let mut out = Vec::with_capacity(files.len());
    for p in files {
        job.check()?;
        let r = load(&p);
        out.push((p, r));
        job.advance(1);
    }
    Ok(out)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Opens a client folder (or `data.sah` archive) for the rules. A typed clientdata error becomes `Io` (missing or unreadable) or
/// `Format`, so every failure to open is exit code 3.
pub fn open_client(dir: &Path) -> Result<ClientRules> {
    use shaiya_clientdata::Error as CdError;
    if let Err(e) = shaiya_clientdata::ClientSource::open(dir) {
        return Err(match e {
            CdError::NotFound(p) => not_found(Path::new(&p), "not found"),
            CdError::Io { path, source } => Error::io(path, source),
            CdError::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
            CdError::Archive(s) => Error::format(dir.display().to_string(), None, s),
        });
    }
    ClientRules::open(dir).map_err(|s| Error::format(dir.display().to_string(), None, s))
}

/// The notes of an open client: the files it could not read and the episode notes.
fn client_note_lines(rules: &ClientRules) -> Vec<String> {
    let mut notes: Vec<String> = rules.problems.iter().map(|p| format!("client file skipped: {p}")).collect();
    notes.extend(crate::episode::client_notes(rules));
    notes
}

/// Whether `e` is the internal-error case of [`build`] (the output did not read back to the same map): the command line exits 5.
pub fn is_internal(e: &Error) -> bool {
    matches!(e, Error::Format { what, .. } if what.starts_with("internal: "))
}

// ------------------------------------------------------------------------------------------------------------------ dump

/// A map read for `dump`.
pub struct Dump {
    pub map: Option<u32>,
    pub svmap: Svmap,
}

pub fn dump(file: &Path) -> Result<Dump> {
    Ok(Dump { map: map_id(file), svmap: load(file)? })
}

impl Dump {
    /// The full JSON document (blocked cells as base64), exactly what [`build`] reads. A NaN or infinity is a `Format` error.
    pub fn json(&self, file: &Path) -> Result<String> {
        to_json(&self.svmap).map_err(|e| Error::format(file.display().to_string(), None, e.to_string()))
    }

    pub fn summary(&self) -> String {
        summary(&self.svmap, self.map)
    }
}

fn f3(v: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", v[0], v[1], v[2])
}

/// The text of `dump` without `--json`.
pub fn summary(m: &Svmap, id: Option<u32>) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let cells = m.size as u64 * m.size as u64;
    let blocked = m.blocked.count();
    let _ = writeln!(
        s,
        "map {}: size {} ({} cells), blocked {} ({:.2} %)",
        id.map_or("?".to_string(), |i| i.to_string()),
        m.size,
        cells,
        blocked,
        if cells == 0 { 0.0 } else { blocked as f64 * 100.0 / cells as f64 }
    );
    let _ = writeln!(s, "section A: {} records, section B: {} points", m.section_a.len(), m.points_b.len());
    for (i, p) in m.points_b.iter().enumerate() {
        let _ = writeln!(s, "  B #{i} {}", f3(*p));
    }

    let entries: usize = m.mob_areas.iter().map(|a| a.mobs.len()).sum();
    let total: u64 = m.mob_areas.iter().flat_map(|a| &a.mobs).map(|e| e.count as u64).sum();
    let empty = m.mob_areas.iter().filter(|a| a.mobs.is_empty()).count();
    let ids = m.mob_areas.iter().flat_map(|a| a.mobs.iter().map(|e| e.mob));
    let (lo, hi) = ids.fold((u32::MAX, 0), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let _ = write!(s, "monster areas: {} ({entries} entries, {total} monsters in total, {empty} empty", m.mob_areas.len());
    if entries > 0 {
        let _ = write!(s, ", monster ids {lo}..{hi}");
    }
    let _ = writeln!(s, ")");

    let mut types: BTreeMap<u32, usize> = BTreeMap::new();
    for n in &m.npcs {
        *types.entry(n.kind).or_default() += 1;
    }
    let pts: usize = m.npcs.iter().map(|n| n.points.len()).sum();
    let _ = writeln!(s, "NPCs: {} ({pts} points; by type: {})", m.npcs.len(), types.iter().map(|(t, n)| format!("{t}x{n}")).collect::<Vec<_>>().join(" "));

    let _ = writeln!(s, "portals: {}", m.portals.len());
    for (i, p) in m.portals.iter().enumerate() {
        let _ = writeln!(s, "  #{i} {} country {} lv {}-{} -> map {} {}", f3(p.pos), p.country, p.level_min, p.level_max, p.to_map, f3(p.to_pos));
    }
    let _ = writeln!(s, "bind areas: {}", m.binds.len());
    for (i, b) in m.binds.iter().enumerate() {
        let c = b.boxes.first().map(|x| f3(x.centre())).unwrap_or_else(|| "(no box)".into());
        let _ = writeln!(s, "  #{i} country {} attr {} boxes {} first centre {c}", b.country, b.attr, b.boxes.len());
    }
    let _ = writeln!(s, "regions: {}", m.regions.len());
    s
}

// ------------------------------------------------------------------------------------------------------------------ check

/// What `check` is given besides the target. `client` is an already resolved client folder.
#[derive(Clone, Debug, Default)]
pub struct CheckOptions {
    pub client: Option<PathBuf>,
    pub server_data: Option<PathBuf>,
    pub afterglow_defs: Option<PathBuf>,
    pub target: TargetServer,
}

#[derive(Debug, Serialize)]
pub struct MapCheck {
    pub map: Option<u32>,
    pub file: String,
    pub read_error: Option<String>,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Serialize)]
pub struct CrossRow {
    pub map: Option<u32>,
    pub finding: Finding,
}

#[derive(Debug, Default, Serialize)]
pub struct CheckTotals {
    pub files: usize,
    pub unreadable: usize,
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
}

#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub target: String,
    /// `"file"` or `"folder"`.
    pub kind: &'static str,
    pub maps: Vec<MapCheck>,
    /// `None` without a server data folder.
    pub server_and_client: Option<Vec<CrossRow>>,
    pub notes: Vec<String>,
    pub totals: CheckTotals,
}

type CodeMap = BTreeMap<(std::cmp::Reverse<Severity>, &'static str), (usize, BTreeMap<String, usize>)>;
type ReadableMaps = (Vec<(u32, Svmap)>, Vec<String>);

/// One line of the totals table of the text output: a rule, how often it fired and on which maps.
#[derive(Debug)]
pub struct CodeTotal {
    pub severity: Severity,
    pub code: &'static str,
    pub count: usize,
    /// (map id, or the file name when it has no id, or `folder`; findings on it), in key order.
    pub maps: Vec<(String, usize)>,
}

impl CheckReport {
    /// 3 when a file could not be read, 1 when a finding is an error, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.totals.unreadable > 0 {
            3
        } else if self.totals.errors > 0 {
            1
        } else {
            0
        }
    }

    /// The totals per (severity, rule), errors first, for the text output.
    pub fn code_totals(&self) -> Vec<CodeTotal> {
        let mut t: CodeMap = BTreeMap::new();
        for r in &self.maps {
            for f in &r.findings {
                let e = t.entry((std::cmp::Reverse(f.severity), f.code)).or_default();
                e.0 += 1;
                *e.1.entry(r.map.map_or(r.file.clone(), |i| i.to_string())).or_default() += 1;
            }
        }
        for c in self.server_and_client.iter().flatten() {
            let e = t.entry((std::cmp::Reverse(c.finding.severity), c.finding.code)).or_default();
            e.0 += 1;
            *e.1.entry(c.map.map_or("folder".to_string(), |m| m.to_string())).or_default() += 1;
        }
        t.into_iter().map(|((sev, code), (count, maps))| CodeTotal { severity: sev.0, code, count, maps: maps.into_iter().collect() }).collect()
    }
}

/// One finding as a line of the text output: `error portals #3 [portal-far]: message`.
pub fn finding_line(f: &Finding) -> String {
    let at = match (f.index, f.sub) {
        (Some(i), Some(k)) => format!(" #{i}.{k}"),
        (Some(i), None) => format!(" #{i}"),
        _ => String::new(),
    };
    format!("{} {}{at} [{}]: {}", f.severity.label(), f.section.label(), f.code, f.message)
}

/// A cross-file finding as a line of the text output.
pub fn cross_line(c: &CrossRow) -> String {
    let row = crate::cross_rules::CrossFinding { map: c.map, finding: c.finding.clone() };
    crate::cross_rules::line(&row)
}

/// Validates a file, or every `.svmap` of a folder (file rules and cross-map rules; with `opts.client` the client rules; with
/// `opts.server_data` the rules over the server's data folder as well). A file that cannot be read is a `read_error` entry. A missing
/// target, a missing server data folder, a client that cannot be opened, or a folder without `.svmap` files is an error.
pub fn check(target: &Path, opts: &CheckOptions, job: &Job) -> Result<CheckReport> {
    let is_dir = target.is_dir();
    if !is_dir && !target.exists() {
        return Err(not_found(target, "no such file or folder"));
    }
    if let Some(dir) = &opts.server_data {
        if !is_dir {
            return Err(Error::Refused("a server data folder needs a folder of maps".to_string()));
        }
        if !dir.is_dir() {
            return Err(not_found(dir, "not a folder"));
        }
    }
    job.stage("Opening the client");
    let rules = opts.client.as_deref().map(open_client).transpose()?;
    let mut notes = rules.as_ref().map(client_note_lines).unwrap_or_default();

    let mut maps_out: Vec<MapCheck> = Vec::new();
    let mut cross: Option<Vec<CrossRow>> = None;
    if is_dir {
        let loaded = load_folder(target, job)?;
        let mut maps: BTreeMap<u32, Svmap> = BTreeMap::new();
        for (p, r) in &loaded {
            if let (Some(id), Ok(m)) = (map_id(p), r) {
                maps.insert(id, m.clone());
            }
        }
        if let Some(dir) = &opts.server_data {
            job.stage("Reading the server data");
            let arcs: BTreeMap<u32, Arc<Svmap>> = maps.iter().map(|(k, v)| (*k, Arc::new(v.clone()))).collect();
            let data = crate::server_data::ServerData::load(dir, opts.afterglow_defs.as_deref());
            let found = crate::cross_rules::check(&crate::cross_rules::Inputs { maps: &arcs, client: rules.as_ref(), server: Some(&data), target: opts.target });
            cross = Some(found.into_iter().map(|c| CrossRow { map: c.map, finding: c.finding }).collect());
        }
        job.stage("Checking");
        job.total(Some(loaded.len() as u64), Unit::Files);
        for (p, r) in loaded {
            job.check()?;
            let id = map_id(&p);
            let file = file_name(&p);
            match r {
                Ok(m) => {
                    let mut findings = validate(&m);
                    findings.extend(validate_cross(&m, &|to| maps.get(&to)));
                    if let Some(r) = &rules {
                        findings.extend(r.check(id, &m));
                    }
                    maps_out.push(MapCheck { map: id, file, read_error: None, findings });
                }
                Err(e) => maps_out.push(MapCheck { map: id, file, read_error: Some(e.to_string()), findings: Vec::new() }),
            }
            job.advance(1);
        }
    } else {
        job.stage("Checking");
        let id = map_id(target);
        let file = file_name(target);
        match load(target) {
            Ok(m) => {
                let mut findings = validate(&m);
                if let Some(r) = &rules {
                    if id.is_none() {
                        notes.push(format!("{file}: the file name is not a map id, so the .wld rules were skipped"));
                    }
                    findings.extend(r.check(id, &m));
                }
                maps_out.push(MapCheck { map: id, file, read_error: None, findings });
            }
            Err(e) => maps_out.push(MapCheck { map: id, file, read_error: Some(e.to_string()), findings: Vec::new() }),
        }
    }

    let mut totals = CheckTotals { files: maps_out.len(), ..Default::default() };
    let mut add = |t: (usize, usize, usize)| {
        totals.errors += t.0;
        totals.warnings += t.1;
        totals.info += t.2;
    };
    for r in &maps_out {
        add(tally(&r.findings));
    }
    if let Some(c) = &cross {
        let all: Vec<Finding> = c.iter().map(|r| r.finding.clone()).collect();
        add(tally(&all));
    }
    totals.unreadable = maps_out.iter().filter(|r| r.read_error.is_some()).count();
    Ok(CheckReport {
        target: target.display().to_string(),
        kind: if is_dir { "folder" } else { "file" },
        maps: maps_out,
        server_and_client: cross,
        notes,
        totals,
    })
}

// ------------------------------------------------------------------------------------------------------------------ build

#[derive(Debug, Serialize)]
pub struct BuildCounts {
    pub section_a: usize,
    pub points_b: usize,
    pub mob_areas: usize,
    pub npcs: usize,
    pub portals: usize,
    pub binds: usize,
    pub regions: usize,
}

#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub out: String,
    pub bytes: usize,
    pub size: u32,
    pub blocked: u64,
    pub counts: BuildCounts,
}

/// JSON to `.svmap`. The bytes are read back and compared with the map before anything is written (a difference is the internal
/// error [`is_internal`] names), then written atomically. `out` that exists needs `force`; `out` that is the input is refused.
pub fn build(input: &Path, out: &Path, force: bool, job: &Job) -> Result<BuildReport> {
    job.stage("Building");
    let text = std::fs::read_to_string(input).map_err(|e| Error::io(input, e))?;
    let m = from_json(&text).map_err(|e| Error::format(input.display().to_string(), None, e.to_string()))?;
    let bytes = write(&m);
    let back = read(&bytes).map_err(|e| Error::format(out.display().to_string(), None, format!("internal: the output does not parse: {}", read_what(&e))))?;
    if back != m {
        return Err(Error::format(out.display().to_string(), None, "internal: the output does not read back to the same map"));
    }
    job.check()?;
    let plan = shaiya_core::fs::plan_output(input, Some(out), false, force)?;
    shaiya_core::fs::write_atomic(&plan.target, &bytes)?;
    Ok(BuildReport {
        out: out.display().to_string(),
        bytes: bytes.len(),
        size: m.size,
        blocked: m.blocked.count() as u64,
        counts: BuildCounts {
            section_a: m.section_a.len(),
            points_b: m.points_b.len(),
            mob_areas: m.mob_areas.len(),
            npcs: m.npcs.len(),
            portals: m.portals.len(),
            binds: m.binds.len(),
            regions: m.regions.len(),
        },
    })
}

// ------------------------------------------------------------------------------------------------------------------ roundtrip

#[derive(Debug, Serialize)]
pub struct RoundtripFile {
    pub file: String,
    pub map: Option<u32>,
    /// `identical`, `differs` or `unreadable`.
    pub binary: &'static str,
    /// `identical`, `differs`, `failed` or `not_run`.
    pub json: &'static str,
    pub first_difference: Option<usize>,
    pub length_in: Option<usize>,
    pub length_out: Option<usize>,
    pub message: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct RoundtripTotals {
    pub files: usize,
    pub binary_identical: usize,
    pub json_identical: usize,
    /// Files whose binary or JSON round trip is not identical (not counting unreadable ones).
    pub differs: usize,
    pub unreadable: usize,
}

#[derive(Debug, Serialize)]
pub struct RoundtripReport {
    pub folder: String,
    pub files: Vec<RoundtripFile>,
    pub totals: RoundtripTotals,
}

impl RoundtripReport {
    /// 3 when a file is unreadable, 1 when any file differs, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.totals.unreadable > 0 {
            3
        } else if self.totals.differs > 0 {
            1
        } else {
            0
        }
    }
}

/// Reads and writes every map of a folder, in binary and through JSON, and says whether the bytes come back identical.
pub fn roundtrip(folder: &Path, job: &Job) -> Result<RoundtripReport> {
    let paths = list_folder(folder)?;
    if paths.is_empty() {
        return Err(Error::format(folder.display().to_string(), None, "no .svmap files"));
    }
    job.stage("Round trip");
    job.total(Some(paths.len() as u64), Unit::Files);
    let mut files = Vec::new();
    for p in &paths {
        job.check()?;
        let file = file_name(p);
        let map = map_id(p);
        let mut row = RoundtripFile { file, map, binary: "unreadable", json: "not_run", first_difference: None, length_in: None, length_out: None, message: None };
        match std::fs::read(p) {
            Err(e) => row.message = Some(format!("cannot read: {e}")),
            Ok(bytes) => match read(&bytes) {
                Err(e) => row.message = Some(read_what(&e)),
                Ok(m) => {
                    let out = write(&m);
                    row.length_in = Some(bytes.len());
                    row.length_out = Some(out.len());
                    if out == bytes {
                        row.binary = "identical";
                    } else {
                        row.binary = "differs";
                        row.first_difference = Some(out.iter().zip(&bytes).position(|(a, b)| a != b).unwrap_or(out.len().min(bytes.len())));
                    }
                    match to_json(&m).and_then(|j| from_json(&j)) {
                        Ok(back) if write(&back) == bytes => row.json = "identical",
                        Ok(_) => row.json = "differs",
                        Err(e) => {
                            row.json = "failed";
                            row.message = Some(e.to_string());
                        }
                    }
                }
            },
        }
        files.push(row);
        job.advance(1);
    }
    let mut totals = RoundtripTotals { files: files.len(), ..Default::default() };
    for r in &files {
        totals.binary_identical += (r.binary == "identical") as usize;
        totals.json_identical += (r.json == "identical") as usize;
        if r.binary == "unreadable" {
            totals.unreadable += 1;
        } else if r.binary != "identical" || r.json != "identical" {
            totals.differs += 1;
        }
    }
    Ok(RoundtripReport { folder: folder.display().to_string(), files, totals })
}

// ------------------------------------------------------------------------------------------------------------------ diff

#[derive(Debug, Serialize)]
pub struct SectionDiff {
    pub section: SectionId,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

#[derive(Debug, Serialize)]
pub struct FileDiff {
    pub a: String,
    pub b: String,
    /// Always `"files"`.
    pub kind: &'static str,
    pub identical: bool,
    pub sections: Vec<SectionDiff>,
    pub entries: Vec<DiffEntry>,
}

impl FileDiff {
    /// 0 when identical, else 1.
    pub fn exit_code(&self) -> i32 {
        (!self.identical) as i32
    }
}

/// Record-level difference of two map files (`a` is the old side).
pub fn diff_files(a: &Path, b: &Path, job: &Job) -> Result<FileDiff> {
    job.stage("Comparing");
    let (ma, mb) = (load(a)?, load(b)?);
    let entries = diff(&ma, &mb);
    let mut sections: Vec<SectionDiff> = Vec::new();
    for e in &entries {
        if sections.last().map(|s| s.section) != Some(e.section) {
            let of = |k: DiffKind| entries.iter().filter(|x| x.section == e.section && x.kind == k).count();
            sections.push(SectionDiff { section: e.section, added: of(DiffKind::Added), removed: of(DiffKind::Removed), changed: of(DiffKind::Changed) });
        }
    }
    Ok(FileDiff { a: a.display().to_string(), b: b.display().to_string(), kind: "files", identical: entries.is_empty(), sections, entries })
}

/// The counts of a map that differs: records only in A, only in B, and changed, per section.
#[derive(Debug, Serialize)]
pub struct SectionCounts {
    pub section: SectionId,
    pub only_a: usize,
    pub only_b: usize,
    pub differ: usize,
}

#[derive(Debug, Serialize)]
pub struct SizeCounts {
    pub a: u32,
    pub b: u32,
}

/// `folderdiff::Counts` with the sides named A and B (A is "here", B is "there").
#[derive(Debug, Serialize)]
pub struct DiffCounts {
    pub sections: Vec<SectionCounts>,
    /// Blocked cells that differ.
    pub cells: u64,
    /// Set when the two sizes differ.
    pub size: Option<SizeCounts>,
}

#[derive(Debug, Serialize)]
pub struct FolderRow {
    pub map: u32,
    /// `same`, `differs`, `only_a`, `only_b` or `unreadable`.
    pub status: &'static str,
    pub counts: Option<DiffCounts>,
    pub message: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct FolderTotals {
    pub same: usize,
    pub differs: usize,
    pub only_a: usize,
    pub only_b: usize,
    pub unreadable: usize,
}

#[derive(Debug, Serialize)]
pub struct FolderDiff {
    pub a: String,
    pub b: String,
    /// Always `"folders"`.
    pub kind: &'static str,
    pub rows: Vec<FolderRow>,
    pub totals: FolderTotals,
    #[serde(skip)]
    raw: Vec<crate::folderdiff::Row>,
}

impl FolderDiff {
    /// 3 when a map could not be read, 1 when anything differs or is on one side only, else 0.
    pub fn exit_code(&self) -> i32 {
        let t = &self.totals;
        if t.unreadable > 0 {
            3
        } else if t.differs + t.only_a + t.only_b > 0 {
            1
        } else {
            0
        }
    }

    /// The lines of the text output: a row per map id, then the totals.
    pub fn lines(&self) -> Vec<String> {
        crate::folderdiff::report_lines(&self.raw, &|id| shaiya_clientdata::map_name(id, None))
    }
}

/// Compares the maps of two folders by id.
pub fn diff_folders(a: &Path, b: &Path, job: &Job) -> Result<FolderDiff> {
    for d in [a, b] {
        if !d.is_dir() {
            return Err(not_found(d, "not a folder"));
        }
    }
    job.stage("Comparing folders");
    let raw = crate::folderdiff::scan(a, b);
    job.check()?;
    if raw.is_empty() {
        return Err(Error::format(a.display().to_string(), None, "neither folder holds <id>.svmap files"));
    }
    let mut rows = Vec::with_capacity(raw.len());
    let mut totals = FolderTotals::default();
    for r in &raw {
        use crate::folderdiff::Status;
        let (status, counts, message) = match &r.status {
            Status::Same => ("same", None, None),
            Status::Differs(c) => (
                "differs",
                Some(DiffCounts {
                    sections: c.sections.iter().map(|(s, a, b, d)| SectionCounts { section: *s, only_a: *a, only_b: *b, differ: *d }).collect(),
                    cells: c.cells,
                    size: c.size.map(|(there, here)| SizeCounts { a: here, b: there }),
                }),
                None,
            ),
            Status::OnlyHere => ("only_a", None, None),
            Status::OnlyThere => ("only_b", None, None),
            Status::Unreadable(e) => ("unreadable", None, Some(e.clone())),
        };
        match status {
            "same" => totals.same += 1,
            "differs" => totals.differs += 1,
            "only_a" => totals.only_a += 1,
            "only_b" => totals.only_b += 1,
            _ => totals.unreadable += 1,
        }
        rows.push(FolderRow { map: r.id, status, counts, message });
    }
    Ok(FolderDiff { a: a.display().to_string(), b: b.display().to_string(), kind: "folders", rows, totals, raw })
}

// ------------------------------------------------------------------------------------------------------------------ wld-check

#[derive(Debug, Serialize)]
pub struct WldFile {
    pub path: String,
    pub bytes: usize,
    /// `identical`, `differs` or `unreadable`.
    pub status: &'static str,
    pub first_difference: Option<usize>,
    pub written: Option<usize>,
    pub reason: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct WldTotals {
    pub files: usize,
    pub identical: usize,
    pub differs: usize,
    pub unreadable: usize,
}

#[derive(Debug, Serialize)]
pub struct WldCheckReport {
    pub client: String,
    pub files: Vec<WldFile>,
    pub totals: WldTotals,
}

impl WldCheckReport {
    /// 3 when a file is unreadable (an unknown layout counts), 1 when a file differs, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.totals.unreadable > 0 {
            3
        } else if self.totals.differs > 0 {
            1
        } else {
            0
        }
    }
}

/// Reads every `world/*.wld` of a client with the lossless model and writes it back in memory.
pub fn wld_check(client: &Path, job: &Job) -> Result<WldCheckReport> {
    job.stage("Opening the client");
    let rules = open_client(client)?;
    let paths = rules.source.wld_paths();
    if paths.is_empty() {
        return Err(Error::format(client.display().to_string(), None, "no world/*.wld files"));
    }
    job.stage("Checking maps");
    job.total(Some(paths.len() as u64), Unit::Files);
    let mut files = Vec::new();
    for p in paths {
        job.check()?;
        let row = match rules.source.read(&p) {
            Err(e) => WldFile { path: p, bytes: 0, status: "unreadable", first_difference: None, written: None, reason: Some(e.to_string()) },
            Ok(bytes) => match shaiya_clientdata::wld_raw::WldRaw::parse(&bytes, &p) {
                Err(e) => WldFile { path: p, bytes: bytes.len(), status: "unreadable", first_difference: None, written: None, reason: Some(e.to_string()) },
                Ok(raw) => {
                    let back = raw.to_bytes();
                    match shaiya_clientdata::wld_raw::first_difference(&bytes, &back) {
                        None => WldFile { path: p, bytes: bytes.len(), status: "identical", first_difference: None, written: Some(back.len()), reason: None },
                        Some(at) => WldFile { path: p, bytes: bytes.len(), status: "differs", first_difference: Some(at), written: Some(back.len()), reason: None },
                    }
                }
            },
        };
        files.push(row);
        job.advance(1);
    }
    let count = |s: &str| files.iter().filter(|r| r.status == s).count();
    let totals = WldTotals { files: files.len(), identical: count("identical"), differs: count("differs"), unreadable: count("unreadable") };
    Ok(WldCheckReport { client: client.display().to_string(), files, totals })
}

// ------------------------------------------------------------------------------------------------------------------ spawns, bulk

/// The maps of a folder that are readable and have an id, and the messages of the ones that are not.
fn readable_maps(dir: &Path, job: &Job) -> Result<ReadableMaps> {
    let loaded = load_folder(dir, job)?;
    let mut maps = Vec::new();
    let mut unreadable = Vec::new();
    for (p, r) in loaded {
        match (map_id(&p), r) {
            (Some(id), Ok(m)) => maps.push((id, m)),
            (_, Err(e)) => unreadable.push(e.to_string()),
            (None, Ok(_)) => {}
        }
    }
    Ok((maps, unreadable))
}

#[derive(Debug, Serialize)]
pub struct SpawnMap {
    pub map: Option<u32>,
    pub areas: usize,
    pub monsters: u64,
    pub kinds: usize,
    pub range: Option<[u32; 2]>,
    /// Findings per group (the group keys of `spawns::Group`).
    pub findings: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Serialize)]
pub struct SpawnTotals {
    pub maps: usize,
    pub areas: usize,
    pub monsters: u64,
    pub kinds: usize,
    pub findings: BTreeMap<&'static str, usize>,
    pub never_spawned: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SpawnFinding {
    pub map: Option<u32>,
    pub group: &'static str,
    pub areas: Vec<usize>,
    pub monsters: Vec<u32>,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct SpawnsReport {
    pub maps: Vec<SpawnMap>,
    pub totals: SpawnTotals,
    pub findings: Vec<SpawnFinding>,
    /// Monster ids no area spawns; `None` without a client (or without a readable `Monster.SData`).
    pub never_spawned: Option<Vec<u32>>,
    pub unreadable: Vec<String>,
    pub notes: Vec<String>,
    /// Whether the client's monster levels were available (a level range per map).
    #[serde(skip)]
    pub levels: bool,
}

impl SpawnsReport {
    /// 3 when a file could not be read, else 0 (the report does not judge).
    pub fn exit_code(&self) -> i32 {
        if self.unreadable.is_empty() {
            0
        } else {
            3
        }
    }
}

/// The names the search and spawn engines use, from the client's tables when there is a client and as bare numbers otherwise.
fn with_env<R>(rules: Option<&ClientRules>, f: impl FnOnce(&crate::search::Env) -> R) -> R {
    let monsters = rules.and_then(|r| r.monsters.as_ref());
    let mob = |id: u32| monsters.and_then(|m| m.get(id)).map(|m| (m.name.clone(), m.level.max(0) as u32));
    let npc = |_: u32, _: u32| None;
    let name = |id: u32| id.to_string();
    let region = |_: u32, _: usize| None;
    f(&crate::search::Env { mob: &mob, npc: &npc, map_name: &name, region: &region })
}

/// The spawn report of a folder: per map the areas, monsters, kinds and level range and the spawn findings; with a client also the
/// monsters no area spawns. Nothing is judged.
pub fn spawns(folder: &Path, client: Option<&Path>, job: &Job) -> Result<SpawnsReport> {
    use crate::spawns::{analyze, never_spawned, Group, Opts};
    let rules = client.map(open_client).transpose()?;
    let notes = rules.as_ref().map(client_note_lines).unwrap_or_default();
    let (maps, unreadable) = readable_maps(folder, job)?;
    job.stage("Analysing");
    let monsters = rules.as_ref().and_then(|r| r.monsters.as_ref());
    let ins: Vec<crate::search::MapIn> = maps.iter().map(|(id, m)| crate::search::MapIn { id: Some(*id), map: m, findings: None }).collect();
    let report = with_env(rules.as_ref(), |env| analyze(&ins, env, &Opts::plain(monsters.is_some())));
    let totals = report.totals(&ins);
    let never = monsters.map(|m| never_spawned(&maps.iter().map(|(_, m)| m).collect::<Vec<_>>(), m));
    let counts = |c: &[usize; 5]| -> BTreeMap<&'static str, usize> { Group::ALL.iter().map(|g| (g.key(), c[g.index()])).collect() };
    Ok(SpawnsReport {
        maps: report
            .maps
            .iter()
            .map(|m| SpawnMap { map: m.id, areas: m.areas, monsters: m.monsters, kinds: m.kinds, range: m.band.map(|(a, b)| [a, b]), findings: counts(&m.counts) })
            .collect(),
        totals: SpawnTotals {
            maps: totals.maps,
            areas: totals.areas,
            monsters: totals.monsters,
            kinds: totals.kinds,
            findings: counts(&totals.findings),
            never_spawned: never.as_ref().map(|n| n.len()),
        },
        findings: report.findings.iter().map(|f| SpawnFinding { map: f.map, group: f.group.key(), areas: f.areas.clone(), monsters: f.mobs.clone(), text: f.text.clone() }).collect(),
        never_spawned: never,
        unreadable,
        notes,
        levels: monsters.is_some(),
    })
}

#[derive(Debug, Serialize)]
pub struct BulkMap {
    pub map: Option<u32>,
    pub matched: usize,
    pub changed: usize,
    pub entries: usize,
    pub monsters_before: u64,
    pub monsters_after: u64,
    pub held_at_8: usize,
    pub emptied: usize,
    pub deleted: usize,
}

#[derive(Debug, Serialize)]
pub struct BulkTotals {
    pub matched: usize,
    pub maps_matched: usize,
    pub changed: usize,
    pub maps_changed: usize,
    pub entries: usize,
    pub held_at_8: usize,
    pub emptied: usize,
}

#[derive(Debug, Serialize)]
pub struct BulkReport {
    /// The rule as normalised text.
    pub rule: String,
    /// Always true: a bulk rule is only ever planned, never written.
    pub dry_run: bool,
    pub maps: Vec<BulkMap>,
    pub totals: BulkTotals,
    pub unreadable: Vec<String>,
    pub notes: Vec<String>,
}

impl BulkReport {
    /// 3 when a file could not be read, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.unreadable.is_empty() {
            0
        } else {
            3
        }
    }
}

/// Parses a bulk rule (`<where> => <action>`). The command line calls this first: a rule that does not parse is a usage error.
pub fn parse_rule(text: &str) -> std::result::Result<crate::bulk::Rule, String> {
    crate::bulk::Rule::parse(text).map_err(|e| format!("rule: {e}"))
}

/// What a bulk rule would do to the monster areas of a folder, map by map. Nothing is written.
pub fn bulk(folder: &Path, rule_text: &str, client: Option<&Path>, opts: crate::bulk::Opts, job: &Job) -> Result<BulkReport> {
    let rule = parse_rule(rule_text).map_err(|e| Error::format("--rule", None, e))?;
    let rules = client.map(open_client).transpose()?;
    let notes = rules.as_ref().map(client_note_lines).unwrap_or_default();
    let (maps, unreadable) = readable_maps(folder, job)?;
    job.stage("Planning");
    let ins: Vec<crate::search::MapIn> = maps.iter().map(|(id, m)| crate::search::MapIn { id: Some(*id), map: m, findings: None }).collect();
    let plan = with_env(rules.as_ref(), |env| crate::bulk::plan(&rule, &ins, env, opts, None));
    Ok(BulkReport {
        rule: rule.text(),
        dry_run: true,
        maps: plan
            .per_map
            .iter()
            .map(|m| BulkMap {
                map: m.id,
                matched: m.matched.len(),
                changed: m.areas.len(),
                entries: m.entries,
                monsters_before: m.before,
                monsters_after: m.after,
                held_at_8: m.capped,
                emptied: m.emptied,
                deleted: m.deleted,
            })
            .collect(),
        totals: BulkTotals {
            matched: plan.matched(),
            maps_matched: plan.per_map.len(),
            changed: plan.changed(),
            maps_changed: plan.maps_changed(),
            entries: plan.entries(),
            held_at_8: plan.capped(),
            emptied: plan.emptied(),
        },
        unreadable,
        notes,
    })
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{not_lossless_wld, tiny_wld, TempDir};
    use shaiya_svmap::{MobArea, MobEntry};

    fn job() -> Job<'static> {
        Job::silent()
    }

    fn put(dir: &Path, id: u32, m: &Svmap) {
        std::fs::write(dir.join(format!("{id}.svmap")), write(m)).unwrap();
    }

    fn area_map(mobs: &[(u32, u32)]) -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas.push(MobArea { min: [1.0, 0.0, 1.0], max: [9.0, 2.0, 9.0], mobs: mobs.iter().map(|(mob, count)| MobEntry { mob: *mob, count: *count }).collect() });
        m
    }

    fn originals() -> Option<PathBuf> {
        match std::env::var_os("SVMAP_ORIGINALS") {
            Some(d) => Some(PathBuf::from(d)),
            None => {
                eprintln!("skipped: SVMAP_ORIGINALS not set");
                None
            }
        }
    }

    fn client() -> Option<PathBuf> {
        match std::env::var_os("SHAIYA_CLIENT") {
            Some(d) => Some(PathBuf::from(d)),
            None => {
                eprintln!("skipped: SHAIYA_CLIENT not set");
                None
            }
        }
    }

    /// Folder diff: identical folders, then one changed map and one missing on the other side.
    #[test]
    fn folder_diff_on_the_originals() {
        let Some(orig) = originals() else { return };
        let copy = TempDir::new("batch-folder-diff");
        for p in list_folder(&orig).unwrap() {
            std::fs::copy(&p, copy.join(p.file_name().unwrap().to_str().unwrap())).unwrap();
        }
        let same = diff_folders(&orig, copy.path(), &job()).unwrap();
        assert_eq!((same.totals.same, same.exit_code()), (77, 0), "identical folders");
        let mut m = load(&copy.join("1.svmap")).unwrap();
        m.mob_areas[0].min[0] += 3.0;
        m.mob_areas[0].max[0] += 3.0;
        std::fs::write(copy.join("1.svmap"), write(&m)).unwrap();
        std::fs::remove_file(copy.join("108.svmap")).unwrap();
        let d = diff_folders(&orig, copy.path(), &job()).unwrap();
        assert_eq!((d.totals.same, d.totals.differs, d.totals.only_a, d.totals.only_b, d.exit_code()), (75, 1, 1, 0, 1));
        let lines = d.lines();
        assert_eq!(lines.last().map(String::as_str), Some("75 same \u{b7} 1 differ \u{b7} 1 only here"), "{lines:?}");
        assert_eq!(lines.len(), 78, "one row per map and the totals");
        assert!(lines.iter().any(|l| l.trim_start().starts_with("1  differs") && l.contains("areas ~1")), "{lines:?}");
        let row = d.rows.iter().find(|r| r.map == 1).unwrap();
        assert_eq!(row.status, "differs");
        assert_eq!(row.counts.as_ref().unwrap().sections[0].differ, 1);
    }

    /// The totals the command prints for the 77 originals: with the client, and with the server's data folder as well.
    #[test]
    fn check_totals_on_the_originals() {
        let (Some(maps), Some(client)) = (originals(), client()) else { return };
        let opts = CheckOptions { client: Some(client.clone()), ..Default::default() };
        let r = check(&maps, &opts, &job()).unwrap();
        let t = &r.totals;
        println!("check totals: {} files, {} errors, {} warnings, {} info", t.files, t.errors, t.warnings, t.info);
        assert_eq!((t.files, t.unreadable, t.errors, t.warnings, t.info), (77, 0, 0, 146, 123));
        assert_eq!(r.exit_code(), 0);
        assert!(r.server_and_client.is_none());

        let server = maps.parent().unwrap().to_path_buf();
        let opts = CheckOptions { client: Some(client), server_data: Some(server), ..Default::default() };
        let r = check(&maps, &opts, &job()).unwrap();
        let t = &r.totals;
        println!("check totals with server data: {} errors, {} warnings, {} info", t.errors, t.warnings, t.info);
        assert_eq!((t.errors, t.warnings, t.info), (0, 156, 142));
        let first = &r.server_and_client.as_ref().unwrap()[0];
        let json = serde_json::to_value(first).unwrap();
        assert!(json["finding"]["code"].is_string() && json["finding"]["severity"].is_string() && json["finding"]["message"].is_string(), "{json}");
    }

    /// The usage and input errors of `check` and their kinds.
    #[test]
    fn check_errors_have_the_exit_code_kinds() {
        use shaiya_core::ErrorKind::*;
        let dir = TempDir::new("batch-check-errors");
        let none = CheckOptions::default();
        assert_eq!(check(&dir.join("missing"), &none, &job()).unwrap_err().kind(), Io, "a missing target");
        assert_eq!(check(dir.path(), &none, &job()).unwrap_err().kind(), Format, "no .svmap in the folder");
        put(dir.path(), 5, &Svmap::new(16));
        let bad = CheckOptions { server_data: Some(dir.join("no-such-server")), ..Default::default() };
        assert_eq!(check(dir.path(), &bad, &job()).unwrap_err().kind(), Io, "a missing server data folder");
        let bad = CheckOptions { client: Some(dir.join("no-such-client")), ..Default::default() };
        assert_eq!(check(dir.path(), &bad, &job()).unwrap_err().kind(), Io, "a client that cannot be opened");
        // an unreadable file is inside the report and raises the exit code to 3
        std::fs::write(dir.join("7.svmap"), [1, 2, 3]).unwrap();
        let r = check(dir.path(), &none, &job()).unwrap();
        assert_eq!((r.totals.files, r.totals.unreadable, r.exit_code()), (2, 1, 3));
        assert!(r.maps[1].read_error.as_deref().is_some_and(|e| e.contains("7.svmap")), "{:?}", r.maps[1].read_error);
        let r = check(&dir.join("7.svmap"), &none, &job()).unwrap();
        assert_eq!((r.kind, r.exit_code()), ("file", 3));
        // a file with a server data folder is refused (the command line reports it as usage first)
        let with = CheckOptions { server_data: Some(dir.path().to_path_buf()), ..Default::default() };
        assert_eq!(check(&dir.join("5.svmap"), &with, &job()).unwrap_err().kind(), Refused);
    }

    /// `check` with a client and a server data folder made in a temp dir: the notes and the cross rules come back in the report.
    #[test]
    fn check_with_a_made_client_and_server_data() {
        let dir = TempDir::new("batch-check-client");
        let maps = dir.join("maps");
        std::fs::create_dir_all(&maps).unwrap();
        put(&maps, 1, &Svmap::new(4));
        let client = dir.join("client");
        std::fs::create_dir_all(client.join("world")).unwrap();
        std::fs::write(client.join("world").join("1.wld"), tiny_wld(4).to_bytes()).unwrap();
        let server = dir.join("server");
        std::fs::create_dir_all(&server).unwrap();
        let opts = CheckOptions { client: Some(client), server_data: Some(server), ..Default::default() };
        let r = check(&maps, &opts, &job()).unwrap();
        assert_eq!(r.totals.files, 1);
        assert!(r.server_and_client.is_some());
        // no Monster.SData, no NpcQuest.SData, no sysmsg: three episode notes, each also a "client file skipped" line
        assert!(r.notes.iter().any(|n| n.starts_with("Monster.SData did not read")), "{:?}", r.notes);
        assert!(r.notes.iter().any(|n| n.starts_with("client file skipped")), "{:?}", r.notes);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["kind"], "folder");
        assert!(json["totals"]["unreadable"].is_u64() && json["server_and_client"].is_array());
    }

    /// `wld-check` on a folder made of lossless, not-lossless and broken files; and the real client's 83.
    #[test]
    fn wld_check_counts_and_exit_codes() {
        let t = TempDir::new("batch-wld-check");
        let world = t.join("world");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("1.wld"), tiny_wld(4).to_bytes()).unwrap();
        std::fs::write(world.join("2.wld"), tiny_wld(8).to_bytes()).unwrap();
        let r = wld_check(t.path(), &job()).unwrap();
        assert_eq!((r.totals.files, r.totals.identical, r.exit_code()), (2, 2, 0));
        let bad = not_lossless_wld(4);
        std::fs::write(world.join("3.wld"), &bad).unwrap();
        let r = wld_check(t.path(), &job()).unwrap();
        assert_eq!((r.totals.identical, r.totals.differs, r.exit_code()), (2, 1, 1));
        let row = r.files.iter().find(|f| f.status == "differs").unwrap();
        assert_eq!((row.bytes, row.written), (bad.len(), Some(bad.len())), "the bytes differ in one count, not in length");
        assert!(row.first_difference.is_some_and(|at| at > 0 && at < bad.len()));
        std::fs::write(world.join("4.wld"), b"FLD\0junk").unwrap();
        let r = wld_check(t.path(), &job()).unwrap();
        assert_eq!((r.totals.unreadable, r.exit_code()), (1, 3));
        assert!(r.files.iter().find(|f| f.status == "unreadable").unwrap().reason.is_some());
        assert_eq!(wld_check(&t.join("no-such"), &job()).unwrap_err().kind(), shaiya_core::ErrorKind::Io);
        let empty = TempDir::new("batch-wld-empty");
        assert_eq!(wld_check(empty.path(), &job()).unwrap_err().kind(), shaiya_core::ErrorKind::Format, "no world/*.wld");
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["totals"]["files"], 4);
        // the user's own client: all 83 are identical
        let Some(c) = client() else { return };
        let r = wld_check(&c, &job()).unwrap();
        assert_eq!((r.totals.files, r.totals.identical, r.exit_code()), (83, 83, 0), "{:?}", r.files.iter().find(|f| f.status != "identical"));
    }

    #[test]
    fn spawns_report_has_the_totals_and_needs_no_client() {
        let dir = TempDir::new("batch-spawns");
        let mut m = area_map(&[(5, 2)]);
        m.mob_areas.push(m.mob_areas[0].clone());
        put(dir.path(), 3, &m);
        let r = spawns(dir.path(), None, &job()).unwrap();
        assert_eq!((r.totals.maps, r.totals.areas, r.totals.monsters, r.totals.kinds), (1, 2, 4, 1));
        assert_eq!(r.totals.findings["duplicate"], 1);
        assert!(r.totals.never_spawned.is_none() && r.never_spawned.is_none(), "without a client there is no never-spawned count");
        assert_eq!(r.exit_code(), 0);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["totals"]["maps"], 1);
        assert_eq!(json["totals"]["findings"]["duplicate"], 1);
        assert!(json["totals"]["never_spawned"].is_null() && json["never_spawned"].is_null());
        assert_eq!(json["maps"][0]["map"], 3);
        // an unreadable file is listed and raises the exit code to 3
        std::fs::write(dir.join("4.svmap"), [1]).unwrap();
        let r = spawns(dir.path(), None, &job()).unwrap();
        assert_eq!((r.unreadable.len(), r.exit_code()), (1, 3));
        let empty = TempDir::new("batch-spawns-empty");
        assert_eq!(spawns(empty.path(), None, &job()).unwrap_err().kind(), shaiya_core::ErrorKind::Format);
    }

    /// `bulk` on the 77 originals (a copy) matches the same areas as Find's `mob:1` and writes nothing.
    #[test]
    fn bulk_dry_run_writes_nothing() {
        let Some(maps) = originals() else { return };
        let copy = TempDir::new("batch-bulk");
        let mut direct = 0usize;
        let mut hashes: BTreeMap<String, u64> = BTreeMap::new();
        let fnv = |b: &[u8]| b.iter().fold(0xcbf29ce484222325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x100000001b3));
        for p in list_folder(&maps).unwrap() {
            let bytes = std::fs::read(&p).unwrap();
            direct += read(&bytes).unwrap().mob_areas.iter().filter(|a| a.mobs.iter().any(|e| e.mob == 1)).count();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            std::fs::write(copy.join(&name), &bytes).unwrap();
            hashes.insert(name, fnv(&bytes));
        }
        assert_eq!(hashes.len(), 77);
        let r = bulk(copy.path(), "mob:1 => count +1", None, crate::bulk::Opts::default(), &job()).unwrap();
        println!("mob:1 => count +1: {} areas match on {} maps, {} would change, {} held at 8", r.totals.matched, r.totals.maps_matched, r.totals.changed, r.totals.held_at_8);
        assert_eq!(r.totals.matched, direct);
        assert_eq!(direct, 30, "Find's mob:1 count on the originals");
        assert!(r.dry_run && r.exit_code() == 0);
        assert!(bulk(copy.path(), "mob:1 => nonsense", None, crate::bulk::Opts::default(), &job()).is_err());
        for (name, h) in &hashes {
            assert_eq!(fnv(&std::fs::read(copy.join(name)).unwrap()), *h, "{name}");
        }
        assert_eq!(list_folder(copy.path()).unwrap().len(), 77, "no file was added");
    }

    #[test]
    fn bulk_plans_a_small_folder_and_reports_the_rule() {
        let dir = TempDir::new("batch-bulk-small");
        put(dir.path(), 1, &area_map(&[(1, 2)]));
        put(dir.path(), 2, &area_map(&[(7, 3)]));
        let before = std::fs::read(dir.join("1.svmap")).unwrap();
        let r = bulk(dir.path(), "mob:1 => count +1", None, crate::bulk::Opts::default(), &job()).unwrap();
        assert_eq!(r.rule, "mob:1 => count +1");
        assert_eq!((r.totals.matched, r.totals.maps_matched, r.totals.changed, r.totals.entries), (1, 1, 1, 1));
        assert_eq!((r.maps[0].map, r.maps[0].monsters_before, r.maps[0].monsters_after), (Some(1), 2, 3));
        assert_eq!(std::fs::read(dir.join("1.svmap")).unwrap(), before, "nothing is written");
        assert!(parse_rule("no arrow").is_err());
        assert_eq!(bulk(dir.path(), "no arrow", None, crate::bulk::Opts::default(), &job()).unwrap_err().kind(), shaiya_core::ErrorKind::Format);
    }

    #[test]
    fn summary_of_an_empty_map() {
        let t = summary(&Svmap::new(8), Some(3));
        assert!(t.starts_with("map 3: size 8 (64 cells), blocked 0 (0.00 %)"));
        assert!(t.contains("monster areas: 0"));
    }

    /// build, then check, diff and roundtrip over the result; and the refusals of build.
    #[test]
    fn end_to_end_build_check_diff_roundtrip() {
        use shaiya_core::ErrorKind::*;
        let guard = TempDir::new("batch-end-to-end");
        let dir = guard.path().to_path_buf();
        let mut m = Svmap::new(16);
        m.points_b.push([1.0, 2.0, 3.0]);
        put(&dir, 5, &m);
        let j = dir.join("5.json");
        let d = dump(&dir.join("5.svmap")).unwrap();
        std::fs::write(&j, d.json(&dir.join("5.svmap")).unwrap()).unwrap();
        assert_eq!(d.map, Some(5));
        let built = dir.join("built.svmap");
        let r = build(&j, &built, false, &job()).unwrap();
        assert_eq!((r.bytes, r.size, r.counts.points_b), (std::fs::read(&built).unwrap().len(), 16, 1));
        assert_eq!(std::fs::read(&built).unwrap(), std::fs::read(dir.join("5.svmap")).unwrap());
        let json = serde_json::to_value(&r).unwrap();
        assert!(json["out"].is_string() && json["counts"]["section_a"] == 0 && json["blocked"] == 0);
        // refusals: the output exists, the output is the input
        assert_eq!(build(&j, &built, false, &job()).unwrap_err().kind(), Refused);
        assert!(build(&j, &built, true, &job()).is_ok(), "--force replaces it");
        assert_eq!(build(&j, &j, true, &job()).unwrap_err().kind(), Refused);
        assert_eq!(build(&dir.join("none.json"), &built, true, &job()).unwrap_err().kind(), Io);
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        assert_eq!(build(&dir.join("bad.json"), &dir.join("out.svmap"), false, &job()).unwrap_err().kind(), Format);
        assert!(!dir.join("out.svmap").exists());
        assert!(!is_internal(&build(&dir.join("bad.json"), &dir.join("out.svmap"), false, &job()).unwrap_err()));

        let d = diff_files(&built, &dir.join("5.svmap"), &job()).unwrap();
        assert!(d.identical && d.exit_code() == 0 && d.entries.is_empty());
        std::fs::remove_file(&built).unwrap();
        std::fs::remove_file(&j).unwrap();
        std::fs::remove_file(dir.join("bad.json")).unwrap();
        let r = roundtrip(&dir, &job()).unwrap();
        assert_eq!((r.totals.files, r.totals.binary_identical, r.totals.json_identical, r.exit_code()), (1, 1, 1, 0));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["files"][0]["binary"], "identical");
        assert_eq!(check(&dir, &CheckOptions::default(), &job()).unwrap().exit_code(), 0);
        // an out-of-range point is an error: exit 1
        m.points_b.push([99.0, 0.0, 0.0]);
        put(&dir, 6, &m);
        let r = check(&dir, &CheckOptions::default(), &job()).unwrap();
        assert_eq!((r.exit_code(), r.totals.errors > 0), (1, true));
        assert!(!r.code_totals().is_empty() && r.maps.iter().any(|m| m.findings.iter().any(|f| finding_line(f).contains('['))));
        let d = diff_files(&dir.join("5.svmap"), &dir.join("6.svmap"), &job()).unwrap();
        assert!(!d.identical && d.exit_code() == 1 && !d.sections.is_empty());
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["kind"], "files");
        assert!(json["entries"][0]["section"].is_string() && json["entries"][0]["kind"].is_string());
        // a garbage file makes roundtrip fail with 3
        std::fs::write(dir.join("7.svmap"), [1, 2, 3]).unwrap();
        let r = roundtrip(&dir, &job()).unwrap();
        assert_eq!((r.totals.unreadable, r.exit_code()), (1, 3));
        assert_eq!(r.files.iter().find(|f| f.file == "7.svmap").unwrap().binary, "unreadable");
        assert_eq!(roundtrip(&dir.join("none"), &job()).unwrap_err().kind(), Io);
    }

    #[test]
    fn a_map_that_json_cannot_carry_is_a_format_error_for_dump() {
        let dir = TempDir::new("batch-dump-nan");
        let mut m = Svmap::new(8);
        m.points_b.push([f32::NAN, 0.0, 0.0]);
        put(dir.path(), 1, &m);
        let d = dump(&dir.join("1.svmap")).unwrap();
        let e = d.json(&dir.join("1.svmap")).unwrap_err();
        assert_eq!(e.kind(), shaiya_core::ErrorKind::Format);
        assert_eq!(dump(&dir.join("2.svmap")).err().map(|e| e.kind()), Some(shaiya_core::ErrorKind::Io));
        std::fs::write(dir.join("3.svmap"), [0, 0]).unwrap();
        let Err(Error::Format { offset, .. }) = dump(&dir.join("3.svmap")) else { panic!("a short file is a format error") };
        assert!(offset.is_some(), "the byte offset comes from the reader");
    }

    #[test]
    fn folder_diff_of_made_folders_and_its_json() {
        use shaiya_core::ErrorKind::*;
        let (a, b) = (TempDir::new("batch-fd-a"), TempDir::new("batch-fd-b"));
        put(a.path(), 1, &area_map(&[(1, 1)]));
        put(b.path(), 1, &area_map(&[(1, 2)]));
        put(a.path(), 2, &Svmap::new(16));
        put(b.path(), 3, &Svmap::new(16));
        std::fs::write(b.join("4.svmap"), [9]).unwrap();
        put(a.path(), 4, &Svmap::new(16));
        let r = diff_folders(a.path(), b.path(), &job()).unwrap();
        let t = &r.totals;
        assert_eq!((t.same, t.differs, t.only_a, t.only_b, t.unreadable), (0, 1, 1, 1, 1));
        assert_eq!(r.exit_code(), 3, "an unreadable map wins over the differences");
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["kind"], "folders");
        let rows = json["rows"].as_array().unwrap();
        let status: Vec<&str> = rows.iter().map(|r| r["status"].as_str().unwrap()).collect();
        assert_eq!(status, ["differs", "only_a", "only_b", "unreadable"]);
        assert!(rows[0]["counts"]["sections"][0]["differ"] == 1 && rows[3]["message"].is_string());
        assert_eq!(diff_folders(a.path(), &a.join("none"), &job()).unwrap_err().kind(), Io);
        let (e1, e2) = (TempDir::new("batch-fd-e1"), TempDir::new("batch-fd-e2"));
        assert_eq!(diff_folders(e1.path(), e2.path(), &job()).unwrap_err().kind(), Format);
    }

    /// A cancelled job stops a folder command with `Cancelled` (exit 130) before it reads anything.
    #[test]
    fn a_cancelled_job_stops_the_folder_commands() {
        let dir = TempDir::new("batch-cancel");
        put(dir.path(), 1, &Svmap::new(8));
        let token = shaiya_core::Cancel::new();
        token.cancel();
        let j = Job::new(&shaiya_core::Silent, token);
        assert_eq!(check(dir.path(), &CheckOptions::default(), &j).unwrap_err().kind(), shaiya_core::ErrorKind::Cancelled);
        assert_eq!(roundtrip(dir.path(), &j).unwrap_err().kind(), shaiya_core::ErrorKind::Cancelled);
        assert_eq!(spawns(dir.path(), None, &j).unwrap_err().kind(), shaiya_core::ErrorKind::Cancelled);
    }
}
