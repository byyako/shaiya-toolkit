//! "Import model/animation...": a glTF 2.0 file (`.gltf` with its `.bin`, or `.glb`), a Wavefront OBJ (`.obj`) or a BVH (`.bvh`) as a new
//! `.ANI`, `.3DC`, `.3DO` or `.3DE` file. The row it is opened from says what is made: an `.ANI` or `.3DC` row is also the like file (the
//! skeleton, reference matrices and first field come from it), a `.3DO` or `.3DE` row only names the kind. An OBJ makes a `.3DO` or `.3DE`, a
//! BVH an `.ANI` (it needs the like file of its row). A background task reads the file (a glTF's buffers only from a data uri or from the same
//! folder; an OBJ's `mtllib` is never opened), imports it with [`shaiya_clientdata::interchange`] and runs the checks of `shaiya anim import`:
//! an `.ANI` must pass against its like file, a `.3DC` must fit it, every game file is read back from its own bytes. The window shows what the
//! import did or every reason it refused; nothing is written until "Save as new file..." (a new file outside every client folder, written
//! with the read-back check). glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte.

use crate::export_interchange::LOSSY_SENTENCE;
use crate::ToolkitSettings;
use shaiya_archive::Archive;
use shaiya_clientdata::anim::{passes, rig_check, validate, RigDiff, Severity};
use shaiya_clientdata::interchange::bvh::import_bvh;
use shaiya_clientdata::interchange::gltf_read::{read_beside, Gltf};
use shaiya_clientdata::interchange::obj::import_obj;
use shaiya_clientdata::interchange::import::{finish, import_gltf, parse_name_map, ImportError, ImportKind, ImportOptions, ImportReport, Imported, Like};
use shaiya_clientdata::modeljson::MAX_MODEL_BYTES;
use shaiya_clientdata::{Ani, C3dc, Mesh3de, Mesh3do};
use shaiya_core::fs::{out_name_problem, same_file, write_atomic_checked};
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// The most reasons the window lists; the rest are counted.
const SHOWN: usize = 20;

/// The settings of one import (the window's checkboxes, and what a test sets).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// The file to read: `.gltf`, `.glb`, `.obj` or `.bvh`.
    pub input: Option<PathBuf>,
    /// A `--map` file: lines `node name = bone index`.
    pub map: Option<PathBuf>,
    pub reduce_influences: bool,
    pub reduce: bool,
    pub keys_exact: bool,
}

/// A finished import: the bytes of the new file and what was done.
#[derive(Clone, Debug)]
pub struct Made {
    pub kind: ImportKind,
    pub bytes: Vec<u8>,
    pub report: ImportReport,
    /// The like file (for the read-back check of the save).
    like: Option<Like>,
}

impl Made {
    /// One line about the result, as the window shows it.
    pub fn summary(&self) -> String {
        let r = &self.report;
        let label = label(self.kind);
        match self.kind {
            ImportKind::Ani => format!("{label}: {} bones, {} keys, {} bytes; it passes the rig check against its like file", r.bones, r.keys, self.bytes.len()),
            _ => format!("{label}: {} meshes, {} vertices, {} bytes", r.meshes, r.vertices, self.bytes.len()),
        }
    }
}

/// Why an import made nothing: the lines the window shows (every reason, at most [`SHOWN`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub lines: Vec<String>,
    /// True for an `.ANI` that was read but does not pass the rig check (the CLI's exit 1); false for a file that cannot be imported.
    pub does_not_pass: bool,
}

/// What the chosen file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Gltf,
    Obj,
    Bvh,
}

impl Source {
    /// The kind of a file name (`.obj` and `.bvh` in any letter case; anything else is read as glTF).
    pub fn of(path: &Path) -> Source {
        match path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
            Some("obj") => Source::Obj,
            Some("bvh") => Source::Bvh,
            _ => Source::Gltf,
        }
    }
}

fn label(kind: ImportKind) -> &'static str {
    match kind {
        ImportKind::Ani => ".ANI",
        ImportKind::C3dc => ".3DC",
        ImportKind::Mesh3do => ".3DO",
        ImportKind::Mesh3de => ".3DE",
    }
}

fn refused_lines(e: &ImportError) -> Refused {
    let mut lines: Vec<String> = e.list.iter().take(SHOWN).map(|r| r.to_string()).collect();
    let rest = e.list.len().saturating_sub(SHOWN) + e.more;
    if rest > 0 {
        lines.push(format!("and {rest} more"));
    }
    Refused { lines, does_not_pass: false }
}

