//! `settings.json` in the editor's data folder: last folders, client path, recent files, window and panel sizes, layer toggles,
//! the unit snap, the session of the last run and the Preferences. Missing or unreadable settings give the defaults; every key is
//! optional, so a settings file of an older build loads unchanged.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// The theme is the toolkit's (same serde names). The enums and the path key moved to the library with the checks.
pub use shaiya_svmap_tools::keys::{path_key, CompareAgainst, TargetServer};
pub use shaiya_ui::theme::Theme;

/// The theme names as the editor shows them (lower case; the toolkit's own `label()` capitalises).
pub fn theme_label(t: Theme) -> &'static str {
    match t {
        Theme::Dark => "dark",
        Theme::Light => "light",
        Theme::System => "system",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layers {
    pub terrain: bool,
    pub minimap: bool,
    pub dungeon: bool,
    pub objects: bool,
    /// 3D view: placed buildings and shapes / trees (`.smod` models).
    pub buildings3d: bool,
    pub trees3d: bool,
    pub blocked: bool,
    pub grid: bool,
    pub ghosts: bool,
    pub names: bool,
    pub mob_areas: bool,
    pub npcs: bool,
    pub portals: bool,
    pub binds: bool,
    pub regions: bool,
    pub points_b: bool,
    pub section_a: bool,
    /// The records that differ from the compare reference (Changes tab), drawn over the map.
    pub compare: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Layers {
            terrain: true,
            minimap: false,
            dungeon: true,
            objects: false,
            buildings3d: true,
            trees3d: true,
            blocked: true,
            grid: true,
            ghosts: true,
            names: true,
            mob_areas: true,
            npcs: true,
            portals: true,
            binds: true,
            regions: true,
            points_b: true,
            section_a: true,
            compare: false,
        }
    }
}

/// Which maps the Problems tab lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProblemsScope {
    #[default]
    Map,
    Folder,
}

/// Sections the user locked (not selectable in the viewport).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Locks {
    pub mob_areas: bool,
    pub npcs: bool,
    pub portals: bool,
    pub binds: bool,
    pub regions: bool,
    pub points_b: bool,
    pub section_a: bool,
}

/// One open tab of the last session (maps with a file only).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionTab {
    pub path: String,
    /// 2D camera: world x, z at the centre and pixels per unit.
    pub view: [f32; 3],
    pub read_only: bool,
}

impl Default for SessionTab {
    fn default() -> Self {
        SessionTab { path: String::new(), view: [0.0, 0.0, 1.0], read_only: false }
    }
}

/// What the editor restores on the next start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub restore: bool,
    pub tabs: Vec<SessionTab>,
    pub active: usize,
    /// Tool name (`select`, `box`, `npc`, `portal`, `paint`, `measure`).
    pub tool: String,
    pub view_3d: bool,
}

impl Default for Session {
    fn default() -> Self {
        Session { restore: true, tabs: Vec::new(), active: 0, tool: "select".to_string(), view_3d: false }
    }
}

/// A saved view of a map (Ctrl+B adds one; Go to `@name` and View > Bookmarks use them).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bookmark {
    pub name: String,
    pub x: f32,
    pub z: f32,
    pub zoom: f32,
}

impl Default for Bookmark {
    fn default() -> Self {
        Bookmark { name: String::new(), x: 0.0, z: 0.0, zoom: 1.0 }
    }
}

/// A saved bulk edit rule: a name and the rule text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BulkRule {
    pub name: String,
    pub text: String,
}

/// A saved monster area: its entries, size and slab height (the Box tool stamps or draws with it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub name: String,
    /// Width (x) and depth (z) in world units.
    pub size: [f32; 2],
    /// Slab height (y2 - y1).
    pub height: f32,
    /// (monster id, count) per entry.
    pub entries: Vec<[u32; 2]>,
}

impl Default for Template {
    fn default() -> Self {
        Template { name: String::new(), size: [16.0, 16.0], height: 2.0, entries: Vec::new() }
    }
}

impl Template {
    /// The smallest width or depth a template stamps.
    pub const MIN_SIDE: f32 = 0.5;

