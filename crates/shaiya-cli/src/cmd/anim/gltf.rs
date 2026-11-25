//! `shaiya anim export --format gltf|glb`: one `.ANI`, `.3DC` (with the animations that move it), `.3DO` or `.3DE` as glTF 2.0, through
//! [`shaiya_clientdata::interchange`]. glTF is a lossy view of the game file; the game file's own round trip (JSON) stays byte for byte.
//! The output rules are those of the JSON export: a new file (an existing one only with `--force`), never inside a client folder, written
//! through `write_atomic_checked`. The toolkit checks what it made before it writes (exit 5 on a failure).

use super::*;
use shaiya_clientdata::interchange::{export_gltf, GltfInput, GltfOptions, GltfOutput, InterchangeError};

const GLTF_NAMES: &str = ".ANI, .3DC, .3DO or .3DE";
const GLTF_EXTS: [&str; 4] = ["ani", "3dc", "3do", "3de"];

pub(super) fn run(ctx: &Ctx, a: &ExportArgs, job: &shaiya_core::Job) -> Res {
    match (&a.out, &a.out_dir) {
        (Some(_), Some(_)) => Err(Fail::Usage("-o and --out-dir cannot be used together".to_string())),
        (None, None) => Err(Fail::Usage("give -o OUT.gltf or OUT.glb (one input) or --out-dir DIR".to_string())),
        (Some(out), None) => match a.inputs.as_slice() {
            [input] => one(ctx, a, input, out),
            _ => Err(Fail::Usage("more than one input needs --out-dir (-o names one file)".to_string())),
        },
        (None, Some(dir)) => {
            if !a.anims.is_empty() {
                return Err(Fail::Usage("--anim needs one .3DC input and -o".to_string()));
            }
            dir_export(ctx, a, dir, job)
        }
    }
}

fn options(a: &ExportArgs, source: &str, bin_name: &str, anim_names: Vec<String>) -> GltfOptions {
    GltfOptions {
        binary: a.format == ExportFormat::Glb,
        allow_lossy: a.allow_lossy,
        pad_end: a.pad_end,
        texture_uri_base: a.texture_uri_base.clone(),
        source: source.to_string(),
        anim_names,
        bin_name: bin_name.to_string(),
    }
}

/// The input as glTF takes it, parsed from `bytes` (the reason names the file; exit 3).
fn input_of(name: &str, bytes: &[u8], anims: Vec<Ani>) -> Result<GltfInput> {
    let ce = clientdata_error;
    Ok(if has_ext(name, &["ani"]) {
        GltfInput::Anim(Ani::parse(bytes, name).map_err(ce)?)
    } else if has_ext(name, &["3dc"]) {
        GltfInput::Mesh { c3dc: C3dc::parse(bytes, name).map_err(ce)?, anims }
    } else if has_ext(name, &["3do"]) {
        GltfInput::Mesh3do(Mesh3do::parse(bytes, name).map_err(ce)?)
    } else if has_ext(name, &["3de"]) {
        GltfInput::Mesh3de(Mesh3de::parse(bytes, name).map_err(ce)?)
    } else {
        return Err(Error::format(name, None, format!("not an {GLTF_NAMES} file")));
    })
}

/// A refusal of the exporter is a problem with the input (exit 3); a failed self-check is a bug of ours (exit 5).
pub(super) fn export_failure(name: &str, e: InterchangeError) -> Fail {
    if e.is_bug() {
        Fail::Bug(e.to_string())
    } else {
        Fail::Error(Error::format(name, None, e.to_string()))
    }
}

pub(super) fn stem_of(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_string()
}