fn one(text: impl Into<String>) -> Refused {
    Refused { lines: vec![text.into()], does_not_pass: false }
}

/// The lines of the rig check of an `.ANI` that does not pass (the differences and the validation errors).
fn rig_lines(candidate: &Ani, like: &Ani) -> Vec<String> {
    let mut v = vec!["The animation does not pass the rig check against its like file:".to_string()];
    for d in rig_check(candidate, like).iter().take(SHOWN) {
        v.push(match d {
            RigDiff::BoneCount { candidate, target } => format!("bone count: the file has {candidate}, the like file {target}"),
            RigDiff::Parent { bone, candidate, target } => format!("bone {bone}: parent is {candidate}, the like file's is {target}"),
            RigDiff::Matrix { bone } => format!("bone {bone}: the reference matrix differs"),
            RigDiff::FirstField { candidate, target } => format!("first field: {candidate}, the like file's is {target}"),
        });
    }
    for i in validate(candidate).iter().filter(|i| i.severity == Severity::Error).take(SHOWN) {
        v.push(format!("{}: {}", i.code, i.message));
    }
    v
}

fn read_like(archive: &Archive, path: &str) -> Result<Vec<u8>> {
    let entry = archive.index().get(path).ok_or_else(|| Error::format(path, None, "not in the archive"))?;
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    archive.read(path)
}

/// Reads at most `cap` bytes: a file that grows after the size check is still stopped.
fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if len > cap {
        return Err(Error::format(path.display().to_string(), None, format!("{len} bytes is over the {cap}-byte limit")));
    }
    let mut out = Vec::new();
    f.take(cap + 1).read_to_end(&mut out).map_err(|e| Error::io(path, e))?;
    if out.len() as u64 > cap {
        return Err(Error::format(path.display().to_string(), None, format!("more than {cap} bytes: over the limit")));
    }
    Ok(out)
}

fn parse_error(path: &str, e: shaiya_clientdata::Error) -> Error {
    match e {
        shaiya_clientdata::Error::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
        other => Error::format(path, None, other.to_string()),
    }
}

/// What the task does: the kind comes from the row at `path`; an `.ANI` or `.3DC` row is read as the like file.
pub fn run(archive: &Archive, path: &str, s: &Settings, cancel: shaiya_core::job::Cancel) -> std::result::Result<Made, Refused> {
    let Some(kind) = ImportKind::of_name(path) else { return Err(one("This kind of file is not imported from a model or animation file")) };
    let Some(input) = &s.input else { return Err(one("Choose the file to import")) };
    let src = Source::of(input);
    match (src, kind) {
        (Source::Gltf, _) | (Source::Obj, ImportKind::Mesh3do | ImportKind::Mesh3de) | (Source::Bvh, ImportKind::Ani) => {}
        (Source::Obj, _) => return Err(one("An OBJ is imported as a .3DO or .3DE (it holds no skin and no keys): open this from a .3DO or .3DE row")),
        (Source::Bvh, _) => return Err(one("A BVH is imported as an .ANI (it holds no mesh): open this from an .ANI row")),
    }
    if src != Source::Gltf && (s.map.is_some() || s.reduce_influences || s.reduce || s.keys_exact) {
        return Err(one("Bone names, reduce and exact keys are for a glTF file only"));
    }
    if kind != ImportKind::C3dc && s.reduce_influences {
        return Err(one("Reduce influences is for a .3DC"));
    }
    if kind != ImportKind::Ani && (s.reduce || s.keys_exact) {
        return Err(one("Reduce keys and exact keys are for an .ANI"));
    }
    if s.reduce && s.keys_exact {
        return Err(one("Reduce keys and exact keys cannot be used together (exact keeps the file's own keys)"));
    }
    let io = |e: Error| one(e.to_string());
    let in_cap = match src {
        Source::Gltf => MAX_MODEL_BYTES,
        Source::Obj => shaiya_clientdata::interchange::obj::MAX_OBJ_BYTES as u64,
        Source::Bvh => shaiya_clientdata::interchange::bvh::MAX_BVH_BYTES as u64,
    };
    let bytes = read_capped(input, in_cap).map_err(io)?;
    let g = if src == Source::Gltf {
        let folder = input.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Some(Gltf::open(&bytes, &mut |name| read_beside(&folder, name)).map_err(|(_, m)| one(format!("{}: {m}", input.display())))?)
    } else {
        None
    };
    let like = if kind.needs_like() {
        let like_bytes = read_like(archive, path).map_err(io)?;
        Some(if kind == ImportKind::Ani { Like::Ani(Ani::parse(&like_bytes, path).map_err(|e| io(parse_error(path, e)))?) } else { Like::C3dc(C3dc::parse(&like_bytes, path).map_err(|e| io(parse_error(path, e)))?) })
    } else {
        None
    };
    let map = match &s.map {
        Some(p) => {
            let text = read_capped(p, 1 << 20).map_err(io)?;
            let text = String::from_utf8(text).map_err(|_| one(format!("{}: the map file is not UTF-8 text", p.display())))?;
            parse_name_map(&text).map_err(|m| one(format!("{}: {m}", p.display())))?
        }
        None => Vec::new(),
    };
    let opts = ImportOptions { map, reduce_influences: s.reduce_influences, reduce: s.reduce, keys_exact: s.keys_exact, cancel: Some(cancel) };
    let (imp, report) = match (&g, &like) {
        (Some(g), _) => import_gltf(g, kind, like.as_ref(), &opts).map_err(|e| refused_lines(&e))?,
        (None, Some(Like::Ani(l))) => {
            let (ani, r) = import_bvh(&bytes, l, opts.cancel.as_ref()).map_err(|e| refused_lines(&e))?;
            (Imported::Ani(ani), r)
        }
        (None, _) => import_obj(&bytes, kind, opts.cancel.as_ref()).map_err(|e| refused_lines(&e))?,
    };
    if let (Imported::Ani(candidate), Some(Like::Ani(target))) = (&imp, &like) {
        if !passes(candidate, target) {
            return Err(Refused { lines: rig_lines(candidate, target), does_not_pass: true });
        }
    }
    let bytes = finish(&imp, like.as_ref()).map_err(|e| refused_lines(&e))?;
    Ok(Made { kind, bytes, report, like })
}