    /// Width and depth at least [`Template::MIN_SIDE`], height at least 0 (a template from a hand-edited settings file or a flat box).
    pub fn clamped(mut self) -> Template {
        let side = |v: f32| if v.is_finite() { v.max(Template::MIN_SIDE) } else { 16.0 };
        self.size = [side(self.size[0]), side(self.size[1])];
        self.height = if self.height.is_finite() { self.height.max(0.0) } else { 2.0 };
        self
    }

    /// The monster area this template stamps with its lower corner at (x, y, z).
    pub fn area_at(&self, x: f32, y: f32, z: f32) -> shaiya_svmap::MobArea {
        let t = self.clone().clamped();
        let (size, height) = (t.size, t.height);
        shaiya_svmap::MobArea {
            min: [x, y, z],
            max: [x + size[0], y + height, z + size[1]],
            mobs: self.entries.iter().map(|e| shaiya_svmap::MobEntry { mob: e[0], count: e[1] }).collect(),
        }
    }
}


pub const SNAP_STEPS_DEFAULT: [f32; 8] = [0.1, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 64.0];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub svmap_folder: Option<String>,
    pub client_folder: Option<String>,
    /// A second copy of the original map files to compare against (Q2).
    pub reference_folder: Option<String>,
    pub first_run_done: bool,
    /// Tool hint line in the status bar and the empty-state hints.
    pub hints: bool,
    pub session: Session,
    pub recent: Vec<String>,
    pub recent_pinned: Vec<String>,
    pub window: [f32; 2],
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
    pub snap: f32,
    pub snap_steps: Vec<f32>,
    pub ui_scale: f32,
    pub theme: Theme,
    /// Seconds between autosaves of unsaved work (0 = off, at least 15).
    pub autosave_secs: u32,
    /// Timestamped backups kept per map (0 = off).
    pub backups_keep: u32,
    pub compare_against: CompareAgainst,
    pub problems_scope: ProblemsScope,
    pub layers: Layers,
    pub locks: Locks,
    pub minimap_alpha: f32,
    /// 3D view: buildings, shapes and trees farther from the camera than this are not drawn.
    pub prop_distance: f32,
    /// Bookmarks per `<folder key>#<map id>`.
    pub bookmarks: std::collections::BTreeMap<String, Vec<Bookmark>>,
    /// Spawn templates for the Box tool (Q3).
    pub templates: Vec<Template>,
    /// Monster areas outside this mean-level range are dimmed and cannot be picked (None = off).
    pub level_filter: Option<[u32; 2]>,
    /// The overview inset in the 2D viewport.
    pub overview: bool,
    /// The panel arrangement (R1): Browser, Inspector and Dock.
    pub layout: crate::layout::LayoutState,
    /// The name of the layout in use ("Simple", "Editing", "Review" or a saved one).
    pub layout_name: String,
    /// Saved layouts (the built-in ones are not stored).
    pub layouts: Vec<crate::layout::NamedLayout>,
    /// The settings of an older build were turned into a layout once.
    pub layout_migrated: bool,
    /// Frame time in the status bar.
    pub show_frame_time: bool,
    /// The statistics box of the 3D view.
    pub show_3d_stats: bool,
    /// Inspector sections the user opened or closed, by section id (a missing id uses the section's own default).
    pub open_sections: std::collections::BTreeMap<String, bool>,
    /// The Maps list shows each map's size.
    pub show_map_sizes: bool,
    /// The heatmap of monster areas (R2).
    pub heat: crate::heat::HeatSettings,
    /// Levels past a map's range by more than this are findings in the Spawns tab.
    pub spawn_tolerance: u32,
    /// Per-map level ranges that win over the derived one, by `<folder key>#<map id>`.
    pub spawn_bands: std::collections::BTreeMap<String, [u32; 2]>,
    /// Edge rulers in the 2D viewport.
    pub rulers: bool,
    /// The Scatter tool's options and monster list (R3).
    pub scatter: crate::scatter::ScatterSettings,
    /// Saved bulk edit rules (R3).
    pub bulk_rules: Vec<BulkRule>,
    /// A left drag in the 3D view that starts on the ground orbits around that point (R4).
    pub orbit_at_cursor: bool,
    /// Frame selection, Frame map, Go to and the compass ease over 200 ms in the 3D view (R4).
    pub smooth_camera: bool,
    /// World view: card positions, per folder key then map id.
    pub world_layout: std::collections::BTreeMap<String, std::collections::BTreeMap<String, [f32; 2]>>,
    /// World view: which cards and details show.
    pub world_filter: crate::world::WorldFilter,
    /// Compare folders: the other folder. Empty falls back to the reference folder.
    pub other_folder: Option<String>,
    /// The server's data folder (`Map.ini`, `MapWar.ini`, `NpcQuest.SData`, `Obelisk.ini`) for the cross-file checks.
    /// Empty: the parent of the svmap folder when it holds `Map.ini`.
    pub server_data: Option<String>,
    /// An Afterglow defs export (a folder with `mobs.psv`, or the file) for `mob-unknown-server`. Empty: the rule is skipped.
    pub afterglow_defs_path: Option<String>,
    /// Which server the cross-file checks judge for.
    pub target_server: TargetServer,
    /// The staging folder for edited client files (C1): `<folder>/data/world/<id>.wld`. Never inside the client folder.
    pub client_out: Option<String>,
    /// The code page of the names in client files: `cp949` (EP4 default), `shift_jis`, `gbk` or `latin1` (C1).
    pub client_codepage: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            svmap_folder: None,
            client_folder: None,
            reference_folder: None,
            first_run_done: false,
            hints: true,
            session: Session::default(),
            recent: Vec::new(),
            recent_pinned: Vec::new(),
            window: [1500.0, 900.0],
            left_width: 260.0,
            right_width: 330.0,
            bottom_height: 170.0,
            snap: 0.1,
            snap_steps: SNAP_STEPS_DEFAULT.to_vec(),
            ui_scale: 1.0,
            theme: Theme::Dark,
            autosave_secs: 60,
            backups_keep: 20,
            compare_against: CompareAgainst::Saved,
            problems_scope: ProblemsScope::Map,
            layers: Layers::default(),
            locks: Locks::default(),
            minimap_alpha: 0.7,
            prop_distance: 800.0,
            bookmarks: Default::default(),
            templates: Vec::new(),
            level_filter: None,
            overview: true,
            layout: crate::layout::LayoutState::simple(),
            layout_name: "Simple".to_string(),
            layouts: Vec::new(),
            layout_migrated: false,
            show_frame_time: false,
            show_3d_stats: false,
            open_sections: Default::default(),
            show_map_sizes: false,
            heat: crate::heat::HeatSettings::default(),
            spawn_tolerance: crate::spawns::DEFAULT_TOLERANCE,
            spawn_bands: Default::default(),
            rulers: false,
            scatter: crate::scatter::ScatterSettings::default(),
            bulk_rules: Vec::new(),
            orbit_at_cursor: true,
            smooth_camera: true,
            world_layout: Default::default(),
            world_filter: crate::world::WorldFilter::default(),
            other_folder: None,
            server_data: None,
            afterglow_defs_path: None,
            target_server: TargetServer::Original,
            client_out: None,
            client_codepage: "cp949".to_string(),
        }
    }
}

