//! "Export as JSON...": the JSON text of one `.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO` or `.3DE` file of the open archive, written to a file
//! outside every client folder. A background task reads the entry, checks its size before reading, builds the text with
//! [`shaiya_clientdata::modeljson::export`] (which reads its own text back) and writes it with `write_atomic_checked`: the temporary file
//! is read back and must import to the entry's bytes before it replaces anything. Nothing here imports a JSON file or writes into a client.
//! A whole folder is exported by [`run_folder`] (one `<path>.json` per model file under a folder the person picked, outside every client; nothing
//! that exists is replaced).

use crate::ToolkitSettings;
use shaiya_archive::{Archive, Entry};
use shaiya_clientdata::modeljson::{self, ModelFormat, MAX_MODEL_BYTES};
use shaiya_core::fs::{client_folder_above, write_atomic_checked};
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// The running exports of one Archive tab.
#[derive(Default)]
pub(crate) struct JsonExports {
    running: Vec<(String, PathBuf, TaskHandle<()>)>,
}

/// Whether the archive path names one of the six formats (any letter case).
pub fn is_model_path(path: &str) -> bool {
    ModelFormat::from_file_name(path.rsplit('/').next().unwrap_or(path)).is_some()
}

/// The default name in the save dialog: the file's name without its extension, plus `.json`.
pub(crate) fn default_name(file_name: &str) -> String {
    let stem = file_name.rsplit_once('.').map(|(s, _)| s).unwrap_or(file_name);
    format!("{stem}.json")
}

/// The refusal text when `out` would land inside the open client's folder or a saved client's folder (or a folder that does not exist).
pub(crate) fn refusal(out: &Path, client_dir: Option<&Path>, saved: &[PathBuf]) -> Option<String> {
    let mut all: Vec<PathBuf> = saved.to_vec();
    all.extend(client_dir.map(Path::to_path_buf));
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(p) = parent {
        if !p.is_dir() {
            return Some(format!("The folder does not exist: {}", p.display()));
        }
    }
    client_folder_above(out, &all).map(|dir| format!("Choose a folder outside the client: {}", dir.display()))
}

/// True when two export targets are the same file name: letter case is ignored on Windows, where `A.json` and `a.json` are one file.
fn same_target(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// True when the file name ends in `.json` (any letter case), like the CLI's `-o` rule.
pub(crate) fn is_json_name(out: &Path) -> bool {
    out.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

fn to_error(path: &str, e: modeljson::JsonError) -> Error {
    match e {
        modeljson::JsonError::Unreadable(shaiya_clientdata::Error::Format { file, what, offset }) => Error::format(file, Some(offset as u64), what),
        modeljson::JsonError::Unreadable(e) => Error::format(path, None, e.to_string()),
        e if e.is_bug() => Error::Unsupported(format!("internal error (a bug): {e}")),
        e => Error::format(path, None, e.to_string()),
    }
}

/// What the task does: size cap, read, export, write with the read-back check.
fn run(archive: &Archive, entry: &Entry, path: &str, format: ModelFormat, out: &Path) -> Result<()> {
    if u64::from(entry.length) > MAX_MODEL_BYTES {
        return Err(Error::format(path, None, format!("{} bytes is over the {MAX_MODEL_BYTES}-byte limit for a model file", entry.length)));
    }
    let bytes = archive.read_entry(entry)?;
    let text = modeljson::export(format, &bytes, path).map_err(|e| to_error(path, e))?;
    write_atomic_checked(out, text.as_bytes(), &|tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|e| Error::io(tmp, e))?;
        match modeljson::import(&back) {
            Ok((f, b)) if f == format && b == bytes => Ok(()),
            _ => Err(Error::Unsupported("internal error (a bug): the written JSON does not import to the file's bytes".to_string())),
        }
    })
}

impl JsonExports {
    pub(crate) fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    /// Checks the target against the client folders (before anything is read), then starts the task. A refusal is shown and nothing starts.
    pub(crate) fn start(&mut self, archive: Arc<Archive>, entry: Entry, path: String, out: PathBuf, client_dir: Option<&Path>, cx: &mut ToolkitCx<'_>) {
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let Some(format) = ModelFormat::from_file_name(&name) else { return };
        if !is_json_name(&out) {
            cx.shell.error("Export failed", &Error::Refused(format!("The file name must end in .json: {}", out.display())));
            return;
        }
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        if let Some(text) = refusal(&out, client_dir, &saved) {
            cx.shell.error("Export failed", &Error::Refused(text));
            return;
        }
        if self.running.iter().any(|(_, t, _)| same_target(t, &out)) {
            cx.shell.error("Export failed", &Error::Refused(format!("An export to {} is running", out.display())));
            return;
        }
        let target = out.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Export {name} as JSON"), cancellable: false, exclusive: None }, move |_job| run(&archive, &entry, &path, format, &out));
        self.running.push((name, target, h));
    }

