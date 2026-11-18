//! First-run setup: the server map folder, the client folder and an optional reference folder, with a report on what each pick
//! holds. The inspections (`inspect_server`, `inspect_client`) are plain functions of a path; the wizard window and the Preferences
//! window run them on a worker thread and show the lines they return. Nothing here writes anything.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use egui::{Color32, RichText};

// ------------------------------------------------------------------------------------------------------------------ report lines

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Info,
    Warn,
    Error,
}

impl Tone {
    pub fn colour(self, ui: &egui::Ui) -> Color32 {
        let p = crate::theme::pal_ui(ui);
        match self {
            Tone::Ok => p.ok,
            Tone::Info => p.info,
            Tone::Warn => p.warning,
            Tone::Error => p.error,
        }
    }
}

pub type Line = (Tone, String);

/// `0–67, 100–108`: the ids as ranges (en dash), ascending.
pub fn ranges_text(ids: &[u32]) -> String {
    let mut v: Vec<u32> = ids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        parts.push(if j == i { v[i].to_string() } else if j == i + 1 { format!("{}, {}", v[i], v[j]) } else { format!("{}–{}", v[i], v[j]) });
        i = j + 1;
    }
    parts.join(", ")
}

fn lower_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// `<number>.svmap`: the map id, or None.
pub fn map_id_of(name: &str) -> Option<u32> {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(".svmap")?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

fn has_child(dir: &Path, name: &str) -> bool {
    std::fs::read_dir(dir).is_ok_and(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)))
}

fn child_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name) && e.path().is_dir()).map(|e| e.path())
}

// ------------------------------------------------------------------------------------------------------------------ server folder

#[derive(Clone, Debug, Default)]
pub struct ServerReport {
    pub exists: bool,
    /// A `.svmap` file was picked, not a folder.
    pub picked_file: bool,
    /// The folder that holds the maps (the pick itself, or the folder of a picked file).
    pub folder: Option<PathBuf>,
    pub maps: Vec<u32>,
    /// `.svmap` files whose name is not a number.
    pub ignored: usize,
    /// `..\Map.ini` exists beside the folder.
    pub map_ini: bool,
    /// The folder looks like a client folder (data.sah or world\).
    pub client_like: bool,
    /// Folders below the pick that hold maps: (folder, number of maps).
    pub suggestions: Vec<(PathBuf, usize)>,
    pub search_stopped: bool,
}

impl ServerReport {
    pub fn valid(&self) -> bool {
        self.folder.is_some() && !self.maps.is_empty()
    }

    pub fn lines(&self) -> Vec<Line> {
        let mut l = Vec::new();
        if !self.exists {
            l.push((Tone::Error, "not found".to_string()));
            return l;
        }
        if self.picked_file {
            if let Some(f) = &self.folder {
                l.push((Tone::Info, format!("You picked a file; its folder is used: {}", f.display())));
            }
        }
        if self.valid() {
            l.push((Tone::Ok, format!("{} maps: {}", self.maps.len(), ranges_text(&self.maps))));
            if self.ignored > 0 {
                l.push((Tone::Info, format!("{} other .svmap file{} ignored (the name is not a number)", self.ignored, if self.ignored == 1 { "" } else { "s" })));
            }
            l.push((Tone::Info, if self.map_ini { "..\\Map.ini found (information only)".to_string() } else { "..\\Map.ini not found (information only)".to_string() }));
            return l;
        }
        if self.client_like {
            l.push((Tone::Warn, "This is a client folder; it belongs in step 2.".to_string()));
        }
        if self.suggestions.is_empty() {
            l.push((Tone::Warn, "No map files here.".to_string()));
        }
        if self.search_stopped {
            l.push((Tone::Info, "The search stopped early; pick the Map folder directly.".to_string()));
        }
        l
    }
}

