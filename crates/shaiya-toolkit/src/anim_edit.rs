//! "Edit animation...": retime, trim or copy tracks of one `.ANI` of the open archive. The worker reads the file (and the source file of
//! copy tracks), runs the tool of [`shaiya_clientdata::animedit`], checks the result against its input and hands back the numbers and the
//! bytes; the window shows them as the settings change. Nothing is written until the person chooses "Save as new file..." (a new file outside
//! every client folder, written with the read-back check) or "Queue as replacement" (the file goes through the replace checks of the Archive
//! tab like any replacement and joins the queue; nothing is applied). The result is validated offline against the toolkit's own pose rules.

use crate::anim_compare::read_ani;
use crate::ToolkitSettings;
use shaiya_archive::Archive;
use shaiya_clientdata::anim::{self, passes, AniDiff};
use shaiya_clientdata::animedit::{self, Tracks};
use shaiya_clientdata::Ani;
use shaiya_core::fs::write_atomic_checked;
use shaiya_core::{Error, Result};
use shaiya_ui::{Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

/// One tool with its settings, as the worker runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    Retime { factor: f32 },
    /// The new length in seconds (30 keys are one second).
    RetimeSeconds { seconds: f32 },
    Trim { start: i32, end: i32 },
    /// Copy tracks from `source` (an archive path); `bones` empty means every bone.
    CopyTrack { source: String, bones: Vec<usize>, rotation: bool, translation: bool },
}

/// A finished edit: the input, the result and its bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Edited {
    pub input: Ani,
    pub output: Ani,
    pub bytes: Vec<u8>,
    /// How the poses of the result differ from the input (meaningful for trim and copy tracks).
    pub diff: AniDiff,
}

fn refused(e: shaiya_clientdata::Error) -> Error {
    match e {
        shaiya_clientdata::Error::Format { what, .. } => Error::Refused(what),
        other => Error::Refused(other.to_string()),
    }
}

/// What the worker does: read, edit, check against the input, build the bytes.
pub fn run(archive: &Archive, path: &str, tool: &Tool, job: &shaiya_core::Job) -> Result<Edited> {
    job.check()?;
    let input = read_ani(archive, path, "the open archive")?;
    job.check()?;
    let output = match tool {
        Tool::Retime { factor } => animedit::retime(&input, *factor),
        Tool::RetimeSeconds { seconds } => animedit::factor_for_seconds(&input, *seconds).and_then(|f| animedit::retime(&input, f)),
        Tool::Trim { start, end } => animedit::trim(&input, *start, *end),
        Tool::CopyTrack { source, bones, rotation, translation } => {
            let src = read_ani(archive, source, "the open archive")?;
            job.check()?;
            let bones: Vec<usize> = if bones.is_empty() { (0..input.bones.len()).collect() } else { bones.clone() };
            animedit::copy_tracks(&input, &src, &bones, Tracks { rotation: *rotation, translation: *translation })
        }
    }
    .map_err(refused)?;
    job.check()?;
    if !passes(&output, &input) {
        return Err(Error::Unsupported("internal error (a bug): the edited animation does not pass against its input".to_string()));
    }
    let bytes = output.try_to_bytes().map_err(refused)?;
    job.check()?;
    let diff = anim::diff_with(&input, &output, &mut || job.check().is_err()).ok_or(Error::Cancelled)?;
    Ok(Edited { input, output, bytes, diff })
}

/// The bones of a "3,4,5" or "2-4,7" list; "all" or nothing is every bone (an empty list).
pub fn parse_bones(text: &str) -> std::result::Result<Vec<usize>, String> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("all") {
        return Ok(Vec::new());
    }
    let mut v = Vec::new();
    for part in t.split(',') {
        let part = part.trim();
        let num = |s: &str| s.trim().parse::<usize>().map_err(|_| format!("\"{}\" is not a bone number", s.trim()));
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                if a > b || b - a > 70_000 {
                    return Err(format!("\"{part}\" is not a range of bones"));
                }
                v.extend(a..=b);
            }
            None => v.push(num(part)?),
        }
    }
    v.dedup();
    Ok(v)
}

/// The three tools of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Retime,
    Trim,
    CopyTrack,
}

/// The window of one edit.
pub struct AnimEdit {
    path: String,
    archive: Arc<Archive>,
    siblings: Vec<String>,
    client_dir: Option<PathBuf>,
    pub kind: Kind,
    pub by_seconds: bool,
    pub factor: String,
    pub seconds: String,
    pub start: String,
    pub end: String,
    pub source: Option<String>,
    pub bones: String,
    pub rotation: bool,
    pub translation: bool,
    /// The tool the running or last task was started with.
    started: Option<Tool>,
    handle: Option<TaskHandle<Edited>>,
    /// The tool the shown result belongs to.
    shown_for: Option<Tool>,
    /// The last result (or the error text).
    pub result: Option<std::result::Result<Edited, String>>,
    saving: Option<(PathBuf, TaskHandle<()>)>,
    queue_wanted: bool,
    pub open: bool,
}

