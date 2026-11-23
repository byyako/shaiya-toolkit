//! `shaiya anim ...`: describe and check animation and model files, compare a candidate `.ANI`'s skeleton with the file it replaces, and
//! export any of the six formats to JSON and import it back (`export` and `import` write only their `-o` file).

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use shaiya_archive::Archive;
use shaiya_clientdata::anim::{passes, rig_check, rig_of, same_skeleton, validate, AniIssue, RigDiff, Severity};
use shaiya_clientdata::mesh3dc::C3dcLayout;
use shaiya_clientdata::mani::{ManiValue, MANI_FIELDS, MANI_SIZE};
use shaiya_clientdata::modeljson::{self, JsonError, ModelFormat, MAX_JSON_BYTES, MAX_MODEL_BYTES};
use shaiya_clientdata::mesh3de::De3Layout;
use shaiya_clientdata::{Ani, C3dc, Mani, Mesh3de, Mesh3do, Vani};
use shaiya_core::fs::{client_folder_above, same_file, write_atomic_checked};
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::{Error, Result};
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Describe animation and model files: .ANI (bones, keys, validation), .3DC, .VANI, .3DO and .3DE (counts, frame rates), .MANI (fields, every meaning inferred)
    ///
    /// Files or folders (folders are searched for .ani, .3dc, .vani, .mani, .3do and .3de in any letter case), or --from ARCHIVE with patterns (default: all six). `validate` and `compare` read .ANI files only.
    ///
    /// Example: shaiya anim info --from "@us" "character/human/ani/huwm_*"
    Info(InfoArgs),
    /// Check .ANI files for errors and warnings (exit 1 when a file has an error)
    ///
    /// Example: shaiya anim validate --from "@us"
    Validate(InfoArgs),
    /// Compare a candidate .ANI with the file it would replace: skeleton, first field, validation
    ///
    /// The candidate passes when it has the same skeleton (bone count, parents, reference matrices) as TARGET, the same first field
    /// (inferred to be the start key; not checked in the game) and no validation error. Exit 0 passes, 1 does not, 3 unreadable.
    ///
    /// Example: shaiya anim compare new.ani character/human/ani/huwm_117_x.ani --from "@us"
    Compare(CompareArgs),
    /// Export an animation or model file to JSON: .ANI, .3DC, .VANI, .MANI, .3DO or .3DE (the format comes from the file's extension)
    ///
    /// Writes only the -o file, as a new file (an existing one only with --force), never inside a client folder. The JSON is read back and
    /// compared with the file before anything is written; an unedited JSON imports back to the same bytes. Exit 0 written, 2 -o does not end in
    /// .json, 3 the file does not read (or is over 128 MiB), 4 the output exists, is inside a client folder or is the input, 5 a bug.
    ///
    /// Example: shaiya anim export "character/human/ani/huwm_117_x.ani" --from "@us" -o wave.json
    Export(ExportArgs),
    /// Turn a JSON made by `export` back into a file (the format comes from the JSON, the -o extension must match)
    ///
    /// An .ANI needs --like: the file it will replace. It is written only when it passes the same check as `compare` (same skeleton, same first
    /// field, no validation error); exit 1 and nothing written otherwise. Writes only the -o file, as a new file (an existing one only with
    /// --force), never inside a client folder. Put the result into a client with Replace or `archive replace`.
    ///
    /// Example: shaiya anim import wave.json -o new.ani --like "character/human/ani/huwm_117_x.ani" --from "@us"
    Import(ImportArgs),
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Files or folders; with --from, patterns over the archive's files (default: every format `info` reads; *.ani for validate)
    #[arg(value_name = "INPUT", required_unless_present = "from")]
    pub inputs: Vec<String>,
    /// Read from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
}

