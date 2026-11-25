//! `shaiya anim import IN.gltf|IN.glb -o OUT.ani|.3dc|.3do|.3de`: a glTF 2.0 file as a new game file, through
//! [`shaiya_clientdata::interchange::import`]. The same command reads `IN.obj` (a `.3DO` or `.3DE`) and `IN.bvh` (an `.ANI`, `--like`
//! required) through [`shaiya_clientdata::interchange::obj`] and [`shaiya_clientdata::interchange::bvh`], with the same output rules and
//! checks (an `.ANI` is written only when it passes the rig check, exit 1 otherwise). glTF is a lossy view of the game file: an import builds a new file and runs the same checks as
//! `shaiya anim import` of a JSON before it writes: an `.ANI` is written only when it passes the rig check against its `--like` file (same
//! skeleton, same first field, no validation error), a `.3DC` only when its bone count is the like file's and every vertex names a bone the
//! rig has. The output rules are those of the JSON import: a new file (an existing one only with `--force`), never inside a client folder,
//! never one of the inputs, written through `write_atomic_checked` (the temporary file is read back and checked again).

use super::*;
use shaiya_clientdata::interchange::gltf_read::{buffer_name, read_beside, Gltf};
use shaiya_clientdata::interchange::bvh::import_bvh;
use shaiya_clientdata::interchange::obj::import_obj;
use shaiya_clientdata::interchange::import::{finish, import_gltf, parse_name_map, ImportError, ImportKind, ImportOptions, ImportReport, Imported, Like};

const MAX_MAP_BYTES: u64 = 1 << 20;

/// What `import` reports for a refused file: the first reasons, each with its kind.
fn reasons(e: &ImportError) -> String {
    let shown: Vec<String> = e.list.iter().take(20).map(|r| r.to_string()).collect();
    let rest = e.list.len().saturating_sub(20) + e.more;
    let mut s = format!("cannot be imported: {}", shown.join("; "));
    if rest > 0 {
        s.push_str(&format!("; and {rest} more"));
    }
    s
}

/// What the input is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Gltf,
    Obj,
    Bvh,
}