pub const RECENT_CAP: usize = 15;
pub const SETTINGS_FILE: &str = "settings.json";

/// The settings file inside a data folder: `settings.json` in it.
pub fn file_in(dir: &std::path::Path) -> PathBuf {
    dir.join(SETTINGS_FILE)
}


impl Settings {
    /// Reads a settings file; a missing or unreadable one gives the defaults.
    pub fn load_from(path: &std::path::Path) -> Settings {
        Settings::load_report(path).0
    }

    /// Same, and whether the file came from a build before the layouts (it has no `layout` key): its panel sizes became the
    /// Simple layout and `layout_migrated` is set.
    pub fn load_report(path: &std::path::Path) -> (Settings, bool) {
        let Ok(text) = std::fs::read_to_string(path) else { return (Settings::default(), false) };
        let Ok(mut s) = serde_json::from_str::<Settings>(&text) else { return (Settings::default(), false) };
        let had_layout = serde_json::from_str::<serde_json::Value>(&text).ok().is_some_and(|v| v.get("layout").is_some());
        let mut migrated = false;
        if !had_layout {
            s.layout = crate::layout::LayoutState::migrated(s.left_width, s.right_width, s.bottom_height);
            s.layout_name = "Simple".to_string();
            s.layout_migrated = true;
            migrated = true;
        }
        (s.sanitised(), migrated)
    }

