//! "Export as glTF...", "Export as OBJ..." and "Export as BVH...": one `.ANI`, `.3DC`, `.3DO` or `.3DE` file of the open archive as glTF 2.0
//! (`.gltf` with its `.bin`, or one `.glb`), a `.3DO`, `.3DE` or `.3DC` (at rest) as Wavefront OBJ (with a `.mtl` of texture names when the mesh
//! names a texture) or an `.ANI` as BVH, written to a file outside every client folder. glTF is a lossy view of the game file: the game file's own round trip (JSON) stays byte for
//! byte. A background task reads the entry (size checked first), builds the glTF with [`shaiya_clientdata::interchange::export_gltf`]
//! (which checks what it made) and writes it with `write_atomic_checked`: the temporary file is read back and must hold the bytes made. An
//! `.ANI` goes out with the mesh the Mesh box shows when one `.3DC` is chosen there, else alone (its skeleton and the clip). Nothing here
//! imports a file or writes into a client.

use crate::export_json::refusal;
use crate::ToolkitSettings;
use crate::source::FileSource;
use shaiya_archive::Entry;
use shaiya_clientdata::interchange::bvh::{export_bvh, BvhOptions};
use shaiya_clientdata::interchange::obj::{export_obj, ObjInput, ObjOptions};
use shaiya_clientdata::interchange::{export_gltf, GltfInput, GltfOptions, InterchangeError};
use shaiya_clientdata::modeljson::MAX_MODEL_BYTES;
use shaiya_clientdata::{Ani, C3dc, Mesh3de, Mesh3do};
use shaiya_core::fs::write_atomic_checked;
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// The sentence every glTF surface carries.
pub const LOSSY_SENTENCE: &str = "glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes";

/// Which interchange format an export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interchange {
    Gltf,
    Obj,
    Bvh,
}

impl Interchange {
    pub fn label(self) -> &'static str {
        match self {
            Interchange::Gltf => "glTF",
            Interchange::Obj => "OBJ",
            Interchange::Bvh => "BVH",
        }
    }

    /// The extensions of a file this format is written to.
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Interchange::Gltf => &["gltf", "glb"],
            Interchange::Obj => &["obj"],
            Interchange::Bvh => &["bvh"],
        }
    }

    /// Whether the archive path names a file this format is written from.
    pub fn takes(self, path: &str) -> bool {
        match self {
            Interchange::Gltf => is_gltf_path(path),
            Interchange::Obj => is_obj_path(path),
            Interchange::Bvh => is_bvh_path(path),
        }
    }
}

/// The running exports of one Archive tab.
#[derive(Default)]
pub(crate) struct InterchangeExports {
    running: Vec<(String, PathBuf, Interchange, TaskHandle<Vec<String>>)>,
}

/// Whether the archive path names a file glTF export takes (any letter case): `.ANI`, `.3DC`, `.3DO`, `.3DE`.
pub fn is_gltf_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(_, e)| ["ani", "3dc", "3do", "3de"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn has_ext(path: &str, exts: &[&str]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(_, e)| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether the archive path names a file OBJ export takes (any letter case): `.3DO`, `.3DE`, `.3DC`.
pub fn is_obj_path(path: &str) -> bool {
    has_ext(path, &["3do", "3de", "3dc"])
}

/// Whether the archive path names a file BVH export takes (any letter case): `.ANI`.
pub fn is_bvh_path(path: &str) -> bool {
    has_ext(path, &["ani"])
}

/// The default name in the save dialog: the file's name without its extension, plus `.gltf`.
pub(crate) fn default_name(file_name: &str) -> String {
    default_name_for(Interchange::Gltf, file_name)
}

/// The default name in the save dialog of `format`: the file's name without its extension, plus `.gltf`, `.obj` or `.bvh`.
pub(crate) fn default_name_for(format: Interchange, file_name: &str) -> String {
    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name);
    format!("{stem}.{}", format.extensions()[0])
}

fn stem(name: &str) -> String {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_string()
}

fn to_error(path: &str, e: InterchangeError) -> Error {
    if e.is_bug() {
        Error::Unsupported(format!("internal error (a bug): {e}"))
    } else {
        Error::format(path, None, e.to_string())
    }
}

fn clientdata_error(path: &str, e: shaiya_clientdata::Error) -> Error {
    match e {
        shaiya_clientdata::Error::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
        other => Error::format(path, None, other.to_string()),
    }
}

fn read_capped(archive: &dyn FileSource, path: &str) -> Result<Vec<u8>> {
    let length = archive.len(path).ok_or_else(|| Error::format(path, None, "not in the archive"))?;
    if length > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", length)));
    }
    archive.read(path, u64::MAX, &shaiya_core::Job::silent())
}

/// The check of `write_atomic_checked`: the temp file read back must hold `want`.
fn same(want: Vec<u8>) -> impl Fn(&Path) -> Result<()> {
    move |tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|e| Error::io(tmp, e))?;
        if back == want {
            Ok(())
        } else {
            Err(Error::Unsupported("internal error (a bug): the file written did not read back the same".to_string()))
        }
    }
}

