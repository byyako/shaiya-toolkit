//! Settings and the data folder. Generalised from the svmap editor's `settings.rs`: JSON, every key optional, values clamped on load,
//! recent files with pins (cap 15), FNV-1a `path_key`, `--data-dir`, `--no-settings`.
//!
//! Root: `SHAIYA_TOOLKIT_HOME` if set; else `portable.txt` next to the executable -> `<exe dir>/shaiya-toolkit-data`; else Windows
//! `%APPDATA%\shaiya-toolkit`, macOS `~/Library/Application Support/shaiya-toolkit`, Linux `$XDG_CONFIG_HOME/shaiya-toolkit` or
//! `~/.config/shaiya-toolkit`. Files: `shared.json` (all tools), `<tool-id>/settings.json`, `<tool-id>/log.txt`, `recovery/`, `backups/`.
//!
//! Unknown keys are kept through `#[serde(flatten)] extra` maps so an older build does not drop a newer build's keys. `shared.json` is
//! re-read and merged before each write, because several tools may run at once.

use crate::theme::Theme;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use shaiya_core::fs::write_atomic;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const RECENT_CAP: usize = 15;
pub const FORMAT: u32 = 1;
pub const WINDOW_MIN: [f32; 2] = [640.0, 400.0];
pub const WINDOW_MAX: [f32; 2] = [8192.0, 8192.0];

/// One entry of the "my client folders" list the tools share.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Client {
    pub name: String,
    pub path: String,
    pub added_utc: String,
    pub note: String,
}

/// `shared.json`: what every toolkit program shares.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shared {
    pub format: u32,
    pub theme: Theme,
    /// 0.5 to 3.0.
    pub ui_scale: f32,
    /// A font file for CJK text (overrides the system probe).
    pub extra_font: Option<String>,
    pub clients: Vec<Client>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared { format: FORMAT, theme: Theme::Dark, ui_scale: 1.0, extra_font: None, clients: Vec::new(), extra: BTreeMap::new() }
    }
}