    /// Finishes the exports that are done; returns the status line of the last success.
    pub(crate) fn poll(&mut self, cx: &mut ToolkitCx<'_>) -> Option<String> {
        let mut status = None;
        for (name, out, mut h) in std::mem::take(&mut self.running) {
            match h.poll() {
                None => self.running.push((name, out, h)),
                Some(TaskOutcome::Done(())) => {
                    let line = format!("Exported {name} to {}", out.display());
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


// ---- a whole folder ---------------------------------------------------------------------------------------------------------------

/// What a folder export did: how many files it wrote of how many it was asked for, which ones failed and why, and whether it was
/// cancelled (the files already written are kept; nothing else was started).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderReport {
    pub written: usize,
    pub total: usize,
    /// (archive path, reason).
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
    pub dir: PathBuf,
}

impl FolderReport {
    /// "Exported 3 of 4 files to <dir> (1 failed)" (or the cancelled form).
    pub fn status(&self) -> String {
        if self.cancelled {
            format!("Export cancelled: wrote {} of {} files to {}", self.written, self.total, self.dir.display())
        } else {
            format!("Exported {} of {} files to {} ({} failed)", self.written, self.total, self.dir.display(), self.failed.len())
        }
    }
}

/// The JSON file of the archive path `rel` under `dir`: the path is kept and `.json` is added to the file name (`a.ANI` and `a.3DC` stay apart).
pub(crate) fn folder_target(dir: &Path, rel: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".json");
    p.set_file_name(name);
    p
}

/// The refusal text when `dir` would be (inside) a client folder or is a file; the folder may not exist yet (it is made).
pub(crate) fn folder_refusal(dir: &Path, client_dir: Option<&Path>, saved: &[PathBuf]) -> Option<String> {
    let mut all: Vec<PathBuf> = saved.to_vec();
    all.extend(client_dir.map(Path::to_path_buf));
    if dir.exists() && !dir.is_dir() {
        return Some(format!("Not a folder: {}", dir.display()));
    }
    client_folder_above(&dir.join("x"), &all).map(|c| format!("Choose a folder outside the client: {}", c.display()))
}

/// What the worker does for a folder: each item (archive path, entry) in turn, checking the job before each. A file that does not read,
/// a name the archive writers refuse, a path part named `data.sah`, an output that exists, or a folder that turns out to lead into a client
/// is counted as failed and the rest go on. Nothing is replaced.
pub fn run_folder(archive: &Archive, items: &[(String, Entry)], dir: &Path, saved: &[PathBuf], job: &shaiya_core::Job) -> FolderReport {
    job.total(Some(items.len() as u64), shaiya_core::Unit::Files);
    let mut report = FolderReport { total: items.len(), dir: dir.to_path_buf(), ..FolderReport::default() };
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut folders: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for (path, entry) in items {
        if job.check().is_err() {
            report.cancelled = true;
            break;
        }
        match one_of_folder(archive, entry, path, dir, saved, &mut taken, &mut folders) {
            Ok(()) => report.written += 1,
            Err(why) => report.failed.push((path.clone(), why)),
        }
        job.advance(1);
    }
    report
}

fn one_of_folder(archive: &Archive, entry: &Entry, path: &str, dir: &Path, saved: &[PathBuf], taken: &mut std::collections::HashSet<String>, folders: &mut std::collections::HashSet<PathBuf>) -> std::result::Result<(), String> {
    let rel = path.replace('\\', "/");
    let Some(format) = ModelFormat::from_file_name(rel.rsplit('/').next().unwrap_or(&rel)) else { return Err("not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file".to_string()) };
    if let Some(problem) = shaiya_archive::edit::loose_name_problem(&rel) {
        return Err(format!("not written: {problem}"));
    }
    if rel.split('/').any(|part| part.eq_ignore_ascii_case("data.sah")) {
        return Err("not written: a path part named data.sah would make the output folder look like a client folder".to_string());
    }
    let target = folder_target(dir, &rel);
    if !taken.insert(target.to_string_lossy().to_ascii_lowercase()) {
        return Err("another file of this export has the same output name".to_string());
    }
    if target.exists() {
        return Err(format!("{} exists and is kept", target.display()));
    }
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
    if !folders.contains(&folder) {
        std::fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e).to_string())?;
        // the folders under DIR were made by this export, but a link that was already there could lead into a client
        if let Some(client) = client_folder_above(&target, saved) {
            return Err(format!("{} is inside the client folder {}", target.display(), client.display()));
        }
        folders.insert(folder);
    }
    run(archive, entry, path, format, &target).map_err(|e| e.to_string())
}

/// The running folder export of one Archive tab (one at a time) and the last report.
#[derive(Default)]
pub(crate) struct FolderExports {
    running: Option<TaskHandle<FolderReport>>,
    dir: PathBuf,
    total: usize,
    /// The report of the last export that ended.
    pub(crate) last: Option<FolderReport>,
    /// The failure list window is open for `last`.
    pub(crate) show_failures: bool,
}

impl FolderExports {
    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Checks `dir` against the client folders (before anything is read), then starts the task over `items`.
    pub(crate) fn start(&mut self, archive: Arc<Archive>, items: Vec<(String, Entry)>, dir: PathBuf, client_dir: Option<&Path>, cx: &mut ToolkitCx<'_>) {
        if self.running.is_some() {
            cx.shell.error("Export failed", &Error::Refused("A folder export is running".to_string()));
            return;
        }
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        if let Some(text) = folder_refusal(&dir, client_dir, &saved) {
            cx.shell.error("Export failed", &Error::Refused(text));
            return;
        }
        let mut all = saved;
        all.extend(client_dir.map(Path::to_path_buf));
        self.dir = dir.clone();
        self.total = items.len();
        self.last = None;
        self.show_failures = false;
        let title = format!("Export {} as JSON", shaiya_inspect::counted(items.len() as u64, "file", "files"));
        self.running = Some(cx.shell.tasks().spawn(TaskSpec { title, cancellable: true, exclusive: None }, move |job| Ok(run_folder(&archive, &items, &dir, &all, job))));
    }