/// An OBJ (and its `.mtl` of names) or a BVH: reads the entry, makes the text, writes it.
fn run_text(format: Interchange, archive: &dyn FileSource, entry: &Entry, path: &str, out: &Path) -> Result<Vec<String>> {
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    let bytes = archive.read(path, u64::MAX, &shaiya_core::Job::silent())?;
    let lower = path.to_ascii_lowercase();
    let ce = |e| clientdata_error(path, e);
    if format == Interchange::Bvh {
        let ani = Ani::parse(&bytes, path).map_err(ce)?;
        let o = export_bvh(&ani, &BvhOptions::default()).map_err(|e| to_error(path, e))?;
        write_atomic_checked(out, &o.text, &same(o.text.clone()))?;
        return Ok(o.report.notes);
    }
    let input = if lower.ends_with(".3do") {
        ObjInput::Mesh3do(Mesh3do::parse(&bytes, path).map_err(ce)?)
    } else if lower.ends_with(".3de") {
        ObjInput::Mesh3de(Mesh3de::parse(&bytes, path).map_err(ce)?)
    } else if lower.ends_with(".3dc") {
        ObjInput::Mesh3dc(C3dc::parse(&bytes, path).map_err(ce)?)
    } else {
        return Err(Error::format(path, None, "not a .3DO, .3DE or .3DC file"));
    };
    let mtl_path = out.with_extension("mtl");
    let mtl_name = mtl_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let o = export_obj(input, &ObjOptions { source: path.to_string(), mtl_name }).map_err(|e| to_error(path, e))?;
    // a .mtl that is not from an earlier export of this name is not replaced
    if o.mtl.is_some() && mtl_path.exists() && !out.exists() {
        return Err(Error::Refused(format!("{} exists: choose another name", mtl_path.display())));
    }
    // a .mtl made new for this export is removed again when the .obj beside it cannot be written (a replaced one cannot be restored)
    let mtl_is_new = !mtl_path.exists();
    if let Some(m) = &o.mtl {
        write_atomic_checked(&mtl_path, m, &same(m.clone()))?;
    }
    if let Err(e) = write_atomic_checked(out, &o.obj, &same(o.obj.clone())) {
        if let (true, Some(_)) = (mtl_is_new, &o.mtl) {
            let _ = std::fs::remove_file(&mtl_path);
        }
        return Err(e);
    }
    Ok(o.report.notes)
}

/// What the task does; returns the notes of the export. `with_mesh` is the `.3DC` an `.ANI` goes out on.
fn run(archive: &dyn FileSource, entry: &Entry, path: &str, with_mesh: Option<&str>, out: &Path) -> Result<Vec<String>> {
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    let binary = out.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb"));
    let bin_path = (!binary).then(|| out.with_extension("bin"));
    let bytes = archive.read(path, u64::MAX, &shaiya_core::Job::silent())?;
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let lower = name.to_ascii_lowercase();
    let ce = |e| clientdata_error(path, e);
    let (input, source, anim_names) = if lower.ends_with(".ani") {
        let ani = Ani::parse(&bytes, path).map_err(ce)?;
        match with_mesh {
            Some(mesh) => {
                let c3dc = C3dc::parse(&read_capped(archive, mesh)?, mesh).map_err(|e| clientdata_error(mesh, e))?;
                (GltfInput::Mesh { c3dc, anims: vec![ani] }, mesh.to_string(), vec![stem(path)])
            }
            None => (GltfInput::Anim(ani), path.to_string(), Vec::new()),
        }
    } else if lower.ends_with(".3dc") {
        (GltfInput::Mesh { c3dc: C3dc::parse(&bytes, path).map_err(ce)?, anims: Vec::new() }, path.to_string(), Vec::new())
    } else if lower.ends_with(".3do") {
        (GltfInput::Mesh3do(Mesh3do::parse(&bytes, path).map_err(ce)?), path.to_string(), Vec::new())
    } else if lower.ends_with(".3de") {
        (GltfInput::Mesh3de(Mesh3de::parse(&bytes, path).map_err(ce)?), path.to_string(), Vec::new())
    } else {
        return Err(Error::format(path, None, "not an .ANI, .3DC, .3DO or .3DE file"));
    };
    let bin_name = bin_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let opts = GltfOptions { binary, source, anim_names, bin_name, ..GltfOptions::default() };
    let o = export_gltf(input, &opts).map_err(|e| to_error(path, e))?;
    let same = |want: Vec<u8>| move |tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|e| Error::io(tmp, e))?;
        if back == want {
            Ok(())
        } else {
            Err(Error::Unsupported("internal error (a bug): the file written did not read back the same".to_string()))
        }
    };
    // a .bin made new for this export is removed again when the .gltf beside it cannot be written (a replaced one cannot be restored)
    let bin_is_new = bin_path.as_ref().is_some_and(|b| !b.exists());
    if let Some(b) = &bin_path {
        write_atomic_checked(b, &o.bin, &same(o.bin.clone()))?;
    }
    let main = o.glb.clone().unwrap_or_else(|| o.json.clone());
    if let Err(e) = write_atomic_checked(out, &main, &same(main.clone())) {
        if let (true, Some(b)) = (bin_is_new, &bin_path) {
            let _ = std::fs::remove_file(b);
        }
        return Err(e);
    }
    Ok(o.report.notes)
}