#[derive(clap::Args)]
pub struct CompareArgs {
    /// The candidate .ANI, a file on disk
    #[arg(value_name = "CANDIDATE")]
    pub candidate: String,
    /// The file it would replace: a file, or with --from its path in the archive
    #[arg(value_name = "TARGET")]
    pub target: String,
    /// Read TARGET from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// The file to export: a file on disk, or with --from its exact path in the archive (not a pattern)
    #[arg(value_name = "INPUT")]
    pub input: String,
    /// Read INPUT from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// The JSON file to write (must end in .json)
    #[arg(short = 'o', long = "out", value_name = "OUT.json")]
    pub out: PathBuf,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct ImportArgs {
    /// The JSON file made by `export`
    #[arg(value_name = "IN.json")]
    pub input: String,
    /// The file to write; its extension must be the JSON's format (.ANI, .3DC, .VANI, .MANI, .3DO or .3DE)
    #[arg(short = 'o', long = "out", value_name = "OUT")]
    pub out: PathBuf,
    /// For .ANI only (required there): the file the new animation will replace, a file or with --from its path in the archive
    #[arg(long, value_name = "TARGET")]
    pub like: Option<String>,
    /// Read the --like file from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
}

/// Why a command stopped: a usage error (exit 2), an error (the exit code of its kind) or a bug of the toolkit (exit 5).
enum Fail {
    Usage(String),
    Error(Error),
    Bug(String),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Fail {
        Fail::Error(e)
    }
}

type Res = std::result::Result<i32, Fail>;

fn usage(ctx: &Ctx, message: &str) -> i32 {
    if !ctx.json || !ctx.quiet {
        eprintln!("error: {message}");
    }
    if ctx.json {
        print_json(&json!({ "error": { "kind": "usage", "message": message } }));
    }
    2
}

/// The output did not read back as it was made: a bug of ours, exit 5, nothing written (an existing output is kept).
fn internal(ctx: &Ctx, message: &str) -> i32 {
    eprintln!("error: internal error (this is a bug): {message}");
    eprintln!("hint: please report it at https://github.com/byyako/shaiya-toolkit/issues and say which command you ran");
    if ctx.json {
        print_json(&json!({ "error": { "kind": "internal", "message": message } }));
    }
    5
}

/// The exit code of a stopped command (after printing why).
fn stop(ctx: &Ctx, f: &Fail) -> i32 {
    match f {
        Fail::Usage(m) => usage(ctx, m),
        Fail::Error(e) => fail(ctx, e),
        Fail::Bug(m) => internal(ctx, m),
    }
}

/// A [`JsonError`] as the reason a command stops: the toolkit's own bug (the export's self-check) is exit 5, every problem with the input is
/// an input error (exit 3) that names `name`.
fn json_failure(name: &str, e: JsonError) -> Fail {
    if e.is_bug() {
        return Fail::Bug(e.to_string());
    }
    match e {
        JsonError::Unreadable(inner) => Fail::Error(clientdata_error(inner)),
        other => Fail::Error(Error::format(name, None, other.to_string())),
    }
}

pub fn run(ctx: &Ctx, cmd: &Cmd, _job: &shaiya_core::Job) -> i32 {
    let r: Res = match cmd {
        Cmd::Info(a) => describe(ctx, a, false).map_err(Fail::from),
        Cmd::Validate(a) => describe(ctx, a, true).map_err(Fail::from),
        Cmd::Compare(a) => compare(ctx, a).map_err(Fail::from),
        Cmd::Export(a) => export(ctx, a),
        Cmd::Import(a) => import(ctx, a),
    };
    match r {
        Ok(code) => code,
        Err(f) => stop(ctx, &f),
    }
}

// ---- size caps --------------------------------------------------------------------------------------------------------------------

/// True when a file of `len` bytes is over `cap`.
fn too_large(len: u64, cap: u64) -> bool {
    len > cap
}

/// The error for a file over the cap (exit 3). It names `path` when given; `describe` prints the file's name itself.
fn cap_error(path: Option<&Path>) -> Error {
    let why = std::io::Error::other(format!("larger than {} MiB: not read", MAX_MODEL_BYTES >> 20));
    match path {
        Some(p) => Error::io(p, why),
        None => Error::from(why),
    }
}

/// Reads a file on disk unless it is over `cap` (checked from the metadata, before any byte is read).
fn read_capped(path: &Path, name: Option<&Path>, cap: u64) -> Result<Vec<u8>> {
    let len = std::fs::metadata(path).map_err(|e| Error::io(path, e))?.len();
    if too_large(len, cap) {
        return Err(cap_error(name));
    }
    std::fs::read(path).map_err(|e| Error::io(path, e))
}

/// Looks up one entry of an archive by its exact path (not in the archive: exit 3) and refuses it when it is over `cap`, from the entry's length.
fn check_entry(archive: &Archive, path: &str, named: bool, cap: u64) -> Result<()> {
    let entry = archive.index().get(path).ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive")))?;
    if too_large(u64::from(entry.length), cap) {
        return Err(cap_error(named.then(|| Path::new(path))));
    }
    Ok(())
}

/// Reads one entry of an archive by its exact path unless it is over `cap`.
fn read_entry_capped(archive: &Archive, path: &str, named: bool, cap: u64) -> Result<Vec<u8>> {
    check_entry(archive, path, named, cap)?;
    archive.read(path)
}

// ---- inputs -----------------------------------------------------------------------------------------------------------------------

enum Origin {
    Disk(PathBuf),
    Archive,
}

struct Entry {
    name: String,
    origin: Origin,
}

struct Inputs {
    entries: Vec<Entry>,
    archive: Option<Archive>,
    /// Folders that could not be listed: (path, error). They are reported as FAILED; the other files are still described.
    failed: Vec<(String, String)>,
}

impl Inputs {
    fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        match (&e.origin, &self.archive) {
            (Origin::Disk(p), _) => read_capped(p, None, MAX_MODEL_BYTES),
            (Origin::Archive, Some(a)) => read_entry_capped(a, &e.name, false, MAX_MODEL_BYTES),
            (Origin::Archive, None) => Err(Error::Refused("no archive".to_string())),
        }
    }
}

/// True when the name ends in one of `exts` (any letter case).
fn has_ext(name: &str, exts: &[&str]) -> bool {
    Path::new(name).extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Files below `dir`, sorted by name at each level, as (name relative to `dir` with `/`, path). Folders that are symbolic links are not followed.
/// A folder or entry that cannot be read is added to `failed` (path, error) and the walk goes on.
fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, failed: &mut Vec<(String, String)>) {
    let listed = std::fs::read_dir(dir).and_then(|r| r.collect::<std::io::Result<Vec<_>>>());
    let mut entries = match listed {
        Ok(v) => v,
        Err(e) => {
            failed.push((dir.display().to_string(), Error::io(dir, e).to_string()));
            return;
        }
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let link = match std::fs::symlink_metadata(&path) {
            Ok(l) => l,
            Err(x) => {
                failed.push((path.display().to_string(), Error::io(&path, x).to_string()));
                continue;
            }
        };
        if link.file_type().is_symlink() {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                out.push((rel_name, path));
            }
        } else if link.is_dir() {
            walk(&path, &rel_name, out, failed);
        } else {
            out.push((rel_name, path));
        }
    }
}