fn one(ctx: &Ctx, a: &ExportArgs, input: &str, out_file: &Path) -> Res {
    let binary = a.format == ExportFormat::Glb;
    let want = if binary { "glb" } else { "gltf" };
    if !has_ext(&out_file.to_string_lossy(), &[want]) {
        return Err(Fail::Usage(format!("{}: the output of --format {want} must end in .{want}", out_file.display())));
    }
    if !has_ext(input, &GLTF_EXTS) {
        return Err(Error::format(input, None, format!("not an {GLTF_NAMES} file")).into());
    }
    if !a.anims.is_empty() && !has_ext(input, &["3dc"]) {
        return Err(Fail::Usage("--anim goes with a .3DC input".to_string()));
    }
    for n in &a.anims {
        if !has_ext(n, &["ani"]) {
            return Err(Error::format(n, None, "not an .ANI file").into());
        }
    }
    let bin_path = (!binary).then(|| out_file.with_extension("bin"));
    let bin_name = bin_path.as_ref().map(|p| file_name(p)).unwrap_or_default();
    let archive = match &a.from {
        Some(arch) => Some(open_store(ctx, arch)?),
        None => None,
    };
    // the outputs are checked before anything is read: folder, not an input, not existing, not inside a client
    let mut disk: Vec<(String, PathBuf)> = Vec::new();
    if archive.is_none() {
        for n in std::iter::once(&input.to_string()).chain(a.anims.iter()) {
            disk.push((n.clone(), resolve(ctx, n)?));
        }
    }
    let in_paths: Vec<&Path> = disk.iter().map(|(_, p)| p.as_path()).collect();
    check_output(ctx, out_file, a.force, &in_paths)?;
    if let Some(b) = &bin_path {
        check_output(ctx, b, a.force, &in_paths)?;
    }
    let read = |name: &str| -> Result<(Vec<u8>, String)> {
        match &archive {
            Some(arch) => Ok((read_entry_capped(arch, name, true, MAX_MODEL_BYTES)?, name.to_string())),
            None => {
                let p = &disk.iter().find(|(n, _)| n == name).map(|(_, p)| p.clone()).unwrap_or_else(|| PathBuf::from(name));
                Ok((read_capped(p, Some(p), MAX_MODEL_BYTES)?, file_name(p)))
            }
        }
    };
    let (bytes, source) = read(input)?;
    let mut anims = Vec::new();
    let mut anim_names = Vec::new();
    for n in &a.anims {
        let (b, src) = read(n)?;
        anims.push(Ani::parse(&b, &src).map_err(clientdata_error)?);
        anim_names.push(stem_of(&src));
    }
    let gi = input_of(&source, &bytes, anims)?;
    let o = export_gltf(gi, &options(a, &source, &bin_name, anim_names)).map_err(|e| export_failure(input, e))?;
    let bug = Cell::new(false);
    let fail = |e: Error| if bug.get() { Fail::Bug(e.to_string()) } else { Fail::Error(e) };
    // a .bin made new for this export is removed again when the .gltf beside it cannot be written (a replaced one cannot be restored)
    let bin_is_new = bin_path.as_ref().is_some_and(|b| !b.exists());
    if let Some(b) = &bin_path {
        write_atomic_checked(b, &o.bin, &same_bytes_check(&o.bin, &bug)).map_err(fail)?;
    }
    let main: &[u8] = o.glb.as_deref().unwrap_or(&o.json);
    if let Err(e) = write_atomic_checked(out_file, main, &same_bytes_check(main, &bug)) {
        if let (true, Some(b)) = (bin_is_new, &bin_path) {
            let _ = std::fs::remove_file(b);
        }
        return Err(fail(e));
    }
    report(ctx, input, out_file, bin_path.as_deref(), &o, main.len());
    Ok(0)
}

fn report(ctx: &Ctx, input: &str, out_file: &Path, bin: Option<&Path>, o: &GltfOutput, bytes: usize) {
    let r = &o.report;
    if ctx.json {
        print_json(&json!({
            "input": input,
            "format": if bin.is_some() { "gltf" } else { "glb" },
            "out": out_file.display().to_string(),
            "bin": bin.map(|b| b.display().to_string()),
            "bytes": bytes,
            "bones": r.nodes,
            "meshes": r.meshes,
            "vertices": r.vertices,
            "animations": r.animations,
            "projected": r.projected,
            "pose_checked": r.pose_checked,
            "notes": r.notes,
        }));
    } else {
        let mut w = out();
        outln!(
            w,
            "wrote {}{}: glTF 2.0, {} bones, {} meshes, {} vertices, {} animations, {} bytes",
            out_file.display(),
            bin.map(|b| format!(" and {}", b.display())).unwrap_or_default(),
            r.nodes,
            r.meshes,
            num(r.vertices as u64),
            r.animations,
            num(bytes as u64 + o.bin.len() as u64 * u64::from(bin.is_some()))
        );
        let _ = w.flush();
    }
    for n in &r.notes {
        note(ctx, &format!("note: {n}"));
    }
    if r.projected > 0 {
        let first = r.refused.first().map(|x| format!("; first: {x}")).unwrap_or_default();
        note(ctx, &format!("note: --allow-lossy projected {} items{first}; the pose check was skipped for this file", r.projected));
    }
    note(ctx, "note: glTF is a lossy view of the game file; the JSON export stays the byte-exact round trip");
}

/// `<dir>/<rel><suffix>` (`rel` is already checked by `loose_name_problem`).
fn suffixed_target(dir: &Path, rel: &str, suffix: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    p.set_file_name(name);
    p
}

