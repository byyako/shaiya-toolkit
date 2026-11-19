//! The Sign & verify tab: key pairs, signing patches and updater executables, verifying signatures (and, against a client, that a
//! patch has been applied). Shaiya Toolkit never stores key contents; the secret key's path is remembered only when asked.

use crate::clients::ClientInfo;
use crate::keys::KeyForm;
use crate::ToolkitSettings;
use egui::RichText;
use shaiya_core::fs::Existing;
use shaiya_core::Error;
use shaiya_patch::sign;
use shaiya_ui::{Confirm, Cx, TaskHandle, TaskOutcome, TaskSpec};
use std::path::{Path, PathBuf};

type ToolkitCx<'a> = Cx<'a, ToolkitSettings>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Patch,
    Updater,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Sign,
    Verify,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Valid,
    Invalid(String),
    Missing,
    /// Signed or failed (for the sign table).
    Signed(PathBuf),
    Failed(String),
}

impl Outcome {
    pub fn text(&self) -> String {
        match self {
            Outcome::Valid => "valid".into(),
            Outcome::Invalid(why) => format!("invalid: {why}"),
            Outcome::Missing => "missing".into(),
            Outcome::Signed(p) => format!("signed ({})", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            Outcome::Failed(why) => format!("failed: {why}"),
        }
    }
}

type Results = Vec<(PathBuf, Outcome)>;

enum SignFlow {
    Idle,
    /// Some `.sig` files exist: "Replace signature" / "Cancel".
    Confirm(Vec<PathBuf>),
    Running(TaskHandle<Results>),
}

/// The key pair made last: the public key to show with its warnings.
pub struct Generated {
    pub public_hex: String,
    pub secret: PathBuf,
    pub public: PathBuf,
}

pub struct SignTab {
    pub mode: Mode,
    pub updater_version: String,
    pub sign_files: Vec<PathBuf>,
    pub verify_files: Vec<PathBuf>,
    pub drop_target: Section,
    sign_flow: SignFlow,
    pub sign_results: Results,
    verify_handle: Option<TaskHandle<Results>>,
    pub verify_results: Results,
    pub generated: Option<Generated>,
    pub error: Option<String>,
    pub verify_mode: Mode,
    pub verify_updater_version: String,
    pub verify_with_client: bool,
}

impl Default for SignTab {
    fn default() -> Self {
        Self::new()
    }
}

fn name_for(mode: Mode, version: &str) -> Result<Option<String>, String> {
    match mode {
        Mode::Patch => Ok(None),
        Mode::Updater => version.trim().parse::<u32>().map(|v| Some(sign::updater_name(v))).map_err(|_| "enter the updater version (a number)".to_string()),
    }
}

impl SignTab {
    pub fn new() -> SignTab {
        SignTab {
            mode: Mode::Patch,
            updater_version: String::new(),
            sign_files: Vec::new(),
            verify_files: Vec::new(),
            drop_target: Section::Sign,
            sign_flow: SignFlow::Idle,
            sign_results: Vec::new(),
            verify_handle: None,
            verify_results: Vec::new(),
            generated: None,
            error: None,
            verify_mode: Mode::Patch,
            verify_updater_version: String::new(),
            verify_with_client: false,
        }
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.sign_flow, SignFlow::Idle) || self.verify_handle.is_some()
    }