fn collect(ctx: &Ctx, a: &InfoArgs, exts: &[&str]) -> Result<Inputs> {
    let mut entries = Vec::new();
    let mut failed = Vec::new();
    match &a.from {
        Some(arch) => {
            let archive = open_full(ctx, arch)?;
            let patterns: Vec<String> = if a.inputs.is_empty() { exts.iter().map(|e| format!("*.{e}")).collect() } else { a.inputs.clone() };
            let filter = Filter::from_args(&patterns, &[], None, None)?;
            for f in archive.index().visible() {
                if filter.matches(&f.path, f.entry.length as u64) && has_ext(&f.path, exts) {
                    entries.push(Entry { name: f.path, origin: Origin::Archive });
                }
            }
            Ok(Inputs { entries, archive: Some(archive), failed: Vec::new() })
        }
        None => {
            // the pattern is only a way to say "any letter case" to the folder search
            let filter = Filter { include: exts.iter().map(|e| Pattern::new(&format!("*.{e}"))).collect::<Result<Vec<_>>>()?, ..Filter::default() };
            for i in &a.inputs {
                let p = resolve(ctx, i)?;
                if p.is_dir() {
                    let mut found = Vec::new();
                    walk(&p, "", &mut found, &mut failed);
                    for (name, path) in found {
                        if filter.matches(&name, 0) {
                            entries.push(Entry { name, origin: Origin::Disk(path) });
                        }
                    }
                } else {
                    entries.push(Entry { name: i.clone(), origin: Origin::Disk(p) });
                }
            }
            Ok(Inputs { entries, archive: None, failed })
        }
    }
}

fn clientdata_error(e: shaiya_clientdata::Error) -> Error {
    match e {
        shaiya_clientdata::Error::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
        other => Error::format("animation", None, other.to_string()),
    }
}

// ---- describe ---------------------------------------------------------------------------------------------------------------------

enum Parsed {
    Ani(Ani, Vec<AniIssue>),
    C3dc(C3dc),
    Vani(Vani),
    Mani(Mani),
    Mesh3do(Mesh3do),
    Mesh3de(Mesh3de),
}

/// The formats `anim info` reads. `validate` and `compare` read `.ANI` files only.
const INFO_EXTS: [&str; 6] = ["ani", "3dc", "vani", "mani", "3do", "3de"];

/// Refuses a name that is not a file this command describes. It runs before a named file is read, so a wrong file is never read whole.
fn check_ext(name: &str, ani_only: bool) -> Result<()> {
    if ani_only {
        if has_ext(name, &["ani"]) {
            Ok(())
        } else {
            Err(Error::Refused(format!("{name}: not an .ANI file (validate and compare read .ANI files only; `shaiya anim info` also reads .3DC, .VANI, .MANI, .3DO and .3DE)")))
        }
    } else if has_ext(name, &INFO_EXTS) {
        Ok(())
    } else {
        Err(Error::Refused(format!("{name}: not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file")))
    }
}