    pub fn save_to(&self, p: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// Clamps values a hand-edited file could break.
    fn sanitised(mut self) -> Settings {
        self.ui_scale = if self.ui_scale.is_finite() { self.ui_scale.clamp(0.75, 2.0) } else { 1.0 };
        if self.autosave_secs != 0 {
            self.autosave_secs = self.autosave_secs.max(15);
        }
        self.layout = std::mem::take(&mut self.layout).sanitised();
        for l in &mut self.layouts {
            l.state = std::mem::take(&mut l.state).sanitised();
        }
        self.templates = std::mem::take(&mut self.templates).into_iter().map(Template::clamped).collect();
        self.heat = self.heat.clamped();
        self.spawn_tolerance = self.spawn_tolerance.min(80);
        self.spawn_bands.retain(|_, b| b[0] <= b[1]);
        self.scatter = std::mem::take(&mut self.scatter).clamped();
        self.bulk_rules.retain(|r| !r.name.trim().is_empty());
        for m in self.world_layout.values_mut() {
            m.retain(|_, p| p[0].is_finite() && p[1].is_finite());
        }
        if shaiya_clientdata::wld_raw::Codepage::from_key(&self.client_codepage).is_none() {
            self.client_codepage = "cp949".to_string();
        }
        self.snap_steps.retain(|s| s.is_finite() && *s > 0.0);
        if self.snap_steps.is_empty() {
            self.snap_steps = SNAP_STEPS_DEFAULT.to_vec();
        }
        self
    }

    pub fn add_recent(&mut self, path: &str) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_string());
        // pinned entries never fall off the end
        while self.recent.len() > RECENT_CAP {
            match self.recent.iter().rposition(|p| !self.recent_pinned.contains(p)) {
                Some(i) => {
                    self.recent.remove(i);
                }
                None => break,
            }
        }
    }

    pub fn toggle_pin(&mut self, path: &str) {
        if let Some(i) = self.recent_pinned.iter().position(|p| p == path) {
            self.recent_pinned.remove(i);
        } else {
            self.recent_pinned.push(path.to_string());
        }
    }

    pub fn clear_unpinned_recent(&mut self) {
        let pinned = self.recent_pinned.clone();
        self.recent.retain(|p| pinned.contains(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_and_partial_files() {
        let s = Settings::default();
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), s);
        // unknown / missing fields fall back to defaults
        let partial: Settings = serde_json::from_str(r#"{"snap": 4.0, "layers": {"grid": false}}"#).unwrap();
        assert_eq!(partial.snap, 4.0);
        assert!(!partial.layers.grid);
        assert!(partial.layers.terrain);
        assert!(serde_json::from_str::<Settings>("garbage").is_err());
    }

    #[test]
    fn recent_is_unique_and_capped() {
        let mut s = Settings::default();
        for i in 0..20 {
            s.add_recent(&format!("f{i}"));
        }
        s.add_recent("f3");
        assert_eq!(s.recent.len(), RECENT_CAP);
        assert_eq!(s.recent[0], "f3");
        assert_eq!(s.recent.iter().filter(|p| *p == "f3").count(), 1);
    }

    /// A settings file as the M3 build wrote it (before the Q1 keys existed).
    const M3_FILE: &str = r#"{
  "svmap_folder": "D:\\maps",
  "client_folder": null,
  "recent": ["D:\\maps\\1.svmap"],
  "window": [1500.0, 900.0],
  "left_width": 260.0,
  "right_width": 330.0,
  "bottom_height": 170.0,
  "snap": 1.0,
  "layers": { "terrain": true, "minimap": true },
  "locks": { "npcs": true },
  "minimap_alpha": 0.7
}"#;

    #[test]
    fn settings_old_file_loads() {
        let dir = crate::testutil::TempDir::new("settings-old");
        let f = dir.join("settings.json");
        std::fs::write(&f, M3_FILE).unwrap();
        let s = Settings::load_from(&f);
        assert_eq!(s.snap, 1.0);
        assert_eq!(s.svmap_folder.as_deref(), Some("D:\\maps"));
        assert!(s.layers.minimap && s.locks.npcs);
        // the new keys sit at their defaults
        let d = Settings::default();
        assert!(!s.first_run_done && s.hints && s.session == d.session && s.session.restore);
        assert_eq!(s.snap_steps, SNAP_STEPS_DEFAULT.to_vec());
        assert_eq!((s.ui_scale, s.theme, s.autosave_secs, s.backups_keep), (1.0, Theme::Dark, 60, 20));
        assert_eq!((s.compare_against, s.problems_scope, s.layers.compare), (CompareAgainst::Saved, ProblemsScope::Map, false));
        assert!(s.reference_folder.is_none() && s.recent_pinned.is_empty() && s.bookmarks.is_empty());
        assert!(s.templates.is_empty() && s.level_filter.is_none() && s.overview);
        // save and load round-trips, and the save leaves no .tmp behind
        let mut t = s.clone();
        t.first_run_done = true;
        t.session.tabs.push(SessionTab { path: "D:\\maps\\1.svmap".into(), view: [10.0, 20.0, 2.5], read_only: false });
        t.session.tool = "box".into();
        t.theme = Theme::System;
        t.templates.push(Template { name: "wolves".into(), size: [12.0, 10.0], height: 2.0, entries: vec![[5, 3], [6, 1]] });
        t.level_filter = Some([10, 20]);
        t.overview = false;
        t.bookmarks.insert("k#1".into(), vec![Bookmark { name: "gate".into(), x: 1.0, z: 2.0, zoom: 3.0 }]);
        t.save_to(&f).unwrap();
        assert_eq!(Settings::load_from(&f), t);
        assert!(!f.with_extension("json.tmp").exists());
    }

    /// A settings file as the Q3 build wrote it: every Q1 to Q3 key, the old panel sizes, no `layout`.
    const Q3_FILE: &str = r#"{
  "svmap_folder": "D:\\maps",
  "client_folder": "D:\\client",
  "reference_folder": null,
  "first_run_done": true,
  "hints": true,
  "recent": ["D:\\maps\\1.svmap"],
  "window": [1500.0, 900.0],
  "left_width": 281.0,
  "right_width": 345.0,
  "bottom_height": 199.0,
  "snap": 0.5,
  "ui_scale": 1.25,
  "theme": "light",
  "autosave_secs": 90,
  "backups_keep": 10,
  "compare_against": "backup",
  "problems_scope": "folder",
  "layers": { "terrain": true, "minimap": true, "compare": true },
  "locks": { "npcs": true },
  "minimap_alpha": 0.5,
  "prop_distance": 600.0,
  "bookmarks": { "k#1": [{ "name": "gate", "x": 1.0, "z": 2.0, "zoom": 3.0 }] },
  "templates": [{ "name": "wolves", "size": [12.0, 10.0], "height": 2.0, "entries": [[5, 3]] }],
  "level_filter": [10, 20],
  "overview": false
}"#;

    #[test]
    fn settings_q3_file_loads() {
        let dir = crate::testutil::TempDir::new("settings-q3");
        let f = dir.join("settings.json");
        std::fs::write(&f, Q3_FILE).unwrap();
        let (s, migrated) = Settings::load_report(&f);
        assert!(migrated, "a file without a layout is migrated");
        // the old keys are as written
        assert_eq!((s.snap, s.ui_scale, s.theme, s.autosave_secs, s.backups_keep), (0.5, 1.25, Theme::Light, 90, 10));
        assert_eq!((s.compare_against, s.problems_scope), (CompareAgainst::Backup, ProblemsScope::Folder));
        assert_eq!(s.level_filter, Some([10, 20]));
        assert!(!s.overview && s.templates.len() == 1 && s.bookmarks.len() == 1);
        // the layout is Simple with the old panel sizes, the Dock hidden, and the file is marked as migrated
        assert!(s.layout_migrated && s.layout_name == "Simple");
        assert_eq!((s.layout.browser.size, s.layout.inspector.size, s.layout.dock.size), (281.0, 345.0, 199.0));
        assert!(s.layout.browser.visible && s.layout.inspector.visible && !s.layout.dock.visible);
        assert_eq!(s.layout.browser_tab, crate::layout::BrowserTab::Maps);
        // the new keys sit at their defaults
        assert!(!s.show_frame_time && !s.show_3d_stats && s.open_sections.is_empty() && s.layouts.is_empty() && !s.show_map_sizes);
        assert_eq!(s.heat, crate::heat::HeatSettings::default());
        assert_eq!((s.spawn_tolerance, s.spawn_bands.len(), s.rulers), (5, 0, false));
        assert_eq!(s.scatter, crate::scatter::ScatterSettings::default());
        assert!(s.bulk_rules.is_empty());
        assert!(s.orbit_at_cursor && s.smooth_camera, "the 3D camera options start on");
        assert!(s.world_layout.is_empty() && s.other_folder.is_none() && s.world_filter == crate::world::WorldFilter::default());
        assert!(s.server_data.is_none() && s.afterglow_defs_path.is_none() && s.target_server == TargetServer::Original, "the server-data keys start empty");
        assert!(s.client_out.is_none() && s.client_codepage == "cp949", "the C1 keys start empty");
        // saving and loading again round-trips every new key and is not migrated twice
        let mut t = s.clone();
        t.layout.dock.visible = true;
        t.layout.dock_tab = crate::layout::DockTab::Find;
        t.layout.browser_split = true;
        t.layout.stack_ratio = 0.6;
        t.layout_name = "Mine".to_string();
        t.layouts.push(crate::layout::NamedLayout { name: "Mine".into(), state: crate::layout::LayoutState::editing() });
        t.show_frame_time = true;
        t.show_3d_stats = true;
        t.show_map_sizes = true;
        t.open_sections.insert("more-region".into(), true);
        t.open_sections.insert("height".into(), false);
        t.heat = crate::heat::HeatSettings { mode: crate::heat::Mode::Level, tile: 16, opacity: 0.4, max: Some(12.5) };
        t.spawn_tolerance = 8;
        t.spawn_bands.insert("k#1".into(), [10, 30]);
        t.rulers = true;
        t.orbit_at_cursor = false;
        t.smooth_camera = false;
        t.world_layout.entry("k".into()).or_default().insert("1".into(), [10.0, 20.5]);
        t.world_filter.dungeons = false;
        t.other_folder = Some("D:/other".into());
        t.server_data = Some("D:/server/Data".into());
        t.afterglow_defs_path = Some("D:/afterglow/data/defs-export".into());
        t.target_server = TargetServer::Afterglow;
        t.client_out = Some("D:/client-out".into());
        t.client_codepage = "shift_jis".into();
        t.scatter = crate::scatter::ScatterSettings { preset: crate::scatter::Preset::Dense, seed: 77, two_kinds: true, mobs: vec![[5, 3], [6, 1]], ..Default::default() };
        t.bulk_rules.push(BulkRule { name: "more wolves".into(), text: "mob:5 => count +1".into() });
        t.save_to(&f).unwrap();
        let (u, migrated) = Settings::load_report(&f);
        assert!(!migrated);
        assert_eq!(u, t);
        // a file that is not a Q3 file at all (garbage) gives the defaults, not a migration
        std::fs::write(&f, "garbage").unwrap();
        assert_eq!(Settings::load_report(&f), (Settings::default(), false));
    }

    #[test]
    fn hand_edited_values_are_clamped() {
        let s: Settings = serde_json::from_str(r#"{"ui_scale": 9.0, "autosave_secs": 3, "snap_steps": [0.0, -1.0]}"#).unwrap();
        let s = s.sanitised();
        assert_eq!((s.ui_scale, s.autosave_secs), (2.0, 15));
        assert_eq!(s.snap_steps, SNAP_STEPS_DEFAULT.to_vec());
    }

    #[test]
    fn pinned_recent_entries_survive_the_cap_and_clear() {
        let mut s = Settings::default();
        s.add_recent("keep");
        s.toggle_pin("keep");
        for i in 0..40 {
            s.add_recent(&format!("f{i}"));
        }
        assert_eq!(s.recent.len(), RECENT_CAP);
        assert!(s.recent.contains(&"keep".to_string()));
        s.clear_unpinned_recent();
        assert_eq!(s.recent, vec!["keep".to_string()]);
        s.toggle_pin("keep");
        assert!(s.recent_pinned.is_empty());
    }

    #[test]
    fn path_key_is_stable_and_case_blind() {
        assert_eq!(path_key("D:\\Maps"), path_key("d:\\maps"));
        assert_eq!(path_key("").len(), 16);
        // FNV-1a 64 of "a"
        assert_eq!(path_key("a"), "af63dc4c8601ec8c");
    }
}
