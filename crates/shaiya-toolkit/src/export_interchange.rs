//! "Export as glTF...": one `.ANI`, `.3DC`, `.3DO` or `.3DE` file of the open archive as glTF 2.0 (`.gltf` with its `.bin`, or one `.glb`),
//! written to a file outside every client folder. glTF is a lossy view of the game file: the game file's own round trip (JSON) stays byte for
//! byte. A background task reads the entry (size checked first), builds the glTF with [`shaiya_clientdata::interchange::export_gltf`]
//! (which checks what it made) and writes it with `write_atomic_checked`: the temporary file is read back and must hold the bytes made. An
//! `.ANI` goes out with the mesh the Mesh box shows when one `.3DC` is chosen there, else alone (its skeleton and the clip). Nothing here
//! imports a file or writes into a client.

use crate::export_json::refusal;
use crate::ToolkitSettings;
use shaiya_archive::{Archive, Entry};
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

/// The running exports of one Archive tab.
#[derive(Default)]
pub(crate) struct InterchangeExports {
    running: Vec<(String, PathBuf, TaskHandle<Vec<String>>)>,
}

/// Whether the archive path names a file glTF export takes (any letter case): `.ANI`, `.3DC`, `.3DO`, `.3DE`.
pub fn is_gltf_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(_, e)| ["ani", "3dc", "3do", "3de"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The default name in the save dialog: the file's name without its extension, plus `.gltf`.
pub(crate) fn default_name(file_name: &str) -> String {
    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name);
    format!("{stem}.gltf")
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

fn read_capped(archive: &Archive, path: &str) -> Result<Vec<u8>> {
    let entry = archive.index().get(path).ok_or_else(|| Error::format(path, None, "not in the archive"))?;
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    archive.read(path)
}

/// What the task does; returns the notes of the export. `with_mesh` is the `.3DC` an `.ANI` goes out on.
fn run(archive: &Archive, entry: &Entry, path: &str, with_mesh: Option<&str>, out: &Path) -> Result<Vec<String>> {
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    let binary = out.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb"));
    let bin_path = (!binary).then(|| out.with_extension("bin"));
    let bytes = archive.read_entry(entry)?;
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
    pub(crate) fn start(&mut self, archive: Arc<Archive>, entry: Entry, path: String, with_mesh: Option<String>, out: PathBuf, client_dir: Option<&Path>, cx: &mut ToolkitCx<'_>) {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let ext = out.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        if !matches!(ext.as_deref(), Some("gltf" | "glb")) {
            cx.shell.error("Export failed", &Error::Refused(format!("The file name must end in .gltf or .glb: {}", out.display())));
            return;
        }
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
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
        if self.running.iter().any(|(_, t, _)| t == &out) {
            cx.shell.error("Export failed", &Error::Refused(format!("An export to {} is running", out.display())));
            return;
        }
        let target = out.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Export {name} as glTF"), cancellable: false, exclusive: None }, move |_job| run(&archive, &entry, &path, with_mesh.as_deref(), &out));
        self.running.push((name, target, h));
    }

    /// Finishes the exports that are done; returns the status line of the last success.
    pub(crate) fn poll(&mut self, cx: &mut ToolkitCx<'_>) -> Option<String> {
        let mut status = None;
        for (name, out, mut h) in std::mem::take(&mut self.running) {
            match h.poll() {
                None => self.running.push((name, out, h)),
                Some(TaskOutcome::Done(notes)) => {
                    let mut line = format!("Exported {name} to {} (glTF is a lossy view of the game file)", out.display());
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
}