fn parse(name: &str, bytes: &[u8], ani_only: bool) -> Result<Parsed> {
    check_ext(name, ani_only)?;
    let e = clientdata_error;
    if has_ext(name, &["ani"]) {
        let ani = Ani::parse(bytes, name).map_err(e)?;
        let issues = validate(&ani);
        Ok(Parsed::Ani(ani, issues))
    } else if has_ext(name, &["3dc"]) {
        Ok(Parsed::C3dc(C3dc::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["vani"]) {
        Ok(Parsed::Vani(Vani::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["mani"]) {
        Ok(Parsed::Mani(Mani::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["3do"]) {
        Ok(Parsed::Mesh3do(Mesh3do::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["3de"]) {
        Ok(Parsed::Mesh3de(Mesh3de::parse(bytes, name).map_err(e)?))
    } else {
        unreachable!("check_ext accepted a name with an extension this command does not read")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", num(n as u64), if n == 1 { one } else { many })
}

fn severity_text(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn issues_json(issues: &[AniIssue]) -> Value {
    Value::Array(issues.iter().map(|i| json!({"code": i.code, "severity": severity_text(i.severity), "bone": i.bone, "message": i.message})).collect())
}

fn count_severity(issues: &[AniIssue], s: Severity) -> usize {
    issues.iter().filter(|i| i.severity == s).count()
}

/// The kind and rig an archive path says (a path on disk is not an archive path, so it says nothing).
fn kind_of(name: &str, from_archive: bool) -> Option<shaiya_clientdata::anim::RigInfo> {
    let folders = name.split(['/', '\\']).filter(|s| !s.is_empty()).count();
    (from_archive && folders > 1).then(|| rig_of(name))
}

fn ani_json(name: &str, from_archive: bool, a: &Ani, issues: &[AniIssue]) -> Value {
    let (rot, trans) = a.key_counts();
    let info = kind_of(name, from_archive);
    json!({
        "path": name,
        "format": "ani",
        "kind": info.as_ref().map(|i| i.kind.clone()),
        "rig": info.as_ref().and_then(|i| i.rig).map(|r| r.0),
        "race": info.as_ref().and_then(|i| i.rig).map(|r| r.1),
        "bones": a.bones.len(),
        "end_key": a.end_key,
        "seconds": a.end_key as f64 / 30.0,
        "rotation_keys": rot,
        "translation_keys": trans,
        "first_field": a.first_field,
        "first_field_inferred": "start key",
        "issues": issues_json(issues),
    })
}

fn ani_text(name: &str, from_archive: bool, a: &Ani, issues: &[AniIssue]) -> String {
    let (rot, trans) = a.key_counts();
    let kind = match kind_of(name, from_archive) {
        Some(i) => match i.rig {
            Some((rig, race)) => format!(", Character rig {rig} ({race})"),
            None => format!(", {}", i.kind),
        },
        None => String::new(),
    };
    let first = if a.first_field != 0 { format!(", first field {} (inferred: start key)", a.first_field) } else { String::new() };
    format!(
        "{name}: animation{kind}, {}, end key {} ({:.1} s){first}, {}, {}, {}, {}",
        plural(a.bones.len(), "bone", "bones"),
        a.end_key,
        a.end_key as f64 / 30.0,
        plural(rot, "rotation key", "rotation keys"),
        plural(trans, "translation key", "translation keys"),
        plural(count_severity(issues, Severity::Error), "error", "errors"),
        plural(count_severity(issues, Severity::Warning), "warning", "warnings"),
    )
}

fn layout_name(l: C3dcLayout) -> &'static str {
    match l {
        C3dcLayout::Skinned => "skinned",
        C3dcLayout::SkinnedWithSecondMesh => "skinned-with-second-mesh",
        C3dcLayout::Cloak => "cloak",
    }
}

fn c3dc_json(name: &str, m: &C3dc) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3dc",
        "layout": layout_name(m.layout),
        "vertices": v,
        "triangles": t,
        "bones": m.meshes.first().map_or(0, |x| x.bones.len()),
        "max_vertex_bone": m.max_vertex_bone(),
    })
}

fn c3dc_text(name: &str, m: &C3dc) -> String {
    let (v, t) = m.counts();
    let counts = format!("{}, {}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"));
    let bones = m.meshes.first().map_or(0, |x| x.bones.len());
    let highest = m.max_vertex_bone().map(|b| format!(", highest vertex bone {b}")).unwrap_or_default();
    match m.layout {
        C3dcLayout::Skinned => format!("{name}: skinned mesh, {counts}, {}{highest}", plural(bones, "bone", "bones")),
        C3dcLayout::SkinnedWithSecondMesh => format!("{name}: skinned mesh with a second mesh, {counts}, {}{highest}", plural(bones, "bone", "bones")),
        C3dcLayout::Cloak => format!("{name}: cloak mesh without bones, {counts}{highest}"),
    }
}

fn vani_json(name: &str, v: &Vani) -> Value {
    let (verts, tris) = v.counts();
    json!({
        "path": name,
        "format": "vani",
        "items": v.items.len(),
        "vertices_per_step": verts,
        "triangles": tris,
        "steps": v.step_count,
        "frame_ms": v.frame_ms,
        "frame_ms_inferred": "milliseconds per step",
        "textures": (0..v.items.len()).map(|k| v.texture_name(k)).collect::<Vec<_>>(),
        "collision_mesh": v.collision.is_some(),
    })
}

fn vani_text(name: &str, v: &Vani) -> String {
    let (verts, tris) = v.counts();
    let time = if v.frame_ms > 0 { format!("about {} ms per step (inferred)", v.frame_ms) } else { format!("frame_ms {} is not a positive time", v.frame_ms) };
    format!(
        "{name}: vertex-animated model, {}, {} per step, {}, {}, {time}",
        plural(v.items.len(), "item", "items"),
        plural(verts, "vertex", "vertices"),
        plural(tris, "triangle", "triangles"),
        plural(v.step_count, "step", "steps")
    )
}

fn mani_value_json(v: ManiValue) -> Value {
    // a float that is not finite has no JSON number: it is written as text (the raw bits are given as well)
    let f = |x: f32| if x.is_finite() { json!(x) } else { json!(x.to_string()) };
    match v {
        ManiValue::I32(x) => json!(x),
        ManiValue::U16(x) => json!(x),
        ManiValue::F32(x) => f(x),
        ManiValue::F32x3(x) => json!([f(x[0]), f(x[1]), f(x[2])]),
    }
}

fn mani_json(name: &str, m: &Mani) -> Value {
    let fields: Vec<Value> = m
        .fields()
        .into_iter()
        .map(|(f, v)| {
            let raw: String = m.raw[f.offset..f.offset + f.kind.size()].iter().map(|b| format!("{b:02x}")).collect();
            json!({"offset": f.offset, "name": f.name, "value": mani_value_json(v), "raw": raw, "meaning": f.meaning, "inferred": true})
        })
        .collect();
    json!({"path": name, "format": "mani", "size": MANI_SIZE, "fields": fields})
}

fn mani_text(name: &str, m: &Mani) -> String {
    let tick = match m.value(&MANI_FIELDS[0]) {
        ManiValue::I32(x) => x,
        _ => 0,
    };
    format!("{name}: building motion, {MANI_SIZE} bytes, {} (every meaning inferred), tick {tick} ms", plural(MANI_FIELDS.len(), "field", "fields"))
}

fn mesh3do_json(name: &str, m: &Mesh3do) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3do",
        "vertices": v,
        "triangles": t,
        "frames": m.frames.len(),
        "frame_rate": m.frame_rate,
        "frame_rate_inferred": "frames per second",
        "texture": m.texture_name(),
    })
}

fn mesh3do_text(name: &str, m: &Mesh3do) -> String {
    let (v, t) = m.counts();
    let frames = if m.frames.is_empty() { String::new() } else { format!(", {} at {} per second (inferred)", plural(m.frames.len(), "frame", "frames"), m.frame_rate) };
    format!("{name}: 3DO mesh, {}, {}{frames}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"))
}

fn mesh3de_json(name: &str, m: &Mesh3de) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3de",
        "layout": if m.layout() == De3Layout::Static { "static" } else { "standard" },
        "vertices": v,
        "triangles": t,
        "frames": m.frame_count(),
        "max_key": m.block.as_ref().map(|b| b.max_key),
        "max_key_inferred": true,
        "texture": m.texture_name(),
    })
}