    /// Files dropped on the tab go to the section last used.
    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        let list = match self.drop_target {
            Section::Sign => &mut self.sign_files,
            Section::Verify => &mut self.verify_files,
        };
        for f in files {
            if !list.contains(&f) {
                list.push(f);
            }
        }
    }

    pub fn add_to(&mut self, section: Section, files: Vec<PathBuf>) {
        self.drop_target = section;
        self.add_files(files);
    }

    // ---------------------------------------------------------------------------------------- keys

    /// Writes a new key pair (never over an existing file) and shows the public key.
    pub fn generate_keys(&mut self, secret: PathBuf, public: PathBuf, keys: &mut KeyForm, cx: &mut ToolkitCx<'_>) {
        match sign::write_key_pair(&secret, &public) {
            Ok(hex) => {
                self.error = None;
                keys.secret_path = secret.display().to_string();
                keys.public_path = public.display().to_string();
                keys.public_hex = hex.clone();
                keys.store(cx.settings);
                cx.shell.status("Key pair written");
                self.generated = Some(Generated { public_hex: hex, secret, public });
            }
            Err(e) => {
                self.error = Some(e.to_string());
                cx.shell.error("The key pair was not written", &e);
            }
        }
    }

    // ---------------------------------------------------------------------------------------- sign

    /// Why Sign is not possible yet.
    pub fn sign_blocked(&self, keys: &KeyForm) -> Option<String> {
        if self.sign_files.is_empty() {
            return Some("Add the files to sign".into());
        }
        if keys.secret_path.trim().is_empty() {
            return Some("Choose the secret key file".into());
        }
        if keys.check_version_number().is_none() {
            return Some("Enter the CheckVersion (a number)".into());
        }
        if let Err(e) = name_for(self.mode, &self.updater_version) {
            return Some(e);
        }
        if self.is_busy() {
            return Some("Busy".into());
        }
        None
    }

    pub fn begin_sign(&mut self, keys: &KeyForm, cx: &mut ToolkitCx<'_>) {
        if self.sign_blocked(keys).is_some() {
            return;
        }
        let existing: Vec<PathBuf> = self.sign_files.iter().map(|f| sign::signature_path(f)).filter(|p| p.exists()).collect();
        if existing.is_empty() {
            self.start_sign(keys, Existing::Refuse, cx);
        } else {
            self.sign_flow = SignFlow::Confirm(existing);
        }
    }

    fn start_sign(&mut self, keys: &KeyForm, existing: Existing, cx: &mut ToolkitCx<'_>) {
        let secret_path = PathBuf::from(keys.secret_path.trim());
        let version = keys.check_version_number().unwrap_or(0);
        let files = self.sign_files.clone();
        let Ok(name) = name_for(self.mode, &self.updater_version) else { return };
        let h = cx.shell.tasks().spawn(TaskSpec { title: format!("Signing {} file{}", files.len(), if files.len() == 1 { "" } else { "s" }), cancellable: true, exclusive: None }, move |job| {
            let secret = sign::read_key(&secret_path)?;
            let mut out: Results = Vec::new();
            for f in &files {
                job.check()?;
                out.push((f.clone(), match sign::sign_file(&secret, version, f, name.as_deref(), existing, job) {
                    Ok(p) => Outcome::Signed(p),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => Outcome::Failed(e.to_string()),
                }));
            }
            Ok(out)
        });
        self.sign_flow = SignFlow::Running(h);
    }

    // ---------------------------------------------------------------------------------------- verify

    pub fn verify_blocked(&self, keys: &KeyForm) -> Option<String> {
        if self.verify_files.is_empty() {
            return Some("Add the files to verify".into());
        }
        if keys.verify_key().is_none() {
            return Some("Set the public key (file or hex) and the CheckVersion".into());
        }
        if let Err(e) = name_for(self.verify_mode, &self.verify_updater_version) {
            return Some(e);
        }
        if self.is_busy() {
            return Some("Busy".into());
        }
        None
    }

    /// Verifies the listed files; with `client`, patches are also checked against the client folder ("applied").
    pub fn begin_verify(&mut self, keys: &KeyForm, client: Option<&Path>, cx: &mut ToolkitCx<'_>) {
        if self.verify_blocked(keys).is_some() {
            return;
        }
        let Some(key) = keys.verify_key() else { return };
        let Ok(name) = name_for(self.verify_mode, &self.verify_updater_version) else { return };
        let files = self.verify_files.clone();
        let client = client.map(Path::to_path_buf);
        let h = cx.shell.tasks().spawn(TaskSpec { title: "Verifying signatures".into(), cancellable: true, exclusive: None }, move |job| {
            let mut out: Results = Vec::new();
            for f in &files {
                job.check()?;
                let sig = sign::signature_path(f);
                let mut outcome = if !sig.exists() {
                    Outcome::Missing
                } else {
                    match sign::verify_file(&key.public_hex, key.check_version, f, name.as_deref(), job) {
                        Ok(()) => Outcome::Valid,
                        Err(Error::Cancelled) => return Err(Error::Cancelled),
                        Err(e) => Outcome::Invalid(e.to_string()),
                    }
                };
                if let (Some(dir), Outcome::Valid, None) = (&client, &outcome, &name) {
                    match shaiya_patch::verify(f, Some(&key), Some(dir), job) {
                        Ok(r) => {
                            if let Some(na) = r.not_applied.filter(|n| !n.is_empty()) {
                                outcome = Outcome::Invalid(format!("valid signature, but not applied to this client ({}, first: {})", shaiya_inspect::counted(na.len() as u64, "difference", "differences"), na[0]));
                            }
                        }
                        Err(Error::Cancelled) => return Err(Error::Cancelled),
                        Err(e) => outcome = Outcome::Invalid(e.to_string()),
                    }
                }
                out.push((f.clone(), outcome));
            }
            Ok(out)
        });
        self.verify_handle = Some(h);
    }

    // ---------------------------------------------------------------------------------------- polling

    pub fn tick(&mut self, keys: &KeyForm, cx: &mut ToolkitCx<'_>) {
        match std::mem::replace(&mut self.sign_flow, SignFlow::Idle) {
            SignFlow::Idle => {}
            SignFlow::Confirm(existing) => {
                let names: Vec<String> = existing.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
                let c = Confirm {
                    title: "Replace signature".into(),
                    text: format!("{} already exist{}. Replace {}? A published patch number must never be signed again with another key or CheckVersion.", names.join(", "), if names.len() == 1 { "s" } else { "" }, if names.len() == 1 { "it" } else { "them" }),
                    buttons: vec!["Replace signature".into(), "Cancel".into()],
                    danger: Some(0),
                };
                match cx.shell.confirm("toolkit-sign-exists", &c) {
                    None => self.sign_flow = SignFlow::Confirm(existing),
                    Some(0) => self.start_sign(keys, Existing::Overwrite, cx),
                    Some(_) => {}
                }
            }
            SignFlow::Running(mut h) => match h.poll() {
                None => self.sign_flow = SignFlow::Running(h),
                Some(TaskOutcome::Done(r)) => {
                    let ok = r.iter().filter(|(_, o)| matches!(o, Outcome::Signed(_))).count();
                    cx.shell.status(format!("Signed {ok} of {}", shaiya_inspect::counted((r.len()) as u64, "file", "files")));
                    self.sign_results = r;
                }
                Some(TaskOutcome::Failed(e)) => cx.shell.error("Signing failed", &e),
                Some(TaskOutcome::Cancelled) => cx.shell.status("Cancelled; signatures already written stay"),
                Some(TaskOutcome::Panicked(m)) => cx.shell.error("Signing failed", &Error::Unsupported(format!("internal error (a bug): {m}"))),
            },
        }
        if let Some(h) = &mut self.verify_handle {
            match h.poll() {
                None => {}
                Some(TaskOutcome::Done(r)) => {
                    self.verify_results = r;
                    self.verify_handle = None;
                }
                Some(TaskOutcome::Failed(e)) => {
                    cx.shell.error("Verification failed", &e);
                    self.verify_handle = None;
                }
                Some(_) => self.verify_handle = None,
            }
        }
    }

    // ---------------------------------------------------------------------------------------- drawing

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolkitCx<'_>, client: Option<&ClientInfo>, keys: &mut KeyForm) {
        keys.load(client, cx.settings);
        self.tick(keys, cx);
        let pal = cx.shell.pal();
        let mut changed = false;
        let mut gen = false;
        let mut sign_go = false;
        let mut verify_go = false;
        let mut add: Option<Section> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new("Optional. The original updater does not check signatures.").color(pal.weak));
            ui.heading("Keys");
            ui.horizontal(|ui| {
                if ui.button("Generate key pair...").clicked() {
                    gen = true;
                }
                ui.label(RichText::new("Two files: the secret key (keep it off the patch server, backed up) and the public key (share it with whoever checks your patches).").color(pal.weak));
            });
            if let Some(g) = &self.generated {
                ui.group(|ui| {
                    ui.label(format!("Secret key: {}", g.secret.display()));
                    ui.label(format!("Public key file: {}", g.public.display()));
                    ui.horizontal(|ui| {
                        ui.label("Public key:");
                        ui.add(egui::Label::new(RichText::new(&g.public_hex).monospace()).selectable(true));
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(g.public_hex.clone());
                        }
                    });
                    ui.label(RichText::new("Give the public key to whoever checks your patches. Keep the secret key off the patch server and keep a backup.").color(pal.warning));
                    if cfg!(windows) {
                        ui.label(RichText::new("Windows does not restrict who can read the secret key file here: store it in a folder only you can open.").color(pal.warning));
                    }
                });
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(pal.error));
            }
            ui.add_space(8.0);
            ui.separator();
            ui.heading("Sign");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.mode, Mode::Patch, "Patch");
                ui.radio_value(&mut self.mode, Mode::Updater, "Updater executable");
                if self.mode == Mode::Updater {
                    ui.label("UpdaterVersion");
                    ui.add(egui::TextEdit::singleline(&mut self.updater_version).desired_width(60.0));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Secret key file");
                changed |= ui.add(egui::TextEdit::singleline(&mut keys.secret_path).desired_width(360.0)).changed();
                if ui.button("Choose...").clicked() {
                    if let Some(p) = cx.shell.pick_file("secret-key", &[("Key files", &["key", "txt"]), ("All files", &["*"])]) {
                        keys.secret_path = p.display().to_string();
                        changed = true;
                    }
                }
                changed |= ui.checkbox(&mut keys.remember_secret, "Remember where my secret key is").changed();
            });
            ui.horizontal(|ui| {
                ui.label("CheckVersion");
                changed |= ui.add(egui::TextEdit::singleline(&mut keys.check_version).desired_width(60.0)).changed();
                ui.label(RichText::new("the value of the target server's updater (Version.ini CheckVersion)").color(pal.weak));
            });
            file_list(ui, "Files to sign", &mut self.sign_files, Section::Sign, &mut add, &mut self.drop_target);
            ui.horizontal(|ui| {
                let blocked = self.sign_blocked(keys);
                if ui.add_enabled(blocked.is_none(), egui::Button::new("Sign")).clicked() {
                    sign_go = true;
                }
                if let Some(why) = blocked {
                    if !self.sign_files.is_empty() {
                        ui.label(RichText::new(why).color(pal.weak));
                    }
                }
            });
            for (f, o) in &self.sign_results {
                let color = if matches!(o, Outcome::Signed(_)) { pal.ok } else { pal.error };
                ui.label(RichText::new(format!("{}: {}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), o.text())).color(color));
            }
            ui.add_space(8.0);
            ui.separator();
            ui.heading("Verify");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.verify_mode, Mode::Patch, "Patch");
                ui.radio_value(&mut self.verify_mode, Mode::Updater, "Updater executable");
                if self.verify_mode == Mode::Updater {
                    ui.label("UpdaterVersion");
                    ui.add(egui::TextEdit::singleline(&mut self.verify_updater_version).desired_width(60.0));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Public key file");
                changed |= ui.add(egui::TextEdit::singleline(&mut keys.public_path).desired_width(360.0)).changed();
                if ui.button("Choose...").clicked() {
                    if let Some(p) = cx.shell.pick_file("public-key", &[("Key files", &["key", "pub", "txt"]), ("All files", &["*"])]) {
                        keys.public_path = p.display().to_string();
                        changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("or public key (hex)");
                changed |= ui.add(egui::TextEdit::singleline(&mut keys.public_hex).desired_width(460.0)).changed();
            });
            file_list(ui, "Files to verify", &mut self.verify_files, Section::Verify, &mut add, &mut self.drop_target);
            ui.horizontal(|ui| {
                let blocked = self.verify_blocked(keys);
                if ui.add_enabled(blocked.is_none(), egui::Button::new("Verify")).clicked() {
                    verify_go = true;
                }
                if ui.add_enabled(blocked.is_none() && client.is_some() && self.verify_mode == Mode::Patch, egui::Button::new("Verify against client")).on_hover_text("Also checks that the patch is applied to the open client").clicked() {
                    verify_go = true;
                    self.verify_with_client = true;
                }
                if let Some(why) = blocked {
                    if !self.verify_files.is_empty() {
                        ui.label(RichText::new(why).color(pal.weak));
                    }
                }
            });
            for (f, o) in &self.verify_results {
                let color = match o {
                    Outcome::Valid => pal.ok,
                    Outcome::Missing => pal.warning,
                    _ => pal.error,
                };
                ui.label(RichText::new(format!("{}: {}", f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), o.text())).color(color));
            }
        });
        if changed {
            keys.store(cx.settings);
        }
        if let Some(s) = add {
            let files = cx.shell.pick_files("sign-files", &[("All files", &["*"])]);
            self.add_to(s, files);
        }
        if gen {
            if let Some(secret) = cx.shell.save_file("keygen-secret", "secret.key", &[("Key file", &["key"])]) {
                if let Some(public) = cx.shell.save_file("keygen-public", "public.key", &[("Key file", &["key"])]) {
                    self.generate_keys(secret, public, keys, cx);
                }
            }
        }
        if sign_go {
            self.begin_sign(keys, cx);
        }
        if verify_go {
            let dir = if std::mem::take(&mut self.verify_with_client) { client.map(|c| c.dir.clone()) } else { None };
            self.begin_verify(keys, dir.as_deref(), cx);
        }
    }
}

fn file_list(ui: &mut egui::Ui, title: &str, files: &mut Vec<PathBuf>, section: Section, add: &mut Option<Section>, target: &mut Section) {
    ui.horizontal(|ui| {
        ui.label(title);
        if ui.button("Add...").clicked() {
            *add = Some(section);
            *target = section;
        }
        if ui.add_enabled(!files.is_empty(), egui::Button::new("Clear")).clicked() {
            files.clear();
        }
        let sel = *target == section;
        if ui.selectable_label(sel, "drops go here").clicked() {
            *target = section;
        }
    });
    let mut remove = None;
    for (i, f) in files.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(f.display().to_string());
            if ui.small_button("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        files.remove(i);
    }
}