/// Writes `made` to `out` through the checked atomic writer: the temp file must read back as the same bytes, and read as a file that passes
/// the checks again (an `.ANI` against its like file, a `.3DC` fitting it).
fn write_checked(out: &Path, made: &Made) -> Result<()> {
    write_atomic_checked(out, &made.bytes, &|tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|x| Error::io(tmp, x))?;
        let ok = back == made.bytes
            && match (&made.kind, &made.like) {
                (ImportKind::Ani, Some(Like::Ani(like))) => Ani::parse(&back, "the new animation").is_ok_and(|c| passes(&c, like)),
                (ImportKind::C3dc, l) => C3dc::parse(&back, "the new mesh").is_ok_and(|c| finish(&Imported::C3dc(c), l.as_ref()).is_ok()),
                (ImportKind::Mesh3do, _) => Mesh3do::parse(&back, "the new mesh").is_ok(),
                (ImportKind::Mesh3de, _) => Mesh3de::parse(&back, "the new mesh").is_ok(),
                _ => false,
            };
        if ok {
            Ok(())
        } else {
            Err(Error::Unsupported("internal error (a bug): the file written does not read back as the imported file".to_string()))
        }
    })
}

/// The window of one import.
pub struct ImportGltf {
    path: String,
    archive: Arc<Archive>,
    client_dir: Option<PathBuf>,
    kind: ImportKind,
    pub settings: Settings,
    /// The settings the running or last task was started with.
    started: Option<Settings>,
    handle: Option<TaskHandle<std::result::Result<Made, Refused>>>,
    /// The last answer.
    pub result: Option<std::result::Result<Made, Refused>>,
    saving: Option<(PathBuf, TaskHandle<()>)>,
    pub open: bool,
}