fn mesh3de_text(name: &str, m: &Mesh3de) -> String {
    let (v, t) = m.counts();
    let layout = match m.layout() {
        De3Layout::Static => "static, no frame block".to_string(),
        De3Layout::Standard => plural(m.frame_count(), "frame", "frames"),
    };
    format!("{name}: effect mesh, {}, {}, {layout}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"))
}

fn describe(ctx: &Ctx, a: &InfoArgs, validating: bool) -> Result<i32> {
    let exts: &[&str] = if validating { &["ani"] } else { &INFO_EXTS };
    let inputs = collect(ctx, a, exts)?;
    let mut w = out();
    let mut files = Vec::new();
    let (mut unreadable, mut errors, mut warnings, mut files_with_errors) = (0usize, 0usize, 0usize, 0usize);
    for (path, why) in &inputs.failed {
        unreadable += 1;
        if !ctx.json {
            outln!(w, "{path}: FAILED: {why}");
        }
        files.push(json!({"path": path, "error": why}));
    }
    for e in &inputs.entries {
        let parsed = check_ext(&e.name, validating).and_then(|()| inputs.read(e)).and_then(|b| parse(&e.name, &b, validating));
        match parsed {
            Err(err) => {
                unreadable += 1;
                if !ctx.json {
                    outln!(w, "{}: FAILED: {}", e.name, err);
                }
                files.push(json!({"path": e.name, "error": err.to_string()}));
            }
            Ok(Parsed::Ani(ani, issues)) => {
                let (ne, nw) = (count_severity(&issues, Severity::Error), count_severity(&issues, Severity::Warning));
                errors += ne;
                warnings += nw;
                files_with_errors += usize::from(ne > 0);
                if validating {
                    for i in &issues {
                        let line = format!("{}: {}: {}", e.name, i.code, i.message);
                        if i.severity == Severity::Error {
                            eprintln!("error: {line}");
                        } else if !ctx.quiet {
                            eprintln!("warning: {line}");
                        }
                    }
                } else if !ctx.json {
                    outln!(w, "{}", ani_text(&e.name, matches!(e.origin, Origin::Archive), &ani, &issues));
                }
                files.push(ani_json(&e.name, matches!(e.origin, Origin::Archive), &ani, &issues));
            }
            Ok(Parsed::C3dc(m)) => {
                if !ctx.json {
                    outln!(w, "{}", c3dc_text(&e.name, &m));
                }
                files.push(c3dc_json(&e.name, &m));
            }
            Ok(Parsed::Vani(m)) => {
                if !ctx.json {
                    outln!(w, "{}", vani_text(&e.name, &m));
                }
                files.push(vani_json(&e.name, &m));
            }
            Ok(Parsed::Mani(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mani_text(&e.name, &m));
                }
                files.push(mani_json(&e.name, &m));
            }
            Ok(Parsed::Mesh3do(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mesh3do_text(&e.name, &m));
                }
                files.push(mesh3do_json(&e.name, &m));
            }
            Ok(Parsed::Mesh3de(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mesh3de_text(&e.name, &m));
                }
                files.push(mesh3de_json(&e.name, &m));
            }
        }
    }
    let _ = w.flush();
    drop(w);
    if inputs.entries.is_empty() && inputs.failed.is_empty() {
        note(ctx, if validating { "note: no .ANI files found" } else { "note: no .ANI, .3DC, .VANI, .MANI, .3DO or .3DE files found" });
    }
    if validating {
        note(
            ctx,
            &format!("{}: {} in {}, {}, {} could not be read", count(files.len(), "file"), plural(errors, "error", "errors"), plural(files_with_errors, "file", "files"), plural(warnings, "warning", "warnings"), unreadable),
        );
    }
    if ctx.json {
        let mut doc = json!({ "files": files });
        if validating {
            doc["errors"] = json!(errors);
            doc["warnings"] = json!(warnings);
        }
        print_json(&doc);
    }
    Ok(if unreadable > 0 {
        3
    } else if validating && errors > 0 {
        1
    } else {
        0
    })
}

// ---- compare ----------------------------------------------------------------------------------------------------------------------

fn read_ani(name: &str, bytes: &[u8]) -> Result<Ani> {
    Ani::parse(bytes, name).map_err(clientdata_error)
}

fn diff_json(d: &RigDiff) -> Value {
    match d {
        RigDiff::BoneCount { candidate, target } => json!({"kind": "bone_count", "skeleton": true, "bone": null, "candidate": candidate, "target": target}),
        RigDiff::Parent { bone, candidate, target } => json!({"kind": "parent", "skeleton": true, "bone": bone, "candidate": candidate, "target": target}),
        RigDiff::Matrix { bone } => json!({"kind": "matrix", "skeleton": true, "bone": bone, "candidate": null, "target": null}),
        RigDiff::FirstField { candidate, target } => json!({"kind": "first_field", "skeleton": false, "bone": null, "candidate": candidate, "target": target, "inferred": "start key"}),
    }
}

/// How many differences of one kind the text lists before it says "and N more".
const LISTED: usize = 10;