pub(super) fn run(ctx: &Ctx, a: &ImportArgs) -> Res {
    let src = if has_ext(&a.input, &["obj"]) {
        Source::Obj
    } else if has_ext(&a.input, &["bvh"]) {
        Source::Bvh
    } else {
        Source::Gltf
    };
    let out_text = a.out.to_string_lossy().into_owned();
    let Some(kind) = ImportKind::of_name(&out_text) else {
        return Err(Fail::Usage(match src {
            Source::Gltf => "-o of a glTF import must end in .ani, .3dc, .3do or .3de",
            Source::Obj => "-o of an OBJ import must end in .3do or .3de",
            Source::Bvh => "-o of a BVH import must end in .ani",
        }
        .to_string()));
    };
    match (src, kind) {
        (Source::Obj, ImportKind::Mesh3do | ImportKind::Mesh3de) | (Source::Bvh, ImportKind::Ani) | (Source::Gltf, _) => {}
        (Source::Obj, _) => return Err(Fail::Usage("-o of an OBJ import must end in .3do or .3de (an OBJ holds no skin and no keys)".to_string())),
        (Source::Bvh, _) => return Err(Fail::Usage("-o of a BVH import must end in .ani (a BVH holds no mesh)".to_string())),
    }
    if src != Source::Gltf && (a.map.is_some() || a.reduce || a.reduce_influences || a.keys.is_some()) {
        return Err(Fail::Usage("--map, --reduce, --reduce-influences and --keys are for a glTF import (IN.gltf or IN.glb) only".to_string()));
    }
    match (kind.needs_like(), &a.like) {
        (true, None) => return Err(Fail::Usage("--like is required for .ANI and .3DC: the file it will replace (or, for a .3DC, the .ANI of its rig)".to_string())),
        (false, Some(_)) => return Err(Fail::Usage("--like is for .ANI and .3DC only".to_string())),
        _ => {}
    }
    if a.like.is_none() && a.from.is_some() {
        return Err(Fail::Usage("--from needs --like (it says where the --like file is)".to_string()));
    }
    if let Some(l) = &a.like {
        if !has_ext(l, &["ani", "3dc"]) || (kind == ImportKind::Ani && !has_ext(l, &["ani"])) {
            return Err(Fail::Usage(format!("--like of a {} import must be {}", if kind == ImportKind::Ani { ".ANI" } else { ".3DC" }, if kind == ImportKind::Ani { "an .ANI file" } else { "an .ANI or .3DC file" })));
        }
    }
    if a.reduce && kind != ImportKind::Ani {
        return Err(Fail::Usage("--reduce is for .ANI only (it drops keys)".to_string()));
    }
    if a.keys.is_some() && kind != ImportKind::Ani {
        return Err(Fail::Usage("--keys exact is for .ANI only".to_string()));
    }
    if a.reduce_influences && kind != ImportKind::C3dc {
        return Err(Fail::Usage("--reduce-influences is for .3DC only".to_string()));
    }
    if a.map.is_some() && !kind.needs_like() {
        return Err(Fail::Usage("--map names the bones of the like file: it is for .ANI and .3DC only".to_string()));
    }
    if a.reduce && a.keys.is_some() {
        return Err(Fail::Usage("--reduce and --keys exact cannot be used together (exact keeps the file's own keys)".to_string()));
    }
    // the outputs are checked before anything is read
    let in_path = resolve(ctx, &a.input)?;
    let like_path = match (&a.like, &a.from) {
        (Some(l), None) => Some(resolve(ctx, l)?),
        _ => None,
    };
    let map_path = match &a.map {
        Some(m) => Some(resolve(ctx, m)?),
        None => None,
    };
    let protected: Vec<&Path> = std::iter::once(&in_path).chain(like_path.iter()).chain(map_path.iter()).map(PathBuf::as_path).collect();
    check_output(ctx, &a.out, a.force, &protected)?;
    let out_name = file_name(&a.out);
    if let Some(problem) = out_name_problem(&out_name) {
        return Err(Error::Refused(format!("{}: {problem}", a.out.display())).into());
    }
    // read, open, import
    let bytes = read_capped(&in_path, Some(&in_path), MAX_MODEL_BYTES)?;
    let gltf = if src == Source::Gltf {
        let folder = in_path.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let g = Gltf::open(&bytes, &mut |name| read_beside(&folder, name)).map_err(|(_, m)| Error::format(&a.input, None, m))?;
        // the buffer files read next to the input are inputs too: the output may not be one of them
        let buffers: Vec<PathBuf> = g.array("buffers").iter().filter_map(|b| b.get("uri").and_then(|u| u.as_str())).filter_map(|u| buffer_name(u).ok()).map(|n| folder.join(n)).collect();
        if !buffers.is_empty() {
            let all: Vec<&Path> = protected.iter().copied().chain(buffers.iter().map(PathBuf::as_path)).collect();
            check_output(ctx, &a.out, a.force, &all)?;
        }
        Some(g)
    } else {
        None
    };
    let like = match &a.like {
        None => None,
        Some(l) => {
            let like_bytes = match &like_path {
                Some(p) => read_capped(p, Some(p), MAX_MODEL_BYTES)?,
                None => read_target(ctx, l, a.from.as_deref())?,
            };
            Some(if has_ext(l, &["ani"]) { Like::Ani(read_ani(l, &like_bytes)?) } else { Like::C3dc(C3dc::parse(&like_bytes, l).map_err(clientdata_error)?) })
        }
    };
    let map = match &map_path {
        Some(p) => {
            let text = read_capped(p, Some(p), MAX_MAP_BYTES)?;
            let text = String::from_utf8(text).map_err(|_| Error::format(a.map.clone().unwrap_or_default(), None, "the map file is not UTF-8 text"))?;
            parse_name_map(&text).map_err(|m| Error::format(a.map.clone().unwrap_or_default(), None, m))?
        }
        None => Vec::new(),
    };
    let opts = ImportOptions { map, reduce_influences: a.reduce_influences, reduce: a.reduce, keys_exact: a.keys.is_some(), cancel: None };
    let refused = |e: ImportError| Error::format(&a.input, None, reasons(&e));
    let (imp, report) = match (&gltf, &like) {
        (Some(g), _) => import_gltf(g, kind, like.as_ref(), &opts).map_err(refused)?,
        (None, Some(Like::Ani(l))) => {
            let (ani, r) = import_bvh(&bytes, l, None).map_err(refused)?;
            (Imported::Ani(ani), r)
        }
        (None, _) => import_obj(&bytes, kind, None).map_err(refused)?,
    };
    // an .ANI is written only when it passes the rig check against its like file, as `anim import` does
    let mut rig = None;
    if let (Imported::Ani(candidate), Some(Like::Ani(target)), Some(l)) = (&imp, &like, &a.like) {
        let result = RigResult::new(candidate, target);
        if !ctx.json {
            let mut w = out();
            result.print(&mut w, l, candidate, target);
            if !result.ok {
                outln!(w, "nothing written");
            }
            let _ = w.flush();
        } else if !result.ok {
            let mut doc = result.json(&a.input, l);
            doc["written"] = json!(false);
            print_json(&doc);
        }
        if !result.ok {
            return Ok(1);
        }
        rig = Some(result);
    }
    let bytes = finish(&imp, like.as_ref()).map_err(|e| Error::format(&a.input, None, reasons(&e)))?;
    let bug = Cell::new(false);
    let same = same_bytes_check(&bytes, &bug);
    let check = |temp: &Path| -> Result<()> {
        same(temp)?;
        let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
        let ok = match (&imp, &like) {
            (Imported::Ani(_), Some(Like::Ani(target))) => Ani::parse(&back, &out_name).is_ok_and(|c| passes(&c, target)),
            (Imported::C3dc(_), l) => C3dc::parse(&back, &out_name).is_ok_and(|c| finish(&Imported::C3dc(c), l.as_ref()).is_ok()),
            (Imported::Mesh3do(_), _) => Mesh3do::parse(&back, &out_name).is_ok(),
            (Imported::Mesh3de(_), _) => Mesh3de::parse(&back, &out_name).is_ok(),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            bug.set(true);
            Err(Error::Refused("the file written does not pass its checks again".to_string()))
        }
    };
    if let Err(e) = write_atomic_checked(&a.out, &bytes, &check) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    print_result(ctx, a, kind, &report, bytes.len(), rig.as_ref(), src != Source::Gltf);
    Ok(0)
}

