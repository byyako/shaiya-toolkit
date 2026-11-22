//! `shaiya anim ...`: describe and check animation files (`.ANI`, `.3DC`) and compare a candidate's skeleton with the file it replaces.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use shaiya_archive::Archive;
use shaiya_clientdata::anim::{passes, rig_check, rig_of, same_skeleton, validate, AniIssue, RigDiff, Severity};
use shaiya_clientdata::mesh3dc::C3dcLayout;
use shaiya_clientdata::{Ani, C3dc};
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::{Error, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Describe animation files: .ANI (bones, keys, validation) and .3DC meshes (vertices, bones)
    ///
    /// Files or folders (folders are searched for .ANI and .3DC in any letter case), or --from ARCHIVE with patterns (default *.ani and *.3dc).
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
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Files or folders; with --from, patterns over the archive's files (default *.ani and *.3dc; *.ani for validate)
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

pub fn run(ctx: &Ctx, cmd: &Cmd, _job: &shaiya_core::Job) -> i32 {
    let r = match cmd {
        Cmd::Info(a) => describe(ctx, a, false),
        Cmd::Validate(a) => describe(ctx, a, true),
        Cmd::Compare(a) => compare(ctx, a),
    };
    match r {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
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
}

impl Inputs {
    fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        match (&e.origin, &self.archive) {
            (Origin::Disk(p), _) => std::fs::read(p).map_err(|x| Error::io(p, x)),
            (Origin::Archive, Some(a)) => a.read(&e.name),
            (Origin::Archive, None) => Err(Error::Refused("no archive".to_string())),
        }
    }
}

/// True when the name ends in one of `exts` (any letter case).
fn has_ext(name: &str, exts: &[&str]) -> bool {
    Path::new(name).extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Files below `dir`, sorted by name at each level, as (name relative to `dir` with `/`, path). Folders that are symbolic links are not followed.
fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let mut entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?.collect::<std::io::Result<Vec<_>>>().map_err(|e| Error::io(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let link = std::fs::symlink_metadata(&path).map_err(|x| Error::io(&path, x))?;
        if link.file_type().is_symlink() {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                out.push((rel_name, path));
            }
        } else if link.is_dir() {
            walk(&path, &rel_name, out)?;
        } else {
            out.push((rel_name, path));
        }
    }
    Ok(())
}

fn collect(ctx: &Ctx, a: &InfoArgs, exts: &[&str]) -> Result<Inputs> {
    let mut entries = Vec::new();
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
            Ok(Inputs { entries, archive: Some(archive) })
        }
        None => {
            // the pattern is only a way to say "any letter case" to the folder search
            let filter = Filter { include: exts.iter().map(|e| Pattern::new(&format!("*.{e}"))).collect::<Result<Vec<_>>>()?, ..Filter::default() };
            for i in &a.inputs {
                let p = resolve(ctx, i)?;
                if p.is_dir() {
                    let mut found = Vec::new();
                    walk(&p, "", &mut found)?;
                    for (name, path) in found {
                        if filter.matches(&name, 0) {
                            entries.push(Entry { name, origin: Origin::Disk(path) });
                        }
                    }
                } else {
                    entries.push(Entry { name: i.clone(), origin: Origin::Disk(p) });
                }
            }
            Ok(Inputs { entries, archive: None })
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
}

fn parse(name: &str, bytes: &[u8], ani_only: bool) -> Result<Parsed> {
    if has_ext(name, &["ani"]) {
        let ani = Ani::parse(bytes, name).map_err(clientdata_error)?;
        let issues = validate(&ani);
        Ok(Parsed::Ani(ani, issues))
    } else if !ani_only && has_ext(name, &["3dc"]) {
        Ok(Parsed::C3dc(C3dc::parse(bytes, name).map_err(clientdata_error)?))
    } else if ani_only {
        Err(Error::Refused(format!("{name}: not an .ANI file (this command checks .ANI files only)")))
    } else {
        Err(Error::Refused(format!("{name}: not an .ANI or .3DC file")))
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

fn describe(ctx: &Ctx, a: &InfoArgs, validating: bool) -> Result<i32> {
    let exts: &[&str] = if validating { &["ani"] } else { &["ani", "3dc"] };
    let inputs = collect(ctx, a, exts)?;
    let mut w = out();
    let mut files = Vec::new();
    let (mut unreadable, mut errors, mut warnings, mut files_with_errors) = (0usize, 0usize, 0usize, 0usize);
    for e in &inputs.entries {
        let parsed = inputs.read(e).and_then(|b| parse(&e.name, &b, validating));
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
        }
    }
    let _ = w.flush();
    drop(w);
    if inputs.entries.is_empty() {
        note(ctx, "note: no .ANI or .3DC files found");
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
        RigDiff::FirstField { candidate, target } => json!({"kind": "first_field", "skeleton": false, "bone": null, "candidate": candidate, "target": target}),
    }
}

/// How many differences of one kind the text lists before it says "and N more".
const LISTED: usize = 10;

fn compare(ctx: &Ctx, a: &CompareArgs) -> Result<i32> {
    let cand_path = resolve(ctx, &a.candidate)?;
    let cand_bytes = std::fs::read(&cand_path).map_err(|e| Error::io(&cand_path, e))?;
    let candidate = read_ani(&a.candidate, &cand_bytes)?;
    let target_bytes = match &a.from {
        Some(arch) => {
            let archive = open_full(ctx, arch)?;
            archive.index().get(&a.target).ok_or_else(|| Error::io(&a.target, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive")))?;
            archive.read(&a.target)?
        }
        None => {
            let p = resolve(ctx, &a.target)?;
            std::fs::read(&p).map_err(|e| Error::io(&p, e))?
        }
    };
    let target = read_ani(&a.target, &target_bytes)?;

    let diffs = rig_check(&candidate, &target);
    let issues = validate(&candidate);
    let errors: Vec<&AniIssue> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    let ok = passes(&candidate, &target);
    let same_skel = same_skeleton(&diffs);
    let same_first = !diffs.iter().any(|d| !d.is_skeleton());
    if ctx.json {
        print_json(&json!({
            "candidate": a.candidate,
            "target": a.target,
            "same_skeleton": same_skel,
            "same_first_field": same_first,
            "passes": ok,
            "differences": diffs.iter().map(diff_json).collect::<Vec<_>>(),
            "candidate_issues": issues_json(&issues),
        }));
        return Ok(i32::from(!ok));
    }
    let mut w = out();
    if same_skel {
        outln!(w, "same skeleton as {}: {}, parents and reference matrices identical", a.target, plural(target.bones.len(), "bone", "bones"));
    } else {
        outln!(w, "the skeleton differs from {}:", a.target);
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
    let _ = w.flush();
    Ok(i32::from(!ok))
}
