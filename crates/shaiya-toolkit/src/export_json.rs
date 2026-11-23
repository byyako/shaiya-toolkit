//! "Export as JSON...": the JSON text of one `.ANI`, `.3DC`, `.VANI`, `.MANI`, `.3DO` or `.3DE` file of the open archive, written to a file
//! outside every client folder. A background task reads the entry, checks its size before reading, builds the text with
//! [`shaiya_clientdata::modeljson::export`] (which reads its own text back) and writes it with `write_atomic_checked`: the temporary file
//! is read back and must import to the entry's bytes before it replaces anything. Nothing here imports a JSON file or writes into a client.

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
}