fn compare(ctx: &Ctx, a: &CompareArgs) -> Result<i32> {
    // only .ANI files are compared; a name with another extension is refused before anything is read (FAILED, exit 3, like `validate`)
    for name in [&a.candidate, &a.target] {
        if let Err(e) = check_ext(name, true) {
            let why = e.to_string();
            if ctx.json {
                print_json(&json!({"candidate": a.candidate, "target": a.target, "error": why}));
            } else {
                eprintln!("{name}: FAILED: {why}");
            }
            return Ok(3);
        }
    }
    let cand_path = resolve(ctx, &a.candidate)?;
    let cand_bytes = read_capped(&cand_path, Some(&cand_path), MAX_MODEL_BYTES)?;
    let candidate = read_ani(&a.candidate, &cand_bytes)?;
    let target_bytes = read_target(ctx, &a.target, a.from.as_deref())?;
    let target = read_ani(&a.target, &target_bytes)?;

    let rig = RigResult::new(&candidate, &target);
    if ctx.json {
        print_json(&rig.json(&a.candidate, &a.target));
        return Ok(i32::from(!rig.ok));
    }
    let mut w = out();
    rig.print(&mut w, &a.target, &candidate, &target);
    let _ = w.flush();
    Ok(i32::from(!rig.ok))
}

/// TARGET of `compare` and `--like` of `import`: a file on disk, or with `from` its exact path in the archive (over the size cap: exit 3).
fn read_target(ctx: &Ctx, target: &str, from: Option<&str>) -> Result<Vec<u8>> {
    match from {
        Some(arch) => {
            let archive = open_full(ctx, arch)?;
            read_entry_capped(&archive, target, true, MAX_MODEL_BYTES)
        }
        None => {
            let p = resolve(ctx, target)?;
            read_capped(&p, Some(&p), MAX_MODEL_BYTES)
        }
    }
}

/// What the rig check says about a candidate `.ANI` and the file it would replace: shared by `compare` and `import`, so both print the same lines.
struct RigResult {
    diffs: Vec<RigDiff>,
    issues: Vec<AniIssue>,
    ok: bool,
}

impl RigResult {
    fn new(candidate: &Ani, target: &Ani) -> RigResult {
        RigResult { diffs: rig_check(candidate, target), issues: validate(candidate), ok: passes(candidate, target) }
    }

    /// The keys of `compare`'s JSON (`import` adds its own).
    fn json(&self, candidate: &str, target: &str) -> Value {
        json!({
            "candidate": candidate,
            "target": target,
            "same_skeleton": same_skeleton(&self.diffs),
            "same_first_field": !self.diffs.iter().any(|d| !d.is_skeleton()),
            "passes": self.ok,
            "differences": self.diffs.iter().map(diff_json).collect::<Vec<_>>(),
            "candidate_issues": issues_json(&self.issues),
        })
    }

    /// The text of `compare`: the skeleton lines, the first-field line, the candidate's validation errors, and `passes` or `does not pass`.
    fn print(&self, w: &mut impl Write, target_name: &str, candidate: &Ani, target: &Ani) {
        print_rig_result(w, target_name, candidate, target, &self.diffs, &self.issues, self.ok);
    }
}

fn print_rig_result(w: &mut impl Write, target_name: &str, candidate: &Ani, target: &Ani, diffs: &[RigDiff], issues: &[AniIssue], ok: bool) {
    let errors: Vec<&AniIssue> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    if same_skeleton(diffs) {
        outln!(w, "same skeleton as {}: {}, parents and reference matrices identical", target_name, plural(target.bones.len(), "bone", "bones"));
    } else {
        outln!(w, "the skeleton differs from {}:", target_name);
        let counts: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::BoneCount { .. })).collect();
        let parents: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::Parent { .. })).collect();
        let matrices: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::Matrix { .. })).collect();
        for (list, what) in [(counts, "bone count"), (parents, "parent"), (matrices, "reference matrix")] {
            for d in list.iter().take(LISTED) {
                match d {
                    RigDiff::BoneCount { candidate, target } => outln!(w, "  bone count: candidate {candidate}, target {target}"),
                    RigDiff::Parent { bone, candidate, target } => outln!(w, "  bone {bone}: parent is {candidate}, target {target}"),
                    RigDiff::Matrix { bone } => outln!(w, "  bone {bone}: reference matrix differs"),
                    RigDiff::FirstField { .. } => {}
                }
            }
            if list.len() > LISTED {
                outln!(w, "  and {} more {what} differences", num((list.len() - LISTED) as u64));
            }
        }
    }
    match diffs.iter().find_map(|d| if let RigDiff::FirstField { candidate, target } = d { Some((*candidate, *target)) } else { None }) {
        Some((c, t)) => outln!(w, "first field differs: candidate {c}, target {t} (inferred: the start key; not checked in the game)"),
        None => outln!(w, "first field: same ({})", candidate.first_field),
    }
    if !errors.is_empty() {
        outln!(w, "the candidate has {}:", plural(errors.len(), "validation error", "validation errors"));
        for i in &errors {
            outln!(w, "  {}: {}", i.code, i.message);
        }
    }
    outln!(w, "{}", if ok { "passes" } else { "does not pass" });
}

// ---- export and import ------------------------------------------------------------------------------------------------------------

const MODEL_NAMES: &str = ".ANI, .3DC, .VANI, .MANI, .3DO or .3DE";