/// What a server map folder pick holds. A `.svmap` file uses its folder; a folder with no `<number>.svmap` files is searched
/// below (depth 4, at most 2,000 folders) for ones that have.
pub fn inspect_server(path: &Path) -> ServerReport {
    let mut r = ServerReport { exists: path.exists(), ..Default::default() };
    if !r.exists {
        return r;
    }
    let dir = if path.is_file() {
        r.picked_file = true;
        match path.parent() {
            Some(p) => p.to_path_buf(),
            None => return r,
        }
    } else {
        path.to_path_buf()
    };
    let (maps, ignored) = map_files(&dir);
    r.map_ini = dir.parent().is_some_and(|p| has_child(p, "Map.ini"));
    if !maps.is_empty() {
        r.folder = Some(dir);
        r.maps = maps;
        r.ignored = ignored;
        return r;
    }
    r.ignored = ignored;
    r.client_like = has_child(&dir, "data.sah") || child_dir(&dir, "world").is_some();
    // search below, breadth first so the nearest hits come first
    let mut queue = std::collections::VecDeque::new();
    queue.push_back((dir.clone(), 0usize));
    let mut visited = 0usize;
    while let Some((d, depth)) = queue.pop_front() {
        if depth >= 4 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            visited += 1;
            if visited > 2000 {
                r.search_stopped = true;
                return r;
            }
            let (m, _) = map_files(&p);
            if !m.is_empty() {
                r.suggestions.push((p.clone(), m.len()));
                if r.suggestions.len() >= 3 {
                    return r;
                }
            } else {
                queue.push_back((p, depth + 1));
            }
        }
    }
    r
}