impl Shared {
    fn sanitised(mut self) -> Shared {
        self.ui_scale = if self.ui_scale.is_finite() { self.ui_scale.clamp(0.5, 3.0) } else { 1.0 };
        self.clients.retain(|c| !c.path.trim().is_empty());
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recent {
    pub path: String,
    pub pinned: bool,
}

/// The shell's part of `<tool-id>/settings.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellSettings {
    /// Window size in points (position is not stored: multi-monitor changes put windows off screen).
    pub window: [f32; 2],
    pub maximized: bool,
    pub recent: Vec<Recent>,
    /// Last folder per purpose (`"open"`, `"export"`, ...).
    pub last_folder: BTreeMap<String, String>,
    pub activity_open: bool,
    pub activity_height: f32,
    /// Seconds between autosaves of unsaved work (0 = off, at least 15).
    pub autosave_secs: u32,
    /// Timestamped backups kept per file (0 = off).
    pub backups_keep: u32,
    pub restore_session: bool,
    pub first_run_done: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Default for ShellSettings {
    fn default() -> Self {
        ShellSettings {
            window: [1100.0, 700.0],
            maximized: false,
            recent: Vec::new(),
            last_folder: BTreeMap::new(),
            activity_open: false,
            activity_height: 160.0,
            autosave_secs: 60,
            backups_keep: 20,
            restore_session: true,
            first_run_done: false,
            extra: BTreeMap::new(),
        }
    }
}

impl ShellSettings {
    fn sanitised(mut self) -> ShellSettings {
        let clamp = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.window = [clamp(self.window[0], WINDOW_MIN[0], WINDOW_MAX[0], 1100.0), clamp(self.window[1], WINDOW_MIN[1], WINDOW_MAX[1], 700.0)];
        self.activity_height = clamp(self.activity_height, 80.0, 600.0, 160.0);
        if self.autosave_secs != 0 {
            self.autosave_secs = self.autosave_secs.max(15);
        }
        self.recent.retain(|r| !r.path.is_empty());
        self
    }

    /// Most recent first; unpinned entries beyond [`RECENT_CAP`] fall off the end, pinned ones never do.
    pub fn add_recent(&mut self, path: &str) {
        let pinned = self.recent.iter().find(|r| r.path == path).is_some_and(|r| r.pinned);
        self.recent.retain(|r| r.path != path);
        self.recent.insert(0, Recent { path: path.to_string(), pinned });
        while self.recent.len() > RECENT_CAP {
            match self.recent.iter().rposition(|r| !r.pinned) {
                Some(i) => {
                    self.recent.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn toggle_pin(&mut self, path: &str) {
        if let Some(r) = self.recent.iter_mut().find(|r| r.path == path) {
            r.pinned = !r.pinned;
        }
    }

    pub fn clear_unpinned_recent(&mut self) {
        self.recent.retain(|r| r.pinned);
    }

    /// Pinned entries first (in list order), then the rest.
    pub fn recent_sorted(&self) -> Vec<&Recent> {
        let mut v: Vec<&Recent> = self.recent.iter().filter(|r| r.pinned).collect();
        v.extend(self.recent.iter().filter(|r| !r.pinned));
        v
    }
}

/// The whole `<tool-id>/settings.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolFile<S> {
    pub shell: ShellSettings,
    pub tool: S,
    /// Keys this build does not know (kept when writing).
    pub extra: Map<String, Value>,
}

impl<S: Default> Default for ToolFile<S> {
    fn default() -> Self {
        ToolFile { shell: ShellSettings::default(), tool: S::default(), extra: Map::new() }
    }
}

impl<S: Serialize + DeserializeOwned + Default> ToolFile<S> {
    /// Reads a settings file; a missing or unreadable one gives the defaults, a damaged part falls back to its default alone.
    pub fn load(path: &Path) -> ToolFile<S> {
        let Ok(text) = std::fs::read_to_string(path) else { return ToolFile::default() };
        let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(&text) else { return ToolFile::default() };
        let shell = map.remove("shell").and_then(|v| serde_json::from_value::<ShellSettings>(v).ok()).unwrap_or_default().sanitised();
        let tool = map.remove("tool").and_then(|v| serde_json::from_value::<S>(v).ok()).unwrap_or_default();
        map.remove("format");
        ToolFile { shell, tool, extra: map }
    }

    pub fn to_json(&self) -> Result<String, String> {
        let mut map = self.extra.clone();
        map.insert("format".into(), Value::from(FORMAT));
        map.insert("shell".into(), serde_json::to_value(&self.shell).map_err(|e| e.to_string())?);
        map.insert("tool".into(), serde_json::to_value(&self.tool).map_err(|e| e.to_string())?);
        serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        write_atomic(path, self.to_json()?.as_bytes()).map_err(|e| e.to_string())
    }
}

/// Reads `shared.json` (defaults when missing or damaged) and the raw JSON it came from, the base of a later merge.
pub fn load_shared(path: &Path) -> (Shared, Value) {
    let text = std::fs::read_to_string(path).ok();
    let shared = text.as_deref().and_then(|t| serde_json::from_str::<Shared>(t).ok()).unwrap_or_default().sanitised();
    let base = serde_json::to_value(&shared).unwrap_or(Value::Null);
    (shared, base)
}

/// `v` as `Shared` would write it (so a value read from disk compares equal to one this process serialised).
fn normalised(v: &Value) -> Value {
    serde_json::from_value::<Shared>(v.clone()).ok().and_then(|s| serde_json::to_value(s.sanitised()).ok()).unwrap_or_else(|| v.clone())
}

/// Three-way merge of top-level keys: a key this process changed (differs from `base`) wins; every other key takes the value on disk
/// (another running tool may have changed it); keys that exist only on disk are kept.
pub fn merge_values(base: &Value, mine: &Value, disk: &Value) -> Value {
    let (Some(base), Some(mine)) = (base.as_object(), mine.as_object()) else { return mine.clone() };
    let mut out = disk.as_object().cloned().unwrap_or_default();
    for (k, v) in mine {
        if base.get(k) != Some(v) || !out.contains_key(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// Writes `shared.json`: re-reads the file, merges (see [`merge_values`]) and writes atomically. Returns the merged value, which is the
/// new base.
pub fn save_shared(path: &Path, mine: &Shared, base: &Value) -> Result<Value, String> {
    let mine_v = serde_json::to_value(mine).map_err(|e| e.to_string())?;
    let disk_v = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
    let merged = merge_values(base, &mine_v, &disk_v);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())?;
    Ok(normalised(&merged))
}

/// 16 hex digits of FNV-1a 64 over the lower-cased path: the key of recovery entries, backups and bookmarks.
pub fn path_key(p: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub const CURRENT: Os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Linux
    };
}

/// The environment the data folder is resolved from (separate from the process so it can be tested).
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub toolkit_home: Option<OsString>,
    /// The folder of the executable.
    pub exe_dir: Option<PathBuf>,
    /// Whether `portable.txt` exists next to the executable.
    pub portable: bool,
    pub appdata: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub home: Option<OsString>,
}

impl Env {
    pub fn from_process() -> Env {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        let portable = exe_dir.as_ref().is_some_and(|d| d.join("portable.txt").is_file());
        Env {
            toolkit_home: std::env::var_os("SHAIYA_TOOLKIT_HOME").filter(|v| !v.is_empty()),
            exe_dir,
            portable,
            appdata: std::env::var_os("APPDATA"),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()),
            home: std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
        }
    }

    /// The toolkit data root, or `None` when no folder can be determined (then nothing is stored).
    pub fn root(&self, os: Os) -> Option<PathBuf> {
        if let Some(h) = &self.toolkit_home {
            return Some(PathBuf::from(h));
        }
        if self.portable {
            if let Some(d) = &self.exe_dir {
                return Some(d.join("shaiya-toolkit-data"));
            }
        }
        match os {
            Os::Windows => self.appdata.as_ref().map(|a| PathBuf::from(a).join("shaiya-toolkit")),
            Os::Mac => self.home.as_ref().map(|h| PathBuf::from(h).join("Library").join("Application Support").join("shaiya-toolkit")),
            Os::Linux => match &self.xdg_config_home {
                Some(x) => Some(PathBuf::from(x).join("shaiya-toolkit")),
                None => self.home.as_ref().map(|h| PathBuf::from(h).join(".config").join("shaiya-toolkit")),
            },
        }
    }
}

/// The paths of one tool inside the data root.
#[derive(Clone, Debug, PartialEq)]
pub struct Paths {
    pub root: PathBuf,
    pub tool_dir: PathBuf,
}

impl Paths {
    pub fn new(root: PathBuf, tool_id: &str) -> Paths {
        let tool_dir = root.join(tool_id);
        Paths { root, tool_dir }
    }
    pub fn shared_file(&self) -> PathBuf {
        self.root.join("shared.json")
    }
    pub fn settings_file(&self) -> PathBuf {
        self.tool_dir.join("settings.json")
    }
    pub fn log_file(&self) -> PathBuf {
        self.tool_dir.join("log.txt")
    }
    pub fn recovery_dir(&self) -> PathBuf {
        self.tool_dir.join("recovery")
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.tool_dir.join("backups")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-ui-settings-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Tool {
        columns: Vec<f32>,
        mode: String,
    }

    #[test]
    fn defaults_round_trip_and_partial_files() {
        let s = Shared::default();
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Shared>(&text).unwrap(), s);
        let partial: Shared = serde_json::from_str(r#"{"ui_scale": 1.5}"#).unwrap();
        assert_eq!((partial.ui_scale, partial.theme), (1.5, Theme::Dark));
        assert!(serde_json::from_str::<Shared>("garbage").is_err());
        let f: ToolFile<Tool> = ToolFile::default();
        assert_eq!(f.shell.autosave_secs, 60);
    }

    #[test]
    fn tool_file_saves_loads_and_keeps_unknown_keys() {
        let d = tmp("toolfile");
        let p = d.join("settings.json");
        std::fs::write(&p, r#"{"format": 1, "from_the_future": {"a": 1},
            "shell": {"window": [900.0, 500.0], "recent": [{"path": "a", "pinned": true}], "newer_shell_key": 7},
            "tool": {"columns": [10.0, 20.0], "mode": "x"}}"#)
            .unwrap();
        let mut f: ToolFile<Tool> = ToolFile::load(&p);
        assert_eq!(f.shell.window, [900.0, 500.0]);
        assert_eq!(f.tool.columns, vec![10.0, 20.0]);
        assert!(f.shell.recent[0].pinned);
        assert_eq!(f.shell.extra["newer_shell_key"], 7);
        f.shell.maximized = true;
        f.save(&p).unwrap();
        let again: ToolFile<Tool> = ToolFile::load(&p);
        assert_eq!(again, f);
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(raw["from_the_future"]["a"], 1, "unknown top-level keys survive");
        assert_eq!(raw["shell"]["newer_shell_key"], 7, "unknown shell keys survive");
        assert_eq!(raw["format"], 1);
        // a damaged tool part falls back alone, a damaged file gives the defaults
        std::fs::write(&p, r#"{"shell": {"window": [900.0, 500.0]}, "tool": {"columns": "nope"}}"#).unwrap();
        let f: ToolFile<Tool> = ToolFile::load(&p);
        assert_eq!((f.shell.window, f.tool), ([900.0, 500.0], Tool::default()));
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(ToolFile::<Tool>::load(&p), ToolFile::default());
        assert_eq!(ToolFile::<Tool>::load(&d.join("missing.json")), ToolFile::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hand_edited_values_are_clamped() {
        let d = tmp("clamp");
        let p = d.join("settings.json");
        std::fs::write(&p, r#"{"shell": {"window": [10.0, 99999.0], "autosave_secs": 3, "activity_height": 5.0}}"#).unwrap();
        let f: ToolFile<Tool> = ToolFile::load(&p);
        assert_eq!(f.shell.window, [640.0, 8192.0]);
        assert_eq!((f.shell.autosave_secs, f.shell.activity_height), (15, 80.0));
        let s: Shared = serde_json::from_str(r#"{"ui_scale": 9.0}"#).unwrap();
        assert_eq!(s.sanitised().ui_scale, 3.0);
        let s: Shared = serde_json::from_str(r#"{"ui_scale": 0.1}"#).unwrap();
        assert_eq!(s.sanitised().ui_scale, 0.5);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recent_is_unique_capped_and_pins_survive() {
        let mut s = ShellSettings::default();
        s.add_recent("keep");
        s.toggle_pin("keep");
        for i in 0..40 {
            s.add_recent(&format!("f{i}"));
        }
        assert_eq!(s.recent.len(), RECENT_CAP);
        assert!(s.recent.iter().any(|r| r.path == "keep" && r.pinned));
        s.add_recent("f39");
        assert_eq!(s.recent.iter().filter(|r| r.path == "f39").count(), 1);
        assert_eq!(s.recent_sorted()[0].path, "keep", "pinned first");
        s.add_recent("keep");
        assert!(s.recent[0].pinned, "a re-opened pinned entry stays pinned");
        s.clear_unpinned_recent();
        assert_eq!(s.recent.len(), 1);
        s.toggle_pin("keep");
        assert!(!s.recent[0].pinned);
    }

    #[test]
    fn path_key_is_stable_and_case_blind() {
        assert_eq!(path_key("Z:\\Maps"), path_key("z:\\maps"));
        assert_eq!(path_key("").len(), 16);
        assert_eq!(path_key("a"), "af63dc4c8601ec8c"); // FNV-1a 64 of "a"
    }

    #[test]
    fn data_root_follows_the_documented_order() {
        let env = Env {
            toolkit_home: None,
            exe_dir: Some(PathBuf::from("/opt/tk")),
            portable: false,
            appdata: Some("C:/Users/u/AppData/Roaming".into()),
            xdg_config_home: None,
            home: Some("/home/u".into()),
        };
        assert_eq!(env.root(Os::Windows), Some(PathBuf::from("C:/Users/u/AppData/Roaming").join("shaiya-toolkit")));
        assert_eq!(env.root(Os::Mac), Some(PathBuf::from("/home/u/Library/Application Support/shaiya-toolkit")));
        assert_eq!(env.root(Os::Linux), Some(PathBuf::from("/home/u/.config/shaiya-toolkit")));
        let xdg = Env { xdg_config_home: Some("/x/cfg".into()), ..env.clone() };
        assert_eq!(xdg.root(Os::Linux), Some(PathBuf::from("/x/cfg/shaiya-toolkit")));
        let portable = Env { portable: true, ..env.clone() };
        assert_eq!(portable.root(Os::Linux), Some(PathBuf::from("/opt/tk/shaiya-toolkit-data")));
        let home = Env { toolkit_home: Some("/h".into()), ..portable };
        assert_eq!(home.root(Os::Windows), Some(PathBuf::from("/h")));
        assert_eq!(Env::default().root(Os::Windows), None);
        let p = Paths::new(PathBuf::from("/r"), "shaiya-studio");
        assert_eq!(p.settings_file(), PathBuf::from("/r/shaiya-studio/settings.json"));
        assert_eq!(p.shared_file(), PathBuf::from("/r/shared.json"));
    }

    #[test]
    fn shared_merge_keeps_what_another_tool_wrote() {
        let d = tmp("merge");
        let p = d.join("shared.json");
        let (mut mine, base) = load_shared(&p);
        // another tool writes a client and a newer key while this one is running
        std::fs::write(&p, r#"{"clients": [{"name": "us", "path": "Z:/us"}], "theme": "dark", "future_key": 5}"#).unwrap();
        // this tool changed only the scale
        mine.ui_scale = 1.5;
        let merged = save_shared(&p, &mine, &base).unwrap();
        let (now, _) = load_shared(&p);
        assert_eq!(now.ui_scale, 1.5);
        assert_eq!(now.clients.len(), 1, "the other tool's client survives");
        assert_eq!(merged["future_key"], 5);
        assert_eq!(now.extra["future_key"], 5);
        // a key this tool changed wins over the disk
        let mut mine2 = now.clone();
        mine2.theme = Theme::Light;
        let base2 = merged.clone();
        std::fs::write(&p, r#"{"clients": [], "theme": "system", "ui_scale": 1.5}"#).unwrap();
        save_shared(&p, &mine2, &base2).unwrap();
        let (end, _) = load_shared(&p);
        assert_eq!(end.theme, Theme::Light);
        assert!(end.clients.is_empty(), "an untouched key takes the disk's value");
        let _ = std::fs::remove_dir_all(&d);
    }
}
