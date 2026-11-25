//! `shaiya anim export --format obj|bvh`: a `.3DO`, `.3DE` or `.3DC` (at rest) as Wavefront OBJ, an `.ANI` as BVH, through
//! [`shaiya_clientdata::interchange`]. Both are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte. The
//! output rules are those of the JSON export: a new file (an existing one only with `--force`), never inside a client folder, never the
//! input, written through `write_atomic_checked`. `-o` only (no `--out-dir`).

use super::gltf::export_failure;
use super::*;
use shaiya_clientdata::interchange::bvh::{export_bvh, BvhOptions};
use shaiya_clientdata::interchange::obj::{export_obj, ObjInput, ObjOptions};

/// The sentence every OBJ and BVH surface carries (rule 1.1.3).
pub(super) const LOSSY: &str = "glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes";

pub(super) fn run(ctx: &Ctx, a: &ExportArgs) -> Res {
    if a.out_dir.is_some() {
        return Err(Fail::Usage("--format obj and --format bvh take -o only (no --out-dir)".to_string()));
    }
    let bvh = a.format == ExportFormat::Bvh;
    let want = if bvh { "bvh" } else { "obj" };
    let Some(out_file) = &a.out else { return Err(Fail::Usage(format!("give -o OUT.{want}"))) };
    let [input] = a.inputs.as_slice() else { return Err(Fail::Usage("more than one input needs --format gltf or glb with --out-dir (-o names one file)".to_string())) };
    if !a.anims.is_empty() {
        return Err(Fail::Usage("--anim goes with a .3DC input and --format gltf or glb".to_string()));
    }
    if !has_ext(&out_file.to_string_lossy(), &[want]) {
        return Err(Fail::Usage(format!("{}: the output of --format {want} must end in .{want}", out_file.display())));
    }
    let names = if bvh { "an .ANI" } else { "a .3DO, .3DE or .3DC" };
    if !has_ext(input, if bvh { &["ani"][..] } else { &["3do", "3de", "3dc"][..] }) {
        return Err(Error::format(input, None, format!("--format {want} takes {names} file")).into());
    }
    if !bvh && (a.drop_translation || a.scale != 1.0) {
        return Err(Fail::Usage("--drop-translation and --scale need --format bvh".to_string()));
    }
    let archive = match &a.from {
        Some(arch) => Some(open_store(ctx, arch)?),
        None => None,
    };
    // the outputs are checked before anything is read: folder, not the input, not existing, not inside a client
    let in_path = match &archive {
        None => Some(resolve(ctx, input)?),
        Some(_) => None,
    };
    let in_paths: Vec<&Path> = in_path.iter().map(PathBuf::as_path).collect();
    check_output(ctx, out_file, a.force, &in_paths)?;
    let mtl_path = (!bvh).then(|| out_file.with_extension("mtl"));
    let (bytes, source) = match (&archive, &in_path) {
        (Some(arch), _) => (read_entry_capped(arch, input, true, MAX_MODEL_BYTES)?, input.clone()),
        (None, Some(p)) => (read_capped(p, Some(p), MAX_MODEL_BYTES)?, file_name(p)),
        (None, None) => unreachable!("a disk input is resolved above"),
    };
    let ce = clientdata_error;
    let (text, mtl, report) = if bvh {
        let ani = Ani::parse(&bytes, &source).map_err(ce)?;
        let o = export_bvh(&ani, &BvhOptions { drop_translation: a.drop_translation, scale: a.scale }).map_err(|e| export_failure(input, e))?;
        let json = json!({ "bones": o.report.bones, "frames": o.report.frames, "start_key": o.report.start_key, "dropped": o.report.dropped, "max_displacement": o.report.max_displacement, "gimbal_frames": o.report.gimbal_frames, "notes": o.report.notes });
        let line = format!("{} bones, {} frames", o.report.bones, num(o.report.frames as u64));
        (o.text, None, (json, line, o.report.notes))
    } else {
        let obj_in = if has_ext(&source, &["3do"]) {
            ObjInput::Mesh3do(Mesh3do::parse(&bytes, &source).map_err(ce)?)
        } else if has_ext(&source, &["3de"]) {
            ObjInput::Mesh3de(Mesh3de::parse(&bytes, &source).map_err(ce)?)
        } else {
            ObjInput::Mesh3dc(C3dc::parse(&bytes, &source).map_err(ce)?)
        };
        let mtl_name = mtl_path.as_ref().map(|p| file_name(p)).unwrap_or_default();
        let o = export_obj(obj_in, &ObjOptions { source: source.clone(), mtl_name }).map_err(|e| export_failure(input, e))?;
        let json = json!({ "meshes": o.report.meshes, "vertices": o.report.vertices, "mtl": mtl_path.as_ref().filter(|_| o.mtl.is_some()).map(|p| p.display().to_string()), "notes": o.report.notes });
        let line = format!("{} meshes, {} vertices", o.report.meshes, num(o.report.vertices as u64));
        (o.obj, o.mtl, (json, line, o.report.notes))
    };
    if let (Some(_), Some(p)) = (&mtl, &mtl_path) {
        check_output(ctx, p, a.force, &in_paths)?;
    }
    let bug = Cell::new(false);
    let fail = |e: Error| if bug.get() { Fail::Bug(e.to_string()) } else { Fail::Error(e) };
    // a .mtl made new for this export is removed again when the .obj beside it cannot be written (a replaced one cannot be restored)
    let mtl_is_new = mtl_path.as_ref().is_some_and(|p| !p.exists());
    if let (Some(m), Some(p)) = (&mtl, &mtl_path) {
        write_atomic_checked(p, m, &same_bytes_check(m, &bug)).map_err(fail)?;
    }
    if let Err(e) = write_atomic_checked(out_file, &text, &same_bytes_check(&text, &bug)) {
        if let (true, Some(p), Some(_)) = (mtl_is_new, &mtl_path, &mtl) {
            let _ = std::fs::remove_file(p);
        }
        return Err(fail(e));
    }
    let (json_report, line, notes) = report;
    let mtl_shown = mtl.as_ref().and(mtl_path.as_ref());
    if ctx.json {
        let mut doc = json_report;
        doc["input"] = json!(input);
        doc["format"] = json!(want);
        doc["out"] = json!(out_file.display().to_string());
        doc["bytes"] = json!(text.len());
        print_json(&doc);
    } else {
        let mut w = out();
        outln!(w, "wrote {}{}: {}, {line}, {} bytes", out_file.display(), mtl_shown.map(|m| format!(" and {}", m.display())).unwrap_or_default(), want.to_ascii_uppercase(), num(text.len() as u64));
        let _ = w.flush();
    }
    for n in &notes {
        note(ctx, &format!("note: {n}"));
    }
    note(ctx, &format!("note: {LOSSY}"));
    Ok(0)
}