impl InterchangeExports {
    pub(crate) fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    /// Checks the target against the client folders (before anything is read), then starts the task. A refusal is shown and nothing starts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start(&mut self, format: Interchange, archive: Arc<dyn FileSource>, entry: Entry, path: String, with_mesh: Option<String>, out: PathBuf, client_dir: Option<&Path>, cx: &mut ToolkitCx<'_>) {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        if !ext.as_deref().is_some_and(|e| format.extensions().contains(&e)) {
            let want: Vec<String> = format.extensions().iter().map(|e| format!(".{e}")).collect();
            cx.shell.error("Export failed", &Error::Refused(format!("The file name must end in {}: {}", want.join(" or "), out.display())));
            return;
        }
        let saved: Vec<PathBuf> = crate::clients::guard_paths(&cx.shell.shared().clients);
        if let Some(text) = refusal(&out, client_dir, &saved) {
            cx.shell.error("Export failed", &Error::Refused(text));
            return;
        }
        // the .bin beside a .gltf is a second file: a stray one that is not from an earlier export of this name is not replaced
        if ext.as_deref() == Some("gltf") {
            let bin = out.with_extension("bin");
            if bin.exists() && !out.exists() {
                cx.shell.error("Export failed", &Error::Refused(format!("{} exists: choose another name", bin.display())));
                return;
            }
        }
        if self.running.iter().any(|(_, t, _, _)| t == &out) {
            cx.shell.error("Export failed", &Error::Refused(format!("An export to {} is running", out.display())));
            return;
        }
        let target = out.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Export {name} as {}", format.label()), cancellable: false, exclusive: None }, move |_job| match format {
            Interchange::Gltf => run(&*archive, &entry, &path, with_mesh.as_deref(), &out),
            f => run_text(f, &*archive, &entry, &path, &out),
        });
        self.running.push((name, target, format, h));
    }

    /// Finishes the exports that are done; returns the status line of the last success.
    pub(crate) fn poll(&mut self, cx: &mut ToolkitCx<'_>) -> Option<String> {
        let mut status = None;
        for (name, out, format, mut h) in std::mem::take(&mut self.running) {
            match h.poll() {
                None => self.running.push((name, out, format, h)),
                Some(TaskOutcome::Done(notes)) => {
                    let mut line = match format {
                        Interchange::Gltf => format!("Exported {name} to {} (glTF is a lossy view of the game file)", out.display()),
                        _ => format!("Exported {name} to {} ({LOSSY_SENTENCE})", out.display()),
                    };
                    if let Some(n) = notes.first() {
                        line.push_str(&format!("; {n}"));
                    }
                    cx.shell.status(line.clone());
                    status = Some(line);
                }
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Export failed", &e),
                Some(TaskOutcome::Cancelled) => cx.shell.status("Export cancelled; nothing was written"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("Export failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            }
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_kinds_are_offered_in_any_letter_case() {
        for p in ["Character/Human/ANI/a.ANI", "x/3DC/b.3dc", "Item/3DO/c.3DO", "Effect/3DE/d.3de"] {
            assert!(is_gltf_path(p), "{p}");
        }
        for p in ["a.vani", "a.mani", "a.dds", "a"] {
            assert!(!is_gltf_path(p), "{p}");
        }
        assert_eq!(default_name("huwm_torso001.3DC"), "huwm_torso001.gltf");
    }

    #[test]
    fn obj_and_bvh_take_their_own_kinds_and_names() {
        for p in ["Item/3DO/c.3DO", "Effect/3DE/d.3de", "x/3DC/b.3dc"] {
            assert!(is_obj_path(p) && Interchange::Obj.takes(p) && !is_bvh_path(p), "{p}");
        }
        for p in ["Character/Human/ANI/a.ANI", "b.ani"] {
            assert!(is_bvh_path(p) && Interchange::Bvh.takes(p) && !is_obj_path(p), "{p}");
        }
        for p in ["a.vani", "a.mani", "a.dds", "a"] {
            assert!(!Interchange::Obj.takes(p) && !Interchange::Bvh.takes(p), "{p}");
        }
        assert_eq!(default_name_for(Interchange::Obj, "crate.3DO"), "crate.obj");
        assert_eq!(default_name_for(Interchange::Bvh, "huwm_000_normal.ANI"), "huwm_000_normal.bvh");
        assert_eq!(default_name_for(Interchange::Gltf, "a.ANI"), "a.gltf");
    }
}