fn dir_export(ctx: &Ctx, a: &ExportArgs, dir: &Path, job: &shaiya_core::Job) -> Res {
    let saved: Vec<PathBuf> = crate::config::load_clients(ctx.data_dir.as_deref())?.into_iter().map(|c| c.path).collect();
    if let Some(client) = client_folder_for_new(dir, &saved) {
        return Err(Error::Refused(format!("{} is inside the client folder {}: write it elsewhere and put it in with Replace or archive replace", dir.display(), client.display())).into());
    }
    if dir.exists() && !dir.is_dir() {
        return Err(Error::io(dir, std::io::Error::new(std::io::ErrorKind::AlreadyExists, "not a folder")).into());
    }
    let inputs = collect(ctx, &InfoArgs { inputs: a.inputs.clone(), from: a.from.clone() }, &GLTF_EXTS)?;
    job.total(Some(inputs.entries.len() as u64), shaiya_core::Unit::Files);
    let mut written: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String, Why)> = inputs.failed.iter().map(|(p, why)| (p.clone(), why.clone(), Why::Input)).collect();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut folders_checked: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut cancelled = false;
    for e in &inputs.entries {
        if job.check().is_err() {
            cancelled = true;
            break;
        }
        match entry(a, &inputs, e, dir, &saved, &mut taken, &mut folders_checked) {
            Ok(target) => written.push(target.display().to_string()),
            Err((reason, why)) => failed.push((e.name.clone(), reason, why)),
        }
        job.advance(1);
    }
    let total = inputs.entries.len() + inputs.failed.len();
    if ctx.json {
        let list: Vec<Value> = failed.iter().map(|(input, reason, _)| json!({"input": input, "reason": reason})).collect();
        print_json(&json!({ "written": written, "failed": list, "cancelled": cancelled }));
    } else {
        let mut w = out();
        outln!(w, "{}", export_summary(written.len(), total, failed.len(), cancelled));
        let _ = w.flush();
    }
    for (input, reason, _) in &failed {
        eprintln!("{input}: FAILED: {reason}");
    }
    if cancelled {
        return Err(Error::Cancelled.into());
    }
    if total == 0 {
        note(ctx, &format!("note: no {GLTF_NAMES} files found"));
    }
    Ok(failed.iter().map(|(_, _, why)| why.code()).max().unwrap_or(0))
}

/// One file of an `--out-dir` glTF export: checks, read, export, write. `Err` is the reason and its kind.
fn entry(
    a: &ExportArgs,
    inputs: &Inputs,
    e: &Entry,
    dir: &Path,
    saved: &[PathBuf],
    taken: &mut std::collections::HashSet<String>,
    folders_checked: &mut std::collections::HashSet<PathBuf>,
) -> std::result::Result<PathBuf, (String, Why)> {
    let input = |m: String| (m, Why::Input);
    let binary = a.format == ExportFormat::Glb;
    if !has_ext(&e.rel, &GLTF_EXTS) {
        return Err(input(format!("not an {GLTF_NAMES} file")));
    }
    if let Some(problem) = shaiya_archive::edit::loose_name_problem(&e.rel) {
        return Err(input(format!("not written: {problem}")));
    }
    if let Some(problem) = out_name_problem(&e.rel) {
        return Err(input(format!("not written: {problem}")));
    }
    let target = suffixed_target(dir, &e.rel, if binary { ".glb" } else { ".gltf" });
    let bin_target = (!binary).then(|| suffixed_target(dir, &e.rel, ".bin"));
    for t in std::iter::once(&target).chain(bin_target.iter()) {
        if !taken.insert(out_key(t)) {
            return Err(("another input of this run has the same output name".to_string(), Why::Safety));
        }
        if t.exists() && !a.force {
            return Err((format!("{} exists; use --force to replace it", t.display()), Why::Safety));
        }
    }
    let bytes = inputs.read(e).map_err(|x| input(x.to_string()))?;
    let source = match &e.origin {
        Origin::Archive => e.name.clone(),
        Origin::Disk(p) => file_name(p),
    };
    let bin_name = bin_target.as_ref().map(|p| file_name(p)).unwrap_or_default();
    let gi = input_of(&source, &bytes, Vec::new()).map_err(|x| input(x.to_string()))?;
    let o = match export_gltf(gi, &options(a, &source, &bin_name, Vec::new())) {
        Ok(o) => o,
        Err(x) if x.is_bug() => return Err((x.to_string(), Why::Bug)),
        Err(x) => return Err(input(x.to_string())),
    };
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
    if !folders_checked.contains(&folder) {
        match ensure_out_folder(&folder, saved) {
            Ok(None) => {}
            Ok(Some(client)) => return Err((format!("{} is inside the client folder {}", target.display(), client.display()), Why::Safety)),
            Err(x) => return Err((x.to_string(), Why::Safety)),
        }
        folders_checked.insert(folder);
    }
    let bug = Cell::new(false);
    let why = |x: Error| (x.to_string(), if bug.get() { Why::Bug } else { Why::Safety });
    let bin_is_new = bin_target.as_ref().is_some_and(|b| !b.exists());
    if let Some(b) = &bin_target {
        write_atomic_checked(b, &o.bin, &same_bytes_check(&o.bin, &bug)).map_err(why)?;
    }
    let main: &[u8] = o.glb.as_deref().unwrap_or(&o.json);
    if let Err(e) = write_atomic_checked(&target, main, &same_bytes_check(main, &bug)) {
        if let (true, Some(b)) = (bin_is_new, &bin_target) {
            let _ = std::fs::remove_file(b);
        }
        return Err(why(e));
    }
    Ok(target)
}