    /// Finishes the export when it is done; returns the status line.
    pub(crate) fn poll(&mut self, cx: &mut ToolkitCx<'_>) -> Option<String> {
        let h = self.running.as_mut()?;
        let outcome = h.poll()?;
        self.running = None;
        match outcome {
            TaskOutcome::Done(report) => {
                let line = report.status();
                cx.shell.status(line.clone());
                self.show_failures = !report.failed.is_empty();
                self.last = Some(report);
                Some(line)
            }
            TaskOutcome::Failed(e) => {
                cx.shell.error("Export failed", &e);
                None
            }
            TaskOutcome::Cancelled => {
                // the worker reports a cancel as a report; this is a task that never started
                let line = format!("Export cancelled; nothing was written to {}", self.dir.display());
                cx.shell.status(line.clone());
                Some(line)
            }
            TaskOutcome::Panicked(m) => {
                cx.shell.error("Export failed", &Error::Unsupported(format!("internal error (a bug): {m}")));
                None
            }
        }
    }

    /// Asks the running export to stop after the file it is on.
    pub(crate) fn cancel(&self) {
        if let Some(h) = &self.running {
            h.cancel();
        }
    }

    /// The failure list window of the last export.
    pub(crate) fn draw_failures(&mut self, ctx: &egui::Context) {
        if !self.show_failures {
            return;
        }
        let Some(report) = &self.last else { return };
        let mut open = true;
        egui::Window::new("Export failures").open(&mut open).default_size([560.0, 320.0]).show(ctx, |ui| {
            ui.label(report.status());
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for (path, why) in &report.failed {
                    ui.label(format!("{path}: {why}"));
                }
            });
        });
        self.show_failures = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_folder_without_data_sah_still_refuses_and_a_json_name_is_checked() {
        let dir = std::env::temp_dir().join(format!("t12-refusal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("x.json");
        assert!(refusal(&out, None, &[]).is_none(), "an ordinary folder is fine");
        assert!(refusal(&out, None, std::slice::from_ref(&dir)).is_some_and(|t| t.starts_with("Choose a folder outside the client:")), "a saved folder is refused");
        assert!(is_json_name(Path::new("a.JSON")) && !is_json_name(Path::new("a.ani")) && !is_json_name(Path::new("json")));
        assert!(same_target(Path::new("a.json"), Path::new("a.json")));
        assert_eq!(same_target(Path::new("a.json"), Path::new("A.JSON")), cfg!(windows));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn archive_of(tag: &str, files: &[(&str, Vec<u8>)]) -> (shaiya_testkit::TempDir, Archive, Vec<(String, Entry)>) {
        let d = shaiya_testkit::TempDir::new(tag);
        let mut b = shaiya_testkit::SahBuilder::new(0);
        for (name, bytes) in files {
            b = b.file(name.as_bytes(), bytes);
        }
        b.write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let items: Vec<(String, Entry)> = a.index().visible().into_iter().map(|f| (f.path, f.entry.clone())).collect();
        (d, a, items)
    }

    fn files_under(d: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, v: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, v);
                } else {
                    v.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut v = Vec::new();
        if d.exists() {
            walk(d, d, &mut v);
        }
        v.sort();
        v
    }

    #[test]
    fn a_cancelled_folder_export_stops_after_the_file_it_is_on_and_counts_what_was_written() {
        let arm = shaiya_testkit::anim::arm_ani(30);
        let (_c, a, items) = archive_of("t12-w8-cancel", &[("Monster/ANI/a.ANI", arm.clone()), ("Monster/ANI/b.ANI", arm.clone()), ("Monster/ANI/c.ANI", arm)]);
        let out = shaiya_testkit::TempDir::new("t12-w8-cancel-out");
        let cancel = shaiya_core::Cancel::new();
        let progress = shaiya_core::FnProgress::new({
            let cancel = cancel.clone();
            move |done, _| {
                if done == 1 {
                    cancel.cancel();
                }
            }
        });
        let job = shaiya_core::Job::new(&progress, cancel);
        let r = run_folder(&a, &items, out.path(), &[], &job);
        assert!(r.cancelled && r.written == 1 && r.total == 3 && r.failed.is_empty(), "{r:?}");
        assert_eq!(files_under(out.path()), vec!["Monster/ANI/a.ANI.json"]);
        assert!(r.status().starts_with("Export cancelled: wrote 1 of 3 files to "), "{}", r.status());
    }

    #[test]
    fn a_folder_export_keeps_existing_files_and_reports_what_it_refuses() {
        let arm = shaiya_testkit::anim::arm_ani(30);
        let mut cut = arm.clone();
        cut.truncate(cut.len() - 3);
        let (_c, a, items) = archive_of("t12-w8-folder", &[("M/ANI/a.ANI", arm.clone()), ("M/ANI/b.ANI", arm.clone()), ("M/ANI/cut.ANI", cut), ("data.sah/x.ANI", arm.clone()), ("M/ANI/note.txt", b"x".to_vec())]);
        let out = shaiya_testkit::TempDir::new("t12-w8-folder-out");
        std::fs::create_dir_all(out.path().join("M/ANI")).unwrap();
        std::fs::write(out.path().join("M/ANI/a.ANI.json"), b"keep me").unwrap();
        let r = run_folder(&a, &items, out.path(), &[], &shaiya_core::Job::silent());
        assert!(!r.cancelled && r.written == 1 && r.total == 5, "{r:?}");
        let why = |p: &str| r.failed.iter().find(|(f, _)| f == p).map(|(_, w)| w.clone()).unwrap_or_default();
        assert!(why("M/ANI/a.ANI").contains("exists and is kept"), "{r:?}");
        assert!(why("M/ANI/cut.ANI").contains("cut.ANI"), "{r:?}");
        assert!(why("data.sah/x.ANI").contains("data.sah"), "{r:?}");
        assert!(why("M/ANI/note.txt").contains("not an .ANI"), "{r:?}");
        assert_eq!(std::fs::read(out.path().join("M/ANI/a.ANI.json")).unwrap(), b"keep me");
        assert_eq!(files_under(out.path()), vec!["M/ANI/a.ANI.json", "M/ANI/b.ANI.json"]);
        assert_eq!(r.status(), format!("Exported 1 of 5 files to {} (4 failed)", out.path().display()));
    }

    #[test]
    fn a_folder_inside_a_client_is_refused_before_anything_is_made() {
        let c = shaiya_testkit::TempDir::new("t12-w8-refuse");
        std::fs::write(c.path().join("DATA.SAH"), b"x").unwrap();
        let sub = c.path().join("deeper").join("out");
        assert!(folder_refusal(&sub, None, &[]).is_some_and(|t| t.starts_with("Choose a folder outside the client:")));
        assert!(!sub.exists());
        let other = shaiya_testkit::TempDir::new("t12-w8-refuse-ok");
        assert!(folder_refusal(&other.path().join("new"), None, &[]).is_none(), "a folder that does not exist yet is fine");
        assert!(folder_refusal(&other.path().join("new"), Some(other.path()), &[]).is_some(), "the open client's folder counts");
        let file = other.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(folder_refusal(&file, None, &[]).is_some_and(|t| t.starts_with("Not a folder")));
    }
}