impl ImportGltf {
    /// A window importing a glTF as a new file of the kind of the row at `path` of `archive`.
    pub fn new(path: String, archive: Arc<Archive>, client_dir: Option<PathBuf>) -> Option<ImportGltf> {
        let kind = ImportKind::of_name(&path)?;
        Some(ImportGltf { path, archive, client_dir, kind, settings: Settings::default(), started: None, handle: None, result: None, saving: None, open: true })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn kind(&self) -> ImportKind {
        self.kind
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// Starts the worker with the window's settings (nothing when one is running).
    pub fn start(&mut self, cx: &mut ToolkitCx<'_>) {
        if self.handle.is_some() {
            return;
        }
        let s = self.settings.clone();
        self.started = Some(s.clone());
        self.result = None;
        let (archive, path) = (self.archive.clone(), self.path.clone());
        let name = s.input.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.handle = Some(cx.shell.tasks().spawn(TaskSpec { title: format!("Import {name} as {}", label(self.kind)), cancellable: true, exclusive: None }, move |job| {
            let r = run(&archive, &path, &s, job.cancel_token().clone());
            job.check()?;
            Ok(r)
        }));
    }

    /// Takes in the worker's answer and the save's.
    pub fn poll(&mut self, cx: &mut ToolkitCx<'_>) {
        if let Some(h) = &mut self.handle {
            if let Some(outcome) = h.poll() {
                self.handle = None;
                self.result = Some(match outcome {
                    TaskOutcome::Done(r) => r,
                    TaskOutcome::Failed(e) => Err(one(e.to_string())),
                    TaskOutcome::Cancelled => Err(one("cancelled")),
                    TaskOutcome::Panicked(m) => Err(one(format!("internal error (a bug): {m}"))),
                });
            }
        }
        if let Some((out, mut h)) = self.saving.take() {
            match h.poll() {
                None => self.saving = Some((out, h)),
                Some(TaskOutcome::Done(())) => cx.shell.status(format!("Saved {} ({LOSSY_SENTENCE})", out.display())),
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Save failed", &e),
                Some(TaskOutcome::Cancelled) => cx.shell.status("Save cancelled; nothing was written"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("Save failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            }
        }
    }

    /// The finished import, when the last answer is one made with the settings now in the window.
    pub fn made(&self) -> Option<&Made> {
        match (&self.result, &self.started) {
            (Some(Ok(m)), Some(s)) if s == &self.settings => Some(m),
            _ => None,
        }
    }

    /// "Save as new file...": writes the imported file to `out`, a new file outside every client folder, never the glTF or the map. Refused
    /// (with a message) when nothing is ready, the name does not end in the kind's extension, the file exists or is inside a client folder.
    pub fn save_new(&mut self, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(made) = self.made().cloned() else {
            cx.shell.error("Save failed", &Error::Refused("There is no imported file for these settings yet".to_string()));
            return;
        };
        if self.saving.is_some() {
            return;
        }
        let want = label(self.kind).trim_start_matches('.').to_ascii_lowercase();
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sources = [self.settings.input.as_deref(), self.settings.map.as_deref()];
        let problem = if !out.extension().is_some_and(|x| x.eq_ignore_ascii_case(&want)) {
            Some(format!("The file name must end in .{}: {}", want.to_ascii_uppercase(), out.display()))
        } else if let Some(p) = out_name_problem(&name) {
            Some(format!("{}: {p}", out.display()))
        } else if sources.iter().flatten().any(|s| same_file(s, &out)) {
            Some(format!("{} is a file this import reads: choose another name", out.display()))
        } else if out.exists() {
            Some(format!("{} exists; choose a new file name", out.display()))
        } else {
            crate::export_json::refusal(&out, self.client_dir.as_deref(), &saved)
        };
        if let Some(text) = problem {
            cx.shell.error("Save failed", &Error::Refused(text));
            return;
        }
        let target = out.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Save {name}"), cancellable: false, exclusive: None }, move |_job| write_checked(&target, &made));
        self.saving = Some((out, h));
    }

    /// Draws the window; closing it drops the import.
    pub fn draw(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        let mut open = self.open;
        let (mut run_now, mut save) = (false, false);
        let name = self.path.rsplit('/').next().unwrap_or(&self.path).to_string();
        egui::Window::new("Import model/animation").open(&mut open).default_size([600.0, 420.0]).show(ctx, |ui| {
            let what = if self.kind.needs_like() { format!("A new {} like {name}: the skeleton, reference matrices and first field come from it.", label(self.kind)) } else { format!("A new {} mesh.", label(self.kind)) };
            ui.label(egui::RichText::new(what).strong());
            ui.label(egui::RichText::new(LOSSY_SENTENCE).color(pal.weak));
            ui.horizontal(|ui| {
                ui.label("File");
                let shown = self.settings.input.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(none chosen)".to_string());
                ui.label(shown);
                if ui.button("Choose...").clicked() {
                    if let Some(p) = cx.shell.pick_file("import-gltf", &[("glTF, OBJ or BVH", &["gltf", "glb", "obj", "bvh"]), ("glTF", &["gltf", "glb"]), ("OBJ", &["obj"]), ("BVH", &["bvh"])]) {
                        self.settings.input = Some(p);
                    }
                }
            });
            ui.label(egui::RichText::new(".gltf and .glb make any of the four kinds; .obj makes a .3DO or .3DE; .bvh makes an .ANI").color(pal.weak));
            let gltf_only = self.settings.input.as_deref().is_none_or(|p| Source::of(p) == Source::Gltf);
            if self.kind.needs_like() && gltf_only {
                ui.horizontal(|ui| {
                    ui.label("Bone names");
                    let shown = self.settings.map.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Bone_NNN".to_string());
                    ui.label(shown);
                    if ui.button("Map file...").on_hover_text("A text file of lines `node name = bone index` for nodes that are not named Bone_NNN").clicked() {
                        if let Some(p) = cx.shell.pick_file("import-gltf-map", &[("Text", &["txt"]), ("All files", &["*"])]) {
                            self.settings.map = Some(p);
                        }
                    }
                    if self.settings.map.is_some() && ui.button("Clear").clicked() {
                        self.settings.map = None;
                    }
                });
            }
            match (gltf_only, self.kind) {
                (false, _) => {}
                (true, ImportKind::Ani) => {
                    ui.checkbox(&mut self.settings.keys_exact, "Use the file's own keys (each a multiple of 1/30 s; no resampling)");
                    ui.checkbox(&mut self.settings.reduce, "Drop keys that the others reproduce (within 1e-5)");
                }
                (true, ImportKind::C3dc) => {
                    ui.checkbox(&mut self.settings.reduce_influences, "Keep the two largest influences of a vertex that has more");
                }
                (true, _) => {}
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(self.settings.input.is_some() && self.handle.is_none(), egui::Button::new("Import")).clicked() {
                    run_now = true;
                }
                if let Some(h) = &self.handle {
                    ui.label("Working...");
                    if ui.button("Cancel").clicked() {
                        h.cancel();
                    }
                }
            });
            match &self.result {
                None => {}
                Some(Ok(m)) => {
                    ui.label(m.summary());
                    for n in &m.report.notes {
                        ui.label(egui::RichText::new(n).color(pal.weak));
                    }
                    if m.report.influences_reduced > 0 {
                        ui.label(egui::RichText::new(format!("{} vertices had more than 2 influences; the two largest were kept (the largest weight dropped: {:.4})", m.report.influences_reduced, m.report.max_weight_dropped)).color(pal.warning));
                    }
                    if self.made().is_none() {
                        ui.label(egui::RichText::new("The settings changed: this is the result of the earlier ones.").color(pal.weak));
                    }
                }
                Some(Err(r)) => {
                    ui.label(egui::RichText::new(if r.does_not_pass { "Not imported: it does not pass the rig check" } else { "Not imported" }).color(pal.error));
                    for l in &r.lines {
                        ui.label(egui::RichText::new(l).color(pal.error));
                    }
                }
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(self.made().is_some() && self.saving.is_none(), egui::Button::new("Save as new file...")).clicked() {
                    save = true;
                }
            });
            ui.label(egui::RichText::new("Writes a new file outside the client; put it in with Replace. Validated offline: how the game plays it is not tested.").color(pal.weak));
        });
        self.open = open;
        if run_now {
            self.start(cx);
        }
        if save {
            let stem = self.path.rsplit('/').next().unwrap_or(&self.path).to_string();
            let ext = label(self.kind).trim_start_matches('.').to_string();
            if let Some(out) = cx.shell.save_file("import-gltf-save", &stem, &[("Game file", &[ext.as_str()])]) {
                self.save_new(out, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_decides_how_it_is_read() {
        for (p, s) in [("a.gltf", Source::Gltf), ("a.GLB", Source::Gltf), ("a.obj", Source::Obj), ("b.OBJ", Source::Obj), ("c.bvh", Source::Bvh), ("d.BVH", Source::Bvh), ("e", Source::Gltf)] {
            assert_eq!(Source::of(Path::new(p)), s, "{p}");
        }
    }

    #[test]
    fn a_row_decides_what_is_made() {
        for (p, k) in [("a/b.ANI", ImportKind::Ani), ("a/b.3dc", ImportKind::C3dc), ("x.3DO", ImportKind::Mesh3do), ("x.3de", ImportKind::Mesh3de)] {
            assert_eq!(ImportKind::of_name(p), Some(k), "{p}");
        }
        assert_eq!(ImportKind::of_name("a.vani"), None);
    }
}