/// The checks every `-o` of `export` and `import` goes through, in this order: the folder exists (exit 3), `out` is not one of `inputs`
/// (exit 4), `out` does not exist unless `--force` (exit 4), `out` is not inside a client folder (exit 4: a folder holding `data.sah`, or one of
/// the saved client folders, above it or through a link).
fn check_output(ctx: &Ctx, out: &Path, force: bool, inputs: &[&Path]) -> Result<()> {
    let folder = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if !folder.is_dir() {
        return Err(Error::io(&folder, std::io::Error::new(std::io::ErrorKind::NotFound, "folder does not exist")));
    }
    if let Some(i) = inputs.iter().find(|i| same_file(i, out)) {
        return Err(Error::Refused(format!("{} is the input {}: write the output to another file", out.display(), i.display())));
    }
    if out.exists() && !force {
        return Err(Error::Refused(format!("{} exists; use --force to replace it", out.display())));
    }
    let saved: Vec<PathBuf> = crate::config::load_clients(ctx.data_dir.as_deref())?.into_iter().map(|c| c.path).collect();
    if let Some(client) = client_folder_above(out, &saved) {
        return Err(Error::Refused(format!(
            "{} is inside the client folder {}: write it elsewhere and put it in with Replace or archive replace",
            out.display(),
            client.display()
        )));
    }
    Ok(())
}

/// The check of `write_atomic_checked`: the temp file read back from disk must hold exactly `want`. A difference sets `bug` (exit 5).
fn same_bytes_check<'a>(want: &'a [u8], bug: &'a Cell<bool>) -> impl Fn(&Path) -> Result<()> + 'a {
    move |temp| {
        let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
        if back == want {
            Ok(())
        } else {
            bug.set(true);
            Err(Error::Refused("the file written did not read back the same".to_string()))
        }
    }
}

fn export(ctx: &Ctx, a: &ExportArgs) -> Res {
    let Some(format) = ModelFormat::from_file_name(&a.input) else {
        return Err(Error::format(&a.input, None, format!("not an {MODEL_NAMES} file")).into());
    };
    if !has_ext(&a.out.to_string_lossy(), &["json"]) {
        return Err(Fail::Usage(format!("{}: the output of export must end in .json", a.out.display())));
    }
    // the size is checked before the file is read; the output is checked before anything is read or written
    let (bytes, source) = match &a.from {
        Some(arch) => {
            let archive = open_full(ctx, arch)?;
            check_entry(&archive, &a.input, true, MAX_MODEL_BYTES)?;
            check_output(ctx, &a.out, a.force, &[])?;
            (archive.read(&a.input)?, a.input.clone())
        }
        None => {
            let p = resolve(ctx, &a.input)?;
            let len = std::fs::metadata(&p).map_err(|e| Error::io(&p, e))?.len();
            if too_large(len, MAX_MODEL_BYTES) {
                return Err(cap_error(Some(&p)).into());
            }
            check_output(ctx, &a.out, a.force, &[&p])?;
            let bytes = std::fs::read(&p).map_err(|e| Error::io(&p, e))?;
            (bytes, file_name(&p))
        }
    };
    let text = modeljson::export(format, &bytes, &source).map_err(|e| json_failure(&a.input, e))?;
    let bug = Cell::new(false);
    if let Err(e) = write_atomic_checked(&a.out, text.as_bytes(), &same_bytes_check(text.as_bytes(), &bug)) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    if ctx.json {
        print_json(&json!({ "input": a.input, "format": format.extension(), "schema": format.schema(), "out": a.out.display().to_string(), "bytes": text.len() }));
    } else {
        let mut w = out();
        outln!(w, "wrote {}: {} as {}, {} bytes of JSON", a.out.display(), format.label(), format.schema(), num(text.len() as u64));
        let _ = w.flush();
    }
    Ok(0)
}