/// The `<number>.svmap` ids of a folder and how many other `.svmap` files it has.
fn map_files(dir: &Path) -> (Vec<u32>, usize) {
    let mut ids = Vec::new();
    let mut other = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().ends_with(".svmap") || !e.path().is_file() {
                continue;
            }
            match map_id_of(&name) {
                Some(id) => ids.push(id),
                None => other += 1,
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    (ids, other)
}

// ------------------------------------------------------------------------------------------------------------------ client folder

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientKind {
    /// `data.sah` + `data.saf`, read in place; the number of files in the archive.
    Archive(usize),
    /// An extracted folder with `world`.
    Extracted,
    None,
}

#[derive(Clone, Debug)]
pub struct ClientReport {
    pub exists: bool,
    pub kind: ClientKind,
    /// The folder to open as the client (the pick, or the folder of a picked `data.sah`).
    pub folder: Option<PathBuf>,
    /// Problems that make the folder unusable.
    pub errors: Vec<String>,
    /// Folders nearby that look right: (folder, what it is).
    pub suggestions: Vec<(PathBuf, String)>,
    /// The pick holds `<n>.svmap` files.
    pub server_like: bool,
    pub monsters: Option<usize>,
    pub npcs: Option<usize>,
    pub sysmsg: Option<bool>,
    pub minimaps: usize,
    /// Of the server maps given: how many have a `.wld`, and which do not. None without server maps.
    pub wld_match: Option<(usize, usize, Vec<u32>)>,
    pub notes: Vec<String>,
}

impl ClientReport {
    fn empty(path: &Path) -> ClientReport {
        ClientReport {
            exists: path.exists(),
            kind: ClientKind::None,
            folder: None,
            errors: Vec::new(),
            suggestions: Vec::new(),
            server_like: false,
            monsters: None,
            npcs: None,
            sysmsg: None,
            minimaps: 0,
            wld_match: None,
            notes: Vec::new(),
        }
    }

    pub fn valid(&self) -> bool {
        self.kind != ClientKind::None && self.errors.is_empty()
    }

    pub fn lines(&self) -> Vec<Line> {
        let mut l = Vec::new();
        if !self.exists {
            l.push((Tone::Error, "not found".to_string()));
            return l;
        }
        for n in &self.notes {
            l.push((Tone::Info, n.clone()));
        }
        for e in &self.errors {
            l.push((Tone::Error, e.clone()));
        }
        match &self.kind {
            ClientKind::Archive(n) => l.push((Tone::Ok, format!("Game archive: read in place, never written, nothing extracted. {} files.", thousands(*n)))),
            ClientKind::Extracted => l.push((Tone::Ok, "Extracted folder.".to_string())),
            ClientKind::None => {
                if self.errors.is_empty() {
                    if self.server_like {
                        l.push((Tone::Warn, "This is the server map folder; it belongs in step 1.".to_string()));
                    } else if self.suggestions.is_empty() {
                        l.push((Tone::Warn, "This is not a game folder (data.sah and data.saf) or an extracted folder (with a world folder).".to_string()));
                    }
                }
                return l;
            }
        }
        if !self.valid() {
            return l;
        }
        let count = |what: &str, n: Option<usize>| match n {
            Some(n) => (Tone::Ok, format!("{what}: {} records", thousands(n))),
            None => (Tone::Warn, format!("{what}: not found or unreadable (the checks that need it are skipped)")),
        };
        l.push(count("Monster.SData", self.monsters));
        l.push(count("NpcQuest.SData", self.npcs));
        l.push(match self.sysmsg {
            Some(true) => (Tone::Ok, "sysmsg-uni.txt: found".to_string()),
            _ => (Tone::Warn, "sysmsg-uni.txt: not found (map names fall back to \"Map N\")".to_string()),
        });
        l.push((Tone::Info, format!("{} minimap pictures", self.minimaps)));
        if let Some((have, total, missing)) = &self.wld_match {
            if missing.is_empty() {
                l.push((Tone::Ok, format!(".wld found for {have} of {total} maps")));
            } else {
                l.push((Tone::Warn, format!(".wld found for {have} of {total} maps; missing for maps {}. Those maps get no terrain and no client checks.", ranges_text(missing))));
            }
        }
        l
    }
}

pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a client folder pick holds; `server_maps` (the ids of step 1, may be empty) are matched against the client's `.wld` files.
pub fn inspect_client(path: &Path, server_maps: &[u32]) -> ClientReport {
    let mut r = ClientReport::empty(path);
    if !r.exists {
        return r;
    }
    let mut dir = path.to_path_buf();
    if path.is_file() {
        if lower_name(path) == "data.sah" {
            if let Some(p) = path.parent() {
                dir = p.to_path_buf();
                r.notes.push(format!("You picked data.sah; its folder is used: {}", dir.display()));
            }
        } else {
            r.errors.push("This is a file; pick the game folder (the one with data.sah).".to_string());
            return r;
        }
    }
    let sah = has_child(&dir, "data.sah");
    let saf = has_child(&dir, "data.saf");
    let world = child_dir(&dir, "world").is_some();
    if sah && !saf {
        r.errors.push("data.saf is missing next to data.sah".to_string());
        return r;
    }
    if saf && !sah {
        r.errors.push("data.sah is missing next to data.saf".to_string());
        return r;
    }
    if !sah && !world {
        probe_client(&dir, &mut r);
        return r;
    }
    let src = match shaiya_clientdata::ClientSource::open(&dir) {
        Ok(s) => s,
        Err(e) => {
            r.errors.push(format!("cannot open the client folder: {e}"));
            return r;
        }
    };
    let list = src.list();
    r.kind = if sah { ClientKind::Archive(list.len()) } else { ClientKind::Extracted };
    r.folder = Some(dir.clone());
    r.monsters = src.monsters().ok().map(|m| m.records.len());
    r.npcs = src.npc_quest().ok().map(|n| n.lists.iter().map(Vec::len).sum());
    r.sysmsg = Some(src.exists("sysmsg-uni.txt"));
    let mut wld_ids = std::collections::HashSet::new();
    for p in &list {
        let lower = p.to_lowercase().replace('\\', "/");
        if lower.starts_with("interface/minimap/") {
            r.minimaps += 1;
        }
        if let Some(stem) = lower.strip_prefix("world/").and_then(|s| s.strip_suffix(".wld")) {
            if let Ok(id) = stem.parse::<u32>() {
                wld_ids.insert(id);
            }
        }
    }
    if !server_maps.is_empty() {
        let missing: Vec<u32> = server_maps.iter().copied().filter(|id| !wld_ids.contains(id)).collect();
        r.wld_match = Some((server_maps.len() - missing.len(), server_maps.len(), missing));
    }
    r
}

/// The pick is neither kind of client folder: look for one nearby.
fn probe_client(dir: &Path, r: &mut ClientReport) {
    let (maps, _) = map_files(dir);
    r.server_like = !maps.is_empty();
    // the pick is a `world` folder: its parent is the client
    if lower_name(dir) == "world" {
        if let Some(p) = dir.parent() {
            r.suggestions.push((p.to_path_buf(), "its parent (the folder that contains world)".to_string()));
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        kids.sort();
        for k in kids {
            if has_child(&k, "data.sah") {
                r.suggestions.push((k.clone(), "a game folder (data.sah)".to_string()));
            } else if child_dir(&k, "world").is_some() {
                r.suggestions.push((k.clone(), "an extracted folder (world)".to_string()));
            } else if child_dir(&k, "data").is_some_and(|d| child_dir(&d, "world").is_some()) {
                if let Some(d) = child_dir(&k, "data") {
                    r.suggestions.push((d, "an extracted folder (data\\world)".to_string()));
                }
            }
            if r.suggestions.len() >= 3 {
                break;
            }
        }
    }
    if let Some(d) = child_dir(dir, "data") {
        if child_dir(&d, "world").is_some() && !r.suggestions.iter().any(|(p, _)| *p == d) {
            r.suggestions.insert(0, (d, "an extracted folder (data\\world)".to_string()));
        }
    }
    r.suggestions.truncate(3);
}

// ------------------------------------------------------------------------------------------------------------------ background jobs

/// A path inspection that runs on a worker thread; the result is kept for the text it was started for.
pub struct Inspect<T> {
    pub for_text: String,
    pending: Option<Receiver<T>>,
    pub done: Option<T>,
}

impl<T> Default for Inspect<T> {
    fn default() -> Self {
        Inspect { for_text: String::new(), pending: None, done: None }
    }
}

impl<T: Send + 'static> Inspect<T> {
    /// Starts the job unless it already ran for this text.
    pub fn request(&mut self, text: &str, ctx: Option<&egui::Context>, job: impl FnOnce() -> T + Send + 'static) {
        if self.for_text == text && (self.pending.is_some() || self.done.is_some()) {
            return;
        }
        self.for_text = text.to_string();
        self.done = None;
        let (tx, rx) = channel();
        let ctx = ctx.cloned();
        std::thread::spawn(move || {
            let v = job();
            let _ = tx.send(v);
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
        self.pending = Some(rx);
    }

    pub fn clear(&mut self) {
        *self = Inspect::default();
    }

    pub fn poll(&mut self) {
        if let Some(rx) = &self.pending {
            if let Ok(v) = rx.try_recv() {
                self.done = Some(v);
                self.pending = None;
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Blocks until the job is done (tests).
    #[cfg(test)]
    pub fn wait(&mut self) {
        if let Some(rx) = self.pending.take() {
            if let Ok(v) = rx.recv() {
                self.done = Some(v);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------ the wizard

pub struct Outcome {
    pub server: Option<PathBuf>,
    pub client: Option<PathBuf>,
    pub reference: Option<PathBuf>,
    /// "Show me around": open Help at the workflows.
    pub tour: bool,
}

pub enum WizardEvent {
    Finished(Outcome),
    /// Closed without finishing (any way); the first run counts as done.
    Closed,
}

#[derive(Default)]
pub struct Wizard {
    pub step: usize,
    pub server_text: String,
    pub client_text: String,
    pub reference_text: String,
    pub server: Inspect<ServerReport>,
    pub client: Inspect<ClientReport>,
    pub reference: Inspect<ServerReport>,
}

impl Wizard {
    /// A wizard that starts with the paths already set (Help > Setup..., Preferences).
    pub fn with_paths(server: Option<&str>, client: Option<&str>, reference: Option<&str>) -> Wizard {
        Wizard { server_text: server.unwrap_or("").to_string(), client_text: client.unwrap_or("").to_string(), reference_text: reference.unwrap_or("").to_string(), ..Default::default() }
    }

    /// Sets the path of a step (0 = server maps, 1 = client, 2 = reference) as typing or Browse would, and starts its inspection.
    pub fn set_path(&mut self, step: usize, text: &str) {
        match step {
            0 => self.server_text = text.to_string(),
            1 => self.client_text = text.to_string(),
            _ => self.reference_text = text.to_string(),
        }
        self.refresh(None);
    }

    /// Starts the inspections that are out of date for the current texts.
    pub fn refresh(&mut self, ctx: Option<&egui::Context>) {
        let st = self.server_text.trim().to_string();
        if !st.is_empty() {
            let p = PathBuf::from(&st);
            self.server.request(&st, ctx, move || inspect_server(&p));
        } else {
            self.server.clear();
        }
        let maps: Vec<u32> = self.server.done.as_ref().filter(|s| s.valid()).map(|s| s.maps.clone()).unwrap_or_default();
        let ct = self.client_text.trim().to_string();
        if !ct.is_empty() {
            let p = PathBuf::from(&ct);
            // a changed list of maps changes the .wld match, so it is part of what the job was started for
            let key = format!("{ct}|{}", ranges_text(&maps));
            self.client.request(&key, ctx, move || inspect_client(&p, &maps));
        } else {
            self.client.clear();
        }
        let rt = self.reference_text.trim().to_string();
        if !rt.is_empty() {
            let p = PathBuf::from(&rt);
            self.reference.request(&rt, ctx, move || inspect_server(&p));
        } else {
            self.reference.clear();
        }
    }

    pub fn poll(&mut self) {
        self.server.poll();
        self.client.poll();
        self.reference.poll();
    }

    #[cfg(test)]
    pub fn wait(&mut self) {
        // the client job depends on the server's result (the .wld match), so go round until nothing is left
        for _ in 0..3 {
            self.refresh(None);
            self.server.wait();
            self.refresh(None);
            self.client.wait();
            self.reference.wait();
        }
    }

    pub fn busy(&self) -> bool {
        self.server.busy() || self.client.busy() || self.reference.busy()
    }

    fn server_folder(&self) -> Option<PathBuf> {
        self.server.done.as_ref().filter(|s| s.valid()).and_then(|s| s.folder.clone())
    }

    /// Why the reference may not be used, if so.
    pub fn reference_problem(&self) -> Option<String> {
        let r = self.reference.done.as_ref()?;
        if !r.valid() {
            return Some("no map files here".to_string());
        }
        if let (Some(a), Some(b)) = (r.folder.as_ref(), self.server_folder()) {
            if same_path(a, &b) {
                return Some("the reference must not be the server map folder itself (it is for the untouched copy)".to_string());
            }
        }
        None
    }

    /// The paths to use when the wizard finishes: only inspected and valid ones.
    pub fn outcome(&self, tour: bool) -> Outcome {
        Outcome {
            server: self.server_folder(),
            client: self.client.done.as_ref().filter(|c| c.valid()).and_then(|c| c.folder.clone()),
            reference: if self.reference_problem().is_none() { self.reference.done.as_ref().filter(|s| s.valid()).and_then(|s| s.folder.clone()) } else { None },
            tour,
        }
    }

    /// Whether Next is allowed on the current step.
    pub fn can_next(&self) -> bool {
        let (text, ok) = match self.step {
            0 => (&self.server_text, self.server.done.as_ref().is_some_and(|s| s.valid())),
            1 => (&self.client_text, self.client.done.as_ref().is_some_and(|c| c.valid())),
            _ => (&self.reference_text, self.reference.done.is_some() && self.reference_problem().is_none()),
        };
        !self.busy() && (text.trim().is_empty() || ok)
    }

    /// The wizard window. Returns an event when it is done.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<WizardEvent> {
        self.poll();
        self.refresh(Some(ctx));
        let mut event = None;
        let modal = egui::Modal::new(egui::Id::unique("setup-wizard")).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.heading("Welcome to the Shaiya SVMap Editor");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("step {} of 3", self.step + 1)).weak());
                });
            });
            ui.add_space(4.0);
            match self.step {
                0 => self.step_server(ui, ctx),
                1 => self.step_client(ui, ctx),
                _ => self.step_reference(ui, ctx),
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(self.step > 0, egui::Button::new("Back")).clicked() {
                    self.step -= 1;
                }
                let last = self.step == 2;
                if last {
                    if ui.add_enabled(self.can_next(), egui::Button::new("Open the map list")).clicked() {
                        event = Some(WizardEvent::Finished(self.outcome(false)));
                    }
                    if ui.add_enabled(self.can_next(), egui::Button::new("Show me around")).clicked() {
                        event = Some(WizardEvent::Finished(self.outcome(true)));
                    }
                } else if ui.add_enabled(self.can_next(), egui::Button::new("Next")).clicked() {
                    self.step += 1;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let skip = match self.step {
                        0 => "I only open single files",
                        1 => "No client data",
                        _ => "No reference",
                    };
                    if ui.button(skip).clicked() {
                        match self.step {
                            0 => {
                                self.server_text.clear();
                                self.server.clear();
                            }
                            1 => {
                                self.client_text.clear();
                                self.client.clear();
                            }
                            _ => {
                                self.reference_text.clear();
                                self.reference.clear();
                            }
                        }
                        if self.step < 2 {
                            self.step += 1;
                        } else {
                            event = Some(WizardEvent::Finished(self.outcome(false)));
                        }
                    }
                });
            });
        });
        if modal.should_close() && event.is_none() {
            event = Some(WizardEvent::Closed);
        }
        event
    }

    fn path_row(ui: &mut egui::Ui, text: &mut String, hint: &str) -> bool {
        let mut browse = false;
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(ui.available_width() - 90.0));
            if ui.button("Browse...").clicked() {
                browse = true;
            }
        });
        browse
    }

    fn lines_ui(ui: &mut egui::Ui, lines: &[Line]) {
        for (tone, text) in lines {
            ui.label(RichText::new(text).color(tone.colour(ui)));
        }
    }

    fn step_server(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("1. The server's map files").strong());
        ui.label("Pick the folder with the server's map files, named 0.svmap, 1.svmap ... It is PS_Game\\Data\\Map in the server files.");
        if Self::path_row(ui, &mut self.server_text, "folder with 0.svmap, 1.svmap ...") {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.server_text = p.display().to_string();
            }
        }
        self.refresh(Some(ctx));
        if self.server.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("looking...");
            });
        }
        let mut use_path = None;
        if let Some(rep) = &self.server.done {
            Self::lines_ui(ui, &rep.lines());
            for (p, n) in &rep.suggestions {
                if ui.button(format!("Use {} ({n} maps)", p.display())).clicked() {
                    use_path = Some(p.display().to_string());
                }
            }
        }
        if let Some(p) = use_path {
            self.set_path(0, &p);
        }
    }

    fn step_client(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("2. The client data (optional)").strong());
        ui.label("The client gives the editor terrain, minimap, dungeon floors, monster and NPC names, and its own copy of each map for the checks. Pick your game folder (data.sah and data.saf next to game.exe), or an extracted folder (pick the folder that contains world). The game archive is read where it is. Nothing is extracted and nothing is written.");
        if Self::path_row(ui, &mut self.client_text, "game folder, or the folder that contains world") {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.client_text = p.display().to_string();
            }
        }
        self.refresh(Some(ctx));
        if self.client.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("reading the client...");
            });
        }
        let mut use_path = None;
        if let Some(rep) = &self.client.done {
            Self::lines_ui(ui, &rep.lines());
            for (p, what) in &rep.suggestions {
                if ui.button(format!("Use {} ({what})", p.display())).clicked() {
                    use_path = Some(p.display().to_string());
                }
            }
        }
        if let Some(p) = use_path {
            self.set_path(1, &p);
        }
    }

    fn step_reference(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("3. A reference folder (optional) and done").strong());
        ui.label("A second copy of the original map files to compare your work against. Leave it empty if you have no untouched copy. It must not be the folder from step 1.");
        if Self::path_row(ui, &mut self.reference_text, "folder with an untouched copy of the maps") {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                self.reference_text = p.display().to_string();
            }
        }
        self.refresh(Some(ctx));
        if self.reference.busy() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("looking...");
            });
        }
        if let Some(rep) = &self.reference.done {
            Self::lines_ui(ui, &rep.lines());
            if let Some(p) = self.reference_problem() {
                ui.label(RichText::new(p).color(Tone::Error.colour(ui)));
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Summary").strong());
        let show = |ui: &mut egui::Ui, what: &str, v: Option<String>| {
            ui.label(match v {
                Some(p) => format!("{what}: {p}"),
                None => format!("{what}: none"),
            });
        };
        let o = self.outcome(false);
        show(ui, "Server maps", o.server.map(|p| p.display().to_string()));
        show(ui, "Client data", o.client.map(|p| p.display().to_string()));
        show(ui, "Reference", o.reference.map(|p| p.display().to_string()));
        ui.label(RichText::new("Everything can be changed later in Edit > Preferences... or Help > Setup...").weak());
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let (ca, cb) = (a.canonicalize().unwrap_or_else(|_| a.to_path_buf()), b.canonicalize().unwrap_or_else(|_| b.to_path_buf()));
    ca == cb
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn touch(p: &Path) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(p, b"x").unwrap();
    }

    #[test]
    fn ranges_text_compresses_runs() {
        assert_eq!(ranges_text(&[0, 1, 2, 5]), "0–2, 5");
        assert_eq!(ranges_text(&[5, 0, 2, 1, 2]), "0–2, 5");
        assert_eq!(ranges_text(&[3, 4]), "3, 4");
        assert_eq!(ranges_text(&[]), "");
        let all: Vec<u32> = (0..=67).chain(100..=108).collect();
        assert_eq!(ranges_text(&all), "0–67, 100–108");
        assert_eq!(thousands(12345), "12,345");
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1000), "1,000");
    }

    #[test]
    fn map_file_names() {
        assert_eq!(map_id_of("35.svmap"), Some(35));
        assert_eq!(map_id_of("35.SVMAP"), Some(35));
        assert_eq!(map_id_of("35.svmap.bak"), None);
        assert_eq!(map_id_of("town.svmap"), None);
        assert_eq!(map_id_of(".svmap"), None);
    }

    #[test]
    fn setup_inspect_server_numbered_files() {
        let t = TempDir::new("srv-numbered");
        for n in [0, 1, 2, 5] {
            touch(&t.join(&format!("{n}.svmap")));
        }
        touch(&t.join("test.svmap"));
        touch(&t.join("1.svmap.bak"));
        touch(&t.join("Map.ini"));
        let r = inspect_server(t.path());
        assert!(r.valid());
        assert_eq!(r.maps, vec![0, 1, 2, 5]);
        assert_eq!(r.ignored, 1);
        let lines = r.lines();
        assert_eq!(lines[0], (Tone::Ok, "4 maps: 0–2, 5".to_string()));
        assert!(lines.iter().any(|(_, t)| t.contains("1 other .svmap file ignored")));
        // Map.ini is looked for beside the folder, not in it
        assert!(!r.map_ini);
        let inner = t.join("Map");
        std::fs::create_dir_all(&inner).unwrap();
        touch(&inner.join("7.svmap"));
        assert!(inspect_server(&inner).map_ini, "t/Map.ini sits beside t/Map");
    }

    #[test]
    fn setup_inspect_server_picked_file_uses_its_folder() {
        let t = TempDir::new("srv-file");
        touch(&t.join("3.svmap"));
        touch(&t.join("4.svmap"));
        let r = inspect_server(&t.join("3.svmap"));
        assert!(r.valid() && r.picked_file);
        assert_eq!(r.folder.as_deref(), Some(t.path()));
        assert_eq!(r.maps, vec![3, 4]);
        assert!(r.lines().iter().any(|(_, t)| t.contains("You picked a file")));
    }

    #[test]
    fn setup_inspect_server_nested_folder_is_found_at_depth_3() {
        let t = TempDir::new("srv-nested");
        let map = t.join("Server").join("PS_Game").join("Data").join("Map");
        for n in 0..3 {
            touch(&map.join(format!("{n}.svmap")));
        }
        let r = inspect_server(t.path());
        assert!(!r.valid());
        assert_eq!(r.suggestions.len(), 1);
        assert_eq!(r.suggestions[0], (map.clone(), 3));
        // too deep (depth 5) is not searched
        let deep = TempDir::new("srv-deep");
        touch(&deep.join("a").join("b").join("c").join("d").join("e").join("1.svmap"));
        assert!(inspect_server(deep.path()).suggestions.is_empty());
        let l = r.lines();
        assert!(l.iter().all(|(t, _)| *t != Tone::Ok));
    }

    #[test]
    fn setup_inspect_server_client_folder_misplaced_and_empty() {
        let t = TempDir::new("srv-client");
        touch(&t.join("data.sah"));
        let r = inspect_server(t.path());
        assert!(!r.valid() && r.client_like);
        assert!(r.lines().iter().any(|(_, t)| t.contains("client folder; it belongs in step 2")));
        let w = TempDir::new("srv-world");
        std::fs::create_dir_all(w.join("World")).unwrap();
        assert!(inspect_server(w.path()).client_like);
        let e = TempDir::new("srv-empty");
        let r = inspect_server(e.path());
        assert_eq!(r.lines(), vec![(Tone::Warn, "No map files here.".to_string())]);
        assert_eq!(inspect_server(&e.join("nope")).lines(), vec![(Tone::Error, "not found".to_string())]);
    }

    #[test]
    fn setup_inspect_client_sah_without_saf() {
        let t = TempDir::new("cli-sah");
        touch(&t.join("data.sah"));
        let r = inspect_client(t.path(), &[]);
        assert!(!r.valid());
        assert_eq!(r.errors, vec!["data.saf is missing next to data.sah".to_string()]);
        // the other way round
        let u = TempDir::new("cli-saf");
        touch(&u.join("data.saf"));
        assert_eq!(inspect_client(u.path(), &[]).errors, vec!["data.sah is missing next to data.saf".to_string()]);
    }

    #[test]
    fn setup_inspect_client_extracted_folder_and_world_pick() {
        let t = TempDir::new("cli-extracted");
        touch(&t.join("World").join("1.wld"));
        touch(&t.join("World").join("2.wld"));
        touch(&t.join("interface").join("MiniMap").join("1.tga"));
        let r = inspect_client(t.path(), &[1, 2, 3]);
        assert_eq!(r.kind, ClientKind::Extracted);
        assert!(r.valid());
        assert_eq!(r.minimaps, 1);
        assert_eq!(r.wld_match, Some((2, 3, vec![3])));
        assert_eq!(r.monsters, None);
        let lines = r.lines();
        assert!(lines.iter().any(|(_, t)| t == "Extracted folder."));
        assert!(lines.iter().any(|(tone, t)| *tone == Tone::Warn && t.contains(".wld found for 2 of 3 maps; missing for maps 3")));
        assert!(lines.iter().any(|(_, t)| t.contains("Monster.SData: not found")));
        // the world folder picked itself: its parent is offered
        let r = inspect_client(&t.join("World"), &[]);
        assert!(!r.valid());
        assert_eq!(r.suggestions.len(), 1);
        assert_eq!(r.suggestions[0].0, t.path());
        // a parent that holds the extracted folder one level down
        let p = TempDir::new("cli-parent");
        touch(&p.join("Client").join("world").join("1.wld"));
        touch(&p.join("Other").join("data.sah"));
        let r = inspect_client(p.path(), &[]);
        let offered: Vec<&PathBuf> = r.suggestions.iter().map(|(p, _)| p).collect();
        assert!(offered.contains(&&p.join("Client")) && offered.contains(&&p.join("Other")), "{offered:?}");
        // the server map folder picked as the client
        let s = TempDir::new("cli-server");
        touch(&s.join("1.svmap"));
        let r = inspect_client(s.path(), &[]);
        assert!(r.server_like);
        assert!(r.lines().iter().any(|(_, t)| t.contains("server map folder; it belongs in step 1")));
        assert_eq!(inspect_client(&s.join("none"), &[]).lines(), vec![(Tone::Error, "not found".to_string())]);
    }

    #[test]
    fn setup_inspect_client_archive_mode_counts_records() {
        let (Ok(client), Ok(originals)) = (std::env::var("SHAIYA_CLIENT"), std::env::var("SVMAP_ORIGINALS")) else {
            eprintln!("skipped: SHAIYA_CLIENT and SVMAP_ORIGINALS are not both set");
            return;
        };
        let (maps, _) = map_files(Path::new(&originals));
        let r = inspect_client(Path::new(&client), &maps);
        assert!(matches!(r.kind, ClientKind::Archive(n) if n > 1000), "{:?}", r.kind);
        assert_eq!(r.monsters, Some(2474));
        assert!(r.npcs.is_some_and(|n| n > 100));
        assert_eq!(r.sysmsg, Some(true));
        assert_eq!(r.wld_match, Some((77, 77, vec![])), "the client has a .wld for every original map");
        let text: Vec<String> = r.lines().into_iter().map(|(_, t)| t).collect();
        assert!(text.iter().any(|t| t.starts_with("Game archive: read in place")), "{text:?}");
        assert!(text.iter().any(|t| t == "Monster.SData: 2,474 records"));
        assert!(text.iter().any(|t| t == ".wld found for 77 of 77 maps"));
        // picking data.sah itself uses its folder
        let r2 = inspect_client(&Path::new(&client).join("data.sah"), &[]);
        assert!(r2.valid() && r2.notes.iter().any(|n| n.contains("You picked data.sah")));
        let s = inspect_server(Path::new(&originals));
        assert_eq!(s.maps.len(), 77);
        assert_eq!(s.lines()[0].1, "77 maps: 0–67, 100–108");
    }

    #[test]
    fn wizard_rules_for_next_and_reference() {
        let maps = TempDir::new("wiz-maps");
        touch(&maps.join("1.svmap"));
        let other = TempDir::new("wiz-other");
        touch(&other.join("1.svmap"));
        let mut w = Wizard::default();
        // an empty step can always be passed
        assert!(w.can_next());
        w.set_path(0, &maps.path().display().to_string());
        w.wait();
        assert!(w.can_next());
        w.set_path(0, &maps.join("nope").display().to_string());
        w.wait();
        assert!(!w.can_next(), "a path that does not exist stops Next");
        w.set_path(0, &maps.path().display().to_string());
        w.step = 2;
        w.set_path(2, &maps.path().display().to_string());
        w.wait();
        assert!(w.reference_problem().is_some_and(|p| p.contains("must not be the server map folder")));
        assert!(!w.can_next());
        assert!(w.outcome(false).reference.is_none());
        w.set_path(2, &other.path().display().to_string());
        w.wait();
        assert!(w.reference_problem().is_none() && w.can_next());
        let o = w.outcome(true);
        assert_eq!(o.server.as_deref(), Some(maps.path()));
        assert_eq!(o.reference.as_deref(), Some(other.path()));
        assert!(o.client.is_none() && o.tour);
    }
}