impl AnimEdit {
    /// A window editing the `.ANI` at `path` of `archive`; `siblings` are the other `.ANI` paths copy tracks can read from.
    pub fn new(path: String, archive: Arc<Archive>, siblings: Vec<String>, client_dir: Option<PathBuf>) -> AnimEdit {
        AnimEdit {
            path,
            archive,
            siblings,
            client_dir,
            kind: Kind::Retime,
            by_seconds: false,
            factor: "1.5".to_string(),
            seconds: String::new(),
            start: "0".to_string(),
            end: String::new(),
            source: None,
            bones: "all".to_string(),
            rotation: true,
            translation: true,
            started: None,
            handle: None,
            shown_for: None,
            result: None,
            saving: None,
            queue_wanted: false,
            open: true,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// The tool the settings of the window say, or why they say none.
    pub fn wanted(&self) -> std::result::Result<Tool, String> {
        let num = |s: &str, what: &str| s.trim().replace(',', ".").parse::<f32>().map_err(|_| format!("{what}: type a number"));
        match self.kind {
            Kind::Retime if self.by_seconds => Ok(Tool::RetimeSeconds { seconds: num(&self.seconds, "Length in seconds")? }),
            Kind::Retime => Ok(Tool::Retime { factor: num(&self.factor, "Factor")? }),
            Kind::Trim => {
                let key = |s: &str, what: &str| s.trim().parse::<i32>().map_err(|_| format!("{what}: type a whole number"));
                Ok(Tool::Trim { start: key(&self.start, "Start key")?, end: key(&self.end, "End key")? })
            }
            Kind::CopyTrack => {
                let source = self.source.clone().ok_or_else(|| "Choose the animation to copy from".to_string())?;
                if !self.rotation && !self.translation {
                    return Err("Choose rotation, translation or both".to_string());
                }
                Ok(Tool::CopyTrack { source, bones: parse_bones(&self.bones)?, rotation: self.rotation, translation: self.translation })
            }
        }
    }

    /// Sets the window's settings to `tool` (tests and the window use the same fields).
    pub fn set_tool(&mut self, tool: &Tool) {
        match tool {
            Tool::Retime { factor } => {
                self.kind = Kind::Retime;
                self.by_seconds = false;
                self.factor = factor.to_string();
            }
            Tool::RetimeSeconds { seconds } => {
                self.kind = Kind::Retime;
                self.by_seconds = true;
                self.seconds = seconds.to_string();
            }
            Tool::Trim { start, end } => {
                self.kind = Kind::Trim;
                self.start = start.to_string();
                self.end = end.to_string();
            }
            Tool::CopyTrack { source, bones, rotation, translation } => {
                self.kind = Kind::CopyTrack;
                self.source = Some(source.clone());
                self.bones = if bones.is_empty() { "all".to_string() } else { bones.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",") };
                self.rotation = *rotation;
                self.translation = *translation;
            }
        }
    }

    /// Starts the worker for `tool` (nothing when one is running; the window starts the next one when it ends).
    pub fn start(&mut self, tool: Tool, cx: &mut ToolkitCx<'_>) {
        if self.handle.is_some() {
            return;
        }
        self.started = Some(tool.clone());
        let (archive, path) = (self.archive.clone(), self.path.clone());
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.handle = Some(cx.shell.tasks().spawn(TaskSpec { title: format!("Edit {name}"), cancellable: true, exclusive: None }, move |job| run(&archive, &path, &tool, job)));
    }

    /// Takes in the worker's answer and the save's.
    pub fn poll(&mut self, cx: &mut ToolkitCx<'_>) {
        if let Some(h) = &mut self.handle {
            if let Some(outcome) = h.poll() {
                self.handle = None;
                self.shown_for = self.started.clone();
                self.result = Some(match outcome {
                    TaskOutcome::Done(e) => Ok(e),
                    TaskOutcome::Failed(e) => Err(e.to_string()),
                    TaskOutcome::Cancelled => Err("cancelled".to_string()),
                    TaskOutcome::Panicked(m) => Err(format!("internal error (a bug): {m}")),
                });
            }
        }
        if let Some((out, mut h)) = self.saving.take() {
            match h.poll() {
                None => self.saving = Some((out, h)),
                Some(TaskOutcome::Done(())) => cx.shell.status(format!("Saved {}", out.display())),
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Save failed", &e),
                Some(TaskOutcome::Cancelled) => cx.shell.status("Save cancelled; nothing was written"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("Save failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            }
        }
    }

    /// The result that matches the settings now (not one of an earlier setting).
    pub fn current(&self) -> Option<&Edited> {
        match (&self.result, self.wanted(), &self.shown_for) {
            (Some(Ok(e)), Ok(w), Some(s)) if &w == s => Some(e),
            _ => None,
        }
    }

    /// "Save as new file...": writes the result to `out`, a new file outside every client folder; the temp file is read back and must read
    /// as an `.ANI` that passes against the input before it is renamed. Refused (with a message) when nothing is ready, `out` does not end
    /// in `.ANI`, exists, or is inside a client folder.
    pub fn save_new(&mut self, out: PathBuf, cx: &mut ToolkitCx<'_>) {
        let Some(e) = self.current().cloned() else {
            cx.shell.error("Save failed", &Error::Refused("There is no result yet for these settings".to_string()));
            return;
        };
        if self.saving.is_some() {
            return;
        }
        let ani_name = out.extension().is_some_and(|x| x.eq_ignore_ascii_case("ani"));
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        let problem = if !ani_name {
            Some(format!("The file name must end in .ANI: {}", out.display()))
        } else if out.exists() {
            Some(format!("{} exists; choose a new file name", out.display()))
        } else {
            crate::export_json::refusal(&out, self.client_dir.as_deref(), &saved)
        };
        if let Some(text) = problem {
            cx.shell.error("Save failed", &Error::Refused(text));
            return;
        }
        let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let target = out.clone();
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Save {name}"), cancellable: false, exclusive: None }, move |_job| write_checked(&target, &e));
        self.saving = Some((out, h));
    }

    /// Asks the Archive tab to queue the result as a replacement of the file being edited.
    pub fn want_queue(&mut self) {
        self.queue_wanted = true;
    }

    /// The staged file to queue, when "Queue as replacement" was chosen and a result is ready; the file is written into the Toolkit's own
    /// data folder (never a client) and the Archive tab runs its replace checks on it.
    pub fn take_queue(&mut self, cx: &mut ToolkitCx<'_>) -> Option<PathBuf> {
        if !std::mem::take(&mut self.queue_wanted) {
            return None;
        }
        let Some(e) = self.current().cloned() else {
            cx.shell.error("Queue failed", &Error::Refused("There is no result yet for these settings".to_string()));
            return None;
        };
        let Some(dir) = cx.shell.data_dir().map(|d| d.join("staged")) else {
            cx.shell.error("Queue failed", &Error::Refused("The Toolkit has no data folder to keep the edited file in".to_string()));
            return None;
        };
        let flat: String = self.path.chars().map(|c| if c == '/' || c == '\\' || c == ':' { '_' } else { c }).collect();
        let file = dir.join(flat);
        let saved: Vec<PathBuf> = cx.shell.shared().clients.iter().map(|c| PathBuf::from(&c.path)).collect();
        let made = match shaiya_core::fs::ensure_out_folder(&dir, &saved) {
            Ok(None) => write_checked(&file, &e),
            Ok(Some(client)) => Err(Error::Refused(format!("{} is inside the client folder {}", dir.display(), client.display()))),
            Err(x) => Err(x),
        };
        match made {
            Ok(()) => Some(file),
            Err(err) => {
                cx.shell.error("Queue failed", &err);
                None
            }
        }
    }

    /// Draws the window; closing it drops the edit.
    pub fn draw(&mut self, ctx: &egui::Context, cx: &mut ToolkitCx<'_>) {
        let pal = cx.shell.pal();
        let mut open = self.open;
        let mut save = false;
        let mut queue = false;
        egui::Window::new("Edit animation").open(&mut open).default_size([560.0, 460.0]).show(ctx, |ui| {
            ui.label(egui::RichText::new(&self.path).strong());
            ui.label(egui::RichText::new("Makes a new animation; the original is not changed. The result is checked against the original (same skeleton, same first field, no error) and validated offline: how the game plays it is not tested.").color(pal.weak));
            ui.horizontal(|ui| {
                for (k, label) in [(Kind::Retime, "Retime"), (Kind::Trim, "Trim"), (Kind::CopyTrack, "Copy tracks")] {
                    ui.selectable_value(&mut self.kind, k, label);
                }
            });
            ui.separator();
            let known = self.result.as_ref().and_then(|r| r.as_ref().ok()).map(|e| (e.input.first_field, e.input.end_key));
            match self.kind {
                Kind::Retime => {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.by_seconds, false, "Factor");
                        ui.add_enabled(!self.by_seconds, egui::TextEdit::singleline(&mut self.factor).desired_width(70.0));
                        ui.radio_value(&mut self.by_seconds, true, "Length in seconds");
                        ui.add_enabled(self.by_seconds, egui::TextEdit::singleline(&mut self.seconds).desired_width(70.0));
                    });
                    ui.label(egui::RichText::new("Factor 0.1 to 10: 2 is twice as long, 0.5 half as long. Every bone's motion is sampled again at each whole key.").color(pal.weak));
                }
                Kind::Trim => {
                    if let (true, Some((_, end))) = (self.end.is_empty(), known) {
                        self.end = end.to_string();
                    }
                    ui.horizontal(|ui| {
                        ui.label("Keep key");
                        ui.add(egui::TextEdit::singleline(&mut self.start).desired_width(70.0));
                        ui.label("to");
                        ui.add(egui::TextEdit::singleline(&mut self.end).desired_width(70.0));
                        if let Some((first, end)) = known {
                            ui.label(egui::RichText::new(format!("(the file runs from {first} to {end})")).color(pal.weak));
                        }
                    });
                }
                Kind::CopyTrack => {
                    ui.horizontal(|ui| {
                        ui.label("Copy from");
                        let shown = self.source.as_ref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_else(|| "(choose an animation)".to_string());
                        egui::ComboBox::from_id_salt("toolkit-anim-edit-source").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                for p in &self.siblings {
                                    ui.selectable_value(&mut self.source, Some(p.clone()), p.rsplit('/').next().unwrap_or(p));
                                }
                            });
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Bones");
                        ui.add(egui::TextEdit::singleline(&mut self.bones).desired_width(160.0).hint_text("all, or 3,4,7-9"));
                        ui.checkbox(&mut self.rotation, "Rotation");
                        ui.checkbox(&mut self.translation, "Translation");
                    });
                    ui.label(egui::RichText::new("Both files need the same skeleton and first field.").color(pal.weak));
                }
            }
            ui.separator();
            let wanted = self.wanted();
            match &wanted {
                Err(why) => {
                    ui.label(egui::RichText::new(why).color(pal.warning));
                }
                Ok(w) => {
                    let _ = w;
                    if self.handle.is_some() {
                        ui.label("Working...");
                    }
                }
            }
            match (&self.result, self.current().is_some()) {
                (None, _) => {}
                (Some(Err(e)), _) => {
                    ui.label(egui::RichText::new(e).color(pal.error));
                }
                (Some(Ok(e)), current) => {
                    let (ni, no) = (e.input.key_counts(), e.output.key_counts());
                    let secs = |a: &Ani| (a.end_key as f32 - a.first_field as f32) / 30.0;
                    ui.label(format!("End key {} -> {} ({:.2} s -> {:.2} s)", e.input.end_key, e.output.end_key, secs(&e.input), secs(&e.output)));
                    ui.label(format!("Rotation keys {} -> {}, translation keys {} -> {}; {} bytes", ni.0, no.0, ni.1, no.1, e.bytes.len()));
                    if matches!(self.kind, Kind::Trim | Kind::CopyTrack) {
                        let n = e.diff.bones.len();
                        let same_len = e.diff.end_key.0 == e.diff.end_key.1;
                        if self.kind == Kind::CopyTrack || same_len {
                            ui.label(format!("{} differ in pose from the original", shaiya_inspect::counted(n as u64, "bone", "bones")));
                        }
                    }
                    if !current {
                        ui.label(egui::RichText::new("The settings changed: this is the result of the earlier ones.").color(pal.weak));
                    }
                }
            }
            let ready = self.current().is_some();
            ui.horizontal(|ui| {
                if ui.add_enabled(ready && self.saving.is_none(), egui::Button::new("Save as new file...")).clicked() {
                    save = true;
                }
                if ui.add_enabled(ready, egui::Button::new("Queue as replacement")).clicked() {
                    queue = true;
                }
            });
        });
        self.open = open;
        if save {
            let name = self.path.rsplit('/').next().unwrap_or(&self.path).to_string();
            if let Some(out) = cx.shell.save_file("edit-animation", &name, &[("Animation", &["ani", "ANI"])]) {
                self.save_new(out, cx);
            }
        }
        if queue {
            self.want_queue();
        }
        // live preview: the settings changed since the last start
        if self.handle.is_none() {
            if let Ok(w) = self.wanted() {
                if self.started.as_ref() != Some(&w) {
                    self.start(w, cx);
                }
            }
        }
    }
}

/// Writes `e`'s bytes to `out` through the checked atomic writer: the temp file must read as an `.ANI` that passes against the input.
fn write_checked(out: &Path, e: &Edited) -> Result<()> {
    write_atomic_checked(out, &e.bytes, &|tmp: &Path| {
        let back = std::fs::read(tmp).map_err(|x| Error::io(tmp, x))?;
        match Ani::parse(&back, "the edited animation") {
            Ok(a) if back == e.bytes && passes(&a, &e.input) => Ok(()),
            _ => Err(Error::Unsupported("internal error (a bug): the file written does not read back as the edited animation".to_string())),
        }
    })
}