fn import(ctx: &Ctx, a: &ImportArgs) -> Res {
    let in_path = resolve(ctx, &a.input)?;
    let len = std::fs::metadata(&in_path).map_err(|e| Error::io(&in_path, e))?.len();
    if too_large(len, MAX_JSON_BYTES) {
        return Err(Error::io(&in_path, std::io::Error::other(format!("larger than {} MiB: not read", MAX_JSON_BYTES >> 20))).into());
    }
    let json_bytes = std::fs::read(&in_path).map_err(|e| Error::io(&in_path, e))?;
    let format = modeljson::format_of(&json_bytes).map_err(|e| json_failure(&a.input, e))?;
    let out_text = a.out.to_string_lossy().into_owned();
    if ModelFormat::from_file_name(&out_text) != Some(format) {
        let asks = match ModelFormat::from_file_name(&out_text) {
            Some(f) => f.label().to_string(),
            None => a.out.extension().map_or_else(|| "no extension".to_string(), |e| format!(".{}", e.to_string_lossy())),
        };
        return Err(Error::format(&a.input, None, format!("holds {}, but {} asks for {asks}", format.schema(), a.out.display())).into());
    }
    let is_ani = format == ModelFormat::Ani;
    match (&a.like, is_ani) {
        (None, true) => return Err(Fail::Usage("--like is required for .ANI: the file it will replace".to_string())),
        (Some(_), false) => return Err(Fail::Usage("--like is for .ANI only".to_string())),
        (None, false) if a.from.is_some() => return Err(Fail::Usage("--from needs --like (it says where the --like file is)".to_string())),
        _ => {}
    }
    // the --like file on disk is also protected from being the output
    let like_path = match (&a.like, &a.from) {
        (Some(l), None) => Some(resolve(ctx, l)?),
        _ => None,
    };
    let mut protected: Vec<&Path> = vec![&in_path];
    if let Some(l) = &like_path {
        protected.push(l);
    }
    check_output(ctx, &a.out, a.force, &protected)?;

    let (_, bytes) = modeljson::import(&json_bytes).map_err(|e| json_failure(&a.input, e))?;
    let out_name = file_name(&a.out);
    let mut rig = None;
    if let (true, Some(like)) = (is_ani, &a.like) {
        let target_bytes = match &like_path {
            Some(p) => read_capped(p, Some(p), MAX_MODEL_BYTES)?,
            None => read_target(ctx, like, a.from.as_deref())?,
        };
        let target = read_ani(like, &target_bytes)?;
        let candidate = read_ani(&out_name, &bytes)?;
        let result = RigResult::new(&candidate, &target);
        if !ctx.json {
            let mut w = out();
            result.print(&mut w, like, &candidate, &target);
            if !result.ok {
                outln!(w, "nothing written");
            }
            let _ = w.flush();
        } else if !result.ok {
            let mut doc = result.json(&a.input, like);
            doc["written"] = json!(false);
            print_json(&doc);
        }
        if !result.ok {
            return Ok(1);
        }
        rig = Some((result, target));
    }
    let bug = Cell::new(false);
    let same = same_bytes_check(&bytes, &bug);
    let check = |temp: &Path| -> Result<()> {
        same(temp)?;
        if let Some((_, target)) = &rig {
            // the file on disk must read as an .ANI that passes again
            let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
            let ok = Ani::parse(&back, &out_name).is_ok_and(|c| passes(&c, target));
            if !ok {
                bug.set(true);
                return Err(Error::Refused("the file written does not pass the rig check".to_string()));
            }
        }
        Ok(())
    };
    if let Err(e) = write_atomic_checked(&a.out, &bytes, &check) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    if ctx.json {
        let mut doc = match &rig {
            Some((r, _)) => r.json(&a.input, a.like.as_deref().unwrap_or("")),
            None => json!({}),
        };
        doc["input"] = json!(a.input);
        doc["format"] = json!(format.extension());
        doc["out"] = json!(a.out.display().to_string());
        doc["bytes"] = json!(bytes.len());
        doc["written"] = json!(true);
        print_json(&doc);
    } else {
        let mut w = out();
        outln!(w, "wrote {}: {}, {} bytes", a.out.display(), format.label(), num(bytes.len() as u64));
        let _ = w.flush();
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx { data_dir: None, json: false, quiet: true, verbose: 0, progress: crate::progress::Mode::Never }
    }

    #[test]
    fn too_large_is_a_strict_comparison() {
        assert!(!too_large(10, 10) && too_large(11, 10) && !too_large(0, 0));
        assert_eq!(MAX_MODEL_BYTES, 128 << 20);
        assert!(cap_error(None).to_string() == "larger than 128 MiB: not read", "{}", cap_error(None));
    }

    /// The cap is checked from the length before a byte is read: a small cap refuses a ten-byte file with the message `info` prints after FAILED.
    #[test]
    fn info_refuses_a_file_over_the_size_cap() {
        let dir = std::env::temp_dir().join(format!("shaiya-anim-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.ani");
        std::fs::write(&f, [0u8; 10]).unwrap();
        assert_eq!(read_capped(&f, None, 10).unwrap().len(), 10);
        let e = read_capped(&f, None, 9).unwrap_err();
        assert_eq!(e.to_string(), "larger than 128 MiB: not read");
        assert_eq!(crate::exit_code_for(e.kind()), 3);
        let named = read_capped(&f, Some(&f), 9).unwrap_err().to_string();
        assert!(named.ends_with("big.ani: larger than 128 MiB: not read"), "{named}");
        assert!(read_capped(&dir.join("missing.ani"), None, 9).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The bug path (exit 5) cannot be provoked end to end: the mapper is called with the self-check error.
    #[test]
    fn a_self_check_error_is_a_bug_and_exits_5() {
        let bug = json_failure("a.ani", JsonError::SelfCheck("x".to_string()));
        assert!(matches!(bug, Fail::Bug(_)));
        assert_eq!(stop(&ctx(), &bug), 5);
        // a problem with the input is an input error (exit 3), also a refused value or a cap
        for e in [JsonError::Schema("x".into()), JsonError::TooLarge("x".into()), JsonError::NotWritable("x".into()), JsonError::NotFinite { at: "k".into() }] {
            let f = json_failure("a.json", e);
            assert!(matches!(f, Fail::Error(_)));
            assert_eq!(stop(&ctx(), &f), 3);
        }
        assert_eq!(stop(&ctx(), &Fail::Usage("x".into())), 2);
    }

    /// A folder that cannot be listed is reported and the walk goes on. (A path that cannot be listed is simulated by a file where a folder is
    /// expected: the same `read_dir` error path as a permission failure, which cannot be made portably.)
    #[test]
    fn a_folder_that_cannot_be_listed_is_reported_and_the_walk_goes_on() {
        let dir = std::env::temp_dir().join(format!("shaiya-anim-walk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ani"), b"x").unwrap();
        let (mut found, mut failed) = (Vec::new(), Vec::new());
        walk(&dir.join("a.ani"), "", &mut found, &mut failed);
        assert!(found.is_empty());
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].0.ends_with("a.ani"), "{failed:?}");
        walk(&dir, "", &mut found, &mut failed);
        assert_eq!(found.len(), 1);
        assert_eq!(failed.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