fn print_result(ctx: &Ctx, a: &ImportArgs, kind: ImportKind, r: &ImportReport, bytes: usize, rig: Option<&RigResult>, obj_or_bvh: bool) {
    let label = match kind {
        ImportKind::Ani => ".ANI",
        ImportKind::C3dc => ".3DC",
        ImportKind::Mesh3do => ".3DO",
        ImportKind::Mesh3de => ".3DE",
    };
    if ctx.json {
        let mut doc = match rig {
            Some(r) => r.json(&a.input, a.like.as_deref().unwrap_or("")),
            None => json!({}),
        };
        doc["input"] = json!(a.input);
        doc["format"] = json!(label.trim_start_matches('.').to_ascii_lowercase());
        doc["out"] = json!(a.out.display().to_string());
        doc["bytes"] = json!(bytes);
        doc["written"] = json!(true);
        doc["animation"] = json!(r.animation);
        doc["bones"] = json!(r.bones);
        doc["keys"] = json!(r.keys);
        doc["meshes"] = json!(r.meshes);
        doc["vertices"] = json!(r.vertices);
        doc["influences_reduced"] = json!(r.influences_reduced);
        doc["notes"] = json!(r.notes);
        print_json(&doc);
    } else {
        let mut w = out();
        let what = match kind {
            ImportKind::Ani => format!("{} bones, {} keys", r.bones, num(r.keys as u64)),
            _ => format!("{} meshes, {} vertices", r.meshes, num(r.vertices as u64)),
        };
        outln!(w, "wrote {}: {label}, {what}, {} bytes", a.out.display(), num(bytes as u64));
        let _ = w.flush();
    }
    if r.influences_reduced > 0 {
        note(ctx, &format!("note: {} vertices had more than 2 influences; the two largest were kept (the largest weight dropped: {:.4})", r.influences_reduced, r.max_weight_dropped));
    }
    for n in &r.notes {
        note(ctx, &format!("note: {n}"));
    }
    if obj_or_bvh {
        note(ctx, &format!("note: {}", super::objbvh::LOSSY));
    } else {
        note(ctx, "note: glTF is a lossy view of the game file; an import builds a new file and runs the same checks as `shaiya anim import` of a JSON before it writes");
    }
}
